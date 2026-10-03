#!/bin/bash
# The calls a program makes of DataScript, timed over ClojureScript DataScript and over the WebAssembly module: the same
# program (cljs/bench/src) compiled with :advanced against each, run in Node, microseconds a call.
set -o errexit -o nounset -o pipefail
cd "$(dirname "$0")/.."

CLOJURESCRIPT="${CLOJURESCRIPT:-1.12.145}"
WASM="target/wasm32-unknown-unknown/wasm-release/datascript_wasm.wasm"
cargo build -p datascript-wasm --target wasm32-unknown-unknown --profile wasm-release

build() { # name, paths, deps
  clojure -Sdeps "{:paths [$2 \"cljs/bench/src\"] :deps {org.clojure/clojurescript {:mvn/version \"$CLOJURESCRIPT\"} io.github.tonsky/extend-clj {:mvn/version \"0.1.0\"} $3}}" \
    -M -m cljs.main -t node -O advanced -d "cljs/target/bench-$1/out" -o "cljs/target/bench-$1/bench.js" -c datascript.boundary-bench 2>&1 \
    | grep -v '^WARNING: \(A terminally\|sun.misc\|Please consider\)' || true
}
build upstream '"src" "cljs/bench/upstream"' 'persistent-sorted-set/persistent-sorted-set {:mvn/version "0.3.1"}'
build wasm '"cljs/src" "cljs/bench/wasm"' ''

# each three times, the best kept: the machine is seldom this program's alone
for i in 1 2 3; do
  node --expose-gc cljs/target/bench-upstream/bench.js > "cljs/target/bench-upstream-$i.json"
  DATASCRIPT_WASM="$WASM" node --expose-gc cljs/target/bench-wasm/bench.js > "cljs/target/bench-wasm-$i.json"
done
node -e '
const fs = require("fs");
const runs = (n) => [1, 2, 3].map((i) => JSON.parse(fs.readFileSync(`cljs/target/bench-${n}-${i}.json`, "utf8")));
const a = runs("upstream"), b = runs("wasm");
const f = (x) => x >= 100 ? x.toFixed(0) : x >= 10 ? x.toFixed(1) : x.toFixed(2);
console.log("ClojureScript".padStart(14) + "WebAssembly".padStart(14) + "  ratio  (microseconds a call)");
for (const k of Object.keys(a[0])) {
  const x = Math.min(...a.map((r) => r[k])), y = Math.min(...b.map((r) => r[k]));
  console.log(f(x).padStart(14) + f(y).padStart(14) + (y / x).toFixed(2).padStart(7) + "  " + k);
}
'
