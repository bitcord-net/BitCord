use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{RwLock, mpsc, oneshot};
use tracing::{debug, info, warn};

use crate::{
    crypto::certificate::HostingCert,
    identity::NodeIdentity,
    network::{client::NodeClient, node_addr::NodeAddr},
};

use super::types::{NetworkEvent, PeerRegistration};

/// Continuously retries connecting to a seed peer after it disconnects.
///
/// Uses exponential back-off starting at 5 s, capped at 5 minutes.
/// When the connection is re-established a fresh `PeerRegistration` is sent
/// back to the main gossip task so the peer is tracked as a seed again.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn reconnect_seed_loop(
    addr: NodeAddr,
    identity: Arc<NodeIdentity>,
    reg_tx: mpsc::Sender<PeerRegistration>,
    evt_fwd: mpsc::Sender<NetworkEvent>,
    own_pk_hex: String,
    join_community: Option<([u8; 32], String)>,
    join_community_password: Option<String>,
    cert_fingerprint: [u8; 32],
    own_addrs: Arc<RwLock<HashSet<String>>>,
    mut cancel_rx: oneshot::Receiver<()>,
) {
    let addr_str = addr.to_string();
    let mut delay_secs: u64 = 5;
    loop {
        tokio::select! {
            _ = &mut cancel_rx => {
                info!(%addr, "seed reconnect: cancelled (seed evicted from config)");
                return;
            }
            _ = tokio::time::sleep(Duration::from_secs(delay_secs)) => {}
        }
        // If STUN has since identified this address as our own (self-hosted seed),
        // stop retrying — we cannot hairpin-connect to ourselves via NAT.
        if own_addrs.read().await.contains(&addr_str) {
            info!(%addr, "seed reconnect: address is own (self-hosted); stopping loop");
            return;
        }
        debug!(%addr, delay_secs, "seed reconnect: attempting");
        match NodeClient::connect(addr.clone(), cert_fingerprint, Arc::clone(&identity)).await {
            Ok((client, node_pk, push_rx)) => {
                let peer_id: String = node_pk.iter().map(|b| format!("{b:02x}")).collect();
                info!(%peer_id, %addr, "seed peer reconnected");

                if let Some((community_pk, community_id)) = join_community.clone() {
                    let sk = identity.signing_key();
                    if sk.verifying_key().to_bytes() == community_pk {
                        // HostingCert expiry is u64::MAX (certs never expire by design).
                        let cert = HostingCert::new(&sk, node_pk, u64::MAX);
                        if let Err(e) = client
                            .join_community(
                                cert,
                                Some(community_id.clone()),
                                join_community_password.clone(),
                            )
                            .await
                        {
                            warn!(%peer_id, %community_id, "seed reconnect: auto-join failed: {e}");
                        }
                    } else {
                        info!(%peer_id, %community_id, "seed reconnect: reconnected as member (HostingCert not issued — not the community admin)");
                    }
                }

                let _ = reg_tx
                    .send(PeerRegistration {
                        peer_id,
                        node_pk,
                        client,
                        is_seed: true,
                        addr,
                        push_rx,
                        evt_fwd,
                        own_pk: own_pk_hex.clone(),
                        join_community,
                        join_community_password,
                        cert_fingerprint,
                    })
                    .await;
                return; // success — exit the loop
            }
            Err(e) => {
                warn!(%addr, delay_secs, "seed reconnect failed: {e}");
                delay_secs = (delay_secs * 2).min(300); // cap at 5 minutes
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// Verify that `reconnect_seed_loop` exits promptly when the cancel signal
    /// is sent, without waiting for the back-off sleep to expire.
    #[tokio::test]
    async fn reconnect_loop_exits_on_cancel() {
        let addr: NodeAddr = "127.0.0.1:1".parse().expect("parse addr"); // port 1 is unreachable
        let identity = Arc::new(NodeIdentity::generate());
        let (reg_tx, _reg_rx) = mpsc::channel(4);
        let (evt_tx, _evt_rx) = mpsc::channel(4);
        let own_addrs = Arc::new(RwLock::new(HashSet::new()));

        let (cancel_tx, cancel_rx) = oneshot::channel::<()>();

        // Send the cancel signal immediately — before the loop even starts its
        // first sleep — so the select picks it up on the very first iteration.
        cancel_tx.send(()).expect("send cancel");

        // The loop should return almost instantly.
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            reconnect_seed_loop(
                addr,
                identity,
                reg_tx,
                evt_tx,
                String::new(),
                None,
                None,
                [0u8; 32],
                own_addrs,
                cancel_rx,
            ),
        )
        .await
        .expect("reconnect_seed_loop should have exited quickly after cancel");
    }

    /// Verify that the loop does NOT exit before the cancel signal is sent
    /// (i.e., the cancellation is not spurious).
    #[tokio::test]
    async fn reconnect_loop_does_not_exit_without_cancel() {
        let addr: NodeAddr = "127.0.0.1:1".parse().expect("parse addr");
        let identity = Arc::new(NodeIdentity::generate());
        let (reg_tx, _reg_rx) = mpsc::channel(4);
        let (evt_tx, _evt_rx) = mpsc::channel(4);
        let own_addrs = Arc::new(RwLock::new(HashSet::new()));

        let (_cancel_tx, cancel_rx) = oneshot::channel::<()>();
        // _cancel_tx is intentionally NOT sent on, so the loop stays alive.

        let result = tokio::time::timeout(
            std::time::Duration::from_millis(100),
            reconnect_seed_loop(
                addr,
                identity,
                reg_tx,
                evt_tx,
                String::new(),
                None,
                None,
                [0u8; 32],
                own_addrs,
                cancel_rx,
            ),
        )
        .await;

        // Should have timed out (Err) because the loop is still sleeping.
        assert!(
            result.is_err(),
            "loop should still be running without a cancel signal"
        );
    }
}
