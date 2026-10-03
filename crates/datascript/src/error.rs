//! Errors. DataScript throws `ex-info`s: a message, and a map of data with an `:error` keyword. The port returns
//! them as values, with the same messages and data.

use crate::value::Value;
use std::any::Any;
use std::fmt;
use std::sync::Arc;

#[derive(Clone)]
pub struct Error {
    pub message: String,
    /// `ex-data`: a map, or `nil` for an error that carries none
    pub data: Value,
    /// The host's own exception, when a function of the host's threw: it passes through unchanged
    pub host: Option<Arc<dyn Any + Send + Sync>>,
    /// Whether it is of a number that no entity has for an id: a fraction. Reading by one finds nothing.
    pub(crate) no_such_id: bool,
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub fn new(message: impl Into<String>, data: Value) -> Error {
        Error { message: message.into(), data, host: None, no_such_id: false }
    }

    /// An error without data, as a plain `js/Error` is.
    pub fn msg(message: impl Into<String>) -> Error {
        Error { message: message.into(), data: Value::Nil, host: None, no_such_id: false }
    }

    /// The error of an id no entity can have. ClojureScript takes any number for one, and finds nothing by it.
    pub(crate) fn no_such_id(message: impl Into<String>) -> Error {
        Error { message: message.into(), data: Value::Nil, host: None, no_such_id: true }
    }

    /// Nothing, when the error is of an id that no entity has; the error otherwise.
    pub(crate) fn or_nothing<T>(self) -> Result<Option<T>> {
        if self.no_such_id {
            Ok(None)
        } else {
            Err(self)
        }
    }

    pub fn host(message: impl Into<String>, exception: Arc<dyn Any + Send + Sync>) -> Error {
        Error { message: message.into(), data: Value::Nil, host: Some(exception), no_such_id: false }
    }

    /// `(:error (ex-data e))`, as `ns/name`
    pub fn kind(&self) -> Option<&str> {
        match self.data.get(&Value::kw("error")) {
            Some(Value::Keyword(k)) => Some(k.full()),
            _ => None,
        }
    }
}

impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.message, crate::print::pr_str(&self.data))
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Error {}

/// A part of an error's message: DataScript's `raise` writes strings as they are and everything else with `pr-str`.
pub trait MsgPart {
    fn write_to(&self, out: &mut String);
}

impl MsgPart for str {
    fn write_to(&self, out: &mut String) {
        out.push_str(self)
    }
}

impl MsgPart for String {
    fn write_to(&self, out: &mut String) {
        out.push_str(self)
    }
}

impl MsgPart for Value {
    fn write_to(&self, out: &mut String) {
        crate::print::pr_into(out, self)
    }
}

impl MsgPart for crate::named::Keyword {
    fn write_to(&self, out: &mut String) {
        out.push(':');
        out.push_str(self.full())
    }
}

impl MsgPart for crate::named::Symbol {
    fn write_to(&self, out: &mut String) {
        out.push_str(self.full())
    }
}

impl MsgPart for crate::named::Attr {
    fn write_to(&self, out: &mut String) {
        crate::print::pr_into(out, &crate::datom::attr_value(self))
    }
}

impl MsgPart for crate::datom::Datom {
    fn write_to(&self, out: &mut String) {
        crate::print::pr_datom(out, self)
    }
}

macro_rules! msg_part_number {
    ($($t:ty),*) => {$(
        impl MsgPart for $t {
            fn write_to(&self, out: &mut String) {
                out.push_str(&crate::print::number_to_string(*self as f64))
            }
        }
    )*};
}
msg_part_number!(i32, i64, u32, usize, f64);

impl<T: MsgPart + ?Sized> MsgPart for &T {
    fn write_to(&self, out: &mut String) {
        (**self).write_to(out)
    }
}

/// The message `raise` builds from its fragments.
#[macro_export]
macro_rules! message {
    ($($part:expr),+ $(,)?) => {{
        let mut out = String::new();
        $($crate::error::MsgPart::write_to(&$part, &mut out);)+
        out
    }};
}

/// DataScript's `raise`: an error of these message fragments and this data, `{:key value ...}` with keyword keys
/// written `"ns/name"`.
#[macro_export]
macro_rules! raise {
    ($($part:expr),+ ; { $($k:literal => $v:expr),* $(,)? }) => {
        return Err($crate::error::Error::new(
            $crate::message!($($part),+),
            $crate::Value::kw_map(&[$(($k, $crate::Value::from($v))),*]),
        ))
    };
}

/// The error `raise` would return, as a value.
#[macro_export]
macro_rules! error {
    ($($part:expr),+ ; { $($k:literal => $v:expr),* $(,)? }) => {
        $crate::error::Error::new(
            $crate::message!($($part),+),
            $crate::Value::kw_map(&[$(($k, $crate::Value::from($v))),*]),
        )
    };
}
