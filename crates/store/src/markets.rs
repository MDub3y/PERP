use crate::models::Market;
use sqlx::PgPool;

pub async fn fetch_all_markets(pool: &PgPool) -> Result<Vec<Market>, sqlx::Error> {
    sqlx::query_as::<_, Market>("SELECT * FROM markets ORDER BY market")
        .fetch_all(pool)
        .await
}
