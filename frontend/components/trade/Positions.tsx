"use client";

import { useQuery } from "@tanstack/react-query";
import { listPositions } from "@/lib/api";
import { cn, formatNumber } from "@/lib/utils";

export function Positions() {
  const { data: positions = [] } = useQuery({
    queryKey: ["positions"],
    queryFn: listPositions,
    refetchInterval: 5_000,
  });

  if (positions.length === 0) {
    return <p className="py-6 text-center text-xs text-zinc-600">No open positions</p>;
  }

  return (
    <div className="overflow-x-auto">
      <table className="w-full text-left text-xs">
        <thead>
          <tr className="text-zinc-500">
            <th className="pb-2 font-medium">Market</th>
            <th className="pb-2 font-medium">Side</th>
            <th className="pb-2 font-medium">Size</th>
            <th className="pb-2 font-medium">Entry</th>
            <th className="pb-2 font-medium">Leverage</th>
            <th className="pb-2 font-medium">Margin</th>
            <th className="pb-2 font-medium">PnL</th>
          </tr>
        </thead>
        <tbody>
          {positions.map((position) => (
            <tr key={position.id} className="border-t border-zinc-800/50">
              <td className="py-2 font-mono">{position.market}</td>
              <td
                className={cn(
                  "py-2 font-medium",
                  position.variant === "LONG" ? "text-green-400" : "text-red-400",
                )}
              >
                {position.variant}
              </td>
              <td className="py-2 font-mono">{formatNumber(position.quantity, 4)}</td>
              <td className="py-2 font-mono">{formatNumber(position.average_price, 4)}</td>
              <td className="py-2 font-mono">{position.leverage}x</td>
              <td className="py-2 font-mono">{formatNumber(position.allocated_margin, 2)}</td>
              <td
                className={cn(
                  "py-2 font-mono",
                  position.realized_pnl + position.funding_pnl >= 0
                    ? "text-green-400"
                    : "text-red-400",
                )}
              >
                {formatNumber(position.realized_pnl + position.funding_pnl, 2)}
                {position.is_liquidating && (
                  <span className="ml-2 rounded bg-amber-500/20 px-1 text-[10px] text-amber-400">
                    LIQUIDATING
                  </span>
                )}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
