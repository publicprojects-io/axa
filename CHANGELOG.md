# Changelog

All notable changes to this project are documented in this file. The format
is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added

- `axa-paseto`, for PASETO bearer authentication and ABAC enforcement, behind the
  `paseto` feature. Signature checks stay in `authentication-verifier`.
- `axa-redis`, a Redis cache with mandatory expiry, behind the `redis` feature.
- `axa-openapi`, for OpenAPI documents and a Swagger UI pinned to one exact
  version with integrity hashes, behind the `openapi` feature.
- `axa-seaorm`, behind the `sea-orm` feature, which also holds the code formerly
  in `axa::model` and `axa::schema`. Those paths still work. It adds Loco's
  `query::condition()` and `Validatable`, so moving from Loco needs less rewriting.
- A guide to moving SeaORM code from Loco.

## [0.1.0] - 2026-11-05

### Added

- Routes, middleware, health checks, errors, configuration, logging, graceful
  shutdown, the shared command line, tasks, background jobs, and a Postgres job
  queue, plus test helpers and transitional SeaORM support.
- Subcrates, each behind a feature of the same purpose: `axa-sqlx` for pooling,
  migrations, and test resets; `axa-pagination`; `axa-problem` for RFC 9457
  errors; `axa-prometheus`; and `axa-versioning`.
