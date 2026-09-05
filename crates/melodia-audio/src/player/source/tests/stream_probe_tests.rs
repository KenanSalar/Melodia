//! What a hand-typed station URL goes through before the user is told it works.
//!
//! `probe` is `open` minus the ring and the thread, which is what makes it the honest place to
//! refuse a mount: the playlist indirection is followed, the response is buffered and Symphonia
//! probes the container, so a URL that is a web page is refused while the user is still looking
//! at the dialog rather than at the moment they press play. The facts that come back with it are
//! the only way a hand-typed station names itself.

use std::sync::{Arc, OnceLock};

use melodia_testkit::{
    ASSETS_DIR,
    http::{TestResponse, TestServer},
};

use super::probe;
use melodia_core::error::AppError;

const MOUNT: &str = "/live.mp3";
const POINTER: &str = "/station.pls";

fn audio() -> Vec<u8> {
    std::fs::read(std::path::Path::new(ASSETS_DIR).join("silence.mp3")).unwrap_or_default()
}

/// An Icecast mount: the audio, its content type, and the ICY fields a station describes itself
/// with.
fn icecast_mount() -> TestResponse {
    TestResponse::ok(audio())
        .header("Content-Type", "audio/mpeg")
        .header("icy-name", "  Example FM  ")
        .header("icy-genre", "Jazz")
        .header("icy-br", "128")
        .header("icy-url", "https://example.test/")
}

#[tokio::test]
async fn a_mount_names_itself_from_its_icy_headers() -> Result<(), AppError> {
    let server = TestServer::start(|_| icecast_mount())?;

    let facts = probe(&reqwest::Client::new(), &format!("{}{MOUNT}", server.base_url())).await?;

    // Icecast pads these freely, and a station named after whitespace is worse than an unnamed
    // one: the row would render blank with no way to tell it apart from a real name.
    assert_eq!(facts.name.as_deref(), Some("Example FM"));
    assert_eq!(facts.genre, "Jazz");
    assert_eq!(facts.homepage.as_deref(), Some("https://example.test/"));
    assert_eq!(facts.bitrate, 128);
    assert_eq!(facts.codec, "MP3", "the codec is read off what the server sent");
    assert!(!facts.hls);
    Ok(())
}

/// A server that sends the field empty has said nothing, which is not the same as naming the
/// station the empty string.
#[tokio::test]
async fn a_blank_icy_field_leaves_the_station_unnamed() -> Result<(), AppError> {
    let server = TestServer::start(|_| {
        TestResponse::ok(audio()).header("Content-Type", "audio/mpeg").header("icy-name", "   ")
    })?;

    let facts = probe(&reqwest::Client::new(), &format!("{}{MOUNT}", server.base_url())).await?;

    assert_eq!(facts.name, None);
    Ok(())
}

/// The shape most hand-typed stations arrive in: a `.pls` naming the mount. The extension is
/// spotted before anything is opened, so the pointer costs one request rather than a stream
/// opened and thrown away.
#[tokio::test]
async fn a_pointer_playlist_is_followed_to_the_mount_it_names() -> Result<(), AppError> {
    // The pointer has to name an absolute URL and the port is only known once the listener is
    // bound, so the handler reads its own origin out of a cell filled right after the start. It
    // runs no earlier than the first request, which is after the probe below begins.
    let origin: Arc<OnceLock<String>> = Arc::new(OnceLock::new());
    let served = Arc::clone(&origin);
    let server = TestServer::start(move |request| {
        if request.path != POINTER {
            return icecast_mount();
        }
        let mount = served.get().map_or_else(String::new, |base| format!("{base}{MOUNT}"));
        TestResponse::ok(format!("[playlist]\nNumberOfEntries=1\nFile1={mount}\n"))
            .header("Content-Type", "audio/x-scpls")
    })?;
    let _bound = origin.set(server.base_url());

    let facts = probe(&reqwest::Client::new(), &format!("{}{POINTER}", server.base_url())).await?;

    assert_eq!(facts.name.as_deref(), Some("Example FM"));
    // Spotting the extension is what keeps that one request. Left to the content-type fallback
    // the pointer is opened as a stream first, thrown away, and fetched again as a document.
    assert_eq!(server.requests().iter().filter(|request| request.path == POINTER).count(), 1,);
    Ok(())
}

/// A URL that answers with a web page is the commonest thing a user pastes by mistake, and it is
/// refused at the probe rather than staged onto a deck that would then play nothing.
#[tokio::test]
async fn a_mount_that_is_a_web_page_is_refused() {
    let Ok(server) = TestServer::start(|_| {
        TestResponse::ok("<html><body>Now playing</body></html>")
            .header("Content-Type", "text/html")
    }) else {
        unreachable!("a loopback listener on port 0")
    };

    let probed = probe(&reqwest::Client::new(), &format!("{}{MOUNT}", server.base_url())).await;

    assert!(probed.is_err(), "a web page is not a station");
}
