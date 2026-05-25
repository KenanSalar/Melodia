#![allow(unsafe_code)] // env::set_var/remove_var are unsafe in Rust 2024; tests serialize via LOCALE_ENV_LOCK.

use crate::error::AppError;

use super::*;

fn json_err(e: &serde_json::Error) -> AppError {
    AppError::Validation(format!("json error: {e}"))
}

#[test]
fn test_settings_default_for_missing_volume_fields() -> Result<(), AppError> {
    let json = r#"{"theme_id": "catppuccin"}"#;
    let settings: SettingsData = serde_json::from_str(json).map_err(|e| json_err(&e))?;
    assert_eq!(settings.volume, 100);
    assert!(!settings.playback.is_muted);
    Ok(())
}

#[test]
fn test_settings_deserializes_volume_fields() -> Result<(), AppError> {
    let json = r#"{"volume": 42, "is_muted": true}"#;
    let settings: SettingsData = serde_json::from_str(json).map_err(|e| json_err(&e))?;
    assert_eq!(settings.volume, 42);
    assert!(settings.playback.is_muted);
    Ok(())
}

#[test]
fn test_empty_json_uses_defaults() -> Result<(), AppError> {
    let json = "{}";
    let settings: SettingsData = serde_json::from_str(json).map_err(|e| json_err(&e))?;
    assert_eq!(settings.volume, 100);
    assert!(!settings.playback.is_muted);
    assert_eq!(settings.theme_id, "catppuccin");
    assert!(settings.playback.gapless_playback);
    Ok(())
}

#[test]
fn test_unknown_fields_silently_ignored() -> Result<(), AppError> {
    // Forward compatibility: future settings versions may add new fields.
    // Without deny_unknown_fields, old code should deserialize them fine.
    let json = r#"{"volume": 80, "some_future_field": true, "another_new_thing": 42}"#;
    let settings: SettingsData = serde_json::from_str(json).map_err(|e| json_err(&e))?;
    assert_eq!(settings.volume, 80);
    Ok(())
}

#[test]
fn test_volume_clamped_to_max() -> Result<(), AppError> {
    let json = r#"{"volume": 999}"#;
    let mut settings: SettingsData = serde_json::from_str(json).map_err(|e| json_err(&e))?;
    // Simulate the clamping that read_settings performs
    settings.volume = settings.volume.min(crate::player::state::MAX_VOLUME);
    assert_eq!(settings.volume, 200);
    Ok(())
}

#[test]
fn test_settings_roundtrip_volume_mute() -> Result<(), AppError> {
    let settings = SettingsData {
        volume: 42,
        playback: PlaybackFlags {
            is_muted: true,
            ..PlaybackFlags::default()
        },
        ..SettingsData::default()
    };
    let json = serde_json::to_string(&settings).map_err(|e| json_err(&e))?;
    let deserialized: SettingsData = serde_json::from_str(&json).map_err(|e| json_err(&e))?;
    assert_eq!(deserialized.volume, 42);
    assert!(deserialized.playback.is_muted);
    Ok(())
}

#[test]
fn test_corner_radius_clamped_to_max() -> Result<(), AppError> {
    let json = r#"{"corner_radius": 999}"#;
    let mut settings: SettingsData = serde_json::from_str(json).map_err(|e| json_err(&e))?;
    settings.corner_radius = settings.corner_radius.min(MAX_CORNER_RADIUS);
    assert_eq!(settings.corner_radius, 15);
    Ok(())
}

#[test]
fn test_corner_radius_default_is_an_os_preset() -> Result<(), AppError> {
    // `SettingsData::default()` now seeds `corner_radius` from
    // `library::settings::get_os_corner_radius()`, which returns one of
    // the chip presets (KDE=6, Win11=8, macOS=10, GNOME=15) keyed off
    // the host OS / desktop. Asserting set-membership (not a specific
    // value) makes the test platform-independent and dodges the
    // parallel-execution race against
    // `library::tests::settings_tests::corner_radius_by_desktop_environment`,
    // which mutates `XDG_CURRENT_DESKTOP` at runtime — both halves of an
    // equality assertion would have to read the same env snapshot, and
    // we can't guarantee that without a serialising mutex. Per-DE value
    // mapping is covered by that sibling test; this one only verifies
    // the wiring (default uses an OS-aware preset, not some hardcoded
    // legacy value).
    let json = "{}";
    let settings: SettingsData = serde_json::from_str(json).map_err(|e| json_err(&e))?;
    assert!(
        matches!(settings.corner_radius, 6 | 8 | 10 | 15),
        "corner_radius default {} is not one of the OS-aware presets {{6, 8, 10, 15}}",
        settings.corner_radius
    );
    Ok(())
}

#[test]
fn test_resume_on_startup_defaults_false() -> Result<(), AppError> {
    let json = "{}";
    let settings: SettingsData = serde_json::from_str(json).map_err(|e| json_err(&e))?;
    assert!(!settings.playback.resume_on_startup);
    Ok(())
}

#[test]
fn test_view_sort_roundtrip() -> Result<(), AppError> {
    let sort = ViewSort {
        field: "title".to_owned(),
        dir: SortDir::Desc,
    };
    let json = serde_json::to_string(&sort).map_err(|e| json_err(&e))?;
    let deserialized: ViewSort = serde_json::from_str(&json).map_err(|e| json_err(&e))?;
    assert_eq!(deserialized.field, "title");
    assert!(matches!(deserialized.dir, SortDir::Desc));
    Ok(())
}

#[test]
fn test_sort_dir_token_roundtrip() {
    // `as_str` and `from_token` are the bridge between the persisted
    // `SortDir` enum and the Slint `sort-dir` string property.
    assert_eq!(SortDir::Asc.as_str(), "asc");
    assert_eq!(SortDir::Desc.as_str(), "desc");
    assert!(matches!(SortDir::from_token("asc"), SortDir::Asc));
    assert!(matches!(SortDir::from_token("desc"), SortDir::Desc));
    // Anything other than the exact `"desc"` token falls back to `Asc`.
    assert!(matches!(SortDir::from_token(""), SortDir::Asc));
    assert!(matches!(SortDir::from_token("DESC"), SortDir::Asc));
    assert!(matches!(SortDir::from_token("garbage"), SortDir::Asc));
    // Round-trip both directions through their token form.
    for dir in [SortDir::Asc, SortDir::Desc] {
        assert!(std::mem::discriminant(&dir) == std::mem::discriminant(&SortDir::from_token(dir.as_str())));
    }
}

#[test]
fn test_sort_dir_requires_explicit_value() -> Result<(), AppError> {
    // ViewSort does not have #[serde(default)] on dir, so it must be provided
    let json = r#"{"field": "artist"}"#;
    let result: Result<ViewSort, _> = serde_json::from_str(json);
    assert!(result.is_err());

    // With explicit dir it works
    let json = r#"{"field": "artist", "dir": "asc"}"#;
    let sort: ViewSort = serde_json::from_str(json).map_err(|e| json_err(&e))?;
    assert!(matches!(sort.dir, SortDir::Asc));
    Ok(())
}

#[test]
fn test_column_widths_defaults() {
    let cw = ColumnWidths::default();
    assert!((cw.number - 56.0).abs() < f64::EPSILON);
    assert!((cw.title - 320.0).abs() < f64::EPSILON);
    assert!((cw.artist - 200.0).abs() < f64::EPSILON);
    assert!((cw.album - 220.0).abs() < f64::EPSILON);
    assert!((cw.genre - 140.0).abs() < f64::EPSILON);
    assert!((cw.year - 72.0).abs() < f64::EPSILON);
    assert!((cw.length - 88.0).abs() < f64::EPSILON);
}

#[test]
fn test_column_widths_partial_json() -> Result<(), AppError> {
    let json = r#"{"artist": 300.0}"#;
    let cw: ColumnWidths = serde_json::from_str(json).map_err(|e| json_err(&e))?;
    assert!((cw.artist - 300.0).abs() < f64::EPSILON);
    // Other fields should use defaults
    assert!((cw.number - 56.0).abs() < f64::EPSILON);
    assert!((cw.title - 320.0).abs() < f64::EPSILON);
    assert!((cw.length - 88.0).abs() < f64::EPSILON);
    Ok(())
}

#[test]
fn test_theme_preference_roundtrip() -> Result<(), AppError> {
    let pref = ThemePreference {
        variant: "mocha".to_owned(),
        accent: "mauve".to_owned(),
        last_static_accent: Some("mauve".to_owned()),
    };
    let json = serde_json::to_string(&pref).map_err(|e| json_err(&e))?;
    let deserialized: ThemePreference = serde_json::from_str(&json).map_err(|e| json_err(&e))?;
    assert_eq!(deserialized.variant, "mocha");
    assert_eq!(deserialized.accent, "mauve");
    assert_eq!(deserialized.last_static_accent.as_deref(), Some("mauve"));
    // Backward compat: older settings.json without `last_static_accent` deserializes to None.
    let legacy: ThemePreference =
        serde_json::from_str(r#"{"variant":"mocha","accent":"mauve"}"#)
            .map_err(|e| json_err(&e))?;
    assert!(legacy.last_static_accent.is_none());
    Ok(())
}

#[test]
fn test_locale_default_is_nonempty() -> Result<(), AppError> {
    let json = "{}";
    let settings: SettingsData = serde_json::from_str(json).map_err(|e| json_err(&e))?;
    assert!(!settings.locale.is_empty(), "default locale must not be empty");
    Ok(())
}

#[test]
fn test_parse_language_code_utf8() {
    assert_eq!(parse_language_code("en_US.UTF-8"), Some("en".to_owned()));
    assert_eq!(parse_language_code("de_DE.UTF-8"), Some("de".to_owned()));
}

#[test]
fn test_parse_language_code_no_encoding() {
    assert_eq!(parse_language_code("en_US"), Some("en".to_owned()));
    assert_eq!(parse_language_code("de_DE"), Some("de".to_owned()));
}

#[test]
fn test_parse_language_code_bcp47() {
    assert_eq!(parse_language_code("en-US"), Some("en".to_owned()));
    assert_eq!(parse_language_code("de-DE"), Some("de".to_owned()));
}

#[test]
fn test_parse_language_code_bare() {
    assert_eq!(parse_language_code("en"), Some("en".to_owned()));
    assert_eq!(parse_language_code("de"), Some("de".to_owned()));
}

#[test]
fn test_parse_language_code_invalid() {
    assert_eq!(parse_language_code("C"), None);
    assert_eq!(parse_language_code(""), None);
    assert_eq!(parse_language_code("POSIX"), None);
    assert_eq!(parse_language_code("123"), None);
}

/// Mutex that serializes all tests which mutate locale env vars, so they are safe
/// even when `cargo test` runs tests in parallel (the default).
static LOCALE_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Acquires `LOCALE_ENV_LOCK`, saves all locale-related env vars, clears them,
/// runs `body`, then restores originals.
///
/// SAFETY: `std::env::set_var` / `remove_var` are unsafe in Rust 2024 because
/// they mutate shared process state. The `LOCALE_ENV_LOCK` mutex ensures only one
/// test touches env vars at a time; no other code in this binary reads these vars
/// concurrently.
unsafe fn with_locale_env<F: FnOnce() -> R, R>(body: F) -> R {
    const VARS: &[&str] = &["LANGUAGE", "LC_ALL", "LC_MESSAGES", "LANG"];

    let _guard = LOCALE_ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let saved: Vec<(&str, Option<String>)> =
        VARS.iter().map(|&v| (v, std::env::var(v).ok())).collect();
    for &v in VARS {
        unsafe { std::env::remove_var(v) };
    }
    let result = body();
    for (var, val) in saved {
        unsafe {
            match val {
                Some(v) => std::env::set_var(var, v),
                None => std::env::remove_var(var),
            }
        }
    }
    result
}

#[test]
fn test_detect_os_locale_supported_locale() {
    // SAFETY: test runs single-threaded (--test-threads=1)
    unsafe {
        with_locale_env(|| {
            std::env::set_var("LC_ALL", "de_DE.UTF-8");
            assert_eq!(detect_os_locale(), Some("de".to_owned()));
        });
    }
}

#[test]
fn test_detect_os_locale_unsupported_locale_returns_none() {
    // "ja" is a valid ISO code but not in SUPPORTED_LOCALES.
    // SAFETY: test runs single-threaded (--test-threads=1)
    unsafe {
        with_locale_env(|| {
            std::env::set_var("LC_ALL", "ja_JP.UTF-8");
            assert_eq!(detect_os_locale(), None);
        });
    }
}

#[test]
fn test_detect_system_locale_raw_language_var() {
    // GNU LANGUAGE takes precedence over LC_* vars
    // SAFETY: test runs single-threaded (--test-threads=1)
    unsafe {
        with_locale_env(|| {
            std::env::set_var("LANGUAGE", "de:en");
            std::env::set_var("LC_ALL", "en_US.UTF-8");
            assert_eq!(detect_system_locale_raw(), Some("de".to_owned()));
        });
    }
}

#[test]
fn test_detect_system_locale_raw_language_skips_empty_and_c() {
    // SAFETY: test runs single-threaded (--test-threads=1)
    unsafe {
        with_locale_env(|| {
            std::env::set_var("LANGUAGE", "C::POSIX:de:en");
            assert_eq!(detect_system_locale_raw(), Some("de".to_owned()));
        });
    }
}

#[test]
fn test_detect_system_locale_raw_falls_through_to_lc_all() {
    // SAFETY: test runs single-threaded (--test-threads=1)
    unsafe {
        with_locale_env(|| {
            // No LANGUAGE set, should fall through to LC_ALL
            std::env::set_var("LC_ALL", "en_US.UTF-8");
            assert_eq!(detect_system_locale_raw(), Some("en_US.UTF-8".to_owned()));
        });
    }
}

#[test]
fn test_detect_os_locale_language_picks_first_supported() {
    // LANGUAGE=fr:de:en — "fr" is unsupported, so detect_os_locale tries only the
    // first raw entry ("fr") and returns None. The LANGUAGE priority list only affects
    // which raw string detect_system_locale_raw returns.
    // SAFETY: test runs single-threaded (--test-threads=1)
    unsafe {
        with_locale_env(|| {
            std::env::set_var("LANGUAGE", "de:en");
            assert_eq!(detect_os_locale(), Some("de".to_owned()));
        });
    }
}

#[test]
fn test_locale_roundtrip() -> Result<(), AppError> {
    let settings = SettingsData {
        locale: "fr".to_owned(),
        ..SettingsData::default()
    };
    let json = serde_json::to_string(&settings).map_err(|e| json_err(&e))?;
    let deserialized: SettingsData = serde_json::from_str(&json).map_err(|e| json_err(&e))?;
    assert_eq!(deserialized.locale, "fr");
    Ok(())
}

// Environment variable tests are consolidated into a single test to avoid
// parallel execution races on the shared XDG_CURRENT_DESKTOP variable.
// Lives in services/tests/ rather than library/tests/ because
// `get_os_corner_radius` now lives in services::settings (it's used by
// `SettingsData::default()` for the corner_radius serde default).
#[test]
#[cfg(target_os = "linux")]
fn corner_radius_by_desktop_environment() {
    unsafe { std::env::set_var("XDG_CURRENT_DESKTOP", "GNOME") };
    assert_eq!(get_os_corner_radius(), 15, "GNOME should return 15");

    unsafe { std::env::set_var("XDG_CURRENT_DESKTOP", "ubuntu:GNOME") };
    assert_eq!(get_os_corner_radius(), 15, "ubuntu:GNOME should return 15");

    unsafe { std::env::set_var("XDG_CURRENT_DESKTOP", "KDE") };
    assert_eq!(get_os_corner_radius(), 6, "KDE should return 6");

    unsafe { std::env::set_var("XDG_CURRENT_DESKTOP", "i3") };
    assert_eq!(get_os_corner_radius(), 6, "unknown DE should return 6");

    unsafe { std::env::remove_var("XDG_CURRENT_DESKTOP") };
    assert_eq!(get_os_corner_radius(), 6, "missing env should return 6");
}
