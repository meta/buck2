/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! JSON as Python's `json` module reads and writes it. The harness's `serde_json` sorts the keys of
//! objects; [`Json`] keeps them in order, as a Python `dict` does, and [`Json::dumps`] formats as
//! `json.dumps` does, so golden files written by Python tests still match.

use std::fmt;
use std::fmt::Write;

use serde::Deserialize;
use serde::de::MapAccess;
use serde::de::SeqAccess;
use serde::de::Visitor;

/// A JSON value whose objects keep their keys in order.
#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    /// Printed as `serde_json` prints it, which differs from Python's `repr` for floats between
    /// 1e-5 and 1e-4 (`0.00001` against `1e-05`), integers beyond 64 bits, and `NaN`/`Infinity`.
    Number(serde_json::Number),
    String(String),
    Array(Vec<Json>),
    Object(Vec<(String, Json)>),
}

impl Json {
    /// Parses JSON text, as `json.loads` does.
    pub fn loads(text: &str) -> anyhow::Result<Json> {
        Ok(serde_json::from_str(text)?)
    }

    /// The value of `key`, if this is an object that has it.
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Object(entries) => entries.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// The value of `key`, if this is an object that has it.
    pub fn get_mut(&mut self, key: &str) -> Option<&mut Json> {
        match self {
            Json::Object(entries) => entries.iter_mut().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// Sets `key` of an object: in place if it is there, at the end if not, as in a Python `dict`.
    /// Panics if this is not an object.
    #[track_caller]
    pub fn set(&mut self, key: &str, value: impl Into<Json>) {
        let Json::Object(entries) = self else {
            panic!("Setting `{key}` of {self}, which is not an object");
        };
        let value = value.into();
        match entries.iter_mut().find(|(k, _)| k == key) {
            Some((_, v)) => *v = value,
            None => entries.push((key.to_owned(), value)),
        }
    }

    /// The keys of an object, in order; none for anything else.
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        let entries: &[(String, Json)] = match self {
            Json::Object(entries) => entries,
            _ => &[],
        };
        entries.iter().map(|(k, _)| k.as_str())
    }

    /// The value as `json.dumps(value, indent=indent, sort_keys=sort_keys)` writes it: everything
    /// outside ASCII escaped, and `, ` between items when there is no indentation.
    pub fn dumps(&self, indent: Option<usize>, sort_keys: bool) -> String {
        let mut out = String::new();
        self.write(&mut out, indent, sort_keys, 0);
        out
    }

    fn write(&self, out: &mut String, indent: Option<usize>, sort_keys: bool, depth: usize) {
        let newline = |out: &mut String, depth: usize| {
            if let Some(indent) = indent {
                out.push('\n');
                out.push_str(&" ".repeat(indent * depth));
            }
        };
        let separator = if indent.is_some() { "," } else { ", " };
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Json::Number(n) => write!(out, "{n}").expect("writing to a String"),
            Json::String(s) => write_string(out, s),
            Json::Array(items) if items.is_empty() => out.push_str("[]"),
            Json::Array(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push_str(separator);
                    }
                    newline(out, depth + 1);
                    item.write(out, indent, sort_keys, depth + 1);
                }
                newline(out, depth);
                out.push(']');
            }
            Json::Object(entries) if entries.is_empty() => out.push_str("{}"),
            Json::Object(entries) => {
                let mut entries: Vec<&(String, Json)> = entries.iter().collect();
                if sort_keys {
                    entries.sort_by(|(a, _), (b, _)| a.cmp(b));
                }
                out.push('{');
                for (i, (key, value)) in entries.into_iter().enumerate() {
                    if i > 0 {
                        out.push_str(separator);
                    }
                    newline(out, depth + 1);
                    write_string(out, key);
                    out.push_str(": ");
                    value.write(out, indent, sort_keys, depth + 1);
                }
                newline(out, depth);
                out.push('}');
            }
        }
    }
}

/// As `json.dumps(value)`.
impl fmt::Display for Json {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.dumps(None, false))
    }
}

impl From<&str> for Json {
    fn from(s: &str) -> Json {
        Json::String(s.to_owned())
    }
}

impl From<String> for Json {
    fn from(s: String) -> Json {
        Json::String(s)
    }
}

impl From<bool> for Json {
    fn from(b: bool) -> Json {
        Json::Bool(b)
    }
}

impl<T: Into<Json>> From<Vec<T>> for Json {
    fn from(items: Vec<T>) -> Json {
        Json::Array(items.into_iter().map(Into::into).collect())
    }
}

impl From<Json> for serde_json::Value {
    /// The keys of objects end up sorted.
    fn from(json: Json) -> serde_json::Value {
        match json {
            Json::Null => serde_json::Value::Null,
            Json::Bool(b) => serde_json::Value::Bool(b),
            Json::Number(n) => serde_json::Value::Number(n),
            Json::String(s) => serde_json::Value::String(s),
            Json::Array(items) => items.into_iter().map(serde_json::Value::from).collect(),
            Json::Object(entries) => entries
                .into_iter()
                .map(|(k, v)| (k, serde_json::Value::from(v)))
                .collect::<serde_json::Map<_, _>>()
                .into(),
        }
    }
}

impl From<serde_json::Value> for Json {
    fn from(value: serde_json::Value) -> Json {
        match value {
            serde_json::Value::Null => Json::Null,
            serde_json::Value::Bool(b) => Json::Bool(b),
            serde_json::Value::Number(n) => Json::Number(n),
            serde_json::Value::String(s) => Json::String(s),
            serde_json::Value::Array(items) => {
                Json::Array(items.into_iter().map(Json::from).collect())
            }
            serde_json::Value::Object(map) => {
                Json::Object(map.into_iter().map(|(k, v)| (k, Json::from(v))).collect())
            }
        }
    }
}

/// A string as `json.dumps` writes it with `ensure_ascii`.
fn write_string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            ' '..='~' => out.push(c),
            _ => {
                let mut units = [0; 2];
                for unit in c.encode_utf16(&mut units) {
                    write!(out, "\\u{unit:04x}").expect("writing to a String");
                }
            }
        }
    }
    out.push('"');
}

impl<'de> Deserialize<'de> for Json {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct JsonVisitor;

        impl<'de> Visitor<'de> for JsonVisitor {
            type Value = Json;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a JSON value")
            }

            fn visit_unit<E>(self) -> Result<Json, E> {
                Ok(Json::Null)
            }

            fn visit_bool<E>(self, b: bool) -> Result<Json, E> {
                Ok(Json::Bool(b))
            }

            fn visit_i64<E>(self, n: i64) -> Result<Json, E> {
                Ok(Json::Number(n.into()))
            }

            fn visit_u64<E>(self, n: u64) -> Result<Json, E> {
                Ok(Json::Number(n.into()))
            }

            fn visit_f64<E: serde::de::Error>(self, n: f64) -> Result<Json, E> {
                serde_json::Number::from_f64(n)
                    .map(Json::Number)
                    .ok_or_else(|| E::custom("a non-finite number"))
            }

            fn visit_str<E>(self, s: &str) -> Result<Json, E> {
                Ok(Json::String(s.to_owned()))
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Json, A::Error> {
                let mut items = Vec::new();
                while let Some(item) = seq.next_element()? {
                    items.push(item);
                }
                Ok(Json::Array(items))
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Json, A::Error> {
                let mut object = Json::Object(Vec::new());
                // A repeated key keeps its first position and its last value, as in a Python `dict`.
                while let Some((key, value)) = map.next_entry::<String, Json>()? {
                    object.set(&key, value);
                }
                Ok(object)
            }
        }

        deserializer.deserialize_any(JsonVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_keys_keep_their_order() -> anyhow::Result<()> {
        let json = Json::loads(r#"{"z": 1, "a": [true, null], "m": {"y": "x", "b": 2.5}}"#)?;
        assert_eq!(json.keys().collect::<Vec<_>>(), ["z", "a", "m"]);
        assert_eq!(
            json.to_string(),
            r#"{"z": 1, "a": [true, null], "m": {"y": "x", "b": 2.5}}"#
        );
        Ok(())
    }

    #[test]
    fn test_dumps_indents_and_sorts_as_python() -> anyhow::Result<()> {
        let json = Json::loads(r#"{"b": [1, {}], "a": [], "c": {"é": "\u2603"}}"#)?;
        // json.dumps(json.loads(...), indent=2, sort_keys=True)
        assert_eq!(
            json.dumps(Some(2), true),
            "{\n  \"a\": [],\n  \"b\": [\n    1,\n    {}\n  ],\n  \"c\": {\n    \"\\u00e9\": \"\\u2603\"\n  }\n}"
        );
        Ok(())
    }

    #[test]
    fn test_strings_escape_as_python() {
        // json.dumps("q\"\\\n\t\x01\x7f😀")
        assert_eq!(
            Json::from("q\"\\\n\t\u{1}\u{7f}😀").to_string(),
            r#""q\"\\\n\t\u0001\u007f\ud83d\ude00""#
        );
    }

    #[test]
    fn test_set_replaces_in_place_and_appends() -> anyhow::Result<()> {
        let mut json = Json::loads(r#"{"a": 1, "b": 2, "a": 3}"#)?;
        json.set("a", "x");
        json.set("c", true);
        assert_eq!(json.to_string(), r#"{"a": "x", "b": 2, "c": true}"#);
        assert_eq!(json.get("b"), Some(&Json::Number(2.into())));
        assert_eq!(json.get("missing"), None);
        Ok(())
    }
}
