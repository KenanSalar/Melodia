//! Which root a boot lands on: the environment override, and the two the build's shape picks
//! between when there isn't one.
//!
//! Everything under the root is [`Paths::rooted_at`], which `tests/headless.rs` covers by handing
//! it a tempdir. What is left is the choice above it, and it decides which database opens.

use super::*;
use crate::test_support::with_env_var;

/// The override and the path it is spelled as. Joined rather than written out, so the separator
/// is the platform's; a working directory that can't be rendered back is worth reporting rather
/// than quietly passing.
fn override_root(name: &str) -> AppResult<(PathBuf, String)> {
    let path = std::env::current_dir()?.join(name);
    let spelled = path
        .to_str()
        .ok_or_else(|| AppError::Settings("test needs a UTF-8 working directory".into()))?
        .to_owned();
    Ok((path, spelled))
}

/// The override exists to point a dev build at real data, so it has to beat the shape rather than
/// merely stand in when there is none.
#[test]
fn the_override_wins_over_either_build_shape() -> AppResult<()> {
    let (root, spelled) = override_root("override-root")?;

    for is_dev in [true, false] {
        let resolved = with_env_var(DATA_DIR_ENV, Some(&spelled), || Paths::data_root_for(is_dev))?;

        assert_eq!(resolved, root, "the override should decide it, is_dev = {is_dev}");
    }
    Ok(())
}

/// `single_instance` hashes the resolved root for its socket name, so a value left relative keys
/// two working directories onto one socket and two spellings of one directory onto two.
#[test]
fn a_relative_override_comes_back_absolute() -> AppResult<()> {
    let resolved =
        with_env_var(DATA_DIR_ENV, Some("relative-root"), || Paths::data_root_for(true))?;

    assert_eq!(resolved, std::env::current_dir()?.join("relative-root"));
    Ok(())
}

/// The spelling `absolute` keeps rather than closes. Asserted on the bytes because that is what
/// `single_instance` hashes; `Path` compares by component, so both forms pass `assert_eq!` already.
#[test]
fn a_trailing_separator_spells_the_same_root() -> AppResult<()> {
    let (root, spelled) = override_root("trailing-root")?;
    let slashed = format!("{spelled}{}", std::path::MAIN_SEPARATOR_STR);

    let resolved = with_env_var(DATA_DIR_ENV, Some(&slashed), || Paths::data_root_for(true))?;

    assert_eq!(resolved.as_os_str(), root.as_os_str());
    Ok(())
}

/// An exported-but-empty variable is the shape a shell leaves behind, and it means the same as
/// unset everywhere else in the tree.
#[test]
fn an_empty_override_falls_through_to_the_build_shape() -> AppResult<()> {
    let resolved = with_env_var(DATA_DIR_ENV, Some(""), || Paths::data_root_for(true))?;

    assert_eq!(resolved.file_name().and_then(|name| name.to_str()), Some(DEV_DATA_DIR_NAME));
    Ok(())
}

/// Siblings, not one inside the other: a sweep that retires the dev tree must not be able to
/// reach the installed one, and the single-instance claim only separates them if the two roots
/// differ.
#[test]
fn each_build_shape_lands_on_its_own_root() -> AppResult<()> {
    let (installed, dev) = with_env_var(DATA_DIR_ENV, None, || {
        Ok::<_, AppError>((Paths::data_root_for(false)?, Paths::data_root_for(true)?))
    })?;

    assert_eq!(installed.file_name().and_then(|name| name.to_str()), Some(DATA_DIR_NAME));
    assert_eq!(dev.file_name().and_then(|name| name.to_str()), Some(DEV_DATA_DIR_NAME));
    assert_eq!(installed.parent(), dev.parent());
    Ok(())
}
