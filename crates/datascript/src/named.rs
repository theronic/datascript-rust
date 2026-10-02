//! Keywords, symbols and attributes. Each is interned: two with the same name are the same allocation, so equality is
//! a pointer comparison, and the ClojureScript hash is computed once.

use crate::hash;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex, OnceLock};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Keyword,
    Symbol,
    /// A string used as an attribute, as DataScript's JavaScript API names attributes
    Str,
}

pub struct Named {
    kind: Kind,
    ns: Option<Box<str>>,
    name: Box<str>,
    /// `ns/name`, or the name alone
    full: Arc<str>,
    hash: i32,
}

type Table = Mutex<HashMap<Arc<str>, Arc<Named>>>;

fn table(kind: Kind) -> &'static Table {
    static KEYWORDS: OnceLock<Table> = OnceLock::new();
    static SYMBOLS: OnceLock<Table> = OnceLock::new();
    static STRINGS: OnceLock<Table> = OnceLock::new();
    match kind {
        Kind::Keyword => KEYWORDS.get_or_init(Default::default),
        Kind::Symbol => SYMBOLS.get_or_init(Default::default),
        Kind::Str => STRINGS.get_or_init(Default::default),
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

fn intern(kind: Kind, full: &str) -> Arc<Named> {
    let mut t = table(kind).lock().unwrap_or_else(|e| e.into_inner());
    if let Some(n) = t.get(full) {
        return n.clone();
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
    let named = Arc::new(Named { kind, ns: ns.map(Box::from), name: Box::from(name), full: full.clone(), hash });
    t.insert(full, named.clone());
    named
}

fn intern_parts(kind: Kind, ns: Option<&str>, name: &str) -> Arc<Named> {
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
    if a.is_ascii() && b.is_ascii() {
        return a.cmp(b);
    }
    a.encode_utf16().cmp(b.encode_utf16())
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
        #[derive(Clone)]
        pub struct $ty(pub(crate) Arc<Named>);

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

            /// ClojureScript's hash of it
            #[inline]
            pub fn hash(&self) -> i32 {
                self.0.hash
            }

            #[inline]
            pub fn ptr(&self) -> usize {
                Arc::as_ptr(&self.0) as usize
            }
        }

        impl PartialEq for $ty {
            #[inline]
            fn eq(&self, other: &Self) -> bool {
                Arc::ptr_eq(&self.0, &other.0)
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
                if Arc::ptr_eq(&self.0, &other.0) {
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
#[derive(Clone)]
pub struct Attr(pub(crate) Arc<Named>);

impl Attr {
    pub fn keyword(k: &Keyword) -> Attr {
        Attr(k.0.clone())
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
            Some(Keyword(self.0.clone()))
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
        Arc::as_ptr(&self.0) as usize
    }
}

impl PartialEq for Attr {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
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
        if Arc::ptr_eq(&self.0, &other.0) {
            return Ordering::Equal;
        }
        match (self.0.kind, other.0.kind) {
            (Kind::Str, Kind::Str) => compare_str(&self.0.full, &other.0.full),
            (Kind::Str, _) => Ordering::Greater,
            (_, Kind::Str) => Ordering::Less,
            _ => compare_named(&self.0, &other.0),
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
