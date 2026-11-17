//! Property lists, serialized exactly like fontTools.misc.plistlib (via lxml)
//! with `sort_keys=True` and two-space indentation.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fmt::Write as _;

use crate::pyfmt::float_repr;

/// A plist dictionary. `BTreeMap<String, _>` iterates in code point order,
/// which is how Python sorts `str` keys.
pub type Dict = BTreeMap<String, Value>;

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Bool(bool),
    Int(i64),
    Real(f64),
    Str(String),
    Array(Vec<Value>),
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
    if !s.contains(['&', '<', '>', '\r']) {
        return Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len() + 16);
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\r' => out.push_str("&#13;"),
            c => out.push(c),
        }
    }
    Cow::Owned(out)
}

/// Escape an attribute value as lxml does.
pub fn escape_attr(s: &str) -> Cow<'_, str> {
    if !s.contains(['&', '<', '>', '"', '\n', '\r', '\t']) {
        return Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len() + 16);
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\n' => out.push_str("&#10;"),
            '\r' => out.push_str("&#13;"),
            '\t' => out.push_str("&#9;"),
            c => out.push(c),
        }
    }
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
