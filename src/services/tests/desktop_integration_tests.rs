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

/// Shell-like quoting: an unquoted path with a space is two arguments and the
/// launcher runs neither.
#[test]
fn render_desktop_quotes_a_path_with_spaces() {
    let exec = PathBuf::from("/home/User Name/.local/share/Melodia/Melodia");
    let rendered = render_desktop(TEST_DESKTOP_TEMPLATE, &exec);
    assert!(
        rendered.contains("Exec=\"/home/User Name/.local/share/Melodia/Melodia\" %F"),
        "a path with a space must be quoted with the field code left outside; got:\n{rendered}"
    );
}

/// The common case has to stay unquoted — it is what the four packaged sources
/// ship, and the drift guard compares against them.
#[test]
fn render_desktop_leaves_an_ordinary_path_unquoted() {
    let exec = PathBuf::from("/home/user/.local/share/Melodia/Melodia");
    let rendered = render_desktop(TEST_DESKTOP_TEMPLATE, &exec);
    assert!(!rendered.contains('"'), "no quoting was called for; got:\n{rendered}");
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

    // Long enough that an "always write" bug moves the mtime measurably.
    std::thread::sleep(std::time::Duration::from_millis(50));

    let wrote = test_write_if_changed(&target, payload)?;

    assert!(!wrote, "should skip when on-disk bytes match payload");
    let mtime_after = std::fs::metadata(&target)?.modified()?;
    assert_eq!(mtime_before, mtime_after, "skip path must not touch the file at all");
    Ok(())
}

#[test]
fn icon_svg_is_non_empty() {
    // Against an `include_bytes!` resolving to a moved or unreadable asset.
    assert!(!TEST_ICON_SVG.is_empty(), "compiled-in icon payload must not be empty");
    assert_eq!(TEST_ICON_SVG.first(), Some(&b'<'), "compiled-in icon must look like an SVG");
}

#[test]
fn scripts_melodia_desktop_preserves_mime_block() {
    // This file ships verbatim to DEB and to the tarball staging, while the
    // self-deploy renders the template `render_desktop_preserves_mime_block`
    // covers. They must agree, or one release gives three package formats three
    // different sets of audio associations.
    //
    // Three other regressions have bitten here: a duplicate key (which the spec
    // forbids and `desktop-file-validate` rejects), a dropped
    // `Exec`/`Icon`/`StartupWMClass`, and `Keywords=` drift.
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
        "Exec=melodia %F",
        "Icon=melodia",
        "StartupWMClass=Melodia",
        "Keywords=",
    ] {
        assert!(
            SCRIPTS_DESKTOP.contains(key),
            "scripts/Melodia.desktop missing key `{key}`; got:\n{SCRIPTS_DESKTOP}"
        );
    }

    // A key may appear at most once per group, and a copy-paste duplicating the
    // whole MIME line has shipped once. Anchored at a line start so a mention in
    // a header comment can't count.
    let mime_keys = SCRIPTS_DESKTOP.match_indices("\nMimeType=").count();
    assert_eq!(
        mime_keys, 1,
        "scripts/Melodia.desktop must have exactly one MimeType= key, found {mime_keys}; got:\n{SCRIPTS_DESKTOP}"
    );
}

#[test]
fn desktop_template_contains_expected_keys() {
    // Same defence as the icon test: a blanked template still renders, into a
    // useless `.desktop`.
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
    // The identity fields software centres key on, plus the `launchable` that
    // must match the installed desktop-id: drift there breaks the entry ↔
    // component merge, and the app lists with no name, developer or licence.
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
    // Four sources materialise a `.desktop` body independently, and drift
    // between them is silent at build time and corrosive after: an AppImage user
    // can't open audio files from a file manager, KDE shows two taskbar entries,
    // and "Open with…" offers a different set per package format.
    const APPIMAGE_SCRIPT: &str = include_str!("../../../scripts/build-appimage.sh");
    const RPM_SCRIPT: &str = include_str!("../../../scripts/build-rpm.sh");
    const SCRIPTS_DESKTOP: &str = include_str!("../../../scripts/Melodia.desktop");
    // The full set the production template carries; every format declares it or
    // the associations diverge.
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

        // The MIME block makes Melodia offerable; the field code is what makes
        // it work — without one the spec passes no filenames, so the app opens
        // and plays nothing. `%F` over `%U` (no `file://` to percent-decode) and
        // over `%f` (one process per selected file). A missing line defaults to
        // `""` and fails the same assertion, which wants the same message.
        let exec = exec_line(body).unwrap_or_default();
        assert!(
            exec.ends_with(" %F"),
            "{name}'s Exec line is `{exec}`, which carries no `%F` — the desktop environment \
             hands it no paths and the MimeType list above is decorative"
        );
    }
}

/// The `Exec=` line of a desktop-entry body, wherever it sits.
///
/// Two of the four sources are whole shell scripts, where a `contains` can be
/// satisfied by a comment about the line — the hole `strip_xml_comments` closes
/// for the MSI.
fn exec_line(body: &str) -> Option<&str> {
    body.lines().find(|line| line.starts_with("Exec="))
}

/// The fifth source is a rewriter, not a body, so the guard above can't see it:
/// `install-linux.sh` seds the `Exec=` of the *same file* the DEB ships verbatim
/// to make the command absolute. Anchored at `.*` it ate the field code too, so
/// `%F` lived in the DEB and died in the tarball with nothing able to tell.
#[test]
fn the_tarball_installer_rewrite_keeps_the_field_code() {
    const INSTALLER: &str = include_str!("../../../scripts/install-linux.sh");

    assert!(
        INSTALLER.contains("s|^Exec=[^ ]*|"),
        "install-linux.sh must replace only the command token; got:\n{INSTALLER}"
    );
    assert!(
        !INSTALLER.contains("s|^Exec=.*|"),
        "install-linux.sh's `.*` swallows everything after the command, field code included"
    );
}
