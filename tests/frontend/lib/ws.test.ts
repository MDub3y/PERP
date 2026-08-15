import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useMarketSocket } from "@/lib/ws";

class MockWebSocket {
  static instances: MockWebSocket[] = [];
  onopen: (() => void) | null = null;
  onclose: (() => void) | null = null;
  onerror: (() => void) | null = null;
  onmessage: ((event: { data: string }) => void) | null = null;
  url: string;

  constructor(url: string) {
    this.url = url;
    MockWebSocket.instances.push(this);
  }

  close() {
    this.onclose?.();
  }

  emit(frame: unknown) {
    this.onmessage?.({ data: JSON.stringify(frame) });
  }
}

describe("useMarketSocket", () => {
  beforeEach(() => {
    MockWebSocket.instances = [];
    // @ts-expect-error - test double, not a full WebSocket implementation
    vi.stubGlobal("WebSocket", MockWebSocket);
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("dispatches a depth frame into the depth state slot", () => {
    const { result } = renderHook(() => useMarketSocket("SOL"));
    expect(result.current.depth).toBeNull();

    const socket = MockWebSocket.instances[0];
    act(() => {
      socket.emit({
        channel: "depth",
        data: { bids: [[100, 5]], asks: [[101, 3]] },
      });
    });

    expect(result.current.depth).toEqual({ bids: [[100, 5]], asks: [[101, 3]] });
  });

  it("still dispatches book/ticker/trades frames alongside depth", () => {
    const { result } = renderHook(() => useMarketSocket("SOL"));
    const socket = MockWebSocket.instances[0];

    act(() => {
      socket.emit({ channel: "book", data: { best_bid: 99, best_ask: 101 } });
      socket.emit({ channel: "ticker", data: { mark_price: 100 } });
      socket.emit({
        channel: "trades",
        data: { price: 100, quantity: 1, taker_variant: "LONG", is_liquidation: false },
      });
    });

    expect(result.current.bookTop).toEqual({ best_bid: 99, best_ask: 101 });
    expect(result.current.ticker).toEqual({ mark_price: 100 });
    expect(result.current.trades).toHaveLength(1);
  });

  it("resets depth (and other feed state) when the market changes", () => {
    const { result, rerender } = renderHook(({ market }) => useMarketSocket(market), {
      initialProps: { market: "SOL" as const },
    });

    act(() => {
      MockWebSocket.instances[0].emit({
        channel: "depth",
        data: { bids: [[100, 5]], asks: [] },
      });
    });
    expect(result.current.depth).not.toBeNull();

    rerender({ market: "ETH" });
    expect(result.current.depth).toBeNull();
  });
});
