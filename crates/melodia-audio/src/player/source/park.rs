//! The one-slot hand-off that carries a cue track's decoder on into the next track of its file. A
//! track that plays its span out leaves its decoder standing on the next one's first sample, and
//! the preload of that track takes it rather than opening the file again: no seek and no decoder
//! reset, so a lossy codec's overlap runs on across the joint and a long MP3 is never re-walked for
//! an accurate seek.
//!
//! A decoder goes only to a track of the same file starting on the cue frame it stopped at. One
//! slot, since only the track after the playing one is ever staged, and a track start or a stop
//! empties it: a decoder nobody is coming for holds its file open.

use std::sync::Arc;

use parking_lot::Mutex;

use melodia_core::entities::cue::CueFrames;
use melodia_core::entities::track::TrackAudio;

use super::file_decode::FileDecoder;

/// One per engine, the preload its only taker.
#[derive(Default)]
pub struct ParkSlot {
    state: Mutex<SlotState>,
}

/// Nothing drops a decoder while holding this, closing its file being I/O.
#[derive(Default)]
struct SlotState {
    parked: Option<ParkedDecoder>,
    /// Moved by every emptying, which voids the tickets handed out before it.
    generation: u64,
}

struct ParkedDecoder {
    boundary: Boundary,
    decoder: FileDecoder,
}

/// Where a cue track ends inside its file, which is where the track after it starts.
struct Boundary {
    path: String,
    at: CueFrames,
}

impl Boundary {
    fn starts(&self, audio: &TrackAudio) -> bool {
        self.path == audio.path && audio.span.is_some_and(|span| span.start() == self.at)
    }
}

/// A decode-ahead feeder's leave to park its decoder once its track has played out, good until the
/// slot is next emptied.
pub struct ParkTicket {
    slot: Arc<ParkSlot>,
    generation: u64,
    boundary: Boundary,
}

impl ParkSlot {
    /// A ticket for the source playing `audio`, `None` for a track that plays on to the end of its
    /// file, which nothing in the file follows.
    pub fn ticket(self: &Arc<Self>, audio: &TrackAudio) -> Option<ParkTicket> {
        let at = audio.span?.end()?;
        let generation = self.state.lock().generation;
        let boundary = Boundary { path: audio.path.clone(), at };
        Some(ParkTicket { slot: Arc::clone(self), generation, boundary })
    }

    /// The decoder that played the track before `audio` in its file, narrowed to `audio` and
    /// standing on its first sample. Empties the slot either way.
    pub fn take(&self, audio: &TrackAudio) -> Option<FileDecoder> {
        let parked = self.state.lock().empty();
        let ParkedDecoder { mut decoder, .. } =
            parked.filter(|parked| parked.boundary.starts(audio))?;
        decoder.narrow_to(audio.span?);
        Some(decoder)
    }

    /// Lets go of the parked decoder and voids every ticket out, for a track start or a stop.
    pub fn clear(&self) {
        let released = self.state.lock().empty();
        drop(released);
    }
}

impl SlotState {
    /// Voids every ticket out and hands the parked decoder back, to drop once the lock is released.
    fn empty(&mut self) -> Option<ParkedDecoder> {
        self.generation += 1;
        self.parked.take()
    }
}

impl ParkTicket {
    /// Leaves `decoder` for the track after its own, where it played its span out and the slot
    /// hasn't been emptied since this ticket was handed out.
    pub(super) fn park(self, decoder: FileDecoder) {
        if !decoder.played_span_out() {
            return;
        }
        let parked = ParkedDecoder { boundary: self.boundary, decoder };
        let mut state = self.slot.state.lock();
        let released = if state.generation == self.generation {
            state.parked.replace(parked)
        } else {
            Some(parked)
        };
        drop(state);
        drop(released);
    }
}
