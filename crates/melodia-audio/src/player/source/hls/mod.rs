//! Playing a segmented station by making it stop looking segmented: [`open`] hands out a reader a
//! scheduler keeps fed, and [`super::stream_decode`] probes it as it probes an Icecast mount.
//!
//! **Not a demuxer**, Symphonia having no MPEG-TS reader. An audio-only transport stream carries
//! one elementary stream that is already ADTS AAC or MPEG audio, so `segment` takes the framing off
//! and the stream path reads what is underneath. Fragmented MP4 passes through with its `EXT-X-MAP`
//! header spliced ahead of the first segment, and an encrypted playlist is refused with a reason.

pub mod playlist;
mod reader;
mod segment;

pub use reader::{HlsStream, open};
