(ns datascript.wasm-node
  "Loads DataScript's WebAssembly module as this namespace loads, in Node: for programs that make
  databases while their namespaces load, as DataScript's tests do. The module's file is named by
  DATASCRIPT_WASM."
  (:require
    [datascript.wasm :as wasm]
    [goog.object :as gobj]))

(def path
  (or (gobj/getValueByKeys js/process "env" "DATASCRIPT_WASM")
    "target/wasm32-unknown-unknown/wasm-release/datascript_wasm.wasm"))

(wasm/instantiate-sync (js-invoke (js/require "fs") "readFileSync" path))
