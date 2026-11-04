//! Known AOC monitors and the settings each one supports.
//!
//! A setting is writable only if `verified_write` is true, which means a
//! write / read-back / restore check passed on that model. Everything else is
//! read-only. Enum names come from AOC G-Menu's tables for that model and
//! include only values up to the maximum the monitor reports.

use crate::edid::Edid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Code {
    /// Standard MCCS VCP feature.
    Vcp(u8),
    /// Item on AOC's extended `F8 E1` page.
    Aoc(u8),
}

#[derive(Debug, Clone, Copy)]
pub enum Domain {
    Range { min: u16, max: u16 },
    Enum(&'static [&'static str]),
}

#[derive(Debug, Clone, Copy)]
pub struct Setting {
    /// Stable machine-facing id, e.g. "shadow-control".
    pub id: &'static str,
    pub label: &'static str,
    pub code: Code,
    pub domain: Domain,
    pub verified_write: bool,
}

impl Setting {
    /// Name for a raw value of an enum setting.
    pub fn label_for(&self, value: u16) -> Option<&'static str> {
        match self.domain {
            Domain::Enum(names) => names.get(value as usize).copied(),
            Domain::Range { .. } => None,
        }
    }

    /// Parses a user value (number or enum name) and checks it against the domain.
    pub fn parse_value(&self, input: &str) -> Result<u16, String> {
        match self.domain {
            Domain::Range { min, max } => {
                let v: u16 = input
                    .parse()
                    .map_err(|_| format!("{} takes a number from {min} to {max}", self.id))?;
                if v < min || v > max {
                    return Err(format!("{} takes a number from {min} to {max}", self.id));
                }
                Ok(v)
            }
            Domain::Enum(names) => {
                let wanted = normalize(input);
                if let Some(i) = names.iter().position(|n| normalize(n) == wanted) {
                    return Ok(i as u16);
                }
                match input.parse::<u16>() {
                    Ok(v) if (v as usize) < names.len() => Ok(v),
                    _ => Err(format!(
                        "{} takes one of: {}",
                        self.id,
                        names.iter().map(|n| normalize(n)).collect::<Vec<_>>().join(", ")
                    )),
                }
            }
        }
    }
}

/// Lowercases and joins words with '-', so "Top right" matches "top-right".
pub fn normalize(s: &str) -> String {
    s.split(|c: char| c.is_whitespace() || c == '-' || c == '_')
        .filter(|w| !w.is_empty())
        .map(|w| w.to_lowercase())
        .collect::<Vec<_>>()
        .join("-")
}

#[derive(Debug)]
pub struct Profile {
    pub mfg: &'static str,
    pub product_code: u16,
    /// EDID model name guard; a product code with a different name is not matched.
    pub model: &'static str,
    pub settings: &'static [Setting],
}

/// Standard settings offered on every AOC monitor that answers DDC/CI.
pub const COMMON: &[Setting] = &[
    Setting {
        id: "brightness",
        label: "Brightness",
        code: Code::Vcp(0x10),
        domain: Domain::Range { min: 0, max: 100 },
        verified_write: true,
    },
    Setting {
        id: "contrast",
        label: "Contrast",
        code: Code::Vcp(0x12),
        domain: Domain::Range { min: 0, max: 100 },
        verified_write: true,
    },
];

const fn aoc(id: &'static str, label: &'static str, item: u8, domain: Domain, verified_write: bool) -> Setting {
    Setting {
        id,
        label,
        code: Code::Aoc(item),
        domain,
        verified_write,
    }
}

pub const PROFILES: &[Profile] = &[Profile {
    mfg: "AOC",
    product_code: 0xB401,
    model: "24G4",
    settings: &[
        aoc(
            "shadow-control",
            "Shadow control",
            0x58,
            Domain::Range { min: 0, max: 100 },
            true,
        ),
        aoc(
            "game-color",
            "Game color",
            0x57,
            Domain::Range { min: 0, max: 20 },
            true,
        ),
        aoc(
            "overdrive",
            "Overdrive",
            0x51,
            Domain::Enum(&["Off", "Weak", "Medium", "Strong"]),
            true,
        ),
        aoc(
            "low-blue",
            "Low Blue mode",
            0x59,
            Domain::Enum(&["Off", "Multimedia", "Internet", "Office", "Reading"]),
            true,
        ),
        aoc(
            "eco",
            "Eco mode",
            0x11,
            Domain::Enum(&["Standard", "Text", "Internet", "Game", "Movie", "Sports", "Reading"]),
            true,
        ),
        aoc(
            "hdr-mode",
            "HDR mode",
            0x52,
            Domain::Enum(&["Off", "HDR Picture", "HDR Movie", "HDR Game"]),
            true,
        ),
        aoc("dcr", "DCR", 0x13, Domain::Enum(&["Off", "On"]), true),
        aoc(
            "clear-vision",
            "Clear vision",
            0x14,
            Domain::Enum(&["Off", "Weak", "Medium", "Strong"]),
            true,
        ),
        aoc("dial-point", "Dial point", 0x5A, Domain::Enum(&["Off", "On"]), true),
        aoc(
            "frame-counter",
            "Frame counter",
            0x5B,
            Domain::Enum(&["Off", "Top right", "Bottom right", "Bottom left", "Top left"]),
            true,
        ),
    ],
}];

pub fn is_aoc(edid: &Edid) -> bool {
    edid.mfg == "AOC"
}

/// Finds the profile for a monitor: manufacturer + product code, guarded by model name.
pub fn profile_for(edid: &Edid) -> Option<&'static Profile> {
    PROFILES
        .iter()
        .find(|p| p.mfg == edid.mfg && p.product_code == edid.product_code && edid.model.as_deref() == Some(p.model))
}

/// Settings available on a monitor: the common ones plus its profile's.
pub fn settings_for(profile: Option<&'static Profile>) -> Vec<&'static Setting> {
    COMMON
        .iter()
        .chain(profile.map(|p| p.settings).unwrap_or(&[]))
        .collect()
}

/// Every AOC extended item any profile knows, for `probe --aoc-items`.
pub fn known_aoc_items() -> Vec<u8> {
    let mut items: Vec<u8> = PROFILES
        .iter()
        .flat_map(|p| p.settings.iter())
        .filter_map(|s| match s.code {
            Code::Aoc(i) => Some(i),
            Code::Vcp(_) => None,
        })
        .collect();
    items.sort_unstable();
    items.dedup();
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edid(mfg: &str, code: u16, model: &str) -> Edid {
        Edid {
            mfg: mfg.into(),
            product_code: code,
            model: Some(model.into()),
            serial: None,
        }
    }

    fn setting(id: &str) -> &'static Setting {
        PROFILES[0].settings.iter().find(|s| s.id == id).unwrap()
    }

    #[test]
    fn matches_on_product_code_and_model_name() {
        assert!(profile_for(&edid("AOC", 0xB401, "24G4")).is_some());
        assert!(profile_for(&edid("AOC", 0xB401, "27G4")).is_none());
        assert!(profile_for(&edid("AOC", 0xB402, "24G4")).is_none());
        assert!(profile_for(&edid("SNY", 0xB401, "24G4")).is_none());
    }

    #[test]
    fn enum_values_parse_by_name_or_number_within_the_list() {
        let fc = setting("frame-counter");
        assert_eq!(fc.parse_value("top-right"), Ok(1));
        assert_eq!(fc.parse_value("Top Right"), Ok(1));
        assert_eq!(fc.parse_value("4"), Ok(4));
        assert!(fc.parse_value("5").is_err());
        assert!(fc.parse_value("middle").is_err());
    }

    #[test]
    fn ranges_reject_out_of_domain_numbers() {
        let gc = setting("game-color");
        assert_eq!(gc.parse_value("20"), Ok(20));
        assert!(gc.parse_value("21").is_err());
        assert!(gc.parse_value("-1").is_err());
        assert!(gc.parse_value("high").is_err());
    }

    #[test]
    fn setting_ids_are_unique_and_normalized() {
        for p in PROFILES {
            let all = settings_for(Some(p));
            for (i, s) in all.iter().enumerate() {
                assert_eq!(normalize(s.id), s.id);
                assert!(all[i + 1..].iter().all(|o| o.id != s.id), "duplicate {}", s.id);
            }
        }
    }
}
