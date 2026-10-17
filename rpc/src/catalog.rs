//! RPC catalog loading and read-only catalog endpoints.

use std::{
    collections::HashMap,
    env, fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{anyhow, Context, Result};
use axum::{
    extract::{Path as AxumPath, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;
use serde_json::json;

use crate::{
    forward::rpc_error,
    state::{AppState, BenchmarkConfig, EndpointRuntime, RouteRuntime, ServerConfig, Tier},
};

const SCHEMA: &str = include_str!("../db/schema.sql");

#[derive(Serialize)]
struct RouteSummary {
    route: String,
    protocol: String,
    chain_id: String,
    aliases: Vec<String>,
    endpoints: Vec<EndpointSummary>,
}

#[derive(Serialize)]
struct EndpointSummary {
    selection_rank: usize,
    title: String,
    provider: String,
    endpoint: String,
    tier: &'static str,
    priority: u32,
    max_concurrency: usize,
    in_flight: usize,
    requests: u64,
    successes: u64,
    failures: u64,
    average_latency_ms: Option<f64>,
    benchmark: Option<BenchmarkConfig>,
    supports_historical: Option<bool>,
}

pub(crate) fn db_path() -> PathBuf {
    env::var_os("RPC_DB_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("rpc/data/rpc.db"))
}

fn setting(connection: &Connection, key: &str, default: &str) -> Result<String> {
    Ok(connection
        .query_row("SELECT value FROM settings WHERE key=?1", [key], |row| {
            row.get(0)
        })
        .optional()?
        .unwrap_or_else(|| default.to_string()))
}

pub(crate) fn load_state(path: &Path) -> Result<(AppState, ServerConfig)> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create RPC DB directory {}", parent.display()))?;
    }
    let connection =
        Connection::open(path).with_context(|| format!("open RPC database {}", path.display()))?;
    connection
        .execute_batch(SCHEMA)
        .context("initialize RPC database schema")?;
    match connection.execute(
        "ALTER TABLE rpc_endpoints ADD COLUMN supports_historical INTEGER",
        [],
    ) {
        Ok(_) => {}
        Err(error)
            if error
                .to_string()
                .to_ascii_lowercase()
                .contains("duplicate column name") => {}
        Err(error) => return Err(error).context("migrate RPC endpoint capabilities"),
    }

    let server = ServerConfig {
        listen: setting(&connection, "listen", "127.0.0.1:8545")?,
        request_timeout_ms: setting(&connection, "request_timeout_ms", "8000")?
            .parse()
            .context("parse settings.request_timeout_ms")?,
        max_request_bytes: setting(&connection, "max_request_bytes", "10485760")?
            .parse()
            .context("parse settings.max_request_bytes")?,
    };

    let mut chain_statement = connection.prepare(
        "SELECT id, slug, network_slug, protocol, chain_id FROM chains WHERE enabled=1 ORDER BY slug",
    )?;
    let chains = chain_statement
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if chains.is_empty() {
        return Err(anyhow!(
            "RPC database {} has no enabled chains",
            path.display()
        ));
    }

    let mut routes = HashMap::new();
    let mut aliases = HashMap::new();
    for (chain_id, slug, network_slug, protocol, chain_id_value) in chains {
        let mut alias_statement = connection
            .prepare("SELECT alias FROM chain_aliases WHERE chain_id=?1 ORDER BY alias")?;
        let route_aliases = alias_statement
            .query_map([chain_id], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let mut endpoint_statement = connection.prepare(
            "SELECT e.title, e.provider, e.tier, e.priority, e.max_concurrency, e.url,
                    e.supports_historical, b.target_rps, b.p95_ms
             FROM rpc_endpoints e
             LEFT JOIN rpc_benchmarks b ON b.endpoint_id=e.id
                AND b.id=(SELECT latest.id FROM rpc_benchmarks latest
                          WHERE latest.endpoint_id=e.id AND latest.sustainable=1
                          ORDER BY latest.target_rps DESC,
                                   latest.measured_at DESC, latest.id DESC LIMIT 1)
             WHERE e.chain_id=?1 AND e.enabled=1
             ORDER BY CASE e.tier WHEN 'free' THEN 0 ELSE 1 END, e.priority, e.title",
        )?;
        let endpoint_rows = endpoint_statement
            .query_map([chain_id], |row| {
                let supports_historical = row.get::<_, Option<i64>>(6)?.map(|value| value != 0);
                let sustainable_rps = row.get::<_, Option<i64>>(7)?;
                let p95_ms = row.get::<_, Option<f64>>(8)?;
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, String>(5)?,
                    supports_historical,
                    (sustainable_rps.is_some() || p95_ms.is_some()).then_some(BenchmarkConfig {
                        sustainable_rps,
                        p95_ms,
                    }),
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let endpoints = endpoint_rows
            .into_iter()
            .map(
                |(
                    title,
                    provider,
                    tier,
                    priority,
                    max_concurrency,
                    url,
                    supports_historical,
                    benchmark,
                )| {
                    if max_concurrency <= 0 || url.trim().is_empty() {
                        return Err(anyhow!("invalid RPC endpoint {slug}/{title}"));
                    }
                    Ok(Arc::new(EndpointRuntime {
                        title,
                        provider,
                        tier: Tier::parse(&tier),
                        priority: u32::try_from(priority).context("RPC priority out of range")?,
                        max_concurrency: usize::try_from(max_concurrency)
                            .context("RPC max_concurrency out of range")?,
                        url,
                        supports_historical,
                        benchmark,
                        in_flight: Default::default(),
                        requests: Default::default(),
                        successes: Default::default(),
                        failures: Default::default(),
                        total_latency_ms: Default::default(),
                    }))
                },
            )
            .collect::<Result<Vec<_>>>()?;

        let route = Arc::new(RouteRuntime {
            name: slug.clone(),
            protocol,
            chain_id: chain_id_value,
            aliases: route_aliases.clone(),
            endpoints,
        });
        for alias in route_aliases {
            aliases.insert(alias.to_ascii_lowercase(), slug.clone());
        }
        aliases.insert(network_slug.to_ascii_lowercase(), slug.clone());
        routes.insert(slug, route);
    }
    for (alias, canonical) in aliases {
        if let Some(route) = routes.get(&canonical).cloned() {
            routes.insert(alias, route);
        }
    }

    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_millis(server.request_timeout_ms))
        .pool_max_idle_per_host(64)
        .build()
        .context("build RPC HTTP client")?;
    Ok((
        AppState {
            routes: Arc::new(routes),
            http,
        },
        server,
    ))
}

fn masked_url(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return "[redacted]".into();
    };
    let (authority, suffix) = rest.split_once('/').unwrap_or((rest, ""));
    if suffix.is_empty() {
        return url.to_string();
    }
    format!("{scheme}://{authority}/[redacted]")
}

fn route_summary(route: &RouteRuntime) -> RouteSummary {
    let mut endpoints = route.endpoints.clone();
    endpoints.sort_by_key(|endpoint| {
        (
            if endpoint.tier == Tier::Free { 0 } else { 1 },
            endpoint.priority,
            endpoint.title.clone(),
        )
    });
    RouteSummary {
        route: route.name.clone(),
        protocol: route.protocol.clone(),
        chain_id: route.chain_id.clone(),
        aliases: route.aliases.clone(),
        endpoints: endpoints
            .iter()
            .enumerate()
            .map(|(index, endpoint)| {
                let requests = endpoint.requests.load(std::sync::atomic::Ordering::Relaxed);
                let total_latency_ms = endpoint
                    .total_latency_ms
                    .load(std::sync::atomic::Ordering::Relaxed);
                EndpointSummary {
                    selection_rank: index + 1,
                    title: endpoint.title.clone(),
                    provider: endpoint.provider.clone(),
                    endpoint: masked_url(&endpoint.url),
                    tier: endpoint.tier.as_str(),
                    priority: endpoint.priority,
                    max_concurrency: endpoint.max_concurrency,
                    in_flight: endpoint
                        .in_flight
                        .load(std::sync::atomic::Ordering::Relaxed),
                    requests,
                    successes: endpoint
                        .successes
                        .load(std::sync::atomic::Ordering::Relaxed),
                    failures: endpoint.failures.load(std::sync::atomic::Ordering::Relaxed),
                    average_latency_ms: (requests > 0)
                        .then(|| total_latency_ms as f64 / requests as f64),
                    benchmark: endpoint.benchmark.clone(),
                    supports_historical: endpoint.supports_historical,
                }
            })
            .collect(),
    }
}

pub(crate) async fn health() -> impl IntoResponse {
    Json(json!({"status": "ok"}))
}

pub(crate) async fn routes(State(state): State<AppState>) -> impl IntoResponse {
    let mut unique = HashMap::new();
    for route in state.routes.values() {
        unique
            .entry(route.name.clone())
            .or_insert_with(|| route.clone());
    }
    let mut summaries = unique
        .values()
        .map(|route| route_summary(route))
        .collect::<Vec<_>>();
    summaries.sort_by(|left, right| left.route.cmp(&right.route));
    Json(summaries)
}

pub(crate) async fn chain_info(
    State(state): State<AppState>,
    AxumPath(chain): AxumPath<String>,
) -> Response {
    let key = chain.to_ascii_lowercase();
    let Some(route) = state.routes.get(&key) else {
        return rpc_error(StatusCode::NOT_FOUND, format!("unknown RPC route: {chain}"));
    };
    Json(route_summary(route)).into_response()
}
