use crate::types::{Collector, CollectorStream};
use anyhow::Result;
use async_trait::async_trait;
use futures_util::{StreamExt, TryStreamExt};

use crate::mev_share::sse::Event;

/// A collector that streams from MEV-Share SSE endpoint
/// and generates [events](Event), which return tx hash, logs, and bundled txs.
pub struct MevShareCollector {
    mevshare_sse_url: String,
}

impl MevShareCollector {
    pub fn new(mevshare_sse_url: String) -> Self {
        Self { mevshare_sse_url }
    }
}

/// Implementation of the [Collector](Collector) trait for the
/// [MevShareCollector](MevShareCollector).
#[async_trait]
impl Collector<Event> for MevShareCollector {
    async fn get_event_stream<'a>(&'a self) -> Result<CollectorStream<'a, Event>> {
        let response = reqwest::Client::new()
            .get(&self.mevshare_sse_url)
            .send()
            .await?;
        let stream = async_sse::decode(
            response
                .bytes_stream()
                .map_err(std::io::Error::other)
                .into_async_read(),
        )
        .filter_map(|event| async {
            match event {
                Ok(async_sse::Event::Message(message)) => {
                    serde_json::from_slice::<Event>(message.data()).ok()
                }
                _ => None,
            }
        });
        Ok(Box::pin(stream))
    }
}
