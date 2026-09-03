//! What a decoded stream of samples can come out of, and the vocabulary above it.
//!
//! [`audio`] is the interface the rest of the chain is written against, and it imports nothing.
//! [`decode`] is the one Symphonia: the probe, the track pick, the decoder build and the packet
//! cursor, with [`file_decode`] and [`stream_decode`] differing only in the `MediaSource` and
//! `Hint` they hand it. The live trio ([`stream_source`], [`prebuffer`], [`hls`]) is the
//! network's end, the ring keeping a blocking socket read off the audio callback thread.

pub mod aac_config;
pub mod aac_trim;
pub mod audio;
pub mod decode;
pub mod file_decode;
pub mod hls;
pub mod prebuffer;
pub mod stream_decode;
pub mod stream_source;
