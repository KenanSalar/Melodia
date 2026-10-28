//! The ring that keeps the network off the audio callback thread
//! (`docs/adr/0014-no-network-on-the-audio-callback.md`): a feed thread owns the decoder and fills
//! [`SampleRing`], and [`PrebufferSource`] pops without blocking, yielding silence when dry.
//!
//! Running dry is published rather than hidden: [`StreamShared`] is the one cell the feed thread,
//! the audio thread and the monitor read, and the UI's buffering indicator is that flag. The ring
//! is single-producer, single-consumer by construction, atomics over a mutex because one of the two
//! parties is a real-time callback.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use super::audio::{AudioSource, ChannelCount, Sample, SampleRate, SeekError, Shape, SourceFormat};

/// The decode and scheduling cushion, deliberately not the network one, which sits upstream in
/// compressed bytes (`stream_source::DOWNLOAD_BUFFER_BYTES`). Widening it trades resident memory
/// for a tolerance the layer above already has.
pub const PREBUFFER_MS: u64 = 1_500;

/// A healthy stream's ring is full, so this is the feed thread's wake-up rate: short enough that a
/// drained ring refills without a gap, long enough not to spin.
const FEED_PARK: Duration = Duration::from_millis(20);

/// A station announces once a song and the ring holds seconds, so more than one waiting is a title
/// flapping, of which only the newest is ever shown.
const MAX_PENDING_TITLES: usize = 4;

/// What a live stream publishes to everyone off the feed thread. The flags are atomics, being read
/// from the audio callback; the title's generation lets the monitor ask whether it changed without
/// the lock. **A title is held back until its audio is heard**, the decoder reading it a whole ring
/// ahead of the samples after it.
#[derive(Debug)]
pub struct StreamShared {
    buffering: AtomicBool,
    finished: AtomicBool,
    abandoned: AtomicBool,
    title: parking_lot::Mutex<Option<String>>,
    title_generation: AtomicU64,
    announced: parking_lot::Mutex<Announced>,
    ring: OnceLock<Arc<SampleRing>>,
}

/// What the stream has said, as against what has been published of it.
#[derive(Debug, Default)]
struct Announced {
    pending: VecDeque<PendingTitle>,
    /// So a repeat is dropped: **plenty of servers resend an unchanged title** about once a second.
    last: Option<String>,
    /// Apart from `last`, a cleared title being something a stream can say.
    said_any: bool,
}

/// An announcement and the ring position its audio starts at.
#[derive(Debug)]
struct PendingTitle {
    title: Option<String>,
    at_sample: usize,
}

/// Drawn process-wide rather than per cell: the monitor carries **one** last-seen value across
/// stations, so a per-cell counter could hand a new station the number the previous one left, and
/// swallow its first title for a song.
static NEXT_TITLE_GENERATION: AtomicU64 = AtomicU64::new(1);

fn next_title_generation() -> u64 {
    NEXT_TITLE_GENERATION.fetch_add(1, Ordering::Relaxed)
}

impl StreamShared {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            buffering: AtomicBool::new(false),
            finished: AtomicBool::new(false),
            abandoned: AtomicBool::new(false),
            title: parking_lot::Mutex::new(None),
            title_generation: AtomicU64::new(next_title_generation()),
            announced: parking_lot::Mutex::new(Announced::default()),
            ring: OnceLock::new(),
        })
    }

    /// Emitting silence because the ring ran dry. A flag beside `Playing` rather than
    /// `PlaybackStatus::Loading`, which MPRIS reports as Stopped, so a rebuffer can't read as one.
    pub fn is_buffering(&self) -> bool {
        self.buffering.load(Ordering::Relaxed)
    }

    fn set_buffering(&self, on: bool) {
        self.buffering.store(on, Ordering::Relaxed);
    }

    /// The feed thread has nothing left to give. Latched, the only thing following being the deck
    /// draining.
    pub fn finish(&self) {
        self.finished.store(true, Ordering::Release);
    }

    pub fn is_finished(&self) -> bool {
        self.finished.load(Ordering::Acquire)
    }

    /// The source was dropped, so the feed thread should stop and close the connection.
    fn abandon(&self) {
        self.abandoned.store(true, Ordering::Release);
    }

    pub fn is_abandoned(&self) -> bool {
        self.abandoned.load(Ordering::Acquire)
    }

    /// Queues the ICY title behind the audio already decoded, from a metadata callback that must
    /// not block. A title read during the probe, before the ring exists, starts at its first
    /// sample.
    pub fn set_title(&self, title: Option<String>) {
        let mut announced = self.announced.lock();
        if announced.said_any && announced.last == title {
            return;
        }
        let at_sample = self.ring.get().map_or(0, |ring| ring.write_cursor.load(Ordering::Acquire));
        announced.last.clone_from(&title);
        announced.said_any = true;
        if announced.pending.len() == MAX_PENDING_TITLES {
            announced.pending.pop_front();
        }
        announced.pending.push_back(PendingTitle { title, at_sample });
    }

    /// Publishes the newest queued title whose audio has started. The monitor's to call on its
    /// tick, the audio callback being unable to take a lock.
    pub fn publish_heard_title(&self) {
        let played = self.ring.get().map_or(0, |ring| ring.read_cursor.load(Ordering::Acquire));
        let heard = {
            let mut announced = self.announced.lock();
            let due =
                announced.pending.iter().take_while(|title| title.at_sample <= played).count();
            announced.pending.drain(..due).next_back()
        };
        let Some(heard) = heard else { return };

        *self.title.lock() = heard.title;
        self.title_generation.store(next_title_generation(), Ordering::Release);
    }

    /// Moves on every publish and never back onto a value any cell has held, so the monitor takes
    /// the title's lock once a song.
    pub fn title_generation(&self) -> u64 {
        self.title_generation.load(Ordering::Acquire)
    }

    pub fn title(&self) -> Option<String> {
        self.title.lock().clone()
    }
}

/// Cursors are monotonic totals, so `write - read` is the fill level with no ambiguity between full
/// and empty; a `usize` at audio rates takes millions of years to wrap.
struct SampleRing {
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
    /// Sized here rather than by a constant, the rate and channel count being unknown until open.
    fn for_format(channels: ChannelCount, sample_rate: SampleRate) -> Self {
        let frames = u64::from(sample_rate.get()) * PREBUFFER_MS / 1_000;
        let samples = frames * u64::from(channels.get());
        // A frame's room even at an implausibly low rate, so the whole-frame gate can be satisfied.
        let capacity =
            usize::try_from(samples).unwrap_or(usize::MAX).max(usize::from(channels.get()));
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
        self.write_cursor
            .load(Ordering::Acquire)
            .saturating_sub(self.read_cursor.load(Ordering::Acquire))
    }

    /// `false` means the ring is full and the caller should wait.
    fn try_push(&self, sample: f32) -> bool {
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
    fn try_pop(&self) -> Option<f32> {
        let read = self.read_cursor.load(Ordering::Relaxed);
        if read >= self.write_cursor.load(Ordering::Acquire) {
            return None;
        }
        let sample = f32::from_bits(self.slots[read % self.capacity()].load(Ordering::Relaxed));
        self.read_cursor.store(read + 1, Ordering::Release);
        Some(sample)
    }
}

/// The producer half, so the feed loop can only write and the source only read.
pub struct RingWriter {
    ring: Arc<SampleRing>,
    shared: Arc<StreamShared>,
}

impl RingWriter {
    /// Waits for room. `false` means the source was dropped and the feed should stop.
    pub fn push(&self, sample: f32) -> bool {
        while !self.ring.try_push(sample) {
            if self.shared.is_abandoned() {
                return false;
            }
            std::thread::sleep(FEED_PARK);
        }
        true
    }
}

/// What the decks play for a live stream: a pop per sample, silence when starved, and an end only
/// once the feed thread has given up *and* the ring has drained. Its shape is pinned at
/// construction, a reconnect returning another one being refused by the feed thread.
pub struct PrebufferSource {
    ring: Arc<SampleRing>,
    shared: Arc<StreamShared>,
    shape: Shape,
    format: SourceFormat,
    /// The starvation decision is taken at phase 0 and held for the whole frame.
    frame_phase: u16,
    frame_starved: bool,
    /// Mirror of `shared.buffering`, so a healthy stream doesn't store to the atomic every frame.
    starved: bool,
}

impl PrebufferSource {
    pub fn new(
        shared: Arc<StreamShared>,
        shape: Shape,
        format: SourceFormat,
    ) -> (Self, RingWriter) {
        let ring = Arc::new(SampleRing::for_format(shape.channels, shape.rate));
        // One source per cell, so the first ring, which titles were queued against, is kept.
        let _ = shared.ring.set(ring.clone());
        let writer = RingWriter { ring: ring.clone(), shared: shared.clone() };
        let source = Self {
            ring,
            shared,
            shape,
            format,
            frame_phase: 0,
            frame_starved: false,
            starved: false,
        };
        (source, writer)
    }

    fn publish_starved(&mut self, starved: bool) {
        if self.starved != starved {
            self.starved = starved;
            self.shared.set_buffering(starved);
        }
    }
}

impl Drop for PrebufferSource {
    /// Stores a flag and nothing else: joining the feed thread would block for as long as a socket
    /// read takes, wherever a spent source is collected.
    fn drop(&mut self) {
        self.shared.abandon();
    }
}

impl Iterator for PrebufferSource {
    type Item = Sample;

    #[inline]
    fn next(&mut self) -> Option<Sample> {
        if self.frame_phase == 0 {
            let whole_frame = usize::from(self.shape.channels.get());
            self.frame_starved = self.ring.len() < whole_frame;
            if self.frame_starved {
                // A trailing partial frame goes with it: half a frame would flip this deck's
                // channel parity for whatever plays next.
                if self.shared.is_finished() {
                    return None;
                }
                self.publish_starved(true);
            } else {
                self.publish_starved(false);
            }
        }
        self.frame_phase = (self.frame_phase + 1) % self.shape.channels.get();

        if self.frame_starved {
            Some(0.0)
        } else {
            // Checked for the whole frame at phase 0 with this the only consumer: unreachable.
            Some(self.ring.try_pop().unwrap_or(0.0))
        }
    }
}

impl AudioSource for PrebufferSource {
    #[inline]
    fn channels(&self) -> ChannelCount {
        self.shape.channels
    }

    #[inline]
    fn sample_rate(&self) -> SampleRate {
        self.shape.rate
    }

    fn format(&self) -> SourceFormat {
        self.format
    }

    /// Starvation silence alters nothing the station sent.
    fn dsp_engaged(&self) -> bool {
        false
    }

    #[inline]
    fn total_duration(&self) -> Option<Duration> {
        None
    }

    fn try_seek(&mut self, _pos: Duration) -> Result<(), SeekError> {
        Err(SeekError::NotSupported { underlying_source: "live stream" })
    }

    /// The drop's flag, set here so a stopped station releases its connection when it stops rather
    /// than when it is next collected.
    fn release_claims(&mut self) {
        self.shared.abandon();
    }
}

#[cfg(test)]
#[path = "tests/prebuffer_tests.rs"]
mod tests;
