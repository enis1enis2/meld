//! Meld Value Model (spec/value-model.md).
//!
//! JUDGMENT CALL: integers are stored as canonical decimal strings so arbitrary
//! precision survives the T2 JSON envelope without a f64 round-trip.
use std::collections::BTreeMap;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Error)]
pub enum ValueError {
    #[error("invalid integer literal: {0}")]
    InvalidInt(String),
    #[error("invalid wire tag: {0}")]
    InvalidTag(String),
    #[error("type mismatch: {0}")]
    TypeMismatch(String),
    #[error("value cannot be rendered as a source literal: {0}")]
    Unrenderable(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    /// Canonical decimal string, arbitrary precision.
    ///
    /// Wire note: `Int` is encoded as the tagged object `{"$int":"..."}` so it
    /// is never confused with a `String`. The legacy decimal-string reader is
    /// retained in [`Value::from_json`] for old fixtures; use
    /// [`Value::from_json_strict`] to reject that ambiguous form.
    Int(String),
    Float(f64),
    String(String),
    Bytes(Vec<u8>),
    List(Vec<Value>),
    Map(BTreeMap<String, Value>),
    Record(BTreeMap<String, Value>),
    Variant {
        case: String,
        value: Box<Value>,
    },
    Option(Option<Box<Value>>),
    ResultOk(Box<Value>),
    ResultErr(Box<Value>),
    Handle {
        id: String,
        resource: String,
        borrow: bool,
    },
    /// Reference to `region-id:export-name` (spec/value-model.md §1).
    Fn(String),
}

impl Value {
    pub fn int(n: i128) -> Self {
        Value::Int(n.to_string())
    }

    pub fn as_i64_checked(&self) -> Result<i64, ValueError> {
        match self {
            Value::Int(s) => s
                .parse::<i64>()
                .map_err(|_| ValueError::InvalidInt(s.clone())),
            _ => Err(ValueError::TypeMismatch("expected int".to_string())),
        }
    }

    pub fn to_json(&self) -> serde_json::Value {
        use serde_json::{json, Value as J};
        match self {
            Value::Null => J::Null,
            Value::Bool(b) => J::Bool(*b),
            Value::Int(s) => json!({ "$int": s.clone() }),
            Value::Float(f) => {
                if f.is_nan() {
                    J::String("NaN".to_string())
                } else if *f == f64::INFINITY {
                    J::String("Inf".to_string())
                } else if *f == f64::NEG_INFINITY {
                    J::String("-Inf".to_string())
                } else {
                    J::Number(serde_json::Number::from_f64(*f).expect("finite"))
                }
            }
            Value::String(s) => J::String(s.clone()),
            Value::Bytes(b) => json!({ "$bytes": base64(b) }),
            Value::List(xs) => J::Array(xs.iter().map(Value::to_json).collect()),
            Value::Map(m) => {
                let mut inner = serde_json::Map::new();
                for (k, v) in m {
                    inner.insert(k.clone(), v.to_json());
                }
                json!({ "$map": inner })
            }
            Value::Record(m) => {
                let mut inner = serde_json::Map::new();
                for (k, v) in m {
                    inner.insert(k.clone(), v.to_json());
                }
                J::Object(inner)
            }
            Value::Variant { case, value } => {
                json!({ "$variant": { "case": case, "value": value.to_json() } })
            }
            Value::Option(Some(v)) => json!({ "$some": v.to_json() }),
            Value::Option(None) => J::Null,
            Value::ResultOk(v) => json!({ "$ok": v.to_json() }),
            Value::ResultErr(v) => json!({ "$err": v.to_json() }),
            Value::Handle {
                id,
                resource,
                borrow,
            } => {
                json!({ "$handle": { "id": id, "resource": resource, "borrow": borrow } })
            }
            Value::Fn(target) => json!({ "$fn": target }),
        }
    }

    /// Decode a Meld value from its JSON wire representation.
    ///
    /// Compatibility reader: old fixtures encode `Int` as a bare decimal
    /// string, so such strings are still decoded as `Int` here. New encoders
    /// emit the unambiguous `{"$int":"..."}` tag. Use
    /// [`Value::from_json_strict`] when reading untrusted or newly produced
    /// wire data. `$`-prefixed keys are reserved: unknown tags, multiple tags
    /// in one object, and reserved keys in records/maps are hard errors.
    pub fn from_json(j: &serde_json::Value) -> Result<Self, ValueError> {
        decode(j, false)
    }

    /// Strict decoder: rejects the legacy untagged decimal-string form.
    pub fn from_json_strict(j: &serde_json::Value) -> Result<Self, ValueError> {
        decode(j, true)
    }
}

/// Tags owned by the Meld wire format. Any other `$`-prefixed key is reserved
/// and must be rejected rather than silently decoded as a record field.
const WIRE_TAGS: &[&str] = &[
    "$bytes", "$int", "$map", "$variant", "$some", "$ok", "$err", "$handle", "$fn",
];

fn single_reserved_tag(
    object: &serde_json::Map<String, serde_json::Value>,
) -> Result<Option<&str>, ValueError> {
    let tags: Vec<&str> = object
        .keys()
        .map(String::as_str)
        .filter(|key| key.starts_with('$'))
        .collect();
    match tags.as_slice() {
        [] => Ok(None),
        [tag] if WIRE_TAGS.contains(tag) => {
            if object.len() == 1 {
                Ok(Some(*tag))
            } else {
                Err(ValueError::InvalidTag(format!(
                    "tag {tag} must be the only key in its object"
                )))
            }
        }
        [tag] => Err(ValueError::InvalidTag(format!("unknown wire tag {tag}"))),
        _ => Err(ValueError::InvalidTag(
            "multiple wire tags in one object".to_string(),
        )),
    }
}

fn reject_reserved_keys(
    object: &serde_json::Map<String, serde_json::Value>,
    context: &str,
) -> Result<(), ValueError> {
    if let Some(key) = object.keys().find(|key| key.starts_with('$')) {
        return Err(ValueError::InvalidTag(format!(
            "reserved key {key} is not valid in {context}"
        )));
    }
    Ok(())
}

fn decode(j: &serde_json::Value, strict: bool) -> Result<Value, ValueError> {
    use serde_json::Value as J;
    match j {
        J::Null => Ok(Value::Null),
        J::Bool(b) => Ok(Value::Bool(*b)),
        J::Number(n) => Ok(Value::Float(n.as_f64().ok_or_else(|| {
            ValueError::TypeMismatch("number out of f64 range".to_string())
        })?)),
        J::String(s) => match s.as_str() {
            "NaN" => Ok(Value::Float(f64::NAN)),
            "Inf" => Ok(Value::Float(f64::INFINITY)),
            "-Inf" => Ok(Value::Float(f64::NEG_INFINITY)),
            _ if is_decimal(s) => {
                if strict {
                    Err(ValueError::InvalidTag(
                        "untagged integer string; use {\"$int\": \"...\"}".to_string(),
                    ))
                } else {
                    Ok(Value::Int(s.clone()))
                }
            }
            _ => Ok(Value::String(s.clone())),
        },
        J::Array(xs) => Ok(Value::List(
            xs.iter()
                .map(|item| decode(item, strict))
                .collect::<Result<_, _>>()?,
        )),
        J::Object(o) => {
            let tag = single_reserved_tag(o)?;
            match tag {
                Some("$int") => {
                    let s = o
                        .get("$int")
                        .and_then(|value| value.as_str())
                        .ok_or_else(|| ValueError::InvalidTag("$int".to_string()))?;
                    if !is_decimal(s) {
                        return Err(ValueError::InvalidTag(format!("invalid integer: {s:?}")));
                    }
                    Ok(Value::Int(s.to_string()))
                }
                Some("$bytes") => {
                    let s = o
                        .get("$bytes")
                        .and_then(|value| value.as_str())
                        .ok_or_else(|| ValueError::InvalidTag("$bytes".to_string()))?;
                    Ok(Value::Bytes(unbase64(s)?))
                }
                Some("$map") => {
                    let obj = o
                        .get("$map")
                        .and_then(|value| value.as_object())
                        .ok_or_else(|| ValueError::InvalidTag("$map".to_string()))?;
                    reject_reserved_keys(obj, "$map")?;
                    let mut m = BTreeMap::new();
                    for (k, val) in obj {
                        m.insert(k.clone(), decode(val, strict)?);
                    }
                    Ok(Value::Map(m))
                }
                Some("$variant") => {
                    let obj = o
                        .get("$variant")
                        .and_then(|value| value.as_object())
                        .ok_or_else(|| ValueError::InvalidTag("$variant".to_string()))?;
                    reject_reserved_keys(obj, "$variant")?;
                    let case = obj
                        .get("case")
                        .and_then(|x| x.as_str())
                        .ok_or_else(|| ValueError::InvalidTag("$variant.case".to_string()))?
                        .to_string();
                    let value = obj
                        .get("value")
                        .ok_or_else(|| ValueError::InvalidTag("$variant.value".to_string()))?;
                    Ok(Value::Variant {
                        case,
                        value: Box::new(decode(value, strict)?),
                    })
                }
                Some("$some") => Ok(Value::Option(Some(Box::new(decode(
                    o.get("$some").expect("tag present"),
                    strict,
                )?)))),
                Some("$ok") => Ok(Value::ResultOk(Box::new(decode(
                    o.get("$ok").expect("tag present"),
                    strict,
                )?))),
                Some("$err") => Ok(Value::ResultErr(Box::new(decode(
                    o.get("$err").expect("tag present"),
                    strict,
                )?))),
                Some("$handle") => {
                    let obj = o
                        .get("$handle")
                        .and_then(|value| value.as_object())
                        .ok_or_else(|| ValueError::InvalidTag("$handle".to_string()))?;
                    reject_reserved_keys(obj, "$handle")?;
                    Ok(Value::Handle {
                        id: obj
                            .get("id")
                            .and_then(|x| x.as_str())
                            .unwrap_or("")
                            .to_string(),
                        resource: obj
                            .get("resource")
                            .and_then(|x| x.as_str())
                            .unwrap_or("")
                            .to_string(),
                        borrow: obj.get("borrow").and_then(|x| x.as_bool()).unwrap_or(false),
                    })
                }
                Some("$fn") => Ok(Value::Fn(
                    o.get("$fn")
                        .and_then(|value| value.as_str())
                        .ok_or_else(|| ValueError::InvalidTag("$fn".to_string()))?
                        .to_string(),
                )),
                None => {
                    reject_reserved_keys(o, "record")?;
                    let mut m = BTreeMap::new();
                    for (k, val) in o {
                        m.insert(k.clone(), decode(val, strict)?);
                    }
                    Ok(Value::Record(m))
                }
                Some(_) => unreachable!("single_reserved_tag only returns known tags"),
            }
        }
    }
}

/// Source-literal style chosen by a Lang Pack's `inline.toml`.
///
/// JUDGMENT CALL: the core knows a small fixed set of escaping styles, but the
/// pack chooses the style. That keeps inline rendering pack-owned without
/// forcing the core to parse arbitrary template languages. A new language can
/// reuse an existing style or add one in a later core release.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceStyle {
    Python,
    JavaScript,
    Json,
    Lua,
}

impl SourceStyle {
    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "python" | "py" => Some(Self::Python),
            "javascript" | "js" | "node" => Some(Self::JavaScript),
            "json" => Some(Self::Json),
            "lua" => Some(Self::Lua),
            _ => None,
        }
    }
}

impl Value {
    /// Render a decoded Meld Value as a source literal for the *containing*
    /// language. Inline regions are evaluated in their own language first; the
    /// resulting Value is rendered here so the outer region remains valid.
    pub fn to_source_literal(&self, style: SourceStyle) -> Result<String, ValueError> {
        use SourceStyle::*;
        match self {
            Value::Null => Ok(match style {
                Python => "None".to_string(),
                JavaScript | Json => "null".to_string(),
                Lua => "nil".to_string(),
            }),
            Value::Bool(b) => Ok(match (style, b) {
                (Python, true) => "True".to_string(),
                (Python, false) => "False".to_string(),
                (_, true) => "true".to_string(),
                (_, false) => "false".to_string(),
            }),
            Value::Int(s) => Ok(s.clone()),
            Value::Float(f) => {
                if !f.is_finite() {
                    return Err(ValueError::Unrenderable(
                        "NaN and infinity need a language-specific literal".to_string(),
                    ));
                }
                let mut s = f.to_string();
                if !s.contains('.') && !s.contains('e') && !s.contains('E') {
                    s.push_str(".0");
                }
                Ok(s)
            }
            Value::String(s) => Ok(quote_string(s, style)),
            Value::Bytes(bytes) if style == Python => Ok(format!("b{}", quote_bytes(bytes))),
            Value::Bytes(_) => Err(ValueError::Unrenderable(
                "bytes literal is only configured for Python".to_string(),
            )),
            Value::List(xs) => {
                let parts = xs
                    .iter()
                    .map(|x| x.to_source_literal(style))
                    .collect::<Result<Vec<_>, _>>()?;
                match style {
                    Lua => Ok(format!("{{{}}}", parts.join(", "))),
                    _ => Ok(format!("[{}]", parts.join(", "))),
                }
            }
            Value::Map(m) | Value::Record(m) => {
                let parts = m
                    .iter()
                    .map(|(k, v)| {
                        let key = quote_string(k, style);
                        let value = v.to_source_literal(style)?;
                        Ok(match style {
                            Lua => format!("[{key}] = {value}"),
                            _ => format!("{key}: {value}"),
                        })
                    })
                    .collect::<Result<Vec<_>, ValueError>>()?;
                Ok(format!("{{{}}}", parts.join(", ")))
            }
            Value::Variant { .. }
            | Value::Option(_)
            | Value::ResultOk(_)
            | Value::ResultErr(_)
            | Value::Handle { .. }
            | Value::Fn(_) => Err(ValueError::Unrenderable(format!(
                "tagged/opaque value {self:?} has no universal source literal"
            ))),
        }
    }
}

fn quote_string(s: &str, style: SourceStyle) -> String {
    match style {
        SourceStyle::JavaScript | SourceStyle::Json => {
            serde_json::to_string(s).expect("string serialization cannot fail")
        }
        SourceStyle::Python => {
            let mut out = String::from("'");
            for ch in s.chars() {
                match ch {
                    '\\' => out.push_str("\\\\"),
                    '\'' => out.push_str("\\'"),
                    '\n' => out.push_str("\\n"),
                    '\r' => out.push_str("\\r"),
                    '\t' => out.push_str("\\t"),
                    '\u{08}' => out.push_str("\\x08"),
                    '\u{0c}' => out.push_str("\\x0c"),
                    c if c.is_control() && (c as u32) <= 0xff => {
                        out.push_str(&format!("\\x{:02x}", c as u32));
                    }
                    c if c.is_control() => out.push_str(&format!("\\u{:04x}", c as u32)),
                    c => out.push(c),
                }
            }
            out.push('\'');
            out
        }
        SourceStyle::Lua => {
            let mut out = String::from("\"");
            for ch in s.chars() {
                match ch {
                    '\\' => out.push_str("\\\\"),
                    '"' => out.push_str("\\\""),
                    '\n' => out.push_str("\\n"),
                    '\r' => out.push_str("\\r"),
                    '\t' => out.push_str("\\t"),
                    '\u{08}' => out.push_str("\\b"),
                    '\u{0c}' => out.push_str("\\f"),
                    c if c.is_control() && (c as u32) <= 0xff => {
                        out.push_str(&format!("\\{:03}", c as u32));
                    }
                    c if c.is_control() => out.push_str(&format!("\\u{{{:x}}}", c as u32)),
                    c => out.push(c),
                }
            }
            out.push('"');
            out
        }
    }
}

fn quote_bytes(bytes: &[u8]) -> String {
    let mut out = String::from("'");
    for b in bytes {
        match b {
            b'\\' => out.push_str("\\\\"),
            b'\'' => out.push_str("\\'"),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            b'\t' => out.push_str("\\t"),
            0x20..=0x7e => out.push(*b as char),
            _ => out.push_str(&format!("\\x{b:02x}")),
        }
    }
    out.push('\'');
    out
}

fn is_decimal(s: &str) -> bool {
    let s = s.strip_prefix('-').unwrap_or(s);
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

fn base64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = (chunk[0] as u32) << 16
            | (chunk.get(1).copied().unwrap_or(0) as u32) << 8
            | chunk.get(2).copied().unwrap_or(0) as u32;
        out.push(T[((n >> 18) & 63) as usize] as char);
        out.push(T[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            T[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            T[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

fn base64_value(c: u8) -> Option<u8> {
    match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

/// Decode canonical padded base64. Rejects non-alphabet bytes, whitespace,
/// missing/excess padding, `=` outside the final two positions, and non-zero
/// trailing bits. This matches the base64 emitted by [`base64`] and by the
/// bundled Python/JavaScript/Lua marshalers.
fn unbase64(s: &str) -> Result<Vec<u8>, ValueError> {
    let invalid = || ValueError::InvalidTag("$bytes".to_string());
    let bytes = s.as_bytes();
    if !bytes.len().is_multiple_of(4) {
        return Err(invalid());
    }
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    for chunk in bytes.as_chunks::<4>().0 {
        let mut values = [0u8; 4];
        let mut data = 0usize;
        let mut padding = 0usize;
        for (i, &c) in chunk.iter().enumerate() {
            if c == b'=' {
                if i < 2 {
                    return Err(invalid());
                }
                padding += 1;
                continue;
            }
            if padding > 0 {
                return Err(invalid());
            }
            values[data] = base64_value(c).ok_or_else(invalid)?;
            data += 1;
        }
        let n = (u32::from(values[0]) << 18)
            | (u32::from(values[1]) << 12)
            | (u32::from(values[2]) << 6)
            | u32::from(values[3]);
        match (data, padding) {
            (4, 0) => {
                out.push((n >> 16) as u8);
                out.push((n >> 8) as u8);
                out.push(n as u8);
            }
            (3, 1) if values[2] & 0b11 == 0 => {
                out.push((n >> 16) as u8);
                out.push((n >> 8) as u8);
            }
            (2, 2) if values[1] & 0b1111 == 0 => out.push((n >> 16) as u8),
            _ => return Err(invalid()),
        }
    }
    Ok(out)
}
