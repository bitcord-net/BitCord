use governor::{
    Quota, RateLimiter,
    clock::DefaultClock,
    middleware::NoOpMiddleware,
    state::{InMemoryState, NotKeyed},
};
use std::{
    num::NonZeroU32,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::time::interval;
use tracing::trace;

/// Cumulative byte counters and rolling rate estimates, updated every second.
#[derive(Debug, Default)]
pub struct BandwidthStats {
    pub bytes_sent: AtomicU64,
    pub bytes_received: AtomicU64,
    /// Rolling outbound rate in kbps (updated every second).
    pub rate_out_kbps: AtomicU64,
    /// Rolling inbound rate in kbps (updated every second).
    pub rate_in_kbps: AtomicU64,
}

type Limiter = RateLimiter<NotKeyed, InMemoryState, DefaultClock, NoOpMiddleware>;

/// Token-bucket bandwidth limiter.
///
/// Each token represents one byte. The rate is enforced by the underlying
/// governor `RateLimiter`. When `limit_kbps` is `None` the limiter is a no-op.
pub struct BandwidthLimiter {
    /// `(limiter, burst_bytes)` — `None` means unlimited.
    limiter: Option<(Arc<Limiter>, u32)>,
    pub stats: Arc<BandwidthStats>,
}

impl BandwidthLimiter {
    /// Create a limiter capped at `limit_kbps` kilobits per second, or uncapped
    /// when `None`.
    pub fn new(limit_kbps: Option<u64>) -> Self {
        let limiter = limit_kbps.and_then(|kbps| {
            // Convert kbps → bytes per second (1 kbps = 125 bytes/s).
            let bytes_per_sec = ((kbps * 125) as u32).max(1);
            NonZeroU32::new(bytes_per_sec).map(|bps| {
                let quota = Quota::per_second(bps);
                (Arc::new(RateLimiter::direct(quota)), bps.get())
            })
        });
        Self {
            limiter,
            stats: Arc::new(BandwidthStats::default()),
        }
    }

    /// Record `bytes` sent; block until the token bucket allows it.
    pub async fn on_send(&self, bytes: u64) {
        self.stats.bytes_sent.fetch_add(bytes, Ordering::Relaxed);
        if let Some((lim, burst)) = &self.limiter {
            throttle(lim, *burst, bytes).await;
        }
    }

    /// Record `bytes` received; block until the token bucket allows it.
    pub async fn on_receive(&self, bytes: u64) {
        self.stats
            .bytes_received
            .fetch_add(bytes, Ordering::Relaxed);
        if let Some((lim, burst)) = &self.limiter {
            throttle(lim, *burst, bytes).await;
        }
    }

    /// Spawn a background Tokio task that recalculates `rate_*_kbps` every
    /// second by differencing the cumulative byte counters.
    pub fn spawn_stats_updater(stats: Arc<BandwidthStats>) {
        tokio::spawn(async move {
            let mut tick = interval(Duration::from_secs(1));
            let mut prev_sent = 0u64;
            let mut prev_recv = 0u64;
            loop {
                tick.tick().await;
                let sent = stats.bytes_sent.load(Ordering::Relaxed);
                let recv = stats.bytes_received.load(Ordering::Relaxed);
                // bits / 1024 = kbits
                let rate_out = (sent.saturating_sub(prev_sent)) * 8 / 1024;
                let rate_in = (recv.saturating_sub(prev_recv)) * 8 / 1024;
                stats.rate_out_kbps.store(rate_out, Ordering::Relaxed);
                stats.rate_in_kbps.store(rate_in, Ordering::Relaxed);
                trace!("bandwidth: out={rate_out} kbps  in={rate_in} kbps");
                prev_sent = sent;
                prev_recv = recv;
            }
        });
    }
}

/// Consume `bytes` tokens from the limiter, chunking by `burst` to avoid
/// `InsufficientCapacity` errors when a single payload exceeds the burst size.
async fn throttle(lim: &Limiter, burst: u32, bytes: u64) {
    let mut remaining = bytes;
    while remaining > 0 {
        let chunk = remaining.min(burst as u64) as u32;
        let n = NonZeroU32::new(chunk).unwrap_or(NonZeroU32::MIN);
        // until_n_ready returns Err only if n > burst (impossible here since chunk ≤ burst).
        let _ = lim.until_n_ready(n).await;
        remaining = remaining.saturating_sub(chunk as u64);
    }
}
