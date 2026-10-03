//! Tests for the rate sweeps remembered across claims: which asks an answer serves, which sweeps
//! are not kept, and which answer makes room once the set is full.

use std::convert::Infallible;

use super::{Answers, CAPACITY, ProbeKey};
use crate::player::playback::output::rates::RateSet;

const ENDPOINT: &str = "{0.0.0.00000000}.{a-test-endpoint}";

/// The UMC22's answer on Windows, standing for whatever a sweep found earlier.
fn kept() -> RateSet {
    [32_000, 44_100, 48_000].into_iter().collect()
}

/// What the device answers when it is asked now, so an ask answering this one swept again.
fn swept_now() -> RateSet {
    [44_100, 48_000, 96_000, 192_000].into_iter().collect()
}

/// A stereo source on a stereo endpoint.
fn stereo_on(device: &str) -> ProbeKey {
    ProbeKey { device: device.to_owned(), from_channels: 2, device_channels: 2 }
}

/// The endpoint `place` sweeps into a session.
fn endpoint(place: usize) -> ProbeKey {
    stereo_on(&format!("{{0.0.0.00000000}}.{{endpoint-{place}}}"))
}

/// `answers` after a sweep for `key` found [`kept`].
fn remember(answers: &Answers, key: ProbeKey) {
    let _ = answers.answer(key, || Ok::<_, Infallible>(kept()));
}

/// What `answers` says for `key` where sweeping now would find [`swept_now`].
fn ask(answers: &Answers, key: ProbeKey) -> Result<RateSet, Infallible> {
    answers.answer(key, || Ok(swept_now()))
}

/// Answers kept for `count` endpoints, swept in the order of their places.
fn swept_endpoints(count: usize) -> Answers {
    let answers = Answers::new();
    for place in 0..count {
        remember(&answers, endpoint(place));
    }
    answers
}

/// On a 7.1 output a sweep takes the better part of a second, which is what remembering it saves
/// every claim after the first.
#[test]
fn a_second_claim_on_the_same_endpoint_and_layout_reads_the_kept_answer() {
    let answers = Answers::new();
    remember(&answers, stereo_on(ENDPOINT));

    let offered = ask(&answers, stereo_on(ENDPOINT));

    assert_eq!(offered, Ok(kept()));
}

/// The key is everything the sweep asked with, so an answer only stands for a sweep that would
/// have asked the same questions. A change in Speaker Setup moves the device's channel count, and
/// can move the rates with it.
#[test]
fn a_claim_differing_in_any_part_of_the_key_sweeps_again() {
    let rows = [
        ("another endpoint", stereo_on("{0.0.0.00000000}.{another-endpoint}")),
        ("a mono source", ProbeKey { from_channels: 1, ..stereo_on(ENDPOINT) }),
        ("the endpoint set to 7.1", ProbeKey { device_channels: 8, ..stereo_on(ENDPOINT) }),
    ];
    for (what, key) in rows {
        let answers = Answers::new();
        remember(&answers, stereo_on(ENDPOINT));

        let offered = ask(&answers, key);

        assert_eq!(offered, Ok(swept_now()), "{what}");
    }
}

/// A device taken, barred or unplugged ends the sweep with its refusal, and the claim falls back on
/// that, so the panel names the cause rather than a device offering no rates.
#[test]
fn a_failed_sweep_answers_with_its_own_refusal() {
    let answers = Answers::new();

    let offered = answers.answer(stereo_on(ENDPOINT), || Err("the device was taken"));

    assert_eq!(offered, Err("the device was taken"));
}

/// A sweep cut short would otherwise stand for the session, short of rates the device has, so the
/// next claim on the device asks it again.
#[test]
fn a_failed_sweep_is_not_kept() {
    let answers = Answers::new();
    let _ = answers.answer(stereo_on(ENDPOINT), || Err("the device was taken"));

    let offered = answers.answer(stereo_on(ENDPOINT), || Ok::<_, &str>(swept_now()));

    assert_eq!(offered, Ok(swept_now()));
}

/// The boundary from below: a full set has dropped nothing, its oldest answer included.
#[test]
fn a_full_set_still_holds_its_oldest_answer() {
    let answers = swept_endpoints(CAPACITY);

    let oldest = ask(&answers, endpoint(0));

    assert_eq!(oldest, Ok(kept()));
}

/// One past the boundary drops one answer, and it is the oldest: the next-oldest still reads back,
/// and the oldest is swept again. Asked in that order, since sweeping the oldest again makes room
/// for it by dropping the next.
#[test]
fn one_answer_past_a_full_set_drops_only_the_oldest() {
    let answers = swept_endpoints(CAPACITY + 1);

    let next_oldest = ask(&answers, endpoint(1));
    let oldest = ask(&answers, endpoint(0));

    assert_eq!((next_oldest, oldest), (Ok(kept()), Ok(swept_now())));
}
