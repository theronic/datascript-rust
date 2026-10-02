//! `datascript.lru`: the cache of the last parsed queries and pull patterns. A key looked up again becomes the
//! newest; a new key, once the cache holds more than its limit, puts out the oldest.

use crate::lock::Lock;
use std::collections::{BTreeMap, HashMap};
use std::hash::Hash;

pub struct Lru<K, V> {
    key_value: HashMap<K, V>,
    gen_key: BTreeMap<u64, K>,
    key_gen: HashMap<K, u64>,
    gen: u64,
    limit: usize,
}

impl<K: Clone + Eq + Hash, V: Clone> Lru<K, V> {
    pub fn new(limit: usize) -> Lru<K, V> {
        Lru { key_value: HashMap::new(), gen_key: BTreeMap::new(), key_gen: HashMap::new(), gen: 0, limit }
    }

    pub fn get(&self, k: &K) -> Option<&V> {
        self.key_value.get(k)
    }

    pub fn contains_key(&self, k: &K) -> bool {
        self.key_value.contains_key(k)
    }

    pub fn len(&self) -> usize {
        self.key_value.len()
    }

    pub fn is_empty(&self) -> bool {
        self.key_value.is_empty()
    }

    /// `assoc-lru`: a key already there keeps its value and becomes the newest.
    pub fn assoc(&mut self, k: K, v: V) {
        if let Some(g) = self.key_gen.get(&k).copied() {
            self.gen_key.remove(&g);
            self.gen_key.insert(self.gen, k.clone());
            self.key_gen.insert(k, self.gen);
            self.gen += 1;
        } else {
            self.key_value.insert(k.clone(), v);
            self.gen_key.insert(self.gen, k.clone());
            self.key_gen.insert(k, self.gen);
            self.gen += 1;
            self.cleanup();
        }
    }

    /// `cleanup-lru`: over the limit, the oldest key goes.
    fn cleanup(&mut self) {
        if self.key_value.len() > self.limit {
            if let Some((g, k)) = self.gen_key.iter().next().map(|(g, k)| (*g, k.clone())) {
                self.key_value.remove(&k);
                self.gen_key.remove(&g);
                self.key_gen.remove(&k);
            }
        }
    }
}

/// `lru/cache`: the cached value for a key, or the one `compute` makes, which is then cached.
pub struct Cache<K, V> {
    inner: Lock<Lru<K, V>>,
}

impl<K: Clone + Eq + Hash, V: Clone> Cache<K, V> {
    pub fn new(limit: usize) -> Cache<K, V> {
        Cache { inner: Lock::new(Lru::new(limit)) }
    }

    pub fn get<E>(&self, key: &K, compute: impl FnOnce() -> Result<V, E>) -> Result<V, E> {
        {
            let mut lru = self.inner.lock();
            if let Some(cached) = lru.get(key).cloned() {
                lru.assoc(key.clone(), cached.clone());
                return Ok(cached);
            }
        }
        // computed outside the lock: computing may come back to this cache
        let computed = compute()?;
        let mut lru = self.inner.lock();
        lru.assoc(key.clone(), computed.clone());
        Ok(computed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // datascript.test.lru
    #[test]
    fn test_lru() {
        let mut l: Lru<&str, i32> = Lru::new(2);
        l.assoc("a", 1);
        assert_eq!(l.get(&"a"), Some(&1));
        l.assoc("b", 2);
        assert_eq!(l.get(&"a"), Some(&1));
        assert_eq!(l.get(&"b"), Some(&2));
        l.assoc("c", 3);
        assert_eq!(l.get(&"a"), None);
        assert_eq!(l.get(&"b"), Some(&2));
        assert_eq!(l.get(&"c"), Some(&3));
        // touching b makes it the newest, so c goes next
        l.assoc("b", 4);
        l.assoc("d", 5);
        assert_eq!(l.get(&"b"), Some(&2));
        assert_eq!(l.get(&"c"), None);
        assert_eq!(l.get(&"d"), Some(&5));
    }

    #[test]
    fn test_cache() {
        let cache: Cache<&str, i32> = Cache::new(2);
        let get = |k: &'static str, v: i32| cache.get::<()>(&k, || Ok(v)).unwrap();
        assert_eq!(get("a", 1), 1);
        assert_eq!(get("b", 2), 2);
        assert_eq!(get("a", 9), 1);
        assert_eq!(get("c", 3), 3);
        // b was the oldest
        assert_eq!(get("b", 7), 7);
        assert_eq!(get("a", 8), 8);
    }
}
