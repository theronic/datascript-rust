(ns datascript.test.wasm
  "What is particular to DataScript over the WebAssembly module: what crosses the boundary, and how."
  (:require
    [clojure.test :as t :refer [is are deftest testing async]]
    [datascript.core :as d]
    [datascript.db :as db]
    [datascript.serialize :as serialize]
    [datascript.wasm :as wasm]))

(defrecord Point [x y])

(deftype Opaque [n])

(def people
  (d/db-with (d/empty-db {:friend {:db/valueType :db.type/ref}
                          :aka    {:db/cardinality :db.cardinality/many}})
    [{:db/id 1 :name "Ivan" :age 15 :aka ["I" "V"] :friend 2}
     {:db/id 2 :name "Petr" :age 37}
     {:db/id 3 :name "Oleg" :age 37 :friend 1}]))

(deftest test-values-keep-their-identity
  (testing "a value the module has no form for comes back the value it was"
    (let [o  (Opaque. 1)
          a  (atom 1)
          js #js {:a 1}]
      (is (identical? o (d/q '[:find ?x . :in ?x] o)))
      (is (identical? a (d/q '[:find ?x . :in ?x] a)))
      (is (identical? js (d/q '[:find ?x . :in ?x] js)))
      (is (= #{[o]} (d/q '[:find ?x :in [?x ...]] [o o])))))
  (testing "as a datom's value"
    (let [o  (Opaque. 2)
          p  (->Point 1 2)
          db (d/db-with (d/empty-db) [[:db/add 1 :obj o] [:db/add 1 :point p]])]
      (is (identical? o (:obj (d/entity db 1))))
      (is (identical? p (:v (first (d/datoms db :eavt 1 :point)))))
      (is (= {:obj o :point p} (d/pull db [:obj :point] 1)))
      (is (= #{[1]} (d/q '[:find ?e :in $ ?p :where [?e :point ?p]] db (->Point 1 2))))
      (is (= #{} (d/q '[:find ?e :in $ ?p :where [?e :point ?p]] db {:x 1 :y 2})))))
  (testing "a function is called with the values it was given"
    (let [o    (Opaque. 3)
          seen (atom nil)]
      (is (= #{[true]} (d/q '[:find ?r :in ?f ?x :where [(?f ?x) ?r]]
                         (fn [x] (reset! seen x) true) o)))
      (is (identical? o @seen))))
  (testing "types are the host's own"
    (is (identical? js/Number (d/q '[:find ?t . :in ?x :where [(type ?x) ?t]] 1)))
    (is (identical? Keyword (d/q '[:find ?t . :in ?x :where [(type ?x) ?t]] :k)))
    (is (identical? Opaque (d/q '[:find ?t . :in ?x :where [(type ?x) ?t]] (Opaque. 4))))
    (is (identical? Point (d/q '[:find ?t . :in ?x :where [(type ?x) ?t]] (->Point 1 2))))
    (is (= #{[1]} (d/q '[:find ?x :in [?x ...] ?t :where [(type ?x) ?t]] [1 "a" :b] js/Number)))))

(deftest test-values-the-host-answers-for
  (testing "a record is the map it is, to a transaction"
    (let [db (d/db-with (d/empty-db) [(->Point 1 2) (assoc (->Point 3 4) :db/id 10)])]
      (is (= #{[1 :x 1] [1 :y 2] [10 :x 3] [10 :y 4]}
            (set (map (juxt :e :a :v) (d/datoms db :eavt)))))))
  (testing "and to a query's functions"
    (let [p (->Point 1 2)]
      (is (= 2 (d/q '[:find ?y . :in ?p :where [(get ?p :y) ?y]] p)))
      (is (= :none (d/q '[:find ?y . :in ?p :where [(get ?p :z :none) ?y]] p)))
      (is (= 2 (d/q '[:find ?n . :in ?p :where [(count ?p) ?n]] p)))
      (is (= true (d/q '[:find ?r . :in ?p :where [(contains? ?p :x) ?r]] p)))
      (is (= #{[[:x 1]] [[:y 2]]} (d/q '[:find ?kv :in [?kv ...]] p)))))
  (testing "an entity is asked for its attributes"
    (let [e (d/entity people 1)]
      (is (= "Ivan" (d/q '[:find ?n . :in ?e :where [(get ?e :name) ?n]] e)))
      (is (= "Ivan" (d/q '[:find ?n . :in ?e :where [(?e :name) ?n]] e)))
      (is (= #{["Ivan"] ["Oleg"]}
            (d/q '[:find ?n
                   :in $ [?e ...] ?k
                   :where [(?k ?e) ?n]]
              people [(d/entity people 1) (d/entity people 3)] :name)))))
  (testing "what a value of the host's throws is what the caller is thrown"
    (let [boom (js/Error. "boom")
          bad  (reify ICounted (-count [_] (throw boom)))]
      (is (identical? boom
            (try (d/q '[:find ?n . :in ?x :where [(count ?x) ?n]] bad)
              (catch :default e e)))))))

(deftest test-exceptions-pass-through
  (let [conn (d/create-conn)
        boom (ex-info "from a transaction function" {:mine true})]
    (is (identical? boom
          (try (d/transact! conn [[:db.fn/call (fn [_] (throw boom))]])
            (catch :default e e))))
    (is (identical? boom
          (try (d/q '[:find ?x :in ?f :where [(?f) ?x]] (fn [] (throw boom)))
            (catch :default e e))))
    (is (identical? boom
          (try (vec (d/datoms (d/filter people (fn [_ _] (throw boom))) :eavt))
            (catch :default e e))))
    (testing "and the database is as it was"
      (is (= 0 (count @conn)))
      (d/transact! conn [[:db/add 1 :name "Ivan"]])
      (is (= 1 (count @conn))))))

(deftest test-transaction-functions
  (testing "a function is called with the database of the transaction so far"
    (let [conn (d/create-conn)
          seen (atom [])]
      (d/transact! conn
        [[:db/add 1 :n 1]
         [:db.fn/call (fn [db] (swap! seen conj (count db)) [[:db/add 2 :n (count (d/datoms db :eavt))]])]
         [:db.fn/call (fn [db] (swap! seen conj (:n (d/entity db 2))) #js [])]])
      (is (= [1 1] @seen))
      (is (= #{[1 1] [2 1]} (d/q '[:find ?e ?n :where [?e :n ?n]] @conn)))))
  (testing "JavaScript's arrays are vectors to the module"
    (let [db (d/db-with (d/empty-db) #js [#js [:db/add 1 :name "Ivan"]])]
      (is (= #{["Ivan"]} (d/q '[:find ?n :where [_ :name ?n]] db)))
      (is (= #{[1]} (d/q '[:find ?e :in $ [?n ...] :where [?e :name ?n]] db #js ["Ivan" "Petr"]))))))

(deftest test-reports
  (let [conn    (d/create-conn)
        reports (atom [])
        meta    (atom :tx-meta)]
    (d/listen! conn :test #(swap! reports conj %))
    (d/transact! conn [[:db/add 1 :name "Ivan"]] meta)
    (d/transact! conn [[:db/add 2 :name "Petr"]])
    (let [[r1 r2] @reports]
      (is (identical? meta (:tx-meta r1)))
      (is (identical? (:db-after r1) (:db-before r2)))
      (is (identical? (:db-after r2) @conn))
      (is (= [(d/datom 2 :name "Petr" (+ d/tx0 2))] (:tx-data r2))))))

(deftest test-runs-of-datoms
  (let [n     3000
        db    (d/db-with (d/empty-db) (for [i (range 1 (inc n))] [:db/add i :n i]))
        ds    (d/datoms db :eavt)]
    (is (= n (count ds)))
    (is (= (range 1 (inc n)) (map :e ds)))
    (is (= (range n 0 -1) (map :e (rseq ds))))
    (is (= (range n 0 -1) (map :e (reverse ds))))
    (is (= [1 2 3] (map :e (take 3 ds))))
    (is (= 1000 (:e (nth ds 999))))
    (is (= (reduce + (range 1 (inc n))) (reduce (fn [acc d] (+ acc (:v d))) 0 ds)))
    (is (= 10 (reduce (fn [acc d] (if (= 10 (:e d)) (reduced (:e d)) acc)) 0 ds)))
    (is (= (range 2001 (inc n)) (map :e (d/seek-datoms db :eavt 2001))))
    (is (= (range 1000 0 -1) (map :e (d/rseek-datoms db :eavt 1000))))
    (is (= ds (seq ds)))
    (is (= (vec ds) (into [] ds)))
    (is (= n (count (into #{} ds))))
    (testing "as JavaScript iterates"
      (let [it (es6-iterator (d/seek-datoms db :eavt 2990))]
        (is (= (range 2990 (inc n))
              (loop [out []]
                (let [step (.next it)]
                  (if (.-done step) out (recur (conj out (.-e (.-value step)))))))))))
    (testing "a datom's fields are JavaScript's properties"
      (let [d (first ds)]
        (is (= [1 :n 1] [(.-e d) (.-a d) (.-v d)]))
        (is (= (inc d/tx0) (.-tx d)))))
    (testing "a run that is read in parts, from where the part before ended"
      (let [parts (take-while some? (iterate chunk-next (d/datoms db :eavt)))]
        (is (< 3 (count parts)))
        (is (= n (reduce + (map #(count (chunk-first %)) parts))))))))

(deftest test-deep-values
  (let [depth 20000
        deep  (reduce (fn [acc _] [acc]) [:bottom] (range depth))
        db    (d/db-with (d/empty-db) [[:db/add 1 :deep deep]])
        back  (:v (first (d/datoms db :eavt 1 :deep)))]
    (testing "a value nested deeper than the stack goes through and comes back"
      (is (= :bottom (loop [x back] (if (vector? (first x)) (recur (first x)) (first x)))))
      (is (= depth (loop [x back n 0] (if (vector? (first x)) (recur (first x) (inc n)) n)))))
    (testing "what the stack has no room for ends as it does in ClojureScript, and leaves the module in order"
      (dotimes [_ 20]
        (is (thrown? js/RangeError (d/q '[:find ?s . :in ?x :where [(str ?x) ?s]] back))))
      (is (= 1 (d/q '[:find ?e . :where [?e :deep]] db)))
      (is (= "Ivan" (d/q '[:find ?n . :where [1 :name ?n]] people))))
    (testing "a hash the stack had no room for is not half kept: it is asked for again, and runs out again"
      ;; a join on the value hashes it, where it is, in the database
      (let [db2 (d/db-with db [[:db/add 2 :deep back]])]
        (dotimes [_ 5]
          (is (thrown? js/RangeError (d/q '[:find ?e ?e2 :where [?e :deep ?v] [?e2 :deep ?v]] db2))))
        (is (= #{[1] [2]} (d/q '[:find ?e :where [?e :deep]] db2)))))))

(deftest test-databases-are-values
  (let [db1 (d/db-with (d/empty-db) [[:db/add 1 :name "Ivan"]])
        db2 (d/db-with (d/empty-db) [[:db/add 1 :name "Ivan"]])]
    (is (= db1 db2))
    (is (= (hash db1) (hash db2)))
    (is (not (identical? db1 db2)))
    (is (identical? db1 (d/q '[:find ?db . :in ?db] db1)))
    (is (= {:a 1} (meta (with-meta db1 {:a 1}))))
    (is (= db1 (with-meta db1 {:a 1})))
    (is (= #{["Ivan"]} (d/q '[:find ?n :where [_ :name ?n]] (with-meta db1 {:a 1}))))
    (is (= {db1 :found} {db2 :found}))
    (is (= "#datascript/DB {:schema nil, :datoms [[1 :name \"Ivan\" 536870913]]}" (pr-str db1)))
    (is (= db1 (cljs.reader/read-string (pr-str db1))))))

(deftest test-a-database-as-text
  (let [db (d/db-with (d/empty-db {:aka {:db/cardinality :db.cardinality/many} :age {:db/index true}})
             [{:db/id 1 :name "Petr \"the\" \\ Great\n\u0001é😀" :aka ["Devil" "Tupen"] :age 15 :kw :some/kw
               :attach {:k [1 2]}}
              {:db/id 2 :inf ##Inf :ninf ##-Inf :nan ##NaN :b false :age 15 :half 0.5 :big 1e21 :neg -7}])
        text (serialize/json db)]
    (testing "the text the module writes is the text JSON.stringify makes of serializable"
      (is (string? text))
      (is (= text (js/JSON.stringify (d/serializable db)))))
    (testing "and it reads back as the database, as text and as JavaScript's data"
      ;; NaN is not NaN: compared as printed
      (is (= (pr-str db) (pr-str (serialize/from-json text))))
      (is (= (pr-str db) (pr-str (d/from-serializable (js/JSON.parse text)))))
      (is (= (vec (d/datoms db :avet)) (vec (d/datoms (serialize/from-json text) :avet)))))
    (testing "with what stands for values and keywords chosen by the program"
      (let [freeze {:freeze-fn #(str "!" (pr-str %)) :freeze-kw #(subs (str %) 1)}
            thaw   {:thaw-fn #(cljs.reader/read-string (subs % 1)) :thaw-kw keyword}
            text   (serialize/json db freeze)]
        (is (= text (js/JSON.stringify (d/serializable db freeze))))
        (is (re-find #"\"some/kw\"" text))
        (is (= (pr-str db) (pr-str (serialize/from-json text thaw))))))
    (testing "what is no database is told so, the same whichever way it is read"
      (doseq [bad ["{}" "[]" "7" "{\"tx0\":1}"
                   "{\"tx0\":536870912,\"schema\":\"nil\",\"attrs\":[],\"keywords\":[],\"eavt\":7}"
                   "{\"tx0\":536870912,\"schema\":\"nil\",\"attrs\":[\":a\"],\"keywords\":[],\"eavt\":[[1,0,[9],1]]}"]]
        (let [told #(try (%) :read (catch :default e (ex-message e)))]
          (is (string? (told #(serialize/from-json bad))))
          (is (= (told #(d/from-serializable (js/JSON.parse bad))) (told #(serialize/from-json bad))))))
      (is (thrown? js/Error (serialize/from-json "{\"tx0\":")))
      (is (thrown? js/Error (serialize/from-json "no JSON at all"))))
    (testing "a database of many datoms, which is read a row at a time"
      (let [many (d/db-with (d/empty-db {:name {:db/index true} :aka {:db/cardinality :db.cardinality/many}})
                   (for [i (range 1 3001)]
                     {:db/id i :name (str "n" i) :age (mod i 90) :aka [(str "a" i) (str "b" (mod i 7))] :kw (keyword (str "k" (mod i 5)))}))
            text (serialize/json many)]
        (is (= text (js/JSON.stringify (d/serializable many))))
        (is (= many (serialize/from-json text)))
        (is (= (vec (d/datoms many :aevt)) (vec (d/datoms (serialize/from-json text) :aevt))))
        (is (= (vec (d/datoms many :avet)) (vec (d/datoms (d/from-serializable (js/JSON.parse text)) :avet))))))))

(deftest test-a-connection-moves-on
  (testing "the values a connection moved on from are the values they were, however far it has moved on since"
    (let [schema  {:name {:db/index true}}
          tx      (fn [i] [[:db/add (inc (mod i 10)) :n i]
                           {:db/id (inc (mod i 7)) :name (str "n" i)}
                           [:db/retract (inc (mod i 7)) :name (str "n" (- i 7))]])
          conn    (d/create-conn schema)
          reports (atom [])
          _       (d/listen! conn :test #(swap! reports conj %))
          _       (dotimes [i 300] (d/transact! conn (tx i)))
          ;; the same transactions, each on the value before and none moved on from
          values  (vec (reductions (fn [db i] (d/db-with db (tx i))) (d/empty-db schema) (range 300)))
          datoms  (fn [db] [(mapv (juxt :e :a :v :tx) (d/datoms db :eavt))
                            (mapv (juxt :e :a :v :tx) (d/datoms db :avet))
                            (:max-eid db) (:max-tx db)])]
      (is (= 300 (count @reports)))
      (is (= (datoms (values 0)) (datoms (:db-before (first @reports)))))
      (is (= (datoms (values 150)) (datoms (:db-after (nth @reports 149)))))
      (is (every? true? (map (fn [r before after]
                               (and (= (datoms before) (datoms (:db-before r)))
                                 (= (datoms after) (datoms (:db-after r)))))
                          @reports values (rest values))))
      (is (= (values 37) (:db-before (nth @reports 37))))
      (is (= "n35" (:name (d/entity (:db-before (nth @reports 37)) 1))))
      (is (= #{[1]} (d/q '[:find ?e :where [?e :name "n35"]] (:db-before (nth @reports 37)))))
      (is (= (last values) @conn)))))

(deftest test-a-run-read-while-its-connection-moves-on
  (let [schema {:tag {:db/index true}}
        fresh  (fn []
                 (let [conn (d/create-conn schema)]
                   (d/transact! conn (for [i (range 1 1501)] {:db/id i :n i :tag (str "t" (mod i 13))}))
                   conn))
        row    (juxt :e :a :v :tx)
        ;; reads a run a datom at a time, transacting on the connection on the way: early on, two transactions that
        ;; change the indexes all through what is still to be read, so that the database being read, made again from
        ;; the one after them, is built another way; and then one every so often at the datoms just read
        walk   (fn [conn run]
                 (let [seen (volatile! [])]
                   (doseq [d run]
                     (vswap! seen conj (row d))
                     (case (count @seen)
                       40 (d/transact! conn (for [e (range 1 1500 4)] [:db/add e :extra e]))
                       45 (d/transact! conn (for [e (range 2 1500 7)] [:db/add e :tag "t5x"]))
                       (when (zero? (mod (count @seen) 100))
                         (d/transact! conn [[:db/add (:e d) :seen (count @seen)]
                                            [:db/retract (:e d) :tag (str "t" (mod (:e d) 13))]
                                            {:db/id (+ 5000 (count @seen)) :tag "t5" :n 0}]))))
                   @seen))]
    (testing "a run of datoms of a database a connection has since moved on from is read to its end as it was"
      (are [read] (let [conn     (fresh)
                        db       @conn
                        expected (mapv row (into [] (read db)))]
                    (= expected (walk conn (read db))))
        #(d/datoms % :eavt)
        #(d/datoms % :aevt)
        #(d/datoms % :avet)
        #(d/seek-datoms % :eavt 700)
        #(d/rseek-datoms % :eavt 700)
        #(rseq (d/datoms % :aevt :tag))
        #(d/index-range % :tag "t1" "t7")
        #(d/datoms (d/filter % (fn [_ datom] (odd? (:e datom)))) :aevt)))
    (testing "and counts as it did"
      (let [conn (fresh)
            db   @conn
            run  (d/datoms db :eavt)]
        (d/transact! conn [[:db/add 1 :n 100] [:db/add 9000 :n 1]])
        (is (= 3000 (count run)))
        (is (= 3001 (count (d/datoms @conn :eavt))))))))

(defn- settle
  "Calls back once the garbage collector has run and what it frees has been let go of."
  [rounds done]
  (if (zero? rounds)
    (done)
    (do
      (js/gc)
      (js/setTimeout #(settle (dec rounds) done) 10))))

(deftest test-databases-are-let-go-of
  (if-not (exists? js/gc)
    (println "  (handles: run node with --expose-gc to check that the module lets databases go)")
    (async done
      (settle 5
        (fn []
          (let [before (wasm/held)]
            ;; databases, filtered databases with a function of the host's each, and half-read runs of datoms
            (dotimes [i 500]
              (let [db (d/db-with (d/empty-db) (for [j (range 100)] [:db/add (inc j) :n (+ i j)]))
                    f  (d/filter db (fn [_ d] (odd? (:v d))))]
                (first (d/datoms db :eavt))
                (first (d/datoms f :eavt))))
            (is (< (+ (:dbs before) 500) (:dbs (wasm/held))))
            (settle 10
              (fn []
                (let [after (wasm/held)]
                  (println "  handles before:" (pr-str before) "after:" (pr-str after))
                  (is (<= (:dbs after) (+ (:dbs before) 4)))
                  (is (<= (:host-fns after) (+ (:host-fns before) 2)))
                  (done))))))))))
