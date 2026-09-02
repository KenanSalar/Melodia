//! Summing the voices into one block of device frames.
//!
//! The sum is **unclamped**, and that is load-bearing rather than an omission. A crossfade runs
//! complementary linear ramps on two decks (`g_out + g_in ≡ 1`) over sources that have each already
//! been clamped to ±1 by the limiter, so the sum is inside ±1 by construction. A ceiling here would
//! do nothing in the ordinary case and flatten the fade in the one case it fired.
//!
//! [`pair`] builds the two halves with no device between them, which is what lets
//! `tests/crossfade.rs` and `tests/stream_rate.rs` drive the whole chain — decoder, EQ, ramp, tap,
//! sum — by pulling [`MixerPull`] on the test thread. Anything that stops being reachable that way
//! stops being testable without a sound card.

use std::sync::Arc;

use super::super::audio::{Sample, Shape};

use super::voice::{Voice, VoicePull};

/// Device frames every voice advances before any voice advances again.
///
/// **This is what bounds a crossfade's overshoot.** The two ramps are armed together on the control
/// thread but each advances as *its own* source is pulled, so a voice that has already been
/// rendered for this block picks the arm up a block late. Render a whole device period per voice and
/// outgoing track stays at full gain for that whole period while the incoming one ramps up, and the
/// sum — which nothing clamps, deliberately — goes past unity by the period over the fade length.
/// Stepping every voice through the block together bounds that to this many frames instead, which
/// against the shortest crossfade the settings allow is a fraction of a percent. rodio's mixer
/// pulled one sample from every voice in turn, so this is the same property at a coarser grain,
/// chosen so the loop costs a few dozen iterations per callback rather than a few thousand.
pub const LOCKSTEP_FRAMES: usize = 64;

/// The control side: the voices everything is summed from.
///
/// The voices are shared rather than lent out, so `player::decks` can hold them for as long as it
/// needs without borrowing from this. Reference counting is what replaces the `Box::leak` rodio's
/// arrangement needed: a leaked handle outlives everything by never being dropped, and so never
/// releases the device either. The negotiated shape is not repeated here —
/// [`super::AudioOutput::negotiated`] is where it is asked for, and it carries the format too.
pub struct Mixer {
    voices: Box<[Arc<Voice>]>,
}

impl Mixer {
    /// The voice at `index`, or `None` past the count this mixer was built with.
    pub fn voice(&self, index: usize) -> Option<Arc<Voice>> {
        self.voices.get(index).map(Arc::clone)
    }
}

/// The audio side: pulled by the output callback, or by a test standing in for one.
pub struct MixerPull {
    voices: Box<[VoicePull]>,
    device: Shape,
    /// One voice's contribution before it is summed. Sized for a single [`LOCKSTEP_FRAMES`] step,
    /// which is the longest slice a voice is ever handed, so the audio thread never grows it.
    scratch: Vec<Sample>,
}

impl MixerPull {
    /// Write one block of interleaved device frames.
    ///
    /// A partial trailing frame is zeroed but never written into: half a frame would shear the
    /// channel layout for the rest of the stream. No host asks for one, and the zero is what makes
    /// that survivable anyway — cpal 0.18 pre-fills its own block with silence, but
    /// `device::output_stream` reuses one staging buffer and hands it over holding the last block.
    ///
    /// **The first voice with anything to say writes; the rest add.** Summing into a zeroed block
    /// would be simpler and would cost the sign of zero — `0.0 + -0.0` is `0.0` — so a lone voice at
    /// unity would not be the passthrough the bit-perfect claim rests on.
    pub fn fill(&mut self, out: &mut [Sample]) {
        let width = usize::from(self.device.channels.get());

        // Before the truncation, so the tail below is covered too.
        out.fill(0.0);

        let whole = out.len() - out.len() % width;
        let out = &mut out[..whole];

        for step in out.chunks_mut(LOCKSTEP_FRAMES * width) {
            let mut written = false;
            for voice in &mut self.voices {
                if written {
                    let reached = voice.render(&mut self.scratch[..step.len()]);
                    for (slot, sample) in step.iter_mut().zip(&self.scratch[..reached]) {
                        // Unclamped, deliberately — see the module docs.
                        *slot += *sample;
                    }
                } else {
                    // Straight into the block, so nothing has touched these samples on the way.
                    written = voice.render(step) > 0;
                }
            }
        }
    }
}

/// Build a mixer and its puller, with `voices` many brought to `device`.
pub fn pair(voices: usize, device: Shape) -> (Mixer, MixerPull) {
    let (controls, pulls): (Vec<_>, Vec<_>) =
        (0..voices).map(|_| super::voice::pair(device)).map(|(v, p)| (Arc::new(v), p)).unzip();

    let width = usize::from(device.channels.get());
    (
        Mixer {
            voices: controls.into_boxed_slice(),
        },
        MixerPull {
            voices: pulls.into_boxed_slice(),
            device,
            scratch: vec![0.0; LOCKSTEP_FRAMES * width],
        },
    )
}

#[cfg(test)]
#[path = "tests/mixer_tests.rs"]
mod tests;
