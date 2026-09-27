//! Exclusive output on Windows: a WASAPI endpoint in exclusive, event-driven mode, opened at the
//! source's own shape.
//!
//! Exclusive mode hands the endpoint's buffer to the driver with no audio engine in between, so
//! every refusal is final: a rate, channel count or format the device lacks fails the claim rather
//! than being converted on the way. What reaches the device is [`encode`]'s output of the mixer's
//! block, and nothing else touches it.
//!
//! **Two things users will report, and neither is a bug.** While the claim holds, Melodia is gone
//! from the Windows volume mixer and every other application on that device falls silent. And a
//! device whose "Allow applications to take exclusive control of this device" box is cleared, in
//! its Advanced properties in the Sound control panel, refuses every claim; the panel names that.
//!
//! **Every COM call is made on the `wasapi-out` thread**, which opens the endpoint, answers the
//! open with what it agreed to, then feeds it until told to stop. No COM object crosses a thread,
//! and nothing depends on the apartment of whichever thread asked for the claim.
//!
//! [`encode`]: super::encode

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, SyncSender};
use std::thread::JoinHandle;
use std::time::Duration;

use wasapi::{
    AudioClient, AudioRenderClient, Device, DeviceEnumerator, Direction, Handle, SampleType,
    ShareMode, StreamMode, WasapiError, WaveFormat,
};
use windows_sys::Win32::Media::Audio::{
    AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED, AUDCLNT_E_DEVICE_IN_USE, AUDCLNT_E_DEVICE_INVALIDATED,
    AUDCLNT_E_EXCLUSIVE_MODE_NOT_ALLOWED, AUDCLNT_E_UNSUPPORTED_FORMAT,
};

use melodia_audio::player::source::audio::{
    ChannelCount, Sample, SampleRate, Shape, SourceFormat,
};
use melodia_core::error::describe;

use super::claim::ClaimError;
use super::device::Feed;
use super::encode::{self, DeviceFormat};
use super::{Negotiated, OutputDevice, OutputFormat, mmcss};

pub(super) const SUPPORTED: bool = true;

/// How much the writer hands the device per event. Short enough that a stop lands quickly, long
/// enough that the thread wakes rarely.
const PERIOD: Duration = Duration::from_millis(20);

/// Intel HD Audio controllers take buffers only in multiples of this many bytes, and in exclusive
/// mode the buffer is the controller's own.
const HDA_ALIGN_BYTES: u32 = 128;

/// How long the writer waits for the device to ask for more before checking it is still there.
const EVENT_WAIT_MS: u32 = 200;

const THREAD_NAME: &str = "wasapi-out";

/// Nothing outlives a stream here: an endpoint has no reservation to carry across a reopen.
pub(super) enum Claim {}

/// The live stream. Dropping it stops the writer, which stops the device and releases it.
pub(super) struct ExclusiveStream {
    writer: Option<JoinHandle<()>>,
    stop: Arc<AtomicBool>,
    negotiated: Negotiated,
}

impl ExclusiveStream {
    pub(super) fn negotiated(&self) -> Negotiated {
        self.negotiated.clone()
    }

    pub(super) fn into_claim(self) -> Option<Claim> {
        drop(self);
        None
    }
}

impl Drop for ExclusiveStream {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(writer) = self.writer.take() {
            join(writer);
        }
    }
}

/// Every active playback endpoint, the system default first.
pub(super) fn devices() -> Vec<OutputDevice> {
    let _com = ComApartment::enter();
    match DeviceEnumerator::new().and_then(|enumerator| endpoints(&enumerator)) {
        Ok(endpoints) => endpoints.into_iter().map(|endpoint| endpoint.device).collect(),
        Err(e) => {
            log::warn!("audio: failed to list the output devices: {}", describe(&e));
            Vec::new()
        }
    }
}

/// Claim `device`, or the system default where none is named, at exactly `shape` and a format
/// that holds `source`, and start feeding it from `feed`.
///
/// # Errors
///
/// A [`ClaimError`] naming why the device could not be had, which the caller falls back on.
pub(super) fn open(
    device: Option<&str>,
    shape: Shape,
    source: SourceFormat,
    feed: &Feed,
    _: Option<Claim>,
) -> Result<ExclusiveStream, ClaimError> {
    let request = Request { device: device.map(str::to_owned), shape, source };
    let stop = Arc::new(AtomicBool::new(false));
    let (answer_tx, answer_rx) = mpsc::sync_channel(1);
    let writer = {
        let feed = feed.clone();
        let stop = Arc::clone(&stop);
        std::thread::Builder::new()
            .name(THREAD_NAME.to_owned())
            .spawn(move || run(&request, &feed, &stop, &answer_tx))
            .map_err(|e| ClaimError::io("Failed to start the exclusive output thread", e))?
    };
    match answer_rx.recv() {
        Ok(Ok(negotiated)) => Ok(ExclusiveStream { writer: Some(writer), stop, negotiated }),
        Ok(Err(e)) => {
            join(writer);
            Err(e)
        }
        Err(e) => {
            join(writer);
            Err(ClaimError::io("The exclusive output thread stopped before it answered", e))
        }
    }
}

fn join(writer: JoinHandle<()>) {
    if writer.join().is_err() {
        log::warn!("audio: the exclusive output thread panicked");
    }
}

/// What an open asked for, owned so the writer thread can take it.
struct Request {
    device: Option<String>,
    shape: Shape,
    source: SourceFormat,
}

type Answer = Result<Negotiated, ClaimError>;

/// The whole life of the `wasapi-out` thread: claim, answer the opener, feed, release.
fn run(request: &Request, feed: &Feed, stop: &AtomicBool, answer: &SyncSender<Answer>) {
    let _com = ComApartment::enter();
    let _mmcss = mmcss::register_current_thread();
    let (session, negotiated) = match claim(request, feed) {
        Ok(claimed) => claimed,
        Err(e) => {
            // A refusal opened nothing, so an opener that has gone leaves nothing to release.
            let _ = answer.send(Err(e));
            return;
        }
    };
    // The opener blocks on this answer. One it can't take means nobody holds the stop flag.
    if answer.send(Ok(negotiated)).is_err() {
        return;
    }
    if let Err(e) = session.play(feed, stop) {
        log::warn!("audio: the exclusive output stopped: {}", describe(&e));
        feed.health.report_device_lost();
    }
}

fn claim(request: &Request, feed: &Feed) -> Result<(Session, Negotiated), ClaimError> {
    let enumerator = DeviceEnumerator::new()
        .map_err(|e| ClaimError::io("Failed to reach the audio devices", e))?;
    let endpoint = resolve(&enumerator, request.device.as_deref())?;
    let session = negotiate(&endpoint, request.shape, request.source)?;
    feed.pull.lock().reshape(session.shape);
    session
        .start()
        .map_err(|e| claim_error("Failed to start the audio device", &endpoint.device.id, e))?;
    let negotiated = Negotiated {
        device_name: Some(endpoint.device.name),
        shape: session.shape,
        format: OutputFormat::Exclusive(session.format),
        fallback: None,
        requested_period: Some(period_frames(request.shape.rate)),
        period: u32::try_from(session.frames).ok(),
    };
    Ok((session, negotiated))
}

/// An active playback endpoint, as the picker lists it and as an open resolves it.
struct Endpoint {
    device: OutputDevice,
    handle: Device,
}

/// Every active playback endpoint, the system default first, so that no device chosen and the
/// first one listed are the same endpoint, as they are for a sound card on Linux.
fn endpoints(enumerator: &DeviceEnumerator) -> Result<Vec<Endpoint>, WasapiError> {
    let default_id =
        enumerator.get_default_device(&Direction::Render).and_then(|device| device.get_id()).ok();
    let collection = enumerator.get_device_collection(&Direction::Render)?;
    let count = collection.get_nbr_devices()?;
    let mut found = Vec::with_capacity(count as usize);
    // Walked by index: the crate's iterator unwraps the count on every step.
    for index in 0..count {
        let handle = match collection.get_device_at_index(index) {
            Ok(handle) => handle,
            Err(e) => {
                log::debug!("audio: skipping an unreadable output device: {}", describe(&e));
                continue;
            }
        };
        let (Ok(id), Ok(name)) = (handle.get_id(), handle.get_friendlyname()) else { continue };
        found.push(Endpoint { device: OutputDevice { id, name }, handle });
    }
    let default = default_id.and_then(|id| found.iter().position(|e| e.device.id == id));
    if let Some(default) = default {
        found[..=default].rotate_right(1);
    }
    Ok(found)
}

fn resolve(enumerator: &DeviceEnumerator, device: Option<&str>) -> Result<Endpoint, ClaimError> {
    let mut endpoints = endpoints(enumerator)
        .map_err(|e| ClaimError::io("Failed to list the audio devices", e))?
        .into_iter();
    let found = match device {
        Some(id) => endpoints.find(|endpoint| endpoint.device.id == id),
        None => endpoints.next(),
    };
    found.ok_or_else(|| ClaimError::NotConnected { id: device.unwrap_or_default().to_owned() })
}

/// Initialise the first candidate the device takes.
fn negotiate(
    endpoint: &Endpoint,
    shape: Shape,
    source: SourceFormat,
) -> Result<Session, ClaimError> {
    let id = endpoint.device.id.as_str();
    let probe = endpoint
        .handle
        .get_iaudioclient()
        .map_err(|e| claim_error("Failed to open the audio device", id, e))?;
    let mix = probe
        .get_mixformat()
        .map_err(|e| claim_error("Failed to read the device's own format", id, e))?;
    for (candidate, format, wave) in candidates(shape, source, mix.get_nchannels()) {
        let Some(wave) = exclusive_spelling(&probe, &wave, id)? else { continue };
        match initialize(&endpoint.handle, &wave) {
            Ok(client) => {
                return Session::new(client, candidate, format)
                    .map_err(|e| claim_error("Failed to prepare the audio device", id, e));
            }
            // A driver can pass a format it then refuses, so this is a refusal of the format.
            Err(e) if hresult(&e) == Some(AUDCLNT_E_UNSUPPORTED_FORMAT) => {}
            Err(e) => return Err(claim_error("The device refused its configuration", id, e)),
        }
    }
    Err(refusal(&probe, &mix, shape, source))
}

/// Every shape and format worth asking for, best first: the source's own channel count before a
/// wider one, since the mixer lays a narrower source on the first channels at no cost, and each
/// through the ladder's rungs WASAPI can declare.
fn candidates(
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
fn wave_format(format: DeviceFormat, shape: Shape) -> Option<WaveFormat> {
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
/// Asked plainly first: the quirk walk behind it swallows every error, so a busy or barred device
/// would read as one refusing the format.
fn exclusive_spelling(
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
    Ok(client.is_supported_exclusive_with_quirks(wave).ok())
}

/// A fresh client initialised for exclusive, event-driven use in `wave`.
///
/// A driver that wants a buffer the period didn't land on refuses with
/// `AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED`. The client that refused knows the size it wanted, but a
/// client initialises once, so the retry takes a fresh one.
fn initialize(device: &Device, wave: &WaveFormat) -> Result<AudioClient, WasapiError> {
    let mut client = device.get_iaudioclient()?;
    let period = client.calculate_aligned_period_near(hns(PERIOD), Some(HDA_ALIGN_BYTES), wave)?;
    let mode = StreamMode::EventsExclusive { period_hns: period };
    let Err(e) = client.initialize_client(wave, &Direction::Render, &mode) else {
        return Ok(client);
    };
    if hresult(&e) != Some(AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED) {
        return Err(e);
    }
    let aligned = wasapi::calculate_period_100ns(
        i64::from(client.get_buffer_size()?),
        i64::from(wave.get_samplespersec()),
    );
    let mut client = device.get_iaudioclient()?;
    client.initialize_client(
        wave,
        &Direction::Render,
        &StreamMode::EventsExclusive { period_hns: aligned },
    )?;
    Ok(client)
}

/// Why no candidate took. WASAPI can't be asked about a rate, a channel count and a format
/// separately, so a rung the device takes at its own rate is what says the rate was the problem.
fn refusal(
    client: &AudioClient,
    mix: &WaveFormat,
    shape: Shape,
    source: SourceFormat,
) -> ClaimError {
    let own_rate = SampleRate::new(mix.get_samplespersec()).filter(|&rate| rate != shape.rate);
    let takes_own_rate = own_rate.is_some_and(|rate| {
        candidates(Shape { rate, ..shape }, source, mix.get_nchannels())
            .any(|(_, _, wave)| client.is_supported_exclusive_with_quirks(&wave).is_ok())
    });
    if takes_own_rate {
        ClaimError::RateRefused { rate: shape.rate.get() }
    } else if shape.channels.get() > mix.get_nchannels() {
        ClaimError::ChannelsRefused { channels: shape.channels.get() }
    } else {
        ClaimError::FormatRefused { bits: source.bits }
    }
}

fn hresult(e: &WasapiError) -> Option<i32> {
    match e {
        WasapiError::Windows(e) => Some(e.code().0),
        _ => None,
    }
}

/// `e` as a refusal of the whole device, or `e` back where it only refuses what was asked of it.
fn device_refusal(e: WasapiError, id: &str) -> Result<ClaimError, WasapiError> {
    Ok(match hresult(&e) {
        Some(AUDCLNT_E_DEVICE_IN_USE) => ClaimError::Busy(e.into()),
        Some(AUDCLNT_E_EXCLUSIVE_MODE_NOT_ALLOWED) => ClaimError::NotAllowed(e.into()),
        Some(AUDCLNT_E_DEVICE_INVALIDATED) => ClaimError::NotConnected { id: id.to_owned() },
        _ => return Err(e),
    })
}

fn claim_error(context: &'static str, id: &str, e: WasapiError) -> ClaimError {
    device_refusal(e, id).unwrap_or_else(|e| ClaimError::io(context, e))
}

/// An initialised exclusive client and what it was opened with.
struct Session {
    client: AudioClient,
    render: AudioRenderClient,
    event: Handle,
    shape: Shape,
    format: DeviceFormat,
    /// Frames per buffer, which in exclusive event mode is what every event asks for.
    frames: usize,
}

impl Session {
    fn new(client: AudioClient, shape: Shape, format: DeviceFormat) -> Result<Self, WasapiError> {
        let event = client.set_get_eventhandle()?;
        let render = client.get_audiorenderclient()?;
        let frames = client.get_buffer_size()? as usize;
        Ok(Self { client, render, event, shape, format, frames })
    }

    fn samples_per_buffer(&self) -> usize {
        self.frames * usize::from(self.shape.channels.get())
    }

    fn start(&self) -> Result<(), WasapiError> {
        // Silence rather than the mixer's first block: the resync hold is laid in after the open
        // returns, and a block pulled now would play ahead of it.
        let silence = vec![0_u8; self.samples_per_buffer() * self.format.bytes_per_sample()];
        self.render.write_to_device(self.frames, &silence, None)?;
        self.client.start_stream()
    }

    /// Hand the device one buffer per event until told to stop.
    ///
    /// Nothing here logs: the health counters are how this loop reports, and the one error it
    /// returns is logged once, on the way out.
    fn play(&self, feed: &Feed, stop: &AtomicBool) -> Result<(), WasapiError> {
        let mut block: Vec<Sample> = vec![0.0; self.samples_per_buffer()];
        let mut bytes = Vec::with_capacity(block.len() * self.format.bytes_per_sample());
        while !stop.load(Ordering::Relaxed) {
            if self.event.wait_for_event(EVENT_WAIT_MS).is_err() {
                // An endpoint that has gone can stop signalling rather than fail a call, so a
                // quiet wait asks whether it is still there.
                self.client.get_current_padding()?;
                continue;
            }
            feed.fill(&mut block);
            encode::encode(&block, self.format, &mut bytes);
            self.render.write_to_device(self.frames, &bytes, None)?;
        }
        Ok(())
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if let Err(e) = self.client.stop_stream() {
            // A device that has gone can't be stopped either, and it is released regardless.
            log::debug!("audio: the exclusive output didn't stop cleanly: {}", describe(&e));
        }
    }
}

/// COM on the calling thread while this lives, balanced only where `enter` succeeded: a thread
/// already in a single-threaded apartment is refused, stays as it was, and reaches the endpoints
/// from there.
struct ComApartment {
    entered: bool,
}

impl ComApartment {
    fn enter() -> Self {
        Self { entered: wasapi::initialize_mta().is_ok() }
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        if self.entered {
            wasapi::deinitialize();
        }
    }
}

/// [`PERIOD`] in frames at `rate`.
fn period_frames(rate: SampleRate) -> u32 {
    let frames = u128::from(rate.get()) * PERIOD.as_millis() / 1_000;
    u32::try_from(frames).unwrap_or(u32::MAX)
}

/// `duration` in the 100 ns units WASAPI counts periods in.
fn hns(duration: Duration) -> i64 {
    i64::try_from(duration.as_nanos() / 100).unwrap_or(i64::MAX)
}

#[cfg(test)]
#[path = "tests/wasapi_tests.rs"]
mod tests;
