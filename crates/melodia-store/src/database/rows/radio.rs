use melodia_core::entities::radio::{RadioStation, StoredLogoAnswer};

use super::by_name;

by_name!(RadioStation {
    id,
    station_uuid,
    name,
    stream_url,
    homepage,
    local_homepage,
    favicon_url,
    local_favicon_url,
    local_tags,
    local_country,
    artwork_path,
    tags,
    country,
    country_code,
    language,
    codec,
    probed_codec,
    bitrate,
    hls,
    is_favorite,
    sort_key,
    date_added,
    last_played,
    play_count,
});

by_name!(StoredLogoAnswer { favicon_url, artwork_path, retry_after });
