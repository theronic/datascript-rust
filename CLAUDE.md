# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

DataScript is an immutable in-memory database and Datalog query engine implemented in Clojure and ClojureScript. It's designed to run in the browser and JVM, providing a lightweight, ephemeral database with Datalog querying capabilities similar to Datomic.

## Key Commands

### Testing

Prefer Clojure JVM tests for correctness (much faster), check cljs and js only after test_clj.sh passes.

```bash
# Run Clojure tests
./script/test_clj.sh
# or: clojure -X:dev user/-test-main

# Run ClojureScript tests
./script/test_cljs.sh

# Run all tests
./script/test_all.sh

# Run Datomic compatibility tests (requires datomic-free setup)
./script/test_datomic.sh
# or: clj -M:datomic
```

### Benchmarking

```bash
# Run Clojure benchmarks
./script/bench_clj.sh
# or: clojure -A:bench -M -m datascript.bench.datascript

# Run ClojureScript benchmarks
./script/bench_cljs.sh

# Run Datomic benchmarks (requires datomic-free setup)
./script/bench_datomic.sh

# Run all benchmarks
./script/bench_all.sh
```

### Development

```bash
# Start a REPL
./script/repl.sh

# Build ClojureScript release
lein with-profile test cljsbuild once release
```

## Architecture Overview

### Core Components

**Database Layer (`datascript.db`):**
- `Datom` records represent facts as [entity attribute value transaction] tuples
- `DB` is the immutable database value with three indexes: EAVT, AEVT, AVET
- Uses persistent sorted sets for index storage (`me.tonsky.persistent-sorted-set`)
- Transaction IDs start at `0x20000000` (tx0), entity IDs start at 0 (e0)
- Schema is optional and only needed for special behaviors (cardinality many, unique attributes, ref types)

**Query Engine (`datascript.query`):**
- Implements Datalog query language
- Queries are parsed, cached (LRU cache of 100), and executed against relations
- Supports rules, aggregates, predicates, negation, and multiple data sources
- `Relation` records store query intermediate results as {attrs tuples}
- `Context` holds relations, sources, and rules during query execution

**Connection Management (`datascript.conn`):**
- Connections are atoms holding the latest DB value
- Access current DB with `@conn`, not `(d/db conn)` as in Datomic
- Transaction reports include `:db-before`, `:db-after`, `:tx-data`, `:tempids`, `:tx-meta`
- Supports transaction listeners via `listen!`

**Entity API (`datascript.impl.entity`):**
- Lazy map-like entities for navigating database
- Entities cache accessed attributes
- Reverse references via `_` prefix (`:_ref`)
- Component references return single entity instead of collection

**Storage (`datascript.storage`):**
- Protocol-based persistent storage for databases
- Serializes indexes as persistent sorted sets with addressable chunks
- Supports custom storage backends via `IStorage` protocol

**Pull API (`datascript.pull-api`, `datascript.pull-parser`):**
- EQL-style pattern-based entity graph extraction
- Supports recursive patterns, attribute limits, default values

### Key Differences from Datomic

- Entities use `-1`, `-2` for tempids instead of `#db/id[:db.part/user -100]`
- No `:db/txInstant` annotations by default
- Custom query functions in ClojureScript must be passed as source (no `resolve`)
- Transaction functions called as `[:db.fn/call f args]` where `f` is function reference
- Additional `:db.fn/retractAttribute` built-in function
- Schemas are not queryable and attributes don't need pre-declaration

### File Organization

- `src/datascript/` - Core implementation (.cljc files for JVM + JS)
  - `core.cljc` - Public API entry point
  - `db.cljc` - Database, datom, and index implementation
  - `query.cljc` - Datalog query engine
  - `pull_api.cljc`, `pull_parser.cljc` - Pull API
  - `storage.clj/.cljs` - Persistent storage (platform-specific)
  - `conn.cljc` - Connection and transaction handling
  - `parser.cljc` - Query/rule parsing
  - `built_ins.cljc` - Built-in query functions and aggregates

- `test/datascript/test/` - Test suite (.cljc files)
  - Tests use `.cljc` files for cross-platform testing
  - Acceptance tests demonstrate usage: `test/datascript/test/*.cljc`

- `bench/` - Benchmarking code
- `bench_datomic/` - Datomic compatibility checks
- `script/` - Build and test scripts

### Development Notes

- Uses `.cljc` files for cross-platform (JVM + ClojureScript) code
- Custom macros in `datascript.db` like `defn+` handle CLJS efficiency
- `*warn-on-reflection*` enabled for JVM performance
- Storage implementation differs between Clojure (`.clj`) and ClojureScript (`.cljs`)
- shadow-cljs users must add `:compiler-options {:externs ["datascript/externs.js"]}`

### Aliases (deps.edn)

- `:dev` - Development with REPL tools
- `:test` - Test dependencies
- `:bench` - Benchmarking tools (criterium, profiler)
- `:datomic` - Datomic compatibility testing
- `:cljs` - ClojureScript compilation
- `:1.9`, `:1.10`, `:1.11.1`, `:1.12` - Override Clojure versions

## The Rust port

This fork ports DataScript to Rust for WebAssembly (docs/rust.md). `src/` is the port's reference and is not changed
by it.

- `crates/datascript` - the database, a Rust library; `crates/datascript-wasm` - the WebAssembly module;
  `cljs/` - `datascript.core` over the module; `crates/conformance`, `conformance/` - the harness.
- The port answers what ClojureScript DataScript answers, in its order, with its errors. A change to the port is
  checked against the oracle, never against what seems right.

```bash
cargo test --workspace            # the crates' tests
./conformance/run.sh              # the Rust library against ClojureScript DataScript
./conformance/run-wasm.sh         # the module behind its ClojureScript interface, likewise
./cljs/test.sh                    # DataScript's own tests on the module (./cljs/test.sh advanced too)
./cljs/js-api.sh                  # DataScript's JavaScript API and its tests on the module
./cljs/bench.sh                   # ClojureScript DataScript and the module, timed side by side
./script/test_rust.sh             # all of the checks above
```
