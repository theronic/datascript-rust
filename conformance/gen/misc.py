#!/usr/bin/env python3
"""Writes conformance/cases/misc.edn: what the other files leave: serialization, diffs of databases, transaction
functions, and older database values staying what they were. After DataScript's own tests (test/datascript/test/
serialize.cljc, issues.cljc, transact.cljc, conn.cljc, listen.cljc) and around them."""
import sys

lines = []
def case(steps): lines.append('[' + ' '.join(steps) + ']')

DB = '#r db'
def setup(schema, tx, name='db'):
    return ['{:op :empty-db :schema %s :as %s0}' % (schema, name), '{:op :db-with :db #r %s0 :tx %s :as %s}' % (name, tx, name)]
def q(query, *inputs): return '{:op :q :query %s :inputs [%s]}' % (query, ' '.join(inputs))
def entity(eid, attrs='[]', touch=False, db=DB): return '{:op :entity :db %s :eid %s :attrs %s%s}' % (db, eid, attrs, ' :touch true' if touch else '')
def datoms(index, *components, db=DB, limit=None):
    return '{:op :datoms :db %s :index :%s :components [%s]%s}' % (db, index, ' '.join(components), ' :limit %d' % limit if limit else '')
def ser(db=DB, name=None): return '{:op :serializable :db %s%s}' % (db, ' :as %s' % name if name else '')
def diff(a, b): return '{:op :diff :a #r %s :b #r %s}' % (a, b)

# ---------------------------------------------------------------- serialize.cljc
SCHEMA = ('{:name {} :aka {:db/cardinality :db.cardinality/many} :age {:db/index true} :follows {:db/valueType :db.type/ref} :email {:db/unique :db.unique/identity} '
          ':avatar {:db/valueType :db.type/ref, :db/isComponent true} :url {} :attach {}}')
DATA = ['[1 :name "Petr"]', '[1 :aka "Devil"]', '[1 :aka "Tupen"]', '[1 :age 15]', '[1 :follows 2]', '[1 :email "petr@gmail.com"]', '[1 :avatar 10]', '[10 :url "http://"]', '[1 :attach {:some-key :some-value}]',
        '[2 :name "Oleg"]', '[2 :age 30]', '[2 :email "oleg@gmail.com"]', '[2 :attach [:just :values]]', '[3 :name "Ivan"]', '[3 :age 15]', '[3 :follows 2]', '[3 :attach {:another :map}]', '[3 :avatar 30]',
        '[4 :name "Nick" 536870912]', '[5 :inf ##Inf]', '[5 :-inf ##-Inf]', '[536870912 :txInstant 3735928559]', '[30 :url "https://"]']
TX = '[' + ' '.join('[:db/add %s]' % d[1:-1].replace(' 536870912]', ']') if d.startswith('[4 ') else '[:db/add %s]' % d[1:-1] for d in DATA) + ']'
case(['{:op :init-db :datoms [%s] :schema %s :as init}' % (' '.join(DATA), SCHEMA), '{:op :empty-db :schema %s :as e}' % SCHEMA, '{:op :db-with :db #r e :tx %s :as db}' % TX,
      '{:op :db-eq :a #r init :b #r db}', '{:op :db-with :db #r init :tx [[:db/add -1 :name "Lex"]] :as i2}', '{:op :db-with :db #r db :tx [[:db/add -1 :name "Lex"]] :as t2}', '{:op :db-eq :a #r i2 :b #r t2}',
      ser(name='back'), '{:op :db-eq :a #r db :b #r back}', '{:op :db-hash-eq :a #r db :b #r back}', ser('#r init', 'back2'), '{:op :db-eq :a #r init :b #r back2}', ser('#r back'),
      '{:op :db-with :db #r back :tx [[:db/add -1 :name "Lex"] [:db/add 1 :aka "X"] [:db/retract 2 :age 30]] :as b3}', '{:op :db-with :db #r db :tx [[:db/add -1 :name "Lex"] [:db/add 1 :aka "X"] [:db/retract 2 :age 30]] :as d3}',
      '{:op :db-eq :a #r b3 :b #r d3}', q('[:find ?e ?v :where [?e :attach ?v]]', '#r back'), entity('1', '[]', True, '#r back'), datoms('avet', ':age', db='#r back'), datoms('avet', ':email', '"oleg@gmail.com"', db='#r back'),
      '{:op :entid :db #r back :eid [:email "petr@gmail.com"]}', '{:op :rschema :db #r back}', '{:op :init-db :datoms [[:add -1 :name "Ivan"] {:add -1 :age 35}] :schema %s}' % SCHEMA])
VALUES = ['"str"', '""', '"multi\\nline \\"quoted\\" \\\\ tab\\t unicode é 😀 \\u0001"', '0', '-0.0', '1.5', '-17', '1e21', '1e-7', '123456789012', '##Inf', '##-Inf', '##NaN', 'true', 'false', ':kw', ':ns/kw', ':a.b/c-d', 'sym', 'ns/sym',
          '[1 2 3]', '[]', '{}', '{:a 1 :b [2 {:c #{3}}]}', '#{1 2}', '#{}', '[nil]', '#uuid "550e8400-e29b-41d4-a716-446655440000"', '#inst "2020-01-02T03:04:05.678-00:00"', '[:kw "s" 1.5 nil true]',
          '{"string key" 1}', '[[1 2] [3 [4 5]]]', '":looks-like-kw"', '"1"', '"true"', '"[0 1]"']
steps = ['{:op :empty-db :schema {:many {:db/cardinality :db.cardinality/many} :idx {:db/index true}} :as e}',
         '{:op :db-with :db #r e :tx [%s] :as db}' % ' '.join('[:db/add %d :v %s] [:db/add %d :many %s] [:db/add 100 :many %s] [:db/add %d :idx %s]' % (i + 1, v, i + 1, v, v, i + 1, v) for i, v in enumerate(VALUES)),
         ser(name='back'), '{:op :db-hash-eq :a #r db :b #r back}', datoms('avet', ':idx', db='#r back'), datoms('aevt', ':many', db='#r back')]
case(steps)
for schema, tx in [('nil', '[]'), ('{}', '[]'), ('{:a {:db/unique :db.unique/identity}}', '[]'), ('nil', '[[:db/add 1 :a 1]]'), ('{"s" {:db/index true}}', '[[:db/add 1 "s" 1] [:db/add 1 "t" :kw] [:db/add 2 "u" "x"]]'),
                   ('{:t {:db/tupleAttrs [:a :b]} :r {:db/valueType :db.type/ref :db/cardinality :db.cardinality/many}}', '[{:db/id 1 :a 1 :b "x" :r [2 3]} {:db/id 2 :a 3} {:db/id 3 :b 2}]'),
                   ('{:a {:db/index true}}', '[[:db/add 1 :a 1 536870999] [:db/add 2 :a 2 536870915] {:db/id :db/current-tx :note "n"}]'),
                   ('nil', '[' + ' '.join('[:db/add %d :k%d :v%d]' % (i, i % 13, i % 7) for i in range(1, 80)) + ']')]:
    case(setup(schema, tx) + [ser(name='back'), '{:op :db-eq :a #r db :b #r back}', '{:op :db-with :db #r back :tx [{:new "entity"}]}', '{:op :db-with :db #r db :tx [{:new "entity"}]}'])
# reading what was not written by it
GOOD = '{\\"count\\":1,\\"tx0\\":536870912,\\"max-eid\\":1,\\"max-tx\\":536870913,\\"schema\\":\\"nil\\",\\"attrs\\":[\\":a\\"],\\"keywords\\":[],\\"eavt\\":[[1,0,%s,1]],\\"aevt\\":[0],\\"avet\\":[]}'
case(['{:op :from-serializable :json "%s"}' % (GOOD % v) for v in ['1', '\\"s\\"', 'true', '[0,0]', '[1,\\"{:k 1}\\"]', '[2]', '[3]', '[4]', '[5]', '[9,1]', '[]', 'null', '{}', '[1,\\"not edn ((\\"]', '[1,5]']]
     + ['{:op :from-serializable :json "%s"}' % j for j in [
         '{}', '[]', 'null', '5', '{\\"tx0\\":536870912}', 'not json',
         '{\\"count\\":0,\\"tx0\\":536870912,\\"max-eid\\":0,\\"max-tx\\":536870912,\\"schema\\":\\"nil\\",\\"attrs\\":[],\\"keywords\\":[],\\"eavt\\":[],\\"aevt\\":[],\\"avet\\":[]}',
         '{\\"count\\":0,\\"tx0\\":536870912,\\"max-eid\\":7,\\"max-tx\\":536870920,\\"schema\\":\\"{:a {:db/cardinality :db.cardinality/many}}\\",\\"attrs\\":[],\\"keywords\\":[],\\"eavt\\":[],\\"aevt\\":[],\\"avet\\":[]}',
         '{\\"count\\":0,\\"tx0\\":536870912,\\"max-eid\\":0,\\"max-tx\\":536870912,\\"schema\\":\\"{:a {:db/cardinality :nope}}\\",\\"attrs\\":[],\\"keywords\\":[],\\"eavt\\":[],\\"aevt\\":[],\\"avet\\":[]}',
         '{\\"count\\":2,\\"tx0\\":100,\\"max-eid\\":2,\\"max-tx\\":536870913,\\"schema\\":\\"nil\\",\\"attrs\\":[\\":a\\",\\"b\\"],\\"keywords\\":[\\":k\\",\\"s\\"],\\"eavt\\":[[1,0,[0,0],1],[2,1,[0,1],5]],\\"aevt\\":[0,1],\\"avet\\":[]}',
         '{\\"count\\":1,\\"tx0\\":536870912,\\"max-eid\\":1,\\"max-tx\\":536870913,\\"schema\\":\\"nil\\",\\"attrs\\":[\\":a\\"],\\"keywords\\":[],\\"eavt\\":[[1,3,1,1]],\\"aevt\\":[0],\\"avet\\":[]}',
         '{\\"count\\":1,\\"tx0\\":536870912,\\"max-eid\\":1,\\"max-tx\\":536870913,\\"schema\\":\\"nil\\",\\"attrs\\":[\\":a\\"],\\"keywords\\":[],\\"eavt\\":[[1,0,[0,3],1]],\\"aevt\\":[0],\\"avet\\":[]}',
     ]])

# ---------------------------------------------------------------- diffs
D = setup('{:aka {:db/cardinality :db.cardinality/many}}', '[{:db/id 1 :name "Ivan" :age 15 :aka ["a" "b"]} {:db/id 2 :name "Petr"} {:db/id 3 :name "Oleg" :age 3}]', 'a')
D += ['{:op :db-with :db #r a :tx [[:db/add 1 :age 16] [:db/retract 2 :name "Petr"] [:db/add 4 :name "New"] [:db/add 1 :aka "c"]] :as b}', '{:op :db-with :db #r a :tx [] :as same}',
      '{:op :db-with :db #r a :tx [[:db/add 1 :name "Ivan"]] :as touched}', '{:op :empty-db :schema {:aka {:db/cardinality :db.cardinality/many}} :as empty}', '{:op :empty-db :as bare}',
      '{:op :filter :db #r a :pred #f f-all :as fall}', '{:op :filter :db #r a :pred #f f-even-e :as feven}', '{:op :db-with :db #r a0 :tx [[:db/add 1 :name :Ivan] [:db/add 1 :age "15"] [:db/add 2 :name "Petr"]] :as typed}',
      '{:op :db-with :db #r bare :tx [[:db/add 1 "name" "Ivan"] [:db/add 1 "zzz" 1] [:db/add 5 "name" "x"]] :as strs}']
D += [diff(x, y) for (x, y) in [('a', 'b'), ('b', 'a'), ('a', 'a'), ('a', 'same'), ('a', 'touched'), ('a', 'empty'), ('empty', 'a'), ('empty', 'bare'), ('bare', 'empty'), ('a', 'fall'), ('fall', 'a'), ('a', 'feven'),
                                ('feven', 'a'), ('feven', 'feven'), ('a', 'typed'), ('typed', 'a'), ('a', 'strs'), ('strs', 'a'), ('strs', 'bare'), ('bare', 'bare')]]
D += ['{:op :diff :a #r a :b 5}', '{:op :diff :a 5 :b #r a}']
case(D)
case(setup('nil', '[[:db/add 1 :attr :aa]]', 'x') + setup('nil', '[[:db/add 1 :attr "aa"]]', 'y') + [diff('x', 'y'), diff('y', 'x')])

# ---------------------------------------------------------------- transaction functions
NAME = '{:name {:db/unique :db.unique/identity}}'
def quiet(schema, txs, name='db'): return '{:op :conn-quiet :schema %s :txs [%s] :as %s}' % (schema, ' '.join(txs), name)
ALL = q('[:find ?e ?a ?v :where [?e ?a ?v] [(!= ?a :db/fn)]]', DB)
case([quiet(NAME, ['[{:db/id 1 :name "Petr" :age 31} [:db/add 1 :aka "Devil"] [:db/add 1 :aka "Tupen"]]', '[[:db.fn/call #f tx-inc-age "Bob"]]', '[[:db.fn/call #f tx-inc-age "Petr"]]',
                   '[[:db.fn/call #f tx-oleg]]', '[[:db.fn/call #f tx-vera]]', '[[:db.fn/call #f tx-nothing]]', '[[:db.fn/call #f tx-nil]]', '[[:db.fn/call #f tx-throw]]', '[[:db.fn/call #f tx-add 1 :x 1] [:db.fn/call #f tx-add 1 :y 2]]',
                   '[[:db.fn/call #f tx-inc 1 :age 10] [:db.fn/call #f tx-inc 1 :age 10]]', '[[:db.fn/call #f tx-count 1 :count]]', '[[:db.fn/call #f tx-entity {:name "Ent" :db/id -5}] [:db/add -5 :via "tempid"]]',
                   '[[:db.fn/call #f tx-nested 1]]', '[[:db.fn/call #f tx-q]]', '[[:db.fn/call]]', '[[:db.fn/call nil]]', '[[:db.fn/call 5]]', '[[:db.fn/call :kw 1]]', '[[:db.fn/call #f tx-add]]', '[[:db.fn/call #f always]]',
                   '[[:db/add 1 :before 1] [:db.fn/call #f tx-throw] [:db/add 1 :after 1]]', '[[:db/add 1 :b 1] [:db.fn/call #f tx-count 1 :count2] [:db/add 1 :c 1]]']),
      ALL, entity('1', '[]', True)])
case([quiet(NAME, ['[{:db/id 1 :name "Petr" :age 31 :db/ident :Petr} {:db/ident :inc-age :db/fn #f tx-inc-age} {:db/ident :add :db/fn #f tx-add} {:db/ident :not-fn :db/fn 5}]', '[[:unknown-fn]]', '[[:Petr]]',
                   '[[:inc-age "Bob"]]', '[[:inc-age "Petr"]]', '[[:add 1 :via :ident]]', '[[:add 1 :a 1] [:inc-age "Petr"] [:add 1 :b 2]]', '[[:not-fn]]', '[[:inc-age]]', '[[:db/add 1 :x 1] [:unknown-fn] [:db/add 1 :y 1]]',
                   '[["str-op" 1 :a 1]]', '[[nil 1 :a 1]]', '[[5 1 :a 1]]', '[[:db/unknown 1 :a 1]]', '[[:db.fn/unknown 1 :a 1]]']),
      ALL, entity('1', '[]', True), '{:op :entid :db #r db :eid :inc-age}', '{:op :entid :db #r db :eid :Petr}', '{:op :entid :db #r db :eid :nope}'])
case([quiet('nil', ['[[:db.fn/call #f tx-oleg]]'], 'db'), ALL, '{:op :conn :db #r db :txs [[[:db.fn/call #f tx-oleg]] [[:db.fn/call #f tx-vera] [:db.fn/call #f tx-vera]] [[:db.fn/call #f tx-entity {:foo "bar"}]]]}'])

# ---------------------------------------------------------------- older values stay what they were (issue-373)
EVENS = range(0, 1024, 2)
case(setup('{:vs {:db/cardinality :db.cardinality/many}}', '[{:db/id 1 :vs [%s]}]' % ' '.join(map(str, EVENS)), 'd')
     + ['{:op :db-with :db #r d :tx [%s] :as later}' % ' '.join(['[:db/add 1 :vs %d]' % v for v in range(1, 256, 2)] + ['[:db/retract 1 :vs %d]' % v for v in range(0, 256, 2)] + ['[:db/add 1 :vs %d]' % v for v in range(512, 768)]),
        datoms('eavt', db='#r d'), datoms('aevt', db='#r d'), datoms('eavt', '1', ':vs', '300', db='#r d'), datoms('aevt', ':vs', '1', '2', db='#r d'), datoms('eavt', '1', ':vs', '2', db='#r later'),
        datoms('eavt', '1', ':vs', '3', db='#r later'), '{:op :db-with :db #r later :tx [[:db/retractEntity 1]] :as gone}', datoms('eavt', db='#r gone'), datoms('eavt', db='#r later', limit=5), datoms('eavt', db='#r d', limit=5)])
case(setup('{:login {:db/unique :db.unique/identity}}', '[%s]' % ' '.join('{:db/id %d :login %d}' % (v // 2 + 1, v) for v in EVENS), 'd')
     + ['{:op :db-with :db #r d :tx [%s] :as later}' % ' '.join(['{:login %d}' % v for v in range(1, 256, 2)] + ['[:db/retract %d :login %d]' % (v // 2 + 1, v) for v in range(0, 256, 2)] + ['{:login %d}' % v for v in range(1025, 1536, 2)]),
        datoms('avet', ':login', db='#r d'), '{:op :entid :db #r d :eid [:login 100]}', '{:op :entid :db #r d :eid [:login 1022]}', '{:op :entid :db #r later :eid [:login 100]}', '{:op :entid :db #r later :eid [:login 101]}'])
case(setup('{:name {:db/unique :db.unique/identity} :vs {:db/cardinality :db.cardinality/many}}', '[{:db/id 1 :name "target"} %s]' % ' '.join('[:db/add 2 :vs %d]' % v for v in EVENS), 'd')
     + ['{:op :with :db #r d :tx [%s] :as later}' % ' '.join(['[:db/add 2 :vs %d]' % v for v in range(1, 256, 2)] + ['[:db/retract 2 :vs %d]' % v for v in range(0, 256, 2)] + ['[:db/add -1 :age 37]', '[:db/add -1 :name "target"]']),
        datoms('aevt', ':age', db='#r later'), datoms('eavt', '2', ':vs', db='#r d'), '{:op :entid :db #r d :eid [:name "target"]}', entity('1', '[:age]', db='#r later')])

with open(sys.argv[1] if len(sys.argv) > 1 else 'conformance/cases/misc.edn', 'w') as f:
    f.write('; Serialization, diffs, transaction functions and older values. Written by conformance/gen/misc.py.\n')
    f.write('\n'.join(lines) + '\n')
print('%d cases' % len(lines))
