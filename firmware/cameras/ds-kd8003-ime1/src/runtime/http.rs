//! Read-only diagnostics: `GET /state` (JSON) and `GET /log?after=N`. No secrets, no commands.
use super::Device;
use super::ffi::json_str;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::time::Duration;

fn state_json(g: &Device) -> String {
    let s = g.st.lock().unwrap();
    let st = |i: usize| match s.streams[i] {
        Some(v) => format!(
            r#"{{"w":{},"h":{},"fps":{},"kbps":{},"gop":{},"vbr":{},"profile":{},"frames":{}}}"#,
            v[0],
            v[1],
            v[2],
            v[3],
            v[4],
            v[5] == 2,
            v[6],
            s.stream_frames[i]
        ),
        None => r#"{"off":true}"#.to_string(),
    };
    format!(
        concat!(
            r#"{{"uptime_s":{},"phase":{},"media_rc":{},"ao_rc":{},"wdt_feeds":{},"viewers":{},"#,
            r#""streams":[{},{}],"audio":{{"frames":{},"err":{},"mic_peak":{},"speaker_busy":{},"speaker_plays":{},"amp":{}}},"#,
            r#""inputs":{{"mask":{},"tamper":{},"reed_open":{},"button_presses":{}}},"#,
            r#""relay":{{"mask":{},"remaining_ms":{},"rc":{},"pulses":{}}},"#,
            r#""keypad":{{"phase":{},"polls":{},"restarts":{},"granted":{},"denied":{}}},"#,
            r#""mqtt":{{"online":{},"drops":{}}},"night":{},"ir_duty":{},"healthy":{},"last_log":{}}}"#
        ),
        g.t0.elapsed().as_secs(),
        json_str(&s.phase),
        s.media_rc,
        s.ao_rc,
        s.wdt_feeds,
        s.viewers,
        st(0),
        st(1),
        s.audio_frames,
        s.audio_err,
        s.mic_peak,
        s.speaker_busy,
        s.speaker_plays,
        s.amp,
        s.inputs,
        s.tamper,
        s.inputs & 1 != 0,
        s.button_presses,
        s.relay_mask,
        s.strike
            .pulse(g.t0.elapsed().as_millis() as u64)
            .map_or(0, |p| p.remaining_ms),
        s.relay_rc,
        s.relay_pulses,
        json_str(s.keypad_phase),
        s.keypad_polls,
        s.keypad_restarts,
        s.granted,
        s.denied,
        s.mqtt_online,
        s.mqtt_drops,
        s.night,
        s.ir_duty,
        s.healthy,
        s.next_log as i64 - 1
    )
}

fn log_json(g: &Device, after: i64) -> String {
    let s = g.st.lock().unwrap();
    let rows: Vec<String> = s
        .log
        .iter()
        .filter(|(n, _, _)| *n as i64 > after)
        .map(|(n, t, x)| format!("[{n},{t},{}]", json_str(x)))
        .collect();
    format!("[{}]", rows.join(","))
}

fn reply(c: &mut TcpStream, code: u16, body: &str) {
    let _ = write!(
        c,
        "HTTP/1.0 {code} X\r\nContent-Type: application/json\r\nCache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

pub fn handle(g: Arc<Device>, mut c: TcpStream) {
    let _ = c.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = c.set_write_timeout(Some(Duration::from_secs(5)));
    // One client at a time: a slow one gets 5 s for its whole request, not 5 s per byte.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut buf = vec![0u8; 4096];
    let mut n = 0;
    loop {
        if std::time::Instant::now() > deadline {
            return;
        }
        match c.read(&mut buf[n..]) {
            Ok(0) | Err(_) => return,
            Ok(k) => {
                n += k;
                if buf[..n].windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
                if n == buf.len() {
                    return;
                }
            }
        }
    }
    let head = String::from_utf8_lossy(&buf[..n]).to_string();
    let target = head.split_whitespace().nth(1).unwrap_or("/").to_string();
    let (path, query) = target.split_once('?').unwrap_or((&target, ""));
    match path {
        "/state" => reply(&mut c, 200, &state_json(&g)),
        "/log" => {
            let after = query
                .strip_prefix("after=")
                .and_then(|v| v.parse().ok())
                .unwrap_or(-1);
            reply(&mut c, 200, &log_json(&g, after));
        }
        _ => reply(&mut c, 404, "{}"),
    }
}
