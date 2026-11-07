//! RTSP message parsing and building, digest authentication (RFC 2617, MD5, as the vendor used) and SDP.
//! Pure functions: no sockets here.

use super::md5::md5_hex;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub method: String,
    pub uri: String,
    pub headers: Vec<(String, String)>,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    pub fn cseq(&self) -> &str {
        self.header("CSeq").unwrap_or("0")
    }

    /// The path of `uri` without scheme, host and query: `rtsp://h:554/a/b?x` → `a/b`.
    pub fn path(&self) -> &str {
        let u = self.uri.as_str();
        let rest = u.split_once("://").map_or(u, |(_, r)| r);
        let path = rest.split_once('/').map_or("", |(_, p)| p);
        path.split(['?', '#'])
            .next()
            .unwrap_or("")
            .trim_end_matches('/')
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Parsed {
    /// Need more bytes.
    Incomplete,
    /// A request and the number of bytes it used (head plus any body, which is ignored).
    Request(Request, usize),
    /// Not RTSP; close the connection.
    Bad,
}

/// Parses one request from the front of `buf`.
pub fn parse_request(buf: &[u8]) -> Parsed {
    // Control requests have no large payloads. Bound each message, not the whole buffer,
    // which may contain several pipelined requests or interleaved RTP after this request.
    const MAX_REQUEST: usize = 16 * 1024;
    let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") else {
        return if buf.len() >= MAX_REQUEST {
            Parsed::Bad
        } else {
            Parsed::Incomplete
        };
    };
    if end + 4 > MAX_REQUEST {
        return Parsed::Bad;
    }
    let Ok(head) = std::str::from_utf8(&buf[..end]) else {
        return Parsed::Bad;
    };
    let mut lines = head.split("\r\n");
    let mut first = lines.next().unwrap_or("").split(' ');
    let (Some(method), Some(uri), Some(ver)) = (first.next(), first.next(), first.next()) else {
        return Parsed::Bad;
    };
    if !ver.starts_with("RTSP/") || method.is_empty() {
        return Parsed::Bad;
    }
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect();
    let req = Request {
        method: method.to_string(),
        uri: uri.to_string(),
        headers,
    };
    let mut lengths = req
        .headers
        .iter()
        .filter(|(k, _)| k.eq_ignore_ascii_case("Content-Length"));
    let body = match lengths.next() {
        None => 0,
        Some((_, v)) => match v.parse::<usize>() {
            Ok(n) if !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()) => n,
            _ => return Parsed::Bad,
        },
    };
    if lengths.next().is_some() {
        return Parsed::Bad;
    }
    let Some(total) = (end + 4).checked_add(body).filter(|n| *n <= MAX_REQUEST) else {
        return Parsed::Bad;
    };
    if buf.len() < total {
        return Parsed::Incomplete;
    }
    Parsed::Request(req, total)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub code: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Response {
    pub fn new(code: u16) -> Self {
        Self {
            code,
            headers: Vec::new(),
            body: String::new(),
        }
    }

    pub fn with(mut self, k: &str, v: impl Into<String>) -> Self {
        self.headers.push((k.to_string(), v.into()));
        self
    }

    pub fn body(mut self, content_type: &str, body: String) -> Self {
        self.headers
            .push(("Content-Type".into(), content_type.into()));
        self.body = body;
        self
    }

    pub fn encode(&self, cseq: &str) -> Vec<u8> {
        let mut s = format!(
            "RTSP/1.0 {} {}\r\nCSeq: {cseq}\r\n",
            self.code,
            reason(self.code)
        );
        for (k, v) in &self.headers {
            s.push_str(&format!("{k}: {v}\r\n"));
        }
        if !self.body.is_empty() {
            s.push_str(&format!("Content-Length: {}\r\n", self.body.len()));
        }
        s.push_str("\r\n");
        s.push_str(&self.body);
        s.into_bytes()
    }
}

fn reason(code: u16) -> &'static str {
    match code {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        453 => "Not Enough Bandwidth",
        454 => "Session Not Found",
        455 => "Method Not Valid in This State",
        461 => "Unsupported Transport",
        _ => "Error",
    }
}

/// Parsed `Transport:` request header of SETUP.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Transport {
    Tcp { rtp_ch: u8, rtcp_ch: u8 },
    Udp { rtp_port: u16, rtcp_port: u16 },
}

pub fn parse_transport(h: &str) -> Option<Transport> {
    // go2rtc/ffmpeg offer several comma-separated specs; take the first one this server can serve.
    for spec in h.split(',') {
        let parts: Vec<&str> = spec.split(';').map(str::trim).collect();
        let proto = parts.first()?.to_ascii_uppercase();
        let param = |name: &str| {
            parts
                .iter()
                .find_map(|p| p.strip_prefix(name).and_then(|r| r.strip_prefix('=')))
        };
        let pair = |v: &str| -> Option<(u16, u16)> {
            let (a, b) = v.split_once('-').unwrap_or((v, ""));
            let a: u16 = a.parse().ok()?;
            Some((a, b.parse().unwrap_or(a.saturating_add(1))))
        };
        if parts.iter().any(|p| p.eq_ignore_ascii_case("multicast")) {
            continue;
        }
        if proto.ends_with("/TCP") {
            let (a, b) = param("interleaved").and_then(pair).unwrap_or((0, 1));
            return Some(Transport::Tcp {
                rtp_ch: u8::try_from(a).ok()?,
                rtcp_ch: u8::try_from(b).ok()?,
            });
        }
        if proto == "RTP/AVP" || proto == "RTP/AVP/UDP" {
            let (a, b) = param("client_port").and_then(pair)?;
            return Some(Transport::Udp {
                rtp_port: a,
                rtcp_port: b,
            });
        }
    }
    None
}

/// Digest credentials the server accepts.
#[derive(Debug, Clone)]
pub struct Credentials {
    pub user: String,
    pub password: String,
}

impl Credentials {
    /// First line of an auth file: `user:password`.
    pub fn parse(text: &str) -> Option<Self> {
        let line = text.lines().next()?.trim_end_matches('\r');
        let (user, password) = line.split_once(':')?;
        (!user.is_empty() && !password.is_empty()).then(|| Self {
            user: user.to_string(),
            password: password.to_string(),
        })
    }
}

pub const REALM: &str = "cam";

/// The `WWW-Authenticate` value for a 401.
pub fn challenge(nonce: &str) -> String {
    format!("Digest realm=\"{REALM}\", nonce=\"{nonce}\"")
}

fn digest_param<'a>(h: &'a str, name: &str) -> Option<&'a str> {
    let h = h.strip_prefix("Digest")?.trim_start();
    for part in split_params(h) {
        let (k, v) = part.split_once('=')?;
        if k.trim().eq_ignore_ascii_case(name) {
            return Some(v.trim().trim_matches('"'));
        }
    }
    None
}

/// Splits on commas outside quotes.
fn split_params(s: &str) -> Vec<&str> {
    let (mut out, mut start, mut quoted) = (Vec::new(), 0, false);
    for (i, c) in s.char_indices() {
        match c {
            '"' => quoted = !quoted,
            ',' if !quoted => {
                out.push(s[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(s[start..].trim());
    out
}

/// The `uri` a client signs must be the request URI or the aggregate (stream) URL it is a track
/// of: LIVE555 clients (Synology Surveillance Device, VLC) sign every `SETUP` of
/// `.../Channels/101/trackID=0` with the base `.../Channels/101/` from the SDP.
fn uri_covers(signed: &str, request_uri: &str) -> bool {
    if signed == request_uri {
        return true;
    }
    !signed.is_empty()
        && signed.ends_with('/')
        && request_uri.starts_with(signed)
        && !request_uri[signed.len()..].contains('/')
}

/// Verifies an `Authorization: Digest …` header for `method` and the request-line `request_uri` against
/// `creds` and the server `nonce`. The signed `uri` must cover the request's (see `uri_covers`); only `qop=auth` is accepted,
/// and a `qop` without `nc` and `cnonce` is refused rather than downgraded to the RFC 2069 form.
pub fn check_digest(
    header: &str,
    method: &str,
    request_uri: &str,
    creds: &Credentials,
    nonce: &str,
) -> bool {
    let get = |n| digest_param(header, n);
    let (Some(user), Some(realm), Some(n), Some(uri), Some(resp)) = (
        get("username"),
        get("realm"),
        get("nonce"),
        get("uri"),
        get("response"),
    ) else {
        return false;
    };
    if user != creds.user || realm != REALM || n != nonce || !uri_covers(uri, request_uri) {
        return false;
    }
    let ha1 = md5_hex(format!("{user}:{realm}:{}", creds.password).as_bytes());
    let ha2 = md5_hex(format!("{method}:{uri}").as_bytes());
    let expect = match (get("qop"), get("nc"), get("cnonce")) {
        (Some(qop), Some(nc), Some(cn)) if qop.eq_ignore_ascii_case("auth") => {
            md5_hex(format!("{ha1}:{nonce}:{nc}:{cn}:auth:{ha2}").as_bytes())
        }
        (None, None, None) => md5_hex(format!("{ha1}:{nonce}:{ha2}").as_bytes()),
        _ => return false,
    };
    constant_time_eq(expect.as_bytes(), resp.to_ascii_lowercase().as_bytes())
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0
}

/// Everything the SDP of one H.264 video track needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoSdp {
    pub sps: Vec<u8>,
    pub pps: Vec<u8>,
}

fn b64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::new();
    for c in data.chunks(3) {
        let n = (u32::from(c[0]) << 16)
            | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
            | u32::from(*c.get(2).unwrap_or(&0));
        for i in 0..4 {
            if i <= c.len() {
                s.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                s.push('=');
            }
        }
    }
    s
}

/// SDP for DESCRIBE. `base` is the request URI (control URL base).
pub fn sdp(v: &VideoSdp, base: &str) -> String {
    let profile = if v.sps.len() >= 4 {
        format!("{:02x}{:02x}{:02x}", v.sps[1], v.sps[2], v.sps[3])
    } else {
        "4d001f".into()
    };
    format!(
        "v=0\r\no=- 0 0 IN IP4 0.0.0.0\r\ns=cam\r\nc=IN IP4 0.0.0.0\r\nt=0 0\r\na=control:{base}\r\n\
         m=video 0 RTP/AVP 96\r\na=rtpmap:96 H264/90000\r\n\
         a=fmtp:96 packetization-mode=1;profile-level-id={profile};sprop-parameter-sets={},{}\r\n\
         a=control:trackID=0\r\na=recvonly\r\n",
        b64(&v.sps),
        b64(&v.pps)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_request_and_its_length() {
        let raw = b"DESCRIBE rtsp://cam:554/Preview_01_sub RTSP/1.0\r\nCSeq: 2\r\nAccept: application/sdp\r\n\r\nPLAY";
        let Parsed::Request(r, n) = parse_request(raw) else {
            panic!("not parsed")
        };
        assert_eq!(r.method, "DESCRIBE");
        assert_eq!(r.cseq(), "2");
        assert_eq!(r.path(), "Preview_01_sub");
        assert_eq!(&raw[n..], b"PLAY");
    }

    #[test]
    fn waits_for_head_and_body_and_rejects_garbage() {
        assert_eq!(
            parse_request(b"OPTIONS * RTSP/1.0\r\nCSeq: 1\r\n"),
            Parsed::Incomplete
        );
        let part = b"SET_PARAMETER rtsp://h/x RTSP/1.0\r\nCSeq: 3\r\nContent-Length: 4\r\n\r\nab";
        assert_eq!(parse_request(part), Parsed::Incomplete);
        assert_eq!(parse_request(b"GET / HTTP/1.1\r\n\r\n"), Parsed::Bad);
        assert_eq!(parse_request(&vec![b'x'; 20_000]), Parsed::Bad);
    }

    #[test]
    fn rejects_unbounded_or_ambiguous_requests_before_buffering_the_body() {
        for length in [
            "16777216",
            "4294967295",
            "18446744073709551615",
            "-1",
            "garbage",
        ] {
            let request =
                format!("SET_PARAMETER rtsp://h/x RTSP/1.0\r\nContent-Length: {length}\r\n\r\n");
            assert_eq!(parse_request(request.as_bytes()), Parsed::Bad, "{length}");
        }
        assert_eq!(
            parse_request(b"SET_PARAMETER rtsp://h/x RTSP/1.0\r\nContent-Length: 0\r\nContent-Length: 8\r\n\r\n"),
            Parsed::Bad
        );
        let oversized_head = format!("OPTIONS * RTSP/1.0\r\nX: {}\r\n\r\n", "x".repeat(16384));
        assert_eq!(parse_request(oversized_head.as_bytes()), Parsed::Bad);
    }

    #[test]
    fn path_strips_scheme_host_query_and_track() {
        let mk = |u: &str| Request {
            method: "SETUP".into(),
            uri: u.into(),
            headers: vec![],
        };
        assert_eq!(
            mk("rtsp://u:p@h:554/Preview_01_main?x=1").path(),
            "Preview_01_main"
        );
        assert_eq!(
            mk("rtsp://h/Preview_01_main/trackID=0").path(),
            "Preview_01_main/trackID=0"
        );
        assert_eq!(mk("*").path(), "");
    }

    #[test]
    fn transports() {
        assert_eq!(
            parse_transport("RTP/AVP/TCP;unicast;interleaved=2-3"),
            Some(Transport::Tcp {
                rtp_ch: 2,
                rtcp_ch: 3
            })
        );
        assert_eq!(
            parse_transport("RTP/AVP;unicast;client_port=5000-5001"),
            Some(Transport::Udp {
                rtp_port: 5000,
                rtcp_port: 5001
            })
        );
        assert_eq!(
            parse_transport("RTP/AVP;multicast, RTP/AVP/TCP;unicast;interleaved=0-1"),
            Some(Transport::Tcp {
                rtp_ch: 0,
                rtcp_ch: 1
            })
        );
        assert_eq!(parse_transport("RTP/AVP;unicast"), None);
    }

    #[test]
    fn digest_accepts_the_right_response_only() {
        let creds = Credentials {
            user: "admin".into(),
            password: "secret".into(),
        };
        const U: &str = "rtsp://h/p";
        let nonce = "abc123";
        let ha1 = md5_hex(b"admin:cam:secret");
        let ha2 = md5_hex(b"DESCRIBE:rtsp://h/p");
        let resp = md5_hex(format!("{ha1}:{nonce}:{ha2}").as_bytes());
        let ok = format!(
            "Digest username=\"admin\", realm=\"cam\", nonce=\"{nonce}\", uri=\"rtsp://h/p\", response=\"{resp}\""
        );
        assert!(check_digest(&ok, "DESCRIBE", U, &creds, nonce));
        assert!(!check_digest(&ok, "SETUP", U, &creds, nonce));
        assert!(!check_digest(&ok, "DESCRIBE", U, &creds, "other"));
        assert!(!check_digest(
            &ok.replace("admin\"", "root\""),
            "DESCRIBE",
            U,
            &creds,
            nonce
        ));
        assert!(!check_digest(
            "Basic YWRtaW46c2VjcmV0",
            "DESCRIBE",
            U,
            &creds,
            nonce
        ));
        // qop=auth form, as ffmpeg and go2rtc send it
        let qop = md5_hex(format!("{ha1}:{nonce}:00000001:xyz:auth:{ha2}").as_bytes());
        let with_qop = format!(
            "Digest username=\"admin\", realm=\"cam\", nonce=\"{nonce}\", uri=\"rtsp://h/p\", qop=auth, nc=00000001, cnonce=\"xyz\", response=\"{qop}\""
        );
        assert!(check_digest(&with_qop, "DESCRIBE", U, &creds, nonce));
        // the signed uri must be the request's
        assert!(!check_digest(
            &ok,
            "DESCRIBE",
            "rtsp://h/other",
            &creds,
            nonce
        ));
        // an unsupported qop, or qop without nc/cnonce, is refused instead of downgraded
        let int = with_qop.replace("qop=auth,", "qop=auth-int,");
        assert!(!check_digest(&int, "DESCRIBE", U, &creds, nonce));
        let no_nc = format!(
            "Digest username=\"admin\", realm=\"cam\", nonce=\"{nonce}\", uri=\"{U}\", qop=auth, response=\"{resp}\""
        );
        assert!(!check_digest(&no_nc, "DESCRIBE", U, &creds, nonce));
    }

    #[test]
    fn base64_and_sdp() {
        assert_eq!(b64(b"Man"), "TWFu");
        assert_eq!(b64(b"Ma"), "TWE=");
        assert_eq!(b64(b"M"), "TQ==");
        let s = sdp(
            &VideoSdp {
                sps: vec![0x67, 0x4d, 0x00, 0x1f, 1],
                pps: vec![0x68, 0xee],
            },
            "rtsp://h/x",
        );
        assert!(s.contains("profile-level-id=4d001f"));
        assert!(s.contains("sprop-parameter-sets=Z00AHwE=,aO4="));
        assert!(s.contains("m=video 0 RTP/AVP 96"));
    }

    #[test]
    fn credentials_file_is_user_colon_password() {
        let c = Credentials::parse("admin:p:w\nignored\n").unwrap();
        assert_eq!((c.user.as_str(), c.password.as_str()), ("admin", "p:w"));
        assert!(Credentials::parse("nopassword").is_none());
        assert!(Credentials::parse(":x").is_none());
        assert!(Credentials::parse("").is_none());
    }

    #[test]
    fn response_encoding() {
        let r = Response::new(200)
            .with("Session", "1")
            .body("application/sdp", "v=0\r\n".into());
        let s = String::from_utf8(r.encode("7")).unwrap();
        assert!(s.starts_with("RTSP/1.0 200 OK\r\nCSeq: 7\r\n"));
        assert!(s.contains("Content-Length: 5\r\n\r\nv=0\r\n"));
    }

    #[test]
    fn digest_accepts_live555_aggregate_uri() {
        let base = "rtsp://cam:554/Streaming/Channels/101/";
        assert!(uri_covers(
            base,
            "rtsp://cam:554/Streaming/Channels/101/trackID=0"
        ));
        assert!(uri_covers(base, base));
        assert!(!uri_covers(
            base,
            "rtsp://cam:554/Streaming/Channels/101/x/trackID=0"
        ));
        assert!(!uri_covers(
            "rtsp://cam:554/Streaming/Channels/101",
            "rtsp://cam:554/Streaming/Channels/1011"
        ));
        assert!(!uri_covers("", "rtsp://cam:554/a"));
    }
}
