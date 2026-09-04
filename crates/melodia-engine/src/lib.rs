//! The state machine, the queue, the action list and the backend that runs them.
//!
//! The top of the audio stack and the only tier that names neither cpal nor the network: what it
//! takes from the two below is a `Deck` to drive and an `AudioSource` to hand it. `.claude/rules/
//! audio-stack.md` carries the contracts all three share.
//!
//! `fixtures` is `#[doc(hidden)] pub` rather than `#[cfg(test)]` because three crates read it and
//! a `cfg(test)` item cannot cross a crate boundary; the module says so at its own head.

pub mod player {
    pub mod engine;
}
