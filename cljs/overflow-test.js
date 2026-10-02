// The host's stack runs out while it is inside the module, again and again: a recursion of the host's that asks the
// module something at every level, each of a dozen things in turn, from a different depth each time, so that the
// stack gives out at as many places in the module as can be had. After each, the module has to answer as it did.
//
//   node cljs/overflow-test.js [rounds]      over cljs/target/js/datascript.js (cljs/js-api.sh), DATASCRIPT_WASM
const fs = require('fs'), path = require('path');
const d = require(path.join(__dirname, 'target/js/datascript.js'));
const instance = d.instantiate_sync(fs.readFileSync(process.env.DATASCRIPT_WASM));

const schema = { friend: { ':db/valueType': ':db.type/ref' }, aka: { ':db/cardinality': ':db.cardinality/many' }, name: { ':db/index': true } };
const fresh = () => {
  const conn = d.create_conn(schema);
  d.transact(conn, Array.from({ length: 40 }, (_, i) => ({
    ':db/id': i + 1, name: 'n' + i, age: i % 9, aka: ['a' + i, 'b' + i], friend: 1 + (i * 7) % 40, nested: [[i, [i]], { k: [i] }],
  })));
  return conn;
};
const answer = (c) => JSON.stringify([
  d.q('[:find ?n ?a :where [?e "name" ?n] [?e "age" ?a] [(< ?a 3)]]', d.db(c)).sort(),
  d.datoms(d.db(c), ':aevt', 'name').length,
  d.pull(d.db(c), '["name" "aka" {"friend" ["name"]}]', 3),
  d.q('[:find (count ?e) . :where [?e "nested"]]', d.db(c)),
]);

let conn = fresh();
const expected = answer(conn);
const rules = '[[(knows ?a ?b) [?a "friend" ?b]] [(knows ?a ?b) [?a "friend" ?x] (knows ?x ?b)]]';
const asked = [
  (db, n) => d.datoms(db, ':eavt', 1 + n % 40),
  (db, n) => d.q('[:find ?n . :in $ ?e :where [?e "name" ?n]]', db, 1 + n % 40),
  (db, n) => d.pull(db, '["name" {"friend" ["name" {"friend" ["aka"]}]}]', 1 + n % 40),
  (db, n) => d.entity(db, 1 + n % 40).get('aka'),
  (db, n) => d.transact(conn, [[':db/add', 1 + n % 40, 'seen', n % 5]]),
  (db, n) => d.db_with(db, [[':db/add', 1 + n % 40, 'aka', 'x' + (n % 3)], { ':db/id': -1, name: 'new', nested: [[n]] }]),
  (db, n) => d.q('[:find ?n :in $ ?f :where [?e "age" ?a] [(?f ?a)] [?e "name" ?n]]', db, (a) => a === n % 9),
  (db, n) => d.q('[:find ?x ?y :in [[?x ?y] ...]]', [[[1, [2, [3]]], { a: [n] }], [[n], [[n, [n]]]]]),
  (db) => d.q('[:find ?f :in $ % :where (knows 1 ?f)]', db, rules),
  (db) => d.q('[:find (distinct ?a) . :where [_ "aka" ?a]]', db),
  (db, n) => d.filter(db, (_, datom) => datom.e % 2 === n % 2),
  (db, n) => d.datoms(d.filter(db, (_, datom) => datom.a !== 'age'), ':eavt', 1 + n % 40),
  (db) => d.serializable(db),
];

function dive(db, ask, n) { ask(db, n); return dive(db, ask, n + 1) + 1; }
function from(depth, f, a, b) { return depth === 0 ? f() : from(depth - 1, f, a, b) + 0; }

// Between rounds the event loop gets a turn, and the garbage collector too where it can be asked (node --expose-gc):
// the database values a round made are the module's to drop only once the collector has found them unreachable, and a
// round makes thousands.
const turn = () => new Promise((resolve) => { if (global.gc) global.gc(); setImmediate(resolve); });

(async () => {
  const rounds = +process.argv[2] || 300;
  let ranOut = 0;
  for (let round = 0; round < rounds; round++) {
    try {
      from((round * 13) % 211 + (round % 7) * 3, () => dive(d.db(conn), asked[round % asked.length], 0), 1, 2);
    } catch (e) {
      if (e instanceof RangeError) ranOut++;
    }
    let got;
    try {
      conn = fresh(); // what the recursion transacted is left behind
      got = answer(conn);
    } catch (e) {
      got = `an error: ${(e && e.message) || e}`;
    }
    if (got !== expected) {
      console.error(`after the stack ran out ${ranOut} times, in round ${round}, the module answers ${got.slice(0, 200)}`);
      process.exit(1);
    }
    await turn();
    await turn();
  }
  const mb = Math.round(instance.exports.memory.buffer.byteLength / 1048576);
  console.log(`the stack ran out under the module ${ranOut} times in ${rounds} rounds, and it answers as it did (its memory: ${mb} MB)`);
})();
