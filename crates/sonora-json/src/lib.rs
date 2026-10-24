//! Dependency-free JSON for SONORA.
//!
//! Provides an insertion-ordered [`Value`] model, a strict RFC 8259 parser with
//! a nesting limit, compact/pretty serialisation and a [`ToJson`] trait used by
//! the engine and core to build IPC payloads.

use std::collections::{BTreeMap, HashMap};
use std::fmt::{self, Write as _};

mod parse;
pub use parse::{parse, ParseError};

/// Maximum nesting depth accepted by the parser.
pub const MAX_DEPTH: usize = 128;

/// A JSON value. Objects preserve insertion order.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum Value {
    #[default]
    Null,
    Bool(bool),
    /// Integral numbers that fit in an `i64`.
    Int(i64),
    /// All other numbers. Non-finite values serialise as `null`.
    Float(f64),
    String(String),
    Array(Vec<Value>),
    Object(Map),
}

/// Insertion-ordered string map used for JSON objects.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Map {
    entries: Vec<(String, Value)>,
}

impl Map {
    pub fn new() -> Self {
        Map { entries: Vec::new() }
    }

    pub fn with_capacity(n: usize) -> Self {
        Map { entries: Vec::with_capacity(n) }
    }

    /// Inserts or replaces `key`. Returns the previous value if any.
    pub fn insert(&mut self, key: impl Into<String>, value: impl Into<Value>) -> Option<Value> {
        let key = key.into();
        let value = value.into();
        if let Some(slot) = self.entries.iter_mut().find(|(k, _)| *k == key) {
            return Some(std::mem::replace(&mut slot.1, value));
        }
        self.entries.push((key, value));
        None
    }

    /// Appends without checking for duplicates. Use only when keys are known
    /// to be unique (hot paths that build large result sets).
    pub fn push_unchecked(&mut self, key: impl Into<String>, value: impl Into<Value>) {
        self.entries.push((key.into(), value.into()));
    }

    /// Whether no key appears twice (checked by [`obj_unique!`] in debug builds).
    #[doc(hidden)]
    pub fn keys_unique(&self) -> bool {
        self.entries.iter().enumerate().all(|(i, (k, _))| self.entries[..i].iter().all(|(o, _)| o != k))
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.entries.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    pub fn get_mut(&mut self, key: &str) -> Option<&mut Value> {
        self.entries.iter_mut().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    pub fn remove(&mut self, key: &str) -> Option<Value> {
        let idx = self.entries.iter().position(|(k, _)| k == key)?;
        Some(self.entries.remove(idx).1)
    }

    pub fn contains_key(&self, key: &str) -> bool {
        self.entries.iter().any(|(k, _)| k == key)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &Value)> {
        self.entries.iter().map(|(k, v)| (k.as_str(), v))
    }

    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|(k, _)| k.as_str())
    }

    pub fn into_entries(self) -> Vec<(String, Value)> {
        self.entries
    }
}

impl FromIterator<(String, Value)> for Map {
    fn from_iter<I: IntoIterator<Item = (String, Value)>>(iter: I) -> Self {
        let mut m = Map::new();
        for (k, v) in iter {
            m.insert(k, v);
        }
        m
    }
}

/// Error returned by typed accessors.
#[derive(Clone, Debug, PartialEq)]
pub struct AccessError {
    pub key: String,
    pub expected: &'static str,
    pub missing: bool,
}

impl fmt::Display for AccessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.missing {
            write!(f, "missing required field `{}`", self.key)
        } else {
            write!(f, "field `{}` must be {}", self.key, self.expected)
        }
    }
}

impl std::error::Error for AccessError {}

impl Value {
    pub fn object() -> Value {
        Value::Object(Map::new())
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Int(i) => Some(*i),
            Value::Float(f) if f.is_finite() && f.fract() == 0.0 && f.abs() < 9.007_199_254_740_992e15 => {
                Some(*f as i64)
            }
            _ => None,
        }
    }

    pub fn as_u64(&self) -> Option<u64> {
        self.as_i64().and_then(|i| u64::try_from(i).ok())
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Int(i) => Some(*i as f64),
            Value::Float(f) => Some(*f),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&Vec<Value>> {
        match self {
            Value::Array(a) => Some(a),
            _ => None,
        }
    }

    pub fn as_object(&self) -> Option<&Map> {
        match self {
            Value::Object(o) => Some(o),
            _ => None,
        }
    }

    pub fn as_object_mut(&mut self) -> Option<&mut Map> {
        match self {
            Value::Object(o) => Some(o),
            _ => None,
        }
    }

    /// Object field lookup; `None` for non-objects or missing keys.
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.as_object().and_then(|o| o.get(key))
    }

    /// Like [`Value::get`] but treats explicit `null` as missing.
    pub fn get_present(&self, key: &str) -> Option<&Value> {
        self.get(key).filter(|v| !v.is_null())
    }

    /// Inserts into an object value; no-op for other variants.
    pub fn set(&mut self, key: impl Into<String>, value: impl Into<Value>) {
        if let Value::Object(o) = self {
            o.insert(key, value);
        }
    }

    pub fn req_str(&self, key: &str) -> Result<&str, AccessError> {
        match self.get_present(key) {
            None => Err(AccessError { key: key.into(), expected: "a string", missing: true }),
            Some(v) => v.as_str().ok_or(AccessError { key: key.into(), expected: "a string", missing: false }),
        }
    }

    pub fn req_i64(&self, key: &str) -> Result<i64, AccessError> {
        match self.get_present(key) {
            None => Err(AccessError { key: key.into(), expected: "an integer", missing: true }),
            Some(v) => v.as_i64().ok_or(AccessError { key: key.into(), expected: "an integer", missing: false }),
        }
    }

    pub fn req_f64(&self, key: &str) -> Result<f64, AccessError> {
        match self.get_present(key) {
            None => Err(AccessError { key: key.into(), expected: "a number", missing: true }),
            Some(v) => v.as_f64().ok_or(AccessError { key: key.into(), expected: "a number", missing: false }),
        }
    }

    pub fn req_bool(&self, key: &str) -> Result<bool, AccessError> {
        match self.get_present(key) {
            None => Err(AccessError { key: key.into(), expected: "a boolean", missing: true }),
            Some(v) => v.as_bool().ok_or(AccessError { key: key.into(), expected: "a boolean", missing: false }),
        }
    }

    pub fn req_array(&self, key: &str) -> Result<&Vec<Value>, AccessError> {
        match self.get_present(key) {
            None => Err(AccessError { key: key.into(), expected: "an array", missing: true }),
            Some(v) => v.as_array().ok_or(AccessError { key: key.into(), expected: "an array", missing: false }),
        }
    }

    /// Optional typed accessors: `Ok(None)` when absent or null, error when present with the wrong type.
    pub fn opt_str(&self, key: &str) -> Result<Option<&str>, AccessError> {
        match self.get_present(key) {
            None => Ok(None),
            Some(v) => v
                .as_str()
                .map(Some)
                .ok_or(AccessError { key: key.into(), expected: "a string", missing: false }),
        }
    }

    pub fn opt_i64(&self, key: &str) -> Result<Option<i64>, AccessError> {
        match self.get_present(key) {
            None => Ok(None),
            Some(v) => v
                .as_i64()
                .map(Some)
                .ok_or(AccessError { key: key.into(), expected: "an integer", missing: false }),
        }
    }

    pub fn opt_f64(&self, key: &str) -> Result<Option<f64>, AccessError> {
        match self.get_present(key) {
            None => Ok(None),
            Some(v) => v
                .as_f64()
                .map(Some)
                .ok_or(AccessError { key: key.into(), expected: "a number", missing: false }),
        }
    }

    pub fn opt_bool(&self, key: &str) -> Result<Option<bool>, AccessError> {
        match self.get_present(key) {
            None => Ok(None),
            Some(v) => v
                .as_bool()
                .map(Some)
                .ok_or(AccessError { key: key.into(), expected: "a boolean", missing: false }),
        }
    }

    /// Serialises to a compact string.
    pub fn to_string_compact(&self) -> String {
        let mut out = String::with_capacity(64);
        write_value(&mut out, self, None, 0);
        out
    }

    /// Serialises with two-space indentation.
    pub fn to_string_pretty(&self) -> String {
        let mut out = String::with_capacity(128);
        write_value(&mut out, self, Some(2), 0);
        out
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_string_compact())
    }
}

/// Rounds a float to `decimals` places and wraps it, mapping non-finite values to `null`.
pub fn num(x: f64, decimals: u32) -> Value {
    if !x.is_finite() {
        return Value::Null;
    }
    let p = 10f64.powi(decimals as i32);
    let r = (x * p).round() / p;
    // Avoid "-0".
    Value::Float(if r == 0.0 { 0.0 } else { r })
}

/// Optional variant of [`num`].
pub fn opt_num(x: Option<f64>, decimals: u32) -> Value {
    match x {
        Some(v) => num(v, decimals),
        None => Value::Null,
    }
}

fn write_indent(out: &mut String, indent: Option<usize>, level: usize) {
    if let Some(n) = indent {
        out.push('\n');
        for _ in 0..(n * level) {
            out.push(' ');
        }
    }
}

fn write_value(out: &mut String, v: &Value, indent: Option<usize>, level: usize) {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Int(i) => {
            let _ = write!(out, "{i}");
        }
        Value::Float(f) => write_float(out, *f),
        Value::String(s) => write_str(out, s),
        Value::Array(items) => {
            if items.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_indent(out, indent, level + 1);
                write_value(out, item, indent, level + 1);
            }
            write_indent(out, indent, level);
            out.push(']');
        }
        Value::Object(map) => {
            if map.is_empty() {
                out.push_str("{}");
                return;
            }
            out.push('{');
            for (i, (k, item)) in map.entries.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_indent(out, indent, level + 1);
                write_str(out, k);
                out.push(':');
                if indent.is_some() {
                    out.push(' ');
                }
                write_value(out, item, indent, level + 1);
            }
            write_indent(out, indent, level);
            out.push('}');
        }
    }
}

fn write_float(out: &mut String, f: f64) {
    if !f.is_finite() {
        out.push_str("null");
        return;
    }
    if f == 0.0 {
        out.push('0');
        return;
    }
    if f.fract() == 0.0 && f.abs() < 1e15 {
        let _ = write!(out, "{}", f as i64);
        return;
    }
    // Debug formatting yields the shortest representation that round-trips
    // and switches to exponent notation for very large/small magnitudes.
    let _ = write!(out, "{f:?}");
}

/// Writes a JSON string literal with escaping. U+2028/U+2029 are escaped so
/// the output is also safe to embed in JavaScript source.
pub fn write_str(out: &mut String, s: &str) {
    out.push('"');
    let mut start = 0;
    for (i, ch) in s.char_indices() {
        let esc: Option<&str> = match ch {
            '"' => Some("\\\""),
            '\\' => Some("\\\\"),
            '\n' => Some("\\n"),
            '\r' => Some("\\r"),
            '\t' => Some("\\t"),
            '\u{08}' => Some("\\b"),
            '\u{0C}' => Some("\\f"),
            _ => None,
        };
        let needs_u = ch < '\u{20}' || ch == '\u{2028}' || ch == '\u{2029}' || ch == '\u{7F}';
        if esc.is_some() || needs_u {
            out.push_str(&s[start..i]);
            if let Some(e) = esc {
                out.push_str(e);
            } else {
                let _ = write!(out, "\\u{:04x}", ch as u32);
            }
            start = i + ch.len_utf8();
        }
    }
    out.push_str(&s[start..]);
    out.push('"');
}

// ---------------------------------------------------------------------------
// Conversions
// ---------------------------------------------------------------------------

impl From<bool> for Value {
    fn from(b: bool) -> Self {
        Value::Bool(b)
    }
}

macro_rules! from_int {
    ($($t:ty),*) => {$(
        impl From<$t> for Value {
            fn from(i: $t) -> Self { Value::Int(i as i64) }
        }
    )*};
}
from_int!(i8, i16, i32, i64, u8, u16, u32, isize);

impl From<u64> for Value {
    fn from(i: u64) -> Self {
        if i <= i64::MAX as u64 {
            Value::Int(i as i64)
        } else {
            Value::Float(i as f64)
        }
    }
}

impl From<usize> for Value {
    fn from(i: usize) -> Self {
        Value::from(i as u64)
    }
}

impl From<f64> for Value {
    fn from(f: f64) -> Self {
        if f.is_finite() {
            Value::Float(f)
        } else {
            Value::Null
        }
    }
}

impl From<f32> for Value {
    fn from(f: f32) -> Self {
        Value::from(f as f64)
    }
}

impl From<String> for Value {
    fn from(s: String) -> Self {
        Value::String(s)
    }
}

impl From<&str> for Value {
    fn from(s: &str) -> Self {
        Value::String(s.to_string())
    }
}

impl From<&String> for Value {
    fn from(s: &String) -> Self {
        Value::String(s.clone())
    }
}

impl From<Map> for Value {
    fn from(m: Map) -> Self {
        Value::Object(m)
    }
}

impl From<Vec<Value>> for Value {
    fn from(v: Vec<Value>) -> Self {
        Value::Array(v)
    }
}

impl<T: Into<Value>> From<Option<T>> for Value {
    fn from(o: Option<T>) -> Self {
        match o {
            Some(v) => v.into(),
            None => Value::Null,
        }
    }
}

/// Conversion of domain types into JSON.
pub trait ToJson {
    fn to_json(&self) -> Value;
}

impl ToJson for Value {
    fn to_json(&self) -> Value {
        self.clone()
    }
}

impl ToJson for bool {
    fn to_json(&self) -> Value {
        Value::Bool(*self)
    }
}

macro_rules! to_json_via_from {
    ($($t:ty),*) => {$(
        impl ToJson for $t {
            fn to_json(&self) -> Value { Value::from(*self) }
        }
    )*};
}
to_json_via_from!(i8, i16, i32, i64, u8, u16, u32, u64, usize, isize, f32, f64);

impl ToJson for String {
    fn to_json(&self) -> Value {
        Value::String(self.clone())
    }
}

impl ToJson for str {
    fn to_json(&self) -> Value {
        Value::String(self.to_string())
    }
}

impl<T: ToJson + ?Sized> ToJson for &T {
    fn to_json(&self) -> Value {
        (**self).to_json()
    }
}

impl<T: ToJson> ToJson for Option<T> {
    fn to_json(&self) -> Value {
        match self {
            Some(v) => v.to_json(),
            None => Value::Null,
        }
    }
}

impl<T: ToJson> ToJson for Vec<T> {
    fn to_json(&self) -> Value {
        Value::Array(self.iter().map(ToJson::to_json).collect())
    }
}

impl<T: ToJson> ToJson for [T] {
    fn to_json(&self) -> Value {
        Value::Array(self.iter().map(ToJson::to_json).collect())
    }
}

impl<T: ToJson> ToJson for BTreeMap<String, T> {
    fn to_json(&self) -> Value {
        let mut m = Map::with_capacity(self.len());
        for (k, v) in self {
            m.push_unchecked(k.clone(), v.to_json());
        }
        Value::Object(m)
    }
}

impl<T: ToJson> ToJson for HashMap<String, T> {
    fn to_json(&self) -> Value {
        // Sort for deterministic output.
        let mut keys: Vec<&String> = self.keys().collect();
        keys.sort();
        let mut m = Map::with_capacity(self.len());
        for k in keys {
            m.push_unchecked(k.clone(), self[k].to_json());
        }
        Value::Object(m)
    }
}

/// Builds a JSON object: `obj! { "a" => 1, "b" => "x" }`. Values go through [`ToJson`].
#[macro_export]
macro_rules! obj {
    () => { $crate::Value::Object($crate::Map::new()) };
    ($($k:expr => $v:expr),+ $(,)?) => {{
        let mut m = $crate::Map::new();
        $( m.insert($k, $crate::ToJson::to_json(&$v)); )+
        $crate::Value::Object(m)
    }};
}

/// Like [`obj!`], for objects whose keys are distinct literals and that are
/// built by the thousand (track rows): no search for a key already there,
/// and one allocation for the entries.
#[macro_export]
macro_rules! obj_unique {
    ($($k:expr => $v:expr),+ $(,)?) => {{
        let mut m = $crate::Map::with_capacity($crate::count_keys!($($k),+));
        $( m.push_unchecked($k, $crate::ToJson::to_json(&$v)); )+
        debug_assert!(m.keys_unique(), "obj_unique! with a repeated key");
        $crate::Value::Object(m)
    }};
}

/// Like [`obj!`], but takes each value by move ([`Value::from`]) instead of
/// converting a reference: a large array or object is moved in, not copied.
#[macro_export]
macro_rules! obj_move {
    ($($k:expr => $v:expr),+ $(,)?) => {{
        let mut m = $crate::Map::with_capacity($crate::count_keys!($($k),+));
        $( m.insert($k, $crate::Value::from($v)); )+
        $crate::Value::Object(m)
    }};
}

#[doc(hidden)]
#[macro_export]
macro_rules! count_keys {
    () => { 0usize };
    ($h:expr $(, $t:expr)*) => { 1usize + $crate::count_keys!($($t),*) };
}

/// Builds a JSON array from expressions that implement [`ToJson`].
#[macro_export]
macro_rules! arr {
    () => { $crate::Value::Array(Vec::new()) };
    ($($v:expr),+ $(,)?) => {
        $crate::Value::Array(vec![$($crate::ToJson::to_json(&$v)),+])
    };
}

/// Converts `snake_case` to `camelCase`.
pub fn camel(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut upper = false;
    for ch in s.chars() {
        if ch == '_' {
            upper = true;
        } else if upper {
            out.extend(ch.to_uppercase());
            upper = false;
        } else {
            out.push(ch);
        }
    }
    out
}

#[cfg(test)]
mod tests {

    #[test]
    fn obj_unique_builds_the_same_object_as_obj() {
        let a = crate::obj_unique! { "id" => 7i64, "title" => "Night Drive", "bpm" => crate::num(124.004, 2), "flags" => vec!["clipping".to_string()] };
        let b = crate::obj! { "id" => 7i64, "title" => "Night Drive", "bpm" => crate::num(124.004, 2), "flags" => vec!["clipping".to_string()] };
        assert_eq!(a.to_string_compact(), b.to_string_compact());
        assert_eq!(a.to_string_compact(), r#"{"id":7,"title":"Night Drive","bpm":124,"flags":["clipping"]}"#);
    }

    use super::*;

    #[test]
    fn roundtrip_basic() {
        let src = r#"{"a":1,"b":[true,false,null],"c":"x\nyé\"","d":-2.5e-3,"e":{}}"#;
        let v = parse(src).unwrap();
        assert_eq!(v.get("a"), Some(&Value::Int(1)));
        assert_eq!(v.get("c").unwrap().as_str().unwrap(), "x\nyé\"");
        assert_eq!(v.get("d").unwrap().as_f64().unwrap(), -0.0025);
        let again = parse(&v.to_string_compact()).unwrap();
        assert_eq!(v, again);
        let pretty = parse(&v.to_string_pretty()).unwrap();
        assert_eq!(v, pretty);
    }

    #[test]
    fn insertion_order_and_replace() {
        let mut m = Map::new();
        m.insert("z", 1);
        m.insert("a", 2);
        m.insert("z", 3);
        let v = Value::Object(m);
        assert_eq!(v.to_string_compact(), r#"{"z":3,"a":2}"#);
    }

    #[test]
    fn floats_and_nonfinite() {
        assert_eq!(Value::from(f64::NAN).to_string_compact(), "null");
        assert_eq!(Value::Float(1.0).to_string_compact(), "1");
        assert_eq!(Value::Float(0.1).to_string_compact(), "0.1");
        assert_eq!(num(1.23456, 2).to_string_compact(), "1.23");
        assert_eq!(num(-0.0001, 2).to_string_compact(), "0");
        let tiny = Value::Float(1e-12).to_string_compact();
        assert_eq!(parse(&tiny).unwrap().as_f64().unwrap(), 1e-12);
    }

    #[test]
    fn escapes() {
        let s = "tab\there \u{2028} ctrl\u{01}";
        let v = Value::from(s);
        let out = v.to_string_compact();
        assert!(out.contains("\\t"));
        assert!(out.contains("\\u2028"));
        assert!(out.contains("\\u0001"));
        assert_eq!(parse(&out).unwrap().as_str().unwrap(), s);
    }

    #[test]
    fn macros_and_accessors() {
        let v = obj! { "name" => "x", "n" => 4u32, "opt" => None::<i64>, "list" => vec![1, 2] };
        assert_eq!(v.req_str("name").unwrap(), "x");
        assert_eq!(v.req_i64("n").unwrap(), 4);
        assert!(v.req_i64("opt").unwrap_err().missing);
        assert!(v.req_str("n").is_err());
        assert_eq!(v.opt_i64("opt").unwrap(), None);
        assert_eq!(v.req_array("list").unwrap().len(), 2);
        assert_eq!(arr![1, "a"].to_string_compact(), r#"[1,"a"]"#);
        assert_eq!(camel("sample_rate_hz"), "sampleRateHz");
    }
}
