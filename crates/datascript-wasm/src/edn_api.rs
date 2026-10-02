//! The module for a host that writes EDN: `ds_edn` takes one operation as text and answers its value as text.
//!
//! ```text
//! [:empty-db schema]                          → #datascript/handle 0
//! [:init-db [#datascript/Datom [...] ...] schema]
//! [:db-with db tx-data]                       → #datascript/handle 1
//! [:with db tx-data tx-meta]                  → {:db-after … :tx-data [...] :tempids {...}}
//! [:q query input ...]
//! [:pull db pattern eid]   [:pull-many db pattern eids]
//! [:datoms db index c0 ...]   [:seek-datoms …]   [:rseek-datoms …]   [:index-range db attr start end]
//! [:entid db eid]   [:schema db]   [:count db]
//! [:serializable db]   [:from-serializable "json"]
//! [:db-string db]   [:read-db "#datascript/DB {...}"]
//! [:release db]
//! ```
//!
//! A database is `#datascript/handle n` wherever it is an argument or an answer. An error answers
//! `{:message "…" :data {…}}`.

use crate::state::with_state;
use datascript::db;
use datascript::print::pr_str;
use datascript::serialize::{self, Json};
use datascript::value::{HostObj, HostObject};
use datascript::{clj, edn, CljMap, Db, Error, Index, Result, Value};
use std::any::Any;
use std::sync::Arc;

/// A database as the answer names it.
struct Handle(u32);

impl HostObject for Handle {
    fn hash(&self) -> i32 {
        self.0 as i32
    }
    fn equiv(&self, other: &dyn HostObject) -> bool {
        other.as_any().downcast_ref::<Handle>().is_some_and(|o| o.0 == self.0)
    }
    fn type_name(&self) -> String {
        "datascript/handle".into()
    }
    fn pr_str(&self) -> String {
        format!("#datascript/handle {}", self.0)
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// The value with each database in it as its handle.
fn with_handles(v: &Value) -> Value {
    match v {
        Value::Db(db) => Value::Host(HostObj::new(Handle(with_state(|state| state.db_handle(db))))),
        Value::Vector(items) => Value::vector(items.iter().map(with_handles).collect()),
        Value::List(items) => Value::list(items.iter().map(with_handles).collect()),
        Value::Map(m) => {
            let mut out = m.empty_like();
            for (k, x) in m.iter() {
                out.assoc(with_handles(k), with_handles(x));
            }
            Value::map(out)
        }
        Value::Set(s) => Value::set(s.iter().map(with_handles).collect()),
        other => other.clone(),
    }
}

fn db_of(v: &Value) -> Result<Db> {
    match v {
        Value::Db(db) => Ok(db.clone()),
        other => Err(Error::msg(format!("Expected a database, #datascript/handle n, got {}", pr_str(other)))),
    }
}

fn datoms_value(datoms: Vec<datascript::Datom>) -> Value {
    Value::vector(datoms.into_iter().map(|d| Value::Datom(Arc::new(d))).collect())
}

fn run(text: &str) -> Result<Value> {
    let form = edn::read_string_with(text, &|tag, form| {
        if tag != "datascript/handle" {
            return Ok(None);
        }
        let handle = form.as_num().filter(|n| *n >= 0.0 && n.fract() == 0.0).ok_or_else(|| Error::msg("A handle is a number"))?;
        with_state(|state| state.db(handle as u32)).map(|db| Some(Value::Db(db)))
    })?;
    let items = form.as_seq().ok_or_else(|| Error::msg("An operation is a vector: [:op arg ...]"))?;
    let op = match items.first() {
        Some(Value::Keyword(k)) => k.full().to_string(),
        _ => return Err(Error::msg("An operation is a vector: [:op arg ...]")),
    };
    static NIL: Value = Value::Nil;
    let arg = |i: usize| items.get(i).unwrap_or(&NIL);
    let rest = |from: usize| items.get(from..).unwrap_or(&[]).to_vec();
    let component = |i: usize| items.get(i).unwrap_or(&NIL);
    Ok(match op.as_str() {
        "empty-db" => Value::Db(Db::empty(arg(1).clone())?),
        "init-db" => {
            let mut datoms = Vec::new();
            for item in clj::seq(arg(1))? {
                match item {
                    Value::Datom(d) => datoms.push((*d).clone()),
                    other => datoms.push(datascript::datom::datom_from_reader(&other)?),
                }
            }
            Value::Db(Db::init(datoms, arg(2).clone())?)
        }
        "db-with" => Value::Db(datascript::db_with(&db_of(arg(1))?, arg(2))?),
        "with" => {
            let report = datascript::with(&db_of(arg(1))?, arg(2), arg(3).clone())?;
            let mut m = CljMap::new();
            m.assoc(Value::kw("db-after"), Value::Db(report.db_after));
            m.assoc(Value::kw("tx-data"), datoms_value(report.tx_data));
            m.assoc(Value::kw("tempids"), report.tempids);
            m.assoc(Value::kw("tx-meta"), report.tx_meta);
            Value::map(m)
        }
        "q" => datascript::q(arg(1), &rest(2))?,
        "pull" => datascript::pull(&db_of(arg(1))?, arg(2), arg(3), None)?,
        "pull-many" => Value::vector(datascript::pull_many(&db_of(arg(1))?, arg(2), &clj::seq(arg(3))?, None)?),
        "datoms" | "seek-datoms" | "rseek-datoms" => {
            let db = db_of(arg(1))?;
            let index = Index::from_value(arg(2)).ok_or_else(|| Error::msg(format!("Unknown index {}", pr_str(arg(2)))))?;
            let (c0, c1, c2, c3) = (component(3), component(4), component(5), component(6));
            let found = match op.as_str() {
                "datoms" => db::datoms(&db, index, c0, c1, c2, c3)?,
                "seek-datoms" => db::seek_datoms(&db, index, c0, c1, c2, c3)?,
                _ => db::rseek_datoms(&db, index, c0, c1, c2, c3)?,
            };
            datoms_value(found.to_vec()?)
        }
        "index-range" => datoms_value(db::index_range(&db_of(arg(1))?, arg(2), arg(3), arg(4))?.to_vec()?),
        "entid" => db_of(arg(1))?.entid_value(arg(2))?,
        "schema" => db_of(arg(1))?.schema_value(),
        "count" => Value::from(db_of(arg(1))?.count()?),
        "serializable" => Value::from(serialize::serializable(&db_of(arg(1))?, &serialize::Options::default())?.to_json_string()),
        "from-serializable" => {
            let text = arg(1).as_str().ok_or_else(|| Error::msg("from-serializable takes JSON text"))?;
            Value::Db(serialize::from_serializable(&Json::parse(text)?, &serialize::Options::default())?)
        }
        "db-string" => Value::from(pr_str(&Value::Db(db_of(arg(1))?))),
        "read-db" => {
            let text = arg(1).as_str().ok_or_else(|| Error::msg("read-db takes the text of a database"))?;
            Value::Db(db_of(&edn::read_string(text)?)?)
        }
        "release" => {
            let released = match arg(1) {
                Value::Db(db) => {
                    let handle = with_state(|state| state.db_handle(db));
                    with_state(|state| state.release_db(handle))
                }
                _ => None,
            };
            Value::Bool(released.is_some())
        }
        other => return Err(Error::msg(format!("No operation :{other}"))),
    })
}

/// One operation: its status, and the text of its answer.
pub fn call(text: &str) -> (u32, String) {
    match run(text) {
        Ok(v) => (0, pr_str(&with_handles(&v))),
        Err(e) => {
            let mut m = CljMap::new();
            m.assoc(Value::kw("message"), Value::str(&e.message));
            m.assoc(Value::kw("data"), with_handles(&e.data));
            (1, pr_str(&Value::map(m)))
        }
    }
}
