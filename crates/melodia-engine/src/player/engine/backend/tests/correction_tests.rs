//! Which correction plays: the heard device's while the switch is on, every device keeping its own
//! through a switch flipped off, a device switched away from, and back.

use super::super::headphones::HeadphoneControls;
use melodia_core::entities::correction::{
    Correction, CorrectionFilter, CorrectionFilters, CorrectionOrigin, FilterKind, FilterSetError,
};
use melodia_core::entities::device_map::DeviceMap;

const CARD: &str = "hw:CARD=UMC22,DEV=0";
const OTHER_CARD: &str = "hw:CARD=PCH,DEV=0";

fn correction() -> Result<Correction, FilterSetError> {
    let filter =
        CorrectionFilter { kind: FilterKind::Peaking, freq_hz: 105.0, gain_db: 6.5, q: 0.7 };
    Ok(Correction {
        origin: CorrectionOrigin::Paste,
        device_name: None,
        filters: CorrectionFilters::try_from(vec![filter])?,
    })
}

/// A map holding one correction, for the card when `card` is set and for System default when not.
fn one_for(card: Option<&str>) -> Result<DeviceMap<Correction>, FilterSetError> {
    let mut corrections = DeviceMap::default();
    corrections.insert(card.map(str::to_owned), correction()?);
    Ok(corrections)
}

fn control_for(
    enabled: bool,
    corrections: DeviceMap<Correction>,
    heard: Option<&str>,
) -> HeadphoneControls {
    let mut control = HeadphoneControls::default();
    control.set_corrections(corrections);
    control.set_correction_enabled(enabled);
    control.aim(heard);
    control
}

#[test]
fn a_correction_plays_only_for_the_heard_device_while_switched_on() -> Result<(), FilterSetError> {
    let cases = [
        ("switched off", false, one_for(Some(CARD))?, Some(CARD), false),
        ("on, the card's own", true, one_for(Some(CARD))?, Some(CARD), true),
        ("on, another card's", true, one_for(Some(OTHER_CARD))?, Some(CARD), false),
        ("on, System default's", true, one_for(None)?, None, true),
        ("on, System default's heard on a card", true, one_for(None)?, Some(CARD), false),
        ("on, a card's heard on System default", true, one_for(Some(CARD))?, None, false),
        ("on, none set", true, DeviceMap::default(), Some(CARD), false),
    ];
    for (what, enabled, corrections, heard, plays) in cases {
        assert_eq!(control_for(enabled, corrections, heard).correction_engaged(), plays, "{what}");
    }
    Ok(())
}

/// The switch is a before and after, so turning it back on finds the correction where it was.
#[test]
fn switching_off_and_on_again_brings_the_correction_back() -> Result<(), FilterSetError> {
    let mut control = control_for(true, one_for(Some(CARD))?, Some(CARD));
    control.set_correction_enabled(false);

    control.set_correction_enabled(true);

    assert!(control.correction_engaged());
    Ok(())
}

/// Switching device in Melodia switches the correction with it, both ways.
#[test]
fn aiming_back_at_a_device_brings_its_correction_back() -> Result<(), FilterSetError> {
    let mut control = control_for(true, one_for(Some(CARD))?, Some(CARD));
    control.aim(Some(OTHER_CARD));
    let away = control.correction_engaged();

    control.aim(Some(CARD));

    let back = control.correction_engaged();
    assert_eq!((away, back), (false, true), "(on the other card, back on this one)");
    Ok(())
}
