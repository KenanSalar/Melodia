/// A test binary carries `debug_assertions`, so this is the source-build answer. It pins the gate
/// rather than the mapping: without it a release lands on a binary cargo rebuilds. Gated on that
/// arm because a test binary reaches no other, `target/<profile>/deps/` putting the profile where
/// the path check looks for `target`, so `--release` would invert the assert.
#[cfg(debug_assertions)]
#[test]
fn a_source_build_has_no_in_app_updater() {
    assert!(!super::is_available());
}
