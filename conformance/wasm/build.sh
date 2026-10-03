#!/bin/bash
# Compiles the conformance harness over the WebAssembly module: the oracle's program (conformance/oracle) with
# cljs/src in place of ClojureScript DataScript, into conformance/wasm/target/harness.js, for Node.
set -o errexit -o nounset -o pipefail
cd "$(dirname "$0")/../.."

CLOJURESCRIPT="${CLOJURESCRIPT:-1.12.145}"
OPT="${OPT:-simple}"
DEPS="{:paths [\"cljs/src\" \"conformance/wasm/src\" \"conformance/oracle/src\"]
       :deps {org.clojure/clojurescript {:mvn/version \"$CLOJURESCRIPT\"}
              io.github.tonsky/extend-clj {:mvn/version \"0.1.0\"}}}"

cargo build -p datascript-wasm --target wasm32-unknown-unknown --profile wasm-release

mkdir -p conformance/wasm/target
clojure -Sdeps "$DEPS" -M -m cljs.main \
  -t node -O "$OPT" \
  -d conformance/wasm/target/out \
  -o conformance/wasm/target/harness.js \
  -c oracle.core
echo "harness: conformance/wasm/target/harness.js ($(wc -c < conformance/wasm/target/harness.js | tr -d ' ') bytes)"
