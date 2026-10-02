//! ClojureScript's maps and sets, as far as their order goes.
//!
//! DataScript returns ClojureScript collections, and what a caller sees of them is their order of iteration: the
//! order a query's result set prints in, the order `[:find [?e ...]]` lists its entities, the order an entity map's
//! attributes are transacted. ClojureScript keeps a map of up to 8 entries as an array, in the order they were added,
//! and a larger one as a hash trie, in the order of its keys' hashes (`hash::hamt_order`). A set is a map of its
//! elements. These types keep the same two shapes and move between them when ClojureScript does, so they iterate as
//! ClojureScript's do.

use crate::hash::{hamt_order, hash_map_entry, hash_unordered};
use crate::value::Value;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::sync::OnceLock;

/// The collections waiting to be dropped, and whether someone is dropping them.
struct Pending {
    draining: bool,
    queue: Vec<Value>,
}

thread_local! {
    static PENDING: RefCell<Pending> = const { RefCell::new(Pending { draining: false, queue: Vec::new() }) };
}

/// Drops the collections that a collection being dropped held. They wait their turn in a list, and the first
/// collection to be dropped drops them all: a value nested thousands deep, which a recursive pull makes, is then
/// dropped without a call for every level of it, which WebAssembly's stack has no room for.
pub(crate) fn drop_nested(nested: Vec<Value>) {
    let first = PENDING.try_with(|pending| {
        let mut pending = pending.borrow_mut();
        pending.queue.extend(nested);
        !std::mem::replace(&mut pending.draining, true)
    });
    // no list any more, as a thread ends: `nested` was dropped as it is
    if first != Ok(true) {
        return;
    }
    // each is dropped outside the borrow: its own collections join the list
    while let Some(next) = PENDING.with(|pending| pending.borrow_mut().queue.pop()) {
        drop(next);
    }
    PENDING.with(|pending| pending.borrow_mut().draining = false);
}

/// Lets the list of collections to drop be dropped again, after a drop that did not finish: a host whose stack ran
/// out under the module calls this, since nothing unwinds there.
pub fn recover_drops() {
    let recovered = PENDING.try_with(|pending| match pending.try_borrow_mut() {
        Ok(mut pending) => {
            pending.draining = false;
            true
        }
        Err(_) => false,
    });
    if recovered == Ok(true) {
        drop_nested(Vec::new());
    }
}

/// The collections among a map's or a set's keys and values, taken out of it.
fn take_nested<V: Nested>(repr: &mut Repr<V>) -> Vec<Value> {
    let mut nested = Vec::new();
    let mut keep = |k: Value, v: V| {
        if k.nests() {
            nested.push(k);
        }
        v.keep(&mut nested);
    };
    match std::mem::replace(repr, Repr::Array(Vec::new())) {
        Repr::Array(entries) => entries.into_iter().for_each(|(k, v)| keep(k, v)),
        Repr::Hash { tree, .. } => tree.into_values().for_each(|(k, v)| keep(k, v)),
    }
    nested
}

/// What a map's value or a set's lack of one adds to the collections to drop.
trait Nested {
    fn nests(&self) -> bool;
    fn keep(self, nested: &mut Vec<Value>);
}

impl Nested for Value {
    fn nests(&self) -> bool {
        Value::nests(self)
    }

    fn keep(self, nested: &mut Vec<Value>) {
        if Value::nests(&self) {
            nested.push(self);
        }
    }
}

impl Nested for () {
    fn nests(&self) -> bool {
        false
    }

    fn keep(self, _: &mut Vec<Value>) {}
}

/// `PersistentArrayMap.HASHMAP-THRESHOLD`
pub const HASHMAP_THRESHOLD: usize = 8;

#[derive(Clone)]
enum Repr<V> {
    /// `PersistentArrayMap`: in the order added
    Array(Vec<(Value, V)>),
    /// `PersistentHashMap`: by the keys' hashes, in the trie's order; keys with one hash in the order added. The
    /// `nil` key, which ClojureScript keeps beside the trie and iterates first, is at `(0, 0)`.
    Hash { tree: BTreeMap<(u32, u32), (Value, V)>, next: u32 },
}

impl<V: Clone> Repr<V> {
    fn len(&self) -> usize {
        match self {
            Repr::Array(a) => a.len(),
            Repr::Hash { tree, .. } => tree.len(),
        }
    }

    fn find(&self, k: &Value) -> Option<&(Value, V)> {
        match self {
            Repr::Array(a) => a.iter().find(|(x, _)| x == k),
            Repr::Hash { tree, .. } => {
                if k.is_nil() {
                    return tree.get(&(0, 0));
                }
                let h = hamt_order(k.cljs_hash());
                tree.range((h, 1)..=(h, u32::MAX)).map(|(_, e)| e).find(|(x, _)| x == k)
            }
        }
    }

    fn hash_slot(tree: &BTreeMap<(u32, u32), (Value, V)>, k: &Value) -> Option<(u32, u32)> {
        if k.is_nil() {
            return tree.contains_key(&(0, 0)).then_some((0, 0));
        }
        let h = hamt_order(k.cljs_hash());
        tree.range((h, 1)..=(h, u32::MAX)).find(|(_, (x, _))| x == k).map(|(slot, _)| *slot)
    }

    fn hash_insert(tree: &mut BTreeMap<(u32, u32), (Value, V)>, next: &mut u32, k: Value, v: V) -> bool {
        match Self::hash_slot(tree, &k) {
            // the key that was there stays, as ClojureScript keeps it
            Some(slot) => {
                tree.get_mut(&slot).unwrap().1 = v;
                false
            }
            None => {
                let slot = if k.is_nil() {
                    (0, 0)
                } else {
                    *next += 1;
                    (hamt_order(k.cljs_hash()), *next)
                };
                tree.insert(slot, (k, v));
                true
            }
        }
    }

    /// `assoc`: true when the key is new.
    fn insert(&mut self, k: Value, v: V) -> bool {
        match self {
            Repr::Array(a) => {
                if let Some(e) = a.iter_mut().find(|(x, _)| *x == k) {
                    e.1 = v;
                    return false;
                }
                if a.len() < HASHMAP_THRESHOLD {
                    a.push((k, v));
                    return true;
                }
                // the ninth entry: into a hash map, as `(into PersistentHashMap.EMPTY coll)` and `assoc`
                let mut tree = BTreeMap::new();
                let mut next = 0;
                for (x, y) in std::mem::take(a) {
                    Self::hash_insert(&mut tree, &mut next, x, y);
                }
                Self::hash_insert(&mut tree, &mut next, k, v);
                *self = Repr::Hash { tree, next };
                true
            }
            Repr::Hash { tree, next } => Self::hash_insert(tree, next, k, v),
        }
    }

    /// `dissoc`, which keeps the order of what remains. A hash map stays a hash map.
    fn remove(&mut self, k: &Value) -> Option<(Value, V)> {
        match self {
            Repr::Array(a) => {
                let i = a.iter().position(|(x, _)| x == k)?;
                Some(a.remove(i))
            }
            Repr::Hash { tree, .. } => {
                let slot = Self::hash_slot(tree, k)?;
                tree.remove(&slot)
            }
        }
    }

    /// `dissoc!` on a transient: an array map moves its last entry into the gap.
    fn remove_transient(&mut self, k: &Value) -> Option<(Value, V)> {
        match self {
            Repr::Array(a) => {
                let i = a.iter().position(|(x, _)| x == k)?;
                Some(a.swap_remove(i))
            }
            Repr::Hash { .. } => self.remove(k),
        }
    }

    fn iter(&self) -> ReprIter<'_, V> {
        match self {
            Repr::Array(a) => ReprIter::Array(a.iter()),
            Repr::Hash { tree, .. } => ReprIter::Hash(tree.values()),
        }
    }

    fn is_array(&self) -> bool {
        matches!(self, Repr::Array(_))
    }
}

enum ReprIter<'a, V> {
    Array(std::slice::Iter<'a, (Value, V)>),
    Hash(std::collections::btree_map::Values<'a, (u32, u32), (Value, V)>),
}

impl<'a, V> Iterator for ReprIter<'a, V> {
    type Item = &'a (Value, V);

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        match self {
            ReprIter::Array(i) => i.next(),
            ReprIter::Hash(i) => i.next(),
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        match self {
            ReprIter::Array(i) => i.size_hint(),
            ReprIter::Hash(i) => i.size_hint(),
        }
    }
}

/// A ClojureScript map.
#[derive(Clone)]
pub struct CljMap {
    repr: Repr<Value>,
    hash: OnceLock<i32>,
}

impl Default for CljMap {
    fn default() -> Self {
        CljMap::new()
    }
}

impl CljMap {
    /// `{}`
    pub fn new() -> CljMap {
        CljMap { repr: Repr::Array(Vec::new()), hash: OnceLock::new() }
    }

    /// `(hash-map)`: a hash map however few its entries.
    pub fn new_hash() -> CljMap {
        CljMap { repr: Repr::Hash { tree: BTreeMap::new(), next: 0 }, hash: OnceLock::new() }
    }

    /// `(array-map k v ...)`: an array map however many its entries; a later key replaces an earlier one's value.
    pub fn array_map<I: IntoIterator<Item = (Value, Value)>>(pairs: I) -> CljMap {
        let mut a: Vec<(Value, Value)> = Vec::new();
        for (k, v) in pairs {
            match a.iter_mut().find(|(x, _)| *x == k) {
                Some(e) => e.1 = v,
                None => a.push((k, v)),
            }
        }
        CljMap { repr: Repr::Array(a), hash: OnceLock::new() }
    }

    /// An array map of entries whose keys are known to differ, in their order: a map as another runtime held it.
    pub fn array_map_of_distinct(pairs: Vec<(Value, Value)>) -> CljMap {
        CljMap { repr: Repr::Array(pairs), hash: OnceLock::new() }
    }

    /// `(hash-map k v ...)`
    pub fn hash_map<I: IntoIterator<Item = (Value, Value)>>(pairs: I) -> CljMap {
        let mut m = CljMap::new_hash();
        for (k, v) in pairs {
            m.assoc(k, v);
        }
        m
    }

    /// `(into {} pairs)`, which is also `zipmap` and a transient map filled in order.
    pub fn from_pairs<I: IntoIterator<Item = (Value, Value)>>(pairs: I) -> CljMap {
        let mut m = CljMap::new();
        for (k, v) in pairs {
            m.assoc(k, v);
        }
        m
    }

    /// A map literal as ClojureScript's reader makes it: an array map of up to 8 entries in the order written, a
    /// hash map of more.
    pub fn from_literal(pairs: Vec<(Value, Value)>) -> CljMap {
        if pairs.len() <= HASHMAP_THRESHOLD {
            CljMap::array_map(pairs)
        } else {
            CljMap::hash_map(pairs)
        }
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.repr.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Whether it is a `PersistentArrayMap`, which keeps the order its entries were added in.
    #[inline]
    pub fn is_array(&self) -> bool {
        self.repr.is_array()
    }

    pub fn get(&self, k: &Value) -> Option<&Value> {
        self.repr.find(k).map(|(_, v)| v)
    }

    pub fn contains_key(&self, k: &Value) -> bool {
        self.repr.find(k).is_some()
    }

    /// `assoc`. True when the key is new.
    pub fn assoc(&mut self, k: Value, v: Value) -> bool {
        self.hash = OnceLock::new();
        self.repr.insert(k, v)
    }

    /// `dissoc`
    pub fn dissoc(&mut self, k: &Value) -> Option<Value> {
        self.hash = OnceLock::new();
        self.repr.remove(k).map(|(_, v)| v)
    }

    /// `dissoc!` on a transient map
    pub fn dissoc_transient(&mut self, k: &Value) -> Option<Value> {
        self.hash = OnceLock::new();
        self.repr.remove_transient(k).map(|(_, v)| v)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&Value, &Value)> + '_ {
        self.repr.iter().map(|(k, v)| (k, v))
    }

    pub fn keys(&self) -> impl Iterator<Item = &Value> + '_ {
        self.repr.iter().map(|(k, _)| k)
    }

    pub fn vals(&self) -> impl Iterator<Item = &Value> + '_ {
        self.repr.iter().map(|(_, v)| v)
    }

    /// `(empty m)`: an empty map of the same kind.
    pub fn empty_like(&self) -> CljMap {
        if self.is_array() {
            CljMap::new()
        } else {
            CljMap::new_hash()
        }
    }

    pub fn cljs_hash(&self) -> i32 {
        *self
            .hash
            .get_or_init(|| hash_unordered(self.iter().map(|(k, v)| hash_map_entry(k.cljs_hash(), v.cljs_hash()))))
    }
}

impl Drop for CljMap {
    fn drop(&mut self) {
        if self.repr.iter().any(|(k, v)| k.nests() || Nested::nests(v)) {
            drop_nested(take_nested(&mut self.repr));
        }
    }
}

impl PartialEq for CljMap {
    /// `equiv-map`
    fn eq(&self, other: &CljMap) -> bool {
        self.len() == other.len() && self.iter().all(|(k, v)| other.get(k).is_some_and(|w| v == w))
    }
}

impl FromIterator<(Value, Value)> for CljMap {
    fn from_iter<I: IntoIterator<Item = (Value, Value)>>(iter: I) -> CljMap {
        CljMap::from_pairs(iter)
    }
}

/// A ClojureScript set: a map of its elements, in a map's order.
#[derive(Clone)]
pub struct CljSet {
    repr: Repr<()>,
    hash: OnceLock<i32>,
}

impl Default for CljSet {
    fn default() -> Self {
        CljSet::new()
    }
}

impl CljSet {
    /// `#{}`
    pub fn new() -> CljSet {
        CljSet { repr: Repr::Array(Vec::new()), hash: OnceLock::new() }
    }

    /// A set that keeps its elements in the order given, however many: one of up to 8 that another runtime held.
    /// The elements are known to differ.
    pub fn array_set_of_distinct(items: Vec<Value>) -> CljSet {
        CljSet { repr: Repr::Array(items.into_iter().map(|v| (v, ())).collect()), hash: OnceLock::new() }
    }

    /// A set in the order of its elements' hashes, however few they are: one that was larger once.
    pub fn hash_set<I: IntoIterator<Item = Value>>(items: I) -> CljSet {
        let mut repr = Repr::Hash { tree: BTreeMap::new(), next: 0 };
        for v in items {
            repr.insert(v, ());
        }
        CljSet { repr, hash: OnceLock::new() }
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.repr.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn contains(&self, v: &Value) -> bool {
        self.repr.find(v).is_some()
    }

    /// The element equal to `v` that the set holds
    pub fn get(&self, v: &Value) -> Option<&Value> {
        self.repr.find(v).map(|(k, _)| k)
    }

    /// `conj`. True when the element is new.
    pub fn insert(&mut self, v: Value) -> bool {
        self.hash = OnceLock::new();
        if self.repr.find(&v).is_some() {
            return false;
        }
        self.repr.insert(v, ())
    }

    /// `disj`
    pub fn remove(&mut self, v: &Value) -> bool {
        self.hash = OnceLock::new();
        self.repr.remove(v).is_some()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Value> + '_ {
        self.repr.iter().map(|(k, _)| k)
    }

    /// Whether it still keeps the order its elements were added in (up to 8 of them).
    #[inline]
    pub fn is_array(&self) -> bool {
        self.repr.is_array()
    }

    pub fn cljs_hash(&self) -> i32 {
        *self.hash.get_or_init(|| hash_unordered(self.iter().map(Value::cljs_hash)))
    }
}

impl Drop for CljSet {
    fn drop(&mut self) {
        if self.repr.iter().any(|(k, _)| k.nests()) {
            drop_nested(take_nested(&mut self.repr));
        }
    }
}

impl PartialEq for CljSet {
    fn eq(&self, other: &CljSet) -> bool {
        self.len() == other.len() && self.iter().all(|v| other.contains(v))
    }
}

impl FromIterator<Value> for CljSet {
    /// `(set coll)`, `(into #{} coll)`
    fn from_iter<I: IntoIterator<Item = Value>>(iter: I) -> CljSet {
        let mut s = CljSet::new();
        for v in iter {
            s.insert(v);
        }
        s
    }
}
