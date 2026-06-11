use async_trait::async_trait;

use alloy::{primitives::B256, providers::Provider, rpc::types::Transaction};
use futures_util::{Stream, StreamExt};
use std::future::Future;
use std::sync::Arc;

use crate::types::{Collector, CollectorStream};
use anyhow::Result;

const PENDING_TRANSACTION_FETCH_CONCURRENCY: usize = 256;

fn pending_hashes_to_fetched_items<'a, S, F, Fut, T>(
    hashes: S,
    fetch_item: F,
) -> CollectorStream<'a, T>
where
    S: Stream<Item = B256> + Send + 'a,
    F: Fn(B256) -> Fut + Clone + Send + Sync + 'a,
    Fut: Future<Output = Result<Option<T>>> + Send + 'a,
    T: Send + 'a,
{
    let items = hashes
        .map(move |hash| {
            let fetch_item = fetch_item.clone();
            async move { fetch_item(hash).await.ok().flatten() }
        })
        .buffer_unordered(PENDING_TRANSACTION_FETCH_CONCURRENCY)
        .filter_map(|item| async move { item });

    Box::pin(items)
}

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
/// This implementation uses Alloy provider pubsub support to subscribe to pending transaction hashes
/// and fetches each transaction body by hash. This preserves compatibility with endpoints that do not
/// support Geth's `eth_subscribe("newPendingTransactions", true)` full-payload extension.
#[async_trait]
impl<P> Collector<Transaction> for MempoolCollector<P>
where
    P: Provider + Send + Sync,
{
    async fn get_event_stream<'a>(&'a self) -> Result<CollectorStream<'a, Transaction>> {
        let hashes = self
            .provider
            .subscribe_pending_transactions()
            .await?
            .into_stream();
        let provider = Arc::clone(&self.provider);
        Ok(pending_hashes_to_fetched_items(hashes, move |hash| {
            let provider = Arc::clone(&provider);
            async move {
                provider
                    .get_transaction_by_hash(hash)
                    .await
                    .map_err(Into::into)
            }
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::B256;
    use tokio_stream::{iter, StreamExt};

    #[tokio::test]
    async fn pending_hash_subscription_fetches_full_transactions_and_filters_missing_hashes() {
        let missing_hash = B256::with_last_byte(2);
        let hashes = iter([
            B256::with_last_byte(1),
            missing_hash,
            B256::with_last_byte(3),
        ]);

        let mut transactions = pending_hashes_to_fetched_items(hashes, move |hash| async move {
            if hash == missing_hash {
                Ok(None)
            } else {
                Ok(Some(()))
            }
        });

        let mut fetched = 0;
        while transactions.next().await.is_some() {
            fetched += 1;
        }

        assert_eq!(fetched, 2);
    }
}
