//! The steps of a case, as conformance/oracle/src/oracle/core.cljs runs them, printed as it prints them.

use crate::fns;
use datascript::cmp::value_compare;
use datascript::coll::CljMap;
use datascript::datom::datom_from_reader;
use datascript::print::{pr_str, str_of};
use datascript::value::{HostObj, HostObject};
use datascript::{clj, edn, Datom, Db, Error, Index, Result, Value};
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
            let v = Value::from(db_arg(&arg("db")?)?.entid(&arg("eid")?)?);
            (p(&v), v)
        }
        "schema" => (p(&db_arg(&arg("db")?)?.schema_value()), nothing),
        "rschema" => match arg("db")? {
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
        "read-db" => {
            let s = arg("string")?;
            let v = edn::read_string(s.as_str().ok_or_else(|| Error::msg("read-db takes a string"))?)?;
            let db = db_arg(&v)?;
            (db_line(&db)?, Value::Db(db))
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
            (format!("#conn {} {} {}", p(&Value::vector(lines)), p(&Value::vector(seen)), db_line(&db)?), Value::Db(db))
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
            clj::sort_by(&mut items, |a, b| datascript::cmp::value_compare_checked(a, b))?;
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
    for step in steps.as_seq().unwrap_or(&[]) {
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
    for (i, line) in lines.enumerate() {
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
