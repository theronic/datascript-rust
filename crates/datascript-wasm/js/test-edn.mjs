// The module through its EDN interface, from JavaScript alone: node crates/datascript-wasm/js/test-edn.mjs [datascript.wasm]
import assert from 'node:assert/strict';
import fs from 'node:fs';
import { instantiate } from './datascript-edn.mjs';

const wasm = process.argv[2] ?? new URL('../../../target/wasm32-unknown-unknown/wasm-release/datascript_wasm.wasm', import.meta.url);
const ds = await instantiate(fs.readFileSync(wasm));

const schema = '{:aka {:db/cardinality :db.cardinality/many} :friend {:db/valueType :db.type/ref} :email {:db/unique :db.unique/identity}}';
const db0 = ds.call(`[:empty-db ${schema}]`);
assert.match(db0, /^#datascript\/handle \d+$/);

const db = ds.call(`[:db-with ${db0} [{:db/id -1 :name "Ivan" :age 15 :aka ["Devil" "Tupen"] :email "ivan@example.com"}
                                      {:db/id -2 :name "Petr" :age 37 :friend -1}
                                      {:db/id -3 :name "Oleg" :age 37 :friend -2}]]`);
assert.equal(ds.call(`[:count ${db}]`), '11');
assert.equal(ds.call(`[:count ${db0}]`), '0'); // a database is a value: the one it was made from is as it was

// queries answer as ClojureScript DataScript prints them, in its order: what is expected here is what it answers
assert.equal(ds.call(`[:q [:find ?n :where [?e :aka "Tupen"] [?e :name ?n]] ${db}]`), '#{["Ivan"]}');
assert.equal(ds.call(`[:q [:find ?n ?a :where [?e :name ?n] [?e :age ?a]] ${db}]`), '#{["Ivan" 15] ["Petr" 37] ["Oleg" 37]}');
assert.equal(ds.call(`[:q [:find (max ?a) . :where [_ :age ?a]] ${db}]`), '37');
assert.equal(ds.call(`[:q [:find [?n ...] :in $ ?min :where [?e :age ?a] [(>= ?a ?min)] [?e :name ?n]] ${db} 18]`), '["Petr" "Oleg"]');
assert.equal(
  ds.call(`[:q [:find ?n :in $ % :where [?e :name "Oleg"] (knows ?e ?f) [?f :name ?n]] ${db}
            [[(knows ?a ?b) [?a :friend ?b]] [(knows ?a ?b) [?a :friend ?x] (knows ?x ?b)]]]`),
  '#{["Ivan"] ["Petr"]}');

assert.equal(ds.call(`[:pull ${db} [:name {:friend [:name]}] 3]`), '{:friend {:name "Petr"}, :name "Oleg"}');
assert.equal(ds.call(`[:pull-many ${db} [:name] [1 2]]`), '[{:name "Ivan"} {:name "Petr"}]');
assert.equal(ds.call(`[:entid ${db} [:email "ivan@example.com"]]`), '1');
assert.equal(ds.call(`[:entity ${db} 2]`), '{:age 37, :friend {:db/id 1}, :name "Petr", :db/id 2}');
assert.equal(ds.call(`[:entity ${db} 99]`), 'nil');
assert.equal(ds.call(`[:datoms ${db} :eavt 1 :aka]`), '[#datascript/Datom [1 :aka "Devil" 536870913 true] #datascript/Datom [1 :aka "Tupen" 536870913 true]]');
assert.equal(ds.call(`[:seek-datoms ${db} :eavt 3 :friend]`), '[#datascript/Datom [3 :friend 2 536870913 true] #datascript/Datom [3 :name "Oleg" 536870913 true]]');
assert.equal(ds.call(`[:index-range ${db} :email "a" "z"]`), '[#datascript/Datom [1 :email "ivan@example.com" 536870913 true]]');

// a transaction's report
const report = ds.call(`[:with ${db} [[:db/add "new" :name "Vera"] [:db/retract 1 :aka "Devil"]] {:by "test"}]`);
assert.match(report, /^\{:db-after #datascript\/handle \d+, :tx-data \[#datascript\/Datom \[4 :name "Vera" 536870914 true\] #datascript\/Datom \[1 :aka "Devil" 536870914 false\]\], :tempids \{"new" 4, :db\/current-tx 536870914\}, :tx-meta \{:by "test"\}\}$/);

// an error is DataScript's, with its data
assert.throws(() => ds.call(`[:db-with ${db} [[:db/add nil :name "x"]]]`), (e) => /entity id/i.test(e.message) && e.edn.includes(':data'));
assert.throws(() => ds.call('[:no-such-operation]'), /No operation/);
assert.throws(() => ds.call('[:count #datascript/handle 999]'), /no database of handle/);

// a database out and in again: as EDN, and as the JSON DataScript's own serializable is
const text = JSON.parse(ds.call(`[:db-string ${db}]`));
assert.match(text, /^#datascript\/DB \{:schema /);
const again = ds.call(`[:read-db ${JSON.stringify(text)}]`);
assert.equal(ds.call(`[:q [:find (count ?e) . :where [?e :name]] ${again}]`), '3');
const json = JSON.parse(ds.call(`[:serializable ${db}]`));
assert.equal(JSON.parse(json).count, 11);
const thawed = ds.call(`[:from-serializable ${JSON.stringify(json)}]`);
assert.equal(ds.call(`[:datoms ${thawed} :eavt 1 :aka]`), ds.call(`[:datoms ${db} :eavt 1 :aka]`));

// a handle is given back
assert.equal(ds.call(`[:release ${again}]`), 'true');
assert.throws(() => ds.call(`[:count ${again}]`), /no database of handle/);

console.log('datascript.wasm, through EDN: ok');
