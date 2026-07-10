/*!
 * Milestone 6 — Comrade CLI Harness
 *
 * Text-based workspace switcher and interactive validation shell that wires
 * every engine to a single entry point.  Demonstrates smooth transitions
 * between Base mode, Off-Grid Travel (Saathi mesh), and the Sakha/Sakhi
 * Couple Sandbox via the Progressive-Disclosure state machine.
 */

use std::io::{self, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use comrade_core::{
    companion::{prompt_for, scan_safety, CompanionMode, EntrySource, Insights, JournalEntry},
    crypto::KeyProfile,
    sabha::build_chitthi_thread,
    vault::{build_pay_regex, extract_upi_intents},
};
use comrade_state::{AppWorkspace, PairRole, RuntimeContext, StorageStatus};
use comrade_storage::{Chitthi, EncryptedStore, StorageError, StoredIdentity};
use tokio::sync::RwLock;
use tracing::warn;
use tracing_subscriber::EnvFilter;

/// Default on-disk location for the encrypted store.
const STORE_PATH: &str = "comrade-data";

/// sled tree holding the private companion journal (anonymous, encrypted).
const JOURNAL_TREE: &str = "companion_journal";

// ── Shared application state ─────────────────────────────────────────────────

struct AppState {
    ctx: RuntimeContext,
    profile: KeyProfile,
    partner_profile: Option<KeyProfile>,
    /// Encrypted local store, present only after `unlock <PIN>`.
    store: Option<EncryptedStore>,
}

impl AppState {
    fn new(profile: KeyProfile) -> Self {
        Self {
            ctx: RuntimeContext::new(),
            profile,
            partner_profile: None,
            store: None,
        }
    }
}

// ── CLI helpers ──────────────────────────────────────────────────────────────

fn print_banner() {
    println!();
    println!("╔══════════════════════════════════════════════════════════╗");
    println!("║          C O M R A D E  —  your quiet companion          ║");
    println!("║   Journal · Vent · Reflect · Brainstorm — private & local ║");
    println!("╚══════════════════════════════════════════════════════════╝");
    println!();
}

fn print_workspace_header(ws: &AppWorkspace) {
    println!();
    println!("┌─────────────────────────────────────────────────────────┐");
    println!("│  Workspace: {:45}│", ws.label());
    println!(
        "│  Relays: {:2}  |  Mesh: {:3}  |  Sandbox: {:3}            │",
        if ws.is_relay_connected() { "ON" } else { "OFF" },
        if ws.is_mesh_active() { "ON" } else { "OFF" },
        if ws.is_couple_sandbox() { "ON" } else { "OFF" },
    );
    println!("└─────────────────────────────────────────────────────────┘");
}

fn print_help() {
    println!();
    println!("  Commands:");
    println!("  ─────────────────────────────────────────────────────────");
    println!("  Companion (private, anonymous, on-device):");
    println!("  journal <text> — write anything to your private journal");
    println!("  vent <text>    — unload; the companion just listens");
    println!("  brainstorm [t] — idea prompts (writes if you add text)");
    println!("  reflect [text] — gentle reflection prompt / entry");
    println!("  mood <-2..2> [text] — log how you feel (optionally note it)");
    println!("  entries        — read your journal (newest first)");
    println!("  insights       — streaks, mood trend, top tags");
    println!("  ─────────────────────────────────────────────────────────");
    println!("  ws         — show current workspace");
    println!("  base       — switch to Base mode (Sabha + Vault)");
    println!("  travel     — switch to Off-Grid Travel (Saathi mesh)");
    println!("  pair sakha — enter Couple Sandbox as Sakha");
    println!("  pair sakhi — enter Couple Sandbox as Sakhi");
    println!("  back       — step back to previous workspace");
    println!("  keygen     — generate a new identity keypair");
    println!("  keys       — display current npub / nsec");
    println!("  partner    — generate a partner keypair (for demo pairing)");
    println!("  dh         — compute DH shared secret with partner");
    println!("  tree/feed  — demo: build a ChitthiThread + cache it (Sabha timeline)");
    println!("  cache      — render the Chitthi feed from the encrypted store (offline)");
    println!("  pay <msg>  — demo: extract UPI /pay intents from text");
    println!("  ledger     — demo: append an entry and show CRDT ledger");
    println!("  relays     — demo: NIP-65 outbox-model relay routing");
    println!("  media      — demo: NIP-94/96 encrypted media pipeline");
    println!("  call       — demo: audio/video call signaling handshake (Pukar)");
    println!("  voicemsg   — demo: encrypted voice message with duration (NIP-94)");
    println!("  unlock <PIN> — open the encrypted local store with a PIN");
    println!("  save       — persist current identity to the encrypted store");
    println!("  load       — load the saved identity from the encrypted store");
    println!("  help       — show this help");
    println!("  exit / q   — quit");
    println!();
}

fn read_line(prompt: &str) -> io::Result<String> {
    print!("{prompt}");
    io::stdout().flush()?;
    let mut buf = String::new();
    // 0 bytes read = EOF (Ctrl-D / exhausted pipe): surface it as an error so
    // callers exit instead of spinning on empty prompts forever.
    if io::stdin().read_line(&mut buf)? == 0 {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "stdin closed"));
    }
    Ok(buf.trim().to_string())
}

// ── Demo scenarios ───────────────────────────────────────────────────────────

/// Map a raw Nostr Kind-1 event into a persistable [`Chitthi`] cache row.
fn event_to_chitthi(event: &nostr_sdk::Event, reply_to: Option<String>) -> Chitthi {
    Chitthi {
        id: event.id.to_hex(),
        author_npub: event.pubkey.to_hex(),
        content: event.content.clone(),
        created_at: event.created_at.as_secs(),
        reply_to,
    }
}

async fn demo_chitthi_feed(state: &Arc<RwLock<AppState>>) {
    use comrade_core::sabha::ChitthiNode;
    use nostr_sdk::prelude::*;

    println!("  Building a sample NIP-10 ChitthiThread …");

    let keys = Keys::generate();
    let root = EventBuilder::new(Kind::TextNote, "Root Chitthi: Namaste from Sabha!")
        .sign_with_keys(&keys)
        .expect("sign root");

    let root_id = root.id.to_hex();
    let reply_tag = Tag::parse(["e", root_id.as_str(), "", "reply"]).unwrap_or(Tag::event(root.id));

    let reply = EventBuilder::new(Kind::TextNote, "Reply Chitthi")
        .tags([reply_tag])
        .sign_with_keys(&keys)
        .expect("sign reply");

    // Persist incoming Chitthis to the encrypted cache if the store is unlocked.
    // In production this very logic is the body of the SabhaEngine
    // subscribe_chitthi_feed callback firing inside the Tokio notification loop.
    {
        let guard = state.read().await;
        if let Some(store) = guard.store.as_ref() {
            let rows = [
                event_to_chitthi(&root, None),
                event_to_chitthi(&reply, Some(root_id.clone())),
            ];
            let mut saved = 0;
            for row in &rows {
                match store.cache_chitthi(row) {
                    Ok(()) => saved += 1,
                    Err(e) => println!("  Cache write failed: {e}"),
                }
            }
            let _ = store.flush();
            println!("  Persisted {saved} Chitthi(s) to the encrypted cache.");
        } else {
            println!("  (store locked — run 'unlock <PIN>' to persist this feed offline)");
        }
    }

    let thread = build_chitthi_thread(vec![root, reply]);

    fn print_node(node: &ChitthiNode) {
        println!(
            "  {} [depth {}] {} …",
            "  ".repeat(node.depth),
            node.depth,
            &node.event.id.to_hex()[..16],
        );
        for child in &node.children {
            print_node(child);
        }
    }

    println!(
        "  ── Sabha Timeline (Chitthis) — {} total ─────────────────",
        thread.len()
    );
    for root in &thread.roots {
        print_node(root);
    }
}

/// Render the Chitthi feed straight from the encrypted on-disk cache — the
/// offline / cold-start path that needs no relay connection.
async fn show_cached_feed(state: &Arc<RwLock<AppState>>) {
    let guard = state.read().await;
    let Some(store) = guard.store.as_ref() else {
        println!("  Run 'unlock <PIN>' first to open the encrypted cache.");
        return;
    };
    match store.chitthi_cache() {
        Ok(feed) if feed.is_empty() => {
            println!("  Chitthi cache is empty — run 'feed' once to populate it.")
        }
        Ok(feed) => {
            println!(
                "  ── Cached Sabha Timeline (offline) — {} Chitthi(s) ──────",
                feed.len()
            );
            for c in &feed {
                let kind = if c.reply_to.is_some() {
                    "reply"
                } else {
                    "root "
                };
                println!("  [{kind}] {}  {}", &c.id[..16.min(c.id.len())], c.content);
            }
        }
        Err(e) => println!("  Failed to read cache: {e}"),
    }
}

fn demo_pay_extraction(text: &str) {
    let re = match build_pay_regex() {
        Ok(r) => r,
        Err(e) => {
            println!("  Regex error: {e}");
            return;
        }
    };
    let intents = extract_upi_intents(text, &re);
    if intents.is_empty() {
        println!("  No /pay commands detected in: {:?}", text);
    } else {
        for intent in &intents {
            println!("  ─ Amount : ₹{:.2}", intent.amount_inr);
            println!("  ─ VPA    : {}", intent.vpa);
            println!("  ─ URI    : {}", intent.uri);
        }
    }
}

async fn demo_ledger(state: &Arc<RwLock<AppState>>) {
    use comrade_core::sakha::{LedgerEntry, SakhaEngine};

    let guard = state.read().await;
    if guard.partner_profile.is_none() {
        println!("  Run 'partner' first to generate a demo partner keypair.");
        return;
    }
    let our_keys = guard.profile.keys.clone();
    let partner_keys = guard.partner_profile.as_ref().unwrap().keys.clone();
    drop(guard);

    let mut engine = match SakhaEngine::new(&our_keys, vec![]).await {
        Ok(e) => e,
        Err(e) => {
            println!("  Sakha engine init failed: {e}");
            return;
        }
    };

    if let Err(e) = engine.pair_with(partner_keys.public_key()) {
        println!("  Pairing failed: {e}");
        return;
    }

    let entry = LedgerEntry::new("Demo dinner", 420.0, "Sakha");
    if let Err(e) = engine.add_entry(entry).await {
        println!("  Add entry failed: {e}");
        return;
    }

    let ledger = engine.read_ledger().await;
    println!("  ── Hisab-Kitab Ledger ──────────────────────────────────");
    for line in ledger.lines() {
        println!("  {line}");
    }
    println!("  ────────────────────────────────────────────────────────");
}

fn demo_relay_routing() {
    use comrade_core::relay::{RelayList, RelayListEntry, RelayPolicy, RelayRouter};

    println!("  Demonstrating the NIP-65 outbox model …");
    let mut router = RelayRouter::new(vec!["wss://relay.damus.io".into()]);

    // Two friends advertise different read/write relays.
    router.update(
        "alice",
        RelayList {
            entries: vec![
                RelayListEntry {
                    url: "wss://alice.write".into(),
                    policy: RelayPolicy::Write,
                },
                RelayListEntry {
                    url: "wss://alice.read".into(),
                    policy: RelayPolicy::Read,
                },
            ],
        },
    );
    router.update(
        "bob",
        RelayList {
            entries: vec![RelayListEntry {
                url: "wss://bob.inbox".into(),
                policy: RelayPolicy::ReadWrite,
            }],
        },
    );

    println!(
        "  To READ Alice's posts  → {:?}",
        router.read_relays_for("alice")
    );
    println!(
        "  To DELIVER to Alice    → {:?}",
        router.delivery_relays_for("alice")
    );
    println!(
        "  Write pool for [Alice, Bob] (where to publish to reach both):\n    {:?}",
        router.write_pool(&["alice".to_string(), "bob".to_string()])
    );
    println!(
        "  Unknown user 'carol' falls back to defaults → {:?}",
        router.delivery_relays_for("carol")
    );
}

/// Walk both sides of a call through the real Pukar state machines: offer →
/// ring → accept → ICE → connect → hang-up, plus the busy path. This is the
/// exact logic the Android/desktop WebRTC layers drive over the network.
fn demo_call_signaling() {
    use comrade_core::pukar::{CallEvent, CallManager, CallMedia, CallSignal, RejectReason};

    let now = 1_000u64;
    let mut alice = CallManager::new();
    let mut bob = CallManager::new();
    // Incoming calls are deny-by-default (consent gate); the demo peers
    // explicitly allow anyone, as a real app would allow saved contacts.
    alice.set_policy(comrade_core::pukar::CallPolicy::AllowAll);
    bob.set_policy(comrade_core::pukar::CallPolicy::AllowAll);
    println!("  Demonstrating Pukar call signaling (SDP/ICE are platform blobs) …");

    // Alice places a video call to Bob.
    let (session, offer) =
        match alice.place_call("bob-pubkey", CallMedia::Video, "<sdp-offer>", now) {
            Ok(r) => r,
            Err(e) => {
                println!("  place_call failed: {e}");
                return;
            }
        };
    println!("  Alice → Offer   (call {}…, video)", &session.call_id[..8]);

    // Bob's device rings.
    let (events, _) = bob.handle_signal("alice-pubkey", offer, now);
    if let Some(CallEvent::IncomingCall { call, .. }) = events.first() {
        println!("  Bob   ← ringing (from {}, {:?})", call.peer, call.media);
    }

    // Carol calls Bob while he is ringing — auto-busy.
    let carol_offer = CallSignal::Offer {
        call_id: "carol-call-id".into(),
        media: CallMedia::Audio,
        sdp: "<sdp>".into(),
    };
    let (_, reply) = bob.handle_signal("carol-pubkey", carol_offer, now + 1);
    if let Some(CallSignal::Reject {
        reason: RejectReason::Busy,
        ..
    }) = reply
    {
        println!("  Carol ← Reject(busy) — Bob's call is undisturbed");
    }

    // Bob accepts; Alice applies the answer.
    let (answer, _withheld_ice) = match bob.accept(&session.call_id, "<sdp-answer>", now + 3) {
        Ok(r) => r,
        Err(e) => {
            println!("  accept failed: {e}");
            return;
        }
    };
    println!("  Bob   → Answer");
    let (events, _) = alice.handle_signal("bob-pubkey", answer, now + 3);
    if matches!(events.first(), Some(CallEvent::CallAnswered { .. })) {
        println!("  Alice ← answered → connecting");
    }

    // Trickle ICE, then both sides report media up.
    if let Ok(ice) = alice.local_ice(&session.call_id, "<candidate>", Some("0".into()), Some(0)) {
        let _ = bob.handle_signal("alice-pubkey", ice, now + 4);
        println!("  Alice → Ice, Bob applies candidate");
    }
    let _ = alice.mark_connected(&session.call_id, now + 5);
    let _ = bob.mark_connected(&session.call_id, now + 5);
    println!("  Both  — media flowing (Active)");

    // Alice hangs up after a minute of talk.
    match alice.hangup(&session.call_id, now + 65) {
        Ok(end) => {
            let _ = bob.handle_signal("alice-pubkey", end, now + 65);
            println!("  Alice → End(hangup)");
        }
        Err(e) => println!("  hangup failed: {e}"),
    }

    println!("  ── Call logs ────────────────────────────────────────────");
    for (who, mgr) in [("Alice", &alice), ("Bob", &bob)] {
        for c in mgr.call_log() {
            println!(
                "  {who}: {:?} {:?} — {:?}, talk {}s",
                c.direction,
                c.media,
                c.cause.unwrap_or(comrade_core::pukar::EndCause::Failed),
                c.duration_secs().unwrap_or(0),
            );
        }
    }
}

/// Record → encrypt → upload → describe → fetch → decrypt a voice message.
async fn demo_voice_message() {
    use comrade_core::crypto::KeyProfile;
    use comrade_core::media::{decrypt_media, parse_file_metadata, InMemoryUploader, MediaEngine};

    println!("  Demonstrating an encrypted voice message …");
    let keys = match KeyProfile::generate() {
        Ok(p) => p.keys.clone(),
        Err(e) => {
            println!("  Keygen failed: {e}");
            return;
        }
    };
    let uploader = InMemoryUploader::new("https://blossom.example");
    let engine = MediaEngine::new(uploader.clone(), keys);

    let audio = b"OggS pretend-opus-voice-note-bytes";
    let key = [0x42u8; 32]; // in practice: per-file random, shared via E2E DM
    let (event, secret) = match engine
        .share_voice_message(audio, "audio/ogg", 17, &key)
        .await
    {
        Ok(r) => r,
        Err(e) => {
            println!("  Share failed: {e}");
            return;
        }
    };

    match parse_file_metadata(&event) {
        Ok(meta) => {
            println!("  url      : {}", meta.url);
            println!("  mime     : {}", meta.mime_type);
            println!("  duration : {}s", meta.duration_secs.unwrap_or(0));
            if let Some(blob) = uploader.fetch(&meta.url).await {
                match decrypt_media(&blob, &secret) {
                    Ok(rec) => println!(
                        "  Recipient decrypted {} bytes — matches original: {}",
                        rec.len(),
                        rec == audio
                    ),
                    Err(e) => println!("  Decrypt failed: {e}"),
                }
            }
        }
        Err(e) => println!("  Metadata parse failed: {e}"),
    }
}

async fn demo_media() {
    use comrade_core::crypto::KeyProfile;
    use comrade_core::media::{decrypt_media, parse_file_metadata, InMemoryUploader, MediaEngine};

    println!("  Demonstrating encrypted media (NIP-94/96) …");

    let keys = match KeyProfile::generate() {
        Ok(p) => p.keys.clone(),
        Err(e) => {
            println!("  Keygen failed: {e}");
            return;
        }
    };
    let uploader = InMemoryUploader::new("https://blossom.example");
    let engine = MediaEngine::new(uploader.clone(), keys);

    let photo = b"\xFF\xD8\xFF pretend this is a JPEG \x00\x01\x02";
    let key = [0x5Au8; 32]; // in practice: DH-derived (couples) or per-file (vault)

    let (event, secret) = match engine
        .share_encrypted(photo, "image/jpeg", "sunset over the hills", &key)
        .await
    {
        Ok(r) => r,
        Err(e) => {
            println!("  Media share failed: {e}");
            return;
        }
    };

    let meta = match parse_file_metadata(&event) {
        Ok(m) => m,
        Err(e) => {
            println!("  Metadata parse failed: {e}");
            return;
        }
    };

    println!("  ── NIP-94 file event (public) ──────────────────────────");
    println!("  url     : {}", meta.url);
    println!("  mime    : {}", meta.mime_type);
    println!("  x  (enc): {}", meta.sha256_hex);
    println!(
        "  ox (orig): {}",
        meta.original_sha256_hex.as_deref().unwrap_or("-")
    );
    println!("  caption : {}", meta.caption);
    println!("  (decryption key travels out-of-band, NOT in this event)");

    // Recipient side: fetch the opaque blob and decrypt with the out-of-band secret.
    match uploader.fetch(&meta.url).await {
        Some(blob) => match decrypt_media(&blob, &secret) {
            Ok(recovered) => {
                let ok = recovered == photo;
                println!(
                    "  Recipient decrypted {} bytes — matches original: {ok}",
                    recovered.len()
                );
            }
            Err(e) => println!("  Decrypt failed: {e}"),
        },
        None => println!("  Blob not found at URL"),
    }
    println!("  ────────────────────────────────────────────────────────");
}

// ── Companion: private, anonymous journal ─────────────────────────────────────

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

/// A locally-unique, identity-free entry id (`<unix_nanos>-<seq>`).
fn companion_id() -> String {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    format!("{nanos}-{}", SEQ.fetch_add(1, Ordering::Relaxed))
}

/// Print a crisis-safety notice with resources when an entry looks concerning.
/// We never block writing — we just make sure help is one glance away.
fn print_safety_if_concerning(body: &str) {
    let assessment = scan_safety(body);
    if !assessment.concerning {
        return;
    }
    println!();
    println!("  ┌─ You are not alone ────────────────────────────────────┐");
    if let Some(msg) = &assessment.message {
        for line in wrap(msg, 54) {
            println!("  │ {line:54} │");
        }
    }
    println!("  ├────────────────────────────────────────────────────────┤");
    for r in &assessment.resources {
        for line in wrap(&format!("{} — {} ({})", r.region, r.name, r.contact), 54) {
            println!("  │ {line:54} │");
        }
    }
    println!("  └────────────────────────────────────────────────────────┘");
    println!("  (Comrade is not a clinician — please reach out to a person.)");
}

/// Naive word-wrap for the fixed-width safety box.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut cur = String::new();
    for word in text.split_whitespace() {
        if cur.is_empty() {
            cur = word.to_string();
        } else if cur.len() + 1 + word.len() <= width {
            cur.push(' ');
            cur.push_str(word);
        } else {
            lines.push(std::mem::take(&mut cur));
            cur = word.to_string();
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    lines
}

/// Write an anonymous journal entry into the encrypted store.
async fn companion_write(
    state: &Arc<RwLock<AppState>>,
    mode: CompanionMode,
    body: &str,
    mood: Option<i8>,
) {
    if body.trim().is_empty() && mood.is_none() {
        println!("  Nothing to save — type something after the command.");
        return;
    }
    let guard = state.read().await;
    let Some(store) = guard.store.as_ref() else {
        println!("  Run 'unlock <PIN>' first — your journal is encrypted at rest.");
        return;
    };

    let mut entry = JournalEntry::new(companion_id(), now_secs(), mode, EntrySource::Typed, body);
    if let Some(m) = mood {
        entry = entry.with_mood(m);
    }

    match store
        .put(JOURNAL_TREE, &entry.id, &entry)
        .and_then(|()| store.flush())
    {
        Ok(()) => {
            println!(
                "  Saved to your {} (anonymous, on-device only).",
                mode.label()
            );
            let seed = store
                .keys(JOURNAL_TREE)
                .map(|k| k.len() as u64)
                .unwrap_or(0);
            println!("  Prompt: {}", prompt_for(mode, seed));
        }
        Err(e) => println!("  Could not save entry: {e}"),
    }
    drop(guard);
    print_safety_if_concerning(body);
}

/// Show a supportive prompt for a mode without writing anything.
async fn companion_prompt(state: &Arc<RwLock<AppState>>, mode: CompanionMode) {
    let guard = state.read().await;
    let seed = guard
        .store
        .as_ref()
        .and_then(|s| s.keys(JOURNAL_TREE).ok())
        .map(|k| k.len() as u64)
        .unwrap_or(0);
    println!("  [{}] {}", mode.label(), prompt_for(mode, seed));
}

/// List the private journal, newest first.
async fn companion_entries(state: &Arc<RwLock<AppState>>) {
    let guard = state.read().await;
    let Some(store) = guard.store.as_ref() else {
        println!("  Run 'unlock <PIN>' first to open your encrypted journal.");
        return;
    };
    let mut entries: Vec<JournalEntry> = match store.values(JOURNAL_TREE) {
        Ok(e) => e,
        Err(e) => {
            println!("  Could not read journal: {e}");
            return;
        }
    };
    if entries.is_empty() {
        println!("  Your journal is empty. Try 'journal <anything on your mind>'.");
        return;
    }
    entries.sort_by(|a, b| {
        b.created_at
            .cmp(&a.created_at)
            .then_with(|| b.id.cmp(&a.id))
    });
    println!(
        "  ── Your Journal — {} entry(ies) ─────────────────────────",
        entries.len()
    );
    for e in &entries {
        let mood = e.mood.map(|m| format!(" mood:{m:+}")).unwrap_or_default();
        let tags = if e.tags.is_empty() {
            String::new()
        } else {
            format!("  [{}]", e.tags.join(", "))
        };
        println!("  · {:<10}{mood}{tags}", e.mode.label());
        println!("    {}", e.body);
    }
}

/// Print on-device journaling insights.
async fn companion_insights(state: &Arc<RwLock<AppState>>) {
    let guard = state.read().await;
    let Some(store) = guard.store.as_ref() else {
        println!("  Run 'unlock <PIN>' first.");
        return;
    };
    let entries: Vec<JournalEntry> = store.values(JOURNAL_TREE).unwrap_or_default();
    let i = Insights::from_entries(&entries, now_secs());
    println!("  ── Companion Insights ───────────────────────────────────");
    println!("  Entries total     : {}", i.total);
    println!("  Current streak    : {} day(s)", i.current_streak_days);
    println!("  This week         : {}", i.entries_this_week);
    match i.avg_mood_recent {
        Some(m) => println!("  Mood (7-day avg)  : {m:+.1}  (−2 low … +2 good)"),
        None => println!("  Mood (7-day avg)  : —"),
    }
    if !i.top_tags.is_empty() {
        let tags: Vec<String> = i
            .top_tags
            .iter()
            .take(5)
            .map(|(t, n)| format!("#{t}×{n}"))
            .collect();
        println!("  Top tags          : {}", tags.join("  "));
    }
}

// ── Startup identity bootstrap (Milestone 4) ──────────────────────────────────

/// Decide the startup identity.
///
/// If an encrypted store already exists on disk we prompt for the passphrase and
/// restore the saved profile from it, instead of minting a throwaway keypair.
/// Otherwise we mint a fresh in-memory identity the user can later persist via
/// `unlock <PIN>` + `save`.
fn bootstrap_state() -> anyhow::Result<AppState> {
    if Path::new(STORE_PATH).exists() {
        println!("  Encrypted profile detected at '{STORE_PATH}'.");
        return unlock_existing_profile();
    }
    let profile = KeyProfile::generate().expect("keygen");
    println!("  Identity   : {}", profile.npub);
    println!("  (no encrypted store yet — use 'unlock <PIN>' + 'save' to persist it)");
    Ok(AppState::new(profile))
}

/// Prompt for the passphrase (a few attempts) and open the existing store.
fn unlock_existing_profile() -> anyhow::Result<AppState> {
    for attempt in 1..=3 {
        let pin = read_line("  Passphrase to unlock profile: ")?;
        if pin.is_empty() {
            println!("  Passphrase cannot be empty.");
            continue;
        }
        match EncryptedStore::open(STORE_PATH, &pin) {
            Ok(store) => return Ok(restore_or_seed_identity(store)),
            Err(StorageError::InvalidPin) => println!("  Wrong passphrase ({attempt}/3)."),
            Err(e) => {
                println!("  Unlock failed: {e}");
                break;
            }
        }
    }
    println!("  Continuing with a non-persistent identity; 'unlock <PIN>' to retry.");
    Ok(AppState::new(KeyProfile::generate().expect("keygen")))
}

/// Given an unlocked store, restore the saved profile or seed a new one into it.
fn restore_or_seed_identity(store: EncryptedStore) -> AppState {
    let profile = match store.load_identity() {
        Ok(Some(id)) => match KeyProfile::from_nsec(&id.nsec) {
            Ok(p) => {
                println!("  Profile unlocked: {}", p.npub);
                p
            }
            Err(e) => {
                println!("  Stored nsec invalid ({e}); generating a fresh identity.");
                KeyProfile::generate().expect("keygen")
            }
        },
        Ok(None) => {
            let p = KeyProfile::generate().expect("keygen");
            println!(
                "  Store unlocked but empty; new identity {} (run 'save').",
                p.npub
            );
            p
        }
        Err(e) => {
            println!("  Could not read stored identity ({e}); using a fresh one.");
            KeyProfile::generate().expect("keygen")
        }
    };
    let mut st = AppState::new(profile);
    st.ctx.set_storage_status(StorageStatus::Unlocked);
    st.store = Some(store);
    st
}

// ── Main loop ────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialise structured logging; RUST_LOG overrides the default level
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn")),
        )
        .with_target(false)
        .compact()
        .init();

    print_banner();

    // Detect an existing encrypted profile and unlock it, or mint a fresh one.
    let state = Arc::new(RwLock::new(bootstrap_state()?));

    print_help();

    loop {
        let current_ws = {
            let guard = state.read().await;
            guard.ctx.current().clone()
        };

        let prompt = format!(
            "[{}]> ",
            match &current_ws {
                AppWorkspace::Base => "Base",
                AppWorkspace::OffGridTravel => "Saathi",
                AppWorkspace::CoupleSandbox(PairRole::Sakha) => "Sakha",
                AppWorkspace::CoupleSandbox(PairRole::Sakhi) => "Sakhi",
            }
        );

        let line = match read_line(&prompt) {
            Ok(l) => l,
            Err(e) => {
                warn!("read error: {e}");
                break;
            }
        };

        if line.is_empty() {
            continue;
        }

        let parts: Vec<&str> = line.splitn(2, ' ').collect();
        let cmd = parts[0].to_lowercase();
        let arg = parts.get(1).copied().unwrap_or("");

        match cmd.as_str() {
            "exit" | "q" | "quit" => {
                println!("  Goodbye.");
                break;
            }

            "help" | "?" => print_help(),

            "ws" => {
                let guard = state.read().await;
                print_workspace_header(guard.ctx.current());
            }

            "base" => {
                let mut guard = state.write().await;
                match guard.ctx.transition(AppWorkspace::Base) {
                    Ok(()) => print_workspace_header(guard.ctx.current()),
                    Err(e) => println!("  Transition error: {e}"),
                }
            }

            "travel" => {
                let mut guard = state.write().await;
                match guard.ctx.transition(AppWorkspace::OffGridTravel) {
                    Ok(()) => {
                        print_workspace_header(guard.ctx.current());
                        println!(
                            "  Saathi mesh would spin up here (run 'saathi' binary in production)"
                        );
                    }
                    Err(e) => println!("  Transition error: {e}"),
                }
            }

            "pair" => {
                let role = match arg.to_lowercase().as_str() {
                    "sakha" => PairRole::Sakha,
                    "sakhi" => PairRole::Sakhi,
                    other => {
                        println!("  Unknown role: '{other}'. Use 'pair sakha' or 'pair sakhi'.");
                        continue;
                    }
                };
                let mut guard = state.write().await;
                match guard.ctx.transition(AppWorkspace::CoupleSandbox(role)) {
                    Ok(()) => print_workspace_header(guard.ctx.current()),
                    Err(e) => println!("  Transition error: {e}"),
                }
            }

            "back" => {
                let mut guard = state.write().await;
                match guard.ctx.step_back() {
                    Some(prev) => {
                        println!("  Stepped back from {prev}");
                        print_workspace_header(guard.ctx.current());
                    }
                    None => println!("  Already at initial state."),
                }
            }

            "keygen" => match KeyProfile::generate() {
                Ok(profile) => {
                    println!("  New identity:");
                    println!("  npub : {}", profile.npub);
                    println!("  nsec : {}…", &profile.nsec[..20]);
                    state.write().await.profile = profile;
                }
                Err(e) => println!("  Keygen failed: {e}"),
            },

            "keys" => {
                let guard = state.read().await;
                println!("  npub : {}", guard.profile.npub);
                println!(
                    "  nsec : {}… (truncated for display)",
                    &guard.profile.nsec[..20]
                );
            }

            "partner" => match KeyProfile::generate() {
                Ok(partner) => {
                    println!("  Partner identity generated:");
                    println!("  npub : {}", partner.npub);
                    state.write().await.partner_profile = Some(partner);
                }
                Err(e) => println!("  Partner keygen failed: {e}"),
            },

            "dh" => {
                use comrade_core::crypto::compute_dh_shared_secret;
                let guard = state.read().await;
                match &guard.partner_profile {
                    None => println!("  Run 'partner' first to generate a demo partner."),
                    Some(partner) => {
                        match compute_dh_shared_secret(
                            guard.profile.keys.secret_key(),
                            &partner.public_key(),
                        ) {
                            Ok(secret) => {
                                println!(
                                    "  DH shared secret (first 16 bytes): {:?}",
                                    &secret[..16]
                                );
                                println!("  (Both sides produce the same 32-byte value)");
                            }
                            Err(e) => println!("  DH failed: {e}"),
                        }
                    }
                }
            }

            // ── Companion: private journal / vent / reflect / brainstorm ─────
            "journal" | "write" | "diary" => {
                companion_write(&state, CompanionMode::Journal, arg, None).await
            }

            "vent" => companion_write(&state, CompanionMode::Vent, arg, None).await,

            "brainstorm" | "ideas" => {
                if arg.is_empty() {
                    companion_prompt(&state, CompanionMode::Brainstorm).await;
                } else {
                    companion_write(&state, CompanionMode::Brainstorm, arg, None).await;
                }
            }

            "reflect" | "reflection" => {
                if arg.is_empty() {
                    companion_prompt(&state, CompanionMode::Reflect).await;
                } else {
                    companion_write(&state, CompanionMode::Reflect, arg, None).await;
                }
            }

            "mood" => {
                // split_whitespace tolerates double spaces ('mood  -1 tired').
                let mut it = arg.split_whitespace();
                match it.next().and_then(|v| v.parse::<i8>().ok()) {
                    Some(m) => {
                        let note = it.collect::<Vec<_>>().join(" ");
                        companion_write(&state, CompanionMode::Journal, &note, Some(m)).await;
                    }
                    None => {
                        println!("  Usage: mood <-2..2> [optional note]  (e.g. 'mood -1 tired')")
                    }
                }
            }

            "entries" | "journalfeed" => companion_entries(&state).await,

            "insights" => companion_insights(&state).await,

            "tree" | "feed" | "chitthi" => demo_chitthi_feed(&state).await,

            "cache" | "offline" => show_cached_feed(&state).await,

            "pay" => {
                if arg.is_empty() {
                    println!("  Usage: pay <message containing /pay command>");
                    println!("  Example: pay /pay 250 to friend@upi");
                } else {
                    demo_pay_extraction(arg);
                }
            }

            "ledger" => demo_ledger(&state).await,

            "relays" => demo_relay_routing(),

            "media" => demo_media().await,

            "call" => demo_call_signaling(),

            "voicemsg" => demo_voice_message().await,

            "unlock" => {
                // Trim: the startup passphrase prompt trims too — a PIN that
                // works at `unlock` must keep working at the next boot.
                let pin = arg.trim();
                if pin.is_empty() {
                    println!("  Usage: unlock <PIN>");
                } else if state.read().await.store.is_some() {
                    println!("  Store is already unlocked (sled holds an exclusive lock).");
                } else {
                    match EncryptedStore::open(STORE_PATH, pin) {
                        Ok(store) => {
                            println!("  Encrypted store unlocked at '{STORE_PATH}'.");
                            let mut guard = state.write().await;
                            // Restore the persisted identity — otherwise the
                            // session keeps its throwaway keypair and a later
                            // 'save' would silently overwrite the real nsec.
                            match store.load_identity() {
                                Ok(Some(id)) => match KeyProfile::from_nsec(&id.nsec) {
                                    Ok(profile) => {
                                        println!("  Profile restored: {}", profile.npub);
                                        guard.profile = profile;
                                    }
                                    Err(e) => println!(
                                        "  Stored nsec invalid ({e}); keeping the current identity."
                                    ),
                                },
                                Ok(None) => println!(
                                    "  (no saved identity yet — 'save' to persist this one)"
                                ),
                                Err(e) => println!("  Could not read stored identity: {e}"),
                            }
                            guard.store = Some(store);
                            guard.ctx.set_storage_status(StorageStatus::Unlocked);
                        }
                        Err(e) => println!("  Unlock failed: {e}"),
                    }
                }
            }

            "save" => {
                let guard = state.read().await;
                match &guard.store {
                    None => println!("  Run 'unlock <PIN>' first."),
                    Some(store) => {
                        let identity = StoredIdentity::new(
                            guard.profile.npub.clone(),
                            guard.profile.nsec.clone(),
                            Some("primary".into()),
                        );
                        match store.save_identity(&identity).and_then(|()| store.flush()) {
                            Ok(()) => println!("  Identity saved (encrypted at rest)."),
                            Err(e) => println!("  Save failed: {e}"),
                        }
                    }
                }
            }

            "load" => {
                let mut guard = state.write().await;
                let loaded = match &guard.store {
                    None => {
                        println!("  Run 'unlock <PIN>' first.");
                        None
                    }
                    Some(store) => match store.load_identity() {
                        Ok(Some(id)) => Some(id),
                        Ok(None) => {
                            println!("  No saved identity found. Use 'save' first.");
                            None
                        }
                        Err(e) => {
                            println!("  Load failed: {e}");
                            None
                        }
                    },
                };
                if let Some(id) = loaded {
                    match KeyProfile::from_nsec(&id.nsec) {
                        Ok(profile) => {
                            println!("  Identity restored from encrypted store:");
                            println!("  npub : {}", profile.npub);
                            guard.profile = profile;
                        }
                        Err(e) => println!("  Stored nsec invalid: {e}"),
                    }
                }
            }

            unknown => {
                println!("  Unknown command: '{unknown}'. Type 'help' for available commands.")
            }
        }
    }

    Ok(())
}
