//! The database: an immutable value of three indexes over one set of datoms, a schema, and two counters. Searching
//! it, walking its indexes, and resolving entity ids.

use crate::datom::{attr_value, value_attr, Bound, Datom, Index, E0, EMAX, TX0, TXMAX};
use crate::error::{Error, Result};
use crate::hash::{hash_combine, hash_unordered};
use crate::lock::{Guard, HashCache, Lock};
use crate::named::Attr;
use crate::schema::{kw, AttrProps, Schema};
use crate::sorted_set::{Slice, SortedSet};
use crate::value::Value;
use crate::{message, raise};
use std::cmp::Ordering;
use std::sync::atomic::{AtomicU32, Ordering as AtomicOrdering};
use std::sync::Arc;

/// A predicate over datoms: a filtered database's.
pub type Pred = Arc<dyn Fn(&Datom) -> Result<bool> + Send + Sync>;

/// A database value. Cloning it is cheap, and a clone is the same value: `ptr_eq` holds between them, as
/// `identical?` holds in ClojureScript.
#[derive(Clone)]
pub struct Db(pub(crate) Arc<DbRepr>);

pub(crate) enum DbRepr {
    Plain(Plain),
    Filtered(Filtered),
}

/// The database itself: `datascript.db/DB`. Its indexes, its schema and its two counters.
#[derive(Clone)]
pub struct DbCore {
    pub(crate) schema: Arc<Schema>,
    pub(crate) eavt: SortedSet<Datom>,
    pub(crate) aevt: SortedSet<Datom>,
    pub(crate) avet: SortedSet<Datom>,
    pub(crate) max_eid: i32,
    pub(crate) max_tx: i32,
}

/// A database value that is no view of another. Its schema and counters are always here. Its indexes are here too,
/// or they are those of the database a transaction made of it, with what the transaction did undone: a connection
/// keeps the value it moved on from that way (`Db::superseded_by`), since a value that has been replaced is seldom
/// read again, and its own copy of the indexes' changed nodes is several kilobytes that nothing may come to free:
/// a host of the WebAssembly module learns that a value is unreachable only when its garbage collector says so.
pub(crate) struct Plain {
    schema: Arc<Schema>,
    max_eid: i32,
    max_tx: i32,
    hash: HashCache,
    indexes: Lock<Indexes>,
    /// How many times the indexes were made again: a place in them (`Cursor::place`) is good for one making
    epoch: AtomicU32,
    mark: AtomicU32,
}

enum Indexes {
    Here(Arc<DbCore>),
    /// In the database a transaction made of this one: the same indexes, with the transaction's changes undone
    Later {
        after: Db,
        changes: Arc<[Change]>,
    },
    /// Taken away, as the value is dropped
    Gone,
}

/// What a transaction did to the indexes, a datom at a time: the datom it put in, or the one it took out.
#[derive(Clone)]
pub enum Change {
    Added(Datom),
    Removed(Datom),
}

impl Plain {
    fn new(core: DbCore) -> Plain {
        Plain {
            schema: core.schema.clone(),
            max_eid: core.max_eid,
            max_tx: core.max_tx,
            hash: HashCache::new(),
            indexes: Lock::new(Indexes::Here(Arc::new(core))),
            epoch: AtomicU32::new(0),
            mark: AtomicU32::new(0),
        }
    }

    fn indexes(&self) -> Guard<'_, Indexes> {
        self.indexes.lock()
    }
}

impl Drop for Plain {
    /// A long run of transactions on a connection leaves a chain of values, each kept by the one before it. The
    /// chain is let go of in a loop: a value dropping the next, which drops the next, would be a call a link.
    fn drop(&mut self) {
        let mut link = std::mem::replace(self.indexes.get_mut(), Indexes::Gone);
        while let Indexes::Later { after, .. } = link {
            // the next value, if nothing else holds it: its own link is taken before it is dropped
            match Arc::try_unwrap(after.0) {
                Ok(DbRepr::Plain(mut next)) => {
                    link = std::mem::replace(next.indexes.get_mut(), Indexes::Gone);
                }
                _ => break,
            }
        }
    }
}

/// A database's indexes for as long as they are read: borrowed from a transaction's own, or kept from a database
/// value's, whose own hold on them may be given up meanwhile.
pub enum CoreRef<'a> {
    Here(&'a DbCore),
    Kept(Arc<DbCore>),
}

impl std::ops::Deref for CoreRef<'_> {
    type Target = DbCore;

    #[inline]
    fn deref(&self) -> &DbCore {
        match self {
            CoreRef::Here(core) => core,
            CoreRef::Kept(core) => core,
        }
    }
}

/// `datascript.db/FilteredDB`: a view of a database through a predicate.
pub(crate) struct Filtered {
    unfiltered: Db,
    pred: Pred,
    hash: HashCache,
    mark: AtomicU32,
}

/// What searching needs of a database: the indexes, and the predicate of a filtered one. A transaction searches the
/// database it is building through this too.
pub trait Searchable {
    fn core(&self) -> CoreRef<'_>;
    fn schema(&self) -> &Arc<Schema>;
    fn pred(&self) -> Option<&Pred>;
}

impl Searchable for DbCore {
    #[inline]
    fn core(&self) -> CoreRef<'_> {
        CoreRef::Here(self)
    }

    #[inline]
    fn schema(&self) -> &Arc<Schema> {
        &self.schema
    }

    #[inline]
    fn pred(&self) -> Option<&Pred> {
        None
    }
}

impl Searchable for Db {
    fn core(&self) -> CoreRef<'_> {
        CoreRef::Kept(self.plain().core(self))
    }

    #[inline]
    fn schema(&self) -> &Arc<Schema> {
        &self.plain().schema
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
    /// The index the run is of
    index: Index,
    rev: bool,
    v: Option<Value>,
    tx: Option<i32>,
    pred: Option<Pred>,
    /// What finding them failed with: it is theirs to raise when they are read
    failed: Option<Error>,
}

impl Datoms {
    fn new(slice: Slice<Datom>, index: Index, pred: Option<&Pred>) -> Datoms {
        Datoms { slice, index, rev: false, v: None, tx: None, pred: pred.cloned(), failed: None }
    }

    pub fn empty() -> Datoms {
        Datoms { slice: Slice::empty(), index: Index::Eavt, rev: false, v: None, tx: None, pred: None, failed: None }
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

    /// A cursor at a place one of these datoms' cursors was at (`Cursor::place`): where a caller that read a part
    /// and kept the place reads on, with the same datoms found again in the same database. The place is one in the
    /// index as it was built: a database whose indexes were made again since (`Db::epoch`) is read on from the last
    /// datom read, with `cursor_after`.
    pub fn cursor_at(&self, place: &[u16]) -> Result<Cursor> {
        let pos = crate::sorted_set::Pos::from_path(place)
            .filter(|pos| self.slice.has(pos))
            .ok_or_else(|| Error::msg("datascript: no such place in the index"))?;
        Ok(Cursor { pos, done: false })
    }

    /// A cursor after a datom of the run, in the direction the run is read in: where a caller that kept the last
    /// datom it read reads on, however the index is built.
    pub fn cursor_after(&self, last: &Datom) -> Cursor {
        let index = self.index;
        let pos = if self.rev {
            // backwards from the datom itself, which is left out
            self.slice.lower_bound(|d| index.cmp(d, last))
        } else {
            self.slice.upper_bound(|d| index.cmp(d, last))
        };
        Cursor { pos, done: false }
    }

    /// The next datoms from a cursor, at most `n`, which moves the cursor past them. An empty answer is the end.
    pub fn next_chunk(&self, cursor: &mut Cursor, n: usize) -> Result<Vec<Datom>> {
        let mut out = Vec::new();
        self.for_chunk(cursor, n, |d| out.push(d.clone()))?;
        Ok(out)
    }

    /// The next datoms from a cursor, at most `n`, each given to `f` where it lies; the cursor moves past them. How
    /// many there were: none is the end.
    pub fn for_chunk(&self, cursor: &mut Cursor, n: usize, mut f: impl FnMut(&Datom)) -> Result<usize> {
        self.check()?;
        let mut count = 0;
        if cursor.done || n == 0 {
            return Ok(count);
        }
        let plain = self.unfiltered();
        let (from, to) = self.slice.bounds();
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
                            f(d);
                            count += 1;
                            if count == n {
                                break;
                            }
                        }
                    }
                }
            }
            cursor.pos = it.pos();
            cursor.done |= cursor.pos <= from;
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
                            f(d);
                            count += 1;
                            if count == n {
                                break;
                            }
                        }
                    }
                }
            }
            cursor.pos = it.pos();
            cursor.done |= cursor.pos >= to;
        }
        Ok(count)
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

    /// The place as numbers, which `Datoms::cursor_at` makes a cursor of again.
    pub fn place(&self) -> &[u16] {
        self.pos.path()
    }
}

// ---------------------------------------------------------------- the schema's answers

#[inline]
pub(crate) fn props<'a, D: Searchable>(db: &'a D, a: &Attr) -> &'a AttrProps {
    db.schema().props(a)
}

#[inline]
pub(crate) fn props_of<'a, D: Searchable>(db: &'a D, a: &Value) -> &'a AttrProps {
    db.schema().props_of(a)
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
            let ident = Value::Keyword(kw().db_ident);
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
    let first = core.eavt.all().first()?.a;
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
            out = Datoms::new(slice(&core, Index::Eavt, &b, &b), Index::Eavt, pred);
        }
        (Some(e), Some(a), Some(v), None) => {
            out = Datoms::new(
                slice(&core, Index::Eavt, &bound(e, Some(a), Some(v), TX0), &bound(e, Some(a), Some(v), TXMAX)),
                Index::Eavt,
                pred,
            );
        }
        (Some(e), Some(a), None, tx) => {
            out = Datoms::new(
                slice(&core, Index::Eavt, &bound(e, Some(a), None, TX0), &bound(e, Some(a), None, TXMAX)),
                Index::Eavt,
                pred,
            );
            out.tx = tx;
        }
        (Some(e), None, v, tx) => {
            out = Datoms::new(
                slice(&core, Index::Eavt, &bound(e, None, None, TX0), &bound(e, None, None, TXMAX)),
                Index::Eavt,
                pred,
            );
            out.v = v.cloned();
            out.tx = tx;
        }
        (None, Some(a), Some(v), tx) => {
            if core.schema.props(a).index {
                out = Datoms::new(
                    slice(&core, Index::Avet, &bound(E0, Some(a), Some(v), TX0), &bound(EMAX, Some(a), Some(v), TXMAX)),
                    Index::Avet,
                    pred,
                );
            } else {
                out = Datoms::new(
                    slice(&core, Index::Aevt, &bound(E0, Some(a), None, TX0), &bound(EMAX, Some(a), None, TXMAX)),
                    Index::Aevt,
                    pred,
                );
                out.v = Some(v.clone());
            }
            out.tx = tx;
        }
        (None, Some(a), None, tx) => {
            out = Datoms::new(
                slice(&core, Index::Aevt, &bound(E0, Some(a), None, TX0), &bound(EMAX, Some(a), None, TXMAX)),
                Index::Aevt,
                pred,
            );
            out.tx = tx;
        }
        (None, None, v, tx) => {
            out = Datoms::new(core.eavt.all(), Index::Eavt, pred);
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
        out.failed = kind_error(&core, index, e.unwrap_or(E0), a);
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
    let core = db.core();
    bound_kind_error(&core, index, &from)?;
    bound_kind_error(&core, index, &to)?;
    Ok(Datoms::new(slice(&core, index, &from, &to), index, db.pred()))
}

fn seek_bounds<D: Searchable>(db: &D, index: Index, from: &Bound) -> Result<Datoms> {
    let to = Bound::new(EMAX, None, Value::Nil, TXMAX);
    Ok(Datoms::new(slice(&db.core(), index, from, &to), index, db.pred()))
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
    bound_kind_error(&db.core(), index, &from)?;
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
    let core = db.core();
    bound_kind_error(&core, index, &to)?;
    let from = Bound::new(E0, None, Value::Nil, TX0);
    Ok(Datoms::new(slice(&core, index, &from, &to), index, db.pred()).reversed())
}

/// `-index-range`: the part of AVET between two values of an attribute.
pub fn index_range<D: Searchable>(db: &D, attr: &Value, start: &Value, end: &Value) -> Result<Datoms> {
    validate_indexed(db, Index::Avet, attr, &Value::Nil, &Value::Nil, &Value::Nil)?;
    validate_attr(attr, || {
        Value::list(vec![Value::sym("-index-range"), Value::sym("db"), attr.clone(), start.clone(), end.clone()])
    })?;
    let from = resolve_datom(db, &Value::Nil, attr, start, &Value::Nil, E0, TX0)?;
    let to = resolve_datom(db, &Value::Nil, attr, end, &Value::Nil, EMAX, TXMAX)?;
    Ok(Datoms::new(slice(&db.core(), Index::Avet, &from, &to), Index::Avet, db.pred()))
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
    let core = db.core();
    bound_kind_error(&core, index, &from)?;
    let set = index_of(&core, index);
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

    /// `with-datom`, less its check of uniqueness: the datom into the indexes, or its fact out of them. What it
    /// did to them, if anything, for whoever may have to undo it.
    pub(crate) fn apply(&mut self, datom: &Datom) -> Result<Option<Change>> {
        if datom.added() {
            // keywords and strings do not compare: a database's attributes are all of one kind
            if let Some(first) = self.eavt.all().first() {
                if first.a.is_keyword() != datom.a.is_keyword() {
                    return Err(cannot_compare(&first.a, &datom.a));
                }
            }
            let new = self.insert(datom);
            self.advance_max_eid(datom.e);
            Ok(new.then(|| Change::Added(datom.clone())))
        } else if let Some(removing) = fsearch(self, datom.e, &datom.a, Some(&datom.v))? {
            self.remove(&removing);
            Ok(Some(Change::Removed(removing)))
        } else {
            Ok(None)
        }
    }

    /// A datom into the indexes. Whether it was not there.
    fn insert(&mut self, datom: &Datom) -> bool {
        let new = self.eavt.insert(datom.clone(), &|a, b| Index::Eavt.cmp(a, b));
        self.aevt.insert(datom.clone(), &|a, b| Index::Aevt.cmp(a, b));
        if self.schema.props(&datom.a).index {
            self.avet.insert(datom.clone(), &|a, b| Index::Avet.cmp(a, b));
        }
        new
    }

    /// A datom out of the indexes.
    fn remove(&mut self, datom: &Datom) {
        self.eavt.remove(datom, &|a, b| Index::Eavt.cmp(a, b));
        self.aevt.remove(datom, &|a, b| Index::Aevt.cmp(a, b));
        if self.schema.props(&datom.a).index {
            self.avet.remove(datom, &|a, b| Index::Avet.cmp(a, b));
        }
    }

    /// The indexes as they were before a change. The counters are the caller's to put back.
    fn undo(&mut self, change: &Change) {
        match change {
            Change::Added(datom) => self.remove(datom),
            Change::Removed(datom) => {
                self.insert(datom);
            }
        }
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
        let from = Bound::new(E0, Some(*attr), Value::from(E0), TX0);
        let to = Bound::new(TX0 - 1, Some(*attr), Value::from(TX0 - 1), TXMAX);
        if let Some(Value::Num(v)) = last(avet, Index::Avet, &from, &to).map(|d| d.v) {
            if v > res as f64 && v <= i32::MAX as f64 {
                res = v as i32;
            }
        }
    }
    res
}

impl Plain {
    /// The indexes: this value's own, or made again from those of the database that came after it, which this
    /// value then holds for itself and is no longer kept by.
    fn core(&self, me: &Db) -> Arc<DbCore> {
        if let Indexes::Here(core) = &*self.indexes() {
            return core.clone();
        }
        // from the nearest value that has its indexes, back through what each transaction on the way did
        let mut undo: Vec<Arc<[Change]>> = Vec::new();
        let mut at = me.clone();
        let later = loop {
            let next = match &*at.plain().indexes() {
                Indexes::Here(core) => break core.clone(),
                Indexes::Later { after, changes } => {
                    undo.push(changes.clone());
                    after.clone()
                }
                Indexes::Gone => unreachable!("a database that is being dropped is not read"),
            };
            at = next;
        };
        let mut core = (*later).clone();
        for changes in undo.iter().rev() {
            for change in changes.iter().rev() {
                core.undo(change);
            }
        }
        core.schema = self.schema.clone();
        core.max_eid = self.max_eid;
        core.max_tx = self.max_tx;
        let core = Arc::new(core);
        // the link to the later value is dropped outside the lock: it may be the last hold on a chain of them
        let before = std::mem::replace(&mut *self.indexes(), Indexes::Here(core.clone()));
        self.epoch.fetch_add(1, AtomicOrdering::Relaxed);
        drop(before);
        core
    }
}

impl Db {
    fn plain_of(core: DbCore) -> Db {
        Db(Arc::new(DbRepr::Plain(Plain::new(core))))
    }

    /// The value itself, or the one a filtered database is a view of.
    #[inline]
    fn plain(&self) -> &Plain {
        match &*self.0 {
            DbRepr::Plain(plain) => plain,
            DbRepr::Filtered(f) => f.unfiltered.plain(),
        }
    }

    /// Gives up this value's indexes for those of `after`, the database a transaction made of it, and what the
    /// transaction did (`TxReport::changes`): they are made again if this value is ever read, by undoing that. What
    /// a connection does with the value it moves on from. Nothing that reads the value tells the difference, but
    /// by how long the first read after takes.
    pub fn superseded_by(&self, after: &Db, changes: Arc<[Change]>) {
        let (DbRepr::Plain(plain), DbRepr::Plain(_)) = (&*self.0, &*after.0) else { return };
        if self.ptr_eq(after) {
            return;
        }
        // the indexes are dropped outside the lock
        let before = {
            let mut indexes = plain.indexes();
            match &*indexes {
                Indexes::Here(_) => std::mem::replace(&mut *indexes, Indexes::Later { after: after.clone(), changes }),
                _ => return,
            }
        };
        drop(before);
    }

    /// Whether this value holds its indexes itself.
    pub fn holds_indexes(&self) -> bool {
        matches!(&*self.plain().indexes(), Indexes::Here(_))
    }

    /// How many times this value's indexes were made again after it gave them up. A place in a run of its datoms
    /// (`Cursor::place`) is a place in the indexes as they were built when it was taken, and is good while this
    /// number is what it was then.
    pub fn epoch(&self) -> u32 {
        self.plain().epoch.load(AtomicOrdering::Relaxed)
    }

    pub(crate) fn from_core(core: DbCore) -> Db {
        Db::plain_of(core)
    }

    /// `(empty-db schema)`: the schema is a map or `nil`.
    pub fn empty(schema: Value) -> Result<Db> {
        Ok(Db::plain_of(DbCore::new(Arc::new(Schema::new(schema)?))))
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
        Ok(Db::plain_of(DbCore { schema, eavt, aevt, avet, max_eid, max_tx }))
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
        Db::plain_of(DbCore {
            schema,
            eavt: SortedSet::from_sorted(eavt),
            aevt: SortedSet::from_sorted(aevt),
            avet: SortedSet::from_sorted(avet),
            max_eid,
            max_tx,
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
        let mut core = (*self.core()).clone();
        core.schema = Arc::new(Schema::unchecked(schema));
        Ok(Db::plain_of(core))
    }

    /// `(empty db)`: no datoms, the same schema.
    pub fn empty_like(&self) -> Result<Db> {
        if self.is_filtered() {
            return Err(Error::msg("-empty is not supported on FilteredDB"));
        }
        Ok(Db::plain_of(DbCore::new(self.plain().schema.clone())))
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
        Db(Arc::new(DbRepr::Filtered(Filtered {
            unfiltered,
            pred: combined,
            hash: HashCache::new(),
            mark: AtomicU32::new(0),
        })))
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

    /// A number for whoever keeps database values in a table of their own to put on this one, 0 until they do: the
    /// WebAssembly module's handle for a value its host holds, so that the same value is the same handle with no
    /// table of values to keep beside the table of handles.
    pub fn mark(&self) -> &AtomicU32 {
        match &*self.0 {
            DbRepr::Plain(plain) => &plain.mark,
            DbRepr::Filtered(filtered) => &filtered.mark,
        }
    }

    /// `(:schema db)`: the schema as given, a map or `nil`.
    pub fn schema_value(&self) -> Value {
        self.plain().schema.value.clone()
    }

    /// `(:rschema db)`
    pub fn rschema_value(&self) -> Value {
        self.plain().schema.rschema.clone()
    }

    pub fn schema(&self) -> &Arc<Schema> {
        &self.plain().schema
    }

    /// `(:max-eid db)`
    pub fn max_eid(&self) -> i32 {
        self.plain().max_eid
    }

    /// `(:max-tx db)`
    pub fn max_tx(&self) -> i32 {
        self.plain().max_tx
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
        Datoms::new(self.core().eavt.all(), Index::Eavt, self.pred())
    }

    /// A whole index, as `(:eavt db)`; of a filtered database, the unfiltered index, as in ClojureScript.
    pub fn index(&self, index: Index) -> Datoms {
        Datoms::new(index_of(&self.core(), index).all(), index, None)
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
        if self.plain().schema.value != other.plain().schema.value {
            return false;
        }
        if self.pred().is_none() && other.pred().is_none() {
            let (mine, theirs) = (self.core(), other.core());
            let (a, b) = (&mine.eavt, &theirs.eavt);
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
            hash_combine(self.plain().schema.value.cljs_hash(), hash_unordered(hashes))
        };
        match &*self.0 {
            DbRepr::Plain(plain) => plain.hash.get_or(compute),
            DbRepr::Filtered(f) => f.hash.get_or(compute),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edn::read_string;
    use crate::print::pr_str;
    use crate::transact::{advance, with};

    fn tx(i: usize) -> Value {
        read_string(&format!(
            r#"[[:db/add {} :n {}] [:db/add {} :name "n{}"] [:db/retract {} :name "n{}"]]"#,
            1 + i % 50,
            i,
            1 + i % 7,
            i,
            1 + i % 7,
            i.saturating_sub(7)
        ))
        .unwrap()
    }

    fn digest(db: &Db) -> String {
        let datoms = db.datoms(Index::Eavt, &[]).unwrap().to_vec().unwrap();
        let avet = db.datoms(Index::Avet, &[]).unwrap().to_vec().unwrap();
        format!("{} {} {:?} {:?}", db.max_eid(), db.max_tx(), datoms.iter().map(pr).collect::<Vec<_>>(), avet.len())
    }

    fn pr(d: &Datom) -> String {
        format!("{} {} {} {}", d.e, d.a.full(), pr_str(&d.v), d.tx())
    }

    /// A value a connection moved on from is the value it was, whenever it is read, and however far the
    /// connection has moved on since.
    #[test]
    fn a_superseded_database_reads_as_it_did() {
        let schema = read_string("{:name {:db/index true}}").unwrap();
        let mut plain = vec![Db::empty(schema.clone()).unwrap()];
        let mut moved = vec![Db::empty(schema).unwrap()];
        for i in 0..400 {
            plain.push(with(plain.last().unwrap(), &tx(i), Value::Nil).unwrap().db_after);
            moved.push(advance(moved.last().unwrap(), &tx(i), Value::Nil).unwrap().db_after);
        }
        assert!(moved[..400].iter().all(|db| !db.holds_indexes()));
        assert!(moved[400].holds_indexes());
        // the oldest first, which is the longest way back; then some in the middle; then all of them
        for i in [0, 1, 399, 200, 37, 201] {
            assert_eq!(digest(&plain[i]), digest(&moved[i]), "version {i}");
            assert!(moved[i].holds_indexes());
        }
        for (i, (a, b)) in plain.iter().zip(&moved).enumerate() {
            assert_eq!(digest(a), digest(b), "version {i}");
            assert!(a.equiv(b));
            assert_eq!(a.cljs_hash(), b.cljs_hash());
        }
    }

    /// A value that is read again may be moved on from again.
    #[test]
    fn a_database_moved_on_from_twice() {
        let db0 = Db::empty(Value::Nil).unwrap();
        let a = advance(&db0, &tx(1), Value::Nil).unwrap().db_after;
        assert!(!db0.holds_indexes());
        let b = advance(&db0, &tx(2), Value::Nil).unwrap().db_after;
        assert_eq!(db0.count().unwrap(), 0);
        assert_eq!(digest(&a), digest(&with(&Db::empty(Value::Nil).unwrap(), &tx(1), Value::Nil).unwrap().db_after));
        assert_eq!(digest(&b), digest(&with(&Db::empty(Value::Nil).unwrap(), &tx(2), Value::Nil).unwrap().db_after));
    }

    /// A run of datoms read in parts is read on from the last datom read, when the database's indexes were made
    /// again between two parts and a place in them is a place no more.
    #[test]
    fn a_run_is_read_on_after_its_database_was_moved_on_from() {
        let schema = read_string("{:name {:db/index true}}").unwrap();
        let mut db = Db::empty(schema).unwrap();
        for i in 0..300 {
            db = with(&db, &tx(i), Value::Nil).unwrap().db_after;
        }
        for index in [Index::Eavt, Index::Aevt, Index::Avet] {
            for reverse in [false, true] {
                let run = |db: &Db| {
                    let run = db.datoms(index, &[]).unwrap();
                    if reverse {
                        run.reversed()
                    } else {
                        run
                    }
                };
                let whole: Vec<String> = run(&db).to_vec().unwrap().iter().map(pr).collect();
                // a part, then the database is moved on from, by transactions that change the very leaves read
                let first = run(&db);
                let mut cursor = first.cursor();
                let mut read = first.next_chunk(&mut cursor, 7).unwrap();
                let epoch = db.epoch();
                let mut head = db.clone();
                for i in 300..420 {
                    head = advance(&head, &tx(i), Value::Nil).unwrap().db_after;
                }
                assert!(!db.holds_indexes());
                // the rest, from the last datom read: the indexes are made again, and are built another way
                let rest = run(&db);
                assert_ne!(db.epoch(), epoch);
                let mut cursor = rest.cursor_after(read.last().unwrap());
                loop {
                    let part = rest.next_chunk(&mut cursor, 5).unwrap();
                    if part.is_empty() {
                        break;
                    }
                    read.extend(part);
                }
                assert_eq!(whole, read.iter().map(pr).collect::<Vec<_>>(), "{index:?} reverse {reverse}");
            }
        }
    }

    /// A long chain of values, each kept by the one before, is dropped without a call for each.
    #[test]
    fn a_long_chain_is_dropped_in_a_loop() {
        let first = Db::empty(Value::Nil).unwrap();
        let mut db = first.clone();
        for i in 0..200_000 {
            db = advance(&db, &read_string(&format!("[[:db/add 1 :n {i}]]")).unwrap(), Value::Nil).unwrap().db_after;
        }
        assert_eq!(db.count().unwrap(), 1);
        drop(db);
        // `first` alone holds the chain now, and reading it walks all of it
        assert_eq!(first.count().unwrap(), 0);
        drop(first);
    }
}
