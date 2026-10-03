#!/usr/bin/env python3
"""Writes conformance/cases/pull.edn: the pull API, entities and filtered databases, after DataScript's own tests
(test/datascript/test/pull_api.cljc, pull_parser.cljc, entity.cljc, filter.cljc, components.cljc) and around them."""
import random
import sys

lines = []
def case(steps): lines.append('[' + ' '.join(steps) + ']')

DB = '#r db'
def pull(pattern, eid, db=DB): return '{:op :pull :db %s :pattern %s :eid %s}' % (db, pattern, eid)
def pull_many(pattern, eids, db=DB): return '{:op :pull-many :db %s :pattern %s :eids %s}' % (db, pattern, eids)
def visit(pattern, eid, db=DB): return '{:op :pull-visit :db %s :pattern %s :eid %s}' % (db, pattern, eid)
def pcount(pattern, eid, db=DB): return '{:op :pull-count :db %s :pattern %s :eid %s}' % (db, pattern, eid)
def parse(pattern, db=DB): return '{:op :parse-pull :db %s :pattern %s}' % (db, pattern)
def entity(eid, attrs='[]', touch=False, db=DB): return '{:op :entity :db %s :eid %s :attrs %s%s}' % (db, eid, attrs, ' :touch true' if touch else '')
def path(eid, p, db=DB): return '{:op :entity-path :db %s :eid %s :path %s}' % (db, eid, p)
def q(query, *inputs): return '{:op :q :query %s :inputs [%s]}' % (query, ' '.join(inputs))
def datoms(index, *components, db=DB): return '{:op :datoms :db %s :index :%s :components [%s]}' % (db, index, ' '.join(components))
def seek(index, *components, db=DB): return '{:op :seek-datoms :db %s :index :%s :components [%s]}' % (db, index, ' '.join(components))
def rseek(index, *components, db=DB): return '{:op :rseek-datoms :db %s :index :%s :components [%s]}' % (db, index, ' '.join(components))
def flt(pred, src='db', name='f'): return '{:op :filter :db #r %s :pred #f %s :as %s}' % (src, pred, name)

def init(datoms, schema, name='db'): return ['{:op :init-db :datoms [%s] :schema %s :as %s}' % (' '.join(datoms), schema, name)]
def setup(schema, tx, name='db'):
    return ['{:op :empty-db :schema %s :as %s0}' % (schema, name), '{:op :db-with :db #r %s0 :tx %s :as %s}' % (name, tx, name)]

# ---------------------------------------------------------------- pull_api.cljc
SCHEMA = ('{:name {:db/unique :db.unique/identity} :aka {:db/cardinality :db.cardinality/many} :child {:db/cardinality :db.cardinality/many :db/valueType :db.type/ref} '
          ':friend {:db/cardinality :db.cardinality/many :db/valueType :db.type/ref} :enemy {:db/cardinality :db.cardinality/many :db/valueType :db.type/ref} :father {:db/valueType :db.type/ref} '
          ':part {:db/valueType :db.type/ref :db/isComponent true :db/cardinality :db.cardinality/many} :spec {:db/valueType :db.type/ref :db/isComponent true :db/cardinality :db.cardinality/one}}')
DATOMS = ['[1 :name "Petr"]', '[1 :aka "Devil"]', '[1 :aka "Tupen"]', '[2 :name "David"]', '[3 :name "Thomas"]', '[4 :name "Lucy"]', '[5 :name "Elizabeth"]', '[6 :name "Matthew"]', '[7 :name "Eunan"]',
          '[8 :name "Kerri"]', '[9 :name "Rebecca"]', '[1 :child 2]', '[1 :child 3]', '[2 :father 1]', '[3 :father 1]', '[6 :father 3]', '[10 :name "Part A"]', '[11 :name "Part A.A"]', '[10 :part 11]',
          '[12 :name "Part A.A.A"]', '[11 :part 12]', '[13 :name "Part A.A.A.A"]', '[12 :part 13]', '[14 :name "Part A.A.A.B"]', '[12 :part 14]', '[15 :name "Part A.B"]', '[10 :part 15]',
          '[16 :name "Part A.B.A"]', '[15 :part 16]', '[17 :name "Part A.B.A.A"]', '[16 :part 17]', '[18 :name "Part A.B.A.B"]', '[16 :part 18]']
PATTERNS = [
    ('[:name :aka]', '1'), ('[:name :father :db/id]', '6'), ('[:name :_child]', '2'), ('[:name {:_child [:name]}]', '2'), ('[:name :_father]', '3'), ('[:name :_father]', '1'),
    ('[:name {:_father [:name]}]', '3'), ('[:name {:_father [:name]}]', '1'), ('[:name :_father :_child]', '1'), ('[:name :part]', '10'), ('[:name :_part]', '11'), ('[:name {:_part [:name]}]', '11'),
    ('[:name {:_part ...}]', '14'), ('[:name {:_part 2}]', '14'), ('[*]', '1'), ('[* :_child]', '2'), ('[:name *]', '1'), ('[:aka :name *]', '1'), ('[:aka :child :name *]', '1'),
    ('[[:aka :as :alias] [:name :as :first-name] *]', '1'), ('[* {:child ...}]', '1'), ('[:foo]', '1'), ('[(default :foo "bar")]', '1'), ('[[:foo :default "bar"]]', '1'), ('[[:foo :default false]]', '1'),
    ('[[:foo :as :bar :default false]]', '1'), ('[[:name :default "[name]"] [:aka :default "[aka]"] {[:child :default "[child]"] ...}]', '1'),
    ('[[:name :default "[name]"] [:aka :default "[aka]"] {[:child :default "[child]"] ...}]', '2'), ('[:db/id [:child :default 1]]', '2'), ('[:db/id [:_child :default 2]]', '1'),
    ('[[:name :as "Name"] [:aka :as :alias]]', '1'), ('[[:x :as "Name" :default "Nothing"]]', '1'), ('[:name {:father [:name]}]', '6'), ('[:name {:child [:name]}]', '1'), ('[:name {:father [:name]}]', '1'),
    ('[:name {:child [:foo]}]', '1'), ('[:name {:part [:name]}]', '10'), ('[:name {:part 1}]', '10'), ('[:name :aka]', '[:name "Petr"]'), ('[:name :aka]', '[:name "NotInDatabase"]'), ('[*]', '[:name "No such name"]'),
    ('[[:db/id :xform vector] [:name :xform vector] [:aka :xform vector] {[:child :xform vector] ...}]', '1'),
    ('[[:db/id :xform #f x-vector] [:name :xform #f x-vector] [:aka :xform #f x-vector] {[:child :xform #f x-vector] ...}]', '1'),
    ('[:name {[:father :xform #f get-name] [*]}]', '2'), ('[:name {[:_father :xform #f names] [:name]}]', '1'), ('[:name {[:_part :xform #f get-name] [:name]}]', '11'),
    ('[[:normal :xform vector] [:aka :xform vector] {[:child :xform vector] ...}]', '2'), ('[[:unknown :default "[unknown]" :xform vector]]', '1'),
    # around the tests
    ('[]', '1'), ('[:db/id]', '1'), ('[:db/id]', '99'), ('[:name]', '99'), ('[*]', '99'), ('[*]', '9'), ('[:name]', 'nil'), ('[:name]', '"str"'), ('[:name]', ':kw'), ('[:name]', '[:name]'), ('[:name]', '[:aka "Devil"]'),
    ('[:name]', '{:db/id 1}'), ('[:name]', '1.5'), ('[:name]', '-1'), ('[:name]', '0'), (':name', '1'), ('nil', '1'), ('"*"', '1'), ('["*"]', '1'), ('[:*]', '1'), ('(:name :aka)', '1'), ('[(:name)]', '1'), ('[[:name]]', '1'),
    ('[:name :name]', '1'), ('[:name [:name :as :n2]]', '1'), ('[[:name :as :n1] [:name :as :n2]]', '1'), ('[[:name :as :aka] :aka]', '1'), ('[:aka [:name :as :aka]]', '1'), ('[[:aka :limit 1]]', '1'),
    ('[(limit :aka 1)]', '1'), ('[("limit" :aka 1)]', '1'), ('[[:aka :limit nil]]', '1'), ('[[:aka :limit 1 :as :one] [:aka :as :all]]', '1'), ('[[:aka :limit 0]]', '1'), ('[[:aka :limit -1]]', '1'),
    ('[[:aka :limit 1.5]]', '1'), ('[[:name :limit 1]]', '1'), ('[[:child :limit 1]]', '1'), ('[{[:child :limit 1] [:name]}]', '1'), ('[{(limit :child 1) [:name]}]', '1'), ('[{:child [:name] :father [:name]}]', '2'),
    ('[{:child [:name]} {:father [:name]}]', '2'), ('[{:child [:name]} {:child [:db/id]}]', '1'), ('[:child {:child [:name]}]', '1'), ('[{:child [:name]} :child]', '1'), ('[{:child ...} :child]', '1'),
    ('[:name {:child 1}]', '1'), ('[:name {:child 0}]', '1'), ('[:name {:child -1}]', '1'), ('[:name {:child 2}]', '1'), ('[:name {:child "..."}]', '1'), ('[:name {:child ...} {:father ...}]', '1'),
    ('[:name {:father ...} {:child ...}]', '2'), ('[:name {:father ... :child ...}]', '2'), ('[:name {:father 1 :child 1}]', '2'), ('[:name {:father 2 :child 1}]', '6'), ('[:name {:_father ...}]', '1'),
    ('[:name {:_father 1}]', '1'), ('[:name {:_child ...}]', '6'), ('[:name {:father [:name {:father [:name {:_child [:name]}]}]}]', '6'), ('[:name {:name [:name]}]', '1'), ('[:name {:aka [:name]}]', '1'),
    ('[:_name]', '1'), ('[:_aka]', '1'), ('[:_child :_father :_part :_spec]', '2'), ('[:_child :_father :_part :_spec]', '11'), ('[:_part]', '11'), ('[{:_part [*]}]', '11'), ('[{:_part [:name :_part]}]', '13'),
    ('[:part]', '10'), ('[:part]', '13'), ('[{:part [:db/id]}]', '10'), ('[{:part ...}]', '10'), ('[{:part 1}]', '10'), ('[{:part 2}]', '10'), ('[* {:part 1}]', '10'), ('[:spec]', '10'), ('[:name :spec :part]', '16'),
    ('[[:part :limit 1]]', '10'), ('[{[:part :limit 1] ...}]', '10'), ('[[:part :as :parts :limit 1]]', '10'), ('[[:part :default :none]]', '13'), ('[[:part :xform #f x-count]]', '10'), ('[[:aka :xform #f x-count]]', '1'),
    ('[[:aka :xform count]]', '1'), ('[[:name :xform count]]', '1'), ('[[:name :xform str]]', '1'), ('[[:name :xform nope]]', '1'), ('[[:name :xform :kw]]', '1'), ('[[:name :xform #f nil-fn]]', '1'),
    ('[[:name :xform #f throw]]', '1'), ('[[:nope :xform #f throw]]', '1'), ('[[:nope :xform #f always]]', '1'), ('[[:nope :xform #f nil-fn]]', '1'), ('[[:db/id :xform str]]', '1'), ('[[:db/id :as :id]]', '1'),
    ('[[:db/id :default 5]]', '1'), ('[[:child :xform #f names]]', '1'), ('[{[:child :xform #f names] [:name]}]', '1'), ('[{[:child :xform #f nil-fn] [:name]}]', '1'), ('[{[:father :xform #f nil-fn] [:name]}]', '2'),
    ('[{[:_father :xform #f nil-fn] [:name]}]', '1'), ('[{[:_father :xform #f x-count] [:name]}]', '1'), ('[{[:_father :as :kids] [:name]}]', '1'), ('[[:_father :as :kids]]', '1'), ('[[:_father :default []]]', '9'),
    ('[[:_father :limit 1]]', '1'), ('[{[:_father :limit 1] [:name]}]', '1'), ('[(limit :_father 1)]', '1'), ('[(default :_father :none)]', '9'), ('[:name "aka"]', '1'), ('["name"]', '1'), ('[":db/id"]', '1'),
    ('[:name 5]', '1'), ('[:name nil]', '1'), ('[:name {}]', '1'), ('[:name #{:aka}]', '1'), ('[:name {:child :name}]', '1'), ('[:name {:child nil}]', '1'), ('[:name {:child []}]', '1'), ('[:name {:child [*]}]', '1'),
    ('[:name {:child [* {:father [*]}]}]', '1'), ('[{:child [:name {:father [:name {:child [:name]}]}]}]', '1'), ('[* {:child [*]} {:_father [*]}]', '1'), ('[* :_father]', '3'), ('[* [:name :as :n]]', '1'),
    ('[* [:name :default "x"]]', '4'), ('[* [:zzz :default "x"]]', '4'), ('[* [:aaa :default "x"]]', '4'), ('[* :zzz :aaa]', '4'), ('[* [:name :xform str] [:aka :limit 1]]', '1'), ('[* {:child 1}]', '1'),
    ('[* {:father ...}]', '6'), ('[:aaa :name :zzz]', '4'), ('[:aaa [:name :as :n] :zzz :father]', '6'), ('[[:aka :default []] [:name :default ""] [:zzz :default 0]]', '2'),
]
steps = init(DATOMS, SCHEMA)
steps += [pull(p, e) for (p, e) in PATTERNS]
steps += [pull_many('[:name]', '[1 5 7 9]'), pull_many('[:aka]', '[[:name "Elizabeth"] [:name "Petr"] [:name "Eunan"] [:name "Rebecca"] [:name "Unknown"]]'), pull_many('[*]', '[1 99 2 1]'),
          pull_many('[:name]', '[]'), pull_many('[:name]', 'nil'), pull_many('[:name]', '#{1 2}'), pull_many('[:name]', '(3 2 1)'), pull_many('[:name]', '5'), pull_many('[:bad {}]', '[]'), pull_many(':bad', '[1]'),
          pull_many('[:name]', '[1 :kw 2]'), pull_many('[:name]', '[1 [:aka "x"] 2]')]
case(steps)
case(init(DATOMS, SCHEMA) + [visit(p, e) for (p, e) in [
    ('[:name]', '1'), ('[:name :aka]', '1'), ('[:db/id]', '1'), ('[:db/id :name]', '1'), ('[*]', '1'), ('[:missing]', '1'), ('[* :missing]', '1'), ('[[:missing :default 10]]', '1'), ('[[:child :default 10]]', '2'),
    ('[:child]', '1'), ('[{:child [:name]}]', '1'), ('[:name {:child ...}]', '1'), ('[:name :_child]', '2'), ('[:name :part]', '10'), ('[* {:child [*]}]', '1'), ('[:name :_part :_child :_father]', '11'),
    ('[{:_father [:name :_father]}]', '1'), ('[[:aka :limit 1] :name]', '1'), ('[:name]', '99'), ('[:name]', '[:name "Petr"]'), ('[:aaa :zzz]', '1'), ('[* :aaa :zzz]', '9'), ('[{:part 1}]', '10'), ('[{:_part ...}]', '14')]])

# pull-limit
LIMIT = DATOMS + ['[4 :friend 5]', '[4 :friend 6]', '[4 :friend 7]', '[4 :friend 8]'] + ['[8 :aka "aka-%d"]' % i for i in range(2000)]
case(init(LIMIT, SCHEMA) + [pull('[[:aka :xform count]]', '8'), pull('[[:aka :limit 500 :xform count]]', '8'), pull('[[(limit :aka 500) :xform count]]', '8'), pull('[[(limit :aka 1500) :xform count]]', '8'),
                           pull('[[(limit :aka nil) :xform count]]', '8'), pull('[:name {(limit :friend 2) [:name]}]', '4'), pull('[(limit :aka 3)]', '8'), pull('[[:aka :limit 3] :name]', '8'),
                           pull('[[:aka :limit 3 :as :a3] [:aka :limit 2 :as :a2] :name]', '8'), pull('[* [:aka :limit 2]]', '8'), pull('[:name {[:friend :limit 1] [:name]} :aka]', '4'),
                           pull('[{[:friend :limit 3] [:name [:aka :limit 2]]}]', '4'), pull('[[:friend :limit 2]]', '4'), pull('[[:friend :limit 2 :xform #f x-count]]', '4')])

# recursion
REC = ['{:op :db-with :db #r db :tx [[:db/add 4 :friend 5] [:db/add 5 :friend 6] [:db/add 6 :friend 7] [:db/add 7 :friend 8] [:db/add 4 :enemy 6] [:db/add 5 :enemy 7] [:db/add 6 :enemy 8] [:db/add 7 :enemy 4]] :as rec}',
       '{:op :db-with :db #r rec :tx [[:db/add 8 :friend 4]] :as cyc}', '{:op :db-with :db #r db :tx [[:db/add 12 :part 10]] :as recdb}']
case(init(DATOMS, SCHEMA) + REC + [pull(p, e, '#r rec') for (p, e) in [
    ('[:db/id :name {:friend ...}]', '4'), ('[:db/id :name {:friend 2 :enemy 2}]', '4'), ('[:db/id {:_friend ...}]', '8'), ('[:db/id {:_friend 2}]', '8'), ('[:db/id :name {:friend 1 :enemy 1}]', '4'),
    ('[:db/id :name {:friend 3 :enemy 1}]', '4'), ('[:db/id :name {:friend 1 :enemy 3}]', '4'), ('[:db/id :name {:friend ... :enemy ...}]', '4'), ('[:db/id {:friend ...} {:enemy 1}]', '4'),
    ('[:db/id {:friend 1} {:_enemy ...}]', '4'), ('[:name {:friend ...} {:_friend ...}]', '6'), ('[* {:friend ...}]', '4'), ('[* {:friend 2} {:_friend 2}]', '6'), ('[{[:friend :as :f] 2}]', '4'),
    ('[{[:friend :limit 1] ...}]', '4'), ('[{[:friend :xform #f x-count] ...}]', '4'), ('[{[:friend :default :none] ...} :db/id]', '4'), ('[:db/id {[:friend :as :f] ... [:enemy :as :e] 1}]', '4')]]
     + [pull(p, e, '#r cyc') for (p, e) in [('[:db/id :name {:friend ...}]', '4'), ('[:db/id {:friend ...}]', '8'), ('[:db/id {:friend 3}]', '4'), ('[:db/id {:friend 10}]', '4'), ('[:db/id {:_friend ...}]', '4'),
                                            ('[:db/id {:friend ... :enemy ...}]', '4'), ('[* {:friend ...}]', '5'), ('[:name {:friend 100 :_friend 100}]', '5')]]
     + [pull('[:name :part]', '10', '#r recdb'), pull('[* {:part ...}]', '10', '#r recdb'), pull('[:name {:part ...}]', '12', '#r recdb'), pull('[:name {:_part ...}]', '12', '#r recdb'), pull('[*]', '12', '#r recdb'),
        pull('[:name :_part]', '10', '#r recdb')])
case(setup('{:friend {:db/valueType :db.type/ref} :enemy {:db/valueType :db.type/ref}}', '[{:db/id 1 :name "1" :friend 2 :enemy 2} {:db/id 2 :name "2"}]') + [pull('[:name {:friend [:name], :enemy [:name]}]', '1'), pull('[* {:friend [*] :enemy [*]}]', '1')])
case(setup('{:friend {:db/valueType :db.type/ref} :enemy {:db/valueType :db.type/ref}}', '[{:db/id 1 :friend 2} {:db/id 2 :enemy 3} {:db/id 3 :friend 4} {:db/id 4 :enemy 5} {:db/id 5 :friend 6} {:db/id 6 :enemy 7}]')
     + [pull(p, '1') for p in ['[:db/id {:friend ...}]', '[:db/id {:friend 1 :enemy 1}]', '[:db/id {:friend 2 :enemy 1}]', '[:db/id {:friend 2 :enemy 2}]', '[:db/id {:friend ... :enemy ...}]', '[:db/id {:friend 1 :enemy ...}]']])
case(setup('{:part {:db/valueType :db.type/ref} :spec {:db/valueType :db.type/ref}}', '[[:db/add 1 :part 2] [:db/add 2 :part 3] [:db/add 3 :part 1] [:db/add 1 :spec 2] [:db/add 2 :spec 1]]')
     + [pull(p, '1') for p in ['[:db/id {:part ...} {:spec ...}]', '[:db/id {:part 2} {:spec 2}]', '[:db/id {:part ...}]', '[:db/id {:spec 1}]', '[* {:part ...}]']])
# deep recursion: as deep as memory, not as the machine's stack
DEEP = DATOMS + ['[100 :name "Person-100"]'] + [x for i in range(101, 3000) for x in ('[%d :name "Person-%d"]' % (i, i), '[%d :friend %d]' % (i - 1, i))]
def depth(pattern, eid, key): return '{:op :pull-depth :db #r db :pattern %s :eid %s :key %s}' % (pattern, eid, key)
case(init(DEEP, SCHEMA) + [depth('[:name {:friend ...}]', '100', ':friend'), pull('[{:friend ...}]', '2990'), depth('[:name {:_friend ...}]', '2999', ':_friend'), pull('[:name {:friend 5}]', '100'),
                           depth('[:name {:friend 2000}]', '100', ':friend'), depth('[* {:friend ...}]', '100', ':friend')])
case(setup('{:ref {:db/valueType :db.type/ref :db/isComponent true}}', '[{:name "1" :ref {:name "2" :ref {:name "3"}}}]')
     + [pull('[:name {:ref [:name {:ref [:name {:_ref [:name]}]}]}]', '1'), pull('[*]', '1'), pull('[:name :ref]', '1'), pull('[:name :_ref]', '3'), pull('[:name {:_ref ...}]', '3'), pull('[* :_ref]', '2')])

# other databases: filtered, with string attributes
OTHER = init(DATOMS, SCHEMA) + [flt('f-not-tupen'), flt('f-even-e', name='even'), flt('f-not-name', name='noname'), flt('f-none', name='none'), flt('f-all', name='all'), flt('f-throw', name='thr')]
for d in ['#r f', '#r even', '#r noname', '#r none', '#r all', '#r thr']:
    OTHER += [pull(p, e, d) for (p, e) in [('[:name :aka]', '1'), ('[*]', '1'), ('[:name :_child]', '2'), ('[:name {:child [:name]}]', '1'), ('[:name :part]', '10'), ('[* {:part ...}]', '10'), ('[:name {:_part ...}]', '14'),
                                           ('[:name :part]', '4'), ('[:aka :father :name]', '4'), ('[:aaa]', '4'), ('[:zzz]', '4'), ('[:name :part]', '2'), ('[:name :zzz]', '2'), ('[:father :part]', '2'),
                                           ('[:child]', '2'), ('[{:child [:name]}]', '9'), ('[:name]', '[:name "Petr"]'), ('[:db/id]', '4'), ('[]', '4'), ('[:_father]', '1'), ('[{:_father [:name]}]', '1')]]
    OTHER += [pull_many('[:name :part]', '[1 4 10 99]', d)]
OTHER += [pcount(p, e) for (p, e) in [('[:name]', '1'), ('[:name :aka]', '1'), ('[*]', '1'), ('[:name :part]', '10'), ('[:name]', '18'), ('[:aaa]', '1'), ('[:zzz]', '1'), ('[:db/id]', '1'), ('[]', '1'), ('[:name {:child [:name]}]', '1'),
                                      ('[:_father]', '1'), ('[:name]', '99'), ('[* {:part ...}]', '10')]]
OTHER += [visit('[:name :part]', '4', '#r f'), visit('[*]', '1', '#r f'), visit('[:name :_child]', '2', '#r even')]
case(OTHER)
STR_SCHEMA = '{"aka" {:db/cardinality :db.cardinality/many} "friend" {:db/valueType :db.type/ref :db/cardinality :db.cardinality/many} ":strange" {:db/valueType :db.type/ref} "ns/name" {:db/unique :db.unique/identity}}'
case(setup(STR_SCHEMA, '[{:db/id 1 "name" "Ivan" "aka" ["X" "Y"] "friend" [2 3] "ns/name" "i" ":strange" 2} {:db/id 2 "name" "Petr" "ns/name" "p"} {:db/id 3 "name" "Oleg" "friend" [1]}]')
     + [pull(p, e) for (p, e) in [('["name" "aka"]', '1'), ('["*"]', '1'), ('[*]', '1'), ('["name" {"friend" ["name"]}]', '1'), ('["name" "_friend"]', '2'), ('["name" {"_friend" ["name"]}]', '1'),
                                  ('["name" {"friend" "..."}]', '1'), ('["name" {"friend" 1}]', '1'), ('[["name" :as "n"] ["aka" :limit 1]]', '1'), ('[("limit" "aka" 1) ("default" "nope" 0)]', '1'),
                                  ('[":db/id" "name"]', '1'), ('[:db/id "name"]', '1'), ('["name"]', '["ns/name" "p"]'), ('["ns/name" "ns/_name"]', '1'), ('[":strange" ":_strange"]', '1'), ('[":strange"]', '2'),
                                  ('[:name]', '1'), ('[:name "name"]', '1'), ('["name" :aka]', '1'), ('[* :name]', '1'), ('["limit"]', '1'), ('["default"]', '1'), ('["name" "default" "limit"]', '1')]]
     + [parse(p) for p in ['["name" "aka"]', '["b" "a" "ns/name" ":strange" ":db/id" "_friend"]', '[":b" "a" "c/d" ":c/a"]', '["*" ":db/id"]']]
     + [q('[:find ?e ?n :where [?e "name" ?n]]', DB), q('[:find ?e ?f :where [?e "friend" ?f]]', DB), q('[:find ?e ?n :where [?e :name ?n]]', DB), q('[:find ?v :where [1 :name ?v]]', DB),
        q('[:find ?v :where [9 :name ?v]]', DB), q('[:find ?e ?a ?v :where [?e ?a ?v]]', DB), q('[:find (pull ?e ["name"]) :where [?e "aka"]]', DB), q('[:find ?e :in $ ?a :where [?e ?a]]', DB, '"name"'),
        q('[:find ?e :in $ ?a :where [?e ?a]]', DB, ':name'), q('[:find ?v :where [(get-else $ 1 "name" "?") ?v]]', DB), q('[:find ?v :where [(get-else $ 1 :name "?") ?v]]', DB),
        q('[:find ?v :where [(get-else $ 9 :name "?") ?v]]', DB), q('[:find ?v :where [(missing? $ 1 "nope") ?v]]', DB), q('[:find ?v :where [(missing? $ 1 "_friend") ?v]]', DB),
        q('[:find ?v :where [(missing? $ 1 :nope) ?v]]', DB), q('[:find ?e :where [["ns/name" "p"] "name" ?e]]', DB)]
     + [datoms('eavt', '1', '"name"'), datoms('aevt', '"name"'), datoms('eavt', '1', ':name'), datoms('eavt', '9', ':name'), datoms('aevt', ':name'), seek('eavt', '1', ':name'), seek('eavt', '9', ':name'),
        rseek('eavt', '1', ':name'), rseek('aevt', ':name'), datoms('avet', '"ns/name"', '"p"'), datoms('avet', ':ns/name', '"p"'),
        '{:op :find-datom :db #r db :index :eavt :components [1 :name]}', '{:op :find-datom :db #r db :index :aevt :components [:name]}', '{:op :find-datom :db #r db :index :eavt :components [9 :name]}',
        '{:op :db-with :db #r db :tx [[:db/add 1 :kw "x"]]}', '{:op :db-with :db #r db :tx [[:db/add 9 :kw "x"]]}', '{:op :db-with :db #r db :tx [{:db/id 9 :kw "x"}]}', '{:op :db-with :db #r db :tx [[:db/retract 1 :name "Ivan"]]}',
        '{:op :db-with :db #r db :tx [[:db.fn/retractAttribute 1 :name]]}', '{:op :db-with :db #r db :tx [[:db.fn/retractAttribute 9 :name]]}']
     + [entity('1', '["name" "aka" "friend" "_friend" ":strange" ":_strange" "ns/name" "nope" :db/id]', True), entity('2', '["_friend" ":_strange" "ns/_name"]'), entity('1', '[:name]'), entity('9', '[:name]')])
case(setup('nil', '[{:db/id 1 :name "Ivan"}]') + ['{:op :db-with :db #r db :tx [[:db/add 1 "name" "x"]]}', '{:op :db-with :db #r db :tx [[:db/add 9 "name" "x"]]}', '{:op :db-with :db #r db :tx [{:db/id 9 "name" "x"}]}',
                                                    '{:op :db-with :db #r db :tx [{"name" "x"}]}', '{:op :db-with :db #r db :tx [[:db/retract 1 "name" "Ivan"]]}', '{:op :db-with :db #r db :tx [[:db/retract 9 "name" "Ivan"]]}',
                                                    '{:op :db-with :db #r db :tx [[:db.fn/retractAttribute 1 "name"]]}', '{:op :db-with :db #r db :tx [[:db.fn/cas 1 "name" nil "x"]]}',
                                                    datoms('eavt', '1', '"name"'), datoms('eavt', '9', '"name"'), datoms('aevt', '"name"'), seek('aevt', '"name"'), rseek('aevt', '"name"'), seek('eavt', '0', '"name"'),
                                                    rseek('eavt', '9', '"name"'), pull('["name"]', '1'), pull('["name"]', '9'), pull('[:name "name"]', '1'), pull('["_name"]', '1'), entity('1', '["name"]'), entity('1', '["_name"]'),
                                                    q('[:find ?v :where [1 "name" ?v]]', DB), q('[:find ?v :where [9 "name" ?v]]', DB), q('[:find ?e :where [?e "name"]]', DB), q('[:find ?e :where [?e "name" "Ivan"]]', DB)])

# ---------------------------------------------------------------- pull_parser.cljc
PSCHEMA = ('{:ref {:db/valueType :db.type/ref} :ref2 {:db/valueType :db.type/ref} :ref3 {:db/valueType :db.type/ref} :ns/ref {:db/valueType :db.type/ref} :multival {:db/cardinality :db.cardinality/many} '
           ':multiref {:db/valueType :db.type/ref :db/cardinality :db.cardinality/many} :component {:db/valueType :db.type/ref :db/isComponent true} '
           ':multicomponent {:db/valueType :db.type/ref :db/isComponent true :db/cardinality :db.cardinality/many}}')
PARSE = ['[:normal]', '[(:normal)]', '[[:normal]]', '[:db/id]', '[*]', '["*"]', '[* :normal]', '[* :db/id]', '[* [:db/id :as :xxx]]', '[:ref]', '[:_ref]', '[:component]', '[:_component]', '[:ns/_ref]', '[:c :b :a]',
         '[:ref2 :ref3 :ref]', '[:_ref2 :_ref3 :_ref]', '[:ref2 (:ref3 :as :ref) (:ref :as :ref3)]', '[(:normal :as :normal2)]', '[(:normal :as "normal2")]', '[(:normal :as 123)]', '[(:normal :as nil)]',
         '[(:ns/_ref :as :ns/ref)]', '[(:db/id :as :id)]', '[:multival]', '[(:multival :limit 100)]', '[(limit :multival 100)]', '[(limit :multival nil)]', '[("limit" :multival 100)]', '[[limit :multival 100]]',
         '[(:multival :default :xyz)]', '[(default :multival :xyz)]', '[("default" :multival :xyz)]', '[[default :multival :xyz]]', '[[:normal :xform inc]]', '[[:normal :xform #f x-str]]',
         '[(:multival :limit 100 :default :xyz :as :other :xform inc)]', '[(:multival :xform inc :as :other :default :xyz :limit 100)]', '[((:multival :limit 100) :default :xyz)]', '[((:multival :default :xyz) :limit 100)]',
         '[(limit (default :multival :xyz) 100)]', '[(default (limit :multival 100) :xyz)]', '[(limit (:multival :default :xyz) 100)]', '[(default (:multival :limit 100) :xyz)]', '[(((limit :multival 100) :default :xyz))]',
         '[(((default :multival :xyz) :limit 100))]', '[:multival [:multival :default :xyz] [:multival :limit 100]]', '[:ref {:ref ...}]', '[{:ref ...} :ref]', '[{:ref [:normal]}]', '[{:_ref [:normal]}]', '[{:ref [*]}]',
         '[{:ref [{:ref2 [{:ref3 [*]}]}]}]', '[{:ref [:normal] :ref2 [:normal2]}]', '[{:ref [:normal]} {:ref2 [:normal2]}]', '[{(:multiref :limit 100) [:normal]}]', '[{(limit :multiref 100) [:normal]}]', '[{:component 1}]',
         '[{:ref 100}]', '[{:ref ...}]', '[{:ref "..."}]', '[{:_ref 100}]', '[{:_ref ...}]',
         # errors
         '[:_normal]', '[(:multival :limit)]', '[(limit :multival)]', '[(:normal :limit 100)]', '[(limit :normal 100)]', '[(:multival :limit :abc)]', '[(limit :multival :abc)]', '[(default :normal)]', '[(default :normal 1 2)]',
         '[[:normal :xform unknown]]', '[{:normal [:normal2]}]', '[{(:ref :limit 100) [:normal]}]', '[{:ref :normal}]',
         # around the tests
         '[]', 'nil', ':normal', '"str"', '5', '{}', '#{:normal}', '(:normal :ref)', '[nil]', '[5]', '[#{}]', '[()]', '[[]]', '[{}]', '[[5]]', '[["limit"]]', '[[limit]]', '[[default]]', '[(limit 5 5)]',
         '[(limit :multival 0)]', '[(limit :multival -5)]', '[(limit :multival 1.5)]', '[(limit :multival "5")]', '[(:multival :limit 0)]', '[(:normal :foo 1)]', '[(:normal :as)]', '[(:normal "as" 1)]', '[(:normal as 1)]',
         '[{:ref 0}]', '[{:ref -1}]', '[{:ref 1.5}]', '[{:ref nil}]', '[{:ref "str"}]', '[{:ref {}}]', '[{5 [:normal]}]', '[{nil [:normal]}]', '[{[:ref] [:normal]}]', '[{[:ref :as :r] [:normal]}]', '[{(:ref :as :r) ...}]',
         '[{:multicomponent ...}]', '[{:_multicomponent 3}]', '[:multicomponent]', '[:_multicomponent]', '[:_multiref]', '[:multiref]', '[* *]', '[:* "*" *]', '[* :_ref]', '[* {:ref [*]}]', '[{:ref [* {:ref2 ...}]}]',
         '[:b :a/z :a :c/a :a/a ":s"]', '[:_ref :ns/_ref :_ref2]', '[[:a :as :x] [:b :as :x]]', '[[:_ref :as :x] [:_ref2 :as :x]]', '[[:ref :as :x] [:_ref2 :as :x]]', '[:a :a]', '[:a [:a :as :a]]', '[(:a :xform :kw)]',
         '[(:a :xform 5)]', '[(:a :xform nil)]', '[(:a :xform "str")]', '[(:a :xform clojure.string/blank?)]', '[(:a :default nil)]', '[(:a :default false)]', '[(default :a nil)]', '[:db/id :db/id]', '[[:db/id :limit 5]]',
         '[{:db/id [:a]}]', '[":db/id" :db/id]', '[("limit" "multival" 5)]', '[(limit (limit :multival 5) 6)]', '[(default (default :a 1) 2)]', '[((:a :as :b) :as :c)]', '[(:ref :default 5)]', '[{(:ref :default 5) [:a]}]',
         '[{(default :ref 5) ...}]', '[{:ref [:a] :_ref [:b] :multiref 2 :component ...}]']
case(['{:op :empty-db :schema %s :as db}' % PSCHEMA] + [parse(p) for p in PARSE])

# ---------------------------------------------------------------- entity.cljc
case(setup('{:aka {:db/cardinality :db.cardinality/many}}', '[{:db/id 1, :name "Ivan", :age 19, :aka ["X" "Y"]} {:db/id 2, :name "Ivan", :sex "male", :aka ["Z"]} [:db/add 3 :huh? false]]')
     + [entity('1'), entity('1', '[:db/id :name :age :aka]'), entity('1', '[:unknown]'), entity('1', '[:name]'), entity('1', '[]', True), entity('2', '[]', True), entity('3', '[]', True), entity('3', '[:huh?]'),
        entity('1', '[:name :unknown :_aka :db/id]', True), entity('4'), entity('nil'), entity('"abc"'), entity(':keyword'), entity('[:name "Petr"]'), entity('777'), entity('[:not-an-attr 777]'), entity('1.5'), entity('-1'),
        entity('0'), entity('{:db/id 1}'), entity('1', '[5]'), entity('1', '[nil]'), entity('1', '["name"]'), entity('1', '[:_name]'), entity('1', '[[:a]]'),
        path('1', '[[:contains :age]]'), path('1', '[[:contains :not-found]]'), path('1', '[[:contains :db/id]]'), path('1', '[[:contains :_aka]]'), path('3', '[[:contains :huh?]]'), path('1', '[[:call :name]]'),
        path('1', '[[:get :name :nf]]'), path('1', '[[:get :nope :nf]]'), path('3', '[[:get :huh? :nf]]'), path('1', '[:oracle/count]'), path('1', '[:oracle/keys]'), path('1', '[:oracle/seq]'), path('1', '[:oracle/first]'),
        path('1', '[:name :oracle/count]'), path('1', '[:aka :oracle/count]'), path('1', '[:aka :oracle/first]'), path('1', '[:oracle/touch :oracle/touch]'), path('9', '[:oracle/touch]'), path('9', '[:name]'),
        path('1', '[:name :oracle/touch]')])
case(setup('{:father {:db/valueType :db.type/ref} :children {:db/valueType :db.type/ref :db/cardinality :db.cardinality/many}}',
           '[{:db/id 1, :children [10]} {:db/id 10, :father 1, :children [100 101]} {:db/id 100, :father 10} {:db/id 101, :father 10}]')
     + [entity('1', '[:children]'), entity('10', '[:children :father]'), entity('100', '[:children]'), entity('1', '[:_children :_father]'), entity('10', '[:_children :_father]'), entity('1', '[]', True), entity('10', '[]', True),
        path('1', '[:children :oracle/first :children]'), path('10', '[:children :oracle/first :father]'), path('10', '[:father :children]'), path('1', '[:oracle/touch :children :oracle/first :children]'),
        path('10', '[:oracle/touch :children :oracle/first :father]'), path('10', '[:oracle/touch :father :children]'), path('100', '[:_children :oracle/first :_children]'), path('1', '[:_father :oracle/first :_father]'),
        path('1', '[:children :oracle/first :oracle/touch]'), path('1', '[:children :oracle/first :children :oracle/first :father :father :db/id]'), path('100', '[:father :father :father]'),
        path('100', '[:father :_father :oracle/count]'), path('1', '[:_children]'), path('1', '[:_nope]'), path('1', '[:children :oracle/first [:contains :father]]')])
case(setup('{:ref {:db/valueType :db.type/ref} :comp {:db/valueType :db.type/ref :db/isComponent true} :multiref {:db/valueType :db.type/ref :db/cardinality :db.cardinality/many} :multicomp {:db/valueType :db.type/ref :db/isComponent true :db/cardinality :db.cardinality/many}}',
           '[[:db/add 1 :ref 2] [:db/add 1 :comp 3] [:db/add 1 :multiref 4] [:db/add 1 :multiref 5] [:db/add 1 :multicomp 6] [:db/add 1 :multicomp 6]]')
     + [entity('1', '[:ref :comp :multiref :multicomp]'), entity('1', '[]', True), entity('1', '[:ref :comp :multiref :multicomp]', True), path('1', '[[:contains :ref]]'), path('1', '[[:contains :multiref]]'),
        path('1', '[[:get :ref :nf]]'), path('1', '[[:get :multiref :nf]]'), path('1', '[:oracle/touch [:get :ref :nf]]'), path('1', '[:oracle/touch [:contains :ref]]'), entity('2'), pull('[*]', '1'), pull('[:ref :comp]', '1')])
case(setup('{:name {:db/unique :db.unique/identity}}', '[{:db/id 1, :name "Ivan"} {:db/id 2, :name "Oleg"}]')
     + [entity('[:name "Ivan"]', '[:name :db/id]'), entity('[:name "Petr"]'), '{:op :db-with :db #r db :tx [] :as db2}', '{:op :db-with :db #r db :tx [{:db/id 3, :name "X"}] :as db3}',
        '{:op :entity-eq :a #r db :e1 1 :b #r db :e2 1}', '{:op :entity-eq :a #r db :e1 1 :b #r db :e2 2}', '{:op :entity-eq :a #r db :e1 1 :b #r db2 :e2 1}', '{:op :entity-eq :a #r db :e1 1 :b #r db3 :e2 1}',
        '{:op :entity-eq :a #r db :e1 1 :b #r db :e2 [:name "Ivan"]}', '{:op :entity-eq :a #r db :e1 9 :b #r db :e2 8}'])
# components (components.cljc)
COMP = '{:profile {:db/valueType :db.type/ref :db/isComponent true} :parts {:db/valueType :db.type/ref :db/isComponent true :db/cardinality :db.cardinality/many}}'
case(setup(COMP, '[{:db/id 1 :name "Ivan" :profile 3} {:db/id 3 :email "@3"} {:db/id 4 :email "@4"}]')
     + [entity('1', '[:profile]'), entity('1', '[]', True), path('1', '[:profile :email]'), path('3', '[:_profile]'), path('3', '[:_profile :name]'), path('4', '[:_profile]'), path('1', '[:oracle/touch :profile]'),
        pull('[*]', '1'), pull('[:name :profile]', '1'), pull('[:email :_profile]', '3'), pull('[:email {:_profile [:name]}]', '3'), '{:op :with :db #r db :tx [[:db.fn/retractEntity 1]]}',
        '{:op :with :db #r db :tx [[:db.fn/retractAttribute 1 :profile]]}', '{:op :with :db #r db :tx [[:db/retract 1 :profile 3]]}'])
case(setup(COMP, '[{:db/id 1 :name "Ivan" :profile {:email "@2" :parts [{:name "p1"} {:name "p2" :parts [{:name "p3"}]}]} :parts [{:db/id 10 :name "top"}]}]')
     + [entity('1', '[]', True), path('1', '[:oracle/touch :profile :parts]'), path('1', '[:parts]'), path('1', '[:parts :oracle/first :_parts]'), path('2', '[:_profile]'), path('5', '[:_parts :_parts]'),
        pull('[*]', '1'), pull('[:name {:profile [:email {:parts ...}]}]', '1'), pull('[* :_parts]', '5'), pull('[:name {:_parts ...}]', '5'), '{:op :with :db #r db :tx [[:db.fn/retractEntity 1]]}',
        '{:op :with :db #r db :tx [[:db.fn/retractEntity 2]]}', '{:op :with :db #r db :tx [[:db.fn/retractAttribute 2 :parts]]}'])

# ---------------------------------------------------------------- filter.cljc
FDB = setup('{:aka {:db/cardinality :db.cardinality/many}}',
            '[{:db/id 1 :name "Petr" :email "petya@spb.ru" :aka ["I" "Great"] :password "<SECRET>"} {:db/id 2 :name "Ivan" :aka ["Terrible" "IV"] :password "<PROTECTED>"} {:db/id 3 :name "Nikolai" :aka ["II"] :password "<UNKWOWN>"}]')
FDB += [flt('f-not-password', name='nopass'), flt('f-not-e2', name='noivan'), flt('f-long-akas', name='long'), flt('f-not-password', 'noivan', 'both'), flt('f-long-akas', 'noivan', 'ivanlong'), flt('f-not-e2', 'long', 'longivan'),
        flt('f-all', name='all'), flt('f-none', name='none'), flt('f-none', 'db0', 'empty-none'), flt('f-all', 'db0', 'empty-all'), '{:op :db-with :db #r db :tx [[:db.fn/retractEntity 2]] :as retracted}']
for d in ['db', 'nopass', 'noivan', 'long', 'both', 'ivanlong', 'longivan', 'all', 'none']:
    FDB += [q('[:find ?v :where [_ :password ?v]]', '#r ' + d), q('[:find ?v :where [_ :aka ?v]]', '#r ' + d), q('[:find ?e ?a ?v :where [?e ?a ?v]]', '#r ' + d), entity('1', '[:password :aka :name]', True, '#r ' + d),
            entity('2', '[:password :aka :name]', False, '#r ' + d), datoms('aevt', ':password', db='#r ' + d), datoms('eavt', db='#r ' + d), datoms('eavt', '2', db='#r ' + d), seek('eavt', '2', db='#r ' + d),
            rseek('eavt', '2', db='#r ' + d), seek('aevt', ':name', db='#r ' + d), rseek('aevt', ':name', db='#r ' + d), '{:op :entid :db #r %s :eid 2}' % d, '{:op :schema :db #r %s}' % d,
            '{:op :index-range :db #r %s :attr :name :start nil :end nil}' % d, '{:op :find-datom :db #r %s :index :eavt :components [2]}' % d,
            '{:op :datoms :db #r %s :index :eavt :components [] :limit 3}' % d, pull('[*]', '2', '#r ' + d), pull_many('[:name :aka]', '[1 2 3]', '#r ' + d)]
FDB += ['{:op :db-eq :a #r retracted :b #r noivan}', '{:op :db-hash-eq :a #r retracted :b #r noivan}', '{:op :db-eq :a #r db0 :b #r none}', '{:op :db-eq :a #r db0 :b #r empty-all}', '{:op :db-eq :a #r empty-none :b #r none}',
        '{:op :db-hash-eq :a #r db0 :b #r empty-all}', '{:op :db-hash-eq :a #r db0 :b #r none}', '{:op :db-eq :a #r db :b #r all}', '{:op :db-hash-eq :a #r db :b #r all}', '{:op :db-eq :a #r all :b #r db}',
        '{:op :db-eq :a #r noivan :b #r retracted}', '{:op :db-eq :a #r nopass :b #r noivan}', '{:op :db-with :db #r all :tx [[:db/add 1 :x 1]]}', '{:op :with :db #r all :tx [[:db/add 1 :x 1]]}',
        '{:op :with-schema :db #r all :schema {}}', '{:op :db-empty :db #r all}', '{:op :conn :db #r all :txs [[[:db/add 1 :x 1]]]}', '{:op :rschema :db #r all}', '{:op :filter :db 5 :pred #f f-all}',
        '{:op :filter :db nil :pred #f f-all}', '{:op :filter :db #r db :pred nil :as nilpred}', q('[:find ?e :where [?e :name]]', '#r nilpred'),
        '{:op :filter :db #r db :pred #f f-throw :as thr}', q('[:find ?e :where [?e :name]]', '#r thr'), datoms('eavt', db='#r thr'), entity('1', '[:name]', False, '#r thr'), '{:op :entid :db #r thr :eid 1}']
case(FDB)
case(setup('nil', '[{:db/id 1, :name "Petr", :age 32} {:db/id 2, :name "Oleg"} {:db/id 3, :name "Ivan", :age 12}]')
     + [flt('f-has-age', name='aged'), flt('f-adult', 'aged', 'adult'), flt('f-adult', name='adult-only'), datoms('aevt', ':name'), datoms('aevt', ':name', db='#r aged'), datoms('aevt', ':name', db='#r adult'),
        datoms('aevt', ':name', db='#r adult-only'), q('[:find ?e ?n :where [?e :name ?n]]', '#r adult'), q('[:find (count ?e) . :where [?e]]', '#r aged')])

# ---------------------------------------------------------------- order: larger entities and results
rnd = random.Random(7)
ATTRS = ['a%02d' % i for i in range(14)]
wide = []
for e in range(1, 9):
    for a in rnd.sample(ATTRS, rnd.randint(9, 14)):
        wide.append('[%d :%s %d]' % (e, a, rnd.randint(0, 99)))
    for t in range(rnd.randint(9, 20)):
        wide.append('[%d :tags :t%d]' % (e, rnd.randint(0, 40)))
    for r in rnd.sample(range(1, 9), rnd.randint(0, 4)):
        wide.append('[%d :refs %d]' % (e, r))
many_refs = ['[20 :refs %d]' % t for t in rnd.sample(range(21, 60), 8)] + ['[%d :a00 %d]' % (t, t) for t in range(21, 60)]
WIDE_SCHEMA = '{:tags {:db/cardinality :db.cardinality/many} :refs {:db/valueType :db.type/ref :db/cardinality :db.cardinality/many}}'
steps = init(wide + many_refs, WIDE_SCHEMA)
for e in range(1, 9):
    steps += [pull('[*]', str(e)), entity(str(e), '[]', True), entity(str(e), '[:tags :refs :a00 :a13 :_refs]')]
steps += [pull('[' + ' '.join(':' + a for a in ATTRS) + ']', '1'), pull('[' + ' '.join(':' + a for a in reversed(ATTRS)) + ' :tags]', '2'), pull('[* {:refs [*]}]', '3'), pull('[:a00 {:refs ...}]', '1'),
          pull('[' + ' '.join('[:%s :as :%s]' % (a, 'z%d' % (99 - i)) for i, a in enumerate(ATTRS)) + ']', '4'), pull('[' + ' '.join('[:%s :default 0]' % a for a in ATTRS) + ' [:nope :default 1]]', '5'),
          pull('[:refs :_refs]', '20'), pull('[{:refs [:a00]}]', '20'), entity('20', '[:refs]'), entity('20', '[]', True), path('20', '[:refs :oracle/count]'), entity('30', '[:_refs]'),
          pull_many('[:a00 :a01 :_refs]', '[' + ' '.join(str(e) for e in range(1, 9)) + ']'), q('[:find ?e (pull ?e [*]) :where [?e :a00]]', DB), q('[:find [(pull ?e [:a00 :a05 :tags]) ...] :where [?e :a05]]', DB)]
case(steps)

with open(sys.argv[1] if len(sys.argv) > 1 else 'conformance/cases/pull.edn', 'w') as f:
    f.write('; The pull API, entities and filtered databases, after DataScript\'s own tests and around them. Written by conformance/gen/pull.py.\n')
    f.write('\n'.join(lines) + '\n')
print('%d cases' % len(lines))
