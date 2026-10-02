(ns datascript.bench-setup
  "The WebAssembly module, loaded before the benchmark asks anything of it."
  (:require
    [datascript.wasm :as wasm]
    [goog.object :as gobj]))

(wasm/instantiate-sync
  (js-invoke (js/require "fs") "readFileSync"
    (or (gobj/getValueByKeys js/process "env" "DATASCRIPT_WASM")
      "target/wasm32-unknown-unknown/wasm-release/datascript_wasm.wasm")))
