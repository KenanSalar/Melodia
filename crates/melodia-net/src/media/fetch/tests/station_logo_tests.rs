//! The guards a station logo passes on its way into the store, and the two answers a caller has to
//! tell apart.
//!
//! The first band needs no socket: what may be fetched, what a response is filed as, and what is
//! too small to draw. The second drives `fetch` against a loopback server, where the distinction
//! that matters is `Ok(None)` against `Err`: `library::radio::ask_logo_url` earns the URL a
//! day-long backoff on the first and deliberately not on the second.

use std::io::Cursor;

use melodia_testkit::http::{TestResponse, TestServer};

use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// A square PNG of `side` px, which is what the floor measures and what the store hashes.
fn png(side: u32) -> Result<Vec<u8>, image::ImageError> {
    let mut bytes = Vec::new();
    image::RgbImage::from_pixel(side, side, image::Rgb([80, 140, 200]))
        .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)?;
    Ok(bytes)
}

/// A `favicon_url` is whatever the station's owner typed, so the scheme is the one thing standing
/// between a directory row and a request the app would never otherwise make.
#[test]
fn only_http_and_https_urls_are_fetched() {
    for url in [
        "http://example.test/logo.png",
        "https://example.test/logo.png",
    ] {
        assert!(fetchable_url(url).is_ok(), "{url} should be fetched");
    }
    for url in [
        "file:///etc/passwd",
        "data:image/png;base64,iVBORw0KGgo=",
        "ftp://example.test/logo.png",
        "not a url at all",
    ] {
        assert!(fetchable_url(url).is_err(), "{url} should be refused");
    }
}

#[test]
fn a_content_type_names_the_extension_it_is_stored_under() {
    let cases = [
        ("image/png", Some("png")),
        ("image/webp", Some("webp")),
        ("image/gif", Some("gif")),
        ("image/bmp", Some("bmp")),
        ("image/x-ms-bmp", Some("bmp")),
        ("image/tiff", Some("tiff")),
        // The favicon container, and the whole reason `image` carries the `ico` feature.
        ("image/x-icon", Some("ico")),
        ("image/vnd.microsoft.icon", Some("ico")),
        ("image/jpeg", Some("jpg")),
        // An image type nothing here names is filed as JPEG and settled by the header parse.
        ("image/svg+xml", Some("jpg")),
        ("text/html", None),
        ("application/octet-stream", None),
        ("", None),
    ];
    for (header, expected) in cases {
        assert_eq!(extension_for(header), expected, "for {header:?}");
    }
}

/// Hosts send a charset parameter and mixed case on a header that is neither, and neither may
/// change what the response is filed as.
#[test]
fn a_content_type_is_read_past_its_parameters_and_its_case() {
    assert_eq!(extension_for("IMAGE/PNG"), Some("png"));
    assert_eq!(extension_for("image/x-icon; charset=binary"), Some("ico"));
    assert_eq!(extension_for("  image/webp  "), Some("webp"));
}

/// The floor sits at favicon size, so the smallest real logos still get in and the 1x1 that fills
/// the field in for a tracker does not.
#[test]
fn the_floor_admits_a_favicon_and_refuses_a_tracking_pixel() -> TestResult {
    let dir = tempfile::tempdir()?;

    assert!(store_if_big_enough(&png(1)?, "png", dir.path()).is_none(), "a 1x1 must be refused");
    assert!(
        store_if_big_enough(&png(MIN_LOGO_DIM - 1)?, "png", dir.path()).is_none(),
        "one pixel under the floor must be refused"
    );

    let stored = store_if_big_enough(&png(MIN_LOGO_DIM)?, "png", dir.path());
    assert!(
        stored.as_ref().is_some_and(|logo| Path::new(&logo.path).exists()),
        "a {MIN_LOGO_DIM}px logo must reach the store, got {stored:?}"
    );
    // The size the answer table bills the cache for, and the one bound the row cannot re-derive.
    assert!(
        stored.is_some_and(|logo| logo.bytes > 0),
        "a stored logo has to report what it cost, or the cache has no size to hold itself to"
    );
    Ok(())
}

/// A source with nothing opaque in it has no ground to build a tile from, and storing it untreated
/// is the one outcome worse than storing nothing: every tier below drops the alpha channel rather
/// than compositing it, so the card paints an empty square where its monogram was the honest
/// answer. The two "no tile" verdicts are opposite instructions and the store has to tell them
/// apart.
#[test]
fn a_source_with_no_opaque_pixel_is_refused_rather_than_stored_untreated() -> TestResult {
    let dir = tempfile::tempdir()?;
    let side = MIN_LOGO_DIM * 2;

    let mut bytes = Vec::new();
    image::RgbaImage::from_pixel(side, side, image::Rgba([200, 40, 40, 0]))
        .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)?;

    // Well clear of the floor, so the refusal is the tile's verdict and not the size guard's.
    assert!(
        store_if_big_enough(&bytes, "png", dir.path()).is_none(),
        "a fully transparent {side}px source must not reach the store"
    );
    Ok(())
}

/// **The now-playing tile skips `native-size` because of this number**, so the argument for that
/// omission lives in one tree and the number it rests on lives in another.
///
/// `source-artwork.slint` reasons that no source reaching it can be small enough for
/// `ArtworkImage`'s inset arm to fire, since the floor is 32 px and the largest tile mounting it
/// is 46. Lower the floor and that stops being true with nothing to say so: the tile would upscale
/// a 16 px favicon across 46 px rather than insetting it, which is exactly the treatment the inset
/// arm exists to give. Raise it past the tile and the comment merely reads oddly.
#[test]
fn the_slint_tile_that_skips_native_size_still_agrees_with_the_floor() {
    const TILE: &str =
        include_str!("../../../../../melodia-ui/ui/components/now-playing/source-artwork.slint");

    assert!(
        TILE.contains(&format!("`media::fetch::station_logo::MIN_LOGO_DIM` is {MIN_LOGO_DIM} px")),
        "`source-artwork.slint` restates the floor to argue it needs no `native-size`; it is \
         {MIN_LOGO_DIM} px here and the two have drifted"
    );
    // Stripped, or the comment making the argument satisfies the search for the binding it
    // argues against.
    assert!(
        !melodia_testkit::strip_line_comments(TILE).contains("native-size"),
        "the argument stands or the binding does — if the tile now sets `native-size`, this pin \
         and the comment above it are both stale"
    );
}

// ---- over a socket ----

/// A wide opaque source, which is the shape `logo_tile` composes rather than keeping. JPEG so the
/// container the tile replaces is not the one it is stored under.
fn wordmark_jpeg(width: u32, height: u32) -> Result<Vec<u8>, image::ImageError> {
    let mut bytes = Vec::new();
    image::RgbImage::from_pixel(width, height, image::Rgb([20, 60, 120]))
        .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Jpeg)?;
    Ok(bytes)
}

/// A server answering every request with `body` under `content_type`.
fn logo_server(content_type: &'static str, body: Vec<u8>) -> std::io::Result<TestServer> {
    TestServer::start(move |_| TestResponse::ok(body.clone()).header("content-type", content_type))
}

/// One logo off `server` into `dir`.
async fn fetch_from(server: &TestServer, dir: &Path) -> Result<Option<StoredLogo>, AppError> {
    fetch(&reqwest::Client::new(), &format!("{}/logo.png", server.base_url()), dir).await
}

/// **A usable answer with no logo in it, which is not the same as a failure.** The content-type
/// bail sits ahead of every other check for that reason: a host that served a page has answered,
/// and the caller records the backoff that stops it being asked again tomorrow.
#[tokio::test]
async fn a_response_that_is_not_an_image_is_an_answer_with_no_logo() -> TestResult {
    let dir = tempfile::tempdir()?;
    let server = logo_server("text/html; charset=utf-8", b"<html></html>".to_vec())?;

    let answered = fetch_from(&server, dir.path()).await?;

    assert!(answered.is_none(), "an HTML page is an answer, and the answer is no logo");
    Ok(())
}

#[tokio::test]
async fn a_response_that_says_nothing_about_its_type_is_the_same_answer() -> TestResult {
    let dir = tempfile::tempdir()?;
    let server = TestServer::start(|_| TestResponse::ok(vec![0u8; 64]))?;

    let answered = fetch_from(&server, dir.path()).await?;

    assert!(answered.is_none(), "nothing says what it is, so nothing says the store can hold it");
    Ok(())
}

/// The other side of that pair: a host that refused has not answered, and recording a backoff
/// against it would suppress a perfectly good logo over an afternoon of downtime.
#[tokio::test]
async fn a_status_that_is_not_success_is_a_failure_worth_retrying() -> TestResult {
    let dir = tempfile::tempdir()?;
    let server = TestServer::start(|_| TestResponse::status(503))?;

    let refused = fetch_from(&server, dir.path()).await;

    assert!(
        matches!(&refused, Err(AppError::Network { msg, .. }) if msg.contains("503")),
        "a host that is down must not be filed as a host with no logo: {refused:?}"
    );
    Ok(())
}

/// The header is checked ahead of the body so an oversized host costs a header rather than a
/// transfer. A body that would comfortably have fit is what says the claim is what refused.
#[tokio::test]
async fn a_declared_length_over_the_cap_costs_a_header_rather_than_a_transfer() -> TestResult {
    let dir = tempfile::tempdir()?;
    let drawable = png(MIN_LOGO_DIM * 2)?;
    let server = TestServer::start(move |_| {
        TestResponse::ok(drawable.clone())
            .header("content-type", "image/png")
            .claiming_length(MAX_LOGO_BYTES + 1)
    })?;

    let refused = fetch_from(&server, dir.path()).await;

    assert!(
        matches!(&refused, Err(AppError::Network { msg, .. }) if msg.contains("too large")),
        "a claim over the cap is refused whatever the body turns out to be: {refused:?}"
    );
    Ok(())
}

/// One gate at the writer rather than one per surface, so a source too small to draw leaves nothing
/// behind for a later tier to reject.
#[tokio::test]
async fn a_source_under_the_floor_never_reaches_the_store() -> TestResult {
    let dir = tempfile::tempdir()?;
    let server = logo_server("image/png", png(MIN_LOGO_DIM - 1)?)?;

    let answered = fetch_from(&server, dir.path()).await?;

    assert!(answered.is_none(), "a source under the floor is an answer with no logo in it");
    assert_eq!(std::fs::read_dir(dir.path())?.count(), 0, "and it wrote nothing");
    Ok(())
}

#[tokio::test]
async fn a_logo_that_clears_the_floor_lands_in_the_store() -> TestResult {
    let dir = tempfile::tempdir()?;
    let server = logo_server("image/png", png(MIN_LOGO_DIM * 2)?)?;

    let stored = fetch_from(&server, dir.path()).await?;

    let Some(stored) = stored else {
        return Err("a source well clear of the floor has to reach the store".into());
    };
    assert!(Path::new(&stored.path).exists(), "the answer names a file that is actually there");
    assert!(
        stored.bytes > 0,
        "and reports what it cost, or the cache has no size to hold itself to"
    );
    Ok(())
}

/// A tile is flat ground behind a hard-edged mark, which is what JPEG rings around and what PNG
/// holds in a few kilobytes. So the source's own container stops having a say the moment a tile
/// replaces it.
#[tokio::test]
async fn a_composed_tile_is_stored_as_png_whatever_the_source_was() -> TestResult {
    let dir = tempfile::tempdir()?;
    let server = logo_server("image/jpeg", wordmark_jpeg(MIN_LOGO_DIM * 4, MIN_LOGO_DIM * 2)?)?;

    let stored = fetch_from(&server, dir.path()).await?;

    let Some(stored) = stored else {
        return Err("a wide opaque source is composed, not refused".into());
    };
    assert_eq!(
        Path::new(&stored.path).extension().and_then(std::ffi::OsStr::to_str),
        Some("png"),
        "a wordmark is stored as the tile it became, not the JPEG it arrived as: {}",
        stored.path
    );
    Ok(())
}
