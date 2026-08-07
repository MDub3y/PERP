"use client";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { apiErrorMessage, cancelOrder, listOrders } from "@/lib/api";
import { cn, formatNumber } from "@/lib/utils";

export function OpenOrders() {
  const queryClient = useQueryClient();

  const { data: orders = [] } = useQuery({
    queryKey: ["orders"],
    queryFn: listOrders,
    refetchInterval: 5_000,
  });

  const cancelMutation = useMutation({
    mutationFn: cancelOrder,
    onSuccess: () => {
      toast.success("Order cancelled");
      queryClient.invalidateQueries({ queryKey: ["orders"] });
      queryClient.invalidateQueries({ queryKey: ["me"] });
    },
    onError: (err) => toast.error(apiErrorMessage(err, "Could not cancel order")),
  });

  if (orders.length === 0) {
    return <p className="py-6 text-center text-xs text-zinc-600">No open orders</p>;
  }

  return (
    <div className="overflow-x-auto">
      <table className="w-full text-left text-xs">
        <thead>
          <tr className="text-zinc-500">
            <th className="pb-2 font-medium">Market</th>
            <th className="pb-2 font-medium">Side</th>
            <th className="pb-2 font-medium">Type</th>
            <th className="pb-2 font-medium">Price</th>
            <th className="pb-2 font-medium">Remaining</th>
            <th className="pb-2 font-medium">Status</th>
            <th className="pb-2 font-medium"></th>
          </tr>
        </thead>
        <tbody>
          {orders.map((order) => (
            <tr key={order.id} className="border-t border-zinc-800/50">
              <td className="py-2 font-mono">{order.market}</td>
              <td
                className={cn(
                  "py-2 font-medium",
                  order.variant === "LONG" ? "text-green-400" : "text-red-400",
                )}
              >
                {order.variant}
              </td>
              <td className="py-2 text-zinc-400">{order.order_type}</td>
              <td className="py-2 font-mono">
                {order.price != null ? formatNumber(order.price, 4) : "Market"}
              </td>
              <td className="py-2 font-mono">{formatNumber(order.remaining_qty, 4)}</td>
              <td className="py-2 text-zinc-400">{order.status}</td>
              <td className="py-2 text-right">
                {!order.status.includes("FILLED") && order.status !== "CANCELLED" && (
                  <button
                    onClick={() => cancelMutation.mutate(order.id)}
                    disabled={cancelMutation.isPending}
                    className="rounded border border-zinc-700 px-2 py-1 text-zinc-300 hover:bg-zinc-800 disabled:opacity-50"
                  >
                    Cancel
                  </button>
                )}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
