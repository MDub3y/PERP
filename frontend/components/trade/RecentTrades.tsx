import type { TradeMessage } from "@/lib/types";
import { cn, formatNumber } from "@/lib/utils";

export function RecentTrades({ trades }: { trades: TradeMessage[] }) {
  return (
    <div className="flex flex-col gap-2 rounded-lg border border-zinc-800 bg-zinc-900 p-4">
      <h2 className="text-xs font-medium text-zinc-500">Recent trades</h2>
      <div className="flex flex-col overflow-y-auto" style={{ maxHeight: 260 }}>
        {trades.length === 0 && (
          <p className="py-4 text-center text-xs text-zinc-600">No trades yet</p>
        )}
        {trades.map((trade, i) => (
          <div
            key={i}
            className="flex items-center justify-between border-b border-zinc-800/50 py-1 text-xs last:border-0"
          >
            <span
              className={cn(
                "font-mono",
                trade.taker_variant === "LONG" ? "text-green-400" : "text-red-400",
              )}
            >
              {formatNumber(trade.price, 4)}
            </span>
            <span className="font-mono text-zinc-400">{formatNumber(trade.quantity, 4)}</span>
            {trade.is_liquidation && (
              <span className="rounded bg-amber-500/20 px-1 text-[10px] text-amber-400">
                LIQ
              </span>
            )}
          </div>
        ))}
      </div>
    </div>
  );
}
