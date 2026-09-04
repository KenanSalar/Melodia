//! Bakes the radio blocklist's fingerprints into `$OUT_DIR` for `services::net::radio_blocklist`
//! to `include!`.
//!
//! Here rather than in the binary's build script because `OUT_DIR` is per-crate: the file this
//! writes is only reachable from the crate whose script wrote it. The two dotfiles it reads and
//! writes stay at the repo root, where the `gh secret` workflow already looks for them, reached
//! through `CARGO_MANIFEST_DIR` since cargo sets a build script's working directory to its own
//! package root.

use std::path::{Path, PathBuf};

fn main() {
    write_blocklist();
}

/// The repo root, two directories above this crate.
fn repo_root() -> PathBuf {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
    Path::new(&manifest).join("..").join("..")
}

/// The shared normalization, key derivation and hash, compiled into this script as
/// well as into the crate so the terms baked here and the values looked up at run
/// time cannot come to disagree. A drift there has no symptom — every lookup would
/// simply stop matching.
mod blocklist {
    include!("src/services/net/radio_blocklist/source.rs");
}

/// Leave the pre-hashed form beside the list, for `gh secret set` to read.
///
/// Written here rather than by a separate tool because the hashing already happened:
/// a second binary or example target to redo it would cost a `Cargo.toml` entry
/// naming the feature, which is the visible surface this whole path avoids.
///
/// **Not an error if it fails.** It is a convenience copy of numbers that ship in
/// the binary regardless, so a read-only checkout should still build.
fn refresh_hashed_copy(terms: &blocklist::Terms) {
    let rendered = blocklist::render_hashed(terms);
    // An identical rewrite would touch the mtime on every build for nothing.
    let dest = repo_root().join(BLOCKLIST_HASHED_FILE);
    if std::fs::read_to_string(&dest).is_ok_and(|current| current == rendered) {
        return;
    }
    if let Err(e) = std::fs::write(&dest, rendered) {
        println!("cargo::warning=radio blocklist: could not refresh {BLOCKLIST_HASHED_FILE}: {e}");
    }
}

/// One fingerprint as a Rust literal, digits grouped so the generated file clears
/// `clippy::unreadable_literal`.
fn separated(value: u64) -> String {
    let digits = value.to_string();
    let mut literal = String::with_capacity(digits.len() + digits.len() / 3);
    for (position, digit) in digits.char_indices() {
        if position > 0 && (digits.len() - position).is_multiple_of(3) {
            literal.push('_');
        }
        literal.push(digit);
    }
    literal
}

/// Where the blocklist source is read from when it isn't in the environment, relative to
/// [`repo_root`] rather than to this package.
///
/// Matches `.gitignore`'s pre-existing `.env.*.local` rule, so keeping it out of the
/// repo costs no entry of its own.
const BLOCKLIST_FILE: &str = ".env.radio.local";

/// The environment variable carrying the source's *contents*, which is how CI hands
/// over a secret it has no file for.
const BLOCKLIST_ENV: &str = "MELODIA_RADIO_BLOCKLIST";

/// Where the pre-hashed form is left after a local build, for the CI secret to be
/// set from, beside the source at [`repo_root`]. Covered by the same `.env.*.local`
/// ignore rule.
const BLOCKLIST_HASHED_FILE: &str = ".env.radio.hashed.local";

/// Bake the radio blocklist's fingerprints into `$OUT_DIR` for
/// `services::net::radio_blocklist` to `include!`.
///
/// [`load_dotenv`]'s three guards, for its reasons: the environment wins over the
/// file, an empty value counts as absent (CI substitutes `""` for a secret that
/// isn't set), and no source at all is a silent no-op that blocks nothing.
///
/// A source that *is* present and won't parse fails the build instead. A skipped
/// line would unblock a station with nothing anywhere to report it, which is the one
/// failure this whole path is shaped to avoid.
///
/// **Nothing here may print a term.** Build logs are public on a public repository,
/// so a message quoting the line it choked on would hand over the entry it was
/// protecting — hence line numbers and counts only. The parser's half is structural:
/// `blocklist::Refusal` is fieldless and has nowhere to put one.
fn write_blocklist() {
    println!("cargo:rerun-if-env-changed={BLOCKLIST_ENV}");
    println!("cargo:rerun-if-changed={}", repo_root().join(BLOCKLIST_FILE).display());
    // Without this the hashes would survive a change to how they are computed.
    println!("cargo:rerun-if-changed=src/services/net/radio_blocklist/source.rs");

    let from_environment =
        std::env::var(BLOCKLIST_ENV).ok().filter(|contents| !contents.trim().is_empty());
    let from_file = std::fs::read_to_string(repo_root().join(BLOCKLIST_FILE)).ok();
    // Only a build that read the *list* has a fresh hashing to leave behind. A CI
    // build is already handed the result, and writing into a runner's workspace would
    // put it where a caching or artifact step could pick it up.
    let refresh_hashed = from_environment.is_none() && from_file.is_some();
    let source = from_environment.or(from_file).unwrap_or_default();

    let terms = match blocklist::parse_any(&source) {
        Ok(terms) => terms,
        Err(reason) => {
            println!("cargo::error=radio blocklist: {reason}");
            std::process::exit(1);
        }
    };

    if refresh_hashed {
        refresh_hashed_copy(&terms);
    }

    let key = terms.key.map(|byte| byte.to_string()).join(",");
    let term_count = terms.fingerprints.len();
    let pattern_count = terms.patterns.len();
    let length_count = terms.pattern_lengths.len();
    // No `u64` suffixes: the declared array type is what gives them their type. The
    // digit separators are not cosmetic — the generated file is compiled as part of
    // the crate and `clippy::unreadable_literal` denies a bare 19-digit literal, so
    // without them any build carrying a non-empty list fails the gate.
    let fingerprints =
        terms.fingerprints.iter().map(|f| separated(*f)).collect::<Vec<_>>().join(",");
    let patterns = terms.patterns.iter().map(|f| separated(*f)).collect::<Vec<_>>().join(",");
    // Lengths are short enough to need no separators.
    let lengths = terms.pattern_lengths.iter().map(u32::to_string).collect::<Vec<_>>().join(",");
    let generated = format!(
        "const BLOCKED_KEY: [u8; 32] = [{key}];\n\
         const BLOCKED_TERMS: [u64; {term_count}] = [{fingerprints}];\n\
         const BLOCKED_PATTERNS: [u64; {pattern_count}] = [{patterns}];\n\
         const PATTERN_LENGTHS: [u32; {length_count}] = [{lengths}];\n"
    );

    let Some(out_dir) = std::env::var_os("OUT_DIR") else {
        println!("cargo::error=radio blocklist: OUT_DIR is unset");
        std::process::exit(1);
    };
    let dest = std::path::Path::new(&out_dir).join("radio_blocklist_terms.rs");
    if let Err(e) = std::fs::write(&dest, generated) {
        println!("cargo::error=radio blocklist: failed to write {}: {e}", dest.display());
        std::process::exit(1);
    }
}
