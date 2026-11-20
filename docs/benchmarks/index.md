# Benchmarks

## Table of Contents

- [Benchmark Results](#benchmark-results)
    - [databases](#databases)
    - [a GET](#a-get)
    - [cursor page](#cursor-page)
    - [webhook, 4 KiB body](#webhook,-4-kib-body)
    - [escape_like](#escape_like)

## Benchmark Results

### databases

|        | `sqlite: ping`           | `sqlite: insert`                 | `sqlite: read by key`           | `sqlite: page of 50`             |
|:-------|:-------------------------|:---------------------------------|:--------------------------------|:-------------------------------- |
|        | `22.06 us` (✅ **1.00x**) | `159.28 us` (❌ *7.22x slower*)   | `22.38 us` (✅ **1.01x slower**) | `45.23 us` (❌ *2.05x slower*)    |

### a GET

|        | `bare axum`               | `axa, test settings`            | `axa, development settings`          | `axa, an unknown path as a problem`           |
|:-------|:--------------------------|:--------------------------------|:-------------------------------------|:--------------------------------------------- |
|        | `481.51 ns` (✅ **1.00x**) | `9.61 us` (❌ *19.96x slower*)   | `9.91 us` (❌ *20.58x slower*)        | `9.88 us` (❌ *20.53x slower*)                 |

### cursor page

|        | `first page`              | `next page, cursor verified`           |
|:-------|:--------------------------|:-------------------------------------- |
|        | `456.10 ns` (✅ **1.00x**) | `620.68 ns` (❌ *1.36x slower*)         |

### webhook, 4 KiB body

|        | `hmac: sign`            | `hmac: verify`                 | `ed25519: sign`                 | `ed25519: verify`                 |
|:-------|:------------------------|:-------------------------------|:--------------------------------|:--------------------------------- |
|        | `1.96 us` (✅ **1.00x**) | `2.01 us` (✅ **1.02x slower**) | `13.29 us` (❌ *6.78x slower*)   | `24.37 us` (❌ *12.43x slower*)    |

### escape_like

|        | `216 bytes`                |
|:-------|:-------------------------- |
|        | `382.87 ns` (✅ **1.00x**)  |

---
Made with [criterion-table](https://github.com/nu11ptr/criterion-table)


## Environment

- Revision: 64656a56c129c25864e7256f2959c2f6f59c4556, with uncommitted changes
- Rust: rustc 1.100.0 (0ef1bc386 2026-11-09)
- Host: Darwin 27.2.0 arm64, aarch64-apple-darwin
- Databases: sqlite
- Date: 2026-11-19T08:48:29Z
