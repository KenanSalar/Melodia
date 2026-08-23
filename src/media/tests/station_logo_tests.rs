//! The three guards a station logo passes before it reaches the store, none of which needs a
//! socket: what may be fetched, what a response is filed as, and what is too small to draw.

use std::io::Cursor;

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
