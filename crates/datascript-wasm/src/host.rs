//! The host: what the module imports from it, and the values of the host's that the module holds.
//!
//! A function of the host's is a handle the host knows it by. The module calls it through `ds_host_call`, with its
//! arguments encoded; the host answers through `ds_reply` before it returns. A value of the host's that the module
//! has no form for is a handle too, with the hash the host gives it; the module asks the host when it must know
//! more of it. Each mention of a handle the host sends is one the module gives back, with `ds_host_release`, when
//! it lets go of it.

use crate::codec::{Reader, Writer, VECTOR};
use crate::state::State;
use datascript::value::{Func, HostObj, HostObject};
use datascript::{Error, Result, Value};
use std::any::Any;
use std::cell::RefCell;
use std::cmp::Ordering;
use std::sync::Arc;

pub const KIND_FN: u32 = 0;
pub const KIND_OBJ: u32 = 1;

/// `ds_host_op`: whether two of the host's values are equal (1) or not (0)
pub const OP_EQUIV: u32 = 0;
/// the value as ClojureScript prints it, in the reply
pub const OP_PR_STR: u32 = 1;
/// the name of its type, as DataScript orders values of different types by, in the reply
pub const OP_TYPE_NAME: u32 = 2;
/// how two of the host's values of one type order: 0 less, 1 equal, 2 greater, 3 if the host cannot say
pub const OP_COMPARE: u32 = 3;
/// the value as ClojureScript's `str` makes a string of it, in the reply
pub const OP_STR: u32 = 4;
/// whether the value is one ClojureScript makes a sequence of (1) or not (0)
pub const OP_SEQABLE: u32 = 5;

/// The handle the host's regular expressions answer to, when the host matches them (`ds_set_option`):
/// `[source flags input]` → `nil`, or `[index whole group ...]`.
pub const REGEX_FN: u32 = u32::MAX;
/// What the module asks of a value of the host's that it does not look into itself, as functions of the host's
/// with handles of their own: `[x k not-found]` → `(get x k not-found)`
pub const GET_FN: u32 = u32::MAX - 1;
/// `[x]` → `(count x)`
pub const COUNT_FN: u32 = u32::MAX - 2;
/// `[x]` → `(seq x)`
pub const SEQ_FN: u32 = u32::MAX - 3;
/// `[x k]` → `(contains? x k)`
pub const CONTAINS_FN: u32 = u32::MAX - 4;
/// `[x args]` → `(apply x args)`
pub const APPLY_FN: u32 = u32::MAX - 5;
/// `[x]` → the map `x` is, when it is a map of a kind the module has no form for (a record); `nil` otherwise
pub const AS_MAP_FN: u32 = u32::MAX - 6;

#[cfg(target_arch = "wasm32")]
mod imports {
    #[link(wasm_import_module = "datascript")]
    extern "C" {
        pub fn ds_host_call(handle: u32, ptr: *const u8, len: usize) -> u32;
        pub fn ds_host_op(op: u32, a: u32, b: u32) -> u32;
        pub fn ds_host_release(kind: u32, handle: u32);
        pub fn ds_host_random() -> f64;
        pub fn ds_host_log(level: u32, ptr: *const u8, len: usize);
    }
}

/// Off WebAssembly there is no host but the one a test puts here.
#[cfg(not(target_arch = "wasm32"))]
pub mod native {
    use std::cell::RefCell;

    pub type Call = Box<dyn Fn(u32, &[u8]) -> u32>;

    thread_local! {
        pub static CALL: RefCell<Option<Call>> = const { RefCell::new(None) };
        pub static RELEASED: RefCell<Vec<(u32, u32)>> = const { RefCell::new(Vec::new()) };
    }
}

fn raw_call(handle: u32, bytes: &[u8]) -> u32 {
    #[cfg(target_arch = "wasm32")]
    unsafe {
        imports::ds_host_call(handle, bytes.as_ptr(), bytes.len())
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        native::CALL.with(|c| match &*c.borrow() {
            Some(call) => call(handle, bytes),
            None => {
                set_reply(Vec::new());
                1
            }
        })
    }
}

fn raw_op(op: u32, a: u32, b: u32) -> u32 {
    #[cfg(target_arch = "wasm32")]
    unsafe {
        imports::ds_host_op(op, a, b)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (op, a, b);
        0
    }
}

pub fn release(kind: u32, handle: u32) {
    #[cfg(target_arch = "wasm32")]
    unsafe {
        imports::ds_host_release(kind, handle)
    }
    #[cfg(not(target_arch = "wasm32"))]
    native::RELEASED.with(|r| r.borrow_mut().push((kind, handle)));
}

pub fn random() -> f64 {
    #[cfg(target_arch = "wasm32")]
    unsafe {
        imports::ds_host_random()
    }
    #[cfg(not(target_arch = "wasm32"))]
    0.5
}

pub fn log(level: u32, message: &str) {
    #[cfg(target_arch = "wasm32")]
    unsafe {
        imports::ds_host_log(level, message.as_ptr(), message.len())
    }
    #[cfg(not(target_arch = "wasm32"))]
    eprintln!("datascript[{level}]: {message}");
}

thread_local! {
    /// What the host answered its last call with
    static REPLY: RefCell<Option<Vec<u8>>> = const { RefCell::new(None) };
}

pub fn set_reply(bytes: Vec<u8>) {
    REPLY.with(|r| *r.borrow_mut() = Some(bytes));
}

pub fn take_reply() -> Option<Vec<u8>> {
    REPLY.with(|r| r.borrow_mut().take())
}

// ---------------------------------------------------------------- functions

/// A function of the host's, for as long as the module holds it.
struct FnGuard {
    handle: u32,
}

impl Drop for FnGuard {
    fn drop(&mut self) {
        release(KIND_FN, self.handle)
    }
}

/// The function of a handle the host sent. One the module already holds is the same value again.
pub fn host_fn(handle: u32, state: &mut State) -> Func {
    if let Some(f) = state.host_fns.get(&handle).and_then(|weak| weak.upgrade()) {
        // the host counted this mention too, and one is held already
        release(KIND_FN, handle);
        return f;
    }
    let guard: Arc<dyn Any + Send + Sync> = Arc::new(FnGuard { handle });
    let f = Func::host("Function", guard, move |args| call(handle, args));
    state.host_fns.insert(handle, f.downgrade());
    f
}

/// The host's handle of a function, when it is the host's.
pub fn fn_handle(f: &Func) -> Option<u32> {
    f.host_handle()?.downcast_ref::<FnGuard>().map(|g| g.handle)
}

/// Calls a function of the host's. What it throws comes back as an error that carries the host's own exception,
/// for the host to throw on as it was.
pub fn call(handle: u32, args: &[Value]) -> Result<Value> {
    let mut w = Writer::new();
    w.count(VECTOR, args.len());
    for a in args {
        w.value(a);
    }
    call_written(handle, w)
}

/// A call whose arguments are written.
fn call_written(handle: u32, w: Writer) -> Result<Value> {
    let status = raw_call(handle, &w.buf);
    let reply = take_reply().ok_or_else(|| Error::msg("datascript: the host answered a call with nothing"))?;
    if status == 0 {
        Reader::new(&reply).value()
    } else {
        Err(decode_error(&reply))
    }
}

/// An error as the host tells it: `[message data exception]`.
fn decode_error(reply: &[u8]) -> Error {
    let told = Reader::new(reply).value();
    let Ok(told) = told else { return Error::msg("datascript: a function of the host's failed") };
    let part = |i: usize| told.as_seq().and_then(|s| s.get(i)).cloned().unwrap_or(Value::Nil);
    let message = part(0).as_str().unwrap_or("").to_string();
    let mut error = Error::new(message, part(1));
    let exception = part(2);
    if exception.is_some() {
        error.host = Some(Arc::new(exception));
    }
    error
}

/// The host's exception an error carries, if the error came from the host.
pub fn exception_of(error: &Error) -> Option<Value> {
    error.host.as_ref()?.downcast_ref::<Value>().cloned()
}

// ---------------------------------------------------------------- values the module does not look into

struct ExternRef {
    handle: u32,
    hash: i32,
}

impl Drop for ExternRef {
    fn drop(&mut self) {
        release(KIND_OBJ, self.handle)
    }
}

fn op_string(op: u32, handle: u32, otherwise: &str) -> String {
    raw_op(op, handle, 0);
    take_reply().and_then(|bytes| String::from_utf8(bytes).ok()).unwrap_or_else(|| otherwise.to_string())
}

impl ExternRef {
    /// Asks the host of this value: one of its functions, with the value and then `args`.
    fn ask(&self, function: u32, args: &[&Value]) -> Result<Value> {
        let mut w = Writer::new();
        w.count(VECTOR, 1 + args.len());
        w.byte(crate::codec::HOST_OBJ);
        w.varint(self.handle as u64);
        w.zigzag(self.hash as i64);
        for a in args {
            w.value(a);
        }
        call_written(function, w)
    }
}

impl HostObject for ExternRef {
    fn hash(&self) -> i32 {
        self.hash
    }

    fn lookup(&self, k: &Value, not_found: &Value) -> Option<Result<Value>> {
        Some(self.ask(GET_FN, &[k, not_found]))
    }

    fn count(&self) -> Option<Result<usize>> {
        Some(self.ask(COUNT_FN, &[]).map(|n| n.as_num().unwrap_or(0.0) as usize))
    }

    fn seqable(&self) -> bool {
        raw_op(OP_SEQABLE, self.handle, 0) == 1
    }

    fn seq(&self) -> Option<Result<Vec<Value>>> {
        Some(self.ask(SEQ_FN, &[]).map(|items| items.as_seq().map(<[Value]>::to_vec).unwrap_or_default()))
    }

    fn contains(&self, k: &Value) -> Option<Result<bool>> {
        Some(self.ask(CONTAINS_FN, &[k]).map(|found| found.truthy()))
    }

    fn invoke(&self, args: &[Value]) -> Option<Result<Value>> {
        Some(self.ask(APPLY_FN, &[&Value::list(args.to_vec())]))
    }

    fn as_map(&self) -> Result<Option<Value>> {
        Ok(Some(self.ask(AS_MAP_FN, &[])?).filter(|m| matches!(m, Value::Map(_))))
    }

    fn equiv(&self, other: &dyn HostObject) -> bool {
        match other.as_any().downcast_ref::<ExternRef>() {
            Some(o) => o.handle == self.handle || raw_op(OP_EQUIV, self.handle, o.handle) == 1,
            None => false,
        }
    }

    fn type_name(&self) -> String {
        op_string(OP_TYPE_NAME, self.handle, "object")
    }

    fn pr_str(&self) -> String {
        op_string(OP_PR_STR, self.handle, "#object[Object]")
    }

    fn compare(&self, other: &dyn HostObject) -> Option<Ordering> {
        let o = other.as_any().downcast_ref::<ExternRef>()?;
        match raw_op(OP_COMPARE, self.handle, o.handle) {
            0 => Some(Ordering::Less),
            1 => Some(Ordering::Equal),
            2 => Some(Ordering::Greater),
            _ => None,
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub fn host_object(handle: u32, hash: i32) -> Value {
    Value::Host(HostObj::new(ExternRef { handle, hash }))
}

/// The host's handle and hash of a value, when it is the host's.
pub fn object_handle(obj: &HostObj) -> Option<(u32, i32)> {
    obj.0.as_any().downcast_ref::<ExternRef>().map(|r| (r.handle, r.hash))
}

// ---------------------------------------------------------------- regular expressions

/// The host matches regular expressions: its own are JavaScript's, and exactly so.
pub fn use_host_regex(on: bool) {
    if !on {
        datascript::regex::set_engine(None);
        return;
    }
    datascript::regex::set_engine(Some(Arc::new(|source, flags, input| {
        // nil, or [index whole group ...]
        let found = call(REGEX_FN, &[Value::str(source), Value::str(flags), Value::str(input)])?;
        let Some(items) = found.as_seq() else { return Ok(None) };
        let index = items.first().and_then(Value::as_num).unwrap_or(0.0) as usize;
        let groups = items.iter().skip(1).map(|g| g.as_str().map(String::from)).collect();
        Ok(Some(datascript::regex::Match { index, groups }))
    })));
}
