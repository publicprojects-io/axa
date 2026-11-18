//! What Axa's standard middleware costs a request: the same handler on a bare Axum
//! router and behind `into_app`, driven with `oneshot`, with no network.

use axa::{
    Environment,
    config::{Config, Defaults},
    prelude::*,
};
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use criterion::{Criterion, criterion_group, criterion_main};
use tower::ServiceExt;

async fn hello() -> &'static str {
    "Hello, World!"
}

fn bare() -> Router {
    Router::new().route("/", get(hello))
}

fn standard(environment: Environment) -> Router {
    let defaults = Defaults {
        database: "",
        ..Defaults::default()
    };
    let config = Config::from_env(&environment, &defaults).unwrap();
    AppRoutes::with_default_routes()
        .add_route(Routes::new().add("/", get(hello)))
        .into_app(AppContext::new(environment, (), config))
        .unwrap()
}

async fn call(router: Router, path: &str) {
    let request = Request::builder().uri(path).body(Body::empty()).unwrap();
    let response = router.oneshot(request).await.unwrap();
    assert!(response.status() == StatusCode::OK || response.status() == StatusCode::NOT_FOUND);
    let _ = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
}

fn middleware(c: &mut Criterion) {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let mut group = c.benchmark_group("a GET");
    let cases = [
        ("bare axum", bare()),
        ("axa, test settings", standard(Environment::Test)),
        (
            "axa, development settings",
            standard(Environment::Development),
        ),
    ];
    for (name, router) in cases {
        group.bench_function(name, |b| {
            b.to_async(&runtime).iter(|| call(router.clone(), "/"));
        });
    }
    let router = standard(Environment::Test);
    group.bench_function("axa, an unknown path as a problem", |b| {
        b.to_async(&runtime).iter(|| call(router.clone(), "/nope"));
    });
    group.finish();
}

criterion_group!(benches, middleware);
criterion_main!(benches);
