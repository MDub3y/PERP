use crate::book::OrderBook;
use crate::events::publish_event;
use crate::matcher::{self, MatchFill, MatchOutcome};
use crate::publish::{publish_book_top, publish_depth, publish_ticker, publish_trade, publish_user_order_event};
use chrono::Utc;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::collections::{HashMap, HashSet};
use std::time::Duration;
use store::ledger::FillEvent;
use store::models::{FeeTier, Market, MarketSymbol, MarginMode, Order, OrderVariant};
use tokio::sync::mpsc;

/// How often the match-loop task dumps its full state to Postgres. On
/// crash, at most this much engine activity is lost - accepted explicitly
/// as the tradeoff for keeping the hot path entirely in-memory/Redis-speed
/// (no replay-from-event-log on recovery; see bootstrap()).
const SNAPSHOT_INTERVAL: Duration = Duration::from_secs(10);

/// Number of price levels per side published on the `market:{SYM}:depth`
/// channel - deep enough for a UI depth ladder without dumping the whole
/// book on every mutation.
const DEPTH_LEVELS: usize = 10;

pub enum IntakeCommand {
    NewOrder(Order),
    CancelOrder { order_id: i64, market: MarketSymbol },
    /// Sent once per UTC day by fee_scheduler after it has already updated
    /// Postgres via store::ledger::refresh_fee_tiers - the match loop just
    /// swaps in the fresh user_id -> tier map, no DB access on this task.
    ReloadFeeTiers(HashMap<i32, i16>),
    /// Sent every 5s by funding_scheduler. Book state only exists inside
    /// the match loop, so the impact-price computation has to happen here;
    /// the scheduler's only job is ticking on time.
    SampleFunding,
    /// Sent once per UTC hour by funding_scheduler.
    SettleFunding,
    /// Sent periodically by liquidation_scheduler. Margin health is
    /// evaluated at this cadence rather than synchronously on every fill/
    /// mark-price tick (a documented simplification - "continuous" per the
    /// spec becomes "polled every few seconds" here); see check_liquidations.
    CheckLiquidations,
    /// Sent by oracle::run_oracle_poller whenever a fresh Pyth Hermes price
    /// lands for a market. Purely in-memory - unlike mark_price this is
    /// never persisted, since it's a live external feed, not a fact this
    /// engine established itself.
    UpdateIndexPrice { market: MarketSymbol, price: Decimal },
}

/// Owns every market's order book plus a lightweight net-position-quantity
/// cache (used only for reduce-only clamping in this phase - full
/// in-memory equity/margin tracking for liquidation lands in a later
/// phase). Single-owner by construction: this struct only ever exists
/// inside run_match_loop's task, and every other task talks to it via the
/// IntakeCommand mpsc channel - no shared mutable state, no locks around
/// the book.
#[derive(Serialize, Deserialize)]
struct EngineState {
    books: HashMap<MarketSymbol, OrderBook>,
    // (i32, MarketSymbol) tuple keys can't serialize as JSON object keys
    // (serde_json requires string/int/unit-variant keys) - see position_map.
    #[serde(with = "position_map")]
    positions: HashMap<(i32, MarketSymbol), Decimal>,
    fee_tiers: HashMap<i32, i16>,
    fee_rates: HashMap<i16, FeeTier>,
    markets: HashMap<MarketSymbol, Market>,
    /// Mark price = engine's own last-trade price (no external oracle
    /// exists in this codebase). A stub for a future oracle integration
    /// (Pyth is the natural fit, already Solana-native) - see README.
    mark_prices: HashMap<MarketSymbol, Decimal>,
    /// External index price (Pyth Hermes), fed by oracle::run_oracle_poller
    /// via IntakeCommand::UpdateIndexPrice. Ephemeral - never persisted or
    /// included in snapshots, since it's re-fetched from Pyth on every
    /// restart within seconds. Distinct from `mark_prices` (this engine's
    /// own book-derived last-trade price): funding's premium calc wants
    /// Index = oracle, Mark = book, which this split now makes possible.
    #[serde(skip)]
    index_prices: HashMap<MarketSymbol, Decimal>,
    /// Scopes currently blocked from new order intake: `(user_id, None)` is
    /// an account-wide cross flag, `(user_id, Some(market))` is a
    /// single-market isolated flag. Checked synchronously on every
    /// NewOrder (cheap in-memory lookup) so a flagged account can't place
    /// new orders between liquidation-check ticks.
    liquidating_scopes: HashSet<(i32, Option<MarketSymbol>)>,
    next_event_id: i64,
    next_trade_id: i64,
}

mod position_map {
    use super::{Decimal, HashMap, MarketSymbol};
    use serde::{Deserialize, Deserializer, Serializer, ser::SerializeSeq};

    pub fn serialize<S: Serializer>(
        map: &HashMap<(i32, MarketSymbol), Decimal>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(Some(map.len()))?;
        for (&(user_id, market), qty) in map {
            seq.serialize_element(&(user_id, market, *qty))?;
        }
        seq.end()
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<HashMap<(i32, MarketSymbol), Decimal>, D::Error> {
        let entries = Vec::<(i32, MarketSymbol, Decimal)>::deserialize(deserializer)?;
        Ok(entries.into_iter().map(|(user_id, market, qty)| ((user_id, market), qty)).collect())
    }
}

impl EngineState {
    fn apply_position_delta(&mut self, user_id: i32, market: MarketSymbol, delta: Decimal) {
        let entry = self.positions.entry((user_id, market)).or_insert(Decimal::ZERO);
        *entry += delta;
    }

    /// Tiered maker/taker rate for a user, defaulting to tier 0 for anyone
    /// the engine hasn't loaded a tier for yet (brand new signups).
    fn fee_rate(&self, user_id: i32, is_maker: bool) -> Decimal {
        let tier = self.fee_tiers.get(&user_id).copied().unwrap_or(0);
        match self.fee_rates.get(&tier) {
            Some(t) if is_maker => t.maker_rate,
            Some(t) => t.taker_rate,
            None => Decimal::ZERO,
        }
    }

    fn liquidation_fee_rate(&self, market: MarketSymbol) -> Decimal {
        self.markets.get(&market).map(|m| m.liquidation_fee_rate).unwrap_or(Decimal::ZERO)
    }
}

/// Tries the latest snapshot first (self-contained - books, positions, fee
/// caches, and the event/trade id counters all travel together, so no
/// replay-from-event-log step is needed on top of it). Falls back to a
/// from-scratch Postgres bootstrap only on a genuinely first-ever boot, or
/// if the snapshot fails to deserialize.
async fn bootstrap(pool: &PgPool) -> EngineState {
    match store::ledger::load_latest_snapshot(pool).await {
        Ok(Some((bytes, last_event_id))) => match serde_json::from_slice::<EngineState>(&bytes) {
            Ok(mut state) => {
                for book in state.books.values_mut() {
                    book.rebuild_index();
                }
                tracing::info!(
                    "restored from snapshot: last_event_id={last_event_id}, next_event_id={}",
                    state.next_event_id
                );
                return state;
            }
            Err(e) => {
                tracing::error!("snapshot deserialize failed ({e}), falling back to Postgres bootstrap");
            }
        },
        Ok(None) => {
            tracing::info!("no snapshot found, bootstrapping from Postgres");
        }
        Err(e) => {
            tracing::error!("snapshot load failed ({e}), falling back to Postgres bootstrap");
        }
    }

    bootstrap_from_postgres(pool).await
}

async fn bootstrap_from_postgres(pool: &PgPool) -> EngineState {
    let mut books = HashMap::new();
    for market in [MarketSymbol::Sol, MarketSymbol::Eth, MarketSymbol::Btc] {
        books.insert(market, OrderBook::new());
    }

    let mut positions = HashMap::new();
    let rows: Vec<(i32, MarketSymbol, Decimal)> =
        sqlx::query_as("SELECT user_id, market, quantity FROM positions")
            .fetch_all(pool)
            .await
            .unwrap_or_default();
    for (user_id, market, quantity) in rows {
        positions.insert((user_id, market), quantity);
    }

    let (last_event_id,): (i64,) =
        sqlx::query_as("SELECT last_applied_event_id FROM engine_event_cursor")
            .fetch_one(pool)
            .await
            .unwrap_or((0,));

    let (max_trade_id,): (Option<i64>,) = sqlx::query_as("SELECT MAX(trade_id) FROM trades")
        .fetch_one(pool)
        .await
        .unwrap_or((None,));

    let fee_tiers: HashMap<i32, i16> = sqlx::query_as("SELECT id, fee_tier FROM users")
        .fetch_all(pool)
        .await
        .unwrap_or_default()
        .into_iter()
        .collect();

    let fee_rates: HashMap<i16, FeeTier> = sqlx::query_as::<_, FeeTier>("SELECT * FROM fee_tiers")
        .fetch_all(pool)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|t| (t.tier, t))
        .collect();

    let markets: HashMap<MarketSymbol, Market> = sqlx::query_as::<_, Market>("SELECT * FROM markets")
        .fetch_all(pool)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|m| (m.market, m))
        .collect();

    let mark_prices: HashMap<MarketSymbol, Decimal> = markets
        .values()
        .filter_map(|m| m.mark_price.map(|p| (m.market, p)))
        .collect();

    // Positions found already flagged in Postgres (e.g. process restarted
    // mid-liquidation) resume as flagged in memory too.
    let liquidating_scopes: HashSet<(i32, Option<MarketSymbol>)> = sqlx::query_as::<_, (i32, MarketSymbol, MarginMode)>(
        "SELECT user_id, market, margin_mode FROM positions WHERE is_liquidating = TRUE",
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default()
    .into_iter()
    .map(|(user_id, market, mode)| match mode {
        MarginMode::Cross => (user_id, None),
        MarginMode::Isolated => (user_id, Some(market)),
    })
    .collect();

    EngineState {
        books,
        positions,
        fee_tiers,
        fee_rates,
        markets,
        mark_prices,
        index_prices: HashMap::new(),
        liquidating_scopes,
        next_event_id: last_event_id + 1,
        next_trade_id: max_trade_id.unwrap_or(0) + 1,
    }
}

/// The single match-loop task: bootstraps state from Postgres, then
/// processes IntakeCommands one at a time for as long as the process runs.
/// No other task ever touches `books`/`positions` directly.
pub async fn run_match_loop(
    pool: PgPool,
    redis_client: redis::Client,
    events_stream_key: String,
    mut rx: mpsc::Receiver<IntakeCommand>,
) {
    let mut state = bootstrap(&pool).await;

    let mut events_conn = match redis_client.get_multiplexed_async_connection().await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("match-loop: redis connect failed: {e}");
            return;
        }
    };

    tracing::info!(
        "match-loop starting: next_event_id={}, next_trade_id={}",
        state.next_event_id,
        state.next_trade_id
    );

    let mut snapshot_ticker = tokio::time::interval(SNAPSHOT_INTERVAL);
    snapshot_ticker.tick().await; // first tick fires immediately; skip it

    loop {
        tokio::select! {
            command = rx.recv() => {
                let Some(command) = command else { break };
                match command {
                    IntakeCommand::NewOrder(order) => {
                        handle_new_order(&mut state, &pool, &mut events_conn, &events_stream_key, order).await;
                    }
                    IntakeCommand::CancelOrder { order_id, market } => {
                        let removed = state.books.get_mut(&market).is_some_and(|book| book.remove_by_id(order_id).is_some());
                        if removed {
                            tracing::info!("removed cancelled order {order_id} from {market:?} book");
                            publish_book_update(&state, &mut events_conn, market).await;
                        }
                    }
                    IntakeCommand::ReloadFeeTiers(fee_tiers) => {
                        tracing::info!("reloaded fee tiers for {} accounts", fee_tiers.len());
                        state.fee_tiers = fee_tiers;
                    }
                    IntakeCommand::SampleFunding => {
                        sample_funding(&state, &pool);
                    }
                    IntakeCommand::SettleFunding => {
                        settle_funding(&mut state, &pool, &mut events_conn, &events_stream_key).await;
                    }
                    IntakeCommand::CheckLiquidations => {
                        check_liquidations(&mut state, &pool, &mut events_conn, &events_stream_key).await;
                    }
                    IntakeCommand::UpdateIndexPrice { market, price } => {
                        state.index_prices.insert(market, price);
                    }
                }
            }
            _ = snapshot_ticker.tick() => {
                take_snapshot(&state, &pool);
            }
        }
    }
}

/// Serializes state synchronously (fast, no I/O) then hands the bytes off
/// to a detached task for the actual Postgres write, so the match loop
/// never blocks on a snapshot - matching never waits on the database.
fn take_snapshot(state: &EngineState, pool: &PgPool) {
    let bytes = match serde_json::to_vec(state) {
        Ok(b) => b,
        Err(e) => {
            tracing::error!("snapshot serialize failed: {e}");
            return;
        }
    };
    let last_event_id = state.next_event_id - 1;
    let pool = pool.clone();
    tokio::spawn(async move {
        if let Err(e) = store::ledger::save_snapshot(&pool, &bytes, last_event_id).await {
            tracing::error!("snapshot save failed: {e}");
        } else {
            tracing::info!("snapshot saved: last_event_id={last_event_id}, {} bytes", bytes.len());
        }
    });
}

/// Samples the premium index for every market: walk the book for the
/// bid/ask VWAP that would fill `impact_notional`, falling back to Index
/// (the Pyth oracle price - see the `index_prices` doc comment) with zero
/// contribution on whichever side can't fill it. Fire-and-forget DB write,
/// same pattern as take_snapshot, so the match loop never blocks on it.
fn sample_funding(state: &EngineState, pool: &PgPool) {
    for (&market, book) in &state.books {
        let Some(index) = state.index_prices.get(&market).copied() else {
            continue; // no Pyth index price yet (cold start before first poll)
        };
        let Some(market_cfg) = state.markets.get(&market) else { continue };

        let bid_impact = book.bid_impact_vwap(market_cfg.impact_notional).unwrap_or(index);
        let ask_impact = book.ask_impact_vwap(market_cfg.impact_notional).unwrap_or(index);

        let ipd = (bid_impact - index).max(Decimal::ZERO) - (index - ask_impact).max(Decimal::ZERO);
        let premium_index = if index == Decimal::ZERO { Decimal::ZERO } else { ipd / index };

        let pool = pool.clone();
        tokio::spawn(async move {
            if let Err(e) =
                store::ledger::insert_funding_sample(&pool, market, index, bid_impact, ask_impact, premium_index).await
            {
                tracing::error!("funding sample insert failed for {market:?}: {e}");
            }
        });
    }
}

/// Hourly settlement, per the spec's formula exactly:
/// mean_P -> F_8h = scale*(mean_P + clamp(0.0001-mean_P, +/-0.0005)) ->
/// FR_hour = clamp(F_8h/8, +/-0.04). scale is fixed at 1.0 (crypto-only
/// exchange; the 0.5 "non-crypto" branch is dead code for this market set
/// but the constant is kept named in case that changes). Settles every
/// open position in-memory (not a Postgres query) and emits one
/// FUNDING_SETTLED event per position for the ledger writer to apply.
async fn settle_funding(
    state: &mut EngineState,
    pool: &PgPool,
    events_conn: &mut redis::aio::MultiplexedConnection,
    events_stream_key: &str,
) {
    let interest_leg: Decimal = "0.0001".parse().unwrap();
    let interest_clamp: Decimal = "0.0005".parse().unwrap();
    let hour_cap: Decimal = "0.04".parse().unwrap();
    let scale = Decimal::ONE; // crypto markets only

    let settlement_time = Utc::now();
    let window_start = settlement_time - chrono::Duration::hours(1);

    for market in [MarketSymbol::Sol, MarketSymbol::Eth, MarketSymbol::Btc] {
        let mean_p = match store::ledger::mean_premium_index(pool, market, window_start).await {
            Ok(Some(p)) => p,
            Ok(None) => continue, // no samples this window
            Err(e) => {
                tracing::error!("mean_premium_index failed for {market:?}: {e}");
                continue;
            }
        };

        let diff = (interest_leg - mean_p).clamp(-interest_clamp, interest_clamp);
        let f_8h = scale * (mean_p + diff);
        let fr_hour = (f_8h / Decimal::from(8)).clamp(-hour_cap, hour_cap);

        let Some(mark_price) = state.mark_prices.get(&market).copied() else { continue };

        let to_settle: Vec<(i32, Decimal)> = state
            .positions
            .iter()
            .filter(|&(&(_, m), &qty)| m == market && qty != Decimal::ZERO)
            .map(|(&(user_id, _), &qty)| (user_id, qty))
            .collect();

        tracing::info!(
            "funding settlement {market:?}: mean_P={mean_p}, FR_hour={fr_hour}, {} positions",
            to_settle.len()
        );

        for (user_id, qty) in to_settle {
            // rate>0 (perp rich): longs pay shorts. rate<0: shorts pay
            // longs. This single signed formula covers both directions
            // uniformly - see settle_funding's callers for the derivation.
            let amount = -qty * mark_price * fr_hour;

            let event = store::ledger::FundingPaymentEvent {
                event_id: state.next_event_id,
                settlement_time,
                market,
                user_id,
                position_qty: qty,
                funding_rate_hour: fr_hour,
                amount,
            };
            state.next_event_id += 1;
            publish_event(events_conn, events_stream_key, "FUNDING_SETTLED", &event).await;
        }
    }
}

/// Returns `None` if the order was rejected outright (liquidation-flagged
/// scope) rather than matched - callers that need to know whether an
/// unwind IOC fully filled (the liquidation backstop check) inspect
/// `Some(outcome).cancelled_remainder`.
async fn handle_new_order(
    state: &mut EngineState,
    pool: &PgPool,
    events_conn: &mut redis::aio::MultiplexedConnection,
    events_stream_key: &str,
    order: Order,
) -> Option<MatchOutcome> {
    if !order.is_liquidation {
        let cross_flagged = state.liquidating_scopes.contains(&(order.user_id, None));
        let market_flagged = state.liquidating_scopes.contains(&(order.user_id, Some(order.market)));
        if cross_flagged || market_flagged {
            reject_order(state, events_conn, events_stream_key, &order).await;
            return None;
        }
    }

    let Some(book) = state.books.get_mut(&order.market) else {
        tracing::error!("no order book for market {:?}", order.market);
        return None;
    };

    let position_qty = state.positions.get(&(order.user_id, order.market)).copied().unwrap_or(Decimal::ZERO);

    let new_order = matcher::NewOrder {
        order_id: order.id,
        user_id: order.user_id,
        market: order.market,
        variant: order.variant,
        order_type: order.order_type,
        tif: order.tif,
        reduce_only: order.reduce_only,
        leverage: order.leverage,
        margin_mode: order.margin_mode,
        price: order.price,
        quantity: order.remaining_qty,
        is_liquidation: order.is_liquidation,
    };

    let outcome = matcher::match_order(book, &new_order, position_qty);

    publish_user_order_event(events_conn, order.user_id, "ACCEPTED", order.id, serde_json::json!({})).await;

    tracing::info!(
        "order {} ({:?} {:?} {}@{:?}): {} fills, {} rested, {} cancelled remainder",
        order.id,
        order.variant,
        order.order_type,
        order.remaining_qty,
        order.price,
        outcome.fills.len(),
        outcome.rested_order_id.map(|_| order.remaining_qty).unwrap_or(Decimal::ZERO),
        outcome.cancelled_remainder,
    );

    for fill in &outcome.fills {
        apply_fill_to_position_cache(state, &order, fill);
        emit_fill_event(state, pool, events_conn, events_stream_key, &order, fill).await;
    }

    // Any order with at least one fill already gets OPEN/PARTIALLY_FILLED/
    // FILLED set by apply_fill_side - ORDER_RESTED only covers the one gap
    // that leaves an order stuck at PENDING: a GTC LIMIT that rests with
    // zero fills.
    if outcome.rested_order_id.is_some() && outcome.fills.is_empty() {
        let event = store::ledger::OrderRestedEvent {
            event_id: state.next_event_id,
            order_id: order.id,
        };
        state.next_event_id += 1;
        publish_event(events_conn, events_stream_key, "ORDER_RESTED", &event).await;
        publish_user_order_event(events_conn, order.user_id, "RESTED", order.id, serde_json::json!({})).await;
    }

    if outcome.cancelled_remainder > Decimal::ZERO {
        let event = store::ledger::OrderCancelledRemainderEvent {
            event_id: state.next_event_id,
            order_id: order.id,
        };
        state.next_event_id += 1;
        publish_event(events_conn, events_stream_key, "ORDER_CANCELLED_REMAINDER", &event).await;
        publish_user_order_event(
            events_conn,
            order.user_id,
            "CANCELED",
            order.id,
            serde_json::json!({ "cancelled_remainder": outcome.cancelled_remainder }),
        )
        .await;
    }

    // Resting a new order or filling against the book both change depth,
    // even when no fill occurred (a GTC LIMIT resting untouched still adds
    // liquidity) - so this fires on either, not fill-only like the old
    // top-of-book-only publish did.
    if !outcome.fills.is_empty() || outcome.rested_order_id.is_some() {
        publish_book_update(state, events_conn, order.market).await;
    }

    Some(outcome)
}

/// Publishes both the best-bid/ask and full depth snapshot for a market
/// right after its book was mutated (new order, fill, or cancel).
async fn publish_book_update(state: &EngineState, events_conn: &mut redis::aio::MultiplexedConnection, market: MarketSymbol) {
    let Some(book) = state.books.get(&market) else { return };
    publish_book_top(events_conn, market, book.best_bid(), book.best_ask()).await;
    let (bids, asks) = book.depth(DEPTH_LEVELS);
    publish_depth(events_conn, market, &bids, &asks).await;
}

async fn reject_order(
    state: &mut EngineState,
    events_conn: &mut redis::aio::MultiplexedConnection,
    events_stream_key: &str,
    order: &Order,
) {
    let event = store::ledger::OrderRejectedEvent {
        event_id: state.next_event_id,
        order_id: order.id,
    };
    state.next_event_id += 1;
    publish_event(events_conn, events_stream_key, "ORDER_REJECTED", &event).await;
    publish_user_order_event(
        events_conn,
        order.user_id,
        "REJECTED",
        order.id,
        serde_json::json!({ "reason": "account or market flagged for liquidation" }),
    )
    .await;
}

fn apply_fill_to_position_cache(state: &mut EngineState, taker_order: &Order, fill: &MatchFill) {
    let taker_delta = match taker_order.variant {
        OrderVariant::Long => fill.quantity,
        OrderVariant::Short => -fill.quantity,
    };
    state.apply_position_delta(taker_order.user_id, taker_order.market, taker_delta);

    let maker_variant = taker_order.variant.opposite();
    let maker_delta = match maker_variant {
        OrderVariant::Long => fill.quantity,
        OrderVariant::Short => -fill.quantity,
    };
    state.apply_position_delta(fill.maker_user_id, taker_order.market, maker_delta);
}

async fn emit_fill_event(
    state: &mut EngineState,
    pool: &PgPool,
    events_conn: &mut redis::aio::MultiplexedConnection,
    events_stream_key: &str,
    taker_order: &Order,
    fill: &MatchFill,
) {
    let notional = fill.price * fill.quantity;

    let mut taker_rate = state.fee_rate(taker_order.user_id, false);
    if taker_order.is_liquidation {
        taker_rate += state.liquidation_fee_rate(taker_order.market);
    }
    let maker_rate = state.fee_rate(fill.maker_user_id, true);

    let event = FillEvent {
        event_id: state.next_event_id,
        trade_id: state.next_trade_id,
        time: Utc::now(),
        market: taker_order.market,
        price: fill.price,
        quantity: fill.quantity,
        maker_order_id: fill.maker_order_id,
        taker_order_id: taker_order.id,
        maker_user_id: fill.maker_user_id,
        taker_user_id: taker_order.user_id,
        taker_variant: taker_order.variant,
        // maker_rate is negative at the top tier - that's the rebate,
        // applied as a plain sign-flipped transfer downstream in
        // apply_fill_side, no special-cased branch needed.
        maker_fee: notional * maker_rate,
        taker_fee: notional * taker_rate,
        is_liquidation: taker_order.is_liquidation,
    };

    state.next_event_id += 1;
    state.next_trade_id += 1;

    publish_event(events_conn, events_stream_key, "FILL", &event).await;

    publish_trade(
        events_conn,
        taker_order.market,
        fill.price,
        fill.quantity,
        taker_order.variant,
        taker_order.is_liquidation,
    )
    .await;
    publish_ticker(events_conn, taker_order.market, fill.price).await;
    publish_user_order_event(
        events_conn,
        taker_order.user_id,
        "FILL",
        taker_order.id,
        serde_json::json!({ "role": "taker", "price": fill.price, "quantity": fill.quantity }),
    )
    .await;
    publish_user_order_event(
        events_conn,
        fill.maker_user_id,
        "FILL",
        fill.maker_order_id,
        serde_json::json!({ "role": "maker", "price": fill.price, "quantity": fill.quantity }),
    )
    .await;

    // Mark price = last-trade price (documented stub for a future external
    // oracle). Cached in-memory for MARKET-order estimation elsewhere in
    // the match loop, and mirrored to Postgres so store::orders::
    // place_order can read it synchronously without an engine dependency.
    state.mark_prices.insert(taker_order.market, fill.price);
    let pool = pool.clone();
    let market = taker_order.market;
    let price = fill.price;
    tokio::spawn(async move {
        if let Err(e) = store::orders::update_mark_price(&pool, market, price).await {
            tracing::error!("failed to update mark_price for {market:?}: {e}");
        }
    });
}

// ============================================================
// Liquidation
//
// MarginRatio = Equity / MaintenanceMargin; liquidation triggers when
// Equity < MaintenanceMargin, clears when Equity >= InitialMargin (the
// spec's "recovery initial margin"). Cross evaluates an account's combined
// cross exposure; isolated evaluates one position independently. Health is
// polled here rather than recomputed synchronously on every fill/mark-price
// tick - a documented simplification of "continuous" - see
// IntakeCommand::CheckLiquidations.
// ============================================================

/// Picks which of `user_id`'s open cross positions to unwind first: the one
/// with the largest absolute notional (`|qty| * mark_price`). A position
/// with no mark price yet is treated as zero notional (never picked ahead
/// of one that does have a price) rather than panicking or being skipped
/// entirely, so unwinding still makes progress.
fn largest_notional_position(
    positions: &HashMap<(i32, MarketSymbol), Decimal>,
    mark_prices: &HashMap<MarketSymbol, Decimal>,
    user_id: i32,
) -> Option<MarketSymbol> {
    positions
        .iter()
        .filter(|&(&(u, _), &qty)| u == user_id && qty != Decimal::ZERO)
        .max_by_key(|&(&(_, m), &qty)| qty.abs() * mark_prices.get(&m).copied().unwrap_or(Decimal::ZERO))
        .map(|(&(_, m), _)| m)
}

async fn check_liquidations(
    state: &mut EngineState,
    pool: &PgPool,
    events_conn: &mut redis::aio::MultiplexedConnection,
    events_stream_key: &str,
) {
    let cross_rows: Vec<(i32, Decimal, Decimal, Decimal)> = sqlx::query_as(
        "SELECT p.user_id,
                (u.collateral_available + u.collateral_locked + SUM(p.quantity * (m.mark_price - p.average_price))) AS equity,
                SUM(m.maintenance_margin_rate * ABS(p.quantity) * m.mark_price) AS maintenance_margin,
                SUM(m.initial_margin_rate * ABS(p.quantity) * m.mark_price) AS initial_margin
         FROM positions p
         JOIN users u ON u.id = p.user_id
         JOIN markets m ON m.market = p.market
         WHERE p.margin_mode = 'CROSS' AND p.quantity <> 0 AND m.mark_price IS NOT NULL
         GROUP BY p.user_id, u.collateral_available, u.collateral_locked",
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    let mut cross_health: HashMap<i32, (Decimal, Decimal)> = HashMap::new();
    for (user_id, equity, mm, im) in cross_rows {
        cross_health.insert(user_id, (equity, mm));
        let scope = (user_id, None);
        let flagged = state.liquidating_scopes.contains(&scope);
        if !flagged && mm > Decimal::ZERO && equity < mm {
            state.liquidating_scopes.insert(scope);
            spawn_set_liquidating(pool.clone(), user_id, None, true);
            tracing::warn!("account {user_id} flagged for CROSS liquidation: equity={equity}, mm={mm}");
        } else if flagged && equity >= im {
            state.liquidating_scopes.remove(&scope);
            spawn_set_liquidating(pool.clone(), user_id, None, false);
            tracing::info!("account {user_id} recovered from CROSS liquidation: equity={equity}, im={im}");
        }
    }

    let cross_flagged_users: Vec<i32> = state
        .liquidating_scopes
        .iter()
        .filter_map(|&(u, m)| if m.is_none() { Some(u) } else { None })
        .collect();

    for user_id in cross_flagged_users {
        let market = largest_notional_position(&state.positions, &state.mark_prices, user_id);
        let Some(market) = market else { continue };
        let (equity, mm) = cross_health.get(&user_id).copied().unwrap_or((Decimal::ZERO, Decimal::ZERO));
        unwind_scope(state, pool, events_conn, events_stream_key, user_id, market, equity, mm).await;
    }

    let iso_rows: Vec<(i32, MarketSymbol, Decimal, Decimal, Decimal)> = sqlx::query_as(
        "SELECT p.user_id, p.market,
                (p.allocated_margin + p.quantity * (m.mark_price - p.average_price)) AS equity,
                m.maintenance_margin_rate * ABS(p.quantity) * m.mark_price AS maintenance_margin,
                m.initial_margin_rate * ABS(p.quantity) * m.mark_price AS initial_margin
         FROM positions p
         JOIN markets m ON m.market = p.market
         WHERE p.margin_mode = 'ISOLATED' AND p.quantity <> 0 AND m.mark_price IS NOT NULL",
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    let mut iso_health: HashMap<(i32, MarketSymbol), (Decimal, Decimal)> = HashMap::new();
    for (user_id, market, equity, mm, im) in iso_rows {
        iso_health.insert((user_id, market), (equity, mm));
        let scope = (user_id, Some(market));
        let flagged = state.liquidating_scopes.contains(&scope);
        if !flagged && mm > Decimal::ZERO && equity < mm {
            state.liquidating_scopes.insert(scope);
            spawn_set_liquidating(pool.clone(), user_id, Some(market), true);
            tracing::warn!("position ({user_id},{market:?}) flagged for ISOLATED liquidation: equity={equity}, mm={mm}");
        } else if flagged && equity >= im {
            state.liquidating_scopes.remove(&scope);
            spawn_set_liquidating(pool.clone(), user_id, Some(market), false);
            tracing::info!("position ({user_id},{market:?}) recovered from ISOLATED liquidation");
        }
    }

    let iso_flagged: Vec<(i32, MarketSymbol)> =
        state.liquidating_scopes.iter().filter_map(|&(u, m)| m.map(|mkt| (u, mkt))).collect();

    for (user_id, market) in iso_flagged {
        let (equity, mm) = iso_health.get(&(user_id, market)).copied().unwrap_or((Decimal::ZERO, Decimal::ZERO));
        unwind_scope(state, pool, events_conn, events_stream_key, user_id, market, equity, mm).await;
    }
}

fn spawn_set_liquidating(pool: PgPool, user_id: i32, market: Option<MarketSymbol>, flagged: bool) {
    tokio::spawn(async move {
        let result = match market {
            Some(m) => {
                sqlx::query(
                    "UPDATE positions SET is_liquidating = $1, liquidation_flagged_at = CASE WHEN $1 THEN NOW() ELSE NULL END
                     WHERE user_id = $2 AND market = $3 AND margin_mode = 'ISOLATED'",
                )
                .bind(flagged)
                .bind(user_id)
                .bind(m)
                .execute(&pool)
                .await
            }
            None => {
                sqlx::query(
                    "UPDATE positions SET is_liquidating = $1, liquidation_flagged_at = CASE WHEN $1 THEN NOW() ELSE NULL END
                     WHERE user_id = $2 AND margin_mode = 'CROSS'",
                )
                .bind(flagged)
                .bind(user_id)
                .execute(&pool)
                .await
            }
        };
        if let Err(e) = result {
            tracing::error!("failed to update is_liquidating for user {user_id}: {e}");
        }
    });
}

/// Clears whichever liquidation flag(s) `user_id`/`market` currently has -
/// used when a position closes fully, since a closed position (quantity=0)
/// drops out of check_liquidations' query and would otherwise never be
/// revisited by the normal flag/clear pass. The cross flag only clears once
/// *every* cross position for the account is closed (a cross account can
/// hold positions in several markets - closing one doesn't mean the account
/// overall has recovered), checked against Postgres since state.positions
/// doesn't distinguish cross from isolated exposure.
fn clear_flag(state: &mut EngineState, pool: &PgPool, user_id: i32, market: MarketSymbol) {
    if state.liquidating_scopes.contains(&(user_id, None)) {
        // Checked against the engine's own in-memory position cache, not
        // Postgres: the position-deletion for the fill that just closed
        // this out is applied asynchronously by the ledger writer, so a
        // Postgres query here would race it and (almost always) still see
        // the stale pre-fill row - state.positions is immediately
        // consistent since this task is the one that just updated it.
        let other_positions_open = state.positions.iter().any(|(&(u, _), &qty)| u == user_id && qty != Decimal::ZERO);
        if !other_positions_open {
            state.liquidating_scopes.remove(&(user_id, None));
            spawn_set_liquidating(pool.clone(), user_id, None, false);
            tracing::info!("account {user_id} liquidation flag cleared (all cross positions closed)");
        }
    }

    if state.liquidating_scopes.remove(&(user_id, Some(market))) {
        spawn_set_liquidating(pool.clone(), user_id, Some(market), false);
        tracing::info!("position ({user_id},{market:?}) liquidation flag cleared (position closed)");
    }
}

/// Submits one reduce-only IOC unwind order for `market`, sized to the
/// account's full remaining exposure there (IOC naturally partial-fills
/// against whatever the book can absorb). If the book couldn't fully fill
/// it and equity has fallen far enough that further book attempts are
/// unlikely to help, triggers the insurance-fund backstop instead.
async fn unwind_scope(
    state: &mut EngineState,
    pool: &PgPool,
    events_conn: &mut redis::aio::MultiplexedConnection,
    events_stream_key: &str,
    user_id: i32,
    market: MarketSymbol,
    equity: Decimal,
    mm: Decimal,
) {
    let qty = state.positions.get(&(user_id, market)).copied().unwrap_or(Decimal::ZERO);
    if qty == Decimal::ZERO {
        // Already closed. A fully-closed position drops out of the
        // check_liquidations query entirely (WHERE quantity <> 0), so
        // nothing else will ever clear this flag - do it here instead of
        // relying on a future pass that will never see this scope again.
        clear_flag(state, pool, user_id, market);
        return;
    }

    let row: Option<(MarginMode, i16)> = sqlx::query_as("SELECT margin_mode, leverage FROM positions WHERE user_id = $1 AND market = $2")
        .bind(user_id)
        .bind(market)
        .fetch_optional(pool)
        .await
        .unwrap_or(None);
    let Some((margin_mode, leverage)) = row else { return };

    let variant = if qty > Decimal::ZERO { OrderVariant::Short } else { OrderVariant::Long };
    let close_qty = qty.abs();

    let liquidation_order =
        match store::orders::create_liquidation_order(pool, user_id, market, variant, close_qty, leverage, margin_mode).await {
            Ok(o) => o,
            Err(e) => {
                tracing::error!("failed to create liquidation order for user {user_id} market {market:?}: {e}");
                return;
            }
        };

    tracing::warn!(
        "liquidating {close_qty} {market:?} for user {user_id} (equity={equity}, mm={mm})"
    );

    let Some(outcome) = handle_new_order(state, pool, events_conn, events_stream_key, liquidation_order).await else {
        return; // is_liquidation=true bypasses the reject path, but stay safe
    };

    if outcome.cancelled_remainder > Decimal::ZERO {
        let backstop_ratio = state.markets.get(&market).map(|m| m.backstop_equity_ratio).unwrap_or(Decimal::ZERO);
        if mm > Decimal::ZERO && equity <= backstop_ratio * mm {
            trigger_insurance_backstop(state, events_conn, events_stream_key, user_id, market).await;
        }
    } else if state.positions.get(&(user_id, market)).copied().unwrap_or(Decimal::ZERO) == Decimal::ZERO {
        // Fully filled and the position is now flat - clear immediately
        // rather than waiting for a tick that will never see this scope
        // again (a closed position drops out of the health-check query).
        clear_flag(state, pool, user_id, market);
    }
}

/// Skips the book entirely and absorbs the remaining position (+ remaining
/// equity for cross; + allocated margin for isolated) directly into the
/// insurance-fund account. Triggered when an unwind IOC fails to fully fill
/// (the book demonstrably can't absorb it) and equity has fallen to a small
/// fraction of maintenance margin - see unwind_scope.
async fn trigger_insurance_backstop(
    state: &mut EngineState,
    events_conn: &mut redis::aio::MultiplexedConnection,
    events_stream_key: &str,
    user_id: i32,
    market: MarketSymbol,
) {
    let Some(mark_price) = state.mark_prices.get(&market).copied() else { return };

    let event = store::ledger::LiquidationTransferEvent {
        event_id: state.next_event_id,
        user_id,
        market,
        mark_price,
    };
    state.next_event_id += 1;
    publish_event(events_conn, events_stream_key, "LIQUIDATION_TRANSFER", &event).await;

    // Zero out the engine's own view immediately - the ledger writer makes
    // the same change in Postgres. Nothing left to liquidate, so the flag
    // clears too.
    state.positions.remove(&(user_id, market));
    state.liquidating_scopes.remove(&(user_id, None));
    state.liquidating_scopes.remove(&(user_id, Some(market)));

    tracing::error!("INSURANCE FUND BACKSTOP triggered for user {user_id} market {market:?} at mark_price={mark_price}");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> Decimal {
        s.parse().unwrap()
    }

    #[test]
    fn largest_notional_position_picks_biggest_notional_not_first_found() {
        let mut positions = HashMap::new();
        positions.insert((1, MarketSymbol::Sol), d("1")); // 1 * 100 = 100 notional
        positions.insert((1, MarketSymbol::Eth), d("10")); // 10 * 50 = 500 notional (largest)
        positions.insert((1, MarketSymbol::Btc), d("0.1")); // 0.1 * 1000 = 100 notional
        positions.insert((2, MarketSymbol::Eth), d("100")); // other user, must be ignored

        let mut mark_prices = HashMap::new();
        mark_prices.insert(MarketSymbol::Sol, d("100"));
        mark_prices.insert(MarketSymbol::Eth, d("50"));
        mark_prices.insert(MarketSymbol::Btc, d("1000"));

        assert_eq!(largest_notional_position(&positions, &mark_prices, 1), Some(MarketSymbol::Eth));
    }

    #[test]
    fn largest_notional_position_ignores_zero_quantity_positions() {
        let mut positions = HashMap::new();
        positions.insert((1, MarketSymbol::Sol), Decimal::ZERO);
        positions.insert((1, MarketSymbol::Eth), d("1"));
        let mut mark_prices = HashMap::new();
        mark_prices.insert(MarketSymbol::Eth, d("50"));

        assert_eq!(largest_notional_position(&positions, &mark_prices, 1), Some(MarketSymbol::Eth));
    }

    #[test]
    fn largest_notional_position_missing_mark_price_treated_as_zero_not_panicking() {
        let mut positions = HashMap::new();
        positions.insert((1, MarketSymbol::Sol), d("5")); // no mark price -> zero notional
        positions.insert((1, MarketSymbol::Eth), d("1")); // has a mark price -> nonzero, wins
        let mut mark_prices = HashMap::new();
        mark_prices.insert(MarketSymbol::Eth, d("50"));

        assert_eq!(largest_notional_position(&positions, &mark_prices, 1), Some(MarketSymbol::Eth));
    }

    #[test]
    fn largest_notional_position_no_open_positions_returns_none() {
        let positions = HashMap::new();
        let mark_prices = HashMap::new();
        assert_eq!(largest_notional_position(&positions, &mark_prices, 1), None);
    }
}
