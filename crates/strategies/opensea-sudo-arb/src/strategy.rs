use std::collections::HashMap;

use std::sync::Arc;

use async_trait::async_trait;

use alloy::sol_types::{SolCall, SolEvent};
use alloy::{
    primitives::{
        Address as AlloyAddress, Bytes as AlloyBytes, B256 as AlloyB256, U256 as AlloyU256,
    },
    rpc::types::{TransactionInput, TransactionRequest as AlloyTransactionRequest},
};
use bindings::sudo_pair_quoter::{SellQuote, SUDOPAIRQUOTER_DEPLOYED_BYTECODE};
use tracing::info;

use crate::constants::FACTORY_DEPLOYMENT_BLOCK;
use crate::types::Config;
use anyhow::Result;
use artemis_core::collectors::block_collector::NewBlock;
use artemis_core::collectors::opensea_order_collector::OpenseaOrder;
use artemis_core::executors::mempool_executor::{GasBidInfo, SubmitTxToMempool};
use artemis_core::types::Strategy;
use artemis_core::utilities::state_override_middleware::StateOverrideMiddleware;
use ethers::providers::Middleware;
#[cfg(test)]
use ethers::types::NameOrAddress;
use ethers::types::{
    transaction::eip2718::TypedTransaction, Bytes as EthersBytes, Filter,
    TransactionRequest as EthersTransactionRequest, H160, H256, U256,
};
use opensea_stream::schema::Chain;
use opensea_v2::client::OpenSeaV2Client;

use super::constants::{LSSVM_PAIR_FACTORY_ADDRESS, POOL_EVENT_SIGNATURES};
use super::types::{
    fulfill_listing_response_to_basic_order_parameters, hash_to_fulfill_listing_request, Action,
    Event,
};

#[derive(Debug, Clone)]
pub struct OpenseaSudoArb<M> {
    /// Ethers client.
    client: Arc<M>,
    /// Opensea V2 client
    opensea_client: OpenSeaV2Client,
    /// LSSVM pair factory contract for getting pair history.
    lssvm_pair_factory_address: H160,
    /// Quoter for batch reading pair state.
    quoter: Arc<StateOverrideMiddleware<Arc<M>>>,
    /// Address where the quoter bytecode is injected for read calls.
    quoter_address: H160,
    /// Arb contract.
    arb_contract_address: AlloyAddress,
    /// Map NFT addresses to a list of Sudo pair addresses which trade that NFT.
    sudo_pools: HashMap<H160, Vec<H160>>,
    /// Map Sudo pool addresses to the current bid for that pool (in ETH).
    pool_bids: HashMap<H160, U256>,
    /// Amount of profits to bid in gas
    bid_percentage: u64,
}

impl<M: Middleware + 'static> OpenseaSudoArb<M> {
    pub fn new(client: Arc<M>, opensea_client: OpenSeaV2Client, config: Config) -> Self {
        // Set up Sudo pair quoter contract.
        let mut state_override = StateOverrideMiddleware::new(client.clone());
        // Override account with contract bytecode
        let quoter_address =
            state_override.add_code(EthersBytes::from_static(SUDOPAIRQUOTER_DEPLOYED_BYTECODE));

        Self {
            client,
            opensea_client,
            lssvm_pair_factory_address: *LSSVM_PAIR_FACTORY_ADDRESS,
            quoter: Arc::new(state_override),
            quoter_address,
            arb_contract_address: config.arb_contract_address,
            sudo_pools: HashMap::new(),
            pool_bids: HashMap::new(),
            bid_percentage: config.bid_percentage,
        }
    }
}

#[async_trait]
impl<M: Middleware + 'static> Strategy<Event, Action> for OpenseaSudoArb<M> {
    // In order to sync this strategy, we need to get the current bid for all Sudo pools.
    async fn sync_state(&mut self) -> Result<()> {
        // Block in which the pool factory was deployed.
        let start_block = FACTORY_DEPLOYMENT_BLOCK;

        let current_block = self.client.get_block_number().await?.as_u64();

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

impl<M: Middleware + 'static> OpenseaSudoArb<M> {
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
        if event.listing.payment_token.address != H160::zero() {
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
        order_hash: H256,
        sudo_pool: H160,
        sudo_bid: U256,
    ) -> Option<Action> {
        // Get full order from Opensea V2 API.
        let response = self
            .opensea_client
            .fulfill_listing(hash_to_fulfill_listing_request(ethers_b256_to_alloy(
                order_hash,
            )))
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
            AlloyU256::from(payment_value),
            ethers_address_to_alloy(sudo_pool),
        );
        Some(Action::SubmitTx(SubmitTxToMempool {
            tx,
            gas_bid_info: Some(GasBidInfo {
                total_profit: ethers_u256_to_alloy(total_profit),
                bid_percentage: self.bid_percentage,
            }),
        }))
    }

    /// Get quotes for a list of pools.
    async fn get_quotes_for_pools(&self, pools: Vec<H160>) -> Result<Vec<(H160, SellQuote)>> {
        let pool_addresses = pools
            .iter()
            .copied()
            .map(ethers_address_to_alloy)
            .collect::<Vec<_>>();
        let call = bindings::sudo_pair_quoter::SudoPairQuoter::getMultipleSellQuotesCall {
            pool_addresses,
        };
        let tx: TypedTransaction = EthersTransactionRequest::new()
            .to(self.quoter_address)
            .data(EthersBytes::from(call.abi_encode()))
            .into();
        let response = self.quoter.call(&tx, None).await?;
        let quotes =
            bindings::sudo_pair_quoter::SudoPairQuoter::getMultipleSellQuotesCall::abi_decode_returns(
                response.as_ref(),
            )?;
        let res = pools
            .into_iter()
            .zip(quotes)
            .collect::<Vec<(H160, SellQuote)>>();
        Ok(res)
    }

    /// Update the internal state of the strategy with new pool addresses and quotes.
    fn update_internal_pool_state(&mut self, pools_and_quotes: Vec<(H160, SellQuote)>) {
        for (pool_address, quote) in pools_and_quotes {
            // If a quote is available, update both the pool_bids and the sudo_pools maps.
            if quote.quote_available {
                self.pool_bids
                    .insert(pool_address, alloy_u256_to_ethers(quote.price));
                self.sudo_pools
                    .entry(alloy_address_to_ethers(quote.nft_address))
                    .or_insert(vec![])
                    .push(pool_address);
            }
            // If a quote is unavailable, remove from both the pool_bids and the sudo_pools maps.
            else {
                self.pool_bids.remove(&pool_address);
                if let Some(addresses) = self
                    .sudo_pools
                    .get_mut(&alloy_address_to_ethers(quote.nft_address))
                {
                    addresses.retain(|address| *address != pool_address);
                }
            }
        }
    }

    /// Find all pools that were touched in a given block range.
    async fn get_touched_pools(&self, from_block: u64, to_block: u64) -> Result<Vec<H160>> {
        let address_list = self.pool_bids.keys().cloned().collect::<Vec<_>>();
        let filter = Filter::new()
            .from_block(from_block)
            .to_block(to_block)
            .address(address_list)
            .events(&*POOL_EVENT_SIGNATURES);

        let events = self.client.get_logs(&filter).await?;
        let touched_pools = events.iter().map(|event| event.address).collect::<Vec<_>>();
        Ok(touched_pools)
    }

    /// Find all pools that were created in a given block range.
    async fn get_new_pools(&self, from_block: u64, to_block: u64) -> Result<Vec<H160>> {
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
                        .topic0(alloy_b256_to_ethers(
                            bindings::lssvm_pair_factory::LSSVMPairFactory::NewPair::SIGNATURE_HASH,
                        )),
                )
                .await?;

            let addresses = events
                .iter()
                .filter_map(|event| {
                    let topics = event
                        .topics
                        .iter()
                        .map(|topic| AlloyB256::from_slice(topic.as_bytes()))
                        .collect::<Vec<_>>();
                    let decoded =
                        bindings::lssvm_pair_factory::LSSVMPairFactory::NewPair::decode_raw_log(
                            topics,
                            event.data.as_ref(),
                        )
                        .ok()?;
                    Some(alloy_address_to_ethers(decoded.pool_address))
                })
                .collect::<Vec<_>>();

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

fn build_execute_arb_tx(
    arb_contract_address: AlloyAddress,
    basic_order: bindings::zone_interface::BasicOrderParameters,
    payment_value: AlloyU256,
    sudo_pool: AlloyAddress,
) -> AlloyTransactionRequest {
    let call = bindings::sudo_opensea_arb::SudoOpenseaArb::executeArbCall {
        basic_order,
        payment_value,
        sudo_pool,
    };

    AlloyTransactionRequest::default()
        .to(arb_contract_address)
        .input(TransactionInput::new(AlloyBytes::from(call.abi_encode())))
}

#[cfg(test)]
fn ethers_typed_tx_to_alloy_request(tx: &TypedTransaction) -> Option<AlloyTransactionRequest> {
    let (TypedTransaction::Legacy(_) | TypedTransaction::Eip1559(_)) = tx else {
        // Access-list and other typed transactions need explicit Alloy support; do not
        // silently drop fields that affect signing or execution semantics.
        return None;
    };

    let mut request = AlloyTransactionRequest::default();

    if let Some(from) = tx.from() {
        request = request.from(ethers_address_to_alloy(*from));
    }

    match tx.to()? {
        NameOrAddress::Address(to) => {
            request = request.to(ethers_address_to_alloy(*to));
        }
        NameOrAddress::Name(_) => return None,
    }

    if let Some(value) = tx.value() {
        request = request.value(ethers_u256_to_alloy(*value));
    }
    if let Some(gas) = tx.gas() {
        request = request.gas_limit(ethers_u256_to_u64(*gas)?);
    }
    if let Some(nonce) = tx.nonce() {
        request = request.nonce(ethers_u256_to_u64(*nonce)?);
    }
    if let Some(gas_price) = tx.gas_price() {
        request = request.gas_price(ethers_u256_to_u128(gas_price)?);
    }
    if let Some(chain_id) = tx.chain_id() {
        request.chain_id = Some(chain_id.as_u64());
    }
    if let Some(data) = tx.data() {
        request = request.input(TransactionInput::new(AlloyBytes::from(data.to_vec())));
    }

    if let TypedTransaction::Eip1559(eip1559) = tx {
        if !eip1559.access_list.0.is_empty() {
            // Non-empty EIP-1559 access lists affect signing/execution semantics;
            // reject rather than silently dropping them until explicit Alloy support lands.
            return None;
        }
        request.transaction_type = Some(2);
        if let Some(max_fee_per_gas) = eip1559.max_fee_per_gas {
            request = request.max_fee_per_gas(ethers_u256_to_u128(max_fee_per_gas)?);
        }
        if let Some(max_priority_fee_per_gas) = eip1559.max_priority_fee_per_gas {
            request =
                request.max_priority_fee_per_gas(ethers_u256_to_u128(max_priority_fee_per_gas)?);
        }
    }

    Some(request)
}

fn ethers_address_to_alloy(address: H160) -> AlloyAddress {
    AlloyAddress::from_slice(address.as_bytes())
}

fn ethers_b256_to_alloy(value: H256) -> AlloyB256 {
    AlloyB256::from_slice(value.as_bytes())
}

fn alloy_address_to_ethers(address: AlloyAddress) -> H160 {
    H160::from_slice(address.as_slice())
}

fn alloy_b256_to_ethers(value: AlloyB256) -> H256 {
    H256::from_slice(value.as_slice())
}

fn alloy_u256_to_ethers(value: AlloyU256) -> U256 {
    U256::from_dec_str(&value.to_string()).expect("alloy U256 decimal is valid")
}

fn ethers_u256_to_alloy(value: U256) -> AlloyU256 {
    AlloyU256::from_str_radix(&value.to_string(), 10).expect("ethers U256 decimal is valid")
}

#[cfg(test)]
fn ethers_u256_to_u64(value: U256) -> Option<u64> {
    if value > U256::from(u64::MAX) {
        None
    } else {
        Some(value.as_u64())
    }
}

#[cfg(test)]
fn ethers_u256_to_u128(value: U256) -> Option<u128> {
    if value > U256::from(u128::MAX) {
        None
    } else {
        Some(value.as_u128())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::{address, b256};
    use ethers::types::{
        transaction::{
            eip1559::Eip1559TransactionRequest,
            eip2718::TypedTransaction,
            eip2930::{AccessList, AccessListItem},
        },
        Bytes as EthersBytes, TransactionRequest as EthersTransactionRequest, H256,
    };

    #[test]
    fn opensea_arb_execute_arb_tx_builder_encodes_alloy_call() {
        let basic_order = bindings::zone_interface::BasicOrderParameters {
            consideration_token: address!("0000000000000000000000000000000000000001"),
            consideration_identifier: AlloyU256::from(2),
            consideration_amount: AlloyU256::from(3),
            offerer: address!("0000000000000000000000000000000000000004"),
            zone: address!("0000000000000000000000000000000000000005"),
            offer_token: address!("0000000000000000000000000000000000000006"),
            offer_identifier: AlloyU256::from(7),
            offer_amount: AlloyU256::from(8),
            basic_order_type: 9,
            start_time: AlloyU256::from(10),
            end_time: AlloyU256::from(11),
            zone_hash: b256!("1212121212121212121212121212121212121212121212121212121212121212"),
            salt: AlloyU256::from(13),
            offerer_conduit_key: b256!(
                "1414141414141414141414141414141414141414141414141414141414141414"
            ),
            fulfiller_conduit_key: b256!(
                "1515151515151515151515151515151515151515151515151515151515151515"
            ),
            total_original_additional_recipients: AlloyU256::from(1),
            additional_recipients: vec![bindings::zone_interface::AdditionalRecipient {
                amount: AlloyU256::from(16),
                recipient: address!("0000000000000000000000000000000000000017"),
            }],
            signature: AlloyBytes::from(vec![0xaa, 0xbb]),
        };

        let tx = build_execute_arb_tx(
            address!("9999999999999999999999999999999999999999"),
            basic_order.clone(),
            AlloyU256::from(18),
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
        assert_eq!(decoded.payment_value, AlloyU256::from(18));
        assert_eq!(
            decoded.sudo_pool,
            address!("8888888888888888888888888888888888888888")
        );
    }

    #[test]
    fn opensea_arb_legacy_typed_tx_conversion_preserves_fields() {
        let tx = EthersTransactionRequest::new()
            .from(
                "1111111111111111111111111111111111111111"
                    .parse::<H160>()
                    .unwrap(),
            )
            .to("2222222222222222222222222222222222222222"
                .parse::<H160>()
                .unwrap())
            .value(U256::from(1234))
            .gas(21_000)
            .nonce(7)
            .gas_price(1_500_000_000u64)
            .chain_id(1u64)
            .data(EthersBytes::from(vec![0xde, 0xad, 0xbe, 0xef]));

        let converted = ethers_typed_tx_to_alloy_request(&TypedTransaction::Legacy(tx)).unwrap();

        assert_eq!(
            converted.from,
            Some(address!("1111111111111111111111111111111111111111"))
        );
        assert_eq!(
            *converted.to.unwrap().to().unwrap(),
            address!("2222222222222222222222222222222222222222")
        );
        assert_eq!(converted.value, Some(AlloyU256::from(1234)));
        assert_eq!(converted.gas, Some(21_000));
        assert_eq!(converted.nonce, Some(7));
        assert_eq!(converted.gas_price, Some(1_500_000_000));
        assert_eq!(converted.chain_id, Some(1));
        assert_eq!(
            converted.input.input().unwrap().as_ref(),
            &[0xde, 0xad, 0xbe, 0xef]
        );
    }

    #[test]
    fn opensea_arb_eip1559_typed_tx_conversion_preserves_fee_fields() {
        let tx = Eip1559TransactionRequest::new()
            .from(
                "1111111111111111111111111111111111111111"
                    .parse::<H160>()
                    .unwrap(),
            )
            .to("2222222222222222222222222222222222222222"
                .parse::<H160>()
                .unwrap())
            .value(U256::from(1234))
            .gas(21_000)
            .nonce(7)
            .max_fee_per_gas(2_000_000_000u64)
            .max_priority_fee_per_gas(1_000_000_000u64)
            .chain_id(1u64)
            .data(EthersBytes::from(vec![0xca, 0xfe]));

        let converted = ethers_typed_tx_to_alloy_request(&TypedTransaction::Eip1559(tx)).unwrap();

        assert_eq!(
            converted.from,
            Some(address!("1111111111111111111111111111111111111111"))
        );
        assert_eq!(
            *converted.to.unwrap().to().unwrap(),
            address!("2222222222222222222222222222222222222222")
        );
        assert_eq!(converted.value, Some(AlloyU256::from(1234)));
        assert_eq!(converted.gas, Some(21_000));
        assert_eq!(converted.nonce, Some(7));
        assert_eq!(converted.max_fee_per_gas, Some(2_000_000_000));
        assert_eq!(converted.max_priority_fee_per_gas, Some(1_000_000_000));
        assert_eq!(converted.chain_id, Some(1));
        assert_eq!(converted.input.input().unwrap().as_ref(), &[0xca, 0xfe]);
    }

    #[test]
    fn opensea_arb_eip1559_typed_tx_conversion_rejects_non_empty_access_list() {
        let tx = Eip1559TransactionRequest::new()
            .to("2222222222222222222222222222222222222222"
                .parse::<H160>()
                .unwrap())
            .access_list(AccessList(vec![AccessListItem {
                address: "3333333333333333333333333333333333333333"
                    .parse::<H160>()
                    .unwrap(),
                storage_keys: vec![H256::from_low_u64_be(1)],
            }]));

        assert!(ethers_typed_tx_to_alloy_request(&TypedTransaction::Eip1559(tx)).is_none());
    }
}
