(ns ^:no-doc datascript.db
  "DataScript's database, as ClojureScript sees it: datoms, database values and what is asked of
  them. The database itself is in the WebAssembly module (datascript.wasm); a database value here is
  the handle of one there.

  The names are datascript.db's, so that what was written against it, DataScript's own
  datascript.core, datascript.conn and datascript.impl.entity among it, runs on this."
  (:require-macros
    [datascript.db :refer [defrecord-updatable]])
  (:require
    [clojure.data]
    [datascript.wasm :as wasm])
  (:refer-clojure :exclude [seqable?]))

(def Exception js/Error)
(def IllegalArgumentException js/Error)
(def UnsupportedOperationException js/Error)

(def ^:const e0 0)
(def ^:const tx0 0x20000000)
(def ^:const emax 0x7FFFFFFF)
(def ^:const txmax 0x7FFFFFFF)

(def ^:const implicit-schema
  {:db/ident {:db/unique :db.unique/identity}})

(defn ^boolean seqable? [x]
  (and (not (string? x))
    (or (cljs.core/seqable? x)
      (array? x))))

(defn combine-hashes [x y]
  (hash-combine x y))

(defn- raise [msg data]
  (throw (ex-info msg data)))

(defn value-compare
  "The order of values in DataScript's indexes: negative, zero or positive."
  [x y]
  (wasm/call wasm/op-compare #js [x y]))

;; ---------------------------------------------------------------- datoms

(declare hash-datom equiv-datom seq-datom nth-datom assoc-datom val-at-datom)

(defprotocol IDatom
  (datom-tx [this])
  (datom-added [this])
  (datom-get-idx [this])
  (datom-set-idx [this value]))

(deftype Datom [^number e a v ^number tx ^:mutable ^number idx ^:mutable ^number _hash]
  IDatom
  (datom-tx [d] (if (pos? tx) tx (- tx)))
  (datom-added [d] (pos? tx))
  (datom-get-idx [_] idx)
  (datom-set-idx [_ value] (set! idx (int value)))

  IHash
  (-hash [d] (if (zero? _hash)
               (set! _hash (hash-datom d))
               _hash))
  IEquiv
  (-equiv [d o] (and (instance? Datom o) (equiv-datom d o)))

  ISeqable
  (-seq [d] (seq-datom d))

  ILookup
  (-lookup [d k] (val-at-datom d k nil))
  (-lookup [d k nf] (val-at-datom d k nf))

  IIndexed
  (-nth [this i] (nth-datom this i))
  (-nth [this i not-found] (nth-datom this i not-found))

  IAssociative
  (-assoc [d k v] (assoc-datom d k v))

  IPrintWithWriter
  (-pr-writer [d writer opts]
    (pr-sequential-writer writer pr-writer
      "#datascript/Datom [" " " "]"
      opts [(.-e d) (.-a d) (.-v d) (datom-tx d) (datom-added d)])))

(goog/exportSymbol "datascript.db.Datom" Datom)

(defn ^Datom datom
  ([e a v] (Datom. e a v tx0 0 0))
  ([e a v tx] (Datom. e a v tx 0 0))
  ([e a v tx added] (Datom. e a v (if added tx (- tx)) 0 0)))

(defn datom? [x] (instance? Datom x))

(defn- hash-datom [^Datom d]
  (-> (hash (.-e d))
    (combine-hashes (hash (.-a d)))
    (combine-hashes (hash (.-v d)))))

(defn- equiv-datom [^Datom d ^Datom o]
  (and (== (.-e d) (.-e o))
    (= (.-a d) (.-a o))
    (= (.-v d) (.-v o))))

(defn- seq-datom [^Datom d]
  (list (.-e d) (.-a d) (.-v d) (datom-tx d) (datom-added d)))

(defn- val-at-datom [^Datom d k not-found]
  (cond
    (keyword? k)
    (case k
      :e     (.-e d)
      :a     (.-a d)
      :v     (.-v d)
      :tx    (datom-tx d)
      :added (datom-added d)
      not-found)

    (string? k)
    (case k
      "e"     (.-e d)
      "a"     (.-a d)
      "v"     (.-v d)
      "tx"    (datom-tx d)
      "added" (datom-added d)
      not-found)

    :else
    not-found))

(defn- nth-datom
  ([^Datom d i]
   (case i
     0 (.-e d)
     1 (.-a d)
     2 (.-v d)
     3 (datom-tx d)
     4 (datom-added d)
     (throw (js/Error. (str "Datom/-nth: Index out of bounds: " i)))))
  ([^Datom d i not-found]
   (case i
     0 (.-e d)
     1 (.-a d)
     2 (.-v d)
     3 (datom-tx d)
     4 (datom-added d)
     not-found)))

(defn- ^Datom assoc-datom [^Datom d k v]
  (case k
    :e     (datom v       (.-a d) (.-v d) (datom-tx d) (datom-added d))
    :a     (datom (.-e d) v       (.-v d) (datom-tx d) (datom-added d))
    :v     (datom (.-e d) (.-a d) v       (datom-tx d) (datom-added d))
    :tx    (datom (.-e d) (.-a d) (.-v d) v            (datom-added d))
    :added (datom (.-e d) (.-a d) (.-v d) (datom-tx d) v)
    (throw (IllegalArgumentException. (str "invalid key for #datascript/Datom: " k)))))

(defn ^Datom datom-from-reader [vec]
  (apply datom vec))

;; ---------------------------------------------------------------- runs of datoms

(declare ->DatomSeq)

;; What is left of a run of datoms in the module: a cursor there, read once, and what it read.
(deftype More [^:mutable cursor ^:mutable fetched ^:mutable following])

(def ^:private cursor-registry
  (when (exists? js/FinalizationRegistry)
    (js/FinalizationRegistry.
      (fn [cursor]
        (when (wasm/ready?)
          (wasm/call wasm/op-cursor-free #js [cursor]))))))

(def ^:const ^:private chunk-size 512)

(defn- more-of [cursor]
  (when (some? cursor)
    (let [more (More. cursor false nil)]
      (when (some? cursor-registry)
        (.register cursor-registry more cursor more))
      more)))

(defn- following
  "The run after the part already read: read from the module the first time it is asked for."
  [^More more]
  (when-not (.-fetched more)
    (let [[datoms cursor] (wasm/call wasm/op-cursor-next #js [(.-cursor more) chunk-size])]
      (when (some? cursor-registry)
        (.unregister cursor-registry more))
      (set! (.-fetched more) true)
      (set! (.-cursor more) nil)
      (set! (.-following more)
        (when (pos? (count datoms))
          (->DatomSeq datoms 0 (more-of cursor) nil nil)))))
  (.-following more))

;; A run of datoms, as a sequence: the part read so far, and where the rest is.
;; `reversed`, at the run's head, is a function that reads the same run from its other end.
(deftype DatomSeq [chunk i ^More more reversed meta]
  Object
  (toString [this] (pr-str* this))
  (equiv [this other] (-equiv this other))

  ISeqable
  (-seq [this] this)

  ISeq
  (-first [_] (-nth chunk i))
  (-rest [this] (or (-next this) ()))

  INext
  (-next [_]
    (if (< (inc i) (count chunk))
      (DatomSeq. chunk (inc i) more nil nil)
      (when (some? more)
        (following more))))

  IChunkedSeq
  (-chunked-first [_] (array-chunk (to-array (subvec chunk i))))
  (-chunked-rest [_] (or (when (some? more) (following more)) ()))

  IChunkedNext
  (-chunked-next [_] (when (some? more) (following more)))

  ICollection
  (-conj [this o] (cons o this))

  IEmptyableCollection
  (-empty [_] ())

  ISequential

  IEquiv
  (-equiv [this other] (equiv-sequential this other))

  IHash
  (-hash [this] (hash-ordered-coll this))

  IReduce
  (-reduce [this f]
    (seq-reduce f this))
  (-reduce [this f init]
    (loop [acc   init
           chunk chunk
           i     i
           more  more]
      (cond
        (reduced? acc) @acc
        (< i (count chunk)) (recur (f acc (-nth chunk i)) chunk (inc i) more)
        :else
        (if-some [^DatomSeq next (when (some? more) (following more))]
          (recur acc (.-chunk next) (.-i next) (.-more next))
          acc))))

  IReversible
  (-rseq [this]
    (if (some? reversed)
      (reversed)
      (rseq (vec this))))

  IMeta
  (-meta [_] meta)

  IWithMeta
  (-with-meta [_ m] (DatomSeq. chunk i more reversed m))

  IPrintWithWriter
  (-pr-writer [this writer opts]
    (pr-sequential-writer writer pr-writer "(" " " ")" opts this)))

(es6-iterable DatomSeq)

(defn- datom-seq
  "What an operation that reads a run of datoms answers, [datoms cursor], as a sequence."
  [answer reversed]
  (let [datoms (nth answer 0)]
    (when (pos? (count datoms))
      (DatomSeq. datoms 0 (more-of (nth answer 1)) reversed nil))))

;; ---------------------------------------------------------------- database values

(defprotocol ISearch
  (-search [data pattern]))

(defprotocol IIndexAccess
  (-datoms [db index c0 c1 c2 c3])
  (-seek-datoms [db index c0 c1 c2 c3])
  (-rseek-datoms [db index c0 c1 c2 c3])
  (-index-range [db attr start end]))

(defprotocol IDB
  (-schema [db])
  (-attrs-by [db property]))

(declare DB FilteredDB hash-db hash-fdb equiv-db pr-db empty-db db? diff-dbs)

(def ^:private db-keys
  [:schema :eavt :aevt :avet :max-eid :max-tx :rschema :pull-patterns :pull-attrs :hash])

;; The schemas read from the module, by their numbers there: #js [schema rschema]
(def ^:private schemas (js/Map.))

(defn- schema-entry [db uid]
  (or (.get schemas uid)
    (let [[schema rschema] (wasm/call wasm/op-schema #js [db])
          entry #js [schema rschema]]
      (.set schemas uid entry)
      entry)))

(def ^:const ^:private first-chunk 64)

(defn- index-read [op db index c0 c1 c2 c3]
  (datom-seq
    (wasm/call op #js [db index c0 c1 c2 c3 first-chunk false])
    #(datom-seq (wasm/call op #js [db index c0 c1 c2 c3 chunk-size true]) nil)))

(defn- range-read [db attr start end]
  (datom-seq
    (wasm/call wasm/op-index-range #js [db attr start end first-chunk false])
    #(datom-seq (wasm/call wasm/op-index-range #js [db attr start end chunk-size true]) nil)))

(defn- search-read [db pattern]
  (let [[e a v tx] pattern]
    (datom-seq
      (wasm/call wasm/op-search #js [db e a v tx chunk-size false])
      #(datom-seq (wasm/call wasm/op-search #js [db e a v tx chunk-size true]) nil))))

;; A database value: the handle of one in the module. `owner` is what holds the handle: when nothing
;; holds the owner any more, the module is told to let the database go.
(deftype DB [handle owner schema-uid max-eid max-tx hash meta]
  Object
  (toString [this] (pr-str* this))
  (equiv [this other] (-equiv this other))

  IHash
  (-hash [db] (hash-db db))

  IEquiv
  (-equiv [db other] (equiv-db db other))

  ;; as the record DataScript's database is: a map of its fields
  IRecord
  IMap
  (-dissoc [_ k] (throw (js/Error. (str "datascript: a database's " k " is not taken away"))))

  ISeqable
  (-seq [db]
    (map (fn [k] (MapEntry. k (-lookup db k nil) nil)) db-keys))

  IReversible
  (-rseq [db] (some-> (-datoms db :eavt nil nil nil nil) rseq))

  ICounted
  (-count [db] (wasm/call wasm/op-db-count #js [db]))

  IEmptyableCollection
  (-empty [db] (with-meta (wasm/call wasm/op-db-empty #js [db]) meta))

  IPrintWithWriter
  (-pr-writer [db w opts] (pr-db db w opts))

  IMeta
  (-meta [_] meta)

  IWithMeta
  (-with-meta [_ m] (DB. handle owner schema-uid max-eid max-tx hash m))

  ILookup
  (-lookup [db k] (-lookup db k nil))
  (-lookup [db k not-found]
    (case k
      :schema  (-schema db)
      :rschema (aget (schema-entry db schema-uid) 1)
      :max-eid max-eid
      :max-tx  max-tx
      :eavt    (-datoms db :eavt nil nil nil nil)
      :aevt    (-datoms db :aevt nil nil nil nil)
      :avet    (-datoms db :avet nil nil nil nil)
      :hash    hash
      (:pull-patterns :pull-attrs) nil
      not-found))

  IAssociative
  (-contains-key? [_ k]
    (some? (some #(= k %) db-keys)))
  (-assoc [_ k _]
    (throw (js/Error. (str "datascript: a database's " k " is not set with assoc"))))

  IDB
  (-schema [db] (aget (schema-entry db schema-uid) 0))
  (-attrs-by [db property] (get (aget (schema-entry db schema-uid) 1) property))

  ISearch
  (-search [db pattern] (search-read db pattern))

  IIndexAccess
  (-datoms [db index c0 c1 c2 c3] (index-read wasm/op-datoms db index c0 c1 c2 c3))
  (-seek-datoms [db index c0 c1 c2 c3] (index-read wasm/op-seek-datoms db index c0 c1 c2 c3))
  (-rseek-datoms [db index c0 c1 c2 c3] (index-read wasm/op-rseek-datoms db index c0 c1 c2 c3))
  (-index-range [db attr start end] (range-read db attr start end))

  clojure.data/EqualityPartition
  (equality-partition [_] :datascript/db)

  clojure.data/Diff
  (diff-similar [a b] (diff-dbs a b)))

;; A view of a database through a predicate.
(deftype FilteredDB [handle owner schema-uid max-eid max-tx hash meta]
  Object
  (toString [this] (pr-str* this))
  (equiv [this other] (-equiv this other))

  IHash
  (-hash [db] (hash-fdb db))

  IEquiv
  (-equiv [db other] (equiv-db db other))

  IRecord
  IMap
  (-dissoc [_ k] (throw (js/Error. (str "datascript: a database's " k " is not taken away"))))

  ISeqable
  (-seq [db]
    (list (MapEntry. :unfiltered-db (wasm/call wasm/op-unfiltered #js [db]) nil)
      (MapEntry. :pred nil nil)
      (MapEntry. :hash hash nil)))

  ICounted
  (-count [db] (wasm/call wasm/op-db-count #js [db]))

  IPrintWithWriter
  (-pr-writer [db w opts] (pr-db db w opts))

  IEmptyableCollection
  (-empty [_] (throw (js/Error. "-empty is not supported on FilteredDB")))

  IMeta
  (-meta [_] meta)

  IWithMeta
  (-with-meta [_ m] (FilteredDB. handle owner schema-uid max-eid max-tx hash m))

  ILookup
  (-lookup [_ _] (throw (js/Error. "-lookup is not supported on FilteredDB")))
  (-lookup [_ _ _] (throw (js/Error. "-lookup is not supported on FilteredDB")))

  IAssociative
  (-contains-key? [_ _] (throw (js/Error. "-contains-key? is not supported on FilteredDB")))
  (-assoc [_ _ _] (throw (js/Error. "-assoc is not supported on FilteredDB")))

  IDB
  (-schema [db] (aget (schema-entry db schema-uid) 0))
  (-attrs-by [db property] (get (aget (schema-entry db schema-uid) 1) property))

  ISearch
  (-search [db pattern] (or (search-read db pattern) ()))

  IIndexAccess
  (-datoms [db index c0 c1 c2 c3] (or (index-read wasm/op-datoms db index c0 c1 c2 c3) ()))
  (-seek-datoms [db index c0 c1 c2 c3] (or (index-read wasm/op-seek-datoms db index c0 c1 c2 c3) ()))
  (-rseek-datoms [db index c0 c1 c2 c3] (or (index-read wasm/op-rseek-datoms db index c0 c1 c2 c3) ()))
  (-index-range [db attr start end] (or (range-read db attr start end) ())))

;; ---------------------------------------------------------------- handles

;; The database values that are held, by handle: #js [WeakRef-of-owner WeakRef-of-value token].
;; The owner outlives the value when a copy of the value with other metadata is held.
(def ^:private held (js/Map.))

(def ^:private weak? (and (exists? js/WeakRef) (exists? js/FinalizationRegistry)))

(def ^:private db-registry
  (when weak?
    (js/FinalizationRegistry.
      (fn [^array told]
        ;; [handle token]: the owner of that token is gone. If the handle has not been taken up again since,
        ;; the module lets the database go.
        (let [handle (aget told 0)
              entry  (.get held handle)]
          (when (and (some? entry) (identical? (aget entry 2) (aget told 1)))
            (.delete held handle)
            (when (wasm/ready?)
              (wasm/call wasm/op-release-db #js [handle]))))))))

(defn- db-of
  "The database value of a handle the module names: the one already held, when one is."
  [handle uid max-eid max-tx filtered?]
  (let [entry (.get held handle)
        owner (when (some? entry)
                (if weak? (.deref (aget entry 0)) (aget entry 0)))
        value (when (and (some? entry) (some? owner))
                (if weak? (.deref (aget entry 1)) (aget entry 1)))]
    (or value
      (let [owner (or owner #js {})
            value (if filtered?
                    (FilteredDB. handle owner uid max-eid max-tx (atom 0) nil)
                    (DB. handle owner uid max-eid max-tx (atom 0) nil))]
        (if weak?
          (if (and (some? entry) (some? (.deref (aget entry 0))))
            (aset entry 1 (js/WeakRef. value))
            (let [token #js {}]
              (.set held handle #js [(js/WeakRef. owner) (js/WeakRef. value) token])
              (.register db-registry owner #js [handle token])))
          (.set held handle #js [owner value nil]))
        value))))

(wasm/register-type! Datom "datascript.db/Datom")
(set! wasm/datom-type Datom)
(set! wasm/datom-ctor datom)
(set! wasm/db-ctor db-of)
(wasm/register-type! DB "datascript.db/DB")
(wasm/register-type! FilteredDB "datascript.db/FilteredDB")
(set! wasm/db-handle
  (fn [x]
    (cond
      (instance? DB x) (.-handle x)
      (instance? FilteredDB x) (.-handle x)
      :else nil)))

;; ---------------------------------------------------------------- what is asked of a database

(defn db? [x]
  (and (satisfies? ISearch x)
    (satisfies? IIndexAccess x)
    (satisfies? IDB x)))

(defn- wasm-db? [x]
  (or (instance? DB x) (instance? FilteredDB x)))

(defn unfiltered-db ^DB [db]
  (if (instance? FilteredDB db)
    (wasm/call wasm/op-unfiltered #js [db])
    db))

(defn- hash-of
  "A database's hash, kept in the atom it holds for it once it is known."
  [db]
  (let [cache (.-hash db)
        h     @cache]
    (if (zero? h)
      (reset! cache (wasm/call wasm/op-db-hash #js [db]))
      h)))

(defn hash-db [^DB db]
  (hash-of db))

(defn hash-fdb [^FilteredDB db]
  (hash-of db))

(defn equiv-db [db other]
  (and (wasm-db? other)
    (or (identical? db other)
      (wasm/call wasm/op-db-equiv #js [db other]))))

(defn pr-db [db w opts]
  (-write w "#datascript/DB {")
  (-write w ":schema ")
  (pr-writer (-schema db) w opts)
  (-write w ", :datoms ")
  (pr-sequential-writer w
    (fn [d w opts]
      (pr-sequential-writer w pr-writer "[" " " "]" opts [(.-e d) (.-a d) (.-v d) (datom-tx d)]))
    "[" " " "]" opts (-datoms db :eavt nil nil nil nil))
  (-write w "}"))

(defn- diff-dbs [a b]
  (wasm/call wasm/op-diff #js [a b]))

(defn ^DB empty-db [schema opts]
  (wasm/call wasm/op-empty-db #js [schema]))

(defn ^DB init-db [datoms schema opts]
  (wasm/call wasm/op-init-db #js [datoms schema]))

(defn ^DB with-schema [db schema]
  (wasm/call wasm/op-with-schema #js [db schema]))

(defn ^DB db-from-reader [{:keys [schema datoms]}]
  (init-db (map (fn [[e a v tx]] (datom e a v tx)) datoms) schema {}))

(defn is-attr? ^boolean [db attr property]
  (contains? (-attrs-by db property) attr))

(defn multival? ^boolean [db attr]
  (is-attr? db attr :db.cardinality/many))

(defn multi-value? ^boolean [db attr value]
  (and
    (is-attr? db attr :db.cardinality/many)
    (or
      (array? value)
      (and (coll? value) (not (map? value))))))

(defn ref? ^boolean [db attr]
  (is-attr? db attr :db.type/ref))

(defn component? ^boolean [db attr]
  (is-attr? db attr :db/isComponent))

(defn indexing? ^boolean [db attr]
  (is-attr? db attr :db/index))

(defn tuple? ^boolean [db attr]
  (is-attr? db attr :db.type/tuple))

(defn tuple-source? ^boolean [db attr]
  (is-attr? db attr :db/attrTuples))

(defn reverse-ref? ^boolean [attr]
  (cond
    (keyword? attr)
    (= \_ (nth (name attr) 0))

    (string? attr)
    (boolean (re-matches #"(?:([^/]+)/)?_([^/]+)" attr))

    :else
    (raise (str "Bad attribute type: " (pr-str attr) ", expected keyword or string")
      {:error :transact/syntax, :attribute attr})))

(defn reverse-ref [attr]
  (cond
    (keyword? attr)
    (if (reverse-ref? attr)
      (keyword (namespace attr) (subs (name attr) 1))
      (keyword (namespace attr) (str "_" (name attr))))

    (string? attr)
    (let [[_ ns name] (re-matches #"(?:([^/]+)/)?([^/]+)" attr)]
      (if (= \_ (nth name 0))
        (if ns (str ns "/" (subs name 1)) (subs name 1))
        (if ns (str ns "/_" name) (str "_" name))))

    :else
    (raise (str "Bad attribute type: " (pr-str attr) ", expected keyword or string")
      {:error :transact/syntax, :attribute attr})))

(defn entid [db eid]
  {:pre [(db? db)]}
  (if (and (number? eid) (pos? eid) (<= eid emax))
    eid
    (wasm/call wasm/op-entid #js [db eid])))

(defn entid-strict [db eid]
  (or (entid db eid)
    (raise (str "Nothing found for entity id " (pr-str eid))
      {:error :entity-id/missing
       :entity-id eid})))

(defn entid-some [db eid]
  (when eid
    (entid-strict db eid)))

(defn numeric-eid-exists? ^boolean [db eid]
  ;; the first datom from the entity on, and no more: the cursor to the rest is given back
  (let [[datoms cursor] (wasm/call wasm/op-seek-datoms #js [db :eavt eid nil nil nil 1 false])]
    (when (some? cursor)
      (wasm/call wasm/op-cursor-free #js [cursor]))
    (= eid (some-> (first datoms) :e))))

(defn find-datom [db index c0 c1 c2 c3]
  (wasm/call wasm/op-find-datom #js [db index c0 c1 c2 c3]))

;; ---------------------------------------------------------------- transactions

(defrecord TxReport [db-before db-after tx-data tempids tx-meta])

(defn transact-tx-data
  "The transaction applied to the report's database: the report of it."
  [initial-report initial-es]
  (let [db      (:db-before initial-report)
        tx-meta (:tx-meta initial-report)
        ;; the transaction's metadata stays here: the report carries it, the module has no use for it
        [db-after tx-data tempids] (wasm/call wasm/op-with #js [db initial-es nil])]
    (TxReport. db db-after tx-data tempids tx-meta)))
