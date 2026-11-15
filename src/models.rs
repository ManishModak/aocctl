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
    Range {
        min: u16,
        max: u16,
    },
    /// Named values that start at 0 and count up.
    Enum(&'static [&'static str]),
    /// Named values at arbitrary numbers, e.g. the game mode VCP.
    Values(&'static [(u16, &'static str)]),
}

#[derive(Debug, Clone, Copy)]
pub struct Setting {
    /// Stable machine-facing id, e.g. "shadow-control".
    pub id: &'static str,
    pub label: &'static str,
    pub code: Code,
    pub domain: Domain,
    pub verified_write: bool,
    /// The monitor ignores or leaves this setting in FPS, RTS and Racing game
    /// modes; writing it from one of those modes is refused.
    pub locked_in_game_presets: bool,
}

impl Setting {
    /// Name for a raw value of an enum setting.
    pub fn label_for(&self, value: u16) -> Option<&'static str> {
        match self.domain {
            Domain::Enum(names) => names.get(value as usize).copied(),
            Domain::Values(values) => values.iter().find(|(v, _)| *v == value).map(|(_, name)| *name),
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
            Domain::Values(values) => {
                let wanted = normalize(input);
                if let Some((value, _)) = values.iter().find(|(_, n)| normalize(n) == wanted) {
                    return Ok(*value);
                }
                match input.parse::<u16>() {
                    Ok(v) if values.iter().any(|(value, _)| *value == v) => Ok(v),
                    _ => Err(format!(
                        "{} takes one of: {}",
                        self.id,
                        values.iter().map(|(_, n)| normalize(n)).collect::<Vec<_>>().join(", ")
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

/// Standard VCP code for the monitor's game mode (MCCS "display application").
pub const GAME_MODE_VCP: u8 = 0xDC;

/// Game modes as the 27G1G4 reports them. FPS, RTS and Racing are fixed presets;
/// Gamer 1-3 are the three user-editable slots.
pub const GAME_MODES: &[(u16, &str)] = &[
    (0x00, "Off"),
    (0x0B, "FPS"),
    (0x0C, "RTS"),
    (0x0D, "Racing"),
    (0x0E, "Gamer 1"),
    (0x0F, "Gamer 2"),
    (0x10, "Gamer 3"),
];

/// The name of a game-mode value, or None if the monitor reported an unknown one.
pub fn game_mode_label(value: u16) -> Option<&'static str> {
    GAME_MODES.iter().find(|(v, _)| *v == value).map(|(_, name)| *name)
}

/// True for FPS, RTS and Racing: the modes whose picture settings are fixed.
pub fn preset_game_mode_label(value: u16) -> Option<&'static str> {
    if matches!(value, 0x0B..=0x0D) {
        game_mode_label(value)
    } else {
        None
    }
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
        locked_in_game_presets: false,
    },
    Setting {
        id: "contrast",
        label: "Contrast",
        code: Code::Vcp(0x12),
        domain: Domain::Range { min: 0, max: 100 },
        verified_write: true,
        locked_in_game_presets: false,
    },
];

const fn setting(
    id: &'static str,
    label: &'static str,
    code: Code,
    domain: Domain,
    verified_write: bool,
    locked_in_game_presets: bool,
) -> Setting {
    Setting {
        id,
        label,
        code,
        domain,
        verified_write,
        locked_in_game_presets,
    }
}

const fn vcp(id: &'static str, label: &'static str, code: u8, domain: Domain, verified_write: bool) -> Setting {
    setting(id, label, Code::Vcp(code), domain, verified_write, false)
}

const fn vcp_locked(id: &'static str, label: &'static str, code: u8, domain: Domain, verified_write: bool) -> Setting {
    setting(id, label, Code::Vcp(code), domain, verified_write, true)
}

const fn aoc(id: &'static str, label: &'static str, item: u8, domain: Domain, verified_write: bool) -> Setting {
    setting(id, label, Code::Aoc(item), domain, verified_write, false)
}

const fn aoc_locked(id: &'static str, label: &'static str, item: u8, domain: Domain, verified_write: bool) -> Setting {
    setting(id, label, Code::Aoc(item), domain, verified_write, true)
}

pub const PROFILES: &[Profile] = &[
    Profile {
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
    },
    // Verified on a 27G1G4: every setting below was written, read back and
    // restored. The fixed game presets were tested separately with the monitor
    // in Off mode, since they are not adjustable in FPS, RTS or Racing.
    Profile {
        mfg: "AOC",
        product_code: 0x2701,
        model: "27G1G4",
        settings: &[
            // These override the common brightness/contrast entries so that a
            // write can be refused in the fixed FPS/RTS/Racing game modes.
            vcp_locked(
                "brightness",
                "Brightness",
                0x10,
                Domain::Range { min: 0, max: 100 },
                true,
            ),
            vcp_locked("contrast", "Contrast", 0x12, Domain::Range { min: 0, max: 100 }, true),
            vcp(
                "game-mode",
                "Game mode",
                GAME_MODE_VCP,
                Domain::Values(GAME_MODES),
                true,
            ),
            aoc_locked(
                "shadow-control",
                "Shadow control",
                0x58,
                Domain::Range { min: 0, max: 100 },
                true,
            ),
            aoc_locked(
                "game-color",
                "Game color",
                0x57,
                Domain::Range { min: 0, max: 20 },
                true,
            ),
            aoc_locked(
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
            // The monitor reports max 15 here, but its Eco modes use the same
            // seven values as the 24G4; Sports reads back as 5.
            aoc_locked(
                "eco",
                "Eco mode",
                0x11,
                Domain::Enum(&["Standard", "Text", "Internet", "Game", "Movie", "Sports", "Reading"]),
                true,
            ),
            aoc_locked("dcr", "DCR", 0x13, Domain::Enum(&["Off", "On"]), true),
            aoc("dial-point", "Dial point", 0x5A, Domain::Enum(&["Off", "On"]), true),
            aoc(
                "frame-counter",
                "Frame counter",
                0x5B,
                Domain::Enum(&["Off", "Top right", "Bottom right", "Bottom left", "Top left"]),
                true,
            ),
        ],
    },
];

/// Settings that switch a picture mode, in the order presets write them. A mode switch can
/// swap in that mode's own values (on the 24G4, HDR mode changes brightness and DCR on sets
/// Eco to a DCR-only value), so presets write these first and everything else after. Game
/// mode comes first because the fixed modes also lock several other settings.
pub const MODE_SETTINGS: &[&str] = &["game-mode", "dcr", "eco", "hdr-mode", "low-blue"];

pub fn is_aoc(edid: &Edid) -> bool {
    edid.mfg == "AOC"
}

/// Finds the profile for a monitor: manufacturer + product code, guarded by model name.
pub fn profile_for(edid: &Edid) -> Option<&'static Profile> {
    PROFILES
        .iter()
        .find(|p| p.mfg == edid.mfg && p.product_code == edid.product_code && edid.model.as_deref() == Some(p.model))
}

/// Settings available on a monitor: the common ones plus its profile's. A profile
/// can override a common setting (the 27G1G4 does for brightness and contrast).
pub fn settings_for(profile: Option<&'static Profile>) -> Vec<&'static Setting> {
    let extra = profile.map(|p| p.settings).unwrap_or(&[]);
    COMMON
        .iter()
        .filter(|c| !extra.iter().any(|s| s.id == c.id))
        .chain(extra)
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
    fn matches_the_27g1g4_with_verified_settings() {
        let p = profile_for(&edid("AOC", 0x2701, "27G1G4")).unwrap();
        assert_eq!(p.model, "27G1G4");
        for id in [
            "brightness",
            "contrast",
            "game-mode",
            "shadow-control",
            "game-color",
            "overdrive",
            "low-blue",
            "eco",
            "dcr",
            "dial-point",
            "frame-counter",
        ] {
            let s = p.settings.iter().find(|s| s.id == id).unwrap();
            assert!(s.verified_write, "{id} should be writable");
        }
        for id in [
            "brightness",
            "contrast",
            "shadow-control",
            "game-color",
            "overdrive",
            "eco",
            "dcr",
        ] {
            let s = p.settings.iter().find(|s| s.id == id).unwrap();
            assert!(s.locked_in_game_presets, "{id} should be locked in fixed modes");
        }
        for id in ["game-mode", "low-blue", "dial-point", "frame-counter"] {
            let s = p.settings.iter().find(|s| s.id == id).unwrap();
            assert!(!s.locked_in_game_presets, "{id} should stay available");
        }
    }

    #[test]
    fn the_27g1g4_overrides_common_brightness_and_contrast() {
        let p = profile_for(&edid("AOC", 0x2701, "27G1G4")).unwrap();
        let all = settings_for(Some(p));
        for id in ["brightness", "contrast"] {
            let matches: Vec<_> = all.iter().filter(|s| s.id == id).collect();
            assert_eq!(matches.len(), 1, "{id} should appear once");
            assert!(matches[0].locked_in_game_presets);
        }
        let low_blue = all.iter().find(|s| s.id == "low-blue").unwrap();
        assert!(!low_blue.locked_in_game_presets);
    }

    #[test]
    fn game_modes_parse_by_name_or_raw_value() {
        let p = profile_for(&edid("AOC", 0x2701, "27G1G4")).unwrap();
        let gm = p.settings.iter().find(|s| s.id == "game-mode").unwrap();
        assert_eq!(gm.label_for(0x0D), Some("Racing"));
        assert_eq!(gm.label_for(0x11), None);
        assert_eq!(gm.parse_value("racing"), Ok(0x0D));
        assert_eq!(gm.parse_value("Gamer 2"), Ok(0x0F));
        assert_eq!(gm.parse_value("13"), Ok(0x0D));
        assert!(gm.parse_value("1").is_err());
        assert!(gm.parse_value("17").is_err());
        assert!(gm.parse_value("custom").is_err());
    }

    #[test]
    fn only_fps_rts_and_racing_are_fixed_game_modes() {
        assert_eq!(preset_game_mode_label(0x0B), Some("FPS"));
        assert_eq!(preset_game_mode_label(0x0C), Some("RTS"));
        assert_eq!(preset_game_mode_label(0x0D), Some("Racing"));
        assert_eq!(preset_game_mode_label(0x00), None);
        assert_eq!(preset_game_mode_label(0x0E), None);
        assert_eq!(preset_game_mode_label(0x10), None);
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
