//! The stretch of a file one cue sheet track plays, in the decoder's sample frames. It narrows the
//! [`super::file_decode::FileDecoder`] rather than anything above it: a deck's clock counts the
//! samples it pulls from where the track started, so a source that opens on the span's first
//! sample, ends on its last and states its length is read right by every reader of position and
//! length, the seek, MPRIS, the scrobble threshold and the resume after a restart included.

use std::time::Duration;

use melodia_core::entities::cue::CueSpan;

use super::audio::{SampleRate, frames_to_duration};

/// A [`CueSpan`] at one sample rate, counted from the first real sample of the file.
#[derive(Clone, Copy)]
pub(super) struct Span {
    pub start: u64,
    /// `None` plays on to the end of the file.
    pub len: Option<u64>,
}

impl Span {
    pub(super) fn at_rate(span: CueSpan, rate: SampleRate) -> Self {
        let start = span.start().to_frames(rate);
        let len = span.end().map(|end| end.to_frames(rate).saturating_sub(start));
        Self { start, len }
    }

    /// How long the span plays, `file` being how long the whole file does.
    pub(super) fn duration(self, file: Option<Duration>, rate: SampleRate) -> Option<Duration> {
        match self.len {
            Some(len) => Some(frames_to_duration(len, rate)),
            None => file.map(|file| file.saturating_sub(frames_to_duration(self.start, rate))),
        }
    }
}
