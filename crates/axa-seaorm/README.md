# axa-seaorm

SeaORM connections, migration helpers, and model errors for Axum services, from
the [axa](https://github.com/publicprojects-io/axa) family.

It exists to make leaving [Loco](https://loco.rs) easy: a project keeps SeaORM
and its code mostly as it is, because the helpers Loco provided keep their names
and behaviour here.

| Loco | axa-seaorm |
|---|---|
| `loco_rs::model::{ModelError, ModelResult}` | `axa_seaorm::model` |
| `loco_rs::model::query::condition()` | `axa_seaorm::query::condition()` |
| `loco_rs::schema` | `axa_seaorm::schema` |
| `loco_rs::validation::Validatable` | `axa_seaorm::validation::Validatable` |
| `loco_rs::db::truncate_table` | `axa_seaorm::truncate_table` |

Enable it through the `sea-orm` feature of `axa`, or depend on this crate alone.
The full walkthrough is
[Moving SeaORM code from Loco to axa](https://github.com/publicprojects-io/axa/blob/main/help/from-loco-seaorm-into-axa-seaorm/index.md).

## License

See [LICENSE.md](https://github.com/publicprojects-io/axa/blob/main/LICENSE.md).
