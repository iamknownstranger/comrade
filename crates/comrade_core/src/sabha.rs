/*!
 * Milestone 3a — Sabha: Public Microblogging Engine ("Chitthi Feed")
 *
 * Connects to public Nostr relays, subscribes to Kind-1 text notes (each one a
 * public *Chitthi* — a letter to the world), and parses a flat unsorted stream
 * of events into a structured NIP-10 `ChitthiThread` using recursive
 * parent-child resolution.
 *
 * Nomenclature: at the application layer a public post is a **Chitthi** and a
 * reply tree is a **ChitthiThread**. The Nostr protocol constant `Kind::TextNote`
 * is left untouched — only the execution/timeline layer adopts the Chitthi name.
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
pub struct ChitthiNode {
    /// The raw Nostr event at this tree position.
    pub event: Event,
    /// Zero-indexed depth from a root node.
    pub depth: usize,
    /// Direct replies to this node, sorted by created_at ascending.
    pub children: Vec<ChitthiNode>,
}

impl ChitthiNode {
    fn new(event: Event, depth: usize) -> Self {
        Self {
            event,
            depth,
            children: Vec::new(),
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct ChitthiThread {
    /// Top-level events — those that have no parent within the local set.
    pub roots: Vec<ChitthiNode>,
}

impl ChitthiThread {
    /// Total number of events across all levels.
    pub fn len(&self) -> usize {
        fn count(nodes: &[ChitthiNode]) -> usize {
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
pub fn build_chitthi_thread(events: Vec<Event>) -> ChitthiThread {
    if events.is_empty() {
        return ChitthiThread::default();
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
    ) -> ChitthiNode {
        let event = event_map[id].clone();
        let mut node = ChitthiNode::new(event, depth);
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

    ChitthiThread { roots }
}

// ── Sabha Engine ─────────────────────────────────────────────────────────────

pub type ChitthiCallback = Box<dyn Fn(Event) + Send + Sync + 'static>;

/// Connects to Nostr relays and streams Kind-1 text notes as public Chitthis.
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

    /// Broadcast a Chitthi (Kind-1 text note) to the public relay set.
    pub async fn broadcast_chitthi(&self, content: &str) -> Result<EventId, SabhaError> {
        self.broadcast_chitthi_reply(content, None).await
    }

    /// Broadcast a Chitthi, optionally as a NIP-10 reply to `reply_to`.
    ///
    /// When `reply_to` is `Some`, an `["e", <id>, "", "reply"]` tag is attached so
    /// relays and clients can thread the response under its parent. This is the
    /// engine entry point behind the IPC bridge's `broadcast_chitthi` command.
    pub async fn broadcast_chitthi_reply(
        &self,
        content: &str,
        reply_to: Option<EventId>,
    ) -> Result<EventId, SabhaError> {
        let mut builder = EventBuilder::text_note(content);
        if let Some(parent) = reply_to {
            let tag = Tag::parse(["e", parent.to_hex().as_str(), "", "reply"])
                .map_err(|e| SabhaError::ParseError(e.to_string()))?;
            builder = builder.tags([tag]);
        }
        let output = self
            .client
            .send_event_builder(builder)
            .await
            .map_err(|e| SabhaError::RelayError(e.to_string()))?;
        info!(event_id = %output.id(), reply = reply_to.is_some(), "Chitthi broadcast to Sabha");
        Ok(*output.id())
    }

    /// Publish (or update) our public profile — a Kind-0 metadata event carrying
    /// the chosen @handle — so peers can discover this identity by name.
    ///
    /// Handles are display names, not unique identifiers: the network cannot
    /// stop two people from publishing the same handle. Identity remains the
    /// keypair; callers must always bind contacts to the npub, never the handle.
    pub async fn publish_profile(
        &self,
        name: &str,
        about: Option<&str>,
    ) -> Result<EventId, SabhaError> {
        let mut metadata = Metadata::new().name(name).display_name(name);
        if let Some(about) = about {
            metadata = metadata.about(about);
        }
        let output = self
            .client
            .send_event_builder(EventBuilder::metadata(&metadata))
            .await
            .map_err(|e| SabhaError::RelayError(e.to_string()))?;
        info!(event_id = %output.id(), name, "profile published");
        Ok(*output.id())
    }

    /// Best-effort people search: query Kind-0 profiles via NIP-50 full-text
    /// search on whichever connected relays support it (relay.nostr.band in the
    /// default set does; the rest silently return nothing).
    ///
    /// Returns at most `limit` profiles, newest metadata per author, as
    /// `(author, metadata)` pairs. An empty result is normal — it means no
    /// search-capable relay knew the handle, not that the person doesn't exist.
    pub async fn search_profiles(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<(PublicKey, Metadata)>, SabhaError> {
        let filter = Filter::new()
            .kind(Kind::Metadata)
            .search(query)
            .limit(limit * 3); // headroom: duplicates collapse per author below
        let events = self
            .client
            .fetch_events(filter, std::time::Duration::from_secs(8))
            .await
            .map_err(|e| SabhaError::RelayError(e.to_string()))?;

        // Keep only the newest Kind-0 per author.
        let mut newest: HashMap<PublicKey, Event> = HashMap::new();
        for event in events.into_iter() {
            match newest.get(&event.pubkey) {
                Some(existing) if existing.created_at >= event.created_at => {}
                _ => {
                    newest.insert(event.pubkey, event);
                }
            }
        }
        let mut profiles: Vec<(PublicKey, Metadata)> = newest
            .into_values()
            .filter_map(|e| Metadata::from_json(&e.content).ok().map(|m| (e.pubkey, m)))
            .collect();
        profiles.sort_by_key(|a| profile_sort_key(&a.1));
        profiles.truncate(limit);
        Ok(profiles)
    }

    /// Subscribe to the Chitthi feed (Kind-1 events) since `since_secs` seconds ago.
    pub async fn subscribe_chitthi_feed(
        &self,
        since_secs: u64,
        callback: ChitthiCallback,
    ) -> Result<(), SabhaError> {
        let filter = Filter::new()
            .kind(Kind::TextNote)
            .since(Timestamp::now() - since_secs);

        self.client
            .subscribe(filter, None)
            .await
            .map_err(|e| SabhaError::SubscriptionError(e.to_string()))?;

        info!("Chitthi feed subscription active");

        let callback = Arc::new(callback);
        self.client
            .handle_notifications(move |notification| {
                let cb = callback.clone();
                async move {
                    if let RelayPoolNotification::Event { event, .. } = notification {
                        if event.kind == Kind::TextNote {
                            debug!(event_id = %event.id, "Chitthi received");
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

/// Stable ordering for search results: named profiles first, then by name.
fn profile_sort_key(m: &Metadata) -> String {
    m.name.clone().unwrap_or_else(|| "\u{10FFFF}".to_string())
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
        let tree = build_chitthi_thread(vec![]);
        assert!(tree.is_empty());
    }

    #[test]
    fn single_event_becomes_root() {
        let event = make_bare_event(None, None);
        let tree = build_chitthi_thread(vec![event]);
        assert_eq!(tree.roots.len(), 1);
        assert_eq!(tree.len(), 1);
    }

    #[test]
    fn parent_child_depth_is_correct() {
        let root = make_bare_event(None, None);
        let root_id = root.id.to_hex();

        let child = make_bare_event(Some(&root_id), Some("reply"));
        let tree = build_chitthi_thread(vec![child, root]);

        assert_eq!(tree.roots.len(), 1);
        assert_eq!(tree.roots[0].children.len(), 1);
        assert_eq!(tree.roots[0].children[0].depth, 1);
        assert_eq!(tree.len(), 2);
    }

    #[test]
    fn orphan_events_become_independent_roots() {
        let orphan_id = "a".repeat(64);
        let child = make_bare_event(Some(&orphan_id), Some("reply"));
        let tree = build_chitthi_thread(vec![child]);
        assert_eq!(tree.roots.len(), 1);
    }
}
