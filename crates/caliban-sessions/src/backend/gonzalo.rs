//! gonzalo-facade session store. Feature-gated; the vanilla build never sees it.
//!
//! Placeholder — the real `GonzaloSessionBackend` lands in Task 4. This file
//! exists now only so `rustfmt` can resolve the
//! `#[cfg(feature = "gonzalo")] pub mod gonzalo;` declaration in `mod.rs`
//! (rustfmt walks module declarations regardless of `cfg`). Under the default
//! build the whole file compiles to nothing.
#![cfg(feature = "gonzalo")]
