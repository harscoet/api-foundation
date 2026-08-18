# api-foundation — Design Document

## Overview

`api-foundation` provides Rust primitives for building AIP-compliant `List` methods. It is not a gRPC framework and does not generate protobuf code.

Initial scope: `filter`, `order_by`, `pagination`, `list`.

---

## Normative sources

- [AIP-132](https://google.aip.dev/132) — standard List method
- [AIP-158](https://google.aip.dev/158) — pagination
- [AIP-160](https://google.aip.dev/160) — filtering
- [aip-160 crate](https://docs.rs/aip-160/0.1.5) v0.1.5 — Rust parser

---

## Target directory layout

```
src/
├── lib.rs
├── error.rs              — shared error types
├── filter.rs             — module root: re-exports, FilterableField trait
├── filter/
│   ├── typed.rs          — TypedFilter<F>, TypedExpression<F>
│   └── validate.rs       — conversion aip_160::Filter → TypedFilter<F>
├── order_by.rs           — module root: re-exports, OrderableField trait
├── order_by/
│   └── parse.rs          — order_by string parser
├── pagination.rs         — module root: re-exports
├── pagination/
│   ├── token.rs          — PageToken (encode/decode/verify)
│   └── page.rs           — Page<T>, PageRequest
├── list.rs               — module root: re-exports
└── list/
    └── query.rs          — ListQuery<F>
```

---

## Module `filter`

### Role of `aip-160`

The `aip-160` v0.1.5 crate is responsible for parsing the filter string. It produces:

```rust
// aip_160::ast
pub struct Filter { pub expression: Expression }
pub enum Expression {
    And(Box<Expression>, Box<Expression>),
    Or(Box<Expression>, Box<Expression>),
    Not(Box<Expression>),
    Restriction(Restriction),
    Sequence(Sequence),        // e.g. "user.name" with no operator
}
pub struct Restriction {
    pub field: String,         // "price", "user.name"
    pub comparator: Comparator,
    pub value: Value,
}
pub enum Comparator { Equal, NotEqual, GreaterThan, GreaterThanOrEqual, LessThan, LessThanOrEqual, Has }
pub enum Value { String(String), Number(f64), Boolean(bool), Null }
```

Entry point: `aip_160::parse_filter(input: &str) -> Result<Filter, FilterError>`.

### Known limitation of `aip-160`

AIP-160 specifies that **OR has higher precedence than AND**. The `aip-160` crate implements the opposite (AND binds tighter, standard convention). This divergence is documented and not corrected: it does not justify reimplementing the parser. Concrete APIs should document the effective behavior if they expose complex filters.

### `TypedFilter<F>`

After parsing, the AST is validated to produce a `TypedFilter<F>` where `F` is the concrete API's field enum. The `String` field in `Restriction` is replaced by `F`.

Rationale for this type alongside the `aip-160` AST:
1. Makes it impossible to use an invalid field
2. Simplifies repository-side mapping (match on `F` variants, not strings)
3. Enables per-field operator validation

```rust
pub struct TypedFilter<F> {
    pub expression: TypedExpression<F>,
    raw: String,   // original string for the consistency fingerprint
}

pub enum TypedExpression<F> {
    And(Box<TypedExpression<F>>, Box<TypedExpression<F>>),
    Or(Box<TypedExpression<F>>, Box<TypedExpression<F>>),
    Not(Box<TypedExpression<F>>),
    Restriction(TypedRestriction<F>),
}

pub struct TypedRestriction<F> {
    pub field: F,
    pub comparator: aip_160::Comparator,
    pub value: aip_160::Value,
}
```

`Sequence` (a field path with no operator) is treated as a validation error for now — the case is ambiguous in AIP-160 and uncommon in standard gRPC APIs.

### Trait `Field`

`FilterableField` and `OrderableField` are merged into a single `Field` trait:

```rust
pub trait Field: Sized + FromStr {
    /// Comparators allowed when this field appears in a filter expression.
    /// Default `&[]` makes the field non-filterable: any comparator is rejected.
    fn allowed_comparators(&self) -> &[aip_160::Comparator] { &[] }

    /// Whether this field may appear in an `order_by` clause.
    fn is_orderable(&self) -> bool { false }
}
```

`FromStr` is the bound for field-name parsing — strum's `#[derive(EnumString)]` with `#[strum(serialize_all = "snake_case")]` is the idiomatic implementation.

Validation steps:
1. Walk the `Expression` tree recursively
2. For each `Restriction`, call `F::from_str(&restriction.field)`
3. If `Err` → `Error::UnknownField` error
4. Verify the `Comparator` is in `allowed_comparators()` → `Error::DisallowedComparator` if not
5. Produce a `TypedExpression<F>`

---

## Module `order_by`

### Format (AIP-132)

```
"name asc, price desc, created_at"
```

- Separator: comma
- Direction is optional: `asc` (default) or `desc`
- Whitespace trimmed around each clause

### Types

```rust
pub struct OrderBy<F> {
    pub clauses: Vec<OrderClause<F>>,
}

pub struct OrderClause<F> {
    pub field: F,
    pub direction: Direction,
}

pub enum Direction { Asc, Desc }
```

Orderability is checked via `Field::is_orderable()` — unknown field names are rejected through the same `FromStr` bound as the filter.

### Relationship with keyset pagination

`OrderBy` directly determines the cursor shape. If the sort is `created_at DESC, id DESC`, the cursor must hold the values of `created_at` and `id` from the last returned row. `OrderBy` is the source of truth for cursor composition.

---

## Module `pagination`

### `PageRequest`

```rust
pub struct PageRequest {
    pub page_size: u32,   // 0 → default (50), >1000 → coerced to 1000 silently
}
```

AIP-158 rules:
- Negative `page_size` → `Error::InvalidPageSize(i32)`
- `page_size` > 1000 → coerce to 1000, no error
- `page_token` is managed by `ListQuery`, not `PageRequest`

### `PageToken` (internal representation)

```rust
// Serialized with bitcode, encoded as URL-safe base64
#[derive(bitcode::Encode, bitcode::Decode)]
struct RawPageToken {
    cursor: Vec<CursorEntry>,
    request_fingerprint: u64,  // deterministic hash of filter + order_by
    version: u8,               // for future format evolution
}

#[derive(bitcode::Encode, bitcode::Decode)]
struct CursorEntry {
    field_name: String,
    value: CursorValue,
}

#[derive(bitcode::Encode, bitcode::Decode)]
enum CursorValue {
    String(String),
    Int64(i64),
    Float64([u8; 8]),  // raw bits to avoid bitcode float limitations
    Bool(bool),
    Null,
}
```

#### Request fingerprint

The `request_fingerprint` detects token reuse with mismatched parameters (AIP-132: "all other parameters MUST match").

Construction: deterministic hash (not `std::hash`, which is not stable across processes) of the canonical representation `filter_raw + "\0" + order_by_raw`. Algorithm to be chosen during implementation (FNV-1a or SipHash with a fixed seed are candidates).

`page_size` is intentionally excluded from the fingerprint — AIP-158 does not require it and clients may legitimately adjust page size between pages.

#### Public token API

```rust
pub struct PageToken(/* opaque */);

impl PageToken {
    pub fn new(cursor: Vec<CursorEntry>, fingerprint: u64) -> Self { /* ... */ }
    pub fn encode(&self) -> String { /* bitcode + base64 */ }
    pub fn decode(s: &str) -> Result<Self> { /* base64 + bitcode */ }
    pub fn verify_fingerprint(&self, fingerprint: u64) -> Result<()> { /* ... */ }
    pub fn cursor(&self) -> &[CursorEntry] { /* ... */ }
}
```

### `Page<T>`

```rust
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_page_token: Option<String>,  // None means collection exhausted
    pub total_size: Option<i64>,          // optional; i64 matches diesel's COUNT return type
}
```

---

## Module `list`

### `ListQuery<F>`

```rust
pub struct ListQuery<F> {
    pub filter: Option<TypedFilter<F>>,
    pub order_by: Option<OrderBy<F>>,
    pub page_size: u32,
    pub cursor: Option<PageToken>,
    fingerprint: u64,  // private — access via .fingerprint()
}
```

### Construction from a gRPC request

The concrete API provides the raw strings from the protobuf message. `ListQuery` orchestrates validation:

```rust
impl<F: Field> ListQuery<F> {
    pub fn build(
        filter: Option<&str>,
        order_by: Option<&str>,
        page_size: i32,
        page_token: Option<&str>,
    ) -> Result<Self> { /* ... */ }

    pub fn fingerprint(&self) -> u64 { /* ... */ }
}
```

### Token/request consistency

Inside `build()`:
1. Parse and validate `filter` → `TypedFilter<F>`
2. Parse and validate `order_by` → `OrderBy<F>`
3. Build `PageRequest`
4. Compute FNV-1a fingerprint of `filter_raw + "\0" + order_by_raw`
5. If `page_token` is present:
   a. Decode the token
   b. Call `token.verify_fingerprint(fingerprint)` → `Error::PageTokenMismatch` on mismatch

---

## Errors

```rust
#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("invalid filter: {0}")]
    InvalidFilter(String),

    #[error("unknown field: {field:?}")]
    UnknownField { field: String },

    #[error("comparator '{comparator}' is not allowed for field '{field}'")]
    DisallowedComparator { field: String, comparator: String },

    #[error("invalid order_by: {0}")]
    InvalidOrderBy(String),

    #[error("page_size must be non-negative, got {0}")]
    InvalidPageSize(i32),

    #[error("invalid page_token: {0}")]
    InvalidPageToken(String),

    #[error("page_token does not match request parameters")]
    PageTokenMismatch,
}
```

All of these map to gRPC `INVALID_ARGUMENT`.

---

## Invariants

1. A `TypedFilter<F>` can only be constructed after successful validation — no unsafe constructor or `new_unchecked`.
2. A successfully decoded `PageToken` contains a cursor consistent with the `OrderBy` that created it.
3. `ListQuery::build()` is the sole public entry point — it is impossible to obtain an unvalidated `ListQuery`.
4. `Page<T>` with `next_page_token = None` signals collection exhaustion.

---

## Open questions / deferred decisions

| Question | Status |
|---|---|
| Merge `FilterableField` + `OrderableField` into `ApiField` | **Resolved** — single `Field` trait with `FromStr` bound |
| Hash algorithm for fingerprint (must be deterministic across processes) | **Resolved** — FNV-1a with standard constants |
| Token expiry (AIP-158 suggests ~3 days) | Out of initial scope; `version` field reserved for future use |
| `Sequence` handling (field path without operator) | Validation error for now |
| Wildcard `*` in string values (AIP-160) | Not supported by `aip-160` parser — out of scope |
| `skip` field (AIP-158) | Out of initial scope |

---

## Implementation plan

1. **`error.rs`** — error types + unit tests
2. **`order_by`** — string parser + validation + tests (no external dependencies)
3. **`filter`** — `FilterableField` trait + validation over `aip_160::Filter` + tests
4. **`pagination`** — `PageRequest` + `PageToken` (encode/decode/verify) + tests
5. **`list`** — `ListQuery::build()` + consistency check + integration tests
6. **`examples/`** — a complete `ProductField` example

Each increment must compile and have green tests before moving to the next.

---

## Current dependencies

| Crate | Usage |
|---|---|
| `aip-160` v0.1.5 | AIP-160 filter parser |
| `thiserror` v2.0 | Structured errors |
| `bitcode` v0.6 | PageToken serialization |
| `base64` v0.23 | PageToken encoding |
| `tokio` v1.53 | Async support for repositories |

Before adding any new dependency: verify maturity, license, MSRV compatibility, and necessity.
