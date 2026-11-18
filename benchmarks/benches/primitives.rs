//! The per-request work Axa adds that is not middleware: checking a page's cursor,
//! verifying a webhook with each kind of key, and escaping text for `LIKE`.

use std::hint::black_box;

use axa_cursor_pagination::{CursorKey, CursorLimits, CursorPage, escape_like};
use axa_webhooks::{Signer, SigningKey, Verifier};
use axum::http::Uri;
use criterion::{Criterion, Throughput, criterion_group, criterion_main};

const LIMITS: CursorLimits = CursorLimits {
    default_limit: 50,
    max_limit: 200,
};

fn cursors(c: &mut Criterion) {
    let key = CursorKey::new(&[7; 32]).unwrap();
    let first: Uri = "/things?status=open&owner=ada&limit=50".parse().unwrap();
    let page = CursorPage::new(&key, LIMITS, &first).unwrap();
    let link = page.next_link().unwrap();
    let next: Uri = link
        .to_str()
        .unwrap()
        .trim_start_matches('<')
        .split('>')
        .next()
        .unwrap()
        .parse()
        .unwrap();
    let mut group = c.benchmark_group("cursor page");
    group.bench_function("first page", |b| {
        b.iter(|| CursorPage::new(&key, LIMITS, black_box(&first)).unwrap());
    });
    group.bench_function("next page, cursor verified", |b| {
        b.iter(|| CursorPage::new(&key, LIMITS, black_box(&next)).unwrap());
    });
    group.finish();
}

fn webhooks(c: &mut Criterion) {
    let hmac: SigningKey = "whsec_MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw".parse().unwrap();
    let ed25519: SigningKey = "whsk_BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc="
        .parse()
        .unwrap();
    let body = vec![b'x'; 4096];
    let mut group = c.benchmark_group("webhook, 4 KiB body");
    group.throughput(Throughput::Bytes(body.len() as u64));
    for (name, key) in [("hmac", hmac), ("ed25519", ed25519)] {
        let signer = Signer::new(key.clone());
        let headers = signer.headers("msg_1", 1_700_000_000, &body).unwrap();
        let verifier = Verifier::new(key.verifying_key());
        group.bench_function(format!("{name}: sign"), |b| {
            b.iter(|| {
                signer
                    .signature("msg_1", 1_700_000_000, black_box(&body))
                    .unwrap()
            });
        });
        group.bench_function(format!("{name}: verify"), |b| {
            b.iter(|| {
                verifier
                    .verify_at(&headers, black_box(&body), 1_700_000_000)
                    .unwrap()
            });
        });
    }
    group.finish();
}

fn like(c: &mut Criterion) {
    let text = "50% off_everything \\ today ".repeat(8);
    c.bench_function("escape_like, 216 bytes", |b| {
        b.iter(|| escape_like(black_box(&text)));
    });
}

criterion_group!(benches, cursors, webhooks, like);
criterion_main!(benches);
