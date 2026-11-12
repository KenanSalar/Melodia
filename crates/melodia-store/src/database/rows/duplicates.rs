use melodia_core::entities::audio_quality::AudioFormat;
use melodia_core::entities::cue::CueSpan;
use melodia_core::entities::duplicates::{DuplicateCopy, KeptOutFile};

use super::by_name;

by_name!(AudioFormat { codec, bitrate, sample_rate, bit_depth });

by_name!(DuplicateCopy {
    id,
    file_path,
    file_hash,
    file_size,
    span: Option<CueSpan>,
    title,
    artist,
    album,
    disc_number,
    track_number,
    year,
    musicbrainz_track_id,
    musicbrainz_release_id,
    duration_ms,
    date_added,
    format: AudioFormat,
    dismissal,
    play_count,
    rating,
    is_favorite,
    playlist_count,
});

by_name!(KeptOutFile { id, path, excluded_at });
