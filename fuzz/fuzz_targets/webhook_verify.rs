//! `Verifier::verify_at` with any headers and body a sender makes up: a refusal a
//! client caused, never a panic, and never a delivery without a valid signature.

#![no_main]

use arbitrary::Arbitrary;
use axa_webhooks::{ID_HEADER, SIGNATURE_HEADER, Signer, SigningKey, TIMESTAMP_HEADER, Verifier};
use axum::http::{HeaderMap, HeaderValue};
use libfuzzer_sys::fuzz_target;

const SECRET: &str = "whsec_MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw";

#[derive(Arbitrary, Debug)]
struct Input<'a> {
    id: &'a [u8],
    timestamp: &'a [u8],
    signature: &'a [u8],
    body: &'a [u8],
    now: u32,
}

fuzz_target!(|input: Input| {
    let key: SigningKey = SECRET.parse().unwrap();
    let mut headers = HeaderMap::new();
    for (name, value) in [
        (ID_HEADER, input.id),
        (TIMESTAMP_HEADER, input.timestamp),
        (SIGNATURE_HEADER, input.signature),
    ] {
        if let Ok(value) = HeaderValue::from_bytes(value) {
            headers.insert(name, value);
        }
    }
    let now = u64::from(input.now);
    match Verifier::new(key.verifying_key()).verify_at(&headers, input.body, now) {
        Ok(delivery) => {
            // Accepted, so the signature must be the one this key makes.
            let made = Signer::new(key)
                .signature(&delivery.id, delivery.timestamp, input.body)
                .unwrap();
            let listed = headers[SIGNATURE_HEADER].to_str().unwrap();
            assert!(listed.split_ascii_whitespace().any(|entry| entry == made));
        }
        Err(refusal) => assert!(refusal.status().is_client_error()),
    }
});
