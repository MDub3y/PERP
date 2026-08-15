import type { DepthMessage } from "@/lib/types";
import { formatNumber } from "@/lib/utils";

/** Depth ladder: top N bid/ask levels from the engine's market:{SYM}:depth
 * channel. Bids are rendered strongest-first (highest price), asks
 * weakest-first (lowest price / closest to mid), matching the order the
 * engine already publishes them in (see crates/engine/src/book.rs::depth). */
export function OrderBookDepth({ depth }: { depth: DepthMessage | null }) {
  const bids = depth?.bids ?? [];
  const asks = depth?.asks ?? [];
  const maxQty = Math.max(0, ...bids.map(([, q]) => q), ...asks.map(([, q]) => q));

  return (
    <div className="flex flex-col gap-2 rounded-lg border border-zinc-800 bg-zinc-900 p-4">
      <h2 className="text-xs font-medium text-zinc-500">Depth</h2>
      <div className="grid grid-cols-2 gap-3 font-mono text-xs">
        <div className="flex flex-col gap-0.5">
          {asks
            .slice()
            .reverse()
            .map(([price, qty]) => (
              <DepthRow key={`ask-${price}`} price={price} qty={qty} maxQty={maxQty} side="ask" />
            ))}
          {asks.length === 0 && <span className="text-zinc-600">—</span>}
        </div>
        <div className="flex flex-col gap-0.5">
          {bids.map(([price, qty]) => (
            <DepthRow key={`bid-${price}`} price={price} qty={qty} maxQty={maxQty} side="bid" />
          ))}
          {bids.length === 0 && <span className="text-zinc-600">—</span>}
        </div>
      </div>
    </div>
  );
}

function DepthRow({
  price,
  qty,
  maxQty,
  side,
}: {
  price: number;
  qty: number;
  maxQty: number;
  side: "bid" | "ask";
}) {
  const pct = maxQty > 0 ? Math.min(100, (qty / maxQty) * 100) : 0;
  return (
    <div className="relative flex items-center justify-between overflow-hidden rounded-sm px-1 py-0.5">
      <div
        className={`absolute inset-y-0 ${side === "bid" ? "right-0 bg-green-500/10" : "left-0 bg-red-500/10"}`}
        style={{ width: `${pct}%` }}
      />
      <span className={`relative ${side === "bid" ? "text-green-400" : "text-red-400"}`}>
        {formatNumber(price, 4)}
      </span>
      <span className="relative text-zinc-400">{formatNumber(qty, 4)}</span>
    </div>
  );
}
