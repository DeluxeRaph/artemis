use std::{collections::HashMap, sync::Arc};

use async_trait::async_trait;

use alloy::sol_types::{SolCall, SolEvent};
use alloy::{
    primitives::{Address, Bytes, B256, U256},
    providers::Provider,
    rpc::types::{
        state::{AccountOverride, StateOverride},
        Filter, TransactionInput, TransactionRequest,
    },
};
use bindings::sudo_pair_quoter::{SellQuote, SUDOPAIRQUOTER_DEPLOYED_BYTECODE};
use tracing::info;

use crate::constants::FACTORY_DEPLOYMENT_BLOCK;
use crate::types::Config;
use anyhow::{Context, Result};
use artemis_core::collectors::block_collector::NewBlock;
use artemis_core::collectors::opensea_order_collector::OpenseaOrder;
use artemis_core::executors::mempool_executor::{GasBidInfo, SubmitTxToMempool};
use artemis_core::opensea_stream::schema::Chain;
use artemis_core::types::Strategy;
use opensea_v2::client::OpenSeaV2Client;

use super::constants::{LSSVM_PAIR_FACTORY_ADDRESS, POOL_EVENT_SIGNATURES};
use super::types::{
    fulfill_listing_response_to_basic_order_parameters, hash_to_fulfill_listing_request, Action,
    Event,
};

const QUOTER_ADDRESS: Address = Address::repeat_byte(0x42);

#[derive(Debug, Clone)]
pub struct OpenseaSudoArb<M> {
    /// Alloy provider.
    client: Arc<M>,
    /// Opensea V2 client
    opensea_client: OpenSeaV2Client,
    /// LSSVM pair factory contract for getting pair history.
    lssvm_pair_factory_address: Address,
    /// Quoter for batch reading pair state.
    quoter_state: StateOverride,
    /// Address where the quoter bytecode is injected for read calls.
    quoter_address: Address,
    /// Arb contract.
    arb_contract_address: Address,
    /// Map NFT addresses to a list of Sudo pair addresses which trade that NFT.
    sudo_pools: HashMap<Address, Vec<Address>>,
    /// Map Sudo pool addresses to the current bid for that pool (in ETH).
    pool_bids: HashMap<Address, U256>,
    /// Amount of profits to bid in gas
    bid_percentage: u64,
}

impl<M: Provider + 'static> OpenseaSudoArb<M> {
    pub fn new(client: Arc<M>, opensea_client: OpenSeaV2Client, config: Config) -> Self {
        let quoter_address = QUOTER_ADDRESS;
        let quoter_state = [(
            quoter_address,
            AccountOverride::default()
                .with_code(Bytes::from_static(SUDOPAIRQUOTER_DEPLOYED_BYTECODE)),
        )]
        .into_iter()
        .collect();

        Self {
            client,
            opensea_client,
            lssvm_pair_factory_address: LSSVM_PAIR_FACTORY_ADDRESS,
            quoter_state,
            quoter_address,
            arb_contract_address: config.arb_contract_address,
            sudo_pools: HashMap::new(),
            pool_bids: HashMap::new(),
            bid_percentage: config.bid_percentage,
        }
    }
}

#[async_trait]
impl<M: Provider + Send + Sync + 'static> Strategy<Event, Action> for OpenseaSudoArb<M> {
    // In order to sync this strategy, we need to get the current bid for all Sudo pools.
    async fn sync_state(&mut self) -> Result<()> {
        // Block in which the pool factory was deployed.
        let start_block = FACTORY_DEPLOYMENT_BLOCK;

        let current_block = self.client.get_block_number().await?;

        // Get all Sudo pool addresses deployed in the block range.
        let pool_addresses = self.get_new_pools(start_block, current_block).await?;
        info!("found {} deployed sudo pools", pool_addresses.len());

        // Get current bids for update state for all Sudo pools.
        for addresses in pool_addresses.chunks(200) {
            let quotes = self.get_quotes_for_pools(addresses.to_vec()).await?;
            self.update_internal_pool_state(quotes);
        }
        info!(
            "done syncing state, found available pools for {} collections",
            self.sudo_pools.len()
        );

        Ok(())
    }

    // Process incoming events, seeing if we can arb new orders, and updating the internal state on new blocks.
    async fn process_event(&mut self, event: Event) -> Vec<Action> {
        match event {
            Event::OpenseaOrder(order) => self
                .process_order_event(*order)
                .await
                .map_or(vec![], |a| vec![a]),
            Event::NewBlock(block) => match self.process_new_block_event(block).await {
                Ok(_) => vec![],
                Err(e) => {
                    panic!("Strategy is out of sync {}", e);
                }
            },
        }
    }
}

impl<M: Provider + Send + Sync + 'static> OpenseaSudoArb<M> {
    // Process new orders as they come in.
    async fn process_order_event(&mut self, event: OpenseaOrder) -> Option<Action> {
        let nft_address = event.listing.context.item.nft_id.address;
        info!("processing order event for address {}", nft_address);

        // Ignore orders that are not on Ethereum.
        match event.listing.context.item.nft_id.network {
            Chain::Ethereum => {}
            _ => return None,
        }
        // Ignore orders with non-eth payment.
        if event.listing.payment_token.address != Address::ZERO {
            return None;
        }

        // Find pool with highest bid.
        let pools = self.sudo_pools.get(&nft_address)?;
        let (max_pool, max_bid) = pools
            .iter()
            .filter_map(|pool| self.pool_bids.get(pool).map(|bid| (pool, bid)))
            .max_by(|a, b| a.1.cmp(b.1))?;

        // Ignore orders that are not profitable.
        if max_bid <= &event.listing.base_price {
            return None;
        }

        // Build arb tx.
        self.build_arb_tx(event.listing.order_hash, *max_pool, *max_bid)
            .await
    }

    /// Process new block events, updating the internal state.
    async fn process_new_block_event(&mut self, event: NewBlock) -> Result<()> {
        info!("processing new block {}", event.number);
        // Find new pools tthat were created in the last block.
        let new_pools = self.get_new_pools(event.number, event.number).await?;
        // Find existing pools that were touched in the last block.
        let touched_pools = self.get_touched_pools(event.number, event.number).await?;
        // Get quotes for all new and touched pools and update state.
        let quotes = self
            .get_quotes_for_pools([new_pools, touched_pools].concat())
            .await?;
        self.update_internal_pool_state(quotes);
        Ok(())
    }

    /// Build arb tx from order hash and sudo pool params.
    async fn build_arb_tx(
        &self,
        order_hash: B256,
        sudo_pool: Address,
        sudo_bid: U256,
    ) -> Option<Action> {
        // Get full order from Opensea V2 API.
        let response = self
            .opensea_client
            .fulfill_listing(hash_to_fulfill_listing_request(order_hash))
            .await;
        let order = match response {
            Ok(order) => order,
            Err(e) => {
                info!("Error getting order from opensea: {}", e);
                return None;
            }
        };

        // Parse out arb contract parameters.
        let payment_value = order.fulfillment_data.transaction.value;
        let total_profit = sudo_bid - U256::from(payment_value);

        // Build arb tx.
        let tx = build_execute_arb_tx(
            self.arb_contract_address,
            fulfill_listing_response_to_basic_order_parameters(order),
            U256::from(payment_value),
            sudo_pool,
        );
        Some(Action::SubmitTx(SubmitTxToMempool {
            tx,
            gas_bid_info: Some(GasBidInfo {
                total_profit,
                bid_percentage: self.bid_percentage,
            }),
        }))
    }

    /// Get quotes for a list of pools.
    async fn get_quotes_for_pools(&self, pools: Vec<Address>) -> Result<Vec<(Address, SellQuote)>> {
        let pool_addresses = pools.clone();
        let call = bindings::sudo_pair_quoter::SudoPairQuoter::getMultipleSellQuotesCall {
            pool_addresses,
        };
        let tx = TransactionRequest::default()
            .to(self.quoter_address)
            .input(TransactionInput::new(Bytes::from(call.abi_encode())));
        let response = self
            .client
            .call(tx)
            .overrides(self.quoter_state.clone())
            .await?;
        let quotes =
            bindings::sudo_pair_quoter::SudoPairQuoter::getMultipleSellQuotesCall::abi_decode_returns(
                response.as_ref(),
            )?;
        let res = pools
            .into_iter()
            .zip(quotes)
            .collect::<Vec<(Address, SellQuote)>>();
        Ok(res)
    }

    /// Update the internal state of the strategy with new pool addresses and quotes.
    fn update_internal_pool_state(&mut self, pools_and_quotes: Vec<(Address, SellQuote)>) {
        for (pool_address, quote) in pools_and_quotes {
            // If a quote is available, update both the pool_bids and the sudo_pools maps.
            if quote.quote_available {
                self.pool_bids.insert(pool_address, quote.price);
                self.sudo_pools
                    .entry(quote.nft_address)
                    .or_insert(vec![])
                    .push(pool_address);
            }
            // If a quote is unavailable, remove from both the pool_bids and the sudo_pools maps.
            else {
                self.pool_bids.remove(&pool_address);
                if let Some(addresses) = self.sudo_pools.get_mut(&quote.nft_address) {
                    addresses.retain(|address| *address != pool_address);
                }
            }
        }
    }

    /// Find all pools that were touched in a given block range.
    async fn get_touched_pools(&self, from_block: u64, to_block: u64) -> Result<Vec<Address>> {
        let address_list = self.pool_bids.keys().cloned().collect::<Vec<_>>();
        let filter = Filter::new()
            .from_block(from_block)
            .to_block(to_block)
            .address(address_list)
            .event_signature(POOL_EVENT_SIGNATURES.clone());

        let events = self.client.get_logs(&filter).await?;
        let touched_pools = events
            .iter()
            .map(|event| event.address())
            .collect::<Vec<_>>();
        Ok(touched_pools)
    }

    /// Find all pools that were created in a given block range.
    async fn get_new_pools(&self, from_block: u64, to_block: u64) -> Result<Vec<Address>> {
        let mut pool_addresses = vec![];

        // Maxium range for a single Alchemy query is 2000 blocks.
        for block in (from_block..to_block).step_by(2000) {
            let events = self
                .client
                .get_logs(
                    &Filter::new()
                        .from_block(block)
                        .to_block(block + 2000)
                        .address(self.lssvm_pair_factory_address)
                        .event_signature(
                            bindings::lssvm_pair_factory::LSSVMPairFactory::NewPair::SIGNATURE_HASH,
                        ),
                )
                .await?;

            let addresses = events
                .iter()
                .map(|event| {
                    decode_new_pair_pool_address(
                        event.topics().to_vec(),
                        event.data().data.as_ref(),
                    )
                    .with_context(|| {
                        format!(
                            "failed to decode LSSVMPairFactory NewPair log at address {}",
                            event.address()
                        )
                    })
                })
                .collect::<Result<Vec<_>>>()?;

            info!(
                "found {} new pools in block range, total progress: {}%",
                addresses.len(),
                100 * (block - from_block) / (to_block - from_block)
            );
            pool_addresses.extend(addresses);
        }
        Ok(pool_addresses)
    }
}

fn decode_new_pair_pool_address(topics: Vec<B256>, data: &[u8]) -> Result<Address> {
    let decoded =
        bindings::lssvm_pair_factory::LSSVMPairFactory::NewPair::decode_raw_log(topics, data)
            .context("failed to decode LSSVMPairFactory NewPair log")?;
    Ok(decoded.pool_address)
}

fn build_execute_arb_tx(
    arb_contract_address: Address,
    basic_order: bindings::zone_interface::BasicOrderParameters,
    payment_value: U256,
    sudo_pool: Address,
) -> TransactionRequest {
    let call = bindings::sudo_opensea_arb::SudoOpenseaArb::executeArbCall {
        basic_order,
        payment_value,
        sudo_pool,
    };

    TransactionRequest::default()
        .to(arb_contract_address)
        .input(TransactionInput::new(Bytes::from(call.abi_encode())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::{address, b256, bytes};

    #[test]
    fn opensea_arb_execute_arb_tx_builder_encodes_alloy_call() {
        let basic_order = bindings::zone_interface::BasicOrderParameters {
            consideration_token: address!("0000000000000000000000000000000000000001"),
            consideration_identifier: U256::from(2),
            consideration_amount: U256::from(3),
            offerer: address!("0000000000000000000000000000000000000004"),
            zone: address!("0000000000000000000000000000000000000005"),
            offer_token: address!("0000000000000000000000000000000000000006"),
            offer_identifier: U256::from(7),
            offer_amount: U256::from(8),
            basic_order_type: 9,
            start_time: U256::from(10),
            end_time: U256::from(11),
            zone_hash: b256!("1212121212121212121212121212121212121212121212121212121212121212"),
            salt: U256::from(13),
            offerer_conduit_key: b256!(
                "1414141414141414141414141414141414141414141414141414141414141414"
            ),
            fulfiller_conduit_key: b256!(
                "1515151515151515151515151515151515151515151515151515151515151515"
            ),
            total_original_additional_recipients: U256::from(1),
            additional_recipients: vec![bindings::zone_interface::AdditionalRecipient {
                amount: U256::from(16),
                recipient: address!("0000000000000000000000000000000000000017"),
            }],
            signature: Bytes::from(vec![0xaa, 0xbb]),
        };

        let tx = build_execute_arb_tx(
            address!("9999999999999999999999999999999999999999"),
            basic_order.clone(),
            U256::from(18),
            address!("8888888888888888888888888888888888888888"),
        );

        assert_eq!(
            *tx.to.unwrap().to().unwrap(),
            address!("9999999999999999999999999999999999999999")
        );
        let decoded = bindings::sudo_opensea_arb::SudoOpenseaArb::executeArbCall::abi_decode(
            tx.input.input().unwrap().as_ref(),
        )
        .unwrap();

        assert_eq!(decoded.basic_order, basic_order);
        assert_eq!(decoded.payment_value, U256::from(18));
        assert_eq!(
            decoded.sudo_pool,
            address!("8888888888888888888888888888888888888888")
        );
    }

    #[test]
    fn opensea_arb_tx_builder_sets_only_execution_target_and_calldata() {
        let basic_order = bindings::zone_interface::BasicOrderParameters {
            consideration_token: Address::ZERO,
            consideration_identifier: U256::ZERO,
            consideration_amount: U256::ZERO,
            offerer: Address::ZERO,
            zone: Address::ZERO,
            offer_token: Address::ZERO,
            offer_identifier: U256::ZERO,
            offer_amount: U256::ZERO,
            basic_order_type: 0,
            start_time: U256::ZERO,
            end_time: U256::ZERO,
            zone_hash: B256::ZERO,
            salt: U256::ZERO,
            offerer_conduit_key: B256::ZERO,
            fulfiller_conduit_key: B256::ZERO,
            total_original_additional_recipients: U256::ZERO,
            additional_recipients: vec![],
            signature: Bytes::new(),
        };

        let tx = build_execute_arb_tx(
            address!("9999999999999999999999999999999999999999"),
            basic_order,
            U256::from(18),
            address!("8888888888888888888888888888888888888888"),
        );

        assert_eq!(tx.from, None);
        assert_eq!(tx.value, None);
        assert_eq!(tx.gas, None);
        assert_eq!(tx.gas_price, None);
        assert_eq!(tx.max_fee_per_gas, None);
        assert_eq!(tx.max_priority_fee_per_gas, None);
        assert_eq!(tx.nonce, None);
        assert_eq!(tx.chain_id, None);
        assert_eq!(tx.access_list, None);
        assert_eq!(tx.transaction_type, None);
    }

    #[test]
    fn opensea_arb_new_installs_quoter_bytecode_override() {
        let provider = Arc::new(
            alloy::providers::ProviderBuilder::new()
                .connect_mocked_client(alloy::providers::mock::Asserter::new()),
        );
        let opensea_client = OpenSeaV2Client::new(opensea_v2::client::OpenSeaApiConfig {
            api_key: "test".to_string(),
        });
        let strategy = OpenseaSudoArb::new(
            provider,
            opensea_client,
            Config {
                arb_contract_address: address!("9999999999999999999999999999999999999999"),
                bid_percentage: 50,
            },
        );

        let override_account = strategy
            .quoter_state
            .get(&strategy.quoter_address)
            .expect("quoter address has override");
        assert_eq!(
            override_account.code.as_ref().map(|code| code.as_ref()),
            Some(&SUDOPAIRQUOTER_DEPLOYED_BYTECODE[..])
        );
    }

    #[test]
    fn opensea_arb_new_pair_decode_errors_on_malformed_log_data() {
        let err = decode_new_pair_pool_address(
            vec![bindings::lssvm_pair_factory::LSSVMPairFactory::NewPair::SIGNATURE_HASH],
            &[0xde, 0xad, 0xbe, 0xef],
        )
        .unwrap_err()
        .to_string();

        assert!(err.contains("NewPair"), "unexpected error: {err}");
    }

    #[test]
    fn opensea_arb_new_pair_decode_returns_pool_address() {
        let pool_address = decode_new_pair_pool_address(
            vec![bindings::lssvm_pair_factory::LSSVMPairFactory::NewPair::SIGNATURE_HASH],
            bytes!("0000000000000000000000001111111111111111111111111111111111111111").as_ref(),
        )
        .unwrap();

        assert_eq!(
            pool_address,
            address!("1111111111111111111111111111111111111111")
        );
    }
}
