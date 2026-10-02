//! The values DataScript works with: ClojureScript's, with ClojureScript's equality and hashes.

use crate::coll::{CljMap, CljSet};
use crate::datom::Datom;
use crate::db::Db;
use crate::error::Result;
use crate::hash;
use crate::named::{Keyword, Symbol};
use std::any::Any;
use std::cmp::Ordering;
use std::sync::atomic::{AtomicU32, Ordering as AtomicOrdering};
use std::sync::{Arc, OnceLock};

pub type Str = Arc<str>;

/// A ClojureScript value.
///
/// Numbers are JavaScript's: doubles. A character is a string of one, as ClojureScript reads it. `List` is every
/// sequential collection that is not a vector: a list, a lazy sequence.
#[derive(Clone)]
pub enum Value {
    Nil,
    Bool(bool),
    Num(f64),
    Str(Str),
    Keyword(Keyword),
    Symbol(Symbol),
    Vector(Arc<Seq>),
    List(Arc<Seq>),
    Map(Arc<CljMap>),
    Set(Arc<CljSet>),
    /// `#uuid "..."`, as ClojureScript keeps it: its lower-cased string
    Uuid(Str),
    /// `#inst "..."`: milliseconds since the epoch
    Inst(f64),
    Regex(Arc<Regex>),
    Datom(Arc<Datom>),
    Db(Db),
    Fn(Func),
    /// A value of the host's that the port does not look into
    Host(HostObj),
}

/// The elements of a vector or a list, with their hash once computed.
pub struct Seq {
    items: Vec<Value>,
    hash: OnceLock<i32>,
}

impl Seq {
    pub fn new(items: Vec<Value>) -> Seq {
        Seq { items, hash: OnceLock::new() }
    }

    #[inline]
    pub fn items(&self) -> &[Value] {
        &self.items
    }

    pub fn into_items(mut self) -> Vec<Value> {
        std::mem::take(&mut self.items)
    }

    pub fn cljs_hash(&self) -> i32 {
        *self.hash.get_or_init(|| hash::hash_ordered(self.items.iter().map(Value::cljs_hash)))
    }
}

impl Clone for Seq {
    fn clone(&self) -> Seq {
        Seq::new(self.items.clone())
    }
}

impl Drop for Seq {
    fn drop(&mut self) {
        if self.items.iter().any(Value::nests) {
            let items = std::mem::take(&mut self.items);
            crate::coll::drop_nested(items.into_iter().filter(Value::nests).collect());
        }
    }
}

impl std::ops::Deref for Seq {
    type Target = [Value];

    #[inline]
    fn deref(&self) -> &[Value] {
        &self.items
    }
}

/// A regular expression, as `re-pattern` makes one. `crate::regex` matches it.
pub struct Regex {
    pub source: Str,
    pub flags: Str,
    id: u32,
    /// What it compiles to, once it has been matched against
    pub(crate) program: OnceLock<crate::regex::Compiled>,
}

impl Regex {
    pub fn new(source: &str, flags: &str) -> Regex {
        Regex { source: Arc::from(source), flags: Arc::from(flags), id: next_uid(), program: OnceLock::new() }
    }
}

/// ClojureScript hashes an object with no hash of its own by an id it gives it, `goog.getUid`. So do functions and
/// regular expressions here.
fn next_uid() -> u32 {
    static UID: AtomicU32 = AtomicU32::new(1);
    UID.fetch_add(1, AtomicOrdering::Relaxed)
}

type NativeFn = dyn Fn(&[Value]) -> Result<Value> + Send + Sync;

/// A function value: a predicate, a query function, an aggregate, a transaction function, a listener. It is the
/// host's or Rust's.
#[derive(Clone)]
pub struct Func(Arc<FuncInner>);

struct FuncInner {
    id: u32,
    name: Box<str>,
    f: Box<NativeFn>,
    /// The host's own handle for it, when it is the host's
    host: Option<Arc<dyn Any + Send + Sync>>,
    /// Whether it is a ClojureScript type, which prints as its name
    constructor: bool,
}

impl Func {
    pub fn new<F>(name: &str, f: F) -> Func
    where
        F: Fn(&[Value]) -> Result<Value> + Send + Sync + 'static,
    {
        Func(Arc::new(FuncInner {
            id: next_uid(),
            name: Box::from(name),
            f: Box::new(f),
            host: None,
            constructor: false,
        }))
    }

    /// A ClojureScript type, as `(type x)` answers: it prints as its name, `cljs.core/Keyword`.
    pub fn constructor<F>(name: &str, f: F) -> Func
    where
        F: Fn(&[Value]) -> Result<Value> + Send + Sync + 'static,
    {
        Func(Arc::new(FuncInner {
            id: next_uid(),
            name: Box::from(name),
            f: Box::new(f),
            host: None,
            constructor: true,
        }))
    }

    pub fn is_constructor(&self) -> bool {
        self.0.constructor
    }

    /// A function of the host's: `handle` is what the host knows it by, and is handed back when the function is a
    /// value in an answer.
    pub fn host<F>(name: &str, handle: Arc<dyn Any + Send + Sync>, f: F) -> Func
    where
        F: Fn(&[Value]) -> Result<Value> + Send + Sync + 'static,
    {
        Func(Arc::new(FuncInner {
            id: next_uid(),
            name: Box::from(name),
            f: Box::new(f),
            host: Some(handle),
            constructor: false,
        }))
    }

    #[inline]
    pub fn call(&self, args: &[Value]) -> Result<Value> {
        (self.0.f)(args)
    }

    pub fn name(&self) -> &str {
        &self.0.name
    }

    pub fn host_handle(&self) -> Option<&Arc<dyn Any + Send + Sync>> {
        self.0.host.as_ref()
    }

    #[inline]
    pub fn id(&self) -> u32 {
        self.0.id
    }

    #[inline]
    pub fn ptr_eq(&self, other: &Func) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    /// A reference that does not keep the function: for a table that hands the same function out again while
    /// something still holds it.
    pub fn downgrade(&self) -> WeakFunc {
        WeakFunc(Arc::downgrade(&self.0))
    }
}

pub struct WeakFunc(std::sync::Weak<FuncInner>);

impl WeakFunc {
    pub fn upgrade(&self) -> Option<Func> {
        self.0.upgrade().map(Func)
    }
}

/// A value of the host's, opaque here. The host answers for its hash, equality, order and printing, as ClojureScript
/// would ask the value itself.
pub trait HostObject: Send + Sync + 'static {
    fn hash(&self) -> i32;
    fn equiv(&self, other: &dyn HostObject) -> bool;
    /// `(type->str (type x))`
    fn type_name(&self) -> String;
    fn pr_str(&self) -> String;
    /// How it orders against another value of its own type, when the host can say
    fn compare(&self, _other: &dyn HostObject) -> Option<Ordering> {
        None
    }
    fn as_any(&self) -> &dyn Any;
    /// Itself as a shared `Any`, for a type that is handed back out as what it is.
    fn as_arc_any(self: Arc<Self>) -> Option<Arc<dyn Any + Send + Sync>> {
        None
    }
}

#[derive(Clone)]
pub struct HostObj(pub Arc<dyn HostObject>);

impl HostObj {
    pub fn new<T: HostObject>(obj: T) -> HostObj {
        HostObj(Arc::new(obj))
    }

    pub fn ptr_eq(&self, other: &HostObj) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

// ---------------------------------------------------------------- construction

impl Value {
    #[inline]
    pub fn str(s: &str) -> Value {
        Value::Str(Arc::from(s))
    }

    /// A keyword, from `ns/name`
    #[inline]
    pub fn kw(full: &str) -> Value {
        Value::Keyword(Keyword::parse(full))
    }

    /// A symbol, from `ns/name`
    #[inline]
    pub fn sym(full: &str) -> Value {
        Value::Symbol(Symbol::parse(full))
    }

    #[inline]
    pub fn num<N: Into<f64>>(n: N) -> Value {
        Value::Num(n.into())
    }

    #[inline]
    pub fn int(n: i64) -> Value {
        Value::Num(n as f64)
    }

    #[inline]
    pub fn vector(items: Vec<Value>) -> Value {
        Value::Vector(Arc::new(Seq::new(items)))
    }

    #[inline]
    pub fn list(items: Vec<Value>) -> Value {
        Value::List(Arc::new(Seq::new(items)))
    }

    #[inline]
    pub fn map(m: CljMap) -> Value {
        Value::Map(Arc::new(m))
    }

    #[inline]
    pub fn set(s: CljSet) -> Value {
        Value::Set(Arc::new(s))
    }

    /// `(uuid s)`
    pub fn uuid(s: &str) -> Value {
        Value::Uuid(Arc::from(s.to_lowercase()))
    }

    /// `{}` with these entries added in order, each key a keyword `ns/name`.
    pub fn kw_map(pairs: &[(&str, Value)]) -> Value {
        Value::map(pairs.iter().map(|(k, v)| (Value::kw(k), v.clone())).collect())
    }

    #[inline]
    pub fn some_or_nil(v: Option<Value>) -> Value {
        v.unwrap_or(Value::Nil)
    }
}

impl From<bool> for Value {
    fn from(b: bool) -> Value {
        Value::Bool(b)
    }
}

impl From<f64> for Value {
    fn from(n: f64) -> Value {
        Value::Num(n)
    }
}

impl From<i32> for Value {
    fn from(n: i32) -> Value {
        Value::Num(n as f64)
    }
}

impl From<i64> for Value {
    fn from(n: i64) -> Value {
        Value::Num(n as f64)
    }
}

impl From<usize> for Value {
    fn from(n: usize) -> Value {
        Value::Num(n as f64)
    }
}

impl From<&str> for Value {
    fn from(s: &str) -> Value {
        Value::str(s)
    }
}

impl From<String> for Value {
    fn from(s: String) -> Value {
        Value::Str(Arc::from(s))
    }
}

impl From<Keyword> for Value {
    fn from(k: Keyword) -> Value {
        Value::Keyword(k)
    }
}

impl From<&Keyword> for Value {
    fn from(k: &Keyword) -> Value {
        Value::Keyword(k.clone())
    }
}

impl From<Symbol> for Value {
    fn from(s: Symbol) -> Value {
        Value::Symbol(s)
    }
}

impl From<Vec<Value>> for Value {
    fn from(items: Vec<Value>) -> Value {
        Value::vector(items)
    }
}

impl From<CljMap> for Value {
    fn from(m: CljMap) -> Value {
        Value::map(m)
    }
}

impl From<CljSet> for Value {
    fn from(s: CljSet) -> Value {
        Value::set(s)
    }
}

impl<T: Into<Value>> From<Option<T>> for Value {
    fn from(o: Option<T>) -> Value {
        match o {
            Some(v) => v.into(),
            None => Value::Nil,
        }
    }
}

// ---------------------------------------------------------------- what a value is

impl Value {
    #[inline]
    pub fn is_nil(&self) -> bool {
        matches!(self, Value::Nil)
    }

    #[inline]
    pub fn is_some(&self) -> bool {
        !self.is_nil()
    }

    /// ClojureScript's truth: everything but `nil` and `false`.
    #[inline]
    pub fn truthy(&self) -> bool {
        !matches!(self, Value::Nil | Value::Bool(false))
    }

    #[inline]
    pub fn is_number(&self) -> bool {
        matches!(self, Value::Num(_))
    }

    #[inline]
    pub fn is_string(&self) -> bool {
        matches!(self, Value::Str(_))
    }

    #[inline]
    pub fn is_keyword(&self) -> bool {
        matches!(self, Value::Keyword(_))
    }

    #[inline]
    pub fn is_symbol(&self) -> bool {
        matches!(self, Value::Symbol(_))
    }

    #[inline]
    pub fn is_vector(&self) -> bool {
        matches!(self, Value::Vector(_))
    }

    #[inline]
    pub fn is_map(&self) -> bool {
        matches!(self, Value::Map(_))
    }

    #[inline]
    pub fn is_set(&self) -> bool {
        matches!(self, Value::Set(_))
    }

    /// `sequential?`
    #[inline]
    pub fn is_sequential(&self) -> bool {
        matches!(self, Value::Vector(_) | Value::List(_))
    }

    /// Whether it is a collection that may hold others: what is dropped by a list and not by the call stack
    /// (`coll::drop_nested`).
    #[inline]
    pub(crate) fn nests(&self) -> bool {
        matches!(self, Value::Vector(_) | Value::List(_) | Value::Map(_) | Value::Set(_))
    }

    /// `coll?`
    #[inline]
    pub fn is_coll(&self) -> bool {
        matches!(self, Value::Vector(_) | Value::List(_) | Value::Map(_) | Value::Set(_))
    }

    /// DataScript's `seqable?`: a collection, a datom, or `nil`; not a string.
    #[inline]
    pub fn is_seqable(&self) -> bool {
        matches!(self, Value::Nil | Value::Vector(_) | Value::List(_) | Value::Map(_) | Value::Set(_) | Value::Datom(_))
    }

    #[inline]
    pub fn is_fn(&self) -> bool {
        matches!(self, Value::Fn(_))
    }

    #[inline]
    pub fn as_num(&self) -> Option<f64> {
        match self {
            Value::Num(n) => Some(*n),
            _ => None,
        }
    }

    #[inline]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    #[inline]
    pub fn as_keyword(&self) -> Option<&Keyword> {
        match self {
            Value::Keyword(k) => Some(k),
            _ => None,
        }
    }

    #[inline]
    pub fn as_symbol(&self) -> Option<&Symbol> {
        match self {
            Value::Symbol(s) => Some(s),
            _ => None,
        }
    }

    /// The elements of a vector or list.
    #[inline]
    pub fn as_seq(&self) -> Option<&[Value]> {
        match self {
            Value::Vector(s) | Value::List(s) => Some(s.items()),
            _ => None,
        }
    }

    #[inline]
    pub fn as_map(&self) -> Option<&CljMap> {
        match self {
            Value::Map(m) => Some(m),
            _ => None,
        }
    }

    #[inline]
    pub fn as_set(&self) -> Option<&CljSet> {
        match self {
            Value::Set(s) => Some(s),
            _ => None,
        }
    }

    #[inline]
    pub fn as_db(&self) -> Option<&Db> {
        match self {
            Value::Db(db) => Some(db),
            _ => None,
        }
    }

    /// Whether it is this keyword, `ns/name`.
    #[inline]
    pub fn is_kw(&self, k: &Keyword) -> bool {
        matches!(self, Value::Keyword(x) if x == k)
    }

    /// Whether it is this symbol.
    #[inline]
    pub fn is_sym(&self, name: &str) -> bool {
        matches!(self, Value::Symbol(x) if x.full() == name)
    }

    /// `(get m k)` on a map; `nil` otherwise.
    pub fn get(&self, k: &Value) -> Option<&Value> {
        match self {
            Value::Map(m) => m.get(k),
            _ => None,
        }
    }

    /// `(get m :ns/name)`
    pub fn get_kw(&self, k: &Keyword) -> Option<&Value> {
        match self {
            Value::Map(m) => m.get(&Value::Keyword(k.clone())),
            _ => None,
        }
    }

    /// `(count x)` of a collection or string; 0 of `nil`.
    pub fn count(&self) -> Option<usize> {
        match self {
            Value::Nil => Some(0),
            Value::Vector(s) | Value::List(s) => Some(s.len()),
            Value::Map(m) => Some(m.len()),
            Value::Set(s) => Some(s.len()),
            Value::Str(s) => Some(s.encode_utf16().count()),
            Value::Datom(_) => Some(5),
            _ => None,
        }
    }

    /// `(seq x)` as values: a collection's elements, a map's entries as vectors, a datom's five fields.
    pub fn seq_items(&self) -> Option<Vec<Value>> {
        match self {
            Value::Nil => Some(Vec::new()),
            Value::Vector(s) | Value::List(s) => Some(s.items().to_vec()),
            Value::Map(m) => Some(m.iter().map(|(k, v)| Value::vector(vec![k.clone(), v.clone()])).collect()),
            Value::Set(s) => Some(s.iter().cloned().collect()),
            Value::Datom(d) => Some(d.seq_values()),
            Value::Str(s) => Some(crate::print::string_chars(s)),
            _ => None,
        }
    }

    /// `(type->str (type x))`, by which DataScript orders values of different types.
    pub fn type_name(&self) -> std::borrow::Cow<'static, str> {
        use std::borrow::Cow::Borrowed as B;
        match self {
            Value::Nil => B("nil"),
            Value::Bool(_) => B("function Boolean() { [native code] }"),
            Value::Num(_) => B("function Number() { [native code] }"),
            Value::Str(_) => B("function String() { [native code] }"),
            Value::Keyword(_) => B("cljs.core/Keyword"),
            Value::Symbol(_) => B("cljs.core/Symbol"),
            Value::Vector(_) => B("cljs.core/PersistentVector"),
            Value::List(s) if s.is_empty() => B("cljs.core/EmptyList"),
            Value::List(_) => B("cljs.core/List"),
            Value::Map(m) if m.is_array() => B("cljs.core/PersistentArrayMap"),
            Value::Map(_) => B("cljs.core/PersistentHashMap"),
            Value::Set(_) => B("cljs.core/PersistentHashSet"),
            Value::Uuid(_) => B("cljs.core/UUID"),
            Value::Inst(_) => B("function Date() { [native code] }"),
            Value::Regex(_) => B("function RegExp() { [native code] }"),
            Value::Datom(_) => B("datascript.db/Datom"),
            Value::Db(db) if db.is_filtered() => B("datascript.db/FilteredDB"),
            Value::Db(_) => B("datascript.db/DB"),
            Value::Fn(_) => B("function Function() { [native code] }"),
            Value::Host(h) => std::borrow::Cow::Owned(h.0.type_name()),
        }
    }

    /// `(hash x)`
    pub fn cljs_hash(&self) -> i32 {
        match self {
            Value::Nil => 0,
            Value::Bool(true) => 1231,
            Value::Bool(false) => 1237,
            Value::Num(n) => hash::hash_number(*n),
            Value::Str(s) => hash::hash_string(s),
            Value::Keyword(k) => k.hash(),
            Value::Symbol(s) => s.hash(),
            Value::Vector(s) | Value::List(s) => s.cljs_hash(),
            Value::Map(m) => m.cljs_hash(),
            Value::Set(s) => s.cljs_hash(),
            Value::Uuid(s) => hash::hash_string(s),
            Value::Inst(ms) => hash::to_int32(*ms),
            Value::Regex(r) => r.id as i32,
            Value::Datom(d) => d.cljs_hash(),
            Value::Db(db) => db.cljs_hash(),
            Value::Fn(f) => f.id() as i32,
            Value::Host(h) => h.0.hash(),
        }
    }
}

/// ClojureScript's `=`.
impl PartialEq for Value {
    fn eq(&self, other: &Value) -> bool {
        match (self, other) {
            (Value::Nil, Value::Nil) => true,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Num(a), Value::Num(b)) => a == b,
            (Value::Str(a), Value::Str(b)) => Arc::ptr_eq(a, b) || a == b,
            (Value::Keyword(a), Value::Keyword(b)) => a == b,
            (Value::Symbol(a), Value::Symbol(b)) => a == b,
            // a vector equals a list of the same elements
            (Value::Vector(a) | Value::List(a), Value::Vector(b) | Value::List(b)) => {
                Arc::ptr_eq(a, b) || a.items() == b.items()
            }
            (Value::Map(a), Value::Map(b)) => Arc::ptr_eq(a, b) || a == b,
            (Value::Set(a), Value::Set(b)) => Arc::ptr_eq(a, b) || a == b,
            (Value::Uuid(a), Value::Uuid(b)) => a == b,
            (Value::Inst(a), Value::Inst(b)) => a == b,
            (Value::Regex(a), Value::Regex(b)) => Arc::ptr_eq(a, b),
            (Value::Datom(a), Value::Datom(b)) => a.equiv(b),
            (Value::Db(a), Value::Db(b)) => a.equiv(b),
            (Value::Fn(a), Value::Fn(b)) => a.ptr_eq(b),
            (Value::Host(a), Value::Host(b)) => a.ptr_eq(b) || a.0.equiv(&*b.0),
            _ => false,
        }
    }
}

/// As in ClojureScript, `NaN` equals nothing, itself included: a map never finds a `NaN` key again.
impl Eq for Value {}

impl std::hash::Hash for Value {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        state.write_i32(self.cljs_hash())
    }
}

impl std::fmt::Debug for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&crate::print::pr_str(self))
    }
}

impl std::fmt::Display for Value {
    /// `pr-str`
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&crate::print::pr_str(self))
    }
}

/// Builds a vector value: `vector![a, b]`.
#[macro_export]
macro_rules! vector {
    ($($x:expr),* $(,)?) => {
        $crate::Value::vector(vec![$($crate::Value::from($x)),*])
    };
}
