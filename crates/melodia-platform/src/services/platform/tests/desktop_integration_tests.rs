use std::path::PathBuf;

use tempfile::tempdir;

use crate::services::platform::desktop_integration::{
    TEST_DESKTOP_ENTRY, TEST_ICON_SVG, TEST_METAINFO, render_desktop, test_write_if_changed,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn render_desktop_swaps_the_packaged_command_for_the_absolute_path() {
    let exec = PathBuf::from("/home/user/.local/share/Melodia/Melodia");
    let rendered = render_desktop(TEST_DESKTOP_ENTRY, &exec);
    assert!(
        rendered.contains("Exec=/home/user/.local/share/Melodia/Melodia %F"),
        "Exec= line must carry the absolute path and keep its field code; got:\n{rendered}"
    );
    assert!(
        !rendered.contains("Exec=melodia "),
        "the packaged command must be swapped out; got:\n{rendered}"
    );
}

#[test]
fn render_desktop_preserves_mime_block() {
    let exec = PathBuf::from("/usr/bin/melodia");
    let rendered = render_desktop(TEST_DESKTOP_ENTRY, &exec);
    assert!(rendered.contains("audio/flac"));
    assert!(rendered.contains("audio/mpeg"));
    assert!(rendered.contains("audio/mp4"));
    assert!(rendered.contains("audio/ogg"));
    // Same single-key invariant the spec mandates; mirrors the
    // assertion in `the_desktop_entry_declares_every_audio_type_once`.
    let mime_keys = rendered.match_indices("\nMimeType=").count();
    assert_eq!(
        mime_keys, 1,
        "rendered entry must have exactly one MimeType= key, found {mime_keys}; got:\n{rendered}"
    );
}

/// Shell-like quoting: an unquoted path with a space is two arguments and the
/// launcher runs neither.
#[test]
fn render_desktop_quotes_a_path_with_spaces() {
    let exec = PathBuf::from("/home/User Name/.local/share/Melodia/Melodia");
    let rendered = render_desktop(TEST_DESKTOP_ENTRY, &exec);
    assert!(
        rendered.contains("Exec=\"/home/User Name/.local/share/Melodia/Melodia\" %F"),
        "a path with a space must be quoted with the field code left outside; got:\n{rendered}"
    );
}

/// The common case stays unquoted, as `install-linux.sh` writes it, or every
/// tarball install's first launch rewrites its entry.
#[test]
fn render_desktop_leaves_an_ordinary_path_unquoted() {
    let exec = PathBuf::from("/home/user/.local/share/Melodia/Melodia");
    let rendered = render_desktop(TEST_DESKTOP_ENTRY, &exec);
    assert!(!rendered.contains('"'), "no quoting was called for; got:\n{rendered}");
}

/// The odd-looking count, pinned so it can't be "simplified" back to two: the
/// desktop-entry `string` unescape halves it before the shell-like quoting sees
/// it, so each layer needs its own.
#[test]
fn render_desktop_escapes_a_backslash_for_both_unescaping_layers() {
    let exec = PathBuf::from(r"/home/od\d/Melodia");
    let rendered = render_desktop(TEST_DESKTOP_ENTRY, &exec);
    assert!(
        rendered.contains(r#"Exec="/home/od\\\\d/Melodia" %F"#),
        "a literal backslash must survive both layers as four; got:\n{rendered}"
    );
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

/// Every Linux format installs this one file, so a type it lacks is one no format offers Melodia
/// for under "Open with". Hand-maintained rather than derived from
/// `utils::audio_ext::AUDIO_EXTENSIONS`, because the two don't correspond: freedesktop maps `.oga`
/// to four types and `.caf` to none.
#[test]
fn the_desktop_entry_declares_every_audio_type_once() {
    const AUDIO_TYPES: &[&str] = &[
        "audio/mpeg",
        "audio/flac",
        "audio/x-flac",
        "audio/mp4",
        "audio/x-m4a",
        "audio/x-m4b",
        "audio/ogg",
        "audio/x-vorbis+ogg",
        "audio/x-flac+ogg",
        "audio/x-opus+ogg",
        "audio/wav",
        "audio/x-wav",
        "audio/aac",
        "audio/x-aac",
        "audio/aiff",
        "audio/x-aiff",
        "audio/x-aifc",
        "audio/x-matroska",
        "application/x-cue",
    ];

    let declared: Vec<&str> = TEST_DESKTOP_ENTRY
        .lines()
        .find_map(|line| line.strip_prefix("MimeType="))
        .map_or_default(|types| types.trim_suffix(';').split(';').collect());
    assert_eq!(
        declared, AUDIO_TYPES,
        "every Linux format offers Melodia for exactly the types this line lists"
    );

    // A key may appear at most once per group, and a copy-paste duplicating the
    // whole MIME line has shipped once. Anchored at a line start so a mention in
    // a header comment can't count.
    let mime_keys = TEST_DESKTOP_ENTRY.match_indices("\nMimeType=").count();
    assert_eq!(
        mime_keys, 1,
        "the desktop entry must have exactly one MimeType= key, found {mime_keys}; got:\n{TEST_DESKTOP_ENTRY}"
    );
}

/// A blanked or trimmed entry still installs, into a launcher tile that can't start Melodia or
/// group its window.
#[test]
fn the_desktop_entry_carries_the_keys_launchers_read() {
    for key in &[
        "Type=Application",
        "Name=Melodia",
        "Icon=melodia",
        "StartupWMClass=com.github.kenansalar.melodia",
        "Keywords=",
        // The command every package installs, which `render_desktop` swaps out. `%F` is what
        // hands the app the files it was opened with: `%U` would need `file://` decoded, and
        // `%f` starts one process per file.
        "Exec=melodia %F",
    ] {
        assert!(
            TEST_DESKTOP_ENTRY.contains(key),
            "the desktop entry is missing `{key}`; got:\n{TEST_DESKTOP_ENTRY}"
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

/// `install-linux.sh` seds the `Exec=` of the *same file* every package ships, to
/// make the command absolute. Anchored at `.*` it ate the field code too, so `%F`
/// lived in the packages and died in the tarball with nothing able to tell.
#[test]
fn the_tarball_installer_rewrite_keeps_the_field_code() {
    const INSTALLER: &str = include_str!("../../../../../../packaging/install-linux.sh");

    assert!(
        INSTALLER.contains("s|^Exec=[^ ]*|"),
        "install-linux.sh must replace only the command token; got:\n{INSTALLER}"
    );
    assert!(
        !INSTALLER.contains("s|^Exec=.*|"),
        "install-linux.sh's `.*` swallows everything after the command, field code included"
    );
}
