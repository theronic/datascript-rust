//! An EDN reader, after ClojureScript's (`cljs.reader/read-string`, which is `cljs.tools.reader.edn`): the same
//! tokens, numbers and collections, and maps and sets made as ClojureScript's reader makes them, so that what is read
//! here iterates as it would there.

use crate::coll::{CljMap, CljSet};
use crate::error::{Error, Result};
use crate::named::{Keyword, Symbol};
use crate::print::days_from_civil;
use crate::value::Value;

/// What a tagged literal reads as: `Ok(Some(value))`, or `Ok(None)` to leave the tag to the reader's own.
pub type TagReader<'a> = &'a dyn Fn(&str, &Value) -> Result<Option<Value>>;

/// `(cljs.reader/read-string s)`: the first form in `s`, or `nil` when there is none.
pub fn read_string(s: &str) -> Result<Value> {
    Reader::new(s, None).read_top()
}

/// As `read_string`, with a reader for tagged literals the EDN reader does not know.
pub fn read_string_with(s: &str, tags: TagReader<'_>) -> Result<Value> {
    Reader::new(s, Some(tags)).read_top()
}

/// Every form in `s`.
pub fn read_all(s: &str) -> Result<Vec<Value>> {
    let mut r = Reader::new(s, None);
    let mut out = Vec::new();
    while let Some(v) = r.read_next()? {
        out.push(v);
    }
    Ok(out)
}

struct Reader<'a> {
    chars: std::iter::Peekable<std::str::CharIndices<'a>>,
    tags: Option<TagReader<'a>>,
}

fn reader_error(msg: impl Into<String>) -> Error {
    Error::new(msg, Value::kw_map(&[("type", Value::kw("reader-exception")), ("ex-kind", Value::kw("reader-error"))]))
}

fn eof_error(what: &str) -> Error {
    Error::new(
        format!("Unexpected EOF while reading {what}."),
        Value::kw_map(&[("type", Value::kw("reader-exception")), ("ex-kind", Value::kw("eof"))]),
    )
}

/// An error raised while reading, by a tag's reader for one, is the reader's: it carries the reader's data.
fn wrap(e: Error) -> Error {
    if e.data.is_nil() {
        Error { data: Value::kw_map(&[("type", Value::kw("reader-exception"))]), ..e }
    } else {
        e
    }
}

/// `inspect`: a value as the reader's messages describe it, cut short.
fn inspect(v: &Value, truncate: bool) -> String {
    let col = |items: Vec<&Value>, open: &str, close: &str| -> String {
        let n = items.len();
        let l = if truncate { 0 } else { n.min(10) };
        let shown: Vec<String> = items.iter().take(l).map(|x| inspect(x, true)).collect();
        format!("{open}{}{}{close}", shown.join(" "), if l < n { "..." } else { "" })
    };
    match v {
        Value::Nil => "nil".into(),
        Value::Str(s) => {
            let n = if truncate { 5 } else { 20 };
            let units: Vec<u16> = s.encode_utf16().collect();
            let cut = String::from_utf16_lossy(&units[..n.min(units.len())]);
            format!("\"{cut}{}", if units.len() > n { "...\"" } else { "\"" })
        }
        Value::Keyword(_) | Value::Num(_) | Value::Symbol(_) | Value::Bool(_) => crate::print::str_of(v),
        Value::Vector(items) => col(items.iter().collect(), "[", "]"),
        Value::List(items) => col(items.iter().collect(), "(", ")"),
        Value::Set(items) => col(items.iter().collect(), "#{", "}"),
        Value::Map(m) => {
            let shown = if truncate { 0 } else { m.len() };
            let contents: Vec<&Value> = m.iter().take(shown).flat_map(|(k, v)| [k, v]).collect();
            col(contents, "{", if m.len() > shown { "...}" } else { "}" })
        }
        other => crate::print::pr_str(other),
    }
}

/// `duplicate-keys-error`
fn duplicate_keys_error(kind: &str, keys: &[Value]) -> Error {
    let freqs = crate::clj::frequencies(keys);
    let dups: Vec<String> = freqs
        .iter()
        .filter(|(_, n)| n.as_num().is_some_and(|n| n > 1.0))
        .map(|(k, _)| crate::print::str_of(k))
        .collect();
    reader_error(format!(
        "{kind} literal contains duplicate key{}: {}",
        if dups.len() > 1 { "s" } else { "" },
        dups.join(", ")
    ))
}

/// The characters a token cannot hold
fn not_constituent(c: char) -> bool {
    matches!(c, '@' | '`' | '~')
}

/// `whitespace?`: JavaScript's `\s`, and the comma.
fn is_whitespace(c: char) -> bool {
    c == ',' || c.is_whitespace() || c == '\u{feff}'
}

/// The characters that start a form of their own, which end a token.
fn is_macro(c: char) -> bool {
    matches!(c, '"' | ':' | ';' | '^' | '(' | ')' | '[' | ']' | '{' | '}' | '\\' | '#')
}

/// `macro-terminating?`: the macro characters that end a symbol; `#` and `:` may be inside one.
fn is_terminating(c: char) -> bool {
    matches!(c, '"' | ';' | '^' | '(' | ')' | '[' | ']' | '{' | '}' | '\\')
}

impl<'a> Reader<'a> {
    fn new(src: &'a str, tags: Option<TagReader<'a>>) -> Reader<'a> {
        Reader { chars: src.char_indices().peekable(), tags }
    }

    #[inline]
    fn peek(&mut self) -> Option<char> {
        self.chars.peek().map(|(_, c)| *c)
    }

    #[inline]
    fn next(&mut self) -> Option<char> {
        self.chars.next().map(|(_, c)| c)
    }

    fn read_top(&mut self) -> Result<Value> {
        Ok(self.read_next()?.unwrap_or(Value::Nil))
    }

    /// The next form, or `None` at the end of input.
    fn read_next(&mut self) -> Result<Option<Value>> {
        loop {
            match self.read_form().map_err(wrap)? {
                Form::Value(v) => return Ok(Some(v)),
                Form::Skip => continue,
                Form::Eof => return Ok(None),
                Form::Close(c) => return Err(reader_error(format!("Unmatched delimiter {c}."))),
            }
        }
    }

    /// The next form, which must be there: after a tag, a `#_`, metadata.
    fn read_required(&mut self) -> Result<Value> {
        loop {
            match self.read_form()? {
                Form::Value(v) => return Ok(v),
                Form::Skip => continue,
                Form::Eof => {
                    return Err(Error::new(
                        "EOF while reading.",
                        Value::kw_map(&[("type", Value::kw("reader-exception")), ("ex-kind", Value::kw("eof"))]),
                    ))
                }
                Form::Close(c) => return Err(reader_error(format!("Unmatched delimiter {c}."))),
            }
        }
    }

    fn read_form(&mut self) -> Result<Form> {
        loop {
            let Some(c) = self.peek() else { return Ok(Form::Eof) };
            if is_whitespace(c) {
                self.next();
                continue;
            }
            return match c {
                ';' => {
                    self.skip_line();
                    Ok(Form::Skip)
                }
                '"' => {
                    self.next();
                    self.read_str().map(Form::Value)
                }
                ':' => {
                    self.next();
                    self.read_keyword().map(Form::Value)
                }
                '(' => {
                    self.next();
                    Ok(Form::Value(Value::list(self.read_delimited(')', "list")?)))
                }
                '[' => {
                    self.next();
                    Ok(Form::Value(Value::vector(self.read_delimited(']', "vector")?)))
                }
                '{' => {
                    self.next();
                    let items = self.read_delimited('}', "map")?;
                    make_map(items).map(Form::Value)
                }
                ')' | ']' | '}' => {
                    self.next();
                    Ok(Form::Close(c))
                }
                '\\' => {
                    self.next();
                    self.read_char().map(Form::Value)
                }
                '^' => {
                    self.next();
                    self.read_meta().map(Form::Value)
                }
                '#' => {
                    self.next();
                    self.read_dispatch()
                }
                c if c.is_ascii_digit() => {
                    self.next();
                    self.read_number(c).map(Form::Value)
                }
                '+' | '-' => {
                    self.next();
                    match self.peek() {
                        Some(d) if d.is_ascii_digit() => self.read_number(c).map(Form::Value),
                        _ => self.read_symbol(c).map(Form::Value),
                    }
                }
                c => {
                    self.next();
                    self.read_symbol(c).map(Form::Value)
                }
            };
        }
    }

    /// `^meta form`: the form, without its metadata, which nothing here carries.
    fn read_meta(&mut self) -> Result<Value> {
        let m = self.read_required()?;
        if !matches!(m, Value::Keyword(_) | Value::Symbol(_) | Value::Str(_) | Value::Map(_)) {
            return Err(reader_error(format!(
                "Metadata cannot be {}. Metadata must be a Symbol, Keyword, String or Map.",
                inspect(&m, false)
            )));
        }
        let target = self.read_required()?;
        if !matches!(target, Value::Symbol(_) | Value::Vector(_) | Value::List(_) | Value::Map(_) | Value::Set(_)) {
            return Err(reader_error(format!(
                "Metadata can not be applied to {}. Metadata can only be applied to IMetas.",
                inspect(&target, false)
            )));
        }
        Ok(target)
    }

    fn skip_line(&mut self) {
        while let Some(c) = self.next() {
            if c == '\n' {
                break;
            }
        }
    }

    fn read_delimited(&mut self, close: char, what: &str) -> Result<Vec<Value>> {
        let mut out = Vec::new();
        loop {
            match self.read_form()? {
                Form::Value(v) => out.push(v),
                Form::Skip => {}
                Form::Eof => return Err(eof_error(&format!("item {} of {what}", out.len()))),
                Form::Close(c) if c == close => return Ok(out),
                Form::Close(c) => return Err(reader_error(format!("Unmatched delimiter {c}."))),
            }
        }
    }

    /// The rest of a token that began with `first`: up to whitespace or a character that ends a symbol.
    fn read_token(&mut self, kind: &str, first: Option<char>, validate_leading: bool) -> Result<String> {
        let mut s = String::new();
        if let Some(c) = first {
            if validate_leading && not_constituent(c) {
                return Err(reader_error(format!("Invalid character: {c} found while reading {kind}.")));
            }
            s.push(c);
        }
        while let Some(c) = self.peek() {
            if is_whitespace(c) || is_terminating(c) {
                break;
            }
            if not_constituent(c) {
                return Err(reader_error(format!("Invalid character: {c} found while reading {kind}.")));
            }
            s.push(c);
            self.next();
        }
        Ok(s)
    }

    fn read_symbol(&mut self, first: char) -> Result<Value> {
        let token = self.read_token("symbol", Some(first), true)?;
        match token.as_str() {
            "nil" => Ok(Value::Nil),
            "true" => Ok(Value::Bool(true)),
            "false" => Ok(Value::Bool(false)),
            "/" => Ok(Value::Symbol(Symbol::parse("/"))),
            _ => match parse_symbol(&token) {
                Some((ns, name)) => Ok(Value::Symbol(Symbol::new(ns, name))),
                None => Err(reader_error(format!("Invalid symbol: {token}."))),
            },
        }
    }

    fn read_keyword(&mut self) -> Result<Value> {
        match self.peek() {
            Some(c) if !is_whitespace(c) => {
                self.next();
                let token = self.read_token("keyword", Some(c), true)?;
                match parse_symbol(&token) {
                    Some((ns, name)) if !token.contains("::") && !token.starts_with(':') => {
                        Ok(Value::Keyword(Keyword::new(ns, name)))
                    }
                    _ => Err(reader_error(format!("Invalid keyword: :{token}."))),
                }
            }
            Some(_) => Err(reader_error("A single colon is not a valid keyword.")),
            None => Err(Error::new(
                "Unexpected EOF while reading start of keyword.",
                Value::kw_map(&[("type", Value::kw("reader-exception")), ("ex-kind", Value::kw("eof"))]),
            )),
        }
    }

    fn read_number(&mut self, first: char) -> Result<Value> {
        let mut s = String::new();
        s.push(first);
        while let Some(c) = self.peek() {
            if is_whitespace(c) || is_macro(c) {
                break;
            }
            s.push(c);
            self.next();
        }
        match match_number(&s) {
            Some(n) => Ok(Value::Num(n)),
            None => Err(reader_error(format!("Invalid number: {s}."))),
        }
    }

    fn read_str(&mut self) -> Result<Value> {
        let mut s = String::new();
        loop {
            match self.next() {
                None => {
                    return Err(Error::new(
                        format!("Unexpected EOF reading string starting \"\"{s}."),
                        Value::kw_map(&[("type", Value::kw("reader-exception")), ("ex-kind", Value::kw("eof"))]),
                    ))
                }
                Some('"') => return Ok(Value::from(s)),
                Some('\\') => match self.next() {
                    None => return Err(reader_error("Unsupported escape character: \\.")),
                    Some('t') => s.push('\t'),
                    Some('r') => s.push('\r'),
                    Some('n') => s.push('\n'),
                    Some('\\') => s.push('\\'),
                    Some('"') => s.push('"'),
                    Some('b') => s.push('\u{8}'),
                    Some('f') => s.push('\u{c}'),
                    Some('u') => {
                        let code = self.read_digits(16, 4, true)?;
                        s.push(char::from_u32(code).unwrap_or('\u{fffd}'));
                    }
                    Some(c) if c.is_ascii_digit() => {
                        let start = c
                            .to_digit(8)
                            .ok_or_else(|| reader_error(format!("Invalid digit {c} in unicode character.")))?;
                        let code = self.read_digits_from(start, 8, 2)?;
                        if code > 0o377 {
                            return Err(reader_error("Octal escape sequence must be in range [0, 377]."));
                        }
                        s.push(char::from_u32(code).unwrap_or('\u{fffd}'));
                    }
                    Some(c) => return Err(reader_error(format!("Unsupported escape character: \\{c}."))),
                },
                Some(c) => s.push(c),
            }
        }
    }

    /// Exactly `len` digits in `base`, or at most `len` when not `exact`.
    fn read_digits(&mut self, base: u32, len: usize, exact: bool) -> Result<u32> {
        let mut code = 0u32;
        let mut n = 0;
        while n < len {
            match self.peek().and_then(|c| c.to_digit(base)) {
                Some(d) => {
                    code = code * base + d;
                    self.next();
                    n += 1;
                }
                None => break,
            }
        }
        if n == 0 || (exact && n != len) {
            return Err(reader_error("Invalid unicode escape."));
        }
        Ok(code)
    }

    fn read_digits_from(&mut self, start: u32, base: u32, more: usize) -> Result<u32> {
        let mut code = start;
        for _ in 0..more {
            match self.peek().and_then(|c| c.to_digit(base)) {
                Some(d) => {
                    code = code * base + d;
                    self.next();
                }
                None => break,
            }
        }
        Ok(code)
    }

    /// A character literal, which ClojureScript reads as a string of one character.
    fn read_char(&mut self) -> Result<Value> {
        let Some(c) = self.next() else { return Err(eof_error("character")) };
        let token = if is_terminating(c) || not_constituent(c) || is_whitespace(c) {
            c.to_string()
        } else {
            self.read_token("character", Some(c), false)?
        };
        let mut it = token.chars();
        let first = it.next().unwrap();
        if it.next().is_none() {
            return Ok(Value::from(token));
        }
        let named = match token.as_str() {
            "newline" => Some('\n'),
            "space" => Some(' '),
            "tab" => Some('\t'),
            "backspace" => Some('\u{8}'),
            "formfeed" => Some('\u{c}'),
            "return" => Some('\r'),
            _ => None,
        };
        if let Some(c) = named {
            return Ok(Value::from(c.to_string()));
        }
        // `read-unicode-char`: the digits after the first character, all of them of the base
        let digits = |base: u32| -> Result<u32> {
            let mut code = 0u32;
            for c in token.chars().skip(1) {
                let d = c.to_digit(base).ok_or_else(|| {
                    Error::new(
                        format!("Invalid digit {c} in unicode character \\{token}."),
                        Value::kw_map(&[
                            ("type", Value::kw("reader-exception")),
                            ("ex-kind", Value::kw("illegal-argument")),
                        ]),
                    )
                })?;
                code = code * base + d;
            }
            Ok(code)
        };
        let len = token.chars().count();
        if first == 'u' {
            if len != 5 {
                return Err(reader_error(format!("Invalid unicode literal: \\{token}.")));
            }
            let code = digits(16)?;
            if (0xd800..0xe000).contains(&code) {
                return Err(reader_error(format!("Invalid character literal \\u{code:x}.")));
            }
            return Ok(Value::from(char::from_u32(code).unwrap_or('\u{fffd}').to_string()));
        }
        if first == 'o' {
            if len - 1 > 3 {
                return Err(reader_error(format!(
                    "Invalid octal escape sequence in a character literal: {token}. Octal escape sequences must be 3 or fewer digits."
                )));
            }
            let code = digits(8)?;
            if code > 0o377 {
                return Err(reader_error("Octal escape sequence must be in range [0, 377]."));
            }
            return Ok(Value::from(char::from_u32(code).unwrap_or('\u{fffd}').to_string()));
        }
        Err(reader_error(format!("Unsupported character: \\{token}.")))
    }

    fn read_dispatch(&mut self) -> Result<Form> {
        match self.peek() {
            None => Err(eof_error("dispatch character")),
            Some('{') => {
                self.next();
                let items = self.read_delimited('}', "set")?;
                let n = items.len();
                let set: CljSet = items.iter().cloned().collect();
                if set.len() != n {
                    return Err(duplicate_keys_error("Set", &items));
                }
                Ok(Form::Value(Value::set(set)))
            }
            Some('_') => {
                self.next();
                self.read_required()?;
                Ok(Form::Skip)
            }
            Some('^') => {
                self.next();
                self.read_meta().map(Form::Value)
            }
            Some('!') => {
                self.skip_line();
                Ok(Form::Skip)
            }
            Some('#') => {
                self.next();
                let sym = self.read_required()?;
                match &sym {
                    Value::Symbol(s) if s.full() == "Inf" => Ok(Form::Value(Value::Num(f64::INFINITY))),
                    Value::Symbol(s) if s.full() == "-Inf" => Ok(Form::Value(Value::Num(f64::NEG_INFINITY))),
                    Value::Symbol(s) if s.full() == "NaN" => Ok(Form::Value(Value::Num(f64::NAN))),
                    _ => Err(reader_error(format!("Invalid token: ##{}", crate::print::str_of(&sym)))),
                }
            }
            Some(':') => {
                self.next();
                self.read_namespaced_map().map(Form::Value)
            }
            Some('<') => Err(reader_error("Unreadable form")),
            Some(_) => {
                let tag = self.read_required()?;
                let form = self.read_required()?;
                let Value::Symbol(tag) = tag else {
                    return Err(reader_error(
                        "Invalid reader tag: \"Reader tag must be a sy...\". Reader tags must be symbols.",
                    ));
                };
                self.read_tagged(&tag, form).map(Form::Value)
            }
        }
    }

    fn read_namespaced_map(&mut self) -> Result<Value> {
        let first = self.next();
        let token = self.read_token("namespaced-map", first, true)?;
        if !matches!(parse_symbol(&token), Some((None, _))) {
            return Err(reader_error(format!("Invalid value used as namespace in namespaced map: {token}.")));
        }
        loop {
            match self.peek() {
                Some(c) if is_whitespace(c) => {
                    self.next();
                }
                Some('{') => {
                    self.next();
                    break;
                }
                _ => {
                    return Err(reader_error(format!("Namespaced map with namespace {token} does not specify a map.")))
                }
            }
        }
        let items = self.read_delimited('}', "namespaced-map")?;
        if items.len() % 2 != 0 {
            return Err(odd_map_error(&items));
        }
        let qualify = |k: Value| -> Value {
            match &k {
                Value::Keyword(kw) => match kw.ns() {
                    None => Value::Keyword(Keyword::new(Some(&token), kw.name())),
                    Some("_") => Value::Keyword(Keyword::new(None, kw.name())),
                    Some(_) => k,
                },
                Value::Symbol(s) => match s.ns() {
                    None => Value::Symbol(Symbol::new(Some(&token), s.name())),
                    Some("_") => Value::Symbol(Symbol::new(None, s.name())),
                    Some(_) => k,
                },
                _ => k,
            }
        };
        let mut pairs = Vec::with_capacity(items.len() / 2);
        let mut it = items.into_iter();
        while let (Some(k), Some(v)) = (it.next(), it.next()) {
            pairs.push((qualify(k), v));
        }
        let keys: Vec<Value> = pairs.iter().map(|(k, _)| k.clone()).collect();
        let distinct: CljSet = keys.iter().cloned().collect();
        if distinct.len() != keys.len() {
            return Err(duplicate_keys_error("Namespaced-map", &keys));
        }
        // (zipmap keys vals)
        Ok(Value::map(CljMap::from_pairs(pairs)))
    }

    fn read_tagged(&mut self, tag: &Symbol, form: Value) -> Result<Value> {
        if let Some(tags) = self.tags {
            if let Some(v) = tags(tag.full(), &form)? {
                return Ok(v);
            }
        }
        match tag.full() {
            "inst" => match &form {
                Value::Str(s) => parse_timestamp(s).map(Value::Inst),
                _ => Err(Error::msg("Instance literal expects a string for its timestamp.")),
            },
            "uuid" => match &form {
                Value::Str(s) => Ok(Value::uuid(s)),
                _ => Err(Error::msg("UUID literal expects a string as its representation.")),
            },
            "datascript/Datom" => crate::datom::datom_from_reader(&form).map(|d| Value::Datom(std::sync::Arc::new(d))),
            "datascript/DB" => crate::db::db_from_reader(&form).map(Value::Db),
            other => Err(reader_error(format!("No reader function for tag {other}."))),
        }
    }
}

enum Form {
    Value(Value),
    /// A comment, a discarded form
    Skip,
    Eof,
    Close(char),
}

fn odd_map_error(items: &[Value]) -> Error {
    reader_error(format!(
        "The map literal starting with {} contains {} form(s). Map literals must contain an even number of forms.",
        inspect(items.first().unwrap_or(&Value::Nil), false),
        items.len()
    ))
}

fn make_map(items: Vec<Value>) -> Result<Value> {
    if items.len() % 2 != 0 {
        return Err(odd_map_error(&items));
    }
    let mut pairs = Vec::with_capacity(items.len() / 2);
    let mut it = items.into_iter();
    while let (Some(k), Some(v)) = (it.next(), it.next()) {
        pairs.push((k, v));
    }
    make_map_from_pairs(pairs)
}

fn make_map_from_pairs(pairs: Vec<(Value, Value)>) -> Result<Value> {
    let keys: Vec<Value> = pairs.iter().map(|(k, _)| k.clone()).collect();
    let distinct: CljSet = keys.iter().cloned().collect();
    if distinct.len() != keys.len() {
        return Err(duplicate_keys_error("Map", &keys));
    }
    Ok(Value::map(CljMap::from_literal(pairs)))
}

/// `parse-symbol`: a token's namespace and name, or `None` when it is no symbol.
fn parse_symbol(token: &str) -> Option<(Option<&str>, &str)> {
    if token.is_empty() || token.ends_with(':') || token.starts_with("::") {
        return None;
    }
    match token.find('/') {
        Some(i) if i > 0 => {
            let ns = &token[..i];
            let name = &token[i + 1..];
            if name.is_empty()
                || name.starts_with(|c: char| c.is_ascii_digit())
                || ns.ends_with(':')
                || !(name == "/" || !name.contains('/'))
            {
                return None;
            }
            Some((Some(ns), name))
        }
        _ => {
            if token == "/" || !token.contains('/') {
                Some((None, token))
            } else {
                None
            }
        }
    }
}

/// JavaScript's `parseInt(s, radix)` of digits that are all of the radix.
fn parse_int_radix(digits: &str, radix: u32) -> Option<f64> {
    if digits.is_empty() || !(2..=36).contains(&radix) {
        return None;
    }
    let mut n = 0f64;
    for c in digits.chars() {
        let d = c.to_digit(radix)?;
        n = n * radix as f64 + d as f64;
    }
    Some(n)
}

/// `match-number`: an integer in base 10, 16 (`0x`), 8 (`0…`) or any (`36rZ`), a float, or a ratio, each a JavaScript
/// number. A token shaped like an integer is read as one or not at all: `09` is no number.
fn match_number(s: &str) -> Option<f64> {
    match match_int(s) {
        Some(n) => n,
        None => match_float(s).or_else(|| match_ratio(s)),
    }
}

/// `None` when the token is not shaped like an integer; `Some(None)` when it is, and is none.
fn match_int(s: &str) -> Option<Option<f64>> {
    let (negative, body) = match s.as_bytes().first()? {
        b'-' => (true, &s[1..]),
        b'+' => (false, &s[1..]),
        _ => (false, s),
    };
    let body = body.strip_suffix('N').unwrap_or(body);
    let all = |digits: &str, radix: u32| !digits.is_empty() && digits.chars().all(|c| c.is_digit(radix));
    let sign = |n: f64| if negative { -n } else { n };
    if body == "0" {
        return Some(Some(0.0));
    }
    if let Some(hex) = body.strip_prefix("0x").or_else(|| body.strip_prefix("0X")) {
        return if all(hex, 16) { Some(parse_int_radix(hex, 16).map(sign)) } else { None };
    }
    if let Some(rest) = body.strip_prefix('0') {
        return if all(rest, 8) {
            Some(parse_int_radix(rest, 8).map(sign))
        } else if all(rest, 10) {
            // 0[0-9]+ with a digit that is not octal
            Some(None)
        } else {
            None
        };
    }
    if let Some(i) = body.find(['r', 'R']) {
        let (radix, digits) = (&body[..i], &body[i + 1..]);
        if !(1..=2).contains(&radix.len()) || !all(radix, 10) {
            return None;
        }
        if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_alphanumeric()) {
            return None;
        }
        // parseInt reads the digits the radix has, and stops at the first it has not
        let radix: u32 = radix.parse().ok()?;
        if !(2..=36).contains(&radix) {
            return Some(None);
        }
        let valid: String = digits.chars().take_while(|c| c.is_digit(radix)).collect();
        return Some(parse_int_radix(&valid, radix).map(sign));
    }
    if all(body, 10) {
        return Some(body.parse::<f64>().ok().map(sign));
    }
    None
}

fn match_float(s: &str) -> Option<f64> {
    let body = s.strip_suffix('M').unwrap_or(s);
    let b = body.as_bytes();
    let mut i = 0;
    if matches!(b.first(), Some(b'-') | Some(b'+')) {
        i += 1;
    }
    let digits_start = i;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    if i == digits_start {
        return None;
    }
    if i < b.len() && b[i] == b'.' {
        i += 1;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        i += 1;
        if i < b.len() && (b[i] == b'-' || b[i] == b'+') {
            i += 1;
        }
        let exp_start = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i == exp_start {
            return None;
        }
    }
    if i != b.len() {
        return None;
    }
    // Rust reads "1." and "+1" as JavaScript's parseFloat does
    body.strip_prefix('+').unwrap_or(body).trim_end_matches('.').parse::<f64>().ok().or_else(|| body.parse().ok())
}

fn match_ratio(s: &str) -> Option<f64> {
    let (num, den) = s.split_once('/')?;
    let num_digits = num.strip_prefix(['-', '+']).unwrap_or(num);
    if num_digits.is_empty() || !num_digits.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    if den.is_empty() || !den.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let n: f64 = num.strip_prefix('+').unwrap_or(num).parse().ok()?;
    let d: f64 = den.parse().ok()?;
    Some(n / d)
}

/// `parse-timestamp`: an RFC 3339 instant, whole or cut short at any field, as milliseconds since the epoch.
pub fn parse_timestamp(s: &str) -> Result<f64> {
    let bad = || Error::msg(format!("Unrecognized date/time syntax: {s}"));
    let b = s.as_bytes();
    let mut i = 0;
    let num = |from: usize, len: usize| -> Option<i64> {
        let part = b.get(from..from + len)?;
        if part.iter().all(|c| c.is_ascii_digit()) {
            std::str::from_utf8(part).ok()?.parse().ok()
        } else {
            None
        }
    };
    let years = num(0, 4).ok_or_else(bad)?;
    i += 4;
    let (mut months, mut days, mut hours, mut minutes, mut seconds, mut millis) = (1i64, 1i64, 0i64, 0i64, 0i64, 0i64);
    if b.get(i) == Some(&b'-') {
        if let Some(m) = num(i + 1, 2) {
            months = m;
            i += 3;
            if b.get(i) == Some(&b'-') {
                if let Some(d) = num(i + 1, 2) {
                    days = d;
                    i += 3;
                    if b.get(i) == Some(&b'T') {
                        if let Some(h) = num(i + 1, 2) {
                            hours = h;
                            i += 3;
                            if b.get(i) == Some(&b':') {
                                if let Some(m) = num(i + 1, 2) {
                                    minutes = m;
                                    i += 3;
                                    if b.get(i) == Some(&b':') {
                                        if let Some(sec) = num(i + 1, 2) {
                                            seconds = sec;
                                            i += 3;
                                            if b.get(i) == Some(&b'.') {
                                                let start = i + 1;
                                                let mut j = start;
                                                while j < b.len() && b[j].is_ascii_digit() {
                                                    j += 1;
                                                }
                                                if j > start {
                                                    // the fraction, filled or cut to three digits
                                                    let mut frac: String = s[start..j].chars().take(3).collect();
                                                    while frac.len() < 3 {
                                                        frac.push('0');
                                                    }
                                                    millis = frac.parse().map_err(|_| bad())?;
                                                    i = j;
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    let mut offset = 0i64;
    match b.get(i) {
        None => {}
        Some(b'Z') => i += 1,
        Some(sign @ (b'-' | b'+')) => {
            let h = num(i + 1, 2).ok_or_else(bad)?;
            if b.get(i + 3) != Some(&b':') {
                return Err(bad());
            }
            let m = num(i + 4, 2).ok_or_else(bad)?;
            offset = (h * 60 + m) * if *sign == b'-' { -1 } else { 1 };
            i += 6;
        }
        Some(_) => return Err(bad()),
    }
    if i != b.len() {
        return Err(bad());
    }
    let leap = years % 4 == 0 && (years % 100 != 0 || years % 400 == 0);
    let dim = match months {
        2 => {
            if leap {
                29
            } else {
                28
            }
        }
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    let check = |low: i64, n: i64, high: i64, what: &str| -> Result<()> {
        if low <= n && n <= high {
            Ok(())
        } else {
            Err(Error::msg(format!("{what} Failed:  {low}<={n}<={high}")))
        }
    };
    check(1, months, 12, "timestamp month field must be in range 1..12")?;
    check(1, days, dim, "timestamp day field must be in range 1..last day in month")?;
    check(0, hours, 23, "timestamp hour field must be in range 0..23")?;
    check(0, minutes, 59, "timestamp minute field must be in range 0..59")?;
    check(0, seconds, if minutes == 59 { 60 } else { 59 }, "timestamp second field must be in range 0..60")?;
    check(0, millis, 999, "timestamp millisecond field must be in range 0..999")?;
    let day_ms = days_from_civil(years, months as u32, days as u32) * 86_400_000;
    let ms = day_ms + hours * 3_600_000 + minutes * 60_000 + seconds * 1000 + millis - offset * 60_000;
    Ok(ms as f64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::print::pr_str;

    fn rt(s: &str) -> String {
        pr_str(&read_string(s).unwrap())
    }

    #[test]
    fn scalars() {
        assert_eq!(rt("nil"), "nil");
        assert_eq!(rt("true"), "true");
        assert_eq!(rt("42"), "42");
        assert_eq!(rt("-42"), "-42");
        assert_eq!(rt("+7"), "7");
        assert_eq!(rt("0x1F"), "31");
        assert_eq!(rt("017"), "15");
        assert_eq!(rt("2r101"), "5");
        assert_eq!(rt("36rZ"), "35");
        assert_eq!(rt("1.5"), "1.5");
        assert_eq!(rt("1e3"), "1000");
        assert_eq!(rt("1/2"), "0.5");
        assert_eq!(rt("10N"), "10");
        assert_eq!(rt("1.5M"), "1.5");
        assert_eq!(rt("##Inf"), "##Inf");
        assert_eq!(rt("##NaN"), "##NaN");
        assert_eq!(rt("\"a\\nb\\u0041\""), "\"a\\nbA\"");
        assert_eq!(rt("\\a"), "\"a\"");
        assert_eq!(rt("\\newline"), "\"\\n\"");
        assert_eq!(rt(":a/b"), ":a/b");
        assert_eq!(rt("a/b"), "a/b");
        assert_eq!(rt("?e"), "?e");
        assert_eq!(rt("..."), "...");
        assert_eq!(rt("/"), "/");
        assert_eq!(rt("-"), "-");
        assert_eq!(rt("+"), "+");
        assert_eq!(rt(""), "nil");
        assert!(read_string("09").is_err());
        assert!(read_string("1a").is_err());
        assert!(read_string(":").is_err());
    }

    #[test]
    fn collections() {
        assert_eq!(rt("[1 2 (3 4) #{5} {:a 1, :b 2}]"), "[1 2 (3 4) #{5} {:a 1, :b 2}]");
        assert_eq!(rt("[1 ; comment\n 2 #_ 3 4]"), "[1 2 4]");
        assert_eq!(rt("#:person{:name \"x\" :_/age 3 :other/id 4}"), "{:person/name \"x\", :age 3, :other/id 4}");
        assert_eq!(rt("^{:a 1} [1 2]"), "[1 2]");
        assert!(read_string("{:a 1 :a 2}").is_err());
        assert!(read_string("#{1 1}").is_err());
        assert!(read_string("{:a}").is_err());
        assert!(read_string("[1 2").is_err());
        assert!(read_string("]").is_err());
        assert_eq!(read_all("1 2 [3]").unwrap().len(), 3);
    }

    #[test]
    fn tagged() {
        assert_eq!(rt("#inst \"2020-01-01T00:00:00.000Z\""), "#inst \"2020-01-01T00:00:00.000-00:00\"");
        assert_eq!(rt("#inst \"2020\""), "#inst \"2020-01-01T00:00:00.000-00:00\"");
        assert_eq!(rt("#inst \"2020-01-01T02:00:00+02:00\""), "#inst \"2020-01-01T00:00:00.000-00:00\"");
        assert_eq!(
            rt("#uuid \"550E8400-e29b-41d4-a716-446655440000\""),
            "#uuid \"550e8400-e29b-41d4-a716-446655440000\""
        );
        assert!(read_string("#foo 1").is_err());
        let v = read_string_with("#r x", &|tag, form| {
            Ok(if tag == "r" { Some(Value::vector(vec![Value::str("ref"), form.clone()])) } else { None })
        })
        .unwrap();
        assert_eq!(pr_str(&v), "[\"ref\" x]");
    }
}
