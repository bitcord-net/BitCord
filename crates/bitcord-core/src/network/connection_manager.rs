//! QUIC `ConnectionManager` — maintains a live Quinn connection and handles
//! reconnects with exponential back-off.
//!
//! # Multiplexing model
//! - **Request / response**: `ConnectionManager::request()` opens a fresh
//!   bidirectional QUIC stream per call, writes one framed `ClientRequest`,
//!   reads one framed `NodeResponse`, then closes the stream.
//! - **Server push**: the node opens unidirectional streams; callers subscribe
//!   via `ConnectionManager::subscribe_push()` which spawns a background
//!   reader task and returns an `mpsc::Receiver<NodePush>`.

use std::{net::SocketAddr, sync::Arc, time::Duration};

use anyhow::{Context, Result};
use tokio::sync::{Mutex, mpsc};
use tracing::{debug, info, warn};

use crate::network::protocol::{
    ClientRequest, NodePush, NodeResponse, decode_payload, encode_frame,
};

use super::tls::client_config_pinned;

// ── Connection state ──────────────────────────────────────────────────────────

enum ConnState {
    /// Never connected — auto-connect on first use.
    Fresh,
    /// Live connection.
    Connected(quinn::Connection),
    /// Was connected, now dropped — do NOT auto-reconnect (caller must re-auth).
    Down,
}

// ── ConnectionManager ─────────────────────────────────────────────────────────

/// Manages a QUIC connection to a single node endpoint.
///
/// Thread-safe: `Arc<ConnectionManager>` may be cloned freely across tasks.
pub struct ConnectionManager {
    addr: SocketAddr,
    endpoint: quinn::Endpoint,
    state: Mutex<ConnState>,
}

impl ConnectionManager {
    /// Create a new manager that will connect to `addr` and pin `fingerprint`.
    ///
    /// Does **not** connect immediately; the first `request()` call establishes
    /// the connection.
    pub async fn new(addr: SocketAddr, fingerprint: [u8; 32]) -> Result<Self> {
        let client_cfg = client_config_pinned(fingerprint).context("build client TLS config")?;

        // Bind to an OS-assigned ephemeral port on all interfaces.
        let mut endpoint =
            quinn::Endpoint::client("0.0.0.0:0".parse()?).context("create QUIC client endpoint")?;
        endpoint.set_default_client_config(client_cfg);

        Ok(Self {
            addr,
            endpoint,
            state: Mutex::new(ConnState::Fresh),
        })
    }

    // ── Internal helpers ──────────────────────────────────────────────────

    /// Attempt a single connection to the node with a 10-second timeout.
    async fn try_connect(&self) -> Result<quinn::Connection> {
        debug!("connecting to {}", self.addr);
        let connecting = self
            .endpoint
            .connect(self.addr, "bitcord-node")
            .context("initiate QUIC connection")?;
        let conn = tokio::time::timeout(Duration::from_secs(10), connecting)
            .await
            .map_err(|_| anyhow::anyhow!("QUIC handshake: timed out"))?
            .context("QUIC handshake")?;
        info!("connected to {}", self.addr);
        Ok(conn)
    }

    /// Reconnect with exponential back-off, up to `max_attempts` tries.
    ///
    /// Initial delay: 100 ms. Each failure doubles the delay, capped at 30 s.
    pub async fn reconnect_with_backoff(&self, max_attempts: u32) -> Result<quinn::Connection> {
        let mut delay = Duration::from_millis(100);
        for attempt in 1..=max_attempts {
            match self.try_connect().await {
                Ok(conn) => {
                    *self.state.lock().await = ConnState::Connected(conn.clone());
                    return Ok(conn);
                }
                Err(e) if attempt < max_attempts => {
                    warn!(
                        "connection attempt {attempt}/{max_attempts} failed: {e}, \
                         retrying in {delay:?}"
                    );
                    tokio::time::sleep(delay).await;
                    delay = (delay * 2).min(Duration::from_secs(30));
                }
                Err(e) => return Err(e).context("max reconnect attempts exceeded"),
            }
        }
        unreachable!()
    }

    /// Return the live connection.
    ///
    /// - `Fresh`: connects once and transitions to `Connected`.
    /// - `Connected` + live: returns the existing connection.
    /// - `Connected` + closed: transitions to `Down` and returns an error.
    /// - `Down`: returns an error immediately (caller must re-authenticate).
    pub async fn get_connection(&self) -> Result<quinn::Connection> {
        let mut state = self.state.lock().await;
        match &*state {
            ConnState::Fresh => {
                let conn = self.try_connect().await.context("initial connect")?;
                *state = ConnState::Connected(conn.clone());
                Ok(conn)
            }
            ConnState::Connected(conn) => {
                if conn.close_reason().is_none() {
                    return Ok(conn.clone());
                }
                warn!(
                    "QUIC connection to {} closed; marking Down (re-auth required)",
                    self.addr
                );
                *state = ConnState::Down;
                anyhow::bail!(
                    "connection to {} is down; re-authentication required",
                    self.addr
                )
            }
            ConnState::Down => {
                anyhow::bail!(
                    "connection to {} is down; re-authentication required",
                    self.addr
                )
            }
        }
    }

    // ── Public API ────────────────────────────────────────────────────────

    /// Send a `ClientRequest` and return the `NodeResponse`.
    ///
    /// Opens a fresh bidirectional QUIC stream per call.  Does **not**
    /// auto-reconnect on failure — the caller (NodeClient / network_handle) is
    /// responsible for reconnecting with a fresh authenticated session.
    pub async fn request(&self, req: &ClientRequest) -> Result<NodeResponse> {
        self.do_request(req).await
    }

    async fn do_request(&self, req: &ClientRequest) -> Result<NodeResponse> {
        let conn = self.get_connection().await?;
        let (mut send, mut recv) = conn
            .open_bi()
            .await
            .context("open bidirectional QUIC stream")?;

        // Write the length-prefixed request.
        let frame = encode_frame(req).context("encode request")?;
        send.write_all(&frame)
            .await
            .context("write request frame")?;
        send.finish().context("finish send stream")?;

        // Read the 4-byte length prefix.
        let mut len_buf = [0u8; 4];
        recv.read_exact(&mut len_buf)
            .await
            .context("read response length")?;
        let len = u32::from_be_bytes(len_buf) as usize;

        // Read the payload.
        let mut payload = vec![0u8; len];
        recv.read_exact(&mut payload)
            .await
            .context("read response payload")?;

        decode_payload::<NodeResponse>(&payload).context("decode response")
    }

    /// Subscribe to push events from the node.
    ///
    /// Spawns a background task that calls `connection.accept_uni()` in a loop,
    /// reads each `NodePush` frame, and forwards it to the returned channel.
    /// If the connection drops, the task attempts to reconnect (up to 5 times).
    pub async fn subscribe_push(self: Arc<Self>, buffer: usize) -> mpsc::Receiver<NodePush> {
        let (tx, rx) = mpsc::channel(buffer);
        tokio::spawn(push_reader_task(self, tx));
        rx
    }
}

// ── Push reader task ──────────────────────────────────────────────────────────

async fn push_reader_task(mgr: Arc<ConnectionManager>, tx: mpsc::Sender<NodePush>) {
    // Get the already-established authenticated connection.
    // Do NOT loop here — if the connection is down we must NOT reconnect,
    // because reconnecting without re-running the auth handshake will cause
    // subsequent requests to receive 401 errors.
    let conn = match mgr.get_connection().await {
        Ok(c) => c,
        Err(e) => {
            warn!("push reader: connection unavailable: {e}");
            // Dropping tx signals the network_handle push_reader to exit,
            // which will trigger a full re-auth reconnect via reconnect_seed_loop.
            return;
        }
    };

    // Drain unidirectional streams from the node until the connection closes.
    loop {
        match conn.accept_uni().await {
            Ok(mut recv) => {
                // Read length prefix.
                let mut len_buf = [0u8; 4];
                if recv.read_exact(&mut len_buf).await.is_err() {
                    break;
                }
                let len = u32::from_be_bytes(len_buf) as usize;

                // Read payload.
                let mut payload = vec![0u8; len];
                if recv.read_exact(&mut payload).await.is_err() {
                    break;
                }

                match decode_payload::<NodePush>(&payload) {
                    Ok(push) => {
                        if tx.send(push).await.is_err() {
                            // Receiver dropped; stop the task cleanly.
                            return;
                        }
                    }
                    Err(e) => warn!("push reader: decode error: {e}"),
                }
            }
            Err(e) => {
                warn!("push reader: accept_uni failed: {e}");
                // Mark connection as Down so future get_connection() calls
                // return an error rather than silently reconnecting without auth.
                *mgr.state.lock().await = ConnState::Down;
                // Exit; dropping tx signals network_handle to trigger re-auth.
                return;
            }
        }
    }
}
