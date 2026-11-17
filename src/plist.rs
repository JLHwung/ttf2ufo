//! Property lists, serialized exactly like fontTools.misc.plistlib (via lxml)
//! with `sort_keys=True` and two-space indentation.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fmt::Write as _;

use crate::Error;
use crate::pyfmt::float_repr;

/// lxml's check on every string fontTools writes as XML (plistlib and
/// glifLib): no C0 control characters other than tab, LF and CR, and no
/// U+FFFE or U+FFFF. ufoLib2 refuses to save a UFO that fails it.
pub fn check_xml_str(s: &str) -> Result<(), Error> {
    // U+FFFE and U+FFFF are EF BF BE and EF BF BF; 0xEF also starts other
    // characters, so a hit is checked exactly.
    let b = s.as_bytes();
    let candidate = |c: u8| (c < 0x20) & (c != b'\t') & (c != b'\n') & (c != b'\r') | (c == 0xEF);
    if !any_byte(b, candidate) {
        return Ok(());
    }
    let bad = b.iter().enumerate().any(|(i, &c)| {
        candidate(c)
            && (c != 0xEF
                || (b.get(i + 1) == Some(&0xBF) && matches!(b.get(i + 2), Some(0xBE | 0xBF))))
    });
    if bad {
        return Err(format!(
            "{s:?}: All strings must be XML compatible: Unicode or ASCII, no NULL bytes or \
             control characters"
        )
        .into());
    }
    Ok(())
}

/// [`check_xml_str`] on every string and dict key in `value`.
pub fn check_xml_value(value: &Value) -> Result<(), Error> {
    match value {
        Value::Str(s) => check_xml_str(s),
        Value::Array(items) => items.iter().try_for_each(check_xml_value),
        Value::Dict(d) => check_xml_dict(d),
        Value::Bool(_) | Value::Int(_) | Value::Real(_) => Ok(()),
    }
}

/// [`check_xml_str`] on every string and key in `dict`.
pub fn check_xml_dict(dict: &Dict) -> Result<(), Error> {
    dict.iter().try_for_each(|(k, v)| {
        check_xml_str(k)?;
        check_xml_value(v)
    })
}

/// A plist dictionary. `BTreeMap<String, _>` iterates in code point order,
/// which is how Python sorts `str` keys.
pub type Dict = BTreeMap<String, Value>;

/// A plist value.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// `<true/>` or `<false/>`.
    Bool(bool),
    /// `<integer>`.
    Int(i64),
    /// `<real>`, formatted like Python's `repr(float)`.
    Real(f64),
    /// `<string>`.
    Str(String),
    /// `<array>`.
    Array(Vec<Value>),
    /// `<dict>`, with keys sorted.
    Dict(Dict),
}

impl From<bool> for Value {
    fn from(v: bool) -> Self {
        Value::Bool(v)
    }
}
impl From<i64> for Value {
    fn from(v: i64) -> Self {
        Value::Int(v)
    }
}
macro_rules! from_small_int {
    ($($t:ty),*) => {$(
        impl From<$t> for Value {
            fn from(v: $t) -> Self {
                Value::Int(v as i64)
            }
        }
    )*};
}
from_small_int!(u8, i16, u16);

impl From<i32> for Value {
    fn from(v: i32) -> Self {
        Value::Int(v as i64)
    }
}
impl From<u32> for Value {
    fn from(v: u32) -> Self {
        Value::Int(v as i64)
    }
}
impl From<f64> for Value {
    fn from(v: f64) -> Self {
        Value::Real(v)
    }
}
impl From<&str> for Value {
    fn from(v: &str) -> Self {
        Value::Str(v.to_string())
    }
}
impl From<String> for Value {
    fn from(v: String) -> Self {
        Value::Str(v)
    }
}
impl From<Dict> for Value {
    fn from(v: Dict) -> Self {
        Value::Dict(v)
    }
}
impl<T: Into<Value>> From<Vec<T>> for Value {
    fn from(v: Vec<T>) -> Self {
        Value::Array(v.into_iter().map(Into::into).collect())
    }
}

/// Escape element text as lxml does.
pub fn escape_text(s: &str) -> Cow<'_, str> {
    escape(
        s,
        |b| (b == b'&') | (b == b'<') | (b == b'>') | (b == b'\r'),
        |b| match b {
            b'&' => "&amp;",
            b'<' => "&lt;",
            b'>' => "&gt;",
            _ => "&#13;",
        },
    )
}

/// Escape an attribute value as lxml does.
pub fn escape_attr(s: &str) -> Cow<'_, str> {
    escape(
        s,
        |b| {
            (b == b'&')
                | (b == b'<')
                | (b == b'>')
                | (b == b'"')
                | (b == b'\n')
                | (b == b'\r')
                | (b == b'\t')
        },
        |b| match b {
            b'&' => "&amp;",
            b'<' => "&lt;",
            b'>' => "&gt;",
            b'"' => "&quot;",
            b'\n' => "&#10;",
            b'\r' => "&#13;",
            _ => "&#9;",
        },
    )
}

/// Whether any byte satisfies `pred`. Bytes are tested in 64-byte blocks
/// without branching, which vectorizes; most strings have no such byte.
/// `pred` must not branch either: written with `&` and `|` rather than
/// `&&`, `||` or `matches!`, and with a `u8` accumulator rather than a
/// `bool` one, this runs at about 20 GB/s instead of 2.
#[inline(always)]
fn any_byte(bytes: &[u8], pred: impl Fn(u8) -> bool) -> bool {
    let (blocks, rest) = bytes.as_chunks::<64>();
    for block in blocks {
        let mut hit = 0u8;
        for &c in block {
            hit |= pred(c) as u8;
        }
        if hit != 0 {
            return true;
        }
    }
    rest.iter().any(|&c| pred(c))
}

/// Replace the ASCII bytes `entity` maps, copying the runs between them
/// whole; borrows `s` when nothing needs escaping.
fn escape(
    s: &str,
    special: impl Fn(u8) -> bool + Copy,
    entity: impl Fn(u8) -> &'static str,
) -> Cow<'_, str> {
    if !any_byte(s.as_bytes(), special) {
        return Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len() + 16);
    let mut start = 0;
    for (i, &b) in s.as_bytes().iter().enumerate() {
        if special(b) {
            // the byte is ASCII, so `start..i` lies on char boundaries
            out.push_str(&s[start..i]);
            out.push_str(entity(b));
            start = i + 1;
        }
    }
    out.push_str(&s[start..]);
    Cow::Owned(out)
}

/// Two spaces per indentation level, without allocating for common depths.
fn pad(indent: usize) -> Cow<'static, str> {
    const SPACES: &str = "                                                                ";
    match SPACES.get(..2 * indent) {
        Some(p) => Cow::Borrowed(p),
        None => Cow::Owned("  ".repeat(indent)),
    }
}

/// Append `value` as plist XML, one element per line, at `indent` levels.
pub fn write_value(out: &mut String, value: &Value, indent: usize) {
    let pad = pad(indent);
    let pad = pad.as_ref();
    match value {
        Value::Bool(true) => {
            out.push_str(pad);
            out.push_str("<true/>\n");
        }
        Value::Bool(false) => {
            out.push_str(pad);
            out.push_str("<false/>\n");
        }
        Value::Int(i) => writeln!(out, "{pad}<integer>{i}</integer>").unwrap(),
        Value::Real(f) => writeln!(out, "{pad}<real>{}</real>", float_repr(*f)).unwrap(),
        Value::Str(s) => {
            out.push_str(pad);
            out.push_str("<string>");
            out.push_str(&escape_text(s));
            out.push_str("</string>\n");
        }
        Value::Array(items) if items.is_empty() => {
            out.push_str(pad);
            out.push_str("<array/>\n");
        }
        Value::Array(items) => {
            out.push_str(pad);
            out.push_str("<array>\n");
            for item in items {
                write_value(out, item, indent + 1);
            }
            out.push_str(pad);
            out.push_str("</array>\n");
        }
        Value::Dict(d) => write_dict(out, d, indent),
    }
}

/// Append a dict as plist XML at `indent` levels.
pub fn write_dict(out: &mut String, dict: &Dict, indent: usize) {
    let pad = pad(indent);
    let pad = pad.as_ref();
    if dict.is_empty() {
        out.push_str(pad);
        out.push_str("<dict/>\n");
        return;
    }
    out.push_str(pad);
    out.push_str("<dict>\n");
    for (k, v) in dict {
        out.push_str(pad);
        out.push_str("  <key>");
        out.push_str(&escape_text(k));
        out.push_str("</key>\n");
        write_value(out, v, indent + 1);
    }
    out.push_str(pad);
    out.push_str("</dict>\n");
}

/// A complete .plist file, as `fontTools.misc.plistlib.dumps` writes it.
pub fn to_file(root: &Value) -> String {
    file_with(|s| write_value(s, root, 1))
}

/// A complete .plist file whose root is `dict`.
pub fn dict_file(dict: &Dict) -> String {
    file_with(|s| write_dict(s, dict, 1))
}

fn file_with(body: impl FnOnce(&mut String)) -> String {
    let mut s = String::from(
        "<?xml version='1.0' encoding='UTF-8'?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n",
    );
    body(&mut s);
    s.push_str("</plist>\n");
    s
}

#[cfg(test)]
mod scan_tests {
    use super::*;

    #[test]
    fn check_xml_str_finds_bad_characters_anywhere() {
        let ok = "a\tb\nc\r".repeat(40);
        assert!(check_xml_str(&ok).is_ok());
        // U+FFFF (EF BF BF) split across the 64-byte chunk boundary
        let s = format!("{}\u{FFFF}", "x".repeat(63));
        assert!(check_xml_str(&s).is_err());
        let s = format!("{}\u{FFFE}x", "x".repeat(62));
        assert!(check_xml_str(&s).is_err());
        // a control character as the first byte of the second chunk
        let s = format!("{}\u{1}", "x".repeat(64));
        assert!(check_xml_str(&s).is_err());
        // other characters starting with 0xEF are fine (U+FFFD is EF BF BD)
        let s = format!("{}\u{FFFD}\u{F000}", "x".repeat(63));
        assert!(check_xml_str(&s).is_ok());
    }

    #[test]
    fn escaping_matches_char_by_char() {
        let reference = |s: &str| -> String {
            s.chars()
                .map(|c| match c {
                    '&' => "&amp;".to_string(),
                    '<' => "&lt;".to_string(),
                    '>' => "&gt;".to_string(),
                    '\r' => "&#13;".to_string(),
                    c => c.to_string(),
                })
                .collect()
        };
        for s in [
            String::new(),
            "plain".to_string(),
            format!("{}&{}<é>\r", "ü".repeat(40), "x".repeat(30)),
            format!("{}>", "x".repeat(63)),
            format!("{}<", "中".repeat(30)),
        ] {
            assert_eq!(escape_text(&s), reference(&s), "{s:?}");
        }
        assert_eq!(escape_attr("a\"b\n\t"), "a&quot;b&#10;&#9;");
    }
}
