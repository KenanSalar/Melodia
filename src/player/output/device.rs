//! Opening the output device, and the ladder that keeps a boot from ending in silence.
//!
//! **The ladder is the whole point of this module.** Asking cpal for the device's default config
//! and giving up when it is refused turns any config the host dislikes into a start with no audio
//! at all — which is what rodio's `open_stream` did and why this tree always called
//! `open_sink_or_fallback` instead. What follows is that behaviour, owned: the default first, then
//! every config the device reports, in cpal's own preference order, taking the first that opens and
//! reporting the *original* failure if none do.
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
//! `tests/headless.rs` fail looking like a scan bug.

use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SizedSample};

use crate::error::AppError;

use super::super::audio::{ChannelCount, Sample, SampleRate};
use super::convert::Shape;
use super::mixer::MixerPull;

/// Device frames the host is asked to hand over at a time.
///
/// Latency against wakeup cost, and it is what the position runs *ahead of* the ear by: the clock
/// counts frames handed to the device, which are audible a buffer later. rodio asked for the same
/// 50 ms; the value is a request rather than a promise, and a host outside its own reported range
/// clamps or ignores it. The second pass doesn't ask at all, and keeps this only as the size the
/// callback's staging buffer starts at.
const TARGET_BUFFER: Duration = Duration::from_millis(50);

/// What the device actually agreed to, as opposed to what it was asked for.
///
/// Reported rather than assumed because every part of it can differ from the request, and because a
/// bit-perfect mode is only checkable if the negotiated end of it is visible.
#[derive(Debug, Clone, Copy)]
pub struct Negotiated {
    pub shape: Shape,
    pub format: cpal::SampleFormat,
    /// The period that was asked for, or `None` where the host was left to name its own.
    ///
    /// The request rather than the answer — cpal reports no block size back, and the only place
    /// the real one appears is `data.len()` inside the callback. Worth carrying anyway: it says
    /// which pass of the ladder won, which is the difference between a block this tree sized and
    /// one nobody did, and the boot log line is where a bug report reads that back.
    pub period: Option<cpal::FrameCount>,
}

/// The live stream. Dropping it stops audio and releases the device.
pub struct DeviceStream {
    _stream: cpal::Stream,
    negotiated: Negotiated,
}

impl DeviceStream {
    pub fn negotiated(&self) -> Negotiated {
        self.negotiated
    }
}

/// Open the default device, building the callback's state for whichever config takes.
///
/// `build` is handed the shape of each attempt and returns the puller for it plus whatever the
/// caller wants to keep from the successful one. It runs per attempt because the mixer has to be
/// built against the negotiated shape, and only opening the stream says whether that shape works.
///
/// # Errors
///
/// [`AppError::Player`] when there is no output device, when it cannot name its own default
/// config, or when nothing opens.
pub fn open<T, E, B>(mut build: B, error_callback: E) -> Result<(DeviceStream, T), AppError>
where
    E: FnMut(cpal::StreamError) + Clone + Send + 'static,
    B: FnMut(Shape) -> (T, MixerPull),
{
    let device = cpal::default_host()
        .default_output_device()
        .ok_or_else(|| AppError::Player("No audio output device".to_owned()))?;

    let default = device
        .default_output_config()
        .map_err(|e| AppError::Player(format!("Failed to read the output device's config: {e}")))?;

    // Listed once for both passes, and **not** with `?`: a device that cannot enumerate can still
    // open the config it just named as its default, which is the likeliest rung of all and the one
    // rodio reached first — it only lists inside the fallback its default attempt failed into. A
    // `?` here spends a listing failure on the whole boot without trying that config once.
    let rungs = ladder(&device).unwrap_or_else(|e| {
        log::warn!("Falling back to the default output config alone: {e}");
        Vec::new()
    });

    let mut first = None;
    for buffer in [Buffer::Target, Buffer::HostChoice] {
        // The device's own config leads each pass: it is the likeliest to open, and on the second
        // pass it has not been tried at that block size at all.
        for candidate in std::iter::once(&default).chain(&rungs) {
            match attempt(&device, candidate, buffer, &mut build, error_callback.clone()) {
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

/// Every config the device reports, best first, each at its top rate, then 44.1 kHz where that is
/// in range, then its floor. cpal's own ordering, which is what rodio walked.
fn ladder(device: &cpal::Device) -> Result<Vec<cpal::SupportedStreamConfig>, AppError> {
    let mut supported: Vec<_> = device
        .supported_output_configs()
        .map_err(|e| AppError::Player(format!("Failed to list the output device's configs: {e}")))?
        .collect();
    supported.sort_by(|a, b| b.cmp_default_heuristics(a));

    Ok(supported
        .into_iter()
        .flat_map(|range| {
            let (min, max) = (range.min_sample_rate(), range.max_sample_rate());
            rates_for(min, max).map(move |rate| range.with_sample_rate(rate))
        })
        .collect())
}

/// The rates one reported range is tried at: its top, then 44.1 kHz, then its floor.
///
/// **44.1 kHz only when it falls strictly inside**, and that is a panic guard rather than a
/// tidiness one: `SupportedStreamConfigRange::with_sample_rate` is a `try_` plus an `expect`, so a
/// rate outside the range takes the boot with it. Strict because the two endpoints are already the
/// entries either side of it, which makes the guard free.
///
/// The floor is dropped where it *is* the top, which rodio's walk emitted twice: a rung costs a
/// whole `build`, and that allocates the mixer the failed attempt then throws away.
fn rates_for(
    min: cpal::SampleRate,
    max: cpal::SampleRate,
) -> impl Iterator<Item = cpal::SampleRate> {
    const PREFERRED_RATE: cpal::SampleRate = 44_100;

    let preferred = (min < PREFERRED_RATE && PREFERRED_RATE < max).then_some(PREFERRED_RATE);
    let floor = (min < max).then_some(min);
    std::iter::once(max).chain(preferred).chain(floor)
}

/// Build and start a stream for one config, or say why it could not be.
fn attempt<T, E, B>(
    device: &cpal::Device,
    supported: &cpal::SupportedStreamConfig,
    buffer: Buffer,
    build: &mut B,
    error_callback: E,
) -> Result<(DeviceStream, T), AppError>
where
    E: FnMut(cpal::StreamError) + Send + 'static,
    B: FnMut(Shape) -> (T, MixerPull),
{
    let Some(shape) = shape_of(supported) else {
        return Err(AppError::Player("Output config has no channels or no rate".to_owned()));
    };
    let (kept, pull) = build(shape);

    let period = buffer.requested(supported);
    let mut config = supported.config();
    config.buffer_size = period.map_or(cpal::BufferSize::Default, cpal::BufferSize::Fixed);

    let format = supported.sample_format();
    let staging = staging_samples(supported);
    let stream = build_stream(device, &config, format, staging, pull, error_callback)?;
    stream
        .play()
        .map_err(|e| AppError::Player(format!("Failed to start the audio stream: {e}")))?;

    Ok((
        DeviceStream {
            _stream: stream,
            negotiated: Negotiated {
                shape,
                format,
                period,
            },
        },
        kept,
    ))
}

fn shape_of(supported: &cpal::SupportedStreamConfig) -> Option<Shape> {
    Some(Shape {
        channels: ChannelCount::new(supported.channels())?,
        rate: SampleRate::new(supported.sample_rate())?,
    })
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
    // A driver reporting a zero floor and a ceiling under two lands this on zero — a request for a
    // period of nothing, which cpal's range check waves through, zero being inside the range it
    // checks. One frame either fails that check or gets rounded up, and both beat a dead stream.
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
/// Every arm stages in [`Sample`] and converts on the way out, so [`MixerPull::fill`] stays the one
/// place samples are produced however the device wants them — the conversion is the only thing that
/// differs between a shared stream and an exclusive one later.
fn build_stream<E>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    format: cpal::SampleFormat,
    staging_samples: usize,
    pull: MixerPull,
    error_callback: E,
) -> Result<cpal::Stream, AppError>
where
    E: FnMut(cpal::StreamError) + Send + 'static,
{
    macro_rules! arms {
        ($($variant:ident => $ty:ty),+ $(,)?) => {
            match format {
                $(cpal::SampleFormat::$variant => {
                    output_stream::<$ty, E>(device, config, staging_samples, pull, error_callback)
                })+
                other => Err(AppError::Player(format!("Unsupported sample format {other}"))),
            }
        };
    }

    // Every variant cpal 0.17 has, which is what rodio covered. The 24-bit pair is the one worth
    // naming: cpal's own config ordering ranks it, and a card offering nothing else would otherwise
    // fail every rung of the ladder and take the boot with it.
    arms! {
        F32 => f32,
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

fn output_stream<T, E>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    staging_samples: usize,
    mut pull: MixerPull,
    error_callback: E,
) -> Result<cpal::Stream, AppError>
where
    T: SizedSample + FromSample<Sample>,
    E: FnMut(cpal::StreamError) + Send + 'static,
{
    // A host handing over more than `staging_samples` allowed for grows this once and keeps it.
    let mut staging: Vec<Sample> = vec![0.0; staging_samples];
    device
        .build_output_stream::<T, _, _>(
            config,
            move |data, _| {
                if staging.len() < data.len() {
                    staging.resize(data.len(), 0.0);
                }
                let block = &mut staging[..data.len()];
                pull.fill(block);
                for (slot, sample) in data.iter_mut().zip(block.iter()) {
                    *slot = T::from_sample(*sample);
                }
            },
            error_callback,
            None,
        )
        .map_err(|e| AppError::Player(format!("Failed to open the audio stream: {e}")))
}

#[cfg(test)]
#[path = "tests/device_tests.rs"]
mod tests;
