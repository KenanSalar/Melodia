use crate::services::updater::probe::dir_is_writable;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn detects_writable_tempdir() -> TestResult {
    let tmp = tempfile::tempdir()?;
    assert!(
        dir_is_writable(tmp.path()),
        "fresh tempdir should report writable"
    );
    Ok(())
}

#[test]
fn returns_false_for_missing_dir() {
    let bogus = std::path::Path::new("/no/such/dir/melodia-probe-target");
    assert!(
        !dir_is_writable(bogus),
        "non-existent path must not report writable"
    );
}

#[test]
#[cfg(unix)]
fn detects_readonly_dir() -> TestResult {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir()?;
    let ro = tmp.path().join("ro");
    std::fs::create_dir(&ro)?;
    std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o555))?;
    // Skip the assertion where the mode isn't a barrier — running as root (CI
    // containers occasionally do), or a filesystem that ignores permissions. Ask
    // the question directly rather than through `geteuid`: uid 0 is only one of
    // the reasons a write can land, and the probe would (correctly) succeed for
    // any of them.
    //
    // The sentinel is the same primitive `dir_is_writable` uses, so this reads as
    // a precondition on the environment rather than as an oracle for the
    // function — it establishes that 0555 means something here, and the assertion
    // below is what tests the probe. Removed either way so the skip path leaves
    // the directory as it found it.
    let sentinel = ro.join("melodia-mode-check");
    let mode_is_enforced = std::fs::File::create(&sentinel).is_err();
    let _ = std::fs::remove_file(&sentinel);
    if !mode_is_enforced {
        return Ok(());
    }
    assert!(
        !dir_is_writable(&ro),
        "0o555 dir must report not writable for non-root callers"
    );
    Ok(())
}

#[test]
fn unique_probe_names_under_repeated_calls() -> TestResult {
    // Regression for the previous PID-only probe name: two sequential
    // calls in the same process must both succeed (the file must be
    // unlinked between calls, AND the seq counter must give a fresh
    // name even if cleanup raced).
    let tmp = tempfile::tempdir()?;
    for _ in 0..16 {
        assert!(dir_is_writable(tmp.path()));
    }
    // Confirm no leaked probe files.
    let leaked: Vec<_> = std::fs::read_dir(tmp.path())?
        .filter_map(Result::ok)
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with(".melodia-write-probe-")
        })
        .collect();
    assert!(
        leaked.is_empty(),
        "leaked probe files: {leaked:?}"
    );
    Ok(())
}
