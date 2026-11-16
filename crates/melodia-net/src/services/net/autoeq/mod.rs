//! `AutoEq`'s published results: the list of recommended profiles, and one model's parametric file.
//!
//! A list entry is data off the network, so [`profile_url`] builds a model's address from its
//! decoded segments and refuses one that lands outside the results folder.

mod list;

pub use list::parse_list;

use std::time::Duration;

use melodia_core::entities::correction::AutoEqEntry;
use melodia_core::error::AppError;
use melodia_core::utils::text::from_utf8_lossy_owned;
use reqwest::Url;

use super::{CappedGet, IfChanged, get_capped_if_changed, get_capped_text, http_url};

/// The trailing `/` is what keeps a join inside the folder.
const RESULTS_BASE: &str = "https://raw.githubusercontent.com/jaakkopasanen/AutoEq/master/results/";

/// The recommended list, which keeps one measurement per model.
const LIST_FILE: &str = "README.md";

/// Follows the model's folder name to make its parametric file's.
const PROFILE_SUFFIX: &str = " ParametricEQ.txt";

/// A few times the list as published, which grows with every model measured.
const LIST_GET: CappedGet = CappedGet {
    what: "AutoEq's headphone list",
    timeout: Duration::from_secs(30),
    max_bytes: 2 * 1024 * 1024,
};

/// A profile is a line per filter.
const PROFILE_GET: CappedGet =
    CappedGet { what: "AutoEq profile", timeout: Duration::from_secs(15), max_bytes: 8 * 1024 };

/// What [`fetch_list`] found.
#[derive(Debug)]
pub enum ListFetch {
    NotModified,
    Fresh { text: String, etag: Option<String> },
}

/// The list, sending `cached_etag` so an unchanged one costs an empty `304`.
pub async fn fetch_list(
    client: &reqwest::Client,
    cached_etag: Option<&str>,
) -> Result<ListFetch, AppError> {
    let url = results_base()?
        .join(LIST_FILE)
        .map_err(|e| AppError::network("AutoEq's list URL could not be built", e))?;
    Ok(match get_capped_if_changed(client, &url, cached_etag, &LIST_GET).await? {
        IfChanged::NotModified => ListFetch::NotModified,
        IfChanged::Fresh { body, etag } => {
            ListFetch::Fresh { text: from_utf8_lossy_owned(body), etag }
        }
    })
}

pub async fn fetch_profile(
    client: &reqwest::Client,
    entry: &AutoEqEntry,
) -> Result<String, AppError> {
    get_capped_text(client, &profile_url(entry)?, &PROFILE_GET).await
}

/// Each segment escaped as it is pushed, the file name as one segment of its own.
fn profile_url(entry: &AutoEqEntry) -> Result<Url, AppError> {
    let base = results_base()?;
    let model = entry.path.rsplit('/').next().unwrap_or_default();
    let mut url = base.clone();
    url.path_segments_mut()
        .map_err(|()| AppError::network_msg("AutoEq's results base takes no path"))?
        .pop_if_empty()
        .extend(entry.path.split('/'))
        .push(&format!("{model}{PROFILE_SUFFIX}"));

    let inside = url.origin() == base.origin() && url.path().starts_with(base.path());
    if !inside {
        return Err(AppError::network_msg("an AutoEq list entry points outside its results"));
    }
    Ok(url)
}

fn results_base() -> Result<Url, AppError> {
    http_url(RESULTS_BASE)
        .ok_or_else(|| AppError::network_msg("AutoEq's results base is not an http(s) URL"))
}
