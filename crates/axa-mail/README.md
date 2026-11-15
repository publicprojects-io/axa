# axa-mail

Mail for Axum services, over [`lettre`](https://crates.io/crates/lettre): an SMTP, file, or
memory transport chosen by a URL, with production refusing SMTP that does not use TLS. In
`axa` it is the `mail` feature, as `axa::mail`.

```toml
[dependencies]
axa-mail = "0.1"
```

```rust,no_run
use axa_mail::{Mailer, Message};

# async fn example(url: &str) -> Result<(), axa_mail::Error> {
let mailer = Mailer::from_url(url, true)?;
let message = Message::builder()
    .from("Accounts <accounts@example.com>".parse()?)
    .to("someone@example.org".parse()?)
    .subject("Your sign-in link")
    .body(String::from("Follow the link to sign in."))?;
mailer.send(&message).await?;
# Ok(())
# }
```

## Transports

| URL | Transport | In production |
|---|---|---|
| `smtps://user:password@smtp.example.com:465` | SMTP over TLS from the start | Yes |
| `smtp://user:password@smtp.example.com:587?tls=required` | SMTP that must upgrade with `STARTTLS` | Yes |
| `smtp://localhost:1025` | SMTP in the clear, for a local catcher such as Mailpit | Refused |
| `smtp://smtp.example.com?tls=opportunistic` | `STARTTLS` when offered, else in the clear | Refused |
| `file:///var/tmp/mail` | One `.eml` file per message, in that directory | Refused |
| `memory:` | Kept in memory | Refused |

Keep the URL, which may hold a password, in a secret: with `axa::config::Settings` it can
come from a `_FILE` path. Neither an error nor a mailer's `Debug` text quotes it.

## In tests

```rust
# #[tokio::main(flavor = "current_thread")]
# async fn main() {
let (mailer, outbox) = axa_mail::Mailer::memory();
// ... hand `mailer` to the code under test, which sends ...
# let message = axa_mail::Message::builder()
#     .from("a@example.com".parse().unwrap()).to("b@example.com".parse().unwrap())
#     .subject("Welcome").body(String::new()).unwrap();
# mailer.send(&message).await.unwrap();
let sent = outbox.take();
assert_eq!(sent[0].subject.as_deref(), Some("Welcome"));
# }
```

## What it does not do

It does not queue mail. `send` talks to the server while it is awaited, and a failure is
the caller's to retry. Send from a job, queued in the transaction that made the reason to
send, so mail goes out once the record is there and is retried with the job.
