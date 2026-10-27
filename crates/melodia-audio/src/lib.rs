//! What a decoded stream of samples can come out of, and the vocabulary the whole chain is
//! written against.
//!
//! A new source kind is a new `AudioSource` and nothing else: from here it can't reach the mixer
//! or the state machine, neither being in this crate's manifest. This crate names the network and
//! never cpal, and `melodia-playback` is the other way round.

pub mod player {
    pub mod source;
}
