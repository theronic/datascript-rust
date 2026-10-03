(ns ^:no-doc datascript.pull-api
  "Pull: the WebAssembly module's."
  (:require
    [datascript.db :as db]
    [datascript.wasm :as wasm]))

(defn pull
  "Supported opts:

   :visitor a fn of 4 arguments, will be called for every entity/attribute pull touches

   (:db.pull/attr     e   a   nil) - when pulling a normal attribute, no matter if it has value or not
   (:db.pull/wildcard e   nil nil) - when pulling every attribute on an entity
   (:db.pull/reverse  nil a   v  ) - when pulling reverse attribute"
  ([db pattern id] (pull db pattern id {}))
  ([db pattern id opts]
   {:pre [(db/db? db)]}
   (wasm/call wasm/op-pull #js [db pattern id (:visitor opts)])))

(defn pull-many
  ([db pattern ids] (pull-many db pattern ids {}))
  ([db pattern ids opts]
   {:pre [(db/db? db)]}
   (wasm/call wasm/op-pull-many #js [db pattern ids (:visitor opts)])))
