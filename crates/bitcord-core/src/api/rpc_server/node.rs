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
            seed_nodes: cfg.seed_nodes.clone(),
            max_connections: cfg.max_connections,
            storage_limit_mb: cfg.storage_limit_mb,
            bandwidth_limit_kbps: cfg.bandwidth_limit_kbps,
            is_seed_node: cfg.is_seed_node,
            seed_priority: cfg.seed_priority,
            mdns_enabled: cfg.mdns_enabled,
            log_level: cfg.log_level.clone(),
            server_enabled: cfg.server_enabled,
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
        if let Some(v) = p.seed_nodes {
            cfg.seed_nodes = v;
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
        if let Some(v) = p.is_seed_node {
            cfg.is_seed_node = v;
        }
        if let Some(v) = p.seed_priority {
            cfg.seed_priority = v;
        }
        if let Some(v) = p.mdns_enabled {
            cfg.mdns_enabled = v;
        }
        if let Some(v) = p.log_level {
            cfg.log_level = v;
        }
        if let Some(v) = p.server_enabled {
            cfg.server_enabled = v;
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
        let peers = ctx.connected_peers.read().await;
        Ok::<Vec<PeerSummary>, ErrorObjectOwned>(peers.clone())
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
