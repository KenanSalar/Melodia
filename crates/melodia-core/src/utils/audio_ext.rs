//! What Melodia will ingest, asked of a file extension.

/// The sole gate on what Melodia ingests: containers an encoder emits, hence no `alac`, which
/// lives in `.m4a`. A new entry owes two `crates/melodia/wix/main.wxs` rows (pinned by
/// `crates/melodia/tests/packaging.rs`) and a MIME entry in the four `.desktop` sources where
/// freedesktop defines one.
pub const AUDIO_EXTENSIONS: &[&str] = &[
    "mp3", "flac", "m4a", "m4b", "aac", "ogg", "oga", "opus", "wav", "aiff", "aif", "aifc", "mka",
    "caf",
];

/// `ext` without the dot, in any case. Case-folded rather than lowercased, since the library walk
/// asks this of every file in the tree.
pub fn is_audio_extension(ext: &str) -> bool {
    AUDIO_EXTENSIONS.iter().any(|a| ext.eq_ignore_ascii_case(a))
}
