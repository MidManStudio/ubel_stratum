const fs = require('fs');
const [,, wasmPath] = process.argv;
(async () => {
  let out = '', failed = null, mem;
  const dec = new TextDecoder();
  const env = {
    js_print_i64: v => { out += v.toString(); },
    js_print_bool: b => { out += b ? 'true' : 'false'; },
    js_print_str: (p, n) => { out += dec.decode(new Uint8Array(mem.buffer, p, Number(n))); },
    js_trap: code => { failed = code; throw new Error('trap ' + code); },
  };
  const { instance } = await WebAssembly.instantiate(fs.readFileSync(wasmPath), { env });
  mem = instance.exports.memory;
  try { instance.exports.ubel_main(); } catch (e) { if (failed === null) throw e; }
  const live = instance.exports.ubel_arena_live_count();
  process.stdout.write(out);
  process.stderr.write(`[wasm] trap=${failed} arenas_live=${live}\n`);
  process.exit(failed !== null ? 1 : (live !== 0n && live !== 0 ? 3 : 0));
})();
