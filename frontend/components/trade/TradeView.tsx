"use client";

import { useQuery, useQueryClient } from "@tanstack/react-query";
import Link from "next/link";
import { cn } from "@/lib/utils";
import { getMarkets } from "@/lib/api";
import { useMarketSocket, useUserSocket } from "@/lib/ws";
import type { MarketSymbol, UserOrderMessage } from "@/lib/types";
import { PriceChart } from "@/components/trade/PriceChart";
import { OrderBookTop } from "@/components/trade/OrderBookTop";
import { RecentTrades } from "@/components/trade/RecentTrades";
import { OrderForm } from "@/components/trade/OrderForm";
import { OpenOrders } from "@/components/trade/OpenOrders";
import { Positions } from "@/components/trade/Positions";
import { formatNumber } from "@/lib/utils";
import { toast } from "sonner";

const ALL_MARKETS: MarketSymbol[] = ["SOL", "ETH", "BTC"];

const EVENT_LABEL: Record<UserOrderMessage["event_type"], string> = {
  ACCEPTED: "Order accepted",
  RESTED: "Order resting on book",
  CANCELED: "Order cancelled",
  REJECTED: "Order rejected",
  FILL: "Order filled",
};

export function TradeView({ market }: { market: MarketSymbol }) {
  const queryClient = useQueryClient();
  const { data: markets = [] } = useQuery({ queryKey: ["markets"], queryFn: getMarkets });
  const marketConfig = markets.find((m) => m.market === market);

  const { bookTop, ticker, trades } = useMarketSocket(market);

  useUserSocket((event) => {
    const label = EVENT_LABEL[event.event_type] ?? event.event_type;
    if (event.event_type === "REJECTED") {
      toast.error(label);
    } else if (event.event_type === "FILL") {
      toast.success(label);
    } else {
      toast.info(label);
    }
    queryClient.invalidateQueries({ queryKey: ["orders"] });
    queryClient.invalidateQueries({ queryKey: ["positions"] });
    queryClient.invalidateQueries({ queryKey: ["me"] });
  });

  const markPrice = ticker?.mark_price ?? marketConfig?.mark_price ?? null;

  return (
    <div className="mx-auto flex w-full max-w-7xl flex-col gap-4 p-4">
      <div className="flex items-center gap-4">
        <div className="flex gap-1">
          {ALL_MARKETS.map((m) => (
            <Link
              key={m}
              href={`/trade/${m}`}
              className={cn(
                "rounded-md px-3 py-1.5 text-sm font-medium",
                m === market ? "bg-zinc-800 text-zinc-50" : "text-zinc-500 hover:text-zinc-300",
              )}
            >
              {m}-PERP
            </Link>
          ))}
        </div>
        <div className="font-mono text-lg text-zinc-100">
          {markPrice != null ? `$${formatNumber(markPrice, 4)}` : "—"}
        </div>
        {marketConfig && (
          <div className="text-xs text-zinc-500">
            Max leverage {marketConfig.max_leverage}x &middot; Tick {marketConfig.tick_size}{" "}
            &middot; Lot {marketConfig.lot_size}
          </div>
        )}
      </div>

      <div className="grid grid-cols-1 gap-4 lg:grid-cols-[1fr_280px_320px]">
        <div className="flex h-[420px] flex-col rounded-lg border border-zinc-800 bg-zinc-900 p-4">
          <PriceChart price={markPrice} />
        </div>

        <div className="flex flex-col gap-4">
          <OrderBookTop bookTop={bookTop} />
          <RecentTrades trades={trades} />
        </div>

        <OrderForm market={marketConfig} marketSymbol={market} markPrice={markPrice} />
      </div>

      <div className="grid grid-cols-1 gap-4 lg:grid-cols-2">
        <div className="rounded-lg border border-zinc-800 bg-zinc-900 p-4">
          <h2 className="mb-3 text-xs font-medium text-zinc-500">Open orders</h2>
          <OpenOrders />
        </div>
        <div className="rounded-lg border border-zinc-800 bg-zinc-900 p-4">
          <h2 className="mb-3 text-xs font-medium text-zinc-500">Positions</h2>
          <Positions />
        </div>
      </div>
    </div>
  );
}
