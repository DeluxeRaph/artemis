use alloy_primitives::{Address, Bytes, B256, U256};
use artemis_core::{
    collectors::{block_collector::NewBlock, opensea_order_collector::OpenseaOrder},
    executors::mempool_executor::SubmitTxToMempool,
};
use bindings::zone_interface::{AdditionalRecipient, BasicOrderParameters};
use ethers::types::{H160, H256};
use opensea_v2::types::{
    Chain, FulfillListingRequest, FulfillListingResponse, Fulfiller, Listing, ProtocolVersion,
};

/// Core Event enum for the current strategy.
#[derive(Debug, Clone)]
pub enum Event {
    NewBlock(NewBlock),
    OpenseaOrder(Box<OpenseaOrder>),
}

/// Core Action enum for the current strategy.
#[derive(Debug, Clone)]
pub enum Action {
    SubmitTx(SubmitTxToMempool),
}

/// Configuration for variables we need to pass to the strategy.
#[derive(Debug, Clone)]
pub struct Config {
    pub arb_contract_address: Address,
    pub bid_percentage: u64,
}

/// Convenience function to convert a hash to a fulfill listing request
pub fn hash_to_fulfill_listing_request(hash: B256) -> FulfillListingRequest {
    FulfillListingRequest {
        listing: Listing {
            hash,
            chain: Chain::Mainnet,
            protocol_version: ProtocolVersion::V1_5,
        },
        fulfiller: Fulfiller {
            address: Address::ZERO,
        },
    }
}

/// Convenience function to convert a fulfill listing response to basic order parameters
pub fn fulfill_listing_response_to_basic_order_parameters(
    val: FulfillListingResponse,
) -> BasicOrderParameters {
    let params = val.fulfillment_data.transaction.input_data.parameters;

    let recipients: Vec<AdditionalRecipient> = params
        .additional_recipients
        .iter()
        .map(|ar| AdditionalRecipient {
            recipient: address_to_ethers(ar.recipient),
            amount: u256_to_ethers(ar.amount),
        })
        .collect();

    BasicOrderParameters {
        consideration_token: address_to_ethers(params.consideration_token),
        consideration_identifier: u256_to_ethers(params.consideration_identifier),
        consideration_amount: u256_to_ethers(params.consideration_amount),
        offerer: address_to_ethers(params.offerer),
        zone: address_to_ethers(params.zone),
        offer_token: address_to_ethers(params.offer_token),
        offer_identifier: u256_to_ethers(params.offer_identifier),
        offer_amount: u256_to_ethers(params.offer_amount),
        basic_order_type: params.basic_order_type,
        start_time: u256_to_ethers(params.start_time),
        end_time: u256_to_ethers(params.end_time),
        zone_hash: b256_to_ethers(params.zone_hash).into(),
        salt: u256_to_ethers(params.salt),
        offerer_conduit_key: b256_to_ethers(params.offerer_conduit_key).into(),
        fulfiller_conduit_key: b256_to_ethers(params.fulfiller_conduit_key).into(),
        total_original_additional_recipients: u256_to_ethers(
            params.total_original_additional_recipients,
        ),
        additional_recipients: recipients,
        signature: bytes_to_ethers(params.signature),
    }
}

fn address_to_ethers(address: Address) -> H160 {
    H160::from_slice(address.as_slice())
}

fn b256_to_ethers(value: B256) -> H256 {
    H256::from_slice(value.as_slice())
}

fn bytes_to_ethers(bytes: Bytes) -> ethers::types::Bytes {
    bytes.to_vec().into()
}

fn u256_to_ethers(value: U256) -> ethers::types::U256 {
    ethers::types::U256::from_dec_str(&value.to_string()).expect("alloy U256 decimal is valid")
}

#[cfg(test)]
mod tests {
    use super::*;
    use ethers::types::U256 as EthersU256;
    use serde_json::json;
    use std::path::PathBuf;

    #[test]
    fn hash_to_fulfill_listing_request_serializes_alloy_values_for_opensea() {
        let hash = B256::from_slice(&[0x11; 32]);

        let req = hash_to_fulfill_listing_request(hash);
        let serialized = serde_json::to_value(req).unwrap();

        assert_eq!(
            serialized,
            json!({
                "listing": {
                    "hash": format!("0x{}", "11".repeat(32)),
                    "chain": "ethereum",
                    "protocol_address": "0x00000000000000ADc04C56Bf30aC9d3c0aAF14dC"
                },
                "fulfiller": {
                    "address": "0x0000000000000000000000000000000000000000"
                }
            })
        );
    }

    #[test]
    fn fulfill_listing_response_conversion_preserves_ethers_binding_values() {
        let mut d = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        d.push("../../clients/opensea-v2/resources/sample_response_1.5.json");
        let response = std::fs::read_to_string(d).unwrap();
        let response: FulfillListingResponse = serde_json::from_str(&response).unwrap();

        let params = fulfill_listing_response_to_basic_order_parameters(response);

        assert_eq!(params.consideration_token, H160::zero());
        assert_eq!(
            params.consideration_amount,
            EthersU256::from_dec_str("17700000000000000").unwrap()
        );
        assert_eq!(
            params.offerer,
            "0x5980565737bb2885790c79f126d2c862ad1dc8ab"
                .parse::<H160>()
                .unwrap()
        );
        assert_eq!(
            params.offer_identifier,
            EthersU256::from_dec_str(
                "40482595849772694285173713041642282097106100196042549765489072528810617864193",
            )
            .unwrap()
        );
        assert_eq!(params.additional_recipients.len(), 2);
        assert_eq!(
            params.additional_recipients[0].amount,
            EthersU256::from_dec_str("500000000000000").unwrap()
        );
        assert_eq!(params.signature.0.len(), 64);
    }
}
