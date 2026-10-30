//! Authoring a station: probing a hand-typed URL, saving it, and the four fields a user may
//! override on a station the directory owns.

use crate::state::AppState;
use melodia_audio::player::source::stream_source::{self, StationFacts};
use melodia_core::entities::radio;
use melodia_core::error::AppError;
use melodia_net::services::net::radio_blocklist;
use melodia_store::database::{DbPool, queries};

use super::logos::{AnswerSeed, adopted, ask_logo_url};
use super::{directory_client, get_station, save_station, set_favorite};

/// Open a URL far enough to know it plays, and report what the server said about itself. Behind
/// the switch like every other outbound call, a URL the user typed included.
async fn probe_station(state: &AppState, stream_url: &str) -> Result<StationFacts, AppError> {
    stream_source::probe(directory_client(state)?, stream_url).await
}

/// What to call a station: the user's text, then what the server calls itself, then the host, a
/// full stream URL being unreadable on a card. Shared with [`crate::library::radio_files`] so a
/// typed station and a nameless imported one end up named alike.
pub(in crate::library) fn resolve_station_name(
    typed: &str,
    from_stream: Option<&str>,
    stream_url: &str,
) -> String {
    let typed = typed.trim();
    if !typed.is_empty() {
        return typed.to_owned();
    }
    if let Some(name) = from_stream.map(str::trim).filter(|n| !n.is_empty()) {
        return name.to_owned();
    }
    reqwest::Url::parse(stream_url)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
        .unwrap_or_else(|| stream_url.to_owned())
}

/// Keep a station the user typed in, after proving it plays, and favorite it.
///
/// **The probe is the validation**: the whole connect, so a web page, a segmented playlist or a
/// codec with no decoder is refused while the dialog is open. A URL already here merges onto its
/// row and favorites it, promoting a station that was only played.
pub async fn add_custom_station(
    state: &AppState,
    stream_url: &str,
    name: &str,
) -> Result<i64, AppError> {
    // Ahead of the merge and the probe, so a refused station costs no network and writes no row.
    // The URL is the one handle a typed station has to match on.
    if radio_blocklist::blocks(radio_blocklist::StationTerms {
        station_uuid: None,
        name: "",
        stream_url,
        country_code: "",
        language: "",
        codec: "",
        tags: "",
    }) {
        return Err(AppError::validation_msg("This station can't be added"));
    }

    if let Some(id) = merged_onto_existing(&state.db, stream_url).await? {
        return Ok(id);
    }

    let facts = probe_station(state, stream_url).await?;
    let station = radio::NewRadioStation {
        station_uuid: None,
        name: resolve_station_name(name, facts.name.as_deref(), stream_url),
        stream_url: stream_url.to_owned(),
        homepage: facts.homepage.clone(),
        favicon_url: facts.logo_url.clone(),
        tags: facts.genre.clone(),
        // A guess from the host would be wrong more often than blank is.
        country: String::new(),
        country_code: String::new(),
        language: String::new(),
        codec: facts.codec.clone(),
        bitrate: facts.bitrate,
        // A segment playlist opens like any other mount, so only the connect knows.
        hls: facts.hls,
    };

    let id = save_station(&state.db, &station).await?;
    set_favorite(&state.db, id, true).await?;
    if let Some(logo_url) = facts.logo_url.as_deref() {
        adopt_logo(state, id, logo_url).await;
    }
    Ok(id)
}

/// [`add_custom_station`]'s merge, settled before anything opens a socket.
async fn merged_onto_existing(db: &DbPool, stream_url: &str) -> Result<Option<i64>, AppError> {
    let Some(id) = queries::radio::station_id_with_url(db, stream_url).await? else {
        return Ok(None);
    };
    queries::radio::set_favorite(db, id, true).await?;
    Ok(Some(id))
}

/// Record what the user says about a station, and fetch a logo where they named one.
///
/// The only fields a directory-owned row takes from them, in the `local_*` columns the directory
/// never writes, so they survive the next re-import.
///
/// **The fetch is this door's and not the import's**: [`super::heal_station_logo`] skips a row
/// that has a logo, so a typed *replacement* would never land, where an import's rows are new and
/// healed behind its toast.
pub async fn set_station_overrides(
    state: &AppState,
    id: i64,
    form: &radio::StationOverrides,
) -> Result<(), AppError> {
    let overrides = validated_overrides(form)?;
    queries::radio::set_local_fields(&state.db, id, &overrides).await?;

    if let Some(logo_url) = overrides.logo_url.as_deref() {
        adopt_logo(state, id, logo_url).await;
    }
    Ok(())
}

/// What the four typed fields store as, or the first refusal among them, checked before anything
/// is written so a typo leaves the save untouched rather than half-applied. An empty field clears
/// its column.
pub(in crate::library) fn validated_overrides(
    form: &radio::StationOverrides,
) -> Result<radio::StationOverrides, AppError> {
    Ok(radio::StationOverrides {
        website: website_url(form.website.as_deref().unwrap_or_default())?,
        logo_url: website_url(form.logo_url.as_deref().unwrap_or_default())?,
        genre: trimmed(form.genre.as_deref()),
        country: trimmed(form.country.as_deref()),
    })
}

fn trimmed(value: Option<&str>) -> Option<String> {
    value.map(str::trim).filter(|text| !text.is_empty()).map(str::to_owned)
}

/// What to store for a typed website, or a refusal; `None` is the empty field. It ends up behind a
/// button that opens the browser, so a typo is caught at the field, and is stored normalized so
/// two spellings of one site don't read as two links.
pub(super) fn website_url(website: &str) -> Result<Option<String>, AppError> {
    let website = website.trim();
    if website.is_empty() {
        return Ok(None);
    }
    melodia_net::services::net::http_url(website).map(|url| Some(url.to_string())).ok_or_else(
        || AppError::validation_msg("A station website must be an http:// or https:// address"),
    )
}

/// Refuse to edit a station the directory owns. `save_station`'s conflict clause rewrites `name`
/// and `stream_url` on its next favorite or play, yielding only on the `local_*` columns
/// [`set_station_overrides`] writes.
pub(super) fn ensure_editable(station: &radio::RadioStation) -> Result<(), AppError> {
    if station.station_uuid.is_none() {
        return Ok(());
    }
    Err(AppError::validation_msg("Only a station you added by URL can be edited"))
}

/// Rewrite a hand-typed station's name and URL.
///
/// The probe runs only when the URL moved, so renaming a station that is off air today works.
/// When it did move, **everything the old mount said about itself goes with it**, logo included;
/// the `local_*` values the user typed stay theirs to carry over or clear.
pub async fn update_custom_station(
    state: &AppState,
    id: i64,
    stream_url: &str,
    name: &str,
) -> Result<(), AppError> {
    let existing = get_station(&state.db, id).await?;
    ensure_editable(&existing)?;
    let moved = existing.stream_url != stream_url;

    let mut edit = radio::StationEdit {
        name: resolve_station_name(name, Some(existing.name.as_str()), stream_url),
        stream_url: stream_url.to_owned(),
        homepage: existing.homepage,
        favicon_url: existing.favicon_url,
        tags: existing.tags,
        codec: existing.codec,
        bitrate: existing.bitrate,
    };

    let mut logo_url = None;
    if moved {
        let facts = probe_station(state, stream_url).await?;
        logo_url = facts.logo_url.clone();
        edit.homepage = facts.homepage;
        edit.favicon_url = facts.logo_url;
        edit.tags = facts.genre;
        edit.codec = facts.codec;
        edit.bitrate = facts.bitrate;
    }

    queries::radio::update_station(&state.db, id, &edit).await?;
    if let Some(logo_url) = logo_url.as_deref() {
        adopt_logo(state, id, logo_url).await;
    }
    Ok(())
}

/// Download a station's logo and point the row at it, or leave the row alone. Silent on failure:
/// a station with no logo takes the glyph, and nobody can fix a dead favicon host.
async fn adopt_logo(state: &AppState, id: i64, logo_url: &str) {
    if let Some(path) = ask_logo_url(state, &AnswerSeed::unseeded(), logo_url).await {
        adopted(&state.db, id, path).await;
    }
}

#[cfg(test)]
#[path = "../tests/radio_authoring_tests.rs"]
mod tests;
