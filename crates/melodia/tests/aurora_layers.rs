//! Two properties of the aurora layers no single component owns.
//!
//! One asks every layer whether it darkens the periphery, the other asks the whole Slint tree
//! whether anything writes the backdrop choice — which is Rust's to decide.

use melodia_testkit::{MIN_SLINT_SOURCES, UI_DIR, stripped_sources};

/// Nothing darkens the periphery. A vignette sat over this stack while the model was a dark surface
/// framing a bright middle; under sweeps carrying the album's colour along the *edges* it is the
/// direct inverse, neutral black over them being what the backdrop was dull for. Deleted rather than
/// defaulted off — a property nothing sets is a layer waiting to be switched back on.
#[test]
fn no_layer_darkens_the_periphery() {
    for (path, src) in stripped_sources(UI_DIR, "slint", MIN_SLINT_SOURCES) {
        assert!(!src.contains("vignette"), "{path} paints a vignette over an aurora corner");
    }
}

/// Nothing in Slint writes the backdrop choice. The `in` qualifier on `Theme.aurora-backdrop`
/// already makes a write fail to compile, so what this holds is the *reason*: the two arms publish
/// unrelated tiers, so a live flip leaves one stack painting the other's colours, and the tiers
/// decide at construction whether to build a blurred half at all. The pin outlives the qualifier.
#[test]
fn nothing_in_slint_writes_the_backdrop_choice() {
    for (path, src) in stripped_sources(UI_DIR, "slint", MIN_SLINT_SOURCES) {
        assert!(
            !src.contains("Theme.aurora-backdrop ="),
            "{path} writes `Theme.aurora-backdrop`, which the two tiers already read once at boot"
        );
    }
}
