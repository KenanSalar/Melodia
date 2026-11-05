//! SQL assembled at runtime: text from literals and vouched-for structure, every value bound.
//!
//! **Caching follows the text.** A statement still holding the `&'static str` it was made from
//! stays prepared on its connection; one with anything pushed onto it is prepared per run, so the
//! placeholder list each chunk size spells can't crowd out the statements worth keeping.

use std::borrow::Cow;

use sqlx::query::{Query, QueryAs};
use sqlx::sqlite::{Sqlite, SqliteArguments, SqliteRow};
use sqlx::{Arguments, AssertSqlSafe, FromRow};

use melodia_core::entities::duplicates::DismissalSet;
use melodia_core::error::AppError;

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
    pub(crate) async fn execute<'e, E>(self, executor: E) -> Result<u64, AppError>
    where
        E: sqlx::Executor<'e, Database = Sqlite>,
    {
        Ok(self.query()?.execute(executor).await?.rows_affected())
    }

    pub(crate) async fn fetch_all<'e, T, E>(self, executor: E) -> Result<Vec<T>, AppError>
    where
        T: for<'r> FromRow<'r, SqliteRow> + Send + Unpin,
        E: sqlx::Executor<'e, Database = Sqlite>,
    {
        Ok(self.query_as()?.fetch_all(executor).await?)
    }

    pub(crate) async fn fetch_one<'e, T, E>(self, executor: E) -> Result<T, AppError>
    where
        T: for<'r> FromRow<'r, SqliteRow> + Send + Unpin,
        E: sqlx::Executor<'e, Database = Sqlite>,
    {
        Ok(self.query_as()?.fetch_one(executor).await?)
    }

    pub(crate) async fn fetch_optional<'e, T, E>(self, executor: E) -> Result<Option<T>, AppError>
    where
        T: for<'r> FromRow<'r, SqliteRow> + Send + Unpin,
        E: sqlx::Executor<'e, Database = Sqlite>,
    {
        Ok(self.query_as()?.fetch_optional(executor).await?)
    }

    fn query(self) -> Result<Query<'static, Sqlite, SqliteArguments>, sqlx::Error> {
        let cached = self.is_static();
        let arguments = arguments(self.params)?;
        Ok(sqlx::query_with(AssertSqlSafe(self.text), arguments).persistent(cached))
    }

    fn query_as<T>(self) -> Result<QueryAs<'static, Sqlite, T, SqliteArguments>, sqlx::Error>
    where
        T: for<'r> FromRow<'r, SqliteRow>,
    {
        let cached = self.is_static();
        let arguments = arguments(self.params)?;
        Ok(sqlx::query_as_with(AssertSqlSafe(self.text), arguments).persistent(cached))
    }

    fn is_static(&self) -> bool {
        matches!(self.text, Cow::Borrowed(_))
    }

    #[cfg(test)]
    pub(crate) fn text(&self) -> &str {
        &self.text
    }
}

fn arguments(params: Vec<Param>) -> Result<SqliteArguments, sqlx::Error> {
    let mut arguments = SqliteArguments::default();
    for param in params {
        let added = match param {
            Param::Null => Arguments::add(&mut arguments, None::<i64>),
            Param::Integer(value) => Arguments::add(&mut arguments, value),
            Param::Real(value) => Arguments::add(&mut arguments, value),
            Param::Text(value) => Arguments::add(&mut arguments, value),
        };
        added.map_err(sqlx::Error::Encode)?;
    }
    Ok(arguments)
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
