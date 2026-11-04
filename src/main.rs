//! `aocctl` command line. Every command takes `--json` for scripts and
//! desktop-shell widgets; JSON output always carries `"schema": 1`.

use aocctl::ddc::{DdcError, Reading};
use aocctl::detect::SYSFS_DRM;
use aocctl::models::{self, Domain, Setting};
use aocctl::monitor::{self, Monitor, WriteStatus};
use aocctl::verify::{self, Outcome, Verification};
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
    /// Test that a setting's write works on your monitor: write one step away,
    /// read back, restore. Prints a JSON report for a write-verification issue.
    Verify {
        setting: String,
        /// Connector name (e.g. DP-1) or serial number.
        #[arg(long)]
        display: Option<String>,
        /// Required: verify writes to the monitor (and restores the original value).
        #[arg(long)]
        confirm_write: bool,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(&cli) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            // verify's output is always JSON, since it's pasted into an issue.
            if cli.json || matches!(cli.cmd, Cmd::Verify { .. }) {
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
        Cmd::Verify {
            setting,
            display,
            confirm_write,
        } => verify_cmd(
            root,
            monitor::select(&monitors, display.as_deref())?,
            setting,
            *confirm_write,
        ),
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

/// Blocks termination signals while alive, so `verify` can't be stopped between its
/// test write and the restore. A signal that arrived meanwhile is delivered on drop.
struct HoldSignals(libc::sigset_t);

impl HoldSignals {
    fn new() -> Self {
        // SAFETY: plain sigset manipulation on zeroed sets; this program is single-threaded.
        unsafe {
            let mut set: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut set);
            for sig in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP, libc::SIGQUIT] {
                libc::sigaddset(&mut set, sig);
            }
            let mut old: libc::sigset_t = std::mem::zeroed();
            libc::sigprocmask(libc::SIG_BLOCK, &set, &mut old);
            HoldSignals(old)
        }
    }
}

impl Drop for HoldSignals {
    fn drop(&mut self) {
        // SAFETY: restores the mask saved in `new`.
        unsafe {
            libc::sigprocmask(libc::SIG_SETMASK, &self.0, std::ptr::null_mut());
        }
    }
}

/// Runs `verify` on one of the monitor's settings and prints the report (always JSON,
/// since it's meant to be pasted into an issue). Exits 0 only when it passed.
fn verify_cmd(root: &Path, m: &Monitor, id: &str, confirm: bool) -> Result<bool, String> {
    let s = m.setting(id).ok_or_else(|| unknown_setting(m, id))?;
    if !confirm {
        return Err(format!(
            "verify writes to your monitor: it sets {} one step away from its current value, reads it back, \
             then restores the original. Run it again with --confirm-write to go ahead",
            s.id
        ));
    }
    let mut ddc = m.open().map_err(|e| e.to_string())?;
    let name = &m.connector.name;
    // Taken right before the test write and held until the report is out.
    let mut hold = None;
    let v = verify::verify(
        &mut ddc,
        s,
        || monitor::recheck(root, m),
        |before, test| {
            hold = Some(HoldSignals::new());
            eprintln!(
                "aocctl: {name}: {} is {before}; writing {test}, then restoring {before}",
                s.id
            )
        },
    )?;
    let report = verify_report(m, s, &v);
    println!("{}", serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?);
    verify_summary(m, s, &v);
    use std::io::Write;
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
    drop(hold);
    Ok(v.outcome == Outcome::Pass)
}

fn read_json(r: &verify::Read) -> Value {
    match r {
        Ok(r) => json!({ "value": r.value, "max": r.max }),
        Err(e) => json!({ "error": e }),
    }
}

fn step_json(s: &verify::Step) -> Value {
    json!({ "value": s.value, "send_error": s.send_error, "read_back": read_json(&s.read_back) })
}

/// The report a maintainer reads. Leaves out the serial number, since it's posted publicly.
fn verify_report(m: &Monitor, s: &Setting, v: &Verification) -> Value {
    let e = &m.connector.edid;
    let code = match s.code {
        models::Code::Vcp(c) => format!("vcp:{c:02x}"),
        models::Code::Aoc(i) => format!("aoc:{i:02x}"),
    };
    let result = match v.outcome {
        Outcome::Pass => "pass",
        Outcome::Fail => "fail",
        Outcome::RestoreUnconfirmed => "restore_unconfirmed",
        Outcome::RestoreSkipped => "restore_skipped",
    };
    let mut report = json!({
        "schema": 1,
        "kind": "verify",
        "aocctl": env!("CARGO_PKG_VERSION"),
        "display": m.connector.name,
        "mfg": e.mfg,
        "model": e.model,
        "product_code": format!("{:04x}", e.product_code),
        "setting": s.id,
        "code": code,
        "already_verified": s.verified_write,
        "domain": { "min": v.domain.0, "max": v.domain.1 },
        "before": { "value": v.before.value, "max": v.before.max },
        "game_mode_before": v.game_mode_before,
        "test": step_json(&v.test),
        "game_mode_after": v.game_mode_after,
        "result": result,
    });
    match &v.restore {
        Ok(r) => report["restore"] = step_json(r),
        Err(why) => report["restore_skipped"] = json!(why),
    }
    report
}

/// One-line result on stderr, plus how to get the original value back if the restore
/// isn't confirmed.
fn verify_summary(m: &Monitor, s: &Setting, v: &Verification) {
    let before = describe(s, v.before.value);
    match v.outcome {
        Outcome::Pass => eprintln!(
            "aocctl: {} passed: wrote {}, read it back, restored {before}",
            s.id, v.test.value
        ),
        Outcome::Fail => eprintln!(
            "aocctl: {} failed: {} didn't read back; {before} is restored",
            s.id, v.test.value
        ),
        Outcome::RestoreSkipped => eprintln!(
            "aocctl: WARNING: the monitor on {} changed during the test, so {} wasn't restored. \
             On the original monitor, set it back to {before} in its on-screen menu.",
            m.connector.name, s.id
        ),
        Outcome::RestoreUnconfirmed => {
            eprintln!(
                "aocctl: WARNING: couldn't confirm {} is back at {before}. Set it back in the monitor's on-screen menu.",
                s.id
            );
            if s.verified_write {
                eprintln!(
                    "aocctl: or run: aocctl set {} {} --display {}",
                    s.id, v.before.value, m.connector.id
                );
            }
        }
    }
}
