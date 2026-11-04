//! DDC/CI framing and commands: standard VCP get/set, capabilities, and AOC's
//! extended `F8 E1` page. Talks to the monitor through a [`Transport`], so tests
//! can script replies without hardware.

use std::thread::sleep;
use std::time::Duration;

/// Host address byte that starts every packet we send.
const HOST: u8 = 0x51;
/// Monitor's 8-bit write address (7-bit 0x37); seeds the request checksum.
const DISPLAY_WRITE: u8 = 0x6E;
/// Seeds the reply checksum (DDC/CI virtual host address).
const REPLY_SEED: u8 = 0x50;
/// AOC's extended page: requests carry `F8 E1 <item>`.
const AOC_PAGE: [u8; 2] = [0xF8, 0xE1];

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum DdcError {
    #[error("i2c error: {0}")]
    Io(String),
    #[error("monitor sent no reply")]
    NoReply,
    #[error("monitor reply failed its checksum")]
    BadChecksum,
    #[error("monitor reply was malformed: {0}")]
    Malformed(String),
    #[error("monitor does not support code {0:#04x}")]
    Unsupported(u8),
    #[error("monitor replied about {got:#04x}, expected {want:#04x} (stale reply)")]
    Stale { want: u8, got: u8 },
}

/// Raw byte pipe to the monitor's DDC/CI address.
pub trait Transport {
    fn write(&mut self, bytes: &[u8]) -> Result<(), DdcError>;
    fn read(&mut self, buf: &mut [u8]) -> Result<(), DdcError>;
}

/// Delays DDC/CI needs between packets. Tests use [`Timing::none`].
#[derive(Debug, Clone, Copy)]
pub struct Timing {
    /// After sending a request, before reading its reply.
    pub reply: Duration,
    /// After a set, before the next command.
    pub after_set: Duration,
    /// Extra wait before each retry.
    pub retry: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Timing {
            reply: Duration::from_millis(80),
            after_set: Duration::from_millis(200),
            retry: Duration::from_millis(100),
        }
    }
}

impl Timing {
    pub fn none() -> Self {
        Timing {
            reply: Duration::ZERO,
            after_set: Duration::ZERO,
            retry: Duration::ZERO,
        }
    }
}

/// A value and its maximum, as the monitor reports them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct Reading {
    pub value: u16,
    pub max: u16,
}

/// Frames `payload` as a host-to-display packet: `51 (80|len) payload.. chk`.
pub fn encode(payload: &[u8]) -> Vec<u8> {
    let mut packet = Vec::with_capacity(payload.len() + 3);
    packet.push(HOST);
    packet.push(0x80 | payload.len() as u8);
    packet.extend_from_slice(payload);
    let chk = packet.iter().fold(DISPLAY_WRITE, |acc, b| acc ^ b);
    packet.push(chk);
    packet
}

/// Checks a display-to-host packet `6E (80|len) payload.. chk` and returns its payload.
/// A zero-length payload is DDC/CI's null message ("nothing to say").
pub fn decode(raw: &[u8]) -> Result<&[u8], DdcError> {
    if raw.len() < 3 || raw.iter().all(|&b| b == 0 || b == 0xFF) {
        return Err(DdcError::NoReply);
    }
    if raw[0] != DISPLAY_WRITE || raw[1] & 0x80 == 0 {
        return Err(DdcError::Malformed(hex(&raw[..raw.len().min(4)])));
    }
    let len = (raw[1] & 0x7F) as usize;
    if raw.len() < len + 3 {
        return Err(DdcError::Malformed(format!("length {len} exceeds reply")));
    }
    let chk = raw[..len + 2].iter().fold(REPLY_SEED, |acc, b| acc ^ b);
    if chk != raw[len + 2] {
        return Err(DdcError::BadChecksum);
    }
    if len == 0 {
        return Err(DdcError::NoReply);
    }
    Ok(&raw[2..len + 2])
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ")
}

/// DDC/CI client over one monitor's transport.
pub struct Ddc<T: Transport> {
    transport: T,
    timing: Timing,
    attempts: u32,
}

impl<T: Transport> Ddc<T> {
    pub fn new(transport: T, timing: Timing) -> Self {
        Ddc {
            transport,
            timing,
            attempts: 3,
        }
    }

    #[cfg(test)]
    pub fn transport(&self) -> &T {
        &self.transport
    }

    /// Sends a read request and returns the reply payload. Reads change nothing, so a
    /// failed send, lost reply or garbled reply is retried.
    fn request(&mut self, payload: &[u8], reply_len: usize) -> Result<Vec<u8>, DdcError> {
        let mut last = DdcError::NoReply;
        for attempt in 0..self.attempts {
            if attempt > 0 {
                sleep(self.timing.retry);
            }
            if let Err(e) = self.transport.write(&encode(payload)) {
                last = e;
                continue;
            }
            sleep(self.timing.reply);
            let mut buf = vec![0u8; reply_len + 3];
            if let Err(e) = self.transport.read(&mut buf) {
                last = e;
                continue;
            }
            match decode(&buf) {
                Ok(p) => return Ok(p.to_vec()),
                Err(e @ (DdcError::NoReply | DdcError::BadChecksum)) => last = e,
                Err(e) => return Err(e),
            }
        }
        Err(last)
    }

    fn send(&mut self, payload: &[u8]) -> Result<(), DdcError> {
        self.transport.write(&encode(payload))?;
        sleep(self.timing.after_set);
        Ok(())
    }

    /// Reads a standard VCP feature. Reply: `02 rc code type mh ml sh sl`.
    pub fn get_vcp(&mut self, code: u8) -> Result<Reading, DdcError> {
        for _ in 0..self.attempts {
            let p = self.request(&[0x01, code], 8)?;
            if p.len() < 8 || p[0] != 0x02 {
                return Err(DdcError::Malformed(hex(&p)));
            }
            if p[2] != code {
                continue; // reply to an earlier request; ask again
            }
            if p[1] != 0x00 {
                return Err(DdcError::Unsupported(code));
            }
            return Ok(Reading {
                max: u16::from_be_bytes([p[4], p[5]]),
                value: u16::from_be_bytes([p[6], p[7]]),
            });
        }
        Err(DdcError::NoReply)
    }

    pub fn set_vcp(&mut self, code: u8, value: u16) -> Result<(), DdcError> {
        let [hi, lo] = value.to_be_bytes();
        self.send(&[0x03, code, hi, lo])
    }

    /// Reads an item on AOC's extended page. Reply: `02 00 F8 item mh ml vh vl`.
    pub fn get_aoc(&mut self, item: u8) -> Result<Reading, DdcError> {
        let mut got = 0;
        for _ in 0..self.attempts {
            let p = self.request(&[0x01, AOC_PAGE[0], AOC_PAGE[1], item], 8)?;
            if p.len() < 8 || p[0] != 0x02 || p[2] != AOC_PAGE[0] {
                return Err(DdcError::Malformed(hex(&p)));
            }
            if p[3] != item {
                got = p[3];
                continue;
            }
            if p[1] != 0x00 {
                return Err(DdcError::Unsupported(item));
            }
            return Ok(Reading {
                max: u16::from_be_bytes([p[4], p[5]]),
                value: u16::from_be_bytes([p[6], p[7]]),
            });
        }
        Err(DdcError::Stale { want: item, got })
    }

    pub fn set_aoc(&mut self, item: u8, value: u16) -> Result<(), DdcError> {
        let [hi, lo] = value.to_be_bytes();
        self.send(&[0x03, AOC_PAGE[0], AOC_PAGE[1], item, hi, lo])
    }

    /// Reads the capabilities string in fragments (`F3 off`, reply `E3 off data..`).
    pub fn capabilities(&mut self) -> Result<String, DdcError> {
        let mut out = Vec::new();
        loop {
            let [hi, lo] = (out.len() as u16).to_be_bytes();
            let p = self.request(&[0xF3, hi, lo], 35)?;
            if p.len() < 3 || p[0] != 0xE3 {
                return Err(DdcError::Malformed(hex(&p)));
            }
            if u16::from_be_bytes([p[1], p[2]]) as usize != out.len() {
                continue; // fragment for a different offset; ask again
            }
            let data = &p[3..];
            if data.is_empty() {
                break;
            }
            out.extend_from_slice(data);
            if out.len() > 4096 {
                return Err(DdcError::Malformed("capabilities string too long".into()));
            }
        }
        while out.last() == Some(&0) {
            out.pop();
        }
        Ok(String::from_utf8_lossy(&out).into_owned())
    }
}

#[cfg(test)]
pub mod fake {
    use super::*;
    use std::collections::VecDeque;

    /// Transport that records writes and plays back scripted replies.
    #[derive(Default)]
    pub struct Fake {
        pub writes: Vec<Vec<u8>>,
        pub replies: VecDeque<Vec<u8>>,
    }

    impl Fake {
        /// Queues a well-formed display reply carrying `payload`.
        pub fn reply(&mut self, payload: &[u8]) -> &mut Self {
            let mut raw = vec![DISPLAY_WRITE, 0x80 | payload.len() as u8];
            raw.extend_from_slice(payload);
            let chk = raw.iter().fold(REPLY_SEED, |acc, b| acc ^ b);
            raw.push(chk);
            self.replies.push_back(raw);
            self
        }
        pub fn raw(&mut self, raw: &[u8]) -> &mut Self {
            self.replies.push_back(raw.to_vec());
            self
        }
    }

    impl Transport for Fake {
        fn write(&mut self, bytes: &[u8]) -> Result<(), DdcError> {
            self.writes.push(bytes.to_vec());
            Ok(())
        }
        fn read(&mut self, buf: &mut [u8]) -> Result<(), DdcError> {
            let r = self.replies.pop_front().unwrap_or_default();
            let n = r.len().min(buf.len());
            buf[..n].copy_from_slice(&r[..n]);
            buf[n..].fill(0);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::Fake;
    use super::*;

    fn ddc(f: Fake) -> Ddc<Fake> {
        Ddc::new(f, Timing::none())
    }

    #[test]
    fn encodes_requests_like_the_monitor_expects() {
        // Captured from the working Linux probe on an AOC 24G4.
        assert_eq!(
            encode(&[0x01, 0xF8, 0xE1, 0x58]),
            vec![0x51, 0x84, 0x01, 0xF8, 0xE1, 0x58, 0xFB]
        );
        assert_eq!(encode(&[0x01, 0x10]), vec![0x51, 0x82, 0x01, 0x10, 0xAC]);
    }

    #[test]
    fn decodes_a_real_aoc_reply() {
        let raw = [0x6e, 0x88, 0x02, 0x00, 0xf8, 0x58, 0x00, 0x64, 0x00, 0x32, 0x42, 0x00];
        assert_eq!(decode(&raw).unwrap(), &raw[2..10]);
    }

    #[test]
    fn rejects_bad_checksum_and_null_replies() {
        let raw = [0x6e, 0x88, 0x02, 0x00, 0xf8, 0x58, 0x00, 0x64, 0x00, 0x32, 0x43];
        assert_eq!(decode(&raw), Err(DdcError::BadChecksum));
        assert_eq!(decode(&[0x6e, 0x80, 0xbe]), Err(DdcError::NoReply));
        assert_eq!(decode(&[0, 0, 0, 0]), Err(DdcError::NoReply));
    }

    #[test]
    fn aoc_get_reads_value_and_max() {
        let mut f = Fake::default();
        f.reply(&[0x02, 0x00, 0xF8, 0x58, 0x00, 0x64, 0x00, 0x32]);
        let mut d = ddc(f);
        assert_eq!(d.get_aoc(0x58).unwrap(), Reading { value: 50, max: 100 });
    }

    #[test]
    fn aoc_get_skips_a_stale_reply_for_another_item() {
        let mut f = Fake::default();
        f.reply(&[0x02, 0x00, 0xF8, 0x57, 0x00, 0x14, 0x00, 0x0a]);
        f.reply(&[0x02, 0x00, 0xF8, 0x58, 0x00, 0x64, 0x00, 0x32]);
        let mut d = ddc(f);
        assert_eq!(d.get_aoc(0x58).unwrap().value, 50);
        assert_eq!(d.transport.writes.len(), 2);
    }

    #[test]
    fn aoc_get_gives_up_on_repeated_stale_replies() {
        let mut f = Fake::default();
        for _ in 0..3 {
            f.reply(&[0x02, 0x00, 0xF8, 0x57, 0x00, 0x14, 0x00, 0x0a]);
        }
        assert_eq!(ddc(f).get_aoc(0x58), Err(DdcError::Stale { want: 0x58, got: 0x57 }));
    }

    #[test]
    fn retries_after_a_garbled_reply() {
        let mut f = Fake::default();
        f.raw(&[0x6e, 0x88, 0x02, 0x00, 0xf8, 0x58, 0x00, 0x64, 0x00, 0x32, 0x00]);
        f.reply(&[0x02, 0x00, 0xF8, 0x58, 0x00, 0x64, 0x00, 0x32]);
        assert_eq!(ddc(f).get_aoc(0x58).unwrap().value, 50);
    }

    #[test]
    fn no_reply_after_all_attempts_is_an_error() {
        assert_eq!(ddc(Fake::default()).get_aoc(0x58), Err(DdcError::NoReply));
    }

    #[test]
    fn aoc_set_sends_big_endian_value() {
        let mut d = ddc(Fake::default());
        d.set_aoc(0x58, 51).unwrap();
        assert_eq!(d.transport.writes, vec![encode(&[0x03, 0xF8, 0xE1, 0x58, 0x00, 51])]);
    }

    #[test]
    fn vcp_get_reports_unsupported_codes() {
        let mut f = Fake::default();
        f.reply(&[0x02, 0x01, 0x99, 0x00, 0x00, 0x00, 0x00, 0x00]);
        assert_eq!(ddc(f).get_vcp(0x99), Err(DdcError::Unsupported(0x99)));
    }

    #[test]
    fn capabilities_joins_fragments() {
        let mut f = Fake::default();
        f.reply(&[0xE3, 0x00, 0x00, b'(', b'a', b'b']);
        f.reply(&[0xE3, 0x00, 0x03, b'c', b')', 0x00]);
        f.reply(&[0xE3, 0x00, 0x06]);
        let mut d = ddc(f);
        assert_eq!(d.capabilities().unwrap(), "(abc)");
        assert_eq!(d.transport.writes[1], encode(&[0xF3, 0x00, 0x03]));
    }
}
