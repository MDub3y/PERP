"use client";

import { zodResolver } from "@hookform/resolvers/zod";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import { useForm } from "react-hook-form";
import { toast } from "sonner";
import { z } from "zod";
import { apiErrorMessage, placeOrder } from "@/lib/api";
import type {
  Market,
  MarginMode,
  MarketSymbol,
  OrderType,
  OrderVariant,
  TimeInForce,
} from "@/lib/types";
import { cn } from "@/lib/utils";

function buildSchema(market: Market | undefined, orderType: OrderType) {
  return z.object({
    quantity: z
      .number()
      .positive("Quantity must be positive")
      .refine(
        (v) => !market || Math.abs(v / market.lot_size - Math.round(v / market.lot_size)) < 1e-8,
        market ? `Must be a multiple of ${market.lot_size}` : "Invalid lot size",
      ),
    price:
      orderType === "LIMIT"
        ? z
            .number()
            .positive("Price must be positive")
            .refine(
              (v) =>
                !market || Math.abs(v / market.tick_size - Math.round(v / market.tick_size)) < 1e-6,
              market ? `Must be a multiple of ${market.tick_size}` : "Invalid tick size",
            )
        : z.number().nullable().optional(),
    leverage: z.number().int("Leverage must be a whole number").min(1).max(market?.max_leverage ?? 100),
  });
}

export function OrderForm({
  market,
  marketSymbol,
  markPrice,
}: {
  market: Market | undefined;
  marketSymbol: MarketSymbol;
  markPrice: number | null;
}) {
  const queryClient = useQueryClient();
  const [orderType, setOrderType] = useState<OrderType>("LIMIT");
  const [variant, setVariant] = useState<OrderVariant>("LONG");
  const [tif, setTif] = useState<TimeInForce>("GTC");
  const [marginMode, setMarginMode] = useState<MarginMode>("CROSS");
  const [reduceOnly, setReduceOnly] = useState(false);

  const schema = buildSchema(market, orderType);
  type FormValues = z.infer<typeof schema>;

  const {
    register,
    handleSubmit,
    reset,
    formState: { errors },
  } = useForm<FormValues>({
    resolver: zodResolver(schema),
    defaultValues: { leverage: 1 },
  });

  useEffect(() => {
    reset({ leverage: 1 });
  }, [marketSymbol, reset]);

  const mutation = useMutation({
    mutationFn: (values: FormValues) =>
      placeOrder({
        market: marketSymbol,
        variant,
        order_type: orderType,
        tif: orderType === "MARKET" ? "IOC" : tif,
        reduce_only: reduceOnly,
        leverage: values.leverage,
        margin_mode: marginMode,
        price: orderType === "LIMIT" ? (values.price as number) : null,
        quantity: values.quantity,
      }),
    onSuccess: () => {
      toast.success("Order submitted");
      queryClient.invalidateQueries({ queryKey: ["orders"] });
      queryClient.invalidateQueries({ queryKey: ["positions"] });
      queryClient.invalidateQueries({ queryKey: ["me"] });
      reset({ leverage: 1 });
    },
    onError: (err) => {
      toast.error(apiErrorMessage(err, "Order rejected"));
    },
  });

  return (
    <div className="flex flex-col gap-4 rounded-lg border border-zinc-800 bg-zinc-900 p-4">
      <div className="grid grid-cols-2 gap-1 rounded-md bg-zinc-950 p-1">
        {(["LONG", "SHORT"] as const).map((v) => (
          <button
            key={v}
            type="button"
            onClick={() => setVariant(v)}
            className={cn(
              "rounded px-3 py-1.5 text-sm font-semibold transition-colors",
              variant === v
                ? v === "LONG"
                  ? "bg-green-500/20 text-green-400"
                  : "bg-red-500/20 text-red-400"
                : "text-zinc-500 hover:text-zinc-300",
            )}
          >
            {v === "LONG" ? "Long" : "Short"}
          </button>
        ))}
      </div>

      <div className="flex gap-1 text-xs">
        {(["LIMIT", "MARKET"] as const).map((t) => (
          <button
            key={t}
            type="button"
            onClick={() => setOrderType(t)}
            className={cn(
              "rounded px-2 py-1 font-medium",
              orderType === t ? "bg-zinc-800 text-zinc-50" : "text-zinc-500 hover:text-zinc-300",
            )}
          >
            {t}
          </button>
        ))}
      </div>

      <form
        onSubmit={handleSubmit((values) => mutation.mutate(values))}
        className="flex flex-col gap-3"
      >
        {orderType === "LIMIT" && (
          <div>
            <label className="mb-1 block text-xs text-zinc-400">Price (USD)</label>
            <input
              type="number"
              step="any"
              defaultValue={markPrice ?? undefined}
              {...register("price", { valueAsNumber: true })}
              className="w-full rounded-md border border-zinc-700 bg-zinc-950 px-3 py-2 text-sm outline-none focus:border-zinc-500"
            />
            {errors.price && (
              <p className="mt-1 text-xs text-red-400">{errors.price.message}</p>
            )}
          </div>
        )}

        <div>
          <label className="mb-1 block text-xs text-zinc-400">Quantity</label>
          <input
            type="number"
            step="any"
            {...register("quantity", { valueAsNumber: true })}
            className="w-full rounded-md border border-zinc-700 bg-zinc-950 px-3 py-2 text-sm outline-none focus:border-zinc-500"
          />
          {errors.quantity && (
            <p className="mt-1 text-xs text-red-400">{errors.quantity.message}</p>
          )}
        </div>

        <div>
          <label className="mb-1 block text-xs text-zinc-400">
            Leverage (max {market?.max_leverage ?? "—"}x)
          </label>
          <input
            type="number"
            step="1"
            {...register("leverage", { valueAsNumber: true })}
            className="w-full rounded-md border border-zinc-700 bg-zinc-950 px-3 py-2 text-sm outline-none focus:border-zinc-500"
          />
          {errors.leverage && (
            <p className="mt-1 text-xs text-red-400">{errors.leverage.message}</p>
          )}
        </div>

        <div className="flex items-center gap-4 text-xs text-zinc-400">
          {orderType === "LIMIT" && (
            <label className="flex items-center gap-1.5">
              <span>TIF</span>
              <select
                value={tif}
                onChange={(e) => setTif(e.target.value as TimeInForce)}
                className="rounded border border-zinc-700 bg-zinc-950 px-1.5 py-1 text-zinc-200"
              >
                <option value="GTC">GTC</option>
                <option value="IOC">IOC</option>
              </select>
            </label>
          )}
          <label className="flex items-center gap-1.5">
            <span>Margin</span>
            <select
              value={marginMode}
              onChange={(e) => setMarginMode(e.target.value as MarginMode)}
              className="rounded border border-zinc-700 bg-zinc-950 px-1.5 py-1 text-zinc-200"
            >
              <option value="CROSS">Cross</option>
              <option value="ISOLATED">Isolated</option>
            </select>
          </label>
          <label className="flex items-center gap-1.5">
            <input
              type="checkbox"
              checked={reduceOnly}
              onChange={(e) => setReduceOnly(e.target.checked)}
              className="accent-zinc-50"
            />
            Reduce only
          </label>
        </div>

        <button
          type="submit"
          disabled={mutation.isPending}
          className={cn(
            "mt-1 rounded-md px-4 py-2.5 text-sm font-semibold transition-colors disabled:opacity-50",
            variant === "LONG"
              ? "bg-green-500 text-zinc-950 hover:bg-green-400"
              : "bg-red-500 text-zinc-950 hover:bg-red-400",
          )}
        >
          {mutation.isPending
            ? "Submitting..."
            : `${variant === "LONG" ? "Buy / Long" : "Sell / Short"} ${marketSymbol}`}
        </button>
      </form>
    </div>
  );
}
