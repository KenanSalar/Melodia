//! A station's end of the ring that keeps the network off the audio callback thread
//! (`docs/adr/0014-no-network-on-the-audio-callback.md`): [`PrebufferSource`] plays it, yielding
//! silence when dry.
//!
//! Running dry is published rather than hidden: [`StreamShared`] is the one cell the feed thread,
//! the audio thread and the monitor read, and the UI's buffering indicator is that flag.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use super::audio::{AudioSource, ChannelCount, Sample, SampleRate, SeekError, Shape, SourceFormat};
use super::sample_ring::{self, FeedState, Pulled, RingReader, RingWriter, SampleRing};

/// The decode and scheduling cushion, deliberately not the network one, which sits upstream in
/// compressed bytes (`stream_source::DOWNLOAD_BUFFER_BYTES`). Widening it trades resident memory
/// for a tolerance the layer above already has.
const PREBUFFER: Duration = Duration::from_millis(1_500);

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
    feed: Arc<FeedState>,
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
            feed: FeedState::new(),
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

    pub fn finish(&self) {
        self.feed.finish();
    }

    pub fn is_finished(&self) -> bool {
        self.feed.is_finished()
    }

    pub fn is_abandoned(&self) -> bool {
        self.feed.is_abandoned()
    }

    /// Queues the ICY title behind the audio already decoded, from a metadata callback that must
    /// not block. A title read during the probe, before the ring exists, starts at its first
    /// sample.
    pub fn set_title(&self, title: Option<String>) {
        let mut announced = self.announced.lock();
        if announced.said_any && announced.last == title {
            return;
        }
        let at_sample = self.ring.get().map_or(0, |ring| ring.pushed());
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
        let played = self.ring.get().map_or(0, |ring| ring.popped());
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

/// What the decks play for a live stream: a pop per sample, silence when starved, and an end only
/// once the feed thread has given up *and* the ring has drained. Its shape is pinned at
/// construction, a reconnect returning another one being refused by the feed thread.
pub struct PrebufferSource {
    reader: RingReader,
    shared: Arc<StreamShared>,
    shape: Shape,
    format: SourceFormat,
    /// Mirror of `shared.buffering`, so a healthy stream doesn't store to the atomic every frame.
    starved: bool,
}

impl PrebufferSource {
    pub fn new(
        shared: Arc<StreamShared>,
        shape: Shape,
        format: SourceFormat,
    ) -> (Self, RingWriter) {
        let (writer, reader) =
            sample_ring::ring(shape, PREBUFFER, FEED_PARK, Arc::clone(&shared.feed));
        // One source per cell, so the first ring, which titles were queued against, is kept.
        let _ = shared.ring.set(reader.ring());
        let source = Self { reader, shared, shape, format, starved: false };
        (source, writer)
    }

    fn publish_starved(&mut self, starved: bool) {
        if self.starved != starved {
            self.starved = starved;
            self.shared.set_buffering(starved);
        }
    }
}

impl Iterator for PrebufferSource {
    type Item = Sample;

    #[inline]
    fn next(&mut self) -> Option<Sample> {
        match self.reader.pull() {
            Pulled::Sample(sample) => {
                self.publish_starved(false);
                Some(sample)
            }
            Pulled::Starved => {
                self.publish_starved(true);
                Some(0.0)
            }
            Pulled::Ended => None,
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
        self.reader.abandon();
    }
}

#[cfg(test)]
#[path = "tests/prebuffer_tests.rs"]
mod tests;
