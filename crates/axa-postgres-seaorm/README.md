# axa-postgres-seaorm

SeaORM connections, migrations, schema helpers, and test resets for Axum services,
from the [axa](https://github.com/publicprojects-io/axa) family.

It has the same shape as `axa-postgres-sqlx`, `axa-postgres-diesel`, and `axa-postgres-toasty`: a `Pool`, a
`PoolConfig`, `connect`, `ping`, `migrate`, `revert`, `pending`, `truncate`,
`dangerously_recreate`, `is_unique_violation`, and `quote_identifier`. The migration
functions take the service's `Migrator` as a type.

It exists to make leaving [Loco](https://loco.rs) easy: a project keeps SeaORM
and its code mostly as it is, because the helpers Loco provided keep their names
and behaviour here.

| Loco | axa-postgres-seaorm |
|---|---|
| `loco_rs::model::{ModelError, ModelResult}` | `axa::model`, the same for every database |
| `loco_rs::model::query::condition()` | `axa_postgres_seaorm::query::condition()` |
| `loco_rs::schema` | `axa_postgres_seaorm::schema` |
| `loco_rs::validation::Validatable` | `axa::validation::Validatable`, the same for every database |
| `loco_rs::db::truncate_table` | `axa_postgres_seaorm::truncate_table` |

Enable it through the `postgres-seaorm` feature of `axa`, or depend on this crate alone.
The full walkthrough is
[Moving SeaORM code from Loco to axa](https://github.com/publicprojects-io/axa/blob/main/help/from-loco-seaorm-into-axa-seaorm/index.md).

## License

See [LICENSE.md](https://github.com/publicprojects-io/axa/blob/main/LICENSE.md).
