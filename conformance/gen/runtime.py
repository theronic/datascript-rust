#!/usr/bin/env python3
"""Writes conformance/cases/runtime.edn: ClojureScript's runtime as the port keeps it. Hashes, printing, equality,
order, and the order of maps and sets around the size where ClojureScript turns an array map into a hash map."""
import itertools, random, sys

random.seed(7)
vals = [
 'nil','true','false','0','1','-1','42','3.14','-0.5','1e21','1.5e-7','0.1','100','536870913','2147483647','2147483648','4294967296','-2147483648','9007199254740991','9007199254740993','1e100','-1e-100','##Inf','##-Inf',
 '""','"a"','"abc"','"hello world"','"ünï"','"日本語"','"a\\nb"','"quote\\"d"','"\\\\"','"😀"','"A"','"b"','"aa"',
 ':a',':b',':a/b',':db/id',':db.type/ref',':person/name',':z',':A','a','b','foo/bar','?e','_','$','%','...','*','+',
 '[]','[1]','[1 2 3]','[:a "b" 3]','[[1 2] [3]]','[nil]','[nil 1]','[1 nil]','()','(1)','(1 2 3)','(:a (1 2))',
 '{}','{:a 1}','{:a 1 :b 2}','{:b 2 :a 1}','{"a" 1}','{1 2}','{:a {:b {:c 1}}}','{[1 2] :v}','{:a nil}',
 '#{}','#{1}','#{1 2 3}','#{:a :b}','#{"a" "b"}','#{[1] [2]}','#{#{1}}',
 '#uuid "550e8400-e29b-41d4-a716-446655440000"','#uuid "00000000-0000-0000-0000-000000000001"',
 '#inst "2020-01-01T00:00:00.000Z"','#inst "1969-12-31T23:59:59.999Z"','#inst "2026-10-02T12:34:56.789Z"',
 '\\a','\\newline','\\space',
 '{:a 1 :b 2 :c 3 :d 4 :e 5 :f 6 :g 7 :h 8}','{:a 1 :b 2 :c 3 :d 4 :e 5 :f 6 :g 7 :h 8 :i 9}',
 '{:a 1 :b 2 :c 3 :d 4 :e 5 :f 6 :g 7 :h 8 :i 9 :j 10 :k 11 :l 12}',
 '#{1 2 3 4 5 6 7 8}','#{1 2 3 4 5 6 7 8 9}','#{:a :b :c :d :e :f :g :h :i :j}',
 '[1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24 25 26 27 28 29 30 31 32 33 34 35]',
]
lines = []
def case(steps): lines.append('[' + ' '.join(steps) + ']')

# hash, pr-str and str of every value. (str of an instant is JavaScript's Date.toString, the machine's own.)
for chunk in [vals[i:i+12] for i in range(0, len(vals), 12)]:
    case(['{:op :cljs/hash :val %s}' % v for v in chunk])
    case(['{:op :cljs/pr-str :val %s}' % v for v in chunk])
    case(['{:op :cljs/str :val %s}' % v for v in chunk if not v.startswith('#inst')])

# equality and DataScript's order between every pair of a sample
sample = ['1','1.5','-1','"a"','"b"','"A"',':a',':b',':a/b','a','b','[1]','[1 2]','[2]','(1)','(1 2)','()','[]','{}','{:a 1}','{:a 2}','#{}','#{1}','true','false','#uuid "550e8400-e29b-41d4-a716-446655440000"','#uuid "00000000-0000-0000-0000-000000000001"','#inst "2020-01-01T00:00:00.000Z"','#inst "2021-01-01T00:00:00.000Z"','[nil 1]','[1 nil]','[nil nil]','"日本語"','"😀"','"￿"','{:a 1 :b 2 :c 3 :d 4 :e 5 :f 6 :g 7 :h 8 :i 9}','[:a]','[:b]','["a"]','[[1]]','[[2]]','0','-0.0','100','99.9','["a" 1]','[1 "a"]']
pairs = list(itertools.product(sample, sample))
for chunk in [pairs[i:i+30] for i in range(0, len(pairs), 30)]:
    case(['{:op :cljs/compare :a %s :b %s}' % (a, b) for a, b in chunk])
    case(['{:op :cljs/eq :a %s :b %s}' % (a, b) for a, b in chunk])

# sorting mixed values. A list equals the vector of its elements, yet sorts on the other side of a set from it, so
# no order holds between all three: lists are left out, as what a sort then does is its algorithm's.
sortable = [x for x in sample if not x.startswith('(')]
for n in range(60):
    xs = random.sample(sortable, random.randint(2, 14))
    case(['{:op :cljs/sort :vals [%s]}' % ' '.join(xs)])

# sets and maps of growing size: where the order turns from insertion to hash
def ints(n, lo=-50, hi=2000): return [str(random.randint(lo, hi)) for _ in range(n)]
def strs(n): return ['"%s"' % ''.join(random.choice('abcdefghij') for _ in range(random.randint(1, 6))) for _ in range(n)]
def kws(n): return [':%s%s' % (random.choice(['', 'ns/', 'a.b/']), ''.join(random.choice('abcdefgh') for _ in range(random.randint(1, 5)))) for _ in range(n)]
def vecs(n): return ['[%s]' % ' '.join(ints(random.randint(0, 3), 0, 20)) for _ in range(n)]
def floats(n): return [repr(round(random.uniform(-100, 100), random.randint(1, 6))) for _ in range(n)]
gens = [ints, strs, kws, vecs, floats]
for n in [0, 1, 2, 7, 8, 9, 10, 16, 17, 31, 32, 33, 40, 64, 100, 300, 1200]:
    for g in gens:
        xs = g(n)
        case(['{:op :cljs/set :vals [%s]}' % ' '.join(xs),
              '{:op :cljs/distinct :vals [%s]}' % ' '.join(xs),
              '{:op :cljs/frequencies :vals [%s]}' % ' '.join(xs),
              '{:op :cljs/into-map :pairs [%s]}' % ' '.join('[%s %d]' % (x, i) for i, x in enumerate(xs)),
              '{:op :cljs/zipmap :ks [%s] :vs [%s]}' % (' '.join(xs), ' '.join(str(i) for i in range(len(xs)))),
              '{:op :cljs/group-by-first :vals [%s]}' % ' '.join('[%s %d]' % (x, i) for i, x in enumerate(xs))])

# sets of values of many types
for n in [3, 8, 9, 12, 20, 50]:
    xs = []
    for g in gens: xs += g(n)
    random.shuffle(xs)
    xs = xs[:n * 2]
    case(['{:op :cljs/set :vals [%s]}' % ' '.join(xs), '{:op :cljs/into-map :pairs [%s]}' % ' '.join('[%s %d]' % (x, i) for i, x in enumerate(xs))])

# assoc, dissoc, conj, disj across the threshold
m8 = '{:a 1 :b 2 :c 3 :d 4 :e 5 :f 6 :g 7 :h 8}'
m9 = '{:a 1 :b 2 :c 3 :d 4 :e 5 :f 6 :g 7 :h 8 :i 9}'
case(['{:op :cljs/assoc :map %s :kvs [:i 9]}' % m8, '{:op :cljs/assoc :map %s :kvs [:a 100]}' % m8, '{:op :cljs/assoc :map %s :kvs [:i 9 :j 10 :a 0]}' % m8,
      '{:op :cljs/dissoc :map %s :ks [:a]}' % m9, '{:op :cljs/dissoc :map %s :ks [:a :b :c :d :e]}' % m9, '{:op :cljs/dissoc :map %s :ks [:d]}' % m8,
      '{:op :cljs/assoc :map {} :kvs [nil 1 :a 2]}', '{:op :cljs/assoc :map %s :kvs [nil 0]}' % m9, '{:op :cljs/assoc :map nil :kvs [:a 1]}',
      '{:op :cljs/assoc :map [1 2 3] :kvs [0 :x 3 :y]}', '{:op :cljs/merge :maps [%s {:z 26} {:a 0}]}' % m8, '{:op :cljs/merge :maps [nil {:a 1}]}', '{:op :cljs/merge :maps [{:a 1} nil {:b 2}]}',
      '{:op :cljs/select-keys :map %s :ks [:i :a :zz :c]}' % m9, '{:op :cljs/select-keys :map %s :ks #{:a :b :c :d :e :f :g :h :i :j}}' % m9])
s8 = '#{1 2 3 4 5 6 7 8}'
case(['{:op :cljs/conj :coll %s :xs [9]}' % s8, '{:op :cljs/conj :coll %s :xs [1]}' % s8, '{:op :cljs/conj :coll %s :xs [100 9 50]}' % s8,
      '{:op :cljs/disj :coll #{1 2 3 4 5 6 7 8 9 10} :xs [1 2 3 4 5]}', '{:op :cljs/disj :coll %s :xs [3]}' % s8, '{:op :cljs/conj :coll [1 2] :xs [3 4]}',
      '{:op :cljs/conj :coll (1 2) :xs [3 4]}', '{:op :cljs/conj :coll {:a 1} :xs [[:b 2] {:c 3 :d 4}]}', '{:op :cljs/conj :coll nil :xs [1 2]}',
      '{:op :cljs/conj :coll #{} :xs [nil 0 "" false]}', '{:op :cljs/conj :coll #{1 2 3 4 5 6 7 8 9} :xs [nil 0 "" false]}'])

# hash collisions: 0, "" and nil all hash to 0; and equality across kinds of collection
case(['{:op :cljs/set :vals [0 "" nil false 1 2 3 4 5 6]}', '{:op :cljs/set :vals ["" 0 6 5 4 3 2 1 false nil]}', '{:op :cljs/into-map :pairs [[0 :zero] ["" :empty] [nil :nil] [1 1] [2 2] [3 3] [4 4] [5 5] [6 6] [7 7]]}',
      '{:op :cljs/hash :val [0 "" nil]}', '{:op :cljs/set :vals [[1 2] (1 2) [1 2 3]]}', '{:op :cljs/eq :a {:a 1 :b 2} :b {:b 2 :a 1}}', '{:op :cljs/eq :a #{1 2} :b #{2 1}}', '{:op :cljs/eq :a [1 2] :b (1 2)}', '{:op :cljs/eq :a 1 :b 1.0}'])

# reading
reads = ['"[1 2 3]"','"{:a 1, :b [2 3]}"','"#{:x}"','"(1 2 #_ 3 4)"','"0x10"','"017"','"2r1010"','"1e3"','"1/4"','"-1.5e-3"','"+5"','"\\\\a"','"\\\\newline"','"\\"s\\\\tx\\""','":a/b"','"a.b/c"','"#inst \\"2020-02-03\\""','"#uuid \\"ABCDEF00-0000-0000-0000-000000000000\\""','"##NaN"','"nil"','"[true false nil]"','"; comment\\n 5"','"#:foo{:a 1 :bar/b 2}"','"{:a 1 :b 2 :c 3 :d 4 :e 5 :f 6 :g 7 :h 8 :i 9 :j 10}"','"#{1 2 3 4 5 6 7 8 9 10 11}"','"09"','"1a"','"{:a}"','"[1 2"','"{:a 1 :a 2}"','"#{1 1}"','"#foo 1"','":"','"a/"','"10N"','"1.5M"','"-0"','"36rZZ"','"\\\\u0041"','"\\\\o101"','"^:foo [1]"','"^{:a 1} {:b 2}"',
         '"{:a 1 :b 2 :a 3 :b 4}"','"{[1 2 3 4 5 6 7 8 9 10 11 12] 1 2}"','"{\\"a string that is long enough to cut\\" 1 2}"','"#{1 2 1 2}"','"#:foo{:a 1 :a 2}"','"#:foo/bar{:a 1}"','"#:foo [1]"','"##Foo"','"]"','"(1 2"','"\\"abc"','"#_"','"#inst \\"nope\\""','"#uuid 5"','"a@b"','"~a"','"^5 [1]"','"^:foo 5"','": a"','"1 2"','""','"   "','"#"','"\\\\"','"\\\\u00zz"','"\\"\\\\q\\""','"#:foo{:a 1 :b 2 :c 3 :d 4 :e 5 :f 6 :g 7 :h 8 :i 9 :j 10}"','"#datascript/Datom [1 :a 2 536870913 true]"','"#datascript/DB {:schema {:a {:db/index true}} :datoms [[1 :a 2 536870913] [2 :a 1 536870914]]}"']
for chunk in [reads[i:i+14] for i in range(0, len(reads), 14)]:
    case(['{:op :cljs/read :string %s}' % r for r in chunk])

out = sys.argv[1] if len(sys.argv) > 1 else 'conformance/cases/runtime.edn'
open(out, 'w').write("; ClojureScript's runtime as the port keeps it: hashes, printing, equality, order, and the order of maps and sets.\n; Written by conformance/gen/runtime.py.\n" + '\n'.join(lines) + '\n')
print(len(lines), 'cases')
