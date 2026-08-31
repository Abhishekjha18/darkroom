//! The browser-facing ABI for the QR playground — `wasm32-unknown-unknown`,
//! no `wasm-bindgen`, no dependency at all. `[dependencies]` stays empty for
//! this target the same as it does for the binary; the cost is that the
//! boundary is raw bytes in linear memory instead of generated bindings.
//!
//! Four exports, and the JS side (`playground/app.js`) is the only thing
//! that needs to agree with their shapes:
//!
//! - `alloc(len) -> ptr` — JS writes `len` bytes of UTF-8 text at `ptr`.
//! - `dealloc(ptr, len)` — frees a buffer this module handed out or was
//!   handed, once JS is done with it.
//! - `encode_qr(ptr, len) -> ptr` — reads the text, returns a pointer to a
//!   result buffer: byte 0 is status (`0` = ok, `1` = too long for the
//!   78-byte version-4-L capacity `qr::encode` supports — see
//!   `docs/LLD-QR.md`), byte 1 is the module grid's side length, and the
//!   rest is `side * side` bytes of `0`/`1`, row-major.
//! - `result_len(ptr) -> usize` — JS cannot compute the result buffer's
//!   length without reading it first, so this reads just enough of it
//!   (`ptr`'s own header) to report the full length to free correctly.

use std::alloc::{Layout, alloc as sys_alloc, dealloc as sys_dealloc};

#[unsafe(no_mangle)]
pub extern "C" fn alloc(len: usize) -> *mut u8 {
    if len == 0 {
        return std::ptr::null_mut();
    }
    // SAFETY: `len` is nonzero and the layout's align (1) always divides it.
    unsafe { sys_alloc(Layout::from_size_align_unchecked(len, 1)) }
}

/// # Safety
/// `ptr`/`len` must be exactly the `(ptr, len)` this module's own `alloc`
/// handed out, or that `encode_qr` returned paired with `result_len(ptr)`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dealloc(ptr: *mut u8, len: usize) {
    if ptr.is_null() || len == 0 {
        return;
    }
    // SAFETY: guaranteed by this function's own safety contract, above.
    unsafe { sys_dealloc(ptr, Layout::from_size_align_unchecked(len, 1)) };
}

/// # Safety
/// `ptr` must be null, or a pointer `encode_qr` returned that has not since
/// been freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn result_len(ptr: *const u8) -> usize {
    if ptr.is_null() {
        return 0;
    }
    // SAFETY: `ptr` came from `encode_qr`, which always writes at least one
    // status byte, and a second (side) byte whenever status is 0.
    let status = unsafe { *ptr };
    if status != 0 {
        return 1;
    }
    let side = unsafe { *ptr.add(1) } as usize;
    2 + side * side
}

/// # Safety
/// `ptr` must point to at least `len` initialized bytes this module's own
/// `alloc` handed out, not freed since.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn encode_qr(ptr: *const u8, len: usize) -> *mut u8 {
    // SAFETY: guaranteed by this function's own safety contract, above.
    let bytes = unsafe { std::slice::from_raw_parts(ptr, len) };
    let text = match std::str::from_utf8(bytes) {
        Ok(t) => t,
        // Not reachable from the playground's `<input>`, which is always
        // valid UTF-8 by construction, but a raw C boundary has no type
        // system backing that up, so it is still handled as "too long"
        // rather than left as undefined behaviour.
        Err(_) => return too_long(),
    };

    match crate::qr::encode(text) {
        Ok(qr) => {
            let out_len = 2 + qr.size * qr.size;
            // SAFETY: `out_len` is nonzero (`qr.size` is always >= 21).
            let out = unsafe { sys_alloc(Layout::from_size_align_unchecked(out_len, 1)) };
            // SAFETY: `out` was just allocated with exactly `out_len` bytes,
            // and every index written below is `< out_len`.
            unsafe {
                *out = 0;
                *out.add(1) = qr.size as u8;
                for row in 0..qr.size {
                    for col in 0..qr.size {
                        *out.add(2 + row * qr.size + col) = qr.dark(row, col) as u8;
                    }
                }
            }
            out
        }
        Err(_) => too_long(),
    }
}

fn too_long() -> *mut u8 {
    // SAFETY: a fixed 1-byte, align-1 allocation.
    let out = unsafe { sys_alloc(Layout::from_size_align_unchecked(1, 1)) };
    unsafe { *out = 1 };
    out
}
