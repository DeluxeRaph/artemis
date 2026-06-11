use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;

use alloy::consensus::{SignableTransaction, TxLegacy};
use alloy::eips::eip2718::Encodable2718;
use alloy::network::{TransactionBuilder, TxSigner};
use alloy::primitives::{
    Address as AlloyAddress, Bytes as AlloyBytes, Signature, U256 as AlloyU256,
};
use alloy::providers::Provider;
use alloy::rpc::types::TransactionRequest;
use alloy::sol;
use alloy::sol_types::SolCall;
use anyhow::Result;
use artemis_core::types::Strategy;
use mev_share::rpc::{BundleItem, Inclusion, SendBundleRequest};
use tracing::info;

use crate::types::V2V3PoolRecord;

use super::types::{Action, Event};

sol! {
    interface BlindArb {
        function executeArb__WETH_token0(
            address v2Pair,
            address v3Pair,
            uint256 amountIn,
            uint256 percentageToPayToCoinbase
        );

        function executeArb__WETH_token1(
            address v2Pair,
            address v3Pair,
            uint256 amountIn,
            uint256 percentageToPayToCoinbase
        );
    }
}

/// Information about a uniswap v2 pool.
#[derive(Debug, Clone)]
pub struct V2PoolInfo {
    /// Address of the v2 pool.
    pub v2_pool: AlloyAddress,
    /// Whether the pool has weth as token0.
    pub is_weth_token0: bool,
}

#[derive(Debug, Clone)]
pub struct MevShareUniArb<M, S> {
    /// Alloy provider.
    client: Arc<M>,
    /// Maps uni v3 pool address to v2 pool information.
    pool_map: HashMap<AlloyAddress, V2PoolInfo>,
    /// Signer for transactions.
    tx_signer: S,
    /// Arb contract address.
    arb_contract_address: AlloyAddress,
}

struct SignedArbParams {
    v2_pool: AlloyAddress,
    v3_address: AlloyAddress,
    size: AlloyU256,
    payment_percentage: AlloyU256,
    is_weth_token0: bool,
    bid_gas_price: u128,
    chain_id: u64,
}

impl<M: Provider + 'static, S: TxSigner<Signature>> MevShareUniArb<M, S> {
    /// Create a new instance of the strategy.
    pub fn new(client: Arc<M>, signer: S, arb_contract_address: AlloyAddress) -> Self {
        Self {
            client: client.clone(),
            pool_map: HashMap::new(),
            tx_signer: signer,
            arb_contract_address,
        }
    }
}

#[async_trait]
impl<M: Provider + 'static, S: TxSigner<Signature> + Send + Sync + 'static> Strategy<Event, Action>
    for MevShareUniArb<M, S>
{
    /// Initialize the strategy. This is called once at startup, and loads
    /// pool information into memory.
    async fn sync_state(&mut self) -> Result<()> {
        // Read pool information from csv file.
        let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        path.push("resources/v3_v2_pools.csv");
        let mut reader = csv::Reader::from_path(path)?;

        for record in reader.deserialize() {
            // Parse records into PoolRecord struct.
            let record: V2V3PoolRecord = record?;
            self.pool_map.insert(
                record.v3_pool,
                V2PoolInfo {
                    v2_pool: record.v2_pool,
                    is_weth_token0: record.weth_token0,
                },
            );
        }

        Ok(())
    }

    // Process incoming events, seeing if we can arb new orders.
    async fn process_event(&mut self, event: Event) -> Vec<Action> {
        match event {
            Event::MEVShareEvent(event) => {
                info!("Received mev share event: {:?}", event);
                // skip if event has no logs
                if event.logs.is_empty() {
                    return vec![];
                }
                let address = alloy_address_from_mev(event.logs[0].address.as_fixed_bytes());
                // skip if address is not a v3 pool
                if !self.pool_map.contains_key(&address) {
                    return vec![];
                }
                // if it's a v3 pool we care about, submit bundles
                info!(
                    "Found a v3 pool match at address {:?}, submitting bundles",
                    address
                );
                self.generate_bundles(address, &event)
                    .await
                    .into_iter()
                    .map(Action::SubmitBundle)
                    .collect()
            }
        }
    }
}

impl<M: Provider + 'static, S: TxSigner<Signature> + Send + Sync + 'static> MevShareUniArb<M, S> {
    /// Generate a series of bundles of varying sizes to submit to the matchmaker.
    pub async fn generate_bundles(
        &self,
        v3_address: AlloyAddress,
        event: &mev_share::sse::Event,
    ) -> Vec<SendBundleRequest> {
        let mut bundles = Vec::new();
        let v2_info = self.pool_map.get(&v3_address).unwrap();

        // The sizes of the backruns we want to submit.
        // TODO: Run some analysis to figure out likely sizes.
        let sizes = vec![
            AlloyU256::from(100000_u128),
            AlloyU256::from(1000000_u128),
            AlloyU256::from(10000000_u128),
            AlloyU256::from(100000000_u128),
            AlloyU256::from(1000000000_u128),
            AlloyU256::from(10000000000_u128),
            AlloyU256::from(100000000000_u128),
            AlloyU256::from(1000000000000_u128),
            AlloyU256::from(10000000000000_u128),
            AlloyU256::from(100000000000000_u128),
            AlloyU256::from(1000000000000000_u128),
            AlloyU256::from(10000000000000000_u128),
            AlloyU256::from(100000000000000000_u128),
            AlloyU256::from(1000000000000000000_u128),
        ];

        // Set parameters for the backruns.
        let payment_percentage = AlloyU256::ZERO;
        let bid_gas_price = self.client.get_gas_price().await.unwrap();
        let block_num = self.client.get_block_number().await.unwrap();
        let chain_id = self.client.get_chain_id().await.unwrap();

        for size in sizes {
            let arb_tx = match self
                .build_signed_arb_transaction(SignedArbParams {
                    v2_pool: v2_info.v2_pool,
                    v3_address,
                    size,
                    payment_percentage,
                    is_weth_token0: v2_info.is_weth_token0,
                    bid_gas_price,
                    chain_id,
                })
                .await
            {
                Ok(tx) => tx,
                Err(e) => {
                    println!("Error building signed tx: {}", e);
                    continue;
                }
            };
            info!("generated signed arb tx: {:?}", arb_tx);

            let txs = vec![
                BundleItem::Hash { hash: event.hash },
                BundleItem::Tx {
                    tx: arb_tx.to_vec().into(),
                    can_revert: false,
                },
            ];
            let bundle = SendBundleRequest {
                bundle_body: txs,
                inclusion: Inclusion {
                    block: (block_num + 1).into(),
                    // set a large validity window to ensure builder gets a chance to include bundle.
                    max_block: Some((block_num + 30).into()),
                },
                ..Default::default()
            };
            info!("submitting bundle: {:?}", bundle);
            bundles.push(bundle);
        }
        bundles
    }

    async fn build_signed_arb_transaction(&self, params: SignedArbParams) -> Result<AlloyBytes> {
        let nonce = self
            .client
            .get_transaction_count(self.tx_signer.address())
            .await?;
        let mut arb_tx = {
            // Construct arb tx based on whether the v2 pool has weth as token0.
            let inner = build_arb_transaction(
                self.arb_contract_address,
                params.v2_pool,
                params.v3_address,
                params.size,
                params.payment_percentage,
                params.is_weth_token0,
            )
            .gas_limit(400000)
            .gas_price(params.bid_gas_price)
            .nonce(nonce)
            .with_chain_id(params.chain_id);

            tx_request_to_legacy(inner)?
        };

        let signature = self.tx_signer.sign_transaction(&mut arb_tx).await?;
        Ok(arb_tx.into_signed(signature).encoded_2718().into())
    }
}

fn build_arb_transaction(
    arb_contract_address: AlloyAddress,
    v2_pool: AlloyAddress,
    v3_pool: AlloyAddress,
    amount_in: AlloyU256,
    payment_percentage: AlloyU256,
    is_weth_token0: bool,
) -> TransactionRequest {
    TransactionRequest::default()
        .with_to(arb_contract_address)
        .with_input(encode_arb_call(
            v2_pool,
            v3_pool,
            amount_in,
            payment_percentage,
            is_weth_token0,
        ))
}

fn encode_arb_call(
    v2_pool: AlloyAddress,
    v3_pool: AlloyAddress,
    amount_in: AlloyU256,
    payment_percentage: AlloyU256,
    is_weth_token0: bool,
) -> AlloyBytes {
    let encoded = if is_weth_token0 {
        BlindArb::executeArb__WETH_token0Call {
            v2Pair: v2_pool,
            v3Pair: v3_pool,
            amountIn: amount_in,
            percentageToPayToCoinbase: payment_percentage,
        }
        .abi_encode()
    } else {
        BlindArb::executeArb__WETH_token1Call {
            v2Pair: v2_pool,
            v3Pair: v3_pool,
            amountIn: amount_in,
            percentageToPayToCoinbase: payment_percentage,
        }
        .abi_encode()
    };

    AlloyBytes::from(encoded).to_vec().into()
}

fn tx_request_to_legacy(tx: TransactionRequest) -> Result<TxLegacy> {
    Ok(tx.build_legacy()?)
}

fn alloy_address_from_mev(value: &[u8; 20]) -> AlloyAddress {
    AlloyAddress::from_slice(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::TxKind;

    fn alloy_test_address(byte: u8) -> AlloyAddress {
        AlloyAddress::repeat_byte(byte)
    }

    fn assert_arb_call(
        encoded: AlloyBytes,
        selector: [u8; 4],
        v2_byte: u8,
        v3_byte: u8,
        amount_in: AlloyU256,
        payment_percentage: AlloyU256,
    ) {
        assert_eq!(&encoded[..4], selector.as_slice());
        assert_eq!(&encoded[4..16], [0_u8; 12]);
        assert_eq!(&encoded[16..36], [v2_byte; 20]);
        assert_eq!(&encoded[36..48], [0_u8; 12]);
        assert_eq!(&encoded[48..68], [v3_byte; 20]);
        assert_eq!(&encoded[68..100], amount_in.to_be_bytes::<32>().as_slice());
        assert_eq!(
            &encoded[100..132],
            payment_percentage.to_be_bytes::<32>().as_slice()
        );
    }

    #[test]
    fn alloy_encoder_preserves_token0_call_fields() {
        let v2_pool = alloy_test_address(0x11);
        let v3_pool = alloy_test_address(0x22);
        let amount_in = AlloyU256::from(1000000000000000000_u128);
        let payment_percentage = AlloyU256::from(5_u64);

        let encoded = encode_arb_call(v2_pool, v3_pool, amount_in, payment_percentage, true);
        assert_arb_call(
            encoded,
            [0x43, 0x3f, 0x1e, 0x90],
            0x11,
            0x22,
            amount_in,
            payment_percentage,
        );
    }

    #[test]
    fn alloy_encoder_preserves_token1_call_fields() {
        let v2_pool = alloy_test_address(0x33);
        let v3_pool = alloy_test_address(0x44);
        let amount_in = AlloyU256::from(123456789_u64);
        let payment_percentage = AlloyU256::ZERO;

        let encoded = encode_arb_call(v2_pool, v3_pool, amount_in, payment_percentage, false);
        assert_arb_call(
            encoded,
            [0x65, 0xc8, 0x05, 0x3b],
            0x33,
            0x44,
            amount_in,
            payment_percentage,
        );
    }

    #[test]
    fn build_arb_transaction_preserves_to_and_data_only() {
        let arb_contract = alloy_test_address(0xaa);
        let v2_pool = alloy_test_address(0xbb);
        let v3_pool = alloy_test_address(0xcc);
        let amount_in = AlloyU256::from(42_u64);
        let payment_percentage = AlloyU256::from(7_u64);

        let tx = build_arb_transaction(
            arb_contract,
            v2_pool,
            v3_pool,
            amount_in,
            payment_percentage,
            true,
        );

        assert_eq!(
            tx.to,
            Some(TxKind::Call(arb_contract)),
            "arb contract target must be preserved"
        );
        assert_eq!(
            tx.input.input().unwrap().as_ref(),
            encode_arb_call(v2_pool, v3_pool, amount_in, payment_percentage, true).as_ref(),
            "calldata must be preserved"
        );
        assert!(tx.gas.is_none(), "gas is filled after tx construction");
        assert!(
            tx.gas_price.is_none(),
            "gas price is filled after tx construction"
        );
        assert!(
            tx.access_list.is_none(),
            "legacy transaction should not silently carry an access list"
        );
    }

    #[test]
    fn tx_request_to_legacy_preserves_filled_fields() {
        let to = alloy_test_address(0xaa);
        let input = AlloyBytes::from(vec![1, 2, 3]);
        let tx = TransactionRequest::default()
            .with_to(to)
            .with_input(input.clone())
            .gas_limit(400000)
            .gas_price(10)
            .nonce(3)
            .with_chain_id(1);

        let legacy = tx_request_to_legacy(tx).unwrap();

        assert_eq!(legacy.to, TxKind::Call(to));
        assert_eq!(legacy.input, input);
        assert_eq!(legacy.gas_limit, 400000);
        assert_eq!(legacy.gas_price, 10);
        assert_eq!(legacy.nonce, 3);
        assert_eq!(legacy.chain_id, Some(1));
        assert_eq!(legacy.value, AlloyU256::ZERO);
    }
}
