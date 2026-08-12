use std::path::PathBuf;

use flexi_logger::{FlexiLoggerError, LogSpecification};
use log::LevelFilter;

use crate::services::describe;

use super::{NORMAL_LEVEL, VERBOSE_LEVEL, newest_first, spec_for};

/// The spec is a string, so a typo in a target name parses cleanly and matches
/// nothing — the app keeps logging and only the directive silently stops
/// applying. `melodia` vs `Melodia` is the live example: they are the lib and
/// bin targets, one character apart, and dropping either loses half the
/// narrative.
///
/// Both levels, since the switch reaches the second at runtime and nothing else
/// would notice it malformed. The mutation that matters is a verbose spec built
/// without `SPEC_TAIL` — that unmutes `layer3`, a warning per decoded frame.
#[test]
fn both_specs_parse_into_the_directives_they_spell() {
    for (level, expected) in [
        (NORMAL_LEVEL, LevelFilter::Info),
        (VERBOSE_LEVEL, LevelFilter::Debug),
    ] {
        let spec_str = spec_for(level);
        let parsed = LogSpecification::parse(&spec_str);
        assert!(parsed.is_ok(), "the {level} spec does not parse: {spec_str}");
        let Ok(spec) = parsed else { continue };

        let level_of = |module: Option<&str>| {
            spec.module_filters()
                .iter()
                .find(|filter| filter.module_name.as_deref() == module)
                .map(|filter| filter.level_filter)
        };

        assert_eq!(level_of(None), Some(LevelFilter::Warn), "{level}: dependency floor");
        assert_eq!(level_of(Some("melodia")), Some(expected), "{level}: lib target");
        assert_eq!(level_of(Some("Melodia")), Some(expected), "{level}: bin target");
        assert_eq!(
            level_of(Some("symphonia_bundle_mp3::layer3")),
            Some(LevelFilter::Error),
            "{level}: the per-frame bit-reservoir warning would be back, at a frame a line"
        );
        // The least observable of the four: it only fires on Wayland under a desktop
        // whose button-layout answer sctk-adwaita can't parse, once per launch, for a
        // titlebar we ask winit not to draw. Nothing on this machine reports it going
        // missing, which is exactly why it is asserted here.
        assert_eq!(
            level_of(Some("sctk_adwaita::buttons")),
            Some(LevelFilter::Error),
            "{level}: the button-layout parse warning would be back, naming controls never drawn"
        );
    }
}

/// The two levels have to actually differ, or the switch is a no-op that still
/// logs "verbose logging enabled" and persists a flag.
#[test]
fn the_verbose_level_is_louder_than_the_normal_one() {
    assert_ne!(NORMAL_LEVEL, VERBOSE_LEVEL);
}

/// `services::diagnostics` spends a byte budget down this list, so the order is
/// the whole contract. `LoggerHandle::existing_log_files` ends with a plain
/// `sort()`, so the rotated files arrive **oldest** first however `FileSpec`
/// happens to have ordered them — the mutation to catch is the `reverse()`
/// going missing, which leaves the budget on the oldest log Melodia still has.
#[test]
fn the_rotated_files_follow_the_live_one_newest_first() {
    let path = |name: &str| PathBuf::from("/logs").join(name);
    // What the handle hands back: ascending, and under `Naming::Numbers` a
    // higher index is the newer file.
    let rotated = vec![
        path("melodia_r00000.log"),
        path("melodia_r00001.log"),
        path("melodia_r00002.log"),
    ];

    let ordered = newest_first(vec![path("melodia_rCURRENT.log")], rotated);

    assert_eq!(
        ordered,
        [
            path("melodia_rCURRENT.log"),
            path("melodia_r00002.log"),
            path("melodia_r00001.log"),
            path("melodia_r00000.log"),
        ]
    );
}

/// The recorded reason is the whole of what a diagnostics bundle can say about a
/// sink that never started, and every `FlexiLoggerError` message is a static
/// sentence — so without the chain, a root-owned log file and a full disk report
/// the same thing. Both halves are asserted: the second is the contract, the
/// first says why it is needed and reports if upstream ever starts interpolating.
#[test]
fn the_recorded_reason_carries_the_io_cause() {
    let error = FlexiLoggerError::from(std::io::Error::from(std::io::ErrorKind::PermissionDenied));
    let described = describe(&error);

    assert!(
        !error.to_string().to_lowercase().contains("permission denied"),
        "flexi_logger now interpolates its source; describe() may have stopped earning its place"
    );
    assert!(
        described.to_lowercase().contains("permission denied"),
        "the cause is the whole message: {described}"
    );
}

/// A first run has rotated nothing, and a logger that failed to reach a file has
/// neither half — neither may cost the list its shape.
#[test]
fn a_missing_half_is_not_a_problem() {
    let current = PathBuf::from("/logs/melodia_rCURRENT.log");

    assert_eq!(newest_first(vec![current.clone()], Vec::new()), [current]);
    assert!(newest_first(Vec::new(), Vec::new()).is_empty());
}
