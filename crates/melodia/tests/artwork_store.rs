//! Nothing writes into the artwork store without staging and renaming.
//!
//! A half-written file under a content-addressed name is permanent: the hash is the name, so
//! nothing ever recomputes it, and the decode that fails on the truncated bytes is cached. The
//! sweep cannot help either — the row still references it.
//!
//! Asked of whatever holds a store directory rather than of a ledger of today's writers, which
//! is why it walks every crate: the writer this is about is the one added next, in a file no
//! ledger has heard of.

use melodia_testkit::rust_sources;

/// A bare `fs::write` leaves a truncated file on a crash, a full disk or a force-exit — and
/// because the name is content-addressed and every writer guards on `exists()`, that file is
/// never rewritten and the cover is gone until someone deletes it by hand. `CoverThumbs` then
/// caches the failed decode, so it does not even retry. The sweep cannot help: it is still
/// referenced.
///
/// **Asked of whatever holds a store directory, rather than of a ledger of today's writers.** A
/// ledger only ever catches its own entries being renamed; the writer this is about is the one
/// added next, in a file the ledger has never heard of.
#[test]
fn nothing_writes_into_the_store_without_staging_and_renaming() {
    /// A floor for the sources that reach a store directory — most only pass one along, so this
    /// stays a floor rather than becoming an equality that fails on every new courier.
    const MIN_STORE_TOUCHING: usize = 8;

    let mut touching = 0;
    for (path, code) in rust_sources() {
        // Test sources stage nothing and write wherever their tempdir is. By segment rather than
        // by substring: a crate's top-level `src/tests/` comes back with no leading slash.
        if path.split('/').any(|segment| segment == "tests")
            || !(code.contains("artwork_dir") || code.contains("artists_dir"))
        {
            continue;
        }
        assert!(
            !code.contains("fs::write(") && !code.contains("File::create("),
            "{path} holds a store directory and writes a file in one step — go through \
             `write_atomic`, which stages beside the destination and renames, so the final name \
             existing means the file is complete"
        );
        touching += 1;
    }

    assert!(
        touching >= MIN_STORE_TOUCHING,
        "only {touching} sources reach a store directory, so the walk has stopped finding them"
    );
}
