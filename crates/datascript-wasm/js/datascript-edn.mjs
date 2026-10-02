// DataScript's WebAssembly module for a JavaScript program that writes EDN: the smallest host of it, and the one to
// read before writing another, in JavaScript or not. One operation goes in as EDN text and its answer comes back as
// EDN text (crates/datascript-wasm/src/edn_api.rs lists the operations):
//
//   import { instantiate } from './datascript-edn.mjs';
//   const ds = await instantiate(bytesOrResponse);
//   const db = ds.call('[:empty-db {:aka {:db/cardinality :db.cardinality/many}}]');   // "#datascript/handle 0"
//   const db1 = ds.call(`[:db-with ${db} [{:db/id -1 :name "Ivan" :aka ["Devil" "Tupen"]}]]`);
//   ds.call(`[:q [:find ?n :where [?e :aka "Tupen"] [?e :name ?n]] ${db1}]`);          // "#{[\"Ivan\"]}"
//   ds.call(`[:release ${db}]`);
//
// A database is a handle, `#datascript/handle n`, which stays in the module until it is released. An operation that
// fails throws an Error whose message is DataScript's and whose `edn` is `{:message "…" :data {…}}`.
//
// The module imports five functions from a module named `datascript`. A host that passes it no functions of its own,
// as this one does not, never has `ds_host_call` called, and answers the rest as below.

const encoder = new TextEncoder(), decoder = new TextDecoder();

/** What the module imports, for a host with no functions or values of its own to lend it. `memory` answers the
 *  instance's memory, once there is one. */
export function imports(memory) {
  return {
    datascript: {
      // a function of the host's, called by the module: there are none
      ds_host_call: () => 1,
      // a question about a value of the host's: there are none
      ds_host_op: () => 0,
      // a function or a value of the host's let go of: nothing to do
      ds_host_release: () => {},
      // for rand, rand-int and the sampling aggregates
      ds_host_random: () => Math.random(),
      // what the module has to say: level 0 is a failure of its own
      ds_host_log: (level, ptr, len) => {
        const text = decoder.decode(new Uint8Array(memory().buffer, ptr, len));
        (level === 0 ? console.error : console.log)(text);
      },
    },
  };
}

/** The module, from its bytes, a Response for them or a compiled WebAssembly.Module: `{ call, exports }`. */
export async function instantiate(source) {
  let exports;
  const imported = imports(() => exports.memory);
  const made = source instanceof WebAssembly.Module
    ? { instance: await WebAssembly.instantiate(source, imported) }
    : typeof Response !== 'undefined' && source instanceof Response
      ? await WebAssembly.instantiateStreaming(source, imported)
      : await WebAssembly.instantiate(source, imported);
  exports = made.instance.exports;
  if (exports.ds_abi_version() !== 1) throw new Error(`datascript.wasm speaks interface ${exports.ds_abi_version()}, this host interface 1`);

  // whether a call did not return and the module has not been told yet
  let unrecovered = false;
  const recover = () => {
    exports.ds_recover();
    unrecovered = false;
  };

  /** One operation, as EDN: its answer, as EDN. */
  function call(edn) {
    if (unrecovered) recover();
    const bytes = encoder.encode(edn);
    // The module runs on this stack. Should the stack run out under it, the engine ends the call where it stands:
    // the module's own stack is put back where it was, and the module told, before the error goes on its way. Where
    // the stack is still too short for the telling, the module is told before the next call.
    const top = exports.__stack_pointer.value;
    let status;
    try {
      // memory the module takes over, and frees
      const ptr = exports.ds_alloc(bytes.length);
      new Uint8Array(exports.memory.buffer, ptr, bytes.length).set(bytes);
      status = exports.ds_edn(ptr, bytes.length);
    } catch (e) {
      exports.__stack_pointer.value = top;
      unrecovered = true;
      recover();
      throw e;
    }
    // the answer stays where it is until the next call
    const answer = decoder.decode(new Uint8Array(exports.memory.buffer, exports.ds_result_ptr(), exports.ds_result_len()));
    if (status !== 0) {
      const message = answer.match(/:message "((?:[^"\\]|\\.)*)"/);
      throw Object.assign(new Error(message ? JSON.parse(`"${message[1]}"`) : answer), { edn: answer });
    }
    return answer;
  }

  return { call, exports };
}
