//! `aocctl preset ...`: save, list, show, apply and delete user presets.
//! JSON output follows the rest of the CLI (`"schema": 1`, non-zero exit on failure).

use crate::describe;
use aocctl::models::{Domain, Setting, normalize};
use aocctl::monitor::{self, Monitor};
use aocctl::presets::{self, ModelKey, Outcome, Preset, Store};
use clap::Subcommand;
use serde_json::{Value, json};
use std::path::Path;

#[derive(Subcommand)]
pub enum PresetCmd {
    /// List the presets saved for this monitor's model.
    List,
    /// Show a preset's values.
    Show { name: String },
    /// Save a preset from the monitor's current values, or from SETTING=VALUE pairs.
    Save {
        name: String,
        /// Values to store without reading the monitor, e.g. `brightness=10 low-blue=office`.
        /// They're added to the preset if it already exists.
        #[arg(value_name = "SETTING=VALUE")]
        values: Vec<String>,
        /// Capture only these settings (comma-separated) instead of all writable ones.
        #[arg(long, value_delimiter = ',', conflicts_with = "values")]
        only: Vec<String>,
    },
    /// Write a preset's values to the monitor; values already set are left alone.
    Apply { name: String },
    /// Delete a preset.
    Delete { name: String },
}

/// Runs a preset command for the selected monitor. Ok(false) means something it reports failed.
pub fn run(as_json: bool, root: &Path, m: &Monitor, cmd: &PresetCmd) -> Result<bool, String> {
    let key = ModelKey::of(&m.connector.edid)?;
    let path = presets::default_path()?;
    let base = || json!({ "schema": 1, "display": m.connector.name, "model": key.to_string() });
    match cmd {
        PresetCmd::List => {
            let store = Store::load(&path)?;
            let list = store.presets(&key);
            if as_json {
                let mut out = base();
                out["presets"] = list.iter().map(preset_json).collect();
                println!("{out}");
            } else if list.is_empty() {
                println!("No presets for {key} yet. Save one with `aocctl preset save <name>`.");
            } else {
                for p in &list {
                    match p.problems.as_slice() {
                        [] => println!("{:<20} {}", p.name, summary(&p.values)),
                        probs => println!("{:<20} invalid: {}", p.name, probs.join("; ")),
                    }
                }
            }
            Ok(true)
        }
        PresetCmd::Show { name } => {
            let p = Store::load(&path)?.find(&key, name)?;
            if as_json {
                let mut out = base();
                out["preset"] = preset_json(&p);
                println!("{out}");
            } else {
                for (s, v) in &p.values {
                    println!("{:<16} {}", s.id, describe(s, *v));
                }
                for problem in &p.problems {
                    println!("invalid: {problem}");
                }
            }
            Ok(true)
        }
        PresetCmd::Save { name, values, only } => {
            presets::check_name(name)?;
            let (values, skipped, merge) = if values.is_empty() {
                let (kept, skipped) = capture(m, &key, only)?;
                (kept, skipped, false)
            } else {
                (parse_assignments(&key, values)?, Vec::new(), true)
            };
            let mut store = Store::edit(&path)?;
            let (p, created) = store.put(&key, name, &values, merge)?;
            store.write()?;
            if as_json {
                let mut out = base();
                out["preset"] = preset_json(&p);
                out["created"] = json!(created);
                out["skipped"] = skipped
                    .iter()
                    .map(|(s, v)| json!({ "id": s.id, "value": v, "reason": "not a value aocctl can set" }))
                    .collect();
                println!("{out}");
            } else {
                let verb = if created { "Saved" } else { "Updated" };
                println!("{verb} preset '{}' for {key}: {}", p.name, summary(&p.values));
                for (s, v) in &skipped {
                    eprintln!(
                        "aocctl: left out {} = {v}: not a value aocctl can set (it may follow from another setting)",
                        s.id
                    );
                }
            }
            Ok(true)
        }
        PresetCmd::Apply { name } => {
            let p = Store::load(&path)?.find(&key, name)?;
            let values = p.applicable()?;
            let mut ddc = m.open().map_err(|e| e.to_string())?;
            monitor::recheck(root, m)?;
            let results = presets::apply(&mut ddc, values);
            let ok = results.iter().all(|(_, _, o)| o.ok());
            if as_json {
                let rows: Vec<Value> = results.iter().map(|(s, v, o)| apply_json(s, *v, o)).collect();
                let mut out = base();
                out["preset"] = json!(p.name);
                out["settings"] = Value::Array(rows);
                println!("{out}");
            } else {
                for (s, v, o) in &results {
                    let status = match o {
                        Outcome::Unchanged => "already set".to_string(),
                        Outcome::Verified => "set".to_string(),
                        Outcome::Mismatch(got) => format!("FAILED: reads back {}", describe(s, *got)),
                        Outcome::Unknown(e) => format!("FAILED: sent, but couldn't read it back: {e}"),
                        Outcome::Failed(e) => format!("FAILED: {e}"),
                    };
                    println!("{:<16} {:<16} {status}", s.id, describe(s, *v));
                }
            }
            Ok(ok)
        }
        PresetCmd::Delete { name } => {
            let mut store = Store::edit(&path)?;
            let deleted = store.remove(&key, name)?;
            store.write()?;
            if as_json {
                let mut out = base();
                out["deleted"] = json!(deleted);
                println!("{out}");
            } else {
                println!("Deleted preset '{deleted}' for {key}");
            }
            Ok(true)
        }
    }
}

/// Reads the values `preset save` captures: all writable settings, or the `--only` ones.
#[allow(clippy::type_complexity)]
fn capture(
    m: &Monitor,
    key: &ModelKey,
    only: &[String],
) -> Result<(Vec<(&'static Setting, u16)>, Vec<(&'static Setting, u16)>), String> {
    let settings: Vec<&'static Setting> = if only.is_empty() {
        key.settings().into_iter().filter(|s| s.verified_write).collect()
    } else {
        let mut picked: Vec<&'static Setting> = Vec::new();
        for id in only {
            let s = key.writable(id)?;
            if !picked.iter().any(|p| p.id == s.id) {
                picked.push(s);
            }
        }
        picked
    };
    let mut ddc = m.open().map_err(|e| e.to_string())?;
    let (kept, skipped) = presets::capture(&mut ddc, &settings)?;
    if kept.is_empty() {
        return Err("none of the settings has a value aocctl can save".into());
    }
    Ok((kept, skipped))
}

/// Parses `setting=value` pairs with the same checks as `aocctl set`.
fn parse_assignments(key: &ModelKey, pairs: &[String]) -> Result<Vec<(&'static Setting, u16)>, String> {
    let mut out: Vec<(&'static Setting, u16)> = Vec::new();
    for pair in pairs {
        let (id, value) = pair
            .split_once('=')
            .ok_or_else(|| format!("'{pair}' isn't SETTING=VALUE, e.g. brightness=10"))?;
        let s = key.writable(id.trim())?;
        if out.iter().any(|(o, _)| o.id == s.id) {
            return Err(format!("{} is given more than once", s.id));
        }
        out.push((s, s.parse_value(value.trim())?));
    }
    Ok(out)
}

/// `{"name", "settings": [{"id", "value", "label"}], "error"}`; `error` is null when the preset can be applied.
fn preset_json(p: &Preset) -> Value {
    let settings: Vec<Value> = p
        .values
        .iter()
        .map(|(s, v)| json!({ "id": s.id, "value": v, "label": s.label_for(*v) }))
        .collect();
    let error = (!p.problems.is_empty()).then(|| p.problems.join("; "));
    json!({ "name": p.name, "settings": settings, "error": error })
}

fn apply_json(s: &Setting, v: u16, o: &Outcome) -> Value {
    let (status, read_back, error) = match o {
        Outcome::Unchanged => ("unchanged", Some(v), None),
        Outcome::Verified => ("verified", Some(v), None),
        Outcome::Mismatch(got) => ("mismatch", Some(*got), None),
        Outcome::Unknown(e) => ("unknown", None, Some(e.as_str())),
        Outcome::Failed(e) => ("error", None, Some(e.as_str())),
    };
    json!({
        "id": s.id, "value": v, "label": s.label_for(v),
        "status": status, "read_back": read_back, "error": error,
    })
}

/// `brightness=10 low-blue=office`, the same form `preset save` takes.
fn summary(values: &[(&'static Setting, u16)]) -> String {
    values
        .iter()
        .map(|(s, v)| match (s.domain, s.label_for(*v)) {
            (Domain::Enum(_), Some(l)) => format!("{}={}", s.id, normalize(l)),
            _ => format!("{}={v}", s.id),
        })
        .collect::<Vec<_>>()
        .join(" ")
}
