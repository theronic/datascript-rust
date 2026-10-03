//! `datascript.lru`: the cache of the last parsed queries and pull patterns. A key looked up again becomes the
//! newest; a new key, once the cache holds more than its limit, puts out the oldest.
//!
//! The cache is one list of at most `limit` entries, each with the time it was last asked for, and every change to
//! it is one entry written or one number stored. A call that the host's stack runs out under (`crate::lock`) can
//! then leave it nothing but whole: maps kept in step with each other could be left out of step, and a map of the
//! standard library's that is cut short while it moves its entries is no longer a map.

use crate::lock::Lock;
use std::hash::{Hash, Hasher};

struct Entry<K, V> {
    hash: u64,
    /// When it was last asked for: the oldest is the one put out
    at: u64,
    key: K,
    value: V,
}

pub struct Lru<K, V> {
    entries: Vec<Entry<K, V>>,
    /// Where the last key asked for was found: the same query is asked again more often than not
    last: usize,
    now: u64,
    limit: usize,
}

/// A key's hash, from what its `Hash` writes: FNV-1a, which is all a list this short needs.
#[derive(Default)]
struct Fnv(u64);

impl Hasher for Fnv {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        let mut h = if self.0 == 0 { 0xcbf2_9ce4_8422_2325 } else { self.0 };
        for b in bytes {
            h = (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3);
        }
        self.0 = h;
    }

    #[inline]
    fn finish(&self) -> u64 {
        self.0
    }
}

fn hash_of<K: Hash>(k: &K) -> u64 {
    let mut h = Fnv::default();
    k.hash(&mut h);
    h.finish()
}

impl<K: Eq + Hash, V> Lru<K, V> {
    pub fn new(limit: usize) -> Lru<K, V> {
        Lru { entries: Vec::new(), last: 0, now: 0, limit }
    }

    fn position(&self, k: &K) -> Option<usize> {
        let hash = hash_of(k);
        let is = |e: &Entry<K, V>| e.hash == hash && e.key == *k;
        if self.entries.get(self.last).is_some_and(is) {
            return Some(self.last);
        }
        self.entries.iter().position(is)
    }

    pub fn get(&self, k: &K) -> Option<&V> {
        self.position(k).map(|i| &self.entries[i].value)
    }

    pub fn contains_key(&self, k: &K) -> bool {
        self.position(k).is_some()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The value of a key that is here, which becomes the newest.
    pub fn touch(&mut self, k: &K) -> Option<&V> {
        let i = self.position(k)?;
        self.entries[i].at = self.now;
        self.now += 1;
        self.last = i;
        Some(&self.entries[i].value)
    }

    /// `assoc-lru`: a key already there keeps its value and becomes the newest; a new one, over the limit, takes
    /// the place of the oldest (`cleanup-lru`).
    pub fn assoc(&mut self, k: K, v: V) {
        if self.touch(&k).is_some() || self.limit == 0 {
            return;
        }
        let entry = Entry { hash: hash_of(&k), at: self.now, key: k, value: v };
        if self.entries.len() < self.limit {
            self.entries.push(entry);
            self.last = self.entries.len() - 1;
            self.now += 1;
        } else {
            let mut oldest = 0;
            for (i, e) in self.entries.iter().enumerate() {
                if e.at < self.entries[oldest].at {
                    oldest = i;
                }
            }
            // the entry is written whole, and what it replaces is dropped only after
            let before = std::mem::replace(&mut self.entries[oldest], entry);
            self.last = oldest;
            self.now += 1;
            drop(before);
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
        if let Some(cached) = self.inner.lock().touch(key) {
            return Ok(cached.clone());
        }
        // computed outside the lock: computing may come back to this cache
        let computed = compute()?;
        self.inner.lock().assoc(key.clone(), computed.clone());
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

    /// Against the cache as `datascript.lru` keeps it: a map, and the order its keys were last asked for in.
    #[test]
    fn the_oldest_goes_whatever_was_asked_for() {
        let limit = 7;
        let mut l: Lru<u32, u32> = Lru::new(limit);
        let mut model: Vec<(u32, u32)> = Vec::new(); // oldest first
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        for step in 0..20_000u32 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let k = (seed % 23) as u32;
            if seed & 0x100 == 0 {
                assert_eq!(l.get(&k), model.iter().find(|(key, _)| *key == k).map(|(_, v)| v), "step {step}");
                continue;
            }
            l.assoc(k, step);
            match model.iter().position(|(key, _)| *key == k) {
                Some(i) => {
                    let kept = model.remove(i);
                    model.push(kept);
                }
                None => {
                    model.push((k, step));
                    if model.len() > limit {
                        model.remove(0);
                    }
                }
            }
            assert_eq!(l.len(), model.len());
            for (key, v) in &model {
                assert_eq!(l.get(key), Some(v), "step {step}");
            }
        }
    }

    #[test]
    fn a_cache_of_nothing_keeps_nothing() {
        let mut l: Lru<&str, i32> = Lru::new(0);
        l.assoc("a", 1);
        assert_eq!(l.get(&"a"), None);
        assert!(l.is_empty());
    }
}
