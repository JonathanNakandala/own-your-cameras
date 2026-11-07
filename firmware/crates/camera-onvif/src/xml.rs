//! A small, bounded XML reader for SOAP requests, and text escaping for responses.
//!
//! Requests are tiny (tens of elements), so the reader builds a tree with namespace URIs resolved
//! and looks elements up by local name. It refuses DTDs (no entity expansion), more than
//! [`MAX_DEPTH`] levels and more than [`MAX_ELEMENTS`] elements. Pure: no I/O.

use std::collections::HashMap;

pub const MAX_DEPTH: usize = 32;
pub const MAX_ELEMENTS: usize = 2000;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Element {
    /// Local name, without its prefix.
    pub name: String,
    /// Resolved namespace URI ("" when none is in scope).
    pub ns: String,
    /// Attributes by local name.
    pub attrs: Vec<(String, String)>,
    /// Concatenated character data directly inside this element, trimmed.
    pub text: String,
    pub children: Vec<Element>,
}

impl Element {
    pub fn child(&self, name: &str) -> Option<&Element> {
        self.children.iter().find(|c| c.name == name)
    }

    pub fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Element> {
        self.children.iter().filter(move |c| c.name == name)
    }

    /// The first element named `name` at any depth below this one (depth first).
    pub fn descendant(&self, name: &str) -> Option<&Element> {
        self.children.iter().find_map(|c| {
            if c.name == name {
                Some(c)
            } else {
                c.descendant(name)
            }
        })
    }

    /// Text of the child `name`.
    pub fn text_of(&self, name: &str) -> Option<&str> {
        self.child(name).map(|c| c.text.as_str())
    }

    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XmlError {
    NotUtf8,
    Malformed,
    Unsupported,
    TooDeep,
    TooLarge,
}

fn local(qname: &str) -> (&str, &str) {
    match qname.split_once(':') {
        Some((prefix, name)) => (prefix, name),
        None => ("", qname),
    }
}

pub fn unescape(s: &str) -> Result<String, XmlError> {
    if !s.contains('&') {
        return Ok(s.to_string());
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        let end = rest[at..].find(';').ok_or(XmlError::Malformed)? + at;
        let entity = &rest[at + 1..end];
        let c = match entity {
            "lt" => '<',
            "gt" => '>',
            "amp" => '&',
            "quot" => '"',
            "apos" => '\'',
            _ => {
                let code = if let Some(hex) = entity.strip_prefix("#x") {
                    u32::from_str_radix(hex, 16).ok()
                } else if let Some(dec) = entity.strip_prefix('#') {
                    dec.parse().ok()
                } else {
                    None
                };
                code.and_then(char::from_u32).ok_or(XmlError::Malformed)?
            }
        };
        out.push(c);
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

/// Escapes text or an attribute value for output.
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

struct Open {
    element: Element,
    qname: String,
    /// Prefix bindings declared on this element.
    scope: HashMap<String, String>,
    /// Character data seen so far, still escaped.
    text: String,
}

pub fn parse(input: &[u8]) -> Result<Element, XmlError> {
    let s = std::str::from_utf8(input).map_err(|_| XmlError::NotUtf8)?;
    let mut stack: Vec<Open> = Vec::new();
    let mut root: Option<Element> = None;
    let mut count = 0usize;
    let mut i = 0usize;
    let resolve = |stack: &[Open], prefix: &str| -> String {
        stack
            .iter()
            .rev()
            .find_map(|o| o.scope.get(prefix).cloned())
            .unwrap_or_default()
    };
    while i < s.len() {
        let rest = &s[i..];
        if let Some(body) = rest.strip_prefix("<!--") {
            let end = body.find("-->").ok_or(XmlError::Malformed)?;
            i += 4 + end + 3;
        } else if let Some(body) = rest.strip_prefix("<![CDATA[") {
            let end = body.find("]]>").ok_or(XmlError::Malformed)?;
            // Character data is unescaped when the element closes: escape CDATA so it survives verbatim.
            if let Some(open) = stack.last_mut() {
                open.text.push_str(&escape(&body[..end]));
            }
            i += 9 + end + 3;
        } else if rest.starts_with("<!") {
            return Err(XmlError::Unsupported);
        } else if let Some(body) = rest.strip_prefix("<?") {
            let end = body.find("?>").ok_or(XmlError::Malformed)?;
            i += 2 + end + 2;
        } else if let Some(body) = rest.strip_prefix("</") {
            let end = body.find('>').ok_or(XmlError::Malformed)?;
            let qname = body[..end].trim();
            let open = stack.pop().ok_or(XmlError::Malformed)?;
            if open.qname != qname {
                return Err(XmlError::Malformed);
            }
            let mut element = open.element;
            element.text = unescape(open.text.trim())?;
            match stack.last_mut() {
                Some(parent) => parent.element.children.push(element),
                None => root = Some(element),
            }
            i += 2 + end + 1;
        } else if rest.starts_with('<') {
            if root.is_some() {
                return Err(XmlError::Malformed);
            }
            let end = tag_end(rest).ok_or(XmlError::Malformed)?;
            let inner = &rest[1..end];
            let (inner, closed) = match inner.strip_suffix('/') {
                Some(inner) => (inner, true),
                None => (inner, false),
            };
            let (qname, attrs) = split_tag(inner)?;
            count += 1;
            if count > MAX_ELEMENTS {
                return Err(XmlError::TooLarge);
            }
            if stack.len() >= MAX_DEPTH {
                return Err(XmlError::TooDeep);
            }
            let mut scope = HashMap::new();
            let mut plain = Vec::new();
            for (k, v) in attrs {
                let v = unescape(&v)?;
                if k == "xmlns" {
                    scope.insert(String::new(), v);
                } else if let Some(prefix) = k.strip_prefix("xmlns:") {
                    scope.insert(prefix.to_string(), v);
                } else {
                    plain.push((local(&k).1.to_string(), v));
                }
            }
            let (prefix, name) = local(qname);
            let ns = scope
                .get(prefix)
                .cloned()
                .unwrap_or_else(|| resolve(&stack, prefix));
            let element = Element {
                name: name.to_string(),
                ns,
                attrs: plain,
                ..Element::default()
            };
            let open = Open {
                element,
                qname: qname.to_string(),
                scope,
                text: String::new(),
            };
            i += end + 1;
            if closed {
                match stack.last_mut() {
                    Some(parent) => parent.element.children.push(open.element),
                    None => root = Some(open.element),
                }
            } else {
                stack.push(open);
            }
        } else {
            let end = rest.find('<').unwrap_or(rest.len());
            match stack.last_mut() {
                Some(open) => open.text.push_str(&rest[..end]),
                None if rest[..end].trim().is_empty() => {}
                None => return Err(XmlError::Malformed),
            }
            i += end;
        }
    }
    if !stack.is_empty() {
        return Err(XmlError::Malformed);
    }
    root.ok_or(XmlError::Malformed)
}

/// Index of the `>` closing the tag that starts `s`, outside quoted attribute values.
fn tag_end(s: &str) -> Option<usize> {
    let mut quote = None;
    for (i, c) in s.char_indices() {
        match (quote, c) {
            (None, '"' | '\'') => quote = Some(c),
            (Some(q), c) if c == q => quote = None,
            (None, '>') => return Some(i),
            _ => {}
        }
    }
    None
}

type Attrs = Vec<(String, String)>;

fn split_tag(inner: &str) -> Result<(&str, Attrs), XmlError> {
    let inner = inner.trim();
    let name_end = inner
        .find(|c: char| c.is_ascii_whitespace())
        .unwrap_or(inner.len());
    let qname = &inner[..name_end];
    if qname.is_empty() {
        return Err(XmlError::Malformed);
    }
    let mut attrs = Vec::new();
    let mut rest = inner[name_end..].trim_start();
    while !rest.is_empty() {
        let eq = rest.find('=').ok_or(XmlError::Malformed)?;
        let key = rest[..eq].trim();
        let after = rest[eq + 1..].trim_start();
        let quote = after.chars().next().ok_or(XmlError::Malformed)?;
        if quote != '"' && quote != '\'' {
            return Err(XmlError::Malformed);
        }
        let close = after[1..].find(quote).ok_or(XmlError::Malformed)? + 1;
        attrs.push((key.to_string(), after[1..close].to_string()));
        rest = after[close + 1..].trim_start();
    }
    Ok((qname, attrs))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn namespaces_resolve_through_scopes_and_prefixes_are_dropped() {
        let doc = br#"<?xml version="1.0" encoding="UTF-8"?>
<s:Envelope xmlns:s="http://www.w3.org/2003/05/soap-envelope" xmlns:t="urn:t">
  <!-- a comment -->
  <s:Body><t:Get a:x="1" xmlns:a="urn:a" plain='2'><Token xmlns="urn:d">main &amp; &#x41;</Token><t:E/></t:Get></s:Body>
</s:Envelope>"#;
        let root = parse(doc).unwrap();
        assert_eq!(root.name, "Envelope");
        assert_eq!(root.ns, "http://www.w3.org/2003/05/soap-envelope");
        let get = root.child("Body").unwrap().child("Get").unwrap();
        assert_eq!(get.ns, "urn:t");
        assert_eq!(get.attr("x"), Some("1"));
        assert_eq!(get.attr("plain"), Some("2"));
        let token = get.child("Token").unwrap();
        assert_eq!(
            (token.ns.as_str(), token.text.as_str()),
            ("urn:d", "main & A")
        );
        assert_eq!(get.child("E").unwrap().ns, "urn:t");
        assert_eq!(root.descendant("Token").unwrap().text, "main & A");
        let cdata = parse(b"<a><![CDATA[x & <y>]]></a>").unwrap();
        assert_eq!(cdata.text, "x & <y>");
    }

    #[test]
    fn hostile_or_broken_documents_are_refused() {
        assert_eq!(
            parse(b"<!DOCTYPE x [<!ENTITY a 'b'>]><x/>"),
            Err(XmlError::Unsupported)
        );
        assert_eq!(parse(b"<a><b></a>"), Err(XmlError::Malformed));
        assert_eq!(parse(b"<a>"), Err(XmlError::Malformed));
        assert_eq!(parse(b"<a/><b/>"), Err(XmlError::Malformed));
        assert_eq!(parse(&[0xff, 0xfe]), Err(XmlError::NotUtf8));
        let deep = "<a>".repeat(40) + &"</a>".repeat(40);
        assert_eq!(parse(deep.as_bytes()), Err(XmlError::TooDeep));
        let wide = format!("<a>{}</a>", "<b/>".repeat(MAX_ELEMENTS + 1));
        assert_eq!(parse(wide.as_bytes()), Err(XmlError::TooLarge));
        assert_eq!(parse(b"<a>&bogus;</a>"), Err(XmlError::Malformed));
    }

    #[test]
    fn escape_covers_markup_characters() {
        assert_eq!(
            escape(r#"<a & "b" 'c'>"#),
            "&lt;a &amp; &quot;b&quot; &apos;c&apos;&gt;"
        );
        assert_eq!(unescape(&escape("x<y>&z")).unwrap(), "x<y>&z");
    }
}
