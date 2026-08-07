"use client";

import Link from "next/link";
import { usePathname, useRouter } from "next/navigation";
import { useQuery } from "@tanstack/react-query";
import { LogOut, Wallet } from "lucide-react";
import { getMe } from "@/lib/api";
import { useAuthStore } from "@/store/auth";
import { cn, formatNumber } from "@/lib/utils";

const MARKETS = ["SOL", "ETH", "BTC"] as const;

export function Navbar() {
  const pathname = usePathname();
  const router = useRouter();
  const token = useAuthStore((s) => s.token);
  const setUser = useAuthStore((s) => s.setUser);
  const logout = useAuthStore((s) => s.logout);

  const { data: me } = useQuery({
    queryKey: ["me"],
    queryFn: async () => {
      const user = await getMe();
      setUser(user);
      return user;
    },
    enabled: !!token,
    refetchInterval: 10_000,
  });

  function handleLogout() {
    logout();
    router.push("/login");
  }

  return (
    <header className="sticky top-0 z-20 border-b border-zinc-800 bg-zinc-950/95 backdrop-blur">
      <div className="mx-auto flex h-14 max-w-7xl items-center gap-6 px-4">
        <Link href="/" className="text-sm font-semibold tracking-tight text-zinc-50">
          PERP
        </Link>

        {token && (
          <nav className="flex items-center gap-1">
            {MARKETS.map((market) => (
              <Link
                key={market}
                href={`/trade/${market}`}
                className={cn(
                  "rounded-md px-3 py-1.5 text-sm font-medium text-zinc-400 transition-colors hover:text-zinc-50",
                  pathname === `/trade/${market}` && "bg-zinc-800 text-zinc-50",
                )}
              >
                {market}-PERP
              </Link>
            ))}
          </nav>
        )}

        <div className="ml-auto flex items-center gap-4">
          {token && me ? (
            <>
              <div className="hidden text-right text-xs leading-tight sm:block">
                <div className="text-zinc-500">Available</div>
                <div className="font-mono text-zinc-100">
                  ${formatNumber(me.collateral_available)}
                </div>
              </div>
              <div className="hidden text-right text-xs leading-tight sm:block">
                <div className="text-zinc-500">Locked</div>
                <div className="font-mono text-zinc-100">
                  ${formatNumber(me.collateral_locked)}
                </div>
              </div>
              <Link
                href="/wallet"
                className={cn(
                  "flex items-center gap-1.5 rounded-md px-3 py-1.5 text-sm font-medium text-zinc-400 transition-colors hover:text-zinc-50",
                  pathname === "/wallet" && "bg-zinc-800 text-zinc-50",
                )}
              >
                <Wallet className="h-4 w-4" />
                Wallet
              </Link>
              <button
                onClick={handleLogout}
                className="flex items-center gap-1.5 rounded-md px-3 py-1.5 text-sm font-medium text-zinc-400 transition-colors hover:text-zinc-50"
              >
                <LogOut className="h-4 w-4" />
                Log out
              </button>
            </>
          ) : (
            <>
              <Link
                href="/login"
                className="rounded-md px-3 py-1.5 text-sm font-medium text-zinc-400 hover:text-zinc-50"
              >
                Log in
              </Link>
              <Link
                href="/signup"
                className="rounded-md bg-zinc-50 px-3 py-1.5 text-sm font-medium text-zinc-950 hover:bg-zinc-200"
              >
                Sign up
              </Link>
            </>
          )}
        </div>
      </div>
    </header>
  );
}
