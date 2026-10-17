//! DefiLlama historical source.
//!
//! This module returns raw observations only. It does not write SQLite and it
//! does not calculate volatility or pair metrics.

use std::collections::HashMap;

use anyhow::{Context, Result};
use reqwest::Client;

use super::{llama_chain, normalize_address, BatchResult, TokenRef, LLAMA_URL};

async fn fetch_batch_once(
    client: &Client,
    batch: &[TokenRef],
    span_days: u64,
) -> Result<BatchResult> {
    let chain = batch
        .first()
        .map(|item| item.chain.clone())
        .unwrap_or_default();
    let identifiers = batch
        .iter()
        .map(|item| format!("{}:{}", llama_chain(&item.chain), item.address))
        .collect::<Vec<_>>()
        .join(",");
    let url = format!("{}{identifiers}?span={span_days}d", LLAMA_URL);
    let payload = match client
        .get(url)
        .header("accept", "application/json")
        .header("user-agent", "local-rpc-price/0.1")
        .send()
        .await
    {
        Ok(response) if response.status().is_success() => response
            .json::<super::LlamaResponse>()
            .await
            .context("decode DefiLlama response"),
        Ok(response) => Err(response.error_for_status().unwrap_err().into()),
        Err(error) => Err(error.into()),
    }?;
    let mut prices = HashMap::new();
    for (key, coin) in payload.coins {
        let Some((_, address)) = key.split_once(':') else {
            continue;
        };
        let points = coin
            .prices
            .into_iter()
            .filter(|point| point.price.is_finite() && point.price > 0.0)
            .map(|point| (point.timestamp, point.price))
            .collect::<Vec<_>>();
        if !points.is_empty() {
            prices.insert((chain.clone(), normalize_address(address)), points);
        }
    }
    Ok(BatchResult { prices, errors: 0 })
}

#[async_recursion::async_recursion]
pub(crate) async fn fetch_batch(
    client: Client,
    batch: Vec<TokenRef>,
    span_days: u64,
) -> BatchResult {
    match fetch_batch_once(&client, &batch, span_days).await {
        Ok(result) => result,
        Err(_) if batch.len() > 1 => {
            let midpoint = batch.len() / 2;
            let left = fetch_batch(client.clone(), batch[..midpoint].to_vec(), span_days).await;
            let right = fetch_batch(client, batch[midpoint..].to_vec(), span_days).await;
            let mut prices = left.prices;
            prices.extend(right.prices);
            BatchResult {
                prices,
                errors: left.errors + right.errors,
            }
        }
        Err(_) => BatchResult {
            errors: 1,
            ..Default::default()
        },
    }
}
