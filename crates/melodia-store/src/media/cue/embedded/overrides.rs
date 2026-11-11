//! `CUE_TRACKnn_<FIELD>` comments, which taggers write beside an embedded sheet to give one track a
//! field the sheet doesn't state, or states otherwise. Only what a sheet track holds is read: a
//! field of the whole disc belongs to the sheet's own lines.

use super::super::parse::Sheet;
use melodia_core::entities::cue::CueTrack;

const PREFIX: &str = "CUE_TRACK";

/// A field an override sets on its track.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TrackField {
    Title,
    Artist,
    Composer,
    Genre,
    Date,
    Isrc,
}

impl TrackField {
    const ALL: [Self; 6] =
        [Self::Title, Self::Artist, Self::Composer, Self::Genre, Self::Date, Self::Isrc];

    /// The Vorbis field name an override spells it with.
    fn name(self) -> &'static str {
        match self {
            Self::Title => "TITLE",
            Self::Artist => "ARTIST",
            Self::Composer => "COMPOSER",
            Self::Genre => "GENRE",
            Self::Date => "DATE",
            Self::Isrc => "ISRC",
        }
    }
}

/// The track and field the comment `key` overrides, `CUE_TRACK01_TITLE` and `cue_track1_title`
/// alike.
pub(crate) fn override_key(key: &str) -> Option<(CueTrack, TrackField)> {
    if !key.get(..PREFIX.len())?.eq_ignore_ascii_case(PREFIX) {
        return None;
    }
    let (number, name) = key.get(PREFIX.len()..)?.split_once('_')?;
    let track = CueTrack::try_from(number.parse::<i64>().ok()?).ok()?;
    let field =
        TrackField::ALL.into_iter().find(|field| field.name().eq_ignore_ascii_case(name))?;
    Some((track, field))
}

/// Lays each override in `overrides` over the track of `sheet` it numbers. A blank one, or one
/// numbering a track the sheet doesn't state, changes nothing.
pub(super) fn apply(sheet: &mut Sheet, overrides: &[(String, String)]) {
    for (key, value) in overrides {
        let value = value.trim();
        let Some((number, field)) = override_key(key).filter(|_| !value.is_empty()) else {
            continue;
        };
        let Some(track) = sheet.tracks.iter_mut().find(|track| track.number == number) else {
            continue;
        };
        let value = value.to_owned();
        match field {
            TrackField::Title => track.fields.title = Some(value),
            TrackField::Artist => track.fields.performer = Some(value),
            TrackField::Composer => track.fields.songwriter = Some(value),
            TrackField::Genre => track.fields.set_rem("GENRE", value),
            TrackField::Date => track.fields.set_rem("DATE", value),
            TrackField::Isrc => track.isrc = Some(value),
        }
    }
}
