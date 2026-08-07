"use client";

import { useEffect, useRef, useState } from "react";
import { useAuthStore } from "@/store/auth";
import type {
  BookTopMessage,
  MarketSymbol,
  MarketWsFrame,
  TickerMessage,
  TradeMessage,
  UserOrderMessage,
  UserWsFrame,
} from "@/lib/types";

export const WS_URL = process.env.NEXT_PUBLIC_WS_URL ?? "ws://localhost:3001";

const MAX_TRADES = 30;
const RECONNECT_DELAY_MS = 2000;

/** Live top-of-book, ticker, and recent-trades feed for one market. */
export function useMarketSocket(market: MarketSymbol) {
  const [bookTop, setBookTop] = useState<BookTopMessage | null>(null);
  const [ticker, setTicker] = useState<TickerMessage | null>(null);
  const [trades, setTrades] = useState<TradeMessage[]>([]);
  const [connected, setConnected] = useState(false);

  // Reset feed state when the market changes, without doing it as a
  // setState-in-effect side effect (react-hooks/set-state-in-effect) - this
  // is React's documented "adjust state during render" pattern instead.
  const [prevMarket, setPrevMarket] = useState(market);
  if (market !== prevMarket) {
    setPrevMarket(market);
    setBookTop(null);
    setTicker(null);
    setTrades([]);
  }

  useEffect(() => {
    let socket: WebSocket | null = null;
    let reconnectTimer: ReturnType<typeof setTimeout> | null = null;
    let cancelled = false;

    function connect() {
      if (cancelled) return;
      socket = new WebSocket(`${WS_URL}/ws/market/${market}`);

      socket.onopen = () => setConnected(true);
      socket.onclose = () => {
        setConnected(false);
        if (!cancelled) {
          reconnectTimer = setTimeout(connect, RECONNECT_DELAY_MS);
        }
      };
      socket.onerror = () => socket?.close();
      socket.onmessage = (event) => {
        let frame: MarketWsFrame;
        try {
          frame = JSON.parse(event.data);
        } catch {
          return;
        }
        if (frame.channel === "book") setBookTop(frame.data);
        else if (frame.channel === "ticker") setTicker(frame.data);
        else if (frame.channel === "trades") {
          setTrades((prev) => [frame.data, ...prev].slice(0, MAX_TRADES));
        }
      };
    }

    connect();

    return () => {
      cancelled = true;
      if (reconnectTimer) clearTimeout(reconnectTimer);
      socket?.close();
    };
  }, [market]);

  return { bookTop, ticker, trades, connected };
}

/** Live per-user order lifecycle feed (ACCEPTED/RESTED/CANCELED/REJECTED/FILL). */
export function useUserSocket(onEvent?: (event: UserOrderMessage) => void) {
  const token = useAuthStore((s) => s.token);
  const [connected, setConnected] = useState(false);
  const onEventRef = useRef(onEvent);
  useEffect(() => {
    onEventRef.current = onEvent;
  }, [onEvent]);

  useEffect(() => {
    if (!token) return;

    let socket: WebSocket | null = null;
    let reconnectTimer: ReturnType<typeof setTimeout> | null = null;
    let cancelled = false;

    function connect() {
      if (cancelled || !token) return;
      socket = new WebSocket(`${WS_URL}/ws/user?token=${encodeURIComponent(token)}`);

      socket.onopen = () => setConnected(true);
      socket.onclose = () => {
        setConnected(false);
        if (!cancelled) {
          reconnectTimer = setTimeout(connect, RECONNECT_DELAY_MS);
        }
      };
      socket.onerror = () => socket?.close();
      socket.onmessage = (event) => {
        let frame: UserWsFrame;
        try {
          frame = JSON.parse(event.data);
        } catch {
          return;
        }
        if (frame.channel === "orders") {
          onEventRef.current?.(frame.data);
        }
      };
    }

    connect();

    return () => {
      cancelled = true;
      if (reconnectTimer) clearTimeout(reconnectTimer);
      socket?.close();
    };
  }, [token]);

  return { connected };
}
