use crate::book::{OrderBook, RestingOrder};
use rust_decimal::Decimal;
use std::collections::VecDeque;
use std::ops::Bound;
use store::models::{MarginMode, MarketSymbol, OrderType, OrderVariant, TimeInForce};

// `market` isn't read by the matcher itself (the caller already picked the
// right book) but is kept on the struct for later phases (liquidation's
// synthetic reduce-only orders, event construction).
#[allow(dead_code)]
pub struct NewOrder {
    pub order_id: i64,
    pub user_id: i32,
    pub market: MarketSymbol,
    pub variant: OrderVariant,
    pub order_type: OrderType,
    pub tif: TimeInForce,
    pub reduce_only: bool,
    pub leverage: i16,
    pub margin_mode: MarginMode,
    pub price: Option<Decimal>,
    pub quantity: Decimal,
    pub is_liquidation: bool,
}

#[derive(Debug, Clone)]
pub struct MatchFill {
    pub price: Decimal,
    pub quantity: Decimal,
    pub maker_order_id: i64,
    pub maker_user_id: i32,
}

pub struct MatchOutcome {
    pub fills: Vec<MatchFill>,
    /// Quantity from the (reduce-only-clamped) incoming order that neither
    /// filled nor rested - i.e. was IOC-cancelled or reduce-only-clamped
    /// away.
    pub cancelled_remainder: Decimal,
    pub rested_order_id: Option<i64>,
}

/// The single match-loop entry point: crosses `incoming` against the
/// opposing side of `book`, then either rests any GTC LIMIT remainder or
/// reports it as a cancelled remainder (LIMIT+IOC and MARKET, which is
/// always treated as IOC, never rest).
///
/// `position_qty` is the taker's current signed net position in this
/// market (positive = long, negative = short), used to clamp reduce-only
/// orders so they can never flip or increase a position.
pub fn match_order(book: &mut OrderBook, incoming: &NewOrder, position_qty: Decimal) -> MatchOutcome {
    let requested_qty = if incoming.reduce_only {
        let opposing_exposure = match incoming.variant {
            OrderVariant::Long => (-position_qty).max(Decimal::ZERO),
            OrderVariant::Short => position_qty.max(Decimal::ZERO),
        };
        incoming.quantity.min(opposing_exposure)
    } else {
        incoming.quantity
    };

    let mut remaining = requested_qty;
    let mut fills = Vec::new();

    if remaining > Decimal::ZERO {
        match incoming.variant {
            OrderVariant::Long => cross_asks(book, incoming, &mut remaining, &mut fills),
            OrderVariant::Short => cross_bids(book, incoming, &mut remaining, &mut fills),
        }
    }

    let is_ioc = incoming.tif == TimeInForce::Ioc || incoming.order_type == OrderType::Market;
    let can_rest = !is_ioc && !incoming.is_liquidation && remaining > Decimal::ZERO;

    let mut rested_order_id = None;
    let cancelled_remainder = if can_rest {
        book.insert_resting(RestingOrder {
            order_id: incoming.order_id,
            user_id: incoming.user_id,
            variant: incoming.variant,
            price: incoming.price.expect("GTC LIMIT orders always carry a price"),
            remaining,
            leverage: incoming.leverage,
            margin_mode: incoming.margin_mode,
            reduce_only: incoming.reduce_only,
            is_liquidation: incoming.is_liquidation,
        });
        rested_order_id = Some(incoming.order_id);
        Decimal::ZERO
    } else {
        remaining
    };

    MatchOutcome {
        fills,
        cancelled_remainder,
        rested_order_id,
    }
}

fn cross_asks(book: &mut OrderBook, incoming: &NewOrder, remaining: &mut Decimal, fills: &mut Vec<MatchFill>) {
    let mut skip_at_or_before: Option<Decimal> = None;

    loop {
        if *remaining <= Decimal::ZERO {
            break;
        }
        let next_price = match skip_at_or_before {
            None => book.asks.keys().next().copied(),
            Some(p) => book
                .asks
                .range((Bound::Excluded(p), Bound::Unbounded))
                .next()
                .map(|(p, _)| *p),
        };
        let Some(price) = next_price else { break };
        if let Some(limit) = incoming.price {
            if price > limit {
                break;
            }
        }

        let before = *remaining;
        let level = book.asks.get_mut(&price).expect("price came from asks.keys()");
        match_against_level(level, incoming.user_id, remaining, fills, price);
        if level.is_empty() {
            book.asks.remove(&price);
        }
        if *remaining == before {
            // Level was entirely self-orders (skipped) - move past it
            // permanently for this call, or we'd loop on it forever.
            skip_at_or_before = Some(price);
        }
    }
}

fn cross_bids(book: &mut OrderBook, incoming: &NewOrder, remaining: &mut Decimal, fills: &mut Vec<MatchFill>) {
    let mut skip_at_or_after: Option<Decimal> = None;

    loop {
        if *remaining <= Decimal::ZERO {
            break;
        }
        let next_price = match skip_at_or_after {
            None => book.bids.keys().next_back().copied(),
            Some(p) => book.bids.range(..p).next_back().map(|(p, _)| *p),
        };
        let Some(price) = next_price else { break };
        if let Some(limit) = incoming.price {
            if price < limit {
                break;
            }
        }

        let before = *remaining;
        let level = book.bids.get_mut(&price).expect("price came from bids.keys()");
        match_against_level(level, incoming.user_id, remaining, fills, price);
        if level.is_empty() {
            book.bids.remove(&price);
        }
        if *remaining == before {
            skip_at_or_after = Some(price);
        }
    }
}

/// Matches `remaining` against one price level, FIFO, skipping resting
/// orders that belong to `taker_user_id` (self-trade prevention) while
/// preserving their relative priority for future orders.
fn match_against_level(
    level: &mut VecDeque<RestingOrder>,
    taker_user_id: i32,
    remaining: &mut Decimal,
    fills: &mut Vec<MatchFill>,
    price: Decimal,
) {
    let mut skipped = Vec::new();

    while *remaining > Decimal::ZERO {
        let Some(front) = level.front() else { break };
        if front.user_id == taker_user_id {
            skipped.push(level.pop_front().expect("front() just returned Some"));
            continue;
        }

        let resting = level.front_mut().expect("front() just returned Some");
        let fill_qty = (*remaining).min(resting.remaining);
        fills.push(MatchFill {
            price,
            quantity: fill_qty,
            maker_order_id: resting.order_id,
            maker_user_id: resting.user_id,
        });
        resting.remaining -= fill_qty;
        *remaining -= fill_qty;
        if resting.remaining == Decimal::ZERO {
            level.pop_front();
        }
    }

    for order in skipped.into_iter().rev() {
        level.push_front(order);
    }
}
