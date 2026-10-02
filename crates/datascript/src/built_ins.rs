//! `datascript.built-ins`: the functions a query calls by name, and its aggregates. Each behaves as ClojureScript's
//! does on ClojureScript's values, numbers being JavaScript's.

use crate::clj;
use crate::cmp::{compare, ordering_num, value_compare_checked};
use crate::coll::CljMap;
use crate::datom::value_attr;
use crate::db::{entid, search};
use crate::entity::entity;
use crate::error::{Error, Result};
use crate::named::{compare_str, Keyword, Symbol};
use crate::print::{pr_str, pr_str_all, print_str_all, str_of};
use crate::value::{Func, Value};
use crate::{message, raise};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::sync::OnceLock;

type BuiltIn = fn(&[Value]) -> Result<Value>;

static NIL: Value = Value::Nil;

fn arg(args: &[Value], i: usize) -> &Value {
    args.get(i).unwrap_or(&NIL)
}

// ---------------------------------------------------------------- JavaScript's arithmetic

/// JavaScript's `ToNumber`, of a ClojureScript value.
pub fn js_number(v: &Value) -> f64 {
    match v {
        Value::Nil => 0.0,
        Value::Bool(b) => *b as u8 as f64,
        Value::Num(n) => *n,
        Value::Str(s) => {
            let t = s.trim();
            if t.is_empty() {
                0.0
            } else if let Some(hex) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
                u64::from_str_radix(hex, 16).map_or(f64::NAN, |n| n as f64)
            } else if matches!(t, "Infinity" | "+Infinity") {
                f64::INFINITY
            } else if t == "-Infinity" {
                f64::NEG_INFINITY
            } else if t.bytes().all(|c| c.is_ascii_digit() || matches!(c, b'.' | b'e' | b'E' | b'+' | b'-')) {
                t.parse().unwrap_or(f64::NAN)
            } else {
                f64::NAN
            }
        }
        Value::Inst(ms) => *ms,
        _ => f64::NAN,
    }
}

/// JavaScript's `ToString`, as `+` sees an operand.
fn js_string(v: &Value) -> String {
    match v {
        Value::Nil => "null".into(),
        _ => str_of(v),
    }
}

/// JavaScript's `a + b`: numbers add; with a string, or anything that is neither number, boolean nor `nil`, both
/// are joined as strings.
fn js_add(a: &Value, b: &Value) -> Value {
    let numeric = |v: &Value| matches!(v, Value::Nil | Value::Bool(_) | Value::Num(_));
    if numeric(a) && numeric(b) {
        Value::Num(js_number(a) + js_number(b))
    } else {
        Value::from(format!("{}{}", js_string(a), js_string(b)))
    }
}

/// JavaScript's `ToPrimitive` for a comparison: a string, or a number. An object that is no date is its string.
fn js_primitive(v: &Value) -> std::result::Result<f64, String> {
    match v {
        Value::Nil | Value::Bool(_) | Value::Num(_) | Value::Inst(_) => Ok(js_number(v)),
        Value::Str(s) => Err(s.to_string()),
        other => Err(str_of(other)),
    }
}

/// JavaScript's `a > b`: two strings by their code units, anything else as numbers.
fn js_gt(a: &Value, b: &Value) -> bool {
    match (js_primitive(a), js_primitive(b)) {
        (Err(x), Err(y)) => compare_str(&x, &y) == Ordering::Greater,
        (x, y) => {
            let num = |p: std::result::Result<f64, String>| p.unwrap_or_else(|s| js_number(&Value::from(s)));
            num(x) > num(y)
        }
    }
}

/// JavaScript's `isNaN`: whether a value is no number once made one.
fn js_is_nan(v: &Value) -> bool {
    match js_primitive(v) {
        Ok(n) => n.is_nan(),
        Err(s) => js_number(&Value::from(s)).is_nan(),
    }
}

/// `max` and `min` of two as ClojureScript's functions answer: what is no number is the answer, the first before
/// the second; otherwise the one JavaScript's `>` or `<` picks, the second when it picks neither.
fn extreme_of_two(x: &Value, y: &Value, max: bool) -> Value {
    if js_is_nan(x) {
        x.clone()
    } else if js_is_nan(y) {
        y.clone()
    } else if (max && js_gt(x, y)) || (!max && js_gt(y, x)) {
        x.clone()
    } else {
        y.clone()
    }
}

/// `max` and `min`: of three and more, the first two go through the comparison alone.
fn extreme_of(args: &[Value], max: bool) -> Value {
    let pick = |x: &Value, y: &Value| if (max && js_gt(x, y)) || (!max && js_gt(y, x)) { x.clone() } else { y.clone() };
    match args {
        [] => Value::Nil,
        [x] => x.clone(),
        [x, y] => extreme_of_two(x, y, max),
        [x, y, more @ ..] => more.iter().fold(pick(x, y), |acc, z| extreme_of_two(&acc, z, max)),
    }
}

/// What ClojureScript throws when a function of several arities is called with none of them.
fn arity(n: usize) -> Error {
    Error::msg(format!("Invalid arity: {n}"))
}

fn num(n: f64) -> Value {
    Value::Num(n)
}

/// An argument as a number. One that was not passed is JavaScript's `undefined`, which is no number.
fn num_arg(args: &[Value], i: usize) -> f64 {
    args.get(i).map_or(f64::NAN, js_number)
}

/// `fix`: towards zero
fn fix(q: f64) -> f64 {
    if q >= 0.0 {
        q.floor()
    } else {
        q.ceil()
    }
}

/// `js-mod`: JavaScript's `%`
fn js_mod(n: f64, d: f64) -> f64 {
    n % d
}

fn quot(n: f64, d: f64) -> f64 {
    let rem = js_mod(n, d);
    fix((n - rem) / d)
}

/// `integer?`: a number that `parseInt` reads back whole, which past 1e21, where JavaScript writes numbers with an
/// exponent, it does not.
fn is_integer(v: &Value) -> bool {
    matches!(v, Value::Num(n) if n.is_finite() && n.fract() == 0.0 && n.abs() < 1e21)
}

fn reduce_arith(args: &[Value], unit: Value, f: impl Fn(&Value, &Value) -> Value) -> Value {
    match args {
        [] => unit,
        [x] => x.clone(),
        [x, rest @ ..] => rest.iter().fold(x.clone(), |acc, y| f(&acc, y)),
    }
}

fn add(args: &[Value]) -> Result<Value> {
    Ok(reduce_arith(args, num(0.0), js_add))
}

fn sub(args: &[Value]) -> Result<Value> {
    Ok(match args {
        // of no arguments JavaScript subtracts undefined from undefined
        [] => num(f64::NAN),
        [x] => num(-js_number(x)),
        _ => reduce_arith(args, num(0.0), |a, b| num(js_number(a) - js_number(b))),
    })
}

fn mul(args: &[Value]) -> Result<Value> {
    Ok(reduce_arith(args, num(1.0), |a, b| num(js_number(a) * js_number(b))))
}

fn div(args: &[Value]) -> Result<Value> {
    Ok(match args {
        [] => num(f64::NAN),
        [x] => num(1.0 / js_number(x)),
        _ => reduce_arith(args, num(1.0), |a, b| num(js_number(a) / js_number(b))),
    })
}

// ---------------------------------------------------------------- randomness

static RANDOM_STATE: AtomicU64 = AtomicU64::new(0x9E37_79B9_7F4A_7C15);

/// Seeds the generator `rand`, `rand-int`, `sample` and `squuid` draw from.
pub fn seed_random(seed: u64) {
    RANDOM_STATE.store(seed | 1, AtomicOrdering::Relaxed);
}

/// `Math.random`: a number in `[0, 1)`.
pub fn random() -> f64 {
    // xorshift64*
    let mut x = RANDOM_STATE.load(AtomicOrdering::Relaxed);
    x ^= x >> 12;
    x ^= x << 25;
    x ^= x >> 27;
    RANDOM_STATE.store(x, AtomicOrdering::Relaxed);
    let bits = x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11;
    bits as f64 / (1u64 << 53) as f64
}

fn rand_int(n: f64) -> f64 {
    (random() * n).floor()
}

// ---------------------------------------------------------------- comparisons

/// DataScript's `<`, `>`, `<=`, `>=`: `value-compare` between each neighbouring pair.
fn chain(args: &[Value], holds: impl Fn(Ordering) -> bool) -> Result<Value> {
    if args.is_empty() {
        // no arguments are two that are undefined, and equal
        return Ok(Value::Bool(holds(Ordering::Equal)));
    }
    for pair in args.windows(2) {
        if !holds(value_compare_checked(&pair[0], &pair[1])?) {
            return Ok(Value::Bool(false));
        }
    }
    Ok(Value::Bool(true))
}

fn eq(args: &[Value]) -> Result<Value> {
    Ok(Value::Bool(args.windows(2).all(|p| p[0] == p[1])))
}

fn not_eq(args: &[Value]) -> Result<Value> {
    Ok(Value::Bool(!args.windows(2).all(|p| p[0] == p[1])))
}

// ---------------------------------------------------------------- strings and regular expressions

fn utf16(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

/// JavaScript's `String.prototype.substring`
fn substring(s: &str, start: f64, end: Option<f64>) -> String {
    let units = utf16(s);
    let len = units.len() as f64;
    let clamp = |x: f64| if x.is_nan() { 0.0 } else { x.max(0.0).min(len) };
    let (mut a, mut b) = (clamp(start), end.map_or(len, clamp));
    if a > b {
        std::mem::swap(&mut a, &mut b);
    }
    String::from_utf16_lossy(&units[a as usize..b as usize])
}

fn string_arg<'a>(args: &'a [Value], i: usize, what: &str) -> Result<&'a str> {
    match arg(args, i) {
        Value::Str(s) => Ok(s),
        _ => Err(Error::msg(format!("{what} must match against a string."))),
    }
}

/// `re-pattern`: a regular expression from its source; `(?i)` and the like in front are its flags.
fn re_pattern(args: &[Value]) -> Result<Value> {
    match arg(args, 0) {
        Value::Regex(_) => Ok(arg(args, 0).clone()),
        Value::Str(s) => {
            let (flags, pattern) = match s.strip_prefix("(?").and_then(|rest| rest.split_once(')')) {
                Some((flags, pattern)) if flags.bytes().all(|c| b"idmsux".contains(&c)) => (flags, pattern),
                _ => ("", &**s),
            };
            let re = crate::value::Regex::new(pattern, flags);
            // as `new RegExp` does, a pattern that is none is refused when it is made
            crate::regex::validate(&re)?;
            Ok(Value::Regex(std::sync::Arc::new(re)))
        }
        _ => Err(Error::msg("re-find must match against a string.")),
    }
}

fn regex_arg(args: &[Value], i: usize) -> Result<&crate::value::Regex> {
    match arg(args, i) {
        Value::Regex(r) => Ok(r),
        other => Err(Error::msg(format!("{} is not a regular expression", pr_str(other)))),
    }
}

/// A match as ClojureScript gives it: the matched string, or with groups a vector of it and them.
fn match_value(m: &crate::regex::Match) -> Value {
    if m.groups.len() == 1 {
        Value::from(m.groups[0].clone().unwrap_or_default())
    } else {
        Value::vector(m.groups.iter().map(|g| g.clone().map_or(Value::Nil, Value::from)).collect())
    }
}

fn re_find(args: &[Value]) -> Result<Value> {
    let re = regex_arg(args, 0)?;
    let s = string_arg(args, 1, "re-find")?;
    Ok(crate::regex::exec(re, s)?.map_or(Value::Nil, |m| match_value(&m)))
}

fn re_matches(args: &[Value]) -> Result<Value> {
    let re = regex_arg(args, 0)?;
    let s = string_arg(args, 1, "re-matches")?;
    Ok(match crate::regex::exec(re, s)? {
        Some(m) if m.groups[0].as_deref() == Some(s) => match_value(&m),
        _ => Value::Nil,
    })
}

fn re_seq(args: &[Value]) -> Result<Value> {
    let re = regex_arg(args, 0)?;
    let s = string_arg(args, 1, "re-seq")?;
    let units = utf16(s);
    let mut out = Vec::new();
    let mut from = 0usize;
    loop {
        let rest = String::from_utf16_lossy(&units[from..]);
        let Some(m) = crate::regex::exec(re, &rest)? else { break };
        out.push(match_value(&m));
        let matched = m.groups[0].as_deref().map_or(0, |g| g.encode_utf16().count());
        let post = from + m.index + matched.max(1);
        if post > units.len() {
            break;
        }
        from = post;
    }
    Ok(if out.is_empty() { Value::Nil } else { Value::list(out) })
}

/// The string a method of JavaScript's strings is called on: the first argument, which must be one.
fn js_method_string<'a>(args: &'a [Value], method: &str) -> Result<&'a str> {
    match arg(args, 0) {
        Value::Str(s) => Ok(s),
        Value::Nil => Err(Error::msg(format!("Cannot read properties of null (reading '{method}')"))),
        _ => Err(Error::msg(format!("str.{method} is not a function"))),
    }
}

// ---------------------------------------------------------------- collections

fn count(args: &[Value]) -> Result<Value> {
    let v = arg(args, 0);
    match v.count() {
        Some(n) => Ok(Value::from(n)),
        None => Err(Error::msg(clj::no_count(v))),
    }
}

fn range(args: &[Value]) -> Result<Value> {
    let (start, end, step) = match args {
        [] => return Err(Error::msg("(range) without an end is infinite")),
        [end] => (0.0, js_number(end), 1.0),
        [start, end] => (js_number(start), js_number(end), 1.0),
        [start, end, step, ..] => (js_number(start), js_number(end), js_number(step)),
    };
    let mut out = Vec::new();
    if step > 0.0 {
        let mut x = start;
        while x < end {
            out.push(num(x));
            x += step;
            if out.len() > 10_000_000 {
                return Err(Error::msg("range is too long"));
            }
        }
    } else if step < 0.0 {
        let mut x = start;
        while x > end {
            out.push(num(x));
            x += step;
            if out.len() > 10_000_000 {
                return Err(Error::msg("range is too long"));
            }
        }
    } else if start != end {
        return Err(Error::msg("a range of step 0 is infinite"));
    }
    Ok(Value::list(out))
}

fn keyword(args: &[Value]) -> Result<Value> {
    let named = |v: &Value| -> Option<String> {
        match v {
            Value::Keyword(k) => Some(k.name().to_string()),
            Value::Symbol(s) => Some(s.name().to_string()),
            Value::Str(s) => Some(s.to_string()),
            _ => None,
        }
    };
    Ok(match args {
        [Value::Keyword(_)] => args[0].clone(),
        [Value::Symbol(s)] => Value::Keyword(Keyword::new(s.ns(), s.name())),
        [Value::Str(s)] => Value::Keyword(Keyword::parse(s)),
        [ns, name] => {
            let name = named(name).unwrap_or_else(|| str_of(name));
            let ns = if ns.is_nil() { None } else { Some(named(ns).unwrap_or_else(|| str_of(ns))) };
            Value::Keyword(Keyword::new(ns.as_deref(), &name))
        }
        [] => return Err(arity(0)),
        _ => Value::Nil,
    })
}

fn name(args: &[Value]) -> Result<Value> {
    match arg(args, 0) {
        Value::Keyword(k) => Ok(Value::str(k.name())),
        Value::Symbol(s) => Ok(Value::str(s.name())),
        Value::Str(_) => Ok(args[0].clone()),
        other => Err(Error::msg(format!("Doesn't support name: {}", str_of(other)))),
    }
}

fn namespace(args: &[Value]) -> Result<Value> {
    match arg(args, 0) {
        Value::Keyword(k) => Ok(Value::from(k.ns())),
        Value::Symbol(s) => Ok(Value::from(s.ns())),
        other => Err(Error::msg(format!("Doesn't support namespace: {}", str_of(other)))),
    }
}

/// `(type x)`: the same function for every value of a type.
fn type_of(args: &[Value]) -> Result<Value> {
    static TYPES: OnceLock<std::sync::Mutex<HashMap<String, Value>>> = OnceLock::new();
    let v = arg(args, 0);
    if v.is_nil() {
        return Ok(Value::Nil);
    }
    let name = v.type_name().into_owned();
    let mut types = TYPES.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    Ok(types
        .entry(name.clone())
        .or_insert_with(|| {
            let not_called = |_: &[Value]| Err(Error::msg("a type is not called"));
            match name.strip_prefix("function ").and_then(|n| n.split('(').next()) {
                // JavaScript's own: #object[Number]
                Some(js) => Value::Fn(Func::new(js, not_called)),
                // ClojureScript's prints as its name: cljs.core/Keyword
                None => Value::Fn(Func::constructor(&name, not_called)),
            }
        })
        .clone())
}

fn hash_map(args: &[Value]) -> Result<Value> {
    if args.len() % 2 != 0 {
        return Err(Error::msg(format!("No value supplied for key: {}", str_of(&args[args.len() - 1]))));
    }
    Ok(Value::map(CljMap::hash_map(args.chunks(2).map(|kv| (kv[0].clone(), kv[1].clone())))))
}

fn array_map(args: &[Value]) -> Result<Value> {
    if args.len() % 2 != 0 {
        return Err(Error::msg(format!("No value supplied for key: {}", str_of(&args[args.len() - 1]))));
    }
    Ok(Value::map(CljMap::array_map(args.chunks(2).map(|kv| (kv[0].clone(), kv[1].clone())))))
}

fn not_empty(args: &[Value]) -> Result<Value> {
    let v = arg(args, 0);
    let empty = match v {
        Value::Str(s) => s.is_empty(),
        _ => clj::seq(v)?.is_empty(),
    };
    Ok(if empty { Value::Nil } else { v.clone() })
}

fn is_empty(args: &[Value]) -> Result<Value> {
    let v = arg(args, 0);
    Ok(Value::Bool(match v {
        Value::Nil => true,
        Value::Str(s) => s.is_empty(),
        _ => match v.count() {
            Some(n) => n == 0,
            None => return Err(Error::msg(format!("{} is not ISeqable", str_of(v)))),
        },
    }))
}

// ---------------------------------------------------------------- DataScript's own

/// `-differ?`: whether the first half of the arguments differs from the second.
fn differ(args: &[Value]) -> Result<Value> {
    let half = args.len().div_ceil(2);
    let (a, b) = (&args[..half.min(args.len())], &args[half.min(args.len())..]);
    Ok(Value::Bool(a != b))
}

fn db_arg<'a>(args: &'a [Value], what: &str) -> Result<&'a crate::db::Db> {
    match arg(args, 0) {
        Value::Db(db) => Ok(db),
        other => Err(Error::msg(format!("{what}: expected a database, got {}", pr_str(other)))),
    }
}

/// `(-search db [(entid db e) a])`'s first datom. An entity that resolves to nothing leaves the entity open, as
/// in the original.
fn first_datom(db: &crate::db::Db, e: &Value, a: &Value) -> Result<Option<crate::datom::Datom>> {
    let e = match entid(db, e) {
        // no entity has a fraction for an id
        Err(err) if err.no_such_id => return Ok(None),
        other => other?,
    };
    let attr = if a.truthy() {
        Some(value_attr(a).ok_or_else(|| Error::msg(format!("Cannot compare {} to an attribute", str_of(a))))?)
    } else {
        None
    };
    search(db, e, attr.as_ref(), None, None).first()
}

/// `get-else`: an attribute's value, or the default when the entity has none.
fn get_else(args: &[Value]) -> Result<Value> {
    let (e, a, else_val) = (arg(args, 1), arg(args, 2), arg(args, 3));
    if else_val.is_nil() {
        raise!("get-else: nil default value is not supported"; {"error" => Value::kw("query/where")})
    }
    let db = db_arg(args, "get-else")?;
    Ok(first_datom(db, e, a)?.map_or_else(|| else_val.clone(), |d| d.v))
}

/// `get-some`: the first of the attributes the entity has, with its value.
fn get_some(args: &[Value]) -> Result<Value> {
    let db = db_arg(args, "get-some")?;
    let e = arg(args, 1);
    for a in args.iter().skip(2) {
        if let Some(d) = first_datom(db, e, a)? {
            return Ok(Value::vector(vec![d.a_value(), d.v]));
        }
    }
    Ok(Value::Nil)
}

/// `missing?`: whether the entity has no value for the attribute.
fn missing(args: &[Value]) -> Result<Value> {
    let db = db_arg(args, "missing?")?;
    Ok(Value::Bool(match entity(db, arg(args, 1))? {
        Some(e) => e.lookup(arg(args, 2))?.is_none(),
        None => true,
    }))
}

/// `(apply f args)`, of anything ClojureScript can call: a function, a keyword or symbol looking itself up, a map,
/// a set, a vector.
pub fn call(f: &Value, args: &[Value]) -> Result<Value> {
    let get_or = |coll: &Value, k: &Value| clj::get(coll, k).unwrap_or_else(|| arg(args, 1).clone());
    match f {
        Value::Fn(f) => f.call(args),
        Value::Keyword(_) | Value::Symbol(_) => Ok(get_or(arg(args, 0), f)),
        Value::Map(_) | Value::Set(_) => Ok(clj::get(f, arg(args, 0)).unwrap_or_else(|| arg(args, 1).clone())),
        Value::Vector(items) => match arg(args, 0).as_num().filter(|i| i.fract() == 0.0 && *i >= 0.0) {
            Some(i) if (i as usize) < items.len() => Ok(items[i as usize].clone()),
            _ => Err(Error::msg(format!("No item {} in vector of length {}", str_of(arg(args, 0)), items.len()))),
        },
        Value::Host(_) => match crate::entity::Entity::from_value(f) {
            // an entity looks the attribute up in itself
            Some(e) => Ok(e.lookup_entry(arg(args, 0))?.unwrap_or_else(|| arg(args, 1).clone())),
            None => Err(Error::msg(message!(f, " is not a function"))),
        },
        other => Err(Error::msg(message!(other, " is not a function"))),
    }
}

/// `built-ins/query-fns`
pub fn query_fn(sym: &Symbol) -> Option<Value> {
    static FNS: OnceLock<HashMap<&'static str, Value>> = OnceLock::new();
    FNS.get_or_init(|| {
        let table: &[(&'static str, BuiltIn)] = &[
            ("=", eq),
            // of no arguments JavaScript finds undefined not identical to nil
            ("==", |a| if a.is_empty() { Ok(Value::Bool(false)) } else { eq(a) }),
            ("not=", not_eq),
            ("!=", not_eq),
            ("<", |a| chain(a, |o| o == Ordering::Less)),
            (">", |a| chain(a, |o| o == Ordering::Greater)),
            ("<=", |a| chain(a, |o| o != Ordering::Greater)),
            (">=", |a| chain(a, |o| o != Ordering::Less)),
            ("+", add),
            ("-", sub),
            ("*", mul),
            ("/", div),
            ("quot", |a| Ok(num(quot(num_arg(a, 0), num_arg(a, 1))))),
            ("rem", |a| {
                let (n, d) = (num_arg(a, 0), num_arg(a, 1));
                Ok(num(n - d * quot(n, d)))
            }),
            ("mod", |a| {
                let (n, d) = (num_arg(a, 0), num_arg(a, 1));
                Ok(num(js_mod(js_mod(n, d) + d, d)))
            }),
            ("inc", |a| Ok(if a.is_empty() { num(f64::NAN) } else { js_add(&a[0], &num(1.0)) })),
            ("dec", |a| Ok(num(num_arg(a, 0) - 1.0))),
            ("max", |a| Ok(extreme_of(a, true))),
            ("min", |a| Ok(extreme_of(a, false))),
            ("zero?", |a| Ok(Value::Bool(matches!(arg(a, 0), Value::Num(n) if *n == 0.0)))),
            ("pos?", |a| Ok(Value::Bool(js_number(arg(a, 0)) > 0.0))),
            ("neg?", |a| Ok(Value::Bool(js_number(arg(a, 0)) < 0.0))),
            ("even?", |a| {
                if !is_integer(arg(a, 0)) {
                    return Err(Error::msg(format!("Argument must be an integer: {}", str_of(arg(a, 0)))));
                }
                Ok(Value::Bool(js_number(arg(a, 0)) % 2.0 == 0.0))
            }),
            ("odd?", |a| {
                if !is_integer(arg(a, 0)) {
                    return Err(Error::msg(format!("Argument must be an integer: {}", str_of(arg(a, 0)))));
                }
                Ok(Value::Bool(js_number(arg(a, 0)) % 2.0 != 0.0))
            }),
            ("compare", |a| Ok(num(ordering_num(compare(arg(a, 0), arg(a, 1))?)))),
            ("rand", |a| Ok(num(if a.is_empty() { random() } else { js_number(&a[0]) * random() }))),
            ("rand-int", |a| Ok(num(rand_int(js_number(arg(a, 0)))))),
            ("true?", |a| Ok(Value::Bool(matches!(arg(a, 0), Value::Bool(true))))),
            ("false?", |a| Ok(Value::Bool(matches!(arg(a, 0), Value::Bool(false))))),
            ("nil?", |a| Ok(Value::Bool(arg(a, 0).is_nil()))),
            ("some?", |a| Ok(Value::Bool(arg(a, 0).is_some()))),
            ("not", |a| Ok(Value::Bool(!arg(a, 0).truthy()))),
            // and-fn: the last truthy argument, or the first that is not
            ("and", |a| {
                let mut last = Value::Bool(true);
                for x in a {
                    last = x.clone();
                    if !x.truthy() {
                        break;
                    }
                }
                Ok(last)
            }),
            // or-fn: the first truthy argument, or the last
            ("or", |a| {
                let mut last = Value::Nil;
                for x in a {
                    last = x.clone();
                    if x.truthy() {
                        break;
                    }
                }
                Ok(last)
            }),
            ("complement", |a| {
                let f = arg(a, 0).clone();
                Ok(Value::Fn(Func::new("complement", move |args| Ok(Value::Bool(!call(&f, args)?.truthy())))))
            }),
            ("identical?", |a| {
                Ok(Value::Bool(match (arg(a, 0), arg(a, 1)) {
                    (Value::Vector(x), Value::Vector(y)) | (Value::List(x), Value::List(y)) => {
                        std::sync::Arc::ptr_eq(x, y)
                    }
                    (Value::Map(x), Value::Map(y)) => std::sync::Arc::ptr_eq(x, y),
                    (Value::Set(x), Value::Set(y)) => std::sync::Arc::ptr_eq(x, y),
                    (x, y) => x == y,
                }))
            }),
            ("identity", |a| Ok(arg(a, 0).clone())),
            ("keyword", keyword),
            ("meta", |_| Ok(Value::Nil)),
            ("name", name),
            ("namespace", namespace),
            ("type", type_of),
            ("vector", |a| Ok(Value::vector(a.to_vec()))),
            ("list", |a| Ok(Value::list(a.to_vec()))),
            ("set", |a| clj::set(arg(a, 0))),
            ("hash-map", hash_map),
            ("array-map", array_map),
            ("count", count),
            ("range", range),
            ("not-empty", not_empty),
            ("empty?", is_empty),
            ("contains?", |a| Ok(Value::Bool(clj::contains(arg(a, 0), arg(a, 1))?))),
            ("str", |a| Ok(Value::from(a.iter().map(str_of).collect::<String>()))),
            ("subs", |a| {
                let s = match arg(a, 0) {
                    Value::Str(s) => s,
                    other => return Err(Error::msg(format!("{}.substring is not a function", str_of(other)))),
                };
                Ok(Value::from(substring(s, js_number(arg(a, 1)), a.get(2).map(js_number))))
            }),
            ("get", |a| Ok(clj::get(arg(a, 0), arg(a, 1)).unwrap_or_else(|| arg(a, 2).clone()))),
            ("pr-str", |a| Ok(Value::from(pr_str_all(a)))),
            ("print-str", |a| Ok(Value::from(print_str_all(a)))),
            ("println-str", |a| Ok(Value::from(print_str_all(a) + "\n"))),
            ("prn-str", |a| Ok(Value::from(pr_str_all(a) + "\n"))),
            ("re-find", re_find),
            ("re-matches", re_matches),
            ("re-seq", re_seq),
            ("re-pattern", re_pattern),
            ("-differ?", differ),
            ("get-else", get_else),
            ("get-some", get_some),
            ("missing?", missing),
            ("ground", |a| Ok(arg(a, 0).clone())),
            ("clojure.string/blank?", |a| {
                Ok(Value::Bool(match arg(a, 0) {
                    Value::Nil => true,
                    v => str_of(v).chars().all(crate::regex::is_space),
                }))
            }),
            ("clojure.string/includes?", |a| {
                Ok(Value::Bool(js_method_string(a, "indexOf")?.contains(&js_string(arg(a, 1)))))
            }),
            ("clojure.string/starts-with?", |a| {
                Ok(Value::Bool(js_method_string(a, "lastIndexOf")?.starts_with(&js_string(arg(a, 1)))))
            }),
            ("clojure.string/ends-with?", |a| {
                let s = js_method_string(a, "indexOf")?;
                match arg(a, 1) {
                    Value::Str(suffix) => Ok(Value::Bool(s.ends_with(&**suffix))),
                    Value::Nil => Err(Error::msg("Cannot read properties of null (reading 'length')")),
                    // what has no length is no suffix of anything
                    _ => Ok(Value::Bool(false)),
                }
            }),
            ("tuple", |a| Ok(Value::vector(a.to_vec()))),
            ("untuple", |a| Ok(arg(a, 0).clone())),
        ];
        table.iter().map(|(name, f)| (*name, Value::Fn(Func::new(name, *f)))).collect()
    })
    .get(sym.full())
    .cloned()
}

// ---------------------------------------------------------------- aggregates

fn coll_arg(args: &[Value], i: usize) -> Result<Vec<Value>> {
    clj::seq(arg(args, i))
}

fn sum(coll: &[Value]) -> Value {
    coll.iter().fold(num(0.0), |acc, x| js_add(&acc, x))
}

fn avg(coll: &[Value]) -> f64 {
    js_number(&sum(coll)) / coll.len() as f64
}

fn aggregate_median(args: &[Value]) -> Result<Value> {
    let mut terms = coll_arg(args, 0)?;
    clj::sort_by(&mut terms, compare)?;
    let size = terms.len();
    let med = size >> 1;
    let at = |i: usize| terms.get(i).ok_or_else(|| Error::msg("Index out of bounds"));
    let m = at(med)?;
    if size % 2 == 0 {
        Ok(num(js_number(&js_add(m, at(med.wrapping_sub(1))?)) / 2.0))
    } else {
        Ok(m.clone())
    }
}

fn variance(coll: &[Value]) -> f64 {
    let mean = avg(coll);
    let total: f64 = coll
        .iter()
        .map(|x| {
            let delta = js_number(x) - mean;
            delta * delta
        })
        .sum();
    total / coll.len() as f64
}

/// `aggregate-min` and `aggregate-max` of one argument: the least, the greatest, by `compare`.
fn extreme(coll: &[Value], better: Ordering) -> Result<Value> {
    let mut it = coll.iter();
    let Some(mut acc) = it.next() else { return Ok(Value::Nil) };
    for x in it {
        if compare(x, acc)? == better {
            acc = x;
        }
    }
    Ok(acc.clone())
}

/// `(min n ?x)`, `(max n ?x)`: the `n` least, or greatest, in order.
fn extremes(n: f64, coll: &[Value], better: Ordering) -> Result<Value> {
    let mut acc: Vec<Value> = Vec::new();
    for x in coll {
        if (acc.len() as f64) < n {
            acc.push(x.clone());
            clj::sort_by(&mut acc, compare)?;
        } else if better == Ordering::Less {
            // smaller than the largest kept
            match acc.last() {
                Some(last) if compare(x, last)? == Ordering::Less => {
                    acc.pop();
                    acc.push(x.clone());
                    clj::sort_by(&mut acc, compare)?;
                }
                Some(_) => {}
                None => {
                    // (compare x nil): nothing is kept, and nothing ever will be
                    compare(x, &Value::Nil)?;
                }
            }
        } else {
            match acc.first() {
                Some(first) if compare(x, first)? == Ordering::Greater => {
                    acc.remove(0);
                    acc.push(x.clone());
                    clj::sort_by(&mut acc, compare)?;
                }
                Some(_) => {}
                None => {
                    if compare(x, &Value::Nil)? == Ordering::Greater {
                        acc.push(x.clone());
                    }
                }
            }
        }
    }
    Ok(Value::vector(acc))
}

fn rand_nth(coll: &[Value]) -> Result<Value> {
    let i = rand_int(coll.len() as f64) as usize;
    coll.get(i).cloned().ok_or_else(|| Error::msg("Index out of bounds"))
}

/// `built-ins/aggregates`
pub fn aggregate_fn(name: &Symbol) -> Option<Value> {
    static FNS: OnceLock<HashMap<&'static str, Value>> = OnceLock::new();
    FNS.get_or_init(|| {
        let table: &[(&'static str, BuiltIn)] = &[
            ("sum", |a| Ok(sum(&coll_arg(a, 0)?))),
            ("avg", |a| Ok(num(avg(&coll_arg(a, 0)?)))),
            ("median", aggregate_median),
            ("variance", |a| Ok(num(variance(&coll_arg(a, 0)?)))),
            ("stddev", |a| Ok(num(variance(&coll_arg(a, 0)?).sqrt()))),
            ("distinct", |a| clj::set(arg(a, 0))),
            ("min", |a| match a {
                [coll] => extreme(&clj::seq(coll)?, Ordering::Less),
                _ => extremes(js_number(arg(a, 0)), &coll_arg(a, 1)?, Ordering::Less),
            }),
            ("max", |a| match a {
                [coll] => extreme(&clj::seq(coll)?, Ordering::Greater),
                _ => extremes(js_number(arg(a, 0)), &coll_arg(a, 1)?, Ordering::Greater),
            }),
            ("rand", |a| match a {
                [coll] => rand_nth(&clj::seq(coll)?),
                _ => {
                    let coll = coll_arg(a, 1)?;
                    let n = js_number(arg(a, 0)).max(0.0) as usize;
                    Ok(Value::vector((0..n).map(|_| rand_nth(&coll)).collect::<Result<Vec<_>>>()?))
                }
            }),
            ("sample", |a| {
                // (vec (take n (shuffle coll))): goog.array.shuffle
                let mut coll = coll_arg(a, 1)?;
                for i in (1..coll.len()).rev() {
                    let j = (random() * (i + 1) as f64).floor() as usize;
                    coll.swap(i, j);
                }
                coll.truncate(js_number(arg(a, 0)).max(0.0) as usize);
                Ok(Value::vector(coll))
            }),
            ("count", count),
            ("count-distinct", |a| Ok(Value::from(clj::distinct(&coll_arg(a, 0)?).len()))),
        ];
        table.iter().map(|(name, f)| (*name, Value::Fn(Func::new(name, *f)))).collect()
    })
    .get(name.full())
    .cloned()
}
