(ns oracle.core
  "The reference side of the conformance harness: ClojureScript DataScript itself (this repository's
  src/), run in Node over a file of cases. A case is one line of EDN, a vector of steps; each step
  is a map with an :op. The oracle prints one line per step: what ClojureScript DataScript answers,
  printed with pr-str, so that the order of every set, map and sequence shows. The Rust port runs
  the same file (crates/conformance) and must print the same lines.

  A step may name its result (:as), and later steps of the case refer to it as #r name. #f name is
  one of the functions below, for predicates, query functions, aggregates and transaction
  functions, which both sides implement alike."
  (:require
    [cljs.reader :as reader]
    [clojure.data]
    [clojure.string :as str]
    [clojure.walk :as walk]
    [datascript.core :as d]
    [datascript.db :as db]
    [datascript.impl.entity :as de]
    [datascript.parser :as dp]
    [datascript.pull-parser :as dpp]))

(def fs (js/require "fs"))

(defrecord Ref [name])
(defrecord FnRef [name])

(declare fns)

(defn- resolve-args
  "x with every #r replaced by the value the case bound to it, and every #f by its function."
  [env x]
  (walk/prewalk
    (fn [v]
      (cond
        (instance? Ref v)
        (let [k (:name v)]
          (when-not (contains? env k)
            (throw (js/Error. (str "oracle: unbound ref " k))))
          (get env k))

        (instance? FnRef v)
        (or (get fns (:name v))
          (throw (js/Error. (str "oracle: unknown fn " (:name v)))))

        :else v))
    x))

;; ---------------------------------------------------------------- functions both sides know

(def fns
  {'even?        even?
   'odd?         odd?
   'pos?         pos?
   'adult?       (fn [age] (>= age 18))
   'str-len      (fn [s] (count s))
   'starts-with? (fn [s prefix] (str/starts-with? s prefix))
   'plus         (fn [& xs] (reduce + 0 xs))
   'pair         (fn [a b] [a b])
   'triples      (fn [x] [[x 1] [x 2] [x 3]])
   'nil-fn       (fn [& _] nil)
   'always       (fn [& _] true)
   'never        (fn [& _] false)
   'throw        (fn [& _] (throw (ex-info "thrown by host fn" {:error :host/thrown})))
   ;; aggregates
   'agg-count    (fn [coll] (count coll))
   'agg-first    (fn [coll] (first coll))
   'agg-sorted   (fn [coll] (vec (sort coll)))
   'agg-nth      (fn [n coll] (nth (vec coll) n nil))
   ;; transaction functions: (f db & args) -> tx-data
   'tx-inc       (fn [db e a by]
                   (let [v (:v (first (d/datoms db :eavt e a)))]
                     [[:db/add e a (+ (or v 0) by)]]))
   'tx-add       (fn [db e a v] [[:db/add e a v]])
   'tx-nothing   (fn [db & _] [])
   'tx-nil       (fn [db & _] nil)
   'tx-entity    (fn [db m] [m])
   'tx-count     (fn [db e a] [[:db/add e a (count (d/datoms db :eavt))]])
   'tx-throw     (fn [db & _] (throw (ex-info "thrown by tx fn" {:error :host/thrown})))
   ;; filter predicates: (pred db datom)
   'f-even-e     (fn [_ datom] (even? (:e datom)))
   'f-not-name   (fn [_ datom] (not= :name (:a datom)))
   'f-num-v      (fn [_ datom] (number? (:v datom)))
   'f-tx-odd     (fn [_ datom] (odd? (:tx datom)))
   'f-has-name   (fn [db datom] (some? (:name (d/entity db (:e datom)))))
   'f-none       (fn [_ _] false)
   'f-all        (fn [_ _] true)
   ;; pull xforms
   'x-vector     vector
   'x-str        str
   'x-count      (fn [x] (if (coll? x) (count x) x))})

;; ---------------------------------------------------------------- printing

(defn- p [x]
  (binding [*print-namespace-maps* false]
    (pr-str x)))

(defn- db-line
  "A database as the harness prints it: its indexes' sizes, its counters and itself."
  [db]
  (str "#db " (p {:max-eid (:max-eid (db/unfiltered-db db))
                  :max-tx  (:max-tx (db/unfiltered-db db))
                  :count   (count db)
                  :aevt    (when-not (d/is-filtered db) (mapv (juxt :a :e :v :tx) (:aevt db)))
                  :avet    (when-not (d/is-filtered db) (mapv (juxt :a :v :e :tx) (:avet db)))})
    " " (p db)))

(defn- report-line [report]
  (str "#report " (p {:tx-data (vec (:tx-data report))
                      :tempids (:tempids report)
                      :tx-meta (:tx-meta report)})
    " " (db-line (:db-after report))))

(defn- error-line [e]
  (if-some [data (ex-data e)]
    (str "#error " (p {:msg (ex-message e) :error (:error data)}))
    (str "#error :native " (p (str (or (ex-message e) e))))))

(defn- error-line-quiet
  "An error inside a longer line: JavaScript's own without its wording, which is the engine's."
  [e]
  (if (ex-data e) (error-line e) "#error :native"))

;; ---------------------------------------------------------------- steps

(defn- entity-view
  "An entity as the harness prints it: the attributes asked for, in order, then the entity itself,
  whose printing shows what it cached."
  [e attrs touch?]
  (when e
    (let [vals (mapv #(get e %) attrs)
          e    (if touch? (d/touch e) e)]
      {:vals vals
       :str  (p e)
       :seq  (when touch? (vec (seq e)))
       :count (when touch? (count e))})))

(defn- run-step
  "One step: [what to print, the value to bind]."
  [env {:keys [op] :as step}]
  (let [arg (fn [k] (resolve-args env (get step k)))]
    (case op
      :empty-db
      (let [db (if (contains? step :schema) (d/empty-db (arg :schema)) (d/empty-db))]
        [(db-line db) db])

      :init-db
      (let [datoms (map (fn [d] (apply d/datom d)) (arg :datoms))
            db     (if (contains? step :schema) (d/init-db datoms (arg :schema)) (d/init-db datoms))]
        [(db-line db) db])

      :db-with
      (let [db (d/db-with (arg :db) (arg :tx))]
        [(db-line db) db])

      :with
      (let [report (if (contains? step :tx-meta)
                     (d/with (arg :db) (arg :tx) (arg :tx-meta))
                     (d/with (arg :db) (arg :tx)))]
        [(report-line report) (:db-after report)])

      :with-schema
      (let [db (d/with-schema (arg :db) (arg :schema))]
        [(db-line db) db])

      :filter
      (let [db (d/filter (arg :db) (arg :pred))]
        [(db-line db) db])

      :q
      (let [r (apply d/q (arg :query) (arg :inputs))]
        [(p r) r])

      :pull
      (let [r (d/pull (arg :db) (arg :pattern) (arg :eid))]
        [(p r) r])

      :pull-many
      (let [r (d/pull-many (arg :db) (arg :pattern) (arg :eids))]
        [(p r) r])

      :pull-visit
      (let [seen (atom [])
            r    (d/pull (arg :db) (arg :pattern) (arg :eid)
                   {:visitor (fn [k e a v] (swap! seen conj [k e a v]))})]
        [(p {:result r :visited @seen}) r])

      (:datoms :seek-datoms :rseek-datoms)
      (let [f (case op :datoms d/datoms :seek-datoms d/seek-datoms :rseek-datoms d/rseek-datoms)
            r (apply f (arg :db) (arg :index) (arg :components))
            r (if-some [n (:limit step)] (take n r) r)]
        [(p (vec r)) nil])

      :find-datom
      (let [r (apply d/find-datom (arg :db) (arg :index) (arg :components))]
        [(p r) r])

      :index-range
      (let [r (d/index-range (arg :db) (arg :attr) (arg :start) (arg :end))]
        [(p (vec r)) nil])

      :entid
      (let [r (d/entid (arg :db) (arg :eid))]
        [(p r) r])

      :entity
      (let [e (d/entity (arg :db) (arg :eid))]
        [(p (entity-view e (arg :attrs) (:touch step))) nil])

      :schema
      [(p (d/schema (arg :db))) nil]

      :rschema
      [(p (:rschema (arg :db))) nil]

      :db-eq
      [(p (= (arg :a) (arg :b))) nil]

      :db-hash-eq
      [(p (= (hash (arg :a)) (hash (arg :b)))) nil]

      :db-empty
      (let [db (empty (arg :db))]
        [(db-line db) db])

      :diff
      [(p (clojure.data/diff (arg :a) (arg :b))) nil]

      :serializable
      (let [s  (d/serializable (arg :db))
            db (d/from-serializable (js/JSON.parse (js/JSON.stringify s)))]
        [(str (js/JSON.stringify s) " " (db-line db)) db])

      :read-db
      (let [db (reader/read-string (arg :string))]
        [(db-line db) db])

      :conn
      ;; a connection's life: transactions in order, each listened to, then its database
      (let [conn    (if (contains? step :db)
                      (d/conn-from-db (arg :db))
                      (d/create-conn (arg :schema)))
            reports (atom [])
            _       (d/listen! conn :oracle #(swap! reports conj %))
            lines   (mapv (fn [tx]
                            (try
                              (cond
                                (and (map? tx) (contains? tx :reset-conn))
                                (do (d/reset-conn! conn (resolve-args env (:reset-conn tx)) (:tx-meta tx)) "reset")

                                (and (map? tx) (contains? tx :reset-schema))
                                (do (d/reset-schema! conn (resolve-args env (:reset-schema tx))) "schema")

                                (and (map? tx) (contains? tx :tx-data))
                                (report-line (d/transact! conn (resolve-args env (:tx-data tx)) (:tx-meta tx)))

                                :else
                                (report-line (d/transact! conn (resolve-args env tx))))
                              (catch :default e (error-line-quiet e))))
                      (:txs step))]
        [(str "#conn " (p lines) " " (p (mapv (fn [r] {:tx-data (vec (:tx-data r)) :tempids (:tempids r) :tx-meta (:tx-meta r)}) @reports))
           " " (db-line @conn))
         @conn])

      ;; --- the runtime underneath: ClojureScript's own hash, equality, order and printing
      :cljs/hash    [(p (hash (arg :val))) nil]
      :cljs/pr-str  [(p (p (arg :val))) nil]
      :cljs/str     [(p (str (arg :val))) nil]
      :cljs/eq      [(p (= (arg :a) (arg :b))) nil]
      :cljs/compare [(p (let [c (db/value-compare (arg :a) (arg :b))] (cond (neg? c) -1 (pos? c) 1 :else 0))) nil]
      :cljs/set     [(p (set (arg :vals))) nil]
      :cljs/into-map [(p (into {} (arg :pairs))) nil]
      :cljs/assoc   [(p (apply assoc (arg :map) (arg :kvs))) nil]
      :cljs/dissoc  [(p (apply dissoc (arg :map) (arg :ks))) nil]
      :cljs/conj    [(p (apply conj (arg :coll) (arg :xs))) nil]
      :cljs/disj    [(p (apply disj (arg :coll) (arg :xs))) nil]
      :cljs/sort    [(p (vec (sort db/value-compare (arg :vals)))) nil]
      :cljs/read    [(p (reader/read-string (arg :string))) nil]
      :cljs/group-by-first [(p (group-by first (arg :vals))) nil]
      :cljs/distinct [(p (vec (distinct (arg :vals)))) nil]
      :cljs/frequencies [(p (frequencies (arg :vals))) nil]
      :cljs/zipmap  [(p (zipmap (arg :ks) (arg :vs))) nil]
      :cljs/merge   [(p (apply merge (arg :maps))) nil]
      :cljs/select-keys [(p (select-keys (arg :map) (arg :ks))) nil]

      ;; --- the parsers, printed as their records print
      :parse-query   [(p (dp/parse-query (arg :query))) nil]
      :parse-pull    [(p (walk/postwalk #(if (fn? %) :fn %) (into {} (dpp/parse-pattern (arg :db) (arg :pattern))))) nil]

      (throw (js/Error. (str "oracle: unknown op " op))))))

(defn- run-case [steps]
  (loop [env {} steps (seq steps) out []]
    (if-not steps
      out
      (let [step (first steps)
            [line value] (try
                           (run-step env step)
                           (catch :default e [(error-line e) nil]))]
        (recur (if-some [k (:as step)] (assoc env k value) env)
          (next steps)
          (conj out line))))))

(defn- read-case [line]
  (reader/read-string {:readers {'r ->Ref 'f ->FnRef}} line))

(defn -main [& args]
  (let [[in out] args
        lines (->> (str/split (.readFileSync fs in "utf8") #"\n")
                (remove #(or (str/blank? %) (str/starts-with? % ";"))))
        sb    (array)]
    (doseq [[i line] (map-indexed vector lines)]
      (.push sb (str "== " i))
      (try
        (doseq [l (run-case (read-case line))]
          (.push sb l))
        (catch :default e
          (.push sb (str "#case-error " (p (str (or (ex-message e) e))))))))
    (.push sb "")
    (.writeFileSync fs out (.join sb "\n"))))

(set! *main-cli-fn* -main)
