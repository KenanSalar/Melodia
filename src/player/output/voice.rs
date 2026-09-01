//! One voice: the source playing on it, the one staged behind, and the transport state the mixer
//! reads while it pulls.
//!
//! Split in two because the two halves live on different threads. [`Voice`] is the control side and
//! is all the rest of the tree ever sees; [`VoicePull`] is owned by the mixer and touched only from
//! the audio callback. Between them sit a command channel and a pair of counters.
//!
//! **`clear` blocks until the callback has serviced it**, which is not an accident of the
//! implementation but the contract the layer above rests on: `Decks::cut_to` clears both decks and
//! then starts a source on one, and a clear that had not landed yet would take the new source with
//! it. `tests/crossfade.rs` is the audio thread while it pulls, which is why a control op that
//! blocks has to be driven from another thread there. It is the only one: a seek is a `replace`,
//! which carries a source the caller has already positioned and so has nothing to wait for.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, sync_channel};
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use super::super::audio::{AudioSource, Sample, SampleRate, Shape};
use super::super::dsp::AtomicF64;
use super::convert::{Converter, Filled};

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
///
/// It is also what [`VoicePull::staged`] is sized against. The layer above stages one source
/// behind the playing one and no more, but a `clear` too full to send never reaches the callback,
/// so the append behind it lands on a source that should already have gone. Sized to cover one
/// drain, an allocation being the one thing that thread must not do.
const COMMAND_SLOTS: usize = 8;

/// How many spent sources may await collection before the callback frees one itself.
///
/// A clear retires the playing source and the staged one together, so two per voice covers the
/// widest single op; the rest is slack for a collector that has not run yet. Full means falling
/// back to the drop this exists to avoid, which is a worse tick rather than a broken one.
const SPENT_SLOTS: usize = 8;

/// A source and the converter that brings it to the device.
///
/// Built on the control thread, because [`Converter::new`] allocates and the thread it would
/// otherwise be built on is the one that must not.
///
/// **Freed off the audio thread too**, through [`VoicePull::retire`]: dropping one here frees a
/// decoder's 64 KiB read buffer, too big for glibc's per-thread cache, so it takes the arena lock
/// this process shares with the UI and tokio threads.
///
/// Deferring the whole drop was tried once and reverted, because the visualizer scopes a deck's
/// liveness to its source's *drop*, so a spent source awaiting collection kept a dead deck's ring
/// in the analysis window. What makes it work now is that the two halves are separable:
/// [`AudioSource::release_claims`] gives up the claim on the callback, where it is a counter
/// decrement, and only the free is handed back.
struct Loaded {
    source: Box<dyn AudioSource>,
    converter: Converter,
}

enum Command {
    Append(Loaded),
    Clear,
    /// Swap the playing source for one the control thread has already positioned, anchoring the
    /// clock at `frames` of it.
    ///
    /// This is what a seek is. Moving a *mounted* source meant running the demuxer's scan inside
    /// the callback, on a thread that may not read a file, and blocking the caller on the result;
    /// building the sought source first leaves this side a pointer swap. `frames` comes with it
    /// because the clock is re-anchored by whoever knows where the source now is, and `mounted`
    /// because the swap has to be refused if what it was built against has since been replaced.
    Replace {
        loaded: Loaded,
        frames: u64,
        mounted: u64,
    },
}

/// What both sides read without going through the channel.
struct VoiceShared {
    paused: AtomicBool,
    /// Both `f64` because the ratio is: [`Converter::fill`] takes `speed` at that width, and a
    /// narrowing here would round it before the step it scales. Volume rides the same width for
    /// the pair's sake and narrows at its one use, where the multiply is in [`Sample`].
    volume: AtomicF64,
    speed: AtomicF64,
    /// Sources appended and not yet finished. Bumped by `append` before the callback can see the
    /// source, so a just-fed voice never reads as empty.
    sources: AtomicUsize,
    /// Frames the current source has been pulled for, on its own timeline, and that source's rate.
    /// The pair is the clock: media position is one divided by the other.
    ///
    /// **`rate` is the published half and carries the pair's ordering.** Both are written when a
    /// source takes over, and a reader that saw the new rate against the old source's frame count
    /// would report the previous track's end as this one's start. `rate` is stored last under
    /// `Release` and loaded first under `Acquire`, so seeing it means seeing the zeroed count
    /// behind it; the reverse pairing only ever mis-scales a count near zero.
    frames: AtomicU64,
    rate: AtomicU32,
    /// Commands sent, and commands the callback has drained. A control op that must land before it
    /// returns waits for the second to reach the first.
    issued: AtomicU64,
    serviced: AtomicU64,
    /// A ticket per source that has taken over as the playing one, so a control op prepared
    /// against what was mounted can be refused if something else has since taken the deck.
    ///
    /// The seek is why it exists: it reads the file on its own thread, and the source under it can
    /// be replaced meanwhile by a *gapless* handover, which happens in this callback and so is
    /// under neither `exec_lock` nor `deck_epoch`. Written by the callback alone.
    mounted: AtomicU64,
}

/// The control side of one voice.
pub struct Voice {
    shared: Arc<VoiceShared>,
    commands: SyncSender<Command>,
    /// Spent sources the callback handed back, freed by whoever calls [`Self::collect_spent`].
    /// Behind a lock because a `Receiver` is not `Sync` and this side is shared; nothing contends
    /// it, the collector being one task.
    spent: Mutex<Receiver<Loaded>>,
    device: Shape,
}

impl Voice {
    /// Queue `source` behind whatever is already on this voice.
    pub fn append<S: AudioSource + 'static>(&self, source: S) {
        let loaded = self.load(source);
        self.send_counted(Command::Append(loaded));
    }

    /// Put `source` on this voice in place of the one `mounted` names, with the clock anchored at
    /// `position` of it.
    ///
    /// How a seek is spelled: `source` is already positioned, so the callback does a pointer swap
    /// rather than a demuxer scan, and nothing here waits on it. The ticket is what makes that
    /// safe over the gap — see [`VoiceShared::mounted`] — and a voice that has moved on, or run
    /// dry, takes no source from this.
    pub fn replace<S: AudioSource + 'static>(&self, source: S, position: Duration, mounted: u64) {
        let frames = frames_at(position, source.sample_rate());
        let loaded = self.load(source);
        self.send_counted(Command::Replace {
            loaded,
            frames,
            mounted,
        });
    }

    /// The ticket of the source last mounted here, for a later [`Self::replace`] to be matched
    /// against. Read it against the same observation of the deck the seek is decided on.
    pub fn mounted(&self) -> u64 {
        self.shared.mounted.load(Ordering::Acquire)
    }

    /// Pair `source` with the converter that brings it to this device.
    fn load<S: AudioSource + 'static>(&self, source: S) -> Loaded {
        Loaded {
            converter: Converter::new(source.shape(), self.device),
            source: Box::new(source),
        }
    }

    /// Send a command carrying a source, counting it before the callback can see it.
    ///
    /// The bump leads the send so the count is never behind what the callback can already act on.
    /// Both carriers pay it back the same way: `Append` when its source finishes, `Replace` when
    /// the callback drops whichever source the swap leaves over.
    fn send_counted(&self, command: Command) {
        self.shared.sources.fetch_add(1, Ordering::SeqCst);
        if !self.send(command) {
            self.shared.sources.fetch_sub(1, Ordering::SeqCst);
        }
    }

    /// Drop everything on this voice, pause it, and rewind its clock.
    ///
    /// The clock is zeroed on this side too, not only by the callback: a voice whose source drained
    /// on its own is empty *and* still reporting that source's final position, so a clear that only
    /// asked the callback would leave `Decks::{cut_to,crossfade_to}` starting a track whose
    /// position reads as the previous one's end until the append is picked up.
    ///
    /// Zeroed **last**, where the voice is quiet either way. Ahead of the count it races the
    /// callback's own final `fetch_add`, which lands before the `sources` decrement that makes an
    /// empty voice look empty; ahead of the send it can rewind a source still playing.
    pub fn clear(&self) {
        self.pause();
        if self.shared.sources.load(Ordering::SeqCst) != 0 && self.send(Command::Clear) {
            self.await_service();
        }
        self.shared.frames.store(0, Ordering::Relaxed);
    }

    /// Free whatever the callback has finished with, here rather than there.
    ///
    /// Cheap and idempotent, so it belongs on a timer the app already runs. Skipping it costs
    /// nothing but a few spent sources held until the next call, and once the queue fills the
    /// callback goes back to freeing its own.
    pub fn collect_spent(&self) {
        let spent = self.spent.lock();
        while spent.try_recv().is_ok() {}
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
    ///
    /// `SeqCst` to match every write: this is what the transport reads to decide a voice has run
    /// dry, and `Decks::busy` pairs it with `paused`, which is already `SeqCst`.
    pub fn len(&self) -> usize {
        self.shared.sources.load(Ordering::SeqCst)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn set_volume(&self, volume: f64) {
        self.shared.volume.store(volume);
    }

    pub fn set_speed(&self, speed: f64) {
        self.shared.speed.store(speed);
    }

    /// Where the playing source has been pulled to, on its own timeline.
    ///
    /// Media time directly: the frames counted are the source's, and playback speed is applied
    /// below this by the converter rather than by inflating the rate reported upward.
    pub fn position(&self) -> Duration {
        let rate = u64::from(self.shared.rate.load(Ordering::Acquire));
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
        // anyway; blocking here would do it while holding the transport instead. Dropped rather
        // than returned because there is no answer worth giving: the only thing that fills this
        // queue is a device that has stopped asking for samples, and a lost `Append` leaves the
        // voice empty, which the monitor reads as end-of-stream and advances on regardless. What
        // tells the user is `tasks::audio_health`, off the fault counters, not this line.
        if self.commands.try_send(command).is_err() {
            log::warn!(
                "audio callback is not draining its control channel; a transport op was lost"
            );
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
pub struct VoicePull {
    shared: Arc<VoiceShared>,
    commands: Receiver<Command>,
    current: Option<Loaded>,
    staged: VecDeque<Loaded>,
    /// Where a spent source goes instead of being freed here. See [`Loaded`].
    spent: SyncSender<Loaded>,
    device: Shape,
}

impl VoicePull {
    /// Write this voice's output into `block`, already sized to whole device frames, and say how
    /// far it reached.
    ///
    /// Volume is applied here rather than by the mixer so that **unity is a short circuit**: a voice
    /// at full volume hands its samples over untouched, which is what makes a lone voice through the
    /// whole chain bit-identical to what the decoder produced.
    ///
    /// Servicing the control channel comes first and happens even while paused, or a voice could not
    /// be loaded or cleared without being played.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "volume is an f64 the transport caps at unity, and even the boost band a voice would otherwise accept narrows to f32 with nothing audible lost"
    )]
    pub fn render(&mut self, block: &mut [Sample]) -> usize {
        // Not decoration: a block ending mid-frame leaves `Converter::fill` no whole chunk to
        // write, so it returns nothing without ending the source and the loop below never advances
        // — on the one thread that must not stall. The invariant is the mixer's, three frames up.
        debug_assert!(
            block.len().is_multiple_of(usize::from(self.device.channels.get())),
            "a voice is only ever handed whole device frames"
        );

        self.service();
        if self.shared.paused.load(Ordering::SeqCst) {
            return 0;
        }
        let speed = self.shared.speed.load();
        // Read once, so the whole block renders against one observation, and compared as a bit
        // pattern because the short circuit above has to be exact: an epsilon would take it a shade
        // off unity.
        let volume = self.shared.volume.load();
        let at_unity = volume.to_bits() == 1.0_f64.to_bits();

        let mut written = 0;
        while written < block.len() {
            let Some(loaded) = self.current.as_mut() else {
                break;
            };
            let Filled {
                samples,
                source_frames,
            } = loaded.converter.fill(&mut block[written..], &mut *loaded.source, speed);
            written += samples;
            self.shared.frames.fetch_add(source_frames, Ordering::Relaxed);
            if loaded.converter.is_done() {
                self.finish_current();
            }
        }

        if !at_unity {
            let volume = volume as Sample;
            for slot in &mut block[..written] {
                *slot *= volume;
            }
        }
        written
    }

    /// Drain the control side's commands.
    ///
    /// First thing in every render, which the mixer runs once per lockstep step rather than once
    /// per callback — so an op lands at the head of the next *step*, part way through a block. That
    /// is the cheap direction: a waiting control thread is released a step early instead of a block
    /// late, and one step of skew between the voices is what `LOCKSTEP_FRAMES` already bounds.
    fn service(&mut self) {
        let seen = self.shared.issued.load(Ordering::Acquire);
        // Nothing issued since the last drain, which is every step of every block but the ones
        // carrying a transport op. Answering that from the counter keeps the channel, and the
        // fence its `try_recv` issues even when empty, off the path the mixer runs dozens of
        // times per callback. Only this thread writes `serviced`, so the load needs no ordering.
        if seen == self.shared.serviced.load(Ordering::Relaxed) {
            return;
        }
        loop {
            match self.commands.try_recv() {
                Ok(Command::Append(loaded)) => self.accept(loaded),
                Ok(Command::Clear) => self.clear(),
                Ok(Command::Replace {
                    loaded,
                    frames,
                    mounted,
                }) => self.replace(loaded, frames, mounted),
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
        self.shared.serviced.store(seen, Ordering::Release);
    }

    fn accept(&mut self, loaded: Loaded) {
        if self.current.is_none() {
            self.start(loaded);
        } else {
            self.staged.push_back(loaded);
        }
    }

    /// Make `loaded` the playing source, with the clock anchored `frames` into it.
    ///
    /// Count first, rate last — the pair's ordering is argued on [`VoiceShared::frames`]. The
    /// ticket is its own `Release`, the control side comparing it and nothing else.
    fn start_at(&mut self, loaded: Loaded, frames: u64) {
        self.shared.frames.store(frames, Ordering::Relaxed);
        self.shared.rate.store(loaded.source.sample_rate().get(), Ordering::Release);
        self.shared.mounted.fetch_add(1, Ordering::Release);
        self.current = Some(loaded);
    }

    /// [`Self::start_at`] from the top, which is every arrival but a seek's.
    fn start(&mut self, loaded: Loaded) {
        self.start_at(loaded, 0);
    }

    /// Swap the source `mounted` names for `loaded`, which the control thread has already
    /// positioned.
    ///
    /// **A deck that moved on takes nothing.** It drained, or a staged gapless successor took it
    /// over from under the caller — and a swap there would restart a track that is either already
    /// over or not the one the caller was looking at. The ticket catches both, where the
    /// in-place seek this replaced silently moved whatever it found. Either way one source is
    /// dropped, which is the count `Voice::send_counted` added for.
    fn replace(&mut self, loaded: Loaded, frames: u64, mounted: u64) {
        let moved_on = self.shared.mounted.load(Ordering::Relaxed) != mounted;
        let taken = if moved_on { None } else { self.current.take() };
        // Whichever source the swap leaves over: the one it displaces, or `loaded` itself where
        // there is nothing left to displace it against.
        let spent = match taken {
            Some(displaced) => {
                self.start_at(loaded, frames);
                displaced
            }
            None => loaded,
        };
        self.retire(spent);
        self.shared.sources.fetch_sub(1, Ordering::SeqCst);
    }

    /// The playing source ran out: retire it and take the staged one, if any.
    ///
    /// Retiring rather than dropping is what keeps the free off this thread while the visualizer's
    /// claim is still given up on it — see [`Loaded`].
    fn finish_current(&mut self) {
        if let Some(spent) = self.current.take() {
            self.retire(spent);
            self.shared.sources.fetch_sub(1, Ordering::SeqCst);
        }
        if let Some(next) = self.staged.pop_front() {
            self.start(next);
        }
    }

    fn clear(&mut self) {
        let dropped = usize::from(self.current.is_some()) + self.staged.len();
        if let Some(spent) = self.current.take() {
            self.retire(spent);
        }
        while let Some(spent) = self.staged.pop_front() {
            self.retire(spent);
        }
        self.shared.sources.fetch_sub(dropped, Ordering::SeqCst);
        self.shared.frames.store(0, Ordering::Relaxed);
    }

    /// Give up `spent`'s claims here and hand the rest back to be freed elsewhere.
    ///
    /// The send is the whole point and the fallback is the status quo: a full queue frees it right
    /// here, which is what every retirement used to do. See [`Loaded`].
    fn retire(&mut self, mut spent: Loaded) {
        spent.source.release_claims();
        drop(self.spent.try_send(spent));
    }
}

/// Frames of a source running at `rate` that `position` is worth.
///
/// Integer throughout: this count is what [`Voice::position`] divides back down, so a float round
/// trip would read back a millisecond off the target the caller asked for.
fn frames_at(position: Duration, rate: SampleRate) -> u64 {
    let frames = position.as_micros().saturating_mul(u128::from(rate.get())) / 1_000_000;
    u64::try_from(frames).unwrap_or(u64::MAX)
}

/// Build one voice's two halves.
pub fn pair(device: Shape) -> (Voice, VoicePull) {
    let (command_tx, command_rx) = sync_channel(COMMAND_SLOTS);
    let (spent_tx, spent_rx) = sync_channel(SPENT_SLOTS);

    let shared = Arc::new(VoiceShared {
        paused: AtomicBool::new(false),
        volume: AtomicF64::new(1.0),
        speed: AtomicF64::new(1.0),
        sources: AtomicUsize::new(0),
        frames: AtomicU64::new(0),
        rate: AtomicU32::new(0),
        issued: AtomicU64::new(0),
        serviced: AtomicU64::new(0),
        mounted: AtomicU64::new(0),
    });

    let voice = Voice {
        shared: shared.clone(),
        commands: command_tx,
        spent: Mutex::new(spent_rx),
        device,
    };
    let pull = VoicePull {
        shared,
        commands: command_rx,
        current: None,
        staged: VecDeque::with_capacity(COMMAND_SLOTS),
        spent: spent_tx,
        device,
    };
    (voice, pull)
}

#[cfg(test)]
#[path = "tests/voice_tests.rs"]
mod tests;
