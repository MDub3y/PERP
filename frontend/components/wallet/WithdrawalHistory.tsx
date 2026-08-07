"use client";

import { useQuery } from "@tanstack/react-query";
import { format } from "date-fns";
import { listWithdrawals } from "@/lib/api";
import type { WithdrawalStatus } from "@/lib/types";
import { cn, formatNumber } from "@/lib/utils";

const STATUS_STYLES: Record<WithdrawalStatus, string> = {
  QUEUED: "bg-zinc-500/20 text-zinc-300",
  SUBMITTING: "bg-amber-500/20 text-amber-400",
  SUBMITTED: "bg-amber-500/20 text-amber-400",
  CONFIRMED: "bg-green-500/20 text-green-400",
  FAILED: "bg-red-500/20 text-red-400",
  REFUNDED: "bg-blue-500/20 text-blue-400",
};

export function WithdrawalHistory() {
  const { data: withdrawals = [] } = useQuery({
    queryKey: ["withdrawals"],
    queryFn: listWithdrawals,
    refetchInterval: 5_000,
  });

  if (withdrawals.length === 0) {
    return (
      <div className="rounded-lg border border-zinc-800 bg-zinc-900 p-4">
        <h2 className="mb-3 text-sm font-semibold">Withdrawal history</h2>
        <p className="py-6 text-center text-xs text-zinc-600">No withdrawals yet</p>
      </div>
    );
  }

  return (
    <div className="rounded-lg border border-zinc-800 bg-zinc-900 p-4">
      <h2 className="mb-3 text-sm font-semibold">Withdrawal history</h2>
      <div className="overflow-x-auto">
        <table className="w-full text-left text-xs">
          <thead>
            <tr className="text-zinc-500">
              <th className="pb-2 font-medium">Amount</th>
              <th className="pb-2 font-medium">Destination</th>
              <th className="pb-2 font-medium">Status</th>
              <th className="pb-2 font-medium">Requested</th>
            </tr>
          </thead>
          <tbody>
            {withdrawals.map((w) => (
              <tr key={w.id} className="border-t border-zinc-800/50">
                <td className="py-2 font-mono">${formatNumber(w.amount, 2)}</td>
                <td className="py-2 font-mono text-zinc-400">
                  {w.destination_pubkey.slice(0, 4)}...{w.destination_pubkey.slice(-4)}
                </td>
                <td className="py-2">
                  <span className={cn("rounded px-1.5 py-0.5 font-medium", STATUS_STYLES[w.status])}>
                    {w.status}
                  </span>
                </td>
                <td className="py-2 text-zinc-500">
                  {format(new Date(w.created_at), "MMM d, HH:mm")}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  );
}
