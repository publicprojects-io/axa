# Moving SeaORM code from Loco to axa

This guide covers the database half of leaving
[Loco](https://loco.rs). You keep SeaORM, your entities, and your migrations, and
swap Loco's helpers for `axa-seaorm`, which keeps their names and behaviour.

For the rest of the move (routes, boot, configuration, tests), see
[Converting a project from Loco to axa](../from-loco-into-axa/index.md).

## What moves

| Loco | axa | Change needed |
|---|---|---|
| `loco_rs::model::{ModelError, ModelResult}` | `axa::model` | Import only |
| `loco_rs::model::query::condition()` | `axa::query::condition()` | Import only |
| `loco_rs::schema` | `axa::schema` | Import only |
| `loco_rs::validation::Validatable` | `axa::validation::Validatable` | Import, plus `validator` |
| `loco_rs::db::truncate_table` | `axa::db::truncate_table` | Import only |
| `Authenticable` | Plain methods on your model | Small rewrite |
| `db::seed` from YAML fixtures | A test helper | Small rewrite |

## 1. Update the manifests

```toml
[dependencies]
axa = { version = "0.11", features = ["sea-orm"] }
```

In a separate migration crate:

```toml
[dependencies]
axa = { version = "0.11", features = ["sea-orm"] }
sea-orm-migration = { version = "2.0", features = ["runtime-tokio-rustls", "sqlx-postgres"] }
```

The `Validate` derive needs `validator` in your own manifest, with `derive`:

```toml
validator = { version = "0.20", features = ["derive"] }
```

Loco was enabling some features for you, so also expect to add `uuid`'s `serde`.

## 2. Replace the imports

| Replace | With |
|---|---|
| `loco_rs::prelude` | `axa::prelude` |
| `loco_rs::model::` | `axa::model::` |
| `loco_rs::schema` | `axa::schema` |
| `loco_rs::validation::` | `axa::validation::` |
| `loco_rs::db::` | `axa::db::` |

`axa::prelude` includes `ModelError`, `ModelResult`, `query`, `Validatable`, and
the SeaORM traits Loco's prelude exported.

## 3. Queries

Most code moves unchanged:

```rust,ignore
let user = users::Entity::find()
    .filter(query::condition().eq(users::Column::Email, email).build())
    .one(db)
    .await?;
```

The builder covers `eq`, `ne`, `gt`, `gte`, `lt`, `lte`, `is_null`, `is_not_null`,
`is_in`, and `contains`. For anything else, pass a SeaORM condition to `with`:

```rust,ignore
query::condition().with(users::Column::Name.like("a%")).build()
```

Or write the condition directly, which is just as short and is what we suggest
for new code:

```rust,ignore
.filter(users::Column::Email.eq(email))
```

`contains` treats `%` and `_` in its argument as wildcards, so escape text from a
user first, for example with `axa_cursor_pagination::escape_like`.

## 4. Validation

Your `Validatable` implementation and `before_save` stay the same:

```rust,ignore
use axa::prelude::*;
use validator::Validate;

#[derive(Debug, Validate, Deserialize)]
pub struct Validator {
    #[validate(email(message = "invalid email"))]
    pub email: String,
}

impl Validatable for ActiveModel {
    fn validator(&self) -> Box<dyn Validate> {
        Box::new(Validator {
            email: self.email.as_ref().to_owned(),
        })
    }
}
```

`self.validate()?` in `before_save` becomes the same `DbErr::Custom` JSON text as
under Loco, so existing snapshots of these errors keep matching. Returning
`ModelError::Validation` from a handler gives the same `400` with an `errors` body.

Bring `Validate` from `validator`, because `axa::prelude` does not export it.

## 5. Replace `Authenticable`

`axa` has no such trait. Its methods become plain ones on your model, and any
you no longer call can be deleted:

```rust,ignore
impl Model {
    pub async fn find_by_api_key(db: &DatabaseConnection, api_key: &str) -> ModelResult<Self> {
        users::Entity::find()
            .filter(users::Column::ApiKey.eq(api_key))
            .one(db)
            .await?
            .ok_or(ModelError::EntityNotFound)
    }
}
```

## 6. Migrations

Change only the import:

```rust,ignore
use axa::schema::{ColType, create_table, drop_table};
```

`create_table` adds `created_at` and `updated_at`, pluralises the table name, and
names foreign keys `fk-<table>-<column>-to-<parent>`, exactly as before. Your
`seaql_migrations` history is unaffected, because SeaORM still runs them.

The helpers cover `create_table`, `drop_table`, `add_column`, and `remove_column`,
with the common column types. A helper or `ColType` variant that is missing is
a compile error, so nothing can go wrong silently. Use SeaORM's own
`Table::create()` for those, or `execute_unprepared` with SQL.

## 7. Test databases

Check `config/test.yaml` before deleting it, because this is the mistake that
caused us the most trouble:

| `test.yaml` | Test boot |
|---|---|
| `dangerously_recreate: true` | `Migrator::fresh(&db)` |
| `auto_migrate` and `dangerously_truncate` | `Migrator::up(&db, None)`, then your `truncate` |

`truncate_table` empties only the tables you list. If `test.yaml` recreated the
schema, your truncate list may never have been complete, and rows from one test
then appear in the next.

## 8. Seed data

Loco's `db::seed` read YAML fixtures. Write a small helper instead, and insert
with `Entity::insert`:

```rust,ignore
users::Entity::insert(user).exec(db).await?;
```

Do not use `ActiveModel::insert` for fixtures. It runs `before_save`, which may
replace the fixed ids your tests look up.

## Check your work

Run your whole suite against a real Postgres, not only the unit tests, and compare
the counts with a run from before the change. Then confirm Loco has gone:

```sh
git grep -n -E 'loco_rs|loco-rs'
cargo tree -i loco-rs
```
