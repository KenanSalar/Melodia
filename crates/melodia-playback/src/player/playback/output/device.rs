//! Opening the output device, and the ladder that keeps a boot from ending in silence.
//!
//! **The ladder is the whole point of this module.** Asking cpal for the device's default config
//! and giving up when it is refused turns any config the host dislikes into a start with no audio
//! at all — which is what rodio's `open_stream` did and why this tree always called
//! `open_sink_or_fallback` instead. What follows is that behaviour, owned: the default first, then
//! every config the device reports, in cpal's own preference order, taking the first that opens and
//! reporting the *original* failure if none do. Where a rate is asked for, the configs that can run
//! it go ahead of the default.
//!
//! **The block size is one of the things a rung varies, not a constant across them.** rodio asked
//! for a fixed block only on its first attempt: its retry rungs rebuilt the config from scratch and
//! dropped the request with it, so a host that took the config and refused `BufferSize::Fixed`
//! still opened. Carrying one size down every rung loses that, and `Fixed` is the least portable
//! thing in this file — on ALSA it names a *period*, checked against a range reported for the whole
//! buffer. So the walk runs twice, [`TARGET_BUFFER`] first and the host's own choice behind it.
//! Both passes run the whole ladder, which lets a rung at the size we want beat the device's own
//! config at a size we didn't — safe because `Fixed` is a device-wide constraint, so a first pass
//! that fails at the default fails at every rung and the second lands back on the default.
//!
//! It also has to survive CI, where there is no sound card and `.github/actions/headless-audio`
//! points ALSA's default PCM at the userspace `null` device. A stricter open than this one makes
//! `crates/melodia/tests/headless.rs` fail looking like a scan bug.

use std::sync::Arc;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SizedSample};
use parking_lot::Mutex;

use melodia_core::error::AppError;

use melodia_audio::player::source::audio::{ChannelCount, Sample, SampleRate, Shape};

use super::super::stream_health::{self, AudioStreamHealth};
use super::mixer::MixerPull;

/// Device frames the host is asked to hand over at a time.
///
/// Latency against wakeup cost, and it is what the position runs *ahead of* the ear by: the clock
/// counts frames handed to the device, which are audible a buffer later. rodio asked for the same
/// 50 ms; the value is a request rather than a promise, and a host outside its own reported range
/// clamps or ignores it. The second pass doesn't ask at all, and keeps this only as the size the
/// callback's staging buffer starts at.
const TARGET_BUFFER: Duration = Duration::from_millis(50);

/// What the device actually agreed to, beside what it was asked for.
///
/// Reported rather than assumed because every part of it can differ from the request, and because a
/// bit-perfect mode is only checkable if the negotiated end of it is visible.
#[derive(Debug, Clone)]
pub struct Negotiated {
    /// What the host calls the device, or `None` where it would not say. After a reopen this is
    /// the only thing telling a bug report which output the audio moved to.
    pub device_name: Option<String>,
    pub shape: Shape,
    pub format: cpal::SampleFormat,
    /// The period that was asked for, or `None` where the host was left to name its own.
    ///
    /// Kept beside the answer because it is the one of the two that says which pass of the ladder
    /// won, which is the difference between a block this tree sized and one nobody did.
    pub requested_period: Option<cpal::FrameCount>,
    /// What the host says it will hand the callback at a time, or `None` where it cannot say.
    ///
    /// Asked rather than inferred: `StreamTrait::buffer_size` arrived in cpal 0.18, and before it
    /// the only place the real block appeared was `data.len()` inside the callback. cpal calls it
    /// advisory and the hosts that don't track one answer `UnsupportedOperation`, so this is where
    /// a bug report reads the block back, not a bound anything sizes against.
    pub period: Option<cpal::FrameCount>,
}

/// The live stream. Dropping it stops audio and releases the device.
pub struct DeviceStream {
    _stream: cpal::Stream,
    negotiated: Negotiated,
}

impl DeviceStream {
    pub fn negotiated(&self) -> Negotiated {
        self.negotiated.clone()
    }
}

/// The device an open is aimed at, and the config it names as its own.
///
/// Resolved afresh for every open rather than kept, so a reopen after the device went away lands
/// on whatever the system calls its default *now*.
pub struct Target {
    device: cpal::Device,
    default: cpal::SupportedStreamConfig,
}

impl Target {
    /// The shape the device prefers, which is the first rung [`open`] tries.
    ///
    /// # Errors
    ///
    /// [`AppError::Player`] when that config reports no channels or no rate.
    pub fn default_shape(&self) -> Result<Shape, AppError> {
        shape_of(&self.default)
    }
}

/// The system's default output device.
///
/// # Errors
///
/// [`AppError::Player`] when there is no output device, or when it cannot name its own default
/// config.
pub fn default_target() -> Result<Target, AppError> {
    let device = cpal::default_host()
        .default_output_device()
        .ok_or_else(|| AppError::Player("No audio output device".to_owned()))?;

    let default = device
        .default_output_config()
        .map_err(|e| AppError::Player(format!("Failed to read the output device's config: {e}")))?;

    Ok(Target { device, default })
}

/// What every stream's callbacks hold: the puller, and the health cell they report into.
///
/// The puller never leaves its owner: each stream holds a clone of the `Arc`, so dropping the
/// stream is all it takes to get it back.
#[derive(Clone)]
pub struct Feed {
    pub(super) pull: Arc<Mutex<MixerPull>>,
    pub(super) health: Arc<AudioStreamHealth>,
}

impl Feed {
    pub fn new(pull: MixerPull, health: Arc<AudioStreamHealth>) -> Self {
        Self { pull: Arc::new(Mutex::new(pull)), health }
    }

    /// Pull one block, or write silence where the puller is taken.
    ///
    /// **The beat comes first and unconditionally**, silence included: a callback that has stopped
    /// being called is the one failure no host reports, and the beat is how it is seen.
    ///
    /// `try_lock` because this is the audio thread. The one holder it can meet is a reopen
    /// reshaping the puller, and that only happens once the stream it would be racing has been
    /// dropped, so the silence arm is reachable in principle and inaudible in practice.
    fn fill(&self, block: &mut [Sample]) {
        self.health.beat();
        match self.pull.try_lock() {
            Some(mut pull) => pull.fill(block),
            None => block.fill(0.0),
        }
    }
}

/// Open a stream on `target`, feeding it from `feed` at whichever config takes, `rate` first where
/// one is asked for.
///
/// Each rung reshapes the puller to its own shape before building, because only opening the stream
/// says whether that shape works.
///
/// # Errors
///
/// [`AppError::Player`] when nothing opens.
pub fn open(
    target: &Target,
    feed: &Feed,
    rate: Option<SampleRate>,
) -> Result<DeviceStream, AppError> {
    let Target { device, default } = target;

    // Listed once for both passes, and **not** with `?`: a device that cannot enumerate can still
    // open the config it just named as its default, which is the likeliest rung of all and the one
    // rodio reached first — it only lists inside the fallback its default attempt failed into. A
    // `?` here spends a listing failure on the whole open without trying that config once.
    let supported = supported_configs(device).unwrap_or_else(|e| {
        log::warn!("Falling back to the default output config alone: {e}");
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
    Err(first.unwrap_or_else(|| AppError::Player("The output device offered no config".to_owned())))
}

/// Every config the device reports, best first by cpal's own ordering, which is what rodio walked.
fn supported_configs(
    device: &cpal::Device,
) -> Result<Vec<cpal::SupportedStreamConfigRange>, AppError> {
    let mut supported: Vec<_> = device
        .supported_output_configs()
        .map_err(|e| AppError::Player(format!("Failed to list the output device's configs: {e}")))?
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

/// Rank `supported`, already in preference order, into [`Rungs`] for `rate`.
///
/// `try_` rather than `with_sample_rate`, whose out-of-range arm is an `expect` that would take the
/// boot with it: a range that cannot run a rate answers `None` and drops out.
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

/// The rates one reported range is tried at: its top, the two standard rates, then its floor.
///
/// **A standard rate only when it falls strictly inside.** The endpoints are already the rungs
/// either side of it, so the strict test is what stops a duplicate, and a rung costs a stream the
/// driver may take its time refusing. The floor drops the same way where it *is* the top, which
/// rodio's walk emitted twice.
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

/// Build and start a stream for one config, or say why it could not be.
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
    stream
        .play()
        .map_err(|e| AppError::Player(format!("Failed to start the audio stream: {e}")))?;
    // A host with no answer is a log line missing one term, never a rung that fails. The name
    // likewise.
    let period = stream.buffer_size().ok();
    let device_name = device.description().ok().map(|description| description.name().to_owned());

    Ok(DeviceStream {
        _stream: stream,
        negotiated: Negotiated { device_name, shape, format, requested_period, period },
    })
}

fn shape_of(supported: &cpal::SupportedStreamConfig) -> Result<Shape, AppError> {
    ChannelCount::new(supported.channels())
        .zip(SampleRate::new(supported.sample_rate()))
        .map(|(channels, rate)| Shape { channels, rate })
        .ok_or_else(|| AppError::Player("Output config has no channels or no rate".to_owned()))
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

/// The period [`Buffer::Target`] asks for: [`target_frames`] held inside what the device reports.
///
/// **Held under half the reported maximum, not under it.** On ALSA the range bounds the *buffer*
/// while a `Fixed` is a period cpal doubles into a buffer request, so asking for the whole of it
/// lands on one period per buffer. Nothing refuses that either — the `_near` setters take what they
/// are given — so the stream opens and underruns for as long as it is held, where a rung that fails
/// outright would just fall through to the next one. Core Audio's range is the callback size itself
/// and WASAPI usually reports none, so there the halving costs a little latency and buys nothing.
///
/// `min` last rather than `clamp`, which asserts its two bounds are the right way round: the pair
/// comes straight off a driver, and one reporting them backwards would panic the boot.
fn period_frames(supported: &cpal::SupportedStreamConfig) -> cpal::FrameCount {
    let target = target_frames(supported);
    let held = match supported.buffer_size() {
        cpal::SupportedBufferSize::Range { min, max } => target.min(max / 2).max(*min),
        cpal::SupportedBufferSize::Unknown => target,
    };
    // A driver reporting a zero floor and a ceiling under two lands this on zero, which cpal 0.18
    // refuses before the driver sees it — so the rung would fail on the request rather than on the
    // device. One frame is something a driver can round up, which beats not asking.
    held.max(1)
}

/// Samples the callback's staging buffer holds before it has ever run.
///
/// [`TARGET_BUFFER`]'s worth as a floor, which on [`Buffer::HostChoice`] is a floor under a block
/// nobody named — comfortably over the 512–2048 frames the mainstream hosts pick for themselves.
/// Sizing that pass from the request would size it from nothing, leaving the callback to allocate
/// its way up to the host's own block, on the one thread in the process that must not wait for the
/// arena lock.
///
/// The larger of the two rather than [`period_frames`] alone, for the same reason in reverse: a
/// period narrowed to what the device can double-buffer bounds what we may *ask* for, not what a
/// host may hand over. A device whose floor sits above the target pushes the period the other way,
/// and staging follows it up or the first callback allocates.
fn staging_samples(supported: &cpal::SupportedStreamConfig) -> usize {
    let frames = target_frames(supported).max(period_frames(supported));
    frames as usize * usize::from(supported.channels())
}

/// One arm per sample format the host can ask for, because the callback is monomorphic in it.
///
/// Every arm but one stages in [`Sample`] and converts on the way out, so [`MixerPull::fill`] stays
/// the one place samples are produced however the device wants them — the conversion is the only
/// thing that differs between a shared stream and an exclusive one later. The exception is the
/// format that *is* [`Sample`], which needs neither half; [`direct_stream`] says what that saves.
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
                other => Err(AppError::Player(format!("Unsupported sample format {other}"))),
            }
        };
    }

    // Every format `SizedSample` names bar `F32` above, which together is what rodio covered.
    // The three DSD ones are outside it by construction — nothing implements `SizedSample` for
    // them, so only `build_output_stream_raw` could carry one — and `SampleFormat` is
    // `#[non_exhaustive]` besides, which is what makes `other` the compiler's requirement rather
    // than a courtesy. The 24-bit pair is the one worth naming: cpal's own config ordering ranks
    // it above `I16` now, so a card offering nothing else is no longer the edge case it was, and
    // without an arm it would fail every rung of the ladder and take the boot with it.
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

/// The stream for a device that already speaks [`Sample`]: no staging buffer, no conversion pass,
/// the mixer writing straight into the block cpal handed over.
///
/// This is the rung essentially every install lands on, the default config being `f32` on ALSA,
/// `PipeWire`, `CoreAudio` and WASAPI shared mode, so what the other arms need is worth not paying
/// here: a resident buffer the size of one period, and a full pass over every block to copy each
/// sample onto itself. [`MixerPull::fill`] zeroes what it is handed before writing, partial
/// trailing frame included, so nothing depended on owning that buffer first.
fn direct_stream(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    feed: Feed,
) -> Result<cpal::Stream, AppError> {
    let error_callback = stream_health::error_callback(Arc::clone(&feed.health));
    opened(device.build_output_stream::<Sample, _, _>(
        config,
        move |data, _| feed.fill(data),
        error_callback,
        None,
    ))
}

/// The one wording for a stream that would not open, so the two builders can't drift.
fn opened(built: Result<cpal::Stream, cpal::Error>) -> Result<cpal::Stream, AppError> {
    built.map_err(|e| AppError::Player(format!("Failed to open the audio stream: {e}")))
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
    // A host handing over more than `staging_samples` allowed for grows this once and keeps it.
    let mut staging: Vec<Sample> = vec![0.0; staging_samples];
    opened(device.build_output_stream::<T, _, _>(
        config,
        move |data, _| {
            if staging.len() < data.len() {
                staging.resize(data.len(), 0.0);
            }
            let block = &mut staging[..data.len()];
            feed.fill(block);
            for (slot, sample) in data.iter_mut().zip(block.iter()) {
                *slot = T::from_sample(*sample);
            }
        },
        error_callback,
        None,
    ))
}

#[cfg(test)]
#[path = "tests/device_tests.rs"]
mod tests;
