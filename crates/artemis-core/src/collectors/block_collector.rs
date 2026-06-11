use crate::types::{Collector, CollectorStream};
use alloy::{primitives::B256, providers::Provider};
use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;
use tokio_stream::StreamExt;

/// A collector that listens for new blocks, and generates a stream of
/// [events](NewBlock) which contain the block number and hash.
pub struct BlockCollector<P> {
    provider: Arc<P>,
}

/// A new block event, containing the block number and hash.
#[derive(Debug, Clone)]
pub struct NewBlock {
    pub hash: B256,
    pub number: u64,
}

impl<P> BlockCollector<P> {
    pub fn new(provider: Arc<P>) -> Self {
        Self { provider }
    }
}

/// Implementation of the [Collector](Collector) trait for the [BlockCollector](BlockCollector).
/// This implementation uses Alloy provider pubsub support to subscribe to new block headers.
#[async_trait]
impl<P> Collector<NewBlock> for BlockCollector<P>
where
    P: Provider + Send + Sync,
{
    async fn get_event_stream<'a>(&'a self) -> Result<CollectorStream<'a, NewBlock>> {
        let stream = self.provider.subscribe_blocks().await?;
        let stream = stream.into_stream().map(|header| NewBlock {
            hash: header.hash,
            number: header.number,
        });
        Ok(Box::pin(stream))
    }
}
