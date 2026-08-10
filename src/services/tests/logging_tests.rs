use std::path::PathBuf;

use flexi_logger::{FlexiLoggerError, LogSpecification};
use log::LevelFilter;

use crate::services::describe;

use super::{DEFAULT_LOG_SPEC, newest_first};

/// The spec is a string, so a typo in a target name parses cleanly and matches
/// nothing — the app keeps logging and only the directive silently stops
/// applying. `melodia` vs `Melodia` is the live example: they are the lib and
/// bin targets, one character apart, and dropping either loses half the
/// narrative.
#[test]
fn the_default_spec_parses_into_the_directives_it_spells() {
    let parsed = LogSpecification::parse(DEFAULT_LOG_SPEC);
    assert!(parsed.is_ok(), "DEFAULT_LOG_SPEC does not parse: {DEFAULT_LOG_SPEC}");
    let Ok(spec) = parsed else { return };

    let level_of = |module: Option<&str>| {
        spec.module_filters()
            .iter()
            .find(|filter| filter.module_name.as_deref() == module)
            .map(|filter| filter.level_filter)
    };

    assert_eq!(level_of(None), Some(LevelFilter::Warn), "dependency floor");
    assert_eq!(level_of(Some("melodia")), Some(LevelFilter::Info), "lib target");
    assert_eq!(level_of(Some("Melodia")), Some(LevelFilter::Info), "bin target");
    assert_eq!(
        level_of(Some("symphonia_bundle_mp3::layer3")),
        Some(LevelFilter::Error),
        "the per-frame bit-reservoir warning would be back, at a frame a line"
    );
    // The least observable of the four: it only fires on Wayland under a desktop
    // whose button-layout answer sctk-adwaita can't parse, once per launch, for a
    // titlebar we ask winit not to draw. Nothing on this machine reports it going
    // missing, which is exactly why it is asserted here.
    assert_eq!(
        level_of(Some("sctk_adwaita::buttons")),
        Some(LevelFilter::Error),
        "the button-layout parse warning would be back, naming controls never drawn"
    );
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
