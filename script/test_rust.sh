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
# the module once more with its names, which is how its allocator's functions are found and read
CARGO_PROFILE_WASM_RELEASE_STRIP=none RUSTFLAGS='-D warnings' cargo build --locked -p datascript-wasm --target wasm32-unknown-unknown --profile wasm-release --target-dir target/named
node crates/datascript-wasm/js/leaf-check.mjs target/named/wasm32-unknown-unknown/wasm-release/datascript_wasm.wasm

./conformance/run.sh
./conformance/run-wasm.sh
./cljs/test.sh
./cljs/test.sh advanced
./cljs/js-api.sh
node crates/datascript-wasm/js/test-edn.mjs
echo "the Rust port: all checks passed"
