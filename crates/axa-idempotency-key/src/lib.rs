//! `Idempotency-Key` handling over Postgres.
//!
//! A caller that sends `Idempotency-Key` on a write gets that write run once. A retry
//! gets the first answer back, marked `Idempotent-Replayed: true`, so a timeout on the
//! caller's side cannot create a second record.
//!
//! ```no_run
//! use axa_idempotency_key::{Idempotency, idempotent};
//! use axum::{Router, middleware::from_fn_with_state};
//! use sqlx::PgPool;
//!
//! #[derive(Clone)]
//! struct Caller(String);
//!
//! # async fn router(routes: Router, pool: PgPool) -> Result<Router, sqlx::Error> {
//! let keys = Idempotency::new(pool, |request| {
//!     request.extensions().get::<Caller>().map(|caller| caller.0.clone())
//! });
//! keys.check().await?;
//! Ok(routes.layer(from_fn_with_state(keys, idempotent)))
//! # }
//! ```
//!
//! # Where the layer goes
//!
//! Inside `axa::middleware::apply`, and inside anything that refuses a caller. Add it
//! after `into_router`, before the authentication layer, and call `apply` last:
//!
//! - Inside compression, it stores the plain body. Outside, it would see gzip.
//! - Inside authentication, a replay reaches only a caller the guards let through.
//!
//! [Where the layer goes](https://github.com/publicprojects-io/axa/blob/main/help/idempotency-keys/index.md)
//! has the whole router.
//!
//! # What happens to a request
//!
//! | The request | The answer |
//! |---|---|
//! | No key, or a method that changes nothing | Runs as if this layer were not there |
//! | No key, where [`Idempotency::required`] asks for one | `400` `idempotency_key_required` |
//! | First with this key | Runs, and a `2xx` or `3xx` answer is kept |
//! | Same key, same request, finished | The first answer, with `Idempotent-Replayed: true` |
//! | Same key, same request, still running | `409` `idempotency_in_progress`, with `Retry-After` |
//! | Same key, a different method, path, query, or body | `422` `idempotency_key_reused` |
//! | The first ran, and its answer could not be kept | `409` `idempotency_answer_not_kept` |
//! | The first failed with a `4xx` or `5xx` | Runs again: the key was released |
//! | The key is not 1 to 255 visible ASCII characters | `400` `idempotency_key_invalid` |
//! | No caller could be named | `401` `idempotency_no_caller` |
//! | The body is over [`Idempotency::max_request_bytes`] | `413` `idempotency_body_too_large` |
//! | The store cannot be reached | `503` `idempotency_unavailable`, and nothing runs |
//!
//! # The caller
//!
//! A key belongs to a caller, so that one caller cannot read another's answer. This
//! crate does not know who is calling: the closure given to [`Idempotency::new`] says.
//! It reads the request, usually an extension that authentication left there. It sees
//! no token and verifies nothing. A service with tenants returns a text that names both.
//!
//! When it returns `None` the request is refused, not run unprotected. A public route
//! that takes keys has to say whose they are, for example from a session, and returning
//! one fixed text there makes every anonymous caller share their answers.
//!
//! # A crash, a hang-up, and a panic
//!
//! Each of these ends the run with the key still claimed, and the lost run may have done
//! its work. So by default the claim holds the key for a whole [`Idempotency::ttl`]
//! from when its run last reported in, and a retry gets `409` `idempotency_in_progress`
//! until then. That is the safe answer:
//! the write never runs twice under one key. Once the `ttl` ends the key is free, as any
//! expired key is, and [`Idempotency::sweep`] deletes it. A client that has checked the
//! write did not happen can send it sooner under a new key.
//!
//! A run reports in every third of its lease, so a handler can run for as long as it
//! needs. The lease is the `ttl` unless a service chooses a shorter one with
//! [`Idempotency::rerun_abandoned_after`]. The same request may then take over a claim
//! not renewed for that long, and run the write a second time. Only a handler that is
//! safe to run twice should allow that. A different request under that key is always
//! refused.
//!
//! Each claim has its own identifier, so a run that lost its claim cannot overwrite the
//! answer of the run that took over.
//!
//! This crate stores an answer after the handler has committed, in a separate
//! transaction. A crash between the two leaves the key held with no answer to give, and
//! a retry under [`Idempotency::rerun_abandoned_after`] runs the handler again. A handler
//! that must never repeat, such as one that takes a payment, also needs a unique
//! constraint of its own.
//!
//! # What is kept
//!
//! The answers that are kept are a `2xx` or `3xx` with a known size of at most
//! [`Idempotency::max_response_bytes`] and no `content-encoding`, and only the headers in
//! [`KEPT_HEADERS`]. A `set-cookie` is never kept.
//!
//! Any other `2xx` or `3xx` reaches its caller untouched, and the key records that it ran
//! with no answer to give. A retry is refused with `409`, because running it again would
//! repeat the write. That covers a stream, which is passed on without being read, as one
//! may never end.
//!
//! A `4xx` or `5xx` releases the key, so that a caller can correct the request and try
//! again under the same key. A key must not hold a failure for good, and a `5xx` is what
//! a retry is for. A handler that commits and then fails gets run again, so it should
//! not do that.
//!
//! An answer is accepted work and not a result. A `202` for a job is replayed for as
//! long as the key lives, even once that job has failed. When such a job fails, call
//! [`Idempotency::forget`], which lets a retry under the same key run.
//!
//! # Personal data
//!
//! A kept answer is a copy of what the handler sent, and is not encrypted. It lives in
//! the `axa_idempotency` table for [`Idempotency::ttl`], 24 hours by default. The table
//! holds the caller, the key, and the answer, and of the request only a hash.
//!
//! An expired row is never replayed, but it stays on disk until someone deletes it, and
//! nothing here runs by itself, so **a service must call [`Idempotency::sweep`] on a
//! schedule**, or the answers are kept for ever. A route that answers with a secret, such
//! as one that issues a token, should stay out of this layer, which
//! [`Idempotency::only`] arranges.
//!
//! [`Idempotency::forget_caller`] erases by who made the request, and not by whose data
//! an answer holds. When staff create a patient, that answer belongs to the staff member,
//! so erasing the patient does not reach it, and it stays until `ttl` ends and the sweep
//! runs. A service must add `axa_idempotency` to its record of personal data, and its
//! privacy notice must allow for erasure taking up to `ttl`. A route that cannot accept
//! this is left out, or given a shorter `ttl`.
//!
//! Log lines hold no key, caller, or body.
//!
//! # The table
//!
//! This crate does not own a service's schema. Run [`SETUP_STATEMENTS`] in a migration
//! and call [`Idempotency::check`] at boot, so that a missing table stops the service
//! rather than its first write. [`setup`] does the same at boot, for a service whose
//! database role may change the schema.

use std::{sync::Arc, time::Duration};

use axa_problem::Problem;
use axum::{
    body::{Body, HttpBody, to_bytes},
    extract::{Request, State},
    http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};
use uuid::Uuid;

/// The header that names a write.
pub const HEADER: HeaderName = HeaderName::from_static("idempotency-key");

/// The header on an answer that is a replay.
pub const REPLAYED: HeaderName = HeaderName::from_static("idempotent-replayed");

/// The longest key accepted, in bytes.
pub const MAX_KEY_BYTES: usize = 255;

/// The headers of an answer that are kept with it. A new request id, the rate limit, and
/// the security headers come from the layers outside, and a `set-cookie` is left out, as
/// a replay should not hand out a session a second time.
pub const KEPT_HEADERS: [HeaderName; 8] = [
    header::CACHE_CONTROL,
    header::CONTENT_DISPOSITION,
    header::CONTENT_LANGUAGE,
    header::CONTENT_TYPE,
    header::ETAG,
    header::LAST_MODIFIED,
    header::LINK,
    header::LOCATION,
];

const SETUP: &str = "CREATE TABLE IF NOT EXISTS axa_idempotency (
    caller TEXT NOT NULL,
    key TEXT NOT NULL,
    fingerprint TEXT NOT NULL,
    claim UUID NOT NULL,
    state TEXT NOT NULL DEFAULT 'running' CHECK (state IN ('running', 'done', 'not_kept')),
    status INTEGER,
    headers JSONB,
    body BYTEA,
    lease_until TIMESTAMPTZ NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (caller, key)
)";
const SETUP_INDEX: &str =
    "CREATE INDEX IF NOT EXISTS axa_idempotency_expiry ON axa_idempotency (expires_at)";
/// Advisory lock key for table setup: `axa_i` as bytes.
const SETUP_LOCK: i64 = 0x0061_7861_5F69;
const CHECK: &str = "SELECT caller, key, fingerprint, claim, state, status, headers, body,
    lease_until, expires_at, created_at FROM axa_idempotency LIMIT 0";
/// One statement, so that of any number of requests exactly one takes the key. A row
/// that says nothing any more is reused in place: one past its expiry, or a run that
/// stopped reporting in, which only the same request may take.
const CLAIM: &str = "INSERT INTO axa_idempotency
        (caller, key, fingerprint, claim, lease_until, expires_at)
    VALUES ($1, $2, $3, $4, now() + make_interval(secs => $5), now() + make_interval(secs => $6))
    ON CONFLICT (caller, key) DO UPDATE SET
        fingerprint = EXCLUDED.fingerprint,
        claim = EXCLUDED.claim,
        state = 'running',
        status = NULL,
        headers = NULL,
        body = NULL,
        lease_until = EXCLUDED.lease_until,
        expires_at = EXCLUDED.expires_at,
        created_at = now()
    WHERE axa_idempotency.lease_until <= now()
      AND (axa_idempotency.expires_at <= now()
           OR (axa_idempotency.state = 'running'
               AND axa_idempotency.fingerprint = EXCLUDED.fingerprint))
    RETURNING claim";
const LOOKUP: &str = "SELECT fingerprint, state, status, headers::text AS headers, body,
        expires_at <= now() AS expired,
        EXTRACT(EPOCH FROM now() - created_at)::float8 AS running_for,
        EXTRACT(EPOCH FROM lease_until - now())::float8 AS lease_left
    FROM axa_idempotency WHERE caller = $1 AND key = $2";
/// `claim` identifies this run, so a run that lost the key finds no row in any of these
/// and changes nothing.
const HEARTBEAT: &str = "UPDATE axa_idempotency SET lease_until = now() + make_interval(secs => $4)
    WHERE caller = $1 AND key = $2 AND claim = $3 AND state = 'running'";
const FINISH: &str = "UPDATE axa_idempotency SET state = 'done', status = $4,
        headers = $5::text::jsonb, body = $6,
        lease_until = now(), expires_at = now() + make_interval(secs => $7)
    WHERE caller = $1 AND key = $2 AND claim = $3 AND state = 'running'";
const NOT_KEPT: &str = "UPDATE axa_idempotency SET state = 'not_kept',
        lease_until = now(), expires_at = now() + make_interval(secs => $4)
    WHERE caller = $1 AND key = $2 AND claim = $3 AND state = 'running'";
const RELEASE: &str = "DELETE FROM axa_idempotency WHERE caller = $1 AND key = $2 AND claim = $3";
const FORGET: &str =
    "DELETE FROM axa_idempotency WHERE caller = $1 AND key = $2 AND state <> 'running'";
const FORGET_CALLER: &str = "DELETE FROM axa_idempotency WHERE caller = $1";
const SWEEP: &str =
    "DELETE FROM axa_idempotency WHERE expires_at <= now() AND lease_until <= now()";

/// The statements that make the table, in order, each safe to repeat. A service runs
/// these in a migration, and calls [`Idempotency::check`] at boot.
pub const SETUP_STATEMENTS: [&str; 2] = [SETUP, SETUP_INDEX];

/// The shortest lease, and so the shortest `ttl`. A run reports in every third of its
/// lease, so this keeps the heartbeat to at most one in 20 ms.
const MIN_LEASE: Duration = Duration::from_millis(60);

/// How many times a key is tried when the row it conflicted with goes away.
const CLAIM_TRIES: usize = 3;

/// Create the table if needed, which runs DDL. Use [`SETUP_STATEMENTS`] in a migration
/// where a service's role may not, or where its schema is its migrations'.
///
/// # Errors
///
/// Fails when the database cannot be reached or the table not created.
pub async fn setup(pool: &PgPool) -> Result<(), sqlx::Error> {
    // `CREATE ... IF NOT EXISTS` is not safe against itself: services booting together
    // can both pass the check. The lock makes setup run one at a time.
    let mut setup = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(SETUP_LOCK)
        .execute(&mut *setup)
        .await?;
    for statement in SETUP_STATEMENTS {
        sqlx::query(statement).execute(&mut *setup).await?;
    }
    setup.commit().await
}

type CallerOf = dyn Fn(&Request) -> Option<String> + Send + Sync;
type Covers = dyn Fn(&Method, &str) -> bool + Send + Sync;

/// The settings and the store behind [`idempotent`].
///
/// Clones share one pool.
#[derive(Clone)]
pub struct Idempotency {
    pool: PgPool,
    caller: Arc<CallerOf>,
    only: Option<Arc<Covers>>,
    required: Option<Arc<Covers>>,
    ttl: Duration,
    rerun_abandoned_after: Option<Duration>,
    max_request_bytes: usize,
    max_response_bytes: usize,
}

impl std::fmt::Debug for Idempotency {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Idempotency")
            .field("ttl", &self.ttl)
            .field("rerun_abandoned_after", &self.rerun_abandoned_after)
            .field("max_request_bytes", &self.max_request_bytes)
            .field("max_response_bytes", &self.max_response_bytes)
            .finish_non_exhaustive()
    }
}

impl Idempotency {
    /// Keep answers in `pool`, with `caller` naming who each request is from. A pool from
    /// `sea_orm` is available as `DatabaseConnection::get_postgres_connection_pool`.
    ///
    /// `caller` runs on each request that carries a key. It reads what authentication
    /// left on the request, and `None` or an empty text refuses the request.
    pub fn new(
        pool: PgPool,
        caller: impl Fn(&Request) -> Option<String> + Send + Sync + 'static,
    ) -> Self {
        Self {
            pool,
            caller: Arc::new(caller),
            only: None,
            required: None,
            ttl: Duration::from_hours(24),
            rerun_abandoned_after: None,
            max_request_bytes: 1024 * 1024,
            max_response_bytes: 1024 * 1024,
        }
    }

    /// Cover only the requests this says yes to, given the method and path. By default
    /// every request that can change something is covered: not `GET`, `HEAD`, `OPTIONS`,
    /// or `TRACE`, which this cannot widen. A request left out is passed on and its key
    /// ignored, for a route that keeps idempotency of its own.
    #[must_use]
    pub fn only(mut self, covers: impl Fn(&Method, &str) -> bool + Send + Sync + 'static) -> Self {
        self.only = Some(Arc::new(covers));
        self
    }

    /// Refuse a request with no key when this says yes, given the method and path. By
    /// default a key is optional everywhere. This applies only to what is covered, so it
    /// cannot ask a read, or a route [`Idempotency::only`] left out, for a key. Say so in
    /// the service's documentation, as a client cannot find out any other way.
    #[must_use]
    pub fn required(
        mut self,
        requires: impl Fn(&Method, &str) -> bool + Send + Sync + 'static,
    ) -> Self {
        self.required = Some(Arc::new(requires));
        self
    }

    /// How long a key and its answer are kept. 24 hours by default. A run that ends
    /// without an answer, by a crash, a hang-up, or a panic, holds its key this long too,
    /// unless [`Idempotency::rerun_abandoned_after`] says otherwise.
    ///
    /// # Panics
    ///
    /// When `ttl` is under 60 milliseconds, or shorter than the lease given to
    /// [`Idempotency::rerun_abandoned_after`]. These are mistakes in code, found at boot.
    #[must_use]
    pub const fn ttl(mut self, ttl: Duration) -> Self {
        assert!(
            ttl.as_nanos() >= MIN_LEASE.as_nanos(),
            "an idempotency ttl must be at least 60 milliseconds"
        );
        if let Some(lease) = self.rerun_abandoned_after {
            assert!(
                lease.as_nanos() <= ttl.as_nanos(),
                "an idempotency ttl cannot be shorter than rerun_abandoned_after"
            );
        }
        self.ttl = ttl;
        self
    }

    /// Let the same request run the write a second time once its first run has not
    /// reported in for `lease`, as after a crash. Without this, a claim not finished holds
    /// its key for the whole [`Idempotency::ttl`], and the write never runs twice.
    ///
    /// Use it only for a handler that is safe to run twice, such as one that keeps a
    /// unique constraint of its own. The first run may have done its work before it was
    /// lost, and this runs it again. A run reports in every third of `lease`, so a live
    /// run is not taken over however long it takes.
    ///
    /// # Panics
    ///
    /// When `lease` is under 60 milliseconds, which the heartbeat cannot keep, or longer
    /// than the `ttl`, which would hold a key past its life. These are mistakes in code,
    /// found at boot.
    #[must_use]
    pub const fn rerun_abandoned_after(mut self, lease: Duration) -> Self {
        assert!(
            lease.as_nanos() >= MIN_LEASE.as_nanos(),
            "rerun_abandoned_after must be at least 60 milliseconds"
        );
        assert!(
            lease.as_nanos() <= self.ttl.as_nanos(),
            "rerun_abandoned_after cannot be longer than the idempotency ttl"
        );
        self.rerun_abandoned_after = Some(lease);
        self
    }

    /// The largest request held to compare it, 1 MiB by default. A larger one with a key
    /// is refused with a `413`. This applies with `axa`'s body limit, and neither raises
    /// the other, so a route accepting more needs both raised or to be left out by
    /// [`Idempotency::only`].
    #[must_use]
    pub const fn max_request_bytes(mut self, bytes: usize) -> Self {
        self.max_request_bytes = bytes;
        self
    }

    /// The largest answer kept, 1 MiB by default.
    #[must_use]
    pub const fn max_response_bytes(mut self, bytes: usize) -> Self {
        self.max_response_bytes = bytes;
        self
    }

    /// Check that the table is there and has every column, and change nothing. Call this
    /// at boot, so that a missing table is found then.
    ///
    /// # Errors
    ///
    /// Fails when the database cannot be reached, or the table is missing or out of date.
    pub async fn check(&self) -> Result<(), sqlx::Error> {
        sqlx::query(CHECK).execute(&self.pool).await.map(|_| ())
    }

    /// Delete what has expired, and return how many rows went. Nothing calls this for
    /// you, and until something does an expired answer stays on disk. Safe to run on
    /// every replica, and at any time.
    ///
    /// # Errors
    ///
    /// Fails on a database error.
    pub async fn sweep(&self) -> Result<u64, sqlx::Error> {
        let result = sqlx::query(SWEEP).execute(&self.pool).await?;
        Ok(result.rows_affected())
    }

    /// Free one finished key, so that a retry under it runs. For a `202` whose job later
    /// failed. A run in progress is left alone, and `false` says nothing was freed.
    ///
    /// # Errors
    ///
    /// Fails on a database error.
    pub async fn forget(&self, caller: &str, key: &str) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(FORGET)
            .bind(caller)
            .bind(key)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Delete everything held for a caller, to erase a person. A run in progress keeps
    /// going and stores nothing. Returns how many rows went.
    ///
    /// # Errors
    ///
    /// Fails on a database error.
    pub async fn forget_caller(&self, caller: &str) -> Result<u64, sqlx::Error> {
        let result = sqlx::query(FORGET_CALLER)
            .bind(caller)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected())
    }

    /// How long a claim lasts without reporting in: the `ttl`, unless a service chose
    /// a shorter one.
    fn lease(&self) -> Duration {
        self.rerun_abandoned_after.unwrap_or(self.ttl)
    }

    fn covers(&self, method: &Method, path: &str) -> bool {
        !method.is_safe() && self.only.as_ref().is_none_or(|only| only(method, path))
    }

    fn requires(&self, method: &Method, path: &str) -> bool {
        self.required
            .as_ref()
            .is_some_and(|required| required(method, path))
    }

    /// Take the key, or say what to answer instead.
    async fn claim(&self, id: &Id) -> Result<Claimed, sqlx::Error> {
        let claim = Uuid::new_v4();
        for _ in 0..CLAIM_TRIES {
            let taken = sqlx::query(CLAIM)
                .bind(&id.caller)
                .bind(&id.key)
                .bind(&id.fingerprint)
                .bind(claim)
                .bind(self.lease().as_secs_f64())
                .bind(self.ttl.as_secs_f64())
                .fetch_optional(&self.pool)
                .await?;
            if taken.is_some() {
                return Ok(Claimed::Run(claim));
            }
            let row = sqlx::query(LOOKUP)
                .bind(&id.caller)
                .bind(&id.key)
                .fetch_optional(&self.pool)
                .await?;
            let row = row.as_ref().map(Stored::read).transpose()?;
            match decide(row.as_ref(), &id.fingerprint) {
                Decision::Claim => {}
                Decision::Reused => return Ok(Claimed::Answer(reused())),
                Decision::Running => {
                    let wait = row
                        .as_ref()
                        .map_or(1, |row| wait(row.running_for, row.lease_left));
                    return Ok(Claimed::Answer(in_progress(wait)));
                }
                Decision::NotKept => return Ok(Claimed::Answer(not_kept())),
                Decision::Replay => {
                    return Ok(Claimed::Answer(
                        row.as_ref()
                            .and_then(Stored::replay)
                            .unwrap_or_else(unreadable),
                    ));
                }
            }
        }
        // The row keeps changing under this request: another is at work on this key.
        Ok(Claimed::Answer(in_progress(1)))
    }

    /// Run `work`, reporting in as it goes. The heartbeat is polled beside the work and
    /// not between its polls, so a slow database cannot hold up a handler.
    async fn hold(&self, run: &Run, work: impl Future<Output = Response>) -> Response {
        let heartbeat = async {
            loop {
                // At least 20 ms, as no lease is under `MIN_LEASE`. A 24-hour `ttl`
                // reports in every 8 hours.
                tokio::time::sleep(self.lease() / 3).await;
                self.renew(run).await;
            }
        };
        tokio::select! {
            response = work => response,
            () = heartbeat => unreachable!("the heartbeat never ends"),
        }
    }

    async fn renew(&self, run: &Run) {
        let renewed = sqlx::query(HEARTBEAT)
            .bind(&run.id.caller)
            .bind(&run.id.key)
            .bind(run.claim)
            .bind(self.lease().as_secs_f64())
            .execute(&self.pool)
            .await;
        match renewed {
            Ok(result) if result.rows_affected() == 0 => {
                tracing::error!("idempotency_claim_lost");
            }
            Ok(_) => {}
            Err(error) => tracing::error!(%error, "idempotency_heartbeat_failed"),
        }
    }

    /// Keep the answer, or release the key, and hand on what the caller gets.
    async fn settle(&self, run: &Run, response: Response) -> Response {
        match outcome(&response, self.max_response_bytes) {
            Outcome::Release => {
                self.release(run).await;
                response
            }
            Outcome::Unkeepable(reason) => {
                tracing::warn!(reason, "idempotency_answer_not_kept");
                self.mark_not_kept(run).await;
                response
            }
            Outcome::Keep => {
                let (parts, body) = response.into_parts();
                let Ok(bytes) = to_bytes(body, self.max_response_bytes).await else {
                    // The handler may have done its work, so a retry must not repeat it.
                    self.mark_not_kept(run).await;
                    return refuse(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "internal_server_error",
                        None,
                    );
                };
                let kept = sqlx::query(FINISH)
                    .bind(&run.id.caller)
                    .bind(&run.id.key)
                    .bind(run.claim)
                    .bind(i32::from(parts.status.as_u16()))
                    .bind(kept_headers(&parts.headers))
                    .bind(bytes.as_ref())
                    .bind(self.ttl.as_secs_f64())
                    .execute(&self.pool)
                    .await;
                log_settled(kept);
                Response::from_parts(parts, Body::from(bytes))
            }
        }
    }

    async fn mark_not_kept(&self, run: &Run) {
        log_settled(
            sqlx::query(NOT_KEPT)
                .bind(&run.id.caller)
                .bind(&run.id.key)
                .bind(run.claim)
                .bind(self.ttl.as_secs_f64())
                .execute(&self.pool)
                .await,
        );
    }

    async fn release(&self, run: &Run) {
        let released = sqlx::query(RELEASE)
            .bind(&run.id.caller)
            .bind(&run.id.key)
            .bind(run.claim)
            .execute(&self.pool)
            .await;
        // The caller gets its answer regardless, and the key comes free when its lease
        // ends.
        if let Err(error) = released {
            tracing::error!(%error, "idempotency_release_failed");
        }
    }
}

/// Say what became of a finished run. The caller gets its answer whatever happens here.
/// When the store fails, the claim holds the key until its lease ends, which is the `ttl`
/// unless a service chose to allow a second run.
fn log_settled(result: Result<sqlx::postgres::PgQueryResult, sqlx::Error>) {
    match result {
        Ok(result) if result.rows_affected() == 0 => tracing::error!("idempotency_claim_lost"),
        Ok(_) => {}
        Err(error) => tracing::error!(%error, "idempotency_store_failed"),
    }
}

/// The layer. See the [crate documentation](crate), which says where it goes.
pub async fn idempotent(State(keys): State<Idempotency>, request: Request, next: Next) -> Response {
    if !keys.covers(request.method(), request.uri().path()) {
        return next.run(request).await;
    }
    if !request.headers().contains_key(HEADER) {
        if keys.requires(request.method(), request.uri().path()) {
            return refuse(
                StatusCode::BAD_REQUEST,
                "idempotency_key_required",
                Some("this route needs an Idempotency-Key header"),
            );
        }
        return next.run(request).await;
    }
    let Some(key) = key_of(request.headers()) else {
        return refuse(
            StatusCode::BAD_REQUEST,
            "idempotency_key_invalid",
            Some("Idempotency-Key must be one value of 1 to 255 visible ASCII characters"),
        );
    };
    let Some(caller) = (keys.caller)(&request).filter(|caller| !caller.is_empty()) else {
        return refuse(StatusCode::UNAUTHORIZED, "idempotency_no_caller", None);
    };
    let (parts, body) = request.into_parts();
    let Ok(bytes) = to_bytes(body, keys.max_request_bytes).await else {
        return refuse(
            StatusCode::PAYLOAD_TOO_LARGE,
            "idempotency_body_too_large",
            Some("the body is too large for a request with an Idempotency-Key"),
        );
    };
    let id = Id {
        fingerprint: fingerprint(
            parts.method.as_str(),
            parts.uri.path(),
            parts.uri.query().unwrap_or(""),
            &bytes,
        ),
        caller,
        key,
    };
    let claim = match keys.claim(&id).await {
        Ok(Claimed::Run(claim)) => claim,
        Ok(Claimed::Answer(answer)) => return answer,
        Err(error) => {
            tracing::error!(%error, "idempotency_store_failed");
            return unavailable();
        }
    };
    let run = Run { id, claim };
    let work = next.run(Request::from_parts(parts, Body::from(bytes)));
    let response = keys.hold(&run, work).await;
    keys.settle(&run, response).await
}

/// What identifies a request to the store.
struct Id {
    caller: String,
    key: String,
    fingerprint: String,
}

/// A request that holds its key.
struct Run {
    id: Id,
    claim: Uuid,
}

/// The result of trying for a key.
enum Claimed {
    /// This request holds the key under this claim.
    Run(Uuid),
    /// Send this instead of running.
    Answer(Response),
}

/// What is held under a key.
#[derive(Debug, PartialEq)]
struct Stored {
    fingerprint: String,
    state: String,
    status: Option<i32>,
    headers: Option<String>,
    body: Option<Vec<u8>>,
    expired: bool,
    /// Seconds since this claim was taken.
    running_for: f64,
    /// Seconds until the claim can be taken over, if its run stops reporting in.
    lease_left: f64,
}

impl Stored {
    fn read(row: &sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        Ok(Self {
            fingerprint: row.try_get("fingerprint")?,
            state: row.try_get("state")?,
            status: row.try_get("status")?,
            headers: row.try_get("headers")?,
            body: row.try_get("body")?,
            expired: row.try_get("expired")?,
            running_for: row.try_get("running_for")?,
            lease_left: row.try_get("lease_left")?,
        })
    }

    /// The first answer again, or `None` if the row cannot be read as one.
    fn replay(&self) -> Option<Response> {
        let status = u16::try_from(self.status?).ok()?;
        let headers: Vec<(String, String)> = serde_json::from_str(self.headers.as_deref()?).ok()?;
        let mut response = Response::new(Body::from(self.body.clone()?));
        *response.status_mut() = StatusCode::from_u16(status).ok()?;
        for (name, value) in headers {
            response.headers_mut().insert(
                HeaderName::try_from(name).ok()?,
                HeaderValue::try_from(value).ok()?,
            );
        }
        response
            .headers_mut()
            .insert(REPLAYED, HeaderValue::from_static("true"));
        Some(response)
    }
}

/// What to do with a request, given what the key holds once it could not be claimed.
#[derive(Debug, PartialEq, Eq)]
enum Decision {
    /// Nothing that counts is held: try for the key again.
    Claim,
    /// The key was used for a different request.
    Reused,
    /// The same request is running.
    Running,
    /// The same request finished, and its answer is kept.
    Replay,
    /// The same request finished, and its answer could not be kept.
    NotKept,
}

fn decide(stored: Option<&Stored>, fingerprint: &str) -> Decision {
    let Some(stored) = stored else {
        return Decision::Claim;
    };
    if stored.expired && stored.state != "running" {
        return Decision::Claim;
    }
    if stored.fingerprint != fingerprint {
        return Decision::Reused;
    }
    match stored.state.as_str() {
        "done" => Decision::Replay,
        "not_kept" => Decision::NotKept,
        _ => Decision::Running,
    }
}

/// What becomes of a finished run.
#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    /// Store the answer.
    Keep,
    /// Forget the key, so a retry runs.
    Release,
    /// It ran, and its answer cannot be stored, for this reason.
    Unkeepable(&'static str),
}

fn outcome(response: &Response, max_bytes: usize) -> Outcome {
    let status = response.status();
    if !(status.is_success() || status.is_redirection()) {
        return Outcome::Release;
    }
    // Stored, this would replay to a caller that never asked for it, and with no
    // `content-encoding` to say what it is. A plain body is what this layer sees when it
    // sits where it should, inside compression.
    if response.headers().contains_key(header::CONTENT_ENCODING) {
        return Outcome::Unkeepable("encoded");
    }
    match response.body().size_hint().exact() {
        Some(size) if usize::try_from(size).is_ok_and(|size| size <= max_bytes) => Outcome::Keep,
        Some(_) => Outcome::Unkeepable("too_large"),
        None => Outcome::Unkeepable("stream"),
    }
}

/// The key a request carries, when it is one value of visible ASCII within the limit.
/// A key is never trimmed or cut, as two keys cut to one would share an answer.
fn key_of(headers: &HeaderMap) -> Option<String> {
    let mut values = headers.get_all(HEADER).iter();
    let value = values.next()?;
    if values.next().is_some() {
        return None;
    }
    let bytes = value.as_bytes();
    let usable = !bytes.is_empty()
        && bytes.len() <= MAX_KEY_BYTES
        && bytes.iter().all(|byte| (0x21..=0x7e).contains(byte));
    usable.then(|| value.to_str().ok().map(str::to_string))?
}

/// A hash of what makes a request the same request. Each part carries its length, so
/// that moving a byte from one part to the next changes the hash.
fn fingerprint(method: &str, path: &str, query: &str, body: &[u8]) -> String {
    use std::fmt::Write as _;

    let mut hash = Sha256::new();
    for part in [method.as_bytes(), path.as_bytes(), query.as_bytes(), body] {
        hash.update((part.len() as u64).to_be_bytes());
        hash.update(part);
    }
    hash.finalize()
        .iter()
        .fold(String::with_capacity(64), |mut text, byte| {
            let _ = write!(text, "{byte:02x}");
            text
        })
}

/// The headers worth keeping, as JSON. A value that is not text is dropped.
fn kept_headers(headers: &HeaderMap) -> String {
    let kept: Vec<(&str, &str)> = KEPT_HEADERS
        .iter()
        .filter_map(|name| {
            let value = headers.get(name)?.to_str().ok()?;
            Some((name.as_str(), value))
        })
        .collect();
    serde_json::json!(kept).to_string()
}

fn refuse(status: StatusCode, code: &str, detail: Option<&str>) -> Response {
    let mut problem = Problem::new(status).with_extension("code", serde_json::json!(code));
    if let Some(detail) = detail {
        problem = problem.with_detail(detail);
    }
    problem.into_response()
}

fn reused() -> Response {
    refuse(
        StatusCode::UNPROCESSABLE_ENTITY,
        "idempotency_key_reused",
        Some("this Idempotency-Key was used for a different request"),
    )
}

/// How many seconds a duplicate is told to wait: as long as the first has run so far,
/// from 1 second up to when its claim could be taken over.
///
/// Most runs finish in under a second, so most duplicates wait one. A client that obeys
/// waits about twice as long each time, so a run lost under a 24-hour lease is asked
/// about some 17 times rather than once a second, and never told to wait past the moment
/// its key could come free.
fn wait(running_for: f64, lease_left: f64) -> u64 {
    let ceil = |seconds: f64| {
        if seconds.is_finite() && seconds > 1.0 {
            // A lease is at most a `ttl`, which fits in a `u64` of seconds.
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let seconds = seconds.ceil() as u64;
            seconds
        } else {
            1
        }
    };
    ceil(running_for).min(ceil(lease_left))
}

fn in_progress(wait: u64) -> Response {
    let mut response = refuse(
        StatusCode::CONFLICT,
        "idempotency_in_progress",
        Some("the first request with this Idempotency-Key has not finished"),
    );
    response
        .headers_mut()
        .insert(header::RETRY_AFTER, HeaderValue::from(wait));
    response
}

fn not_kept() -> Response {
    refuse(
        StatusCode::CONFLICT,
        "idempotency_answer_not_kept",
        Some("the first request with this Idempotency-Key ran, and its answer was not kept"),
    )
}

fn unavailable() -> Response {
    refuse(
        StatusCode::SERVICE_UNAVAILABLE,
        "idempotency_unavailable",
        None,
    )
}

/// A row that says the request ran, in a shape nothing here wrote. It must not run again,
/// and there is nothing to send.
fn unreadable() -> Response {
    tracing::error!("idempotency_row_unreadable");
    unavailable()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stored(state: &str, print: &str) -> Stored {
        Stored {
            fingerprint: print.to_string(),
            state: state.to_string(),
            status: Some(201),
            headers: Some(r#"[["content-type","application/json"]]"#.to_string()),
            body: Some(b"{}".to_vec()),
            expired: false,
            running_for: 0.0,
            lease_left: 60.0,
        }
    }

    #[test]
    fn a_duplicate_waits_as_long_as_the_first_has_run_and_no_longer_than_its_lease() {
        // A run under a second, or a clock that reads a little behind: one second.
        assert_eq!(wait(0.0, 86_400.0), 1);
        assert_eq!(wait(0.2, 86_400.0), 1);
        assert_eq!(wait(-3.0, 86_400.0), 1);
        // Rounded up, so a duplicate never asks again before it said it would.
        assert_eq!(wait(1.2, 86_400.0), 2);
        assert_eq!(wait(3_600.0, 86_400.0), 3_600);
        // Never past the moment the claim could be taken over.
        assert_eq!(wait(72_000.0, 600.0), 600);
        assert_eq!(wait(72_000.0, 0.5), 1);
        assert_eq!(wait(72_000.0, -1.0), 1);
        assert_eq!(wait(f64::NAN, f64::INFINITY), 1);
    }

    #[test]
    fn obeying_the_wait_asks_about_a_lost_run_a_handful_of_times_a_day() {
        let (lease, mut at, mut asked) = (86_400.0, 0.0, 0);
        while at < lease {
            #[allow(clippy::cast_precision_loss)]
            let waited = wait(at, lease - at) as f64;
            at += waited;
            asked += 1;
        }
        assert!(asked <= 18, "asked {asked} times");
    }

    fn headers_with(values: &[&[u8]]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for value in values {
            headers.append(HEADER, HeaderValue::from_bytes(value).unwrap());
        }
        headers
    }

    #[test]
    fn a_key_is_one_value_of_visible_ascii_within_the_limit() {
        let longest = "x".repeat(MAX_KEY_BYTES);
        let good = key_of(&headers_with(&[b"abc-123"]));
        assert_eq!(good.as_deref(), Some("abc-123"));
        assert_eq!(
            key_of(&headers_with(&[longest.as_bytes()])).as_deref(),
            Some(longest.as_str())
        );
        let too_long = "x".repeat(MAX_KEY_BYTES + 1);
        for bad in [
            b"".as_slice(),
            b"   ",
            b"has space",
            b"tab\there",
            "ключ".as_bytes(),
            too_long.as_bytes(),
        ] {
            assert_eq!(key_of(&headers_with(&[bad])), None, "{bad:?}");
        }
        assert_eq!(key_of(&headers_with(&[b"one", b"two"])), None);
    }

    #[test]
    fn a_key_is_never_cut_so_two_keys_cannot_become_one() {
        let a = "x".repeat(MAX_KEY_BYTES + 1);
        assert_eq!(key_of(&headers_with(&[a.as_bytes()])), None);
    }

    #[test]
    fn the_fingerprint_sees_every_part_and_where_each_ends() {
        let base = fingerprint("POST", "/a", "x=1", b"{}");
        assert_eq!(base, fingerprint("POST", "/a", "x=1", b"{}"));
        assert_eq!(base.len(), 64);
        assert_ne!(base, fingerprint("PUT", "/a", "x=1", b"{}"));
        assert_ne!(base, fingerprint("POST", "/b", "x=1", b"{}"));
        assert_ne!(base, fingerprint("POST", "/a", "x=2", b"{}"));
        assert_ne!(base, fingerprint("POST", "/a", "x=1", b"{ }"));
        assert_ne!(
            fingerprint("POST", "/a", "b", b""),
            fingerprint("POST", "/ab", "", b"")
        );
    }

    #[test]
    fn what_a_key_holds_decides_the_answer() {
        assert_eq!(decide(None, "a"), Decision::Claim);
        assert_eq!(decide(Some(&stored("done", "a")), "a"), Decision::Replay);
        assert_eq!(
            decide(Some(&stored("running", "a")), "a"),
            Decision::Running
        );
        assert_eq!(
            decide(Some(&stored("not_kept", "a")), "a"),
            Decision::NotKept
        );
        for state in ["done", "running", "not_kept"] {
            assert_eq!(decide(Some(&stored(state, "a")), "b"), Decision::Reused);
        }
    }

    #[test]
    fn an_expired_answer_is_never_replayed() {
        let expired = |state| Stored {
            expired: true,
            ..stored(state, "a")
        };
        assert_eq!(decide(Some(&expired("done")), "a"), Decision::Claim);
        assert_eq!(decide(Some(&expired("not_kept")), "a"), Decision::Claim);
        // A run is its lease's to end, however long it has been going.
        assert_eq!(decide(Some(&expired("running")), "a"), Decision::Running);
    }

    fn response(status: StatusCode, body: Body) -> Response {
        let mut response = Response::new(body);
        *response.status_mut() = status;
        response
    }

    #[test]
    fn a_failure_is_released_and_a_success_or_redirect_is_kept() {
        for status in [StatusCode::OK, StatusCode::CREATED, StatusCode::SEE_OTHER] {
            let response = response(status, Body::from("{}"));
            assert_eq!(outcome(&response, 10), Outcome::Keep, "{status}");
        }
        for status in [
            StatusCode::BAD_REQUEST,
            StatusCode::CONFLICT,
            StatusCode::INTERNAL_SERVER_ERROR,
            StatusCode::SWITCHING_PROTOCOLS,
        ] {
            let response = response(status, Body::from("{}"));
            assert_eq!(outcome(&response, 10), Outcome::Release, "{status}");
        }
    }

    #[test]
    fn an_empty_answer_has_a_known_size_and_is_kept() {
        let response = response(StatusCode::NO_CONTENT, Body::empty());
        assert_eq!(outcome(&response, 0), Outcome::Keep);
    }

    #[test]
    fn what_cannot_be_replayed_is_not_kept() {
        let large = response(StatusCode::OK, Body::from("eleven char"));
        assert_eq!(outcome(&large, 10), Outcome::Unkeepable("too_large"));

        let mut encoded = response(StatusCode::OK, Body::from("x"));
        encoded
            .headers_mut()
            .insert(header::CONTENT_ENCODING, HeaderValue::from_static("gzip"));
        assert_eq!(outcome(&encoded, 10), Outcome::Unkeepable("encoded"));

        let chunks = Body::from("x").into_data_stream();
        let stream = response(StatusCode::OK, Body::from_stream(chunks));
        assert_eq!(outcome(&stream, 10), Outcome::Unkeepable("stream"));
    }

    #[test]
    fn only_the_listed_headers_are_kept() {
        let mut headers = HeaderMap::new();
        headers.insert(header::LOCATION, HeaderValue::from_static("/things/1"));
        headers.insert(header::SET_COOKIE, HeaderValue::from_static("session=1"));
        headers.insert("x-request-id", HeaderValue::from_static("first"));
        assert_eq!(kept_headers(&headers), r#"[["location","/things/1"]]"#);
        assert_eq!(kept_headers(&HeaderMap::new()), "[]");
    }

    #[test]
    fn a_replay_is_the_first_answer_and_says_so() {
        let row = Stored {
            headers: Some(r#"[["location","/things/1"]]"#.to_string()),
            ..stored("done", "a")
        };
        let replay = row.replay().unwrap();
        assert_eq!(replay.status(), StatusCode::CREATED);
        assert_eq!(replay.headers()[header::LOCATION], "/things/1");
        assert_eq!(replay.headers()[REPLAYED], "true");
    }

    #[test]
    fn a_row_that_cannot_be_read_is_not_guessed_at() {
        let broken = [
            Stored {
                status: None,
                ..stored("done", "a")
            },
            Stored {
                status: Some(99),
                ..stored("done", "a")
            },
            Stored {
                headers: Some("not json".to_string()),
                ..stored("done", "a")
            },
            Stored {
                body: None,
                ..stored("done", "a")
            },
        ];
        for row in broken {
            assert!(row.replay().is_none(), "{row:?}");
        }
    }

    fn keys() -> Idempotency {
        let pool = PgPool::connect_lazy("postgres://localhost/none").unwrap();
        Idempotency::new(pool, |_| None)
    }

    #[tokio::test]
    async fn the_lease_is_the_ttl_unless_a_shorter_one_is_chosen() {
        assert_eq!(keys().lease(), Duration::from_hours(24));
        let hour = Duration::from_hours(1);
        assert_eq!(keys().ttl(hour).lease(), hour);
        let minute = Duration::from_mins(1);
        assert_eq!(keys().rerun_abandoned_after(minute).lease(), minute);
        assert_eq!(
            keys().ttl(hour).rerun_abandoned_after(minute).lease(),
            minute
        );
        // Set either way round, the shorter lease is kept.
        assert_eq!(
            keys().rerun_abandoned_after(minute).ttl(hour).lease(),
            minute
        );
        assert_eq!(keys().rerun_abandoned_after(hour).ttl(hour).lease(), hour);
    }

    #[tokio::test]
    #[should_panic(expected = "cannot be longer than the idempotency ttl")]
    async fn a_lease_longer_than_the_ttl_is_refused() {
        let _ = keys()
            .ttl(Duration::from_mins(1))
            .rerun_abandoned_after(Duration::from_mins(2));
    }

    #[tokio::test]
    #[should_panic(expected = "cannot be shorter than rerun_abandoned_after")]
    async fn a_ttl_shorter_than_its_lease_is_refused() {
        let _ = keys()
            .rerun_abandoned_after(Duration::from_mins(2))
            .ttl(Duration::from_mins(1));
    }

    #[tokio::test]
    #[should_panic(expected = "rerun_abandoned_after must be at least 60 milliseconds")]
    async fn a_lease_the_heartbeat_cannot_keep_is_refused() {
        let _ = keys().rerun_abandoned_after(Duration::from_millis(59));
    }

    #[tokio::test]
    #[should_panic(expected = "ttl must be at least 60 milliseconds")]
    async fn a_ttl_the_heartbeat_cannot_keep_is_refused() {
        let _ = keys().ttl(Duration::ZERO);
    }

    #[tokio::test]
    async fn the_debug_text_holds_only_settings() {
        let pool = PgPool::connect_lazy("postgres://user:secret@localhost/none").unwrap();
        let keys = Idempotency::new(pool, |_| None);
        let text = format!("{keys:?}");
        assert!(text.contains("ttl"));
        assert!(!text.contains("secret"), "{text}");
    }
}
