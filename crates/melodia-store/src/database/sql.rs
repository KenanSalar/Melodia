//! Running SQL on a connection: static text, or text assembled at runtime from literals and
//! vouched-for structure, every value bound.
//!
//! **Caching follows the text's type.** A `&'static str`, or an [`Sql`] still holding the one it
//! was made from, stays prepared on its connection; one with anything pushed onto it is prepared
//! per run, so the placeholder list each chunk size spells can't crowd out the statements worth
//! keeping.

use std::borrow::Cow;
use std::ops::{Deref, DerefMut};

use rusqlite::types::{ToSqlOutput, ValueRef};
use rusqlite::{
    CachedStatement, Connection, OptionalExtension, Params, Statement, ToSql, params_from_iter,
};

use melodia_core::entities::duplicates::DismissalSet;
use melodia_core::error::AppError;

use super::rows::FromRow;

/// Runs a statement that returns no rows, answering how many it changed.
pub(crate) fn execute(
    conn: &Connection,
    sql: &'static str,
    params: impl Params,
) -> Result<u64, AppError> {
    changed_rows(&mut *conn.prepare_cached(sql)?, params)
}

pub(crate) fn fetch_all<T: FromRow>(
    conn: &Connection,
    sql: &'static str,
    params: impl Params,
) -> Result<Vec<T>, AppError> {
    all_rows(&mut *conn.prepare_cached(sql)?, params)
}

pub(crate) fn fetch_one<T: FromRow>(
    conn: &Connection,
    sql: &'static str,
    params: impl Params,
) -> Result<T, AppError> {
    one_row(&mut *conn.prepare_cached(sql)?, params)
}

pub(crate) fn fetch_optional<T: FromRow>(
    conn: &Connection,
    sql: &'static str,
    params: impl Params,
) -> Result<Option<T>, AppError> {
    optional_row(&mut *conn.prepare_cached(sql)?, params)
}

/// Hands each row to `each` as it is read, so a library-sized result is never held whole.
pub(crate) fn fetch_each<T: FromRow>(
    conn: &Connection,
    sql: &'static str,
    params: impl Params,
    mut each: impl FnMut(T),
) -> Result<(), AppError> {
    let mut statement = conn.prepare_cached(sql)?;
    let mut rows = statement.query(params)?;
    while let Some(row) = rows.next()? {
        each(T::from_row(row)?);
    }
    Ok(())
}

/// A statement and the values bound to its `?`s, in order.
pub(crate) struct Sql {
    text: Cow<'static, str>,
    params: Vec<Param>,
}

/// A value bound to one `?`, as one of `SQLite`'s storage classes.
pub(crate) enum Param {
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
}

impl Sql {
    pub(crate) fn new(text: &'static str) -> Self {
        Self { text: Cow::Borrowed(text), params: Vec::new() }
    }

    /// Text built at runtime that the caller vouches holds structure only: placeholder lists,
    /// column projections, order clauses. A value goes through [`Self::bind`] instead.
    pub(crate) fn trusted(text: String) -> Self {
        Self { text: Cow::Owned(text), params: Vec::new() }
    }

    pub(crate) fn push(&mut self, text: &'static str) -> &mut Self {
        self.text.to_mut().push_str(text);
        self
    }

    /// [`Self::trusted`] for a fragment.
    pub(crate) fn push_trusted(&mut self, text: &str) -> &mut Self {
        self.text.to_mut().push_str(text);
        self
    }

    /// Pushes a `?` and binds `value` to it.
    pub(crate) fn push_bind(&mut self, value: impl Into<Param>) -> &mut Self {
        self.push("?").bind(value)
    }

    /// Binds `value` to the next `?` the text already holds.
    pub(crate) fn bind(&mut self, value: impl Into<Param>) -> &mut Self {
        self.params.push(value.into());
        self
    }

    /// Runs a statement that returns no rows, answering how many it changed.
    pub(crate) fn execute(self, conn: &Connection) -> Result<u64, AppError> {
        changed_rows(&mut *self.prepare(conn)?, params_from_iter(&self.params))
    }

    pub(crate) fn fetch_all<T: FromRow>(self, conn: &Connection) -> Result<Vec<T>, AppError> {
        all_rows(&mut *self.prepare(conn)?, params_from_iter(&self.params))
    }

    pub(crate) fn fetch_one<T: FromRow>(self, conn: &Connection) -> Result<T, AppError> {
        one_row(&mut *self.prepare(conn)?, params_from_iter(&self.params))
    }

    pub(crate) fn fetch_optional<T: FromRow>(
        self,
        conn: &Connection,
    ) -> Result<Option<T>, AppError> {
        optional_row(&mut *self.prepare(conn)?, params_from_iter(&self.params))
    }

    fn prepare<'conn>(&self, conn: &'conn Connection) -> rusqlite::Result<Prepared<'conn>> {
        Ok(match &self.text {
            Cow::Borrowed(text) => Prepared::Cached(conn.prepare_cached(text)?),
            Cow::Owned(text) => Prepared::Once(conn.prepare(text)?),
        })
    }

    #[cfg(test)]
    pub(crate) fn text(&self) -> &str {
        &self.text
    }
}

/// A statement from the connection's cache, or one prepared for a single run.
enum Prepared<'conn> {
    Cached(CachedStatement<'conn>),
    Once(Statement<'conn>),
}

impl<'conn> Deref for Prepared<'conn> {
    type Target = Statement<'conn>;

    fn deref(&self) -> &Self::Target {
        match self {
            Self::Cached(statement) => statement,
            Self::Once(statement) => statement,
        }
    }
}

impl DerefMut for Prepared<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        match self {
            Self::Cached(statement) => statement,
            Self::Once(statement) => statement,
        }
    }
}

fn changed_rows(statement: &mut Statement<'_>, params: impl Params) -> Result<u64, AppError> {
    let changed = statement.execute(params)?;
    Ok(u64::try_from(changed).unwrap_or(u64::MAX))
}

fn all_rows<T: FromRow>(
    statement: &mut Statement<'_>,
    params: impl Params,
) -> Result<Vec<T>, AppError> {
    let rows = statement.query_map(params, T::from_row)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

fn one_row<T: FromRow>(statement: &mut Statement<'_>, params: impl Params) -> Result<T, AppError> {
    Ok(statement.query_row(params, T::from_row)?)
}

fn optional_row<T: FromRow>(
    statement: &mut Statement<'_>,
    params: impl Params,
) -> Result<Option<T>, AppError> {
    Ok(statement.query_row(params, T::from_row).optional()?)
}

impl ToSql for Param {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(match self {
            Self::Null => ToSqlOutput::Borrowed(ValueRef::Null),
            Self::Integer(value) => ToSqlOutput::from(*value),
            Self::Real(value) => ToSqlOutput::from(*value),
            Self::Text(value) => ToSqlOutput::from(value.as_str()),
        })
    }
}

macro_rules! param_from {
    ($($source:ty => $class:ident),+ $(,)?) => {$(
        impl From<$source> for Param {
            fn from(value: $source) -> Self {
                Self::$class(value.into())
            }
        }
    )+};
}

param_from!(
    i64 => Integer,
    i32 => Integer,
    bool => Integer,
    DismissalSet => Integer,
    f64 => Real,
    String => Text,
    &str => Text,
);

impl<T: Into<Self>> From<Option<T>> for Param {
    fn from(value: Option<T>) -> Self {
        value.map_or(Self::Null, Into::into)
    }
}
