use rust_decimal::Decimal;
use rust_decimal::prelude::FromPrimitive;
use sqlx::postgres::PgPoolOptions;

struct MarketConfig {
    market: &'static str,
    tick_size: &'static str,
    lot_size: &'static str,
    max_leverage: i16,
    /// Pyth Hermes price-feed id (hex, no "0x" prefix) - see
    /// crates/engine/src/oracle.rs, which polls Hermes for these ids to
    /// feed the funding sampler's external index price.
    pyth_price_feed_id: &'static str,
}

/// Initial/maintenance margin rates are derived from max_leverage
/// (initial = 1/leverage, maintenance = half of initial) rather than
/// hand-picked per market, so every market is internally consistent.
const MARKETS: &[MarketConfig] = &[
    MarketConfig {
        market: "SOL",
        tick_size: "0.01",
        lot_size: "0.01",
        max_leverage: 20,
        pyth_price_feed_id: "ef0d8b6fda2ceba41da15d4095d1da392a0d2f8ed0c6c7bc0f4cfac8c280b56d",
    },
    MarketConfig {
        market: "ETH",
        tick_size: "0.01",
        lot_size: "0.001",
        max_leverage: 20,
        pyth_price_feed_id: "ff61491a931112ddf1bd8147cd1b641375f79f5825126d665480874634fd0ace",
    },
    MarketConfig {
        market: "BTC",
        tick_size: "0.1",
        lot_size: "0.0001",
        max_leverage: 20,
        pyth_price_feed_id: "e62df6c8b4a85fe1a67db44dc12de5db330f7ac66b72dc658afedf0f4a415b43",
    },
];

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenvy::dotenv().ok();
    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://postgres:password@localhost:5432/perp_exchange".into());

    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&database_url)
        .await?;

    for m in MARKETS {
        let initial_margin_rate = Decimal::ONE / Decimal::from_i16(m.max_leverage).unwrap();
        let maintenance_margin_rate = initial_margin_rate / Decimal::from(2);

        sqlx::query(
            "INSERT INTO markets (market, tick_size, lot_size, max_leverage, initial_margin_rate, maintenance_margin_rate, pyth_price_feed_id)
             VALUES ($1::text::market_name, $2, $3, $4, $5, $6, $7)
             ON CONFLICT (market) DO UPDATE SET
                tick_size = EXCLUDED.tick_size,
                lot_size = EXCLUDED.lot_size,
                max_leverage = EXCLUDED.max_leverage,
                initial_margin_rate = EXCLUDED.initial_margin_rate,
                maintenance_margin_rate = EXCLUDED.maintenance_margin_rate,
                pyth_price_feed_id = EXCLUDED.pyth_price_feed_id",
        )
        .bind(m.market)
        .bind(m.tick_size.parse::<Decimal>()?)
        .bind(m.lot_size.parse::<Decimal>()?)
        .bind(m.max_leverage)
        .bind(initial_margin_rate)
        .bind(maintenance_margin_rate)
        .bind(m.pyth_price_feed_id)
        .execute(&pool)
        .await?;

        println!(
            "Seeded market {} (tick={}, lot={}, max_leverage={}x, IMR={}, MMR={})",
            m.market, m.tick_size, m.lot_size, m.max_leverage, initial_margin_rate, maintenance_margin_rate
        );
    }

    println!("Market config seed complete.");
    Ok(())
}
