(ns datascript.boundary-bench
  "How long the calls a program makes of DataScript take: the same program compiled twice, over
  ClojureScript DataScript (../src) and over the WebAssembly module (cljs/src), in Node.
  cljs/bench.sh runs both and prints them side by side."
  (:require
    [datascript.bench-setup]
    [datascript.core :as d]
    [goog.object :as gobj]))

(def n-people 20000)

(def schema
  {:id      {:db/unique :db.unique/identity}
   :follows {:db/valueType :db.type/ref :db/cardinality :db.cardinality/many}
   :alias   {:db/cardinality :db.cardinality/many}})

(defn- person [i]
  {:db/id     (inc i)
   :id        (str "p" i)
   :name      (str "Name" (mod i 997))
   :last-name (str "Last" (mod i 1009))
   :age       (+ 10 (mod (* i 7) 70))
   :salary    (* 100 (mod (* i 13) 1000))
   :alias     [(str "a" i) (str "b" i)]
   :follows   [(inc (mod (* i 31) n-people)) (inc (mod (* i 17) n-people))]})

(defn- now [] (js-invoke js/performance "now"))

(defn- settle
  "Calls back once the garbage collector has run, where it can be asked to, and what it frees has been let go
  of: the database values a round made are the module's to drop only then."
  [done]
  (if (exists? js/gc)
    (do (js/gc) (js/setTimeout (fn [] (js/gc) (js/setTimeout done 0)) 0))
    (js/setTimeout done 0)))

(defn- measure
  "Microseconds a call of f takes, given to `done`: the best of a few rounds, each of enough calls to time, with
  the garbage collected between them."
  [f done]
  (dotimes [_ 30] (f 0))
  (let [per-round (loop [n 1]
                    (let [t0 (now)]
                      (dotimes [i n] (f i))
                      (if (or (> (- (now) t0) 20) (>= n 20000)) n (recur (* n 4)))))
        round     (fn round [left best]
                    (if (zero? left)
                      (done best)
                      (settle
                        (fn []
                          (let [t0 (now)]
                            (dotimes [i per-round] (f i))
                            (round (dec left) (min best (/ (* 1000 (- (now) t0)) per-round))))))))]
    (round 7 js/Infinity)))

(defn -main [& _]
  (let [t0     (now)
        people (mapv person (range n-people))
        db     (d/db-with (d/empty-db schema) people)
        built  (- (now) t0)
        conn   (d/conn-from-db db)
        id     (fn [i] (inc (mod (* i 7919) n-people)))
        out    #js {}
        benches (array)
        ;; BENCH_ONLY names the one benchmark to run, for two seconds: for a profiler
        only   (gobj/getValueByKeys js/process "env" "BENCH_ONLY")
        bench  (fn [name f]
                 (cond
                   (nil? only) (.push benches #js [name f])
                   (.includes name only) (let [until (+ (now) 2000)]
                                           (loop [i 0]
                                             (f i)
                                             (when (< (now) until) (recur (inc i)))))))]
    (gobj/set out "transact 20k entities, ms" built)
    (bench "datoms, an attribute's value: (first (d/datoms db :eavt e :name))"
      (fn [i] (:v (first (d/datoms db :eavt (id i) :name)))))
    (bench "datoms, an entity's: (vec (d/datoms db :eavt e))"
      (fn [i] (count (vec (d/datoms db :eavt (id i))))))
    (bench "find-datom"
      (fn [i] (:v (d/find-datom db :eavt (id i) :name))))
    (bench "seek-datoms, the first three"
      (fn [i] (count (into [] (take 3) (d/seek-datoms db :eavt (id i))))))
    (bench "rseek-datoms, the first"
      (fn [i] (:e (first (d/rseek-datoms db :eavt (id i))))))
    (bench "index-range, ten ages"
      (fn [i] (count (take 10 (d/index-range db :id (str "p" (id i)) nil)))))
    (bench "entid of a lookup ref"
      (fn [i] (d/entid db [:id (str "p" (id i))])))
    (bench "entity, one attribute"
      (fn [i] (:name (d/entity db (id i)))))
    (bench "entity, touched"
      (fn [i] (count (d/touch (d/entity db (id i))))))
    (bench "pull, two attributes"
      (fn [i] (d/pull db [:name :age] (id i))))
    (bench "pull, wildcard"
      (fn [i] (d/pull db '[*] (id i))))
    (bench "q, one entity's attribute"
      (fn [i] (d/q '[:find ?n . :in $ ?e :where [?e :name ?n]] db (id i))))
    (bench "q, a join, about 20 rows"
      (fn [i] (count (d/q '[:find ?e ?l :in $ ?n :where [?e :name ?n] [?e :last-name ?l]] db (str "Name" (mod i 997))))))
    (bench "q, a predicate over 20k"
      (fn [_] (count (d/q '[:find ?e :where [?e :salary ?s] [(> ?s 99000)]] db))))
    (bench "q, with a function of the program's over 20k"
      (fn [_] (count (d/q '[:find ?e :in $ ?f :where [?e :age ?a] [(?f ?a)]] db (fn [a] (> a 78))))))
    (bench "with, one datom"
      (fn [i] (d/db-with db [[:db/add (id i) :name "New"]])))
    (bench "with, an entity of five attributes"
      (fn [i] (d/db-with db [{:db/id -1 :id (str "n" i) :name "New" :last-name "Newer" :age 30 :salary 1}])))
    (bench "transact!, one datom, with a listener"
      (let [seen (volatile! 0)]
        (d/listen! conn :bench (fn [_] (vswap! seen inc)))
        (fn [i] (d/transact! conn [[:db/add (id i) :age (mod i 90)]]))))
    (bench "datoms, all 180k counted"
      (fn [_] (count (seq (d/datoms db :eavt)))))
    (bench "datoms, all 180k reduced"
      (fn [_] (reduce (fn [acc d] (+ acc (.-e d))) 0 (d/datoms db :aevt))))
    (bench "filter, then datoms of an entity"
      (let [f (d/filter db (fn [_ d] (not= :salary (.-a d))))]
        (fn [i] (count (vec (d/datoms f :eavt (id i)))))))
    ((fn run [i]
       (if (< i (.-length benches))
         (let [[name f] (aget benches i)]
           (measure f (fn [us] (gobj/set out name us) (run (inc i)))))
         (println (js/JSON.stringify out))))
     0)))

(set! *main-cli-fn* -main)
