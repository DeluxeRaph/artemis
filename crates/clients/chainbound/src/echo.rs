use std::{sync::Arc, time::Duration};

use alloy::{
    consensus::{TxEnvelope, TypedTransaction},
    eips::eip2718::Encodable2718,
    network::{Ethereum, NetworkWallet},
    providers::Provider,
    rpc::types::TransactionRequest as AlloyTransactionRequest,
    signers::Signer,
};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
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
pub struct EchoExecutor<M, W, A> {
    /// The Echo RPC endpoint
    echo_endpoint: String,
    /// The HTTP client to send requests to the Echo RPC
    echo_client: Client,
    /// The Alloy provider used to query chain state.
    inner: Arc<M>,
    /// The signer to sign transactions before sending to the builders
    tx_signer: W,
    /// the signer to compute the `X-Flashbots-Signature` of the bundle payload
    auth_signer: A,
}

impl<M, W, A> EchoExecutor<M, W, A>
where
    M: Provider,
    W: NetworkWallet<Ethereum>,
    A: Signer + Send + Sync,
{
    /// Initialize a new Echo executor.
    ///
    /// ## Arguments
    /// - `inner`: The Alloy provider that can query the blockchain
    /// - `tx_signer`: The actual signer of the bundle transactions
    /// - `auth_signer`: The signer to compute the `X-Flashbots-Signature` of the bundle payload
    /// - `api_key`: The Echo API key to use
    pub fn new(inner: Arc<M>, tx_signer: W, auth_signer: A, api_key: impl Into<String>) -> Self {
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

    /// Returns a reference to the Alloy provider.
    pub fn provider(&self) -> Arc<M> {
        self.inner.clone()
    }
}

#[async_trait]
impl<M, W, A> Executor<SendBundleArgs> for EchoExecutor<M, W, A>
where
    M: Provider + 'static,
    W: NetworkWallet<Ethereum> + 'static,
    A: Signer + Send + Sync + 'static,
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
            let tx = alloy_tx_request_to_typed(tx)?;
            let signed = sign_transaction(&self.tx_signer, tx).await?;
            action.standard_features.txs.push(signed);
        }

        // Set block number to the next block if not specified
        if action.standard_features.block_number.is_none() {
            let block_number = self.inner.get_block_number().await?;
            let next_block_number_hex = format!("0x{:x}", block_number + 1);
            action.standard_features.block_number = Some(next_block_number_hex);
        }

        // TODO: Simulate bundle

        // Sign bundle payload (without the Echo-specific features)
        let signable_payload = serde_json::to_string(&action.standard_features)?;
        let flashbots_signature = self
            .auth_signer
            .sign_message(signable_payload.as_bytes())
            .await?;

        // Create the `X-Flashbots-Signature` header
        let flashbots_signature_header: HeaderValue =
            format!("{}:{}", self.auth_signer.address(), flashbots_signature).parse()?;

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

fn alloy_tx_request_to_typed(tx: &AlloyTransactionRequest) -> Result<TypedTransaction> {
    reject_unsupported_alloy_bundle_fields(tx)?;

    tx.clone().build_typed_tx().map_err(|unbuilt| {
        anyhow!("bundle transaction request cannot be built for signing: {unbuilt:?}")
    })
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
    let has_eip1559_fee_fields =
        tx.max_fee_per_gas.is_some() || tx.max_priority_fee_per_gas.is_some();
    if tx.gas_price.is_some() && (tx.transaction_type == Some(2) || has_eip1559_fee_fields) {
        return Err(anyhow!(
            "bundle transaction request cannot mix gas_price with EIP-1559 transaction type or fee fields"
        ));
    }

    Ok(())
}

async fn sign_transaction<W>(wallet: &W, tx: TypedTransaction) -> Result<String>
where
    W: NetworkWallet<Ethereum>,
{
    let signed = wallet.sign_transaction(tx).await?;
    Ok(encoded_envelope_hex(&signed))
}

fn encoded_envelope_hex(envelope: &TxEnvelope) -> String {
    let mut encoded = Vec::with_capacity(envelope.encode_2718_len());
    envelope.encode_2718(&mut encoded);
    format!("0x{}", alloy::hex::encode(encoded))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::{
        primitives::{address, b256, bytes, TxKind, U256},
        rpc::types::{AccessList, AccessListItem, TransactionInput},
    };

    #[test]
    fn alloy_bundle_transaction_request_preserves_fields_for_alloy_signing() {
        let mut tx = AlloyTransactionRequest::default()
            .from(address!("1111111111111111111111111111111111111111"))
            .to(address!("2222222222222222222222222222222222222222"))
            .value(U256::from(1234))
            .gas_limit(21_000)
            .nonce(7)
            .gas_price(1_500_000_000)
            .input(TransactionInput::new(bytes!("deadbeef")));
        tx.chain_id = Some(1);

        let converted = alloy_tx_request_to_typed(&tx).unwrap();

        let TypedTransaction::Legacy(converted) = converted else {
            panic!("expected legacy typed transaction");
        };
        assert_eq!(
            converted.to,
            TxKind::Call(address!("2222222222222222222222222222222222222222"))
        );
        assert_eq!(converted.value, U256::from(1234));
        assert_eq!(converted.gas_limit, 21_000);
        assert_eq!(converted.nonce, 7);
        assert_eq!(converted.gas_price, 1_500_000_000);
        assert_eq!(converted.chain_id, Some(1u64.into()));
        assert_eq!(converted.input.as_ref(), bytes!("deadbeef").as_ref());
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

        let converted = alloy_tx_request_to_typed(&tx).unwrap();

        match converted {
            TypedTransaction::Eip1559(eip1559) => {
                assert_eq!(eip1559.max_fee_per_gas, 2_000_000_000);
                assert_eq!(eip1559.max_priority_fee_per_gas, 1_000_000_000);
                assert_eq!(eip1559.chain_id, 1);
                assert_eq!(eip1559.gas_limit, 21_000);
                assert_eq!(eip1559.nonce, 7);
            }
            other => panic!("expected EIP-1559 typed transaction, got {other:?}"),
        }
    }

    #[test]
    fn alloy_bundle_transaction_request_uses_explicit_eip1559_type_with_fee_fields() {
        let mut tx = AlloyTransactionRequest::default()
            .to(address!("2222222222222222222222222222222222222222"))
            .gas_limit(21_000)
            .nonce(7)
            .max_fee_per_gas(2_000_000_000)
            .max_priority_fee_per_gas(1_000_000_000);
        tx.transaction_type = Some(2);

        let converted = alloy_tx_request_to_typed(&tx).unwrap();

        match converted {
            TypedTransaction::Eip1559(eip1559) => {
                assert_eq!(eip1559.max_fee_per_gas, 2_000_000_000);
                assert_eq!(eip1559.max_priority_fee_per_gas, 1_000_000_000);
            }
            other => panic!("expected explicit type 2 to produce EIP-1559, got {other:?}"),
        }
    }

    #[test]
    fn alloy_bundle_transaction_request_rejects_incomplete_explicit_eip1559_type() {
        let mut tx = AlloyTransactionRequest::default()
            .to(address!("2222222222222222222222222222222222222222"))
            .gas_limit(21_000)
            .nonce(7);
        tx.transaction_type = Some(2);

        let err = alloy_tx_request_to_typed(&tx).unwrap_err().to_string();
        assert!(
            err.contains("max_fee_per_gas") || err.contains("max_priority_fee_per_gas"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn alloy_bundle_transaction_request_rejects_explicit_eip1559_type_with_gas_price() {
        let mut tx = AlloyTransactionRequest::default().gas_price(1_500_000_000);
        tx.transaction_type = Some(2);

        let err = alloy_tx_request_to_typed(&tx).unwrap_err().to_string();

        assert!(err.contains("gas_price"), "unexpected error: {err}");
        assert!(err.contains("EIP-1559"), "unexpected error: {err}");
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

        let err = alloy_tx_request_to_typed(&tx).unwrap_err().to_string();
        assert!(err.contains("access_list"), "unexpected error: {err}");
    }

    #[test]
    fn alloy_bundle_transaction_request_rejects_conflicting_input_aliases() {
        let tx = AlloyTransactionRequest {
            input: TransactionInput {
                input: Some(bytes!("dead")),
                data: Some(bytes!("beef")),
            },
            ..Default::default()
        };

        assert!(alloy_tx_request_to_typed(&tx).is_err());
    }

    #[tokio::test]
    async fn alloy_bundle_transaction_signing_returns_raw_2718_hex() {
        use alloy::{network::EthereumWallet, signers::local::PrivateKeySigner};

        let wallet = EthereumWallet::new(PrivateKeySigner::random());
        let mut tx = AlloyTransactionRequest::default()
            .to(address!("2222222222222222222222222222222222222222"))
            .value(U256::from(1234))
            .gas_limit(21_000)
            .nonce(7)
            .gas_price(1_500_000_000);
        tx.chain_id = Some(1);

        let typed = alloy_tx_request_to_typed(&tx).unwrap();
        let signed = sign_transaction(&wallet, typed).await.unwrap();

        assert!(signed.starts_with("0x"), "unexpected signed tx: {signed}");
        assert!(signed.len() > 2, "signed tx must not be empty");
    }
}
