//! Streaming download with HTTP-range resume, ETag-aware `If-Range` freshness, and a 5 %-slack
//! size bound that aborts a runaway transfer before it can fill the user's disk.

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

use futures_util::StreamExt;

use melodia_core::error::{AppError, AppResult, describe};

use super::staging::{
    StagedMeta, discard_staging_if_sidecar_mismatches, sidecar_meta_path, write_staged_meta,
};

/// 5 % over the manifest's size absorbs a CDN that over-reports `Content-Length`, while a runaway
/// transfer stops there rather than after filling the disk and failing its signature.
pub(crate) fn exceeds_size_bound(downloaded: u64, expected_size: u64) -> bool {
    // Saturating toward "too big", which rejects a giant claim rather than accepting one.
    let bound = expected_size.saturating_mul(105) / 100;
    downloaded > bound
}

/// The `ETag` if it is a strong tag. RFC 9110 forbids a weak one in `If-Range`, and a server
/// handed one evaluates it false, forcing a full re-download on every resume.
pub(crate) fn capture_strong_etag(header_value: Option<&str>) -> Option<String> {
    header_value.filter(|tag| !tag.starts_with("W/")).map(str::to_owned)
}

/// What an existing `dest` means as a download starts. `Skip` is a file at the manifest's size,
/// such as a verified package whose install was cancelled, and is verified again regardless.
/// `Resume` is a partial one, restarting if the server answers 200. `Fresh` covers no file, an
/// empty one and one larger than expected.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ResumeState {
    Skip,
    Resume(u64),
    Fresh,
}

pub(crate) fn plan_resume(existing_size: u64, expected_size: u64) -> ResumeState {
    if expected_size == 0 {
        // A malformed manifest; fresh lets the size bound catch whatever is served.
        return ResumeState::Fresh;
    }
    if existing_size == 0 {
        return ResumeState::Fresh;
    }
    if existing_size == expected_size {
        return ResumeState::Skip;
    }
    if existing_size < expected_size {
        return ResumeState::Resume(existing_size);
    }
    ResumeState::Fresh
}

/// The fingerprint [`download_to_file`] writes as it starts and again whenever the `ETag` moves.
struct Sidecar<'a> {
    path: PathBuf,
    version: &'a str,
    size: u64,
    url: &'a str,
}

impl Sidecar<'_> {
    fn write(&self, etag: Option<&str>) -> AppResult<()> {
        write_staged_meta(
            &self.path,
            &StagedMeta {
                version: self.version.to_string(),
                size: self.size,
                asset_url: self.url.to_string(),
                etag: etag.map(str::to_owned),
            },
        )
    }
}

/// What `dest` already holds as a download starts.
struct Existing {
    size: u64,
    /// The previous attempt's `ETag`, which a resume sends back as `If-Range`.
    etag: Option<String>,
    resume: ResumeState,
}

pub(super) async fn download_to_file(
    http: &reqwest::Client,
    url: &str,
    expected_version: &str,
    expected_size: u64,
    dest: &Path,
    on_progress: &(impl Fn(u8) + Send + Sync),
) -> AppResult<()> {
    let sidecar = Sidecar {
        path: sidecar_meta_path(dest),
        version: expected_version,
        size: expected_size,
        url,
    };
    let existing = prepare_resume(dest, &sidecar);
    if matches!(existing.resume, ResumeState::Skip) {
        log::info!(
            "updater: skipping download — {} already has {} bytes (matches manifest); \
             verify step will validate",
            dest.display(),
            existing.size
        );
        on_progress(100);
        return Ok(());
    }

    let resp = build_request(http, url, &existing, expected_size)
        .send()
        .await
        .map_err(|e| AppError::network(format!("update download GET {url} failed"), e))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(AppError::network_msg(format!("update download {url} returned HTTP {status}")));
    }

    let response_etag = capture_strong_etag(
        resp.headers().get(reqwest::header::ETAG).and_then(|v| v.to_str().ok()),
    );

    let (file, downloaded) =
        open_destination(dest, &existing, status, response_etag.as_deref(), &sidecar)?;
    stream_to_file(resp, file, dest, downloaded, expected_size, on_progress).await
}

/// Decide what to do with what `dest` already holds, and write the sidecar for it.
fn prepare_resume(dest: &Path, sidecar: &Sidecar<'_>) -> Existing {
    // Before any size decision: bytes another release left can't be trusted to skip or resume.
    let etag =
        discard_staging_if_sidecar_mismatches(dest, sidecar.version, sidecar.size, sidecar.url)
            .and_then(|m| m.etag);

    let size = std::fs::metadata(dest).map_or(0, |m| m.len());
    let resume = plan_resume(size, sidecar.size);

    // Written before the GET, carrying the old etag, so a kill mid-stream leaves a fingerprint the
    // next attempt resumes against. Best-effort: without one that attempt restarts from zero.
    if let Err(e) = sidecar.write(etag.as_deref()) {
        log::warn!(
            "updater: failed to write staging sidecar at {}: {} (download continues; \
             worst case a future retry restarts from zero)",
            sidecar.path.display(),
            describe(&e)
        );
    }
    Existing { size, etag, resume }
}

fn build_request(
    http: &reqwest::Client,
    url: &str,
    existing: &Existing,
    expected_size: u64,
) -> reqwest::RequestBuilder {
    let mut req = http.get(url);
    if let ResumeState::Resume(offset) = existing.resume {
        log::info!(
            "updater: resuming download at byte {offset} (have {} of {expected_size})",
            existing.size
        );
        req = req.header(reqwest::header::RANGE, format!("bytes={offset}-"));
        // A resource re-uploaded since this tag answers 200 and the whole body rather than 206.
        if let Some(ref tag) = existing.etag
            && let Ok(value) = reqwest::header::HeaderValue::from_str(tag)
        {
            req = req.header(reqwest::header::IF_RANGE, value);
        }
    }
    req
}

/// The file the body goes into, and how many bytes of it are already there. A 200 to a resume
/// means the server ignored `Range:` or the `If-Range` tag missed, so the partial bytes go.
fn open_destination(
    dest: &Path,
    existing: &Existing,
    status: reqwest::StatusCode,
    response_etag: Option<&str>,
    sidecar: &Sidecar<'_>,
) -> AppResult<(File, u64)> {
    match (&existing.resume, status) {
        (&ResumeState::Resume(offset), reqwest::StatusCode::PARTIAL_CONTENT) => {
            // Rare but legal: a 206 under a new tag.
            if response_etag.is_some() && response_etag != existing.etag.as_deref() {
                let _ = sidecar.write(response_etag);
            }
            let f = std::fs::OpenOptions::new().append(true).open(dest)?;
            Ok((f, offset))
        }
        (ResumeState::Resume(_), _) => {
            log::info!(
                "updater: server returned {status} on Range request (likely If-Range mismatch \
                 or proxy stripped the header); restarting from offset 0"
            );
            let _ = sidecar.write(response_etag);
            Ok((File::create(dest)?, 0u64))
        }
        _ => {
            let _ = sidecar.write(response_etag);
            Ok((File::create(dest)?, 0u64))
        }
    }
}

async fn stream_to_file(
    resp: reqwest::Response,
    mut file: File,
    dest: &Path,
    mut downloaded: u64,
    expected_size: u64,
    on_progress: &(impl Fn(u8) + Send + Sync),
) -> AppResult<()> {
    let chunk_total = resp.content_length().unwrap_or(expected_size).max(1);
    // Against the manifest size: a resume's `Content-Length` counts only what remains.
    let progress_denominator = expected_size.max(chunk_total);
    let mut stream = resp.bytes_stream();
    let mut last_pct = percent(downloaded, progress_denominator);
    on_progress(last_pct);

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| AppError::network("update download stream broke", e))?;
        let new_total = downloaded.saturating_add(chunk.len() as u64);
        if exceeds_size_bound(new_total, expected_size) {
            // Only verified bytes are kept for a retry, and these never reached the check.
            drop(file);
            let _ = std::fs::remove_file(dest);
            return Err(AppError::network_msg(format!(
                "update download exceeded declared size: {new_total} > {expected_size} (5% slack); \
                 manifest may be compromised or the CDN is misbehaving"
            )));
        }
        file.write_all(&chunk)?;
        downloaded = new_total;
        let pct = percent(downloaded, progress_denominator);
        if pct != last_pct {
            on_progress(pct);
            last_pct = pct;
        }
    }
    file.flush()?;
    Ok(())
}

/// `downloaded` as a whole percentage of `denominator`, capped at 100.
fn percent(downloaded: u64, denominator: u64) -> u8 {
    u8::try_from((downloaded.saturating_mul(100) / denominator).min(100)).unwrap_or(100)
}

#[cfg(test)]
#[path = "../tests/download_tests.rs"]
mod tests;
