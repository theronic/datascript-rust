(ns datascript.bench-setup
  "ClojureScript DataScript needs nothing started."
  (:require
    [datascript.core :as d]))

;; a database as JSON text, and back
(defn to-json [db] (js/JSON.stringify (d/serializable db)))
(defn from-json [text] (d/from-serializable (js/JSON.parse text)))
