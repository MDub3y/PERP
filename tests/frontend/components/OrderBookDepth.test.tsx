import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { OrderBookDepth } from "@/components/trade/OrderBookDepth";

describe("OrderBookDepth", () => {
  it("renders a dash placeholder when depth is null", () => {
    render(<OrderBookDepth depth={null} />);
    expect(screen.getAllByText("—")).toHaveLength(2);
  });

  it("renders bid and ask price levels from a DepthMessage", () => {
    render(
      <OrderBookDepth
        depth={{
          bids: [
            [100, 5],
            [99, 3],
          ],
          asks: [
            [101, 2],
            [102, 4],
          ],
        }}
      />,
    );

    expect(screen.getByText("100.0000")).toBeInTheDocument();
    expect(screen.getByText("99.0000")).toBeInTheDocument();
    expect(screen.getByText("101.0000")).toBeInTheDocument();
    expect(screen.getByText("102.0000")).toBeInTheDocument();
  });
});
