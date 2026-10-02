//! ClojureScript records, as DataScript's parser makes them: a named type with fields in order. They hash, compare
//! and print as ClojureScript's do, since a query's parse tree is put in maps and sets, and printed in errors.

use crate::hash::{hash_map_entry, hash_string, hash_unordered};
use crate::lock::HashCache;
use crate::print::pr_into;
use crate::value::{HostObj, HostObject, Value};
use std::any::Any;

pub struct Record {
    /// The namespace, `datascript.parser`
    pub ns: &'static str,
    /// The type's name, `Variable`
    pub name: &'static str,
    pub fields: Vec<(&'static str, Value)>,
    /// `(:source (meta record))`: the form it was parsed from, where the parser keeps it
    pub source: Option<Value>,
    hash: HashCache,
}

impl Record {
    /// The value of a field; `nil` when there is no such field.
    pub fn get(&self, field: &str) -> &Value {
        static NIL: Value = Value::Nil;
        self.fields.iter().find(|(k, _)| *k == field).map_or(&NIL, |(_, v)| v)
    }

    pub fn is(&self, name: &str) -> bool {
        self.name == name
    }
}

impl HostObject for Record {
    /// A record's hash: of its entries as a map's, mixed with the hash of its type's name.
    fn hash(&self) -> i32 {
        self.hash.get_or(|| {
            let magic = hash_string(&format!("{}.{}", self.ns, self.name));
            let entries = self.fields.iter().map(|(k, v)| hash_map_entry(Value::kw(k).cljs_hash(), v.cljs_hash()));
            magic ^ hash_unordered(entries)
        })
    }

    fn equiv(&self, other: &dyn HostObject) -> bool {
        match other.as_any().downcast_ref::<Record>() {
            Some(o) => {
                self.name == o.name
                    && self.ns == o.ns
                    && self.fields.len() == o.fields.len()
                    && self.fields.iter().zip(&o.fields).all(|((_, a), (_, b))| a == b)
            }
            None => false,
        }
    }

    fn type_name(&self) -> String {
        format!("{}/{}", self.ns, self.name)
    }

    /// `#datascript.parser.Variable{:symbol ?x}`
    fn pr_str(&self) -> String {
        let mut out = format!("#{}.{}{{", self.ns, self.name);
        for (i, (k, v)) in self.fields.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            out.push(':');
            out.push_str(k);
            out.push(' ');
            pr_into(&mut out, v);
        }
        out.push('}');
        out
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// A record value.
pub fn record(ns: &'static str, name: &'static str, fields: Vec<(&'static str, Value)>) -> Value {
    Value::Host(HostObj::new(Record { ns, name, fields, source: None, hash: HashCache::new() }))
}

/// A record value that remembers the form it was parsed from (`with-source`).
pub fn record_with_source(
    ns: &'static str,
    name: &'static str,
    fields: Vec<(&'static str, Value)>,
    source: &Value,
) -> Value {
    Value::Host(HostObj::new(Record { ns, name, fields, source: Some(source.clone()), hash: HashCache::new() }))
}

/// The record a value is, if it is one.
#[inline]
pub fn as_record(v: &Value) -> Option<&Record> {
    match v {
        Value::Host(h) => h.0.as_any().downcast_ref::<Record>(),
        _ => None,
    }
}

/// Whether a value is a record of this type.
#[inline]
pub fn is_record(v: &Value, name: &str) -> bool {
    as_record(v).is_some_and(|r| r.name == name)
}
