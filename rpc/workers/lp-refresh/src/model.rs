//! Data contracts shared by pool selection, RPC transport, and decoders.
//!
//! Keeping these structs out of `main.rs` makes the worker's boundaries
//! visible: transport code deals in `RpcRequest`/`RpcResponse`, decoders emit
//! `StateRow`, and orchestration only coordinates batches.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone)]
pub(crate) struct Pool {
    pub(crate) address: String,
    pub(crate) chain: String,
    pub(crate) protocol: String,
    pub(crate) pool_type: String,
    pub(crate) token_a_symbol: String,
    pub(crate) token_b_symbol: String,
    pub(crate) token_a_address: Option<String>,
    pub(crate) token_b_address: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct Endpoint {
    pub(crate) network: String,
    pub(crate) protocol: String,
    pub(crate) url: String,
}

#[derive(Debug, Serialize, Clone)]
pub(crate) struct RpcRequest<'a> {
    pub(crate) jsonrpc: &'static str,
    pub(crate) id: u64,
    pub(crate) method: &'a str,
    pub(crate) params: Value,
}

#[derive(Debug, Deserialize)]
pub(crate) struct RpcResponse {
    pub(crate) id: u64,
    pub(crate) result: Option<Value>,
    pub(crate) error: Option<Value>,
}

#[derive(Debug)]
pub(crate) struct StateRow {
    pub(crate) pool_address: String,
    pub(crate) chain: String,
    pub(crate) network: String,
    pub(crate) protocol: String,
    pub(crate) checked_at: i64,
    pub(crate) state_block: Option<i64>,
    pub(crate) account_exists: Option<bool>,
    pub(crate) state_status: String,
    pub(crate) pool_family: String,
    pub(crate) decoder_version: String,
    pub(crate) state_error: Option<String>,
    pub(crate) state_json: Option<String>,
    pub(crate) assets_json: Option<String>,
    pub(crate) quote_supported: bool,
    pub(crate) token0_address: Option<String>,
    pub(crate) token1_address: Option<String>,
    pub(crate) fee_bps: Option<f64>,
    pub(crate) reserve0_raw: Option<String>,
    pub(crate) reserve1_raw: Option<String>,
    pub(crate) sqrt_price_x96: Option<String>,
    pub(crate) active_liquidity_raw: Option<String>,
    pub(crate) spot_price_usd: Option<f64>,
    pub(crate) account_owner: Option<String>,
    pub(crate) account_data_bytes: Option<i64>,
}

#[derive(Debug, Serialize, Clone)]
pub(crate) struct RefreshReport {
    pub(crate) status: String,
    pub(crate) cycle_started_at: i64,
    pub(crate) cycle_finished_at: i64,
    pub(crate) duration_ms: u64,
    pub(crate) selected: usize,
    pub(crate) states_written: usize,
    pub(crate) batches: usize,
    pub(crate) live_or_present: usize,
    pub(crate) account_missing: usize,
    pub(crate) rpc_error: usize,
    pub(crate) transport_error: usize,
    pub(crate) unsupported_protocol: usize,
    pub(crate) pools_per_second: f64,
    pub(crate) batch_size: usize,
    pub(crate) concurrency: usize,
    pub(crate) interval_secs: u64,
    pub(crate) error: Option<String>,
}

#[derive(Debug)]
pub(crate) struct SolanaDecodedState {
    pub(crate) decoder_version: &'static str,
    pub(crate) fee_bps: Option<f64>,
    pub(crate) sqrt_price: Option<String>,
    pub(crate) active_liquidity: Option<String>,
    pub(crate) state_json: Value,
}
