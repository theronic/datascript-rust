//! Transactions: `transact-tx-data`, which turns transaction data into datoms and a new database.
//!
//! It is DataScript's loop, branch for branch: a queue of entities, each rewritten into simpler ones until it is a
//! datom to add or retract. The order the queue is worked in is the order of `:tx-data`, so the maps and sets the
//! original walks are walked here in ClojureScript's order.

use crate::coll::{CljMap, CljSet};
use crate::datom::{attr_value, id_from_num, Datom, Index, TX0};
use crate::db::{
    datoms, entid, entid_strict, fsearch, props, props_of, resolve_tuple_refs, resolved_eid, search, tuple_ref_slot,
    validate_attr, validate_tuple, validate_val, Change, Db, DbCore, Searchable,
};
use crate::error::{Error, Result};
use crate::named::Attr;
use crate::schema::kw;
use crate::value::{HostObj, HostObject, Value};
use crate::{message, raise};
use std::any::Any;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU32, Ordering as AtomicOrdering};
use std::sync::Arc;

/// What a transaction did: `datascript.db/TxReport`.
#[derive(Clone)]
pub struct TxReport {
    pub db_before: Db,
    pub db_after: Db,
    /// The datoms added and retracted, in order
    pub tx_data: Vec<Datom>,
    /// Each tempid's entity id, and `:db/current-tx`'s
    pub tempids: Value,
    pub tx_meta: Value,
    /// What it did to the indexes, in order: the datoms put in, and the ones taken out, which are not the
    /// retractions of `tx_data` but the datoms those retracted
    pub changes: Arc<[Change]>,
}

/// `(d/with db tx-data tx-meta)`
pub fn with(db: &Db, tx_data: &Value, tx_meta: Value) -> Result<TxReport> {
    if db.is_filtered() {
        return Err(Error::new(
            "Filtered DB cannot be modified",
            Value::kw_map(&[("error", Value::kw("transaction/filtered"))]),
        ));
    }
    transact_tx_data(db, tx_data, tx_meta)
}

/// The most a transaction may change for the database before it to be kept as the database after, undone. A
/// larger one leaves the two little in common, so there is little to save, and much to undo should the database
/// before be read again.
const SUPERSEDE_UP_TO: usize = 512;

/// `with`, for whoever moves on from the database to the one the transaction makes, as a connection does: the
/// database before then keeps its indexes as the database after's, undone (`Db::superseded_by`). It is the same
/// value still, to anything that reads it.
pub fn advance(db: &Db, tx_data: &Value, tx_meta: Value) -> Result<TxReport> {
    let report = with(db, tx_data, tx_meta)?;
    if report.changes.len() <= SUPERSEDE_UP_TO {
        report.db_before.superseded_by(&report.db_after, report.changes.clone());
    }
    Ok(report)
}

/// `(d/db-with db tx-data)`
pub fn db_with(db: &Db, tx_data: &Value) -> Result<Db> {
    Ok(with(db, tx_data, Value::Nil)?.db_after)
}

// ---------------------------------------------------------------- tempids

/// `AutoTempid`: the tempid of an entity map that has no `:db/id`.
struct AutoTempid(u32);

impl HostObject for AutoTempid {
    fn hash(&self) -> i32 {
        self.0 as i32
    }

    fn equiv(&self, other: &dyn HostObject) -> bool {
        other.as_any().downcast_ref::<AutoTempid>().is_some_and(|o| o.0 == self.0)
    }

    fn type_name(&self) -> String {
        "datascript.db/AutoTempid".into()
    }

    fn pr_str(&self) -> String {
        format!("#datascript/AutoTempid [{}]", self.0)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

fn auto_tempid() -> Value {
    static LAST: AtomicU32 = AtomicU32::new(0);
    Value::Host(HostObj::new(AutoTempid(LAST.fetch_add(1, AtomicOrdering::Relaxed) + 1)))
}

fn is_auto_tempid(v: &Value) -> bool {
    matches!(v, Value::Host(h) if h.0.as_any().is::<AutoTempid>())
}

/// `tx-id?`: the names the current transaction goes by.
fn is_tx_id(e: &Value) -> bool {
    match e {
        Value::Keyword(k) => *k == kw().db_current_tx,
        Value::Str(s) => matches!(&**s, ":db/current-tx" | "datomic.tx" | "datascript.tx"),
        _ => false,
    }
}

/// `tempid?`: a negative number, any string, or an entity map's own.
fn is_tempid(x: &Value) -> bool {
    match x {
        Value::Num(n) => *n < 0.0,
        Value::Str(_) => true,
        _ => is_auto_tempid(x),
    }
}

/// `reverse-ref?`: `:ns/_name`, or `"ns/_name"`.
pub(crate) fn is_reverse_ref(attr: &Value) -> Result<bool> {
    match attr {
        Value::Keyword(k) => Ok(k.name().starts_with('_')),
        Value::Str(s) => {
            let name = match s.split_once('/') {
                Some((ns, name)) if !ns.is_empty() => name,
                Some(_) => return Ok(false),
                None => s,
            };
            Ok(name.len() > 1 && name.starts_with('_') && !name.contains('/'))
        }
        _ => raise!("Bad attribute type: ", attr, ", expected keyword or string";
            {"error" => Value::kw("transact/syntax"), "attribute" => attr.clone()}),
    }
}

/// `reverse-ref`: the attribute read the other way, `:ns/name` for `:ns/_name` and back.
pub(crate) fn reverse_ref(attr: &Value) -> Result<Value> {
    match attr {
        Value::Keyword(k) => Ok(match k.name().strip_prefix('_') {
            Some(name) => Value::Keyword(crate::named::Keyword::new(k.ns(), name)),
            None => Value::Keyword(crate::named::Keyword::new(k.ns(), &format!("_{}", k.name()))),
        }),
        Value::Str(s) => {
            let (ns, name) = match s.split_once('/') {
                Some((ns, name)) => (Some(ns), name),
                None => (None, &**s),
            };
            Ok(Value::from(match (name.strip_prefix('_'), ns) {
                (Some(straight), Some(ns)) => format!("{ns}/{straight}"),
                (Some(straight), None) => straight.to_string(),
                (None, Some(ns)) => format!("{ns}/_{name}"),
                (None, None) => format!("_{name}"),
            }))
        }
        _ => raise!("Bad attribute type: ", attr, ", expected keyword or string";
            {"error" => Value::kw("transact/syntax"), "attribute" => attr.clone()}),
    }
}

/// `multi-value?`: a collection given for an attribute of cardinality many.
fn is_multi_value(many: bool, value: &Value) -> bool {
    many && matches!(value, Value::Vector(_) | Value::List(_) | Value::Set(_))
}

/// `assoc-auto-tempids`: every entity map without a `:db/id` gets a tempid of its own, the maps nested under
/// reference attributes too.
fn assoc_auto_tempids(db: &DbCore, tx_data: &[Value]) -> Result<Vec<Value>> {
    tx_data.iter().map(|entity| auto_tempids_entity(db, entity)).collect()
}

fn auto_tempids_coll(db: &DbCore, v: &Value) -> Result<Value> {
    let items = v.seq_items().unwrap_or_default();
    Ok(Value::list(assoc_auto_tempids(db, &items)?))
}

fn auto_tempids_entity(db: &DbCore, entity: &Value) -> Result<Value> {
    let kw = kw();
    match entity {
        Value::Map(m) => {
            let db_id = Value::Keyword(kw.db_id);
            let mut with_id;
            let source: &CljMap = if m.contains_key(&db_id) {
                m
            } else {
                with_id = (**m).clone();
                with_id.assoc(db_id, auto_tempid());
                &with_id
            };
            let mut out = CljMap::new();
            for (a, v) in source.iter() {
                let nv = if !(a.is_keyword() || a.is_string()) {
                    v.clone()
                } else {
                    let p = props_of(db, a);
                    if p.is_ref && is_multi_value(p.many, v) {
                        auto_tempids_coll(db, v)?
                    } else if p.is_ref {
                        auto_tempids_entity(db, v)?
                    } else if is_reverse_ref(a)? && v.is_sequential() {
                        auto_tempids_coll(db, v)?
                    } else if is_reverse_ref(a)? {
                        auto_tempids_entity(db, v)?
                    } else {
                        v.clone()
                    }
                };
                out.assoc(a.clone(), nv);
            }
            Ok(Value::map(out))
        }
        // a map the host has, and the port has no form for: a record is transacted as the map it is
        Value::Host(h) => match h.0.as_map()? {
            Some(map) => auto_tempids_entity(db, &map),
            None => Ok(entity.clone()),
        },
        Value::Vector(items) | Value::List(items) => {
            let get = |i: usize| items.get(i).cloned().unwrap_or(Value::Nil);
            let (op, e, a, v) = (get(0), get(1), get(2), get(3));
            if op.is_kw(&kw.db_add) {
                let p = props_of(db, &a);
                if p.is_ref {
                    let v = if is_multi_value(p.many, &v) {
                        auto_tempids_coll(db, &v)?
                    } else {
                        auto_tempids_entity(db, &v)?
                    };
                    return Ok(Value::vector(vec![op, e, a, v]));
                }
            }
            Ok(entity.clone())
        }
        _ => Ok(entity.clone()),
    }
}

// ---------------------------------------------------------------- the report in the making

struct Report {
    db_before: Db,
    /// `:db-after`, as it is so far
    db: DbCore,
    tx_data: Vec<Datom>,
    /// What was done to `db`'s indexes so far
    changes: Vec<Change>,
    tempids: CljMap,
    /// `::upserted-tempids`: tempids known, from an earlier run, to stand for an entity already there
    upserted_tempids: CljMap,
    /// `::reverse-tempids`: an entity id's tempids
    reverse_tempids: HashMap<i32, CljSet>,
    /// `::value-tempids`: entity ids given to tempids that were used as values
    value_tempids: Option<CljMap>,
    /// `::tx-redundant`: entities asserted with what they already had
    tx_redundant: Vec<i32>,
    /// `::queued-tuples`: `{eid {tuple-attr value}}`, the composite tuples to write once their entity is done
    queued_tuples: Option<CljMap>,
}

impl Report {
    #[inline]
    fn current_tx(&self) -> i32 {
        self.db_before.max_tx() + 1
    }

    #[inline]
    fn next_eid(&self) -> i32 {
        self.db.max_eid + 1
    }

    /// `allocate-eid`: `e`, a tempid or the transaction's name, now stands for `eid`.
    fn allocate_eid(&mut self, e: &Value, eid: i32) {
        if is_tx_id(e) || is_tempid(e) {
            self.tempids.assoc(e.clone(), Value::from(eid));
            self.reverse_tempids.entry(eid).or_default().insert(e.clone());
        }
        if !is_tempid(e) && eid > self.db.max_eid && eid < TX0 {
            self.tempids.assoc(Value::from(eid), Value::from(eid));
        }
        self.db.advance_max_eid(eid);
    }

    fn value_tempid(&mut self, resolved: i32, tempid: &Value) {
        self.value_tempids.get_or_insert_with(CljMap::new).assoc(Value::from(resolved), tempid.clone());
    }
}

/// `validate-datom`: a unique attribute's value is not asserted twice.
fn validate_datom(db: &DbCore, datom: &Datom) -> Result<()> {
    if datom.added() && props(db, &datom.a).unique {
        let a = attr_value(&datom.a);
        let found = datoms(db, Index::Avet, &a, &datom.v, &Value::Nil, &Value::Nil)?.to_vec()?;
        if !found.is_empty() {
            let found = Value::list(found.into_iter().map(|d| Value::Datom(std::sync::Arc::new(d))).collect());
            raise!("Cannot add ", datom, " because of unique constraint: ", found;
                {"error" => Value::kw("transact/unique"), "attribute" => a.clone(),
                 "datom" => Value::Datom(std::sync::Arc::new(datom.clone()))})
        }
    }
    Ok(())
}

/// `transact-report`: the datom into the database and the report; and where its attribute is part of composite
/// tuples, those tuples queued with their new values.
fn transact_report(report: &mut Report, datom: Datom) -> Result<()> {
    validate_datom(&report.db, &datom)?;
    if let Some(change) = report.db.apply(&datom)? {
        report.changes.push(change);
    }
    let sources = &props(&report.db, &datom.a).attr_tuples;
    if !sources.is_empty() {
        let sources = sources.clone();
        let e = Value::from(datom.e);
        let v = if datom.added() { datom.v.clone() } else { Value::Nil };
        let mut queue = match report.queued_tuples.as_ref().and_then(|q| q.get(&e)) {
            Some(Value::Map(q)) => (**q).clone(),
            _ => CljMap::new(),
        };
        for (tuple, idx) in &sources {
            let tuple_key = attr_value(tuple);
            let current = match queue.get(&tuple_key).filter(|v| v.truthy()) {
                Some(v) => v.clone(),
                None => match search(&report.db, Some(datom.e), Some(tuple), None, None).first()?.map(|d| d.v) {
                    Some(v) if v.truthy() => v,
                    _ => {
                        let n = props(&report.db, tuple).tuple_attrs.as_ref().and_then(Value::count).unwrap_or(0);
                        Value::vector(vec![Value::Nil; n])
                    }
                },
            };
            let mut items = current.seq_items().unwrap_or_default();
            if *idx < items.len() {
                items[*idx] = v.clone();
            } else if *idx == items.len() {
                items.push(v.clone());
            } else {
                return Err(Error::msg("Index out of bounds"));
            }
            queue.assoc(tuple_key, Value::vector(items));
        }
        report.queued_tuples.get_or_insert_with(CljMap::new).assoc(e, Value::map(queue));
    }
    report.tx_data.push(datom);
    Ok(())
}

/// `transact-add`: `[:db/add e a v]`, which adds nothing a fact already there, and replaces the value of an
/// attribute of cardinality one.
fn transact_add(report: &mut Report, e: &Value, a: &Value, v: &Value, tx: Option<&Value>, ent: &Value) -> Result<()> {
    let attr = validate_attr(a, || ent.clone())?;
    validate_val(v, || ent.clone())?;
    let tx = match tx.filter(|t| t.truthy()) {
        Some(Value::Num(n)) => id_from_num(*n)?,
        Some(other) => return Err(Error::msg(message!("Transaction id must be a number, got ", other))),
        None => report.current_tx(),
    };
    let p = props(&report.db, &attr);
    validate_tuple(p, a, v, || ent.clone())?;
    let (is_ref, many) = (p.is_ref, p.many);
    let e = entid_strict(&report.db, e)?;
    let v = if is_ref { Value::from(entid_strict(&report.db, v)?) } else { v.clone() };
    let old = if many { fsearch(&report.db, e, &attr, Some(&v))? } else { fsearch(&report.db, e, &attr, None)? };
    match old {
        None => transact_report(report, Datom::new(e, attr, v, tx)),
        Some(old) if old.v == v => {
            report.tx_redundant.push(e);
            Ok(())
        }
        Some(old) => {
            transact_report(report, Datom::with_added(e, attr, old.v, tx, false))?;
            transact_report(report, Datom::new(e, attr, v, tx))
        }
    }
}

/// `transact-retract-datom`
fn transact_retract_datom(report: &mut Report, d: &Datom) -> Result<()> {
    let tx = report.current_tx();
    transact_report(report, Datom::with_added(d.e, d.a, d.v.clone(), tx, false))
}

/// `retract-components`: a retraction of each entity these datoms hold as a component, as the set ClojureScript
/// makes of them.
fn retract_components(db: &DbCore, datoms: &[Datom]) -> Vec<Value> {
    let op = Value::Keyword(kw().db_fn_retract_entity);
    let set: CljSet = datoms
        .iter()
        .filter(|d| props(db, &d.a).component)
        .map(|d| Value::vector(vec![op.clone(), d.v.clone()]))
        .collect();
    set.iter().cloned().collect()
}

/// `tuple-has-tempids?`
fn tuple_has_tempids(db: &DbCore, a: &Value, vs: &Value) -> bool {
    let Some(items) = vs.as_seq() else { return false };
    let p = props_of(db, a);
    items.iter().enumerate().any(|(idx, v)| tuple_ref_slot(db, p, idx) && is_tempid(v))
}

/// `resolve-tuple-value`: a tuple value with the references, tempids and `:db/current-tx` in its reference slots
/// resolved, each tempid getting its entity id as it is met.
fn resolve_tuple_value(report: &mut Report, a: &Value, vs: &Value, context: &Value) -> Result<Value> {
    let p = props_of(&report.db, a).clone();
    validate_tuple(&p, a, vs, || context.clone())?;
    let Value::Vector(items) = vs else { return Ok(vs.clone()) };
    // The original reads the next entity id from the database as it was when the tuple was met, for every tempid
    // of the tuple: two new tempids in one tuple get the same id. The port keeps what DataScript does.
    let next_eid = report.next_eid();
    let mut out = Vec::with_capacity(items.len());
    for (idx, v) in items.iter().enumerate() {
        if !tuple_ref_slot(&report.db, &p, idx) || resolved_eid(v) {
            out.push(v.clone());
        } else if is_tx_id(v) {
            let id = report.current_tx();
            report.allocate_eid(v, id);
            out.push(Value::from(id));
        } else if is_tempid(v) {
            let resolved = match report.tempids.get(v).and_then(Value::as_num) {
                Some(n) => id_from_num(n)?,
                None => {
                    report.allocate_eid(v, next_eid);
                    next_eid
                }
            };
            report.value_tempid(resolved, v);
            out.push(Value::from(resolved));
        } else {
            // a lookup ref or an ident
            out.push(Value::from(entid_strict(&report.db, v)?));
        }
    }
    Ok(Value::vector(out))
}

/// `resolve-upserts`: the entity less the unique attributes that find an entity already there, and those: `{attr
/// {value entity-id}}`.
fn resolve_upserts(db: &DbCore, entity: &Value) -> Result<(CljMap, CljMap)> {
    let Value::Map(m) = entity else { return Ok((CljMap::new(), CljMap::new())) };
    let resolve = |a: &Value, v: &Value| -> Result<Option<i32>> {
        let p = props_of(db, a);
        let first = |v: &Value| -> Result<Option<i32>> {
            Ok(datoms(db, Index::Avet, a, v, &Value::Nil, &Value::Nil)?.first()?.map(|d| d.e))
        };
        if p.tuple && tuple_has_tempids(db, a, v) {
            Ok(None)
        } else if p.tuple {
            first(&resolve_tuple_refs(db, a, v, || entity.clone())?)
        } else if !p.is_ref {
            first(v)
        } else if !is_tempid(v) {
            first(&Value::from(entid(db, v)?))
        } else {
            Ok(None)
        }
    };
    let mut entity_out = CljMap::new();
    let mut upserts = CljMap::new();
    for (a, v) in m.iter() {
        validate_attr(a, || entity.clone())?;
        validate_val(v, || entity.clone())?;
        let p = props_of(db, a);
        if !p.unique_identity {
            entity_out.assoc(a.clone(), v.clone());
        } else if is_multi_value(p.many, v) {
            let mut insert = Vec::new();
            let mut upsert = CljMap::new();
            for x in v.seq_items().unwrap_or_default() {
                match resolve(a, &x)? {
                    Some(e) => {
                        upsert.assoc(x, Value::from(e));
                    }
                    None => insert.push(x),
                }
            }
            if !insert.is_empty() {
                entity_out.assoc(a.clone(), Value::vector(insert));
            }
            if !upsert.is_empty() {
                upserts.assoc(a.clone(), Value::map(upsert));
            }
        } else {
            match resolve(a, v)? {
                Some(e) => {
                    upserts.assoc(a.clone(), Value::map([(v.clone(), Value::from(e))].into_iter().collect()));
                }
                None => {
                    entity_out.assoc(a.clone(), v.clone());
                }
            }
        }
    }
    Ok((entity_out, upserts))
}

/// `validate-upserts`: the one entity every upsert points to, if any; an error when they point to two, or to
/// another than the entity's own id.
fn validate_upserts(entity: &CljMap, upserts: &CljMap) -> Result<Option<i32>> {
    // {entity-id [attr value]}
    let mut upsert_ids = CljMap::new();
    for (a, v_to_e) in upserts.iter() {
        if let Value::Map(v_to_e) = v_to_e {
            for (v, e) in v_to_e.iter() {
                upsert_ids.assoc(e.clone(), Value::vector(vec![a.clone(), v.clone()]));
            }
        }
    }
    let mut ids = upsert_ids.iter();
    let first = ids.next();
    if let (Some((e1, av1)), Some((e2, av2))) = (first, ids.next()) {
        let part = |av: &Value, i: usize| av.as_seq().map(|s| s[i].clone()).unwrap_or(Value::Nil);
        raise!("Conflicting upserts: ", av1, " resolves to ", e1, ", but ", av2, " resolves to ", e2;
            {"error" => Value::kw("transact/upsert"),
             "assertion" => Value::vector(vec![e1.clone(), part(av1, 0), part(av1, 1)]),
             "conflict" => Value::vector(vec![e2.clone(), part(av2, 0), part(av2, 1)])})
    }
    let Some((upsert_id, av)) = first else { return Ok(None) };
    let eid = entity.get(&Value::Keyword(kw().db_id)).cloned().unwrap_or(Value::Nil);
    if upsert_id.is_some() && eid.is_some() && !is_tempid(&eid) && *upsert_id != eid {
        let part = |i: usize| av.as_seq().map(|s| s[i].clone()).unwrap_or(Value::Nil);
        raise!("Conflicting upsert: ", av, " resolves to ", upsert_id, ", but entity already has :db/id ", eid;
            {"error" => Value::kw("transact/upsert"),
             "assertion" => Value::vector(vec![upsert_id.clone(), part(0), part(1)]),
             "conflict" => Value::kw_map(&[("db/id", eid.clone())])})
    }
    match upsert_id {
        Value::Num(n) => Ok(Some(id_from_num(*n)?)),
        _ => Ok(None),
    }
}

/// `maybe-wrap-multival`: the values an entity map gives for one attribute. A collection for an attribute of
/// cardinality many, or a reverse one, is its elements; unless it looks like a lookup ref.
fn maybe_wrap_multival(db: &DbCore, a: &Value, vs: &Value) -> Result<Vec<Value>> {
    if !(is_reverse_ref(a)? || props_of(db, a).many) {
        return Ok(vec![vs.clone()]);
    }
    if !matches!(vs, Value::Vector(_) | Value::List(_) | Value::Set(_)) {
        return Ok(vec![vs.clone()]);
    }
    let items = vs.seq_items().unwrap_or_default();
    if items.len() == 2 && props_of(db, &items[0]).unique_identity {
        return Ok(vec![vs.clone()]);
    }
    Ok(items)
}

/// An entity map on its way to being `[:db/add ...]`s: `explode`, an attribute at a time, as the lazy sequence the
/// original makes is read.
struct Explode {
    eid: Value,
    /// The attributes that are not tuples, then the tuples
    pairs: Vec<(Value, Value)>,
    next: usize,
}

fn explode(db: &DbCore, entity: &CljMap) -> Explode {
    let eid = entity.get(&Value::Keyword(kw().db_id)).cloned().unwrap_or(Value::Nil);
    let (mut plain, mut tuples) = (Vec::new(), Vec::new());
    for (a, vs) in entity.iter() {
        if props_of(db, a).tuple {
            tuples.push((a.clone(), vs.clone()));
        } else {
            plain.push((a.clone(), vs.clone()));
        }
    }
    plain.append(&mut tuples);
    Explode { eid, pairs: plain, next: 0 }
}

/// The next attribute of an exploded map that has values, as `[:db/add ...]`s at the front of the queue, and the
/// rest of the map after them. The attributes passed on the way are validated, as the original's lazy sequence
/// validates them when it is read.
fn expand(mut ex: Explode, db: &DbCore, es: &mut VecDeque<Item>) -> Result<()> {
    let kw = kw();
    let mut ops = Vec::new();
    while ex.next < ex.pairs.len() && ops.is_empty() {
        let (a, vs) = ex.pairs[ex.next].clone();
        ex.next += 1;
        if a.is_kw(&kw.db_id) {
            continue;
        }
        let context =
            || Value::map([(Value::Keyword(kw.db_id), ex.eid.clone()), (a.clone(), vs.clone())].into_iter().collect());
        validate_attr(&a, context)?;
        let reverse = is_reverse_ref(&a)?;
        let straight_a = if reverse { reverse_ref(&a)? } else { a.clone() };
        let straight_ref = props_of(db, &straight_a).is_ref;
        if reverse && !straight_ref {
            let context = context();
            raise!("Bad attribute ", a, ": reverse attribute name requires {:db/valueType :db.type/ref} in schema";
                {"error" => Value::kw("transact/syntax"), "attribute" => a.clone(), "context" => context})
        }
        for v in maybe_wrap_multival(db, &a, &vs)? {
            if straight_ref && v.is_map() {
                // another entity, given as a nested map
                let Value::Map(nested) = &v else { unreachable!() };
                let mut nested = (**nested).clone();
                nested.assoc(reverse_ref(&a)?, ex.eid.clone());
                ops.push(Value::map(nested));
            } else if reverse {
                ops.push(vec4(&Value::Keyword(kw.db_add), v, &straight_a, ex.eid.clone()));
            } else {
                ops.push(vec4(&Value::Keyword(kw.db_add), ex.eid.clone(), &straight_a, v));
            }
        }
    }
    if ex.next < ex.pairs.len() {
        es.push_front(Item::Explode(ex));
    }
    for op in ops.into_iter().rev() {
        es.push_front(Item::Entity(op));
    }
    Ok(())
}

/// A place in the queue of entities.
enum Item {
    Entity(Value),
    /// `::flush-tuples`: after each entity of the transaction, when the schema has composite tuples
    FlushTuples,
    /// A tuple's own write, which the checks on tuples let through (`^::internal`)
    Internal(Value),
    Explode(Explode),
}

/// `flush-tuples`: the composite tuples whose parts changed, as additions and retractions.
fn flush_tuples(report: &Report) -> Result<Vec<Item>> {
    let kw = kw();
    let mut out = Vec::new();
    let Some(queued) = &report.queued_tuples else { return Ok(out) };
    for (eid, tuples) in queued.iter() {
        let Value::Map(tuples) = tuples else { continue };
        let e = id_from_num(eid.as_num().unwrap_or(0.0))?;
        for (tuple, value) in tuples.iter() {
            let all_nil = value.as_seq().is_none_or(|items| items.iter().all(Value::is_nil));
            let value = if all_nil { Value::Nil } else { value.clone() };
            let attr = validate_attr(tuple, || tuple.clone())?;
            let current = search(&report.db, Some(e), Some(&attr), None, None).first()?.map_or(Value::Nil, |d| d.v);
            if value == current {
                continue;
            }
            if value.is_nil() {
                out.push(Item::Internal(Value::vector(vec![
                    Value::Keyword(kw.db_retract),
                    eid.clone(),
                    tuple.clone(),
                    current,
                ])));
            } else {
                out.push(Item::Internal(Value::vector(vec![
                    Value::Keyword(kw.db_add),
                    eid.clone(),
                    tuple.clone(),
                    value,
                ])));
            }
        }
    }
    Ok(out)
}

/// `check-value-tempids`: a tempid used only as a value names no entity.
fn check_value_tempids(report: &mut Report) -> Result<()> {
    let Some(mut unused) = report.value_tempids.take() else { return Ok(()) };
    for d in &report.tx_data {
        if d.added() {
            unused.dissoc_transient(&Value::from(d.e));
        }
    }
    for e in &report.tx_redundant {
        unused.dissoc_transient(&Value::from(*e));
    }
    if unused.is_empty() {
        return Ok(());
    }
    let mut tempids: Vec<Value> = unused.vals().cloned().collect();
    // (sort ...), which fails where two tempids cannot be compared
    let mut failure = None;
    tempids.sort_by(|a, b| {
        crate::cmp::compare(a, b).unwrap_or_else(|e| {
            failure.get_or_insert(e);
            std::cmp::Ordering::Equal
        })
    });
    if let Some(e) = failure {
        return Err(e);
    }
    raise!("Tempids used only as value in transaction: ", Value::list(tempids);
        {"error" => Value::kw("transact/syntax"), "tempids" => Value::map(unused.clone())})
}

/// What a run of the loop came to: the report, or a tempid found to stand for an entity already there, with which
/// the transaction starts over.
enum Outcome {
    Done(Box<Report>),
    Retry { tempid: Value, upserted_eid: i32, tempids: CljMap },
}

fn vec4(op: &Value, e: Value, a: &Value, v: Value) -> Value {
    Value::vector(vec![op.clone(), e, a.clone(), v])
}

/// `transact-tx-data`
fn transact_tx_data(db: &Db, es: &Value, tx_meta: Value) -> Result<TxReport> {
    let items = match es {
        Value::Nil => Vec::new(),
        Value::Vector(s) | Value::List(s) => s.items().to_vec(),
        _ => raise!("Bad transaction data ", es, ", expected sequential collection";
            {"error" => Value::kw("transact/syntax"), "tx-data" => es.clone()}),
    };
    let core = db.core();
    let initial_es = assoc_auto_tempids(&core, &items)?;
    let has_tuples = core.schema.has_tuples;

    // `retry-with-tempid` starts over, remembering what the tempid stands for
    let mut tempids = CljMap::new();
    let mut upserted = CljMap::new();
    loop {
        let report = Report {
            db_before: db.clone(),
            db: (*core).clone(),
            tx_data: Vec::new(),
            changes: Vec::new(),
            tempids: tempids.clone(),
            upserted_tempids: upserted.clone(),
            reverse_tempids: HashMap::new(),
            value_tempids: None,
            tx_redundant: Vec::new(),
            queued_tuples: None,
        };
        match run(report, &initial_es, has_tuples)? {
            Outcome::Done(report) => {
                let report = *report;
                return Ok(TxReport {
                    db_before: report.db_before,
                    db_after: Db::from_core(report.db),
                    tx_data: report.tx_data,
                    tempids: Value::map(report.tempids),
                    tx_meta,
                    changes: Arc::from(report.changes),
                });
            }
            Outcome::Retry { tempid, upserted_eid, tempids: so_far } => {
                if let Some(eid) = upserted.get(&tempid) {
                    raise!("Conflicting upsert: ", tempid, " resolves", " both to ", upserted_eid, " and ", eid;
                        {"error" => Value::kw("transact/upsert")})
                }
                tempids = so_far;
                tempids.assoc(tempid.clone(), Value::from(upserted_eid));
                upserted.assoc(tempid, Value::from(upserted_eid));
            }
        }
    }
}

/// `transact-tx-data-impl`
fn run(mut report: Report, initial_es: &[Value], has_tuples: bool) -> Result<Outcome> {
    let kw = kw();
    let mut es: VecDeque<Item> = VecDeque::with_capacity(initial_es.len() * 2);
    for e in initial_es {
        es.push_back(Item::Entity(e.clone()));
        if has_tuples {
            es.push_back(Item::FlushTuples);
        }
    }

    macro_rules! retry {
        ($tempid:expr, $eid:expr) => {
            return Ok(Outcome::Retry { tempid: $tempid, upserted_eid: $eid, tempids: report.tempids })
        };
    }

    while let Some(item) = es.pop_front() {
        let (entity, internal) = match item {
            Item::FlushTuples => {
                if report.queued_tuples.is_some() {
                    let flushed = flush_tuples(&report)?;
                    report.queued_tuples = None;
                    for item in flushed.into_iter().rev() {
                        es.push_front(item);
                    }
                }
                continue;
            }
            Item::Explode(ex) => {
                expand(ex, &report.db, &mut es)?;
                continue;
            }
            Item::Entity(v) => (v, false),
            Item::Internal(v) => (v, true),
        };
        // The original takes an entity off a lazy sequence with `[entity & entities]`, which reads the entity after
        // it too: what an exploded map has next is validated before the entity in hand is transacted.
        while matches!(es.front(), Some(Item::Explode(_))) {
            let Some(Item::Explode(ex)) = es.pop_front() else { unreachable!() };
            expand(ex, &report.db, &mut es)?;
        }

        match &entity {
            Value::Nil => continue,

            Value::Map(m) => {
                let db_id = Value::Keyword(kw.db_id);
                let old_eid = m.get(&db_id).cloned().unwrap_or(Value::Nil);

                // :db/current-tx, "datomic.tx": the transaction itself
                if is_tx_id(&old_eid) {
                    let id = report.current_tx();
                    report.allocate_eid(&old_eid, id);
                    let mut m = (**m).clone();
                    m.assoc(db_id, Value::from(id));
                    es.push_front(Item::Entity(Value::map(m)));
                    continue;
                }

                // a lookup ref: resolved, or an error
                if old_eid.is_sequential() {
                    let id = entid_strict(&report.db, &old_eid)?;
                    let mut m = (**m).clone();
                    m.assoc(db_id, Value::from(id));
                    es.push_front(Item::Entity(Value::map(m)));
                    continue;
                }

                // upserted: exploded onto the entity found, or an error
                let (mut entity_rest, upserts) = resolve_upserts(&report.db, &entity)?;
                if let Some(upserted_eid) = validate_upserts(&entity_rest, &upserts)? {
                    if is_tempid(&old_eid) {
                        if let Some(known) = report.tempids.get(&old_eid) {
                            if *known != Value::from(upserted_eid) {
                                retry!(old_eid, upserted_eid);
                            }
                        }
                    }
                    report.allocate_eid(&old_eid, upserted_eid);
                    report.tx_redundant.push(upserted_eid);
                    entity_rest.assoc(db_id, Value::from(upserted_eid));
                    es.push_front(Item::Explode(explode(&report.db, &entity_rest)));
                    continue;
                }

                // an entity id, a tempid, or none
                if matches!(old_eid, Value::Num(_) | Value::Nil | Value::Str(_)) || is_auto_tempid(&old_eid) {
                    es.push_front(Item::Explode(explode(&report.db, m)));
                    continue;
                }

                raise!("Expected number, string or lookup ref for :db/id, got ", old_eid;
                    {"error" => Value::kw("entity-id/syntax"), "entity" => entity.clone()})
            }

            Value::Vector(items) | Value::List(items) => {
                let get = |i: usize| items.get(i).cloned().unwrap_or(Value::Nil);
                let (op, e, a, v) = (get(0), get(1), get(2), get(3));

                // [:db.fn/call f & args]
                if op.is_kw(&kw.db_fn_call) {
                    let f = get(1);
                    let Value::Fn(f) = &f else {
                        return Err(Error::msg(message!(f, " is not a function")));
                    };
                    let mut args = vec![Value::Db(Db::from_core(report.db.clone()))];
                    args.extend(items.iter().skip(2).cloned());
                    let result = f.call(&args)?;
                    let result = result.seq_items().ok_or_else(|| Error::msg(message!(result, " is not ISeqable")))?;
                    for item in assoc_auto_tempids(&report.db, &result)?.into_iter().rev() {
                        es.push_front(Item::Entity(item));
                    }
                    continue;
                }

                // [:an/ident & args], a transaction function installed in the database
                if let Value::Keyword(k) = &op {
                    let builtin = [
                        &kw.db_fn_call,
                        &kw.db_fn_cas,
                        &kw.db_cas,
                        &kw.db_add,
                        &kw.db_retract,
                        &kw.db_fn_retract_attribute,
                        &kw.db_fn_retract_entity,
                        &kw.db_retract_entity,
                    ];
                    if !builtin.contains(&k) {
                        let Some(ident) = entid(&report.db, &op)? else {
                            raise!("Can’t find entity for transaction fn ", op;
                                {"error" => Value::kw("transact/syntax"), "operation" => Value::kw("db.fn/call"), "tx-data" => entity.clone()})
                        };
                        let fun = fsearch(&report.db, ident, &Attr::keyword(&kw.db_fn), None)?.map(|d| d.v);
                        let Some(Value::Fn(fun)) = fun else {
                            raise!("Entity ", op, " expected to have :db/fn attribute with fn? value";
                                {"error" => Value::kw("transact/syntax"), "operation" => Value::kw("db.fn/call"), "tx-data" => entity.clone()})
                        };
                        let mut args = vec![Value::Db(Db::from_core(report.db.clone()))];
                        args.extend(items.iter().skip(1).cloned());
                        let result = fun.call(&args)?;
                        let result =
                            result.seq_items().ok_or_else(|| Error::msg(message!(result, " is not ISeqable")))?;
                        for item in result.into_iter().rev() {
                            es.push_front(Item::Entity(item));
                        }
                        continue;
                    }
                }

                let is_add = op.is_kw(&kw.db_add);
                let is_retract = op.is_kw(&kw.db_retract);

                if is_tempid(&e) && !is_add {
                    raise!("Can't use tempid in '", entity, "'. Tempids are allowed in :db/add only";
                        {"error" => Value::kw("transact/syntax"), "op" => entity.clone()})
                }

                // [:db/cas e a old new]
                if op.is_kw(&kw.db_fn_cas) || op.is_kw(&kw.db_cas) {
                    let (ov, nv) = (get(3), get(4));
                    let e = entid_strict(&report.db, &e)?;
                    let attr = validate_attr(&a, || entity.clone())?;
                    let db = &report.db;
                    let p = props(db, &attr);
                    let resolve = |x: &Value| -> Result<Value> {
                        if p.is_ref {
                            Ok(Value::from(entid_strict(db, x)?))
                        } else if p.tuple {
                            resolve_tuple_refs(db, &a, x, || entity.clone())
                        } else {
                            Ok(x.clone())
                        }
                    };
                    let ov = resolve(&ov)?;
                    let nv = resolve(&nv)?;
                    validate_val(&nv, || entity.clone())?;
                    let found = search(db, Some(e), Some(&attr), None, None).to_vec()?;
                    let as_values = |ds: &[Datom]| -> Vec<Value> {
                        ds.iter().map(|d| Value::Datom(std::sync::Arc::new(d.clone()))).collect()
                    };
                    if p.many {
                        if !found.iter().any(|d| d.v == ov) {
                            let vs = Value::list(found.iter().map(|d| d.v.clone()).collect());
                            raise!(":db.fn/cas failed on datom [", e, " ", a, " ", vs, "], expected ", ov;
                                {"error" => Value::kw("transact/cas"), "old" => Value::vector(as_values(&found)),
                                 "expected" => ov.clone(), "new" => nv.clone()})
                        }
                    } else {
                        let current = found.first().map_or(Value::Nil, |d| d.v.clone());
                        if current != ov {
                            raise!(":db.fn/cas failed on datom [", e, " ", a, " ", current, "], expected ", ov;
                                {"error" => Value::kw("transact/cas"),
                                 "old" => found.first().map_or(Value::Nil, |d| Value::Datom(std::sync::Arc::new(d.clone()))),
                                 "expected" => ov.clone(), "new" => nv.clone()})
                        }
                    }
                    let add = vec4(&Value::Keyword(kw.db_add), Value::from(e), &a, nv.clone());
                    transact_add(&mut report, &Value::from(e), &a, &nv, None, &add)?;
                    continue;
                }

                if is_tx_id(&e) {
                    let tx = report.current_tx();
                    report.allocate_eid(&e, tx);
                    es.push_front(Item::Entity(vec4(&op, Value::from(tx), &a, v)));
                    continue;
                }

                let p = props_of(&report.db, &a);
                let (a_ref, a_tuple, a_identity, a_composite) = (p.is_ref, p.tuple, p.unique_identity, p.composite);

                if a_ref && is_tx_id(&v) {
                    let tx = report.current_tx();
                    report.allocate_eid(&v, tx);
                    es.push_front(Item::Entity(vec4(&op, e, &a, Value::from(tx))));
                    continue;
                }

                if a_ref && is_tempid(&v) {
                    match report.tempids.get(&v).cloned() {
                        Some(resolved) => {
                            if let Some(n) = resolved.as_num() {
                                report.value_tempid(id_from_num(n)?, &v);
                            }
                            es.push_front(Item::Entity(vec4(&op, e, &a, resolved)));
                        }
                        None => {
                            let resolved = report.next_eid();
                            report.allocate_eid(&v, resolved);
                            report.value_tempid(resolved, &v);
                            // the same entity again, its tempid now known
                            es.push_front(if internal {
                                Item::Internal(entity.clone())
                            } else {
                                Item::Entity(entity.clone())
                            });
                        }
                    }
                    continue;
                }

                if (is_add || is_retract) && !internal && a_tuple {
                    let resolved = resolve_tuple_value(&mut report, &a, &v, &entity)?;
                    if v != resolved {
                        es.push_front(Item::Entity(vec4(&op, e, &a, resolved)));
                        continue;
                    }
                }

                if is_tempid(&e) {
                    let upserted_eid = if a_identity {
                        datoms(&report.db, Index::Avet, &a, &v, &Value::Nil, &Value::Nil)?.first()?.map(|d| d.e)
                    } else {
                        None
                    };
                    let allocated_eid = match report.tempids.get(&e).and_then(Value::as_num) {
                        Some(n) => Some(id_from_num(n)?),
                        None => None,
                    };
                    if let (Some(upserted), Some(allocated)) = (upserted_eid, allocated_eid) {
                        if upserted != allocated {
                            retry!(e, upserted);
                        }
                    }
                    let eid = upserted_eid.or(allocated_eid).unwrap_or_else(|| report.next_eid());
                    report.allocate_eid(&e, eid);
                    es.push_front(Item::Entity(vec4(&op, Value::from(eid), &a, v)));
                    continue;
                }

                // an entity that tempids stand for turns out, by a unique attribute, to be another entity
                if a_identity && e.truthy() {
                    if let Some(n) = e.as_num() {
                        if let Ok(eid) = id_from_num(n) {
                            if report.reverse_tempids.contains_key(&eid) {
                                let upserted_eid = datoms(&report.db, Index::Avet, &a, &v, &Value::Nil, &Value::Nil)?
                                    .first()?
                                    .map(|d| d.e);
                                if let Some(upserted_eid) = upserted_eid.filter(|u| *u != eid) {
                                    let tempid = report.reverse_tempids[&eid]
                                        .iter()
                                        .find(|t| !report.upserted_tempids.contains_key(t))
                                        .cloned();
                                    match tempid {
                                        Some(tempid) => retry!(tempid, upserted_eid),
                                        None => {
                                            raise!("Conflicting upsert: ", e, " resolves to ", upserted_eid, " via ", entity;
                                            {"error" => Value::kw("transact/upsert")})
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                // a composite tuple may be written only as what its parts already make it
                if !internal && a_composite {
                    let tuple_attrs =
                        props_of(&report.db, &a).tuple_attrs.as_ref().and_then(Value::seq_items).unwrap_or_default();
                    let Some(vs) = v.seq_items().filter(|_| v.count().is_some()) else {
                        return Err(Error::msg(crate::clj::no_count(&v)));
                    };
                    let mut same = tuple_attrs.len() == vs.len() && vs.iter().all(Value::is_some);
                    if same {
                        for (tuple_attr, tuple_value) in tuple_attrs.iter().zip(&vs) {
                            let db_value = datoms(&report.db, Index::Eavt, &e, tuple_attr, &Value::Nil, &Value::Nil)?
                                .first()?
                                .map_or(Value::Nil, |d| d.v);
                            if *tuple_value != db_value {
                                same = false;
                                break;
                            }
                        }
                    }
                    if same {
                        continue;
                    }
                    raise!("Can’t modify tuple attrs directly: ", entity;
                        {"error" => Value::kw("transact/syntax"), "tx-data" => entity.clone()})
                }

                if is_add {
                    transact_add(&mut report, &e, &a, &v, items.get(4), &entity)?;
                    continue;
                }

                // [:db/retract e a v]
                if is_retract && v.is_some() {
                    if let Some(e) = entid(&report.db, &e)? {
                        let v = if a_ref { Value::from(entid_strict(&report.db, &v)?) } else { v };
                        let attr = validate_attr(&a, || entity.clone())?;
                        validate_val(&v, || entity.clone())?;
                        validate_tuple(props(&report.db, &attr), &a, &v, || entity.clone())?;
                        if let Some(old) = fsearch(&report.db, e, &attr, Some(&v))? {
                            transact_retract_datom(&mut report, &old)?;
                        }
                    }
                    continue;
                }

                // [:db.fn/retractAttribute e a], [:db/retract e a]
                if op.is_kw(&kw.db_fn_retract_attribute) || is_retract {
                    if let Some(e) = entid(&report.db, &e)? {
                        let attr = validate_attr(&a, || entity.clone())?;
                        let found = search(&report.db, Some(e), Some(&attr), None, None).to_vec()?;
                        for d in &found {
                            transact_retract_datom(&mut report, d)?;
                        }
                        for op in retract_components(&report.db, &found).into_iter().rev() {
                            es.push_front(Item::Entity(op));
                        }
                    }
                    continue;
                }

                // [:db.fn/retractEntity e]: its own datoms, and every reference to it
                if op.is_kw(&kw.db_fn_retract_entity) || op.is_kw(&kw.db_retract_entity) {
                    if let Some(e) = entid(&report.db, &e)? {
                        let e_datoms = search(&report.db, Some(e), None, None, None).to_vec()?;
                        let mut v_datoms = Vec::new();
                        let target = Value::from(e);
                        for attr in report.db.schema.ref_attrs.clone() {
                            v_datoms.extend(search(&report.db, None, Some(&attr), Some(&target), None).to_vec()?);
                        }
                        for d in e_datoms.iter().chain(&v_datoms) {
                            transact_retract_datom(&mut report, d)?;
                        }
                        for op in retract_components(&report.db, &e_datoms).into_iter().rev() {
                            es.push_front(Item::Entity(op));
                        }
                    }
                    continue;
                }

                raise!("Unknown operation at ", entity, ", expected :db/add, :db/retract, :db.fn/call, :db.fn/retractAttribute, :db.fn/retractEntity or an ident corresponding to an installed transaction function (e.g. {:db/ident <keyword> :db/fn <Ifn>}, usage of :db/ident requires {:db/unique :db.unique/identity} in schema)";
                    {"error" => Value::kw("transact/syntax"), "operation" => op.clone(), "tx-data" => entity.clone()})
            }

            // a datom: added as it is, or its fact retracted
            Value::Datom(d) => {
                if d.added() {
                    let add = Value::vector(vec![
                        Value::Keyword(kw.db_add),
                        Value::from(d.e),
                        d.a_value(),
                        d.v.clone(),
                        Value::from(d.tx()),
                    ]);
                    transact_add(&mut report, &Value::from(d.e), &d.a_value(), &d.v, Some(&Value::from(d.tx())), &add)?;
                } else {
                    es.push_front(Item::Entity(vec4(
                        &Value::Keyword(kw.db_retract),
                        Value::from(d.e),
                        &d.a_value(),
                        d.v.clone(),
                    )));
                }
                continue;
            }

            _ => raise!("Bad entity type at ", entity, ", expected map or vector";
                {"error" => Value::kw("transact/syntax"), "tx-data" => entity.clone()}),
        }
    }

    check_value_tempids(&mut report)?;
    // the tempids of maps that had no :db/id are not the caller's
    let mut tempids = report.tempids.empty_like();
    for (k, v) in report.tempids.iter() {
        if !is_auto_tempid(k) {
            tempids.assoc(k.clone(), v.clone());
        }
    }
    tempids.assoc(Value::Keyword(kw.db_current_tx), Value::from(report.current_tx()));
    report.tempids = tempids;
    report.db.max_tx += 1;
    Ok(Outcome::Done(Box::new(report)))
}
