use std::path::PathBuf;

use tempfile::tempdir;

use crate::services::desktop_integration::{
    TEST_DESKTOP_TEMPLATE, TEST_ICON_SVG, TEST_METAINFO, render_desktop, test_write_if_changed,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn render_desktop_substitutes_exec_placeholder_with_absolute_path() {
    let exec = PathBuf::from("/home/user/.local/share/Melodia/Melodia");
    let rendered = render_desktop(TEST_DESKTOP_TEMPLATE, &exec);
    assert!(
        rendered.contains("Exec=/home/user/.local/share/Melodia/Melodia"),
        "Exec= line must carry the absolute path; got:\n{rendered}"
    );
    assert!(!rendered.contains("@EXEC@"), "placeholder must be substituted; got:\n{rendered}");
}

#[test]
fn render_desktop_preserves_mime_block() {
    let exec = PathBuf::from("/usr/bin/melodia");
    let rendered = render_desktop(TEST_DESKTOP_TEMPLATE, &exec);
    // Spot-check a couple of representative MIME types — if these
    // ever drift, the package-manager copies and the self-installed
    // copies will diverge and users will lose audio-file
    // associations.
    assert!(rendered.contains("audio/flac"));
    assert!(rendered.contains("audio/mpeg"));
    assert!(rendered.contains("audio/mp4"));
    assert!(rendered.contains("audio/ogg"));
    // Same single-key invariant the spec mandates; mirrors the
    // assertion in `scripts_melodia_desktop_preserves_mime_block`.
    let mime_keys = rendered.match_indices("\nMimeType=").count();
    assert_eq!(
        mime_keys, 1,
        "rendered template must have exactly one MimeType= key, found {mime_keys}; got:\n{rendered}"
    );
}

#[test]
fn render_desktop_handles_path_with_spaces() {
    // Slint patches `Exec=` lines on a single line in the .desktop
    // spec; spaces in the path are NOT escaped in the rendered
    // output. Freedesktop's spec says Exec lines with spaces in the
    // command path need shell-style quoting — but the common case
    // (no spaces) is fine, and the AppImage/tarball install paths
    // we target don't usually have spaces. This test documents the
    // current behavior so a future fix is intentional.
    let exec = PathBuf::from("/home/User Name/.local/share/Melodia/Melodia");
    let rendered = render_desktop(TEST_DESKTOP_TEMPLATE, &exec);
    assert!(rendered.contains("Exec=/home/User Name/.local/share/Melodia/Melodia"));
}

#[test]
fn write_if_changed_writes_when_missing() -> TestResult {
    let dir = tempdir()?;
    let target = dir.path().join("nested").join("melodia.desktop");
    assert!(!target.exists());

    let payload = b"hello world";
    let wrote = test_write_if_changed(&target, payload)?;

    assert!(wrote, "should write when destination is missing");
    assert_eq!(std::fs::read(&target)?, payload);
    Ok(())
}

#[test]
fn write_if_changed_writes_when_content_differs() -> TestResult {
    let dir = tempdir()?;
    let target = dir.path().join("melodia.desktop");
    std::fs::write(&target, b"v0.1.0 desktop entry")?;

    let new_payload = b"v0.2.0 desktop entry -- new MIME types added";
    let wrote = test_write_if_changed(&target, new_payload)?;

    assert!(wrote, "should write when content differs");
    assert_eq!(std::fs::read(&target)?, new_payload);
    Ok(())
}

#[test]
fn write_if_changed_skips_when_content_matches() -> TestResult {
    let dir = tempdir()?;
    let target = dir.path().join("melodia.desktop");
    let payload = b"identical bytes";
    std::fs::write(&target, payload)?;
    let mtime_before = std::fs::metadata(&target)?.modified()?;

    // Sleep briefly so a buggy "always write" would produce a
    // distinguishable mtime change.
    std::thread::sleep(std::time::Duration::from_millis(50));

    let wrote = test_write_if_changed(&target, payload)?;

    assert!(!wrote, "should skip when on-disk bytes match payload");
    let mtime_after = std::fs::metadata(&target)?.modified()?;
    assert_eq!(mtime_before, mtime_after, "skip path must not touch the file at all");
    Ok(())
}

#[test]
fn icon_svg_is_non_empty() {
    // Defends against an empty `include_bytes!` resolving to an
    // unreadable / moved asset path at build time.
    assert!(!TEST_ICON_SVG.is_empty(), "compiled-in icon payload must not be empty");
    // SVG files start with `<` (XML declaration or root element).
    assert_eq!(TEST_ICON_SVG.first(), Some(&b'<'), "compiled-in icon must look like an SVG");
}

#[test]
fn scripts_melodia_desktop_preserves_mime_block() {
    // The `scripts/Melodia.desktop` file ships verbatim to .deb
    // installs (per `Cargo.toml`'s `[package.metadata.deb] assets`)
    // and to the tarball staging in CI. The in-app self-deploy uses
    // a different file (`assets/desktop/Melodia.desktop.tmpl`)
    // covered by `render_desktop_preserves_mime_block`. Both need
    // to agree on the MIME block, or the same release on RPM vs DEB
    // vs tarball produces inconsistent audio-file associations.
    //
    // Beyond MIME coverage, this test guards three other regressions
    // that have bitten us before: (1) duplicate keys — the FreeDesktop
    // Desktop Entry Specification forbids the same key appearing
    // twice in a group and `desktop-file-validate` rejects it;
    // (2) accidental removal of `Exec`/`Icon`/`StartupWMClass`, any
    // of which silently breaks the launcher; (3) `Keywords=` drift —
    // the launcher search ergonomics depend on it.
    const SCRIPTS_DESKTOP: &str = include_str!("../../../scripts/Melodia.desktop");

    for mime in &[
        "audio/mpeg",
        "audio/flac",
        "audio/mp4",
        "audio/ogg",
        "audio/wav",
        "audio/aac",
    ] {
        assert!(
            SCRIPTS_DESKTOP.contains(mime),
            "scripts/Melodia.desktop missing MIME `{mime}`; got:\n{SCRIPTS_DESKTOP}"
        );
    }

    for key in &[
        "Type=Application",
        "Exec=melodia",
        "Icon=melodia",
        "StartupWMClass=Melodia",
        "Keywords=",
    ] {
        assert!(
            SCRIPTS_DESKTOP.contains(key),
            "scripts/Melodia.desktop missing key `{key}`; got:\n{SCRIPTS_DESKTOP}"
        );
    }

    // Per the Desktop Entry Spec, a key may appear at most once per
    // group. Counting line-starts of `MimeType=` catches the exact
    // bug shipped (and reverted) on 2026-05-18: a copy-paste that
    // duplicated the entire MIME line. `\nMimeType=` skips any
    // appearance of `MimeType=` inside the file's header comments
    // (none today, but cheap defense).
    let mime_keys = SCRIPTS_DESKTOP.match_indices("\nMimeType=").count();
    assert_eq!(
        mime_keys, 1,
        "scripts/Melodia.desktop must have exactly one MimeType= key, found {mime_keys}; got:\n{SCRIPTS_DESKTOP}"
    );
}

#[test]
fn desktop_template_contains_expected_keys() {
    // Same defence as the icon test: an accidentally-blanked
    // template file would render but produce a useless .desktop.
    for key in &[
        "Type=Application",
        "Name=Melodia",
        "Icon=melodia",
        "Exec=@EXEC@",
        "StartupWMClass=Melodia",
        "Keywords=",
    ] {
        assert!(
            TEST_DESKTOP_TEMPLATE.contains(key),
            "compiled-in template missing `{key}`; got:\n{TEST_DESKTOP_TEMPLATE}"
        );
    }
}

#[test]
fn metainfo_declares_expected_component() {
    // The compiled-in AppStream MetaInfo is deployed for per-user
    // tarball installs and shipped verbatim by the RPM, DEB and
    // AppImage. Guard the identity fields software centres key on, and
    // the `launchable` that must match the installed desktop file's id
    // (`com.github.kenansalar.melodia.desktop`) — a drift there breaks
    // the desktop-entry ↔ component merge and the app renders with no
    // name / developer / licence in KDE Discover and GNOME Software.
    for needle in &[
        "<id>com.github.kenansalar.melodia</id>",
        "<name>Melodia</name>",
        "<project_license>AGPL-3.0-or-later</project_license>",
        "<launchable type=\"desktop-id\">com.github.kenansalar.melodia.desktop</launchable>",
        "<developer id=\"com.github.kenansalar\">",
    ] {
        assert!(
            TEST_METAINFO.contains(needle),
            "metainfo missing `{needle}`; got:\n{TEST_METAINFO}"
        );
    }
}

#[test]
fn all_desktop_sources_agree_on_mime_and_wmclass() {
    // Four sources independently materialise a `.desktop` body — the
    // template compiled into the binary (self-deploy + DEB),
    // `scripts/Melodia.desktop` (verbatim DEB asset + tarball),
    // and two heredocs inside the AppImage + RPM build scripts.
    // Drift between them is silent at build time and corrosive at
    // runtime: a user on AppImage can't open audio files from a file
    // manager, KDE shows two taskbar entries, "Open with…" lists
    // a different set of apps depending on which package format the
    // user installed from. This test fails the build if any source
    // diverges from the canonical MIME + WMClass invariants.
    const APPIMAGE_SCRIPT: &str = include_str!("../../../scripts/build-appimage.sh");
    const RPM_SCRIPT: &str = include_str!("../../../scripts/build-rpm.sh");
    const SCRIPTS_DESKTOP: &str = include_str!("../../../scripts/Melodia.desktop");
    // Full 13-MIME-type set the production template carries. Every
    // packaged form must declare the same set or "Open with…"
    // associations diverge across formats.
    const CANONICAL_MIME_TYPES: &[&str] = &[
        "audio/mpeg",
        "audio/flac",
        "audio/x-flac",
        "audio/mp4",
        "audio/x-m4a",
        "audio/ogg",
        "audio/x-vorbis+ogg",
        "audio/wav",
        "audio/x-wav",
        "audio/aac",
        "audio/x-aac",
        "audio/aiff",
        "audio/x-aiff",
    ];

    let sources = [
        ("template", TEST_DESKTOP_TEMPLATE),
        ("scripts/Melodia.desktop", SCRIPTS_DESKTOP),
        ("scripts/build-appimage.sh heredoc", APPIMAGE_SCRIPT),
        ("scripts/build-rpm.sh heredoc", RPM_SCRIPT),
    ];

    for (name, body) in &sources {
        for mime in CANONICAL_MIME_TYPES {
            assert!(
                body.contains(mime),
                "{name} missing MIME `{mime}` — drift between desktop sources will produce \
                 inconsistent file-manager associations across package formats"
            );
        }
        assert!(
            body.contains("StartupWMClass=Melodia"),
            "{name} missing `StartupWMClass=Melodia` — KDE shows two taskbar entries without it",
        );
        assert!(
            body.contains("Keywords="),
            "{name} missing `Keywords=` — launcher search loses fuzzy matches",
        );
    }
}
