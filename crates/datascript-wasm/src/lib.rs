//! DataScript's WebAssembly module: the database behind a small binary interface, for ClojureScript, JavaScript and
//! other WebAssembly programs.
//!
//! # Calling it
//!
//! A host calls `ds_call(op, ptr, len)`: an operation's number (`ops`), and its arguments as one encoded vector
//! (`codec`) in memory the host got from `ds_alloc`, which the call takes over. The answer is at `ds_result_ptr()`,
//! `ds_result_len()` bytes long, until the next call returns: the operation's value when `ds_call` answered 0, and
//! `[message data exception]` when it answered 1.
//!
//! A database is a handle (`codec::DB`). It stays in the module until the host gives the handle back
//! (`ops::RELEASE_DB`). The same database value has the same handle for as long as the host holds it.
//!
//! # Being called
//!
//! The module imports, from the module named `datascript`:
//!
//! - `ds_host_call(handle, ptr, len) -> status`: call the host's function of that handle with the encoded vector of
//!   arguments at `ptr`; answer through `ds_reply(ptr, len)`, memory from `ds_alloc` that the module takes over, with
//!   the value (status 0) or `[message data exception]` (status 1). The host may call `ds_call` while it is called.
//! - `ds_host_op(op, a, b) -> n`: what the module asks of the host's own values (`host::OP_*`).
//! - `ds_host_release(kind, handle)`: the module lets go of one mention of a function (0) or value (1) of the host's.
//! - `ds_host_random() -> f64`: in `[0, 1)`.
//! - `ds_host_log(level, ptr, len)`: a message for whoever reads the host's log; level 0 is a failure of the module.
//!
//! # When a call does not return
//!
//! The module's code runs on the host's stack. A value nested deeper than that stack has room for — ClojureScript
//! has the same limit, and tells of it with the same error — ends the call with the host's error, with nothing
//! unwound. The host then puts the exported `__stack_pointer` back to what it was before the call and calls
//! `ds_recover()`; the databases it holds are as they were.
//!
//! `ds_edn(ptr, len)` is the same database for a host that would rather write EDN than encode values (`edn_api`).

pub mod codec;
pub mod edn_api;
pub mod host;
pub mod ops;
pub mod state;

use codec::{Reader, Writer};
use datascript::{Error, Value};
use std::cell::RefCell;

/// The interface's version: a host checks it against the one it was written for.
pub const ABI_VERSION: u32 = 1;

thread_local! {
    static RESULT: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

fn set_result(bytes: Vec<u8>) {
    RESULT.with(|r| *r.borrow_mut() = bytes);
}

fn init() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        std::panic::set_hook(Box::new(|info| host::log(0, &format!("datascript: {info}"))));
        // the generator `rand`, `rand-int` and the sampling aggregates draw from
        let seed = (host::random() * 9007199254740992.0) as u64 ^ 0x9E37_79B9_7F4A_7C15;
        datascript::built_ins::seed_random(seed);
    });
}

#[no_mangle]
pub extern "C" fn ds_abi_version() -> u32 {
    ABI_VERSION
}

/// Memory for the host to write a message into.
#[no_mangle]
pub extern "C" fn ds_alloc(len: usize) -> *mut u8 {
    let mut buf = Vec::<u8>::with_capacity(len.max(1));
    let ptr = buf.as_mut_ptr();
    std::mem::forget(buf);
    ptr
}

/// Memory from `ds_alloc` that was handed to no call.
///
/// # Safety
/// `ptr` and `len` are what `ds_alloc` was asked for and answered.
#[no_mangle]
pub unsafe extern "C" fn ds_free(ptr: *mut u8, len: usize) {
    drop(Vec::from_raw_parts(ptr, 0, len.max(1)));
}

/// The message at `ptr`, which becomes the module's.
unsafe fn take(ptr: *mut u8, len: usize) -> Vec<u8> {
    Vec::from_raw_parts(ptr, len, len.max(1))
}

fn error_value(e: &Error) -> Value {
    Value::vector(vec![Value::str(&e.message), e.data.clone(), host::exception_of(e).unwrap_or(Value::Nil)])
}

/// One operation. 0, and its value is the result; 1, and `[message data exception]` is.
///
/// # Safety
/// `ptr` is memory from `ds_alloc(len)`, written by the host, and not used by it again.
#[no_mangle]
pub unsafe extern "C" fn ds_call(op: u32, ptr: *mut u8, len: usize) -> u32 {
    init();
    let message = take(ptr, len);
    let args = Reader::new(&message).value();
    drop(message);
    let answer = args.and_then(|args| ops::dispatch(op, args.as_seq().unwrap_or(&[])));
    let mut w = Writer::new();
    let status = match &answer {
        Ok(v) => {
            w.value(v);
            0
        }
        Err(e) => {
            w.value(&error_value(e));
            1
        }
    };
    set_result(w.buf);
    status
}

#[no_mangle]
pub extern "C" fn ds_result_ptr() -> *const u8 {
    RESULT.with(|r| r.borrow().as_ptr())
}

#[no_mangle]
pub extern "C" fn ds_result_len() -> usize {
    RESULT.with(|r| r.borrow().len())
}

/// The host's answer to the call the module is making of it.
///
/// # Safety
/// `ptr` is memory from `ds_alloc(len)`, written by the host, and not used by it again.
#[no_mangle]
pub unsafe extern "C" fn ds_reply(ptr: *mut u8, len: usize) {
    host::set_reply(take(ptr, len));
}

/// Puts the module back in order after a call that did not return: one the host's stack ran out under, or one
/// that trapped. The host calls it once it has put the stack pointer back.
#[no_mangle]
pub extern "C" fn ds_recover() {
    datascript::coll::recover_drops();
    host::take_reply();
}

/// One operation written as EDN, answered as EDN (`edn_api`). 0 and the value, or 1 and `{:message … :data …}`.
///
/// # Safety
/// `ptr` is memory from `ds_alloc(len)` holding UTF-8, and not used by the host again.
#[no_mangle]
pub unsafe extern "C" fn ds_edn(ptr: *mut u8, len: usize) -> u32 {
    init();
    let message = take(ptr, len);
    let (status, text) = match std::str::from_utf8(&message) {
        Ok(text) => edn_api::call(text),
        Err(_) => (1, "{:message \"datascript: the request is not UTF-8\", :data nil}".to_string()),
    };
    set_result(text.into_bytes());
    status
}
