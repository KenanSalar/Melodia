use flexi_logger::LogSpecification;
use log::LevelFilter;

use super::DEFAULT_LOG_SPEC;

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
}
