//! An inbox for a consumer, so that it applies each event once.
//!
//! Delivery is at least once, so every consumer dedupes on the event's id. [`seen`]
//! does that in the consumer's own transaction: it notes the id, and says whether it
//! was noted before. The note commits or rolls back with what the consumer does with
//! the event, so an event whose handling failed is not taken as seen.
//!
//! ```no_run
//! # async fn example(pool: sqlx::PgPool, event_id: uuid::Uuid) -> Result<(), sqlx::Error> {
//! let mut transaction = pool.begin().await?;
//! if axa_outbox::inbox::seen(&mut transaction, event_id).await? {
//!     return Ok(()); // Applied before: acknowledge it and do nothing.
//! }
//! // ... apply the event in `transaction` ...
//! transaction.commit().await?;
//! # Ok(())
//! # }
//! ```
//!
//! `axa_inbox` is a record, so it is only ever inserted into, and its triggers refuse
//! every change. A consumer's migration runs [`SETUP_STATEMENTS`].

use sqlx::{Postgres, Transaction};
use uuid::Uuid;

const INBOX: &str = "CREATE TABLE IF NOT EXISTS axa_inbox (
    event_id UUID PRIMARY KEY,
    seen_at TIMESTAMPTZ NOT NULL DEFAULT now()
)";
const REFUSE: &str = "CREATE OR REPLACE FUNCTION axa_inbox_refuse() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION '% on axa_inbox is refused: the inbox is only ever added to', TG_OP;
END
$$";
const INBOX_REFUSE: &str = "CREATE OR REPLACE TRIGGER axa_inbox_refuse
    BEFORE UPDATE OR DELETE ON axa_inbox
    FOR EACH ROW EXECUTE FUNCTION axa_inbox_refuse()";
const INBOX_REFUSE_TRUNCATE: &str = "CREATE OR REPLACE TRIGGER axa_inbox_refuse_truncate
    BEFORE TRUNCATE ON axa_inbox
    FOR EACH STATEMENT EXECUTE FUNCTION axa_inbox_refuse()";
const ALWAYS: &str = "ALTER TABLE axa_inbox
    ENABLE ALWAYS TRIGGER axa_inbox_refuse,
    ENABLE ALWAYS TRIGGER axa_inbox_refuse_truncate";

/// The statements that make `axa_inbox` and its triggers, in order, each safe to repeat.
/// They need Postgres 14 or later.
pub const SETUP_STATEMENTS: [&str; 5] =
    [INBOX, REFUSE, INBOX_REFUSE, INBOX_REFUSE_TRUNCATE, ALWAYS];

/// Note `event_id` in the consumer's `transaction`, and say whether it had been noted
/// already, in which case the consumer skips the event. A second transaction noting the
/// same id waits for the first, and then sees it, unless the first rolled back.
///
/// # Errors
///
/// Fails on a database error, which includes a missing `axa_inbox`.
pub async fn seen(
    transaction: &mut Transaction<'_, Postgres>,
    event_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let noted = sqlx::query(
        "INSERT INTO axa_inbox (event_id) VALUES ($1) ON CONFLICT (event_id) DO NOTHING",
    )
    .bind(event_id)
    .execute(&mut **transaction)
    .await?
    .rows_affected();
    Ok(noted == 0)
}
