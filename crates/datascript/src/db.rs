//! The database: an immutable value of three indexes over one set of datoms, a schema, and two counters. Searching
//! it, walking its indexes, and resolving entity ids.

use crate::datom::{attr_value, value_attr, Bound, Datom, Index, E0, EMAX, TX0, TXMAX};
use crate::error::{Error, Result};
use crate::hash::{hash_combine, hash_unordered};
use crate::named::Attr;
use crate::schema::{kw, AttrProps, Schema};
use crate::sorted_set::{Slice, SortedSet};
use crate::value::Value;
use crate::{message, raise};
use std::cmp::Ordering;
use std::sync::{Arc, OnceLock};

/// A predicate over datoms: a filtered database's.
pub type Pred = Arc<dyn Fn(&Datom) -> Result<bool> + Send + Sync>;

/// A database value. Cloning it is cheap, and a clone is the same value: `ptr_eq` holds between them, as
/// `identical?` holds in ClojureScript.
#[derive(Clone)]
pub struct Db(pub(crate) Arc<DbRepr>);

pub(crate) enum DbRepr {
    Plain(DbCore),
    Filtered(Filtered),
}

/// The database itself: `datascript.db/DB`.
#[derive(Clone)]
pub struct DbCore {
    pub(crate) schema: Arc<Schema>,
    pub(crate) eavt: SortedSet<Datom>,
    pub(crate) aevt: SortedSet<Datom>,
    pub(crate) avet: SortedSet<Datom>,
    pub(crate) max_eid: i32,
    pub(crate) max_tx: i32,
    pub(crate) hash: OnceLock<i32>,
}

/// `datascript.db/FilteredDB`: a view of a database through a predicate.
pub(crate) struct Filtered {
    unfiltered: Db,
    pred: Pred,
    hash: OnceLock<i32>,
}

/// What searching needs of a database: the indexes, and the predicate of a filtered one. A transaction searches the
/// database it is building through this too.
pub trait Searchable {
    fn core(&self) -> &DbCore;
    fn pred(&self) -> Option<&Pred>;
}

impl Searchable for DbCore {
    #[inline]
    fn core(&self) -> &DbCore {
        self
    }

    #[inline]
    fn pred(&self) -> Option<&Pred> {
        None
    }
}

impl Searchable for Db {
    #[inline]
    fn core(&self) -> &DbCore {
        match &*self.0 {
            DbRepr::Plain(core) => core,
            DbRepr::Filtered(f) => f.unfiltered.core(),
        }
    }

    #[inline]
    fn pred(&self) -> Option<&Pred> {
        match &*self.0 {
            DbRepr::Plain(_) => None,
            DbRepr::Filtered(f) => Some(&f.pred),
        }
    }
}

/// Datoms found: a run of an index, forwards or backwards, less those a search's value or transaction, or a filtered
/// database's predicate, leave out. Nothing is read until it is asked for.
#[derive(Clone)]
pub struct Datoms {
    slice: Slice<Datom>,
    rev: bool,
    v: Option<Value>,
    tx: Option<i32>,
    pred: Option<Pred>,
    /// What finding them failed with: it is theirs to raise when they are read
    failed: Option<Error>,
}

impl Datoms {
    fn new(slice: Slice<Datom>, pred: Option<&Pred>) -> Datoms {
        Datoms { slice, rev: false, v: None, tx: None, pred: pred.cloned(), failed: None }
    }

    pub fn empty() -> Datoms {
        Datoms { slice: Slice::empty(), rev: false, v: None, tx: None, pred: None, failed: None }
    }

    #[inline]
    fn check(&self) -> Result<()> {
        match &self.failed {
            Some(e) => Err(e.clone()),
            None => Ok(()),
        }
    }

    fn keep(&self, d: &Datom) -> Result<bool> {
        if let Some(v) = &self.v {
            if d.v != *v {
                return Ok(false);
            }
        }
        if let Some(tx) = self.tx {
            if d.tx() != tx {
                return Ok(false);
            }
        }
        match &self.pred {
            Some(pred) => pred(d),
            None => Ok(true),
        }
    }

    #[inline]
    fn unfiltered(&self) -> bool {
        self.v.is_none() && self.tx.is_none() && self.pred.is_none()
    }

    /// Each datom in turn, until `f` answers false.
    pub fn try_for_each(&self, mut f: impl FnMut(&Datom) -> Result<bool>) -> Result<()> {
        self.check()?;
        let plain = self.unfiltered();
        let mut step = |d: &Datom| -> Result<bool> {
            if plain || self.keep(d)? {
                f(d)
            } else {
                Ok(true)
            }
        };
        if self.rev {
            for d in self.slice.iter_rev() {
                if !step(d)? {
                    break;
                }
            }
        } else {
            for d in self.slice.iter() {
                if !step(d)? {
                    break;
                }
            }
        }
        Ok(())
    }

    pub fn first(&self) -> Result<Option<Datom>> {
        self.check()?;
        if self.unfiltered() && !self.rev {
            return Ok(self.slice.first().cloned());
        }
        let mut found = None;
        self.try_for_each(|d| {
            found = Some(d.clone());
            Ok(false)
        })?;
        Ok(found)
    }

    pub fn is_empty(&self) -> Result<bool> {
        self.check()?;
        if self.unfiltered() {
            return Ok(self.slice.is_empty());
        }
        Ok(self.first()?.is_none())
    }

    pub fn to_vec(&self) -> Result<Vec<Datom>> {
        self.check()?;
        if self.unfiltered() && !self.rev {
            return Ok(self.slice.to_vec());
        }
        let mut out = Vec::new();
        self.try_for_each(|d| {
            out.push(d.clone());
            Ok(true)
        })?;
        Ok(out)
    }

    /// At most `n` datoms.
    pub fn take(&self, n: usize) -> Result<Vec<Datom>> {
        let mut out = Vec::new();
        if n == 0 {
            return Ok(out);
        }
        self.try_for_each(|d| {
            out.push(d.clone());
            Ok(out.len() < n)
        })?;
        Ok(out)
    }

    /// At most `n` datoms after the first `skip`, and whether there are more: how a host reads a long run of an
    /// index a part at a time.
    pub fn page(&self, skip: usize, n: usize) -> Result<(Vec<Datom>, bool)> {
        let mut out = Vec::new();
        let mut seen = 0usize;
        let mut more = false;
        self.try_for_each(|d| {
            if seen < skip {
                seen += 1;
                return Ok(true);
            }
            if out.len() == n {
                more = true;
                return Ok(false);
            }
            out.push(d.clone());
            Ok(true)
        })?;
        Ok((out, more))
    }

    pub fn count(&self) -> Result<usize> {
        self.check()?;
        if self.unfiltered() {
            return Ok(self.slice.count());
        }
        let mut n = 0;
        self.try_for_each(|_| {
            n += 1;
            Ok(true)
        })?;
        Ok(n)
    }

    /// The same datoms from the other end, as `rseq` of a slice.
    pub fn reversed(mut self) -> Datoms {
        self.rev = !self.rev;
        self
    }

    /// Where reading starts: a place to read a part at a time from, as a lazy sequence is read.
    pub fn cursor(&self) -> Cursor {
        let (from, to) = self.slice.bounds();
        Cursor { pos: if self.rev { to } else { from }, done: self.slice.is_empty() }
    }

    /// The next datoms from a cursor, at most `n`, which moves the cursor past them. An empty answer is the end.
    pub fn next_chunk(&self, cursor: &mut Cursor, n: usize) -> Result<Vec<Datom>> {
        self.check()?;
        let mut out = Vec::new();
        if cursor.done || n == 0 {
            return Ok(out);
        }
        let plain = self.unfiltered();
        if self.rev {
            let mut it = self.slice.iter_rev_from(cursor.pos);
            loop {
                match it.next() {
                    None => {
                        cursor.done = true;
                        break;
                    }
                    Some(d) => {
                        if plain || self.keep(d)? {
                            out.push(d.clone());
                            if out.len() == n {
                                break;
                            }
                        }
                    }
                }
            }
            cursor.pos = it.pos();
        } else {
            let mut it = self.slice.iter_from(cursor.pos);
            loop {
                match it.next() {
                    None => {
                        cursor.done = true;
                        break;
                    }
                    Some(d) => {
                        if plain || self.keep(d)? {
                            out.push(d.clone());
                            if out.len() == n {
                                break;
                            }
                        }
                    }
                }
            }
            cursor.pos = it.pos();
        }
        Ok(out)
    }
}

/// A place in a run of datoms that is read a part at a time.
#[derive(Clone, Copy)]
pub struct Cursor {
    pos: crate::sorted_set::Pos,
    done: bool,
}

impl Cursor {
    pub fn is_done(&self) -> bool {
        self.done
    }
}

// ---------------------------------------------------------------- the schema's answers

#[inline]
pub(crate) fn props<'a, D: Searchable>(db: &'a D, a: &Attr) -> &'a AttrProps {
    db.core().schema.props(a)
}

#[inline]
pub(crate) fn props_of<'a, D: Searchable>(db: &'a D, a: &Value) -> &'a AttrProps {
    db.core().schema.props_of(a)
}

// ---------------------------------------------------------------- validation

/// `validate-attr`: an attribute is a keyword or a string.
pub(crate) fn validate_attr(a: &Value, at: impl FnOnce() -> Value) -> Result<Attr> {
    match value_attr(a) {
        Some(attr) => Ok(attr),
        None => {
            let at = at();
            raise!("Bad entity attribute ", a, " at ", at, ", expected keyword or string";
                {"error" => Value::kw("transact/syntax"), "attribute" => a.clone(), "context" => at.clone()})
        }
    }
}

/// `validate-val`: `nil` is not stored.
pub(crate) fn validate_val(v: &Value, at: impl FnOnce() -> Value) -> Result<()> {
    if v.is_nil() {
        let at = at();
        raise!("Cannot store nil as a value at ", at;
            {"error" => Value::kw("transact/syntax"), "value" => Value::Nil, "context" => at.clone()})
    }
    Ok(())
}

/// `validate-tuple`: a tuple attribute's value is a vector, of the declared arity where one is declared.
pub(crate) fn validate_tuple(p: &AttrProps, a: &Value, v: &Value, context: impl FnOnce() -> Value) -> Result<()> {
    if v.is_nil() {
        return Ok(());
    }
    if let Some(types) = &p.tuple_types {
        let n = types.count().unwrap_or(0);
        if !(v.is_vector() && v.count() == Some(n)) {
            let context = context();
            raise!("Attribute ", a, " expected a ", n, "-element vector, got: ", v, " in ", context;
                {"error" => Value::kw("transact/syntax"), "attribute" => a.clone(), "value" => v.clone(), "context" => context.clone()})
        }
    }
    if p.tuple_type.is_some() && !v.is_vector() {
        let context = context();
        raise!("Attribute ", a, " expected a vector, got: ", v, " in ", context;
            {"error" => Value::kw("transact/syntax"), "attribute" => a.clone(), "value" => v.clone(), "context" => context.clone()})
    }
    Ok(())
}

/// `tuple-ref-slot?`: whether a tuple's slot holds an entity id.
pub(crate) fn tuple_ref_slot<D: Searchable>(db: &D, p: &AttrProps, idx: usize) -> bool {
    let type_ref = &kw().db_type_ref;
    if let Some(attrs) = p.tuple_attrs.as_ref().filter(|v| v.truthy()) {
        match attrs.as_seq().and_then(|s| s.get(idx)) {
            Some(a) => props_of(db, a).is_ref,
            None => false,
        }
    } else if let Some(types) = p.tuple_types.as_ref().filter(|v| v.truthy()) {
        types.as_seq().and_then(|s| s.get(idx)).is_some_and(|t| t.is_kw(type_ref))
    } else {
        p.tuple_type.as_ref().is_some_and(|t| t.is_kw(type_ref))
    }
}

/// `resolved-eid?`: `nil`, or a number that is not a tempid.
#[inline]
pub(crate) fn resolved_eid(v: &Value) -> bool {
    match v {
        Value::Nil => true,
        Value::Num(n) => *n >= 0.0,
        _ => false,
    }
}

/// `resolve-tuple-refs`: a tuple value with the lookup refs and idents in its reference slots resolved. On the read
/// path there are no tempids, and a reference that resolves to nothing is an error.
pub(crate) fn resolve_tuple_refs<D: Searchable>(
    db: &D,
    a: &Value,
    vs: &Value,
    context: impl Fn() -> Value,
) -> Result<Value> {
    let p = props_of(db, a);
    validate_tuple(p, a, vs, &context)?;
    let Some(items) = vs.as_seq() else { return Ok(vs.clone()) };
    let mut out = Vec::with_capacity(items.len());
    for (idx, v) in items.iter().enumerate() {
        if tuple_ref_slot(db, p, idx) && !resolved_eid(v) {
            // a fraction refers to nothing, and stays what it is
            out.push(match entid_strict(db, v) {
                Ok(e) => Value::from(e),
                Err(err) if err.no_such_id => v.clone(),
                Err(err) => return Err(err),
            });
        } else {
            out.push(v.clone());
        }
    }
    Ok(Value::vector(out))
}

// ---------------------------------------------------------------- entity ids

/// `entid`: the entity id an id, a lookup ref or an ident stands for; `None` when a lookup finds nothing.
pub fn entid<D: Searchable>(db: &D, eid: &Value) -> Result<Option<i32>> {
    match eid {
        Value::Num(n) if *n > 0.0 => {
            if *n > EMAX as f64 {
                raise!("Highest supported entity id is ", EMAX, ", got ", eid;
                    {"error" => Value::kw("entity-id"), "value" => eid.clone()})
            }
            crate::datom::id_from_num(*n).map(Some)
        }
        Value::Vector(items) | Value::List(items) => {
            if items.len() != 2 {
                raise!("Lookup ref should contain 2 elements: ", eid;
                    {"error" => Value::kw("lookup-ref/syntax"), "entity-id" => eid.clone()})
            }
            let (attr, value) = (&items[0], &items[1]);
            let p = props_of(db, attr);
            if !p.unique {
                raise!("Lookup ref attribute should be marked as :db/unique: ", eid;
                    {"error" => Value::kw("lookup-ref/unique"), "entity-id" => eid.clone()})
            }
            if value.is_nil() {
                return Ok(None);
            }
            let datoms = if p.tuple {
                let value = resolve_tuple_refs(db, attr, value, || eid.clone())?;
                datoms(db, Index::Avet, attr, &value, &Value::Nil, &Value::Nil)?
            } else {
                datoms(db, Index::Avet, attr, value, &Value::Nil, &Value::Nil)?
            };
            Ok(datoms.first()?.map(|d| d.e))
        }
        Value::Keyword(_) => {
            let ident = Value::Keyword(kw().db_ident.clone());
            Ok(datoms(db, Index::Avet, &ident, eid, &Value::Nil, &Value::Nil)?.first()?.map(|d| d.e))
        }
        _ => raise!("Expected number or lookup ref for entity id, got ", eid;
            {"error" => Value::kw("entity-id/syntax"), "entity-id" => eid.clone()}),
    }
}

/// `entid-strict`: as `entid`, and an error when nothing is found.
pub fn entid_strict<D: Searchable>(db: &D, eid: &Value) -> Result<i32> {
    match entid(db, eid)? {
        Some(e) => Ok(e),
        None => raise!("Nothing found for entity id ", eid;
            {"error" => Value::kw("entity-id/missing"), "entity-id" => eid.clone()}),
    }
}

/// `numeric-eid-exists?`
pub fn numeric_eid_exists<D: Searchable>(db: &D, eid: i32) -> Result<bool> {
    let from = Bound::new(eid, None, Value::Nil, TX0);
    Ok(seek_bounds(db, Index::Eavt, &from)?.first()?.is_some_and(|d| d.e == eid))
}

// ---------------------------------------------------------------- searching

fn index_of(core: &DbCore, index: Index) -> &SortedSet<Datom> {
    match index {
        Index::Eavt => &core.eavt,
        Index::Aevt => &core.aevt,
        Index::Avet => &core.avet,
    }
}

/// `(set/slice index from to)`: the datoms from the first not less than `from` to the last not greater than `to`.
fn slice(core: &DbCore, index: Index, from: &Bound, to: &Bound) -> Slice<Datom> {
    let set = index_of(core, index);
    let lo = set.lower_bound(|d| index.cmp_bound(d, from));
    let hi = set.upper_bound(|d| index.cmp_bound(d, to));
    set.slice(lo, hi)
}

/// ClojureScript does not compare a keyword with a string, and a database's attributes are all of the one kind or
/// all of the other. An attribute of the other kind is an error wherever a search would compare it with one of the
/// database's: among the datoms of its entity in EAVT, anywhere in an index that attributes lead.
fn kind_error(core: &DbCore, index: Index, e: i32, a: &Attr) -> Option<Error> {
    let first = core.eavt.all().first()?.a.clone();
    if first.is_keyword() == a.is_keyword() {
        return None;
    }
    let compared = match index {
        Index::Eavt => {
            !slice(core, Index::Eavt, &Bound::new(e, None, Value::Nil, TX0), &Bound::new(e, None, Value::Nil, TXMAX))
                .is_empty()
        }
        Index::Aevt => true,
        Index::Avet => !core.avet.is_empty(),
    };
    compared.then(|| cannot_compare(&first, a))
}

fn cannot_compare(a: &Attr, b: &Attr) -> Error {
    let s = |a: &Attr| crate::print::str_of(&attr_value(a));
    Error::msg(format!("Cannot compare {} to {}", s(a), s(b)))
}

fn bound_kind_error(core: &DbCore, index: Index, bound: &Bound) -> Result<()> {
    match bound.a.as_ref().and_then(|a| kind_error(core, index, bound.e, a)) {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

/// `-search`: the datoms matching a pattern, each component of which may be left open. The index that serves the
/// pattern best is walked, and what it cannot narrow is filtered.
pub fn search<D: Searchable>(db: &D, e: Option<i32>, a: Option<&Attr>, v: Option<&Value>, tx: Option<i32>) -> Datoms {
    let core = db.core();
    let pred = db.pred();
    let v = v.filter(|v| v.is_some());
    let bound = |e: i32, a: Option<&Attr>, v: Option<&Value>, tx: i32| {
        Bound::new(e, a.cloned(), v.cloned().unwrap_or(Value::Nil), tx)
    };
    let mut out;
    match (e, a, v, tx) {
        (Some(e), Some(a), Some(v), Some(tx)) => {
            let b = bound(e, Some(a), Some(v), tx);
            out = Datoms::new(slice(core, Index::Eavt, &b, &b), pred);
        }
        (Some(e), Some(a), Some(v), None) => {
            out = Datoms::new(
                slice(core, Index::Eavt, &bound(e, Some(a), Some(v), TX0), &bound(e, Some(a), Some(v), TXMAX)),
                pred,
            );
        }
        (Some(e), Some(a), None, tx) => {
            out = Datoms::new(
                slice(core, Index::Eavt, &bound(e, Some(a), None, TX0), &bound(e, Some(a), None, TXMAX)),
                pred,
            );
            out.tx = tx;
        }
        (Some(e), None, v, tx) => {
            out = Datoms::new(slice(core, Index::Eavt, &bound(e, None, None, TX0), &bound(e, None, None, TXMAX)), pred);
            out.v = v.cloned();
            out.tx = tx;
        }
        (None, Some(a), Some(v), tx) => {
            if core.schema.props(a).index {
                out = Datoms::new(
                    slice(core, Index::Avet, &bound(E0, Some(a), Some(v), TX0), &bound(EMAX, Some(a), Some(v), TXMAX)),
                    pred,
                );
            } else {
                out = Datoms::new(
                    slice(core, Index::Aevt, &bound(E0, Some(a), None, TX0), &bound(EMAX, Some(a), None, TXMAX)),
                    pred,
                );
                out.v = Some(v.clone());
            }
            out.tx = tx;
        }
        (None, Some(a), None, tx) => {
            out = Datoms::new(
                slice(core, Index::Aevt, &bound(E0, Some(a), None, TX0), &bound(EMAX, Some(a), None, TXMAX)),
                pred,
            );
            out.tx = tx;
        }
        (None, None, v, tx) => {
            out = Datoms::new(core.eavt.all(), pred);
            out.v = v.cloned();
            out.tx = tx;
        }
    }
    if let Some(a) = a {
        let index = match e {
            Some(_) => Index::Eavt,
            None if v.is_some() && core.schema.props(a).index => Index::Avet,
            None => Index::Aevt,
        };
        out.failed = kind_error(core, index, e.unwrap_or(E0), a);
    }
    out
}

/// `fsearch`: the first datom a search finds.
pub(crate) fn fsearch<D: Searchable>(db: &D, e: i32, a: &Attr, v: Option<&Value>) -> Result<Option<Datom>> {
    search(db, Some(e), Some(a), v, None).first()
}

// ---------------------------------------------------------------- index access

/// `validate-indexed`: only indexed attributes are in AVET.
fn validate_indexed<D: Searchable>(db: &D, index: Index, c0: &Value, c1: &Value, c2: &Value, c3: &Value) -> Result<()> {
    if index == Index::Avet && c0.is_some() && !props_of(db, c0).index {
        raise!("Attribute ", c0, " should be marked as :db/index true";
            {"error" => Value::kw("index-access"), "index" => Value::kw("avet"),
             "components" => Value::vector(vec![c0.clone(), c1.clone(), c2.clone(), c3.clone()])})
    }
    Ok(())
}

/// `resolve-datom`: a bound from components, with entity ids, references and tuples resolved, and what is left open
/// set to the given defaults.
fn resolve_datom<D: Searchable>(
    db: &D,
    e: &Value,
    a: &Value,
    v: &Value,
    t: &Value,
    default_e: i32,
    default_tx: i32,
) -> Result<Bound> {
    let context =
        || Value::list(vec![Value::sym("resolve-datom"), Value::sym("db"), e.clone(), a.clone(), v.clone(), t.clone()]);
    let attr = if a.is_some() { Some(validate_attr(a, context)?) } else { None };
    // An id that is a fraction is no entity's, and lies between two that may be: a bound from below is the next
    // above it, a bound from above the next below.
    let id = |x: &Value, lower: bool| -> Result<i32> {
        match entid_strict(db, x) {
            Err(err) if err.no_such_id => {
                let n = x.as_num().unwrap_or(0.0);
                Ok(if lower { n.ceil() as i32 } else { n.floor() as i32 })
            }
            other => other,
        }
    };
    let e = if e.is_some() { id(e, default_e == E0)? } else { default_e };
    let v = if v.is_nil() {
        Value::Nil
    } else {
        let p = props_of(db, a);
        if p.is_ref {
            match entid_strict(db, v) {
                Ok(e) => Value::from(e),
                Err(err) if err.no_such_id => v.clone(),
                Err(err) => return Err(err),
            }
        } else if p.tuple {
            resolve_tuple_refs(db, a, v, context)?
        } else {
            v.clone()
        }
    };
    let tx = if t.is_some() { id(t, default_tx == TX0)? } else { default_tx };
    Ok(Bound::new(e, attr, v, tx))
}

/// `components->pattern`
fn components_bound<D: Searchable>(
    db: &D,
    index: Index,
    c0: &Value,
    c1: &Value,
    c2: &Value,
    c3: &Value,
    default_e: i32,
    default_tx: i32,
) -> Result<Bound> {
    match index {
        Index::Eavt => resolve_datom(db, c0, c1, c2, c3, default_e, default_tx),
        Index::Aevt => resolve_datom(db, c1, c0, c2, c3, default_e, default_tx),
        Index::Avet => resolve_datom(db, c2, c0, c1, c3, default_e, default_tx),
    }
}

/// `-datoms`: the datoms of an index whose leading components are these.
pub fn datoms<D: Searchable>(db: &D, index: Index, c0: &Value, c1: &Value, c2: &Value, c3: &Value) -> Result<Datoms> {
    validate_indexed(db, index, c0, c1, c2, c3)?;
    let from = components_bound(db, index, c0, c1, c2, c3, E0, TX0)?;
    let to = components_bound(db, index, c0, c1, c2, c3, EMAX, TXMAX)?;
    bound_kind_error(db.core(), index, &from)?;
    bound_kind_error(db.core(), index, &to)?;
    Ok(Datoms::new(slice(db.core(), index, &from, &to), db.pred()))
}

fn seek_bounds<D: Searchable>(db: &D, index: Index, from: &Bound) -> Result<Datoms> {
    let to = Bound::new(EMAX, None, Value::Nil, TXMAX);
    Ok(Datoms::new(slice(db.core(), index, from, &to), db.pred()))
}

/// `-seek-datoms`: from these components to the end of the index.
pub fn seek_datoms<D: Searchable>(
    db: &D,
    index: Index,
    c0: &Value,
    c1: &Value,
    c2: &Value,
    c3: &Value,
) -> Result<Datoms> {
    validate_indexed(db, index, c0, c1, c2, c3)?;
    let from = components_bound(db, index, c0, c1, c2, c3, E0, TX0)?;
    bound_kind_error(db.core(), index, &from)?;
    seek_bounds(db, index, &from)
}

/// `-rseek-datoms`: from these components back to the start of the index.
pub fn rseek_datoms<D: Searchable>(
    db: &D,
    index: Index,
    c0: &Value,
    c1: &Value,
    c2: &Value,
    c3: &Value,
) -> Result<Datoms> {
    validate_indexed(db, index, c0, c1, c2, c3)?;
    let to = components_bound(db, index, c0, c1, c2, c3, EMAX, TXMAX)?;
    bound_kind_error(db.core(), index, &to)?;
    let from = Bound::new(E0, None, Value::Nil, TX0);
    Ok(Datoms::new(slice(db.core(), index, &from, &to), db.pred()).reversed())
}

/// `-index-range`: the part of AVET between two values of an attribute.
pub fn index_range<D: Searchable>(db: &D, attr: &Value, start: &Value, end: &Value) -> Result<Datoms> {
    validate_indexed(db, Index::Avet, attr, &Value::Nil, &Value::Nil, &Value::Nil)?;
    validate_attr(attr, || {
        Value::list(vec![Value::sym("-index-range"), Value::sym("db"), attr.clone(), start.clone(), end.clone()])
    })?;
    let from = resolve_datom(db, &Value::Nil, attr, start, &Value::Nil, E0, TX0)?;
    let to = resolve_datom(db, &Value::Nil, attr, end, &Value::Nil, EMAX, TXMAX)?;
    Ok(Datoms::new(slice(db.core(), Index::Avet, &from, &to), db.pred()))
}

/// `find-datom`: the first datom of an index with these leading components.
pub fn find_datom(db: &Db, index: Index, c0: &Value, c1: &Value, c2: &Value, c3: &Value) -> Result<Option<Datom>> {
    validate_indexed(db, index, c0, c1, c2, c3)?;
    if db.is_filtered() {
        // it reads the index itself, which a filtered database does not give
        return Err(Error::msg("-lookup is not supported on FilteredDB"));
    }
    let from = components_bound(db, index, c0, c1, c2, c3, E0, TX0)?;
    let to = components_bound(db, index, c0, c1, c2, c3, EMAX, TXMAX)?;
    bound_kind_error(db.core(), index, &from)?;
    let set = index_of(db.core(), index);
    let found = set.at(set.lower_bound(|d| index.cmp_bound(d, &from)));
    Ok(found.filter(|d| index.cmp_bound(d, &to) != Ordering::Greater).cloned())
}

// ---------------------------------------------------------------- making databases

impl DbCore {
    fn new(schema: Arc<Schema>) -> DbCore {
        DbCore {
            schema,
            eavt: SortedSet::new(),
            aevt: SortedSet::new(),
            avet: SortedSet::new(),
            max_eid: E0,
            max_tx: TX0,
            hash: OnceLock::new(),
        }
    }

    /// The datoms of an index between two bounds, both included.
    pub(crate) fn slice_between(&self, index: Index, from: &Bound, to: &Bound) -> Result<Vec<Datom>> {
        bound_kind_error(self, index, from)?;
        bound_kind_error(self, index, to)?;
        Ok(slice(self, index, from, to).to_vec())
    }

    /// `advance-max-eid`: an entity id above the highest so far, and below the transactions', is the highest now.
    #[inline]
    pub(crate) fn advance_max_eid(&mut self, eid: i32) {
        if eid > self.max_eid && eid < TX0 {
            self.max_eid = eid;
        }
    }

    /// `with-datom`, less its check of uniqueness: the datom into the indexes, or its fact out of them.
    pub(crate) fn apply(&mut self, datom: &Datom) -> Result<()> {
        let indexing = self.schema.props(&datom.a).index;
        if datom.added() {
            // keywords and strings do not compare: a database's attributes are all of one kind
            if let Some(first) = self.eavt.all().first() {
                if first.a.is_keyword() != datom.a.is_keyword() {
                    return Err(cannot_compare(&first.a, &datom.a));
                }
            }
            self.eavt.insert(datom.clone(), &|a, b| Index::Eavt.cmp(a, b));
            self.aevt.insert(datom.clone(), &|a, b| Index::Aevt.cmp(a, b));
            if indexing {
                self.avet.insert(datom.clone(), &|a, b| Index::Avet.cmp(a, b));
            }
            self.advance_max_eid(datom.e);
            self.hash = OnceLock::new();
        } else if let Some(removing) = fsearch(self, datom.e, &datom.a, Some(&datom.v))? {
            self.eavt.remove(&removing, &|a, b| Index::Eavt.cmp(a, b));
            self.aevt.remove(&removing, &|a, b| Index::Aevt.cmp(a, b));
            if indexing {
                self.avet.remove(&removing, &|a, b| Index::Avet.cmp(a, b));
            }
            self.hash = OnceLock::new();
        }
        Ok(())
    }
}

/// `init-max-eid`: the highest entity id among the datoms' entities and the values of reference attributes.
fn init_max_eid(schema: &Schema, eavt: &SortedSet<Datom>, avet: &SortedSet<Datom>) -> i32 {
    let last = |set: &SortedSet<Datom>, index: Index, from: &Bound, to: &Bound| -> Option<Datom> {
        let lo = set.lower_bound(|d| index.cmp_bound(d, from));
        let hi = set.upper_bound(|d| index.cmp_bound(d, to));
        set.slice(lo, hi).iter_rev().next().cloned()
    };
    let mut res = E0;
    if let Some(d) =
        last(eavt, Index::Eavt, &Bound::new(E0, None, Value::Nil, TX0), &Bound::new(TX0 - 1, None, Value::Nil, TXMAX))
    {
        res = res.max(d.e);
    }
    for attr in &schema.ref_attrs {
        let from = Bound::new(E0, Some(attr.clone()), Value::from(E0), TX0);
        let to = Bound::new(TX0 - 1, Some(attr.clone()), Value::from(TX0 - 1), TXMAX);
        if let Some(Value::Num(v)) = last(avet, Index::Avet, &from, &to).map(|d| d.v) {
            if v > res as f64 && v <= i32::MAX as f64 {
                res = v as i32;
            }
        }
    }
    res
}

impl Db {
    fn plain(core: DbCore) -> Db {
        Db(Arc::new(DbRepr::Plain(core)))
    }

    pub(crate) fn from_core(core: DbCore) -> Db {
        Db::plain(core)
    }

    /// `(empty-db schema)`: the schema is a map or `nil`.
    pub fn empty(schema: Value) -> Result<Db> {
        Ok(Db::plain(DbCore::new(Arc::new(Schema::new(schema)?))))
    }

    /// `(init-db datoms schema)`: a database of these datoms, taken as they are.
    pub fn init(datoms: Vec<Datom>, schema: Value) -> Result<Db> {
        let schema = Arc::new(Schema::new(schema)?);
        if let Some(first) = datoms.first() {
            if let Some(other) = datoms.iter().find(|d| d.a.is_keyword() != first.a.is_keyword()) {
                return Err(cannot_compare(&first.a, &other.a));
            }
        }
        let mut by_eavt = datoms;
        by_eavt.sort_by(|a, b| Index::Eavt.cmp(a, b));
        let mut by_aevt = by_eavt.clone();
        by_aevt.sort_by(|a, b| Index::Aevt.cmp(a, b));
        let mut by_avet: Vec<Datom> = by_eavt.iter().filter(|d| schema.props(&d.a).index).cloned().collect();
        by_avet.sort_by(|a, b| Index::Avet.cmp(a, b));
        let max_tx = by_eavt.iter().map(Datom::tx).fold(TX0, i32::max);
        let eavt = SortedSet::from_sorted(by_eavt);
        let aevt = SortedSet::from_sorted(by_aevt);
        let avet = SortedSet::from_sorted(by_avet);
        let max_eid = init_max_eid(&schema, &eavt, &avet);
        Ok(Db::plain(DbCore { schema, eavt, aevt, avet, max_eid, max_tx, hash: OnceLock::new() }))
    }

    /// `restore-db`: a database of indexes already in order, as a serialized one is read back.
    pub(crate) fn restore(
        schema: Arc<Schema>,
        eavt: Vec<Datom>,
        aevt: Vec<Datom>,
        avet: Vec<Datom>,
        max_eid: i32,
        max_tx: i32,
    ) -> Db {
        Db::plain(DbCore {
            schema,
            eavt: SortedSet::from_sorted(eavt),
            aevt: SortedSet::from_sorted(aevt),
            avet: SortedSet::from_sorted(avet),
            max_eid,
            max_tx,
            hash: OnceLock::new(),
        })
    }

    /// `(with-schema db schema)`: the same datoms under another schema, which is neither validated nor applied to
    /// what is already indexed.
    pub fn with_schema(&self, schema: Value) -> Result<Db> {
        if !(schema.is_nil() || schema.is_map()) {
            return Err(Error::msg("Assert failed: (or (nil? schema) (map? schema))"));
        }
        if self.is_filtered() {
            return Err(Error::msg("-assoc is not supported on FilteredDB"));
        }
        let mut core = self.core().clone();
        core.schema = Arc::new(Schema::unchecked(schema));
        core.hash = OnceLock::new();
        Ok(Db::plain(core))
    }

    /// `(empty db)`: no datoms, the same schema.
    pub fn empty_like(&self) -> Result<Db> {
        if self.is_filtered() {
            return Err(Error::msg("-empty is not supported on FilteredDB"));
        }
        Ok(Db::plain(DbCore::new(self.core().schema.clone())))
    }

    /// `(filter db pred)`: a view of the database with only the datoms `(pred db datom)` holds for. A view of a
    /// view asks both predicates, each of the unfiltered database.
    pub fn filter(&self, pred: Arc<dyn Fn(&Db, &Datom) -> Result<bool> + Send + Sync>) -> Db {
        let unfiltered = self.unfiltered();
        let of = unfiltered.clone();
        let combined: Pred = match self.pred().cloned() {
            Some(first) => Arc::new(move |d| Ok(first(d)? && pred(&of, d)?)),
            None => Arc::new(move |d| pred(&of, d)),
        };
        Db(Arc::new(DbRepr::Filtered(Filtered { unfiltered, pred: combined, hash: OnceLock::new() })))
    }

    #[inline]
    pub fn is_filtered(&self) -> bool {
        matches!(&*self.0, DbRepr::Filtered(_))
    }

    /// `unfiltered-db`
    pub fn unfiltered(&self) -> Db {
        match &*self.0 {
            DbRepr::Plain(_) => self.clone(),
            DbRepr::Filtered(f) => f.unfiltered.clone(),
        }
    }

    /// Whether both are one value: ClojureScript's `identical?`.
    #[inline]
    pub fn ptr_eq(&self, other: &Db) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    /// A number that is this value's alone while it lives, as ClojureScript hashes an entity by its database's id.
    pub fn identity(&self) -> usize {
        Arc::as_ptr(&self.0) as *const () as usize
    }

    /// `(:schema db)`: the schema as given, a map or `nil`.
    pub fn schema_value(&self) -> Value {
        self.core().schema.value.clone()
    }

    /// `(:rschema db)`
    pub fn rschema_value(&self) -> Value {
        self.core().schema.rschema.clone()
    }

    pub fn schema(&self) -> &Arc<Schema> {
        &self.core().schema
    }

    /// `(:max-eid db)`
    pub fn max_eid(&self) -> i32 {
        self.core().max_eid
    }

    /// `(:max-tx db)`
    pub fn max_tx(&self) -> i32 {
        self.core().max_tx
    }

    /// `(count db)`: its datoms, less those a filter leaves out.
    pub fn count(&self) -> Result<usize> {
        match self.pred() {
            None => Ok(self.core().eavt.len()),
            Some(_) => self.all().count(),
        }
    }

    /// Every datom, in EAVT order.
    pub fn all(&self) -> Datoms {
        Datoms::new(self.core().eavt.all(), self.pred())
    }

    /// A whole index, as `(:eavt db)`; of a filtered database, the unfiltered index, as in ClojureScript.
    pub fn index(&self, index: Index) -> Datoms {
        Datoms::new(index_of(self.core(), index).all(), None)
    }

    pub(crate) fn for_each_datom(&self, mut f: impl FnMut(&Datom)) {
        let _ = self.all().try_for_each(|d| {
            f(d);
            Ok(true)
        });
    }

    /// `equiv-db`: the same schema and the same facts, whatever their transactions.
    pub fn equiv(&self, other: &Db) -> bool {
        if self.ptr_eq(other) {
            return true;
        }
        if self.core().schema.value != other.core().schema.value {
            return false;
        }
        if self.pred().is_none() && other.pred().is_none() {
            let (a, b) = (&self.core().eavt, &other.core().eavt);
            return a.ptr_eq(b) || (a.len() == b.len() && a.iter().zip(b.iter()).all(|(x, y)| x.equiv(y)));
        }
        match (self.all().to_vec(), other.all().to_vec()) {
            (Ok(a), Ok(b)) => a.len() == b.len() && a.iter().zip(b.iter()).all(|(x, y)| x.equiv(y)),
            _ => false,
        }
    }

    /// `hash-db`, `hash-fdb`: of the schema and the datoms.
    pub fn cljs_hash(&self) -> i32 {
        let compute = || {
            let mut hashes = Vec::new();
            self.for_each_datom(|d| hashes.push(d.cljs_hash()));
            hash_combine(self.core().schema.value.cljs_hash(), hash_unordered(hashes))
        };
        match &*self.0 {
            DbRepr::Plain(core) => *core.hash.get_or_init(compute),
            DbRepr::Filtered(f) => *f.hash.get_or_init(compute),
        }
    }

    /// `(d/datoms db index & components)`
    pub fn datoms(&self, index: Index, components: &[Value]) -> Result<Datoms> {
        let c = pad(components);
        datoms(self, index, c[0], c[1], c[2], c[3])
    }

    /// `(d/seek-datoms db index & components)`
    pub fn seek_datoms(&self, index: Index, components: &[Value]) -> Result<Datoms> {
        let c = pad(components);
        seek_datoms(self, index, c[0], c[1], c[2], c[3])
    }

    /// `(d/rseek-datoms db index & components)`
    pub fn rseek_datoms(&self, index: Index, components: &[Value]) -> Result<Datoms> {
        let c = pad(components);
        rseek_datoms(self, index, c[0], c[1], c[2], c[3])
    }

    /// `(d/find-datom db index & components)`
    pub fn find_datom(&self, index: Index, components: &[Value]) -> Result<Option<Datom>> {
        let c = pad(components);
        find_datom(self, index, c[0], c[1], c[2], c[3])
    }

    /// `(d/index-range db attr start end)`
    pub fn index_range(&self, attr: &Value, start: &Value, end: &Value) -> Result<Datoms> {
        index_range(self, attr, start, end)
    }

    /// `(d/entid db eid)`
    pub fn entid(&self, eid: &Value) -> Result<Option<i32>> {
        entid(self, eid)
    }

    /// `(d/entid db eid)` as ClojureScript answers it: a number is itself, whatever number it is.
    pub fn entid_value(&self, eid: &Value) -> Result<Value> {
        match entid(self, eid) {
            Ok(e) => Ok(Value::from(e)),
            Err(err) if err.no_such_id => Ok(eid.clone()),
            Err(err) => Err(err),
        }
    }
}

static NIL: Value = Value::Nil;

/// Up to four components, the rest open.
fn pad(components: &[Value]) -> [&Value; 4] {
    let mut out = [&NIL; 4];
    for (slot, c) in out.iter_mut().zip(components) {
        *slot = c;
    }
    out
}

/// `db-from-reader`: `#datascript/DB {:schema ..., :datoms [[e a v tx] ...]}`
pub fn db_from_reader(form: &Value) -> Result<Db> {
    let schema = form.get(&Value::kw("schema")).cloned().unwrap_or(Value::Nil);
    let mut datoms = Vec::new();
    if let Some(items) = form.get(&Value::kw("datoms")).and_then(Value::seq_items) {
        for item in &items {
            let fields = item.as_seq().ok_or_else(|| Error::msg(message!("Cannot read a datom from ", item)))?;
            // [e a v tx], and no more
            let fields = Value::vector(fields.iter().take(4).cloned().collect());
            datoms.push(crate::datom::datom_from_reader(&fields)?);
        }
    }
    Db::init(datoms, schema)
}

/// `clojure.data/diff` of two databases: `[only-in-a only-in-b in-both]`, each a vector of datoms or `nil`. Datoms
/// are the same when entity, attribute and value are. Equal databases answer `[nil nil a]`; a filtered database is
/// no database to `diff`, and differs whole.
pub fn diff(a: &Db, b: &Db) -> Result<Value> {
    if a.equiv(b) {
        return Ok(Value::vector(vec![Value::Nil, Value::Nil, Value::Db(a.clone())]));
    }
    if a.is_filtered() || b.is_filtered() {
        return Ok(Value::vector(vec![Value::Db(a.clone()), Value::Db(b.clone()), Value::Nil]));
    }
    let (xs, ys) = (a.index(Index::Eavt).to_vec()?, b.index(Index::Eavt).to_vec()?);
    let (mut only_a, mut only_b, mut both) = (Vec::new(), Vec::new(), Vec::new());
    let datom = |d: &Datom| Value::Datom(Arc::new(d.clone()));
    let (mut i, mut j) = (0, 0);
    while i < xs.len() && j < ys.len() {
        let (x, y) = (&xs[i], &ys[j]);
        if x.e == y.e && x.a.is_keyword() && !y.a.is_keyword() {
            // a keyword does not compare with a string: neither datom is in the other's order, and both move on
            only_a.push(datom(x));
            only_b.push(datom(y));
            i += 1;
            j += 1;
            continue;
        }
        let attrs = || match (x.a.is_keyword(), y.a.is_keyword()) {
            // a string compares with a keyword as with the keyword's string
            (false, true) => crate::named::compare_str(x.a.full(), &format!(":{}", y.a.full())),
            _ => x.a.cmp(&y.a),
        };
        let c = x.e.cmp(&y.e).then_with(attrs).then_with(|| crate::cmp::value_compare(&x.v, &y.v));
        match c {
            Ordering::Equal => {
                both.push(datom(x));
                i += 1;
                j += 1;
            }
            Ordering::Less => {
                only_a.push(datom(x));
                i += 1;
            }
            Ordering::Greater => {
                only_b.push(datom(y));
                j += 1;
            }
        }
    }
    only_a.extend(xs[i..].iter().map(datom));
    only_b.extend(ys[j..].iter().map(datom));
    let not_empty = |v: Vec<Value>| if v.is_empty() { Value::Nil } else { Value::vector(v) };
    Ok(Value::vector(vec![not_empty(only_a), not_empty(only_b), not_empty(both)]))
}
