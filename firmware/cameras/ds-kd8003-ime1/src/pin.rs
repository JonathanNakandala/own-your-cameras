//! PIN entry and verification: follows the keypad's cumulative entry buffer and decides on codes. A
//! code opens as soon as its digits are the last ones typed, no `#` needed; `#`
//! still works as an explicit submit and a submit that matches nothing is a refusal. Pure: the
//! caller supplies monotonic seconds and acts on the outcome. Digits never leave this module in any
//! `Debug`/`Display` output.
//!
//! Policy: codes are 4..8 digits; a refusal never locks the keypad (the stock 300 s lockout after
//! three failures cost more than it protected). Every refusal carries the
//! consecutive-failure count so Home Assistant can alert or act on a guessing streak; a success
//! resets the count, and it restarts at zero after a reboot.
use crate::config::Credential;
use zeroize::{Zeroize, Zeroizing};

pub const MIN_DIGITS: usize = 4;
pub const MAX_DIGITS: usize = 8;
/// A partially typed code older than this no longer counts towards the next submission.
pub const ENTRY_TIMEOUT_SECS: u64 = 20;

/// Follows the keypad's entry buffer snapshots. The keypad reports the whole current text (digits,
/// `#`, `*`); `#` ends an entry as on the stock firmware (`#<digits>#` or `<digits>#`).
#[derive(Default)]
pub struct Entry {
    previous: Zeroizing<Vec<u8>>,
    started_s: Option<u64>,
    /// Characters typed before a timeout; a later submission only counts the digits after them.
    stale: usize,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Change {
    /// Nothing new, or the keypad is idle.
    None,
    /// Characters were typed or erased; carries the live trailing digits (typed since the last
    /// timeout, grant or non-digit key), for the no-`#` match.
    Edited(Zeroizing<Vec<u8>>),
    /// The visitor pressed `#` after some digits.
    Submitted(Zeroizing<String>),
    /// The buffer went back to empty (keypad cleared it or `*` was pressed).
    Cleared,
}

impl Entry {
    /// `text` is the keypad's current buffer (ASCII graphic bytes only).
    pub fn snapshot(&mut self, text: &[u8], now_s: u64) -> Change {
        if text == self.previous.as_slice() {
            if let Some(started) = self.started_s
                && !text.is_empty()
                && now_s.saturating_sub(started) > ENTRY_TIMEOUT_SECS
            {
                // Whatever is in the buffer now belongs to nobody: a later visitor's digits are judged alone.
                self.stale = text.len();
                self.started_s = None;
            }
            return Change::None;
        }
        let was_empty = self.previous.is_empty();
        self.previous.zeroize();
        self.previous.extend_from_slice(text);
        if text.is_empty() {
            self.started_s = None;
            self.stale = 0;
            return Change::Cleared;
        }
        if was_empty || self.started_s.is_none() {
            self.started_s = Some(now_s);
        }
        if text.len() < self.stale {
            // Erased into the stale part: it is live again.
            self.stale = text.len();
        }
        if text.len() >= 2 && text.ends_with(b"#") {
            let body = &text[..text.len() - 1];
            let from = body
                .iter()
                .rposition(|&b| b == b'#')
                .map_or(0, |p| p + 1)
                .max(self.stale.min(body.len()));
            let digits = &body[from..];
            if !digits.is_empty() && digits.iter().all(u8::is_ascii_digit) {
                let code = Zeroizing::new(String::from_utf8_lossy(digits).into_owned());
                self.started_s = None;
                self.stale = text.len();
                return Change::Submitted(code);
            }
        }
        let live = &text[self.stale.min(text.len())..];
        let from = live
            .iter()
            .rposition(|b| !b.is_ascii_digit())
            .map_or(0, |p| p + 1);
        Change::Edited(Zeroizing::new(live[from..].to_vec()))
    }

    /// The digits typed so far have been judged (a grant): later ones are judged alone.
    pub fn consume(&mut self) {
        self.stale = self.previous.len();
        self.started_s = None;
    }

    pub fn reset(&mut self) {
        self.previous.zeroize();
        self.started_s = None;
        self.stale = 0;
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Granted { name: String, clip: String },
    Denied { failures: u32 },
}

/// Verifies submitted codes against the configured table and counts consecutive failures.
pub struct Verifier {
    credentials: Vec<Credential>,
    failures: u32,
}

impl Verifier {
    pub fn new(credentials: Vec<Credential>) -> Self {
        Self {
            credentials,
            failures: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.credentials.len()
    }

    pub fn is_empty(&self) -> bool {
        self.credentials.is_empty()
    }

    /// A code whose digits end the typed run opens without `#`.
    pub fn matches_suffix(&mut self, typed: &[u8]) -> Option<Outcome> {
        let c = self.credentials.iter().find(|c| {
            let pin = c.pin.as_bytes();
            typed.len() >= pin.len() && constant_time_eq(&typed[typed.len() - pin.len()..], pin)
        })?;
        self.failures = 0;
        Some(Outcome::Granted {
            name: c.name.clone(),
            clip: c.clip.clone(),
        })
    }

    /// A refusal that is not a submitted code (see [`crate::access`]); returns the new count.
    pub fn refuse(&mut self) -> u32 {
        self.failures += 1;
        self.failures
    }

    pub fn submit(&mut self, code: &str) -> Outcome {
        let hit = (MIN_DIGITS..=MAX_DIGITS).contains(&code.len())
            && self
                .credentials
                .iter()
                .any(|c| constant_time_eq(c.pin.as_bytes(), code.as_bytes()));
        if hit {
            self.failures = 0;
            let c = self
                .credentials
                .iter()
                .find(|c| c.pin.as_str() == code)
                .expect("matched above");
            return Outcome::Granted {
                name: c.name.clone(),
                clip: c.clip.clone(),
            };
        }
        self.failures += 1;
        Outcome::Denied {
            failures: self.failures,
        }
    }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table() -> Vec<Credential> {
        crate::config::parse_pins("1234 resident resident\n987654 guest\n").unwrap()
    }

    fn code(s: &str) -> Change {
        Change::Submitted(Zeroizing::new(s.into()))
    }

    fn typed(d: &str) -> Change {
        Change::Edited(Zeroizing::new(d.as_bytes().to_vec()))
    }

    #[test]
    fn entry_follows_stock_framing() {
        let mut e = Entry::default();
        assert_eq!(e.snapshot(b"", 0), Change::None);
        assert_eq!(e.snapshot(b"#", 1), typed(""));
        assert_eq!(e.snapshot(b"#12", 2), typed("12"));
        assert_eq!(e.snapshot(b"#12", 3), Change::None);
        assert_eq!(e.snapshot(b"#1234#", 4), code("1234"));
        assert_eq!(e.snapshot(b"", 5), Change::Cleared);
        assert_eq!(e.snapshot(b"5678#", 6), code("5678"));
        assert_eq!(e.snapshot(b"", 7), Change::Cleared);
        assert_eq!(
            e.snapshot(b"##", 8),
            typed(""),
            "no digits is not a submission"
        );
        assert_eq!(e.snapshot(b"", 9), Change::Cleared);
        assert_eq!(
            e.snapshot(b"12*4#", 10),
            typed(""),
            "non-digits are not a code"
        );
    }

    #[test]
    fn digits_open_without_hash_and_are_judged_once() {
        let mut e = Entry::default();
        let mut v = Verifier::new(table());
        assert_eq!(e.snapshot(b"9", 0), typed("9"));
        assert!(v.matches_suffix(b"9").is_none());
        assert_eq!(e.snapshot(b"91234", 1), typed("91234"));
        assert!(matches!(
            v.matches_suffix(b"91234"),
            Some(Outcome::Granted { .. })
        ));
        e.consume();
        assert_eq!(
            e.snapshot(b"912345", 2),
            typed("5"),
            "after a grant only new digits count"
        );
        assert!(v.matches_suffix(b"5").is_none());
        assert_eq!(
            e.snapshot(b"912345#", 3),
            code("5"),
            "# still submits, this one is refused"
        );
        assert_eq!(v.submit("5"), Outcome::Denied { failures: 1 });
    }

    #[test]
    fn a_stale_prefix_does_not_join_the_next_visitors_code() {
        let mut e = Entry::default();
        assert_eq!(e.snapshot(b"1", 100), typed("1"));
        assert_eq!(e.snapshot(b"1", 100 + ENTRY_TIMEOUT_SECS + 1), Change::None);
        assert_eq!(e.snapshot(b"12", 200), typed("2"));
        assert_eq!(
            e.snapshot(b"12345#", 201),
            code("2345"),
            "only the digits typed after the timeout"
        );
        assert_eq!(e.snapshot(b"", 202), Change::Cleared);
        assert_eq!(e.snapshot(b"1234#", 203), code("1234"));
        let mut e = Entry::default();
        assert_eq!(e.snapshot(b"12", 0), typed("12"));
        assert_eq!(e.snapshot(b"12", ENTRY_TIMEOUT_SECS + 1), Change::None);
        assert_eq!(
            e.snapshot(b"1", ENTRY_TIMEOUT_SECS + 2),
            typed(""),
            "the kept digit stays stale; only what is typed after it counts"
        );
        assert_eq!(e.snapshot(b"1234#", ENTRY_TIMEOUT_SECS + 3), code("234"));
    }

    #[test]
    fn grants_known_codes_and_counts_failures_without_locking() {
        let mut v = Verifier::new(table());
        let resident = Outcome::Granted {
            name: "resident".into(),
            clip: "resident".into(),
        };
        assert_eq!(v.submit("1234"), resident);
        assert_eq!(v.submit("0000"), Outcome::Denied { failures: 1 });
        assert_eq!(v.submit("0001"), Outcome::Denied { failures: 2 });
        assert_eq!(v.submit("1234"), resident, "a success resets the count");
        for n in 1..=20 {
            assert_eq!(v.submit("0000"), Outcome::Denied { failures: n });
        }
        assert_eq!(
            v.submit("987654"),
            Outcome::Granted {
                name: "guest".into(),
                clip: "door_opened".into()
            },
            "no lockout, ever"
        );
    }

    #[test]
    fn length_bounds_apply_before_lookup() {
        let mut v = Verifier::new(table());
        assert_eq!(v.submit("123"), Outcome::Denied { failures: 1 });
        assert_eq!(v.submit("123456789"), Outcome::Denied { failures: 2 });
        assert!(!v.is_empty() && v.len() == 2);
    }
}
