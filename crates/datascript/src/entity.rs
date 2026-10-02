//! `datascript.impl.entity`: an entity is a lazy, map-like view of one entity id in one database value. Attributes
//! are looked up when asked for and kept; references are entities in turn.

use crate::coll::{CljMap, CljSet};
use crate::datom::{value_attr, Datom};
use crate::db::{entid, numeric_eid_exists, props, search, Db};
use crate::error::{Error, Result};
use crate::hash::{hash_combine, hash_number};
use crate::named::Attr;
use crate::schema::kw;
use crate::transact::{is_reverse_ref, reverse_ref};
use crate::value::{HostObj, HostObject, Value};
use std::any::Any;
use std::sync::{Arc, Mutex};

/// How deep components are touched within components before the touch is given up as endless.
const MAX_TOUCH_DEPTH: usize = 1000;

#[derive(Clone)]
pub struct Entity(Arc<Inner>);

struct Inner {
    db: Db,
    eid: i32,
    state: Mutex<State>,
}

struct State {
    touched: bool,
    cache: CljMap,
}

/// ClojureScript hashes an entity with its database's identity (`goog.getUid`): a number that is the database
/// value's alone. Here it is drawn from where the value lives.
fn db_uid(db: &Db) -> i32 {
    (db.identity() as u64 % 2_147_483_647) as i32
}

/// `(entity db eid)`: the entity of an id, a lookup ref or an ident, if it has any datoms.
pub fn entity(db: &Db, eid: &Value) -> Result<Option<Entity>> {
    let e = match eid {
        Value::Num(_) | Value::Vector(_) | Value::List(_) | Value::Keyword(_) => {
            entid(db, eid).or_else(Error::or_nothing)?
        }
        _ => None,
    };
    match e {
        Some(e) if numeric_eid_exists(db, e)? => Ok(Some(Entity::new(db, e))),
        _ => Ok(None),
    }
}

impl Entity {
    fn new(db: &Db, eid: i32) -> Entity {
        Entity(Arc::new(Inner {
            db: db.clone(),
            eid,
            state: Mutex::new(State { touched: false, cache: CljMap::new() }),
        }))
    }

    /// An entity of this id, whether or not it has datoms: what a host that already holds one hands back.
    pub fn of(db: &Db, eid: i32) -> Entity {
        Entity::new(db, eid)
    }

    pub fn db(&self) -> &Db {
        &self.0.db
    }

    pub fn eid(&self) -> i32 {
        self.0.eid
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.0.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The entity as a value, as it is in a set of references.
    pub fn to_value(&self) -> Value {
        Value::Host(HostObj(self.0.clone()))
    }

    /// The entity a value is, if it is one.
    pub fn from_value(v: &Value) -> Option<Entity> {
        match v {
            Value::Host(h) => {
                let any: Arc<dyn Any + Send + Sync> = h.0.clone().as_arc_any()?;
                any.downcast::<Inner>().ok().map(Entity)
            }
            _ => None,
        }
    }

    /// `lookup-entity`: an attribute's value. A reference is an entity, an attribute of cardinality many a set, a
    /// reverse attribute `:ns/_attr` the entities that refer to this one. `None` when there is nothing.
    pub fn lookup(&self, attr: &Value) -> Result<Option<Value>> {
        let (db, eid) = (&self.0.db, self.0.eid);
        if attr.is_kw(&kw().db_id) {
            return Ok(Some(Value::from(eid)));
        }
        if is_reverse_ref(attr)? {
            return lookup_backwards(db, eid, &reverse_ref(attr)?);
        }
        {
            let state = self.state();
            if let Some(v) = state.cache.get(attr).filter(|v| v.is_some()) {
                return Ok(Some(v.clone()));
            }
            if state.touched {
                return Ok(None);
            }
        }
        let Some(a) = value_attr(attr) else { return Ok(None) };
        let datoms = search(db, Some(eid), Some(&a), None, None).to_vec()?;
        if datoms.is_empty() {
            return Ok(None);
        }
        let value = entity_attr(db, &a, &datoms)?;
        self.state().cache.assoc(attr.clone(), value.clone());
        Ok(Some(value).filter(Value::is_some))
    }

    /// `(touch e)`: every attribute read and kept, components touched in turn.
    pub fn touch(&self) -> Result<()> {
        self.touch_at(0)
    }

    /// Components that lead back to the entity they came from are touched without end: each turn makes entities
    /// anew, none of them touched yet. ClojureScript runs out of stack; so, at a depth it would not reach, does this.
    fn touch_at(&self, depth: usize) -> Result<()> {
        if depth > MAX_TOUCH_DEPTH {
            return Err(Error::msg("Maximum call stack size exceeded"));
        }
        if self.state().touched {
            return Ok(());
        }
        let db = &self.0.db;
        let datoms = search(db, Some(self.0.eid), None, None, None).to_vec()?;
        if datoms.is_empty() {
            return Ok(());
        }
        // datoms->cache: each attribute's datoms, in the index's order
        let mut cache = CljMap::new();
        let mut start = 0;
        while start < datoms.len() {
            let a = datoms[start].a.clone();
            let end = datoms[start..].iter().position(|d| d.a != a).map_or(datoms.len(), |n| start + n);
            cache.assoc(datoms[start].a_value(), entity_attr(db, &a, &datoms[start..end])?);
            start = end;
        }
        // touch-components
        let mut touched = CljMap::new();
        for (a, v) in cache.iter() {
            let p = db.schema().props_of(a);
            let v = if p.component {
                if p.many {
                    let mut set = CljSet::new();
                    for x in v.seq_items().unwrap_or_default() {
                        if let Some(e) = Entity::from_value(&x) {
                            e.touch_at(depth + 1)?;
                        }
                        set.insert(x);
                    }
                    Value::set(set)
                } else {
                    if let Some(e) = Entity::from_value(v) {
                        e.touch_at(depth + 1)?;
                    }
                    v.clone()
                }
            } else {
                v.clone()
            };
            touched.assoc(a.clone(), v);
        }
        let mut state = self.state();
        state.cache = touched;
        state.touched = true;
        Ok(())
    }

    /// What has been read of the entity so far: `@(.-cache e)`.
    pub fn cache(&self) -> CljMap {
        self.state().cache.clone()
    }

    pub fn is_touched(&self) -> bool {
        self.state().touched
    }

    /// `(seq e)`: its attributes and values, all read.
    pub fn entries(&self) -> Result<Vec<(Value, Value)>> {
        self.touch()?;
        Ok(self.state().cache.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
    }

    /// `(contains? e k)`
    pub fn contains_key(&self, attr: &Value) -> Result<bool> {
        Ok(self.lookup_or_nil(attr)?.is_some())
    }

    /// `(get e attr not-found)`: `None` is not found; a reference to an entity that is not there is found, and `nil`.
    pub fn lookup_entry(&self, attr: &Value) -> Result<Option<Value>> {
        self.lookup_or_nil(attr)
    }

    /// `lookup-entity` as `contains?` reads it: a value that is there, be it `nil`.
    fn lookup_or_nil(&self, attr: &Value) -> Result<Option<Value>> {
        let (db, eid) = (&self.0.db, self.0.eid);
        if attr.is_kw(&kw().db_id) {
            return Ok(Some(Value::from(eid)));
        }
        if is_reverse_ref(attr)? {
            return lookup_backwards(db, eid, &reverse_ref(attr)?);
        }
        if let Some(v) = self.lookup(attr)? {
            return Ok(Some(v));
        }
        // a reference to an entity that is not there is a value, and that value is nil
        let Some(a) = value_attr(attr) else { return Ok(None) };
        if self.state().touched {
            return Ok(None);
        }
        Ok(if search(db, Some(eid), Some(&a), None, None).is_empty()? { None } else { Some(Value::Nil) })
    }

    /// `(assoc @cache :db/id eid)`: the map an entity prints as.
    pub fn print_map(&self) -> CljMap {
        let mut m = self.state().cache.clone();
        m.assoc(Value::Keyword(kw().db_id.clone()), Value::from(self.0.eid));
        m
    }

    /// `equiv-entity`: the same id in the same database value.
    pub fn equiv(&self, other: &Entity) -> bool {
        self.0.db.ptr_eq(&other.0.db) && self.0.eid == other.0.eid
    }
}

/// `entity-attr`: what an entity holds for an attribute, given its datoms.
fn entity_attr(db: &Db, a: &Attr, datoms: &[Datom]) -> Result<Value> {
    let p = props(db, a);
    let target = |d: &Datom| -> Result<Value> { Ok(entity(db, &d.v)?.map_or(Value::Nil, |e| e.to_value())) };
    if p.many {
        let mut set = CljSet::new();
        for d in datoms {
            set.insert(if p.is_ref { target(d)? } else { d.v.clone() });
        }
        Ok(Value::set(set))
    } else if p.is_ref {
        target(&datoms[0])
    } else {
        Ok(datoms[0].v.clone())
    }
}

/// `-lookup-backwards`: the entities whose `attr` refers to this one; the one entity, for a component.
fn lookup_backwards(db: &Db, eid: i32, attr: &Value) -> Result<Option<Value>> {
    let Some(a) = value_attr(attr) else {
        return Err(Error::msg(format!("Bad attribute type: {}", crate::print::pr_str(attr))));
    };
    let datoms = search(db, None, Some(&a), Some(&Value::from(eid)), None).to_vec()?;
    if datoms.is_empty() {
        return Ok(None);
    }
    let source =
        |d: &Datom| -> Result<Value> { Ok(entity(db, &Value::from(d.e))?.map_or(Value::Nil, |e| e.to_value())) };
    if props(db, &a).component {
        return Ok(Some(source(&datoms[0])?));
    }
    let mut set = CljSet::new();
    for d in &datoms {
        set.insert(source(d)?);
    }
    Ok(Some(Value::set(set)))
}

impl HostObject for Inner {
    /// `hash-entity`
    fn hash(&self) -> i32 {
        hash_combine(hash_number(self.eid as f64), db_uid(&self.db))
    }

    fn equiv(&self, other: &dyn HostObject) -> bool {
        other.as_any().downcast_ref::<Inner>().is_some_and(|o| self.db.ptr_eq(&o.db) && self.eid == o.eid)
    }

    fn type_name(&self) -> String {
        "datascript.impl.entity/Entity".into()
    }

    fn pr_str(&self) -> String {
        let mut m = self.state.lock().unwrap_or_else(|e| e.into_inner()).cache.clone();
        m.assoc(Value::Keyword(kw().db_id.clone()), Value::from(self.eid));
        crate::print::pr_str(&Value::map(m))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_arc_any(self: Arc<Self>) -> Option<Arc<dyn Any + Send + Sync>> {
        Some(self)
    }
}
