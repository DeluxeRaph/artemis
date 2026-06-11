use std::{
    ops::{Div, Mul},
    sync::Arc,
};

use crate::types::Executor;
use alloy::{primitives::U256, providers::Provider, rpc::types::TransactionRequest};
use anyhow::{Context, Result};
use async_trait::async_trait;

/// An executor that sends transactions to the mempool.
pub struct MempoolExecutor<P> {
    client: Arc<P>,
}

/// Information about the gas bid for a transaction.
#[derive(Debug, Clone)]
pub struct GasBidInfo {
    /// Total profit expected from opportunity
    pub total_profit: U256,

    /// Percentage of bid profit to use for gas
    pub bid_percentage: u64,
}

#[derive(Debug, Clone)]
pub struct SubmitTxToMempool {
    pub tx: TransactionRequest,
    pub gas_bid_info: Option<GasBidInfo>,
}

impl<P: Provider> MempoolExecutor<P> {
    pub fn new(client: Arc<P>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl<P> Executor<SubmitTxToMempool> for MempoolExecutor<P>
where
    P: Provider + Send + Sync,
{
    /// Send a transaction to the mempool.
    async fn execute(&self, mut action: SubmitTxToMempool) -> Result<()> {
        let gas_usage = self
            .client
            .estimate_gas(action.tx.clone())
            .await
            .context("Error estimating gas usage: {}")?;

        let bid_gas_price;
        if let Some(gas_bid_info) = action.gas_bid_info {
            // gas price at which we'd break even, meaning 100% of profit goes to validator
            let breakeven_gas_price = gas_bid_info.total_profit / U256::from(gas_usage);
            // gas price corresponding to bid percentage
            bid_gas_price = u256_to_u128(
                breakeven_gas_price
                    .mul(U256::from(gas_bid_info.bid_percentage))
                    .div(U256::from(100)),
            )?;
        } else {
            bid_gas_price = self
                .client
                .get_gas_price()
                .await
                .context("Error getting gas price: {}")?;
        }
        action.tx = action.tx.gas_price(bid_gas_price);
        let _ = self.client.send_transaction(action.tx).await?;
        Ok(())
    }
}

fn u256_to_u128(value: U256) -> Result<u128> {
    value
        .try_into()
        .context("gas bid price does not fit into u128")
}
