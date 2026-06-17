/*!
 * Milestone 3a — Sabha: Public Microblogging Engine
 *
 * Connects to public Nostr relays, subscribes to Kind-1 text notes, and
 * parses a flat unsorted stream of events into a structured NIP-10 comment
 * tree using recursive parent-child resolution.
 */

use std::{collections::HashMap, sync::Arc};

use nostr_sdk::prelude::*;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::error::SabhaError;

// ── Well-known public relays ─────────────────────────────────────────────────

pub const DEFAULT_RELAYS: &[&str] = &[
    "wss://relay.damus.io",
    "wss://relay.nostr.band",
    "wss://nos.lol",
];

// ── NIP-10 thread-tree structures ────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadNode {
    /// The raw Nostr event at this tree position.
    pub event: Event,
    /// Zero-indexed depth from a root node.
    pub depth: usize,
    /// Direct replies to this node, sorted by created_at ascending.
    pub children: Vec<ThreadNode>,
}

impl ThreadNode {
    fn new(event: Event, depth: usize) -> Self {
        Self {
            event,
            depth,
            children: Vec::new(),
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct ThreadTree {
    /// Top-level events — those that have no parent within the local set.
    pub roots: Vec<ThreadNode>,
}

impl ThreadTree {
    /// Total number of events across all levels.
    pub fn len(&self) -> usize {
        fn count(nodes: &[ThreadNode]) -> usize {
            nodes.iter().map(|n| 1 + count(&n.children)).sum()
        }
        count(&self.roots)
    }

    pub fn is_empty(&self) -> bool {
        self.roots.is_empty()
    }
}

// ── Tag parsing helpers ──────────────────────────────────────────────────────

/// Extract NIP-10 "e" tags from an event using the canonical JSON representation.
///
/// Returns `(event_id_hex, relay_url, marker)` for each "e" tag where marker
/// is one of "root", "reply", "mention", or "" (positional).
fn extract_e_tags(event: &Event) -> Vec<(String, String, String)> {
    let val = match serde_json::to_value(event) {
        Ok(v) => v,
        Err(e) => {
            warn!("failed to serialise event for tag extraction: {e}");
            return Vec::new();
        }
    };

    let Some(tags_arr) = val.get("tags").and_then(|t| t.as_array()) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for tag in tags_arr {
        let Some(arr) = tag.as_array() else { continue };
        let kind = arr.first().and_then(|v| v.as_str()).unwrap_or("");
        if kind != "e" {
            continue;
        }
        let event_id = arr
            .get(1)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let relay_url = arr
            .get(2)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let marker = arr
            .get(3)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        if !event_id.is_empty() {
            out.push((event_id, relay_url, marker));
        }
    }
    out
}

/// Determine the immediate parent event ID for a given event following NIP-10.
fn resolve_parent_id(event: &Event) -> Option<String> {
    let e_tags = extract_e_tags(event);
    if e_tags.is_empty() {
        return None;
    }
    if let Some((id, _, _)) = e_tags.iter().find(|(_, _, m)| m == "reply") {
        return Some(id.clone());
    }
    if let Some((id, _, _)) = e_tags.iter().find(|(_, _, m)| m == "root") {
        return Some(id.clone());
    }
    e_tags.last().map(|(id, _, _)| id.clone())
}

// ── Tree builder ─────────────────────────────────────────────────────────────

/// Transform a flat, unsorted vector of Kind-1 Nostr events into a structured
/// NIP-10 comment tree.
pub fn build_thread_tree(events: Vec<Event>) -> ThreadTree {
    if events.is_empty() {
        return ThreadTree::default();
    }

    let event_map: HashMap<String, Event> =
        events.iter().map(|e| (e.id.to_hex(), e.clone())).collect();

    let parent_of: HashMap<String, Option<String>> = event_map
        .keys()
        .map(|id| {
            let event = &event_map[id];
            let parent = resolve_parent_id(event).filter(|pid| event_map.contains_key(pid));
            (id.clone(), parent)
        })
        .collect();

    let mut children_of: HashMap<String, Vec<String>> = HashMap::new();
    let mut root_ids: Vec<String> = Vec::new();

    for (id, maybe_parent) in &parent_of {
        match maybe_parent {
            Some(pid) => children_of.entry(pid.clone()).or_default().push(id.clone()),
            None => root_ids.push(id.clone()),
        }
    }

    root_ids.sort_by_key(|id| event_map[id].created_at);

    fn build_node(
        id: &str,
        depth: usize,
        event_map: &HashMap<String, Event>,
        children_of: &HashMap<String, Vec<String>>,
    ) -> ThreadNode {
        let event = event_map[id].clone();
        let mut node = ThreadNode::new(event, depth);
        if let Some(child_ids) = children_of.get(id) {
            let mut sorted = child_ids.clone();
            sorted.sort_by_key(|cid| event_map[cid].created_at);
            for child_id in &sorted {
                node.children
                    .push(build_node(child_id, depth + 1, event_map, children_of));
            }
        }
        node
    }

    let roots = root_ids
        .iter()
        .map(|id| build_node(id, 0, &event_map, &children_of))
        .collect();

    ThreadTree { roots }
}

// ── Sabha Engine ─────────────────────────────────────────────────────────────

pub type SabhaEventCallback = Box<dyn Fn(Event) + Send + Sync + 'static>;

/// Connects to Nostr relays and streams Kind-1 text notes.
pub struct SabhaEngine {
    client: Client,
}

impl SabhaEngine {
    pub async fn new(keys: &Keys) -> Result<Self, SabhaError> {
        let client = Client::new(keys.clone());
        for relay in DEFAULT_RELAYS {
            client
                .add_relay(*relay)
                .await
                .map_err(|e| SabhaError::RelayError(e.to_string()))?;
        }
        Ok(Self { client })
    }

    pub async fn add_relay(&self, url: &str) -> Result<(), SabhaError> {
        self.client
            .add_relay(url)
            .await
            .map_err(|e| SabhaError::RelayError(e.to_string()))?;
        Ok(())
    }

    pub async fn connect(&self) {
        self.client.connect().await;
        info!("Sabha engine connected to public relays");
    }

    pub async fn disconnect(&self) {
        self.client.disconnect().await;
    }

    /// Publish a text note to the public relay set.
    pub async fn publish_note(&self, content: &str) -> Result<EventId, SabhaError> {
        let output = self
            .client
            .send_event_builder(EventBuilder::text_note(content))
            .await
            .map_err(|e| SabhaError::RelayError(e.to_string()))?;
        info!(event_id = %output.id(), "Sabha note published");
        Ok(*output.id())
    }

    /// Subscribe to Kind-1 events since `since_secs` seconds ago.
    pub async fn subscribe_feed(
        &self,
        since_secs: u64,
        callback: SabhaEventCallback,
    ) -> Result<(), SabhaError> {
        let filter = Filter::new()
            .kind(Kind::TextNote)
            .since(Timestamp::now() - since_secs);

        self.client
            .subscribe(filter, None)
            .await
            .map_err(|e| SabhaError::SubscriptionError(e.to_string()))?;

        info!("Sabha feed subscription active");

        let callback = Arc::new(callback);
        self.client
            .handle_notifications(move |notification| {
                let cb = callback.clone();
                async move {
                    if let RelayPoolNotification::Event { event, .. } = notification {
                        if event.kind == Kind::TextNote {
                            debug!(event_id = %event.id, "Sabha event received");
                            cb(*event);
                        }
                    }
                    Ok::<bool, Box<dyn std::error::Error>>(false)
                }
            })
            .await
            .map_err(|e| SabhaError::SubscriptionError(e.to_string()))
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_bare_event(parent_hex: Option<&str>, marker: Option<&str>) -> Event {
        let keys = Keys::generate();
        let mut builder = EventBuilder::new(Kind::TextNote, "test content");

        if let Some(pid) = parent_hex {
            let eid = EventId::from_hex(pid).unwrap();
            if let Some(m) = marker {
                // Build a tagged event: ["e", "<pid>", "", "<marker>"]
                if let Ok(t) = Tag::parse(["e", pid, "", m]) {
                    builder = builder.tag(t);
                } else {
                    builder = builder.tag(Tag::event(eid));
                }
            } else {
                builder = builder.tag(Tag::event(eid));
            }
        }

        builder.sign_with_keys(&keys).unwrap()
    }

    #[test]
    fn empty_input_gives_empty_tree() {
        let tree = build_thread_tree(vec![]);
        assert!(tree.is_empty());
    }

    #[test]
    fn single_event_becomes_root() {
        let event = make_bare_event(None, None);
        let tree = build_thread_tree(vec![event]);
        assert_eq!(tree.roots.len(), 1);
        assert_eq!(tree.len(), 1);
    }

    #[test]
    fn parent_child_depth_is_correct() {
        let root = make_bare_event(None, None);
        let root_id = root.id.to_hex();

        let child = make_bare_event(Some(&root_id), Some("reply"));
        let tree = build_thread_tree(vec![child, root]);

        assert_eq!(tree.roots.len(), 1);
        assert_eq!(tree.roots[0].children.len(), 1);
        assert_eq!(tree.roots[0].children[0].depth, 1);
        assert_eq!(tree.len(), 2);
    }

    #[test]
    fn orphan_events_become_independent_roots() {
        let orphan_id = "a".repeat(64);
        let child = make_bare_event(Some(&orphan_id), Some("reply"));
        let tree = build_thread_tree(vec![child]);
        assert_eq!(tree.roots.len(), 1);
    }
}
