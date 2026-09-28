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

use melodia_audio::player::source::audio::{
    ChannelCount, Sample, SampleRate, Shape, SourceFormat, frames_in, frames_to_duration,
};
use melodia_core::error::describe;

use super::super::stream_health::AudioStreamHealth;
use super::alsa_volume::{self, CardVolume};
use super::claim::ClaimError;
use super::device::Feed;
use super::encode::{self, DeviceFormat};
use super::{ExclusiveRequest, Negotiated, OutputDevice, OutputFormat, realtime, reserve};

pub(super) const SUPPORTED: bool = true;

/// The writer always blocks on the card, so there is no event mode to poll instead of.
pub(super) const POLLING: bool = false;

/// A claim carries the volume on the card's simple mixer where it has an element to trust.
pub(super) const HARDWARE_VOLUME: bool = true;

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
pub(super) struct Card {
    pub(super) device: OutputDevice,
    pub(super) index: i32,
    /// The device's number on its card, the `DEV` in its id.
    pub(super) number: u32,
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
                number,
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
pub(super) struct ExclusiveStream {
    writer: Option<JoinHandle<()>>,
    control: Arc<WriterControl>,
    negotiated: Negotiated,
    /// Dropped after the writer is joined, so the name is never free while the card is open.
    claim: Option<Claim>,
}

impl ExclusiveStream {
    pub(super) fn negotiated(&self) -> Negotiated {
        self.negotiated.clone()
    }

    pub(super) fn hardware_volume(&self) -> bool {
        self.negotiated.hardware_volume
    }

    /// Close the card and keep its reservation, for a reopen that may land on the same card.
    pub(super) fn into_claim(mut self) -> Option<Claim> {
        let claim = self.claim.take();
        drop(self);
        claim
    }
}

impl Drop for ExclusiveStream {
    fn drop(&mut self) {
        self.control.stop();
        if let Some(writer) = self.writer.take()
            && writer.join().is_err()
        {
            log::warn!("audio: the exclusive output thread panicked");
        }
    }
}

/// Claim the requested card, or the first listed where none is named, at exactly its shape and a
/// format that holds its source, and start feeding it from `feed`. `held` is the last stream's
/// claim, reused when it is on the same card and given back otherwise.
///
/// # Errors
///
/// A [`ClaimError`] naming why the card could not be had, which the caller falls back on.
pub(super) fn open(
    request: &ExclusiveRequest,
    feed: &Feed,
    held: Option<Claim>,
) -> Result<ExclusiveStream, ClaimError> {
    let card = resolve(request.device.as_deref())?;
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
    let config = configure(&pcm, request)?;
    let device_shape = Shape { channels: config.channels, rate: request.shape.rate };
    feed.pull.lock().reshape(device_shape);
    // Before the writer starts, so the first period already plays at the level.
    let volume =
        if request.hardware_volume { hardware_volume(&card, feed.volume.load()) } else { None };
    let hardware_volume = volume.is_some();
    let device_level = alsa_volume::read(&card);
    let lowered = volume.as_ref().and_then(CardVolume::lowered);

    let writer = Writer {
        pcm,
        feed: feed.clone(),
        control: Arc::clone(&control),
        format: config.format,
        rate: device_shape.rate,
        block_samples: config.period_frames * usize::from(config.channels.get()),
        volume,
    };
    let writer = std::thread::Builder::new()
        .name(THREAD_NAME.to_owned())
        .spawn(move || writer.run())
        .map_err(|e| ClaimError::io("Failed to start the exclusive output thread", e))?;
    // After the start, so a claim that failed leaves the slider where the user had it.
    if let Some(level) = lowered {
        feed.external_volume.report(level);
    }

    Ok(ExclusiveStream {
        writer: Some(writer),
        control,
        negotiated: Negotiated {
            device_name: Some(card.device.name),
            shape: device_shape,
            format: OutputFormat::Exclusive(config.format),
            fallback: None,
            hardware_volume,
            device_level,
            requested_period: Some(config.requested_period),
            period: u32::try_from(config.period_frames).ok(),
        },
        claim: Some(claim),
    })
}

/// The card's volume element, set to `volume`, or `None` where it has none a claim can trust or
/// won't hand it over. Either way the claim goes ahead, with the voices carrying the level.
fn hardware_volume(card: &Card, volume: f64) -> Option<CardVolume> {
    alsa_volume::take(card, volume).unwrap_or_else(|e| {
        log::info!(
            "audio: {} keeps the volume in software, its own control refused: {}",
            card.device.name,
            describe(&e)
        );
        None
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

fn configure(pcm: &PCM, request: &ExclusiveRequest) -> Result<Config, ClaimError> {
    let ExclusiveRequest { shape, format: source, tuning, .. } = *request;
    let rate = shape.rate.get();
    let hw =
        HwParams::any(pcm).map_err(|e| ClaimError::io("Failed to read the card's limits", e))?;
    hw.set_access(Access::RWInterleaved)
        .map_err(|e| ClaimError::io("The card refused interleaved access", e))?;
    hw.set_rate_resample(false).map_err(|e| ClaimError::io("Failed to turn resampling off", e))?;
    hw.set_rate(rate, ValueOr::Nearest).map_err(|_| ClaimError::RateRefused { rate })?;
    let channels = set_channels(&hw, shape.channels)?;
    let format = set_format(&hw, source)?;

    let requested_period = u32::try_from(frames_in(tuning.period, shape.rate)).unwrap_or(u32::MAX);
    // The buffer's periods have to fit the card's largest buffer, and the period is set first, so a
    // long one at a high rate is held down here rather than left to shorten the buffer.
    let longest_period = hw
        .get_buffer_size_max()
        .map_err(|e| ClaimError::io("Failed to read the card's largest buffer", e))?
        / alsa::pcm::Frames::from(PERIODS_PER_BUFFER);
    let period = hw
        .set_period_size_near(
            alsa::pcm::Frames::from(requested_period).min(longest_period),
            ValueOr::Nearest,
        )
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
    let (format, alsa) = DeviceFormat::ladder(source)
        .iter()
        .filter_map(|&format| alsa_format(format).map(|alsa| (format, alsa)))
        .find(|&(_, alsa)| hw.test_format(alsa).is_ok())
        .ok_or(ClaimError::FormatRefused { format: source })?;
    hw.set_format(alsa).map_err(|_| ClaimError::FormatRefused { format: source })?;
    Ok(format)
}

/// The ALSA format for `format`, or `None` for the MSB-aligned 24-in-32, which ALSA only spells
/// as `S32_LE` and which the ladder already tries as that.
fn alsa_format(format: DeviceFormat) -> Option<Format> {
    match format {
        DeviceFormat::S16 => Some(Format::S16LE),
        DeviceFormat::S24Packed => Some(Format::S243LE),
        DeviceFormat::S24Low => Some(Format::S24LE),
        DeviceFormat::S24High => None,
        DeviceFormat::S32 => Some(Format::S32LE),
        DeviceFormat::F32 => Some(Format::FloatLE),
    }
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
    rate: SampleRate,
    block_samples: usize,
    /// The card's own volume where it carries the level.
    volume: Option<CardVolume>,
}

impl Writer {
    fn run(mut self) {
        realtime::promote_current_thread();
        // Info rather than a warning: a card that went away is recovered from, and only a
        // recovery that fails is something to warn about.
        if let Err(e) = self.write_until_stopped() {
            log::info!("audio: the exclusive output stopped: {}", describe(&e));
            self.feed.health.report_device_lost();
        }
        let Self { pcm, volume, control, .. } = self;
        // The card stops before its level goes back, so no buffered period plays at the original.
        drop(pcm);
        drop(volume);
        control.mark_closed();
    }

    /// Pull, encode and write one period at a time until told to stop.
    ///
    /// Nothing here logs: the health counters are how this loop reports, and the one error it
    /// returns is logged once, on the way out. A volume element that stops taking or reporting the
    /// level ends the stream like a write refused, and the claim that follows decides again whether
    /// to use it.
    fn write_until_stopped(&mut self) -> Result<(), alsa::Error> {
        let io = self.pcm.io_bytes();
        let frame_bytes = usize::try_from(self.pcm.frames_to_bytes(1))
            .unwrap_or(self.format.bytes_per_sample())
            .max(1);
        let mut block: Vec<Sample> = vec![0.0; self.block_samples];
        let mut bytes = Vec::with_capacity(self.block_samples * self.format.bytes_per_sample());
        while !self.control.stopping() {
            self.feed.fill(&mut block);
            encode::encode(&block, self.format, &mut bytes);
            write_all(&self.pcm, &io, &bytes, frame_bytes, &self.feed)?;
            // After the write, where the period's slack is.
            if let Some(volume) = &mut self.volume {
                volume.follow(self.feed.volume.load())?;
                if let Some(level) = volume.take_move()? {
                    self.feed.external_volume.report(level);
                }
            }
            self.report_lead();
        }
        Ok(())
    }

    /// Say how long the card holds what was just written before it is heard. A card that won't
    /// answer leaves the last reading standing: it is the ear's position that suffers, not the audio.
    fn report_lead(&self) {
        if let Ok(delay) = self.pcm.delay() {
            let frames = u64::try_from(delay).unwrap_or(0);
            self.feed.report_lead(frames_to_duration(frames, self.rate));
        }
    }
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
