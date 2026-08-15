use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, VecDeque};
use store::models::{MarginMode, OrderVariant};

/// (price, total remaining quantity) at one book level.
pub type DepthLevel = (Decimal, Decimal);

// leverage/margin_mode/reduce_only/is_liquidation aren't read by the match
// loop yet - they're what the liquidation subsystem (a later phase) scans
// the book for (e.g. to skip matching against another account's own
// in-flight liquidation orders, or margin-mode-aware accounting).
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestingOrder {
    pub order_id: i64,
    pub user_id: i32,
    pub variant: OrderVariant,
    pub price: Decimal,
    pub remaining: Decimal,
    pub leverage: i16,
    pub margin_mode: MarginMode,
    pub reduce_only: bool,
    pub is_liquidation: bool,
}

/// Standard price-time-priority book: BTreeMap<price, VecDeque<order>> per
/// side gives O(log n) best-price access and O(1) FIFO append/pop per
/// level - no exotic structure needed at the scale of a handful of markets
/// with thousands of resting orders each.
///
/// bids/asks serialize via `decimal_keyed_map` (a Vec of pairs) rather than
/// directly, because the snapshot format is JSON (serde_json can't use a
/// Decimal - a float under this workspace's rust_decimal feature flags - as
/// an object key). by_id is never persisted at all: it's a pure derived
/// index, cheaply rebuilt from bids/asks via rebuild_index() right after a
/// snapshot loads, so it can never drift from them in the persisted blob.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct OrderBook {
    #[serde(with = "decimal_keyed_map")]
    pub bids: BTreeMap<Decimal, VecDeque<RestingOrder>>, // iterate .next_back() for best (highest)
    #[serde(with = "decimal_keyed_map")]
    pub asks: BTreeMap<Decimal, VecDeque<RestingOrder>>, // iterate .next() for best (lowest)
    #[serde(skip)]
    by_id: HashMap<i64, (OrderVariant, Decimal)>,
}

impl OrderBook {
    pub fn new() -> Self {
        Self::default()
    }

    /// Rebuilds the by_id index from bids/asks. Call once after
    /// deserializing a snapshot (by_id itself is never persisted).
    pub fn rebuild_index(&mut self) {
        self.by_id.clear();
        for (&price, level) in self.bids.iter().chain(self.asks.iter()) {
            for order in level {
                self.by_id.insert(order.order_id, (order.variant, price));
            }
        }
    }

    pub fn best_bid(&self) -> Option<Decimal> {
        self.bids.keys().next_back().copied()
    }

    pub fn best_ask(&self) -> Option<Decimal> {
        self.asks.keys().next().copied()
    }

    /// VWAP achieved selling into the top bids while filling `notional`
    /// quote units. `None` if the bids can't absorb the full notional -
    /// the funding sampler's "fall back to Index" case.
    pub fn bid_impact_vwap(&self, notional: Decimal) -> Option<Decimal> {
        impact_vwap(self.bids.iter().rev(), notional)
    }

    /// VWAP paid buying from the top asks while filling `notional` quote
    /// units. `None` if the asks can't absorb the full notional.
    pub fn ask_impact_vwap(&self, notional: Decimal) -> Option<Decimal> {
        impact_vwap(self.asks.iter(), notional)
    }

    pub fn insert_resting(&mut self, order: RestingOrder) {
        self.by_id.insert(order.order_id, (order.variant, order.price));
        let side = match order.variant {
            OrderVariant::Long => &mut self.bids,
            OrderVariant::Short => &mut self.asks,
        };
        side.entry(order.price).or_default().push_back(order);
    }

    /// Top `levels` price levels per side, each summed to (price, total
    /// remaining quantity resting at that price) - same per-level summation
    /// `impact_vwap` already does, just without the notional-walk cutoff.
    pub fn depth(&self, levels: usize) -> (Vec<DepthLevel>, Vec<DepthLevel>) {
        let bids = self
            .bids
            .iter()
            .rev()
            .take(levels)
            .map(|(&price, level)| (price, level.iter().map(|o| o.remaining).sum()))
            .collect();
        let asks = self
            .asks
            .iter()
            .take(levels)
            .map(|(&price, level)| (price, level.iter().map(|o| o.remaining).sum()))
            .collect();
        (bids, asks)
    }

    pub fn remove_by_id(&mut self, order_id: i64) -> Option<RestingOrder> {
        let (variant, price) = self.by_id.remove(&order_id)?;
        let side = match variant {
            OrderVariant::Long => &mut self.bids,
            OrderVariant::Short => &mut self.asks,
        };
        let level = side.get_mut(&price)?;
        let idx = level.iter().position(|o| o.order_id == order_id)?;
        let removed = level.remove(idx);
        if level.is_empty() {
            side.remove(&price);
        }
        removed
    }
}

fn impact_vwap<'a>(levels: impl Iterator<Item = (&'a Decimal, &'a VecDeque<RestingOrder>)>, notional: Decimal) -> Option<Decimal> {
    let mut remaining = notional;
    let mut filled_notional = Decimal::ZERO;
    let mut filled_qty = Decimal::ZERO;

    for (&price, level) in levels {
        if remaining <= Decimal::ZERO {
            break;
        }
        let level_qty: Decimal = level.iter().map(|o| o.remaining).sum();
        let level_notional = level_qty * price;
        let take_notional = remaining.min(level_notional);
        let take_qty = if price == Decimal::ZERO { Decimal::ZERO } else { take_notional / price };
        filled_notional += take_notional;
        filled_qty += take_qty;
        remaining -= take_notional;
    }

    if remaining > Decimal::ZERO || filled_qty == Decimal::ZERO {
        return None;
    }
    Some(filled_notional / filled_qty)
}

mod decimal_keyed_map {
    use super::{BTreeMap, Decimal, RestingOrder, VecDeque};
    use serde::{Deserialize, Deserializer, Serializer, ser::SerializeSeq};

    pub fn serialize<S: Serializer>(
        map: &BTreeMap<Decimal, VecDeque<RestingOrder>>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(Some(map.len()))?;
        for (price, level) in map {
            seq.serialize_element(&(*price, level))?;
        }
        seq.end()
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<BTreeMap<Decimal, VecDeque<RestingOrder>>, D::Error> {
        let entries = Vec::<(Decimal, VecDeque<RestingOrder>)>::deserialize(deserializer)?;
        Ok(entries.into_iter().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> Decimal {
        s.parse().unwrap()
    }

    fn resting(order_id: i64, variant: OrderVariant, price: Decimal, remaining: Decimal) -> RestingOrder {
        RestingOrder {
            order_id,
            user_id: 1,
            variant,
            price,
            remaining,
            leverage: 1,
            margin_mode: MarginMode::Cross,
            reduce_only: false,
            is_liquidation: false,
        }
    }

    #[test]
    fn depth_empty_book_returns_empty_vecs() {
        let book = OrderBook::new();
        let (bids, asks) = book.depth(10);
        assert!(bids.is_empty());
        assert!(asks.is_empty());
    }

    #[test]
    fn depth_single_level_sums_quantity_at_price() {
        let mut book = OrderBook::new();
        book.insert_resting(resting(1, OrderVariant::Long, d("100"), d("2")));
        book.insert_resting(resting(2, OrderVariant::Long, d("100"), d("3")));
        let (bids, asks) = book.depth(10);
        assert_eq!(bids, vec![(d("100"), d("5"))]);
        assert!(asks.is_empty());
    }

    #[test]
    fn depth_orders_bids_desc_and_asks_asc_and_respects_levels_cap() {
        let mut book = OrderBook::new();
        for (id, price) in [(1, d("99")), (2, d("100")), (3, d("101"))] {
            book.insert_resting(resting(id, OrderVariant::Long, price, d("1")));
        }
        for (id, price) in [(4, d("102")), (5, d("103")), (6, d("104"))] {
            book.insert_resting(resting(id, OrderVariant::Short, price, d("1")));
        }
        let (bids, asks) = book.depth(2);
        assert_eq!(bids, vec![(d("101"), d("1")), (d("100"), d("1"))]);
        assert_eq!(asks, vec![(d("102"), d("1")), (d("103"), d("1"))]);
    }

    #[test]
    fn impact_vwap_empty_book_returns_none() {
        let book = OrderBook::new();
        assert_eq!(book.bid_impact_vwap(d("1000")), None);
        assert_eq!(book.ask_impact_vwap(d("1000")), None);
    }

    #[test]
    fn impact_vwap_insufficient_liquidity_returns_none() {
        let mut book = OrderBook::new();
        book.insert_resting(resting(1, OrderVariant::Short, d("100"), d("1")));
        // Only 100 notional available on the ask side; asking for 1000 can't be filled.
        assert_eq!(book.ask_impact_vwap(d("1000")), None);
    }

    #[test]
    fn impact_vwap_partial_level_computes_correct_average() {
        let mut book = OrderBook::new();
        book.insert_resting(resting(1, OrderVariant::Short, d("100"), d("10")));
        book.insert_resting(resting(2, OrderVariant::Short, d("110"), d("10")));
        // Fill 1050 notional: all of level 100 (1000 notional, qty 10) plus 50/110 at level 110.
        let vwap = book.ask_impact_vwap(d("1050")).unwrap();
        let expected = d("1050") / (d("10") + d("50") / d("110"));
        assert_eq!(vwap, expected);
    }
}
