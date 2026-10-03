//! A sound card's own volume control, as the [`VolumeControl`] an ALSA claim carries the level on:
//! one element of the card's simple mixer, on the curve `alsamixer` shows. Taking it, following it
//! and giving it back is [`hardware_volume`]'s.
//!
//! **`Master`, else the card's only element with a playback volume, and only for the card's device
//! 0.** Anything looser can take an element that changes nothing the claim plays, and the voices
//! would then hand the card unity samples at full volume:
//! - the mixer is the card's, and an HDMI or S/PDIF device on an HD Audio card shares it while no
//!   element on it acts on that device;
//! - `PCM` on an HD Audio card is usually alsa-lib's softvol, a user control only its own plugin
//!   applies, so it does nothing on `hw:`, and the alsa crate can't tell a user control from a
//!   driver's;
//! - past `Master`, an HD Audio codec's elements each reach one output or an input's loopback,
//!   never the whole card.
//!
//! Where none qualifies the voices keep the level, and the claim goes ahead.
//!
//! **The curve is alsa-utils' `volume_mapping.c`**, so `alsamixer` and Melodia's slider read the
//! same percentage: linear in dB over a range of 24 dB or less, a cube-root taper of the amplitude
//! past it, and the raw steps where the element states no dB range.
//!
//! [`hardware_volume`]: super::hardware_volume

use alsa::mixer::{MilliBel, Selem, SelemChannelId, SelemId};
use alsa::{Mixer, Round};

use melodia_core::error::describe;

use super::DeviceLevel;
use super::alsa::Card;
use super::hardware_volume::{HardwareVolume, VolumeControl};

/// The one device of a card its mixer's elements are known to act on.
const MIXER_DEVICE: u32 = 0;

const MASTER: &str = "Master";

/// The widest dB range `alsamixer` spreads linearly, in hundredths of a dB.
const MAX_LINEAR_DB_RANGE: i64 = 24 * 100;

/// The floor an element states where its lowest step is silence rather than a level: alsa-lib's
/// `SND_CTL_TLV_DB_GAIN_MUTE`, in hundredths of a dB.
const DB_GAIN_MUTE: i64 = -9_999_999;

/// Hundredths of a dB per decade of the taper, which is the amplitude's cube root: 20 dB a decade,
/// three times over.
const TAPER_DECADE: f64 = 6000.0;

/// A claimed card's hardware volume, put back to its original level when dropped.
pub(super) type CardVolume = HardwareVolume<MixerControl>;

/// One element of a card's simple mixer.
pub(super) struct MixerControl {
    mixer: Mixer,
    /// Looked up on each call, since an element borrows the mixer it came from.
    element: SelemId,
    scale: Scale,
}

/// The volume element for a claim on `card`, set to `volume` unless the system had it quieter, or
/// `None` where the card has none a claim can trust.
///
/// # Errors
///
/// What the card answered when its mixer was opened, or asked for the element's level or a new one.
pub(super) fn take(card: &Card, volume: f64) -> Result<Option<CardVolume>, alsa::Error> {
    let Some(control) = MixerControl::open(card)? else { return Ok(None) };
    HardwareVolume::take(control, &card.device.id, volume).map(Some)
}

/// The element a claim on `card` carries the volume on, or would, as it stands. `None` where the
/// card has none, or won't say.
pub(super) fn read(card: &Card) -> Option<DeviceLevel> {
    read_level(card).unwrap_or_else(|e| {
        log::debug!("audio: {}'s own volume couldn't be read: {}", card.device.name, describe(&e));
        None
    })
}

fn read_level(card: &Card) -> Result<Option<DeviceLevel>, alsa::Error> {
    let Some(control) = MixerControl::open(card)? else { return Ok(None) };
    let element = control.element()?;
    let level = control.scale.level_of(control.reading(&element)?);
    let muted =
        element.has_playback_switch() && element.get_playback_switch(SelemChannelId::mono())? == 0;
    Ok(Some(DeviceLevel::new(level, muted)))
}

impl MixerControl {
    /// The card's volume element, or `None` where a claim on its device can't trust one.
    fn open(card: &Card) -> Result<Option<Self>, alsa::Error> {
        if card.number != MIXER_DEVICE {
            return Ok(None);
        }
        let mixer = Mixer::new(&format!("hw:{}", card.index), true)?;
        let Some(element) = volume_element(&mixer) else { return Ok(None) };
        let Some(scale) = mixer.find_selem(&element).map(|selem| Scale::of(&selem)) else {
            return Ok(None);
        };
        Ok(Some(Self { mixer, element, scale }))
    }

    fn element(&self) -> Result<Selem<'_>, alsa::Error> {
        self.mixer.find_selem(&self.element).ok_or_else(|| {
            alsa::Error::new("snd_mixer_find_selem", rustix::io::Errno::NODEV.raw_os_error())
        })
    }

    /// Where the element sits, on its scale's own terms.
    fn reading(&self, element: &Selem<'_>) -> Result<i64, alsa::Error> {
        let step = element.get_playback_volume(SelemChannelId::mono())?;
        self.reading_of(element, step)
    }

    fn reading_of(&self, element: &Selem<'_>, step: i64) -> Result<i64, alsa::Error> {
        match self.scale {
            Scale::Steps { .. } => Ok(step),
            Scale::Decibels { .. } | Scale::Taper { .. } => {
                element.ask_playback_vol_db(step).map(|MilliBel(reading)| reading)
            }
        }
    }

    /// The step whose level lands nearest `level`, so Melodia's slider and `alsamixer` agree to the
    /// whole percent. Silence is the lowest step, with the switch left alone.
    fn step_for(&self, element: &Selem<'_>, level: f32) -> Result<i64, alsa::Error> {
        let (lowest, highest) = element.get_playback_volume_range();
        if level <= 0.0 {
            return Ok(lowest);
        }
        let target = self.scale.reading_for(level);
        let (below, above) = match self.scale {
            Scale::Steps { .. } => (whole(target.floor()), whole(target.ceil())),
            Scale::Decibels { .. } | Scale::Taper { .. } => {
                let target = MilliBel(whole(target));
                (
                    element.ask_playback_db_vol(target, Round::Floor)?,
                    element.ask_playback_db_vol(target, Round::Ceil)?,
                )
            }
        };
        let miss = |step: i64| -> Result<f32, alsa::Error> {
            Ok((self.scale.level_of(self.reading_of(element, step)?) - level).abs())
        };
        let nearest = if miss(above)? < miss(below)? { above } else { below };
        Ok(nearest.clamp(lowest, highest))
    }
}

impl VolumeControl for MixerControl {
    type Error = alsa::Error;

    fn level(&self) -> Result<f32, alsa::Error> {
        // The mixer caches each element until its events are read, and a move made elsewhere
        // arrives as one.
        self.mixer.handle_events()?;
        let element = self.element()?;
        Ok(self.scale.level_of(self.reading(&element)?))
    }

    fn set_level(&self, level: f32) -> Result<(), alsa::Error> {
        let element = self.element()?;
        let step = self.step_for(&element, level)?;
        element.set_playback_volume_all(step)
    }
}

/// The element a claim carries the volume on, of the ones `mixer` lists.
fn volume_element(mixer: &Mixer) -> Option<SelemId> {
    let candidates: Vec<(String, u32)> = mixer
        .iter()
        .filter_map(Selem::new)
        .filter(Selem::has_playback_volume)
        .filter_map(|selem| {
            let id = selem.get_id();
            Some((id.get_name().ok()?.to_owned(), id.get_index()))
        })
        .collect();
    let (name, index) = pick(&candidates)?;
    Some(SelemId::new(name, *index))
}

/// Of the elements with a playback volume, by name and index, the one a claim can trust.
fn pick(candidates: &[(String, u32)]) -> Option<&(String, u32)> {
    let master = candidates.iter().find(|(name, _)| name == MASTER);
    master.or(match candidates {
        [only] => Some(only),
        _ => None,
    })
}

/// How an element's readings map onto the fraction `alsamixer` shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scale {
    /// No dB range stated: the raw steps, spread linearly.
    Steps { min: i64, max: i64 },
    /// A dB range no wider than [`MAX_LINEAR_DB_RANGE`], spread linearly in dB.
    Decibels { min: i64, max: i64 },
    /// Wider: the cube-root taper, its floor moved to zero unless that is silence already.
    Taper { min: i64, max: i64 },
}

impl Scale {
    fn of(element: &Selem<'_>) -> Self {
        let (MilliBel(min), MilliBel(max)) = element.get_playback_db_range();
        Self::from_ranges((min, max), element.get_playback_volume_range())
    }

    /// The scale for an element stating `decibels` and `steps`, each as `(min, max)`. The alsa
    /// crate answers `(0, 0)` for an element with no dB range, which lands on the steps.
    fn from_ranges(decibels: (i64, i64), steps: (i64, i64)) -> Self {
        let (min, max) = decibels;
        if min >= max {
            let (min, max) = steps;
            return Self::Steps { min, max };
        }
        if max - min <= MAX_LINEAR_DB_RANGE {
            Self::Decibels { min, max }
        } else {
            Self::Taper { min, max }
        }
    }

    /// The fraction for `reading`: a raw step, or hundredths of a dB where the element states a
    /// range.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "a fraction held to 0..=1 loses nothing a mixer step can resolve in f32"
    )]
    fn level_of(self, reading: i64) -> f32 {
        let fraction = match self {
            Self::Steps { min, max } | Self::Decibels { min, max } => {
                if max <= min {
                    return 0.0;
                }
                float(reading - min) / float(max - min)
            }
            Self::Taper { min, max } => {
                let level = taper(reading - max);
                if min == DB_GAIN_MUTE {
                    level
                } else {
                    let floor = taper(min - max);
                    (level - floor) / (1.0 - floor)
                }
            }
        };
        fraction.clamp(0.0, 1.0) as f32
    }

    /// The reading [`Self::level_of`] answers `level` for, before it is rounded to a step. `level`
    /// is above zero: silence is the lowest step, not a reading.
    fn reading_for(self, level: f32) -> f64 {
        let level = f64::from(level);
        match self {
            Self::Steps { min, max } | Self::Decibels { min, max } => {
                level * float(max - min) + float(min)
            }
            Self::Taper { min, max } => {
                let level = if min == DB_GAIN_MUTE {
                    level
                } else {
                    let floor = taper(min - max);
                    level * (1.0 - floor) + floor
                };
                TAPER_DECADE * level.log10() + float(max)
            }
        }
    }
}

/// The taper's amplitude-cube-root fraction for a reading `below_top` hundredths of a dB under the
/// element's top.
fn taper(below_top: i64) -> f64 {
    10_f64.powf(float(below_top) / TAPER_DECADE)
}

#[expect(
    clippy::cast_precision_loss,
    reason = "a mixer reading is far inside the integers a double holds exactly"
)]
fn float(reading: i64) -> f64 {
    reading as f64
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "a reading on a mixer's own range, far inside i64"
)]
fn whole(reading: f64) -> i64 {
    reading.round() as i64
}

#[cfg(test)]
#[path = "tests/alsa_volume_tests.rs"]
mod tests;
