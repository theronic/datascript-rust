#!/bin/bash
# Everything that checks the Rust port (docs/rust.md): the crates' tests and lints, the port against ClojureScript
# DataScript as a library and as the WebAssembly module, DataScript's own ClojureScript and JavaScript tests on the
# module, and the module through its EDN interface.
set -o errexit -o nounset -o pipefail
cd "$(dirname "$0")/.."

cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo run -q -p datascript --example hello > /dev/null
RUSTFLAGS='-D warnings' cargo build --locked -p datascript-wasm --target wasm32-unknown-unknown --profile wasm-release

./conformance/run.sh
./conformance/run-wasm.sh
./cljs/test.sh
./cljs/test.sh advanced
./cljs/js-api.sh
node crates/datascript-wasm/js/test-edn.mjs
echo "the Rust port: all checks passed"
