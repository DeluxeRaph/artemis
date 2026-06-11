# Artemis ethers-rs to Alloy migration inventory

This inventory reflects the integrated `migration` branch after the MEV-share
binding removal, OpenSea binding replacement, Chainbound Fiber event migration,
relay bridge hardening, app/example bridge isolation, and dependency cleanup.

Scope of the audit:

- Excluded `.git/`, `target/`, `Cargo.lock`, `docs/`, `AGENTS.md`, and
  `README.md` from active source counts.
- Counted remaining `ethers::` and `ethers_` source references.
- Checked manifest dependencies and the active dependency graph with
  `cargo tree`.

## Summary

The largest local ethers surfaces have been removed:

- `crates/strategies/mev-share-uni-arb/bindings/` was deleted.
- The OpenSea sudo arb generated ethers binding crate was replaced with a small
  Alloy `sol!` surface.
- Chainbound Fiber transaction events are Alloy RPC transactions at the Artemis
  boundary.
- Bundle bridge conversions have fail-closed tests for unsupported transaction
  fields.

Fresh active source scan:

- Non-doc active source scan: 65 `ethers::|ethers_` lines.
- Those references are concentrated in 10 files:
  - `bin/artemis/src/main.rs`
  - `examples/mev-share-arb/src/main.rs`
  - `crates/artemis-core/src/executors/flashbots_executor.rs`
  - `crates/artemis-core/src/utilities/state_override_middleware.rs`
  - `crates/clients/chainbound/src/echo.rs`
  - `crates/clients/chainbound/src/fiber.rs`
  - `crates/clients/chainbound/src/lib.rs`
  - `crates/strategies/mev-share-uni-arb/src/strategy.rs`
  - `crates/strategies/opensea-sudo-arb/src/constants.rs`
  - `crates/strategies/opensea-sudo-arb/src/strategy.rs`

Active manifests with direct ethers dependencies:

- `Cargo.toml`
- `bin/artemis/Cargo.toml`
- `crates/artemis-core/Cargo.toml`
- `crates/clients/chainbound/Cargo.toml`
- `crates/strategies/opensea-sudo-arb/Cargo.toml`
- `crates/strategies/mev-share-uni-arb/Cargo.toml`
- `examples/mev-share-arb/Cargo.toml`

`cargo tree -i alloy` shows the workspace on `alloy v2.0.5`.
`cargo tree -i alloy-primitives` shows `alloy-primitives v1.6.0`, which is the
Alloy 2 crate family currently resolved by `alloy v2.0.5`.

`cargo tree -i ethers` still resolves ethers through the local compatibility
bridges and upstream crates:

- `bin/artemis`
- `crates/artemis-core`
- `crates/clients/chainbound`
- `crates/strategies/opensea-sudo-arb`
- `crates/strategies/mev-share-uni-arb`
- `examples/mev-share-arb`
- `ethers-flashbots`
- `fiber`
- `mev-share` / `mev-share-rpc-api`

## Crate inventory

### Workspace root

Files:

- `Cargo.toml`

Remaining ethers usage:

- Workspace dependency `ethers = { version = "2", features = ["ws", "rustls"] }`.

Removable now:

- No. It is still consumed by active compatibility bridges and upstream crates.

Next action:

- Remove the workspace dependency only after OpenSea, relay executors,
  Chainbound Echo/Fiber bridges, MEV-share strategy, and app/example bridge
  modules have dropped their direct ethers usage.

### `bin/artemis`

Files:

- `bin/artemis/Cargo.toml`
- `bin/artemis/src/main.rs`

Remaining ethers usage:

- Isolated in a private `strategy_ethers_bridge` module.
- Builds an ethers WebSocket provider, nonce manager, and `LocalWallet` only for
  `OpenseaSudoArb::new`.
- The rest of the binary setup is Alloy-first for migrated collectors and
  executors.

Removable now:

- Not independently. This binary can drop ethers after `opensea-sudo-arb`
  accepts an Alloy provider/signer end to end.

Next action:

- Migrate the OpenSea strategy provider/state override path to Alloy, then
  collapse this binary to one Alloy provider/signer stack.

### `crates/artemis-core`

Files:

- `crates/artemis-core/Cargo.toml`
- `crates/artemis-core/src/executors/flashbots_executor.rs`
- `crates/artemis-core/src/utilities/state_override_middleware.rs`

Remaining ethers usage:

- `flashbots_executor.rs` keeps an Alloy-facing public bundle type but converts
  each request to ethers `TypedTransaction` for signing/submission through
  `ethers-flashbots::FlashbotsMiddleware`.
- The conversion path preserves supported legacy and EIP-1559 fields and
  rejects unsupported access-list, blob, authorization-list, conflicting input,
  and incompatible fee/type combinations.
- `state_override_middleware.rs` is an ethers `Middleware` wrapper around
  `ethers::providers::spoof::State`; it is used by the OpenSea sudo arb quoter.
- `mev-share = "0.1.4"` keeps ethers signer crates in the dependency graph
  through `mev-share-rpc-api`.

Removable now:

- `state_override_middleware.rs`: only after OpenSea sudo arb uses Alloy calls
  with state override support or an equivalent local RPC helper.
- `flashbots_executor.rs`: not cleanly removable without replacing
  `ethers-flashbots`.
- `mev-share`: not locally removable while `MevshareExecutor` and MEV-share
  strategy actions still use upstream `mev_share::rpc` request types.

Upstream-blocked:

- `ethers-flashbots` is ethers-native.
- `mev-share`/`mev-share-rpc-api` keep ethers-shaped dependencies.

Next actions:

- For Flashbots, implement a minimal Alloy-native `eth_sendBundle` /
  `eth_callBundle` JSON-RPC client locally, or verify a current Alloy 2-native
  relay extension that does not pull an older Alloy generation.
- For MEV-share, decide whether to keep upstream request structs behind a
  bridge or replace the request types locally.
- Delete `StateOverrideMiddleware` after the OpenSea quoter no longer uses it.

### `crates/clients/chainbound`

Files:

- `crates/clients/chainbound/Cargo.toml`
- `crates/clients/chainbound/src/echo.rs`
- `crates/clients/chainbound/src/fiber.rs`
- `crates/clients/chainbound/src/lib.rs`

Remaining ethers usage:

- `fiber.rs` converts upstream `fiber-rs` ethers transactions into Alloy RPC
  transactions at the collector boundary.
- `echo.rs` exposes Alloy transaction requests in `SendBundleArgs` but converts
  them to ethers `TypedTransaction` for signing and uses an ethers middleware to
  fetch the next block number.
- Tests/examples use ethers providers and wallets.

Removable now:

- Echo block-number lookup and signing can be migrated locally to Alloy.
- Fiber upstream conversion can only disappear if `fiber-rs` exposes Alloy
  transactions or the crate stops using `fiber-rs` directly.

Upstream-blocked:

- `fiber-rs` still emits ethers transaction objects.

Next actions:

- Migrate Echo to an Alloy provider/signer or local signer helper.
- Keep the Fiber conversion helper covered by field-preservation tests.

### `crates/strategies/opensea-sudo-arb`

Files:

- `crates/strategies/opensea-sudo-arb/Cargo.toml`
- `crates/strategies/opensea-sudo-arb/src/constants.rs`
- `crates/strategies/opensea-sudo-arb/src/strategy.rs`
- `crates/strategies/opensea-sudo-arb/src/types.rs`
- `crates/strategies/opensea-sudo-arb/bindings/Cargo.toml`
- `crates/strategies/opensea-sudo-arb/bindings/src/alloy_bindings.rs`
- `crates/strategies/opensea-sudo-arb/bindings/src/sudo_pair_quoter_bytecode.rs`

Remaining ethers usage:

- The broad generated ethers binding crate is gone.
- The new binding crate is a small Alloy `sol!` surface.
- The strategy still has an ethers `Middleware` provider bound, ethers
  `Filter`/event path for `opensea-stream` values, ethers quoter state override
  plumbing, and a bridge from ethers `TypedTransaction` to Alloy
  `TransactionRequest`.
- `constants.rs` still uses ethers event signature helpers for the remaining
  event filter path.

Removable now:

- Partly. The generated binding blocker is gone; the remaining work is provider,
  state override, and event/filter migration.

Next actions:

- Move provider calls, block/log filters, and quoter state override to Alloy.
- Replace remaining ethers pool/order primitives with Alloy primitives at the
  strategy boundary.
- Remove this crate's direct ethers dependency once the provider/filter bridge is
  gone.
- Simplify `bin/artemis` after the strategy constructor accepts Alloy types end
  to end.

### `crates/strategies/mev-share-uni-arb`

Files:

- `crates/strategies/mev-share-uni-arb/Cargo.toml`
- `crates/strategies/mev-share-uni-arb/src/strategy.rs`
- `crates/strategies/mev-share-uni-arb/src/types.rs`

Remaining ethers usage:

- The generated binding crate is deleted.
- Calldata generation uses Alloy `sol!`.
- Pool records and strategy lookup keys use Alloy addresses.
- The strategy still uses ethers provider/signer traits, ethers
  `TypedTransaction`, and ethers `TransactionRequest` for filling, signing, RLP
  encoding, and building upstream `mev_share::rpc::SendBundleRequest`.
- Tests use ethers ABI encoding as a compatibility oracle.

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

### `examples/mev-share-arb`

Files:

- `examples/mev-share-arb/Cargo.toml`
- `examples/mev-share-arb/src/main.rs`

Remaining ethers usage:

- Isolated in a private `strategy_ethers_bridge` module for
  `MevShareUniArb::new`.
- Uses an Alloy `PrivateKeySigner` for `MevshareExecutor` relay auth.

Removable now:

- Not independently. It depends on the MEV-share strategy constructor still
  requiring ethers provider/signer types.

Next action:

- After `mev-share-uni-arb` moves provider/signing to Alloy, collapse the
  example to one Alloy provider/signer stack.

## Recommended next migration order

1. OpenSea provider/filter/state override migration.
2. Chainbound Echo local signing/block-number migration.
3. Flashbots local Alloy-native relay client or verified Alloy 2-native
   replacement.
4. MEV-share local signing/filling migration and request adapter.
5. App/example bridge removal and final workspace ethers dependency cleanup.

## Estimated completion

Estimated overall migration completeness after this integrated pass: 85-90%.

Most local generated binding and public API migration work is complete. The
remaining work is concentrated in upstream compatibility bridges and provider /
signing internals that still require ethers until replaced or isolated behind
local Alloy-native clients.
