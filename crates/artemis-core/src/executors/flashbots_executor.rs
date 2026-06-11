use std::sync::Arc;

use alloy::{
    consensus::{transaction::RlpEcdsaEncodableTx, SignableTransaction, TxEip1559, TxLegacy},
    network::TxSigner,
    primitives::{Bytes, Signature, TxKind},
    providers::Provider,
    rpc::types::{AccessList, TransactionRequest},
};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use reqwest::Url;
use serde::Serialize;
use serde_json::{json, Value};
use tracing::info;

use crate::types::Executor;

/// A Flashbots executor that sends transactions to the Flashbots relay.
pub struct FlashbotsExecutor<P, S> {
    client: Arc<P>,
    relay_client: FlashbotsRelayClient,
    tx_signer: S,
}

/// A bundle of Alloy transaction requests to send to the Flashbots relay.
pub type FlashbotsBundle = Vec<TransactionRequest>;

impl<P, S> FlashbotsExecutor<P, S> {
    pub fn new(client: Arc<P>, tx_signer: S, relay_signer: S, relay_url: impl Into<Url>) -> Self
    where
        S: alloy::signers::Signer + Clone + Send + Sync + 'static,
    {
        Self {
            client,
            relay_client: FlashbotsRelayClient::new(relay_url.into(), relay_signer),
            tx_signer,
        }
    }
}

#[async_trait]
impl<P, S> Executor<FlashbotsBundle> for FlashbotsExecutor<P, S>
where
    P: Provider + 'static,
    S: TxSigner<Signature> + Send + Sync + 'static,
{
    /// Send a bundle of signed transactions to the Flashbots relay.
    async fn execute(&self, action: FlashbotsBundle) -> Result<()> {
        let mut txs = Vec::with_capacity(action.len());
        for tx in action {
            txs.push(sign_bundle_transaction(&self.tx_signer, &tx).await?);
        }

        let block_number = self.client.get_block_number().await?;
        let target_block = block_number + 1;
        let bundle = RelayBundleRequest {
            txs,
            block_number: quantity_hex(target_block),
            min_timestamp: None,
            max_timestamp: None,
            reverting_tx_hashes: Vec::new(),
        };

        let simulation = self.relay_client.call_bundle(&bundle, block_number).await?;
        info!("Flashbots bundle simulation response: {:?}", simulation);

        let response = self.relay_client.send_bundle(&bundle).await?;
        info!("Flashbots bundle response: {:?}", response);

        Ok(())
    }
}

#[derive(Clone)]
struct FlashbotsRelayClient {
    relay_url: Url,
    http: reqwest::Client,
    relay_signer: Arc<dyn alloy::signers::Signer + Send + Sync>,
}

impl FlashbotsRelayClient {
    fn new<S>(relay_url: Url, relay_signer: S) -> Self
    where
        S: alloy::signers::Signer + Send + Sync + 'static,
    {
        Self {
            relay_url,
            http: reqwest::Client::new(),
            relay_signer: Arc::new(relay_signer),
        }
    }

    async fn send_bundle(&self, bundle: &RelayBundleRequest) -> Result<Value> {
        self.rpc("eth_sendBundle", json!([bundle])).await
    }

    async fn call_bundle(&self, bundle: &RelayBundleRequest, state_block: u64) -> Result<Value> {
        self.rpc(
            "eth_callBundle",
            json!([{
                "txs": bundle.txs,
                "blockNumber": bundle.block_number,
                "stateBlockNumber": quantity_hex(state_block),
                "timestamp": 0_u64,
            }]),
        )
        .await
    }

    async fn rpc(&self, method: &str, params: Value) -> Result<Value> {
        let body = json!({
            "jsonrpc": "2.0",
            "id": 1_u64,
            "method": method,
            "params": params,
        });
        let body = serde_json::to_vec(&body)?;
        let signature = crate::executors::mev_share_executor::flashbots_signature_header_value(
            self.relay_signer.as_ref(),
            &body,
        )
        .await?;

        let response = self
            .http
            .post(self.relay_url.clone())
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header("x-flashbots-signature", signature)
            .body(body)
            .send()
            .await?;

        let status = response.status();
        let value: Value = response.json().await?;
        if !status.is_success() {
            return Err(anyhow!("Flashbots relay HTTP {status}: {value}"));
        }
        if let Some(error) = value.get("error") {
            return Err(anyhow!("Flashbots relay RPC error: {error}"));
        }

        Ok(value.get("result").cloned().unwrap_or(Value::Null))
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RelayBundleRequest {
    txs: Vec<Bytes>,
    block_number: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    min_timestamp: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_timestamp: Option<u64>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    reverting_tx_hashes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum BundleTransaction {
    Legacy(TxLegacy),
    Eip1559(TxEip1559),
}

impl BundleTransaction {
    fn signable(&mut self) -> &mut dyn SignableTransaction<Signature> {
        match self {
            Self::Legacy(tx) => tx,
            Self::Eip1559(tx) => tx,
        }
    }

    fn network_encode(&self, signature: &Signature) -> Bytes {
        let mut encoded = Vec::new();
        match self {
            Self::Legacy(tx) => tx.network_encode(signature, &mut encoded),
            Self::Eip1559(tx) => tx.network_encode(signature, &mut encoded),
        }
        encoded.into()
    }
}

async fn sign_bundle_transaction<S>(signer: &S, tx: &TransactionRequest) -> Result<Bytes>
where
    S: TxSigner<Signature> + Send + Sync,
{
    if let Some(from) = tx.from {
        let signer_address = TxSigner::address(signer);
        if from != signer_address {
            return Err(anyhow!(
                "Flashbots bundle transaction request from address {from} does not match signer address {signer_address}"
            ));
        }
    }

    let mut tx = alloy_tx_request_to_signable(tx)?;
    let signature = signer.sign_transaction(tx.signable()).await?;
    Ok(tx.network_encode(&signature))
}

fn alloy_tx_request_to_signable(tx: &TransactionRequest) -> Result<BundleTransaction> {
    reject_unsupported_alloy_bundle_fields(tx)?;

    let is_explicit_eip1559 = tx.transaction_type == Some(2);
    let has_eip1559_fee_fields = tx.has_eip1559_fields();

    if is_explicit_eip1559 || has_eip1559_fee_fields {
        if tx.gas_price.is_some() {
            return Err(anyhow!(
                "Flashbots bundle transaction request cannot mix gas_price with EIP-1559 transaction type or fee fields"
            ));
        }

        return Ok(BundleTransaction::Eip1559(alloy_tx_request_to_eip1559(tx)?));
    }

    Ok(BundleTransaction::Legacy(alloy_tx_request_to_legacy(tx)?))
}

fn reject_unsupported_alloy_bundle_fields(tx: &TransactionRequest) -> Result<()> {
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

fn alloy_tx_request_to_legacy(tx: &TransactionRequest) -> Result<TxLegacy> {
    let Some(nonce) = tx.nonce else {
        return Err(anyhow!(
            "legacy Flashbots bundle transaction request missing nonce"
        ));
    };
    let Some(gas_price) = tx.gas_price else {
        return Err(anyhow!(
            "legacy Flashbots bundle transaction request missing gas_price"
        ));
    };
    let Some(gas_limit) = tx.gas else {
        return Err(anyhow!(
            "legacy Flashbots bundle transaction request missing gas limit"
        ));
    };

    Ok(TxLegacy {
        chain_id: tx.chain_id,
        nonce,
        gas_price,
        gas_limit,
        to: tx.to.unwrap_or(TxKind::Create),
        value: tx.value.unwrap_or_default(),
        input: tx
            .input
            .clone()
            .unique_input()?
            .cloned()
            .unwrap_or_default(),
    })
}

fn alloy_tx_request_to_eip1559(tx: &TransactionRequest) -> Result<TxEip1559> {
    let Some(chain_id) = tx.chain_id else {
        return Err(anyhow!(
            "EIP-1559 Flashbots bundle transaction request missing chain_id"
        ));
    };
    let Some(nonce) = tx.nonce else {
        return Err(anyhow!(
            "EIP-1559 Flashbots bundle transaction request missing nonce"
        ));
    };
    let Some(max_fee_per_gas) = tx.max_fee_per_gas else {
        return Err(anyhow!(
            "EIP-1559 Flashbots bundle transaction request missing max_fee_per_gas"
        ));
    };
    let Some(max_priority_fee_per_gas) = tx.max_priority_fee_per_gas else {
        return Err(anyhow!(
            "EIP-1559 Flashbots bundle transaction request missing max_priority_fee_per_gas"
        ));
    };
    let Some(gas_limit) = tx.gas else {
        return Err(anyhow!(
            "EIP-1559 Flashbots bundle transaction request missing gas limit"
        ));
    };

    Ok(TxEip1559 {
        chain_id,
        nonce,
        gas_limit,
        max_fee_per_gas,
        max_priority_fee_per_gas,
        to: tx.to.unwrap_or(TxKind::Create),
        value: tx.value.unwrap_or_default(),
        access_list: AccessList::default(),
        input: tx
            .input
            .clone()
            .unique_input()?
            .cloned()
            .unwrap_or_default(),
    })
}

fn quantity_hex(value: u64) -> String {
    format!("0x{value:x}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::{
        primitives::{address, b256, bytes, U256},
        providers::{mock::Asserter, ProviderBuilder},
        rpc::types::{AccessListItem, TransactionInput},
        signers::local::PrivateKeySigner,
    };
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
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

        let converted = alloy_tx_request_to_signable(&tx).unwrap();

        let BundleTransaction::Legacy(converted) = converted else {
            panic!("expected legacy transaction");
        };
        assert_eq!(converted.chain_id, Some(1));
        assert_eq!(
            converted.to,
            TxKind::Call(address!("2222222222222222222222222222222222222222"))
        );
        assert_eq!(converted.value, U256::from(1234));
        assert_eq!(converted.gas_limit, 21_000);
        assert_eq!(converted.nonce, 7);
        assert_eq!(converted.gas_price, 1_500_000_000);
        assert_eq!(converted.input, bytes!("deadbeef"));
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

        let converted = alloy_tx_request_to_signable(&tx).unwrap();

        match converted {
            BundleTransaction::Eip1559(eip1559) => {
                assert_eq!(eip1559.chain_id, 1);
                assert_eq!(
                    eip1559.to,
                    TxKind::Call(address!("2222222222222222222222222222222222222222"))
                );
                assert_eq!(eip1559.value, U256::from(1234));
                assert_eq!(eip1559.max_fee_per_gas, 2_000_000_000);
                assert_eq!(eip1559.max_priority_fee_per_gas, 1_000_000_000);
                assert_eq!(eip1559.gas_limit, 21_000);
                assert_eq!(eip1559.nonce, 7);
                assert_eq!(eip1559.input, bytes!("deadbeef"));
            }
            other => panic!("expected EIP-1559 transaction, got {other:?}"),
        }
    }

    #[test]
    fn flashbots_bundle_transaction_request_preserves_contract_creation() {
        let tx = TransactionRequest::default()
            .create()
            .gas_limit(100_000)
            .nonce(1)
            .gas_price(1)
            .input(TransactionInput::new(bytes!("60806040")));

        let converted = alloy_tx_request_to_signable(&tx).unwrap();

        let BundleTransaction::Legacy(converted) = converted else {
            panic!("expected legacy transaction");
        };
        assert_eq!(converted.to, TxKind::Create);
        assert_eq!(converted.gas_limit, 100_000);
        assert_eq!(converted.input, bytes!("60806040"));
    }

    #[tokio::test]
    async fn flashbots_bundle_transaction_signs_to_raw_bytes() {
        let signer = PrivateKeySigner::random();
        let mut tx = TransactionRequest::default()
            .to(address!("2222222222222222222222222222222222222222"))
            .value(U256::from(1234))
            .gas_limit(21_000)
            .nonce(7)
            .gas_price(1_500_000_000)
            .input(TransactionInput::new(bytes!("deadbeef")));
        tx.chain_id = Some(1);

        let raw = sign_bundle_transaction(&signer, &tx).await.unwrap();

        assert!(!raw.is_empty());
    }

    #[tokio::test]
    async fn flashbots_execute_returns_error_when_relay_rejects_send_bundle() {
        let asserter = Asserter::new();
        asserter.push_success(&123_u64);
        let provider = Arc::new(ProviderBuilder::new().connect_mocked_client(asserter));

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let relay_url: Url = format!("http://{}", listener.local_addr().unwrap())
            .parse()
            .unwrap();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 4096];
            let _ = socket.read(&mut request).await.unwrap();
            write_http_response(
                &mut socket,
                200,
                "OK",
                r#"{"jsonrpc":"2.0","id":1,"result":{"bundleHash":"0xabc"}}"#,
            )
            .await;

            let (mut socket, _) = listener.accept().await.unwrap();

            let mut request = [0_u8; 4096];
            let _ = socket.read(&mut request).await.unwrap();
            write_http_response(
                &mut socket,
                500,
                "Internal Server Error",
                r#"{"jsonrpc":"2.0","id":1,"error":{"message":"boom"}}"#,
            )
            .await;
        });

        let tx_signer = PrivateKeySigner::random();
        let relay_signer = PrivateKeySigner::random();
        let executor = FlashbotsExecutor::new(provider, tx_signer.clone(), relay_signer, relay_url);
        let mut tx = TransactionRequest::default()
            .from(tx_signer.address())
            .to(address!("2222222222222222222222222222222222222222"))
            .value(U256::from(1234))
            .gas_limit(21_000)
            .nonce(7)
            .gas_price(1_500_000_000)
            .input(TransactionInput::new(bytes!("deadbeef")));
        tx.chain_id = Some(1);

        let err = executor.execute(vec![tx]).await.unwrap_err().to_string();

        assert!(err.contains("500"), "unexpected error: {err}");
    }

    async fn write_http_response(
        socket: &mut tokio::net::TcpStream,
        status: u16,
        reason: &str,
        body: &str,
    ) {
        let response = format!(
            "HTTP/1.1 {status} {reason}\r\ncontent-type: application/json\r\nconnection: close\r\ncontent-length: {}\r\n\r\n{body}",
            body.len()
        );
        socket.write_all(response.as_bytes()).await.unwrap();
    }

    #[tokio::test]
    async fn flashbots_bundle_transaction_accepts_matching_from_address() {
        let signer = PrivateKeySigner::random();
        let mut tx = TransactionRequest::default()
            .from(signer.address())
            .to(address!("2222222222222222222222222222222222222222"))
            .value(U256::from(1234))
            .gas_limit(21_000)
            .nonce(7)
            .gas_price(1_500_000_000)
            .input(TransactionInput::new(bytes!("deadbeef")));
        tx.chain_id = Some(1);

        let raw = sign_bundle_transaction(&signer, &tx).await.unwrap();

        assert!(!raw.is_empty());
    }

    #[tokio::test]
    async fn flashbots_bundle_transaction_rejects_mismatched_from_address() {
        let signer = PrivateKeySigner::random();
        let mut tx = TransactionRequest::default()
            .from(address!("1111111111111111111111111111111111111111"))
            .to(address!("2222222222222222222222222222222222222222"))
            .value(U256::from(1234))
            .gas_limit(21_000)
            .nonce(7)
            .gas_price(1_500_000_000)
            .input(TransactionInput::new(bytes!("deadbeef")));
        tx.chain_id = Some(1);

        let err = sign_bundle_transaction(&signer, &tx)
            .await
            .unwrap_err()
            .to_string();

        assert!(err.contains("from"), "unexpected error: {err}");
        assert!(
            err.contains("does not match signer"),
            "unexpected error: {err}"
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

        let err = alloy_tx_request_to_signable(&tx).unwrap_err().to_string();

        assert!(err.contains("access_list"), "unexpected error: {err}");
    }

    #[test]
    fn flashbots_bundle_transaction_request_rejects_eip2930_type() {
        let tx = TransactionRequest::default().transaction_type(1);

        let err = alloy_tx_request_to_signable(&tx).unwrap_err().to_string();

        assert!(
            err.contains("unsupported transaction_type 1"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn flashbots_bundle_transaction_request_rejects_blob_fields() {
        let tx = TransactionRequest::default().max_fee_per_blob_gas(1);

        let err = alloy_tx_request_to_signable(&tx).unwrap_err().to_string();

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

        let err = alloy_tx_request_to_signable(&tx).unwrap_err().to_string();

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

        let err = alloy_tx_request_to_signable(&tx).unwrap_err().to_string();

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

        let err = alloy_tx_request_to_signable(&tx).unwrap_err().to_string();

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

        let err = alloy_tx_request_to_signable(&tx).unwrap_err().to_string();

        assert!(
            err.contains("cannot mix gas_price with EIP-1559"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn flashbots_bundle_transaction_request_uses_explicit_eip1559_type() {
        let mut tx = TransactionRequest::default()
            .transaction_type(2)
            .gas_limit(21_000)
            .nonce(7)
            .max_fee_per_gas(2_000_000_000)
            .max_priority_fee_per_gas(1_000_000_000);
        tx.chain_id = Some(1);

        let converted = alloy_tx_request_to_signable(&tx).unwrap();

        match converted {
            BundleTransaction::Eip1559(eip1559) => {
                assert_eq!(eip1559.gas_limit, 21_000);
                assert_eq!(eip1559.max_fee_per_gas, 2_000_000_000);
                assert_eq!(eip1559.max_priority_fee_per_gas, 1_000_000_000);
            }
            other => panic!("expected EIP-1559 transaction, got {other:?}"),
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

        assert!(alloy_tx_request_to_signable(&tx).is_err());
    }

    #[test]
    fn flashbots_bundle_transaction_request_rejects_incomplete_legacy_tx() {
        let tx = TransactionRequest::default().gas_price(1);

        let err = alloy_tx_request_to_signable(&tx).unwrap_err().to_string();

        assert!(err.contains("missing nonce"), "unexpected error: {err}");
    }

    #[test]
    fn flashbots_bundle_transaction_request_rejects_incomplete_eip1559_tx() {
        let tx = TransactionRequest::default()
            .transaction_type(2)
            .max_fee_per_gas(2_000_000_000);

        let err = alloy_tx_request_to_signable(&tx).unwrap_err().to_string();

        assert!(err.contains("missing chain_id"), "unexpected error: {err}");
    }
}
