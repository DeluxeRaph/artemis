use alloy::rpc::types::Transaction as AlloyTransaction;
use anyhow::Result;
use async_trait::async_trait;
use ethers::types::Transaction as EthersTransaction;
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

fn fiber_transaction_to_alloy(tx: EthersTransaction) -> Result<FiberTransaction> {
    let tx_hash = tx.hash;
    let value = serde_json::to_value(tx)?;
    serde_json::from_value(value).map_err(|err| {
        anyhow::anyhow!("failed to convert Fiber transaction {tx_hash:#x} to Alloy: {err}")
    })
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
    use ethers::types::{
        transaction::eip2930::{AccessList, AccessListItem},
        Action, Address as EthersAddress, Bytes as EthersBytes, Transaction as EthersTransaction,
        H256 as EthersH256, U256 as EthersU256, U64,
    };

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
        let ethers_tx = EthersTransaction {
            hash: EthersH256::from_slice(
                b256!("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
                    .as_slice(),
            ),
            nonce: EthersU256::from(7),
            block_hash: Some(EthersH256::from_slice(
                b256!("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")
                    .as_slice(),
            )),
            block_number: Some(U64::from(42)),
            transaction_index: Some(U64::from(3)),
            from: EthersAddress::from_slice(
                address!("1111111111111111111111111111111111111111").as_slice(),
            ),
            to: Some(EthersAddress::from_slice(
                address!("2222222222222222222222222222222222222222").as_slice(),
            )),
            value: EthersU256::from(1234),
            gas_price: Some(EthersU256::from(1_500_000_000u64)),
            gas: EthersU256::from(21_000),
            input: EthersBytes::from(bytes!("deadbeef").to_vec()),
            v: U64::from(37),
            r: EthersU256::from(1),
            s: EthersU256::from(2),
            transaction_type: None,
            chain_id: Some(EthersU256::from(1)),
            ..Default::default()
        };

        let alloy_tx = super::fiber_transaction_to_alloy(ethers_tx)?;

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
        let ethers_tx = EthersTransaction {
            hash: EthersH256::from_slice(
                b256!("cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc")
                    .as_slice(),
            ),
            nonce: EthersU256::from(8),
            from: EthersAddress::from_slice(
                address!("1111111111111111111111111111111111111111").as_slice(),
            ),
            to: Some(EthersAddress::from_slice(
                address!("2222222222222222222222222222222222222222").as_slice(),
            )),
            value: EthersU256::from(5678),
            gas: EthersU256::from(30_000),
            input: EthersBytes::from(bytes!("c0ffee").to_vec()),
            v: U64::from(1),
            r: EthersU256::from(3),
            s: EthersU256::from(4),
            transaction_type: Some(U64::from(2)),
            access_list: Some(AccessList(vec![AccessListItem {
                address: EthersAddress::from_slice(
                    address!("3333333333333333333333333333333333333333").as_slice(),
                ),
                storage_keys: vec![EthersH256::from_slice(
                    b256!("0000000000000000000000000000000000000000000000000000000000000001")
                        .as_slice(),
                )],
            }])),
            max_priority_fee_per_gas: Some(EthersU256::from(1_000_000_000u64)),
            max_fee_per_gas: Some(EthersU256::from(2_000_000_000u64)),
            chain_id: Some(EthersU256::from(1)),
            ..Default::default()
        };

        let alloy_tx = super::fiber_transaction_to_alloy(ethers_tx)?;

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
