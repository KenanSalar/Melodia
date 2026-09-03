//! The ring that keeps the network off the audio callback thread.
//!
//! The mixer pulls each [`AudioSource`] from inside the cpal data callback, so a source that
//! reads a socket stalls the whole block — the local track on the other deck included — for as
//! long as the network is wedged. Everything here exists to make that impossible: a feed thread
//! owns the decoder and fills [`SampleRing`] ahead of time, and [`PrebufferSource`] pops from it
//! without ever blocking, yielding silence when it runs dry.
//!
//! Running dry is published rather than hidden. [`StreamShared`] is the one cell the feed thread,
//! the audio thread and the playback monitor all read, and the buffering indicator the UI shows is
//! that flag rather than a second mechanism inferring the same thing.
//!
//! The ring is single-producer / single-consumer by construction: the feed thread is the only
//! writer of `write_cursor`, the audio thread the only writer of `read_cursor`. It borrows its
//! shape from `playback::visualizer`'s per-deck ring, which is the same trade — atomics over a
//! mutex, because one of the two parties is a real-time callback.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use super::audio::{AudioSource, ChannelCount, Sample, SampleRate, SeekError, Shape};

/// How much decoded audio the ring holds before the feed thread has to wait for room.
///
/// This is the *decode and scheduling* cushion: it covers a slow packet, a preempted feed thread
/// and the jitter between cpal's period size and the decoder's frame size. It is deliberately not
/// the network cushion — that one is upstream, measured in compressed bytes, and argued at
/// `stream_source::DOWNLOAD_BUFFER_BYTES`. Widening this trades resident memory for tolerance of
/// something the layer above already tolerates better.
pub const PREBUFFER_MS: u64 = 1_500;

/// How long the feed thread sleeps when it finds the ring full.
///
/// The steady state of a healthy stream is a full ring, so this is the thread's wakeup rate rather
/// than an error path. Short enough that a drained ring refills without an audible gap, long
/// enough that the thread is not spinning through a period's worth of samples at a time.
const FEED_PARK: Duration = Duration::from_millis(20);

/// The state a live stream publishes to everyone who is not on the feed thread.
///
/// Three of the four fields are one-way latches or plain flags read from the audio callback, so
/// they are atomics; the title is the one value with a payload, and its generation counter is what
/// lets the playback monitor ask "did it change?" without taking the lock on every tick.
#[derive(Debug)]
pub struct StreamShared {
    buffering: AtomicBool,
    finished: AtomicBool,
    abandoned: AtomicBool,
    title: parking_lot::Mutex<Option<String>>,
    title_generation: AtomicU64,
}

/// Tickets for [`StreamShared::title_generation`], drawn process-wide rather than counted per
/// cell. The monitor carries **one** last-seen value across stations, so a per-cell counter lets a
/// new station coincide with the number the previous one left behind — and a coincidence there
/// swallows that station's first title for a whole song. A ticket no cell has held cannot.
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
        })
    }

    /// Whether the source is currently emitting silence because the ring ran dry. Drives the UI's
    /// buffering indicator; see `.claude/rules/audio-stack.md` for why it is not
    /// `PlaybackStatus::Loading`.
    pub fn is_buffering(&self) -> bool {
        self.buffering.load(Ordering::Relaxed)
    }

    fn set_buffering(&self, on: bool) {
        self.buffering.store(on, Ordering::Relaxed);
    }

    /// The feed thread has nothing left to give: the stream ended and reconnecting failed, or the
    /// server came back with a format the already-appended source can't carry. Latched, because
    /// the only thing that follows is the deck draining.
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

    /// Publish the live ICY title. Called from the feed thread inside `IcyMetadataReader`'s
    /// callback, which must not block — an uncontended lock and a counter bump is the whole
    /// handler.
    pub fn set_title(&self, title: Option<String>) {
        *self.title.lock() = title;
        self.title_generation.store(next_title_generation(), Ordering::Release);
    }

    /// Moves on every [`Self::set_title`], and never back onto a value any cell has published. The
    /// monitor keeps the last value it saw and only takes the lock when this moved, so a station
    /// that changes track once a song costs one relaxed load per tick.
    pub fn title_generation(&self) -> u64 {
        self.title_generation.load(Ordering::Acquire)
    }

    pub fn title(&self) -> Option<String> {
        self.title.lock().clone()
    }
}

/// The samples in flight between the decoder and the audio callback.
///
/// Cursors are monotonic totals rather than wrapped indices, so `write - read` is the fill level
/// with no ambiguity between full and empty; the modulo when addressing a slot is the only wrap.
/// A `usize` counter at audio rates takes millions of years to overflow.
struct SampleRing {
    /// `f32` bit patterns, one slot per sample.
    slots: Box<[AtomicU32]>,
    write_cursor: AtomicUsize,
    read_cursor: AtomicUsize,
}

impl SampleRing {
    /// A ring holding [`PREBUFFER_MS`] of audio at the source's real rate and channel count, which
    /// is why it is sized here and not by a constant: neither is known until the stream is open.
    fn for_format(channels: ChannelCount, sample_rate: SampleRate) -> Self {
        let frames = u64::from(sample_rate.get()) * PREBUFFER_MS / 1_000;
        let samples = frames * u64::from(channels.get());
        // The floor keeps a frame's worth of room even at an implausibly low rate, so `try_push`
        // can always make progress and the source's whole-frame gate can ever be satisfied.
        let capacity =
            usize::try_from(samples).unwrap_or(usize::MAX).max(usize::from(channels.get()));
        Self {
            // `AtomicU32` isn't `Clone`, so this can't be `vec![…; n]`; collecting also keeps the
            // buffer off the stack on its way out.
            slots: (0..capacity).map(|_| AtomicU32::new(0)).collect(),
            write_cursor: AtomicUsize::new(0),
            read_cursor: AtomicUsize::new(0),
        }
    }

    fn capacity(&self) -> usize {
        self.slots.len()
    }

    /// How many samples are waiting. Safe from either side: each party reads the other's cursor
    /// with `Acquire` and its own with `Relaxed`, and the difference can only be stale in the
    /// direction that makes the caller conservative.
    fn len(&self) -> usize {
        self.write_cursor
            .load(Ordering::Acquire)
            .saturating_sub(self.read_cursor.load(Ordering::Acquire))
    }

    /// Producer side. `false` means the ring is full and the caller should wait.
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

    /// Consumer side, called from the audio callback. Never blocks.
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

/// The producer half, handed to the feed thread.
///
/// Separate from the ring itself so the feed loop can only write and the source can only read —
/// the SPSC invariant is then a property of the types rather than of review.
pub struct RingWriter {
    ring: Arc<SampleRing>,
    shared: Arc<StreamShared>,
}

impl RingWriter {
    /// Push one sample, waiting for room. `false` means the source was dropped and the caller
    /// should stop feeding and close the connection.
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

/// The [`AudioSource`] the decks play for a live stream: a ring pop per sample, silence when
/// starved, and an end only once the feed thread has given up *and* the ring has drained.
///
/// Everything it reports about the audio's shape is pinned at construction. A live mount does not
/// renegotiate its format mid-stream, and a reconnect that comes back with a different one is
/// refused by the feed thread rather than absorbed here — the deck has already been told what this
/// source is.
pub struct PrebufferSource {
    ring: Arc<SampleRing>,
    shared: Arc<StreamShared>,
    shape: Shape,
    /// Which sample of the current frame comes next. The starvation decision is taken once per
    /// frame at phase 0 and held for the whole frame.
    frame_phase: u16,
    frame_starved: bool,
    /// Mirror of `shared.buffering`, so a healthy stream doesn't store to the atomic every frame.
    starved: bool,
}

impl PrebufferSource {
    /// The source and the writer that feeds it, plus the shared cell both report through.
    pub fn new(shared: Arc<StreamShared>, shape: Shape) -> (Self, RingWriter) {
        let ring = Arc::new(SampleRing::for_format(shape.channels, shape.rate));
        let writer = RingWriter {
            ring: ring.clone(),
            shared: shared.clone(),
        };
        let source = Self {
            ring,
            shared,
            shape,
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
    /// Stores a flag and does nothing else. Joining the feed thread here would block for as long
    /// as a socket read takes, and the drop happens wherever a spent source is collected.
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
                // channel parity for whatever plays next, and it is inaudible either way.
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
            // Availability was checked for the whole frame at phase 0 and this is the only
            // consumer, so the fallback is unreachable rather than a swallowed error.
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

    #[inline]
    fn total_duration(&self) -> Option<Duration> {
        None
    }

    fn try_seek(&mut self, _pos: Duration) -> Result<(), SeekError> {
        Err(SeekError::NotSupported {
            underlying_source: "live stream",
        })
    }

    /// A live socket is the kind of claim this exists for, and the flag is one the drop already
    /// sets — so setting it here is the difference between a station releasing its connection when
    /// it stops and when someone next collects it.
    fn release_claims(&mut self) {
        self.shared.abandon();
    }
}

#[cfg(test)]
#[path = "tests/prebuffer_tests.rs"]
mod tests;
