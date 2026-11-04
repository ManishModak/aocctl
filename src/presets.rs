//! User presets: named sets of setting values for one monitor model ("Gaming", "Night"),
//! kept in `$XDG_CONFIG_HOME/aocctl/presets.toml`. The CLI (and later the GUI) reads
//! them with [`Store::load`], changes them through [`Store::edit`], and writes them to
//! the monitor with [`apply`].
//!
//! The file is meant to be edited by hand too, and keeps its comments when aocctl
//! changes it:
//!
//! ```toml
//! [[preset]]
//! name = "Night"
//! mfg = "AOC"
//! model = "24G4"
//! product-code = "b401"
//!
//! [preset.settings]
//! brightness = 10
//! low-blue = "office"
//! ```
//!
//! A preset belongs to one model (EDID manufacturer, product code and model name) and
//! is never listed for or applied to another. Every stored value is checked against
//! that model's settings with the same rules as `aocctl set`, so a hand-edited file
//! can't get around them.

use crate::ddc::{Ddc, Transport};
use crate::edid::Edid;
use crate::models::{self, Domain, Setting};
use crate::monitor::{self, WriteStatus};
use std::fs::{self, File};
use std::io::Write;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use toml_edit::{ArrayOfTables, DocumentMut, Item, Table, TableLike, Value};

const HEADER: &str = "\
# aocctl presets. Change them with `aocctl preset save` or by hand: each [[preset]]
# belongs to one monitor model, and its settings use the ids and values `aocctl get` shows.
";
const KEYS: [&str; 5] = ["name", "mfg", "model", "product-code", "settings"];

/// The monitor model a preset belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelKey {
    pub mfg: String,
    pub product_code: u16,
    pub model: String,
}

impl ModelKey {
    pub fn of(edid: &Edid) -> Result<Self, String> {
        let model = edid
            .model
            .clone()
            .ok_or("this monitor's EDID has no model name, so presets can't be tied to its model")?;
        Ok(ModelKey {
            mfg: edid.mfg.clone(),
            product_code: edid.product_code,
            model,
        })
    }

    /// Every setting aocctl knows for this model.
    pub fn settings(&self) -> Vec<&'static Setting> {
        let edid = Edid {
            mfg: self.mfg.clone(),
            product_code: self.product_code,
            model: Some(self.model.clone()),
            serial: None,
        };
        models::settings_for(models::profile_for(&edid))
    }

    /// The setting with this id, if the model has it and its writes are verified.
    pub fn writable(&self, id: &str) -> Result<&'static Setting, String> {
        let all = self.settings();
        match all.iter().find(|s| s.id == models::normalize(id)) {
            Some(s) if s.verified_write => Ok(s),
            Some(s) => Err(format!("{} is read-only on this model", s.id)),
            None => {
                let ids: Vec<&str> = all.iter().filter(|s| s.verified_write).map(|s| s.id).collect();
                Err(format!(
                    "unknown setting '{id}' for this model; available: {}",
                    ids.join(", ")
                ))
            }
        }
    }
}

impl std::fmt::Display for ModelKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}", self.mfg, self.model)
    }
}

/// A preset of one model, as stored. Values are in the model's setting order.
#[derive(Debug, Clone)]
pub struct Preset {
    pub name: String,
    pub values: Vec<(&'static Setting, u16)>,
    /// What's wrong with the stored values. A preset with problems is never applied.
    pub problems: Vec<String>,
}

impl Preset {
    /// The values to write, or why the preset can't be applied.
    pub fn applicable(&self) -> Result<&[(&'static Setting, u16)], String> {
        match self.problems.as_slice() {
            [] => Ok(&self.values),
            p => Err(format!("preset '{}' can't be applied: {}", self.name, p.join("; "))),
        }
    }
}

/// Where presets live: `$XDG_CONFIG_HOME/aocctl/presets.toml`, or `~/.config/...`.
pub fn default_path() -> Result<PathBuf, String> {
    let base = match std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from) {
        Some(p) if p.is_absolute() => p,
        _ => PathBuf::from(std::env::var_os("HOME").ok_or("neither XDG_CONFIG_HOME nor HOME is set")?).join(".config"),
    };
    Ok(base.join("aocctl").join("presets.toml"))
}

/// The presets file, parsed. Load it with [`Store::load`] to read, or [`Store::edit`]
/// to change it and then [`Store::write`] it.
pub struct Store {
    path: PathBuf,
    doc: DocumentMut,
    /// Exclusive lock on the presets directory, held by an editing store until it's dropped.
    _lock: Option<File>,
}

/// One [[preset]] entry's identity, after the file's structure was checked.
struct Entry {
    name: String,
    key: ModelKey,
}

impl Store {
    /// Reads the file (a missing file has no presets) and checks its structure.
    pub fn load(path: &Path) -> Result<Store, String> {
        let text = match fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(format!("{}: {e}", path.display())),
        };
        Store::parse(path, &text).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Parses the file's text and checks its structure.
    fn parse(path: &Path, text: &str) -> Result<Store, String> {
        let store = Store {
            path: path.to_path_buf(),
            doc: text.parse().map_err(|e: toml_edit::TomlError| e.to_string())?,
            _lock: None,
        };
        store.entries()?;
        Ok(store)
    }

    /// Locks the presets directory against other aocctl edits, then loads the file.
    pub fn edit(path: &Path) -> Result<Store, String> {
        let dir = path.parent().ok_or("presets path has no directory")?;
        fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let lock = lock_dir(dir)?;
        Ok(Store {
            _lock: Some(lock),
            ..Store::load(path)?
        })
    }

    fn array(&self) -> Option<&ArrayOfTables> {
        self.doc.get("preset").and_then(Item::as_array_of_tables)
    }

    /// Checks that every entry names its preset and model; values are checked per preset.
    fn entries(&self) -> Result<Vec<Entry>, String> {
        if let Some((k, _)) = self.doc.iter().find(|(k, _)| *k != "preset") {
            return Err(format!("unknown key '{k}' (presets go in [[preset]] tables)"));
        }
        let Some(item) = self.doc.get("preset") else {
            return Ok(Vec::new());
        };
        let arr = item
            .as_array_of_tables()
            .ok_or("'preset' must be a list of [[preset]] tables")?;
        arr.iter()
            .enumerate()
            .map(|(i, t)| entry(t).map_err(|e| format!("preset #{}: {e}", i + 1)))
            .collect()
    }

    /// Positions of the entries of `key` named `name` (names match ignoring case).
    fn matches(&self, key: &ModelKey, name: &str) -> Vec<usize> {
        let entries = self.entries().unwrap_or_default();
        (0..entries.len())
            .filter(|&i| entries[i].key == *key && same_name(&entries[i].name, name))
            .collect()
    }

    /// The presets of one model, in file order, each with its values checked.
    pub fn presets(&self, key: &ModelKey) -> Vec<Preset> {
        let entries = self.entries().unwrap_or_default();
        let Some(arr) = self.array() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for (e, t) in entries.iter().zip(arr.iter()) {
            if e.key != *key {
                continue;
            }
            let mut p = read_values(key, &e.name, t);
            if entries
                .iter()
                .filter(|o| o.key == *key && same_name(&o.name, &e.name))
                .count()
                > 1
            {
                p.problems.push(format!("'{}' is defined more than once", e.name));
            }
            out.push(p);
        }
        out
    }

    /// The preset of `key` named `name`.
    pub fn find(&self, key: &ModelKey, name: &str) -> Result<Preset, String> {
        let mut found: Vec<Preset> = self
            .presets(key)
            .into_iter()
            .filter(|p| same_name(&p.name, name))
            .collect();
        match found.len() {
            1 => Ok(found.remove(0)),
            0 => {
                let names: Vec<String> = self.presets(key).into_iter().map(|p| p.name).collect();
                Err(if names.is_empty() {
                    format!("no preset '{name}' for {key}; save one with `aocctl preset save {name}`")
                } else {
                    format!("no preset '{name}' for {key}; have: {}", names.join(", "))
                })
            }
            n => Err(format!(
                "preset '{name}' is defined {n} times for {key} in {}; remove the extras",
                self.path.display()
            )),
        }
    }

    /// Stores `values` under `name`. With `merge`, adds them to an existing preset's
    /// values; otherwise they replace them. Returns the stored preset and whether it's
    /// new. Refuses (leaving the file unwritten) if the result wouldn't be valid.
    pub fn put(
        &mut self,
        key: &ModelKey,
        name: &str,
        values: &[(&'static Setting, u16)],
        merge: bool,
    ) -> Result<(Preset, bool), String> {
        check_name(name)?;
        let found = self.matches(key, name);
        if found.len() > 1 {
            return Err(self.find(key, name).expect_err("defined more than once"));
        }
        let new_file = self.doc.to_string().trim().is_empty();
        let arr = self
            .doc
            .entry("preset")
            .or_insert(Item::ArrayOfTables(ArrayOfTables::new()))
            .as_array_of_tables_mut()
            .ok_or("'preset' must be a list of [[preset]] tables")?;
        let stored_name = match found.first() {
            Some(&i) => {
                let t = arr.get_mut(i).expect("index from matches");
                match t.get_mut("settings").and_then(Item::as_table_like_mut) {
                    Some(existing) if merge => {
                        for (s, v) in values {
                            existing.insert(s.id, Item::Value(stored(s, *v)));
                        }
                    }
                    _ => {
                        t.insert("settings", settings_table(values));
                    }
                }
                t["name"].as_str().unwrap_or(name).to_string()
            }
            None => {
                let mut t = Table::new();
                if new_file {
                    t.decor_mut().set_prefix(format!("{HEADER}\n"));
                }
                t.insert("name", toml_edit::value(name));
                t.insert("mfg", toml_edit::value(key.mfg.as_str()));
                t.insert("model", toml_edit::value(key.model.as_str()));
                t.insert("product-code", toml_edit::value(format!("{:04x}", key.product_code)));
                t.insert("settings", settings_table(values));
                arr.push(t);
                name.to_string()
            }
        };
        let p = self.find(key, &stored_name)?;
        p.applicable()?;
        Ok((p, found.is_empty()))
    }

    /// Deletes the preset of `key` named `name`; returns its stored name.
    pub fn remove(&mut self, key: &ModelKey, name: &str) -> Result<String, String> {
        let p = self.find(key, name)?;
        let i = self.matches(key, name)[0];
        self.doc["preset"]
            .as_array_of_tables_mut()
            .expect("find succeeded")
            .remove(i);
        Ok(p.name)
    }

    /// Writes the file atomically (temp file + rename), through a symlink if the
    /// presets file is one, keeping its permissions.
    pub fn write(&self) -> Result<(), String> {
        let target = fs::canonicalize(&self.path).unwrap_or_else(|_| self.path.clone());
        let dir = target.parent().ok_or("presets path has no directory")?;
        let file_name = target.file_name().ok_or("presets path has no file name")?;
        let tmp = dir.join(format!(".{}.tmp-{}", file_name.to_string_lossy(), std::process::id()));
        let err = |e: std::io::Error| format!("{}: {e}", tmp.display());
        let result = (|| {
            let mut f = File::create(&tmp).map_err(err)?;
            f.write_all(self.doc.to_string().as_bytes()).map_err(err)?;
            if let Ok(meta) = fs::metadata(&target) {
                f.set_permissions(meta.permissions()).map_err(err)?;
            }
            f.sync_all().map_err(err)?;
            fs::rename(&tmp, &target).map_err(|e| format!("{}: {e}", target.display()))
        })();
        if result.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        result
    }
}

/// Reads one entry's identity fields; rejects unknown keys so typos don't go unnoticed.
fn entry(t: &Table) -> Result<Entry, String> {
    if let Some((k, _)) = t.iter().find(|(k, _)| !KEYS.contains(k)) {
        return Err(format!("unknown key '{k}' (expected {})", KEYS.join(", ")));
    }
    let text = |k: &str| {
        t.get(k)
            .and_then(Item::as_str)
            .ok_or_else(|| format!("'{k}' must be a string"))
    };
    let name = text("name")?;
    check_name(name)?;
    let code = text("product-code")?;
    let product_code = match u16::from_str_radix(code, 16) {
        Ok(c) if code.len() == 4 && code.chars().all(|c| c.is_ascii_hexdigit()) => c,
        _ => {
            return Err(format!(
                "product-code '{code}' must be 4 hex digits, as `aocctl list --json` shows"
            ));
        }
    };
    if t.get("settings").is_some_and(|s| s.as_table_like().is_none()) {
        return Err(format!("preset '{name}': 'settings' must be a table"));
    }
    Ok(Entry {
        name: name.to_string(),
        key: ModelKey {
            mfg: text("mfg")?.to_string(),
            product_code,
            model: text("model")?.to_string(),
        },
    })
}

/// Checks a stored preset's values against its model, as `aocctl set` would.
fn read_values(key: &ModelKey, name: &str, t: &Table) -> Preset {
    let mut p = Preset {
        name: name.to_string(),
        values: Vec::new(),
        problems: Vec::new(),
    };
    let empty = Table::new();
    let settings: &dyn TableLike = t.get("settings").and_then(Item::as_table_like).unwrap_or(&empty);
    for (id, item) in settings.iter() {
        let s = match key.writable(id) {
            Ok(s) if s.id == id => s,
            Ok(s) => {
                p.problems.push(format!("'{id}' should be written '{}'", s.id));
                continue;
            }
            Err(e) => {
                p.problems.push(e);
                continue;
            }
        };
        let input = match item.as_value() {
            Some(Value::Integer(i)) => i.value().to_string(),
            Some(Value::String(v)) => v.value().clone(),
            _ => {
                p.problems.push(format!("{id} must be a number or a value name"));
                continue;
            }
        };
        match s.parse_value(&input) {
            Ok(v) => p.values.push((s, v)),
            Err(e) => p.problems.push(e),
        }
    }
    if p.values.is_empty() && p.problems.is_empty() {
        p.problems.push("it has no settings".into());
    }
    let order = key.settings();
    p.values.sort_by_key(|(s, _)| order.iter().position(|o| o.id == s.id));
    p
}

fn settings_table(values: &[(&'static Setting, u16)]) -> Item {
    let mut t = Table::new();
    for (s, v) in values {
        t.insert(s.id, Item::Value(stored(s, *v)));
    }
    Item::Table(t)
}

/// How a value is stored: numbers for ranges, the value's name for lists.
fn stored(s: &Setting, v: u16) -> Value {
    match (s.domain, s.label_for(v)) {
        (Domain::Enum(_), Some(label)) => Value::from(models::normalize(label)),
        _ => Value::from(i64::from(v)),
    }
}

fn same_name(a: &str, b: &str) -> bool {
    a.to_lowercase() == b.to_lowercase()
}

/// Names are what people type and widgets show: short, printable, no stray spaces.
pub fn check_name(name: &str) -> Result<(), String> {
    let ok =
        !name.is_empty() && name.chars().count() <= 64 && name.trim() == name && !name.chars().any(char::is_control);
    if ok {
        Ok(())
    } else {
        Err(format!(
            "invalid preset name {name:?}: use 1 to 64 characters, without control characters or leading/trailing spaces"
        ))
    }
}

fn lock_dir(dir: &Path) -> Result<File, String> {
    let f = File::open(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let start = Instant::now();
    // SAFETY: the fd is valid and owned by `f`.
    while unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        if start.elapsed() > Duration::from_secs(5) {
            return Err(format!("{} is locked by another aocctl", dir.display()));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Ok(f)
}

/// Reads the current values of `settings` for `preset save`. A value outside the
/// setting's known values can't be written back (the 24G4's Eco reads 7 while DCR is
/// on), so it's left out and returned second. Fails if any setting can't be read.
#[allow(clippy::type_complexity)]
pub fn capture<T: Transport>(
    ddc: &mut Ddc<T>,
    settings: &[&'static Setting],
) -> Result<(Vec<(&'static Setting, u16)>, Vec<(&'static Setting, u16)>), String> {
    let (mut kept, mut skipped) = (Vec::new(), Vec::new());
    for &s in settings {
        let r = monitor::read(ddc, s).map_err(|e| format!("couldn't read {}: {e}", s.id))?;
        if s.parse_value(&r.value.to_string()).is_ok() {
            kept.push((s, r.value));
        } else {
            skipped.push((s, r.value));
        }
    }
    Ok((kept, skipped))
}

/// What applying one preset value did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Already had that value; nothing was sent.
    Unchanged,
    /// Written, and it still reads back after the whole preset was applied.
    Verified,
    /// Reads back a different value (the write didn't take, or a later setting changed it).
    Mismatch(u16),
    /// Couldn't read the value back after writing it.
    Unknown(String),
    /// The value couldn't be read first, or the write was refused or failed to send.
    Failed(String),
}

impl Outcome {
    pub fn ok(&self) -> bool {
        matches!(self, Outcome::Unchanged | Outcome::Verified)
    }
}

/// Order presets write in: mode settings first (see [`models::MODE_SETTINGS`]), then the rest.
pub fn apply_order(values: &[(&'static Setting, u16)]) -> Vec<(&'static Setting, u16)> {
    let mut out = values.to_vec();
    out.sort_by_key(|(s, _)| {
        models::MODE_SETTINGS
            .iter()
            .position(|m| *m == s.id)
            .unwrap_or(models::MODE_SETTINGS.len())
    });
    out
}

/// Writes a preset's values over an open (locked) connection: each value that differs
/// is written once and read back, in [`apply_order`]. If anything was written, every
/// value is read again at the end, since a mode switch can change values set before it.
pub fn apply<T: Transport>(
    ddc: &mut Ddc<T>,
    values: &[(&'static Setting, u16)],
) -> Vec<(&'static Setting, u16, Outcome)> {
    let mut out = Vec::new();
    let mut wrote = false;
    for (s, v) in apply_order(values) {
        let outcome = match monitor::read(ddc, s) {
            Err(e) => Outcome::Failed(format!("couldn't read it first: {e}")),
            Ok(r) if r.value == v => Outcome::Unchanged,
            Ok(_) => {
                wrote = true;
                match monitor::write(ddc, s, v) {
                    Ok(WriteStatus::Verified) => Outcome::Verified,
                    Ok(WriteStatus::Mismatch(got)) => Outcome::Mismatch(got),
                    Ok(WriteStatus::Unknown(e)) => Outcome::Unknown(e),
                    Err(e) => Outcome::Failed(e.to_string()),
                }
            }
        };
        out.push((s, v, outcome));
    }
    if wrote {
        for (s, v, outcome) in out.iter_mut().filter(|(_, _, o)| o.ok()) {
            match monitor::read(ddc, s) {
                Ok(r) if r.value == *v => {}
                Ok(r) => *outcome = Outcome::Mismatch(r.value),
                Err(e) => *outcome = Outcome::Unknown(e.to_string()),
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ddc::{Timing, fake::Fake};
    use crate::models::Code;

    fn key() -> ModelKey {
        ModelKey {
            mfg: "AOC".into(),
            product_code: 0xB401,
            model: "24G4".into(),
        }
    }

    fn set(id: &str) -> &'static Setting {
        key().writable(id).unwrap()
    }

    fn ids(values: &[(&'static Setting, u16)]) -> Vec<(&'static str, u16)> {
        values.iter().map(|(s, v)| (s.id, *v)).collect()
    }

    fn store(text: &str) -> Store {
        Store::parse(Path::new("presets.toml"), text).unwrap()
    }

    const NIGHT: &str = r#"
# keep me
[[preset]]
name = "Night"
mfg = "AOC"
model = "24G4"
product-code = "B401"
settings = { brightness = 10, low-blue = "Office" } # and me

[[preset]]
name = "Night"
mfg = "AOC"
model = "27G4"
product-code = "b402"
settings = { brightness = 5 }
"#;

    #[test]
    fn presets_belong_to_one_model_and_values_parse_like_set() {
        let s = store(NIGHT);
        let night = s.find(&key(), "night").unwrap();
        assert_eq!(ids(&night.values), [("brightness", 10), ("low-blue", 3)]);
        assert!(night.problems.is_empty());
        assert_eq!(s.presets(&key()).len(), 1, "the 27G4 preset isn't listed for a 24G4");
        let other = ModelKey {
            product_code: 0xB402,
            ..key()
        };
        assert!(s.presets(&other).is_empty(), "same name, other product code");
    }

    #[test]
    fn hand_edited_values_outside_the_domain_block_the_preset() {
        let s = store(
            r#"
[[preset]]
name = "Bad"
mfg = "AOC"
model = "24G4"
product-code = "b401"
settings = { brightness = 300, overdrive = "turbo", gamma = 2, eco = true, contrast = 40 }
"#,
        );
        let p = s.find(&key(), "Bad").unwrap();
        assert_eq!(ids(&p.values), [("contrast", 40)]);
        assert_eq!(p.problems.len(), 4, "{:?}", p.problems);
        assert!(p.applicable().is_err());
    }

    #[test]
    fn structural_mistakes_reject_the_whole_file() {
        let bad = [
            "name = 'x'",
            "[[preset]]\nname = 'x'\nmfg = 'AOC'\nmodel = '24G4'\nproduct-code = 'b401'\nsetings = {}",
            "[[preset]]\nname = 'x'\nmfg = 'AOC'\nmodel = '24G4'\nproduct-code = 'b4011'",
            "[[preset]]\nname = 'x'\nmfg = 'AOC'\nmodel = '24G4'\nproduct-code = '+b40'",
            "[[preset]]\nname = 'x'\nmfg = 'AOC'\nproduct-code = 'b401'",
            "[[preset]]\nname = ' x'\nmfg = 'AOC'\nmodel = '24G4'\nproduct-code = 'b401'",
            "[preset]\nname = 'x'",
            "not toml",
        ];
        for text in bad {
            assert!(Store::parse(Path::new("p"), text).is_err(), "{text}");
        }
    }

    #[test]
    fn duplicate_names_are_never_applied() {
        let s = store(&format!(
            "{NIGHT}{}",
            NIGHT.replace("# keep me", "").replace("Night", "NIGHT")
        ));
        assert!(s.find(&key(), "Night").is_err());
        assert!(s.presets(&key()).iter().all(|p| p.applicable().is_err()));
    }

    #[test]
    fn put_creates_merges_replaces_and_keeps_comments() {
        let mut s = store(NIGHT);
        let (p, created) = s.put(&key(), "NIGHT", &[(set("contrast"), 40)], true).unwrap();
        assert!(!created);
        assert_eq!(p.name, "Night");
        assert_eq!(p.values.len(), 3);
        let text = s.doc.to_string();
        assert!(text.contains("# keep me") && text.contains("# and me"), "{text}");

        let (p, _) = s.put(&key(), "Night", &[(set("overdrive"), 3)], false).unwrap();
        assert_eq!(ids(&p.values), [("overdrive", 3)]);
        assert!(s.doc.to_string().contains(r#"overdrive = "strong""#));

        let (_, created) = s.put(&key(), "Gaming", &[(set("frame-counter"), 1)], false).unwrap();
        assert!(created);
        assert!(s.doc.to_string().contains(r#"frame-counter = "top-right""#));
        assert_eq!(s.remove(&key(), "gaming").unwrap(), "Gaming");
        assert_eq!(s.presets(&key()).len(), 1);
        assert!(s.remove(&key(), "gaming").is_err());
    }

    #[test]
    fn a_new_file_gets_a_header_and_reloads() {
        let mut s = store("");
        s.put(&key(), "Day", &[(set("brightness"), 40)], false).unwrap();
        let text = s.doc.to_string();
        assert!(text.starts_with("# aocctl presets."), "{text}");
        let again = store(&text).find(&key(), "Day").unwrap();
        assert_eq!(ids(&again.values), [("brightness", 40)]);
    }

    #[test]
    fn put_refuses_a_result_that_is_still_invalid() {
        let mut s = store(&NIGHT.replace("brightness = 10", "brightness = 300"));
        assert!(s.put(&key(), "Night", &[(set("contrast"), 40)], true).is_err());
        assert!(s.put(&key(), "Night", &[(set("brightness"), 40)], true).is_ok());
        assert!(s.put(&key(), " bad", &[(set("brightness"), 40)], true).is_err());
    }

    #[test]
    fn write_replaces_the_file_through_a_symlink() {
        let dir = std::env::temp_dir().join(format!("aocctl-presets-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("dots")).unwrap();
        fs::write(dir.join("dots/presets.toml"), NIGHT).unwrap();
        std::os::unix::fs::symlink("dots/presets.toml", dir.join("presets.toml")).unwrap();
        let path = dir.join("presets.toml");
        let mut s = Store::edit(&path).unwrap();
        s.put(&key(), "Day", &[(set("brightness"), 40)], false).unwrap();
        s.write().unwrap();
        drop(s);
        assert!(fs::symlink_metadata(&path).unwrap().file_type().is_symlink());
        assert!(Store::load(&path).unwrap().find(&key(), "Day").is_ok());
        assert!(
            Store::load(&dir.join("missing.toml"))
                .unwrap()
                .presets(&key())
                .is_empty()
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    /// A pretend 24G4 that answers reads from its state and applies writes, with optional
    /// side effects (writing `when` also sets `then` to a value) and settings that ignore writes.
    #[derive(Default)]
    struct Sim {
        state: Vec<(Code, u16, u16)>,
        effects: Vec<(Code, Code, u16)>,
        stuck: Vec<Code>,
        sets: Vec<(Code, u16)>,
        replies: Fake,
    }

    impl Sim {
        fn with(values: &[(&str, u16)]) -> Sim {
            let mut sim = Sim::default();
            for s in key().settings() {
                let max = match s.domain {
                    Domain::Range { max, .. } => max,
                    Domain::Enum(n) => n.len() as u16 - 1,
                };
                let v = values.iter().find(|(id, _)| *id == s.id).map_or(0, |(_, v)| *v);
                sim.state.push((s.code, v, max));
            }
            sim
        }

        fn value(&self, id: &str) -> u16 {
            self.state.iter().find(|(c, ..)| *c == set(id).code).unwrap().1
        }

        fn store(&mut self, code: Code, v: u16) {
            self.state.iter_mut().find(|(c, ..)| *c == code).unwrap().1 = v;
        }
    }

    impl Transport for Sim {
        fn write(&mut self, bytes: &[u8]) -> Result<(), crate::ddc::DdcError> {
            let p = &bytes[2..bytes.len() - 1];
            let (code, rest) = match p {
                [op, 0xF8, 0xE1, item, rest @ ..] => (Code::Aoc(*item), (*op, rest)),
                [op, c, rest @ ..] => (Code::Vcp(*c), (*op, rest)),
                _ => panic!("unexpected packet {p:02x?}"),
            };
            match rest {
                (0x01, []) => {
                    let &(_, v, max) = self.state.iter().find(|(c, ..)| *c == code).unwrap();
                    let [mh, ml] = max.to_be_bytes();
                    let [vh, vl] = v.to_be_bytes();
                    match code {
                        Code::Aoc(i) => self.replies.reply(&[0x02, 0x00, 0xF8, i, mh, ml, vh, vl]),
                        Code::Vcp(c) => self.replies.reply(&[0x02, 0x00, c, 0x00, mh, ml, vh, vl]),
                    };
                }
                (0x03, [hi, lo]) => {
                    let v = u16::from_be_bytes([*hi, *lo]);
                    self.sets.push((code, v));
                    if !self.stuck.contains(&code) {
                        self.store(code, v);
                    }
                    for (when, then, to) in self.effects.clone() {
                        if when == code {
                            self.store(then, to);
                        }
                    }
                }
                _ => panic!("unexpected packet {p:02x?}"),
            }
            Ok(())
        }

        fn read(&mut self, buf: &mut [u8]) -> Result<(), crate::ddc::DdcError> {
            self.replies.read(buf)
        }
    }

    fn run(sim: Sim, values: &[(&str, u16)]) -> (Vec<(&'static str, Outcome)>, Ddc<Sim>) {
        let values: Vec<_> = values.iter().map(|(id, v)| (set(id), *v)).collect();
        let mut ddc = Ddc::new(sim, Timing::none());
        let out = apply(&mut ddc, &values)
            .into_iter()
            .map(|(s, _, o)| (s.id, o))
            .collect();
        (out, ddc)
    }

    #[test]
    fn apply_writes_modes_first_and_skips_values_already_set() {
        // HDR mode swaps in its own brightness, so it must go before brightness.
        let mut sim = Sim::with(&[("brightness", 20), ("overdrive", 2)]);
        sim.effects.push((set("hdr-mode").code, set("brightness").code, 25));
        let (out, ddc) = run(sim, &[("brightness", 30), ("overdrive", 2), ("hdr-mode", 1)]);
        let sim = ddc.transport();
        assert_eq!(
            out,
            vec![
                ("hdr-mode", Outcome::Verified),
                ("brightness", Outcome::Verified),
                ("overdrive", Outcome::Unchanged),
            ]
        );
        assert_eq!(sim.sets, vec![(set("hdr-mode").code, 1), (set("brightness").code, 30)]);
        assert_eq!(sim.value("brightness"), 30);
    }

    #[test]
    fn apply_reports_values_a_later_write_changed() {
        let mut sim = Sim::with(&[("brightness", 20)]);
        sim.effects.push((set("overdrive").code, set("brightness").code, 25));
        let (out, _) = run(sim, &[("brightness", 30), ("overdrive", 3)]);
        assert_eq!(
            out,
            vec![("brightness", Outcome::Mismatch(25)), ("overdrive", Outcome::Verified)]
        );
    }

    #[test]
    fn apply_reports_a_write_that_did_not_take_and_sends_it_once() {
        let mut sim = Sim::with(&[]);
        sim.stuck.push(set("shadow-control").code);
        let (out, ddc) = run(sim, &[("shadow-control", 55)]);
        let sim = ddc.transport();
        assert_eq!(out, vec![("shadow-control", Outcome::Mismatch(0))]);
        assert!(!out[0].1.ok());
        assert_eq!(sim.sets.len(), 1);
    }

    #[test]
    fn apply_with_nothing_to_change_sends_nothing() {
        let (out, ddc) = run(Sim::with(&[("brightness", 20)]), &[("brightness", 20)]);
        let sim = ddc.transport();
        assert_eq!(out, vec![("brightness", Outcome::Unchanged)]);
        assert!(sim.sets.is_empty());
    }

    #[test]
    fn capture_leaves_out_values_aocctl_cannot_set() {
        // On the 24G4, DCR on makes Eco read 7, which isn't one of Eco's values.
        let mut ddc = Ddc::new(Sim::with(&[("eco", 7), ("dcr", 1)]), Timing::none());
        let (kept, skipped) = capture(&mut ddc, &[set("dcr"), set("eco")]).unwrap();
        assert_eq!(ids(&kept), [("dcr", 1)]);
        assert_eq!(ids(&skipped), [("eco", 7)]);
    }
}
