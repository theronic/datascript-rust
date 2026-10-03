(ns oracle.internals
  "What the oracle asks of the inside of ClojureScript DataScript, and of ClojureScript itself. The
  same harness runs over the WebAssembly module (conformance/wasm), where these ask the module."
  (:require
    [clojure.walk :as walk]
    [datascript.db :as db]
    [datascript.parser :as dp]
    [datascript.pull-parser :as dpp]))

(defn hash-of [x] (hash x))
(defn pr-str-of [x] (pr-str x))
(defn str-of [x] (str x))
(defn equal? [a b] (= a b))
(defn value-compare [a b] (db/value-compare a b))

(defn parse-query-line [query]
  (binding [*print-namespace-maps* false]
    (pr-str (dp/parse-query query))))

(defn parse-pull-line [db pattern]
  (binding [*print-namespace-maps* false]
    (pr-str (walk/postwalk #(if (fn? %) :fn %) (into {} (dpp/parse-pattern db pattern))))))
