import Link from "next/link";

export default function Home() {
  return (
    <div className="flex flex-1 flex-col items-center justify-center gap-6 px-4 text-center">
      <h1 className="text-4xl font-semibold tracking-tight">PERP</h1>
      <p className="max-w-md text-zinc-400">
        A centralized perpetuals exchange on Solana. Trade SOL, ETH, and BTC
        perpetuals with cross or isolated margin.
      </p>
      <div className="flex gap-3">
        <Link
          href="/trade/SOL"
          className="rounded-md bg-zinc-50 px-5 py-2.5 text-sm font-medium text-zinc-950 hover:bg-zinc-200"
        >
          Start trading
        </Link>
        <Link
          href="/signup"
          className="rounded-md border border-zinc-700 px-5 py-2.5 text-sm font-medium text-zinc-200 hover:bg-zinc-900"
        >
          Create account
        </Link>
      </div>
    </div>
  );
}
