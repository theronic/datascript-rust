//! The form values cross the module's boundary in: a tag byte, then what the tag says follows. Integers are
//! LEB128 varints, signed ones zigzagged; strings are their length in bytes and UTF-8.
//!
//! A keyword or a symbol is its number: the module numbers every name it meets, for good, and a host learns a
//! name's number, or a number's name, once, by asking (`ds_intern`, `ds_name_ptr`, `ds_name_len`).
//!
//! A map or a set says which of ClojureScript's two shapes it has, since that is what its order of iteration is:
//! `ARRAY_*` keeps the order its entries come in, `HASH_*` is ordered by its keys' hashes.

use crate::state::with_state;
use datascript::coll::{CljMap, CljSet};
use datascript::datom::value_attr;
use datascript::value::Regex;
use datascript::{Datom, Error, Keyword, Result, Symbol, Value};
use std::sync::Arc;

pub const NIL: u8 = 0;
pub const FALSE: u8 = 1;
pub const TRUE: u8 = 2;
/// A whole number of at most 32 bits: zigzag varint
pub const INT: u8 = 3;
/// Any other number: 8 bytes, little-endian
pub const F64: u8 = 4;
pub const STR: u8 = 5;
/// A keyword: its number, which is the module's for it for good (`ds_intern` gives a name's number, `ds_name_ptr`
/// and `ds_name_len` a number's name)
pub const KW: u8 = 6;
/// A symbol: its number, likewise
pub const SYM: u8 = 8;
/// count, elements
pub const VECTOR: u8 = 10;
pub const LIST: u8 = 11;
/// count, then key and value for each entry
pub const ARRAY_MAP: u8 = 12;
pub const HASH_MAP: u8 = 13;
pub const ARRAY_SET: u8 = 14;
pub const HASH_SET: u8 = 15;
/// its string
pub const UUID: u8 = 16;
/// milliseconds: 8 bytes
pub const INST: u8 = 17;
/// source, flags
pub const REGEX: u8 = 18;
/// e (zigzag), a, v, tx (zigzag), added (a byte)
pub const DATOM: u8 = 19;
/// a database's handle, as the host names one
pub const DB: u8 = 20;
/// a function of the host's: its handle there
pub const HOST_FN: u8 = 21;
/// a value of the host's that the module does not look into: its handle there, its hash (zigzag)
pub const HOST_OBJ: u8 = 22;
/// a function of the module's: its handle here, its name
pub const MODULE_FN: u8 = 23;
/// a database as the module names one: its handle, its schema's number, max-eid and max-tx (zigzag), and whether
/// it is filtered (a byte)
pub const DB_INFO: u8 = 24;
/// a type, as `type` answers one: its name, which the host has the constructor of
pub const TYPE: u8 = 25;
/// a run of datoms, as the module answers an index read with: how many (4 bytes, little-endian), then of each its
/// e (zigzag), a, v, and tx (zigzag), which is negative for a datom that was retracted
pub const DATOMS: u8 = 26;

pub struct Writer {
    pub buf: Vec<u8>,
}

impl Default for Writer {
    fn default() -> Self {
        Writer::new()
    }
}

thread_local! {
    /// What writers wrote into, kept for the next: a call writes at least one message, and a query that asks the
    /// host of every row writes one a row.
    static SPARE: datascript::lock::Slot<Vec<Vec<u8>>> = const { datascript::lock::Slot::new(Vec::new()) };
}

/// A buffer that held a message, for a writer to come. Large ones are let go.
pub fn recycle(mut buf: Vec<u8>) {
    if buf.capacity() < 64 || buf.capacity() > 1 << 16 {
        return;
    }
    buf.clear();
    // not kept as a thread ends, when there is nowhere to keep it
    let _ = SPARE.try_with(|spare| {
        let mut spare = spare.get();
        if spare.len() < 8 {
            spare.push(buf);
        }
    });
}

impl Drop for Writer {
    fn drop(&mut self) {
        recycle(std::mem::take(&mut self.buf));
    }
}

impl Writer {
    pub fn new() -> Writer {
        let buf = SPARE.try_with(|spare| spare.get().pop()).ok().flatten();
        Writer { buf: buf.unwrap_or_else(|| Vec::with_capacity(256)) }
    }

    /// What was written, which is the caller's from here on.
    pub fn into_bytes(mut self) -> Vec<u8> {
        std::mem::take(&mut self.buf)
    }

    #[inline]
    pub fn byte(&mut self, b: u8) {
        self.buf.push(b)
    }

    pub fn varint(&mut self, mut n: u64) {
        while n >= 0x80 {
            self.buf.push((n as u8) | 0x80);
            n >>= 7;
        }
        self.buf.push(n as u8);
    }

    pub fn zigzag(&mut self, n: i64) {
        self.varint(((n << 1) ^ (n >> 63)) as u64)
    }

    pub fn f64(&mut self, n: f64) {
        self.buf.extend_from_slice(&n.to_le_bytes())
    }

    pub fn str(&mut self, s: &str) {
        self.varint(s.len() as u64);
        self.buf.extend_from_slice(s.as_bytes());
    }

    pub fn count(&mut self, tag: u8, n: usize) {
        self.byte(tag);
        self.varint(n as u64);
    }

    pub fn number(&mut self, n: f64) {
        // -0 keeps its sign as a double
        if n.fract() == 0.0 && n.abs() < 2147483648.0 && !(n == 0.0 && n.is_sign_negative()) {
            self.byte(INT);
            self.zigzag(n as i64);
        } else {
            self.byte(F64);
            self.f64(n);
        }
    }

    #[inline]
    pub fn keyword(&mut self, k: &Keyword) {
        self.byte(KW);
        self.varint(k.id() as u64);
    }

    /// Begins a run of datoms: where its count goes, once it is known (`datoms_end`).
    pub fn datoms_begin(&mut self) -> usize {
        self.byte(DATOMS);
        self.buf.extend_from_slice(&[0; 4]);
        self.buf.len() - 4
    }

    /// One datom of a run.
    pub fn datom_of_run(&mut self, d: &Datom) {
        self.zigzag(d.e as i64);
        match d.a.as_keyword() {
            Some(k) => self.keyword(&k),
            None => {
                self.byte(STR);
                self.str(d.a.full());
            }
        }
        self.value(&d.v);
        self.zigzag(if d.added() { d.tx() as i64 } else { -(d.tx() as i64) });
    }

    pub fn datoms_end(&mut self, begun: usize, count: usize) {
        self.buf[begun..begun + 4].copy_from_slice(&(count as u32).to_le_bytes());
    }

    /// A value. One nested thousands deep, as a recursive pull makes, is written from a list of what is still to
    /// write and not by a call for every level of it: WebAssembly's stack has no room for that many.
    pub fn value(&mut self, v: &Value) {
        if self.flat(v) {
            return;
        }
        let mut todo = vec![Todo::Value(v)];
        while let Some(next) = todo.pop() {
            match next {
                Todo::Value(v) => self.open(v, &mut todo),
                Todo::DatomEnd(tx, added) => {
                    self.zigzag(tx);
                    self.byte(added as u8);
                }
            }
        }
    }

    /// Writes a value that holds no collection, whole, and answers true. Of one that does, writes nothing and
    /// answers false.
    fn flat(&mut self, v: &Value) -> bool {
        match v {
            Value::Nil => self.byte(NIL),
            Value::Bool(false) => self.byte(FALSE),
            Value::Bool(true) => self.byte(TRUE),
            Value::Num(n) => self.number(*n),
            Value::Str(s) => {
                self.byte(STR);
                self.str(s);
            }
            Value::Keyword(k) => self.keyword(k),
            Value::Symbol(s) => {
                self.byte(SYM);
                self.varint(s.id() as u64);
            }
            Value::Vector(_) | Value::List(_) | Value::Map(_) | Value::Set(_) => return false,
            Value::Uuid(s) => {
                self.byte(UUID);
                self.str(s);
            }
            Value::Inst(ms) => {
                self.byte(INST);
                self.f64(*ms);
            }
            Value::Regex(r) => {
                self.byte(REGEX);
                self.str(&r.source);
                self.str(&r.flags);
            }
            Value::Datom(d) => {
                if d.v.is_coll() {
                    return false;
                }
                self.datom_head(d);
                self.flat(&d.v);
                self.zigzag(d.tx() as i64);
                self.byte(d.added() as u8);
            }
            Value::Db(db) => {
                let handle = with_state(|state| state.db_handle(db));
                self.byte(DB_INFO);
                self.varint(handle as u64);
                self.varint(db.schema().uid() as u64);
                self.zigzag(db.max_eid() as i64);
                self.zigzag(db.max_tx() as i64);
                self.byte(db.is_filtered() as u8);
            }
            Value::Fn(f) => match crate::host::fn_handle(f) {
                Some(handle) => {
                    self.byte(HOST_FN);
                    self.varint(handle as u64);
                }
                None => match datascript::built_ins::type_name_of(f) {
                    Some(name) => {
                        self.byte(TYPE);
                        self.str(&name);
                    }
                    None => {
                        let handle = with_state(|state| state.module_fn_handle(f));
                        self.byte(MODULE_FN);
                        self.varint(handle as u64);
                        self.str(f.name());
                    }
                },
            },
            Value::Host(h) => match crate::host::object_handle(h) {
                Some((handle, hash)) => {
                    self.byte(HOST_OBJ);
                    self.varint(handle as u64);
                    self.zigzag(hash as i64);
                }
                // a value of the module's own that the host has no form for: as it prints
                None => {
                    self.byte(STR);
                    self.str(&datascript::print::pr_str(v));
                }
            },
        }
        true
    }

    /// A datom up to its value: its entity and its attribute.
    fn datom_head(&mut self, d: &Datom) {
        self.byte(DATOM);
        self.zigzag(d.e as i64);
        self.flat(&d.a_value());
    }

    /// Begins a value that holds collections: what it starts with, and its elements.
    fn open<'a>(&mut self, v: &'a Value, todo: &mut Vec<Todo<'a>>) {
        match v {
            Value::Vector(items) => {
                self.count(VECTOR, items.len());
                self.elements(items.iter(), todo);
            }
            Value::List(items) => {
                self.count(LIST, items.len());
                self.elements(items.iter(), todo);
            }
            Value::Map(m) => {
                self.count(if m.is_array() { ARRAY_MAP } else { HASH_MAP }, m.len());
                self.elements(m.iter().flat_map(|(k, x)| [k, x]), todo);
            }
            Value::Set(s) => {
                self.count(if s.is_array() { ARRAY_SET } else { HASH_SET }, s.len());
                self.elements(s.iter(), todo);
            }
            Value::Datom(d) if d.v.is_coll() => {
                self.datom_head(d);
                todo.push(Todo::DatomEnd(d.tx() as i64, d.added()));
                todo.push(Todo::Value(&d.v));
            }
            flat => {
                self.flat(flat);
            }
        }
    }

    /// A collection's elements, in their order: written here up to the first that holds collections, and from
    /// there on left to the list.
    fn elements<'a>(&mut self, items: impl Iterator<Item = &'a Value>, todo: &mut Vec<Todo<'a>>) {
        let start = todo.len();
        for item in items {
            if todo.len() > start || !self.flat(item) {
                todo.push(Todo::Value(item));
            }
        }
        todo[start..].reverse();
    }
}

/// What is still to be written, the next on top.
enum Todo<'a> {
    Value(&'a Value),
    /// What follows a datom's value: its transaction, and whether it was added
    DatomEnd(i64, bool),
}

pub struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

fn truncated() -> Error {
    Error::msg("datascript: the message ends before its value does")
}

impl<'a> Reader<'a> {
    pub fn new(bytes: &'a [u8]) -> Reader<'a> {
        Reader { bytes, pos: 0 }
    }

    pub fn is_done(&self) -> bool {
        self.pos >= self.bytes.len()
    }

    #[inline]
    pub fn byte(&mut self) -> Result<u8> {
        let b = *self.bytes.get(self.pos).ok_or_else(truncated)?;
        self.pos += 1;
        Ok(b)
    }

    pub fn varint(&mut self) -> Result<u64> {
        let mut n = 0u64;
        let mut shift = 0;
        loop {
            let b = self.byte()?;
            n |= ((b & 0x7f) as u64) << shift;
            if b & 0x80 == 0 {
                return Ok(n);
            }
            shift += 7;
            if shift > 63 {
                return Err(Error::msg("datascript: a number in the message is too long"));
            }
        }
    }

    pub fn zigzag(&mut self) -> Result<i64> {
        let n = self.varint()?;
        Ok(((n >> 1) as i64) ^ -((n & 1) as i64))
    }

    pub fn f64(&mut self) -> Result<f64> {
        let bytes = self.bytes.get(self.pos..self.pos + 8).ok_or_else(truncated)?;
        self.pos += 8;
        Ok(f64::from_le_bytes(bytes.try_into().expect("8 bytes")))
    }

    pub fn str(&mut self) -> Result<&'a str> {
        let len = self.varint()? as usize;
        let bytes = self.bytes.get(self.pos..self.pos + len).ok_or_else(truncated)?;
        self.pos += len;
        std::str::from_utf8(bytes).map_err(|_| Error::msg("datascript: a string in the message is not UTF-8"))
    }

    /// A value. Like the writer, it keeps the collections it is in the middle of in a list, and makes no call
    /// for a level of nesting.
    pub fn value(&mut self) -> Result<Value> {
        let mut open: Vec<Partial> = Vec::new();
        loop {
            let Some(mut value) = self.begin(&mut open)? else { continue };
            // into the collection it is an element of, which it may complete, and so on outwards
            loop {
                let Some(around) = open.last_mut() else { return Ok(value) };
                if !around.take(value) {
                    break;
                }
                value = self.finish(open.pop().expect("a collection is open"))?;
            }
        }
    }

    /// As many elements as a count says, as far as the message can hold them: a count is no promise, and what is
    /// not there is not allocated for.
    fn room<T>(&self, n: usize) -> Vec<T> {
        Vec::with_capacity(n.min(self.bytes.len() - self.pos))
    }

    /// Reads a value that holds no other, whole; or the beginning of one that does, which is then open.
    fn begin(&mut self, open: &mut Vec<Partial>) -> Result<Option<Value>> {
        let tag = self.byte()?;
        Ok(Some(match tag {
            NIL => Value::Nil,
            FALSE => Value::Bool(false),
            TRUE => Value::Bool(true),
            INT => Value::Num(self.zigzag()? as f64),
            F64 => Value::Num(self.f64()?),
            STR => Value::str(self.str()?),
            KW => {
                let id = self.varint()? as u32;
                Value::Keyword(Keyword::from_id(id).ok_or_else(|| Error::msg("datascript: no keyword of that number"))?)
            }
            SYM => {
                let id = self.varint()? as u32;
                Value::Symbol(Symbol::from_id(id).ok_or_else(|| Error::msg("datascript: no symbol of that number"))?)
            }
            VECTOR | LIST | ARRAY_SET | HASH_SET => {
                let left = self.varint()? as usize;
                let partial = Partial::Items { tag, items: self.room(left), left };
                if left == 0 {
                    return self.finish(partial).map(Some);
                }
                open.push(partial);
                return Ok(None);
            }
            ARRAY_MAP | HASH_MAP => {
                let left = self.varint()? as usize;
                let partial = Partial::Pairs { tag, pairs: self.room(left), key: None, left };
                if left == 0 {
                    return self.finish(partial).map(Some);
                }
                open.push(partial);
                return Ok(None);
            }
            UUID => Value::Uuid(Arc::from(self.str()?)),
            INST => Value::Inst(self.f64()?),
            REGEX => {
                let source = self.str()?;
                let flags = self.str()?;
                Value::Regex(Arc::new(Regex::new(source, flags)))
            }
            DATOM => {
                open.push(Partial::Datom { e: self.zigzag()?, a: None, v: None });
                return Ok(None);
            }
            DB => {
                let handle = self.varint()? as u32;
                Value::Db(with_state(|state| state.db(handle))?)
            }
            DB_INFO => {
                let handle = self.varint()? as u32;
                let (_schema, _max_eid, _max_tx, _filtered) =
                    (self.varint()?, self.zigzag()?, self.zigzag()?, self.byte()?);
                Value::Db(with_state(|state| state.db(handle))?)
            }
            HOST_FN => {
                let handle = self.varint()? as u32;
                Value::Fn(with_state(|state| crate::host::host_fn(handle, state)))
            }
            HOST_OBJ => {
                let handle = self.varint()? as u32;
                let hash = self.zigzag()? as i32;
                crate::host::host_object(handle, hash)
            }
            MODULE_FN => {
                let handle = self.varint()? as u32;
                let _name = self.str()?;
                Value::Fn(with_state(|state| state.module_fn(handle))?)
            }
            TYPE => Value::Fn(datascript::built_ins::type_named(self.str()?)),
            other => return Err(Error::msg(format!("datascript: the message has a value of no known kind ({other})"))),
        }))
    }

    /// The value of a collection that has all its elements; of a datom, with what follows its value.
    fn finish(&mut self, partial: Partial) -> Result<Value> {
        Ok(match partial {
            Partial::Items { tag: VECTOR, items, .. } => Value::vector(items),
            Partial::Items { tag: LIST, items, .. } => Value::list(items),
            Partial::Items { tag: ARRAY_SET, items, .. } => Value::set(CljSet::array_set_of_distinct(items)),
            Partial::Items { items, .. } => Value::set(CljSet::hash_set(items)),
            Partial::Pairs { tag: ARRAY_MAP, pairs, .. } => Value::map(CljMap::array_map_of_distinct(pairs)),
            Partial::Pairs { pairs, .. } => Value::map(CljMap::hash_map(pairs)),
            Partial::Datom { e, a, v } => {
                let tx = self.zigzag()?;
                let added = self.byte()? != 0;
                let attr = value_attr(&a.unwrap_or(Value::Nil))
                    .ok_or_else(|| Error::msg("datascript: a datom's attribute is a keyword or a string"))?;
                let id = |n: i64| i32::try_from(n).map_err(|_| Error::msg("datascript: a datom's ids are 32 bits"));
                Value::Datom(Arc::new(Datom::with_added(id(e)?, attr, v.unwrap_or(Value::Nil), id(tx)?, added)))
            }
        })
    }
}

/// A collection that is being read: what it has so far, and how much is to come.
enum Partial {
    Items {
        tag: u8,
        items: Vec<Value>,
        left: usize,
    },
    Pairs {
        tag: u8,
        pairs: Vec<(Value, Value)>,
        key: Option<Value>,
        left: usize,
    },
    /// A datom, whose attribute and value are values like any other
    Datom {
        e: i64,
        a: Option<Value>,
        v: Option<Value>,
    },
}

impl Partial {
    /// Its next element. True when it was the last.
    fn take(&mut self, value: Value) -> bool {
        match self {
            Partial::Items { items, left, .. } => {
                items.push(value);
                *left -= 1;
                *left == 0
            }
            Partial::Pairs { pairs, key, left, .. } => match key.take() {
                None => {
                    *key = Some(value);
                    false
                }
                Some(k) => {
                    pairs.push((k, value));
                    *left -= 1;
                    *left == 0
                }
            },
            Partial::Datom { a, v, .. } => {
                if a.is_none() {
                    *a = Some(value);
                    false
                } else {
                    *v = Some(value);
                    true
                }
            }
        }
    }
}
