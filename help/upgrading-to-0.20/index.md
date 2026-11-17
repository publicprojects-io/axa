# Upgrading to Axa 0.20

This covers a service on 0.19. A service that neither takes nor sends Zstd upgrades by
changing the version.

## What will not compile

| Change | What to do |
|---|---|
| `axa-test` 0.11 is the one for `axa` 0.20 | Set `axa-test = "0.11"` in `[dev-dependencies]` |

## What changes with no compile error

| Change | What to do |
|---|---|
| Zstd is behind the `zstd` feature, off by default | A service whose clients send `content-encoding: zstd` bodies, or that wants to answer in Zstd, adds `features = ["zstd"]` to `axa`. Without it, a `zstd` request body is a `415` problem, and a caller asking for `zstd` alone gets an uncompressed answer. Gzip, Brotli, and deflate are unchanged |

## Why

Zstd builds the C zstd library, which needs a C compiler and brings C code into every
build. Few clients send Zstd request bodies, and browsers that accept Zstd also accept
Brotli and gzip, so most services lose nothing, and their builds become pure Rust for
compression.
