//! The operations of `ds_call`: each takes a vector of arguments and answers a value.

use crate::host;
use crate::state::with_state;
use datascript::built_ins;
use datascript::datom::{id_from_num, value_attr};
use datascript::db::{self, Datoms};
use datascript::serialize::{self, Json};
use datascript::{vector, Datom, Db, Error, Index, Result, Value};
use std::sync::Arc;

/// `[schema]` → db
pub const EMPTY_DB: u32 = 1;
/// `[datoms schema]` → db
pub const INIT_DB: u32 = 2;
/// `[db tx-data tx-meta]` → `[db-after tx-data tempids]`
pub const WITH: u32 = 3;
/// `[db schema]` → db
pub const WITH_SCHEMA: u32 = 4;
/// `[db pred]` → db: `(pred unfiltered-db datom)`
pub const FILTER: u32 = 5;
/// `[db]` → db
pub const UNFILTERED: u32 = 6;
/// `[query inputs]` → the query's result
pub const Q: u32 = 7;
/// `[db pattern eid visitor]` → map or nil
pub const PULL: u32 = 8;
/// `[db pattern eids visitor]` → vector
pub const PULL_MANY: u32 = 9;
/// `[db index c0 c1 c2 c3 n reverse?]` → `[datoms cursor]`: the first `n` datoms, and a cursor if there may be more
pub const DATOMS: u32 = 10;
pub const SEEK_DATOMS: u32 = 11;
pub const RSEEK_DATOMS: u32 = 12;
/// `[db attr start end n reverse?]` → `[datoms cursor]`
pub const INDEX_RANGE: u32 = 13;
/// `[db e a v tx n reverse?]` → `[datoms cursor]`: `-search`, each component `nil` when open
pub const SEARCH: u32 = 14;
/// `[cursor n]` → `[datoms cursor]`: the cursor answered is the one to ask next, `nil` at the end
pub const CURSOR_NEXT: u32 = 15;
/// `[cursor]`
pub const CURSOR_FREE: u32 = 16;
/// `[db index c0 c1 c2 c3]` → datom or nil
pub const FIND_DATOM: u32 = 17;
/// `[db eid]` → the entity's id, or nil
pub const ENTID: u32 = 18;
/// `[db]` → `[schema-uid max-eid max-tx filtered?]`
pub const DB_INFO: u32 = 19;
/// `[db]` → `[schema rschema]`
pub const SCHEMA: u32 = 20;
/// `[db]` → the number of its datoms
pub const DB_COUNT: u32 = 21;
/// `[a b]` → whether the two are equal databases
pub const DB_EQUIV: u32 = 22;
/// `[db]` → its hash
pub const DB_HASH: u32 = 23;
/// `[db]` → a database of the same schema and no datoms
pub const DB_EMPTY: u32 = 24;
/// `[db freeze-fn freeze-kw]` → JSON text
pub const SERIALIZABLE: u32 = 25;
/// `[json-text thaw-fn thaw-kw]` → db
pub const FROM_SERIALIZABLE: u32 = 26;
/// `[a b]` → `[only-a only-b both]`
pub const DIFF: u32 = 27;
/// `[handle]`: the host lets go of a database
pub const RELEASE_DB: u32 = 28;
/// `[handle]`: the host lets go of a function of the module's
pub const RELEASE_FN: u32 = 29;
/// `[f args]` → `(apply f args)`
pub const CALL_FN: u32 = 30;
/// `[]` → `[databases cursors functions]` held for the host
pub const HELD: u32 = 31;
/// `[value]` → as ClojureScript prints it
pub const PR_STR: u32 = 32;
/// `[text]` → the value EDN reads, DataScript's tags among it
pub const READ_STRING: u32 = 33;
/// `[value]` → ClojureScript's hash of it
pub const HASH: u32 = 34;
/// `[name value]`: `"host-regex"` true, and the host matches regular expressions
pub const SET_OPTION: u32 = 35;
/// `[db]` → `[eavt-count aevt-count avet-count]`
pub const INDEX_COUNTS: u32 = 36;

static NIL: Value = Value::Nil;

fn arg(args: &[Value], i: usize) -> &Value {
    args.get(i).unwrap_or(&NIL)
}

fn db_arg(args: &[Value], i: usize) -> Result<Db> {
    match arg(args, i) {
        Value::Db(db) => Ok(db.clone()),
        _ => Err(Error::msg("Assert failed: (db/db? db)")),
    }
}

fn index_arg(v: &Value) -> Result<Index> {
    Index::from_value(v).ok_or_else(|| Error::msg(format!("Unknown index {}", datascript::print::pr_str(v))))
}

fn handle_arg(v: &Value) -> Result<u32> {
    v.as_num().filter(|n| *n >= 0.0 && n.fract() == 0.0).map(|n| n as u32).ok_or_else(|| Error::msg("datascript: a handle is a number"))
}

fn count_arg(v: &Value) -> usize {
    v.as_num().filter(|n| *n >= 1.0).map_or(usize::MAX, |n| n as usize)
}

fn datoms_value(datoms: Vec<Datom>) -> Value {
    Value::vector(datoms.into_iter().map(|d| Value::Datom(Arc::new(d))).collect())
}

/// The first `n` of a run of datoms, and a cursor to read on from when there may be more.
fn first_chunk(datoms: Datoms, n: usize, reverse: bool) -> Result<Value> {
    let datoms = if reverse { datoms.reversed() } else { datoms };
    let mut cursor = datoms.cursor();
    let chunk = datoms.next_chunk(&mut cursor, n)?;
    let more = if cursor.is_done() || chunk.len() < n {
        Value::Nil
    } else {
        Value::from(with_state(|state| state.insert_cursor((datoms, cursor))) as usize)
    };
    Ok(vector![datoms_value(chunk), more])
}

/// A function of the host's that a database's filter asks of every datom.
fn filter(db: &Db, pred: &Value) -> Db {
    let pred = pred.clone();
    db.filter(Arc::new(move |db, datom| {
        Ok(built_ins::call(&pred, &[Value::Db(db.clone()), Value::Datom(Arc::new(datom.clone()))])?.truthy())
    }))
}

fn serialize_options<'a>(
    freeze_fn: &'a Option<Box<dyn Fn(&Value) -> Result<Json> + 'a>>,
    freeze_kw: &'a Option<Box<dyn Fn(&Value) -> Result<Json> + 'a>>,
    thaw_fn: &'a Option<Box<dyn Fn(&Json) -> Result<Value> + 'a>>,
    thaw_kw: &'a Option<Box<dyn Fn(&Json) -> Result<Value> + 'a>>,
) -> serialize::Options<'a> {
    serialize::Options {
        freeze_fn: freeze_fn.as_deref(),
        freeze_kw: freeze_kw.as_deref(),
        thaw_fn: thaw_fn.as_deref(),
        thaw_kw: thaw_kw.as_deref(),
    }
}

/// A function of the host's as one of `serializable`'s: what it answers, as JavaScript's data.
fn freezer<'a>(f: &'a Value) -> Option<Box<dyn Fn(&Value) -> Result<Json> + 'a>> {
    if f.is_nil() {
        return None;
    }
    Some(Box::new(move |v| Ok(Json::from_value(&built_ins::call(f, &[v.clone()])?))))
}

fn thawer<'a>(f: &'a Value) -> Option<Box<dyn Fn(&Json) -> Result<Value> + 'a>> {
    if f.is_nil() {
        return None;
    }
    Some(Box::new(move |j| built_ins::call(f, &[j.to_value()])))
}

pub fn dispatch(op: u32, args: &[Value]) -> Result<Value> {
    Ok(match op {
        EMPTY_DB => Value::Db(Db::empty(arg(args, 0).clone())?),
        INIT_DB => {
            let mut datoms = Vec::new();
            for item in datascript::clj::seq(arg(args, 0))? {
                match item {
                    Value::Datom(d) => datoms.push((*d).clone()),
                    other => {
                        let ty = built_ins::call(&built_ins::query_fn(&datascript::Symbol::parse("type")).expect("type"), &[other])?;
                        return Err(Error::new(
                            format!("init-db expects list of Datoms, got {}", datascript::print::pr_str(&ty)),
                            Value::kw_map(&[("error", Value::kw("init-db"))]),
                        ));
                    }
                }
            }
            Value::Db(Db::init(datoms, arg(args, 1).clone())?)
        }
        WITH => {
            let report = datascript::with(&db_arg(args, 0)?, arg(args, 1), arg(args, 2).clone())?;
            vector![Value::Db(report.db_after), datoms_value(report.tx_data), report.tempids]
        }
        WITH_SCHEMA => Value::Db(db_arg(args, 0)?.with_schema(arg(args, 1).clone())?),
        FILTER => Value::Db(filter(&db_arg(args, 0)?, arg(args, 1))),
        UNFILTERED => Value::Db(db_arg(args, 0)?.unfiltered()),
        Q => datascript::q(arg(args, 0), &datascript::clj::seq(arg(args, 1))?)?,
        PULL => {
            let visitor = Some(arg(args, 3)).filter(|v| v.is_some());
            datascript::pull(&db_arg(args, 0)?, arg(args, 1), arg(args, 2), visitor)?
        }
        PULL_MANY => {
            let visitor = Some(arg(args, 3)).filter(|v| v.is_some());
            let ids = datascript::clj::seq(arg(args, 2))?;
            Value::vector(datascript::pull_many(&db_arg(args, 0)?, arg(args, 1), &ids, visitor)?)
        }
        DATOMS | SEEK_DATOMS | RSEEK_DATOMS => {
            let db = db_arg(args, 0)?;
            let index = index_arg(arg(args, 1))?;
            let (c0, c1, c2, c3) = (arg(args, 2), arg(args, 3), arg(args, 4), arg(args, 5));
            let found = match op {
                DATOMS => db::datoms(&db, index, c0, c1, c2, c3)?,
                SEEK_DATOMS => db::seek_datoms(&db, index, c0, c1, c2, c3)?,
                _ => db::rseek_datoms(&db, index, c0, c1, c2, c3)?,
            };
            first_chunk(found, count_arg(arg(args, 6)), arg(args, 7).truthy())?
        }
        INDEX_RANGE => {
            let found = db::index_range(&db_arg(args, 0)?, arg(args, 1), arg(args, 2), arg(args, 3))?;
            first_chunk(found, count_arg(arg(args, 4)), arg(args, 5).truthy())?
        }
        SEARCH => {
            let db = db_arg(args, 0)?;
            // as `-search` reads its pattern: what is nil or false is left open
            let open = |v: &Value| !v.truthy();
            let id = |v: &Value| -> Result<Option<i32>> {
                match v.as_num() {
                    _ if open(v) => Ok(None),
                    Some(n) => id_from_num(n).map(Some),
                    None => Err(Error::msg(format!("Cannot compare {} to an id", datascript::print::str_of(v)))),
                }
            };
            let (e, tx) = match (id(arg(args, 1)), id(arg(args, 4))) {
                (Ok(e), Ok(tx)) => (e, tx),
                // no datom has an id that is a fraction
                (Err(err), _) | (_, Err(err)) => {
                    return if err.data.is_nil() && arg(args, 1).as_num().or(arg(args, 4).as_num()).is_some() {
                        Ok(vector![Value::vector(Vec::new()), Value::Nil])
                    } else {
                        Err(err)
                    };
                }
            };
            let a = arg(args, 2);
            let attr = if open(a) {
                None
            } else {
                Some(value_attr(a).ok_or_else(|| Error::msg(format!("Cannot compare {} to an attribute", datascript::print::str_of(a))))?)
            };
            let v = Some(arg(args, 3)).filter(|v| v.is_some());
            let found = db::search(&db, e, attr.as_ref(), v, tx);
            first_chunk(found, count_arg(arg(args, 5)), arg(args, 6).truthy())?
        }
        CURSOR_NEXT => {
            let handle = handle_arg(arg(args, 0))?;
            let (datoms, mut cursor) = with_state(|state| state.take_cursor(handle))
                .ok_or_else(|| Error::msg(format!("datascript: no cursor of handle {handle}")))?;
            let n = count_arg(arg(args, 1));
            let chunk = datoms.next_chunk(&mut cursor, n)?;
            let more = if cursor.is_done() || chunk.len() < n {
                Value::Nil
            } else {
                Value::from(with_state(|state| state.insert_cursor((datoms, cursor))) as usize)
            };
            vector![datoms_value(chunk), more]
        }
        CURSOR_FREE => {
            let freed = with_state(|state| state.take_cursor(handle_arg(arg(args, 0)).unwrap_or(0)));
            Value::Bool(freed.is_some())
        }
        FIND_DATOM => {
            let found = db::find_datom(&db_arg(args, 0)?, index_arg(arg(args, 1))?, arg(args, 2), arg(args, 3), arg(args, 4), arg(args, 5))?;
            found.map_or(Value::Nil, |d| Value::Datom(Arc::new(d)))
        }
        ENTID => db_arg(args, 0)?.entid_value(arg(args, 1))?,
        DB_INFO => {
            let db = db_arg(args, 0)?;
            vector![db.schema().uid() as usize, db.max_eid(), db.max_tx(), db.is_filtered()]
        }
        SCHEMA => {
            let db = db_arg(args, 0)?;
            vector![db.schema_value(), db.rschema_value()]
        }
        DB_COUNT => Value::from(db_arg(args, 0)?.count()?),
        DB_EQUIV => Value::Bool(match (arg(args, 0), arg(args, 1)) {
            (Value::Db(a), Value::Db(b)) => a.equiv(b),
            _ => false,
        }),
        DB_HASH => Value::from(db_arg(args, 0)?.cljs_hash()),
        DB_EMPTY => Value::Db(db_arg(args, 0)?.empty_like()?),
        SERIALIZABLE => {
            let (freeze_fn, freeze_kw) = (freezer(arg(args, 1)), freezer(arg(args, 2)));
            let json = serialize::serializable(&db_arg(args, 0)?, &serialize_options(&freeze_fn, &freeze_kw, &None, &None))?;
            Value::from(json.to_json_string())
        }
        FROM_SERIALIZABLE => {
            let text = arg(args, 0).as_str().ok_or_else(|| Error::msg("datascript: from-serializable takes JSON text"))?;
            let (thaw_fn, thaw_kw) = (thawer(arg(args, 1)), thawer(arg(args, 2)));
            let db = serialize::from_serializable(&Json::parse(text)?, &serialize_options(&None, &None, &thaw_fn, &thaw_kw))?;
            Value::Db(db)
        }
        DIFF => db::diff(&db_arg(args, 0)?, &db_arg(args, 1)?)?,
        RELEASE_DB => {
            // dropped here, outside the state's borrow
            let db = with_state(|state| state.release_db(handle_arg(arg(args, 0)).unwrap_or(u32::MAX)));
            Value::Bool(db.is_some())
        }
        RELEASE_FN => {
            let f = with_state(|state| state.release_module_fn(handle_arg(arg(args, 0)).unwrap_or(u32::MAX)));
            Value::Bool(f.is_some())
        }
        CALL_FN => built_ins::call(arg(args, 0), &datascript::clj::seq(arg(args, 1))?)?,
        HELD => {
            let (dbs, cursors, fns) = with_state(|state| state.held());
            vector![dbs, cursors, fns]
        }
        PR_STR => Value::from(datascript::print::pr_str(arg(args, 0))),
        READ_STRING => {
            let text = arg(args, 0).as_str().ok_or_else(|| Error::msg("datascript: read-string takes a string"))?;
            datascript::edn::read_string(text)?
        }
        HASH => Value::from(arg(args, 0).cljs_hash()),
        SET_OPTION => {
            match arg(args, 0).as_str() {
                Some("host-regex") => host::use_host_regex(arg(args, 1).truthy()),
                other => return Err(Error::msg(format!("datascript: no option {other:?}"))),
            }
            Value::Nil
        }
        INDEX_COUNTS => {
            let db = db_arg(args, 0)?;
            vector![db.index(Index::Eavt).count()?, db.index(Index::Aevt).count()?, db.index(Index::Avet).count()?]
        }
        other => return Err(Error::msg(format!("datascript: no operation {other}"))),
    })
}
