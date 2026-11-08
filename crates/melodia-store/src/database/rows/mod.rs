//! Decoding a result row: a lone column or a tuple by position, an entity by column name, so a
//! projection's column order carries nothing.

mod duplicates;
mod library;
mod radio;
mod track;

use rusqlite::types::FromSql;
use rusqlite::{Row, Statement};

use melodia_core::entities::duplicates::DismissalSet;

/// A value one result row decodes into.
pub trait FromRow: Sized {
    /// Where the value's columns sit in a statement's result.
    type Columns;

    /// Finds [`Self::Columns`] once per statement, before its first step: rusqlite's lookup by
    /// name scans every column name on each read. A result's columns follow its tables', which
    /// only a migration moves.
    fn columns(statement: &Statement<'_>) -> rusqlite::Result<Self::Columns>;

    fn from_row(row: &Row<'_>, columns: &Self::Columns) -> rusqlite::Result<Self>;
}

/// [`FromRow`] for a struct whose every field reads the column it is named after. A `field: Type`
/// entry reads a nested row from the same columns.
macro_rules! by_name {
    ($row_type:ty { $($field:ident $(: $nested:ty)?),+ $(,)? }) => {
        impl $crate::database::rows::FromRow for $row_type {
            type Columns = ($($crate::database::rows::by_name!(@column_type $($nested)?),)+);

            fn columns(statement: &::rusqlite::Statement<'_>) -> ::rusqlite::Result<Self::Columns> {
                Ok(($($crate::database::rows::by_name!(@column statement, $field $(, $nested)?),)+))
            }

            fn from_row(
                row: &::rusqlite::Row<'_>,
                columns: &Self::Columns,
            ) -> ::rusqlite::Result<Self> {
                let ($($field,)+) = columns;
                Ok(Self { $($field: $crate::database::rows::by_name!(@read row, $field $(, $nested)?)),+ })
            }
        }
    };
    (@column_type) => {
        usize
    };
    (@column_type $nested:ty) => {
        <$nested as $crate::database::rows::FromRow>::Columns
    };
    (@column $statement:ident, $field:ident) => {
        $statement.column_index(stringify!($field))?
    };
    (@column $statement:ident, $field:ident, $nested:ty) => {
        <$nested as $crate::database::rows::FromRow>::columns($statement)?
    };
    (@read $row:ident, $field:ident) => {
        $row.get(*$field)?
    };
    (@read $row:ident, $field:ident, $nested:ty) => {
        <$nested as $crate::database::rows::FromRow>::from_row($row, $field)?
    };
}
pub(crate) use by_name;

macro_rules! lone_column {
    ($($column:ty),+) => {$(
        impl FromRow for $column {
            type Columns = ();

            fn columns(_: &Statement<'_>) -> rusqlite::Result<()> {
                Ok(())
            }

            fn from_row(row: &Row<'_>, (): &()) -> rusqlite::Result<Self> {
                row.get(0)
            }
        }
    )+};
}

lone_column!(i64, Option<i64>, i32, Option<i32>, String, Option<String>, bool, DismissalSet);

macro_rules! tuple {
    ($($column:ident: $index:literal),+) => {
        impl<$($column: FromSql),+> FromRow for ($($column,)+) {
            type Columns = ();

            fn columns(_: &Statement<'_>) -> rusqlite::Result<()> {
                Ok(())
            }

            fn from_row(row: &Row<'_>, (): &()) -> rusqlite::Result<Self> {
                Ok(($(row.get::<_, $column>($index)?,)+))
            }
        }
    };
}

tuple!(A: 0, B: 1);
tuple!(A: 0, B: 1, C: 2);
tuple!(A: 0, B: 1, C: 2, D: 3);
tuple!(A: 0, B: 1, C: 2, D: 3, E: 4);
tuple!(A: 0, B: 1, C: 2, D: 3, E: 4, F: 5);
tuple!(A: 0, B: 1, C: 2, D: 3, E: 4, F: 5, G: 6);
