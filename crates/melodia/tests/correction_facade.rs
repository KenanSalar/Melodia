//! The `library::correction` facade's contract, in the parts only its source text carries.
//!
//! One switch decides whether Melodia may reach `AutoEq`, and it is read in one place:
//! `autoeq_client`, which checks it before handing out the shared client. That holds only while
//! every fetch in the facade takes its client there, the facade builds none of its own, and
//! nothing outside it reaches `AutoEq`'s client module at all. Neither half covers the other: the
//! first passes with a second module fetching on its own, the second with every fetch inside the
//! facade on a raw client.

use melodia_testkit::{block_after, rust_sources, stripped_sources};

/// The facade's own tree, from the repo root.
const FACADE_DIR: &str =
    concat!(env!("MELODIA_REPO_ROOT"), "crates/melodia-app/src/library/correction");

/// Vacuity floor over the facade's production files, loose enough that folding two together does
/// not trip it.
const MIN_FACADE_FILES: usize = 3;

/// The directory each of the facade's unit-test files sits in, matched as a whole segment.
const FACADE_TESTS: &str = "tests";

/// Every production file the facade is made of, concatenated, line comments stripped.
///
/// Read off the directory, so a submodule nobody has written yet is covered the day it lands.
/// `tests/` is dropped: a test naming the switch is not a second reader of it, and the facade's
/// tests outnumber its floor, so they could otherwise stand in for a facade deleted under them.
fn facade_source() -> String {
    let mut source = String::new();
    let mut files = 0usize;

    for (path, text) in stripped_sources(FACADE_DIR, "rs", MIN_FACADE_FILES) {
        if path.split('/').any(|segment| segment == FACADE_TESTS) {
            continue;
        }
        files += 1;
        source.push_str(&text);
        source.push('\n');
    }

    assert!(
        files >= MIN_FACADE_FILES,
        "only {files} production files under `library::correction`, so the pins below are \
         reading a facade that has mostly stopped existing"
    );
    source
}

/// An equality, failing both ways: a second door reading the switch for itself takes the count to
/// two, and deleting the one that enforces it to zero.
#[test]
fn the_online_switch_is_read_in_one_place() {
    let source = facade_source();

    assert_eq!(
        source.matches("correction_online").count(),
        1,
        "`correction_online` may be named exactly once in `library::correction`, inside \
         `autoeq_client`: a second reader is a guard nothing else can hold"
    );
}

/// One guard covers every fetch only while each takes its client from `autoeq_client`. Counted on
/// any receiver: a reach spelled off another binding is the thing this is for.
#[test]
fn every_fetch_takes_its_client_from_behind_the_switch() {
    let source = facade_source();
    let seam = block_after(&source, "fn autoeq_client");

    assert_eq!(
        source.matches(".http_client()").count(),
        1,
        "`http_client()` may be named exactly once in `library::correction`, inside \
         `autoeq_client`: any other reach is traffic a user who left the switch off still pays"
    );
    assert!(!seam.is_empty(), "`autoeq_client` moved or changed shape");
    assert!(
        seam.contains("correction_online") && seam.contains(".http_client()"),
        "the one client this counts has to be handed out by the seam, behind its switch"
    );
}

/// The route the two counts above can't see: building a client costs a line and reads like
/// plumbing, not like a hole in a switch.
#[test]
fn the_correction_facade_builds_no_client_of_its_own() {
    const CONSTRUCTORS: [&str; 3] = ["Client::new", "Client::builder", "ClientBuilder"];
    let source = facade_source();

    let built: Vec<&str> =
        CONSTRUCTORS.into_iter().filter(|needle| source.contains(needle)).collect();

    assert!(
        built.is_empty(),
        "`library::correction` names {built:?}, so it can reach `AutoEq` without asking \
         `autoeq_client`; take the client from the seam, which is where the switch is"
    );
}

/// The client module's name, spelled as a whole identifier.
const MODULE: &str = "autoeq";
/// Where the module is declared.
const CLIENT_DECL: &str = "services/net/mod.rs";
/// The client's own tree, trailing separator included so a sibling sharing the prefix isn't
/// exempted with it.
const CLIENT_TREE: &str = "services/net/autoeq/";
/// The one facade allowed to name it.
const CALLER_TREE: &str = "library/correction/";

fn is_identifier_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Whether `source` names the module itself, which the data folder's `autoeq_list_path` and the
/// facade's own `autoeq_client` do not.
fn names_the_module(source: &str) -> bool {
    source.match_indices(MODULE).any(|(at, _)| {
        let before = source[..at].chars().next_back();
        let after = source[at + MODULE.len()..].chars().next();
        !before.is_some_and(is_identifier_char) && !after.is_some_and(is_identifier_char)
    })
}

/// A whole identifier rather than a path, so a grouped import or a `super::` spelling can't step
/// round it. Each exemption is asserted to still match: a moved declaration pre-authorises
/// whatever takes its path next, and a facade that stopped naming the module leaves this passing
/// over nothing.
#[test]
fn only_the_correction_facade_reaches_autoeq() {
    let mut strays = Vec::new();
    let mut declaration_seen = false;
    let mut facade_files = 0usize;

    for (path, source) in rust_sources() {
        if path.starts_with(CLIENT_TREE) || !names_the_module(&source) {
            continue;
        }
        if path == CLIENT_DECL {
            declaration_seen = true;
        } else if path.starts_with(CALLER_TREE) {
            facade_files += 1;
        } else {
            strays.push(path);
        }
    }

    assert!(
        strays.is_empty(),
        "{strays:?} name `{MODULE}`: every fetch goes through `library::correction`, the only \
         place its switch can stop one"
    );
    assert!(
        declaration_seen,
        "`{CLIENT_DECL}` no longer names `{MODULE}`, so a moved declaration has pre-authorised \
         whatever takes its path next"
    );
    assert!(
        facade_files > 0,
        "no file under `{CALLER_TREE}` names `{MODULE}`, so the facade has stopped being the \
         door and this walk is passing over an empty set"
    );
}
