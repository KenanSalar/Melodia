//! What the session memo is allowed to skip, and when.
//!
//! Both halves are invisible from the outside: a logo that is merely absent looks exactly like a
//! station that has none, and the card has a monogram either way. So a memo that skips too much
//! reads as a directory full of logo-less stations, and one that skips too little reads as nothing
//! at all until the request bill arrives.

use super::{EXPLICIT_RESULT_MAX, Effort, LogoMemo};

const URL: &str = "https://example.invalid/logo.png";

/// A narrow answer is only the station the user typed when the query that fetched it is the one
/// they just finished typing.
///
/// The count is measured on the **result** rather than on how much of it is still unanswered — the
/// two diverge on a full page whose logos are nearly all in hand, which is a page nobody named and
/// would otherwise buy the explicit effort with its own cache hits.
#[test]
fn only_a_fresh_narrow_result_is_the_station_the_user_asked_for() {
    assert_eq!(Effort::for_result(true, 1), Effort::Explicit);
    assert_eq!(Effort::for_result(true, EXPLICIT_RESULT_MAX), Effort::Explicit);
    assert_eq!(
        Effort::for_result(true, EXPLICIT_RESULT_MAX + 1),
        Effort::Page,
        "a page this wide is stations nobody named, whatever was typed to reach it"
    );
    assert_eq!(
        Effort::for_result(false, 1),
        Effort::Page,
        "paging on and re-warming after a leave are the same stations a second time, and the \
         session already holds their answers"
    );
}

/// The bug the effort split exists for.
///
/// `None` is written by a genuine miss, by a transport failure, and by the backoff sweep, so a
/// wide keystroke on the way to a narrow one poisons every URL the two share — and the narrow
/// result then never asks, which is a station the user is looking at painting a monogram.
#[test]
fn a_narrow_result_re_asks_what_this_session_gave_up_on() {
    let memo = LogoMemo::new();
    memo.record(URL.to_owned(), None);

    assert!(
        memo.unanswered(std::iter::once(URL), Effort::Page).is_empty(),
        "a page nobody named takes the session's own answer, miss included"
    );
    assert_eq!(
        memo.unanswered(std::iter::once(URL), Effort::Explicit),
        vec![URL.to_owned()],
        "a miss is not an answer when the user is looking at the station that earned it"
    );
}

/// A hit is an answer under either effort — re-downloading a logo already in the store buys
/// nothing, and the URL-keyed memo is what makes a *moved* logo land on a new file anyway.
#[test]
fn a_logo_already_found_is_never_asked_for_twice() {
    let memo = LogoMemo::new();
    memo.record(URL.to_owned(), Some("/store/abc.png".to_owned()));

    for effort in [Effort::Page, Effort::Explicit] {
        assert!(memo.unanswered(std::iter::once(URL), effort).is_empty(), "{effort:?}");
    }
}

/// A page carries the same host repeatedly and the directory serves empty logo fields by the
/// hundred; neither is worth a request, and the surviving order is what puts the visible prefix
/// in flight first.
#[test]
fn what_is_asked_about_is_deduplicated_blank_free_and_in_page_order() {
    let memo = LogoMemo::new();
    let page = [
        "https://b.invalid/a.png",
        "",
        "https://a.invalid/a.png",
        "https://b.invalid/a.png",
    ];

    assert_eq!(
        memo.unanswered(page.into_iter(), Effort::Page),
        vec![
            "https://b.invalid/a.png".to_owned(),
            "https://a.invalid/a.png".to_owned()
        ],
    );
}
