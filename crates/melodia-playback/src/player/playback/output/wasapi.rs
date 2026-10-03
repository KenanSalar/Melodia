//! Exclusive output on Windows: a WASAPI endpoint in exclusive mode, opened at the source's own
//! shape. Event-driven by default; polled where the user asks, for the USB drivers that stutter
//! under events.
//!
//! Exclusive mode hands the endpoint's buffer to the driver with no audio engine in between, so
//! nothing converts on the way: a channel count or format the device lacks fails the claim, and so
//! does a rate, unless the request lets the voices convert to one the device has. What reaches the
//! device is [`encode`]'s output of the mixer's block, and nothing else touches it.
//!
//! **Two things users will report, and neither is a bug.** While the claim holds, Melodia is gone
//! from the Windows volume mixer, and every other application on that device loses it: some fall
//! silent, others move to another output. And a device whose "Allow applications to take exclusive
//! control of this device" box is cleared, in its Advanced properties in the Sound control panel,
//! refuses every claim; the panel names that.
//!
//! **A claim's COM calls are made on the `wasapi-out` thread**, which opens the endpoint, answers
//! the open with what it agreed to, then feeds it until told to stop, and on the helpers its rate
//! sweep starts, each in an apartment and with a client of its own. No COM object crosses a
//! thread, and nothing depends on the apartment of whichever thread asked for the claim.
//!
//! [`encode`]: super::encode

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, SyncSender};
use std::thread::JoinHandle;
use std::time::Duration;

use wasapi::{
    AudioClient, AudioClock, AudioRenderClient, Device, DeviceEnumerator, DeviceState, Direction,
    Handle, StreamMode, WasapiError, WaveFormat,
};
use windows_sys::Win32::Foundation::ERROR_NOT_FOUND;
use windows_sys::Win32::Media::Audio::{
    AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED, AUDCLNT_E_UNSUPPORTED_FORMAT,
};

use melodia_audio::player::source::audio::{
    Sample, SampleRate, Shape, frames_in, frames_to_duration,
};
use melodia_core::error::describe;

use super::claim::ClaimError;
use super::device::Feed;
use super::dither::Dither;
use super::encode::{self, DeviceFormat};
use super::endpoint_volume::{self, EndpointVolume};
use super::rates::RateSet;
use super::wasapi_clock::{Clock, ClockReading, StallWatch, clock_duration, from_hns, hns};
use super::wasapi_formats::{
    candidates, claim_error, exclusive_spelling, hresult, offered_rates, refusal,
};
use super::wasapi_offered::{self, ProbeKey};
use super::{
    Drive, ExclusiveRequest, ExclusiveTuning, Negotiated, OutputDevice, OutputFormat, RateFallback,
    hardware_volume, mmcss, rates,
};

pub(super) const SUPPORTED: bool = true;

pub(super) const POLLING: bool = true;

pub(super) const HARDWARE_VOLUME: bool = true;

pub(super) const RATE_FALLBACK: bool = true;

/// An endpoint's id is the one cpal opens it by, so the picker's list serves shared output too.
pub(super) const SHARED_DEVICE: bool = true;

/// Intel HD Audio controllers take buffers only in multiples of this many bytes, and in exclusive
/// mode the buffer is the controller's own.
const HDA_ALIGN_BYTES: u32 = 128;

/// How long the writer waits for the device to ask for more before checking it is still there.
/// Above `ExclusiveTuning::MAX_PERIOD`, so a quiet wait is never just a long period.
const EVENT_WAIT_MS: u32 = 200;

/// Periods a polled buffer holds, which is how late a timer wake may be before an underrun.
/// Event mode has none of this slack: its buffer is one period.
const POLLED_PERIODS: i64 = 4;

/// What the device enumerator answers for an id it has no endpoint for.
const ENDPOINT_NOT_FOUND: i32 = windows::core::HRESULT::from_win32(ERROR_NOT_FOUND).0;

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

    pub(super) fn hardware_volume(&self) -> bool {
        self.negotiated.hardware_volume
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

/// Claim the requested device, or the system default where none is named, at the source's shape,
/// or at another of the device's rates where the request allows, in a format that holds the
/// source, and start feeding it from `feed`.
///
/// # Errors
///
/// A [`ClaimError`] naming why the device could not be had, which the caller falls back on.
pub(super) fn open(
    request: &ExclusiveRequest,
    feed: &Feed,
    _: Option<Claim>,
) -> Result<ExclusiveStream, ClaimError> {
    let request = request.clone();
    let stop = Arc::new(AtomicBool::new(false));
    let (answer_tx, answer_rx) = mpsc::sync_channel(1);
    let writer = {
        let feed = feed.clone();
        let stop = Arc::clone(&stop);
        std::thread::Builder::new()
            .name("wasapi-out".to_owned())
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

type Answer = Result<Negotiated, ClaimError>;

/// The whole life of the `wasapi-out` thread: claim, answer the opener, feed, release.
fn run(request: &ExclusiveRequest, feed: &Feed, stop: &AtomicBool, answer: &SyncSender<Answer>) {
    let _com = ComApartment::enter();
    let _mmcss = mmcss::register_current_thread();
    let (mut session, negotiated) = match claim(request, feed) {
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
    // Info rather than a warning: a device that went away is recovered from, and only a recovery
    // that fails is something to warn about.
    if let Err(e) = session.play(feed, stop) {
        log::info!("audio: the exclusive output stopped: {}", describe(&e));
        feed.health.report_device_lost();
    }
}

fn claim(request: &ExclusiveRequest, feed: &Feed) -> Result<(Session, Negotiated), ClaimError> {
    let enumerator = DeviceEnumerator::new()
        .map_err(|e| ClaimError::io("Failed to reach the audio devices", e))?;
    let endpoint = resolve(&enumerator, request.device.as_deref())?;
    let mut session = negotiate(&endpoint, request)?;
    // Before the start, so the first sample already plays at the level and a start that fails
    // still puts the device's own back.
    if request.hardware_volume {
        let taken =
            endpoint_volume::take(&endpoint.handle, &endpoint.device.id, feed.volume.load());
        session.volume = hardware_volume::or_software(&endpoint.device.name, taken);
    }
    feed.pull.lock().reshape(session.shape);
    session
        .start()
        .map_err(|e| claim_error("Failed to start the audio device", &endpoint.device.id, e))?;
    // After the start, so a claim that failed leaves the slider where the user had it.
    if let Some(level) = session.volume.as_ref().and_then(EndpointVolume::lowered) {
        feed.external_volume.report(level);
    }
    let negotiated = Negotiated {
        device_name: Some(endpoint.device.name),
        shape: session.shape,
        format: OutputFormat::Exclusive(session.format),
        fallback: None,
        hardware_volume: session.volume.is_some(),
        device_level: None,
        offered: session.offered,
        requested_period: u32::try_from(frames_in(request.tuning.period, session.shape.rate)).ok(),
        period: u32::try_from(session.period_frames).ok(),
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

/// The endpoint a claim is aimed at: the chosen one looked up by id, or the system default.
///
/// By id rather than through [`endpoints`], which reads every device's name to find one. Only a
/// device the system doesn't know or doesn't have active is `NotConnected`, the two cases the
/// listing would leave out. Any other failure read as a disconnect would have the reclaim poll
/// retrying a device that is plugged in, since the listing still shows it.
fn resolve(enumerator: &DeviceEnumerator, device: Option<&str>) -> Result<Endpoint, ClaimError> {
    let Some(id) = device else {
        return endpoints(enumerator)
            .map_err(|e| ClaimError::io("Failed to list the audio devices", e))?
            .into_iter()
            .next()
            .ok_or_else(|| ClaimError::NotConnected { id: String::new() });
    };
    let not_connected = || ClaimError::NotConnected { id: id.to_owned() };
    let handle = match enumerator.get_device(id) {
        Ok(handle) => handle,
        Err(e) if hresult(&e) == Some(ENDPOINT_NOT_FOUND) => return Err(not_connected()),
        Err(e) => return Err(ClaimError::io("Failed to reach the chosen audio device", e)),
    };
    let state = handle
        .get_state()
        .map_err(|e| ClaimError::io("Failed to read the chosen audio device's state", e))?;
    if state != DeviceState::Active {
        return Err(not_connected());
    }
    let name = handle
        .get_friendlyname()
        .map_err(|e| ClaimError::io("Failed to read the chosen audio device's name", e))?;
    Ok(Endpoint { device: OutputDevice { id: id.to_owned(), name }, handle })
}

/// Initialise the device for `request`, noting the rates it offers where the claim may resample.
fn negotiate(endpoint: &Endpoint, request: &ExclusiveRequest) -> Result<Session, ClaimError> {
    let id = endpoint.device.id.as_str();
    let probe = endpoint
        .handle
        .get_iaudioclient()
        .map_err(|e| claim_error("Failed to open the audio device", id, e))?;
    let mix = probe
        .get_mixformat()
        .map_err(|e| claim_error("Failed to read the device's own format", id, e))?;
    // Asked before anything initialises, so nothing of ours holds the device while it answers.
    let offered = match request.rate_fallback {
        RateFallback::Resample => {
            let key = ProbeKey {
                device: id.to_owned(),
                from_channels: request.shape.channels.get(),
                device_channels: mix.get_nchannels(),
            };
            Some(wasapi_offered::remembered(key, || {
                // A busy or barred device fails every probe alike, and is retried at each track
                // start, so the claim's own first ask goes ahead of the sweep and refuses in one
                // call.
                let first = candidates(request.shape, request.format, mix.get_nchannels()).next();
                if let Some((_, _, wave)) = first {
                    exclusive_spelling(&probe, &wave, id)?;
                }
                offered_rates(&probe, &mix, request.shape, id)
            })?)
        }
        RateFallback::Shared => None,
    };
    let mut session = open_session(endpoint, &probe, &mix, request, offered)?;
    session.offered = offered;
    Ok(session)
}

/// Initialise the first candidate the device takes at the source's rate, or, where the device lacks
/// that rate and the request allows, at the one [`rates::device_rate_for`] picks out of the others
/// it `offered`, and then at the device's own.
///
/// `offered` answers for any layout rather than this source's, so the pick can be a rate the device
/// takes only in a format this source can't use. Nothing opens there, and the device's own rate
/// stands behind it, as it does for a driver that passes a rate and then won't initialise it.
///
/// It also answers for every layout the source's rate would be asked in, so a rate it lacks isn't
/// asked again: each of those asks would be refused, one candidate at a time. [`wasapi_offered`]
/// says what an answer gone stale costs.
fn open_session(
    endpoint: &Endpoint,
    probe: &AudioClient,
    mix: &WaveFormat,
    request: &ExclusiveRequest,
    offered: Option<RateSet>,
) -> Result<Session, ClaimError> {
    let ExclusiveRequest { shape, format: source, tuning, rate_fallback, .. } = *request;
    let id = endpoint.device.id.as_str();
    let device_channels = mix.get_nchannels();
    let own_rate = SampleRate::new(mix.get_samplespersec());
    let at_source_rate = candidates(shape, source, device_channels);
    if !source_rate_lacking(shape.rate, own_rate, offered)
        && let Some(session) = open_first(endpoint, probe, at_source_rate, tuning)?
    {
        return Ok(session);
    }
    let refused = refusal(probe, mix, shape, source, id);
    let converts = rate_fallback == RateFallback::Resample
        && matches!(refused, ClaimError::RateRefused { .. });
    if !converts {
        return Err(refused);
    }
    for rate in retry_rates(shape.rate, own_rate, offered) {
        let at_rate = candidates(Shape { rate, ..shape }, source, device_channels);
        if let Some(session) = open_first(endpoint, probe, at_rate, tuning)? {
            return Ok(session);
        }
    }
    Err(refused)
}

/// Whether the rates the device `offered` already say it lacks the `source` rate. Only where its
/// `own` rate is another, which is where [`refusal`] asks instead, so a device that won't be asked
/// at all, busy or barred, still says so rather than reading as one refusing the format.
fn source_rate_lacking(
    source: SampleRate,
    own: Option<SampleRate>,
    offered: Option<RateSet>,
) -> bool {
    own.is_some_and(|own| own != source)
        && offered.is_some_and(|offered| offered.contains(source.get()) == Some(false))
}

/// The rates a claim refused at the `source` rate tries next: the one [`rates::device_rate_for`]
/// picks out of the others `offered`, then the device's `own`.
///
/// A driver can pass a rate it then won't initialise. The mix rate is the one the audio engine
/// already runs the device at, so it stands behind the pick. The source's rate is left out even
/// where it was offered: it has just been refused.
fn retry_rates(
    source: SampleRate,
    own: Option<SampleRate>,
    offered: Option<RateSet>,
) -> impl Iterator<Item = SampleRate> {
    let picked = offered.and_then(|offered| {
        let others: RateSet = offered.rates().filter(|&rate| rate != source.get()).collect();
        rates::device_rate_for(source, others)
    });
    picked.into_iter().chain(own.filter(|&own| Some(own) != picked))
}

/// Initialise the first of `wanted` the device takes, or `None` where it takes none of them.
fn open_first(
    endpoint: &Endpoint,
    probe: &AudioClient,
    wanted: impl Iterator<Item = (Shape, DeviceFormat, WaveFormat)>,
    tuning: ExclusiveTuning,
) -> Result<Option<Session>, ClaimError> {
    let id = endpoint.device.id.as_str();
    for (shape, format, wave) in wanted {
        let Some(wave) = exclusive_spelling(probe, &wave, id)? else { continue };
        match initialize(&endpoint.handle, &wave, hns(tuning.period), tuning.drive) {
            Ok((client, period_hns)) => {
                return Session::new(client, shape, format, tuning.drive, period_hns)
                    .map(Some)
                    .map_err(|e| claim_error("Failed to prepare the audio device", id, e));
            }
            // A driver can pass a format it then refuses, so this is a refusal of the format.
            Err(e) if hresult(&e) == Some(AUDCLNT_E_UNSUPPORTED_FORMAT) => {}
            Err(e) => return Err(claim_error("The device refused its configuration", id, e)),
        }
    }
    Ok(None)
}

/// A fresh client initialised for exclusive use in `wave`, driven as `drive` asks at a period near
/// `period_hns`, and the period it took.
///
/// The crate rounds the period up to the device's minimum. A driver that wants a buffer the period
/// didn't land on still refuses with `AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED`, which Windows returns
/// only to an event-driven claim, whose buffer is its period. The client that refused knows the
/// size it wanted, but a client initialises once, so the retry takes a fresh one, and the refused
/// one goes back first as Microsoft's recovery sequence has it.
fn initialize(
    device: &Device,
    wave: &WaveFormat,
    period_hns: i64,
    drive: Drive,
) -> Result<(AudioClient, i64), WasapiError> {
    let mut client = device.get_iaudioclient()?;
    let period = client.calculate_aligned_period_near(period_hns, Some(HDA_ALIGN_BYTES), wave)?;
    let Err(e) = client.initialize_client(wave, &Direction::Render, &stream_mode(drive, period))
    else {
        return Ok((client, period));
    };
    if hresult(&e) != Some(AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED) {
        return Err(e);
    }
    let aligned_period = wasapi::calculate_period_100ns(
        i64::from(client.get_buffer_size()?),
        i64::from(wave.get_samplespersec()),
    );
    drop(client);
    let mut client = device.get_iaudioclient()?;
    client.initialize_client(wave, &Direction::Render, &stream_mode(drive, aligned_period))?;
    Ok((client, aligned_period))
}

/// Exclusive use at `period_hns`: a buffer of one period under events, a few under polling, where
/// the timer's lateness is what the slack is for.
fn stream_mode(drive: Drive, period_hns: i64) -> StreamMode {
    match drive {
        Drive::Events => StreamMode::EventsExclusive { period_hns },
        Drive::Polling => StreamMode::PollingExclusive {
            period_hns,
            buffer_duration_hns: period_hns.saturating_mul(POLLED_PERIODS),
        },
    }
}

/// An initialised exclusive client and what it was opened with.
struct Session {
    /// The device's own volume where it carries the level. First, so it is put back once the
    /// stream has stopped but before the client goes: released first, other applications could
    /// resume on the device at Melodia's level.
    volume: Option<EndpointVolume>,
    client: AudioClient,
    render: AudioRenderClient,
    pacing: Pacing,
    /// The device's own count of what it has played, and the ticks per second it counts in.
    clock: AudioClock,
    clock_hz: u64,
    shape: Shape,
    format: DeviceFormat,
    buffer_frames: usize,
    /// What the device takes per period: the whole buffer under events, a slice of it polled.
    period_frames: usize,
    /// The rates the device offered, where the claim may resample.
    offered: Option<RateSet>,
}

/// What wakes the writer.
enum Pacing {
    /// The device's own event, signalled each time its one-period buffer wants refilling.
    Events(Handle),
    /// A timer at half a period, so the buffer is topped up at least twice per period.
    Polling(Duration),
}

impl Session {
    fn new(
        client: AudioClient,
        shape: Shape,
        format: DeviceFormat,
        drive: Drive,
        period_hns: i64,
    ) -> Result<Self, WasapiError> {
        let period = from_hns(period_hns);
        let pacing = match drive {
            Drive::Events => Pacing::Events(client.set_get_eventhandle()?),
            Drive::Polling => Pacing::Polling(period / 2),
        };
        let render = client.get_audiorenderclient()?;
        let clock = client.get_audioclock()?;
        let clock_hz = clock.get_frequency()?.max(1);
        let buffer_frames = client.get_buffer_size()? as usize;
        let period_frames = match drive {
            Drive::Events => buffer_frames,
            Drive::Polling => usize::try_from(frames_in(period, shape.rate)).unwrap_or(usize::MAX),
        };
        Ok(Self {
            volume: None,
            client,
            render,
            pacing,
            clock,
            clock_hz,
            shape,
            format,
            buffer_frames,
            period_frames,
            offered: None,
        })
    }

    fn samples_per_buffer(&self) -> usize {
        self.buffer_frames * usize::from(self.shape.channels.get())
    }

    fn start(&self) -> Result<(), WasapiError> {
        // The device wants a filled buffer before it starts. Silence rather than the mixer's first
        // block keeps every pull in `play`.
        let silence = vec![0_u8; self.samples_per_buffer() * self.format.bytes_per_sample()];
        self.render.write_to_device(self.buffer_frames, &silence, None)?;
        self.client.start_stream()
    }

    /// Hand the device whatever it has room for, each time it has some, until told to stop.
    ///
    /// Nothing here logs: the health counters are how this loop reports, and the one error it
    /// returns is logged once, on the way out. A volume control that stops taking or reporting the
    /// level ends the stream like a write refused, and the claim that follows decides again whether
    /// to use it.
    fn play(&mut self, feed: &Feed, stop: &AtomicBool) -> Result<(), WasapiError> {
        let channels = usize::from(self.shape.channels.get());
        let mut block: Vec<Sample> = vec![0.0; self.samples_per_buffer()];
        let mut dither = Dither::default();
        let mut bytes = Vec::with_capacity(block.len() * self.format.bytes_per_sample());
        // The priming silence `start` wrote is on the device's clock too.
        let mut written = self.buffer_frames as u64;
        let mut stalls = StallWatch::default();
        while !stop.load(Ordering::Relaxed) {
            let Some(frames) = self.wait_for_room()? else { continue };
            let pulled = &mut block[..frames * channels];
            feed.fill(pulled);
            dither.quantize(pulled, self.format);
            encode::encode(pulled, self.format, &mut bytes);
            self.render.write_to_device(frames, &bytes, None)?;
            written += frames as u64;
            if let Some(volume) = &mut self.volume {
                volume.sync(feed)?;
            }
            // A clock that won't answer leaves the last reading standing: it is the ear's
            // position that suffers, not the audio, so it doesn't end the stream.
            if let Ok((played, at)) = self.clock.get_position() {
                let clock = stalls.read(ClockReading { played, at }, self.clock_hz);
                feed.report_lead(self.unplayed(written, played));
                if clock != Clock::Moving {
                    feed.health.record_xrun();
                }
                // A polled stream tops up whatever room it finds, so it recovers by itself.
                if clock == Clock::Stuck && matches!(self.pacing, Pacing::Events(_)) {
                    self.restart()?;
                    written = self.buffer_frames as u64;
                    stalls.restart();
                }
            }
        }
        Ok(())
    }

    /// Stop the device, drop what it holds and start it again from silence.
    ///
    /// An event-driven stream needs this once a stall leaves it stuck. Its event resets itself, so
    /// the signals that landed while the writer was stalled come back as one wake, and the half of
    /// the buffer they stood for is never refilled. Left running, the device runs dry every period,
    /// which sounds as a buzz until the stream is reopened. The reset also sets its clock back to
    /// zero. Restarts that don't bring the clock back stop being asked for, as [`StallWatch`] says.
    fn restart(&self) -> Result<(), WasapiError> {
        self.client.stop_stream()?;
        self.client.reset_stream()?;
        self.start()
    }

    /// How long the `written` frames outlast the `played` clock ticks. The device's own latency
    /// past its clock is not in it: the crate doesn't expose `GetStreamLatency`.
    fn unplayed(&self, written: u64, played: u64) -> Duration {
        frames_to_duration(written, self.shape.rate)
            .saturating_sub(clock_duration(played, self.clock_hz))
    }

    /// Frames the device has room for once the writer wakes, or `None` where it woke to none.
    fn wait_for_room(&self) -> Result<Option<usize>, WasapiError> {
        match &self.pacing {
            Pacing::Events(event) => {
                if event.wait_for_event(EVENT_WAIT_MS).is_err() {
                    // An endpoint that has gone can stop signalling rather than fail a call, so a
                    // quiet wait asks whether it is still there.
                    self.client.get_current_padding()?;
                    return Ok(None);
                }
                Ok(Some(self.buffer_frames))
            }
            // A device that has gone fails the padding query, which ends the writer as a loss. The
            // crate's room query asks the buffer size again on every call, and it never changes.
            Pacing::Polling(interval) => {
                std::thread::sleep(*interval);
                let queued = self.client.get_current_padding()? as usize;
                let room = self.buffer_frames.saturating_sub(queued);
                Ok((room > 0).then_some(room))
            }
        }
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
pub(super) struct ComApartment {
    entered: bool,
}

impl ComApartment {
    pub(super) fn enter() -> Self {
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

#[cfg(test)]
#[path = "tests/wasapi_tests.rs"]
mod tests;
