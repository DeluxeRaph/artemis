# Codex handoff: finish Artemis ethers-rs -> Alloy migration

This repository is being migrated from `ethers-rs` to Alloy so it can be used as a modern base for QuickNode + AgentCash on-chain arbitrage experiments. The immediate goal is to make a large dent in the migration on a machine with more storage, using a main Codex agent plus focused subagents working on separate slices.

## Current branch and PR

- Work branch: `migration`
- Fork remote branch: `DeluxeRaph/artemis:migration`
- Upstream PR: https://github.com/paradigmxyz/artemis/pull/83
- Starting estimate from Hermes before this handoff: roughly `58-60%` complete.
- The previous Linux VPS ran out of comfortable disk because `target/` grew to ~36G while both ethers and Alloy stacks were compiled. On the Mac, do **not** copy `target/`; rebuild locally.

## User goal

Finish as much of the migration as possible, not just one tiny patch. Use subagents for independent workstreams, then have the main agent integrate, compile, test, and resolve cross-slice conflicts.

The user wants progress updates to include:

- what changed
- tests/checks run
- remaining blockers
- estimated percent complete after the slice

## Hard constraints

1. Keep the repo green after every integrated slice.
2. Do not claim a slice is done unless these pass or a real blocker is documented:
   - `cargo +stable check --workspace`
   - relevant targeted tests for the touched crates
   - `cargo +stable test --workspace` when feasible
   - `cargo +stable clippy --all --all-features -- -D warnings` before final handoff if feasible
   - `git diff --check`
3. Do not silently drop transaction fields during conversions. For unsupported fields, fail closed with a clear error/`None` and add regression tests.
4. Avoid mixing old Alloy crate generations with Alloy 2.x. Prefer coherent Alloy 2.x dependencies.
5. Generated binding directories can cause enormous rustfmt churn. Do not reformat generated bindings unless that is the specific slice.
6. Keep compatibility bridges narrow and close to the remaining ethers boundary. Do not spread ethers back into migrated core surfaces.
7. Use tests first for behavioral migration work where practical: write/observe a failing targeted test, implement, then verify pass.

## Environment setup on Mac

Suggested fresh setup:

```bash
git clone https://github.com/DeluxeRaph/artemis.git
cd artemis
git checkout migration
rustup toolchain install stable
rustup default stable
# Foundry is needed for workspace tests that spawn anvil.
curl -L https://foundry.paradigm.xyz | bash
foundryup
# Then ensure anvil/forge are on PATH, e.g. source shell profile or:
export PATH="$HOME/.foundry/bin:$PATH"
```

Do not copy `/root/Projects/artemis/target` from the VPS. It is rebuildable and was the disk-pressure source.

## Migration status already completed

High-level completed slices on `migration` include:

- OpenSea v2 client response/request primitives migrated to Alloy primitives.
- Artemis core provider boundary migrated toward Alloy provider traits.
- Mempool collector compatibility improved to use pending hash subscription + full tx lookup.
- Generator templates updated to use Alloy provider bounds and include Alloy dependencies.
- Chainbound/Echo bundle public APIs moved to Alloy `TransactionRequest` with a private ethers signing bridge.
- MEV-Share relay auth now uses an Alloy signer for the `x-flashbots-signature` path.
- Flashbots bundle public action type now accepts Alloy `TransactionRequest`, with private Alloy -> ethers conversion for current relay submission.
- OpenSea sudo arb strategy bridge now preserves legacy and EIP-1559 fields when converting ethers `TypedTransaction` output from generated bindings into Alloy `TransactionRequest` core actions.

Recent important commits at handoff time:

```text
be6a868 Reject EIP-1559 access lists in arb tx conversion
feaf3c9 Preserve OpenSea arb EIP-1559 bridge fees
2ac5f07 Migrate Flashbots bundle API to Alloy requests
969b56a Migrate MEV-Share relay auth to Alloy signer
4fda5cf Fix explicit Chainbound EIP-1559 bundle conversion
```

## Known remaining blockers / likely big slices

### 1. OpenSea sudo arb generated bindings

Path:

- `crates/strategies/opensea-sudo-arb/`
- `crates/strategies/opensea-sudo-arb/bindings/`

Current state:

- The strategy still relies on ethers Abigen-generated bindings.
- A narrow bridge converts generated ethers transaction output into Alloy `TransactionRequest` for core mempool execution.
- The binding crate is huge and contributes heavily to compile artifact growth.

Suggested subagent assignment:

```text
Subagent A: Replace or narrow OpenSea sudo arb generated bindings.
Goal: Identify the minimal contracts/types the strategy actually uses, then replace broad ethers Abigen output with narrow Alloy `sol!` bindings or a smaller generated surface. Keep the slice green. Preserve behavior with tests around order parameter conversion, quotes, factory events, and tx building.
```

Important files:

- `crates/strategies/opensea-sudo-arb/src/strategy.rs`
- `crates/strategies/opensea-sudo-arb/src/types.rs`
- `crates/strategies/opensea-sudo-arb/bindings/src/sudo_opensea_arb.rs`
- `crates/strategies/opensea-sudo-arb/bindings/src/sudo_pair_quoter.rs`
- `crates/strategies/opensea-sudo-arb/bindings/src/lssvm_pair_factory.rs`

### 2. MEV-share uni arb generated bindings and strategy

Path:

- `crates/strategies/mev-share-uni-arb/`
- `crates/strategies/mev-share-uni-arb/bindings/`

Current state:

- Still ethers-binding based.
- MEV-share upstream APIs are also ethers-shaped, so full removal may require local/forked replacement types or a compatibility bridge.

Suggested subagent assignment:

```text
Subagent B: Migrate MEV-share uni arb strategy surface.
Goal: Map all ethers uses in `mev-share-uni-arb`, decide what can be converted to Alloy now, and isolate unavoidable upstream ethers APIs behind small bridges. Add tests for conversion/value preservation. Keep the crate and workspace compiling.
```

### 3. Relay / bundle executors and upstream ethers dependencies

Paths:

- `crates/artemis-core/src/executors/flashbots_executor.rs`
- `crates/artemis-core/src/executors/mev_share_executor.rs`
- `crates/clients/chainbound/src/echo.rs`
- `examples/mev-share-arb/`

Current state:

- Public/core surfaces are partially Alloy-facing.
- Internally still uses `ethers-flashbots` and some `mev-share`/upstream ethers-shaped APIs.
- Current approach is intentionally transitional: Alloy public request/signature surfaces with private ethers bridges where upstream libraries require it.

Suggested subagent assignment:

```text
Subagent C: Continue relay/bundle executor migration.
Goal: Determine whether Alloy 2 native MEV/Flashbots provider APIs can replace the private ethers bridge without version skew. If not, document the precise upstream blocker and make the bridge safer with fail-closed tests.
```

Important warning:

- Do **not** add old `alloy-mev` or older Alloy-generation crates if they pull Alloy 1.x / old `alloy-consensus`; that can create native-link conflicts with Alloy 2.x (`c-kzg` link conflict was observed previously).

### 4. Dependency graph cleanup

Paths:

- root `Cargo.toml`
- `Cargo.lock`
- all crate `Cargo.toml` files

Current state:

- Root workspace still has `ethers` / `ethers-signers` workspace dependencies.
- Some crates still need them, but migrated crates should not keep stale direct deps.

Suggested subagent assignment:

```text
Subagent D: Dependency cleanup and ethers inventory.
Goal: Produce an accurate crate-by-crate list of remaining ethers dependencies and remove any that are stale after other slices. Keep Cargo.lock coherent and avoid mixed Alloy versions.
```

Useful commands:

```bash
# remaining Rust ethers references, excluding target
grep -RIn --exclude-dir=target 'ethers::\|ethers_' .

# Cargo manifests with ethers dependencies
grep -RIn --include='Cargo.toml' 'ethers' .

# Alloy versions in graph
cargo tree -i alloy
cargo tree -i alloy-primitives
cargo tree -i ethers
```

## Recommended Codex orchestration pattern

Use one main Codex agent as integrator. It should create separate git worktrees for subagents so parallel edits do not collide.

Example:

```bash
# from repo root on migration branch
git fetch origin
git checkout migration
git pull --ff-only origin migration

mkdir -p ../artemis-worktrees

git worktree add -b codex/opensea-bindings ../artemis-worktrees/opensea-bindings migration
git worktree add -b codex/mevshare-strategy ../artemis-worktrees/mevshare-strategy migration
git worktree add -b codex/relay-executors ../artemis-worktrees/relay-executors migration
git worktree add -b codex/dependency-cleanup ../artemis-worktrees/dependency-cleanup migration
```

Then launch subagents with focused prompts, for example:

```bash
codex exec --full-auto "Read AGENTS.md. Work only on the OpenSea sudo arb generated-binding migration slice. Keep tests green. Commit your slice with a clear message and leave a summary of tests run."
```

The main agent should then:

1. Inspect each subagent commit/diff.
2. Cherry-pick or merge one slice at a time into `migration`.
3. Resolve conflicts.
4. Run targeted tests for the slice.
5. Run `cargo +stable check --workspace` after each integration.
6. Run broader tests/clippy before final push.
7. Push `migration` and update PR #83.

Do not integrate all subagent work blindly. Each slice must compile independently after integration.

## Suggested final verification before pushing major progress

```bash
export PATH="$HOME/.foundry/bin:$PATH"
cargo +stable fmt --all
# If generated binding churn appears, revert generated formatting unless intentional.
git diff --check
cargo +stable check --workspace
cargo +stable test --workspace
cargo +stable clippy --all --all-features -- -D warnings
```

If `cargo fmt --all` causes huge generated binding changes, prefer targeted `rustfmt --edition 2021 <touched non-generated files>` and document the stable rustfmt warning.

## Definition of done for this migration push

A strong Codex pass should aim for at least one of these outcomes:

- Major reduction in generated ethers binding usage.
- MEV-share strategy boundary moved to Alloy where possible with remaining upstream blockers isolated.
- Relay/bundle executors using Alloy-native APIs or explicitly documented as blocked by upstream libraries.
- Workspace still passes `cargo check --workspace` and relevant tests.
- Estimated completion clearly updated in the final report.

Ideal final report format to the user:

```text
Summary: <what changed>
Files: <main files touched>
Tests/checks run: <commands and pass/fail>
Remaining blockers: <specific blockers>
Estimated migration completeness: <percent>
Next best slice: <one concrete task>
```
