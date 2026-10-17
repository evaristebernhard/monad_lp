//! Raw token price/history maintainer for the RPC workspace.
//!
//! This binary is intentionally separate from the JSON-RPC forwarder. It
//! stores raw token observations and pool-local price samples; the backend owns
//! averages, pair prices, and volatility calculations.

#[path = "../price/mod.rs"]
mod price;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    price::run().await
}
