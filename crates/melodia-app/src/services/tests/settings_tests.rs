use melodia_core::error::AppError;
use melodia_testkit::{reading_env, with_env_set};
// Only the desktop probes need it, and Windows has no desktop to ask.
#[cfg(not(target_os = "windows"))]
use melodia_testkit::with_env_var;

use super::*;
use crate::services::settings::{ONBOARDING_VERSION, WINDOW_BORDER_SYSTEM_COLOR, WindowBorder};

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
    let settings: SettingsData =
        reading_env(|| serde_json::from_str(json)).map_err(|e| json_err(&e))?;
    assert_eq!(settings.volume, 100);
    assert!(!settings.playback.is_muted);
    assert!(settings.playback.gapless_playback);
    Ok(())
}

// The theme ids below are spelled as literals rather than read off the registry: they are what
// `settings.json` stores, so renaming one strands every install that saved it.

#[cfg(target_os = "windows")]
#[test]
fn a_fresh_windows_install_opens_on_fluent_following_the_app_mode() {
    let settings = reading_env(SettingsData::default);

    assert_eq!(
        (
            settings.theme_id.as_str(),
            settings.theme_variant.as_str(),
            settings.accent_color.as_str()
        ),
        ("windows-fluent", "system", "blue"),
    );
}

#[cfg(not(target_os = "windows"))]
#[test]
fn a_fresh_install_on_a_desktop_without_a_theme_opens_on_catppuccin_mocha() {
    let settings = with_env_var("XDG_CURRENT_DESKTOP", None, SettingsData::default);

    assert_eq!(
        (
            settings.theme_id.as_str(),
            settings.theme_variant.as_str(),
            settings.accent_color.as_str()
        ),
        ("catppuccin", "mocha", "mauve"),
    );
}

#[cfg(target_os = "linux")]
#[test]
fn a_fresh_kde_install_opens_on_breeze_following_the_system_scheme() {
    let settings = with_env_var("XDG_CURRENT_DESKTOP", Some("KDE"), SettingsData::default);

    assert_eq!(
        (
            settings.theme_id.as_str(),
            settings.theme_variant.as_str(),
            settings.accent_color.as_str()
        ),
        ("kde-breeze", "system", "blue"),
    );
}

#[cfg(target_os = "linux")]
#[test]
fn a_fresh_gnome_install_opens_on_adwaita_following_the_system_scheme() {
    let settings = with_env_var("XDG_CURRENT_DESKTOP", Some("GNOME"), SettingsData::default);

    assert_eq!(
        (
            settings.theme_id.as_str(),
            settings.theme_variant.as_str(),
            settings.accent_color.as_str()
        ),
        ("gnome-adwaita", "system", "blue"),
    );
}

#[cfg(target_os = "linux")]
#[test]
fn only_a_fresh_kde_install_opens_on_the_native_titlebar() {
    let native_titlebar_under = |desktop| {
        with_env_var("XDG_CURRENT_DESKTOP", desktop, SettingsData::default)
            .window
            .use_native_titlebar
    };

    assert_eq!(
        [
            native_titlebar_under(Some("KDE")),
            native_titlebar_under(Some("GNOME")),
            native_titlebar_under(None)
        ],
        [true, false, false],
    );
}

/// The tint's field default is per-desktop too, but it reaches only a file missing the key, so a
/// fresh KDE install opened on the native titlebar with the tint off.
#[cfg(target_os = "linux")]
#[test]
fn only_a_fresh_kde_install_tints_its_chrome_while_unfocused() {
    let tint_under = |desktop| {
        with_env_var("XDG_CURRENT_DESKTOP", desktop, SettingsData::default)
            .layout
            .match_unfocused_to_system_bg
    };

    assert_eq!(
        [tint_under(Some("KDE")), tint_under(Some("GNOME")), tint_under(None)],
        [true, false, false],
    );
}

#[cfg(target_os = "linux")]
#[test]
fn a_fresh_install_takes_its_desktops_corner_radius() {
    let radius_under =
        |desktop| with_env_var("XDG_CURRENT_DESKTOP", desktop, SettingsData::default).corner_radius;

    assert_eq!(
        [radius_under(Some("KDE")), radius_under(Some("GNOME")), radius_under(None)],
        [6, 15, 6]
    );
}

/// Only a fresh install asks the desktop. A settings file without the key reads as
/// `WindowFlags::default()`, so an existing KDE install keeps the titlebar it has.
#[cfg(target_os = "linux")]
#[test]
fn a_kde_settings_file_without_the_titlebar_key_keeps_the_custom_titlebar() -> Result<(), AppError>
{
    let settings: SettingsData = with_env_var("XDG_CURRENT_DESKTOP", Some("KDE"), || {
        serde_json::from_str(r#"{"volume": 80}"#)
    })
    .map_err(|e| json_err(&e))?;

    assert!(!settings.window.use_native_titlebar);
    Ok(())
}

/// Every saved install carries this key, so renaming the field without an alias would reset each
/// of them to the custom titlebar.
#[test]
fn a_saved_native_titlebar_reads_back() -> Result<(), AppError> {
    let window: WindowFlags =
        serde_json::from_str(r#"{"use_native_titlebar": true}"#).map_err(|e| json_err(&e))?;

    assert!(window.use_native_titlebar);
    Ok(())
}

#[test]
fn a_fresh_install_draws_the_window_border() {
    assert_eq!(WindowFlags::default().window_border, WindowBorder::Shown);
}

#[test]
fn a_fresh_install_draws_the_window_border_in_the_system_color() {
    assert_eq!(WindowFlags::default().window_border_color, WINDOW_BORDER_SYSTEM_COLOR);
}

/// A `settings.json` written before the border existed carries no key for it, and has to read as
/// the default rather than as an install that turned the border off.
#[test]
fn a_settings_file_from_before_the_border_draws_it() -> Result<(), AppError> {
    let window: WindowFlags =
        serde_json::from_str(r#"{"use_native_titlebar": true}"#).map_err(|e| json_err(&e))?;

    assert_eq!(window.window_border, WindowBorder::Shown);
    Ok(())
}

/// Persisted as a name, the way the titlebar button tokens are, so a third state needs no schema
/// change and a hand-edited file reads the way it is spelled.
#[test]
fn the_window_border_persists_as_a_token() -> Result<(), AppError> {
    let hidden = serde_json::to_string(&WindowBorder::Hidden).map_err(|e| json_err(&e))?;

    assert_eq!(hidden, r#""hidden""#);
    Ok(())
}

#[test]
fn a_hidden_window_border_reads_back_from_its_token() -> Result<(), AppError> {
    let border: WindowBorder = serde_json::from_str(r#""hidden""#).map_err(|e| json_err(&e))?;

    assert_eq!(border, WindowBorder::Hidden);
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
    settings.volume = settings.volume.min(melodia_engine::player::engine::state::MAX_VOLUME);
    assert_eq!(settings.volume, melodia_engine::player::engine::state::MAX_VOLUME);
    Ok(())
}

#[test]
fn test_settings_roundtrip_volume_mute() -> Result<(), AppError> {
    let settings = SettingsData {
        volume: 42,
        playback: PlaybackFlags { is_muted: true, ..PlaybackFlags::default() },
        // `SettingsData::default()` reads the environment through its serde
        // defaults, so it takes the same lock the mutating tests below do.
        ..reading_env(SettingsData::default)
    };
    let json = serde_json::to_string(&settings).map_err(|e| json_err(&e))?;
    let deserialized: SettingsData = serde_json::from_str(&json).map_err(|e| json_err(&e))?;
    assert_eq!(deserialized.volume, 42);
    assert!(deserialized.playback.is_muted);
    Ok(())
}

#[test]
fn test_settings_roundtrip_playback_speed() -> Result<(), AppError> {
    let settings = SettingsData {
        playback: PlaybackFlags { playback_speed: 1.5, ..PlaybackFlags::default() },
        ..reading_env(SettingsData::default)
    };
    let json = serde_json::to_string(&settings).map_err(|e| json_err(&e))?;
    let deserialized: SettingsData = serde_json::from_str(&json).map_err(|e| json_err(&e))?;
    assert!((deserialized.playback.playback_speed - 1.5).abs() < f64::EPSILON);
    Ok(())
}

#[test]
fn test_playback_speed_defaults_when_missing() -> Result<(), AppError> {
    // Settings files written before this field existed must still load,
    // defaulting the speed to 1.0 (the `#[serde(default)]` on PlaybackFlags).
    let json = r#"{"volume": 80}"#;
    let settings: SettingsData = serde_json::from_str(json).map_err(|e| json_err(&e))?;
    assert!((settings.playback.playback_speed - 1.0).abs() < f64::EPSILON);
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
    // Set membership rather than one value, so the test holds on every host OS; which desktop
    // gets which preset is `desktop`'s own test.
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

/// `MotionFlags` is `#[serde(flatten)]`ed like every other substruct, so its key sits at
/// the top level of `settings.json` — a nested object here would be a shape change on
/// installs that already have a file.
#[test]
fn test_skip_startup_animation_defaults_false_and_reads_a_top_level_key() -> Result<(), AppError> {
    let absent: SettingsData = serde_json::from_str("{}").map_err(|e| json_err(&e))?;
    assert!(!absent.motion.skip_startup_animation);

    let present: SettingsData =
        serde_json::from_str(r#"{"skip_startup_animation": true}"#).map_err(|e| json_err(&e))?;
    assert!(present.motion.skip_startup_animation);
    Ok(())
}

#[test]
fn test_view_sort_roundtrip() -> Result<(), AppError> {
    let sort = ViewSort { field: "title".to_owned(), dir: SortDir::Desc };
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
    // Round-trip both directions through their token form. `SortDir` has no
    // `PartialEq`, so compare the tokens rather than the variants — `as_str` is
    // injective (pinned just above), so a wrong variant round-trips to a
    // different token, and the failure prints the tokens instead of an opaque
    // `Discriminant(..)`.
    for dir in [SortDir::Asc, SortDir::Desc] {
        assert_eq!(SortDir::from_token(dir.as_str()).as_str(), dir.as_str());
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
    let legacy: ThemePreference = serde_json::from_str(r#"{"variant":"mocha","accent":"mauve"}"#)
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

/// Runs `body` with only `set` present among the four variables
/// `detect_os_locale` consults, listed here in the order it consults them.
///
/// Takes the overrides rather than a bare closure so it owns the mutation, like
/// its `with_env_var` siblings — which is what leaves no `unsafe` in this file.
fn with_locale_env<F: FnOnce() -> R, R>(set: &[(&str, &str)], body: F) -> R {
    with_env_set(&["LANGUAGE", "LC_ALL", "LC_MESSAGES", "LANG"], set, body)
}

#[test]
fn test_detect_os_locale_supported_locale() {
    with_locale_env(&[("LC_ALL", "de_DE.UTF-8")], || {
        assert_eq!(detect_os_locale(), Some("de".to_owned()));
    });
}

#[test]
fn test_detect_os_locale_unsupported_locale_returns_none() {
    // "ja" is a valid ISO code but not in SUPPORTED_LOCALES.
    with_locale_env(&[("LC_ALL", "ja_JP.UTF-8")], || {
        assert_eq!(detect_os_locale(), None);
    });
}

#[test]
fn test_detect_system_locale_raw_language_var() {
    // GNU LANGUAGE takes precedence over LC_* vars
    with_locale_env(&[("LANGUAGE", "de:en"), ("LC_ALL", "en_US.UTF-8")], || {
        assert_eq!(detect_system_locale_raw(), Some("de".to_owned()));
    });
}

#[test]
fn test_detect_system_locale_raw_language_skips_empty_and_c() {
    with_locale_env(&[("LANGUAGE", "C::POSIX:de:en")], || {
        assert_eq!(detect_system_locale_raw(), Some("de".to_owned()));
    });
}

#[test]
fn test_detect_system_locale_raw_falls_through_to_lc_all() {
    // No LANGUAGE set, should fall through to LC_ALL
    with_locale_env(&[("LC_ALL", "en_US.UTF-8")], || {
        assert_eq!(detect_system_locale_raw(), Some("en_US.UTF-8".to_owned()));
    });
}

#[test]
fn test_detect_os_locale_language_picks_first_supported() {
    // LANGUAGE=fr:de:en — "fr" is unsupported, so detect_os_locale tries only the
    // first raw entry ("fr") and returns None. The LANGUAGE priority list only affects
    // which raw string detect_system_locale_raw returns.
    with_locale_env(&[("LANGUAGE", "de:en")], || {
        assert_eq!(detect_os_locale(), Some("de".to_owned()));
    });
}

#[test]
fn test_locale_roundtrip() -> Result<(), AppError> {
    let settings = SettingsData {
        locale: "fr".to_owned(),
        // The locale tests above clear and set exactly the four variables
        // `default_locale()` consults, so this read has to be under the lock too.
        ..reading_env(SettingsData::default)
    };
    let json = serde_json::to_string(&settings).map_err(|e| json_err(&e))?;
    let deserialized: SettingsData = serde_json::from_str(&json).map_err(|e| json_err(&e))?;
    assert_eq!(deserialized.locale, "fr");
    Ok(())
}

#[test]
fn test_replaygain_defaults_when_absent() -> Result<(), AppError> {
    // An older settings.json (written before ReplayGain existed) deserializes to
    // the inert defaults: off, "album" mode, 0 dB preamp, prevent-clipping on.
    let json = r#"{"theme_id": "catppuccin"}"#;
    let settings: SettingsData = serde_json::from_str(json).map_err(|e| json_err(&e))?;
    assert!(!settings.replaygain.rg_enabled);
    assert_eq!(settings.replaygain.rg_mode, "album");
    assert!((settings.replaygain.rg_preamp - 0.0).abs() < f32::EPSILON);
    assert!(settings.replaygain.rg_prevent_clipping);
    Ok(())
}

#[test]
fn test_replaygain_roundtrip() -> Result<(), AppError> {
    let settings = SettingsData {
        replaygain: ReplayGainFlags {
            rg_enabled: true,
            rg_mode: "track".to_owned(),
            rg_preamp: -3.0,
            rg_prevent_clipping: false,
        },
        ..reading_env(SettingsData::default)
    };
    let json = serde_json::to_string(&settings).map_err(|e| json_err(&e))?;
    let deserialized: SettingsData = serde_json::from_str(&json).map_err(|e| json_err(&e))?;
    assert!(deserialized.replaygain.rg_enabled);
    assert_eq!(deserialized.replaygain.rg_mode, "track");
    assert!((deserialized.replaygain.rg_preamp - (-3.0)).abs() < f32::EPSILON);
    assert!(!deserialized.replaygain.rg_prevent_clipping);
    Ok(())
}

/// Click reporting is the one field a derive would get wrong, which is why `RadioFlags` writes its
/// `Default` by hand: `false` there silently ships a directory nobody's plays are counted for, and
/// popularity ordering is what every user browses by.
#[test]
fn test_radio_defaults_when_absent() -> Result<(), AppError> {
    let json = r#"{"theme_id": "catppuccin"}"#;
    let settings: SettingsData = serde_json::from_str(json).map_err(|e| json_err(&e))?;

    assert!(!settings.radio.radio_enabled);
    assert!(!settings.radio.radio_hide_segmented);
    assert!(settings.radio.radio_send_clicks);
    Ok(())
}

/// The key this replaced shipped defaulting to `true`, so every install that has written the file
/// carries it. Renaming is what makes those installs take the new default; read back under the old
/// name they would go on hiding stations that now play.
#[test]
fn test_the_retired_hls_key_does_not_carry_forward() -> Result<(), AppError> {
    let json = r#"{"theme_id": "catppuccin", "radio_enabled": true, "radio_hide_hls": true}"#;
    let settings: SettingsData = serde_json::from_str(json).map_err(|e| json_err(&e))?;

    assert!(!settings.radio.radio_hide_segmented);
    Ok(())
}

#[test]
fn test_radio_roundtrip() -> Result<(), AppError> {
    let settings = SettingsData {
        radio: RadioFlags {
            radio_enabled: true,
            radio_hide_segmented: true,
            radio_send_clicks: false,
        },
        ..reading_env(SettingsData::default)
    };
    let json = serde_json::to_string(&settings).map_err(|e| json_err(&e))?;
    let deserialized: SettingsData = serde_json::from_str(&json).map_err(|e| json_err(&e))?;

    assert!(deserialized.radio.radio_enabled);
    assert!(deserialized.radio.radio_hide_segmented);
    assert!(!deserialized.radio.radio_send_clicks);
    Ok(())
}

/// Click reporting is opt-*out*, so an install that turned radio on before it existed has to come
/// back with it still on rather than with whatever a missing key defaults to elsewhere.
#[test]
fn test_radio_sub_toggles_survive_an_older_settings_file() -> Result<(), AppError> {
    let json = r#"{"theme_id": "catppuccin", "radio_enabled": true}"#;
    let settings: SettingsData = serde_json::from_str(json).map_err(|e| json_err(&e))?;

    assert!(settings.radio.radio_enabled);
    assert!(settings.radio.radio_send_clicks);
    Ok(())
}

/// `0` is what a file written before the card existed deserializes to, and it is the only value
/// that has to mean "owed". The comparison is `<` rather than `!=` so an install carrying a
/// revision from a *newer* build — a downgrade, or a shared home directory — is left alone rather
/// than shown a card it has already seen.
#[test]
fn the_welcome_card_is_owed_only_below_the_current_revision() {
    assert!(OnboardingFlags { onboarding_version: 0 }.needs_onboarding());
    assert!(!OnboardingFlags { onboarding_version: ONBOARDING_VERSION }.needs_onboarding());
    assert!(!OnboardingFlags { onboarding_version: ONBOARDING_VERSION + 1 }.needs_onboarding());
}

/// The field is flattened, so an install written before the card existed has no key at all — and
/// that absence is what has to read as "never seen" rather than as a parse failure.
#[test]
fn an_older_settings_file_is_owed_the_welcome_card() -> Result<(), AppError> {
    let json = r#"{"theme_id": "catppuccin"}"#;
    let settings: SettingsData = serde_json::from_str(json).map_err(|e| json_err(&e))?;

    assert_eq!(settings.onboarding.onboarding_version, 0);
    assert!(settings.onboarding.needs_onboarding());
    Ok(())
}

/// The tray is the one deliberate exception to defaults-off, and what makes it safe is that it
/// reaches nobody who already has a `settings.json`: every field serializes, so a file from any
/// previous build spells `tray_enabled` and keeps its own answer.
#[test]
fn the_tray_ships_on_but_never_overrides_a_saved_answer() -> Result<(), AppError> {
    assert!(reading_env(SettingsData::default).tray.tray_enabled);

    let json = r#"{"theme_id": "catppuccin", "tray_enabled": false}"#;
    let settings: SettingsData = serde_json::from_str(json).map_err(|e| json_err(&e))?;
    assert!(!settings.tray.tray_enabled);

    // The window still closes to a quit unless asked otherwise, tray or no tray.
    assert!(!reading_env(SettingsData::default).tray.close_to_tray);
    Ok(())
}

/// Two of the three ship off and one ships on, so a derived `Default` would be right about the
/// feature and the lookup and silently wrong about the romanization. A sheet in a script the
/// reader cannot sound out is the whole reason that one is on.
#[test]
fn the_lyrics_switches_ship_as_two_off_and_one_on() {
    let lyrics = reading_env(SettingsData::default).lyrics;

    assert!(!lyrics.lyrics_enabled, "the column opens on Up Next");
    assert!(!lyrics.lyrics_online_enabled, "an outbound feature is opt-in");
    assert!(lyrics.lyrics_romanization_shown);
}

#[test]
fn the_lyrics_switches_take_their_defaults_from_a_file_that_predates_them() -> Result<(), AppError>
{
    // They are flattened into the same object as every other key, so an older `settings.json` is
    // missing all three rather than carrying an empty section.
    let json = r#"{"theme_id": "catppuccin"}"#;
    let settings: SettingsData = serde_json::from_str(json).map_err(|e| json_err(&e))?;

    assert!(!settings.lyrics.lyrics_enabled);
    assert!(!settings.lyrics.lyrics_online_enabled);
    assert!(settings.lyrics.lyrics_romanization_shown);
    Ok(())
}

/// v0.13.0 saved the switch as the column preference it grew out of, and an install that had
/// lyrics showing must not upgrade to Up Next.
#[test]
fn a_settings_file_from_0_13_keeps_lyrics_switched_on() -> Result<(), AppError> {
    let json = r#"{"theme_id": "catppuccin", "lyrics_panel_shown": true}"#;
    let settings: SettingsData =
        reading_env(|| serde_json::from_str(json)).map_err(|e| json_err(&e))?;

    assert!(settings.lyrics.lyrics_enabled);
    Ok(())
}

#[test]
fn the_lyrics_switch_is_saved_under_its_current_name() -> Result<(), AppError> {
    let flags = LyricsFlags { lyrics_enabled: true, ..LyricsFlags::default() };

    let saved = serde_json::to_value(&flags).map_err(|e| json_err(&e))?;

    assert_eq!(saved.get("lyrics_enabled"), Some(&serde_json::Value::Bool(true)));
    assert_eq!(saved.get("lyrics_panel_shown"), None);
    Ok(())
}

#[test]
fn the_romanization_ships_on_but_never_overrides_a_saved_answer() -> Result<(), AppError> {
    // The direction a default-on field goes wrong: a reader that fills the default in over a
    // saved `false` turns the row back on at every launch.
    let json = r#"{"theme_id": "catppuccin", "lyrics_romanization_shown": false}"#;
    let settings: SettingsData = serde_json::from_str(json).map_err(|e| json_err(&e))?;

    assert!(!settings.lyrics.lyrics_romanization_shown);
    Ok(())
}

/// A one-shot marker ships false, and the direction it fails is re-running the sweep on every
/// launch: an install that has already had its backfill would queue the whole library for a
/// re-parse each time the default landed over the saved answer.
#[test]
fn the_tag_backfill_marker_ships_unset_and_survives_being_set() -> Result<(), AppError> {
    assert!(!reading_env(SettingsData::default).library.tags_backfilled);

    let predating = r#"{"theme_id": "catppuccin"}"#;
    let settings: SettingsData = serde_json::from_str(predating).map_err(|e| json_err(&e))?;
    assert!(!settings.library.tags_backfilled, "an older file has never had the pass");

    let recorded = r#"{"theme_id": "catppuccin", "tags_backfilled": true}"#;
    let settings: SettingsData = serde_json::from_str(recorded).map_err(|e| json_err(&e))?;
    assert!(settings.library.tags_backfilled);
    Ok(())
}
