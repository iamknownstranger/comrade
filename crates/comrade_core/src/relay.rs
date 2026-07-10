/*!
 * Track 4 — NIP-65 Relay List Metadata & Outbox-Model Routing
 *
 * Connecting to a couple of static relays breaks the moment they go offline or
 * block a user. NIP-65 publishes, per user, the relays they *write* to and the
 * relays they *read* from (kind-10002 events with `r` tags). This module:
 *
 *  • parses kind-10002 events into a typed [`RelayList`];
 *  • builds your own kind-10002 event for publishing;
 *  • maintains a [`RelayRouter`] mapping pubkey → relay list and applies the
 *    NIP-65 *outbox model* to decide which relays to use:
 *      - to READ a user's posts        → that user's WRITE relays;
 *      - to DELIVER an event to a user → that user's READ relays.
 *
 * The routing core is pure logic with no I/O, so it is fully unit-testable.
 * [`GossipEngine`] wires it to a live `nostr_sdk::Client` for discovery and
 * dynamic pool reconfiguration.
 */

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use nostr_sdk::prelude::*;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use crate::error::GossipError;

/// NIP-65 relay list metadata event kind.
pub const RELAY_LIST_KIND: u16 = 10002;

// ── Relay policy & list ────────────────────────────────────────────────────────

/// Whether a relay is used for reading, writing, or both (NIP-65 `r` markers).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelayPolicy {
    Read,
    Write,
    ReadWrite,
}

impl RelayPolicy {
    fn reads(self) -> bool {
        matches!(self, RelayPolicy::Read | RelayPolicy::ReadWrite)
    }

    fn writes(self) -> bool {
        matches!(self, RelayPolicy::Write | RelayPolicy::ReadWrite)
    }

    /// Parse the optional NIP-65 marker. Absent/empty marker means read+write.
    fn from_marker(marker: &str) -> Self {
        match marker.trim() {
            "read" => RelayPolicy::Read,
            "write" => RelayPolicy::Write,
            _ => RelayPolicy::ReadWrite,
        }
    }
}

/// A single relay URL with its access policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayListEntry {
    pub url: String,
    pub policy: RelayPolicy,
}

/// A user's full NIP-65 relay list.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RelayList {
    pub entries: Vec<RelayListEntry>,
}

impl RelayList {
    /// Relays this user reads from (where to deliver events to reach them).
    pub fn read_relays(&self) -> Vec<&str> {
        self.entries
            .iter()
            .filter(|e| e.policy.reads())
            .map(|e| e.url.as_str())
            .collect()
    }

    /// Relays this user writes to (where to read their posts from).
    pub fn write_relays(&self) -> Vec<&str> {
        self.entries
            .iter()
            .filter(|e| e.policy.writes())
            .map(|e| e.url.as_str())
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

// ── Parsing & building NIP-65 events ───────────────────────────────────────────

/// Normalise a relay URL for stable de-duplication (lowercase, no trailing `/`).
fn normalise_url(url: &str) -> String {
    url.trim().trim_end_matches('/').to_lowercase()
}

/// Parse a kind-10002 event's `r` tags into a [`RelayList`].
///
/// Uses the event's canonical JSON form so it stays robust across nostr-sdk tag
/// API changes. Duplicate URLs are merged (last marker wins).
pub fn parse_relay_list(event: &Event) -> Result<RelayList, GossipError> {
    let val = serde_json::to_value(event)
        .map_err(|e| GossipError::ParseFailed(format!("serialise event: {e}")))?;

    let tags = val
        .get("tags")
        .and_then(|t| t.as_array())
        .ok_or_else(|| GossipError::ParseFailed("event has no tags array".into()))?;

    // Preserve insertion order while de-duplicating by normalised URL.
    let mut order: Vec<String> = Vec::new();
    let mut by_url: HashMap<String, RelayListEntry> = HashMap::new();

    for tag in tags {
        let Some(arr) = tag.as_array() else { continue };
        let name = arr.first().and_then(|v| v.as_str()).unwrap_or("");
        if name != "r" {
            continue;
        }
        let Some(raw_url) = arr.get(1).and_then(|v| v.as_str()) else {
            continue;
        };
        let url = normalise_url(raw_url);
        if url.is_empty() {
            continue;
        }
        let marker = arr.get(2).and_then(|v| v.as_str()).unwrap_or("");
        let policy = RelayPolicy::from_marker(marker);

        if !by_url.contains_key(&url) {
            order.push(url.clone());
        }
        by_url.insert(url.clone(), RelayListEntry { url, policy });
    }

    let entries = order
        .into_iter()
        .filter_map(|u| by_url.remove(&u))
        .collect();
    Ok(RelayList { entries })
}

/// Build a signed kind-10002 relay-list event from a set of entries.
pub fn build_relay_list_event(
    keys: &Keys,
    entries: &[RelayListEntry],
) -> Result<Event, GossipError> {
    let mut tags: Vec<Tag> = Vec::with_capacity(entries.len());
    for entry in entries {
        let tag = match entry.policy {
            RelayPolicy::ReadWrite => Tag::parse(["r", entry.url.as_str()]),
            RelayPolicy::Read => Tag::parse(["r", entry.url.as_str(), "read"]),
            RelayPolicy::Write => Tag::parse(["r", entry.url.as_str(), "write"]),
        }
        .map_err(|e| GossipError::ParseFailed(format!("build r tag: {e}")))?;
        tags.push(tag);
    }

    EventBuilder::new(Kind::from(RELAY_LIST_KIND), "")
        .tags(tags)
        .sign_with_keys(keys)
        .map_err(|e| GossipError::SigningFailed(e.to_string()))
}

// ── Router ─────────────────────────────────────────────────────────────────────

/// Default cap on relays selected per user to bound fan-out.
pub const DEFAULT_MAX_RELAYS_PER_USER: usize = 3;

/// Maps pubkeys to their NIP-65 relay lists and applies outbox-model routing.
#[derive(Debug, Clone)]
pub struct RelayRouter {
    lists: HashMap<String, RelayList>,
    /// `created_at` of the newest ingested kind-10002 per author, so a relay
    /// replaying an OLD replaceable event can never roll a user's list back.
    freshness: HashMap<String, u64>,
    fallback_relays: Vec<String>,
    max_relays_per_user: usize,
}

impl RelayRouter {
    /// Create a router with a fallback relay set used for users whose relay
    /// list is unknown.
    pub fn new(fallback_relays: Vec<String>) -> Self {
        Self {
            lists: HashMap::new(),
            freshness: HashMap::new(),
            fallback_relays: fallback_relays.iter().map(|r| normalise_url(r)).collect(),
            max_relays_per_user: DEFAULT_MAX_RELAYS_PER_USER,
        }
    }

    pub fn with_max_relays_per_user(mut self, max: usize) -> Self {
        self.max_relays_per_user = max.max(1);
        self
    }

    /// Record (or replace) a user's relay list unconditionally (trusted local
    /// input — e.g. the user editing their own list). Clears any recorded
    /// event freshness for the author.
    pub fn update(&mut self, pubkey_hex: impl Into<String>, list: RelayList) {
        let key = pubkey_hex.into();
        self.freshness.remove(&key);
        self.lists.insert(key, list);
    }

    /// Parse a kind-10002 event and record its author's relay list.
    ///
    /// Kind 10002 is a *replaceable* event: only the newest per author counts.
    /// An event older than the newest already ingested for that author is
    /// ignored, so replays cannot roll a relay list back.
    pub fn ingest_event(&mut self, event: &Event) -> Result<(), GossipError> {
        let list = parse_relay_list(event)?;
        let author = event.pubkey.to_hex();
        let created_at = event.created_at.as_secs();
        if let Some(&newest) = self.freshness.get(&author) {
            if created_at <= newest {
                debug!(author = %author, "gossip: ignoring stale relay-list event");
                return Ok(());
            }
        }
        debug!(author = %author, relays = list.entries.len(), "gossip: ingested relay list");
        self.freshness.insert(author.clone(), created_at);
        self.lists.insert(author, list);
        Ok(())
    }

    pub fn known_users(&self) -> usize {
        self.lists.len()
    }

    pub fn relay_list_for(&self, pubkey_hex: &str) -> Option<&RelayList> {
        self.lists.get(pubkey_hex)
    }

    /// Relays from which to READ a user's posts = their WRITE relays
    /// (falls back to the default set if unknown), capped.
    pub fn read_relays_for(&self, pubkey_hex: &str) -> Vec<String> {
        match self.lists.get(pubkey_hex) {
            Some(list) if !list.write_relays().is_empty() => {
                cap(list.write_relays(), self.max_relays_per_user)
            }
            _ => cap_owned(&self.fallback_relays, self.max_relays_per_user),
        }
    }

    /// Relays to which to WRITE to reach a user = their READ relays
    /// (falls back to the default set if unknown), capped.
    pub fn delivery_relays_for(&self, pubkey_hex: &str) -> Vec<String> {
        match self.lists.get(pubkey_hex) {
            Some(list) if !list.read_relays().is_empty() => {
                cap(list.read_relays(), self.max_relays_per_user)
            }
            _ => cap_owned(&self.fallback_relays, self.max_relays_per_user),
        }
    }

    /// Deduplicated union of read relays for a set of authors — the pool you
    /// connect to in order to fetch all their content.
    pub fn read_pool(&self, authors: &[String]) -> Vec<String> {
        let mut pool: BTreeSet<String> = BTreeSet::new();
        for author in authors {
            pool.extend(self.read_relays_for(author));
        }
        if pool.is_empty() {
            pool.extend(self.fallback_relays.iter().cloned());
        }
        pool.into_iter().collect()
    }

    /// Deduplicated union of delivery relays for a set of recipients — the pool
    /// you publish to in order to reach all of them.
    pub fn write_pool(&self, recipients: &[String]) -> Vec<String> {
        let mut pool: BTreeSet<String> = BTreeSet::new();
        for recipient in recipients {
            pool.extend(self.delivery_relays_for(recipient));
        }
        if pool.is_empty() {
            pool.extend(self.fallback_relays.iter().cloned());
        }
        pool.into_iter().collect()
    }
}

/// Cap a borrowed list of URLs, normalising and de-duplicating.
fn cap(urls: Vec<&str>, max: usize) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for u in urls {
        let n = normalise_url(u);
        if seen.insert(n.clone()) {
            out.push(n);
            if out.len() >= max {
                break;
            }
        }
    }
    out
}

fn cap_owned(urls: &[String], max: usize) -> Vec<String> {
    urls.iter().take(max).cloned().collect()
}

// ── Live gossip engine ─────────────────────────────────────────────────────────

/// Wires the [`RelayRouter`] to a live `nostr_sdk::Client` for NIP-65 discovery
/// and dynamic relay-pool reconfiguration.
pub struct GossipEngine {
    client: Client,
    keys: Keys,
    router: Arc<RwLock<RelayRouter>>,
}

impl GossipEngine {
    pub fn new(client: Client, keys: Keys, fallback_relays: Vec<String>) -> Self {
        Self {
            client,
            keys,
            router: Arc::new(RwLock::new(RelayRouter::new(fallback_relays))),
        }
    }

    /// Shared handle to the routing table (read by other engines for delivery).
    pub fn router(&self) -> Arc<RwLock<RelayRouter>> {
        self.router.clone()
    }

    /// Publish our own NIP-65 relay list so peers can discover where to reach us.
    pub async fn publish_my_relay_list(
        &self,
        entries: &[RelayListEntry],
    ) -> Result<EventId, GossipError> {
        let event = build_relay_list_event(&self.keys, entries)?;
        let output = self
            .client
            .send_event(&event)
            .await
            .map_err(|e| GossipError::RelayError(e.to_string()))?;
        info!(event_id = %output.id(), "gossip: published own relay list");
        Ok(*output.id())
    }

    /// Subscribe to kind-10002 events from `authors` and ingest them into the
    /// router as they arrive.
    pub async fn subscribe_relay_lists(&self, authors: Vec<PublicKey>) -> Result<(), GossipError> {
        let filter = Filter::new()
            .kind(Kind::from(RELAY_LIST_KIND))
            .authors(authors);

        self.client
            .subscribe(filter, None)
            .await
            .map_err(|e| GossipError::SubscriptionError(e.to_string()))?;

        info!("gossip: relay-list subscription active");

        let router = self.router.clone();
        self.client
            .handle_notifications(move |notification| {
                let router = router.clone();
                async move {
                    if let RelayPoolNotification::Event { event, .. } = notification {
                        if event.kind == Kind::from(RELAY_LIST_KIND) {
                            if let Err(e) = router.write().await.ingest_event(&event) {
                                warn!("gossip: failed to ingest relay list: {e}");
                            }
                        }
                    }
                    Ok::<bool, Box<dyn std::error::Error>>(false)
                }
            })
            .await
            .map_err(|e| GossipError::SubscriptionError(e.to_string()))
    }

    /// Reconfigure the client's connection pool to include every relay needed
    /// to read `authors`' content (outbox model). Returns the relays added.
    pub async fn connect_read_pool(&self, authors: &[String]) -> Result<Vec<String>, GossipError> {
        let pool = self.router.read().await.read_pool(authors);
        for relay in &pool {
            if let Err(e) = self.client.add_relay(relay.as_str()).await {
                warn!(relay = %relay, "gossip: failed to add relay: {e}");
            }
        }
        self.client.connect().await;
        info!(count = pool.len(), "gossip: read pool connected");
        Ok(pool)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn relay_list_event(keys: &Keys, tags: Vec<Vec<&str>>) -> Event {
        let parsed: Vec<Tag> = tags
            .into_iter()
            .map(|t| Tag::parse(t).expect("tag"))
            .collect();
        EventBuilder::new(Kind::from(RELAY_LIST_KIND), "")
            .tags(parsed)
            .sign_with_keys(keys)
            .expect("sign")
    }

    #[test]
    fn stale_relay_list_event_cannot_roll_back_newer_one() {
        let keys = Keys::generate();
        let newer = EventBuilder::new(Kind::from(RELAY_LIST_KIND), "")
            .tags([Tag::parse(["r", "wss://new.example"]).unwrap()])
            .custom_created_at(Timestamp::from(2_000_000_000u64))
            .sign_with_keys(&keys)
            .unwrap();
        let older = EventBuilder::new(Kind::from(RELAY_LIST_KIND), "")
            .tags([Tag::parse(["r", "wss://old.example"]).unwrap()])
            .custom_created_at(Timestamp::from(1_000_000_000u64))
            .sign_with_keys(&keys)
            .unwrap();

        let mut router = RelayRouter::new(vec![]);
        router.ingest_event(&newer).unwrap();
        // Replayed older event must be ignored (kind 10002 is replaceable).
        router.ingest_event(&older).unwrap();
        let list = router.relay_list_for(&keys.public_key().to_hex()).unwrap();
        assert_eq!(list.entries[0].url, "wss://new.example");

        // A manual (local, trusted) update still overrides.
        router.update(
            keys.public_key().to_hex(),
            RelayList {
                entries: vec![RelayListEntry {
                    url: "wss://manual.example".into(),
                    policy: RelayPolicy::ReadWrite,
                }],
            },
        );
        let list = router.relay_list_for(&keys.public_key().to_hex()).unwrap();
        assert_eq!(list.entries[0].url, "wss://manual.example");
    }

    #[test]
    fn parses_read_write_markers() {
        let keys = Keys::generate();
        let event = relay_list_event(
            &keys,
            vec![
                vec!["r", "wss://both.example"],
                vec!["r", "wss://read.example", "read"],
                vec!["r", "wss://write.example", "write"],
            ],
        );
        let list = parse_relay_list(&event).unwrap();
        assert_eq!(list.entries.len(), 3);

        let mut reads = list.read_relays();
        reads.sort();
        assert_eq!(reads, vec!["wss://both.example", "wss://read.example"]);

        let mut writes = list.write_relays();
        writes.sort();
        assert_eq!(writes, vec!["wss://both.example", "wss://write.example"]);
    }

    #[test]
    fn duplicate_urls_are_merged_last_wins() {
        let keys = Keys::generate();
        let event = relay_list_event(
            &keys,
            vec![
                vec!["r", "wss://dup.example", "read"],
                vec!["r", "wss://dup.example/", "write"], // trailing slash → same url
            ],
        );
        let list = parse_relay_list(&event).unwrap();
        assert_eq!(list.entries.len(), 1);
        assert_eq!(list.entries[0].policy, RelayPolicy::Write);
    }

    #[test]
    fn non_r_tags_are_ignored() {
        let keys = Keys::generate();
        let event = relay_list_event(
            &keys,
            vec![
                vec!["p", "deadbeef"],
                vec!["r", "wss://keep.example"],
                vec!["t", "hashtag"],
            ],
        );
        let list = parse_relay_list(&event).unwrap();
        assert_eq!(list.entries.len(), 1);
        assert_eq!(list.entries[0].url, "wss://keep.example");
    }

    #[test]
    fn build_then_parse_roundtrip() {
        let keys = Keys::generate();
        let entries = vec![
            RelayListEntry {
                url: "wss://a.example".into(),
                policy: RelayPolicy::ReadWrite,
            },
            RelayListEntry {
                url: "wss://b.example".into(),
                policy: RelayPolicy::Read,
            },
        ];
        let event = build_relay_list_event(&keys, &entries).unwrap();
        let parsed = parse_relay_list(&event).unwrap();
        assert_eq!(parsed.entries, entries);
    }

    #[test]
    fn outbox_model_read_uses_write_relays() {
        // To READ Alice's posts we connect to the relays Alice WRITES to.
        let mut router = RelayRouter::new(vec!["wss://fallback.example".into()]);
        router.update(
            "alice",
            RelayList {
                entries: vec![
                    RelayListEntry {
                        url: "wss://alice-write.example".into(),
                        policy: RelayPolicy::Write,
                    },
                    RelayListEntry {
                        url: "wss://alice-read.example".into(),
                        policy: RelayPolicy::Read,
                    },
                ],
            },
        );
        assert_eq!(
            router.read_relays_for("alice"),
            vec!["wss://alice-write.example".to_string()]
        );
    }

    #[test]
    fn outbox_model_delivery_uses_read_relays() {
        // To DELIVER to Alice we publish to the relays Alice READS from.
        let mut router = RelayRouter::new(vec!["wss://fallback.example".into()]);
        router.update(
            "alice",
            RelayList {
                entries: vec![
                    RelayListEntry {
                        url: "wss://alice-write.example".into(),
                        policy: RelayPolicy::Write,
                    },
                    RelayListEntry {
                        url: "wss://alice-read.example".into(),
                        policy: RelayPolicy::Read,
                    },
                ],
            },
        );
        assert_eq!(
            router.delivery_relays_for("alice"),
            vec!["wss://alice-read.example".to_string()]
        );
    }

    #[test]
    fn unknown_user_falls_back() {
        let router = RelayRouter::new(vec!["wss://fallback.example".into()]);
        assert_eq!(
            router.read_relays_for("nobody"),
            vec!["wss://fallback.example".to_string()]
        );
        assert_eq!(
            router.delivery_relays_for("nobody"),
            vec!["wss://fallback.example".to_string()]
        );
    }

    #[test]
    fn write_pool_unions_recipients_read_relays() {
        let mut router = RelayRouter::new(vec!["wss://fallback.example".into()]);
        router.update(
            "alice",
            RelayList {
                entries: vec![RelayListEntry {
                    url: "wss://alice-read.example".into(),
                    policy: RelayPolicy::Read,
                }],
            },
        );
        router.update(
            "bob",
            RelayList {
                entries: vec![RelayListEntry {
                    url: "wss://bob-read.example".into(),
                    policy: RelayPolicy::Read,
                }],
            },
        );
        let pool = router.write_pool(&["alice".to_string(), "bob".to_string()]);
        assert_eq!(
            pool,
            vec![
                "wss://alice-read.example".to_string(),
                "wss://bob-read.example".to_string()
            ]
        );
    }

    #[test]
    fn per_user_cap_is_enforced() {
        let mut router =
            RelayRouter::new(vec!["wss://fallback.example".into()]).with_max_relays_per_user(2);
        router.update(
            "alice",
            RelayList {
                entries: (0..5)
                    .map(|i| RelayListEntry {
                        url: format!("wss://r{i}.example"),
                        policy: RelayPolicy::Write,
                    })
                    .collect(),
            },
        );
        assert_eq!(router.read_relays_for("alice").len(), 2);
    }

    #[test]
    fn ingest_event_records_author() {
        let keys = Keys::generate();
        let event = relay_list_event(&keys, vec![vec!["r", "wss://x.example"]]);
        let mut router = RelayRouter::new(vec![]);
        router.ingest_event(&event).unwrap();
        assert_eq!(router.known_users(), 1);
        assert_eq!(
            router.read_relays_for(&keys.public_key().to_hex()),
            vec!["wss://x.example".to_string()]
        );
    }
}
