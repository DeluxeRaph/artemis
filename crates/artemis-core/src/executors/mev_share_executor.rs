use crate::types::Executor;
use alloy::{primitives::keccak256, signers::Signer as AlloySigner};
use anyhow::Result;
use async_trait::async_trait;
use futures_util::future::BoxFuture;
use http::{header::HeaderValue, HeaderName, Request};
use hyper::Body;
use jsonrpsee::http_client::{
    transport::{self},
    HttpClientBuilder,
};
use mev_share::rpc::{MevApiClient, SendBundleRequest};
use std::{
    error::Error,
    task::{Context, Poll},
};

use tower::{Layer, Service};
use tracing::{error, info};

const FLASHBOTS_HEADER: HeaderName = HeaderName::from_static("x-flashbots-signature");

/// Layer that applies Flashbots-style request authentication using an Alloy signer.
#[derive(Clone)]
struct AlloyFlashbotsSignerLayer<S> {
    signer: S,
}

impl<S> AlloyFlashbotsSignerLayer<S> {
    fn new(signer: S) -> Self {
        Self { signer }
    }
}

impl<S: Clone, I> Layer<I> for AlloyFlashbotsSignerLayer<S> {
    type Service = AlloyFlashbotsSigner<S, I>;

    fn layer(&self, inner: I) -> Self::Service {
        AlloyFlashbotsSigner {
            signer: self.signer.clone(),
            inner,
        }
    }
}

/// Middleware that adds the x-flashbots-signature header to JSON POST requests.
#[derive(Clone)]
struct AlloyFlashbotsSigner<S, I> {
    signer: S,
    inner: I,
}

impl<S, I> Service<Request<Body>> for AlloyFlashbotsSigner<S, I>
where
    I: Service<Request<Body>> + Clone + Send + 'static,
    I::Future: Send,
    I::Error: Into<Box<dyn Error + Send + Sync>> + 'static,
    S: AlloySigner + Clone + Send + Sync + 'static,
{
    type Response = I::Response;
    type Error = Box<dyn Error + Send + Sync>;
    type Future = BoxFuture<'static, std::result::Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<std::result::Result<(), Self::Error>> {
        self.inner.poll_ready(cx).map_err(Into::into)
    }

    fn call(&mut self, request: Request<Body>) -> Self::Future {
        let clone = self.inner.clone();
        let mut inner = std::mem::replace(&mut self.inner, clone);
        let signer = self.signer.clone();

        let (mut parts, body) = request.into_parts();

        if parts.method != http::Method::POST {
            return Box::pin(async move {
                Err(format!("Invalid method: {}", parts.method.as_str()).into())
            });
        }

        let is_json = parts
            .headers
            .get(http::header::CONTENT_TYPE)
            .map(|v| v == HeaderValue::from_static("application/json"))
            .unwrap_or(false);
        let has_sig = parts.headers.contains_key(FLASHBOTS_HEADER);

        if !is_json || has_sig {
            return Box::pin(async move {
                let request = Request::from_parts(parts, body);
                inner.call(request).await.map_err(Into::into)
            });
        }

        Box::pin(async move {
            let body_bytes = hyper::body::to_bytes(body).await?;
            let header = flashbots_signature_header(&signer, body_bytes.as_ref()).await?;
            parts.headers.insert(FLASHBOTS_HEADER, header);

            let request = Request::from_parts(parts, Body::from(body_bytes.clone()));
            inner.call(request).await.map_err(Into::into)
        })
    }
}

async fn flashbots_signature_header<S>(signer: &S, body: &[u8]) -> Result<HeaderValue>
where
    S: AlloySigner + Sync,
{
    let body_hash = keccak256(body);
    let message = format!("{body_hash:#x}");
    let signature = signer.sign_message(message.as_bytes()).await?;
    Ok(HeaderValue::from_str(&format!(
        "{}:{}",
        signer.address(),
        signature
    ))?)
}

/// An executor that sends bundles to the MEV-share Matchmaker.
pub struct MevshareExecutor {
    mev_share_client: Box<dyn MevApiClient + Send + Sync>,
}

impl MevshareExecutor {
    pub fn new(signer: impl AlloySigner + Clone + Send + Sync + 'static) -> Self {
        // Set up flashbots-style auth middleware
        let http = HttpClientBuilder::default()
            .set_middleware(
                tower::ServiceBuilder::new()
                    .map_err(transport::Error::Http)
                    .layer(AlloyFlashbotsSignerLayer::new(signer)),
            )
            .build("https://relay.flashbots.net:443")
            .expect("failed to build HTTP client");
        Self {
            mev_share_client: Box::new(http),
        }
    }
}

#[async_trait]
impl Executor<SendBundleRequest> for MevshareExecutor {
    /// Send bundles to the matchmaker.
    async fn execute(&self, action: SendBundleRequest) -> Result<()> {
        let body = self.mev_share_client.send_bundle(action).await;
        match body {
            Ok(body) => info!("Bundle response: {:?}", body),
            Err(e) => error!("Bundle error: {}", e),
        };
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::signers::{local::PrivateKeySigner, Signer};

    #[test]
    fn mevshare_executor_accepts_alloy_signer() {
        let signer = PrivateKeySigner::random();
        let _executor = MevshareExecutor::new(signer);
    }

    #[tokio::test]
    async fn flashbots_signature_header_uses_alloy_signer() {
        let signer = PrivateKeySigner::random();
        let body = br#"{"jsonrpc":"2.0","method":"mev_sendBundle"}"#;

        let header = flashbots_signature_header(&signer, body).await.unwrap();
        let header = header.to_str().unwrap();
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
}
