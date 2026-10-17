//! Local RPC transport service.
//!
//! The binary only wires the independent modules together:
//! - catalog: SQLite catalog and provider inspection;
//! - forward: raw JSON-RPC routing and retry;
//! - history: historical read-only transport;
//! - state: shared runtime state and endpoint selection.

mod catalog;
mod forward;
mod history;
mod monad_listener;
mod monad_market_live;
mod monad_store;
mod solana_listener;
mod state;

#[path = "price/quote.rs"]
mod price_quote;

use anyhow::{Context, Result};
use axum::{
    extract::DefaultBodyLimit,
    routing::{get, post},
    Router,
};
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> Result<()> {
    let path = catalog::db_path();
    let (state, server) = catalog::load_state(&path)?;
    if std::env::args().any(|arg| arg == "--backfill-monad-hours") {
        return monad_listener::backfill_recent_hours(&state).await;
    }
    let route_count = state
        .routes
        .values()
        .map(|route| route.name.as_str())
        .collect::<std::collections::HashSet<_>>()
        .len();
    println!(
        "local RPC listening on {}; db={}; routes={route_count}",
        server.listen,
        path.display()
    );

    solana_listener::spawn(state.clone())?;
    monad_listener::spawn(state.clone())?;
    monad_market_live::spawn(state.clone());

    let app = Router::new()
        .route("/monad/events", get(monad_listener::events))
        .route("/monad/whale-alerts", get(monad_listener::alerts))
        .route("/monad/pool-snapshots", get(monad_listener::snapshots))
        .route("/monad/pool-history", get(monad_listener::history))
        .route("/monad/protocols", get(monad_listener::protocols))
        .route("/monad/listener/status", get(monad_listener::status))
        .route("/healthz", get(catalog::health))
        .route("/routes", get(catalog::routes))
        .route("/rpc/{chain}/providers", get(catalog::chain_info))
        .route(
            "/rpc/{chain}",
            get(catalog::chain_info).post(forward::forward),
        )
        .route("/history/{chain}/providers", get(catalog::chain_info))
        .route("/history/{chain}", post(history::forward))
        .route("/price/{chain}/{pool}", post(price_quote::quote))
        .route("/solana/events", get(solana_listener::events))
        .route("/solana/whale-alerts", get(solana_listener::whale_alerts))
        .route(
            "/solana/pool-snapshots",
            get(solana_listener::pool_snapshots),
        )
        .route("/solana/pool-history", get(solana_listener::pool_history))
        .route("/solana/protocols", get(solana_listener::protocols))
        .route("/solana/listener/start", post(solana_listener::start))
        .route("/solana/listener/status", get(solana_listener::status))
        .layer(DefaultBodyLimit::max(server.max_request_bytes))
        .with_state(state);
    let listener = TcpListener::bind(&server.listen)
        .await
        .with_context(|| format!("bind {}", server.listen))?;
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
        .context("serve local RPC")?;
    Ok(())
}
