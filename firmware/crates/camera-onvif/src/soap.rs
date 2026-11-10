//! SOAP 1.2 envelopes: a request becomes its action (the first Body element), its WS-Security token
//! and its WS-Addressing header; responses and faults are rendered with every ONVIF namespace bound
//! at the root. Pure.

use crate::auth::UsernameToken;
use crate::xml::{self, Element};

pub const ENV: &str = "http://www.w3.org/2003/05/soap-envelope";
pub const TDS: &str = "http://www.onvif.org/ver10/device/wsdl";
pub const TRT: &str = "http://www.onvif.org/ver10/media/wsdl";
pub const TPTZ: &str = "http://www.onvif.org/ver20/ptz/wsdl";
pub const TEV: &str = "http://www.onvif.org/ver10/events/wsdl";
pub const TMD: &str = "http://www.onvif.org/ver10/deviceIO/wsdl";
pub const TIMG: &str = "http://www.onvif.org/ver20/imaging/wsdl";
pub const WSNT: &str = "http://docs.oasis-open.org/wsn/b-2";
const WSSE: &str =
    "http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-wssecurity-secext-1.0.xsd";
const DIGEST_TYPE: &str = "#PasswordDigest";

/// Every prefix a response may use.
const NAMESPACES: &str = concat!(
    r#"xmlns:env="http://www.w3.org/2003/05/soap-envelope" "#,
    r#"xmlns:xs="http://www.w3.org/2001/XMLSchema" "#,
    r#"xmlns:tt="http://www.onvif.org/ver10/schema" "#,
    r#"xmlns:tds="http://www.onvif.org/ver10/device/wsdl" "#,
    r#"xmlns:trt="http://www.onvif.org/ver10/media/wsdl" "#,
    r#"xmlns:tptz="http://www.onvif.org/ver20/ptz/wsdl" "#,
    r#"xmlns:tev="http://www.onvif.org/ver10/events/wsdl" "#,
    r#"xmlns:tmd="http://www.onvif.org/ver10/deviceIO/wsdl" "#,
    r#"xmlns:timg="http://www.onvif.org/ver20/imaging/wsdl" "#,
    r#"xmlns:ter="http://www.onvif.org/ver10/error" "#,
    r#"xmlns:tns1="http://www.onvif.org/ver10/topics" "#,
    r#"xmlns:wsnt="http://docs.oasis-open.org/wsn/b-2" "#,
    r#"xmlns:wstop="http://docs.oasis-open.org/wsn/t-1" "#,
    r#"xmlns:wsa="http://www.w3.org/2005/08/addressing" "#,
    r#"xmlns:d="http://schemas.xmlsoap.org/ws/2005/04/discovery" "#,
    r#"xmlns:dn="http://www.onvif.org/ver10/network/wsdl""#,
);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    /// Local name of the first Body element (`GetProfiles`).
    pub action: String,
    /// Its namespace URI, which tells services apart (`GetServiceCapabilities` exists in each).
    pub ns: String,
    pub body: Element,
    pub token: Option<UsernameToken>,
    /// The WS-Addressing `To`, when sent.
    pub to: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseError {
    Xml(xml::XmlError),
    NotSoap,
}

pub fn parse(bytes: &[u8]) -> Result<Request, ParseError> {
    let root = xml::parse(bytes).map_err(ParseError::Xml)?;
    if root.name != "Envelope" {
        return Err(ParseError::NotSoap);
    }
    let body = root
        .child("Body")
        .and_then(|b| b.children.first())
        .ok_or(ParseError::NotSoap)?
        .clone();
    let header = root.child("Header");
    let token = header
        .and_then(|h| h.child("Security"))
        .filter(|s| s.ns == WSSE || s.ns.is_empty() || s.ns.contains("wss"))
        .and_then(|s| s.child("UsernameToken"))
        .map(|t| {
            let password = t.child("Password");
            // A Password without Type is PasswordText (WSS UsernameToken Profile 1.0).
            let digest = password
                .filter(|p| p.attr("Type").is_some_and(|ty| ty.ends_with(DIGEST_TYPE)))
                .and_then(|p| crate::codec::base64_decode(&p.text));
            UsernameToken {
                username: t.text_of("Username").unwrap_or_default().to_string(),
                digest,
                nonce: t
                    .text_of("Nonce")
                    .and_then(crate::codec::base64_decode)
                    .unwrap_or_default(),
                created: t.text_of("Created").unwrap_or_default().to_string(),
            }
        });
    let to = header.and_then(|h| h.child("To")).map(|t| t.text.clone());
    Ok(Request {
        action: body.name.clone(),
        ns: body.ns.clone(),
        body,
        token,
        to,
    })
}

/// A complete response envelope around `body` (and an optional SOAP header).
pub fn envelope(header: &str, body: &str) -> String {
    let header = if header.is_empty() {
        String::new()
    } else {
        format!("<env:Header>{header}</env:Header>")
    };
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<env:Envelope {NAMESPACES}>{header}<env:Body>{body}</env:Body></env:Envelope>"
    )
}

/// An ONVIF fault: `Sender` for a bad request (HTTP 400), `Receiver` for the camera's refusal (500).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fault {
    pub sender: bool,
    /// Subcodes, outermost first (`ter:InvalidArgVal`, `ter:NoProfile`).
    pub subcodes: Vec<&'static str>,
    pub reason: String,
}

impl Fault {
    pub fn sender(subcodes: &[&'static str], reason: impl Into<String>) -> Self {
        Self {
            sender: true,
            subcodes: subcodes.to_vec(),
            reason: reason.into(),
        }
    }

    pub fn receiver(subcodes: &[&'static str], reason: impl Into<String>) -> Self {
        Self {
            sender: false,
            subcodes: subcodes.to_vec(),
            reason: reason.into(),
        }
    }

    pub fn action_not_supported() -> Self {
        Self::receiver(
            &["ter:ActionNotSupported"],
            "Optional action not implemented",
        )
    }

    pub fn not_authorized() -> Self {
        Self::sender(&["ter:NotAuthorized"], "Sender not authorized")
    }

    pub fn invalid_arg(detail: &'static str, reason: impl Into<String>) -> Self {
        Self::sender(&["ter:InvalidArgVal", detail], reason)
    }

    pub fn status(&self) -> u16 {
        if self.sender { 400 } else { 500 }
    }

    pub fn render(&self) -> String {
        let mut code = String::new();
        for sub in self.subcodes.iter().rev() {
            code = format!("<env:Subcode><env:Value>{sub}</env:Value>{code}</env:Subcode>");
        }
        let value = if self.sender {
            "env:Sender"
        } else {
            "env:Receiver"
        };
        envelope(
            "",
            &format!(
                "<env:Fault><env:Code><env:Value>{value}</env:Value>{code}</env:Code><env:Reason><env:Text xml:lang=\"en\">{}</env:Text></env:Reason></env:Fault>",
                xml::escape(&self.reason)
            ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REQUEST: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<s:Envelope xmlns:s="http://www.w3.org/2003/05/soap-envelope" xmlns:a="http://www.w3.org/2005/08/addressing">
<s:Header>
<a:To>http://CAMERA_HOST/onvif/subscription/3</a:To>
<Security s:mustUnderstand="1" xmlns="http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-wssecurity-secext-1.0.xsd">
<UsernameToken><Username>admin</Username>
<Password Type="http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-username-token-profile-1.0#PasswordDigest">tuOSpGlFlIXsozq4HFNeeGeFLEI=</Password>
<Nonce EncodingType="http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-soap-message-security-1.0#Base64Binary">LKqI6G/AikKCQrN0zqZFlg==</Nonce>
<Created xmlns="http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-wssecurity-utility-1.0.xsd">2010-09-16T07:50:45Z</Created>
</UsernameToken></Security></s:Header>
<s:Body><GetProfiles xmlns="http://www.onvif.org/ver10/media/wsdl"/></s:Body></s:Envelope>"#;

    #[test]
    fn a_request_yields_its_action_namespace_token_and_address() {
        let r = parse(REQUEST.as_bytes()).unwrap();
        assert_eq!((r.action.as_str(), r.ns.as_str()), ("GetProfiles", TRT));
        let t = r.token.unwrap();
        assert_eq!(t.username, "admin");
        assert_eq!(t.created, "2010-09-16T07:50:45Z");
        assert_eq!(t.nonce.len(), 16);
        assert_eq!(
            crate::codec::base64_encode(&t.digest.unwrap()),
            "tuOSpGlFlIXsozq4HFNeeGeFLEI="
        );
        assert_eq!(
            r.to.as_deref(),
            Some("http://CAMERA_HOST/onvif/subscription/3")
        );
    }

    #[test]
    fn plain_text_passwords_carry_no_digest_and_non_soap_is_refused() {
        let plain = REQUEST.replace("#PasswordDigest", "#PasswordText");
        assert_eq!(parse(plain.as_bytes()).unwrap().token.unwrap().digest, None);
        assert_eq!(parse(b"<html/>"), Err(ParseError::NotSoap));
        assert!(parse(b"<s:Envelope xmlns:s=\"x\"><s:Body/></s:Envelope>").is_err());
    }

    #[test]
    fn faults_nest_subcodes_and_map_to_http_status() {
        let f = Fault::invalid_arg("ter:NoProfile", "no <such> profile");
        assert_eq!(f.status(), 400);
        let xml = f.render();
        assert!(xml.contains("<env:Value>env:Sender</env:Value><env:Subcode><env:Value>ter:InvalidArgVal</env:Value><env:Subcode><env:Value>ter:NoProfile</env:Value></env:Subcode></env:Subcode>"));
        assert!(xml.contains("no &lt;such&gt; profile"));
        crate::xml::parse(xml.as_bytes()).unwrap();
        assert_eq!(Fault::action_not_supported().status(), 500);
    }
}
