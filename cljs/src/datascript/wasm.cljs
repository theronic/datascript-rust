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
(def ^:const t-kw 6)
(def ^:const t-sym 8)
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
(def ^:const t-type 25)
(def ^:const t-datoms 26)

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
(def ^:const op-count-run 15)
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
(def ^:const op-compare 37)
(def ^:const op-equiv 38)
(def ^:const op-str 39)

;; ---------------------------------------------------------------- what datascript.db fills in

(def ^{:doc "The Datom type, and (fn [e a v tx added]) that makes one."} datom-type nil)
(def datom-ctor nil)
(def ^{:doc "(fn [reader]) → the run of datoms the reader is at, as an array"} datoms-reader nil)
(def ^{:doc "(fn []), called when an instance of the module is taken: what was kept of another is forgotten"} on-attach nil)
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
(def ^:private ds-intern nil)
(def ^:private ds-name-ptr nil)
(def ^:private ds-name-len nil)
;; the module's own stack, in its memory: the pointer to its top
(def ^:private stack-pointer nil)

(defn ^boolean ready?
  "Whether the module is loaded."
  []
  (some? ds-call))

(def ^:private text-encoder (js/TextEncoder.))
(def ^:private text-decoder (js/TextDecoder. "utf-8"))

;; The module's memory, as bytes and as a view to read numbers from. Memory that grows leaves these of no length,
;; and they are made again.
(def ^:private mem-u8 (js/Uint8Array. 0))
(def ^:private mem-dv nil)

(defn- mem! []
  (when (zero? (.-length mem-u8))
    (let [buffer (.-buffer memory)]
      (set! mem-u8 (js/Uint8Array. buffer))
      (set! mem-dv (js/DataView. buffer)))))

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

;; ---------------------------------------------------------------- types

;; What `type` answers is a constructor here and a name in the module: the two, by each other.
(def ^:private type-by-name (js/Map.))
(def ^:private name-by-type (js/Map.))

(defn register-type!
  "Makes a type known to the module, by a name: the one given, which is the module's own for a type it
  has values of, or the one type->str gives, which is what DataScript orders values of different
  types by."
  ([ctor]
   (or (.get name-by-type ctor)
     (let [name (type->str ctor)]
       ;; optimized builds leave two types of one shape with one name
       (register-type! ctor (if (.has type-by-name name) (str name "#" (.-size type-by-name)) name)))))
  ([ctor name]
   (.set type-by-name name ctor)
   (.set name-by-type ctor name)
   name))

;; JavaScript's own, as the module names them, and ClojureScript's by their names
(doseq [[name ctor] [["Number" js/Number] ["String" js/String] ["Boolean" js/Boolean] ["Date" js/Date]
                     ["RegExp" (type #"")] ["Function" js/Function] ["Object" js/Object]
                     ["Array" js/Array] ["Error" js/Error]]]
  (register-type! ctor (str "function " name "() { [native code] }")))

(doseq [[name ctor] [["Keyword" Keyword] ["Symbol" Symbol] ["PersistentVector" PersistentVector]
                     ["List" List] ["EmptyList" EmptyList] ["PersistentArrayMap" PersistentArrayMap]
                     ["PersistentHashMap" PersistentHashMap] ["PersistentHashSet" PersistentHashSet]
                     ["UUID" UUID] ["LazySeq" LazySeq] ["Cons" Cons] ["Subvec" Subvec]
                     ["MapEntry" MapEntry] ["PersistentTreeMap" PersistentTreeMap]
                     ["PersistentTreeSet" PersistentTreeSet] ["PersistentQueue" PersistentQueue]
                     ["Range" Range] ["IntegerRange" IntegerRange] ["Atom" Atom]
                     ["ExceptionInfo" ExceptionInfo]]]
  (register-type! ctor (str "cljs.core/" name)))

;; ---------------------------------------------------------------- writing

(deftype Enc [^:mutable buf ^:mutable dv ^:mutable pos ^:mutable depth])

(def ^:private enc-pool (array))
(def ^:private enc-depth 0)

(defn- enc-open
  "A writer. A call made while another is being written — a lazy sequence that queries as it is
  read — gets its own."
  []
  (let [depth enc-depth
        e     (or (aget enc-pool depth)
                (let [buf (js/Uint8Array. 4096)
                      e   (Enc. buf (js/DataView. (.-buffer buf)) 0 0)]
                  (aset enc-pool depth e)
                  e))]
    (set! enc-depth (inc depth))
    (set! (.-pos e) 0)
    (set! (.-depth e) 0)
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

;; Keywords and symbols are numbers to the module, which numbers every name it meets, for good. Here, the numbers of
;; the names that have crossed, and the keywords and symbols of the numbers: each is asked of the module once.
(def ^:private kw-ids (js/Map.))
(def ^:private sym-ids (js/Map.))
(def ^:private kws (array))
(def ^:private syms (array))

(defn- number-of
  "The module's number for a name of a kind: 0 a keyword, 1 a symbol."
  [kind fqn ^js ids]
  (let [bytes (.encode text-encoder fqn)
        len   (.-length bytes)
        ptr   (unsigned-bit-shift-right (ds-alloc len) 0)]
    (mem!)
    (.set mem-u8 bytes ptr)
    (let [id (ds-intern kind ptr len)]
      (.set ids fqn id)
      id)))

(defn- name-of
  "The name the module has for a number of a kind."
  [kind id]
  (let [ptr (unsigned-bit-shift-right (ds-name-ptr kind id) 0)
        len (ds-name-len kind id)]
    (when (zero? ptr)
      (throw (js/Error. (str "datascript: no name of number " id))))
    (mem!)
    (.decode text-decoder (.subarray mem-u8 ptr (+ ptr len)))))

(defn- w-keyword [^Enc e fqn]
  (let [id (.get kw-ids fqn)]
    (w-byte e t-kw)
    (w-varint e (if (some? id) id (number-of 0 fqn kw-ids)))))

(defn- w-symbol [^Enc e fqn]
  (let [id (.get sym-ids fqn)]
    (w-byte e t-sym)
    (w-varint e (if (some? id) id (number-of 1 fqn sym-ids)))))

(declare w-value)

(defn- w-host-obj [^Enc e x]
  (w-byte e t-host-obj)
  (w-varint e (mention! host-objs x))
  (w-zigzag e (try (hash x) (catch :default _ 0))))

(defn- whole-int32? [n]
  (and (number? n) (== (bit-or n 0) n)))

(defn- ^boolean datom-ids?
  "Whether a datom's ids are ones the module has datoms for."
  [d]
  (and (whole-int32? (.-e d)) (whole-int32? (.-tx d))))

(defn- w-datom-head [^Enc e d]
  (w-byte e t-datom)
  (w-zigzag e (.-e d))
  (w-value e (.-a d)))

(defn- w-datom-end [^Enc e d]
  (let [tx (.-tx d)]
    (w-zigzag e (if (neg? tx) (- tx) tx))
    (w-byte e (if (pos? tx) 1 0))))

(defn- datom-as-vector
  "A datom with ids the module has no datom for, as the transaction form of it, which says what is
  wrong with them."
  [d]
  (let [tx (.-tx d)]
    #js [(if (pos? tx) :db/add :db/retract) (.-e d) (.-a d) (.-v d) (if (neg? tx) (- tx) tx)]))

(defn- ^boolean w-flat
  "Writes a value that holds no other, whole, and answers true. Of a collection, or a datom whose value
  is one, writes nothing and answers false."
  [^Enc e x]
  (cond
    (nil? x)     (do (w-byte e t-nil) true)
    (number? x)  (do (w-number e x) true)
    (string? x)  (do (w-byte e t-str) (w-str e x) true)
    (keyword? x) (do (w-keyword e (.-fqn x)) true)
    (true? x)    (do (w-byte e t-true) true)
    (false? x)   (do (w-byte e t-false) true)

    (instance? PersistentVector x)   false
    (instance? PersistentArrayMap x) false

    (instance? datom-type x)
    (let [v (.-v x)]
      (if (or (not (datom-ids? x)) (coll? v) (array? v))
        false
        (do
          (w-datom-head e x)
          (w-value e v)
          (w-datom-end e x)
          true)))

    (symbol? x)
    (do (w-symbol e (.-str x)) true)

    (some? (db-handle x))
    (do (w-byte e t-db) (w-varint e (db-handle x)) true)

    (instance? PersistentHashMap x) false
    (instance? PersistentHashSet x) false

    ;; a record stays the value it is: the module asks the host what it needs to know of it
    (record? x)
    (do (w-host-obj e x) true)

    (or (map? x) (set? x) (vector? x) (sequential? x) (array? x))
    false

    (uuid? x)
    (do (w-byte e t-uuid) (w-str e (.-uuid x)) true)

    (inst? x)
    (do (w-byte e t-inst) (w-f64 e (.getTime x)) true)

    (regexp? x)
    (do (w-byte e t-regex) (w-str e (.-source x)) (w-str e (.-flags x)) true)

    (some? (gobj/get x "datascript$moduleFn"))
    (do (w-byte e t-module-fn) (w-varint e (gobj/get x "datascript$moduleFn")) (w-str e "") true)

    (fn? x)
    (do
      (if-some [name (.get name-by-type x)]
        (do (w-byte e t-type) (w-str e name))
        (do (w-byte e t-host-fn) (w-varint e (mention! host-fns x))))
      true)

    :else
    (do (w-host-obj e x) true)))

(defn- w-array [^Enc e tag ^array arr]
  (w-byte e tag)
  (w-varint e (.-length arr))
  (dotimes [i (.-length arr)]
    (w-value e (aget arr i))))

(defn- w-nested
  "A collection, or a datom whose value is one: each value in it written by a call."
  [^Enc e x]
  (cond
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
    (if (datom-ids? x)
      (do
        (w-datom-head e x)
        (w-value e (.-v x))
        (w-datom-end e x))
      (w-array e t-vector (datom-as-vector x)))

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

    ;; a sorted map is the map of its entries in their order
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

    :else
    (w-array e t-vector x)))

(defn- begin
  "Writes what a collection begins with, and answers its elements in the order they follow."
  [^Enc e x]
  (let [elements
        (cond
          (instance? PersistentVector x)   (do (w-byte e t-vector) (to-array x))
          (instance? PersistentArrayMap x) (do (w-byte e t-array-map) (.-arr x))
          (instance? datom-type x)         (do (w-byte e t-vector) (datom-as-vector x))
          (instance? PersistentHashMap x)  (let [kvs (array)]
                                             (w-byte e t-hash-map)
                                             (reduce-kv (fn [_ k v] (.push kvs k v) nil) nil x)
                                             kvs)
          (instance? PersistentHashSet x)  (do
                                             (w-byte e (if (instance? PersistentArrayMap (.-hash-map x)) t-array-set t-hash-set))
                                             (to-array x))
          (map? x)                         (let [kvs (array)]
                                             (w-byte e t-array-map)
                                             (doseq [entry (seq x)]
                                               (.push kvs (key entry) (val entry)))
                                             kvs)
          (set? x)                         (do (w-byte e t-array-set) (to-array x))
          (vector? x)                      (do (w-byte e t-vector) (to-array x))
          (sequential? x)                  (do (w-byte e t-list) (to-array x))
          :else                            (do (w-byte e t-vector) x))]
    (w-varint e (if (map? x) (/ (.-length elements) 2) (.-length elements)))
    elements))

(defn- w-deep
  "A collection, or a datom whose value is one, written from a list of the collections it is in the
  middle of and not by a call for each: for values nested deeper than the stack has room for."
  [^Enc e x]
  ;; the elements still to write, how many of them are written, and the datom to end when they are
  (let [todo  (array)
        at    (array)
        ends  (array)
        open! (fn [x]
                (when-not (w-flat e x)
                  (if (and (instance? datom-type x) (datom-ids? x))
                    (do
                      (w-datom-head e x)
                      (.push todo #js [(.-v x)])
                      (.push ends x))
                    (do
                      (.push todo (begin e x))
                      (.push ends nil)))
                  (.push at 0)))]
    (open! x)
    (loop []
      (let [top (dec (.-length todo))]
        (when-not (neg? top)
          (let [elements (aget todo top)
                i        (aget at top)]
            (if (< i (.-length elements))
              (do
                (aset at top (inc i))
                (open! (aget elements i)))
              (do
                (when-some [d (aget ends top)]
                  (w-datom-end e d))
                (.pop todo)
                (.pop at)
                (.pop ends))))
          (recur))))))

(def ^:const ^:private max-depth
  "How deep values are written and read by calls."
  128)

(defn- w-value [^Enc e x]
  (when-not (w-flat e x)
    (let [depth (.-depth e)]
      (if (< depth max-depth)
        (do
          (set! (.-depth e) (inc depth))
          (w-nested e x)
          (set! (.-depth e) depth))
        (w-deep e x)))))

(defn- written
  "What a writer holds, copied into the module's memory: where it is there. It is (.-pos e) bytes long."
  [^Enc e]
  (let [len (.-pos e)
        ptr (unsigned-bit-shift-right (ds-alloc len) 0)
        buf (.-buf e)]
    (mem!)
    (if (< len 96)
      (let [mem mem-u8]
        (loop [i 0]
          (when (< i len)
            (aset mem (+ ptr i) (aget buf i))
            (recur (inc i)))))
      (.set mem-u8 (.subarray buf 0 len) ptr))
    ptr))

;; ---------------------------------------------------------------- reading

(deftype Dec [^:mutable buf ^:mutable dv ^:mutable pos ^:mutable depth ^:mutable busy])

(defn r-byte [^Dec d]
  (let [pos (.-pos d)]
    (set! (.-pos d) (inc pos))
    (aget (.-buf d) pos)))

(defn r-varint [^Dec d]
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

(defn r-zigzag [^Dec d]
  (let [n (r-varint d)]
    (bit-xor (unsigned-bit-shift-right n 1) (- (bit-and n 1)))))

(defn- r-f64 [^Dec d]
  (let [pos (.-pos d)]
    (set! (.-pos d) (+ pos 8))
    (.getFloat64 (.-dv d) pos true)))

(defn r-u32 [^Dec d]
  (let [pos (.-pos d)]
    (set! (.-pos d) (+ pos 4))
    (.getUint32 (.-dv d) pos true)))

;; the characters of a short string, to make it of at once
(def ^:private str-chars (array))

(defn- r-str [^Dec d]
  (let [len   (r-varint d)
        buf   (.-buf d)
        start (.-pos d)
        end   (+ start len)]
    (set! (.-pos d) end)
    (cond
      (zero? len) ""

      ;; short, and most likely ASCII: its characters are its bytes
      (< len 64)
      (let [chars str-chars]
        (set! (.-length chars) len)
        (loop [i 0]
          (if (< i len)
            (let [c (aget buf (+ start i))]
              (if (< c 128)
                (do (aset chars i c) (recur (inc i)))
                (.decode text-decoder (.subarray buf start end))))
            (.apply js/String.fromCharCode nil chars))))

      :else
      (.decode text-decoder (.subarray buf start end)))))

(defn- named-parts
  "ns/name as the module splits it: at the first slash, unless the name is the slash."
  [fqn]
  (let [i (.indexOf fqn "/")]
    (if (or (neg? i) (== (.-length fqn) 1))
      #js [nil fqn]
      #js [(.substring fqn 0 i) (.substring fqn (inc i))])))

(defn- keyword-of
  "The keyword of a number: made once, the first time the module sends the number."
  [id]
  (let [fqn   (name-of 0 id)
        parts (named-parts fqn)
        k     (Keyword. (aget parts 0) (aget parts 1) fqn nil)]
    (aset kws id k)
    (.set kw-ids fqn id)
    k))

(defn- symbol-of [id]
  (let [fqn   (name-of 1 id)
        parts (named-parts fqn)
        s     (symbol (aget parts 0) (aget parts 1))]
    (aset syms id s)
    (.set sym-ids fqn id)
    s))

(declare r-value r-keyword call)

(def ^:private module-fns (js/Map.))

(defn- module-fn
  "A function of the module's, as one of the host's: calling it calls the module. The module's
  functions are the ones it is built with, and each is one function here."
  [handle]
  (or (.get module-fns handle)
    (let [f (fn self [& args] (call op-call-fn #js [self (vec args)]))]
      (gobj/set f "datascript$moduleFn" handle)
      (.set module-fns handle f)
      f)))

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
    6 (r-keyword d)
    8 (let [id (r-varint d)
            s  (aget syms id)]
        (if (some? s) s (symbol-of id)))
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
    25 (let [name (r-str d)]
         (or (.get type-by-name name)
           (throw (js/Error. (str "datascript: a type the host has no constructor for: " name)))))
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

(defn r-tagged
  "The value of a tag that was read."
  [^Dec d tag]
  (cond
    (== tag t-datoms)
    (datoms-reader d)

    (nested-tag? tag)
    (let [depth (.-depth d)]
      (if (< depth max-depth)
        (do
          (set! (.-depth d) (inc depth))
          (let [v (r-nested d tag)]
            (set! (.-depth d) depth)
            v))
        (r-deep d tag)))

    :else
    (r-flat d tag)))

(defn r-value [^Dec d]
  (r-tagged d (r-byte d)))

(defn r-keyword
  "The keyword of the number the reader is at."
  [^Dec d]
  (let [id (r-varint d)
        k  (aget kws id)]
    (if (some? k) k (keyword-of id))))

;; The reader of what the module writes. One is enough: a value is read whole before anything else is asked of the
;; module. Were one ever read in the middle of another, it would get a reader of its own.
(def ^:private the-dec (Dec. nil nil 0 0 false))

(defn- read-at
  "The value the module wrote at ptr."
  [ptr]
  (mem!)
  (let [^Dec d the-dec]
    (if (.-busy d)
      (r-value (Dec. mem-u8 mem-dv ptr 0 true))
      (do
        (set! (.-busy d) true)
        (set! (.-buf d) mem-u8)
        (set! (.-dv d) mem-dv)
        (set! (.-pos d) ptr)
        (set! (.-depth d) 0)
        (try
          (r-value d)
          (finally
            (set! (.-busy d) false)))))))

;; ---------------------------------------------------------------- calling

;; The module lets go of a function or a value of the host's as soon as it has no use for it, which may
;; be before the host has read the answer that names it: what is let go of during a call waits until
;; the call has been read.
(def ^:private calls 0)
(def ^:private let-go (array))

(defn- host-release [kind handle]
  (if (pos? calls)
    (.push let-go kind handle)
    (release! (if (== kind 0) host-fns host-objs) handle)))

(defn- call-ended! []
  (set! calls (dec calls))
  (when (and (zero? calls) (pos? (.-length let-go)))
    (let [pending let-go]
      (set! let-go (array))
      (loop [i 0]
        (when (< i (.-length pending))
          (release! (if (== (aget pending i) 0) host-fns host-objs) (aget pending (inc i)))
          (recur (+ i 2)))))))

(defn- ds-call-with
  "Writes an operation's arguments and calls it: the status it answers."
  [op ^array args]
  (let [e (enc-open)]
    (try
      (w-array e t-vector args)
      (catch :default ex
        (enc-close)
        (throw ex)))
    (let [len (.-pos e)
          ptr (written e)]
      (enc-close)
      (ds-call op ptr len))))

(defn call
  "An operation of the module's, with its arguments in an array: its value, or what it throws."
  [op ^array args]
  (when (nil? ds-call)
    (throw (js/Error. "DataScript's WebAssembly module is not loaded: datascript.wasm/instantiate first")))
  (set! calls (inc calls))
  (try
    (let [top    (.-value stack-pointer)
          status (try
                   (ds-call-with op args)
                   (catch :default e
                     ;; The call did not return: the stack ran out under it, or the module gave up. Its
                     ;; own stack is put back where it was, and what it was in the middle of is let go of.
                     (set! (.-value stack-pointer) top)
                     (ds-recover)
                     (throw e)))
          v      (read-at (unsigned-bit-shift-right (ds-result-ptr) 0))]
      (if (== status 0)
        v
        ;; [message data exception]: what a function of the host's threw is thrown on as it was
        (let [exception (nth v 2)]
          (throw
            (cond
              (some? exception) exception
              (some? (nth v 1)) (ex-info (nth v 0) (nth v 1))
              :else             (js/Error. (nth v 0)))))))
    (finally
      (call-ended!))))

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
  (let [len (.-pos e)]
    (ds-reply (written e) len)))

(defn- host-call
  "The module calls a function of the host's. What it throws goes back with the exception itself, for
  whoever called the module to be thrown it."
  [handle ptr len]
  (let [args (read-at (unsigned-bit-shift-right ptr 0))
        f    (if (neg? handle)
               (case handle
                 -1 regex-exec
                 ;; what the module asks of a value it does not look into
                 -2 (fn [x k not-found] (get x k not-found))
                 -3 count
                 -4 seq
                 -5 contains?
                 -6 apply
                 -7 (fn [x] (when (map? x) (into {} x))))
               (aget (.-table host-fns) handle))]
    (try
      (let [result (apply f args)]
        ;; what a predicate answers needs no reply written: the status says it
        (cond
          (nil? result)   2
          (false? result) 3
          (true? result)  4
          :else
          (let [e (enc-open)]
            (try
              (w-value e result)
              (reply! e)
              (finally
                (enc-close)))
            0)))
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
    (mem!)
    (.set mem-u8 bytes ptr)
    (ds-reply ptr (.-length bytes))))

(defn- host-op
  "What the module asks of a value it does not look into."
  [op a b]
  (let [table (.-table host-objs)
        x     (aget table a)]
    (case op
      0 (if (= x (aget table b)) 1 0)
      1 (do (reply-string! (pr-str x)) 0)
      ;; its type is known by that name from here on
      2 (do (reply-string! (if (nil? x) "nil" (register-type! (type x)))) 0)
      3 (try
          (let [c (compare x (aget table b))]
            (cond (neg? c) 0 (zero? c) 1 :else 2))
          (catch :default _ 3))
      4 (do (reply-string! (str x)) 0)
      5 (if (and (not (string? x)) (or (seqable? x) (array? x))) 1 0)
      0)))

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
    (set! ds-intern (gobj/get exports "ds_intern"))
    (set! ds-name-ptr (gobj/get exports "ds_name_ptr"))
    (set! ds-name-len (gobj/get exports "ds_name_len"))
    ;; the numbers are this instance's
    (.clear kw-ids)
    (.clear sym-ids)
    (set! kws (array))
    (set! syms (array))
    (set! mem-u8 (js/Uint8Array. 0))
    (when (some? on-attach)
      (on-attach))
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
  "How much the module holds for this host: {:dbs n :fns n}, and what the host holds for it."
  []
  (let [[dbs fns] (call op-held #js [])]
    {:dbs dbs :fns fns
     :host-fns (.-size (.-by-value host-fns))
     :host-objs (.-size (.-by-value host-objs))}))
