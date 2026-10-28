//! The logo a station's own site advertises, for a directory row that names none: the field is
//! community-maintained, and the site usually still points at one from its `<head>`.
//!
//! **A URL, not a second download path**: it goes through `library::radio::fetch_logo` like any
//! `favicon_url`, so every guard applies. Asked only about a station somebody named, never a whole
//! page, which `library::radio` scopes. Nothing here logs a URL.

use crate::media::fetch::station_logo::{LOGO_REQUEST_TIMEOUT, fetchable_url};
use crate::services::net::read_capped;
use melodia_core::error::AppError;

/// A `<head>` that hasn't named an icon inside this much markup isn't going to.
const MAX_PAGE_BYTES: u64 = 512 * 1024;

/// Still worth a request when the document names nothing: plenty of sites serve only this.
const WELL_KNOWN_ICON: &str = "/favicon.ico";

/// `homepage` when the row has one, otherwise the stream's host over HTTPS: a mount's port and
/// path say nothing about where the site is, and a cleartext mount is about the audio server.
pub fn origin_for(homepage: &str, stream_url: &str) -> Option<reqwest::Url> {
    if let Ok(homepage) = fetchable_url(homepage) {
        return Some(homepage);
    }
    let host = fetchable_url(stream_url).ok()?.host_str()?.to_owned();
    reqwest::Url::parse(&format!("https://{host}/")).ok()
}

/// The absolute icon URL `origin`'s document advertises, else [`WELL_KNOWN_ICON`]. `Ok(None)` only
/// where the answer couldn't be made absolute.
pub async fn icon_url(
    client: &reqwest::Client,
    origin: &reqwest::Url,
) -> Result<Option<String>, AppError> {
    let href = advertised_href(client, origin).await?;
    let href = href.as_deref().unwrap_or(WELL_KNOWN_ICON);
    Ok(origin.join(href).ok().map(String::from))
}

async fn advertised_href(
    client: &reqwest::Client,
    origin: &reqwest::Url,
) -> Result<Option<String>, AppError> {
    let response = client
        .get(origin.clone())
        .timeout(LOGO_REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(|e| AppError::network("Station site could not be read", e))?;

    if !response.status().is_success() {
        return Ok(None);
    }

    let page = read_capped(response, "Station site document", MAX_PAGE_BYTES).await?;
    Ok(icon_href(&String::from_utf8_lossy(&page)))
}

/// The first icon the head advertises, by how deliberate the choice was: an `apple-touch-icon`,
/// then a plain `icon`, then an `og:image` share card. Only the head is read.
fn icon_href(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let head = lower.find("</head>").unwrap_or(lower.len());
    let (lower, raw) = (&lower[..head], &html[..head]);

    let mut plain_icon = None;
    for (tag_lower, tag_raw) in tags(lower, raw, "<link") {
        let Some(rel) = attribute(tag_lower, tag_raw, "rel") else {
            continue;
        };
        let rel = rel.to_ascii_lowercase();
        if !rel.contains("icon") {
            continue;
        }
        let Some(href) = attribute(tag_lower, tag_raw, "href") else {
            continue;
        };
        if rel.contains("apple-touch-icon") {
            return Some(href.to_owned());
        }
        plain_icon.get_or_insert_with(|| href.to_owned());
    }
    if plain_icon.is_some() {
        return plain_icon;
    }

    tags(lower, raw, "<meta")
        .into_iter()
        .filter(|(tag_lower, _)| tag_lower.contains("og:image"))
        .find_map(|(tag_lower, tag_raw)| attribute(tag_lower, tag_raw, "content"))
        .map(str::to_owned)
}

/// Every `name …>` span, lowercased and raw. Byte-aligned, `to_ascii_lowercase` rewriting only
/// A–Z; a `>` inside a quoted attribute costs that one tag.
fn tags<'a>(lower: &'a str, raw: &'a str, name: &str) -> Vec<(&'a str, &'a str)> {
    lower
        .match_indices(name)
        .filter_map(|(start, _)| {
            let end = start + lower[start..].find('>')? + 1;
            Some((&lower[start..end], &raw[start..end]))
        })
        .collect()
}

/// The name has to start a word, or `href` would match inside `xlink:href`.
fn attribute<'a>(tag_lower: &str, tag_raw: &'a str, name: &str) -> Option<&'a str> {
    let mut from = 0;
    while let Some(at) = tag_lower[from..].find(name) {
        let start = from + at;
        let after = start + name.len();
        if tag_lower[..start].ends_with(char::is_whitespace) && tag_lower[after..].starts_with('=')
        {
            return attribute_value(&tag_lower[after + 1..], &tag_raw[after + 1..]);
        }
        from = after;
    }
    None
}

/// The value after an `=`, quoted or bare. A trailing `/` is the tag's self-close and trimmed;
/// one anywhere else is a path, as in a minifier's `href=/logo.png`.
fn attribute_value<'a>(rest_lower: &str, rest_raw: &'a str) -> Option<&'a str> {
    let quote = rest_lower.chars().next().filter(|c| *c == '"' || *c == '\'');
    let value = if let Some(quote) = quote {
        let end = 1 + rest_raw[1..].find(quote)?;
        &rest_raw[1..end]
    } else {
        let end = rest_raw.find(|c: char| c.is_whitespace() || c == '>').unwrap_or(rest_raw.len());
        let bare = &rest_raw[..end];
        bare.strip_suffix('/').unwrap_or(bare)
    };
    (!value.is_empty()).then_some(value)
}

#[cfg(test)]
#[path = "tests/logo_discovery_tests.rs"]
mod tests;
