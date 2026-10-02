//! Order: ClojureScript's `compare`, and DataScript's `value-compare`, which orders the values in its indexes and
//! any two values a query compares with `<`.

use crate::error::{Error, Result};
use crate::named::compare_str;
use crate::print::str_of;
use crate::value::Value;
use std::cmp::Ordering;

/// `goog.array.defaultCompare` on numbers: `NaN` is neither less nor greater than anything.
#[inline]
pub fn compare_num(a: f64, b: f64) -> Ordering {
    if a > b {
        Ordering::Greater
    } else if a < b {
        Ordering::Less
    } else {
        Ordering::Equal
    }
}

/// DataScript's `seq-compare`: the shorter first; of equal lengths, by element, `nil` lowest.
fn seq_compare(xs: &[Value], ys: &[Value]) -> Ordering {
    match xs.len().cmp(&ys.len()) {
        Ordering::Equal => {}
        other => return other,
    }
    for (x, y) in xs.iter().zip(ys) {
        let c = match (x.is_nil(), y.is_nil()) {
            (true, true) => Ordering::Equal,
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
            (false, false) => value_compare(x, y),
        };
        if c != Ordering::Equal {
            return c;
        }
    }
    Ordering::Equal
}

/// DataScript's `value-compare`, of two values that are not `nil`.
///
/// Equal values are equal; two sequential collections compare by length and then by element; values of one type
/// compare as ClojureScript compares them, or, where it does not, by their hashes; values of different types compare
/// by the names of their types.
pub fn value_compare(x: &Value, y: &Value) -> Ordering {
    match (x, y) {
        (Value::Num(a), Value::Num(b)) => compare_num(*a, *b),
        (Value::Str(a), Value::Str(b)) => compare_str(a, b),
        (Value::Keyword(a), Value::Keyword(b)) => a.cmp(b),
        (Value::Vector(a) | Value::List(a), Value::Vector(b) | Value::List(b)) => seq_compare(a, b),
        (Value::Bool(a), Value::Bool(b)) => a.cmp(b),
        (Value::Symbol(a), Value::Symbol(b)) => a.cmp(b),
        (Value::Uuid(a), Value::Uuid(b)) => compare_str(a, b),
        (Value::Inst(a), Value::Inst(b)) => compare_num(*a, *b),
        _ => {
            if x == y {
                return Ordering::Equal;
            }
            let (tx, ty) = (x.type_name(), y.type_name());
            if tx != ty {
                return compare_str(&tx, &ty);
            }
            if let (Value::Host(a), Value::Host(b)) = (x, y) {
                if let Some(c) = a.0.compare(&*b.0) {
                    return c;
                }
            }
            x.cljs_hash().cmp(&y.cljs_hash())
        }
    }
}

/// `value-compare` as a query's `<` calls it, where an argument may be `nil`: ClojureScript then fails on the type
/// of `nil`.
pub fn value_compare_checked(x: &Value, y: &Value) -> Result<Ordering> {
    if x.is_nil() != y.is_nil() {
        return Err(Error::msg("Cannot read properties of null (reading 'cljs$lang$ctorStr')"));
    }
    Ok(value_compare(x, y))
}

/// `value-cmp`: `nil` on either side is equal, which is how a search bound leaves a component open.
#[inline]
pub fn value_cmp(x: &Value, y: &Value) -> Ordering {
    if x.is_nil() || y.is_nil() {
        Ordering::Equal
    } else {
        value_compare(x, y)
    }
}

fn cannot_compare(x: &Value, y: &Value) -> Error {
    Error::msg(format!("Cannot compare {} to {}", str_of(x), str_of(y)))
}

/// `cljs.core/compare`: `nil` first; numbers, strings and booleans among themselves; keywords, symbols, UUIDs,
/// instants and vectors among themselves; anything else cannot be compared.
pub fn compare(x: &Value, y: &Value) -> Result<Ordering> {
    match (x, y) {
        (Value::Nil, Value::Nil) => Ok(Ordering::Equal),
        (Value::Nil, _) => Ok(Ordering::Less),
        (_, Value::Nil) => Ok(Ordering::Greater),
        (Value::Num(a), Value::Num(b)) => Ok(compare_num(*a, *b)),
        (Value::Str(a), Value::Str(b)) => Ok(compare_str(a, b)),
        (Value::Bool(a), Value::Bool(b)) => Ok(a.cmp(b)),
        (Value::Keyword(a), Value::Keyword(b)) => Ok(a.cmp(b)),
        (Value::Symbol(a), Value::Symbol(b)) => Ok(a.cmp(b)),
        (Value::Uuid(a), Value::Uuid(b)) => Ok(compare_str(a, b)),
        (Value::Inst(a), Value::Inst(b)) => Ok(compare_num(*a, *b)),
        (Value::Vector(a), Value::Vector(b)) => {
            match a.len().cmp(&b.len()) {
                Ordering::Equal => {}
                other => return Ok(other),
            }
            for (p, q) in a.iter().zip(b.iter()) {
                let c = compare(p, q)?;
                if c != Ordering::Equal {
                    return Ok(c);
                }
            }
            Ok(Ordering::Equal)
        }
        _ => Err(cannot_compare(x, y)),
    }
}

/// An ordering as the number ClojureScript's comparators answer with.
#[inline]
pub fn ordering_num(o: Ordering) -> f64 {
    match o {
        Ordering::Less => -1.0,
        Ordering::Equal => 0.0,
        Ordering::Greater => 1.0,
    }
}
