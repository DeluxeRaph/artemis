# Artemis ethers-rs to Alloy migration inventory

This document maps remaining `ethers-rs` usage in Artemis to the Alloy APIs that should replace it. It is intentionally repo-scoped: no QuickNode, AgentCash, or private harness logic belongs here.

## Current migration status

- Branch: `migration`
- First code slice completed: `opensea-v2` client models now use Alloy primitives and no longer depend directly on `ethers`.
- Remaining migration work is concentrated in:
  - `artemis-core` provider-facing collectors/executors/utilities
  - Chainbound / bundle submission client code
  - strategy wrappers that still depend on generated ethers bindings
  - generated `ethers-rs` Abigen bindings
  - binaries, examples, and generator templates

## Inventory summary

The reproducible active-code/dependency scan used for this inventory excludes `.git/`, `target/`, `docs/`, `Cargo.lock`, every `README.md`, and `justfile`, then counts regex matches for `ethers\w*`:

```bash
python3 - <<'PY'
from pathlib import Path
import re

root = Path('.')
pattern = re.compile(r'ethers\w*')
exclude_dirs = {'.git', 'target'}
exclude_names = {'Cargo.lock', 'README.md', 'justfile'}

files = hits = generated_files = generated_hits = 0
binding_cargo_files = binding_cargo_hits = 0
for path in root.rglob('*'):
    if not path.is_file():
        continue
    rel = path.relative_to(root).as_posix()
    if exclude_dirs.intersection(path.parts):
        continue
    if rel.startswith('docs/') or path.name in exclude_names:
        continue
    text = path.read_text(errors='ignore')
    count = len(pattern.findall(text))
    if not count:
        continue
    files += 1
    hits += count
    is_binding = '/bindings/' in rel or rel.endswith('/bindings/Cargo.toml')
    if is_binding:
        generated_files += 1
        generated_hits += count
        if rel.endswith('/bindings/Cargo.toml'):
            binding_cargo_files += 1
            binding_cargo_hits += count

print(f'active files={files} active ethers_tokens={hits}')
print(f'generated bindings including binding Cargo.toml={generated_files} files / {generated_hits} ethers_tokens')
print(f'binding Cargo.toml still active={binding_cargo_files} files / {binding_cargo_hits} ethers_tokens')
print(f'generated .rs-only bindings={generated_files - binding_cargo_files} files / {generated_hits - binding_cargo_hits} ethers_tokens')
PY
```

Current output:

- Active code/dependency scan: 118 files with remaining ethers-rs references / 11,314 `ethers*` token occurrences.
- Generated bindings dominate the count: 90 files / 11,240 `ethers*` token occurrences when the two binding `Cargo.toml` files are included.
- Generated `.rs` binding files only: 88 files / 11,238 `ethers*` token occurrences.
- The two excluded-from-`.rs` binding `Cargo.toml` files still contain active ethers dependencies: `crates/strategies/opensea-sudo-arb/bindings/Cargo.toml` and `crates/strategies/mev-share-uni-arb/bindings/Cargo.toml`.
- A broader repository scan excluding only `.git/` and `target/` finds 124 files / 11,453 `ethers*` token occurrences because it includes docs, READMEs, `Cargo.lock`, and `justfile`.

### Non-generated hotspots

- Workspace root:
  - `Cargo.toml`
  - currently declares `ethers` and `ethers-signers` workspace dependencies.
- `crates/artemis-core`:
  - `Cargo.toml`
  - `src/collectors/block_collector.rs`
  - `src/collectors/log_collector.rs`
  - `src/collectors/mempool_collector.rs`
  - `src/executors/flashbots_executor.rs`
  - `src/executors/mempool_executor.rs`
  - `src/executors/mev_share_executor.rs`
  - `src/utilities/state_override_middleware.rs`
  - `tests/main.rs`
- `crates/clients/chainbound`:
  - `Cargo.toml`
  - `src/echo.rs`
  - `src/fiber.rs`
  - `src/lib.rs`
  - `src/mev_bundle.rs`
- Strategies, non-generated:
  - `crates/strategies/mev-share-uni-arb/Cargo.toml`
  - `crates/strategies/mev-share-uni-arb/src/strategy.rs`
  - `crates/strategies/mev-share-uni-arb/src/types.rs`
  - `crates/strategies/opensea-sudo-arb/Cargo.toml`
  - `crates/strategies/opensea-sudo-arb/src/constants.rs`
  - `crates/strategies/opensea-sudo-arb/src/strategy.rs`
  - `crates/strategies/opensea-sudo-arb/src/types.rs`
- Generated binding crates:
  - `crates/strategies/mev-share-uni-arb/bindings`
  - `crates/strategies/opensea-sudo-arb/bindings`
- Other:
  - `bin/artemis`
  - `examples/mev-share-arb`
  - `crates/generator`

## Target Alloy crate set

Use a coherent Alloy version across the workspace rather than mixing old and new Alloy generations.

Recommended target crate family:

- `alloy` with features as needed:
  - `contract`
  - `providers`
  - `provider-http`
  - `provider-ws` for WebSocket providers and subscription transports
  - `pubsub` for `subscribe_blocks`, `subscribe_logs`, `subscribe_pending_transactions`, and `subscribe_full_pending_transactions`
  - `provider-mev-api` for `alloy_provider::ext::MevApi` Flashbots / MEV provider extensions
  - `rpc-types-mev` for umbrella-crate access to MEV RPC schemas
  - `signer-local`
  - `rpc-types`
  - `reqwest-rustls-tls`
- `alloy-primitives`
- `alloy-rpc-types-eth`
- `alloy-provider` with equivalent `ws`, `pubsub`, and `mev-api` feature support when using direct crates instead of the umbrella `alloy` crate
- `alloy-network`
- `alloy-consensus`
- `alloy-signer`
- `alloy-signer-local`
- `alloy-contract`
- `alloy-sol-types` / `alloy::sol!`
- `alloy-rpc-types-mev` for Flashbots / MEV RPC types

Avoid `alloy-flashbots-rs = "0.1.0"` for the full migration unless it is updated: it depends on old Alloy crates and can conflict with modern Alloy dependency graphs.

## Ethers to Alloy replacement map

### Provider and middleware

Current ethers APIs:

```rust
ethers::providers::Middleware
ethers::providers::Provider
ethers::providers::PubsubClient
```

Alloy replacements:

```rust
alloy_provider::Provider
alloy_provider::ProviderBuilder
alloy_provider::RootProvider
```

Recommended generic bound:

```rust
P: alloy_provider::Provider + Clone + Send + Sync + 'static
```

Provider construction:

```rust
let http_provider = alloy_provider::ProviderBuilder::new().connect_http(url);
let ws_provider = alloy_provider::ProviderBuilder::new().connect(ws_url).await?;
```

Alloy does not use ethers-style `Middleware` as the central abstraction. For custom behavior, prefer wrapper structs, provider fillers/layers where appropriate, or explicit helper functions at the call site.

### Block subscription

Current ethers:

```rust
provider.subscribe_blocks().await?
```

Alloy:

```rust
let sub = provider.subscribe_blocks().await?;
let stream = sub.into_stream();
```

Notes:

- Alloy `subscribe_blocks()` subscribes to `newHeads` and yields header responses.
- If full block bodies are needed, use `subscribe_full_blocks()`.
- HTTP polling alternatives include `watch_blocks()`, `watch_headers()`, and `watch_full_blocks()`.

### Log subscription

Current ethers:

```rust
provider.subscribe_logs(&filter).await?
```

Alloy:

```rust
use alloy_rpc_types_eth::Filter;

let sub = provider.subscribe_logs(&filter).await?;
let stream = sub.into_stream();
```

HTTP polling alternative:

```rust
provider.watch_logs(&filter).await?
```

### Pending transaction subscription

Current ethers:

```rust
let stream = provider.subscribe_pending_txs().await?;
let stream = stream.transactions_unordered(256);
```

Alloy options:

```rust
let hashes = provider.subscribe_pending_transactions().await?.into_stream();
let full = provider.subscribe_full_pending_transactions().await?.into_stream();
```

If full pending transactions are not supported by the node, subscribe to hashes and call:

```rust
provider.get_transaction_by_hash(hash).await?
```

HTTP polling alternatives:

```rust
provider.watch_pending_transactions().await?
provider.watch_full_pending_transactions().await?
```

### Common provider calls

- `get_block_number()` → `provider.get_block_number().await?`
- `send_raw_transaction(bytes)` → `provider.send_raw_transaction(&encoded_tx).await?`
- `send_transaction(tx, None)` → `provider.send_transaction(tx).await?`
- `estimate_gas(&tx, None)` → `provider.estimate_gas(tx).await?`
- `get_gas_price()` → `provider.get_gas_price().await?`

Important return differences:

- Alloy `estimate_gas` returns `u64`.
- Alloy `get_gas_price` returns `u128`.
- Alloy `send_raw_transaction` returns a `PendingTransactionBuilder`.

### Primitive and RPC types

Ethers primitives:

```rust
H160
H256
U256
U64
Bytes
Address
```

Alloy primitives:

```rust
alloy_primitives::Address
alloy_primitives::B256
alloy_primitives::U256
alloy_primitives::Bytes
u64 / u128 for many RPC quantities
```

Ethers RPC types:

```rust
ethers::types::Transaction
ethers::types::Block
ethers::types::Log
ethers::types::Filter
ethers::types::TransactionRequest
ethers::types::Chain
```

Alloy RPC types:

```rust
alloy_rpc_types_eth::Transaction
alloy_rpc_types_eth::Block
alloy_rpc_types_eth::Header
alloy_rpc_types_eth::Log
alloy_rpc_types_eth::Filter
alloy_rpc_types_eth::TransactionRequest
alloy_primitives::ChainId // usually u64-compatible
```

### Filter builder

Ethers patterns:

```rust
Filter::new().address(...).topic0(...).event(...)
```

Alloy patterns:

```rust
Filter::new()
    .address(address)
    .event("Transfer(address,address,uint256)")
    .event_signature(topic0)
    .topic1(topic1)
    .topic2(topic2)
    .topic3(topic3)
    .from_block(...)
    .to_block(...)
    .at_block_hash(...)
```

### Wallets and signers

Ethers:

```rust
ethers::signers::LocalWallet
ethers::signers::Signer
wallet.sign_transaction(&tx).await?
wallet.sign_message(msg).await?
wallet.address()
wallet.with_chain_id(chain)
```

Alloy:

```rust
alloy_signer_local::PrivateKeySigner
alloy_signer::Signer
alloy_signer::SignerSync
alloy_network::TxSigner
alloy_network::TxSignerSync
```

Examples:

```rust
let signer: alloy_signer_local::PrivateKeySigner = private_key_hex.parse()?;
let signer = signer.with_chain_id(Some(chain_id));
let address = signer.address();
let sig = signer.sign_message(message).await?;
signer.sign_transaction(&mut tx).await?;
```

Important difference: Alloy transaction signing works over mutable signable transaction types rather than ethers `TypedTransaction`.

### Transaction request and typed transactions

Ethers:

```rust
ethers::types::transaction::eip2718::TypedTransaction
ethers::types::TransactionRequest
```

Alloy:

```rust
alloy_rpc_types_eth::TransactionRequest
alloy_consensus::{TxLegacy, TxEip2930, TxEip1559, TxEip4844, TxEip7702, TxEnvelope, TypedTransaction}
```

Recommended Artemis split:

- Use `TransactionRequest` for normal provider-filled/sent mempool transactions.
- Use Alloy consensus/envelope types where Artemis needs to sign and encode bundle transactions manually.

### Contract bindings and Abigen

Current ethers generated bindings use:

```rust
ethers::contract::Contract
ethers::contract::ContractFactory
ethers::contract::builders::*
ethers::providers::Middleware
ethers::contract::{EthError, EthDisplay, EthEvent}
```

Alloy replacement:

```rust
alloy::sol!
#[sol(rpc)]
#[sol(bytecode = "...")]
alloy_contract::CallBuilder
```

Pattern:

```rust
use alloy::sol;

sol! {
    #[sol(rpc)]
    #[sol(bytecode = "0x...")]
    contract MyContract {
        constructor(address owner);
        function doStuff(uint256 value) external returns (bytes32);
    }
}

let contract = MyContract::new(address, &provider);
let result = contract.doStuff(value).call().await?;
let pending = contract.doStuff(value).send().await?;
```

Do not manually port generated ethers bindings. Regenerate or replace them strategy-by-strategy with Alloy `sol!` interfaces and keep generated churn isolated.

### Flashbots and MEV RPCs

Current ethers path:

```rust
ethers_flashbots::{BundleRequest, FlashbotsMiddleware}
FlashbotsMiddleware::new(client, relay_url, relay_signer)
fb_client.simulate_bundle(&bundle)
fb_client.send_bundle(&bundle)
```

Alloy path:

```rust
use alloy_provider::ext::MevApi;
use alloy_rpc_types_mev::{EthCallBundle, EthSendBundle};

provider.call_bundle(call_bundle).await?;
provider.send_bundle(send_bundle).await?;
```

Authentication support is available under Alloy provider MEV auth helpers, including Flashbots signature header generation.

### MEV-Share

Current Artemis uses `mev-share = "0.1.4"`, `FlashbotsSignerLayer`, `MevApiClient`, and ethers signer traits.

Replacement direction:

- Prefer Alloy MEV provider extension:

```rust
provider.send_mev_bundle(mev_bundle).await?;
```

- Use `alloy_rpc_types_mev::MevSendBundle` where schemas match.
- If SSE event collection remains needed, either keep an isolated non-ethers SSE crate if possible or implement reqwest/eventsource collection using Alloy primitive response models.

### State override middleware

Current ethers:

```rust
ethers::providers::spoof::State
custom Middleware wrapper
CallBuilder::new(...).state(&self.state)
```

Alloy direction:

Provider calls use `EthCall::overrides(...)` / `overrides_opt(...)`:

```rust
use alloy_rpc_types_eth::state::StateOverride;

provider.call(tx).overrides(overrides).await?
```

Alloy contract call builders expose state override support with `CallBuilder::state(...)`, so Artemis likely does not need a direct custom `Middleware` analog. Keep the provider-call and contract-call APIs distinct when porting call sites.

## Proposed implementation order

1. **Normalize Alloy crate versions**
   - Move workspace to a coherent Alloy crate family.
   - Avoid mixing old `alloy-rpc-types-eth = 0.2.x` with modern Alloy where possible.

2. **Core provider boundary**
   - Migrate `NewBlock`, log events, transaction events, filters, and mempool executor request types to Alloy RPC/primitives.
   - Introduce compatibility helpers only at strategy/generated-binding boundaries.

3. **Collectors**
   - `BlockCollector`: `Provider::subscribe_blocks()` / `subscribe_full_blocks()`.
   - `LogCollector`: `Provider::subscribe_logs(&Filter)`.
   - `MempoolCollector`: `subscribe_full_pending_transactions()` or hashes + `get_transaction_by_hash()` fallback.

4. **Mempool executor**
   - Use Alloy `TransactionRequest`, `estimate_gas`, `get_gas_price`, and `send_transaction`/`send_raw_transaction`.

5. **Relay and bundle executors**
   - Replace `ethers-flashbots` with Alloy MEV API types.
   - Replace or isolate `mev-share` ethers-signer dependency.
   - Migrate Chainbound transaction signing to Alloy signers/envelopes.

6. **Generated bindings**
   - Regenerate `mev-share-uni-arb` bindings with Alloy `sol!` first, because it is smaller.
   - Then migrate `opensea-sudo-arb` bindings.

7. **Binaries, examples, and generator**
   - Update CLI/main provider construction and wallet parsing.
   - Update generator templates to emit Alloy-based dependencies and code.

8. **Remove ethers workspace dependencies**
   - Only after all non-generated and generated usages are gone.
   - Final acceptance: `git grep -n 'ethers' -- '*.rs' '*.toml'` returns no dependency/code references except historical docs if intentionally retained.

## Acceptance checks for full migration

- `cargo test --all`
- `cargo clippy --all --all-features -- -D warnings`
- `cargo check --workspace`
- `git grep` confirms no active ethers-rs code/dependency references remain.
- Generated bindings are Alloy-based or replaced by checked-in Alloy interface modules.
- Existing runtime behavior is covered by tests or compile-time type assertions at every migrated boundary.
