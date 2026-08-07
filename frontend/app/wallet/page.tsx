import { AuthGuard } from "@/components/AuthGuard";
import { WithdrawForm } from "@/components/wallet/WithdrawForm";
import { WithdrawalHistory } from "@/components/wallet/WithdrawalHistory";

export default function WalletPage() {
  return (
    <AuthGuard>
      <div className="mx-auto flex w-full max-w-3xl flex-col gap-4 p-4">
        <h1 className="text-lg font-semibold">Wallet</h1>
        <WithdrawForm />
        <WithdrawalHistory />
      </div>
    </AuthGuard>
  );
}
