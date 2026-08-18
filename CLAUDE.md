# api-foundation — Claude Code Context

## Goal

Rust crate providing reusable primitives for building gRPC/Protobuf APIs conforming to Google AIPs (filtering, ordering, pagination, List composition). This is a **foundation library**, not a gRPC framework.

Out of scope: gRPC server, tonic, protobuf codegen, SQL, ORM, field masks, resource names, caching, business logic.

---

## Architecture

```
api-foundation/
└── src/
    ├── lib.rs
    ├── error.rs          — shared error types
    ├── filter/           — AIP-160 parsing + field/operator validation → TypedFilter<F>
    ├── order_by/         — "field asc, field desc" parsing + validation → OrderBy<F>
    ├── pagination/       — PageRequest, PageToken, Page<T>
    └── list/             — ListQuery<F>: composes filter + order_by + pagination
```

The empty stub files `filter.rs`, `order.rs`, `pagination.rs` at the root of `src/` should be migrated to submodules during implementation.

---

## Module responsibilities

| Module | Responsibility |
|---|---|
| `filter` | Integrate `aip-160`, validate fields and operators, produce `TypedFilter<F>` |
| `order_by` | Parse the `order_by` string, validate orderable fields, produce `OrderBy<F>` |
| `pagination` | Encode/decode `PageToken` (bitcode + base64), `PageRequest`, `Page<T>` |
| `list` | `ListQuery<F>` composing all three, token/request consistency check |

---

## `Field` trait

Concrete APIs define an enum of allowed fields and implement the `Field` trait:

```rust
enum ProductField { Name, Type, Price, CreatedAt }
```

The trait must support:
- Mapping `&str → Result<F, _>` via the `FromStr` bound (strum's `EnumString` is the idiomatic derive)
- Declaring allowed comparators per field (`allowed_comparators` — default `&[]` = non-filterable)
- Declaring whether a field is orderable (`is_orderable` — default `false`)

---

## `ListQuery<F>`

Main entry point. Built from the gRPC request:

```rust
let query = ListQuery::build(...)?;
let page = repository.list(query).await?;
```

Contains:
- `Option<TypedFilter<F>>`
- `Option<OrderBy<F>>`
- `PageRequest`

The repository receives an already-validated `ListQuery<F>`. It does not re-implement parsing, validation, token encoding, or filter/order_by/page_token consistency.

---

## `TypedFilter<F>`

Typed AST produced after validating `aip_160::Filter`. Replaces `field: String` with `field: F`. Rationale: makes it impossible to use an invalid field; simplifies repository-side mapping (match on `F` variants, not strings).

---

## PageToken — fundamental invariant

A token is bound to the request that generated it. It contains:
- a keyset cursor (field values from the last returned row)
- a consistency fingerprint (deterministic hash of canonical `filter` + `order_by`)

If the fingerprint does not match the current request → `INVALID_ARGUMENT`.

Internal encoding: `bitcode` (serialization) + URL-safe `base64`. Opaque to the client.

---

## `aip-160` integration

- `aip-160` is responsible for **parsing** the AIP-160 filter string
- `api-foundation` is responsible for **validation** (fields, operators, types) and integration into `ListQuery`
- Do not reimplement the parser
- Known limitation: `aip-160` gives AND higher precedence than OR; AIP-160 spec says the opposite. Document this; do not fix without a concrete reason.

---

## Keyset pagination

Do not use SQL `OFFSET`. The cursor encodes the field values of the last returned row for each sort field. Example:

```
ORDER BY created_at DESC, id DESC
cursor: { created_at: T, id: X }
```

The core crate remains backend-independent. Future adapters (`api-foundation-sqlx`, etc.) are out of scope.

---

## Development rules

- Add a dependency only when necessary (check maturity, license, MSRV)
- Run `cargo fmt`, `cargo test`, `cargo clippy` after each increment
- No panics on user input
- Structured errors with `thiserror`
- Minimal public API — strongly typed, invariants in types
- No premature generic abstractions
- **Consult relevant AIPs before any API decision**

---

## Test conventions

Cover: valid parsing, invalid parsing, invalid fields, invalid operators, corrupted tokens, token/request mismatch, keyset cursors, multi-field ordering. Use property-based tests where they add real value (parsers, tokens). Error cases are as important as happy paths.

---

## AIP references

- AIP-132: standard List method (page_size default 50, max 1000, coerce silently)
- AIP-158: pagination (opaque tokens, ~3-day expiry acceptable, skip optional)
- AIP-160: filtering (grammar, operators, field validation)
- https://google.aip.dev/

---

## Guiding principle

> A developer should be able to implement a new AIP-compliant List method with very little API-specific code, while being protected against common parsing, validation, ordering, and pagination mistakes.
