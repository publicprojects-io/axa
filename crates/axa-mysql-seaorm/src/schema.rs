//! Migration helpers over `sea-orm-migration`.
//!
//! [`create_table`] adds `created_at` and `updated_at` with a
//! `CURRENT_TIMESTAMP` default, and table names are pluralised, so migrations written against
//! these helpers produce the schema they always have.

use heck::ToSnakeCase;
use sea_orm::DbErr;
use sea_orm_migration::{
    prelude::*,
    schema::{
        big_integer, big_integer_null, big_pk_auto, boolean, boolean_null, date, date_null,
        date_time, date_time_null, double, double_null, integer, integer_null, pk_uuid, string,
        string_null, string_uniq, text, text_null, uuid, uuid_null, uuid_uniq,
    },
    sea_query::{ForeignKeyAction, Table},
};

/// A column type.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum ColType {
    /// 64-bit auto-increment primary key.
    PkAuto,
    /// UUID primary key.
    PkUuid,
    /// Required string.
    String,
    /// Optional string.
    StringNull,
    /// Required unique string.
    StringUniq,
    /// Required text.
    Text,
    /// Optional text.
    TextNull,
    /// Required 32-bit integer.
    Integer,
    /// Optional 32-bit integer.
    IntegerNull,
    /// Required 64-bit integer.
    BigInteger,
    /// Optional 64-bit integer.
    BigIntegerNull,
    /// Required double.
    Double,
    /// Optional double.
    DoubleNull,
    /// Required boolean.
    Boolean,
    /// Optional boolean.
    BooleanNull,
    /// Required boolean with a default.
    BooleanWithDefault(bool),
    /// Required date.
    Date,
    /// Optional date.
    DateNull,
    /// Required timestamp, a MySQL `DATETIME` in UTC.
    TimestampWithTimeZone,
    /// Optional timestamp, a MySQL `DATETIME` in UTC.
    TimestampWithTimeZoneNull,
    /// Required UUID.
    Uuid,
    /// Optional UUID.
    UuidNull,
    /// Required unique UUID.
    UuidUniq,
    /// Required JSONB.
    JsonBinary,
    /// Optional JSONB.
    JsonBinaryNull,
}

impl ColType {
    fn to_def(&self, name: Alias) -> ColumnDef {
        match self {
            Self::PkAuto => big_pk_auto(name),
            Self::PkUuid => pk_uuid(name),
            Self::String => string(name),
            Self::StringNull => string_null(name),
            Self::StringUniq => string_uniq(name),
            Self::Text => text(name),
            Self::TextNull => text_null(name),
            Self::Integer => integer(name),
            Self::IntegerNull => integer_null(name),
            Self::BigInteger => big_integer(name),
            Self::BigIntegerNull => big_integer_null(name),
            Self::Double => double(name),
            Self::DoubleNull => double_null(name),
            Self::Boolean => boolean(name),
            Self::BooleanNull => boolean_null(name),
            Self::BooleanWithDefault(value) => boolean(name).default(*value).take(),
            Self::Date => date(name),
            Self::DateNull => date_null(name),
            Self::TimestampWithTimeZone => date_time(name),
            Self::TimestampWithTimeZoneNull => date_time_null(name),
            Self::Uuid => uuid(name),
            Self::UuidNull => uuid_null(name),
            Self::UuidUniq => uuid_uniq(name),
            Self::JsonBinary => ColumnDef::new(name).json_binary().not_null().take(),
            Self::JsonBinaryNull => ColumnDef::new(name).json_binary().null().take(),
        }
    }
}

fn normalize_table(table: &str) -> std::string::String {
    cruet::to_plural(table).to_snake_case()
}

fn reference_id(table: &str) -> std::string::String {
    format!("{}_id", cruet::to_singular(table).to_snake_case())
}

/// Create a table with `created_at` and `updated_at`.
///
/// `refs` lists `(parent_table, column)`. An empty column means
/// `<parent>_id`, and a `?` suffix on the parent makes the reference
/// optional.
///
/// # Errors
///
/// Propagates the database error.
pub async fn create_table(
    m: &SchemaManager<'_>,
    table: &str,
    cols: &[(&str, ColType)],
    refs: &[(&str, &str)],
) -> Result<(), DbErr> {
    let table = normalize_table(table);
    let mut stmt = Table::create()
        .table(Alias::new(&table))
        .if_not_exists()
        .take();
    stmt.col(date_time(Alias::new("created_at")).default(Expr::current_timestamp()));
    stmt.col(date_time(Alias::new("updated_at")).default(Expr::current_timestamp()));
    for (name, col) in cols {
        stmt.col(col.to_def(Alias::new(*name)));
    }
    for (parent, column) in refs {
        let (parent, nullable) = parent.strip_suffix('?').map_or_else(
            || (normalize_table(parent), false),
            |stripped| (normalize_table(stripped), true),
        );
        let column = if column.is_empty() {
            reference_id(&parent)
        } else {
            (*column).to_string()
        };
        if !cols.iter().any(|(name, _)| *name == column) {
            let col = if nullable {
                ColType::BigIntegerNull
            } else {
                ColType::BigInteger
            };
            stmt.col(col.to_def(Alias::new(&column)));
        }
        let mut fk = ForeignKey::create();
        fk.name(format!("fk-{table}-{column}-to-{parent}"));
        fk.from(Alias::new(&table), Alias::new(&column));
        fk.to(Alias::new(parent), Alias::new("id"));
        if nullable {
            fk.on_delete(ForeignKeyAction::SetNull);
            fk.on_update(ForeignKeyAction::NoAction);
        } else {
            fk.on_delete(ForeignKeyAction::Cascade);
            fk.on_update(ForeignKeyAction::Cascade);
        }
        stmt.foreign_key(&mut fk);
    }
    m.create_table(stmt).await
}

/// Drop a table.
///
/// # Errors
///
/// Propagates the database error.
pub async fn drop_table(m: &SchemaManager<'_>, table: &str) -> Result<(), DbErr> {
    m.drop_table(
        Table::drop()
            .table(Alias::new(normalize_table(table)))
            .to_owned(),
    )
    .await
}

/// Add a column.
///
/// # Errors
///
/// Propagates the database error.
pub async fn add_column(
    m: &SchemaManager<'_>,
    table: &str,
    name: &str,
    col: ColType,
) -> Result<(), DbErr> {
    m.alter_table(
        Table::alter()
            .table(Alias::new(normalize_table(table)))
            .add_column(col.to_def(Alias::new(name)))
            .to_owned(),
    )
    .await
}

/// Remove a column.
///
/// # Errors
///
/// Propagates the database error.
pub async fn remove_column(m: &SchemaManager<'_>, table: &str, name: &str) -> Result<(), DbErr> {
    m.alter_table(
        Table::alter()
            .table(Alias::new(normalize_table(table)))
            .drop_column(Alias::new(name))
            .to_owned(),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_names_are_pluralised_snake_case() {
        assert_eq!(normalize_table("user"), "users");
        assert_eq!(normalize_table("users"), "users");
        assert_eq!(normalize_table("CarePathway"), "care_pathways");
    }

    #[test]
    fn reference_columns_are_singular() {
        assert_eq!(reference_id("users"), "user_id");
    }
}
