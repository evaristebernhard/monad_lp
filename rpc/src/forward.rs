//! Raw JSON-RPC forwarding.
//!
//! This module deliberately stops at transport. It chooses an upstream
//! endpoint, forwards the request, handles retryable provider failures, and
//! returns raw chain data. Price aggregation and volatility remain backend
//! responsibilities.

use std::collections::HashSet;
use std::time::Instant;

use axum::{
    body::Bytes,
    extract::{Path as AxumPath, Query, State},
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde_json::{json, Value};

use crate::state::{AppState, RpcQuery};

pub(crate) async fn forward(
    State(state): State<AppState>,
    AxumPath(chain): AxumPath<String>,
    Query(query): Query<RpcQuery>,
    body: Bytes,
) -> Response {
    forward_impl(state, chain, query, body, false).await
}

pub(crate) fn rpc_error(status: StatusCode, message: impl Into<String>) -> Response {
    (status, Json(json!({"error": message.into()}))).into_response()
}

pub(crate) fn is_retryable_method(body: &[u8]) -> bool {
    let Ok(value) = serde_json::from_slice::<Value>(body) else {
        return false;
    };
    let methods = match value {
        Value::Array(values) => values,
        value => vec![value],
    };
    methods.iter().all(|request| {
        let Some(method) = request.get("method").and_then(Value::as_str) else {
            return false;
        };
        let method = method.to_ascii_lowercase();
        // Retry/failover is limited to read-only calls. Match write methods
        // explicitly: a read method such as Solana's
        // `getSignaturesForAddress` legitimately contains the word
        // "signatures" and must remain retryable.
        !method.starts_with("eth_send")
            && !method.starts_with("eth_sign")
            && !method.starts_with("solana_sign")
            && !method.starts_with("sign")
            && !method.starts_with("personal_")
    })
}

#[cfg(test)]
mod tests {
    use super::is_retryable_method;

    #[test]
    fn solana_signature_history_is_read_only() {
        assert!(is_retryable_method(
            br#"{"jsonrpc":"2.0","id":1,"method":"getSignaturesForAddress","params":[]}"#
        ));
    }

    #[test]
    fn signing_and_submission_methods_are_not_retryable() {
        for method in [
            "eth_sendTransaction",
            "eth_sendRawTransaction",
            "eth_sign",
            "solana_signTransaction",
            "signTransaction",
            "personal_sign",
        ] {
            let body = format!(r#"{{"jsonrpc":"2.0","id":1,"method":"{method}","params":[]}}"#);
            assert!(!is_retryable_method(body.as_bytes()), "{method}");
        }
    }

    #[test]
    fn batches_require_every_method_to_be_read_only() {
        assert!(is_retryable_method(
            br#"[{"jsonrpc":"2.0","id":1,"method":"getSlot"},{"jsonrpc":"2.0","id":2,"method":"getBlockTime"}]"#
        ));
        assert!(!is_retryable_method(
            br#"[{"jsonrpc":"2.0","id":1,"method":"getSlot"},{"jsonrpc":"2.0","id":2,"method":"eth_sendRawTransaction"}]"#
        ));
    }
}

fn is_retryable_upstream_body(body: &[u8]) -> bool {
    let Ok(value) = serde_json::from_slice::<Value>(body) else {
        return false;
    };
    let responses = match value {
        Value::Array(values) => values,
        value => vec![value],
    };
    responses.iter().any(|response| {
        let Some(error) = response.get("error") else {
            return false;
        };
        let text = error.to_string().to_ascii_lowercase();
        text.contains("rate limit")
            || text.contains("over rate")
            || text.contains("too many requests")
            || text.contains("compute units per second")
            || text.contains("capacity")
            || text.contains("exceeded")
            || text.contains("payload too large")
            || text.contains("request too large")
            || text.contains("temporarily unavailable")
            || text.contains("try again")
            || text.contains("-32016")
    })
}

fn upstream_headers(response: &reqwest::Response) -> Option<HeaderValue> {
    response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| HeaderValue::from_bytes(value.as_bytes()).ok())
}

pub(crate) async fn forward_impl(
    state: AppState,
    chain: String,
    query: RpcQuery,
    body: Bytes,
    historical_only: bool,
) -> Response {
    let key = chain.to_ascii_lowercase();
    let Some(route) = state.routes.get(&key).cloned() else {
        return rpc_error(StatusCode::NOT_FOUND, format!("unknown RPC route: {chain}"));
    };
    if body.is_empty() {
        return rpc_error(StatusCode::BAD_REQUEST, "empty JSON-RPC request");
    }
    if route.endpoints.is_empty() {
        return rpc_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "no enabled RPC endpoints configured for this chain",
        );
    }
    let retryable = is_retryable_method(&body);
    let mut tried = HashSet::new();
    let max_attempts = if retryable { route.endpoints.len() } else { 1 };
    let mut last_error = "no endpoint available".to_string();

    for _ in 0..max_attempts {
        let Some((permit, endpoint)) =
            route.select(query.title.as_deref(), &tried, historical_only)
        else {
            break;
        };
        tried.insert(endpoint.title.clone());
        let started = Instant::now();
        let result = state
            .http
            .post(&endpoint.url)
            .header(header::CONTENT_TYPE, "application/json")
            .header("x-local-rpc-route", route.name.as_str())
            .body(body.clone())
            .send()
            .await;
        let elapsed_ms = started.elapsed().as_millis() as u64;
        match result {
            Ok(response) => {
                let status = response.status();
                let content_type = upstream_headers(&response);
                match response.bytes().await {
                    Ok(bytes) => {
                        let ok = status.is_success();
                        let retryable_body = ok && retryable && is_retryable_upstream_body(&bytes);
                        endpoint.record(ok && !retryable_body, elapsed_ms);
                        drop(permit);
                        if retryable_body {
                            last_error =
                                format!("{} returned a retryable JSON-RPC error", endpoint.title);
                            continue;
                        }
                        if ok
                            || !retryable
                            || !(status == StatusCode::TOO_MANY_REQUESTS
                                || status == StatusCode::PAYLOAD_TOO_LARGE
                                || status.is_server_error())
                        {
                            let mut output = Response::new(bytes.into());
                            *output.status_mut() = status;
                            if let Some(content_type) = content_type {
                                output
                                    .headers_mut()
                                    .insert(header::CONTENT_TYPE, content_type);
                            }
                            return output;
                        }
                        last_error = format!("{} returned {}", endpoint.title, status);
                    }
                    Err(error) => {
                        endpoint.record(false, elapsed_ms);
                        drop(permit);
                        last_error = format!("{} response read failed: {error}", endpoint.title);
                    }
                }
            }
            Err(error) => {
                endpoint.record(false, elapsed_ms);
                drop(permit);
                last_error = format!("{} request failed: {error}", endpoint.title);
            }
        }
    }
    if last_error == "no endpoint available" {
        rpc_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "all configured RPC endpoints are saturated",
        )
    } else {
        rpc_error(StatusCode::BAD_GATEWAY, last_error)
    }
}
