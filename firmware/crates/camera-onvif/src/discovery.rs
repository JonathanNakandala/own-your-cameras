//! WS-Discovery (2005/04) as ONVIF uses it: answer a matching `Probe` with a `ProbeMatch`, announce
//! `Hello` on start and `Bye` on a clean stop. Pure: message ids and addresses are passed in.

use crate::DeviceIdentity;
use crate::xml::{self, escape};
use std::net::Ipv4Addr;

pub const MULTICAST: Ipv4Addr = Ipv4Addr::new(239, 255, 255, 250);
pub const PORT: u16 = 3702;
const WSD: &str = "http://schemas.xmlsoap.org/ws/2005/04/discovery";
const ANONYMOUS: &str = "http://schemas.xmlsoap.org/ws/2004/08/addressing/role/anonymous";
const MULTICAST_TO: &str = "urn:schemas-xmlsoap-org:ws:2005:04:discovery";
const TYPES: &str = "dn:NetworkVideoTransmitter tds:Device";
/// Local names of the types this device matches.
const TYPE_NAMES: [&str; 2] = ["NetworkVideoTransmitter", "Device"];

fn percent(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

/// RFC 3986 matching (the WS-Discovery default): the probe's scope equals ours or is a prefix of it that
/// ends at a path segment.
fn scope_matches(wanted: &str, ours: &str) -> bool {
    let wanted = wanted.trim_end_matches('/');
    ours == wanted
        || ours
            .strip_prefix(wanted)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// The scopes the device announces.
pub fn scopes(identity: &DeviceIdentity, ptz: bool) -> Vec<String> {
    let mut out = vec![
        "onvif://www.onvif.org/Profile/Streaming".to_string(),
        "onvif://www.onvif.org/type/video_encoder".to_string(),
        "onvif://www.onvif.org/type/audio_encoder".to_string(),
    ];
    if ptz {
        out.push("onvif://www.onvif.org/type/ptz".into());
    }
    out.push(format!(
        "onvif://www.onvif.org/hardware/{}",
        percent(&identity.model)
    ));
    out.push(format!(
        "onvif://www.onvif.org/name/{}",
        percent(&identity.name)
    ));
    out
}

fn envelope(action: &str, message_id: &str, to: &str, extra: &str, body: &str) -> Vec<u8> {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<env:Envelope xmlns:env=\"http://www.w3.org/2003/05/soap-envelope\" xmlns:wsa=\"http://schemas.xmlsoap.org/ws/2004/08/addressing\" xmlns:d=\"{WSD}\" xmlns:dn=\"http://www.onvif.org/ver10/network/wsdl\" xmlns:tds=\"http://www.onvif.org/ver10/device/wsdl\"><env:Header><wsa:MessageID>urn:uuid:{message_id}</wsa:MessageID>{extra}<wsa:To>{to}</wsa:To><wsa:Action>{WSD}/{action}</wsa:Action></env:Header><env:Body>{body}</env:Body></env:Envelope>"
    )
    .into_bytes()
}

fn endpoint(identity: &DeviceIdentity) -> String {
    format!(
        "<wsa:EndpointReference><wsa:Address>urn:uuid:{}</wsa:Address></wsa:EndpointReference>",
        escape(&identity.uuid)
    )
}

fn description(identity: &DeviceIdentity, scopes: &[String], xaddr: &str) -> String {
    format!(
        "{}<d:Types>{TYPES}</d:Types><d:Scopes>{}</d:Scopes><d:XAddrs>{}</d:XAddrs><d:MetadataVersion>1</d:MetadataVersion>",
        endpoint(identity),
        escape(&scopes.join(" ")),
        escape(xaddr)
    )
}

/// The reply to `probe` when it asks for this device, else `None` (also for anything that is no Probe).
pub fn probe_match(
    probe: &[u8],
    identity: &DeviceIdentity,
    scopes: &[String],
    xaddr: &str,
    message_id: &str,
) -> Option<Vec<u8>> {
    let root = xml::parse(probe).ok()?;
    let body = root.child("Body")?.child("Probe")?;
    if body.ns != WSD {
        return None;
    }
    let types_ok = body.text_of("Types").is_none_or(|types| {
        types.split_ascii_whitespace().all(|t| {
            let local = t.rsplit(':').next().unwrap_or(t);
            TYPE_NAMES.contains(&local)
        })
    });
    let scopes_ok = body.text_of("Scopes").is_none_or(|wanted| {
        wanted
            .split_ascii_whitespace()
            .all(|w| scopes.iter().any(|s| scope_matches(w, s)))
    });
    if !types_ok || !scopes_ok {
        return None;
    }
    let relates = root
        .child("Header")
        .and_then(|h| h.text_of("MessageID"))
        .unwrap_or_default();
    Some(envelope(
        "ProbeMatches",
        message_id,
        ANONYMOUS,
        &format!("<wsa:RelatesTo>{}</wsa:RelatesTo>", escape(relates)),
        &format!(
            "<d:ProbeMatches><d:ProbeMatch>{}</d:ProbeMatch></d:ProbeMatches>",
            description(identity, scopes, xaddr)
        ),
    ))
}

pub fn hello(
    identity: &DeviceIdentity,
    scopes: &[String],
    xaddr: &str,
    message_id: &str,
) -> Vec<u8> {
    envelope(
        "Hello",
        message_id,
        MULTICAST_TO,
        "",
        &format!(
            "<d:Hello>{}</d:Hello>",
            description(identity, scopes, xaddr)
        ),
    )
}

pub fn bye(identity: &DeviceIdentity, message_id: &str) -> Vec<u8> {
    envelope(
        "Bye",
        message_id,
        MULTICAST_TO,
        "",
        &format!("<d:Bye>{}</d:Bye>", endpoint(identity)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> DeviceIdentity {
        DeviceIdentity {
            model: "E1 Zoom".into(),
            name: "Test camera".into(),
            uuid: "00000000-0000-5000-8000-000000000001".into(),
            ..DeviceIdentity::default()
        }
    }

    fn probe(types: &str, scopes: &str) -> Vec<u8> {
        format!(
            r#"<?xml version="1.0" encoding="utf-8"?><Envelope xmlns="http://www.w3.org/2003/05/soap-envelope" xmlns:a="http://schemas.xmlsoap.org/ws/2004/08/addressing"><Header><a:MessageID>uuid:probe-1</a:MessageID><a:To>urn:schemas-xmlsoap-org:ws:2005:04:discovery</a:To><a:Action>http://schemas.xmlsoap.org/ws/2005/04/discovery/Probe</a:Action></Header><Body><Probe xmlns="http://schemas.xmlsoap.org/ws/2005/04/discovery" xmlns:dn="http://www.onvif.org/ver10/network/wsdl">{types}{scopes}</Probe></Body></Envelope>"#
        )
        .into_bytes()
    }

    #[test]
    fn a_video_transmitter_probe_gets_a_match_with_xaddr_scopes_and_relation() {
        let scopes = scopes(&identity(), true);
        assert!(scopes.contains(&"onvif://www.onvif.org/name/Test%20camera".to_string()));
        assert!(scopes.contains(&"onvif://www.onvif.org/type/ptz".to_string()));
        let reply = probe_match(
            &probe("<Types>dn:NetworkVideoTransmitter</Types>", ""),
            &identity(),
            &scopes,
            "http://CAMERA_HOST/onvif/device_service",
            "m-1",
        )
        .unwrap();
        let root = xml::parse(&reply).unwrap();
        let header = root.child("Header").unwrap();
        assert_eq!(header.text_of("RelatesTo"), Some("uuid:probe-1"));
        assert_eq!(
            header.text_of("Action"),
            Some("http://schemas.xmlsoap.org/ws/2005/04/discovery/ProbeMatches")
        );
        let m = root.descendant("ProbeMatch").unwrap();
        assert_eq!(
            m.text_of("XAddrs"),
            Some("http://CAMERA_HOST/onvif/device_service")
        );
        assert_eq!(m.text_of("Types"), Some(TYPES));
        assert_eq!(
            m.descendant("Address").unwrap().text,
            "urn:uuid:00000000-0000-5000-8000-000000000001"
        );
    }

    #[test]
    fn untyped_probes_match_and_foreign_types_or_scopes_do_not() {
        let scopes = scopes(&identity(), false);
        let xaddr = "http://CAMERA_HOST/onvif/device_service";
        let id = identity();
        assert!(probe_match(&probe("", ""), &id, &scopes, xaddr, "m").is_some());
        assert!(
            probe_match(
                &probe("<Types>tds:Device</Types>", ""),
                &id,
                &scopes,
                xaddr,
                "m"
            )
            .is_some()
        );
        assert!(
            probe_match(
                &probe("<Types>p:Printer</Types>", ""),
                &id,
                &scopes,
                xaddr,
                "m"
            )
            .is_none()
        );
        let streaming = "<Scopes>onvif://www.onvif.org/Profile/Streaming</Scopes>";
        assert!(probe_match(&probe("", streaming), &id, &scopes, xaddr, "m").is_some());
        let partial = "<Scopes>onvif://www.onvif.org/name/Te</Scopes>";
        assert!(probe_match(&probe("", partial), &id, &scopes, xaddr, "m").is_none());
        let segment = "<Scopes>onvif://www.onvif.org/name</Scopes>";
        assert!(probe_match(&probe("", segment), &id, &scopes, xaddr, "m").is_some());
        let ptz = "<Scopes>onvif://www.onvif.org/type/ptz</Scopes>";
        assert!(probe_match(&probe("", ptz), &id, &scopes, xaddr, "m").is_none());
        assert!(probe_match(b"<x/>", &id, &scopes, xaddr, "m").is_none());
    }

    #[test]
    fn hello_and_bye_are_multicast_announcements() {
        let id = identity();
        let hello = xml::parse(&hello(
            &id,
            &scopes(&id, false),
            "http://CAMERA_HOST/x",
            "m",
        ))
        .unwrap();
        assert_eq!(
            hello.child("Header").unwrap().text_of("To"),
            Some(MULTICAST_TO)
        );
        assert!(hello.descendant("Hello").unwrap().child("XAddrs").is_some());
        let bye = xml::parse(&bye(&id, "m")).unwrap();
        assert!(
            bye.descendant("Bye")
                .unwrap()
                .descendant("Address")
                .is_some()
        );
    }
}
