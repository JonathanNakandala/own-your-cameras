//! Keypad access policy: turns the keypad's buffer snapshots into decisions the runtime acts on.
//! Pure: the caller passes monotonic seconds and whether the strike is released, and opens the
//! strike, plays clips and reports to Home Assistant from the returned [`Decision`].
//!
//! Rules on top of [`crate::pin`]:
//! - a code that is still holding the strike open is not judged again: the keypad re-sends its
//!   buffer, and a `#` after a code that already matched arrives as a submission;
//! - a run of [`MAX_DIGITS`] digits that matched no code is refused like a wrong code. Without `#`
//!   every keystroke tests the window it ends, so an unbounded run would be free guessing.
use crate::config::Credential;
use crate::pin::{Change, Entry, MAX_DIGITS, Outcome, Verifier};
use zeroize::Zeroizing;

#[derive(Debug, PartialEq, Eq)]
pub enum Decision {
    /// Nothing to act on.
    None,
    /// Release the strike, play `clip`, report `name`.
    Grant { name: String, clip: String },
    /// Play the refusal and report it with the consecutive-failure count.
    Deny { failures: u32 },
}

pub struct Access {
    entry: Entry,
    verifier: Verifier,
    /// Digits of the code that released the strike, kept only while it is still released.
    holding: Option<Zeroizing<Vec<u8>>>,
}

impl Access {
    pub fn new(credentials: Vec<Credential>) -> Self {
        Self {
            entry: Entry::default(),
            verifier: Verifier::new(credentials),
            holding: None,
        }
    }

    /// The keypad link went away: whatever was typed is forgotten.
    pub fn reset(&mut self) {
        self.entry.reset();
    }

    /// One keypad buffer snapshot (`text`, ASCII graphic bytes).
    pub fn keypad(&mut self, text: &[u8], now_s: u64, strike_released: bool) -> Decision {
        if !strike_released {
            self.holding = None;
        }
        match self.entry.snapshot(text, now_s) {
            Change::None | Change::Cleared => Decision::None,
            Change::Edited(digits) => {
                if self.holds(&digits) {
                    return Decision::None;
                }
                if let Some(Outcome::Granted { name, clip }) = self.verifier.matches_suffix(&digits)
                {
                    self.entry.consume();
                    self.holding = Some(digits);
                    return Decision::Grant { name, clip };
                }
                if digits.len() >= MAX_DIGITS {
                    self.entry.consume();
                    return Decision::Deny {
                        failures: self.verifier.refuse(),
                    };
                }
                Decision::None
            }
            Change::Submitted(code) => {
                if self.holds(code.as_bytes()) {
                    return Decision::None;
                }
                match self.verifier.submit(&code) {
                    Outcome::Granted { name, clip } => {
                        self.holding = Some(Zeroizing::new(code.as_bytes().to_vec()));
                        Decision::Grant { name, clip }
                    }
                    Outcome::Denied { failures } => Decision::Deny { failures },
                }
            }
        }
    }

    fn holds(&self, digits: &[u8]) -> bool {
        self.holding.as_deref().map(Vec::as_slice) == Some(digits)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn access() -> Access {
        Access::new(crate::config::parse_pins("1234 resident resident\n987654 guest\n").unwrap())
    }

    fn resident() -> Decision {
        Decision::Grant {
            name: "resident".into(),
            clip: "resident".into(),
        }
    }

    #[test]
    fn a_code_opens_once_while_the_strike_holds() {
        let mut a = access();
        assert_eq!(a.keypad(b"1", 0, false), Decision::None);
        assert_eq!(a.keypad(b"1234", 1, false), resident());
        assert_eq!(
            a.keypad(b"1234#", 2, true),
            Decision::None,
            "`#` after the matched code is the same visitor"
        );
        assert_eq!(a.keypad(b"", 3, true), Decision::None);
        assert_eq!(
            a.keypad(b"1234", 3, true),
            Decision::None,
            "the keypad re-sending its buffer"
        );
        assert_eq!(a.keypad(b"", 4, true), Decision::None);
        assert_eq!(
            a.keypad(b"1234", 10, false),
            resident(),
            "once the strike is back the code opens again"
        );
    }

    #[test]
    fn hash_submissions_are_judged() {
        let mut a = access();
        assert_eq!(a.keypad(b"0000#", 0, false), Decision::Deny { failures: 1 });
        assert_eq!(a.keypad(b"", 1, false), Decision::None);
        assert_eq!(
            a.keypad(b"987654#", 2, false),
            Decision::Grant {
                name: "guest".into(),
                clip: "door_opened".into()
            },
            "the suffix match opened it on the last digit"
        );
    }

    #[test]
    fn a_long_unmatched_run_is_a_refusal_and_starts_over() {
        let mut a = access();
        let typed = b"55555555";
        for n in 1..typed.len() {
            assert_eq!(a.keypad(&typed[..n], n as u64, false), Decision::None);
        }
        assert_eq!(
            a.keypad(typed, 8, false),
            Decision::Deny { failures: 1 },
            "{MAX_DIGITS} digits without a match"
        );
        assert_eq!(a.keypad(b"555555551", 9, false), Decision::None);
        assert_eq!(
            a.keypad(b"5555555512", 10, false),
            Decision::None,
            "the next run is judged alone"
        );
        assert_eq!(a.keypad(b"555555551234", 12, false), resident());
    }

    #[test]
    fn a_reset_forgets_the_partial_entry() {
        let mut a = access();
        assert_eq!(a.keypad(b"12", 0, false), Decision::None);
        a.reset();
        assert_eq!(a.keypad(b"34", 1, false), Decision::None);
    }
}
