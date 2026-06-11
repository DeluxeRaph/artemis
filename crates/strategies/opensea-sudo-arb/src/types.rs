use alloy_primitives::{Address, B256};
use artemis_core::{
    collectors::{block_collector::NewBlock, opensea_order_collector::OpenseaOrder},
    executors::mempool_executor::SubmitTxToMempool,
};
use bindings::zone_interface::{AdditionalRecipient, BasicOrderParameters};
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
            recipient: ar.recipient,
            amount: ar.amount,
        })
        .collect();

    BasicOrderParameters {
        consideration_token: params.consideration_token,
        consideration_identifier: params.consideration_identifier,
        consideration_amount: params.consideration_amount,
        offerer: params.offerer,
        zone: params.zone,
        offer_token: params.offer_token,
        offer_identifier: params.offer_identifier,
        offer_amount: params.offer_amount,
        basic_order_type: params.basic_order_type,
        start_time: params.start_time,
        end_time: params.end_time,
        zone_hash: params.zone_hash,
        salt: params.salt,
        offerer_conduit_key: params.offerer_conduit_key,
        fulfiller_conduit_key: params.fulfiller_conduit_key,
        total_original_additional_recipients: params.total_original_additional_recipients,
        additional_recipients: recipients,
        signature: params.signature,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{address, U256};
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
    fn fulfill_listing_response_conversion_preserves_basic_order_values() {
        let mut d = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        d.push("../../clients/opensea-v2/resources/sample_response_1.5.json");
        let response = std::fs::read_to_string(d).unwrap();
        let response: FulfillListingResponse = serde_json::from_str(&response).unwrap();

        let params = fulfill_listing_response_to_basic_order_parameters(response);

        assert_eq!(params.consideration_token, Address::ZERO);
        assert_eq!(
            params.consideration_amount,
            U256::from_str_radix("17700000000000000", 10).unwrap()
        );
        assert_eq!(
            params.offerer,
            address!("5980565737bb2885790c79f126d2c862ad1dc8ab")
        );
        assert_eq!(
            params.offer_identifier,
            U256::from_str_radix(
                "40482595849772694285173713041642282097106100196042549765489072528810617864193",
                10
            )
            .unwrap()
        );
        assert_eq!(params.additional_recipients.len(), 2);
        assert_eq!(
            params.additional_recipients[0].amount,
            U256::from_str_radix("500000000000000", 10).unwrap()
        );
        assert_eq!(params.signature.len(), 64);
    }
}
