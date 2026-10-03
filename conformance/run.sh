#!/bin/bash
# Runs conformance cases through ClojureScript DataScript (the oracle) and through the Rust port, and compares.
#   conformance/run.sh [cases.edn ...]     default: every file in conformance/cases
set -o errexit -o nounset -o pipefail
cd "$(dirname "$0")/.."

if [ ! -f conformance/oracle/target/oracle.js ] || [ -n "$(find src conformance/oracle/src -newer conformance/oracle/target/oracle.js -name '*.clj*' -print -quit)" ]; then
  conformance/oracle/build.sh >&2
fi
cargo build --quiet --release -p conformance

mkdir -p conformance/out
status=0
for cases in "${@:-conformance/cases/*.edn}"; do
  for f in $cases; do
    name="$(basename "$f" .edn)"
    node conformance/oracle/target/oracle.js "$f" "conformance/out/$name.oracle"
    "${CARGO_TARGET_DIR:-target}/release/conformance" run "$f" "conformance/out/$name.rust"
    printf '%s: ' "$name"
    "${CARGO_TARGET_DIR:-target}/release/conformance" compare "conformance/out/$name.oracle" "conformance/out/$name.rust" "$f" || status=1
  done
done
exit $status
