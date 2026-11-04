//! Decodes the parts of an EDID block that identify a monitor.

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Edid {
    /// Three-letter PNP manufacturer id, e.g. "AOC".
    pub mfg: String,
    pub product_code: u16,
    /// Model name descriptor (0xFC), e.g. "24G4".
    pub model: Option<String>,
    /// Serial number descriptor (0xFF).
    pub serial: Option<String>,
}

const HEADER: [u8; 8] = [0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00];

/// Parses the base 128-byte EDID block. Returns `None` if it isn't a valid EDID.
pub fn parse(raw: &[u8]) -> Option<Edid> {
    if raw.len() < 128 || raw[..8] != HEADER {
        return None;
    }
    if raw[..128].iter().fold(0u8, |acc, b| acc.wrapping_add(*b)) != 0 {
        return None;
    }
    let id = u16::from_be_bytes([raw[8], raw[9]]);
    let letter = |shift: u16| (b'A' - 1 + ((id >> shift) & 0x1F) as u8) as char;
    let mfg: String = [letter(10), letter(5), letter(0)].into_iter().collect();
    let mut edid = Edid {
        mfg,
        product_code: u16::from_le_bytes([raw[10], raw[11]]),
        model: None,
        serial: None,
    };
    for d in raw[54..126].chunks(18) {
        if d[..3] != [0, 0, 0] {
            continue; // detailed timing, not a text descriptor
        }
        let text = String::from_utf8_lossy(&d[5..18])
            .split('\n')
            .next()
            .unwrap_or("")
            .trim()
            .to_string();
        match d[3] {
            0xFC => edid.model = Some(text),
            0xFF => edid.serial = Some(text),
            _ => {}
        }
    }
    Some(edid)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// EDID base block of an AOC 24G4, with the serial replaced.
    pub fn aoc_24g4() -> Vec<u8> {
        let hex = "00ffffffffffff0005e301b43d01000018220104b5351e783ff7a5ac5049a8270b5054bfef00d1c0\
                   81803168317c4568457c6168617c023a801871382d40582c45000f282100001e000000ff00544553\
                   54303030303030303031000000fc00323447340a2020202020202020000000fd0030b4c8c83c010a\
                   20202020202001bb";
        let clean: String = hex.split_whitespace().collect();
        (0..clean.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&clean[i..i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn decodes_aoc_24g4() {
        let e = parse(&aoc_24g4()).unwrap();
        assert_eq!(e.mfg, "AOC");
        assert_eq!(e.product_code, 0xB401);
        assert_eq!(e.model.as_deref(), Some("24G4"));
        assert_eq!(e.serial.as_deref(), Some("TEST000000001"));
    }

    #[test]
    fn rejects_bad_header_and_checksum() {
        let mut raw = aoc_24g4();
        raw[127] ^= 1;
        assert!(parse(&raw).is_none());
        assert!(parse(&[0u8; 128]).is_none());
    }
}
