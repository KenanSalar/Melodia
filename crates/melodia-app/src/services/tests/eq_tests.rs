//! `eq.json`, where the curves a user builds up live: every device's correction surviving a
//! restart, a hand edit unable to hand the DSP a bad one, and the equalizer's curve moved out of
//! the `settings.json` a released build wrote without losing it.

use serde_json::{Map, Value, json};

use super::{move_legacy_eq, mutate_eq, read_eq};
use crate::services::settings::read_settings;
use crate::state::fixtures::seeded_root;
use melodia_core::config::Paths;
use melodia_core::entities::correction::{
    Correction, CorrectionFilter, CorrectionFilters, CorrectionOrigin, FilterKind,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const PEAK: CorrectionFilter =
    CorrectionFilter { kind: FilterKind::Peaking, freq_hz: 105.0, gain_db: 6.5, q: 0.7 };
const LOW_SHELF: CorrectionFilter =
    CorrectionFilter { kind: FilterKind::LowShelf, freq_hz: 105.0, gain_db: 6.0, q: 0.7 };
const HIGH_SHELF: CorrectionFilter =
    CorrectionFilter { kind: FilterKind::HighShelf, freq_hz: 10_000.0, gain_db: -2.5, q: 0.7 };

const CARD: &str = "hw:CARD=UMC22,DEV=0";

/// A correction's origin, device name and filters, to compare two without `PartialEq`.
type Shown = (String, Option<String>, Vec<CorrectionFilter>);

fn shown(correction: Option<&Correction>) -> Option<Shown> {
    let correction = correction?;
    let origin = match &correction.origin {
        CorrectionOrigin::AutoEq { model, source } => format!("AutoEq {model} by {source}"),
        CorrectionOrigin::File { name } => format!("file {name}"),
        CorrectionOrigin::Paste => "paste".to_owned(),
    };
    Some((origin, correction.device_name.clone(), correction.filters.filters().to_vec()))
}

/// A tag rename or a serde shape that writes what it can't read back would reset the map at the
/// next boot, every device's correction with it.
#[test]
fn every_devices_correction_reads_back_from_eq_json() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let paths = Paths::rooted_at(tmp.path().to_path_buf());
    let headphones = Correction {
        origin: CorrectionOrigin::AutoEq {
            model: "Beyerdynamic DT 990 Pro (250 Ohm)".to_owned(),
            source: "crinacle".to_owned(),
        },
        device_name: None,
        filters: CorrectionFilters::try_from(vec![PEAK])?,
    };
    let card = Correction {
        origin: CorrectionOrigin::File { name: "Sundara ParametricEQ.txt".to_owned() },
        device_name: Some("UMC22".to_owned()),
        filters: CorrectionFilters::try_from(vec![LOW_SHELF, HIGH_SHELF])?,
    };
    mutate_eq(&paths, move |eq| {
        eq.corrections.insert(None, headphones);
        eq.corrections.insert(Some(CARD.to_owned()), card);
    })?;

    let read = read_eq(&paths)?.corrections;

    let expected = (
        Some(("AutoEq Beyerdynamic DT 990 Pro (250 Ohm) by crinacle".to_owned(), None, vec![PEAK])),
        Some((
            "file Sundara ParametricEQ.txt".to_owned(),
            Some("UMC22".to_owned()),
            vec![LOW_SHELF, HIGH_SHELF],
        )),
    );
    assert_eq!((shown(read.for_device(None)), shown(read.for_device(Some(CARD)))), expected);
    Ok(())
}

/// A hand edit can write a filter out of range, a Q of zero here. Only that key resets, so the
/// equalizer's curve beside it survives the edit.
#[test]
fn a_correction_the_dsp_cannot_run_resets_alone_in_eq_json() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let paths = Paths::rooted_at(tmp.path().to_path_buf());
    let hand_edited = json!({
        "band_gains": [3.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        "preset": "Custom",
        "preamp": -2.0,
        "corrections": {
            "system_default": {
                "origin": "paste",
                "device_name": null,
                "filters": [{ "kind": "peaking", "freq_hz": 1000.0, "gain_db": 3.0, "q": 0.0 }],
            },
            "devices": {},
        },
    });
    std::fs::write(&paths.eq_path, hand_edited.to_string())?;

    let eq = read_eq(&paths)?;

    let read = (eq.corrections.for_device(None).is_some(), eq.preset.as_str(), eq.preamp.to_bits());
    assert_eq!(read, (false, "Custom", (-2.0_f32).to_bits()), "(correction kept, preset, preamp)");
    Ok(())
}

/// The keys a released build wrote the curve under.
const LEGACY_KEYS: [&str; 3] = ["eq_band_gains", "eq_selected_preset", "eq_preamp"];
const BASS_BOOST: [f32; 10] = [6.0, 5.0, 4.0, 2.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0];

fn settings_object(paths: &Paths) -> Result<Map<String, Value>, Box<dyn std::error::Error>> {
    match serde_json::from_str(&std::fs::read_to_string(&paths.settings_path)?)? {
        Value::Object(fields) => Ok(fields),
        other => Err(format!("settings.json is not an object: {other}").into()),
    }
}

/// A data root whose `settings.json` is a released build's, the curve still in it.
fn released_root() -> Result<(tempfile::TempDir, Paths), Box<dyn std::error::Error>> {
    let (tmp, paths) = seeded_root()?;
    let mut fields = settings_object(&paths)?;
    fields.insert("eq_band_gains".to_owned(), json!(BASS_BOOST));
    fields.insert("eq_selected_preset".to_owned(), json!("Bass Boost"));
    fields.insert("eq_preamp".to_owned(), json!(-3.0));
    std::fs::write(&paths.settings_path, Value::Object(fields).to_string())?;
    Ok((tmp, paths))
}

fn curve(paths: &Paths) -> Result<(Vec<f32>, String, u32), Box<dyn std::error::Error>> {
    let eq = read_eq(paths)?;
    Ok((eq.band_gains, eq.preset, eq.preamp.to_bits()))
}

fn legacy_keys_left(paths: &Paths) -> Result<Vec<&'static str>, Box<dyn std::error::Error>> {
    let fields = settings_object(paths)?;
    Ok(LEGACY_KEYS.into_iter().filter(|key| fields.contains_key(*key)).collect())
}

#[test]
fn an_older_settings_jsons_curve_moves_into_eq_json() -> TestResult {
    let (_tmp, paths) = released_root()?;

    move_legacy_eq(&paths, &read_settings(&paths)?)?;

    assert_eq!(
        curve(&paths)?,
        (BASS_BOOST.to_vec(), "Bass Boost".to_owned(), (-3.0_f32).to_bits())
    );
    Ok(())
}

/// Left in, they would read as the live curve to anyone editing `settings.json`, and move back
/// over the curve the day `eq.json` went missing.
#[test]
fn a_moved_curve_leaves_settings_json() -> TestResult {
    let (_tmp, paths) = released_root()?;

    move_legacy_eq(&paths, &read_settings(&paths)?)?;

    assert_eq!(legacy_keys_left(&paths)?, Vec::<&str>::new());
    Ok(())
}

#[test]
fn an_eq_json_already_there_wins_over_an_older_curve() -> TestResult {
    let (_tmp, paths) = released_root()?;
    mutate_eq(&paths, |eq| {
        eq.band_gains = vec![1.0; 10];
        eq.preset = "Custom".to_owned();
        eq.preamp = -1.0;
    })?;

    move_legacy_eq(&paths, &read_settings(&paths)?)?;

    assert_eq!(curve(&paths)?, (vec![1.0; 10], "Custom".to_owned(), (-1.0_f32).to_bits()));
    Ok(())
}

/// Only `eq.json`'s folder is locked, so `settings.json` stays writable and the order of the two
/// writes alone decides.
#[cfg(unix)]
#[test]
fn a_curve_that_cannot_reach_eq_json_stays_in_settings_json() -> TestResult {
    use std::os::unix::fs::PermissionsExt;

    let (tmp, mut paths) = released_root()?;
    let locked = tmp.path().join("locked");
    std::fs::create_dir(&locked)?;
    paths.eq_path = locked.join("eq.json");
    let settings = read_settings(&paths)?;
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555))?;
    // Root, or a filesystem ignoring the mode, writes anyway, and then nothing here can fail.
    if writes_anyway(&locked) {
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755))?;
        return Ok(());
    }

    let moved = move_legacy_eq(&paths, &settings);

    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755))?;
    assert!(moved.is_err(), "the write into a read-only folder has to fail for this to test it");
    assert_eq!(legacy_keys_left(&paths)?, LEGACY_KEYS.to_vec());
    Ok(())
}

#[cfg(unix)]
fn writes_anyway(dir: &std::path::Path) -> bool {
    let probe = dir.join("mode-check");
    let written = std::fs::File::create(&probe).is_ok();
    let _ = std::fs::remove_file(&probe);
    written
}
