//! What a decoded stream of samples can come out of, the vocabulary the whole chain is written
//! against, and the loudness measured off that same decode.
//!
//! A new source kind is a new `AudioSource` and nothing else: from here it can't reach the mixer
//! or the state machine, neither being in this crate's manifest. This crate names the network and
//! never cpal, and `melodia-playback` is the other way round.

pub mod loudness;

pub mod player {
    pub mod source;
}
