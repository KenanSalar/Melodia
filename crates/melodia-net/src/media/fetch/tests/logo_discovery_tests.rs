//! The site a station is asked about, what its document turns out to advertise, and the answer
//! that comes back when it advertises nothing.
//!
//! The scraper is the half worth the cases: it slices a lowercased copy of the document and reads
//! its answers out of the original, so every ordering, boundary and word-guard here rests on a byte
//! index addressing the same character in both.

use melodia_testkit::http::{TestResponse, TestServer};

use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// A plain `http` origin. The stream arm forces HTTPS, so every loopback case below has to come in
/// through the homepage arm.
fn origin_of(server: &TestServer) -> Result<reqwest::Url, Box<dyn std::error::Error>> {
    origin_for(&server.base_url(), "")
        .ok_or_else(|| "the loopback base URL must parse as a site".into())
}

/// A document whose head carries `head` and nothing else.
fn page(head: &str) -> String {
    format!("<html><head>{head}</head><body></body></html>")
}

// ---- the site to ask ----

#[test]
fn the_site_to_ask_is_the_homepage_when_the_row_carries_one() {
    let origin =
        origin_for("https://site.example.test/about", "http://stream.example.test:8811/live");

    assert_eq!(origin.as_ref().map(reqwest::Url::as_str), Some("https://site.example.test/about"));
}

/// A mount on `:8811` says nothing about where the site is, and a cleartext mount is a statement
/// about the audio server rather than about the site.
#[test]
fn a_row_with_no_homepage_asks_the_streams_own_host_over_https() {
    let origin = origin_for("", "http://stream.example.test:8811/live.mp3?token=abc");

    assert_eq!(origin.as_ref().map(reqwest::Url::as_str), Some("https://stream.example.test/"));
}

#[test]
fn a_row_naming_no_host_either_way_has_no_site() {
    for (homepage, stream_url) in [("", ""), ("", "http://"), ("not a url", "also not one")] {
        assert!(origin_for(homepage, stream_url).is_none(), "for {homepage:?} and {stream_url:?}");
    }
}

// ---- what the document advertises ----

/// Ordered by how deliberate the choice is, not by where it sits: an `apple-touch-icon` is a square
/// somebody picked at a drawable size, where a plain `icon` is whatever the tab needed.
#[test]
fn an_apple_touch_icon_outranks_a_plain_icon_wherever_it_sits() {
    let head = r#"<link rel="icon" href="/tab.png"><link rel="apple-touch-icon" href="/app.png">"#;

    assert_eq!(icon_href(&page(head)).as_deref(), Some("/app.png"));
}

#[test]
fn a_link_icon_outranks_an_og_image() {
    let head = r#"<meta property="og:image" content="/card.png"><link rel="icon" href="/tab.png">"#;

    assert_eq!(icon_href(&page(head)).as_deref(), Some("/tab.png"));
}

/// A share card at least belongs to the site, which is more than the well-known path can promise.
#[test]
fn an_og_image_answers_only_when_no_link_icon_does() {
    let head = r#"<meta property="og:image" content="https://cdn.example.test/card.png">"#;

    assert_eq!(icon_href(&page(head)).as_deref(), Some("https://cdn.example.test/card.png"));
}

/// A `<link>` in the body cannot outrank one the author put where it belongs.
#[test]
fn only_the_head_is_read() {
    let inside = r#"<link rel="icon" href="/head.png">"#;
    let outside = r#"<link rel="apple-touch-icon" href="/body.png">"#;

    let both = format!("<html><head>{inside}</head><body>{outside}</body></html>");
    assert_eq!(icon_href(&both).as_deref(), Some("/head.png"), "an apple icon in the body loses");

    let body_only = format!("<html><head></head><body>{outside}</body></html>");
    assert_eq!(icon_href(&body_only), None, "and it is not a fallback either");
}

/// **What the raw slice beside the lowercased one is for.** Read the href out of the lowercased
/// copy and every site with a capital in its path gets a 404 for a logo it advertised correctly.
#[test]
fn an_href_keeps_the_case_the_document_wrote_it_in() {
    let head = r#"<LINK REL="ICON" HREF="/Assets/Logo.PNG">"#;

    assert_eq!(icon_href(&page(head)).as_deref(), Some("/Assets/Logo.PNG"));
}

/// The name has to start a word, or `href` matches inside `xlink:href` and `rel` inside anything
/// hyphenated onto it.
#[test]
fn an_attribute_name_has_to_start_a_word() {
    let head = r#"<link data-rel="nope" rel="icon" xlink:href="/wrong.png" href="/right.png">"#;

    assert_eq!(icon_href(&page(head)).as_deref(), Some("/right.png"));
}

/// The `/` of a self-closing tag is the tag's; one leading a path is not, and reading it as a
/// terminator dropped every unquoted absolute href.
#[test]
fn a_bare_attribute_value_runs_to_the_tag_and_keeps_its_leading_slash() {
    let cases = [
        ("<link rel=icon href=/logo.png>", "/logo.png"),
        ("<link rel=icon href=logo.png/>", "logo.png"),
        ("<link rel=icon href=logo.png >", "logo.png"),
        ("<link rel='icon' href='/logo.png'>", "/logo.png"),
    ];
    for (head, expected) in cases {
        assert_eq!(icon_href(&page(head)).as_deref(), Some(expected), "for {head}");
    }
}

#[test]
fn an_empty_href_is_not_an_answer() {
    let head = r#"<link rel="icon" href=""><link rel="icon" href="/real.png">"#;

    assert_eq!(icon_href(&page(head)).as_deref(), Some("/real.png"));
}

/// A span is cut at the first `>`, so one inside a quoted value ends the tag early. The cost is
/// that tag and no other, which is the trade the scan is written around.
#[test]
fn a_quoted_angle_bracket_costs_its_own_tag_and_no_other() {
    let head =
        r#"<link rel="icon" title="a>b" href="/lost.png"><link rel="icon" href="/kept.png">"#;

    assert_eq!(icon_href(&page(head)).as_deref(), Some("/kept.png"));
}

/// A `<link>` saying nothing about what it links to is not an icon, however much its href looks
/// like one.
#[test]
fn a_document_advertising_nothing_names_nothing() {
    let head = concat!(
        "<title>Example Radio</title>",
        r#"<link rel="stylesheet" href="/site.css">"#,
        r#"<link href="/could-be-anything.png">"#,
    );

    assert_eq!(icon_href(&page(head)), None);
}

// ---- over a socket ----

#[tokio::test]
async fn an_advertised_icon_is_resolved_against_the_site() -> TestResult {
    let body = page(r#"<link rel="icon" href="assets/logo.png">"#);
    let server = TestServer::start(move |_| TestResponse::ok(body.clone()))?;
    let origin = origin_of(&server)?;

    let found = icon_url(&reqwest::Client::new(), &origin).await?;

    assert_eq!(found, Some(format!("{}/assets/logo.png", server.base_url())));
    Ok(())
}

/// The well-known path predates the `<link>` and plenty of sites serve only it, so a document with
/// nothing to say is still worth the request that follows.
#[tokio::test]
async fn a_site_advertising_nothing_still_answers_the_well_known_path() -> TestResult {
    let server = TestServer::start(|_| TestResponse::ok(page("<title>Example Radio</title>")))?;
    let origin = origin_of(&server)?;

    let found = icon_url(&reqwest::Client::new(), &origin).await?;

    assert_eq!(found, Some(format!("{}{WELL_KNOWN_ICON}", server.base_url())));
    Ok(())
}

/// A refused document is not a refused icon, and the two are worth separating: plenty of sites
/// serve a favicon from behind a homepage that answers 404.
#[tokio::test]
async fn a_site_that_refuses_the_document_still_answers_the_well_known_path() -> TestResult {
    let server = TestServer::start(|_| TestResponse::status(404))?;
    let origin = origin_of(&server)?;

    let found = icon_url(&reqwest::Client::new(), &origin).await?;

    assert_eq!(found, Some(format!("{}{WELL_KNOWN_ICON}", server.base_url())));
    Ok(())
}

/// Not reaching the host at all is the one answer that must not become a guess: `library::radio`
/// reads an `Err` as a moment offline and anything else as worth recording a backoff against.
#[tokio::test]
async fn a_site_that_cannot_be_reached_is_an_error_rather_than_a_guess() -> TestResult {
    // Dropping the server closes its listener, so the port refuses rather than hangs.
    let origin = {
        let server = TestServer::start(|_| TestResponse::ok(""))?;
        origin_of(&server)?
    };

    let answered = icon_url(&reqwest::Client::new(), &origin).await;

    assert!(
        matches!(&answered, Err(AppError::Network { .. })),
        "an unreachable site must not resolve to a favicon path nobody can serve: {answered:?}"
    );
    Ok(())
}

/// A `<head>` that has not named an icon inside this much markup is not going to, and the cap is
/// what stops a site streaming a document into a station refresh.
#[tokio::test]
async fn a_document_past_the_page_cap_is_refused() -> TestResult {
    let oversized = usize::try_from(MAX_PAGE_BYTES).unwrap_or(0) + 1;
    let server = TestServer::start(move |_| TestResponse::ok(vec![b'<'; oversized]))?;
    let origin = origin_of(&server)?;

    let answered = icon_url(&reqwest::Client::new(), &origin).await;

    assert!(
        matches!(&answered, Err(AppError::Network { msg, .. }) if msg.contains("larger than")),
        "the page read is bounded, or a station refresh pays for whatever the site sends: \
         {answered:?}"
    );
    Ok(())
}
