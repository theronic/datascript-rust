//! The functions of `cljs.core` that DataScript's answers pass through, with ClojureScript's results: the ones a
//! query may call by name, and the ones the port itself needs to build a collection as ClojureScript would.

use crate::coll::{CljMap, CljSet};
use crate::error::{Error, Result};
use crate::print::pr_str;
use crate::value::Value;
use std::sync::Arc;

/// `(conj coll x)`: onto the end of a vector, the front of a list, into a set, an entry into a map. `nil` is an
/// empty list.
pub fn conj(coll: &Value, x: Value) -> Result<Value> {
    match coll {
        Value::Nil => Ok(Value::list(vec![x])),
        Value::Vector(items) => {
            let mut items = items.items().to_vec();
            items.push(x);
            Ok(Value::vector(items))
        }
        Value::List(items) => {
            let mut out = Vec::with_capacity(items.len() + 1);
            out.push(x);
            out.extend(items.iter().cloned());
            Ok(Value::list(out))
        }
        Value::Set(s) => {
            let mut s = (**s).clone();
            s.insert(x);
            Ok(Value::set(s))
        }
        Value::Map(m) => {
            let mut m = (**m).clone();
            conj_map(&mut m, &x)?;
            Ok(Value::map(m))
        }
        _ => Err(Error::msg(format!("No protocol method ICollection.-conj defined for type {}", coll.type_name()))),
    }
}

/// `conj` on a map: a vector `[k v]` is an entry; anything else is entries, a map's among them.
fn conj_map(m: &mut CljMap, x: &Value) -> Result<()> {
    match x {
        Value::Vector(kv) => {
            if kv.len() != 2 {
                return Err(Error::msg("Index out of bounds"));
            }
            m.assoc(kv[0].clone(), kv[1].clone());
            Ok(())
        }
        Value::Map(other) => {
            for (k, v) in other.iter() {
                m.assoc(k.clone(), v.clone());
            }
            Ok(())
        }
        Value::Nil => Ok(()),
        _ => {
            for e in x.seq_items().ok_or_else(|| Error::msg(format!("{} is not ISeqable", pr_str(x))))? {
                match &e {
                    Value::Vector(kv) if kv.len() == 2 => {
                        m.assoc(kv[0].clone(), kv[1].clone());
                    }
                    _ => return Err(Error::msg("conj on a map takes map entries or seqables of map entries")),
                }
            }
            Ok(())
        }
    }
}

/// `(into {} pairs)`, where each of `pairs` is `[k v]` or a map.
pub fn into_map(pairs: &[Value]) -> Result<CljMap> {
    let mut m = CljMap::new();
    for p in pairs {
        conj_map(&mut m, p)?;
    }
    Ok(m)
}

/// `(merge & maps)`
pub fn merge(maps: &[Value]) -> Result<Value> {
    if !maps.iter().any(Value::truthy) {
        return Ok(Value::Nil);
    }
    let mut acc: Option<Value> = None;
    for m in maps {
        acc = Some(match acc {
            None => m.clone(),
            Some(a) => {
                let base = if a.truthy() { a } else { Value::map(CljMap::new()) };
                conj(&base, m.clone())?
            }
        });
    }
    Ok(acc.unwrap_or(Value::Nil))
}

/// `(select-keys m ks)`
pub fn select_keys(m: &Value, ks: &[Value]) -> Value {
    let mut out = CljMap::new();
    for k in ks {
        if let Some(v) = get(m, k) {
            out.assoc(k.clone(), v);
        }
    }
    Value::map(out)
}

/// `(get coll k)`: a map's value, a set's element, a vector's or string's nth; `None` when there is none.
pub fn get(coll: &Value, k: &Value) -> Option<Value> {
    match coll {
        Value::Map(m) => m.get(k).cloned(),
        Value::Set(s) => s.get(k).cloned(),
        Value::Vector(items) => {
            let i = k.as_num()?;
            if i.fract() != 0.0 || i < 0.0 {
                return None;
            }
            items.get(i as usize).cloned()
        }
        Value::Str(s) => {
            let i = k.as_num()?;
            if i.fract() != 0.0 || i < 0.0 {
                return None;
            }
            crate::print::string_chars(s).into_iter().nth(i as usize)
        }
        Value::Datom(d) => d.val_at(k),
        _ => None,
    }
}

/// `(contains? coll k)`
pub fn contains(coll: &Value, k: &Value) -> Result<bool> {
    match coll {
        Value::Nil => Ok(false),
        Value::Map(m) => Ok(m.contains_key(k)),
        Value::Set(s) => Ok(s.contains(k)),
        Value::Vector(items) => {
            Ok(k.as_num().is_some_and(|i| i.fract() == 0.0 && i >= 0.0 && (i as usize) < items.len()))
        }
        Value::Str(s) => {
            Ok(k.as_num().is_some_and(|i| i.fract() == 0.0 && i >= 0.0 && (i as usize) < s.encode_utf16().count()))
        }
        _ => Err(Error::msg(format!("contains? not supported on type: {}", coll.type_name()))),
    }
}

/// `(assoc m k v ...)` on a map, a vector or `nil`.
pub fn assoc(coll: &Value, kvs: &[Value]) -> Result<Value> {
    if kvs.len() % 2 != 0 {
        return Err(Error::msg("assoc expects even number of arguments after map/vector, found odd number"));
    }
    match coll {
        Value::Nil | Value::Map(_) => {
            let mut m = match coll {
                Value::Map(m) => (**m).clone(),
                _ => CljMap::new(),
            };
            for kv in kvs.chunks(2) {
                m.assoc(kv[0].clone(), kv[1].clone());
            }
            Ok(Value::map(m))
        }
        Value::Vector(items) => {
            let mut items = items.items().to_vec();
            for kv in kvs.chunks(2) {
                let i = kv[0].as_num().filter(|i| i.fract() == 0.0 && *i >= 0.0).map(|i| i as usize);
                match i {
                    Some(i) if i < items.len() => items[i] = kv[1].clone(),
                    Some(i) if i == items.len() => items.push(kv[1].clone()),
                    _ => {
                        return Err(Error::msg(format!("Index {} out of bounds  [0,{}]", pr_str(&kv[0]), items.len())))
                    }
                }
            }
            Ok(Value::vector(items))
        }
        _ => Err(Error::msg(format!("No protocol method IAssociative.-assoc defined for type {}", coll.type_name()))),
    }
}

/// `(dissoc m k ...)`
pub fn dissoc(coll: &Value, ks: &[Value]) -> Result<Value> {
    match coll {
        Value::Nil => Ok(Value::Nil),
        Value::Map(m) => {
            let mut m = (**m).clone();
            for k in ks {
                m.dissoc(k);
            }
            Ok(Value::map(m))
        }
        _ => Err(Error::msg(format!("No protocol method IMap.-dissoc defined for type {}", coll.type_name()))),
    }
}

/// `(disj s x ...)`
pub fn disj(coll: &Value, xs: &[Value]) -> Result<Value> {
    match coll {
        Value::Nil => Ok(Value::Nil),
        Value::Set(s) => {
            let mut s = (**s).clone();
            for x in xs {
                s.remove(x);
            }
            Ok(Value::set(s))
        }
        _ => Err(Error::msg(format!("No protocol method ISet.-disjoin defined for type {}", coll.type_name()))),
    }
}

/// `(set coll)`
pub fn set(coll: &Value) -> Result<Value> {
    if let Value::Set(_) = coll {
        return Ok(coll.clone());
    }
    let items = coll.seq_items().ok_or_else(|| Error::msg(format!("{} is not ISeqable", pr_str(coll))))?;
    Ok(Value::set(items.into_iter().collect()))
}

/// `(distinct coll)`
pub fn distinct(items: &[Value]) -> Vec<Value> {
    let mut seen = CljSet::new();
    items.iter().filter(|x| seen.insert((*x).clone())).cloned().collect()
}

/// `(group-by f coll)`: each key's elements in order, the keys in the order of ClojureScript's map of them.
pub fn group_by<K: FnMut(&Value) -> Result<Value>>(items: &[Value], mut key: K) -> Result<CljMap> {
    let mut out = CljMap::new();
    for x in items {
        let k = key(x)?;
        let mut group = match out.get(&k) {
            Some(Value::Vector(g)) => g.items().to_vec(),
            _ => Vec::new(),
        };
        group.push(x.clone());
        out.assoc(k, Value::vector(group));
    }
    Ok(out)
}

/// `(frequencies coll)`
pub fn frequencies(items: &[Value]) -> CljMap {
    let mut out = CljMap::new();
    for x in items {
        let n = out.get(x).and_then(Value::as_num).unwrap_or(0.0);
        out.assoc(x.clone(), Value::Num(n + 1.0));
    }
    out
}

/// `(zipmap ks vs)`
pub fn zipmap(ks: &[Value], vs: &[Value]) -> CljMap {
    ks.iter().cloned().zip(vs.iter().cloned()).collect()
}

/// `(sort comp coll)`: a stable sort; the first failure of `comp` is the sort's.
pub fn sort_by<C: FnMut(&Value, &Value) -> Result<std::cmp::Ordering>>(items: &mut [Value], mut comp: C) -> Result<()> {
    let mut failure = None;
    items.sort_by(|a, b| match comp(a, b) {
        Ok(o) => o,
        Err(e) => {
            failure.get_or_insert(e);
            std::cmp::Ordering::Equal
        }
    });
    match failure {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

/// The elements of a seqable value, or ClojureScript's complaint that it is not one.
pub fn seq(v: &Value) -> Result<Vec<Value>> {
    v.seq_items().ok_or_else(|| Error::msg(format!("{} is not ISeqable", pr_str(v))))
}

/// `(vec coll)`
pub fn vec_of(v: &Value) -> Result<Value> {
    match v {
        Value::Vector(_) => Ok(v.clone()),
        _ => Ok(Value::vector(seq(v)?)),
    }
}

/// A vector's elements as a list, sharing them.
pub fn as_list(v: &Value) -> Value {
    match v {
        Value::Vector(s) => Value::List(Arc::clone(s)),
        _ => v.clone(),
    }
}

/// ClojureScript's complaint that a value has no count.
pub fn no_count(v: &Value) -> String {
    let ty = match v {
        Value::Num(_) => "number",
        Value::Bool(_) => "boolean",
        Value::Keyword(_) => "cljs.core/Keyword",
        Value::Symbol(_) => "cljs.core/Symbol",
        Value::Fn(_) => "function",
        _ => "object",
    };
    format!("No protocol method ICounted.-count defined for type {ty}: {}", crate::print::str_of(v))
}
