//! Tests for the visualizer's spectrum analysis.

use std::cell::Cell;

use super::*;
use crate::player::dsp::{VISUALIZER_DECAY, db_to_linear};
use crate::player::tests::helpers::{assert_approx as approx, fill_sine};

// --- helpers -----------------------------------------------------------------

/// Fill both of an analyzer's windows with the same sine.
fn fill_both(analyzer: &mut SpectrumAnalyzer, freq_hz: f32, sample_rate: f32, amplitude: f32) {
    let (bass, main) = analyzer.windows_mut();
    fill_sine(bass, freq_hz, sample_rate, amplitude);
    fill_sine(main, freq_hz, sample_rate, amplitude);
}

/// Silence both of an analyzer's windows.
fn silence_both(analyzer: &mut SpectrumAnalyzer) {
    let (bass, main) = analyzer.windows_mut();
    bass.fill(0.0);
    main.fill(0.0);
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

// --- band_tilt_gains ---------------------------------------------------------

#[test]
fn there_is_one_tilt_gain_per_band() {
    for rate in RATES {
        let edges = band_edges(NUM_BANDS, FFT_SIZE, rate);
        let gains = band_tilt_gains(&edges, FFT_SIZE, rate);
        assert_eq!(gains.len(), NUM_BANDS, "at {rate} Hz");
        assert!(
            gains.iter().all(|g| g.is_finite() && *g > 0.0),
            "a gain was not a positive number at {rate} Hz"
        );
    }
}

#[test]
fn no_edges_yield_no_tilt_gains() {
    assert!(band_tilt_gains(&[], FFT_SIZE, 44_100.0).is_empty());
    // One edge describes no band, so it has no gain either.
    assert!(band_tilt_gains(&[4.0], FFT_SIZE, 44_100.0).is_empty());
}

#[test]
fn the_tilt_cuts_the_bass_lifts_the_treble_and_spares_the_pivot() {
    let rate = 44_100.0;
    let edges = band_edges(NUM_BANDS, FFT_SIZE, rate);
    let gains = band_tilt_gains(&edges, FFT_SIZE, rate);

    for pair in gains.windows(2) {
        if let [prev, next] = pair {
            assert!(next > prev, "the tilt must ascend, went {prev} -> {next}");
        }
    }
    assert!(gains.first().is_some_and(|&g| g < 1.0), "the bass end must be cut");
    assert!(gains.last().is_some_and(|&g| g > 1.0), "the treble end must be lifted");

    // The band holding the pivot keeps its level: it sits within half a band of
    // it, so a fraction of an octave of tilt.
    let pivot = hz_to_bin(TILT_PIVOT_HZ, FFT_SIZE, rate);
    let at_pivot = edges
        .windows(2)
        .position(|pair| matches!(pair, [lo, hi] if (*lo..*hi).contains(&pivot)));
    assert!(
        at_pivot
            .and_then(|band| gains.get(band))
            .is_some_and(|&g| linear_to_db(g).abs() < 0.5),
        "the band holding {TILT_PIVOT_HZ} Hz should be left near unity"
    );
}

#[test]
fn the_tilt_is_the_stated_number_of_db_per_octave() {
    // Edges an octave apart, so consecutive gains differ by exactly one octave.
    let rate = 44_100.0;
    let bin = |hz: f32| hz_to_bin(hz, FFT_SIZE, rate);
    let gains = band_tilt_gains(&[bin(250.0), bin(500.0), bin(1000.0), bin(2000.0)], FFT_SIZE, rate);
    assert_eq!(gains.len(), 3);
    for pair in gains.windows(2) {
        if let [prev, next] = pair {
            approx(linear_to_db(*next) - linear_to_db(*prev), TILT_DB_PER_OCTAVE);
        }
    }
}

// --- crossover_band ----------------------------------------------------------

#[test]
fn the_crossover_is_the_first_band_the_main_transform_resolves() {
    for rate in RATES {
        let edges = band_edges(NUM_BANDS, FFT_SIZE, rate);
        let split = crossover_band(&edges);
        assert!(split > 0, "the bass transform must own something at {rate} Hz");
        assert!(split < NUM_BANDS, "the main transform must own something at {rate} Hz");
        // Every band below it is narrower than a bin, every band above it is not.
        for (band, pair) in edges.windows(2).enumerate() {
            if let [lo, hi] = pair {
                assert_eq!(
                    hi - lo >= 1.0,
                    band >= split,
                    "band {band} sits on the wrong side of the crossover at {rate} Hz"
                );
            }
        }
        // And every band the main transform keeps takes the summing path, never
        // the sub-bin interpolation — an interval a bin wide contains one.
        for pair in edges.windows(2).skip(split) {
            if let [lo, hi] = pair {
                assert!(hi.floor() >= lo.ceil(), "a kept band has no whole bin at {rate} Hz");
            }
        }
    }
}

#[test]
fn a_coarser_transform_pushes_the_crossover_up() {
    // The whole point of deriving it: a higher rate widens every bin, so more
    // bands go unresolved and the bass transform has to own more of them.
    let low = crossover_band(&band_edges(NUM_BANDS, FFT_SIZE, 44_100.0));
    let high = crossover_band(&band_edges(NUM_BANDS, FFT_SIZE, 96_000.0));
    assert!(high > low, "96 kHz should need more bass bands than 44.1, got {high} vs {low}");
    // ...and a longer window resolves more of them, which is why the bass one is
    // long: at 44.1 kHz it leaves almost nothing unresolved.
    let bass = crossover_band(&band_edges(NUM_BANDS, BASS_FFT_SIZE, 44_100.0));
    assert!(bass < low, "the bass transform should resolve more, got {bass} vs {low}");
}

#[test]
fn an_unresolved_display_falls_back_to_the_bass_transform() {
    assert_eq!(crossover_band(&[]), 0);
    // Three edges, two bands, neither of them a fifth of a bin wide: nothing is
    // resolved, so the main transform is handed none of it.
    assert_eq!(crossover_band(&[1.0, 1.2, 1.4]), 2);
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
fn the_ceiling_maps_to_a_full_level() {
    approx(level_from_magnitude(db_to_linear(CEILING_DB)), 1.0);
    // Anything louder clamps rather than overshooting the bar — including full
    // scale, which now sits above the ceiling rather than on it.
    approx(level_from_magnitude(1.0), 1.0);
    approx(level_from_magnitude(4.0), 1.0);
}

#[test]
fn the_floor_maps_to_a_zero_level() {
    approx(level_from_magnitude(db_to_linear(FLOOR_DB)), 0.0);
    // Halfway up the bar is halfway between the two ends, not half the floor.
    approx(level_from_magnitude(db_to_linear(f32::midpoint(FLOOR_DB, CEILING_DB))), 0.5);
}

// --- bands_from_spectrum -----------------------------------------------------

#[test]
fn a_band_reads_the_energy_of_the_bins_it_covers() {
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
    bands_from_spectrum(&spectrum, &[1.0, 3.0, 6.0], &[1.0, 1.0], 1.0, &mut out);
    // Root-sum-square over the two bins, not the louder of them.
    approx(out[0], level_from_magnitude(0.25_f32.hypot(1.0)));
    // ...and nothing leaks into the silent band.
    approx(out[1], 0.0);
}

#[test]
fn a_lone_bin_reads_the_same_however_wide_its_band() {
    // What root-sum-square buys over a mean: a tone is not divided by the width
    // of whichever band it happens to land in, so the mids keep their bite.
    let mut spectrum = [Complex::new(0.0, 0.0); 12];
    spectrum[2] = Complex::new(db_to_linear(-40.0), 0.0);

    let mut narrow = [0.0; 1];
    bands_from_spectrum(&spectrum, &[2.0, 4.0], &[1.0], 1.0, &mut narrow);
    let mut wide = [0.0; 1];
    bands_from_spectrum(&spectrum, &[2.0, 11.0], &[1.0], 1.0, &mut wide);

    approx(wide[0], narrow[0]);
}

#[test]
fn broadband_energy_accumulates_across_a_wider_band() {
    // The other half of the same property, and the reason a peak leaned bass: a
    // band holding signal in every bin outreads a narrow one, which is the free
    // ~3 dB/octave the explicit tilt is sized on top of.
    let spectrum = [Complex::new(db_to_linear(-50.0), 0.0); 12];
    let mut narrow = [0.0; 1];
    bands_from_spectrum(&spectrum, &[2.0, 3.0], &[1.0], 1.0, &mut narrow);
    let mut wide = [0.0; 1];
    bands_from_spectrum(&spectrum, &[2.0, 6.0], &[1.0], 1.0, &mut wide);

    assert!(
        wide[0] > narrow[0],
        "broadband energy must accumulate, got {} vs {}",
        wide[0],
        narrow[0]
    );
}

#[test]
fn a_bands_tilt_gain_scales_its_level() {
    // Quiet enough that neither reading clamps at the ceiling.
    let quiet = db_to_linear(-40.0);
    let spectrum = [Complex::new(0.0, 0.0), Complex::new(quiet, 0.0), Complex::new(quiet, 0.0)];
    let mut plain = [0.0; 1];
    bands_from_spectrum(&spectrum, &[1.0, 2.0], &[1.0], 1.0, &mut plain);
    let mut lifted = [0.0; 1];
    bands_from_spectrum(&spectrum, &[1.0, 2.0], &[2.0], 1.0, &mut lifted);
    // A ×2 gain is +6 dB, which on this scale is a fixed fraction of the bar.
    approx(lifted[0] - plain[0], 6.020_6 / (CEILING_DB - FLOOR_DB));
}

#[test]
fn a_band_narrower_than_a_bin_interpolates_between_its_neighbours() {
    // Bin 1 is silent, bin 2 is loud — but quiet enough that neither reading
    // clamps at the ceiling. A band sitting entirely between them has no bin of
    // its own, so it must read the slope rather than seize either.
    let loud = db_to_linear(-45.0);
    let spectrum = [Complex::new(0.0, 0.0), Complex::new(0.0, 0.0), Complex::new(loud, 0.0)];
    let quarter = level_from_magnitude(0.25 * loud);
    let half = level_from_magnitude(0.5 * loud);

    // Centre 1.25 -> a quarter of the way from bin 1 to bin 2.
    let mut out = [0.0; 1];
    bands_from_spectrum(&spectrum, &[1.2, 1.3], &[1.0], 1.0, &mut out);
    approx(out[0], quarter);

    // Centre 1.5 -> halfway. Strictly louder than the band below it, which is the
    // property the old whole-bin snapping destroyed: both would have read bin 1.
    bands_from_spectrum(&spectrum, &[1.45, 1.55], &[1.0], 1.0, &mut out);
    approx(out[0], half);
    assert!(half > quarter, "adjacent sub-bin bands must slope, not step");
}

#[test]
fn a_sub_bin_band_on_the_last_bin_holds_rather_than_fading() {
    // Nothing above the final bin to interpolate toward — reading a phantom zero
    // there would notch the top bar on every frame.
    let spectrum = [Complex::new(0.0, 0.0), Complex::new(1.0, 0.0)];
    let mut out = [0.0; 1];
    bands_from_spectrum(&spectrum, &[1.4, 1.6], &[1.0], 1.0, &mut out);
    approx(out[0], 1.0);
}

#[test]
fn bands_beyond_the_edges_read_as_silence() {
    let spectrum = [Complex::new(1.0, 0.0); 4];
    let mut out = [0.5; 4];
    // Three edges describe two bands, leaving two output slots unclaimed. The top
    // edge stays on the last bin, as `band_edges`' clamp guarantees in production.
    bands_from_spectrum(&spectrum, &[1.0, 2.0, 3.0], &[1.0, 1.0], 1.0, &mut out);
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
    bands_from_spectrum(&spectrum, &[], &[], 1.0, &mut out);
    for (i, &level) in out.iter().enumerate() {
        approx(level, 0.0);
        assert!(level.abs() < 1e-4, "band {i} kept a stale height");
    }
}

// --- smooth ------------------------------------------------------------------

#[test]
fn levels_rise_immediately_to_a_higher_value() {
    let mut levels = [0.0, 0.2];
    smooth(&mut levels, &[1.0, 0.9], ATTACK, VISUALIZER_DECAY);
    approx(levels[0], 1.0);
    approx(levels[1], 0.9);
}

#[test]
fn levels_decay_gradually_toward_a_lower_value() {
    let mut levels = [1.0];
    let mut previous = 1.0;
    for _ in 0..8 {
        smooth(&mut levels, &[0.0], ATTACK, VISUALIZER_DECAY);
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
        smooth(&mut levels, &[0.0], ATTACK, VISUALIZER_DECAY);
    }
    approx(levels[0], 0.0);
}

#[test]
fn a_level_never_decays_below_its_band() {
    let mut levels = [1.0];
    for _ in 0..50 {
        smooth(&mut levels, &[0.4], ATTACK, VISUALIZER_DECAY);
        assert!(levels[0] >= 0.4, "decayed past the band's own level: {}", levels[0]);
    }
    approx(levels[0], 0.4);
}

// --- SpectrumAnalyzer --------------------------------------------------------

#[test]
fn both_windows_are_filled_from_one_read_of_the_newest_samples() {
    // The short window has to be the newest *tail* of the long one. Taking its
    // head instead would hand the treble transform samples a whole bass window
    // old — inaudible in every other assertion here, since both windows would
    // still hold real audio.
    let mut analyzer = SpectrumAnalyzer::new(FFT_SIZE, NUM_BANDS);
    let reads = Cell::new(0_usize);
    // A distinct value per position, so a window taken from the wrong end
    // cannot match by coincidence.
    analyzer.fill_windows(|window| {
        reads.set(reads.get() + 1);
        for (i, sample) in window.iter_mut().enumerate() {
            *sample = index_to_f32(i);
        }
    });

    assert_eq!(
        reads.get(),
        1,
        "a second read lands at a later instant, leaving the transforms on different moments"
    );

    let (bass, main) = analyzer.windows_mut();
    assert_eq!(bass.len(), BASS_FFT_SIZE);
    assert_eq!(main.len(), FFT_SIZE);
    let tail = bass.len() - main.len();
    // The scalar first: it reports "expected 6144, got 0" where the slice
    // comparison below would lead with ten thousand floats.
    approx(main[0], index_to_f32(tail));
    assert_eq!(&main[..], &bass[tail..], "the short window is not the long one's tail");
}

#[test]
fn a_sine_lands_in_the_band_containing_its_frequency() {
    let rate = 44_100;
    let mut analyzer = SpectrumAnalyzer::new(FFT_SIZE, NUM_BANDS);
    // Well under the ceiling on purpose: a full-scale tone clamps its whole
    // neighbourhood at 1.0, and the tie would leave `loudest` picking the first.
    fill_both(&mut analyzer, 1_000.0, rate_to_f32(rate), db_to_linear(-25.0));
    let levels = analyzer.analyze(rate);

    let (peak_band, peak_level) = loudest(levels);
    let tone_bin = hz_to_bin(1_000.0, FFT_SIZE, rate_to_f32(rate));
    let edges = band_edges(NUM_BANDS, FFT_SIZE, rate_to_f32(rate));
    let holds_tone = edges.windows(2).position(|pair| match pair {
        [lo, hi] => (*lo..*hi).contains(&tone_bin),
        _ => false,
    });
    assert_eq!(holds_tone, Some(peak_band));

    assert!(peak_level > 0.5, "expected a tall bar, got {peak_level}");
    // ...and the rest of the display stays down.
    assert!(levels[0] < 0.2, "bass leaked from a 1 kHz tone: {}", levels[0]);
}

#[test]
fn the_low_bars_move_independently() {
    // What the bass transform is for. At `FFT_SIZE` the bottom bands are all
    // interpolations of the same handful of bins, so a tone sitting in one of
    // them lights its neighbours just as brightly and the left of the display
    // moves as a block.
    let rate = 44_100;
    let mut analyzer = SpectrumAnalyzer::new(FFT_SIZE, NUM_BANDS);
    fill_both(&mut analyzer, 120.0, rate_to_f32(rate), db_to_linear(-25.0));
    let levels = analyzer.analyze(rate);
    let (peak, level) = loudest(levels);

    assert!((6..14).contains(&peak), "a 120 Hz tone belongs near band 10, lit {peak}");
    // Six bands out is ~45 Hz away here — well clear of the Hann main lobe, so
    // it must be dark. Closer than that it cannot be: down here the bands are
    // narrower than the lobe itself, which is a resolution limit rather than the
    // smearing this guards against. At `FFT_SIZE` alone the whole neighbourhood
    // read the same interpolated pair of bins and none of it separated.
    for band in [peak - 6, peak + 6] {
        assert!(
            levels[band] < level * 0.5,
            "band {band} reads {} against the tone's {level}: the low bars are still smeared",
            levels[band]
        );
    }
}

#[test]
fn a_band_reads_the_same_either_side_of_the_crossover() {
    // Root-sum-square makes the join self-levelling: per-bin magnitude falls as
    // 1/√N, a fixed-width band holds ∝N bins. Broadband noise is the case that
    // would show a step, since a tone lands in one bin either way.
    let rate = 44_100;
    let mut analyzer = SpectrumAnalyzer::new(FFT_SIZE, NUM_BANDS);
    {
        let (bass, main) = analyzer.windows_mut();
        // Same deterministic broadband signal in both windows.
        let noise = |i: usize| ((index_to_f32(i) * 12.9898).sin() * 43_758.547).fract() - 0.5;
        for (i, s) in bass.iter_mut().enumerate() {
            *s = noise(i) * 0.05;
        }
        for (i, s) in main.iter_mut().enumerate() {
            *s = noise(i) * 0.05;
        }
    }
    let levels = analyzer.analyze(rate);
    let split = crossover_band(&band_edges(NUM_BANDS, FFT_SIZE, rate_to_f32(rate)));

    // The two bands straddling the join come from different transforms; a
    // missing correction factor would show up as a step between them.
    let (below, above) = (levels[split - 1], levels[split]);
    assert!(
        (below - above).abs() < 0.15,
        "a step across the crossover: {below} -> {above}"
    );
}

#[test]
fn silence_produces_all_zero_bands() {
    let mut analyzer = SpectrumAnalyzer::new(FFT_SIZE, NUM_BANDS);
    silence_both(&mut analyzer);
    let levels = analyzer.analyze(44_100);
    assert_eq!(levels.len(), NUM_BANDS);
    for (i, &level) in levels.iter().enumerate() {
        assert!(level.abs() < 1e-4, "band {i} is {level} on silence");
    }
}

#[test]
fn an_unknown_sample_rate_produces_no_bands() {
    let mut analyzer = SpectrumAnalyzer::new(FFT_SIZE, NUM_BANDS);
    fill_both(&mut analyzer, 1_000.0, 44_100.0, 1.0);
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
    fill_both(&mut analyzer, 1_000.0, rate_to_f32(rate), db_to_linear(-25.0));
    let loud = loudest(analyzer.analyze(rate));

    silence_both(&mut analyzer);
    let quiet = analyzer.analyze(rate);
    assert!(quiet[loud.0] < loud.1, "the bar should start falling once the tone stops");
    assert!(quiet[loud.0] > 0.0, "it should fall gently, not cut out");
}

#[test]
fn changing_the_sample_rate_rebuilds_the_band_edges() {
    let mut analyzer = SpectrumAnalyzer::new(FFT_SIZE, NUM_BANDS);
    silence_both(&mut analyzer);
    analyzer.analyze(44_100);
    assert_eq!(analyzer.mapped_rate, 44_100);
    let first: Vec<_> = analyzer.main.edges.to_vec();

    silence_both(&mut analyzer);
    analyzer.analyze(96_000);
    assert_eq!(analyzer.mapped_rate, 96_000);
    assert_ne!(analyzer.main.edges.to_vec(), first, "band edges must follow the sample rate");
}
