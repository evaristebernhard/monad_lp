//! Shared data contracts for raw token-price acquisition.

use std::collections::HashMap;

use serde::Deserialize;

#[derive(Debug, Clone)]
pub(crate) struct TokenRef {
    pub(crate) chain: String,
    pub(crate) address: String,
}

#[derive(Debug, Clone)]
pub(crate) struct Edge {
    pub(crate) chain: String,
    pub(crate) left: String,
    pub(crate) right: String,
    pub(crate) right_per_left: f64,
}

#[derive(Debug, Clone)]
pub(crate) struct PoolRatio {
    pub(crate) chain: String,
    pub(crate) protocol: String,
    pub(crate) pool_address: String,
    pub(crate) token0_address: String,
    pub(crate) token1_address: String,
    /// Price(token1) / Price(token0), independent of the USD anchor.
    pub(crate) token1_per_token0: f64,
    pub(crate) source: &'static str,
}

#[derive(Debug, Clone)]
pub(crate) struct PoolGroup {
    pub(crate) chain: String,
    pub(crate) protocol: String,
    pub(crate) live_pools: usize,
}

#[derive(Debug, Deserialize)]
pub(crate) struct LlamaCoin {
    pub(crate) prices: Vec<LlamaPoint>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct LlamaPoint {
    pub(crate) timestamp: i64,
    pub(crate) price: f64,
}

#[derive(Debug, Deserialize)]
pub(crate) struct LlamaResponse {
    pub(crate) coins: HashMap<String, LlamaCoin>,
}

#[derive(Debug, Default)]
pub(crate) struct BatchResult {
    pub(crate) prices: HashMap<(String, String), Vec<(i64, f64)>>,
    pub(crate) errors: usize,
}
