use async_trait::async_trait;

use alloy::{providers::Provider, rpc::types::Transaction};
use std::sync::Arc;

use crate::types::{Collector, CollectorStream};
use anyhow::Result;

/// A collector that listens for new transactions in the mempool, and generates a stream of
/// [events](Transaction) which contain the transaction.
pub struct MempoolCollector<P> {
    provider: Arc<P>,
}

impl<P> MempoolCollector<P> {
    pub fn new(provider: Arc<P>) -> Self {
        Self { provider }
    }
}

/// Implementation of the [Collector](Collector) trait for the [MempoolCollector](MempoolCollector).
/// This implementation uses Alloy provider pubsub support to subscribe to full pending transactions.
#[async_trait]
impl<P> Collector<Transaction> for MempoolCollector<P>
where
    P: Provider + Send + Sync,
{
    async fn get_event_stream<'a>(&'a self) -> Result<CollectorStream<'a, Transaction>> {
        let stream = self.provider.subscribe_full_pending_transactions().await?;
        let stream = stream.into_stream();
        Ok(Box::pin(stream))
    }
}
