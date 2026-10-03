(ns oracle.internals
  "The oracle's harness over the WebAssembly module: this namespace in place of the oracle's own
  (conformance/oracle), and cljs/src in place of ClojureScript DataScript. What the oracle asks of
  ClojureScript itself, the hash of a value, how it prints, how two compare, is asked of the module
  here, of the value as it arrives there. The parsers are inside the module and are not asked."
  (:require
    [datascript.wasm :as wasm]
    [goog.object :as gobj]))

(wasm/instantiate-sync
  (js-invoke (js/require "fs") "readFileSync"
    (or (gobj/getValueByKeys js/process "env" "DATASCRIPT_WASM")
      "target/wasm32-unknown-unknown/wasm-release/datascript_wasm.wasm")))

(defn hash-of [x] (wasm/call wasm/op-hash #js [x]))
(defn pr-str-of [x] (wasm/call wasm/op-pr-str #js [x]))
(defn str-of [x] (wasm/call wasm/op-str #js [x]))
(defn equal? [a b] (wasm/call wasm/op-equiv #js [a b]))
(defn value-compare [a b] (wasm/call wasm/op-compare #js [a b]))

(defn parse-query-line [_] "#skipped")
(defn parse-pull-line [_ _] "#skipped")
