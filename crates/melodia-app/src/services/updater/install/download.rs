//! Streaming download with HTTP-range resume, ETag-aware `If-Range` freshness, and a 5 %-slack
//! size bound that aborts a runaway transfer before it can fill the user's disk.

use std::fs::File;
use std::io::Write;
use std::path::Path;

use futures_util::StreamExt;

use melodia_core::error::{AppError, AppResult};

use super::staging::{
    StagedMeta, discard_staging_if_sidecar_mismatches, sidecar_meta_path, write_staged_meta,
};

/// 5 % slack over `asset.size` absorbs the rare CDN that over-reports `Content-Length` without
/// giving a buggy manifest enough rope to fill the user's disk — a claim of 80 MiB serving 10 GiB
/// trips at the 84 MiB mark, rather than waiting for the whole transfer to fail its signature.
pub(crate) fn exceeds_size_bound(downloaded: u64, expected_size: u64) -> bool {
    // `expected_size * 1.05` without floats. The saturation biases toward "too big", which is the
    // safe direction — it rejects a giant claim, never accepts one.
    let bound = expected_size.saturating_mul(105) / 100;
    downloaded > bound
}

/// Filter an `ETag` header value down to a strong tag, dropping weak ones (`W/"..."`) and any
/// header that didn't decode as ASCII.
///
/// RFC 9110 §13.1.5 forbids generating an `If-Range` containing a weak entity-tag, and §8.8.3.2's
/// strong comparison means a server that received one would always evaluate it false — silently
/// forcing a full re-download on every resume. `None` makes the resume protocol fall back to plain
/// `Range:`, still safe because the post-download signature verify catches a concatenation
/// accident. A futureproof for an origin switch: the production CDN serves strong tags today.
pub(crate) fn capture_strong_etag(header_value: Option<&str>) -> Option<String> {
    header_value.filter(|tag| !tag.starts_with("W/")).map(str::to_owned)
}

/// Decision tree for an existing `dest` file when a download starts:
///
/// * **`Skip`** — bytes on disk match the manifest's `size`. The "retention on failure" hot path:
///   a previously-verified `.rpm`/`.deb` whose `dnf install` was cancelled stays on disk. The
///   caller's `verify_staged` still runs, so a corrupted file gets caught.
/// * **`Resume(offset)`** — partial bytes exist. Try `Range: bytes=<offset>-`; the caller falls
///   back to `Fresh` if the server answers 200 instead of 206.
/// * **`Fresh`** — start a new download into a truncated file. Covers no file, an empty file, and
///   `existing > expected` (a leftover from a release with a larger asset).
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ResumeState {
    Skip,
    Resume(u64),
    Fresh,
}

pub(crate) fn plan_resume(existing_size: u64, expected_size: u64) -> ResumeState {
    if expected_size == 0 {
        // Manifest reports zero size — malformed. Force fresh so the bound check catches whatever
        // the server actually serves.
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
    // existing > expected — leftover from a different release, or a manifest-size shrink. Discard.
    ResumeState::Fresh
}

/// Write the `(version, size, url, etag?)` staging sidecar next to a staged file. Single source of
/// truth for sidecar construction — [`download_to_file`] writes it once at download start and
/// refreshes it whenever the captured `ETag` changes.
fn write_sidecar(
    path: &Path,
    version: &str,
    size: u64,
    url: &str,
    etag: Option<&str>,
) -> AppResult<()> {
    write_staged_meta(
        path,
        &StagedMeta {
            version: version.to_string(),
            size,
            asset_url: url.to_string(),
            etag: etag.map(str::to_owned),
        },
    )
}

pub(super) async fn download_to_file(
    http: &reqwest::Client,
    url: &str,
    expected_version: &str,
    expected_size: u64,
    dest: &Path,
    on_progress: &(impl Fn(u8) + Send + Sync),
) -> AppResult<()> {
    // Sidecar check before any size-based decision: bytes beside a missing or mismatching sidecar
    // are from a different release and can't be trusted for `Skip` or `Resume`. Discarding here
    // means a `--clobber` re-push at the same version, or any cross-release resume, restarts
    // cleanly instead of gluing mismatched bytes.
    //
    // A surviving sidecar carries the previous attempt's ETag, which a resume sends back as
    // `If-Range`.
    let existing_etag =
        discard_staging_if_sidecar_mismatches(dest, expected_version, expected_size, url)
            .and_then(|m| m.etag);

    let existing_size = std::fs::metadata(dest).map_or(0, |m| m.len());
    let resume = plan_resume(existing_size, expected_size);

    // Write (or refresh) the sidecar at the *start* of the download so a
    // kill-mid-stream leaves a matching `(version, size, url, etag?)` next
    // to the partial bytes — the next attempt resumes safely instead of
    // discarding. Preserves any pre-existing etag so the kill-before-GET
    // window doesn't lose freshness metadata that's still correlated with
    // the disk bytes. Best-effort: a sidecar write failure shouldn't
    // abort the download (worst case the next attempt re-discards and
    // starts fresh, which is the same as today's behaviour without
    // sidecars).
    if let Err(e) = write_sidecar(
        &sidecar_meta_path(dest),
        expected_version,
        expected_size,
        url,
        existing_etag.as_deref(),
    ) {
        log::warn!(
            "updater: failed to write staging sidecar at {}: {e} (download continues; \
             worst case a future retry restarts from zero)",
            sidecar_meta_path(dest).display()
        );
    }

    if matches!(resume, ResumeState::Skip) {
        log::info!(
            "updater: skipping download — {} already has {existing_size} bytes (matches manifest); \
             verify step will validate",
            dest.display()
        );
        on_progress(100);
        return Ok(());
    }

    let mut req = http.get(url);
    if let ResumeState::Resume(offset) = resume {
        log::info!(
            "updater: resuming download at byte {offset} (have {existing_size} of {expected_size})"
        );
        req = req.header(reqwest::header::RANGE, format!("bytes={offset}-"));
        // If-Range: <etag> — the server returns 206 only when the resource is byte-identical to
        // the version that produced this ETag, else 200 and the full body. Closes the "release
        // re-uploaded with --clobber mid-download" hole that would otherwise concatenate bytes
        // from two different artifacts.
        if let Some(ref tag) = existing_etag
            && let Ok(value) = reqwest::header::HeaderValue::from_str(tag)
        {
            req = req.header(reqwest::header::IF_RANGE, value);
        }
    }

    let resp = req
        .send()
        .await
        .map_err(|e| AppError::network(format!("update download GET {url} failed"), e))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(AppError::network_msg(format!("update download {url} returned HTTP {status}")));
    }

    // Capture the response ETag so future resumes can send it back as `If-Range`. Read eagerly,
    // before consuming the body, since some reqwest versions move headers out of the response on
    // the first streaming call.
    let response_etag = capture_strong_etag(
        resp.headers().get(reqwest::header::ETAG).and_then(|v| v.to_str().ok()),
    );

    // Server-side resume support is best-effort. A 206 means the server honoured the `Range:` and
    // the stream continues from `offset`; a 200 means it ignored the header or the `If-Range:`
    // ETag didn't match, and the partial bytes have to be discarded rather than concatenated into
    // a garbled file.
    let sidecar_path = sidecar_meta_path(dest);
    let refresh_sidecar = |etag: Option<&str>| -> AppResult<()> {
        write_sidecar(&sidecar_path, expected_version, expected_size, url, etag)
    };
    let (mut file, mut downloaded) = match (resume, status) {
        (ResumeState::Resume(offset), reqwest::StatusCode::PARTIAL_CONTENT) => {
            // 206 — existing bytes stay correlated with `existing_etag`. Update the sidecar only
            // if the server bumped the ETag, which is rare but legal.
            if response_etag.is_some() && response_etag.as_deref() != existing_etag.as_deref() {
                let _ = refresh_sidecar(response_etag.as_deref());
            }
            let f = std::fs::OpenOptions::new().append(true).open(dest)?;
            (f, offset)
        }
        (ResumeState::Resume(_), _) => {
            // 200 where a resume was asked for — reset bytes and sidecar etag so the next attempt
            // resumes against the new resource.
            log::info!(
                "updater: server returned {status} on Range request (likely If-Range mismatch \
                 or proxy stripped the header); restarting from offset 0"
            );
            let _ = refresh_sidecar(response_etag.as_deref());
            (File::create(dest)?, 0u64)
        }
        _ => {
            // Fresh download — write the sidecar with the captured ETag so an interrupted attempt
            // can resume against it.
            let _ = refresh_sidecar(response_etag.as_deref());
            (File::create(dest)?, 0u64)
        }
    };

    let chunk_total = resp.content_length().unwrap_or(expected_size).max(1);
    // Progress is against the *manifest* size, not the response's `Content-Length` — a resumed
    // download's Content-Length is the remaining bytes, so the bar would restart at zero on a file
    // already most of the way there.
    let progress_denominator = expected_size.max(chunk_total);
    let mut stream = resp.bytes_stream();
    let mut last_pct: u8 =
        u8::try_from((downloaded.saturating_mul(100) / progress_denominator).min(100)).unwrap_or(0);
    on_progress(last_pct);

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| AppError::network("update download stream broke", e))?;
        let new_total = downloaded.saturating_add(chunk.len() as u64);
        if exceeds_size_bound(new_total, expected_size) {
            // Drop the handle and remove the partial bytes before surfacing the error. The
            // retention-on-failure pattern is for *verified* bytes; a size-bound abort means these
            // never had a chance to pass verify, so cleanup is right.
            drop(file);
            let _ = std::fs::remove_file(dest);
            return Err(AppError::network_msg(format!(
                "update download exceeded declared size: {new_total} > {expected_size} (5% slack); \
                 manifest may be compromised or the CDN is misbehaving"
            )));
        }
        file.write_all(&chunk)?;
        downloaded = new_total;
        let pct = u8::try_from((downloaded.saturating_mul(100) / progress_denominator).min(100))
            .unwrap_or(100);
        if pct != last_pct {
            on_progress(pct);
            last_pct = pct;
        }
    }
    file.flush()?;
    Ok(())
}

#[cfg(test)]
#[path = "../tests/download_tests.rs"]
mod tests;
