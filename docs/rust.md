# DataScript in Rust, for WebAssembly

A port of DataScript 1.8.1 from ClojureScript to Rust, compiled to a WebAssembly module, with a ClojureScript
interface that is `datascript.core` over that module. The ClojureScript in `src/` stays in this repository as the
port's reference, and the port is checked against it.

The port is a functional one. It answers what ClojureScript DataScript answers, in the order it answers it, and raises
its errors with its messages and data. Inside, it differs where Rust does better another way.

| | |
|---|---|
| `crates/datascript` | The database: values, indexes, transactions, queries, pull, entities, filtered databases, serialization. A Rust library, with no dependencies. |
| `crates/datascript-wasm` | The WebAssembly module: the database behind a small interface, binary or EDN. |
| `cljs/` | `datascript.core` for ClojureScript over the module, with DataScript's own connections, entities and JavaScript API on top. |
| `crates/conformance`, `conformance/` | The harness that runs the port and ClojureScript DataScript over the same cases and compares every answer. |

## From ClojureScript

`cljs/src` is a drop-in for DataScript's own `src`: the same namespaces, `datascript.core` first among them, so a
program written for DataScript compiles against it unchanged. It needs `io.github.tonsky/extend-clj`, as DataScript
does, and not `persistent-sorted-set`.

```clojure
;; deps.edn
{:deps {io.github.theronic/datascript-rust {:git/sha "…" :deps/root "cljs"}}}
```

The module is built with cargo, and is one file to serve beside the program:

```bash
cargo build -p datascript-wasm --target wasm32-unknown-unknown --profile wasm-release
# target/wasm32-unknown-unknown/wasm-release/datascript_wasm.wasm
```

A program loads the module before it asks anything of DataScript:

```clojure
(ns app.core
  (:require [datascript.core :as d]
            [datascript.wasm :as wasm]))

(-> (wasm/instantiate (js/fetch "datascript.wasm"))   ; a Response, bytes, or a compiled WebAssembly.Module
    (.then (fn [_]
             (let [conn (d/create-conn {:aka {:db/cardinality :db.cardinality/many}})]
               (d/transact! conn [{:db/id -1 :name "Ivan" :aka ["Devil" "Tupen"]}])
               (d/q '[:find ?n :where [?e :aka "Tupen"] [?e :name ?n]] @conn)))))   ; #{["Ivan"]}
```

Where compiling may block, in Node or a worker, `(wasm/instantiate-sync bytes)` loads it at once. A program that makes
databases while its namespaces load, `(defonce conn (d/create-conn …))`, has to have the module loaded before those
namespaces are, which a browser's main thread allows only for an instance made elsewhere first; such a program makes
its connection after `instantiate` instead.

It compiles under `:advanced`. A datom's fields are JavaScript's properties, `d.e`, `d.a`, `d.v`, `d.tx`, as
DataScript's are, and the externs for them come with the sources (`deps.cljs`); shadow-cljs wants them named, as it
does for DataScript: `:compiler-options {:externs ["datascript/externs.js"]}`.

### What is the same

Everything `datascript.core` has: databases as values, `with` and `db-with`, connections with `transact!` and
listeners, queries with rules, aggregates, predicates, functions and pull, the pull API, entities, filtered
databases, index access, `serializable`, the `#datascript/DB` and `#datascript/Datom` readers, and DataScript's
JavaScript API (`datascript.js`). DataScript's own tests of its public API run on it, 157 of them, with `:simple` and
with `:advanced` (`cljs/test.sh`), and so do its tests of the JavaScript API (`cljs/js-api.sh`).

A function of the program's is called from the module wherever DataScript calls one: a predicate or a function in a
query, an aggregate, a transaction function, a filter's predicate, a pull's `:xform` or `:visitor`. What it throws is
what the caller of DataScript is thrown, the same exception.

### What crosses the boundary, and how

The database is in the module, and a value on its way in or out is written into the module's memory or read from it.

- **Numbers, strings, booleans, keywords, symbols, vectors, lists, maps, sets, UUIDs, dates, regular expressions and
  datoms** are the module's own values, and come back as equal ClojureScript ones. A map or a set keeps the order
  ClojureScript iterates it in.
- **Any other value** stays where it is: the module holds a handle to it, and what comes back is the same object. A
  record, an atom, a JavaScript object or an entity can be a datom's value or a query's input. What ClojureScript asks
  of such a value's own protocols, its hash, `=`, `compare`, `get`, `count`, `seq`, `contains?`, calling it, and
  printing it, the module asks the host. A record is a map to a transaction.
- **A database value** is a handle to the database in the module. The same database value is the same object for as
  long as the program holds it. When the garbage collector finds it unreachable, the module is told to let the
  database go (`FinalizationRegistry`).
- **JavaScript arrays** are vectors to the module.
- A value nested deeper than the stack has room for, as a recursive pull makes, crosses without recursion.

The module runs on its host's stack. When that runs out under it, in a recursion of the program's own that reads the
database at every level, or over a value nested too deep to hash or print, JavaScript's `RangeError` is what the
program is thrown, as it would be by ClojureScript DataScript, and the databases the program holds are as they were
before the call. The engine ends the call where it stands, with nothing unwound, so that is by how the module is
written: it keeps nothing locked, what it keeps between calls changes by one value stored at a time, and its
allocator (`crates/datascript-wasm/src/heap.rs`) calls nothing while its own lists are half changed, which the
standard library's does. `cljs/overflow-test.js` runs the stack out under the module some hundreds of times, at a
different place each time, and the module's memory is all that shows it: what a call that was cut short had allocated
is not freed, and a database it was reading is not let go of, though the program lose every other hold on it.

### What differs

- A database value is not a record of its indexes: `(:eavt db)` is the datoms of the index, in order, and not the
  sorted set DataScript keeps them in; `assoc` on a database is not supported. `(:schema db)`, `(:rschema db)`,
  `(:max-eid db)` and `(:max-tx db)` are what they are in DataScript.
- What is inside DataScript and not part of its API is not here: `datascript.parser`, `datascript.pull-parser`,
  `datascript.built-ins`, `datascript.lru`, and the experimental `datascript.query-v3`.
- The module is told of an unreachable database value when the garbage collector's finalizers run, which is between
  tasks, and not during one. A long loop that makes a database value a turn, without yielding, keeps them all in the
  module until it ends, and WebAssembly's memory does not shrink afterwards. A connection's are kept small: the value
  `transact!` moves on from stays in the module as the new value with the transaction undone, and is made whole
  again if it is read. It costs the module some two hundred bytes, where a value kept whole, as each of a chain of
  `with` or `db-with` is, costs some six thousand: a million small transactions in a row on a connection, with never
  a turn of the event loop, leave the module at 200 MB. One transaction of many entities is the way to load much
  data, as it is in DataScript.
- A run of datoms (`datoms`, `seek-datoms`, `index-range`) is read from the module a part at a time, the first part
  at once. A filtered database's predicate is therefore asked of the first few dozen datoms of a run when the run is
  made, and not as they are read.
- A string with half of a surrogate pair in it does not survive the crossing: the module's strings are UTF-8.
- Where values of different types meet under one attribute, DataScript orders them by the names of their types. The
  names here are the ones an unoptimized ClojureScript build has; under `:advanced`, ClojureScript DataScript's own
  are whatever the compiler left of the constructors.

## From JavaScript, or any other WebAssembly host

The module is `wasm32-unknown-unknown` with no bindings generated: it exports a handful of functions and imports
five, from a module named `datascript`.

**DataScript's JavaScript API.** `cljs/js-api.sh` packs `datascript.js` as DataScript's npm release is packed,
over the module: 250 KB, 48 KB at brotli's best, beside the module. A program loads the module once, and the API is
DataScript's from there:

```js
const d = require('./datascript.js');
await d.instantiate(fetch('datascript.wasm'));        // or d.instantiate_sync(bytes)
const conn = d.create_conn({ aka: { ':db/cardinality': ':db.cardinality/many' } });
d.transact(conn, [{ ':db/id': -1, name: 'Ivan', aka: ['Devil', 'Tupen'] }]);
d.q('[:find ?n :where [?e "aka" "Tupen"] [?e "name" ?n]]', d.db(conn));   // [["Ivan"]]
```

**As EDN.** `ds_edn(ptr, len)` takes one operation as EDN text and answers its value as EDN text.
`crates/datascript-wasm/js/datascript-edn.mjs` is a complete host in ninety lines, and the one to read before writing
another:

```js
import { instantiate } from './datascript-edn.mjs';
const ds = await instantiate(await fetch('datascript.wasm'));
const db0 = ds.call('[:empty-db {:aka {:db/cardinality :db.cardinality/many}}]');     // "#datascript/handle 0"
const db = ds.call(`[:db-with ${db0} [{:db/id -1 :name "Ivan" :aka ["Devil" "Tupen"]}]]`);
ds.call(`[:q [:find ?n :where [?e :aka "Tupen"] [?e :name ?n]] ${db}]`);              // '#{["Ivan"]}'
ds.call(`[:pull ${db} [:name :aka] 1]`);                                               // '{:aka ["Devil" "Tupen"], :name "Ivan"}'
```

The operations are `:empty-db`, `:init-db`, `:db-with`, `:with`, `:q`, `:pull`, `:pull-many`, `:datoms`,
`:seek-datoms`, `:rseek-datoms`, `:index-range`, `:entid`, `:entity`, `:schema`, `:count`, `:serializable`,
`:from-serializable`, `:db-string`, `:read-db` and `:release` (`crates/datascript-wasm/src/edn_api.rs`). A database is
`#datascript/handle n`, and stays in the module until it is released.

**As values.** `ds_call(op, ptr, len)` takes an operation's number and its arguments in the module's binary form, and
answers likewise. This is what the ClojureScript interface speaks; with it a host lends the module its own functions
and values, and reads long runs of datoms a part at a time. The form of a value is in
`crates/datascript-wasm/src/codec.rs`, the operations in `ops.rs`, what the module asks of its host in `host.rs`, and
the whole of it is described at the top of `lib.rs`. `cljs/src/datascript/wasm.cljs` is a host of it.

## From Rust

```rust
use datascript::{db_with, edn, pull, q, Db, Value};

let schema = edn::read_string("{:aka {:db/cardinality :db.cardinality/many}}")?;
let db = db_with(&Db::empty(schema)?, &edn::read_string(r#"[{:db/id -1 :name "Ivan" :aka ["Devil" "Tupen"]}]"#)?)?;
let found = q(&edn::read_string(r#"[:find ?n :where [?e :aka "Tupen"] [?e :name ?n]]"#)?, &[Value::Db(db.clone())])?;
assert_eq!(datascript::print::pr_str(&found), r#"#{["Ivan"]}"#);
let ivan = pull(&db, &edn::read_string("[:name :aka]")?, &Value::from(1), None)?;
assert_eq!(datascript::print::pr_str(&ivan), r#"{:aka ["Devil" "Tupen"], :name "Ivan"}"#);
```

(`cargo run -p datascript --example hello`)

Values are ClojureScript's (`Value`): numbers are doubles, and maps and sets iterate in ClojureScript's order.
`datascript::Conn` is a connection, `datascript::entity` an entity, `Db::filter` a filtered database, and
`datascript::serialize` DataScript's serializable form.

### A Rust program in one module with it

A Rust program for WebAssembly that has `datascript` and `datascript-wasm` among its crates is one module with
them: its exports are the module's and its own, and its memory is the module's. To its host it is DataScript, whole,
for `datascript.core` or any other interface over the module; and it reads and transacts the databases that host
holds by calling `datascript` as above, with nothing encoded and no boundary crossed.

```rust
use datascript::{Index, Result, Value};
use datascript_wasm::codec::Writer;

fn answer(op: u32, args: &[Value], w: &mut Writer) -> Result<()> {
    // [db attr] → how many datoms the database the host named has of the attribute
    let [Value::Db(db), attr] = args else { return Err(datascript::Error::msg("a database, and an attribute")) };
    w.value(&Value::from(db.datoms(Index::Aevt, std::slice::from_ref(attr))?.count()?));
    Ok(())
}

#[no_mangle]
pub extern "C" fn start() {
    datascript_wasm::start();          // the module first
    datascript_wasm::extend(answer);   // then what answers the program's operations, from ops::EXTENSION up
}
```

```clojure
(wasm/call 1000 #js [@conn :name])   ; the host calls them as the interface calls the module's own
```

- **Databases** cross as what they are. One the host names in a message is that database in the program's hands
  (`Value::Db`), and one the program answers with is a database value to the host, held and let go of as any other.
- **Functions of the host's** in a message are functions the program calls (`built_ins::call`), with their
  arguments and answers in the module's form.
- **A connection the host holds** is transacted on from the program with two such functions: one that answers the
  database the connection holds, and one the program calls with the report of the transaction it made of that
  database (`datascript::advance`), for the host to move its connection on and tell its listeners. In ClojureScript
  the second is `datascript.conn/-moved-on!`.
- **Linking.** The program's build script asks for the module's stack and for `__stack_pointer` to be exported, as
  `crates/datascript-wasm/build.rs` does: what a build script asks for is asked of its own crate alone.

`crates/datascript-wasm/examples/embedded.rs` is such a program, with that operation and one that transacts on a
connection of its host's; `./cljs/test.sh simple embedded` runs DataScript's tests on it, and its own operations.

## How it is checked

```bash
./script/test_rust.sh              # all of the below, the lints, and the allocator's functions read in the built module
cargo test --workspace             # the crates' own tests
./conformance/run.sh               # the Rust library against ClojureScript DataScript
./conformance/run-wasm.sh          # the module, behind its ClojureScript interface, against ClojureScript DataScript
./cljs/test.sh                     # DataScript's own tests on the module; ./cljs/test.sh advanced under :advanced
./cljs/test.sh simple embedded     # the same on the module built into another program, and that program's operations
./cljs/js-api.sh                   # DataScript's JavaScript API over the module, and its own tests of it
node crates/datascript-wasm/js/test-edn.mjs   # the module through its EDN interface
```

**The harness.** `conformance/cases` holds cases, each a line of EDN: steps that make databases, transact, query,
pull, read indexes, walk entities, filter, serialize, and ask ClojureScript itself for hashes, orders and printed
forms. The oracle (`conformance/oracle`) is ClojureScript DataScript, this repository's `src/`, compiled for Node; it
prints every step's answer with `pr-str`, so that the order of every set, map and sequence shows, and every error
with its message and data. The Rust runner prints the same steps, and the two outputs are compared line by line.
14,543 steps are in the repository, a third of them from a generator of random transactions and queries
(`conformance/gen/fuzz.py`), and all of them answer alike. The same cases run through the WebAssembly module and its
ClojureScript interface, which checks what crosses the boundary.

**DataScript's tests.** The tests of DataScript's public API in `test/` are compiled against `cljs/src` and run on
the module in Node, with `:simple` and `:advanced` optimizations, beside tests of what is particular to the boundary
(`cljs/test/datascript/test/wasm.cljs`): values that keep their identity, exceptions, long runs of datoms, values
nested 20,000 deep, the stack running out and the module carrying on, a database as JSON text, and databases being
let go of when the garbage collector says so. The same tests run on the module built into another program
(`crates/datascript-wasm/examples/embedded.rs`), with that program's operations: a database read where it is, and a
connection held in ClojureScript transacted on from Rust, its listeners told what `transact!` would have told them
(`cljs/test/datascript/test/embedded.cljs`).

**The allocator.** `crates/datascript-wasm/js/leaf-check.mjs` reads the built module, and passes when the two
functions that take memory and give it back call nothing and everything else that allocates calls only them: what
`crates/datascript-wasm/src/heap.rs` promises of them. `cljs/overflow-test.js` is the other half of that: the stack
run out under the module some hundreds of times, in a different place each time.

## How much memory it takes

The same as ClojureScript DataScript, for the database itself. 100,000 entities of six attributes, 600,000 datoms,
transacted ten thousand entities at a time through DataScript's JavaScript API, in Node:

| | ClojureScript's heap | the module's memory |
|---|---:|---:|
| the database loaded | 103 MB | 103 MB |
| after 50,000 transactions of one datom | 101 MB | 107 MB |
| after `serializable`, 19 MB of JSON | 117 MB | 159 MB |
| after `from-serializable` of it, a second database | 204 MB | 227 MB |

The module's memory is the most it has needed at once: WebAssembly's memory grows and is never given back. What
`serializable` takes while it writes, the text and where each datom is in the other two indexes, stays the module's
afterwards, free for whatever the module needs next; the text itself is the program's, 24 MB of its own heap.

A node of an index holds what it has and room for one datom more, where a vector that doubles would have left the
indexes half empty: the database above took 149 MB before its nodes were made to fit. A database is written out as
it is read, a datom at a time, and read back a row at a time, with nothing made of the whole of it in between:
`serializable` left the module at 298 MB before that.

## How fast it is

`cljs/bench.sh` compiles one program twice, against ClojureScript DataScript and against the module, with
`:advanced`, and times it in Node. Microseconds a call, the lesser of three runs, on a database of 20,000 entities
and 180,000 datoms, on a laptop:

| | ClojureScript | WebAssembly | ratio |
|---|---:|---:|---:|
| transact 20,000 entities, ms | 747 | 324 | 0.43 |
| `(first (d/datoms db :eavt e :name))` | 0.79 | 1.32 | 1.66 |
| `(vec (d/datoms db :eavt e))` | 0.95 | 2.08 | 2.20 |
| `find-datom` | 0.71 | 0.96 | 1.35 |
| `seek-datoms`, the first three | 0.83 | 2.61 | 3.13 |
| `index-range`, ten datoms | 2.43 | 7.37 | 3.03 |
| `entid` of a lookup ref | 2.36 | 1.34 | 0.57 |
| an entity's attribute | 1.51 | 2.57 | 1.70 |
| an entity, touched | 7.36 | 9.52 | 1.29 |
| `pull`, two attributes | 2.11 | 1.88 | 0.89 |
| `pull`, wildcard | 8.03 | 4.06 | 0.51 |
| `q`, one entity's attribute | 30.6 | 4.32 | 0.14 |
| `q`, a join of about 20 rows | 1653 | 990 | 0.60 |
| `q`, a predicate over 20,000 | 3257 | 629 | 0.19 |
| `q`, with a function of the program's over 20,000 | 1051 | 1737 | 1.65 |
| `with`, one datom | 8.35 | 8.40 | 1.01 |
| `with`, an entity of five attributes | 33.6 | 15.3 | 0.45 |
| `transact!`, one datom, with a listener | 3.75 | 2.72 | 0.73 |
| all 180,000 datoms, counted | 3189 | 554 | 0.17 |
| all 180,000 datoms, reduced over | 3642 | 15841 | 4.35 |
| a filtered database's datoms of an entity | 1.72 | 4.33 | 2.52 |
| `serializable`, all 180,000 datoms | 11858 | 35605 | 3.00 |
| `from-serializable`, the same | 12048 | 32701 | 2.71 |
| the database to JSON text | 25297 | 21986 | 0.87 |
| the database from JSON text | 31199 | 27781 | 0.89 |

What the module does inside, queries, pulls and transactions, is up to seven times faster. A read of a few datoms
pays for the crossing, a microsecond or two; a function of the program's that a query calls for every row pays for it
every row; and reading every datom of a database into ClojureScript pays for making each one again there.

`serializable` pays for it too: the module writes the database as JSON text, and the text is parsed into the
JavaScript data DataScript answers with. A program that keeps its database as text has no use for the data in
between, and `datascript.serialize/json` and `from-json` (`serializable_json` and `from_json` in JavaScript) are the
two without it: the text `JSON.stringify` makes of `serializable`, and the database of such a text. Those are the
last two rows, against `JSON.stringify` of `serializable` and `from-serializable` of `JSON.parse` in ClojureScript.

The module is 1.0 MB, 250 KB at brotli's best; built for size (`opt-level = "z"`), 0.85 MB and 215 KB. The
ClojureScript interface adds less to a program than DataScript itself does.

## Where the port knowingly differs from ClojureScript DataScript

The harness holds the port to DataScript's behaviour, its accidents included: the order of results, what an
unbound variable in a predicate raises, how many times a filter's predicate is asked. These are the places where the
port does something else on purpose:

- **Keyword and string attributes in one database** are refused. DataScript compares attributes with `compare`, which
  throws between a keyword and a string, so such a database works or fails by the order its datoms happen to meet in.
- **An entity id with a fraction** is refused by a transaction. Reads by one find nothing, as in DataScript.
- **`[(array-map …) ?m]`** in a query binds each row its own map. In ClojureScript every row gets the last row's,
  since the map is built over the engine's argument buffer.
- **A wildcard pull over an attribute named like a reverse reference**, `:_x`, which `[:db/add e :_x v]` can make,
  pulls it as the attribute it is. DataScript does not return.
- **`touch` of components that lead back to their owner** raises "Maximum call stack size exceeded" at a depth of
  1,000, where ClojureScript runs out of stack.
- **A query's predicates** are applied to a relation at once, where ClojureScript's lazy sequences apply them a chunk
  at a time. It shows only in a predicate with side effects.
- **A set of more than eight entities** is ordered by the entities' hashes, which in ClojureScript come from an id
  the runtime gives each database object, and are a run's own.
- **Lists, vectors and other collections as values of one indexed attribute** are not ordered consistently by
  DataScript's comparator, so where such a datom lands depends on how the index was built. The port's index is not
  the same tree, and may land it elsewhere.
- **The numbers of the tempids DataScript makes up** for entities without an id appear in some error messages, and
  come from a counter the port draws on more often.
- `datascript.query-v3` and the JVM's storage are not ported.
