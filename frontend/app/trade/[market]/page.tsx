import { notFound } from "next/navigation";
import { AuthGuard } from "@/components/AuthGuard";
import { TradeView } from "@/components/trade/TradeView";
import type { MarketSymbol } from "@/lib/types";

const VALID_MARKETS: MarketSymbol[] = ["SOL", "ETH", "BTC"];

export default async function TradePage({
  params,
}: {
  params: Promise<{ market: string }>;
}) {
  const { market } = await params;
  const symbol = market.toUpperCase();

  if (!VALID_MARKETS.includes(symbol as MarketSymbol)) {
    notFound();
  }

  return (
    <AuthGuard>
      <TradeView market={symbol as MarketSymbol} />
    </AuthGuard>
  );
}
