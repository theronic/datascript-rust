#!/bin/bash
# Runs conformance cases through ClojureScript DataScript (the oracle) and through the WebAssembly module behind its
# ClojureScript interface (cljs/), and compares. What crosses the module's boundary is what is checked here; the
# database itself is checked by conformance/run.sh.
#   conformance/run-wasm.sh [cases.edn ...]     default: every file in conformance/cases
set -o errexit -o nounset -o pipefail
cd "$(dirname "$0")/.."

if [ ! -f conformance/oracle/target/oracle.js ] || [ -n "$(find src conformance/oracle/src -newer conformance/oracle/target/oracle.js -name '*.clj*' -print -quit)" ]; then
  conformance/oracle/build.sh >&2
fi
conformance/wasm/build.sh >&2
cargo build --quiet --release -p conformance

mkdir -p conformance/out
status=0
for cases in "${@:-conformance/cases/*.edn}"; do
  for f in $cases; do
    name="$(basename "$f" .edn)"
    node conformance/oracle/target/oracle.js "$f" "conformance/out/$name.oracle"
    node conformance/wasm/target/harness.js "$f" "conformance/out/$name.wasm"
    printf '%s: ' "$name"
    "${CARGO_TARGET_DIR:-target}/release/conformance" compare "conformance/out/$name.oracle" "conformance/out/$name.wasm" "$f" || status=1
  done
done
exit $status
