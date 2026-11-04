//! The one thread that talks to monitors and the presets file, so the UI never blocks.
//! The window sends [`Job`]s in order and gets one [`Reply`] per job (a superseded
//! write gets none). Every job opens and locks the monitor's bus itself.

use aocctl::ddc::Reading;
use aocctl::detect::SYSFS_DRM;
use aocctl::models::Setting;
use aocctl::monitor::{self, Monitor, WriteStatus};
use aocctl::presets::{self, ModelKey, Outcome, Preset, Store};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;

/// Each setting of a monitor with its reading, or why it couldn't be read.
pub type Values = Vec<(&'static Setting, Result<Reading, String>)>;

pub enum Job {
    Discover,
    ReadAll(Monitor),
    Write {
        monitor: Monitor,
        setting: &'static Setting,
        value: u16,
        /// Identifies this request in its reply.
        id: u64,
    },
    ListPresets(Monitor),
    SavePreset {
        monitor: Monitor,
        name: String,
    },
    ApplyPreset {
        monitor: Monitor,
        name: String,
    },
    DeletePreset {
        monitor: Monitor,
        name: String,
    },
}

/// Replies name the monitor's connector id, so the window can drop replies about a
/// monitor it no longer shows.
pub enum Reply {
    Monitors(Vec<Monitor>),
    Values {
        connector: String,
        result: Result<Values, String>,
    },
    Written {
        connector: String,
        setting: &'static Setting,
        id: u64,
        result: Result<WriteStatus, String>,
    },
    Presets {
        connector: String,
        result: Result<Vec<Preset>, String>,
    },
    Saved {
        connector: String,
        /// The stored name and the settings left out because their value can't be set back.
        result: Result<(String, Vec<&'static Setting>), String>,
    },
    Applied {
        connector: String,
        name: String,
        result: Result<Vec<(&'static Setting, u16, Outcome)>, String>,
    },
    Deleted {
        connector: String,
        result: Result<String, String>,
    },
}

/// Runs jobs until the window drops its sender.
pub fn run(jobs: Receiver<Job>, replies: async_channel::Sender<Reply>) {
    let root = Path::new(SYSFS_DRM);
    let mut queue = VecDeque::new();
    loop {
        if queue.is_empty() {
            match jobs.recv() {
                Ok(job) => queue.push_back(job),
                Err(_) => return,
            }
        }
        queue.extend(jobs.try_iter());
        let job = queue.pop_front().expect("queue has a job");
        if superseded(&job, &queue) {
            continue;
        }
        if replies.send_blocking(handle(root, job)).is_err() {
            return;
        }
    }
}

/// A write is skipped when a later write of the same setting on the same monitor is queued
/// right behind it, with only other writes in between: only the newest value matters, and each
/// write takes about half a second. Any other job in between (a preset save, say) must see it.
fn superseded(job: &Job, queue: &VecDeque<Job>) -> bool {
    let Job::Write { monitor, setting, .. } = job else {
        return false;
    };
    for later in queue {
        let Job::Write {
            monitor: m, setting: s, ..
        } = later
        else {
            return false;
        };
        if m.connector.id == monitor.connector.id && s.id == setting.id {
            return true;
        }
    }
    false
}

fn handle(root: &Path, job: Job) -> Reply {
    match job {
        Job::Discover => Reply::Monitors(monitor::discover(root)),
        Job::ReadAll(m) => Reply::Values {
            result: read_all(&m),
            connector: m.connector.id,
        },
        Job::Write {
            monitor: m,
            setting,
            value,
            id,
        } => Reply::Written {
            result: write(root, &m, setting, value),
            connector: m.connector.id,
            setting,
            id,
        },
        Job::ListPresets(m) => Reply::Presets {
            result: list_presets(&m),
            connector: m.connector.id,
        },
        Job::SavePreset { monitor: m, name } => Reply::Saved {
            result: save_preset(&m, &name),
            connector: m.connector.id,
        },
        Job::ApplyPreset { monitor: m, name } => Reply::Applied {
            result: apply_preset(root, &m, &name),
            connector: m.connector.id,
            name,
        },
        Job::DeletePreset { monitor: m, name } => Reply::Deleted {
            result: delete_preset(&m, &name),
            connector: m.connector.id,
        },
    }
}

fn read_all(m: &Monitor) -> Result<Values, String> {
    let mut ddc = m.open().map_err(|e| e.to_string())?;
    Ok(m.settings()
        .into_iter()
        .map(|s| (s, monitor::read(&mut ddc, s).map_err(|e| e.to_string())))
        .collect())
}

fn write(root: &Path, m: &Monitor, s: &'static Setting, value: u16) -> Result<WriteStatus, String> {
    let mut ddc = m.open().map_err(|e| e.to_string())?;
    monitor::recheck(root, m)?;
    monitor::write(&mut ddc, s, value).map_err(|e| e.to_string())
}

/// Where presets live. Inside Flatpak, `XDG_CONFIG_HOME` is the app's private folder, so
/// use the host's `~/.config/aocctl` (granted in the manifest) to share presets with the CLI.
pub fn presets_path() -> Result<PathBuf, String> {
    match (std::env::var_os("FLATPAK_ID"), std::env::var_os("HOME")) {
        (Some(_), Some(home)) => Ok(PathBuf::from(home).join(".config/aocctl/presets.toml")),
        _ => presets::default_path(),
    }
}

fn list_presets(m: &Monitor) -> Result<Vec<Preset>, String> {
    let key = ModelKey::of(&m.connector.edid)?;
    Ok(Store::load(&presets_path()?)?.presets(&key))
}

/// Captures every writable setting's current value as preset `name`, replacing its values.
fn save_preset(m: &Monitor, name: &str) -> Result<(String, Vec<&'static Setting>), String> {
    presets::check_name(name)?;
    let key = ModelKey::of(&m.connector.edid)?;
    let settings: Vec<&'static Setting> = key.settings().into_iter().filter(|s| s.verified_write).collect();
    let (kept, skipped) = {
        let mut ddc = m.open().map_err(|e| e.to_string())?;
        presets::capture(&mut ddc, &settings)?
    };
    if kept.is_empty() {
        return Err("none of the settings has a value aocctl can save".into());
    }
    let mut store = Store::edit(&presets_path()?)?;
    let (p, _) = store.put(&key, name, &kept, false)?;
    store.write()?;
    Ok((p.name, skipped.into_iter().map(|(s, _)| s).collect()))
}

#[allow(clippy::type_complexity)]
fn apply_preset(root: &Path, m: &Monitor, name: &str) -> Result<Vec<(&'static Setting, u16, Outcome)>, String> {
    let key = ModelKey::of(&m.connector.edid)?;
    let p = Store::load(&presets_path()?)?.find(&key, name)?;
    let values = p.applicable()?;
    let mut ddc = m.open().map_err(|e| e.to_string())?;
    monitor::recheck(root, m)?;
    Ok(presets::apply(&mut ddc, values))
}

fn delete_preset(m: &Monitor, name: &str) -> Result<String, String> {
    let key = ModelKey::of(&m.connector.edid)?;
    let mut store = Store::edit(&presets_path()?)?;
    let deleted = store.remove(&key, name)?;
    store.write()?;
    Ok(deleted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aocctl::detect::Connector;
    use aocctl::edid::Edid;
    use aocctl::models::COMMON;

    fn monitor(id: &str) -> Monitor {
        Monitor {
            connector: Connector {
                id: id.into(),
                name: id.into(),
                edid: Edid {
                    mfg: "AOC".into(),
                    product_code: 0xB401,
                    model: Some("24G4".into()),
                    serial: None,
                },
                buses: vec![],
            },
            profile: None,
            bus: Some(8),
            error: None,
        }
    }

    fn write(id: &str, setting: usize, value: u16) -> Job {
        Job::Write {
            monitor: monitor(id),
            setting: &COMMON[setting],
            value,
            id: value.into(),
        }
    }

    #[test]
    fn only_the_newest_queued_write_of_a_setting_runs() {
        let queue: VecDeque<Job> = [write("card1-DP-1", 1, 40), write("card1-DP-1", 0, 30)].into();
        assert!(superseded(&write("card1-DP-1", 0, 20), &queue));
        // Another setting, another monitor, or nothing newer queued: it runs.
        assert!(!superseded(&write("card1-DP-1", 0, 30), &VecDeque::new()));
        assert!(!superseded(&write("card1-DP-2", 0, 20), &queue));
        let other: VecDeque<Job> = [write("card1-DP-1", 1, 40), Job::Discover].into();
        assert!(!superseded(&write("card1-DP-1", 0, 20), &other));
        // A job in between (e.g. a preset save) must see the older value written first.
        let between: VecDeque<Job> = [Job::Discover, write("card1-DP-1", 0, 30)].into();
        assert!(!superseded(&write("card1-DP-1", 0, 20), &between));
        assert!(!superseded(&Job::Discover, &queue));
    }
}
