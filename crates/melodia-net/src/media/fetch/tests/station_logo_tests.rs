//! The guards a station logo passes on its way off the host, and the two answers a caller has to
//! tell apart.
//!
//! The first band needs no socket: what may be fetched and what a response is filed as. The second
//! drives `fetch` against a loopback server, where the distinction that matters is `Ok(None)`
//! against `Err`: `library::radio::ask_logo_url` earns the URL a day-long backoff on the first and
//! deliberately not on the second.

use melodia_testkit::http::{TestResponse, TestServer};

use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// A `favicon_url` is whatever the station's owner typed, so the scheme is the one thing standing
/// between a directory row and a request the app would never otherwise make.
#[test]
fn only_http_and_https_urls_are_fetched() {
    for url in ["http://example.test/logo.png", "https://example.test/logo.png"] {
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

// ---- over a socket ----

/// A server answering every request with `body` under `content_type`.
fn logo_server(content_type: &'static str, body: Vec<u8>) -> std::io::Result<TestServer> {
    TestServer::start(move |_| TestResponse::ok(body.clone()).header("content-type", content_type))
}

/// One logo off `server`.
async fn fetch_from(server: &TestServer) -> Result<Option<FetchedLogo>, AppError> {
    fetch(&reqwest::Client::new(), &format!("{}/logo.png", server.base_url())).await
}

/// **A usable answer with no logo in it, which is not the same as a failure.** The content-type
/// bail sits ahead of every other check for that reason: a host that served a page has answered,
/// and the caller records the backoff that stops it being asked again tomorrow.
#[tokio::test]
async fn a_response_that_is_not_an_image_is_an_answer_with_no_logo() -> TestResult {
    let server = logo_server("text/html; charset=utf-8", b"<html></html>".to_vec())?;

    let answered = fetch_from(&server).await?;

    assert!(answered.is_none(), "an HTML page is an answer, and the answer is no logo");
    Ok(())
}

#[tokio::test]
async fn a_response_that_says_nothing_about_its_type_is_the_same_answer() -> TestResult {
    let server = TestServer::start(|_| TestResponse::ok(vec![0u8; 64]))?;

    let answered = fetch_from(&server).await?;

    assert!(answered.is_none(), "nothing says what it is, so nothing says the store can hold it");
    Ok(())
}

/// The other side of that pair: a host that refused has not answered, and recording a backoff
/// against it would suppress a perfectly good logo over an afternoon of downtime.
#[tokio::test]
async fn a_status_that_is_not_success_is_a_failure_worth_retrying() -> TestResult {
    let server = TestServer::start(|_| TestResponse::status(503))?;

    let refused = fetch_from(&server).await;

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
    let server = TestServer::start(|_| {
        TestResponse::ok(vec![0u8; 64])
            .header("content-type", "image/png")
            .claiming_length(MAX_LOGO_BYTES + 1)
    })?;

    let refused = fetch_from(&server).await;

    assert!(
        matches!(&refused, Err(AppError::Network { msg, .. }) if msg.contains("too large")),
        "a claim over the cap is refused whatever the body turns out to be: {refused:?}"
    );
    Ok(())
}

/// What the store is handed: the body as it arrived, filed under what its type says it is.
#[tokio::test]
async fn an_image_response_comes_back_as_its_bytes_and_extension() -> TestResult {
    let body = vec![7u8; 64];
    let server = logo_server("image/png", body.clone())?;

    let fetched = fetch_from(&server).await?;

    let Some(fetched) = fetched else {
        return Err("an image response is a logo for the store to judge".into());
    };
    assert_eq!(fetched.extension, "png");
    assert_eq!(fetched.bytes, body, "the bytes reach the store as the host served them");
    Ok(())
}
