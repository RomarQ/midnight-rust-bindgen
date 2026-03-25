# midnight-rust-bindgen -- Design Specification

Generate typed Rust bindings from Compact smart contract compiler output, enabling Rust services to deserialize and interact with Midnight contract ledger state without hardcoded field indices.

## Problem

The Compact compiler produces typed contract bindings for TypeScript only. Rust consumers must hardcode positional indices and manually reconstruct type mappings to access contract ledger state:

```rust
// Without bindings: fragile, no compile-time guarantees
const EGRESS_JOBS_INDEX: usize = 4;
let entries = view.get_map_entries(EGRESS_JOBS_INDEX)?;
```

There is no Midnight contract ABI standard. The compiler's metadata output (`contract-info.json`) omits ledger field declarations because it was designed solely for cross-contract type-checking. The TypeScript bindings get ledger info directly from the compiler's internal IR -- a path not available to external tools.

## Solution

```rust
// With bindings: type-safe, named access generated at compile time
midnight_bindgen::contract!(Gateway, "compiled/gateway/compiler/contract-info.json");

use gateway::{Gateway, EgressJob, JobStatus};

let ledger = Gateway::from_hex(&hex_state)?;
let threshold: u8 = ledger.threshold()?;

for result in ledger.egress_jobs()?.iter() {
    let (_key, job) = result?;
    println!("job {}: amount={}, status={:?}", job.id, job.amount, job.status);
}
```

Two components:

1. **Compiler extension** ([fork](https://github.com/RomarQ/compact/tree/feat/ledger-in-contract-info)) -- extends `contract-info.json` with a `"ledger"` array containing field names, path indices, types, storage kinds, and an `"exported"` flag
2. **Rust bindgen** -- reads the enriched JSON and generates typed Rust code via proc macro or CLI

## Prior Art

| Ecosystem | Tool | Approach |
|-----------|------|----------|
| Ethereum | `alloy` `sol!` | Generate Rust bindings from Solidity ABI JSON via `quote!` |
| Substrate | `subxt` | Generate Rust types from chain metadata |
| Protobuf | `prost` / `tonic` | Generate Rust from `.proto` definitions |
| GraphQL | `graphql-client` | Generate Rust from `.graphql` schemas |

Our architecture follows alloy's `sol!` pattern: JSON input -> IR -> `quote!`-based TokenStream -> Rust types.

---

## Architecture

```
midnight-bindgen                        <- user-facing: one dependency
+-- midnight-bindgen-macro              <- proc macro: reads JSON, calls codegen
+-- midnight-bindgen-runtime            <- runtime: StateError, MapAccessor, Bytes, re-exports
|   +-- depends on midnight-ledger      <- same crates that power the Midnight node
+-- compact-codegen                     <- code generation library + CLI
```

### Crate responsibilities

**`midnight-bindgen`** -- Public facade. Re-exports the `contract!` macro and all runtime types. Users add only this dependency.

**`midnight-bindgen-macro`** -- Proc macro crate. Parses macro input (path + optional name), reads JSON from disk, calls `compact_codegen::generate_bindings_from_json()` to get a `TokenStream`, wraps it in an optional module. Uses `include_bytes!` on the JSON file so that changes to `contract-info.json` trigger recompilation automatically.

**`compact-codegen`** -- Code generation library + CLI binary. Parses `contract-info.json` into `ContractInfo` IR, generates Rust code as `proc_macro2::TokenStream` using `quote!`. The CLI formats the output via `prettyplease` and writes a standalone Cargo crate.

**`midnight-bindgen-runtime`** -- Runtime support for generated code. Provides `StateError`, typed accessors (`MapAccessor`, `SetAccessor`, `ListAccessor`, `MerkleTreeAccessor`), `Bytes<N>` newtype, navigation helpers (`cell_value`, `get_field`, `get_field_path`, `variant_name`), and re-exports midnight-ledger types needed by generated code (`Aligned`, `Alignment`, `AlignedValue`, `ValueSlice`, `InvalidBuiltinDecode`, `ContractState`, `StateValue`, `InMemoryDB`, `tagged_deserialize`, `TransientFr`, `EmbeddedGroupAffine`, `MerkleTree`, `MerkleTreeDigest`, etc.). Also re-exports `hex` so generated `from_hex` code works without an additional dependency. The `lazy` module provides the `StateQueryProvider` trait, `ContractError`, `StateQuery`/`StateQueryResult` types, and helpers (`index_to_query_key`, `build_query_path`, `decode_state_value`) for per-field RPC queries.

### Code generation pipeline

```
contract-info.json (file on disk)
        |
  serde_json::from_str()
        |
  ContractInfo { circuits, witnesses, ledger }     <- IR (types.rs)
        |
  EmitCtxt::expand()                               <- codegen (expand/mod.rs)
  +-- emit_field_constants()     -> quote! { pub const FIELD_X: usize = N; }
  |                                 or:   { pub const FIELD_X: &[usize] = &[N, M]; }
  +-- emit_data_types()          -> quote! { pub struct X { ... } impl Aligned ... impl TryFrom<&ValueSlice> ... }
  +-- emit_circuit_types()       -> quote! { pub struct XCall { ... } pub type XReturn = T; pub enum Calls { ... } }
  +-- emit_ledger_wrapper()      -> quote! { pub struct Gateway { ... } impl Gateway { from_hex, accessors } }
  +-- emit_lazy_ledger_wrapper() -> quote! { pub struct GatewayQuery<P> { ... } impl { async accessors } }
        |
  proc_macro2::TokenStream
        |
  +-- Proc macro path: returned directly to the compiler (no string roundtrip)
  +-- CLI path: syn::parse2 -> prettyplease::unparse -> formatted .rs file
```

### Why `quote!` instead of string concatenation

Follows the alloy `sol!` pattern. Benefits:

- **Type safety** -- `quote!` catches syntax errors at compile time of the generator
- **No roundtrip** -- proc macro path avoids `String -> parse -> TokenStream`
- **Keyword handling** -- `Ident::new_raw` handles all Rust keywords automatically
- **Readability** -- generated code reads like the code it produces
- **Maintainability** -- adding new type mappings is a `quote!` block, not string surgery

---

## Compact Compiler Deep Dive

### How the compiler works

The Compact compiler (`compactc`) is written in **Chez Scheme** using the **nanopass framework** -- a chain of 19+ intermediate representations where each pass transforms one IR into the next.

Key files in the compiler:

| File | Purpose |
|------|---------|
| `compiler/passes.ss` | Pipeline orchestration |
| `compiler/langs.ss` | All 19+ intermediate language definitions |
| `compiler/analysis-passes.ss` | Module expansion, type inference, `determine-ledger-paths` |
| `compiler/save-contract-info-passes.ss` | Emits `contract-info.json` |
| `compiler/typescript-passes.ss` | Emits TypeScript bindings |

### IR pipeline

```
Lsrc -> ... -> Lwithpaths -> Lnodisclose --+-> save-contract-info-passes -> contract-info.json
                                           +-> typescript-passes         -> index.js + index.d.ts
                                           +-> circuit/zkir passes       -> ZKIR + keys
```

**Ledger field declarations** flow through the IR as `Public-Ledger-Binding` nodes carrying `(field-name, path-index*, type)`. The `determine-ledger-paths` pass assigns positional indices based on source declaration order.

### Why `contract-info.json` omits ledger fields

The file's purpose is cross-contract type-checking, which only needs circuit and witness signatures. Ledger fields were not included because they are not needed for this use case. There is no deliberate privacy concern.

### What we extended

Our [fork](https://github.com/RomarQ/compact/tree/feat/ledger-in-contract-info) modifies a single file (`save-contract-info-passes.ss`) to also serialize `Public-Ledger-Binding` nodes into a `"ledger"` array. Each entry includes an `"exported"` flag indicating whether the field was declared with `export ledger` in the Compact source. The extension is backward-compatible -- existing consumers ignore the new key. All 742 compiler tests pass.

---

## Ledger schema

A new top-level `"ledger"` array in `contract-info.json`:

```json
{
  "ledger": [
    {
      "name": "threshold",
      "index": 0,
      "exported": true,
      "storage": "cell",
      "type": { "type-name": "Uint", "maxval": 255 }
    },
    {
      "name": "egress_jobs",
      "index": 4,
      "exported": true,
      "storage": "map",
      "key-type": { "type-name": "Field" },
      "value-type": {
        "type-name": "Struct",
        "name": "EgressJob",
        "elements": [
          { "name": "id", "type": { "type-name": "Uint", "maxval": 340282366920938463463374607431768211455 } },
          { "name": "status", "type": { "type-name": "Enum", "name": "JobStatus", "elements": ["pending", "completed"] } }
        ]
      }
    },
    {
      "name": "next_job_id",
      "index": 3,
      "exported": false,
      "storage": "counter",
      "type": { "type-name": "Uint", "maxval": 18446744073709551615 }
    }
  ]
}
```

### Storage kinds

| Compact declaration | `storage` | Extra JSON fields |
|---|---|---|
| `ledger x: Uint<N>` / `Bytes<N>` / `Boolean` | `"cell"` | `type` |
| `ledger x: Counter` | `"counter"` | `type` (always Uint<64>) |
| `ledger x: Map<K, V>` | `"map"` | `key-type`, `value-type` |
| `ledger x: Set<T>` | `"set"` | `element-type` |
| `ledger x: List<T>` | `"list"` | `element-type` |
| `ledger x: MerkleTree<T>` | `"merkle-tree"` | `element-type`, `depth` |

### Path indices

For contracts with 15 or fewer ledger fields, `"index"` is a simple 0-based integer (`FieldIndex::Single`). For contracts with more than 15 fields, the compiler uses B-tree batching and the index becomes a JSON array of integers (e.g., `[1, 3]`), parsed as `FieldIndex::Path`. The generated code handles this transparently -- field constants become `pub const FIELD_X: &[usize] = &[1, 3];` and accessors use `get_field_path` instead of `get_field`.

### Exported flag

Each ledger field carries `"exported": true/false`, indicating whether it was declared with `export ledger` in the Compact source. Non-exported fields are still on-chain and accessible; the flag records the original declaration intent.

---

## Type mapping

| `contract-info.json` type | Rust type | Notes |
|---|---|---|
| `Uint` maxval <= 255 | `u8` | |
| `Uint` maxval <= 65535 | `u16` | |
| `Uint` maxval <= 2^32-1 | `u32` | |
| `Uint` maxval <= 2^64-1 | `u64` | |
| `Uint` maxval <= 2^128-1 | `u128` | |
| `Uint` maxval > 2^128 | `Vec<u8>` | Fallback for very large values |
| `Field` | `TransientFr` | BLS12-381 scalar field element |
| `Bytes<N>` | `Bytes<N>` | Newtype over `[u8; N]` (see below) |
| `Boolean` | `bool` | |
| `Vector<N, T>` | `[T; N]` | Fixed-size array |
| `Tuple(types)` | `(T, U, ...)` | Single-element: `(T,)`, empty: `()` |
| `Struct { fields }` | Generated struct | + `Aligned` + `TryFrom<&ValueSlice>` impl |
| `Enum { variants }` | `#[repr(u8)]` enum | + `Aligned` + `TryFrom<&ValueSlice>` impl |
| `Alias(name, T)` | Transparent | Delegates to inner type |
| `Counter` storage | `u64` | Always Uint<64> per midnight-ledger |
| `Map<K, V>` storage | `MapAccessor<K, V>` | Lazy accessor (see below) |
| `Set<T>` storage | `SetAccessor<T>` | Lazy accessor (see below) |
| `List<T>` storage | `ListAccessor<T>` | Lazy accessor (see below) |
| `MerkleTree<D, T>` storage | `MerkleTreeAccessor` | Structural access (see below) |
| `HistoricMerkleTree<D, T>` storage | `MerkleTreeAccessor` | Same layout as MerkleTree |
| `Opaque("JubjubPoint")` | `EmbeddedGroupAffine` | On-curve validated |
| `Opaque("Scalar<BLS12-381>")` | `TransientFr` | BLS12-381 scalar |
| `Opaque` (unknown) | `Vec<u8>` | Fallback |
| `Contract { name }` | `Vec<u8>` | Contract reference |
| Unknown `type-name` | `Vec<u8>` | Forward-compatible catch-all (`#[serde(other)]`) |

### The `Bytes<N>` newtype

Midnight-ledger provides `Aligned for [u8; N]` and `TryFrom<ValueAtom> for [u8; N]`, but not `TryFrom<&ValueSlice> for [u8; N]`. Since generated struct deserialization uses tuple decomposition from `ValueSlice`, raw `[u8; N]` cannot participate directly.

The runtime provides `Bytes<N>` -- a newtype wrapper over `[u8; N]` that:

- Implements `Aligned` (delegates to `[u8; N]`)
- Implements `TryFrom<&ValueSlice>` (extracts a single atom, converts to `[u8; N]`)
- Implements `Deref<Target = [u8; N]>`, `AsRef<[u8]>`, `From`/`Into` conversions
- Implements `From<Bytes<N>> for ValueAtom` (for key lookups in `MapAccessor`/`SetAccessor`)
- Provides `into_inner()` to unwrap
- Has hex-based `Debug` and `Display` formatting

### `Maybe<T>` as a product type

Compact's `Maybe<T>` is emitted as a `Struct` with fields `[is_some: Boolean, value: T]`. The code generator detects this pattern and adds an `into_option()` convenience method:

```rust
pub struct Maybe {
    pub is_some: bool,
    pub value: T,
}

impl Maybe {
    pub fn into_option(self) -> Option<T> {
        if self.is_some { Some(self.value) } else { None }
    }
}
```

This is not `Option<T>` directly because midnight-ledger's on-chain representation always stores both fields, and `Aligned` + `TryFrom<&ValueSlice>` must handle the full product type.

---

## Deserialization strategy: `Aligned` + `TryFrom<&ValueSlice>`

Generated types implement midnight-ledger's native traits directly rather than going through custom runtime traits:

- **`Aligned`** -- declares the type's alignment (how many atoms it occupies and their structure). For structs, alignment is the concatenation of each field's alignment. For enums, alignment matches `u8`.
- **`TryFrom<&ValueSlice>`** -- deserializes from a slice of value atoms using **tuple decomposition**. The generated code destructures the `ValueSlice` into a tuple of field types in one step, then constructs the struct.

Example generated code for a struct:

```rust
impl Aligned for EgressJob {
    fn alignment() -> Alignment {
        Alignment::concat([
            &<u128 as Aligned>::alignment(),
            &<Bytes<32> as Aligned>::alignment(),
            &<JobStatus as Aligned>::alignment(),
        ])
    }
}

impl<'a> TryFrom<&'a ValueSlice> for EgressJob {
    type Error = InvalidBuiltinDecode;

    fn try_from(vs: &'a ValueSlice) -> Result<Self, Self::Error> {
        let (id, destination, status): (u128, Bytes<32>, JobStatus) = vs.try_into()?;
        Ok(Self { id, destination, status })
    }
}
```

For enums:

```rust
impl Aligned for JobStatus {
    fn alignment() -> Alignment {
        <u8 as Aligned>::alignment()
    }
}

impl<'a> TryFrom<&'a ValueSlice> for JobStatus {
    type Error = InvalidBuiltinDecode;

    fn try_from(vs: &'a ValueSlice) -> Result<Self, Self::Error> {
        let v: u8 = vs.try_into()?;
        match v {
            0 => Ok(JobStatus::Pending),
            1 => Ok(JobStatus::Completed),
            _ => Err(InvalidBuiltinDecode("invalid JobStatus variant")),
        }
    }
}
```

This approach avoids custom runtime traits entirely. Primitive types (`bool`, `u8`-`u128`, `TransientFr`, `EmbeddedGroupAffine`) already implement `Aligned` + `TryFrom<&ValueSlice>` in midnight-ledger. The `Bytes<N>` newtype bridges the gap for fixed-size byte arrays.

---

## Accessor types

### `MapAccessor<K, V>`

Wraps a reference to midnight-ledger's `HashMap<AlignedValue, StateValue<InMemoryDB>, InMemoryDB>`. Provides typed access without copying the underlying storage.

| Method | Bounds on K/V | Description |
|---|---|---|
| `get(key)` | `K: Into<AlignedValue>`, `V: TryFrom<&ValueSlice>` | Lookup by key. Returns `Option<Result<V, StateError>>` |
| `iter()` | `K: TryFrom<&ValueSlice>`, `V: TryFrom<&ValueSlice>` | Iterate all pairs. Returns `impl Iterator<Item = Result<(K, V), StateError>>` |
| `contains_key(key)` | `K: Into<AlignedValue>` | Membership check |
| `size()` | -- | Number of entries |
| `is_empty()` | -- | Whether the map has zero entries |

### `SetAccessor<T>`

Wraps the same `HashMap` type (sets are maps with `Null` values in midnight-ledger).

| Method | Bounds on T | Description |
|---|---|---|
| `contains(key)` | `T: Into<AlignedValue>` | Membership check |
| `iter()` | `T: TryFrom<&ValueSlice>` | Iterate all elements. Returns `impl Iterator<Item = Result<T, StateError>>` |
| `size()` | -- | Number of elements |
| `is_empty()` | -- | Whether the set has zero elements |

### `ListAccessor<T>`

Wraps `Array<StateValue<InMemoryDB>, InMemoryDB>` where each element is a `StateValue::Cell` containing the serialized value.

| Method | Bounds on T | Description |
|---|---|---|
| `get(index)` | `T: TryFrom<&ValueSlice>` | Get element at index. Returns `Option<Result<T, StateError>>` |
| `iter()` | `T: TryFrom<&ValueSlice>` | Iterate all elements. Returns `impl Iterator<Item = Result<T, StateError>>` |
| `len()` | -- | Number of elements |
| `is_empty()` | -- | Whether the list has zero elements |

### `MerkleTreeAccessor`

Structural accessor for merkle tree fields. Not parameterized by `T` because on-chain merkle trees store only leaf hashes, not the original typed values.

Both `MerkleTree<D, T>` and `HistoricMerkleTree<D, T>` share the same on-chain layout: a compound `StateValue::Array` with 3 elements:
- `[0]`: `StateValue::BoundedMerkleTree` — the live tree
- `[1]`: `StateValue::Cell(u64)` — `first_free` index counter
- `[2]`: `StateValue::Map` — history set (root hashes → Null, used by `HistoricMerkleTree`)

| Method | Description |
|---|---|
| `from_state(sv)` | Construct from the compound `StateValue::Array` |
| `height()` | Tree height (depth) |
| `first_free()` | Index of the next free slot |
| `root()` | Root hash (`Option<MerkleTreeDigest>`) |

---

## Navigation helpers

The runtime provides functions for traversing `StateValue` trees:

| Function | Purpose |
|---|---|
| `cell_value(sv)` | Extract `&AlignedValue` from a `StateValue::Cell` |
| `get_field(sv, index)` | Index into a `StateValue::Array` by position |
| `get_field_path(sv, &[indices])` | Navigate nested arrays by a path of indices |
| `variant_name(sv)` | Return the variant name of a `StateValue` (for error messages) |

The ledger wrapper's `from_hex` method uses `hex::decode` + `tagged_deserialize` to reconstruct a `ContractState<InMemoryDB>` from a hex-encoded string. Each field accessor calls `get_field` or `get_field_path` on `state.data` to navigate to the correct `StateValue`, then pattern-matches on the storage kind (Cell, Map, etc.).

---

## Lazy state queries

The eager approach (`from_hex`) downloads the entire contract state blob and deserializes it in memory. For contracts with large state (many map entries, deep merkle trees), this is O(n) in state size even when the caller only needs one field.

The lazy approach generates a `{Name}Query<P>` struct alongside the eager one. Each accessor makes an individual `query_contract_state` RPC call to the node, fetching only the requested field -- O(log n) per query.

### `StateQueryProvider` trait

Defined in `midnight_bindgen::lazy`, this is the only trait downstream providers need to implement:

```rust
pub trait StateQueryProvider: Send + Sync {
    type Error: std::error::Error + Send + Sync + 'static;

    async fn query_contract_state(
        &self,
        address: &str,
        queries: Vec<StateQuery>,
    ) -> Result<Vec<StateQueryResult>, Self::Error>;
}
```

The trait has no dependencies outside of `std` -- it uses native async (no `async_trait` macro). Downstream crates (e.g., `midnight-provider` in `midnight-rs`) implement it for their concrete provider types.

### `StateQuery` and `StateQueryResult`

```rust
pub struct StateQuery {
    pub path: Vec<String>,  // hex-encoded serialized AlignedValue keys
}

pub struct StateQueryResult {
    pub query: StateQuery,
    pub value: Option<String>,  // hex-encoded tagged-serialized StateValue
    pub error: Option<String>,
}
```

### `ContractError`

```rust
pub enum ContractError {
    Provider(Box<dyn std::error::Error + Send + Sync>),
    State(StateError),
    QueryFailed(String),
    NoValue,
}
```

### Path encoding

Field indices are converted to hex-encoded serialized `AlignedValue` keys at runtime via `index_to_query_key(index: usize)`. This uses `AlignedValue::from(index as u8)` + `Serializable::serialize` + `hex::encode`. Known values:

| Index | Hex key |
|-------|---------|
| 0 | `4001` |
| 1 | `0101` |
| 2 | `0201` |

For B-tree path indices (>15 fields), `build_query_path(&[usize])` converts each level to a hex key.

### Generated lazy accessor flow

```
GatewayQuery::threshold(&self).await
    |
    build_query_path(&[FIELD_THRESHOLD])        -> vec!["4001"]
    |
    provider.query_contract_state(address, queries).await
    |
    decode_state_value(&result[0])              -> StateValue<InMemoryDB>
    |
    cell_value(&sv)                             -> &AlignedValue
    |
    u8::try_from(&*av.value)                    -> u8
```

### Supported storage kinds (lazy)

| Storage | Lazy accessor | Signature |
|---------|--------------|-----------|
| `cell` | Read value | `field() -> Result<T, ContractError>` |
| `counter` | Read value | `field() -> Result<u64, ContractError>` |
| `map` | Lookup by key | `field(key: impl Into<AlignedValue>) -> Result<Option<V>, ContractError>` |
| `set` | Membership check | `field(key: impl Into<AlignedValue>) -> Result<bool, ContractError>` |
| `list` | Get by index | `field(index: usize) -> Result<Option<T>, ContractError>` |
| `merkle-tree` | Not supported | Tree structure doesn't map to single-value RPC queries |

Map lookups return `None` when the key is not found (the RPC returns no value and no error). Set membership returns `true` if the key exists (sets store `Null` values for present keys). List access returns `None` for out-of-bounds indices.

### No indexer required

Lazy queries use the node's `midnight_queryContractState` RPC directly. Unlike the eager path (which fetches the full hex state from the indexer's GraphQL API), lazy queries only need a node WebSocket connection.

---

## `StateError`

A unified error type for all deserialization and navigation failures:

```rust
pub enum StateError {
    IndexOutOfBounds(usize),
    UnexpectedVariant { expected: &'static str, actual: &'static str },
    Conversion(InvalidBuiltinDecode),
    Deserialize(std::io::Error),
    HexDecode(String),
}
```

- `IndexOutOfBounds` -- field index exceeds the state array size
- `UnexpectedVariant` -- expected `Cell` but got `Map`, etc.
- `Conversion` -- midnight-ledger's `TryFrom<&ValueSlice>` failed (wrong atom count, invalid data)
- `Deserialize` -- `tagged_deserialize` failed (corrupt or truncated hex state)
- `HexDecode` -- input string is not valid hex

---

## What gets generated

For a contract (e.g., MCS gateway with 10 ledger fields, 3 structs, 2 enums, 6 circuits):

1. **Field constants** -- `pub const FIELD_THRESHOLD: usize = 0;` (or `pub const FIELD_X: &[usize] = &[1, 3];` for >15 fields)
2. **Data structs** -- `pub struct EgressJob { pub id: u128, pub destination: Bytes<32>, ... }`
3. **Enums** -- `#[derive(Debug, Clone, Copy, PartialEq, Eq)] #[repr(u8)] pub enum JobStatus { Pending = 0, Completed = 1 }`
4. **Alignment** -- `impl Aligned for EgressJob` (field alignment concatenation)
5. **Deserialization** -- `impl TryFrom<&ValueSlice> for EgressJob` (tuple decomposition)
6. **Circuit types** -- `pub struct WithdrawCall { ... }`, `pub type WithdrawReturn = u128;`, `pub enum Calls { ... }`
7. **Ledger wrapper** -- `pub struct Gateway` with `new(state)`, `from_hex(hex)`, and typed accessor methods per field
8. **Lazy query wrapper** -- `pub struct GatewayQuery<P: StateQueryProvider>` with async accessor methods for cell/counter fields
9. **Maybe helpers** -- `into_option()` method on structs matching the `Maybe<T>` pattern

### Scope: read-only state access

The tool generates **read-only deserialization** bindings in two modes:
- **Eager** -- `{Name}::from_hex()` downloads the full state blob and provides synchronous typed accessors for all storage kinds
- **Lazy** -- `{Name}Query<P>` makes per-field async RPC calls to the node, supporting cell and counter fields

It does NOT generate:
- Transaction construction or submission code
- Proof generation or verification
- Serialization (Rust -> on-chain state)
- Circuit call wrappers

---

## Identifier handling

Compact uses `$` in witness and circuit names (e.g., `vote$commit`, `private$secretKey`). Since `$` is not valid in Rust identifiers, the code generator replaces it with `_` via `sanitize_ident()`. Other invalid characters are similarly replaced. Rust reserved keywords are emitted using raw identifier syntax (`r#type`, `r#match`, etc.), including edition 2024 additions like `gen`.

Pascal case conversion (`to_pascal_case`) treats `_`, `-`, and `$` as word boundaries.

---

## Witness serde

The Compact compiler emits `"result-type"` in circuit entries but `"result type"` (with a space) in some witness entries. The `Witness` struct handles this via a serde alias:

```rust
#[serde(rename = "result-type", alias = "result type")]
pub result_type: TypeNode,
```

---

## Proc macro: file change tracking

The proc macro emits an `include_bytes!` statement pointing at the `contract-info.json` file:

```rust
const _: &[u8] = include_bytes!("/absolute/path/to/contract-info.json");
```

This causes `rustc` to track the file as a dependency. When the JSON changes, the macro is re-expanded and the bindings are regenerated. This is a standard technique (used by `include_str!` / `include_bytes!` in proc macros) that works without nightly `proc_macro::tracked_path`.

---

## Midnight-ledger building blocks

The `midnight-ledger` crates are referenced via the project's `flake.nix`, which points to the midnight-ledger fork. This avoids hardcoding a git rev in `Cargo.toml`.

| Type / Function | Crate | Purpose |
|---|---|---|
| `ContractState<InMemoryDB>` | `midnight-onchain-state` | Root state container |
| `StateValue<D>` enum | `midnight-onchain-state` | `Cell`, `Map`, `Array`, `Null`, `BoundedMerkleTree` |
| `AlignedValue` | `midnight-base-crypto` | Cell data: aligned value with `Value` inside |
| `ValueSlice` | `midnight-base-crypto` | Slice of `ValueAtom`s for tuple decomposition |
| `ValueAtom` | `midnight-base-crypto` | Single serialization atom |
| `Aligned` trait | `midnight-base-crypto` | Declares a type's atom alignment |
| `Alignment` | `midnight-base-crypto` | Alignment descriptor, supports `concat` |
| `InvalidBuiltinDecode` | `midnight-base-crypto` | Error type for `TryFrom<&ValueSlice>` failures |
| `HashMap<K,V,D>` | `midnight-storage` | Map/Set storage, provides `get`, `contains_key`, `iter`, `keys`, `size` |
| `Array<V,D>` | `midnight-storage` | Positional field storage, provides `get(index)` |
| `tagged_deserialize` | `midnight-serialize` | Binary -> `ContractState` |
| `EmbeddedGroupAffine` | `midnight-transient-crypto` | Jubjub curve point |
| `Fr` (as `TransientFr`) | `midnight-transient-crypto` | BLS12-381 scalar field element |

---

## Open Questions

1. **Contract evolution** -- Ledger fields can only be appended (never reordered) in Compact. Generated code should be versioned alongside the contract checksum.

2. **Upstream acceptance** -- If `LFDT-Minokawa/compact` does not accept the PR, we maintain the fork. The change is small and low-conflict.

3. **Circuit call / transaction construction** -- The current scope is read-only. Building and submitting transactions is a separate concern.

## References

- [Compact language reference](https://docs.midnight.network/develop/reference/compact/lang-ref)
- [Compact compiler](https://github.com/LFDT-Minokawa/compact)
- [midnight-ledger](https://github.com/midnightntwrk/midnight-ledger) -- Rust ledger crates
- [midnight-zk](https://github.com/midnightntwrk/midnight-zk) -- Curve types (`midnight-curves`)
- [alloy sol! macro](https://github.com/alloy-rs/core) -- Architecture inspiration
