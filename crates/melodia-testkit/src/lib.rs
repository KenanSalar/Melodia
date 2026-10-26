//! Shared test fixtures and corpus walkers for every crate in the workspace.
//!
//! Every other member dev-depends on this one, so it names none of them: `Cargo.toml` says why.

mod env;
mod fixtures;
pub mod http;
mod lex;
mod slint;
mod walk;

pub use env::*;
pub use fixtures::*;
pub use lex::*;
pub use slint::*;
pub use walk::*;

// `MELODIA_REPO_ROOT` (`.cargo/config.toml`) already ends in a separator, so the suffixes
// below spell none of their own.

/// The root of the Slint tree, for the pins that walk it rather than naming files.
pub const UI_DIR: &str = concat!(env!("MELODIA_REPO_ROOT"), "crates/melodia-ui/ui");

/// The bundled font faces, which the Slint build compiles into the binary — so every
/// artifact this repo ships redistributes them and owes their licence text.
pub const FONTS_DIR: &str = concat!(env!("MELODIA_REPO_ROOT"), "crates/melodia-ui/ui/assets/fonts");

/// The repo root, for the pins that reach packaging — it lives beside `src/`, not under it.
pub const REPO_ROOT: &str = env!("MELODIA_REPO_ROOT");

/// The checked-in fixtures, for the tests that need a real file rather than a synthesized one.
/// Anchored on the repo root for the reason [`UI_SRC_DIR`] gives.
///
/// At the root rather than inside a crate because three of them read it: a corpus this many
/// members share cannot sit in one member's `tests/`, and the name says it is nobody's target.
pub const ASSETS_DIR: &str = concat!(env!("MELODIA_REPO_ROOT"), "test-assets");

/// The Rust UI tree, for the pins asking the same question of every slice's wiring. Anchored
/// on the repo root like its siblings, which is what the C13 extraction cashed in: a bare
/// `"src/ui"` resolves against the harness's working directory, and that stopped being this
/// tree the moment views became a crate of its own.
pub const UI_SRC_DIR: &str = concat!(env!("MELODIA_REPO_ROOT"), "crates/melodia-views/src/ui");

/// The subsystem-contract rules, whose `paths:` frontmatter decides which loads for which
/// file. Pinned because a stale glob fails *silently* — the rule stops loading for the code
/// it governs and nothing in the build, the lint gate or the test suite is looking.
pub const RULES_DIR: &str = concat!(env!("MELODIA_REPO_ROOT"), ".claude/rules");
