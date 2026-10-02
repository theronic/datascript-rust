(ns datascript.db
  "The one macro of datascript.db that code outside DataScript has referred to.")

(defmacro defrecord-updatable
  "A record whose protocol implementations replace the ones `defrecord` gives it."
  [name fields & impls]
  `(do
     (defrecord ~name ~fields)
     (extend-type ~name ~@impls)))
