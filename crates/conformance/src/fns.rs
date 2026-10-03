//! The functions a case names with `#f`: predicates, query functions, aggregates, transaction functions and
//! filters, written here as conformance/oracle/src/oracle/core.cljs writes them.

use datascript::built_ins::{call, js_number, query_fn};
use datascript::cmp::compare;
use datascript::value::Func;
use datascript::{clj, vector, Db, Error, Index, Result, Symbol, Value};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

fn built_in(name: &str) -> Value {
    query_fn(&Symbol::parse(name)).unwrap_or_else(|| panic!("no built-in {name}"))
}

fn arg(args: &[Value], i: usize) -> Value {
    args.get(i).cloned().unwrap_or(Value::Nil)
}

fn db_arg(args: &[Value]) -> Result<Db> {
    match args.first() {
        Some(Value::Db(db)) => Ok(db.clone()),
        _ => Err(Error::msg("expected a database")),
    }
}

/// `(:e datom)`, `(:a datom)`, …
fn field(datom: &Value, k: &str) -> Value {
    clj::get(datom, &Value::kw(k)).unwrap_or(Value::Nil)
}

/// `(attr (d/entity db e))`
fn entity_attr(db: &Db, e: &Value, attr: &str) -> Result<Value> {
    Ok(match datascript::entity(db, e)? {
        Some(e) => e.lookup(&Value::kw(attr))?.unwrap_or(Value::Nil),
        None => Value::Nil,
    })
}

fn thrown(what: &str) -> Error {
    Error::new(what, Value::kw_map(&[("error", Value::kw("host/thrown"))]))
}

fn f<F>(name: &str, body: F) -> Option<Value>
where
    F: Fn(&[Value]) -> Result<Value> + Send + Sync + 'static,
{
    Some(Value::Fn(Func::new(name, body)))
}

/// The function of a name: one function a name, as the oracle's map of them holds one each.
pub fn lookup(name: &str) -> Option<Value> {
    static FNS: OnceLock<Mutex<HashMap<String, Value>>> = OnceLock::new();
    let mut fns = FNS.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    if let Some(found) = fns.get(name) {
        return Some(found.clone());
    }
    let made = make(name)?;
    fns.insert(name.to_string(), made.clone());
    Some(made)
}

fn make(name: &str) -> Option<Value> {
    match name {
        "even?" | "odd?" | "pos?" => Some(built_in(name)),
        "adult?" => f(name, |a| Ok(Value::Bool(js_number(&arg(a, 0)) >= 18.0))),
        "str-len" => Some(built_in("count")),
        "starts-with?" => Some(built_in("clojure.string/starts-with?")),
        "plus" => f(name, |a| {
            let mut args = vec![Value::from(0)];
            args.extend(a.iter().cloned());
            call(&built_in("+"), &args)
        }),
        "pair" => f(name, |a| Ok(vector![arg(a, 0), arg(a, 1)])),
        "triples" => f(name, |a| {
            let x = arg(a, 0);
            Ok(vector![vector![x.clone(), 1], vector![x.clone(), 2], vector![x, 3]])
        }),
        "nil-fn" => f(name, |_| Ok(Value::Nil)),
        "always" => f(name, |_| Ok(Value::Bool(true))),
        "never" => f(name, |_| Ok(Value::Bool(false))),
        "throw" => f(name, |_| Err(thrown("thrown by host fn"))),
        "minmax" => f(name, |a| {
            let xs = clj::seq(&arg(a, 0))?;
            Ok(vector![call(&built_in("min"), &xs)?, call(&built_in("max"), &xs)?])
        }),
        "range" => Some(built_in("range")),
        "five" => f(name, |_| Ok(Value::from(5))),
        "when-even" => f(name, |a| {
            let x = arg(a, 0);
            Ok(if call(&built_in("even?"), std::slice::from_ref(&x))?.truthy() { x } else { Value::Nil })
        }),
        "first" => f(name, |a| Ok(clj::seq(&arg(a, 0))?.into_iter().next().unwrap_or(Value::Nil))),
        "second" => f(name, |a| Ok(clj::seq(&arg(a, 0))?.into_iter().nth(1).unwrap_or(Value::Nil))),
        "gt18" => f(name, |a| Ok(Value::Bool(js_number(&arg(a, 0)) > 18.0))),
        "age-is" => f(name, |a| {
            let age = match datascript::entity(&db_arg(a)?, &arg(a, 1))? {
                Some(e) => e.lookup(&Value::kw("age"))?.unwrap_or(Value::Nil),
                None => Value::Nil,
            };
            Ok(Value::Bool(arg(a, 2) == age))
        }),
        "sort-reverse" => f(name, |a| {
            let mut items = clj::seq(&arg(a, 0))?;
            clj::sort_by(&mut items, compare)?;
            items.reverse();
            Ok(Value::list(items))
        }),
        "false-fn" => f(name, |_| Ok(Value::Bool(false))),
        "kv" => f(name, |a| Ok(Value::vector(clj::seq(&arg(a, 0))?))),
        "throw-odd" => f(name, |a| {
            let x = arg(a, 0);
            if call(&built_in("odd?"), std::slice::from_ref(&x))?.truthy() {
                Err(Error::new("odd", Value::kw_map(&[("error", Value::kw("host/odd"))])))
            } else {
                Ok(x)
            }
        }),

        // aggregates
        "agg-count" => Some(built_in("count")),
        "agg-first" => f(name, |a| Ok(clj::seq(&arg(a, 0))?.into_iter().next().unwrap_or(Value::Nil))),
        "agg-sorted" => f(name, |a| {
            let mut items = clj::seq(&arg(a, 0))?;
            clj::sort_by(&mut items, compare)?;
            Ok(Value::vector(items))
        }),
        "agg-nth" => f(name, |a| {
            let items = clj::seq(&arg(a, 1))?;
            let n = js_number(&arg(a, 0));
            Ok(if n >= 0.0 { items.get(n as usize).cloned().unwrap_or(Value::Nil) } else { Value::Nil })
        }),

        // transaction functions: (f db & args) -> tx-data
        "tx-inc" => f(name, |a| {
            let db = db_arg(a)?;
            let (e, attr, by) = (arg(a, 1), arg(a, 2), arg(a, 3));
            let v = db.datoms(Index::Eavt, &[e.clone(), attr.clone()])?.first()?.map_or(Value::Nil, |d| d.v);
            let v = if v.truthy() { v } else { Value::from(0) };
            Ok(vector![vector![Value::kw("db/add"), e, attr, call(&built_in("+"), &[v, by])?]])
        }),
        "tx-add" => f(name, |a| Ok(vector![vector![Value::kw("db/add"), arg(a, 1), arg(a, 2), arg(a, 3)]])),
        "tx-nothing" => f(name, |_| Ok(Value::vector(Vec::new()))),
        "tx-nil" => f(name, |_| Ok(Value::Nil)),
        "tx-entity" => f(name, |a| Ok(vector![arg(a, 1)])),
        "tx-count" => f(name, |a| {
            let n = db_arg(a)?.datoms(Index::Eavt, &[])?.count()?;
            Ok(vector![vector![Value::kw("db/add"), arg(a, 1), arg(a, 2), n]])
        }),
        "tx-throw" => f(name, |_| Err(thrown("thrown by tx fn"))),
        "tx-inc-age" => f(name, |a| {
            let who = arg(a, 1);
            let Some(ent) = datascript::entity(&db_arg(a)?, &vector![Value::kw("name"), who.clone()])? else {
                return Err(Error::new(
                    format!("No entity with name: {}", datascript::print::str_of(&who)),
                    Value::map(datascript::CljMap::new()),
                ));
            };
            let age = ent.lookup(&Value::kw("age"))?.unwrap_or(Value::Nil);
            let id = Value::from(ent.eid());
            Ok(vector![
                Value::kw_map(&[("db/id", id.clone()), ("age", call(&built_in("inc"), &[age])?)]),
                vector![Value::kw("db/add"), id, Value::kw("had-birthday"), true]
            ])
        }),
        "tx-oleg" => f(name, |_| Ok(vector![Value::kw_map(&[("name", Value::str("Oleg"))])])),
        "tx-vera" => {
            f(name, |_| Ok(vector![Value::kw_map(&[("db/id", Value::from(-1)), ("name", Value::str("Vera"))])]))
        }
        "tx-nested" => f(name, |a| {
            let inner = Value::Fn(Func::new("nested", |a| {
                let n = db_arg(a)?.count()?;
                Ok(vector![vector![Value::kw("db/add"), arg(a, 1), Value::kw("nested"), n]])
            }));
            Ok(vector![vector![Value::kw("db.fn/call"), inner, arg(a, 1)]])
        }),
        "tx-q" => f(name, |a| {
            let query = datascript::edn::read_string("[:find ?e :where [?e :name]]")?;
            let found = datascript::q(&query, &[arg(a, 0)])?;
            let mut out = Vec::new();
            for tuple in clj::seq(&found)? {
                let e = clj::seq(&tuple)?.into_iter().next().unwrap_or(Value::Nil);
                out.push(vector![Value::kw("db/add"), e, Value::kw("seen"), true]);
            }
            Ok(Value::list(out))
        }),

        // filter predicates: (pred db datom)
        "f-even-e" => f(name, |a| call(&built_in("even?"), &[field(&arg(a, 1), "e")])),
        "f-not-name" => f(name, |a| Ok(Value::Bool(field(&arg(a, 1), "a") != Value::kw("name")))),
        "f-num-v" => f(name, |a| Ok(Value::Bool(field(&arg(a, 1), "v").is_number()))),
        "f-tx-odd" => f(name, |a| call(&built_in("odd?"), &[field(&arg(a, 1), "tx")])),
        "f-has-name" => f(name, |a| {
            let found = match datascript::entity(&db_arg(a)?, &field(&arg(a, 1), "e"))? {
                Some(e) => e.lookup(&Value::kw("name"))?,
                None => None,
            };
            Ok(Value::Bool(found.is_some()))
        }),
        "f-none" => f(name, |_| Ok(Value::Bool(false))),
        "f-all" => f(name, |_| Ok(Value::Bool(true))),
        "f-not-password" => f(name, |a| Ok(Value::Bool(field(&arg(a, 1), "a") != Value::kw("password")))),
        "f-not-e2" => f(name, |a| Ok(Value::Bool(field(&arg(a, 1), "e") != Value::from(2)))),
        "f-long-akas" => f(name, |a| {
            let datom = arg(a, 1);
            if field(&datom, "a") != Value::kw("aka") {
                return Ok(Value::Bool(true));
            }
            let akas = entity_attr(&db_arg(a)?, &field(&datom, "e"), "aka")?;
            let count = |v: Value| -> Result<f64> { Ok(js_number(&call(&built_in("count"), &[v])?)) };
            Ok(Value::Bool(count(akas)? <= 1.0 || count(field(&datom, "v"))? >= 4.0))
        }),
        "f-has-age" => {
            f(name, |a| Ok(Value::Bool(entity_attr(&db_arg(a)?, &field(&arg(a, 1), "e"), "age")?.is_some())))
        }
        "f-adult" => {
            f(name, |a| Ok(Value::Bool(js_number(&entity_attr(&db_arg(a)?, &field(&arg(a, 1), "e"), "age")?) >= 18.0)))
        }
        "f-not-tupen" => f(name, |a| Ok(Value::Bool(field(&arg(a, 1), "v") != Value::str("Tupen")))),
        "f-throw" => f(name, |_| Err(thrown("thrown by filter"))),
        "get-name" => f(name, |a| Ok(clj::get(&arg(a, 0), &Value::kw("name")).unwrap_or(Value::Nil))),
        "names" => f(name, |a| {
            Ok(Value::vector(
                clj::seq(&arg(a, 0))?.iter().map(|x| clj::get(x, &Value::kw("name")).unwrap_or(Value::Nil)).collect(),
            ))
        }),

        // pull xforms
        "x-vector" => Some(built_in("vector")),
        "x-str" => Some(built_in("str")),
        "x-count" => f(name, |a| {
            let x = arg(a, 0);
            Ok(if x.is_coll() { Value::from(x.count().unwrap_or(0)) } else { x })
        }),
        _ => None,
    }
}
