//! `aocctl` command line. Every command takes `--json` for scripts and
//! desktop-shell widgets; JSON output always carries `"schema": 1`.

use aocctl::ddc::{DdcError, Reading};
use aocctl::detect::SYSFS_DRM;
use aocctl::models::{self, Domain, Setting};
use aocctl::monitor::{self, Monitor, WriteStatus};
use clap::{Parser, Subcommand};
use serde_json::{Value, json};
use std::path::Path;
use std::process::ExitCode;

mod preset_cli;

#[derive(Parser)]
#[command(name = "aocctl", version, about = "Control AOC monitor settings over DDC/CI")]
struct Cli {
    /// Print JSON instead of text.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// List connected monitors and whether aocctl can control them.
    List,
    /// Read settings (all of the monitor's settings when none are named).
    Get {
        settings: Vec<String>,
        /// Connector name (e.g. DP-1) or serial number.
        #[arg(long)]
        display: Option<String>,
    },
    /// Change a setting, then read it back to confirm.
    Set {
        setting: String,
        /// A number, or a value name such as `medium` or `top-right`.
        value: String,
        /// Connector name (e.g. DP-1) or serial number.
        #[arg(long)]
        display: Option<String>,
    },
    /// Save, list, apply and delete presets: named sets of setting values.
    Preset {
        #[command(subcommand)]
        cmd: preset_cli::PresetCmd,
        /// Connector name (e.g. DP-1) or serial number.
        #[arg(long, global = true)]
        display: Option<String>,
    },
    /// Print a JSON report about the monitor, to attach to a model support request.
    Probe {
        /// Also read AOC's vendor items (experimental: reads only, AOC monitors only).
        #[arg(long)]
        aoc_items: bool,
        /// Connector name (e.g. DP-1) or serial number.
        #[arg(long)]
        display: Option<String>,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(&cli) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            if cli.json {
                println!("{}", json!({ "schema": 1, "error": e }));
            }
            eprintln!("aocctl: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Runs the command. Ok(false) means it ran but something it reports failed.
fn run(cli: &Cli) -> Result<bool, String> {
    let root = Path::new(SYSFS_DRM);
    let monitors = monitor::discover(root);
    match &cli.cmd {
        Cmd::List => {
            list(cli.json, &monitors);
            Ok(true)
        }
        Cmd::Get { settings, display } => get(cli.json, monitor::select(&monitors, display.as_deref())?, settings),
        Cmd::Set {
            setting,
            value,
            display,
        } => set(
            cli.json,
            root,
            monitor::select(&monitors, display.as_deref())?,
            setting,
            value,
        ),
        Cmd::Preset { cmd, display } => {
            preset_cli::run(cli.json, root, monitor::select(&monitors, display.as_deref())?, cmd)
        }
        Cmd::Probe { aoc_items, display } => {
            probe(monitor::select(&monitors, display.as_deref())?, *aoc_items)?;
            Ok(true)
        }
    }
}

fn status(m: &Monitor) -> String {
    match (m.is_aoc(), m.bus, m.profile) {
        (false, _, _) => "not an AOC monitor".into(),
        (true, None, _) => format!("no DDC/CI answer: {}", m.error.as_deref().unwrap_or("no i2c bus found")),
        (true, Some(_), Some(_)) => "supported".into(),
        (true, Some(_), None) => "brightness/contrast only (model not in aocctl yet; see `aocctl probe`)".into(),
    }
}

fn list(as_json: bool, monitors: &[Monitor]) {
    if as_json {
        let rows: Vec<Value> = monitors
            .iter()
            .map(|m| {
                let e = &m.connector.edid;
                json!({
                    "connector": m.connector.name,
                    "connector_id": m.connector.id,
                    "mfg": e.mfg,
                    "model": e.model,
                    "product_code": format!("{:04x}", e.product_code),
                    "serial": e.serial,
                    "aoc": m.is_aoc(),
                    "bus": m.bus,
                    "profile": m.profile.is_some(),
                    "status": status(m),
                })
            })
            .collect();
        println!("{}", json!({ "schema": 1, "displays": rows }));
        return;
    }
    if monitors.is_empty() {
        println!("No connected external monitors found.");
    }
    for m in monitors {
        let e = &m.connector.edid;
        let model = format!("{} {}", e.mfg, e.model.as_deref().unwrap_or("?"));
        let bus = m.bus.map(|b| format!("  (i2c-{b})")).unwrap_or_default();
        println!("{:<10} {:<16} {}{bus}", m.connector.name, model, status(m));
    }
}

fn describe(s: &Setting, value: u16) -> String {
    match (s.label_for(value), s.domain) {
        (Some(l), _) => format!("{value} ({l})"),
        (None, Domain::Range { .. }) => value.to_string(),
        (None, Domain::Enum(_)) => format!("{value} (unknown)"),
    }
}

fn get(as_json: bool, m: &Monitor, wanted: &[String]) -> Result<bool, String> {
    let settings: Vec<&Setting> = if wanted.is_empty() {
        m.settings()
    } else {
        wanted
            .iter()
            .map(|w| m.setting(w).ok_or_else(|| unknown_setting(m, w)))
            .collect::<Result<_, _>>()?
    };
    let mut ddc = m.open().map_err(|e| e.to_string())?;
    let mut ok = true;
    let mut rows = Vec::new();
    for s in settings {
        let r = monitor::read(&mut ddc, s);
        ok &= r.is_ok();
        if as_json {
            rows.push(match r {
                Ok(r) => json!({
                    "id": s.id, "value": r.value, "max": r.max,
                    "label": s.label_for(r.value), "writable": s.verified_write,
                }),
                Err(e) => json!({ "id": s.id, "error": e.to_string() }),
            });
        } else {
            let lock = if s.verified_write { "" } else { "  (read-only)" };
            match r {
                Ok(r) => println!("{:<16} {}{lock}", s.id, describe(s, r.value)),
                Err(e) => println!("{:<16} error: {e}", s.id),
            }
        }
    }
    if as_json {
        println!(
            "{}",
            json!({ "schema": 1, "display": m.connector.name, "settings": rows })
        );
    }
    Ok(ok)
}

fn set(as_json: bool, root: &Path, m: &Monitor, id: &str, input: &str) -> Result<bool, String> {
    let s = m.setting(id).ok_or_else(|| unknown_setting(m, id))?;
    let value = s.parse_value(input)?;
    let mut ddc = m.open().map_err(|e| e.to_string())?;
    monitor::recheck(root, m)?;
    let status = monitor::write(&mut ddc, s, value).map_err(|e| e.to_string())?;
    let (word, read_back, detail) = match &status {
        WriteStatus::Verified => ("verified", Some(value), None),
        WriteStatus::Mismatch(v) => ("mismatch", Some(*v), None),
        WriteStatus::Unknown(e) => ("unknown", None, Some(e.clone())),
    };
    if as_json {
        println!(
            "{}",
            json!({
                "schema": 1, "display": m.connector.name, "id": s.id, "value": value,
                "label": s.label_for(value), "status": word, "read_back": read_back, "error": detail,
            })
        );
    } else {
        match status {
            WriteStatus::Verified => println!("{} = {}", s.id, describe(s, value)),
            WriteStatus::Mismatch(v) => eprintln!("aocctl: sent {value} but {} reads back {}", s.id, describe(s, v)),
            WriteStatus::Unknown(e) => eprintln!("aocctl: sent {value} but couldn't read {} back: {e}", s.id),
        }
    }
    Ok(word == "verified")
}

fn unknown_setting(m: &Monitor, id: &str) -> String {
    let ids: Vec<&str> = m.settings().iter().map(|s| s.id).collect();
    format!("unknown setting '{id}' for this monitor; available: {}", ids.join(", "))
}

/// Read-only report. Leaves out the serial number, since reports are posted publicly.
fn probe(m: &Monitor, aoc_items: bool) -> Result<(), String> {
    let e = &m.connector.edid;
    let mut ddc = m.open().map_err(|e| e.to_string())?;
    let caps = ddc.capabilities();
    let mut report = json!({
        "schema": 1,
        "aocctl": env!("CARGO_PKG_VERSION"),
        "connector": m.connector.name,
        "mfg": e.mfg,
        "model": e.model,
        "product_code": format!("{:04x}", e.product_code),
        "known_model": m.profile.is_some(),
        "capabilities": caps.as_ref().ok(),
        "capabilities_error": caps.as_ref().err().map(|e| e.to_string()),
        "brightness": reading(ddc.get_vcp(0x10)),
        "contrast": reading(ddc.get_vcp(0x12)),
    });
    if aoc_items {
        let items: serde_json::Map<String, Value> = models::known_aoc_items()
            .into_iter()
            .map(|i| (format!("{i:02x}"), reading(ddc.get_aoc(i))))
            .collect();
        report["aoc_items"] = Value::Object(items);
    }
    println!("{}", serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?);
    Ok(())
}

fn reading(r: Result<Reading, DdcError>) -> Value {
    match r {
        Ok(r) => json!({ "value": r.value, "max": r.max }),
        Err(e) => json!({ "error": e.to_string() }),
    }
}
