//! Connections: a mutable reference to the latest database value, with listeners told of every transaction.

use crate::coll::CljMap;
use crate::datom::Datom;
use crate::db::Db;
use crate::error::Result;
use crate::transact::{advance, TxReport};
use crate::value::Value;
use std::sync::{Arc, Mutex};

/// A listener: called with the report of each transaction.
pub type Listener = Arc<dyn Fn(&TxReport) -> Result<()> + Send + Sync>;

struct Inner {
    db: Db,
    /// The listeners by key, in the order of ClojureScript's map of them
    order: CljMap,
    listeners: Vec<Option<Listener>>,
}

/// `datascript.conn/Conn`. Cloning it gives the same connection.
#[derive(Clone)]
pub struct Conn(Arc<Mutex<Inner>>);

impl Conn {
    /// `(conn-from-db db)`
    pub fn from_db(db: Db) -> Conn {
        Conn(Arc::new(Mutex::new(Inner { db, order: CljMap::new(), listeners: Vec::new() })))
    }

    /// `(create-conn schema)`
    pub fn create(schema: Value) -> Result<Conn> {
        Ok(Conn::from_db(Db::empty(schema)?))
    }

    /// `(conn-from-datoms datoms schema)`
    pub fn from_datoms(datoms: Vec<Datom>, schema: Value) -> Result<Conn> {
        Ok(Conn::from_db(Db::init(datoms, schema)?))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// `@conn`: the database as it is now.
    pub fn db(&self) -> Db {
        self.lock().db.clone()
    }

    fn listeners(&self) -> Vec<Listener> {
        let inner = self.lock();
        inner.order.vals().filter_map(|slot| inner.listeners.get(slot.as_num()? as usize)?.clone()).collect()
    }

    /// `(transact! conn tx-data tx-meta)`: the transaction applied to the connection's database, the connection
    /// moved on to the result, and every listener told.
    pub fn transact(&self, tx_data: &Value, tx_meta: Value) -> Result<TxReport> {
        let report = {
            let mut inner = self.lock();
            // the database the connection moves on from keeps its indexes as the new one's, undone
            let report = advance(&inner.db, tx_data, tx_meta)?;
            inner.db = report.db_after.clone();
            report
        };
        // outside the lock: a listener may read the connection, or transact on it
        for listener in self.listeners() {
            listener(&report)?;
        }
        Ok(report)
    }

    /// `(reset-conn! conn db tx-meta)`: the connection now holds `db`; listeners are told of everything the old
    /// database held as retracted, and everything the new one holds as added.
    pub fn reset(&self, db: Db, tx_meta: Value) -> Result<Db> {
        let before = {
            let mut inner = self.lock();
            std::mem::replace(&mut inner.db, db.clone())
        };
        let listeners = self.listeners();
        if !listeners.is_empty() {
            let mut tx_data: Vec<Datom> = before
                .all()
                .to_vec()?
                .into_iter()
                .map(|d| Datom::with_added(d.e, d.a, d.v.clone(), d.tx(), false))
                .collect();
            tx_data.extend(db.all().to_vec()?);
            let report = TxReport {
                db_before: before,
                db_after: db.clone(),
                tx_data,
                tempids: Value::Nil,
                tx_meta,
                changes: Arc::from([]),
            };
            for listener in listeners {
                listener(&report)?;
            }
        }
        Ok(db)
    }

    /// `(reset-schema! conn schema)`
    pub fn reset_schema(&self, schema: Value) -> Result<Db> {
        let mut inner = self.lock();
        let db = inner.db.with_schema(schema)?;
        inner.db = db.clone();
        Ok(db)
    }

    /// `(listen! conn key callback)`: a listener under a key, replacing the one that key had.
    pub fn listen(&self, key: Value, callback: Listener) -> Value {
        let mut inner = self.lock();
        match inner.order.get(&key).and_then(Value::as_num) {
            Some(slot) => inner.listeners[slot as usize] = Some(callback),
            None => {
                let slot = inner.listeners.len();
                inner.listeners.push(Some(callback));
                inner.order.assoc(key.clone(), Value::from(slot));
            }
        }
        key
    }

    /// `(unlisten! conn key)`
    pub fn unlisten(&self, key: &Value) {
        let mut inner = self.lock();
        if let Some(slot) = inner.order.dissoc(key).and_then(|s| s.as_num()) {
            inner.listeners[slot as usize] = None;
        }
    }

    /// Whether both are one connection.
    pub fn ptr_eq(&self, other: &Conn) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
