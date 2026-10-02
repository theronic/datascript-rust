(ns datascript.test.wasm
  "What is particular to DataScript over the WebAssembly module: what crosses the boundary, and how."
  (:require
    [clojure.test :as t :refer [is are deftest testing]]
    [datascript.core :as d]
    [datascript.db :as db]
    [datascript.wasm :as wasm]))
