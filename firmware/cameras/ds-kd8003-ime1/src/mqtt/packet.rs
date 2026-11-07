//! MQTT 3.1.1 control packets: the few the application needs, encoded and decoded by hand.

#[derive(Debug, PartialEq, Eq)]
pub enum Packet {
    ConnAck {
        session_present: bool,
        code: u8,
    },
    Publish {
        topic: String,
        payload: Vec<u8>,
        retain: bool,
    },
    SubAck {
        id: u16,
        codes: Vec<u8>,
    },
    PingResp,
    /// A valid packet this client does not act on.
    Other(u8),
}

#[derive(Debug, PartialEq, Eq)]
pub enum Decode {
    /// Need more bytes.
    Incomplete,
    Packet(Packet, usize),
    Malformed,
}

pub struct Will<'a> {
    pub topic: &'a str,
    pub payload: &'a [u8],
    pub retain: bool,
}

fn string(out: &mut Vec<u8>, s: &str) {
    out.extend_from_slice(&(s.len() as u16).to_be_bytes());
    out.extend_from_slice(s.as_bytes());
}

fn remaining_length(out: &mut Vec<u8>, mut n: usize) {
    loop {
        let mut byte = (n % 128) as u8;
        n /= 128;
        if n > 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if n == 0 {
            break;
        }
    }
}

fn frame(kind: u8, body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(body.len() + 5);
    out.push(kind);
    remaining_length(&mut out, body.len());
    out.extend_from_slice(body);
    out
}

pub fn connect(
    client_id: &str,
    keep_alive_s: u16,
    user: Option<(&str, &str)>,
    will: Option<Will<'_>>,
) -> Vec<u8> {
    let mut body = Vec::new();
    string(&mut body, "MQTT");
    body.push(4);
    let mut flags = 0x02; // clean session
    if let Some(w) = &will {
        flags |= 0x04 | if w.retain { 0x20 } else { 0 };
    }
    if user.is_some() {
        flags |= 0xc0;
    }
    body.push(flags);
    body.extend_from_slice(&keep_alive_s.to_be_bytes());
    string(&mut body, client_id);
    if let Some(w) = will {
        string(&mut body, w.topic);
        body.extend_from_slice(&(w.payload.len() as u16).to_be_bytes());
        body.extend_from_slice(w.payload);
    }
    if let Some((u, p)) = user {
        string(&mut body, u);
        string(&mut body, p);
    }
    frame(0x10, &body)
}

pub fn publish(topic: &str, payload: &[u8], retain: bool) -> Vec<u8> {
    let mut body = Vec::with_capacity(topic.len() + payload.len() + 2);
    string(&mut body, topic);
    body.extend_from_slice(payload);
    frame(0x30 | u8::from(retain), &body)
}

pub fn subscribe(id: u16, topics: &[&str]) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(&id.to_be_bytes());
    for t in topics {
        string(&mut body, t);
        body.push(0);
    }
    frame(0x82, &body)
}

pub fn pingreq() -> Vec<u8> {
    vec![0xc0, 0]
}

pub fn disconnect() -> Vec<u8> {
    vec![0xe0, 0]
}

/// Decodes the first packet in `buf`.
pub fn decode(buf: &[u8]) -> Decode {
    let Some(&first) = buf.first() else {
        return Decode::Incomplete;
    };
    let mut len = 0usize;
    let mut mult = 1usize;
    let mut i = 1;
    loop {
        let Some(&b) = buf.get(i) else {
            return Decode::Incomplete;
        };
        len += (b & 0x7f) as usize * mult;
        i += 1;
        if b & 0x80 == 0 {
            break;
        }
        if i > 4 {
            return Decode::Malformed;
        }
        mult *= 128;
    }
    let total = i + len;
    if buf.len() < total {
        return Decode::Incomplete;
    }
    let body = &buf[i..total];
    let packet = match first >> 4 {
        2 => {
            if body.len() != 2 {
                return Decode::Malformed;
            }
            Packet::ConnAck {
                session_present: body[0] & 1 == 1,
                code: body[1],
            }
        }
        3 => {
            let qos = (first >> 1) & 3;
            if body.len() < 2 {
                return Decode::Malformed;
            }
            let tlen = u16::from_be_bytes([body[0], body[1]]) as usize;
            let after = 2 + tlen + if qos > 0 { 2 } else { 0 };
            if qos == 3 || body.len() < after {
                return Decode::Malformed;
            }
            let Ok(topic) = std::str::from_utf8(&body[2..2 + tlen]) else {
                return Decode::Malformed;
            };
            Packet::Publish {
                topic: topic.to_string(),
                payload: body[after..].to_vec(),
                retain: first & 1 == 1,
            }
        }
        9 => {
            if body.len() < 3 {
                return Decode::Malformed;
            }
            Packet::SubAck {
                id: u16::from_be_bytes([body[0], body[1]]),
                codes: body[2..].to_vec(),
            }
        }
        13 => Packet::PingResp,
        kind => Packet::Other(kind),
    };
    Decode::Packet(packet, total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connect_with_will_and_credentials_matches_the_specification_layout() {
        let p = connect(
            "gate",
            30,
            Some(("u", "pw")),
            Some(Will {
                topic: "a/avail",
                payload: b"offline",
                retain: true,
            }),
        );
        assert_eq!(p[0], 0x10);
        assert_eq!(p[1] as usize, p.len() - 2);
        assert_eq!(
            &p[2..10],
            &[0, 4, b'M', b'Q', b'T', b'T', 4, 0xc0 | 0x20 | 0x04 | 0x02]
        );
        assert_eq!(&p[10..12], &[0, 30]);
        assert_eq!(&p[12..18], &[0, 4, b'g', b'a', b't', b'e']);
        assert_eq!(
            &p[18..27],
            &[0, 7, b'a', b'/', b'a', b'v', b'a', b'i', b'l']
        );
        assert_eq!(
            &p[27..36],
            &[0, 7, b'o', b'f', b'f', b'l', b'i', b'n', b'e']
        );
        assert_eq!(&p[36..], &[0, 1, b'u', 0, 2, b'p', b'w']);
    }

    #[test]
    fn publish_and_subscribe_round_trip_through_decode() {
        let p = publish("t/x", b"hello", true);
        assert_eq!(p[0], 0x31);
        assert_eq!(
            decode(&p),
            Decode::Packet(
                Packet::Publish {
                    topic: "t/x".into(),
                    payload: b"hello".to_vec(),
                    retain: true
                },
                p.len()
            )
        );
        let s = subscribe(7, &["a/b", "c"]);
        assert_eq!(&s[..2], &[0x82, 12]);
        assert_eq!(&s[2..], &[0, 7, 0, 3, b'a', b'/', b'b', 0, 0, 1, b'c', 0]);
        assert_eq!(
            decode(&[0x20, 2, 0, 0]),
            Decode::Packet(
                Packet::ConnAck {
                    session_present: false,
                    code: 0
                },
                4
            )
        );
        assert_eq!(
            decode(&[0x90, 3, 0, 7, 0]),
            Decode::Packet(
                Packet::SubAck {
                    id: 7,
                    codes: vec![0]
                },
                5
            )
        );
        assert_eq!(decode(&[0xd0, 0]), Decode::Packet(Packet::PingResp, 2));
    }

    #[test]
    fn decode_handles_partial_long_and_broken_input() {
        assert_eq!(decode(&[]), Decode::Incomplete);
        assert_eq!(decode(&[0x30]), Decode::Incomplete);
        assert_eq!(decode(&[0x30, 5, 0, 1]), Decode::Incomplete);
        let big = publish("t", &vec![7u8; 300], false);
        assert_eq!(&big[1..3], &[0xaf, 0x02]);
        assert!(
            matches!(decode(&big), Decode::Packet(Packet::Publish { payload, .. }, n) if payload.len() == 300 && n == big.len())
        );
        assert_eq!(decode(&[0x30, 1, 0]), Decode::Malformed);
        assert_eq!(
            decode(&[0x36, 4, 0, 1, b'x', 0]),
            Decode::Malformed,
            "QoS 3 is reserved"
        );
        assert_eq!(decode(&[0x20, 1, 0]), Decode::Malformed);
        assert_eq!(
            decode(&[0x30, 0xff, 0xff, 0xff, 0xff, 0xff]),
            Decode::Malformed
        );
        assert_eq!(
            decode(&[0x40, 2, 0, 1]),
            Decode::Packet(Packet::Other(4), 4)
        );
    }
}
