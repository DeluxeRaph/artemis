use crate::types::{Collector, CollectorStream};
use alloy::{
    providers::Provider,
    rpc::types::{Filter, Log},
};
use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;

/// A collector that listens for new blockchain event logs based on a [Filter](Filter),
/// and generates a stream of [events](Log).
pub struct LogCollector<P> {
    provider: Arc<P>,
    filter: Filter,
}

impl<P> LogCollector<P> {
    pub fn new(provider: Arc<P>, filter: Filter) -> Self {
        Self { provider, filter }
    }
}

/// Implementation of the [Collector](Collector) trait for the [LogCollector](LogCollector).
/// This implementation uses Alloy provider pubsub support to subscribe to new logs.
#[async_trait]
impl<P> Collector<Log> for LogCollector<P>
where
    P: Provider + Send + Sync,
{
    async fn get_event_stream<'a>(&'a self) -> Result<CollectorStream<'a, Log>> {
        let stream = self.provider.subscribe_logs(&self.filter).await?;
        let stream = stream.into_stream();
        Ok(Box::pin(stream))
    }
}
