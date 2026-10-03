(ns datascript.conn
  "DataScript's connections, as DataScript has them: datascript/conn.cljc, less what only the JVM's
  storage uses."
  (:require
    [datascript.db :as db :refer [DB FilteredDB]]
    [datascript.storage :as storage]
    [extend-clj.core :as extend]))

(extend/deftype-atom Conn [atom]
  (deref-impl [this]
    (:db @atom))
  (compare-and-set-impl [this oldv newv]
    (compare-and-set!
      atom
      (assoc @atom :db oldv)
      (assoc @atom :db newv))))

(defn- make-conn [opts]
  (->Conn (atom opts)))

(defn with
  ([db tx-data] (with db tx-data nil))
  ([db tx-data tx-meta]
   {:pre [(db/db? db)]}
   (if (instance? FilteredDB db)
     (throw (ex-info "Filtered DB cannot be modified" {:error :transaction/filtered}))
     (db/transact-tx-data (db/->TxReport db db [] {} tx-meta) tx-data))))

(defn ^DB db-with
  "Applies transaction to an immutable db value, returning new immutable db value. Same as `(:db-after (with db tx-data))`."
  [db tx-data]
  {:pre [(db/db? db)]}
  (:db-after (with db tx-data)))

(defn conn? [conn]
  (and
    #?(:clj  (instance? clojure.lang.IDeref conn)
       :cljs (satisfies? cljs.core/IDeref conn))
    (if-some [db @conn]
      (db/db? db)
      true)))

(defn conn-from-db [db]
  {:pre [(db/db? db)]}
  (if-some [storage (storage/storage db)]
    (do
      (storage/store db)
      (make-conn
        {:db db
         :tx-tail []
         :db-last-stored db}))
    (make-conn {:db db})))

(defn conn-from-datoms
  ([datoms]
   (conn-from-db (db/init-db datoms nil {})))
  ([datoms schema]
   (conn-from-db (db/init-db datoms schema {})))
  ([datoms schema opts]
   (conn-from-db (db/init-db datoms schema (storage/maybe-adapt-storage opts)))))

(defn create-conn
  ([]
   (conn-from-db (db/empty-db nil {})))
  ([schema]
   (conn-from-db (db/empty-db schema {})))
  ([schema opts]
   (conn-from-db (db/empty-db schema (storage/maybe-adapt-storage opts)))))

#?(:clj
   (defn restore-conn
     ([storage]
      (restore-conn storage {}))
     ([storage opts]
      (when-some [[db tail] (storage/restore-impl storage opts)]
        (make-conn
          {:db (storage/db-with-tail db tail)
           :tx-tail tail
           :db-last-stored db})))))

(defn ^:no-doc -transact! [conn tx-data tx-meta]
  {:pre [(conn? conn)]}
  (let [*report (volatile! nil)]
    (swap! conn
      (fn [db]
        ;; the connection moves on from db, which the module is told
        (let [r (binding [db/*moving-on* true]
                  (with db tx-data tx-meta))]
          (vreset! *report r)
          (:db-after r))))
    #?(:clj
       (when (storage/storage @conn)
         (let [{db :db-after
                datoms :tx-data} @*report]
           (when-not (empty? datoms)
             (let [settings (set/settings (:eavt db))
                   *atom    (:atom conn)
                   tx-tail' (:tx-tail (swap! *atom update :tx-tail conj datoms))]
               (if (> (transduce (map count) + 0 tx-tail') (:branching-factor settings))
                 ;; overflow tail
                 (do
                   (storage/store-impl! db (storage/storage-adapter db) false)
                   (swap! *atom assoc
                     :tx-tail        []
                     :db-last-stored db))
                 ;; just update tail
                 (storage/store-tail db tx-tail')))))))
    @*report))

(defn transact!
  ([conn tx-data]
   (transact! conn tx-data nil))
  ([conn tx-data tx-meta]
   {:pre [(conn? conn)]}
   (locking conn
     (let [report (-transact! conn tx-data tx-meta)]
       (doseq [[_ callback] (:listeners @(:atom conn))]
         (callback report))
       report))))

(defn ^:no-doc -moved-on!
  "The connection moved on by a transaction that transact! did not make: one a program in the module
  made, of the database the connection held (datascript-wasm, built into another program). The
  connection now holds the report's :db-after, and its listeners are told of the report, as
  transact! tells them. Throws when the connection holds another database than the report's
  :db-before, and is left as it was."
  [conn report]
  {:pre [(conn? conn)]}
  (when-not (compare-and-set! conn (:db-before report) (:db-after report))
    (throw (ex-info "The connection holds another database than the one the transaction was made of"
             {:error :transact/moved-on})))
  (doseq [[_ callback] (:listeners @(:atom conn))]
    (callback report))
  report)

(defn reset-conn!
  ([conn db]
   (reset-conn! conn db nil))
  ([conn db tx-meta]
   {:pre [(conn? conn)
          (db/db? db)]}
   (let [db-before @conn
         report    (db/map->TxReport
                     {:db-before db-before
                      :db-after  db
                      :tx-data   (concat
                                   (when db-before
                                     (map #(assoc % :added false) (db/-datoms db-before :eavt nil nil nil nil)))
                                   (db/-datoms db :eavt nil nil nil nil))
                      :tx-meta   tx-meta})]
     (if (storage/storage db-before)
       (do
         (storage/store db)
         (swap! (:atom conn) assoc
           :db             db
           :tx-tail        []
           :db-last-stored db))
       (reset! conn db))
     (doseq [[_ callback] (:listeners @(:atom conn))]
       (callback report))
     db)))

(defn reset-schema! [conn schema]
  {:pre [(conn? conn)]}
  (let [db (swap! conn db/with-schema schema)]
    #?(:clj
       (when-some [storage (storage/storage @conn)]
         (storage/store-impl! db (storage/storage-adapter db) true)
         (swap! (:atom conn) assoc
           :tx-tail []
           :db-last-stored db)))
    db))

(defn listen!
  ([conn callback]
   (listen! conn (rand) callback))
  ([conn key callback]
   {:pre [(conn? conn)]}
   (swap! (:atom conn) update :listeners assoc key callback)
   key))

(defn unlisten! [conn key]
  {:pre [(conn? conn)]}
  (swap! (:atom conn) update :listeners dissoc key))
