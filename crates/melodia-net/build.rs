//! Bakes the radio blocklist's fingerprints into `$OUT_DIR`. Here rather than in the binary's
//! script because `OUT_DIR` is per crate; the dotfiles stay at the repo root, reached through
//! `CARGO_MANIFEST_DIR`.

use std::path::{Path, PathBuf};

fn main() {
    write_blocklist();
}

/// The repo root, two directories above this crate.
fn repo_root() -> PathBuf {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
    Path::new(&manifest).join("..").join("..")
}

/// The crate's own normalization and hash, so the baking and the lookup can't disagree.
mod blocklist {
    include!("src/services/net/radio_blocklist/source.rs");
}

/// Leaves the pre-hashed form beside the list, for `gh secret set`. Here because the hashing
/// already happened and a second tool would need a `Cargo.toml` entry. Not an error if it fails:
/// a read-only checkout should still build.
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

/// Digits grouped so the generated file clears `clippy::unreadable_literal`.
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

/// Relative to [`repo_root`], and caught by `.gitignore`'s `.env.*.local` rule.
const BLOCKLIST_FILE: &str = ".env.radio.local";

/// The source's *contents*, which is how CI hands over a secret it has no file for.
const BLOCKLIST_ENV: &str = "MELODIA_RADIO_BLOCKLIST";

/// The pre-hashed form, beside the source and under the same ignore rule.
const BLOCKLIST_HASHED_FILE: &str = ".env.radio.hashed.local";

/// The environment wins over the file, an empty value counts as absent (CI substitutes `""` for
/// an unset secret), and no source is a silent no-op. A source that won't parse fails the build,
/// a skipped line unblocking a station with nothing to report it.
///
/// **Nothing here may print a term**: build logs are public, hence line numbers and counts only.
fn write_blocklist() {
    println!("cargo:rerun-if-env-changed={BLOCKLIST_ENV}");
    println!("cargo:rerun-if-changed={}", repo_root().join(BLOCKLIST_FILE).display());
    // Without this the hashes would survive a change to how they are computed.
    println!("cargo:rerun-if-changed=src/services/net/radio_blocklist/source.rs");

    let from_environment =
        std::env::var(BLOCKLIST_ENV).ok().filter(|contents| !contents.trim().is_empty());
    let from_file = std::fs::read_to_string(repo_root().join(BLOCKLIST_FILE)).ok();
    // Only a build that read the *list* has a fresh hashing to leave, and a CI workspace is where
    // a caching or artifact step could pick one up.
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
    // Separated because the file compiles into the crate, where `clippy::unreadable_literal`
    // denies a bare 19-digit literal.
    let fingerprints =
        terms.fingerprints.iter().map(|f| separated(*f)).collect::<Vec<_>>().join(",");
    let patterns = terms.patterns.iter().map(|f| separated(*f)).collect::<Vec<_>>().join(",");
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
