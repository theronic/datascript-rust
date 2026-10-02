//! DataScript, the immutable in-memory database and Datalog query engine, ported from ClojureScript.
//!
//! The port follows ClojureScript DataScript function for function, and keeps its semantics: the same answers, in
//! the same order, with the same errors. What ClojureScript's runtime gives DataScript for free is here too, as far
//! as it shows in an answer: its values, their equality and hashes (`value`, `hash`), the order of its maps and sets
//! (`coll`), its printing and reading (`print`, `edn`), and its comparisons (`cmp`).

pub mod built_ins;
pub mod clj;
pub mod cmp;
pub mod coll;
pub mod conn;
pub mod datom;
pub mod db;
pub mod edn;
pub mod entity;
pub mod error;
pub mod hash;
pub mod lru;
pub mod named;
pub mod parser;
pub mod print;
pub mod pull_api;
pub mod pull_parser;
pub mod query;
pub mod record;
pub mod regex;
pub mod schema;
pub mod sorted_set;
pub mod transact;
pub mod value;

pub use coll::{CljMap, CljSet};
pub use conn::Conn;
pub use datom::{Datom, Index, E0, EMAX, TX0, TXMAX};
pub use db::Db;
pub use entity::{entity, Entity};
pub use error::{Error, Result};
pub use named::{Attr, Keyword, Symbol};
pub use pull_api::{pull, pull_many};
pub use query::q;
pub use transact::{db_with, with, TxReport};
pub use value::Value;
