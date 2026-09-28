//! The endpoint's own volume control, as the [`VolumeControl`] a WASAPI claim carries the level on.
//! Taking it, following it and giving it back is [`hardware_volume`]'s.
//!
//! **Only a control in hardware.** On an endpoint without one the audio engine applies the volume,
//! and an exclusive stream bypasses the engine, so setting it would change nothing heard. The
//! meter's hardware-support query answers that through `wasapi`'s safe half, so such a device never
//! reaches the calls below.
//!
//! **The level is Windows' own percentage**, not dB. That percentage is Windows' audio taper over
//! the device's dB range rather than the voices' linear gain, so a position sounds a little
//! different here than with the voices carrying it; set in dB instead, the two sliders disagree at
//! every position but the ends.
//!
//! **Polled rather than subscribed to**, for the moves the system makes: a subscription means
//! implementing `IAudioEndpointVolumeCallback` and reading the notification through a raw pointer,
//! more `unsafe` for no gain, on a COM worker thread rather than the writer's.
//!
//! **Every call is made on `wasapi-out`**, which entered the MTA first, like the rest of the
//! backend. The interface comes from the `windows` crate: `wasapi` wraps none of it and keeps its
//! `IMMDevice` to itself.
//!
//! [`hardware_volume`]: super::hardware_volume

use wasapi::{Device, WasapiError};
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::{IMMDeviceEnumerator, MMDeviceEnumerator};
use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance};
use windows::core::HSTRING;

use super::hardware_volume::{HardwareVolume, VolumeControl};

/// A claimed endpoint's hardware volume, put back to its original level when dropped.
pub(super) type EndpointVolume = HardwareVolume<EndpointControl>;

pub(super) struct EndpointControl(IAudioEndpointVolume);

/// The hardware control of the endpoint `id`, set to `volume` unless the system had it quieter, or
/// `None` where it has none.
///
/// # Errors
///
/// What the device answered when asked for the control, its level or a new one.
pub(super) fn take(
    device: &Device,
    id: &str,
    volume: f64,
) -> Result<Option<EndpointVolume>, WasapiError> {
    if !device.get_audiometerinformation()?.query_hardware_support()?.volume {
        return Ok(None);
    }
    let control = EndpointControl(activate(id)?);
    Ok(Some(HardwareVolume::take(control, id, volume)?))
}

impl VolumeControl for EndpointControl {
    type Error = windows::core::Error;

    /// The device's level, as the percentage Windows shows for it over 100.
    #[allow(unsafe_code, reason = "COM call to GetMasterVolumeLevelScalar, which takes no pointer.")]
    fn level(&self) -> windows::core::Result<f32> {
        // SAFETY: the interface is live, owned by the smart pointer, and nothing is handed over.
        unsafe { self.0.GetMasterVolumeLevelScalar() }
    }

    /// Set the device's level as the percentage Windows shows for it over 100.
    #[allow(
        unsafe_code,
        reason = "COM call to SetMasterVolumeLevelScalar with a null event context, which the API takes as none."
    )]
    fn set_level(&self, level: f32) -> windows::core::Result<()> {
        // SAFETY: a null event context is documented as none, and it is the only pointer passed.
        unsafe { self.0.SetMasterVolumeLevelScalar(level, std::ptr::null()) }
    }
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
