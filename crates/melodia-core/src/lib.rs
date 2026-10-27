//! The vocabulary every other crate is written against: errors, paths, boundary DTOs, themes and
//! the primitives any layer may name. It names no other member, and `error.rs` no `crate::` path.

pub mod config;
pub mod entities;
pub mod error;
pub mod themes;
pub mod utils;
