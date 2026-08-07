import type { BookTopMessage } from "@/lib/types";
import { formatNumber } from "@/lib/utils";

export function OrderBookTop({ bookTop }: { bookTop: BookTopMessage | null }) {
  return (
    <div className="flex flex-col gap-2 rounded-lg border border-zinc-800 bg-zinc-900 p-4">
      <h2 className="text-xs font-medium text-zinc-500">Top of book</h2>
      <div className="flex items-center justify-between">
        <span className="text-xs text-zinc-500">Best bid</span>
        <span className="font-mono text-sm text-green-400">
          {bookTop?.best_bid != null ? formatNumber(bookTop.best_bid, 4) : "—"}
        </span>
      </div>
      <div className="flex items-center justify-between">
        <span className="text-xs text-zinc-500">Best ask</span>
        <span className="font-mono text-sm text-red-400">
          {bookTop?.best_ask != null ? formatNumber(bookTop.best_ask, 4) : "—"}
        </span>
      </div>
    </div>
  );
}
