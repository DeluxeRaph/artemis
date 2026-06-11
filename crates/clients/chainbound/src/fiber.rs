use alloy::rpc::types::Transaction as AlloyTransaction;
use anyhow::Result;
use async_trait::async_trait;
use fiber::{
    eth::{CompactBeaconBlock, ExecutionPayload, ExecutionPayloadHeader},
    Client,
};
use futures::StreamExt;

use artemis_core::types::{Collector, CollectorStream};

const FIBER_DEFAULT_URL: &str = "beta.fiberapi.io:8080";

/// Alloy RPC transaction emitted by the Fiber transaction stream.
pub type FiberTransaction = AlloyTransaction;

/// Possible events emitted by the Fiber collector.
#[derive(Clone, Debug)]
#[allow(clippy::large_enum_variant)]
#[allow(missing_docs)]
pub enum Event {
    Transaction(FiberTransaction),
    TransactionConversionError(String),
    ExectionHeader(ExecutionPayloadHeader),
    ExecutionPayload(ExecutionPayload),
    BeaconBlock(CompactBeaconBlock),
}

/// Fiber collector stream type, used to specify which stream to subscribe to.
pub enum StreamType {
    /// Subscribe to new pending transactions as seen by the Fiber network.
    Transactions,
    /// Subscribe to new [ExecutionPayloadHeader]s, which contain the block header without the
    /// transaction objects. This stream is (on avg) 20-30ms faster than the [StreamType::ExecutionPayloads].
    ExecutionHeaders,
    /// Subscribe to new [ExecutionPayload]s.
    ///
    /// This stream currently exposes upstream Fiber payload types. `fiber-rs` generates these
    /// payload structs from protobuf and still embeds its own transaction representation there.
    /// Prefer [StreamType::Transactions] when the Artemis-facing transaction surface should be
    /// Alloy-native.
    ExecutionPayloads,
    /// Subscribe to new [CompactBeaconBlock]s, which contain the consensus-layer block info.
    /// Refer to the official [Fiber-rs client types](https://github.com/chainbound/fiber-rs/blob/c2f28b28250d52ebb6591d7517e55ead98c041d0/src/eth.rs#L173)
    /// for more info on the streamed objects.
    BeaconBlocks,
}

/// A Fiber collector that subscribes to the specified stream type.
pub struct FiberCollector {
    /// The Fiber-rs client
    client: Client,
    /// The Fiber API key
    api_key: String,
    /// The type of stream to subscribe to
    ty: StreamType,
}

impl FiberCollector {
    /// Initialize a new Fiber collector.
    ///
    /// ## Arguments
    /// - `api_key`: The Fiber API key to use
    /// - `ty`: The type of stream to subscribe to
    pub async fn new(api_key: String, ty: StreamType) -> Self {
        let client = Client::connect(FIBER_DEFAULT_URL.into(), api_key.clone())
            .await
            .expect("failed to connect to Fiber");

        Self {
            client,
            api_key,
            ty,
        }
    }

    /// Optionally set the Fiber endpoint, overriding the default
    pub async fn set_fiber_endpoint(&mut self, endpoint: impl Into<String>) {
        self.client = Client::connect(endpoint.into(), self.api_key.clone())
            .await
            .expect("failed to connect to Fiber");
    }

    /// Get the event stream for the specified stream type.
    pub async fn get_event_stream(&self) -> Result<CollectorStream<'_, Event>> {
        match self.ty {
            StreamType::Transactions => {
                let stream = self.client.subscribe_new_txs(None).await;
                let stream = stream.map(|tx| match fiber_transaction_to_alloy(tx) {
                    Ok(tx) => Event::Transaction(tx),
                    Err(err) => Event::TransactionConversionError(err.to_string()),
                });
                Ok(Box::pin(stream))
            }
            StreamType::ExecutionHeaders => {
                let stream = self.client.subscribe_new_execution_headers().await;
                let stream = stream.map(Event::ExectionHeader);
                Ok(Box::pin(stream))
            }
            StreamType::ExecutionPayloads => {
                let stream = self.client.subscribe_new_execution_payloads().await;
                let stream = stream.map(Event::ExecutionPayload);
                Ok(Box::pin(stream))
            }
            StreamType::BeaconBlocks => {
                let stream = self.client.subscribe_new_beacon_blocks().await;
                let stream = stream.map(Event::BeaconBlock);
                Ok(Box::pin(stream))
            }
        }
    }
}

#[async_trait]
impl Collector<Event> for FiberCollector {
    async fn get_event_stream<'a>(&'a self) -> Result<CollectorStream<'a, Event>> {
        self.get_event_stream().await
    }
}

fn fiber_transaction_to_alloy<T>(tx: T) -> Result<FiberTransaction>
where
    T: serde::Serialize,
{
    let value = serde_json::to_value(tx)?;
    serde_json::from_value(value)
        .map_err(|err| anyhow::anyhow!("failed to convert Fiber transaction to Alloy: {err}"))
}

#[cfg(test)]
mod tests {
    use alloy::{
        consensus::Transaction as _,
        network::TransactionResponse as _,
        primitives::{address, b256, bytes, U256},
    };
    use anyhow::Result;
    use artemis_core::engine::Engine;
    use serde_json::json;

    use crate::Action;
    use crate::Event;
    use crate::FiberCollector;
    use crate::StreamType;

    #[tokio::test]
    async fn test_fiber_collector_txs() -> Result<()> {
        if let Ok(api_key) = std::env::var("FIBER_TEST_KEY") {
            let fiber_collector = FiberCollector::new(api_key, StreamType::Transactions).await;

            let mut engine: Engine<Event, Action> = Engine::default();
            engine.add_collector(Box::new(fiber_collector));

            if let Ok(mut set) = engine.run().await {
                while let Some(res) = set.join_next().await {
                    println!("res: {:?}", res);
                }
            }
        } else {
            println!("Skipping Fiber test, no API key found in FIBER_TEST_KEY env var");
        }

        Ok(())
    }

    #[test]
    fn fiber_transaction_conversion_preserves_legacy_fields() -> Result<()> {
        let upstream_tx = json!({
            "hash": "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "nonce": "0x7",
            "blockHash": "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "blockNumber": "0x2a",
            "transactionIndex": "0x3",
            "from": "0x1111111111111111111111111111111111111111",
            "to": "0x2222222222222222222222222222222222222222",
            "value": "0x4d2",
            "gasPrice": "0x59682f00",
            "gas": "0x5208",
            "input": "0xdeadbeef",
            "v": "0x25",
            "r": "0x1",
            "s": "0x2",
            "chainId": "0x1"
        });

        let alloy_tx = super::fiber_transaction_to_alloy(upstream_tx)?;

        assert_eq!(
            alloy_tx.tx_hash(),
            b256!("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
        );
        assert_eq!(
            alloy_tx.from(),
            address!("1111111111111111111111111111111111111111")
        );
        assert_eq!(
            alloy_tx.to(),
            Some(address!("2222222222222222222222222222222222222222"))
        );
        assert_eq!(alloy_tx.value(), U256::from(1234));
        assert_eq!(
            alloy::consensus::Transaction::gas_price(&alloy_tx),
            Some(1_500_000_000)
        );
        assert_eq!(alloy_tx.gas_limit(), 21_000);
        assert_eq!(alloy_tx.nonce(), 7);
        assert_eq!(alloy_tx.chain_id(), Some(1));
        assert_eq!(alloy_tx.input().as_ref(), bytes!("deadbeef").as_ref());
        assert_eq!(alloy_tx.block_number, Some(42));
        assert_eq!(alloy_tx.transaction_index, Some(3));

        Ok(())
    }

    #[test]
    fn fiber_transaction_conversion_preserves_eip1559_fields() -> Result<()> {
        let upstream_tx = json!({
            "hash": "0xcccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
            "nonce": "0x8",
            "from": "0x1111111111111111111111111111111111111111",
            "to": "0x2222222222222222222222222222222222222222",
            "value": "0x162e",
            "gas": "0x7530",
            "input": "0xc0ffee",
            "v": "0x1",
            "r": "0x3",
            "s": "0x4",
            "type": "0x2",
            "accessList": [{
                "address": "0x3333333333333333333333333333333333333333",
                "storageKeys": [
                    "0x0000000000000000000000000000000000000000000000000000000000000001"
                ]
            }],
            "maxPriorityFeePerGas": "0x3b9aca00",
            "maxFeePerGas": "0x77359400",
            "chainId": "0x1"
        });

        let alloy_tx = super::fiber_transaction_to_alloy(upstream_tx)?;

        assert_eq!(
            alloy_tx.tx_hash(),
            b256!("cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc")
        );
        assert_eq!(alloy_tx.transaction_type(), Some(2));
        assert_eq!(alloy_tx.max_priority_fee_per_gas(), Some(1_000_000_000));
        assert_eq!(
            alloy::consensus::Transaction::max_fee_per_gas(&alloy_tx),
            2_000_000_000
        );
        assert_eq!(alloy_tx.gas_limit(), 30_000);
        assert_eq!(alloy_tx.value(), U256::from(5678));
        assert_eq!(alloy_tx.chain_id(), Some(1));
        assert_eq!(alloy_tx.input().as_ref(), bytes!("c0ffee").as_ref());
        assert_eq!(alloy_tx.access_list().map(|list| list.len()), Some(1));

        Ok(())
    }
}
