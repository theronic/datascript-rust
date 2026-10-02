//! The functions a case names with `#f`: predicates, query functions, aggregates, transaction functions and
//! filters, written here as conformance/oracle/src/oracle/core.cljs writes them.

use datascript::Value;

pub fn lookup(_name: &str) -> Option<Value> {
    None
}
