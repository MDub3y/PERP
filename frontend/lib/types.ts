// Mirrors crates/store/src/models.rs. rust_decimal uses the `serde-float`
// feature workspace-wide, so Decimal fields serialize as JSON numbers.

export type MarketSymbol = "SOL" | "ETH" | "BTC";

export type OrderVariant = "LONG" | "SHORT";

export type OrderType = "LIMIT" | "MARKET";

export type TimeInForce = "GTC" | "IOC";

export type MarginMode = "CROSS" | "ISOLATED";

export type OrderStatus =
  | "PENDING"
  | "OPEN"
  | "PARTIALLY_FILLED"
  | "FILLED"
  | "CANCELLED"
  | "REJECTED";

export type WithdrawalStatus =
  | "QUEUED"
  | "SUBMITTING"
  | "SUBMITTED"
  | "CONFIRMED"
  | "FAILED"
  | "REFUNDED";

export interface User {
  id: number;
  username: string;
  collateral_available: number;
  collateral_locked: number;
  pubkey: string | null;
}

export interface Market {
  market: MarketSymbol;
  tick_size: number;
  lot_size: number;
  max_leverage: number;
  initial_margin_rate: number;
  maintenance_margin_rate: number;
  backstop_equity_ratio: number;
  liquidation_fee_rate: number;
  impact_notional: number;
  is_active: boolean;
  mark_price: number | null;
  mark_price_updated_at: string | null;
}

export interface Order {
  id: number;
  user_id: number;
  market: MarketSymbol;
  variant: OrderVariant;
  order_type: OrderType;
  tif: TimeInForce;
  reduce_only: boolean;
  leverage: number;
  margin_mode: MarginMode;
  price: number | null;
  quantity: number;
  remaining_qty: number;
  reserved_margin: number;
  status: OrderStatus;
  is_liquidation: boolean;
  created_at: string;
  updated_at: string;
}

export interface Position {
  id: number;
  user_id: number;
  market: MarketSymbol;
  variant: OrderVariant;
  margin_mode: MarginMode;
  leverage: number;
  quantity: number;
  average_price: number;
  allocated_margin: number;
  realized_pnl: number;
  funding_pnl: number;
  is_liquidating: boolean;
  liquidation_flagged_at: string | null;
  updated_at: string;
}

export interface WithdrawalRequest {
  id: number;
  user_id: number;
  amount: number;
  destination_pubkey: string;
  status: WithdrawalStatus;
  signature: string | null;
  nonce_account: string | null;
  nonce_hash: string | null;
  error_message: string | null;
  created_at: string;
  updated_at: string;
  submitted_at: string | null;
  confirmed_at: string | null;
}

export interface PlaceOrderPayload {
  market: MarketSymbol;
  variant: OrderVariant;
  order_type: OrderType;
  tif: TimeInForce;
  reduce_only: boolean;
  leverage: number;
  margin_mode: MarginMode;
  price: number | null;
  quantity: number;
}

export interface WithdrawalRequestPayload {
  amount: number;
  destination_pubkey: string;
}

// ---- WS gateway payloads (crates/engine/src/publish.rs) ----

export interface TradeMessage {
  price: number;
  quantity: number;
  taker_variant: OrderVariant;
  is_liquidation: boolean;
}

export interface BookTopMessage {
  best_bid: number | null;
  best_ask: number | null;
}

export interface TickerMessage {
  mark_price: number;
}

/** Top N price levels per side, each (price, total remaining quantity). */
export interface DepthMessage {
  bids: [number, number][];
  asks: [number, number][];
}

export interface UserOrderMessage {
  event_type: "ACCEPTED" | "RESTED" | "CANCELED" | "REJECTED" | "FILL";
  order_id: number;
  [key: string]: unknown;
}

export type MarketWsFrame =
  | { channel: "trades"; data: TradeMessage }
  | { channel: "book"; data: BookTopMessage }
  | { channel: "ticker"; data: TickerMessage }
  | { channel: "depth"; data: DepthMessage };

export type UserWsFrame = { channel: "orders"; data: UserOrderMessage };
