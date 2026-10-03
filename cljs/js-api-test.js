// DataScript's own JavaScript tests (test/js/tests.js), on the JavaScript API over the WebAssembly module. The tests
// ask for DataScript's release, and are given this one.
const fs = require('fs'), path = require('path'), Module = require('module');

const bundle = path.join(__dirname, 'target/js/datascript.js');
const resolve = Module._resolveFilename;
Module._resolveFilename = function (request, ...rest) {
  return request.endsWith('release-js/datascript.js') ? bundle : resolve.call(this, request, ...rest);
};

require(bundle).instantiate_sync(fs.readFileSync(process.env.DATASCRIPT_WASM));
const res = require('../test/js/tests.js').test_all();
if (res.fail + res.error > 0) process.exit(1);
