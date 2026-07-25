//! Tests for the visualizer's spectrum analysis.

use super::*;
use crate::player::dsp::db_to_linear;

// --- helpers -----------------------------------------------------------------

/// Assert two values are equal to within a tight tolerance.
fn approx(a: f32, b: f32) {
    assert!((a - b).abs() < 1e-4, "expected {b}, got {a}");
}

/// Fill a buffer with a full-scale sine.
#[allow(
    clippy::cast_precision_loss,
    reason = "test buffers are a few thousand samples, which convert to f32 exactly"
)]
fn fill_sine(buf: &mut [f32], freq_hz: f32, sample_rate: f32) {
    for (i, sample) in buf.iter_mut().enumerate() {
        *sample = (2.0 * std::f32::consts::PI * freq_hz * i as f32 / sample_rate).sin();
    }
}

/// Index and value of the loudest band.
fn loudest(levels: &[f32]) -> (usize, f32) {
    levels
        .iter()
        .enumerate()
        .fold((0, f32::MIN), |best, (i, &v)| if v > best.1 { (i, v) } else { best })
}

// --- hann_window -------------------------------------------------------------

#[test]
fn the_hann_window_starts_and_ends_at_zero() {
    let w = hann_window(FFT_SIZE);
    assert_eq!(w.len(), FFT_SIZE);
    approx(w[0], 0.0);
    approx(w[FFT_SIZE - 1], 0.0);
}

#[test]
fn the_hann_window_peaks_at_one_in_the_middle() {
    // An odd length puts a sample exactly on the peak.
    let odd = hann_window(9);
    approx(odd[4], 1.0);
    // An even length straddles it, so the two centre samples are both ~1.
    let w = hann_window(FFT_SIZE);
    approx(w[FFT_SIZE / 2], 1.0);
    approx(w[FFT_SIZE / 2 - 1], 1.0);
}

#[test]
fn the_hann_window_is_symmetric() {
    let w = hann_window(FFT_SIZE);
    for (i, &v) in w.iter().enumerate() {
        approx(v, w[FFT_SIZE - 1 - i]);
    }
}

#[test]
fn a_degenerate_hann_window_is_a_passthrough() {
    // No span to taper across — the formula would divide by zero.
    assert!(hann_window(0).is_empty());
    assert_eq!(hann_window(1).len(), 1);
    approx(hann_window(1)[0], 1.0);
}

// --- coherent_gain_scale -----------------------------------------------------

#[test]
fn the_hann_windows_coherent_gain_is_one_half() {
    let w = hann_window(FFT_SIZE);
    let mean = w.iter().sum::<f32>() / index_to_f32(FFT_SIZE);
    assert!((mean - 0.5).abs() < 1e-3, "coherent gain should be ~0.5, got {mean}");
    // The scale is what turns `A/2 · Σw` back into `A`.
    approx(coherent_gain_scale(&w) * w.iter().sum::<f32>(), 2.0);
}

#[test]
fn an_empty_window_has_no_scale() {
    approx(coherent_gain_scale(&[]), 0.0);
}

// --- band_edges --------------------------------------------------------------

const RATES: [f32; 3] = [44_100.0, 48_000.0, 96_000.0];

/// The frequency a (fractional) bin position sits at.
fn bin_to_hz(bin: f32, fft_size: usize, sample_rate: f32) -> f32 {
    bin * sample_rate / index_to_f32(fft_size)
}

#[test]
fn there_is_one_more_edge_than_there_are_bands() {
    for rate in RATES {
        let edges = band_edges(NUM_BANDS, FFT_SIZE, rate);
        assert_eq!(edges.len(), NUM_BANDS + 1, "at {rate} Hz");
    }
}

#[test]
fn edges_ascend_and_never_touch_dc() {
    for rate in RATES {
        let edges = band_edges(NUM_BANDS, FFT_SIZE, rate);
        // Bin 0 is DC. A band reaching it would plot any offset in the signal.
        assert!(edges.iter().all(|&e| e >= 1.0), "an edge reached DC at {rate} Hz");
        for pair in edges.windows(2) {
            if let [prev, next] = pair {
                assert!(next > prev, "edges {prev} -> {next} do not ascend at {rate} Hz");
            }
        }
    }
}

#[test]
fn the_range_runs_from_the_floor_to_the_cap() {
    for rate in RATES {
        let edges = band_edges(NUM_BANDS, FFT_SIZE, rate);
        let hz = |bin: f32| bin_to_hz(bin, FFT_SIZE, rate);
        assert!(edges.first().is_some_and(|&e| (hz(e) - MIN_HZ).abs() < 1.0), "at {rate} Hz");
        // Every one of these rates has Nyquist above the cap, so the cap wins.
        assert!(edges.last().is_some_and(|&e| (hz(e) - MAX_HZ).abs() < 1.0), "at {rate} Hz");
    }
}

#[test]
fn the_cap_yields_to_nyquist_on_a_low_rate_file() {
    // A 22.05 kHz file has no 16 kHz to show. Letting the edges run to the cap
    // anyway would clamp the top several bands onto the final bin, so they would
    // all read the same value — or, once clamped, span nothing at all.
    let rate = 22_050.0;
    let edges = band_edges(NUM_BANDS, FFT_SIZE, rate);
    let nyquist = rate / 2.0;
    let top = edges.last().map_or(0.0, |&e| bin_to_hz(e, FFT_SIZE, rate));
    assert!((top - nyquist).abs() < 1.0, "top band should reach Nyquist, reached {top} Hz");
    assert!(top < MAX_HZ, "the cap must not win over a lower Nyquist");
    // ...and the pile-up the clamp would have caused must not be there.
    let max_bin = index_to_f32(FFT_SIZE / 2);
    let pinned = edges.iter().filter(|&&e| e >= max_bin).count();
    assert_eq!(pinned, 1, "only the final edge may sit on the last bin");
}

#[test]
fn a_nonsense_sample_rate_yields_no_edges() {
    // The last of these is a rate whose Nyquist is below the band floor.
    for rate in [0.0, -44_100.0, f32::NAN, f32::INFINITY, 80.0] {
        assert!(band_edges(NUM_BANDS, FFT_SIZE, rate).is_empty(), "rate {rate}");
    }
    assert!(band_edges(0, FFT_SIZE, 44_100.0).is_empty());
    assert!(band_edges(NUM_BANDS, 1, 44_100.0).is_empty());
}

#[test]
fn more_bands_than_bins_stays_in_range() {
    // A 16-point transform has 9 bins, far fewer than the bands want. The edges
    // must still land inside it rather than off the end of the spectrum.
    let edges = band_edges(NUM_BANDS, 16, 44_100.0);
    assert_eq!(edges.len(), NUM_BANDS + 1);
    for &edge in &edges {
        assert!(edge.is_finite(), "edge {edge} is not a number");
        assert!((1.0..=8.0).contains(&edge), "edge {edge} is outside the spectrum");
    }
}

// --- dsp::linear_to_db -------------------------------------------------------

#[test]
fn linear_to_db_reference_points() {
    // The inverse of `db_to_linear`, which `replaygain_tests` pins from the
    // other side: unity is 0 dB, ×2 is ~+6.02, ×0.5 is ~-6.02, ×10 is +20.
    approx(linear_to_db(1.0), 0.0);
    approx(linear_to_db(2.0), 6.020_6);
    approx(linear_to_db(0.5), -6.020_6);
    approx(linear_to_db(10.0), 20.0);
}

// --- level_from_magnitude ----------------------------------------------------

#[test]
fn silence_maps_to_a_zero_level() {
    // The guard that keeps `log10(0)` out of the bars.
    approx(level_from_magnitude(0.0), 0.0);
    approx(level_from_magnitude(-1.0), 0.0);
}

#[test]
fn full_scale_maps_to_a_full_level() {
    approx(level_from_magnitude(1.0), 1.0);
    // Anything louder clamps rather than overshooting the bar.
    approx(level_from_magnitude(4.0), 1.0);
}

#[test]
fn the_floor_maps_to_a_zero_level() {
    approx(level_from_magnitude(db_to_linear(FLOOR_DB)), 0.0);
    approx(level_from_magnitude(db_to_linear(FLOOR_DB / 2.0)), 0.5);
}

// --- bands_from_spectrum -----------------------------------------------------

#[test]
fn a_band_takes_its_loudest_bin() {
    let spectrum = [
        Complex::new(0.0, 0.0),
        Complex::new(0.25, 0.0),
        Complex::new(1.0, 0.0),
        Complex::new(0.0, 0.0),
        Complex::new(0.0, 0.0),
        Complex::new(0.0, 0.0),
    ];
    // Two bands, each spanning whole bins: 1..=2 then 3..=5.
    let mut out = [0.0; 2];
    bands_from_spectrum(&spectrum, &[1.0, 3.0, 6.0], 1.0, &mut out);
    // The 1.0 bin wins over its quieter neighbour, so the bar is full...
    approx(out[0], 1.0);
    // ...and it stays out of the next band.
    approx(out[1], 0.0);
}

#[test]
fn a_band_narrower_than_a_bin_interpolates_between_its_neighbours() {
    // Bin 1 is silent, bin 2 is full scale. A band sitting entirely between them
    // has no bin of its own, so it must read the slope rather than seize either.
    let spectrum = [Complex::new(0.0, 0.0), Complex::new(0.0, 0.0), Complex::new(1.0, 0.0)];
    let quarter = level_from_magnitude(0.25);
    let half = level_from_magnitude(0.5);

    // Centre 1.25 -> a quarter of the way from bin 1 to bin 2.
    let mut out = [0.0; 1];
    bands_from_spectrum(&spectrum, &[1.2, 1.3], 1.0, &mut out);
    approx(out[0], quarter);

    // Centre 1.5 -> halfway. Strictly louder than the band below it, which is the
    // property the old whole-bin snapping destroyed: both would have read bin 1.
    bands_from_spectrum(&spectrum, &[1.45, 1.55], 1.0, &mut out);
    approx(out[0], half);
    assert!(half > quarter, "adjacent sub-bin bands must slope, not step");
}

#[test]
fn a_sub_bin_band_on_the_last_bin_holds_rather_than_fading() {
    // Nothing above the final bin to interpolate toward — reading a phantom zero
    // there would notch the top bar on every frame.
    let spectrum = [Complex::new(0.0, 0.0), Complex::new(1.0, 0.0)];
    let mut out = [0.0; 1];
    bands_from_spectrum(&spectrum, &[1.4, 1.6], 1.0, &mut out);
    approx(out[0], 1.0);
}

#[test]
fn bands_beyond_the_edges_read_as_silence() {
    let spectrum = [Complex::new(1.0, 0.0); 4];
    let mut out = [0.5; 4];
    // Three edges describe two bands, leaving two output slots unclaimed. The top
    // edge stays on the last bin, as `band_edges`' clamp guarantees in production.
    bands_from_spectrum(&spectrum, &[1.0, 2.0, 3.0], 1.0, &mut out);
    approx(out[0], 1.0);
    approx(out[1], 1.0);
    // Stale heights left in the tail would freeze on screen.
    approx(out[2], 0.0);
    approx(out[3], 0.0);
}

#[test]
fn no_edges_at_all_silences_every_band() {
    let spectrum = [Complex::new(1.0, 0.0); 4];
    let mut out = [0.5; 3];
    bands_from_spectrum(&spectrum, &[], 1.0, &mut out);
    for (i, &level) in out.iter().enumerate() {
        approx(level, 0.0);
        assert!(level.abs() < 1e-4, "band {i} kept a stale height");
    }
}

// --- smooth ------------------------------------------------------------------

#[test]
fn levels_rise_immediately_to_a_higher_value() {
    let mut levels = [0.0, 0.2];
    smooth(&mut levels, &[1.0, 0.9], ATTACK, DECAY);
    approx(levels[0], 1.0);
    approx(levels[1], 0.9);
}

#[test]
fn levels_decay_gradually_toward_a_lower_value() {
    let mut levels = [1.0];
    let mut previous = 1.0;
    for _ in 0..8 {
        smooth(&mut levels, &[0.0], ATTACK, DECAY);
        assert!(levels[0] < previous, "level should fall, went {previous} -> {}", levels[0]);
        previous = levels[0];
    }
    // Gradually: a single frame is nowhere near the whole distance.
    assert!(previous > 0.0, "eight frames should not have reached silence");
}

#[test]
fn decayed_levels_converge_to_zero() {
    let mut levels = [1.0];
    for _ in 0..200 {
        smooth(&mut levels, &[0.0], ATTACK, DECAY);
    }
    approx(levels[0], 0.0);
}

#[test]
fn a_level_never_decays_below_its_band() {
    let mut levels = [1.0];
    for _ in 0..50 {
        smooth(&mut levels, &[0.4], ATTACK, DECAY);
        assert!(levels[0] >= 0.4, "decayed past the band's own level: {}", levels[0]);
    }
    approx(levels[0], 0.4);
}

// --- SpectrumAnalyzer --------------------------------------------------------

#[test]
fn a_full_scale_sine_lands_in_the_band_containing_its_frequency() {
    let rate = 44_100;
    let mut analyzer = SpectrumAnalyzer::new(FFT_SIZE, NUM_BANDS);
    fill_sine(analyzer.window_mut(), 1_000.0, rate_to_f32(rate));
    let levels = analyzer.analyze(rate);

    let (peak_band, peak_level) = loudest(levels);
    let tone_bin = hz_to_bin(1_000.0, FFT_SIZE, rate_to_f32(rate));
    let edges = band_edges(NUM_BANDS, FFT_SIZE, rate_to_f32(rate));
    let holds_tone = edges.windows(2).position(|pair| match pair {
        [lo, hi] => (*lo..*hi).contains(&tone_bin),
        _ => false,
    });
    assert_eq!(holds_tone, Some(peak_band));

    // Full scale reads near the top of the bar; Hann's worst-case scalloping
    // loss is ~1.4 dB, which is a hair off full height on a 70 dB scale.
    assert!(peak_level > 0.95, "expected a near-full bar, got {peak_level}");
    // ...and the rest of the display stays down.
    assert!(levels[0] < 0.2, "bass leaked from a 1 kHz tone: {}", levels[0]);
}

#[test]
fn silence_produces_all_zero_bands() {
    let mut analyzer = SpectrumAnalyzer::new(FFT_SIZE, NUM_BANDS);
    analyzer.window_mut().fill(0.0);
    let levels = analyzer.analyze(44_100);
    assert_eq!(levels.len(), NUM_BANDS);
    for (i, &level) in levels.iter().enumerate() {
        assert!(level.abs() < 1e-4, "band {i} is {level} on silence");
    }
}

#[test]
fn an_unknown_sample_rate_produces_no_bands() {
    let mut analyzer = SpectrumAnalyzer::new(FFT_SIZE, NUM_BANDS);
    fill_sine(analyzer.window_mut(), 1_000.0, 44_100.0);
    // Nothing has played, so there is no rate to place band edges against.
    let levels = analyzer.analyze(0);
    for (i, &level) in levels.iter().enumerate() {
        assert!(level.abs() < 1e-4, "band {i} is {level} with no sample rate");
    }
}

#[test]
fn the_bars_decay_once_the_signal_stops() {
    let rate = 44_100;
    let mut analyzer = SpectrumAnalyzer::new(FFT_SIZE, NUM_BANDS);
    fill_sine(analyzer.window_mut(), 1_000.0, rate_to_f32(rate));
    let loud = loudest(analyzer.analyze(rate));

    analyzer.window_mut().fill(0.0);
    let quiet = analyzer.analyze(rate);
    assert!(quiet[loud.0] < loud.1, "the bar should start falling once the tone stops");
    assert!(quiet[loud.0] > 0.0, "it should fall gently, not cut out");
}

#[test]
fn changing_the_sample_rate_rebuilds_the_band_edges() {
    let mut analyzer = SpectrumAnalyzer::new(FFT_SIZE, NUM_BANDS);
    analyzer.window_mut().fill(0.0);
    analyzer.analyze(44_100);
    assert_eq!(analyzer.mapped_rate, 44_100);
    let first: Vec<_> = analyzer.edges.to_vec();

    analyzer.window_mut().fill(0.0);
    analyzer.analyze(96_000);
    assert_eq!(analyzer.mapped_rate, 96_000);
    assert_ne!(analyzer.edges.to_vec(), first, "band edges must follow the sample rate");
}
