# Artemis ethers-rs to Alloy migration inventory

This inventory reflects the `migration` branch after the final zero-ethers
cleanup.

## Summary

The active workspace has no `ethers-rs` dependencies left.

Removed or replaced:

- Workspace `ethers` dependency.
- `ethers-flashbots` Flashbots relay path.
- Upstream `mev-share` / `mev-share-rpc-api` request types.
- Upstream `opensea-stream` schema/client dependency.
- Upstream `fiber-rs` dependency path that pulled `ethers` internally.
- Generated ethers bindings in MEV-share uni arb and OpenSea sudo arb.
- Local bridge modules that built ethers providers, middleware, wallets, or
  typed transactions.

Current dependency graph checks:

```sh
cargo +stable tree --workspace -i ethers
cargo +stable tree --workspace -i ethers-core
cargo +stable tree --workspace -i ethers-signers
```

All three commands should report no matching package.

## Crate Inventory

### Workspace root

- `Cargo.toml` has no `ethers` workspace dependency.
- `Cargo.lock` has no `ethers*` package entries.

### `bin/artemis`

- Uses Alloy provider and signer setup directly.
- No `opensea-stream` or ethers bridge dependency remains.

### `crates/artemis-core`

- Owns local MEV-share wire types in `src/mev_share.rs`.
- Owns local OpenSea stream wire types/client helpers in
  `src/opensea_stream.rs`.
- Flashbots submission is handled by local relay JSON-RPC helpers.

### `crates/clients/chainbound`

- Echo uses Alloy request/signing/provider types.
- Fiber collector consumes the local `crates/clients/fiber` path dependency.
- Fiber transaction events are Alloy RPC transactions at the Artemis boundary.

### `crates/clients/fiber`

- Vendored from `fiber-rs` and patched to remove ethers.
- Keeps protobuf-generated API and stream types.
- Converts Fiber protobuf transactions into Alloy RPC transactions.

### `crates/strategies/mev-share-uni-arb`

- Uses Alloy `sol!` calldata generation and local Artemis MEV-share wire types.
- No upstream `mev-share` dependency remains.

### `examples/mev-share-arb`

- Uses Alloy provider/signer setup and local Artemis MEV-share collector and
  executor types.

### `crates/strategies/opensea-sudo-arb`

- Uses Alloy provider, filter, state override, transaction request, and `sol!`
  binding surfaces.
- Consumes OpenSea stream events from `artemis-core::opensea_stream`.

## Literal String Exceptions

The following remaining source strings are intentional and do not indicate an
`ethers-rs` dependency:

- `justfile`: Foundry `etherscan-source` commands and `ETHERSCAN_API_KEY`.
- `crates/generator/src/parser.rs`: regression test names/messages asserting
  generated manifests do not contain `ethers =`.
