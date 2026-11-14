# axa-webhooks

Webhook signing and verifying for Axum services, to the
[Standard Webhooks](https://www.standardwebhooks.com/) scheme, with HMAC-SHA256 and
Ed25519 keys. In `axa` it is the `webhooks` feature, as `axa::webhooks`.

```toml
[dependencies]
axa-webhooks = "0.1"
```

## Sending

```rust
use axa_webhooks::{Signer, SigningKey};

let key: SigningKey = "whsec_MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw".parse().unwrap();
let body = br#"{"type":"invoice.paid"}"#;
let headers = Signer::new(key).headers_now("msg_2p5jXN8AQM9LWM0D4", body).unwrap();
// Send `body` with `headers`: webhook-id, webhook-timestamp, and webhook-signature.
```

Use the same id on every retry of a delivery, so the receiver can tell a retry from a
new one.

## Receiving

```rust
use axa_webhooks::{Verified, Verifier, VerifyingKey};
use axum::{Router, routing::post};

async fn invoice_paid(Verified { delivery, body }: Verified) -> &'static str {
    // `body` is exactly what was signed: parse it here.
    let _ = (delivery.id, body);
    "ok"
}

let key: VerifyingKey = "whsec_MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw".parse().unwrap();
let app: Router = Router::new()
    .route("/hooks/billing", post(invoice_paid))
    .with_state(Verifier::new(key));
```

The handler runs only for a delivery signed by a key the verifier knows, with a
timestamp within five minutes of now. Otherwise the answer is a problem, which quotes
nothing that was sent:

| What was wrong | Status | `code` |
|---|---|---|
| A header is missing or malformed | `400` | `webhook_headers_invalid` |
| The timestamp is outside the window | `401` | `webhook_timestamp_out_of_range` |
| No signature verifies | `401` | `webhook_signature_invalid` |

The window stops a delivery being replayed later, but not within the window. A receiver
that must act once per delivery records each `delivery.id` it handles, for at least as
long as the window.

## Keys

| Key | Who holds it | Form |
|---|---|---|
| HMAC secret | Sender and receiver | `whsec_` and base64 of 24 to 64 random bytes |
| Ed25519 secret key | Sender | `whsk_` and base64 of the 32-byte seed, or of the seed and the public key |
| Ed25519 public key | Receiver | `whpk_` and base64 of 32 bytes, from `SigningKey::public_key` |

An HMAC secret is shared, so anyone who can verify can also sign. With Ed25519 the
receiver holds only the public key, which signs nothing.

To rotate a key, sign with both the old key and the new one,
`Signer::new(old).and(new)`, so each delivery lists two signatures. Move each receiver
to the new key, then stop signing with the old one. A receiver may also accept both
during the move, with `Verifier::new(new).and(old)`.

No key appears in `Debug` output or in a parse error.
