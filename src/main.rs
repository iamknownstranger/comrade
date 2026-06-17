/*!
 * Milestone 6 — Comrade CLI Harness
 *
 * Text-based workspace switcher and interactive validation shell that wires
 * every engine to a single entry point.  Demonstrates smooth transitions
 * between Base mode, Off-Grid Travel (Saathi mesh), and the Sakha/Sakhi
 * Couple Sandbox via the Progressive-Disclosure state machine.
 */

use std::io::{self, Write};
use std::sync::Arc;

use comrade_core::{
    crypto::KeyProfile,
    sabha::build_chitthi_thread,
    vault::{build_pay_regex, extract_upi_intents},
};
use comrade_state::{AppWorkspace, PairRole, RuntimeContext};
use comrade_storage::{EncryptedStore, StoredIdentity};
use tokio::sync::RwLock;
use tracing::warn;
use tracing_subscriber::EnvFilter;

/// Default on-disk location for the encrypted store.
const STORE_PATH: &str = "comrade-data";

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
    println!("║              C O M R A D E  —  Unified Client            ║");
    println!("║   Sabha · Vault · Saathi · Sakha/Sakhi                   ║");
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
    println!("  tree/feed  — demo: build a sample NIP-10 ChitthiThread (Sabha timeline)");
    println!("  pay <msg>  — demo: extract UPI /pay intents from text");
    println!("  ledger     — demo: append an entry and show CRDT ledger");
    println!("  relays     — demo: NIP-65 outbox-model relay routing");
    println!("  media      — demo: NIP-94/96 encrypted media pipeline");
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
    io::stdin().read_line(&mut buf)?;
    Ok(buf.trim().to_string())
}

// ── Demo scenarios ───────────────────────────────────────────────────────────

fn demo_chitthi_feed() {
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

async fn demo_media() {
    use comrade_core::crypto::KeyProfile;
    use comrade_core::media::{decrypt_media, parse_file_metadata, InMemoryUploader, MediaEngine};

    println!("  Demonstrating encrypted media (NIP-94/96) …");

    let keys = match KeyProfile::generate() {
        Ok(p) => p.keys,
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

    // Generate or restore identity
    let profile = KeyProfile::generate().expect("keygen");
    println!("  Identity   : {}", profile.npub);
    println!("  (nsec kept in memory; use 'unlock <PIN>' + 'save' to persist it encrypted)");

    let state = Arc::new(RwLock::new(AppState::new(profile)));

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

            "tree" | "feed" | "chitthi" => demo_chitthi_feed(),

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

            "unlock" => {
                if arg.is_empty() {
                    println!("  Usage: unlock <PIN>");
                } else {
                    match EncryptedStore::open(STORE_PATH, arg) {
                        Ok(store) => {
                            println!("  Encrypted store unlocked at '{STORE_PATH}'.");
                            state.write().await.store = Some(store);
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
