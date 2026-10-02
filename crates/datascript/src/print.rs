//! Printing, as ClojureScript prints: `pr-str` and `str`. DataScript's error messages are built with `pr-str`, its
//! query functions include `str` and `pr-str`, and a database prints as `#datascript/DB {...}`.

use crate::datom::Datom;
use crate::db::Db;
use crate::value::Value;

/// `(pr-str v)`
pub fn pr_str(v: &Value) -> String {
    let mut out = String::new();
    pr_into(&mut out, v);
    out
}

/// `(pr-str a b ...)`: each, with a space between.
pub fn pr_str_all(vs: &[Value]) -> String {
    let mut out = String::new();
    for (i, v) in vs.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        pr_into(&mut out, v);
    }
    out
}

/// `(print-str a b ...)`: as `pr-str`, with strings as they are.
pub fn print_str_all(vs: &[Value]) -> String {
    let mut out = String::new();
    for (i, v) in vs.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        write(&mut out, v, false);
    }
    out
}

pub fn pr_into(out: &mut String, v: &Value) {
    write(out, v, true)
}

/// `(str v)`: JavaScript's `toString` of it.
pub fn str_of(v: &Value) -> String {
    let mut out = String::new();
    str_into(&mut out, v);
    out
}

pub fn str_into(out: &mut String, v: &Value) {
    match v {
        Value::Nil => {}
        Value::Str(s) => out.push_str(s),
        Value::Num(n) => out.push_str(&number_to_string(*n)),
        Value::Uuid(s) => out.push_str(s),
        // a database and a datom have no toString of their own
        Value::Db(_) | Value::Datom(_) => out.push_str("[object Object]"),
        // a collection's toString is its pr-str
        _ => write(out, v, true),
    }
}

/// A string's characters as ClojureScript sees them: one-character strings, a UTF-16 code unit each.
pub fn string_chars(s: &str) -> Vec<Value> {
    // a character beyond the basic plane is two code units to ClojureScript; here it stays whole, as half of it
    // is no string in Rust
    s.chars().map(|c| Value::str(c.encode_utf8(&mut [0u8; 4]))).collect()
}

fn write(out: &mut String, v: &Value, readably: bool) {
    match v {
        Value::Nil => out.push_str("nil"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Num(n) => {
            if n.is_nan() {
                out.push_str("##NaN")
            } else if *n == f64::INFINITY {
                out.push_str("##Inf")
            } else if *n == f64::NEG_INFINITY {
                out.push_str("##-Inf")
            } else if *n == 0.0 && n.is_sign_negative() {
                out.push_str("-0.0")
            } else {
                out.push_str(&number_to_string(*n))
            }
        }
        Value::Str(s) => {
            if readably {
                quote_string(out, s)
            } else {
                out.push_str(s)
            }
        }
        Value::Keyword(k) => {
            out.push(':');
            out.push_str(k.full())
        }
        Value::Symbol(s) => out.push_str(s.full()),
        Value::Vector(items) => write_seq(out, "[", "]", items.iter(), readably),
        Value::List(items) => write_seq(out, "(", ")", items.iter(), readably),
        Value::Set(s) => write_seq(out, "#{", "}", s.iter(), readably),
        Value::Map(m) => {
            out.push('{');
            for (i, (k, v)) in m.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write(out, k, readably);
                out.push(' ');
                write(out, v, readably);
            }
            out.push('}');
        }
        Value::Uuid(s) => {
            out.push_str("#uuid \"");
            out.push_str(s);
            out.push('"');
        }
        Value::Inst(ms) => {
            out.push_str("#inst \"");
            out.push_str(&inst_to_string(*ms));
            out.push('"');
        }
        Value::Regex(r) => {
            out.push_str("#\"");
            out.push_str(&r.source);
            out.push('"');
        }
        Value::Datom(d) => pr_datom(out, d),
        Value::Db(db) => pr_db(out, db),
        Value::Fn(f) => {
            out.push_str("#object[");
            out.push_str(if f.name().is_empty() { "Function" } else { f.name() });
            out.push(']');
        }
        Value::Host(h) => out.push_str(&h.0.pr_str()),
    }
}

fn write_seq<'a, I: Iterator<Item = &'a Value>>(out: &mut String, open: &str, close: &str, items: I, readably: bool) {
    out.push_str(open);
    for (i, x) in items.enumerate() {
        if i > 0 {
            out.push(' ');
        }
        write(out, x, readably);
    }
    out.push_str(close);
}

/// `#datascript/Datom [e a v tx added]`
pub fn pr_datom(out: &mut String, d: &Datom) {
    out.push_str("#datascript/Datom [");
    out.push_str(&number_to_string(d.e as f64));
    out.push(' ');
    pr_into(out, &d.a_value());
    out.push(' ');
    pr_into(out, &d.v);
    out.push(' ');
    out.push_str(&number_to_string(d.tx() as f64));
    out.push(' ');
    out.push_str(if d.added() { "true" } else { "false" });
    out.push(']');
}

/// `#datascript/DB {:schema ..., :datoms [[e a v tx] ...]}`
pub fn pr_db(out: &mut String, db: &Db) {
    out.push_str("#datascript/DB {:schema ");
    pr_into(out, &db.schema_value());
    out.push_str(", :datoms [");
    let mut first = true;
    db.for_each_datom(|d| {
        if !first {
            out.push(' ');
        }
        first = false;
        out.push('[');
        out.push_str(&number_to_string(d.e as f64));
        out.push(' ');
        pr_into(out, &d.a_value());
        out.push(' ');
        pr_into(out, &d.v);
        out.push(' ');
        out.push_str(&number_to_string(d.tx() as f64));
        out.push(']');
    });
    out.push_str("]}");
}

/// `quote-string`: the escapes ClojureScript writes.
fn quote_string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
}

/// JavaScript's `Number.prototype.toString`: the shortest digits that read back as the number, written out for
/// exponents from -7 to 21, in exponent notation beyond.
pub fn number_to_string(n: f64) -> String {
    if n.is_nan() {
        return "NaN".into();
    }
    if n == 0.0 {
        return "0".into();
    }
    if n.is_infinite() {
        return if n > 0.0 { "Infinity".into() } else { "-Infinity".into() };
    }
    // integers that fit print as they are
    if n.fract() == 0.0 && n.abs() < 1e15 {
        return format!("{}", n as i64);
    }
    let mut out = String::new();
    if n < 0.0 {
        out.push('-');
    }
    // Rust's `{:e}` gives the shortest digits that round-trip, as d.ddd…e±x
    let sci = format!("{:e}", n.abs());
    let (mantissa, exp) = sci.split_once('e').expect("exponent");
    let exp: i32 = exp.parse().expect("exponent");
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let k = digits.len() as i32;
    let point = exp + 1; // digits before the decimal point
    if k <= point && point <= 21 {
        out.push_str(&digits);
        for _ in 0..(point - k) {
            out.push('0');
        }
    } else if 0 < point && point <= 21 {
        out.push_str(&digits[..point as usize]);
        out.push('.');
        out.push_str(&digits[point as usize..]);
    } else if -6 < point && point <= 0 {
        out.push_str("0.");
        for _ in 0..(-point) {
            out.push('0');
        }
        out.push_str(&digits);
    } else {
        out.push_str(&digits[..1]);
        if k > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        out.push('e');
        out.push(if point - 1 < 0 { '-' } else { '+' });
        out.push_str(&(point - 1).abs().to_string());
    }
    out
}

/// A count of days since 1970-01-01 as a civil date (Howard Hinnant's algorithm).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// The civil date as days since 1970-01-01.
pub(crate) fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = if m > 2 { m - 3 } else { m + 9 } as i64;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// An instant as ClojureScript prints it: `2020-01-01T00:00:00.000-00:00`.
pub fn inst_to_string(ms: f64) -> String {
    if !ms.is_finite() {
        return "NaN-NaN-NaNTNaN:NaN:NaN.NaN-00:00".into();
    }
    let ms = ms.trunc() as i64;
    let days = ms.div_euclid(86_400_000);
    let rem = ms.rem_euclid(86_400_000);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}-00:00",
        y,
        m,
        d,
        rem / 3_600_000,
        rem / 60_000 % 60,
        rem / 1000 % 60,
        rem % 1000
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_as_javascript_writes_them() {
        for (n, s) in [
            (1.0, "1"),
            (-1.0, "-1"),
            (1.5, "1.5"),
            (0.1, "0.1"),
            (1e21, "1e+21"),
            (1.5e-7, "1.5e-7"),
            (1e-7, "1e-7"),
            (0.000001, "0.000001"),
            (123456789012345680000.0, "123456789012345680000"),
            (1e20, "100000000000000000000"),
            (1.2345e25, "1.2345e+25"),
            (536870913.0, "536870913"),
            (0.5, "0.5"),
            (100.25, "100.25"),
            (-0.0, "0"),
            (9007199254740993.0, "9007199254740992"),
            (1e300, "1e+300"),
            (5e-324, "5e-324"),
            (123.456, "123.456"),
        ] {
            assert_eq!(number_to_string(n), s, "{n:e}");
        }
    }

    #[test]
    fn instants() {
        assert_eq!(inst_to_string(1_577_836_800_000.0), "2020-01-01T00:00:00.000-00:00");
        assert_eq!(inst_to_string(0.0), "1970-01-01T00:00:00.000-00:00");
        assert_eq!(inst_to_string(-1.0), "1969-12-31T23:59:59.999-00:00");
        assert_eq!(days_from_civil(2020, 1, 1) * 86_400_000, 1_577_836_800_000);
    }
}
