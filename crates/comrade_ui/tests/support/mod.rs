//! A minimal, in-process Nostr relay (a NIP-01 subset) so the two-peer
//! integration suite (`tests/two_peer_integration.rs`) is hermetic — no
//! Docker, no public-internet relay dependency (AUDIT.md COMMS-03: "create a
//! test environment with an isolated Nostr relay").
//!
//! Supports exactly what `nostr-sdk`'s client needs to drive Sabha/Vault
//! traffic: `EVENT` (signature-verified, stored, broadcast to matching live
//! subscriptions, acked with `OK`), `REQ` (replays stored matches, then
//! `EOSE`, then forwards future matching events live), `CLOSE`. Everything
//! else a production relay would have — NIP-11, rate limiting, AUTH, storage
//! limits — is deliberately out of scope: this exists to test *our* code
//! against a real (if tiny) relay implementation, not to be one.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use futures_util::{SinkExt, StreamExt};
use nostr_sdk::prelude::*;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, oneshot};
use tokio_tungstenite::tungstenite::Message;

/// Bounded generously above anything one test session publishes; a lagged
/// receiver would only mean a live-forward gets skipped; the initial `REQ`
/// replay is unaffected either way since it reads the persisted `store`, not
/// this channel.
const EVENT_BROADCAST_CAPACITY: usize = 1024;

/// A running in-process relay bound to an OS-assigned local port. Call
/// [`TestRelay::stop`] (or just let it drop — the accept loop is independent
/// of this handle either way, but tests should still call `stop` so a slow
/// test doesn't leak listening sockets across the suite).
pub struct TestRelay {
    pub url: String,
    shutdown_tx: Option<oneshot::Sender<()>>,
    accept_task: Option<tokio::task::JoinHandle<()>>,
}

impl TestRelay {
    /// Bind an ephemeral local port and start serving immediately.
    pub async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind ephemeral test-relay port");
        let addr = listener.local_addr().expect("listener has a local addr");
        let url = format!("ws://{addr}");

        let store: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
        let (events_tx, _) = broadcast::channel::<Event>(EVENT_BROADCAST_CAPACITY);
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel();

        let accept_task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = &mut shutdown_rx => break,
                    accepted = listener.accept() => {
                        let Ok((stream, _)) = accepted else { continue };
                        tokio::spawn(serve_connection(stream, store.clone(), events_tx.clone()));
                    }
                }
            }
        });

        Self {
            url,
            shutdown_tx: Some(shutdown_tx),
            accept_task: Some(accept_task),
        }
    }

    /// Stop accepting connections and wait for the accept loop to exit.
    pub async fn stop(mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        if let Some(task) = self.accept_task.take() {
            let _ = task.await;
        }
    }
}

async fn serve_connection(
    stream: TcpStream,
    store: Arc<Mutex<Vec<Event>>>,
    events_tx: broadcast::Sender<Event>,
) {
    let Ok(ws) = tokio_tungstenite::accept_async(stream).await else {
        return;
    };
    let (mut write, mut read) = ws.split();
    let mut live_subs: HashMap<SubscriptionId, Vec<Filter>> = HashMap::new();
    let mut events_rx = events_tx.subscribe();
    let match_opts = MatchEventOptions::new();

    loop {
        tokio::select! {
            // Forward a just-published event to every live subscription it matches.
            broadcasted = events_rx.recv() => {
                let Ok(event) = broadcasted else { continue };
                for (sub_id, filters) in &live_subs {
                    if filters.iter().any(|f| f.match_event(&event, match_opts)) {
                        let msg = RelayMessage::Event {
                            subscription_id: Cow::Borrowed(sub_id),
                            event: Cow::Borrowed(&event),
                        };
                        if write.send(Message::text(msg.as_json())).await.is_err() {
                            return;
                        }
                    }
                }
            }
            incoming = read.next() => {
                let Some(Ok(incoming)) = incoming else { return };
                let text = match incoming {
                    Message::Text(t) => t.to_string(),
                    Message::Close(_) => return,
                    _ => continue,
                };
                let Ok(client_msg) = ClientMessage::from_json(&text) else { continue };
                match client_msg {
                    ClientMessage::Event(event) => {
                        let event = event.into_owned();
                        let verified = event.verify().is_ok();
                        let ok_reply = RelayMessage::Ok {
                            event_id: event.id,
                            status: verified,
                            message: Cow::Borrowed(if verified { "" } else { "invalid: bad signature" }),
                        };
                        if write.send(Message::text(ok_reply.as_json())).await.is_err() {
                            return;
                        }
                        if verified {
                            store.lock().unwrap().push(event.clone());
                            let _ = events_tx.send(event);
                        }
                    }
                    ClientMessage::Req { subscription_id, filters } => {
                        let sub_id = subscription_id.into_owned();
                        let filters: Vec<Filter> = filters.into_iter().map(Cow::into_owned).collect();
                        let backlog: Vec<Event> = {
                            let stored = store.lock().unwrap();
                            stored
                                .iter()
                                .filter(|e| filters.iter().any(|f| f.match_event(e, match_opts)))
                                .cloned()
                                .collect()
                        };
                        for event in &backlog {
                            let msg = RelayMessage::Event {
                                subscription_id: Cow::Borrowed(&sub_id),
                                event: Cow::Borrowed(event),
                            };
                            if write.send(Message::text(msg.as_json())).await.is_err() {
                                return;
                            }
                        }
                        let eose = RelayMessage::EndOfStoredEvents(Cow::Borrowed(&sub_id));
                        if write.send(Message::text(eose.as_json())).await.is_err() {
                            return;
                        }
                        live_subs.insert(sub_id, filters);
                    }
                    ClientMessage::Close(sub_id) => {
                        live_subs.remove(&*sub_id);
                    }
                    // AUTH/COUNT/NEG-*: unsupported, and never sent by our client.
                    _ => {}
                }
            }
        }
    }
}
