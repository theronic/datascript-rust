(ns datascript.test.embedded
  "The module built into another program (crates/datascript-wasm/examples/embedded.rs): the program's
  own operations, called as the module's are, and a connection held here that the program transacts
  on. Run by `cljs/test.sh simple embedded`; on the module itself, which has no such operations,
  there is nothing to try."
  (:require
    [clojure.test :as t :refer [is deftest testing]]
    [datascript.conn :as conn]
    [datascript.core :as d]
    [datascript.db :as db]
    [datascript.wasm :as wasm]
    [datascript.wasm-node :as node]))

;; the program's operations: the first numbers that are none of the module's own
(def ^:const op-count 1000)
(def ^:const op-transact 1001)

(defn- transact-there!
  "The transaction made by the program, in the module, of the database the connection holds: how many
  datoms it added and retracted. The connection moves on here, and its listeners are told."
  [conn tx-data]
  (wasm/call op-transact
    #js [(fn [] @conn)
         (fn [before after tx-data tempids]
           (conn/-moved-on! conn (db/->TxReport before after tx-data tempids nil))
           nil)
         tx-data]))

(defn- datoms [report]
  (mapv (juxt :e :a :v :tx :added) (:tx-data report)))

(deftest test-the-programs-operations
  (when node/embedded?
    (testing "an operation of the program's reads a database the host names, where the database is"
      (let [db (d/db-with (d/empty-db {:aka {:db/cardinality :db.cardinality/many}})
                 [{:db/id 1 :name "Ivan" :aka ["I" "V" "Vanya"]}
                  {:db/id 2 :name "Petr"}])]
        (is (= 2 (wasm/call op-count #js [db :name])))
        (is (= 3 (wasm/call op-count #js [db :aka])))
        (is (= 0 (wasm/call op-count #js [db :age])))
        (is (= 3 (wasm/call op-count #js [(d/db-with db [[:db/add 3 :name "Oleg"]]) :name])))))
    (testing "what an operation of the program's fails with is thrown here"
      (is (thrown-with-msg? js/Error #"a database, and an attribute" (wasm/call op-count #js [1 2])))
      (is (thrown-with-msg? js/Error #"no operation 1999" (wasm/call 1999 #js []))))))

(deftest test-the-program-transacts-on-a-connection
  (when node/embedded?
    (let [schema  {:name {:db/unique :db.unique/identity}
                   :friend {:db/valueType :db.type/ref}}
          conn    (d/create-conn schema)
          twin    (d/create-conn schema)
          reports (atom [])
          tx      [{:db/id "p" :name "Petr" :age 37}
                   [:db/add [:name "Ivan"] :friend "p"]
                   [:db/retract [:name "Ivan"] :age 15]]]
      (d/listen! conn :test #(swap! reports conj %))
      (d/transact! conn [{:name "Ivan" :age 15}])
      (d/transact! twin [{:name "Ivan" :age 15}])
      (testing "the connection moves on to what transact! would have made, and its listeners are told the same"
        (let [before   @conn
              n        (transact-there! conn tx)
              expected (d/transact! twin tx)
              report   (peek @reports)]
          (is (= 4 n))
          (is (= 2 (count @reports)))
          (is (identical? before (:db-before report)))
          (is (identical? @conn (:db-after report)))
          (is (= @twin @conn))
          (is (= (datoms expected) (datoms report)))
          (is (= (:tempids expected) (:tempids report)))
          (is (= (:max-tx @twin) (:max-tx @conn)))
          (is (= (mapv (juxt :e :a :v :tx) (d/datoms @twin :eavt))
                (mapv (juxt :e :a :v :tx) (d/datoms @conn :eavt))))))
      (testing "and on from there, by transact! and by the program in turn"
        (dotimes [i 20]
          (let [tx [[:db/add [:name "Petr"] :age (+ 38 i)] {:name (str "n" i) :friend [:name "Ivan"]}]]
            (if (even? i)
              (d/transact! conn tx)
              (transact-there! conn tx))
            (d/transact! twin tx)))
        (is (= @twin @conn))
        (is (= 22 (count @reports)))
        (is (= 57 (:age (d/entity @conn [:name "Petr"])))))
      (testing "the database before is the value it was"
        (let [before (:db-before (nth @reports 1))]
          (is (= 15 (:age (d/entity before [:name "Ivan"]))))
          (is (nil? (d/entity before [:name "Petr"])))))
      (testing "what the transaction throws is thrown here, and the connection is as it was"
        (let [held  @conn
              told  (count @reports)]
          (is (thrown-with-msg? ExceptionInfo #"Cannot add .* because of unique constraint"
                (transact-there! conn [[:db/add 1 :name "Petr"]])))
          (is (identical? held @conn))
          (is (= told (count @reports)))))
      (testing "what a listener throws is thrown here too, once the connection has moved on"
        (d/listen! conn :thrower (fn [_] (throw (ex-info "the listener's own" {:of :listener}))))
        (let [held @conn]
          (is (= {:of :listener}
                (ex-data (try (transact-there! conn [[:db/add 1 :age 16]]) (catch ExceptionInfo e e)))))
          (is (not (identical? held @conn)))
          (is (= 16 (:age (d/entity @conn 1)))))
        (d/unlisten! conn :thrower)))))
