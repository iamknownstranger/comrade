/*!
 * Milestone 4 — Saathi: Off-Grid Libp2p Mesh Engine
 *
 * Spins up a local peer-to-peer Swarm using:
 *  • TCP transport with Noise handshake + Yamux multiplexing
 *  • mDNS for automatic local-network peer discovery
 *  • Gossipsub for message propagation across all discovered peers
 *
 * When `AppWorkspace::OffGridTravel` is active this engine replaces the
 * Nostr relay connection. Outbound messages issued while no peers are
 * reachable are cached locally and broadcast as soon as peers join.
 */

use std::{
    collections::VecDeque,
    hash::{DefaultHasher, Hash, Hasher},
    sync::Arc,
    time::Duration,
};

use libp2p::{
    futures::StreamExt,
    gossipsub::{self, IdentTopic, MessageId},
    mdns,
    swarm::{NetworkBehaviour, SwarmEvent},
    PeerId, Swarm,
};
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, Mutex};
use tracing::{debug, info, warn};

use crate::error::SaathiError;

// ── Gossipsub topic ──────────────────────────────────────────────────────────

const TOPIC_NAME: &str = "comrade/saathi/v1";
const MAX_CACHE: usize = 256;

// ── Wire message format ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeshMessage {
    pub sender: String,
    pub content: String,
    pub timestamp: u64,
}

impl MeshMessage {
    pub fn new(sender: impl Into<String>, content: impl Into<String>) -> Self {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        Self {
            sender: sender.into(),
            content: content.into(),
            timestamp,
        }
    }
}

// ── Combined NetworkBehaviour ────────────────────────────────────────────────

#[derive(NetworkBehaviour)]
struct ComradeBehaviour {
    gossipsub: gossipsub::Behaviour,
    mdns: mdns::tokio::Behaviour,
}

// ── Saathi engine ────────────────────────────────────────────────────────────

/// Shared state accessed from both the swarm driver task and callers.
struct SaathiShared {
    #[allow(dead_code)]
    peer_id: PeerId,
    outbox_cache: VecDeque<MeshMessage>,
    received: Vec<MeshMessage>,
}

pub struct SaathiEngine {
    /// Sender half of the command channel into the swarm driver task.
    cmd_tx: mpsc::Sender<SaathiCmd>,
    /// Received messages stream.
    msg_rx: Arc<Mutex<mpsc::Receiver<MeshMessage>>>,
    shared: Arc<Mutex<SaathiShared>>,
    local_id: String,
}

enum SaathiCmd {
    Broadcast(MeshMessage),
    Shutdown,
}

impl SaathiEngine {
    /// Initialise the Saathi engine — builds the Swarm and spawns the driver.
    pub async fn new(local_sender_label: impl Into<String>) -> Result<Self, SaathiError> {
        let sender_label = local_sender_label.into();

        // Message-ID function: hash the raw gossipsub payload for deduplication
        let message_id_fn = |msg: &gossipsub::Message| {
            let mut h = DefaultHasher::new();
            msg.data.hash(&mut h);
            MessageId::from(h.finish().to_string())
        };

        let gossipsub_config = gossipsub::ConfigBuilder::default()
            .heartbeat_interval(Duration::from_secs(10))
            .validation_mode(gossipsub::ValidationMode::Strict)
            .message_id_fn(message_id_fn)
            .build()
            .map_err(|e| SaathiError::SwarmInit(e.to_string()))?;

        let mut swarm: Swarm<ComradeBehaviour> = libp2p::SwarmBuilder::with_new_identity()
            .with_tokio()
            .with_tcp(
                libp2p::tcp::Config::default(),
                libp2p::noise::Config::new,
                libp2p::yamux::Config::default,
            )
            .map_err(|e| SaathiError::SwarmInit(e.to_string()))?
            .with_behaviour(|key| {
                let gossipsub = gossipsub::Behaviour::new(
                    gossipsub::MessageAuthenticity::Signed(key.clone()),
                    gossipsub_config,
                )
                .map_err(std::io::Error::other)?;

                let mdns = mdns::tokio::Behaviour::new(
                    mdns::Config::default(),
                    key.public().to_peer_id(),
                )?;

                Ok(ComradeBehaviour { gossipsub, mdns })
            })
            .map_err(|e| SaathiError::SwarmInit(e.to_string()))?
            .with_swarm_config(|c| c.with_idle_connection_timeout(Duration::from_secs(60)))
            .build();

        let local_peer_id = *swarm.local_peer_id();
        info!(peer_id = %local_peer_id, "Saathi swarm identity");

        // Subscribe to the shared mesh topic
        let topic = IdentTopic::new(TOPIC_NAME);
        swarm
            .behaviour_mut()
            .gossipsub
            .subscribe(&topic)
            .map_err(|e| SaathiError::SwarmInit(e.to_string()))?;

        // Listen on a random TCP port
        swarm
            .listen_on("/ip4/0.0.0.0/tcp/0".parse().expect("valid multiaddr"))
            .map_err(|e| SaathiError::TransportError(e.to_string()))?;

        // Channel for incoming messages surfaced to callers
        let (msg_tx, msg_rx) = mpsc::channel::<MeshMessage>(128);
        // Channel for commands from callers into the swarm loop
        let (cmd_tx, mut cmd_rx) = mpsc::channel::<SaathiCmd>(64);

        let shared = Arc::new(Mutex::new(SaathiShared {
            peer_id: local_peer_id,
            outbox_cache: VecDeque::new(),
            received: Vec::new(),
        }));

        let shared_clone = shared.clone();
        let _label_clone = sender_label.clone();

        // ── Swarm driver task ──────────────────────────────────────────────
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    // Process commands from callers
                    Some(cmd) = cmd_rx.recv() => {
                        match cmd {
                            SaathiCmd::Broadcast(msg) => {
                                let bytes = match serde_json::to_vec(&msg) {
                                    Ok(b)  => b,
                                    Err(e) => {
                                        warn!("Saathi: failed to serialise message: {e}");
                                        continue;
                                    }
                                };
                                let topic = IdentTopic::new(TOPIC_NAME);
                                if let Err(e) = swarm.behaviour_mut().gossipsub.publish(topic, bytes) {
                                    warn!("Saathi: gossipsub publish failed: {e} — caching message");
                                    let mut guard = shared_clone.lock().await;
                                    if guard.outbox_cache.len() < MAX_CACHE {
                                        guard.outbox_cache.push_back(msg);
                                    } else {
                                        warn!("Saathi: outbox cache full, dropping message");
                                    }
                                }
                            }
                            SaathiCmd::Shutdown => {
                                info!("Saathi swarm shutting down");
                                break;
                            }
                        }
                    }

                    // Process swarm events
                    event = swarm.next() => {
                        let Some(event) = event else { break };
                        match event {
                            SwarmEvent::Behaviour(ComradeBehaviourEvent::Mdns(
                                mdns::Event::Discovered(peers)
                            )) => {
                                for (peer_id, _) in peers {
                                    info!(peer = %peer_id, "Saathi: peer discovered via mDNS");
                                    swarm.behaviour_mut()
                                         .gossipsub
                                         .add_explicit_peer(&peer_id);

                                    // Drain the outbox cache now that we have a peer
                                    let mut guard = shared_clone.lock().await;
                                    while let Some(cached_msg) = guard.outbox_cache.pop_front() {
                                        let bytes = match serde_json::to_vec(&cached_msg) {
                                            Ok(b)  => b,
                                            Err(e) => {
                                                warn!("Saathi: cache drain serialise fail: {e}");
                                                continue;
                                            }
                                        };
                                        let topic = IdentTopic::new(TOPIC_NAME);
                                        if let Err(e) = swarm.behaviour_mut().gossipsub.publish(topic, bytes) {
                                            warn!("Saathi: cache drain publish fail: {e}");
                                        } else {
                                            debug!("Saathi: cached message drained to network");
                                        }
                                    }
                                }
                            }

                            SwarmEvent::Behaviour(ComradeBehaviourEvent::Mdns(
                                mdns::Event::Expired(peers)
                            )) => {
                                for (peer_id, _) in peers {
                                    debug!(peer = %peer_id, "Saathi: peer expired from mDNS");
                                    swarm.behaviour_mut()
                                         .gossipsub
                                         .remove_explicit_peer(&peer_id);
                                }
                            }

                            SwarmEvent::Behaviour(ComradeBehaviourEvent::Gossipsub(
                                gossipsub::Event::Message { message, .. }
                            )) => {
                                match serde_json::from_slice::<MeshMessage>(&message.data) {
                                    Ok(msg) => {
                                        debug!(
                                            sender    = %msg.sender,
                                            content   = %msg.content,
                                            "Saathi mesh message received"
                                        );
                                        let mut guard = shared_clone.lock().await;
                                        guard.received.push(msg.clone());
                                        drop(guard);
                                        if msg_tx.send(msg).await.is_err() {
                                            warn!("Saathi: receiver dropped, stopping driver");
                                            break;
                                        }
                                    }
                                    Err(e) => {
                                        warn!("Saathi: failed to deserialise mesh message: {e}");
                                    }
                                }
                            }

                            SwarmEvent::NewListenAddr { address, .. } => {
                                info!(addr = %address, "Saathi: listening on");
                            }

                            _ => {}
                        }
                    }
                }
            }
        });

        Ok(Self {
            cmd_tx,
            msg_rx: Arc::new(Mutex::new(msg_rx)),
            shared,
            local_id: sender_label,
        })
    }

    /// Broadcast a plaintext message to all mesh peers.
    /// If no peers are currently reachable the message is cached locally.
    pub async fn broadcast(&self, content: impl Into<String>) -> Result<(), SaathiError> {
        let msg = MeshMessage::new(self.local_id.clone(), content);
        self.cmd_tx
            .send(SaathiCmd::Broadcast(msg))
            .await
            .map_err(|_| SaathiError::BroadcastFailed("swarm task terminated".into()))
    }

    /// Receive the next incoming mesh message (blocks until one arrives).
    pub async fn recv_message(&self) -> Option<MeshMessage> {
        self.msg_rx.lock().await.recv().await
    }

    /// Snapshot of all messages received so far.
    pub async fn all_received(&self) -> Vec<MeshMessage> {
        self.shared.lock().await.received.clone()
    }

    /// Number of messages still sitting in the offline outbox cache.
    pub async fn cached_count(&self) -> usize {
        self.shared.lock().await.outbox_cache.len()
    }

    /// Gracefully shut down the swarm driver.
    pub async fn shutdown(&self) {
        let _ = self.cmd_tx.send(SaathiCmd::Shutdown).await;
    }

    pub fn local_peer_label(&self) -> &str {
        &self.local_id
    }
}
