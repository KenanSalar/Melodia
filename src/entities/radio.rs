use serde::{Deserialize, Serialize};
use sqlx::FromRow;

/// A stored radio station.
///
/// Favorites, hand-typed URLs and play history are the same row at different
/// points in its life, so the flags below are what tell them apart rather than
/// the table a row sits in.
#[derive(Clone, Debug, PartialEq, FromRow, Serialize, Deserialize)]
pub struct RadioStation {
    pub id: i64,
    /// radio-browser.info's id, or `None` for a station the user typed in.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub station_uuid: Option<String>,
    pub name: String,
    pub stream_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    /// Where the logo came from, kept so a re-download can be retried after the
    /// stored file is swept.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub favicon_url: Option<String>,
    /// The logo in the shared artwork store. One of the five columns the sweep
    /// reads through, and the only one whose file cannot be re-derived from the
    /// user's own library.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artwork_path: Option<String>,
    /// The directory's free-form comma-separated tags, stored verbatim.
    pub tags: String,
    pub country_code: String,
    pub language: String,
    pub codec: String,
    /// Advertised kbps, `0` where the directory does not know. A display hint
    /// only: it is zero on a large share of live stations, so nothing may divide
    /// by it without a fallback.
    pub bitrate: i32,
    /// Segmented stream. Unplayable until Symphonia grows an MPEG-TS demuxer,
    /// and stored because a favorited station has left the directory behind.
    pub hls: bool,
    pub is_favorite: bool,
    #[serde(default, skip_serializing)]
    pub sort_key: String,
    pub date_added: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_played: Option<String>,
    pub play_count: i32,
}

/// What a caller supplies when saving a station.
///
/// The rest of [`RadioStation`] is the table's own: the id, the derived
/// `sort_key`, `date_added`, and the play stats a save must never reset. A
/// struct rather than a dozen parameters because most of them are `String` and
/// a positional list of those is a bind-order bug waiting to happen.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct NewRadioStation {
    pub station_uuid: Option<String>,
    pub name: String,
    pub stream_url: String,
    pub homepage: Option<String>,
    pub favicon_url: Option<String>,
    pub tags: String,
    pub country_code: String,
    pub language: String,
    pub codec: String,
    pub bitrate: i32,
    pub hls: bool,
}
