//! Authoring a station: probing a hand-typed URL, saving it, and the four fields a user may
//! override on a station the directory owns.
//!
//! **Validation and the write it validates for are separable on purpose.** `library::radio_files`
//! composes [`validated_overrides`] with its own write rather than calling
//! [`set_station_overrides`], because an import of fifty stations must not fire fifty logo
//! downloads. Both halves argue it at their own definitions.

use crate::database::queries;
use crate::entities::radio;
use crate::error::AppError;
use crate::player::stream_source::{self, StationFacts};
use crate::services::radio_blocklist;
use crate::state::AppState;

use super::logos::{AnswerSeed, adopted, ask_logo_url};
use super::{directory_client, get_station, save_station, set_favorite};

/// Open a URL far enough to know it plays, and report what the server said about itself.
///
/// Behind [`directory_client`] like every other outbound call here: a probe is traffic whoever
/// is on the other end, and "off means no traffic" does not have an exception for a URL the user
/// typed.
async fn probe_station(state: &AppState, stream_url: &str) -> Result<StationFacts, AppError> {
    stream_source::probe(directory_client(state)?, stream_url).await
}

/// What to call a station nobody named.
///
/// The user's own text wins, then whatever the server calls itself, and the host last — a row
/// titled with a full stream URL is unreadable in a card and sorts under `https`.
///
/// Shared with [`super::radio_files`] so a station typed in and one imported from a nameless
/// playlist entry end up called the same thing.
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

/// Keep a station the user typed in, after proving it plays.
///
/// **The probe is the validation**, and it is deliberately the whole connect rather than a
/// reachability check: a mount that is a web page, a segmented playlist or a codec with no
/// decoder is refused here, with the dialog still open, instead of at the first click.
///
/// Starred on the way in, because a station is typed in for one reason. That is also what puts it
/// in `get_favorite_stations`, which is why the kept list needs no query of its own.
///
/// **A URL already here merges onto its row rather than adding a second one**, which is the answer
/// the other two doors already give: a browsed station can't duplicate (`station_uuid` is UNIQUE)
/// and the import skips what it already holds. Without it a re-pasted URL spends a whole probe to
/// produce a second identical card. The merge stars what it finds, so re-adding a station that was
/// only ever played promotes it into the kept list.
pub async fn add_custom_station(
    state: &AppState,
    stream_url: &str,
    name: &str,
) -> Result<i64, AppError> {
    // Ahead of both the merge and the probe: a refused station should cost no network
    // and must not star a row it may already have from before it was blocked. **The
    // URL is the only handle worth checking at this door** — the user supplies the
    // name, and a stream announces no country, language or tags to match on.
    if radio_blocklist::blocks(radio_blocklist::StationTerms {
        station_uuid: None,
        name: "",
        stream_url,
        country_code: "",
        language: "",
        codec: "",
        tags: "",
    }) {
        return Err(AppError::Validation("This station can't be added".to_owned()));
    }

    if let Some(id) = queries::radio::station_id_with_url(&state.db, stream_url).await? {
        set_favorite(state, id, true).await?;
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
        // The three the directory would have filled. A stream announces no country and no
        // language, and guessing one from the host would be wrong more often than blank is.
        country: String::new(),
        country_code: String::new(),
        language: String::new(),
        codec: facts.codec.clone(),
        bitrate: facts.bitrate,
        // Off the probe rather than assumed: a segment playlist now opens like any other mount,
        // so whether this one was segmented is something only the connect knows.
        hls: facts.hls,
    };

    let id = save_station(state, &station).await?;
    set_favorite(state, id, true).await?;
    if let Some(logo_url) = facts.logo_url.as_deref() {
        adopt_logo(state, id, logo_url).await;
    }
    Ok(id)
}

/// Record what the user says about a station, and fetch a logo where they named one.
///
/// **The only fields a directory-owned row will take from them**, and it takes them because the
/// directory is community-maintained and frequently partial: an entry carrying no homepage has no
/// website button and no way to grow one, a third of them ship no logo, and nothing is derivable
/// from a stream URL that usually belongs to a streaming provider rather than to the station. They
/// land in the `local_*` columns, which the directory never writes, so they survive the re-import
/// that follows the next play. Everything else about a browsed row stays the directory's, per
/// [`ensure_editable`].
///
/// **The fetch is this door's and not the import's**, and the asymmetry is not an optimization: a
/// station whose row already points at a logo is one [`heal_station_logo`] skips, so a
/// *replacement* URL typed here would never land on its own. Every row an import creates is new
/// and logo-less, so the refresh behind its toast heals the lot four at a time — which is why
/// `radio_files` composes [`validated_overrides`] with the write itself rather than calling this.
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

/// What the four typed fields store as, or the first refusal among them.
///
/// **Everything is checked before anything is written**, so a typo in one URL leaves the whole
/// save untouched rather than half-applied. An empty field clears its column.
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

/// A typed free-text field, or `None` where it holds nothing worth storing.
fn trimmed(value: Option<&str>) -> Option<String> {
    value.map(str::trim).filter(|text| !text.is_empty()).map(str::to_owned)
}

/// What to store for a typed website, or a refusal.
///
/// `None` is the empty field, which clears the column. Anything else has to parse as an HTTP(S)
/// URL with a host: the value ends up behind a button that opens the user's browser, so a typo is
/// caught while they are looking at the field rather than handed to `open::that_detached`. Through
/// [`crate::services::http_url`], which the logo fetch and the playlist reader share.
///
/// Normalized through `Url` rather than stored as typed, so `nidaa.fm` and a trailing-slash-less
/// spelling of the same site do not read as two different links.
pub(super) fn website_url(website: &str) -> Result<Option<String>, AppError> {
    let website = website.trim();
    if website.is_empty() {
        return Ok(None);
    }
    crate::services::http_url(website).map(|url| Some(url.to_string())).ok_or_else(|| {
        AppError::Validation("A station website must be an http:// or https:// address".into())
    })
}

/// Refuse to edit a station the directory owns.
///
/// **The refusal is the point rather than a nicety**: `save_station`'s conflict clause rewrites
/// `name` and `stream_url` from the directory on the next favorite or play of the same uuid, so
/// an edit to a browsed station would revert with nothing on screen to say why. The card only
/// offers the full editor on a custom station; this is what holds when something else asks.
/// [`set_station_overrides`] is the deliberate exception and takes no part in it, the four
/// `local_*` columns being the ones the conflict clause yields on.
pub(super) fn ensure_editable(station: &radio::RadioStation) -> Result<(), AppError> {
    if station.station_uuid.is_none() {
        return Ok(());
    }
    Err(AppError::Validation("Only a station you added by URL can be edited".to_owned()))
}

/// Rewrite a hand-typed station's name and URL.
///
/// The probe runs only when the URL actually moved, so renaming a station that happens to be off
/// air today still works — and when it did move, **everything the old mount said about itself
/// goes with it**, logo included. A repointed station keeping the previous brand's icon and
/// homepage link is the failure `keep_station` already argues against on the directory's side.
/// The `local_*` columns are outside that: a value the user typed is theirs to carry over or
/// clear, and the editor shows it on the form that repointed the station.
pub async fn update_custom_station(
    state: &AppState,
    id: i64,
    stream_url: &str,
    name: &str,
) -> Result<(), AppError> {
    let existing = get_station(state, id).await?;
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

/// Download a station's logo and point the row at it, or leave the row alone.
///
/// Failures are silent by design: a station with no logo takes the Material Symbols glyph, which
/// is what most of the directory takes anyway, and there is nothing a user could do about a dead
/// favicon host.
async fn adopt_logo(state: &AppState, id: i64, logo_url: &str) {
    if let Some(path) = ask_logo_url(state, &AnswerSeed::unseeded(), logo_url).await {
        adopted(state, id, path).await;
    }
}
