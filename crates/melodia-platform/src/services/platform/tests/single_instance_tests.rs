use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use interprocess::local_socket::{Stream, prelude::*};
use tempfile::tempdir;

use super::{
    Claim, LENGTH_PREFIX_LEN, MAX_PAYLOAD_LEN, RESPAWN_ENV, allow_missing_timeout, claim,
    decode_paths, encode_frame, name_is_taken_on, serve, socket_name,
};
use melodia_testkit::{reading_env, with_env_set};

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

/// The socket test below only ever exercises the Unix answer, and the platform
/// with the other one has no runner — so both are asked of the pure half, which
/// takes the platform as an argument precisely so a Linux gate can reach them.
#[test]
fn a_taken_name_is_recognised_in_both_spellings() {
    let taken_on = |kind, windows| name_is_taken_on(&std::io::Error::from(kind), windows);

    assert!(taken_on(std::io::ErrorKind::AddrInUse, false), "`bind` says `EADDRINUSE`");
    assert!(
        taken_on(std::io::ErrorKind::PermissionDenied, true),
        "a second named-pipe instance under `FILE_FLAG_FIRST_PIPE_INSTANCE` says \
         `ERROR_ACCESS_DENIED`, which is the whole reason this predicate has two arms"
    );
    assert!(
        !taken_on(std::io::ErrorKind::PermissionDenied, false),
        "off Windows a permission failure is a real one, and `bind` never reports it for a \
         name that is merely held"
    );
    assert!(!taken_on(std::io::ErrorKind::NotFound, true));
}

/// The socket test below only ever exercises the transport the runner has, and the one that
/// answers this question differently has no runner. Getting it wrong is not a dropped
/// timeout: `forward` propagates, `claim` reads that as `Unenforced`, and the second launch
/// opens a second window and a second writer onto one database.
#[test]
fn a_transport_without_timeouts_still_forwards() {
    let refused = |kind| allow_missing_timeout(Err(std::io::Error::from(kind)));

    assert!(
        refused(std::io::ErrorKind::Unsupported).is_ok(),
        "a Windows named pipe has no settable timeout, and that may not cost us the claim"
    );
    assert!(
        refused(std::io::ErrorKind::BrokenPipe).is_err(),
        "a real failure on the way to setting one is still a failure"
    );
    assert!(allow_missing_timeout(Ok(())).is_ok());
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

/// What `spawn_reader` exists for. A peer that connects and then says nothing is a read that
/// only one transport will ever cut short — a Windows named pipe takes no deadline at all — so
/// read on the accept thread it costs every launch behind it, not just itself.
///
/// The silent peer connects first so it is mid-read when the real launch arrives, and the
/// assertion window is inside `IO_TIMEOUT`: this fails on a Linux runner too, where the read
/// does eventually give up, because "eventually" is the whole complaint.
#[test]
fn a_silent_peer_does_not_cost_the_launch_behind_it() {
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

        let Ok(name) = socket_name(data_dir.path()) else {
            unreachable!("the name just claimed must still spell")
        };
        let Ok(_silent) = Stream::connect(name) else {
            unreachable!("the primary is listening")
        };

        let opened = PathBuf::from("/music/Album/02 - Track.flac");
        assert!(
            matches!(claim(data_dir.path(), std::slice::from_ref(&opened)), Claim::Secondary),
            "a claim against a held name must forward rather than bind"
        );

        let delivered = rx.recv_timeout(Duration::from_secs(1)).ok();
        assert_eq!(
            delivered,
            Some(vec![opened.to_string_lossy().into_owned()]),
            "a peer that said nothing parked the accept loop and the launch behind it was lost"
        );
    });
}

/// The detached restart arm, from the child's side. `shutdown::spawn_detached` leaves parent and
/// child alive together and marks the child with [`RESPAWN_ENV`]; a marked child that forwarded
/// instead of waiting would hand its launch to a parent already on its way out and exit behind
/// it, leaving the user with no window at all.
///
/// The parent's exit is a thread that drops the listener, which is the whole of what frees the
/// name. Well inside `RESPAWN_WAIT`, so a slow runner still lands on the same answer.
#[test]
fn a_restarting_child_waits_for_the_name_rather_than_forwarding() {
    let Ok(data_dir) = tempdir() else {
        unreachable!("no writable temp directory")
    };

    with_env_set(&[RESPAWN_ENV], &[(RESPAWN_ENV, "1")], || {
        let Claim::Primary(held) = claim(data_dir.path(), &[]) else {
            unreachable!("an unused data directory must be claimable")
        };
        let parent_exits = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            drop(held);
        });

        assert!(
            matches!(claim(data_dir.path(), &[]), Claim::Primary(_)),
            "a restart must come back up on the name the parent released, not forward into it",
        );
        assert!(parent_exits.join().is_ok());
    });
}
