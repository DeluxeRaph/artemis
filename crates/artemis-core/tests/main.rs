use alloy::{
    consensus::Transaction as _,
    node_bindings::{Anvil, AnvilInstance},
    primitives::{B256, U256},
    providers::{Provider, ProviderBuilder},
    rpc::types::TransactionRequest,
};
use artemis_core::{
    collectors::{block_collector::BlockCollector, mempool_collector::MempoolCollector},
    executors::mempool_executor::{MempoolExecutor, SubmitTxToMempool},
    types::{Collector, Executor},
};
use std::sync::Arc;
use tokio::time::{sleep, Duration};
use tokio_stream::StreamExt;

/// Spawns Anvil and instantiates a WS Alloy provider.
pub async fn spawn_anvil() -> (impl Provider, AnvilInstance) {
    let anvil = Anvil::new().block_time(1u64).spawn();
    let provider = ProviderBuilder::new()
        .connect(&anvil.ws_endpoint())
        .await
        .unwrap();
    (provider, anvil)
}

/// Test that block collector correctly emits blocks.
#[tokio::test]
async fn test_block_collector_sends_blocks() {
    let (provider, _anvil) = spawn_anvil().await;
    let provider = Arc::new(provider);
    let block_collector = BlockCollector::new(provider.clone());
    let mut block_stream = block_collector.get_event_stream().await.unwrap();
    let block_a = block_stream.next().await.unwrap();

    assert_ne!(block_a.hash, B256::ZERO);
    assert!(block_a.number > 0);
}

/// Test that mempool collector correctly emits transactions.
#[tokio::test]
async fn test_mempool_collector_sends_txs() {
    let (provider, _anvil) = spawn_anvil().await;
    let provider = Arc::new(provider);
    let mempool_collector = MempoolCollector::new(provider.clone());
    let mut mempool_stream = mempool_collector.get_event_stream().await.unwrap();

    let account = provider.get_accounts().await.unwrap()[0];
    let value: u64 = 42;
    let gas_price = 100_000_000_000_000_000u128;
    let tx = TransactionRequest::default()
        .to(account)
        .from(account)
        .value(U256::from(value))
        .gas_price(gas_price);

    let _ = provider.send_transaction(tx).await.unwrap();
    let tx = mempool_stream.next().await.unwrap();
    assert_eq!(tx.value(), U256::from(value));
}

/// Test that the mempool executor correctly sends txs.
#[tokio::test]
async fn test_mempool_executor_sends_tx_simple() {
    let (provider, _anvil) = spawn_anvil().await;
    let provider = Arc::new(provider);
    let mempool_executor = MempoolExecutor::new(provider.clone());

    let account = provider.get_accounts().await.unwrap()[0];
    let value: u64 = 42;
    let gas_price = 100_000_000_000_000_000u128;
    let tx = TransactionRequest::default()
        .to(account)
        .from(account)
        .value(U256::from(value))
        .gas_price(gas_price);
    let action = SubmitTxToMempool {
        tx,
        gas_bid_info: None,
    };
    mempool_executor.execute(action).await.unwrap();
    sleep(Duration::from_secs(2)).await;
    let tx_count = provider.get_transaction_count(account).await.unwrap();
    assert_eq!(tx_count, 1);
}
