# Alloy migration notes

This branch is moving Artemis away from `ethers-rs` and toward Alloy.

## Current checkpoint

Checkpoint `9dc34ca` has completed several major migration slices:

- Core collectors and the mempool executor use Alloy provider/request surfaces.
- OpenSea v2 client request/response models use Alloy primitives.
- Chainbound and Flashbots public bundle action surfaces accept Alloy
  `TransactionRequest` values, with private ethers bridges where current relay
  libraries still require them.
- MEV-Share relay auth uses an Alloy signer.
- MEV-share uni arb generated ethers bindings have been removed; calldata is now
  encoded through Alloy `sol!`.
- OpenSea sudo arb bridge code preserves legacy and EIP-1559 transaction fields
  and rejects unsupported fields instead of silently dropping them.

The repository is still not ethers-free. The remaining coupling is concentrated
in:

- OpenSea sudo arb ethers Abigen bindings.
- Relay/bundle internals that still depend on `ethers-flashbots` or
  `mev-share` request/signing types.
- Chainbound Fiber transaction events, which are delivered by `fiber-rs` as
  `ethers::types::Transaction`.
- Binaries/examples that still construct ethers providers for strategies whose
  constructors have not fully moved to Alloy.

See `docs/alloy-migration-inventory.md` for the crate-by-crate inventory.

## Dependency status

The active Alloy graph is coherent around `alloy v2.0.5`. Avoid adding older
Alloy-generation MEV/Flashbots crates unless their dependency graph has been
verified not to pull incompatible Alloy versions.

The workspace still defines `ethers = "2"` because active crates still require
it. Remove that workspace dependency only after the remaining direct consumers
have migrated.

## Recommended migration order

1. **OpenSea sudo arb generated bindings**
   Replace the broad ethers Abigen binding crate with a minimal Alloy `sol!` or
   hand-written ABI surface. This is the largest remaining source-reference
   reduction and unlocks cleanup in `bin/artemis` and
   `StateOverrideMiddleware`.

2. **Chainbound Echo**
   Move Echo signing and block-number lookup to Alloy while keeping Fiber
   handling separate. Fiber can remain bridged if upstream `fiber-rs` still
   exposes ethers transaction objects.

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
