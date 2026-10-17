//! Historical, read-only RPC entrypoint.
//!
//! Historical state is deliberately kept in the RPC layer as a transport
//! concern. The data layer can submit ordinary JSON-RPC reads with an old
//! block tag without knowing provider URLs or how free/paid fallback works.

use axum::{
    body::Bytes,
    extract::{Path as AxumPath, Query, State},
    response::Response,
};

use crate::{
    forward::{forward_impl, is_retryable_method, rpc_error},
    state::{AppState, RpcQuery},
};

/// Forward a historical JSON-RPC read through the history route.
///
/// The body remains standard JSON-RPC, so this supports a single request or a
/// batch. Typical callers submit `eth_call` with a block tag, `eth_getLogs`,
/// `eth_getBlockByNumber`, or `eth_getBlockByHash`.
pub(crate) async fn forward(
    State(state): State<AppState>,
    AxumPath(chain): AxumPath<String>,
    Query(query): Query<RpcQuery>,
    body: Bytes,
) -> Response {
    if !is_retryable_method(&body) {
        return rpc_error(
            axum::http::StatusCode::METHOD_NOT_ALLOWED,
            "historical RPC only accepts read-only JSON-RPC methods",
        );
    }
    forward_impl(state, chain, query, body, true).await
}
