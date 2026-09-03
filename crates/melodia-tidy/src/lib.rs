//! Repository-wide checks, in one place and outside every crate they check.
//!
//! A pin here asks a question of the *tree*: does anything in the workspace spell this, does
//! every package format ship that, has a rule's glob stopped matching. Those questions have no
//! owner among the thirteen crates, and while they sat inside one they made that crate's suite
//! a whole-tree walk — `cargo test -p melodia-net` compiled `melodia-views`' sources to answer
//! a question about neither. rustc keeps the same checks in `src/tools/tidy` and rust-analyzer
//! behind `cargo xtask tidy`, for the same reason and with the same shape.
//!
//! **The criterion for living here is the corpus, not the subject.** A check that *enumerates*
//! one — [`melodia_testkit::rust_sources`] and its siblings, or a directory under the repo root
//! — belongs here. A pin on one named file does not: an `include_str!` of a sibling module is
//! a unit test about that module's shape and stays beside it.
//!
//! Moving them out bought one property beyond the reach: a walk that sits inside the corpus it
//! walks has to exempt itself, because it spells the needle it greps for. Every exemption of
//! that kind is gone, and the ones that remain now name a real second caller.
//!
//! Everything is an integration test under `tests/`. This file is deliberately empty of code —
//! the crate exists for its test targets, and a `lib.rs` is what gives cargo a package to hang
//! them on.
