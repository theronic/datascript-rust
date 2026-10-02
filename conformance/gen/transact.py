#!/usr/bin/env python3
"""Writes conformance/cases/transact.edn: transactions, after DataScript's own tests (test/datascript/test/
transact.cljc, upsert.cljc, explode.cljc, components.cljc, tuples.cljc, lookup_refs.cljc, validation.cljc, ident.cljc,
index.cljc) and around them. Each scenario is a schema and transactions in order through a connection, so that one
that fails leaves the next the database it found; then the indexes are read."""
import sys

lines = []
def case(steps): lines.append('[' + ' '.join(steps) + ']')

def scenario(schema, txs, reads=()):
    """Transactions in order through a connection; then reads of the database it ends with."""
    steps = ['{:op :conn :schema %s :txs [%s] :as db}' % (schema, ' '.join(txs))]
    steps.append('{:op :rschema :db #r db}')
    for r in reads:
        steps.append(r % {'db': '#r db'} if '%(db)s' in r else r)
    case(steps)

def chain(schema, txs, reads=()):
    """Each transaction on the database before it, with its report; one that fails ends the chain."""
    steps = ['{:op :empty-db :schema %s :as d0}' % schema]
    for i, tx in enumerate(txs):
        steps.append('{:op :with :db #r d%d :tx %s :as d%d}' % (i, tx, i + 1))
    for r in reads:
        steps.append(r % {'db': '#r d%d' % len(txs)})
    case(steps)

def datoms(index, *components): return '{:op :datoms :db %%(db)s :index :%s :components [%s]}' % (index, ' '.join(components))
def seek(index, *components): return '{:op :seek-datoms :db %%(db)s :index :%s :components [%s]}' % (index, ' '.join(components))
def rseek(index, *components): return '{:op :rseek-datoms :db %%(db)s :index :%s :components [%s]}' % (index, ' '.join(components))
def entid(e): return '{:op :entid :db %%(db)s :eid %s}' % e
def irange(a, s, e): return '{:op :index-range :db %%(db)s :attr %s :start %s :end %s}' % (a, s, e)
def find(index, *components): return '{:op :find-datom :db %%(db)s :index :%s :components [%s]}' % (index, ' '.join(components))

AKA = '{:aka {:db/cardinality :db.cardinality/many}}'

# ---- transact.cljc
scenario(AKA, ['[[:db/add 1 :name "Ivan"]]', '[[:db/add 1 :name "Petr"]]', '[[:db/add 1 :aka "Devil"]]', '[[:db/add 1 :aka "Tupen"]]',
               '[[:db/retract 1 :name "Petr"]]', '[[:db/retract 1 :aka "Devil"]]', '[[:db/retract 1 :name "Ivan"]]'])
scenario('nil', ['[[:db/add 1 :attr 2] nil [:db/add 3 :attr 4]]'])
scenario('nil', ['[#datascript/Datom [1 :name "Oleg"] #datascript/Datom [1 :age 17 536870913] [:db/add 1 :aka "x" 536870914]]',
                 '[#datascript/Datom [1 :name "Oleg" 536870912 false]]', '[#datascript/Datom [2 :name "X" 536870950 true] #datascript/Datom [2 :name "X" 536870951 false]]'])
RET = '{:aka {:db/cardinality :db.cardinality/many} :friend {:db/valueType :db.type/ref}}'
BASE = '[{:db/id 1, :name "Ivan", :age 15, :aka ["X" "Y" "Z"], :friend 2} {:db/id 2, :name "Petr", :age 37, :employed? true, :married? false}]'
for tx in ['[[:db.fn/retractEntity 1]]', '[[:db/retractEntity 1]]', '[[:db.fn/retractEntity 2]]', '[[:db.fn/retractAttribute 1 :name]]',
           '[[:db.fn/retractAttribute 1 :aka]]', '[[:db/retract 1 :name] [:db/retract 1 :aka] [:db/retract 2 :employed?] [:db/retract 2 :married?]]',
           '[[:db/retract 2 :employed? false]]', '[[:db.fn/retractEntity 1] [:db.fn/retractEntity 1]]', '[[:db.fn/retractEntity 5]]',
           '[[:db/retract 1 :aka "X"] [:db/retract 1 :aka "Q"] [:db/add 1 :aka "X"]]']:
    chain(RET, [BASE, tx])
NAME_ID = '{:name {:db/unique :db.unique/identity}}'
for op in ['[:db/retract 2 :name "Petr"]', '[:db.fn/retractAttribute 2 :name]', '[:db.fn/retractEntity 2]', '[:db/retractEntity 2]',
           '[:db/retract [:name "Petr"] :name "Petr"]', '[:db.fn/retractAttribute [:name "Petr"] :name]', '[:db.fn/retractEntity [:name "Petr"]]',
           '[:db/retract 1 :name "Ivan"]', '[:db.fn/retractAttribute 1 :name]', '[:db.fn/retractEntity 1]', '[:db/retractEntity 1]',
           '[:db/retract [:name "Ivan"] :name "Ivan"]', '[:db.fn/retractAttribute [:name "Ivan"] :name]', '[:db.fn/retractEntity [:name "Ivan"]]']:
    chain(NAME_ID, ['[[:db/add 1 :name "Ivan"]]', '[%s]' % op])
    chain(NAME_ID, ['[[:db/add 1 :name "Ivan"]]', '[%s %s]' % (op, op)])
# cas
scenario('nil', ['[[:db/add 1 :weight 200]]', '[[:db.fn/cas 1 :weight 200 300]]', '[[:db/cas 1 :weight 300 400]]', '[[:db.fn/cas 1 :weight 200 210]]',
                 '[[:db.fn/cas 1 :age nil 42]]', '[[:db.fn/cas 1 :age nil 4711]]', '[[:db/add -1 :name "Ivan"] [:db.fn/cas -1 :attr nil :val]]',
                 '[[:db.fn/cas 1 :weight 400 nil]]', '[[:db.fn/cas 9 :weight nil 1]]', '[[:db/cas 1 5 400 1]]', '[[:db/cas [:weight 400] :weight 400 1]]'])
scenario('{:label {:db/cardinality :db.cardinality/many}}', ['[[:db/add 1 :label :x]]', '[[:db/add 1 :label :y]]', '[[:db.fn/cas 1 :label :y :z]]', '[[:db.fn/cas 1 :label :s :t]]'])
scenario('{:ref {:db/valueType :db.type/ref} :name {:db/unique :db.unique/identity}}',
         ['[{:db/id 1 :name "a"} {:db/id 2 :name "b"} {:db/id 3 :name "c" :ref 1}]', '[[:db/cas 3 :ref 1 2]]', '[[:db/cas 3 :ref [:name "b"] [:name "a"]]]',
          '[[:db/cas 3 :ref [:name "zzz"] 1]]', '[[:db/cas 3 :ref 2 1]]', '[[:db/cas 3 :ref nil 1]]'])
# tempids
REF3 = '{:name {:db/unique :db.unique/identity} :aka {:db/unique :db.unique/identity :db/cardinality :db.cardinality/many} :ref {:db/valueType :db.type/ref}}'
chain(REF3, ['[[:db/add -1 :name "Ivan"] [:db/add -1 :age 19] [:db/add -2 :name "Petr"] [:db/add -2 :age 22] [:db/add "Serg" :name "Sergey"] [:db/add "Serg" :age 30]]'])
chain(REF3, ['[[:db/add -1 :name "Ivan"] [:db/add -2 :ref -1]]'])
chain(REF3, ['[[:db/add -1 :name "Ivan"]]', '[[:db/add -1 :name "Ivan"] [:db/add -2 :ref -1]]'])
chain(REF3, ['[[:db/add -1 :aka "Batman"]]', '[[:db/add -1 :aka "Batman"] [:db/add -2 :ref -1]]'])
chain('{:ref {:db/unique :db.unique/identity :db/valueType :db.type/ref}}', ['[[:db/add -1 :name "Ivan"] [:db/add -2 :name "Petr"] [:db/add -1 :ref -2]]'])
FRIENDS = '{:friend {:db/valueType :db.type/ref :db/cardinality :db.cardinality/many}}'
chain(FRIENDS, ['[{:name "Sergey" :friend [-1 -2]} [:db/add -1 :name "Ivan"] [:db/add -2 :name "Petr"] [:db/add "B" :name "Boris"] [:db/add "B" :friend -3] [:db/add -3 :name "Oleg"] [:db/add -3 :friend "B"]]'])
UNUSED = '{:friend {:db/valueType :db.type/ref} :comp {:db/valueType :db.type/ref, :db/isComponent true} :multi {:db/cardinality :db.cardinality/many}}'
for tx in ['[[:db/add -1 :friend -2]]', '[{:db/id -1 :friend -2}]', '[{:db/id -1} [:db/add -2 :friend -1]]', '[{:db/id -1 :multi []} [:db/add -2 :friend -1]]',
           '[{:db/id -1 :comp {}} [:db/add -2 :friend -1]]', '[[:db/add -1 :friend -2] [:db/add -3 :friend "x"]]', '[[:db/add 1 :friend -2] [:db/add 1 :friend "s"] [:db/add 2 :friend -9]]']:
    chain(UNUSED, [tx])
for tid in [':db/current-tx', '"datomic.tx"', '"datascript.tx"', '":db/current-tx"']:
    scenario('{:created-at {:db/valueType :db.type/ref}}',
             ['[{:name "X", :created-at %s} {:db/id %s, :prop1 "prop1"} [:db/add %s :prop2 "prop2"] [:db/add -1 :name "Y"] [:db/add -1 :created-at %s]]' % (tid, tid, tid, tid),
              '[[:db/add %s :prop3 "prop3"]]' % tid, '[{:db/id %s, :prop4 "prop4"}]' % tid, '[[:db/retract %s :prop4 "prop4"]]' % tid])
chain('nil', ['[{:db/id %d :a1 1 :a2 2 :a3 3}]' % i for i in range(1, 10)] + ['[[:db.fn/retractEntity 1] [:db.fn/retractEntity 2]]'])
for tx in ['[[:db/add 285873023227265 :name "Valerii"]]', '[{:db/id 285873023227265 :name "Valerii"}]', '[{:db/id 1 :ref 285873023227265}]', '[[:db/add 2147483647 :name "max"]]',
           '[[:db/add 536870912 :name "tx0"]]', '[[:db/add 536870911 :name "below"]]', '[[:db/add 0 :name "zero"]]', '[[:db/add 1 :ref 0]]', '[[:db/add 1 :ref 536870912]]']:
    chain('{:ref {:db/valueType :db.type/ref}}', [tx, '[{:name "next"}]'])
UNCMP = '{:multi {:db/cardinality :db.cardinality/many} :index {:db/index true}}'
chain(UNCMP, ['[[:db/add 1 :single {:map 1}]]', '[[:db/retract 1 :single {:map 1}]]', '[[:db/add 1 :single {:map 2}]]', '[[:db/add 1 :single {:map 3}]]'],
      [datoms('eavt', '1', ':single', '{:map 3}'), datoms('aevt', ':single', '1', '{:map 3}')])
chain(UNCMP, ['[[:db/add 1 :multi {:map 1}]]', '[[:db/add 1 :multi {:map 1}]]', '[[:db/add 1 :multi {:map 2}]]'], [datoms('eavt', '1', ':multi', '{:map 2}'), datoms('aevt', ':multi', '1', '{:map 2}')])
chain(UNCMP, ['[[:db/add 1 :index {:map 1}]]', '[[:db/retract 1 :single {:map 1}]]', '[[:db/add 1 :index {:map 2}]]', '[[:db/add 1 :index {:map 3}]]'], [datoms('avet', ':index', '{:map 3}', '1')])
chain('nil', ['[{:num 42.5}]', '[[:db/retract 1 :num 42]]'])
scenario('{:block/uid {:db/unique :db.unique/identity}}',
         ['[{:block/uid "%s"}]' % u for u in ["2LB4tlJGy", "2ON453J0Z", "2KqLLNbPg", "2L0dcD7yy", "2KqFNrhTZ", "2KdQmItUD", "2O8BcBfIL", "2L4ZbI7nK", "2KotiW36Z", "2O4o-y5J8", "2KimvuGko", "dTR20ficj", "wRmp6bXAx", "rfL-iQOZm", "tya6s422-"]] + ['[{:block/uid 45619}]'],
         [entid('[:block/uid "tya6s422-"]'), entid('[:block/uid 45619]'), entid('[:block/uid "nope"]')])

# ---- upsert.cljc
UPS = '{:name {:db/unique :db.unique/identity} :email {:db/unique :db.unique/identity} :slugs {:db/unique :db.unique/identity :db/cardinality :db.cardinality/many} :ref {:db/unique :db.unique/identity :db/type :db.type/ref}}'
UPS_BASE = '[{:db/id 1 :name "Ivan" :email "@1"} {:db/id 2 :name "Petr" :email "@2" :ref 3} {:db/id 3 :name "Dima" :email "@3" :ref 4} {:db/id 4 :name "Olga" :email "@4" :ref 1}]'
for tx in ['[{:name "Ivan" :age 35}]', '[{:name "Ivan" :email "@1" :age 35}]', '[{:db/id -1 :name "Ivan" :age 35}]',
           '[{:db/id "1" :name "Ivan" :age 35} [:db/add "2" :name "Oleg"] [:db/add "2" :email "@2"]]', '[{:db/id -1 :name "Ivan" :email "@1" :age 35}]',
           '[{:db/id -1 :name "Ivan" :age 35} {:db/id -1 :name "Ivan" :age 36}]', '[{:db/id -1 :name "Ivan" :age 35} {:db/id -2 :name "Ivan" :age 36}]',
           '[{:db/id 1 :name "Ivan" :age 35}]', '[{:db/id 1 :name "Ivan" :email "@1" :age 35}]', '[{:db/id [:name "Ivan"] :name "Ivan" :email "@1" :age 35}]',
           '[{:db/id 2 :name "Ivan" :age 36}]', '[{:db/id 5 :name "Ivan" :age 36}]', '[{:name "Ivan" :email "@5" :age 35}]', '[{:name "Ivan" :email "@2" :age 35}]',
           '[{:name "Igor" :age 35} {:name "Igor" :age 36}]', '[{:db/id -1 :name "Igor" :age 35} {:db/id -1 :name "Igor" :age 36}]',
           '[{:db/id -1 :name "Igor" :age 35} {:db/id -2 :name "Igor" :age 36}]', '[{:db/id :db/current-tx :name "Ivan" :age 35}]',
           '[{:ref 3 :age 36}]', '[{:ref 4 :age 37}]', '[{:ref 1 :age 38}]', '[{:ref [:name "Dima"] :age 36}]', '[{:ref [:name "Olga"] :age 37}]', '[{:ref [:name "Ivan"] :age 38}]',
           '[{:db/id -1 :name "Igor"} {:db/id -2 :name "Anna" :ref -1}]', '[{:db/id "A" :name "Igor"} {:db/id "B" :name "Anna" :ref "A"}]',
           '[{:ref [:name "Nobody"] :age 1}]', '[{:db/id [:name "Nobody"] :age 1}]', '[{:db/id [:age 1] :age 1}]', '[{:db/id [:name] :age 1}]', '[{:db/id :kw :age 1}]', '[{:db/id {:a 1} :age 1}]',
           '[{:db/id nil :age 1}]', '[{:name nil}]', '[{5 6}]', '[{:name "Ivan" :slugs "ivan1"} {:name "Petr" :slugs "petr1"}]']:
    chain(UPS, [UPS_BASE, tx])
chain(UPS, [UPS_BASE, '[{:name "Ivan" :slugs "ivan1"} {:name "Petr" :slugs "petr1"}]', '[{:name "Ivan" :slugs ["ivan1" "ivan2"]}]'])
chain(UPS, [UPS_BASE, '[{:name "Ivan" :slugs "ivan1"} {:name "Petr" :slugs "petr1"}]', '[{:slugs ["ivan1" "petr1"]}]'])
chain(UPS, [UPS_BASE, '[{:name "Ivan" :slugs "ivan1"} {:name "Petr" :slugs "petr1"}]', '[{:slugs #{"new1" "ivan1" "new2"} :age 5}]'])
chain(NAME_ID, ['[{:db/id -1 :name "Ivan"}]', '[{:db/id -1 :age 35} {:db/id -1 :name "Ivan" :age 36}]'])
chain(NAME_ID, ['[{:db/id -1 :name "Ivan"} {:db/id -2 :name "Oleg"}]', '[{:db/id -1 :name "Ivan" :age 35} {:db/id -1 :name "Oleg" :age 36}]'])
chain(NAME_ID, ['[[:db/add -1 :age 42] [:db/add -2 :likes "Pizza"] [:db/add -1 :name "Bob"] [:db/add -2 :name "Bob"]]'])
chain(NAME_ID, ['[[:db/add -1 :age 42] [:db/add -2 :likes "Pizza"] [:db/add -2 :name "Bob"] [:db/add -1 :name "Bob"]]'])
NAME_REF = '{:name {:db/unique :db.unique/identity} :ref {:db/valueType :db.type/ref}}'
for tx in ['[{:db/id "user", :name "Alice"} {:age 36, :ref "user"}]', '[[:db/add "user" :name "Alice"] {:age 36, :ref "user"}]', '[{:db/id -1, :name "Alice"} {:age 36, :ref -1}]', '[[:db/add -1, :name "Alice"] {:age 36, :ref -1}]']:
    chain(NAME_REF, ['[{:name "Alice"}]', tx])
chain(NAME_REF, ['[{:name "Alice"} {:name "Bob"}]', '[{:db/id 3, :ref "A"} {:db/id 4, :ref "B"} {:db/id "A", :name "Alice"} {:db/id "B", :name "Bob"}]'])
chain(NAME_ID, ['[{:db/id -1, :name "Ivan"}]', '[[:db/add -1 :name "Ivan"] [:db/add -1 :age 12]]'])
chain(NAME_ID, ['[{:db/id -1, :name "Ivan"}]', '[[:db/add -1 :age 12] [:db/add -1 :name "Ivan"]]'])
chain(NAME_ID, ['[[:db/add -1 :name "Ivan"] [:db/add -2 :name "Oleg"]]', '[[:db/add -1 :name "Ivan"] [:db/add -1 :age 35] [:db/add -1 :name "Oleg"] [:db/add -1 :age 36]]'])
chain(NAME_ID, ['[[:db/add -1 :name "Ivan"] [:db/add -2 :name "Oleg"]]', '[[:db/add 1 :name "Oleg"]]'])
chain('{:name {:db/unique :db.unique/value}}', ['[[:db/add -1 :name "Ivan"] [:db/add -2 :name "Oleg"]]', '[[:db/add -1 :name "Ivan"]]'])
chain('{:name {:db/unique :db.unique/value}}', ['[{:name "Ivan"}]', '[{:name "Ivan" :age 5}]'])

# ---- explode.cljc
EXP = '{:aka {:db/cardinality :db.cardinality/many} :also {:db/cardinality :db.cardinality/many}}'
for coll in ['["Devil" "Tupen"]', '#{"Devil" "Tupen"}', '("Devil" "Tupen")', '[]', '#{"a" "b" "c" "d" "e" "f" "g" "h" "i" "j"}', '"single"', '{:a 1}', '[[1 2] [3 4]]', '[:name "x"]']:
    chain(EXP, ['[{:db/id -1 :name "Ivan" :age 16 :aka %s :also "ok"}]' % coll])
CHILDREN = '{:children {:db/valueType :db.type/ref :db/cardinality :db.cardinality/many}}'
for children in ['[-2 -3]', '#{-2 -3}', '(-2 -3)']:
    chain(CHILDREN, ['[{:db/id -1, :name "Ivan", :children %s} {:db/id -2, :name "Petr"} {:db/id -3, :name "Evgeny"}]' % children])
chain(CHILDREN, ['[{:db/id -1, :name "Ivan"} {:db/id -2, :name "Petr", :_children -1} {:db/id -3, :name "Evgeny", :_children -1}]'])
chain(CHILDREN, ['[{:name "Sergey" :_parent 1}]'])
chain(CHILDREN, ['[{:db/id -1, :name "Ivan"} {:db/id -2, :name "Petr", :_children [-1]} {:name "Both" :_children [-1 -2]}]'])
PROFILE = '{:profile {:db/valueType :db.type/ref}}'
PROFILES = '{:profile {:db/valueType :db.type/ref :db/cardinality :db.cardinality/many}}'
for schema in [PROFILE, PROFILES]:
    for tx in ['[{:db/id 5 :name "Ivan" :profile {:db/id 7 :email "@2"}}]', '[{:name "Ivan" :profile {:email "@2"}}]', '[{:profile {:email "@2"}}]', '[{:email "@2" :_profile {:name "Ivan"}}]',
               '[{:db/id 5 :name "Ivan" :profile [{:db/id 7 :email "@2"} {:db/id 8 :email "@3"}]}]', '[{:name "Ivan" :profile [{:email "@2"} {:email "@3"}]}]',
               '[{:name "Ivan" :profile #{{:email "@2"} {:email "@3"}}}]', '[{:name "Ivan" :profile ({:email "@2"} {:email "@3"})}]',
               '[{:email "@2" :_profile [{:name "Ivan"} {:name "Petr"}]}]', '[{:name "Deep" :profile {:email "@1" :profile {:email "@2" :profile {:email "@3"}}}}]',
               '[[:db/add -1 :profile {:email "nested"}]]', '[[:db/add 1 :profile [{:email "a"} {:email "b"}]]]', '[{:name "x" :profile [:name "x"]}]']:
        chain(schema, [tx])
for schema in ['{:comp {:db/valueType :db.type/ref :db/cardinality :db.cardinality/many :db/isComponent true}}', '{:comp {:db/valueType :db.type/ref :db/cardinality :db.cardinality/many}}']:
    chain(schema, ['[{:db/id -1 :name "Name"}]', '[{:db/id 1, :comp [{:name "C"}]}]'])
for schema in ['{:comp {:db/valueType :db.type/ref :db/isComponent true}}', '{:comp {:db/valueType :db.type/ref}}']:
    chain(schema, ['[{:db/id -1 :name "Name"}]', '[{:db/id 1, :comp {:name "C"}}]'])

# ---- components.cljc
COMP = '{:profile {:db/valueType :db.type/ref :db/isComponent true}}'
COMPS = '{:profile {:db/valueType :db.type/ref :db/cardinality :db.cardinality/many :db/isComponent true}}'
chain(COMP, ['[{:db/id 1 :name "Ivan" :profile 3} {:db/id 3 :email "@3"} {:db/id 4 :email "@4"}]', '[[:db/add 3 :profile 4]]', '[[:db.fn/retractEntity 1]]'])
chain(COMP, ['[{:db/id 1 :name "Ivan" :profile 3} {:db/id 3 :email "@3"} {:db/id 4 :email "@4"}]', '[[:db.fn/retractAttribute 1 :profile]]'])
chain(COMPS, ['[{:db/id 1 :name "Ivan" :profile [3 4]} {:db/id 3 :email "@3"} {:db/id 4 :email "@4"}]', '[[:db.fn/retractEntity 1]]'])
chain(COMPS, ['[{:db/id 1 :name "Ivan" :profile [3 4]} {:db/id 3 :email "@3"} {:db/id 4 :email "@4"}]', '[[:db.fn/retractAttribute 1 :profile]]'])
# many components: the retractions come out of a set, in its order
chain(COMPS, ['[{:db/id 1 :name "Ivan" :profile [%s]} %s]' % (' '.join(str(i) for i in range(2, 22)), ' '.join('{:db/id %d :email "@%d" :profile [%d]}' % (i, i, i + 100) for i in range(2, 22))),
              '[[:db.fn/retractEntity 1]]'])
chain(COMPS, ['[{:db/id 1 :profile [2 3 4 5]} {:db/id 2 :profile [3 6]} {:db/id 3 :profile [1]} {:db/id 6 :x 1}]', '[[:db.fn/retractEntity 1]]'])
for schema in ['{:profile {:db/isComponent true}}', '{:profile {:db/isComponent "aaa" :db/valueType :db.type/ref}}', '{:a {:db/unique :bad}}', '{:a {:db/valueType :db.type/string}}',
               '{:a {:db/cardinality :many}}', '{:a {:db/unique nil :db/cardinality nil}}', '{:a nil}', '{:a {}}', '[]', '5', '{:t1 {:db/tupleAttrs [:a :b]} :t2 {:db/tupleAttrs [:c :d :e :t1]}}',
               '{:t1 {:db/tupleAttrs :a}}', '{:t1 {:db/tupleAttrs ()}}', '{:t1 {:db/tupleAttrs [:a :b :c] :db/cardinality :db.cardinality/many}}',
               '{:a {:db/cardinality :db.cardinality/many} :t1 {:db/tupleAttrs [:a :b :c]}}', '{:foo+bar {:db/valueType :db.type/tuple}}',
               '{:t {:db/tupleAttrs [:a :b] :db/tupleTypes [:db.type/long :db.type/long]}}', '{:t {:db/tupleTypes [:db.type/long]}}', '{:t {:db/tupleTypes :db.type/long}}', '{:t {:db/tupleTypes [1 2]}}',
               '{:t {:db/tupleType "str"}}', '{:t {:db/tupleType :db.type/ref :db/tupleTypes [:a :b] :db/tupleAttrs [:x :y]}}',
               '{:year+session {:db/tupleAttrs [:year :session]} :semester+course+student {:db/tupleAttrs [:semester :course :student]} :session+student {:db/tupleAttrs [:session :student] :db/valueType :db.type/tuple}}',
               '{:t1 {:db/tupleAttrs [:a :b]} :t2 {:db/valueType :db.type/tuple :db/tupleTypes [:db.type/long :db.type/string]} :t3 {:db/valueType :db.type/tuple :db/tupleType :db.type/keyword}}',
               '{:a {:db/index true} :b {:db/index false} :c {:db/isComponent false} :d {:db/unique :db.unique/value :db/valueType :db.type/ref :db/cardinality :db.cardinality/many :db/isComponent true}}',
               '{:db/ident {:db/cardinality :db.cardinality/many}}', '{"str" {:db/valueType :db.type/ref} :kw {:db/valueType :db.type/ref}}',
               '{:a1 {:db/valueType :db.type/ref} :a2 {:db/valueType :db.type/ref} :a3 {:db/valueType :db.type/ref} :a4 {:db/valueType :db.type/ref} :a5 {:db/valueType :db.type/ref} :a6 {:db/valueType :db.type/ref} :a7 {:db/valueType :db.type/ref} :a8 {:db/valueType :db.type/ref} :a9 {:db/valueType :db.type/ref} :a10 {:db/valueType :db.type/ref} :a11 {:db/valueType :db.type/ref}}']:
    case(['{:op :empty-db :schema %s :as db}' % schema, '{:op :rschema :db #r db}', '{:op :schema :db #r db}'])
# retracting an entity that many reference attributes point at: by attribute, in the order of their set
MANYREFS = '{' + ' '.join(':a%d {:db/valueType :db.type/ref}' % i for i in range(1, 12)) + '}'
chain(MANYREFS, ['[' + ' '.join('{:db/id %d :a%d 100}' % (i, i) for i in range(1, 12)) + ' {:db/id 100 :name "target"}]', '[[:db.fn/retractEntity 100]]'])
chain(MANYREFS, ['[' + ' '.join('{:db/id %d :a%d 100 :a%d 100}' % (20 - i, i, (i % 11) + 1) for i in range(1, 12)) + ' {:db/id 100 :name "target"}]', '[[:db/retractEntity 100]]'])

# ---- tuples.cljc
T2 = '{:a+b {:db/tupleAttrs [:a :b]} :a+c+d {:db/tupleAttrs [:a :c :d]}}'
scenario(T2, ['[[:db/add 1 :a "a"]]', '[[:db/add 1 :b "b"]]', '[[:db/add 1 :a "A"]]', '[[:db/add 1 :c "c"] [:db/add 1 :d "d"]]', '[[:db/add 1 :a "a"]]',
              '[[:db/add 1 :a "A"] [:db/add 1 :b "B"] [:db/add 1 :c "C"] [:db/add 1 :d "D"]]', '[[:db/retract 1 :a "A"]]', '[[:db/retract 1 :b "B"]]', '[{:db/id 1 :a+b ["A" "B"]}]',
              '[[:db.fn/retractEntity 1]]', '[{:db/id 2 :a 1 :b 2 :c 3 :d 4} {:db/id 3 :a 1}]', '[[:db.fn/retractAttribute 2 :a]]', '[[:db/retract 2 :a+c+d]]'])
TAB = '{:a+b {:db/tupleAttrs [:a :b]}}'
scenario(TAB, ['[{:db/id 1 :a "a" :b "b" :a+b ["a" "b"]}]', '[{:db/id 2 :a "x" :b "y" :a+b ["a" "b"]}]', '[{:db/id 2 :a+b ["a" "b"] :a "x" :b "y"}]', '[{:db/id 2 :a "a" :b "b" :a+b ["a"]}]',
               '[{:db/id 2 :a "a" :b "b" :a+b ["a" "b" "c"]}]', '[{:db/id 2 :a "a" :b "b" :a+b ["a" nil]}]', '[{:db/id 1 :a "x" :a+b ["a" "b"]}]', '[{:db/id 1 :a+b ["a" "B"]}]',
               '[{:db/id 1 :a "a" :b "b" :a+b ["a"]}]', '[{:db/id 1 :a "a" :b "b" :a+b ["a" nil]}]', '[{:db/id 1 :a+b ["a" "b"]}]', '[{:db/id 1 :b "B" :a+b ["a" "B"]}]', '[{:db/id 1 :a+b ["A" "B"] :a "A"}]',
               '[[:db/add 1 :a+b 5]]', '[[:db/add 1 :a+b nil]]', '[[:db/add 1 :a+b "ab"]]', '[[:db/retract 1 :a+b ["A" "B"]]]', '[[:db/add [:a+b ["A" "B"]] :x 1]]'])
TU = '{:a+b {:db/tupleAttrs [:a :b] :db/unique :db.unique/identity}}'
scenario(TU, ['[[:db/add 1 :a "a"]]', '[[:db/add 2 :a "A"]]', '[[:db/add 1 :a "A"]]', '[[:db/add 1 :b "b"] [:db/add 2 :b "b"] {:db/id 3 :a "a" :b "B"}]', '[[:db/add 1 :a "A"]]', '[[:db/add 1 :b "B"]]',
              '[[:db/add 1 :a "A"] [:db/add 1 :b "B"]]', '[{:db/id 1 :a "A" :b "B"}]', '[{:db/id 4 :a "a" :b "b"}]'],
         [entid('[:a+b ["a" "b"]]'), entid('[:a+b ["zz" "b"]]'), datoms('avet', ':a+b', '["a" "b"]'), datoms('avet', ':a+b'), irange(':a+b', '["A" "B"]', '["a" "b"]')])
TUC = '{:a+b {:db/tupleAttrs [:a :b] :db/unique :db.unique/identity} :c {:db/unique :db.unique/identity}}'
scenario(TUC, ['[{:db/id 1 :a "A" :b "B"} {:db/id 2 :a "a" :b "b"}]', '[{:a+b ["A" "B"] :c "C"} {:a+b ["a" "b"] :c "c"}]', '[{:a+b ["A" "B"] :c "c"}]', '[{:a+b ["A" "B"] :b "b" :d "D"}]'])
scenario(TUC, ['[{:db/id 1 :a "A" :b "B"} {:db/id 2 :a "a" :b "b"}]', '[[:db/add [:a+b ["A" "B"]] :c "C"] {:db/id [:a+b ["a" "b"]] :c "c"}]', '[[:db/add [:a+b ["A" "B"]] :c "c"]]',
               '[{:db/id [:a+b ["A" "B"]] :c "c"}]', '[{:db/id [:a+b ["A" "B"]] :b "b" :d "D"}]'])
for tx in ['[{:db/id -1 :a "A" :b "B" :name "Oleg"}]', '[{:a "A" :b "B" :name "Oleg"}]', '[[:db/add -1 :a "A"] [:db/add -1 :b "B"] [:db/add -1 :name "Oleg"]]']:
    chain(TU, ['[{:a "A" :b "B" :name "Ivan"}]', tx])
PLAYERS = '{:player {:db/unique :db.unique/identity} :home {:db/valueType :db.type/ref} :away {:db/valueType :db.type/ref} :players {:db/unique :db.unique/identity :db/tupleAttrs [:home :away]}}'
chain(PLAYERS, ['[[:db/add -1 :player "Nadal"] [:db/add -2 :player "Federer"] {:home -1 :away -2}]', '[{:db/id "p1" :player "Nadal"} {:db/id "p2" :player "Federer"} {:db/id "match" :players ["p1" "p2"] :game 3}]'])
REFNAME = '{:ref {:db/valueType :db.type/ref} :name {:db/unique :db.unique/identity} :ref+name {:db/valueType :db.type/tuple :db/tupleAttrs [:ref :name] :db/unique :db.unique/identity}}'
RN_BASE = '[{:db/id -1 :name "Ivan"} {:db/id -2 :name "Oleg"} {:db/id -3 :name "Petr" :ref -1} {:db/id -4 :name "Yuri" :ref -2}]'
for tx in ['[{:ref+name [1 "Petr"], :age 32}]', '[{:ref+name [[:name "Ivan"] "Petr"], :age 32}]', '[[:db/add -1 :ref+name [1 "Petr"]] [:db/add -1 :age 32]]', '[[:db/add -1 :ref+name [[:name "Ivan"] "Petr"]] [:db/add -1 :age 32]]',
           '[{:ref+name [[:name "Nobody"] "Petr"], :age 32}]', '[{:ref+name [-1 "Petr"], :age 32}]', '[[:db/add -1 :ref+name [:db/current-tx "Petr"]]]']:
    chain(REFNAME, [RN_BASE, tx], [entid('[:name "Ivan"]'), entid('[:ref+name [1 "Petr"]]'), entid('[:ref+name [[:name "Ivan"] "Petr"]]'), entid('[:ref+name [[:name "Nobody"] "Petr"]]')])
for tx, start in [('[[:db/add 1 :a+b [nil nil]]]', None), ('[[:db/add 1 :a+b ["a" nil]]]', '[[:db/add 1 :a "a"]]'), ('[[:db/add 1 :a "a"] [:db/add 1 :a+b ["a" nil]]]', None), ('[[:db/retract 1 :a+b ["a" nil]]]', '[[:db/add 1 :a "a"]]')]:
    chain(TAB, ([start] if start else []) + [tx])
ABC = '{:a+b+c {:db/tupleAttrs [:a :b :c]}}'
chain(ABC, ['[' + ' '.join('{:db/id %d :a "%s" :b "%s" :c "%s"}' % (i + 1, 'aA'[i & 1], 'bB'[(i >> 1) & 1], 'cC'[(i >> 2) & 1]) for i in range(8)) + ']'],
      [datoms('avet', ':a+b+c', '["A" "b" "C"]'), datoms('avet', ':a+b+c', '["A" "b" nil]'), irange(':a+b+c', '["A" "B" "C"]', '["A" "b" "c"]'), irange(':a+b+c', '["A" "B" nil]', '["A" "b" nil]'),
       seek('avet', ':a+b+c', '["a" "B" nil]'), rseek('avet', ':a+b+c', '["a" "B" nil]')])
# heterogeneous and homogeneous tuples, with reference slots
HT = '{:t2 {:db/valueType :db.type/tuple :db/tupleTypes [:db.type/long :db.type/string]} :t3 {:db/valueType :db.type/tuple :db/tupleType :db.type/keyword} :pair {:db/valueType :db.type/tuple :db/tupleTypes [:db.type/ref :db.type/ref] :db/unique :db.unique/identity} :refs {:db/valueType :db.type/tuple :db/tupleType :db.type/ref :db/cardinality :db.cardinality/many} :name {:db/unique :db.unique/identity}}'
scenario(HT, ['[{:db/id 1 :name "a" :t2 [1 "x"] :t3 [:a :b :c]}]', '[{:db/id 2 :name "b" :t2 [1]}]', '[{:db/id 2 :name "b" :t2 "str"}]', '[{:db/id 2 :name "b" :t3 :kw}]', '[{:db/id 2 :name "b" :t2 (1 "x")}]',
              '[{:db/id 2 :name "b" :t3 []}]', '[{:db/id 3 :name "c" :pair [1 2]}]', '[{:db/id 4 :name "d" :pair [[:name "a"] [:name "b"]]}]', '[{:db/id 5 :name "e" :pair [-1 -2]} {:db/id -1 :name "f"} {:db/id -2 :name "g"}]',
              '[{:name "h" :pair [-7 1]}]', '[{:name "i" :pair [:db/current-tx 1]}]', '[{:name "j" :pair ["datomic.tx" "x"]} {:db/id "x" :name "k"}]', '[{:name "l" :pair [[:name "nobody"] 1]}]',
              '[{:pair [1 2] :upserted true}]', '[{:pair [[:name "a"] [:name "b"]] :upserted 2}]', '[[:db/add 3 :refs [[1 2] [[:name "a"]]]]]', '[[:db/add 3 :refs [1 2]]]', '[[:db/retract 3 :pair [[:name "a"] [:name "b"]]]]',
              '[[:db/cas 1 :t2 [1 "x"] [2 "y"]]]', '[[:db/cas 4 :pair [[:name "a"] 2] [2 [:name "a"]]]]', '[[:db/cas 1 :t2 [9 "z"] [2 "y"]]]', '[[:db/add 1 :t2 nil]]', '[[:db/retract 1 :t2 [2 "y"]]]'],
         [datoms('avet', ':pair', '[[:name "a"] 2]'), entid('[:pair [1 2]]'), entid('[:pair [[:name "a"] [:name "b"]]]'), entid('[:pair [1]]'), entid('[:pair [[:name "zz"] 2]]'), datoms('eavt', '3', ':pair', '[1 [:name "b"]]')])

# ---- lookup refs, idents, validation
LR = '{:name {:db/unique :db.unique/identity} :email {:db/unique :db.unique/value} :friend {:db/valueType :db.type/ref} :friends {:db/valueType :db.type/ref :db/cardinality :db.cardinality/many}}'
LR_BASE = '[{:db/id 1 :name "Ivan" :email "@1"} {:db/id 2 :name "Petr" :email "@2"} {:db/id 3 :name "Oleg" :email "@3"}]'
for tx in ['[[:db/add [:name "Ivan"] :age 35]]', '[{:db/id [:name "Ivan"] :age 35}]', '[[:db/add 1 :friend [:name "Petr"]]]', '[{:db/id 1 :friend [:name "Petr"]}]', '[{:db/id 1 :friends [[:name "Petr"] [:name "Oleg"]]}]',
           '[{:db/id 1 :friends [:name "Petr"]}]', '[{:db/id 2 :_friend [:name "Ivan"]}]', '[{:db/id 2 :_friends [[:name "Ivan"] [:name "Oleg"]]}]', '[[:db/add [:name "Nobody"] :age 1]]', '[[:db/add 1 :friend [:name "Nobody"]]]',
           '[[:db/add [:age 5] :age 1]]', '[[:db/add [:name "Ivan" "x"] :age 1]]', '[[:db/add [:email "@1"] :age 1]]', '[[:db/retract [:name "Ivan"] :email "@1"]]', '[[:db.fn/retractEntity [:email "@2"]]]',
           '[[:db/add [:name nil] :age 1]]', '[[:db/add 1 :friend [:name nil]]]', '[{:db/id [:name "Ivan"] :name "Petr"}]', '[{:db/id [:name "Ivan"] :email "@2"}]', '[[:db/add [:friend 1] :age 1]]',
           '[[:db/add :an-ident :age 1]]', '[[:db/add 1 :friend :an-ident]]', '[{:db/id 1 :friend {:db/id [:name "Petr"] :age 99}}]']:
    chain(LR, [LR_BASE, tx])
IDENT = '{:ref {:db/valueType :db.type/ref}}'
chain(IDENT, ['[[:db/add 1 :db/ident :ent1] [:db/add 2 :db/ident :ent2] [:db/add 2 :ref 1]]', '[[:db/add :ent1 :ref :ent2]]', '[{:db/id :ent2 :name "by ident"}]', '[[:db/add :nope :x 1]]', '[{:db/ident :ent1 :upserted true}]',
              '[[:db.fn/retractEntity :ent2]]', '[[:ent1 5]]', '[[:unknown-fn]]', '[["str-op" 1 :a 2]]', '[[nil 1 :a 2]]', '[[:db/add]]', '[[]]', '[5]', '["str"]', '[#{1 2}]'],
      [entid(':ent1'), entid(':ent2'), entid(':nope'), datoms('avet', ':ref', ':ent1'), datoms('eavt', ':ent1')])
for tx in ['[[:db/add -1 :name nil]]', '[[:db/add -1 nil 1]]', '[[:db/add -1 5 1]]', '[{:db/id -1 "str-attr" 1}]', '[[:db/add nil :name 1]]', '[[:db/add "s" :name 1] [:db/retract "s" :name 1]]',
           '[[:db/retract -1 :name 1]]', '[[:db.fn/retractEntity -1]]', '{:db/id 1 :name "map as tx"}', '5', '"str"', '#{[:db/add 1 :a 1]}', '([:db/add 1 :a 1])', 'nil', '[]',
           '[[:db/retract 1 :name nil]]', '[[:db/retract 1 nil]]', '[[:db.fn/retractAttribute 1 5]]', '[[:db/add 1 :a 1 536870999]]', '[[:db/add 1 :a 1 nil]]', '[{:db/id 1 :a [1 2 3]}]', '[{:db/id 1 :a #{1 2}}]',
           '[{:db/id 1 :a {:nested "map"}}]', '[{:db/id 1 :a ()}]', '[{:db/id 1 :a nil}]', '[{:db/id 1}]', '[{}]', '[{:a 1} {:a 2} {:a 3}]', '[[:db/add 1 :a 1] [:db/add 1 :a 1]]', '[[:db/add 1 :a 1] [:db/add 1 :a 2] [:db/add 1 :a 1]]']:
    chain('nil', [tx], [datoms('eavt')])
# string attributes, as DataScript's JavaScript API names them
chain('{"friend" {:db/valueType :db.type/ref} "aka" {:db/cardinality :db.cardinality/many} "ns/name" {:db/unique :db.unique/identity}}',
      ['[{:db/id 1 "ns/name" "Ivan" "aka" ["a" "b"] "friend" {"ns/name" "Petr"}}]', '[{"ns/name" "Petr" "_friend" [1]}]', '[{"ns/name" "Oleg" "ns/_friend" 1}]', '[["db/add" 1 "x" 2]]', '[[:db/add 1 "x" 2] [:db/retract 1 "aka" "a"]]'],
      [datoms('aevt', '"aka"'), datoms('avet', '"ns/name"', '"Petr"'), entid('["ns/name" "Ivan"]')])
# many attributes: an entity map of more than 8 keys is a hash map, and its datoms come in that order
chain('nil', ['[{:db/id 1 ' + ' '.join(':attr%d %d' % (i, i) for i in range(1, 21)) + '}]', '[{' + ' '.join(':k%d "v%d"' % (i, i) for i in range(1, 9)) + '}]', '[{' + ' '.join(':k%d "w%d"' % (i, i) for i in range(1, 10)) + '}]'])
# many tempids: the map of them turns into a hash map
chain(NAME_ID, ['[' + ' '.join('{:db/id %d :name "n%d"}' % (-i, i) for i in range(1, 14)) + ' ' + ' '.join('[:db/add "s%d" :age %d]' % (i, i) for i in range(1, 6)) + ']',
                '[' + ' '.join('{:name "n%d" :age %d}' % (i, i) for i in range(1, 14)) + ' {:name "new"} {:other 1} {:other 2}]', '[{:db/id -1 :name "n1" :v 1} {:db/id -2 :name "n2" :v 2}]'])

# ---- index.cljc and around: reading the indexes
IDX = '{:name {:db/index true} :age {:db/index true} :friend {:db/valueType :db.type/ref} :tag {:db/cardinality :db.cardinality/many :db/index true}}'
IDX_BASE = '[{:db/id 1 :name "Petr" :age 44 :tag [:a :b]} {:db/id 2 :name "Ivan" :age 25 :friend 1 :tag [:b]} {:db/id 3 :name "Sergey" :age 11 :friend 2 :misc 1.5} {:db/id 4 :name "Ivan" :age 44 :misc "str"} {:db/id 5 :misc :kw :age 25}]'
reads = [datoms('eavt'), datoms('eavt', '1'), datoms('eavt', '1', ':name'), datoms('eavt', '1', ':name', '"Petr"'), datoms('eavt', '1', ':name', '"Petr"', '536870913'), datoms('eavt', '1', ':name', '"Nope"'),
         datoms('eavt', '9'), datoms('aevt'), datoms('aevt', ':name'), datoms('aevt', ':name', '2'), datoms('aevt', ':nope'), datoms('avet'), datoms('avet', ':name'), datoms('avet', ':name', '"Ivan"'),
         datoms('avet', ':name', '"Ivan"', '4'), datoms('avet', ':age', '44'), datoms('avet', ':friend', '1'), datoms('avet', ':misc'), datoms('avet', ':tag', ':b'), datoms('eavt', 'nil', ':name'),
         seek('eavt', '2'), seek('eavt', '2', ':friend'), seek('eavt', '2', ':name', '"A"'), seek('eavt', '9'), seek('aevt', ':misc'), seek('avet', ':name', '"J"'), seek('avet', ':age', '25', '3'), seek('avet', ':misc'),
         rseek('eavt', '2'), rseek('eavt', '2', ':friend'), rseek('eavt', '0'), rseek('aevt', ':misc'), rseek('avet', ':name', '"J"'), rseek('avet', ':age', '25', '3'), rseek('eavt'), seek('eavt'),
         irange(':name', '"I"', '"Q"'), irange(':name', '"Ivan"', '"Ivan"'), irange(':age', '20', '44'), irange(':age', '45', '100'), irange(':age', 'nil', '25'), irange(':age', '25', 'nil'), irange(':misc', '1', '2'), irange(':friend', '1', '2'),
         irange(':tag', ':a', ':b'), irange('5', '1', '2'), find('eavt'), find('eavt', '2'), find('eavt', '2', ':name'), find('eavt', '2', ':zzz'), find('eavt', '9'), find('aevt', ':age'), find('avet', ':age', '25'), find('avet', ':age', '26'), find('avet', ':misc'),
         datoms('eavt', '[:name "Ivan"]'), datoms('eavt', ':kw'), datoms('eavt', '"str"'), datoms('eavt', '-1'), datoms('eavt', '1', '5'), datoms('avet', ':friend', '[:name "x"]'), datoms('zzzz'),
         entid('1'), entid('99'), entid('0'), entid('-1'), entid('2147483648'), entid('"s"'), entid('nil'), entid('[:name "Ivan"]'), entid('[:nope 1]'), entid('{:a 1}')]
chain(IDX, [IDX_BASE], reads)
# seeking and reading through more than a leaf of the index
chain('{:n {:db/index true}}', ['[' + ' '.join('{:db/id %d :n %d :s "s%03d"}' % (i, (i * 37) % 101, i) for i in range(1, 300)) + ']'],
      [datoms('avet', ':n', '50'), '{:op :seek-datoms :db %(db)s :index :avet :components [:n 50] :limit 70}', '{:op :rseek-datoms :db %(db)s :index :avet :components [:n 50] :limit 70}',
       '{:op :seek-datoms :db %(db)s :index :eavt :components [150] :limit 5}', '{:op :rseek-datoms :db %(db)s :index :eavt :components [150] :limit 5}', irange(':n', '10', '12'), datoms('eavt', '299'), datoms('aevt', ':s', '64'),
       '{:op :rseek-datoms :db %(db)s :index :aevt :components [:s] :limit 3}', '{:op :seek-datoms :db %(db)s :index :aevt :components [:n 298] :limit 4}'])

# ---- databases as values: equality, hashes, emptying, schemas changed, reading back
case(['{:op :empty-db :as e0}', '{:op :empty-db :schema {:a {:db/index true}} :as e1}', '{:op :db-with :db #r e0 :tx [[:db/add 1 :a 1] [:db/add 2 :a 2]] :as a}',
      '{:op :db-with :db #r e0 :tx [[:db/add 2 :a 2]] :as b0}', '{:op :db-with :db #r b0 :tx [[:db/add 1 :a 1]] :as b}', '{:op :db-with :db #r e1 :tx [[:db/add 1 :a 1] [:db/add 2 :a 2]] :as c}',
      '{:op :db-eq :a #r a :b #r b}', '{:op :db-hash-eq :a #r a :b #r b}', '{:op :db-eq :a #r a :b #r c}', '{:op :db-eq :a #r a :b #r e0}', '{:op :db-empty :db #r c :as c0}', '{:op :db-eq :a #r c0 :b #r e1}',
      '{:op :with-schema :db #r a :schema {:a {:db/index true}} :as a2}', '{:op :db-eq :a #r a2 :b #r c}', '{:op :datoms :db #r a2 :index :avet :components [:a]}', '{:op :db-with :db #r a2 :tx [[:db/add 3 :a 3]] :as a3}',
      '{:op :datoms :db #r a3 :index :avet :components [:a]}', '{:op :with-schema :db #r a :schema 5}', '{:op :db-with :db #r a :tx [] :as a4}', '{:op :db-eq :a #r a :b #r a4}', '{:op :cljs/eq :a #r a :b [1 2]}', '{:op :cljs/hash :val #r e0}', '{:op :cljs/hash :val #r a}',
      '{:op :cljs/pr-str :val #r a}', '{:op :cljs/str :val #r e1}'])
case(['{:op :read-db :string "#datascript/DB {:schema {:aka {:db/cardinality :db.cardinality/many}} :datoms [[1 :aka \\"x\\" 536870913] [1 :name \\"Ivan\\" 536870913] [2 :age 5 536870915]]}" :as db}',
      '{:op :db-with :db #r db :tx [{:name "next"}] :as db2}', '{:op :read-db :string "#datascript/DB {:schema nil :datoms []}"}', '{:op :read-db :string "#datascript/DB {:datoms [[5 :b 1 536870913] [5 :a 1 536870913] [1 :z 1 536870913]]}"}',
      '{:op :init-db :datoms [[3 :ref 77 536870914] [1 :name "x"] [1 :name "x" 536870920] [2 :name "y" 536870913 true]] :schema {:ref {:db/valueType :db.type/ref} :name {:db/index true}} :as i}',
      '{:op :db-with :db #r i :tx [{:name "after"}]}', '{:op :init-db :datoms [] :as i0}', '{:op :init-db :datoms [[1 :a 1]] :schema {:a {:db/unique :bad}}}', '{:op :init-db :datoms [[536870913 :txattr 1 536870913] [5 :a 1 536870913]]}'])

out = sys.argv[1] if len(sys.argv) > 1 else 'conformance/cases/transact.edn'
open(out, 'w').write("; Transactions, after DataScript's own tests and around them. Written by conformance/gen/transact.py.\n" + '\n'.join(lines) + '\n')
print(len(lines), 'cases')
