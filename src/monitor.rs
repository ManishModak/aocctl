//! High-level operations the CLI (and later the GUI) use: discover AOC monitors,
//! pick one, read settings, and write a setting with read-back verification.

use crate::ddc::{Ddc, DdcError, Reading, Timing, Transport};
use crate::detect::{self, Connector};
use crate::i2c::I2cDev;
use crate::models::{self, Code, Profile, Setting};
use std::path::Path;

#[derive(Debug, Clone)]
pub struct Monitor {
    pub connector: Connector,
    pub profile: Option<&'static Profile>,
    /// The bus DDC/CI answered on, if any (AOC monitors only; others are never probed).
    pub bus: Option<u32>,
}

impl Monitor {
    pub fn is_aoc(&self) -> bool {
        models::is_aoc(&self.connector.edid)
    }

    pub fn settings(&self) -> Vec<&'static Setting> {
        models::settings_for(self.profile)
    }

    pub fn setting(&self, id: &str) -> Option<&'static Setting> {
        let id = models::normalize(id);
        self.settings().into_iter().find(|s| s.id == id)
    }

    pub fn open(&self) -> Result<Ddc<I2cDev>, DdcError> {
        let bus = self.bus.ok_or(DdcError::NoReply)?;
        Ok(Ddc::new(I2cDev::open(&detect::dev_path(bus))?, Timing::default()))
    }
}

/// Lists connected monitors. For AOC monitors, finds the bus whose DDC/CI answers.
pub fn discover(root: &Path) -> Vec<Monitor> {
    detect::connectors(root)
        .into_iter()
        .map(|connector| {
            let aoc = models::is_aoc(&connector.edid);
            let bus = if aoc {
                connector.buses.iter().copied().find(|b| answers(*b))
            } else {
                None
            };
            Monitor {
                profile: models::profile_for(&connector.edid),
                connector,
                bus,
            }
        })
        .collect()
}

fn answers(bus: u32) -> bool {
    match I2cDev::open(&detect::dev_path(bus)) {
        Ok(dev) => Ddc::new(dev, Timing::default()).get_vcp(0x10).is_ok(),
        Err(_) => false,
    }
}

/// Picks the monitor named by `wanted` (connector name or serial), or the only
/// AOC monitor that answers when `wanted` is None.
pub fn select<'a>(monitors: &'a [Monitor], wanted: Option<&str>) -> Result<&'a Monitor, String> {
    let usable: Vec<&Monitor> = monitors.iter().filter(|m| m.is_aoc()).collect();
    let Some(wanted) = wanted else {
        // Default only to the sole connected AOC monitor, answering or not, so a busy
        // monitor never silently redirects a command to another one.
        return match usable.as_slice() {
            [one] if one.bus.is_some() => Ok(one),
            [one] => Err(format!("{} does not answer over DDC/CI", one.connector.name)),
            [] => Err("no AOC monitor connected (see `aocctl list`)".into()),
            _ => Err("several AOC monitors connected; pick one with --display <connector|serial>".into()),
        };
    };
    let by_name: Vec<&Monitor> = usable
        .iter()
        .copied()
        .filter(|m| m.connector.name.eq_ignore_ascii_case(wanted))
        .collect();
    let by_serial: Vec<&Monitor> = usable
        .iter()
        .copied()
        .filter(|m| m.connector.edid.serial.as_deref() == Some(wanted))
        .collect();
    let found = match (by_name.as_slice(), by_serial.as_slice()) {
        ([one], _) => *one,
        ([], [one]) => *one,
        ([], []) => return Err(format!("no AOC monitor matches '{wanted}' (see `aocctl list`)")),
        _ => {
            return Err(format!(
                "'{wanted}' matches more than one monitor; use the connector name"
            ));
        }
    };
    if found.bus.is_none() {
        return Err(format!("{} does not answer over DDC/CI", found.connector.name));
    }
    Ok(found)
}

/// Confirms, right before a write, that the monitor is still on the same connector
/// with the same EDID and still owns the same bus. Call it while holding the bus lock.
pub fn recheck(root: &Path, monitor: &Monitor) -> Result<(), String> {
    let now = detect::connectors(root);
    let same = now
        .iter()
        .find(|c| c.name == monitor.connector.name)
        .is_some_and(|c| c.edid == monitor.connector.edid && monitor.bus.is_some_and(|b| c.buses.contains(&b)));
    if same {
        Ok(())
    } else {
        Err(format!(
            "{} changed since it was detected; try again",
            monitor.connector.name
        ))
    }
}

pub fn read<T: Transport>(ddc: &mut Ddc<T>, setting: &Setting) -> Result<Reading, DdcError> {
    match setting.code {
        Code::Vcp(c) => ddc.get_vcp(c),
        Code::Aoc(i) => ddc.get_aoc(i),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteStatus {
    /// Read back the value we wrote.
    Verified,
    /// Read back a different value.
    Mismatch(u16),
    /// The write was sent but reading it back failed.
    Unknown(String),
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum WriteError {
    #[error("{0} is read-only on this model (its write hasn't been verified yet)")]
    NotVerified(&'static str),
    #[error("{id} value {value} is above the maximum {max} the monitor reports")]
    AboveMax { id: &'static str, value: u16, max: u16 },
    #[error(transparent)]
    Ddc(#[from] DdcError),
}

/// Writes a setting the model has verified, after checking the monitor's reported
/// maximum, then reads it back. The write is never resent.
pub fn write<T: Transport>(ddc: &mut Ddc<T>, setting: &Setting, value: u16) -> Result<WriteStatus, WriteError> {
    if !setting.verified_write {
        return Err(WriteError::NotVerified(setting.id));
    }
    let before = read(ddc, setting)?;
    if value > before.max {
        return Err(WriteError::AboveMax {
            id: setting.id,
            value,
            max: before.max,
        });
    }
    match setting.code {
        Code::Vcp(c) => ddc.set_vcp(c, value)?,
        Code::Aoc(i) => ddc.set_aoc(i, value)?,
    }
    Ok(match read(ddc, setting) {
        Ok(r) if r.value == value => WriteStatus::Verified,
        Ok(r) => WriteStatus::Mismatch(r.value),
        Err(e) => WriteStatus::Unknown(e.to_string()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ddc::{encode, fake::Fake};
    use crate::models::{COMMON, Domain};

    const SHADOW: Setting = Setting {
        id: "shadow-control",
        label: "Shadow control",
        code: Code::Aoc(0x58),
        domain: Domain::Range { min: 0, max: 100 },
        verified_write: true,
    };

    fn shadow_reply(f: &mut Fake, value: u8, max: u8) {
        f.reply(&[0x02, 0x00, 0xF8, 0x58, 0x00, max, 0x00, value]);
    }

    #[test]
    fn write_reads_back_and_reports_verified() {
        let mut f = Fake::default();
        shadow_reply(&mut f, 50, 100);
        shadow_reply(&mut f, 51, 100);
        let mut d = Ddc::new(f, Timing::none());
        assert_eq!(write(&mut d, &SHADOW, 51), Ok(WriteStatus::Verified));
    }

    #[test]
    fn write_reports_mismatch_without_resending() {
        let mut f = Fake::default();
        shadow_reply(&mut f, 50, 100);
        shadow_reply(&mut f, 50, 100);
        let mut d = Ddc::new(f, Timing::none());
        assert_eq!(write(&mut d, &SHADOW, 51), Ok(WriteStatus::Mismatch(50)));
    }

    #[test]
    fn write_reports_unknown_when_read_back_fails() {
        let mut f = Fake::default();
        shadow_reply(&mut f, 50, 100);
        let mut d = Ddc::new(f, Timing::none());
        assert!(matches!(write(&mut d, &SHADOW, 51), Ok(WriteStatus::Unknown(_))));
    }

    #[test]
    fn write_refuses_values_above_reported_max_before_sending() {
        let mut f = Fake::default();
        shadow_reply(&mut f, 50, 60);
        let mut d = Ddc::new(f, Timing::none());
        assert_eq!(
            write(&mut d, &SHADOW, 61),
            Err(WriteError::AboveMax {
                id: "shadow-control",
                value: 61,
                max: 60
            })
        );
    }

    #[test]
    fn write_refuses_unverified_settings_without_touching_the_bus() {
        let unverified = Setting {
            verified_write: false,
            ..SHADOW
        };
        let mut d = Ddc::new(Fake::default(), Timing::none());
        assert_eq!(
            write(&mut d, &unverified, 1),
            Err(WriteError::NotVerified("shadow-control"))
        );
        let mut fresh = Fake::default();
        fresh.reply(&[0x02, 0x00, 0x10, 0x00, 0x00, 100, 0x00, 20]);
        fresh.reply(&[0x02, 0x00, 0x10, 0x00, 0x00, 100, 0x00, 30]);
        let mut d = Ddc::new(fresh, Timing::none());
        assert_eq!(write(&mut d, &COMMON[0], 30), Ok(WriteStatus::Verified));
        assert_eq!(encode(&[0x03, 0x10, 0x00, 30]), d_writes(&d)[1]);
    }

    fn d_writes(d: &Ddc<Fake>) -> Vec<Vec<u8>> {
        d.transport().writes.clone()
    }

    fn monitor(name: &str, mfg: &str, serial: &str, bus: Option<u32>) -> Monitor {
        let edid = crate::edid::Edid {
            mfg: mfg.into(),
            product_code: 0xB401,
            model: Some("24G4".into()),
            serial: Some(serial.into()),
        };
        Monitor {
            connector: Connector {
                name: name.into(),
                edid,
                buses: vec![],
            },
            profile: None,
            bus,
        }
    }

    #[test]
    fn default_selection_needs_exactly_one_connected_aoc_monitor() {
        let one = [
            monitor("DP-1", "AOC", "A", Some(8)),
            monitor("HDMI-A-1", "SNY", "B", None),
        ];
        assert_eq!(select(&one, None).unwrap().connector.name, "DP-1");
        // The second AOC monitor is silent, but it still blocks the default.
        let two = [monitor("DP-1", "AOC", "A", Some(8)), monitor("DP-2", "AOC", "B", None)];
        assert!(select(&two, None).is_err());
        assert_eq!(select(&two, Some("dp-1")).unwrap().connector.name, "DP-1");
        assert!(select(&two, Some("B")).is_err()); // found by serial, but doesn't answer
        let silent = [monitor("DP-1", "AOC", "A", None)];
        assert!(select(&silent, None).is_err());
    }
}
