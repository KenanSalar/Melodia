//! The set a source reads off the cells, which has to be the set the engine wrote: each filter's
//! kind travels as a code, and a set read twice would crossfade into itself.

use super::CorrectionWriter;
use melodia_core::entities::correction::{
    CorrectionFilter, CorrectionFilters, FilterKind, FilterSetError,
};

const SET: [CorrectionFilter; 3] = [
    CorrectionFilter { kind: FilterKind::LowShelf, freq_hz: 105.0, gain_db: 6.0, q: 0.7 },
    CorrectionFilter { kind: FilterKind::Peaking, freq_hz: 2_400.0, gain_db: -3.5, q: 2.1 },
    CorrectionFilter { kind: FilterKind::HighShelf, freq_hz: 10_000.0, gain_db: 2.5, q: 0.7 },
];

#[test]
fn a_published_set_reads_back_whole() -> Result<(), FilterSetError> {
    let mut writer = CorrectionWriter::default();
    writer.publish(&CorrectionFilters::try_from(SET.to_vec())?);

    let read = writer.shared().read_if_changed(None);

    assert_eq!(read.map(|(_, set)| set.filters().to_vec()), Some(SET.to_vec()));
    Ok(())
}

/// A source polls every frame, and a set read again would be swapped in again.
#[test]
fn a_set_already_read_is_not_read_again() -> Result<(), FilterSetError> {
    let mut writer = CorrectionWriter::default();
    writer.publish(&CorrectionFilters::try_from(SET.to_vec())?);
    let shared = writer.shared();
    let seen = shared.read_if_changed(None).map(|(sequence, _)| sequence);

    let again = shared.read_if_changed(seen);

    assert!(again.is_none(), "an unchanged set read a second time");
    Ok(())
}

/// Clearing is how the switch, a device without a correction and Clear reach every source.
#[test]
fn a_clear_reads_back_as_an_empty_set() -> Result<(), FilterSetError> {
    let mut writer = CorrectionWriter::default();
    writer.publish(&CorrectionFilters::try_from(SET.to_vec())?);
    let shared = writer.shared();
    let seen = shared.read_if_changed(None).map(|(sequence, _)| sequence);
    writer.clear();

    let read = shared.read_if_changed(seen);

    assert_eq!(read.map(|(_, set)| set.filters().len()), Some(0));
    Ok(())
}
