//! The intercom call: the one owner of call policy. A press rings, any device answers, joins, leaves,
//! sets its own microphone flag or opens the strike; the call ends when the last participant leaves,
//! at the maximum talk time, on a decline, or as a missed call at the ring timeout.
//!
//! Pure and time-injected: monotonic milliseconds in, [`Effect`]s out. The runtime executes the
//! effects (speaker, strike, publications) and feeds every press, command and lost lease through
//! [`Call::handle`] and every loop pass through [`Call::tick`]. Wall-clock times are the publisher's.
use std::collections::BTreeMap;

/// Four random bytes, shown as 8 hex digits.
pub type CallId = [u8; 4];

/// The participant id of the panel on the direct link. MQTT may not use it.
pub const PANEL: &str = "panel";
/// `ended` lasts this long before the call returns to `idle`.
pub const ENDED_MS: u64 = 4_000;
/// Longest device id.
pub const MAX_DEVICE: usize = 32;

pub fn hex(id: &CallId) -> String {
    id.iter().map(|b| format!("{b:02x}")).collect()
}

/// `a3f9c2e1` → the id; anything else → `None`.
pub fn parse_id(text: &str) -> Option<CallId> {
    if text.len() != 8 || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let mut id = [0u8; 4];
    for (i, b) in id.iter_mut().enumerate() {
        *b = u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(id)
}

/// A participant: `panel`, or a card's random id; 1..=32 of `[A-Za-z0-9_-]`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct DeviceId(String);

impl DeviceId {
    pub fn new(id: &str) -> Option<Self> {
        let ok = !id.is_empty()
            && id.len() <= MAX_DEVICE
            && id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
        ok.then(|| Self(id.to_owned()))
    }
    pub fn panel() -> Self {
        Self(PANEL.to_owned())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
    /// The panel's lease is its link; every other device keeps its own with `keep`.
    fn leased(&self) -> bool {
        self.0 != PANEL
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Answer { mic: bool },
    Join,
    Decline,
    Leave,
    Mic(bool),
    Keep,
    Open,
    ClearMissed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    Rejected,
    Missed,
    Hangup,
    MaxTalk,
}

impl Reason {
    pub fn name(self) -> &'static str {
        match self {
            Self::Rejected => "rejected",
            Self::Missed => "missed",
            Self::Hangup => "hangup",
            Self::MaxTalk => "max_talk",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Input<'a> {
    Pressed,
    Command {
        device: &'a DeviceId,
        call: Option<CallId>,
        op: Op,
    },
    LeaseLost(&'a DeviceId),
}

/// Short generated sounds at the gate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    /// Two rising notes: someone answered with the microphone on.
    Answered,
    /// Two falling notes: declined.
    Declined,
    /// Two short beeps: hang-up or maximum talk time.
    Ended,
    /// Nobody answered: the wrong-PIN sound (the `denied` clip, else the built-in deny tone).
    Missed,
}

/// `call/event` and the `event` entity's types.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventKind {
    Ringing,
    Answered,
    Declined,
    Missed,
    Ended,
    MissedCleared,
}

impl EventKind {
    pub const ALL: [&'static str; 6] = [
        "ringing",
        "answered",
        "declined",
        "missed",
        "ended",
        "missed_cleared",
    ];
    pub fn name(self) -> &'static str {
        match self {
            Self::Ringing => "ringing",
            Self::Answered => "answered",
            Self::Declined => "declined",
            Self::Missed => "missed",
            Self::Ended => "ended",
            Self::MissedCleared => "missed_cleared",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CallEvent {
    pub kind: EventKind,
    pub call_id: CallId,
    pub reason: Option<Reason>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    StartRingback,
    StopRingback,
    Tone(Tone),
    /// Open the pedestrian strike through the strike owner, with `door_opened`.
    OpenStrike,
    /// The snapshot changed: publish it on every transport.
    Changed,
    /// The missed mark changed: persist and publish it.
    MissedChanged,
    Event(CallEvent),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The command names another call than the current one.
    Stale,
    /// The command does not apply in this phase (or from this device).
    NotNow,
    /// The device id is malformed or not allowed on this transport.
    BadDevice,
}

impl Refusal {
    pub fn name(self) -> &'static str {
        match self {
            Self::Stale => "stale",
            Self::NotNow => "not_now",
            Self::BadDevice => "bad_device",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub ring_timeout_ms: u64,
    pub max_talk_ms: u64,
    pub card_lease_ms: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            ring_timeout_ms: 45_000,
            max_talk_ms: 180_000,
            card_lease_ms: 30_000,
        }
    }
}

/// The last missed call. `at_ms` is monotonic and only known for a mark made since this boot;
/// `wall_ms` (Unix milliseconds) is set by the runtime when its clock is trusted, and survives a reboot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Missed {
    pub call_id: CallId,
    pub at_ms: Option<u64>,
    pub wall_ms: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Participant {
    pub mic: bool,
    /// `None` for the panel (its link is its lease).
    pub lease_until: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Idle,
    Ringing {
        id: CallId,
        since: u64,
    },
    Active {
        id: CallId,
        since: u64,
        answered_at: u64,
    },
    Ended {
        id: CallId,
        reason: Reason,
        at: u64,
    },
}

impl Phase {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Ringing { .. } => "ringing",
            Self::Active { .. } => "active",
            Self::Ended { .. } => "ended",
        }
    }
    fn id(&self) -> Option<CallId> {
        match *self {
            Self::Idle => None,
            Self::Ringing { id, .. } | Self::Active { id, .. } | Self::Ended { id, .. } => Some(id),
        }
    }
}

/// What every transport publishes; times are monotonic milliseconds of this boot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub seq: u64,
    pub phase: Phase,
    pub participants: BTreeMap<String, bool>,
    pub missed: Option<Missed>,
    pub now_ms: u64,
}

#[derive(Debug)]
pub struct Call {
    phase: Phase,
    participants: BTreeMap<DeviceId, Participant>,
    missed: Option<Missed>,
    seq: u64,
}

impl Call {
    /// `_new_id` is accepted for symmetry with [`Call::handle`]; a call id is only drawn on a press.
    pub fn new(_new_id: CallId, missed: Option<Missed>) -> Self {
        Self {
            phase: Phase::Idle,
            participants: BTreeMap::new(),
            missed,
            seq: 1,
        }
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    pub fn missed(&self) -> Option<Missed> {
        self.missed
    }

    /// The runtime stamps a fresh mark with the wall clock when it trusts it.
    pub fn stamp_missed(&mut self, wall_ms: u64) {
        if let Some(m) = self.missed.as_mut() {
            m.wall_ms = Some(wall_ms);
        }
    }

    pub fn handle(
        &mut self,
        now_ms: u64,
        input: Input,
        limits: &Limits,
        fresh_id: CallId,
    ) -> Result<Vec<Effect>, Refusal> {
        let mut fx = Vec::new();
        match input {
            Input::Pressed => match self.phase {
                Phase::Idle | Phase::Ended { .. } => {
                    self.participants.clear();
                    self.phase = Phase::Ringing {
                        id: fresh_id,
                        since: now_ms,
                    };
                    fx.push(Effect::StartRingback);
                    fx.push(self.event(EventKind::Ringing, fresh_id, None));
                    self.changed(&mut fx);
                }
                // A repeat press restarts nothing; a press during a conversation is ignored.
                Phase::Ringing { .. } | Phase::Active { .. } => return Err(Refusal::NotNow),
            },
            Input::LeaseLost(device) => self.leave(device, now_ms, &mut fx),
            Input::Command { device, call, op } => {
                self.command(now_ms, device, call, op, limits, &mut fx)?
            }
        }
        Ok(fx)
    }

    fn command(
        &mut self,
        now_ms: u64,
        device: &DeviceId,
        call: Option<CallId>,
        op: Op,
        limits: &Limits,
        fx: &mut Vec<Effect>,
    ) -> Result<(), Refusal> {
        match op {
            Op::ClearMissed => {
                if let Some(m) = self.missed.take() {
                    fx.push(Effect::MissedChanged);
                    fx.push(self.event(EventKind::MissedCleared, m.call_id, None));
                    self.changed(fx);
                }
                return Ok(());
            }
            // Opening is a door command first: an open naming no call, or a call that is gone,
            // opens the strike and leaves the call alone.
            Op::Open
                if call.is_none()
                    || call != self.phase.id()
                    || matches!(self.phase, Phase::Ended { .. }) =>
            {
                fx.push(Effect::OpenStrike);
                return Ok(());
            }
            _ => {}
        }
        let current = self.phase.id();
        if call.is_none() || call != current {
            return Err(Refusal::Stale);
        }
        let lease = device.leased().then_some(now_ms + limits.card_lease_ms);
        match (self.phase, op) {
            (Phase::Ringing { id, since }, Op::Answer { .. } | Op::Join | Op::Open) => {
                let mic = match op {
                    Op::Answer { mic } => mic,
                    Op::Join => true,
                    _ => false,
                };
                self.phase = Phase::Active {
                    id,
                    since,
                    answered_at: now_ms,
                };
                self.participants.clear();
                self.participants.insert(
                    device.clone(),
                    Participant {
                        mic,
                        lease_until: lease,
                    },
                );
                fx.push(Effect::StopRingback);
                if mic {
                    fx.push(Effect::Tone(Tone::Answered));
                }
                if op == Op::Open {
                    fx.push(Effect::OpenStrike);
                }
                fx.push(self.event(EventKind::Answered, id, None));
                self.changed(fx);
            }
            (Phase::Ringing { id, .. }, Op::Decline) => {
                self.end(id, Reason::Rejected, now_ms, fx);
                fx.insert(0, Effect::StopRingback);
                fx.insert(1, Effect::Tone(Tone::Declined));
            }
            (Phase::Active { .. }, Op::Open) => fx.push(Effect::OpenStrike),
            (Phase::Active { .. }, Op::Answer { .. } | Op::Join) => {
                match self.participants.get_mut(device) {
                    Some(p) => p.lease_until = lease,
                    None => {
                        let mic = !matches!(op, Op::Answer { mic: false });
                        self.participants.insert(
                            device.clone(),
                            Participant {
                                mic,
                                lease_until: lease,
                            },
                        );
                        self.changed(fx);
                    }
                }
            }
            (Phase::Active { .. }, Op::Leave) => self.leave(device, now_ms, fx),
            (Phase::Active { .. }, Op::Mic(on)) => {
                let p = self.participants.get_mut(device).ok_or(Refusal::NotNow)?;
                p.lease_until = lease;
                if p.mic != on {
                    p.mic = on;
                    self.changed(fx);
                }
            }
            (Phase::Active { .. }, Op::Keep) => {
                let p = self.participants.get_mut(device).ok_or(Refusal::NotNow)?;
                p.lease_until = lease;
            }
            _ => return Err(Refusal::NotNow),
        }
        Ok(())
    }

    fn leave(&mut self, device: &DeviceId, now_ms: u64, fx: &mut Vec<Effect>) {
        let Phase::Active { id, .. } = self.phase else {
            return;
        };
        if self.participants.remove(device).is_none() {
            return;
        }
        if self.participants.is_empty() {
            self.end(id, Reason::Hangup, now_ms, fx);
            fx.insert(0, Effect::Tone(Tone::Ended));
        } else {
            self.changed(fx);
        }
    }

    /// Timeouts, expired leases and the return to idle.
    pub fn tick(&mut self, now_ms: u64, limits: &Limits) -> Vec<Effect> {
        let mut fx = Vec::new();
        match self.phase {
            Phase::Ringing { id, since }
                if now_ms.saturating_sub(since) >= limits.ring_timeout_ms =>
            {
                self.missed = Some(Missed {
                    call_id: id,
                    at_ms: Some(now_ms),
                    wall_ms: None,
                });
                fx.push(Effect::StopRingback);
                fx.push(Effect::Tone(Tone::Missed));
                fx.push(Effect::MissedChanged);
                self.end(id, Reason::Missed, now_ms, &mut fx);
            }
            Phase::Active {
                id, answered_at, ..
            } => {
                if now_ms.saturating_sub(answered_at) >= limits.max_talk_ms {
                    fx.push(Effect::Tone(Tone::Ended));
                    self.end(id, Reason::MaxTalk, now_ms, &mut fx);
                } else {
                    let expired: Vec<DeviceId> = self
                        .participants
                        .iter()
                        .filter(|(_, p)| p.lease_until.is_some_and(|u| now_ms >= u))
                        .map(|(d, _)| d.clone())
                        .collect();
                    for d in &expired {
                        self.leave(d, now_ms, &mut fx);
                    }
                }
            }
            Phase::Ended { at, .. } if now_ms.saturating_sub(at) >= ENDED_MS => {
                self.phase = Phase::Idle;
                self.participants.clear();
                self.changed(&mut fx);
            }
            _ => {}
        }
        fx
    }

    fn end(&mut self, id: CallId, reason: Reason, at: u64, fx: &mut Vec<Effect>) {
        self.phase = Phase::Ended { id, reason, at };
        self.participants.clear();
        let kind = match reason {
            Reason::Rejected => EventKind::Declined,
            Reason::Missed => EventKind::Missed,
            Reason::Hangup | Reason::MaxTalk => EventKind::Ended,
        };
        fx.push(self.event(kind, id, Some(reason)));
        self.changed(fx);
    }

    fn event(&self, kind: EventKind, call_id: CallId, reason: Option<Reason>) -> Effect {
        Effect::Event(CallEvent {
            kind,
            call_id,
            reason,
        })
    }

    fn changed(&mut self, fx: &mut Vec<Effect>) {
        self.seq += 1;
        fx.push(Effect::Changed);
    }

    /// Something outside the call that its snapshot carries changed (the panel link, a setting):
    /// the next publication gets a new `seq`, so consumers that drop repeats keep it.
    pub fn bump(&mut self) {
        self.seq += 1;
    }

    pub fn snapshot(&self, now_ms: u64) -> Snapshot {
        Snapshot {
            seq: self.seq,
            phase: self.phase,
            participants: self
                .participants
                .iter()
                .map(|(d, p)| (d.as_str().to_owned(), p.mic))
                .collect(),
            missed: self.missed,
            now_ms,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: CallId = [0xa3, 0xf9, 0xc2, 0xe1];
    const B: CallId = [0x01, 0x02, 0x03, 0x04];
    const L: Limits = Limits {
        ring_timeout_ms: 45_000,
        max_talk_ms: 180_000,
        card_lease_ms: 30_000,
    };

    fn dev(s: &str) -> DeviceId {
        DeviceId::new(s).unwrap()
    }
    fn cmd(
        c: &mut Call,
        now: u64,
        d: &str,
        call: Option<CallId>,
        op: Op,
    ) -> Result<Vec<Effect>, Refusal> {
        let d = dev(d);
        c.handle(
            now,
            Input::Command {
                device: &d,
                call,
                op,
            },
            &L,
            B,
        )
    }
    fn ringing(now: u64) -> Call {
        let mut c = Call::new(B, None);
        c.handle(now, Input::Pressed, &L, A).unwrap();
        c
    }
    fn active(now: u64) -> Call {
        let mut c = ringing(0);
        cmd(&mut c, now, "panel", Some(A), Op::Answer { mic: true }).unwrap();
        c
    }
    fn events(fx: &[Effect]) -> Vec<EventKind> {
        fx.iter()
            .filter_map(|e| match e {
                Effect::Event(ev) => Some(ev.kind),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn ids_are_eight_hex_and_devices_are_bounded() {
        assert_eq!(hex(&A), "a3f9c2e1");
        assert_eq!(parse_id("a3f9c2e1"), Some(A));
        assert_eq!(parse_id("a3f9c2e"), None);
        assert_eq!(parse_id("a3f9c2eg"), None);
        assert!(DeviceId::new("dk-7f3a91_X").is_some());
        assert!(DeviceId::new("").is_none());
        assert!(DeviceId::new("a b").is_none());
        assert!(DeviceId::new(&"x".repeat(33)).is_none());
        assert!(DeviceId::new(&"x".repeat(32)).is_some());
    }

    #[test]
    fn press_in_idle_rings_with_ringback_first() {
        let mut c = Call::new(B, None);
        let fx = c.handle(10, Input::Pressed, &L, A).unwrap();
        assert_eq!(fx[0], Effect::StartRingback);
        assert_eq!(events(&fx), [EventKind::Ringing]);
        assert!(fx.contains(&Effect::Changed));
        assert_eq!(c.phase(), Phase::Ringing { id: A, since: 10 });
    }

    #[test]
    fn press_while_ringing_restarts_nothing() {
        let mut c = ringing(0);
        let seq = c.snapshot(0).seq;
        assert_eq!(c.handle(5_000, Input::Pressed, &L, B), Err(Refusal::NotNow));
        assert_eq!(c.phase(), Phase::Ringing { id: A, since: 0 });
        assert_eq!(c.snapshot(5_000).seq, seq);
        // The timeout still counts from the first press.
        assert!(events(&c.tick(45_000, &L)).contains(&EventKind::Missed));
    }

    #[test]
    fn press_while_active_is_ignored() {
        let mut c = active(1_000);
        assert_eq!(c.handle(2_000, Input::Pressed, &L, B), Err(Refusal::NotNow));
        assert!(matches!(c.phase(), Phase::Active { id: A, .. }));
    }

    #[test]
    fn press_while_ended_starts_a_new_call_at_once() {
        let mut c = ringing(0);
        cmd(&mut c, 1_000, "panel", Some(A), Op::Decline).unwrap();
        let fx = c.handle(1_500, Input::Pressed, &L, B).unwrap();
        assert_eq!(fx[0], Effect::StartRingback);
        assert_eq!(
            c.phase(),
            Phase::Ringing {
                id: B,
                since: 1_500
            }
        );
    }

    #[test]
    fn answer_while_ringing_stops_ringback_chimes_and_adds_the_device_with_mic() {
        let mut c = ringing(0);
        let fx = cmd(&mut c, 2_000, "dk-1", Some(A), Op::Answer { mic: true }).unwrap();
        assert_eq!(fx[0], Effect::StopRingback);
        assert!(fx.contains(&Effect::Tone(Tone::Answered)));
        assert_eq!(events(&fx), [EventKind::Answered]);
        assert_eq!(
            c.phase(),
            Phase::Active {
                id: A,
                since: 0,
                answered_at: 2_000
            }
        );
        assert_eq!(c.snapshot(2_000).participants.get("dk-1"), Some(&true));
    }

    #[test]
    fn answer_with_mic_off_has_no_chime() {
        let mut c = ringing(0);
        let fx = cmd(&mut c, 2_000, "panel", Some(A), Op::Answer { mic: false }).unwrap();
        assert!(!fx.contains(&Effect::Tone(Tone::Answered)));
        assert_eq!(c.snapshot(2_000).participants.get("panel"), Some(&false));
    }

    #[test]
    fn a_second_answer_is_a_join_and_a_repeat_join_is_a_no_op() {
        let mut c = active(1_000);
        let fx = cmd(&mut c, 1_100, "dk-1", Some(A), Op::Answer { mic: true }).unwrap();
        assert_eq!(
            fx,
            [Effect::Changed],
            "no ringback stop, no chime, no answered event"
        );
        let seq = c.snapshot(0).seq;
        assert_eq!(cmd(&mut c, 1_200, "dk-1", Some(A), Op::Join), Ok(vec![]));
        assert_eq!(
            cmd(&mut c, 1_200, "panel", Some(A), Op::Answer { mic: true }),
            Ok(vec![])
        );
        assert_eq!(c.snapshot(0).seq, seq);
        assert_eq!(c.snapshot(0).participants.len(), 2);
    }

    #[test]
    fn join_adds_with_mic_on_and_nobody_else_changes() {
        let mut c = active(1_000);
        cmd(&mut c, 1_000, "panel", Some(A), Op::Mic(false)).unwrap();
        cmd(&mut c, 2_000, "dk-2", Some(A), Op::Join).unwrap();
        let p = c.snapshot(0).participants;
        assert_eq!((p.get("panel"), p.get("dk-2")), (Some(&false), Some(&true)));
    }

    #[test]
    fn a_stale_call_id_is_refused_and_changes_nothing() {
        let mut c = ringing(0);
        let before = c.snapshot(0);
        for op in [
            Op::Answer { mic: true },
            Op::Join,
            Op::Decline,
            Op::Leave,
            Op::Mic(true),
            Op::Keep,
        ] {
            assert_eq!(
                cmd(&mut c, 100, "dk-1", Some(B), op),
                Err(Refusal::Stale),
                "{op:?}"
            );
            assert_eq!(
                cmd(&mut c, 100, "dk-1", None, op),
                Err(Refusal::Stale),
                "{op:?}"
            );
        }
        assert_eq!(c.snapshot(0), before);
    }

    #[test]
    fn open_while_ringing_answers_with_mic_off_then_opens() {
        let mut c = ringing(0);
        let fx = cmd(&mut c, 3_000, "dk-1", Some(A), Op::Open).unwrap();
        assert_eq!(fx[0], Effect::StopRingback);
        assert!(fx.contains(&Effect::OpenStrike));
        assert!(
            !fx.contains(&Effect::Tone(Tone::Answered)),
            "door_opened plays instead"
        );
        assert_eq!(events(&fx), [EventKind::Answered]);
        assert_eq!(c.snapshot(0).participants.get("dk-1"), Some(&false));
    }

    #[test]
    fn open_while_active_only_opens() {
        let mut c = active(1_000);
        let before = c.snapshot(0);
        assert_eq!(
            cmd(&mut c, 2_000, "dk-9", Some(A), Op::Open),
            Ok(vec![Effect::OpenStrike])
        );
        assert_eq!(c.snapshot(0), before);
    }

    #[test]
    fn open_in_idle_or_without_or_with_an_old_call_is_a_plain_open() {
        let mut c = Call::new(B, None);
        assert_eq!(
            cmd(&mut c, 0, "panel", None, Op::Open),
            Ok(vec![Effect::OpenStrike])
        );
        assert_eq!(
            cmd(&mut c, 0, "panel", Some(A), Op::Open),
            Ok(vec![Effect::OpenStrike])
        );
        let mut c = ringing(0);
        assert_eq!(
            cmd(&mut c, 0, "dk-1", Some(B), Op::Open),
            Ok(vec![Effect::OpenStrike])
        );
        assert_eq!(
            cmd(&mut c, 0, "dk-1", None, Op::Open),
            Ok(vec![Effect::OpenStrike])
        );
        assert!(matches!(c.phase(), Phase::Ringing { .. }));
    }

    #[test]
    fn open_during_ended_is_a_plain_open_even_with_its_call_id() {
        let mut c = ringing(0);
        cmd(&mut c, 1_000, "panel", Some(A), Op::Decline).unwrap();
        assert_eq!(
            cmd(&mut c, 2_000, "dk-1", Some(A), Op::Open),
            Ok(vec![Effect::OpenStrike])
        );
        assert!(matches!(c.phase(), Phase::Ended { .. }));
    }

    #[test]
    fn bump_raises_seq() {
        let mut c = Call::new(B, None);
        let s = c.snapshot(0).seq;
        c.bump();
        assert!(c.snapshot(0).seq > s);
    }

    #[test]
    fn decline_while_ringing_ends_rejected_with_its_tone() {
        let mut c = ringing(0);
        let fx = cmd(&mut c, 1_000, "panel", Some(A), Op::Decline).unwrap();
        assert_eq!(
            &fx[..2],
            [Effect::StopRingback, Effect::Tone(Tone::Declined)]
        );
        assert_eq!(events(&fx), [EventKind::Declined]);
        assert_eq!(
            c.phase(),
            Phase::Ended {
                id: A,
                reason: Reason::Rejected,
                at: 1_000
            }
        );
    }

    #[test]
    fn decline_outside_ringing_is_not_now() {
        let mut c = active(1_000);
        assert_eq!(
            cmd(&mut c, 2_000, "panel", Some(A), Op::Decline),
            Err(Refusal::NotNow)
        );
        assert!(matches!(c.phase(), Phase::Active { .. }));
        let mut c = Call::new(B, None);
        assert_eq!(
            cmd(&mut c, 0, "panel", None, Op::Decline),
            Err(Refusal::Stale)
        );
    }

    #[test]
    fn leave_not_last_keeps_the_call_leave_last_ends_it() {
        let mut c = active(1_000);
        cmd(&mut c, 1_100, "dk-1", Some(A), Op::Join).unwrap();
        let fx = cmd(&mut c, 2_000, "panel", Some(A), Op::Leave).unwrap();
        assert_eq!(fx, [Effect::Changed]);
        assert!(matches!(c.phase(), Phase::Active { .. }));
        let fx = cmd(&mut c, 3_000, "dk-1", Some(A), Op::Leave).unwrap();
        assert_eq!(fx[0], Effect::Tone(Tone::Ended));
        assert_eq!(events(&fx), [EventKind::Ended]);
        assert_eq!(
            c.phase(),
            Phase::Ended {
                id: A,
                reason: Reason::Hangup,
                at: 3_000
            }
        );
    }

    #[test]
    fn leave_by_a_non_participant_changes_nothing() {
        let mut c = active(1_000);
        assert_eq!(cmd(&mut c, 2_000, "dk-5", Some(A), Op::Leave), Ok(vec![]));
        assert!(matches!(c.phase(), Phase::Active { .. }));
    }

    #[test]
    fn mic_is_per_device_and_only_for_participants() {
        let mut c = active(1_000);
        cmd(&mut c, 1_000, "dk-1", Some(A), Op::Join).unwrap();
        assert_eq!(
            cmd(&mut c, 1_500, "dk-1", Some(A), Op::Mic(false)),
            Ok(vec![Effect::Changed])
        );
        assert_eq!(
            cmd(&mut c, 1_600, "dk-1", Some(A), Op::Mic(false)),
            Ok(vec![])
        );
        let p = c.snapshot(0).participants;
        assert_eq!((p.get("panel"), p.get("dk-1")), (Some(&true), Some(&false)));
        assert_eq!(
            cmd(&mut c, 1_700, "dk-7", Some(A), Op::Mic(true)),
            Err(Refusal::NotNow)
        );
    }

    #[test]
    fn a_card_lease_expires_without_keep_and_the_panel_has_none() {
        let mut c = ringing(0);
        cmd(&mut c, 1_000, "dk-1", Some(A), Op::Answer { mic: true }).unwrap();
        cmd(&mut c, 1_000, "panel", Some(A), Op::Join).unwrap();
        assert!(c.tick(30_999, &L).is_empty());
        cmd(&mut c, 20_000, "dk-1", Some(A), Op::Keep).unwrap();
        assert!(c.tick(49_999, &L).is_empty(), "keep moved the lease");
        assert_eq!(c.tick(50_000, &L), [Effect::Changed]);
        assert_eq!(c.snapshot(0).participants.len(), 1, "the panel stays");
        assert!(c.tick(170_000, &L).is_empty());
    }

    #[test]
    fn the_last_lease_lost_ends_the_call() {
        let mut c = active(1_000);
        let panel = DeviceId::panel();
        let fx = c.handle(5_000, Input::LeaseLost(&panel), &L, B).unwrap();
        assert_eq!(events(&fx), [EventKind::Ended]);
        assert!(matches!(
            c.phase(),
            Phase::Ended {
                reason: Reason::Hangup,
                ..
            }
        ));
    }

    #[test]
    fn max_talk_counts_from_the_answer() {
        let mut c = active(10_000);
        assert!(c.tick(189_999, &L).is_empty());
        let fx = c.tick(190_000, &L);
        assert_eq!(fx[0], Effect::Tone(Tone::Ended));
        assert!(matches!(
            c.phase(),
            Phase::Ended {
                reason: Reason::MaxTalk,
                ..
            }
        ));
    }

    #[test]
    fn ring_timeout_is_a_missed_call_with_a_mark() {
        let mut c = ringing(1_000);
        assert!(c.tick(45_999, &L).is_empty());
        let fx = c.tick(46_000, &L);
        assert_eq!(
            &fx[..3],
            [
                Effect::StopRingback,
                Effect::Tone(Tone::Missed),
                Effect::MissedChanged
            ]
        );
        assert_eq!(events(&fx), [EventKind::Missed]);
        assert_eq!(
            c.missed(),
            Some(Missed {
                call_id: A,
                at_ms: Some(46_000),
                wall_ms: None
            })
        );
        c.stamp_missed(1_794_146_607_412);
        assert_eq!(c.missed().unwrap().wall_ms, Some(1_794_146_607_412));
    }

    #[test]
    fn a_new_missed_call_replaces_the_mark_and_an_answered_one_keeps_it() {
        let mut c = ringing(0);
        c.tick(45_000, &L);
        c.tick(49_000, &L);
        c.handle(50_000, Input::Pressed, &L, B).unwrap();
        cmd(&mut c, 51_000, "panel", Some(B), Op::Answer { mic: true }).unwrap();
        assert_eq!(c.missed().unwrap().call_id, A);
        cmd(&mut c, 52_000, "panel", Some(B), Op::Leave).unwrap();
        c.tick(56_000, &L);
        c.handle(60_000, Input::Pressed, &L, [9; 4]).unwrap();
        c.tick(105_000, &L);
        assert_eq!(c.missed().unwrap().call_id, [9; 4]);
    }

    #[test]
    fn clear_missed_from_any_device_clears_it_once() {
        let mut c = Call::new(
            B,
            Some(Missed {
                call_id: A,
                at_ms: None,
                wall_ms: Some(1),
            }),
        );
        let fx = cmd(&mut c, 0, "dk-1", None, Op::ClearMissed).unwrap();
        assert!(fx.contains(&Effect::MissedChanged));
        assert_eq!(events(&fx), [EventKind::MissedCleared]);
        assert_eq!(c.missed(), None);
        assert_eq!(cmd(&mut c, 0, "panel", None, Op::ClearMissed), Ok(vec![]));
    }

    #[test]
    fn ended_returns_to_idle_after_four_seconds() {
        let mut c = ringing(0);
        cmd(&mut c, 1_000, "panel", Some(A), Op::Decline).unwrap();
        assert!(c.tick(4_999, &L).is_empty());
        assert_eq!(c.tick(5_000, &L), [Effect::Changed]);
        assert_eq!(c.phase(), Phase::Idle);
        assert_eq!(
            cmd(&mut c, 6_000, "panel", Some(A), Op::Join),
            Err(Refusal::Stale)
        );
    }

    #[test]
    fn commands_in_ended_are_not_now() {
        let mut c = ringing(0);
        cmd(&mut c, 1_000, "panel", Some(A), Op::Decline).unwrap();
        assert_eq!(
            cmd(&mut c, 1_100, "panel", Some(A), Op::Answer { mic: true }),
            Err(Refusal::NotNow)
        );
    }

    #[test]
    fn seq_rises_on_every_change_and_only_then() {
        let mut c = Call::new(B, None);
        let mut last = c.snapshot(0).seq;
        let mut step = |c: &mut Call, fx: Vec<Effect>| {
            let seq = c.snapshot(0).seq;
            if fx.contains(&Effect::Changed) {
                assert!(seq > last);
            } else {
                assert_eq!(seq, last);
            }
            last = seq;
        };
        let fx = c.handle(0, Input::Pressed, &L, A).unwrap();
        step(&mut c, fx);
        let fx = cmd(&mut c, 1, "panel", Some(A), Op::Answer { mic: true }).unwrap();
        step(&mut c, fx);
        let fx = cmd(&mut c, 2, "panel", Some(A), Op::Keep).unwrap();
        step(&mut c, fx);
        let fx = cmd(&mut c, 3, "panel", Some(A), Op::Mic(false)).unwrap();
        step(&mut c, fx);
        let fx = cmd(&mut c, 4, "panel", Some(A), Op::Leave).unwrap();
        step(&mut c, fx);
        let fx = c.tick(5_000, &L);
        step(&mut c, fx);
    }
}
