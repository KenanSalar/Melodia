//! One voice: the source playing on it, the one staged behind, and the transport state the mixer
//! reads while it pulls.
//!
//! Split in two because the two halves live on different threads. [`Deck`] is the control side and
//! is all the rest of the tree ever sees; [`DeckVoice`] is owned by the mixer and touched only from
//! the audio callback. Between them sit a command channel and a pair of counters.
//!
//! **`clear` and `seek` block until the callback has serviced them**, which is not an accident of
//! the implementation but the contract the layer above rests on: `Decks::cut_to` clears both decks
//! and then starts a source on one, and a clear that had not landed yet would take the new source
//! with it. `tests/crossfade.rs` is the audio thread while it pulls, which is why a control op that
//! blocks has to be driven from another thread there.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, sync_channel};
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use super::super::audio::{AudioSource, Sample, SeekError};
use super::convert::{Converter, Filled, Shape};

/// How long a control op waits for the callback before giving up.
///
/// Long enough to cover any plausible device period, short enough that a stream which has stopped
/// being serviced — a device pulled mid-track — stalls the transport for an eyeblink rather than
/// wedging the UI thread behind it. rodio waited forever here, which is only safe while the stream
/// is guaranteed to be running.
const SERVICE_TIMEOUT: Duration = Duration::from_millis(500);

/// How long the waiter sleeps between checks. Below one device period, so the wait ends on the
/// callback that services it rather than on the next tick of a coarser timer.
const SERVICE_POLL: Duration = Duration::from_millis(1);

/// How many control ops can be in flight before the sender gives up on the callback.
///
/// Bounded so the queue is allocated once rather than per send. Nothing issues ops faster than the
/// transport can be clicked, so reaching this means the callback has stopped draining.
const COMMAND_SLOTS: usize = 8;

/// A source and the converter that brings it to the device.
///
/// Built on the control thread, because [`Converter::new`] allocates and the thread it would
/// otherwise be built on is the one that must not.
///
/// **Dropped on the audio thread**, which frees a decoder's 64 KiB read buffer there: too big for
/// glibc's per-thread cache, so it takes the arena lock this process shares with the UI and tokio
/// threads. Handing finished sources back to a control thread instead was tried and reverted — the
/// visualizer's per-deck liveness is scoped to the source's *drop*, so deferring it leaves a
/// finished deck's ring mixing into the analysis window until someone collects.
struct Voice {
    source: Box<dyn AudioSource>,
    converter: Converter,
}

enum Command {
    Append(Voice),
    Clear,
    Seek(Duration),
}

/// What both sides read without going through the channel.
struct DeckShared {
    paused: AtomicBool,
    /// `f64` bit patterns, so the transport's own type reaches the mixer without a narrowing.
    volume: AtomicU64,
    speed: AtomicU64,
    /// Sources appended and not yet finished. Bumped by `append` before the callback can see the
    /// source, so a just-fed deck never reads as empty.
    sources: AtomicUsize,
    /// Frames the current source has been pulled for, on its own timeline, and that source's rate.
    /// The pair is the clock: media position is one divided by the other.
    frames: AtomicU64,
    rate: AtomicU32,
    /// Commands sent, and commands the callback has drained. A control op that must land before it
    /// returns waits for the second to reach the first.
    issued: AtomicU64,
    serviced: AtomicU64,
    /// Where the callback leaves a seek's outcome for the control thread waiting on it.
    seek_result: Mutex<Option<Result<(), SeekError>>>,
}

/// The control side of one voice.
pub struct Deck {
    shared: Arc<DeckShared>,
    commands: SyncSender<Command>,
    device: Shape,
}

impl Deck {
    /// Queue `source` behind whatever is already on this deck.
    pub fn append<S: AudioSource + 'static>(&self, source: S) {
        let shape = Shape {
            channels: source.channels(),
            rate: source.sample_rate(),
        };
        let voice = Voice {
            source: Box::new(source),
            converter: Converter::new(shape, self.device),
        };
        // Before the send, so the count is never behind what the callback can already see.
        self.shared.sources.fetch_add(1, Ordering::SeqCst);
        if !self.send(Command::Append(voice)) {
            self.shared.sources.fetch_sub(1, Ordering::SeqCst);
        }
    }

    /// Drop everything on this deck and pause it, as rodio's `clear` did.
    pub fn clear(&self) {
        self.pause();
        if self.shared.sources.load(Ordering::SeqCst) == 0 {
            return;
        }
        if self.send(Command::Clear) {
            self.await_service();
        }
    }

    /// Seek the playing source. Nothing playing is not an error — there is simply nowhere to go.
    ///
    /// # Errors
    ///
    /// Whatever the source's own `try_seek` returned.
    pub fn try_seek(&self, position: Duration) -> Result<(), SeekError> {
        if self.shared.sources.load(Ordering::SeqCst) == 0 {
            return Ok(());
        }
        *self.shared.seek_result.lock() = None;
        if !self.send(Command::Seek(position)) {
            return Ok(());
        }
        self.await_service();
        // A timed-out wait leaves the slot empty, which reads as the seek having landed. That is
        // the same answer rodio gave when its feedback channel closed, and the alternative is a
        // warning about a stream that has already stopped producing audio.
        self.shared.seek_result.lock().take().unwrap_or(Ok(()))
    }

    pub fn play(&self) {
        self.shared.paused.store(false, Ordering::SeqCst);
    }

    pub fn pause(&self) {
        self.shared.paused.store(true, Ordering::SeqCst);
    }

    pub fn is_paused(&self) -> bool {
        self.shared.paused.load(Ordering::SeqCst)
    }

    /// Sources appended and not yet finished.
    pub fn len(&self) -> usize {
        self.shared.sources.load(Ordering::Relaxed)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn set_volume(&self, volume: f64) {
        self.shared.volume.store(volume.to_bits(), Ordering::Relaxed);
    }

    pub fn set_speed(&self, speed: f64) {
        self.shared.speed.store(speed.to_bits(), Ordering::Relaxed);
    }

    pub fn speed(&self) -> f64 {
        f64::from_bits(self.shared.speed.load(Ordering::Relaxed))
    }

    /// Where the playing source has been pulled to, on its own timeline.
    ///
    /// Media time directly: the frames counted are the source's, and playback speed is applied
    /// below this by the converter rather than by inflating the rate reported upward.
    pub fn position(&self) -> Duration {
        let rate = u64::from(self.shared.rate.load(Ordering::Relaxed));
        if rate == 0 {
            return Duration::ZERO;
        }
        let frames = self.shared.frames.load(Ordering::Relaxed);
        Duration::from_micros(frames.saturating_mul(1_000_000) / rate)
    }

    /// Whether the callback will see `command`.
    ///
    /// The bump lands *after* the send, so a ticket the callback can observe is one whose command
    /// it will also find on the channel.
    fn send(&self, command: Command) -> bool {
        // Full means the callback has stopped draining, which a bounded wait is about to report
        // anyway; blocking here would do it while holding the transport instead.
        if self.commands.try_send(command).is_err() {
            return false;
        }
        self.shared.issued.fetch_add(1, Ordering::Release);
        true
    }

    /// Wait for the callback to drain what has been sent so far.
    fn await_service(&self) {
        let ticket = self.shared.issued.load(Ordering::Acquire);
        let deadline = Instant::now() + SERVICE_TIMEOUT;
        while self.shared.serviced.load(Ordering::Acquire) < ticket {
            if Instant::now() >= deadline {
                log::warn!(
                    "audio callback did not service a control op within {SERVICE_TIMEOUT:?}"
                );
                return;
            }
            std::thread::sleep(SERVICE_POLL);
        }
    }
}

/// The audio side of one voice. Pulled only from the callback.
pub struct DeckVoice {
    shared: Arc<DeckShared>,
    commands: Receiver<Command>,
    current: Option<Voice>,
    staged: VecDeque<Voice>,
}

impl DeckVoice {
    /// Write this voice's output into `block`, already sized to whole device frames, and say how
    /// far it reached.
    ///
    /// Volume is applied here rather than by the mixer so that **unity is a short circuit**: a deck
    /// at full volume hands its samples over untouched, which is what makes a lone deck through the
    /// whole chain bit-identical to what the decoder produced.
    ///
    /// Servicing the control channel comes first and happens even while paused, or a deck could not
    /// be loaded or cleared without being played.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "volume is bounded to 0.0..=1.0, whose round-trip through f32 is inaudible"
    )]
    pub fn render(&mut self, block: &mut [Sample]) -> usize {
        self.service();
        if self.shared.paused.load(Ordering::SeqCst) {
            return 0;
        }
        let speed = f64::from_bits(self.shared.speed.load(Ordering::Relaxed));
        // Compared as a bit pattern rather than a value, because what is being asked is whether the
        // multiply can be skipped exactly, and an epsilon would answer yes a shade off unity.
        let volume_bits = self.shared.volume.load(Ordering::Relaxed);

        let mut written = 0;
        while written < block.len() {
            let Some(voice) = self.current.as_mut() else {
                break;
            };
            let Filled {
                samples,
                source_frames,
            } = voice.converter.fill(&mut block[written..], &mut *voice.source, speed);
            written += samples;
            self.shared.frames.fetch_add(source_frames, Ordering::Relaxed);
            if voice.converter.is_done() {
                self.finish_current();
            }
        }

        if volume_bits != 1.0_f64.to_bits() {
            let volume = f64::from_bits(volume_bits) as Sample;
            for slot in &mut block[..written] {
                *slot *= volume;
            }
        }
        written
    }

    /// Drain the control side's commands. First thing in every fill, so an op issued between two
    /// callbacks lands at the head of the next one rather than part way through it.
    fn service(&mut self) {
        let seen = self.shared.issued.load(Ordering::Acquire);
        loop {
            match self.commands.try_recv() {
                Ok(Command::Append(voice)) => self.accept(voice),
                Ok(Command::Clear) => self.clear(),
                Ok(Command::Seek(position)) => self.seek(position),
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
        self.shared.serviced.store(seen, Ordering::Release);
    }

    fn accept(&mut self, voice: Voice) {
        if self.current.is_none() {
            self.start(voice);
        } else {
            self.staged.push_back(voice);
        }
    }

    /// Make `voice` the playing source and re-anchor the clock on it.
    fn start(&mut self, voice: Voice) {
        self.shared.rate.store(voice.source.sample_rate().get(), Ordering::Relaxed);
        self.shared.frames.store(0, Ordering::Relaxed);
        self.current = Some(voice);
    }

    /// The playing source ran out: drop it and take the staged one, if any.
    ///
    /// The drop is what releases the visualizer's claim on this deck's ring, so it happens here
    /// rather than being deferred — see [`Voice`].
    fn finish_current(&mut self) {
        if self.current.take().is_some() {
            self.shared.sources.fetch_sub(1, Ordering::SeqCst);
        }
        if let Some(next) = self.staged.pop_front() {
            self.start(next);
        }
    }

    fn clear(&mut self) {
        let dropped = usize::from(self.current.take().is_some()) + self.staged.len();
        self.staged.clear();
        self.shared.sources.fetch_sub(dropped, Ordering::SeqCst);
        self.shared.frames.store(0, Ordering::Relaxed);
    }

    fn seek(&mut self, position: Duration) {
        let Some(voice) = self.current.as_mut() else {
            return;
        };
        let result = voice.source.try_seek(position);
        if result.is_ok() {
            // Integer throughout: the count this re-anchors is what `Deck::position` divides back
            // down, so a float round trip here would read back a millisecond off its own target.
            let rate = u128::from(voice.source.sample_rate().get());
            let frames = position.as_micros().saturating_mul(rate) / 1_000_000;
            self.shared.frames.store(u64::try_from(frames).unwrap_or(u64::MAX), Ordering::Relaxed);
        }
        if let Some(mut slot) = self.shared.seek_result.try_lock() {
            *slot = Some(result);
        }
    }
}

/// Build one voice's two halves.
pub fn pair(device: Shape) -> (Deck, DeckVoice) {
    let (command_tx, command_rx) = sync_channel(COMMAND_SLOTS);

    let shared = Arc::new(DeckShared {
        paused: AtomicBool::new(false),
        volume: AtomicU64::new(1.0_f64.to_bits()),
        speed: AtomicU64::new(1.0_f64.to_bits()),
        sources: AtomicUsize::new(0),
        frames: AtomicU64::new(0),
        rate: AtomicU32::new(0),
        issued: AtomicU64::new(0),
        serviced: AtomicU64::new(0),
        seek_result: Mutex::new(None),
    });

    let deck = Deck {
        shared: shared.clone(),
        commands: command_tx,
        device,
    };
    let voice = DeckVoice {
        shared,
        commands: command_rx,
        current: None,
        staged: VecDeque::with_capacity(2),
    };
    (deck, voice)
}

#[cfg(test)]
#[path = "tests/deck_tests.rs"]
mod tests;
