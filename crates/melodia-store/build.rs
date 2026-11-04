//! Embeds `migrations/` in version order, each file beside the SHA-384 of its bytes, the checksum
//! `_sqlx_migrations` records. Hashed here so a boot compares without reading or hashing a file.

use sha2::{Digest, Sha384};
use std::fmt::Display;
use std::path::{Path, PathBuf};

fn main() {
    let dir = repo_root().join("migrations");
    println!("cargo:rerun-if-changed={}", dir.display());

    let mut migrations = read_migrations(&dir);
    migrations.sort_by_key(|migration| migration.version);
    if let Some([first, _]) = migrations.array_windows().find(|[a, b]| a.version == b.version) {
        fail(format_args!("two migrations share version {}", first.version));
    }

    let Some(out_dir) = std::env::var_os("OUT_DIR") else {
        fail("OUT_DIR is unset");
    };
    let dest = Path::new(&out_dir).join("migrations.rs");
    if let Err(e) = std::fs::write(&dest, render(&migrations)) {
        fail(format_args!("failed to write {}: {e}", dest.display()));
    }
}

/// One `<version>_<description>.sql`, its description spelled as sqlx records it.
struct MigrationFile {
    version: i64,
    description: String,
    path: String,
    checksum: Vec<u8>,
}

fn repo_root() -> PathBuf {
    match std::env::var("CARGO_MANIFEST_DIR") {
        Ok(manifest) => Path::new(&manifest).join("..").join(".."),
        Err(e) => fail(format_args!("CARGO_MANIFEST_DIR: {e}")),
    }
}

/// Every entry but a dotfile, which an editor leaves beside the file it has open.
fn read_migrations(dir: &Path) -> Vec<MigrationFile> {
    let entries = std::fs::read_dir(dir)
        .unwrap_or_else(|e| fail(format_args!("failed to read {}: {e}", dir.display())));
    let mut migrations = Vec::new();
    for entry in entries {
        let path = match entry {
            Ok(entry) => entry.path(),
            Err(e) => fail(format_args!("failed to read {}: {e}", dir.display())),
        };
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            fail(format_args!("{} is not a UTF-8 name", path.display()));
        };
        if name.starts_with('.') {
            continue;
        }
        migrations.push(read_migration(name, &path));
    }
    migrations
}

/// Refuses what sqlx would skip or treat apart, the directory holding nothing but migrations
/// that each run inside a transaction.
fn read_migration(name: &str, path: &Path) -> MigrationFile {
    if !path.is_file() {
        fail(format_args!("{name} is not a file"));
    }
    let Some((version, description)) =
        name.strip_suffix(".sql").and_then(|stem| stem.split_once('_'))
    else {
        fail(format_args!("{name} is not named `<version>_<description>.sql`"));
    };
    let reversible = Path::new(description)
        .extension()
        .is_some_and(|half| half.eq_ignore_ascii_case("up") || half.eq_ignore_ascii_case("down"));
    if reversible {
        fail(format_args!("{name}: reversible migrations are not supported"));
    }
    let version: i64 = match version.parse() {
        Ok(version) if version > 0 => version,
        Ok(_) => fail(format_args!("{name}: versions start at 1")),
        Err(e) => fail(format_args!("{name}: version {version:?}: {e}")),
    };

    let sql = std::fs::read_to_string(path)
        .unwrap_or_else(|e| fail(format_args!("failed to read {name}: {e}")));
    if sql.starts_with("-- no-transaction") {
        fail(format_args!("{name}: every migration runs inside a transaction"));
    }
    let Some(path) = path.to_str() else {
        fail(format_args!("{} is not a UTF-8 path", path.display()));
    };

    MigrationFile {
        version,
        description: description.replace('_', " "),
        path: path.to_owned(),
        checksum: Sha384::digest(sql.as_bytes()).to_vec(),
    }
}

/// An array expression of `database::migrate::Migration`s, the SQL itself left to `include_str!`.
fn render(migrations: &[MigrationFile]) -> String {
    let entries: Vec<String> = migrations
        .iter()
        .map(|migration| {
            let checksum: Vec<String> =
                migration.checksum.iter().map(|byte| format!("{byte:#04x}")).collect();
            format!(
                "    Migration {{ version: {}, description: {:?}, sql: include_str!({:?}), checksum: [{}] }},",
                grouped(migration.version),
                migration.description,
                migration.path,
                checksum.join(", "),
            )
        })
        .collect();
    format!("&[\n{}\n]\n", entries.join("\n"))
}

/// Digits grouped in threes, since the file compiles into the crate, where
/// `clippy::unreadable_literal` denies a bare timestamp.
fn grouped(version: i64) -> String {
    let digits = version.to_string();
    let mut literal = String::with_capacity(digits.len() + digits.len() / 3);
    for (position, digit) in digits.char_indices() {
        if position > 0 && (digits.len() - position).is_multiple_of(3) {
            literal.push('_');
        }
        literal.push(digit);
    }
    literal
}

fn fail(reason: impl Display) -> ! {
    println!("cargo::error=migrations: {reason}");
    std::process::exit(1);
}
