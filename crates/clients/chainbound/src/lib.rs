#![warn(missing_docs, unreachable_pub)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]

//! # Chainbound Artemis
//!
//! This crate gives you access to the [Chainbound][chainbound] suite of tools & services for MEV.
//! It is built directly into the [Artemis][artemis] framework for seamless integration with your existing
//! trading strategies.
//!
//! This crate offers two main components, which are implemented following the standard Artemis traits:
//!
//! - Fiber Collector: a low-latency, reliable `mempool` and `new_blocks` stream for Ethereum.
//! - Echo Executor: a feature-rich RPC endpoint to propagate your MEV bundles to block builders.
//!
//! Please refer to the crate README file for an example on how to use these components.

/// Fiber Network client module
pub mod fiber;
pub use fiber::{Event, FiberCollector, StreamType};

/// Echo RPC client module
pub mod echo;
pub use echo::{Action, EchoExecutor};

/// MEV bundle helper types
pub mod mev_bundle;
pub use mev_bundle::{BlockBuilder, BundleNotification, SendBundleArgs, SendBundleResponse};

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use alloy::{
        network::EthereumWallet,
        primitives::{Address, U256},
        providers::{Provider, ProviderBuilder, WsConnect},
        rpc::types::TransactionRequest,
        signers::local::PrivateKeySigner,
    };
    use artemis_core::types::Executor;
    use futures::StreamExt;

    use crate::{BlockBuilder, EchoExecutor, Event, FiberCollector, SendBundleArgs, StreamType};

    #[tokio::test]
    async fn test_chainbound_client() {
        if let Ok(api_key) = std::env::var("FIBER_TEST_KEY") {
            // ==== Open a Fiber transaction stream, and verify that we receive transactions ====

            let ty = StreamType::Transactions;
            let fiber_tx_collector = FiberCollector::new(api_key.clone(), ty).await;
            let fiber_tx_stream = fiber_tx_collector.get_event_stream().await.unwrap();
            let fiber_tx = fiber_tx_stream.into_future().await.0.unwrap();
            assert!(matches!(fiber_tx, Event::Transaction(_)));

            // ==== Create an Echo Executor, and send a random bundle to block builders ====

            let provider = Arc::new(
                ProviderBuilder::new()
                    .connect_ws(WsConnect::new("wss://eth.llamarpc.com"))
                    .await
                    .unwrap(),
            );
            let tx_signer = PrivateKeySigner::random();
            let auth_signer = PrivateKeySigner::random();
            let account = Address::from_slice(tx_signer.address().as_slice());

            let echo_executor = EchoExecutor::new(
                provider,
                EthereumWallet::new(tx_signer),
                auth_signer,
                api_key,
            );

            // Fill in the bundle with a random transaction
            let tx = TransactionRequest::default()
                .to(account)
                .from(account)
                .value(U256::from(42))
                .gas_price(100000000000000000);

            // Set the block as the next one
            let next_block = echo_executor.provider().get_block_number().await.unwrap() + 1;

            // Build the bundle with the selected transaction and options.
            // Look at the `SendBundleArgs` struct for info on available methods.
            let mut bundle = SendBundleArgs::with_txs(vec![tx]);
            bundle.set_block_number(next_block);
            bundle.set_mev_builders(vec![BlockBuilder::Flashbots, BlockBuilder::Titan]);
            bundle.set_replacement_uuid("a34daefc-e640-48fc-a1c7-352fc518720f".to_string());
            bundle.set_refund_percent(90);
            bundle.set_refund_index(0);

            if let Err(e) = echo_executor.execute(bundle).await {
                panic!("Failed to send bundle: {}", e);
            }
        } else {
            println!("Skipping test_chainbound_clients because FIBER_TEST_KEY is not set");
        }
    }
}
