use alloy::primitives::Address;

use mev_share::{rpc::SendBundleRequest, sse};

/// Core Event enum for the current strategy.
#[derive(Debug, Clone)]
pub enum Event {
    MEVShareEvent(sse::Event),
}

/// Core Action enum for the current strategy.
#[derive(Debug, Clone)]
pub enum Action {
    SubmitBundle(SendBundleRequest),
}

#[derive(Debug, serde::Deserialize)]
pub struct PoolRecord {
    pub token_address: Address,
    pub uni_pool_address: Address,
    pub sushi_pool_address: Address,
}

#[derive(Debug, serde::Deserialize)]
pub struct V2V3PoolRecord {
    pub token_address: Address,
    pub v3_pool: Address,
    pub v2_pool: Address,
    pub weth_token0: bool,
}
