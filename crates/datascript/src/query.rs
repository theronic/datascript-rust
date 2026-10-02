//! `datascript.query`: the Datalog engine. A query's `:where` clauses are resolved one after another against a
//! context of relations; the relations left are collected into the result.
//!
//! The engine is the original's, function for function, and so are its orders: the order relations are kept in the
//! context, the order a hash join walks its two sides, the order a set of result tuples iterates in. Together they
//! are the order of the answer.

use crate::built_ins::{aggregate_fn, call, query_fn};
use crate::clj;
use crate::coll::{CljMap, CljSet};
use crate::datom::{value_attr, Datom};
use crate::db::{entid, entid_strict, props_of, resolve_tuple_refs, search, Db};
use crate::error::{Error, Result};
use crate::lru::Cache;
use crate::named::Symbol;
use crate::parser::{self, find_elements, find_vars, is_aggregate, is_pull, record_symbol};
use crate::record::{as_record, is_record};
use crate::value::Value;
use crate::{message, raise};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::sync::{Arc, OnceLock};

// ---------------------------------------------------------------- relations

/// Where a relation's tuples hold a variable: a place in a row, or a field of a datom.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Idx {
    Pos(usize),
    E,
    A,
    V,
    Tx,
}

/// A relation's variables and where its tuples hold each: `{?e 0, ?v 1}`, or `{?e "e", ?v "v"}` over datoms.
#[derive(Clone, PartialEq, Default)]
struct Attrs(Vec<(Symbol, Idx)>);

impl Attrs {
    fn get(&self, sym: &Symbol) -> Option<Idx> {
        self.0.iter().find(|(s, _)| s == sym).map(|(_, i)| *i)
    }

    fn contains(&self, sym: &Symbol) -> bool {
        self.0.iter().any(|(s, _)| s == sym)
    }

    /// `assoc`: a variable already there keeps its place among the keys
    fn assoc(&mut self, sym: Symbol, idx: Idx) {
        match self.0.iter_mut().find(|(s, _)| *s == sym) {
            Some(e) => e.1 = idx,
            None => self.0.push((sym, idx)),
        }
    }

    /// `(zipmap syms (range))`
    fn numbered<I: IntoIterator<Item = Symbol>>(syms: I) -> Attrs {
        let mut attrs = Attrs::default();
        for (i, s) in syms.into_iter().enumerate() {
            attrs.assoc(s, Idx::Pos(i));
        }
        attrs
    }

    fn keys(&self) -> impl Iterator<Item = &Symbol> + '_ {
        self.0.iter().map(|(s, _)| s)
    }

    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Whether every place is a number: rows, not datoms
    fn all_positions(&self) -> bool {
        self.0.iter().all(|(_, i)| matches!(i, Idx::Pos(_)))
    }

    /// As ClojureScript prints the map
    fn to_value(&self) -> Value {
        Value::map(
            self.0
                .iter()
                .map(|(s, i)| {
                    let idx = match i {
                        Idx::Pos(n) => Value::from(*n),
                        Idx::E => Value::str("e"),
                        Idx::A => Value::str("a"),
                        Idx::V => Value::str("v"),
                        Idx::Tx => Value::str("tx"),
                    };
                    (Value::Symbol(s.clone()), idx)
                })
                .collect(),
        )
    }
}

enum Tuples {
    Datoms(Vec<Datom>),
    Rows(Vec<Vec<Value>>),
}

impl Tuples {
    fn len(&self) -> usize {
        match self {
            Tuples::Datoms(d) => d.len(),
            Tuples::Rows(r) => r.len(),
        }
    }

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// `(da/aget tuple idx)`
    #[inline]
    fn get(&self, i: usize, idx: Idx) -> Value {
        match (self, idx) {
            (Tuples::Rows(rows), Idx::Pos(p)) => rows[i].get(p).cloned().unwrap_or(Value::Nil),
            (Tuples::Datoms(ds), Idx::E) => Value::from(ds[i].e),
            (Tuples::Datoms(ds), Idx::A) => ds[i].a_value(),
            (Tuples::Datoms(ds), Idx::V) => ds[i].v.clone(),
            (Tuples::Datoms(ds), Idx::Tx) => Value::from(ds[i].tx()),
            // a datom read by position, as (nth datom i); a row read by a datom's field has none
            (Tuples::Datoms(ds), Idx::Pos(p)) => ds[i].nth(p).unwrap_or(Value::Nil),
            (Tuples::Rows(_), _) => Value::Nil,
        }
    }
}

#[derive(Clone)]
struct Relation {
    attrs: Arc<Attrs>,
    tuples: Arc<Tuples>,
}

impl Relation {
    fn new(attrs: Attrs, tuples: Tuples) -> Relation {
        Relation { attrs: Arc::new(attrs), tuples: Arc::new(tuples) }
    }

    fn rows(attrs: Attrs, rows: Vec<Vec<Value>>) -> Relation {
        Relation::new(attrs, Tuples::Rows(rows))
    }
}

/// How a value bound to a pattern's variable is read when a join compares it: a lookup ref or ident is the entity
/// it resolves to (`*lookup-attrs*`).
#[derive(Clone)]
enum Resolver {
    /// `resolve-eid`
    Eid,
    /// A tuple attribute's value, with the references in it resolved
    Tuple { attr: Value, pattern: Value },
}

/// What the original binds dynamically while it resolves a clause.
#[derive(Clone, Default)]
struct Env {
    /// `*implicit-source*`: the source a pattern, a rule or a lookup ref is read from
    source: Option<Value>,
    /// `*lookup-attrs*`
    lookup_attrs: Option<Arc<Vec<(Symbol, Resolver)>>>,
}

/// `Context`: the relations so far, the sources and the rules.
#[derive(Clone)]
struct Context {
    rels: Vec<Relation>,
    /// Whether `:rels` is a sequence, which `conj` adds to the front of, or a vector, which it adds to the end of
    rels_seq: bool,
    sources: Arc<HashMap<Symbol, Value>>,
    /// The rules' branches by name, each as it was given
    rules: Arc<HashMap<Value, Vec<Value>>>,
}

impl Context {
    /// `(update context :rels conj rel)`
    fn conj_rel(&mut self, rel: Relation) {
        if self.rels_seq {
            self.rels.insert(0, rel);
        } else {
            self.rels.push(rel);
        }
    }

    fn set_rels(&mut self, rels: Vec<Relation>) {
        self.rels = rels;
        self.rels_seq = false;
    }
}

// ---------------------------------------------------------------- utilities

fn is_free_var(v: &Value) -> bool {
    matches!(v, Value::Symbol(s) if s.name().starts_with('?'))
}

fn is_source(v: &Value) -> bool {
    matches!(v, Value::Symbol(s) if s.name().starts_with('$'))
}

fn is_blank(v: &Value) -> bool {
    v.is_sym("_")
}

fn is_attr(v: &Value) -> bool {
    v.is_keyword() || v.is_string()
}

fn is_lookup_ref(v: &Value) -> bool {
    v.as_seq().is_some_and(|s| s.len() == 2 && is_attr(&s[0]))
}

fn head_is(clause: &[Value], name: &str) -> bool {
    clause.first().is_some_and(|h| h.is_sym(name))
}

/// The variables two relations share, in the first's order.
fn intersect_keys(a: &Attrs, b: &Attrs) -> Vec<Symbol> {
    a.keys().filter(|s| b.contains(s)).cloned().collect()
}

/// `(next xs)` as a list value, or `nil`
fn rest_value(items: &[Value], from: usize) -> Value {
    if items.len() > from {
        Value::list(items[from..].to_vec())
    } else {
        Value::Nil
    }
}

// ---------------------------------------------------------------- relation algebra

/// A join's key: the value of the one shared variable, or of each of several.
#[derive(PartialEq, Eq, Hash)]
enum Key {
    One(Value),
    Many(Vec<Value>),
}

/// `tuple-key-fn`: a tuple's key over the shared variables, each value resolved where the pattern asks.
struct KeyFn {
    getters: Vec<(Idx, Option<Resolver>)>,
}

impl KeyFn {
    fn new(attrs: &Attrs, common: &[Symbol], env: &Env) -> KeyFn {
        let getters = common
            .iter()
            .map(|sym| {
                let resolver =
                    env.lookup_attrs.as_ref().and_then(|la| la.iter().find(|(s, _)| s == sym).map(|(_, r)| r.clone()));
                (attrs.get(sym).unwrap_or(Idx::Pos(usize::MAX)), resolver)
            })
            .collect();
        KeyFn { getters }
    }

    fn value(&self, tuples: &Tuples, i: usize, getter: &(Idx, Option<Resolver>), env: &Env) -> Result<Value> {
        let v = tuples.get(i, getter.0);
        match &getter.1 {
            // a number is an entity id already
            Some(resolver) if v.is_sequential() => resolve(resolver, &v, env),
            _ => Ok(v),
        }
    }

    fn key(&self, tuples: &Tuples, i: usize, env: &Env) -> Result<Key> {
        if self.getters.len() == 1 {
            Ok(Key::One(self.value(tuples, i, &self.getters[0], env)?))
        } else {
            let mut vs = Vec::with_capacity(self.getters.len());
            for g in &self.getters {
                vs.push(self.value(tuples, i, g, env)?);
            }
            Ok(Key::Many(vs))
        }
    }
}

fn source_db(env: &Env) -> Result<&Db> {
    match &env.source {
        Some(Value::Db(db)) => Ok(db),
        _ => Err(Error::msg("Assert failed: (db? db)")),
    }
}

fn resolve(resolver: &Resolver, v: &Value, env: &Env) -> Result<Value> {
    let db = source_db(env)?;
    match resolver {
        Resolver::Eid => Ok(Value::from(entid(db, v)?)),
        Resolver::Tuple { attr, pattern } => resolve_tuple_refs(db, attr, v, || pattern.clone()),
    }
}

/// `hash-attrs`: the tuples of a relation by key, each key's in the reverse of their order, as the original conses
/// them onto a list.
fn hash_attrs(key_fn: &KeyFn, tuples: &Tuples, env: &Env) -> Result<HashMap<Key, Vec<u32>>> {
    let mut hash: HashMap<Key, Vec<u32>> = HashMap::new();
    for i in 0..tuples.len() {
        hash.entry(key_fn.key(tuples, i, env)?).or_default().push(i as u32);
    }
    Ok(hash)
}

/// `join-tuples`
fn join_tuples(t1: &Tuples, i1: usize, idxs1: &[Idx], t2: &Tuples, i2: usize, idxs2: &[Idx]) -> Vec<Value> {
    let mut res = Vec::with_capacity(idxs1.len() + idxs2.len());
    res.extend(idxs1.iter().map(|idx| t1.get(i1, *idx)));
    res.extend(idxs2.iter().map(|idx| t2.get(i2, *idx)));
    res
}

/// `hash-join`: for each tuple of the second relation in order, the tuples of the first with the same key, latest
/// first.
fn hash_join(rel1: &Relation, rel2: &Relation, env: &Env) -> Result<Relation> {
    let (attrs1, attrs2) = (&*rel1.attrs, &*rel2.attrs);
    let common = intersect_keys(attrs1, attrs2);
    let keep_attrs1: Vec<Symbol> = attrs1.keys().cloned().collect();
    let keep_attrs2: Vec<Symbol> = attrs2.keys().filter(|s| !attrs1.contains(s)).cloned().collect();
    let keep_idxs1: Vec<Idx> = attrs1.0.iter().map(|(_, i)| *i).collect();
    let keep_idxs2: Vec<Idx> = keep_attrs2.iter().filter_map(|s| attrs2.get(s)).collect();
    let key_fn1 = KeyFn::new(attrs1, &common, env);
    let key_fn2 = KeyFn::new(attrs2, &common, env);
    let hash = hash_attrs(&key_fn1, &rel1.tuples, env)?;
    let mut new_tuples = Vec::new();
    for i2 in 0..rel2.tuples.len() {
        if let Some(found) = hash.get(&key_fn2.key(&rel2.tuples, i2, env)?) {
            for i1 in found.iter().rev() {
                new_tuples.push(join_tuples(&rel1.tuples, *i1 as usize, &keep_idxs1, &rel2.tuples, i2, &keep_idxs2));
            }
        }
    }
    Ok(Relation::rows(Attrs::numbered(keep_attrs1.into_iter().chain(keep_attrs2)), new_tuples))
}

/// `(reduce hash-join rels)`
fn join_all(rels: &[Relation], env: &Env) -> Result<Relation> {
    let mut it = rels.iter();
    let Some(first) = it.next() else {
        // (hash-join) of nothing, as JavaScript calls it: no variables, no tuples
        return Ok(Relation::rows(Attrs::default(), Vec::new()));
    };
    let mut acc = first.clone();
    for rel in it {
        acc = hash_join(&acc, rel, env)?;
    }
    Ok(acc)
}

/// `subtract-rel`: the tuples of `a` whose key no tuple of `b` has.
fn subtract_rel(a: &Relation, b: &Relation) -> Result<Relation> {
    let env = Env::default();
    let attrs = intersect_keys(&a.attrs, &b.attrs);
    let key_fn_b = KeyFn::new(&b.attrs, &attrs, &env);
    let hash = hash_attrs(&key_fn_b, &b.tuples, &env)?;
    let key_fn_a = KeyFn::new(&a.attrs, &attrs, &env);
    let mut keep = Vec::new();
    for i in 0..a.tuples.len() {
        if !hash.contains_key(&key_fn_a.key(&a.tuples, i, &env)?) {
            keep.push(i);
        }
    }
    let tuples = match &*a.tuples {
        Tuples::Datoms(ds) => Tuples::Datoms(keep.into_iter().map(|i| ds[i].clone()).collect()),
        Tuples::Rows(rows) => Tuples::Rows(keep.into_iter().map(|i| rows[i].clone()).collect()),
    };
    Ok(Relation { attrs: a.attrs.clone(), tuples: Arc::new(tuples) })
}

/// `sum-rel*`: the tuples of `b` laid out as `a`'s rows, after `a`'s.
fn sum_rel_rows(attrs_a: &Attrs, mut rows: Vec<Vec<Value>>, attrs_b: &Attrs, tuples_b: &Tuples) -> Relation {
    let idxb_idxa: Vec<(Idx, usize)> = attrs_b
        .0
        .iter()
        .filter_map(|(sym, idx_b)| match attrs_a.get(sym) {
            Some(Idx::Pos(idx_a)) => Some((*idx_b, idx_a)),
            _ => None,
        })
        .collect();
    let tlen = attrs_a
        .0
        .iter()
        .filter_map(|(_, i)| match i {
            Idx::Pos(p) => Some(*p + 1),
            _ => None,
        })
        .max()
        .unwrap_or(0);
    for i in 0..tuples_b.len() {
        let mut tuple = vec![Value::Nil; tlen];
        for (idx_b, idx_a) in &idxb_idxa {
            tuple[*idx_a] = tuples_b.get(i, *idx_b);
        }
        rows.push(tuple);
    }
    Relation::rows(attrs_a.clone(), rows)
}

/// `sum-rel`: the union of two relations over the same variables.
fn sum_rel(a: &Relation, b: &Relation) -> Result<Relation> {
    let (attrs_a, attrs_b) = (&*a.attrs, &*b.attrs);
    let same_map = attrs_a.0.len() == attrs_b.0.len() && attrs_a.0.iter().all(|(s, i)| attrs_b.get(s) == Some(*i));
    if same_map {
        let tuples = match (&*a.tuples, &*b.tuples) {
            (Tuples::Datoms(x), Tuples::Datoms(y)) => Tuples::Datoms(x.iter().chain(y).cloned().collect()),
            (Tuples::Rows(x), Tuples::Rows(y)) => Tuples::Rows(x.iter().chain(y).cloned().collect()),
            // equal maps over tuples of two kinds are both empty of variables
            (x, y) => {
                let row = |t: &Tuples, i: usize| -> Vec<Value> {
                    match t {
                        Tuples::Rows(r) => r[i].clone(),
                        Tuples::Datoms(d) => d[i].seq_values(),
                    }
                };
                Tuples::Rows((0..x.len()).map(|i| row(x, i)).chain((0..y.len()).map(|i| row(y, i))).collect())
            }
        };
        return Ok(Relation { attrs: a.attrs.clone(), tuples: Arc::new(tuples) });
    }
    // before the keys are compared: a relation may have been cut short
    if a.tuples.is_empty() {
        return Ok(b.clone());
    }
    if b.tuples.is_empty() {
        return Ok(a.clone());
    }
    let same_keys = attrs_a.0.len() == attrs_b.0.len()
        && attrs_a.keys().all(|s| attrs_b.contains(s))
        && attrs_b.keys().all(|s| attrs_a.contains(s));
    if !same_keys {
        raise!("Can’t sum relations with different attrs: ", attrs_a.to_value(), " and ", attrs_b.to_value();
            {"error" => Value::kw("query/where")})
    }
    if attrs_a.all_positions() {
        let rows = match &*a.tuples {
            Tuples::Rows(rows) => rows.clone(),
            Tuples::Datoms(_) => Vec::new(),
        };
        return Ok(sum_rel_rows(attrs_a, rows, attrs_b, &b.tuples));
    }
    // datoms cannot be added to: as rows first
    let number_attrs = Attrs::numbered(attrs_a.keys().cloned());
    let as_rows = sum_rel_rows(&number_attrs, Vec::new(), attrs_a, &a.tuples);
    sum_rel(&as_rows, b)
}

fn sum_all(rels: &[Relation]) -> Result<Relation> {
    let mut it = rels.iter();
    let Some(first) = it.next() else {
        return Ok(Relation::rows(Attrs::default(), Vec::new()));
    };
    let mut acc = first.clone();
    for rel in it {
        acc = sum_rel(&acc, rel)?;
    }
    Ok(acc)
}

/// `(prod-rel)`: no variables, one empty tuple.
fn prod_unit() -> Relation {
    Relation::rows(Attrs::default(), vec![Vec::new()])
}

/// `prod-rel`: every tuple of the first with every tuple of the second.
fn prod_rel(rel1: &Relation, rel2: &Relation) -> Relation {
    let idxs1: Vec<Idx> = rel1.attrs.0.iter().map(|(_, i)| *i).collect();
    let idxs2: Vec<Idx> = rel2.attrs.0.iter().map(|(_, i)| *i).collect();
    let mut rows = Vec::with_capacity(rel1.tuples.len() * rel2.tuples.len());
    for i1 in 0..rel1.tuples.len() {
        for i2 in 0..rel2.tuples.len() {
            rows.push(join_tuples(&rel1.tuples, i1, &idxs1, &rel2.tuples, i2, &idxs2));
        }
    }
    Relation::rows(Attrs::numbered(rel1.attrs.keys().chain(rel2.attrs.keys()).cloned()), rows)
}

fn prod_all(rels: &[Relation]) -> Relation {
    let mut it = rels.iter();
    let Some(first) = it.next() else { return prod_unit() };
    let mut acc = first.clone();
    for rel in it {
        acc = prod_rel(&acc, rel);
    }
    acc
}

// ---------------------------------------------------------------- inputs

/// `empty-rel`: a binding's variables, and no tuples.
fn empty_rel(binding: &Value) -> Relation {
    let vars = parser::collect_vars_distinct(binding).iter().filter_map(record_symbol).collect::<Vec<_>>();
    Relation::rows(Attrs::numbered(vars), Vec::new())
}

/// `in->rel`: the relation a binding makes of a value.
fn in_to_rel(binding: &Value, value: &Value) -> Result<Relation> {
    let Some(r) = as_record(binding) else {
        return Err(Error::msg(message!("No protocol method IBinding.in->rel defined for type ", binding)));
    };
    match r.name {
        "BindIgnore" => Ok(prod_unit()),
        "BindScalar" => {
            let sym = record_symbol(r.get("variable")).ok_or_else(|| Error::msg("a scalar binding has a variable"))?;
            Ok(Relation::rows(Attrs::numbered([sym]), vec![vec![value.clone()]]))
        }
        "BindColl" => {
            if !value.is_seqable() {
                raise!("Cannot bind value ", value, " to collection ", parser::source(binding);
                    {"error" => Value::kw("query/binding"), "value" => value.clone(), "binding" => parser::source(binding)})
            }
            let items = value.seq_items().unwrap_or_default();
            if items.is_empty() {
                return Ok(empty_rel(binding));
            }
            let rels = items.iter().map(|x| in_to_rel(r.get("binding"), x)).collect::<Result<Vec<_>>>()?;
            sum_all(&rels)
        }
        "BindTuple" => {
            if !value.is_seqable() {
                raise!("Cannot bind value ", value, " to tuple ", parser::source(binding);
                    {"error" => Value::kw("query/binding"), "value" => value.clone(), "binding" => parser::source(binding)})
            }
            let bindings = r.get("bindings").as_seq().unwrap_or(&[]);
            let items = value.seq_items().unwrap_or_default();
            if items.len() < bindings.len() {
                raise!("Not enough elements in a collection ", value, " to bind tuple ", parser::source(binding);
                    {"error" => Value::kw("query/binding"), "value" => value.clone(), "binding" => parser::source(binding)})
            }
            let rels = bindings.iter().zip(&items).map(|(b, x)| in_to_rel(b, x)).collect::<Result<Vec<_>>>()?;
            Ok(prod_all(&rels))
        }
        _ => Err(Error::msg(message!("No protocol method IBinding.in->rel defined for type ", binding))),
    }
}

/// The query's `parse-rules`: the rules validated, and their branches by name.
fn parse_rules(rules: &Value) -> Result<HashMap<Value, Vec<Value>>> {
    // a string of rules is read, for callers that have no data to pass
    let rules = match rules {
        Value::Str(s) => crate::edn::read_string(s)?,
        other => other.clone(),
    };
    parser::parse_rules(&rules)?;
    let mut out: HashMap<Value, Vec<Value>> = HashMap::new();
    for rule in clj::seq(&rules)? {
        // (ffirst rule)
        let head = rule.seq_items().and_then(|r| r.into_iter().next()).unwrap_or(Value::Nil);
        let name = head.seq_items().and_then(|h| h.into_iter().next()).unwrap_or(Value::Nil);
        out.entry(name).or_default().push(rule);
    }
    Ok(out)
}

/// `resolve-in`
fn resolve_in(context: &mut Context, binding: &Value, value: &Value) -> Result<()> {
    if let Some(r) = as_record(binding).filter(|r| r.is("BindScalar")) {
        let var = r.get("variable");
        if is_record(var, "SrcVar") {
            if let Some(sym) = record_symbol(var) {
                Arc::make_mut(&mut context.sources).insert(sym, value.clone());
            }
            return Ok(());
        }
        if is_record(var, "RulesVar") {
            context.rules = Arc::new(parse_rules(value)?);
            return Ok(());
        }
    }
    context.conj_rel(in_to_rel(binding, value)?);
    Ok(())
}

/// `resolve-ins`: each of `:in`'s bindings with its input. They are paired in a map, so they are taken in the
/// order of ClojureScript's map of them.
fn resolve_ins(context: &mut Context, bindings: &[Value], values: &[Value]) -> Result<()> {
    let (cb, cv) = (bindings.len(), values.len());
    if cb != cv {
        let expected = Value::vector(
            bindings.iter().map(|b| as_record(b).and_then(|r| r.source.clone()).unwrap_or(Value::Nil)).collect(),
        );
        let data = Value::kw_map(&[
            ("error", Value::kw("query/inputs")),
            ("expected", Value::vector(bindings.to_vec())),
            ("got", if values.is_empty() { Value::Nil } else { Value::list(values.to_vec()) }),
        ]);
        let what = if cb < cv { "Extra inputs passed, expected: " } else { "Too few inputs passed, expected: " };
        return Err(Error::new(message!(what, expected, ", got: ", cv), data));
    }
    let paired = clj::zipmap(bindings, values);
    for (binding, value) in paired.iter() {
        resolve_in(context, binding, value)?;
    }
    Ok(())
}

// ---------------------------------------------------------------- patterns

/// `rel-with-attr`
fn rel_with_attr<'a>(context: &'a Context, sym: &Symbol) -> Option<&'a Relation> {
    context.rels.iter().find(|r| r.attrs.contains(sym))
}

/// `context-resolve-val`: a variable's value in the first tuple of the relation that has it.
fn context_resolve_val(context: &Context, sym: &Symbol) -> Option<Value> {
    let rel = rel_with_attr(context, sym)?;
    if rel.tuples.is_empty() {
        return None;
    }
    Some(rel.tuples.get(0, rel.attrs.get(sym)?))
}

/// `substitute-constant`: a variable bound to one value only is that value.
fn substitute_constant(context: &Context, el: &Value) -> Option<Value> {
    let Value::Symbol(sym) = el else { return None };
    if !is_free_var(el) {
        return None;
    }
    let rel = rel_with_attr(context, sym)?;
    if rel.tuples.len() != 1 {
        return None;
    }
    Some(rel.tuples.get(0, rel.attrs.get(sym)?)).filter(Value::truthy)
}

/// `resolve-pattern-lookup-refs`: the lookup refs and idents of a pattern over a database, as entity ids.
fn resolve_pattern_lookup_refs(source: Option<&Value>, pattern: &[Value]) -> Result<Vec<Value>> {
    let Some(Value::Db(db)) = source else { return Ok(pattern.to_vec()) };
    let get = |i: usize| pattern.get(i).cloned().unwrap_or(Value::Nil);
    let (e, a, v, tx) = (get(0), get(1), get(2), get(3));
    let e2 = if is_lookup_ref(&e) || is_attr(&e) { Value::from(entid_strict(db, &e)?) } else { e };
    let v2 = if v.is_nil() || !is_attr(&a) {
        v
    } else {
        let p = props_of(db, &a);
        if p.is_ref {
            if is_lookup_ref(&v) || is_attr(&v) {
                Value::from(entid_strict(db, &v)?)
            } else {
                v
            }
        } else if p.tuple && v.is_sequential() {
            let whole = Value::vector(pattern.to_vec());
            resolve_tuple_refs(db, &a, &v, || whole.clone())?
        } else {
            v
        }
    };
    let tx2 = if is_lookup_ref(&tx) { Value::from(entid_strict(db, &tx)?) } else { tx };
    if pattern.len() > 4 {
        return Err(Error::msg("Index out of bounds"));
    }
    let mut out = vec![e2, a, v2, tx2];
    out.truncate(pattern.len());
    Ok(out)
}

/// `lookup-pattern-db`: the datoms a pattern finds, as a relation over its variables.
fn lookup_pattern_db(context: &Context, db: &Db, pattern: &[Value]) -> Result<Relation> {
    let substituted: Vec<Value> =
        pattern.iter().map(|el| substitute_constant(context, el).unwrap_or_else(|| el.clone())).collect();
    let resolved = resolve_pattern_lookup_refs(Some(&Value::Db(db.clone())), &substituted)?;
    let part =
        |i: usize| -> Option<&Value> { resolved.get(i).filter(|v| !(is_blank(v) || is_free_var(v) || v.is_nil())) };
    let datoms = {
        let e = match part(0) {
            None => None,
            Some(Value::Num(n)) => match crate::datom::id_from_num(*n) {
                Ok(e) => Some(e),
                // no datom has such an entity
                Err(_) => return Ok(Relation::new(pattern_attrs(pattern), Tuples::Datoms(Vec::new()))),
            },
            Some(other) if other.truthy() => {
                return Err(Error::msg(message!("Cannot compare ", other, " to an entity id")));
            }
            Some(_) => None,
        };
        let a = match part(1) {
            None => None,
            Some(a) if !a.truthy() => None,
            Some(a) => Some(
                value_attr(a)
                    .ok_or_else(|| Error::msg(format!("Cannot compare {} to an attribute", crate::print::str_of(a))))?,
            ),
        };
        let v = part(2);
        let tx = match part(3) {
            None => None,
            Some(Value::Num(n)) => match crate::datom::id_from_num(*n) {
                Ok(tx) => Some(tx),
                Err(_) => return Ok(Relation::new(pattern_attrs(pattern), Tuples::Datoms(Vec::new()))),
            },
            Some(other) if other.truthy() => {
                return Err(Error::msg(message!("Cannot compare ", other, " to a transaction id")));
            }
            Some(_) => None,
        };
        search(db, e, a.as_ref(), v, tx).to_vec()?
    };
    Ok(Relation::new(pattern_attrs(pattern), Tuples::Datoms(datoms)))
}

/// A pattern's variables, each at the datom field of its place; a variable used twice is read at its last.
fn pattern_attrs(pattern: &[Value]) -> Attrs {
    let mut attrs = Attrs::default();
    for (el, idx) in pattern.iter().zip([Idx::E, Idx::A, Idx::V, Idx::Tx]) {
        if let (true, Value::Symbol(sym)) = (is_free_var(el), el) {
            attrs.assoc(sym.clone(), idx);
        }
    }
    attrs
}

/// `matches-pattern?`: a tuple against a pattern, as far as the shorter of them goes. Their first elements are
/// compared whatever their lengths: a tuple of nothing has `nil` there.
fn matches_pattern(pattern: &[Value], tuple: &[Value]) -> bool {
    let mut i = 0;
    loop {
        let (t, p) = (tuple.get(i).unwrap_or(&Value::Nil), pattern.get(i).unwrap_or(&Value::Nil));
        if !(is_blank(p) || is_free_var(p) || t == p) {
            return false;
        }
        i += 1;
        if i >= tuple.len() || i >= pattern.len() {
            return true;
        }
    }
}

/// `lookup-pattern-coll`: the tuples of a collection a pattern matches.
fn lookup_pattern_coll(coll: &Value, pattern: &[Value]) -> Result<Relation> {
    let mut rows = Vec::new();
    for tuple in clj::seq(coll)? {
        // a tuple that is nil matches any pattern
        let items = clj::seq(&tuple)?;
        if tuple.is_nil() || matches_pattern(pattern, &items) {
            rows.push(items);
        }
    }
    let mut attrs = Attrs::default();
    for (i, el) in pattern.iter().enumerate() {
        if let (true, Value::Symbol(sym)) = (is_free_var(el), el) {
            attrs.assoc(sym.clone(), Idx::Pos(i));
        }
    }
    Ok(Relation::rows(attrs, rows))
}

/// `lookup-pattern`
fn lookup_pattern(context: &Context, source: Option<&Value>, pattern: &[Value]) -> Result<Relation> {
    match source {
        Some(Value::Db(db)) => lookup_pattern_db(context, db, pattern),
        Some(coll) => lookup_pattern_coll(coll, pattern),
        None => lookup_pattern_coll(&Value::Nil, pattern),
    }
}

/// `collapse-rels`: the new relation joined with every relation it shares a variable with, after the rest.
fn collapse_rels(rels: &[Relation], new_rel: Relation, env: &Env) -> Result<Vec<Relation>> {
    let mut new_rel = new_rel;
    let mut acc = Vec::with_capacity(rels.len() + 1);
    for rel in rels {
        if rel.attrs.keys().any(|s| new_rel.attrs.contains(s)) {
            new_rel = hash_join(rel, &new_rel, env)?;
        } else {
            acc.push(rel.clone());
        }
    }
    acc.push(new_rel);
    Ok(acc)
}

/// `rel-prod-by-attrs`: the relations that hold any of these variables, taken out of the context and multiplied.
fn rel_prod_by_attrs(context: &mut Context, attrs: &[Symbol]) -> Relation {
    let (picked, rest): (Vec<Relation>, Vec<Relation>) =
        context.rels.drain(..).partition(|rel| attrs.iter().any(|a| rel.attrs.contains(a)));
    // what is left is a lazy sequence now, which `conj` adds to the front of
    context.rels = rest;
    context.rels_seq = true;
    prod_all(&picked)
}

/// `-call-fn`: a function over the tuples of a relation, its arguments the constants, sources and variables of
/// the call.
struct CallFn<'a> {
    f: &'a Value,
    static_args: Vec<Value>,
    tuple_args: Vec<Option<Idx>>,
}

impl<'a> CallFn<'a> {
    fn new(context: &Context, rel: &Relation, f: &'a Value, args: &[Value]) -> CallFn<'a> {
        let mut static_args = vec![Value::Nil; args.len()];
        let mut tuple_args = vec![None; args.len()];
        for (i, arg) in args.iter().enumerate() {
            match arg {
                Value::Symbol(sym) => match context.sources.get(sym) {
                    Some(source) => static_args[i] = source.clone(),
                    None => tuple_args[i] = rel.attrs.get(sym),
                },
                other => static_args[i] = other.clone(),
            }
        }
        CallFn { f, static_args, tuple_args }
    }

    fn call(&mut self, tuples: &Tuples, i: usize) -> Result<Value> {
        // as the original, the arguments of one call are the next call's until overwritten
        for (slot, idx) in self.static_args.iter_mut().zip(&self.tuple_args) {
            if let Some(idx) = idx {
                *slot = tuples.get(i, *idx);
            }
        }
        call(self.f, &self.static_args)
    }
}

fn symbols_of(args: &[Value]) -> Vec<Symbol> {
    args.iter().filter_map(|a| a.as_symbol().cloned()).collect()
}

/// A call's function: built in, or the value of a variable.
fn resolve_fn(context: &Context, f: &Value, what: &str, clause: &Value) -> Result<Option<Value>> {
    let sym = f.as_symbol();
    if let Some(found) = sym.and_then(query_fn) {
        return Ok(Some(found));
    }
    if let Some(found) = sym.and_then(|s| context_resolve_val(context, s)).filter(Value::truthy) {
        return Ok(Some(found));
    }
    if sym.is_none_or(|s| rel_with_attr(context, s).is_none()) {
        return Err(Error::new(
            message!("Unknown ", what, " '", f, " in ", clause),
            Value::kw_map(&[("error", Value::kw("query/where")), ("form", clause.clone()), ("var", f.clone())]),
        ));
    }
    Ok(None)
}

/// `filter-by-pred`: `[(pred ?a ?b)]`
fn filter_by_pred(context: &mut Context, clause: &Value, items: &[Value]) -> Result<()> {
    let call_form = items[0].as_seq().unwrap_or(&[]);
    let f = call_form.first().cloned().unwrap_or(Value::Nil);
    let args = call_form.get(1..).unwrap_or(&[]);
    let pred = resolve_fn(context, &f, "predicate", clause)?;
    let production = rel_prod_by_attrs(context, &symbols_of(args));
    let new_rel = match &pred {
        Some(pred) => {
            let mut tuple_pred = CallFn::new(context, &production, pred, args);
            let mut keep = Vec::new();
            for i in 0..production.tuples.len() {
                if tuple_pred.call(&production.tuples, i)?.truthy() {
                    keep.push(i);
                }
            }
            let tuples = match &*production.tuples {
                Tuples::Datoms(ds) => Tuples::Datoms(keep.into_iter().map(|i| ds[i].clone()).collect()),
                Tuples::Rows(rows) => Tuples::Rows(keep.into_iter().map(|i| rows[i].clone()).collect()),
            };
            Relation { attrs: production.attrs.clone(), tuples: Arc::new(tuples) }
        }
        None => Relation { attrs: production.attrs.clone(), tuples: Arc::new(Tuples::Rows(Vec::new())) },
    };
    context.conj_rel(new_rel);
    Ok(())
}

/// `bind-by-fn`: `[(fn ?a ?b) ?res]`
fn bind_by_fn(context: &mut Context, clause: &Value, items: &[Value]) -> Result<()> {
    let call_form = items[0].as_seq().unwrap_or(&[]);
    let f = call_form.first().cloned().unwrap_or(Value::Nil);
    let args = call_form.get(1..).unwrap_or(&[]);
    let binding = parser::parse_binding(&items[1])?;
    let fun = resolve_fn(context, &f, "function", clause)?;
    let production = rel_prod_by_attrs(context, &symbols_of(args));
    let env = Env::default();
    let new_rel = match &fun {
        Some(fun) => {
            let mut tuple_fn = CallFn::new(context, &production, fun, args);
            let mut rels = Vec::new();
            for i in 0..production.tuples.len() {
                let val = tuple_fn.call(&production.tuples, i)?;
                if val.is_nil() {
                    continue;
                }
                let one = match &*production.tuples {
                    Tuples::Datoms(ds) => Tuples::Datoms(vec![ds[i].clone()]),
                    Tuples::Rows(rows) => Tuples::Rows(vec![rows[i].clone()]),
                };
                let tuple_rel = Relation { attrs: production.attrs.clone(), tuples: Arc::new(one) };
                rels.push(prod_all(&collapse_rels(&[tuple_rel], in_to_rel(&binding, &val)?, &env)?));
            }
            if rels.is_empty() {
                prod_rel(&production, &empty_rel(&binding))
            } else {
                sum_all(&rels)?
            }
        }
        None => {
            let none = Relation { attrs: production.attrs.clone(), tuples: Arc::new(Tuples::Rows(Vec::new())) };
            prod_rel(&none, &empty_rel(&binding))
        }
    };
    let rels = collapse_rels(&context.rels, new_rel, &env)?;
    context.set_rels(rels);
    Ok(())
}

// ---------------------------------------------------------------- rules

/// `rule?`: whether a clause calls a rule. A clause that starts with a plain symbol calls one, which must be known.
fn is_rule(context: &Context, clause: &Value) -> Result<bool> {
    let Some(items) = clause.as_seq() else { return Ok(false) };
    let head = match items.first() {
        Some(first) if is_source(first) => items.get(1),
        other => other,
    };
    let Some(Value::Symbol(sym)) = head else { return Ok(false) };
    if sym.name().starts_with('?') || matches!(sym.full(), "_" | "or" | "or-join" | "and" | "not" | "not-join") {
        return Ok(false);
    }
    if !context.rules.contains_key(&Value::Symbol(sym.clone())) {
        raise!("Unknown rule '", sym, " in ", clause; {"error" => Value::kw("query/where"), "form" => clause.clone()})
    }
    Ok(true)
}

static RULE_SEQID: AtomicU64 = AtomicU64::new(0);

/// `clojure.walk/postwalk`, over the forms of a query.
fn postwalk(form: &Value, f: &dyn Fn(Value) -> Value) -> Value {
    match form {
        Value::Vector(items) => f(Value::vector(items.iter().map(|x| postwalk(x, f)).collect())),
        Value::List(items) => f(Value::list(items.iter().map(|x| postwalk(x, f)).collect())),
        Value::Map(m) => {
            let mut out = m.empty_like();
            for (k, v) in m.iter() {
                out.assoc(postwalk(k, f), postwalk(v, f));
            }
            f(Value::map(out))
        }
        Value::Set(s) => f(Value::set(s.iter().map(|x| postwalk(x, f)).collect())),
        other => f(other.clone()),
    }
}

/// `expand-rule`: each branch of a rule, its head's variables replaced by the call's arguments, and its other
/// variables renamed so that no two expansions share them.
fn expand_rule(clause: &[Value], context: &Context) -> Vec<Vec<Value>> {
    let rule = &clause[0];
    let call_args = &clause[1..];
    let seqid = RULE_SEQID.fetch_add(1, AtomicOrdering::Relaxed) + 1;
    let branches = context.rules.get(rule).cloned().unwrap_or_default();
    branches
        .iter()
        .map(|branch| {
            let parts = branch.seq_items().unwrap_or_default();
            let head = parts.first().and_then(Value::seq_items).unwrap_or_default();
            let rule_args = head.get(1..).unwrap_or(&[]);
            let replacements = clj::zipmap(rule_args, call_args);
            parts
                .get(1..)
                .unwrap_or(&[])
                .iter()
                .map(|c| {
                    postwalk(c, &|x| {
                        if is_free_var(&x) {
                            match replacements.get(&x).filter(|r| r.is_some()) {
                                Some(r) => r.clone(),
                                None => {
                                    let name = x.as_symbol().map_or("", |s| s.name());
                                    Value::Symbol(Symbol::parse(&format!("{name}__auto__{seqid}")))
                                }
                            }
                        } else {
                            x
                        }
                    })
                })
                .collect()
        })
        .collect()
}

/// `rule-gen-guards`: for each earlier call of the rule on this path, a clause that this call's arguments differ
/// from that call's.
fn rule_gen_guards(rule_clause: &[Value], used_args: &HashMap<Value, Vec<Vec<Value>>>) -> Vec<Value> {
    let call_args = &rule_clause[1..];
    used_args
        .get(&rule_clause[0])
        .map(|prev| {
            prev.iter()
                .map(|prev_args| {
                    // remove-pairs
                    let pairs: Vec<(&Value, &Value)> =
                        call_args.iter().zip(prev_args).filter(|(x, y)| x != y).collect();
                    let mut guard = vec![Value::sym("-differ?")];
                    guard.extend(pairs.iter().map(|(x, _)| (*x).clone()));
                    guard.extend(pairs.iter().map(|(_, y)| (*y).clone()));
                    Value::vector(vec![Value::list(guard)])
                })
                .collect()
        })
        .unwrap_or_default()
}

/// `walk-collect` with `free-var?`: the variables of a form, in the order a walk meets them.
fn walk_free_vars(form: &Value, acc: &mut Vec<Value>) {
    match form {
        Value::Vector(items) | Value::List(items) => items.iter().for_each(|x| walk_free_vars(x, acc)),
        Value::Map(m) => m.iter().for_each(|(k, v)| {
            walk_free_vars(k, acc);
            walk_free_vars(v, acc);
        }),
        Value::Set(s) => s.iter().for_each(|x| walk_free_vars(x, acc)),
        other => {
            if is_free_var(other) {
                acc.push(other.clone());
            }
        }
    }
}

/// `collect-vars`: the set of a form's variables.
fn collect_vars(form: &Value) -> CljSet {
    let mut acc = Vec::new();
    walk_free_vars(form, &mut acc);
    acc.into_iter().collect()
}

fn collect_vars_all(forms: &[Value]) -> CljSet {
    let mut acc = Vec::new();
    forms.iter().for_each(|f| walk_free_vars(f, &mut acc));
    acc.into_iter().collect()
}

/// `split-guards`: the guards whose arguments the clauses so far bind, and the rest.
fn split_guards(clauses: &[Value], guards: Vec<Value>) -> (Vec<Value>, Vec<Value>) {
    let bound = collect_vars_all(clauses);
    guards.into_iter().partition(|guard| {
        let call = guard.as_seq().and_then(|g| g.first()).and_then(Value::as_seq).unwrap_or(&[]);
        call.iter().skip(1).all(|v| bound.contains(v))
    })
}

struct Frame {
    prefix_clauses: Vec<Value>,
    prefix_context: Context,
    clauses: Vec<Value>,
    used_args: HashMap<Value, Vec<Vec<Value>>>,
    pending_guards: Vec<Value>,
}

/// `solve-rule`: the relation of a rule call. Its branches are expanded depth first; a call with the arguments of
/// a call it came from is cut.
fn solve_rule(context: &Context, clause: &Value, env: &Env) -> Result<Relation> {
    let clause_items = clause.as_seq().unwrap_or(&[]);
    let final_attrs: Vec<Symbol> =
        clause_items.iter().filter(|v| is_free_var(v)).filter_map(|v| v.as_symbol().cloned()).collect();
    let final_attrs_map = Attrs::numbered(final_attrs.iter().cloned());
    let solve = |prefix: &Context, clauses: &[Value]| -> Result<Context> {
        let mut context = prefix.clone();
        for c in clauses {
            context = resolve_clause_inner(context, c, c, env)?;
        }
        Ok(context)
    };
    let mut stack: Vec<Frame> = vec![Frame {
        prefix_clauses: Vec::new(),
        prefix_context: context.clone(),
        clauses: vec![clause.clone()],
        used_args: HashMap::new(),
        pending_guards: Vec::new(),
    }];
    let mut rel = Relation::rows(final_attrs_map.clone(), Vec::new());
    // the stack's top is its last
    while let Some(frame) = stack.pop() {
        let mut split = frame.clauses.len();
        for (i, c) in frame.clauses.iter().enumerate() {
            if is_rule(context, c)? {
                split = i;
                break;
            }
        }
        let clauses = &frame.clauses[..split];
        match frame.clauses.get(split) {
            // no rules: resolve, collect, sum
            None => {
                let solved = solve(&frame.prefix_context, clauses)?;
                let collected = collect_tuples(&solved, &final_attrs);
                let mut seen = CljSet::new();
                let tuples: Vec<Vec<Value>> =
                    collected.into_iter().filter(|t| seen.insert(Value::vector(t.clone()))).collect();
                rel = sum_rel(&rel, &Relation::rows(final_attrs_map.clone(), tuples))?;
            }
            // a rule: its guards, then its branches onto the stack
            Some(rule_clause) => {
                let rule_items = rule_clause.as_seq().unwrap_or(&[]);
                let next_clauses = &frame.clauses[split + 1..];
                let mut guards = rule_gen_guards(rule_items, &frame.used_args);
                guards.extend(frame.pending_guards.iter().cloned());
                let so_far: Vec<Value> = frame.prefix_clauses.iter().chain(clauses).cloned().collect();
                let (active, pending) = split_guards(&so_far, guards);
                // a guard of no arguments never holds: this call repeats one it came from
                let dead = Value::vector(vec![Value::list(vec![Value::sym("-differ?")])]);
                if active.iter().any(|g| *g == dead) {
                    continue;
                }
                let prefix_clauses: Vec<Value> = clauses.iter().chain(&active).cloned().collect();
                let prefix_context = solve(&frame.prefix_context, &prefix_clauses)?;
                if prefix_context.rels.iter().any(|r| r.tuples.is_empty()) {
                    continue;
                }
                let mut used_args = frame.used_args.clone();
                used_args.entry(rule_items[0].clone()).or_default().push(rule_items[1..].to_vec());
                let branches = expand_rule(rule_items, context);
                for branch in branches.into_iter().rev() {
                    stack.push(Frame {
                        prefix_clauses: prefix_clauses.clone(),
                        prefix_context: prefix_context.clone(),
                        clauses: branch.into_iter().chain(next_clauses.iter().cloned()).collect(),
                        used_args: used_args.clone(),
                        pending_guards: pending.clone(),
                    });
                }
            }
        }
    }
    Ok(rel)
}

// ---------------------------------------------------------------- clauses

/// `dynamic-lookup-attrs`: the variables of a pattern over a database whose values may be lookup refs.
fn dynamic_lookup_attrs(db: &Db, pattern: &[Value]) -> Vec<(Symbol, Resolver)> {
    let get = |i: usize| pattern.get(i).cloned().unwrap_or(Value::Nil);
    let (e, a, v, tx) = (get(0), get(1), get(2), get(3));
    let mut out: Vec<(Symbol, Resolver)> = Vec::new();
    let mut assoc = |sym: &Value, r: Resolver| {
        if let Value::Symbol(s) = sym {
            match out.iter_mut().find(|(x, _)| x == s) {
                Some(entry) => entry.1 = r,
                None => out.push((s.clone(), r)),
            }
        }
    };
    if is_free_var(&e) {
        assoc(&e, Resolver::Eid);
    }
    if is_free_var(&tx) {
        assoc(&tx, Resolver::Eid);
    }
    if is_free_var(&v) && is_attr(&a) {
        let p = props_of(db, &a);
        if p.is_ref {
            assoc(&v, Resolver::Eid);
        }
        if p.tuple {
            assoc(&v, Resolver::Tuple { attr: a.clone(), pattern: Value::vector(pattern.to_vec()) });
        }
    }
    out
}

/// `limit-context`: the context as only these variables see it.
fn limit_context(context: &Context, vars: &[Value]) -> Context {
    let rels = context
        .rels
        .iter()
        .filter_map(|rel| {
            let mut attrs = Attrs::default();
            for v in vars {
                if let Value::Symbol(s) = v {
                    if let Some(idx) = rel.attrs.get(s) {
                        attrs.assoc(s.clone(), idx);
                    }
                }
            }
            (!attrs.is_empty()).then(|| Relation { attrs: Arc::new(attrs), tuples: rel.tuples.clone() })
        })
        .collect();
    Context { rels, rels_seq: true, sources: context.sources.clone(), rules: context.rules.clone() }
}

/// `bound-vars`
fn bound_vars(context: &Context) -> CljSet {
    context.rels.iter().flat_map(|r| r.attrs.keys().map(|s| Value::Symbol(s.clone())).collect::<Vec<_>>()).collect()
}

/// `clojure.set/difference`
fn difference(a: &CljSet, b: &CljSet) -> CljSet {
    let mut out = a.clone();
    if a.len() < b.len() {
        for x in a.iter() {
            if b.contains(x) {
                out.remove(x);
            }
        }
    } else {
        for x in b.iter() {
            out.remove(x);
        }
    }
    out
}

/// `check-bound`
fn check_bound(bound: &CljSet, vars: &[Value], form: &Value) -> Result<()> {
    // `clojure.set/subset?` of the variables as they are written: more of them than are bound is no subset, though
    // they be one variable written twice
    if vars.len() <= bound.len() && vars.iter().all(|v| bound.contains(v)) {
        return Ok(());
    }
    let missing = difference(&vars.iter().cloned().collect(), bound);
    raise!("Insufficient bindings: ", Value::set(missing.clone()), " not bound in ", form;
        {"error" => Value::kw("query/where"), "form" => form.clone(), "vars" => Value::set(missing)})
}

/// `check-free-same`: every branch of an `or` uses the same variables that are not yet bound.
fn check_free_same(bound: &CljSet, branches: &[Value], form: &Value) -> Result<()> {
    let free: Vec<CljSet> = branches.iter().map(|b| difference(&collect_vars(b), bound)).collect();
    if free.is_empty() {
        return Err(Error::msg("Invalid arity: 0"));
    }
    if free.windows(2).all(|p| p[0] == p[1]) {
        return Ok(());
    }
    let free = Value::vector(free.into_iter().map(Value::set).collect());
    raise!("All clauses in 'or' must use same set of free vars, had ", free, " in ", form;
        {"error" => Value::kw("query/where"), "form" => form.clone(), "vars" => free.clone()})
}

/// `check-free-subset`
fn check_free_subset(bound: &CljSet, vars: &[Value], branches: &[Value]) -> Result<()> {
    let free: CljSet = vars.iter().filter(|v| !bound.contains(v)).cloned().collect();
    for branch in branches {
        let missing = difference(&free, &collect_vars(branch));
        if !missing.is_empty() {
            raise!("All clauses in 'or' must use same set of free vars, had ", Value::set(missing.clone()), " not bound in ", branch;
                {"error" => Value::kw("query/where"), "form" => branch.clone(), "vars" => Value::set(missing)})
        }
    }
    Ok(())
}

/// The call of a predicate or function clause: a sequential form that starts with a symbol, or is empty.
fn is_call_form(v: &Value) -> bool {
    v.as_seq().is_some_and(|s| s.first().is_none_or(Value::is_symbol))
}

/// `-resolve-clause`: one clause that is not a rule call.
fn resolve_clause_inner(mut context: Context, clause: &Value, orig_clause: &Value, env: &Env) -> Result<Context> {
    let Some(items) = clause.as_seq() else {
        return Err(Error::msg(message!("No matching clause: ", clause)));
    };

    // [(pred ?a ?b ?c)]
    if items.len() == 1 && is_call_form(&items[0]) {
        let args: Vec<Value> =
            items[0].as_seq().unwrap_or(&[]).iter().skip(1).filter(|v| is_free_var(v)).cloned().collect();
        check_bound(&bound_vars(&context), &args, clause)?;
        filter_by_pred(&mut context, clause, items)?;
        return Ok(context);
    }

    // [(fn ?a ?b) ?res]
    if items.len() == 2 && is_call_form(&items[0]) {
        let args: Vec<Value> =
            items[0].as_seq().unwrap_or(&[]).iter().skip(1).filter(|v| is_free_var(v)).cloned().collect();
        check_bound(&bound_vars(&context), &args, clause)?;
        bind_by_fn(&mut context, clause, items)?;
        return Ok(context);
    }

    // a source, and anything
    if items.first().is_none_or(is_source) {
        let source = items.first().and_then(Value::as_symbol).and_then(|s| context.sources.get(s).cloned());
        let env = Env { source, lookup_attrs: env.lookup_attrs.clone() };
        return resolve_clause_inner(context, &rest_value(items, 1), clause, &env);
    }

    // (or ...)
    if head_is(items, "or") {
        let branches = &items[1..];
        check_free_same(&bound_vars(&context), branches, clause)?;
        // as the original's lazy sequences are realized: a branch is resolved and joined, then summed with those
        // before it, then the next branch is resolved
        let none = Env::default();
        let mut first: Option<Context> = None;
        let mut sum: Option<Relation> = None;
        for b in branches {
            let resolved = resolve_clause(context.clone(), b, env)?;
            let rel = join_all(&resolved.rels, &none)?;
            sum = Some(match sum {
                None => rel,
                Some(acc) => sum_rel(&acc, &rel)?,
            });
            first.get_or_insert(resolved);
        }
        let mut first = first.unwrap_or(context);
        first.set_rels(vec![sum.unwrap_or_else(|| sum_all(&[]).expect("an empty relation"))]);
        return Ok(first);
    }

    if head_is(items, "or-join") && items.len() > 1 && items[1].is_sequential() {
        let join_vars = items[1].as_seq().unwrap_or(&[]);
        let branches = &items[2..];
        // (or-join [[req-vars] vars] ...)
        if join_vars.first().is_none_or(Value::is_sequential) {
            let req_vars = join_vars.first().and_then(Value::as_seq).unwrap_or(&[]);
            let vars = join_vars.get(1..).unwrap_or(&[]);
            let bound = bound_vars(&context);
            check_bound(&bound, req_vars, orig_clause)?;
            check_free_subset(&bound, vars, branches)?;
            if join_vars.is_empty() {
                return Err(Error::msg("or-join without variables"));
            }
            let mut flat = vec![Value::sym("or-join"), Value::list(req_vars.iter().chain(vars).cloned().collect())];
            flat.extend(branches.iter().cloned());
            return resolve_clause_inner(context, &Value::list(flat), clause, env);
        }
        // (or-join [vars] ...)
        // the variables as the set of them iterates
        let vars: Vec<Value> = join_vars.iter().cloned().collect::<CljSet>().iter().cloned().collect();
        check_free_subset(&bound_vars(&context), &vars, branches)?;
        let join_context = limit_context(&context, &vars);
        let none = Env::default();
        let mut sum: Option<Relation> = None;
        for b in branches {
            let resolved = limit_context(&resolve_clause(join_context.clone(), b, env)?, &vars);
            let rel = join_all(&resolved.rels, &none)?;
            sum = Some(match sum {
                None => rel,
                Some(acc) => sum_rel(&acc, &rel)?,
            });
        }
        let sum = match sum {
            Some(sum) => sum,
            None => sum_all(&[])?,
        };
        let collapsed = collapse_rels(&context.rels, sum, &none)?;
        context.set_rels(collapsed);
        return Ok(context);
    }

    // (and ...)
    if head_is(items, "and") {
        for c in &items[1..] {
            context = resolve_clause(context, c, env)?;
        }
        return Ok(context);
    }

    // (not ...)
    if head_is(items, "not") {
        let clauses = &items[1..];
        let bound = bound_vars(&context);
        let negation_vars = collect_vars_all(clauses);
        if !negation_vars.iter().any(|v| bound.contains(v)) {
            raise!("Insufficient bindings: none of ", Value::set(negation_vars), " is bound in ", orig_clause;
                {"error" => Value::kw("query/where"), "form" => orig_clause.clone()})
        }
        let none = Env::default();
        let joined = join_all(&context.rels, &none)?;
        context.set_rels(vec![joined.clone()]);
        let mut negation_context = context.clone();
        for c in clauses {
            negation_context = resolve_clause(negation_context, c, env)?;
        }
        let negation = subtract_rel(&joined, &join_all(&negation_context.rels, &none)?)?;
        context.set_rels(vec![negation]);
        return Ok(context);
    }

    // (not-join [vars] ...)
    if head_is(items, "not-join") && items.len() > 1 && items[1].is_sequential() {
        let vars = items[1].as_seq().unwrap_or(&[]);
        let clauses = &items[2..];
        check_bound(&bound_vars(&context), vars, orig_clause)?;
        let none = Env::default();
        let joined = join_all(&context.rels, &none)?;
        context.set_rels(vec![joined.clone()]);
        let mut negation_context = limit_context(&context, vars);
        for c in clauses {
            negation_context = resolve_clause(negation_context, c, env)?;
        }
        let negation_context = limit_context(&negation_context, vars);
        let negation = subtract_rel(&joined, &join_all(&negation_context.rels, &none)?)?;
        context.set_rels(vec![negation]);
        return Ok(context);
    }

    // a pattern
    let pattern = resolve_pattern_lookup_refs(env.source.as_ref(), items)?;
    let relation = lookup_pattern(&context, env.source.as_ref(), &pattern)?;
    let join_env = match &env.source {
        Some(Value::Db(db)) => {
            Env { source: env.source.clone(), lookup_attrs: Some(Arc::new(dynamic_lookup_attrs(db, &pattern))) }
        }
        _ => env.clone(),
    };
    let rels = collapse_rels(&context.rels, relation, &join_env)?;
    context.set_rels(rels);
    Ok(context)
}

/// `short-circuit-empty-rel`: once a relation is empty the result is: one empty relation of every variable.
fn short_circuit_empty_rel(mut context: Context) -> Context {
    if context.rels.iter().any(|r| r.tuples.is_empty()) {
        let all = Attrs::numbered(context.rels.iter().flat_map(|r| r.attrs.keys().cloned().collect::<Vec<_>>()));
        context.set_rels(vec![Relation::rows(all, Vec::new())]);
    }
    context
}

/// `resolve-clause`
fn resolve_clause(context: Context, clause: &Value, env: &Env) -> Result<Context> {
    if context.rels.iter().any(|r| r.tuples.is_empty()) {
        // the result is empty already
        return Ok(context);
    }
    let resolved = if is_rule(&context, clause)? {
        let items = clause.as_seq().unwrap_or(&[]);
        if items.first().is_some_and(is_source) {
            let source = items[0].as_symbol().and_then(|s| context.sources.get(s).cloned());
            let env = Env { source, lookup_attrs: env.lookup_attrs.clone() };
            return Ok(short_circuit_empty_rel(resolve_clause(context, &rest_value(items, 1), &env)?));
        }
        let rel = solve_rule(&context, clause, env)?;
        let mut context = context;
        let rels = collapse_rels(&context.rels, rel, env)?;
        context.set_rels(rels);
        context
    } else {
        resolve_clause_inner(context, clause, clause, env)?
    };
    Ok(short_circuit_empty_rel(resolved))
}

// ---------------------------------------------------------------- collecting

/// `-collect`: the product of the relations that hold any of the symbols, a row of the symbols' values each.
fn collect_tuples(context: &Context, symbols: &[Symbol]) -> Vec<Vec<Value>> {
    let mut acc: Vec<Vec<Value>> = vec![vec![Value::Nil; symbols.len()]];
    for rel in &context.rels {
        // one empty relation, and the whole result is empty
        if rel.tuples.is_empty() {
            return Vec::new();
        }
        let copy_map: Vec<Option<Idx>> = symbols.iter().map(|s| rel.attrs.get(s)).collect();
        if copy_map.iter().all(Option::is_none) {
            continue;
        }
        let mut next = Vec::with_capacity(acc.len() * rel.tuples.len());
        for t1 in &acc {
            for i in 0..rel.tuples.len() {
                let mut res = t1.clone();
                for (slot, idx) in res.iter_mut().zip(&copy_map) {
                    if let Some(idx) = idx {
                        *slot = rel.tuples.get(i, *idx);
                    }
                }
                next.push(res);
            }
        }
        acc = next;
    }
    acc
}

/// `collect`: the set of result tuples.
fn collect(context: &Context, symbols: &[Symbol]) -> CljSet {
    collect_tuples(context, symbols).into_iter().map(Value::vector).collect()
}

/// `-context-resolve`: what a find element's part stands for.
fn context_resolve(var: &Value, context: &Context) -> Value {
    let Some(r) = as_record(var) else { return Value::Nil };
    match r.name {
        "Variable" => record_symbol(var).and_then(|s| context_resolve_val(context, &s)).unwrap_or(Value::Nil),
        "SrcVar" => record_symbol(var).and_then(|s| context.sources.get(&s).cloned()).unwrap_or(Value::Nil),
        "PlainSymbol" => record_symbol(var).and_then(|s| aggregate_fn(&s)).unwrap_or(Value::Nil),
        "Constant" => r.get("value").clone(),
        _ => Value::Nil,
    }
}

/// `-aggregate`: one row of a group, its aggregates computed over the group.
fn aggregate_row(elements: &[Value], context: &Context, tuples: &[Value]) -> Result<Value> {
    let first = tuples.first().and_then(Value::as_seq).unwrap_or(&[]);
    let mut out = Vec::with_capacity(elements.len());
    for (i, element) in elements.iter().enumerate() {
        if i >= first.len() {
            break;
        }
        if is_aggregate(element) {
            let r = as_record(element).expect("an aggregate");
            let f = context_resolve(r.get("fn"), context);
            let arg_terms = r.get("args").as_seq().unwrap_or(&[]);
            let mut args: Vec<Value> =
                arg_terms[..arg_terms.len().saturating_sub(1)].iter().map(|a| context_resolve(a, context)).collect();
            let vals: Vec<Value> =
                tuples.iter().map(|t| t.as_seq().and_then(|t| t.get(i)).cloned().unwrap_or(Value::Nil)).collect();
            args.push(Value::list(vals));
            out.push(call(&f, &args)?);
        } else {
            out.push(first[i].clone());
        }
    }
    Ok(Value::vector(out))
}

/// `aggregate`: the result grouped by the find elements that are not aggregates, a row a group.
fn aggregate(elements: &[Value], context: &Context, resultset: &[Value]) -> Result<Vec<Value>> {
    let group_idxs: Vec<usize> =
        elements.iter().enumerate().filter(|(_, e)| !is_aggregate(e)).map(|(i, _)| i).collect();
    let grouped = clj::group_by(resultset, |tuple| {
        let t = tuple.as_seq().unwrap_or(&[]);
        Ok(Value::list(group_idxs.iter().map(|i| t.get(*i).cloned().unwrap_or(Value::Nil)).collect()))
    })?;
    grouped.vals().map(|tuples| aggregate_row(elements, context, tuples.as_seq().unwrap_or(&[]))).collect()
}

/// `tuples->return-map`'s row: `{:key value ...}`
fn return_map_row(symbols: &[Value], tuple: &Value) -> Value {
    let t = tuple.as_seq().unwrap_or(&[]);
    // (nth nil i) is nil: no tuple makes a map of nils
    let at = |i: usize| t.get(i).cloned().unwrap_or(Value::Nil);
    Value::map(symbols.iter().enumerate().map(|(i, s)| (s.clone(), at(i))).collect::<CljMap>())
}

/// `tuples->return-map`: each tuple as a map, in a collection of the kind the tuples came in: a set stays a set, a
/// vector a vector, and a sequence comes out reversed, as `conj` onto an empty list reverses it.
fn tuples_to_return_map(return_map: &Value, tuples: &Value) -> Value {
    let symbols = as_record(return_map).map(|r| r.get("symbols").as_seq().unwrap_or(&[]).to_vec()).unwrap_or_default();
    match tuples {
        Value::Set(s) => Value::set(s.iter().map(|t| return_map_row(&symbols, t)).collect()),
        Value::Vector(v) => Value::vector(v.iter().map(|t| return_map_row(&symbols, t)).collect()),
        Value::List(l) => Value::list(l.iter().rev().map(|t| return_map_row(&symbols, t)).collect()),
        other => other.clone(),
    }
}

/// `-post-process`: the result in the shape `:find` asks for.
fn post_process(find: &Value, return_map: &Value, tuples: Value) -> Value {
    let name = as_record(find).map_or("", |r| r.name);
    let first = |v: &Value| -> Value { v.seq_items().and_then(|s| s.into_iter().next()).unwrap_or(Value::Nil) };
    match name {
        "FindRel" => {
            if return_map.is_nil() {
                tuples
            } else {
                tuples_to_return_map(return_map, &tuples)
            }
        }
        "FindColl" => Value::vector(tuples.seq_items().unwrap_or_default().iter().map(first).collect()),
        "FindScalar" => first(&first(&tuples)),
        "FindTuple" => {
            if return_map.is_some() {
                first(&tuples_to_return_map(return_map, &Value::vector(vec![first(&tuples)])))
            } else {
                first(&tuples)
            }
        }
        _ => tuples,
    }
}

/// `pull`: each pull element of each row replaced by what it pulls.
fn pull_rows(elements: &[Value], context: &Context, resultset: &[Value]) -> Result<Vec<Value>> {
    // the original resolves the patterns lazily: with no rows, a pattern is never looked at
    if resultset.is_empty() {
        return Ok(Vec::new());
    }
    let mut resolved = Vec::with_capacity(elements.len());
    for find in elements {
        if is_pull(find) {
            let r = as_record(find).expect("a pull");
            let db = match context_resolve(r.get("source"), context) {
                Value::Db(db) => db,
                other => return Err(Error::msg(message!("Assert failed: (db/db? db), got ", other))),
            };
            let pattern = context_resolve(r.get("pattern"), context);
            resolved.push(Some(crate::pull_api::parse_opts(&db, &pattern, None)?));
        } else {
            resolved.push(None);
        }
    }
    let mut out = Vec::with_capacity(resultset.len());
    for tuple in resultset {
        let items = tuple.as_seq().unwrap_or(&[]);
        let mut row = Vec::with_capacity(items.len());
        for (opts, el) in resolved.iter().zip(items) {
            row.push(match opts {
                Some(opts) => crate::pull_api::pull_impl(opts, el)?,
                None => el.clone(),
            });
        }
        out.push(Value::vector(row));
    }
    Ok(out)
}

/// The last queries parsed, by their forms (`*query-cache*`).
fn query_cache() -> &'static Cache<Value, Value> {
    static CACHE: OnceLock<Cache<Value, Value>> = OnceLock::new();
    CACHE.get_or_init(|| Cache::new(100))
}

/// `(d/q query & inputs)`
pub fn q(query: &Value, inputs: &[Value]) -> Result<Value> {
    let parsed_q = query_cache().get(query, || parser::parse_query(query))?;
    let parsed = as_record(&parsed_q).ok_or_else(|| Error::msg("a parsed query is a record"))?;
    let find = parsed.get("qfind");
    let elements = find_elements(find);
    let find_var_syms: Vec<Symbol> = find_vars(find).iter().filter_map(|v| v.as_symbol().cloned()).collect();
    let result_arity = elements.len();
    let with: Vec<Symbol> = parsed.get("qwith").as_seq().unwrap_or(&[]).iter().filter_map(record_symbol).collect();
    let all_vars: Vec<Symbol> = find_var_syms.iter().chain(&with).cloned().collect();
    let qm = match query {
        Value::Map(m) => (**m).clone(),
        Value::Vector(items) | Value::List(items) => parser::query_to_map(items),
        _ => CljMap::new(),
    };
    let wheres = qm.get(&Value::kw("where")).and_then(Value::seq_items).unwrap_or_default();
    let mut context = Context { rels: Vec::new(), rels_seq: false, sources: Arc::default(), rules: Arc::default() };
    resolve_ins(&mut context, parsed.get("qin").as_seq().unwrap_or(&[]), inputs)?;

    // -q
    let env = Env { source: context.sources.get(&Symbol::parse("$")).cloned(), lookup_attrs: None };
    let mut resolved = context.clone();
    for clause in &wheres {
        resolved = resolve_clause(resolved, clause, &env)?;
    }
    let resultset = collect(&resolved, &all_vars);

    let mut result = Value::set(resultset);
    if qm.get(&Value::kw("with")).is_some_and(Value::truthy) {
        let rows = result.seq_items().unwrap_or_default();
        result = Value::vector(
            rows.iter()
                .map(|t| Value::vector(t.as_seq().unwrap_or(&[]).iter().take(result_arity).cloned().collect()))
                .collect(),
        );
    }
    if elements.iter().any(is_aggregate) {
        result = Value::list(aggregate(&elements, &context, &result.seq_items().unwrap_or_default())?);
    }
    if elements.iter().any(is_pull) {
        result = Value::list(pull_rows(&elements, &context, &result.seq_items().unwrap_or_default())?);
    }
    Ok(post_process(find, parsed.get("qreturn-map"), result))
}
