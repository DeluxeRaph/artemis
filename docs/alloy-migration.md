# Alloy migration notes

This branch moves Artemis away from `ethers-rs` and toward Alloy.

## Current state

The migration is complete for active Rust dependencies and local source:

- Core collectors and executors use Alloy provider/request/signer surfaces.
- Flashbots bundle submission uses a local JSON-RPC relay client instead of
  `ethers-flashbots`.
- MEV-share uses local wire types in `artemis-core` instead of the upstream
  `mev-share` crate.
- OpenSea stream handling uses local wire types backed by Alloy primitives
  instead of `opensea-stream`.
- OpenSea v2 and OpenSea sudo arb request/response/binding surfaces use Alloy
  primitives and `sol!` bindings.
- Chainbound Echo uses Alloy provider/wallet/signer paths.
- Chainbound Fiber is vendored as a local no-ethers client and emits Alloy RPC
  transactions at the collector boundary.

## Dependency status

The active dependency graph has no `ethers`, `ethers-core`,
`ethers-signers`, `ethers-flashbots`, `mev-share`, or `opensea-stream`
packages.

Verification commands:

```sh
cargo +stable tree --workspace -i ethers
cargo +stable tree --workspace -i ethers-core
cargo +stable tree --workspace -i ethers-signers
```

Each command should report that the package ID does not match any packages.

The remaining literal `ethers` strings in active non-doc files are unrelated to
runtime dependencies:

- `justfile` uses `etherscan` in Foundry source download commands.
- `crates/generator/src/parser.rs` has a negative regression test that asserts
  generated Cargo manifests do not include `ethers =`.

## Verification expectations

Before handoff, run:

```sh
cargo +stable fmt --all
git diff --check
cargo +stable check --workspace
cargo +stable test --workspace
cargo +stable clippy --workspace --all-features -- -D warnings
```

Generated binding directories can cause large rustfmt churn. Avoid formatting
generated files unless that is the explicit slice.
