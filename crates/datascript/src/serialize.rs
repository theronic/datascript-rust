//! `datascript.serialize`: a database as a structure of arrays, numbers and strings that JSON, or any format like
//! it, carries; and back.
//!
//! ```text
//! count    :: number
//! tx0      :: number
//! max-eid  :: number
//! max-tx   :: number
//! schema   :: freezed :schema
//! attrs    :: [keywords ...]
//! keywords :: [keywords ...]
//! eavt     :: [[e a-idx v dtx] ...]
//! a-idx    :: index in attrs
//! v        :: (string | number | boolean | [0 <index in keywords>] | [1 <freezed v>])
//! dtx      :: tx - tx0
//! aevt     :: [<index in eavt> ...]
//! avet     :: [<index in eavt> ...]
//! ```

use crate::datom::{attr_value, value_attr, Datom, Index, TX0};
use crate::db::Db;
use crate::error::{Error, Result};
use crate::print::{number_to_string, pr_str, str_of};
use crate::schema::Schema;
use crate::value::Value;
use crate::{message, raise};
use std::cmp::Ordering;
use std::sync::Arc;

const MARKER_KW: f64 = 0.0;
const MARKER_OTHER: f64 = 1.0;
const MARKER_INF: f64 = 2.0;
const MARKER_MINUS_INF: f64 = 3.0;
const MARKER_NAN: f64 = 4.0;

/// JavaScript's plain data: what `serializable` answers with, and JSON writes.
#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Array(Vec<Json>),
    /// An object, its keys in order
    Object(Vec<(String, Json)>),
}

impl Json {
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Object(entries) => entries.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Json]> {
        match self {
            Json::Array(items) => Some(items),
            _ => None,
        }
    }

    pub fn as_num(&self) -> Option<f64> {
        match self {
            Json::Num(n) => Some(*n),
            _ => None,
        }
    }

    /// The value as the host's data: an array a vector, an object a map of strings.
    pub fn to_value(&self) -> Value {
        match self {
            Json::Null => Value::Nil,
            Json::Bool(b) => Value::Bool(*b),
            Json::Num(n) => Value::Num(*n),
            Json::Str(s) => Value::str(s),
            Json::Array(items) => Value::vector(items.iter().map(Json::to_value).collect()),
            Json::Object(entries) => Value::map(entries.iter().map(|(k, v)| (Value::str(k), v.to_value())).collect()),
        }
    }

    /// The host's data as JavaScript's: a vector or list an array, a map an object. What JavaScript has no plain
    /// form for is its printed form.
    pub fn from_value(v: &Value) -> Json {
        match v {
            Value::Nil => Json::Null,
            Value::Bool(b) => Json::Bool(*b),
            Value::Num(n) => Json::Num(*n),
            Value::Str(s) => Json::Str(s.to_string()),
            Value::Vector(items) | Value::List(items) => Json::Array(items.iter().map(Json::from_value).collect()),
            Value::Map(m) => Json::Object(m.iter().map(|(k, v)| (str_of(k), Json::from_value(v))).collect()),
            other => Json::Str(pr_str(other)),
        }
    }

    /// As ClojureScript prints JavaScript's data: `#js [1 2]`, `#js {:key 1}`.
    pub fn pr_str(&self) -> String {
        match self {
            Json::Array(items) => {
                format!("#js [{}]", items.iter().map(Json::pr_str).collect::<Vec<_>>().join(" "))
            }
            Json::Object(entries) => {
                let simple = |k: &str| {
                    let mut chars = k.chars();
                    let first = |c: char| c.is_ascii_alphabetic() || "_*+?!-'".contains(c);
                    chars.next().is_some_and(first) && chars.all(|c| c.is_ascii_alphanumeric() || "_*+?!-'".contains(c))
                };
                let entry = |(k, v): &(String, Json)| {
                    let key = if simple(k) { format!(":{k}") } else { pr_str(&Value::str(k)) };
                    format!("{key} {}", v.pr_str())
                };
                format!("#js {{{}}}", entries.iter().map(entry).collect::<Vec<_>>().join(", "))
            }
            other => pr_str(&other.to_value()),
        }
    }

    /// `JSON.stringify`
    pub fn write(&self, out: &mut String) {
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Json::Num(n) => {
                if n.is_finite() {
                    out.push_str(&number_to_string(if *n == 0.0 { 0.0 } else { *n }))
                } else {
                    out.push_str("null")
                }
            }
            Json::Str(s) => write_string(out, s),
            Json::Array(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    item.write(out);
                }
                out.push(']');
            }
            Json::Object(entries) => {
                out.push('{');
                for (i, (k, v)) in entries.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    write_string(out, k);
                    out.push(':');
                    v.write(out);
                }
                out.push('}');
            }
        }
    }

    pub fn to_json_string(&self) -> String {
        let mut out = String::new();
        self.write(&mut out);
        out
    }

    /// `JSON.parse`
    pub fn parse(text: &str) -> Result<Json> {
        let mut p = JsonParser { chars: text.as_bytes(), text, pos: 0 };
        p.skip_ws();
        let v = p.value(0)?;
        p.skip_ws();
        if p.pos < p.chars.len() {
            return Err(p.error("Unexpected non-whitespace character after JSON"));
        }
        Ok(v)
    }
}

fn write_string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

struct JsonParser<'a> {
    chars: &'a [u8],
    text: &'a str,
    pos: usize,
}

impl JsonParser<'_> {
    fn error(&self, what: &str) -> Error {
        Error::msg(format!("{what} at position {}", self.pos))
    }

    fn skip_ws(&mut self) {
        while self.pos < self.chars.len() && matches!(self.chars[self.pos], b' ' | b'\t' | b'\n' | b'\r') {
            self.pos += 1;
        }
    }

    fn eat(&mut self, c: u8) -> bool {
        if self.chars.get(self.pos) == Some(&c) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn literal(&mut self, word: &str, v: Json) -> Result<Json> {
        if self.text[self.pos..].starts_with(word) {
            self.pos += word.len();
            Ok(v)
        } else {
            Err(self.error("Unexpected token in JSON"))
        }
    }

    fn value(&mut self, depth: usize) -> Result<Json> {
        if depth > 10_000 {
            return Err(self.error("JSON nested too deep"));
        }
        match self.chars.get(self.pos) {
            None => Err(self.error("Unexpected end of JSON input")),
            Some(b'n') => self.literal("null", Json::Null),
            Some(b't') => self.literal("true", Json::Bool(true)),
            Some(b'f') => self.literal("false", Json::Bool(false)),
            Some(b'"') => Ok(Json::Str(self.string()?)),
            Some(b'[') => {
                self.pos += 1;
                let mut items = Vec::new();
                self.skip_ws();
                if self.eat(b']') {
                    return Ok(Json::Array(items));
                }
                loop {
                    self.skip_ws();
                    items.push(self.value(depth + 1)?);
                    self.skip_ws();
                    if self.eat(b']') {
                        return Ok(Json::Array(items));
                    }
                    if !self.eat(b',') {
                        return Err(self.error("Expected ',' or ']' after array element in JSON"));
                    }
                }
            }
            Some(b'{') => {
                self.pos += 1;
                let mut entries: Vec<(String, Json)> = Vec::new();
                self.skip_ws();
                if self.eat(b'}') {
                    return Ok(Json::Object(entries));
                }
                loop {
                    self.skip_ws();
                    if self.chars.get(self.pos) != Some(&b'"') {
                        return Err(self.error("Expected double-quoted property name in JSON"));
                    }
                    let key = self.string()?;
                    self.skip_ws();
                    if !self.eat(b':') {
                        return Err(self.error("Expected ':' after property name in JSON"));
                    }
                    self.skip_ws();
                    let v = self.value(depth + 1)?;
                    // a key given twice has the value given last
                    match entries.iter_mut().find(|(k, _)| *k == key) {
                        Some(entry) => entry.1 = v,
                        None => entries.push((key, v)),
                    }
                    self.skip_ws();
                    if self.eat(b'}') {
                        return Ok(Json::Object(entries));
                    }
                    if !self.eat(b',') {
                        return Err(self.error("Expected ',' or '}' after property value in JSON"));
                    }
                }
            }
            Some(c) if *c == b'-' || c.is_ascii_digit() => {
                let start = self.pos;
                self.eat(b'-');
                while self.pos < self.chars.len()
                    && matches!(self.chars[self.pos], b'0'..=b'9' | b'.' | b'e' | b'E' | b'+' | b'-')
                {
                    self.pos += 1;
                }
                self.text[start..self.pos]
                    .parse::<f64>()
                    .map(Json::Num)
                    .map_err(|_| self.error("No number after minus sign in JSON"))
            }
            Some(_) => Err(self.error("Unexpected token in JSON")),
        }
    }

    fn string(&mut self) -> Result<String> {
        self.pos += 1;
        let mut out = String::new();
        loop {
            let start = self.pos;
            while self.pos < self.chars.len() && !matches!(self.chars[self.pos], b'"' | b'\\') {
                self.pos += 1;
            }
            out.push_str(&self.text[start..self.pos]);
            match self.chars.get(self.pos) {
                None => return Err(self.error("Unterminated string in JSON")),
                Some(b'"') => {
                    self.pos += 1;
                    return Ok(out);
                }
                Some(_) => {
                    self.pos += 1;
                    let c = *self.chars.get(self.pos).ok_or_else(|| self.error("Unterminated string in JSON"))?;
                    self.pos += 1;
                    match c {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let unit = self.hex4()?;
                            // a surrogate pair is one character; half of one is none
                            if (0xD800..0xDC00).contains(&unit) && self.text[self.pos..].starts_with("\\u") {
                                let save = self.pos;
                                self.pos += 2;
                                let low = self.hex4()?;
                                if (0xDC00..0xE000).contains(&low) {
                                    let c = 0x10000 + ((unit - 0xD800) << 10) + (low - 0xDC00);
                                    out.push(char::from_u32(c).unwrap_or('\u{fffd}'));
                                } else {
                                    self.pos = save;
                                    out.push('\u{fffd}');
                                }
                            } else {
                                out.push(char::from_u32(unit).unwrap_or('\u{fffd}'));
                            }
                        }
                        _ => return Err(self.error("Bad escaped character in JSON")),
                    }
                }
            }
        }
    }

    fn hex4(&mut self) -> Result<u32> {
        let digits = self.text.get(self.pos..self.pos + 4).ok_or_else(|| self.error("Bad Unicode escape in JSON"))?;
        let unit = u32::from_str_radix(digits, 16).map_err(|_| self.error("Bad Unicode escape in JSON"))?;
        self.pos += 4;
        Ok(unit)
    }
}

/// How values that JSON has no form for are written and read back, and how attributes and keywords are. The
/// defaults are `pr-str` and reading EDN, and `str` and `keyword`.
#[derive(Default)]
pub struct Options<'a> {
    /// `:freeze-fn`: of a value, what stands for it
    pub freeze_fn: Option<&'a dyn Fn(&Value) -> Result<Json>>,
    /// `:freeze-kw`: of a keyword or attribute, the string that stands for it
    pub freeze_kw: Option<&'a dyn Fn(&Value) -> Result<Json>>,
    /// `:thaw-fn`
    pub thaw_fn: Option<&'a dyn Fn(&Json) -> Result<Value>>,
    /// `:thaw-kw`
    pub thaw_kw: Option<&'a dyn Fn(&Json) -> Result<Value>>,
}

fn freeze_default(v: &Value) -> Result<Json> {
    Ok(Json::Str(pr_str(v)))
}

/// `freeze-kw`: `str`
fn freeze_kw_default(v: &Value) -> Result<Json> {
    Ok(Json::Str(str_of(v)))
}

fn thaw_default(v: &Json) -> Result<Value> {
    match v {
        Json::Str(s) => crate::edn::read_string(s),
        other => Err(Error::msg(format!("Cannot read {}: not a string", other.to_json_string()))),
    }
}

/// `thaw-kw`: a string that starts with a colon is a keyword
fn thaw_kw_default(v: &Json) -> Result<Value> {
    match v {
        Json::Str(s) => Ok(match s.strip_prefix(':') {
            Some(k) => Value::kw(k),
            None => Value::str(s),
        }),
        other => Err(Error::msg(format!("{}.startsWith is not a function", other.to_json_string()))),
    }
}

/// `(d/serializable db opts)`
pub fn serializable(db: &Db, opts: &Options) -> Result<Json> {
    if db.is_filtered() {
        return Err(Error::msg("-lookup is not supported on FilteredDB"));
    }
    let freeze = |v: &Value| match opts.freeze_fn {
        Some(f) => f(v),
        None => freeze_default(v),
    };
    let freeze_kw = |v: &Value| match opts.freeze_kw {
        Some(f) => f(v),
        None => freeze_kw_default(v),
    };
    let eavt = db.index(Index::Eavt).to_vec()?;
    let aevt = db.index(Index::Aevt).to_vec()?;
    let avet = db.index(Index::Avet).to_vec()?;

    // all-attrs: the attributes, each once, in order
    let mut attrs: Vec<Value> = Vec::new();
    for d in &aevt {
        let a = d.a_value();
        if attrs.last() != Some(&a) {
            attrs.push(a);
        }
    }
    let attr_index = |d: &Datom| -> f64 {
        let a = d.a_value();
        attrs.iter().position(|x| *x == a).map_or(f64::NAN, |i| i as f64)
    };

    let mut kws: Vec<Value> = Vec::new();
    let mut kw_index: std::collections::HashMap<Value, usize> = std::collections::HashMap::new();
    let mut rows = Vec::with_capacity(eavt.len());
    for d in &eavt {
        let v = match &d.v {
            Value::Str(s) => Json::Str(s.to_string()),
            Value::Num(n) if *n == f64::INFINITY => Json::Array(vec![Json::Num(MARKER_INF)]),
            Value::Num(n) if *n == f64::NEG_INFINITY => Json::Array(vec![Json::Num(MARKER_MINUS_INF)]),
            Value::Num(n) if n.is_nan() => Json::Array(vec![Json::Num(MARKER_NAN)]),
            Value::Num(n) => Json::Num(*n),
            Value::Bool(b) => Json::Bool(*b),
            Value::Keyword(_) => {
                let idx = *kw_index.entry(d.v.clone()).or_insert_with(|| {
                    kws.push(d.v.clone());
                    kws.len() - 1
                });
                Json::Array(vec![Json::Num(MARKER_KW), Json::Num(idx as f64)])
            }
            other => Json::Array(vec![Json::Num(MARKER_OTHER), freeze(other)?]),
        };
        rows.push(Json::Array(vec![
            Json::Num(d.e as f64),
            Json::Num(attr_index(d)),
            v,
            Json::Num((d.tx() - TX0) as f64),
        ]));
    }
    // where each datom is in EAVT: by what it is, since its order among values that do not order is not to be
    // searched by
    let mut places: std::collections::HashMap<(i32, Value, Value), usize> =
        std::collections::HashMap::with_capacity(eavt.len());
    for (i, d) in eavt.iter().enumerate() {
        places.insert((d.e, d.a_value(), d.v.clone()), i);
    }
    let position = |d: &Datom| -> Json {
        let found = places.get(&(d.e, d.a_value(), d.v.clone())).copied().or_else(|| {
            // a value that equals nothing, itself included: NaN
            eavt.iter().position(|x| Index::Eavt.cmp(x, d) == Ordering::Equal)
        });
        Json::Num(found.map_or(f64::NAN, |i| i as f64))
    };
    let schema = freeze(&db.schema_value())?;
    let attrs_frozen = attrs.iter().map(&freeze_kw).collect::<Result<Vec<_>>>()?;
    let kws_frozen = kws.iter().map(&freeze_kw).collect::<Result<Vec<_>>>()?;
    Ok(Json::Object(vec![
        ("count".into(), Json::Num(eavt.len() as f64)),
        ("tx0".into(), Json::Num(TX0 as f64)),
        ("max-eid".into(), Json::Num(db.max_eid() as f64)),
        ("max-tx".into(), Json::Num(db.max_tx() as f64)),
        ("schema".into(), schema),
        ("attrs".into(), Json::Array(attrs_frozen)),
        ("keywords".into(), Json::Array(kws_frozen)),
        ("eavt".into(), Json::Array(rows)),
        ("aevt".into(), Json::Array(aevt.iter().map(&position).collect())),
        ("avet".into(), Json::Array(avet.iter().map(&position).collect())),
    ]))
}

fn member<'a>(from: &'a Json, key: &str) -> Result<&'a Json> {
    from.get(key).ok_or_else(|| Error::msg(format!("Cannot read properties of undefined (reading '{key}')")))
}

fn array<'a>(v: &'a Json, what: &str) -> Result<&'a [Json]> {
    v.as_array().ok_or_else(|| Error::msg(format!("{what} is not iterable")))
}

fn number(v: Option<&Json>, what: &str) -> Result<f64> {
    v.and_then(Json::as_num).ok_or_else(|| Error::msg(format!("{what} is not a number")))
}

/// `(d/from-serializable from opts)`: the database read back. Its indexes are taken in the order they come in.
pub fn from_serializable(from: &Json, opts: &Options) -> Result<Db> {
    let thaw = |v: &Json| match opts.thaw_fn {
        Some(f) => f(v),
        None => thaw_default(v),
    };
    let thaw_kw = |v: &Json| match opts.thaw_kw {
        Some(f) => f(v),
        None => thaw_kw_default(v),
    };
    let tx0 = number(from.get("tx0"), "tx0")?;
    let schema = Arc::new(Schema::new(thaw(member(from, "schema")?)?)?);
    let attrs = array(member(from, "attrs")?, "attrs")?.iter().map(&thaw_kw).collect::<Result<Vec<_>>>()?;
    let keywords = array(member(from, "keywords")?, "keywords")?.iter().map(&thaw_kw).collect::<Result<Vec<_>>>()?;
    let nth = |items: &[Value], i: Option<&Json>| -> Result<Value> {
        let i = number(i, "index")?;
        if i >= 0.0 && i.fract() == 0.0 && (i as usize) < items.len() {
            Ok(items[i as usize].clone())
        } else {
            Err(Error::msg("Index out of bounds"))
        }
    };
    let mut eavt = Vec::new();
    for row in array(member(from, "eavt")?, "eavt")? {
        let row = array(row, "datom")?;
        let e = crate::datom::id_from_num(number(row.first(), "e")?)?;
        let a = nth(&attrs, row.get(1))?;
        let attr = value_attr(&a).ok_or_else(|| Error::msg(message!("Bad attribute ", a)))?;
        let v = match row.get(2) {
            Some(Json::Num(n)) => Value::Num(*n),
            Some(Json::Str(s)) => Value::str(s),
            Some(Json::Bool(b)) => Value::Bool(*b),
            Some(Json::Array(marked)) => {
                let marker = marked.first().and_then(Json::as_num);
                match marker {
                    Some(m) if m == MARKER_KW => nth(&keywords, marked.get(1))?,
                    Some(m) if m == MARKER_OTHER => thaw(marked.get(1).unwrap_or(&Json::Null))?,
                    Some(m) if m == MARKER_INF => Value::Num(f64::INFINITY),
                    Some(m) if m == MARKER_MINUS_INF => Value::Num(f64::NEG_INFINITY),
                    Some(m) if m == MARKER_NAN => Value::Num(f64::NAN),
                    _ => {
                        let v = Json::Array(marked.clone());
                        let marker = marked.first().map_or(Value::Nil, Json::to_value);
                        raise!("Unexpected value marker ", marker, " in ", Value::from(v.pr_str());
                            {"error" => Value::kw("serialize"), "value" => v.to_value()})
                    }
                }
            }
            other => {
                let v = other.cloned().unwrap_or(Json::Null);
                let ty = if v == Json::Null { "nil" } else { "#object[Object]" };
                raise!("Unexpected value type ", ty, " (", Value::from(v.pr_str()), ")";
                    {"error" => Value::kw("serialize"), "value" => v.to_value()})
            }
        };
        let tx = crate::datom::id_from_num(tx0 + number(row.get(3), "tx")?)?;
        eavt.push(Datom::new(e, attr, v, tx));
    }
    let by_index = |key: &str| -> Result<Vec<Datom>> {
        array(member(from, key)?, key)?
            .iter()
            .map(|i| {
                let i = number(Some(i), "index")?;
                if i >= 0.0 && i.fract() == 0.0 && (i as usize) < eavt.len() {
                    Ok(eavt[i as usize].clone())
                } else {
                    Err(Error::msg("Index out of bounds"))
                }
            })
            .collect()
    };
    let aevt = by_index("aevt")?;
    let avet = by_index("avet")?;
    let max_eid = crate::datom::id_from_num(number(from.get("max-eid"), "max-eid")?)?;
    let max_tx = crate::datom::id_from_num(number(from.get("max-tx"), "max-tx")?)?;
    Ok(Db::restore(schema, eavt, aevt, avet, max_eid, max_tx))
}

/// The attribute a serialized database names: for a host that reads the structure itself.
pub fn attr_name(a: &crate::named::Attr) -> String {
    str_of(&attr_value(a))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_round_trip() {
        let text = r#"{"a":[1,2.5,-3,"x\n\"y\"",true,false,null],"b":{"c":[]},"d":"é😀"}"#;
        let v = Json::parse(text).unwrap();
        assert_eq!(v.get("a").and_then(Json::as_array).map(<[Json]>::len), Some(7));
        assert_eq!(v.get("d"), Some(&Json::Str("é😀".into())));
        assert_eq!(Json::parse(&v.to_json_string()).unwrap(), v);
        assert_eq!(Json::Num(-0.0).to_json_string(), "0");
        assert_eq!(Json::Num(1e21).to_json_string(), "1e+21");
        assert!(Json::parse("[1,]").is_err());
        assert!(Json::parse("{\"a\":1} x").is_err());
    }

    #[test]
    fn database_round_trip() {
        let schema =
            crate::edn::read_string("{:aka {:db/cardinality :db.cardinality/many} :age {:db/index true}}").unwrap();
        let tx = crate::edn::read_string(
            r#"[{:db/id 1 :name "Petr" :aka ["Devil" "Tupen"] :age 15 :kw :some/kw :attach {:k [1 2]}} {:db/id 2 :inf ##Inf :ninf ##-Inf :nan ##NaN :b false}]"#,
        )
        .unwrap();
        let db = crate::db_with(&Db::empty(schema).unwrap(), &tx).unwrap();
        let json = serializable(&db, &Options::default()).unwrap();
        let back = from_serializable(&Json::parse(&json.to_json_string()).unwrap(), &Options::default()).unwrap();
        assert_eq!(back.count().unwrap(), db.count().unwrap());
        assert_eq!(back.max_eid(), db.max_eid());
        assert_eq!(back.max_tx(), db.max_tx());
        // NaN equals nothing, itself included: compare what is printed
        assert_eq!(pr_str(&Value::Db(back)), pr_str(&Value::Db(db)));
    }
}
