import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { OrderBookTop } from "@/components/trade/OrderBookTop";

describe("OrderBookTop", () => {
  it("renders em-dashes when there is no book data yet", () => {
    render(<OrderBookTop bookTop={null} />);
    expect(screen.getAllByText("—")).toHaveLength(2);
  });

  it("renders best bid and ask when present", () => {
    render(<OrderBookTop bookTop={{ best_bid: 99.5, best_ask: 100.25 }} />);
    expect(screen.getByText("99.5000")).toBeInTheDocument();
    expect(screen.getByText("100.2500")).toBeInTheDocument();
  });

  it("renders a dash for a null side even when the other side has a price", () => {
    render(<OrderBookTop bookTop={{ best_bid: 99.5, best_ask: null }} />);
    expect(screen.getByText("99.5000")).toBeInTheDocument();
    expect(screen.getAllByText("—")).toHaveLength(1);
  });
});
