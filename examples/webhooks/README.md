# Webhooks

A service that takes signed webhooks from a billing provider, and sends signed webhooks
to a subscriber, with Axa's `webhooks` and `http-client` features.

## Run it

```sh
cd examples/webhooks
export WEBHOOK_SECRET="whsec_$(openssl rand -base64 32)"
export SUBSCRIBER_URL=https://example.com/hooks
cargo run
```

`WEBHOOK_SECRET` may instead come from a file, as a mounted secret does:
`WEBHOOK_SECRET_FILE=/run/secrets/webhook`. It is read with `config::Settings`, so a
production boot with no secret, or one that is not a `whsec_` key, stops and says so,
without printing the value.

## Receiving: `POST /hooks/billing`

The handler takes `Verified`, so it runs only for a verified delivery, and reads only
the body that was signed:

```rust,ignore
async fn billing(Verified { delivery, body }: Verified) -> Result<Response> {
    let event: Event = serde_json::from_slice(&body)?;
    // ...
}
```

`Verified` finds the `Verifier` in `ctx.shared_store`, where `app` puts it. Were it
missing, every delivery would be refused with a `500`, code `webhook_verifier_missing`.

| The delivery | The answer |
|---|---|
| Signed by the key, within five minutes | `200`, `{"received": "<id>"}` |
| Signed by another key, or changed after signing | `401`, code `webhook_signature_invalid` |
| Older than five minutes | `401`, code `webhook_timestamp_out_of_range` |
| Missing a `webhook-` header | `400`, code `webhook_headers_invalid` |

A delivery may come twice within the window, as a retry or a replay. A real service
keeps the ids it has handled for at least five minutes, and does nothing for an id it
has seen.

## Sending: `POST /api/notify`

The handler signs the event with `Signer::headers_now`, and sends it with
`axa::http_client::Client`. The subscriber's URL is the kind of URL someone else
chooses, so the client refuses any address that is not public:

| `SUBSCRIBER_URL` | The answer |
|---|---|
| `https://example.com/hooks`, answering `2xx` | `200`, `{"sent": "<id>"}` |
| `http://127.0.0.1/…`, `http://localhost/…`, `http://169.254.169.254/…`, `http://[::1]/…` | `422`, code `subscriber_refused`, before any connection |
| A host that fails or answers otherwise | `502`, code `subscriber_failed` |

In production the client takes `https` only. It reads at most 64 KB of the
subscriber's answer.

## Test it

```sh
cargo test
```

The tests sign deliveries with the example's key and with another key, and point the
subscriber at loopback and at the metadata address, which must never be reached.
