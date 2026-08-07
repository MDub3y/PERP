"use client";

import { zodResolver } from "@hookform/resolvers/zod";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useForm } from "react-hook-form";
import { toast } from "sonner";
import { z } from "zod";
import { apiErrorMessage, requestWithdrawal } from "@/lib/api";

const BASE58_PUBKEY = /^[1-9A-HJ-NP-Za-km-z]{32,44}$/;

const schema = z.object({
  amount: z.number().positive("Amount must be positive"),
  destination_pubkey: z
    .string()
    .regex(BASE58_PUBKEY, "Must be a valid base58 Solana address"),
});

type FormValues = z.infer<typeof schema>;

export function WithdrawForm() {
  const queryClient = useQueryClient();

  const {
    register,
    handleSubmit,
    reset,
    formState: { errors },
  } = useForm<FormValues>({ resolver: zodResolver(schema) });

  const mutation = useMutation({
    mutationFn: requestWithdrawal,
    onSuccess: () => {
      toast.success("Withdrawal queued");
      queryClient.invalidateQueries({ queryKey: ["withdrawals"] });
      queryClient.invalidateQueries({ queryKey: ["me"] });
      reset();
    },
    onError: (err) => toast.error(apiErrorMessage(err, "Withdrawal failed")),
  });

  return (
    <form
      onSubmit={handleSubmit((values) => mutation.mutate(values))}
      className="flex flex-col gap-4 rounded-lg border border-zinc-800 bg-zinc-900 p-4"
    >
      <h2 className="text-sm font-semibold">Request withdrawal</h2>
      <div>
        <label className="mb-1 block text-xs text-zinc-400">Destination address</label>
        <input
          {...register("destination_pubkey")}
          placeholder="Solana public key"
          className="w-full rounded-md border border-zinc-700 bg-zinc-950 px-3 py-2 font-mono text-sm outline-none focus:border-zinc-500"
        />
        {errors.destination_pubkey && (
          <p className="mt-1 text-xs text-red-400">{errors.destination_pubkey.message}</p>
        )}
      </div>
      <div>
        <label className="mb-1 block text-xs text-zinc-400">Amount (USD)</label>
        <input
          type="number"
          step="any"
          {...register("amount", { valueAsNumber: true })}
          className="w-full rounded-md border border-zinc-700 bg-zinc-950 px-3 py-2 text-sm outline-none focus:border-zinc-500"
        />
        {errors.amount && <p className="mt-1 text-xs text-red-400">{errors.amount.message}</p>}
      </div>
      <button
        type="submit"
        disabled={mutation.isPending}
        className="rounded-md bg-zinc-50 px-4 py-2.5 text-sm font-semibold text-zinc-950 hover:bg-zinc-200 disabled:opacity-50"
      >
        {mutation.isPending ? "Submitting..." : "Withdraw"}
      </button>
    </form>
  );
}
