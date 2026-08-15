//! External price oracle: polls Pyth's Hermes REST API for the configured
//! feed id per market and feeds the result into the match loop as the
//! `index_prices` state (see state.rs's `IntakeCommand::UpdateIndexPrice`
//! and `EngineState::index_prices` doc comments for why this is kept
//! distinct from the engine's own book-derived `mark_prices`).
//!
//! HTTP polling of Hermes rather than on-chain `pyth-sdk-solana` decoding:
//! this crate has zero Solana dependency today (order matching never talks
//! to the chain), and pulling in solana-client/solana-sdk just to decode a
//! price account would be a heavier addition than a plain HTTP client for a
//! feed this codebase only reads, never writes.

use crate::state::IntakeCommand;
use rust_decimal::Decimal;
use serde::Deserialize;
use std::time::Duration;
use store::models::MarketSymbol;
use tokio::sync::mpsc;

const POLL_INTERVAL: Duration = Duration::from_secs(3);
const HERMES_LATEST_PRICE_URL: &str = "https://hermes.pyth.network/v2/updates/price/latest";

#[derive(Debug, Deserialize)]
struct HermesResponse {
    parsed: Vec<HermesParsedPrice>,
}

#[derive(Debug, Deserialize)]
struct HermesParsedPrice {
    id: String,
    price: HermesPrice,
}

#[derive(Debug, Deserialize)]
struct HermesPrice {
    price: String,
    expo: i32,
}

/// Polls Hermes every `POLL_INTERVAL` for every `(market, feed_id)` pair and
/// sends `IntakeCommand::UpdateIndexPrice` for each fresh price. Never
/// crashes the task on an HTTP/parse error - a stalled oracle just leaves
/// `index_prices` stale (funding sampling skips markets with no price at
/// all, but tolerates a slightly-old one), it must not take the engine down.
pub async fn run_oracle_poller(tx: mpsc::Sender<IntakeCommand>, markets: Vec<(MarketSymbol, String)>) {
    if markets.is_empty() {
        tracing::warn!("oracle-poller: no markets have a pyth_price_feed_id configured, nothing to poll");
        return;
    }

    let client = reqwest::Client::new();
    let mut ticker = tokio::time::interval(POLL_INTERVAL);

    loop {
        ticker.tick().await;

        let mut request = client.get(HERMES_LATEST_PRICE_URL);
        for (_, feed_id) in &markets {
            request = request.query(&[("ids[]", feed_id.as_str())]);
        }

        let response = match request.send().await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("oracle-poller: Hermes request failed: {e}");
                continue;
            }
        };

        let parsed: HermesResponse = match response.json().await {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!("oracle-poller: Hermes response parse failed: {e}");
                continue;
            }
        };

        for entry in parsed.parsed {
            let Some((market, _)) = markets.iter().find(|(_, feed_id)| feed_id_matches(feed_id, &entry.id)) else {
                continue;
            };
            let Some(price) = hermes_price_to_decimal(&entry.price) else {
                tracing::warn!("oracle-poller: could not convert Hermes price for {market:?}: {:?}", entry.price);
                continue;
            };
            if tx.send(IntakeCommand::UpdateIndexPrice { market: *market, price }).await.is_err() {
                tracing::error!("oracle-poller: match-loop channel closed");
                return;
            }
        }
    }
}

/// Hermes ids come back without the "0x" prefix present in some published
/// feed lists - compare case-insensitively with any prefix stripped so
/// config either way matches.
fn feed_id_matches(configured: &str, returned: &str) -> bool {
    configured.trim_start_matches("0x").eq_ignore_ascii_case(returned.trim_start_matches("0x"))
}

/// Pyth prices are `price * 10^expo` with `price` an integer string and
/// `expo` typically negative (e.g. price="6012345000000", expo=-8 ->
/// 60123.45). `Decimal::new` takes a scale (digits after the decimal
/// point), which is exactly `-expo` for the expected negative-exponent case.
fn hermes_price_to_decimal(p: &HermesPrice) -> Option<Decimal> {
    let raw: i64 = p.price.parse().ok()?;
    if p.expo > 0 {
        // Positive exponent is unusual for Pyth crypto feeds but handle it
        // rather than silently producing a wrong price.
        return Decimal::from(raw).checked_mul(Decimal::from(10i64.checked_pow(p.expo as u32)?));
    }
    let scale = (-p.expo) as u32;
    Some(Decimal::new(raw, scale))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hermes_price_to_decimal_negative_expo() {
        let p = HermesPrice { price: "6012345000000".into(), expo: -8 };
        assert_eq!(hermes_price_to_decimal(&p), Some("60123.45000000".parse().unwrap()));
    }

    #[test]
    fn hermes_price_to_decimal_bad_input_returns_none() {
        let p = HermesPrice { price: "not-a-number".into(), expo: -8 };
        assert_eq!(hermes_price_to_decimal(&p), None);
    }

    #[test]
    fn feed_id_matches_ignores_0x_prefix_and_case() {
        assert!(feed_id_matches("0xABCDEF", "abcdef"));
        assert!(feed_id_matches("abcdef", "0xABCDEF"));
        assert!(!feed_id_matches("abcdef", "123456"));
    }
}
