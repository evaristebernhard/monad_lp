use std::{
    collections::{HashMap, HashSet},
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        Arc,
    },
};

use reqwest::Client;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tier {
    Free,
    Paid,
}

impl Tier {
    pub(crate) fn parse(value: &str) -> Self {
        if value.eq_ignore_ascii_case("paid") {
            Self::Paid
        } else {
            Self::Free
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Free => "free",
            Self::Paid => "paid",
        }
    }
}

#[derive(Debug, Serialize, Clone)]
pub(crate) struct BenchmarkConfig {
    pub(crate) sustainable_rps: Option<i64>,
    pub(crate) p95_ms: Option<f64>,
}

pub(crate) struct EndpointRuntime {
    pub(crate) title: String,
    pub(crate) provider: String,
    pub(crate) tier: Tier,
    pub(crate) priority: u32,
    pub(crate) max_concurrency: usize,
    pub(crate) url: String,
    pub(crate) supports_historical: Option<bool>,
    pub(crate) benchmark: Option<BenchmarkConfig>,
    pub(crate) in_flight: AtomicUsize,
    pub(crate) requests: AtomicU64,
    pub(crate) successes: AtomicU64,
    pub(crate) failures: AtomicU64,
    pub(crate) total_latency_ms: AtomicU64,
}

pub(crate) struct EndpointPermit {
    pub(crate) endpoint: Arc<EndpointRuntime>,
}

impl Drop for EndpointPermit {
    fn drop(&mut self) {
        self.endpoint.in_flight.fetch_sub(1, Ordering::Release);
    }
}

impl EndpointRuntime {
    pub(crate) fn try_acquire(self: &Arc<Self>) -> Option<EndpointPermit> {
        loop {
            let current = self.in_flight.load(Ordering::Acquire);
            if current >= self.max_concurrency {
                return None;
            }
            if self
                .in_flight
                .compare_exchange(current, current + 1, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return Some(EndpointPermit {
                    endpoint: Arc::clone(self),
                });
            }
        }
    }

    pub(crate) fn record(&self, ok: bool, elapsed_ms: u64) {
        self.requests.fetch_add(1, Ordering::Relaxed);
        self.total_latency_ms
            .fetch_add(elapsed_ms, Ordering::Relaxed);
        if ok {
            self.successes.fetch_add(1, Ordering::Relaxed);
        } else {
            self.failures.fetch_add(1, Ordering::Relaxed);
        }
    }
}

pub(crate) struct RouteRuntime {
    pub(crate) name: String,
    pub(crate) protocol: String,
    pub(crate) chain_id: String,
    pub(crate) aliases: Vec<String>,
    pub(crate) endpoints: Vec<Arc<EndpointRuntime>>,
}

impl RouteRuntime {
    pub(crate) fn select(
        &self,
        title: Option<&str>,
        tried: &HashSet<String>,
        historical_only: bool,
    ) -> Option<(EndpointPermit, Arc<EndpointRuntime>)> {
        let prefer_historical = historical_only
            && title.is_none()
            && self
                .endpoints
                .iter()
                .any(|endpoint| endpoint.supports_historical == Some(true));
        let mut candidates: Vec<_> = self
            .endpoints
            .iter()
            .filter(|endpoint| !tried.contains(&endpoint.title))
            .filter(|endpoint| !prefer_historical || endpoint.supports_historical == Some(true))
            .filter(|endpoint| {
                title.is_none_or(|wanted| endpoint.title.eq_ignore_ascii_case(wanted))
            })
            .cloned()
            .collect();
        candidates.sort_by_key(|endpoint| {
            (
                if endpoint.tier == Tier::Free { 0 } else { 1 },
                (endpoint.in_flight.load(Ordering::Relaxed) as u64 * 1000)
                    / endpoint.max_concurrency.max(1) as u64,
                endpoint.priority,
                endpoint.in_flight.load(Ordering::Relaxed),
            )
        });
        for endpoint in candidates {
            if let Some(permit) = endpoint.try_acquire() {
                return Some((permit, endpoint));
            }
        }
        None
    }
}

#[derive(Clone)]
pub(crate) struct AppState {
    pub(crate) routes: Arc<HashMap<String, Arc<RouteRuntime>>>,
    pub(crate) http: Client,
}

#[derive(Debug, Deserialize)]
pub(crate) struct RpcQuery {
    pub(crate) title: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct ServerConfig {
    pub(crate) listen: String,
    pub(crate) request_timeout_ms: u64,
    pub(crate) max_request_bytes: usize,
}
