# Upgrading to Axa 0.18

This covers a service on 0.17. Most services upgrade by changing the version. One that
uses `http-client` checks where its TLS certificates come from, below.

## What will not compile

| Change | What to do |
|---|---|
| `axa-test` 0.9 is the one for `axa` 0.18 | Set `axa-test = "0.9"` in `[dev-dependencies]` |
| `axa::http_client` re-exports `reqwest` 0.13 | A service that names `reqwest` types through Axa, or depends on `reqwest` itself, moves to 0.13 and follows its changelog. One that uses only `Client`, `Builder`, `refusal`, and `bytes_limited` changes nothing |

## What changes with no compile error

**The HTTP client trusts the system's certificates.** `axa-http-client` checked
certificates against Mozilla's roots, built into the binary. It now checks them against
the operating system's trust store. A container image needs its CA certificates, as
the `ca-certificates` package, or every HTTPS request fails; a `scratch` or minimal
distroless image may have none. A private CA that the host trusts is now trusted too.

**The HTTP client installs ring as the process's TLS provider**, when it builds its
first client, unless the service installed one first, which it then keeps. TLS
elsewhere in the process that takes the process default then takes ring, as the other
Axa crates already use. No `aws-lc-rs` is built.

## What is new

`SessionOptions::create_table(false)` starts a session store without making its table,
for a database role that may not create tables, or a schema that only migrations
change. Make the table in a migration from `postgres_table_sql(table_name)`. The
default still makes it.
