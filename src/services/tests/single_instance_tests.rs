use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use tempfile::tempdir;

use super::{
    Claim, LENGTH_PREFIX_LEN, MAX_PAYLOAD_LEN, claim, decode_paths, encode_frame, serve,
    socket_name,
};
use crate::test_support::reading_env;

/// Split a frame the way `read_payload` does.
fn split_frame(frame: &[u8]) -> (u32, &[u8]) {
    let (prefix, body) = frame.split_at(LENGTH_PREFIX_LEN);
    let declared = u32::from_le_bytes(prefix.try_into().unwrap_or_default());
    (declared, body)
}

#[test]
fn a_path_list_survives_the_round_trip() {
    let files = vec![
        PathBuf::from("/music/Album/01 - Track.flac"),
        PathBuf::from("/music/Another Album/02 - Track, with a comma.mp3"),
    ];

    let frame = encode_frame(&files);
    let (declared, body) = split_frame(&frame);

    assert_eq!(declared as usize, body.len(), "the prefix must describe the body exactly");
    assert_eq!(
        decode_paths(body),
        vec![
            "/music/Album/01 - Track.flac".to_owned(),
            "/music/Another Album/02 - Track, with a comma.mp3".to_owned(),
        ]
    );
}

#[test]
fn a_single_path_carries_no_separator() {
    let frame = encode_frame(&[PathBuf::from("/music/one.ogg")]);
    let (_, body) = split_frame(&frame);

    assert!(!body.contains(&0));
    assert_eq!(decode_paths(body), vec!["/music/one.ogg".to_owned()]);
}

/// A newline is legal in a Unix filename, which is why the separator is NUL.
#[test]
fn a_newline_in_a_filename_is_not_a_separator() {
    let frame = encode_frame(&[PathBuf::from("/music/two\nline.flac")]);
    let (_, body) = split_frame(&frame);

    assert_eq!(decode_paths(body), vec!["/music/two\nline.flac".to_owned()]);
}

/// A bare second launch still forwards — the primary has a window to raise.
#[test]
fn an_empty_launch_is_a_frame_with_no_body() {
    let frame = encode_frame(&[]);

    assert_eq!(frame.len(), LENGTH_PREFIX_LEN);
    assert_eq!(split_frame(&frame).0, 0);
    assert!(decode_paths(&[]).is_empty());
}

#[test]
fn empty_segments_are_dropped() {
    assert_eq!(
        decode_paths(b"\0/music/a.mp3\0\0/music/b.mp3\0"),
        vec!["/music/a.mp3".to_owned(), "/music/b.mp3".to_owned(),]
    );
}

#[test]
fn the_payload_cap_leaves_room_for_a_realistic_selection() {
    let files: Vec<PathBuf> = (0..500)
        .map(|i| PathBuf::from(format!("/music/Some Album/{i:02} - Track Title.flac")))
        .collect();

    assert!(u64::from(split_frame(&encode_frame(&files)).0) < MAX_PAYLOAD_LEN);
}

/// Keyed on the data directory, that being what two Melodias would corrupt.
#[test]
fn two_data_directories_get_two_names() {
    let one = socket_name(Path::new("/home/alice/.local/share/Melodia")).ok();
    let two = socket_name(Path::new("/home/bo/.local/share/Melodia")).ok();

    assert!(one.is_some());
    assert_ne!(format!("{one:?}"), format!("{two:?}"));
}

#[test]
fn the_same_data_directory_gets_the_same_name() {
    let path = Path::new("/home/alice/.local/share/Melodia");

    assert_eq!(format!("{:?}", socket_name(path).ok()), format!("{:?}", socket_name(path).ok()));
}

/// Why a real socket earns its setup: the two halves can each be right and
/// still deadlock. An earlier draft had the sender wait on the receiver's close
/// while the receiver read to EOF — both blocked to their timeouts and the paths
/// were dropped, which every codec test above passed straight through.
///
/// A tempdir keys the name, so this can't collide with a live Melodia.
/// `reading_env` because `claim` reads `MELODIA_RESPAWN` and a sibling test
/// mutating the environment races it either way.
#[test]
fn a_second_launch_hands_its_paths_to_the_first_and_stands_down() {
    let Ok(data_dir) = tempdir() else {
        unreachable!("no writable temp directory")
    };
    let (tx, rx) = mpsc::channel();

    reading_env(|| {
        let Claim::Primary(listener) = claim(data_dir.path(), &[]) else {
            unreachable!("an unused data directory must be claimable")
        };
        serve(listener, move |paths| {
            let _ = tx.send(paths);
        });

        let opened = PathBuf::from("/music/Album/01 - Track.flac");
        assert!(
            matches!(claim(data_dir.path(), std::slice::from_ref(&opened)), Claim::Secondary),
            "a claim against a held name must forward rather than bind"
        );

        // Under the 2 s socket timeouts, so a deadlock fails here rather than
        // passing slowly.
        let delivered = rx.recv_timeout(Duration::from_secs(1)).ok();
        assert_eq!(
            delivered,
            Some(vec![opened.to_string_lossy().into_owned()]),
            "the primary never saw the forwarded launch — `None` is the timeout, i.e. a deadlock"
        );
    });
}
