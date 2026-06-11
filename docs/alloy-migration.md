# Alloy migration notes

This branch is moving Artemis away from `ethers-rs` and toward Alloy.

## Current state

The integrated migration branch has completed the major local binding and public
surface migrations:

- Core collectors and the mempool executor use Alloy provider/request surfaces.
- OpenSea v2 client request/response models use Alloy primitives.
- Chainbound, Flashbots, and MEV-share public bundle/action surfaces accept
  Alloy-facing request or signer types where the local API controls them.
- MEV-share uni arb generated ethers bindings have been removed; calldata is
  encoded with Alloy `sol!`.
- OpenSea sudo arb generated ethers bindings have been replaced by a small
  Alloy `sol!` binding surface.
- Chainbound Fiber transaction events are Alloy RPC transactions at the Artemis
  boundary, with an isolated conversion from upstream `fiber-rs`.
- Relay and bundle bridges preserve supported legacy/EIP-1559 fields and reject
  unsupported fields instead of silently dropping them.

The repository is still not fully ethers-free. Remaining ethers usage is
concentrated in compatibility boundaries:

- `ethers-flashbots` for Flashbots bundle submission.
- `mev-share` / `mev-share-rpc-api` request and bundle types.
- `fiber-rs`, which still emits ethers transaction objects internally.
- OpenSea sudo arb provider/state override code and `opensea-stream` event
  primitives.
- App/example bridge modules that construct ethers providers only for strategy
  constructors that still require ethers middleware/signers.

See `docs/alloy-migration-inventory.md` for the crate-by-crate inventory.

## Dependency status

The active Alloy graph is coherent around `alloy v2.0.5`. Avoid adding older
Alloy-generation MEV/Flashbots crates unless their dependency graph has been
verified not to pull incompatible Alloy versions.

The workspace still defines `ethers = "2"` because active compatibility bridges
still require it. Remove that workspace dependency only after the remaining
direct consumers have migrated or been replaced.

## Recommended migration order

1. **OpenSea provider/state override**
   Move the OpenSea sudo arb strategy from ethers `Middleware`, `Filter`, and
   `StateOverrideMiddleware` to Alloy provider calls and state override support.

2. **Chainbound Echo**
   Move Echo signing and block-number lookup to Alloy. Fiber can remain bridged
   while upstream `fiber-rs` emits ethers transaction objects internally.

3. **Flashbots executor**
   Replace `ethers-flashbots` with either a verified Alloy 2-native relay API or
   a minimal local JSON-RPC client for bundle simulation/submission.

4. **MEV-share strategy/executor**
   Move local gas/block lookup, filling, and signing to Alloy. Keep upstream
   `mev_share::rpc` request structs behind a narrow adapter, or replace them
   locally if the goal is to remove the upstream crate.

5. **Final dependency cleanup**
   Remove stale direct ethers dependencies crate by crate, then remove the
   workspace ethers dependency and verify with `cargo tree -i ethers`.

## Verification expectations

For each integrated migration slice:

```sh
git diff --check
cargo +stable check --workspace
```

Run targeted tests for touched crates. Before a final handoff, run workspace
tests and clippy when feasible:

```sh
cargo +stable test --workspace
cargo +stable clippy --all --all-features -- -D warnings
```

Generated binding directories can cause large rustfmt churn. Avoid formatting
generated files unless that is the explicit slice.
