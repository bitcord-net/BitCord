use std::sync::Arc;

use jsonrpsee::RpcModule;
use jsonrpsee::types::ErrorObjectOwned;
use tracing::debug;

use super::super::AppState;
use super::super::types::{NodeConfigDto, NodeLocalInfo, PeerSummary, SetConfigParams};
use super::invalid_params;
use crate::resource::metrics::MetricsSnapshot;

pub(super) fn register_node_methods(module: &mut RpcModule<Arc<AppState>>) -> anyhow::Result<()> {
    module.register_async_method("node_get_metrics", |_params, ctx, _| async move {
        let snapshot = ctx.metrics.snapshot();
        Ok::<MetricsSnapshot, ErrorObjectOwned>(snapshot)
    })?;

    module.register_async_method("node_get_config", |_params, ctx, _| async move {
        let cfg = ctx.config.read().await;
        let dto = NodeConfigDto {
            listen_addrs: cfg.listen_addrs.clone(),
            max_connections: cfg.max_connections,
            storage_limit_mb: cfg.storage_limit_mb,
            bandwidth_limit_kbps: cfg.bandwidth_limit_kbps,
            node_mode: cfg.node_mode.clone(),
            seed_priority: cfg.seed_priority,
            log_level: cfg.log_level.clone(),
            preferred_mailbox_node: cfg.preferred_mailbox_node.clone(),
        };
        Ok::<NodeConfigDto, ErrorObjectOwned>(dto)
    })?;

    module.register_async_method("node_set_config", |params, ctx, _| async move {
        let p: SetConfigParams = params.parse().map_err(|e| invalid_params(e.to_string()))?;
        let mut cfg = ctx.config.write().await;
        if let Some(v) = p.listen_addrs {
            cfg.listen_addrs = v;
        }
        if let Some(v) = p.max_connections {
            cfg.max_connections = v;
        }
        if let Some(v) = p.storage_limit_mb {
            cfg.storage_limit_mb = v;
        }
        if let Some(v) = p.bandwidth_limit_kbps {
            cfg.bandwidth_limit_kbps = v;
        }
        if let Some(v) = p.node_mode {
            cfg.node_mode = v;
        }
        if let Some(v) = p.seed_priority {
            cfg.seed_priority = v;
        }
        if let Some(v) = p.log_level {
            cfg.log_level = v.clone();
            reload_tracing_filter(&v);
        }
        if let Some(v) = p.preferred_mailbox_node {
            cfg.preferred_mailbox_node = v;
        }
        if let Err(e) = cfg.save(&ctx.config_path) {
            tracing::warn!("failed to persist node config: {e:#}");
        } else {
            debug!("node config saved to {:?}", ctx.config_path);
        }
        Ok::<bool, ErrorObjectOwned>(true)
    })?;

    module.register_async_method("node_get_peers", |_params, ctx, _| async move {
        let peers_map = ctx.connected_peers.read().await;
        let all: Vec<PeerSummary> = peers_map.values().flatten().cloned().collect();
        Ok::<Vec<PeerSummary>, ErrorObjectOwned>(all)
    })?;

    module.register_async_method("node_get_local_addrs", |_params, ctx, _| async move {
        let info = NodeLocalInfo {
            node_address: ctx.node_address.clone(),
            listen_addrs: ctx.actual_listen_addrs.read().await.clone(),
        };
        Ok::<NodeLocalInfo, ErrorObjectOwned>(info)
    })?;

    Ok(())
}

/// Dynamically update the global tracing filter.
///
/// Only works if the `tracing-subscriber` was initialized with a `ReloadHandle`.
/// For headless `bitcord-node` and Tauri, this currently re-reads the filter
/// from the `RUST_LOG` environment variable or the provided string if using
/// a compatible global subscriber.
fn reload_tracing_filter(filter: &str) {
    // If the user set a custom filter string, try to apply it to the global
    // subscriber.  Since we use `fmt().init()` in main.rs, we can't easily
    // swap the filter at runtime without using a `reload::Layer`.
    //
    // For now, we update the env so that any new tasks/threads might pick it
    // up if they check, though most subscribers bake it in at init.
    // TODO: refactor init to use reloadable layer for true dynamic updates.
    unsafe {
        std::env::set_var("RUST_LOG", filter);
    }
    debug!(
        "requested log level update to: {filter} (requires restart if not using reloadable layer)"
    );
}
