use std::sync::Arc;

use jsonrpsee::RpcModule;

use super::super::AppState;
use super::internal_err;

pub(super) fn register_subscription(module: &mut RpcModule<Arc<AppState>>) -> anyhow::Result<()> {
    module.register_subscription(
        "subscribe_events",
        "event",
        "unsubscribe_events",
        |_params, pending, ctx, _| async move {
            let mut rx = ctx.broadcaster.subscribe();
            let sink = pending.accept().await?;
            loop {
                tokio::select! {
                    result = rx.recv() => {
                        match result {
                            Ok(event) => {
                                let msg = jsonrpsee::SubscriptionMessage::from_json(&event)
                                    .map_err(|e| internal_err(e.to_string()))?;
                                if sink.send(msg).await.is_err() {
                                    break;
                                }
                            }
                            // Lagged (receiver fell behind) — skip and continue
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                        }
                    }
                    _ = sink.closed() => break,
                }
            }
            Ok(())
        },
    )?;

    Ok(())
}
