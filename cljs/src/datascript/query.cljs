(ns ^:no-doc datascript.query
  "Datalog queries: the WebAssembly module's engine."
  (:require
    [datascript.db]
    [datascript.wasm :as wasm]))

(defn q [q & inputs]
  (wasm/call wasm/op-q #js [q (to-array inputs)]))
