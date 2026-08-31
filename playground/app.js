// Talks to darkroom.wasm across a raw C ABI boundary - no wasm-bindgen, to
// match the empty [dependencies] the Rust side is built under. The wire
// format is documented in src/wasm.rs; this file is the other half of it.

const QUIET_ZONE = 4; // matches src/qr/render.rs's QUIET_ZONE
const MODULE_PX = 10; // canvas pixels per QR module, before CSS scales down

const input = document.getElementById("text");
const canvas = document.getElementById("qr");
const message = document.getElementById("message");
const status = document.getElementById("status");
const ctx = canvas.getContext("2d");

let wasm = null;

async function boot() {
  try {
    const resp = await fetch("darkroom.wasm");
    if (!resp.ok) throw new Error(`fetch failed: ${resp.status}`);
    const { instance } = await WebAssembly.instantiateStreaming(resp, {});
    wasm = instance.exports;
    status.textContent = "";
    render(input.value);
  } catch (err) {
    status.textContent = `couldn't load darkroom.wasm: ${err.message}`;
  }
}

function encode(text) {
  const utf8 = new TextEncoder().encode(text);
  const inLen = utf8.length || 1;
  const inPtr = wasm.alloc(inLen);
  new Uint8Array(wasm.memory.buffer, inPtr, utf8.length).set(utf8);

  const outPtr = wasm.encode_qr(inPtr, utf8.length);
  wasm.dealloc(inPtr, inLen);

  const outLen = wasm.result_len(outPtr);
  // Copy out before freeing - memory.buffer can be detached/resized by a
  // later allocation, which would invalidate a view over the old buffer.
  const out = new Uint8Array(wasm.memory.buffer, outPtr, outLen).slice();
  wasm.dealloc(outPtr, outLen);

  if (out[0] !== 0) return { error: true };
  const side = out[1];
  return { side, modules: out.subarray(2) };
}

function render(text) {
  if (!wasm) return;
  const result = encode(text);

  if (result.error) {
    canvas.hidden = true;
    message.textContent =
      "too long for darkroom's QR encoder (78-byte capacity, version 4-L)";
    return;
  }

  message.textContent = "";
  canvas.hidden = false;

  const { side, modules } = result;
  const n = side + QUIET_ZONE * 2;
  canvas.width = n * MODULE_PX;
  canvas.height = n * MODULE_PX;

  ctx.fillStyle = "#fff";
  ctx.fillRect(0, 0, canvas.width, canvas.height);
  ctx.fillStyle = "#000";
  for (let row = 0; row < side; row++) {
    for (let col = 0; col < side; col++) {
      if (modules[row * side + col]) {
        ctx.fillRect(
          (col + QUIET_ZONE) * MODULE_PX,
          (row + QUIET_ZONE) * MODULE_PX,
          MODULE_PX,
          MODULE_PX,
        );
      }
    }
  }
}

let debounce = null;
input.addEventListener("input", () => {
  clearTimeout(debounce);
  debounce = setTimeout(() => render(input.value), 80);
});

boot();
