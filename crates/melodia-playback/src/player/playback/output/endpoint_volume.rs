//! The device's own volume control, carrying the level while an exclusive claim hands it the
//! voices' samples at unity.
//!
//! **Only a control in hardware.** On an endpoint without one the audio engine applies the volume,
//! and an exclusive stream bypasses the engine, so setting it would change nothing heard. The
//! meter's hardware-support query answers that through `wasapi`'s safe half, so such a device never
//! reaches the calls below.
//!
//! **The level is set as Windows' own percentage**, so Melodia's slider and the one Windows shows
//! for the device read the same number. That percentage is Windows' audio taper over the device's
//! dB range rather than the voices' linear gain, so a position sounds a little different here than
//! with the voices carrying it; set in dB instead, the two sliders disagree at every position but
//! the ends.
//!
//! **The level is Windows' own volume for that device** for as long as the claim holds it, so the
//! system's slider and volume keys move it too. Those moves are watched for and handed back, so
//! Melodia's slider follows them. Polled rather than subscribed to: a subscription means
//! implementing `IAudioEndpointVolumeCallback` and reading the notification through a raw pointer,
//! more `unsafe` for no gain, on a COM worker thread rather than the writer's. A release puts back
//! what the device had before the claim regardless. That original outlives the claim until a
//! release has put it back, because Windows persists an endpoint's level: a device unplugged
//! mid-claim can't be restored, and replugged it reports Melodia's level as its own. A crash still
//! leaves it there.
//!
//! **Every call is made on `wasapi-out`**, which entered the MTA first, like the rest of the
//! backend. The interface comes from the `windows` crate: `wasapi` wraps none of it and keeps its
//! `IMMDevice` to itself.

use std::time::{Duration, Instant};

use parking_lot::Mutex;
use wasapi::{Device, WasapiError};
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::{IMMDeviceEnumerator, MMDeviceEnumerator};
use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance};
use windows::core::HSTRING;

use melodia_core::error::describe;

/// How often a moving volume reaches the device. Each set is a call into the audio service, made
/// on the thread with a period to refill, so a slider drag mustn't cost one per period.
const FOLLOW_INTERVAL: Duration = Duration::from_millis(20);

/// How often the device is asked whether something else moved it. Quick enough that Melodia's
/// slider follows the system's while it is dragged, and rare enough to cost the writer nothing.
const WATCH_INTERVAL: Duration = Duration::from_millis(250);

/// How far the device's level has to move to count as someone moving it: well under the whole
/// percent the system's slider steps in, and well over any rounding of a level Melodia set.
const MOVE_THRESHOLD: f32 = 0.001;

/// How near the device has to sit to a volume already for setting it to be skipped: under the half
/// percent a followed move comes back rounded to.
const FOLLOWED_TOLERANCE: f32 = 0.005;

/// Each device's level from before Melodia first claimed it, by endpoint id, until a release has
/// put it back.
static ORIGINAL_LEVELS: Mutex<Vec<(String, f32)>> = Mutex::new(Vec::new());

/// A claimed device's hardware volume, put back to its original level when dropped.
pub(super) struct EndpointVolume {
    control: IAudioEndpointVolume,
    id: String,
    /// The volume last set, and when, so an unchanged one costs no call.
    applied: Option<(f64, Instant)>,
    /// The level the device last reported, read back after each set so the device's own rounding
    /// of it never reads as a move.
    reported: f32,
    /// When the device was last asked whether something else moved it.
    watched: Instant,
}

impl EndpointVolume {
    /// The hardware control of the endpoint `id`, already set to `volume`, or `None` where it has
    /// none.
    ///
    /// # Errors
    ///
    /// What the device answered when asked for the control, its level or a new one.
    pub(super) fn take(
        device: &Device,
        id: &str,
        volume: f64,
    ) -> Result<Option<Self>, WasapiError> {
        if !device.get_audiometerinformation()?.query_hardware_support()?.volume {
            return Ok(None);
        }
        let control = activate(id)?;
        let original = level(&control)?;
        remember_original(id, original);
        let mut taken = Self {
            control,
            id: id.to_owned(),
            applied: None,
            reported: original,
            watched: Instant::now(),
        };
        taken.set(volume)?;
        Ok(Some(taken))
    }

    /// Set the device to `volume` where it has moved, at most once per [`FOLLOW_INTERVAL`]. A move
    /// held back lands on a later call, so it only ever arrives late.
    ///
    /// **A level the device already sits at is not written.** A system move handed back returns as
    /// the slider's rounding of it, and writing that would undo any move the system made meanwhile,
    /// which the read-back would then take as Melodia's own and never report.
    ///
    /// # Errors
    ///
    /// The device refusing the level, which the caller treats like a write it refused.
    pub(super) fn follow(&mut self, volume: f64) -> Result<(), WasapiError> {
        let due = self.applied.is_none_or(|(applied, at)| {
            applied.to_bits() != volume.to_bits() && at.elapsed() >= FOLLOW_INTERVAL
        });
        if !due {
            return Ok(());
        }
        if already_at(level_for(volume), self.reported) {
            self.applied = Some((volume, Instant::now()));
            return Ok(());
        }
        self.set(volume)
    }

    /// The level something outside Melodia moved the device to since the last call, looked for at
    /// most once per [`WATCH_INTERVAL`].
    ///
    /// # Errors
    ///
    /// The device refusing to say, which the caller treats like a write it refused.
    pub(super) fn take_move(&mut self) -> Result<Option<f64>, WasapiError> {
        if self.watched.elapsed() < WATCH_INTERVAL {
            return Ok(None);
        }
        self.watched = Instant::now();
        let now = level(&self.control)?;
        if (now - self.reported).abs() < MOVE_THRESHOLD {
            return Ok(None);
        }
        self.reported = now;
        Ok(Some(f64::from(now)))
    }

    fn set(&mut self, volume: f64) -> Result<(), WasapiError> {
        set_level(&self.control, level_for(volume))?;
        self.reported = level(&self.control)?;
        self.applied = Some((volume, Instant::now()));
        Ok(())
    }
}

impl Drop for EndpointVolume {
    fn drop(&mut self) {
        let mut originals = ORIGINAL_LEVELS.lock();
        let Some(index) = originals.iter().position(|(id, _)| *id == self.id) else { return };
        match set_level(&self.control, originals[index].1) {
            Ok(()) => {
                originals.swap_remove(index);
            }
            // Kept for the next claim of the device: one that has gone can't be set either.
            Err(e) => log::debug!("audio: the device's own volume wasn't put back: {}", describe(&e)),
        }
    }
}

/// Note `level` as the device's original, unless a claim that couldn't put one back already did.
fn remember_original(id: &str, level: f32) {
    let mut originals = ORIGINAL_LEVELS.lock();
    if !originals.iter().any(|(known, _)| known == id) {
        originals.push((id.to_owned(), level));
    }
}

/// `volume`, the slider's fraction, as the device level Windows shows as the same percentage.
/// Silence is the device's floor: the voices silence it already, and a rise from it should start
/// low rather than at whatever level it had before.
#[expect(
    clippy::cast_possible_truncation,
    reason = "a fraction held to 0..=1 loses nothing the device can resolve in f32"
)]
fn level_for(volume: f64) -> f32 {
    if volume.is_nan() {
        return 0.0;
    }
    volume.clamp(0.0, 1.0) as f32
}

/// Whether a device reporting `reported` already plays at `level`, as near as a whole percent on
/// the slider can say.
fn already_at(level: f32, reported: f32) -> bool {
    (level - reported).abs() < FOLLOWED_TOLERANCE
}

#[allow(
    unsafe_code,
    reason = "COM calls into the endpoint volume, which wasapi doesn't wrap. Made on a thread in the MTA; the only pointer handed over is a string held across its call."
)]
fn activate(id: &str) -> windows::core::Result<IAudioEndpointVolume> {
    // SAFETY: the CLSID is a static the crate declares, there is no outer object, and the calling
    // thread has entered COM.
    let enumerator: IMMDeviceEnumerator =
        unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)? };
    let id = HSTRING::from(id);
    // SAFETY: `id` is a NUL-terminated wide string that outlives the call, which keeps no pointer
    // to it.
    let device = unsafe { enumerator.GetDevice(&id)? };
    // SAFETY: no activation parameters are passed, and the interface comes back owned.
    unsafe { device.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None) }
}

/// The device's level, as the percentage Windows shows for it over 100.
#[allow(unsafe_code, reason = "COM call to GetMasterVolumeLevelScalar, which takes no pointer.")]
fn level(control: &IAudioEndpointVolume) -> windows::core::Result<f32> {
    // SAFETY: `control` is a live interface the smart pointer owns, and nothing is handed over.
    unsafe { control.GetMasterVolumeLevelScalar() }
}

/// Set the device's level as the percentage Windows shows for it over 100.
#[allow(
    unsafe_code,
    reason = "COM call to SetMasterVolumeLevelScalar with a null event context, which the API takes as none."
)]
fn set_level(control: &IAudioEndpointVolume, level: f32) -> windows::core::Result<()> {
    // SAFETY: a null event context is documented as none, and it is the only pointer passed.
    unsafe { control.SetMasterVolumeLevelScalar(level, std::ptr::null()) }
}

#[cfg(test)]
#[path = "tests/endpoint_volume_tests.rs"]
mod tests;
