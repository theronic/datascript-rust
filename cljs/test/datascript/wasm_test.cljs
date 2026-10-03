(ns datascript.wasm-test
  "DataScript's own tests of its public API, on the WebAssembly module. Left out are the tests of
  what is inside ClojureScript DataScript and is not here: its parsers, its cache, its storage and
  the experimental query engine."
  (:require
    [datascript.wasm-node]
    [clojure.test :as t]
    [clojure.walk]
    [goog.object :as gobj]
    [datascript.wasm :as wasm]
    datascript.test.core
    datascript.test.components
    datascript.test.conn
    datascript.test.db
    datascript.test.entity
    datascript.test.explode
    datascript.test.filter
    datascript.test.ident
    datascript.test.index
    datascript.test.listen
    datascript.test.lookup-refs
    datascript.test.pull-api
    datascript.test.query
    datascript.test.query-aggregates
    datascript.test.query-find-specs
    datascript.test.query-fns
    datascript.test.query-not
    datascript.test.query-or
    datascript.test.query-pull
    datascript.test.query-return-map
    datascript.test.query-rules
    datascript.test.serialize
    datascript.test.transact
    datascript.test.tuples
    datascript.test.validation
    datascript.test.upsert
    datascript.test.issues
    datascript.test.datafy
    datascript.test.wasm))

(defn -main [& _]
  ;; the tests that wait for the garbage collector end after this function does
  (add-watch datascript.test.core/test-summary ::done
    (fn [_ _ _ summary]
      (println "held:" (pr-str (wasm/held)))
      (when (pos? (+ (:fail summary) (:error summary)))
        (js-invoke js/process "exit" 1))))
  (t/run-all-tests #"datascript\.test\..*"))

(set! *main-cli-fn* -main)
