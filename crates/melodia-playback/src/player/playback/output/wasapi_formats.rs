//! What a WASAPI endpoint takes for exclusive use, asked before anything initialises: the layouts a
//! claim tries and how each is spelled, the rates the device offers, and why a claim was refused.

use std::io;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;

use wasapi::{
    AudioClient, DeviceEnumerator, SampleType, ShareMode, WasapiError, WaveFormat,
    make_channelmasks,
};
use windows_sys::Win32::Media::Audio::{
    AUDCLNT_E_DEVICE_IN_USE, AUDCLNT_E_DEVICE_INVALIDATED, AUDCLNT_E_EXCLUSIVE_MODE_NOT_ALLOWED,
};

use melodia_audio::player::source::audio::{ChannelCount, SampleRate, Shape, SourceFormat};
use melodia_core::error::describe;

use super::claim::ClaimError;
use super::encode::DeviceFormat;
use super::rates::{self, RateSet};
use super::wasapi::ComApartment;

/// Every shape and format worth asking for, best first: the source's own channel count before a
/// wider one, since the mixer lays a narrower source on the first channels at no cost, and each
/// through the ladder's rungs WASAPI can declare.
pub(super) fn candidates(
    shape: Shape,
    source: SourceFormat,
    device_channels: u16,
) -> impl Iterator<Item = (Shape, DeviceFormat, WaveFormat)> {
    let widest = device_channels.max(shape.channels.get());
    (shape.channels.get()..=widest)
        .filter_map(ChannelCount::new)
        .map(move |channels| Shape { channels, ..shape })
        .flat_map(move |shape| {
            DeviceFormat::ladder(source).iter().filter_map(move |&format| {
                wave_format(format, shape).map(|wave| (shape, format, wave))
            })
        })
}

/// `format` at `shape` as WASAPI declares it, or `None` for `S24Low`: WASAPI's 24-in-32 is
/// MSB-aligned, which is `S24High`, and it has no way to declare the other.
pub(super) fn wave_format(format: DeviceFormat, shape: Shape) -> Option<WaveFormat> {
    let (container, valid, sample_type) = match format {
        DeviceFormat::S16 => (16, 16, SampleType::Int),
        DeviceFormat::S24Packed => (24, 24, SampleType::Int),
        DeviceFormat::S24Low => return None,
        DeviceFormat::S24High => (32, 24, SampleType::Int),
        DeviceFormat::S32 => (32, 32, SampleType::Int),
        DeviceFormat::F32 => (32, 32, SampleType::Float),
    };
    let rate = shape.rate.get() as usize;
    let channels = usize::from(shape.channels.get());
    Some(WaveFormat::new(container, valid, &sample_type, rate, channels, None))
}

/// The spelling of `wave` the device takes for exclusive use, or `None` where it takes none.
///
/// Asked plainly first: the respellings behind it swallow every error, so a busy or barred device
/// would read as one refusing the format.
pub(super) fn exclusive_spelling(
    client: &AudioClient,
    wave: &WaveFormat,
    id: &str,
) -> Result<Option<WaveFormat>, ClaimError> {
    let Err(e) = client.is_supported(wave, &ShareMode::Exclusive) else {
        return Ok(Some(wave.clone()));
    };
    if let Ok(refused) = device_refusal(e, id) {
        return Err(refused);
    }
    Ok(respelled(client, wave))
}

/// Another spelling of `wave` the device takes where it refused the one asked: the short header,
/// then each channel mask the crate suggests. The crate's own quirk walk, less one rung and the
/// mask `wave` already carried.
///
/// **The short header is never offered for integer PCM wider than 16 bits.** Windows defines it
/// for 8- and 16-bit PCM only, and a driver can take a wider one and misread it: the Realtek HD
/// Audio driver refuses 24-bit packed in the extensible header, takes it in the short one, and
/// drains it as 32-bit samples, so the track plays fast and garbled while every call succeeds.
/// Refused here instead, the claim moves on to the 24-in-32 rung, which that driver plays right.
fn respelled(client: &AudioClient, wave: &WaveFormat) -> Option<WaveFormat> {
    let takes =
        |spelling: &WaveFormat| client.is_supported(spelling, &ShareMode::Exclusive).is_ok();
    if wave.get_nchannels() <= 2
        && short_header_defined(wave)
        && let Ok(short) = wave.to_waveformatex()
        && takes(&short)
    {
        return Some(short);
    }
    make_channelmasks(usize::from(wave.get_nchannels()))
        .into_iter()
        // The crate's default mask is in the list, and the plain ask was made with it already.
        .filter(|&mask| mask != wave.wave_fmt.dwChannelMask)
        .find_map(|mask| {
            let mut masked = wave.clone();
            masked.wave_fmt.dwChannelMask = mask;
            takes(&masked).then_some(masked)
        })
}

/// Whether the short `WAVEFORMATEX` header can state `wave`: IEEE float, or integer PCM of at most
/// 16 bits.
pub(super) fn short_header_defined(wave: &WaveFormat) -> bool {
    matches!(wave.get_subformat(), Ok(SampleType::Float)) || wave.get_bitspersample() <= 16
}

/// Why no candidate took, or the device's own refusal where it won't be asked at all. WASAPI can't
/// be asked about a rate, a channel count and a format separately, so a rung the device takes at
/// its own rate is what says the rate was the problem.
pub(super) fn refusal(
    client: &AudioClient,
    mix: &WaveFormat,
    shape: Shape,
    source: SourceFormat,
    id: &str,
) -> ClaimError {
    let own_rate = SampleRate::new(mix.get_samplespersec()).filter(|&rate| rate != shape.rate);
    let takes_own_rate = match own_rate {
        Some(rate) => match takes_at(client, mix.get_nchannels(), Shape { rate, ..shape }, source, id)
        {
            Ok(takes) => takes,
            Err(refused) => return refused,
        },
        None => false,
    };
    if takes_own_rate {
        ClaimError::RateRefused { rate: shape.rate.get() }
    } else if shape.channels.get() > mix.get_nchannels() {
        ClaimError::ChannelsRefused { channels: shape.channels.get() }
    } else {
        ClaimError::FormatRefused { format: source }
    }
}

/// Whether the device takes `source` at `shape` in any layout a claim would ask for, or why it
/// refuses to be asked at all.
fn takes_at(
    client: &AudioClient,
    device_channels: u16,
    shape: Shape,
    source: SourceFormat,
    id: &str,
) -> Result<bool, ClaimError> {
    for (_, _, wave) in candidates(shape, source, device_channels) {
        if exclusive_spelling(client, &wave, id)?.is_some() {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The source format whose ladder is every rung, a float source converting to all of them.
const ANY_LAYOUT: SourceFormat = SourceFormat::F32;

/// The ladder's rungs the device takes in any layout WASAPI can declare, at any channel count a
/// claim for `shape` asks for.
///
/// Any layout rather than the source's own ladder, because the set answers for the tracks after
/// this one, whatever their format. A set wider than a later track's own costs that track a reopen
/// at most, where a narrower one could keep it converted that a fresh claim would play at its rate.
///
/// A device taken or unplugged part way ends the sweep with its refusal, rather than answering a
/// set short of the rates it had left to ask.
///
/// The rates are shared out across [`SWEEP_THREADS`] threads, `client`'s own among them. Proving a
/// rate missing takes every layout at every channel count, one ask each, and the device answers
/// asks from several threads at once.
pub(super) fn offered_rates(
    client: &AudioClient,
    mix: &WaveFormat,
    shape: Shape,
    id: &str,
) -> Result<RateSet, ClaimError> {
    let device_channels = mix.get_nchannels();
    let takes = |client: &AudioClient, rate| {
        takes_at(client, device_channels, Shape { rate, ..shape }, ANY_LAYOUT, id)
    };
    sweep_ladder(
        |rate| takes(client, rate),
        |queue| {
            let _com = ComApartment::enter();
            let reached = DeviceEnumerator::new()
                .and_then(|enumerator| enumerator.get_device(id))
                .and_then(|device| device.get_iaudioclient());
            match reached {
                Ok(own_client) => queue.drain(|rate| takes(&own_client, rate)),
                // Takes nothing off the queue, which the claim's own thread, holding a client
                // already, still empties.
                Err(e) => {
                    log::debug!(
                        "audio: a rate sweep helper couldn't reach the device: {}",
                        describe(&e)
                    );
                    Ok(Vec::new())
                }
            }
        },
    )
}

/// Threads a sweep asks from, the claim's own included. Past four the gain flattens, while each
/// helper still opens a client of its own.
const SWEEP_THREADS: usize = 4;

/// The [`rates::LADDER`] rates the device takes, asked through `own` on the calling thread and
/// through `helper` on each of the others, all working one queue.
///
/// A refusal anywhere stops every thread, and a helper that panics leaves the rate it held unasked,
/// so either way the sweep is asked again through `own` alone, whose answer stands. A device that
/// is busy, barred or gone refuses that run's first ask, while one refusing only asks made from
/// several threads at once still answers in full. A helper that couldn't be started leaves its
/// share of the queue to the threads that did start.
fn sweep_ladder(
    mut own: impl FnMut(SampleRate) -> Result<bool, ClaimError>,
    helper: impl Fn(&RateQueue) -> Result<Vec<u32>, ClaimError> + Sync,
) -> Result<RateSet, ClaimError> {
    let queue = RateQueue::default();
    let swept = thread::scope(|scope| {
        let helpers: Vec<_> = (1..SWEEP_THREADS)
            .filter_map(|_| {
                thread::Builder::new()
                    .name("wasapi-sweep".to_owned())
                    .spawn_scoped(scope, || helper(&queue))
                    .inspect_err(|e| {
                        log::debug!("audio: a rate sweep helper didn't start: {}", describe(e));
                    })
                    .ok()
            })
            .collect();
        let mut swept = vec![queue.drain(&mut own)];
        swept.extend(helpers.into_iter().map(|helper| {
            helper.join().unwrap_or_else(|_| {
                Err(ClaimError::io("A rate sweep thread panicked", io::Error::other("panicked")))
            })
        }));
        swept
    });
    match swept.into_iter().collect::<Result<Vec<Vec<u32>>, _>>() {
        Ok(swept) => Ok(swept.into_iter().flatten().collect()),
        Err(e) => {
            log::debug!("audio: a rate sweep across threads failed, asking on one: {}", describe(&e));
            Ok(RateQueue::default().drain(own)?.into_iter().collect())
        }
    }
}

/// The [`rates::LADDER`] rates one sweep has still to ask about, taken one at a time by every
/// thread it asks from.
#[derive(Default)]
struct RateQueue {
    next: AtomicUsize,
    /// Set by the first ask the device refused outright, which ends the sweep for every thread.
    refused: AtomicBool,
}

impl RateQueue {
    /// The rates `ask` finds the device taking, taken off the queue until it is empty or an ask
    /// anywhere was refused outright.
    fn drain(
        &self,
        mut ask: impl FnMut(SampleRate) -> Result<bool, ClaimError>,
    ) -> Result<Vec<u32>, ClaimError> {
        let mut offered = Vec::new();
        while !self.refused.load(Ordering::Relaxed) {
            let Some(&rate) = rates::LADDER.get(self.next.fetch_add(1, Ordering::Relaxed)) else {
                break;
            };
            let Some(rate) = SampleRate::new(rate) else { continue };
            match ask(rate) {
                Ok(true) => offered.push(rate.get()),
                Ok(false) => {}
                Err(refused) => {
                    self.refused.store(true, Ordering::Relaxed);
                    return Err(refused);
                }
            }
        }
        Ok(offered)
    }
}

pub(super) fn hresult(e: &WasapiError) -> Option<i32> {
    match e {
        WasapiError::Windows(e) => Some(e.code().0),
        _ => None,
    }
}

/// `e` as a refusal of the whole device, or `e` back where it only refuses what was asked of it.
pub(super) fn device_refusal(e: WasapiError, id: &str) -> Result<ClaimError, WasapiError> {
    Ok(match hresult(&e) {
        Some(AUDCLNT_E_DEVICE_IN_USE) => ClaimError::Busy(e.into()),
        Some(AUDCLNT_E_EXCLUSIVE_MODE_NOT_ALLOWED) => ClaimError::NotAllowed(e.into()),
        Some(AUDCLNT_E_DEVICE_INVALIDATED) => ClaimError::NotConnected { id: id.to_owned() },
        _ => return Err(e),
    })
}

pub(super) fn claim_error(context: &'static str, id: &str, e: WasapiError) -> ClaimError {
    device_refusal(e, id).unwrap_or_else(|e| ClaimError::io(context, e))
}

#[cfg(test)]
#[path = "tests/wasapi_formats_tests.rs"]
mod tests;
