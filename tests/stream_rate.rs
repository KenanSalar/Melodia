//! Two stations at different sample rates through one deck, no audio device.
//!
//! rodio's `mixer()` builds a device-less `Mixer` + `MixerSource`, and a `Player` connected to it
//! is one mixer voice: one `UniformSourceIterator` wrapping everything the deck ever plays. That
//! is the whole hazard. The converter inside it is rebuilt only at a span boundary, so a source
//! reporting no span (`Source::current_span_len() == None`) reaches it as an unbounded `Take` and
//! pins it to whichever station landed first. Every station after that is resampled at the first
//! one's rate, and it lasts until the process restarts.
//!
//! Nothing reports that, and no unit test on the source can see it: the fault is in what the
//! *mixer* built out of the answer. Hence a square wave with a known period, read back off the
//! mixer's own output in output samples per half period, which is the ratio of the two rates and
//! nothing else. `player::tests::prebuffer` pins the answer; this pins what rodio does with it.

use std::num::NonZero;

use melodia::player::prebuffer::{PrebufferSource, StreamShared};
use melodia::player::rodio_compat::RodioBridge;

/// The device rate everything is resampled to.
const OUT_RATE: u32 = 48_000;
const CHANNELS: u16 = 1;

/// The first station's rate, deliberately half the second's: a converter still pinned to it plays
/// the second station at exactly twice the period, which no tolerance can absorb.
const FIRST_RATE: u32 = 24_000;
const SECOND_RATE: u32 = 48_000;

/// Source samples per polarity of the square wave. Long enough that the resampler's interpolated
/// crossing is a rounding error against it, short enough for hundreds of readings per second.
const HALF_PERIOD: usize = 48;

/// How much of each station to feed. Comfortably inside the ring, which holds `PREBUFFER_MS` of
/// audio, so the whole station goes in up front and nothing is measured against a starved ring.
const STATION_SECONDS: u32 = 1;

/// Output samples pulled before a measurement is taken, covering the mixer's own pipeline latency
/// and the first station's partial leading run.
const WARMUP: usize = 4_000;

/// Output samples each reading is taken over.
const WINDOW: usize = 16_000;

/// Fractional slack on a half-period reading. The two rates differ by 2x, so this only has to
/// absorb the interpolated crossing and the partial runs at either end of the window.
const TOLERANCE: f64 = 0.05;

fn nz_u16(value: u16) -> rodio::ChannelCount {
    NonZero::new(value).unwrap_or(NonZero::<u16>::MIN)
}

fn nz_u32(value: u32) -> rodio::SampleRate {
    NonZero::new(value).unwrap_or(NonZero::<u32>::MIN)
}

/// A station already fully buffered and closed, so the deck plays it start to finish without ever
/// reaching for the feed thread this test does not have.
fn station(rate: u32, seconds: u32) -> RodioBridge<PrebufferSource> {
    let shared = StreamShared::new();
    let (source, writer) = PrebufferSource::new(shared.clone(), nz_u16(CHANNELS), nz_u32(rate));

    let total = usize::try_from(rate * seconds).unwrap_or(0);
    for index in 0..total {
        let polarity = if (index / HALF_PERIOD).is_multiple_of(2) {
            1.0
        } else {
            -1.0
        };
        assert!(writer.push(polarity), "the ring must take the whole station up front");
    }
    shared.finish();
    RodioBridge::new(source)
}

fn pull(out: &mut rodio::mixer::MixerSource, count: usize) -> Vec<f32> {
    let mut samples = Vec::with_capacity(count);
    for _ in 0..count {
        let Some(sample) = out.next() else {
            break;
        };
        samples.push(sample);
    }
    samples
}

/// Mean output samples between polarity changes, or `None` if the window held no wave.
///
/// Read on a hysteresis band rather than on the sign, the resampler crossing zero over an
/// interpolated sample or two. The first and last runs are partial by construction, so the mean is
/// taken between the outermost flips rather than over every run.
fn half_period(samples: &[f32]) -> Option<f64> {
    const BAND: f32 = 0.5;

    let mut polarity: Option<bool> = None;
    let mut first_flip = None;
    let mut last_flip = 0;
    let mut flips = 0_u32;

    for (index, sample) in samples.iter().enumerate() {
        let level = if *sample > BAND {
            Some(true)
        } else if *sample < -BAND {
            Some(false)
        } else {
            continue;
        };
        if polarity.is_some() && polarity != level {
            first_flip.get_or_insert(index);
            last_flip = index;
            flips += 1;
        }
        polarity = level;
    }

    let runs = flips.checked_sub(1).filter(|runs| *runs > 0)?;
    let span = u32::try_from(last_flip - first_flip?).ok()?;
    Some(f64::from(span) / f64::from(runs))
}

/// Output samples per half period a station at `rate` should read back as.
fn expected(rate: u32) -> f64 {
    let ratio = f64::from(OUT_RATE) / f64::from(rate);
    let half_period = u32::try_from(HALF_PERIOD).unwrap_or(u32::MAX);
    f64::from(half_period) * ratio
}

fn assert_reads_as(samples: &[f32], rate: u32, what: &str) {
    let want = expected(rate);
    let measured = half_period(samples);
    assert!(
        measured.is_some_and(|measured| (measured - want).abs() <= want * TOLERANCE),
        "{what}: read {measured:?} output samples per half period, expected ~{want} for a \
         {rate} Hz source"
    );
}

/// The regression: tune to a 24 kHz station, then to a 48 kHz one, and the second must play at its
/// own rate. Pinned to the first station's it reads back at twice the period, which is what a
/// listener hears as the second station playing slow.
#[test]
fn a_second_station_plays_at_its_own_rate_rather_than_the_first_ones() {
    let (mixer, mut out) = rodio::mixer::mixer(nz_u16(CHANNELS), nz_u32(OUT_RATE));
    let deck = rodio::Player::connect_new(&mixer);

    deck.append(station(FIRST_RATE, STATION_SECONDS));
    deck.play();

    let _ = pull(&mut out, WARMUP);
    let first = pull(&mut out, WINDOW);
    assert_reads_as(&first, FIRST_RATE, "the first station");

    // Drain what is left of it, so the second starts from the deck's own silence rather than
    // mid-station. One second of source at either rate is one second of output.
    let drain = usize::try_from(OUT_RATE).unwrap_or(0);
    let _ = pull(&mut out, drain);

    deck.append(station(SECOND_RATE, STATION_SECONDS));
    let _ = pull(&mut out, WARMUP);
    let second = pull(&mut out, WINDOW);
    assert_reads_as(&second, SECOND_RATE, "the second station");
}
