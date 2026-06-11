use std::collections::HashMap;
use std::ops::Add;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;

use alloy::primitives::{Address as AlloyAddress, Bytes as AlloyBytes, U256 as AlloyU256};
use alloy::sol;
use alloy::sol_types::SolCall;
use anyhow::Result;
use artemis_core::types::Strategy;

use ethers::signers::Signer;

use ethers::providers::Middleware;
use ethers::types::transaction::eip2718::TypedTransaction;
use ethers::types::TransactionRequest;
use ethers::types::H256;
use ethers::types::{H160, U256};
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
    /// Ethers client.
    client: Arc<M>,
    /// Maps uni v3 pool address to v2 pool information.
    pool_map: HashMap<AlloyAddress, V2PoolInfo>,
    /// Signer for transactions.
    tx_signer: S,
    /// Arb contract address.
    arb_contract_address: AlloyAddress,
}

impl<M: Middleware + 'static, S: Signer> MevShareUniArb<M, S> {
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
impl<M: Middleware + 'static, S: Signer + 'static> Strategy<Event, Action>
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
                let address = alloy_address(event.logs[0].address);
                // skip if address is not a v3 pool
                if !self.pool_map.contains_key(&address) {
                    return vec![];
                }
                // if it's a v3 pool we care about, submit bundles
                info!(
                    "Found a v3 pool match at address {:?}, submitting bundles",
                    address
                );
                self.generate_bundles(address, event.hash)
                    .await
                    .into_iter()
                    .map(Action::SubmitBundle)
                    .collect()
            }
        }
    }
}

impl<M: Middleware + 'static, S: Signer + 'static> MevShareUniArb<M, S> {
    /// Generate a series of bundles of varying sizes to submit to the matchmaker.
    pub async fn generate_bundles(
        &self,
        v3_address: AlloyAddress,
        tx_hash: H256,
    ) -> Vec<SendBundleRequest> {
        let mut bundles = Vec::new();
        let v2_info = self.pool_map.get(&v3_address).unwrap();

        // The sizes of the backruns we want to submit.
        // TODO: Run some analysis to figure out likely sizes.
        let sizes = vec![
            U256::from(100000_u128),
            U256::from(1000000_u128),
            U256::from(10000000_u128),
            U256::from(100000000_u128),
            U256::from(1000000000_u128),
            U256::from(10000000000_u128),
            U256::from(100000000000_u128),
            U256::from(1000000000000_u128),
            U256::from(10000000000000_u128),
            U256::from(100000000000000_u128),
            U256::from(1000000000000000_u128),
            U256::from(10000000000000000_u128),
            U256::from(100000000000000000_u128),
            U256::from(1000000000000000000_u128),
        ];

        // Set parameters for the backruns.
        let payment_percentage = U256::from(0);
        let bid_gas_price = self.client.get_gas_price().await.unwrap();
        let block_num = self.client.get_block_number().await.unwrap();

        for size in sizes {
            let arb_tx = {
                // Construct arb tx based on whether the v2 pool has weth as token0.
                let mut inner = build_arb_transaction(
                    self.arb_contract_address,
                    v2_info.v2_pool,
                    v3_address,
                    size,
                    payment_percentage,
                    v2_info.is_weth_token0,
                );
                // Set gas parameters (this is a bit hacky)
                inner.set_gas(400000);
                inner.set_gas_price(bid_gas_price);
                let fill = self.client.fill_transaction(&mut inner, None).await;

                match fill {
                    Ok(_) => {}
                    Err(e) => {
                        println!("Error filling tx: {}", e);
                        continue;
                    }
                }

                inner
            };
            info!("generated arb tx: {:?}", arb_tx);

            // Sign tx and construct bundle
            let signature = self.tx_signer.sign_transaction(&arb_tx).await.unwrap();
            let bytes = arb_tx.rlp_signed(&signature);
            let txs = vec![
                BundleItem::Hash { hash: tx_hash },
                BundleItem::Tx {
                    tx: bytes,
                    can_revert: false,
                },
            ];
            let bundle = SendBundleRequest {
                bundle_body: txs,
                inclusion: Inclusion {
                    block: block_num.add(1),
                    // set a large validity window to ensure builder gets a chance to include bundle.
                    max_block: Some(block_num.add(30)),
                },
                ..Default::default()
            };
            info!("submitting bundle: {:?}", bundle);
            bundles.push(bundle);
        }
        bundles
    }
}

fn build_arb_transaction(
    arb_contract_address: AlloyAddress,
    v2_pool: AlloyAddress,
    v3_pool: AlloyAddress,
    amount_in: U256,
    payment_percentage: U256,
    is_weth_token0: bool,
) -> TypedTransaction {
    TransactionRequest::new()
        .to(ethers_address(arb_contract_address))
        .data(encode_arb_call(
            v2_pool,
            v3_pool,
            amount_in,
            payment_percentage,
            is_weth_token0,
        ))
        .into()
}

fn encode_arb_call(
    v2_pool: AlloyAddress,
    v3_pool: AlloyAddress,
    amount_in: U256,
    payment_percentage: U256,
    is_weth_token0: bool,
) -> ethers::types::Bytes {
    let amount_in = alloy_u256(amount_in);
    let payment_percentage = alloy_u256(payment_percentage);

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

fn alloy_address(value: H160) -> AlloyAddress {
    AlloyAddress::from_slice(value.as_fixed_bytes())
}

fn ethers_address(value: AlloyAddress) -> H160 {
    H160::from_slice(value.as_slice())
}

fn alloy_u256(value: U256) -> AlloyU256 {
    let mut bytes = [0_u8; 32];
    value.to_big_endian(&mut bytes);
    AlloyU256::from_be_bytes(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ethers::abi::{Function, Param, ParamType, StateMutability, Token};
    use ethers::types::NameOrAddress;

    fn ethers_test_address(byte: u8) -> H160 {
        H160::repeat_byte(byte)
    }

    fn alloy_test_address(byte: u8) -> AlloyAddress {
        AlloyAddress::repeat_byte(byte)
    }

    #[allow(deprecated)]
    fn arb_function(name: &str) -> Function {
        Function {
            name: name.to_string(),
            inputs: vec![
                Param {
                    name: "v2Pair".to_string(),
                    kind: ParamType::Address,
                    internal_type: None,
                },
                Param {
                    name: "v3Pair".to_string(),
                    kind: ParamType::Address,
                    internal_type: None,
                },
                Param {
                    name: "amountIn".to_string(),
                    kind: ParamType::Uint(256),
                    internal_type: None,
                },
                Param {
                    name: "percentageToPayToCoinbase".to_string(),
                    kind: ParamType::Uint(256),
                    internal_type: None,
                },
            ],
            outputs: vec![],
            constant: None,
            state_mutability: StateMutability::NonPayable,
        }
    }

    #[test]
    fn alloy_encoder_matches_ethers_abi_for_token0() {
        let v2_pool = alloy_test_address(0x11);
        let v3_pool = alloy_test_address(0x22);
        let amount_in = U256::from_dec_str("1000000000000000000").unwrap();
        let payment_percentage = U256::from(5_u64);

        let encoded = encode_arb_call(v2_pool, v3_pool, amount_in, payment_percentage, true);
        let expected = arb_function("executeArb__WETH_token0")
            .encode_input(&[
                Token::Address(ethers_test_address(0x11)),
                Token::Address(ethers_test_address(0x22)),
                Token::Uint(amount_in),
                Token::Uint(payment_percentage),
            ])
            .unwrap();

        assert_eq!(encoded.as_ref(), expected.as_slice());
    }

    #[test]
    fn alloy_encoder_matches_ethers_abi_for_token1() {
        let v2_pool = alloy_test_address(0x33);
        let v3_pool = alloy_test_address(0x44);
        let amount_in = U256::from(123456789_u64);
        let payment_percentage = U256::zero();

        let encoded = encode_arb_call(v2_pool, v3_pool, amount_in, payment_percentage, false);
        let expected = arb_function("executeArb__WETH_token1")
            .encode_input(&[
                Token::Address(ethers_test_address(0x33)),
                Token::Address(ethers_test_address(0x44)),
                Token::Uint(amount_in),
                Token::Uint(payment_percentage),
            ])
            .unwrap();

        assert_eq!(encoded.as_ref(), expected.as_slice());
    }

    #[test]
    fn build_arb_transaction_preserves_to_and_data_only() {
        let arb_contract = alloy_test_address(0xaa);
        let v2_pool = alloy_test_address(0xbb);
        let v3_pool = alloy_test_address(0xcc);
        let amount_in = U256::from(42_u64);
        let payment_percentage = U256::from(7_u64);

        let tx = build_arb_transaction(
            arb_contract,
            v2_pool,
            v3_pool,
            amount_in,
            payment_percentage,
            true,
        );

        assert_eq!(
            tx.to(),
            Some(&NameOrAddress::Address(ethers_address(arb_contract))),
            "arb contract target must be preserved"
        );
        assert_eq!(
            tx.data().unwrap().as_ref(),
            encode_arb_call(v2_pool, v3_pool, amount_in, payment_percentage, true).as_ref(),
            "calldata must be preserved"
        );
        assert!(tx.gas().is_none(), "gas is filled after tx construction");
        assert!(
            tx.gas_price().is_none(),
            "gas price is filled after tx construction"
        );
        assert!(
            tx.access_list().is_none(),
            "legacy transaction should not silently carry an access list"
        );
    }
}
