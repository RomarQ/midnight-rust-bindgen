//! Generate typed Rust bindings from Midnight Compact smart contracts.
//!
//! This crate provides a single dependency for generating type-safe Rust
//! bindings from a Compact compiler's `contract-info.json` output.
//!
//! # Usage
//!
//! ```ignore
//! // Generate bindings with a named module (recommended).
//! midnight_bindgen::contract!(Gateway, "compiled/gateway/compiler/contract-info.json");
//!
//! use gateway::*;
//!
//! let ledger = Gateway::new(state);
//! let threshold: u8 = ledger.threshold()?;
//! ```
//!
//! # What gets generated
//!
//! - **Field constants** -- `FIELD_THRESHOLD`, `FIELD_VALIDATORS`, etc.
//! - **Data types** -- Structs and enums mirroring Compact type definitions
//! - **Ledger struct** -- Typed accessors for each ledger field
//! - **Circuit call types** -- `*Call` structs and `*Return` type aliases

/// Re-export the proc macro.
pub use midnight_bindgen_macro::contract;

/// Re-export the runtime so generated code can use `midnight_bindgen::*`.
pub use midnight_bindgen_runtime::*;
