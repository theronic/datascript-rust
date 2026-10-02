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
program is thrown, as it would be by ClojureScript DataScript, and the module is as it was before the call: it keeps
nothing locked or half set that a call cut short could leave so, and the interface puts its stack back
(`cljs/overflow-test.js` runs the stack out under it some thousand times).

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
`crates/datascript-wasm/js/datascript-edn.mjs` is a complete host in sixty lines, and the one to read before writing
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

## How it is checked

```bash
cargo test --workspace             # the crates' own tests
./conformance/run.sh               # the Rust library against ClojureScript DataScript
./conformance/run-wasm.sh          # the module, behind its ClojureScript interface, against ClojureScript DataScript
./cljs/test.sh                     # DataScript's own tests on the module; ./cljs/test.sh advanced under :advanced
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
nested 20,000 deep, the stack running out and the module carrying on, and databases being let go of when the garbage
collector says so.

## How fast it is

`cljs/bench.sh` compiles one program twice, against ClojureScript DataScript and against the module, with
`:advanced`, and times it in Node. Microseconds a call, the lesser of three runs, on a database of 20,000 entities
and 180,000 datoms, on a laptop:

| | ClojureScript | WebAssembly | ratio |
|---|---:|---:|---:|
| transact 20,000 entities, ms | 806 | 337 | 0.42 |
| `(first (d/datoms db :eavt e :name))` | 0.88 | 1.45 | 1.66 |
| `(vec (d/datoms db :eavt e))` | 1.00 | 2.31 | 2.31 |
| `find-datom` | 0.79 | 1.03 | 1.29 |
| `seek-datoms`, the first three | 0.92 | 2.83 | 3.09 |
| `index-range`, ten datoms | 2.73 | 8.40 | 3.08 |
| `entid` of a lookup ref | 2.64 | 1.40 | 0.53 |
| an entity's attribute | 1.60 | 2.78 | 1.74 |
| an entity, touched | 8.15 | 10.4 | 1.27 |
| `pull`, two attributes | 2.26 | 2.14 | 0.94 |
| `pull`, wildcard | 8.60 | 5.07 | 0.59 |
| `q`, one entity's attribute | 33.1 | 4.96 | 0.15 |
| `q`, a join of about 20 rows | 2117 | 1113 | 0.53 |
| `q`, a predicate over 20,000 | 3584 | 678 | 0.19 |
| `q`, with a function of the program's over 20,000 | 1144 | 1815 | 1.59 |
| `with`, one datom | 9.22 | 9.06 | 0.98 |
| `with`, an entity of five attributes | 36.9 | 16.7 | 0.45 |
| `transact!`, one datom, with a listener | 3.87 | 2.94 | 0.76 |
| all 180,000 datoms, counted | 3402 | 585 | 0.17 |
| all 180,000 datoms, reduced over | 4777 | 16764 | 3.51 |
| a filtered database's datoms of an entity | 1.97 | 5.05 | 2.57 |

What the module does inside, queries, pulls and transactions, is up to seven times faster. A read of a few datoms
pays for the crossing, a microsecond or two; a function of the program's that a query calls for every row pays for it
every row; and reading every datom of a database into ClojureScript pays for making each one again there.

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
