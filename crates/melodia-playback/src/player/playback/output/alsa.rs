//! Exclusive output on Linux: an ALSA `hw:` device, opened at the source's own shape.
//!
//! `hw:` is the card with nothing in between, so every refusal is final: a rate, channel count or
//! format the card lacks fails the claim rather than being converted on the way, which is the
//! whole difference from the shared path. What reaches the card is [`encode`]'s output of the
//! mixer's block, and nothing else touches it.
//!
//! **Two things users will report, and neither is a bug.** While the claim holds, Melodia's
//! stream is gone from the sound server, so it is missing from its mixer and its volume, and
//! every other application on that card falls silent. And `PIPEWIRE_ALSA`, which `main.rs` sets
//! for the shared stream, has no effect here: it configures `PipeWire`'s ALSA plugin, and `hw:`
//! never goes through the plugin.
//!
//! [`encode`]: super::encode

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use alsa::ctl::{Ctl, DeviceIter};
use alsa::pcm::{Access, Format, HwParams, IO, PCM};
use alsa::{Direction, ValueOr};
use parking_lot::{Condvar, Mutex};

use melodia_audio::player::source::audio::{ChannelCount, Sample, Shape, SourceFormat};
use melodia_core::error::describe;

use super::super::stream_health::AudioStreamHealth;
use super::claim::ClaimError;
use super::device::Feed;
use super::encode::{self, DeviceFormat};
use super::{Negotiated, OutputDevice, OutputFormat, realtime, reserve};

/// How much the writer hands the card at a time. Short enough that a stop lands quickly, long
/// enough that an RT thread wakes rarely.
const PERIOD: Duration = Duration::from_millis(20);

/// Periods the card's buffer holds, which is how long the writer may be late before an underrun.
const PERIODS_PER_BUFFER: u32 = 4;

/// Failed writes in a row the writer recovers from before calling the device lost.
const RECOVER_ATTEMPTS: u32 = 3;

/// How long a granted release waits for the writer to close the card before answering anyway.
const RELEASE_WAIT: Duration = Duration::from_secs(1);

const THREAD_NAME: &str = "alsa-out";

/// How long an open waits for the sound server to finish closing a card it has just let go.
/// Past it the card is someone else's, and the claim falls back as busy.
const BUSY_WAIT: Duration = Duration::from_secs(1);

const BUSY_RETRY: Duration = Duration::from_millis(50);

/// A card's playback device, as the picker lists it and as an open resolves it.
struct Card {
    device: OutputDevice,
    index: i32,
}

/// Every playback device on every card, by the stable `hw:CARD=<id>,DEV=<n>` spelling.
///
/// Built from the cards themselves rather than ALSA's name hints, which list plugin aliases and
/// numbered names that move when a card is plugged in.
pub(super) fn devices() -> Vec<OutputDevice> {
    cards().into_iter().map(|card| card.device).collect()
}

fn cards() -> Vec<Card> {
    let mut found = Vec::new();
    for card in alsa::card::Iter::new() {
        let card = match card {
            Ok(card) => card,
            Err(e) => {
                log::debug!("audio: skipping an unreadable sound card: {}", describe(&e));
                continue;
            }
        };
        let ctl = match Ctl::from_card(&card, false) {
            Ok(ctl) => ctl,
            Err(e) => {
                log::debug!("audio: skipping card {}: {}", card.get_index(), describe(&e));
                continue;
            }
        };
        let Ok(info) = ctl.card_info() else { continue };
        let (Ok(card_id), Ok(card_name)) = (info.get_id(), info.get_name()) else { continue };
        for device in DeviceIter::new(&ctl) {
            let Ok(number) = u32::try_from(device) else { continue };
            // A capture-only device answers with an error here, which is how it drops out.
            let Ok(pcm) = ctl.pcm_info(number, 0, Direction::Playback) else { continue };
            let name = match pcm.get_name() {
                Ok(pcm_name) if !pcm_name.is_empty() => format!("{card_name} · {pcm_name}"),
                _ => card_name.to_owned(),
            };
            found.push(Card {
                device: OutputDevice { id: format!("hw:CARD={card_id},DEV={number}"), name },
                index: card.get_index(),
            });
        }
    }
    found
}

/// A card's reservation, kept apart from the stream on it so a reopen on the same card can carry
/// it over rather than hand the card back and take it again.
///
/// **The hand-back is what breaks the desktop, not the claim.** The session manager rebuilds a
/// card in the background once it has it back, and a claim landing mid-rebuild strands the card's
/// node name, so the card returns under a numbered one: every rename and the saved default output
/// are keyed on the old name, and both are lost until the session manager restarts. A reopen is
/// exactly that pattern, milliseconds apart.
pub(super) struct Claim {
    card: i32,
    /// The writer a granted release stops, which is whichever stream holds the claim now.
    writer: Arc<Mutex<Arc<WriterControl>>>,
    _reservation: Option<reserve::Reservation>,
}

/// The live stream. Dropping it stops the writer, closes the card, then gives the claim back.
pub(super) struct AlsaStream {
    writer: Option<JoinHandle<()>>,
    control: Arc<WriterControl>,
    negotiated: Negotiated,
    /// Dropped after the writer is joined, so the name is never free while the card is open.
    claim: Option<Claim>,
}

impl AlsaStream {
    pub(super) fn negotiated(&self) -> Negotiated {
        self.negotiated.clone()
    }

    /// Close the card and keep its reservation, for a reopen that may land on the same card.
    pub(super) fn into_claim(mut self) -> Option<Claim> {
        let claim = self.claim.take();
        drop(self);
        claim
    }
}

impl Drop for AlsaStream {
    fn drop(&mut self) {
        self.control.stop();
        if let Some(writer) = self.writer.take()
            && writer.join().is_err()
        {
            log::warn!("audio: the exclusive output thread panicked");
        }
    }
}

/// Claim `device`, or the first card listed where none is named, at exactly `shape` and a format
/// that holds `source`, and start feeding it from `feed`. `held` is the last stream's claim, reused
/// when it is on the same card and given back otherwise.
///
/// # Errors
///
/// A [`ClaimError`] naming why the card could not be had, which the caller falls back on.
pub(super) fn open(
    device: Option<&str>,
    shape: Shape,
    source: SourceFormat,
    feed: &Feed,
    held: Option<Claim>,
) -> Result<AlsaStream, ClaimError> {
    let card = resolve(device)?;
    let control = Arc::new(WriterControl::default());
    let claim = match held {
        Some(claim) if claim.card == card.index => {
            *claim.writer.lock() = Arc::clone(&control);
            claim
        }
        other => {
            // Another card's name goes back before this one is asked for.
            drop(other);
            reserve_card(&card, &control, &feed.health)?
        }
    };

    let pcm = open_pcm(&card.device.id)?;
    let config = configure(&pcm, shape, source)?;
    let device_shape = Shape { channels: config.channels, rate: shape.rate };
    feed.pull.lock().reshape(device_shape);

    let writer = Writer {
        pcm,
        feed: feed.clone(),
        control: Arc::clone(&control),
        format: config.format,
        block_samples: config.period_frames * usize::from(config.channels.get()),
    };
    let writer = std::thread::Builder::new()
        .name(THREAD_NAME.to_owned())
        .spawn(move || writer.run())
        .map_err(|e| ClaimError::io("Failed to start the exclusive output thread", e))?;

    Ok(AlsaStream {
        writer: Some(writer),
        control,
        negotiated: Negotiated {
            device_name: Some(card.device.name),
            shape: device_shape,
            format: OutputFormat::Exclusive(config.format),
            fallback: None,
            requested_period: Some(config.requested_period),
            period: u32::try_from(config.period_frames).ok(),
        },
        claim: Some(claim),
    })
}

fn reserve_card(
    card: &Card,
    control: &Arc<WriterControl>,
    health: &Arc<AudioStreamHealth>,
) -> Result<Claim, ClaimError> {
    let writer = Arc::new(Mutex::new(Arc::clone(control)));
    let reservation = reserve::acquire(card.index, card.device.name.clone(), {
        let writer = Arc::clone(&writer);
        let health = Arc::clone(health);
        move || {
            // Cloned out so the slot isn't held through the wait.
            let control = Arc::clone(&writer.lock());
            let closed = control.stop_and_wait(RELEASE_WAIT);
            // Reported as a loss so the usual recovery reopens, meets the new holder's claim and
            // falls back to shared with its name.
            health.report_device_lost();
            closed
        }
    })?;
    Ok(Claim { card: card.index, writer, _reservation: reservation })
}

fn resolve(device: Option<&str>) -> Result<Card, ClaimError> {
    let mut cards = cards().into_iter();
    let found = match device {
        Some(id) => cards.find(|card| card.device.id == id),
        None => cards.next(),
    };
    found.ok_or_else(|| ClaimError::NotConnected { id: device.unwrap_or_default().to_owned() })
}

/// Open the card, waiting out a busy one for up to [`BUSY_WAIT`].
///
/// The sound server answers a release once it has let the card go, but closes it a moment later,
/// so the first open after taking the card over can meet it still open.
fn open_pcm(id: &str) -> Result<PCM, ClaimError> {
    let deadline = Instant::now() + BUSY_WAIT;
    loop {
        match PCM::new(id, Direction::Playback, false) {
            Ok(pcm) => return Ok(pcm),
            Err(e) if is_busy(&e) && Instant::now() < deadline => std::thread::sleep(BUSY_RETRY),
            Err(e) => return Err(open_error(id, e)),
        }
    }
}

fn is_busy(e: &alsa::Error) -> bool {
    rustix::io::Errno::from_raw_os_error(e.errno()) == rustix::io::Errno::BUSY
}

fn open_error(id: &str, e: alsa::Error) -> ClaimError {
    match rustix::io::Errno::from_raw_os_error(e.errno()) {
        rustix::io::Errno::BUSY => ClaimError::Busy(e.into()),
        rustix::io::Errno::NOENT | rustix::io::Errno::NODEV => {
            ClaimError::NotConnected { id: id.to_owned() }
        }
        _ => ClaimError::io("Failed to open the sound card", e),
    }
}

/// What the card agreed to.
struct Config {
    channels: ChannelCount,
    format: DeviceFormat,
    requested_period: u32,
    period_frames: usize,
}

fn configure(pcm: &PCM, shape: Shape, source: SourceFormat) -> Result<Config, ClaimError> {
    let rate = shape.rate.get();
    let hw =
        HwParams::any(pcm).map_err(|e| ClaimError::io("Failed to read the card's limits", e))?;
    hw.set_access(Access::RWInterleaved)
        .map_err(|e| ClaimError::io("The card refused interleaved access", e))?;
    hw.set_rate_resample(false).map_err(|e| ClaimError::io("Failed to turn resampling off", e))?;
    hw.set_rate(rate, ValueOr::Nearest).map_err(|_| ClaimError::RateRefused { rate })?;
    let channels = set_channels(&hw, shape.channels)?;
    let format = set_format(&hw, source)?;

    let requested_period = period_frames(rate);
    let period = hw
        .set_period_size_near(alsa::pcm::Frames::from(requested_period), ValueOr::Nearest)
        .map_err(|e| ClaimError::io("The card refused the period size", e))?;
    hw.set_buffer_size_near(period * alsa::pcm::Frames::from(PERIODS_PER_BUFFER))
        .map_err(|e| ClaimError::io("The card refused the buffer size", e))?;
    pcm.hw_params(&hw).map_err(|e| ClaimError::io("The card refused its configuration", e))?;

    let current = pcm
        .hw_params_current()
        .map_err(|e| ClaimError::io("Failed to read the card's configuration back", e))?;
    // Read back because a driver may round an exact request and still report success.
    if current.get_rate().ok() != Some(rate) {
        return Err(ClaimError::RateRefused { rate });
    }
    let period = current
        .get_period_size()
        .map_err(|e| ClaimError::io("Failed to read the card's period", e))?;
    let buffer = current
        .get_buffer_size()
        .map_err(|e| ClaimError::io("Failed to read the card's buffer", e))?;

    // Start once the buffer is full, so the first period plays behind a full cushion.
    let sw = pcm
        .sw_params_current()
        .map_err(|e| ClaimError::io("Failed to read the card's start settings", e))?;
    sw.set_start_threshold(buffer)
        .and_then(|()| sw.set_avail_min(period))
        .and_then(|()| pcm.sw_params(&sw))
        .map_err(|e| ClaimError::io("The card refused its start settings", e))?;

    let period_frames = usize::try_from(period)
        .map_err(|e| ClaimError::io("The card reported a negative period", e))?;
    Ok(Config { channels, format, requested_period, period_frames })
}

/// The source's own channel count, else the narrowest wider one the card offers. The mixer lays
/// a narrower source on the first channels untouched, so a wider device costs nothing.
fn set_channels(hw: &HwParams<'_>, source: ChannelCount) -> Result<ChannelCount, ClaimError> {
    let refused = || ClaimError::ChannelsRefused { channels: source.get() };
    let widest = hw.get_channels_max().unwrap_or(0);
    let channels = (u32::from(source.get())..=widest)
        .find(|&channels| hw.test_channels(channels).is_ok())
        .ok_or_else(refused)?;
    hw.set_channels(channels).map_err(|_| refused())?;
    u16::try_from(channels).ok().and_then(ChannelCount::new).ok_or_else(refused)
}

fn set_format(hw: &HwParams<'_>, source: SourceFormat) -> Result<DeviceFormat, ClaimError> {
    let format = DeviceFormat::ladder(source)
        .iter()
        .copied()
        .find(|&format| hw.test_format(alsa_format(format)).is_ok())
        .ok_or(ClaimError::FormatRefused { bits: source.bits })?;
    hw.set_format(alsa_format(format))
        .map_err(|_| ClaimError::FormatRefused { bits: source.bits })?;
    Ok(format)
}

fn alsa_format(format: DeviceFormat) -> Format {
    match format {
        DeviceFormat::S16 => Format::S16LE,
        DeviceFormat::S24Packed => Format::S243LE,
        DeviceFormat::S24Low => Format::S24LE,
        DeviceFormat::S32 => Format::S32LE,
        DeviceFormat::F32 => Format::FloatLE,
    }
}

/// [`PERIOD`] in frames at `rate`.
fn period_frames(rate: u32) -> u32 {
    let frames = u128::from(rate) * PERIOD.as_millis() / 1_000;
    u32::try_from(frames).unwrap_or(u32::MAX)
}

/// The stop flag the writer polls, and the latch it sets once the card is closed.
#[derive(Default)]
struct WriterControl {
    stop: AtomicBool,
    closed: Mutex<bool>,
    closed_changed: Condvar,
}

impl WriterControl {
    fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    fn stopping(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    fn mark_closed(&self) {
        *self.closed.lock() = true;
        self.closed_changed.notify_all();
    }

    /// Stop the writer and wait up to `timeout` for the card to close. `true` once it has.
    fn stop_and_wait(&self, timeout: Duration) -> bool {
        self.stop();
        let mut closed = self.closed.lock();
        self.closed_changed.wait_while_for(&mut closed, |closed| !*closed, timeout);
        *closed
    }
}

/// Everything the `alsa-out` thread owns.
struct Writer {
    pcm: PCM,
    feed: Feed,
    control: Arc<WriterControl>,
    format: DeviceFormat,
    block_samples: usize,
}

impl Writer {
    fn run(self) {
        realtime::promote_current_thread();
        let Self { pcm, feed, control, format, block_samples } = self;
        if let Err(e) = write_until_stopped(&pcm, &feed, &control, format, block_samples) {
            log::warn!("audio: the exclusive output stopped: {}", describe(&e));
            feed.health.report_device_lost();
        }
        drop(pcm);
        control.mark_closed();
    }
}

/// Pull, encode and write one period at a time until told to stop.
///
/// Nothing here logs: the health counters are how this loop reports, and the one error it returns
/// is logged once, on the way out.
fn write_until_stopped(
    pcm: &PCM,
    feed: &Feed,
    control: &WriterControl,
    format: DeviceFormat,
    block_samples: usize,
) -> Result<(), alsa::Error> {
    let io = pcm.io_bytes();
    let frame_bytes =
        usize::try_from(pcm.frames_to_bytes(1)).unwrap_or(format.bytes_per_sample()).max(1);
    let mut block: Vec<Sample> = vec![0.0; block_samples];
    let mut bytes = Vec::with_capacity(block_samples * format.bytes_per_sample());
    while !control.stopping() {
        feed.fill(&mut block);
        encode::encode(&block, format, &mut bytes);
        write_all(pcm, &io, &bytes, frame_bytes, feed)?;
    }
    Ok(())
}

/// Write the whole of `bytes`, recovering an underrun rather than giving up on it.
fn write_all(
    pcm: &PCM,
    io: &IO<'_, u8>,
    mut bytes: &[u8],
    frame_bytes: usize,
    feed: &Feed,
) -> Result<(), alsa::Error> {
    let mut failures = 0;
    while !bytes.is_empty() {
        match io.writei(bytes) {
            Ok(frames) => {
                bytes = bytes.get(frames * frame_bytes..).unwrap_or_default();
                failures = 0;
            }
            Err(e) => {
                failures += 1;
                if failures > RECOVER_ATTEMPTS {
                    return Err(e);
                }
                pcm.try_recover(e, true)?;
                feed.health.record_xrun();
            }
        }
    }
    Ok(())
}
