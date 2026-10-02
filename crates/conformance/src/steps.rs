//! The steps of a case, as conformance/oracle/src/oracle/core.cljs runs them, printed as it prints them.

use crate::fns;
use datascript::built_ins::call;
use datascript::cmp::value_compare;
use datascript::coll::CljMap;
use datascript::datom::datom_from_reader;
use datascript::print::{pr_str, str_of};
use datascript::value::{Func, HostObj, HostObject};
use datascript::{clj, edn, serialize, vector, Datom, Db, Entity, Error, Index, Result, Value};
use std::any::Any;
use std::collections::HashMap;
use std::sync::Arc;

/// `#r name`: what an earlier step of the case named
struct RefMarker(String);
/// `#f name`: one of the functions both sides know
struct FnMarker(String);

macro_rules! marker {
    ($ty:ident, $tag:literal) => {
        impl HostObject for $ty {
            fn hash(&self) -> i32 {
                0
            }
            fn equiv(&self, other: &dyn HostObject) -> bool {
                other.as_any().downcast_ref::<$ty>().is_some_and(|o| o.0 == self.0)
            }
            fn type_name(&self) -> String {
                concat!("oracle.core/", stringify!($ty)).into()
            }
            fn pr_str(&self) -> String {
                format!(concat!("#", $tag, " {}"), self.0)
            }
            fn as_any(&self) -> &dyn Any {
                self
            }
        }
    };
}
marker!(RefMarker, "r");
marker!(FnMarker, "f");

type Env = HashMap<String, Value>;

fn read_case(line: &str) -> Result<Value> {
    edn::read_string_with(line, &|tag, form| {
        let name = || match form {
            Value::Symbol(s) => Ok(s.full().to_string()),
            other => Err(Error::msg(format!("#{tag} takes a symbol, got {}", pr_str(other)))),
        };
        Ok(match tag {
            "r" => Some(Value::Host(HostObj::new(RefMarker(name()?)))),
            "f" => Some(Value::Host(HostObj::new(FnMarker(name()?)))),
            _ => None,
        })
    })
}

/// `x` with every `#r` replaced by the value the case bound to it, and every `#f` by its function. What holds
/// neither is left as it is, so that it stays the collection it was read as.
fn resolve(env: &Env, x: &Value) -> Result<Value> {
    Ok(resolve_opt(env, x)?.unwrap_or_else(|| x.clone()))
}

fn resolve_opt(env: &Env, x: &Value) -> Result<Option<Value>> {
    match x {
        Value::Host(h) => {
            if let Some(r) = h.0.as_any().downcast_ref::<RefMarker>() {
                return env
                    .get(&r.0)
                    .cloned()
                    .map(Some)
                    .ok_or_else(|| Error::msg(format!("oracle: unbound ref {}", r.0)));
            }
            if let Some(f) = h.0.as_any().downcast_ref::<FnMarker>() {
                return fns::lookup(&f.0).map(Some).ok_or_else(|| Error::msg(format!("oracle: unknown fn {}", f.0)));
            }
            Ok(None)
        }
        Value::Vector(items) | Value::List(items) => {
            let mut changed = false;
            let mut out = Vec::with_capacity(items.len());
            for i in items.iter() {
                match resolve_opt(env, i)? {
                    Some(v) => {
                        changed = true;
                        out.push(v);
                    }
                    None => out.push(i.clone()),
                }
            }
            if !changed {
                return Ok(None);
            }
            Ok(Some(if x.is_vector() { Value::vector(out) } else { Value::list(out) }))
        }
        Value::Map(m) => {
            let mut changed = false;
            let mut out = m.empty_like();
            for (k, v) in m.iter() {
                let (rk, rv) = (resolve_opt(env, k)?, resolve_opt(env, v)?);
                changed |= rk.is_some() || rv.is_some();
                out.assoc(rk.unwrap_or_else(|| k.clone()), rv.unwrap_or_else(|| v.clone()));
            }
            Ok(changed.then(|| Value::map(out)))
        }
        Value::Set(s) => {
            let mut changed = false;
            let mut out = Vec::with_capacity(s.len());
            for i in s.iter() {
                match resolve_opt(env, i)? {
                    Some(v) => {
                        changed = true;
                        out.push(v);
                    }
                    None => out.push(i.clone()),
                }
            }
            Ok(changed.then(|| Value::set(out.into_iter().collect())))
        }
        _ => Ok(None),
    }
}

fn kwv(name: &str) -> Value {
    Value::kw(name)
}

fn datom_value(d: Datom) -> Value {
    Value::Datom(Arc::new(d))
}

fn datoms_vector(ds: Vec<Datom>) -> Value {
    Value::vector(ds.into_iter().map(datom_value).collect())
}

/// A database as the harness prints it: its indexes' sizes, its counters and itself.
pub fn db_line(db: &Db) -> Result<String> {
    let unfiltered = db.unfiltered();
    let index = |index: Index, f: &dyn Fn(&Datom) -> Value| -> Result<Value> {
        if db.is_filtered() {
            return Ok(Value::Nil);
        }
        Ok(Value::vector(db.index(index).to_vec()?.iter().map(f).collect()))
    };
    let mut m = CljMap::new();
    m.assoc(kwv("max-eid"), Value::from(unfiltered.max_eid()));
    m.assoc(kwv("max-tx"), Value::from(unfiltered.max_tx()));
    m.assoc(kwv("count"), Value::from(db.count()?));
    m.assoc(
        kwv("aevt"),
        index(Index::Aevt, &|d| Value::vector(vec![d.a_value(), Value::from(d.e), d.v.clone(), Value::from(d.tx())]))?,
    );
    m.assoc(
        kwv("avet"),
        index(Index::Avet, &|d| Value::vector(vec![d.a_value(), d.v.clone(), Value::from(d.e), Value::from(d.tx())]))?,
    );
    Ok(format!("#db {} {}", pr_str(&Value::map(m)), pr_str(&Value::Db(db.clone()))))
}

/// A database in few words: its counters and two of its indexes.
fn db_digest(db: &Db) -> Result<Value> {
    let unfiltered = db.unfiltered();
    let eavt = db.datoms(Index::Eavt, &[])?.to_vec()?;
    let avet = db.datoms(Index::Avet, &[])?.to_vec()?;
    let row = |d: &Datom, v_first: bool| {
        let (e, a, v, tx) = (Value::from(d.e), d.a_value(), d.v.clone(), Value::from(d.tx()));
        Value::vector(if v_first { vec![a, v, e, tx] } else { vec![e, a, v, tx] })
    };
    Ok(Value::vector(vec![
        Value::from(unfiltered.max_eid()),
        Value::from(unfiltered.max_tx()),
        Value::vector(eavt.iter().map(|d| row(d, false)).collect()),
        Value::vector(avet.iter().map(|d| row(d, true)).collect()),
    ]))
}

pub fn report_line(report: &datascript::TxReport) -> Result<String> {
    let mut m = CljMap::new();
    m.assoc(kwv("tx-data"), datoms_vector(report.tx_data.clone()));
    m.assoc(kwv("tempids"), report.tempids.clone());
    m.assoc(kwv("tx-meta"), report.tx_meta.clone());
    Ok(format!("#report {} {}", pr_str(&Value::map(m)), db_line(&report.db_after)?))
}

pub fn error_line(e: &Error) -> String {
    if e.data.is_nil() {
        format!("#error :native {}", pr_str(&Value::str(&e.message)))
    } else {
        let mut m = CljMap::new();
        m.assoc(kwv("msg"), Value::str(&e.message));
        m.assoc(kwv("error"), e.data.get(&kwv("error")).cloned().unwrap_or(Value::Nil));
        format!("#error {}", pr_str(&Value::map(m)))
    }
}

fn db_arg(v: &Value) -> Result<Db> {
    match v {
        Value::Db(db) => Ok(db.clone()),
        _ => Err(Error::msg("Assert failed: (db/db? db)")),
    }
}

fn index_arg(v: &Value) -> Result<Index> {
    Index::from_value(v).ok_or_else(|| Error::msg(format!("Unknown index {}", pr_str(v))))
}

fn seq_arg(v: &Value) -> Result<Vec<Value>> {
    clj::seq(v)
}

/// One step: what to print, and the value to bind.
fn run_step(env: &Env, step: &Value) -> Result<(String, Value)> {
    let has = |k: &str| step.get(&kwv(k)).is_some();
    let raw = |k: &str| step.get(&kwv(k)).cloned().unwrap_or(Value::Nil);
    let arg = |k: &str| resolve(env, &raw(k));
    let op = match step.get(&kwv("op")) {
        Some(Value::Keyword(k)) => k.full().to_string(),
        other => return Err(Error::msg(format!("oracle: unknown op {}", other.map_or("nil".into(), pr_str)))),
    };
    let p = pr_str;
    let nothing = Value::Nil;
    Ok(match op.as_str() {
        "empty-db" => {
            let db = Db::empty(if has("schema") { arg("schema")? } else { Value::Nil })?;
            (db_line(&db)?, Value::Db(db))
        }
        "init-db" => {
            let datoms: Result<Vec<Datom>> = seq_arg(&arg("datoms")?)?.iter().map(datom_from_reader).collect();
            let db = Db::init(datoms?, if has("schema") { arg("schema")? } else { Value::Nil })?;
            (db_line(&db)?, Value::Db(db))
        }
        "db-with" => {
            let db = datascript::db_with(&db_arg(&arg("db")?)?, &arg("tx")?)?;
            (db_line(&db)?, Value::Db(db))
        }
        "with" => {
            let meta = if has("tx-meta") { arg("tx-meta")? } else { Value::Nil };
            let report = datascript::with(&db_arg(&arg("db")?)?, &arg("tx")?, meta)?;
            (report_line(&report)?, Value::Db(report.db_after))
        }
        "with-schema" => {
            let db = db_arg(&arg("db")?)?.with_schema(arg("schema")?)?;
            (db_line(&db)?, Value::Db(db))
        }
        "filter" => {
            let db = db_arg(&arg("db")?)?;
            let pred = arg("pred")?;
            let filtered = db.filter(Arc::new(move |db, datom| {
                Ok(call(&pred, &[Value::Db(db.clone()), datom_value(datom.clone())])?.truthy())
            }));
            (db_line(&filtered)?, Value::Db(filtered))
        }
        "q" => {
            let inputs = seq_arg(&arg("inputs")?)?;
            let r = datascript::q(&arg("query")?, &inputs)?;
            (p(&r), r)
        }
        "q-count" | "pull-count" => {
            // a query over the database behind a filter that passes everything and counts what it is asked
            let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let counter = count.clone();
            let db = db_arg(&arg("db")?)?.filter(Arc::new(move |_, _| {
                counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                Ok(true)
            }));
            let r = if op == "q-count" {
                let mut inputs = vec![Value::Db(db)];
                inputs.extend(seq_arg(&arg("inputs")?)?);
                datascript::q(&arg("query")?, &inputs)?
            } else {
                datascript::pull(&db, &arg("pattern")?, &arg("eid")?, None)?
            };
            let n = count.load(std::sync::atomic::Ordering::Relaxed);
            (p(&vector![n, r.clone()]), r)
        }
        "pull" => {
            let r = datascript::pull(&db_arg(&arg("db")?)?, &arg("pattern")?, &arg("eid")?, None)?;
            (p(&r), r)
        }
        "pull-depth" => {
            // how deep a pull went along one attribute, and what it found there: too deep to print whole
            let r = datascript::pull(&db_arg(&arg("db")?)?, &arg("pattern")?, &arg("eid")?, None)?;
            let key = arg("key")?;
            let (mut depth, mut node) = (0usize, r.clone());
            loop {
                let next = clj::get(&node, &key).and_then(|v| clj::get(&v, &Value::from(0)));
                match next {
                    Some(next) => {
                        depth += 1;
                        node = next;
                    }
                    None => break,
                }
            }
            (p(&vector![depth, node]), nothing)
        }
        "pull-many" => {
            let ids = seq_arg(&arg("eids")?)?;
            let r = Value::vector(datascript::pull_many(&db_arg(&arg("db")?)?, &arg("pattern")?, &ids, None)?);
            (p(&r), r)
        }
        "pull-visit" => {
            let seen: Arc<std::sync::Mutex<Vec<Value>>> = Arc::default();
            let log = seen.clone();
            let visitor = Value::Fn(Func::new("visitor", move |args| {
                log.lock().unwrap().push(Value::vector(args.to_vec()));
                Ok(Value::Nil)
            }));
            let r = datascript::pull(&db_arg(&arg("db")?)?, &arg("pattern")?, &arg("eid")?, Some(&visitor))?;
            let mut m = CljMap::new();
            m.assoc(kwv("result"), r.clone());
            m.assoc(kwv("visited"), Value::vector(seen.lock().unwrap().clone()));
            (p(&Value::map(m)), r)
        }
        "entity" => {
            let e = datascript::entity(&db_arg(&arg("db")?)?, &arg("eid")?)?;
            let attrs = seq_arg(&arg("attrs")?)?;
            let touch = raw("touch").truthy();
            let view = match e {
                None => Value::Nil,
                Some(e) => {
                    let mut vals = Vec::with_capacity(attrs.len());
                    for a in &attrs {
                        vals.push(e.lookup(a)?.unwrap_or(Value::Nil));
                    }
                    if touch {
                        e.touch()?;
                    }
                    let mut m = CljMap::new();
                    m.assoc(kwv("vals"), Value::vector(vals));
                    m.assoc(kwv("str"), Value::from(p(&e.to_value())));
                    if touch {
                        let entries = e.entries()?;
                        m.assoc(
                            kwv("seq"),
                            Value::vector(entries.iter().map(|(k, v)| vector![k.clone(), v.clone()]).collect()),
                        );
                        m.assoc(kwv("count"), Value::from(entries.len()));
                    } else {
                        m.assoc(kwv("seq"), Value::Nil);
                        m.assoc(kwv("count"), Value::Nil);
                    }
                    Value::map(m)
                }
            };
            (p(&view), nothing)
        }
        "entity-path" => {
            // from an entity along a path: an attribute, or one of the oracle's steps
            let mut x = datascript::entity(&db_arg(&arg("db")?)?, &arg("eid")?)?.map_or(Value::Nil, |e| e.to_value());
            for k in seq_arg(&arg("path")?)? {
                let entity = Entity::from_value(&x);
                let step = k.as_keyword().map(|k| k.full().to_string());
                let op = k.as_seq().and_then(|s| s.first()).and_then(Value::as_keyword).map(|k| k.full().to_string());
                let operand = |i: usize| k.as_seq().and_then(|s| s.get(i)).cloned().unwrap_or(Value::Nil);
                let lookup = |x: &Value, k: &Value| -> Result<Option<Value>> {
                    match Entity::from_value(x) {
                        Some(e) => e.lookup_entry(k),
                        None => Ok(clj::get(x, k)),
                    }
                };
                x = match (step.as_deref(), op.as_deref()) {
                    (Some("oracle/first"), _) => match &entity {
                        Some(e) => e.entries()?.into_iter().next().map_or(Value::Nil, |(k, v)| vector![k, v]),
                        None => clj::seq(&x)?.into_iter().next().unwrap_or(Value::Nil),
                    },
                    (Some("oracle/touch"), _) => {
                        if let Some(e) = &entity {
                            e.touch()?;
                        } else if x.is_some() {
                            return Err(Error::msg("Assert failed: (or (nil? e) (entity? e))"));
                        }
                        x
                    }
                    (Some("oracle/count"), _) => match &entity {
                        Some(e) => Value::from(e.entries()?.len()),
                        None => {
                            call(&datascript::built_ins::query_fn(&datascript::Symbol::parse("count")).unwrap(), &[x])?
                        }
                    },
                    (Some("oracle/seq"), _) => match &entity {
                        Some(e) => Value::vector(e.entries()?.into_iter().map(|(k, v)| vector![k, v]).collect()),
                        None => Value::vector(clj::seq(&x)?),
                    },
                    (Some("oracle/keys"), _) => match &entity {
                        Some(e) => Value::vector(e.entries()?.into_iter().map(|(k, _)| k).collect()),
                        None => match &x {
                            Value::Map(m) => Value::vector(m.keys().cloned().collect()),
                            _ => Value::vector(Vec::new()),
                        },
                    },
                    (_, Some("contains")) => match &entity {
                        Some(e) => Value::Bool(e.contains_key(&operand(1))?),
                        None => Value::Bool(clj::contains(&x, &operand(1))?),
                    },
                    (_, Some("call")) => call(&x, &[operand(1)])?,
                    (_, Some("get")) => lookup(&x, &operand(1))?.unwrap_or_else(|| operand(2)),
                    _ => lookup(&x, &k)?.unwrap_or(Value::Nil),
                };
            }
            (p(&x), nothing)
        }
        "entity-eq" => {
            let e1 = datascript::entity(&db_arg(&arg("a")?)?, &arg("e1")?)?.map_or(Value::Nil, |e| e.to_value());
            let e2 = datascript::entity(&db_arg(&arg("b")?)?, &arg("e2")?)?.map_or(Value::Nil, |e| e.to_value());
            (p(&vector![e1 == e2, e1.cljs_hash() == e2.cljs_hash()]), nothing)
        }
        "parse-query" => (p(&datascript::parser::parse_query(&arg("query")?)?), nothing),
        "parse-pull" => {
            let parsed = datascript::pull_parser::parse_pattern(&db_arg(&arg("db")?)?, &arg("pattern")?)?;
            (p(&datascript::pull_parser::pattern_to_value(&parsed)), nothing)
        }
        "datoms" | "seek-datoms" | "rseek-datoms" => {
            let db = db_arg(&arg("db")?)?;
            let index = index_arg(&arg("index")?)?;
            let components = seq_arg(&arg("components")?)?;
            if components.len() > 4 {
                return Err(Error::msg("Invalid arity: 7"));
            }
            let found = match op.as_str() {
                "datoms" => db.datoms(index, &components)?,
                "seek-datoms" => db.seek_datoms(index, &components)?,
                _ => db.rseek_datoms(index, &components)?,
            };
            let found = match step.get(&kwv("limit")).and_then(Value::as_num) {
                Some(n) => found.take(n as usize)?,
                None => found.to_vec()?,
            };
            (p(&datoms_vector(found)), nothing)
        }
        "find-datom" => {
            let db = db_arg(&arg("db")?)?;
            let components = seq_arg(&arg("components")?)?;
            if components.len() > 4 {
                return Err(Error::msg("Invalid arity: 7"));
            }
            let found = db.find_datom(index_arg(&arg("index")?)?, &components)?;
            let v = found.map_or(Value::Nil, datom_value);
            (p(&v), v)
        }
        "index-range" => {
            let db = db_arg(&arg("db")?)?;
            let found = db.index_range(&arg("attr")?, &arg("start")?, &arg("end")?)?.to_vec()?;
            (p(&datoms_vector(found)), nothing)
        }
        "entid" => {
            let v = db_arg(&arg("db")?)?.entid_value(&arg("eid")?)?;
            (p(&v), v)
        }
        "schema" => (p(&db_arg(&arg("db")?)?.schema_value()), nothing),
        "rschema" => match arg("db")? {
            // (:rschema db), which a filtered database does not answer
            Value::Db(db) if db.is_filtered() => return Err(Error::msg("-lookup is not supported on FilteredDB")),
            Value::Db(db) => (p(&db.rschema_value()), nothing),
            // (:rschema nil), after a step that failed
            _ => ("nil".into(), nothing),
        },
        "db-eq" => (p(&Value::Bool(arg("a")? == arg("b")?)), nothing),
        "db-hash-eq" => (p(&Value::Bool(arg("a")?.cljs_hash() == arg("b")?.cljs_hash())), nothing),
        "db-empty" => {
            let db = db_arg(&arg("db")?)?.empty_like()?;
            (db_line(&db)?, Value::Db(db))
        }
        "diff" => {
            let (a, b) = (arg("a")?, arg("b")?);
            let v = match (&a, &b) {
                (Value::Db(x), Value::Db(y)) => datascript::db::diff(x, y)?,
                // clojure.data/diff of what is no database: equal, or different whole
                _ if a == b => vector![Value::Nil, Value::Nil, a.clone()],
                _ => vector![a.clone(), b.clone(), Value::Nil],
            };
            (p(&v), nothing)
        }
        "serializable" => {
            let text = serialize::serializable_text(&db_arg(&arg("db")?)?, &serialize::Options::default())?;
            let db = serialize::from_serializable_text(&text, &serialize::Options::default())?;
            (format!("{} {}", text, db_line(&db)?), Value::Db(db))
        }
        "from-serializable" => {
            let text = arg("json")?;
            let text = text.as_str().ok_or_else(|| Error::msg("from-serializable takes JSON"))?;
            let db = serialize::from_serializable_text(text, &serialize::Options::default())?;
            (db_line(&db)?, Value::Db(db))
        }
        "read-db" => {
            let s = arg("string")?;
            let v = edn::read_string(s.as_str().ok_or_else(|| Error::msg("read-db takes a string"))?)?;
            let db = db_arg(&v)?;
            (db_line(&db)?, Value::Db(db))
        }

        "conn-quiet" => {
            // transactions through a connection, each answering ok or its error; then the database, unprinted
            let conn = if has("db") {
                datascript::conn::Conn::from_db(db_arg(&arg("db")?)?)
            } else {
                datascript::conn::Conn::create(arg("schema")?)?
            };
            let mut lines = Vec::new();
            for tx in seq_arg(&raw("txs"))? {
                lines.push(Value::from(match resolve(env, &tx).and_then(|tx| conn.transact(&tx, Value::Nil)) {
                    Ok(_) => "ok".to_string(),
                    Err(e) if e.data.is_nil() => "#error :native".into(),
                    Err(e) => error_line(&e),
                }));
            }
            (format!("#conn {}", p(&Value::vector(lines))), Value::Db(conn.db()))
        }
        "conn" => {
            // a connection's life: transactions in order, each listened to, then its database
            let conn = if has("db") {
                datascript::conn::Conn::from_db(db_arg(&arg("db")?)?)
            } else {
                datascript::conn::Conn::create(arg("schema")?)?
            };
            let reports: Arc<std::sync::Mutex<Vec<datascript::TxReport>>> = Arc::default();
            let seen = reports.clone();
            conn.listen(
                kwv("oracle"),
                Arc::new(move |r| {
                    seen.lock().unwrap().push(r.clone());
                    Ok(())
                }),
            );
            let mut lines = Vec::new();
            for tx in seq_arg(&raw("txs"))? {
                let run = || -> Result<String> {
                    if tx.is_map() && tx.get(&kwv("reset-conn")).is_some() {
                        let db = db_arg(&resolve(env, tx.get(&kwv("reset-conn")).unwrap())?)?;
                        conn.reset(db, tx.get(&kwv("tx-meta")).cloned().unwrap_or(Value::Nil))?;
                        Ok("reset".into())
                    } else if tx.is_map() && tx.get(&kwv("reset-schema")).is_some() {
                        conn.reset_schema(resolve(env, tx.get(&kwv("reset-schema")).unwrap())?)?;
                        Ok("schema".into())
                    } else if tx.is_map() && tx.get(&kwv("tx-data")).is_some() {
                        let data = resolve(env, tx.get(&kwv("tx-data")).unwrap())?;
                        report_line(&conn.transact(&data, tx.get(&kwv("tx-meta")).cloned().unwrap_or(Value::Nil))?)
                    } else {
                        report_line(&conn.transact(&resolve(env, &tx)?, Value::Nil)?)
                    }
                };
                lines.push(Value::from(match run() {
                    Ok(line) => line,
                    Err(e) if e.data.is_nil() => "#error :native".into(),
                    Err(e) => error_line(&e),
                }));
            }
            let seen: Vec<Value> = reports
                .lock()
                .unwrap()
                .iter()
                .map(|r| {
                    let mut m = CljMap::new();
                    m.assoc(kwv("tx-data"), datoms_vector(r.tx_data.clone()));
                    m.assoc(kwv("tempids"), r.tempids.clone());
                    m.assoc(kwv("tx-meta"), r.tx_meta.clone());
                    Value::map(m)
                })
                .collect();
            let db = conn.db();
            // and at the end every database the connection held on the way, the first of them first
            let mut history = Vec::new();
            for r in reports.lock().unwrap().iter() {
                history.push(Value::vector(vec![db_digest(&r.db_before)?, db_digest(&r.db_after)?]));
            }
            (
                format!(
                    "#conn {} {} {} {}",
                    p(&Value::vector(lines)),
                    p(&Value::vector(seen)),
                    db_line(&db)?,
                    p(&Value::vector(history))
                ),
                Value::Db(db),
            )
        }

        // --- the runtime underneath: ClojureScript's own hash, equality, order and printing
        "cljs/hash" => (p(&Value::from(arg("val")?.cljs_hash())), nothing),
        "cljs/pr-str" => (p(&Value::from(p(&arg("val")?))), nothing),
        "cljs/str" => (p(&Value::from(str_of(&arg("val")?))), nothing),
        "cljs/eq" => (p(&Value::Bool(arg("a")? == arg("b")?)), nothing),
        "cljs/compare" => {
            let c = datascript::cmp::value_compare_checked(&arg("a")?, &arg("b")?)?;
            (p(&Value::Num(datascript::cmp::ordering_num(c))), nothing)
        }
        "cljs/set" => (p(&clj::set(&arg("vals")?)?), nothing),
        "cljs/into-map" => (p(&Value::map(clj::into_map(&seq_arg(&arg("pairs")?)?)?)), nothing),
        "cljs/assoc" => (p(&clj::assoc(&arg("map")?, &seq_arg(&arg("kvs")?)?)?), nothing),
        "cljs/dissoc" => (p(&clj::dissoc(&arg("map")?, &seq_arg(&arg("ks")?)?)?), nothing),
        "cljs/conj" => {
            let mut coll = arg("coll")?;
            for x in seq_arg(&arg("xs")?)? {
                coll = clj::conj(&coll, x)?;
            }
            (p(&coll), nothing)
        }
        "cljs/disj" => (p(&clj::disj(&arg("coll")?, &seq_arg(&arg("xs")?)?)?), nothing),
        "cljs/sort" => {
            let mut items = seq_arg(&arg("vals")?)?;
            clj::sort_by(&mut items, datascript::cmp::value_compare_checked)?;
            let _ = value_compare;
            (p(&Value::vector(items)), nothing)
        }
        "cljs/read" => {
            let s = arg("string")?;
            (p(&edn::read_string(s.as_str().ok_or_else(|| Error::msg("read takes a string"))?)?), nothing)
        }
        "cljs/group-by-first" => {
            let items = seq_arg(&arg("vals")?)?;
            let grouped = clj::group_by(&items, |x| Ok(clj::seq(x)?.into_iter().next().unwrap_or(Value::Nil)))?;
            (p(&Value::map(grouped)), nothing)
        }
        "cljs/distinct" => (p(&Value::vector(clj::distinct(&seq_arg(&arg("vals")?)?))), nothing),
        "cljs/frequencies" => (p(&Value::map(clj::frequencies(&seq_arg(&arg("vals")?)?))), nothing),
        "cljs/zipmap" => (p(&Value::map(clj::zipmap(&seq_arg(&arg("ks")?)?, &seq_arg(&arg("vs")?)?))), nothing),
        "cljs/merge" => (p(&clj::merge(&seq_arg(&arg("maps")?)?)?), nothing),
        "cljs/select-keys" => (p(&clj::select_keys(&arg("map")?, &seq_arg(&arg("ks")?)?)), nothing),

        other => return Err(Error::msg(format!("oracle: unknown op :{other}"))),
    })
}

fn run_case(steps: &Value) -> Vec<String> {
    let mut env: Env = HashMap::new();
    let mut out = Vec::new();
    let trace = std::env::var_os("CONFORMANCE_TRACE").is_some();
    for (j, step) in steps.as_seq().unwrap_or(&[]).iter().enumerate() {
        if trace {
            eprintln!("  step {j}");
        }
        let (line, value) = match run_step(&env, step) {
            Ok(r) => r,
            Err(e) => (error_line(&e), Value::Nil),
        };
        if let Some(Value::Symbol(name)) = step.get(&kwv("as")) {
            env.insert(name.full().to_string(), value);
        }
        out.push(line);
    }
    out
}

pub fn run_file(input: &str) -> String {
    let mut out = String::new();
    let lines = input.lines().filter(|l| !l.trim().is_empty() && !l.starts_with(';'));
    let trace = std::env::var_os("CONFORMANCE_TRACE").is_some();
    for (i, line) in lines.enumerate() {
        if trace {
            eprintln!("case {i}");
        }
        out.push_str(&format!("== {i}\n"));
        match read_case(line) {
            Ok(case) => {
                for l in run_case(&case) {
                    out.push_str(&l);
                    out.push('\n');
                }
            }
            Err(e) => {
                out.push_str(&format!("#case-error {}\n", pr_str(&Value::str(&e.message))));
            }
        }
    }
    out
}
