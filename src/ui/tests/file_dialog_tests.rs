/// Every native dialog in the tree, and the source it is built from.
///
/// A dialog is a handful of builder calls, so the temptation at a sixth site is
/// to spell `AsyncFileDialog::new()` inline and be done — which works, and is
/// wrong in a way no review on this machine can see.
const CALLERS: [(&str, &str); 5] = [
    (
        "callbacks/library_settings.rs",
        include_str!("../callbacks/library_settings.rs"),
    ),
    (
        "callbacks/playlists/files/import.rs",
        include_str!("../callbacks/playlists/files/import.rs"),
    ),
    (
        "callbacks/playlists/files/export.rs",
        include_str!("../callbacks/playlists/files/export.rs"),
    ),
    ("callbacks/tags.rs", include_str!("../callbacks/tags.rs")),
    ("diagnostics.rs", include_str!("../diagnostics.rs")),
];

/// The parenting is what stops the OS picker opening *behind* Melodia on
/// Windows and macOS, and it is unobservable on Linux — the XDG portal parents
/// OS-side whatever we hand it. So a call site that builds its own dialog is
/// correct on the platform it is written and reviewed on, and wrong on the two
/// it is not. Reaching for the helper is the whole guarantee; this is the check
/// that it was reached for.
#[test]
fn every_native_dialog_is_built_by_the_shared_helper() {
    for (name, source) in CALLERS {
        assert!(
            !source.contains("AsyncFileDialog::new()"),
            "{name} builds its own dialog — use `ui::file_dialog::parented`, \
             which is what parents it to the main window"
        );
        assert!(
            !source.contains("set_parent"),
            "{name} parents a dialog by hand; the helper owns that call"
        );
        assert!(
            source.contains("file_dialog::parented("),
            "{name} is listed as a native-dialog caller but no longer opens one — \
             drop it from CALLERS"
        );
    }
}
