use std::sync::Arc;

use alloy::{
    primitives::{Address as AlloyAddress, TxKind, U256 as AlloyU256},
    rpc::types::TransactionRequest as AlloyTransactionRequest,
};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use ethers::{
    providers::Middleware,
    signers::Signer,
    types::{
        transaction::{eip1559::Eip1559TransactionRequest, eip2718::TypedTransaction},
        Address as EthersAddress, Bytes as EthersBytes,
        TransactionRequest as EthersTransactionRequest, U256 as EthersU256,
    },
};
use ethers_flashbots::{BundleRequest, FlashbotsMiddleware};
use reqwest::Url;
use tracing::error;

use crate::types::Executor;

/// A Flashbots executor that sends transactions to the Flashbots relay.
pub struct FlashbotsExecutor<M, S> {
    /// The Flashbots middleware.
    fb_client: FlashbotsMiddleware<Arc<M>, S>,

    /// The signer to sign transactions before sending to the relay.
    tx_signer: S,
}

/// A bundle of Alloy transaction requests to send to the Flashbots relay.
pub type FlashbotsBundle = Vec<AlloyTransactionRequest>;

impl<M: Middleware, S: Signer> FlashbotsExecutor<M, S> {
    pub fn new(client: Arc<M>, tx_signer: S, relay_signer: S, relay_url: impl Into<Url>) -> Self {
        let fb_client = FlashbotsMiddleware::new(client, relay_url, relay_signer);
        Self {
            fb_client,
            tx_signer,
        }
    }
}

#[async_trait]
impl<M, S> Executor<FlashbotsBundle> for FlashbotsExecutor<M, S>
where
    M: Middleware + 'static,
    M::Error: 'static,
    S: Signer + 'static,
{
    /// Send a bundle to transactions to the Flashbots relay.
    async fn execute(&self, action: FlashbotsBundle) -> Result<()> {
        // Add txs to bundle.
        let mut bundle = BundleRequest::new();

        // Sign each Alloy transaction request in bundle.
        for tx in action {
            let tx = alloy_tx_request_to_ethers(&tx)?;
            let signature = self.tx_signer.sign_transaction(&tx).await?;
            bundle.add_transaction(tx.rlp_signed(&signature));
        }

        // Simulate bundle.
        let block_number = self.fb_client.get_block_number().await?;
        let bundle = bundle
            .set_block(block_number + 1)
            .set_simulation_block(block_number)
            .set_simulation_timestamp(0);

        let simulated_bundle = self.fb_client.simulate_bundle(&bundle).await;

        if let Err(simulate_error) = simulated_bundle {
            error!("Error simulating bundle: {:?}", simulate_error);
        }

        // Send bundle.
        let pending_bundle = self.fb_client.send_bundle(&bundle).await;

        if let Err(send_error) = pending_bundle {
            error!("Error sending bundle: {:?}", send_error);
        }

        Ok(())
    }
}

fn alloy_tx_request_to_ethers(tx: &AlloyTransactionRequest) -> Result<TypedTransaction> {
    reject_unsupported_alloy_bundle_fields(tx)?;

    let is_explicit_eip1559 = tx.transaction_type == Some(2);
    let has_eip1559_fee_fields = tx.has_eip1559_fields();

    if is_explicit_eip1559 || has_eip1559_fee_fields {
        if tx.gas_price.is_some() {
            return Err(anyhow!(
                "Flashbots bundle transaction request cannot mix gas_price with EIP-1559 transaction type or fee fields"
            ));
        }

        return Ok(TypedTransaction::Eip1559(alloy_tx_request_to_eip1559(tx)?));
    }

    Ok(TypedTransaction::Legacy(alloy_tx_request_to_legacy(tx)?))
}

fn reject_unsupported_alloy_bundle_fields(tx: &AlloyTransactionRequest) -> Result<()> {
    if tx.access_list.is_some() {
        return Err(anyhow!(
            "Flashbots bundle transaction request contains unsupported access_list; access-list bundle signing is not yet implemented"
        ));
    }
    if tx.has_eip4844_fields() {
        return Err(anyhow!(
            "Flashbots bundle transaction request contains unsupported EIP-4844 blob fields"
        ));
    }
    if tx.authorization_list.is_some() {
        return Err(anyhow!(
            "Flashbots bundle transaction request contains unsupported EIP-7702 authorization_list"
        ));
    }
    if let Some(transaction_type) = tx.transaction_type {
        match transaction_type {
            0 => {
                if tx.max_fee_per_gas.is_some() || tx.max_priority_fee_per_gas.is_some() {
                    return Err(anyhow!(
                        "legacy Flashbots bundle transaction request cannot contain EIP-1559 fee fields"
                    ));
                }
            }
            2 => {}
            unsupported => {
                return Err(anyhow!(
                    "Flashbots bundle transaction request contains unsupported transaction_type {unsupported}"
                ));
            }
        }
    }

    Ok(())
}

fn alloy_tx_request_to_legacy(tx: &AlloyTransactionRequest) -> Result<EthersTransactionRequest> {
    let mut request = EthersTransactionRequest::new();

    if let Some(from) = tx.from {
        request = request.from(alloy_address_to_ethers(from));
    }

    if let Some(to) = tx.to {
        match to {
            TxKind::Call(to) => request = request.to(alloy_address_to_ethers(to)),
            TxKind::Create => {}
        }
    }

    if let Some(value) = tx.value {
        request = request.value(alloy_u256_to_ethers(value));
    }
    if let Some(gas) = tx.gas {
        request = request.gas(gas);
    }
    if let Some(nonce) = tx.nonce {
        request = request.nonce(nonce);
    }
    if let Some(gas_price) = tx.gas_price {
        request = request.gas_price(gas_price);
    }
    if let Some(chain_id) = tx.chain_id {
        request = request.chain_id(chain_id);
    }
    if let Some(input) = tx.input.unique_input()? {
        request = request.data(EthersBytes::from(input.to_vec()));
    }

    Ok(request)
}

fn alloy_tx_request_to_eip1559(tx: &AlloyTransactionRequest) -> Result<Eip1559TransactionRequest> {
    let mut request = Eip1559TransactionRequest::new();

    if let Some(from) = tx.from {
        request = request.from(alloy_address_to_ethers(from));
    }
    if let Some(to) = tx.to {
        match to {
            TxKind::Call(to) => request = request.to(alloy_address_to_ethers(to)),
            TxKind::Create => {}
        }
    }

    if let Some(value) = tx.value {
        request = request.value(alloy_u256_to_ethers(value));
    }
    if let Some(gas) = tx.gas {
        request = request.gas(gas);
    }
    if let Some(nonce) = tx.nonce {
        request = request.nonce(nonce);
    }
    if let Some(max_fee_per_gas) = tx.max_fee_per_gas {
        request = request.max_fee_per_gas(EthersU256::from(max_fee_per_gas));
    }
    if let Some(max_priority_fee_per_gas) = tx.max_priority_fee_per_gas {
        request = request.max_priority_fee_per_gas(EthersU256::from(max_priority_fee_per_gas));
    }
    if let Some(chain_id) = tx.chain_id {
        request = request.chain_id(chain_id);
    }
    if let Some(input) = tx.input.unique_input()? {
        request = request.data(EthersBytes::from(input.to_vec()));
    }

    Ok(request)
}

fn alloy_address_to_ethers(address: AlloyAddress) -> EthersAddress {
    EthersAddress::from_slice(address.as_slice())
}

fn alloy_u256_to_ethers(value: AlloyU256) -> EthersU256 {
    EthersU256::from_dec_str(&value.to_string()).expect("alloy U256 decimal is valid")
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::{
        primitives::{address, b256, bytes, U256},
        rpc::types::{AccessList, AccessListItem, TransactionInput, TransactionRequest},
    };
    use ethers::types::{
        transaction::eip2718::TypedTransaction, Bytes as EthersBytes, U256 as EthersU256,
    };

    #[test]
    fn flashbots_bundle_public_type_accepts_alloy_transaction_requests() {
        fn assert_flashbots_bundle(_: FlashbotsBundle) {}

        assert_flashbots_bundle(vec![TransactionRequest::default()]);
    }

    #[test]
    fn flashbots_bundle_transaction_request_preserves_legacy_fields_for_signing() {
        let mut tx = TransactionRequest::default()
            .from(address!("1111111111111111111111111111111111111111"))
            .to(address!("2222222222222222222222222222222222222222"))
            .value(U256::from(1234))
            .gas_limit(21_000)
            .nonce(7)
            .gas_price(1_500_000_000)
            .input(TransactionInput::new(bytes!("deadbeef")));
        tx.chain_id = Some(1);

        let converted = alloy_tx_request_to_ethers(&tx).unwrap();

        let TypedTransaction::Legacy(converted) = converted else {
            panic!("expected legacy typed transaction");
        };
        assert_eq!(
            converted.from,
            Some("1111111111111111111111111111111111111111".parse().unwrap())
        );
        assert_eq!(
            converted.to,
            Some(
                alloy_address_to_ethers(address!("2222222222222222222222222222222222222222"))
                    .into()
            )
        );
        assert_eq!(converted.value, Some(EthersU256::from(1234)));
        assert_eq!(converted.gas, Some(21_000u64.into()));
        assert_eq!(converted.nonce, Some(7u64.into()));
        assert_eq!(converted.gas_price, Some(1_500_000_000u64.into()));
        assert_eq!(converted.chain_id, Some(1u64.into()));
        assert_eq!(
            converted.data,
            Some(EthersBytes::from(vec![0xde, 0xad, 0xbe, 0xef]))
        );
    }

    #[test]
    fn flashbots_bundle_transaction_request_preserves_eip1559_fee_fields() {
        let mut tx = TransactionRequest::default()
            .from(address!("1111111111111111111111111111111111111111"))
            .to(address!("2222222222222222222222222222222222222222"))
            .value(U256::from(1234))
            .gas_limit(21_000)
            .nonce(7)
            .max_fee_per_gas(2_000_000_000)
            .max_priority_fee_per_gas(1_000_000_000)
            .input(TransactionInput::new(bytes!("deadbeef")));
        tx.chain_id = Some(1);

        let converted = alloy_tx_request_to_ethers(&tx).unwrap();

        match converted {
            TypedTransaction::Eip1559(eip1559) => {
                assert_eq!(
                    eip1559.from,
                    Some("1111111111111111111111111111111111111111".parse().unwrap())
                );
                assert_eq!(
                    eip1559.to,
                    Some(
                        alloy_address_to_ethers(address!(
                            "2222222222222222222222222222222222222222"
                        ))
                        .into()
                    )
                );
                assert_eq!(eip1559.value, Some(EthersU256::from(1234)));
                assert_eq!(eip1559.max_fee_per_gas, Some(2_000_000_000u64.into()));
                assert_eq!(
                    eip1559.max_priority_fee_per_gas,
                    Some(1_000_000_000u64.into())
                );
                assert_eq!(eip1559.chain_id, Some(1u64.into()));
                assert_eq!(eip1559.gas, Some(21_000u64.into()));
                assert_eq!(eip1559.nonce, Some(7u64.into()));
                assert_eq!(
                    eip1559.data,
                    Some(EthersBytes::from(vec![0xde, 0xad, 0xbe, 0xef]))
                );
            }
            other => panic!("expected EIP-1559 typed transaction, got {other:?}"),
        }
    }

    #[test]
    fn flashbots_bundle_transaction_request_preserves_contract_creation() {
        let tx = TransactionRequest::default()
            .create()
            .gas_limit(100_000)
            .input(TransactionInput::new(bytes!("60806040")));

        let converted = alloy_tx_request_to_ethers(&tx).unwrap();

        let TypedTransaction::Legacy(converted) = converted else {
            panic!("expected legacy typed transaction");
        };
        assert_eq!(converted.to, None);
        assert_eq!(converted.gas, Some(100_000u64.into()));
        assert_eq!(
            converted.data,
            Some(EthersBytes::from(vec![0x60, 0x80, 0x60, 0x40]))
        );
    }

    #[test]
    fn flashbots_bundle_transaction_request_rejects_access_lists() {
        let mut tx =
            TransactionRequest::default().to(address!("2222222222222222222222222222222222222222"));
        tx.access_list = Some(AccessList(vec![AccessListItem {
            address: address!("3333333333333333333333333333333333333333"),
            storage_keys: vec![b256!(
                "0000000000000000000000000000000000000000000000000000000000000001"
            )],
        }]));

        let err = alloy_tx_request_to_ethers(&tx).unwrap_err().to_string();

        assert!(err.contains("access_list"), "unexpected error: {err}");
    }

    #[test]
    fn flashbots_bundle_transaction_request_rejects_eip2930_type() {
        let tx = TransactionRequest::default().transaction_type(1);

        let err = alloy_tx_request_to_ethers(&tx).unwrap_err().to_string();

        assert!(
            err.contains("unsupported transaction_type 1"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn flashbots_bundle_transaction_request_rejects_blob_fields() {
        let tx = TransactionRequest::default().max_fee_per_blob_gas(1);

        let err = alloy_tx_request_to_ethers(&tx).unwrap_err().to_string();

        assert!(
            err.contains("EIP-4844 blob fields"),
            "unexpected error: {err}"
        );

        let tx = TransactionRequest {
            blob_versioned_hashes: Some(vec![b256!(
                "0000000000000000000000000000000000000000000000000000000000000001"
            )]),
            ..Default::default()
        };

        let err = alloy_tx_request_to_ethers(&tx).unwrap_err().to_string();

        assert!(
            err.contains("EIP-4844 blob fields"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn flashbots_bundle_transaction_request_rejects_authorization_lists() {
        let tx = TransactionRequest {
            authorization_list: Some(vec![]),
            ..Default::default()
        };

        let err = alloy_tx_request_to_ethers(&tx).unwrap_err().to_string();

        assert!(
            err.contains("authorization_list"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn flashbots_bundle_transaction_request_rejects_legacy_type_with_eip1559_fees() {
        let tx = TransactionRequest::default()
            .transaction_type(0)
            .max_fee_per_gas(2_000_000_000);

        let err = alloy_tx_request_to_ethers(&tx).unwrap_err().to_string();

        assert!(
            err.contains(
                "legacy Flashbots bundle transaction request cannot contain EIP-1559 fee fields"
            ),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn flashbots_bundle_transaction_request_rejects_mixed_legacy_and_eip1559_fees() {
        let tx = TransactionRequest::default()
            .gas_price(1_000_000_000)
            .max_fee_per_gas(2_000_000_000);

        let err = alloy_tx_request_to_ethers(&tx).unwrap_err().to_string();

        assert!(
            err.contains("cannot mix gas_price with EIP-1559"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn flashbots_bundle_transaction_request_uses_explicit_eip1559_type() {
        let tx = TransactionRequest::default()
            .transaction_type(2)
            .gas_limit(21_000);

        let converted = alloy_tx_request_to_ethers(&tx).unwrap();

        match converted {
            TypedTransaction::Eip1559(eip1559) => {
                assert_eq!(eip1559.gas, Some(21_000u64.into()));
                assert_eq!(eip1559.max_fee_per_gas, None);
                assert_eq!(eip1559.max_priority_fee_per_gas, None);
            }
            other => panic!("expected EIP-1559 typed transaction, got {other:?}"),
        }
    }

    #[test]
    fn flashbots_bundle_transaction_request_rejects_conflicting_input_aliases() {
        let tx = TransactionRequest {
            input: TransactionInput {
                input: Some(bytes!("dead")),
                data: Some(bytes!("beef")),
            },
            ..Default::default()
        };

        assert!(alloy_tx_request_to_ethers(&tx).is_err());
    }
}
