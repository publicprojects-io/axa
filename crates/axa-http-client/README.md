# axa-http-client

An outbound HTTP client for URLs that someone other than your service chose, such as
webhook receivers, avatars, link previews, and imports. It refuses to reach anywhere
that is not public, so a chosen URL cannot turn the service against its own network
(server-side request forgery). In `axa` it is the `http-client` feature, as
`axa::http_client`.

```toml
[dependencies]
axa-http-client = "0.1"
```

```rust,no_run
use axa_http_client::{Client, bytes_limited};

# async fn deliver(url: &str, body: Vec<u8>) -> Result<(), Box<dyn std::error::Error>> {
let client = Client::builder().https_only(true).build()?;
let response = client.post(url)?.body(body).send().await?;
let answer = bytes_limited(response, 64_000).await?;
# let _ = answer;
# Ok(())
# }
```

## What it refuses

Any address that is not public:

- private, carrier-grade NAT and loopback ranges;
- link-local, which holds the cloud metadata service at `169.254.169.254`;
- unique local IPv6, which holds `fd00:ec2::254`;
- documentation, benchmarking, multicast and reserved ranges, and `0.0.0.0`;
- IPv4 inside IPv6 (mapped, NAT64, 6to4 and Teredo), judged as the IPv4 address it
  reaches.

It checks where it connects, not only what the URL says:

| Where the address comes from | When it is checked |
|---|---|
| Written in the URL, in any form, such as `http://2130706433/` or `http://[::ffff:127.0.0.1]/` | Before sending, by `get`, `post`, or `request`, as `Refused::Address` |
| A host name | By the client's own DNS lookup, which keeps only allowed addresses and connects only to them. With none left, `send` fails, and `refusal(&error)` is `Refused::NoAddress` |
| A redirect | Every hop, the same way. `https` to `http` is `Refused::Downgrade` |

No proxy is used, not even one from `HTTP_PROXY`, as a proxy would make the
connection for it and so get around the checks.

## Options

| Builder | Effect |
|---|---|
| `https_only(true)` | Refuse `http`. Production should set it |
| `allow_hosts([...])` | Reach only these hosts, exactly, or every name under `*.example.com` |
| `allow_nets([...])` | Let these ranges, such as `10.20.0.0/16`, be reached though they are private. Never list one holding `169.254.169.254` unless you mean to |
| `max_redirects(n)` | 5 by default. With 0, a redirect is handed back as the response |
| `connect_timeout`, `timeout` | 5 and 30 seconds by default |

`bytes_limited(response, limit)` reads a body up to `limit` bytes. A URL chosen by
someone else can answer with a body of any size, so read through it rather than
`bytes()`.

## What it does not do

It cannot stop a service that takes `reqwest`'s own client back out of a request, with
`build_split`, and sends another URL with that. A response is whatever the server chose
to send, so treat it as untrusted. Section 4.23 of the spec gives the full threat model.
