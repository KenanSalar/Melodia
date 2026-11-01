//! The ring that keeps a blocking read off the audio callback thread: a feeder thread owns the
//! decoder and pushes, and the callback pops without blocking, a whole frame at a time.
//! Single-producer, single-consumer by construction, atomics over a mutex because one of the two
//! parties is a real-time callback.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::time::Duration;

use super::audio::{ChannelCount, Sample, Shape, frames_in, interleaved};

/// What a ring's two ends and its feeder agree on besides the samples. Apart from the ring because a
/// station's exists before its ring does, its connection being read while it is probed.
#[derive(Debug, Default)]
pub struct FeedState {
    finished: AtomicBool,
    abandoned: AtomicBool,
}

impl FeedState {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// The feeder has nothing left to give. Latched, the only thing following being the deck
    /// draining.
    pub fn finish(&self) {
        self.finished.store(true, Ordering::Release);
    }

    pub fn is_finished(&self) -> bool {
        self.finished.load(Ordering::Acquire)
    }

    /// The source was dropped, so the feeder should stop and close what it reads.
    fn abandon(&self) {
        self.abandoned.store(true, Ordering::Release);
    }

    pub fn is_abandoned(&self) -> bool {
        self.abandoned.load(Ordering::Acquire)
    }
}

/// Cursors are monotonic totals, so `write - read` is the fill level with no ambiguity between full
/// and empty; a `usize` at audio rates takes millions of years to wrap.
pub(super) struct SampleRing {
    /// `f32` bit patterns, one slot per sample.
    slots: Box<[AtomicU32]>,
    write_cursor: AtomicUsize,
    read_cursor: AtomicUsize,
}

impl std::fmt::Debug for SampleRing {
    // The slots are seconds of audio, which no log line wants.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SampleRing")
            .field("write_cursor", &self.write_cursor)
            .field("read_cursor", &self.read_cursor)
            .finish_non_exhaustive()
    }
}

impl SampleRing {
    /// `depth` of audio at `shape`, which is unknown until the decoder opens.
    fn new(shape: Shape, depth: Duration) -> Self {
        let samples = interleaved(frames_in(depth, shape.rate), shape.channels);
        // A frame's room even at an implausibly low rate, so the whole-frame gate can be satisfied.
        let capacity =
            usize::try_from(samples).unwrap_or(usize::MAX).max(usize::from(shape.channels.get()));
        Self {
            // `AtomicU32` isn't `Clone`, so this can't be `vec![…; n]`.
            slots: (0..capacity).map(|_| AtomicU32::new(0)).collect(),
            write_cursor: AtomicUsize::new(0),
            read_cursor: AtomicUsize::new(0),
        }
    }

    fn capacity(&self) -> usize {
        self.slots.len()
    }

    /// Safe from either side: each reads the other's cursor with `Acquire`, and the difference can
    /// only be stale in the direction that makes the caller conservative.
    fn len(&self) -> usize {
        self.pushed().saturating_sub(self.popped())
    }

    /// Samples pushed since the ring opened, which a station's title is queued against.
    pub(super) fn pushed(&self) -> usize {
        self.write_cursor.load(Ordering::Acquire)
    }

    /// Samples popped since the ring opened, which a station's title is published against.
    pub(super) fn popped(&self) -> usize {
        self.read_cursor.load(Ordering::Acquire)
    }

    /// `false` means the ring is full and the caller should wait.
    fn try_push(&self, sample: Sample) -> bool {
        let write = self.write_cursor.load(Ordering::Relaxed);
        if write - self.read_cursor.load(Ordering::Acquire) >= self.capacity() {
            return false;
        }
        self.slots[write % self.capacity()].store(sample.to_bits(), Ordering::Relaxed);
        // Releases the slot store above to whichever `try_pop` loads this cursor next.
        self.write_cursor.store(write + 1, Ordering::Release);
        true
    }

    /// Called from the audio callback. Never blocks.
    fn try_pop(&self) -> Option<Sample> {
        let read = self.read_cursor.load(Ordering::Relaxed);
        if read >= self.write_cursor.load(Ordering::Acquire) {
            return None;
        }
        let sample = f32::from_bits(self.slots[read % self.capacity()].load(Ordering::Relaxed));
        self.read_cursor.store(read + 1, Ordering::Release);
        Some(sample)
    }
}

/// A ring holding `depth` of audio at `shape`, its writer napping `park` whenever it finds the ring
/// full. Split in two so the feeder can only write and the source only read.
pub fn ring(
    shape: Shape,
    depth: Duration,
    park: Duration,
    feed: Arc<FeedState>,
) -> (RingWriter, RingReader) {
    let ring = Arc::new(SampleRing::new(shape, depth));
    let writer = RingWriter { ring: Arc::clone(&ring), feed: Arc::clone(&feed), park };
    let reader =
        RingReader { ring, feed, channels: shape.channels, frame_phase: 0, frame_starved: false };
    (writer, reader)
}

pub struct RingWriter {
    ring: Arc<SampleRing>,
    feed: Arc<FeedState>,
    park: Duration,
}

impl RingWriter {
    /// Waits for room. `false` means the source was dropped and the feed should stop.
    pub fn push(&self, sample: Sample) -> bool {
        while !self.ring.try_push(sample) {
            if self.feed.is_abandoned() {
                return false;
            }
            std::thread::sleep(self.park);
        }
        true
    }

    /// Whether a [`Self::push`] would wait, for a fill that has to stop rather than nap.
    pub fn is_full(&self) -> bool {
        self.ring.len() >= self.ring.capacity()
    }

    pub fn finish(&self) {
        self.feed.finish();
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Pulled {
    Sample(Sample),
    /// Less than a whole frame is there, so this whole frame is owed as silence.
    Starved,
    /// The feeder has finished and less than a frame is left.
    Ended,
}

pub struct RingReader {
    ring: Arc<SampleRing>,
    feed: Arc<FeedState>,
    channels: ChannelCount,
    /// The starvation decision is taken at phase 0 and held for the whole frame.
    frame_phase: u16,
    frame_starved: bool,
}

impl RingReader {
    /// Never blocks, the audio callback being its caller.
    pub fn pull(&mut self) -> Pulled {
        if self.frame_phase == 0 {
            // Before the fill level: the feeder's last push lands ahead of its finish, so a short
            // ring once finished stays short.
            let finished = self.feed.is_finished();
            self.frame_starved = self.ring.len() < usize::from(self.channels.get());
            // A trailing partial frame goes with it: half a frame would flip this deck's channel
            // parity for whatever plays next.
            if self.frame_starved && finished {
                return Pulled::Ended;
            }
        }
        self.frame_phase = (self.frame_phase + 1) % self.channels.get();

        if self.frame_starved {
            return Pulled::Starved;
        }
        // Checked for the whole frame at phase 0 with this the only consumer: unreachable.
        Pulled::Sample(self.ring.try_pop().unwrap_or(0.0))
    }

    /// Samples pushed and not yet pulled.
    pub fn buffered(&self) -> usize {
        self.ring.len()
    }

    pub fn is_finished(&self) -> bool {
        self.feed.is_finished()
    }

    /// The drop's flag, for a source giving up its feeder before it is collected.
    pub fn abandon(&self) {
        self.feed.abandon();
    }

    pub(super) fn ring(&self) -> Arc<SampleRing> {
        Arc::clone(&self.ring)
    }
}

impl Drop for RingReader {
    /// Stores a flag and nothing else: joining the feeder would block for as long as its read
    /// takes, wherever a spent source is collected.
    fn drop(&mut self) {
        self.feed.abandon();
    }
}
