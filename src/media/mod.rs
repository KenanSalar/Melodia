pub mod artwork;
pub mod cover_thumbs;
pub mod deezer;
pub mod image_decode;
pub mod itunes;
pub mod metadata;
pub mod scanner;
pub mod self_writes;
pub mod station_logo;
pub mod tag_writer;
pub mod watcher;

// A walking pin rather than a module's own tests: what it asks is where in `src/` a lofty parse
// may start, which no one file in here is positioned to answer.
#[cfg(test)]
#[path = "tests/lofty_open_tests.rs"]
mod lofty_open_tests;

/// The extensions the library scans, and the sole gate on what Melodia will ingest:
/// every decoder we compile is reachable only through an entry here.
///
/// Each is a container an encoder actually emits. That is why there is no `alac`: Apple
/// Lossless lives inside `.m4a`, so the entry would match nothing on disk while still
/// costing the walk a lookup for anything named that way. Adding one owes two
/// `wix/main.wxs` rows (`services::tests::the_msi_offers_every_audio_extension`) and,
/// where freedesktop defines a type for it, a MIME entry in the four `.desktop` sources.
pub const AUDIO_EXTENSIONS: &[&str] = &[
    "mp3", "flac", "m4a", "m4b", "aac", "ogg", "oga", "wav", "aiff", "aif", "aifc", "mka", "caf",
];

/// True when `ext` — a file extension without the dot, in any case — is one we
/// scan. The single spelling of that question; every call site (library walk,
/// watcher, import, Browse) routes through here.
///
/// Case-folded rather than lowercased: `AUDIO_EXTENSIONS` is pure ASCII, and the
/// library walk asks this for *every* file in the tree (cover art, .cue, .log,
/// .m3u, …), not just the audio ones — so lowercasing allocated a `String` per
/// walked file to answer a question that never needed one.
pub fn is_audio_extension(ext: &str) -> bool {
    AUDIO_EXTENSIONS.iter().any(|a| ext.eq_ignore_ascii_case(a))
}
