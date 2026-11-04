//! `aocctl verify`: checks that a setting's write works on the user's monitor by
//! writing one step away from its current value, reading it back, and restoring
//! the original. The CLI prints the result as a report a maintainer uses to mark
//! the setting `verified_write` for that model.
//!
//! Only settings with an established domain (the model's profile, brightness and
//! contrast) are tested. Every value it relies on comes from a fresh reply: a
//! marker read of another code goes first, so the monitor's stale reply buffer
//! (which repeats its last answer) can never pass as the setting's value.

use crate::ddc::{Ddc, DdcError, Reading, Transport};
use crate::models::{Code, Domain, Setting};
use crate::monitor;

/// Standard VCP code of the monitor's game mode (MCCS "display application").
pub const GAME_MODE: u8 = 0xDC;
/// Read-backs per write: the value may still be settling after the first.
const POLLS: usize = 3;

/// A read during `verify`: the value and maximum, or why it failed.
pub type Read = Result<Reading, String>;

/// One write `verify` sent and what it read back afterwards.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub value: u16,
    /// The i2c error the write reported, if any. The monitor may still have taken it.
    pub send_error: Option<String>,
    /// The last fresh read-back.
    pub read_back: Read,
}

impl Step {
    pub fn confirmed(&self) -> bool {
        matches!(&self.read_back, Ok(r) if r.value == self.value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The test value and the restored value both read back.
    Pass,
    /// The original value is back, but the test value didn't read back.
    Fail,
    /// The restore didn't read back: the monitor may still hold the test value.
    RestoreUnconfirmed,
    /// The monitor's identity changed before the restore, so nothing more was sent.
    RestoreSkipped,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verification {
    pub before: Reading,
    /// Raw values the setting may take on this monitor: its domain cut to the reported maximum.
    pub domain: (u16, u16),
    pub game_mode_before: Option<u16>,
    pub test: Step,
    /// The restore write, or why it was skipped.
    pub restore: Result<Step, String>,
    pub game_mode_after: Option<u16>,
    pub outcome: Outcome,
}

/// Raw values the setting accepts on this monitor, or None if there are none.
pub fn effective_domain(setting: &Setting, reported_max: u16) -> Option<(u16, u16)> {
    let (lo, hi) = match setting.domain {
        Domain::Range { min, max } => (min, max),
        Domain::Enum(names) => (0, (names.len() as u16).checked_sub(1)?),
    };
    let hi = hi.min(reported_max);
    (lo <= hi).then_some((lo, hi))
}

/// The value to test: one step down if possible, else one step up.
pub fn test_value(current: u16, (lo, hi): (u16, u16)) -> Option<u16> {
    if current > lo {
        Some(current - 1)
    } else if current < hi {
        Some(current + 1)
    } else {
        None
    }
}

/// Reads the setting right after reading a different code, so a reply can't be
/// the monitor's stale answer to an earlier read of the same setting.
fn fresh_read<T: Transport>(ddc: &mut Ddc<T>, setting: &Setting) -> Read {
    let marker = if setting.code == Code::Vcp(0x10) { 0x12 } else { 0x10 };
    ddc.get_vcp(marker)
        .map_err(|e| format!("marker read before it failed: {e}"))?;
    monitor::read(ddc, setting).map_err(|e| e.to_string())
}

fn send<T: Transport>(ddc: &mut Ddc<T>, setting: &Setting, value: u16) -> Result<(), DdcError> {
    match setting.code {
        Code::Vcp(c) => ddc.set_vcp(c, value),
        Code::Aoc(i) => ddc.set_aoc(i, value),
    }
}

/// Writes `value` once, then reads it back until it matches (a few fresh reads at most).
fn step<T: Transport>(ddc: &mut Ddc<T>, setting: &Setting, value: u16) -> Step {
    let send_error = send(ddc, setting, value).err().map(|e| e.to_string());
    let mut read_back = fresh_read(ddc, setting);
    for _ in 1..POLLS {
        if matches!(&read_back, Ok(r) if r.value == value) {
            break;
        }
        ddc.pause();
        read_back = fresh_read(ddc, setting);
    }
    Step {
        value,
        send_error,
        read_back,
    }
}

/// Runs the check. `recheck` confirms the monitor's identity (call it while holding
/// the bus lock); it runs before each write. `announce(before, test)` runs just
/// before the first write. Err means it stopped before writing anything.
pub fn verify<T: Transport>(
    ddc: &mut Ddc<T>,
    setting: &Setting,
    mut recheck: impl FnMut() -> Result<(), String>,
    announce: impl FnOnce(u16, u16),
) -> Result<Verification, String> {
    let before = fresh_read(ddc, setting).map_err(|e| format!("couldn't read {}: {e}", setting.id))?;
    let domain = effective_domain(setting, before.max).ok_or_else(|| {
        format!(
            "{} has no values to test below the reported maximum {}",
            setting.id, before.max
        )
    })?;
    if before.value < domain.0 || before.value > domain.1 {
        return Err(format!(
            "{} is {}, outside its known values {}..={}; not touching it",
            setting.id, before.value, domain.0, domain.1
        ));
    }
    let test = test_value(before.value, domain).ok_or_else(|| format!("{} has only one value", setting.id))?;
    let game_mode_before = ddc.get_vcp(GAME_MODE).ok().map(|r| r.value);
    recheck()?;
    announce(before.value, test);

    let test = step(ddc, setting, test);
    let restore = recheck().map(|()| step(ddc, setting, before.value));
    let game_mode_after = ddc.get_vcp(GAME_MODE).ok().map(|r| r.value);
    let outcome = match &restore {
        Err(_) => Outcome::RestoreSkipped,
        Ok(r) if !r.confirmed() => Outcome::RestoreUnconfirmed,
        Ok(_) if test.confirmed() => Outcome::Pass,
        Ok(_) => Outcome::Fail,
    };
    Ok(Verification {
        before,
        domain,
        game_mode_before,
        test,
        restore,
        game_mode_after,
        outcome,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ddc::{Timing, encode, fake::Fake};

    const SHADOW: Setting = Setting {
        id: "shadow-control",
        label: "Shadow control",
        code: Code::Aoc(0x58),
        domain: Domain::Range { min: 0, max: 100 },
        verified_write: false,
    };

    fn shadow(f: &mut Fake, value: u8) {
        f.reply(&[0x02, 0x00, 0xF8, 0x58, 0x00, 100, 0x00, value]);
    }
    fn marker(f: &mut Fake) {
        f.reply(&[0x02, 0x00, 0x10, 0x00, 0x00, 100, 0x00, 20]);
    }
    /// A fresh read of shadow control: marker reply, then the value.
    fn fresh(f: &mut Fake, value: u8) {
        marker(f);
        shadow(f, value);
    }
    fn game_mode(f: &mut Fake) {
        f.reply(&[0x02, 0x00, GAME_MODE, 0x00, 0x00, 16, 0x00, 0]);
    }
    fn set(value: u8) -> Vec<u8> {
        encode(&[0x03, 0xF8, 0xE1, 0x58, 0x00, value])
    }
    fn sets(d: &Ddc<Fake>) -> Vec<Vec<u8>> {
        d.transport().writes.iter().filter(|w| w[2] == 0x03).cloned().collect()
    }
    fn run(f: Fake) -> (Result<Verification, String>, Ddc<Fake>) {
        let mut d = Ddc::new(f, Timing::none());
        let r = verify(&mut d, &SHADOW, || Ok(()), |_, _| {});
        (r, d)
    }

    #[test]
    fn test_value_steps_down_first_and_stays_in_the_domain() {
        assert_eq!(test_value(50, (0, 100)), Some(49));
        assert_eq!(test_value(0, (0, 100)), Some(1));
        assert_eq!(test_value(3, (3, 3)), None);
        let overdrive = Setting {
            domain: Domain::Enum(&["Off", "Weak", "Medium", "Strong"]),
            ..SHADOW
        };
        assert_eq!(effective_domain(&overdrive, 3), Some((0, 3)));
        assert_eq!(effective_domain(&overdrive, 2), Some((0, 2)));
        assert_eq!(effective_domain(&SHADOW, 60), Some((0, 60)));
        let high_min = Setting {
            domain: Domain::Range { min: 10, max: 20 },
            ..SHADOW
        };
        assert_eq!(effective_domain(&high_min, 5), None);
    }

    #[test]
    fn pass_writes_exactly_the_test_value_then_the_original() {
        let mut f = Fake::default();
        fresh(&mut f, 50);
        game_mode(&mut f);
        fresh(&mut f, 49);
        fresh(&mut f, 50);
        game_mode(&mut f);
        let (r, d) = run(f);
        let v = r.unwrap();
        assert_eq!(v.outcome, Outcome::Pass);
        assert_eq!((v.game_mode_before, v.game_mode_after), (Some(0), Some(0)));
        assert_eq!(sets(&d), vec![set(49), set(50)]);
    }

    #[test]
    fn a_test_value_that_doesnt_read_back_still_restores() {
        let mut f = Fake::default();
        fresh(&mut f, 50);
        game_mode(&mut f);
        for _ in 0..POLLS {
            fresh(&mut f, 50); // monitor ignored the test value
        }
        fresh(&mut f, 50);
        let (r, d) = run(f);
        assert_eq!(r.unwrap().outcome, Outcome::Fail);
        assert_eq!(sets(&d), vec![set(49), set(50)]);
    }

    #[test]
    fn a_failed_send_still_waits_reads_and_restores() {
        let mut f = Fake::default();
        fresh(&mut f, 50);
        game_mode(&mut f);
        fresh(&mut f, 49); // the packet got through despite the error
        fresh(&mut f, 50);
        // Writes: marker, get, game-mode, test set (index 3).
        f.failing_writes = vec![3];
        let (r, d) = run(f);
        let v = r.unwrap();
        assert!(v.test.send_error.is_some());
        assert_eq!(v.outcome, Outcome::Pass);
        assert_eq!(sets(&d), vec![set(49), set(50)]);
    }

    #[test]
    fn stale_replies_never_count_as_the_setting() {
        // After every marker the monitor repeats its last reply (the marker's), so
        // nothing about shadow control is ever fresh.
        let mut f = Fake::default();
        for _ in 0..6 {
            marker(&mut f);
        }
        let (r, d) = run(f);
        assert!(r.is_err());
        assert!(sets(&d).is_empty());
    }

    #[test]
    fn an_unconfirmed_restore_is_reported_not_retried() {
        let mut f = Fake::default();
        fresh(&mut f, 50);
        game_mode(&mut f);
        fresh(&mut f, 49);
        for _ in 0..POLLS {
            fresh(&mut f, 49); // restore not taken
        }
        let (r, d) = run(f);
        assert_eq!(r.unwrap().outcome, Outcome::RestoreUnconfirmed);
        assert_eq!(sets(&d), vec![set(49), set(50)]);
    }

    #[test]
    fn identity_change_before_the_restore_sends_nothing_more() {
        let mut f = Fake::default();
        fresh(&mut f, 50);
        game_mode(&mut f);
        fresh(&mut f, 49);
        let mut d = Ddc::new(f, Timing::none());
        let mut calls = 0;
        let recheck = || {
            calls += 1;
            if calls == 1 {
                Ok(())
            } else {
                Err("DP-1 changed".to_string())
            }
        };
        let v = verify(&mut d, &SHADOW, recheck, |_, _| {}).unwrap();
        assert_eq!(v.outcome, Outcome::RestoreSkipped);
        assert_eq!(sets(&d), vec![set(49)]);
    }

    #[test]
    fn nothing_is_written_when_the_start_is_unsafe() {
        // Identity changed before the first write.
        let mut f = Fake::default();
        fresh(&mut f, 50);
        let mut d = Ddc::new(f, Timing::none());
        assert!(verify(&mut d, &SHADOW, || Err("changed".into()), |_, _| {}).is_err());
        assert!(sets(&d).is_empty());
        // Current value above the reported maximum.
        let mut f = Fake::default();
        marker(&mut f);
        f.reply(&[0x02, 0x00, 0xF8, 0x58, 0x00, 60, 0x00, 61]);
        let (r, d) = run(f);
        assert!(r.is_err());
        assert!(sets(&d).is_empty());
    }
}
