//! The module built into another program: a Rust program that has DataScript's crates among its own, and so is one
//! WebAssembly module with them (`datascript_wasm`'s own words on it).
//!
//!     cargo build -p datascript-wasm --example embedded --target wasm32-unknown-unknown --profile wasm-release
//!
//! Its host loads it as it loads the module itself, then calls `embedded_start`, and from there on has DataScript,
//! whole, and this program's two operations beside the module's own:
//!
//! - `COUNT`, `[db attr]` → how many datoms the database has of the attribute. The database is the one the host
//!   named: the program reads its index where it is, and nothing of it crosses to the host but the number.
//! - `TRANSACT`, `[deref moved-on tx-data]` → how many datoms the transaction added and retracted. The program
//!   transacts on a connection the host holds: `deref`, a function of the host's, answers the database the
//!   connection holds now; the program makes the transaction of it, as `transact!` would, and calls `moved-on`
//!   with the database before, the database after, the transaction's datoms and its tempids, for the host to move
//!   its connection on and tell its listeners (`datascript.conn/-moved-on!`, in ClojureScript).
//!
//! `cljs/test.sh simple embedded` runs DataScript's tests on it, and these two operations
//! (`datascript.test.embedded`).

use datascript::{built_ins, Error, Index, Result, Value};
use datascript_wasm::codec::Writer;
use datascript_wasm::ops::EXTENSION;
use std::sync::Arc;

pub const COUNT: u32 = EXTENSION;
pub const TRANSACT: u32 = EXTENSION + 1;

fn count(args: &[Value]) -> Result<Value> {
    let [Value::Db(db), attr] = args else {
        return Err(Error::msg("embedded: a database, and an attribute"));
    };
    Ok(Value::from(db.datoms(Index::Aevt, std::slice::from_ref(attr))?.count()?))
}

fn transact(args: &[Value]) -> Result<Value> {
    let [deref, moved_on, tx_data] = args else {
        return Err(Error::msg(
            "embedded: what answers the connection's database, what moves it on, and the transaction",
        ));
    };
    let before = built_ins::call(deref, &[])?;
    let Value::Db(db) = &before else {
        return Err(Error::msg("embedded: the connection holds no database"));
    };
    // the connection moves on from its database, which then keeps its indexes as the new one's
    let report = datascript::advance(db, tx_data, Value::Nil)?;
    let datoms = Value::vector(report.tx_data.iter().map(|d| Value::Datom(Arc::new(d.clone()))).collect());
    built_ins::call(moved_on, &[before.clone(), Value::Db(report.db_after.clone()), datoms, report.tempids.clone()])?;
    Ok(Value::from(report.tx_data.len()))
}

fn answer(op: u32, args: &[Value], w: &mut Writer) -> Result<()> {
    let value = match op {
        COUNT => count(args)?,
        TRANSACT => transact(args)?,
        other => return Err(Error::msg(format!("embedded: no operation {other}"))),
    };
    w.value(&value);
    Ok(())
}

/// The program starts: the module first, then what answers the program's own operations.
#[no_mangle]
pub extern "C" fn embedded_start() {
    datascript_wasm::start();
    datascript_wasm::extend(answer);
}
