//! Summing the voices into one block of device frames, unclamped: a crossfade's complementary
//! ramps over already-clamped sources sum inside ±1, and a ceiling would only ever flatten a fade.
//!
//! [`pair`] builds the two halves with no device between them, which is how `crossfade.rs` and
//! `stream_rate.rs` drive the whole chain on the test thread with no sound card.

use std::sync::Arc;
use std::time::Duration;

use melodia_audio::player::source::audio::{Sample, SampleRate, Shape, frames_in};

use super::voice::{Voice, VoicePull};

/// Frames every voice advances before any advances again, which bounds a crossfade's overshoot:
/// each ramp moves as its own voice is pulled, so a whole period per voice would hold the outgoing
/// track at full gain for that period. An interpolating converter's lookahead adds to it. Coarse
/// enough that the loop runs a few dozen times per callback rather than thousands.
pub const LOCKSTEP_FRAMES: usize = 64;

/// The voices, reference-counted so the decks hold them without borrowing from this.
pub struct Mixer {
    voices: Box<[Arc<Voice>]>,
}

impl Mixer {
    pub fn voice(&self, index: usize) -> Option<Arc<Voice>> {
        self.voices.get(index).map(Arc::clone)
    }
}

/// Pulled by the output callback, or by a test standing in for one.
pub struct MixerPull {
    voices: Box<[VoicePull]>,
    device: Shape,
    /// One [`LOCKSTEP_FRAMES`] step, the longest slice a voice is handed, so it never grows.
    scratch: Vec<Sample>,
    /// Silence still owed before any voice plays ([`Self::hold`]).
    held: usize,
    resync: Option<Resync>,
}

/// A reopen's rate-change silence, decided at each stream's reshape.
#[derive(Clone, Copy)]
struct Resync {
    /// The rate the device ran at before the reopen, or `None` where no stream was open.
    from: Option<SampleRate>,
    hold: Duration,
    /// What was still held when the reopen began, which a stream at the same rate carries on.
    held: usize,
}

impl MixerPull {
    /// A partial trailing frame is zeroed and never written, or it would shear the channel layout;
    /// the zero matters, the staging buffer arriving holding the last block. The first voice with
    /// anything writes and the rest add: summing into zeros would lose `-0.0`, and with it the
    /// bit-perfect passthrough.
    pub fn fill(&mut self, out: &mut [Sample]) {
        let width = usize::from(self.device.channels.get());

        // Before the truncation, so the tail below is covered too.
        out.fill(0.0);

        let whole = out.len() - out.len() % width;
        let out = &mut out[..whole];

        for step in out.chunks_mut(LOCKSTEP_FRAMES * width) {
            if self.held > 0 {
                // Serviced but not rendered: a clear issued behind a reopen still has to land, and
                // no source may advance through audio nobody hears.
                for voice in &mut self.voices {
                    voice.service();
                }
                self.held = self.held.saturating_sub(step.len() / width);
                continue;
            }
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

    /// Nothing loaded depends on the device, so a source carries on from the same frame. Only
    /// while no stream pulls.
    pub fn reshape(&mut self, device: Shape) {
        self.device = device;
        for voice in &mut self.voices {
            voice.reshape(device);
        }
        self.scratch.resize(LOCKSTEP_FRAMES * usize::from(device.channels.get()), 0.0);
        if let Some(Resync { from, hold, held }) = self.resync {
            let frames = if from == Some(device.rate) {
                held
            } else {
                usize::try_from(frames_in(hold, device.rate)).unwrap_or(usize::MAX)
            };
            self.hold(frames);
        }
    }

    /// A DAC mutes while its clock relocks, so the silence is what gets lost rather than the
    /// track's head. Rounded up to whole [`LOCKSTEP_FRAMES`] steps.
    fn hold(&mut self, frames: usize) {
        self.held = frames;
    }

    /// Settled at each [`Self::reshape`], the last step before a stream pulls, since a reopen can
    /// try several rates before one opens.
    pub fn arm_resync(&mut self, from: Option<SampleRate>, hold: Duration) {
        self.resync = Some(Resync { from, hold, held: self.held });
    }

    pub fn disarm_resync(&mut self) {
        self.resync = None;
    }
}

/// Build a mixer and its puller, with `voices` many brought to `device`. A voice's `clear()`
/// blocks until the puller services it, so a test driving the puller runs it on its own thread.
pub fn pair(voices: usize, device: Shape) -> (Mixer, MixerPull) {
    let (controls, pulls): (Vec<_>, Vec<_>) =
        (0..voices).map(|_| super::voice::pair(device)).map(|(v, p)| (Arc::new(v), p)).unzip();

    let width = usize::from(device.channels.get());
    (
        Mixer { voices: controls.into_boxed_slice() },
        MixerPull {
            voices: pulls.into_boxed_slice(),
            device,
            scratch: vec![0.0; LOCKSTEP_FRAMES * width],
            held: 0,
            resync: None,
        },
    )
}

#[cfg(test)]
#[path = "tests/mixer_tests.rs"]
mod tests;
