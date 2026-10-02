#!/bin/bash
# DataScript's JavaScript API over the WebAssembly module: cljs/target/js/datascript.js, packed as DataScript's own
# release is (release-js/), and DataScript's own JavaScript tests (test/js/tests.js) run on it in Node.
#   const d = require('./datascript.js');
#   await d.instantiate(fetch('datascript.wasm'));      // or d.instantiate_sync(bytes)
set -o errexit -o nounset -o pipefail
cd "$(dirname "$0")/.."

CLOJURESCRIPT="${CLOJURESCRIPT:-1.12.145}"
WASM="target/wasm32-unknown-unknown/wasm-release/datascript_wasm.wasm"
DEPS="{:paths [\"cljs/src\"]
       :deps {org.clojure/clojurescript {:mvn/version \"$CLOJURESCRIPT\"}
              io.github.tonsky/extend-clj {:mvn/version \"0.1.0\"}}}"

cargo build -p datascript-wasm --target wasm32-unknown-unknown --profile wasm-release

mkdir -p cljs/target/js
clojure -Sdeps "$DEPS" -M -m cljs.main \
  -O advanced -co '{:output-wrapper false :elide-asserts true}' \
  -d cljs/target/js/out \
  -o cljs/target/js/datascript.bare.js \
  -c datascript.js
cat release-js/wrapper.prefix cljs/target/js/datascript.bare.js release-js/wrapper.suffix > cljs/target/js/datascript.js
echo "cljs/target/js/datascript.js ($(wc -c < cljs/target/js/datascript.js | tr -d ' ') bytes)"

DATASCRIPT_WASM="$WASM" node cljs/js-api-test.js
