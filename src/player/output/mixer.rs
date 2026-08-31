//! Summing the decks into one block of device frames.
//!
//! The sum is **unclamped**, and that is load-bearing rather than an omission. A crossfade runs
//! complementary linear ramps on two decks (`g_out + g_in ≡ 1`) over sources that have each already
//! been clamped to ±1 by the limiter, so the sum is inside ±1 by construction. A ceiling here would
//! do nothing in the ordinary case and flatten the fade in the one case it fired.
//!
//! [`pair`] builds the two halves with no device between them, which is what lets `tests/crossfade.rs`
//! and `tests/stream_rate.rs` drive the whole chain — decoder, EQ, ramp, tap, sum — by pulling
//! [`MixerPull`] on the test thread. Anything that stops being reachable that way stops being
//! testable without a sound card.

use super::super::audio::Sample;
use super::convert::Shape;
use super::deck::{Deck, DeckVoice};

/// Device frames the scratch buffer is sized for up front.
///
/// Only a ceiling: a host asking for a longer block grows it once and keeps it. Comfortably past
/// the ~50 ms period the output asks for, so in practice the growth never happens.
const SCRATCH_FRAMES: usize = 8_192;

/// The control side: the decks, and the shape everything is brought to.
pub struct Mixer {
    decks: Box<[Deck]>,
    device: Shape,
}

impl Mixer {
    /// The deck at `index`, or `None` past the voice count this mixer was built with.
    pub fn deck(&self, index: usize) -> Option<&Deck> {
        self.decks.get(index)
    }

    pub fn device(&self) -> Shape {
        self.device
    }
}

/// The audio side: pulled by the output callback, or by a test standing in for one.
pub struct MixerPull {
    voices: Box<[DeckVoice]>,
    device: Shape,
    /// One deck's contribution before it is summed. Sized once; never grown on the audio thread
    /// after the first block that needs it.
    scratch: Vec<Sample>,
}

impl MixerPull {
    /// Write one block of interleaved device frames.
    ///
    /// A partial trailing frame is left untouched: writing half of one would shear the channel
    /// layout for the rest of the stream, and no host asks for one.
    ///
    /// **The first deck with anything to say writes; the rest add.** Summing into a zeroed block
    /// would be simpler and would cost the sign of zero — `0.0 + -0.0` is `0.0` — so a lone deck at
    /// unity would not be the passthrough the bit-perfect claim rests on. It also saves a pass in
    /// the case that is almost always the live one, a single deck playing.
    pub fn fill(&mut self, out: &mut [Sample]) {
        let width = usize::from(self.device.channels.get());
        let whole = out.len() - out.len() % width;
        let out = &mut out[..whole];

        out.fill(0.0);
        if self.scratch.len() < whole {
            self.scratch.resize(whole, 0.0);
        }

        let mut written = false;
        for voice in &mut self.voices {
            if written {
                let reached = voice.render(&mut self.scratch[..whole]);
                for (slot, sample) in out.iter_mut().zip(&self.scratch[..reached]) {
                    // Unclamped, deliberately — see the module docs.
                    *slot += *sample;
                }
            } else {
                // Straight into the block, so nothing has touched these samples on the way.
                written = voice.render(out) > 0;
            }
        }
    }
}

/// Build a mixer and its puller, with `voices` decks brought to `device`.
pub fn pair(voices: usize, device: Shape) -> (Mixer, MixerPull) {
    let (decks, pulls): (Vec<_>, Vec<_>) = (0..voices).map(|_| super::deck::pair(device)).unzip();

    let width = usize::from(device.channels.get());
    (
        Mixer {
            decks: decks.into_boxed_slice(),
            device,
        },
        MixerPull {
            voices: pulls.into_boxed_slice(),
            device,
            scratch: vec![0.0; SCRATCH_FRAMES * width],
        },
    )
}

#[cfg(test)]
#[path = "tests/mixer_tests.rs"]
mod tests;
