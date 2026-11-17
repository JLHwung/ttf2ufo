//! Property lists, serialized exactly like fontTools.misc.plistlib (via lxml)
//! with `sort_keys=True` and two-space indentation.

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
pub fn escape_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\r' => out.push_str("&#13;"),
            c => out.push(c),
        }
    }
    out
}

/// Escape an attribute value as lxml does.
pub fn escape_attr(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
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
    out
}

fn plist_string(s: &str) -> String {
    escape_text(s)
}

/// Append `value` as plist XML, one element per line, at `indent` levels.
pub fn write_value(out: &mut String, value: &Value, indent: usize) {
    let pad = "  ".repeat(indent);
    match value {
        Value::Bool(true) => writeln!(out, "{pad}<true/>"),
        Value::Bool(false) => writeln!(out, "{pad}<false/>"),
        Value::Int(i) => writeln!(out, "{pad}<integer>{i}</integer>"),
        Value::Real(f) => writeln!(out, "{pad}<real>{}</real>", float_repr(*f)),
        Value::Str(s) if s.is_empty() => writeln!(out, "{pad}<string></string>"),
        Value::Str(s) => writeln!(out, "{pad}<string>{}</string>", plist_string(s)),
        Value::Array(items) if items.is_empty() => writeln!(out, "{pad}<array/>"),
        Value::Array(items) => {
            writeln!(out, "{pad}<array>").unwrap();
            for item in items {
                write_value(out, item, indent + 1);
            }
            writeln!(out, "{pad}</array>")
        }
        Value::Dict(d) if d.is_empty() => writeln!(out, "{pad}<dict/>"),
        Value::Dict(d) => {
            writeln!(out, "{pad}<dict>").unwrap();
            for (k, v) in d {
                writeln!(out, "{pad}  <key>{}</key>", plist_string(k)).unwrap();
                write_value(out, v, indent + 1);
            }
            writeln!(out, "{pad}</dict>")
        }
    }
    .unwrap();
}

/// A complete .plist file, as `fontTools.misc.plistlib.dumps` writes it.
pub fn to_file(root: &Value) -> String {
    let mut s = String::from(
        "<?xml version='1.0' encoding='UTF-8'?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n",
    );
    write_value(&mut s, root, 1);
    s.push_str("</plist>\n");
    s
}
