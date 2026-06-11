use crate::{
    mev_share::rpc::{SendBundleRequest, SendBundleResponse},
    types::Executor,
};
use alloy::{primitives::keccak256, signers::Signer as AlloySigner};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use reqwest::Url;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;
use tracing::{error, info};

const FLASHBOTS_HEADER: &str = "x-flashbots-signature";

/// Build a Flashbots-style signature header value from a JSON request body.
pub(crate) async fn flashbots_signature_header_value<S>(signer: &S, body: &[u8]) -> Result<String>
where
    S: AlloySigner + Sync + ?Sized,
{
    let body_hash = keccak256(body);
    let message = format!("{body_hash:#x}");
    let signature = signer.sign_message(message.as_bytes()).await?;
    Ok(format!("{}:{}", signer.address(), signature))
}

/// An executor that sends bundles to the MEV-Share matchmaker.
pub struct MevshareExecutor {
    relay_url: Url,
    http: reqwest::Client,
    signer: Arc<dyn AlloySigner + Send + Sync>,
}

impl MevshareExecutor {
    pub fn new(signer: impl AlloySigner + Clone + Send + Sync + 'static) -> Self {
        Self::with_relay_url(signer, "https://relay.flashbots.net:443".parse().unwrap())
    }

    pub fn with_relay_url(
        signer: impl AlloySigner + Send + Sync + 'static,
        relay_url: Url,
    ) -> Self {
        Self {
            relay_url,
            http: reqwest::Client::new(),
            signer: Arc::new(signer),
        }
    }

    async fn send_bundle(&self, action: SendBundleRequest) -> Result<SendBundleResponse> {
        let response = self.rpc("mev_sendBundle", json!([action])).await?;
        Ok(serde_json::from_value(response)?)
    }

    async fn rpc(&self, method: &str, params: Value) -> Result<Value> {
        let body = json!({
            "jsonrpc": "2.0",
            "id": 1_u64,
            "method": method,
            "params": params,
        });
        let body = serde_json::to_vec(&body)?;
        let signature = flashbots_signature_header_value(self.signer.as_ref(), &body).await?;

        let response = self
            .http
            .post(self.relay_url.clone())
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header(FLASHBOTS_HEADER, signature)
            .body(body)
            .send()
            .await?;

        let status = response.status();
        let value: JsonRpcResponse = response.json().await?;
        if !status.is_success() {
            return Err(anyhow!("MEV-Share relay HTTP {status}: {:?}", value.error));
        }
        if let Some(error) = value.error {
            return Err(anyhow!("MEV-Share relay RPC error: {error}"));
        }

        Ok(value.result.unwrap_or(Value::Null))
    }
}

#[async_trait]
impl Executor<SendBundleRequest> for MevshareExecutor {
    /// Send bundles to the matchmaker.
    async fn execute(&self, action: SendBundleRequest) -> Result<()> {
        match self.send_bundle(action).await {
            Ok(body) => info!("Bundle response: {:?}", body),
            Err(e) => error!("Bundle error: {}", e),
        };
        Ok(())
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct JsonRpcResponse {
    #[allow(dead_code)]
    jsonrpc: String,
    #[allow(dead_code)]
    id: Value,
    result: Option<Value>,
    error: Option<Value>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::{
        primitives::{address, b256, bytes},
        signers::{local::PrivateKeySigner, Signer},
    };

    #[test]
    fn mevshare_executor_accepts_alloy_signer() {
        let signer = PrivateKeySigner::random();
        let _executor = MevshareExecutor::new(signer);
    }

    #[tokio::test]
    async fn flashbots_signature_header_uses_alloy_signer() {
        let signer = PrivateKeySigner::random();
        let body = br#"{"jsonrpc":"2.0","method":"mev_sendBundle"}"#;

        let header = flashbots_signature_header_value(&signer, body)
            .await
            .unwrap();
        let (address, signature) = header.split_once(':').unwrap();

        let body_hash = keccak256(body);
        let message = format!("{body_hash:#x}");
        let expected_signature = signer.sign_message(message.as_bytes()).await.unwrap();

        assert_eq!(address, signer.address().to_string());
        assert_eq!(signature, expected_signature.to_string());
        assert_eq!(
            expected_signature
                .recover_address_from_msg(message.as_bytes())
                .unwrap(),
            signer.address()
        );
    }

    #[test]
    fn send_bundle_request_serializes_mev_share_wire_shape() {
        let request = SendBundleRequest::new(
            1,
            Some(2),
            crate::mev_share::rpc::ProtocolVersion::V0_1,
            vec![
                crate::mev_share::rpc::BundleItem::Hash {
                    hash: b256!("1111111111111111111111111111111111111111111111111111111111111111"),
                },
                crate::mev_share::rpc::BundleItem::Tx {
                    tx: bytes!("deadbeef"),
                    can_revert: false,
                },
            ],
        );

        let json = serde_json::to_value(request).unwrap();

        assert_eq!(json["version"], "v0.1");
        assert_eq!(json["inclusion"]["block"], "0x1");
        assert_eq!(json["inclusion"]["maxBlock"], "0x2");
        assert_eq!(
            json["body"][0]["hash"],
            "0x1111111111111111111111111111111111111111111111111111111111111111"
        );
        assert_eq!(json["body"][1]["tx"], "0xdeadbeef");
        assert_eq!(json["body"][1]["canRevert"], false);
    }

    #[test]
    fn mev_share_sse_event_deserializes_null_sequences() {
        let raw = serde_json::json!({
            "hash": "0x9d525cbf4ed0cd367df93a685da93da036bf5c6d0d6e9e31945779ddbca31d3b",
            "txs": null,
            "logs": [{
                "address": address!("074201cb10b1efedbd8dec271c37687e1ab5be4e"),
                "topics": ["0xd78ad95fa46c994b6551d0da85fc275fe613ce37657fb8d5e3d130840159d822"]
            }]
        });

        let event: crate::mev_share::sse::Event = serde_json::from_value(raw).unwrap();

        assert!(event.transactions.is_empty());
        assert_eq!(event.logs.len(), 1);
    }
}
