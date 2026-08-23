//! The logo a station's own site advertises, for a directory row that names none.
//!
//! Roughly a third of the directory carries no `favicon_url` at all, and a station whose row is
//! empty is not always a station with no logo: the field is community-maintained and the site
//! usually still points at one from its own `<head>`. This finds that pointer and hands it on.
//!
//! **A URL, not a second download path.** What comes back goes through
//! [`super::station_logo::fetch`] exactly as a directory-supplied `favicon_url` would, so the
//! scheme guard, the byte caps, the size floor and the tile composition all apply unchanged.
//!
//! **Kept stations only.** A browsed page would pay a page fetch per logo-less row, which is a
//! third of every page, against one extra request once for a station the user actually saved.
//! `library::radio` is where that scoping lives. Nothing here logs a URL.

use crate::error::AppError;
use crate::media::station_logo::{LOGO_REQUEST_TIMEOUT, fetchable_url, read_capped};

/// Ceiling on the page read. A `<head>` that has not named an icon inside this much markup is not
/// going to; the cap is what stops a site streaming a document into a station refresh.
const MAX_PAGE_BYTES: u64 = 512 * 1024;

/// What to try when the document names nothing. Still worth a request: the well-known path
/// predates the `<link>` and plenty of sites serve only it.
const WELL_KNOWN_ICON: &str = "/favicon.ico";

/// The site to ask, derived from what the row actually carries.
///
/// `homepage` when the directory has one. Otherwise the stream's own host, with its port and path
/// dropped: a mount on `:8811` says nothing about where the site is, and the host is the only part
/// of a stream URL that names the station's owner. HTTPS regardless of the stream's scheme, a
/// cleartext mount being a statement about the audio server rather than about the site.
pub fn origin_for(homepage: &str, stream_url: &str) -> Option<reqwest::Url> {
    if let Ok(homepage) = fetchable_url(homepage) {
        return Some(homepage);
    }
    let host = fetchable_url(stream_url).ok()?.host_str()?.to_owned();
    reqwest::Url::parse(&format!("https://{host}/")).ok()
}

/// The absolute logo URL `origin`'s document advertises, or the well-known path when it names
/// none.
///
/// `Ok(None)` only where the answer could not be made absolute. A page that refuses or carries no
/// pointer still resolves to [`WELL_KNOWN_ICON`], because the caller's next step is a fetch that
/// answers that question properly.
pub async fn icon_url(
    client: &reqwest::Client,
    origin: &reqwest::Url,
) -> Result<Option<String>, AppError> {
    let href = advertised_href(client, origin).await?;
    let href = href.as_deref().unwrap_or(WELL_KNOWN_ICON);
    Ok(origin.join(href).ok().map(String::from))
}

/// The raw href the document names, if it names one.
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

    let page = read_capped(response, MAX_PAGE_BYTES).await?;
    Ok(icon_href(&String::from_utf8_lossy(&page)))
}

/// The first icon the document advertises, as the href it was written with.
///
/// Ordered by how deliberate the choice is: an `apple-touch-icon` is a square someone picked at a
/// drawable size, a plain `icon` is whatever the tab needed, and `og:image` is a share card that at
/// least belongs to the site. Only the head is read, so a `<link>` in the body cannot outrank one
/// the author put where it belongs.
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

/// Every `name …>` span in the document, lowercased and raw.
///
/// The two slices are byte-aligned: `to_ascii_lowercase` only rewrites A–Z, so an index found in
/// one addresses the same character in the other. A `>` inside a quoted attribute cuts the span
/// short, which costs that one tag and nothing else.
fn tags<'a>(lower: &'a str, raw: &'a str, name: &str) -> Vec<(&'a str, &'a str)> {
    lower
        .match_indices(name)
        .filter_map(|(start, _)| {
            let end = start + lower[start..].find('>')? + 1;
            Some((&lower[start..end], &raw[start..end]))
        })
        .collect()
}

/// One attribute's value out of a tag span.
///
/// The name has to start a word, or `href` would match inside `xlink:href` and `rel` inside a
/// `hreflang` that happens to precede the one wanted.
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

/// The value sitting immediately after an `=`, quoted or bare.
fn attribute_value<'a>(rest_lower: &str, rest_raw: &'a str) -> Option<&'a str> {
    let quote = rest_lower.chars().next().filter(|c| *c == '"' || *c == '\'');
    let value = if let Some(quote) = quote {
        let end = 1 + rest_raw[1..].find(quote)?;
        &rest_raw[1..end]
    } else {
        let end = rest_raw
            .find(|c: char| c.is_whitespace() || c == '>' || c == '/')
            .unwrap_or(rest_raw.len());
        &rest_raw[..end]
    };
    (!value.is_empty()).then_some(value)
}
