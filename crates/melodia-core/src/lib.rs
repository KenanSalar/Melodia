//! The vocabulary every other crate is written against: the error type, the path resolver, the
//! boundary DTOs, the theme registry, and the primitives that may be named from any layer.
//!
//! It names nothing. `error.rs` holds no `crate::` path at all and the other four name only
//! each other, which is what makes this the one crate eleven others can depend on.

pub mod config;
pub mod entities;
pub mod error;
pub mod themes;
pub mod utils;

// The corpus walkers and env-lock fixtures. Aliased so `crate::test_support::…` keeps resolving
// in the test modules that spell it.
#[cfg(test)]
pub(crate) use melodia_testkit as test_support;
