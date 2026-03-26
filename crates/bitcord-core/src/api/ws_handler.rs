//! WebSocket subscription helpers.
//!
//! The JSON-RPC WebSocket server is powered by `jsonrpsee`. This module
//! provides supporting utilities for the push-event subscription, including the
//! helper that bridges `broadcast::Receiver<PushEvent>` to a
//! `jsonrpsee::SubscriptionSink`.
//!
//! Push events flow:
//!
//! ```text
//! SwarmEvent / metrics task
//!       │
//!       ▼
//! PushBroadcaster::send(event)          (broadcast::Sender<PushEvent>)
//!       │   fanout
//!       ├──▶ SubscriptionSink (WS connection 1)
//!       ├──▶ SubscriptionSink (WS connection 2)
//!       └──▶ ...
//! ```
//!
//! Each active `subscribe_events` subscription calls
//! [`forward_push_events`] which runs until the WebSocket is closed or
//! the broadcast channel is dropped.

use jsonrpsee::{SubscriptionMessage, core::server::SubscriptionSink, types::ErrorObjectOwned};
use tokio::sync::broadcast;
use tracing::debug;

use super::push_broadcaster::PushEvent;

/// Forward push events from a broadcast receiver into a JSON-RPC subscription sink.
///
/// Returns when the client closes the subscription or the broadcast sender is dropped.
pub async fn forward_push_events(mut rx: broadcast::Receiver<PushEvent>, sink: SubscriptionSink) {
    loop {
        tokio::select! {
            result = rx.recv() => {
                match result {
                    Ok(event) => {
                        let Ok(msg) = SubscriptionMessage::from_json(&event) else {
                            debug!("failed to serialize push event");
                            continue;
                        };
                        if sink.send(msg).await.is_err() {
                            // Client disconnected.
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        debug!("push subscriber lagged by {n} messages");
                        // Continue — the client is just slow; we skip the missed events.
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
            _ = sink.closed() => {
                debug!("push subscription closed by client");
                break;
            }
        }
    }
}

/// Convert an `ErrorObjectOwned` into a loggable string (for subscriber error handling).
pub fn format_rpc_error(err: &ErrorObjectOwned) -> String {
    format!("RPC error {}: {}", err.code(), err.message())
}
