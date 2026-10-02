//! Keywords, symbols and attributes. Each is interned: two with the same name are the same allocation, so equality is
//! a pointer comparison, and the ClojureScript hash is computed once. A name is kept for as long as the program runs,
//! as ClojureScript keeps its keywords, so a keyword is a pointer that is copied, and copying a datom counts nothing.

use crate::hash;
use crate::lock::{Lazy, Lock};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Keyword,
    Symbol,
    /// A string used as an attribute, as DataScript's JavaScript API names attributes
    Str,
}

pub struct Named {
    kind: Kind,
    /// Its number among the names of its kind, in the order they were first met
    id: u32,
    ns: Option<Box<str>>,
    name: Box<str>,
    /// `ns/name`, or the name alone
    full: Arc<str>,
    hash: i32,
}

/// The names of one kind: by `ns/name`, and by number.
#[derive(Default)]
struct Names {
    by_name: HashMap<Arc<str>, &'static Named>,
    by_id: Vec<&'static Named>,
}

type Table = Lock<Names>;

fn table(kind: Kind) -> &'static Table {
    static KEYWORDS: Lazy<Table> = Lazy::new();
    static SYMBOLS: Lazy<Table> = Lazy::new();
    static STRINGS: Lazy<Table> = Lazy::new();
    match kind {
        Kind::Keyword => KEYWORDS.get_or_init(|| Lock::new(Names::default())),
        Kind::Symbol => SYMBOLS.get_or_init(|| Lock::new(Names::default())),
        Kind::Str => STRINGS.get_or_init(|| Lock::new(Names::default())),
    }
}

/// A name's namespace and name, as ClojureScript's reader splits `ns/name`: at the first slash, and `/` alone is a
/// name.
fn split(full: &str) -> (Option<&str>, &str) {
    if full == "/" {
        return (None, full);
    }
    match full.find('/') {
        Some(i) if i > 0 && i + 1 < full.len() => (Some(&full[..i]), &full[i + 1..]),
        _ => (None, full),
    }
}

fn intern(kind: Kind, full: &str) -> &'static Named {
    let mut t = table(kind).lock();
    if let Some(n) = t.by_name.get(full) {
        return n;
    }
    let (ns, name) = match kind {
        Kind::Str => (None, full),
        _ => split(full),
    };
    let hash = match kind {
        Kind::Keyword => hash::hash_keyword(ns, name),
        Kind::Symbol => hash::hash_symbol(ns, name),
        Kind::Str => hash::hash_string(full),
    };
    let full: Arc<str> = Arc::from(full);
    let id = t.by_id.len() as u32;
    let named: &'static Named =
        Box::leak(Box::new(Named { kind, id, ns: ns.map(Box::from), name: Box::from(name), full: full.clone(), hash }));
    // by number first: should the call be cut short between the two, the name is numbered again the next time
    // it is met, and no number is ever two names'
    t.by_id.push(named);
    t.by_name.insert(full, named);
    named
}

fn by_id(kind: Kind, id: u32) -> Option<&'static Named> {
    table(kind).lock().by_id.get(id as usize).copied()
}

fn intern_parts(kind: Kind, ns: Option<&str>, name: &str) -> &'static Named {
    match ns {
        Some(ns) if !ns.is_empty() => {
            let mut full = String::with_capacity(ns.len() + 1 + name.len());
            full.push_str(ns);
            full.push('/');
            full.push_str(name);
            intern(kind, &full)
        }
        // Names are interned by `ns/name`, as ClojureScript compares keywords by it
        _ => intern(kind, name),
    }
}

/// JavaScript's string order (`goog.array.defaultCompare` on strings): by UTF-16 code units. It differs from the
/// order of UTF-8 bytes only where a character above U+FFFF meets one in U+E000..U+FFFF.
pub fn compare_str(a: &str, b: &str) -> Ordering {
    let (x, y) = (a.as_bytes(), b.as_bytes());
    match x.iter().zip(y).position(|(p, q)| p != q) {
        None => x.len().cmp(&y.len()),
        Some(i) => {
            // Where the two first differ, both are at the same place in a character. A lead byte from 0xF0 is a
            // character above U+FFFF, which UTF-16 writes as a pair of surrogates, below U+E000; 0xEE and 0xEF lead
            // the characters from U+E000 to U+FFFF.
            let (p, q) = (x[i], y[i]);
            if p >= 0xF0 && (q == 0xEE || q == 0xEF) {
                Ordering::Less
            } else if q >= 0xF0 && (p == 0xEE || p == 0xEF) {
                Ordering::Greater
            } else {
                p.cmp(&q)
            }
        }
    }
}

/// `compare-keywords` and `compare-symbols`: a name without a namespace first, then by namespace, then by name.
fn compare_named(a: &Named, b: &Named) -> Ordering {
    match (&a.ns, &b.ns) {
        (None, Some(_)) => Ordering::Less,
        (Some(_), None) => Ordering::Greater,
        (Some(x), Some(y)) => compare_str(x, y).then_with(|| compare_str(&a.name, &b.name)),
        (None, None) => compare_str(&a.name, &b.name),
    }
}

macro_rules! named_type {
    ($(#[$meta:meta])* $ty:ident, $kind:expr) => {
        $(#[$meta])*
        #[derive(Clone, Copy)]
        pub struct $ty(pub(crate) &'static Named);

        impl $ty {
            /// From `ns/name`, or a name alone.
            pub fn parse(full: &str) -> Self {
                $ty(intern($kind, full))
            }

            pub fn new(ns: Option<&str>, name: &str) -> Self {
                $ty(intern_parts($kind, ns, name))
            }

            #[inline]
            pub fn ns(&self) -> Option<&str> {
                self.0.ns.as_deref()
            }

            #[inline]
            pub fn name(&self) -> &str {
                &self.0.name
            }

            /// `ns/name`, or the name alone
            #[inline]
            pub fn full(&self) -> &str {
                &self.0.full
            }

            /// `ns/name`, which is kept for as long as the program runs
            #[inline]
            pub fn full_static(&self) -> &'static str {
                &self.0.full
            }

            /// ClojureScript's hash of it
            #[inline]
            pub fn hash(&self) -> i32 {
                self.0.hash
            }

            #[inline]
            pub fn ptr(&self) -> usize {
                self.0 as *const Named as usize
            }

            /// Its number: the names of a kind are numbered from 0 in the order they are first met, and keep
            /// their numbers. What knows the number knows the name without spelling it.
            #[inline]
            pub fn id(&self) -> u32 {
                self.0.id
            }

            /// The name of a number, if one has it.
            pub fn from_id(id: u32) -> Option<Self> {
                by_id($kind, id).map($ty)
            }
        }

        impl PartialEq for $ty {
            #[inline]
            fn eq(&self, other: &Self) -> bool {
                std::ptr::eq(self.0, other.0)
            }
        }

        impl Eq for $ty {}

        impl std::hash::Hash for $ty {
            fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
                state.write_i32(self.0.hash)
            }
        }

        impl PartialOrd for $ty {
            fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
                Some(self.cmp(other))
            }
        }

        impl Ord for $ty {
            fn cmp(&self, other: &Self) -> Ordering {
                if std::ptr::eq(self.0, other.0) {
                    Ordering::Equal
                } else {
                    compare_named(&self.0, &other.0)
                }
            }
        }
    };
}

named_type!(
    /// A ClojureScript keyword, `:ns/name`.
    Keyword,
    Kind::Keyword
);
named_type!(
    /// A ClojureScript symbol, `ns/name`.
    Symbol,
    Kind::Symbol
);

impl fmt::Display for Keyword {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, ":{}", self.full())
    }
}

impl fmt::Debug for Keyword {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, ":{}", self.full())
    }
}

impl fmt::Display for Symbol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.full())
    }
}

impl fmt::Debug for Symbol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.full())
    }
}

/// An attribute: a keyword, as ClojureScript names attributes, or a string, as DataScript's JavaScript API does.
#[derive(Clone, Copy)]
pub struct Attr(pub(crate) &'static Named);

impl Attr {
    pub fn keyword(k: &Keyword) -> Attr {
        Attr(k.0)
    }

    /// A keyword attribute, from `ns/name`.
    pub fn kw(full: &str) -> Attr {
        Attr(intern(Kind::Keyword, full))
    }

    pub fn string(s: &str) -> Attr {
        Attr(intern(Kind::Str, s))
    }

    #[inline]
    pub fn is_keyword(&self) -> bool {
        self.0.kind == Kind::Keyword
    }

    #[inline]
    pub fn as_keyword(&self) -> Option<Keyword> {
        if self.is_keyword() {
            Some(Keyword(self.0))
        } else {
            None
        }
    }

    #[inline]
    pub fn ns(&self) -> Option<&str> {
        self.0.ns.as_deref()
    }

    #[inline]
    pub fn name(&self) -> &str {
        &self.0.name
    }

    /// The keyword's `ns/name`, or the string
    #[inline]
    pub fn full(&self) -> &str {
        &self.0.full
    }

    #[inline]
    pub(crate) fn full_arc(&self) -> &Arc<str> {
        &self.0.full
    }

    #[inline]
    pub fn hash(&self) -> i32 {
        self.0.hash
    }

    #[inline]
    pub fn ptr(&self) -> usize {
        self.0 as *const Named as usize
    }
}

impl PartialEq for Attr {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self.0, other.0)
    }
}

impl Eq for Attr {}

impl std::hash::Hash for Attr {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        state.write_usize(self.ptr())
    }
}

impl PartialOrd for Attr {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// DataScript orders attributes with `compare`, which holds between keywords and between strings. Between a keyword
/// and a string ClojureScript's `compare` throws; here keywords come first, so that an index stays a total order.
impl Ord for Attr {
    #[inline]
    fn cmp(&self, other: &Self) -> Ordering {
        if std::ptr::eq(self.0, other.0) {
            return Ordering::Equal;
        }
        match (self.0.kind, other.0.kind) {
            (Kind::Str, Kind::Str) => compare_str(&self.0.full, &other.0.full),
            (Kind::Str, _) => Ordering::Greater,
            (_, Kind::Str) => Ordering::Less,
            _ => compare_named(self.0, other.0),
        }
    }
}

impl fmt::Display for Attr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_keyword() {
            write!(f, ":{}", self.full())
        } else {
            write!(f, "{:?}", self.full())
        }
    }
}

impl fmt::Debug for Attr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interned() {
        let a = Keyword::parse("db/id");
        let b = Keyword::new(Some("db"), "id");
        assert!(a == b);
        assert_eq!(a.ns(), Some("db"));
        assert_eq!(a.name(), "id");
        assert_eq!(a.to_string(), ":db/id");
        assert_eq!(Keyword::parse("/").name(), "/");
        assert_eq!(Keyword::parse("a").ns(), None);
        assert!(Symbol::parse("?e") == Symbol::parse("?e"));
        assert_eq!(Keyword::parse("a/b").hash(), 1482224565);
        assert_eq!(Symbol::parse("foo/bar").hash(), 254379989);
    }

    #[test]
    fn order() {
        let k = Keyword::parse;
        assert!(k("a") < k("b"));
        assert!(k("z") < k("a/a")); // no namespace first
        assert!(k("a/z") < k("b/a"));
        assert!(k("a/a") < k("a/b"));
        assert!(Attr::kw("z") < Attr::string("a"));
    }

    #[test]
    fn utf16_order() {
        // U+FFFF sorts after U+10000 in UTF-16 code units, before it in code points
        assert_eq!(compare_str("\u{ffff}", "\u{10000}"), Ordering::Greater);
        assert_eq!(compare_str("a", "b"), Ordering::Less);
    }
}
