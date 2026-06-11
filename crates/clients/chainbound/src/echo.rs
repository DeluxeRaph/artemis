use std::{sync::Arc, time::Duration};

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
use reqwest::{
    header::{HeaderMap, HeaderValue},
    Client,
};
use tracing::{debug, error};

use artemis_core::types::Executor;

use crate::SendBundleArgs;

/// Possible actions that can be executed by the Echo executor
#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
#[allow(missing_docs)]
pub enum Action {
    SendBundle(SendBundleArgs),
}

const ECHO_RPC_URL: &str = "https://echo-rpc.chainbound.io";

/// An Echo executor that sends transactions to the specified block builders
pub struct EchoExecutor<M, S> {
    /// The Echo RPC endpoint
    echo_endpoint: String,
    /// The HTTP client to send requests to the Echo RPC
    echo_client: Client,
    /// The native ethers middleware
    inner: Arc<M>,
    /// The signer to sign transactions before sending to the builders
    tx_signer: S,
    /// the signer to compute the `X-Flashbots-Signature` of the bundle payload
    auth_signer: S,
}

impl<M: Middleware, S: Signer> EchoExecutor<M, S> {
    /// Initialize a new Echo executor.
    ///
    /// ## Arguments
    /// - `inner`: The native ethers middleware that can query the blockchain
    /// - `tx_signer`: The actual signer of the bundle transactions
    /// - `auth_signer`: The signer to compute the `X-Flashbots-Signature` of the bundle payload
    /// - `api_key`: The Echo API key to use
    pub fn new(inner: Arc<M>, tx_signer: S, auth_signer: S, api_key: impl Into<String>) -> Self {
        let mut headers = HeaderMap::new();
        headers.insert("Content-Type", "application/json".parse().unwrap());
        headers.insert("X-Api-Key", api_key.into().parse().expect("Broken API key"));

        let echo_client = Client::builder()
            .timeout(Duration::from_secs(300))
            .default_headers(headers)
            .build()
            .expect("Could not instantiate HTTP client");

        Self {
            echo_endpoint: ECHO_RPC_URL.into(),
            echo_client,
            inner,
            tx_signer,
            auth_signer,
        }
    }

    /// Optionally set the Echo RPC endpoint, overriding the default
    pub fn set_rpc_endpoint(&mut self, endpoint: impl Into<String>) {
        self.echo_endpoint = endpoint.into();
    }

    /// Returns a reference to the native ethers middleware
    pub fn provider(&self) -> Arc<M> {
        self.inner.clone()
    }
}

#[async_trait]
impl<M, S> Executor<SendBundleArgs> for EchoExecutor<M, S>
where
    M: Middleware + 'static,
    M::Error: 'static,
    S: Signer + 'static,
{
    /// Send a bundle to transactions to the specified builders
    async fn execute(&self, mut action: SendBundleArgs) -> Result<()> {
        if action.unsigned_txs.is_empty() {
            return Err(anyhow!(
                "Bundle must contain at least one transaction. 
                To cancel a bundle, use the `eth_cancelBundle` method."
            ));
        }

        // Sign each transaction in bundle
        for tx in action.unsigned_txs.iter() {
            let tx = alloy_tx_request_to_ethers(tx)?;
            let signature = self.tx_signer.sign_transaction(&tx).await?;
            let signed = tx.rlp_signed(&signature).to_string();
            action.standard_features.txs.push(signed);
        }

        // Set block number to the next block if not specified
        if action.standard_features.block_number.is_none() {
            let block_number = self.inner.get_block_number().await?;
            let next_block_number_hex = format!("0x{:#x}", block_number.as_u64() + 1);
            action.standard_features.block_number = Some(next_block_number_hex);
        }

        // TODO: Simulate bundle

        // Sign bundle payload (without the Echo-specific features)
        let signable_payload = serde_json::to_string(&action.standard_features)?;
        let flashbots_signature = self.auth_signer.sign_message(&signable_payload).await?;

        // Create the `X-Flashbots-Signature` header
        let flashbots_signature_header: HeaderValue =
            format!("{:#x}:{}", self.auth_signer.address(), flashbots_signature).parse()?;

        // Prepare the full JSON-RPC request body
        let bundle_json = serde_json::to_string(&action)?;

        let request_body = format!(
            r#"{{"id":1,"jsonrpc":"2.0","method":"eth_sendBundle","params":[{}]}}"#,
            bundle_json
        );

        // Send bundle
        let echo_response = self
            .echo_client
            .post(&self.echo_endpoint)
            .body(request_body)
            .header("X-Flashbots-Signature", flashbots_signature_header)
            .send()
            .await;

        match echo_response {
            Ok(send_response) => {
                let status = send_response.status();
                let body = send_response.text().await?;

                dbg!(body.clone());

                if status.is_success() {
                    debug!("Echo bundle response: {:?}", body);
                } else {
                    error!("Error in Echo bundle response: {:?}", body);
                }
            }
            Err(send_error) => error!("Error while sending bundle to Echo: {:?}", send_error),
        }

        Ok(())
    }
}

fn alloy_tx_request_to_ethers(tx: &AlloyTransactionRequest) -> Result<TypedTransaction> {
    reject_unsupported_alloy_bundle_fields(tx)?;

    if tx.max_fee_per_gas.is_some() || tx.max_priority_fee_per_gas.is_some() {
        if tx.gas_price.is_some() {
            return Err(anyhow!(
                "bundle transaction request cannot mix gas_price with EIP-1559 fee fields"
            ));
        }

        return Ok(TypedTransaction::Eip1559(alloy_tx_request_to_eip1559(tx)?));
    }

    Ok(TypedTransaction::Legacy(alloy_tx_request_to_legacy(tx)?))
}

fn reject_unsupported_alloy_bundle_fields(tx: &AlloyTransactionRequest) -> Result<()> {
    if tx.access_list.is_some() {
        return Err(anyhow!(
            "bundle transaction request contains unsupported access_list; access-list bundle signing is not yet implemented"
        ));
    }
    if tx.max_fee_per_blob_gas.is_some()
        || tx.blob_versioned_hashes.is_some()
        || tx.sidecar.is_some()
    {
        return Err(anyhow!(
            "bundle transaction request contains unsupported EIP-4844 blob fields"
        ));
    }
    if tx.authorization_list.is_some() {
        return Err(anyhow!(
            "bundle transaction request contains unsupported EIP-7702 authorization_list"
        ));
    }
    if let Some(transaction_type) = tx.transaction_type {
        match transaction_type {
            0 => {
                if tx.max_fee_per_gas.is_some() || tx.max_priority_fee_per_gas.is_some() {
                    return Err(anyhow!(
                        "legacy bundle transaction request cannot contain EIP-1559 fee fields"
                    ));
                }
            }
            2 => {}
            unsupported => {
                return Err(anyhow!(
                    "bundle transaction request contains unsupported transaction_type {unsupported}"
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
        rpc::types::{AccessList, AccessListItem, TransactionInput},
    };
    use ethers::types::transaction::eip2718::TypedTransaction;

    #[test]
    fn alloy_bundle_transaction_request_preserves_fields_for_ethers_signing() {
        let mut tx = AlloyTransactionRequest::default()
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
    fn alloy_bundle_transaction_request_preserves_eip1559_fee_fields() {
        let mut tx = AlloyTransactionRequest::default()
            .to(address!("2222222222222222222222222222222222222222"))
            .gas_limit(21_000)
            .nonce(7)
            .max_fee_per_gas(2_000_000_000)
            .max_priority_fee_per_gas(1_000_000_000);
        tx.chain_id = Some(1);

        let converted = alloy_tx_request_to_ethers(&tx).unwrap();

        match converted {
            TypedTransaction::Eip1559(eip1559) => {
                assert_eq!(eip1559.max_fee_per_gas, Some(2_000_000_000u64.into()));
                assert_eq!(
                    eip1559.max_priority_fee_per_gas,
                    Some(1_000_000_000u64.into())
                );
                assert_eq!(eip1559.chain_id, Some(1u64.into()));
                assert_eq!(eip1559.gas, Some(21_000u64.into()));
                assert_eq!(eip1559.nonce, Some(7u64.into()));
            }
            other => panic!("expected EIP-1559 typed transaction, got {other:?}"),
        }
    }

    #[test]
    fn alloy_bundle_transaction_request_rejects_access_lists_instead_of_dropping_them() {
        let mut tx = AlloyTransactionRequest::default()
            .to(address!("2222222222222222222222222222222222222222"));
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
    fn alloy_bundle_transaction_request_rejects_conflicting_input_aliases() {
        let mut tx = AlloyTransactionRequest::default();
        tx.input = TransactionInput {
            input: Some(bytes!("dead").into()),
            data: Some(bytes!("beef").into()),
        };

        assert!(alloy_tx_request_to_ethers(&tx).is_err());
    }
}
