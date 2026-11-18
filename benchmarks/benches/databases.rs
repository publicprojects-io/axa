//! The same four operations on each database Axa's sqlx crates reach, through each
//! crate's own `connect`: a ping, an insert, a read by key, and a page of 50.
//!
//! SQLite always runs, on a file in the target directory. Postgres and MySQL run when
//! `AXA_BENCH_POSTGRES_URL` or `AXA_BENCH_MYSQL_URL` is set, and each makes and drops
//! the table `axa_bench_items`, so point them at a database for benchmarks, not tests:
//!
//! ```sh
//! AXA_BENCH_POSTGRES_URL=postgres://axa:axa@localhost:5432/axa_bench \
//! AXA_BENCH_MYSQL_URL=mysql://axa:axa@127.0.0.1:3307/axa_bench \
//! cargo bench --bench databases
//! ```

use std::sync::atomic::{AtomicI64, Ordering};

use criterion::{
    BenchmarkGroup, Criterion, criterion_group, criterion_main, measurement::WallTime,
};
use tokio::runtime::Runtime;

/// Rows in the table before reads are measured.
const ROWS: i64 = 1_000;

/// The SQL that differs between databases: only the placeholders.
struct Dialect {
    insert: &'static str,
    by_id: &'static str,
    page: &'static str,
}

const NUMBERED: Dialect = Dialect {
    insert: "INSERT INTO axa_bench_items (id, name) VALUES ($1, $2)",
    by_id: "SELECT id, name FROM axa_bench_items WHERE id = $1",
    page: "SELECT id, name FROM axa_bench_items ORDER BY id LIMIT 50 OFFSET $1",
};

const QUESTION_MARKS: Dialect = Dialect {
    insert: "INSERT INTO axa_bench_items (id, name) VALUES (?, ?)",
    by_id: "SELECT id, name FROM axa_bench_items WHERE id = ?",
    page: "SELECT id, name FROM axa_bench_items ORDER BY id LIMIT 50 OFFSET ?",
};

const CREATE: &str =
    "CREATE TABLE axa_bench_items (id BIGINT PRIMARY KEY, name VARCHAR(64) NOT NULL)";
const DROP: &str = "DROP TABLE IF EXISTS axa_bench_items";

/// Benchmark one database, given its pool and dialect. A macro, as sqlx's executors
/// differ in type for each database.
macro_rules! bench_database {
    ($group:expr, $runtime:expr, $name:expr, $pool:expr, $dialect:expr, $ping:path) => {{
        let pool = $pool;
        let dialect = &$dialect;
        $runtime.block_on(async {
            sqlx::query(DROP).execute(&pool).await.unwrap();
            sqlx::query(CREATE).execute(&pool).await.unwrap();
            for id in 0..ROWS {
                sqlx::query(dialect.insert)
                    .bind(id)
                    .bind(format!("item {id}"))
                    .execute(&pool)
                    .await
                    .unwrap();
            }
        });
        let next = AtomicI64::new(ROWS);
        $group.bench_function(format!("{}: ping", $name), |b| {
            b.to_async($runtime)
                .iter(|| async { $ping(&pool).await.unwrap() });
        });
        $group.bench_function(format!("{}: insert", $name), |b| {
            b.to_async($runtime).iter(|| async {
                let id = next.fetch_add(1, Ordering::Relaxed);
                sqlx::query(dialect.insert)
                    .bind(id)
                    .bind("new")
                    .execute(&pool)
                    .await
                    .unwrap();
            });
        });
        $group.bench_function(format!("{}: read by key", $name), |b| {
            b.to_async($runtime).iter(|| async {
                sqlx::query(dialect.by_id)
                    .bind(ROWS / 2)
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            });
        });
        $group.bench_function(format!("{}: page of 50", $name), |b| {
            b.to_async($runtime).iter(|| async {
                sqlx::query(dialect.page)
                    .bind(ROWS / 2)
                    .fetch_all(&pool)
                    .await
                    .unwrap();
            });
        });
        $runtime.block_on(async { sqlx::query(DROP).execute(&pool).await.unwrap() });
    }};
}

fn sqlite(group: &mut BenchmarkGroup<'_, WallTime>, runtime: &Runtime) {
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("axa_bench.sqlite");
    let _ = std::fs::remove_file(&path);
    let config = axa_sqlite_sqlx::PoolConfig::new(format!("sqlite://{}?mode=rwc", path.display()));
    let pool = runtime.block_on(axa_sqlite_sqlx::connect(&config)).unwrap();
    bench_database!(
        group,
        runtime,
        "sqlite",
        pool,
        QUESTION_MARKS,
        axa_sqlite_sqlx::ping
    );
}

fn postgres(group: &mut BenchmarkGroup<'_, WallTime>, runtime: &Runtime) {
    let Ok(url) = std::env::var("AXA_BENCH_POSTGRES_URL") else {
        eprintln!("postgres: skipped, AXA_BENCH_POSTGRES_URL is not set");
        return;
    };
    let config = axa_postgres_sqlx::PoolConfig::new(url);
    let pool = runtime
        .block_on(axa_postgres_sqlx::connect(&config))
        .unwrap();
    bench_database!(
        group,
        runtime,
        "postgres",
        pool,
        NUMBERED,
        axa_postgres_sqlx::ping
    );
}

fn mysql(group: &mut BenchmarkGroup<'_, WallTime>, runtime: &Runtime) {
    let Ok(url) = std::env::var("AXA_BENCH_MYSQL_URL") else {
        eprintln!("mysql: skipped, AXA_BENCH_MYSQL_URL is not set");
        return;
    };
    let config = axa_mysql_sqlx::PoolConfig::new(url);
    let pool = runtime.block_on(axa_mysql_sqlx::connect(&config)).unwrap();
    bench_database!(
        group,
        runtime,
        "mysql",
        pool,
        QUESTION_MARKS,
        axa_mysql_sqlx::ping
    );
}

fn databases(c: &mut Criterion) {
    let runtime = Runtime::new().unwrap();
    let mut group = c.benchmark_group("databases");
    sqlite(&mut group, &runtime);
    postgres(&mut group, &runtime);
    mysql(&mut group, &runtime);
    group.finish();
}

criterion_group!(benches, databases);
criterion_main!(benches);
