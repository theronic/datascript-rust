// The module's allocator calls nothing while anything of its own is half changed (src/heap.rs): the two functions
// that take memory and give it back have no call in them, and everything else that allocates goes through those two.
// That is a property of the built module, so it is the built module that is read here: one built with its names,
//
//   CARGO_PROFILE_WASM_RELEASE_STRIP=none cargo build -p datascript-wasm --target wasm32-unknown-unknown \
//     --profile wasm-release --target-dir target/named
//   node crates/datascript-wasm/js/leaf-check.mjs target/named/wasm32-unknown-unknown/wasm-release/datascript_wasm.wasm
//
// which is the module as it ships but for the names.
import fs from 'node:fs';

const file = process.argv[2];
if (!file) throw new Error('leaf-check.mjs <datascript_wasm.wasm, built with its names>');
const bytes = fs.readFileSync(file);

let pos = 8;
const leb = () => {
  let result = 0, shift = 0, byte;
  do {
    byte = bytes[pos++];
    result += (byte & 0x7f) * 2 ** shift;
    shift += 7;
  } while (byte & 0x80);
  return result;
};
const text = () => {
  const length = leb();
  pos += length;
  return bytes.toString('utf8', pos - length, pos);
};

// the sections: where each starts and ends
const sections = new Map(), custom = new Map();
while (pos < bytes.length) {
  const id = bytes[pos++], size = leb(), end = pos + size;
  if (id === 0) custom.set(text(), [pos, end]);
  else sections.set(id, [pos, end]);
  pos = end;
}

// the functions the module imports come first in the numbering
let imported = 0;
if (sections.has(2)) {
  [pos] = sections.get(2);
  for (let n = leb(); n > 0; n--) {
    text();
    text();
    const kind = bytes[pos++];
    if (kind === 0) { leb(); imported++; }
    else if (kind === 1) { pos++; const flags = bytes[pos++]; leb(); if (flags & 1) leb(); }
    else if (kind === 2) { const flags = bytes[pos++]; leb(); if (flags & 1) leb(); }
    else if (kind === 3) pos += 2;
    else throw new Error(`an import of kind ${kind}`);
  }
}

const names = new Map();
if (!custom.has('name')) throw new Error(`${file} has no names: build it with CARGO_PROFILE_WASM_RELEASE_STRIP=none`);
{
  const [start, end] = custom.get('name');
  pos = start;
  while (pos < end) {
    const id = bytes[pos++], size = leb(), next = pos + size;
    if (id === 1) for (let n = leb(); n > 0; n--) { const index = leb(); names.set(index, text()); }
    pos = next;
  }
}

/** What a function's body calls: the numbers of the functions, and 'indirect' for a call through a table. */
function calls(start, end) {
  const out = [];
  pos = start;
  for (let n = leb(); n > 0; n--) { leb(); pos++; }
  while (pos < end) {
    const op = bytes[pos++];
    if (op === 0x02 || op === 0x03 || op === 0x04) { if (bytes[pos] & 0x80) leb(); else pos++; }  // a block's type
    else if (op === 0x0c || op === 0x0d) leb();
    else if (op === 0x0e) { for (let n = leb() + 1; n > 0; n--) leb(); }
    else if (op === 0x10 || op === 0x12) out.push(leb());
    else if (op === 0x11 || op === 0x13) { leb(); leb(); out.push('indirect'); }
    else if (op === 0x1c) pos += leb();
    else if (op >= 0x20 && op <= 0x26) leb();
    else if (op >= 0x28 && op <= 0x3e) { leb(); leb(); }
    else if (op === 0x3f || op === 0x40) leb();
    else if (op === 0x41 || op === 0x42) leb();
    else if (op === 0x43) pos += 4;
    else if (op === 0x44) pos += 8;
    else if (op === 0xd0) pos++;
    else if (op === 0xd2) leb();
    else if (op === 0xfc) {
      const sub = leb();
      if (sub <= 7) { /* a conversion */ }
      else if (sub === 8) { leb(); pos++; }
      else if (sub === 9 || sub === 13 || (sub >= 15 && sub <= 17)) leb();
      else if (sub === 10) pos += 2;
      else if (sub === 11) pos++;
      else if (sub === 12 || sub === 14) { leb(); leb(); }
      else throw new Error(`an instruction 0xfc ${sub} this does not know`);
    }
    else if (op === 0x00 || op === 0x01 || op === 0x05 || op === 0x0b || op === 0x0f || op === 0x1a || op === 0x1b || op === 0xd1 || (op >= 0x45 && op <= 0xc4)) { /* nothing follows it */ }
    else throw new Error(`an instruction 0x${op.toString(16)} this does not know`);
  }
  return out;
}

const called = new Map();
{
  const [start] = sections.get(10);
  pos = start;
  const count = leb();
  let at = pos;
  for (let i = 0; i < count; i++) {
    pos = at;
    const size = leb(), body = pos;
    at = body + size;
    called.set(imported + i, calls(body, at));
  }
}

const named = (pattern) => [...names].filter(([, name]) => pattern.test(name)).map(([index]) => index);
const one = (what, pattern) => {
  const found = named(pattern);
  if (found.length !== 1) throw new Error(`${found.length} functions are ${what}: ${found.map((i) => names.get(i)).join(', ')}`);
  return found[0];
};
const take = one('the heap taking memory', /4heap.*Heap.*5alloc/), give = one('the heap giving memory back', /4heap.*Heap.*4free/);
const problems = [];
for (const leaf of [take, give]) {
  if (called.get(leaf).length) problems.push(`${names.get(leaf)} calls ${called.get(leaf).map((i) => names.get(i) ?? i).join(', ')}`);
}
// what the rest of the module allocates through: these call the two, whole, and nothing else
const through = named(/__rust_alloc$|__rust_dealloc$|__rust_realloc$|__rust_alloc_zeroed$|__rg_/);
if (through.length < 4) problems.push(`only ${through.length} of the allocator's four entries were found`);
for (const entry of through) {
  const others = called.get(entry).filter((i) => i !== take && i !== give);
  if (others.length) problems.push(`${names.get(entry)} calls ${others.map((i) => names.get(i) ?? i).join(', ')}`);
}
if (named(/dlmalloc/).length) problems.push("the standard library's allocator is in the module too");

if (problems.length) {
  console.error(`the allocator is not what src/heap.rs says it is:\n  ${problems.join('\n  ')}`);
  process.exit(1);
}
console.log(`the allocator: taking memory and giving it back call nothing, and its ${through.length} entries call only them`);
