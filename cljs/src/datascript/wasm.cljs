(ns datascript.wasm
  "DataScript's WebAssembly module, from ClojureScript: loading it, calling it, and the form values
  cross its boundary in. The tags, the operations and the imports are crates/datascript-wasm's.

  A page loads the module before it uses DataScript:

      (-> (js/fetch \"datascript.wasm\")
        (.then #(datascript.wasm/instantiate %))
        (.then start-the-app))

  or, where compiling may block (Node, a worker): (datascript.wasm/instantiate-sync bytes)."
  (:require
    [goog.object :as gobj]))

;; ---------------------------------------------------------------- the interface's numbers

(def ^:const abi-version 1)

(def ^:const t-nil 0)
(def ^:const t-false 1)
(def ^:const t-true 2)
(def ^:const t-int 3)
(def ^:const t-f64 4)
(def ^:const t-str 5)
(def ^:const t-kw-def 6)
(def ^:const t-kw-ref 7)
(def ^:const t-sym-def 8)
(def ^:const t-sym-ref 9)
(def ^:const t-vector 10)
(def ^:const t-list 11)
(def ^:const t-array-map 12)
(def ^:const t-hash-map 13)
(def ^:const t-array-set 14)
(def ^:const t-hash-set 15)
(def ^:const t-uuid 16)
(def ^:const t-inst 17)
(def ^:const t-regex 18)
(def ^:const t-datom 19)
(def ^:const t-db 20)
(def ^:const t-host-fn 21)
(def ^:const t-host-obj 22)
(def ^:const t-module-fn 23)
(def ^:const t-db-info 24)

(def ^:const op-empty-db 1)
(def ^:const op-init-db 2)
(def ^:const op-with 3)
(def ^:const op-with-schema 4)
(def ^:const op-filter 5)
(def ^:const op-unfiltered 6)
(def ^:const op-q 7)
(def ^:const op-pull 8)
(def ^:const op-pull-many 9)
(def ^:const op-datoms 10)
(def ^:const op-seek-datoms 11)
(def ^:const op-rseek-datoms 12)
(def ^:const op-index-range 13)
(def ^:const op-search 14)
(def ^:const op-cursor-next 15)
(def ^:const op-cursor-free 16)
(def ^:const op-find-datom 17)
(def ^:const op-entid 18)
(def ^:const op-db-info 19)
(def ^:const op-schema 20)
(def ^:const op-db-count 21)
(def ^:const op-db-equiv 22)
(def ^:const op-db-hash 23)
(def ^:const op-db-empty 24)
(def ^:const op-serializable 25)
(def ^:const op-from-serializable 26)
(def ^:const op-diff 27)
(def ^:const op-release-db 28)
(def ^:const op-release-fn 29)
(def ^:const op-call-fn 30)
(def ^:const op-held 31)
(def ^:const op-pr-str 32)
(def ^:const op-read-string 33)
(def ^:const op-hash 34)
(def ^:const op-set-option 35)
(def ^:const op-index-counts 36)

;; ---------------------------------------------------------------- what datascript.db fills in

(def ^{:doc "The Datom type, and (fn [e a v tx added]) that makes one."} datom-type nil)
(def datom-ctor nil)
(def ^{:doc "(fn [handle schema-uid max-eid max-tx filtered?]) → the database value of a handle"} db-ctor nil)
(def ^{:doc "(fn [x]) → the handle of a database value, nil of anything else"} db-handle nil)

;; ---------------------------------------------------------------- the module

(def ^:private memory nil)
(def ^:private ds-alloc nil)
(def ^:private ds-call nil)
(def ^:private ds-reply nil)
(def ^:private ds-result-ptr nil)
(def ^:private ds-result-len nil)
(def ^:private ds-recover nil)
;; the module's own stack, in its memory: the pointer to its top
(def ^:private stack-pointer nil)

(defn ^boolean ready?
  "Whether the module is loaded."
  []
  (some? ds-call))

(def ^:private text-encoder (js/TextEncoder.))
(def ^:private text-decoder (js/TextDecoder. "utf-8"))

;; ---------------------------------------------------------------- the host's functions and values, by handle

(deftype Handles [by-value table free])

(defn- handles [] (Handles. (js/Map.) (array) (array)))

(def ^:private host-fns (handles))
(def ^:private host-objs (handles))

(defn- mention!
  "The handle of a function or a value of the host's, counted once more: the module gives each mention back."
  [^Handles hs x]
  (let [by-value (.-by-value hs)
        entry    (.get by-value x)]
    (if (some? entry)
      (do
        (aset entry 1 (inc (aget entry 1)))
        (aget entry 0))
      (let [free   (.-free hs)
            table  (.-table hs)
            handle (if (pos? (.-length free)) (.pop free) (.-length table))]
        (aset table handle x)
        (.set by-value x #js [handle 1])
        handle))))

(defn- release! [^Handles hs handle]
  (let [table (.-table hs)
        x     (aget table handle)
        entry (.get (.-by-value hs) x)]
    (when (some? entry)
      (if (> (aget entry 1) 1)
        (aset entry 1 (dec (aget entry 1)))
        (do
          (.delete (.-by-value hs) x)
          (aset table handle nil)
          (.push (.-free hs) handle))))))

;; ---------------------------------------------------------------- writing

(deftype Enc [^:mutable buf ^:mutable dv ^:mutable pos ^:mutable kws ^:mutable syms])

(def ^:private enc-pool (array))
(def ^:private enc-depth 0)

(defn- enc-open
  "A writer. A call made while another is being written — a lazy sequence that queries as it is
  read — gets its own."
  []
  (let [depth enc-depth
        e     (or (aget enc-pool depth)
                (let [buf (js/Uint8Array. 4096)
                      e   (Enc. buf (js/DataView. (.-buffer buf)) 0 nil nil)]
                  (aset enc-pool depth e)
                  e))]
    (set! enc-depth (inc depth))
    (set! (.-pos e) 0)
    (set! (.-kws e) nil)
    (set! (.-syms e) nil)
    e))

(defn- enc-close []
  (set! enc-depth (dec enc-depth)))

(defn- room! [^Enc e n]
  (let [need (+ (.-pos e) n)
        buf  (.-buf e)]
    (when (> need (.-length buf))
      (let [bigger (js/Uint8Array. (js/Math.max need (* 2 (.-length buf))))]
        (.set bigger buf)
        (set! (.-buf e) bigger)
        (set! (.-dv e) (js/DataView. (.-buffer bigger)))))))

(defn- w-byte [^Enc e b]
  (room! e 1)
  (aset (.-buf e) (.-pos e) b)
  (set! (.-pos e) (inc (.-pos e))))

(defn- w-varint
  "A whole number from 0 up to 2^53."
  [^Enc e n]
  (room! e 10)
  (let [buf (.-buf e)]
    (loop [n n
           pos (.-pos e)]
      (if (>= n 128)
        (do
          (aset buf pos (bit-or (js-mod n 128) 128))
          (recur (js/Math.floor (/ n 128)) (inc pos)))
        (do
          (aset buf pos n)
          (set! (.-pos e) (inc pos)))))))

(defn- w-zigzag
  "A whole number of 32 bits."
  [^Enc e n]
  (w-varint e (unsigned-bit-shift-right (bit-xor (bit-shift-left n 1) (bit-shift-right n 31)) 0)))

(defn- w-f64 [^Enc e n]
  (room! e 8)
  (.setFloat64 (.-dv e) (.-pos e) n true)
  (set! (.-pos e) (+ (.-pos e) 8)))

(defn- w-str [^Enc e s]
  (let [n (.-length s)]
    (if (< n 64)
      ;; short, and most likely ASCII: a byte each, after a length of one byte
      (do
        (room! e (+ 1 n))
        (let [buf   (.-buf e)
              start (.-pos e)
              ascii (loop [i 0]
                      (if (< i n)
                        (let [c (.charCodeAt s i)]
                          (if (< c 128)
                            (do (aset buf (+ start 1 i) c) (recur (inc i)))
                            false))
                        true))]
          (if ascii
            (do
              (aset buf start n)
              (set! (.-pos e) (+ start 1 n)))
            (let [bytes (.encode text-encoder s)]
              (w-varint e (.-length bytes))
              (room! e (.-length bytes))
              (.set (.-buf e) bytes (.-pos e))
              (set! (.-pos e) (+ (.-pos e) (.-length bytes)))))))
      (let [bytes (.encode text-encoder s)]
        (w-varint e (.-length bytes))
        (room! e (.-length bytes))
        (.set (.-buf e) bytes (.-pos e))
        (set! (.-pos e) (+ (.-pos e) (.-length bytes)))))))

(defn- w-number [^Enc e n]
  ;; a whole number of 32 bits, -0 not among them
  (if (and (== (bit-or n 0) n) (not (and (== n 0) (neg? (/ 1 n)))))
    (do (w-byte e t-int) (w-zigzag e n))
    (do (w-byte e t-f64) (w-f64 e n))))

(defn- w-named [^Enc e table-key fqn def-tag ref-tag]
  (let [table (if (== def-tag t-kw-def)
                (or (.-kws e) (set! (.-kws e) (js/Map.)))
                (or (.-syms e) (set! (.-syms e) (js/Map.))))
        id    (.get table fqn)]
    (if (some? id)
      (do (w-byte e ref-tag) (w-varint e id))
      (do
        (.set table fqn (.-size table))
        (w-byte e def-tag)
        (w-str e fqn)))))

(declare w-value)

(defn- w-host-obj [^Enc e x]
  (w-byte e t-host-obj)
  (w-varint e (mention! host-objs x))
  (w-zigzag e (try (hash x) (catch :default _ 0))))

(defn- w-array [^Enc e tag ^array arr]
  (w-byte e tag)
  (w-varint e (.-length arr))
  (dotimes [i (.-length arr)]
    (w-value e (aget arr i))))

(defn- whole-int32? [n]
  (and (number? n) (== (bit-or n 0) n)))

(defn- w-datom [^Enc e d]
  (let [ee (.-e d)
        tx (.-tx d)]
    (if (and (whole-int32? ee) (whole-int32? tx))
      (do
        (w-byte e t-datom)
        (w-zigzag e ee)
        (w-value e (.-a d))
        (w-value e (.-v d))
        (w-zigzag e (if (neg? tx) (- tx) tx))
        (w-byte e (if (pos? tx) 1 0)))
      ;; ids the module has no datom for: as the transaction form of it, which says what is wrong with them
      (w-array e t-vector
        #js [(if (pos? tx) :db/add :db/retract) ee (.-a d) (.-v d) (if (neg? tx) (- tx) tx)]))))

(defn- w-value [^Enc e x]
  (cond
    (nil? x)     (w-byte e t-nil)
    (number? x)  (w-number e x)
    (string? x)  (do (w-byte e t-str) (w-str e x))
    (keyword? x) (w-named e nil (.-fqn x) t-kw-def t-kw-ref)
    (true? x)    (w-byte e t-true)
    (false? x)   (w-byte e t-false)

    (instance? PersistentVector x)
    (let [n (count x)]
      (w-byte e t-vector)
      (w-varint e n)
      (dotimes [i n]
        (w-value e (-nth x i))))

    (instance? PersistentArrayMap x)
    (let [arr (.-arr x)
          n   (.-length arr)]
      (w-byte e t-array-map)
      (w-varint e (/ n 2))
      (dotimes [i n]
        (w-value e (aget arr i))))

    (instance? datom-type x)
    (w-datom e x)

    (symbol? x)
    (w-named e nil (.-str x) t-sym-def t-sym-ref)

    (some? (db-handle x))
    (do (w-byte e t-db) (w-varint e (db-handle x)))

    (instance? PersistentHashMap x)
    (do
      (w-byte e t-hash-map)
      (w-varint e (count x))
      (reduce-kv (fn [_ k v] (w-value e k) (w-value e v) nil) nil x))

    (instance? PersistentHashSet x)
    (do
      (w-byte e (if (instance? PersistentArrayMap (.-hash-map x)) t-array-set t-hash-set))
      (w-varint e (count x))
      (reduce (fn [_ v] (w-value e v) nil) nil x))

    ;; a record is the map of its fields, a sorted map the map of its entries in their order
    (map? x)
    (let [entries (to-array (seq x))]
      (w-byte e t-array-map)
      (w-varint e (.-length entries))
      (dotimes [i (.-length entries)]
        (let [entry (aget entries i)]
          (w-value e (key entry))
          (w-value e (val entry)))))

    (set? x)
    (w-array e t-array-set (to-array x))

    ;; another vector: a subvec, a map entry
    (vector? x)
    (w-array e t-vector (to-array x))

    ;; a list, a lazy sequence: read to its end here
    (sequential? x)
    (w-array e t-list (to-array x))

    (array? x)
    (w-array e t-vector x)

    (uuid? x)
    (do (w-byte e t-uuid) (w-str e (.-uuid x)))

    (inst? x)
    (do (w-byte e t-inst) (w-f64 e (.getTime x)))

    (regexp? x)
    (do (w-byte e t-regex) (w-str e (.-source x)) (w-str e (.-flags x)))

    (some? (gobj/get x "datascript$moduleFn"))
    (do (w-byte e t-module-fn) (w-varint e (gobj/get x "datascript$moduleFn")) (w-str e ""))

    (fn? x)
    (do (w-byte e t-host-fn) (w-varint e (mention! host-fns x)))

    :else
    (w-host-obj e x)))

(defn- written
  "What a writer holds, in the module's memory: [ptr len]."
  [^Enc e]
  (let [len (.-pos e)
        ptr (unsigned-bit-shift-right (ds-alloc len) 0)]
    (.set (js/Uint8Array. (.-buffer memory) ptr len) (.subarray (.-buf e) 0 len))
    #js [ptr len]))

;; ---------------------------------------------------------------- reading

(deftype Dec [buf dv ^:mutable pos ^:mutable kws ^:mutable syms ^:mutable depth])

(defn- r-byte [^Dec d]
  (let [pos (.-pos d)]
    (set! (.-pos d) (inc pos))
    (aget (.-buf d) pos)))

(defn- r-varint [^Dec d]
  (let [buf (.-buf d)]
    (loop [pos (.-pos d)
           n   0
           mul 1]
      (let [b (aget buf pos)]
        (if (< b 128)
          (do
            (set! (.-pos d) (inc pos))
            (+ n (* b mul)))
          (recur (inc pos) (+ n (* (bit-and b 127) mul)) (* mul 128)))))))

(defn- r-zigzag [^Dec d]
  (let [n (r-varint d)]
    (bit-xor (unsigned-bit-shift-right n 1) (- (bit-and n 1)))))

(defn- r-f64 [^Dec d]
  (let [pos (.-pos d)]
    (set! (.-pos d) (+ pos 8))
    (.getFloat64 (.-dv d) pos true)))

(defn- r-str [^Dec d]
  (let [len   (r-varint d)
        buf   (.-buf d)
        start (.-pos d)
        end   (+ start len)]
    (set! (.-pos d) end)
    (if (< len 24)
      (loop [i start
             s ""]
        (if (< i end)
          (let [c (aget buf i)]
            (if (< c 128)
              (recur (inc i) (str s (js/String.fromCharCode c)))
              (.decode text-decoder (.subarray buf start end))))
          s))
      (.decode text-decoder (.subarray buf start end)))))

(def ^:private keywords
  "Keywords by name: one is made once."
  (js/Map.))

(defn- named-parts
  "ns/name as the module splits it: at the first slash, unless the name is the slash."
  [fqn]
  (let [i (.indexOf fqn "/")]
    (if (or (neg? i) (== (.-length fqn) 1))
      #js [nil fqn]
      #js [(.substring fqn 0 i) (.substring fqn (inc i))])))

(defn- keyword-of [fqn]
  (or (.get keywords fqn)
    (let [parts (named-parts fqn)
          k     (Keyword. (aget parts 0) (aget parts 1) fqn nil)]
      (.set keywords fqn k)
      k)))

(defn- symbol-of [fqn]
  (let [parts (named-parts fqn)]
    (symbol (aget parts 0) (aget parts 1))))

(declare r-value call)

(defn- module-fn
  "A function of the module's, as one of the host's: calling it calls the module."
  [handle]
  (let [f (fn self [& args] (call op-call-fn #js [self (vec args)]))]
    (gobj/set f "datascript$moduleFn" handle)
    f))

;; The collections, of their elements in an array

(defn- vector-of [^array arr]
  (if (<= (.-length arr) 32)
    (PersistentVector. nil (.-length arr) 5 (.-EMPTY-NODE PersistentVector) arr nil)
    (vec arr)))

(defn- list-of [^array arr]
  (loop [i (dec (.-length arr))
         l ()]
    (if (neg? i)
      l
      (recur (dec i) (conj l (aget arr i))))))

(defn- hash-map-of
  "Of keys and values in turn."
  [^array arr]
  (let [n (.-length arr)]
    (loop [i 0
           m (transient (.-EMPTY PersistentHashMap))]
      (if (< i n)
        (recur (+ i 2) (assoc! m (aget arr i) (aget arr (inc i))))
        (persistent! m)))))

(defn- array-set-of [^array arr]
  (let [n   (.-length arr)
        kvs (make-array (* 2 n))]
    (dotimes [i n]
      (aset kvs (* 2 i) (aget arr i))
      (aset kvs (inc (* 2 i)) nil))
    (PersistentHashSet. nil (PersistentArrayMap. nil n kvs nil) nil)))

(defn- hash-set-of [^array arr]
  (let [n (.-length arr)]
    (loop [i 0
           m (transient (.-EMPTY PersistentHashMap))]
      (if (< i n)
        (recur (inc i) (assoc! m (aget arr i) nil))
        (PersistentHashSet. nil (persistent! m) nil)))))

(defn- r-flat
  "A value of that tag that holds no other."
  [^Dec d tag]
  (case tag
    0 nil
    1 false
    2 true
    3 (r-zigzag d)
    4 (r-f64 d)
    5 (r-str d)
    6 (let [k (keyword-of (r-str d))]
        (.push (or (.-kws d) (set! (.-kws d) (array))) k)
        k)
    7 (aget (.-kws d) (r-varint d))
    8 (let [s (symbol-of (r-str d))]
        (.push (or (.-syms d) (set! (.-syms d) (array))) s)
        s)
    9 (aget (.-syms d) (r-varint d))
    16 (uuid (r-str d))
    17 (js/Date. (r-f64 d))
    18 (let [source (r-str d)
             flags  (r-str d)]
         (js/RegExp. source flags))
    20 (throw (js/Error. "datascript: a database's handle without its database"))
    21 (aget (.-table host-fns) (r-varint d))
    22 (let [handle (r-varint d)
             _hash  (r-zigzag d)]
         (aget (.-table host-objs) handle))
    23 (let [handle (r-varint d)
             _name  (r-str d)]
         (module-fn handle))
    24 (let [handle   (r-varint d)
             uid      (r-varint d)
             max-eid  (r-zigzag d)
             max-tx   (r-zigzag d)
             filtered (r-byte d)]
         (db-ctor handle uid max-eid max-tx (== filtered 1)))
    (throw (js/Error. (str "datascript: a value of no known kind (" tag ")")))))

(defn- ^boolean nested-tag?
  "Whether a value of that tag holds others: a collection, a datom."
  [tag]
  (or (and (>= tag 10) (<= tag 15)) (== tag 19)))

(defn- r-items
  "The elements of a collection: a count, then that many values."
  [^Dec d per-element]
  (let [n   (* per-element (r-varint d))
        arr (make-array n)]
    (loop [i 0]
      (when (< i n)
        (aset arr i (r-value d))
        (recur (inc i))))
    arr))

(defn- r-nested
  "A collection or a datom, each value in it read by a call."
  [^Dec d tag]
  (case tag
    10 (vector-of (r-items d 1))
    11 (list-of (r-items d 1))
    12 (let [arr (r-items d 2)]
         (PersistentArrayMap. nil (/ (.-length arr) 2) arr nil))
    13 (hash-map-of (r-items d 2))
    14 (array-set-of (r-items d 1))
    15 (hash-set-of (r-items d 1))
    19 (let [e     (r-zigzag d)
             a     (r-value d)
             v     (r-value d)
             tx    (r-zigzag d)
             added (r-byte d)]
         (datom-ctor e a v tx (== added 1)))))

;; A collection that is being read, where calls would nest too deep: #js [tag elements read expected]

(defn- r-open [^Dec d tag]
  (case tag
    (12 13) (let [n (* 2 (r-varint d))]
              #js [tag (make-array n) 0 n])
    ;; a datom: its entity, then its attribute and its value, which are values like any other
    19 #js [tag #js [(r-zigzag d) nil nil] 1 3]
    (let [n (r-varint d)]
      #js [tag (make-array n) 0 n])))

(defn- r-close [^Dec d ^array frame]
  (let [arr (aget frame 1)]
    (case (aget frame 0)
      10 (vector-of arr)
      11 (list-of arr)
      12 (PersistentArrayMap. nil (/ (.-length arr) 2) arr nil)
      13 (hash-map-of arr)
      14 (array-set-of arr)
      15 (hash-set-of arr)
      19 (let [tx    (r-zigzag d)
               added (r-byte d)]
           (datom-ctor (aget arr 0) (aget arr 1) (aget arr 2) tx (== added 1))))))

(defn- r-deep
  "A collection or a datom, read from a list of the collections it is in the middle of and not by a
  call for each: what a recursive pull answers nests thousands deep, and the stack does not."
  [^Dec d tag]
  (let [stack (array (r-open d tag))]
    (loop []
      (let [frame (aget stack (dec (.-length stack)))
            i     (aget frame 2)]
        (if (< i (aget frame 3))
          (let [tag (r-byte d)]
            (if (nested-tag? tag)
              (.push stack (r-open d tag))
              (do
                (aset (aget frame 1) i (r-flat d tag))
                (aset frame 2 (inc i))))
            (recur))
          (let [value (r-close d frame)]
            (.pop stack)
            (if (zero? (.-length stack))
              value
              (let [around (aget stack (dec (.-length stack)))]
                (aset (aget around 1) (aget around 2) value)
                (aset around 2 (inc (aget around 2)))
                (recur)))))))))

(def ^:const ^:private max-depth
  "How deep values are read by calls."
  128)

(defn- r-value [^Dec d]
  (let [tag (r-byte d)]
    (if (nested-tag? tag)
      (let [depth (.-depth d)]
        (if (< depth max-depth)
          (do
            (set! (.-depth d) (inc depth))
            (let [v (r-nested d tag)]
              (set! (.-depth d) depth)
              v))
          (r-deep d tag)))
      (r-flat d tag))))

(defn- read-at
  "The value the module wrote at ptr."
  [ptr len]
  (let [buf (js/Uint8Array. (.-buffer memory) ptr len)]
    (r-value (Dec. buf (js/DataView. (.-buffer memory) ptr len) 0 nil nil 0))))

;; ---------------------------------------------------------------- calling

(defn- write-args [^array args]
  (let [e (enc-open)]
    (try
      (w-array e t-vector args)
      (written e)
      (finally
        (enc-close)))))

(defn call
  "An operation of the module's, with its arguments in an array: its value, or what it throws."
  [op ^array args]
  (when (nil? ds-call)
    (throw (js/Error. "DataScript's WebAssembly module is not loaded: datascript.wasm/instantiate first")))
  (let [at     (write-args args)
        top    (.-value stack-pointer)
        status (try
                 (ds-call op (aget at 0) (aget at 1))
                 (catch :default e
                   ;; The call did not return: the stack ran out under it, or the module gave up. Its own
                   ;; stack is put back where it was, and what it was in the middle of is let go of.
                   (set! (.-value stack-pointer) top)
                   (ds-recover)
                   (throw e)))
        v      (read-at (unsigned-bit-shift-right (ds-result-ptr) 0) (ds-result-len))]
    (if (== status 0)
      v
      ;; [message data exception]: what a function of the host's threw is thrown on as it was
      (let [exception (nth v 2)]
        (throw
          (cond
            (some? exception) exception
            (some? (nth v 1)) (ex-info (nth v 0) (nth v 1))
            :else             (js/Error. (nth v 0))))))))

;; ---------------------------------------------------------------- being called

(def ^:private regexes (js/Map.))

(defn- regex-exec
  "JavaScript's own match of a regular expression: nil, or [index whole group ...]."
  [source flags input]
  (let [key (str flags "/" source)
        re  (or (.get regexes key)
              (let [re (js/RegExp. source flags)]
                (.set regexes key re)
                re))]
    (set! (.-lastIndex re) 0)
    (when-some [m (.exec re input)]
      (let [out (array (.-index m))]
        (dotimes [i (.-length m)]
          (.push out (aget m i)))
        out))))

(defn- reply! [^Enc e]
  (let [at (written e)]
    (ds-reply (aget at 0) (aget at 1))))

(defn- host-call
  "The module calls a function of the host's. What it throws goes back with the exception itself, for
  whoever called the module to be thrown it."
  [handle ptr len]
  (let [args (read-at (unsigned-bit-shift-right ptr 0) len)
        f    (if (== handle -1) regex-exec (aget (.-table host-fns) handle))]
    (try
      (let [result (apply f args)
            e      (enc-open)]
        (try
          (w-value e result)
          (reply! e)
          (finally
            (enc-close)))
        0)
      (catch :default ex
        (let [e (enc-open)]
          (try
            (w-byte e t-vector)
            (w-varint e 3)
            (w-value e (or (ex-message ex) (str ex)))
            (w-value e (ex-data ex))
            (w-host-obj e ex)
            (reply! e)
            (finally
              (enc-close))))
        1))))

(defn- reply-string! [s]
  (let [bytes (.encode text-encoder s)
        ptr   (unsigned-bit-shift-right (ds-alloc (.-length bytes)) 0)]
    (.set (js/Uint8Array. (.-buffer memory) ptr (.-length bytes)) bytes)
    (ds-reply ptr (.-length bytes))))

(defn- host-op
  "What the module asks of a value it does not look into."
  [op a b]
  (let [table (.-table host-objs)
        x     (aget table a)]
    (case op
      0 (if (= x (aget table b)) 1 0)
      1 (do (reply-string! (pr-str x)) 0)
      2 (do (reply-string! (type->str (type x))) 0)
      3 (try
          (let [c (compare x (aget table b))]
            (cond (neg? c) 0 (zero? c) 1 :else 2))
          (catch :default _ 3))
      4 (do (reply-string! (str x)) 0)
      0)))

(defn- host-release [kind handle]
  (release! (if (== kind 0) host-fns host-objs) handle))

(defn- host-log [level ptr len]
  (let [text (.decode text-decoder (js/Uint8Array. (.-buffer memory) (unsigned-bit-shift-right ptr 0) len))]
    (if (== level 0)
      (js/console.error text)
      (js/console.log text))))

(defn ^:export imports
  "What the module imports: the object to instantiate it with."
  []
  (let [fns #js {}]
    (gobj/set fns "ds_host_call" host-call)
    (gobj/set fns "ds_host_op" host-op)
    (gobj/set fns "ds_host_release" host-release)
    (gobj/set fns "ds_host_random" (fn [] (js/Math.random)))
    (gobj/set fns "ds_host_log" host-log)
    (let [out #js {}]
      (gobj/set out "datascript" fns)
      out)))

(defn ^:export attach!
  "Takes an instance of the module, made with (imports), as the one DataScript runs on."
  [instance]
  (let [exports (gobj/get instance "exports")
        version ((gobj/get exports "ds_abi_version"))]
    (when-not (== version abi-version)
      (throw (js/Error. (str "datascript.wasm speaks interface " version ", this ClojureScript interface " abi-version))))
    (set! memory (gobj/get exports "memory"))
    (set! ds-alloc (gobj/get exports "ds_alloc"))
    (set! ds-reply (gobj/get exports "ds_reply"))
    (set! ds-result-ptr (gobj/get exports "ds_result_ptr"))
    (set! ds-result-len (gobj/get exports "ds_result_len"))
    (set! ds-recover (gobj/get exports "ds_recover"))
    (set! stack-pointer (gobj/get exports "__stack_pointer"))
    (set! ds-call (gobj/get exports "ds_call"))
    ;; regular expressions are JavaScript's own
    (call op-set-option #js ["host-regex" true])
    instance))

(defn ^:export instantiate
  "Loads the module: from a Response (or a promise of one), bytes, or a compiled WebAssembly.Module.
  A promise of the instance, by when DataScript is ready."
  [source]
  (let [imports (imports)]
    (-> (js/Promise.resolve source)
      (.then
        (fn [source]
          (cond
            (instance? js/WebAssembly.Module source)
            (js/WebAssembly.instantiate source imports)

            (and (exists? js/Response) (instance? js/Response source))
            (-> (.arrayBuffer source)
              (.then #(js/WebAssembly.instantiate % imports))
              (.then #(gobj/get % "instance")))

            :else
            (-> (js/WebAssembly.instantiate source imports)
              (.then #(gobj/get % "instance"))))))
      (.then attach!))))

(defn ^:export instantiate-sync
  "Loads the module from its bytes, at once: where compiling may block."
  [bytes]
  (attach! (js/WebAssembly.Instance. (js/WebAssembly.Module. bytes) (imports))))

(defn held
  "How much the module holds for this host: {:dbs n :cursors n :fns n}, and what the host holds for it."
  []
  (let [[dbs cursors fns] (call op-held #js [])]
    {:dbs dbs :cursors cursors :fns fns
     :host-fns (.-size (.-by-value host-fns))
     :host-objs (.-size (.-by-value host-objs))}))
