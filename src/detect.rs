//! Finds connected monitors through DRM sysfs, and the i2c bus each one's
//! DDC/CI answers on. Only buses tied to a connector are ever opened.

use crate::edid::{self, Edid};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

pub const SYSFS_DRM: &str = "/sys/class/drm";

#[derive(Debug, Clone)]
pub struct Connector {
    /// Unique sysfs name, e.g. "card1-DP-1".
    pub id: String,
    /// Short name shown to users, e.g. "DP-1" (may repeat across GPUs).
    pub name: String,
    pub edid: Edid,
    /// i2c bus numbers to try, best first: a child i2c-N (DP AUX), then the `ddc` link.
    pub buses: Vec<u32>,
}

/// Lists connected connectors with a valid EDID. Buses shared by two connectors are
/// ambiguous and dropped from both.
pub fn connectors(root: &Path) -> Vec<Connector> {
    let mut found = Vec::new();
    let Ok(entries) = fs::read_dir(root) else { return found };
    let mut paths: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    paths.sort();
    for path in paths {
        let Some(dir) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some((card, name)) = dir.split_once('-') else {
            continue;
        };
        if !card.starts_with("card") || is_internal(name) {
            continue;
        }
        let status = fs::read_to_string(path.join("status")).unwrap_or_default();
        if status.trim() != "connected" {
            continue;
        }
        let Some(edid) = fs::read(path.join("edid")).ok().and_then(|raw| edid::parse(&raw)) else {
            continue;
        };
        found.push(Connector {
            id: dir.to_string(),
            name: name.to_string(),
            edid,
            buses: buses(&path),
        });
    }
    let mut uses: HashMap<u32, usize> = HashMap::new();
    for c in &found {
        for b in &c.buses {
            *uses.entry(*b).or_default() += 1;
        }
    }
    for c in &mut found {
        c.buses.retain(|b| uses[b] == 1);
    }
    found
}

/// Built-in laptop/tablet panels; never probed.
fn is_internal(name: &str) -> bool {
    ["eDP", "LVDS", "DSI"].iter().any(|t| name.starts_with(t))
}

fn bus_number(name: &str) -> Option<u32> {
    name.strip_prefix("i2c-")?.parse().ok()
}

fn buses(connector: &Path) -> Vec<u32> {
    let mut children: Vec<u32> = fs::read_dir(connector)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| bus_number(e.file_name().to_str()?))
        .collect();
    children.sort_unstable();
    let ddc = fs::read_link(connector.join("ddc"))
        .ok()
        .and_then(|t| bus_number(t.file_name()?.to_str()?));
    children.extend(ddc.filter(|d| !children.contains(d)));
    children
}

pub fn dev_path(bus: u32) -> PathBuf {
    PathBuf::from(format!("/dev/i2c-{bus}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edid::tests::aoc_24g4;
    use std::os::unix::fs::symlink;

    struct TempDir(PathBuf);
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn temp(name: &str) -> TempDir {
        let p = std::env::temp_dir().join(format!("aocctl-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        TempDir(p)
    }

    fn connector(root: &Path, name: &str, status: &str, edid: &[u8], child: Option<u32>, ddc: Option<u32>) {
        let dir = root.join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("status"), format!("{status}\n")).unwrap();
        fs::write(dir.join("edid"), edid).unwrap();
        if let Some(c) = child {
            fs::create_dir(dir.join(format!("i2c-{c}"))).unwrap();
        }
        if let Some(d) = ddc {
            symlink(format!("../../../i2c-{d}"), dir.join("ddc")).unwrap();
        }
    }

    #[test]
    fn prefers_the_dp_aux_child_bus_over_the_ddc_link() {
        // Layout seen on amdgpu: DP-1 has ddc -> i2c-4 (silent) and child i2c-8 (answers).
        let t = temp("aux");
        connector(&t.0, "card1-DP-1", "connected", &aoc_24g4(), Some(8), Some(4));
        connector(&t.0, "card1-HDMI-A-1", "disconnected", &aoc_24g4(), None, Some(5));
        connector(&t.0, "card1-Writeback-1", "unknown", &[], None, None);
        connector(&t.0, "card1-eDP-1", "connected", &aoc_24g4(), None, Some(9));
        let found = connectors(&t.0);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "DP-1");
        assert_eq!(found[0].edid.model.as_deref(), Some("24G4"));
        assert_eq!(found[0].buses, vec![8, 4]);
    }

    #[test]
    fn skips_invalid_edid_and_drops_shared_buses() {
        let t = temp("shared");
        connector(&t.0, "card0-DP-1", "connected", &aoc_24g4(), None, Some(3));
        connector(&t.0, "card0-DP-2", "connected", &aoc_24g4(), Some(7), Some(3));
        connector(&t.0, "card0-DP-3", "connected", &[0u8; 128], None, Some(9));
        let found = connectors(&t.0);
        assert_eq!(found.len(), 2);
        assert!(found[0].buses.is_empty());
        assert_eq!(found[1].buses, vec![7]);
    }
}
