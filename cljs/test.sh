#!/bin/bash
# DataScript's own ClojureScript tests (test/), run against the WebAssembly module in Node.
#   ./cljs/test.sh                   compiled with :simple optimizations
#   ./cljs/test.sh advanced          compiled with :advanced, as a release build is
#   ./cljs/test.sh simple embedded   against the module built into another program
#                                    (crates/datascript-wasm/examples/embedded.rs), whose own operations
#                                    datascript.test.embedded then tries too
set -o errexit -o nounset -o pipefail
cd "$(dirname "$0")/.."

OPT="${1:-simple}"
MODULE="${2:-module}"
CLOJURESCRIPT="${CLOJURESCRIPT:-1.12.145}"
case "$MODULE" in
  module)   BUILD=(-p datascript-wasm); WASM="target/wasm32-unknown-unknown/wasm-release/datascript_wasm.wasm" ;;
  embedded) BUILD=(-p datascript-wasm --example embedded); WASM="target/wasm32-unknown-unknown/wasm-release/examples/embedded.wasm" ;;
  *) echo "test.sh: the module is 'module' or 'embedded', not $MODULE" >&2; exit 2 ;;
esac
DEPS="{:paths [\"cljs/src\" \"cljs/test\" \"test\"]
       :deps {org.clojure/clojurescript {:mvn/version \"$CLOJURESCRIPT\"}
              io.github.tonsky/extend-clj {:mvn/version \"0.1.0\"}
              com.cognitect/transit-cljs {:mvn/version \"0.8.269\"}}}"

cargo build "${BUILD[@]}" --target wasm32-unknown-unknown --profile wasm-release

clojure -Sdeps "$DEPS" -M -m cljs.main \
  -t node -O "$OPT" \
  -d "cljs/target/test-$OPT/out" \
  -o "cljs/target/test-$OPT/test.js" \
  -c datascript.wasm-test

DATASCRIPT_WASM="$WASM" node --expose-gc "cljs/target/test-$OPT/test.js"
