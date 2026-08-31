//! The library face of darkroom, for targets that cannot be a photo-library
//! binary: WebAssembly, most concretely. `main.rs` owns the CLI/indexer/
//! server binary and does not depend on this file at all — the two crate
//! targets happen to share source files, not logic.
//!
//! Only modules with no filesystem, thread, or socket dependency are
//! exported here, because those are the only ones a WASM build can satisfy.
//! `qr` (plus the `image::Image` type its renderer returns) is the first:
//! encoding is pure computation over bytes in, a module grid out.
//!
//! `deflate` and `png` are here too, not because `wasm.rs` calls them, but
//! because `qr::render`'s own tests round-trip a QR code through
//! `png::encode`/`decode` — that test module is part of `qr/render.rs`
//! regardless of which crate compiles it, so `cargo test` needs `crate::png`
//! to resolve here the same as it does in the binary. Release-mode dead-code
//! elimination keeps them out of the shipped `.wasm`; nothing `wasm.rs`
//! exports reaches them.

pub mod deflate;
pub mod image;
pub mod png;
pub mod qr;

#[cfg(target_arch = "wasm32")]
pub mod wasm;
