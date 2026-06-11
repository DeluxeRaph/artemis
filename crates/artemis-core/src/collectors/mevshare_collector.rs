use crate::types::{Collector, CollectorStream};
use anyhow::Result;
use async_trait::async_trait;
use futures_util::{StreamExt, TryStreamExt};
use tracing::warn;

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
            .await?
            .error_for_status()?;
        let stream = async_sse::decode(
            response
                .bytes_stream()
                .map_err(std::io::Error::other)
                .into_async_read(),
        )
        .filter_map(|event| async {
            match event {
                Ok(async_sse::Event::Message(message)) => {
                    match serde_json::from_slice::<Event>(message.data()) {
                        Ok(event) => Some(event),
                        Err(error) => {
                            warn!(
                                ?error,
                                payload = %String::from_utf8_lossy(message.data()),
                                "failed to decode MEV-Share SSE message"
                            );
                            None
                        }
                    }
                }
                Ok(other) => {
                    warn!(?other, "ignoring non-message MEV-Share SSE event");
                    None
                }
                Err(error) => {
                    warn!(?error, "failed to decode MEV-Share SSE event");
                    None
                }
            }
        });
        Ok(Box::pin(stream))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Collector;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    #[tokio::test]
    async fn mevshare_collector_returns_error_for_http_failure_status() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 1024];
            let _ = socket.read(&mut request).await.unwrap();
            socket
                .write_all(
                    b"HTTP/1.1 401 Unauthorized\r\ncontent-type: text/plain\r\ncontent-length: 12\r\n\r\nunauthorized",
                )
                .await
                .unwrap();
        });
        let collector = MevShareCollector::new(url);

        let err = match collector.get_event_stream().await {
            Ok(_) => panic!("collector unexpectedly accepted HTTP failure status"),
            Err(err) => err.to_string(),
        };

        assert!(err.contains("401"), "unexpected error: {err}");
    }
}
