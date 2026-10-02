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
use crate::named::Attr;
use crate::print::{number_to_string, pr_str, str_of};
use crate::schema::Schema;
use crate::sorted_set::SortedSet;
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
            Json::Num(n) => write_number(out, *n),
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

/// A number as `JSON.stringify` writes it: one JSON has no form for is `null`.
fn write_number(out: &mut String, n: f64) {
    if !n.is_finite() {
        out.push_str("null");
    } else if n.fract() == 0.0 && n.abs() < 1e15 {
        // a whole number, as nearly all of a database's are: its digits, with no string made to hold them
        let mut digits = [0u8; 16];
        let mut at = digits.len();
        let mut left = n.abs() as u64;
        loop {
            at -= 1;
            digits[at] = b'0' + (left % 10) as u8;
            left /= 10;
            if left == 0 {
                break;
            }
        }
        if n < 0.0 {
            out.push('-');
        }
        out.push_str(std::str::from_utf8(&digits[at..]).expect("digits"));
    } else {
        out.push_str(&number_to_string(n));
    }
}

fn write_string(out: &mut String, s: &str) {
    out.push('"');
    // what needs no escape is copied a stretch at a time
    let mut from = 0;
    for (i, b) in s.bytes().enumerate() {
        let escape = match b {
            b'"' => "\\\"",
            b'\\' => "\\\\",
            8 => "\\b",
            12 => "\\f",
            b'\n' => "\\n",
            b'\r' => "\\r",
            b'\t' => "\\t",
            0..=0x1f => "",
            _ => continue,
        };
        out.push_str(&s[from..i]);
        if escape.is_empty() {
            out.push_str(&format!("\\u{b:04x}"));
        } else {
            out.push_str(escape);
        }
        from = i + 1;
    }
    out.push_str(&s[from..]);
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
                self.number().map(Json::Num).ok_or_else(|| self.error("No number after minus sign in JSON"))
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

    /// Goes past a value without making it: `None` of anything `value` would not take, and of the few things it
    /// would that are stricter to say, which are then read by `value` itself.
    fn skip(&mut self, depth: usize) -> Option<()> {
        if depth > 10_000 {
            return None;
        }
        match *self.chars.get(self.pos)? {
            b'n' => self.skip_word("null"),
            b't' => self.skip_word("true"),
            b'f' => self.skip_word("false"),
            b'"' => self.skip_string(),
            b'[' => {
                self.pos += 1;
                self.skip_ws();
                if self.eat(b']') {
                    return Some(());
                }
                loop {
                    self.skip_ws();
                    self.skip(depth + 1)?;
                    self.skip_ws();
                    if self.eat(b']') {
                        return Some(());
                    }
                    if !self.eat(b',') {
                        return None;
                    }
                }
            }
            b'{' => {
                self.pos += 1;
                self.skip_ws();
                if self.eat(b'}') {
                    return Some(());
                }
                loop {
                    self.skip_ws();
                    if self.chars.get(self.pos) != Some(&b'"') {
                        return None;
                    }
                    self.skip_string()?;
                    self.skip_ws();
                    if !self.eat(b':') {
                        return None;
                    }
                    self.skip_ws();
                    self.skip(depth + 1)?;
                    self.skip_ws();
                    if self.eat(b'}') {
                        return Some(());
                    }
                    if !self.eat(b',') {
                        return None;
                    }
                }
            }
            c if c == b'-' || c.is_ascii_digit() => {
                let start = self.pos;
                self.eat(b'-');
                while matches!(self.chars.get(self.pos), Some(b'0'..=b'9' | b'.' | b'e' | b'E' | b'+' | b'-')) {
                    self.pos += 1;
                }
                // what JSON calls a number, which any reader of numbers takes
                let s = &self.chars[start..self.pos];
                let digits = |at: &mut usize| {
                    let from = *at;
                    while s.get(*at).is_some_and(u8::is_ascii_digit) {
                        *at += 1;
                    }
                    *at > from
                };
                let mut at = usize::from(s[0] == b'-');
                if !digits(&mut at) {
                    return None;
                }
                if s.get(at) == Some(&b'.') {
                    at += 1;
                    if !digits(&mut at) {
                        return None;
                    }
                }
                if matches!(s.get(at), Some(b'e' | b'E')) {
                    at += 1;
                    if matches!(s.get(at), Some(b'+' | b'-')) {
                        at += 1;
                    }
                    if !digits(&mut at) {
                        return None;
                    }
                }
                (at == s.len()).then_some(())
            }
            _ => None,
        }
    }

    /// The number that starts here, with a minus sign or a digit: every character a number is written with is taken
    /// as part of it, and `None` is what they do not spell.
    fn number(&mut self) -> Option<f64> {
        let start = self.pos;
        self.pos += 1;
        while matches!(self.chars.get(self.pos), Some(b'0'..=b'9' | b'.' | b'e' | b'E' | b'+' | b'-')) {
            self.pos += 1;
        }
        // a whole number of up to fifteen digits is what its digits come to, as nearly every number here is
        let (negative, digits) = match &self.chars[start..self.pos] {
            [b'-', digits @ ..] => (true, digits),
            digits => (false, digits),
        };
        if !digits.is_empty() && digits.len() <= 15 && digits.iter().all(u8::is_ascii_digit) {
            let n = digits.iter().fold(0u64, |n, d| n * 10 + u64::from(d - b'0')) as f64;
            return Some(if negative { -n } else { n });
        }
        self.text[start..self.pos].parse::<f64>().ok()
    }

    /// A number, where there is one.
    fn plain_number(&mut self) -> Option<f64> {
        if matches!(self.chars.get(self.pos), Some(b'-' | b'0'..=b'9')) {
            self.number()
        } else {
            None
        }
    }

    /// A row of EAVT as nearly every one is, `[e, a, v, tx]` with a value that is a string, a number, true or
    /// false: its parts, with nothing made of the row itself. `None` of any other row, which `value` then reads.
    fn plain_row(&mut self) -> Option<(f64, f64, Value, f64)> {
        let start = self.pos;
        let row = self.plain_row_parts();
        if row.is_none() {
            self.pos = start;
        }
        row
    }

    fn plain_row_parts(&mut self) -> Option<(f64, f64, Value, f64)> {
        let comma = |p: &mut Self| {
            p.skip_ws();
            let found = p.eat(b',');
            p.skip_ws();
            found.then_some(())
        };
        if !self.eat(b'[') {
            return None;
        }
        self.skip_ws();
        let e = self.plain_number()?;
        comma(self)?;
        let a = self.plain_number()?;
        comma(self)?;
        let v = match *self.chars.get(self.pos)? {
            b'"' => {
                // a string with nothing escaped in it is the text between its quotes
                let from = self.pos + 1;
                let mut to = from;
                while !matches!(self.chars.get(to), None | Some(b'"' | b'\\')) {
                    to += 1;
                }
                if self.chars.get(to) == Some(&b'"') {
                    self.pos = to + 1;
                    Value::str(&self.text[from..to])
                } else {
                    Value::str(&self.string().ok()?)
                }
            }
            b't' => {
                self.skip_word("true")?;
                Value::Bool(true)
            }
            b'f' => {
                self.skip_word("false")?;
                Value::Bool(false)
            }
            _ => Value::Num(self.plain_number()?),
        };
        comma(self)?;
        let tx = self.plain_number()?;
        self.skip_ws();
        self.eat(b']').then_some((e, a, v, tx))
    }

    fn skip_word(&mut self, word: &str) -> Option<()> {
        if self.text[self.pos..].starts_with(word) {
            self.pos += word.len();
            Some(())
        } else {
            None
        }
    }

    fn skip_string(&mut self) -> Option<()> {
        self.pos += 1;
        loop {
            while self.pos < self.chars.len() && !matches!(self.chars[self.pos], b'"' | b'\\') {
                self.pos += 1;
            }
            if *self.chars.get(self.pos)? == b'"' {
                self.pos += 1;
                return Some(());
            }
            self.pos += 1;
            let escaped = *self.chars.get(self.pos)?;
            self.pos += 1;
            match escaped {
                b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't' => {}
                b'u' => {
                    u32::from_str_radix(self.text.get(self.pos..self.pos + 4)?, 16).ok()?;
                    self.pos += 4;
                }
                _ => return None,
            }
        }
    }
}

/// Where the value of each member of a JSON text is, when the text is an object: found without a value being made,
/// so that what is long is read a part at a time.
struct Members<'a> {
    text: &'a str,
    at: Vec<(String, usize)>,
}

impl<'a> Members<'a> {
    /// `None` of a text that is no object, or that `Json::parse` would not take: it is `Json::parse` that says why.
    fn of(text: &'a str) -> Option<Members<'a>> {
        let mut p = JsonParser { chars: text.as_bytes(), text, pos: 0 };
        let mut at: Vec<(String, usize)> = Vec::new();
        p.skip_ws();
        if !p.eat(b'{') {
            return None;
        }
        p.skip_ws();
        if !p.eat(b'}') {
            loop {
                p.skip_ws();
                if p.chars.get(p.pos) != Some(&b'"') {
                    return None;
                }
                let key = p.string().ok()?;
                p.skip_ws();
                if !p.eat(b':') {
                    return None;
                }
                p.skip_ws();
                let start = p.pos;
                p.skip(1)?;
                // a key given twice has the value given last
                match at.iter_mut().find(|(k, _)| *k == key) {
                    Some(entry) => entry.1 = start,
                    None => at.push((key, start)),
                }
                p.skip_ws();
                if p.eat(b'}') {
                    break;
                }
                if !p.eat(b',') {
                    return None;
                }
            }
        }
        p.skip_ws();
        (p.pos == p.chars.len()).then_some(Members { text, at })
    }

    fn parser(&self, pos: usize) -> JsonParser<'a> {
        JsonParser { chars: self.text.as_bytes(), text: self.text, pos }
    }

    /// A member's value, made.
    fn get(&self, key: &str) -> Result<Option<Json>> {
        match self.at.iter().find(|(k, _)| k == key) {
            Some((_, start)) => self.parser(*start).value(1).map(Some),
            None => Ok(None),
        }
    }

    /// A member there has to be.
    fn member(&self, key: &str) -> Result<Json> {
        self.get(key)?.ok_or_else(|| Error::msg(format!("Cannot read properties of undefined (reading '{key}')")))
    }

    /// The elements of a member that is an array, each made as it is come to.
    fn elements(&self, key: &str) -> Result<Elements<'a>> {
        let Some((_, start)) = self.at.iter().find(|(k, _)| k == key) else {
            return Err(Error::msg(format!("Cannot read properties of undefined (reading '{key}')")));
        };
        if self.text.as_bytes()[*start] != b'[' {
            return Err(Error::msg(format!("{key} is not iterable")));
        }
        let mut p = self.parser(*start + 1);
        p.skip_ws();
        let more = !p.eat(b']');
        Ok(Elements { p, more })
    }
}

struct Elements<'a> {
    p: JsonParser<'a>,
    more: bool,
}

impl Elements<'_> {
    /// Goes on to the next element, past the one just read.
    fn past(&mut self) {
        self.p.skip_ws();
        if !self.p.eat(b',') {
            self.more = false;
        }
    }

    /// The next element when it is a row as nearly every row is (`JsonParser::plain_row`).
    fn plain_row(&mut self) -> Option<(f64, f64, Value, f64)> {
        if !self.more {
            return None;
        }
        self.p.skip_ws();
        let row = self.p.plain_row()?;
        self.past();
        Some(row)
    }
}

impl Iterator for Elements<'_> {
    type Item = Result<Json>;

    fn next(&mut self) -> Option<Result<Json>> {
        if !self.more {
            return None;
        }
        self.p.skip_ws();
        let v = self.p.value(2);
        self.past();
        Some(v)
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

/// A hasher for keys that are numbers already: an attribute's address, an entity's id, a value's own hash. The
/// standard one guards against keys chosen to collide, which these are not, at several times the cost.
#[derive(Default)]
struct Mixed(u64);

impl Mixed {
    #[inline]
    fn mix(&mut self, n: u64) {
        self.0 = (self.0.rotate_left(5) ^ n).wrapping_mul(0x517c_c1b7_2722_0a95);
    }
}

impl std::hash::Hasher for Mixed {
    fn write(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.mix(u64::from(*b));
        }
    }

    #[inline]
    fn write_usize(&mut self, n: usize) {
        self.mix(n as u64);
    }

    #[inline]
    fn write_i32(&mut self, n: i32) {
        self.mix(n as u32 as u64);
    }

    #[inline]
    fn write_u32(&mut self, n: u32) {
        self.mix(u64::from(n));
    }

    #[inline]
    fn finish(&self) -> u64 {
        self.0
    }
}

type ByNumber<K, V> = std::collections::HashMap<K, V, std::hash::BuildHasherDefault<Mixed>>;

/// A datom's value in a row of EAVT.
enum Cell<'a> {
    Num(f64),
    Str(&'a str),
    Bool(bool),
    /// `[marker]`: a number JSON has no form for
    Marked(f64),
    /// `[0 index]`: a keyword, by its place among the keywords
    Keyword(usize),
    /// `[1 frozen]`: anything else, as `:freeze-fn` made of it
    Other(Json),
}

/// Where the rows of EAVT go as they are made: JavaScript's data, or its text.
trait Rows {
    fn row(&mut self, e: f64, a: f64, v: Cell<'_>, tx: f64);
}

struct Tree(Vec<Json>);

impl Rows for Tree {
    fn row(&mut self, e: f64, a: f64, v: Cell<'_>, tx: f64) {
        let v = match v {
            Cell::Num(n) => Json::Num(n),
            Cell::Str(s) => Json::Str(s.to_string()),
            Cell::Bool(b) => Json::Bool(b),
            Cell::Marked(marker) => Json::Array(vec![Json::Num(marker)]),
            Cell::Keyword(index) => Json::Array(vec![Json::Num(MARKER_KW), Json::Num(index as f64)]),
            Cell::Other(frozen) => Json::Array(vec![Json::Num(MARKER_OTHER), frozen]),
        };
        self.0.push(Json::Array(vec![Json::Num(e), Json::Num(a), v, Json::Num(tx)]));
    }
}

/// Digits and punctuation on their way into a text, a few thousand bytes at a time: a number is written where it
/// goes, with no string made for it and the text asked for room once in a few hundred of them.
struct Staged {
    bytes: [u8; 4096],
    len: usize,
}

impl Staged {
    fn new() -> Staged {
        Staged { bytes: [0; 4096], len: 0 }
    }

    /// What is gathered goes into the text.
    fn flush(&mut self, out: &mut String) {
        out.push_str(std::str::from_utf8(&self.bytes[..self.len]).expect("digits and punctuation"));
        self.len = 0;
    }

    /// Room for `n` bytes more.
    #[inline]
    fn room(&mut self, out: &mut String, n: usize) {
        if self.len + n > self.bytes.len() {
            self.flush(out);
        }
    }

    #[inline]
    fn byte(&mut self, b: u8) {
        self.bytes[self.len] = b;
        self.len += 1;
    }

    #[inline]
    fn bytes(&mut self, text: &[u8]) {
        self.bytes[self.len..self.len + text.len()].copy_from_slice(text);
        self.len += text.len();
    }

    /// A number as `write_number` writes it, in at most 17 bytes when it is a whole one, as nearly all are.
    #[inline]
    fn number(&mut self, out: &mut String, n: f64) {
        if n.fract() == 0.0 && n.abs() < 1e15 {
            if n < 0.0 {
                self.byte(b'-');
            }
            let mut left = n.abs() as u64;
            let mut digits = 1;
            let mut rest = left;
            while rest >= 10 {
                rest /= 10;
                digits += 1;
            }
            self.len += digits;
            let mut at = self.len;
            loop {
                at -= 1;
                self.bytes[at] = b'0' + (left % 10) as u8;
                left /= 10;
                if left == 0 {
                    break;
                }
            }
        } else {
            self.flush(out);
            write_number(out, n);
        }
    }
}

/// The rows as text, with commas between them.
struct Text {
    out: String,
    staged: Staged,
    rows: usize,
}

impl Text {
    fn new() -> Text {
        Text { out: String::new(), staged: Staged::new(), rows: 0 }
    }

    fn finish(mut self) -> String {
        self.staged.flush(&mut self.out);
        self.out
    }
}

impl Rows for Text {
    fn row(&mut self, e: f64, a: f64, v: Cell<'_>, tx: f64) {
        let (out, staged) = (&mut self.out, &mut self.staged);
        // the most a row is long, but for a string or a frozen value, which go into the text themselves
        staged.room(out, 80);
        if self.rows > 0 {
            staged.byte(b',');
        }
        self.rows += 1;
        staged.byte(b'[');
        staged.number(out, e);
        staged.byte(b',');
        staged.number(out, a);
        staged.byte(b',');
        match v {
            Cell::Num(n) => staged.number(out, n),
            Cell::Str(s) => {
                staged.flush(out);
                write_string(out, s);
            }
            Cell::Bool(b) => staged.bytes(if b { b"true" } else { b"false" }),
            Cell::Marked(marker) => {
                staged.byte(b'[');
                staged.number(out, marker);
                staged.byte(b']');
            }
            Cell::Keyword(index) => {
                staged.bytes(b"[0,");
                staged.number(out, index as f64);
                staged.byte(b']');
            }
            Cell::Other(frozen) => {
                staged.bytes(b"[1,");
                staged.flush(out);
                frozen.write(out);
                staged.byte(b']');
            }
        }
        staged.byte(b',');
        staged.number(out, tx);
        staged.byte(b']');
    }
}

/// All of a serialized database but the rows of EAVT.
struct Rest {
    count: usize,
    max_eid: i32,
    max_tx: i32,
    schema: Json,
    attrs: Vec<Json>,
    keywords: Vec<Json>,
    /// Where each of AEVT's datoms is in EAVT, and each of AVET's: `NOWHERE` for one that is not there
    aevt: Vec<usize>,
    avet: Vec<usize>,
}

const NOWHERE: usize = usize::MAX;

/// Where each datom of an index is in EAVT, by what it is: its order among values that do not order is not to be
/// searched by. `places` has the datoms of EAVT that the index may hold.
fn places_of(
    run: &crate::db::Datoms,
    eavt: &[&Datom],
    places: &ByNumber<(i32, Attr, Value), usize>,
) -> Result<Vec<usize>> {
    let mut out = Vec::new();
    run.try_for_each(|d| {
        let found = places.get(&(d.e, d.a, d.v.clone())).copied().or_else(|| {
            // a value that equals nothing, itself included: NaN
            eavt.iter().position(|x| Index::Eavt.cmp(x, d) == Ordering::Equal)
        });
        out.push(found.unwrap_or(NOWHERE));
        Ok(true)
    })?;
    Ok(out)
}

/// Reads the database through once, giving `rows` each datom of EAVT as DataScript writes it, and answers the rest.
/// The values are frozen first and the schema after them, as DataScript does it.
fn gather(db: &Db, opts: &Options, rows: &mut dyn Rows) -> Result<Rest> {
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
    let (eavt_run, aevt_run, avet_run) = (db.index(Index::Eavt), db.index(Index::Aevt), db.index(Index::Avet));
    // the datoms where the index has them: nothing is copied
    let eavt = eavt_run.refs()?;

    // all-attrs: the attributes, each once, in order; and how many datoms each has
    let mut attrs: Vec<Attr> = Vec::new();
    let mut datoms_of: Vec<usize> = Vec::new();
    aevt_run.try_for_each(|d| {
        if attrs.last() != Some(&d.a) {
            attrs.push(d.a);
            datoms_of.push(0);
        }
        *datoms_of.last_mut().expect("an attribute") += 1;
        Ok(true)
    })?;
    let mut number: ByNumber<Attr, usize> = ByNumber::default();
    for (i, a) in attrs.iter().enumerate() {
        number.entry(*a).or_insert(i);
    }
    // the attributes AVET has: only their datoms are looked up by what they are
    let mut in_avet = vec![false; attrs.len()];
    let mut before = None;
    avet_run.try_for_each(|d| {
        if before != Some(d.a) {
            before = Some(d.a);
            if let Some(a) = number.get(&d.a) {
                in_avet[*a] = true;
            }
        }
        Ok(true)
    })?;

    // AEVT has each attribute's datoms as EAVT has them, wherever values order: so where each of AEVT's datoms is
    // in EAVT is EAVT's datoms sorted by attribute, which is counted and not searched for
    let mut next_of: Vec<usize> = Vec::with_capacity(attrs.len());
    let mut total = 0;
    for n in &datoms_of {
        next_of.push(total);
        total += n;
    }
    let mut aevt = vec![NOWHERE; total];
    let mut as_eavt = total == eavt.len();

    let mut kws: Vec<Value> = Vec::new();
    let mut kw_index: std::collections::HashMap<Value, usize> = std::collections::HashMap::new();
    let mut places: ByNumber<(i32, Attr, Value), usize> = ByNumber::default();
    for (i, d) in eavt.iter().enumerate() {
        let a = number.get(&d.a).copied();
        if let Some(a) = a {
            match aevt.get_mut(next_of[a]) {
                Some(place) if *place == NOWHERE => *place = i,
                _ => as_eavt = false,
            }
            next_of[a] += 1;
            if in_avet[a] {
                places.insert((d.e, d.a, d.v.clone()), i);
            }
        }
        let v = match &d.v {
            Value::Str(s) => Cell::Str(s),
            Value::Num(n) if *n == f64::INFINITY => Cell::Marked(MARKER_INF),
            Value::Num(n) if *n == f64::NEG_INFINITY => Cell::Marked(MARKER_MINUS_INF),
            Value::Num(n) if n.is_nan() => Cell::Marked(MARKER_NAN),
            Value::Num(n) => Cell::Num(*n),
            Value::Bool(b) => Cell::Bool(*b),
            Value::Keyword(_) => Cell::Keyword(*kw_index.entry(d.v.clone()).or_insert_with(|| {
                kws.push(d.v.clone());
                kws.len() - 1
            })),
            other => Cell::Other(freeze(other)?),
        };
        rows.row(d.e as f64, a.map_or(f64::NAN, |a| a as f64), v, (d.tx() - TX0) as f64);
    }
    let schema = freeze(&db.schema_value())?;
    let attrs_frozen = attrs.iter().map(|a| freeze_kw(&attr_value(a))).collect::<Result<Vec<_>>>()?;
    let kws_frozen = kws.iter().map(&freeze_kw).collect::<Result<Vec<_>>>()?;

    // AEVT as it is, against what was counted: the same datoms in the same order, or each is looked up
    if as_eavt {
        let mut at = 0;
        aevt_run.try_for_each(|d| {
            as_eavt = aevt[at] != NOWHERE && eavt[aevt[at]].equiv(d);
            at += 1;
            Ok(as_eavt)
        })?;
    }
    if !as_eavt {
        let mut all = ByNumber::with_capacity_and_hasher(eavt.len(), Default::default());
        for (i, d) in eavt.iter().enumerate() {
            all.insert((d.e, d.a, d.v.clone()), i);
        }
        aevt = places_of(&aevt_run, &eavt, &all)?;
    }
    let avet = places_of(&avet_run, &eavt, &places)?;
    Ok(Rest {
        count: eavt.len(),
        max_eid: db.max_eid(),
        max_tx: db.max_tx(),
        schema,
        attrs: attrs_frozen,
        keywords: kws_frozen,
        aevt,
        avet,
    })
}

/// `(d/serializable db opts)`
pub fn serializable(db: &Db, opts: &Options) -> Result<Json> {
    let mut rows = Tree(Vec::new());
    let rest = gather(db, opts, &mut rows)?;
    let places = |at: Vec<usize>| {
        Json::Array(at.into_iter().map(|i| Json::Num(if i == NOWHERE { f64::NAN } else { i as f64 })).collect())
    };
    Ok(Json::Object(vec![
        ("count".into(), Json::Num(rest.count as f64)),
        ("tx0".into(), Json::Num(TX0 as f64)),
        ("max-eid".into(), Json::Num(rest.max_eid as f64)),
        ("max-tx".into(), Json::Num(rest.max_tx as f64)),
        ("schema".into(), rest.schema),
        ("attrs".into(), Json::Array(rest.attrs)),
        ("keywords".into(), Json::Array(rest.keywords)),
        ("eavt".into(), Json::Array(rows.0)),
        ("aevt".into(), places(rest.aevt)),
        ("avet".into(), places(rest.avet)),
    ]))
}

/// `(js/JSON.stringify (d/serializable db opts))`: the same text, written as the database is read, with nothing
/// built of it first. What a database is written out with: a host that wants JavaScript's data parses the text.
pub fn serializable_text(db: &Db, opts: &Options) -> Result<String> {
    let mut rows = Text::new();
    let rest = gather(db, opts, &mut rows)?;
    let mut head = String::new();
    let number = |out: &mut String, key: &str, n: f64| {
        out.push_str(key);
        write_number(out, n);
    };
    number(&mut head, "{\"count\":", rest.count as f64);
    number(&mut head, ",\"tx0\":", TX0 as f64);
    number(&mut head, ",\"max-eid\":", rest.max_eid as f64);
    number(&mut head, ",\"max-tx\":", rest.max_tx as f64);
    head.push_str(",\"schema\":");
    rest.schema.write(&mut head);
    head.push_str(",\"attrs\":");
    Json::Array(rest.attrs).write(&mut head);
    head.push_str(",\"keywords\":");
    Json::Array(rest.keywords).write(&mut head);
    head.push_str(",\"eavt\":[");
    // the rows are the most of it, and are where they are: what comes before them is put before them
    let mut out = rows.finish();
    out.reserve(head.len() + 8 * (rest.aevt.len() + rest.avet.len()) + 32);
    out.insert_str(0, &head);
    let mut staged = Staged::new();
    let mut places = |out: &mut String, key: &str, at: &[usize]| {
        out.push_str(key);
        for (n, i) in at.iter().enumerate() {
            staged.room(out, 24);
            if n > 0 {
                staged.byte(b',');
            }
            staged.number(out, if *i == NOWHERE { f64::NAN } else { *i as f64 });
        }
        staged.flush(out);
    };
    places(&mut out, "],\"aevt\":[", &rest.aevt);
    places(&mut out, "],\"avet\":[", &rest.avet);
    out.push_str("]}");
    Ok(out)
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

/// The value a row names by its place in a list of the database's.
fn nth(items: &[Value], i: Option<&Json>) -> Result<Value> {
    let i = number(i, "index")?;
    if i >= 0.0 && i.fract() == 0.0 && (i as usize) < items.len() {
        Ok(items[i as usize].clone())
    } else {
        Err(Error::msg("Index out of bounds"))
    }
}

/// A row of EAVT, read back as the datom it is.
fn datom_of(
    row: &Json,
    attrs: &[Value],
    keywords: &[Value],
    tx0: f64,
    thaw: &dyn Fn(&Json) -> Result<Value>,
) -> Result<Datom> {
    let row = array(row, "datom")?;
    let e = crate::datom::id_from_num(number(row.first(), "e")?)?;
    let a = nth(attrs, row.get(1))?;
    let attr = value_attr(&a).ok_or_else(|| Error::msg(message!("Bad attribute ", a)))?;
    let v = match row.get(2) {
        Some(Json::Num(n)) => Value::Num(*n),
        Some(Json::Str(s)) => Value::str(s),
        Some(Json::Bool(b)) => Value::Bool(*b),
        Some(Json::Array(marked)) => {
            let marker = marked.first().and_then(Json::as_num);
            match marker {
                Some(m) if m == MARKER_KW => nth(keywords, marked.get(1))?,
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
    Ok(Datom::new(e, attr, v, tx))
}

/// The datom of EAVT that AEVT or AVET names by its place there.
fn datom_at(eavt: &[Datom], i: &Json) -> Result<Datom> {
    let i = number(Some(i), "index")?;
    if i >= 0.0 && i.fract() == 0.0 && (i as usize) < eavt.len() {
        Ok(eavt[i as usize].clone())
    } else {
        Err(Error::msg("Index out of bounds"))
    }
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
    let mut eavt = Vec::new();
    for row in array(member(from, "eavt")?, "eavt")? {
        eavt.push(datom_of(row, &attrs, &keywords, tx0, &thaw)?);
    }
    // each index is built before the next is read: the datoms an index was read into are not kept beside it
    let by_index = |key: &str| -> Result<SortedSet<Datom>> {
        let datoms = array(member(from, key)?, key)?.iter().map(|i| datom_at(&eavt, i)).collect::<Result<Vec<_>>>()?;
        Ok(SortedSet::from_sorted(datoms))
    };
    let aevt = by_index("aevt")?;
    let avet = by_index("avet")?;
    let max_eid = crate::datom::id_from_num(number(from.get("max-eid"), "max-eid")?)?;
    let max_tx = crate::datom::id_from_num(number(from.get("max-tx"), "max-tx")?)?;
    Ok(Db::restore(schema, SortedSet::from_sorted(eavt), aevt, avet, max_eid, max_tx))
}

/// `(d/from-serializable (js/JSON.parse text) opts)`: the database read back from the text itself, a row at a time,
/// with nothing made of the whole text first. What a database is read in with.
pub fn from_serializable_text(text: &str, opts: &Options) -> Result<Db> {
    // what is no object, or no JSON, is read whole: reading it is what says so
    let Some(from) = Members::of(text) else { return from_serializable(&Json::parse(text)?, opts) };
    let thaw = |v: &Json| match opts.thaw_fn {
        Some(f) => f(v),
        None => thaw_default(v),
    };
    let thaw_kw = |v: &Json| match opts.thaw_kw {
        Some(f) => f(v),
        None => thaw_kw_default(v),
    };
    let tx0 = number(from.get("tx0")?.as_ref(), "tx0")?;
    let schema = Arc::new(Schema::new(thaw(&from.member("schema")?)?)?);
    let attrs = array(&from.member("attrs")?, "attrs")?.iter().map(&thaw_kw).collect::<Result<Vec<_>>>()?;
    let keywords = array(&from.member("keywords")?, "keywords")?.iter().map(&thaw_kw).collect::<Result<Vec<_>>>()?;
    let mut eavt = Vec::new();
    let mut rows = from.elements("eavt")?;
    // the attribute at each place among the attributes, once a row has named it
    let mut attr_at: Vec<Option<Attr>> = vec![None; attrs.len()];
    loop {
        if let Some((e, a, v, tx)) = rows.plain_row() {
            // as `datom_of` makes a datom of the row, part by part and in its order
            let e = crate::datom::id_from_num(e)?;
            let at = (a >= 0.0 && a.fract() == 0.0 && (a as usize) < attr_at.len()).then_some(a as usize);
            let attr = match at.and_then(|at| attr_at[at]) {
                Some(attr) => attr,
                None => {
                    let named = nth(&attrs, Some(&Json::Num(a)))?;
                    let attr = value_attr(&named).ok_or_else(|| Error::msg(message!("Bad attribute ", named)))?;
                    if let Some(at) = at {
                        attr_at[at] = Some(attr);
                    }
                    attr
                }
            };
            let tx = crate::datom::id_from_num(tx0 + tx)?;
            eavt.push(Datom::new(e, attr, v, tx));
        } else {
            match rows.next() {
                Some(row) => eavt.push(datom_of(&row?, &attrs, &keywords, tx0, &thaw)?),
                None => break,
            }
        }
    }
    let by_index = |key: &str| -> Result<SortedSet<Datom>> {
        let datoms = from.elements(key)?.map(|i| datom_at(&eavt, &i?)).collect::<Result<Vec<_>>>()?;
        Ok(SortedSet::from_sorted(datoms))
    };
    let aevt = by_index("aevt")?;
    let avet = by_index("avet")?;
    let max_eid = crate::datom::id_from_num(number(from.get("max-eid")?.as_ref(), "max-eid")?)?;
    let max_tx = crate::datom::id_from_num(number(from.get("max-tx")?.as_ref(), "max-tx")?)?;
    Ok(Db::restore(schema, SortedSet::from_sorted(eavt), aevt, avet, max_eid, max_tx))
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
    fn numbers_and_strings_as_json_stringify_writes_them() {
        let number = |n: f64| {
            let mut out = String::new();
            write_number(&mut out, n);
            out
        };
        for n in [0.0, -0.0, 1.0, -1.0, 9.0, 10.0, 536870912.0, -2147483648.0, 999999999999999.0, 1e15, 1e21, 0.5] {
            assert_eq!(number(n), number_to_string(if n == 0.0 { 0.0 } else { n }), "{n}");
        }
        // each count of digits, and the zeros inside a number
        let mut n = 1u64;
        while n < 1_000_000_000_000_000 {
            for near in [n - 1, n, n + 1, n * 7 + 5, n * 10 / 3] {
                if near < 1_000_000_000_000_000 {
                    assert_eq!(number(near as f64), near.to_string());
                    assert_eq!(number(-(near as f64)), if near == 0 { "0".to_string() } else { format!("-{near}") });
                }
            }
            n *= 10;
        }
        for n in [100, 1000, 1001, 10010, 100100, 1000000, 20000300, 999999] {
            assert_eq!(number(n as f64), n.to_string());
        }
        assert_eq!(number(f64::NAN), "null");
        assert_eq!(number(f64::INFINITY), "null");
        assert_eq!(number(-123456789012345.0), "-123456789012345");
        let string = |s: &str| {
            let mut out = String::new();
            write_string(&mut out, s);
            out
        };
        assert_eq!(string("plain"), "\"plain\"");
        assert_eq!(
            string("a\"b\\c\n\r\t\u{8}\u{c}\u{1}\u{1f}é😀\u{7f}"),
            "\"a\\\"b\\\\c\\n\\r\\t\\b\\f\\u0001\\u001fé😀\u{7f}\""
        );
        assert_eq!(string(""), "\"\"");
    }

    #[test]
    fn numbers_are_read_as_they_are_spelled() {
        for text in [
            "0",
            "-0",
            "7",
            "007",
            "-12",
            "123456789012345",
            "1234567890123456",
            "9007199254740993",
            "1.5",
            "-1e3",
            "1E+2",
            "0.1",
            "5e-324",
            "1e400",
        ] {
            let n = Json::parse(text).unwrap().as_num().unwrap();
            let std = text.parse::<f64>().unwrap();
            assert_eq!(n.to_bits(), std.to_bits(), "{text}");
        }
        for text in ["-", "1-2", "--1", "1.2.3", "1e", "+1"] {
            assert!(Json::parse(text).is_err(), "{text}");
        }
    }

    fn sample(schema: &str, tx: &str) -> Db {
        let schema = crate::edn::read_string(schema).unwrap();
        crate::db_with(&Db::empty(schema).unwrap(), &crate::edn::read_string(tx).unwrap()).unwrap()
    }

    fn samples() -> Vec<Db> {
        let mut many = String::from("[");
        for i in 1..400 {
            many.push_str(&format!(
                "{{:db/id {i} :name \"n{i}\" :code \"c{}\" :age {} :aka [\"a{i}\" \"b{}\"] :kw :k/w{} :score {}.5}} ",
                i % 37,
                i % 9,
                i % 5,
                i % 3,
                i % 11
            ));
        }
        many.push(']');
        vec![
            sample("nil", "[]"),
            sample(
                "{:aka {:db/cardinality :db.cardinality/many} :age {:db/index true}}",
                r#"[{:db/id 1 :name "Petr" :aka ["Devil" "Tupen"] :age 15 :kw :some/kw :attach {:k [1 2]}}
                    {:db/id 2 :inf ##Inf :ninf ##-Inf :nan ##NaN :b false :age 15 :name "a\"b\\c\n"}]"#,
            ),
            sample(
                "{:aka {:db/cardinality :db.cardinality/many :db/index true} :name {:db/unique :db.unique/identity} :code {:db/index true} :age {:db/index true} :kw {:db/index true}}",
                &many,
            ),
            // values that do not order, under one attribute that is indexed: each is looked up by what it is
            sample("{:v {:db/cardinality :db.cardinality/many :db/index true}}", "[{:db/id 1 :v [##NaN 1 \"x\" :k [1 2] true]} {:db/id 2 :v [##NaN 2]}]"),
        ]
    }

    #[test]
    fn database_round_trip() {
        for db in samples() {
            let json = serializable(&db, &Options::default()).unwrap();
            let back = from_serializable(&Json::parse(&json.to_json_string()).unwrap(), &Options::default()).unwrap();
            assert_eq!(back.count().unwrap(), db.count().unwrap());
            assert_eq!(back.max_eid(), db.max_eid());
            assert_eq!(back.max_tx(), db.max_tx());
            // NaN equals nothing, itself included: compare what is printed
            assert_eq!(pr_str(&Value::Db(back)), pr_str(&Value::Db(db)));
        }
    }

    #[test]
    fn the_text_is_the_data_written() {
        for db in samples() {
            let text = serializable_text(&db, &Options::default()).unwrap();
            assert_eq!(text, serializable(&db, &Options::default()).unwrap().to_json_string());
            // and read back a row at a time, it is the database that reading it whole makes
            let whole = from_serializable(&Json::parse(&text).unwrap(), &Options::default()).unwrap();
            let by_rows = from_serializable_text(&text, &Options::default()).unwrap();
            assert_eq!(pr_str(&Value::Db(by_rows.clone())), pr_str(&Value::Db(whole.clone())));
            for index in [Index::Eavt, Index::Aevt, Index::Avet] {
                let rows = |db: &Db| {
                    db.index(index)
                        .to_vec()
                        .unwrap()
                        .iter()
                        .map(|d| pr_str(&Value::Datom(Arc::new(d.clone()))))
                        .collect::<Vec<_>>()
                };
                assert_eq!(rows(&by_rows), rows(&whole));
                assert_eq!(rows(&by_rows), rows(&db));
            }
        }
    }

    /// `serializable` as it was first written, the plain way: every datom of every index copied out, every row built,
    /// and every datom of EAVT in a map by what it is. What reads the database once answers the same.
    fn the_plain_way(db: &Db) -> Json {
        let eavt = db.index(Index::Eavt).to_vec().unwrap();
        let aevt = db.index(Index::Aevt).to_vec().unwrap();
        let avet = db.index(Index::Avet).to_vec().unwrap();
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
                other => Json::Array(vec![Json::Num(MARKER_OTHER), freeze_default(other).unwrap()]),
            };
            rows.push(Json::Array(vec![
                Json::Num(d.e as f64),
                Json::Num(attr_index(d)),
                v,
                Json::Num((d.tx() - TX0) as f64),
            ]));
        }
        let mut places: std::collections::HashMap<(i32, Value, Value), usize> = std::collections::HashMap::new();
        for (i, d) in eavt.iter().enumerate() {
            places.insert((d.e, d.a_value(), d.v.clone()), i);
        }
        let position = |d: &Datom| -> Json {
            let found = places
                .get(&(d.e, d.a_value(), d.v.clone()))
                .copied()
                .or_else(|| eavt.iter().position(|x| Index::Eavt.cmp(x, d) == Ordering::Equal));
            Json::Num(found.map_or(f64::NAN, |i| i as f64))
        };
        Json::Object(vec![
            ("count".into(), Json::Num(eavt.len() as f64)),
            ("tx0".into(), Json::Num(TX0 as f64)),
            ("max-eid".into(), Json::Num(db.max_eid() as f64)),
            ("max-tx".into(), Json::Num(db.max_tx() as f64)),
            ("schema".into(), freeze_default(&db.schema_value()).unwrap()),
            ("attrs".into(), Json::Array(attrs.iter().map(|a| freeze_kw_default(a).unwrap()).collect())),
            ("keywords".into(), Json::Array(kws.iter().map(|k| freeze_kw_default(k).unwrap()).collect())),
            ("eavt".into(), Json::Array(rows)),
            ("aevt".into(), Json::Array(aevt.iter().map(&position).collect())),
            ("avet".into(), Json::Array(avet.iter().map(&position).collect())),
        ])
    }

    #[test]
    fn read_once_it_answers_as_the_plain_way_does() {
        for db in samples() {
            // NaN is not NaN: compared as written
            assert_eq!(
                serializable(&db, &Options::default()).unwrap().to_json_string(),
                the_plain_way(&db).to_json_string()
            );
        }
        // databases made of transactions in no order, with what was added taken out again
        let mut seed = 0x9E37_79B9_7F4A_7C15u64;
        let mut next = |n: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % n
        };
        for _ in 0..40 {
            let mut db = Db::empty(crate::edn::read_string("{:tag {:db/cardinality :db.cardinality/many :db/index true} :n {:db/index true} :ref {:db/valueType :db.type/ref}}").unwrap()).unwrap();
            for _ in 0..(5 + next(60)) {
                let e = 1 + next(30);
                let tx = match next(7) {
                    0 => format!("[[:db/add {e} :n {}]]", next(9)),
                    1 => format!("[[:db/add {e} :tag \"t{}\"]]", next(6)),
                    2 => format!("[[:db/add {e} :tag :k{}] [:db/add {e} :s \"s{}\"]]", next(4), next(50)),
                    3 => format!("[[:db/add {e} :ref {}]]", 1 + next(30)),
                    4 => format!("[[:db.fn/retractEntity {e}]]"),
                    5 => format!("[[:db/add {e} :f {}.25] [:db/add {e} :b {}]]", next(5), next(2) == 0),
                    _ => format!("[[:db/retract {e} :tag \"t{}\"] [:db/add {e} :m {{:a [{}]}}]]", next(6), next(3)),
                };
                db = crate::db_with(&db, &crate::edn::read_string(&tx).unwrap()).unwrap();
            }
            assert_eq!(serializable(&db, &Options::default()).unwrap(), the_plain_way(&db));
            let text = serializable_text(&db, &Options::default()).unwrap();
            assert_eq!(text, the_plain_way(&db).to_json_string());
            let back = from_serializable_text(&text, &Options::default()).unwrap();
            assert!(back.equiv(&db));
        }
    }

    #[test]
    fn where_each_datom_is_in_eavt() {
        // AEVT and AVET name EAVT's datoms by their places: every place once in AEVT, and AVET's are the indexed
        for db in samples() {
            let json = serializable(&db, &Options::default()).unwrap();
            let count = db.count().unwrap();
            let mut aevt: Vec<usize> =
                json.get("aevt").unwrap().as_array().unwrap().iter().map(|i| i.as_num().unwrap() as usize).collect();
            aevt.sort_unstable();
            assert_eq!(aevt, (0..count).collect::<Vec<_>>());
            assert_eq!(json.get("avet").unwrap().as_array().unwrap().len(), db.index(Index::Avet).count().unwrap());
        }
    }

    /// What is wrong with a text is said as reading it whole says it, whichever way it is read.
    #[test]
    fn a_text_that_is_not_a_database() {
        let good = serializable_text(&samples().remove(1), &Options::default()).unwrap();
        let mut texts: Vec<String> = [
            "", "[]", "7", "{", "{}", "{\"tx0\":1}", "{\"tx0\":\"x\"}", "nonsense", "{\"tx0\":536870912,\"schema\":\"nil\"}",
            "{\"tx0\":536870912,\"schema\":\"nil\",\"attrs\":[],\"keywords\":[],\"eavt\":7}",
            "{\"tx0\":536870912,\"schema\":\"nil\",\"attrs\":[],\"keywords\":[],\"eavt\":[7]}",
            "{\"tx0\":536870912,\"schema\":\"nil\",\"attrs\":[\":a\"],\"keywords\":[],\"eavt\":[[1,0,null,1]]}",
            "{\"tx0\":536870912,\"schema\":\"nil\",\"attrs\":[\":a\"],\"keywords\":[],\"eavt\":[[1,0,[9],1]]}",
            "{\"tx0\":536870912,\"schema\":\"nil\",\"attrs\":[\":a\"],\"keywords\":[],\"eavt\":[[1,5,1,1]]}",
            "{\"tx0\":536870912,\"schema\":\"nil\",\"attrs\":[\":a\"],\"keywords\":[],\"eavt\":[[1,0,1,1]],\"aevt\":[3],\"avet\":[]}",
            "{\"tx0\":536870912,\"schema\":\"nil\",\"attrs\":[\":a\"],\"keywords\":[],\"eavt\":[[1,0,1,1]],\"aevt\":[0],\"avet\":[]}",
            "{\"tx0\":536870912,\"schema\":\"nil\",\"attrs\":[\":a\"],\"keywords\":[],\"eavt\":[[1,0,1,1],],\"aevt\":[0],\"avet\":[]}",
            "{\"tx0\":536870912,\"schema\":\"nil\",\"attrs\":[\":a\"],\"keywords\":[],\"eavt\":[[1,0,\"\\q\",1]],\"aevt\":[0],\"avet\":[]}",
            "{\"tx0\":536870912,\"schema\":\"nil\",\"attrs\":[\":a\"],\"keywords\":[],\"eavt\":[[1,0,01,1]],\"aevt\":[0],\"avet\":[],\"max-eid\":1,\"max-tx\":536870913}",
            "{\"tx0\":536870912,\"schema\":\"nil\",\"attrs\":[\":a\"],\"keywords\":[],\"eavt\":[[1,0,1.e,1]],\"aevt\":[0],\"avet\":[],\"max-eid\":1,\"max-tx\":536870913}",
            " {\"max-tx\":536870913, \"tx0\":536870912,\"eavt\":[ [1 ,0, \"\\u00e9\\ud83d\\ude00\" ,1] ] ,\"schema\":\"nil\",\"attrs\":[\":a\"],\"keywords\":[],\"aevt\":[ 0 ],\"avet\":[ ],\"max-eid\":1, \"tx0\":536870912 } ",
        ]
        .iter()
        .map(|t| t.to_string())
        .collect();
        // the text of a database cut short at every length, and with one byte changed at a time
        for cut in (0..good.len()).step_by(7) {
            if good.is_char_boundary(cut) {
                texts.push(good[..cut].to_string());
            }
        }
        for at in (0..good.len()).step_by(11) {
            let mut bytes = good.clone().into_bytes();
            bytes[at] = b"]}[{\",:x0-. "[at % 12];
            if let Ok(changed) = String::from_utf8(bytes) {
                texts.push(changed);
            }
        }
        let mut read = 0;
        for text in &texts {
            let whole = Json::parse(text).and_then(|json| from_serializable(&json, &Options::default()));
            let by_rows = from_serializable_text(text, &Options::default());
            match (whole, by_rows) {
                (Ok(a), Ok(b)) => {
                    read += 1;
                    assert_eq!(pr_str(&Value::Db(a)), pr_str(&Value::Db(b)), "{text}")
                }
                (Err(a), Err(b)) => assert_eq!((a.message, pr_str(&a.data)), (b.message, pr_str(&b.data)), "{text}"),
                (a, b) => panic!(
                    "{text}: whole {:?}, by rows {:?}",
                    a.map(|_| ()).map_err(|e| e.message),
                    b.map(|_| ()).map_err(|e| e.message)
                ),
            }
        }
        assert!(read >= 3, "{read} of the texts are databases");
    }
}
