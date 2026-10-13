// SPDX-License-Identifier: AGPL-3.0-only

//! JSON as the tool reads and writes it. Reading is strict: a key may appear once in an object
//! (parsers disagree about which copy wins), numbers are non-negative integers, and every reader
//! names the fields it knows and refuses the rest. Writing is by hand, in a fixed field order
//! without spaces, so that the same policy always gives the same bytes.

use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Json {
    Null,
    Bool(bool),
    Int(u64),
    Str(String),
    Array(Vec<Json>),
    Object(Vec<(String, Json)>),
}

struct JsonVisitor;

impl<'de> Visitor<'de> for JsonVisitor {
    type Value = Json;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("JSON with unique keys and non-negative integers")
    }

    fn visit_unit<E>(self) -> Result<Json, E> {
        Ok(Json::Null)
    }

    fn visit_bool<E>(self, v: bool) -> Result<Json, E> {
        Ok(Json::Bool(v))
    }

    fn visit_u64<E>(self, v: u64) -> Result<Json, E> {
        Ok(Json::Int(v))
    }

    fn visit_i64<E: de::Error>(self, v: i64) -> Result<Json, E> {
        u64::try_from(v)
            .map(Json::Int)
            .map_err(|_| E::custom("a negative number"))
    }

    fn visit_f64<E: de::Error>(self, _: f64) -> Result<Json, E> {
        Err(E::custom(
            "a number that is not a plain non-negative integer",
        ))
    }

    fn visit_str<E>(self, v: &str) -> Result<Json, E> {
        Ok(Json::Str(v.to_owned()))
    }

    fn visit_string<E>(self, v: String) -> Result<Json, E> {
        Ok(Json::Str(v))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Json, A::Error> {
        let mut items = Vec::new();
        while let Some(item) = seq.next_element()? {
            items.push(item);
        }
        Ok(Json::Array(items))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Json, A::Error> {
        let mut fields: Vec<(String, Json)> = Vec::new();
        while let Some((key, value)) = map.next_entry::<String, Json>()? {
            if fields.iter().any(|(k, _)| *k == key) {
                return Err(de::Error::custom(format!("the key {key:?} appears twice")));
            }
            fields.push((key, value));
        }
        Ok(Json::Object(fields))
    }
}

impl<'de> Deserialize<'de> for Json {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Json, D::Error> {
        d.deserialize_any(JsonVisitor)
    }
}

impl Json {
    /// Parses one JSON value; only whitespace may follow it.
    pub fn parse(text: &str) -> Result<Json, String> {
        serde_json::from_str(text).map_err(|e| e.to_string())
    }

    /// The fields of an object, for a reader that takes each by name.
    pub fn fields<'a>(&'a self, what: &'a str) -> Result<Fields<'a>, String> {
        match self {
            Json::Object(fields) => Ok(Fields {
                what,
                left: fields.iter().map(|(k, v)| (k.as_str(), v)).collect(),
            }),
            _ => Err(format!("{what}: JSON 객체가 아닙니다")),
        }
    }
}

/// An object whose fields are taken one by one; [`Fields::done`] refuses what was not taken.
pub struct Fields<'a> {
    what: &'a str,
    left: Vec<(&'a str, &'a Json)>,
}

impl<'a> Fields<'a> {
    pub fn take(&mut self, name: &str) -> Option<&'a Json> {
        let at = self.left.iter().position(|(k, _)| *k == name)?;
        Some(self.left.remove(at).1)
    }

    fn need(&mut self, name: &str) -> Result<&'a Json, String> {
        self.take(name)
            .ok_or_else(|| format!("{}: {name} 필드가 없습니다", self.what))
    }

    fn wrong(&self, name: &str, wanted: &str) -> String {
        format!("{}: {name} 필드에는 {wanted}만 올 수 있습니다", self.what)
    }

    pub fn int(&mut self, name: &str) -> Result<u64, String> {
        match self.need(name)? {
            Json::Int(n) => Ok(*n),
            _ => Err(self.wrong(name, "0 이상의 정수")),
        }
    }

    pub fn str(&mut self, name: &str) -> Result<&'a str, String> {
        match self.need(name)? {
            Json::Str(s) => Ok(s),
            _ => Err(self.wrong(name, "문자열")),
        }
    }

    pub fn opt_int(&mut self, name: &str) -> Result<Option<u64>, String> {
        match self.take(name) {
            None => Ok(None),
            Some(Json::Int(n)) => Ok(Some(*n)),
            Some(_) => Err(self.wrong(name, "0 이상의 정수")),
        }
    }

    pub fn opt_str(&mut self, name: &str) -> Result<Option<&'a str>, String> {
        match self.take(name) {
            None => Ok(None),
            Some(Json::Str(s)) => Ok(Some(s)),
            Some(_) => Err(self.wrong(name, "문자열")),
        }
    }

    pub fn opt_bool(&mut self, name: &str) -> Result<Option<bool>, String> {
        match self.take(name) {
            None => Ok(None),
            Some(Json::Bool(b)) => Ok(Some(*b)),
            Some(_) => Err(self.wrong(name, "true 또는 false")),
        }
    }

    pub fn object(&mut self, name: &str, what: &'a str) -> Result<Fields<'a>, String> {
        self.need(name)?.fields(what)
    }

    pub fn opt_object(&mut self, name: &str, what: &'a str) -> Result<Option<Fields<'a>>, String> {
        self.take(name).map(|v| v.fields(what)).transpose()
    }

    /// Ends the reading: a field nobody took is an error.
    pub fn done(self) -> Result<(), String> {
        match self.left.first() {
            None => Ok(()),
            Some((name, _)) => Err(format!("{}: 모르는 필드 {name:?}", self.what)),
        }
    }
}

/// A JSON string literal: quotes, backslashes and control characters escaped, everything else
/// (Korean text included) as it is.
pub fn quote(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| String::from("\"\""))
}

/// Writes an object field by field, in the order given.
pub struct Writer {
    out: String,
    first: bool,
}

impl Default for Writer {
    fn default() -> Self {
        Writer::new()
    }
}

impl Writer {
    pub fn new() -> Writer {
        Writer {
            out: String::from("{"),
            first: true,
        }
    }

    fn key(&mut self, name: &str) {
        if !self.first {
            self.out.push(',');
        }
        self.first = false;
        self.out.push_str(&quote(name));
        self.out.push(':');
    }

    pub fn int(&mut self, name: &str, value: u64) -> &mut Writer {
        self.key(name);
        self.out.push_str(&value.to_string());
        self
    }

    pub fn str(&mut self, name: &str, value: &str) -> &mut Writer {
        self.key(name);
        self.out.push_str(&quote(value));
        self
    }

    pub fn bool(&mut self, name: &str, value: bool) -> &mut Writer {
        self.key(name);
        self.out.push_str(if value { "true" } else { "false" });
        self
    }

    /// A field whose value is JSON text already (an object or an array).
    pub fn raw(&mut self, name: &str, json: &str) -> &mut Writer {
        self.key(name);
        self.out.push_str(json);
        self
    }

    pub fn opt_str(&mut self, name: &str, value: Option<&str>) -> &mut Writer {
        if let Some(value) = value {
            self.str(name, value);
        }
        self
    }

    pub fn finish(&mut self) -> String {
        let mut out = std::mem::take(&mut self.out);
        out.push('}');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strict_reading() {
        let v = Json::parse(r#" {"a":1,"b":"x","c":[true,null],"d":{"e":0}} "#).unwrap();
        let mut f = v.fields("test").unwrap();
        assert_eq!(f.int("a").unwrap(), 1);
        assert_eq!(f.str("b").unwrap(), "x");
        assert!(f.done().unwrap_err().contains("\"c\""));
        for bad in [
            r#"{"a":1,"a":2}"#,
            r#"{"a":{"b":1,"b":1}}"#,
            r#"[{"a":1,"a":1}]"#,
            r#"{"a":-1}"#,
            r#"{"a":1.0}"#,
            r#"{"a":1e3}"#,
            r#"{"a":18446744073709551616}"#,
            r#"{"a":1} x"#,
            r#"{"a":1}{"a":1}"#,
            "\u{feff}{}",
            r#"{'a':1}"#,
            r#"{"a":1,}"#,
            "",
        ] {
            assert!(Json::parse(bad).is_err(), "{bad:?} parsed");
        }
        // an escaped key is the same key
        assert!(Json::parse(r#"{"a":1,"a":2}"#).is_err());
        assert_eq!(
            Json::parse("18446744073709551615").unwrap(),
            Json::Int(u64::MAX)
        );
    }

    #[test]
    fn field_types() {
        let v = Json::parse(r#"{"n":"1","s":1,"b":"true","o":[]}"#).unwrap();
        assert!(v.fields("t").unwrap().int("n").is_err());
        assert!(v.fields("t").unwrap().str("s").is_err());
        assert!(v.fields("t").unwrap().opt_bool("b").is_err());
        assert!(v.fields("t").unwrap().object("o", "o").is_err());
        assert!(v.fields("t").unwrap().int("missing").is_err());
        assert_eq!(v.fields("t").unwrap().opt_int("missing").unwrap(), None);
        assert!(Json::parse("[]").unwrap().fields("t").is_err());
    }

    #[test]
    fn writing() {
        let mut w = Writer::new();
        w.int("v", 1)
            .str("text", "한글 \"따옴표\" \\ \n 줄")
            .bool("yes", true)
            .opt_str("none", None)
            .raw("o", "{}");
        let text = w.finish();
        assert_eq!(
            text,
            r#"{"v":1,"text":"한글 \"따옴표\" \\ \n 줄","yes":true,"o":{}}"#
        );
        let back = Json::parse(&text).unwrap();
        assert_eq!(
            back.fields("t").unwrap().str("text").unwrap(),
            "한글 \"따옴표\" \\ \n 줄"
        );
        assert_eq!(quote("\u{1}/\u{7f}"), "\"\\u0001/\u{7f}\"");
        assert_eq!(Writer::new().finish(), "{}");
    }
}
