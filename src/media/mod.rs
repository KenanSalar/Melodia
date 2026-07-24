pub mod artwork;
pub mod cover_thumbs;
pub mod deezer;
pub mod itunes;
pub mod metadata;
pub mod scanner;
pub mod self_writes;
pub mod tag_writer;
pub mod watcher;

pub const AUDIO_EXTENSIONS: &[&str] = &[
    "mp3", "flac", "m4a", "aac", "ogg", "wav", "alac", "aiff",
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
