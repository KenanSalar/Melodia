//! Every stream opens through the shared HTTP client.
//!
//! The pin used to sit in `melodia-audio` and exempt itself, naming the constructors being how
//! it forbade them. Out here it needs no exemption, and the question it asks was never audio's
//! to answer alone: any crate could reach for a one-line constructor.

use melodia_testkit::rust_sources;

/// `StreamDownload`'s convenience constructors build their own unconfigured `reqwest::Client`
/// behind your back, through the trait's no-argument `Client::create`. That costs three things at
/// once, none of them visible at the call site: the `Melodia/<version>` User-Agent some Icecast
/// servers gate on, the `Icy-MetaData` header that makes a station name its tracks, and the shared
/// connection pool. Every open has to go through `HttpStream::new` with the shared client instead,
/// and since the temptation is a one-line constructor, this walks the tree rather than naming the
/// module that currently gets it right.
#[test]
fn nothing_reaches_the_convenience_constructors() {
    let forbidden = [
        "StreamDownload::new_http",
        "StreamDownload::new(",
        "new_http_with_middleware",
    ];

    let mut offenders = Vec::new();
    for (path, source) in rust_sources() {
        for needle in forbidden {
            if source.contains(needle) {
                offenders.push(format!("{path} names `{needle}`"));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "open the stream with HttpStream::new and the shared client instead: {offenders:?}"
    );
}
