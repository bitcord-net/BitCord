use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::{sync::mpsc, time::interval};
use tracing::trace;

/// A snapshot of node-level operational metrics.
#[derive(Debug, Clone, Default)]
pub struct MetricsSnapshot {
    pub connected_peers: u64,
    pub stored_channels: u64,
    pub disk_usage_mb: u64,
    pub bandwidth_in_kbps: u64,
    pub bandwidth_out_kbps: u64,
    pub uptime_secs: u64,
}

/// Shared atomic counters updated by the metrics background task.
///
/// All fields are `AtomicU64` for lock-free reads from any thread.
#[derive(Debug, Default)]
pub struct NodeMetrics {
    pub connected_peers: AtomicU64,
    pub stored_channels: AtomicU64,
    pub disk_usage_mb: AtomicU64,
    pub bandwidth_in_kbps: AtomicU64,
    pub bandwidth_out_kbps: AtomicU64,
    pub uptime_secs: AtomicU64,
}

impl NodeMetrics {
    pub fn snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            connected_peers: self.connected_peers.load(Ordering::Relaxed),
            stored_channels: self.stored_channels.load(Ordering::Relaxed),
            disk_usage_mb: self.disk_usage_mb.load(Ordering::Relaxed),
            bandwidth_in_kbps: self.bandwidth_in_kbps.load(Ordering::Relaxed),
            bandwidth_out_kbps: self.bandwidth_out_kbps.load(Ordering::Relaxed),
            uptime_secs: self.uptime_secs.load(Ordering::Relaxed),
        }
    }
}

/// Message sent from the application to the metrics updater.
pub enum MetricsUpdate {
    ConnectedPeers(u64),
    StoredChannels(u64),
    DiskUsageMb(u64),
}

/// Spawn a background task that:
/// - Accepts `MetricsUpdate` messages over `update_rx` to set counter values.
/// - Refreshes `uptime_secs` every 5 seconds.
/// - Copies `bandwidth_*_kbps` from the provided atomic references every 5 s.
pub fn spawn_metrics_task(
    metrics: Arc<NodeMetrics>,
    mut update_rx: mpsc::Receiver<MetricsUpdate>,
    bandwidth_in_kbps: Arc<AtomicU64>,
    bandwidth_out_kbps: Arc<AtomicU64>,
    start_time: Instant,
) {
    tokio::spawn(async move {
        let mut tick = interval(Duration::from_secs(5));
        loop {
            tokio::select! {
                _ = tick.tick() => {
                    // Refresh uptime
                    let uptime = start_time.elapsed().as_secs();
                    metrics.uptime_secs.store(uptime, Ordering::Relaxed);

                    // Mirror bandwidth rates from BandwidthStats
                    let bw_in = bandwidth_in_kbps.load(Ordering::Relaxed);
                    let bw_out = bandwidth_out_kbps.load(Ordering::Relaxed);
                    metrics.bandwidth_in_kbps.store(bw_in, Ordering::Relaxed);
                    metrics.bandwidth_out_kbps.store(bw_out, Ordering::Relaxed);

                    trace!(
                        peers = metrics.connected_peers.load(Ordering::Relaxed),
                        channels = metrics.stored_channels.load(Ordering::Relaxed),
                        disk_mb = metrics.disk_usage_mb.load(Ordering::Relaxed),
                        bw_in,
                        bw_out,
                        uptime,
                        "metrics tick"
                    );
                }

                msg = update_rx.recv() => {
                    match msg {
                        Some(MetricsUpdate::ConnectedPeers(n)) => {
                            metrics.connected_peers.store(n, Ordering::Relaxed);
                        }
                        Some(MetricsUpdate::StoredChannels(n)) => {
                            metrics.stored_channels.store(n, Ordering::Relaxed);
                        }
                        Some(MetricsUpdate::DiskUsageMb(n)) => {
                            metrics.disk_usage_mb.store(n, Ordering::Relaxed);
                        }
                        None => break, // sender dropped; exit task
                    }
                }
            }
        }
    });
}
