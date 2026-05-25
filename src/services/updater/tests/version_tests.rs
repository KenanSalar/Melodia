use crate::services::updater::version::is_upgrade;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn newer_patch_is_upgrade() -> TestResult {
    assert!(is_upgrade("0.1.0", "0.1.1")?);
    Ok(())
}

#[test]
fn newer_minor_is_upgrade() -> TestResult {
    assert!(is_upgrade("0.1.0", "0.2.0")?);
    Ok(())
}

#[test]
fn newer_major_is_upgrade() -> TestResult {
    assert!(is_upgrade("0.9.0", "1.0.0")?);
    Ok(())
}

#[test]
fn equal_is_not_upgrade() -> TestResult {
    assert!(!is_upgrade("0.2.0", "0.2.0")?);
    Ok(())
}

#[test]
fn older_is_not_upgrade() -> TestResult {
    // Downgrade refusal: a compromised manifest reporting an older
    // version must NOT trigger an install. String equality wouldn't
    // catch this.
    assert!(!is_upgrade("0.2.0", "0.1.9")?);
    Ok(())
}

#[test]
fn older_major_is_not_upgrade() -> TestResult {
    assert!(!is_upgrade("1.0.0", "0.9.9")?);
    Ok(())
}

#[test]
fn pre_release_orders_below_release() -> TestResult {
    // Per semver: 0.2.0-rc1 < 0.2.0
    assert!(is_upgrade("0.2.0-rc1", "0.2.0")?);
    assert!(!is_upgrade("0.2.0", "0.2.0-rc1")?);
    Ok(())
}

#[test]
fn invalid_current_is_error() {
    assert!(is_upgrade("not-a-version", "0.2.0").is_err());
}

#[test]
fn invalid_candidate_is_error() {
    // leading `v` is not semver
    assert!(is_upgrade("0.2.0", "v0.3.0").is_err());
}
