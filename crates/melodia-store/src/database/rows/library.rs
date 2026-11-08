use melodia_core::entities::album::{AlbumStats, ReleaseTagRow};
use melodia_core::entities::artist::{Artist, ArtistStats, FavoriteArtist};
use melodia_core::entities::folder::Folder;
use melodia_core::entities::genre::GenreStats;
use melodia_core::entities::playlist::{Playlist, PlaylistStats};

use super::by_name;

by_name!(AlbumStats {
    id,
    name,
    sort_name,
    artist_id,
    artist_name,
    year,
    disc_count,
    is_compilation,
    musicbrainz_id,
    musicbrainz_release_group_id,
    label,
    catalog_number,
    barcode,
    media,
    release_type,
    release_country,
    artwork_path,
    track_count,
    total_duration_ms,
});

by_name!(ReleaseTagRow {
    label,
    catalog_number,
    barcode,
    media,
    release_type,
    release_country,
    is_compilation,
});

by_name!(Artist { id, name, sort_name, musicbrainz_id, image_path });

by_name!(ArtistStats {
    id,
    name,
    sort_name,
    musicbrainz_id,
    image_path,
    track_count,
    album_count,
    total_duration_ms,
});

by_name!(FavoriteArtist { id, name, image_path, favorite_count });

by_name!(GenreStats { id, name, track_count, total_duration_ms });

by_name!(Folder { id, path, is_enabled, last_scanned, added_at });

by_name!(Playlist {
    id,
    name,
    description,
    thumbnail_path,
    is_smart,
    smart_criteria,
    created_at,
    updated_at,
    custom_thumbnail,
});

by_name!(PlaylistStats {
    id,
    name,
    description,
    thumbnail_path,
    is_smart,
    smart_criteria,
    created_at,
    updated_at,
    custom_thumbnail,
    track_count,
    total_duration_ms,
});
