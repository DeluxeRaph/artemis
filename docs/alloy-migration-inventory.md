# Artemis ethers-rs to Alloy migration inventory

This inventory reflects checkpoint `9dc34ca` on branch `migration`.

Scope of the audit:

- Excluded `.git/` and `target/`.
- Excluded the deleted `crates/strategies/mev-share-uni-arb/bindings/` tree.
- Counted remaining `ethers::` and `ethers_` source references.
- Checked manifest dependencies and the active dependency graph with `cargo tree`.

## Summary

The migration is now substantially past the original baseline. Core collectors,
mempool submission, OpenSea v2 client types, MEV-Share relay auth, Chainbound
bundle request surfaces, and MEV-share arb generated bindings have moved toward
Alloy.

Remaining ethers usage is concentrated in three areas:

- OpenSea sudo arb generated ethers Abigen bindings and the strategy bridge that
  still consumes them.
- Bundle relay internals that sign or submit through ethers-shaped upstream
  crates (`ethers-flashbots`, `mev-share`, and Chainbound Fiber).
- Binaries/examples that still construct ethers providers for strategies with
  ethers provider/signing bounds.

Fresh scan results:

- `rg` active source scan excluding target and deleted MEV-share bindings:
  97 files / 10,798 `ethers::|ethers_` matches.
- OpenSea sudo arb generated binding files account for 82 files / 10,698
  matches.
- Non-doc, non-generated files account for 13 files / 72 matches.
- Active manifests with direct ethers dependencies:
  - `Cargo.toml`
  - `bin/artemis/Cargo.toml`
  - `crates/artemis-core/Cargo.toml`
  - `crates/clients/chainbound/Cargo.toml`
  - `crates/strategies/opensea-sudo-arb/Cargo.toml`
  - `crates/strategies/opensea-sudo-arb/bindings/Cargo.toml`
  - `crates/strategies/mev-share-uni-arb/Cargo.toml`
  - `examples/mev-share-arb/Cargo.toml`

`cargo tree -i alloy` shows the workspace on `alloy v2.0.5`.
`cargo tree -i alloy-primitives` shows `alloy-primitives v1.6.0`, which is the
Alloy 2 crate family currently resolved by `alloy v2.0.5`.

`cargo tree -i ethers` shows `ethers v2.0.8` still required by:

- `bin/artemis`
- `crates/artemis-core`
- `crates/strategies/opensea-sudo-arb`
- `crates/strategies/opensea-sudo-arb/bindings`
- `ethers-flashbots`

`cargo tree -i ethers-signers` additionally shows `mev-share-rpc-api` through
`mev-share`, which keeps an upstream signer dependency in the graph even after
local MEV-share binding deletion.

## Crate inventory

### Workspace root

Files:

- `Cargo.toml`

Remaining ethers usage:

- Workspace dependency `ethers = { version = "2", features = ["ws", "rustls"] }`.

Removable now:

- No. It is still consumed by active crates listed below.

Next action:

- Remove the workspace dependency only after the OpenSea strategy/bindings,
  relay executor internals, Chainbound, and MEV-share strategy have dropped
  their direct ethers dependency.

### `bin/artemis`

Files:

- `bin/artemis/Cargo.toml`
- `bin/artemis/src/main.rs`

Remaining ethers usage:

- Builds an ethers WebSocket provider, nonce manager, and `LocalWallet`.
- Passes that provider into `OpenseaSudoArb::new`.
- Builds a separate Alloy provider for migrated collectors/executors.

Removable now:

- Not independently. This binary can drop ethers only after
  `opensea-sudo-arb` accepts an Alloy provider/signer end to end.

Next action:

- Once OpenSea sudo arb bindings are replaced with Alloy `sol!` or a narrower
  Alloy contract surface, remove the ethers provider path and keep a single
  Alloy provider/signer stack in the binary.

### `crates/artemis-core`

Files:

- `crates/artemis-core/Cargo.toml`
- `crates/artemis-core/src/executors/flashbots_executor.rs`
- `crates/artemis-core/src/utilities/state_override_middleware.rs`

Remaining ethers usage:

- `flashbots_executor.rs` keeps an Alloy-facing public bundle type
  (`Vec<alloy::rpc::types::TransactionRequest>`) but converts each request to
  ethers `TypedTransaction` for signing/submission through
  `ethers-flashbots::FlashbotsMiddleware`.
- The conversion path currently preserves legacy and EIP-1559 fields and
  rejects unsupported access-list, blob, authorization-list, conflicting input,
  and incompatible fee/type combinations.
- `state_override_middleware.rs` is an ethers `Middleware` wrapper around
  `ethers::providers::spoof::State`; it is used by the OpenSea sudo arb quoter
  while that strategy still uses ethers bindings.
- `mev-share = "0.1.4"` keeps ethers signer crates in the dependency graph
  through `mev-share-rpc-api`, even though local public relay auth now uses an
  Alloy signer.

Removable now:

- `state_override_middleware.rs`: only after OpenSea sudo arb no longer needs
  ethers `ContractCall` with state override.
- `flashbots_executor.rs`: not cleanly removable without replacing
  `ethers-flashbots` with Alloy-native relay submission or a local JSON-RPC
  implementation.
- `mev-share`: not locally removable while `MevshareExecutor` and MEV-share
  strategy actions still use upstream `mev_share::rpc` request types.

Upstream-blocked:

- `ethers-flashbots` is ethers-native.
- `mev-share`/`mev-share-rpc-api` are ethers-shaped in their request/signing
  dependencies.

Next actions:

- For Flashbots, either implement a minimal Alloy-native `eth_sendBundle` /
  `eth_callBundle` JSON-RPC client locally, or verify a current Alloy 2-native
  relay extension can replace `ethers-flashbots` without pulling an older Alloy
  generation.
- For MEV-share, decide whether to keep upstream request structs behind a narrow
  bridge or fork/replace the request types locally.
- Delete `StateOverrideMiddleware` after the OpenSea quoter is moved to Alloy
  calls with state override support or an equivalent local RPC helper.

### `crates/clients/chainbound`

Files:

- `crates/clients/chainbound/Cargo.toml`
- `crates/clients/chainbound/src/echo.rs`
- `crates/clients/chainbound/src/fiber.rs`
- `crates/clients/chainbound/src/lib.rs`

Remaining ethers usage:

- `echo.rs` exposes Alloy transaction requests in `SendBundleArgs` but converts
  them to ethers `TypedTransaction` for signing and uses an ethers middleware to
  fetch the next block number.
- The Echo conversion path preserves legacy and EIP-1559 fields and rejects
  unsupported transaction fields instead of dropping them.
- `fiber.rs` exposes Fiber transaction events as `ethers::types::Transaction`
  because `fiber-rs` currently streams that type.
- Tests/examples use ethers providers and wallets.

Removable now:

- Echo block-number lookup and signing can be migrated locally to Alloy.
- Fiber transaction event type is upstream-shaped unless Artemis wraps or
  converts Fiber events at the crate boundary.

Upstream-blocked:

- Fiber transaction payloads from `fiber-rs` are ethers transaction objects.

Next actions:

- Split Echo from Fiber dependencies if possible: migrate Echo to an Alloy
  provider/signer or local signer helper first.
- Add an Alloy event wrapper for Fiber transactions if the crate should expose
  Alloy types even while Fiber internally returns ethers values.

### `crates/strategies/opensea-sudo-arb`

Files:

- `crates/strategies/opensea-sudo-arb/Cargo.toml`
- `crates/strategies/opensea-sudo-arb/src/constants.rs`
- `crates/strategies/opensea-sudo-arb/src/strategy.rs`
- `crates/strategies/opensea-sudo-arb/src/types.rs`
- `crates/strategies/opensea-sudo-arb/bindings/Cargo.toml`
- `crates/strategies/opensea-sudo-arb/bindings/src/*.rs`

Remaining ethers usage:

- The generated binding crate is still ethers Abigen output and dominates the
  remaining direct source references.
- The strategy still has an ethers `Middleware` provider bound, ethers pool/order
  primitive types, ethers `Filter`, generated contract calls, and a bridge from
  ethers `TypedTransaction` to Alloy `TransactionRequest`.
- `types.rs` converts Alloy OpenSea response primitives into ethers binding
  parameter structs for the generated arb contract.
- `constants.rs` uses ethers `EthEvent` signatures and `Lazy` because the event
  filters are generated ethers types.

Removable now:

- Yes, with local work. This is the largest remaining local migration slice.

Next actions:

- Replace the broad generated ethers binding crate with minimal Alloy `sol!`
  bindings or hand-written ABI call/event structs for the actual surface used:
  - `SudoOpenseaArb::execute_arb`
  - `SudoPairQuoter::get_multiple_sell_quotes`
  - `SUDOPAIRQUOTER_DEPLOYED_BYTECODE`
  - `LSSVMPairFactory::NewPair` event querying
  - pool touch event signatures used by `POOL_EVENT_SIGNATURES`
- Preserve existing regression coverage for OpenSea order parameter conversion,
  quote conversion, factory event filtering, and transaction request field
  preservation.
- After this lands, remove `StateOverrideMiddleware` if no other crate uses it,
  remove this crate's direct ethers dependency, and simplify `bin/artemis`.

### `crates/strategies/mev-share-uni-arb`

Files:

- `crates/strategies/mev-share-uni-arb/Cargo.toml`
- `crates/strategies/mev-share-uni-arb/src/strategy.rs`
- `crates/strategies/mev-share-uni-arb/src/types.rs`

Remaining ethers usage:

- The generated binding crate has been deleted.
- Calldata generation has moved to Alloy `sol!`.
- The strategy still uses ethers provider/signer traits, ethers
  `TypedTransaction`, and ethers `TransactionRequest` so it can call
  `fill_transaction`, sign the transaction, RLP encode it, and build upstream
  `mev_share::rpc::SendBundleRequest`.
- Tests still use ethers ABI encoding as a compatibility oracle.

Removable now:

- Partly. Local transaction construction/signing can move to Alloy, but
  `mev-share` request types remain upstream-shaped.

Upstream-blocked:

- `mev-share` and `mev-share-rpc-api` keep ethers-shaped types/signing
  dependencies in the graph.

Next actions:

- Move gas/block lookups and transaction filling/signing to Alloy.
- Keep `mev_share::rpc::{SendBundleRequest, BundleItem, Inclusion}` behind a
  small adapter, or replace those request structs locally if removing the
  upstream crate is desired.
- Keep the ABI parity tests until the Alloy-only transaction builder has its own
  field-preservation coverage.

### `examples/mev-share-arb`

Files:

- `examples/mev-share-arb/Cargo.toml`
- `examples/mev-share-arb/src/main.rs`

Remaining ethers usage:

- Builds an ethers WebSocket provider, nonce manager, and `LocalWallet` for
  `MevShareUniArb`.
- Uses an Alloy `PrivateKeySigner` for `MevshareExecutor` relay auth.

Removable now:

- Not independently. It depends on the MEV-share strategy constructor still
  requiring ethers provider/signer types.

Next action:

- After `mev-share-uni-arb` moves provider/signing to Alloy, collapse the
  example to one Alloy provider/signer stack.

### Deleted MEV-share generated bindings

Files:

- `crates/strategies/mev-share-uni-arb/bindings/`

Status:

- Deleted by checkpoint `9dc34ca`; exclude this path from future active
  migration counts.

Next action:

- Do not reintroduce this crate. Keep the local `sol!` interface in
  `strategy.rs` or move it to a small Alloy binding module if it grows.

## Recommended next migration order

1. OpenSea sudo arb generated bindings.
   This removes almost all remaining source-level ethers references and unlocks
   cleanup in `bin/artemis` and `StateOverrideMiddleware`.
2. Chainbound Echo local signing/block-number migration.
   This is mostly local and can reduce direct ethers use without waiting on
   Fiber.
3. Flashbots executor relay replacement.
   Prefer a minimal local JSON-RPC relay client unless an Alloy 2-native
   replacement is verified.
4. MEV-share strategy/executor bridge narrowing.
   Move local signing/filling to Alloy while explicitly isolating upstream
   `mev-share` request types.
5. Fiber event wrapper or upstream update.
   Convert Fiber transaction events at the boundary if an Alloy public API is
   more important than preserving the upstream type exactly.

## Estimated completion

Estimated overall migration completeness after checkpoint `9dc34ca`: 75-80%.

The project has moved most public/core surfaces to Alloy, but the remaining
OpenSea binding crate is large, and relay/Fiber/MEV-share upstream dependencies
still keep ethers in the dependency graph.
