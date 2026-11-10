//! A small condition builder.
//!
//! It has the shape of Loco's, so code using it moves across unchanged. New
//! code can use `SeaORM` directly, which is just as short:
//! `Column::Email.eq(email)`.
//!
//! ```
//! use axa_postgres_seaorm::query::condition;
//! # use axa_postgres_seaorm::sea_orm::{self, entity::prelude::*};
//! # mod users {
//! #     use axa_postgres_seaorm::sea_orm::{self, entity::prelude::*};
//! #     #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
//! #     #[sea_orm(table_name = "users")]
//! #     pub struct Model {
//! #         #[sea_orm(primary_key)]
//! #         pub id: i32,
//! #         pub email: String,
//! #     }
//! #     #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
//! #     pub enum Relation {}
//! #     impl ActiveModelBehavior for ActiveModel {}
//! # }
//! let filter = condition().eq(users::Column::Email, "a@example.org").build();
//! ```

use sea_orm::{ColumnTrait, Condition, Value};

/// A set of conditions, all of which must hold.
#[derive(Debug, Clone)]
pub struct ConditionBuilder {
    condition: Condition,
}

/// Start an empty condition.
#[must_use]
pub fn condition() -> ConditionBuilder {
    ConditionBuilder {
        condition: Condition::all(),
    }
}

impl ConditionBuilder {
    /// Add a column equality.
    #[must_use]
    pub fn eq<T: ColumnTrait, V: Into<Value>>(self, col: T, value: V) -> Self {
        self.with(col.eq(value))
    }

    /// Add a column inequality.
    #[must_use]
    pub fn ne<T: ColumnTrait, V: Into<Value>>(self, col: T, value: V) -> Self {
        self.with(col.ne(value))
    }

    /// Add a "greater than" comparison.
    #[must_use]
    pub fn gt<T: ColumnTrait, V: Into<Value>>(self, col: T, value: V) -> Self {
        self.with(col.gt(value))
    }

    /// Add a "greater than or equal" comparison.
    #[must_use]
    pub fn gte<T: ColumnTrait, V: Into<Value>>(self, col: T, value: V) -> Self {
        self.with(col.gte(value))
    }

    /// Add a "less than" comparison.
    #[must_use]
    pub fn lt<T: ColumnTrait, V: Into<Value>>(self, col: T, value: V) -> Self {
        self.with(col.lt(value))
    }

    /// Add a "less than or equal" comparison.
    #[must_use]
    pub fn lte<T: ColumnTrait, V: Into<Value>>(self, col: T, value: V) -> Self {
        self.with(col.lte(value))
    }

    /// Require a column to be null.
    #[must_use]
    pub fn is_null<T: ColumnTrait>(self, col: T) -> Self {
        self.with(col.is_null())
    }

    /// Require a column not to be null.
    #[must_use]
    pub fn is_not_null<T: ColumnTrait>(self, col: T) -> Self {
        self.with(col.is_not_null())
    }

    /// Require a column to be one of `values`.
    #[must_use]
    pub fn is_in<T, V, I>(self, col: T, values: I) -> Self
    where
        T: ColumnTrait,
        V: Into<Value>,
        I: IntoIterator<Item = V>,
    {
        self.with(col.is_in(values))
    }

    /// Require a column to contain `text`. `%` and `_` in `text` act as `LIKE`
    /// wildcards, so escape text that came from a user.
    #[must_use]
    pub fn contains<T: ColumnTrait>(self, col: T, text: &str) -> Self {
        self.with(col.contains(text))
    }

    /// Add any `SeaORM` condition, for what this builder does not cover.
    #[must_use]
    pub fn with(mut self, condition: impl sea_orm::sea_query::IntoCondition) -> Self {
        self.condition = self.condition.add(condition.into_condition());
        self
    }

    /// Finish.
    #[must_use]
    pub fn build(&self) -> Condition {
        self.condition.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sea_orm::{DbBackend, QueryFilter, QueryTrait, entity::prelude::*};

    mod users {
        use sea_orm::entity::prelude::*;

        #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
        #[sea_orm(table_name = "users")]
        pub struct Model {
            #[sea_orm(primary_key)]
            pub id: i32,
            pub email: String,
            pub age: i32,
            pub deleted_at: Option<DateTimeWithTimeZone>,
        }

        #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
        pub enum Relation {}

        impl ActiveModelBehavior for ActiveModel {}
    }

    use users::{Column, Entity};

    fn sql(builder: &ConditionBuilder) -> String {
        Entity::find()
            .filter(builder.build())
            .build(DbBackend::Postgres)
            .to_string()
    }

    #[test]
    fn eq_and_ne_compare_values() {
        assert!(sql(&condition().eq(Column::Email, "a")).contains(r#""email" = 'a'"#));
        assert!(sql(&condition().ne(Column::Email, "a")).contains(r#""email" <> 'a'"#));
    }

    #[test]
    fn ordering_comparisons_use_their_operators() {
        assert!(sql(&condition().gt(Column::Age, 1)).contains(r#""age" > 1"#));
        assert!(sql(&condition().gte(Column::Age, 1)).contains(r#""age" >= 1"#));
        assert!(sql(&condition().lt(Column::Age, 1)).contains(r#""age" < 1"#));
        assert!(sql(&condition().lte(Column::Age, 1)).contains(r#""age" <= 1"#));
    }

    #[test]
    fn null_checks_use_is_null() {
        assert!(sql(&condition().is_null(Column::DeletedAt)).contains("IS NULL"));
        assert!(sql(&condition().is_not_null(Column::DeletedAt)).contains("IS NOT NULL"));
    }

    #[test]
    fn membership_lists_every_value() {
        assert!(sql(&condition().is_in(Column::Age, [1, 2])).contains("IN (1, 2)"));
    }

    #[test]
    fn contains_wraps_the_text_in_wildcards() {
        assert!(sql(&condition().contains(Column::Email, "ab")).contains("LIKE '%ab%'"));
    }

    #[test]
    fn several_conditions_must_all_hold() {
        let text = sql(&condition().eq(Column::Email, "a").gt(Column::Age, 1));
        assert!(text.contains(" AND "), "{text}");
    }

    #[test]
    fn an_empty_condition_filters_nothing() {
        // Rendered as `WHERE TRUE`, which matches every row.
        assert!(sql(&condition()).ends_with("WHERE TRUE"));
    }

    #[test]
    fn with_accepts_a_seaorm_condition() {
        let text = sql(&condition().with(Column::Email.eq("a")));
        assert!(text.contains(r#""email" = 'a'"#), "{text}");
    }
}
