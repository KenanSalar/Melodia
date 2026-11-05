//! Decoding a result row: a lone column or a tuple by position, an entity by column name, so a
//! projection's column order carries nothing.

mod duplicates;
mod library;
mod radio;
mod track;

use rusqlite::Row;
use rusqlite::types::FromSql;

use melodia_core::entities::duplicates::DismissalSet;

/// A value one result row decodes into.
pub trait FromRow: Sized {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self>;
}

/// [`FromRow`] for a struct whose every field reads the column it is named after. A `field: Type`
/// entry reads a nested row from the same columns.
macro_rules! by_name {
    ($row_type:ty { $($field:ident $(: $nested:ty)?),+ $(,)? }) => {
        impl $crate::database::rows::FromRow for $row_type {
            fn from_row(row: &::rusqlite::Row<'_>) -> ::rusqlite::Result<Self> {
                Ok(Self { $($field: $crate::database::rows::by_name!(@read row, $field $(, $nested)?)),+ })
            }
        }
    };
    (@read $row:ident, $field:ident) => {
        $row.get(stringify!($field))?
    };
    (@read $row:ident, $field:ident, $nested:ty) => {
        <$nested as $crate::database::rows::FromRow>::from_row($row)?
    };
}
pub(crate) use by_name;

macro_rules! lone_column {
    ($($column:ty),+) => {$(
        impl FromRow for $column {
            fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
                row.get(0)
            }
        }
    )+};
}

lone_column!(i64, Option<i64>, Option<i32>, String, Option<String>, bool, DismissalSet);

macro_rules! tuple {
    ($($column:ident: $index:literal),+) => {
        impl<$($column: FromSql),+> FromRow for ($($column,)+) {
            fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
                Ok(($(row.get::<_, $column>($index)?,)+))
            }
        }
    };
}

tuple!(A: 0, B: 1);
tuple!(A: 0, B: 1, C: 2);
tuple!(A: 0, B: 1, C: 2, D: 3);
tuple!(A: 0, B: 1, C: 2, D: 3, E: 4);
