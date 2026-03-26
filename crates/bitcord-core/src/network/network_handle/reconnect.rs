use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
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
) {
    let mut delay_secs: u64 = 5;
    loop {
        tokio::time::sleep(Duration::from_secs(delay_secs)).await;
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
