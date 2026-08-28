//! Playing a segmented station, by making it stop looking segmented.
//!
//! HLS is a playlist of short files rather than one endless response, so the whole of this module
//! exists to put the endless response back: [`open`] hands out a reader that a scheduler behind it
//! keeps fed, and [`super::stream_decode`] probes that exactly as it probes an Icecast mount.
//!
//! **What this is not is a demuxer.** Symphonia has no MPEG-TS reader and no reserved id for one,
//! which is why segmented stations were carried as unplayable. But audio-only transport streams
//! hold a single elementary stream that is already ADTS AAC or MPEG audio, so what stands between
//! a segment and a decoder is framing, not a format nobody has written. `segment` takes that
//! framing off; the bytes underneath are ones the stream path already reads.
//!
//! Encrypted segments are the one thing a station may serve that this refuses, and it refuses at
//! the playlist with a reason rather than mishandling them quietly. Fragmented MP4 is not the
//! second any more: `playlist` reads the `EXT-X-MAP` header out and `reader` splices it in front
//! of the first segment, which is the whole of what its own demuxer wants and the whole of what a
//! bare `.m4s` cannot supply.

pub mod playlist;
mod reader;
mod segment;

pub use reader::{HlsStream, open};
