fn main() {
    // Inject compile-time secrets (the Last.fm API keys) from a local, gitignored
    // `.env` so dev builds work without exporting them each session. Guarded on
    // every side — see `load_dotenv`.
    //
    // This stays put rather than following the Slint compilation into
    // `melodia-ui/build.rs`: `cargo:rustc-env` only reaches the crate whose build
    // script emitted it, so moving it would leave `option_env!` resolving to
    // `None` and every build silently shipping keyless.
    load_dotenv();
    write_blocklist();

    #[cfg(target_os = "windows")]
    embed_windows_icon();
}

/// The shared normalization, key derivation and hash, compiled into this script as
/// well as into the crate so the terms baked here and the values looked up at run
/// time cannot come to disagree. A drift there has no symptom — every lookup would
/// simply stop matching.
mod blocklist {
    include!("src/services/radio_blocklist/source.rs");
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
    if std::fs::read_to_string(BLOCKLIST_HASHED_FILE).is_ok_and(|current| current == rendered) {
        return;
    }
    if let Err(e) = std::fs::write(BLOCKLIST_HASHED_FILE, rendered) {
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

/// Where the blocklist source is read from when it isn't in the environment.
///
/// Matches `.gitignore`'s pre-existing `.env.*.local` rule, so keeping it out of the
/// repo costs no entry of its own.
const BLOCKLIST_FILE: &str = ".env.radio.local";

/// The environment variable carrying the source's *contents*, which is how CI hands
/// over a secret it has no file for.
const BLOCKLIST_ENV: &str = "MELODIA_RADIO_BLOCKLIST";

/// Where the pre-hashed form is left after a local build, for the CI secret to be
/// set from. Covered by the same `.env.*.local` ignore rule as the source.
const BLOCKLIST_HASHED_FILE: &str = ".env.radio.hashed.local";

/// Bake the radio blocklist's fingerprints into `$OUT_DIR` for
/// `services::radio_blocklist` to `include!`.
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
/// protecting — hence line numbers and counts only, both here and in
/// `blocklist::parse_source`.
fn write_blocklist() {
    println!("cargo:rerun-if-env-changed={BLOCKLIST_ENV}");
    println!("cargo:rerun-if-changed={BLOCKLIST_FILE}");
    // Without this the hashes would survive a change to how they are computed.
    println!("cargo:rerun-if-changed=src/services/radio_blocklist/source.rs");

    let from_environment =
        std::env::var(BLOCKLIST_ENV).ok().filter(|contents| !contents.trim().is_empty());
    let from_file = std::fs::read_to_string(BLOCKLIST_FILE).ok();
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

/// Embed `assets/melodia.ico` as the EXE's primary `ICON` resource. Windows'
/// shell pulls this for the titlebar's top-left glyph, the taskbar button, the
/// Alt-Tab thumbnail badge, and the Explorer file icon. Without an embedded
/// resource the running window falls back to a generic placeholder even when
/// the Start-Menu shortcut has its own icon (`WiX` `ProductICO`).
#[cfg(target_os = "windows")]
fn embed_windows_icon() {
    println!("cargo:rerun-if-changed=assets/melodia.ico");
    let mut res = winresource::WindowsResource::new();
    res.set_icon("assets/melodia.ico");
    if let Err(e) = res.compile() {
        // `cargo::error=` is the build script's own failure channel — a clearer
        // report than unwinding out of `main` with a Debug-formatted error.
        println!("cargo::error=failed to embed assets/melodia.ico: {e}");
        std::process::exit(1);
    }
}

/// Load compile-time secrets (the Last.fm API keys) from a local, gitignored
/// `.env`, injecting each `KEY=value` via `cargo:rustc-env` so `option_env!(KEY)`
/// picks it up at compile time. Purely a local-dev convenience — guarded so no
/// other build path can break:
///
/// - **No `.env`** — the case for contributors, forks, and release/CI builds — is
///   a silent no-op, so a build never fails on its absence.
/// - **The environment wins**: a key already set (a shell export, or CI's
///   GitHub-secret env var from `release.yml`) is left untouched, never
///   overwritten by `.env`.
///
/// A keyless build is fully supported: `lastfm::is_configured()` returns false
/// and the app ships ListenBrainz-only with an inert Last.fm Connect button.
fn load_dotenv() {
    // Re-run this script when `.env` appears / changes / vanishes. Harmless when
    // the file never exists.
    println!("cargo:rerun-if-changed=.env");
    let Ok(contents) = std::fs::read_to_string(".env") else {
        return;
    };
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim().trim_matches(['"', '\'']);
        // Only fall back to `.env` when the variable isn't already in the build
        // environment, so a shell export or CI secret is never clobbered.
        if !key.is_empty() && std::env::var_os(key).is_none() {
            println!("cargo:rustc-env={key}={value}");
        }
    }
}
