(ns datascript.bench-setup
  "The WebAssembly module, loaded before the benchmark asks anything of it."
  (:require
    [datascript.serialize :as serialize]
    [datascript.wasm :as wasm]
    [goog.object :as gobj]))

(wasm/instantiate-sync
  (js-invoke (js/require "fs") "readFileSync"
    (or (gobj/getValueByKeys js/process "env" "DATASCRIPT_WASM")
      "target/wasm32-unknown-unknown/wasm-release/datascript_wasm.wasm")))

;; a database as JSON text, and back: the module writes and reads the text itself
(def to-json serialize/json)
(def from-json serialize/from-json)
