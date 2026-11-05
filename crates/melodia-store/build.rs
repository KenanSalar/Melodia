//! Embeds `migrations/` in version order, each file beside the checksum `_sqlx_migrations`
//! records for it. Hashed here so a boot compares without reading or hashing a file.

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
    if let Some(missing) = SQLX_APPLIED.iter().find(|pinned| !dir.join(pinned.file).is_file()) {
        fail(format_args!("{} shipped and is missing", missing.file));
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

/// A shipped file's hashes, in the lowercase hex `sha384sum` and `b3sum` print.
struct Pinned {
    file: &'static str,
    sha384: &'static str,
    blake3: &'static str,
}

/// The migrations sqlx applied, pinned to the SHA-384 their history rows hold and to the blake3
/// of each file as shipped. Every later file records its blake3: a release migrating through sqlx
/// refuses a version it doesn't know before it compares any checksum, so it never reads one.
const SQLX_APPLIED: [Pinned; 14] = [
    Pinned {
        file: "20260514000000_initial_schema.sql",
        sha384: "32a38d680e3ec2d92701eafdd340f8ee68bce38b6b9eb53418cea8139b032626775e9c8a097b70ed28ff975253559fa0",
        blake3: "a1adf5c6add1f20ce9944f92428c59a0ebf9ec764d325123ec7ea848f4793473",
    },
    Pinned {
        file: "20260612000000_drop_unused_track_indexes.sql",
        sha384: "cdfe6ad917d6aa3a484c62ba3b2dc1f995e007f175dae36871b071077fe45c834aa51d97f58866c1cd27a5b25b0c2f31",
        blake3: "8fea758a2d0f7b3069f92beb8788c1d1fa67366139e68b8330acb85b67286ba4",
    },
    Pinned {
        file: "20260705000000_readd_last_played_index.sql",
        sha384: "188ed1bdc6482173bb95ac651d5a025bf793d254ce407eb6156eb217e5f74c8709b47d7e459180122292606e7a990111",
        blake3: "1da6676f341918521e3fc3f94b790f852337d277e6a4dd927ba04a8bcfefb608",
    },
    Pinned {
        file: "20260708000000_readd_rating_index.sql",
        sha384: "2ad30baed99dc0b0fb25c130a3fff46dc0696a0223ecc2b32018f4d320875ef9bed1fc12395941c0f77eac73b613fe2e",
        blake3: "63659eae0da829264ab21656f29eecfab1cb1d4075aacb7df01c81ec880f2021",
    },
    Pinned {
        file: "20260802000000_drop_favorite_album_index.sql",
        sha384: "5d9f540a99fcb49ff1b08c2f050e2bd19155edbbd0dd14708d951b9105e6e285ed84062ad91101ecbf3ded217dd28b32",
        blake3: "86118fe4924a8f4c1bae5570f8fc189f253fca2e3b84d535e110e3f9fb8c74a8",
    },
    Pinned {
        file: "20260802000001_extend_tracks_fts.sql",
        sha384: "9eef11bb081c783ce7724a5a4e22c023a19766610d64318059db00481611f4d82de8e98f6f80f25b2734d635dd33f136",
        blake3: "11dadcace2ac75c1862a3cff63228b71e660163fa15d34515f21a12bc5210cf4",
    },
    Pinned {
        file: "20260820000000_radio_stations.sql",
        sha384: "737caa76b7cb50a3602df0c46ee16f3536105e73dcd5d9e8dc9637c4885f529780d4e291ff674d813868cd8e66a68ebc",
        blake3: "f9feb7272a6c3e71c8b2ae14c783ef2d26e6475e9106c3245ef57de6769e376c",
    },
    Pinned {
        file: "20260910000000_tag_vocabulary.sql",
        sha384: "28648a6019bcf8f9a33db62b2866f433cb6c7bee0ab9c3bec26228515a181b18ae2e941f8c8ccccba7b25eacbe37843d",
        blake3: "0e0ac74ab19b23fe739af0c0af4828f7619404cefb403c6a0ed4bacd2b928be6",
    },
    Pinned {
        file: "20260921000000_radio_probed_codec.sql",
        sha384: "df10e455628bc41dddf1df57b2d8a948b27181554e3ce986b6660883bd4922dea320264866fc9ec05b43515754fb8fb2",
        blake3: "df88fca6f0a8f8060ddfcdbfbb32550779f6877c9cd04085aa33aa68bf7ec70d",
    },
    Pinned {
        file: "20261006000000_unknown_artist_has_no_image.sql",
        sha384: "3a387bd42bdcbd1571e2cf141b2685da29cf9dcdc3724a94d94400883bcae6cdfec3eea7eb2989590a35ff5f895dcaac",
        blake3: "57d9c5009f319694a953e785d088d4c57f1d575a300f46f0837396fc56e82eb7",
    },
    Pinned {
        file: "20261014000000_artist_image_search_miss.sql",
        sha384: "c687870196e4541df9ba2f23a3c008b574416a8e1bffd87dacd271f9b437f6fbf0afd1e0b9ab131b908590f7f4b24b52",
        blake3: "821a894e1764639df7d5c5a80cd03cb569e98957846e6ed2c4a650005afc777c",
    },
    Pinned {
        file: "20261018000000_library_search_text.sql",
        sha384: "5499eba735bf42a857e47c0034421890b78bac73331adfe30eb7b2e355daa6828200a6cb94876d49aa34cd45e9dbc379",
        blake3: "5c79301184576dc9bfec8d2fc3e08be244e968dccf6a1ed68b3c8a33375867b4",
    },
    Pinned {
        file: "20261102000000_duplicate_dismissals.sql",
        sha384: "8e74dca86df8bd4c592f26df6e97bb3502c924f9bd3117426a9c502d0744b4014ba0aa3121bc4b64cdae58438bfebd90",
        blake3: "fcca250dbb0715e9a7ff07f273d668f184d6b4c2be30cacec320b6a74870ec57",
    },
    Pinned {
        file: "20261102000001_excluded_files.sql",
        sha384: "be272479ecc02bc36fccda0cdba54f32d2177ffc494da49460c55ce933c484a125baafe99a695eaf55bfa2305eb9e01d",
        blake3: "fe1e1de034a8943484be0f8c01ec899f182daeac5b50476f49ebe78af1a51ca0",
    },
];

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
        checksum: checksum(name, &sql),
    }
}

/// The checksum the history records for `name`: the pinned SHA-384 of a file sqlx applied, once
/// its blake3 shows it unchanged, or the blake3 of any other.
fn checksum(name: &str, sql: &str) -> Vec<u8> {
    let blake3 = blake3::hash(sql.as_bytes());
    let Some(pinned) = SQLX_APPLIED.iter().find(|pinned| pinned.file == name) else {
        return blake3.as_bytes().to_vec();
    };
    if blake3.to_hex().as_str() != pinned.blake3 {
        fail(format_args!("{name} changed after it shipped"));
    }
    decode_hex(pinned.sha384)
        .unwrap_or_else(|| fail(format_args!("{name}: its pinned SHA-384 is not hex")))
}

/// The bytes `hex` spells, or `None` if it holds anything but pairs of hex digits.
fn decode_hex(hex: &str) -> Option<Vec<u8>> {
    let (pairs, []) = hex.as_bytes().as_chunks::<2>() else {
        return None;
    };
    pairs.iter().map(|&[high, low]| Some((hex_digit(high)? << 4) | hex_digit(low)?)).collect()
}

fn hex_digit(byte: u8) -> Option<u8> {
    char::from(byte).to_digit(16).and_then(|digit| u8::try_from(digit).ok())
}

/// An array expression of `database::migrate::Migration`s, the SQL itself left to `include_str!`.
fn render(migrations: &[MigrationFile]) -> String {
    let entries: Vec<String> = migrations
        .iter()
        .map(|migration| {
            let checksum: Vec<String> =
                migration.checksum.iter().map(|byte| format!("{byte:#04x}")).collect();
            format!(
                "    Migration {{ version: {}, description: {:?}, sql: include_str!({:?}), checksum: &[{}] }},",
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
