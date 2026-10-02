(ns datascript.serialize
  "A database as JavaScript's plain data, and back: the WebAssembly module writes and reads it as
  JSON text."
  (:require
    [datascript.db :as db]
    [datascript.wasm :as wasm]))

(defn serializable
  "A structure of arrays, numbers and strings that JSON, or any format like it, carries.

   :freeze-fn  what stands for a value that is no string, number, boolean or keyword; pr-str by default
   :freeze-kw  what stands for an attribute or keyword; str by default"
  ([db] (serializable db {}))
  ([db {:keys [freeze-fn freeze-kw]}]
   (js/JSON.parse (wasm/call wasm/op-serializable #js [db freeze-fn freeze-kw]))))

(defn- json-text
  "What from-serializable is given, as JSON text: JavaScript's data as it is, ClojureScript's as the
  JavaScript data it stands for."
  [from]
  (js/JSON.stringify
    (if (or (map? from) (vector? from)) (clj->js from) from)))

(defn from-serializable
  "The database a structure made by serializable holds.

   :thaw-fn  what reads back a value :freeze-fn wrote; reading EDN by default
   :thaw-kw  what reads back an attribute or keyword; a keyword of a string that starts with a colon by default"
  ([from] (from-serializable from {}))
  ([from {:keys [thaw-fn thaw-kw]}]
   (wasm/call wasm/op-from-serializable #js [(json-text from) thaw-fn thaw-kw])))
