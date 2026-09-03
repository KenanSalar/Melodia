use super::{reading_env, with_env_set};

/// Names nothing real, so a leak is unambiguous and no sibling test can be the
/// one that set it.
const PROBE: &str = "MELODIA_ENV_SUPPORT_PROBE";

#[test]
fn a_panicking_body_still_restores_what_it_was_handed() {
    // The restore is the step that has gone missing from every hand-rolled copy
    // of this helper, and it goes missing silently: the leaked variable surfaces
    // as an unrelated test failing later, in whatever order the harness happened
    // to run things.
    let before = reading_env(|| std::env::var(PROBE).ok());

    let caught = std::panic::catch_unwind(|| {
        with_env_set(&[PROBE], &[(PROBE, "set-by-the-body")], || {
            let seen = std::env::var(PROBE).ok();
            assert_eq!(seen.as_deref(), Some("set-by-the-body"), "`set` applied");
            // The deliberate failure, spelled as an assertion rather than a bare
            // `panic!` — which is denied crate-wide, and which is what this is
            // standing in for anyway.
            assert!(seen.is_none(), "deliberate: a test failing under the helper");
        });
    });

    assert!(caught.is_err(), "the panic must reach the caller");
    assert_eq!(
        reading_env(|| std::env::var(PROBE).ok()),
        before,
        "the body's variable outlived it",
    );
}

#[test]
#[should_panic(expected = "not reentrant")]
fn nesting_the_env_helpers_panics_rather_than_deadlocking() {
    // One lock for the binary buys serialisation at the cost of reentrancy, and
    // the failure mode is the worst kind: no message, no failing assertion, just
    // a test binary that never finishes. The thread-local flag is what makes it
    // say so instead, and this is what says the flag still works.
    with_env_set(&[PROBE], &[], || reading_env(|| std::env::var(PROBE).ok()));
}
