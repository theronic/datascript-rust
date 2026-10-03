#!/usr/bin/env python3
"""Writes conformance/cases/query.edn: Datalog queries, after DataScript's own tests (test/datascript/test/query*.cljc,
lookup_refs.cljc, tuples.cljc, issues.cljc) and around them: every clause kind, find spec, input binding, built-in
function and aggregate, the errors, and results large enough that their order is the order of ClojureScript's hash
sets rather than of small ones."""
import random
import sys

lines = []
def case(steps): lines.append('[' + ' '.join(steps) + ']')

def q(query, *inputs): return '{:op :q :query %s :inputs [%s]}' % (query, ' '.join(inputs))
def qc(query, *inputs): return '{:op :q-count :db #r db :query %s :inputs [%s]}' % (query, ' '.join(inputs))

def setup(schema, tx, name='db'):
    return ['{:op :empty-db :schema %s :as %s0}' % (schema, name), '{:op :db-with :db #r %s0 :tx %s :as %s}' % (name, tx, name)]

def with_db(schema, tx, queries):
    """Queries over one database: each a query with $ the database, or a (query, inputs...) tuple."""
    steps = setup(schema, tx)
    for item in queries:
        if isinstance(item, str):
            steps.append(q(item, '#r db'))
        else:
            steps.append(q(item[0], *item[1:]))
    case(steps)

def plain(queries):
    """Queries without a database: (query, inputs...) tuples."""
    steps = []
    for item in queries:
        if isinstance(item, str):
            steps.append(q(item))
        else:
            steps.append(q(item[0], *item[1:]))
    case(steps)

DB = '#r db'

# ---------------------------------------------------------------- query.cljc
JOINS = '[{:db/id 1, :name "Ivan", :age 15} {:db/id 2, :name "Petr", :age 37} {:db/id 3, :name "Ivan", :age 37} {:db/id 4, :age 15}]'
with_db('nil', JOINS, [
    '[:find ?e :where [?e :name]]',
    '[:find ?e ?v :where [?e :name "Ivan"] [?e :age ?v]]',
    '[:find ?e1 ?e2 :where [?e1 :name ?n] [?e2 :name ?n]]',
    '[:find ?e ?e2 ?n :where [?e :name "Ivan"] [?e :age ?a] [?e2 :age ?a] [?e2 :name ?n]]',
    '[:find ?e ?a ?v :where [?e ?a ?v]]',
    '[:find ?e ?a ?v ?tx :where [?e ?a ?v ?tx]]',
    '[:find ?a :where [_ ?a]]',
    '[:find ?v :where [_ _ ?v]]',
    '[:find ?tx :where [_ _ _ ?tx]]',
    '[:find ?e :where [?e :age 15]]',
    '[:find ?e :where [?e _ 15]]',
    '[:find ?e :where [?e _ _ 536870913]]',
    '[:find ?e :where [?e :name _ 536870913]]',
    '[:find ?e :where [?e :name "Ivan" 536870913]]',
    '[:find ?e :where [?e :name "Ivan" 536870914]]',
    '[:find ?a ?v :where [1 ?a ?v]]',
    '[:find ?v :where [1 :name ?v]]',
    '[:find ?e :where [?e :nope]]',
    '[:find ?e ?e :where [?e :name]]',
    '[:find ?x :where [?x ?x ?x]]',
    '[:find ?x ?y :where [?x :age ?y] [?y :age ?x]]',
    '[:find ?e ?v :where [?e :age ?v] [?e :age ?v]]',
    '[:find ?a ?b :where [?a :name "Ivan"] [?b :name "Petr"]]',
    '[:find ?a ?b ?c :where [?a :name "Ivan"] [?b :name "Petr"] [?c :age 15]]',
    '[:find ?n ?a :where [?e :age ?a] [?e :name ?n]]',
    '[:find ?n ?a :where [?e :name ?n] [?e :age ?a]]',
    '[:find ?e ?n ?a :where [?e :name ?n] [?e2 :age ?a]]',
    '{:find [?e] :where [[?e :name]]}',
    '{:find [?e ?v] :where [[?e :age ?v]]}',
    '{:find (?e) :where ([?e :name])}',
    '[:find ?v ?e :where [?e :age ?v]]',
])

with_db('{:aka {:db/cardinality :db.cardinality/many}}',
        '[[:db/add 1 :name "Ivan"] [:db/add 1 :aka "ivolga"] [:db/add 1 :aka "pi"] [:db/add 2 :name "Petr"] [:db/add 2 :aka "porosenok"] [:db/add 2 :aka "pi"]]', [
    '[:find ?n1 ?n2 :where [?e1 :aka ?x] [?e2 :aka ?x] [?e1 :name ?n1] [?e2 :name ?n2]]',
    '[:find ?e ?x :where [?e :aka ?x]]',
    '[:find ?x :where [_ :aka ?x]]',
])

COLL = '[[1 :name "Ivan"] [1 :age 19] [1 :aka "dragon_killer_94"] [1 :aka "-=autobot=-"]]'
plain([
    ('[:find ?n ?a :where [?e :aka "dragon_killer_94"] [?e :name ?n] [?e :age ?a]]', COLL),
    ('[:find ?e ?v :where [?e :name ?v]]', '[[1 :name "Ivan" 945 :db/add] [1 :age 39 999 :db/retract]]'),
    ('[:find ?e ?a ?v ?t :where [?e ?a ?v ?t :db/retract]]', '[[1 :name "Ivan" 945 :db/add] [1 :age 39 999 :db/retract]]'),
    ('[:find ?e ?a ?v ?t ?op :where [?e ?a ?v ?t ?op]]', '[[1 :name "Ivan" 945 :db/add] [1 :age 39 999 :db/retract]]'),
    ('[:find ?a :where [?a]]', '[[1] [2] [3] [] nil [4 5]]'),
    ('[:find ?a ?b :where [?a ?b]]', '[[1] [2 3] [] nil [4 5 6]]'),
    ('[:find ?a :where [1 ?a]]', '[[1 2] [] nil [1] [3 4]]'),
    ('[:find ?a ?b :where [?a ?b]]', '#{[1 2]}'),
    ('[:find ?a ?b :where [?a ?b]]', '{:x 1 :y 2}'),
    ('[:find ?a ?b :where [?a ?b]]', '([1 2] (3 4) #{5})'),
    ('[:find ?a ?b :where [?a ?b]]', 'nil'),
    ('[:find ?a ?b :where [?a ?b]]', '[]'),
    ('[:find ?a :where [?a _ _]]', '[[1 2 3] [4 5]]'),
    ('[:find ?a ?b :where [?a :x ?b]]', '[[1 :x 2] [3 :y 4] ["s" :x :k] [nil :x nil]]'),
    ('[:find ?e ?a ?v :where [?e ?a ?v]]', '[#datascript/Datom [1 :name "Ivan" 536870913 true] #datascript/Datom [2 :age 3 536870914 true]]'),
    ('[:find ?e ?added :where [?e _ _ _ ?added]]', '[#datascript/Datom [1 :name "Ivan" 536870913 true] #datascript/Datom [2 :age 3 536870914 false]]'),
    ('[:find ?a :where [?a]]', '5'),
    ('[:find ?a :where [?a]]', '[5]'),
    ('[:find ?a :where [?a]]', '"abc"'),
    ('[:find ?a :where [?a]]', '["abc" "d"]'),
])

IN3 = '[{:db/id 1, :name "Ivan", :age 15} {:db/id 2, :name "Petr", :age 37} {:db/id 3, :name "Ivan", :age 37}]'
with_db('nil', IN3, [
    ('{:find [?e] :in [$ ?attr ?value] :where [[?e ?attr ?value]]}', DB, ':name', '"Ivan"'),
    ('{:find [?e] :in [$ ?attr ?value] :where [[?e ?attr ?value]]}', DB, ':age', '37'),
    ('[:find ?a ?v :in $db ?e :where [$db ?e ?a ?v]]', DB, '1'),
    ('[:find ?e ?email :in $ $b :where [?e :name ?n] [$b ?n ?email]]', DB, '[["Ivan" "ivan@mail.ru"] ["Petr" "petr@gmail.com"]]'),
    ('[:find ?a ?b :in ?a ?b]', '10', '20'),
    ('[:find ?e :where [(inc 1) ?e]]', DB),
    ('[:find ?e :in $ $2 :where [?e]]', DB),
    ('[:find ?e :where [?e]]', DB, DB),
    ('[:find ?e :in $ $2 :where [?e]]', DB, DB, DB),
    ('[:find ?e :where [?e]]',),
    ('[:find ?e :in :where [?e]]',),
    ('[:find ?e :in $ ?x ?y ?z :where [?e]]', DB, '1'),
    # bindings
    ('[:find ?e ?email :in $ [[?n ?email]] :where [?e :name ?n]]', DB, '[["Ivan" "ivan@mail.ru"] ["Petr" "petr@gmail.com"]]'),
    ('[:find ?e :in $ [?name ?age] :where [?e :name ?name] [?e :age ?age]]', DB, '["Ivan" 37]'),
    ('[:find ?attr ?value :in $ ?e [?attr ...] :where [?e ?attr ?value]]', DB, '1', '[:name :age]'),
    ('[:find ?e ?n :in $ ?n :where [?e :name ?n]]', DB, '"Ivan"'),
    ('[:find ?e ?n :in $ ?n :where [?e :name ?n]]', DB, 'nil'),
    ('[:find ?e ?n :in $ ?n :where [?e :name ?n]]', DB, 'false'),
    ('[:find ?e ?n :in $ [?n ...] :where [?e :name ?n]]', DB, '["Ivan" "Petr" "Nobody"]'),
    ('[:find ?e ?n :in $ [?n ...] :where [?e :name ?n]]', DB, '#{"Ivan" "Petr"}'),
    ('[:find ?e ?n :in $ [?n ...] :where [?e :name ?n]]', DB, '("Petr" "Ivan" "Petr")'),
    ('[:find ?e ?n :in $ [?n ...] :where [?e :name ?n]]', DB, '[]'),
    ('[:find ?e ?n :in $ [?n ...] :where [?e :name ?n]]', DB, 'nil'),
    ('[:find ?e ?n :in $ [?n ...] :where [?e :name ?n]]', DB, '"Ivan"'),
    ('[:find ?e ?n :in $ [?n ...] :where [?e :name ?n]]', DB, '5'),
    ('[:find ?e ?n ?a :in $ [[?n ?a] ...] :where [?e :name ?n] [?e :age ?a]]', DB, '[["Ivan" 15] ["Petr" 37] ["Ivan" 37] ["Petr" 15]]'),
    ('[:find ?e ?n ?a :in $ [[?n ?a] ...] :where [?e :name ?n] [?e :age ?a]]', DB, '{"Ivan" 15, "Petr" 37}'),
    ('[:find ?e ?n ?a :in $ [[?n ?a]] :where [?e :name ?n] [?e :age ?a]]', DB, '#{["Ivan" 15] ["Petr" 37]}'),
    ('[:find ?e ?n :in $ [?n _] :where [?e :name ?n]]', DB, '["Ivan" 1 2 3]'),
    ('[:find ?e ?n :in $ [_ ?n] :where [?e :name ?n]]', DB, '[0 "Petr"]'),
    ('[:find ?e ?n :in $ _ ?n :where [?e :name ?n]]', DB, ':ignored', '"Petr"'),
    ('[:find ?e ?n :in $ _ _ ?n :where [?e :name ?n]]', DB, ':ignored', ':too', '"Petr"'),
    ('[:find ?e :in $ ?e :where [?e :name]]', DB, '1'),
    ('[:find ?e :in $ ?e :where [?e :name]]', DB, '4'),
    ('[:find ?e :in $ [?e ...] :where [?e :name]]', DB, '[1 2 3 4 5]'),
    ('[:find ?e ?a :in $ ?e ?a :where [?e ?a]]', DB, '1', ':name'),
    ('[:find ?v :in $ ?e ?a :where [?e ?a ?v]]', DB, '1', ':name'),
    ('[:find ?v :in $ ?e ?a :where [?e ?a ?v]]', DB, '2', '"name"'),
    ('[:find ?e :in $ ?tx :where [?e _ _ ?tx]]', DB, '536870913'),
    ('[:find ?e :in $ ?tx :where [?e _ _ ?tx]]', DB, '536870914'),
])

plain([
    ('[:find ?id :in $ [?id ...] :where [?id :age _]]', '[[1 :name "Ivan"] [2 :name "Petr"]]', '[]'),
    ('[:find ?id :in $ [[?id]] :where [?id :age _]]', '[[1 :name "Ivan"] [2 :name "Petr"]]', '[]'),
    ('[:find ?x ?z :in [?x _ ?z]]', '[:x :y :z]'),
    ('[:find ?x ?z :in [[?x _ ?z]]]', '[[:x :y :z] [:a :b :c]]'),
    ('[:find ?a ?b :in [?a ?b]]', ':a'),
    ('[:find ?a :in [?a ...]]', ':a'),
    ('[:find ?a ?b :in [?a ?b]]', '[:a]'),
    ('[:find ?a ?b :in [?a ?b]]', 'nil'),
    ('[:find ?a ?b :in [?a ?b]]', '[:a :b :c]'),
    ('[:find ?a ?b :in [?a ?b]]', '"ab"'),
    ('[:find ?a ?b :in [[?a ?b]]]', '[[1 2] [3]]'),
    ('[:find ?a ?b :in [[?a ?b]]]', '[[1 2] 3]'),
    ('[:find ?a ?b :in [[?a ?b]]]', '[[1 2] nil]'),
    ('[:find ?a ?b :in [?a [?b ...]]]', '[1 [2 3 4]]'),
    ('[:find ?a ?b ?c :in [?a [?b ?c]]]', '[1 [2 3]]'),
    ('[:find ?a ?b ?c :in [[?a [?b ?c]] ...]]', '[[1 [2 3]] [4 [5 6]]]'),
    ('[:find ?a ?b :in [[?a [?b ...]] ...]]', '[[1 [2 3]] [4 [5 6]] [7 []]]'),
    ('[:find ?a ?b :in [[?a [?b ...]] ...]]', '[[7 []] [1 [2 3]]]'),
    ('[:find ?a :in [[?a _] ...]]', '{:x 1 :y 2}'),
    ('[:find ?a ?a :in ?a]', '1'),
    ('[:find ?a :in ?a ?a]', '1', '2'),
    ('[:find ?a :in [?a ?a]]', '[1 2]'),
    ('[:find ?a ?b :in ?a ?b]', '[1 2]', '{:k :v}'),
    ('[:find ?a ?b :in ?a ?b]', '#{1 2}', 'nil'),
    ('[:find ?a ?b ?c ?d ?e ?f ?g ?h ?i ?j :in ?a ?b ?c ?d ?e ?f ?g ?h ?i ?j]', '1', '2', '3', '4', '5', '6', '7', '8', '9', '10'),
    ('[:find ?a ?b ?c ?d ?e ?f ?g ?h ?i ?j :in [?a ...] [?b ...] ?c ?d ?e ?f ?g ?h ?i ?j]', '[1 11]', '[2 22]', '3', '4', '5', '6', '7', '8', '9', '10'),
    ('[:find ?j ?i ?h ?g ?f ?e ?d ?c ?b ?a :in [?a ...] [?b ...] [?c ...] ?d ?e ?f ?g ?h ?i [?j ...]]', '[1 11]', '[2 22]', '[3 33]', '4', '5', '6', '7', '8', '9', '[10 100]'),
    ('[:find ?a ?b ?c ?d ?e ?f ?g ?h ?i ?j ?k ?l :in ?a ?b ?c ?d ?e ?f ?g ?h ?i ?j ?k ?l]', '1', '2', '3', '4', '5', '6', '7', '8', '9', '10', '11', '12'),
    ('[:find ?a ?b ?c ?d ?e ?f ?g ?h ?i ?j ?k ?l :in ?a ?b ?c ?d ?e ?f ?g ?h ?i ?j ?k]', '1', '2', '3', '4', '5', '6', '7', '8', '9', '10', '11'),
    ('[:find ?a :in ?a _ _ _ _ _ _ _ _ _ _]', '1', '2', '3', '4', '5', '6', '7', '8', '9', '10', '11'),
    ('[:find ?a :in _ _ ?a _]', '1', '2', '3', '4'),
])

# nested bindings, functions as inputs
plain([
    ('[:find ?k ?v :in [[?k ?v] ...] :where [(> ?v 1)]]', '{:a 1, :b 2, :c 3}'),
    ('[:find ?k ?min ?max :in [[?k ?v] ...] ?minmax :where [(?minmax ?v) [?min ?max]] [(> ?max ?min)]]', '{:a [1 2 3 4] :b [5 6 7] :c [3]}', '#f minmax'),
    ('[:find ?k ?x :in [[?k [?min ?max]] ...] ?range :where [(?range ?min ?max) [?x ...]] [(even? ?x)]]', '{:a [1 7] :b [2 4]}', '#f range'),
    ('[:find ?name :in [?name ...] ?key :where [(re-pattern ?key) ?pattern] [(re-find ?pattern ?name)]]', '#{"abc" "abcX" "aXb"}', '"X"'),
    ('[:find ?m ?m-value :in [[?k ?m] ...] ?m-key :where [(get ?m ?m-key) ?m-value]]', '{:a {:b 1} :c {:d 2}}', ':d'),
    ('[:find [?e ...] :where [?e :s b]]', '[[1 :s a] [2 :s b]]'),
])
with_db('nil', '[{:person/name "Joe"}]', [
    ('[:find ?name :in $ ?my-fn :where [?e :person/name ?name] [(?my-fn) ?result] [(< ?result 3)]]', DB, '#f five'),
    ('[:find ?name ?result :in $ ?my-fn :where [?e :person/name ?name] [(?my-fn) ?result] [(> ?result 3)]]', DB, '#f five'),
])
with_db('nil', '[{:db/id 1, :s a} {:db/id 2, :s b}]', ['[:find [?e ...] :where [?e :s b]]', '[:find ?e ?s :where [?e :s ?s]]'])

# constant substitution: how much of the index a pattern reads (issue-462)
steps = setup('{:a {:db/index true} :b {:db/index true} :c {:db/index true}}',
              '[' + ' '.join('[:db/add %d :%s "%d%s"]' % (e, a, e, a) for e in range(1, 11) for a in 'abc') + ']')
steps += [qc('[:find ?v :where [5 :b ?v]]'), qc('[:find ?a :where [5 ?a "5b"]]'), qc('[:find ?e :where [?e :b "5b"]]'),
          qc('[:find ?e ?a ?v :in $ ?e ?a :where [?e ?a ?v]]', '5', ':b'), qc('[:find ?e2 ?a ?v :in $ ?a ?v :where [?e ?a ?v] [?e2 ?a ?v]]', ':b', '"5b"'),
          qc('[:find ?a ?v :in $ ?e :where [?e ?a ?v]]', '5'), qc('[:find ?e ?a :where [?e ?a "5b"]]'), qc('[:find ?e ?a :in $ ?v :where [?e ?a ?v]]', '"5b"'),
          qc('[:find ?e ?a :in $ [?v ...] :where [?e ?a ?v]]', '["5b"]'), qc('[:find ?e ?a :where [(ground "5b") ?v] [?e ?a ?v]]'),
          qc('[:find ?e ?a :in $ [?v ...] :where [?e ?a ?v]]', '["5b" "6c"]'), qc('[:find ?e :where [?e :a] [?e :b "5b"]]'),
          qc('[:find ?e :where [?e :b "5b"] [?e :a]]'), qc('[:find ?e ?v :where [?e :b "5b"] [?e :a ?v]]'),
          qc('[:find ?e :where [?e _ _ 536870913]]'), qc('[:find ?e :where [?e :a _ 536870913]]'), qc('[:find ?e :where [3 :a "3a" 536870913] [?e :b "3b"]]')]
case(steps)

# ---------------------------------------------------------------- query_fns.cljc
plain([('[:find ?x :in [?x ...] :where [(> 2 1)]]', '[:a :b :c]'),
       ('[:find ?x :in [?x ...] :where [(> 1 2)]]', '[:a :b :c]'),
       '[:find ?vowel :where [(ground [:a :e :i :o :u]) [?vowel ...]]]',
       ('[:find ?x ?c :in [?x ...] :where [(count ?x) ?c]]', '["a" "abc"]'),
       '[:find [?tx-data ...] :where [(ground :db/add) ?op] [(vector ?op -1 :attr 12) ?tx-data]]',
       '[:find [?tx-data ...] :where [(hash-map :db/id -1 :age 92 :name "Aaron") ?tx-data]]',
       '[:find ?n :where [(identity 1) ?n] [(identity 2) ?n]]',
       '[:find ?n ?x :where [(identity [3 4]) [?n ?x]] [(identity [1 2]) [?n ?x]]]',
       ('[:find ?e ?y :where [?e :salary ?x] [(+ ?x 100) ?y]]', '[[0 :age 15] [1 :age 35]]'),
       ('[:find ?x :in [?in ...] ?f :where [(?f ?in) ?x]]', '[1 2 3 4]', '#f when-even'),
       ('[:find ?a ?c :in ?in :where [(ground ?in) [?a _ ?c]]]', '[:a :b :c]'),
       ('[:find ?in :in ?in :where [(ground ?in) _]]', ':a'),
       ('[:find ?x ?z :in ?in :where [(ground ?in) [[?x _ ?z] ...]]]', '[[:a :b :c] [:d :e :f]]'),
       ('[:find ?in :in [?in ...] :where [(ground ?in) _]]', '[]'),
       ('[:find ?pair ?x :in $ ?first ?second :where [_ _ ?pair] [(?first ?pair) ?x] [(?second ?pair) ?x]]', '[[1 :pair [:a :a]] [2 :pair [:b :c]]]', '#f first', '#f second'),
       ('[:find ?e :where [?e] [(< ?e 1)]]', '[[0] [1] [""]]'),
       ('[:find ?e :where [?e] [(<= ?e 1)]]', '[[0] [1] [""]]'),
       ('[:find ?e :where [?e] [(> ?e 1)]]', '[[0] [1] [""]]'),
       ('[:find ?e :where [?e] [(>= ?e 1)]]', '[[0] [1] [""]]'),
       ('[:find ?e :in [?e ...] :where [(fun ?e)]]', '[1]'),
       ('[:find ?e ?x :in [?e ...] :where [(fun ?e) ?x]]', '[1]'),
       '[:find ?x :where [(zero? ?x)]]',
       '[:find ?x :where [(inc ?x) ?y]]',
       '[:find ?x :where [?x] [(zero? $2 ?x)]]',
       '[:find ?x :in $2 :where [$2 ?x] [(zero? $ ?x)]]',
       ])
FNS_DB = '[{:db/id 1, :name "Ivan", :age 15} {:db/id 2, :name "Petr", :age 22, :height 240, :parent 1} {:db/id 3, :name "Slava", :age 37, :parent 2}]'
with_db('{:parent {:db/valueType :db.type/ref}}', FNS_DB, [
    '[:find ?e ?age ?height :where [?e :age ?age] [(get-else $ ?e :height 300) ?height]]',
    '[:find ?e ?height :where [?e :age] [(get-else $ ?e :height nil) ?height]]',
    '[:find ?e ?height :where [?e :age] [(get-else $ ?e :height false) ?height]]',
    '[:find ?e ?a ?v :where [?e :name _] [(get-some $ ?e :height :age) [?a ?v]]]',
    '[:find ?e ?a ?v :where [?e :name _] [(get-some $ ?e :nope :nah) [?a ?v]]]',
    '[:find ?e ?x :where [?e :name _] [(get-some $ ?e :height :age) ?x]]',
    '[:find ?e ?age :in $ :where [?e :age ?age] [(missing? $ ?e :height)]]',
    '[:find ?e :in $ :where [?e :age ?age] [(missing? $ ?e :_parent)]]',
    '[:find ?e1 ?e2 :where [?e1 :age ?a1] [?e2 :age ?a2] [(< ?a1 18 ?a2)]]',
    '[:find ?a1 :where [_ :age ?a1] [(< ?a1 22)]]',
    '[:find ?a1 :where [_ :age ?a1] [(<= ?a1 22)]]',
    '[:find ?a1 :where [_ :age ?a1] [(> ?a1 22)]]',
    '[:find ?a1 :where [_ :age ?a1] [(>= ?a1 22)]]',
    ('[:find ?e :in $ ?adult :where [?e :age ?a] [(?adult ?a)]]', DB, '#f gt18'),
    '[:find ?e1 ?e2 ?e3 :where [?e1 :age ?a1] [?e2 :age ?a2] [?e3 :age ?a3] [(+ ?a1 ?a2) ?a12] [(= ?a12 ?a3)]]',
    ('[:find ?n :in $ % :where [(identity 2) ?n] (my-vals ?n)]', DB, '[[(my-vals ?x) [(identity 1) ?x]] [(my-vals ?x) [(identity 2) ?x]] [(my-vals ?x) [(identity 3) ?x]]]'),
    ('[:find ?n :in $ % :where (my-vals ?n) [(identity 2) ?n]]', DB, '[[(my-vals ?x) [(identity 1) ?x]] [(my-vals ?x) [(identity 2) ?x]] [(my-vals ?x) [(identity 3) ?x]]]'),
    '[:find ?age :where [_ :age ?age] [(identity 100) ?age]]',
    '[:find ?age :where [(identity 100) ?age] [_ :age ?age]]',
    '[:find ?age :where [(identity 22) ?age] [_ :age ?age]]',
    '[:find ?age :where [_ :age ?age] [(identity 22) ?age]]',
    ('[:find ?e :in $ ?pred :where [?e :age ?a] [(?pred $ ?e 15)]]', DB, '#f age-is'),
    '[:find ?e ?a :where [_ :pred ?pred] [?e :age ?a] [(?pred ?a)]]',
    '[:find ?e ?n :where [?e :name ?n] [(get-else $ ?e :parent 0) ?p] [(> ?p 0)]]',
    '[:find ?e ?p :where [?e :name ?n] [(get-else $ ?e :parent :none) ?p]]',
    '[:find ?e ?p :where [?e :name] [(get-some $ ?e :parent) [_ ?p]]]',
    '[:find ?x :where [(get-else $ 99 :name "nobody") ?x]]',
    '[:find ?x :where [(get-else $ 1 :name "nobody") ?x]]',
    '[:find ?x :where [(get-else $ [:name "x"] :name "nobody") ?x]]',
    '[:find ?x :where [(get-else $ :ident :name "nobody") ?x]]',
    '[:find ?x :where [(get-some $ 2 :nope :height :age) ?x]]',
    '[:find ?x :where [(missing? $ 2 :height) ?x]]',
    '[:find ?x :where [(missing? $ 1 :height) ?x]]',
    '[:find ?x :where [(missing? $ 99 :height) ?x]]',
    '[:find ?x :where [(missing? $ 1 :_parent) ?x]]',
    '[:find ?x :where [(missing? $ 3 :_parent) ?x]]',
    '[:find ?x :where [(missing? $ 3 :db/id) ?x]]',
])
PRED_DB = '[{:db/id 1 :name "Ivan" :age 10} {:db/id 2 :name "Ivan" :age 20} {:db/id 3 :name "Oleg" :age 10} {:db/id 4 :name "Oleg" :age 20}]'
with_db('nil', PRED_DB, [
    '[:find ?e ?a :where [?e :age ?a] [(> ?a 10)]]',
    '[:find ?e ?e2 :where [?e :name] [?e2 :name] [(< ?e ?e2)]]',
    '[:find ?e ?e2 :where [?e :age ?a] [?e2 :age ?a2] [(< ?e ?e2)]]',
    '[:find ?e ?e2 :where [?e :name "Ivan"] [?e2 :name "Oleg"] [(= ?e ?e2)]]',
    '[:find ?e :where [?e :name "Ivan"] [?e :age 20] [(= ?e 2)]]',
    '[:find ?e :where [?e :name "Ivan"] [?e :age 20] [(= ?e 1)]]',
    ('[:find ?e :in $ ?pred :where [?e :age ?a] [(?pred $ ?e 10)]]', DB, '#f age-is'),
    # the quirk of check-bound: arguments are counted before they are looked for
    '[:find ?e :where [?e :age ?a] [(= ?a ?a)]]',
    ('[:find ?a :in ?a :where [(= ?a ?a)]]', '1'),
    ('[:find ?a ?b :in ?a :where [(+ ?a ?a) ?b]]', '1'),
    ('[:find ?a ?b :in ?a ?c :where [(+ ?a ?a) ?b]]', '1', '2'),
    ('[:find ?a ?b :in ?a ?c :where [(+ ?a ?a ?a) ?b]]', '1', '2'),
    # predicates and functions between relations
    '[:find ?e ?e2 :where [?e :name "Ivan"] [?e2 :name "Oleg"] [(< ?e ?e2)] [?e :age ?a] [?e2 :age ?a]]',
    '[:find ?e ?s :where [?e :age ?a] [?e :name ?n] [(str ?n "-" ?a) ?s]]',
    '[:find ?e ?s :where [?e :age ?a] [(* ?a 2) ?s] [?e :name "Ivan"]]',
    '[:find ?e ?b :where [?e :age ?a] [(> ?a 10) ?b]]',
    '[:find ?e ?b :where [?e :age ?a] [(> ?a 10) ?b] [(true? ?b)]]',
    '[:find ?e ?x ?y :where [?e :age ?a] [(vector ?a ?e) [?x ?y]]]',
    '[:find ?e ?x :where [?e :age ?a] [(vector ?a ?e) [?x ...]]]',
    '[:find ?e ?x :where [?e :age ?a] [(vector ?a ?e) [?x ?e]]]',
    '[:find ?e ?x :where [?e :age ?a] [(vector ?a ?e) [?x ?a]]]',
    '[:find ?e ?x :where [?e :age ?a] [(range ?e) [?x ...]]]',
    '[:find ?e ?n :where [?e :age ?a] [(identity ?e) ?e2] [?e2 :name ?n]]',
    '[:find ?e ?a :where [?e :age ?a] [(ground 20) ?a]]',
    '[:find ?e ?a :where [(ground 20) ?a] [?e :age ?a]]',
    '[:find ?e ?a :where [(ground [10 20]) [?a ...]] [?e :age ?a]]',
    '[:find ?e ?a :where [?e :age ?a] [(ground [10 20]) [?a ...]]]',
    '[:find ?e ?n :where [(ground [[1 "Ivan"] [3 "Oleg"] [4 "Ivan"]]) [[?e ?n]]] [?e :name ?n]]',
    '[:find ?e ?n :where [?e :name ?n] [(ground [[1 "Ivan"] [3 "Oleg"] [4 "Ivan"]]) [[?e ?n]]]]',
    '[:find ?x :where [(ground nil) ?x]]',
    '[:find ?x :where [(ground false) ?x]]',
    '[:find ?x ?y :where [(ground [nil false 1]) [?x ...]] [(some? ?x) ?y]]',
    '[:find ?x :where [(ground 1) ?x] [(ground 1) ?x]]',
    '[:find ?x ?y :where [(ground 1) ?x] [(ground 2) ?y]]',
    '[:find ?x ?y ?z :where [(ground [1 2]) [?x ...]] [(ground [3 4]) [?y ...]] [(+ ?x ?y) ?z]]',
    '[:find ?x ?y ?z :where [(ground [1 2]) [?x ...]] [(ground [3 4]) [?y ...]] [(+ ?x ?y) ?z] [(odd? ?z)]]',
    '[:find ?e ?f :where [?e :age] [(ground :k) ?f]]',
    '[:find ?f :where [(ground :k) ?f] [_ :age]]',
    '[:find ?e :where [?e :age ?a] [(nope ?a)]]',
    '[:find ?e :where [?e :age ?a] [(?a ?e)]]',
    '[:find ?e :where [?e :age ?a] [(?e ?a) ?x]]',
    '[:find ?e :where [?e :age ?a] [(?unbound ?a)]]',
    '[:find ?e :where [?e :age ?a] [(?unbound ?a) ?x]]',
    ('[:find ?e :in $ ?f :where [?e :age ?a] [(?f ?a)]]', DB, 'nil'),
    ('[:find ?e :in $ ?f :where [?e :age ?a] [(?f ?a)]]', DB, 'false'),
    ('[:find ?e ?x :in $ ?f :where [?e :age ?a] [(?f ?a) ?x]]', DB, 'false'),
    ('[:find ?e ?x :in $ ?f :where [?e :age ?a] [(?f ?a) ?x]]', DB, '{10 :ten}'),
    ('[:find ?e ?x :in $ ?f :where [?e :age ?a] [(?f ?a) ?x]]', DB, '#{20}'),
    ('[:find ?e ?x :in $ ?f :where [?e :name ?n] [(?f ?e) ?x]]', DB, '[:zero :one :two :three :four]'),
    ('[:find ?e ?x :in $ ?f :where [?e :name ?n] [(?f ?m) ?x] [(ground {:k 1}) ?m]]', DB, ':k'),
    ('[:find ?e ?x :in $ ?f :where [?e :name ?n] [(ground {:k 1}) ?m] [(?f ?m) ?x]]', DB, ':k'),
    ('[:find ?e :in $ ?f :where [?e :age ?a] [(?f ?a)]]', DB, '#f throw'),
    ('[:find ?e ?x :in $ ?f :where [?e :age ?a] [(?f ?a) ?x]]', DB, '#f throw'),
    ('[:find ?e ?x :in $ ?f :where [?e :age ?a] [(?f ?a) ?x]]', DB, '#f nil-fn'),
    ('[:find ?e ?x :in $ ?f :where [?e :age ?a] [(?f ?a) ?x]]', DB, '#f false-fn'),
    ('[:find ?e ?x ?y :in $ ?f :where [?e :age ?a] [(?f ?e ?a) [?x ?y]]]', DB, '#f pair'),
    ('[:find ?e ?x ?y :in $ ?f :where [?e :age ?a] [(?f ?e) [[?x ?y]]]]', DB, '#f triples'),
    ('[:find ?e ?x :in $ ?f :where [?e :age ?a] [(?f ?e) [[?x _] ...]]]', DB, '#f triples'),
    ('[:find ?e ?s :in $ ?f :where [?e :age ?a] [?e :name ?n] [(?f ?a ?e 1000) ?s]]', DB, '#f plus'),
])
with_db('{:name {:db/unique :db.unique/identity}}', '[{:db/id 1 :name "Ivan" :age 15} {:db/id 2 :name "Petr" :age 22 :height 240}]', [
    ('[:find ?height . :in $ ?e :where [(get-else $ ?e :height "Unknown") ?height]]', DB, '[:name "Ivan"]'),
    ('[:find ?e ?a ?v :in $ ?e :where [(get-some $ ?e :weight :age :height) [?a ?v]]]', DB, '[:name "Petr"]'),
    ('[:find ?e ?m :in $ ?e :where [(missing? $ ?e :height) ?m]]', DB, '[:name "Petr"]'),
    ('[:find ?e ?m :in $ ?e :where [(missing? $ ?e :height) ?m]]', DB, '[:name "Ivan"]'),
    ('[:find ?e ?m :in $ ?e :where [(missing? $ ?e :height) ?m]]', DB, '[:name "Nobody"]'),
    ('[:find ?e ?m :in $ ?e :where [(get-else $ ?e :height 0) ?m]]', DB, '[:name "Nobody"]'),
    ('[:find ?e ?m :in $ ?e :where [(get-some $ ?e :height :age) ?m]]', DB, '[:name "Nobody"]'),
    ('[:find ?e ?m :in $ ?e :where [(get-else $ ?e :height 0) ?m]]', DB, '[:age 15]'),
    ('[:find ?e ?m :in $ ?e :where [(get-else $ ?e :height 0) ?m]]', DB, '"str"'),
    ('[:find ?e ?m :in $ ?e :where [(get-else $ ?e nil 0) ?m]]', DB, '1'),
    ('[:find ?e ?m :in $ ?e :where [(get-else $ ?e nil 0) ?m]]', DB, '[:name "Nobody"]'),
])

# ---------------------------------------------------------------- built-in functions, one by one
def fn_cases(calls, inputs='[1]'):
    plain([('[:find ?r :in [?x ...] :where [%s ?r]]' % c, inputs) for c in calls])

fn_cases(['(= 1 1)', '(= 1 2)', '(= 1 1 1)', '(= 1 1 2)', '(= 1)', '(= [1 2] (1 2))', '(= nil nil)', '(= "a" "a")', '(= :a :a)', '(= 1 1.0)', '(= 1 "1")', '(= {:a 1} {:a 1})', '(= #{1 2} #{2 1})',
          '(== 1 1)', '(== 1 2)', '(== 1 1 1)', '(== 2)', '(not= 1 1)', '(not= 1 2)', '(not= 1 1 1)', '(not= 1 1 2)', '(not= 1)', '(!= 1 1)', '(!= 1 2)', '(!= 1 2 1)',
          '(< 1 2)', '(< 2 1)', '(< 1 1)', '(< 1 2 3)', '(< 1 3 2)', '(< 1)', '(< 1 2 3 4 5)', '(< 1 2 3 3 5)', '(> 2 1)', '(> 1 2)', '(> 3 2 1)', '(> 3 1 2)', '(> 1)', '(<= 1 1)', '(<= 2 1)', '(<= 1 1 2)', '(<= 1 2 1)', '(<= 1)',
          '(>= 1 1)', '(>= 1 2)', '(>= 2 1 1)', '(>= 2 1 3)', '(>= 1)', '(< "a" "b")', '(< "b" "a")', '(< :a :b)', '(< :b :a)', '(< 1 "a")', '(< "a" 1)', '(< :a "a")', '(< "a" :a)', '(< [1 2] [1 3])', '(< [1 2] [1 2 0])', '(< [2] [1 2])',
          '(< nil 1)', '(< 1 nil)', '(< nil nil)', '(< true false)', '(< false true)', '(< a b)', '(< b a)', '(< 1 :a)', '(< :a 1)', '(< 1.5 2)', '(< -1 0)', '(< "a" "B")', '(< {:a 1} {:a 2})', '(< #{1} #{2})',
          '(> "b" "a")', '(> :b :a)', '(>= "a" "a")', '(<= :a :a)', '(< [1 "a"] [1 :a])', '(< [nil] [1])', '(< [1] [nil])', '(< (1 2) [1 3])'])
fn_cases(['(+)', '(+ 1)', '(+ 1 2)', '(+ 1 2 3)', '(+ 1.5 2.25)', '(+ "a" "b")', '(+ "a" 1)', '(+ 1 "a")', '(+ 1 nil)', '(+ nil nil)', '(+ true 1)', '(+ 1 :a)', '(+ "a" nil)', '(+ [1] 2)', '(+ 0.1 0.2)',
          '(- 5)', '(- 5 2)', '(- 5 2 1)', '(- "5" 2)', '(- "a" 2)', '(- nil 2)', '(- 5 nil)', '(-)', '(* )', '(* 3)', '(* 3 4)', '(* 2 3 4)', '(* "3" 4)', '(* "a" 4)', '(* nil 4)',
          '(/ 2)', '(/ 6 3)', '(/ 7 2)', '(/ 24 2 3)', '(/ 1 0)', '(/ -1 0)', '(/ 0 0)', '(/ "6" 3)', '(/)', '(quot 7 2)', '(quot -7 2)', '(quot 7 -2)', '(quot 7.5 2)', '(quot 7 0)',
          '(rem 7 2)', '(rem -7 2)', '(rem 7 -2)', '(rem 7.5 2)', '(rem 7 0)', '(mod 7 2)', '(mod -7 2)', '(mod 7 -2)', '(mod 7.5 2)', '(mod 7 0)',
          '(inc 1)', '(inc 1.5)', '(inc "a")', '(inc nil)', '(dec 1)', '(dec "5")', '(dec nil)', '(max 1 2)', '(max 2 1)', '(max 1)', '(max 1 2 3)', '(max 3 2 1)', '(max "a" "b")', '(max "b" "a")', '(max 1 "a")',
          '(max :a :b)', '(max :b :a)', '(max 1 nil)', '(max nil 1)', '(max -1 nil)', '(max "10" 9)', '(max 9 "10")', '(min 1 2)', '(min 2 1)', '(min 1)', '(min 3 2 1)', '(min "a" "b")', '(min :b :a)', '(min :a :b)', '(min 1 nil)', '(min nil -1)', '(max)', '(min)',
          '(zero? 0)', '(zero? 1)', '(zero? 0.0)', '(zero? "0")', '(zero? nil)', '(pos? 1)', '(pos? 0)', '(pos? -1)', '(pos? "1")', '(pos? nil)', '(neg? -1)', '(neg? 0)', '(neg? "-1")', '(neg? nil)',
          '(even? 2)', '(even? 3)', '(even? 0)', '(even? -2)', '(even? 2.5)', '(even? "2")', '(even? nil)', '(odd? 3)', '(odd? 2)', '(odd? -3)', '(odd? 2.5)', '(odd? :a)',
          '(compare 1 2)', '(compare 2 1)', '(compare 1 1)', '(compare "a" "b")', '(compare :a :b)', '(compare nil 1)', '(compare 1 nil)', '(compare nil nil)', '(compare [1 2] [1 3])', '(compare [1 2] [1])', '(compare 1 "a")', '(compare :a "a")', '(compare true false)', '(compare a b)',
          '(rand-int 1)', '(true? true)', '(true? 1)', '(true? false)', '(false? false)', '(false? nil)', '(nil? nil)', '(nil? false)', '(some? nil)', '(some? false)', '(not nil)', '(not false)', '(not 0)', '(not "")',
          '(and)', '(and 1)', '(and 1 2)', '(and 1 nil 2)', '(and false nil)', '(and nil false)', '(and 1 2 3)', '(or)', '(or 1)', '(or nil 2)', '(or nil false)', '(or false nil)', '(or 1 2)', '(or nil nil 3)',
          '(identical? 1 1)', '(identical? "a" "a")', '(identical? 1 2)', '(identical? nil nil)', '(identity 5)', '(identity nil)', '(identity [1 2])'])
fn_cases(['(keyword "a")', '(keyword "a/b")', '(keyword :a)', '(keyword a)', '(keyword a/b)', '(keyword "a" "b")', '(keyword nil "b")', '(keyword :a :b)', '(keyword a b)', '(keyword 5)', '(keyword nil)',
          '(meta 5)', '(meta [1])', '(name :a)', '(name :a/b)', '(name "s")', '(name a/b)', '(name 5)', '(name nil)', '(namespace :a)', '(namespace :a/b)', '(namespace a/b)', '(namespace "s")', '(namespace 5)',
          '(vector)', '(vector 1 2)', '(vector nil)', '(list)', '(list 1 2)', '(set [1 2 1])', '(set nil)', '(set #{1})', '(set {:a 1})', '(set "ab")', '(set 5)',
          '(hash-map)', '(hash-map :a 1)', '(hash-map :a 1 :b 2 :c 3)', '(hash-map :a 1 :b)', '(hash-map :c 3 :b 2 :a 1 :a 5)', '(array-map)', '(array-map :a 1 :b 2)', '(array-map :b 2 :a 1 :b 3)', '(array-map :a)',
          '(array-map 1 1 2 2 3 3 4 4 5 5 6 6 7 7 8 8 9 9 10 10)', '(hash-map 1 1 2 2 3 3 4 4 5 5 6 6 7 7 8 8 9 9 10 10)',
          '(count [1 2 3])', '(count "abc")', '(count nil)', '(count {:a 1})', '(count #{1 2})', '(count 5)', '(count :a)', '(count ())', '(range 3)', '(range 1 4)', '(range 0 10 3)', '(range 5 0 -2)', '(range 0)', '(range 3 1)', '(range 0 1 0.25)',
          '(not-empty [1])', '(not-empty [])', '(not-empty nil)', '(not-empty "")', '(not-empty "a")', '(not-empty {})', '(not-empty 5)', '(empty? [])', '(empty? [1])', '(empty? nil)', '(empty? "")', '(empty? "a")', '(empty? {})', '(empty? #{1})', '(empty? 5)',
          '(contains? {:a 1} :a)', '(contains? {:a 1} :b)', '(contains? {:a nil} :a)', '(contains? #{1} 1)', '(contains? #{1} 2)', '(contains? [5 6] 1)', '(contains? [5 6] 2)', '(contains? [5 6] 5)', '(contains? nil 1)', '(contains? "abc" 1)', '(contains? "abc" 5)', '(contains? 5 1)', '(contains? :a 1)',
          '(str)', '(str nil)', '(str 1)', '(str 1 2)', '(str "a" :b c 1.5 nil true [1 "x"] {:a 1})', '(str 1.0)', '(str 1e21)', '(str 1e-7)', '(str -0.0)',
          '(subs "abcdef" 2)', '(subs "abcdef" 2 4)', '(subs "abcdef" 4 2)', '(subs "abcdef" -1)', '(subs "abcdef" 10)', '(subs "abc" 1 100)', '(subs 5 1)', '(subs nil 1)',
          '(get {:a 1} :a)', '(get {:a 1} :b)', '(get {:a 1} :b 2)', '(get [5 6] 1)', '(get [5 6] 2)', '(get [5 6] 2 :nf)', '(get #{1} 1)', '(get nil :a)', '(get nil :a :nf)', '(get "abc" 1)', '(get 5 1)', '(get {:a nil} :a :nf)',
          '(pr-str)', '(pr-str 1)', '(pr-str "a")', '(pr-str "a" :b [1 "c"])', '(print-str "a" :b [1 "c"])', '(println-str "a" 1)', '(prn-str "a" 1)', '(prn-str)', '(println-str)',
          '(tuple 1 2)', '(tuple)', '(untuple [1 2])', '(ground 5)', '(-differ? 1 2)', '(-differ? 1 1)', '(-differ? 1 2 1 2)', '(-differ? 1 2 2 1)', '(-differ?)', '(-differ? 1 2 3)', '(-differ? 1)',
          '(clojure.string/blank? "")', '(clojure.string/blank? "  ")', '(clojure.string/blank? "a")', '(clojure.string/blank? nil)', '(clojure.string/blank? " \\t\\n")', '(clojure.string/blank? 5)',
          '(clojure.string/includes? "abc" "b")', '(clojure.string/includes? "abc" "x")', '(clojure.string/includes? "abc" "")', '(clojure.string/includes? "anullb" nil)', '(clojure.string/includes? nil "a")', '(clojure.string/includes? 5 "a")',
          '(clojure.string/starts-with? "abc" "ab")', '(clojure.string/starts-with? "abc" "bc")', '(clojure.string/starts-with? "abc" "")', '(clojure.string/ends-with? "abc" "bc")', '(clojure.string/ends-with? "abc" "ab")', '(clojure.string/ends-with? "abc" "")', '(clojure.string/ends-with? "c" "abc")',
          '(re-pattern "a+")', '(re-find (re-pattern "a+") "caat")', '(type 1)', '(type "a")', '(type :a)', '(type nil)', '(type [1])', '(type {})', '(type #{})', '(type ())', '(type (1))', '(type a)', '(type true)', '(<)', '(>)', '(<=)', '(>=)', '(=)', '(==)', '(not=)', '(keyword)', '(inc)', '(quot 7)'])
plain([('[:find ?r :in ?p [?s ...] :where [(re-pattern ?p) ?re] [(%s ?re ?s) ?r]]' % f, p, '["" "a" "aaa" "abc" "cab" "xaby" "ab-ab" "A" "aXbXc" "foo bar" "2020-01-02"]')
       for f in ['re-find', 're-matches', 're-seq']
       for p in ['"a"', '"a+"', '"^a"', '"b$"', '"(a)(b)?"', '"[a-c]+"', '"X"', '"(?i)a"', '"\\\\d+"', '"(\\\\d+)-(\\\\d+)"', '"a|b"', '""', '"."', '"\\\\w+"', '"\\\\s"', '"(a)|(b)"', '"x*"', '"a{2,}"', '"[^a]"', '"(?:ab)+"', '"\\\\bfoo\\\\b"']])
plain([('[:find ?r :in ?p ?s :where [(%s ?p ?s) ?r]]' % f, p, s) for f in ['re-find', 're-matches', 're-seq'] for (p, s) in [('"a"', '"a"'), ('5', '"a"'), ('nil', '"a"')]]
      + [('[:find ?r :in ?p ?s :where [(re-pattern ?p) ?re] [(%s ?re ?s) ?r]]' % f, p, s) for f in ['re-find', 're-matches', 're-seq'] for (p, s) in [('"a"', '5'), ('"a"', 'nil'), ('"a"', ':a'), ('"("', '"a"'), ('"*"', '"a"'), ('5', '"a"'), ('nil', '"a"')]])
plain([('[:find ?r :in ?f [?x ...] :where [(complement ?f) ?g] [(?g ?x) ?r]]', '#f even?', '[1 2 3]'),
       ('[:find ?x :in ?f [?x ...] :where [(complement ?f) ?g] [(?g ?x)]]', '#f even?', '[1 2 3 4]'),
       ('[:find ?x ?r :in [?x ...] :where [(rand) ?n] [(< ?n 1) ?r]]', '[1 2]'),
       ('[:find ?x ?r :in [?x ...] :where [(rand 10) ?n] [(< ?n 10) ?r]]', '[1 2]'),
       ('[:find ?x ?r :in [?x ...] :where [(rand-int 10) ?n] [(< -1 ?n 10) ?r]]', '[1 2]'),
       ('[:find ?x ?y :in [?x ...] :where [(untuple ?x) [?y ...]]]', '[[1 2] [3 4]]'),
       ('[:find ?x ?y ?z :in [?x ...] :where [(untuple ?x) [?y ?z]]]', '[[1 2] [3 4]]'),
       ('[:find ?t :in [[?x ?y]] :where [(tuple ?x ?y) ?t]]', '[[1 2] [3 4]]'),
       ])

# ---------------------------------------------------------------- query_aggregates.cljc
MONSTERS = '[["Cerberus" 3] ["Medusa" 1] ["Cyclops" 1] ["Chimera" 1]]'
COLORS = '[[:red 1] [:red 2] [:red 3] [:red 4] [:red 5] [:blue 7] [:blue 8]]'
NUMS = '[10 15 20 35 75]'
plain([
    ('[:find ?heads :with ?monster :in [[?monster ?heads]]]', '[["Medusa" 1] ["Cyclops" 1] ["Chimera" 1]]'),
    ('[:find (sum ?heads) :in [[?monster ?heads]]]', MONSTERS),
    ('[:find (sum ?heads) (min ?heads) (max ?heads) (count ?heads) (count-distinct ?heads) :with ?monster :in [[?monster ?heads]]]', MONSTERS),
    ('[:find (min ?x) (max ?x) :in [?x ...]]', '[:a-/b :a/b]'),
    ('[:find (min 2 ?x) (max 2 ?x) :in [?x ...]]', '[:a/b :a-/b :a/c]'),
    ('[:find ?color (max ?amount ?x) (min ?amount ?x) :in [[?color ?x]] ?amount]', COLORS, '3'),
    ('[:find (avg ?x) :in [?x ...]]', NUMS), ('[:find (median ?x) :in [?x ...]]', NUMS), ('[:find (variance ?x) :in [?x ...]]', NUMS), ('[:find (stddev ?x) :in [?x ...]]', NUMS),
    ('[:find (median ?x) :in [?x ...]]', '[10 15 20 35]'), ('[:find (median ?x) :in [?x ...]]', '[7]'), ('[:find (median ?x) :in [?x ...]]', '["b" "a" "c"]'), ('[:find (median ?x) :in [?x ...]]', '["b" "a"]'),
    ('[:find ?color (aggregate ?agg ?x) :in [[?color ?x]] ?agg]', COLORS, '#f sort-reverse'),
    ('[:find ?color (aggregate ?agg ?x) :in [[?color ?x]] ?agg]', COLORS, '#f agg-sorted'),
    ('[:find ?color (aggregate ?agg ?x) :in [[?color ?x]] ?agg]', COLORS, '#f agg-count'),
    ('[:find ?color (aggregate ?agg ?x) :in [[?color ?x]] ?agg]', COLORS, '#f agg-first'),
    ('[:find ?color (aggregate ?agg 1 ?x) :in [[?color ?x]] ?agg]', COLORS, '#f agg-nth'),
    ('[:find ?color (aggregate ?agg ?n ?x) :in [[?color ?x]] ?agg ?n]', COLORS, '#f agg-nth', '0'),
    ('[:find ?color (aggregate ?agg ?x) :in [[?color ?x]] ?agg]', COLORS, '#f throw'),
    ('[:find ?color (aggregate ?agg ?x) :in [[?color ?x]] ?agg]', COLORS, 'nil'),
    ('[:find ?color (nope ?x) :in [[?color ?x]]]', COLORS),
    ('[:find (sum ?x) :in [?x ...]]', '[]'), ('[:find (count ?x) :in [?x ...]]', '[]'), ('[:find (count ?x) . :in [?x ...]]', '[]'), ('[:find [(count ?x) ...] :in [?x ...]]', '[]'), ('[:find [(count ?x)] :in [?x ...]]', '[]'),
    ('[:find (sum ?x) :in [?x ...]]', '[1 2 3.5]'), ('[:find (sum ?x) :in [?x ...]]', '["a" "b"]'), ('[:find (sum ?x) :in [?x ...]]', '[1 "a" 2]'), ('[:find (avg ?x) :in [?x ...]]', '[1 2]'), ('[:find (avg ?x) :in [?x ...]]', '["a"]'),
    ('[:find (count ?x) :in [?x ...]]', '[1 1 2]'), ('[:find (count-distinct ?x) :in [?x ...]]', '[1 1 2]'), ('[:find (count ?x) (count-distinct ?y) :in [[?x ?y]]]', '[[1 1] [2 1] [3 2]]'),
    ('[:find (distinct ?x) :in [?x ...]]', '[3 1 2 1 3]'), ('[:find (distinct ?y) :in [[?x ?y]]]', '[[1 :a] [2 :b] [3 :a] [4 :c]]'),
    ('[:find (distinct ?y) :in [[?x ?y]]]', '[' + ' '.join('[%d %d]' % (i, (i * 7) % 23) for i in range(40)) + ']'),
    ('[:find (min ?x) (max ?x) :in [?x ...]]', '[5 3 9 1 7]'), ('[:find (min ?x) (max ?x) :in [?x ...]]', '["b" "a" "c"]'), ('[:find (min ?x) (max ?x) :in [?x ...]]', '[1 "a"]'), ('[:find (min ?x) (max ?x) :in [?x ...]]', '[[1 2] [1 1] [0 9]]'),
    ('[:find (min 3 ?x) (max 3 ?x) :in [?x ...]]', '[5 3 9 1 7]'), ('[:find (min 1 ?x) (max 1 ?x) :in [?x ...]]', '[5 3 9 1 7]'), ('[:find (min 0 ?x) :in [?x ...]]', '[5 3 9 1 7]'), ('[:find (max 0 ?x) :in [?x ...]]', '[5 3 9 1 7]'),
    ('[:find (min 10 ?x) (max 10 ?x) :in [?x ...]]', '[5 3 9 1 7]'), ('[:find (min -1 ?x) :in [?x ...]]', '[5 3 9]'), ('[:find (max -1 ?x) :in [?x ...]]', '[5 3 9]'), ('[:find (min ?n ?x) :in [?x ...] ?n]', '[5 3 9]', '2'),
    ('[:find (rand 0 ?x) :in [?x ...]]', '[5 3 9]'), ('[:find (sample 0 ?x) :in [?x ...]]', '[5 3 9]'), ('[:find (count ?r) :in [?x ...] :where [(ground ?x) ?r]]', '[5 3 9]'),
    ('[:find ?x (count ?y) :in [[?x ?y]]]', '[[:a 1] [:b 2] [:a 3] [:c 4] [:b 5]]'),
    ('[:find (count ?y) ?x :in [[?x ?y]]]', '[[:a 1] [:b 2] [:a 3] [:c 4] [:b 5]]'),
    ('[:find ?x (count ?y) (sum ?y) :in [[?x ?y]]]', '[[:a 1] [:b 2] [:a 3] [:c 4] [:b 5]]'),
    ('[:find ?x ?z (count ?y) :in [[?x ?y ?z]]]', '[[:a 1 :p] [:b 2 :p] [:a 3 :q] [:c 4 :p] [:b 5 :p] [:a 6 :p]]'),
    ('[:find ?x (count ?y) :with ?z :in [[?x ?y ?z]]]', '[[:a 1 :p] [:b 2 :p] [:a 1 :q] [:c 4 :p] [:b 2 :p] [:a 1 :p]]'),
    ('[:find ?x (count ?y) :in [[?x ?y ?z]]]', '[[:a 1 :p] [:b 2 :p] [:a 1 :q] [:c 4 :p] [:b 2 :p] [:a 1 :p]]'),
    ('[:find [?x (count ?y)] :in [[?x ?y]]]', '[[:a 1] [:b 2] [:a 3]]'), ('[:find [(count ?y) ...] :in [[?x ?y]]]', '[[:a 1] [:b 2] [:a 3]]'), ('[:find (count ?y) . :in [[?x ?y]]]', '[[:a 1] [:b 2] [:a 3]]'),
    ('[:find ?x (count ?y) :keys k n :in [[?x ?y]]]', '[[:a 1] [:b 2] [:a 3]]'),
    # more groups than a small map holds: the groups come in the order of ClojureScript's hash map of them
    ('[:find ?x (count ?y) :in [[?x ?y]]]', '[' + ' '.join('[%d %d]' % (i % 13, i) for i in range(60)) + ']'),
    ('[:find ?x (sum ?y) (max ?y) :in [[?x ?y]]]', '[' + ' '.join('["k%d" %d]' % (i % 17, i) for i in range(70)) + ']'),
    ('[:find ?x ?z (count ?y) :in [[?x ?y ?z]]]', '[' + ' '.join('[:k%d %d %s]' % (i % 5, i, 'true' if i % 3 else 'false') for i in range(50)) + ']'),
    ('[:find (count ?y) ?x (min ?y) ?z :in [[?x ?y ?z]]]', '[' + ' '.join('[:k%d %d "%s"]' % (i % 5, i, 'abc'[i % 3]) for i in range(50)) + ']'),
    ('[:find ?x (distinct ?y) :in [[?x ?y]]]', '[' + ' '.join('[%d %d]' % (i % 3, i % 11) for i in range(50)) + ']'),
    ('[:find ?x (agg ?y) :in [[?x ?y]] agg]', COLORS, '#f agg-count'),
])
with_db('nil', '[[:db/add 1 :name "Petr"] [:db/add 1 :age 44] [:db/add 2 :name "Ivan"] [:db/add 2 :age 25] [:db/add 3 :name "Sergey"] [:db/add 3 :age 11]]', [
    '[:find [?name ...] :where [_ :name ?name]]',
    '[:find [?name ?age] :where [1 :name ?name] [1 :age ?age]]',
    '[:find ?name . :where [1 :name ?name]]',
    '[:find [?name ?age] :where [?e :name ?name] [?e :age ?age]]',
    '[:find ?name . :where [_ :name ?name]]',
    '[:find [(count ?name) ...] :where [_ :name ?name]]',
    '[:find [(count ?name)] :where [_ :name ?name]]',
    '[:find (count ?name) . :where [_ :name ?name]]',
    '[:find ?name . :where [_ :nope ?name]]',
    '[:find [?name ?age] :where [?e :nope ?name] [?e :age ?age]]',
    '[:find [?name ...] :where [_ :nope ?name]]',
    '[:find ?name ?age :where [?e :nope ?name] [?e :age ?age]]',
    # return maps
    '[:find ?name ?age :keys n a :where [?e :name ?name] [?e :age ?age]]',
    '[:find ?name ?age :syms n a :where [?e :name ?name] [?e :age ?age]]',
    '[:find ?name ?age :strs n a :where [?e :name ?name] [?e :age ?age]]',
    '[:find [?name ?age] :keys n a :where [?e :name ?name] [(= ?name "Ivan")] [?e :age ?age]]',
    '[:find [?name ?age] :keys n a :where [?e :name ?name] [(= ?name "Nobody")] [?e :age ?age]]',
    '[:find [?name ?age] :strs n a :where [?e :name ?name] [(= ?name "Nobody")] [?e :age ?age]]',
    '[:find ?name ?age :keys n a :where [?e :nope ?name] [?e :age ?age]]',
    '[:find ?name ?age :keys a/n b/a :where [?e :name ?name] [?e :age ?age]]',
    '[:find ?name ?age :keys n n :where [?e :name ?name] [?e :age ?age]]',
    '[:find ?name ?age :keys n a :with ?e :where [?e :name ?name] [?e :age ?age]]',
    '[:find ?name (count ?e) :keys n c :where [?e :name ?name]]',
    '[:find ?name (max ?age) :strs n c :where [?e :name ?name] [?e :age ?age]]',
    '[:find ?e (pull ?e [:name]) :keys id ent :where [?e :name ?name]]',
    '[:find ?name ?age :keys n :where [?e :name ?name] [?e :age ?age]]',
    '[:find ?name :keys n a :where [?e :name ?name]]',
    '[:find [?name ...] :keys n :where [?e :name ?name]]',
    '[:find ?name . :keys n :where [?e :name ?name]]',
    '[:find ?name ?age :keys n a :strs n a :where [?e :name ?name] [?e :age ?age]]',
    '[:find ?e ?name ?age ?e ?name ?age ?e ?name ?age ?e :keys a b c d e f g h i j :where [?e :name ?name] [?e :age ?age]]',
    '[:find ?e ?name ?age ?e ?name ?age ?e ?name ?age ?e :strs a b c d e f g h i j :where [?e :name ?name] [?e :age ?age]]',
    '[:find ?e ?name ?age ?e ?name ?age ?e ?name ?age ?e :syms j i h g f e d c b a :where [?e :name ?name] [?e :age ?age]]',
])

# ---------------------------------------------------------------- query_rules.cljc
FOLLOW = '[[5 :follow 3] [1 :follow 2] [2 :follow 3] [3 :follow 4] [4 :follow 6] [2 :follow 4]]'
plain([
    ('[:find ?e1 ?e2 :in $ % :where (follow ?e1 ?e2)]', FOLLOW, '[[(follow ?x ?y) [?x :follow ?y]]]'),
    ('[:find ?y ?x :in $ % :where [_ _ ?x] (rule ?x ?y) [(even? ?x)]]', FOLLOW, '[[(rule ?a ?b) [?a :follow ?b]]]'),
    ('[:find ?x :in $ % :where [?e _ _] (rule ?x)]', FOLLOW, '[[(rule ?e) [_ ?e _]]]'),
    ('[:find ?e2 :in $ ?e1 % :where (follow ?e1 ?e2)]', FOLLOW, '1', '[[(follow ?e2 ?e1) [?e2 :follow ?e1]] [(follow ?e2 ?e1) [?e2 :follow ?t] [?t :follow ?e1]]]'),
    ('[:find ?e2 :in $ ?e1 % :where (follow ?e1 ?e2)]', FOLLOW, '1', '[[(follow ?e1 ?e2) [?e1 :follow ?e2]] [(follow ?e1 ?e2) [?e1 :follow ?t] (follow ?t ?e2)]]'),
    ('[:find ?e1 ?e2 :in $ % :where (follow ?e1 ?e2)]', FOLLOW, '[[(follow ?e1 ?e2) [?e1 :follow ?e2]] [(follow ?e1 ?e2) [?e1 :follow ?t] (follow ?t ?e2)]]'),
    ('[:find ?e1 ?e2 :in $ % :where (follow ?e1 ?e2)]', '[[1 :follow 2] [2 :follow 3]]', '[[(follow ?e1 ?e2) [?e1 :follow ?e2]] [(follow ?e1 ?e2) (follow ?e2 ?e1)]]'),
    ('[:find ?e1 ?e2 :in $ % :where (follow ?e1 ?e2)]', '[[1 :follow 2] [2 :follow 3] [3 :follow 1]]', '[[(follow ?e1 ?e2) [?e1 :follow ?e2]] [(follow ?e1 ?e2) (follow ?e2 ?e1)]]'),
    ('[:find ?e1 ?e2 :in $ % :where (f1 ?e1 ?e2)]', '[[0 :f1 1] [1 :f2 2] [2 :f1 3] [3 :f2 4] [4 :f1 5] [5 :f2 6]]',
     '[[(f1 ?e1 ?e2) [?e1 :f1 ?e2]] [(f1 ?e1 ?e2) [?t :f1 ?e2] (f2 ?e1 ?t)] [(f2 ?e1 ?e2) [?e1 :f2 ?e2]] [(f2 ?e1 ?e2) [?t :f2 ?e2] (f1 ?e1 ?t)]]'),
    ('[:find ?x ?y :in $ % ?even :where (match ?even ?x ?y)]', FOLLOW, '[[(match ?pred ?e ?e2) [?e :follow ?e2] [(?pred ?e)] [(?pred ?e2)]]]', '#f even?'),
    ('[:find ?x ?y :in $ % :where (match ?x ?y)]', FOLLOW, '[[(match ?e ?e2) [?e :follow ?e2] [(even? ?e)] [(even? ?e2)]]]'),
    ('[:find ?p :in $ % ?fn :where (rule ?p ?fn "a") (rule ?p ?fn "b")]', '[[1 :attr "a"]]', '[[(rule ?p ?fn ?x) [?p :attr ?x] [(?fn ?x)]]]', '#f always'),
    ('[:find ?p :in $ % ?fn :where (rule ?p ?fn "a")]', '[[1 :attr "a"]]', '[[(rule ?p ?fn ?x) [?p :attr ?x] [(?fn ?x)]]]', '#f always'),
    ('[:find ?n :in $sexes $ages % :where ($sexes male ?n) ($ages adult ?n)]', '[["Ivan" :male] ["Darya" :female] ["Oleg" :male] ["Igor" :male]]', '[["Ivan" 15] ["Oleg" 66] ["Darya" 32]]',
     '[[(male ?x) [?x :male]] [(adult ?y) [?y ?a] [(>= ?a 18)]]]'),
    ('[:find ?x :in $ % :where (wat ?x)]', '[]', '[]'),
    ('[:find ?x :in $ :where (wat ?x)]', '[]'),
    ('[:find ?x :in $ % :where (wat ?x)]', '[]', '[[(rule ?x) [?x]]]'),
    ('[:find ?x :in $ % :where [foo ?x]]', '[]', '[]'),
    ('[:find ?e :in $ % :where [?e]]', '[]', '[[(rule $e1 ?e2) [?e1 :ref ?e2]]]'),
    # rules as a string, as a JavaScript caller passes them
    ('[:find ?e1 ?e2 :in $ % :where (follow ?e1 ?e2)]', FOLLOW, '"[[(follow ?x ?y) [?x :follow ?y]]]"'),
    # the same rule called with the same variable twice, with constants, with nothing bound
    ('[:find ?x :in $ % :where (follow ?x ?x)]', '[[1 :follow 2] [2 :follow 2] [3 :follow 3]]', '[[(follow ?a ?b) [?a :follow ?b]]]'),
    ('[:find ?x :in $ % :where (follow ?x 3)]', FOLLOW, '[[(follow ?a ?b) [?a :follow ?b]]]'),
    ('[:find ?x :in $ % :where (follow 2 ?x)]', FOLLOW, '[[(follow ?a ?b) [?a :follow ?b]]]'),
    ('[:find ?x :in $ % :where [?x :follow] (follow 2 3)]', FOLLOW, '[[(follow ?a ?b) [?a :follow ?b]]]'),
    ('[:find ?x :in $ % :where [?x :follow] (follow 3 2)]', FOLLOW, '[[(follow ?a ?b) [?a :follow ?b]]]'),
    ('[:find ?x ?y :in $ % :where (follow ?x _) (follow _ ?y)]', FOLLOW, '[[(follow ?a ?b) [?a :follow ?b]]]'),
    ('[:find ?x ?y :in $ % :where (follow ?x nil)]', FOLLOW, '[[(follow ?a ?b) [?a :follow ?b]]]'),
    ('[:find ?x ?y :in $ % :where (path ?x ?y)]', FOLLOW, '[[(path ?a ?b) [?a :follow ?b]] [(path ?a ?b) [?a :follow ?c] (path ?c ?b)]]'),
    ('[:find ?x ?y :in $ % :where (path ?x ?y)]', FOLLOW, '[[(path ?a ?b) [?a :follow ?c] (path ?c ?b)] [(path ?a ?b) [?a :follow ?b]]]'),
    ('[:find ?x ?y :in $ % :where (path ?x ?y) [(< ?x ?y)]]', '[[1 :follow 2] [2 :follow 3] [3 :follow 1] [3 :follow 4]]', '[[(path ?a ?b) [?a :follow ?b]] [(path ?a ?b) [?a :follow ?c] (path ?c ?b)]]'),
    ('[:find ?x ?y :in $ % :where [?x :follow] (path ?x ?y)]', FOLLOW, '[[(path ?a ?b) [?a :follow ?b]] [(path ?a ?b) [?a :follow ?c] (path ?c ?b)]]'),
    ('[:find ?y :in $ % ?x :where (path ?x ?y)]', FOLLOW, '[[(path ?a ?b) [?a :follow ?b]] [(path ?a ?b) [?a :follow ?c] (path ?c ?b)]]', '1'),
    ('[:find ?x :in $ % ?y :where (path ?x ?y)]', FOLLOW, '[[(path ?a ?b) [?a :follow ?b]] [(path ?a ?b) [?a :follow ?c] (path ?c ?b)]]', '6'),
    ('[:find ?x ?y :in $ % :where (sib ?x ?y)]', FOLLOW, '[[(sib ?a ?b) [?a :follow ?c] [?b :follow ?c] [(!= ?a ?b)]]]'),
    ('[:find ?x ?y :in $ % :where (r ?x ?y)]', FOLLOW, '[[(r ?a ?b) (or [?a :follow ?b] [?b :follow ?a])]]'),
    ('[:find ?x :in $ % :where (r ?x)]', FOLLOW, '[[(r ?a) [?a :follow] (not [?a :follow 3])]]'),
    ('[:find ?x :in $ % :where (r ?x)]', FOLLOW, '[[(r ?a) [?a :follow] (not (s ?a))] [(s ?a) [?a :follow 3]]]'),
    ('[:find ?x :in $ % :where (r ?x)]', FOLLOW, '[[(r ?a) (or (s ?a) (t ?a))] [(s ?a) [?a :follow 3]] [(t ?a) [?a :follow 6]]]'),
    ('[:find ?x ?y :in $ % :where (r ?x ?y)]', FOLLOW, '[[(r [?a] ?b) [?a :follow ?b]]]'),
    ('[:find ?x ?y :in $ % :where [?x :follow] (r ?x ?y)]', FOLLOW, '[[(r [?a] ?b) [?a :follow ?b]]]'),
    ('[:find ?x ?y :in $ % :where (r ?x ?y)]', FOLLOW, '[[(r ?a ?b) [?a :follow ?b]] [(r ?a) [?a :follow]]]'),
    ('[:find ?x ?y :in $ % :where (r ?x ?y ?z)]', FOLLOW, '[[(r ?a ?b) [?a :follow ?b]]]'),
    ('[:find ?x :in $ % :where (r ?x)]', FOLLOW, '[[(r ?a ?b) [?a :follow ?b]]]'),
    ('[:find ?x ?y :in $ % :where (r ?x ?y)]', FOLLOW, '[[(r ?a ?b) [?a :follow ?c]]]'),
    ('[:find ?x ?y :in $ % :where (r ?x ?y)]', FOLLOW, '[[(r ?a ?b) [(inc ?a) ?b]]]'),
    ('[:find ?x ?y :in $ % :where [?x :follow] (r ?x ?y)]', FOLLOW, '[[(r ?a ?b) [(inc ?a) ?b]]]'),
    ('[:find ?x ?y :in $ % :where (r ?x ?y)]', FOLLOW, '[[(r ?a ?b) [?a :follow ?b] [(> ?b ?unbound)]]]'),
    ('[:find ?x ?y :in $ % :where (r ?x ?y)]', FOLLOW, '[]'),
    ('[:find ?x ?y :in $ % :where (r ?x ?y)]', FOLLOW, 'nil'),
    ('[:find ?x ?y :in $ % :where (r ?x ?y)]', FOLLOW, '[[(r ?a ?b)]]'),
    ('[:find ?x ?y :in $ % :where (r ?x ?y)]', FOLLOW, '[[r ?a ?b]]'),
    ('[:find ?x ?y :in $ % :where (r ?x ?y)]', FOLLOW, '5'),
    ('[:find ?x ?y :in $ % :where (r ?x ?y)]', FOLLOW, '"[[(r ?a ?b) [?a :follow ?b]"'),
])
with_db('nil', '[[:db/add 1 :attr true] [:db/add 2 :attr false]]', [
    ('[:find ?id :in $ % :where (is ?id true)]', DB, '[[(is ?id ?val) [?id :attr ?val]]]'),
    ('[:find ?id :in $ % :where (is ?id false)]', DB, '[[(is ?id ?val) [?id :attr ?val]]]'),
    ('[:find ?id ?v :in $ % :where (is ?id ?v)]', DB, '[[(is ?id ?val) [?id :attr ?val]]]'),
    ('[:find ?id :in $ ?v :where [?id :attr ?v]]', DB, 'false'),
    ('[:find ?id :in $ ?v :where [?id :attr ?v]]', DB, 'true'),
    '[:find ?id :where [?id :attr false]]',
    '[:find ?id :where [?id :attr true]]',
    '[:find ?id ?v :where [?id :attr ?v] [(false? ?v)]]',
])
with_db('nil', '[' + ' '.join('{:db/id %d :item/id %d :item/status "%s"}' % (i, i, ['started', 'pending', 'stopped'][(i * 7) % 3]) for i in range(1, 61)) + ']', [
    '[:find ?e :where [?e :item/status ?status] [(ground "pending") ?status]]',
    ('[:find ?e :in $ % :where [?e :item/status ?status] (pending? ?status)]', DB, '[[(pending? ?status) [(ground "pending") ?status]]]'),
    ('[:find ?e ?status :in $ % :where [?e :item/status ?status] (not-pending? ?status)]', DB, '[[(not-pending? ?status) [(!= "pending" ?status)]]]'),
    ('[:find ?status (count ?e) :in $ % :where (st ?e ?status)]', DB, '[[(st ?e ?s) [?e :item/status ?s]]]'),
])

# ---------------------------------------------------------------- query_not.cljc, query_or.cljc
NOT_DB = '[{:db/id 1 :name "Ivan" :age 10} {:db/id 2 :name "Ivan" :age 20} {:db/id 3 :name "Oleg" :age 10} {:db/id 4 :name "Oleg" :age 20} {:db/id 5 :name "Ivan" :age 10} {:db/id 6 :name "Ivan" :age 20}]'
with_db('nil', NOT_DB, ['[:find [?e ...] :where %s]' % w for w in [
    '[?e :name] (not [?e :name "Ivan"])',
    '[?e :name] (not [?e :name "Ivan"] [?e :age 10])',
    '[?e :name] (not [?e :name "Ivan"]) (not [?e :age 10])',
    '[?e :name] (not [?e :age])',
    '[?e :name "Ivan"] (not [?e :name "Oleg"])',
    '[?e :name] (not [?e :name "Ivan"] [?e :name "Oleg"])',
    '[?e :name] (not [?e :name "Ivan"] (not [?e :age 10]))',
    '[?e :name ?a] (not [?e :age ?f] [?e :age 10])',
]] + ['[:find ?e ?a :where %s]' % w for w in [
    '[?e :name] [?e :age ?a] (not-join [?e] [?e :name "Oleg"] [?e :age ?a])',
    '[?e :age ?a] [?e :age 10] (not-join [?e] [?e :name "Oleg"] [?e :age ?a] [?e :age 10])',
    '[?e :age ?a] (not-join [?a] [?e :name "Petr"] [?e :age ?a])',
    '[?e :age ?a] (not-join [?e ?a] [?e :name "Oleg"])',
    '[?e :age ?a] (not-join [?a ?e] [?e :name "Oleg"])',
    '[?e :age ?a] (not-join [?e ?e] [?e :name "Oleg"])',
    '[?e :age ?a] (not-join [?e] [?e :name ?n] [(= ?n "Oleg")])',
    '[?e :age ?a] (not-join [?x] [?e :name "Oleg"])',
    '[?e :age ?a] (not-join [] [?e :name "Oleg"])',
    '[?e :age ?a] (not-join [?e])',
    '[?e :age ?a] (not)',
    '[?e :age ?a] (not-join [?e ?a] [(> ?a 10)])',
    '[?e :age ?a] (not [(> ?a 10)])',
    '[?e :age ?a] (not [?e :name "Ivan"]) (not [(> ?a 10)])',
    '[?e :age ?a] [?e :name ?n] (not [?e :name "Ivan"]) [(> ?a 10)]',
    '[?e :age ?a] (not [?e2 :name "Ivan"] [?e2 :age ?a])',
    '[?e :age ?a] (not [?e2 :name "Petr"] [?e2 :age ?a])',
    '[?e :age ?a] (not [?e :name ?n] [?e2 :name ?n] [(< ?e ?e2)])',
]] + [
    '[:find ?e :where [?e :name "Oleg"] [?e :age 10] (not [?e :age 20])]',
    '[:find ?e :where [?e :name "Oleg"] [?e :age 10] (not [?e :age 10])]',
    '[:find ?e :where [?e :name "Oleg"] (not [?e :age 10])]',
    '[:find ?e ?e2 :where [?e :name "Ivan"] [?e2 :name "Ivan"] (not [?e :age 10] [?e2 :age 20])]',
    '[:find ?e ?e2 :where [?e :name "Ivan"] [?e2 :name "Oleg"] (not [?e :age 10] [?e2 :age 20])]',
    '[:find ?e ?e2 :where [?e :name "Oleg"] [?e2 :name "Oleg"] (not [?e :age 10] [?e2 :age 20])]',
    '[:find ?e :where (not [?e :name "Ivan"]) [?e :name]]',
    '[:find ?e :where [?e :name] (not-join [?e] (not [1 :age ?a]) [?e :age ?a])]',
    '[:find ?e :where [?e :name] (not [?a :name "Ivan"])]',
    '[:find ?e :where [?e :name] (not-join [?a] [?a :name "Ivan"])]',
    '[:find ?e :where [?e :name] (not-join [?e ?a] [?a :name "Ivan"])]',
    '[:find ?e :where (not-join [?e] [?e :name "Ivan"])]',
] + ['[:find ?e :where %s]' % w for w in [
    '(or [?e :name "Oleg"] [?e :age 10])',
    '(or [?e :name "Oleg"] [?e :age 30])',
    '(or [?e :name "Petr"] [?e :age 30])',
    '[?e :name "Ivan"] (or [?e :name "Oleg"] [?e :age 10])',
    '[?e :age ?a] (or (and [?e :name "Ivan"] [1 :age ?a]) (and [?e :name "Oleg"] [2 :age ?a]))',
    '(or (and [?e :name "Ivan"] [1 :age ?a]) (and [?e :name "Oleg"] [2 :age ?a])) [?e :age ?a]',
    '(or (and [?e :name "Ivan"] [1 :age ?a]) (and [2 :age ?a] [?e :name "Oleg"])) [?e :age ?a]',
    '(or (and [?e :age 30] [?e :name ?n]) (and [?e :age 20] [?e :name ?n])) [(ground "Ivan") ?n]',
    '(or-join [?e] [?e :name ?n] (and [?e :age ?a] [?e :name ?n]))',
    '[?e :name ?a] [?e2 :name ?a] (or-join [?e] (and [?e :age ?a] [?e2 :age ?a]))',
    '(or-join [?e ?n] (and [?e :age 30] [?e :name ?n]) (and [?e :age 20] [?e :name ?n])) [(ground "Ivan") ?n]',
    '(or [?e :name _] [?e :age ?a])',
    '(or-join [[?e]] [?e :name "Ivan"])',
    '(or [?e :name "Oleg"])',
    '(or [?e :name "Oleg"] [?e :name "Oleg"])',
    '(or [?e :name "Oleg"] [?e :name "Ivan"] [?e :age 10])',
    '(or [?e :age 10] [?e :name "Ivan"] [?e :name "Oleg"])',
    '(or (and [?e :name "Oleg"] [?e :age 10]) (and [?e :name "Ivan"] [?e :age 20]))',
    '(or (and [?e :name "Oleg"]) (and [?e :age 20]))',
    '(or (not [?e :name "Oleg"]) [?e :age 20])',
    '[?e :name] (or (not [?e :name "Oleg"]) [?e :age 20])',
    '[?e :name] (or [(> ?e 4)] [?e :age 10])',
    '[?e :name] (or [(> ?e 4)] [(< ?e 2)])',
    '[?e :name] (or-join [?e] [(> ?e 4)] [(< ?e 2)])',
    '[?e :name] (not (or [(> ?e 4)] [(< ?e 2)]))',
    '[?e :name] (or (or [?e :age 10]) (or [?e :name "Oleg"]))',
    '(or-join [?e] [?e :name "Oleg"] [?e :age 10])',
    '(or-join [?e] (and [?e :name ?n] [(= ?n "Oleg")]) [?e :age 10])',
    '(or-join [?e ?x] [?e :name "Oleg"] [?e :age 10])',
    '(or-join [?x] [?e :name "Oleg"] [?e :age 10])',
    '[?e :name] (or-join [?x] [?e :name "Oleg"] [?e :age 10])',
    '[?e :name ?n] (or-join [?e ?n] [?e :age 10] [(= ?n "Oleg")])',
    '[?e :name ?n] (or-join [?e] [?e :age 10] [(= ?n "Oleg")])',
    '[?e :name ?n] (or-join [?n ?e] [?e :age 10] [?e :name "Oleg"])',
    '[?e :name ?n] (or-join [[?e] ?x] [?x :age ?e] [?e :age ?x])',
    '[?e :name ?n] (or-join [[?n ?e]] [?e :age 10] [?e :name "Oleg"])',
    '[?e :name ?n] (or-join [[?n ?zz] ?e] [?e :age 10] [?e :name "Oleg"])',
    '(or-join [?e ?a] [?e :age ?a] (and [?e :name ?n] [(count ?n) ?a]))',
    '(or-join [?a ?e] [?e :age ?a] (and [?e :name ?n] [(count ?n) ?a]))',
    '(or [?e :age 10] (and [?e :name ?n] [(count ?n) ?a] [(> ?a 3)]))',
    '(or (and [?e :age ?a] [(> ?a 10)]) (and [?e :name ?n] [(count ?n) ?a] [(> ?a 3)]))',
    '(or (and [?e :age ?a] [?e :name ?n]) (and [?e :name ?n] [?e :age ?a]))',
    '(and [?e :age 10] [?e :name "Ivan"])',
    '(and [?e :age 10])',
    '(or)',
    '(and)',
    '(or-join [?e])',
    '[?e :name] (or-join [?e] [?e :age 10] [?e :nope])',
    '[?e :name] (or [?e :age 10] [?e :nope])',
    '[?e :name] (or [?e :nope] [?e :age 10])',
    '[?e :name] (or [?e :nope] [?e :nah])',
]] + [
    ('[:find ?e :in $ ?a :where (or [?e :age ?a] [?e :name "Oleg"])]', DB, '10'),
    ('[:find ?e :in $ ?a :where (or-join [?e ?a] [?e :age ?a] [?e :name "Oleg"])]', DB, '10'),
    ('[:find ?e :in $ ?a :where (or-join [[?a] ?e] [?e :age ?a] [?e :name "Oleg"])]', DB, '10'),
    ('[:find ?e :in $ [?a ...] :where (or [?e :age ?a] [?e :name "Oleg"])]', DB, '[10 20]'),
    ('[:find ?e ?a :in $ [?a ...] :where (or-join [?e ?a] [?e :age ?a] [?e :name "Oleg"])]', DB, '[10 20]'),
    ('[:find ?e ?a :in $ [?a ...] :where (or-join [[?a] ?e] [?e :age ?a] (and [?e :name "Oleg"] [(> ?a 10)]))]', DB, '[10 20]'),
])
plain([
    ('[:find ?a ?b ?c :in $xs $ys :where [$xs ?a ?b ?c] (or-join [?a] [$ys ?a ?b ?d])]', '[[:a1 :b1 :c1] [:a2 :b2 :c2] [:a3 :b3 :c3]]', '[[:a1 :b1 :d1] [:a2 :b2* :d2] [:a4 :b4 :c4]]'),
    ('[:find ?a ?c :in $xs $ys :where (or-join [?a ?c] [$xs ?a ?b ?c] [$ys ?a ?c])]', '[[:a1 :b1 :c1]]', '[[:a2 :c2]]'),
    ('[:find ?a ?c :in $xs $ys :where (or-join [?c ?a] [$ys ?a ?c] [$xs ?a ?b ?c])]', '[[:a1 :b1 :c1]]', '[[:a2 :c2]]'),
    ('[:find ?a ?c :in $xs $ys :where (or [$xs ?a ?c] [$ys ?c ?a])]', '[[1 2] [3 4]]', '[[5 6] [7 8]]'),
    ('[:find ?a ?c :in $xs $ys :where (or [$xs ?a ?c] (and [$ys ?c ?b] [$ys ?b ?a]))]', '[[1 2] [3 4]]', '[[5 6] [6 8]]'),
    ('[:find ?a :in $xs $ys :where ($xs or [?a 1] [?a 2])]', '[[:x 1] [:y 2] [:z 3]]', '[[:p 1]]'),
    ('[:find ?a :in $xs $ys :where ($ys or [?a 1] [$xs ?a 2])]', '[[:x 1] [:y 2] [:z 3]]', '[[:p 1]]'),
    ('[:find ?a :in $xs $ys :where [$xs ?a] ($ys not [?a 1])]', '[[:x 1] [:y 2] [:p 3]]', '[[:p 1]]'),
    ('[:find ?a :in $xs $ys :where [$xs ?a] ($ys not-join [?a] [?a 1])]', '[[:x 1] [:y 2] [:p 3]]', '[[:p 1]]'),
    ('[:find ?a :in $xs $ys :where [$xs ?a] ($ys or-join [?a] [?a 1])]', '[[:x 1] [:y 2] [:p 3]]', '[[:p 1]]'),
    ('[:find ?a :in $xs $ys :where ($xs and [?a 1])]', '[[:x 1] [:y 2] [:p 3]]', '[[:p 1]]'),
])
TWO = setup('nil', '[[:db/add 1 :name "Ivan"] [:db/add 2 :name "Oleg"]]', 'db1') + setup('nil', '[[:db/add 1 :age 10] [:db/add 2 :age 20]]', 'db2')
case(TWO + [q('[:find [?e ...] :in $ $2 :where %s]' % w, '#r db1', '#r db2') for w in [
    '[?e :name] (not [?e :name "Ivan"])', '[?e :name] (not [$2 ?e :age 10])', '[?e :name] ($2 not [?e :age 10])', '[?e :name] ($2 not [$ ?e :name "Ivan"])',
    '[?e :name] ($2 not (not [?e :age 10]))', '[?e :name] ($2 not ($ not [?e :name "Ivan"]))',
    '[?e :name] (or [?e :name "Ivan"])', '[?e :name] (or [$2 ?e :age 10])', '[?e :name] ($2 or [?e :age 10])', '[?e :name] ($2 or [$ ?e :name "Ivan"])',
    '[?e :name] ($2 or (or [?e :age 10]))', '[?e :name] ($2 or ($ or [?e :name "Ivan"]))', '[$2 ?e :age ?a] [?e :name ?n]', '[?e :name ?n] [$2 ?e :age ?a]',
    '[?e :name] ($2 not-join [?e] [?e :age 10])', '[?e :name] ($2 or-join [?e] [?e :age 10])', '[?e :name] ($3 not [?e :age 10])', '[$3 ?e :name]']]
     + [q('[:find ?e ?n ?a :in $ $2 :where [?e :name ?n] [$2 ?e :age ?a]]', '#r db1', '#r db2'),
        q('[:find ?e ?n ?a :in $2 $ :where [?e :name ?n] [$2 ?e :age ?a]]', '#r db1', '#r db2'),
        q('[:find ?e ?n ?a :in $a $b :where [$a ?e :name ?n] [$b ?e :age ?a]]', '#r db1', '#r db2'),
        q('[:find ?e ?n :in $a $b :where [?e :name ?n]]', '#r db1', '#r db2'),
        q('[:find ?e (pull $b ?e [*]) (pull $a ?e [*]) :in $a $b :where [$a ?e :name ?n]]', '#r db1', '#r db2'),
        q('[:find ?e (pull ?e [*]) :in $a $b :where [$a ?e :name ?n]]', '#r db1', '#r db2'),
        q('[:find ?e (pull $c ?e [*]) :in $a $b :where [$a ?e :name ?n]]', '#r db1', '#r db2')])
with_db('{:parent {:db/valueType :db.type/ref}}', '[{:db/id "Ivan" :name "Ivan"} {:db/id "Oleg" :name "Oleg" :parent "Ivan"} {:db/id "Petr" :name "Petr" :parent "Oleg"}]', [
    ('[:find ?name ?x ?y :in $ ?name :where [?x :name ?name] (or-join [?x ?y] (and [?x :parent ?z] [?z :parent ?y]) [?y :parent ?x])]', DB, '"Ivan"'),
    ('[:find ?name ?x ?y :in $ ?name :where [?x :name ?name] (or-join [?x ?y] (and [?x :parent ?z] [?z :parent ?y]) [?x :parent ?y])]', DB, '"Ivan"'),
    ('[:find ?name ?x ?y :in $ ?name :where [?x :name ?name] (or-join [?x ?y] (and [?x :parent ?z] [?z :parent ?y]) [?x :parent ?y])]', DB, '"Petr"'),
    ('[:find ?x ?y :in $ % :where (anc ?x ?y)]', DB, '[[(anc ?a ?b) [?a :parent ?b]] [(anc ?a ?b) [?a :parent ?c] (anc ?c ?b)]]'),
    ('[:find ?n1 ?n2 :in $ % :where (anc ?x ?y) [?x :name ?n1] [?y :name ?n2]]', DB, '[[(anc ?a ?b) [?a :parent ?b]] [(anc ?a ?b) [?a :parent ?c] (anc ?c ?b)]]'),
])

# ---------------------------------------------------------------- query_pull.cljc
PULL_DB = '[{:db/id 1 :name "Petr" :age 44} {:db/id 2 :name "Ivan" :age 25} {:db/id 3 :name "Oleg" :age 11}]'
with_db('nil', PULL_DB, ['{:find %s :where [[?e :age ?a] [(>= ?a 18)]]}' % f for f in
                          ['[(pull ?e [:name])]', '[(pull ?e [*])]', '[?e (pull ?e [:name])]', '[?e ?a (pull ?e [:name])]', '[?e (pull ?e [:name]) ?a]', '[(pull ?e [:name]) (pull ?e [:age])]',
                           '[(pull ?e [:nope])]', '[(pull ?a [:name])]', '[(pull ?e [:name :age :db/id])]', '[(pull ?e [[:name :as :n] [:nope :default 0]])]']]
        + [('{:find %s :in [$ ?pattern] :where [[?e :age ?a] [(>= ?a 18)]]}' % f, DB, p) for (f, p) in
           [('[(pull ?e ?pattern)]', '[:name]'), ('[?e ?a ?pattern (pull ?e ?pattern)]', '[:name]'), ('[(pull ?e ?pattern)]', '[*]'), ('[(pull ?e ?pattern)]', ':name'), ('[(pull ?e ?pattern)]', 'nil')]]
        + ['[:find (pull ?e [:name]) . :where [?e :age 25]]', '[:find [(pull ?e [:name]) ...] :where [?e :age ?a]]', '[:find [?e (pull ?e [:name])] :where [?e :age 25]]',
           ('[:find (pull ?e ?p) . :in $ ?p :where [(ground 2) ?e]]', DB, '[:name]'), ('[:find (pull ?e p) . :in $ p :where [(ground 2) ?e]]', DB, '[:name]'),
           '[:find (pull ?e [:name]) . :where [?e :age 99]]', '[:find [(pull ?e [:name]) ...] :where [?e :age 99]]', '[:find (pull ?e [:bad :syntax {}]) :where [?e :age 99]]',
           '[:find (pull ?e [[:name :limit 1]]) :where [?e :age 99]]', '[:find (pull ?e [[:name :limit 1]]) :where [?e :age 25]]', '[:find (pull ?e 5) :where [?e :age 25]]',
           '[:find (pull ?e [:name]) :where [(ground 99) ?e]]', '[:find ?e (pull ?e [:name]) :where [(ground [1 99 2]) [?e ...]]]', '[:find (pull ?e [:name]) :where [(ground "str") ?e]]',
           '[:find (pull ?e [:name]) :where [(ground nil) ?e]]', '[:find (pull ?e [:name]) (count ?a) :where [?e :age ?a]]', '[:find (count ?e) (pull ?a [:name]) :where [?e :age ?a]]',
           '[:find (pull $ ?e [:name]) :where [?e :age 25]]', '[:find (pull $x ?e [:name]) :where [?e :age 25]]'])
with_db('{:value {:db/cardinality :db.cardinality/many}}', '[{:db/id 1 :name "Petr" :value [10 20 30 40]} {:db/id 2 :name "Ivan" :value [14 16]} {:db/id 3 :name "Oleg" :value 1}]', [
    '[:find ?e (pull ?e [:name]) (min ?v) (max ?v) :where [?e :value ?v]]',
    '[:find ?e (pull ?e [:name :value]) (count ?v) :where [?e :value ?v]]',
    '[:find ?e (sum ?v) :where [?e :value ?v]]',
    '[:find (sum ?v) :where [?e :value ?v]]',
    '[:find (sum ?v) :with ?e :where [?e :value ?v]]',
])
with_db('{:name {:db/unique :db.unique/identity}}', PULL_DB, [
    ('[:find ?ref ?a (pull ?ref [:db/id :name]) :in $ [?ref ...] :where [?ref :age ?a] [(>= ?a 18)]]', DB, '[[:name "Ivan"] [:name "Oleg"] [:name "Petr"]]'),
    ('[:find ?ref (pull ?ref [:db/id :name]) :in $ [?ref ...]]', DB, '[[:name "Ivan"] [:name "Nobody"] 3 :kw]'),
])

# ---------------------------------------------------------------- lookup refs in queries (lookup_refs.cljc)
LR = '{:name {:db/unique :db.unique/identity} :friend {:db/valueType :db.type/ref}}'
LR_TX = '[{:db/id 1 :id 1 :name "Ivan" :age 11 :friend 2} {:db/id 2 :id 2 :name "Petr" :age 22 :friend 3} {:db/id 3 :id 3 :name "Oleg" :age 33}]'
steps = setup(LR, LR_TX) + setup(LR, '[{:db/id 3 :name "Ivan" :id 3} {:db/id 1 :name "Petr" :id 1} {:db/id 2 :name "Oleg" :id 2}]', 'db2')
steps += [q(x[0], *x[1:]) for x in [
    ('[:find ?e ?v :in $ ?e :where [?e :age ?v]]', DB, '[:name "Ivan"]'),
    ('[:find [?v ...] :in $ [?e ...] :where [?e :age ?v]]', DB, '[[:name "Ivan"] [:name "Petr"]]'),
    ('[:find [?e ...] :in $ ?v :where [?e :friend ?v]]', DB, '[:name "Petr"]'),
    ('[:find [?e ...] :in $ [?v ...] :where [?e :friend ?v]]', DB, '[[:name "Petr"] [:name "Oleg"]]'),
    ('[:find ?e ?v :in $ ?e ?v :where [?e :friend ?v]]', DB, '[:name "Ivan"]', '[:name "Petr"]'),
    ('[:find ?e ?v :in $ [?e ...] [?v ...] :where [?e :friend ?v]]', DB, '[[:name "Ivan"] [:name "Petr"] [:name "Oleg"]]', '[[:name "Ivan"] [:name "Petr"] [:name "Oleg"]]'),
    ('[:find ?e :in $ [?e ...] :where [?e :friend 3]]', DB, '[1 2 3 "A"]'),
    ('[:find ?e :in $ [?e ...] :where [?e :friend 3]]', DB, '["A"]'),
    ('[:find ?e :in $ [?e ...] :where [?e :friend 3]]', DB, '[:kw 2]'),
    ('[:find ?e :in $ [?e ...] :where [?e :friend 3]]', DB, '[:kw]'),
    ('[:find ?e ?e1 ?e2 :in $1 $2 [?e ...] :where [$1 ?e :id ?e1] [$2 ?e :id ?e2]]', DB, '#r db2', '[[:name "Ivan"] [:name "Petr"] [:name "Oleg"]]'),
    ('[:find ?v :where [[:name "Ivan"] :friend ?v]]', DB),
    ('[:find ?e :where [?e :friend [:name "Petr"]]]', DB),
    ('[:find ?e :where [[:name "Valery"] :friend ?e]]', DB),
    ('[:find ?e :where [?e :friend [:name "Valery"]]]', DB),
    ('[:find ?e :in $ ?e :where [?e :friend]]', DB, '[:name "Valery"]'),
    ('[:find ?e :in $ [?e ...] :where [?e :friend]]', DB, '[[:name "Valery"] [:name "Ivan"]]'),
    ('[:find ?e :in $ [?e ...] :where [?e :friend]]', DB, '[[:name "Valery"]]'),
    ('[:find ?e :in $ [?e ...] :where [?e :friend]]', DB, '[[:age 11] [:name "Ivan"]]'),
    ('[:find ?e :in $ [?e ...] :where [?e :friend]]', DB, '[[:name] [:name "Ivan"]]'),
    ('[:find ?e ?v :in $ ?v :where [?e :age ?v]]', DB, '[:name "Ivan"]'),
    ('[:find ?e ?v :in $ ?v :where [?e :name ?v]]', DB, '[:name "Ivan"]'),
    ('[:find ?e ?f :in $ [?f ...] :where [?e :friend ?f]]', DB, '[2 [:name "Oleg"] [:name "Nobody"] 99]'),
    ('[:find ?e ?f :in $ [[?e ?f]] :where [?e :friend ?f]]', DB, '[[1 2] [[:name "Petr"] [:name "Oleg"]] [[:name "Oleg"] 1]]'),
    ('[:find ?e ?tx :in $ [?tx ...] :where [?e :friend _ ?tx]]', DB, '[536870913 [:name "Ivan"]]'),
    ('[:find ?e :where [?e :friend _ [:name "Ivan"]]]', DB),
    ('[:find ?e :where [?e :age 11 [:name "Ivan"]]]', DB),
    ('[:find ?a ?v :where [:name ?a ?v]]', DB),
    ('[:find ?a ?v :where ["name" ?a ?v]]', DB),
    ('[:find ?e :where [?e :friend :ident]]', DB),
    ('[:find ?e :where [?e :age :ident]]', DB),
    ('[:find ?e :where [?e :age [:name "Ivan"]]]', DB),
    ('[:find ?e ?f :where [?e :friend ?f] [?f :name "Petr"]]', DB),
    ('[:find ?e ?n :where [?e :friend ?f] [?f :name ?n]]', DB),
    ('[:find ?e ?n :where [?f :name ?n] [?e :friend ?f]]', DB),
    ('[:find ?n1 ?n2 :where [?e :friend ?f] [?e :name ?n1] [?f :name ?n2]]', DB),
]]
case(steps)
with_db('{:db/ident {:db/unique :db.unique/identity} :ref {:db/valueType :db.type/ref}}', '[{:db/id 1 :db/ident :ent1 :ref 2} {:db/id 2 :db/ident :ent2 :ref 1} {:db/id 3 :name "x" :ref 1}]', [
    '[:find ?v :where [:ent1 :ref ?v]]', '[:find ?e :where [?e :ref :ent1]]', '[:find ?e :where [?e :ref :ent2]]', '[:find ?e :where [?e :ref :nope]]', '[:find ?v :where [:nope :ref ?v]]',
    ('[:find ?e ?v :in $ ?e :where [?e :ref ?v]]', DB, ':ent1'), ('[:find ?e ?v :in $ [?e ...] :where [?e :ref ?v]]', DB, '[:ent1 :ent2 3]'), ('[:find ?e ?v :in $ ?v :where [?e :ref ?v]]', DB, ':ent1'),
    ('[:find ?e ?v :in $ [?v ...] :where [?e :ref ?v]]', DB, '[:ent1 :ent2]'), ('[:find ?e ?i :where [?e :db/ident ?i]]', DB), ('[:find ?a ?v :where [[:db/ident :ent1] ?a ?v]]', DB),
])

# ---------------------------------------------------------------- tuples in queries (tuples.cljc)
TUP = '{:a+b {:db/tupleAttrs [:a :b]} :a+c+d {:db/tupleAttrs [:a :c :d] :db/unique :db.unique/identity} :t {:db/valueType :db.type/tuple :db/tupleTypes [:db.type/ref :db.type/string]} :u {:db/valueType :db.type/tuple :db/tupleAttrs [:a :b] :db/index true}}'
with_db(TUP, '[{:db/id 1 :a "A" :b "B"} {:db/id 2 :a "A" :b "b"} {:db/id 3 :a "a" :b "B" :c "c" :d "d"} {:db/id 4 :a "x"}]', [
    '[:find ?e ?v :where [?e :a+b ?v]]', '[:find ?e :where [?e :a+b ["A" "B"]]]', '[:find ?e :where [?e :a+b ["A" nil]]]', '[:find ?e ?a ?b :where [?e :a+b ?t] [(untuple ?t) [?a ?b]]]',
    ('[:find ?e :in $ ?t :where [?e :a+b ?t]]', DB, '["A" "b"]'), ('[:find ?e ?t :in $ [?t ...] :where [?e :a+b ?t]]', DB, '[["A" "b"] ["a" "B"] ["no" "no"]]'),
    '[:find ?e ?t :where [?e :a ?a] [?e :b ?b] [(tuple ?a ?b) ?t]]', '[:find ?e ?t :where [?e :a ?a] [?e :b ?b] [(tuple ?a ?b) ?t] [?e :a+b ?t]]',
    '[:find ?e ?v :where [?e :a+c+d ?v]]', '[:find ?e :where [?e :a+c+d ["a" "c" "d"]]]', '[:find ?v :where [[:a+c+d ["a" "c" "d"]] :a ?v]]', '[:find ?v :where [[:a+c+d ["a" "c" "x"]] :a ?v]]',
    ('[:find ?e ?v :in $ ?e :where [?e :a ?v]]', DB, '[:a+c+d ["a" "c" "d"]]'), '[:find ?e ?a ?v :where [?e ?a ?v]]',
])
with_db('{:ref {:db/valueType :db.type/ref} :name {:db/unique :db.unique/identity} :pair {:db/valueType :db.type/tuple :db/tupleTypes [:db.type/ref :db.type/keyword]} :pairs {:db/valueType :db.type/tuple :db/tupleType :db.type/ref :db/cardinality :db.cardinality/many}}',
        '[{:db/id 1 :name "a"} {:db/id 2 :name "b"} {:db/id 3 :pair [1 :x] :pairs [[1 2] [2 1]]} {:db/id 4 :pair [2 :y]}]', [
    '[:find ?e ?p :where [?e :pair ?p]]', '[:find ?e :where [?e :pair [1 :x]]]', '[:find ?e :where [?e :pair [[:name "a"] :x]]]', '[:find ?e :where [?e :pair [[:name "zz"] :x]]]',
    ('[:find ?e ?p :in $ ?p :where [?e :pair ?p]]', DB, '[[:name "a"] :x]'), ('[:find ?e ?p :in $ [?p ...] :where [?e :pair ?p]]', DB, '[[[:name "a"] :x] [2 :y] [[:name "b"] :y]]'),
    '[:find ?e ?p :where [?e :pairs ?p]]', '[:find ?e :where [?e :pairs [[:name "a"] [:name "b"]]]]', ('[:find ?e ?p :in $ [?p ...] :where [?e :pairs ?p]]', DB, '[[[:name "a"] 2] [2 [:name "a"]] [2 2]]'),
    '[:find ?e :where [?e :pair [1]]]', '[:find ?e :where [?e :pair [1 :x :y]]]', '[:find ?e :where [?e :pair 5]]',
])

# ---------------------------------------------------------------- the errors of the parser
BAD = [
    '[:find ?e :where [?e :name] :in $]', '[:find :where [?e :name]]', '[:where [?e :name]]', '[:find ?e]', '[:find ?e :where]', '[]', '{}', '5', '"[:find ?e :where [?e :name]]"', 'nil', ':find',
    '[:find ?e ?e2 :where [?e :name]]', '[:find ?e :with ?f :where [?e :name]]', '[:find ?e :in $ ?f :where [?e :name]]', '[:find ?e :in $ $ :where [?e :name]]', '[:find ?e :in $ ?e ?e :where [?e :name]]',
    '[:find ?e :with ?e :where [?e :name]]', '[:find ?e :in $ % % :where [?e :name]]', '[:find ?e :where [$2 ?e :name]]', '[:find ?e :where [?e :name] (rule ?e)]', '[:find ?e :in $ :where (rule ?e)]',
    '[:find e :where [?e :name]]', '[:find [?e] ?x :where [?e :name ?x]]', '[:find ?e . ?x :where [?e :name ?x]]', '[:find [?e ...] . :where [?e :name ?x]]', '[:find (count) :where [?e :name ?x]]',
    '[:find (count ?e ?x) :where [?e :name ?x]]', '[:find (?e) :where [?e :name ?x]]', '[:find (pull ?e) :where [?e :name ?x]]', '[:find (pull) :where [?e :name ?x]]', '[:find (pull ?e [:name] :x :y) :where [?e :name ?x]]',
    '[:find (aggregate ?e) :where [?e :name ?x]]', '[:find (aggregate) :where [?e :name ?x]]', '[:find (count 5) :where [?e :name ?x]]', '[:find (count "s" ?e) :where [?e :name ?x]]',
    '[:find ?e :where [?e]]', '[:find ?e :where []]', '[:find ?e :where [[]]]', '[:find ?e :where ?e]', '[:find ?e :where :kw]', '[:find ?e :where [?e :name] 5]', '[:find ?e :where [?e :name] nil]',
    '[:find ?e :where ()]', '[:find ?e :where [()]]', '[:find ?e :where [(pred)]]', '[:find ?e :where [(pred) ?e]]', '[:find ?e :where [(pred ?e) ?e ?x]]', '[:find ?e :where [(5 ?e)]]', '[:find ?e :where [("f" ?e)]]',
    '[:find ?e :where [(:kw ?e)]]', '[:find ?e :where [?e :name] (not)]', '[:find ?e :where [?e :name] (not-join)]', '[:find ?e :where [?e :name] (not-join [])]', '[:find ?e :where [?e :name] (not-join [?e])]',
    '[:find ?e :where [?e :name] (not-join ?e [?e :age])]', '[:find ?e :where [?e :name] (not-join [5] [?e :age])]', '[:find ?e :where [?e :name] (not-join [?x] [?e :age])]', '[:find ?e :where (or)]',
    '[:find ?e :where (or-join)]', '[:find ?e :where (or-join [?e])]', '[:find ?e :where (or-join [] [?e :name])]', '[:find ?e :where (or-join ?e [?e :name])]', '[:find ?e :where (or-join [[]] [?e :name])]',
    '[:find ?e :where (or-join [[?e ?e]] [?e :name])]', '[:find ?e :where (or-join [?e ?e] [?e :name])]', '[:find ?e :where (or-join [[?e] ?e] [?e :name])]', '[:find ?e :where (and)]', '[:find ?e :where (and [?e :name])]',
    '[:find ?e :where (or (and))]', '[:find ?e :where (or [?e :name] (and))]', '[:find ?e :where (or (or-join [?e] [?e :name]))]', '[:find ?e :where (not (and [?e :name]))]', '[:find ?e :where [?e :name] (not [?x :name] (and [?e :age]))]',
    '[:find ?e :in [?e :where [?e :name]]', '[:find ?e :in [] :where [?e :name]]', '[:find ?e :in [[]] :where [?e :name]]', '[:find ?e :in [?e ... ?f] :where [?e :name]]', '[:find ?e :in [... ?e] :where [?e :name]]',
    '[:find ?e :in 5 :where [?e :name]]', '[:find ?e :in :kw :where [?e :name]]', '[:find ?e :in e :where [?e :name]]', '[:find ?e :in [[?e] [?f]] :where [?e :name]]', '[:find ?e :in [?e [?f ...] ...] :where [?e :name]]',
    '[:find ?e :keys :where [?e :name]]', '[:find ?e :keys a b :where [?e :name]]', '[:find ?e :keys 5 :where [?e :name]]', '[:find ?e :keys "a" :where [?e :name]]', '[:find ?e :keys :a :where [?e :name]]',
    '[:find ?e :strs a :syms b :where [?e :name]]', '[:find ?e :foo bar :where [?e :name]]', '[:find ?e :where [?e :name] :foo bar]', '[?e :find ?e :where [?e :name]]', '[:find ?e :find ?x :where [?e :name ?x]]',
    '[:find ?e :where [?e :name] :where [?e :age]]', '{:find ?e :where [[?e :name]]}', '{:find [?e] :where [?e :name]}', '{:find [?e] :where [[?e :name]] :in $}', '{:find [?e] :where [[?e :name]] :in [$ ?x]}',
    '{:find [?e] :where [[?e :name]] :with ?x}', '{:find [?e] :where [[?e :name]] :with [?x]}', '{:find [?e] :where [[?e :name]] :keys [a]}', '{:find [?e] :where [[?e :name]] :keys a}', '{:find [?e ?x] :where [[?e :name ?x]] :strs [a b]}',
    '{:find [?e .] :where [[?e :name]]}', '{:find [[?e ...]] :where [[?e :name]]}', '{:find [[?e ?x]] :where [[?e :name ?x]]}', '{:find [(count ?e)] :where [[?e :name]]}', '{:find [?e] :where [[?e :name]] :extra 1}',
    '[:find ?e :in $ [?x] :where [?e :name ?x] [?x]]', '[:find ?e :where [?e :name ?x ?t ?added ?more]]', '[:find ?e :where [_ _ _ _ _ _]]', '[:find ?e :where [?e :name] [$ ?e :age]]', '[:find ?e :in $a :where [$a ?e :name] [?e :age]]',
    '[:find ?e :where [?e :name] [(ground 1) [?x ?y ...]]]', '[:find ?e :where [?e :name] [(ground 1) []]]', '[:find ?e :where [?e :name] [(ground 1) [[]]]]', '[:find ?e :where [?e :name] [(ground 1) 5]]',
    '[:find ?e :where [?e :name] [(ground 1) :kw]]', '[:find ?e :where [?e :name] [(ground 1) [?x ... ...]]]', '[:find ?e :where [?e :name] [(ground 1) $x]]', '[:find ?e :where [?e :name] [(ground 1) %]]',
    '[:find ?e :where [% :name]]', '[:find ?e :where [?e %]]', '[:find % :where [?e :name]]', '[:find $ :where [?e :name]]', '[:find _ :where [?e :name]]', '[:find ?e :where [?e $x]]', '[:find ?e :where [?e :name $x]]',
]
with_db('nil', JOINS, [(b, DB) for b in BAD])
case(['{:op :parse-query :query %s}' % b for b in BAD])
GOOD = [
    '[:find ?e :where [?e :name]]', '[:find ?e ?n :in $ ?n :where [?e :name ?n]]', '[:find [?e ...] :where [?e :name]]', '[:find ?e . :where [?e :name]]', '[:find [?e ?n] :where [?e :name ?n]]',
    '[:find (count ?e) :where [?e :name]]', '[:find ?n (max 3 ?e) :where [?e :name ?n]]', '[:find (aggregate ?f ?e) :in $ ?f :where [?e :name]]', '[:find (pull ?e [:name]) :where [?e :name]]',
    '[:find (pull $x ?e ?p) :in $x ?p :where [$x ?e :name]]', '[:find ?e :with ?n :where [?e :name ?n]]', '[:find ?e ?n :keys a b :where [?e :name ?n]]', '[:find ?e ?n :strs a b :where [?e :name ?n]]',
    '[:find ?e ?n :syms a b :where [?e :name ?n]]', '[:find ?e :in $ % :where (rule ?e)]', '[:find ?e :in $ $2 % ?x [?y ...] [?z _] [[?p ?q]] :where [?e :name ?x] [$2 ?e ?y ?z] (rule ?e ?p ?q)]',
    '[:find ?e :where [?e :name] (not [?e :age])]', '[:find ?e :where [?e :name] (not-join [?e] [?e :age ?a])]', '[:find ?e :where (or [?e :name] [?e :age])]', '[:find ?e :where (or-join [?e] [?e :name] (and [?e :age ?a] [(> ?a 1)]))]',
    '[:find ?e :where (or-join [[?e] ?x] [?e :name ?x])]', '[:find ?e :where ($ or [?e :name])]', '[:find ?e :where [?e :name] ($ not [?e :age])]', '[:find ?e :where [(pred ?e)]]', '[:find ?e :where [(?f ?e)]]',
    '[:find ?e :where [(f ?e 1 "s" :k $ nil) ?x]]', '[:find ?e :where [(f) [?e ?x]]]', '[:find ?e :where [(f) [[?e ?x]]]]', '[:find ?e :where [(f) [?e ...]]]', '[:find ?e :where [(f) [[?e _] ...]]]', '[:find ?e :where [(f) _]]',
    '[:find ?e :where [?e _ _ _]]', '[:find ?e :where [$ ?e :name "x" 5]]', '[:find ?e :where [?e a b]]', '[:find ?e :where [?e "str" 1.5]]', '[:find ?e :where [?e :name [1 2]]]', '[:find ?e :where [[:name "x"] :ref ?e]]',
    '{:find [?e] :where [[?e :name]]}', '{:find [?e ?n] :in [$ ?n] :with [?x] :keys [a b] :where [[?e :name ?n] [?e :age ?x]]}', '{:find [[?e ...]] :where [[?e :name]]}', '{:find [?e .] :where [[?e :name]]}',
    '[:find ?e :in $ :where [?e :name]]', '[:find ?e :in :where [(ground 1) ?e]]', '[:find ?e :where [?e ?a ?v] [(= ?a :name)]]', '(:find ?e :where [?e :name])', '[:find ?e :where (rule ?e) [?e :name]]',
    '[:find ?a :in [?a ?b] :where [(> ?a ?b)]]', '[:find ?a :in [[?a [?b ?c]] ...] :where [(> ?a ?b ?c)]]', '[:find (min ?n ?e) :in $ ?n :where [?e :name]]', '[:find (distinct ?e) . :where [?e :name]]',
    '[:find [(count ?e) (max ?e)] :where [?e :name]]', '[:find ?e :where ($x r ?e)]', '[:find ?e :where (r ?e 1 "s" :k _)]',
]
case(['{:op :parse-query :query %s}' % g for g in GOOD])

# ---------------------------------------------------------------- order: results larger than a small set
rnd = random.Random(42)
NAMES = ['Ivan', 'Petr', 'Oleg', 'Sergey', 'Anna', 'Maria', 'Olga', 'Dmitry', 'Elena', 'Nikolai', 'Igor', 'Vera']
CITIES = ['Moscow', 'Omsk', 'Perm', 'Kazan', 'Tula']
people = []
for i in range(1, 61):
    friends = sorted(set(rnd.randint(1, 60) for _ in range(rnd.randint(0, 4))))
    p = '{:db/id %d :name "%s" :age %d :city "%s" :email "p%d@x.org" :score %s :tags [%s]' % (
        i, NAMES[rnd.randrange(len(NAMES))], rnd.randint(10, 70), CITIES[rnd.randrange(len(CITIES))], i, rnd.choice(['1.5', '2', '-3', '0', '10.25', '7']),
        ' '.join(':t%d' % t for t in sorted(set(rnd.randrange(6) for _ in range(rnd.randint(0, 3))))))
    if friends:
        p += ' :friend [%s]' % ' '.join(map(str, friends))
    if i > 1 and i % 3 != 0:
        p += ' :boss %d' % rnd.randint(max(1, i - 6), i - 1)
    people.append(p + '}')
BIG_SCHEMA = '{:friend {:db/valueType :db.type/ref :db/cardinality :db.cardinality/many} :boss {:db/valueType :db.type/ref} :email {:db/unique :db.unique/identity} :tags {:db/cardinality :db.cardinality/many} :age {:db/index true}}'
BIG = '[' + ' '.join(people) + ']'
BIG_RULES = '[[(friends ?a ?b) [?a :friend ?b]] [(friends ?a ?b) [?b :friend ?a]] [(reach ?a ?b) [?a :boss ?b]] [(reach ?a ?b) [?a :boss ?c] (reach ?c ?b)] [(old ?e) [?e :age ?a] [(> ?a 50)]] [(same-city ?a ?b) [?a :city ?c] [?b :city ?c] [(!= ?a ?b)]]]'
BIG_QUERIES = [
    '[:find ?e :where [?e :name]]', '[:find ?e ?n :where [?e :name ?n]]', '[:find ?n :where [?e :name ?n]]', '[:find ?n ?c :where [?e :name ?n] [?e :city ?c]]', '[:find ?e ?a :where [?e :age ?a]]',
    '[:find ?a :where [_ :age ?a]]', '[:find ?e ?f :where [?e :friend ?f]]', '[:find ?f ?e :where [?e :friend ?f]]', '[:find ?e ?t :where [?e :tags ?t]]', '[:find ?t :where [_ :tags ?t]]',
    '[:find ?e ?s :where [?e :score ?s]]', '[:find ?s :where [_ :score ?s]]', '[:find ?e ?b :where [?e :boss ?b]]', '[:find ?e ?a ?v :where [?e ?a ?v]]', '[:find ?a :where [_ ?a _]]',
    '[:find ?e ?n ?a ?c :where [?e :name ?n] [?e :age ?a] [?e :city ?c]]', '[:find ?n ?a ?c :where [?e :name ?n] [?e :age ?a] [?e :city ?c]]', '[:find ?c ?a ?n :where [?e :city ?c] [?e :age ?a] [?e :name ?n]]',
    '[:find ?e1 ?e2 :where [?e1 :city ?c] [?e2 :city ?c] [?e1 :name ?n] [?e2 :name ?n]]', '[:find ?e1 ?e2 :where [?e1 :name ?n] [?e2 :name ?n] [?e1 :city ?c] [?e2 :city ?c] [(< ?e1 ?e2)]]',
    '[:find ?n1 ?n2 :where [?e1 :friend ?e2] [?e1 :name ?n1] [?e2 :name ?n2]]', '[:find ?e1 ?e3 :where [?e1 :friend ?e2] [?e2 :friend ?e3]]', '[:find ?e1 ?e2 ?e3 :where [?e1 :friend ?e2] [?e2 :friend ?e3] [?e3 :friend ?e1]]',
    '[:find ?e ?n :where [?e :age ?a] [(> ?a 40)] [?e :name ?n]]', '[:find ?e ?n :where [?e :name ?n] [?e :age ?a] [(> ?a 40)]]', '[:find ?e ?a2 :where [?e :age ?a] [(* ?a 2) ?a2]]',
    '[:find ?e ?d :where [?e :age ?a] [(mod ?a 10) ?d]]', '[:find ?d :where [?e :age ?a] [(mod ?a 10) ?d]]', '[:find ?d (count ?e) :where [?e :age ?a] [(mod ?a 10) ?d]]', '[:find ?c (count ?e) :where [?e :city ?c]]',
    '[:find ?n (count ?e) :where [?e :name ?n]]', '[:find ?n (count ?e) (avg ?a) (min ?a) (max ?a) (sum ?a) :where [?e :name ?n] [?e :age ?a]]', '[:find ?c ?n (count ?e) :where [?e :name ?n] [?e :city ?c]]',
    '[:find ?n (distinct ?c) :where [?e :name ?n] [?e :city ?c]]', '[:find ?c (min 3 ?a) (max 3 ?a) :where [?e :city ?c] [?e :age ?a]]', '[:find (count ?e) . :where [?e :friend]]', '[:find [(count ?e) (count-distinct ?f)] :where [?e :friend ?f]]',
    '[:find (median ?a) (variance ?a) (stddev ?a) :where [?e :age ?a]]', '[:find (sum ?s) (avg ?s) :where [?e :score ?s]]', '[:find (sum ?s) (avg ?s) :with ?e :where [?e :score ?s]]', '[:find ?s (count ?e) :where [?e :score ?s]]',
    '[:find ?t (count ?e) :where [?e :tags ?t]]', '[:find ?t (distinct ?c) :where [?e :tags ?t] [?e :city ?c]]', '[:find ?e (count ?f) :where [?e :friend ?f]]', '[:find ?f (count ?e) :where [?e :friend ?f]]',
    '[:find [?e ...] :where [?e :name]]', '[:find [?n ...] :where [_ :name ?n]]', '[:find ?n . :where [_ :name ?n]]', '[:find [?e ?n ?a] :where [?e :name ?n] [?e :age ?a]]', '[:find [?a ...] :where [_ :age ?a]]',
    '[:find ?n :with ?e :where [?e :name ?n]]', '[:find ?n ?c :with ?e :where [?e :name ?n] [?e :city ?c]]', '[:find ?c :with ?e ?n :where [?e :name ?n] [?e :city ?c]]',
    '[:find ?e ?n :keys id name :where [?e :name ?n]]', '[:find ?n ?c :strs name city :where [?e :name ?n] [?e :city ?c]]', '[:find ?n (count ?e) :keys name n :where [?e :name ?n]]', '[:find ?n ?c :syms name city :with ?e :where [?e :name ?n] [?e :city ?c]]',
    '[:find ?e :where [?e :name "Ivan"]]', '[:find ?e :where [?e :city "Omsk"] [?e :name "Ivan"]]', '[:find ?e :where [?e :age 30]]', '[:find ?e :where [?e :tags :t1]]', '[:find ?e :where [?e :friend 5]]', '[:find ?f :where [5 :friend ?f]]',
    '[:find ?e :where [?e :name] (not [?e :friend])]', '[:find ?e :where [?e :name] (not [?e :tags :t1])]', '[:find ?e :where [?e :name] (not [_ :friend ?e])]', '[:find ?e ?n :where [?e :name ?n] (not [?e :city "Omsk"]) (not [?e :city "Perm"])]',
    '[:find ?e :where [?e :name] (not-join [?e] [?e :friend ?f] [?f :city "Omsk"])]', '[:find ?e :where (or [?e :city "Omsk"] [?e :name "Ivan"])]', '[:find ?e :where (or [?e :city "Omsk"] [?e :name "Ivan"] [?e :tags :t0])]',
    '[:find ?e ?n :where [?e :name ?n] (or [?e :city "Omsk"] [?e :tags :t0])]', '[:find ?e ?f :where (or [?e :friend ?f] [?f :friend ?e])]', '[:find ?e ?f :where (or-join [?e ?f] [?e :friend ?f] (and [?e :boss ?b] [?b :friend ?f]))]',
    '[:find ?e ?c :where [?e :city ?c] (or-join [?e] [?e :boss] (and [?e :age ?a] [(> ?a 60)]))]', '[:find ?e :where [?e :age ?a] (or [(< ?a 15)] [(> ?a 65)])]',
    ('[:find ?a ?b :in $ % :where (friends ?a ?b)]', DB, BIG_RULES), ('[:find ?b :in $ % ?a :where (reach ?a ?b)]', DB, BIG_RULES, '1'), ('[:find ?a :in $ % ?b :where (reach ?a ?b)]', DB, BIG_RULES, '7'),
    ('[:find ?e ?n :in $ % :where (old ?e) [?e :name ?n]]', DB, BIG_RULES), ('[:find ?e ?n :in $ % :where [?e :name ?n] (old ?e)]', DB, BIG_RULES), ('[:find ?a ?b :in $ % :where (same-city ?a ?b) (old ?a) (old ?b)]', DB, BIG_RULES),
    ('[:find ?a ?b :in $ % :where (old ?a) (old ?b) (same-city ?a ?b)]', DB, BIG_RULES), ('[:find ?a (count ?b) :in $ % :where (reach ?a ?b)]', DB, BIG_RULES),
    ('[:find ?a ?b :in $ % :where (reach ?a ?b) (reach ?b ?a) [(< ?a ?b)]]', DB, BIG_RULES), ('[:find ?a ?b :in $ % :where (friends ?a ?b) (not (same-city ?a ?b))]', DB, BIG_RULES),
    ('[:find ?e ?n :in $ [?n ...] :where [?e :name ?n]]', DB, '["Ivan" "Anna" "Vera" "Nobody"]'), ('[:find ?e ?n ?c :in $ [[?n ?c]] :where [?e :name ?n] [?e :city ?c]]', DB, '[["Ivan" "Omsk"] ["Anna" "Perm"] ["Vera" "Tula"] ["Oleg" "Kazan"]]'),
    ('[:find ?e ?lo ?hi :in $ ?lo ?hi :where [?e :age ?a] [(<= ?lo ?a ?hi)]]', DB, '20', '30'), ('[:find ?e :in $ [?e ...] :where [?e :friend]]', DB, '[' + ' '.join(str(i) for i in range(1, 61, 3)) + ']'),
    ('[:find ?e ?m :in $ [?e ...] :where [(missing? $ ?e :friend) ?m]]', DB, '[' + ' '.join(str(i) for i in range(1, 31)) + ']'), ('[:find ?e :in $ [?e ...] :where [?e :name]]', DB, '[' + ' '.join('[:email "p%d@x.org"]' % i for i in range(1, 40, 2)) + ']'),
    ('[:find ?e ?n :in $ [?e ...] :where [?e :name ?n]]', DB, '[' + ' '.join('[:email "p%d@x.org"]' % i for i in range(1, 80, 5)) + ']'),
    '[:find ?e (pull ?e [:name :age]) :where [?e :city "Omsk"]]', '[:find (pull ?e [:name {:friend [:name]}]) :where [?e :city "Tula"]]', '[:find [(pull ?e [:email]) ...] :where [?e :age ?a] [(< ?a 30)]]',
    '[:find ?e ?b (pull ?b [:name :city]) :where [?e :boss ?b]]', '[:find ?n (count ?e) (pull ?e [:name]) :where [?e :name ?n]]',
    '[:find ?e ?bn :where [?e :name] [(get-else $ ?e :boss 0) ?b] [(get-else $ ?b :name "none") ?bn]]', '[:find ?e ?x :where [?e :name] [(get-some $ ?e :boss :score) ?x]]', '[:find ?e :where [?e :name] [(missing? $ ?e :boss)]]',
    '[:find ?e ?n :where [?e :name ?n] [(clojure.string/starts-with? ?n "O")]]', '[:find ?e ?l :where [?e :name ?n] [(count ?n) ?l]]', '[:find ?l (count ?e) :where [?e :name ?n] [(count ?n) ?l]]',
    '[:find ?e ?k :where [?e :name ?n] [(keyword ?n) ?k]]', '[:find ?k :where [?e :name ?n] [?e :city ?c] [(keyword ?c ?n) ?k]]', '[:find ?v :where [?e :name ?n] [?e :age ?a] [(vector ?n ?a) ?v]]',
    '[:find ?m :where [?e :name ?n] [?e :age ?a] [(hash-map :n ?n :a ?a) ?m]]', '[:find ?s :where [?e :name ?n] [?e :city ?c] [(str ?n "@" ?c) ?s]]',
    '[:find ?e ?tx :where [?e :name _ ?tx]]', '[:find ?tx :where [_ _ _ ?tx]]', '[:find ?e ?e2 :where [?e :age ?a] [?e2 :age ?a] [(!= ?e ?e2)]]', '[:find ?a (count ?e) :where [?e :age ?a]]',
    '[:find ?e ?s :where [?e :score ?s] [(> ?s 1.9)]]', '[:find ?e ?s2 :where [?e :score ?s] [(/ ?s 4) ?s2]]', '[:find ?s2 :where [?e :score ?s] [(/ ?s 3) ?s2]]', '[:find ?q ?r :where [?e :age ?a] [(quot ?a 7) ?q] [(rem ?a 7) ?r]]',
]
with_db(BIG_SCHEMA, BIG, BIG_QUERIES)
# the same over a collection of tuples, and over a filtered database
steps = setup(BIG_SCHEMA, BIG) + ['{:op :filter :db #r db :pred #f f-even-e :as even}', '{:op :filter :db #r even :pred #f f-not-name :as noname}', '{:op :filter :db #r db :pred #f f-has-name :as named}']
for src in ['#r even', '#r noname', '#r named']:
    steps += [q(x, src) for x in ['[:find ?e ?n :where [?e :name ?n]]', '[:find ?e ?f :where [?e :friend ?f]]', '[:find ?e ?c :where [?e :city ?c] [?e :age ?a] [(> ?a 30)]]', '[:find ?c (count ?e) :where [?e :city ?c]]',
                                  '[:find ?e :where [?e :city] (not [?e :friend])]', '[:find (pull ?e [:name :city {:friend [:db/id :name]}]) :where [?e :age ?a] [(> ?a 60)]]',
                                  '[:find ?e ?m :where [?e :city] [(missing? $ ?e :name) ?m]]', '[:find ?e ?v :where [?e :city] [(get-else $ ?e :name "?") ?v]]']]
    steps += [q('[:find ?a ?b :in $ % :where (friends ?a ?b)]', src, BIG_RULES), q('[:find ?e ?n :in $ [?e ...] :where [?e :name ?n]]', src, '[[:email "p2@x.org"] [:email "p3@x.org"] 4 5]')]
case(steps)
TUPLES = '[' + ' '.join('[%d :k%d "%s" %d]' % (i, i % 7, 'abcdefghij'[i % 10], (i * 37) % 11) for i in range(80)) + ']'
plain([(x, TUPLES) for x in ['[:find ?a ?b ?c ?d :where [?a ?b ?c ?d]]', '[:find ?a :where [?a]]', '[:find ?b :where [_ ?b]]', '[:find ?c ?d :where [_ _ ?c ?d]]', '[:find ?d (count ?a) :where [?a _ _ ?d]]',
                             '[:find ?a1 ?a2 :where [?a1 ?b ?c] [?a2 ?b ?c] [(< ?a1 ?a2)]]', '[:find ?b ?c :where [?a ?b ?c ?d] [(> ?d 8)]]', '[:find [?c ...] :where [_ :k3 ?c]]', '[:find ?c . :where [_ :k3 ?c]]',
                             '[:find ?a ?d :where [?a :k1 _ ?d] [?d]]', '[:find ?x :where [?x] [?x]]', '[:find ?a ?x :where [?a _ _ ?x] [?x _ _ ?a]]', '[:find ?b (distinct ?c) :where [_ ?b ?c]]', '[:find ?b (min ?d) (max ?d) (sum ?d) :where [_ ?b _ ?d]]']])

# many variables: relations wider than a small map
plain([('[:find ?a ?b ?c ?d ?e ?f ?g ?h ?i ?j :in [[?a ?b ?c ?d ?e ?f ?g ?h ?i ?j]]]', '[[1 2 3 4 5 6 7 8 9 10] [11 12 13 14 15 16 17 18 19 20]]'),
       ('[:find ?j ?a :in [[?a ?b ?c ?d ?e ?f ?g ?h ?i ?j]]]', '[[1 2 3 4 5 6 7 8 9 10] [11 12 13 14 15 16 17 18 19 20]]'),
       ('[:find ?a ?j ?k :in [[?a ?b ?c ?d ?e ?f ?g ?h ?i ?j]] [?k ...]]', '[[1 2 3 4 5 6 7 8 9 10] [11 12 13 14 15 16 17 18 19 20]]', '[:x :y]'),
       ('[:find ?a ?j ?s :in [[?a ?b ?c ?d ?e ?f ?g ?h ?i ?j]] :where [(+ ?a ?b ?c ?d ?e ?f ?g ?h ?i ?j) ?s]]', '[[1 2 3 4 5 6 7 8 9 10] [11 12 13 14 15 16 17 18 19 20]]'),
       ('[:find ?a ?j :in [[?a ?b ?c ?d ?e ?f ?g ?h ?i ?j]] :where [(< ?a ?b ?c ?d ?e ?f ?g ?h ?i ?j)]]', '[[1 2 3 4 5 6 7 8 9 10] [11 12 13 14 15 16 17 18 19 2]]'),
       ('[:find ?a ?j ?z :in [[?a ?b ?c ?d ?e ?f ?g ?h ?i ?j]] [[?a ?z]]]', '[[1 2 3 4 5 6 7 8 9 10] [11 12 13 14 15 16 17 18 19 20]]', '[[1 :one] [11 :eleven] [5 :five]]'),
       ('[:find ?a ?b ?c ?d ?e ?f ?g ?h ?i ?j :in [[?a ?b ?c ?d ?e]] [[?e ?f ?g ?h ?i ?j]]]', '[[1 2 3 4 5] [11 12 13 14 15]]', '[[5 6 7 8 9 10] [15 16 17 18 19 20] [5 60 70 80 90 100]]'),
       ('[:find ?a ?b ?c ?d ?e ?f ?g ?h ?i ?j :in [[?a ?b ?c ?d ?e]] [[?e ?f ?g ?h ?i ?j]] :where (not [(= ?f 60)])]', '[[1 2 3 4 5] [11 12 13 14 15]]', '[[5 6 7 8 9 10] [15 16 17 18 19 20] [5 60 70 80 90 100]]'),
       ('[:find ?a ?j :in [[?a ?b ?c ?d ?e]] [[?e ?f ?g ?h ?i ?j]] :where (or [(= ?f 60)] [(= ?a 11)])]', '[[1 2 3 4 5] [11 12 13 14 15]]', '[[5 6 7 8 9 10] [15 16 17 18 19 20] [5 60 70 80 90 100]]'),
       ('[:find ?a ?j :in [[?a ?b ?c ?d ?e]] [[?e ?f ?g ?h ?i ?j]] :where (or-join [?a ?b ?c ?d ?e ?f ?g ?h ?i ?j] [(= ?f 60)] [(= ?a 11)])]', '[[1 2 3 4 5] [11 12 13 14 15]]', '[[5 6 7 8 9 10] [15 16 17 18 19 20] [5 60 70 80 90 100]]'),
       ('[:find ?a ?j :in [[?a ?b ?c ?d ?e]] [[?e ?f ?g ?h ?i ?j]] :where (not-join [?a ?b ?c ?d ?e ?f ?g ?h ?i ?j] [(= ?f 60)])]', '[[1 2 3 4 5] [11 12 13 14 15]]', '[[5 6 7 8 9 10] [15 16 17 18 19 20] [5 60 70 80 90 100]]'),
       ])

with open(sys.argv[1] if len(sys.argv) > 1 else 'conformance/cases/query.edn', 'w') as f:
    f.write('; Datalog queries, after DataScript\'s own tests and around them. Written by conformance/gen/query.py.\n')
    f.write('\n'.join(lines) + '\n')
print('%d cases' % len(lines))
