//! What the two station-list formats are easy to get wrong, and what an import does with them.
//!
//! A miss here is not an error the user sees — the import simply reports fewer stations than the
//! file held, or none at all, and there is nothing on screen naming which line was dropped or
//! which row was passed over. So every shape a real file arrives in is spelled out rather than
//! trusted, and so is what each one does to the table.

use crate::database::{DbPool, queries};
use crate::entities::radio::{self, RadioStation};
use crate::error::AppError;

use super::{
    ImportStationsResult, StationEntry, import_stations_from_file, indexed_key, parse, serialize,
};

/// A kept row, seeded from the same shape the writer tests build.
async fn seed(db: &DbPool, station: &RadioStation) -> Result<RadioStation, AppError> {
    let saved = queries::radio::save_station(db, &station.to_new_station()).await?;
    queries::radio::set_favorite(db, saved.id, true).await?;
    Ok(saved)
}

/// Import through the real door. The file is the half a hand-edit reaches, so reading one is part
/// of what is under test rather than a detail to stub past.
async fn import_text(db: &DbPool, text: &str) -> Result<ImportStationsResult, AppError> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("stations.m3u8");
    std::fs::write(&path, text)?;
    import_stations_from_file(db, &path).await
}

/// One entry, spelled the way the assertions read. Nothing but the name and the URL: every
/// `#MELODIA-*` tag is a Melodia export's alone, and the formats these tests mostly cover have
/// never heard of any of them.
fn entry(name: Option<&str>, url: &str) -> StationEntry {
    StationEntry {
        name: name.map(str::to_owned),
        url: url.to_owned(),
        overrides: radio::StationOverrides::default(),
        snapshot: None,
    }
}

/// The entry an export of `station` reads back as, for a row carrying none of the user's own
/// fields: the name and URL off their own lines, and the row's account of itself out of the
/// station block.
fn exported(station: &RadioStation) -> StationEntry {
    StationEntry {
        name: Some(station.name.clone()),
        url: station.stream_url.clone(),
        overrides: radio::StationOverrides::default(),
        snapshot: Some(station.to_new_station()),
    }
}

/// A stored station with only the two fields the writer touches.
fn station(name: &str, stream_url: &str) -> RadioStation {
    RadioStation {
        id: 1,
        station_uuid: None,
        name: name.to_owned(),
        stream_url: stream_url.to_owned(),
        homepage: None,
        local_homepage: None,
        favicon_url: None,
        local_favicon_url: None,
        local_tags: None,
        local_country: None,
        artwork_path: None,
        tags: String::new(),
        country: String::new(),
        country_code: String::new(),
        language: String::new(),
        codec: String::new(),
        bitrate: 0,
        hls: false,
        is_favorite: true,
        sort_key: name.to_lowercase(),
        date_added: "2026-08-21T00:00:00.000+00:00".to_owned(),
        last_played: None,
        play_count: 0,
    }
}

/// The format Melodia writes, read back by the parser it ships. A round trip is the one assertion
/// that catches a writer and a reader drifting apart, which is the failure neither can see alone.
#[test]
fn what_the_writer_emits_is_what_the_parser_reads_back() {
    let stations = [
        station("Radio One", "https://example.test/one"),
        station("Radio Two", "http://example.test/two.mp3"),
    ];
    let text = serialize(&stations);

    assert!(text.starts_with("#EXTM3U\n"), "an Extended-M3U8 file leads with its header");
    assert_eq!(parse(&text), vec![exported(&stations[0]), exported(&stations[1])]);
}

/// Which half of a station row travels in which half of the file, and that the two never cross.
///
/// The four `local_*` columns go out as their own tags, being the user's and the half a hand-edit
/// reaches for. Everything the directory or the probe filled goes out inside the station block,
/// `station_uuid` above all — that field is what makes a re-imported station the directory's again
/// rather than a hand-typed lookalike, and a list restored without it is a list of the wrong kind
/// of station.
///
/// A *resolved* value written into either half is what this pins against: it would spell one
/// station out of both, and the two are read back by different owners.
#[test]
fn an_export_keeps_the_directory_s_account_apart_from_the_user_s_answers() {
    let mut mine = station("Nidaa FM", "https://example.test/one");
    mine.local_homepage = Some("https://nidaa.fm/".to_owned());
    mine.local_tags = Some("Talk".to_owned());
    mine.local_country = Some("Tunisia".to_owned());
    let mut theirs = station("Listed", "https://example.test/two");
    theirs.station_uuid = Some("uuid-1".to_owned());
    theirs.homepage = Some("https://listed.example/".to_owned());
    theirs.tags = "Jazz".to_owned();

    let text = serialize(&[mine.clone(), theirs.clone()]);
    assert!(
        !text.contains("#MELODIA-GENRE:Jazz")
            && !text.contains("#MELODIA-WEBSITE:https://listed.example/"),
        "a directory value must not travel as one of the user's own tags: {text}"
    );

    assert_eq!(
        parse(&text),
        vec![
            StationEntry {
                name: Some("Nidaa FM".to_owned()),
                url: "https://example.test/one".to_owned(),
                overrides: radio::StationOverrides {
                    website: Some("https://nidaa.fm/".to_owned()),
                    genre: Some("Talk".to_owned()),
                    country: Some("Tunisia".to_owned()),
                    logo_url: None,
                },
                snapshot: Some(mine.to_new_station()),
            },
            exported(&theirs),
        ],
        "every tag has to survive the round trip and belong to its own entry"
    );
}

/// The tag is a comment, so everything that is not this build skips it — including this build
/// before the tag existed, and every other player the export is meant to open in.
#[test]
fn a_reader_that_does_not_know_the_website_tag_still_reads_the_station() {
    let text = "#EXTM3U\n\
                #EXTINF:-1,Nidaa FM\n\
                #MELODIA-WEBSITE:https://nidaa.fm/\n\
                #SOMETHING-ELSE:ignored\n\
                https://example.test/one\n";
    assert_eq!(
        parse(text),
        vec![StationEntry {
            name: Some("Nidaa FM".to_owned()),
            url: "https://example.test/one".to_owned(),
            overrides: radio::StationOverrides {
                website: Some("https://nidaa.fm/".to_owned()),
                ..Default::default()
            },
            snapshot: None,
        }]
    );

    // And an entry with no tag above it must not inherit the previous one's.
    let two = "#EXTM3U\n\
               #EXTINF:-1,One\n\
               #MELODIA-WEBSITE:https://one.example/\n\
               https://example.test/one\n\
               #EXTINF:-1,Two\n\
               https://example.test/two\n";
    let parsed = parse(two);
    assert_eq!(parsed.len(), 2);
    assert_eq!(parsed[1].overrides.website, None, "a website leaked onto the next station");
}

/// A name carrying a newline would otherwise close the `#EXTINF` line early and leave the rest of
/// it parsed as a URL — a station named out of the directory's free-form fields can hold one.
///
/// The station block is safe by construction, JSON escaping the break rather than emitting it, so
/// all the two lines disagree about is which spelling wins. The `#EXTINF` one does: it is what
/// every other player reads and what a hand-edit reaches for.
#[test]
fn a_name_with_a_line_break_cannot_split_its_own_tag() {
    let text = serialize(&[station("Two\nLines", "https://example.test/one")]);
    assert_eq!(
        text.lines().count(),
        4,
        "header, #EXTINF, station block, URL — nothing split: {text}"
    );

    let parsed = parse(&text);
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].name.as_deref(), Some("Two Lines"));
    assert_eq!(
        parsed[0].to_new_station().name,
        "Two Lines",
        "the blob's own spelling is outranked"
    );
}

/// The shape a hand-written or foreign `.m3u` arrives in: no `#EXTINF` at all, unknown comments,
/// blank lines, and a BOM from whatever wrote it.
#[test]
fn a_bare_m3u_needs_no_tags_at_all() {
    let text = "\u{feff}#EXTM3U\n\
                # exported by something else\n\
                \n\
                https://example.test/one\n\
                https://example.test/two\n";
    assert_eq!(
        parse(text),
        vec![
            entry(None, "https://example.test/one"),
            entry(None, "https://example.test/two")
        ]
    );
}

/// `.pls` is what most stations are handed out as, and its titles are **not** required to follow
/// their files — real files put every `File` first and every `Title` after. Pairing by position
/// rather than by index would name both stations wrongly and look right on a one-entry file.
#[test]
fn a_pls_pairs_its_titles_by_index_and_not_by_order() {
    let text = "[playlist]\n\
                NumberOfEntries=2\n\
                File1=https://example.test/one\n\
                File2=https://example.test/two\n\
                Title1=Station One\n\
                Title2=Station Two\n\
                Length1=-1\n\
                Version=2\n";
    assert_eq!(
        parse(text),
        vec![
            entry(Some("Station One"), "https://example.test/one"),
            entry(Some("Station Two"), "https://example.test/two"),
        ]
    );
}

/// The `.pls` keys are case-insensitive in the wild, and `Length`/`Version`/`NumberOfEntries` all
/// parse as `key=value` too — a key filter that missed the case, or one that swallowed every
/// `key=value` line, would drop the stations or import the metadata as one.
#[test]
fn pls_keys_are_case_insensitive_and_the_rest_are_ignored() {
    let text = "[Playlist]\nnumberofentries=1\nfile1=https://example.test/one\ntitle1=Named\n";
    assert_eq!(parse(text), vec![entry(Some("Named"), "https://example.test/one")]);

    assert_eq!(indexed_key("File12", "File"), Some(12));
    assert_eq!(indexed_key("file1", "File"), Some(1));
    assert_eq!(indexed_key("Fil", "File"), None, "a key shorter than the prefix is not one");
    assert_eq!(indexed_key("Filename", "File"), None, "the tail has to be a number");
    assert_eq!(indexed_key("Title1", "File"), None);
}

/// A bare `.m3u` URL routinely carries a session token after a `?`, so its line holds an `=` and
/// reads as a `.pls` key/value pair under the rule above. Falling through to the whole-line
/// reading is what recovers it — and it is the only reason both formats can share one pass.
#[test]
fn an_m3u_url_with_a_query_string_is_not_read_as_a_pls_key() {
    let text = "#EXTM3U\n#EXTINF:-1,Tokened\nhttps://example.test/live?token=abc&x=1\n";
    assert_eq!(
        parse(text),
        vec![entry(
            Some("Tokened"),
            "https://example.test/live?token=abc&x=1"
        )]
    );
}

/// The scheme check is the whole filter, and it is a positive one: a playlist somebody sent you
/// can name a local path, and a station list is not a reason to reach for one.
#[test]
fn only_http_entries_are_taken() {
    let text = "#EXTM3U\n\
                file:///home/someone/track.mp3\n\
                /home/someone/track.mp3\n\
                ftp://example.test/stream\n\
                HTTPS://example.test/one\n";
    assert_eq!(
        parse(text),
        vec![entry(None, "HTTPS://example.test/one")],
        "the scheme is matched case-insensitively but the URL is kept verbatim"
    );
}

/// An `#EXTINF` with no title, and one whose station never arrives. The pending name must not
/// carry across to whatever URL comes next, or a nameless station inherits the one above it.
#[test]
fn a_dangling_extinf_names_nothing_below_it() {
    let text = "#EXTM3U\n\
                #EXTINF:-1,Orphan\n\
                #EXTINF:-1\n\
                https://example.test/one\n";
    assert_eq!(
        parse(text),
        vec![entry(None, "https://example.test/one")],
        "the second `#EXTINF` carries no title and must clear the first one's"
    );
}

/// The failure that shipped: an export re-imported put nothing back.
///
/// Un-starring a directory station that has been played does not delete its row — `is_listed`
/// keeps it for Recently Played's sake — so the by-URL guard the import used to open with found
/// that row and refused the entry. It reported "0 stations added" over a file naming every station
/// the user had, and the only way past it was to clear Recently Played as well.
#[tokio::test]
async fn a_re_import_stars_the_row_a_leftover_play_kept() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let mut listed = station("Listed", "https://example.test/one");
    listed.station_uuid = Some("uuid-1".to_owned());
    let saved = seed(&db, &listed).await?;
    let text = serialize(&queries::radio::get_favorite_stations(&db).await?);

    // Played, then un-starred: the row outlives the star, which is the state the import could not
    // see past.
    queries::radio::mark_played(&db, saved.id).await?;
    queries::radio::set_favorite(&db, saved.id, false).await?;

    assert_eq!(
        import_text(&db, &text).await?,
        ImportStationsResult {
            imported: 1,
            skipped: 0
        },
        "putting the star back is what an import is for"
    );
    let kept = queries::radio::get_favorite_stations(&db).await?;
    assert_eq!(kept.len(), 1, "and it lands on the row that was there rather than beside it");
    assert_eq!(kept[0].id, saved.id);

    assert_eq!(
        import_text(&db, &text).await?,
        ImportStationsResult {
            imported: 0,
            skipped: 1
        },
        "a second pass has nothing to do and reports that rather than adding a duplicate"
    );
    Ok(())
}

/// Which kind each station comes back as, and which half of a row a re-import may write.
///
/// A station kept from Browse has to return as the directory's: the uuid is what gates the pencil,
/// reports the play, and lets the directory refresh the row at all. One typed in by hand has to
/// return as the user's, carrying the fields they filled in. Both are invisible until a card is
/// opened, which is how the import shipped turning every station into a hand-typed lookalike.
#[tokio::test]
async fn an_import_restores_each_station_as_the_kind_it_was_exported_as() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let mut listed = station("Listed", "https://example.test/one");
    listed.station_uuid = Some("uuid-1".to_owned());
    listed.tags = "Jazz".to_owned();
    let from_browse = seed(&db, &listed).await?;

    let hand_typed = seed(&db, &station("Typed", "https://example.test/two")).await?;
    let mine = radio::StationOverrides {
        website: Some("https://typed.example/".to_owned()),
        genre: Some("Talk".to_owned()),
        ..Default::default()
    };
    queries::radio::set_local_fields(&db, hand_typed.id, &mine).await?;

    let text = serialize(&queries::radio::get_favorite_stations(&db).await?);
    queries::radio::delete_station(&db, from_browse.id).await?;
    queries::radio::delete_station(&db, hand_typed.id).await?;

    assert_eq!(
        import_text(&db, &text).await?,
        ImportStationsResult {
            imported: 2,
            skipped: 0
        }
    );

    // Name-ordered, so "Listed" precedes "Typed".
    let rows = queries::radio::get_favorite_stations(&db).await?;
    let [listed_back, typed_back] = rows.as_slice() else {
        return Err(AppError::io_other("expected exactly the two stations back"));
    };

    assert_eq!(
        listed_back.station_uuid.as_deref(),
        Some("uuid-1"),
        "a browsed station has to come back the directory's"
    );
    assert_eq!(listed_back.genre(), Some("Jazz"), "carrying what the directory said about it");
    assert!(!listed_back.can_set_genre(), "so that genre is not the user's to overwrite");

    assert!(typed_back.station_uuid.is_none(), "a hand-typed one has no directory behind it");
    assert_eq!(typed_back.website(), Some("https://typed.example/"));
    assert_eq!(typed_back.genre(), Some("Talk"), "and keeps the fields the user filled in");
    Ok(())
}
