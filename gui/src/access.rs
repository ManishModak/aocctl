//! Works out why an AOC monitor that didn't answer can't be reached, so the window can
//! explain the fix (load i2c-dev, or get access to the bus) instead of a raw error.

use std::io::ErrorKind;
use std::path::PathBuf;

#[derive(Debug, PartialEq, Eq)]
pub enum Access {
    /// None of the monitor's /dev/i2c-N devices exist: the i2c-dev module isn't loaded.
    NoDevices,
    /// A device exists but this user can't open it for reading and writing.
    Denied(PathBuf),
    /// The devices open fine (or the monitor has no bus): the problem is elsewhere.
    Granted,
}

/// Opens each device read-write (and closes it again; opening sends nothing to the monitor).
pub fn check(devices: &[PathBuf]) -> Access {
    let mut missing = 0;
    for dev in devices {
        match std::fs::OpenOptions::new().read(true).write(true).open(dev) {
            Err(e) if e.kind() == ErrorKind::PermissionDenied => return Access::Denied(dev.clone()),
            Err(e) if e.kind() == ErrorKind::NotFound => missing += 1,
            _ => {}
        }
    }
    if missing > 0 && missing == devices.len() {
        Access::NoDevices
    } else {
        Access::Granted
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn tells_missing_devices_from_denied_ones() {
        let dir = std::env::temp_dir().join(format!("aocctl-gtk-access-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let (open, locked, gone) = (dir.join("open"), dir.join("locked"), dir.join("gone"));
        fs::write(&open, "").unwrap();
        fs::write(&locked, "").unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        let root = fs::OpenOptions::new().read(true).open(&locked).is_ok(); // root ignores modes
        assert_eq!(check(std::slice::from_ref(&gone)), Access::NoDevices);
        assert_eq!(check(&[gone.clone(), open.clone()]), Access::Granted);
        assert_eq!(check(&[]), Access::Granted);
        if !root {
            assert_eq!(check(&[gone, locked.clone()]), Access::Denied(locked));
        }
        fs::remove_dir_all(&dir).unwrap();
    }
}
