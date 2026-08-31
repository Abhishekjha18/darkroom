// Drives the built darkroom.wasm the same way the browser will, before it
// ships. A playground that loads but answers wrongly - or doesn't load at
// all - is worse than one that fails to deploy, so this runs in CI on every
// build, not just once by hand.
import fs from "node:fs";

const wasmPath = process.argv[2];
if (!wasmPath) {
  console.error("usage: node verify-playground.mjs <path-to-darkroom.wasm>");
  process.exit(1);
}

const bytes = fs.readFileSync(wasmPath);
const { instance } = await WebAssembly.instantiate(bytes, {});
const { memory, alloc, dealloc, encode_qr, result_len } = instance.exports;
for (const [name, fn] of Object.entries({ memory, alloc, dealloc, encode_qr, result_len })) {
  if (!fn) throw new Error(`darkroom.wasm does not export "${name}"`);
}

function encode(text) {
  const utf8 = new TextEncoder().encode(text);
  const inLen = utf8.length || 1;
  const inPtr = alloc(inLen);
  new Uint8Array(memory.buffer, inPtr, utf8.length).set(utf8);

  const outPtr = encode_qr(inPtr, utf8.length);
  dealloc(inPtr, inLen);

  const len = result_len(outPtr);
  const out = new Uint8Array(memory.buffer, outPtr, len).slice();
  dealloc(outPtr, len);

  if (out[0] !== 0) return { error: true };
  const side = out[1];
  return { side, modules: out.subarray(2) };
}

let checked = 0;
function check(name, cond) {
  checked++;
  if (!cond) throw new Error(`FAILED: ${name}`);
  console.log(`ok - ${name}`);
}

// Same LAN URL darkroom's own CLI prints a QR code for.
const a = encode("http://192.168.0.105:8080");
check("a real LAN URL picks version 3 (side 29)", a.side === 29);

const b = encode("http://192.168.0.105:8080");
check(
  "encoding is deterministic across two WASM calls",
  JSON.stringify(a.modules) === JSON.stringify(b.modules),
);

// Finder pattern present in the top-left corner, same shape src/qr/mod.rs's
// own test asserts: outer ring dark, inner ring light, 3x3 core dark.
const dark = (r, c) => a.modules[r * a.side + c] === 1;
check("finder pattern: outer ring dark", dark(0, 0));
check("finder pattern: inner ring light", !dark(1, 1));
check("finder pattern: 3x3 core dark", dark(3, 3));

const c = encode("https://github.com/Abhishekjha18/darkroom - the QR playground");
check("a longer string promotes to version 4 (side 33)", c.side === 33);

const d = encode("x".repeat(200));
check("over the 78-byte capacity reports an error, not a crash", d.error === true);

const e = encode("");
check("empty text still encodes", !e.error);

console.log(`\n${checked} checks passed`);
