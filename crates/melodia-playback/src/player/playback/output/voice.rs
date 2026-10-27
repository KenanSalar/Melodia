//! One voice: the source playing, the one staged behind, and the transport state the mixer reads.
//! [`Voice`] is the control side the rest of the tree sees; [`VoicePull`] is the mixer's, touched
//! only from the audio callback, with a command channel and counters between them.
//!
//! `clear` blocks until the callback has serviced it, the contract `Decks::cut_to` rests on: a
//! clear not yet landed would take the new source with it. It is the only op that blocks; a seek is
//! a `replace` carrying a source already positioned.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{
    AtomicBool, AtomicU8, AtomicU16, AtomicU32, AtomicU64, AtomicUsize, Ordering,
};
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, sync_channel};
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use super::super::dsp::AtomicF64;
use super::convert::{Conversion, Converter, Filled, Resampler};
use super::resample;
use melodia_audio::player::source::audio::{
    AudioSource, ChannelCount, Sample, SampleRate, Shape, SourceFormat, frames_in,
    frames_to_duration,
};

/// What a voice is playing, as the signal path reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlayingSource {
    pub shape: Shape,
    pub format: SourceFormat,
    /// Whether anything between the decoder and the deck changed the samples.
    pub dsp_engaged: bool,
}

/// Long enough for any device period, short enough that a stream no longer serviced (a device
/// pulled mid-track) stalls the transport for an eyeblink rather than wedging the UI thread.
const SERVICE_TIMEOUT: Duration = Duration::from_millis(500);

/// Below one device period, so the wait ends on the callback that services it.
const SERVICE_POLL: Duration = Duration::from_millis(1);

/// Bounded so the queue is allocated once; reaching it means the callback has stopped draining.
/// [`VoicePull::staged`] is sized against it too, a `clear` too full to send leaving appends to
/// land behind sources that should have gone, and an allocation being what that thread must not do.
const COMMAND_SLOTS: usize = 8;

/// A clear retires two sources per voice; the rest is slack for a collector that hasn't run. Full
/// falls back to freeing on the callback, a worse tick rather than a broken one.
const SPENT_SLOTS: usize = 8;

/// A source and its converter, built on the control thread because [`Converter::new`] allocates.
/// Freed off the audio thread through [`VoicePull::retire`], a decoder's read buffer being too big
/// for glibc's per-thread cache; only its claims ([`AudioSource::release_claims`]) are given up on
/// the callback, so the visualizer stops mixing a dead deck at once.
struct Loaded {
    source: Box<dyn AudioSource>,
    converter: Converter,
}

enum Command {
    /// `frames` anchors the clock for a source mounting at once (a resume's, already positioned);
    /// one landing behind a playing source starts from its own top.
    Append {
        loaded: Loaded,
        frames: u64,
    },
    /// Every mount it makes is a handover, onto a voice already dry included
    /// ([`VoiceShared::handovers`]).
    Stage {
        loaded: Loaded,
    },
    Clear,
    /// A seek: the control thread positions the source, leaving this side a pointer swap rather
    /// than a demuxer scan. `handovers` refuses it once a successor has taken the deck.
    Replace {
        loaded: Loaded,
        frames: u64,
        handovers: u64,
    },
}

/// What both sides read without going through the channel.
struct VoiceShared {
    paused: AtomicBool,
    /// `f64` because [`Converter::fill`] takes `speed` at that width; volume matches and narrows
    /// at its one use.
    volume: AtomicF64,
    speed: AtomicF64,
    /// Stored `Release` after the kernel is prepared, so a callback seeing it finds the table built.
    band_limited: AtomicBool,
    /// Bumped before the callback can see a source, so a just-fed voice never reads as empty.
    sources: AtomicUsize,
    /// The clock, media position being `frames / rate`. `rate` is stored last under `Release` and
    /// loaded first under `Acquire`, so a reader seeing a new rate sees the zeroed count behind it
    /// rather than reporting the last track's end as this one's start.
    frames: AtomicU64,
    rate: AtomicU32,
    /// Where the clock last started (a mount, seek or resume), so [`Voice::heard`] never reads
    /// earlier. Written with `frames`, ahead of `rate`.
    anchor: AtomicU64,
    /// What the converter pulled past the frame it writes, which the ear trails by as well.
    converter_ahead: AtomicU64,
    /// The rest of [`Voice::playing`], written with the clock at takeover; a read straddling a
    /// handover can pair two tracks' answers, which the next read puts right.
    source_channels: AtomicU16,
    source_bits: AtomicU8,
    source_float: AtomicBool,
    source_lossy: AtomicBool,
    dsp_engaged: AtomicBool,
    /// An op that must land before returning waits for `serviced` to reach `issued`.
    issued: AtomicU64,
    serviced: AtomicU64,
    /// A ticket per staged successor that has taken the deck, so a seek prepared against the
    /// playing source is refused once one has. Only that mount counts: it happens in the callback,
    /// under neither `exec_lock` nor `deck_epoch`, where every other mount is sent under the
    /// `exec_lock` the seek holds, so the channel already orders it. Written by the callback alone.
    handovers: AtomicU64,
}

pub struct Voice {
    shared: Arc<VoiceShared>,
    commands: SyncSender<Command>,
    /// Behind a lock only because a `Receiver` isn't `Sync`; the one collector never contends it.
    spent: Mutex<Receiver<Loaded>>,
}

impl Voice {
    /// Queue `source` behind whatever is already on this voice, starting it from its top.
    pub fn append<S: AudioSource + 'static>(&self, source: S) {
        self.append_at(source, Duration::ZERO);
    }

    /// Queue `source` as the gapless successor of the playing one: a handover wherever it mounts,
    /// which [`Self::append`] is not, so a track start sent this way would refuse the seek behind it.
    pub fn stage<S: AudioSource + 'static>(&self, source: S) {
        let loaded = Self::load(source);
        self.send_counted(Command::Stage { loaded });
    }

    /// A track start, the clock anchored at `position`: a resume's source arrives already seeked,
    /// and the clock counts what it hands out. The anchor lands on the callback, so
    /// [`Self::is_settled`] stays false until it has.
    pub fn append_at<S: AudioSource + 'static>(&self, source: S, position: Duration) {
        let frames = frames_in(position, source.sample_rate());
        let loaded = Self::load(source);
        self.send_counted(Command::Append { loaded, frames });
    }

    /// How a seek is spelled: `source` is already positioned, so nothing waits on it. A voice that
    /// has handed over or run dry takes nothing ([`VoiceShared::handovers`]).
    pub fn replace<S: AudioSource + 'static>(&self, source: S, position: Duration, handovers: u64) {
        let frames = frames_in(position, source.sample_rate());
        let loaded = Self::load(source);
        self.send_counted(Command::Replace { loaded, frames, handovers });
    }

    /// Read against the same observation of the deck the seek is decided on.
    pub fn handovers(&self) -> u64 {
        self.shared.handovers.load(Ordering::Acquire)
    }

    fn load<S: AudioSource + 'static>(source: S) -> Loaded {
        Loaded { converter: Converter::new(source.shape()), source: Box::new(source) }
    }

    /// Counts the source before the callback can see it; `Append` pays it back when its source
    /// finishes, `Replace` when the swap drops one.
    fn send_counted(&self, command: Command) {
        self.shared.sources.fetch_add(1, Ordering::SeqCst);
        if !self.send(command) {
            self.shared.sources.fetch_sub(1, Ordering::SeqCst);
        }
    }

    /// The clock is zeroed here too, a voice that drained on its own still reporting its last
    /// source's end, and zeroed last: earlier it races the callback's final count or rewinds a
    /// source still playing.
    pub fn clear(&self) {
        self.pause();
        if self.shared.sources.load(Ordering::SeqCst) != 0 && self.send(Command::Clear) {
            self.await_service();
        }
        self.shared.frames.store(0, Ordering::Relaxed);
        self.shared.anchor.store(0, Ordering::Relaxed);
    }

    /// Cheap and idempotent; skipping it holds a few spent sources, and a full queue sends the
    /// callback back to freeing its own.
    pub fn collect_spent(&self) {
        let spent = self.spent.lock();
        while spent.try_recv().is_ok() {}
    }

    /// A resume re-anchors where the clock stopped, the device having played out everything
    /// through the pause.
    pub fn play(&self) {
        if self.is_paused() {
            let paused_at = self.shared.frames.load(Ordering::Relaxed);
            self.shared.anchor.store(paused_at, Ordering::Relaxed);
        }
        self.shared.paused.store(false, Ordering::SeqCst);
    }

    pub fn pause(&self) {
        self.shared.paused.store(true, Ordering::SeqCst);
    }

    pub fn is_paused(&self) -> bool {
        self.shared.paused.load(Ordering::SeqCst)
    }

    /// `SeqCst` like every write: the transport reads it as running dry, beside `paused`.
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

    /// From the next block on, mid-source included.
    pub fn set_resampler(&self, resampler: Resampler) {
        let band_limited = resampler == Resampler::BandLimited;
        if band_limited {
            resample::prepare();
        }
        self.shared.band_limited.store(band_limited, Ordering::Release);
    }

    /// Media time: speed is applied below this, by the converter.
    pub fn position(&self) -> Duration {
        let Some(rate) = SampleRate::new(self.shared.rate.load(Ordering::Acquire)) else {
            return Duration::ZERO;
        };
        frames_to_duration(self.shared.frames.load(Ordering::Relaxed), rate)
    }

    /// Whether the callback has drained every command sent so far. Until it has, the clock can
    /// still read where the voice was: a seek's swap re-anchors it only once serviced.
    pub fn is_settled(&self) -> bool {
        let issued = self.shared.issued.load(Ordering::Acquire);
        self.shared.serviced.load(Ordering::Acquire) >= issued
    }

    /// [`Self::position`] less the device's `lead`, scaled into media time, and what the converter
    /// pulled ahead; never earlier than the anchor, so a start, seek or resume doesn't read before
    /// where it began while the device plays out what came before.
    pub fn heard(&self, lead: Duration) -> Duration {
        let Some(rate) = SampleRate::new(self.shared.rate.load(Ordering::Acquire)) else {
            return Duration::ZERO;
        };
        let pulled = self.shared.frames.load(Ordering::Relaxed);
        let anchor = self.shared.anchor.load(Ordering::Relaxed).min(pulled);
        let media_lead = lead.as_secs_f64() * self.shared.speed.load();
        let behind = frames_in(Duration::try_from_secs_f64(media_lead).unwrap_or_default(), rate)
            .saturating_add(self.shared.converter_ahead.load(Ordering::Relaxed));
        frames_to_duration(pulled.saturating_sub(behind).max(anchor), rate)
    }

    /// `None` while nothing has mounted.
    pub fn playing(&self) -> Option<PlayingSource> {
        if self.is_empty() {
            return None;
        }
        let rate = SampleRate::new(self.shared.rate.load(Ordering::Acquire))?;
        let channels = ChannelCount::new(self.shared.source_channels.load(Ordering::Relaxed))?;
        let format = SourceFormat {
            bits: self.shared.source_bits.load(Ordering::Relaxed),
            float: self.shared.source_float.load(Ordering::Relaxed),
            lossy: self.shared.source_lossy.load(Ordering::Relaxed),
        };
        Some(PlayingSource {
            shape: Shape { channels, rate },
            format,
            dsp_engaged: self.shared.dsp_engaged.load(Ordering::Relaxed),
        })
    }

    /// The ticket is bumped after the send, so one the callback sees has its command on the
    /// channel.
    fn send(&self, command: Command) -> bool {
        // Full means the callback stopped draining. Dropped rather than blocked on while holding
        // the transport: a lost `Append` reads as end-of-stream and advances regardless, and
        // `tasks::audio_health` is what tells the user.
        if self.commands.try_send(command).is_err() {
            log::warn!(
                "audio callback is not draining its control channel; a transport op was lost"
            );
            return false;
        }
        self.shared.issued.fetch_add(1, Ordering::Release);
        true
    }

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
    /// Where a spent source goes instead of being freed here ([`Loaded`]).
    spent: SyncSender<Loaded>,
    device: Shape,
}

impl VoicePull {
    /// Writes into whole device frames and says how far it reached. Volume is applied here so unity
    /// is a short circuit, a lone voice staying bit-identical to the decoder. Commands are serviced
    /// first, even paused, or a voice couldn't be loaded or cleared without playing.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "volume is an f64 the transport caps at unity, and even the boost band a voice would otherwise accept narrows to f32 with nothing audible lost"
    )]
    pub fn render(&mut self, block: &mut [Sample]) -> usize {
        // A block ending mid-frame gives `Converter::fill` no whole chunk, and the loop below
        // never advances, on the one thread that must not stall.
        debug_assert!(
            block.len().is_multiple_of(usize::from(self.device.channels.get())),
            "a voice is only ever handed whole device frames"
        );

        self.service();
        if self.shared.paused.load(Ordering::SeqCst) {
            return 0;
        }
        let resampler = if self.shared.band_limited.load(Ordering::Acquire) {
            Resampler::BandLimited
        } else {
            Resampler::Linear
        };
        let conversion =
            Conversion { device: self.device, speed: self.shared.speed.load(), resampler };
        // One read per block, compared bitwise because the short circuit has to be exact.
        let volume = self.shared.volume.load();
        let at_unity = volume.to_bits() == 1.0_f64.to_bits();

        let mut written = 0;
        while written < block.len() {
            let Some(loaded) = self.current.as_mut() else {
                break;
            };
            let Filled { samples, source_frames } =
                loaded.converter.fill(&mut block[written..], &mut *loaded.source, conversion);
            written += samples;
            self.shared.frames.fetch_add(source_frames, Ordering::Relaxed);
            if loaded.converter.is_done() {
                self.finish_current();
            } else if loaded.converter.is_starved() {
                self.hand_over_or_drain();
            }
        }

        if !at_unity {
            let volume = volume as Sample;
            for slot in &mut block[..written] {
                *slot *= volume;
            }
        }
        if let Some(loaded) = &self.current {
            self.shared.dsp_engaged.store(loaded.source.dsp_engaged(), Ordering::Relaxed);
            self.shared.converter_ahead.store(loaded.converter.frames_ahead(), Ordering::Relaxed);
        }
        written
    }

    /// Only while no stream pulls, which `AudioOutput::reopen` guarantees by dropping it first.
    pub fn reshape(&mut self, device: Shape) {
        self.device = device;
    }

    /// Run per lockstep step rather than per callback, so an op lands part way through a block,
    /// releasing a waiter a step early; `LOCKSTEP_FRAMES` already bounds that skew.
    pub(super) fn service(&mut self) {
        let seen = self.shared.issued.load(Ordering::Acquire);
        // Most steps carry no op, and the counter keeps the channel's fence off them. Only this
        // thread writes `serviced`, so the load needs no ordering.
        if seen == self.shared.serviced.load(Ordering::Relaxed) {
            return;
        }
        loop {
            match self.commands.try_recv() {
                Ok(Command::Append { loaded, frames }) => self.accept(loaded, frames),
                Ok(Command::Stage { loaded }) => self.stage(loaded),
                Ok(Command::Clear) => self.clear(),
                Ok(Command::Replace { loaded, frames, handovers }) => {
                    self.replace(loaded, frames, handovers);
                }
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
        self.shared.serviced.store(seen, Ordering::Release);
    }

    /// A staged source drops the anchor on purpose: it mounts from
    /// [`Self::finish_current`] whenever the current one drains, which is its own top.
    fn accept(&mut self, loaded: Loaded, frames: u64) {
        if self.current.is_none() {
            self.start_at(loaded, frames);
        } else {
            self.staged.push_back(loaded);
        }
    }

    /// Mounts at once where the playing source has drained; the state is behind the deck either
    /// way.
    fn stage(&mut self, loaded: Loaded) {
        if self.current.is_some() {
            self.staged.push_back(loaded);
        } else {
            self.mount_successor(loaded);
        }
    }

    /// Count first, rate last ([`VoiceShared::frames`]).
    fn start_at(&mut self, loaded: Loaded, frames: u64) {
        let source = &loaded.source;
        let format = source.format();
        self.shared.source_channels.store(source.channels().get(), Ordering::Relaxed);
        self.shared.source_bits.store(format.bits, Ordering::Relaxed);
        self.shared.source_float.store(format.float, Ordering::Relaxed);
        self.shared.source_lossy.store(format.lossy, Ordering::Relaxed);
        self.shared.frames.store(frames, Ordering::Relaxed);
        self.shared.anchor.store(frames, Ordering::Relaxed);
        self.shared.rate.store(source.sample_rate().get(), Ordering::Release);
        self.current = Some(loaded);
    }

    /// From the top, counting the handover a seek's ticket is matched against.
    fn mount_successor(&mut self, next: Loaded) {
        self.start_at(next, 0);
        self.shared.handovers.fetch_add(1, Ordering::Release);
    }

    /// A deck that moved on takes nothing: an empty slot means it drained, a moved ticket that a
    /// successor took it, and a swap would restart the wrong track. One source is dropped either
    /// way, paying back `Voice::send_counted`.
    fn replace(&mut self, loaded: Loaded, frames: u64, handovers: u64) {
        let moved_on = self.shared.handovers.load(Ordering::Relaxed) != handovers;
        let taken = if moved_on { None } else { self.current.take() };
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

    /// A successor of the same shape takes the starved converter, so the seam converts as one
    /// stream; otherwise the window drains first.
    fn hand_over_or_drain(&mut self) {
        let Some(current) = self.current.as_mut() else {
            return;
        };
        let shape = current.source.shape();
        if let Some(next) = self.staged.front_mut().filter(|next| next.source.shape() == shape) {
            std::mem::swap(&mut current.converter, &mut next.converter);
        } else {
            current.converter.drain();
            if !current.converter.is_done() {
                return;
            }
        }
        self.finish_current();
    }

    /// Retires rather than drops ([`Loaded`]).
    fn finish_current(&mut self) {
        if let Some(spent) = self.current.take() {
            self.retire(spent);
            self.shared.sources.fetch_sub(1, Ordering::SeqCst);
        }
        if let Some(next) = self.staged.pop_front() {
            self.mount_successor(next);
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
        self.shared.anchor.store(0, Ordering::Relaxed);
    }

    /// Gives up the claims here and hands the free back; a full queue frees it here instead.
    fn retire(&mut self, mut spent: Loaded) {
        spent.source.release_claims();
        drop(self.spent.try_send(spent));
    }
}

/// Build one voice's two halves.
pub fn pair(device: Shape) -> (Voice, VoicePull) {
    let (command_tx, command_rx) = sync_channel(COMMAND_SLOTS);
    let (spent_tx, spent_rx) = sync_channel(SPENT_SLOTS);

    let shared = Arc::new(VoiceShared {
        paused: AtomicBool::new(false),
        volume: AtomicF64::new(1.0),
        speed: AtomicF64::new(1.0),
        band_limited: AtomicBool::new(false),
        sources: AtomicUsize::new(0),
        frames: AtomicU64::new(0),
        rate: AtomicU32::new(0),
        anchor: AtomicU64::new(0),
        converter_ahead: AtomicU64::new(0),
        source_channels: AtomicU16::new(0),
        source_bits: AtomicU8::new(0),
        source_float: AtomicBool::new(false),
        source_lossy: AtomicBool::new(false),
        dsp_engaged: AtomicBool::new(false),
        issued: AtomicU64::new(0),
        serviced: AtomicU64::new(0),
        handovers: AtomicU64::new(0),
    });

    let voice = Voice { shared: shared.clone(), commands: command_tx, spent: Mutex::new(spent_rx) };
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
