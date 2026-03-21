# midnight-rust-bindgen

Generate typed Rust bindings from [Midnight](https://midnight.network) [Compact](https://docs.midnight.network/compact) smart contracts.

```rust
// One line generates all types at compile time
midnight_bindgen::contract!(Gateway, "compiled/gateway/compiler/contract-info.json");
```

```rust
// Typed, named access to contract state
use gateway::{Gateway, EgressJob, JobStatus};

let ledger = Gateway::from_hex(&hex_state)?;
let threshold: u8 = ledger.threshold()?;
let fee: u64 = ledger.signing_fee()?;

for result in ledger.egress_jobs()?.iter() {
    let (_key, job) = result?;
    println!("job {}: amount={}, status={:?}", job.id, job.amount, job.status);
}
```

## Overview

The Compact compiler generates TypeScript bindings for contract state access. This project brings the same capability to Rust, using the [midnight-ledger](https://github.com/midnightntwrk/midnight-ledger) crates that power the Midnight node.

Two parts:

1. **Compiler extension** ([fork](https://github.com/RomarQ/compact/tree/feat/ledger-in-contract-info)) — extends `contract-info.json` with ledger field declarations (including `"exported"` flag and all storage kinds)
2. **Rust bindgen** — reads the enriched JSON and generates typed Rust code using midnight-ledger's `Aligned` + `TryFrom<&ValueSlice>` traits

## Prerequisites

### Building the compiler from the submodule

The repo includes the [forked compiler](https://github.com/RomarQ/compact/tree/feat/ledger-in-contract-info) as a git submodule. Build and compile contracts with:

```bash
make build-compiler   # builds compactc via nix from the submodule
make compile          # compiles all example contracts (checksum-cached)
```

Or compile a single contract manually:

```bash
compactc --skip-zk my-contract.compact compiled/my-contract
```

### Makefile targets

```
make build-compiler   — Build the Compact compiler from the submodule (nix)
make compile          — Compile all example contracts (skip ZK proofs)
make test             — Run all tests (Rust + codegen)
make test-codegen     — Run codegen tests only
make check            — cargo check --workspace
make clippy           — cargo clippy --workspace
make clean            — Remove compiled contract outputs
```

## Usage

### Option 1: Proc macro (recommended)

One dependency. Bindings generated at compile time. No CLI step, no generated files.

```toml
[dependencies]
midnight-bindgen = { git = "https://github.com/RomarQ/midnight-rust-bindgen" }
```

```rust
// src/lib.rs or src/main.rs
midnight_bindgen::contract!(Gateway, "compiled/gateway/compiler/contract-info.json");

// This generates `pub mod gateway { pub struct Gateway { ... } ... }`
use gateway::{Gateway, EgressJob, JobStatus};

let ledger = Gateway::from_hex(&hex_state)?;
let threshold: u8 = ledger.threshold()?;
```

The path is relative to your crate's `Cargo.toml`. The macro generates a module named after the contract (snake_case). Changes to `contract-info.json` trigger recompilation automatically.

### Option 2: CLI (generated crate)

Generates a self-contained Cargo crate. Useful for inspecting the output or checking it into version control.

```bash
cargo run -p compact-codegen -- \
  --input compiled/gateway/compiler/contract-info.json \
  --output compiled/gateway/rust \
  --name Gateway
```

```toml
[dependencies]
gateway-contract = { path = "compiled/gateway/rust" }
```

```rust
use gateway_contract::Gateway;

let ledger = Gateway::from_hex(&hex_state)?;
```

## What gets generated

For the MCS gateway contract (10 ledger fields, 3 structs, 2 enums):

| Generated | Example |
|---|---|
| Field constants | `FIELD_THRESHOLD: usize = 0` |
| Data structs | `EgressJob { id: u128, destination: Bytes<32>, status: JobStatus }` |
| Named enums | `JobStatus { Pending = 0, Completed = 1 }` |
| Alignment | `impl Aligned for EgressJob` (via midnight-ledger's trait) |
| Deserialization | `impl TryFrom<&ValueSlice> for EgressJob` (tuple decomposition) |
| Ledger wrapper | `Gateway::threshold() -> Result<u8, StateError>` |
| Map accessors | `Gateway::egress_jobs() -> Result<MapAccessor<TransientFr, EgressJob>, StateError>` |
| Set accessors | `Gateway::validators() -> Result<SetAccessor<EmbeddedGroupAffine>, StateError>` |
| Maybe structs | `Maybe { is_some: bool, value: T }` with `into_option()` |
| Curve points | `JubjubPoint` → `EmbeddedGroupAffine` (validated) |

## Supported types

| Compact type | Rust type | Notes |
|---|---|---|
| `Uint<N>` | `u8` / `u16` / `u32` / `u64` / `u128` | Smallest type fitting `maxval` |
| `Bytes<N>` | `Bytes<N>` | Newtype over `[u8; N]`, derefs to it |
| `Boolean` | `bool` | |
| `Field` | `TransientFr` | BLS12-381 scalar field element |
| `Counter` | `u64` | Always Uint<64> per midnight-ledger |
| `Map<K, V>` | `MapAccessor<K, V>` | `.get(key)`, `.iter()`, `.size()`, `.contains_key(key)` |
| `Set<T>` | `SetAccessor<T>` | `.contains(key)`, `.iter()`, `.size()` |
| `Struct` | Generated struct | `Aligned` + `TryFrom<&ValueSlice>` |
| `Enum` | `#[repr(u8)]` enum | `Aligned` + `TryFrom<&ValueSlice>` |
| `Maybe<T>` | Generated struct | Product type with `into_option()` convenience |
| `JubjubPoint` | `EmbeddedGroupAffine` | On-curve validated |
| `Vector<N, T>` | `[T; N]` | |
| `List<T>` | `ListAccessor<T>` | `.get(index)`, `.iter()`, `.len()` |
| `MerkleTree<D, T>` | `MerkleTreeAccessor` | `.height()`, `.root()`, `.first_free()` |

## Contracts with >15 ledger fields

When a contract has more than 15 ledger fields, the compiler batches them into a B-tree. Field indices become multi-level paths (e.g., `[1, 3]` instead of `3`). The generated code handles this transparently using `get_field_path`.

## Architecture

Similar to alloy's [`sol!`](https://github.com/alloy-rs/core) macro — code generation uses `quote!` to produce `TokenStream` directly (no string concatenation).

```
midnight-bindgen                        ← user-facing: one dependency
├── midnight-bindgen-macro              ← proc macro: reads JSON, calls codegen
├── midnight-bindgen-runtime            ← runtime: StateError, MapAccessor, re-exports
│   └── depends on midnight-ledger      ← same crates that power the Midnight node
└── compact-codegen                     ← code generation (quote!) + CLI
```

The runtime is minimal — it re-exports midnight-ledger types, provides `StateError`, navigation helpers (`cell_value`, `get_field`, `get_field_path`), and typed accessors (`MapAccessor`, `SetAccessor`). Generated types implement midnight-ledger's `Aligned` and `TryFrom<&ValueSlice>` traits directly, using tuple decomposition for struct deserialization.

## Compiler extension

The standard Compact compiler's `contract-info.json` only includes circuits and witnesses. Our [fork](https://github.com/RomarQ/compact/tree/feat/ledger-in-contract-info) extends `save-contract-info-passes.ss` to also emit a `"ledger"` array with all fields (both exported and non-exported):

```json
{
  "ledger": [
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
    }
  ]
}
```

## Testing

The project includes tests against 5 real Compact contracts compiled from the submodule:

- **counter** — simplest contract (1 counter field)
- **election** — diverse storage kinds (cells, counters, sets, merkle trees, enums)
- **tiny** — authorization pattern (cells, enum state machine, `Field` type)
- **zerocash** — privacy patterns (HistoricMerkleTree, sets, custom structs)
- **many-fields** — 16 fields exercising B-tree path indices

Tests include:
- **Codegen tests** — verify generated Rust is valid `syn::parse2`-parseable code
- **Synthetic state tests** — construct `ContractState<InMemoryDB>` with known values, verify accessor round-trips

```bash
make test           # compile contracts + run all tests
cargo test -p compact-codegen   # codegen tests only
```

## Status

- [x] Compiler extension ([fork](https://github.com/RomarQ/compact/tree/feat/ledger-in-contract-info))
- [x] Proc macro (`midnight_bindgen::contract!`)
- [x] CLI (`compact-codegen --version`)
- [x] Minimal runtime over midnight-ledger
- [x] `Aligned` + `TryFrom<&ValueSlice>` for generated types
- [x] `MapAccessor` / `SetAccessor` with typed lookups
- [x] `Maybe<T>` as product type with `into_option()`
- [x] B-tree path indices (>15 fields)
- [x] Synthetic state deserialization tests
- [x] JubjubPoint → `EmbeddedGroupAffine` with on-curve validation
- [ ] Upstream PR to [LFDT-Minokawa/compact](https://github.com/LFDT-Minokawa/compact)
- [x] List and MerkleTree typed accessors
- [ ] Circuit call / transaction construction

## References

- [Compact language reference](https://docs.midnight.network/develop/reference/compact/lang-ref)
- [midnight-ledger](https://github.com/midnightntwrk/midnight-ledger) — Rust ledger crates
- [midnight-zk](https://github.com/midnightntwrk/midnight-zk) — Curve types (`midnight-curves`)
- [Compact compiler](https://github.com/LFDT-Minokawa/compact)
