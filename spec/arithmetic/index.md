# Arithmetic

Math must not panic, and must not wrap. This page is part of the [specification](../index.md),
and the source of truth for how `axa` does arithmetic.

An overflow panics in a debug build and wraps silently in a release build. A wrapped
value is worse than a panic: a counter, a size, a deadline, or a rate that wraps is a
wrong answer that nothing reports. A panic in a request handler or a worker kills the
task. So arithmetic on values that can come from outside the code, or that can grow
without bound, is checked, and the caller decides what a failed operation means.

## Rule

- Use the Rust `checked_*` functions: `checked_add`, `checked_sub`, `checked_mul`,
  `checked_div`, `checked_rem`, `checked_neg`, `checked_pow`, `checked_shl`, and
  `checked_shr`.
- Each returns `None` when the operation would overflow, underflow, divide by zero, or
  otherwise be undefined. It never panics and never wraps.
- Do not use the bare operators (`+`, `-`, `*`, `/`, `%`, unary `-`) on such values.
  Do not use `wrapping_*`, `overflowing_*`, or `as` casts that truncate. Use
  `try_from` and `try_into`, which return an error, in place of narrowing `as` casts.
- Indexing and slicing that can fall out of range use `get`, which returns `None`.
- A `None` is handled where it arises: return an error, apply a stated limit, or
  fall back to a documented value. Do not `unwrap` or `expect` it.

`saturating_*` is allowed only where clamping to the limit is the stated behaviour,
for example a backoff delay capped at its maximum. The spec row for that behaviour says
so.

## Example

```rust
assert_eq!(255_u8.checked_add(1), None);    // overflow: None, not a panic or a wrap to 0
assert_eq!(250_u8.checked_add(5), Some(255));
assert_eq!(0_u8.checked_sub(1), None);      // underflow
assert_eq!(10_u32.checked_div(0), None);    // divide by zero

// Handled where it arises.
let total = a.checked_add(b).ok_or(Error::Overflow)?;
```

## Requirements

| Id | Requirement | Test |
|---|---|---|
| ARI-1 | Arithmetic on externally supplied or unbounded values uses `checked_*` and returns `None` on failure, never panicking or wrapping | **unverified**: an audit of the existing bare operators has not been done |
| ARI-2 | A `None` from a checked operation is handled, as an error, a stated limit, or a documented fallback, and never `unwrap`ped | **unverified** |
| ARI-3 | Narrowing conversions use `try_from` or `try_into`, not a truncating `as` | **unverified** |
