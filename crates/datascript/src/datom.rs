//! Datoms: the facts `[e a v tx]`, and the three orders DataScript indexes them in.

use crate::cmp::{value_cmp, value_compare};
use crate::error::{Error, Result};
use crate::hash::hash_combine;
use crate::named::{Attr, Keyword};
use crate::value::Value;
use std::cmp::Ordering;

/// The first entity id
pub const E0: i32 = 0;
/// The first transaction id; entity ids stay below it
pub const TX0: i32 = 0x2000_0000;
pub const EMAX: i32 = 0x7FFF_FFFF;
pub const TXMAX: i32 = 0x7FFF_FFFF;

/// A fact. The transaction id is negative on a datom that retracts, as DataScript keeps it; the indexes hold only
/// assertions.
#[derive(Clone)]
pub struct Datom {
    pub e: i32,
    tx: i32,
    pub a: Attr,
    pub v: Value,
}

impl Datom {
    #[inline]
    pub fn new(e: i32, a: Attr, v: Value, tx: i32) -> Datom {
        Datom { e, tx, a, v }
    }

    #[inline]
    pub fn with_added(e: i32, a: Attr, v: Value, tx: i32, added: bool) -> Datom {
        Datom { e, tx: if added { tx } else { -tx }, a, v }
    }

    /// `datom-tx`: the transaction, whether it asserted or retracted
    #[inline]
    pub fn tx(&self) -> i32 {
        self.tx.abs()
    }

    /// `datom-added`
    #[inline]
    pub fn added(&self) -> bool {
        self.tx > 0
    }

    /// The attribute as a value: its keyword, or its string
    #[inline]
    pub fn a_value(&self) -> Value {
        attr_value(&self.a)
    }

    /// `equiv-datom`: by entity, attribute and value
    pub fn equiv(&self, other: &Datom) -> bool {
        self.e == other.e && self.a == other.a && self.v == other.v
    }

    /// `hash-datom`
    pub fn cljs_hash(&self) -> i32 {
        let h = crate::hash::hash_number(self.e as f64);
        let h = hash_combine(h, self.a.hash());
        hash_combine(h, self.v.cljs_hash())
    }

    /// `(seq datom)`: `(e a v tx added)`
    pub fn seq_values(&self) -> Vec<Value> {
        vec![Value::from(self.e), self.a_value(), self.v.clone(), Value::from(self.tx()), Value::Bool(self.added())]
    }

    /// `(nth datom i)`
    pub fn nth(&self, i: usize) -> Option<Value> {
        match i {
            0 => Some(Value::from(self.e)),
            1 => Some(self.a_value()),
            2 => Some(self.v.clone()),
            3 => Some(Value::from(self.tx())),
            4 => Some(Value::Bool(self.added())),
            _ => None,
        }
    }

    /// `(get datom k)`: by `:e :a :v :tx :added`, or the same as strings
    pub fn val_at(&self, k: &Value) -> Option<Value> {
        let name = match k {
            Value::Keyword(k) if k.ns().is_none() => k.name(),
            Value::Str(s) => &**s,
            _ => return None,
        };
        match name {
            "e" => self.nth(0),
            "a" => self.nth(1),
            "v" => self.nth(2),
            "tx" => self.nth(3),
            "added" => self.nth(4),
            _ => None,
        }
    }
}

impl std::fmt::Debug for Datom {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut out = String::new();
        crate::print::pr_datom(&mut out, self);
        f.write_str(&out)
    }
}

#[inline]
pub fn attr_value(a: &Attr) -> Value {
    match a.as_keyword() {
        Some(k) => Value::Keyword(k),
        None => Value::Str(a.full_arc().clone()),
    }
}

/// An attribute from a value: a keyword or a string.
#[inline]
pub fn value_attr(v: &Value) -> Option<Attr> {
    match v {
        Value::Keyword(k) => Some(Attr::keyword(k)),
        Value::Str(s) => Some(Attr::string(s)),
        _ => None,
    }
}

impl From<&Keyword> for Attr {
    fn from(k: &Keyword) -> Attr {
        Attr::keyword(k)
    }
}

/// An entity or transaction id from a number, which must be whole and within DataScript's range.
pub fn id_from_num(n: f64) -> Result<i32> {
    if n.fract() == 0.0 && n >= i32::MIN as f64 && n <= i32::MAX as f64 {
        Ok(n as i32)
    } else {
        Err(Error::no_such_id(format!(
            "Entity and transaction ids are whole numbers below 2^31, got {}",
            crate::print::number_to_string(n)
        )))
    }
}

/// `datom-from-reader`: `#datascript/Datom [e a v tx added]`, the last two optional.
pub fn datom_from_reader(form: &Value) -> Result<Datom> {
    let bad = || Error::msg(format!("Cannot read a datom from {}", crate::print::pr_str(form)));
    let items = form.as_seq().ok_or_else(bad)?;
    if !(3..=5).contains(&items.len()) {
        return Err(bad());
    }
    let e = id_from_num(items[0].as_num().ok_or_else(bad)?)?;
    let a = value_attr(&items[1]).ok_or_else(bad)?;
    let tx = match items.get(3) {
        Some(t) => id_from_num(t.as_num().ok_or_else(bad)?)?,
        None => TX0,
    };
    let added = items.get(4).is_none_or(Value::truthy);
    Ok(Datom::with_added(e, a, items[2].clone(), tx, added))
}

/// A bound of a search in an index: a datom whose attribute and value may be left open, which then match any.
#[derive(Clone)]
pub struct Bound {
    pub e: i32,
    pub a: Option<Attr>,
    /// `nil` leaves the value open
    pub v: Value,
    pub tx: i32,
}

impl Bound {
    #[inline]
    pub fn new(e: i32, a: Option<Attr>, v: Value, tx: i32) -> Bound {
        Bound { e, a, v, tx }
    }

    /// The bound a datom itself makes.
    pub fn of(d: &Datom) -> Bound {
        Bound { e: d.e, a: Some(d.a.clone()), v: d.v.clone(), tx: d.tx() }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Index {
    Eavt,
    Aevt,
    Avet,
}

impl Index {
    pub fn from_value(v: &Value) -> Option<Index> {
        let name = match v {
            Value::Keyword(k) if k.ns().is_none() => k.name(),
            // DataScript's JavaScript API names an index ":eavt"
            Value::Str(s) => s.strip_prefix(':').unwrap_or(s),
            _ => return None,
        };
        match name {
            "eavt" => Some(Index::Eavt),
            "aevt" => Some(Index::Aevt),
            "avet" => Some(Index::Avet),
            _ => None,
        }
    }

    pub fn keyword(self) -> Value {
        Value::kw(match self {
            Index::Eavt => "eavt",
            Index::Aevt => "aevt",
            Index::Avet => "avet",
        })
    }

    /// `cmp-datoms-*-quick`: the order between two datoms.
    #[inline]
    pub fn cmp(self, d1: &Datom, d2: &Datom) -> Ordering {
        match self {
            Index::Eavt => {
                d1.e.cmp(&d2.e)
                    .then_with(|| d1.a.cmp(&d2.a))
                    .then_with(|| value_compare(&d1.v, &d2.v))
                    .then_with(|| d1.tx().cmp(&d2.tx()))
            }
            Index::Aevt => {
                d1.a.cmp(&d2.a)
                    .then_with(|| d1.e.cmp(&d2.e))
                    .then_with(|| value_compare(&d1.v, &d2.v))
                    .then_with(|| d1.tx().cmp(&d2.tx()))
            }
            Index::Avet => {
                d1.a.cmp(&d2.a)
                    .then_with(|| value_compare(&d1.v, &d2.v))
                    .then_with(|| d1.e.cmp(&d2.e))
                    .then_with(|| d1.tx().cmp(&d2.tx()))
            }
        }
    }

    /// `cmp-datoms-*`: a datom against a bound, whose open components match anything.
    #[inline]
    pub fn cmp_bound(self, d: &Datom, b: &Bound) -> Ordering {
        #[inline]
        fn attr(d: &Datom, b: &Bound) -> Ordering {
            match &b.a {
                Some(a) => d.a.cmp(a),
                None => Ordering::Equal,
            }
        }
        match self {
            Index::Eavt => {
                d.e.cmp(&b.e)
                    .then_with(|| attr(d, b))
                    .then_with(|| value_cmp(&d.v, &b.v))
                    .then_with(|| d.tx().cmp(&b.tx))
            }
            Index::Aevt => attr(d, b)
                .then_with(|| d.e.cmp(&b.e))
                .then_with(|| value_cmp(&d.v, &b.v))
                .then_with(|| d.tx().cmp(&b.tx)),
            Index::Avet => attr(d, b)
                .then_with(|| value_cmp(&d.v, &b.v))
                .then_with(|| d.e.cmp(&b.e))
                .then_with(|| d.tx().cmp(&b.tx)),
        }
    }
}
