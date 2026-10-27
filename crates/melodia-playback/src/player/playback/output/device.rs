//! Opening the output device through a ladder, so a config the host dislikes never boots into
//! silence: the default first, then every config the device reports in cpal's order, taking the
//! first that opens and reporting the original failure if none do. A requested rate's configs go
//! ahead of the default. CI's null device depends on it (root `CLAUDE.md`).
//!
//! The walk runs twice, [`TARGET_BUFFER`] first and the host's own block size behind it, `Fixed`
//! being the least portable request here (on ALSA a period, checked against a range for the whole
//! buffer). It is device-wide, so a first pass failing at the default fails at every rung and the
//! second lands back on the default.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SizedSample};
use parking_lot::Mutex;
use tokio::sync::Notify;

use melodia_core::error::{AppError, describe};

use melodia_audio::player::source::audio::{ChannelCount, Sample, SampleRate, Shape};

use super::super::dsp::AtomicF64;
use super::super::stream_health::{self, AudioStreamHealth};
use super::dither::Dither;
use super::encode::DeviceFormat;
use super::mixer::MixerPull;
use super::{Negotiated, OutputFormat};

/// Latency against wakeup cost, and a request the host may clamp or ignore. The second pass asks
/// nothing and keeps this only as the staging buffer's starting size.
const TARGET_BUFFER: Duration = Duration::from_millis(50);

/// The live stream. Dropping it stops audio and releases the device.
pub struct DeviceStream {
    _stream: cpal::Stream,
    pub(super) negotiated: Negotiated,
}

impl DeviceStream {
    pub fn negotiated(&self) -> Negotiated {
        self.negotiated.clone()
    }
}

/// Resolved per open, so a reopen asks the system again for the named device or today's default.
pub struct Target {
    device: cpal::Device,
    default: cpal::SupportedStreamConfig,
}

impl Target {
    /// The shape the device prefers, which is the first rung [`open`] tries.
    pub fn default_shape(&self) -> Result<Shape, AppError> {
        shape_of(&self.default)
    }

    /// What the host calls the device, or `None` where it would not say.
    pub fn name(&self) -> Option<String> {
        device_name(&self.device)
    }
}

fn device_name(device: &cpal::Device) -> Option<String> {
    device.description().ok().map(|description| description.name().to_owned())
}

/// `host` is the caller's to keep: on ALSA, cpal frees alsa-lib's parsed config once its last
/// host is gone, which a host made per open reaches at every reopen.
pub fn default_target(host: &cpal::Host) -> Result<Target, AppError> {
    let device = host
        .default_output_device()
        .ok_or_else(|| AppError::player_msg("No audio output device"))?;
    target_of(device)
}

/// `None` only where no device lists under `id`; anything else is an error, or a caller waiting for
/// a missing device would find it listed and reopen it endlessly. Listed by hand, cpal's
/// `device_by_id` answering `None` for a failed listing too.
pub fn named_target(host: &cpal::Host, id: &str) -> Result<Option<Target>, AppError> {
    let wanted = cpal::DeviceId::new(host.id(), id);
    let devices = host
        .output_devices()
        .map_err(|e| AppError::player("Failed to list the output devices", e))?;
    let mut unreadable = None;
    for device in devices {
        match device.id() {
            Ok(found) if found == wanted => return target_of(device).map(Some),
            Ok(_) => {}
            Err(e) => unreadable = Some(e),
        }
    }
    match unreadable {
        Some(e) => Err(AppError::player("Failed to read an output device's id", e)),
        None => Ok(None),
    }
}

fn target_of(device: cpal::Device) -> Result<Target, AppError> {
    let default = device
        .default_output_config()
        .map_err(|e| AppError::player("Failed to read the output device's config", e))?;
    Ok(Target { device, default })
}

/// How far the ear is behind the voices' clocks, which count what was pulled. Only the backend
/// feeding a device can measure it; zero until one does.
#[derive(Debug, Default)]
pub struct Lead {
    nanos: AtomicU64,
}

impl Lead {
    pub fn get(&self) -> Duration {
        Duration::from_nanos(self.nanos.load(Ordering::Relaxed))
    }

    fn set(&self, lead: Duration) {
        self.nanos.store(u64::try_from(lead.as_nanos()).unwrap_or(u64::MAX), Ordering::Relaxed);
    }

    /// A gone stream's measurement says nothing about the next.
    pub(super) fn clear(&self) {
        self.set(Duration::ZERO);
    }
}

/// A level the system moved the device's own volume to, waiting for the player. One slot, only the
/// latest meaning anything.
#[derive(Debug)]
pub struct ExternalVolume {
    /// The level's bits, or [`ExternalVolume::NONE`] while nothing waits.
    level: AtomicU64,
    moved: Notify,
}

impl ExternalVolume {
    /// A NaN, which no level a device reports can be.
    const NONE: u64 = u64::MAX;

    /// Hand over `level`, the fraction the system shows the device's volume at. A store and a wake,
    /// so a writer thread may call it.
    pub fn report(&self, level: f64) {
        self.level.store(level.to_bits(), Ordering::Release);
        self.moved.notify_one();
    }

    /// The next level [`Self::report`] hands over, waiting for one where none is waiting yet.
    pub async fn next(&self) -> f64 {
        loop {
            let bits = self.level.swap(Self::NONE, Ordering::AcqRel);
            if bits != Self::NONE {
                return f64::from_bits(bits);
            }
            self.moved.notified().await;
        }
    }
}

impl Default for ExternalVolume {
    fn default() -> Self {
        Self { level: AtomicU64::new(Self::NONE), moved: Notify::new() }
    }
}

/// What every stream's callbacks hold. Each stream holds an `Arc` clone, so dropping the stream
/// returns the puller.
#[derive(Clone)]
pub struct Feed {
    pub(super) pull: Arc<Mutex<MixerPull>>,
    pub(super) health: Arc<AudioStreamHealth>,
    pub(super) lead: Arc<Lead>,
    /// The user's volume, as an amplitude. The voices take theirs from the engine; this is the copy
    /// a writer thread can read, and it outlives every stream so a reopen claims at the level set.
    pub(super) volume: Arc<AtomicF64>,
    /// Where a claim carrying the volume on the device reports it moved without Melodia.
    pub(super) external_volume: Arc<ExternalVolume>,
}

impl Feed {
    pub fn new(pull: MixerPull, health: Arc<AudioStreamHealth>) -> Self {
        Self {
            pull: Arc::new(Mutex::new(pull)),
            health,
            lead: Arc::default(),
            volume: Arc::new(AtomicF64::new(1.0)),
            external_volume: Arc::default(),
        }
    }

    /// An atomic store, so the audio thread may call it.
    pub(super) fn report_lead(&self, lead: Duration) {
        self.lead.set(lead);
    }

    /// The beat comes first, silence included: a callback no longer called is the one failure no
    /// host reports. `try_lock` on the audio thread, the one holder it can meet being its own
    /// reopen, whose gap swallows the silent block.
    pub(super) fn fill(&self, block: &mut [Sample]) {
        self.health.beat();
        match self.pull.try_lock() {
            Some(mut pull) => pull.fill(block),
            None => block.fill(0.0),
        }
    }
}

/// Each rung reshapes the puller first, only opening the stream saying whether a shape works.
pub fn open(
    target: &Target,
    feed: &Feed,
    rate: Option<SampleRate>,
) -> Result<DeviceStream, AppError> {
    let Target { device, default } = target;

    // Not `?`: a device that can't enumerate can still open its own default, the likeliest rung.
    let supported = supported_configs(device).unwrap_or_else(|e| {
        log::warn!("Falling back to the default output config alone: {}", describe(&e));
        Vec::new()
    });
    let Rungs { preferred, fallback } = ladder(supported, rate);

    let mut first = None;
    for buffer in [Buffer::Target, Buffer::HostChoice] {
        // The device's own config leads each pass unless a rate was asked for: it is the likeliest
        // to open, and on the second pass it has not been tried at that block size at all.
        for candidate in preferred.iter().chain(std::iter::once(default)).chain(&fallback) {
            match attempt(device, candidate, buffer, feed) {
                Ok(opened) => return Ok(opened),
                Err(e) => {
                    first.get_or_insert(e);
                }
            }
        }
    }
    // The first failure, not the last: it is the one about the config the device itself named at
    // the block we wanted, and the rest are about configs and sizes nobody asked for.
    Err(first.unwrap_or_else(|| AppError::player_msg("The output device offered no config")))
}

/// Best first by cpal's own ordering.
fn supported_configs(
    device: &cpal::Device,
) -> Result<Vec<cpal::SupportedStreamConfigRange>, AppError> {
    let mut supported: Vec<_> = device
        .supported_output_configs()
        .map_err(|e| AppError::player("Failed to list the output device's configs", e))?
        .collect();
    supported.sort_by(|a, b| b.cmp_default_heuristics(a));
    Ok(supported)
}

/// The configs tried beside the device's own, split around it.
struct Rungs {
    /// Every range that can run the requested rate, at exactly that rate. Tried ahead of the
    /// device's default, since the default is precisely what following the file means not taking.
    preferred: Vec<cpal::SupportedStreamConfig>,
    /// Each range at its top rate, then the two standard rates where those are in range, then its
    /// floor.
    fallback: Vec<cpal::SupportedStreamConfig>,
}

/// `try_` rather than `with_sample_rate`, whose out-of-range arm is an `expect` that would take the
/// boot with it.
fn ladder(supported: Vec<cpal::SupportedStreamConfigRange>, rate: Option<SampleRate>) -> Rungs {
    let preferred = rate.map_or_else(Vec::new, |rate| {
        supported.iter().filter_map(|range| range.try_with_sample_rate(rate.get())).collect()
    });
    let fallback = supported
        .into_iter()
        .flat_map(|range| {
            let (min, max) = (range.min_sample_rate(), range.max_sample_rate());
            rates_for(min, max).filter_map(move |rate| range.try_with_sample_rate(rate))
        })
        .collect();
    Rungs { preferred, fallback }
}

/// Its top, the two standard rates strictly inside, then its floor where it isn't the top: a
/// duplicate rung costs a stream the driver may be slow to refuse.
fn rates_for(
    min: cpal::SampleRate,
    max: cpal::SampleRate,
) -> impl Iterator<Item = cpal::SampleRate> {
    // 48 kHz ahead of 44.1: cpal's own default config prefers it, and it is what current hardware
    // runs natively, so it is the likelier of the two to open.
    let standard = [cpal::SAMPLE_RATE_48K, cpal::SAMPLE_RATE_CD]
        .into_iter()
        .filter(move |&rate| min < rate && rate < max);
    let floor = (min < max).then_some(min);
    std::iter::once(max).chain(standard).chain(floor)
}

fn attempt(
    device: &cpal::Device,
    supported: &cpal::SupportedStreamConfig,
    buffer: Buffer,
    feed: &Feed,
) -> Result<DeviceStream, AppError> {
    let shape = shape_of(supported)?;
    feed.pull.lock().reshape(shape);

    let requested_period = buffer.requested(supported);
    let mut config = supported.config();
    config.buffer_size =
        requested_period.map_or(cpal::BufferSize::Default, cpal::BufferSize::Fixed);

    let format = supported.sample_format();
    let staging = staging_samples(supported);
    let stream = build_stream(device, config, format, staging, feed.clone())?;
    stream.play().map_err(|e| AppError::player("Failed to start the audio stream", e))?;
    // A host with no answer is a log line missing one term, never a rung that fails. The name
    // likewise.
    let period = stream.buffer_size().ok();
    let device_name = device_name(device);

    Ok(DeviceStream {
        _stream: stream,
        negotiated: Negotiated {
            device_name,
            shape,
            format: OutputFormat::Shared(format),
            fallback: None,
            hardware_volume: false,
            device_level: None,
            offered: None,
            requested_period,
            period,
        },
    })
}

fn shape_of(supported: &cpal::SupportedStreamConfig) -> Result<Shape, AppError> {
    ChannelCount::new(supported.channels())
        .zip(SampleRate::new(supported.sample_rate()))
        .map(|(channels, rate)| Shape { channels, rate })
        .ok_or_else(|| AppError::player_msg("Output config has no channels or no rate"))
}

/// What one rung asks the host to hand the callback at a time.
#[derive(Clone, Copy)]
enum Buffer {
    /// [`TARGET_BUFFER`], which is what we actually want.
    Target,
    /// Whatever the host picks, which for some hosts is the only answer they take.
    HostChoice,
}

impl Buffer {
    /// The period this rung asks for, or `None` where the host names its own.
    fn requested(self, supported: &cpal::SupportedStreamConfig) -> Option<cpal::FrameCount> {
        match self {
            Self::Target => Some(period_frames(supported)),
            Self::HostChoice => None,
        }
    }
}

/// [`TARGET_BUFFER`] in frames at this config's rate, before any device range narrows it.
fn target_frames(supported: &cpal::SupportedStreamConfig) -> cpal::FrameCount {
    let target = u128::from(supported.sample_rate()) * TARGET_BUFFER.as_millis() / 1_000;
    cpal::FrameCount::try_from(target).unwrap_or(cpal::FrameCount::MAX)
}

/// [`target_frames`] held under half the reported maximum: on ALSA the range bounds the buffer while
/// `Fixed` is a period cpal doubles, and asking for all of it opens a stream that underruns rather
/// than failing over. `min` then `max` rather than `clamp`, which asserts on a driver reporting its
/// bounds backwards.
fn period_frames(supported: &cpal::SupportedStreamConfig) -> cpal::FrameCount {
    let target = target_frames(supported);
    let held = match supported.buffer_size() {
        cpal::SupportedBufferSize::Range { min, max } => target.min(max / 2).max(*min),
        cpal::SupportedBufferSize::Unknown => target,
    };
    // cpal refuses a zero period before the driver sees it; one frame a driver can round up.
    held.max(1)
}

/// At least [`TARGET_BUFFER`]'s worth and at least the period, so neither a host-chosen block nor a
/// high device floor makes the first callback allocate on the audio thread.
fn staging_samples(supported: &cpal::SupportedStreamConfig) -> usize {
    let frames = target_frames(supported).max(period_frames(supported));
    frames as usize * usize::from(supported.channels())
}

/// One arm per sample format, the callback being monomorphic in it. Every arm but `F32` stages and
/// converts on the way out, so [`MixerPull::fill`] stays the one place samples are produced.
fn build_stream(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    format: cpal::SampleFormat,
    staging_samples: usize,
    feed: Feed,
) -> Result<cpal::Stream, AppError> {
    if format == cpal::SampleFormat::F32 {
        return direct_stream(device, config, feed);
    }

    macro_rules! arms {
        ($($variant:ident => $ty:ty),+ $(,)?) => {
            match format {
                $(cpal::SampleFormat::$variant => {
                    output_stream::<$ty>(device, config, staging_samples, feed)
                })+
                other => Err(AppError::player_msg(format!("Unsupported sample format {other}"))),
            }
        };
    }

    // Every `SizedSample` format bar `F32`; DSD has no `SizedSample`, and `SampleFormat` is
    // `#[non_exhaustive]`. The 24-bit pair matters: cpal ranks it above `I16`, and a card offering
    // nothing else would fail every rung without an arm.
    arms! {
        F64 => f64,
        I8 => i8,
        I16 => i16,
        I24 => cpal::I24,
        I32 => i32,
        I64 => i64,
        U8 => u8,
        U16 => u16,
        U24 => cpal::U24,
        U32 => u32,
        U64 => u64,
    }
}

/// For a device already speaking [`Sample`], nearly every install's default: no staging buffer
/// and no copy pass, [`MixerPull::fill`] zeroing what it is handed.
fn direct_stream(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    feed: Feed,
) -> Result<cpal::Stream, AppError> {
    let error_callback = stream_health::error_callback(Arc::clone(&feed.health));
    let clock = SampleClock::of(&config);
    opened(device.build_output_stream::<Sample, _, _>(
        config,
        move |data, info| {
            feed.fill(data);
            feed.report_lead(clock.lead_after(info, data.len()));
        },
        error_callback,
        None,
    ))
}

/// How long a stream's interleaved samples last, for the lead each callback reports.
#[derive(Clone, Copy)]
struct SampleClock {
    samples_per_second: f64,
}

impl SampleClock {
    fn of(config: &cpal::StreamConfig) -> Self {
        Self { samples_per_second: f64::from(config.sample_rate) * f64::from(config.channels) }
    }

    /// What is still unplayed once a block of `samples` is written: whatever the host queued ahead
    /// of it, which is how long until it says the block plays, and the block itself.
    fn lead_after(self, info: &cpal::OutputCallbackInfo, samples: usize) -> Duration {
        let timestamp = info.timestamp();
        let queued = timestamp.playback.duration_since(timestamp.callback);
        let samples = f64::from(u32::try_from(samples).unwrap_or(u32::MAX));
        let block = samples / self.samples_per_second.max(1.0);
        queued + Duration::try_from_secs_f64(block).unwrap_or_default()
    }
}

/// The one wording for a stream that would not open, so the two builders can't drift.
fn opened(built: Result<cpal::Stream, cpal::Error>) -> Result<cpal::Stream, AppError> {
    built.map_err(|e| AppError::player("Failed to open the audio stream", e))
}

fn output_stream<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    staging_samples: usize,
    feed: Feed,
) -> Result<cpal::Stream, AppError>
where
    T: SizedSample + FromSample<Sample>,
{
    let error_callback = stream_health::error_callback(Arc::clone(&feed.health));
    let clock = SampleClock::of(&config);
    // A host handing over more than `staging_samples` allowed for grows this once and keeps it.
    let mut staging: Vec<Sample> = vec![0.0; staging_samples];
    let rung = dithered_rung(T::FORMAT);
    let mut dither = Dither::default();
    opened(device.build_output_stream::<T, _, _>(
        config,
        move |data, info| {
            if staging.len() < data.len() {
                staging.resize(data.len(), 0.0);
            }
            let block = &mut staging[..data.len()];
            feed.fill(block);
            if let Some(rung) = rung {
                dither.quantize(block, rung);
            }
            for (slot, sample) in data.iter_mut().zip(block.iter()) {
                *slot = T::from_sample(*sample);
            }
            feed.report_lead(clock.lead_after(info, data.len()));
        },
        error_callback,
        None,
    ))
}

/// The grid a narrow shared integer format puts samples on, the container aside. Putting a block on
/// it first also makes cpal's conversion exact, which alone truncates and wraps 24-bit full scale.
fn dithered_rung(format: cpal::SampleFormat) -> Option<DeviceFormat> {
    match format {
        cpal::SampleFormat::I16 | cpal::SampleFormat::U16 => Some(DeviceFormat::S16),
        cpal::SampleFormat::I24 | cpal::SampleFormat::U24 => Some(DeviceFormat::S24Low),
        _ => None,
    }
}

#[cfg(test)]
#[path = "tests/device_tests.rs"]
mod tests;
