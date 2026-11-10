use rusqlite::types::Type;
use rusqlite::{Row, Statement};

use melodia_core::entities::browse::DirectoryTrack;
use melodia_core::entities::cue::{CueFrames, CueSpan};
use melodia_core::entities::track::{
    MostPlayedFavorite, PlaylistExportRow, ScrobbleRow, TagEditRow, Track, TrackLinks,
    TrackListRow, TrackMeta, TrackSummary,
};

use super::{FromRow, by_name};

impl FromRow for Option<CueSpan> {
    type Columns = (usize, usize);

    fn columns(statement: &Statement<'_>) -> rusqlite::Result<(usize, usize)> {
        Ok((statement.column_index("cue_start")?, statement.column_index("cue_end")?))
    }

    fn from_row(row: &Row<'_>, &(start, end): &(usize, usize)) -> rusqlite::Result<Self> {
        let Some(start_frames) = row.get::<_, Option<CueFrames>>(start)? else {
            return Ok(None);
        };
        CueSpan::new(start_frames, row.get(end)?)
            .map(Some)
            .map_err(|e| rusqlite::Error::FromSqlConversionFailure(end, Type::Integer, Box::new(e)))
    }
}

by_name!(TrackSummary {
    id,
    file_path,
    file_name,
    span: Option<CueSpan>,
    title,
    artist,
    album,
    duration_ms,
    artwork_path,
    track_number,
    disc_number,
    last_position,
    is_favorite,
    rating,
    replaygain_track_gain,
    replaygain_track_peak,
    replaygain_album_gain,
    replaygain_album_peak,
});

by_name!(TrackListRow {
    id,
    title,
    artist,
    album_artist,
    album,
    genre,
    track_number,
    disc_number,
    year,
    duration_ms,
    artwork_path,
    is_favorite,
    rating,
    album_id,
    artist_id,
    genre_id,
    sort_key,
});

by_name!(DirectoryTrack { file_path, row: TrackListRow });

by_name!(PlaylistExportRow { file_path, cue_track, file_hash, title, artist, duration_ms });

by_name!(TrackMeta { id, codec, bitrate, sample_rate, bit_depth, channels, year, genre });

by_name!(TagEditRow {
    id,
    file_path,
    cue_track,
    title,
    artist,
    album_artist,
    album,
    year,
    original_year,
    track_number,
    track_total,
    disc_number,
    disc_total,
    disc_subtitle,
    subtitle,
    comment,
    bpm,
    initial_key,
    mood,
    grouping,
    work,
    movement,
    movement_number,
    movement_total,
    language,
    copyright,
    isrc,
    artwork_path,
    codec,
    bitrate,
    sample_rate,
    bit_depth,
    channels,
    file_size,
    date_modified,
    duration_ms,
    file_hash,
});

by_name!(ScrobbleRow {
    id,
    title,
    artist,
    album,
    album_artist,
    duration_ms,
    track_number,
    musicbrainz_track_id,
    musicbrainz_release_id,
});

by_name!(TrackLinks { id, album_id, artist_id, genre_id });

by_name!(Track {
    id,
    file_path,
    file_name,
    file_hash,
    cue_track,
    span: Option<CueSpan>,
    title,
    artist,
    album_artist,
    album,
    genre,
    credits,
    track_number,
    track_total,
    disc_number,
    disc_total,
    disc_subtitle,
    subtitle,
    release_date,
    year,
    original_date,
    original_year,
    comment,
    bpm,
    initial_key,
    mood,
    grouping,
    work,
    movement,
    movement_number,
    movement_total,
    language,
    copyright,
    isrc,
    musicbrainz_track_id,
    musicbrainz_release_id,
    musicbrainz_release_track_id,
    replaygain_track_gain,
    replaygain_track_peak,
    replaygain_album_gain,
    replaygain_album_peak,
    duration_ms,
    file_size,
    codec,
    bitrate,
    channels,
    sample_rate,
    bit_depth,
    artwork_path,
    play_count,
    skip_count,
    rating,
    is_favorite,
    last_played,
    last_position,
    album_id,
    artist_id,
    genre_id,
    folder_id,
    date_added,
    date_modified,
    sort_key,
});

by_name!(MostPlayedFavorite { id, title, artist, artwork_path, play_count, is_favorite });
