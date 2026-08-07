use axum::extract::ws::{Message, WebSocket};
use futures::{SinkExt, StreamExt};

/// Subscribes to `channels` on a fresh Redis pubsub connection and forwards
/// every message to `socket` as a `{"channel": <tag>, "data": <payload>}`
/// text frame, where `tag` is the suffix of the Redis channel name after the
/// last `:` (e.g. "market:SOL:trades" -> "trades"). Runs until the client
/// disconnects or the Redis connection drops.
pub async fn relay(redis_client: &redis::Client, channels: &[String], socket: WebSocket) {
    let mut pubsub = match redis_client.get_async_pubsub().await {
        Ok(p) => p,
        Err(e) => {
            tracing::error!("failed to open redis pubsub connection: {e}");
            return;
        }
    };

    for channel in channels {
        if let Err(e) = pubsub.subscribe(channel).await {
            tracing::error!("failed to subscribe to {channel}: {e}");
            return;
        }
    }

    let mut redis_messages = pubsub.on_message();
    let (mut ws_tx, mut ws_rx) = socket.split();

    loop {
        tokio::select! {
            msg = redis_messages.next() => {
                let Some(msg) = msg else { break };
                let channel_name: String = msg.get_channel_name().to_string();
                let tag = channel_name.rsplit(':').next().unwrap_or(&channel_name);
                let payload: String = match msg.get_payload() {
                    Ok(p) => p,
                    Err(e) => {
                        tracing::warn!("failed to decode redis payload: {e}");
                        continue;
                    }
                };
                let data: serde_json::Value =
                    serde_json::from_str(&payload).unwrap_or(serde_json::Value::Null);
                let frame = serde_json::json!({ "channel": tag, "data": data }).to_string();
                if ws_tx.send(Message::Text(frame.into())).await.is_err() {
                    break;
                }
            }
            incoming = ws_rx.next() => {
                match incoming {
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Err(_)) => break,
                    _ => {}
                }
            }
        }
    }
}
