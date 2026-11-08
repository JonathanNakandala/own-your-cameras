//! The call contract on the wire, for both transports: the snapshot (`call/state` and the link's
//! `state` frame), the missed mark (`call/missed`), events (`call/event`), commands (`call/command`
//! and the link's `cmd` frame) and the other link frames. One reader for both; no state here.
//!
//! Commands are flat JSON objects; the reader accepts strings, numbers, booleans and null and
//! refuses anything nested, so a payload cannot make it allocate more than its own length.
use crate::call::{self, CallEvent, CallId, DeviceId, Missed, Op, Phase, Refusal, Snapshot};
use crate::home::iso_utc;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::time::{Duration, UNIX_EPOCH};

/// Longest frame read from a link client.
pub const MAX_FRAME: usize = 512;

/// What the publisher adds to a snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Context {
    /// 8 hex digits, fresh per boot: `seq` is comparable only within one boot.
    pub boot: String,
    /// Unix milliseconds now, when the clock is trusted.
    pub wall_now_ms: Option<u64>,
    pub panel_connected: bool,
    pub ring_timeout_s: u32,
    pub max_talk_s: u32,
    pub panel_ring_volume: u32,
}

/// A malformed message, or a device id the transport does not allow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bad {
    Malformed(&'static str),
    Device,
}

impl From<Bad> for Option<Refusal> {
    fn from(b: Bad) -> Self {
        matches!(b, Bad::Device).then_some(Refusal::BadDevice)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Str(String),
    Num(f64),
    Bool(bool),
    Null,
}

/// A flat JSON object, keys in order of appearance (a repeated key keeps its last value).
pub fn parse_object(text: &str) -> Result<BTreeMap<String, Value>, Bad> {
    let mut r = Reader {
        b: text.as_bytes(),
        i: 0,
    };
    let mut out = BTreeMap::new();
    r.ws();
    r.expect(b'{')?;
    r.ws();
    if r.peek() == Some(b'}') {
        r.i += 1;
    } else {
        loop {
            r.ws();
            let key = r.string()?;
            r.ws();
            r.expect(b':')?;
            r.ws();
            let v = r.value()?;
            out.insert(key, v);
            r.ws();
            match r.next() {
                Some(b',') => continue,
                Some(b'}') => break,
                _ => return Err(Bad::Malformed("expected , or }")),
            }
        }
    }
    r.ws();
    if r.i != r.b.len() {
        return Err(Bad::Malformed("trailing data"));
    }
    Ok(out)
}

struct Reader<'a> {
    b: &'a [u8],
    i: usize,
}

impl Reader<'_> {
    fn peek(&self) -> Option<u8> {
        self.b.get(self.i).copied()
    }
    fn next(&mut self) -> Option<u8> {
        let c = self.peek();
        self.i += 1;
        c
    }
    fn ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\r' | b'\n')) {
            self.i += 1;
        }
    }
    fn expect(&mut self, c: u8) -> Result<(), Bad> {
        if self.next() == Some(c) {
            Ok(())
        } else {
            Err(Bad::Malformed("unexpected character"))
        }
    }
    fn word(&mut self, w: &str) -> Result<(), Bad> {
        if self.b[self.i..].starts_with(w.as_bytes()) {
            self.i += w.len();
            Ok(())
        } else {
            Err(Bad::Malformed("bad literal"))
        }
    }
    fn value(&mut self) -> Result<Value, Bad> {
        match self.peek() {
            Some(b'"') => self.string().map(Value::Str),
            Some(b't') => self.word("true").map(|_| Value::Bool(true)),
            Some(b'f') => self.word("false").map(|_| Value::Bool(false)),
            Some(b'n') => self.word("null").map(|_| Value::Null),
            Some(b'-' | b'0'..=b'9') => self.number(),
            _ => Err(Bad::Malformed("unsupported value")),
        }
    }
    fn number(&mut self) -> Result<Value, Bad> {
        let start = self.i;
        while matches!(
            self.peek(),
            Some(b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9')
        ) {
            self.i += 1;
        }
        std::str::from_utf8(&self.b[start..self.i])
            .ok()
            .and_then(|s| s.parse::<f64>().ok())
            .filter(|n| n.is_finite())
            .map(Value::Num)
            .ok_or(Bad::Malformed("bad number"))
    }
    fn string(&mut self) -> Result<String, Bad> {
        self.expect(b'"')?;
        let mut out = String::new();
        loop {
            let start = self.i;
            while !matches!(self.peek(), Some(b'"' | b'\\') | None) {
                if self.b[self.i] < 0x20 {
                    return Err(Bad::Malformed("control character in string"));
                }
                self.i += 1;
            }
            out.push_str(
                std::str::from_utf8(&self.b[start..self.i])
                    .map_err(|_| Bad::Malformed("not UTF-8"))?,
            );
            match self.next() {
                Some(b'"') => return Ok(out),
                Some(b'\\') => {
                    let c = match self.next() {
                        Some(b'"') => '"',
                        Some(b'\\') => '\\',
                        Some(b'/') => '/',
                        Some(b'b') => '\u{8}',
                        Some(b'f') => '\u{c}',
                        Some(b'n') => '\n',
                        Some(b'r') => '\r',
                        Some(b't') => '\t',
                        Some(b'u') => {
                            let hex = self
                                .b
                                .get(self.i..self.i + 4)
                                .and_then(|h| std::str::from_utf8(h).ok())
                                .and_then(|h| u32::from_str_radix(h, 16).ok())
                                .ok_or(Bad::Malformed("bad escape"))?;
                            self.i += 4;
                            char::from_u32(hex).ok_or(Bad::Malformed("surrogate escape"))?
                        }
                        _ => return Err(Bad::Malformed("bad escape")),
                    };
                    out.push(c);
                }
                _ => return Err(Bad::Malformed("unterminated string")),
            }
        }
    }
}

fn str_field<'a>(o: &'a BTreeMap<String, Value>, key: &str) -> Result<Option<&'a str>, Bad> {
    match o.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Str(s)) => Ok(Some(s)),
        Some(_) => Err(Bad::Malformed("expected a string")),
    }
}

fn bool_field(o: &BTreeMap<String, Value>, key: &str) -> Result<Option<bool>, Bad> {
    match o.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        Some(_) => Err(Bad::Malformed("expected a boolean")),
    }
}

/// `op`, `call_id`, `mic` of a command object (MQTT `call/command` or the link's `cmd`).
fn op_of(o: &BTreeMap<String, Value>) -> Result<(Option<CallId>, Op), Bad> {
    let call = match str_field(o, "call_id")? {
        None => None,
        Some(s) => Some(call::parse_id(s).ok_or(Bad::Malformed("bad call_id"))?),
    };
    let mic = bool_field(o, "mic")?;
    let op = match str_field(o, "op")?.ok_or(Bad::Malformed("no op"))? {
        "answer" => Op::Answer {
            mic: mic.unwrap_or(true),
        },
        "join" => Op::Join,
        "decline" => Op::Decline,
        "leave" => Op::Leave,
        "mic" => Op::Mic(mic.ok_or(Bad::Malformed("mic needs a boolean"))?),
        "keep" => Op::Keep,
        "open" => Op::Open,
        "clear_missed" => Op::ClearMissed,
        _ => return Err(Bad::Malformed("unknown op")),
    };
    Ok((call, op))
}

/// A `call/command` payload. The panel's id is the link's alone.
pub fn parse_mqtt_command(text: &str) -> Result<(DeviceId, Option<CallId>, Op), Bad> {
    let o = parse_object(text)?;
    let device = str_field(&o, "device")?.ok_or(Bad::Device)?;
    if device == call::PANEL {
        return Err(Bad::Device);
    }
    let device = DeviceId::new(device).ok_or(Bad::Device)?;
    let (call, op) = op_of(&o)?;
    Ok((device, call, op))
}

/// What a link client sends.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClientFrame {
    Auth {
        user: String,
        response: String,
    },
    Cmd {
        req: u32,
        call: Option<CallId>,
        op: Op,
    },
    Ping,
}

pub fn parse_client_frame(line: &str) -> Result<ClientFrame, Bad> {
    if line.len() > MAX_FRAME {
        return Err(Bad::Malformed("frame too long"));
    }
    let o = parse_object(line)?;
    match str_field(&o, "t")?.ok_or(Bad::Malformed("no t"))? {
        "auth" => Ok(ClientFrame::Auth {
            user: str_field(&o, "user")?.unwrap_or_default().to_owned(),
            response: str_field(&o, "response")?.unwrap_or_default().to_owned(),
        }),
        "cmd" => {
            let req = match o.get("req") {
                Some(Value::Num(n))
                    if *n >= 0.0 && *n <= f64::from(u32::MAX) && n.fract() == 0.0 =>
                {
                    *n as u32
                }
                _ => return Err(Bad::Malformed("bad req")),
            };
            let (call, op) = op_of(&o)?;
            Ok(ClientFrame::Cmd { req, call, op })
        }
        "ping" => Ok(ClientFrame::Ping),
        _ => Err(Bad::Malformed("unknown frame")),
    }
}

/// `md5(user:nonce:password)` in lowercase hex: the link's challenge response.
pub fn auth_response(user: &str, nonce: &str, password: &str) -> String {
    camera_transport::rtsp::md5::md5_hex(format!("{user}:{nonce}:{password}").as_bytes())
}

/// Constant-time comparison of two responses.
pub fn same_response(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

pub fn hello(nonce: &str) -> String {
    format!(r#"{{"t":"hello","nonce":"{nonce}"}}"#)
}

pub fn ack(req: u32, result: Result<(), Refusal>) -> String {
    match result {
        Ok(()) => format!(r#"{{"t":"ack","req":{req},"ok":true}}"#),
        Err(r) => format!(
            r#"{{"t":"ack","req":{req},"ok":false,"refusal":"{}"}}"#,
            r.name()
        ),
    }
}

pub fn pong() -> String {
    r#"{"t":"pong"}"#.to_owned()
}

/// The link's `state` frame: the snapshot object with `"t":"state"` first.
pub fn state_frame(snapshot_json: &str) -> String {
    format!(r#"{{"t":"state",{}"#, &snapshot_json[1..])
}

fn secs(ms: u64) -> String {
    let tenths = ms / 100;
    format!("{}.{}", tenths / 10, tenths % 10)
}

/// A monotonic instant as wall time, from one wall reading `wall_now_ms` taken at `now_ms`.
fn wall(ctx: &Context, now_ms: u64, t_ms: u64) -> String {
    match ctx.wall_now_ms {
        Some(w) => format!(
            r#""{}""#,
            iso_utc(
                UNIX_EPOCH + Duration::from_millis(w.saturating_sub(now_ms.saturating_sub(t_ms)))
            )
        ),
        None => "null".to_owned(),
    }
}

fn missed_body(m: &Missed, now_ms: u64) -> String {
    format!(
        r#""call_id":"{}","at":{},"age_s":{}"#,
        call::hex(&m.call_id),
        m.wall_ms.map_or("null".to_owned(), |w| format!(
            r#""{}""#,
            iso_utc(UNIX_EPOCH + Duration::from_millis(w))
        )),
        m.at_ms
            .map_or("null".to_owned(), |a| secs(now_ms.saturating_sub(a))),
    )
}

/// `call/missed`: the mark, or `{}` when there is none.
pub fn missed_json(m: Option<&Missed>, now_ms: u64) -> String {
    match m {
        Some(m) => format!("{{{}}}", missed_body(m, now_ms)),
        None => "{}".to_owned(),
    }
}

/// `call/state` and the link's `state` frame.
pub fn state_json(s: &Snapshot, ctx: &Context) -> String {
    let now = s.now_ms;
    let (id, since, answered, ring_age, talk_age, reason) = match s.phase {
        Phase::Idle => (None, None, None, None, None, None),
        Phase::Ringing { id, since } => (
            Some(id),
            Some(since),
            None,
            Some(now.saturating_sub(since)),
            None,
            None,
        ),
        Phase::Active {
            id,
            since,
            answered_at,
        } => (
            Some(id),
            Some(since),
            Some(answered_at),
            None,
            Some(now.saturating_sub(answered_at)),
            None,
        ),
        Phase::Ended { id, reason, .. } => (Some(id), None, None, None, None, Some(reason)),
    };
    let opt_str = |v: Option<String>| v.map_or("null".to_owned(), |s| format!(r#""{s}""#));
    let mut parts = String::new();
    for (i, (d, mic)) in s.participants.iter().enumerate() {
        if i > 0 {
            parts.push(',');
        }
        let _ = write!(parts, r#""{d}":{{"mic":{mic}}}"#);
    }
    format!(
        concat!(
            r#"{{"seq":{},"boot":"{}","phase":"{}","call_id":{},"since":{},"answered_at":{},"#,
            r#""ring_age_s":{},"talk_age_s":{},"reason":{},"participants":{{{}}},"missed":{},"#,
            r#""panel":"{}","ring_timeout_s":{},"max_talk_s":{},"panel_ring_volume":{}}}"#
        ),
        s.seq,
        ctx.boot,
        s.phase.name(),
        opt_str(id.map(|i| call::hex(&i))),
        since.map_or("null".to_owned(), |t| wall(ctx, now, t)),
        answered.map_or("null".to_owned(), |t| wall(ctx, now, t)),
        ring_age.map_or("null".to_owned(), secs),
        talk_age.map_or("null".to_owned(), secs),
        opt_str(reason.map(|r| r.name().to_owned())),
        parts,
        s.missed.as_ref().map_or("null".to_owned(), |m| format!(
            "{{{}}}",
            missed_body(m, now)
        )),
        if ctx.panel_connected {
            "connected"
        } else {
            "absent"
        },
        ctx.ring_timeout_s,
        ctx.max_talk_s,
        ctx.panel_ring_volume,
    )
}

/// `call/event`: `event_type` for Home Assistant's event entity, `event` for everyone else.
pub fn event_json(e: &CallEvent) -> String {
    let kind = e.kind.name();
    format!(
        r#"{{"event_type":"{kind}","event":"{kind}","call_id":"{}","reason":{}}}"#,
        call::hex(&e.call_id),
        e.reason
            .map_or("null".to_owned(), |r| format!(r#""{}""#, r.name())),
    )
}

/// `missed.json` next to `settings.json`: `{"call_id":"…","at":<unix ms>|null}`.
pub fn missed_file(m: Option<&Missed>) -> String {
    match m {
        Some(m) => format!(
            r#"{{"call_id":"{}","at":{}}}"#,
            call::hex(&m.call_id),
            m.wall_ms.map_or("null".to_owned(), |w| w.to_string())
        ),
        None => "{}".to_owned(),
    }
}

/// The mark kept across a reboot; nothing for an empty, missing or damaged file.
pub fn parse_missed_file(text: &str) -> Option<Missed> {
    let o = parse_object(text).ok()?;
    let call_id = call::parse_id(str_field(&o, "call_id").ok()??)?;
    let wall_ms = match o.get("at") {
        Some(Value::Num(n)) if *n >= 0.0 => Some(*n as u64),
        _ => None,
    };
    Some(Missed {
        call_id,
        at_ms: None,
        wall_ms,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::call::{Call, EventKind, Input, Limits, Reason};
    use serde_json::Value as J;

    const A: CallId = [0xa3, 0xf9, 0xc2, 0xe1];
    /// 2026-11-08T17:02:19.880Z
    const WALL: u64 = 1_794_157_339_880;

    fn ctx(wall: Option<u64>) -> Context {
        Context {
            boot: "0badf00d".into(),
            wall_now_ms: wall,
            panel_connected: true,
            ring_timeout_s: 45,
            max_talk_s: 180,
            panel_ring_volume: 80,
        }
    }

    fn golden(name: &str) -> J {
        let path = format!(
            "{}/tests/fixtures/call/{name}.json",
            env!("CARGO_MANIFEST_DIR")
        );
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap()
    }

    fn check(name: &str, json: &str) {
        let got: J = serde_json::from_str(json).unwrap_or_else(|e| panic!("{name}: {e}: {json}"));
        if std::env::var_os("CALL_GOLDEN_WRITE").is_some() {
            let path = format!(
                "{}/tests/fixtures/call/{name}.json",
                env!("CARGO_MANIFEST_DIR")
            );
            std::fs::write(&path, serde_json::to_string_pretty(&got).unwrap() + "\n").unwrap();
        }
        assert_eq!(got, golden(name), "{name}");
    }

    fn active_call() -> Call {
        let l = Limits::default();
        let mut c = Call::new([0; 4], None);
        c.handle(1_000, Input::Pressed, &l, A).unwrap();
        let panel = DeviceId::panel();
        let dk = DeviceId::new("dk-7f3a91").unwrap();
        for (d, op) in [
            (&panel, Op::Answer { mic: true }),
            (&dk, Op::Join),
            (&dk, Op::Mic(false)),
        ] {
            c.handle(
                9_676,
                Input::Command {
                    device: d,
                    call: Some(A),
                    op,
                },
                &l,
                [0; 4],
            )
            .unwrap();
        }
        c
    }

    #[test]
    fn the_active_snapshot_matches_its_golden_file_with_and_without_a_clock() {
        let c = active_call();
        let s = c.snapshot(9_676 + 12_400);
        check("state-active", &state_json(&s, &ctx(Some(WALL + 12_400))));
        check("state-active-noclock", &state_json(&s, &ctx(None)));
    }

    #[test]
    fn ringing_idle_and_ended_snapshots_match_their_golden_files() {
        let l = Limits::default();
        let mut c = Call::new([0; 4], None);
        check(
            "state-idle",
            &state_json(
                &c.snapshot(0),
                &Context {
                    panel_connected: false,
                    ..ctx(None)
                },
            ),
        );
        c.handle(1_000, Input::Pressed, &l, A).unwrap();
        check(
            "state-ringing",
            &state_json(&c.snapshot(4_500), &ctx(Some(WALL))),
        );
        c.tick(46_000, &l);
        let s = c.snapshot(46_000);
        assert!(matches!(
            s.phase,
            Phase::Ended {
                reason: Reason::Missed,
                ..
            }
        ));
        let mut c2 = Call::new([0; 4], None);
        c2.handle(1_000, Input::Pressed, &l, A).unwrap();
        c2.tick(46_000, &l);
        c2.stamp_missed(WALL);
        check(
            "state-ended-missed",
            &state_json(&c2.snapshot(47_000), &ctx(Some(WALL + 1_000))),
        );
        check("missed", &missed_json(c2.missed().as_ref(), 47_000));
        assert_eq!(missed_json(None, 0), "{}");
    }

    #[test]
    fn link_frames_match_their_golden_files() {
        check("frame-hello", &hello("5f2a9c01d3e4b768"));
        check("frame-ack-ok", &ack(7, Ok(())));
        check("frame-ack-stale", &ack(8, Err(Refusal::Stale)));
        check("frame-pong", &pong());
        let s = active_call().snapshot(22_076);
        let frame: J = serde_json::from_str(&state_frame(&state_json(&s, &ctx(None)))).unwrap();
        assert_eq!(frame["t"], "state");
        assert_eq!(frame["phase"], "active");
    }

    #[test]
    fn events_carry_event_type_for_home_assistant() {
        let e = CallEvent {
            kind: EventKind::Ended,
            call_id: A,
            reason: Some(Reason::MaxTalk),
        };
        check("event-ended", &event_json(&e));
        let v: J = serde_json::from_str(&event_json(&CallEvent {
            kind: EventKind::Ringing,
            call_id: A,
            reason: None,
        }))
        .unwrap();
        assert_eq!(
            (v["event_type"].as_str(), v["reason"].is_null()),
            (Some("ringing"), true)
        );
    }

    #[test]
    fn mqtt_commands_parse_every_op() {
        let p = |s: &str| parse_mqtt_command(s);
        let dk = DeviceId::new("dk-7f3a91").unwrap();
        assert_eq!(
            p(r#"{"op":"answer","call_id":"a3f9c2e1","device":"dk-7f3a91","mic":true}"#),
            Ok((dk.clone(), Some(A), Op::Answer { mic: true }))
        );
        assert_eq!(
            p(r#"{"op":"answer","call_id":"a3f9c2e1","device":"dk-7f3a91"}"#)
                .unwrap()
                .2,
            Op::Answer { mic: true }
        );
        assert_eq!(
            p(r#" { "op" : "mic" , "call_id":"a3f9c2e1","device":"dk-7f3a91","mic":false } "#)
                .unwrap()
                .2,
            Op::Mic(false)
        );
        for (op, want) in [
            ("join", Op::Join),
            ("decline", Op::Decline),
            ("leave", Op::Leave),
            ("keep", Op::Keep),
            ("open", Op::Open),
            ("clear_missed", Op::ClearMissed),
        ] {
            let text = format!(r#"{{"op":"{op}","device":"dk-7f3a91","call_id":null}}"#);
            assert_eq!(p(&text), Ok((dk.clone(), None, want)), "{op}");
        }
    }

    #[test]
    fn bad_mqtt_commands_are_refused() {
        let p = |s: &str| parse_mqtt_command(s);
        assert_eq!(p(r#"{"op":"open","device":"panel"}"#), Err(Bad::Device));
        assert_eq!(p(r#"{"op":"open"}"#), Err(Bad::Device));
        assert_eq!(p(r#"{"op":"open","device":"a b"}"#), Err(Bad::Device));
        assert!(matches!(
            p(r#"{"op":"fly","device":"dk-1"}"#),
            Err(Bad::Malformed(_))
        ));
        assert!(matches!(
            p(r#"{"op":"mic","device":"dk-1"}"#),
            Err(Bad::Malformed(_))
        ));
        assert!(matches!(
            p(r#"{"op":"join","device":"dk-1","call_id":"zz"}"#),
            Err(Bad::Malformed(_))
        ));
        assert!(matches!(
            p(r#"{"op":"join","device":"dk-1","x":{"a":1}}"#),
            Err(Bad::Malformed(_))
        ));
        assert!(matches!(
            p(r#"{"op":"join","device":"dk-1"} x"#),
            Err(Bad::Malformed(_))
        ));
        assert!(matches!(p("open"), Err(Bad::Malformed(_))));
        assert!(matches!(
            p(r#"{"op":"join","device":"dk-1""#),
            Err(Bad::Malformed(_))
        ));
    }

    #[test]
    fn client_frames_parse_and_bound_their_size() {
        assert_eq!(
            parse_client_frame(r#"{"t":"auth","user":"u\"x","response":"ab"}"#),
            Ok(ClientFrame::Auth {
                user: "u\"x".into(),
                response: "ab".into()
            })
        );
        assert_eq!(
            parse_client_frame(
                r#"{"t":"cmd","req":3,"call_id":"a3f9c2e1","op":"answer","mic":false}"#
            ),
            Ok(ClientFrame::Cmd {
                req: 3,
                call: Some(A),
                op: Op::Answer { mic: false }
            })
        );
        assert_eq!(parse_client_frame(r#"{"t":"ping"}"#), Ok(ClientFrame::Ping));
        assert!(parse_client_frame(r#"{"t":"cmd","req":-1,"op":"join"}"#).is_err());
        assert!(parse_client_frame(r#"{"t":"cmd","req":1.5,"op":"join"}"#).is_err());
        assert!(parse_client_frame(r#"{"t":"nope"}"#).is_err());
        let long = format!(r#"{{"t":"ping","pad":"{}"}}"#, "x".repeat(MAX_FRAME));
        assert!(parse_client_frame(&long).is_err());
        assert_eq!(
            parse_object(r#"{"a":"é\n"}"#).unwrap()["a"],
            Value::Str("é\n".into())
        );
        assert!(parse_object(r#"{"a":"\ud800"}"#).is_err());
    }

    #[test]
    fn the_auth_response_is_md5_of_user_nonce_password() {
        // md5("u:n:p")
        assert_eq!(
            auth_response("u", "n", "p"),
            "12603beb66aaf1045e0b6f7dff4d55e5"
        );
        assert!(same_response("abc", "abc"));
        assert!(!same_response("abc", "abd"));
        assert!(!same_response("abc", "ab"));
    }

    #[test]
    fn the_missed_file_round_trips_and_ignores_damage() {
        let m = Missed {
            call_id: A,
            at_ms: Some(5),
            wall_ms: Some(WALL),
        };
        let back = parse_missed_file(&missed_file(Some(&m))).unwrap();
        assert_eq!(
            (back.call_id, back.at_ms, back.wall_ms),
            (A, None, Some(WALL))
        );
        let no_clock = Missed { wall_ms: None, ..m };
        assert_eq!(
            parse_missed_file(&missed_file(Some(&no_clock)))
                .unwrap()
                .wall_ms,
            None
        );
        assert_eq!(parse_missed_file(&missed_file(None)), None);
        assert_eq!(parse_missed_file(""), None);
        assert_eq!(parse_missed_file("{\"call_id\":\"zz\"}"), None);
    }
}
