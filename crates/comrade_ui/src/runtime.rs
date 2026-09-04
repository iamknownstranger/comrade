/*!
 * comrade_ui::runtime — the async IPC bridge orchestrator.
 *
 * [`ComradeRuntime`] is the live "runtime context" the Command & Event Bridge
 * manages behind an `Arc<RwLock<…>>`. It is the single, framework-agnostic
 * aggregate that both the **Tauri desktop** shell (`#[tauri::command]` wrappers)
 * and the **Android** layer (`comrade_jni`'s uniffi-generated Kotlin bindings)
 * drive — keeping all real logic inside the workspace where it is unit-tested
 * and Send/Sync-checked. This crate itself stays uniffi-agnostic beyond
 * deriving `Record`/`Enum`/`Error` on its DTOs — `comrade_jni` is the only
 * place that wraps this type behind actual FFI scaffolding.
 *
 * It composes the sync view-model ([`UiService`] — workspace state, identity,
 * encrypted store) with the live Nostr engines (Sabha public feed, Vault E2E
 * DMs, Sakha couple ledger) and a [`tokio::sync::broadcast`] **event bus**.
 *
 * Naming: the IPC spec refers to this as the `RuntimeContext` app-state handle.
 * It is named `ComradeRuntime` here to avoid colliding with the pure, I/O-free
 * [`comrade_state::RuntimeContext`] (the workspace state machine) that it wraps.
 *
 * Design guarantees the bindings rely on:
 *  • Every method returns a typed [`UiError`] — no `.unwrap()`, no panics — so a
 *    failure becomes a `Promise.reject` (Tauri) or a thrown exception (uniffi).
 *  • Heavy work (relay connect, feed subscription, DM decryption) runs in
 *    spawned Tokio tasks via [`ComradeRuntime::spawn_event_loops`], never on the
 *    caller's UI thread.
 */

use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use comrade_core::anon;
use comrade_core::attention::{
    self, FocusOutcome, UsageSignal, FOCUS_MAX_MINUTES, FOCUS_MIN_MINUTES,
};
use comrade_core::call::{
    call_signal_is_stale, call_signal_retry_delay_ms, derive_sas, ice_servers_for, new_call_id,
    parse_call_envelope, validate_turn_url, CallEnvelope, CallMediaKind, CallSignal, HangupReason,
    IceServer, IceStrategy,
};
use comrade_core::command::{
    self, parse_offer_envelope, render_offer_line, AppAction, ChatCommand, CommandSpec, Mention,
    MusicService, OfferEnvelope,
};
use comrade_core::crypto::derive_media_key;
use comrade_core::dak::outbox::local_message_id;
use comrade_core::dak::{ble, open_dm, seal_dm, Envelope, MeshDm, Reassembler};
use comrade_core::dak::{AttemptOutcome, Outbox, OutboxSnapshot, QueueOutcome, QueuedMessage};
use comrade_core::dm::{
    parse_delete_request, parse_profile_share, parse_reaction, parse_receipt, DeleteRequest,
    ProfileShare, ReactionEnvelope, Receipt, ReceiptKind, MAX_REACTION_BYTES,
};
use comrade_core::handoff::{parse_handoff_envelope, HandoffEnvelope, HandoffSignal};
use comrade_core::karya::{
    new_task_id, parse_karya_envelope, render_task_line, KaryaEnvelope, Party, Task, TaskSignal,
    TaskState,
};
use comrade_core::media::{
    build_file_metadata_event, encrypt_media, fetch_and_decrypt_media, FileMetadata,
    MAX_MEDIA_BYTES,
};
use comrade_core::metrics as core_metrics;
use comrade_core::metrics::Metric as CoreMetric;
use comrade_core::nudge::{is_fresh_at, nudge_expires_at, parse_nudge, Nudge, NudgeWatch};
use comrade_core::presence::{
    is_online_at, parse_presence_beacon, presence_expires_at, PresenceBeacon,
    PRESENCE_HEARTBEAT_SECS, PRESENCE_SWEEP_SECS,
};
use comrade_core::ride::{
    parse_ride_envelope, ride_expires_at, RideEnvelope, RideManeuver, RidePhrase, RideSignal,
};
use comrade_core::saathi::SaathiEngine;
use comrade_core::sabha::{
    display_name_of, ChitthiCallback, FeedFilterSpec, FeedScope, SabhaEngine, DEFAULT_RELAYS,
};
use comrade_core::sakha::{LedgerEntry, SakhaEngine, SakhaSyncCallback};
use comrade_core::seen::{content_key, SeenSet, CONTENT_KEY_PREFIX};
use comrade_core::share::transport::{
    self as share_transport, IcePathKind, RefusalReason, RelayPolicy, TransferVerdict,
};
use comrade_core::share::{
    read_verdict as share_read_verdict, ReadSample, ReadVerdict, ShareSignal,
};
use comrade_core::topic::{parse_topic_envelope, TopicSignal};
use comrade_core::travel::{self, GuideCache, Place, TravelGuide, TravelQuery};

/// Read a stored [`SharePrefs`] back into a policy.
///
/// **An unrecognised string is [`RelayPolicy::DirectOnly`]**, not a panic and
/// not `Always`. The value can only be wrong if an older or newer build wrote
/// it, and the safe reading of "I do not know what this device agreed to" is
/// the one that carries nobody's bytes but our own.
fn relay_policy_from_prefs(prefs: &comrade_storage::SharePrefs) -> RelayPolicy {
    match prefs.relay_policy.as_str() {
        "under_bytes" => RelayPolicy::UnderBytes {
            limit: prefs.relay_limit_bytes,
        },
        "ask_each_time" => RelayPolicy::AskEachTime,
        "always" => RelayPolicy::Always,
        _ => RelayPolicy::DirectOnly,
    }
}

/// The inverse. Round-trips through [`relay_policy_from_prefs`] by test.
fn relay_policy_to_prefs(policy: RelayPolicy) -> comrade_storage::SharePrefs {
    let (name, limit) = match policy {
        RelayPolicy::DirectOnly => ("direct_only", 0),
        RelayPolicy::UnderBytes { limit } => ("under_bytes", limit),
        RelayPolicy::AskEachTime => ("ask_each_time", 0),
        RelayPolicy::Always => ("always", 0),
    };
    comrade_storage::SharePrefs {
        relay_policy: name.to_string(),
        relay_limit_bytes: limit,
    }
}
use comrade_core::catalogue::OpenLicence;
use comrade_core::catalogue::{choose_audio_plan, AudioPlan, CatalogueMatch};
use comrade_core::download::{permit_download, DownloadRefusal};
// The network half of the streaming source exists only where sockets do; the
// config type crosses the FFI in every build.
use comrade_core::subsonic::SubsonicConfig;
use comrade_core::tara::{
    tara_chat_answer, tara_chat_line, CompanionEngine, JournalSignal, ReflectiveCompanion,
};
use comrade_core::together::{
    command_apply, describe_state_change, direct_path_live, direct_signal_admissible,
    heartbeat_interval_ms, parse_together_envelope, projected_peer_pos_ms, session_is_live_at,
    signal_is_fresh, sync_verdict, ClockEcho, ClockFilter, CommandApply, CommandStamp,
    PlayheadControl, Recording, StateChange, SyncSample, SyncVerdict, TogetherContent,
    TogetherEnvelope, TogetherSignal, CLOCK_BURST_PROBES,
};
use comrade_core::vault::{
    build_pay_regex, extract_upi_intents, PayRegex, VaultCallback, VaultEngine, VaultMessage,
};
#[cfg(feature = "catalogue-http")]
use comrade_core::{public_sources, subsonic};
use nostr_sdk::prelude::{EventId, Metadata, PublicKey, ToBech32};
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc};
use tracing::warn;

use crate::{IdentityDto, UiError, UiService, UpiIntentDto, WorkspaceDto};

/// Capacity of the *critical* event bus — DMs, message requests, call
/// signals, message status, mesh status, ledger updates. Slow consumers lag
/// rather than block producers, but this channel is never intentionally
/// flooded (a peer sends a human's worth of DMs/calls, not a relay's worth of
/// public notes), so its capacity only needs to absorb a slow consumer
/// briefly falling behind, not an adversarial volume.
const EVENT_BUS_CAPACITY: usize = 256;

/// Capacity of the separate, deliberately small *feed* event bus —
/// `IncomingChitthi` only. AUDIT.md COMMS-04: a public-feed flood must be
/// able to drop old, unconsumed Chitthis under load without ever competing
/// for the same ring-buffer slots a call signal or DM needs — hence a wholly
/// separate [`broadcast::Sender`], not just "the same channel, hope the
/// consumer drains it fast enough". See [`ComradeRuntime::subscribe_feed_events`].
const FEED_EVENT_BUS_CAPACITY: usize = 64;

/// How far back the Chitthi feed subscription looks, in both the
/// authors-scoped and bootstrap cases (see [`ComradeRuntime::feed_filter_spec`]).
const FEED_SINCE_SECS: u64 = 3600;
/// Event cap for the no-contacts-yet bootstrap feed — an explicit bound so a
/// fresh identity's feed is never the unbounded relay-wide firehose either.
const FEED_BOOTSTRAP_LIMIT: usize = 200;

/// HKDF label binding the ECDH shared secret to media encryption.
const MEDIA_LABEL: &str = "comrade-media-v1";

/// What an encrypted attachment is declared as when it is uploaded.
///
/// Deliberately not the file's real type: the body is AES-GCM ciphertext, and
/// naming it `image/jpeg` would both misdescribe it and hand the media host the
/// one piece of metadata the encryption was meant to withhold — what *kind* of
/// thing was just sent. The recipient learns the true type from the encrypted
/// envelope, never from the host.
const OPAQUE_UPLOAD_MIME: &str = "application/octet-stream";
/// Encrypted-store tree mapping a NIP-94 event id → local [`MediaRef`].
const MEDIA_REFS_TREE: &str = "comrade_media_refs";
/// Encrypted-store tree caching peers' published Kind-0 profiles
/// (npub → [`PeerProfileRecord`]). This is what lets the chat UI show
/// "@charlie" instead of a raw public key.
const PEER_PROFILES_TREE: &str = "peer_profiles";
/// Re-fetch a cached peer profile with a known name after this long (seconds).
const PROFILE_TTL_SECS: u64 = 24 * 60 * 60;
/// Re-fetch a cached record with **no** name after this long (seconds).
/// Short: an offline fetch is indistinguishable from "peer has no profile",
/// and it must not freeze a peer as key-only for a whole day.
const PROFILE_NEGATIVE_TTL_SECS: u64 = 5 * 60;
/// Upper bound on network fetches per [`ComradeRuntime::refresh_peer_profiles`] call.
const PROFILE_REFRESH_CAP: usize = 16;
/// Encrypted-store tree holding vetted avatar *bytes*, keyed by their SHA-256.
///
/// A separate tree, and content-addressed, for two reasons. `EncryptedStore::put`
/// serialises through `serde_json`, so a `Vec<u8>` field on
/// [`PeerProfileRecord`] would be stored as a JSON array of decimal numbers —
/// roughly four bytes on disk per byte of image. And keying by hash means two
/// peers who publish the same picture share one copy. `put_bytes` is the raw
/// path; `DEVICE_SEED_KEY` already uses it for the same reason.
const PEER_AVATAR_BLOBS_TREE: &str = "peer_avatar_blobs";
/// Re-fetch a cached avatar after this long. Much longer than the profile TTL:
/// people change their handle far more often than their picture, and each refetch
/// costs a whole image.
const AVATAR_TTL_SECS: u64 = 7 * 24 * 60 * 60;
/// How long a failed or refused avatar fetch is left alone before another try, so
/// a dead URL is not re-attempted on every sweep.
const AVATAR_NEGATIVE_TTL_SECS: u64 = 15 * 60;
/// Upper bound on avatar downloads per refresh sweep. Lower than the profile
/// cap: each of these is an image, not a line of JSON.
const AVATAR_FETCH_CAP: usize = 8;
/// The longest bio this build will store or publish. Matches the caption bound,
/// so the two peer-chosen free-text fields agree.
const MAX_ABOUT_LEN: usize = 512;
/// Settings key for the user's own bio.
///
/// Not in the `StoredIdentity` label, which is where the @handle lives: that slot
/// holds one string and already overloads `"primary"` as a legacy no-username
/// marker. A second meaning on it would be a third thing one field means.
const PROFILE_ABOUT_KEY: &str = "profile_about";
/// Settings key for whether peer-published pictures may be fetched at all.
const REMOTE_AVATARS_KEY: &str = "remote_avatars";
/// Publish attempts before giving up until the next launch (see
/// [`publish_profile_with_retry`]).
const PUBLISH_ATTEMPTS: u32 = 5;
/// Encrypted-store tree for app settings that are not per-peer (e.g. the TURN
/// relay a user has configured for calls).
const SETTINGS_TREE: &str = "app_settings";
/// Settings key holding the optional [`TurnConfig`] for WebRTC calls.
const TURN_CONFIG_KEY: &str = "turn_server";
/// Settings key holding the user's own Google Places API key, used by the
/// Travel guide's ratings half.
///
/// In the **encrypted** store rather than a device preference, and for a
/// sharper reason than the usual one: this key is billable. A plaintext
/// preference file is readable by anything with a debug bridge and a rooted
/// phone, and the person who pays for the quota is the user.
const TRAVEL_PLACES_KEY: &str = "travel_places_api_key";
/// Settings key holding the high-watermark (unix seconds) of the newest
/// inbox message this device has processed — see [`advance_watermark`] and
/// [`ComradeRuntime::spawn_event_loops`]'s `since_floor` computation.
const VAULT_WATERMARK_KEY: &str = "vault_last_seen_at";
/// Encrypted-store tree holding the sender-outbox snapshot — DMs that could not
/// be published, kept so an offline send survives an app kill instead of
/// evaporating (adopted from bitchat's store-and-forward, see
/// `docs/BITCHAT_ADOPTION.md`).
const OUTBOX_TREE: &str = "comrade_outbox";
const OUTBOX_KEY: &str = "queued";
/// Settings key holding the 32-byte device seed every anonymous persona is
/// derived from — see [`load_or_create_device_seed`].
const DEVICE_SEED_KEY: &str = "anonymity_device_seed";
/// Status string on a message that is queued in the outbox, not yet on a relay.
const STATUS_QUEUED: &str = "queued";
/// Status string on a message the outbox gave up on (attempt cap or TTL).
const STATUS_FAILED: &str = "failed";
/// How often the background flush loop retries queued mail. Comrade has no
/// mesh-style "peer reconnected" event to hang a flush on, so a modest fixed
/// cadence stands in for bitchat's reconnect trigger; a flush with an empty
/// outbox costs one lock and returns.
const OUTBOX_FLUSH_INTERVAL_SECS: u64 = 60;

/// Capacity of the call-signal dedup set: comfortably above one call's signal
/// count (offer/answer/several ICE candidates/hangup is a handful of events), so
/// a single call can never evict its own earlier signals mid-negotiation.
/// At-least-once relay delivery plus the 2-day inbox backfill mean the same
/// wrapper can arrive repeatedly, and a duplicate `Answer` or terminal `Hangup`
/// must not be re-applied downstream.
const CALL_SIGNAL_DEDUP_CAPACITY: usize = 512;

/// Capacity of the together *invite* dedup set, keyed by session id.
///
/// [`TogetherSignal::Start`] is the only signal keyed this way. `Join` is
/// idempotent and `State` is ordered by its own Lamport counter (an exact,
/// unbounded dedup that cannot be evicted), so a two-hour session adds nothing
/// here — which is the point: sharing [`CALL_SIGNAL_DEDUP_CAPACITY`]'s set would
/// let one film evict a live call's signals. `Share` has neither of those
/// defences and gets its own set, [`TOGETHER_SHARE_DEDUP_CAPACITY`].
const TOGETHER_START_DEDUP_CAPACITY: usize = 64;

/// Capacity of the together *share* dedup set: wrapper event ids of transfer
/// signals already handed to a frontend (AUDIT.md Q18).
///
/// Sized for the negotiation, because that is the burst — ask/offer/accept is
/// three events and the ICE trickle behind them is tens more per attempt — so a
/// handful of transfers fit and an `Ask` redelivered by the backfill is still
/// recognised while the transfer it would have restarted is running.
///
/// Its own set rather than a second key space in [`TOGETHER_START_DEDUP_CAPACITY`]'s:
/// that one is keyed by session id and this one by event id, and one bounded set
/// holding both would make an eviction mean two different things.
const TOGETHER_SHARE_DEDUP_CAPACITY: usize = 256;

/// How long a message stays eligible for **cross-transport** dedup.
///
/// A DM can reach the same person twice by two different routes: sealed over
/// the local mesh now, and over a relay when the internet comes back. The two
/// copies carry different ids (the mesh copy is keyed by the sender's local id,
/// the relay copy by the event id a relay assigned), so id dedup cannot catch
/// the pair — the content can.
///
/// Deliberately narrow, and keyed by transport as well as content: two
/// identical messages that arrive over the *same* route are a person typing
/// "ok" twice and are kept. Only a copy arriving over the *other* route inside
/// this window is treated as the same message. That is the whole trade — a
/// wider window would start eating genuine repeats.
const CROSS_TRANSPORT_DEDUP_SECS: u64 = 120;
/// Recent (peer, content, transport) triples kept for the check above.
const CROSS_TRANSPORT_DEDUP_CAPACITY: usize = 512;
/// Transport labels for that key.
const TRANSPORT_RELAY: &str = "relay";
const TRANSPORT_MESH: &str = "mesh";

// The call-signal staleness rule now lives in `comrade_core::call`
// (`call_signal_is_stale`, with its max age and clock-skew tolerance) so it is
// a tested decision rather than an inline comparison. It used to be `age > 90`
// against the *sender's* clock with no tolerance, which silently killed every
// call between two devices whose clocks disagreed by more than that.
/// Encrypted-store tree holding the Sakha/Sakhi pairing record (there is only
/// ever one partner per device, but a tree keeps the storage shape uniform
/// with the rest of the repository layer).
const SAKHA_TREE: &str = "sakha_pairing";
const SAKHA_PAIRING_KEY: &str = "partner";

/// Conversation gate states (persisted in `ConversationMeta.state`).
const STATE_PENDING: &str = "pending";
const STATE_ACCEPTED: &str = "accepted";
const STATE_BLOCKED: &str = "blocked";

// ── Event DTOs (serialised across the IPC / FFI boundary) ────────────────────

/// A public Chitthi (Kind-1) as the frontend sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct ChitthiDto {
    pub id: String,
    pub author: String,
    pub content: String,
    pub created_at: u64,
    pub reply_to: Option<String>,
}

impl ChitthiDto {
    /// Build from a live Nostr Kind-1 event captured in the Tokio feed loop.
    pub fn from_event(event: &nostr_sdk::prelude::Event) -> Self {
        let author = event
            .pubkey
            .to_bech32()
            .unwrap_or_else(|_| event.pubkey.to_hex());
        Self {
            id: event.id.to_hex(),
            author,
            content: event.content.clone(),
            created_at: event.created_at.as_secs(),
            reply_to: None,
        }
    }

    /// Build from a row of the offline encrypted Chitthi cache.
    pub fn from_cached(c: &comrade_storage::Chitthi) -> Self {
        Self {
            id: c.id.clone(),
            author: c.author_npub.clone(),
            content: c.content.clone(),
            created_at: c.created_at,
            reply_to: c.reply_to.clone(),
        }
    }
}

/// One person's emoji reaction to one message, as a frontend sees it.
///
/// Flat rather than "a message plus its reactions" because reactions arrive
/// independently of the message they are about — a reaction can outrun the
/// backfill of its target — so the UI joins them by [`Self::target_id`] the same
/// way it already resolves a `reply_to` into a quoted preview.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct ReactionDto {
    /// Event id (hex) of the message reacted to. A text message or an
    /// attachment — the reaction does not care which.
    pub target_id: String,
    /// The conversation it belongs to (the other party's npub).
    pub peer: String,
    /// Who reacted, as an npub.
    pub reactor: String,
    pub emoji: String,
    pub created_at: u64,
    /// Whether *this device* sent it, so the UI can highlight your own.
    pub outgoing: bool,
}

impl From<comrade_storage::MessageReaction> for ReactionDto {
    fn from(r: comrade_storage::MessageReaction) -> Self {
        Self {
            target_id: r.target_id,
            peer: r.peer_npub,
            reactor: r.reactor_npub,
            emoji: r.emoji,
            created_at: r.created_at,
            outgoing: r.outgoing,
        }
    }
}

/// An incoming encrypted direct message (Kind-4) as the frontend sees it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, uniffi::Record)]
pub struct DirectMessageDto {
    pub id: String,
    pub sender: String,
    pub content: String,
    pub created_at: u64,
    pub upi_intents: Vec<UpiIntentDto>,
    /// Event id (hex) this message replies to, if any (for a quoted preview).
    pub reply_to: Option<String>,
    /// Set when this arrival is a journal note its sender chose to share, so a
    /// frontend appending a live event draws the same card
    /// [`MessageDto::shared_note`] gives it after a reload. Parsed here, in the
    /// one place that knows the grammar — a frontend re-reading the marker off
    /// [`Self::content`] is the second implementation this field exists to
    /// prevent.
    ///
    /// Unlike [`MessageDto::content`], [`Self::content`] keeps the marker: this
    /// is the raw arrival, and a consumer drawing a card reads it from here.
    pub shared_note: Option<SharedNoteDto>,
}

impl From<VaultMessage> for DirectMessageDto {
    fn from(m: VaultMessage) -> Self {
        Self {
            id: m.event_id,
            sender: to_npub(&m.sender_pubkey),
            shared_note: shared_note_of(&m.content),
            content: m.content,
            created_at: m.created_at,
            reply_to: m.reply_to,
            upi_intents: m
                .upi_intents
                .into_iter()
                .map(|i| UpiIntentDto {
                    amount_inr: i.amount_inr,
                    vpa: i.vpa,
                    uri: i.uri,
                })
                .collect(),
        }
    }
}

/// A WebRTC ICE server (STUN/TURN) for the frontend's `RTCConfiguration`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct IceServerDto {
    pub urls: Vec<String>,
    pub username: Option<String>,
    pub credential: Option<String>,
}

/// The support/diagnostic "is a relay configured" status — see
/// [`ComradeRuntime::turn_server_status`]. Deliberately carries the URL only,
/// never `username`/`credential`: this DTO is meant to be safe to show
/// directly in a settings screen (and safe to log), unlike the write-only
/// fields [`ComradeRuntime::set_turn_server`] takes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct TurnServerStatusDto {
    pub configured: bool,
    pub url: Option<String>,
}

impl From<comrade_core::call::IceServer> for IceServerDto {
    fn from(s: comrade_core::call::IceServer) -> Self {
        Self {
            urls: s.urls,
            username: s.username,
            credential: s.credential,
        }
    }
}

/// Everything a frontend needs to begin negotiating a call: the call id, the
/// peer, the media kind, and the ICE servers to hand to `RTCPeerConnection`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct CallSessionDto {
    pub call_id: String,
    pub peer: String,
    pub media: String,
    pub ice_servers: Vec<IceServerDto>,
}

/// One incoming call-signaling payload (offer/answer/ICE/hangup/…) routed to
/// the frontend. `signal` is the actual [`CallSignal`] value (not a JSON blob)
/// so the WebRTC layer — and uniffi, which has no "arbitrary JSON" type — gets
/// a closed enum to `switch` on directly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, uniffi::Record)]
pub struct CallSignalDto {
    pub call_id: String,
    pub peer: String,
    pub media: String,
    pub signal: CallSignal,
    /// The signal's true send time (unix seconds) — the DM's own `created_at`
    /// (already de-randomized from the gift-wrap's timestamp skew; see
    /// `comrade_core::vault::VaultMessage`), not when it was received. Lets a
    /// frontend apply its own freshness judgement in addition to the runtime's
    /// own staleness drop (see `CALL_SIGNAL_MAX_AGE_SECS` in `dispatch_incoming_dm`).
    pub created_at: u64,
}

/// An invitation to watch or listen to something together.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct TogetherInviteDto {
    pub session_id: String,
    pub peer: String,
    pub content: TogetherContent,
    pub pos_ms: u64,
    pub playing: bool,
    /// The invite's true send time (unix seconds), like [`CallSignalDto::created_at`].
    pub created_at: u64,
}

/// A live shared session, as the frontend sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct TogetherSessionDto {
    pub session_id: String,
    pub peer: String,
    pub content: TogetherContent,
    /// Whether *we* started it. The starter leads, and only the follower
    /// corrects drift — see `comrade_core::together::sync_verdict`.
    pub we_lead: bool,
    pub joined: bool,
    pub pos_ms: u64,
    pub playing: bool,
}

/// The peer played, paused or seeked, and this command won the ordering.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct TogetherCommandDto {
    pub session_id: String,
    /// Where to be — already carried forward through the message's flight time,
    /// so a frontend applies this number as-is and does not compensate again.
    pub pos_ms: u64,
    pub playing: bool,
    pub change: StateChange,
    /// Wait this long before applying, so both players change state on the same
    /// instant. Zero means the moment has passed and `pos_ms` already accounts
    /// for it — the only case a relay-speed transport can produce.
    pub apply_in_ms: u64,
}

/// The player should be moved to stay with the other side.
///
/// Emitted **only** when the verdict is not `Hold`, which is what keeps a
/// periodic heartbeat from becoming a periodic producer on the critical event
/// bus (see [`EVENT_BUS_CAPACITY`]). `drift_ms` and `quality_ms` are carried so
/// a UI can report the drift it actually has rather than one we predicted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, uniffi::Record)]
pub struct TogetherCorrectionDto {
    pub session_id: String,
    pub verdict: SyncVerdict,
    pub drift_ms: i64,
    /// How wrong that drift figure could be — half the measured round trip.
    pub quality_ms: u64,
}

/// One step of handing the file over, on its way to the frontend.
///
/// The runtime deliberately keeps **no** state for this. Everything a transfer
/// needs — the peer connection, the data channel, the bytes — lives in the
/// frontend, because that is where WebRTC lives; the runtime's whole job is to
/// carry signals across a gated, end-to-end channel and to answer the policy
/// question. Mirroring the negotiation here as well would mean two state
/// machines that have to agree, which is the shape of the two call bugs this
/// repo already fixed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct TogetherShareDto {
    pub session_id: String,
    pub peer: String,
    pub signal: ShareSignal,
}

/// One ride signal from the other seat of the motorcycle, ready to render —
/// see [`comrade_core::ride`].
///
/// Flattened to strings and options rather than mirroring the core enums,
/// for the reason [`TogetherShareDto`]'s doc gives in reverse: the frontends
/// key decision tables on the wire names (`RidePhrase::as_str`), and a
/// bridged enum would put a Kotlin `when`, a Dart `switch` and a regenerated
/// bridge behind every phrase added to the catalog. `kind` says which of the
/// two field groups is populated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct RideSignalDto {
    pub peer: String,
    /// Display name at the time of the event (alias → published handle), so a
    /// glance card can be drawn without a store round-trip.
    pub name: Option<String>,
    /// `"quick"` or `"route"` — [`comrade_core::ride::RideSignal`]'s tag.
    pub kind: String,
    /// The catalog phrase's wire name, for `kind == "quick"`.
    pub phrase: Option<String>,
    /// The maneuver's wire name, for `kind == "route"`.
    pub maneuver: Option<String>,
    pub distance_m: Option<u32>,
    pub note: Option<String>,
    /// `"urgent"`, `"notice"` or `"info"` — decided in core
    /// ([`comrade_core::ride::RideUrgency`]), so two phones cannot disagree
    /// about whether "pull over" is worth a buzz.
    pub urgency: String,
    /// The signal's true send time (unix seconds), like
    /// [`CallSignalDto::created_at`]: what a frontend ages the card out by.
    pub created_at: u64,
}

// ── In-chat commands (see `comrade_core::command`) ───────────────────────────

/// One `@handle` from a composer, resolved against the saved contacts.
///
/// Three outcomes, and the middle one is the point: `npub` set is a match,
/// `candidates` non-empty is **more than one contact answering to that handle**,
/// and both empty means nobody does. A handle is a self-declared alias, not an
/// identifier ([`ContactDto`]), so picking one of two silently is how a private
/// message reaches the wrong person — the ambiguity is returned for the UI to
/// ask about rather than resolved by guessing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct MentionMatchDto {
    /// Lowercased handle, without the `@`.
    pub handle: String,
    /// Byte span in the text that was parsed, so a composer can draw a chip.
    pub start: u32,
    pub end: u32,
    /// The single contact this names, when exactly one does.
    pub npub: Option<String>,
    /// Every contact answering to the handle when more than one does. Empty
    /// otherwise.
    pub candidates: Vec<ContactDto>,
}

/// One task, as a list wants it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct TaskDto {
    pub id: String,
    pub text: String,
    pub assigner: String,
    /// `None` for a note to self, which never reached a relay.
    pub assignee: Option<String>,
    pub created_at: u64,
    pub updated_at: u64,
    pub state: TaskState,
    /// Whether the local user named this task. With [`Self::assignee`] this is
    /// everything a UI needs to know which buttons to offer — computed here so
    /// three frontends do not each re-derive it from npub comparisons.
    pub assigned_by_me: bool,
    /// Whether the local user is the one being asked, and so may finish or
    /// decline it. True for a note to self.
    pub mine_to_do: bool,
}

/// The result of offering an in-app action: who was told, and why the others
/// were not.
///
/// **A bare count was not enough, and the reason is a bug this replaced.**
/// `offer_action` can reach zero three ways — nobody named was a comrade, the
/// shared cooldown was still running, or every send failed — and a frontend
/// holding only `0` said *"they were told recently"* for all three. Telling
/// somebody their message was throttled when the truth is "that person is not
/// your comrade" is worse than saying nothing: it names a cause that is not
/// real, and the actual fix (mark them a comrade) is never suggested.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct OfferOutcomeDto {
    /// Comrades who were actually told.
    pub sent: Vec<String>,
    /// Named peers who are not comrades — the offer never applied to them.
    /// Marking them a comrade is the fix, and the UI should say so.
    pub not_comrades: Vec<String>,
    /// Comrades left alone because the shared nudge cooldown is still running.
    pub on_cooldown: Vec<String>,
    /// Comrades the send failed for outright — no relay took it, and unlike a
    /// chat message a control envelope is not queued (see
    /// `RuntimeHandles::send_control_envelope`).
    pub failed: Vec<String>,
}

/// What can actually be done with a `/play` query, decided once.
///
/// The decision rather than the prose: each frontend renders its own words from
/// this, which is what lets desktop say "no player here yet"
/// (`docs/TOGETHER.md` §9) while Android opens a session, without either
/// sentence living in core.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, uniffi::Enum)]
#[serde(rename_all = "snake_case")]
pub enum PlayPlan {
    /// A session can open right now — [`PlayTargetDto::content`] is set.
    OpenNow,
    /// We know what recording is meant; look for it in this device's own
    /// library ([`comrade_core::together::match_score`]), and fall back to
    /// `comrade_core::share` if it is not there.
    FindLocally,
    /// The link names something we may not play ourselves — Spotify and Apple
    /// Music serve DRM audio no third-party client may decode
    /// ([`comrade_core::together::MusicLink::playable_in_place`]). All we can
    /// honestly do is say where to open it.
    NameOnly,
    /// Nothing usable in the query.
    Empty,
}

/// A `/play` query, resolved as far as it can be without a network or a library.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct PlayTargetDto {
    pub plan: PlayPlan,
    /// Where to open it, when the command or the link said.
    pub service: Option<MusicService>,
    /// Set when the query was a service link we recognised.
    pub link: Option<comrade_core::together::MusicLink>,
    /// Set when a session can open immediately, i.e. [`PlayPlan::OpenNow`].
    pub content: Option<TogetherContent>,
    /// The recording the query names by words. `None` for a link, whose id
    /// names the thing and whose title is the player's to report.
    pub recording: Option<comrade_core::together::Recording>,
}

/// What a frontend should actually *do* about a `/play`, once it has looked in
/// its own library.
///
/// [`PlayPlan`] is how far the query got before anyone searched; this is the step
/// after, and it is separate because only the frontend can search — `MediaStore`
/// on Android, a file the user picked on desktop. Deciding *here* is what stops
/// each frontend inventing its own idea of when a `/play` may open a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, uniffi::Enum)]
#[serde(rename_all = "snake_case")]
pub enum PlayRoute {
    /// This device found its own copy and is confident it is the right one, so a
    /// session can open on it now.
    StartTogether,
    /// We know which recording is meant and this device has no copy of it near
    /// enough to open unasked. Ask for a file rather than guessing — that is
    /// [`comrade_core::together::MATCH_CONFIDENT`]'s whole purpose.
    AskForFile,
    /// The link names audio no third-party client may decode (Spotify, Apple
    /// Music). All we can honestly do is say where to open it.
    OpenElsewhere,
    /// A YouTube embed, which can be driven in place by a frontend that has a
    /// webview to drive it in. Needs no account on either side, which is what
    /// keeps it the one source that works between two strangers.
    PlayEmbed,
    /// A streaming-service track this device is signed in to and can drive —
    /// play it there, on the listener's own subscription, and hold the session
    /// against it.
    ///
    /// Distinct from [`Self::PlayEmbed`] because the mechanism is different
    /// (a vendor SDK against an authenticated account, not a public embed) and
    /// so is the failure: an expired token or a downgraded subscription turns
    /// this back into [`Self::OpenElsewhere`] at any moment, whereas an embed
    /// either exists or does not.
    PlayOnService,
    /// Nothing usable in the query.
    Nothing,
}

/// Decide [`PlayRoute`] from a resolved query and what the caller's own library
/// turned up.
///
/// `found_local_copy` is the frontend's answer to "is a copy of this on *this*
/// device, above the confidence bar" — and it is consulted **only** for
/// [`PlayPlan::FindLocally`]. A caller that has a local file of a Spotify track
/// still gets a service route or [`PlayRoute::OpenElsewhere`]: the plan is about
/// what the *query* named, and a link does not become a local file because
/// something with a similar title happens to be on the phone.
///
/// `link` is the service link the query resolved to, when it was one, and
/// `access` is what this device is signed in to. Together they answer the
/// question `PlayPlan::NameOnly` could not: a Spotify link is
/// [`PlayRoute::PlayOnService`] on a device with a Premium account behind it and
/// [`PlayRoute::OpenElsewhere`] on one without, and no amount of looking at the
/// URL distinguishes those two devices.
pub fn play_route(
    plan: PlayPlan,
    found_local_copy: bool,
    link: Option<comrade_core::together::MusicLink>,
    access: comrade_core::together::ServiceAccess,
) -> PlayRoute {
    match plan {
        PlayPlan::Empty => PlayRoute::Nothing,
        PlayPlan::NameOnly => match link.as_ref().map(|l| l.playhead_control(&access)) {
            // Signed in and drivable: a real session, on their own subscription.
            Some(PlayheadControl::Full) => PlayRoute::PlayOnService,
            // `StartOnly` deliberately lands here rather than opening a session
            // that cannot be held. Apple Music can be started and never placed,
            // so a session on it would emit corrections nothing applies and a
            // screen that says "catching up…" while nothing catches up. Saying
            // "open it there" is the smaller promise and the true one.
            _ => PlayRoute::OpenElsewhere,
        },
        PlayPlan::OpenNow => PlayRoute::PlayEmbed,
        PlayPlan::FindLocally => {
            if found_local_copy {
                PlayRoute::StartTogether
            } else {
                PlayRoute::AskForFile
            }
        }
    }
}

/// Which tier will supply a recording this device does not have, decided
/// without a network.
///
/// [`play_route`]'s `AskForFile` is the honest answer when all a caller has is
/// its own library — but it is not the *last* answer, and this is the rung below
/// it. Given what the catalogue said (see
/// [`ComradeRuntime::catalogue_lookup`], the only part that touches a socket)
/// and whether the other side offered a copy, [`choose_audio_plan`] picks the
/// first tier that can actually supply the bytes: the peer, then an
/// openly-licensed archive, then nothing but an embed.
///
/// Pure, so the whole policy is unit-tested with no network and no filesystem —
/// and deliberately a thin pass-through rather than a second copy of the
/// ladder. The licence gate lives in `comrade_core::catalogue` and is applied
/// there, not here: a sloppy caller cannot smuggle a licensed URL past it by
/// coming through this function.
///
/// `library` is what the frontend's own resolver turned up — `MediaStore` on
/// Android, a picked file on desktop — each entry a recording and its duration
/// in milliseconds.
pub fn audio_plan(
    want: Recording,
    want_ms: u64,
    library: Vec<LibraryCandidateDto>,
    peer_has_it: bool,
    catalogue: Vec<CatalogueMatch>,
) -> AudioPlan {
    let owned: Vec<(Recording, u64)> = library
        .into_iter()
        .map(|c| (c.recording, c.duration_ms))
        .collect();
    choose_audio_plan(&want, want_ms, &owned, peer_has_it, &catalogue)
}

/// Ask a public catalogue what recording `query` names. **The one part of
/// this path that contacts a third party**, which is why it is the only part
/// behind a feature and the only part that is not pure.
///
/// What leaves the device is the query text and nothing else: no npub, no
/// contact, no library contents, and no indication that a session is being
/// planned. That is the disclosure [`ComradeRuntime::play_query`]'s doc refers
/// to, and it is the whole of it.
///
/// A free function, not a method, and that is the point: it reads nothing from
/// [`ComradeRuntime`], so a caller needs no lock — and therefore cannot hold one
/// across this network round trip, which is the shape of the two deadlocks this
/// repo has already fixed. Needs no vault either.
///
/// Results are metadata: [`CatalogueMatch::audio_url`] is `None` for
/// MusicBrainz, and for any catalogue that *does* serve audio the licence
/// gate is applied by [`audio_plan`], not here. Ordering is the catalogue's
/// own — most likely first — and capped at
/// [`comrade_core::catalogue::MAX_CANDIDATES`].
///
/// An empty list means "the catalogue has no such recording", which is a
/// real answer. A build without `catalogue-http` returns
/// [`UiError::CatalogueUnavailable`] instead, because "we cannot search" and
/// "we searched and found nothing" must not look the same to a UI.
///
/// A catalogue lookup is public data about a public recording, so this works
/// before unlock — deliberately. The alternative is asking somebody to unlock a
/// vault to find out what a song is called.
pub async fn catalogue_lookup(
    query: &str,
    jamendo_client_id: Option<String>,
) -> Result<Vec<CatalogueMatch>, UiError> {
    let q = query.trim();
    if q.is_empty() {
        return Ok(Vec::new());
    }
    #[cfg(feature = "catalogue-http")]
    {
        use comrade_core::catalogue::{CatalogueResolver, Jamendo, MusicBrainz};
        // MusicBrainz asks that clients identify themselves, and a generic
        // agent is what gets a project rate-limited. No version: the string
        // would then change with every release for no benefit to them.
        let mb = MusicBrainz::new("comrade/1.0 (https://github.com/cmullu/comrade)");
        // Both resolvers run together — neither depends on the other, and
        // serialising them doubles the wait a composer is already absorbing.
        let jamendo = async {
            match jamendo_client_id
                .as_deref()
                .filter(|id| !id.trim().is_empty())
            {
                Some(id) => Jamendo::new(id.trim()).lookup(q).await.ok(),
                // No key configured is an empty answer, not an error: this
                // catalogue is optional by design, and its absence must not
                // read as MusicBrainz having failed.
                None => None,
            }
        };
        let (mb_res, jam_res) = tokio::join!(mb.lookup(q), jamendo);
        let mut out = Vec::with_capacity(comrade_core::catalogue::MAX_CANDIDATES * 2);
        // Both failed: one error, not two half-answers. (`None` covers both
        // an unconfigured Jamendo and a failed one — the former is ordinary
        // and the latter is swallowed by design above.)
        if let (Err(e), None) = (&mb_res, &jam_res) {
            return Err(UiError::Catalogue(e.to_string()));
        }
        if let Ok(matches) = mb_res {
            out.extend(matches);
        }
        if let Some(matches) = jam_res {
            out.extend(matches);
        }
        out.truncate(comrade_core::catalogue::MAX_CANDIDATES * 2);
        Ok(out)
    }
    #[cfg(not(feature = "catalogue-http"))]
    {
        // Named so the unused-parameter lint cannot eat the signature change:
        // the lean build refuses identically whether or not a key was passed.
        let _ = jamendo_client_id;
        Err(UiError::CatalogueUnavailable)
    }
}

// ── Streaming from a server you own ─────────────────────────────────────────

/// One playable row from a self-hosted streaming source, ready for a player or
/// a Together invitation.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct StreamCandidateDto {
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub duration_ms: u64,
    /// The URL both halves of a stream session will fetch.
    pub stream_url: String,
    pub artwork_url: Option<String>,
}

/// What a streaming search came back with — **flat, not a `Result`**, because
/// none of these arms is exceptional and each needs its own sentence:
///
/// - "you have no server saved yet" ([`Self::NotConfigured`]) is a setup step,
/// - "this build cannot stream" ([`Self::BuildCannotSearch`]) is a build fact,
/// - "the server said no / could not be reached" ([`Self::Failed`]) is the
///   server's or network's answer, carried verbatim,
/// - and an empty [`Self::Found`] really means "nothing matched".
///
/// Collapsing these into one error string is how a missing config ends up
/// rendered as "no results" — the exact wrong-answer shape §20 exists to
/// prevent. It is also deliberately not a new `UiError` variant: that enum is
/// mirrored into the Flutter bridge, where adding a case breaks four CI lanes
/// for no informational gain over a typed value.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum StreamSearchOutcome {
    Found {
        candidates: Vec<StreamCandidateDto>,
    },
    /// No server has been saved yet. The UI's job is to point at settings.
    NotConfigured,
    /// The build has no network support (`catalogue-http` off) — a different
    /// sentence from any server-side failure.
    BuildCannotSearch,
    /// The server refused, lied, or was unreachable. Its own words, not ours.
    Failed {
        reason: String,
    },
}

/// Search one Subsonic/Navidrome server — the online-streaming source this app
/// sanctions: your library, your server, plain progressive HTTPS out.
///
/// A free function like [`catalogue_lookup`] and for the same reason: nothing
/// here reads runtime state, so no caller can hold a lock across the round
/// trip. `config` arrives per call from the frontend's own settings store; it
/// is never persisted here, and `None` is the ordinary pre-setup state rather
/// than an error.
///
/// Rows whose URL cannot pass the peer guard are dropped inside core, so every
/// candidate returned is one a Together session will accept unchanged.
pub async fn subsonic_search(config: Option<SubsonicConfig>, query: String) -> StreamSearchOutcome {
    #[cfg(feature = "catalogue-http")]
    {
        let Some(cfg) = config else {
            return StreamSearchOutcome::NotConfigured;
        };
        let q = query.trim();
        if q.is_empty() {
            return StreamSearchOutcome::Found {
                candidates: Vec::new(),
            };
        }
        match subsonic::lookup(&cfg, q).await {
            Ok(rows) => StreamSearchOutcome::Found {
                candidates: rows
                    .into_iter()
                    .filter_map(|row| {
                        Some(StreamCandidateDto {
                            title: row.title,
                            artist: row.artist,
                            album: row.album,
                            duration_ms: row.duration_ms,
                            // Built under the peer guard already; a `None`
                            // here means the origin failed it, and the filter
                            // stays total rather than trusting that upstream.
                            stream_url: row.stream_url?,
                            artwork_url: row.artwork_url,
                        })
                    })
                    .collect(),
            },
            Err(subsonic::SearchError::BadServer(issue)) => StreamSearchOutcome::Failed {
                reason: issue.to_string(),
            },
            Err(subsonic::SearchError::ServerRejected(detail)) => {
                StreamSearchOutcome::Failed { reason: detail }
            }
            Err(subsonic::SearchError::Unreachable(detail)) => {
                StreamSearchOutcome::Failed { reason: detail }
            }
        }
    }
    #[cfg(not(feature = "catalogue-http"))]
    {
        let _ = (config, query);
        StreamSearchOutcome::BuildCannotSearch
    }
}

// ── Public collections & lyrics ─────────────────────────────────────────────

/// One Internet Archive collection, as a search row.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ArchiveItemDto {
    pub identifier: String,
    pub title: String,
    pub creator: String,
}

/// One playable track from any public source. Same shape the streaming server
/// answers with — a URL already guarded in core is a URL any of these screens
/// can start a session from without a second thought.
pub type StreamCandidate = StreamCandidateDto;

/// What an archive search came back with — flat, like [`StreamSearchOutcome`].
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum CollectionSearchOutcome {
    Found { items: Vec<ArchiveItemDto> },
    BuildCannotSearch,
    Failed { reason: String },
}

/// What a track listing (archive item / podcast feed) came back with.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum TrackListOutcome {
    Found {
        candidates: Vec<StreamCandidate>,
    },
    /// The feed address itself failed the shareable-URL guard.
    Refused {
        reason: String,
    },
    BuildCannotSearch,
    Failed {
        reason: String,
    },
}

/// One timed lyric line.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct LyricLineDto {
    pub at_ms: u64,
    pub text: String,
}

/// What a lyrics lookup came back with. `Found` with an empty list means "no
/// synced lyrics exist for this" — a real answer, distinct from a failure.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum LyricsOutcome {
    Found { lines: Vec<LyricLineDto> },
    BuildCannotSearch,
    Failed { reason: String },
}

#[cfg(feature = "catalogue-http")]
fn tracks_outcome(result: Result<Vec<public_sources::PublicTrack>, String>) -> TrackListOutcome {
    match result {
        Ok(rows) => TrackListOutcome::Found {
            candidates: rows
                .into_iter()
                .map(|t| StreamCandidate {
                    title: t.title,
                    artist: t.artist,
                    album: t.album,
                    duration_ms: t.duration_ms,
                    stream_url: t.url,
                    artwork_url: t.artwork_url,
                })
                .collect(),
        },
        Err(e) => TrackListOutcome::Failed { reason: e },
    }
}

/// Search the Internet Archive's audio collections.
pub async fn archive_search(query: String) -> CollectionSearchOutcome {
    #[cfg(feature = "catalogue-http")]
    {
        match public_sources::archive_search(query.trim()).await {
            Ok(items) => CollectionSearchOutcome::Found {
                items: items
                    .into_iter()
                    .map(|i| ArchiveItemDto {
                        identifier: i.identifier,
                        title: i.title,
                        creator: i.creator,
                    })
                    .collect(),
            },
            Err(e) => CollectionSearchOutcome::Failed { reason: e },
        }
    }
    #[cfg(not(feature = "catalogue-http"))]
    {
        let _ = query;
        CollectionSearchOutcome::BuildCannotSearch
    }
}

/// List one archive item's playable files.
pub async fn archive_tracks(identifier: String) -> TrackListOutcome {
    #[cfg(feature = "catalogue-http")]
    {
        tracks_outcome(public_sources::archive_tracks(&identifier).await)
    }
    #[cfg(not(feature = "catalogue-http"))]
    {
        let _ = identifier;
        TrackListOutcome::BuildCannotSearch
    }
}

/// List one podcast feed's episodes.
///
/// A refused feed address is its own arm: "that is not a shareable https
/// address" and "the feed could not be fetched" are different sentences about
/// different fixes.
pub async fn podcast_episodes(feed_url: String) -> TrackListOutcome {
    #[cfg(feature = "catalogue-http")]
    {
        if !comrade_core::together::valid_stream_url(feed_url.trim()) {
            return TrackListOutcome::Refused {
                reason: "the feed must be an https address at a named host".into(),
            };
        }
        tracks_outcome(public_sources::podcast_episodes(feed_url.trim()).await)
    }
    #[cfg(not(feature = "catalogue-http"))]
    {
        let _ = feed_url;
        TrackListOutcome::BuildCannotSearch
    }
}

/// Synced lyrics, newest lines first by time. Empty `Found` = none exists.
pub async fn lyrics_lookup(title: String, artist: String, duration_ms: u64) -> LyricsOutcome {
    #[cfg(feature = "catalogue-http")]
    {
        match public_sources::lrc_lookup(&title, &artist, duration_ms).await {
            Ok(lines) => LyricsOutcome::Found {
                lines: lines
                    .into_iter()
                    .map(|l| LyricLineDto {
                        at_ms: l.at_ms,
                        text: l.text,
                    })
                    .collect(),
            },
            Err(e) => LyricsOutcome::Failed { reason: e },
        }
    }
    #[cfg(not(feature = "catalogue-http"))]
    {
        let _ = (title, artist, duration_ms);
        LyricsOutcome::BuildCannotSearch
    }
}

// ── The player's own library ─────────────────────────────────────────────────
//
// Favourites, recently-played, playlists and the persisted queue: the half
// `docs/TOGETHER.md` §20 called "the half that makes it a music player rather
// than a session tool", which until now had no storage, no FFI and no screen.
//
// Everything here lives in the **encrypted vault**, because this is personal
// listening data — what somebody plays and loves is a diary — and the vault is
// where this app keeps diaries. The consequence is stated rather than hidden:
// every method answers [`UiError::VaultLocked`] while locked. A music player
// that shows an empty library when the phone is locked is telling the truth.
//
// The trees are written through [`EncryptedStore`]'s generic put/get, like the
// settings tree, rather than through new `comrade_storage` typed helpers: four
// small collections with one accessor each are not worth a repository layer,
// and runtime.rs already owns the settings tree the same way.

/// Where a track came from, which decides how it can be played again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, uniffi::Enum)]
#[serde(rename_all = "snake_case")]
pub enum PlayerTrackKind {
    /// A file on this device (`local:<media store id>`).
    Local,
    /// A URL this device streams (`stream:<url>`).
    Stream,
    /// A YouTube video (`youtube:<video id>`).
    Youtube,
}

/// One track in the player's own library.
///
/// `key` is the identity everywhere: favourites are keyed by it, history is
/// deduplicated on it, playlists order by it. It encodes both kind and id —
/// `local:4171`, `stream:https://…`, `youtube:dQw4w9WgXcQ` — so a single
/// string can be asked "do I love you?" without carrying the rest of the row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct PlayerTrackDto {
    pub key: String,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    /// Zero when nothing reliable is known; [`PlayerTrackKind::Stream`] rows
    /// carry what their server said, local files what MediaStore did.
    pub duration_ms: u64,
    /// The stream URL or YouTube id, when the track is not on this device.
    pub url: Option<String>,
    pub kind: PlayerTrackKind,
}

impl PlayerTrackDto {
    /// A stream candidate from a server search, as a library track.
    pub fn from_stream(c: &StreamCandidateDto) -> Self {
        Self {
            key: format!("stream:{}", c.stream_url),
            title: c.title.clone(),
            artist: c.artist.clone(),
            album: c.album.clone(),
            duration_ms: c.duration_ms,
            url: Some(c.stream_url.clone()),
            kind: PlayerTrackKind::Stream,
        }
    }
}

/// One recently-played answer: the track and *when* it was last played.
///
/// History is one entry per track keyed by its [key][PlayerTrackDto::key] with
/// the timestamp updated, not an append log — playing the same song twice does
/// not make it twice as interesting, and a list that shows it once per day
/// played is what "recently played" means on every player this feature was
/// modelled on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct HistoryEntryDto {
    pub track: PlayerTrackDto,
    /// Milliseconds since the epoch, as the caller recorded it. Core never
    /// invents timestamps: a caller that says zero gets a zero ordered last.
    pub at_ms: u64,
}

/// A named, ordered list of tracks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct PlaylistDto {
    pub id: String,
    pub name: String,
    pub created_at_ms: u64,
    pub tracks: Vec<PlayerTrackDto>,
}

/// The queue as it stood when saved, so a killed app resumes mid-queue rather
/// than at the top of a lost one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct SavedQueueDto {
    pub tracks: Vec<PlayerTrackDto>,
    /// Which entry was playing. Out of bounds after edits upstream is handled
    /// by the loader: it clamps rather than errors, because a queue one past
    /// its end is still a queue.
    pub index: u32,
    pub position_ms: u64,
    pub saved_at_ms: u64,
}

/// How many history entries survive. A year of daily listening is a few
/// hundred; beyond that the oldest falls off, which is the point of "recent".
pub const HISTORY_MAX_ENTRIES: usize = 100;

/// Order history newest-first and cap it.
///
/// Pure so the rule is testable without a vault: sort descending by time,
/// drop past [`HISTORY_MAX_ENTRIES`]. Stable sort, so two entries recorded at
/// the same millisecond keep the order the store handed them up in.
pub fn prune_history(mut entries: Vec<HistoryEntryDto>) -> Vec<HistoryEntryDto> {
    entries.sort_by_key(|e| std::cmp::Reverse(e.at_ms));
    entries.truncate(HISTORY_MAX_ENTRIES);
    entries
}

/// Move the track at `from` so it sits at `to`, shifting the rest.
///
/// Pure, so the reorder rule is testable without a vault and the Android
/// decision layer can mirror it exactly (`TogetherDecisions.reorderedOrder`).
/// Both indices are **clamped** into range rather than erroring — a drag that
/// ends past the last row is a drag to the end, not a mistake — and `from ==
/// to`, a one-track list and an empty list are all no-ops. The moved element
/// lands *at* `to` in the result: reordering `[a, b, c]` from `0` to `2`
/// yields `[b, c, a]`.
pub fn reorder_tracks(
    mut tracks: Vec<PlayerTrackDto>,
    from: usize,
    to: usize,
) -> Vec<PlayerTrackDto> {
    if tracks.is_empty() {
        return tracks;
    }
    let from = from.min(tracks.len() - 1);
    let to = to.min(tracks.len() - 1);
    if from == to {
        return tracks;
    }
    let item = tracks.remove(from);
    tracks.insert(to, item);
    tracks
}

/// An id for a new playlist.
///
/// Nanoseconds plus a process-lifetime counter: unique enough for keys in one
/// person's vault, and honest about being no more than that — this is not a
/// distributed identifier and must not become one.
fn fresh_playlist_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("pl{nanos:x}{:x}", COUNTER.fetch_add(1, Ordering::Relaxed))
}

const PLAYER_FAVOURITES_TREE: &str = "player_favourites";
const PLAYER_HISTORY_TREE: &str = "player_history";
const PLAYER_PLAYLISTS_TREE: &str = "player_playlists";
const PLAYER_QUEUE_TREE: &str = "player_queue";
/// One queue snapshot per device, under one key — there is exactly one live
/// queue, and a previous snapshot is overwritten, not archived.
const PLAYER_QUEUE_KEY: &str = "current";

/// Whether a catalogue answer may be downloaded, and if not, why.
///
/// **Flat and typed rather than a `Result<_, DownloadRefusal>`**, because this is
/// not a failure — it is the answer to "should this row have a download button".
/// A UI asks it while drawing a list, for every row, and none of the three
/// refusals is exceptional: MusicBrainz never serves audio, so [`Self::NoAudio`]
/// is the *normal* verdict today.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, uniffi::Enum)]
#[serde(rename_all = "snake_case")]
pub enum DownloadVerdictDto {
    /// The licence permits a copy. The licence is carried so a UI can say on
    /// what basis — somebody downloading a track is entitled to know.
    Permitted { licence: OpenLicence },
    /// A metadata-only answer. No button.
    NoAudio,
    /// Audio is there and its licence does not permit copying it. **Serving the
    /// bytes is not licensing them**, which is the whole reason this is a
    /// separate verdict from [`Self::NoAudio`] rather than both being "no".
    NotOpenlyLicensed,
    /// The catalogue gave a non-HTTPS URL.
    InsecureUrl,
}

/// A track that has been fetched, ready for the platform's music library.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct DownloadedTrackDto {
    /// The whole file.
    ///
    /// **Buffered in memory, and that is a real limit rather than an oversight.**
    /// `comrade_core::download::MAX_TRACK_BYTES` is 96 MB to accommodate lossless
    /// album tracks, so a worst-case download is a 96 MB `ByteArray` crossing the
    /// FFI. Typical tracks are 3–12 MB and this is unremarkable for them.
    /// Streaming to a caller-supplied path instead — bounded RAM, one write —
    /// is the follow-up recorded in `docs/TOGETHER.md` §21; it is not done here
    /// because it puts filesystem code in the core for a case no archive this
    /// serves has actually hit.
    pub bytes: Vec<u8>,
    /// `Artist - Title.ext`, already sanitised of path separators, control
    /// characters and reserved punctuation — see
    /// `comrade_core::download::filename_for`. A frontend must not build its own.
    pub filename: String,
    pub mime: String,
    /// The metadata to write into the library's own columns, since no in-file
    /// tags are added (see the `download` module header).
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
}

/// Whether `m` may be downloaded.
///
/// Pure — no network, no lock, no vault. Safe to call per row while drawing a
/// list, which is what it is for.
pub fn download_verdict(m: CatalogueMatch) -> DownloadVerdictDto {
    match permit_download(&m) {
        Ok(ok) => DownloadVerdictDto::Permitted {
            licence: ok.licence(),
        },
        Err(DownloadRefusal::NoAudio) => DownloadVerdictDto::NoAudio,
        Err(DownloadRefusal::NotOpenlyLicensed) => DownloadVerdictDto::NotOpenlyLicensed,
        Err(DownloadRefusal::InsecureUrl) => DownloadVerdictDto::InsecureUrl,
    }
}

/// Fetch a catalogue answer's audio.
///
/// **Re-runs the gate rather than trusting a prior [`download_verdict`] call**,
/// and takes the [`CatalogueMatch`] rather than a URL, so there is no argument a
/// caller could assemble that downloads something the licence does not permit.
/// A UI that skipped the verdict entirely still cannot get past this.
///
/// A free function for the same reason [`catalogue_lookup`] is: it reads nothing
/// from [`ComradeRuntime`], so no caller can hold a lock across the transfer.
/// Needs no vault — this is public audio under a public licence.
pub async fn download_track(m: CatalogueMatch) -> Result<DownloadedTrackDto, UiError> {
    let permitted =
        permit_download(&m).map_err(|refusal| UiError::Download(refusal.to_string()))?;
    #[cfg(feature = "catalogue-http")]
    {
        let got = comrade_core::download::fetch_track(&permitted)
            .await
            .map_err(|e| UiError::Download(e.to_string()))?;
        Ok(DownloadedTrackDto {
            bytes: got.bytes,
            filename: got.filename,
            mime: got.mime,
            title: got.recording.title,
            artist: got.recording.artist,
            album: got.recording.album,
        })
    }
    #[cfg(not(feature = "catalogue-http"))]
    {
        // The gate above still ran, deliberately: a build without the feature must
        // refuse an unlicensed download for the *licence* reason, not report that
        // it cannot download anything. Otherwise a test build would make every
        // refusal look like a missing feature.
        let _ = permitted;
        Err(UiError::CatalogueUnavailable)
    }
}

/// One thing this device already has, as the frontend's library resolver found
/// it.
///
/// A named record rather than a tuple because it crosses two FFI boundaries:
/// uniffi and flutter_rust_bridge both render a `(Recording, u64)` as something
/// positional and unreadable, and a caller that swaps the two arguments gets a
/// duration compared against a title.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct LibraryCandidateDto {
    pub recording: Recording,
    /// As this device's own metadata reports it. `0` when unknown, which
    /// [`comrade_core::together::match_score`] treats as "no duration evidence"
    /// rather than as a zero-length track.
    pub duration_ms: u64,
}

/// One step of handing a large attachment over, on its way to the frontend that
/// owns the peer connection.
///
/// Same division of labour as [`TogetherShareDto`], for the same reason: WebRTC
/// lives in the frontend, and mirroring the negotiation here as well would mean
/// two state machines that have to agree — the shape of both call bugs this repo
/// has already fixed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct AttachmentHandoffDto {
    /// Scopes every signal of one transfer. A signal naming a transfer the
    /// frontend does not have is its to drop.
    pub transfer_id: String,
    pub peer: String,
    pub signal: HandoffSignal,
}

/// Whether a transfer may run over the path ICE actually chose.
///
/// The verdict and the reason are separate fields rather than a nested enum
/// because this crosses two FFI boundaries and gets rendered by three UIs; the
/// typed [`TransferVerdict`] stays the source of truth in core and this is its
/// flattening, the same way `CallSignal` is flattened for the call UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct ShareVerdictDto {
    /// `allow` · `needs_consent` · `refuse`.
    pub verdict: String,
    /// How the path was classified: `host` · `srflx` · `relay` · `unknown`.
    pub path: String,
    /// Present when the verdict is a refusal.
    pub reason: Option<RefusalReason>,
    /// Present when the verdict is `needs_consent`: how many bytes would go
    /// through someone else's relay, so the question can name the number.
    pub relayed_bytes: Option<u64>,
}

/// The live session, as the runtime keeps it. Never persisted, never more than
/// one — see [`ComradeRuntime::together`].
struct TogetherSession {
    id: String,
    /// The other person, as an npub (what every DTO and the arbitration use).
    peer: String,
    /// The same key in hex, so a send does not have to re-derive it.
    peer_hex: String,
    content: TogetherContent,
    we_lead: bool,
    our_npub: String,
    /// Whether they have answered our invitation yet.
    joined: bool,
    /// The last command either side applied — the Lamport position both devices
    /// order against.
    applied: CommandStamp,
    local_pos_ms: u64,
    local_playing: bool,
    peer_pos_ms: u64,
    peer_playing: bool,
    /// Their clock when they last told us where they were.
    peer_at_ms: u64,
    /// Our clock when we last heard anything at all from them — the TTL reads
    /// this, so a heartbeat keeps a session alive even if nothing changed.
    last_heard_ms: u64,
    /// Our clock when we last moved the playhead automatically.
    last_seek_ms: u64,
    /// Whether the frontend has a direct peer-to-peer channel up for this
    /// session, and signals should go down it instead of to a relay.
    ///
    /// Declared by the frontend rather than discovered here, because the
    /// connection belongs to the frontend — the same division of labour
    /// [`TogetherShareDto`] describes. It is per-session and never persisted:
    /// a channel does not outlive the session it was negotiated inside.
    ///
    /// A claim, not a fact — see [`direct_evidence_ms`](Self::direct_evidence_ms)
    /// for what keeps it honest.
    direct_ready: bool,
    /// The last moment the direct channel gave any sign of being alive: the
    /// frontend declaring it up, or an envelope arriving over it.
    ///
    /// `direct_ready` alone is a promise the frontend has no way to keep — a
    /// closed socket reports nothing, and a frontend that crashes past its own
    /// close handler reports nothing forever. `direct_path_live` reads this to
    /// decide whether the promise is still worth acting on.
    direct_evidence_ms: u64,
    /// The rate trim we last asked this device's player for. Tracked because a
    /// trim is sticky — see `SyncSample::local_rate`; without it the ladder can
    /// never take one back off.
    local_rate: f64,
    /// How far behind our reported position the sound actually leaves this
    /// device's speaker, as the frontend measured it. Zero means unmeasured.
    local_output_latency_ms: u64,
    /// The same figure for the peer, from their last heartbeat.
    peer_output_latency_ms: u64,
    clock: ClockFilter,
    /// Our own recent send stamps, so an echo coming back can be matched to the
    /// message that provoked it. Four is plenty: an echo older than that is
    /// older than the probe window cares about.
    sent_at_ms: std::collections::VecDeque<u64>,
    /// What to echo back on our next message, so *they* can measure the round
    /// trip too. Both sides probe from the same traffic.
    echo_back: Option<ClockEcho>,
}

impl TogetherSession {
    fn dto(&self) -> TogetherSessionDto {
        TogetherSessionDto {
            session_id: self.id.clone(),
            peer: self.peer.clone(),
            content: self.content.clone(),
            we_lead: self.we_lead,
            joined: self.joined,
            pos_ms: self.local_pos_ms,
            playing: self.local_playing,
        }
    }

    /// Record that we sent something at `at_ms`, keeping the ring small.
    fn note_send(&mut self, at_ms: u64) {
        self.sent_at_ms.push_back(at_ms);
        while self.sent_at_ms.len() > 4 {
            self.sent_at_ms.pop_front();
        }
    }

    /// Fold one incoming echo into the clock filter. `their_at_ms` is when they
    /// sent the message carrying it, `our_recv_ms` when we got it — which,
    /// together with the two stamps inside the echo, are the four timestamps a
    /// round trip needs.
    fn observe_clock(&mut self, echo: Option<ClockEcho>, their_at_ms: u64, our_recv_ms: u64) {
        if let Some(echo) = echo {
            // `echo.your_at_ms` is one of *our* sends, quoted back at us.
            if self.sent_at_ms.contains(&echo.your_at_ms) {
                self.clock
                    .observe(echo.your_at_ms, echo.my_recv_ms, their_at_ms, our_recv_ms);
            }
        }
        self.echo_back = Some(ClockEcho {
            your_at_ms: their_at_ms,
            my_recv_ms: our_recv_ms,
        });
    }
}

/// A voice/video call-log entry as the frontend sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct CallRecordDto {
    pub id: String,
    pub peer: String,
    pub media: String,
    pub incoming: bool,
    pub outcome: String,
    pub started_at: u64,
    pub duration_secs: u64,
}

impl From<comrade_storage::CallRecord> for CallRecordDto {
    fn from(c: comrade_storage::CallRecord) -> Self {
        Self {
            id: c.id,
            peer: c.peer_npub,
            media: c.media,
            incoming: c.incoming,
            outcome: c.outcome,
            started_at: c.started_at,
            duration_secs: c.duration_secs,
        }
    }
}

/// A pending message request: a stranger's DM that is gated out of the chat
/// list until the user accepts it. Only the preview and timing are exposed —
/// the peer's chosen handle is not shared until they, in turn, accept.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct MessageRequestDto {
    pub peer: String,
    pub last_message: String,
    pub last_at: u64,
}

/// This device's Sakha/Sakhi pairing state — lets the frontend show "pair
/// with your partner" or, for a returning paired couple, "continue as
/// {role}" without asking for the partner's key again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct SakhaStatusDto {
    pub paired: bool,
    pub partner_npub: Option<String>,
    /// `"sakha"` or `"sakhi"` — which role this device paired as, if known.
    pub role: Option<String>,
}

/// A persisted Sakha/Sakhi pairing, so a returning couple survives a
/// relaunch without re-exchanging keys. Never holds the derived symmetric
/// key — that is re-derived from the partner's pubkey plus our own secret
/// key every time [`SakhaEngine::pair_with`] runs.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SakhaPairingRecord {
    /// Partner's public key, hex-encoded.
    partner_pubkey_hex: String,
    /// `"sakha"` or `"sakhi"`.
    role: String,
}

/// A NIP-94 encrypted-media reference as the frontend sees it (no key material).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct MediaMessageDto {
    /// NIP-94 event id — the handle passed back to `download_and_decrypt_media`.
    pub event_id: String,
    pub url: String,
    pub mime_type: String,
    pub caption: String,
    /// Bech32/hex pubkey of the counterpart (sender for incoming).
    pub sender: String,
    pub created_at: u64,
    /// Size of the encrypted blob in bytes.
    pub size: u64,
    /// Whether *this device* sent it (mirrors `MessageDto::outgoing`) — needed
    /// to tell the two apart once media from both directions is merged into
    /// one history by [`ComradeRuntime::media_with`].
    pub outgoing: bool,
}

/// Decrypted media handed back to the frontend. Bytes are base64-encoded so the
/// IPC payload stays compact (the webview rebuilds a `Blob` from it).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct MediaBytesDto {
    pub mime_type: String,
    pub base64: String,
}

/// Locally persisted pointer to an encrypted blob, keyed by NIP-94 event id.
/// Holds everything needed to *re-derive* the key — but never the key itself.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct MediaRef {
    /// NIP-94 event id (hex) — duplicates the store key this row lives under,
    /// so a full-tree scan ([`ComradeRuntime::media_with`]) can rebuild a
    /// complete [`MediaMessageDto`] without a second round trip per row.
    /// Defaulted so refs written before this field are still readable.
    #[serde(default)]
    event_id: String,
    url: String,
    /// Hex pubkey of the other party (recipient if outgoing, sender if incoming).
    peer_pubkey: String,
    mime_type: String,
    caption: String,
    size: u64,
    /// SHA-256 of the *ciphertext* blob (NIP-94 `x`), verified before decrypt.
    /// Defaulted so refs written before this field are still readable.
    #[serde(default)]
    sha256_hex: String,
    outgoing: bool,
    created_at: u64,
}

/// The private envelope carried inside the E2E DM that points at the blob.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct MediaEnvelope {
    /// Format marker / version; must equal 1.
    comrade_media: u8,
    event_id: String,
    url: String,
    mime: String,
    caption: String,
    size: u64,
    /// SHA-256 of the ciphertext (NIP-94 `x`) so the recipient can fail fast on
    /// a wrong/tampered blob. Defaulted for envelopes sent before this field.
    #[serde(default)]
    sha256_hex: String,
}

/// Detect and parse a Comrade media envelope out of a decrypted DM body.
fn parse_media_envelope(content: &str) -> Option<MediaEnvelope> {
    let env: MediaEnvelope = serde_json::from_str(content).ok()?;
    (env.comrade_media == 1).then_some(env)
}

/// Whether a queued outbox payload is a media reference rather than chat text.
///
/// The flush loop needs this: a text message that finally reaches a relay gets
/// its stored row re-keyed to the relay's event id, but a media envelope has no
/// stored row — re-keying one would *create* a text message whose body is the
/// raw envelope JSON, which would then render as a chat bubble and as the chat
/// list's preview. See [`RuntimeHandles::flush_outbox`].
fn is_media_envelope(content: &str) -> bool {
    parse_media_envelope(content).is_some()
}

/// Longest MIME type accepted from a caller or a peer. Real types are short
/// (`application/vnd.openxmlformats-officedocument.presentationml.slide` is 65);
/// this bounds what a peer can make us persist and render.
const MAX_MIME_LEN: usize = 128;

/// Longest attachment caption we send or store. A caption is free text chosen
/// by the sender and rendered verbatim by every frontend, so it is bounded on
/// both the way out and the way in.
const MAX_CAPTION_LEN: usize = 512;

/// Fallback for an attachment whose MIME type we could not use.
const DEFAULT_MIME: &str = "application/octet-stream";

/// Validate and normalise a caller-supplied MIME type.
///
/// MIME types are case-insensitive (RFC 2045 §5.1), so this lowercases them —
/// otherwise `IMAGE/PNG` would miss every `starts_with("image/")` test a
/// frontend makes and a photo would render as an unopenable file. Anything
/// blank, oversized, or carrying control characters / newlines is rejected
/// rather than quietly patched: it is a caller bug, and a header-shaped value
/// has no business reaching an HTTP `Content-Type`.
fn validate_mime_type(mime: &str) -> Result<String, UiError> {
    let trimmed = mime.trim();
    if trimmed.is_empty() {
        return Err(UiError::Engine("attachment has no MIME type".into()));
    }
    if trimmed.len() > MAX_MIME_LEN {
        return Err(UiError::Engine(format!(
            "MIME type is {} characters; the limit is {MAX_MIME_LEN}",
            trimmed.len()
        )));
    }
    if trimmed.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err(UiError::Engine(
            "MIME type contains control characters".into(),
        ));
    }
    if !trimmed.contains('/') {
        return Err(UiError::Engine(format!("malformed MIME type: {trimmed}")));
    }
    Ok(trimmed.to_ascii_lowercase())
}

/// Make a peer-supplied string safe to persist and render: strip control
/// characters (a caption is one line of text in every frontend, and an embedded
/// newline or `\r` would let a peer forge extra UI lines) and truncate to `max`
/// on a character boundary.
fn sanitise_untrusted_text(text: &str, max: usize) -> String {
    text.chars()
        .filter(|c| !c.is_control())
        .take(max)
        .collect::<String>()
        .trim()
        .to_string()
}

/// The chat-list / message-request line for a conversation whose newest item is
/// an attachment. One helper so every surface says the same thing.
fn attachment_preview(caption: &str) -> String {
    let caption = caption.trim();
    if caption.is_empty() {
        "📎 Attachment".to_string()
    } else {
        format!("📎 {caption}")
    }
}

/// The newest attachment in one conversation, reduced to what a chat-list row
/// needs. Built by [`newest_media_by_peer`].
struct MediaSummary {
    created_at: u64,
    preview: String,
    outgoing: bool,
}

/// Newest attachment per peer (keyed by npub), for the chat list and the
/// message-request inbox.
///
/// Media references are *not* stored as messages — the envelope that carries
/// them is consumed by `dispatch_incoming_dm` and never persisted as chat text
/// (otherwise every attachment would also appear as a bubble full of JSON). So
/// any surface that answers "what is the newest thing in this thread?" has to
/// read this tree too, or a photo-only conversation is invisible and a thread
/// whose last item is a photo shows a stale text preview.
fn newest_media_by_peer(
    store: &comrade_storage::EncryptedStore,
) -> Result<std::collections::HashMap<String, MediaSummary>, UiError> {
    let mut newest: std::collections::HashMap<String, MediaSummary> =
        std::collections::HashMap::new();
    for reff in store
        .values::<MediaRef>(MEDIA_REFS_TREE)
        .map_err(|e| UiError::Storage(e.to_string()))?
    {
        let peer = to_npub(&reff.peer_pubkey);
        if newest
            .get(&peer)
            .is_some_and(|existing| existing.created_at >= reff.created_at)
        {
            continue;
        }
        newest.insert(
            peer,
            MediaSummary {
                created_at: reff.created_at,
                preview: attachment_preview(&reff.caption),
                outgoing: reff.outgoing,
            },
        );
    }
    Ok(newest)
}

/// Parse an npub (bech32) or hex public key.
fn parse_pubkey(s: &str) -> Result<PublicKey, UiError> {
    PublicKey::parse(s).map_err(|e| UiError::Engine(format!("invalid pubkey: {e}")))
}

/// Normalise a hex or bech32 public key to a canonical bech32 `npub` for the
/// frontend. Both the incoming and outgoing sides emit the same form, so the UI
/// can key conversations (and the couple panel, which is keyed by the pasted
/// npub) consistently. Falls back to the input unchanged if it cannot be parsed.
fn to_npub(pubkey: &str) -> String {
    PublicKey::parse(pubkey)
        .ok()
        .and_then(|pk| pk.to_bech32().ok())
        .unwrap_or_else(|| pubkey.to_string())
}

/// The local user's profile: the unforgeable identity (npub) plus the chosen
/// display handle. The handle is an alias, never an identifier — see
/// [`ComradeRuntime::set_username`] for the trust model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct ProfileDto {
    pub npub: String,
    pub username: Option<String>,
    /// The user's own bio, as this device has it. Editable — see
    /// [`ComradeRuntime::set_about`].
    #[serde(default)]
    pub about: Option<String>,
    /// The `picture` URL currently published for this identity. Shown to the user
    /// because it is what everybody else sees, and because it is public.
    #[serde(default)]
    pub picture: Option<String>,
    /// Whether the bytes of that picture are in the local cache, i.e. whether a
    /// real avatar can be drawn rather than initials.
    #[serde(default)]
    pub avatar_cached: bool,
}

/// Everything a profile page draws for a peer, from the local cache alone — one
/// call, no relay, works with no connection at all.
///
/// Assembled from three places that a frontend would otherwise have to join
/// itself: the saved contact (alias, comrade), the cached Kind-0 record (handle,
/// bio, picture, nip05) and presence recomputed against the clock.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct PeerProfileDto {
    pub npub: String,
    /// The user-chosen local alias (petname). Empty = none set. First in the
    /// display precedence alias → name → key, and the only one of the three the
    /// peer cannot influence.
    pub alias: String,
    /// The peer's own published @handle — a self-declared claim, never an
    /// identifier.
    pub name: Option<String>,
    pub about: Option<String>,
    /// The `picture` URL as published. Handed over so a page can say "they have a
    /// picture we have not loaded"; a frontend must never fetch it itself.
    pub picture: Option<String>,
    /// Unverified: the core does not check NIP-05, so this carries no flag a UI
    /// could turn into a checkmark.
    pub nip05: Option<String>,
    pub lud16: Option<String>,
    /// Whether vetted bytes for `picture` are cached right now — the
    /// avatar-vs-initials test, and the only avatar question a renderer asks.
    pub avatar_cached: bool,
    pub contact: bool,
    pub comrade: bool,
    /// Whether this peer is blocked. Reported so the page can say so; there is no
    /// unblock command to pair with it, and inventing a button would be a switch
    /// that does nothing.
    pub blocked: bool,
    /// Recomputed against the current clock, never a stored flag — and always
    /// `false` for a peer who is not a comrade, because presence only flows
    /// between comrades.
    pub online: bool,
    pub last_seen_at: u64,
    pub peer_marked_us: bool,
    /// When the cached record was last written. Lets a page say "as of …" rather
    /// than presenting a day-old bio as live truth.
    pub updated_at: u64,
}

/// A profile discovered via relay search. `npub` is the identity; `name` is a
/// self-declared, non-unique handle — the UI must always show both.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct FoundProfileDto {
    pub npub: String,
    pub name: Option<String>,
    pub about: Option<String>,
    /// The `picture` URL as published. Handed over so a row can show that a
    /// picture *exists*; a frontend must never fetch it for a stranger.
    #[serde(default)]
    pub picture: Option<String>,
    /// A NIP-05 address, unverified — the core does not check it, so no frontend
    /// is given a boolean it could draw a checkmark from.
    #[serde(default)]
    pub nip05: Option<String>,
}

/// A saved contact: an npub pinned on first add (trust-on-first-use) with a
/// local alias. A different key later claiming the same handle can never
/// silently replace this entry — contacts are keyed by npub, not by name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct ContactDto {
    pub npub: String,
    /// The *user-chosen* local alias (petname). Empty = none set.
    pub alias: String,
    /// The peer's own published @handle, from the local profile cache.
    /// Display precedence is alias → name → key; never trust name alone.
    pub name: Option<String>,
    /// Whether the user chose this contact as a comrade — see
    /// [`ComradeRuntime::set_comrade`].
    pub comrade: bool,
}

/// One entry of the chat list: a peer plus the newest message in the thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct ConversationDto {
    /// Peer npub (canonical bech32) — the conversation key.
    pub peer: String,
    /// Saved contact alias for the peer, when one exists (user-chosen).
    pub alias: Option<String>,
    /// The peer's own published @handle, from the local profile cache.
    pub peer_name: Option<String>,
    pub last_message: String,
    pub last_at: u64,
    pub last_outgoing: bool,
    /// Whether this peer is one of the user's comrades.
    pub comrade: bool,
    /// Whether that comrade is online *right now* — always recomputed against
    /// the clock, never a stored flag (see [`PresenceDto::online`]). Always
    /// `false` for a peer who isn't a comrade: presence only flows between
    /// comrades, so there is nothing to know.
    pub online: bool,
}

/// A comrade: a contact the user chose to exchange presence with, plus what
/// their last beacon said.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct ComradeDto {
    pub npub: String,
    /// The user-chosen local alias (petname). Empty = none set.
    pub alias: String,
    /// The peer's own published @handle, from the local profile cache.
    pub name: Option<String>,
    /// Live presence, recomputed against the current clock.
    pub online: bool,
    /// Send time (unix seconds) of their last beacon; `0` if none ever
    /// arrived — i.e. they haven't marked us back (or haven't been online
    /// since we marked them).
    pub last_seen_at: u64,
    /// Whether they have marked *us* as their comrade. `false` means we will
    /// never see them online, however long we wait — the UI must say so
    /// rather than showing a permanently grey dot with no explanation.
    pub peer_marked_us: bool,
}

/// A single peer's presence, for a chat header or a contact row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct PresenceDto {
    pub peer: String,
    /// Whether the peer's last claim is still live at this instant. Computed
    /// on every read from `expires_at`, so a stored "online" row can never
    /// outlive the claim that produced it (a phone that dies sends no
    /// goodbye).
    pub online: bool,
    /// Send time (unix seconds) of the last beacon received from this peer.
    pub last_seen_at: u64,
    /// Whether they have marked us as their comrade (any beacon proves it).
    pub peer_marked_us: bool,
}

/// One device-local counter, as a diagnostics screen sees it.
///
/// Deliberately just a name and a number: the metrics layer has no way to
/// attach a peer, a message id, or a timestamp, so a snapshot cannot become a
/// record of who someone talked to and when. See
/// [`ComradeRuntime::metrics_snapshot`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct MetricDto {
    /// Stable dotted key, e.g. `outbox.queued`.
    pub key: String,
    pub value: u64,
}

/// Locally cached snapshot of a peer's published Kind-0 profile. `name` is a
/// self-declared, non-unique handle — a display aid, never an identifier.
/// Every field defaults so rows written by older builds keep deserialising
/// when the record grows (e.g. the planned avatar field).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct PeerProfileRecord {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    about: Option<String>,
    /// The `picture` URL exactly as the peer published it. Kept even when the
    /// bytes are never fetched, so a profile page can say "they have a picture we
    /// have not loaded" instead of implying they have none.
    #[serde(default)]
    picture: Option<String>,
    #[serde(default)]
    nip05: Option<String>,
    #[serde(default)]
    lud16: Option<String>,
    /// SHA-256 of the *vetted* avatar bytes in [`PEER_AVATAR_BLOBS_TREE`].
    /// `Some` is exactly "an avatar is cached", so a contact row costs the same
    /// single store read it already paid for `name`.
    #[serde(default)]
    avatar_sha256: Option<String>,
    /// The sniffed type of those bytes — never the one the host declared.
    #[serde(default)]
    avatar_mime: Option<String>,
    #[serde(default)]
    avatar_fetched_at: u64,
    /// When a fetch last failed or was refused, driving the negative TTL so a
    /// broken URL is not retried on every sweep.
    #[serde(default)]
    avatar_failed_at: u64,
    /// When this record was last written (unix seconds) — drives the TTL.
    #[serde(default)]
    updated_at: u64,
}

/// What a caller *learned* about a peer's profile — every field optional,
/// because almost no caller learns all of them at once.
///
/// This exists so [`merge_peer_profile`] can be the only writer. A caller that
/// means "I learned a name" must not thereby claim "and there is no bio", which
/// is exactly what a whole-record write does.
#[derive(Debug, Clone, Default)]
struct PeerProfilePatch {
    name: Option<String>,
    about: Option<String>,
    picture: Option<String>,
    nip05: Option<String>,
    lud16: Option<String>,
    avatar: Option<(String, String)>,
    avatar_failed: bool,
}

/// A private journal entry as the frontend sees it. The journal is **strictly
/// local**: nothing about it is synchronised, published or uploaded, and the
/// only copy of an entry lives sealed inside the encrypted store.
///
/// The one way words written here reach anybody is
/// [`ComradeRuntime::share_journal_entry`] — one entry, one peer, one tap, as
/// an ordinary DM. That sends a *copy*; the entry is untouched by it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct JournalEntryDto {
    pub id: String,
    /// What the user called this entry, when they named it. Most typed entries
    /// have no title and are drawn from their text alone.
    pub title: Option<String>,
    pub text: String,
    /// Optional self-reported mood marker (an emoji or short tag).
    pub mood: Option<String>,
    /// Present when this entry is a recording — spoken or filmed.
    pub recording: Option<JournalRecordingDto>,
    pub created_at: u64,
}

/// A journal recording — a voice entry or a video entry — as the frontend sees it.
///
/// The runtime never opens the file and never reads a frame or a sample: it
/// stores what the frontend told it about the recording and hands the same
/// description back. Everything about where the file lives — which directory,
/// whether the gallery can see it, when it is deleted — is the frontend's,
/// because it is a platform question and each frontend answers it differently.
///
/// [`mime`](Self::mime) is what tells a frontend which player to draw, and it is
/// the only thing that does — see [`comrade_storage::JournalRecording`] for why
/// there is no second `kind` field. Read that type before writing any copy about
/// this: the description here is sealed by the vault, the file is not (AUDIT J-1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct JournalRecordingDto {
    /// Name of the file inside the frontend's own directory for this kind —
    /// a bare name, never a path.
    pub file_name: String,
    /// The recording's MIME type — `video/mp4` or `audio/aac`. Also the only
    /// thing that says whether this is watched or listened to.
    pub mime: String,
    /// Length in milliseconds, or zero when the frontend could not read one.
    pub duration_ms: u64,
    /// Size on disk in bytes when the entry was saved.
    pub size_bytes: u64,
}

impl From<comrade_storage::JournalRecording> for JournalRecordingDto {
    fn from(v: comrade_storage::JournalRecording) -> Self {
        Self {
            file_name: v.file_name,
            mime: v.mime,
            duration_ms: v.duration_ms,
            size_bytes: v.size_bytes,
        }
    }
}

impl From<JournalRecordingDto> for comrade_storage::JournalRecording {
    fn from(v: JournalRecordingDto) -> Self {
        Self {
            file_name: v.file_name,
            mime: v.mime,
            duration_ms: v.duration_ms,
            size_bytes: v.size_bytes,
        }
    }
}

impl From<comrade_storage::JournalEntry> for JournalEntryDto {
    fn from(e: comrade_storage::JournalEntry) -> Self {
        Self {
            id: e.id,
            title: e.title,
            text: e.text,
            mood: e.mood,
            recording: e.recording.map(JournalRecordingDto::from),
            created_at: e.created_at,
        }
    }
}

/// One turn of the Tara reflective-companion thread as the frontend sees it.
/// Like the journal, the thread is **strictly local**: no relay, no network —
/// the only copy lives sealed inside the encrypted store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct TaraMessageDto {
    pub id: String,
    pub text: String,
    /// `true` for Tara's replies, `false` for the user's messages.
    pub from_tara: bool,
    /// Whether this turn tripped the distress detector — the frontend must
    /// surface the crisis resources alongside it (AUDIT §8 honesty gate).
    pub crisis: bool,
    pub created_at: u64,
}

/// What happened when Tara was asked something **in a conversation** —
/// `@tara …`, the shared spelling. See [`RuntimeHandles::tara_in_chat`].
///
/// Both messages come back so the composer can put them straight into the thread
/// it is already showing, in the order they were sent, without a reload that
/// would race the relay.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct TaraChatDto {
    /// The question as it now sits in the thread. `None` when nothing was sent.
    pub asked: Option<MessageDto>,
    /// Tara's answer as it sits in the thread. `None` when it stayed private.
    pub answered: Option<MessageDto>,
    /// Tara's words, always — the only field set when nothing was shared, and
    /// what a composer shows in its own note in that case.
    pub reply: String,
    /// **Nothing left this device.** True when the question tripped the distress
    /// detector: a helpline hand-off is not something to publish into somebody
    /// else's chat on the asker's behalf, whichever sigil they typed.
    pub kept_private: bool,
    /// Whether the reply carries the crisis hand-off, so the frontend shows the
    /// resources beside it exactly as the private thread does.
    pub crisis: bool,
}

impl From<comrade_storage::TaraMessage> for TaraMessageDto {
    fn from(m: comrade_storage::TaraMessage) -> Self {
        Self {
            id: m.id,
            text: m.text,
            from_tara: m.from_tara,
            crisis: m.crisis,
            created_at: m.created_at,
        }
    }
}

/// One day's usage rollup as the frontend sees it (wellbeing pillar #5).
/// **Strictly local**, exactly like [`JournalEntryDto`] — and only ever a
/// rollup: no app names, no per-app timings, no event stream. See
/// `docs/ATTENTION.md`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct AttentionDayDto {
    /// Local calendar date, `YYYY-MM-DD`.
    pub date: String,
    pub screen_minutes: u32,
    pub pickups: u32,
    /// Minutes in the apps the *user* tagged as their own scroll traps.
    pub doom_minutes: u32,
}

impl From<comrade_storage::AttentionDay> for AttentionDayDto {
    fn from(d: comrade_storage::AttentionDay) -> Self {
        Self {
            date: d.date,
            screen_minutes: d.screen_minutes,
            pickups: d.pickups,
            doom_minutes: d.doom_minutes,
        }
    }
}

/// Today's usage against the user's **own** recent medians — the only
/// comparison this app makes. Never a normative target, never another user
/// (`docs/ATTENTION.md` gate 1); `sample_days` lets the UI say how thin the
/// baseline is instead of calling one prior day "your usual".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct AttentionSummaryDto {
    /// Today's rollup, or `None` if nothing has been recorded today.
    pub today: Option<AttentionDayDto>,
    pub median_screen_minutes: u32,
    pub median_doom_minutes: u32,
    pub median_pickups: u32,
    /// How many prior days (0–7) the medians are drawn from.
    pub sample_days: u32,
}

/// One focus session as the frontend sees it. Strictly local.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct FocusSessionDto {
    pub id: String,
    /// What the user said they'd give the time to (may be empty).
    pub intent: String,
    pub planned_minutes: u32,
    pub started_at: u64,
    pub ended_at: Option<u64>,
    /// `completed` / `abandoned` / `lapsed`; `None` while still running.
    pub outcome: Option<String>,
    /// Seconds left against the plan — 0 for a finished session.
    pub remaining_secs: u64,
}

/// One saved read, opened for reading: the full text already split into
/// chapter-sized chunks (lossless by test —
/// `comrade_core::attention::chunk_reading`), plus where the reader had got to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct SavedReadDto {
    pub id: String,
    pub title: String,
    /// Where it came from — the host of the first link in the text, or empty
    /// for pasted prose. A label, never something to fetch.
    pub source: String,
    pub chunks: Vec<String>,
    /// Index into `chunks`, always in range (a stored position past the end
    /// after an edit is clamped rather than trusted).
    pub position: u32,
    pub added_at: u64,
}

/// A reading-library row: everything the list needs without hauling the whole
/// text across the FFI for every entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct SavedReadSummaryDto {
    pub id: String,
    pub title: String,
    /// See [`SavedReadDto::source`].
    pub source: String,
    pub chunk_count: u32,
    pub position: u32,
    pub added_at: u64,
}

/// One step of the guided stretch break
/// (`comrade_core::attention::STRETCH_ROUTINE`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct StretchStepDto {
    /// Stable key the frontends hang their animation on.
    pub key: String,
    pub name: String,
    pub cue: String,
    /// Seconds to stay with it — per side, when `mirrored`.
    pub seconds: u32,
    /// Done once per side (left, then right) when `true`.
    pub mirrored: bool,
}

/// A crisis helpline surfaced when a Tara message carries distress cues.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct CrisisResourceDto {
    pub name: String,
    pub contact: String,
    pub note: String,
}

/// Who wrote a message, where `outgoing` alone cannot say.
///
/// `outgoing` distinguishes the two people; this distinguishes a person from the
/// companion, so `@tara` can be drawn as a third participant rather than as a
/// line you appear to have typed. The two are orthogonal: an outgoing
/// [`MessageAuthor::Tara`] is her answer in a thread you started, and an
/// incoming one is her answer in a thread they started.
///
/// **Attribution, not attestation.** Nothing signs this. The wire carries
/// [`comrade_core::tara::TARA_CHAT_PREFIX`], which any client — or any person
/// with a keyboard — can put in front of a sentence, so a Tara bubble means
/// *the sending Comrade said this came from Tara*, in the same way a quoted
/// reply means the sender said they were quoting you. It must not be built on
/// as proof, and `AUDIT.md` Q17 records the boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, uniffi::Enum)]
#[serde(rename_all = "snake_case")]
pub enum MessageAuthor {
    /// One of the two people in the conversation — the ordinary case.
    Human,
    /// The companion, answering where both people can read it.
    Tara,
}

/// A single direct message in a conversation, from the offline history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct MessageDto {
    pub id: String,
    /// Peer npub the thread is keyed by (sender if incoming, recipient if outgoing).
    pub peer: String,
    /// What to draw in the bubble — already stripped of any author marker, so a
    /// frontend never renders the wire form. See [`split_author`].
    pub content: String,
    pub created_at: u64,
    pub outgoing: bool,
    /// Who wrote [`Self::content`]. See [`MessageAuthor`] for what this does and
    /// does not claim.
    pub author: MessageAuthor,
    /// Delivery status of an outgoing message: `sent` / `delivered` / `read`.
    /// `None` for incoming messages (no ticks shown on the receiver's side).
    pub status: Option<String>,
    /// Event id (hex) this message replies to, if any.
    pub reply_to: Option<String>,
    /// Set when this message is a journal note its sender chose to share — see
    /// [`SharedNoteDto`] and `comrade_core::note`. `None` for ordinary
    /// messages, which is nearly all of them.
    pub shared_note: Option<SharedNoteDto>,
    /// The link preview the *sender* built and attached, if the message
    /// carries one — see `comrade_core::unfurl`'s module doc for why this
    /// never costs the receiver a network request.
    pub link_preview: Option<LinkPreviewDto>,
    /// Whether this is a forward — see [`forwarded_text`] for why the label
    /// carries no claim about who wrote the original words.
    pub forwarded: bool,
    /// This device's local bookmark/pin state for the message. See
    /// [`MessageActionState`] for why neither field means anything to the peer.
    pub actions: MessageActionState,
}

/// A message's local bookmark/pin state, read from the encrypted store
/// alongside its content.
///
/// Both fields are local device state, same standing as
/// `comrade_storage::StarredMessage`/`PinnedMessage`: there is no protocol
/// message that announces either to the peer, so starring or pinning a
/// message on one device says nothing about any other device or the other
/// person's copy of the conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct MessageActionState {
    pub starred: bool,
    pub pinned: bool,
}

/// A link preview card, as a frontend renders it — the bridge view of
/// `comrade_core::unfurl::LinkPreview`. See that module's doc for the
/// load-bearing decision this rides on: built by the sender, rendered by the
/// receiver with zero network requests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct LinkPreviewDto {
    pub url: String,
    pub canonical_url: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub site_name: Option<String>,
    /// Deliberately absent. `comrade_core::unfurl::LinkPreview::image_url` is
    /// a URL on the linked host — carrying it across the FFI is one obvious
    /// `<img src={preview.image_url}>` away from a frontend having the
    /// receiver's device fetch it, which tells that host which npub read the
    /// link. Nothing crosses this boundary until there is a thumbnail the
    /// *sender* has already fetched and attached as bytes in the envelope;
    /// only then can this DTO carry an image without becoming the leak the
    /// zero-network-request design exists to prevent.
    pub kind: PreviewKindDto,
    /// The domain to draw on the card — from [`Self::url`], never
    /// [`Self::site_name`]. See `comrade_core::unfurl::display_domain`: a
    /// page's own metadata can claim to be named anything, but this cannot lie
    /// about which host actually served the page.
    pub display_domain: Option<String>,
}

/// What kind of thing a preview's link points at — the bridge mirror of
/// `comrade_core::unfurl::PreviewKind`. A hint the linked page volunteered
/// about itself, useful for picking a card layout, never treated as more.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, uniffi::Enum)]
#[serde(rename_all = "snake_case")]
pub enum PreviewKindDto {
    Article,
    Photo,
    Video,
    Profile,
    Unknown,
}

impl From<comrade_core::unfurl::PreviewKind> for PreviewKindDto {
    fn from(k: comrade_core::unfurl::PreviewKind) -> Self {
        use comrade_core::unfurl::PreviewKind as K;
        match k {
            K::Article => Self::Article,
            K::Photo => Self::Photo,
            K::Video => Self::Video,
            K::Profile => Self::Profile,
            K::Unknown => Self::Unknown,
        }
    }
}

impl From<PreviewKindDto> for comrade_core::unfurl::PreviewKind {
    fn from(k: PreviewKindDto) -> Self {
        use comrade_core::unfurl::PreviewKind as K;
        match k {
            PreviewKindDto::Article => K::Article,
            PreviewKindDto::Photo => K::Photo,
            PreviewKindDto::Video => K::Video,
            PreviewKindDto::Profile => K::Profile,
            PreviewKindDto::Unknown => K::Unknown,
        }
    }
}

impl From<comrade_core::unfurl::LinkPreview> for LinkPreviewDto {
    fn from(p: comrade_core::unfurl::LinkPreview) -> Self {
        let display_domain = comrade_core::unfurl::display_domain(&p.url);
        Self {
            url: p.url,
            canonical_url: p.canonical_url,
            title: p.title,
            description: p.description,
            site_name: p.site_name,
            kind: p.kind.into(),
            display_domain,
        }
    }
}

impl From<LinkPreviewDto> for comrade_core::unfurl::LinkPreview {
    fn from(dto: LinkPreviewDto) -> Self {
        comrade_core::unfurl::LinkPreview {
            url: dto.url,
            canonical_url: dto.canonical_url,
            title: dto.title,
            description: dto.description,
            site_name: dto.site_name,
            // Never carried across the bridge — see the field-removal note on
            // `LinkPreviewDto`. `None` here, not a lossy round trip: nothing
            // on this side ever had a URL to lose.
            image_url: None,
            kind: dto.kind.into(),
        }
    }
}

/// Fetch and build a link preview for the first link in `content`, if any.
///
/// Sender-side only: this is what [`ComradeRuntime::send_dm`]'s caller runs
/// *before* sending, on the device that is about to send the message — the
/// same fetch a browser would already have made had the sender opened the
/// link themselves. It must never be called on the receive path; see
/// `comrade_core::unfurl`'s module doc for why. `None` with no link, a failed
/// fetch, or (in a build without the `unfurl-http` feature) always — the
/// caller falls back to sending plain text, never blocking a send on a
/// preview.
///
/// A free function, not a [`ComradeRuntime`] method, for the same reason
/// [`catalogue_lookup`] is: it touches no engine state, so nothing needs to
/// hold a lock across the fetch.
pub async fn compose_link_preview(content: &str) -> Option<LinkPreviewDto> {
    let url = comrade_core::unfurl::first_previewable_url(content)?;
    comrade_core::unfurl::fetch_preview(&url)
        .await
        .ok()
        .map(LinkPreviewDto::from)
}

/// Attach `preview` to `content`, producing the body [`ComradeRuntime::send_dm`]
/// should actually send. See `comrade_core::unfurl::attach_preview` for the
/// wire form — plain text with one suffix line, so a client that never learns
/// to parse it still shows exactly what was typed.
pub fn attach_link_preview(content: &str, preview: &LinkPreviewDto) -> String {
    comrade_core::unfurl::attach_preview(content, &preview.clone().into())
}

/// A journal note one person handed to another, as the bubble draws it.
///
/// The presence of this is what tells a frontend to draw a note card instead of
/// a plain bubble; [`MessageDto::content`] already carries the same text with
/// the marker line off it, so a frontend that ignores this field still shows
/// the words rather than the wire form.
///
/// **A label, not an attestation** — the same standing as [`MessageAuthor`],
/// and for the same reason: the marker is text anyone can type. The card says
/// *the sender says this came out of their journal*. Nothing may be gated on
/// it, and no frontend may word it as a guarantee.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct SharedNoteDto {
    /// The note as it was written. Kept even though it equals
    /// [`MessageDto::content`], so a card can be built from this alone.
    pub text: String,
    /// The self-reported mood marker the entry carried, if it had one.
    pub mood: Option<String>,
}

/// Split a stored/wire message body into who wrote it and what to draw.
///
/// The marker stays on the wire and on disk rather than being stripped before
/// storage, and that is deliberate on two counts. A NIP-17 DM read in some other
/// Nostr client still says "Tara: …" instead of putting her words in the
/// sender's mouth — the fallback rendering is the honest one. And the count in
/// [`ComradeRuntime::tara_in_chat`] reads the stored rows directly, so the
/// history keeps meaning the same thing after this function changes.
///
/// One place, called from [`read_body`], which is called from both
/// [`MessageDto`] construction sites — so a message cannot read one way when it
/// is sent and another way after a reload.
fn split_author(content: String) -> (MessageAuthor, String) {
    match comrade_core::tara::tara_chat_answer(&content) {
        Some(answer) => (MessageAuthor::Tara, answer.to_string()),
        None => (MessageAuthor::Human, content),
    }
}

/// What a forwarded message starts with — a label, never an attestation, in
/// exactly the sense [`MessageAuthor`] already is. Forwarding must not claim
/// the original author cryptographically: the forwarder is the one signing
/// and sending *this* copy, over their own conversation with the recipient,
/// so the wire can only ever say "the forwarding Comrade says this came from
/// somewhere else" — never who. Mirrors `comrade_core::tara::TARA_CHAT_PREFIX`
/// and `comrade_core::note::JOURNAL_NOTE_PREFIX`'s wire-stays-text argument: a
/// client with no idea about forwarding still shows the label as plain text
/// instead of silently presenting the words as the forwarder's own.
const FORWARDED_PREFIX: &str = "↪ Forwarded";

/// Render `text` as a forwarded message body. `text` must already be the
/// plain words to show — see [`RuntimeHandles::forward_message`], which
/// strips any author/note/preview marker from the original before calling
/// this, so a forward is never able to nest another claim inside its own.
fn forwarded_line(text: &str) -> String {
    format!("{FORWARDED_PREFIX}\n{}", text.trim())
}

/// The text of a forwarded message, if `content` is one.
fn forwarded_text(content: &str) -> Option<&str> {
    let rest = content.strip_prefix(FORWARDED_PREFIX)?;
    let text = rest.strip_prefix('\n')?.trim();
    (!text.is_empty()).then_some(text)
}

/// Everything a stored/wire body says about itself, read in one place so a
/// message cannot parse one way when it is sent and another way after a
/// reload — the same argument [`read_body`]'s predecessor made for
/// [`split_author`] and the shared-note marker, extended to cover forwarding
/// and link previews.
struct BodyRead {
    author: MessageAuthor,
    shared_note: Option<SharedNoteDto>,
    link_preview: Option<LinkPreviewDto>,
    forwarded: bool,
    text: String,
}

/// Split a stored/wire message body into who wrote it, what it carries, and
/// the text a bubble draws. See [`BodyRead`].
///
/// The markers are mutually exclusive, checked in this order: Tara's answer
/// first (she has no journal and forwards nothing — a body carrying both
/// markers is someone typing, and the outer claim is the one that stands);
/// then a forward, whose contents [`RuntimeHandles::forward_message`] has
/// already stripped down to plain text, so nothing further to split inside
/// one; then a shared journal note; and only then a link preview, which is
/// the one marker an ordinary human message can carry on its own.
fn read_body(content: String) -> BodyRead {
    let (author, body) = split_author(content);
    if author != MessageAuthor::Human {
        return BodyRead {
            author,
            shared_note: None,
            link_preview: None,
            forwarded: false,
            text: body,
        };
    }
    if let Some(text) = forwarded_text(&body) {
        return BodyRead {
            author,
            shared_note: None,
            link_preview: None,
            forwarded: true,
            text: text.to_string(),
        };
    }
    if let Some(note) = shared_note_of(&body) {
        let text = note.text.clone();
        return BodyRead {
            author,
            shared_note: Some(note),
            link_preview: None,
            forwarded: false,
            text,
        };
    }
    let (text, preview) = comrade_core::unfurl::split_preview(&body);
    BodyRead {
        author,
        shared_note: None,
        link_preview: preview.map(LinkPreviewDto::from),
        forwarded: false,
        text,
    }
}

/// The journal note `content` carries, if it is one. One reader for both DTOs.
fn shared_note_of(content: &str) -> Option<SharedNoteDto> {
    comrade_core::note::shared_journal_note(content).map(|note| SharedNoteDto {
        text: note.text.to_string(),
        mood: note.mood.map(str::to_string),
    })
}

/// Build a [`MessageDto`] from a stored row plus the local action state a
/// caller already looked up — one place, so `messages_with`, `pinned_messages`
/// and `starred_messages` cannot read a message's content differently from
/// one another.
fn stored_message_dto(
    m: comrade_storage::StoredMessage,
    actions: MessageActionState,
) -> MessageDto {
    let read = read_body(m.content);
    MessageDto {
        id: m.id,
        peer: m.peer_npub,
        content: read.text,
        created_at: m.created_at,
        author: read.author,
        status: if m.outgoing {
            Some(m.status.unwrap_or_else(|| "sent".into()))
        } else {
            None
        },
        reply_to: m.reply_to,
        outgoing: m.outgoing,
        shared_note: read.shared_note,
        link_preview: read.link_preview,
        forwarded: read.forwarded,
        actions,
    }
}

/// Every starred message id in `peer`'s conversation, for an O(1) join
/// against a list of stored rows.
fn starred_ids(
    store: &comrade_storage::EncryptedStore,
    peer_npub: &str,
) -> Result<std::collections::HashSet<String>, UiError> {
    Ok(store
        .starred_with(peer_npub)
        .map_err(|e| UiError::Storage(e.to_string()))?
        .into_iter()
        .map(|s| s.message_id)
        .collect())
}

/// Every pinned message id in `peer`'s conversation, for the same join
/// [`starred_ids`] is for.
fn pinned_ids(
    store: &comrade_storage::EncryptedStore,
    peer_npub: &str,
) -> Result<std::collections::HashSet<String>, UiError> {
    Ok(store
        .pinned_with(peer_npub)
        .map_err(|e| UiError::Storage(e.to_string()))?
        .into_iter()
        .map(|p| p.message_id)
        .collect())
}

// ── Threads and topics (see `comrade_core::topic`) ───────────────────────────

/// One topic in one conversation, with the counts a list row needs.
///
/// The counts are computed on read rather than stored, and that is deliberate:
/// a stored count is a second source of truth that drifts the first time a
/// backfill inserts an old message, and this history is bounded by one
/// conversation. See [`ThreadIndex`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct TopicDto {
    /// Canonical slug — the id. See `comrade_core::topic::slugify`.
    pub slug: String,
    /// Display name, as first spelled.
    pub name: String,
    /// Peer npub the conversation is keyed by.
    pub peer: String,
    /// npub of whoever first named it.
    pub created_by: String,
    /// Whether *this device* named it — so a frontend can say "you started
    /// this" without holding the local npub.
    pub mine: bool,
    pub created_at: u64,
    /// Archived: readable, out of the picker. Nothing deletes a topic.
    pub closed: bool,
    /// Threads filed here.
    pub thread_count: u32,
    /// Messages across those threads.
    pub message_count: u32,
    /// `created_at` of the newest message in any of its threads, or the
    /// topic's own creation time when it holds nothing yet — so a list can sort
    /// by liveliness without a second query.
    pub last_activity_at: u64,
}

/// One thread as a list row: what it is about, how big it got, and where it is
/// filed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct ThreadSummaryDto {
    /// Event id of the root message — the thread's id, and what
    /// [`ComradeRuntime::thread`] takes.
    pub root_id: String,
    pub peer: String,
    /// Slug of the topic it is filed under, or `None` for unfiled.
    pub topic_slug: Option<String>,
    /// The root's own text, already stripped of any author marker. Empty when
    /// the root is an uncaptioned attachment, or when the root is not in the
    /// loaded history at all — see [`Self::root_is_media`] and
    /// [`Self::root_missing`], which say which.
    pub preview: String,
    /// Whether the root is an attachment rather than a text message, so a
    /// frontend can render its own "📎 Photo" in its own language rather than
    /// having English pushed up from here — the same split
    /// `comrade_core::command`'s offer envelopes make.
    pub root_is_media: bool,
    /// Whether the root message is missing from this device's history.
    ///
    /// Not an error and not rare: a thread can be filed before its root is
    /// cached (a reply arrives first, the peer files it, the root turns up on
    /// the next backfill), and a reply can quote something older than the
    /// window. The row still renders — its replies are real — and the frontend
    /// says the root is not here rather than showing a blank quote.
    pub root_missing: bool,
    /// `created_at` of the root, or of the oldest message actually held when
    /// the root is missing.
    pub started_at: u64,
    /// Messages *below* the root. A thread of one is a message nobody replied
    /// to, and both shipping frontends hide those from the thread list.
    pub reply_count: u32,
    /// `created_at` of the newest message in the thread.
    pub last_at: u64,
    /// Whether anything in this thread came from the peer after the
    /// conversation's read watermark — the dot on the row. Uses the same
    /// watermark as the main thread's unread divider
    /// (`ConversationMeta::last_read_at`), because a sheet with its own idea of
    /// "unread" would disagree with the screen it opens from.
    pub unread: bool,
}

/// One thread, in full, for the sheet that opens over the conversation.
///
/// Two lists rather than one merged one, because a merge needs a total order
/// and the frontends already have theirs: `ChatsScreen`'s `ChatItem` and the
/// desktop's `msgs` array both interleave text and media by `created_at`
/// today. Handing up a third ordering would be a fourth answer to a question
/// three surfaces already answer identically.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, uniffi::Record)]
pub struct ThreadDto {
    pub root_id: String,
    pub peer: String,
    pub topic_slug: Option<String>,
    /// Every text message in the thread, oldest first, including the root when
    /// the root is a text message.
    pub messages: Vec<MessageDto>,
    /// Every attachment in the thread, oldest first. In practice this is the
    /// root or nothing: an attachment carries no `reply_to`, so it can start a
    /// thread but not join one. That is a real limitation rather than a design
    /// choice — recorded as `AUDIT.md` TOPIC-2.
    pub media: Vec<MediaMessageDto>,
}

/// Live connectivity status of the off-grid Saathi mesh (mDNS discovery +
/// Gossipsub), for a persistent UI indicator — the one signal that still works
/// with zero cellular or relay reachability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct MeshStatusDto {
    /// Whether the mesh engine is running at all — which, since local delivery
    /// landed, is whenever the vault is unlocked, not only in `OffGridTravel`
    /// (see [`ComradeRuntime::sync_saathi_lifecycle`]).
    pub active: bool,
    /// Peers currently reachable over the local network via mDNS. `u64`, not
    /// `SaathiEngine::peer_count`'s native `usize` — uniffi has no FFI-safe
    /// representation for a platform-width integer.
    pub peer_count: u64,
}

/// A push event emitted by the background Tokio loops and forwarded across the
/// webview boundary (`window.emit`) or delivered to Android through a uniffi
/// callback interface (see `comrade_jni::BridgeEventListener`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, uniffi::Enum)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BridgeEvent {
    /// A new public Chitthi (Kind-1) arrived on the Sabha timeline.
    IncomingChitthi(ChitthiDto),
    /// A new encrypted DM (Kind-4) was decrypted in the Vault inbox — from an
    /// already-accepted conversation.
    IncomingDirectMessage(DirectMessageDto),
    /// A new encrypted-media reference (NIP-94) arrived over the DM channel.
    IncomingMedia(MediaMessageDto),
    /// A peer reacted to a message, changed their reaction, or took it back — an
    /// empty [`ReactionDto::emoji`] is the withdrawal. Emitted only when the
    /// visible state actually changed, so a replay off the two-day backfill does
    /// not redraw anything.
    IncomingReaction(ReactionDto),
    /// A call-signaling payload (offer/answer/ICE/hangup) arrived for the
    /// frontend's WebRTC layer.
    IncomingCallSignal(CallSignalDto),
    /// A stranger (not yet accepted) sent a DM — surfaced as a message request,
    /// not a chat. Accepting it moves the conversation into the chat list.
    IncomingMessageRequest(MessageRequestDto),
    /// A delivered/read receipt advanced the status of one or more of our
    /// outgoing messages in `peer`'s thread.
    MessageStatus {
        peer: String,
        message_ids: Vec<String>,
        status: String,
    },
    /// A peer named a topic, filed a thread under one, or archived one — the
    /// structure of `peer`'s conversation moved. See `comrade_core::topic`.
    ///
    /// **Carries the conversation and nothing else.** One coarse event rather
    /// than a payload per signal kind, for two reasons. The sheet re-reads the
    /// whole conversation's topics anyway (the counts are derived, not stored —
    /// see [`TopicDto`]), so a fine-grained payload would be read and thrown
    /// away. And this variant costs a Kotlin `when` arm and a Dart `switch` arm
    /// in files nobody edits, which is the tax
    /// [`Self::TogetherShare`] documents; paying it once for "topics changed"
    /// is the version of that tax worth paying.
    ///
    /// Emitted only when the visible state actually moved, so a replay off the
    /// two-day gift-wrap backfill does not redraw anything — the same rule
    /// [`Self::IncomingReaction`] follows, enforced by
    /// `EncryptedStore::set_thread_topic`'s return value.
    TopicsChanged { peer: String },
    /// A peer shared (or updated) their display handle — e.g. by accepting our
    /// message request. The chat list should re-title their conversation.
    PeerProfileUpdated { peer: String, name: Option<String> },
    /// A comrade came online (or went offline / aged out). Emitted only for
    /// peers the user marked as comrades, and only on an actual transition —
    /// a heartbeat from someone already known to be online is not news. The
    /// `online == true` edge is what a frontend turns into "they're around".
    ComradePresence {
        peer: String,
        /// Display name at the time of the event (alias → published handle),
        /// so a notification can be raised without a store round-trip.
        name: Option<String>,
        online: bool,
        /// Send time of the beacon (unix seconds) for the `online` edge; the
        /// moment the claim lapsed for an aged-out `offline` edge.
        at: u64,
    },
    /// A comrade wrote something for us and did not send it — see
    /// [`comrade_core::nudge`]. Emitted once per hesitation, only for a peer
    /// the user marked as a comrade, and only while the nudge is still fresh:
    /// what a frontend turns into "they're around, and they might need you".
    ///
    /// Carries no more than [`Self::ComradePresence`] does, because the wire
    /// envelope carries no more than that either — never the draft, its
    /// length, or how the writing ended.
    ComradeNudge {
        peer: String,
        /// Display name at the time of the event (alias → published handle),
        /// so a notification can be raised without a store round-trip.
        name: Option<String>,
    },
    /// The other seat of the motorcycle said one of the few things worth
    /// saying at speed — see [`comrade_core::ride`]. Emitted only from an
    /// accepted conversation and only while the signal is still fresh: a
    /// backfilled "left in 400 m" from Tuesday raises nothing.
    ///
    /// One variant for the whole vocabulary rather than one per phrase — the
    /// tax [`Self::TogetherShare`] warns about, avoided the same way.
    RideSignal(RideSignalDto),
    /// Someone asked to watch or listen to something together.
    TogetherInvited(TogetherInviteDto),
    /// They accepted our invitation, so the session is live on both sides.
    TogetherJoined { session_id: String, peer: String },
    /// They played, paused or seeked.
    TogetherCommand(TogetherCommandDto),
    /// Our playhead has drifted from theirs by more than the measurement error
    /// is worth, and here is what to do about it.
    TogetherCorrection(TogetherCorrectionDto),
    /// The session is over. `by_peer` distinguishes "they left" from "we stopped
    /// hearing from them" — which is something *we* observed, never something
    /// the wire told us, because a departing peer sends no reason.
    TogetherEnded {
        session_id: String,
        peer: String,
        by_peer: bool,
    },
    /// One step of handing the file over, because only one of you has it.
    ///
    /// One event for the whole exchange rather than five, so adding a step to
    /// the transfer protocol is a change in `comrade_core::share` and nowhere
    /// else. The alternative — a bridge event per step — would put a Kotlin
    /// `when` arm, a Dart `switch` arm and a regenerated bridge behind every
    /// protocol tweak, which is exactly the tax that keeps protocols from being
    /// tweaked.
    TogetherShare(TogetherShareDto),
    /// Push this envelope down the direct peer channel, now.
    ///
    /// The one place core asks a frontend to *carry* something rather than to
    /// display it, and the reason is the same one that put WebRTC in the
    /// frontend to begin with: core owns the protocol, the frontend owns the
    /// connection. A relay publish happens inside `send_together`; a direct
    /// send cannot, because the socket is not core's to hold.
    ///
    /// One variant for the whole transport rather than one per signal kind —
    /// the tax [`Self::TogetherShare`] warns about is a bridge event *per
    /// protocol step*, and this is a single event for a single capability.
    ///
    /// A frontend that cannot send it should simply drop it: the session's own
    /// TTL and the peer's heartbeats are what notice a dead channel, and
    /// [`RuntimeHandles::together_direct_ready`] is how it says so.
    TogetherOutbound { session_id: String, json: String },
    /// One step of a large-attachment handoff from `peer`.
    AttachmentHandoff(AttachmentHandoffDto),
    /// The off-grid mesh's connectivity changed: it started/stopped, or a peer
    /// joined/left via mDNS. Drives the persistent local-mesh status indicator.
    MeshStatusChanged(MeshStatusDto),
    /// The Sakha/Sakhi shared ledger changed — a partner's entry merged in
    /// over the sync channel. Carries the fresh, fully-merged ledger text.
    LedgerUpdated { ledger: String },
}

// ── Runtime ───────────────────────────────────────────────────────────────────

/// The live IPC runtime context. Wrap in `Arc<RwLock<ComradeRuntime>>` and hand
/// to Tauri's managed state / the JNI global so command handlers can reach the
/// core systems thread-safely.
pub struct ComradeRuntime {
    ui: UiService,
    sabha: Option<Arc<SabhaEngine>>,
    vault: Option<Arc<VaultEngine>>,
    sakha: Option<Arc<SakhaEngine>>,
    /// The off-grid mesh engine — running iff the active workspace is
    /// `OffGridTravel` (see [`Self::sync_saathi_lifecycle`]). Unlike the Nostr
    /// engines above, it is started and stopped on the fly rather than built
    /// once at unlock, since mDNS/Gossipsub only make sense while off-grid.
    saathi: Option<Arc<SaathiEngine>>,
    /// Woken when a transport that was down comes up, so queued mail goes out
    /// then instead of waiting up to [`OUTBOX_FLUSH_INTERVAL_SECS`] for the
    /// next tick. "The other phone just joined the WiFi" is precisely the
    /// moment a queued message becomes sendable, and a minute of clock icon
    /// after that moment reads as the feature not working.
    transport_wake: Arc<tokio::sync::Notify>,
    /// The Bluetooth transport's policy half — always present, but inert until
    /// a platform BLE service marks it active. Constructed unconditionally so
    /// the FFI has something to hand packets to whether or not this build has
    /// a radio behind it.
    ble: Arc<BleRouter>,
    events: broadcast::Sender<BridgeEvent>,
    /// The separate, small-capacity, deliberately-lossy bus for
    /// `IncomingChitthi` only — see [`FEED_EVENT_BUS_CAPACITY`] and
    /// [`Self::subscribe_feed_events`].
    feed_events: broadcast::Sender<BridgeEvent>,
    /// Guards [`spawn_event_loops`] against re-spawning the feed/DM tasks if it
    /// is called more than once. [`spawn_event_loops`]: ComradeRuntime::spawn_event_loops
    loops_spawned: bool,
    /// Guards [`spawn_sakha_sync_loop`] the same way `loops_spawned` guards
    /// the feed/DM loops. [`spawn_sakha_sync_loop`]: ComradeRuntime::spawn_sakha_sync_loop
    sakha_sync_spawned: bool,
    /// The one live profile-publish retry task. Replaced (old one aborted)
    /// whenever the handle changes, so a stale retry loop can never republish
    /// an old name over a new one.
    publish_task: Option<tokio::task::JoinHandle<()>>,
    /// The Sabha feed-subscription task spawned by [`Self::spawn_event_loops`].
    /// Tracked (unlike a bare `tokio::spawn`) so [`Self::lock_vault`] can abort
    /// it — dropping the `Arc<SabhaEngine>` alone would not stop it, since the
    /// task itself holds its own clone.
    feed_task: Option<tokio::task::JoinHandle<()>>,
    /// The Vault inbox-subscription task, tracked for the same reason as
    /// [`Self::feed_task`].
    vault_task: Option<tokio::task::JoinHandle<()>>,
    /// The Sakha sync-subscription task, tracked for the same reason as
    /// [`Self::feed_task`].
    sakha_sync_task: Option<tokio::task::JoinHandle<()>>,
    /// The relay set new engines are built against — [`DEFAULT_RELAYS`] in
    /// production ([`Self::new`]), or an explicit override ([`Self::with_relays`])
    /// for an isolated test environment (AUDIT.md COMMS-03).
    relays: Vec<String>,
    /// Bounded LRU of call-envelope wrapper event ids already dispatched onto
    /// the event bus this session — see [`dispatch_incoming_dm`]'s call-signal
    /// branch. Behind an `Arc` so the vault callback (which outlives `&self`
    /// borrows) can hold its own clone.
    call_signal_dedup: Arc<SeenSet>,
    /// Sender outbox: DMs a relay would not accept, retried by the flush loop
    /// until a receipt clears them or the attempt cap drops them. Restored from
    /// the encrypted store on unlock, so a message survives an app kill.
    outbox: Arc<Outbox>,
    /// The periodic outbox-flush task, tracked like [`Self::feed_task`] so
    /// [`Self::lock_vault`] can abort it.
    outbox_task: Option<tokio::task::JoinHandle<()>>,
    /// Whether this device currently counts as *online* for presence.
    ///
    /// "Online" means **the app is open**, not merely that the process is
    /// alive: a phone in someone's pocket with a connection service running
    /// is reachable, but its owner is not at it, and a green dot that says
    /// otherwise is the kind of small lie this feature exists to avoid.
    /// Frontends set it through [`Self::announce_presence`] (Android: on
    /// foreground/background); the heartbeat refreshes the claim only while
    /// it holds. Behind an `Arc` so the heartbeat task and the
    /// [`RuntimeHandles`] every bridge takes share one answer.
    presence_active: Arc<std::sync::atomic::AtomicBool>,
    /// The comrade-presence heartbeat/expiry loop, tracked for the same
    /// reason as [`Self::feed_task`] — [`Self::lock_vault`] must be able to
    /// stop announcing "I'm online" the moment the user locks up.
    presence_task: Option<tokio::task::JoinHandle<()>>,
    /// Recently ingested (peer, content, transport) triples, so one message
    /// delivered by two routes renders once — see
    /// [`CROSS_TRANSPORT_DEDUP_SECS`].
    transport_dedup: Arc<SeenSet>,
    /// The task that opens sealed frames seen on the local mesh and feeds the
    /// ones addressed to us through the normal DM ingress.
    mesh_dm_task: Option<tokio::task::JoinHandle<()>>,
    /// The same, for frames rebuilt from Bluetooth fragments. A separate task
    /// because the two radios have independent streams, feeding one ingress.
    ble_dm_task: Option<tokio::task::JoinHandle<()>>,
    /// Which composers hold unsent text, for the "your comrade might need you"
    /// nudge ([`comrade_core::nudge`]). Behind an `Arc` so the presence sweep —
    /// which runs on a detached [`RuntimeHandles`], not on `&self` — watches
    /// the same map the frontends report into.
    nudge_watch: Arc<NudgeWatch>,
    /// The one live watch/listen-together session, or none.
    ///
    /// Deliberately **not** persisted, and deliberately at most one: a playhead
    /// is a claim about right now, and a session that outlived the process would
    /// reopen the replay hole that "after a relaunch there is no session, so
    /// every backfilled command is dropped" closes. One at a time also keeps the
    /// command arbitration a two-party problem, which is what makes it provable.
    together: Arc<Mutex<Option<TogetherSession>>>,
    /// Invitation ids already seen — see [`TOGETHER_START_DEDUP_CAPACITY`].
    together_starts_seen: Arc<SeenSet>,
    /// Wrapper event ids of transfer signals already forwarded — see
    /// [`TOGETHER_SHARE_DEDUP_CAPACITY`].
    together_shares_seen: Arc<SeenSet>,
    /// What this device is willing to do when the only path a file transfer can
    /// take is somebody else's relay.
    ///
    /// It lives here rather than in each frontend for the reason the whole
    /// transport module exists: the policy has to be changeable without touching
    /// the code that moves bytes, and three UIs each holding their own copy is
    /// three chances to enforce a different one. Session-scoped in v1 —
    /// [`RelayPolicy::DirectOnly`] on every start, and a device that has never
    /// been told otherwise relays nothing, which is the safe direction to
    /// default in.
    share_policy: Arc<Mutex<RelayPolicy>>,
    /// Travel guides already fetched this session, keyed by coarse cell.
    ///
    /// **In memory only, deliberately.** The guide's *contents* are public
    /// facts about public places, but the set of cells it is keyed by is a
    /// record of where this person has stood — which is the one thing in this
    /// feature worth protecting, and the reason it dies with the process
    /// instead of landing in the vault beside the journal. [`Self::lock_vault`]
    /// clears it early for the same reason.
    ///
    /// Behind an `Arc` so [`Self::travel_cache`] can hand a caller a handle to
    /// clone out from under a short read lock — the network fetch in
    /// [`travel_guide`] then runs with no runtime lock held at all.
    travel: TravelCache,
}

impl Default for ComradeRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl ComradeRuntime {
    pub fn new() -> Self {
        Self::with_relays(DEFAULT_RELAYS.iter().map(|r| r.to_string()).collect())
    }

    /// Build a runtime that connects new engines to `relays` instead of
    /// [`DEFAULT_RELAYS`] — for an isolated test relay (no public-internet
    /// dependency) or a future "selected relays" setting. Production call
    /// sites (`comrade_jni`, the Tauri desktop shell) always use [`Self::new`];
    /// this constructor exists for tests and is not itself exposed over FFI.
    pub fn with_relays(relays: Vec<String>) -> Self {
        let (events, _) = broadcast::channel(EVENT_BUS_CAPACITY);
        let (feed_events, _) = broadcast::channel(FEED_EVENT_BUS_CAPACITY);
        Self {
            ui: UiService::new(),
            sabha: None,
            vault: None,
            sakha: None,
            saathi: None,
            transport_wake: Arc::new(tokio::sync::Notify::new()),
            ble: Arc::new(BleRouter::new()),
            events,
            feed_events,
            loops_spawned: false,
            sakha_sync_spawned: false,
            publish_task: None,
            feed_task: None,
            vault_task: None,
            sakha_sync_task: None,
            relays,
            call_signal_dedup: Arc::new(SeenSet::new(CALL_SIGNAL_DEDUP_CAPACITY)),
            outbox: Arc::new(Outbox::new()),
            outbox_task: None,
            // Default `true`: a frontend that never says otherwise (desktop,
            // CLI) is one whose app is simply open. Android overrides it on
            // every foreground/background transition.
            presence_active: Arc::new(std::sync::atomic::AtomicBool::new(true)),
            presence_task: None,
            transport_dedup: Arc::new(SeenSet::with_ttl(
                CROSS_TRANSPORT_DEDUP_CAPACITY,
                std::time::Duration::from_secs(CROSS_TRANSPORT_DEDUP_SECS),
            )),
            mesh_dm_task: None,
            ble_dm_task: None,
            nudge_watch: Arc::new(NudgeWatch::new()),
            together: Arc::new(Mutex::new(None)),
            together_starts_seen: Arc::new(SeenSet::new(TOGETHER_START_DEDUP_CAPACITY)),
            together_shares_seen: Arc::new(SeenSet::new(TOGETHER_SHARE_DEDUP_CAPACITY)),
            share_policy: Arc::new(Mutex::new(RelayPolicy::default())),
            travel: TravelCache::default(),
        }
    }

    /// Abort any in-flight profile-publish retry loop and start one for
    /// `name`. Last spawn wins — the relays only ever see the newest handle.
    fn spawn_profile_publish(&mut self, sabha: Arc<SabhaEngine>, name: String) {
        // Leave: a launch republish and a handle change have no opinion about the
        // bio, and must not overwrite one the user set from another Nostr client.
        // This is the property `merged_metadata_preserves_foreign_profile_fields`
        // has always pinned, kept intact now that clearing is expressible.
        self.spawn_profile_publish_with(sabha, name, OwnedMetadataEdit::Leave);
    }

    /// As [`Self::spawn_profile_publish`], but saying what to do with the bio.
    fn spawn_profile_publish_with(
        &mut self,
        sabha: Arc<SabhaEngine>,
        name: String,
        about: OwnedMetadataEdit,
    ) {
        if let Some(task) = self.publish_task.take() {
            task.abort();
        }
        self.publish_task = Some(tokio::spawn(publish_profile_with_retry(sabha, name, about)));
    }

    // ── Event bus ──────────────────────────────────────────────────────────

    /// Subscribe to the *critical* push-event stream (everything except
    /// `IncomingChitthi` — see [`Self::subscribe_feed_events`]). The desktop
    /// layer forwards each event to the webview; the JNI layer forwards it to
    /// the registered [`comrade_jni::BridgeEventListener`].
    pub fn subscribe_events(&self) -> broadcast::Receiver<BridgeEvent> {
        self.events.subscribe()
    }

    /// Subscribe to the separate, small-capacity `IncomingChitthi` stream
    /// (AUDIT.md COMMS-04). A public-feed flood drops old, unconsumed
    /// Chitthis from *this* channel under load — it can never crowd out or
    /// delay a call signal, DM, message request, or terminal call event
    /// waiting on [`Self::subscribe_events`], because those live on a wholly
    /// separate channel. A host forwards both streams to the same listener
    /// (two loops feeding one callback, e.g. `comrade_jni::Comrade::set_event_listener`)
    /// — the split is an internal backpressure boundary, not a second API
    /// a caller needs to reason about differently.
    pub fn subscribe_feed_events(&self) -> broadcast::Receiver<BridgeEvent> {
        self.feed_events.subscribe()
    }

    /// A clonable handle to the event bus, for hosts that want to inject events
    /// (e.g. forwarding mesh/Saathi traffic) onto the same stream.
    pub fn event_sender(&self) -> broadcast::Sender<BridgeEvent> {
        self.events.clone()
    }

    // ── Milestone 1 + 4: unlock the vault & start the engines ────────────────

    /// Open the encrypted storage repository with `passphrase`, restore (or
    /// seed) the identity, and construct the core Nostr engines.
    ///
    /// Engine construction is offline — relays are registered but not dialled —
    /// so this never blocks on the network. Call [`spawn_event_loops`] afterward
    /// to connect and begin streaming.
    ///
    /// The Argon2id key stretch + sled open run on Tokio's blocking pool, so a
    /// deliberately slow KDF never stalls a reactor thread that other tasks
    /// (relay loops, other IPC commands) are scheduled on.
    ///
    /// [`spawn_event_loops`]: ComradeRuntime::spawn_event_loops
    pub async fn unlock_vault(
        &mut self,
        path: impl AsRef<std::path::Path>,
        passphrase: &str,
    ) -> Result<IdentityDto, UiError> {
        // Idempotent: a second unlock (both bridges call this at startup) must
        // not rebuild the engines — that would orphan the running ones and,
        // with spawn_event_loops, duplicate the relay connections and event
        // loops. Return the already-loaded identity instead.
        if self.is_vault_unlocked() {
            return self.ui.current_identity().ok_or(UiError::NoIdentity);
        }

        let started = std::time::Instant::now();
        let store = {
            let path = path.as_ref().to_path_buf();
            let passphrase = passphrase.to_string();
            tokio::task::spawn_blocking(move || {
                comrade_storage::EncryptedStore::open(path, &passphrase)
            })
            .await
            .map_err(|e| UiError::Storage(format!("unlock task failed: {e}")))?
            .map_err(|e| UiError::Storage(e.to_string()))?
        };
        let kdf_ms = started.elapsed().as_millis() as u64;
        self.ui.attach_store(store);

        // Mail queued before the last kill goes back into the live outbox, so
        // the flush loop picks it up on this launch.
        if let Some(store) = self.ui.store_ref() {
            if let Ok(Some(snapshot)) = store.get::<OutboxSnapshot>(OUTBOX_TREE, OUTBOX_KEY) {
                let restored = snapshot.queues.values().map(Vec::len).sum::<usize>();
                if restored > 0 {
                    tracing::info!(restored, "restored queued messages from the outbox");
                }
                self.outbox = Arc::new(Outbox::from_snapshot(snapshot));
            }
        }

        // The relay policy is a stored preference, so seed the in-memory cell
        // the WebRTC callbacks read. It is deliberately *only* seeded here: a
        // locked vault has no preference to read, and the cell's default is the
        // refusing one, so a failure to load can only ever be conservative.
        if let Some(store) = self.ui.store_ref() {
            if let Ok(prefs) = store.load_share_prefs() {
                *self.share_policy.lock().unwrap() = relay_policy_from_prefs(&prefs);
            }
        }

        // Unlocking is someone standing at the app with a passphrase typed, so
        // presence starts active again — a previous `lock_vault` left it off
        // (see `spawn_farewell_beacons`), and this runtime outlives the lock.
        self.presence_active
            .store(true, std::sync::atomic::Ordering::Relaxed);

        // Restore the saved identity, or seed and persist a fresh one so the
        // engines always have keys to sign with.
        let identity = match self.ui.load_identity()? {
            Some(id) => id,
            None => {
                let id = self.ui.generate_identity()?;
                self.ui.save_identity()?;
                id
            }
        };

        let keys = self.ui.identity_keys().ok_or(UiError::NoIdentity)?;
        let relays = self.relays.clone();

        self.sabha = Some(Arc::new(
            SabhaEngine::new_with_relays(&keys, relays.clone())
                .await
                .map_err(|e| UiError::Engine(e.to_string()))?,
        ));
        self.vault = Some(Arc::new(
            VaultEngine::new(&keys, relays)
                .await
                .map_err(|e| UiError::Engine(e.to_string()))?,
        ));
        self.sakha = Some(Arc::new(
            SakhaEngine::new(&keys, vec![])
                .await
                .map_err(|e| UiError::Engine(e.to_string()))?,
        ));
        self.restore_sakha_pairing().await;

        // Local-network delivery is not a mode to opt into: bring the mesh up
        // now that we have keys, so a DM can reach someone on this WiFi even
        // with no internet at all. Best-effort — see `sync_saathi_lifecycle`.
        self.sync_saathi_lifecycle().await;

        // Startup observability: the unlock is the gate every frontend waits
        // on, so record how long its two phases actually took.
        tracing::info!(
            kdf_ms,
            total_ms = started.elapsed().as_millis() as u64,
            "vault unlocked: store opened and engines built"
        );

        Ok(identity)
    }

    /// Whether the vault has been unlocked and the engines built.
    pub fn is_vault_unlocked(&self) -> bool {
        self.sabha.is_some()
    }

    /// Re-lock the vault: abort every background loop, drop the engines, the
    /// open encrypted store, and the in-memory identity — the deliberate,
    /// user-initiated counterpart to what process death does by accident (see
    /// AUDIT.md COMMS-01's security-boundary note on `RelayConnectionService`).
    /// After this, [`Self::is_vault_unlocked`] is `false` and every store/engine
    /// -backed method fails with [`UiError::VaultLocked`]/[`UiError::NoIdentity`]
    /// again, exactly as before the first unlock — calling [`Self::unlock_vault`]
    /// resumes normally (a fresh Sabha/Vault/Sakha build, fresh loops).
    ///
    /// Idempotent: locking an already-locked runtime is a harmless no-op.
    pub async fn lock_vault(&mut self) {
        // Say goodbye to the comrades before the engines go: a locked vault
        // is not online, and leaving them to age the claim out would show a
        // green dot next to a person who deliberately shut the door. The
        // send is detached and holds only the vault engine (never the store,
        // whose redb file lock the teardown below is about to reclaim), so a
        // slow or unreachable relay can never make locking hang.
        self.spawn_farewell_beacons();
        // `abort()` only *requests* cancellation at the task's next await
        // point — it does not synchronously drop the task's captured state.
        // Each of these tasks holds its own `Arc` clone of the store/engines
        // (captured once, before the task loop started, so the callbacks
        // inside it work while `self`'s own fields below are cleared) —
        // notably `EncryptedStore`, whose underlying redb file enforces a
        // single-writer lock. Awaiting the (now-erroring) handle after abort
        // blocks until that drop has actually happened, so a `unlock_vault`
        // immediately following a `lock_vault` never races the old store
        // handle for the same file lock.
        for task in [
            self.publish_task.take(),
            self.feed_task.take(),
            self.vault_task.take(),
            self.sakha_sync_task.take(),
            self.outbox_task.take(),
            self.presence_task.take(),
            self.mesh_dm_task.take(),
            self.ble_dm_task.take(),
        ]
        .into_iter()
        .flatten()
        {
            task.abort();
            let _ = task.await;
        }
        if self.saathi.is_some() {
            self.stop_saathi().await;
        }
        self.sabha = None;
        self.vault = None;
        self.sakha = None;
        self.loops_spawned = false;
        self.sakha_sync_spawned = false;
        // A hesitation belongs to the session it happened in. Locking up is a
        // deliberate exit — the goodbye beacon above has just told the comrades
        // we are gone, and a nudge landing after it would claim the opposite.
        // Same reason, one step further: a locked vault is not watching a film
        // with anyone, and a command landing after the goodbye beacon would say
        // otherwise. The peer's own TTL is what actually ends it on their side.
        *self.together.lock().unwrap() = None;
        self.nudge_watch.clear();
        // Where somebody has been standing is not something a locked app should
        // still be holding — see [`Self::travel`].
        self.travel.clear();
        self.ui.lock();
    }

    // ── Milestone 2: connect & stream into the event bus ─────────────────────

    /// Connect the engines and spawn the background Tokio loops that capture
    /// incoming Chitthis (Kind-1) and encrypted DMs (Kind-4) and push them onto
    /// the event bus. Must be called from within a Tokio runtime context.
    ///
    /// All network and decryption work happens inside these spawned tasks,
    /// keeping the UI thread free (Architecture Quality Gate).
    ///
    /// Idempotent: calling it more than once (e.g. after a repeated unlock) is
    /// a no-op, so the feed/DM loops are spawned at most once per runtime.
    pub fn spawn_event_loops(&mut self) {
        if self.loops_spawned {
            return;
        }
        self.loops_spawned = true;

        // Re-publish the saved @handle on every launch. Kind-0 is replaceable
        // (newest wins) and the publish merges into the currently published
        // profile, so this is idempotent — and it heals identities whose
        // original publish was dropped (offline onboarding, relay hiccup),
        // which otherwise stay undiscoverable forever.
        if let (Some(sabha), Some(name)) = (self.sabha.clone(), self.ui.username()) {
            self.spawn_profile_publish(sabha, name);
        }

        if let Some(sabha) = self.sabha.clone() {
            // The small, deliberately-lossy feed channel (not `self.events`,
            // the critical one) — a public-feed flood must never compete with
            // a call signal or DM for ring-buffer space (AUDIT.md COMMS-04).
            let tx = self.feed_events.clone();
            let spec = self.feed_filter_spec();
            self.feed_task = Some(tokio::spawn(async move {
                sabha.connect().await;
                let cb: ChitthiCallback = Box::new(move |event| {
                    // A send error only means no subscribers are listening yet;
                    // the relay loop must keep running regardless.
                    let _ = tx.send(BridgeEvent::IncomingChitthi(ChitthiDto::from_event(&event)));
                });
                if let Err(e) = sabha.subscribe_chitthi_feed(spec, cb).await {
                    warn!("sabha feed loop ended: {e}");
                }
            }));
        }

        if let Some(vault) = self.vault.clone() {
            let tx = self.events.clone();
            let store = self.ui.store_arc();
            // A clone of the vault handle the callback can use to send back
            // delivered receipts (the callback itself is sync; it spawns).
            let vault_cb = vault.clone();
            let dedup = self.call_signal_dedup.clone();
            let outbox = self.outbox.clone();
            let transport_dedup = self.transport_dedup.clone();
            let mesh = self.mesh_link();
            let together_link = TogetherLink {
                session: self.together.clone(),
                starts_seen: self.together_starts_seen.clone(),
                shares_seen: self.together_shares_seen.clone(),
            };
            // Widen the backfill window past the standard gift-wrap skew when
            // this device was last seen longer ago than that (see
            // `VaultEngine::subscribe_inbox_with_callback`'s `since_floor`).
            let since_floor = store.as_ref().and_then(|s| read_watermark(s));
            self.vault_task = Some(tokio::spawn(async move {
                vault.connect().await;
                let cb: VaultCallback = Box::new(move |msg| {
                    let route = DmRoute {
                        label: TRANSPORT_RELAY,
                        dedup: &transport_dedup,
                        mesh: mesh.as_ref(),
                        together: Some(&together_link),
                    };
                    dispatch_incoming_dm(
                        &vault_cb,
                        store.as_ref(),
                        &tx,
                        &dedup,
                        &outbox,
                        &route,
                        msg,
                    );
                });
                if let Err(e) = vault.subscribe_inbox_with_callback(cb, since_floor).await {
                    warn!("vault inbox loop ended: {e}");
                }
            }));
        }

        // Retry queued mail on a cadence (bitchat re-sends its outbox on
        // reconnect events; Comrade has no equivalent signal, so this stands in).
        // Spawned unconditionally alongside the vault loop: a flush with nothing
        // queued is one lock acquisition.
        if self.vault.is_some() {
            let handles = self.handles();
            let wake = self.transport_wake.clone();
            self.outbox_task = Some(tokio::spawn(async move {
                let mut ticker = tokio::time::interval(std::time::Duration::from_secs(
                    OUTBOX_FLUSH_INTERVAL_SECS,
                ));
                // The first tick fires immediately: a launch is exactly when
                // mail queued in a previous session should go out.
                loop {
                    // Either the cadence, or a transport that just came up.
                    // The cadence alone meant a peer joining the WiFi a second
                    // after you hit send left the message under a clock icon
                    // for the rest of the minute — long enough to conclude
                    // local delivery does not work.
                    tokio::select! {
                        _ = ticker.tick() => {}
                        _ = wake.notified() => {}
                    }
                    match handles.flush_outbox().await {
                        Ok(0) => {}
                        Ok(sent) => tracing::info!(sent, "outbox flushed"),
                        Err(e) => tracing::debug!("outbox flush skipped: {e}"),
                    }
                }
            }));
        }

        self.spawn_presence_loop();

        // Begin opening the sealed frames the local mesh sees. The engine
        // itself is started by `unlock_vault` (this method is sync); with no
        // engine running this is a no-op.
        self.spawn_mesh_dm_loop();
        self.spawn_ble_dm_loop();

        // A pairing restored from a previous launch (see `restore_sakha_pairing`,
        // called from `unlock_vault`) should start syncing immediately too —
        // a fresh pairing via `pair_sakha` starts it itself.
        if self.sakha.as_ref().is_some_and(|s| s.is_paired()) {
            self.spawn_sakha_sync_loop();
        }
    }

    /// Start the comrade-presence loop: announce "I'm online" to every
    /// comrade now and every [`PRESENCE_HEARTBEAT_SECS`] after, and on the
    /// same tick age out any comrade whose own claim has lapsed (a phone
    /// that dies sends no goodbye — see [`expire_stale_presence`]).
    ///
    /// Runs only while comrades exist, in the sense that a tick with an
    /// empty comrade list does nothing at all: the feature is invisible and
    /// free until someone opts in.
    fn spawn_presence_loop(&mut self) {
        if self.vault.is_none() {
            return;
        }
        let handles = self.handles();
        let tx = self.events.clone();
        self.presence_task = Some(tokio::spawn(async move {
            let mut ticker =
                tokio::time::interval(std::time::Duration::from_secs(PRESENCE_SWEEP_SECS));
            // The default `Burst` behaviour would fire back-to-back catch-up
            // ticks after a suspended/backgrounded stretch, announcing several
            // times in a row for no benefit.
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            // Announcing costs a DM per comrade, so it happens on the
            // heartbeat; expiring is local work over a handful of rows, so it
            // happens on every (shorter) sweep — a dot must not stay green for
            // an extra heartbeat after the claim behind it lapsed. The first
            // tick fires immediately, so unlocking announces right away.
            let announce_every = PRESENCE_HEARTBEAT_SECS.div_ceil(PRESENCE_SWEEP_SECS).max(1);
            let mut tick: u64 = 0;
            loop {
                ticker.tick().await;
                if tick.is_multiple_of(announce_every) {
                    // Refreshes only while the app is open — a backgrounded
                    // device stays silent instead of undoing its own goodbye.
                    handles.refresh_presence().await;
                }
                expire_stale_presence(handles.store.as_deref(), &tx);
                // Abandoned drafts ride this sweep rather than a timer of
                // their own: a nudge is a courtesy that costs one DM, and
                // arriving a sweep late only makes it more certain (the
                // writer had that much longer to come back). What it must
                // never do is arrive *early* — see `NUDGE_SETTLE_SECS`.
                handles.nudge_abandoned_drafts(now_secs()).await;
                tick = tick.wrapping_add(1);
            }
        }));
    }

    /// Open the sealed frames the local mesh sees and feed the ones addressed to
    /// us through the **same** ingress path a relay-delivered DM takes — so
    /// message-request gating, persistence, receipts, and dedup behave
    /// identically however a message arrived.
    ///
    /// Every peer on the network sees every frame; almost all of them are for
    /// someone else and are skipped after one HMAC comparison against our
    /// rotating tags. Idempotent: a second call replaces nothing, since
    /// [`Self::lock_vault`] is what tears the task down.
    fn spawn_mesh_dm_loop(&mut self) {
        if self.mesh_dm_task.is_some() {
            return;
        }
        let (Some(engine), Some(vault), Some(keys)) = (
            self.saathi.clone(),
            self.vault.clone(),
            self.ui.identity_keys(),
        ) else {
            return;
        };

        let radios = self.mesh_link();
        let Some(ingress) = self.sealed_ingress(vault, keys, radios) else {
            return;
        };

        self.mesh_dm_task = Some(tokio::spawn(async move {
            while let Some(envelope) = engine.recv_sealed().await {
                ingress.accept(&envelope, now_secs());
            }
            tracing::debug!("mesh: sealed-frame stream ended");
        }));
    }

    /// Consume sealed envelopes rebuilt from BLE fragments, through the same
    /// ingress the WiFi mesh uses.
    ///
    /// Two radios, one door. A frame that arrived over Bluetooth is opened,
    /// authenticated and dispatched by exactly the code that handles a frame
    /// off the local network — which is what keeps message-request gating,
    /// receipts, dedup and `/pay` detection from needing a third implementation
    /// that could drift from the other two.
    fn spawn_ble_dm_loop(&mut self) {
        if self.ble_dm_task.is_some() {
            return;
        }
        let (Some(vault), Some(keys)) = (self.vault.clone(), self.ui.identity_keys()) else {
            return;
        };
        let mesh = self.mesh_link();
        let Some(ingress) = self.sealed_ingress(vault, keys, mesh) else {
            return;
        };
        let mut inbound = self.ble.subscribe_inbound();

        self.ble_dm_task = Some(tokio::spawn(async move {
            while let Some(envelope) = inbound.recv().await {
                ingress.accept(&envelope, now_secs());
            }
            tracing::debug!("ble: sealed-frame stream ended");
        }));
    }

    /// Assemble the shared sealed-frame ingress — everything an opened frame
    /// needs, independent of which radio carried it.
    fn sealed_ingress(
        &self,
        vault: Arc<VaultEngine>,
        keys: nostr_sdk::prelude::Keys,
        mesh: Option<LocalRadios>,
    ) -> Option<SealedIngress> {
        Some(SealedIngress {
            vault,
            keys,
            store: self.ui.store_arc(),
            tx: self.events.clone(),
            call_dedup: self.call_signal_dedup.clone(),
            transport_dedup: self.transport_dedup.clone(),
            outbox: self.outbox.clone(),
            mesh,
            together: TogetherLink {
                session: self.together.clone(),
                starts_seen: self.together_starts_seen.clone(),
                shares_seen: self.together_shares_seen.clone(),
            },
            pay_regex: build_pay_regex().ok(),
        })
    }

    /// The public-feed subscription policy (AUDIT.md COMMS-04): self plus
    /// every accepted contact when any are known, else a capped, time-windowed
    /// bootstrap feed for a fresh identity that hasn't added anyone yet — never
    /// the unbounded relay-wide firehose `subscribe_chitthi_feed` used to get.
    fn feed_filter_spec(&self) -> FeedFilterSpec {
        let mut authors: Vec<nostr_sdk::prelude::PublicKey> = Vec::new();
        if let Some(id) = self.ui.current_identity() {
            if let Ok(pk) = parse_pubkey(&id.npub) {
                authors.push(pk);
            }
        }
        if let Ok(contacts) = self.list_contacts() {
            authors.extend(contacts.iter().filter_map(|c| parse_pubkey(&c.npub).ok()));
        }
        // More than just self means at least one real contact is followed.
        let scope = if authors.len() > 1 {
            FeedScope::Authors(authors)
        } else {
            FeedScope::BoundedGlobal {
                limit: FEED_BOOTSTRAP_LIMIT,
            }
        };
        FeedFilterSpec {
            scope,
            since_secs: FEED_SINCE_SECS,
        }
    }

    // ── Milestone 1: timeline + broadcast ────────────────────────────────────

    /// Load the Sabha timeline from the encrypted on-disk cache (offline-first).
    pub fn fetch_sabha_timeline(&self) -> Result<Vec<ChitthiDto>, UiError> {
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        let feed = store
            .chitthi_cache()
            .map_err(|e| UiError::Storage(e.to_string()))?;
        Ok(feed.iter().map(ChitthiDto::from_cached).collect())
    }

    /// Broadcast a Chitthi to the public relay set, optionally as a NIP-10 reply.
    /// On success the Chitthi is also cached locally for offline rendering.
    /// Returns the new event id (hex).
    ///
    /// Delegates to [`RuntimeHandles::broadcast_chitthi`] — see [`Self::send_dm`].
    pub async fn broadcast_chitthi(
        &self,
        content: &str,
        reply_to: Option<String>,
    ) -> Result<String, UiError> {
        self.handles().broadcast_chitthi(content, reply_to).await
    }

    // ── Direct messages (Telegram-like chat flow) ────────────────────────────

    /// Send an end-to-end encrypted DM to `target` (npub or hex pubkey) and
    /// persist it to the offline history. Returns the stored message DTO.
    ///
    /// Delegates to [`RuntimeHandles::send_dm`] after a cheap, synchronous
    /// handle snapshot (AUDIT P2: never hold the runtime lock across the
    /// relay round-trip inside — see [`Self::handles`]).
    pub async fn send_dm(&self, target: &str, content: &str) -> Result<MessageDto, UiError> {
        self.handles().send_dm(target, content).await
    }

    /// Send an E2E DM, optionally as a reply to a prior message (`reply_to` is
    /// the replied message's event id, hex). Sending to someone accepts the
    /// conversation on our side and shares our @handle once (so they can title
    /// the chat) — the sender-side half of "username shared once engaged".
    ///
    /// Delegates to [`RuntimeHandles::send_dm_reply`] — see [`Self::send_dm`].
    pub async fn send_dm_reply(
        &self,
        target: &str,
        content: &str,
        reply_to: Option<&str>,
    ) -> Result<MessageDto, UiError> {
        self.handles()
            .send_dm_reply(target, content, reply_to)
            .await
    }

    /// React to a message in `peer`'s thread, or take an existing reaction back
    /// by tapping the same emoji again. Returns the reaction now standing, or
    /// `None` if the tap withdrew one.
    ///
    /// Delegates to [`RuntimeHandles::toggle_reaction`] — see [`Self::send_dm`]
    /// for why the handle snapshot comes first, and that method for why the
    /// toggle decision lives on this side of the FFI rather than in each frontend.
    pub async fn toggle_reaction(
        &self,
        peer: &str,
        target_id: &str,
        emoji: &str,
    ) -> Result<Option<ReactionDto>, UiError> {
        self.handles().toggle_reaction(peer, target_id, emoji).await
    }

    /// Every reaction in `peer`'s conversation, oldest first — read from the
    /// encrypted store, so a thread opens with its reactions already on it rather
    /// than waiting for a live event.
    ///
    /// Withdrawn reactions are not returned (the store keeps a tombstone to
    /// refuse replays; see `EncryptedStore::set_reaction`).
    pub fn reactions(&self, peer: &str) -> Result<Vec<ReactionDto>, UiError> {
        let store = self.ui.store_ref().ok_or(UiError::StoreLocked)?;
        let rows = store
            .reactions_with(&to_npub(peer))
            .map_err(|e| UiError::Storage(e.to_string()))?;
        Ok(rows.into_iter().map(ReactionDto::from).collect())
    }

    /// Retry queued mail now. Delegates to [`RuntimeHandles::flush_outbox`] —
    /// see [`Self::send_dm`]. Returns how many messages a relay accepted.
    ///
    /// A host that learns connectivity came back (Android's network callback,
    /// the desktop resuming from sleep) should call this instead of waiting for
    /// the periodic loop.
    pub async fn flush_outbox(&self) -> Result<usize, UiError> {
        self.handles().flush_outbox().await
    }

    /// How many messages are waiting for a relay that will take them.
    pub fn outbox_pending(&self) -> usize {
        self.outbox.len()
    }

    /// Broadcast a Chitthi under a throwaway or scoped persona instead of this
    /// device's identity. Delegates to
    /// [`RuntimeHandles::broadcast_anonymous_chitthi`] — see [`Self::send_dm`].
    pub async fn broadcast_anonymous_chitthi(
        &self,
        content: &str,
        scope: Option<String>,
    ) -> Result<String, UiError> {
        self.handles()
            .broadcast_anonymous_chitthi(content, scope.as_deref())
            .await
    }

    /// Device-local delivery counters — bare tallies with no identities, ids,
    /// content, or timestamps, so they are safe to render on a diagnostics
    /// screen or paste into a bug report.
    ///
    /// Adopted from bitchat's `StoreAndForwardMetrics`. Nothing here leaves the
    /// device: there is no exporter and no per-peer dimension to re-identify.
    pub fn metrics_snapshot(&self) -> Vec<MetricDto> {
        core_metrics::snapshot()
            .into_iter()
            .map(|(key, value)| MetricDto { key, value })
            .collect()
    }

    /// **Panic wipe** — destroy everything this device holds, then re-lock.
    ///
    /// Adopted from bitchat, whose privacy assessment names the threat plainly:
    /// for many of the people this is built for, the realistic compromise is a
    /// phone taken, often unlocked under duress. A journal of someone's worst
    /// weeks should not still be there at that point.
    ///
    /// In order: every stored value goes (identity keys included — see
    /// `comrade_storage::EncryptedStore::panic_wipe`, which enumerates the
    /// database's actual tables so a tree added later cannot be missed), then
    /// the in-memory queues, dedup sets, and counters, then the engines and
    /// identity via [`Self::lock_vault`]. Afterwards the runtime is exactly as
    /// it was before its first unlock, and a fresh unlock starts onboarding
    /// over.
    ///
    /// What it deliberately does not do is pretend to be a duress feature: it
    /// wipes, it does not hide, and it needs the app open and unlocked to run.
    pub async fn panic_wipe(&mut self) -> Result<(), UiError> {
        let store = self.ui.store_arc().ok_or(UiError::VaultLocked)?;
        // redb commits are fsync'd, so this is real I/O — keep it off the
        // reactor thread.
        tokio::task::spawn_blocking(move || store.panic_wipe())
            .await
            .map_err(|e| UiError::Storage(format!("wipe task failed: {e}")))?
            .map_err(|e| UiError::Storage(e.to_string()))?;

        self.outbox.clear();
        self.call_signal_dedup.clear();
        // Same argument as the call set: these are peer event ids, and the
        // promise above is that nothing survives. Nothing is reopened by
        // clearing them either — `lock_vault` below drops the live session, and
        // a replayed share signal with no session to name is dropped by the
        // session lookup regardless.
        self.together_shares_seen.clear();
        core_metrics::reset();
        self.lock_vault().await;
        // `debug!`, not `warn!`, since 2026-08-09 — when Rust warnings started
        // reaching Android's logcat. This function's doc is careful to say the
        // wipe "does not hide", and that is still true; what changed is that a
        // warn-level line here would newly leave *"local state destroyed"* in a
        // buffer `adb` reads, timestamped, for a feature whose threat model is a
        // phone taken under duress. The line is a finished-marker with no
        // diagnostic value that debug cannot serve, so there is nothing to weigh
        // against it.
        tracing::debug!("panic wipe complete: local state destroyed and vault locked");
        Ok(())
    }

    /// The chat list: one entry per **accepted** peer, newest thread first, with
    /// saved contact aliases joined in. Pending message requests and blocked
    /// peers are excluded (see [`Self::message_requests`]).
    pub fn conversations(&self) -> Result<Vec<ConversationDto>, UiError> {
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        let contacts = store
            .list_contacts()
            .map_err(|e| UiError::Storage(e.to_string()))?;
        let comrades: std::collections::HashSet<String> = contacts
            .iter()
            .filter(|c| c.comrade)
            .map(|c| c.npub.clone())
            .collect();
        let aliases: std::collections::HashMap<String, String> =
            contacts.into_iter().map(|c| (c.npub, c.petname)).collect();
        let now = now_secs();
        // Peers gated out of the chat list: pending requests + blocked. A peer
        // with no meta at all (e.g. history from before this feature) is treated
        // as an ordinary accepted conversation.
        let gated = self.gated_peers(store)?;

        // `(created_at, preview, outgoing)` for the newest item in each thread —
        // text or attachment, whichever is newer.
        let mut newest: std::collections::HashMap<String, (u64, String, bool)> =
            std::collections::HashMap::new();
        let mut consider = |peer: String, at: u64, preview: String, outgoing: bool| {
            if gated.contains(&peer) {
                return;
            }
            match newest.get(&peer) {
                Some((existing_at, _, _)) if *existing_at >= at => {}
                _ => {
                    newest.insert(peer, (at, preview, outgoing));
                }
            }
        };
        for msg in store
            .all_messages()
            .map_err(|e| UiError::Storage(e.to_string()))?
        {
            consider(msg.peer_npub, msg.created_at, msg.content, msg.outgoing);
        }
        // An attachment is a thread's newest item as often as a text message is,
        // and a thread that only ever exchanged photos has no messages at all.
        for (peer, media) in newest_media_by_peer(store)? {
            consider(peer, media.created_at, media.preview, media.outgoing);
        }

        let mut list: Vec<ConversationDto> = newest
            .into_iter()
            .map(|(peer, (last_at, last_message, last_outgoing))| {
                let comrade = comrades.contains(&peer);
                ConversationDto {
                    alias: aliases.get(&peer).and_then(|a| user_alias(a, &peer)),
                    peer_name: cached_peer_name(store, &peer),
                    // Presence only exists between comrades — never imply
                    // knowledge about anyone else's whereabouts.
                    online: comrade && peer_is_online(store, &peer, now),
                    comrade,
                    peer,
                    last_message,
                    last_at,
                    last_outgoing,
                }
            })
            .collect();
        list.sort_by_key(|c| std::cmp::Reverse(c.last_at));
        Ok(list)
    }

    /// Full offline message history with `peer` (npub or hex), oldest first —
    /// carrying each message's delivery status, reply target, and local
    /// bookmark/pin state. Not gated, so a pending request's thread is
    /// viewable before it is accepted.
    ///
    /// A message with a delete-for-me tombstone is excluded here — that is the
    /// whole reason the tombstone exists rather than a row delete: a backfill
    /// that redelivers the same event id must not bring it back. See
    /// `EncryptedStore::delete_message_for_me`.
    pub fn messages_with(&self, peer: &str) -> Result<Vec<MessageDto>, UiError> {
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        let peer = to_npub(peer);
        let starred = starred_ids(store, &peer)?;
        let pinned = pinned_ids(store, &peer)?;
        let mut msgs: Vec<MessageDto> = store
            .messages_with(&peer)
            .map_err(|e| UiError::Storage(e.to_string()))?
            .into_iter()
            .filter(|m| !store.is_deleted_for_me(&peer, &m.id).unwrap_or(false))
            .map(|m| {
                let actions = MessageActionState {
                    starred: starred.contains(&m.id),
                    pinned: pinned.contains(&m.id),
                };
                stored_message_dto(m, actions)
            })
            .collect();
        msgs.sort_by_key(|m| m.created_at);
        Ok(msgs)
    }

    /// Every pinned message in `peer`'s conversation, oldest pin first — the
    /// order a pinned-messages bar scrolls through. A message unpinned or
    /// deleted-for-me since being pinned is silently absent rather than an
    /// error: the pin row and the message row are independent, so either can
    /// outlive the other by a moment.
    pub fn pinned_messages(&self, peer: &str) -> Result<Vec<MessageDto>, UiError> {
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        let peer = to_npub(peer);
        let starred = starred_ids(store, &peer)?;
        store
            .pinned_with(&peer)
            .map_err(|e| UiError::Storage(e.to_string()))?
            .into_iter()
            .filter(|p| {
                !store
                    .is_deleted_for_me(&peer, &p.message_id)
                    .unwrap_or(false)
            })
            .filter_map(|p| store.get_message(&p.message_id).ok().flatten())
            .map(|m| {
                let starred = starred.contains(&m.id);
                Ok(stored_message_dto(
                    m,
                    MessageActionState {
                        starred,
                        pinned: true,
                    },
                ))
            })
            .collect()
    }

    /// Every starred (bookmarked) message across every conversation, oldest
    /// star first — the "Starred Messages" screen, reading across
    /// conversations the way Telegram's does rather than one at a time.
    pub fn starred_messages(&self) -> Result<Vec<MessageDto>, UiError> {
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        store
            .all_starred()
            .map_err(|e| UiError::Storage(e.to_string()))?
            .into_iter()
            .filter(|s| {
                !store
                    .is_deleted_for_me(&s.peer_npub, &s.message_id)
                    .unwrap_or(false)
            })
            .filter_map(|s| {
                let pinned = store
                    .pinned_with(&s.peer_npub)
                    .ok()?
                    .iter()
                    .any(|p| p.message_id == s.message_id);
                store
                    .get_message(&s.message_id)
                    .ok()
                    .flatten()
                    .map(|m| (m, pinned))
            })
            .map(|(m, pinned)| {
                Ok(stored_message_dto(
                    m,
                    MessageActionState {
                        starred: true,
                        pinned,
                    },
                ))
            })
            .collect()
    }

    /// Star or un-star one of `peer`'s messages for the "starred messages"
    /// list. Local device state only — see `EncryptedStore::star_message`.
    /// Returns whether the stored state changed.
    pub fn star_message(
        &self,
        peer: &str,
        message_id: &str,
        starred: bool,
    ) -> Result<bool, UiError> {
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        store
            .star_message(&to_npub(peer), message_id, starred)
            .map_err(|e| UiError::Storage(e.to_string()))
    }

    /// Pin one of `peer`'s messages. Refuses once the conversation is already
    /// at `EncryptedStore::MAX_PINNED_PER_CONVERSATION` — unpin one first.
    /// Returns `false` (not an error) if it was already pinned.
    pub fn pin_message(&self, peer: &str, message_id: &str) -> Result<bool, UiError> {
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        store
            .pin_message(&to_npub(peer), message_id)
            .map_err(|e| UiError::Storage(e.to_string()))
    }

    /// Unpin one of `peer`'s messages. `true` if it was pinned.
    pub fn unpin_message(&self, peer: &str, message_id: &str) -> Result<bool, UiError> {
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        store
            .unpin_message(&to_npub(peer), message_id)
            .map_err(|e| UiError::Storage(e.to_string()))
    }

    /// Hide one of `peer`'s messages on this device only — a tombstone, not a
    /// row delete, so a relay's cold-start rescan (or a mesh replay) cannot
    /// bring it back. See `EncryptedStore::delete_message_for_me`.
    pub fn delete_message_for_me(&self, peer: &str, message_id: &str) -> Result<(), UiError> {
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        store
            .delete_message_for_me(&to_npub(peer), message_id)
            .map_err(|e| UiError::Storage(e.to_string()))
    }

    /// Delete a message you sent for everyone in `peer`'s conversation.
    ///
    /// **This is not a NIP-09 (kind-5) retraction, and cannot be** — see
    /// `comrade_core::dm::DeleteRequest`'s doc for exactly why: this app's DMs
    /// are NIP-17 gift wraps signed by a one-time key that
    /// `VaultEngine::send_dm_reply` discards the moment the send returns, so
    /// there is no key left on this device to author a real retraction of a
    /// message it sent even a minute ago. What this does instead — hide it
    /// here immediately, and ask the peer's client to hide its copy too — is
    /// the same best-effort courtesy WhatsApp's and Signal's "delete for
    /// everyone" already run on: honoured by a cooperating client, silent on
    /// one that ignores it or already showed the message.
    ///
    /// Only the message's own sender may call this — refused for an incoming
    /// message, the same restriction WhatsApp enforces and for the same
    /// reason: "delete for everyone" pointed at someone else's message is
    /// really "delete for me" wearing a stronger name.
    pub async fn delete_message_for_everyone(
        &self,
        peer: &str,
        message_id: &str,
    ) -> Result<(), UiError> {
        let peer_npub = to_npub(peer);
        let peer_pk = parse_pubkey(peer)?;
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        let msg = store
            .get_message(message_id)
            .map_err(|e| UiError::Storage(e.to_string()))?
            .filter(|m| m.peer_npub == peer_npub)
            .ok_or_else(|| {
                UiError::Engine(format!("no message {message_id} in that conversation"))
            })?;
        if !msg.outgoing {
            return Err(UiError::Engine(
                "only the sender can delete a message for everyone".into(),
            ));
        }
        store
            .delete_message_for_me(&peer_npub, message_id)
            .map_err(|e| UiError::Storage(e.to_string()))?;
        if let Some(vault) = &self.vault {
            if let Ok(json) = DeleteRequest::new(message_id).to_json() {
                // Best-effort, like a receipt: the local hide already happened
                // above, so a peer who is offline or on a client that ignores
                // this envelope simply keeps their own copy — which is exactly
                // the courtesy-not-guarantee this method's doc describes.
                let _ = vault.send_dm(&peer_pk, &json).await;
            }
        }
        Ok(())
    }

    /// Forward one of `from_peer`'s messages to each of `to_peers`, as a new
    /// message in each conversation.
    ///
    /// **A label, not an attestation** — the same standing
    /// [`MessageAuthor`]/[`SharedNoteDto`] already carry. Forwarding sends a
    /// *copy*, signed and delivered by the forwarder over their own
    /// conversation with each recipient; nothing here lets it claim the
    /// original sender's identity, cryptographically or otherwise. The
    /// forwarded text is stripped down to plain words first (see
    /// [`forwarded_line`]) so a re-shared journal note or link preview cannot
    /// smuggle a second claim inside the forward.
    pub async fn forward_message(
        &self,
        from_peer: &str,
        message_id: &str,
        to_peers: &[String],
    ) -> Result<Vec<MessageDto>, UiError> {
        if to_peers.is_empty() {
            return Err(UiError::Engine(
                "forwarding needs at least one recipient".into(),
            ));
        }
        let text = {
            let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
            let from_npub = to_npub(from_peer);
            let row = store
                .get_message(message_id)
                .map_err(|e| UiError::Storage(e.to_string()))?
                .filter(|m| m.peer_npub == from_npub)
                .ok_or_else(|| {
                    UiError::Engine(format!("no message {message_id} in that conversation"))
                })?;
            if store
                .is_deleted_for_me(&from_npub, message_id)
                .unwrap_or(false)
            {
                return Err(UiError::Engine("that message was deleted".into()));
            }
            read_body(row.content).text
        };
        let body = forwarded_line(&text);
        let mut sent = Vec::with_capacity(to_peers.len());
        for peer in to_peers {
            sent.push(self.send_dm(peer, &body).await?);
        }
        Ok(sent)
    }

    // ── Threads and topics (see `comrade_core::topic`) ───────────────────────

    /// Every topic in `peer`'s conversation, oldest first, with live counts.
    ///
    /// Closed ones are included — the archive has to be reachable, and the
    /// picker is where the filtering belongs.
    pub fn topics(&self, peer: &str) -> Result<Vec<TopicDto>, UiError> {
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        let peer_npub = to_npub(peer);
        let peer_hex = parse_pubkey(peer)?.to_hex();
        let me = self.my_npub().unwrap_or_default();
        let index = ThreadIndex::build(store, &peer_npub, &peer_hex)?;
        Ok(store
            .topics_with(&peer_npub)
            .map_err(|e| UiError::Storage(e.to_string()))?
            .into_iter()
            .map(|row| topic_dto(row, &me, &index))
            .collect())
    }

    /// Every thread in `peer`'s conversation, most recently active first.
    ///
    /// `topic_slug` of `None` is "all threads" rather than "unfiled threads" —
    /// the sheet's default view is everything, and a caller wanting only the
    /// unfiled ones filters on [`ThreadSummaryDto::topic_slug`], which it can
    /// do without a second round trip.
    ///
    /// Threads of one — a message nobody replied to — are included. Whether to
    /// show them is a frontend's call: the desktop panel lists them so a thread
    /// can be *started* from the sheet, and Android's sheet hides them.
    pub fn threads(
        &self,
        peer: &str,
        topic_slug: Option<String>,
    ) -> Result<Vec<ThreadSummaryDto>, UiError> {
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        let peer_npub = to_npub(peer);
        let peer_hex = parse_pubkey(peer)?.to_hex();
        let index = ThreadIndex::build(store, &peer_npub, &peer_hex)?;
        let wanted = topic_slug.and_then(|s| comrade_core::topic::slugify(&s));
        Ok(index
            .summaries(&peer_npub)
            .into_iter()
            .filter(|t| match &wanted {
                Some(slug) => t.topic_slug.as_deref() == Some(slug.as_str()),
                None => true,
            })
            .collect())
    }

    /// One thread in full: the root and everything that replied into it.
    ///
    /// `root_id` may name any message in the thread rather than only its root —
    /// this resolves upwards first, so a frontend that has a tapped bubble's id
    /// and not its ancestry gets the right sheet. Returns an empty thread (not
    /// an error) for an id we hold nothing for; see
    /// [`ThreadSummaryDto::root_missing`] for why that is a normal state.
    pub fn thread(&self, peer: &str, root_id: &str) -> Result<ThreadDto, UiError> {
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        let peer_npub = to_npub(peer);
        let peer_hex = parse_pubkey(peer)?.to_hex();
        let root = resolve_thread_root(store, &peer_npub, root_id)?;
        let index = ThreadIndex::build(store, &peer_npub, &peer_hex)?;
        let members: std::collections::HashSet<&str> = index
            .threads
            .get(&root)
            .map(|m| m.iter().map(String::as_str).collect())
            .unwrap_or_default();
        let messages = self
            .messages_with(&peer_npub)?
            .into_iter()
            .filter(|m| members.contains(m.id.as_str()))
            .collect();
        let media = self
            .media_with(&peer_npub)?
            .into_iter()
            .filter(|m| members.contains(m.event_id.as_str()))
            .collect();
        Ok(ThreadDto {
            topic_slug: index.filed.get(&root).cloned(),
            root_id: root,
            peer: peer_npub,
            messages,
            media,
        })
    }

    /// The id of the thread `message_id` belongs to.
    ///
    /// What a frontend calls before opening a sheet from a tapped bubble, so
    /// the four surfaces do not each re-implement the walk up the reply chain —
    /// which is the drift `/pay` already demonstrated.
    pub fn thread_root(&self, peer: &str, message_id: &str) -> Result<String, UiError> {
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        resolve_thread_root(store, &to_npub(peer), message_id)
    }

    /// Name a topic in `peer`'s conversation, and tell them.
    ///
    /// Idempotent: the slug is the id, so naming an existing topic returns it
    /// rather than failing. Delegates to [`RuntimeHandles::create_topic`] — see
    /// [`Self::send_dm`] for why the network half never runs under the lock.
    pub async fn create_topic(&self, peer: &str, name: &str) -> Result<TopicDto, UiError> {
        self.handles().create_topic(peer, name).await
    }

    /// File the thread containing `message_id` under `topic_name`, creating the
    /// topic if it is new — or, with `topic_name` of `None`, take it out of
    /// wherever it was.
    ///
    /// Takes the *message* rather than the thread root on purpose: resolving
    /// the root is this side's job (see [`Self::thread_root`]), and a frontend
    /// that had to do it could file the wrong thread from a reply.
    ///
    /// Delegates to [`RuntimeHandles::assign_thread`].
    pub async fn assign_thread(
        &self,
        peer: &str,
        message_id: &str,
        topic_name: Option<String>,
    ) -> Result<ThreadSummaryDto, UiError> {
        self.handles()
            .assign_thread(peer, message_id, topic_name)
            .await
    }

    /// Archive a topic, or bring it back. Delegates to
    /// [`RuntimeHandles::set_topic_closed`].
    pub async fn set_topic_closed(
        &self,
        peer: &str,
        slug: &str,
        closed: bool,
    ) -> Result<TopicDto, UiError> {
        self.handles().set_topic_closed(peer, slug, closed).await
    }

    /// Reply inside a thread.
    ///
    /// [`Self::send_dm_reply`] with the thread's root as the target, and the
    /// root resolved here rather than by the caller — so a reply typed into the
    /// sheet lands in the thread the sheet is showing even when the last thing
    /// read in it was itself a reply. That is the difference between Slack's
    /// thread composer and quoting a message: the flat shape is the feature.
    ///
    /// Delegates to [`RuntimeHandles::send_thread_reply`].
    pub async fn send_thread_reply(
        &self,
        peer: &str,
        root_id: &str,
        content: &str,
    ) -> Result<MessageDto, UiError> {
        self.handles()
            .send_thread_reply(peer, root_id, content)
            .await
    }

    // ── Message requests (gate strangers; accept/block; profile on accept) ────

    /// Peers to hide from the chat list: those with a `pending` or `blocked`
    /// conversation gate. A peer with no gate record is shown (accepted).
    fn gated_peers(
        &self,
        store: &comrade_storage::EncryptedStore,
    ) -> Result<std::collections::HashSet<String>, UiError> {
        Ok(store
            .list_conversation_meta()
            .map_err(|e| UiError::Storage(e.to_string()))?
            .into_iter()
            .filter(|m| m.state == STATE_PENDING || m.state == STATE_BLOCKED)
            .map(|m| m.peer_npub)
            .collect())
    }

    /// Pending message requests — strangers' DMs awaiting accept/block, newest
    /// first, with a preview of their latest message.
    pub fn message_requests(&self) -> Result<Vec<MessageRequestDto>, UiError> {
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        let pending: std::collections::HashSet<String> = store
            .list_conversation_meta()
            .map_err(|e| UiError::Storage(e.to_string()))?
            .into_iter()
            .filter(|m| m.state == STATE_PENDING)
            .map(|m| m.peer_npub)
            .collect();
        if pending.is_empty() {
            return Ok(vec![]);
        }
        // Newest item per pending peer: `(created_at, preview)`.
        let mut newest: std::collections::HashMap<String, (u64, String)> =
            std::collections::HashMap::new();
        let mut consider = |peer: String, at: u64, preview: String| {
            if !pending.contains(&peer) {
                return;
            }
            match newest.get(&peer) {
                Some((existing_at, _)) if *existing_at >= at => {}
                _ => {
                    newest.insert(peer, (at, preview));
                }
            }
        };
        for msg in store
            .all_messages()
            .map_err(|e| UiError::Storage(e.to_string()))?
        {
            consider(msg.peer_npub, msg.created_at, msg.content);
        }
        // A stranger whose first contact was an attachment is still a request
        // with something to preview — and, before this, a request row with an
        // empty line (the gated branch of `dispatch_incoming_dm` persists the
        // media ref but no message).
        for (peer, media) in newest_media_by_peer(store)? {
            consider(peer, media.created_at, media.preview);
        }
        let mut list: Vec<MessageRequestDto> = newest
            .into_iter()
            .map(|(peer, (last_at, last_message))| MessageRequestDto {
                peer,
                last_message,
                last_at,
            })
            .collect();
        list.sort_by_key(|r| std::cmp::Reverse(r.last_at));
        Ok(list)
    }

    /// Accept a pending message request: mark the conversation accepted, share
    /// our @handle with the peer (this is the moment "the username is shared"),
    /// and acknowledge their messages as read. The conversation now appears in
    /// the chat list. Idempotent for an already-accepted peer.
    pub fn accept_request(&self, peer: &str) -> Result<(), UiError> {
        let peer_pk = parse_pubkey(peer)?;
        let peer_npub = peer_pk
            .to_bech32()
            .map_err(|e| UiError::Engine(e.to_string()))?;
        if self.ui.store_ref().is_none() {
            return Err(UiError::VaultLocked);
        }
        self.mark_accepted_and_share_profile(&peer_npub, &peer_pk);
        // Their messages are now read — acknowledge them.
        let ids = self.incoming_ids(&peer_npub);
        self.spawn_receipt(&peer_pk, ReceiptKind::Read, ids);
        Ok(())
    }

    /// Block a peer: hide them from the chat list and drop their future DMs in
    /// the inbox loop. The message history is left intact locally.
    pub fn block_conversation(&self, peer: &str) -> Result<(), UiError> {
        let peer_pk = parse_pubkey(peer)?;
        let peer_npub = peer_pk
            .to_bech32()
            .map_err(|e| UiError::Engine(e.to_string()))?;
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        // Carry the read position across the block. Nothing reads it while the
        // thread is hidden, but throwing it away would silently restart the
        // history of a peer who is later talked to again.
        let last_read_at = store.read_position(&peer_npub).unwrap_or(0);
        let meta = comrade_storage::ConversationMeta {
            peer_npub,
            state: STATE_BLOCKED.to_string(),
            profile_shared: false,
            last_read_at,
            updated_at: now_secs(),
        };
        store
            .set_conversation_meta(&meta)
            .and_then(|()| store.flush())
            .map_err(|e| UiError::Storage(e.to_string()))
    }

    /// Mark a conversation read: send a read receipt covering the peer's
    /// incoming messages, record how far the user has now read, and return the
    /// position they had reached *before* this call.
    ///
    /// The frontend uses that previous position to open the thread at the first
    /// unread message rather than at the newest one (Telegram's behaviour), and
    /// to draw the "unread messages" divider. It is returned from this call
    /// rather than exposed as a separate getter so there is no window in which
    /// the caller's own mark-read has already overwritten the answer it is
    /// about to position on.
    ///
    /// Returns 0 when the thread has never been opened, which the UI reads as
    /// "no divider, open at the newest message" — for a first visit there is no
    /// meaningful "where I left off".
    ///
    /// Accepted conversations only: we never ack a pending request. A pending
    /// thread also keeps its read position untouched, because acking or
    /// recording it would leak that we read it before deciding to accept.
    pub fn mark_conversation_read(&self, peer: &str) -> Result<u64, UiError> {
        let peer_pk = parse_pubkey(peer)?;
        let peer_npub = peer_pk
            .to_bech32()
            .map_err(|e| UiError::Engine(e.to_string()))?;
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        // Only ack accepted conversations — acking a pending request would leak
        // that we saw it before deciding to accept.
        let accepted = store
            .get_conversation_meta(&peer_npub)
            .map_err(|e| UiError::Storage(e.to_string()))?
            .map(|m| m.state == STATE_ACCEPTED)
            .unwrap_or(true); // no gate record ⇒ ordinary conversation
        if !accepted {
            return Ok(0);
        }
        // Everything currently in the thread is what the reader is being shown,
        // so the newest stored message is the new watermark. Taken from the
        // store rather than the clock: a message that arrives one second after
        // this call must stay unread, and a clock-based mark would swallow it.
        let newest = store
            .messages_with(&peer_npub)
            .map(|msgs| msgs.iter().map(|m| m.created_at).max().unwrap_or(0))
            .unwrap_or(0);
        let previous = store
            .advance_read_position(&peer_npub, newest, now_secs(), STATE_ACCEPTED)
            .map_err(|e| UiError::Storage(e.to_string()))?;
        let ids = self.incoming_ids(&peer_npub);
        self.spawn_receipt(&peer_pk, ReceiptKind::Read, ids);
        Ok(previous)
    }

    /// Event ids of the peer's incoming (received) messages in this thread.
    fn incoming_ids(&self, peer_npub: &str) -> Vec<String> {
        self.ui
            .store_ref()
            .and_then(|s| s.messages_with(peer_npub).ok())
            .map(|msgs| {
                msgs.into_iter()
                    .filter(|m| !m.outgoing)
                    .map(|m| m.id)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Record the conversation as accepted and, once, share our @handle with the
    /// peer over the encrypted channel. See free function
    /// [`share_profile_on_accept`] for the implementation (shared with
    /// [`RuntimeHandles::send_dm_reply`]).
    fn mark_accepted_and_share_profile(&self, peer_npub: &str, peer: &PublicKey) {
        share_profile_on_accept(
            self.ui.store_arc(),
            self.vault.clone(),
            self.ui.username(),
            peer_npub,
            peer,
        );
    }

    /// Fire-and-forget a final "offline" beacon to every comrade, for a
    /// deliberate exit (vault lock). Reads the comrade list synchronously —
    /// before [`Self::lock_vault`] drops the store — and hands only the vault
    /// engine to the spawned send, so the store's file lock is free to be
    /// reclaimed immediately regardless of how long the relay takes.
    fn spawn_farewell_beacons(&self) {
        // A locked vault is not online, whatever the app was doing a moment
        // ago — record that before the beacon, so nothing re-announces in the
        // window before the heartbeat task is aborted.
        self.presence_active
            .store(false, std::sync::atomic::Ordering::Relaxed);
        let Some(store) = self.ui.store_ref() else {
            return;
        };
        let peers = comrade_peers(store);
        spawn_presence_beacons(self.vault.clone(), peers, PresenceBeacon::offline());
    }

    /// Fire-and-forget a receipt DM (delivered/read) to `peer`.
    fn spawn_receipt(&self, peer: &PublicKey, kind: ReceiptKind, message_ids: Vec<String>) {
        if message_ids.is_empty() {
            return;
        }
        let Some(vault) = self.vault.clone() else {
            return;
        };
        let peer = *peer;
        tokio::spawn(async move {
            if let Ok(json) = Receipt::new(kind, message_ids).to_json() {
                let _ = vault.send_dm(&peer, &json).await;
            }
        });
    }

    // ── Calls (voice/video · WebRTC signalled over the DM channel) ────────────

    /// The configured TURN relay, if any (see [`Self::set_turn_server`]).
    fn configured_turn_server(&self) -> Option<IceServer> {
        let store = self.ui.store_ref()?;
        let turn = store
            .get::<TurnConfig>(SETTINGS_TREE, TURN_CONFIG_KEY)
            .ok()??;
        (!turn.url.trim().is_empty())
            .then(|| IceServer::turn(turn.url, turn.username, turn.credential))
    }

    /// The ICE servers to hand a frontend `RTCPeerConnection`: public STUN by
    /// default, plus a user-configured TURN relay if one has been set.
    ///
    /// This is the "give me everything" list; [`Self::call_ice_servers_for`]
    /// exposes the STUN-first, TURN-on-failure strategy new calls should use.
    pub fn call_ice_servers(&self) -> Vec<IceServerDto> {
        ice_servers_for(
            IceStrategy::StunAndTurn,
            self.configured_turn_server().as_ref(),
        )
        .into_iter()
        .map(IceServerDto::from)
        .collect()
    }

    /// The ICE servers for one connection attempt under `strategy`
    /// (`"stun_only"` or `"stun_and_turn"`, see [`comrade_core::call::IceStrategy`]).
    ///
    /// Every call should start with `"stun_only"` (what [`Self::place_call`]
    /// uses): STUN is free and blind to the call, unlike a TURN relay. If the
    /// frontend's `RTCPeerConnection` reports its ICE connection state never
    /// reaches `connected`/`completed` — the CGNAT case a TURN server exists
    /// for — it calls this again with `"stun_and_turn"` and restarts ICE with
    /// the widened server list, now actually routing through the configured
    /// relay.
    pub fn call_ice_servers_for(&self, strategy: &str) -> Vec<IceServerDto> {
        let strategy = IceStrategy::from_str_lenient(strategy);
        ice_servers_for(strategy, self.configured_turn_server().as_ref())
            .into_iter()
            .map(IceServerDto::from)
            .collect()
    }

    /// The 4-emoji short authentication string (SAS) for the in-progress call
    /// whose local and remote SDPs are `local_sdp`/`remote_sdp` — for the two
    /// participants to read aloud and compare, catching a man-in-the-middle
    /// that re-terminated the DTLS-SRTP media path. `None` when either side's
    /// SDP has no `a=fingerprint:` line to derive a code from — an honest
    /// "can't verify" rather than a fabricated one; the frontend should treat
    /// that the same as a user who never checked, not as a failure.
    ///
    /// Pure computation over the two SDP strings already in hand (no store,
    /// no network) — see [`comrade_core::call::derive_sas`] for the crypto.
    pub fn call_sas(&self, local_sdp: &str, remote_sdp: &str) -> Option<Vec<String>> {
        derive_sas(local_sdp, remote_sdp)
    }

    /// Configure (or, with a blank `url`, clear) the TURN relay used for calls
    /// that cannot connect over STUN alone. Persisted in the encrypted store.
    ///
    /// Rejects (without persisting or ever logging `username`/`credential`) a
    /// non-blank `url` that isn't a well-formed `turn:`/`turns:` URI — see
    /// [`validate_turn_url`] — so a copy-paste mistake fails loudly in the
    /// settings UI instead of silently producing a `RTCPeerConnection` that
    /// can never reach the relay.
    pub fn set_turn_server(
        &self,
        url: &str,
        username: &str,
        credential: &str,
    ) -> Result<(), UiError> {
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        if url.trim().is_empty() {
            store
                .delete(SETTINGS_TREE, TURN_CONFIG_KEY)
                .map_err(|e| UiError::Storage(e.to_string()))?;
        } else {
            validate_turn_url(url).map_err(UiError::Engine)?;
            let cfg = TurnConfig {
                url: url.trim().to_string(),
                username: username.to_string(),
                credential: credential.to_string(),
            };
            store
                .put(SETTINGS_TREE, TURN_CONFIG_KEY, &cfg)
                .map_err(|e| UiError::Storage(e.to_string()))?;
        }
        store.flush().map_err(|e| UiError::Storage(e.to_string()))
    }

    /// An honest "is a relay configured" status for a support/diagnostic
    /// screen — the URL (not secret) only, never the username/credential, so
    /// this is safe to log or display without masking. `None` when the vault
    /// is locked (rather than erroring): a settings screen showing "no relay
    /// configured" pre-unlock is more useful than a thrown exception.
    pub fn turn_server_status(&self) -> TurnServerStatusDto {
        match self.configured_turn_server() {
            Some(turn) => TurnServerStatusDto {
                configured: true,
                url: turn.urls.first().cloned(),
            },
            None => TurnServerStatusDto {
                configured: false,
                url: None,
            },
        }
    }

    // ── The player's own library ─────────────────────────────────────────────
    //
    // Favourites, history, playlists and the saved queue. Every method here is
    // a quick local read or write — no await, no network, no third party — so
    // holding the runtime lock for its duration is the same non-event it is
    // for the settings tree beside which these trees live. The vault rule is
    // the one shared gate: [`UiError::VaultLocked`] while locked, because
    // listening data is diary data.

    /// Every favourited track, unordered (the UI orders).
    ///
    /// Unordered on purpose: the store's key order is an implementation detail,
    /// and pretending it means something invites screens that break when the
    /// storage engine changes iteration order. A screen that wants recency has
    /// history for that.
    pub fn favourites_list(&self) -> Result<Vec<PlayerTrackDto>, UiError> {
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        store
            .values::<PlayerTrackDto>(PLAYER_FAVOURITES_TREE)
            .map_err(|e| UiError::Storage(e.to_string()))
    }

    /// Whether `key` is favourited — asked per row while drawing a list.
    pub fn favourite_is(&self, key: String) -> Result<bool, UiError> {
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        store
            .contains(PLAYER_FAVOURITES_TREE, &key)
            .map_err(|e| UiError::Storage(e.to_string()))
    }

    /// Toggle a favourite; answers what it now **is**, so a toggle button can
    /// render from the return value without a second call racing it.
    pub fn favourite_toggle(&self, track: PlayerTrackDto) -> Result<bool, UiError> {
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        let was = store
            .contains(PLAYER_FAVOURITES_TREE, &track.key)
            .map_err(|e| UiError::Storage(e.to_string()))?;
        let result = if was {
            store.delete(PLAYER_FAVOURITES_TREE, &track.key).map(|_| ())
        } else {
            store.put(PLAYER_FAVOURITES_TREE, &track.key, &track)
        };
        result
            .and_then(|()| store.flush())
            .map_err(|e| UiError::Storage(e.to_string()))?;
        Ok(!was)
    }

    /// Record that `track` was just played.
    ///
    /// One entry per track, timestamp updated in place, oldest evicted past
    /// [`HISTORY_MAX_ENTRIES`] — see [`prune_history`] for the whole rule. A
    /// failed write is swallowed rather than surfaced: playback must never
    /// error because its diary could not be written.
    pub fn history_record(&self, track: PlayerTrackDto, at_ms: u64) -> Result<(), UiError> {
        let Some(store) = self.ui.store_ref() else {
            return Ok(());
        };
        let key = track.key.clone();
        let _ = store.put(PLAYER_HISTORY_TREE, &key, &HistoryEntryDto { track, at_ms });
        if store.flush().is_err() {
            return Ok(());
        }
        self.history_prune_locked();
        Ok(())
    }

    /// Recently played, newest first.
    pub fn history_list(&self) -> Result<Vec<HistoryEntryDto>, UiError> {
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        let entries: Vec<HistoryEntryDto> = store
            .values(PLAYER_HISTORY_TREE)
            .map_err(|e| UiError::Storage(e.to_string()))?;
        Ok(prune_history(entries))
    }

    /// Forget everything recently played. A privacy action, not housekeeping —
    /// which is why it is one tap rather than a rolling window nobody controls.
    pub fn history_clear(&self) -> Result<(), UiError> {
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        for key in store
            .keys(PLAYER_HISTORY_TREE)
            .map_err(|e| UiError::Storage(e.to_string()))?
        {
            store
                .delete(PLAYER_HISTORY_TREE, &key)
                .map_err(|e| UiError::Storage(e.to_string()))?;
        }
        store.flush().map_err(|e| UiError::Storage(e.to_string()))
    }

    /// Evict past the cap. Called after writes; errors are swallowed by the
    /// same argument as [`Self::history_record`].
    fn history_prune_locked(&self) {
        let Some(store) = self.ui.store_ref() else {
            return;
        };
        let Ok(all) = store.values::<HistoryEntryDto>(PLAYER_HISTORY_TREE) else {
            return;
        };
        if all.len() <= HISTORY_MAX_ENTRIES {
            return;
        }
        // The surviving keys, per the shared rule; everything else is deleted.
        let keep: std::collections::HashSet<String> = prune_history(all)
            .into_iter()
            .map(|e| e.track.key)
            .collect();
        let Ok(keys) = store.keys(PLAYER_HISTORY_TREE) else {
            return;
        };
        for key in keys {
            if !keep.contains(&key) {
                let _ = store.delete(PLAYER_HISTORY_TREE, &key);
            }
        }
        let _ = store.flush();
    }

    /// Every playlist, each with its tracks in playlist order.
    pub fn playlists_list(&self) -> Result<Vec<PlaylistDto>, UiError> {
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        let mut lists: Vec<PlaylistDto> = store
            .values(PLAYER_PLAYLISTS_TREE)
            .map_err(|e| UiError::Storage(e.to_string()))?;
        lists.sort_by_key(|l| l.created_at_ms);
        Ok(lists)
    }

    /// Create an empty playlist, answering its id.
    pub fn playlist_create(&self, name: String, created_at_ms: u64) -> Result<String, UiError> {
        let name = name.trim().to_string();
        if name.is_empty() {
            return Err(UiError::Engine("a playlist needs a name".into()));
        }
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        let id = fresh_playlist_id();
        store
            .put(
                PLAYER_PLAYLISTS_TREE,
                &id,
                &PlaylistDto {
                    id: id.clone(),
                    name,
                    created_at_ms,
                    tracks: Vec::new(),
                },
            )
            .and_then(|()| store.flush())
            .map_err(|e| UiError::Storage(e.to_string()))?;
        Ok(id)
    }

    /// Delete a playlist. Its tracks are untouched elsewhere: deleting a list
    /// never deletes music.
    pub fn playlist_delete(&self, id: String) -> Result<(), UiError> {
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        store
            .delete(PLAYER_PLAYLISTS_TREE, &id)
            .and_then(|_| store.flush())
            .map_err(|e| UiError::Storage(e.to_string()))?;
        Ok(())
    }

    /// Append a track to a playlist. Duplicates are allowed and are the
    /// caller's business — a mixtape may say the same song twice.
    pub fn playlist_add_track(&self, id: String, track: PlayerTrackDto) -> Result<(), UiError> {
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        let mut list = store
            .get::<PlaylistDto>(PLAYER_PLAYLISTS_TREE, &id)
            .map_err(|e| UiError::Storage(e.to_string()))?
            .ok_or_else(|| UiError::Engine(format!("no playlist {id}")))?;
        list.tracks.push(track);
        store
            .put(PLAYER_PLAYLISTS_TREE, &id, &list)
            .and_then(|()| store.flush())
            .map_err(|e| UiError::Storage(e.to_string()))
    }

    /// Remove every copy of the track whose [key][PlayerTrackDto::key] is
    /// `track_key`.
    ///
    /// **Every copy, not one** — the key is the identity, so two rows carrying
    /// it are not distinguishable enough to remove between. Removing an absent
    /// track succeeds quietly: the requested end state ("this track is not in
    /// this list") already holds.
    pub fn playlist_remove_track(&self, id: String, track_key: String) -> Result<(), UiError> {
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        let mut list = store
            .get::<PlaylistDto>(PLAYER_PLAYLISTS_TREE, &id)
            .map_err(|e| UiError::Storage(e.to_string()))?
            .ok_or_else(|| UiError::Engine(format!("no playlist {id}")))?;
        list.tracks.retain(|t| t.key != track_key);
        store
            .put(PLAYER_PLAYLISTS_TREE, &id, &list)
            .and_then(|()| store.flush())
            .map_err(|e| UiError::Storage(e.to_string()))
    }

    /// Reorder a playlist: move the track at `from` so it sits at `to`.
    ///
    /// The clamping rule is [`reorder_tracks`]'s — an out-of-range index is a
    /// drag to the nearest end, not an error — so a drag gesture never has to
    /// second-guess its own arithmetic. `created_at_ms` is untouched:
    /// arranging a list is not creating it, and it must not jump the shelf.
    pub fn playlist_reorder(&self, id: String, from: u32, to: u32) -> Result<(), UiError> {
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        let mut list = store
            .get::<PlaylistDto>(PLAYER_PLAYLISTS_TREE, &id)
            .map_err(|e| UiError::Storage(e.to_string()))?
            .ok_or_else(|| UiError::Engine(format!("no playlist {id}")))?;
        list.tracks = reorder_tracks(list.tracks, from as usize, to as usize);
        store
            .put(PLAYER_PLAYLISTS_TREE, &id, &list)
            .and_then(|()| store.flush())
            .map_err(|e| UiError::Storage(e.to_string()))
    }

    /// Rename a playlist. Its tracks and `created_at_ms` are untouched, so a
    /// rename never reorders [`Self::playlists_list`]. An empty name is refused
    /// exactly as it is at [creation][Self::playlist_create] — a nameless
    /// playlist is not a thing this store invents.
    pub fn playlist_rename(&self, id: String, name: String) -> Result<(), UiError> {
        let name = name.trim().to_string();
        if name.is_empty() {
            return Err(UiError::Engine("a playlist needs a name".into()));
        }
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        let mut list = store
            .get::<PlaylistDto>(PLAYER_PLAYLISTS_TREE, &id)
            .map_err(|e| UiError::Storage(e.to_string()))?
            .ok_or_else(|| UiError::Engine(format!("no playlist {id}")))?;
        list.name = name;
        store
            .put(PLAYER_PLAYLISTS_TREE, &id, &list)
            .and_then(|()| store.flush())
            .map_err(|e| UiError::Storage(e.to_string()))
    }

    /// Save the live queue over any previous snapshot.
    pub fn queue_save(&self, queue: SavedQueueDto) -> Result<(), UiError> {
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        store
            .put(PLAYER_QUEUE_TREE, PLAYER_QUEUE_KEY, &queue)
            .and_then(|()| store.flush())
            .map_err(|e| UiError::Storage(e.to_string()))
    }

    /// The saved queue, if any.
    pub fn queue_load(&self) -> Result<Option<SavedQueueDto>, UiError> {
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        store
            .get(PLAYER_QUEUE_TREE, PLAYER_QUEUE_KEY)
            .map_err(|e| UiError::Storage(e.to_string()))
    }

    /// Forget the saved queue — "start fresh" is a choice worth making.
    pub fn queue_clear(&self) -> Result<(), UiError> {
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        store
            .delete(PLAYER_QUEUE_TREE, PLAYER_QUEUE_KEY)
            .map(|_| ())
            .and_then(|()| store.flush())
            .map_err(|e| UiError::Storage(e.to_string()))
    }

    /// Begin a call to `peer`: mint a call id and return the session the
    /// frontend needs (id, peer, media kind, ICE servers). No signal is sent
    /// yet — the frontend creates the WebRTC offer, then calls
    /// [`Self::send_call_signal`]. `media` is `"audio"` or `"video"`.
    ///
    /// `ice_servers` starts STUN-only (see [`Self::call_ice_servers_for`]) —
    /// if the connection can't complete, the frontend retries with
    /// `call_ice_servers_for("stun_and_turn")` before falling back to a
    /// `HangupReason::Failed`.
    pub fn place_call(&self, peer: &str, media: &str) -> Result<CallSessionDto, UiError> {
        if self.vault.is_none() {
            return Err(UiError::VaultLocked);
        }
        let _ = parse_pubkey(peer)?; // validate the target up front
        Ok(CallSessionDto {
            call_id: new_call_id(),
            peer: to_npub(peer),
            media: CallMediaKind::from_str_lenient(media).as_str().to_string(),
            ice_servers: self.call_ice_servers_for(IceStrategy::StunOnly.as_str()),
        })
    }

    /// Send one call-signaling payload to `peer` over the encrypted DM channel.
    /// `signal_json` is a serialised [`comrade_core::call::CallSignal`], e.g.
    /// `{"kind":"offer","sdp":"…"}` or `{"kind":"ice","candidate":"…"}`.
    ///
    /// Delegates to [`RuntimeHandles::send_call_signal`] — see [`Self::send_dm`].
    pub async fn send_call_signal(
        &self,
        peer: &str,
        call_id: &str,
        media: &str,
        signal_json: &str,
    ) -> Result<(), UiError> {
        self.handles()
            .send_call_signal(peer, call_id, media, signal_json)
            .await
    }

    /// Invite `peer` to watch or listen to something together.
    /// Delegates to [`RuntimeHandles::together_start`] — see [`Self::send_dm`].
    pub async fn together_start(
        &self,
        peer: &str,
        content: TogetherContent,
    ) -> Result<TogetherSessionDto, UiError> {
        self.handles().together_start(peer, content).await
    }

    /// Accept an invitation. Delegates to [`RuntimeHandles::together_join`].
    pub async fn together_join(&self) -> Result<(), UiError> {
        self.handles().together_join().await
    }

    /// Play, pause or seek. Delegates to [`RuntimeHandles::together_set_state`].
    pub async fn together_set_state(
        &self,
        pos_ms: u64,
        playing: bool,
        effective_in_ms: u64,
    ) -> Result<(), UiError> {
        self.handles()
            .together_set_state(pos_ms, playing, effective_in_ms)
            .await
    }

    /// Leave the session. Delegates to [`RuntimeHandles::together_end`].
    pub async fn together_end(&self) -> Result<(), UiError> {
        self.handles().together_end().await
    }

    /// Tell the runtime where our own player is, without sending anything.
    ///
    /// Synchronous and non-blocking by design: a player calls this several times
    /// a second from its UI thread, and it must never wait behind a vault
    /// unlock. It is also *not* a command — it changes nothing anyone else sees;
    /// it only gives the next drift verdict something true to compare against,
    /// so skipping it under contention fails in the harmless direction (the same
    /// trade [`Self::note_draft`] makes).
    /// `output_latency_ms` is how far behind `pos_ms` the sound actually leaves
    /// this device's speaker — `AudioTrack.getTimestamp` on Android, or zero
    /// from a player that cannot ask. Zero is honest ("unmeasured"), and costs
    /// only the accuracy it cannot supply: without it two devices can agree
    /// perfectly on decoder position and still be a tenth of a second apart in
    /// the room, which is the error no browser-based implementation can even
    /// see.
    pub fn together_report_position(&self, pos_ms: u64, playing: bool, output_latency_ms: u64) {
        if let Ok(mut guard) = self.together.try_lock() {
            if let Some(session) = guard.as_mut() {
                session.local_pos_ms = pos_ms;
                session.local_playing = playing;
                session.local_output_latency_ms = output_latency_ms;
            }
        }
    }

    /// The live session, if there is one.
    pub fn together_session(&self) -> Option<TogetherSessionDto> {
        self.together.lock().unwrap().as_ref().map(|s| s.dto())
    }

    /// Tell the runtime a direct peer channel is up (or has gone), so signals
    /// take it instead of a relay. See
    /// [`RuntimeHandles::together_direct_ready`].
    ///
    /// Shares `self.together` with the handles twin rather than delegating, for
    /// the same reason [`Self::together_report_position`] does: it is a
    /// lock-and-set with nothing to await, and routing it through the handles
    /// would put a `RwLock` in front of a call a frontend makes from a
    /// connection callback.
    pub fn together_direct_ready(&self, ready: bool) {
        if let Some(session) = self.together.lock().unwrap().as_mut() {
            session.direct_ready = ready;
            if ready {
                session.direct_evidence_ms = now_ms();
            }
        }
    }

    /// Hand the runtime an envelope that arrived over the direct peer channel.
    ///
    /// See [`RuntimeHandles::together_receive_direct`] for why this path is
    /// deliberately less privileged than the relay one.
    pub fn together_receive_direct(&self, json: &str) {
        self.handles().together_receive_direct(json);
    }

    /// Send one step of the file handover.
    /// Delegates to [`RuntimeHandles::together_share`] — see [`Self::send_dm`].
    pub async fn together_share(&self, signal: ShareSignal) -> Result<(), UiError> {
        self.handles().together_share(signal).await
    }

    // ── Transfer policy ─────────────────────────────────────────────────────
    //
    // Pure and vault-free, like `call_sas`: no lock is taken beyond the policy
    // cell itself and nothing here touches the network, so a frontend may ask
    // these questions from inside a WebRTC callback without the deadlock that
    // shape has already caused twice in this repo.

    /// What this device currently does when the only path is a relay.
    pub fn share_relay_policy(&self) -> RelayPolicy {
        *self.share_policy.lock().unwrap()
    }

    /// Change it, and remember it. The next transfer connection is built under
    /// the new policy; one already running is not renegotiated, because tearing
    /// down a transfer someone is watching from is a worse answer than letting
    /// it finish under the rules it started with.
    ///
    /// The cell is updated even when the vault is locked, so a frontend can set
    /// a policy for this session without one; it just will not survive the
    /// process. Persistence failures are reported rather than swallowed —
    /// silently forgetting a choice about someone else's bandwidth is the kind
    /// of quiet default this codebase does not do.
    pub fn set_share_relay_policy(&self, policy: RelayPolicy) -> Result<(), UiError> {
        *self.share_policy.lock().unwrap() = policy;
        let Some(store) = self.ui.store_ref() else {
            return Err(UiError::VaultLocked);
        };
        let prefs = relay_policy_to_prefs(policy);
        store
            .save_share_prefs(&prefs)
            .and_then(|()| store.flush())
            .map_err(|e| UiError::Storage(e.to_string()))
    }

    /// Whether a transfer connection may be handed TURN servers at all.
    ///
    /// The *structural* half of the enforcement, and the half that holds even if
    /// every later check were deleted: under the default policy the transfer
    /// connection is configured with STUN only, so a relay candidate is never
    /// gathered and there is no relayed path to detect.
    pub fn share_ice_servers_allowed(&self) -> bool {
        share_transport::ice_servers_allowed(self.share_relay_policy())
    }

    /// Judge the path ICE actually chose, given the candidate types on the
    /// selected pair, and say whether this transfer may run over it.
    ///
    /// The two strings come straight from an `RTCStatsReport` and are peer- and
    /// browser-supplied, so classification is lenient about case and spacing and
    /// anything it does not recognise becomes
    /// [`IcePathKind::Unknown`] — which is *refused*, never waved through.
    ///
    /// `consent_granted` is the answer to a question a *previous* call asked by
    /// returning `needs_consent`; it can only ever turn that into `allow`, never
    /// move a refusal — see [`share_transport::decide_with_consent`] for why
    /// that asymmetry is load-bearing.
    pub fn share_transfer_verdict(
        &self,
        local_candidate_type: &str,
        remote_candidate_type: &str,
        total_bytes: u64,
        consent_granted: bool,
    ) -> ShareVerdictDto {
        let path = IcePathKind::classify(local_candidate_type, remote_candidate_type);
        let verdict = share_transport::decide_with_consent(
            path,
            total_bytes,
            self.share_relay_policy(),
            consent_granted,
        );
        ShareVerdictDto {
            verdict: match verdict {
                TransferVerdict::Allow => "allow",
                TransferVerdict::NeedsConsent { .. } => "needs_consent",
                TransferVerdict::Refuse { .. } => "refuse",
            }
            .to_string(),
            path: match path {
                IcePathKind::Host => "host",
                IcePathKind::ServerReflexive => "srflx",
                IcePathKind::Relay => "relay",
                IcePathKind::Unknown => "unknown",
            }
            .to_string(),
            reason: match verdict {
                TransferVerdict::Refuse { reason } => Some(reason),
                _ => None,
            },
            relayed_bytes: match verdict {
                TransferVerdict::NeedsConsent { relayed_bytes } => Some(relayed_bytes),
                _ => None,
            },
        }
    }

    /// How many chunks may be pushed into a data channel currently holding
    /// `buffered_bytes`. Zero means stop and wait for the drain event.
    ///
    /// Exposed so a frontend without a tested twin of the arithmetic can ask
    /// rather than guess — the desktop has `share_transfer.mjs` because it needs
    /// the answer inside a synchronous event handler, but nothing else should
    /// have to re-derive it.
    pub fn share_chunks_to_send(&self, buffered_bytes: u64) -> u32 {
        share_transport::chunks_to_send(
            buffered_bytes,
            comrade_core::share::SHARE_CHUNK_BYTES,
            share_transport::SHARE_BUFFER_HIGH_WATER,
        )
    }

    /// What a player reading a file that is still arriving should do at the
    /// playhead it is at: start, keep going, or hold for more bytes.
    ///
    /// The two numbers come from the caller's own tracker —
    /// [`ShareTracker::runway_ms`](comrade_core::share::ShareTracker::runway_ms)
    /// and
    /// [`tail_complete_at`](comrade_core::share::ShareTracker::tail_complete_at),
    /// or the desktop's JS twin of them — because the bytes live in the frontend
    /// and the runtime keeps **no** transfer state ([`TogetherShareDto`] says
    /// why: two state machines that have to agree about a connection only one of
    /// them can see is the shape of both call bugs this repo has already fixed).
    /// What the frontend does *not* get to own is the thresholds, and that is
    /// what this call is: which bitmap has arrived is a fact about the frontend,
    /// when it is enough to play is policy, and policy lives here.
    ///
    /// So this is neither async nor a `try_lock` skip like
    /// [`Self::together_report_position`]. That one is skippable because it
    /// *writes* session state and a dropped write only costs the next drift
    /// verdict some accuracy; this one takes no lock at all, writes nothing, and
    /// is a pure function of its arguments — a dropped answer would mean a
    /// player with no instruction. Safe to call from a `MediaDataSource.readAt`
    /// or a `Range` handler, which is where it is needed and where anything that
    /// could block would deadlock.
    ///
    /// Acting on [`ReadVerdict::Hold`] means pausing the **local** player and
    /// nothing else — never `together_set_state(.., playing: false, ..)`, which
    /// is a command that pauses the other person. `docs/TOGETHER.md` §10, and
    /// the variant's own documentation.
    pub fn share_read_verdict(
        &self,
        runway_ms: u64,
        tail_complete: bool,
        playing: bool,
    ) -> ReadVerdict {
        share_read_verdict(&ReadSample {
            playing,
            runway_ms,
            tail_complete,
        })
    }

    /// Convenience: send a `Hangup` signal with `reason` (`normal`, `declined`,
    /// `busy`, `missed`, `cancelled`, `failed`) to end/reject a call.
    ///
    /// Delegates to [`RuntimeHandles::hangup_call`] — see [`Self::send_dm`].
    pub async fn hangup_call(
        &self,
        peer: &str,
        call_id: &str,
        media: &str,
        reason: &str,
    ) -> Result<(), UiError> {
        self.handles()
            .hangup_call(peer, call_id, media, reason)
            .await
    }

    /// Persist a finished call to the call log. `outcome` is one of
    /// `connected` / `missed` / `declined` / `cancelled` / `busy` / `failed`.
    #[allow(clippy::too_many_arguments)]
    pub fn log_call(
        &self,
        peer: &str,
        call_id: &str,
        media: &str,
        incoming: bool,
        outcome: &str,
        started_at: u64,
        duration_secs: u64,
    ) -> Result<CallRecordDto, UiError> {
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        let record = comrade_storage::CallRecord {
            id: call_id.to_string(),
            peer_npub: to_npub(peer),
            media: CallMediaKind::from_str_lenient(media).as_str().to_string(),
            incoming,
            outcome: outcome.to_string(),
            started_at: if started_at == 0 {
                now_secs()
            } else {
                started_at
            },
            duration_secs,
        };
        store
            .save_call_record(&record)
            .and_then(|()| store.flush())
            .map_err(|e| UiError::Storage(e.to_string()))?;
        Ok(record.into())
    }

    /// The call log, newest first — for a single `peer` (npub/hex) or, with
    /// `None`, across every peer.
    pub fn call_history(&self, peer: Option<&str>) -> Result<Vec<CallRecordDto>, UiError> {
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        let calls = match peer {
            Some(p) => store
                .calls_with(&to_npub(p))
                .map_err(|e| UiError::Storage(e.to_string()))?,
            None => store
                .all_calls()
                .map_err(|e| UiError::Storage(e.to_string()))?,
        };
        Ok(calls.into_iter().map(CallRecordDto::from).collect())
    }

    // ── Profile & contacts (username = alias, identity = keypair) ────────────

    /// The local profile: npub plus the chosen @handle (if set).
    pub fn profile(&self) -> Result<ProfileDto, UiError> {
        let id = self.ui.current_identity().ok_or(UiError::NoIdentity)?;
        // The bio and picture live in the store, so they read as `None` while the
        // vault is locked — the same way this method already tolerates having no
        // store at all rather than failing.
        let own = self
            .ui
            .store_ref()
            .and_then(|store| cached_peer_profile(store, &id.npub));
        Ok(ProfileDto {
            npub: id.npub,
            username: self.ui.username(),
            about: self.ui.store_ref().and_then(stored_about),
            picture: own.as_ref().and_then(|r| r.picture.clone()),
            avatar_cached: own.is_some_and(|r| r.avatar_sha256.is_some()),
        })
    }

    /// Everything a profile page draws for one peer, from the local cache alone.
    ///
    /// No relay round trip, so it works offline and returns instantly; a caller
    /// that wants fresher data runs [`Self::refresh_peer_profiles`] and reads
    /// again. Accepts an npub or hex key, and resolves to the canonical npub the
    /// contact is stored under.
    pub fn peer_profile(&self, npub: &str) -> Result<PeerProfileDto, UiError> {
        let canonical = self.canonical_contact_npub(npub)?;
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        let contact = store
            .get_contact(&canonical)
            .map_err(|e| UiError::Storage(e.to_string()))?;
        let record = cached_peer_profile(store, &canonical).unwrap_or_default();
        let presence = store
            .get_peer_presence(&canonical)
            .map_err(|e| UiError::Storage(e.to_string()))?;
        let now = now_secs();
        let comrade = contact.as_ref().is_some_and(|c| c.comrade);
        let blocked = store
            .get_conversation_meta(&canonical)
            .ok()
            .flatten()
            .is_some_and(|m| m.state == STATE_BLOCKED);
        Ok(PeerProfileDto {
            alias: contact
                .as_ref()
                .and_then(|c| user_alias(&c.petname, &canonical))
                .unwrap_or_default(),
            name: record.name,
            about: record.about,
            picture: record.picture,
            nip05: record.nip05,
            lud16: record.lud16,
            avatar_cached: record.avatar_sha256.is_some(),
            contact: contact.is_some(),
            comrade,
            blocked,
            // Presence only flows between comrades, so a non-comrade is never
            // "online" however recent their last beacon looks.
            online: comrade
                && presence
                    .as_ref()
                    .is_some_and(|p| p.online && is_online_at(p.expires_at, now)),
            last_seen_at: presence.as_ref().map_or(0, |p| p.last_seen_at),
            peer_marked_us: presence.as_ref().is_some_and(|p| p.peer_marked_us),
            updated_at: record.updated_at,
            npub: canonical,
        })
    }

    /// A peer's cached avatar bytes, base64-encoded, or `None` to draw initials.
    ///
    /// Reads the encrypted store and never the network — so this is safe to call
    /// while rendering, and calling it can never disclose anything to anyone. The
    /// fetch that fills the cache is a separate, gated decision.
    pub fn peer_avatar(&self, npub: &str) -> Result<Option<MediaBytesDto>, UiError> {
        let canonical = self.canonical_contact_npub(npub)?;
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        let Some(record) = cached_peer_profile(store, &canonical) else {
            return Ok(None);
        };
        let (Some(sha), Some(mime)) = (record.avatar_sha256, record.avatar_mime) else {
            return Ok(None);
        };
        let Some(bytes) = store
            .get_bytes(PEER_AVATAR_BLOBS_TREE, &sha)
            .map_err(|e| UiError::Storage(e.to_string()))?
        else {
            // The record points at bytes that are gone. Initials, not an error:
            // there is nothing the user could do about it and nothing is broken.
            return Ok(None);
        };
        Ok(Some(MediaBytesDto {
            mime_type: mime,
            base64: B64.encode(bytes),
        }))
    }

    /// Whether peer-published pictures may be fetched at all.
    ///
    /// Default **on**, which is a deliberate trade the owner made explicitly: a
    /// profile page whose avatars are all initials until someone finds a setting
    /// is a worse product, and the fetch is narrowed instead — accepted contacts
    /// only, and every guard in [`comrade_core::avatar`]. The switch exists
    /// because the cost is a real one and belongs to the user.
    pub fn remote_avatars_enabled(&self) -> Result<bool, UiError> {
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        Ok(store
            .get::<bool>(SETTINGS_TREE, REMOTE_AVATARS_KEY)
            .map_err(|e| UiError::Storage(e.to_string()))?
            .unwrap_or(true))
    }

    /// Turn peer-published picture fetching on or off.
    pub fn set_remote_avatars_enabled(&self, on: bool) -> Result<(), UiError> {
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        store
            .put(SETTINGS_TREE, REMOTE_AVATARS_KEY, &on)
            .and_then(|()| store.flush())
            .map_err(|e| UiError::Storage(e.to_string()))
    }

    /// Set (or clear, with an empty string) this identity's bio, and republish.
    ///
    /// Stripped of control characters and bounded — our own text should not carry
    /// a newline that would forge a second line on somebody else's profile page
    /// either. Persisted locally first, so an offline edit sticks and republishes
    /// on the next launch, which is how the handle already behaves.
    pub async fn set_about(&mut self, about: &str) -> Result<ProfileDto, UiError> {
        let cleaned = sanitise_untrusted_text(about, MAX_ABOUT_LEN);
        {
            let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
            if cleaned.is_empty() {
                let _ = store.delete(SETTINGS_TREE, PROFILE_ABOUT_KEY);
            } else {
                store
                    .put(SETTINGS_TREE, PROFILE_ABOUT_KEY, &cleaned)
                    .map_err(|e| UiError::Storage(e.to_string()))?;
            }
            store.flush().map_err(|e| UiError::Storage(e.to_string()))?;
        }
        // Republish so the change reaches the network. An empty bio publishes as
        // `Clear`, which is the case `Option<&str>` could not express at all.
        if let (Some(sabha), Some(handle)) = (self.sabha.clone(), self.ui.username()) {
            let edit = if cleaned.is_empty() {
                OwnedMetadataEdit::Clear
            } else {
                OwnedMetadataEdit::Set(cleaned)
            };
            self.spawn_profile_publish_with(sabha, handle, edit);
        }
        self.profile()
    }

    /// Claim a display handle for this identity.
    ///
    /// Trust model — why this cannot be globally unique: Comrade has no central
    /// registry, so nothing can stop a second keypair from publishing the same
    /// handle. The unforgeable identifier is the keypair (npub); the handle is
    /// a discovery alias published as Kind-0 metadata. Contacts pin the npub on
    /// first use, so a later "@same_handle" under a different key shows up as a
    /// different person and can never read or receive this thread's messages.
    ///
    /// The handle is persisted locally first; relay publication happens in a
    /// background task with retries (and again on every launch), so an offline
    /// claim still succeeds and becomes discoverable once a relay is reachable.
    pub async fn set_username(&mut self, handle: &str) -> Result<ProfileDto, UiError> {
        let handle = normalize_handle(handle)?;
        self.ui.set_username(handle.clone())?;
        if let Some(sabha) = self.sabha.clone() {
            // Never block (or fail) the claim on network state — but do keep
            // trying: a single dropped publish is exactly how a fresh identity
            // ends up unfindable by everyone else. Replaces (aborts) any
            // earlier retry loop so a stale name can't win the publish race.
            self.spawn_profile_publish(sabha, handle.clone());
        }
        self.profile()
    }

    /// Canonicalise a contact key: vault must be open, key must parse. One
    /// rule for every contact method, so junk input behaves identically
    /// across add/alias/remove on every bridge.
    fn canonical_contact_npub(&self, npub: &str) -> Result<String, UiError> {
        if self.ui.store_ref().is_none() {
            return Err(UiError::VaultLocked);
        }
        parse_pubkey(npub)?
            .to_bech32()
            .map_err(|e| UiError::Engine(e.to_string()))
    }

    /// Save a contact, pinned by npub — trust on first use. An empty `alias`
    /// leaves any existing alias untouched (so opening a chat with a known
    /// contact never wipes the name the user gave them); a non-empty alias
    /// sets it. Use [`Self::set_contact_alias`] to explicitly clear one.
    pub fn add_contact(&self, npub: &str, alias: &str) -> Result<ContactDto, UiError> {
        let canonical = self.canonical_contact_npub(npub)?;
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        let alias = alias.trim();
        let existing = store
            .get_contact(&canonical)
            .map_err(|e| UiError::Storage(e.to_string()))?;
        if alias.is_empty() {
            if let Some(contact) = existing {
                // Already pinned and nothing to change — don't rewrite the
                // record (this path runs on every chat open).
                return Ok(ContactDto {
                    name: cached_peer_name(store, &contact.npub),
                    alias: user_alias(&contact.petname, &contact.npub).unwrap_or_default(),
                    npub: contact.npub,
                    comrade: contact.comrade,
                });
            }
        }
        self.write_contact(canonical, alias.to_string())
    }

    /// Set (non-empty) or clear (empty) the user-chosen alias for a contact.
    /// Creates the contact if it doesn't exist yet.
    pub fn set_contact_alias(&self, npub: &str, alias: &str) -> Result<ContactDto, UiError> {
        let canonical = self.canonical_contact_npub(npub)?;
        self.write_contact(canonical, alias.trim().to_string())
    }

    fn write_contact(&self, npub: String, petname: String) -> Result<ContactDto, UiError> {
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        // Carry the record's existing state forward: editing an alias must
        // never silently un-choose a comrade (presence is opt-in *and*
        // opt-out — both only ever by an explicit act).
        let existing = store
            .get_contact(&npub)
            .map_err(|e| UiError::Storage(e.to_string()))?;
        let contact = comrade_storage::Contact {
            npub,
            petname,
            relays: existing
                .as_ref()
                .map(|c| c.relays.clone())
                .unwrap_or_default(),
            comrade: existing.is_some_and(|c| c.comrade),
        };
        store
            .upsert_contact(&contact)
            .and_then(|()| store.flush())
            .map_err(|e| UiError::Storage(e.to_string()))?;
        Ok(ContactDto {
            name: cached_peer_name(store, &contact.npub),
            npub: contact.npub,
            alias: contact.petname,
            comrade: contact.comrade,
        })
    }

    /// Remove a saved contact. Returns whether one existed. The message
    /// history with that peer is untouched — only the pin/alias goes.
    pub fn remove_contact(&self, npub: &str) -> Result<bool, UiError> {
        let canonical = self.canonical_contact_npub(npub)?;
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        let removed = store
            .remove_contact(&canonical)
            .map_err(|e| UiError::Storage(e.to_string()))?;
        store.flush().map_err(|e| UiError::Storage(e.to_string()))?;
        Ok(removed)
    }

    /// All saved contacts, sorted by their display title (alias, else
    /// published name, else key).
    pub fn list_contacts(&self) -> Result<Vec<ContactDto>, UiError> {
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        let mut contacts: Vec<ContactDto> = store
            .list_contacts()
            .map_err(|e| UiError::Storage(e.to_string()))?
            .into_iter()
            .map(|c| ContactDto {
                name: cached_peer_name(store, &c.npub),
                alias: user_alias(&c.petname, &c.npub).unwrap_or_default(),
                comrade: c.comrade,
                npub: c.npub,
            })
            .collect();
        contacts.sort_by_key(|c| {
            if !c.alias.is_empty() {
                c.alias.to_lowercase()
            } else {
                c.name.as_deref().unwrap_or(c.npub.as_str()).to_lowercase()
            }
        });
        Ok(contacts)
    }

    /// Best-effort people search by handle over NIP-50-capable relays. An empty
    /// result means no search relay knew the name — offer add-by-npub instead.
    ///
    /// A query that *is* a key (npub/hex) resolves that identity's profile
    /// directly instead of a name search. Every result is cached into the
    /// local profile store so the chat UI can name the peer immediately.
    ///
    /// Delegates to [`RuntimeHandles::search_profiles`] — see [`Self::send_dm`].
    pub async fn search_profiles(&self, query: &str) -> Result<Vec<FoundProfileDto>, UiError> {
        self.handles().search_profiles(query).await
    }

    /// Detach a [`ProfileRefresher`] holding only the engine/store handles.
    ///
    /// The refresh does slow network work; callers behind the shared
    /// `Arc<RwLock<ComradeRuntime>>` (JNI, Tauri) MUST take this under a
    /// briefly-held guard, **drop the guard**, and then await
    /// [`ProfileRefresher::run`] — holding the runtime lock across relay
    /// round-trips stalls every other bridge call (AUDIT P2 discipline:
    /// no guard held across network awaits).
    pub fn profile_refresher(&self) -> Result<ProfileRefresher, UiError> {
        Ok(ProfileRefresher {
            sabha: self.sabha.clone().ok_or(UiError::VaultLocked)?,
            store: self.ui.store_arc().ok_or(UiError::VaultLocked)?,
        })
    }

    /// Convenience wrapper over [`Self::profile_refresher`] for callers that
    /// own the runtime directly (tests, CLI). Bridge code should use the
    /// refresher so the shared lock is not held across the network work.
    pub async fn refresh_peer_profiles(&self) -> Result<usize, UiError> {
        self.profile_refresher()?.run().await
    }

    // ── Comrades (chosen-peer presence — see `comrade_core::presence`) ───────

    /// Choose (or un-choose) a contact as a **comrade**.
    ///
    /// What this actually does, stated plainly because it is a disclosure:
    /// from now on this device tells *that one peer* — nobody else, and no
    /// relay — when it is online, and it starts believing what they say about
    /// their own presence. Turning it off sends them a final "offline" so
    /// their view of us goes dark immediately instead of aging out.
    ///
    /// It does **not** subscribe us to their presence: nothing in a
    /// serverless design can make their device report to us. We see them
    /// online only once they have marked us too — [`ComradeDto::peer_marked_us`]
    /// is what a UI shows so that wait is explained rather than mysterious.
    ///
    /// Marking a peer we have never saved creates the contact record (you can
    /// make someone a comrade straight from their conversation). Idempotent.
    ///
    /// Must be called from within a Tokio runtime context — the beacon is
    /// sent by a spawned task so the caller is never blocked on the network.
    pub fn set_comrade(&self, npub: &str, comrade: bool) -> Result<ContactDto, UiError> {
        let canonical = self.canonical_contact_npub(npub)?;
        let peer = parse_pubkey(&canonical)?;
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        let contact = store
            .set_contact_comrade(&canonical, comrade)
            .and_then(|c| store.flush().map(|()| c))
            .map_err(|e| UiError::Storage(e.to_string()))?;

        // Tell them straight away, either way: a new comrade shouldn't wait a
        // heartbeat to see us, and a dropped one shouldn't keep seeing us.
        // A beacon either way is also what proves reciprocity to them, so
        // choosing someone while the app isn't in the foreground still tells
        // them we chose them — it just doesn't claim we're at the phone.
        let beacon = if comrade
            && self
                .presence_active
                .load(std::sync::atomic::Ordering::Relaxed)
        {
            PresenceBeacon::online()
        } else {
            PresenceBeacon::offline()
        };
        spawn_presence_beacons(self.vault.clone(), vec![peer], beacon);

        // If they had already chosen us, a live beacon of theirs may already
        // be on file — recorded silently at the time, because presence for
        // someone we hadn't chosen is not news. Choosing them is the moment
        // it becomes news, so surface it now rather than leaving the user
        // waiting on a "transition" that already happened.
        if comrade && peer_is_online(store, &contact.npub, now_secs()) {
            let at = store
                .get_peer_presence(&contact.npub)
                .ok()
                .flatten()
                .map(|p| p.last_seen_at)
                .unwrap_or_else(now_secs);
            let _ = self.events.send(BridgeEvent::ComradePresence {
                name: presence_display_name(store, &contact.npub),
                peer: contact.npub.clone(),
                online: true,
                at,
            });
        }

        Ok(ContactDto {
            name: cached_peer_name(store, &contact.npub),
            alias: user_alias(&contact.petname, &contact.npub).unwrap_or_default(),
            npub: contact.npub,
            comrade: contact.comrade,
        })
    }

    /// Every comrade with their live presence, online first, then by most
    /// recently seen. Empty until the user marks someone — the feature costs
    /// nothing (no beacons, no state) while nobody is chosen.
    pub fn comrades(&self) -> Result<Vec<ComradeDto>, UiError> {
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        let now = now_secs();
        let mut list: Vec<ComradeDto> = store
            .list_comrades()
            .map_err(|e| UiError::Storage(e.to_string()))?
            .into_iter()
            .map(|c| {
                let presence = store.get_peer_presence(&c.npub).ok().flatten();
                ComradeDto {
                    name: cached_peer_name(store, &c.npub),
                    alias: user_alias(&c.petname, &c.npub).unwrap_or_default(),
                    online: presence
                        .as_ref()
                        .is_some_and(|p| p.online && is_online_at(p.expires_at, now)),
                    last_seen_at: presence.as_ref().map(|p| p.last_seen_at).unwrap_or(0),
                    peer_marked_us: presence.as_ref().is_some_and(|p| p.peer_marked_us),
                    npub: c.npub,
                }
            })
            .collect();
        list.sort_by(|a, b| {
            b.online
                .cmp(&a.online)
                .then_with(|| b.last_seen_at.cmp(&a.last_seen_at))
                .then_with(|| a.npub.cmp(&b.npub))
        });
        Ok(list)
    }

    /// A single peer's live presence, or `None` if no beacon has ever arrived
    /// from them (they haven't marked us, or haven't been online since).
    pub fn peer_presence(&self, npub: &str) -> Result<Option<PresenceDto>, UiError> {
        let canonical = self.canonical_contact_npub(npub)?;
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        let now = now_secs();
        Ok(store
            .get_peer_presence(&canonical)
            .map_err(|e| UiError::Storage(e.to_string()))?
            .map(|p| PresenceDto {
                online: p.online && is_online_at(p.expires_at, now),
                last_seen_at: p.last_seen_at,
                peer_marked_us: p.peer_marked_us,
                peer: p.peer_npub,
            }))
    }

    /// Announce this device's presence to every comrade and return how many
    /// beacons were sent. Frontends call this on foreground/background
    /// transitions; the heartbeat in [`Self::spawn_event_loops`] keeps it
    /// fresh in between.
    ///
    /// Delegates to [`RuntimeHandles::announce_presence`] — see [`Self::send_dm`]
    /// for why the network half never runs under the runtime lock.
    pub async fn announce_presence(&self, online: bool) -> u64 {
        self.handles().announce_presence(online).await
    }

    /// Whether this device currently counts as online — see
    /// [`Self::presence_active`] (the field) for what that means.
    pub fn is_presence_active(&self) -> bool {
        self.presence_active
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    // ── The nudge (abandoned drafts — see `comrade_core::nudge`) ─────────────

    /// There is unsent text in `peer`'s composer, as of now.
    ///
    /// Frontends call this as the user types — it is idempotent, holds a
    /// mutex for a hash lookup, touches no store and no relay, so it is safe
    /// on a keystroke. What it starts is a clock, not a disclosure: nothing
    /// leaves the device unless the draft is later abandoned *and* every rule
    /// in [`comrade_core::nudge::draft_verdict`] agrees, and nothing at all
    /// happens for a peer who was never marked as a comrade.
    ///
    /// Infallible on purpose, like [`Self::announce_presence`]: a nudge is a
    /// courtesy, and a locked vault or an unparseable key simply means there
    /// is nobody to be courteous to.
    pub fn note_draft(&self, peer: &str) {
        self.nudge_watch.writing(&to_npub(peer), now_secs());
    }

    /// `peer`'s draft is gone — the box was emptied, or the thread was closed
    /// with it still unsent. Safe (and intended) to call unconditionally when a
    /// conversation closes: a composer that never held text does nothing here.
    ///
    /// This is the only trigger for the whole feature, and it is deliberately
    /// *not* the moment anything is sent: see
    /// [`RuntimeHandles::nudge_abandoned_drafts`] for the wait that follows.
    pub fn abandon_draft(&self, peer: &str) {
        self.nudge_watch.abandoned(&to_npub(peer), now_secs());
    }

    /// Tell every comrade, once, that this person might need them — for
    /// someone deliberately reaching for a pause rather than giving up on a
    /// message. Returns how many nudges a relay accepted.
    ///
    /// The same envelope the abandoned-draft trigger sends, so a comrade
    /// cannot tell the two apart, and the same cooldown, so they cannot add up
    /// to two notifications. Delegates to
    /// [`RuntimeHandles::nudge_comrades`] — see [`Self::send_dm`] for why the
    /// network half never runs under the runtime lock.
    pub async fn nudge_comrades(&self) -> u64 {
        self.handles().nudge_comrades().await
    }

    // ── Ride signals (driver + pillion — see `comrade_core::ride`) ───────────

    /// Say one catalog phrase to the other seat of the motorcycle. Delegates
    /// to [`RuntimeHandles::ride_send_quick`].
    pub async fn ride_send_quick(&self, target: &str, phrase: &str) -> Result<(), UiError> {
        self.handles().ride_send_quick(target, phrase).await
    }

    /// Suggest the next maneuver to the person steering. Delegates to
    /// [`RuntimeHandles::ride_send_route`].
    pub async fn ride_send_route(
        &self,
        target: &str,
        maneuver: &str,
        distance_m: Option<u32>,
        note: Option<String>,
    ) -> Result<(), UiError> {
        self.handles()
            .ride_send_route(target, maneuver, distance_m, note)
            .await
    }

    // ── Journal (wellbeing pillar #1 — strictly local) ───────────────────────
    //
    // Nothing in this section is synchronised, published or uploaded, and no
    // engine reads entry text (Tara's opener sees mood markers only, by
    // construction — see `tara_opener`). The single exception is
    // `share_journal_entry`, which is not an exception to the rule so much as
    // the user overriding it for one entry: they pick the entry, they pick the
    // person, and a copy goes as an ordinary DM.

    /// Save a new journal entry. `mood` is an optional self-reported marker.
    /// The entry never leaves the device on its own: no relay, no network —
    /// only the encrypted store. See [`Self::share_journal_entry`] for the one
    /// path out, which is a copy the user asks for by hand.
    pub fn add_journal_entry(
        &self,
        text: &str,
        mood: Option<&str>,
    ) -> Result<JournalEntryDto, UiError> {
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        let text = text.trim();
        if text.is_empty() {
            return Err(UiError::Engine("journal entry is empty".into()));
        }
        let created_at = now_secs();
        let entry = comrade_storage::JournalEntry {
            id: timestamped_store_id(created_at),
            title: None,
            text: text.to_string(),
            mood: mood
                .map(str::trim)
                .filter(|m| !m.is_empty())
                .map(String::from),
            recording: None,
            created_at,
        };
        store
            .save_journal_entry(&entry)
            .and_then(|()| store.flush())
            .map_err(|e| UiError::Storage(e.to_string()))?;
        Ok(entry.into())
    }

    /// Save a journal entry that is a recording — a voice entry or a video
    /// entry the frontend has already put on disk — plus the title, words and
    /// mood that go with it.
    ///
    /// Separate from [`Self::add_journal_entry`] rather than a widened version
    /// of it, because the two have genuinely different rules. A typed entry is
    /// its text and is rejected empty; a recording entry *is* the recording, so
    /// empty text is the normal case and it is the
    /// [`JournalRecordingDto::file_name`] that may not be blank. Folding them
    /// together would mean one function that validates neither properly.
    ///
    /// One function for both kinds, though, because audio and video differ here
    /// in nothing at all: the same validation, the same record, the same
    /// locality. Only [`JournalRecordingDto::mime`] tells them apart, and it is
    /// the frontend that has to act on that difference.
    ///
    /// The runtime does not create, move, verify or delete the file. The caller
    /// wrote it somewhere of the caller's choosing and is the only party that
    /// can clean it up — which is also why [`Self::delete_journal_entry`]
    /// removes the record and leaves the file to the frontend.
    ///
    /// Strictly local, exactly like the rest of the journal. There is no share
    /// path for a recording: [`Self::share_journal_entry`] sends text, and a
    /// note whose text is empty has nothing to send.
    pub fn add_journal_recording(
        &self,
        title: Option<&str>,
        text: &str,
        mood: Option<&str>,
        recording: JournalRecordingDto,
    ) -> Result<JournalEntryDto, UiError> {
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        let file_name = recording.file_name.trim();
        if file_name.is_empty() {
            return Err(UiError::Engine("journal recording has no file".into()));
        }
        // A name with a separator in it is a path, and a path is how one
        // frontend's record reaches outside the directory another frontend
        // chose for it. Refuse rather than normalise: there is no correct
        // reading of `../../secrets.mp4` to recover.
        if file_name.contains('/') || file_name.contains('\\') || file_name.contains("..") {
            return Err(UiError::Engine(
                "journal recording file name must be a bare name".into(),
            ));
        }
        // The mime is the *only* thing that says whether this is watched or
        // listened to, so a blank one is not a cosmetic gap — it decides which
        // player the frontend draws, and defaulting it here would be this
        // layer guessing at something the caller knows for certain.
        let mime = recording.mime.trim();
        if mime.is_empty() {
            return Err(UiError::Engine(
                "journal recording has no mime type — it is what picks the player".into(),
            ));
        }
        let created_at = now_secs();
        let entry = comrade_storage::JournalEntry {
            id: timestamped_store_id(created_at),
            title: clean_optional(title),
            text: text.trim().to_string(),
            mood: clean_optional(mood),
            recording: Some(comrade_storage::JournalRecording {
                file_name: file_name.to_string(),
                mime: mime.to_string(),
                ..recording.into()
            }),
            created_at,
        };
        store
            .save_journal_entry(&entry)
            .and_then(|()| store.flush())
            .map_err(|e| UiError::Storage(e.to_string()))?;
        Ok(entry.into())
    }

    /// Rename a journal entry, or clear its title with `None`/an empty string.
    ///
    /// Returns the updated entry, or `None` when no entry has that id — the
    /// same shape [`Self::delete_journal_entry`] uses for "there was nothing
    /// there", so a stale list on screen fails the same quiet way twice.
    ///
    /// Only the title changes. Retitling is not a rewrite: the words, the mood,
    /// the recording and the time it was written all stay as they were, and in
    /// particular `created_at` is untouched so renaming an entry does not move
    /// it to the top of the user's own history.
    pub fn set_journal_entry_title(
        &self,
        id: &str,
        title: Option<&str>,
    ) -> Result<Option<JournalEntryDto>, UiError> {
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        let Some(mut entry) = store
            .journal_entry(id)
            .map_err(|e| UiError::Storage(e.to_string()))?
        else {
            return Ok(None);
        };
        entry.title = clean_optional(title);
        store
            .save_journal_entry(&entry)
            .and_then(|()| store.flush())
            .map_err(|e| UiError::Storage(e.to_string()))?;
        Ok(Some(entry.into()))
    }

    /// All journal entries, newest first.
    pub fn journal_entries(&self) -> Result<Vec<JournalEntryDto>, UiError> {
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        Ok(store
            .journal_entries()
            .map_err(|e| UiError::Storage(e.to_string()))?
            .into_iter()
            .map(JournalEntryDto::from)
            .collect())
    }

    /// Delete a journal entry by id. Returns whether one existed.
    ///
    /// Removes the record only. For a video entry the footage is the
    /// frontend's file in the frontend's directory (see [`JournalRecordingDto`]),
    /// and the frontend must delete it — this call cannot, and a caller that
    /// forgets leaves an orphan the user has no way to see or remove.
    pub fn delete_journal_entry(&self, id: &str) -> Result<bool, UiError> {
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        let removed = store
            .remove_journal_entry(id)
            .map_err(|e| UiError::Storage(e.to_string()))?;
        store.flush().map_err(|e| UiError::Storage(e.to_string()))?;
        Ok(removed)
    }

    /// Hand one journal entry to one person, as an ordinary DM.
    ///
    /// Delegates to [`RuntimeHandles::share_journal_entry`], which is where the
    /// reasoning is — including the three things sharing deliberately does not
    /// do. See [`Self::send_dm`] for why the network half never runs under the
    /// runtime lock.
    pub async fn share_journal_entry(
        &self,
        peer: &str,
        entry_id: &str,
    ) -> Result<MessageDto, UiError> {
        self.handles().share_journal_entry(peer, entry_id).await
    }

    // ── Tara (wellbeing pillar #4 — reflective companion, strictly local) ────
    //
    // Same locality guarantee as the journal: no relay, no network. The reply
    // engine is `comrade_core::tara::ReflectiveCompanion` — deterministic,
    // on-device templates (AUDIT §8 / OQ9: the LLM slot stays empty until the
    // owner picks an on-device runtime; a cloud backend is out of the question).

    /// Send a message to Tara: persist the user's turn, compute the companion
    /// reply, persist it too, and return it. `crisis` on the returned DTO
    /// means the frontend must show [`Self::tara_crisis_resources`].
    pub fn tara_send(&self, text: &str) -> Result<TaraMessageDto, UiError> {
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        let text = text.trim();
        if text.is_empty() {
            return Err(UiError::Engine("tara message is empty".into()));
        }
        let existing = store
            .tara_messages()
            .map_err(|e| UiError::Storage(e.to_string()))?;
        let prior_user_turns = existing.iter().filter(|m| !m.from_tara).count() as u64;

        let reply = ReflectiveCompanion.reply(text, prior_user_turns);
        let created_at = now_secs();
        // Sequence-numbered ids, not random ones: turns sent within the same
        // second must keep their exact send order (user, reply, user, reply…)
        // under the (created_at, id) sort — random tails would interleave.
        let seq = existing.len() as u64;
        let user_turn = comrade_storage::TaraMessage {
            id: format!("{created_at:020}-{seq:010}"),
            text: text.to_string(),
            from_tara: false,
            crisis: reply.crisis,
            created_at,
        };
        let tara_turn = comrade_storage::TaraMessage {
            id: format!("{created_at:020}-{:010}", seq + 1),
            text: reply.text,
            from_tara: true,
            crisis: reply.crisis,
            created_at,
        };
        store
            .save_tara_message(&user_turn)
            .and_then(|()| store.save_tara_message(&tara_turn))
            .and_then(|()| store.flush())
            .map_err(|e| UiError::Storage(e.to_string()))?;
        Ok(tara_turn.into())
    }

    /// The whole Tara thread, oldest-first (chat order).
    pub fn tara_thread(&self) -> Result<Vec<TaraMessageDto>, UiError> {
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        Ok(store
            .tara_messages()
            .map_err(|e| UiError::Storage(e.to_string()))?
            .into_iter()
            .map(TaraMessageDto::from)
            .collect())
    }

    /// Delete the entire Tara thread; returns how many turns were removed.
    pub fn clear_tara_thread(&self) -> Result<u64, UiError> {
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        let removed = store
            .clear_tara_messages()
            .map_err(|e| UiError::Storage(e.to_string()))?;
        store.flush().map_err(|e| UiError::Storage(e.to_string()))?;
        Ok(removed)
    }

    /// The opener shown when the Tara thread is empty — shaped by recent
    /// journal *mood markers* only (never entry text; data minimisation), and
    /// by yesterday's usage **rollup numbers** only (never app names).
    ///
    /// Mood outranks usage; the precedence lives in
    /// `ReflectiveCompanion::opener_with_usage` so no frontend can decide it
    /// differently. With no usage recorded this is exactly the opener it always
    /// was.
    pub fn tara_opener(&self) -> Result<String, UiError> {
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        let now = now_secs();
        let signals: Vec<JournalSignal> = store
            .journal_entries()
            .map_err(|e| UiError::Storage(e.to_string()))?
            .into_iter()
            .map(|e| JournalSignal {
                mood: e.mood,
                age_days: now.saturating_sub(e.created_at) / 86_400,
            })
            .collect();
        // The newest recorded day is "yesterday" for nudging purposes only
        // once it is no longer today's still-growing row — commenting on a
        // day the user is still living would be both premature and preachy.
        let days = store
            .attention_days()
            .map_err(|e| UiError::Storage(e.to_string()))?;
        let today = iso_date(now);
        let mut prior = days.iter().filter(|d| d.date != today);
        let yesterday = prior.next().map(usage_signal);
        let mut medians: Vec<u32> = prior.take(7).map(|d| d.doom_minutes).collect();
        medians.sort_unstable();
        let median_doom = (!medians.is_empty()).then(|| medians[medians.len() / 2]);
        let nudge = attention::usage_opener(yesterday, median_doom);
        Ok(ReflectiveCompanion.opener_with_usage(&signals, nudge))
    }

    /// The crisis helplines Tara hands off to, for any frontend to render.
    pub fn tara_crisis_resources(&self) -> Vec<CrisisResourceDto> {
        comrade_core::tara::CRISIS_RESOURCES
            .iter()
            .map(|r| CrisisResourceDto {
                name: r.name.to_string(),
                contact: r.contact.to_string(),
                note: r.note.to_string(),
            })
            .collect()
    }

    // ── In-chat commands (see `comrade_core::command`) ───────────────────────
    //
    // The grammar itself is pure and lives in core, so these are thin: parse,
    // resolve `@handles` against the saved contacts, and act. The one rule worth
    // restating here is that **nothing in this section guesses who somebody
    // meant** — see [`Self::resolve_mentions`].

    /// What the text in a composer means. Pure; no vault needed, so a composer
    /// can call it on every keystroke before anything is unlocked.
    pub fn parse_chat_command(&self, text: &str) -> ChatCommand {
        command::parse(text)
    }

    /// Every command a composer should offer, for `/`-autocomplete and `/help`.
    ///
    /// One list for every frontend — the failure `/pay` demonstrates, having
    /// shipped a live composer preview on desktop and never on Flutter.
    pub fn chat_command_catalog(&self) -> Vec<CommandSpec> {
        command::catalog()
    }

    /// Every `@handle` in `text`, unresolved. Pure — for drawing chips while
    /// typing, before it is worth touching the store.
    pub fn chat_mentions(&self, text: &str) -> Vec<Mention> {
        command::mentions(text)
    }

    /// Every `@handle` in `text`, resolved against the saved contacts.
    ///
    /// Matching follows [`ContactDto`]'s display precedence — the user's own
    /// alias first, then the peer's published handle — and **never the published
    /// handle alone when an alias matched**, because a handle is self-declared
    /// and non-unique while an alias is the local user's own word for somebody.
    ///
    /// Two contacts answering to one handle is a real state, not an edge case:
    /// anyone may publish any name. It comes back as
    /// [`MentionMatchDto::candidates`] for the UI to ask about. Resolving it by
    /// picking the first is how `/task … @ana` reaches the wrong Ana.
    pub fn resolve_mentions(&self, text: &str) -> Result<Vec<MentionMatchDto>, UiError> {
        let contacts = self.list_contacts()?;
        Ok(command::mentions(text)
            .into_iter()
            .map(|m| {
                // Alias first. A contact the user named themself outranks a
                // handle anybody could claim, so an exact alias match is taken
                // as decisive even if some other contact publishes that name.
                let by_alias: Vec<&ContactDto> = contacts
                    .iter()
                    .filter(|c| c.alias.to_lowercase() == m.handle)
                    .collect();
                let matched = if by_alias.is_empty() {
                    contacts
                        .iter()
                        .filter(|c| {
                            c.name.as_deref().is_some_and(|n| {
                                n.trim_start_matches('@').to_lowercase() == m.handle
                            })
                        })
                        .collect()
                } else {
                    by_alias
                };
                let (npub, candidates) = match matched.len() {
                    1 => (Some(matched[0].npub.clone()), Vec::new()),
                    0 => (None, Vec::new()),
                    _ => (None, matched.into_iter().cloned().collect()),
                };
                MentionMatchDto {
                    handle: m.handle,
                    start: m.start,
                    end: m.end,
                    npub,
                    candidates,
                }
            })
            .collect())
    }

    /// How far a `/play` query gets without a network or a library.
    ///
    /// Pure. A link resolves to what it identifies
    /// ([`comrade_core::together::parse_music_link`]); free text resolves to the
    /// [`Recording`] it names ([`comrade_core::command::recording_from_query`]),
    /// which is what a library resolver then searches for. **Nothing here
    /// contacts a service** — turning a query into a catalogue id is
    /// `comrade_core::catalogue`'s job, behind a feature and a disclosure.
    ///
    /// [`Recording`]: comrade_core::together::Recording
    pub fn play_query(&self, query: &str, service: Option<MusicService>) -> PlayTargetDto {
        use comrade_core::together::{parse_music_link, MusicLink};

        let q = query.trim();
        if q.is_empty() {
            return PlayTargetDto {
                plan: PlayPlan::Empty,
                service,
                link: None,
                content: None,
                recording: None,
            };
        }
        if let Some(link) = parse_music_link(q) {
            // Only YouTube can be driven by us, and only through its embed.
            let content = match &link {
                MusicLink::Youtube { video_id } => Some(TogetherContent::Youtube {
                    video_id: video_id.clone(),
                }),
                _ => None,
            };
            let plan = if content.is_some() {
                PlayPlan::OpenNow
            } else {
                PlayPlan::NameOnly
            };
            // A link's own service wins over the alias that was typed: a
            // Spotify URL pasted after `/youtube` is still a Spotify URL.
            let service = Some(match &link {
                MusicLink::Spotify { .. } => MusicService::Spotify,
                MusicLink::AppleMusic { .. } => MusicService::AppleMusic,
                MusicLink::Youtube { .. } => MusicService::Youtube,
            });
            return PlayTargetDto {
                plan,
                service,
                link: Some(link),
                content,
                recording: None,
            };
        }
        PlayTargetDto {
            plan: PlayPlan::FindLocally,
            service,
            link: None,
            content: None,
            recording: Some(command::recording_from_query(q)),
        }
    }

    // ── Tasks (see `comrade_core::karya`) ────────────────────────────────────

    /// Name a piece of work. `peer` of `None` is a note to self, which never
    /// touches a relay.
    ///
    /// Delegates to [`RuntimeHandles::assign_task`] — see [`Self::send_dm`] for
    /// why the network half never runs under the runtime lock.
    pub async fn assign_task(&self, peer: Option<String>, text: &str) -> Result<TaskDto, UiError> {
        self.handles().assign_task(peer, text).await
    }

    /// Every task this device knows about, newest first.
    pub fn tasks(&self) -> Result<Vec<TaskDto>, UiError> {
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        let me = self.my_npub()?;
        Ok(store
            .tasks()
            .map_err(|e| UiError::Storage(e.to_string()))?
            .into_iter()
            .filter_map(|row| task_dto(row, &me))
            .collect())
    }

    /// Move a task to `state`, and tell the other party if there is one.
    ///
    /// Delegates to [`RuntimeHandles::set_task_state`].
    pub async fn set_task_state(&self, id: &str, state: TaskState) -> Result<TaskDto, UiError> {
        self.handles().set_task_state(id, state).await
    }

    /// Offer an in-app action to `peers` — "I thought this might help".
    ///
    /// Returns [`OfferOutcomeDto`] rather than a count, because a deliberate
    /// command that silently did nothing reads as a bug and the *reason* it did
    /// nothing is the part a UI has to say out loud. See that type for why a
    /// bare number was actively misleading.
    ///
    /// Delegates to [`RuntimeHandles::offer_action`].
    pub async fn offer_action(
        &self,
        action: AppAction,
        peers: Vec<String>,
    ) -> Result<OfferOutcomeDto, UiError> {
        self.handles().offer_action(action, peers).await
    }

    /// Say something to Tara from inside a conversation — a **private aside**.
    ///
    /// Identical to [`Self::tara_send`] and deliberately so: it is the same
    /// thread, the same store, the same engine, and above all the same
    /// locality. The separate name exists because the *call site* is different
    /// and that difference is the whole feature — a frontend reaching this from
    /// a chat composer must never be able to reach `send_dm` with the same text.
    ///
    /// What is **not** passed: the conversation. Not the peer's messages, not
    /// the history, not who the chat is with. Seeding a companion with the other
    /// person's words would make them a participant in something they never
    /// opted into, and the peer is not a party to this at all.
    pub fn tara_aside(&self, text: &str) -> Result<TaraMessageDto, UiError> {
        self.tara_send(text)
    }

    /// Ask Tara in front of the person you are talking to — the `@tara …`
    /// spelling, and the counterpart to [`Self::tara_aside`]'s `/tara`.
    ///
    /// Delegates to [`RuntimeHandles::tara_in_chat`], which is where the
    /// reasoning is — including why a question that trips the distress detector
    /// is answered without sending anything.
    pub async fn tara_in_chat(&self, peer: &str, text: &str) -> Result<TaraChatDto, UiError> {
        self.handles().tara_in_chat(peer, text).await
    }

    /// This device's own npub, for deciding which side of a task we are on.
    fn my_npub(&self) -> Result<String, UiError> {
        self.ui
            .current_identity()
            .map(|i| i.npub)
            .ok_or(UiError::NoIdentity)
    }

    // ── Attention (wellbeing pillar #5 — strictly local, never networked) ─────
    //
    // The same locality guarantee as the journal and Tara: no relay, no
    // network, nothing uploaded. Usage data is behavioural data of the most
    // sensitive kind, so only *rollups* reach this layer at all — the raw
    // per-app event stream is reduced on the frontend and dropped there. See
    // `docs/ATTENTION.md` for the honesty gates these commands must keep.

    /// Record (or update) one day's usage rollup. `date` is the local calendar
    /// date as `YYYY-MM-DD`; the frontend owns the calendar, because only it
    /// knows the device's timezone.
    ///
    /// Called repeatedly through the day as the numbers grow — the row is
    /// keyed by date, so this upserts rather than accumulating duplicates.
    pub fn record_attention_day(
        &self,
        date: &str,
        screen_minutes: u32,
        pickups: u32,
        doom_minutes: u32,
    ) -> Result<AttentionDayDto, UiError> {
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        let date = date.trim();
        if !is_iso_date(date) {
            return Err(UiError::Engine(format!(
                "attention day needs a YYYY-MM-DD date, got {date:?}"
            )));
        }
        let day = comrade_storage::AttentionDay {
            date: date.to_string(),
            screen_minutes,
            pickups,
            doom_minutes,
            updated_at: now_secs(),
        };
        store
            .save_attention_day(&day)
            .and_then(|()| store.flush())
            .map_err(|e| UiError::Storage(e.to_string()))?;
        Ok(day.into())
    }

    /// Every recorded usage day, newest first — for the local trend view.
    pub fn attention_days(&self) -> Result<Vec<AttentionDayDto>, UiError> {
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        Ok(store
            .attention_days()
            .map_err(|e| UiError::Storage(e.to_string()))?
            .into_iter()
            .map(AttentionDayDto::from)
            .collect())
    }

    /// `today`'s rollup against the user's own medians over the previous days.
    /// `today` is passed in (rather than derived here) for the same reason
    /// [`Self::record_attention_day`] takes a date: the timezone lives in the
    /// frontend.
    pub fn attention_summary(&self, today: &str) -> Result<AttentionSummaryDto, UiError> {
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        let days = store
            .attention_days()
            .map_err(|e| UiError::Storage(e.to_string()))?;
        let today_row = days.iter().find(|d| d.date == today).cloned();
        let prior: Vec<UsageSignal> = days
            .iter()
            .filter(|d| d.date != today)
            .take(7)
            .map(usage_signal)
            .collect();
        let signal = today_row.as_ref().map(usage_signal).unwrap_or(UsageSignal {
            screen_minutes: 0,
            doom_minutes: 0,
            pickups: 0,
        });
        let cmp = attention::compare_today(signal, &prior);
        Ok(AttentionSummaryDto {
            today: today_row.map(AttentionDayDto::from),
            median_screen_minutes: cmp.median_screen_minutes,
            median_doom_minutes: cmp.median_doom_minutes,
            median_pickups: cmp.median_pickups,
            sample_days: cmp.sample_days,
        })
    }

    /// The package names the user tagged as their own scroll traps.
    ///
    /// Comrade ships **no** built-in blacklist of apps: which apps someone is
    /// compulsive about is their judgement, not ours, and a hard-coded list
    /// would also be a claim about other products we have no standing to make.
    pub fn doom_apps(&self) -> Result<Vec<String>, UiError> {
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        Ok(store
            .load_attention_prefs()
            .map_err(|e| UiError::Storage(e.to_string()))?
            .doom_packages)
    }

    /// Replace the user's doom-app list. Blanks are dropped and duplicates
    /// collapsed, so the frontend can pass a raw selection.
    pub fn set_doom_apps(&self, packages: Vec<String>) -> Result<Vec<String>, UiError> {
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        let mut cleaned: Vec<String> = Vec::new();
        for p in packages {
            let p = p.trim().to_string();
            if !p.is_empty() && !cleaned.contains(&p) {
                cleaned.push(p);
            }
        }
        cleaned.sort();
        let prefs = comrade_storage::AttentionPrefs {
            doom_packages: cleaned.clone(),
        };
        store
            .save_attention_prefs(&prefs)
            .and_then(|()| store.flush())
            .map_err(|e| UiError::Storage(e.to_string()))?;
        Ok(cleaned)
    }

    /// Start a focus session, persisted immediately so an app kill can't make
    /// the history lie about a session that really ran.
    ///
    /// At most one session runs at a time: an earlier one still open is
    /// resolved first — completed-in-spirit sessions past their grace window
    /// become `lapsed`, and one genuinely still running is `abandoned`,
    /// because the user has visibly moved on to a new intention.
    pub fn start_focus_session(
        &self,
        intent: &str,
        planned_minutes: u32,
    ) -> Result<FocusSessionDto, UiError> {
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        if !(FOCUS_MIN_MINUTES..=FOCUS_MAX_MINUTES).contains(&planned_minutes) {
            return Err(UiError::Engine(format!(
                "focus session must be between {FOCUS_MIN_MINUTES} and {FOCUS_MAX_MINUTES} minutes"
            )));
        }
        let now = now_secs();
        // Close out whatever was open before starting something new.
        if let Some(open) = self.open_focus_session(store, now)? {
            let outcome = attention::resolve_stale(open.planned_minutes, open.started_at, now)
                .unwrap_or(FocusOutcome::Abandoned);
            self.finish_stored_session(store, open, outcome, now)?;
        }
        let session = comrade_storage::FocusSession {
            id: timestamped_store_id(now),
            intent: intent.trim().to_string(),
            planned_minutes,
            started_at: now,
            ended_at: None,
            outcome: None,
        };
        store
            .save_focus_session(&session)
            .and_then(|()| store.flush())
            .map_err(|e| UiError::Storage(e.to_string()))?;
        Ok(focus_dto(session, now))
    }

    /// Finish the running session. `completed` is the user's own verdict —
    /// `false` records it as abandoned, deliberately without ceremony (no
    /// streak to break, `docs/ATTENTION.md` gate 3).
    ///
    /// A session whose planned end plus grace window has already passed is
    /// recorded as `lapsed` whatever the caller says: claiming a completion
    /// for a session nobody was present for would make the history a lie, and
    /// the history is the only thing the progressive-duration rule reads.
    /// Returns `None` when no session was running.
    pub fn finish_focus_session(
        &self,
        completed: bool,
    ) -> Result<Option<FocusSessionDto>, UiError> {
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        let now = now_secs();
        let Some(open) = self.open_focus_session(store, now)? else {
            return Ok(None);
        };
        let claimed = if completed {
            FocusOutcome::Completed
        } else {
            FocusOutcome::Abandoned
        };
        let outcome =
            attention::resolve_stale(open.planned_minutes, open.started_at, now).unwrap_or(claimed);
        let finished = self.finish_stored_session(store, open, outcome, now)?;
        Ok(Some(focus_dto(finished, now)))
    }

    /// The session currently running, if any. Resolving is a *read* that can
    /// write: a session found past its grace window is recorded as `lapsed`
    /// here and reported as gone, so every caller sees one consistent answer
    /// rather than each frontend inventing its own staleness rule.
    pub fn active_focus_session(&self) -> Result<Option<FocusSessionDto>, UiError> {
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        let now = now_secs();
        Ok(self
            .open_focus_session(store, now)?
            .map(|s| focus_dto(s, now)))
    }

    /// Focus-session history, newest first.
    pub fn focus_sessions(&self) -> Result<Vec<FocusSessionDto>, UiError> {
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        let now = now_secs();
        Ok(store
            .focus_sessions()
            .map_err(|e| UiError::Storage(e.to_string()))?
            .into_iter()
            .map(|s| focus_dto(s, now))
            .collect())
    }

    /// The duration to suggest next, from this user's own completion history —
    /// the "rebuild the span" rule (`attention::suggest_focus_minutes`).
    pub fn suggested_focus_minutes(&self) -> Result<u32, UiError> {
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        let history: Vec<(u32, FocusOutcome)> = store
            .focus_sessions()
            .map_err(|e| UiError::Storage(e.to_string()))?
            .into_iter()
            .filter_map(|s| {
                s.outcome
                    .as_deref()
                    .and_then(FocusOutcome::from_key)
                    .map(|o| (s.planned_minutes, o))
            })
            .collect();
        Ok(attention::suggest_focus_minutes(&history))
    }

    /// The session lengths to offer, in ascending order.
    ///
    /// Unlike everything else on this surface it needs no vault: the ladder's
    /// rungs are a constant of the design, not the user's data, and a frontend
    /// that had to unlock before it could draw its own duration chips would be
    /// gatekeeping for no privacy gain. It is here rather than hardcoded per
    /// frontend so the three UIs cannot drift from
    /// [`attention::suggest_focus_minutes`], which reads the same list to
    /// decide which rung the user has earned — an Android that offered 60m
    /// would offer a length the ladder can never suggest back.
    pub fn focus_presets(&self) -> Vec<u32> {
        attention::FOCUS_PRESETS.to_vec()
    }

    /// The intention nudge to show before starting a session, rotated by how
    /// many sessions came before it.
    pub fn focus_prompt(&self) -> Result<String, UiError> {
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        let prior = store
            .focus_sessions()
            .map_err(|e| UiError::Storage(e.to_string()))?
            .len() as u64;
        Ok(attention::focus_prompt(prior).to_string())
    }

    /// The line to show when a session ends — a reflection prompt for a
    /// completion, plain acknowledgement for anything else.
    pub fn focus_reflection(&self, outcome: &str) -> Result<String, UiError> {
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        let parsed = FocusOutcome::from_key(outcome)
            .ok_or_else(|| UiError::Engine(format!("unknown focus outcome {outcome:?}")))?;
        let prior = store
            .focus_sessions()
            .map_err(|e| UiError::Storage(e.to_string()))?
            .len() as u64;
        Ok(attention::focus_reflection(parsed, prior))
    }

    /// The guided stretch-break routine, in order.
    ///
    /// Vault-free for the same reason [`Self::focus_presets`] is: the routine
    /// is a constant of the design, not the user's data, and it lives in the
    /// engine so no frontend keeps a list that could drift from the others'.
    pub fn stretch_routine(&self) -> Vec<StretchStepDto> {
        attention::STRETCH_ROUTINE
            .iter()
            .map(|s| StretchStepDto {
                key: s.key.to_string(),
                name: s.name.to_string(),
                cue: s.cue.to_string(),
                seconds: s.seconds,
                mirrored: s.mirrored,
            })
            .collect()
    }

    /// Add text to the reading library and return it chunked, ready to read.
    ///
    /// The user brings the text (paste, or the share sheet); nothing here
    /// fetches a URL — a reader that went to the network would put an
    /// arbitrary-fetch path into the one app that promises not to. The source
    /// label is derived offline from the first link in the text
    /// ([`attention::reading_source`]) so a library of articles carried in
    /// from different apps can say where each came from.
    pub fn save_read(&self, title: &str, text: &str) -> Result<SavedReadDto, UiError> {
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        let text = text.trim();
        if text.is_empty() {
            return Err(UiError::Engine("nothing to read".into()));
        }
        let now = now_secs();
        let read = comrade_storage::SavedRead {
            id: timestamped_store_id(now),
            title: title.trim().to_string(),
            source: attention::reading_source(text).unwrap_or_default(),
            text: text.to_string(),
            position: 0,
            added_at: now,
            updated_at: now,
        };
        store
            .save_saved_read(&read)
            .and_then(|()| store.flush())
            .map_err(|e| UiError::Storage(e.to_string()))?;
        Ok(saved_read_dto(read))
    }

    /// The reading library, newest first — rows only, not the texts.
    ///
    /// This read may write once: a vault written before the library existed
    /// holds one read in the old single-slot tree, and the first list moves it
    /// into the library (position included) rather than losing it. The same
    /// "reads may write" trade [`Self::active_focus_session`] already makes.
    pub fn saved_reads(&self) -> Result<Vec<SavedReadSummaryDto>, UiError> {
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        if let Some(legacy) = store
            .load_reading_state()
            .map_err(|e| UiError::Storage(e.to_string()))?
        {
            let migrated = comrade_storage::SavedRead {
                id: timestamped_store_id(legacy.updated_at),
                title: legacy.title,
                source: attention::reading_source(&legacy.text).unwrap_or_default(),
                text: legacy.text,
                position: legacy.position,
                added_at: legacy.updated_at,
                updated_at: legacy.updated_at,
            };
            store
                .save_saved_read(&migrated)
                .and_then(|()| store.clear_reading_state().map(|_| ()))
                .and_then(|()| store.flush())
                .map_err(|e| UiError::Storage(e.to_string()))?;
        }
        Ok(store
            .saved_reads()
            .map_err(|e| UiError::Storage(e.to_string()))?
            .into_iter()
            .map(saved_read_summary_dto)
            .collect())
    }

    /// One saved read, chunked and ready to pick up where it was left off.
    pub fn open_saved_read(&self, id: &str) -> Result<Option<SavedReadDto>, UiError> {
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        Ok(store
            .saved_read(id)
            .map_err(|e| UiError::Storage(e.to_string()))?
            .map(saved_read_dto))
    }

    /// Remember which chunk the reader is on. Clamped to the real chunk count,
    /// so a stored position can never point past the end of the text.
    pub fn set_saved_read_position(
        &self,
        id: &str,
        position: u32,
    ) -> Result<Option<SavedReadDto>, UiError> {
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        let Some(mut read) = store
            .saved_read(id)
            .map_err(|e| UiError::Storage(e.to_string()))?
        else {
            return Ok(None);
        };
        let chunks = attention::chunk_reading(&read.text).len() as u32;
        read.position = position.min(chunks.saturating_sub(1));
        read.updated_at = now_secs();
        store
            .save_saved_read(&read)
            .and_then(|()| store.flush())
            .map_err(|e| UiError::Storage(e.to_string()))?;
        Ok(Some(saved_read_dto(read)))
    }

    /// Forget one saved read. Returns whether it existed.
    pub fn delete_saved_read(&self, id: &str) -> Result<bool, UiError> {
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        let removed = store
            .delete_saved_read(id)
            .map_err(|e| UiError::Storage(e.to_string()))?;
        store.flush().map_err(|e| UiError::Storage(e.to_string()))?;
        Ok(removed)
    }

    /// The session still open, resolving (and persisting) a lapse first so
    /// every caller agrees on what "running" means. Shared by
    /// [`Self::active_focus_session`], [`Self::start_focus_session`] and
    /// [`Self::finish_focus_session`].
    fn open_focus_session(
        &self,
        store: &comrade_storage::EncryptedStore,
        now: u64,
    ) -> Result<Option<comrade_storage::FocusSession>, UiError> {
        let sessions = store
            .focus_sessions()
            .map_err(|e| UiError::Storage(e.to_string()))?;
        let Some(open) = sessions.into_iter().find(|s| s.outcome.is_none()) else {
            return Ok(None);
        };
        match attention::resolve_stale(open.planned_minutes, open.started_at, now) {
            Some(outcome) => {
                self.finish_stored_session(store, open, outcome, now)?;
                Ok(None)
            }
            None => Ok(Some(open)),
        }
    }

    /// Stamp `outcome` on a session and persist it.
    fn finish_stored_session(
        &self,
        store: &comrade_storage::EncryptedStore,
        session: comrade_storage::FocusSession,
        outcome: FocusOutcome,
        now: u64,
    ) -> Result<comrade_storage::FocusSession, UiError> {
        // A lapsed session ended when its plan did, not when someone finally
        // looked: stamping "now" would credit hours nobody was present for.
        let ended_at = match outcome {
            FocusOutcome::Lapsed => session.started_at + u64::from(session.planned_minutes) * 60,
            _ => now,
        };
        let finished = comrade_storage::FocusSession {
            ended_at: Some(ended_at),
            outcome: Some(outcome.as_str().to_string()),
            ..session
        };
        store
            .save_focus_session(&finished)
            .and_then(|()| store.flush())
            .map_err(|e| UiError::Storage(e.to_string()))?;
        Ok(finished)
    }

    // ── Encrypted media pipeline (NIP-94/96 · Blossom) ───────────────────────

    /// Encrypt `bytes` for `target_pubkey`, upload the opaque blob to Blossom,
    /// build a zero-knowledge NIP-94 reference, persist it locally, and deliver
    /// the reference privately over the E2E DM channel. Returns the media DTO.
    ///
    /// The AES key is derived from the ECDH shared secret, so it is never
    /// uploaded and never placed in the public event — the recipient re-derives
    /// it from their own private key and our pubkey.
    ///
    /// Delegates to [`RuntimeHandles::upload_and_send_media`] — see [`Self::send_dm`].
    pub async fn upload_and_send_media(
        &self,
        target_pubkey: &str,
        bytes: Vec<u8>,
        mime_type: &str,
        caption: &str,
    ) -> Result<MediaMessageDto, UiError> {
        self.handles()
            .upload_and_send_media(target_pubkey, bytes, mime_type, caption)
            .await
    }

    /// Resolve a NIP-94 reference by event id, fetch the encrypted blob, and
    /// decrypt it with the re-derived ECDH key. Returns base64 bytes + MIME.
    ///
    /// Delegates to [`RuntimeHandles::download_and_decrypt_media`] — see
    /// [`Self::send_dm`].
    pub async fn download_and_decrypt_media(
        &self,
        event_id: &str,
    ) -> Result<MediaBytesDto, UiError> {
        self.handles().download_and_decrypt_media(event_id).await
    }

    /// Full encrypted-media history with `peer` (npub or hex), oldest first —
    /// the media counterpart of [`Self::messages_with`]. Lets a frontend
    /// render past attachments inline after a restart, not just ones that
    /// arrived live this session (references are persisted the moment they're
    /// sent or received — see [`Self::upload_and_send_media`] and
    /// `dispatch_incoming_dm`).
    pub fn media_with(&self, peer: &str) -> Result<Vec<MediaMessageDto>, UiError> {
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        let peer_hex = parse_pubkey(peer)?.to_hex();
        let own_npub = self
            .ui
            .current_identity()
            .map(|i| i.npub)
            .unwrap_or_default();

        let mut items: Vec<MediaMessageDto> = store
            .values::<MediaRef>(MEDIA_REFS_TREE)
            .map_err(|e| UiError::Storage(e.to_string()))?
            .into_iter()
            .filter(|r| r.peer_pubkey == peer_hex)
            .map(|r| MediaMessageDto {
                event_id: r.event_id,
                url: r.url,
                mime_type: r.mime_type,
                caption: r.caption,
                sender: if r.outgoing {
                    own_npub.clone()
                } else {
                    to_npub(&r.peer_pubkey)
                },
                created_at: r.created_at,
                size: r.size,
                outgoing: r.outgoing,
            })
            .collect();
        items.sort_by_key(|m| m.created_at);
        Ok(items)
    }

    // ── Milestone 3: progressive-disclosure workspace controller ─────────────

    /// Switch the active workspace, enforcing the [`comrade_state`] transition
    /// rules. An invalid or un-paired transition returns a typed [`UiError`]
    /// (surfaced to the frontend as a rejected promise / JSON error).
    ///
    /// On success, also brings the Saathi mesh engine's lifecycle in line with
    /// the new workspace (see [`Self::sync_saathi_lifecycle`]) — entering
    /// `OffGridTravel` really starts mDNS discovery, it doesn't just flip a label.
    pub async fn toggle_workspace(&mut self, target: &str) -> Result<WorkspaceDto, UiError> {
        let dto = self.ui.switch_workspace(target)?;
        self.sync_saathi_lifecycle().await;
        Ok(dto)
    }

    /// Step back to the previous workspace, syncing the Saathi mesh lifecycle
    /// exactly as [`Self::toggle_workspace`] does.
    pub async fn back(&mut self) -> WorkspaceDto {
        let dto = self.ui.back();
        self.sync_saathi_lifecycle().await;
        dto
    }

    /// The Bluetooth transport's policy half, for a platform BLE service to
    /// drive.
    ///
    /// Handed out as an `Arc` rather than proxied method-by-method because the
    /// radio's callbacks run on their own threads at their own cadence and must
    /// never contend on the runtime's `RwLock` — a scanning callback blocking
    /// behind a relay round trip would stall the radio (AUDIT P2, the same
    /// discipline [`Self::handles`] exists for).
    pub fn ble_router(&self) -> Arc<BleRouter> {
        self.ble.clone()
    }

    /// Snapshot of the off-grid mesh's live status — for seeding a UI's
    /// connectivity indicator before any [`BridgeEvent::MeshStatusChanged`]
    /// has arrived (e.g. right after a cold start or an activity recreation).
    pub fn mesh_status(&self) -> MeshStatusDto {
        match &self.saathi {
            Some(engine) => MeshStatusDto {
                active: true,
                peer_count: engine.peer_count() as u64,
            },
            None => MeshStatusDto {
                active: false,
                peer_count: 0,
            },
        }
    }

    /// Ensure the Saathi engine is running whenever anything needs it.
    /// Centralised here (rather than duplicated in
    /// `toggle_workspace`/`back`) so every path that can change the workspace —
    /// a voice command, a future UI toggle, stepping back — drives the same
    /// real engine the persistent mesh-status indicator reads from.
    async fn sync_saathi_lifecycle(&mut self) {
        // Two independent reasons to be running, so this is an OR, not just the
        // workspace flag:
        //
        //  • the OffGridTravel workspace, where the mesh *replaces* relays, and
        //  • any unlocked vault, because "the person I'm messaging is on this
        //    WiFi" is not a mode the user should have to select — it is the
        //    fallback that makes a DM arrive when the internet is down.
        //
        // Only when neither holds does the engine stop.
        let should_run = self.ui.current_workspace().mesh_active || self.is_vault_unlocked();
        match (should_run, self.saathi.is_some()) {
            (true, false) => self.start_saathi().await,
            (false, true) => self.stop_saathi().await,
            _ => {}
        }
    }

    /// Start the Saathi mesh engine and spawn the task that forwards its live
    /// peer-count stream onto the shared event bus. Best-effort: if the swarm
    /// fails to initialise (e.g. no usable socket), the workspace switch still
    /// succeeds — the indicator just reports `active: false` rather than
    /// hanging in a perpetual "connecting" state.
    async fn start_saathi(&mut self) {
        let label = self
            .ui
            .username()
            .or_else(|| self.ui.current_identity().map(|i| i.npub))
            .unwrap_or_else(|| "comrade-mesh-peer".to_string());
        match SaathiEngine::new(label).await {
            Ok(engine) => {
                let engine = Arc::new(engine);
                self.spawn_mesh_status_forwarder(engine.clone());
                self.saathi = Some(engine);
            }
            Err(e) => {
                warn!("Saathi: failed to start mesh engine: {e}");
                let _ = self
                    .events
                    .send(BridgeEvent::MeshStatusChanged(MeshStatusDto {
                        active: false,
                        peer_count: 0,
                    }));
            }
        }
    }

    /// Shut down the Saathi mesh engine and tell the UI it is gone.
    async fn stop_saathi(&mut self) {
        if let Some(engine) = self.saathi.take() {
            engine.shutdown().await;
        }
        let _ = self
            .events
            .send(BridgeEvent::MeshStatusChanged(MeshStatusDto {
                active: false,
                peer_count: 0,
            }));
    }

    /// Forward the engine's reach stream onto the bridge event bus as
    /// [`BridgeEvent::MeshStatusChanged`] — once immediately (the starting
    /// snapshot) and again every time reach changes — and wake the outbox the
    /// moment the local network becomes a route that works.
    ///
    /// `peer_count` on the wire is the **deliverable** count, not the
    /// discovered one. That is the number the indicator is implicitly promising
    /// ("you can reach these people"), and reporting sightings there is what let
    /// the app show a peer badge while every send failed with
    /// `InsufficientPeers`.
    fn spawn_mesh_status_forwarder(&self, engine: Arc<SaathiEngine>) {
        let mut reach_rx = engine.reach_stream();
        let tx = self.events.clone();
        let wake = self.transport_wake.clone();
        tokio::spawn(async move {
            let mut could_deliver = reach_rx.borrow().can_deliver();
            let _ = tx.send(BridgeEvent::MeshStatusChanged(MeshStatusDto {
                active: true,
                peer_count: reach_rx.borrow().deliverable as u64,
            }));
            while reach_rx.changed().await.is_ok() {
                let reach = *reach_rx.borrow();
                let _ = tx.send(BridgeEvent::MeshStatusChanged(MeshStatusDto {
                    active: true,
                    peer_count: reach.deliverable as u64,
                }));
                // Only the *transition* into deliverable wakes the outbox. A
                // second peer arriving changes nothing about whether queued mail
                // can go, and waking on every change would turn a busy café
                // network into a flush loop.
                if reach.can_deliver() && !could_deliver {
                    tracing::info!("local network is now deliverable — flushing queued mail");
                    wake.notify_one();
                }
                could_deliver = reach.can_deliver();
            }
        });
    }

    // ── Sakha/Sakhi CRDT ledger: pairing + entries + sync ────────────────────

    /// Restore a previously-completed pairing (and its ledger snapshot) from
    /// the encrypted store, so a returning paired couple doesn't have to
    /// re-exchange keys every launch. Called once from [`Self::unlock_vault`],
    /// right after the Sakha engine is constructed. Best-effort: a missing or
    /// unreadable record just leaves the engine unpaired, exactly as if this
    /// were the first launch.
    async fn restore_sakha_pairing(&mut self) {
        let Some(store) = self.ui.store_ref() else {
            return;
        };
        let record: Option<SakhaPairingRecord> =
            store.get(SAKHA_TREE, SAKHA_PAIRING_KEY).ok().flatten();
        if let (Some(record), Some(sakha)) = (record, self.sakha.clone()) {
            match PublicKey::parse(&record.partner_pubkey_hex) {
                Ok(partner_pk) => {
                    if let Err(e) = sakha.pair_with(partner_pk) {
                        warn!("failed to restore Sakha pairing: {e}");
                    }
                }
                Err(e) => warn!("stored Sakha partner key is invalid: {e}"),
            }
        }
        // The ledger snapshot restores independently of pairing succeeding —
        // the CRDT text itself doesn't need a partner key to read locally.
        if let Ok(Some(state)) = store.load_ledger_state() {
            if let Some(sakha) = self.sakha.clone() {
                if let Err(e) = sakha.load_snapshot(&state.snapshot).await {
                    warn!("failed to restore Sakha ledger snapshot: {e}");
                }
            }
        }
    }

    /// Start the background loop that merges the partner's incoming ledger
    /// updates: each successful merge pushes [`BridgeEvent::LedgerUpdated`]
    /// and persists a fresh snapshot. Idempotent and safe to call whether
    /// triggered by a fresh [`Self::pair_sakha`] or a pairing restored at
    /// unlock — spawned at most once per runtime.
    fn spawn_sakha_sync_loop(&mut self) {
        if self.sakha_sync_spawned {
            return;
        }
        let Some(sakha) = self.sakha.clone() else {
            return;
        };
        self.sakha_sync_spawned = true;
        let tx = self.events.clone();
        let store = self.ui.store_arc();
        let sakha_for_snapshot = sakha.clone();
        self.sakha_sync_task = Some(tokio::spawn(async move {
            sakha.connect().await;
            let cb: SakhaSyncCallback = Box::new(move |ledger| {
                let _ = tx.send(BridgeEvent::LedgerUpdated { ledger });
                let Some(store) = store.clone() else { return };
                let sakha = sakha_for_snapshot.clone();
                tokio::spawn(async move { persist_ledger_snapshot(&store, &sakha).await });
            });
            if let Err(e) = sakha.subscribe_sync(cb).await {
                warn!("sakha sync loop ended: {e}");
            }
        }));
    }

    /// Perform the Sakha/Sakhi pairing handshake with `partner_pubkey` (npub
    /// or hex) as `role` (`"sakha"`/`"sakhi"`): derives the shared ledger key,
    /// persists the pairing so it survives a restart, and starts the
    /// background sync loop that merges the partner's future ledger updates
    /// live. Returns the resulting pairing status.
    pub async fn pair_sakha(
        &mut self,
        partner_pubkey: &str,
        role: &str,
    ) -> Result<SakhaStatusDto, UiError> {
        let sakha = self.sakha.clone().ok_or(UiError::VaultLocked)?;
        let peer = parse_pubkey(partner_pubkey)?;
        sakha
            .pair_with(peer)
            .map_err(|e| UiError::Engine(e.to_string()))?;

        let role = normalize_pair_role(role);
        if let Some(store) = self.ui.store_ref() {
            let record = SakhaPairingRecord {
                partner_pubkey_hex: peer.to_hex(),
                role,
            };
            store
                .put(SAKHA_TREE, SAKHA_PAIRING_KEY, &record)
                .and_then(|()| store.flush())
                .map_err(|e| UiError::Storage(e.to_string()))?;
        }

        self.spawn_sakha_sync_loop();
        self.sakha_status()
    }

    /// This device's Sakha/Sakhi pairing state.
    pub fn sakha_status(&self) -> Result<SakhaStatusDto, UiError> {
        let sakha = self.sakha.clone().ok_or(UiError::VaultLocked)?;
        let partner_npub = sakha
            .partner_pubkey()
            .map(|pk| pk.to_bech32().unwrap_or_else(|_| pk.to_hex()));
        let role = self
            .ui
            .store_ref()
            .and_then(|s| {
                s.get::<SakhaPairingRecord>(SAKHA_TREE, SAKHA_PAIRING_KEY)
                    .ok()
                    .flatten()
            })
            .map(|r| r.role);
        Ok(SakhaStatusDto {
            paired: sakha.is_paired(),
            partner_npub,
            role,
        })
    }

    /// Append an entry to the shared Sakha/Sakhi CRDT ledger, persist a fresh
    /// local snapshot, and return the merged ledger text. Requires a
    /// completed pairing — use [`Self::pair_sakha`] first.
    ///
    /// Delegates to [`RuntimeHandles::sakha_add_entry`] — see [`Self::send_dm`].
    pub async fn sakha_add_entry(
        &self,
        description: &str,
        amount_inr: f64,
        paid_by: &str,
    ) -> Result<String, UiError> {
        self.handles()
            .sakha_add_entry(description, amount_inr, paid_by)
            .await
    }

    /// The current Sakha/Sakhi ledger text (local CRDT state — no network
    /// round trip). Empty until entries exist or a snapshot/sync restores some.
    pub async fn sakha_read_ledger(&self) -> Result<String, UiError> {
        let sakha = self.sakha.clone().ok_or(UiError::VaultLocked)?;
        Ok(sakha.read_ledger().await)
    }

    /// Publish the current Sakha/Sakhi shared CRDT ledger state to the partner.
    /// Returns the sync event id (hex). Without a completed pairing handshake the
    /// engine returns a typed error rather than panicking.
    ///
    /// Delegates to [`RuntimeHandles::sync_ledger`] — see [`Self::send_dm`].
    pub async fn sync_ledger(&self) -> Result<String, UiError> {
        self.handles().sync_ledger().await
    }

    // ── Sync view-model delegations (shared with the existing desktop UI) ────

    pub fn workspaces(&self) -> Vec<WorkspaceDto> {
        self.ui.workspaces()
    }

    pub fn current_workspace(&self) -> WorkspaceDto {
        self.ui.current_workspace()
    }

    pub fn generate_identity(&mut self) -> Result<IdentityDto, UiError> {
        self.ui.generate_identity()
    }

    pub fn current_identity(&self) -> Option<IdentityDto> {
        self.ui.current_identity()
    }

    pub fn extract_payments(&self, text: &str) -> Result<Vec<UpiIntentDto>, UiError> {
        self.ui.extract_payments(text)
    }

    /// Whether the encrypted store is unlocked (a superset state of the vault).
    pub fn is_store_unlocked(&self) -> bool {
        self.ui.is_store_unlocked()
    }

    /// A cheap, synchronous snapshot of the live engine handles + identity
    /// bits a network operation needs — see [`RuntimeHandles`]. Bridges
    /// (Tauri commands, `comrade_jni`) call this to run a network operation
    /// without holding the shared `Arc<RwLock<ComradeRuntime>>` guard across
    /// the round trip (AUDIT P2 — the same discipline [`Self::profile_refresher`]
    /// already established for profile refreshes, generalised to every other
    /// network-touching method).
    pub fn handles(&self) -> RuntimeHandles {
        RuntimeHandles {
            sabha: self.sabha.clone(),
            vault: self.vault.clone(),
            sakha: self.sakha.clone(),
            store: self.ui.store_arc(),
            keys: self.ui.identity_keys(),
            username: self.ui.username(),
            identity: self.ui.current_identity(),
            outbox: self.outbox.clone(),
            events: self.events.clone(),
            mesh: self.mesh_link(),
            prefer_local: self.ui.current_workspace().mesh_active,
            nudge_watch: self.nudge_watch.clone(),
            presence_active: self.presence_active.clone(),
            together: self.together.clone(),
            together_starts_seen: self.together_starts_seen.clone(),
            together_shares_seen: self.together_shares_seen.clone(),
        }
    }

    /// A sealed-mail sender for the running mesh, if there is one and we have
    /// keys to seal with.
    fn mesh_link(&self) -> Option<LocalRadios> {
        Some(LocalRadios {
            // `None` when the Saathi engine is not up. Bluetooth alone is still
            // a local route, so this is not a reason to have no radios at all.
            mesh: self.saathi.clone().map(|engine| MeshLink { engine }),
            ble: self.ble.clone(),
            keys: self.ui.identity_keys()?,
        })
    }
}

// ── Detached network operations (AUDIT P2) ──────────────────────────────────
//
// Every method below mirrors a `ComradeRuntime` method of the same name
// (which delegates to it via `ComradeRuntime::handles`), but takes owned data
// instead of `&self`. That is the whole point: `&self` methods on
// `ComradeRuntime` are called through `Arc<RwLock<ComradeRuntime>>` in every
// bridge, and Rust ties an `async fn(&self, …)`'s returned future to `&self`'s
// lifetime for its *entire* body — even the part after the method stops
// touching `self` — so a bridge calling `state.read().await.send_dm(…).await`
// holds the guard across the relay round trip no matter how the method body
// is written. `RuntimeHandles` breaks that tie: a bridge takes a snapshot
// under a briefly-held guard (`state.read().await.handles()`, cheap Arc/Option
// clones, no `.await` of its own) and the guard is dropped at the end of that
// statement — then the actual network operation runs guard-free.
#[derive(Clone)]
pub struct RuntimeHandles {
    sabha: Option<Arc<SabhaEngine>>,
    vault: Option<Arc<VaultEngine>>,
    sakha: Option<Arc<SakhaEngine>>,
    store: Option<Arc<comrade_storage::EncryptedStore>>,
    keys: Option<nostr_sdk::prelude::Keys>,
    username: Option<String>,
    identity: Option<IdentityDto>,
    /// The live sender outbox — shared with the runtime and the inbox callback,
    /// so a send failure here and a receipt there act on the same queue.
    outbox: Arc<Outbox>,
    /// Event bus, so a flush can report status changes (`sent` / `failed`)
    /// without going back through `ComradeRuntime`.
    events: broadcast::Sender<BridgeEvent>,
    /// The local radios — WiFi mesh and Bluetooth — the transports that carry
    /// a DM when no relay will.
    mesh: Option<LocalRadios>,
    /// Whether the user has put the local network ahead of relays (the
    /// `OffGridTravel` workspace, switched from the app bar).
    prefer_local: bool,
    /// The composer watch, shared with the runtime the frontends call into —
    /// see [`ComradeRuntime::nudge_watch`] and [`Self::nudge_abandoned_drafts`].
    nudge_watch: Arc<NudgeWatch>,
    /// Shared with [`ComradeRuntime::presence_active`] — see its doc comment.
    presence_active: Arc<std::sync::atomic::AtomicBool>,
    /// Shared with [`ComradeRuntime::together`] — see its doc comment.
    together: Arc<Mutex<Option<TogetherSession>>>,
    /// Shared with the inbox callback's [`TogetherLink`], so an invitation
    /// deduped on one path is deduped on the other. Carried here because
    /// [`Self::together_receive_direct`] rebuilds that link.
    together_starts_seen: Arc<SeenSet>,
    /// Carried because [`Self::together_receive_direct`] has to build a whole
    /// [`TogetherLink`] and that struct has this field — **not** for the reason
    /// above it. The direct path passes `None` for the event id, having no
    /// wrapper to key on, so it neither reads nor writes this set; unlike
    /// `together_starts_seen`, nothing is actually shared through it today.
    ///
    /// It is the runtime's own set rather than a fresh one all the same, because
    /// a fresh one would be a second share set with nothing to reveal that it
    /// had diverged, and this is where an event id would first appear if that
    /// channel ever grew one.
    together_shares_seen: Arc<SeenSet>,
}

impl RuntimeHandles {
    pub async fn send_dm(&self, target: &str, content: &str) -> Result<MessageDto, UiError> {
        self.send_dm_reply(target, content, None).await
    }

    pub async fn send_dm_reply(
        &self,
        target: &str,
        content: &str,
        reply_to: Option<&str>,
    ) -> Result<MessageDto, UiError> {
        if content.trim().is_empty() {
            return Err(UiError::Engine("message is empty".into()));
        }
        let vault = self.vault.clone().ok_or(UiError::VaultLocked)?;
        let peer = parse_pubkey(target)?;
        let peer_npub = to_npub(target);
        let created_at = now_secs();

        // Which radio goes first: whatever is actually up, with the app-bar
        // setting deciding when both are.
        let plan = SendPlan::for_attempt(self.prefer_local, self.reach(&vault).await, 0);
        let local_id = local_message_id(&peer_npub, content, created_at);

        // Local-first: seal it onto this WiFi before spending the internet. A
        // frame nobody took is not delivery, so a `false` here falls straight
        // through to a relay — precedence orders the transports, it does not
        // switch one off.
        let on_mesh = plan.local_first
            && self
                .try_mesh(&peer, &local_id, content, reply_to, created_at)
                .await;

        // A relay that will not take the message is not the end of the road:
        // queue it, persist it as `queued`, and let the flush loop retry
        // (bitchat whitepaper §6.1). Before this, a publish failure lost the
        // text entirely — the worst failure mode an app about staying in touch
        // can have.
        let (id, status) = if on_mesh && !plan.force_both {
            // It is on the local network but not acknowledged — a mesh publish
            // reaching *a* peer is not proof the recipient got it, so it stays
            // queued until their receipt clears it.
            self.enqueue(&local_id, &peer_npub, content, reply_to, created_at);
            (local_id, STATUS_QUEUED)
        } else {
            match vault.send_dm_reply(&peer, content, reply_to).await {
                Ok(id) => (id.to_hex(), "sent"),
                Err(e) => {
                    tracing::info!(error = %e, "DM could not be published — queued for retry");
                    // No relay took it, but the recipient may be on this WiFi.
                    // (Under local precedence that attempt already happened.)
                    if !plan.local_first {
                        self.try_mesh(&peer, &local_id, content, reply_to, created_at)
                            .await;
                    }
                    self.enqueue(&local_id, &peer_npub, content, reply_to, created_at);
                    (local_id, STATUS_QUEUED)
                }
            }
        };

        // The words are out of the composer and into the pipeline, so there is
        // nothing left unsaid to nudge about — including in the queued case,
        // where the outbox will deliver them without the user doing anything
        // else. No frontend has to remember this: sending is the one way to
        // leave a composer that must never look like abandoning it.
        self.nudge_watch.sent(&peer_npub);

        let read = read_body(content.to_string());
        let dto = MessageDto {
            id,
            peer: peer_npub.clone(),
            content: read.text,
            created_at,
            outgoing: true,
            author: read.author,
            status: Some(status.into()),
            reply_to: reply_to.map(str::to_string),
            shared_note: read.shared_note,
            link_preview: read.link_preview,
            forwarded: read.forwarded,
            // Freshly sent — nobody has had the chance to star or pin it yet.
            actions: MessageActionState {
                starred: false,
                pinned: false,
            },
        };
        if let Some(store) = &self.store {
            let row = comrade_storage::StoredMessage {
                id: dto.id.clone(),
                peer_npub: dto.peer.clone(),
                // The wire form, not `dto.content`: the marker has to survive a
                // reload for the bubble to be drawn the same way next time.
                content: content.to_string(),
                created_at: dto.created_at,
                outgoing: true,
                status: Some(status.into()),
                reply_to: dto.reply_to.clone(),
            };
            if let Err(e) = store.save_message(&row).and_then(|()| store.flush()) {
                warn!("failed to persist outgoing DM: {e}");
            }
        }
        // Only a message that actually reached a relay should trigger the
        // profile share; a queued one shares on its successful flush instead.
        if status == "sent" {
            share_profile_on_accept(
                self.store.clone(),
                self.vault.clone(),
                self.username.clone(),
                &peer_npub,
                &peer,
            );
        }
        Ok(dto)
    }

    /// Which routes are usable right now — the input [`SendPlan`] orders on.
    ///
    /// Both probes are local reads (a relay-status scan and a `watch` borrow),
    /// so this is cheap enough to ask on every send rather than caching a
    /// snapshot that could be minutes stale by the time somebody hits enter.
    async fn reach(&self, vault: &Arc<VaultEngine>) -> TransportReach {
        TransportReach {
            relay: vault.has_connected_relay().await,
            // Either local radio counts. "Local" is a route class, not a
            // specific technology, and the precedence the user sets is
            // "nearby before the internet" — which stays true whether nearby
            // means this WiFi or Bluetooth range.
            mesh: self.mesh.as_ref().is_some_and(|radios| {
                radios
                    .mesh
                    .as_ref()
                    .is_some_and(|mesh| mesh.engine.reach().can_deliver())
                    || radios.ble.is_active()
            }),
        }
    }

    /// Seal a message onto the local network, if the mesh is up at all.
    ///
    /// `false` means nothing on this WiFi took the frame — no mesh running, no
    /// peers, or a payload too big to seal. Never an error: the caller falls
    /// back to a relay and the outbox keeps the message either way.
    async fn try_mesh(
        &self,
        peer: &PublicKey,
        message_id: &str,
        content: &str,
        reply_to: Option<&str>,
        created_at: u64,
    ) -> bool {
        let Some(radios) = &self.mesh else {
            return false;
        };
        radios
            .send(
                peer,
                message_id,
                content,
                reply_to.map(str::to_string),
                created_at,
            )
            .await
    }

    /// React to one of `peer`'s messages, or take an existing reaction back.
    ///
    /// **Toggling lives here, not in the frontends.** Tapping the emoji you
    /// already sent means "remove it", and tapping a different one means
    /// "replace"; deciding that needs the current reaction, which only this side
    /// knows. Putting it here is what keeps Android and Flutter from each having
    /// their own answer — and each having their own bug when the two disagree.
    ///
    /// Returns the reaction now standing, or `None` if the tap withdrew one.
    ///
    /// The local write happens **before** the send and is kept whatever the send
    /// does, so the chip appears the moment it is tapped and survives being
    /// offline. A reaction is deliberately **not** queued in the outbox on
    /// failure, unlike a message: the outbox persists `StoredMessage` rows, so a
    /// queued envelope would sit in the conversation as a JSON bubble, and a
    /// reaction is worth much less than the retry machinery would cost.
    /// The peer simply does not learn about it. That becomes worth revisiting if
    /// the outbox ever grows a non-chat lane — at which point reactions should
    /// use it.
    pub async fn toggle_reaction(
        &self,
        peer: &str,
        target_id: &str,
        emoji: &str,
    ) -> Result<Option<ReactionDto>, UiError> {
        if target_id.trim().is_empty() {
            return Err(UiError::Engine("no message to react to".into()));
        }
        // The same bound the parser enforces on the way in, applied on the way
        // out: this device must not send a peer something it would itself refuse.
        if emoji.is_empty() || emoji.len() > MAX_REACTION_BYTES {
            return Err(UiError::Engine(format!(
                "a reaction must be 1..={MAX_REACTION_BYTES} bytes"
            )));
        }
        let store = self.store.clone().ok_or(UiError::StoreLocked)?;
        let vault = self.vault.clone().ok_or(UiError::VaultLocked)?;
        let peer_pk = parse_pubkey(peer)?;
        let peer_npub = to_npub(peer);
        let me = self
            .identity
            .as_ref()
            .map(|i| i.npub.clone())
            .ok_or(UiError::NoIdentity)?;

        let current = store
            .reaction_by(target_id, &me)
            .map_err(|e| UiError::Storage(e.to_string()))?;
        // Tapping what you already sent takes it back; anything else replaces.
        let clearing = current.as_ref().map(|r| r.emoji.as_str()) == Some(emoji);
        let next = if clearing { "" } else { emoji };
        let created_at = now_secs();

        let row = comrade_storage::MessageReaction {
            target_id: target_id.to_string(),
            peer_npub: peer_npub.clone(),
            reactor_npub: me,
            emoji: next.to_string(),
            created_at,
            outgoing: true,
        };
        store
            .set_reaction(&row)
            .map_err(|e| UiError::Storage(e.to_string()))?;
        store.flush().map_err(|e| UiError::Storage(e.to_string()))?;

        let json = ReactionEnvelope::new(target_id, next)
            .to_json()
            .map_err(|e| UiError::Engine(e.to_string()))?;

        // Same transport precedence a message gets: the local mesh first when the
        // user has asked for it, a relay otherwise — a reaction sent across the
        // room should not need the internet either.
        let plan = SendPlan::for_attempt(self.prefer_local, self.reach(&vault).await, 0);
        let local_id = local_message_id(&peer_npub, &json, created_at);
        let on_mesh = plan.local_first
            && self
                .try_mesh(&peer_pk, &local_id, &json, None, created_at)
                .await;
        if !on_mesh || plan.force_both {
            if let Err(e) = vault.send_dm(&peer_pk, &json).await {
                if !plan.local_first {
                    self.try_mesh(&peer_pk, &local_id, &json, None, created_at)
                        .await;
                }
                // Logged, not returned: the local reaction stands either way, and
                // failing the call would make the UI un-draw a chip the user just
                // tapped over something they cannot act on.
                tracing::info!(error = %e, "reaction could not be published");
            }
        }

        Ok((!clearing).then(|| ReactionDto::from(row)))
    }

    /// Park a message in the sender outbox under a locally minted id and
    /// persist the queue, failing the oldest entry if the peer's queue was
    /// full (it is gone, and must stop showing as pending).
    fn enqueue(
        &self,
        message_id: &str,
        peer_npub: &str,
        content: &str,
        reply_to: Option<&str>,
        created_at: u64,
    ) {
        let queued = QueuedMessage::new(
            message_id,
            peer_npub,
            content,
            reply_to.map(str::to_string),
            created_at,
        );
        if let QueueOutcome::Displaced(dropped) = self.outbox.queue(queued) {
            self.mark_status(peer_npub, &[dropped], STATUS_FAILED);
        }
        if let Some(store) = &self.store {
            persist_outbox(store, &self.outbox);
        }
    }

    /// Count a delivery round against a queued message, and mark it failed once
    /// it has run out of them.
    fn record_attempt(&self, queued: &QueuedMessage) {
        if matches!(
            self.outbox
                .record_attempt(&queued.peer_npub, &queued.message_id),
            AttemptOutcome::Exhausted
        ) {
            self.mark_status(
                &queued.peer_npub,
                std::slice::from_ref(&queued.message_id),
                STATUS_FAILED,
            );
        }
    }

    /// Retry every queued message that is still worth retrying, and reap the
    /// ones that are not. Returns how many were accepted by a relay.
    ///
    /// Called on a cadence by the loop [`ComradeRuntime::spawn_event_loops`]
    /// starts (with the first tick at launch, which is when mail queued in a
    /// previous session should go out), and callable directly by a host that
    /// knows connectivity just came back.
    pub async fn flush_outbox(&self) -> Result<usize, UiError> {
        let vault = self.vault.clone().ok_or(UiError::VaultLocked)?;
        let now = now_secs();

        // Who each queued item was for, captured *before* pruning: a media
        // reference has no stored message row, so `peer_of_stored` cannot name
        // its conversation once the queue entry is gone.
        let queued_peers: std::collections::HashMap<String, String> = self
            .outbox
            .snapshot()
            .queues
            .into_iter()
            .flat_map(|(peer, queue)| {
                queue
                    .into_iter()
                    .map(move |m| (m.message_id, peer.clone()))
                    .collect::<Vec<_>>()
            })
            .collect();

        // Expired or attempt-exhausted mail: stop lying about it in the UI.
        for dropped in self.outbox.prune(now) {
            if let Some(peer) = self
                .peer_of_stored(&dropped)
                .or_else(|| queued_peers.get(&dropped).cloned())
            {
                self.mark_status(&peer, &[dropped], STATUS_FAILED);
            }
        }

        let due = self.outbox.due(now);
        if due.is_empty() {
            return Ok(0);
        }

        // Probed once for the whole flush, not per message: a batch of queued
        // mail goes out over the same network conditions, and asking per item
        // would scan the relay pool once per message for no new information.
        let reach = self.reach(&vault).await;

        // With no route at all, there is nothing to attempt — so do not spend
        // an attempt. [`comrade_core::dak::outbox::MAX_ATTEMPTS`] is 8 and the
        // flush cadence is a minute, so a flush that burned an attempt against
        // a network that was not there marked an offline message **failed
        // after eight minutes**, silently overriding the 24-hour
        // [`comrade_core::dak::outbox::TTL_SECS`] that is supposed to govern
        // how long off-grid mail waits. Two phones out of range for a coffee
        // break came back to a screen of red.
        //
        // Attempts now count delivery failures, which is what the cap is for.
        // The TTL above still runs — `prune` happened before this — so mail
        // genuinely too old to matter is still reaped, and the wake in
        // [`ComradeRuntime::spawn_mesh_status_forwarder`] brings the queue
        // straight back the moment a route reappears.
        if !reach.relay && !reach.mesh {
            tracing::debug!(
                queued = due.len(),
                "no transport reachable — holding queued mail rather than spending an attempt"
            );
            return Ok(0);
        }

        let mut sent = 0usize;
        for queued in due {
            let Ok(peer_pk) = parse_pubkey(&queued.peer_npub) else {
                // An unparseable recipient can never succeed; drop it rather
                // than retry it eight times.
                self.outbox
                    .ack(&queued.peer_npub, std::slice::from_ref(&queued.message_id));
                continue;
            };

            let plan = SendPlan::for_attempt(self.prefer_local, reach, queued.attempts);
            // Under local precedence, retry the WiFi first every round — a peer
            // may have joined the network since the last flush, and reaching
            // them there costs nothing. `force_both` is what stops that from
            // becoming an indefinite wait.
            let on_mesh = plan.local_first
                && self
                    .try_mesh(
                        &peer_pk,
                        &queued.message_id,
                        &queued.content,
                        queued.reply_to.as_deref(),
                        queued.queued_at,
                    )
                    .await;
            if on_mesh && !plan.force_both {
                self.record_attempt(&queued);
                continue;
            }

            match vault
                .send_dm_reply(&peer_pk, &queued.content, queued.reply_to.as_deref())
                .await
            {
                Ok(event_id) => {
                    self.outbox
                        .ack(&queued.peer_npub, std::slice::from_ref(&queued.message_id));
                    // A media reference has no stored text row. Re-keying it
                    // would *create* one whose body is the envelope JSON — a
                    // bubble full of machine-readable noise, and the chat
                    // list's preview. Its own NIP-94 event id is the handle the
                    // UI already holds, so that is what the status names.
                    let status_id = if is_media_envelope(&queued.content) {
                        queued.message_id.clone()
                    } else {
                        // Re-key the stored row to the relay's event id: a later
                        // delivered/read receipt names *that* id, so without
                        // this the ticks would never advance past `sent`.
                        self.rekey_stored_message(&queued, &event_id.to_hex());
                        event_id.to_hex()
                    };
                    let _ = self.events.send(BridgeEvent::MessageStatus {
                        peer: queued.peer_npub.clone(),
                        message_ids: vec![status_id],
                        status: "sent".into(),
                    });
                    share_profile_on_accept(
                        self.store.clone(),
                        self.vault.clone(),
                        self.username.clone(),
                        &queued.peer_npub,
                        &peer_pk,
                    );
                    sent += 1;
                }
                Err(e) => {
                    tracing::debug!(error = %e, "queued DM still undeliverable by relay");
                    // Still no relay — try the local network again on every
                    // flush, since a peer may have joined the WiFi since the
                    // last attempt. (Under local precedence that already
                    // happened above.)
                    if !plan.local_first {
                        self.try_mesh(
                            &peer_pk,
                            &queued.message_id,
                            &queued.content,
                            queued.reply_to.as_deref(),
                            queued.queued_at,
                        )
                        .await;
                    }
                    self.record_attempt(&queued);
                }
            }
        }

        if let Some(store) = &self.store {
            persist_outbox(store, &self.outbox);
        }
        Ok(sent)
    }

    /// Broadcast a Chitthi that is **not** signed by this device's identity.
    ///
    /// `scope` picks the privacy shape (adopted from bitchat's per-geohash
    /// identities, and the missing piece `AUDIT.md` §8 flagged for the anonymous
    /// thoughts pillar):
    ///  • `None` — a throwaway key per post. Two anonymous Chitthis cannot be
    ///    linked to each other, let alone to you.
    ///  • `Some(label)` — a stable persona derived from the device seed for that
    ///    label, so replies can reach the same pseudonym while it stays
    ///    unlinkable to your identity and to your other personas.
    ///
    /// The post is cached locally under its throwaway key so it appears in your
    /// own timeline. Nothing links it to the identity on any relay; the network
    /// layer (IP, timing) is a separate exposure this cannot close — see the
    /// engine docs on [`SabhaEngine::broadcast_anonymous_chitthi`].
    pub async fn broadcast_anonymous_chitthi(
        &self,
        content: &str,
        scope: Option<&str>,
    ) -> Result<String, UiError> {
        if content.trim().is_empty() {
            return Err(UiError::Engine("chitthi is empty".into()));
        }
        let sabha = self.sabha.clone().ok_or(UiError::VaultLocked)?;
        let store = self.store.clone().ok_or(UiError::VaultLocked)?;

        let signer = match scope {
            Some(label) => {
                let seed = load_or_create_device_seed(&store)?;
                anon::derive_scoped(&seed, anon::SCOPE_CHITTHI, label)
                    .map_err(|e| UiError::Engine(e.to_string()))?
            }
            None => anon::ephemeral(),
        };

        let id = sabha
            .broadcast_anonymous_chitthi(content, &signer)
            .await
            .map_err(|e| UiError::Engine(e.to_string()))?;

        let author_npub = signer
            .public_key()
            .to_bech32()
            .unwrap_or_else(|_| signer.public_key().to_hex());
        let row = comrade_storage::Chitthi {
            id: id.to_hex(),
            author_npub,
            content: content.to_string(),
            created_at: now_secs(),
            reply_to: None,
        };
        if let Err(e) = store.cache_chitthi(&row).and_then(|()| store.flush()) {
            warn!("failed to cache anonymous chitthi: {e}");
        }
        Ok(id.to_hex())
    }

    /// Persist a status change and announce it on the event bus.
    fn mark_status(&self, peer: &str, message_ids: &[String], status: &str) {
        if let Some(store) = &self.store {
            for id in message_ids {
                let _ = store.set_message_status(id, status);
            }
            let _ = store.flush();
        }
        let _ = self.events.send(BridgeEvent::MessageStatus {
            peer: peer.to_string(),
            message_ids: message_ids.to_vec(),
            status: status.to_string(),
        });
    }

    /// Which conversation a stored message belongs to, for a status update whose
    /// only handle is the message id.
    fn peer_of_stored(&self, message_id: &str) -> Option<String> {
        self.store
            .as_ref()?
            .get_message(message_id)
            .ok()
            .flatten()
            .map(|m| m.peer_npub)
    }

    /// Replace a queued row's local id with the relay's event id, keeping the
    /// message's place in the conversation.
    fn rekey_stored_message(&self, queued: &QueuedMessage, event_id: &str) {
        let Some(store) = &self.store else { return };
        let created_at = store
            .get_message(&queued.message_id)
            .ok()
            .flatten()
            .map(|m| m.created_at)
            .unwrap_or(queued.queued_at);
        let row = comrade_storage::StoredMessage {
            id: event_id.to_string(),
            peer_npub: queued.peer_npub.clone(),
            content: queued.content.clone(),
            created_at,
            outgoing: true,
            status: Some("sent".into()),
            reply_to: queued.reply_to.clone(),
        };
        let result = store
            .save_message(&row)
            .and_then(|()| store.remove_message(&queued.message_id).map(|_| ()))
            .and_then(|()| store.flush());
        if let Err(e) = result {
            warn!("failed to re-key a flushed message: {e}");
        }
    }

    /// Whether this device currently counts as online — see
    /// [`ComradeRuntime::presence_active`].
    pub fn presence_active(&self) -> bool {
        self.presence_active
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Announce presence to every comrade, one gift-wrapped beacon each, and
    /// return how many were accepted by a relay.
    ///
    /// This is the **frontend's** call, and it is what decides the answer:
    /// `online` is recorded, so the heartbeat afterwards keeps refreshing
    /// (or keeps quiet) accordingly. An Android shell calls it `true` when an
    /// Activity comes to the foreground and `false` when the last one goes
    /// away — the app being open is the thing "online" claims, not the
    /// process being alive.
    ///
    /// Never fails loudly: presence is a courtesy, and a relay hiccup must
    /// not surface as an error in a UI that only ever calls this in the
    /// background. Sends are sequential — the comrade list is a handful of
    /// people by design, and a burst of parallel gift wraps buys nothing.
    pub async fn announce_presence(&self, online: bool) -> u64 {
        self.presence_active
            .store(online, std::sync::atomic::Ordering::Relaxed);
        self.send_presence(online).await
    }

    /// The heartbeat's call: re-assert "online" **only while the app is
    /// open**.
    ///
    /// Without this gate the loop would undo every goodbye — a backgrounded
    /// phone would announce itself offline and then, a heartbeat later,
    /// cheerfully claim to be online again. Returns 0 while inactive, which
    /// is also what makes the whole feature free for a backgrounded app: no
    /// beacons, no relay traffic.
    pub async fn refresh_presence(&self) -> u64 {
        if !self.presence_active() {
            return 0;
        }
        self.send_presence(true).await
    }

    /// Fan one beacon out to every comrade. Shared by the two callers above so
    /// the "who gets told, and what" half stays in one place.
    async fn send_presence(&self, online: bool) -> u64 {
        let Some(vault) = self.vault.clone() else {
            return 0;
        };
        let Some(store) = self.store.as_deref() else {
            return 0;
        };
        let peers = comrade_peers(store);
        if peers.is_empty() {
            return 0;
        }
        let beacon = if online {
            PresenceBeacon::online()
        } else {
            PresenceBeacon::offline()
        };
        let Ok(json) = beacon.to_json() else {
            return 0;
        };
        let created_at = now_secs();
        let mut sent = 0u64;
        for peer in peers {
            match vault.send_dm(&peer, &json).await {
                Ok(_) => sent += 1,
                Err(e) => {
                    tracing::debug!(%peer, "presence beacon not sent by relay: {e}");
                    // A beacon no relay will take is exactly the case the dot
                    // is worst at: with the internet gone it keeps showing the
                    // last claim until the TTL runs out, so a comrade sitting
                    // next to you reads as online for eight minutes after they
                    // stop being, and someone who just arrived on this WiFi
                    // reads as offline. Sealing it onto the local network makes
                    // the dot mean "reachable" rather than "was reachable".
                    //
                    // Nothing is needed on the receiving side: an opened mesh
                    // frame runs through the same ingress as a relay DM, and
                    // that ingress already parses presence beacons.
                    let local_id = local_message_id(&peer.to_hex(), &json, created_at);
                    if self
                        .try_mesh(&peer, &local_id, &json, None, created_at)
                        .await
                    {
                        sent += 1;
                    }
                }
            }
        }
        sent
    }

    /// Send the nudge for every draft that has now been abandoned long enough
    /// to mean it — the sending half of [`comrade_core::nudge`]. Called on the
    /// presence sweep; returns how many nudges went out (0 whenever the
    /// feature is idle, which is almost always).
    ///
    /// Three gates, in this order, and each is load-bearing:
    ///  * **[`NudgeWatch::due`] decides**, so every timing rule lives in one
    ///    tested place and the cooldown is recorded at the moment we decide —
    ///    a relay that refuses the DM does not earn the writer a second nudge.
    ///  * **Only comrades are told.** Marking someone is what consents to
    ///    disclosing anything about being at your phone; without it there is
    ///    nothing to send and no one to send it to.
    ///  * **The store read happens here, not on the keystroke.** Watching a
    ///    composer must stay free, and the comrade flag can change between
    ///    typing and sending anyway — so the answer that counts is the one at
    ///    send time.
    ///
    /// Never fails: a locked vault, a peer un-chosen in the meantime, or an
    /// unreachable relay all just mean nothing is sent.
    ///
    /// `now` is passed in rather than read here so a test can stand on the far
    /// side of [`comrade_core::nudge::NUDGE_SETTLE_SECS`] without sleeping
    /// through it — the same
    /// injectable-clock treatment the presence labels get. Production has one
    /// call site, and it passes the real clock.
    pub async fn nudge_abandoned_drafts(&self, now: u64) -> u64 {
        // Take the decisions out from under the mutex *before* any await —
        // `due` hands back owned keys for exactly this reason.
        let peers = self.nudge_watch.due(now);
        if peers.is_empty() {
            return 0;
        }
        let (Some(vault), Some(store)) = (self.vault.clone(), self.store.as_deref()) else {
            return 0;
        };
        // Every store read happens here, before the first await — the same
        // shape [`Self::announce_presence`] uses, so the encrypted store is
        // never being read between two relay round-trips.
        let recipients: Vec<PublicKey> = peers
            .into_iter()
            .filter(|npub| {
                store
                    .get_contact(npub)
                    .ok()
                    .flatten()
                    .is_some_and(|c| c.comrade)
            })
            .filter_map(|npub| PublicKey::parse(&npub).ok())
            .collect();
        deliver_nudges(&vault, recipients).await
    }

    /// Tell every comrade, once, that this person might need them — the
    /// deliberate trigger, for someone reaching for the breathing screen
    /// (`docs/ATTENTION.md`) rather than giving up on a message.
    ///
    /// Returns how many nudges a relay accepted; `0` for someone who has
    /// chosen no comrades, which is what makes the whole thing free until
    /// somebody opts in.
    ///
    /// Deliberately the **same envelope** as the abandoned-draft trigger, not a
    /// second kind of message: a comrade learns "they might need you" and
    /// cannot tell which of the two happened, so this reason added nothing to
    /// what anyone learns. It shares the cooldown too — see
    /// [`comrade_core::nudge::nudged_recently`] — so tapping the button after a
    /// hard half-hour of writing and deleting does not page anyone twice.
    ///
    /// Never fails, like its neighbours: a locked vault, no comrades, or an
    /// unreachable relay all just mean nothing is sent.
    ///
    /// Reads the clock itself, unlike [`Self::nudge_abandoned_drafts`]: there
    /// is no settle window for a test to stand on the far side of here, and
    /// the one timing rule that does apply — the cooldown — is pinned in
    /// `comrade_core::nudge`'s own tests.
    pub async fn nudge_comrades(&self) -> u64 {
        let now = now_secs();
        let (Some(vault), Some(store)) = (self.vault.clone(), self.store.as_deref()) else {
            return 0;
        };
        // Both store reads — who the comrades are, and their keys — happen
        // before the cooldown is claimed, and the claim before any await.
        let comrades: Vec<String> = store
            .list_comrades()
            .unwrap_or_default()
            .into_iter()
            .map(|c| c.npub)
            .collect();
        if comrades.is_empty() {
            return 0;
        }
        let recipients: Vec<PublicKey> = self
            .nudge_watch
            .due_among(&comrades, now)
            .iter()
            .filter_map(|npub| PublicKey::parse(npub).ok())
            .collect();
        deliver_nudges(&vault, recipients).await
    }

    // ── Ride signals (driver + pillion — see `comrade_core::ride`) ───────────

    /// Say one catalog phrase to the other seat. `phrase` is the wire name
    /// ([`RidePhrase::as_str`]); an unknown one is refused here rather than
    /// sent, because the receiver would drop it whole anyway and "it sent"
    /// would be a lie.
    pub async fn ride_send_quick(&self, target: &str, phrase: &str) -> Result<(), UiError> {
        let phrase = RidePhrase::parse(phrase)
            .ok_or_else(|| UiError::Engine(format!("not a ride phrase: {phrase}")))?;
        self.ride_send(target, RideSignal::Quick { phrase }).await
    }

    /// Suggest the next maneuver to the person steering. `maneuver` is the
    /// wire name ([`RideManeuver::as_str`]); the note is trimmed and an empty
    /// one becomes no note, so a cleared composer field does not travel as
    /// `""`.
    pub async fn ride_send_route(
        &self,
        target: &str,
        maneuver: &str,
        distance_m: Option<u32>,
        note: Option<String>,
    ) -> Result<(), UiError> {
        let maneuver = RideManeuver::parse(maneuver)
            .ok_or_else(|| UiError::Engine(format!("not a ride maneuver: {maneuver}")))?;
        let note = note.map(|n| n.trim().to_string()).filter(|n| !n.is_empty());
        self.ride_send(
            target,
            RideSignal::Route {
                maneuver,
                distance_m,
                note,
            },
        )
        .await
    }

    /// The shared tail of both ride sends: the send-side half of the
    /// admissibility check ([`RideSignal::admissible`] — the receive-side half
    /// is inside [`parse_ride_envelope`]), then straight to the vault via
    /// [`Self::send_control_envelope`]. No outbox retry, deliberately: a ride
    /// signal is a claim about now, and "left in 400 m" delivered by a retry
    /// ten minutes later is not late, it is wrong. A send that fails is the
    /// sender's to repeat, and they are holding the phone that failed.
    async fn ride_send(&self, target: &str, signal: RideSignal) -> Result<(), UiError> {
        if !signal.admissible() {
            return Err(UiError::Engine(
                "ride note too long, or distance beyond the next maneuver".into(),
            ));
        }
        let json = RideEnvelope::new(signal, now_ms())
            .to_json()
            .map_err(|e| UiError::Engine(e.to_string()))?;

        // **Both radios and the relay, always** — and the "always" is the
        // decision. `send_together` returns as soon as a local radio takes the
        // frame, which is right for a session: `LocalRadios::send` reports only
        // that a radio *accepted* it, never that it arrived, and a together
        // session repairs a lost signal on its next heartbeat ten seconds
        // later.
        //
        // A ride signal has no such repair. There is no heartbeat, no Lamport
        // counter to notice a gap, and deliberately no outbox retry — so a
        // frame a radio swallowed is a "pull over" that silently never
        // happened. Against that, the cost of also publishing is one gift wrap
        // for a feature that emits a handful of signals on a whole ride, not
        // ten a second.
        //
        // The receiver collapses the pair: the two copies are byte-identical
        // (that is what `RideEnvelope::at_ms` is for) so the ride arm's
        // cross-transport dedup drops whichever lands second.
        //
        // This is also the single best case for the local radios in the whole
        // app, which is why it is worth the wire at all: two people on one
        // motorcycle are a metre apart, and the mobile data they are riding
        // through is the worst link either of them has. A LAN or BLE hop is
        // ~1-5 ms against a relay's hundreds — and unlike the WiFi mesh, which
        // is only up in the off-grid workspace, Bluetooth is always present
        // (see `send_together`'s note on `LocalRadios`).
        let peer_pk = parse_pubkey(target)?;
        let created_at = now_secs();
        let local_id = local_message_id(&to_npub(target), &json, created_at);
        self.try_mesh(&peer_pk, &local_id, &json, None, created_at)
            .await;
        self.send_control_envelope(target, &json).await
    }

    // ── Tasks and offers (see `comrade_core::karya`, `comrade_core::command`) ─

    /// Send a control envelope to `target` without it becoming a chat message.
    ///
    /// [`Self::send_dm`] is the *chat* path: it persists a `StoredMessage`,
    /// queues in the outbox, drives the chat-list preview and cancels the draft
    /// nudge. Putting an envelope through it puts raw JSON in the sender's own
    /// thread and in their chat list — the exact defect `AUDIT.md`'s 2026-07-29
    /// entry records for media references ("a chat bubble full of
    /// machine-readable noise, and the chat list's preview").
    ///
    /// So this goes straight to the vault, the way every other control envelope
    /// here already does ([`Self::send_call_signal`], `deliver_nudges`,
    /// [`Self::together_start`], the receipt sender). The cost is deliberate and
    /// worth naming: **no outbox retry**. A task or an offer no relay accepts is
    /// not queued for later, because a "would you do this?" that silently
    /// arrives an hour after the conversation moved on is worse than one the
    /// sender was told to re-send — and the local row exists either way.
    async fn send_control_envelope(&self, target: &str, json: &str) -> Result<(), UiError> {
        let vault = self.vault.clone().ok_or(UiError::VaultLocked)?;
        let peer = parse_pubkey(target)?;
        vault
            .send_dm(&peer, json)
            .await
            .map(|_| ())
            .map_err(|e| UiError::Engine(e.to_string()))
    }

    /// Name a piece of work. `peer` of `None` is a note to self.
    ///
    /// A note to self never reaches a relay — it is stored and returned, and
    /// that is the whole operation. An assignment stores the row *first* and
    /// then sends, so a relay that refuses does not lose the task the user
    /// typed; the assigner has a record of having asked either way, and the
    /// outbox is not used because a task nobody received is better re-asked than
    /// delivered silently an hour later.
    pub async fn assign_task(&self, peer: Option<String>, text: &str) -> Result<TaskDto, UiError> {
        let store = self.store.clone().ok_or(UiError::VaultLocked)?;
        let me = self
            .identity
            .as_ref()
            .map(|i| i.npub.clone())
            .ok_or(UiError::NoIdentity)?;
        let task = Task::new(
            new_task_id(),
            text,
            me.clone(),
            peer.as_deref().map(to_npub),
            now_secs(),
        );
        if task.text.is_empty() {
            return Err(UiError::Engine("a task needs to say what to do".into()));
        }
        store
            .save_task(&stored_task(&task))
            .and_then(|()| store.flush())
            .map_err(|e| UiError::Storage(e.to_string()))?;

        if let Some(target) = peer {
            let envelope = KaryaEnvelope::new(TaskSignal::Assign {
                id: task.id.clone(),
                text: task.text.clone(),
            });
            let json = envelope
                .to_json()
                .map_err(|e| UiError::Engine(e.to_string()))?;
            // One message, not two: the receiver renders its own chat bubble
            // from the envelope (`apply_karya_signal`) rather than us sending a
            // second, human-readable copy. Two events per assignment would
            // double the relay traffic and let the bubble disagree with the row.
            self.send_control_envelope(&target, &json).await?;
        }
        task_dto(stored_task(&task), &me).ok_or_else(|| UiError::Engine("bad task row".into()))
    }

    /// Move a task to `state` on this device's say-so, and tell the other party.
    ///
    /// The permission check is [`Task::apply`]'s, which is where the table
    /// lives; a refusal comes back as one error rather than three, because a
    /// caller learning *which* rule stopped them learns about a task that may
    /// not be theirs.
    pub async fn set_task_state(&self, id: &str, state: TaskState) -> Result<TaskDto, UiError> {
        let store = self.store.clone().ok_or(UiError::VaultLocked)?;
        let me = self
            .identity
            .as_ref()
            .map(|i| i.npub.clone())
            .ok_or(UiError::NoIdentity)?;
        let row = store
            .task(id)
            .map_err(|e| UiError::Storage(e.to_string()))?
            .ok_or_else(|| UiError::Engine("no such task".into()))?;
        let mut task =
            task_from_stored(&row).ok_or_else(|| UiError::Engine("bad task row".into()))?;
        if !task.apply(state, &me, now_secs()) {
            return Err(UiError::Engine("that is not yours to change".into()));
        }
        store
            .save_task(&stored_task(&task))
            .and_then(|()| store.flush())
            .map_err(|e| UiError::Storage(e.to_string()))?;

        // Whoever is on the other side needs to hear it. On a note to self there
        // is nobody, and no relay is touched.
        let other = if task.assigner_npub == me {
            task.assignee_npub.clone()
        } else {
            Some(task.assigner_npub.clone())
        };
        if let Some(target) = other.filter(|t| *t != me) {
            let json = KaryaEnvelope::new(TaskSignal::State {
                id: task.id.clone(),
                state,
            })
            .to_json()
            .map_err(|e| UiError::Engine(e.to_string()))?;
            if let Err(e) = self.send_control_envelope(&target, &json).await {
                // The local row already moved, and it is the copy this device
                // trusts. A peer who never hears will see it next time they act
                // on the task and are told it is already finished.
                tracing::debug!("task state not sent: {e}");
            }
        }
        task_dto(stored_task(&task), &me).ok_or_else(|| UiError::Engine("bad task row".into()))
    }

    // ── Threads and topics (see `comrade_core::topic`) ───────────────────────

    /// Name a topic in `peer`'s conversation, or return the one that already
    /// answers to that word.
    ///
    /// Stores first and then tells the peer, exactly as [`Self::assign_task`]
    /// does and for the same reason: a relay that refuses must not lose the
    /// topic the user typed. The envelope goes through
    /// [`Self::send_control_envelope`], so there is no outbox retry — a topic
    /// the peer never hears about is one they will hear about the next time
    /// something is filed under it, because [`Self::assign_thread`] re-announces
    /// the name with every filing.
    async fn upsert_topic(
        &self,
        peer_npub: &str,
        name: &str,
        announce: bool,
    ) -> Result<comrade_storage::StoredTopic, UiError> {
        let store = self.store.clone().ok_or(UiError::VaultLocked)?;
        let me = self
            .identity
            .as_ref()
            .map(|i| i.npub.clone())
            .ok_or(UiError::NoIdentity)?;
        let fresh = comrade_core::topic::Topic::new(name, peer_npub, &me, now_secs())
            .ok_or_else(|| UiError::Engine(TOPIC_NAME_REFUSED.into()))?;

        let existing = store
            .get_topic(peer_npub, &fresh.slug)
            .map_err(|e| UiError::Storage(e.to_string()))?;
        let row = match existing {
            Some(row) => row,
            None => {
                let row = stored_topic(&fresh);
                store
                    .save_topic(&row)
                    .and_then(|()| store.flush())
                    .map_err(|e| UiError::Storage(e.to_string()))?;
                row
            }
        };

        if announce {
            let json =
                comrade_core::topic::TopicEnvelope::new(comrade_core::topic::TopicSignal::Create {
                    slug: row.slug.clone(),
                    name: row.name.clone(),
                })
                .to_json()
                .map_err(|e| UiError::Engine(e.to_string()))?;
            if let Err(e) = self.send_control_envelope(peer_npub, &json).await {
                // The row is stored and this device can file against it. The
                // peer learns the name from the next filing, which carries it.
                tracing::debug!("topic not announced: {e}");
            }
        }
        Ok(row)
    }

    /// Name a topic and tell the peer. See [`ComradeRuntime::create_topic`].
    pub async fn create_topic(&self, peer: &str, name: &str) -> Result<TopicDto, UiError> {
        let peer_npub = to_npub(peer);
        let peer_hex = parse_pubkey(peer)?.to_hex();
        let row = self.upsert_topic(&peer_npub, name, true).await?;
        let store = self.store.as_ref().ok_or(UiError::VaultLocked)?;
        let me = self
            .identity
            .as_ref()
            .map(|i| i.npub.clone())
            .unwrap_or_default();
        let index = ThreadIndex::build(store, &peer_npub, &peer_hex)?;
        Ok(topic_dto(row, &me, &index))
    }

    /// File the thread containing `message_id` under `topic_name`, or unfile it.
    /// See [`ComradeRuntime::assign_thread`].
    ///
    /// Two envelopes, not one, when the topic is new: `Create` then `Assign`.
    /// Folding the name into the filing signal would mean a peer who receives a
    /// filing for a topic they have never heard of has to invent one from a
    /// slug, losing the spelling — and the two signals are separately useful
    /// (naming a topic before anything goes in it is how the picker gets
    /// populated).
    pub async fn assign_thread(
        &self,
        peer: &str,
        message_id: &str,
        topic_name: Option<String>,
    ) -> Result<ThreadSummaryDto, UiError> {
        let store = self.store.clone().ok_or(UiError::VaultLocked)?;
        let peer_npub = to_npub(peer);
        let peer_hex = parse_pubkey(peer)?.to_hex();
        let root_id = resolve_thread_root(&store, &peer_npub, message_id)?;

        let slug = match topic_name
            .as_deref()
            .map(str::trim)
            .filter(|n| !n.is_empty())
        {
            Some(name) => Some(self.upsert_topic(&peer_npub, name, true).await?.slug),
            None => None,
        };

        let filing = comrade_storage::ThreadTopic {
            root_id: root_id.clone(),
            peer_npub: peer_npub.clone(),
            slug: slug.clone(),
            updated_at: now_secs(),
        };
        // The local row is the copy this device trusts, and the store's own
        // replay guard is what decides whether this is news — so a filing the
        // guard refuses is still not an error to the caller, it is a filing
        // that was already true.
        store
            .set_thread_topic(&filing)
            .and_then(|c| store.flush().map(|()| c))
            .map_err(|e| UiError::Storage(e.to_string()))?;

        let json =
            comrade_core::topic::TopicEnvelope::new(comrade_core::topic::TopicSignal::Assign {
                root_id: root_id.clone(),
                slug,
            })
            .to_json()
            .map_err(|e| UiError::Engine(e.to_string()))?;
        if let Err(e) = self.send_control_envelope(&peer_npub, &json).await {
            tracing::debug!("thread filing not sent: {e}");
        }

        let index = ThreadIndex::build(&store, &peer_npub, &peer_hex)?;
        index
            .summary(&peer_npub, &root_id)
            .ok_or_else(|| UiError::Engine("that thread is not in this conversation".into()))
    }

    /// Archive a topic, or bring it back.
    /// See [`ComradeRuntime::set_topic_closed`].
    pub async fn set_topic_closed(
        &self,
        peer: &str,
        slug: &str,
        closed: bool,
    ) -> Result<TopicDto, UiError> {
        let store = self.store.clone().ok_or(UiError::VaultLocked)?;
        let peer_npub = to_npub(peer);
        let peer_hex = parse_pubkey(peer)?.to_hex();
        let slug = comrade_core::topic::slugify(slug)
            .ok_or_else(|| UiError::Engine(TOPIC_NAME_REFUSED.into()))?;
        let row = store
            .get_topic(&peer_npub, &slug)
            .map_err(|e| UiError::Storage(e.to_string()))?
            .ok_or_else(|| UiError::Engine("no such topic".into()))?;
        let mut topic = topic_from_stored(&row);
        topic.set_closed(closed, now_secs());
        let row = stored_topic(&topic);
        store
            .save_topic(&row)
            .and_then(|()| store.flush())
            .map_err(|e| UiError::Storage(e.to_string()))?;

        let json =
            comrade_core::topic::TopicEnvelope::new(comrade_core::topic::TopicSignal::Close {
                slug: topic.slug.clone(),
                closed,
            })
            .to_json()
            .map_err(|e| UiError::Engine(e.to_string()))?;
        if let Err(e) = self.send_control_envelope(&peer_npub, &json).await {
            tracing::debug!("topic close not sent: {e}");
        }

        let me = self
            .identity
            .as_ref()
            .map(|i| i.npub.clone())
            .unwrap_or_default();
        let index = ThreadIndex::build(&store, &peer_npub, &peer_hex)?;
        Ok(topic_dto(row, &me, &index))
    }

    /// Reply inside a thread. See [`ComradeRuntime::send_thread_reply`].
    pub async fn send_thread_reply(
        &self,
        peer: &str,
        root_id: &str,
        content: &str,
    ) -> Result<MessageDto, UiError> {
        let store = self.store.clone().ok_or(UiError::VaultLocked)?;
        let root = resolve_thread_root(&store, &to_npub(peer), root_id)?;
        self.send_dm_reply(peer, content, Some(&root)).await
    }

    /// Ask Tara **in front of the other person** — the `@tara …` spelling.
    ///
    /// WhatsApp's `@Meta AI`, with three differences that are the point of this
    /// app rather than incidental to it:
    ///
    /// 1. **The answer is computed here.** `ReflectiveCompanion` is on-device
    ///    template matching, so nothing about the question leaves the phone
    ///    except the two messages the user chose to send.
    /// 2. **The peer's messages are not passed to her.** Exactly as
    ///    [`ComradeRuntime::tara_aside`] documents: she answers the sentence she
    ///    was handed, not the conversation around it. The other person is a
    ///    *reader* here, never material.
    /// 3. **Distress never gets published.** If the question trips
    ///    `detect_distress`, this sends nothing at all and comes back with
    ///    `kept_private`. Someone typing the wrong sigil while in a bad place
    ///    must not have their crisis hand-off delivered into a chat, and the
    ///    grammar cannot tell that case from any other before the reply exists —
    ///    so the check has to be here, after the reply and before the send.
    ///
    /// Both messages are ordinary DMs. The answer carries
    /// `comrade_core::tara::TARA_CHAT_PREFIX` on the wire, and [`split_author`]
    /// turns that into [`MessageAuthor::Tara`] with the marker off the text, so
    /// both DTOs this returns are already in the form a bubble draws. The marker
    /// is a claim by the sending client, not an authentication — see
    /// [`MessageAuthor`].
    ///
    /// The private thread is left untouched: a question asked in front of
    /// somebody is not a turn in the private session, and merging the two would
    /// mean a shared chat could reshape (and be read out of) a journal-adjacent
    /// space the peer has no part in. The rotation seed therefore counts the Tara
    /// lines *in this thread* rather than that thread's turns.
    pub async fn tara_in_chat(&self, peer: &str, text: &str) -> Result<TaraChatDto, UiError> {
        let store = self.store.clone().ok_or(UiError::VaultLocked)?;
        let text = text.trim();
        if text.is_empty() {
            return Err(UiError::Engine("nothing was asked".into()));
        }
        let peer_npub = to_npub(peer);
        // Everything that reads the store happens before the first await, the
        // discipline every send path in this impl follows.
        let prior = store
            .messages_with(&peer_npub)
            .map_err(|e| UiError::Storage(e.to_string()))?
            .iter()
            .filter(|m| tara_chat_answer(&m.content).is_some())
            .count() as u64;

        let reply = ReflectiveCompanion.reply(text, prior);
        if reply.crisis {
            return Ok(TaraChatDto {
                asked: None,
                answered: None,
                reply: reply.text,
                kept_private: true,
                crisis: true,
            });
        }

        // The question first, so the thread reads in the order it happened even
        // if the second send is the one that fails.
        let asked = self.send_dm(&peer_npub, text).await?;
        // The answer *replies* to the question, which is both true and load
        // bearing: two messages sent in the same second carry the same
        // `created_at`, and the receiver's thread sorts on that — so arrival
        // order alone could show her answer above what it answered. The `e` tag
        // pairs them however they land.
        //
        // Only when the question actually reached a relay: an offline send comes
        // back with a local outbox id, and tagging that would name an event no
        // relay has ever seen.
        let reply_to = Some(asked.id.as_str())
            .filter(|id| !comrade_core::dak::outbox::is_local_message_id(id));
        let answered = self
            .send_dm_reply(&peer_npub, &tara_chat_line(&reply.text), reply_to)
            .await?;
        Ok(TaraChatDto {
            asked: Some(asked),
            answered: Some(answered),
            reply: reply.text,
            kept_private: false,
            crisis: false,
        })
    }

    /// Hand one journal entry to one person, as an ordinary DM.
    ///
    /// The body is `comrade_core::note`'s wire form, so the peer's Comrade
    /// draws a note card and any other Nostr client shows text that still says
    /// where it came from. The returned [`MessageDto`] already carries the
    /// parsed [`MessageDto::shared_note`] — [`read_body`] does that for every
    /// message — so the sender's own thread draws the same card immediately.
    ///
    /// Three things this deliberately does **not** do:
    ///
    /// - **It does not touch the entry.** No "shared" flag, no second copy
    ///   anywhere but the message history, so the journal screen looks exactly
    ///   as it did. Deleting the entry afterwards still works and still does not
    ///   reach into the peer's thread: a delivered message is theirs, and an
    ///   unsend that cannot be enforced is worse than none.
    /// - **It shares one entry.** There is no bulk form, no date range, no
    ///   export. Disclosing two entries takes two deliberate choices, which is
    ///   what makes an accidental disclosure of the *whole* journal impossible
    ///   rather than merely discouraged.
    /// - **It does not screen the words.** [`Self::tara_in_chat`] withholds a
    ///   distressed question because the user was addressing the *companion*
    ///   and a crisis hand-off must not be published on a mistyped sigil. Here
    ///   the user picked an entry and then picked a person: saying "this is
    ///   what my week was like" to someone who cares is what this pillar exists
    ///   to make easier, and refusing at that exact moment would be the app
    ///   deciding a bad week is too shameful to say out loud.
    ///
    /// The store read finishes before the first await — the discipline every
    /// send path in this impl follows.
    pub async fn share_journal_entry(
        &self,
        peer: &str,
        entry_id: &str,
    ) -> Result<MessageDto, UiError> {
        let store = self.store.clone().ok_or(UiError::VaultLocked)?;
        let entry = store
            .journal_entry(entry_id)
            .map_err(|e| UiError::Storage(e.to_string()))?
            // Deleted between opening the picker and choosing somebody. Say so
            // rather than sending a card with nothing in it.
            .ok_or_else(|| UiError::Engine("that journal entry is gone".into()))?;
        // What travels is the words. A video entry's recording has no share
        // path at all — nothing here uploads a file — so an entry that is only
        // a recording has nothing to send, and sending the mood marker alone
        // would put a card in someone's chat that says less than nothing about
        // what was actually shared.
        if entry.text.trim().is_empty() {
            return Err(UiError::Engine(
                "that entry has no words to send — sharing sends what you wrote, not a recording"
                    .into(),
            ));
        }
        let line = comrade_core::note::journal_note_line(&entry.text, entry.mood.as_deref());
        self.send_dm(peer, &line).await
    }

    /// Offer an in-app action to `peers`, reporting who was told and who was not.
    ///
    /// Three gates, and each one is a lesson this repo already paid for:
    ///
    /// 1. **Comrades only.** Marking someone a comrade is the existing
    ///    "this person may reach me" grant ([`ComradeRuntime::set_comrade`]);
    ///    an offer is a notification, so it lives inside that grant rather
    ///    than beside it. Anyone named who is not one comes back in
    ///    [`OfferOutcomeDto::not_comrades`] so the UI can say which.
    /// 2. **The shared nudge cooldown.** `comrade_core::nudge::nudged_recently`
    ///    is a floor on *notifications*, not on any one reason for them — the
    ///    reasoning `AUDIT.md` records for the breathing screen's own trigger.
    ///    Someone told twenty minutes ago that a comrade might need them learns
    ///    nothing from being told to breathe now, and being able to send this
    ///    repeatedly would make it a way to needle somebody.
    /// 3. **A control envelope, not a chat message** — see
    ///    [`Self::send_control_envelope`] for why, and for what that costs.
    ///
    /// Never partially fails in the sense of erroring: an unreachable comrade
    /// lands in [`OfferOutcomeDto::failed`] and the rest still go.
    pub async fn offer_action(
        &self,
        action: AppAction,
        peers: Vec<String>,
    ) -> Result<OfferOutcomeDto, UiError> {
        let store = self.store.clone().ok_or(UiError::VaultLocked)?;
        let now = now_secs();
        // Everything that reads the store, and the cooldown claim, happen before
        // the first await — the discipline every send path here follows.
        let comrades: std::collections::HashSet<String> = store
            .list_comrades()
            .map_err(|e| UiError::Storage(e.to_string()))?
            .into_iter()
            .map(|c| c.npub)
            .collect();

        let mut outcome = OfferOutcomeDto {
            sent: Vec::new(),
            not_comrades: Vec::new(),
            on_cooldown: Vec::new(),
            failed: Vec::new(),
        };
        let mut wanted: Vec<String> = Vec::new();
        for npub in peers.iter().map(|p| to_npub(p)) {
            if comrades.contains(&npub) {
                wanted.push(npub);
            } else {
                outcome.not_comrades.push(npub);
            }
        }
        if wanted.is_empty() {
            return Ok(outcome);
        }
        // `due_among` claims the cooldown for everyone it returns, so whoever it
        // leaves out is on cooldown by definition.
        let due = self.nudge_watch.due_among(&wanted, now);
        outcome.on_cooldown = wanted
            .iter()
            .filter(|npub| !due.contains(npub))
            .cloned()
            .collect();
        if due.is_empty() {
            return Ok(outcome);
        }

        let json = OfferEnvelope::new(action)
            .to_json()
            .map_err(|e| UiError::Engine(e.to_string()))?;
        for target in due {
            match self.send_control_envelope(&target, &json).await {
                Ok(()) => outcome.sent.push(target),
                Err(e) => {
                    tracing::debug!(%target, "offer not sent: {e}");
                    outcome.failed.push(target);
                }
            }
        }
        Ok(outcome)
    }

    pub async fn broadcast_chitthi(
        &self,
        content: &str,
        reply_to: Option<String>,
    ) -> Result<String, UiError> {
        let sabha = self.sabha.clone().ok_or(UiError::VaultLocked)?;

        let parent = match reply_to.as_deref() {
            Some(hex) => Some(
                EventId::from_hex(hex)
                    .map_err(|e| UiError::Engine(format!("invalid reply_to id: {e}")))?,
            ),
            None => None,
        };

        let id = sabha
            .broadcast_chitthi_reply(content, parent)
            .await
            .map_err(|e| UiError::Engine(e.to_string()))?;

        // Best-effort: persist our own Chitthi to the encrypted cache so it
        // shows up in the offline timeline immediately.
        if let Some(store) = &self.store {
            let row = comrade_storage::Chitthi {
                id: id.to_hex(),
                author_npub: self
                    .identity
                    .as_ref()
                    .map(|i| i.npub.clone())
                    .unwrap_or_default(),
                content: content.to_string(),
                created_at: now_secs(),
                reply_to,
            };
            if let Err(e) = store.cache_chitthi(&row).and_then(|()| store.flush()) {
                warn!("failed to cache outgoing chitthi: {e}");
            }
        }

        Ok(id.to_hex())
    }

    pub async fn send_call_signal(
        &self,
        peer: &str,
        call_id: &str,
        media: &str,
        signal_json: &str,
    ) -> Result<(), UiError> {
        let vault = self.vault.clone().ok_or(UiError::VaultLocked)?;
        let peer_pk = parse_pubkey(peer)?;
        let signal: CallSignal = serde_json::from_str(signal_json)
            .map_err(|e| UiError::Engine(format!("invalid call signal: {e}")))?;
        let env = CallEnvelope::new(
            call_id.to_string(),
            CallMediaKind::from_str_lenient(media),
            signal,
        );
        let json = env.to_json().map_err(|e| UiError::Engine(e.to_string()))?;
        let kind = env.signal.kind_str();

        // Retry a refused signal rather than dropping it. `send_dm` errors when
        // no relay *accepted* the event, and the case that matters is a relay
        // rate-limiting trickled ICE: a call publishes one gift-wrapped event
        // per candidate, so ten to thirty land within a couple of seconds while
        // the offer and answer — one each, seconds apart — sail through. Losing
        // only the candidates leaves both ends on "Connecting…" until the
        // connect timeout, which is the shape of the bug this guards.
        //
        // See `call_signal_retry_delay_ms` for why the budget is ~1.5s: long
        // enough to outlast a rate-limit window, short enough that a signal is
        // never delivered after the peer has already given up on the call.
        let mut attempt = 1u32;
        loop {
            let err = match vault.send_dm(&peer_pk, &json).await {
                Ok(_) => {
                    if attempt > 1 {
                        tracing::info!(kind, call_id, attempt, "call signal sent after a retry");
                    }
                    return Ok(());
                }
                Err(e) => e,
            };
            let Some(delay_ms) = call_signal_retry_delay_ms(attempt) else {
                // Loud, and specific about which signal was lost: a dropped
                // candidate used to be a debug line nobody would ever correlate
                // with a call that would not connect.
                tracing::warn!(
                    kind,
                    call_id,
                    attempts = attempt,
                    error = %err,
                    "call signal could not be delivered to any relay",
                );
                return Err(UiError::Engine(err.to_string()));
            };
            tracing::debug!(kind, call_id, attempt, error = %err, "call signal refused; retrying");
            tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
            attempt += 1;
        }
    }

    /// Send one together envelope, stamping it with our clock and whatever echo
    /// we owe the peer, and remembering the stamp so their echo can be matched
    /// back to it. The session lock is taken, used, and **dropped before the
    /// send** — the rule that has already cost this repo two shipped deadlocks.
    async fn send_together(&self, signal: TogetherSignal) -> Result<(), UiError> {
        let at_ms = now_ms();
        let (peer_hex, env, session_id, direct_ready) = {
            let mut guard = self.together.lock().unwrap();
            let session = guard
                .as_mut()
                .ok_or_else(|| UiError::Engine("no watch-together session is running".into()))?;
            session.note_send(at_ms);
            let env = TogetherEnvelope::new(
                session.id.clone(),
                session.applied.seq,
                at_ms,
                session.echo_back,
                signal,
            );
            (
                session.peer_hex.clone(),
                env,
                session.id.clone(),
                session.direct_ready && direct_path_live(session.direct_evidence_ms, at_ms),
            )
        };
        let peer_pk = parse_pubkey(&peer_hex)?;
        let json = env.to_json().map_err(|e| UiError::Engine(e.to_string()))?;
        // Prefer the local mesh when one is running. This is the single biggest
        // lever on how tight the sync can be: the deadband is floored by half
        // the round trip, and a LAN hop is ~1-5 ms against a relay's hundreds.
        // A together signal is also exactly the kind of traffic the mesh suits —
        // small, frequent, and worthless once stale.
        //
        // **This comment used to claim `mesh` is `Some` only in the off-grid
        // workspace, and that was stale from the day `LocalRadios` replaced
        // `MeshLink` here.** It cost real debugging time: a reader chasing "why
        // does a together session not work on a hotspot" concludes from it that
        // no local route is even attempted, and goes looking somewhere else.
        //
        // What is actually true: `RuntimeHandles::mesh_link` returns `Some`
        // whenever there are identity keys, and `LocalRadios` is *two* radios —
        // the WiFi mesh, which is indeed `None` outside the off-grid workspace,
        // **and Bluetooth, which is always present** and inert only until a
        // platform radio marks it active. So a together signal does get a local
        // attempt, on BLE at minimum, before either rung below.
        //
        // What remains genuinely missing is starting the *WiFi* mesh for a
        // session, which is still engine-lifecycle work (AUDIT A1 /
        // `docs/COMMS_ARCHITECTURE.md` ADR-4) and deliberately not done here.
        if let Some(mesh) = &self.mesh {
            let created = now_secs();
            let id = local_message_id(&to_npub(&peer_hex), &json, created);
            if mesh.send(&peer_pk, &id, &json, None, created).await {
                return Ok(());
            }
        }
        // Then a direct peer channel, when the frontend has one up. Second
        // rather than first only because the mesh above is a LAN hop and this
        // is usually an internet one — but against a relay it is the difference
        // between tens of milliseconds and hundreds, and since the deadband is
        // floored by half the round trip, it is the difference between a
        // correction that can be tight and one that cannot.
        //
        // Fire-and-forget by construction: the frontend owns the socket, so
        // "did it arrive" is not answerable here and is not asked. What *is*
        // asked is whether the channel has shown any sign of life in the last
        // two heartbeats — `direct_path_live`, folded into `direct_ready`
        // above. Without that, a frontend that lost its channel without
        // reporting it would keep this branch sending into a socket nobody
        // reads until the session died on its TTL.
        if direct_ready {
            let _ = self.events.send(BridgeEvent::TogetherOutbound {
                session_id,
                json: json.clone(),
            });
            return Ok(());
        }
        // Only the relay leg needs the vault, and it is fetched here rather than
        // up front so the two faster rungs are not gated on something they do
        // not use. In practice a locked vault has already cleared the session,
        // so this is a belt on top of a brace — but a `VaultLocked` raised
        // before a mesh or direct send would be a lie about why nothing went.
        let vault = self.vault.clone().ok_or(UiError::VaultLocked)?;
        vault
            .send_dm(&peer_pk, &json)
            .await
            .map_err(|e| UiError::Engine(e.to_string()))?;
        Ok(())
    }

    /// Invite `peer` to watch or listen to `content` together, and become the
    /// session's leader (the side that does not drift-correct).
    pub async fn together_start(
        &self,
        peer: &str,
        content: TogetherContent,
    ) -> Result<TogetherSessionDto, UiError> {
        let vault = self.vault.clone().ok_or(UiError::VaultLocked)?;
        // The same predicate the receiving side runs, so a thing we would refuse
        // to accept is a thing we refuse to send — and so a new content variant
        // cannot slip past one arm while satisfying the other.
        if !content.admissible() {
            return Err(UiError::Engine(
                "that link isn't something Comrade will play".into(),
            ));
        }
        let peer_pk = parse_pubkey(peer)?;
        let peer_npub = to_npub(peer);
        let our_npub = self
            .keys
            .as_ref()
            .map(|k| to_npub(&k.public_key().to_hex()))
            .unwrap_or_default();
        let at_ms = now_ms();
        let session_id = comrade_core::together::new_session_id();
        let (dto, env) = {
            let mut guard = self.together.lock().unwrap();
            if guard.is_some() {
                return Err(UiError::Engine(
                    "already watching something together".into(),
                ));
            }
            let mut session = TogetherSession {
                id: session_id.clone(),
                peer: peer_npub.clone(),
                peer_hex: peer_pk.to_hex(),
                content: content.clone(),
                we_lead: true,
                our_npub: our_npub.clone(),
                joined: false,
                applied: CommandStamp::new(1, our_npub, true),
                local_pos_ms: 0,
                local_playing: false,
                peer_pos_ms: 0,
                peer_playing: false,
                peer_at_ms: at_ms,
                last_heard_ms: at_ms,
                last_seek_ms: 0,
                direct_ready: false,
                direct_evidence_ms: 0,
                local_rate: 1.0,
                local_output_latency_ms: 0,
                peer_output_latency_ms: 0,
                clock: ClockFilter::new(),
                sent_at_ms: std::collections::VecDeque::new(),
                echo_back: None,
            };
            session.note_send(at_ms);
            let env = TogetherEnvelope::new(
                session_id.clone(),
                1,
                at_ms,
                None,
                TogetherSignal::Start {
                    content,
                    pos_ms: 0,
                    playing: false,
                },
            );
            let dto = session.dto();
            *guard = Some(session);
            (dto, env)
        };
        let json = env.to_json().map_err(|e| UiError::Engine(e.to_string()))?;
        if let Err(e) = vault.send_dm(&peer_pk, &json).await {
            // The invitation never left, so do not leave a session behind
            // claiming otherwise.
            *self.together.lock().unwrap() = None;
            return Err(UiError::Engine(e.to_string()));
        }
        self.spawn_together_loop();
        Ok(dto)
    }

    /// Accept the invitation we were sent.
    pub async fn together_join(&self) -> Result<(), UiError> {
        {
            let mut guard = self.together.lock().unwrap();
            let session = guard
                .as_mut()
                .ok_or_else(|| UiError::Engine("nothing to join".into()))?;
            session.joined = true;
        }
        self.send_together(TogetherSignal::Join).await?;
        self.spawn_together_loop();
        Ok(())
    }

    /// Play, pause or seek — one signal, because all three are the same
    /// statement. Bumps the Lamport counter, so this command outranks anything
    /// either side had applied before it.
    pub async fn together_set_state(
        &self,
        pos_ms: u64,
        playing: bool,
        effective_in_ms: u64,
    ) -> Result<(), UiError> {
        let at_ms = now_ms();
        {
            let mut guard = self.together.lock().unwrap();
            let session = guard
                .as_mut()
                .ok_or_else(|| UiError::Engine("no watch-together session is running".into()))?;
            let our_npub = session.our_npub.clone();
            session.applied = CommandStamp::new(session.applied.seq + 1, our_npub, !playing);
            session.local_pos_ms = pos_ms;
            session.local_playing = playing;
        }
        // `effective_in_ms` is a promise by the caller that it will apply the
        // change at that instant on its own player too. A frontend that can
        // defer (any native player can) uses it and both sides change state on
        // the same tick; one that cannot passes 0 and the receiver projects
        // instead. Either way the receiver never adopts a stale position.
        self.send_together(TogetherSignal::State {
            pos_ms,
            playing,
            effective_at_ms: Some(at_ms.saturating_add(effective_in_ms)),
        })
        .await
    }

    /// Leave. Best-effort on the wire — a session the other side never hears
    /// about ending simply ages out on their TTL, which is what that exists for.
    pub async fn together_end(&self) -> Result<(), UiError> {
        let ended = {
            let guard = self.together.lock().unwrap();
            guard.as_ref().map(|s| (s.id.clone(), s.peer.clone()))
        };
        let Some((session_id, peer)) = ended else {
            return Ok(());
        };
        let sent = self.send_together(TogetherSignal::End).await;
        *self.together.lock().unwrap() = None;
        let _ = self.events.send(BridgeEvent::TogetherEnded {
            session_id,
            peer,
            by_peer: false,
        });
        sent
    }

    /// Send one step of the file handover to the other side.
    ///
    /// A thin pass-through on purpose. The negotiation state machine lives in
    /// the frontend next to the peer connection it is negotiating; duplicating
    /// it here would create two machines that have to agree about a connection
    /// only one of them can see.
    ///
    /// It does hold one line, though: a share signal is only sendable inside a
    /// live session, because [`Self::send_together`] refuses otherwise. That is
    /// what stops this becoming a way to open a peer-to-peer connection to
    /// someone who never agreed to watch anything with you.
    pub async fn together_share(&self, signal: ShareSignal) -> Result<(), UiError> {
        self.send_together(TogetherSignal::Share { signal }).await
    }

    /// The frontend telling us whether it has a direct peer channel up for the
    /// running session.
    ///
    /// Idempotent and safe to call with no session — a channel that opens after
    /// one has ended is simply nothing to record.
    ///
    /// Should be set back to `false` the moment the channel closes or fails —
    /// but the runtime does not depend on that happening, because a frontend
    /// that has crashed past its own close handler cannot report anything. A
    /// declaration is treated as a claim with an expiry: two heartbeats of
    /// silence on the channel and sends go back to the relay on their own
    /// ([`comrade_core::together::direct_path_live`]). Reporting `false`
    /// promptly is still worth doing — it moves the fallback from twenty
    /// seconds away to immediate.
    pub fn together_direct_ready(&self, ready: bool) {
        if let Some(session) = self.together.lock().unwrap().as_mut() {
            session.direct_ready = ready;
            if ready {
                session.direct_evidence_ms = now_ms();
            }
        }
    }

    /// An envelope that arrived over the direct peer channel.
    ///
    /// **Deliberately less privileged than the relay path**, in two ways that
    /// are the whole of why this is safe to expose:
    ///
    /// 1. **It cannot create a session.** A `start` here is dropped. The channel
    ///    only exists because a session was negotiated inside one, so a channel
    ///    that could open a session would be an inversion — and it is the one
    ///    signal the relay's per-message authentication is genuinely load-bearing
    ///    for.
    /// 2. **The sender is the session's peer, by definition, not by claim.** The
    ///    identity comes from the session we are already in; nothing in the
    ///    payload is consulted for it. A relay message proves who sent it with
    ///    NIP-44; a data channel proves only "whoever is on the far end of this
    ///    DTLS connection", which is the peer precisely because the connection
    ///    was negotiated with them and never renegotiated.
    ///
    /// Everything past that — the age gate, session scoping, `(seq, actor)`
    /// ordering — is the same code the relay path runs, because it is the same
    /// call.
    pub fn together_receive_direct(&self, json: &str) {
        let Some(env) = parse_together_envelope(json) else {
            return;
        };
        if !direct_signal_admissible(&env.signal) {
            tracing::debug!("refusing a together invite arriving over a direct channel");
            return;
        }
        let known = {
            let mut guard = self.together.lock().unwrap();
            guard.as_mut().map(|s| {
                // The channel just carried something, which is the only proof
                // available that it is still carrying anything — see
                // `direct_path_live`. Stamped on *our* clock rather than from
                // `env.at_ms`, because the envelope's stamp is the sender's
                // claim and a peer that dated it into next week would otherwise
                // buy their channel a permanent reprieve.
                s.direct_evidence_ms = now_ms();
                (s.peer.clone(), s.peer_hex.clone())
            })
        };
        let Some((peer_npub, peer_hex)) = known else {
            return;
        };
        let link = TogetherLink {
            session: self.together.clone(),
            starts_seen: self.together_starts_seen.clone(),
            shares_seen: self.together_shares_seen.clone(),
        };
        // The envelope's own stamp stands in for the gift wrap's `created_at`:
        // there is no wrap here, and `at_ms` is the sender's claim about when
        // they sent it — which is exactly what the relay path's `created_at`
        // is too, in the same units once divided down.
        //
        // And `None` for the event id for the same reason there is no wrap:
        // this arrived on a live channel, which nothing replays. The redelivery
        // the share guard exists for is the relay backfill, and the only id
        // available here would be one derived from the payload — which would
        // drop a *second* legitimate signal with the same bytes rather than a
        // second copy of one.
        handle_together_envelope(
            &self.events,
            &link,
            &peer_npub,
            &peer_hex,
            env.at_ms / 1000,
            None,
            env,
        );
    }

    /// Carry one step of a large-attachment handoff to `peer`.
    ///
    /// Deliberately **not** routed through [`Self::send_together`]: that refuses
    /// outside a live session, which is right for a playhead and wrong for an
    /// attachment — nobody starts a watch-together session to send a video file.
    /// The gate a handoff gets instead is the one on receipt
    /// ([`IncomingGate::Accepted`]), which is the same bar a call signal has to
    /// clear, and for the same reason: both open a peer connection and both leak
    /// ICE candidates to whoever is on the other end. A stranger cannot get that
    /// far, and an accepted contact still has to be told and still has to agree —
    /// [`HandoffSignal::Accept`] comes from a person pressing a button, not from
    /// this runtime.
    ///
    /// Relay-first, unlike a together signal. A handoff is a handful of messages
    /// over the life of one transfer rather than a heartbeat every ten seconds,
    /// so the mesh's latency advantage buys nothing here — and the mesh reaches
    /// only the local network, where a large file would have found a `host`
    /// candidate anyway.
    pub async fn attachment_handoff_send(
        &self,
        peer: &str,
        transfer_id: &str,
        signal: HandoffSignal,
    ) -> Result<(), UiError> {
        let vault = self.vault.clone().ok_or(UiError::VaultLocked)?;
        let peer_pk = parse_pubkey(peer)?;
        let json = HandoffEnvelope::new(transfer_id, signal)
            .to_json()
            .map_err(|e| UiError::Engine(e.to_string()))?;
        vault
            .send_dm(&peer_pk, &json)
            .await
            .map_err(|e| UiError::Engine(e.to_string()))?;
        Ok(())
    }

    /// One pass of the session loop: expire a session we have stopped hearing
    /// from, or tell the other side where we are.
    async fn together_tick(&self) {
        let at = now_ms();
        let expired = {
            let mut guard = self.together.lock().unwrap();
            match guard.as_ref() {
                Some(s) if !session_is_live_at(s.last_heard_ms, at) => {
                    let ended = Some((s.id.clone(), s.peer.clone()));
                    *guard = None;
                    ended
                }
                _ => None,
            }
        };
        if let Some((session_id, peer)) = expired {
            let _ = self.events.send(BridgeEvent::TogetherEnded {
                session_id,
                peer,
                by_peer: false,
            });
            return;
        }
        // A paused session drifts by exactly nothing, so it says nothing: a
        // heartbeat is a persistent gift wrap, and one carrying no news is pure
        // metadata on someone else's relay.
        let beat = {
            let guard = self.together.lock().unwrap();
            guard
                .as_ref()
                // While bursting, a paused session still probes: the clock has
                // to be converged *before* anyone presses play, or the first
                // minute is the worst-synced part of the session.
                .filter(|s| s.joined && (s.local_playing || s.clock.len() < CLOCK_BURST_PROBES))
                .map(|s| TogetherSignal::Heartbeat {
                    pos_ms: s.local_pos_ms,
                    playing: s.local_playing,
                    applied_seq: s.applied.seq,
                    output_latency_ms: s.local_output_latency_ms,
                })
        };
        if let Some(beat) = beat {
            let _ = self.send_together(beat).await;
        }
    }

    /// Run the session loop until the session is gone.
    ///
    /// Detached rather than tracked, and self-terminating rather than aborted:
    /// once [`ComradeRuntime::lock_vault`] has cleared the session there is
    /// nothing left to cancel, because the very next tick finds nothing and
    /// stops. A tracked handle would buy an earlier exit for a loop that is
    /// already doing nothing.
    fn spawn_together_loop(&self) {
        let handles = self.clone();
        tokio::spawn(async move {
            loop {
                // The interval is re-read every pass rather than fixed up front:
                // a session bursts probes until its clock has converged and then
                // settles to the slow tail (`heartbeat_interval_ms`). A
                // `tokio::time::interval` cannot change period, and this loop
                // has no catch-up semantics to preserve — a missed tick should
                // simply be the next tick.
                let probes = match handles.together.lock().unwrap().as_ref() {
                    None => return,
                    Some(session) => session.clock.len(),
                };
                tokio::time::sleep(std::time::Duration::from_millis(heartbeat_interval_ms(
                    probes,
                )))
                .await;
                if handles.together.lock().unwrap().is_none() {
                    return;
                }
                handles.together_tick().await;
            }
        });
    }

    pub async fn hangup_call(
        &self,
        peer: &str,
        call_id: &str,
        media: &str,
        reason: &str,
    ) -> Result<(), UiError> {
        let signal = CallSignal::Hangup {
            reason: HangupReason::from_str_lenient(reason),
        };
        let json = serde_json::to_string(&signal).map_err(|e| UiError::Engine(e.to_string()))?;
        self.send_call_signal(peer, call_id, media, &json).await
    }

    pub async fn upload_and_send_media(
        &self,
        target_pubkey: &str,
        bytes: Vec<u8>,
        mime_type: &str,
        caption: &str,
    ) -> Result<MediaMessageDto, UiError> {
        if bytes.len() > MAX_MEDIA_BYTES {
            return Err(UiError::Engine(format!(
                "media is {} bytes; the limit is {MAX_MEDIA_BYTES}",
                bytes.len()
            )));
        }
        // Fail before the upload, not after it: a zero-byte blob is a picker or
        // permission failure on the frontend's side (a content URI that could
        // not be read yields exactly this), and sending it would cost a real
        // upload and put an undecodable bubble in both people's threads.
        if bytes.is_empty() {
            return Err(UiError::Engine("attachment is empty".into()));
        }
        let mime_type = validate_mime_type(mime_type)?;
        let mime_type = mime_type.as_str();
        let caption = sanitise_untrusted_text(caption, MAX_CAPTION_LEN);
        let caption = caption.as_str();
        // The vault is needed to deliver the reference — check before uploading
        // so a locked vault never leaves a paid-for blob on a host with nothing
        // pointing at it.
        let vault = self.vault.clone().ok_or(UiError::VaultLocked)?;
        let keys = self.keys.clone().ok_or(UiError::NoIdentity)?;
        let peer = parse_pubkey(target_pubkey)?;
        let key = derive_media_key(keys.secret_key(), &peer, MEDIA_LABEL)
            .map_err(|e| UiError::Engine(e.to_string()))?;

        let (media, _secret) =
            encrypt_media(&bytes, mime_type, &key).map_err(|e| UiError::Engine(e.to_string()))?;
        let size = media.size as u64;
        let sha256_hex = media.sha256_hex.clone();

        // Upload ciphertext only — the host sees opaque bytes, and is *told*
        // opaque bytes. This used to send the plaintext's own MIME type as the
        // upload's `Content-Type`, which contradicted the sentence above: it
        // told the media host whether the user had just sent a photo, a voice
        // note or a PDF, when the body it was describing is AES-GCM ciphertext
        // and could not be any of them. The real type travels inside the
        // encrypted DM envelope, which is where the recipient reads it.
        let url = upload_blob(media.ciphertext, OPAQUE_UPLOAD_MIME).await?;

        // Zero-knowledge NIP-94 event: URL + ciphertext hash, no key, no `ox`.
        let meta = FileMetadata {
            url: url.clone(),
            mime_type: mime_type.to_string(),
            sha256_hex,
            original_sha256_hex: None,
            size: Some(media.size),
            caption: caption.to_string(),
        };
        let event =
            build_file_metadata_event(&keys, &meta).map_err(|e| UiError::Engine(e.to_string()))?;
        let event_id = event.id.to_hex();
        let created_at = now_secs();

        // Persist a local ref so download_and_decrypt_media(event_id) resolves.
        let reff = MediaRef {
            event_id: event_id.clone(),
            url: url.clone(),
            peer_pubkey: peer.to_hex(),
            mime_type: mime_type.to_string(),
            caption: caption.to_string(),
            size,
            sha256_hex: media.sha256_hex.clone(),
            outgoing: true,
            created_at,
        };
        if let Some(store) = &self.store {
            store
                .put(MEDIA_REFS_TREE, &event_id, &reff)
                .and_then(|()| store.flush())
                .map_err(|e| UiError::Storage(e.to_string()))?;
        }

        // Privately deliver the reference over the E2E DM channel.
        let envelope = MediaEnvelope {
            comrade_media: 1,
            event_id: event_id.clone(),
            url: url.clone(),
            mime: mime_type.to_string(),
            caption: caption.to_string(),
            size,
            sha256_hex: media.sha256_hex.clone(),
        };
        let envelope_json =
            serde_json::to_string(&envelope).map_err(|e| UiError::Engine(e.to_string()))?;

        // A relay that will not take the reference must not orphan the upload.
        // Before this, a publish failure returned an error *after* the blob was
        // uploaded and the local ref persisted: the sender saw a failure, saw
        // the attachment in their own thread anyway, and the recipient never
        // learned the blob existed — with nothing left to retry from. The
        // reference is ordinary DM content, so it queues in the same outbox as
        // text (`docs/BITCHAT_ADOPTION.md` store-and-forward) keyed by its
        // NIP-94 event id, and the flush loop delivers it when a relay returns.
        let peer_npub = to_npub(target_pubkey);
        if let Err(e) = vault.send_dm(&peer, &envelope_json).await {
            tracing::info!(error = %e, "media reference could not be published — queued for retry");
            let queued = QueuedMessage::new(
                event_id.clone(),
                peer_npub.clone(),
                &envelope_json,
                None,
                created_at,
            );
            if let QueueOutcome::Displaced(dropped) = self.outbox.queue(queued) {
                self.mark_status(&peer_npub, &[dropped], STATUS_FAILED);
            }
            if let Some(store) = &self.store {
                persist_outbox(store, &self.outbox);
            }
            // Not a lie about delivery: `queued` is the same status a text DM
            // gets on the same failure, and it is keyed by the media event id so
            // the flush loop's later "sent" names the same handle the UI holds.
            self.mark_status(&peer_npub, std::slice::from_ref(&event_id), STATUS_QUEUED);
        }

        // An attachment reaches them just as a message does, so it settles the
        // same debt — see the same call in [`Self::send_dm_reply`]. Any text
        // still in the composer starts its own clock again the next time they
        // touch it.
        self.nudge_watch.sent(&peer_npub);

        let sender = keys
            .public_key()
            .to_bech32()
            .unwrap_or_else(|_| keys.public_key().to_hex());
        Ok(MediaMessageDto {
            event_id,
            url,
            mime_type: mime_type.to_string(),
            caption: caption.to_string(),
            sender,
            created_at,
            size,
            outgoing: true,
        })
    }

    pub async fn download_and_decrypt_media(
        &self,
        event_id: &str,
    ) -> Result<MediaBytesDto, UiError> {
        let store = self.store.as_deref().ok_or(UiError::VaultLocked)?;
        let reff: MediaRef = store
            .get(MEDIA_REFS_TREE, event_id)
            .map_err(|e| UiError::Storage(e.to_string()))?
            .ok_or_else(|| UiError::Engine(format!("unknown media event {event_id}")))?;

        let keys = self.keys.clone().ok_or(UiError::NoIdentity)?;
        let peer = parse_pubkey(&reff.peer_pubkey)?;
        let key = derive_media_key(keys.secret_key(), &peer, MEDIA_LABEL)
            .map_err(|e| UiError::Engine(e.to_string()))?;

        // Verify the ciphertext hash when we recorded one (fail fast on a
        // wrong/tampered blob; older refs without it fall back to the AES-GCM
        // tag alone, which still rejects tampering).
        let expected = (!reff.sha256_hex.is_empty()).then_some(reff.sha256_hex.as_str());
        let bytes = fetch_and_decrypt_media(&reff.url, &key, expected)
            .await
            .map_err(|e| UiError::Engine(e.to_string()))?;

        Ok(MediaBytesDto {
            mime_type: reff.mime_type,
            base64: B64.encode(&bytes),
        })
    }

    pub async fn search_profiles(&self, query: &str) -> Result<Vec<FoundProfileDto>, UiError> {
        let sabha = self.sabha.clone().ok_or(UiError::VaultLocked)?;
        let query = query.trim().trim_start_matches('@');
        if query.is_empty() {
            return Ok(vec![]);
        }

        // Exact-key lookup: fetch that author's Kind-0 (name may be absent —
        // the key alone is still a valid, addressable result). Otherwise a
        // NIP-50 name search. Both branches share the DTO mapping and cache.
        let dtos: Vec<FoundProfileDto> = if let Ok(pk) = PublicKey::parse(query) {
            let meta = sabha
                .fetch_profile(&pk)
                .await
                .map_err(|e| UiError::Engine(e.to_string()))?;
            vec![found_profile_dto(&pk, meta.as_ref())]
        } else {
            sabha
                .search_profiles(query, 10)
                .await
                .map_err(|e| UiError::Engine(e.to_string()))?
                .into_iter()
                .map(|(pk, meta)| found_profile_dto(&pk, Some(&meta)))
                .collect()
        };
        cache_found_profiles(self.store.as_deref(), &dtos);
        Ok(dtos)
    }

    pub async fn sync_ledger(&self) -> Result<String, UiError> {
        let sakha = self.sakha.clone().ok_or(UiError::VaultLocked)?;
        let id = sakha
            .publish_sync()
            .await
            .map_err(|e| UiError::Engine(e.to_string()))?;
        Ok(id.to_hex())
    }

    pub async fn sakha_add_entry(
        &self,
        description: &str,
        amount_inr: f64,
        paid_by: &str,
    ) -> Result<String, UiError> {
        if description.trim().is_empty() {
            return Err(UiError::Engine("description is empty".into()));
        }
        let sakha = self.sakha.clone().ok_or(UiError::VaultLocked)?;
        if !sakha.is_paired() {
            return Err(UiError::Engine(
                "not paired with a partner yet — open the Partner Portal first".into(),
            ));
        }
        let entry = LedgerEntry::new(description, amount_inr, paid_by);
        sakha
            .add_entry(entry)
            .await
            .map_err(|e| UiError::Engine(e.to_string()))?;
        let ledger = sakha.read_ledger().await;
        if let Some(store) = &self.store {
            persist_ledger_snapshot(store, &sakha).await;
        }
        Ok(ledger)
    }
}

/// Upload an encrypted blob to Blossom, signed with a BUD-01 auth event.
/// Gated on the `media-http` feature; degrades to a typed error otherwise.
///
/// The BUD-01 auth event is signed with a **fresh ephemeral key**, never the
/// user's chat identity: the blob is already zero-knowledge, and signing
/// with the identity key would let the host link "npub X uploaded blob Y at
/// time T from IP Z" — a metadata leak at odds with the privacy model. Free
/// function (not a `ComradeRuntime`/`RuntimeHandles` method) since it needs
/// no engine/store state at all.
#[cfg(feature = "media-http")]
async fn upload_blob(blob: Vec<u8>, mime: &str) -> Result<String, UiError> {
    use comrade_core::media::{BlossomUploader, MediaUploader, DEFAULT_BLOSSOM_SERVERS};
    // Every default host, in order, not one: media sharing was broken outright
    // for as long as the single hard-coded host was refusing connections, and
    // no frontend could route around it.
    let uploader = BlossomUploader::with_servers(
        DEFAULT_BLOSSOM_SERVERS.iter().copied(),
        nostr_sdk::prelude::Keys::generate(),
    );
    let receipt = uploader
        .upload(&blob, mime)
        .await
        .map_err(|e| UiError::Engine(e.to_string()))?;
    Ok(receipt.url)
}

#[cfg(not(feature = "media-http"))]
async fn upload_blob(_blob: Vec<u8>, _mime: &str) -> Result<String, UiError> {
    Err(UiError::Engine(
        "media upload requires the `media-http` feature".into(),
    ))
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

/// Wall clock in milliseconds. The together channel needs this resolution: a
/// DM's own `created_at` is whole seconds, which is a full second of noise in a
/// system trying to hold a fraction of one.
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default()
}

/// Normalise a pairing-role string to exactly `"sakha"` or `"sakhi"` —
/// anything else (including case variants) falls back to `"sakha"`, mirroring
/// the lenient `from_str_lenient` pattern already used for `CallMediaKind`/
/// `HangupReason` elsewhere in this bridge.
fn normalize_pair_role(role: &str) -> String {
    if role.eq_ignore_ascii_case("sakhi") {
        "sakhi".to_string()
    } else {
        "sakha".to_string()
    }
}

/// Snapshot the Sakha CRDT doc and persist it, so the ledger survives a
/// restart without needing a fresh sync from the partner. Best-effort: a
/// write failure is logged, not propagated — losing a snapshot write is far
/// less bad than failing the ledger update that triggered it.
async fn persist_ledger_snapshot(store: &comrade_storage::EncryptedStore, sakha: &SakhaEngine) {
    let bytes = sakha.snapshot_bytes().await;
    let state = comrade_storage::LedgerState {
        snapshot: bytes,
        updated_at: now_secs(),
    };
    if let Err(e) = store.save_ledger_state(&state).and_then(|()| store.flush()) {
        warn!("failed to persist Sakha ledger snapshot: {e}");
    }
}

/// A stored usage day as the attention engine reads it (rollup numbers only).
fn usage_signal(day: &comrade_storage::AttentionDay) -> UsageSignal {
    UsageSignal {
        screen_minutes: day.screen_minutes,
        doom_minutes: day.doom_minutes,
        pickups: day.pickups,
    }
}

/// A stored focus session as a frontend sees it, with the live countdown
/// filled in (0 once it has ended).
fn focus_dto(session: comrade_storage::FocusSession, now: u64) -> FocusSessionDto {
    let remaining = if session.outcome.is_some() {
        0
    } else {
        attention::remaining_secs(session.planned_minutes, session.started_at, now)
    };
    FocusSessionDto {
        id: session.id,
        intent: session.intent,
        planned_minutes: session.planned_minutes,
        started_at: session.started_at,
        ended_at: session.ended_at,
        outcome: session.outcome,
        remaining_secs: remaining,
    }
}

/// A saved read opened for reading: chunked, with its position clamped into
/// range — a stored position must never point past the end of the text.
fn saved_read_dto(read: comrade_storage::SavedRead) -> SavedReadDto {
    let chunks = attention::chunk_reading(&read.text);
    let last = chunks.len().saturating_sub(1) as u32;
    SavedReadDto {
        id: read.id,
        title: read.title,
        source: read.source,
        chunks,
        position: read.position.min(last),
        added_at: read.added_at,
    }
}

/// A library row. The chunk count is recomputed (chunking is cheap and the
/// text is the truth), and the position is clamped the same way
/// [`saved_read_dto`] clamps it so the two views can never disagree on
/// progress.
fn saved_read_summary_dto(read: comrade_storage::SavedRead) -> SavedReadSummaryDto {
    let chunks = attention::chunk_reading(&read.text).len() as u32;
    SavedReadSummaryDto {
        id: read.id,
        title: read.title,
        source: read.source,
        chunk_count: chunks,
        position: read.position.min(chunks.saturating_sub(1)),
        added_at: read.added_at,
    }
}

/// Whether `s` looks like `YYYY-MM-DD`. Deliberately a shape check, not a
/// calendar check: the frontend owns the timezone and the calendar, and this
/// crate only needs keys that sort chronologically.
fn is_iso_date(s: &str) -> bool {
    let bytes = s.as_bytes();
    bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(i, b)| matches!(i, 4 | 7) || b.is_ascii_digit())
}

/// `YYYY-MM-DD` for a unix timestamp in **UTC**.
///
/// Used only to answer "is the newest recorded row still today's?" for the
/// Tara nudge. Every stored date comes from the frontend, which knows the
/// device's real timezone; the worst this approximation can do is delay (or
/// briefly advance) one journaling nudge by hours near midnight, which is why
/// a timezone database is not pulled in for it.
fn iso_date(unix_secs: u64) -> String {
    // Days since the Unix epoch → civil date (Howard Hinnant's algorithm).
    let days = (unix_secs / 86_400) as i64;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

/// Store key for a local-only record (journal entry, Tara turn): a zero-padded
/// timestamp prefix (so ids sort chronologically) plus a random tail (so two
/// records in the same second never collide). The randomness comes from a
/// throwaway secp256k1 key — no extra dependency, and cryptographically
/// unpredictable.
fn timestamped_store_id(created_at: u64) -> String {
    let tail = nostr_sdk::prelude::Keys::generate().public_key().to_hex();
    format!("{created_at:020}-{}", &tail[..12])
}

/// A trimmed optional field, with whitespace-only treated as absent.
///
/// A title of `"   "` is a user who tapped save on an empty box, not a user who
/// named something; storing it would put a blank line where the frontend draws
/// a heading and make "has a title" answer yes.
fn clean_optional(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(String::from)
}

/// This identity's own bio, from the settings tree.
///
/// A free function taking the store, matching the other cache readers here, so
/// [`ComradeRuntime::profile`] can call it without caring whether the vault
/// happens to be unlocked.
fn stored_about(store: &comrade_storage::EncryptedStore) -> Option<String> {
    store
        .get::<String>(SETTINGS_TREE, PROFILE_ABOUT_KEY)
        .ok()
        .flatten()
        .filter(|s| !s.is_empty())
}

/// The peer's whole cached profile, not just the handle.
///
/// The accessor that was missing: `about` has been persisted by
/// [`cache_found_profiles`] and [`ProfileRefresher::run`] since bios were first
/// cached, and nothing could read it back — every reader went through
/// [`cached_peer_name`], which takes `.name` and drops the rest. A profile page
/// for an existing contact had no way to reach their bio at all.
fn cached_peer_profile(
    store: &comrade_storage::EncryptedStore,
    npub: &str,
) -> Option<PeerProfileRecord> {
    store
        .get::<PeerProfileRecord>(PEER_PROFILES_TREE, npub)
        .ok()
        .flatten()
}

/// The peer's published @handle from the local profile cache, if known.
fn cached_peer_name(store: &comrade_storage::EncryptedStore, npub: &str) -> Option<String> {
    cached_peer_profile(store, npub).and_then(|r| r.name)
}

/// Fold what we just learned into the cached record, leaving alone anything the
/// caller had no opinion about.
///
/// **The only writer of [`PEER_PROFILES_TREE`].** `store_profile_record` is the
/// raw put and must not be called from anywhere else. The reason is a bug this
/// replaces: `cache_pushed_peer_name` built a whole record with `about: None`, so
/// a peer's profile-share envelope — which arrives when a request is accepted —
/// erased any bio already cached for them. Nothing read `about`, so nothing
/// noticed; the moment a profile page renders one, it becomes "their bio vanished
/// when I accepted them". Every new field would have inherited the same shape.
fn merge_peer_profile(
    store: &comrade_storage::EncryptedStore,
    npub: &str,
    patch: PeerProfilePatch,
) -> bool {
    let mut record = cached_peer_profile(store, npub).unwrap_or_default();
    let now = now_secs();
    // A `None` in the patch means "learned nothing about this", never "it is
    // empty" — the whole point of the merge.
    if patch.name.is_some() {
        record.name = patch.name;
    }
    if patch.about.is_some() {
        record.about = patch.about;
    }
    if patch.picture.is_some() {
        // A changed URL invalidates the cached bytes: they are the old picture.
        if record.picture != patch.picture {
            record.avatar_sha256 = None;
            record.avatar_mime = None;
            record.avatar_fetched_at = 0;
            record.avatar_failed_at = 0;
        }
        record.picture = patch.picture;
    }
    if patch.nip05.is_some() {
        record.nip05 = patch.nip05;
    }
    if patch.lud16.is_some() {
        record.lud16 = patch.lud16;
    }
    if let Some((sha, mime)) = patch.avatar {
        record.avatar_sha256 = Some(sha);
        record.avatar_mime = Some(mime);
        record.avatar_fetched_at = now;
        record.avatar_failed_at = 0;
    }
    if patch.avatar_failed {
        // Stamp the failure and nothing else. A refresh that could not reach the
        // host must not throw away a picture we already have — the same reasoning
        // the refresher already applies to a name a silent relay set did not
        // return.
        record.avatar_failed_at = now;
    }
    record.updated_at = now;
    store_profile_record(store, npub, &record)
}

/// Legacy builds auto-filled an empty alias with the first 12 characters of
/// the peer's npub. Normalise those placeholders (and blanks) to "no alias"
/// so the peer's published handle can title the chat — otherwise every
/// pre-existing key-added contact is stuck displaying `npub1abcdefg` forever.
const LEGACY_PLACEHOLDER_LEN: usize = 12;

fn user_alias(petname: &str, npub: &str) -> Option<String> {
    let trimmed = petname.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.len() == LEGACY_PLACEHOLDER_LEN && npub.starts_with(trimmed) {
        return None;
    }
    Some(trimmed.to_string())
}

/// Map a fetched Kind-0 (or its absence) to the search-result DTO. One
/// mapping for both the direct-key branch and the name-search branch, so the
/// two can never render the same profile differently.
fn found_profile_dto(pk: &PublicKey, meta: Option<&Metadata>) -> FoundProfileDto {
    FoundProfileDto {
        npub: pk.to_bech32().unwrap_or_else(|_| pk.to_hex()),
        name: meta.and_then(display_name_of),
        about: meta.and_then(|m| m.about.clone()),
        // Carried so a search row can say "has a picture" and still draw initials.
        // Nothing is ever fetched for a stranger — see `may_fetch_avatar`.
        picture: meta.and_then(|m| m.picture.clone()),
        nip05: meta.and_then(|m| m.nip05.clone()),
    }
}

/// Best-effort single-record write into the profile cache; returns whether
/// the write succeeded. Callers flush once per batch.
fn store_profile_record(
    store: &comrade_storage::EncryptedStore,
    npub: &str,
    record: &PeerProfileRecord,
) -> bool {
    match store.put(PEER_PROFILES_TREE, npub, record) {
        Ok(()) => true,
        Err(e) => {
            warn!("failed to cache peer profile: {e}");
            false
        }
    }
}

/// Persist discovered profiles into the local cache (best-effort) so the
/// chat list can title peers by their handle without another fetch. Free
/// function so [`RuntimeHandles::search_profiles`] can call it with no
/// runtime lock held.
fn cache_found_profiles(
    store: Option<&comrade_storage::EncryptedStore>,
    found: &[FoundProfileDto],
) {
    let Some(store) = store else {
        return;
    };
    let mut wrote = false;
    for profile in found {
        if profile.name.is_none() {
            continue; // nothing displayable; don't shadow a future fetch
        }
        wrote |= merge_peer_profile(
            store,
            &profile.npub,
            PeerProfilePatch {
                name: profile.name.clone(),
                about: profile.about.clone(),
                picture: profile.picture.clone(),
                nip05: profile.nip05.clone(),
                ..Default::default()
            },
        );
    }
    if wrote {
        if let Err(e) = store.flush() {
            warn!("failed to flush profile cache: {e}");
        }
    }
}

/// A user-configured TURN relay for WebRTC calls, sealed in the encrypted store.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct TurnConfig {
    url: String,
    #[serde(default)]
    username: String,
    #[serde(default)]
    credential: String,
}

/// The conversation gate for an incoming DM's sender.
enum IncomingGate {
    /// Peer is blocked — drop everything from them silently.
    Blocked,
    /// Peer is an established conversation — deliver normally + ack.
    Accepted,
    /// Peer is a stranger (or an unaccepted request) — route to requests.
    Pending,
}

/// Classify an incoming DM's sender against the conversation gate. A peer with
/// no gate record is treated as `Pending` (a new stranger); [`send_dm`] and
/// [`accept_request`] are what flip a peer to `Accepted`.
///
/// [`send_dm`]: ComradeRuntime::send_dm
/// [`accept_request`]: ComradeRuntime::accept_request
fn conversation_gate(store: &comrade_storage::EncryptedStore, peer_npub: &str) -> IncomingGate {
    match store.get_conversation_meta(peer_npub).ok().flatten() {
        Some(m) if m.state == STATE_BLOCKED => IncomingGate::Blocked,
        Some(m) if m.state == STATE_ACCEPTED => IncomingGate::Accepted,
        _ => IncomingGate::Pending,
    }
}

/// Record a peer as a pending request if they have no gate record yet.
fn ensure_pending(store: Option<&Arc<comrade_storage::EncryptedStore>>, peer_npub: &str) {
    let Some(store) = store else { return };
    if store
        .get_conversation_meta(peer_npub)
        .ok()
        .flatten()
        .is_none()
    {
        let meta = comrade_storage::ConversationMeta {
            peer_npub: peer_npub.to_string(),
            state: STATE_PENDING.to_string(),
            profile_shared: false,
            // Only reached when no record exists, so there is nothing read yet.
            last_read_at: 0,
            updated_at: now_secs(),
        };
        if let Err(e) = store
            .set_conversation_meta(&meta)
            .and_then(|()| store.flush())
        {
            warn!("failed to record message request: {e}");
        }
    }
}

/// Cache a peer's shared display handle (from a profile-share envelope).
fn cache_pushed_peer_name(store: &comrade_storage::EncryptedStore, npub: &str, name: &str) {
    // A profile share carries a handle and nothing else, so it must claim nothing
    // else. This used to write a whole record with `about: None` and erase a bio
    // we already had — see `merge_peer_profile`.
    let learned = PeerProfilePatch {
        name: Some(name.to_string()),
        ..Default::default()
    };
    if merge_peer_profile(store, npub, learned) {
        let _ = store.flush();
    }
}

/// Record `peer_npub`'s conversation as accepted and, once, share our
/// @handle with them over the encrypted channel. Free function (rather than
/// a `ComradeRuntime` method) so it can run from [`RuntimeHandles::send_dm_reply`]
/// with no runtime lock held, as well as from [`ComradeRuntime::accept_request`].
///
/// The share itself runs in the background so the caller is never blocked on
/// the network; the `profile_shared` flag flips only on a successful send,
/// so a failed share retries next time.
fn share_profile_on_accept(
    store: Option<Arc<comrade_storage::EncryptedStore>>,
    vault: Option<Arc<VaultEngine>>,
    username: Option<String>,
    peer_npub: &str,
    peer: &PublicKey,
) {
    let Some(store) = store else {
        return;
    };
    let existing = store.get_conversation_meta(peer_npub).ok().flatten();
    let already_shared = existing.as_ref().map(|m| m.profile_shared).unwrap_or(false);
    let meta = comrade_storage::ConversationMeta {
        peer_npub: peer_npub.to_string(),
        state: STATE_ACCEPTED.to_string(),
        profile_shared: already_shared,
        // Accepting rewrites the record, so the position has to be carried or
        // the freshly accepted thread would forget what had been read.
        last_read_at: existing.as_ref().map(|m| m.last_read_at).unwrap_or(0),
        updated_at: now_secs(),
    };
    if let Err(e) = store
        .set_conversation_meta(&meta)
        .and_then(|()| store.flush())
    {
        warn!("failed to record accepted conversation: {e}");
    }
    if already_shared {
        return;
    }
    let Some(vault) = vault else {
        return;
    };
    let peer = *peer;
    let peer_npub = peer_npub.to_string();
    tokio::spawn(async move {
        let Ok(json) = ProfileShare::new(username).to_json() else {
            return;
        };
        if vault.send_dm(&peer, &json).await.is_ok() {
            // Re-read rather than reuse the value from before the await: the
            // send is a relay round-trip, and the thread may well have been
            // opened and read in the meantime.
            let last_read_at = store.read_position(&peer_npub).unwrap_or(0);
            let meta = comrade_storage::ConversationMeta {
                peer_npub,
                state: STATE_ACCEPTED.to_string(),
                profile_shared: true,
                last_read_at,
                updated_at: now_secs(),
            };
            let _ = store
                .set_conversation_meta(&meta)
                .and_then(|()| store.flush());
        }
    });
}

// ── Comrade presence (see `comrade_core::presence` for the wire protocol) ────

/// The public keys of every contact marked as a comrade. Unparseable keys are
/// skipped rather than failing the whole fan-out — one corrupt row must not
/// silence presence for everyone else.
fn comrade_peers(store: &comrade_storage::EncryptedStore) -> Vec<PublicKey> {
    store
        .list_comrades()
        .unwrap_or_default()
        .iter()
        .filter_map(|c| PublicKey::parse(&c.npub).ok())
        .collect()
}

/// Fire-and-forget one `beacon` to each of `peers` over the encrypted DM
/// channel. Holds nothing but the vault engine, so it is safe to call
/// immediately before tearing the runtime down (see
/// [`ComradeRuntime::spawn_farewell_beacons`]).
fn spawn_presence_beacons(
    vault: Option<Arc<VaultEngine>>,
    peers: Vec<PublicKey>,
    beacon: PresenceBeacon,
) {
    let Some(vault) = vault else { return };
    if peers.is_empty() {
        return;
    }
    let Ok(json) = beacon.to_json() else { return };
    tokio::spawn(async move {
        for peer in peers {
            if let Err(e) = vault.send_dm(&peer, &json).await {
                tracing::debug!(%peer, "presence beacon not sent: {e}");
            }
        }
    });
}

/// Fan one nudge out to peers the caller has already established are comrades,
/// returning how many a relay accepted.
///
/// The one place a nudge is actually put on the wire, so both triggers — an
/// abandoned draft and a deliberate "I might need you" — send byte-identical
/// envelopes. Two send sites would be two chances for one of them to start
/// carrying a reason.
async fn deliver_nudges(vault: &Arc<VaultEngine>, recipients: Vec<PublicKey>) -> u64 {
    if recipients.is_empty() {
        return 0;
    }
    let Ok(json) = Nudge::new().to_json() else {
        return 0;
    };
    let mut sent = 0u64;
    for peer in recipients {
        match vault.send_dm(&peer, &json).await {
            Ok(_) => sent += 1,
            Err(e) => tracing::debug!(%peer, "nudge not sent: {e}"),
        }
    }
    sent
}

/// Apply an incoming task signal to the local store, returning the chat line to
/// show for it — or `None` when nothing should be shown.
///
/// `None` covers every case where the signal changed nothing: a redelivered
/// assignment (relays deliver at-least-once and the inbox backfills two days on
/// every launch, so this *will* happen), a state change for a task we have never
/// heard of, and a state change the sender has no standing to make. That last
/// one is the forgery check, and it is [`Task::apply`]'s table doing the work:
/// `peer_npub` is the authenticated sender of a gift-wrapped DM, so a peer who
/// is not party to the task, or is the wrong party for that transition, moves
/// nothing.
fn apply_karya_signal(
    store: Option<&Arc<comrade_storage::EncryptedStore>>,
    peer_npub: &str,
    me: &str,
    created_at: u64,
    signal: &TaskSignal,
) -> Option<String> {
    let store = store?;
    match signal {
        TaskSignal::Assign { id, text } => {
            // Already have it: a redelivery, not a new ask.
            if store.task(id).ok().flatten().is_some() {
                return None;
            }
            // The sender is the assigner and we are the assignee — the only
            // shape an incoming assignment can have. A peer cannot assign a
            // task to somebody else through us.
            let task = Task::new(
                id.clone(),
                text,
                peer_npub.to_string(),
                Some(me.to_string()),
                created_at,
            );
            if task.text.is_empty() {
                return None;
            }
            let line = render_task_line(TaskState::Open, &task.text);
            if let Err(e) = store
                .save_task(&stored_task(&task))
                .and_then(|()| store.flush())
            {
                warn!("failed to persist incoming task: {e}");
                return None;
            }
            Some(line)
        }
        TaskSignal::State { id, state } => {
            let row = store.task(id).ok().flatten()?;
            let mut task = task_from_stored(&row)?;
            if !task.apply(*state, peer_npub, created_at) {
                tracing::debug!(
                    peer = %peer_npub,
                    task = %id,
                    "task state change refused — not theirs to make"
                );
                return None;
            }
            let line = render_task_line(*state, &task.text);
            if let Err(e) = store
                .save_task(&stored_task(&task))
                .and_then(|()| store.flush())
            {
                warn!("failed to persist task state: {e}");
                return None;
            }
            Some(line)
        }
    }
}

/// Apply an incoming topic signal to the local store, returning whether the
/// *visible* structure moved — which is what decides whether an event is sent.
///
/// `false` covers every case where the signal changed nothing: a redelivered
/// creation (relays deliver at-least-once and the inbox backfills two days on
/// every launch, so this *will* happen), a filing older than the one we hold,
/// and a close for a topic we have never heard of.
///
/// **There is no standing check here, and that is deliberate.** Unlike
/// [`apply_karya_signal`], which refuses a state change from a peer who is not
/// party to the task, a topic is shared filing rather than a request made of
/// one person: both people can name topics and file threads, so the only
/// authorisation that matters is the one the caller already applied — that the
/// sender is in an accepted conversation. What a peer *cannot* do is reach
/// another conversation, because `peer_npub` is the authenticated sender of a
/// gift-wrapped DM and every row written here is keyed by it.
fn apply_topic_signal(
    store: Option<&Arc<comrade_storage::EncryptedStore>>,
    peer_npub: &str,
    created_at: u64,
    signal: &TopicSignal,
) -> bool {
    let Some(store) = store else {
        return false;
    };
    match signal {
        TopicSignal::Create { slug, name } => {
            let Some(fresh) =
                comrade_core::topic::Topic::new(name, peer_npub, peer_npub, created_at)
            else {
                return false;
            };
            // The slug travels on the wire *and* is re-derived from the name
            // here, and the two must agree. A mismatch means the sender's rules
            // are not ours — a newer build, or a forged envelope — and taking
            // their slug would file threads under a key this device can never
            // produce from any name a user types.
            if &fresh.slug != slug {
                tracing::debug!(peer = %peer_npub, "topic slug does not match its name");
                return false;
            }
            match store.get_topic(peer_npub, slug) {
                Ok(Some(row)) => {
                    let mut topic = topic_from_stored(&row);
                    if !topic.merge_name(name, created_at) {
                        return false;
                    }
                    store
                        .save_topic(&stored_topic(&topic))
                        .and_then(|()| store.flush())
                        .map_err(|e| warn!("failed to persist a topic rename: {e}"))
                        .is_ok()
                }
                Ok(None) => store
                    .save_topic(&stored_topic(&fresh))
                    .and_then(|()| store.flush())
                    .map_err(|e| warn!("failed to persist an incoming topic: {e}"))
                    .is_ok(),
                Err(e) => {
                    warn!("failed to read a topic: {e}");
                    false
                }
            }
        }
        TopicSignal::Assign { root_id, slug } => {
            // A filing whose topic we do not hold is stored anyway. The two
            // envelopes can be reordered by the relay, and refusing here would
            // silently drop the filing that the `Create` behind it was for.
            // `ThreadIndex` treats an unknown slug as unfiled until the name
            // arrives, which is the recoverable version of the same state.
            let filing = comrade_storage::ThreadTopic {
                root_id: root_id.clone(),
                peer_npub: peer_npub.to_string(),
                slug: slug.clone(),
                updated_at: created_at,
            };
            match store
                .set_thread_topic(&filing)
                .and_then(|c| store.flush().map(|()| c))
            {
                Ok(changed) => changed,
                Err(e) => {
                    warn!("failed to persist an incoming thread filing: {e}");
                    false
                }
            }
        }
        TopicSignal::Close { slug, closed } => {
            let Ok(Some(row)) = store.get_topic(peer_npub, slug) else {
                return false;
            };
            let mut topic = topic_from_stored(&row);
            if !topic.set_closed(*closed, created_at) {
                return false;
            }
            store
                .save_topic(&stored_topic(&topic))
                .and_then(|()| store.flush())
                .map_err(|e| warn!("failed to persist a topic archive: {e}"))
                .is_ok()
        }
    }
}

/// Persist and surface a chat line this device generated from a control
/// envelope, so a task or an offer reads as the message it is.
///
/// Reuses the incoming event's real id, so a wrapper redelivered by the *same*
/// transport is caught by the `get_message` check below. That is **not** enough
/// on its own: the same envelope arriving over the other transport carries a
/// different event id, and pairing those is the caller's job — both arms of the
/// dispatcher run `is_cross_transport_duplicate` on the envelope bytes before
/// reaching here. Sends a delivered receipt for the same reason the plain-chat
/// path does: the message *did* arrive.
#[allow(clippy::too_many_arguments)]
fn deliver_synthetic_line(
    vault: &Arc<VaultEngine>,
    store: Option<&Arc<comrade_storage::EncryptedStore>>,
    tx: &broadcast::Sender<BridgeEvent>,
    route: &DmRoute<'_>,
    msg: &VaultMessage,
    peer_npub: &str,
    line: String,
) {
    if let Some(store) = store {
        if store.get_message(&msg.event_id).ok().flatten().is_some() {
            return;
        }
        let row = comrade_storage::StoredMessage {
            id: msg.event_id.clone(),
            peer_npub: peer_npub.to_string(),
            content: line.clone(),
            created_at: msg.created_at,
            outgoing: false,
            status: None,
            reply_to: None,
        };
        if let Err(e) = store.save_message(&row).and_then(|()| store.flush()) {
            warn!("failed to persist a rendered control line: {e}");
        }
    }
    send_delivered_receipt(vault, route.mesh, &msg.sender_pubkey, &msg.event_id);
    let _ = tx.send(BridgeEvent::IncomingDirectMessage(DirectMessageDto {
        id: msg.event_id.clone(),
        sender: peer_npub.to_string(),
        // A rendered control line is Comrade's own words about a transfer, not
        // anybody's journal.
        shared_note: None,
        content: line,
        created_at: msg.created_at,
        upi_intents: Vec::new(),
        reply_to: None,
    }));
}

// ── Thread and topic reads (see `comrade_core::topic`) ───────────────────────

/// What to say when a name cannot be a slug.
///
/// One sentence, and it names the rule rather than the failure, because the
/// commonest way to hit it is by typing a topic in a script the slug rules do
/// not yet accept (`AUDIT.md` TOPIC-1) — and "that's not a valid slug" tells
/// somebody nothing they can act on.
const TOPIC_NAME_REFUSED: &str =
    "A topic name needs two or more letters or digits, and for now Latin ones";

/// One conversation's history reduced to the shape every thread and topic
/// question is asked against, computed once per call.
///
/// Built rather than stored, and the counts are derived from it rather than
/// kept beside the rows, because a stored count is a second source of truth
/// that drifts the first time a backfill inserts an old message into the
/// middle of a thread — and this history is bounded by one conversation, so
/// there is nothing here that scales badly enough to justify the drift.
struct ThreadIndex {
    /// Root id → the ids in that thread, oldest first.
    threads: std::collections::HashMap<String, Vec<String>>,
    /// Every item, by event id. Roots that are not in this map are threads
    /// filed before their root arrived; see [`ThreadSummaryDto::root_missing`].
    items: std::collections::HashMap<String, ThreadItem>,
    /// Root id → the topic it is filed under. Unfiled roots are absent, which
    /// is what an unfiling tombstone reads as.
    filed: std::collections::HashMap<String, String>,
    /// `ConversationMeta::last_read_at` — the same watermark the main thread's
    /// unread divider uses, so a sheet cannot disagree with the screen that
    /// opened it.
    last_read_at: u64,
}

/// One item in a thread, reduced to what a summary row needs.
struct ThreadItem {
    created_at: u64,
    outgoing: bool,
    /// The text to preview. Empty for an uncaptioned attachment — the frontend
    /// supplies its own word, see [`ThreadSummaryDto::root_is_media`].
    preview: String,
    is_media: bool,
}

impl ThreadIndex {
    /// Read one conversation and group it into threads.
    ///
    /// Attachments are included as items but contribute no `reply_to` edges,
    /// because a `MediaRef` carries none: an attachment can *start* a thread
    /// and cannot join one. `AUDIT.md` TOPIC-2.
    fn build(
        store: &comrade_storage::EncryptedStore,
        peer_npub: &str,
        peer_hex: &str,
    ) -> Result<Self, UiError> {
        let rows = store
            .messages_with(peer_npub)
            .map_err(|e| UiError::Storage(e.to_string()))?;
        let media: Vec<MediaRef> = store
            .values::<MediaRef>(MEDIA_REFS_TREE)
            .map_err(|e| UiError::Storage(e.to_string()))?
            .into_iter()
            .filter(|r| r.peer_pubkey == peer_hex)
            .collect();

        let mut items = std::collections::HashMap::new();
        let mut parents = std::collections::HashMap::new();
        for row in &rows {
            if let Some(parent) = row.reply_to.clone().filter(|p| !p.is_empty()) {
                parents.insert(row.id.clone(), parent);
            }
            let (_, preview) = split_author(row.content.clone());
            items.insert(
                row.id.clone(),
                ThreadItem {
                    created_at: row.created_at,
                    outgoing: row.outgoing,
                    preview,
                    is_media: false,
                },
            );
        }
        for r in &media {
            items.insert(
                r.event_id.clone(),
                ThreadItem {
                    created_at: r.created_at,
                    outgoing: r.outgoing,
                    preview: r.caption.clone(),
                    is_media: true,
                },
            );
        }

        // Time-ordered so every thread's member list is time-ordered too, which
        // is what lets a summary read its first and last entry rather than
        // scanning for a minimum and a maximum.
        let mut ids: Vec<String> = items.keys().cloned().collect();
        ids.sort_by(|a, b| {
            let (ta, tb) = (items[a].created_at, items[b].created_at);
            ta.cmp(&tb).then_with(|| a.cmp(b))
        });

        let filed = store
            .thread_topics_with(peer_npub)
            .map_err(|e| UiError::Storage(e.to_string()))?
            .into_iter()
            .filter_map(|f| f.slug.map(|s| (f.root_id, s)))
            .collect();

        let last_read_at = store
            .get_conversation_meta(peer_npub)
            .map_err(|e| UiError::Storage(e.to_string()))?
            .map(|m| m.last_read_at)
            .unwrap_or_default();

        Ok(Self {
            threads: comrade_core::topic::group_threads(&ids, &parents),
            items,
            filed,
            last_read_at,
        })
    }

    /// The summary row for one thread, or `None` if the root names no thread we
    /// hold — which happens when a filing arrived for a root whose whole thread
    /// is still outside the loaded window, and is a row with nothing to say.
    fn summary(&self, peer_npub: &str, root_id: &str) -> Option<ThreadSummaryDto> {
        let members = self.threads.get(root_id)?;
        let root = self.items.get(root_id);
        let first = members.first().and_then(|id| self.items.get(id));
        let last = members.last().and_then(|id| self.items.get(id))?;
        Some(ThreadSummaryDto {
            root_id: root_id.to_string(),
            peer: peer_npub.to_string(),
            topic_slug: self.filed.get(root_id).cloned(),
            preview: root.map(|r| r.preview.clone()).unwrap_or_default(),
            root_is_media: root.is_some_and(|r| r.is_media),
            root_missing: root.is_none(),
            started_at: root.or(first).map(|r| r.created_at).unwrap_or_default(),
            // The root is a member of its own thread, so "replies" is one less
            // — and saturating, because a thread whose root is missing has a
            // member list that does not contain it.
            reply_count: (members.len() as u32).saturating_sub(if root.is_some() { 1 } else { 0 }),
            last_at: last.created_at,
            unread: members.iter().any(|id| {
                self.items
                    .get(id)
                    .is_some_and(|i| !i.outgoing && i.created_at > self.last_read_at)
            }),
        })
    }

    /// Every thread in the conversation, newest activity first.
    fn summaries(&self, peer_npub: &str) -> Vec<ThreadSummaryDto> {
        let mut out: Vec<ThreadSummaryDto> = self
            .threads
            .keys()
            .filter_map(|root| self.summary(peer_npub, root))
            .collect();
        out.sort_by(|a, b| {
            b.last_at
                .cmp(&a.last_at)
                .then_with(|| a.root_id.cmp(&b.root_id))
        });
        out
    }
}

/// The thread `message_id` belongs to, resolved against `peer`'s history.
///
/// A message we have never seen is its own root rather than an error: filing
/// something that arrived on another device before this one backfilled it is a
/// reasonable thing to ask for, and refusing would make the sheet's one
/// destructive-looking action fail for a reason the user cannot act on.
fn resolve_thread_root(
    store: &comrade_storage::EncryptedStore,
    peer_npub: &str,
    message_id: &str,
) -> Result<String, UiError> {
    let parents: std::collections::HashMap<String, String> = store
        .messages_with(peer_npub)
        .map_err(|e| UiError::Storage(e.to_string()))?
        .into_iter()
        .filter_map(|m| {
            m.reply_to
                .filter(|p| !p.is_empty())
                .map(|parent| (m.id, parent))
        })
        .collect();
    Ok(comrade_core::topic::root_of(message_id, &parents))
}

/// A stored topic as a list row, with counts read off `index`.
fn topic_dto(row: comrade_storage::StoredTopic, me: &str, index: &ThreadIndex) -> TopicDto {
    // Only threads this device actually holds are counted. A filing whose
    // whole thread is still outside the loaded window contributes nothing
    // rather than a phantom row, which keeps the count and the list the sheet
    // renders the same number.
    let members: Vec<&Vec<String>> = index
        .filed
        .iter()
        .filter(|(_, slug)| **slug == row.slug)
        .filter_map(|(root, _)| index.threads.get(root))
        .collect();
    let message_count: usize = members.iter().map(|m| m.len()).sum();
    let last_activity_at = members
        .iter()
        .filter_map(|m| m.last())
        .filter_map(|id| index.items.get(id))
        .map(|i| i.created_at)
        .max()
        .unwrap_or(row.created_at);
    TopicDto {
        mine: row.created_by == me,
        thread_count: members.len() as u32,
        message_count: message_count as u32,
        last_activity_at,
        slug: row.slug,
        name: row.name,
        peer: row.peer_npub,
        created_by: row.created_by,
        created_at: row.created_at,
        closed: row.closed,
    }
}

/// A `topic::Topic` as the store holds it, and back — the same two-shape split
/// [`stored_task`] / [`task_from_stored`] make, and for the same reason:
/// `comrade_storage` cannot depend on `comrade_core`.
fn stored_topic(topic: &comrade_core::topic::Topic) -> comrade_storage::StoredTopic {
    comrade_storage::StoredTopic {
        peer_npub: topic.peer_npub.clone(),
        slug: topic.slug.clone(),
        name: topic.name.clone(),
        created_by: topic.created_by.clone(),
        created_at: topic.created_at,
        closed: topic.closed,
        updated_at: topic.updated_at,
    }
}

/// A stored row back as a `topic::Topic`.
fn topic_from_stored(row: &comrade_storage::StoredTopic) -> comrade_core::topic::Topic {
    comrade_core::topic::Topic {
        slug: row.slug.clone(),
        name: row.name.clone(),
        peer_npub: row.peer_npub.clone(),
        created_by: row.created_by.clone(),
        created_at: row.created_at,
        closed: row.closed,
        updated_at: row.updated_at,
    }
}

// ── Task conversions ─────────────────────────────────────────────────────────
//
// Three shapes for one thing, and each earns its place: `karya::Task` holds the
// state machine, `StoredTask` is what `comrade_storage` can persist without
// depending on `comrade_core`, and `TaskDto` is what a list renders. The two
// conversions live here, once, rather than at each call site.

/// A `karya::Task` as the store holds it.
fn stored_task(task: &Task) -> comrade_storage::StoredTask {
    comrade_storage::StoredTask {
        id: task.id.clone(),
        text: task.text.clone(),
        assigner_npub: task.assigner_npub.clone(),
        assignee_npub: task.assignee_npub.clone(),
        created_at: task.created_at,
        state: task.state.as_str().to_string(),
        updated_at: task.updated_at,
    }
}

/// A stored row back as a `karya::Task`, or `None` if its state is one this
/// build does not know — a row from a newer version is skipped rather than
/// coerced into `Open`, which would resurrect somebody's finished task.
fn task_from_stored(row: &comrade_storage::StoredTask) -> Option<Task> {
    Some(Task {
        id: row.id.clone(),
        text: row.text.clone(),
        assigner_npub: row.assigner_npub.clone(),
        assignee_npub: row.assignee_npub.clone(),
        created_at: row.created_at,
        state: TaskState::from_str_opt(&row.state)?,
        updated_at: row.updated_at,
    })
}

/// A stored row as a list wants it, from the point of view of `me`.
fn task_dto(row: comrade_storage::StoredTask, me: &str) -> Option<TaskDto> {
    let task = task_from_stored(&row)?;
    let assigned_by_me = task.assigner_npub == me;
    Some(TaskDto {
        // `Party::Assignee` is what carries the power to finish or decline, and
        // on a note to self it is the same person as the assigner — so this is
        // the one honest source for "may I tick this off".
        mine_to_do: matches!(task.party(me), Some(Party::Assignee)),
        assigned_by_me,
        id: task.id,
        text: task.text,
        assigner: task.assigner_npub,
        assignee: task.assignee_npub,
        created_at: task.created_at,
        updated_at: task.updated_at,
        state: task.state,
    })
}

/// Whether `peer`'s last beacon still claims them online at `now`. The one
/// read path for stored presence, so an expired claim can never render as a
/// green dot just because no sweep has run yet.
fn peer_is_online(store: &comrade_storage::EncryptedStore, peer_npub: &str, now: u64) -> bool {
    store
        .get_peer_presence(peer_npub)
        .ok()
        .flatten()
        .is_some_and(|p| p.online && is_online_at(p.expires_at, now))
}

/// Age out comrades whose "online" claim has lapsed, emitting one
/// [`BridgeEvent::ComradePresence`] per transition so a UI dot goes grey
/// without the peer having to send anything. This is the half of presence
/// that handles the common, silent case: a phone that runs out of battery,
/// loses signal, or is force-killed never sends a goodbye.
fn expire_stale_presence(
    store: Option<&comrade_storage::EncryptedStore>,
    tx: &broadcast::Sender<BridgeEvent>,
) {
    let Some(store) = store else { return };
    let now = now_secs();
    let mut wrote = false;
    for mut row in store.list_peer_presence().unwrap_or_default() {
        if !row.online || is_online_at(row.expires_at, now) {
            continue;
        }
        let expired_at = row.expires_at;
        row.online = false;
        if let Err(e) = store.set_peer_presence(&row) {
            warn!("failed to expire stale presence: {e}");
            continue;
        }
        wrote = true;
        // Only *our* comrades are news; a lapsed row for someone we have
        // since un-chosen is quietly cleaned up, not announced.
        if store
            .get_contact(&row.peer_npub)
            .ok()
            .flatten()
            .is_some_and(|c| c.comrade)
        {
            let _ = tx.send(BridgeEvent::ComradePresence {
                name: presence_display_name(store, &row.peer_npub),
                peer: row.peer_npub,
                online: false,
                at: expired_at,
            });
        }
    }
    if wrote {
        if let Err(e) = store.flush() {
            warn!("failed to flush expired presence: {e}");
        }
    }
}

/// How a peer should be titled in a presence notification: the alias the user
/// gave them, else the handle they published, else nothing (the frontend
/// falls back to the shortened key, exactly as the chat list does).
fn presence_display_name(
    store: &comrade_storage::EncryptedStore,
    peer_npub: &str,
) -> Option<String> {
    store
        .get_contact(peer_npub)
        .ok()
        .flatten()
        .and_then(|c| user_alias(&c.petname, peer_npub))
        .or_else(|| cached_peer_name(store, peer_npub))
        // A peer's published handle is *their* string: a whitespace-only one
        // would otherwise reach a frontend as "a name", and a notification
        // titled " is online" helps nobody. `None` lets every frontend fall
        // back to the key, which is always meaningful.
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
}

/// Apply one incoming [`PresenceBeacon`] from an accepted conversation:
/// persist what it claims, emit an event on a real transition, and answer a
/// comrade's fresh "I'm here" so they don't wait a heartbeat to see us.
///
/// Three rules keep this honest:
///  * **Expired beacons are ignored entirely.** Relays redeliver
///    at-least-once and the inbox backfills days on every launch — a replayed
///    beacon must never light a green dot (`presence_expires_at` measures
///    from the *send* time, so an old one is already spent).
///  * **Only transitions are events.** A heartbeat from a peer already known
///    to be online is state, not news, so it never re-notifies.
///  * **Only chosen comrades are announced.** A beacon from someone we
///    haven't marked is still recorded — it is the proof that *they* marked
///    us, which is what makes the mutual model discoverable — but it raises
///    nothing, and is never answered.
fn handle_presence_beacon(
    vault: &Arc<VaultEngine>,
    store: Option<&Arc<comrade_storage::EncryptedStore>>,
    tx: &broadcast::Sender<BridgeEvent>,
    peer_npub: &str,
    sender_hex: &str,
    created_at: u64,
    beacon: PresenceBeacon,
) {
    let Some(store) = store else { return };
    let now = now_secs();
    let expires_at = presence_expires_at(beacon.state, created_at, beacon.ttl_secs);
    let online = beacon.state.is_online() && is_online_at(expires_at, now);
    if beacon.state.is_online() && !online {
        tracing::debug!(peer = %peer_npub, created_at, "dropping stale presence beacon");
        return;
    }

    let previous = store.get_peer_presence(peer_npub).ok().flatten();
    // An out-of-order beacon (relays do not guarantee ordering) must not
    // rewind what a newer one already told us.
    if previous
        .as_ref()
        .is_some_and(|p| p.last_seen_at > created_at)
    {
        tracing::debug!(peer = %peer_npub, created_at, "dropping out-of-order presence beacon");
        return;
    }
    let was_online = previous
        .as_ref()
        .is_some_and(|p| p.online && is_online_at(p.expires_at, now));

    let row = comrade_storage::PeerPresence {
        peer_npub: peer_npub.to_string(),
        online,
        last_seen_at: created_at,
        expires_at,
        // Any beacon at all proves they chose us: beacons only go to comrades.
        peer_marked_us: true,
    };
    if let Err(e) = store.set_peer_presence(&row).and_then(|()| store.flush()) {
        warn!("failed to persist peer presence: {e}");
    }

    let is_our_comrade = store
        .get_contact(peer_npub)
        .ok()
        .flatten()
        .is_some_and(|c| c.comrade);
    if is_our_comrade && online != was_online {
        let _ = tx.send(BridgeEvent::ComradePresence {
            peer: peer_npub.to_string(),
            name: presence_display_name(store, peer_npub),
            online,
            at: created_at,
        });
    }

    // Answer a comrade who has just *arrived* — that is the case the reply
    // exists for, and the only one where it carries information: if we already
    // had them online, our own heartbeat is already keeping their view of us
    // fresh, and answering every heartbeat would double this feature's relay
    // traffic to say nothing new. Only our own comrades are ever answered:
    // replying to a peer we haven't chosen would disclose our presence to
    // someone we never opted into telling.
    if is_our_comrade && beacon.wants_reply() && !was_online {
        if let Ok(peer) = PublicKey::parse(sender_hex) {
            spawn_presence_beacons(
                Some(vault.clone()),
                vec![peer],
                PresenceBeacon::online_reply(),
            );
        }
    }
}

/// Apply one incoming [`Nudge`] from an accepted conversation: raise it, once,
/// if it is fresh and from someone the user chose.
///
/// Four rules, each the receiving-side mirror of something the sender promised:
///  * **Stale nudges raise nothing.** Freshness is measured from the send time
///    ([`nudge_expires_at`]), so a nudge delayed by a slow relay or replayed out
///    of the two-day backfill is already spent. Without this, every launch would
///    re-announce every hesitation of the last two days.
///  * **A redelivered wrapper raises nothing.** Relays deliver at-least-once, so
///    the same nudge can arrive twice inside its own TTL; the shared dedup set
///    (the one the call-signal branch uses) makes the second one silent.
///  * **Only our own comrades are announced.** A nudge from someone we have not
///    marked is dropped, not recorded: unlike a presence beacon — whose
///    arrival is *how* the mutual model becomes discoverable
///    (`peer_marked_us`) — a nudge writes no presence state at all, so it can
///    neither advance a "last seen" nor light a dot from outside the one path
///    that owns them.
///  * **Nothing about the draft is knowable here**, because nothing about it
///    was sent. The event carries a name for the notification and no more.
fn handle_nudge(
    store: Option<&Arc<comrade_storage::EncryptedStore>>,
    tx: &broadcast::Sender<BridgeEvent>,
    dedup: &SeenSet,
    peer_npub: &str,
    event_id: &str,
    created_at: u64,
    nudge: Nudge,
) {
    let Some(store) = store else { return };
    if !is_fresh_at(nudge_expires_at(created_at, nudge.ttl_secs), now_secs()) {
        tracing::debug!(peer = %peer_npub, created_at, "dropping stale nudge");
        return;
    }
    if dedup.already_seen(event_id) {
        tracing::debug!(%event_id, "dropping duplicate nudge");
        return;
    }
    let is_our_comrade = store
        .get_contact(peer_npub)
        .ok()
        .flatten()
        .is_some_and(|c| c.comrade);
    if !is_our_comrade {
        return;
    }
    let _ = tx.send(BridgeEvent::ComradeNudge {
        name: presence_display_name(store, peer_npub),
        peer: peer_npub.to_string(),
    });
}

/// Surface one incoming ride signal, if it is still worth acting on.
///
/// The replay story is the nudge's, not the together session's, because there
/// is no session: the **freshness gate** is what keeps a two-day-old
/// backfilled "left in 400 m" off a moving driver's screen (measured from send
/// time, peer TTL clamped — [`comrade_core::ride::ride_expires_at`]), and the
/// **event-id set** is what keeps a relay's at-least-once redelivery from
/// buzzing twice for one tap. Everything about *what* may travel — the
/// catalog, the note cap, the distance cap — was already enforced by
/// [`parse_ride_envelope`] before this is called.
fn handle_ride(
    store: Option<&Arc<comrade_storage::EncryptedStore>>,
    tx: &broadcast::Sender<BridgeEvent>,
    dedup: &SeenSet,
    peer_npub: &str,
    event_id: &str,
    created_at: u64,
    env: RideEnvelope,
) {
    if !is_fresh_at(ride_expires_at(created_at, env.ttl_secs), now_secs()) {
        tracing::debug!(peer = %peer_npub, created_at, "dropping stale ride signal");
        return;
    }
    if dedup.already_seen(event_id) {
        tracing::debug!(%event_id, "dropping duplicate ride signal");
        return;
    }
    let urgency = env.signal.urgency().as_str().to_string();
    let (kind, phrase, maneuver, distance_m, note) = match env.signal {
        RideSignal::Quick { phrase } => {
            ("quick", Some(phrase.as_str().to_string()), None, None, None)
        }
        RideSignal::Route {
            maneuver,
            distance_m,
            note,
        } => (
            "route",
            None,
            Some(maneuver.as_str().to_string()),
            distance_m,
            note,
        ),
    };
    let _ = tx.send(BridgeEvent::RideSignal(RideSignalDto {
        peer: peer_npub.to_string(),
        name: store.and_then(|s| presence_display_name(s, peer_npub)),
        kind: kind.to_string(),
        phrase,
        maneuver,
        distance_m,
        note,
        urgency,
        created_at,
    }));
}

/// Apply one incoming together envelope to the live session.
///
/// Every replay guard this feature has meets here. In order: the **age gate**,
/// which is the only thing standing between a two-day-old backfilled invitation
/// and a fresh one; the **session lookup**, which drops every other signal that
/// does not name a session this process is actually in (and after a relaunch it
/// is in none, so the whole backfill is inert); the **Lamport order**, which
/// makes a redelivered command an exact no-op without needing a dedup set that
/// could be evicted; and, for [`TogetherSignal::Share`] alone, an **event-id
/// set**, because a transfer negotiation has neither a Lamport counter nor an
/// idempotent effect on the frontend holding the connection (AUDIT.md Q18).
///
/// `event_id` is the gift wrap that carried this, when there was one. `None`
/// from the direct peer channel, which has no wrapper and is not replayed.
fn handle_together_envelope(
    tx: &broadcast::Sender<BridgeEvent>,
    link: &TogetherLink,
    peer_npub: &str,
    sender_hex: &str,
    created_at: u64,
    event_id: Option<&str>,
    env: TogetherEnvelope,
) {
    if !signal_is_fresh(created_at, now_secs()) {
        tracing::debug!(peer = %peer_npub, created_at, "dropping stale together signal");
        return;
    }
    let at = now_ms();
    let mut guard = link.session.lock().unwrap();

    // An invitation is the only signal that may create state from nothing.
    if let TogetherSignal::Start {
        content,
        pos_ms,
        playing,
    } = env.signal
    {
        if guard.is_some() {
            // One session at a time — that is what keeps the arbitration a
            // two-party problem. We answer nothing: the inviter's own invite
            // simply times out, and "they didn't pick up" is the honest reading.
            tracing::debug!(peer = %peer_npub, "together invite while already in a session");
            return;
        }
        if link.starts_seen.already_seen(&env.session_id) {
            tracing::debug!(session = %env.session_id, "dropping duplicate together invite");
            return;
        }
        // A peer-supplied id or URL ends up in a `src` attribute. Refuse it here
        // so no frontend has to remember to — `TogetherContent::admissible` is
        // the one place that decides, and it matches exhaustively so a new
        // variant cannot inherit a yes.
        if !content.admissible() {
            // Deliberately no `peer` field. Warn-level reaches logcat, which is
            // readable by whoever holds the device — and the distinction that
            // matters there is *whose* data it is: the owner's own configuration
            // they can already read in Settings, but a contact's npub is about
            // somebody else, who did not consent to being named in a system
            // buffer on this phone. The refusal is what is worth logging; which
            // peer sent it adds nothing a developer can act on.
            tracing::warn!("dropping a together invite we will not play");
            return;
        }
        *guard = Some(TogetherSession {
            id: env.session_id.clone(),
            peer: peer_npub.to_string(),
            peer_hex: sender_hex.to_string(),
            content: content.clone(),
            // They invited us, so they lead and we are the side that corrects.
            we_lead: false,
            our_npub: String::new(),
            joined: false,
            applied: CommandStamp::new(env.seq, peer_npub, !playing),
            local_pos_ms: pos_ms,
            local_playing: false,
            peer_pos_ms: pos_ms,
            peer_playing: playing,
            peer_at_ms: env.at_ms,
            last_heard_ms: at,
            last_seek_ms: 0,
            direct_ready: false,
            direct_evidence_ms: 0,
            local_rate: 1.0,
            local_output_latency_ms: 0,
            peer_output_latency_ms: 0,
            clock: ClockFilter::new(),
            sent_at_ms: std::collections::VecDeque::new(),
            echo_back: Some(ClockEcho {
                your_at_ms: env.at_ms,
                my_recv_ms: at,
            }),
        });
        drop(guard);
        let _ = tx.send(BridgeEvent::TogetherInvited(TogetherInviteDto {
            session_id: env.session_id,
            peer: peer_npub.to_string(),
            content,
            pos_ms,
            playing,
            created_at,
        }));
        return;
    }

    // Everything else needs a session this device is already in, with this peer.
    let Some(session) = guard.as_mut() else {
        tracing::debug!(session = %env.session_id, "together signal for no live session");
        return;
    };
    if session.id != env.session_id || session.peer != peer_npub {
        return;
    }
    session.last_heard_ms = at;
    session.observe_clock(env.echo, env.at_ms, at);

    match env.signal {
        TogetherSignal::Start { .. } => unreachable!("handled above"),
        TogetherSignal::Join => {
            if session.joined {
                return;
            }
            session.joined = true;
            let (session_id, peer) = (session.id.clone(), session.peer.clone());
            drop(guard);
            let _ = tx.send(BridgeEvent::TogetherJoined { session_id, peer });
        }
        TogetherSignal::End => {
            let (session_id, peer) = (session.id.clone(), session.peer.clone());
            *guard = None;
            drop(guard);
            let _ = tx.send(BridgeEvent::TogetherEnded {
                session_id,
                peer,
                by_peer: true,
            });
        }
        TogetherSignal::Share { signal } => {
            // The one signal here with no ordering of its own to make a
            // redelivery harmless: no Lamport stamp to lose against, and no
            // idempotent effect to fall back on. Comrade replays gift-wraps
            // deliberately — `inbox_since` widens the subscription floor back to
            // the watermark on every reconnect — so a second copy is a routine
            // arrival and not a hostile one (AUDIT.md Q18).
            //
            // The frontends guard themselves as well, and the two are not
            // redundant, because they reach different distances. Android's
            // `ShareDecisions.decideArm` is the precise one: it refuses only
            // while a *matching* transfer is live, so a redelivery arriving
            // after the first copy achieved nothing still works. But it is
            // consulted at the two arming points only — `FileTransfer.onTransport`
            // has no redelivery guard of its own — and by then `offerOurCopy`
            // has already hashed the whole file. This one is coarser, stops the
            // copy earlier, and holds for every frontend rather than the one
            // that implemented it.
            //
            // What the coarseness can cost is bounded by the age gate at the top
            // of this function: nothing older than
            // `TOGETHER_SIGNAL_MAX_AGE_SECS` reaches here at all, so this can
            // only suppress a retry inside the same minute in which a retry was
            // possible in the first place.
            //
            // Keyed by the wrapper's event id and nothing else, which is what
            // makes it safe over a negotiation: a second ICE candidate is a
            // different event with a different id and still gets through. Only
            // a literal second copy of one event is dropped.
            if let Some(event_id) = event_id {
                if link.shares_seen.already_seen(event_id) {
                    tracing::debug!(event_id, "dropping duplicate together share signal");
                    return;
                }
            }
            // Otherwise straight through to the frontend, which owns the peer
            // connection this is negotiating. It has already passed the
            // acceptance gate, the age gate and the session-id check above —
            // which is the entire argument for putting it inside this envelope
            // rather than giving it one of its own.
            let (session_id, peer) = (session.id.clone(), session.peer.clone());
            drop(guard);
            let _ = tx.send(BridgeEvent::TogetherShare(TogetherShareDto {
                session_id,
                peer,
                signal,
            }));
        }
        TogetherSignal::State {
            pos_ms,
            playing,
            effective_at_ms,
        } => {
            let incoming = CommandStamp::new(env.seq, peer_npub, !playing);
            if !incoming.wins_over(&session.applied) {
                // Either a redelivered copy, or our own command outranks theirs
                // and they will follow our next heartbeat. Either way: silence,
                // which is what stops the two devices arguing.
                tracing::debug!(session = %env.session_id, "together command did not win");
                return;
            }
            let clock = session.clock.estimate(at);
            // The instant this state took effect on their clock. Absent means
            // "when I sent it", which is what a sender with no way to schedule
            // ahead of its own transport says.
            let effective = effective_at_ms.unwrap_or(env.at_ms);
            let apply = command_apply(pos_ms, effective, playing, &clock, at);
            // **This is the improvement over a position-swapping protocol.** We
            // do not adopt `pos_ms` — that number was true when they sent it,
            // and adopting it verbatim lands us behind by exactly the flight
            // time, every time, invisibly, because both sides agree on the
            // number they exchanged. We evaluate their timeline at *our* now.
            let (landed_ms, apply_in_ms) = match apply {
                CommandApply::Now { pos_ms, .. } => (pos_ms, 0),
                CommandApply::At { pos_ms, in_ms, .. } => (pos_ms, in_ms),
            };
            let expected = projected_peer_pos_ms(&SyncSample {
                local_pos_ms: session.local_pos_ms,
                local_playing: session.local_playing,
                local_seq: session.applied.seq,
                peer_pos_ms: session.peer_pos_ms,
                peer_playing: session.peer_playing,
                peer_seq: env.seq,
                peer_at_ms: session.peer_at_ms,
                now_ms: at,
                last_seek_ms: session.last_seek_ms,
                local_rate: session.local_rate,
                clock,
                we_lead: session.we_lead,
                local_output_latency_ms: session.local_output_latency_ms,
                peer_output_latency_ms: session.peer_output_latency_ms,
            })
            .max(0) as u64;
            let change = describe_state_change(session.peer_playing, playing, expected, landed_ms);
            session.applied = incoming;
            // Their anchor is what they said, at the instant they said it held.
            session.peer_pos_ms = pos_ms;
            session.peer_playing = playing;
            session.peer_at_ms = effective;
            session.local_pos_ms = landed_ms;
            session.local_playing = playing;
            let session_id = session.id.clone();
            drop(guard);
            let _ = tx.send(BridgeEvent::TogetherCommand(TogetherCommandDto {
                session_id,
                pos_ms: landed_ms,
                playing,
                change,
                apply_in_ms,
            }));
        }
        TogetherSignal::Heartbeat {
            pos_ms,
            playing,
            applied_seq,
            output_latency_ms,
        } => {
            session.peer_pos_ms = pos_ms;
            session.peer_playing = playing;
            session.peer_at_ms = env.at_ms;
            session.peer_output_latency_ms = output_latency_ms;
            let clock = session.clock.estimate(at);
            let sample = SyncSample {
                local_pos_ms: session.local_pos_ms,
                local_playing: session.local_playing,
                local_seq: session.applied.seq,
                peer_pos_ms: pos_ms,
                peer_playing: playing,
                peer_seq: applied_seq,
                peer_at_ms: env.at_ms,
                now_ms: at,
                last_seek_ms: session.last_seek_ms,
                local_rate: session.local_rate,
                clock,
                we_lead: session.we_lead,
                local_output_latency_ms: session.local_output_latency_ms,
                peer_output_latency_ms: output_latency_ms,
            };
            let verdict = sync_verdict(&sample, session.content.tuning());
            if matches!(verdict, SyncVerdict::Hold) {
                // The steady state, and the reason a ten-second heartbeat is not
                // a periodic producer on the critical bus: nothing is emitted.
                return;
            }
            let drift_ms = sample.local_pos_ms as i64 - projected_peer_pos_ms(&sample);
            match verdict {
                SyncVerdict::Seek { pos_ms } => {
                    session.last_seek_ms = at;
                    session.local_pos_ms = pos_ms;
                    // A jump supersedes any trim that was closing the old gap.
                    session.local_rate = 1.0;
                }
                SyncVerdict::Adopt {
                    pos_ms,
                    playing,
                    seq,
                } => {
                    session.applied = CommandStamp::new(seq, peer_npub, !playing);
                    session.local_pos_ms = pos_ms;
                    session.local_playing = playing;
                    session.peer_pos_ms = pos_ms;
                    // A jump supersedes any trim that was closing the old gap.
                    session.local_rate = 1.0;
                }
                // Remember what we asked the player for: the next verdict needs
                // it to know whether there is a trim left to take back off.
                SyncVerdict::Nudge { rate } => session.local_rate = rate,
                _ => {}
            }
            let session_id = session.id.clone();
            drop(guard);
            let _ = tx.send(BridgeEvent::TogetherCorrection(TogetherCorrectionDto {
                session_id,
                verdict,
                drift_ms,
                quality_ms: clock.uncertainty_at(at),
            }));
        }
    }
}

/// Fire a delivered receipt back to `sender_hex` for `message_id` (best-effort;
/// only ever called for accepted conversations).
fn send_delivered_receipt(
    vault: &Arc<VaultEngine>,
    mesh: Option<&LocalRadios>,
    sender_hex: &str,
    message_id: &str,
) {
    let Ok(peer) = PublicKey::parse(sender_hex) else {
        return;
    };
    let Ok(json) = Receipt::new(ReceiptKind::Delivered, vec![message_id.to_string()]).to_json()
    else {
        return;
    };
    let vault = vault.clone();
    let mesh = mesh.cloned();
    let receipt_id = format!("receipt:{message_id}");
    tokio::spawn(async move {
        if let Err(e) = vault.send_dm(&peer, &json).await {
            tracing::debug!("delivered receipt not sent over a relay: {e}");
            // Offline is exactly when the receipt matters most: it is what
            // clears the *sender's* outbox, so without this a message delivered
            // over the mesh would keep being retried until its attempt cap.
            if let Some(mesh) = mesh {
                mesh.send(&peer, &receipt_id, &json, None, now_secs()).await;
            }
        }
    });
}

/// Write the outbox snapshot into the encrypted store. Best-effort: a failure
/// here costs durability across a kill, never the live queue.
fn persist_outbox(store: &Arc<comrade_storage::EncryptedStore>, outbox: &Arc<Outbox>) {
    let snapshot = outbox.snapshot();
    if let Err(e) = store
        .put(OUTBOX_TREE, OUTBOX_KEY, &snapshot)
        .and_then(|()| store.flush())
    {
        warn!("failed to persist the outbox: {e}");
    }
}

/// Load the device anonymity seed, creating and sealing one on first use.
///
/// This is the root secret every anonymous persona is derived from
/// ([`anon::derive_scoped`]). It lives in the encrypted store next to the
/// identity and is destroyed by the panic wipe, after which past personas cannot
/// be regenerated — which is the intent.
fn load_or_create_device_seed(
    store: &Arc<comrade_storage::EncryptedStore>,
) -> Result<anon::DeviceSeed, UiError> {
    if let Some(bytes) = store
        .get_bytes(SETTINGS_TREE, DEVICE_SEED_KEY)
        .map_err(|e| UiError::Storage(e.to_string()))?
    {
        if let Ok(seed) = anon::DeviceSeed::from_slice(&bytes) {
            return Ok(seed);
        }
        // A wrong-length seed means a corrupt record; replacing it is better
        // than deriving personas from truncated key material.
        warn!("stored anonymity seed was malformed — generating a fresh one");
    }
    let seed = anon::DeviceSeed::generate();
    store
        .put_bytes(SETTINGS_TREE, DEVICE_SEED_KEY, seed.as_bytes())
        .and_then(|()| store.flush())
        .map_err(|e| UiError::Storage(e.to_string()))?;
    Ok(seed)
}

// ── The sealed-frame ingress, shared by every radio ──────────────────────────

/// Everything an incoming sealed frame needs, independent of what carried it.
///
/// Two radios feed this — [`ComradeRuntime::spawn_mesh_dm_loop`] off WiFi and
/// [`ComradeRuntime::spawn_ble_dm_loop`] off Bluetooth — and both hand every
/// frame to [`Self::accept`]. That is the single most important structural
/// decision in the offline stack, and it predates BLE: a second ingress would
/// have meant a second copy of the message-request gating, the persistence, the
/// receipt logic and the dedup rules, and two copies of privacy rules drift.
struct SealedIngress {
    vault: Arc<VaultEngine>,
    keys: nostr_sdk::prelude::Keys,
    store: Option<Arc<comrade_storage::EncryptedStore>>,
    tx: broadcast::Sender<BridgeEvent>,
    call_dedup: Arc<SeenSet>,
    transport_dedup: Arc<SeenSet>,
    outbox: Arc<Outbox>,
    /// Both local radios — how a receipt answers a frame that arrived with no
    /// internet at all. `None` leaves the receipt to a relay.
    mesh: Option<LocalRadios>,
    together: TogetherLink,
    pay_regex: Option<PayRegex>,
}

impl SealedIngress {
    /// Open a frame if it is ours, and dispatch it exactly as a relay DM.
    ///
    /// The overwhelmingly common outcome is "someone else's mail": every device
    /// in range sees every frame, and one HMAC comparison against our rotating
    /// tags rejects it. That is the design working, not an error.
    fn accept(&self, envelope: &Envelope, now: u64) {
        let opened = match open_dm(&self.keys, envelope, now) {
            Ok(None) => return,
            Ok(Some(opened)) => opened,
            Err(e) => {
                tracing::debug!("a frame addressed to us failed to open: {e}");
                return;
            }
        };

        let content = opened.dm.content;
        let upi_intents = self
            .pay_regex
            .as_ref()
            .map(|re| extract_upi_intents(&content, re))
            .unwrap_or_default();
        let msg = VaultMessage {
            event_id: opened.dm.id,
            sender_pubkey: opened.sender.to_hex(),
            content,
            created_at: opened.dm.created_at,
            upi_intents,
            reply_to: opened.dm.reply_to,
        };
        let route = DmRoute {
            label: TRANSPORT_MESH,
            dedup: &self.transport_dedup,
            mesh: self.mesh.as_ref(),
            together: Some(&self.together),
        };
        dispatch_incoming_dm(
            &self.vault,
            self.store.as_ref(),
            &self.tx,
            &self.call_dedup,
            &self.outbox,
            &route,
            msg,
        );
    }
}

// ── Local-network delivery (see docs/OFFLINE_DELIVERY.md) ────────────────────

/// A handle for putting sealed mail onto the local network.
///
/// Wraps the running mesh engine plus our keys, which is everything needed to
/// seal a DM for a peer and flood it to whoever is on this WiFi. Cloneable and
/// cheap, so a fire-and-forget task can own one.
#[derive(Clone)]
struct MeshLink {
    engine: Arc<SaathiEngine>,
}

impl MeshLink {
    /// Publish an already-sealed envelope to the local mesh. Returns whether
    /// the mesh accepted the frame — `false` when nobody else is on the network
    /// (gossipsub has no peer to publish to), which is not an error: the
    /// message stays queued in the outbox exactly as before, and the caller
    /// falls through to the next radio.
    ///
    /// Takes a sealed envelope rather than the message, because both radios
    /// carry the *same* one: sealing twice would produce two different
    /// ciphertexts for one message and defeat the receiver's dedup.
    async fn publish(&self, envelope: &Envelope, peer: &PublicKey) -> bool {
        match self.engine.publish_sealed(envelope).await {
            Ok(()) => {
                tracing::info!(peer = %peer, "DM sealed onto the local mesh");
                true
            }
            Err(e) => {
                tracing::debug!("mesh: nobody to deliver to: {e}");
                false
            }
        }
    }
}

/// Both local radios behind one handle, plus the keys to seal with.
///
/// Every place that falls back to "no internet, but they might be near" holds
/// one of these rather than a specific transport — the sending path, the
/// delivered receipt, a together signal. That is deliberate: when Bluetooth was
/// added, a handle per radio would have meant finding every one of those sites
/// and remembering to try the second one, and the site that got missed would be
/// the receipt, whose absence makes the sender retry to its attempt cap.
#[derive(Clone)]
struct LocalRadios {
    /// The WiFi mesh, when the Saathi engine is running.
    mesh: Option<MeshLink>,
    /// Bluetooth. Always present, inert until a platform radio marks it active.
    ble: Arc<BleRouter>,
    keys: nostr_sdk::prelude::Keys,
}

impl LocalRadios {
    /// Seal a message once and put it on **both** radios.
    ///
    /// Not "WiFi first, Bluetooth if that fails" — that was this function's
    /// original shape and it was wrong in the way that matters, because
    /// [`MeshLink::publish`] returning `true` does not mean the message
    /// arrived. It means gossipsub accepted the frame, which requires only that
    /// *somebody* subscribes to the sealed topic — not the recipient. So a
    /// phone with any mesh peer at all would return early and never touch
    /// Bluetooth, and the message was gone if that peer was not the person
    /// being written to.
    ///
    /// Two real cases made that fatal rather than theoretical. A hotspot with
    /// client isolation lets mDNS through the AP while blocking phone-to-phone
    /// traffic, so peers are discovered, a publish is accepted, and nothing is
    /// carried. And any third device on the network — another Comrade, an
    /// earlier test runtime — is enough to make the publish succeed while the
    /// recipient is not there at all. In both, Bluetooth would have worked and
    /// was never asked.
    ///
    /// This is the same error the `peer_count` indicator made: treating an
    /// intermediate success as delivery. The rule this file now follows is that
    /// **only a receipt proves arrival**, so both radios carry every frame and
    /// the message stays queued until the recipient says otherwise.
    ///
    /// Sending twice is cheap and safe by construction: one seal means one
    /// ciphertext, so a device hearing the frame on both radios dedups it on
    /// envelope id and opens it once. Sealing per-radio would have produced two
    /// ciphertexts for one message and defeated exactly that.
    ///
    /// Returns whether *any* radio took it — the caller uses this only to
    /// decide whether the local path was worth trying, never as proof of
    /// delivery.
    async fn send(
        &self,
        peer: &PublicKey,
        id: &str,
        content: &str,
        reply_to: Option<String>,
        created_at: u64,
    ) -> bool {
        let dm = MeshDm::new(id, content, reply_to, created_at);
        let envelope = match seal_dm(peer, &self.keys, &dm, created_at) {
            Ok(envelope) => envelope,
            Err(e) => {
                // Oversize is the realistic case: sealed mail is capped at
                // 16 KiB and a long message simply waits for a relay.
                tracing::debug!("could not seal a DM for the local radios: {e}");
                return false;
            }
        };
        // Both, always. No `?`, no early return: a radio that cannot take the
        // frame must not stop the other one from trying.
        let on_mesh = match &self.mesh {
            Some(mesh) => mesh.publish(&envelope, peer).await,
            None => false,
        };
        let on_ble = self.ble.enqueue(&envelope);
        on_mesh || on_ble
    }
}

// ── Bluetooth delivery: no router, no relay, no infrastructure ───────────────

/// Flood-dedup capacity: packet ids we remember in order to not relay the same
/// packet twice.
///
/// Sized for a crowd, not a living room — a march or a venue where a few dozen
/// devices are each relaying — because the failure mode of forgetting too early
/// is a packet that circulates again, which is exactly what the dedup exists to
/// stop.
const BLE_SEEN_CAPACITY: usize = 2048;

/// How long a packet id is remembered for flood dedup.
///
/// Comfortably longer than a packet can plausibly still be in flight across
/// [`comrade_core::dak::ble::DEFAULT_TTL`] hops, so the last echo of a packet
/// dies before we would forget it and start relaying it again.
const BLE_SEEN_TTL_SECS: u64 = 300;

/// Conservative usable payload per BLE write, until the radio negotiates up.
///
/// The default ATT MTU is 23 bytes; every modern stack requests more, and
/// Android commonly settles around 247. Starting here rather than at the
/// pessimistic floor means the first message out does not get shredded into
/// dozens of packets while the negotiation lands, and
/// [`BleRouter::set_mtu`] corrects it the moment the platform reports a real
/// number.
const BLE_DEFAULT_MTU: usize = 185;

/// Outbound packets held for the radio to collect.
///
/// The radio drains this on its own cadence, so it needs a ceiling: a phone
/// with Bluetooth off, or out of range of everything, must not accumulate
/// packets until the process dies. Past this the oldest go — the outbox is
/// still holding the *message*, so a dropped packet costs a retry, not a
/// message.
const BLE_OUTBOUND_CAPACITY: usize = 512;

/// The policy half of the Bluetooth transport: fragmentation, reassembly,
/// controlled flooding, and the two queues the radio talks to.
///
/// **This is not the radio.** GATT roles, advertising, scanning and MTU
/// negotiation live in `android/…/ble/BleMeshService.kt`; the platform calls
/// [`Self::deliver`] with what it heard and [`Self::drain_outbound`] for what
/// to send. Everything that decides what goes on the wire, what gets forwarded,
/// and what a stranger with a radio can make this process allocate is here,
/// where it can be tested without a radio at all.
///
/// Shared behind an `Arc` by the runtime, the send path and the FFI, so its
/// interior is locked rather than `&mut` — the same posture
/// [`comrade_core::dak::Outbox`] takes.
pub struct BleRouter {
    /// `false` until a platform BLE layer reports **at least one linked peer**
    /// — not merely that the radio is on.
    ///
    /// That distinction is the same one [`comrade_core::saathi::MeshReach`]
    /// draws between discovered and deliverable, and for the same reason: a
    /// running radio with nobody in range is not a route, and treating it as
    /// one makes every outbox flush spend an attempt against a transport that
    /// cannot deliver — eight of which mark the message failed. Feeds
    /// [`TransportReach`], so a build with no BLE service at all — desktop, the
    /// CLI, a test — simply never routes over Bluetooth.
    active: std::sync::atomic::AtomicBool,
    mtu: std::sync::atomic::AtomicUsize,
    /// Monotonic source of packet ids. Only uniqueness matters, and only
    /// against *our own* recent packets: the id is a dedup and reassembly key,
    /// never an identifier, and it is paired with random-per-process entropy so
    /// two devices restarting together do not collide on `1`.
    next_packet_id: std::sync::atomic::AtomicU64,
    inbound: mpsc::Sender<Envelope>,
    inbound_rx: Mutex<Option<mpsc::Receiver<Envelope>>>,
    outbound: Mutex<std::collections::VecDeque<Vec<u8>>>,
    reassembler: Mutex<Reassembler>,
    /// Packet ids already relayed. The thing that turns a set of pairwise links
    /// into a mesh instead of a broadcast storm.
    seen: SeenSet,
}

impl BleRouter {
    fn new() -> Self {
        let (inbound, inbound_rx) = mpsc::channel(128);
        Self {
            active: std::sync::atomic::AtomicBool::new(false),
            mtu: std::sync::atomic::AtomicUsize::new(BLE_DEFAULT_MTU),
            // Seeded from the process's own randomness rather than zero, so two
            // phones that start at the same moment do not both mint packet id 1
            // and dedup each other's first message out of existence.
            next_packet_id: std::sync::atomic::AtomicU64::new(random_packet_seed()),
            inbound,
            inbound_rx: Mutex::new(Some(inbound_rx)),
            outbound: Mutex::new(std::collections::VecDeque::new()),
            reassembler: Mutex::new(Reassembler::new()),
            seen: SeenSet::with_ttl(
                BLE_SEEN_CAPACITY,
                std::time::Duration::from_secs(BLE_SEEN_TTL_SECS),
            ),
        }
    }

    /// Whether Bluetooth is a route worth trying right now.
    pub fn is_active(&self) -> bool {
        self.active.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Told by the platform whenever the set of linked peers becomes empty or
    /// non-empty. `true` means "there is somebody to write to right now", not
    /// "the radio is powered".
    pub fn set_active(&self, active: bool) {
        self.active
            .store(active, std::sync::atomic::Ordering::Relaxed);
        if !active {
            // Nothing queued is deliverable over a radio that is off, and the
            // outbox still holds every message, so dropping the packets keeps
            // memory honest rather than replaying a stale burst on the next
            // connection.
            self.lock_outbound().clear();
        }
    }

    /// Told by the platform after MTU negotiation, as **usable payload bytes**
    /// per write (the platform subtracts its own ATT overhead first).
    pub fn set_mtu(&self, mtu: usize) {
        self.mtu.store(
            mtu.max(ble::MIN_USABLE_MTU),
            std::sync::atomic::Ordering::Relaxed,
        );
    }

    /// Fragment a sealed envelope and queue it for transmission.
    ///
    /// Returns whether it was queued: `false` means BLE is not a live route, or
    /// the envelope is too large to fragment at the current MTU. Never blocks
    /// on a radio — the caller's outbox is the retry mechanism, exactly as on
    /// the WiFi path.
    pub fn enqueue(&self, envelope: &Envelope) -> bool {
        if !self.is_active() {
            return false;
        }
        let packet_id = self
            .next_packet_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mtu = self.mtu.load(std::sync::atomic::Ordering::Relaxed);
        let fragments = match ble::fragment(&envelope.encode(), packet_id, ble::DEFAULT_TTL, mtu) {
            Ok(f) => f,
            Err(e) => {
                tracing::debug!("ble: could not fragment an envelope: {e}");
                return false;
            }
        };
        // Ours, so it must never come back to us as something to relay.
        self.seen.already_seen(&packet_id.to_string());
        self.push_outbound(fragments.iter().map(ble::Fragment::encode));
        core_metrics::record(CoreMetric::BleSent);
        true
    }

    /// Hand in a packet heard from a BLE peer.
    ///
    /// Does three things, in this order, because each bounds the next: dedup
    /// (have we handled this packet already?), relay (pass it on, one hop
    /// spent), and reassemble (is this the fragment that completes an
    /// envelope?). A completed envelope goes to the shared sealed-frame
    /// ingress — addressed to us or not, since deciding that needs keys this
    /// layer deliberately does not have.
    pub fn deliver(&self, packet: &[u8], now: u64) {
        let fragment = match ble::Fragment::decode(packet) {
            Ok(f) => f,
            Err(e) => {
                tracing::debug!("ble: undecodable packet: {e}");
                core_metrics::record(CoreMetric::BleFragmentDropped);
                return;
            }
        };

        // Relay before reassembly: a frame for someone two hops away has to
        // move on even though we can never open it — that forwarding *is* the
        // mesh.
        //
        // Deduped per **fragment**, not per packet. `packet_id` is shared by
        // every fragment of one envelope (it is also the reassembly key), so
        // keying the flood filter on it alone meant a relay forwarded fragment
        // 0 and then discarded 1..n as echoes of it. One-hop delivery was
        // unaffected and looked fine; anything that had to cross a middle
        // device arrived permanently incomplete, and only for messages too big
        // for a single fragment — which is most of them.
        //
        // Still bounded: each distinct fragment is forwarded at most once per
        // device, which is all a flood filter has to guarantee, and the TTL
        // bounds the rest.
        let relay_key = format!("{}:{}", fragment.packet_id, fragment.index);
        if !self.seen.already_seen(&relay_key) {
            if let Some(onward) = fragment.relayed() {
                self.push_outbound(std::iter::once(onward.encode()));
                core_metrics::record(CoreMetric::BleRelayed);
            }
        }

        let Some(bytes) = self
            .reassembler
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .accept(fragment, now)
        else {
            return;
        };
        match Envelope::decode(&bytes) {
            Ok(envelope) => {
                // `try_send`, not `send`: this runs on the platform's callback
                // thread and must never block the radio. A full channel means
                // the ingress is wedged, which is worth a log line and a
                // dropped frame rather than a stalled BLE stack.
                if self.inbound.try_send(envelope).is_err() {
                    tracing::warn!("ble: ingress is full, dropping a rebuilt envelope");
                    core_metrics::record(CoreMetric::BleFragmentDropped);
                }
            }
            Err(e) => {
                tracing::debug!("ble: rebuilt bytes were not an envelope: {e}");
                core_metrics::record(CoreMetric::BleFragmentDropped);
            }
        }
    }

    /// Packets the radio should transmit, oldest first. Drains the queue.
    pub fn drain_outbound(&self) -> Vec<Vec<u8>> {
        self.lock_outbound().drain(..).collect()
    }

    /// The rebuilt-envelope stream, taken once by the runtime's BLE loop.
    fn subscribe_inbound(&self) -> mpsc::Receiver<Envelope> {
        self.inbound_rx
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
            .unwrap_or_else(|| mpsc::channel(1).1)
    }

    /// Poison-tolerant, for [`comrade_core::dak::Outbox::lock`]'s reason: a
    /// panic while holding this leaves a plain `VecDeque` behind, and
    /// recovering beats turning one panic into two on a delivery path.
    fn lock_outbound(&self) -> std::sync::MutexGuard<'_, std::collections::VecDeque<Vec<u8>>> {
        self.outbound.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn push_outbound(&self, packets: impl Iterator<Item = Vec<u8>>) {
        let mut queue = self.lock_outbound();
        for packet in packets {
            if queue.len() >= BLE_OUTBOUND_CAPACITY {
                queue.pop_front();
                core_metrics::record(CoreMetric::BleFragmentDropped);
            }
            queue.push_back(packet);
        }
    }
}

/// A random starting point for packet ids.
///
/// Only uniqueness against our own recent packets matters — the id is a dedup
/// and reassembly key, never an identifier — but starting every device at zero
/// would have two phones booting together mint the same first id and dedup each
/// other's opening message out of existence. Derived from a throwaway keypair
/// because that is the entropy source the crate already has.
fn random_packet_seed() -> u64 {
    let bytes = nostr_sdk::prelude::Keys::generate().public_key().to_bytes();
    u64::from_be_bytes(bytes[..8].try_into().unwrap_or([0; 8]))
}

impl Default for BleRouter {
    fn default() -> Self {
        Self::new()
    }
}

/// Which routes are usable at this instant.
///
/// Availability, not preference — the answer to "would trying this cost
/// anything but time?". Both fields are cheap live probes:
/// `VaultEngine::has_connected_relay` and `MeshReach::can_deliver`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct TransportReach {
    /// At least one relay is connected.
    relay: bool,
    /// At least one peer on this network is subscribed to sealed mail — i.e.
    /// a publish would actually reach somebody. Deliberately the *deliverable*
    /// count and not the discovered one: see [`comrade_core::saathi::MeshReach`].
    mesh: bool,
}

/// Which transport a send should try first, and whether the other is still
/// worth trying afterwards.
///
/// Two inputs decide this, in that order of authority:
///
/// 1. **What is actually reachable.** A route that is down is not a route. With
///    no relay connected, trying one first costs a five-second
///    `wait_for_any_relay` before the local network is even attempted — on a
///    phone in airplane mode that is the whole difference between a message
///    arriving and a message sitting under a clock icon. With nobody on the
///    local network, the mesh is equally pointless to lead with.
/// 2. **What the user asked for**, from the app bar. This decides when *both*
///    routes are up, which is the only time it is a real choice rather than a
///    way to make the app slower.
///
/// Precedence is an order, not an exclusion — a message that the preferred
/// route cannot carry still takes the other one, because the product's promise
/// is that the message arrives, not that it arrives by a particular radio.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SendPlan {
    /// Try this first.
    local_first: bool,
    /// Try the *other* transport even if the first one accepted the message.
    ///
    /// Only ever true for a local-first retry. A relay `OK` means a relay has
    /// stored the message, so there is nothing to chase; a mesh publish only
    /// means *some* peer on the network took the frame, which may not be the
    /// recipient. After a couple of unacknowledged rounds the message stops
    /// waiting on the local network and also goes out over a relay.
    force_both: bool,
}

/// Mesh-only rounds a local-first message gets before a relay is tried too.
///
/// The outbox retries roughly once a minute, so this is about two minutes of
/// "they might still walk back into WiFi range" before spending the internet.
const LOCAL_FIRST_PATIENCE: u8 = 2;

impl SendPlan {
    fn for_attempt(prefer_local: bool, reach: TransportReach, attempts: u8) -> Self {
        // Availability first: with exactly one route up, the preference is not
        // a choice between two things, and honouring it would only add the dead
        // route's timeout to every send.
        let local_first = match (reach.relay, reach.mesh) {
            (false, true) => true,
            (true, false) => false,
            // Both up, or neither. With both, the user's setting is the real
            // tie-break it was designed to be. With neither, the message is
            // going to the outbox whatever we do, so keep the order stable and
            // predictable rather than flapping.
            _ => prefer_local,
        };
        Self {
            local_first,
            force_both: local_first && attempts >= LOCAL_FIRST_PATIENCE,
        }
    }
}

/// The route a DM arrived by, plus what the ingress needs to answer over the
/// same route when there is no internet.
struct DmRoute<'a> {
    /// [`TRANSPORT_RELAY`] or [`TRANSPORT_MESH`].
    label: &'static str,
    /// Cross-transport dedup set — see [`CROSS_TRANSPORT_DEDUP_SECS`].
    dedup: &'a SeenSet,
    /// The local radios — WiFi mesh and Bluetooth — when the caller has them.
    /// How a receipt answers a frame that arrived with no internet at all.
    mesh: Option<&'a LocalRadios>,
    /// The live watch-together session, when the caller has one to offer.
    ///
    /// It rides here rather than as a ninth positional parameter because it is
    /// per-ingress context of exactly the same kind as `dedup` and `mesh` — the
    /// state this delivery may act on — and because a parameter would have to be
    /// threaded through thirty-odd call sites that have nothing to do with it.
    /// `None` in the tests that are not about together at all.
    together: Option<&'a TogetherLink>,
}

/// The together state one ingress path may touch: the single live session, and
/// the two small sets of ids already handled — invitations by session id,
/// transfer signals by the event id of the wrapper that carried them.
struct TogetherLink {
    session: Arc<Mutex<Option<TogetherSession>>>,
    starts_seen: Arc<SeenSet>,
    shares_seen: Arc<SeenSet>,
}

impl DmRoute<'_> {
    /// Whether this message has already been ingested over the *other*
    /// transport recently, i.e. it is the second copy of one message that took
    /// two routes. Records it either way.
    fn is_cross_transport_duplicate(&self, peer_npub: &str, content: &str) -> bool {
        let key = content_key(content, CONTENT_KEY_PREFIX);
        let other = if self.label == TRANSPORT_MESH {
            TRANSPORT_RELAY
        } else {
            TRANSPORT_MESH
        };
        if self.dedup.contains(&format!("{peer_npub}|{key}|{other}")) {
            core_metrics::record(CoreMetric::DuplicateDropped);
            return true;
        }
        self.dedup
            .already_seen(&format!("{peer_npub}|{key}|{}", self.label));
        false
    }
}

/// Read the persisted inbox high-watermark (unix seconds of the newest
/// message processed so far), if any has been recorded yet.
fn read_watermark(store: &comrade_storage::EncryptedStore) -> Option<u64> {
    store.get(SETTINGS_TREE, VAULT_WATERMARK_KEY).ok().flatten()
}

/// Advance the persisted inbox high-watermark to `created_at` if it is newer
/// than what's stored (a monotonic max — out-of-order delivery must never
/// move it backwards). Read back at the next launch by
/// [`ComradeRuntime::spawn_event_loops`] to widen the backfill window past
/// the standard gift-wrap skew when this device was offline longer than that.
fn advance_watermark(store: &comrade_storage::EncryptedStore, created_at: u64) {
    let current = read_watermark(store).unwrap_or(0);
    if created_at > current {
        if let Err(e) = store
            .put(SETTINGS_TREE, VAULT_WATERMARK_KEY, &created_at)
            .and_then(|()| store.flush())
        {
            warn!("failed to advance vault watermark: {e}");
        }
    }
}

/// Route one decrypted incoming DM: block-drop, control envelopes
/// (receipt/profile-share/call), media, or plain chat — applying the message
/// -request gate throughout. Runs inside the Vault inbox Tokio task.
///
/// Idempotent end-to-end: relays redeliver at-least-once and the inbox
/// subscription backfills up to 2 days (widened further on `since_floor`) on
/// every launch, so this may see the exact same wrapper event more than
/// once. Plain-text/media DMs are deduped by checking persistence *before*
/// emitting/receipting (persist first, so a crash between persist and emit
/// self-heals as "already seen, no event" next time); call signals are
/// deduped by `dedup` and dropped once stale — see the call-envelope branch.
fn dispatch_incoming_dm(
    vault: &Arc<VaultEngine>,
    store: Option<&Arc<comrade_storage::EncryptedStore>>,
    tx: &broadcast::Sender<BridgeEvent>,
    dedup: &SeenSet,
    outbox: &Arc<Outbox>,
    route: &DmRoute<'_>,
    msg: VaultMessage,
) {
    if let Some(store) = store {
        advance_watermark(store, msg.created_at);
    }
    let peer_npub = to_npub(&msg.sender_pubkey);
    let gate = store
        .map(|s| conversation_gate(s, &peer_npub))
        .unwrap_or(IncomingGate::Pending);
    if matches!(gate, IncomingGate::Blocked) {
        return;
    }

    // 1) Receipt — advance our outgoing statuses (accepted conversations only).
    if let Some(receipt) = parse_receipt(&msg.content) {
        if matches!(gate, IncomingGate::Accepted) {
            // A receipt proves the peer holds the message, so anything still
            // queued for a retry is done — clear it before the status update so
            // a concurrent flush cannot re-send an already-delivered message.
            if !outbox.ack(&peer_npub, &receipt.message_ids).is_empty() {
                if let Some(store) = store {
                    persist_outbox(store, outbox);
                }
            }
            if let Some(store) = store {
                let status = receipt.status.as_str();
                let mut changed = Vec::new();
                for id in &receipt.message_ids {
                    if store.set_message_status(id, status).unwrap_or(false) {
                        changed.push(id.clone());
                    }
                }
                let _ = store.flush();
                if !changed.is_empty() {
                    let _ = tx.send(BridgeEvent::MessageStatus {
                        peer: peer_npub,
                        message_ids: changed,
                        status: status.to_string(),
                    });
                }
            }
        }
        return;
    }

    // 2) Profile share — cache the peer's shared @handle (any non-blocked peer;
    //    they revealed it by reaching out or accepting).
    if let Some(profile) = parse_profile_share(&msg.content) {
        if let (Some(store), Some(name)) = (store, profile.username) {
            cache_pushed_peer_name(store, &peer_npub, &name);
            let _ = tx.send(BridgeEvent::PeerProfileUpdated {
                peer: peer_npub,
                name: Some(name),
            });
        }
        return;
    }

    // 3) Presence beacon — a comrade saying "I'm here" / "I'm going". Only
    //    from an established conversation: a stranger must not be able to
    //    push presence state (or trigger a reply that discloses ours) before
    //    their message request is accepted. Returns either way, so a beacon
    //    from an unaccepted peer is dropped silently rather than surfacing as
    //    a message request full of JSON.
    if let Some(beacon) = parse_presence_beacon(&msg.content) {
        if matches!(gate, IncomingGate::Accepted) {
            handle_presence_beacon(
                vault,
                store,
                tx,
                &peer_npub,
                &msg.sender_pubkey,
                msg.created_at,
                beacon,
            );
        }
        return;
    }

    // 4) Nudge — a comrade wrote something for us and did not send it. Gated
    //    exactly like a presence beacon: a stranger must not be able to page us
    //    before their message request is accepted, and returning either way
    //    keeps a nudge from an unaccepted peer from surfacing as a message
    //    request full of JSON.
    if let Some(nudge) = parse_nudge(&msg.content) {
        if matches!(gate, IncomingGate::Accepted) {
            handle_nudge(
                store,
                tx,
                dedup,
                &peer_npub,
                &msg.event_id,
                msg.created_at,
                nudge,
            );
        }
        return;
    }

    // 4a) Ride signal — one seat of a motorcycle to the other. Gated exactly
    //     like a nudge: a stranger must not be able to put "pull over" on a
    //     moving driver's screen, and returning either way keeps an ungated
    //     one from surfacing as a message request full of JSON. Freshness and
    //     same-transport dedup live in `handle_ride`.
    //
    //     The **cross-transport** check is here rather than there because it is
    //     a property of the delivery, not of the signal: a ride signal is sent
    //     on the local radios *and* the relay (`RuntimeHandles::ride_send`
    //     says why), and the two copies carry different wrapper event ids, so
    //     the event-id set inside `handle_ride` cannot pair them. Without this
    //     line one tap buzzes a driver twice.
    //
    //     It keys on content, which is only safe because `RideEnvelope::at_ms`
    //     makes two *sends* differ — otherwise the fixed catalog would make a
    //     deliberately repeated "pull over" identical to the first and it would
    //     be eaten for the next two minutes.
    if let Some(env) = parse_ride_envelope(&msg.content) {
        if matches!(gate, IncomingGate::Accepted)
            && !route.is_cross_transport_duplicate(&peer_npub, &msg.content)
        {
            handle_ride(
                store,
                tx,
                dedup,
                &peer_npub,
                &msg.event_id,
                msg.created_at,
                env,
            );
        }
        return;
    }

    // 5) Call signaling — only from an established conversation, so a stranger
    //    cannot ring you before their message request is accepted. Stale or
    //    already-dispatched signals are dropped: offers older than the ring
    //    timeout are meaningless, and a redelivered wrapper (relay
    //    at-least-once delivery, or the 2-day backfill re-scanning on every
    //    launch) must not re-ring or re-apply a signal already handled.
    if let Some(env) = parse_call_envelope(&msg.content) {
        if matches!(gate, IncomingGate::Accepted) {
            let now = now_secs();
            if call_signal_is_stale(msg.created_at, now) {
                // Warn, not debug: this is the one drop that looks to a user
                // exactly like "calls are broken" while chat keeps working, so
                // it has to be visible in a log someone actually captures.
                tracing::warn!(
                    event_id = %msg.event_id,
                    kind = env.signal.kind_str(),
                    created_at = msg.created_at,
                    now,
                    "dropping a call signal as stale — if this is a live call, \
                     the two devices' clocks disagree by more than the tolerance",
                );
            } else if dedup.already_seen(&msg.event_id) {
                tracing::debug!(event_id = %msg.event_id, "dropping duplicate call signal");
            } else {
                let _ = tx.send(BridgeEvent::IncomingCallSignal(CallSignalDto {
                    call_id: env.call_id,
                    peer: peer_npub,
                    media: env.media.as_str().to_string(),
                    signal: env.signal,
                    created_at: msg.created_at,
                }));
            }
        }
        return;
    }

    // 6) Watch/listen together. Gated exactly like a call signal — a stranger
    //    must not be able to move your playhead — and returning either way, so
    //    an ungated control message is dropped rather than surfacing as a
    //    message request full of JSON. Everything else about replay safety is
    //    inside `handle_together_envelope`.
    if let Some(env) = parse_together_envelope(&msg.content) {
        if matches!(gate, IncomingGate::Accepted) {
            if let Some(link) = route.together {
                handle_together_envelope(
                    tx,
                    link,
                    &peer_npub,
                    &msg.sender_pubkey,
                    msg.created_at,
                    Some(&msg.event_id),
                    env,
                );
            }
        }
        return;
    }

    // 6a) Handing a large attachment over. Gated identically to the together
    //     envelope above and to a call signal — both of those also end in a peer
    //     connection, and a stranger must not be able to make this device gather
    //     ICE candidates for them. Returning either way, so an ungated one is
    //     dropped rather than surfacing as a message request full of JSON.
    //
    //     No session to check against and no replay window to enforce here: the
    //     `transfer_id` is the scope, and the frontend that owns the transfer is
    //     the only thing that knows which ids are live. A signal for an id it
    //     never started is its to ignore — which is also why a *sender-only*
    //     signal arriving for a transfer this side started is checked there and
    //     not here.
    if let Some(env) = parse_handoff_envelope(&msg.content) {
        if matches!(gate, IncomingGate::Accepted) {
            let _ = tx.send(BridgeEvent::AttachmentHandoff(AttachmentHandoffDto {
                transfer_id: env.transfer_id,
                peer: peer_npub.clone(),
                signal: env.signal,
            }));
        }
        return;
    }

    // 7) Emoji reaction — a peer reacted to one of our messages (or to one of
    //    theirs). Gated like a beacon and a nudge: a stranger must not be able to
    //    decorate our messages before their request is accepted, and returning
    //    either way keeps a reaction from an unaccepted peer from surfacing as a
    //    message request full of JSON.
    //
    //    Not deduped by event id, and deliberately: the store's own timestamp
    //    check is the stronger guard (it refuses a replay even when the replay
    //    arrives under a *fresh* wrapper id, which the two-day backfill produces),
    //    and it also collapses a reaction that reached us over both transports.
    if let Some(env) = parse_reaction(&msg.content) {
        if matches!(gate, IncomingGate::Accepted) {
            if let Some(store) = store {
                let row = comrade_storage::MessageReaction {
                    target_id: env.target_id.clone(),
                    peer_npub: peer_npub.clone(),
                    reactor_npub: peer_npub.clone(),
                    emoji: env.emoji.clone(),
                    created_at: msg.created_at,
                    outgoing: false,
                };
                match store
                    .set_reaction(&row)
                    .and_then(|c| store.flush().map(|()| c))
                {
                    // Only news redraws anything — a replay or a repeat is not.
                    Ok(true) => {
                        let _ = tx.send(BridgeEvent::IncomingReaction(ReactionDto {
                            target_id: env.target_id,
                            peer: peer_npub,
                            reactor: row.reactor_npub,
                            emoji: env.emoji,
                            created_at: msg.created_at,
                            outgoing: false,
                        }));
                    }
                    Ok(false) => {}
                    Err(e) => warn!("failed to persist incoming reaction: {e}"),
                }
            }
        }
        return;
    }

    // 7b) A delete-for-everyone courtesy request — hide our own copy of the
    //     named message too. Gated like a reaction: a stranger must not be
    //     able to reach into our history before their request is accepted,
    //     and returning either way keeps an ungated one from surfacing as a
    //     message request full of JSON.
    //
    //     Not a NIP-09 retraction — see `comrade_core::dm::DeleteRequest`'s
    //     doc for why one is out of reach here. No `BridgeEvent` is emitted:
    //     unlike a reaction or a topic change, this has no dedicated push
    //     wired up (adding one is a `BridgeEvent` variant, which both
    //     `android/` and `app/` match exhaustively — see this crate's
    //     top-level doc). An already-open thread picks the hidden message up
    //     on its next `messages_with` read; that gap is a known follow-up,
    //     not an oversight.
    if let Some(env) = parse_delete_request(&msg.content) {
        if matches!(gate, IncomingGate::Accepted) {
            if let Some(store) = store {
                // **A request may only retract what its own sender wrote.**
                //
                // The envelope carries a bare `target_id` and nothing that
                // proves who authored the message it names, so honouring one
                // unchecked lets a peer hand us the id of a message *we* sent
                // and delete our own words out of our own transcript. They know
                // those ids — every message we sent them is one — so this is
                // not a theoretical reach: it is the cheapest possible way to
                // edit somebody else's record of a conversation, and it would
                // leave no trace, because a tombstone renders as absence.
                //
                // The outgoing side already refuses to retract a message the
                // user did not send, but that check runs on the honest client
                // and an attacker simply does not run it. This is the one that
                // has to hold. WhatsApp and Signal draw the line in the same
                // place, for the same reason.
                //
                // A row we have never cached is refused rather than tombstoned
                // pre-emptively: with nothing to compare against there is no
                // authorship to verify, and a tombstone written now would
                // silently hide whatever later arrives under that id.
                let authored_by_peer = match store.get_message(&env.target_id) {
                    Ok(Some(target)) => !target.outgoing && target.peer_npub == peer_npub,
                    Ok(None) => false,
                    Err(e) => {
                        warn!("could not check who wrote a delete-request target: {e}");
                        false
                    }
                };
                if authored_by_peer {
                    if let Err(e) = store.delete_message_for_me(&peer_npub, &env.target_id) {
                        warn!("failed to honour a delete-for-everyone request: {e}");
                    }
                } else {
                    warn!("refused a delete request for a message {peer_npub} did not write");
                }
            }
        }
        return;
    }

    // 8) A task: named, or moved to a new state. Gated exactly like a call
    //    signal — a stranger who can write to your task list has been handed a
    //    harassment channel, and in an app about wellbeing that is worse than
    //    the feature is good. Returns either way, so an ungated one is dropped
    //    rather than surfacing as a message request full of JSON.
    if let Some(env) = parse_karya_envelope(&msg.content) {
        if matches!(gate, IncomingGate::Accepted)
            // A control envelope reaches us twice when a message travels both
            // routes — sealed over the mesh now, over a relay when the internet
            // returns — and the two copies carry *different* event ids, so the
            // id check inside `deliver_synthetic_line` cannot pair them. This
            // can, because the envelope bytes are identical.
            && !route.is_cross_transport_duplicate(&peer_npub, &msg.content)
        {
            let me = vault.our_npub();
            if let Some(line) =
                apply_karya_signal(store, &peer_npub, &me, msg.created_at, &env.signal)
            {
                deliver_synthetic_line(vault, store, tx, route, &msg, &peer_npub, line);
            }
        }
        return;
    }

    // 8b) A topic signal: a name, a filing, or an archive. Gated exactly like a
    //     task — a stranger who can reorganise your conversation has been
    //     handed a way to hide messages from you, which is worse than the
    //     feature is good. Returns either way, so an ungated one is dropped
    //     rather than surfacing as a message request full of JSON.
    //
    //     No `deliver_synthetic_line` here, unlike a task or an offer: filing a
    //     thread emits no bubble on either side. `comrade_core::topic`'s module
    //     header has the argument — a conversation that grew a line every time
    //     somebody tidied would punish tidying. The cross-transport duplicate
    //     check a bubble needs is therefore not needed either: the store's own
    //     replay guards make a second copy a no-op.
    if let Some(env) = parse_topic_envelope(&msg.content) {
        if matches!(gate, IncomingGate::Accepted)
            && apply_topic_signal(store, &peer_npub, msg.created_at, &env.signal)
        {
            let _ = tx.send(BridgeEvent::TopicsChanged {
                peer: peer_npub.clone(),
            });
        }
        return;
    }

    // 9) An offer — "I thought this might help". Gated like a task, and the
    //    action must be one this build actually has a screen for: a bubble
    //    naming a destination that does not exist here would be a button that
    //    goes nowhere.
    if let Some(env) = parse_offer_envelope(&msg.content) {
        // Same two-route pairing as a task, and unlike a task an offer has no
        // id of its own to fall back on — every `/comrade-breathe` sends byte-
        // identical JSON — so this check is the only thing between one offer and
        // two bubbles.
        if matches!(gate, IncomingGate::Accepted)
            && !route.is_cross_transport_duplicate(&peer_npub, &msg.content)
        {
            match env.app_action() {
                Some(action) => deliver_synthetic_line(
                    vault,
                    store,
                    tx,
                    route,
                    &msg,
                    &peer_npub,
                    render_offer_line(action),
                ),
                None => tracing::debug!(action = %env.action, "offer names an unknown action"),
            }
        }
        return;
    }

    // 10) Media envelope — dedup by the NIP-94 event id (a redelivered wrapper
    //    carries the same reference), persist the ref, then surface (gated).
    if let Some(env) = parse_media_envelope(&msg.content) {
        // Everything in the envelope is chosen by the peer. The MIME type
        // decides which renderer every frontend reaches for and the caption is
        // drawn verbatim, so both are bounded and stripped of control
        // characters before they are persisted — once, here, rather than in
        // each of three UIs.
        let mime = match validate_mime_type(&env.mime) {
            Ok(mime) => mime,
            // Not a reason to drop the attachment: it downloads and opens fine,
            // it just gets the generic renderer instead of a claimed one.
            Err(e) => {
                tracing::debug!(error = %e, "incoming media has an unusable MIME type");
                DEFAULT_MIME.to_string()
            }
        };
        let caption = sanitise_untrusted_text(&env.caption, MAX_CAPTION_LEN);
        if let Some(store) = store {
            if store
                .get::<MediaRef>(MEDIA_REFS_TREE, &env.event_id)
                .ok()
                .flatten()
                .is_some()
            {
                return;
            }
            let reff = MediaRef {
                event_id: env.event_id.clone(),
                url: env.url.clone(),
                peer_pubkey: msg.sender_pubkey.clone(),
                mime_type: mime.clone(),
                caption: caption.clone(),
                size: env.size,
                sha256_hex: env.sha256_hex.clone(),
                outgoing: false,
                created_at: msg.created_at,
            };
            if let Err(e) = store
                .put(MEDIA_REFS_TREE, &env.event_id, &reff)
                .and_then(|()| store.flush())
            {
                warn!("failed to persist incoming media ref: {e}");
            }
        }
        if matches!(gate, IncomingGate::Accepted) {
            let _ = tx.send(BridgeEvent::IncomingMedia(MediaMessageDto {
                event_id: env.event_id,
                url: env.url,
                mime_type: mime,
                caption,
                sender: to_npub(&msg.sender_pubkey),
                created_at: msg.created_at,
                size: env.size,
                outgoing: false,
            }));
            send_delivered_receipt(vault, route.mesh, &msg.sender_pubkey, &msg.event_id);
        } else {
            ensure_pending(store, &peer_npub);
            let _ = tx.send(BridgeEvent::IncomingMessageRequest(MessageRequestDto {
                peer: peer_npub,
                last_message: attachment_preview(&caption),
                last_at: msg.created_at,
            }));
        }
        return;
    }

    // 11) Plain chat text — dedup by event id (a redelivered wrapper must not
    //    re-notify or re-send a delivered receipt), persist first, then
    //    deliver or gate into a request.
    // A message can reach us twice by two routes — sealed over the local mesh
    // now, over a relay when the internet returns — under two different ids, so
    // the id check below cannot catch that pair. This can.
    if route.is_cross_transport_duplicate(&peer_npub, &msg.content) {
        tracing::debug!(
            transport = route.label,
            "dropping a message already delivered by the other transport"
        );
        return;
    }
    if let Some(store) = store {
        if store.get_message(&msg.event_id).ok().flatten().is_some() {
            return;
        }
        let row = comrade_storage::StoredMessage {
            id: msg.event_id.clone(),
            peer_npub: peer_npub.clone(),
            content: msg.content.clone(),
            created_at: msg.created_at,
            outgoing: false,
            status: None,
            reply_to: msg.reply_to.clone(),
        };
        if let Err(e) = store.save_message(&row).and_then(|()| store.flush()) {
            warn!("failed to persist incoming DM: {e}");
        }
    }
    if matches!(gate, IncomingGate::Accepted) {
        send_delivered_receipt(vault, route.mesh, &msg.sender_pubkey, &msg.event_id);
        let _ = tx.send(BridgeEvent::IncomingDirectMessage(DirectMessageDto::from(
            msg,
        )));
    } else {
        ensure_pending(store, &peer_npub);
        let _ = tx.send(BridgeEvent::IncomingMessageRequest(MessageRequestDto {
            peer: peer_npub,
            last_message: msg.content,
            last_at: msg.created_at,
        }));
    }
}

/// Detached profile-refresh worker. Holds only the engine and store handles,
/// so the shared `Arc<RwLock<ComradeRuntime>>` guard can be dropped before
/// the slow network work starts (see [`ComradeRuntime::profile_refresher`]).
pub struct ProfileRefresher {
    sabha: Arc<SabhaEngine>,
    store: Arc<comrade_storage::EncryptedStore>,
}

impl ProfileRefresher {
    /// Refresh the cached Kind-0 profiles of everyone we talk to
    /// (conversation peers and saved contacts) in **one** relay round-trip,
    /// bounded by [`PROFILE_REFRESH_CAP`] and per-record freshness windows.
    /// Returns how many display names changed — the frontend reloads its
    /// chat list when > 0.
    pub async fn run(self) -> Result<usize, UiError> {
        let mut peers: Vec<String> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for msg in self
            .store
            .all_messages()
            .map_err(|e| UiError::Storage(e.to_string()))?
        {
            if seen.insert(msg.peer_npub.clone()) {
                peers.push(msg.peer_npub);
            }
        }
        for contact in self
            .store
            .list_contacts()
            .map_err(|e| UiError::Storage(e.to_string()))?
        {
            if seen.insert(contact.npub.clone()) {
                peers.push(contact.npub);
            }
        }

        // Select the stale records. A record that has a name is trusted for
        // the full TTL; a nameless record only briefly — an offline launch
        // yields Ok(no events) from the pool (indistinguishable from "peer
        // has no profile"), and that outcome must not freeze the peer as
        // key-only for a whole day.
        let now = now_secs();
        let mut stale: Vec<(String, PublicKey, Option<PeerProfileRecord>)> = Vec::new();
        for npub in peers {
            if stale.len() >= PROFILE_REFRESH_CAP {
                break;
            }
            let previous: Option<PeerProfileRecord> = self
                .store
                .get(PEER_PROFILES_TREE, &npub)
                .unwrap_or_default();
            let ttl = if previous.as_ref().is_some_and(|p| p.name.is_some()) {
                PROFILE_TTL_SECS
            } else {
                PROFILE_NEGATIVE_TTL_SECS
            };
            let fresh = previous
                .as_ref()
                .is_some_and(|r| now.saturating_sub(r.updated_at) < ttl);
            if fresh {
                continue;
            }
            let Ok(pk) = PublicKey::parse(&npub) else {
                continue;
            };
            stale.push((npub, pk, previous));
        }
        if stale.is_empty() {
            return Ok(0);
        }

        let authors: Vec<PublicKey> = stale.iter().map(|(_, pk, _)| *pk).collect();
        let found = match self.sabha.fetch_profiles(&authors).await {
            Ok(found) => found,
            Err(e) => {
                // Transport error: stamp nothing, so the next refresh retries.
                warn!("peer profile refresh failed: {e}");
                return Ok(0);
            }
        };

        let mut wrote = false;
        let mut changed = 0usize;
        for (npub, pk, previous) in stale {
            let meta = found.get(&pk);
            // A silent relay set must not erase what we already knew — which is
            // now the merge's default rather than an `.or_else` per field, so a
            // field added later inherits the behaviour instead of forgetting it.
            let learned = PeerProfilePatch {
                name: meta.and_then(display_name_of),
                about: meta.and_then(|m| m.about.clone()),
                picture: meta.and_then(|m| m.picture.clone()),
                nip05: meta.and_then(|m| m.nip05.clone()),
                lud16: meta.and_then(|m| m.lud16.clone()),
                ..Default::default()
            };
            let name_changed = learned
                .name
                .as_ref()
                .is_some_and(|n| Some(n) != previous.as_ref().and_then(|p| p.name.as_ref()));
            if merge_peer_profile(&self.store, &npub, learned) {
                wrote = true;
                if name_changed {
                    changed += 1;
                }
            }
        }
        if wrote {
            if let Err(e) = self.store.flush() {
                warn!("failed to flush profile cache: {e}");
            }
        }
        self.refresh_avatars().await;
        Ok(changed)
    }

    /// Second phase: fetch the pictures the first phase just discovered.
    ///
    /// Folded into the same sweep rather than given its own entry point, because
    /// this is the code that already learned the `picture` URLs, both frontends
    /// already call `refresh_peer_profiles`, and one cap is easier to reason about
    /// than two.
    ///
    /// Every skip below is a deliberate gate, not an optimisation:
    ///
    /// - the setting is off → nothing is fetched, at all, for anyone;
    /// - the peer is not accepted and not a saved contact → opening a stranger's
    ///   profile must never make this device call a host they chose;
    /// - a blocked peer → we do not fetch anything on their behalf;
    /// - the bytes are already cached and fresh → an avatar changes far less often
    ///   than a handle, hence [`AVATAR_TTL_SECS`];
    /// - a recent failure → [`AVATAR_NEGATIVE_TTL_SECS`] stops a dead URL being
    ///   retried on every single sweep.
    ///
    /// Errors are never propagated: a picture that will not load is a cosmetic
    /// problem, and failing the whole profile refresh over one would cost the
    /// handles too.
    async fn refresh_avatars(&self) {
        let enabled = self
            .store
            .get::<bool>(SETTINGS_TREE, REMOTE_AVATARS_KEY)
            .ok()
            .flatten()
            .unwrap_or(true);
        if !enabled {
            return;
        }
        let now = now_secs();
        let mut due: Vec<(String, String)> = Vec::new();
        let contacts: std::collections::HashSet<String> = self
            .store
            .list_contacts()
            .map(|cs| cs.into_iter().map(|c| c.npub).collect())
            .unwrap_or_default();
        for npub in contacts.iter() {
            if due.len() >= AVATAR_FETCH_CAP {
                break;
            }
            let Some(record) = cached_peer_profile(&self.store, npub) else {
                continue;
            };
            let Some(url) = record.picture.clone() else {
                continue;
            };
            // Blocked is checked here rather than filtered above, so the reason a
            // peer was skipped stays visible at the point of the decision.
            let blocked = self
                .store
                .get_conversation_meta(npub)
                .ok()
                .flatten()
                .is_some_and(|m| m.state == STATE_BLOCKED);
            if blocked {
                continue;
            }
            let cached_fresh = record.avatar_sha256.is_some()
                && now.saturating_sub(record.avatar_fetched_at) < AVATAR_TTL_SECS;
            if cached_fresh {
                continue;
            }
            if now.saturating_sub(record.avatar_failed_at) < AVATAR_NEGATIVE_TTL_SECS {
                continue;
            }
            due.push((npub.clone(), url));
        }
        if due.is_empty() {
            return;
        }
        let mut wrote = false;
        for (npub, url) in due {
            match comrade_core::media::fetch_avatar(&url).await {
                Ok((bytes, mime)) => {
                    let sha = comrade_core::crypto::sha256_hex(&bytes);
                    if let Err(e) = self.store.put_bytes(PEER_AVATAR_BLOBS_TREE, &sha, &bytes) {
                        warn!("failed to cache avatar bytes: {e}");
                        continue;
                    }
                    wrote |= merge_peer_profile(
                        &self.store,
                        &npub,
                        PeerProfilePatch {
                            avatar: Some((sha, mime)),
                            ..Default::default()
                        },
                    );
                }
                Err(e) => {
                    // Stamp the failure and keep whatever picture we already had.
                    tracing::debug!("avatar fetch failed for {npub}: {e}");
                    wrote |= merge_peer_profile(
                        &self.store,
                        &npub,
                        PeerProfilePatch {
                            avatar_failed: true,
                            ..Default::default()
                        },
                    );
                }
            }
        }
        if wrote {
            if let Err(e) = self.store.flush() {
                warn!("failed to flush avatar cache: {e}");
            }
        }
    }
}

/// An owned [`comrade_core::sabha::MetadataEdit`], for crossing a task boundary.
///
/// `MetadataEdit` borrows, which is right at the publish call and wrong for a
/// `tokio::spawn` that outlives the caller's stack frame.
#[derive(Debug, Clone, PartialEq, Eq)]
enum OwnedMetadataEdit {
    Leave,
    Set(String),
    Clear,
}

impl OwnedMetadataEdit {
    fn as_edit(&self) -> comrade_core::sabha::MetadataEdit<'_> {
        match self {
            OwnedMetadataEdit::Leave => comrade_core::sabha::MetadataEdit::Leave,
            OwnedMetadataEdit::Set(v) => comrade_core::sabha::MetadataEdit::Set(v),
            OwnedMetadataEdit::Clear => comrade_core::sabha::MetadataEdit::Clear,
        }
    }
}

/// Publish the Kind-0 profile with retries and exponential backoff.
///
/// Why this exists: at onboarding the relays are still dialling when the
/// handle is claimed, and a single fire-and-forget publish that fails leaves
/// the identity permanently undiscoverable — peers searching the handle find
/// nothing. `publish_profile` itself waits (bounded) for a connection; this
/// wrapper keeps trying across transient failures. It is also spawned on
/// every launch (Kind-0 is replaceable, so republishing is idempotent).
async fn publish_profile_with_retry(
    sabha: Arc<SabhaEngine>,
    name: String,
    about: OwnedMetadataEdit,
) {
    // Make sure dials were at least initiated, even if the feed loop that
    // normally calls connect() hasn't run yet. Idempotent.
    sabha.connect().await;
    let mut delay = std::time::Duration::from_secs(2);
    for attempt in 1..=PUBLISH_ATTEMPTS {
        let patch = comrade_core::sabha::ProfilePatch {
            name: &name,
            about: about.as_edit(),
            // A handle or bio publish never touches the picture: that has its own
            // path, and clobbering it here would drop an avatar set elsewhere.
            picture: comrade_core::sabha::MetadataEdit::Leave,
        };
        match sabha.publish_profile_patch(patch).await {
            Ok(_) => {
                tracing::info!(attempt, "profile handle published to relays");
                return;
            }
            Err(e) => warn!(attempt, "profile publish failed (will retry): {e}"),
        }
        tokio::time::sleep(delay).await;
        delay = delay.saturating_mul(2);
    }
    warn!("profile publish gave up after {PUBLISH_ATTEMPTS} attempts; will retry on next launch");
}

/// Normalise and validate a chosen @handle: strip a leading '@', lowercase,
/// then require 3–24 chars of `[a-z0-9_]`. One rule shared by every bridge.
fn normalize_handle(raw: &str) -> Result<String, UiError> {
    let handle = raw.trim().trim_start_matches('@').to_lowercase();
    if handle.len() < 3 || handle.len() > 24 {
        return Err(UiError::Engine("username must be 3–24 characters".into()));
    }
    if !handle
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    {
        return Err(UiError::Engine(
            "username may only contain a–z, 0–9 and _".into(),
        ));
    }
    // "primary" is the legacy no-username marker inside the store.
    if handle == "primary" {
        return Err(UiError::Engine("that username is reserved".into()));
    }
    Ok(handle)
}

// ── Travel (see `comrade_core::travel` and `docs/TRAVEL.md`) ─────────────────

/// One place on the Travel tab, ready to render.
///
/// Flattened to strings and options for the reason [`RideSignalDto`]'s doc
/// gives: a frontend keys its icon table on the wire names
/// ([`comrade_core::travel::PlaceKind::as_str`]), and a bridged enum would put a
/// Kotlin `when`, a Dart `switch` and a regenerated bridge behind every kind
/// added to the vocabulary.
///
/// `distance_m` is computed against the caller's **real** fix, not the blurred
/// coordinate the query went out with — see the module header of
/// `comrade_core::travel`. The screen stays accurate while the wire stays vague.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, uniffi::Record)]
pub struct TravelPlaceDto {
    pub id: String,
    pub name: String,
    /// `"restaurant"`, `"street_food"`, `"museum"`, … — [`PlaceKind::as_str`].
    pub kind: String,
    /// `"eat"` or `"do"`, so a frontend never re-derives the split.
    pub section: String,
    pub lat: f64,
    pub lon: f64,
    pub distance_m: u32,
    pub rating: Option<f64>,
    /// How many people rated it. The half of "legendary" that does the work —
    /// and a `None` here always accompanies a `None` rating, never a lone star
    /// average with an unknown crowd behind it.
    pub review_count: Option<u32>,
    /// Whether this earns the badge ([`comrade_core::travel::is_legendary`]).
    /// Decided in core so three frontends cannot disagree about what a legend is.
    pub legendary: bool,
    pub address: Option<String>,
    pub cuisine: Option<String>,
    pub note: Option<String>,
    /// `"google_maps"`, `"openstreetmap"`, `"wikipedia"` — a card that shows a
    /// number has to be able to say who said it.
    pub source: String,
    pub open_url: Option<String>,
}

impl TravelPlaceDto {
    fn from_place(place: &Place, origin: (f64, f64)) -> Self {
        Self {
            id: place.id.clone(),
            name: place.name.clone(),
            kind: place.kind.as_str().to_string(),
            section: place.kind.section().as_str().to_string(),
            lat: place.lat,
            lon: place.lon,
            distance_m: place
                .distance_m(origin)
                .round()
                .clamp(0.0, f64::from(u32::MAX)) as u32,
            rating: place.rating,
            review_count: place.review_count,
            legendary: place.is_legendary(),
            address: place.address.clone(),
            cuisine: place.cuisine.clone(),
            note: place.note.clone(),
            source: place.source.as_str().to_string(),
            open_url: place.open_url.clone(),
        }
    }
}

/// One thing worth knowing about where you are standing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, uniffi::Record)]
pub struct TravelFactDto {
    pub title: String,
    pub text: String,
    pub url: Option<String>,
    /// Present only when the article's subject has coordinates.
    pub distance_m: Option<u32>,
}

/// Everything the Travel tab draws.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, uniffi::Record)]
pub struct TravelGuideDto {
    pub area: Option<String>,
    pub eat: Vec<TravelPlaceDto>,
    pub things_to_do: Vec<TravelPlaceDto>,
    pub facts: Vec<TravelFactDto>,
    /// Which source the ratings came from, or `None` when nothing supplied any.
    /// **"No ratings configured" and "nothing legendary nearby" must not render
    /// the same**, and this field plus [`Self::notice`] is how a frontend tells
    /// them apart.
    pub ratings_from: Option<String>,
    /// A plain sentence about what did not work — a missing API key, a provider
    /// that refused — or `None` when everything answered. Carried rather than
    /// thrown, because an Overpass + Wikipedia guide is still worth showing when
    /// the ratings half is misconfigured, and a blank screen would explain
    /// nothing.
    pub notice: Option<String>,
    /// Unix seconds this guide was actually fetched.
    pub fetched_at: u64,
    /// True when this came from the session cache rather than the network.
    pub from_cache: bool,
    /// True when the cache was all there was — the network refused and this is
    /// an older answer being shown anyway. A frontend should say "last checked
    /// …" rather than pretending it is current.
    pub stale: bool,
}

impl TravelGuideDto {
    fn from_guide(guide: &TravelGuide, origin: (f64, f64), from_cache: bool, now: u64) -> Self {
        Self {
            area: guide.area.clone(),
            eat: guide
                .eat
                .iter()
                .map(|p| TravelPlaceDto::from_place(p, origin))
                .collect(),
            things_to_do: guide
                .things_to_do
                .iter()
                .map(|p| TravelPlaceDto::from_place(p, origin))
                .collect(),
            facts: guide
                .facts
                .iter()
                .map(|f| TravelFactDto {
                    title: f.title.clone(),
                    text: f.text.clone(),
                    url: f.url.clone(),
                    distance_m: f.coord.map(|c| {
                        travel::haversine_m(origin, c)
                            .round()
                            .clamp(0.0, f64::from(u32::MAX)) as u32
                    }),
                })
                .collect(),
            ratings_from: guide.ratings_from.map(|s| s.as_str().to_string()),
            notice: None,
            fetched_at: guide.fetched_at,
            from_cache,
            stale: guide.is_stale(now),
        }
    }
}

/// The session's travel guides, keyed by coarse cell.
///
/// A separate handle type rather than a bare `Arc<Mutex<GuideCache>>` field so
/// it can be cloned out of the runtime and used *after* the lock is released —
/// which is the entire reason [`travel_guide`] can be a free function and can
/// therefore never be the "lock held across an await" bug this repo has already
/// fixed twice.
#[derive(Debug, Clone, Default)]
pub struct TravelCache(Arc<Mutex<GuideCache>>);

impl TravelCache {
    /// The cached guide for `cell` if it is still fresh, or — when
    /// `allow_stale` — whatever is there regardless of age.
    fn get(&self, cell: &str, now: u64, allow_stale: bool) -> Option<TravelGuide> {
        let guard = self.0.lock().unwrap();
        if allow_stale {
            guard.stale(cell).cloned()
        } else {
            guard.fresh(cell, now).cloned()
        }
    }

    fn put(&self, guide: TravelGuide) {
        self.0.lock().unwrap().put(guide);
    }

    /// Forget every cell this session has visited.
    pub fn clear(&self) {
        self.0.lock().unwrap().clear();
    }
}

impl ComradeRuntime {
    /// A handle to this session's travel cache, for [`travel_guide`].
    ///
    /// Cloned out under a short read lock so the network fetch that follows
    /// holds no runtime lock at all — see [`TravelCache`].
    pub fn travel_cache(&self) -> TravelCache {
        self.travel.clone()
    }

    /// The user's own Google Places API key, when one has been saved and the
    /// vault is open.
    ///
    /// `None` when the vault is locked rather than an error: the free half of
    /// the Travel tab (OpenStreetMap, Wikipedia) works without a vault, and a
    /// guide with no ratings and a notice saying so is a better answer than a
    /// thrown exception on a screen somebody opened to find lunch.
    pub fn travel_api_key(&self) -> Option<String> {
        self.ui
            .store_ref()?
            .get::<String>(SETTINGS_TREE, TRAVEL_PLACES_KEY)
            .ok()
            .flatten()
            .filter(|k| !k.trim().is_empty())
    }

    /// Save (or, with a blank `key`, clear) the Google Places API key.
    ///
    /// Validated through [`travel::ApiKey::parse`] before it is written, so a
    /// pasted blank fails here rather than at the far end as an opaque 403 —
    /// and the value is never logged, on the way in or out.
    pub fn set_travel_api_key(&self, key: &str) -> Result<(), UiError> {
        let store = self.ui.store_ref().ok_or(UiError::VaultLocked)?;
        if key.trim().is_empty() {
            store
                .delete(SETTINGS_TREE, TRAVEL_PLACES_KEY)
                .map_err(|e| UiError::Storage(e.to_string()))?;
        } else {
            let parsed = travel::ApiKey::parse(key).map_err(|e| UiError::Travel(e.to_string()))?;
            store
                .put(
                    SETTINGS_TREE,
                    TRAVEL_PLACES_KEY,
                    &parsed.header_value().to_string(),
                )
                .map_err(|e| UiError::Storage(e.to_string()))?;
        }
        // Ratings for a cell already in the cache were fetched under the old
        // configuration; leaving them would make "I just added my key" look
        // like it did nothing.
        self.travel.clear();
        store.flush().map_err(|e| UiError::Storage(e.to_string()))
    }

    /// Whether a ratings provider is configured, for a settings screen. The key
    /// itself is never returned — only whether there is one.
    pub fn travel_ratings_configured(&self) -> bool {
        self.travel_api_key().is_some()
    }
}

/// The sentence a guide carries when no ratings provider is configured.
///
/// Spelled out once, here, because it is the single most likely state of this
/// feature on a fresh install and every frontend has to say the same thing
/// about it.
pub const TRAVEL_NO_KEY_NOTICE: &str =
    "Restaurant ratings need your own Google Places API key — add one in Settings → Travel. \
     Places and facts below come from OpenStreetMap and Wikipedia.";

/// Build the Travel guide for a coordinate: legendary places to eat, things to
/// do, and what this place is.
///
/// **A free function, and that is the point.** It reads nothing from
/// [`ComradeRuntime`] — the caller clones a [`TravelCache`] handle and reads the
/// API key under a short lock, then calls this with no lock held. Three network
/// round trips inside a held `RwLock` is the shape of the two deadlocks this
/// repo has already fixed.
///
/// What leaves the device: a coordinate rounded to a ~150 m geohash cell
/// ([`travel::coarse_origin`]), a radius, and nothing else. No npub, no contact,
/// no indication of who is asking. The Google request additionally carries the
/// user's own API key, in a header rather than the query string, so it does not
/// end up in every proxy log between here and Mountain View.
///
/// Partial failure is a notice, not an exception: a misconfigured or refused
/// ratings provider still leaves an OpenStreetMap + Wikipedia guide worth
/// showing, and [`TravelGuideDto::notice`] says what went wrong. A total
/// failure falls back to a stale cached guide for the same cell if there is
/// one — a guide from this morning with a "last checked" line beats a blank
/// screen on a street — and only errors when there is nothing at all.
///
/// `refresh` skips the fresh-cache check; the cache itself is still written.
pub async fn travel_guide(
    cache: &TravelCache,
    api_key: Option<String>,
    lat: f64,
    lon: f64,
    radius_m: u32,
    refresh: bool,
) -> Result<TravelGuideDto, UiError> {
    let origin = (lat, lon);
    let now = now_secs();
    // Both sections are asked for around the same blurred origin, so one cell
    // key covers the whole guide.
    let eat_query = TravelQuery::around(lat, lon, radius_m, travel::Section::Eat);
    let cell = eat_query.cell.clone();

    if !refresh {
        if let Some(hit) = cache.get(&cell, now, false) {
            return Ok(TravelGuideDto::from_guide(&hit, origin, true, now));
        }
    }

    match fetch_travel_guide(&eat_query, api_key, origin, now).await {
        Ok((guide, notice)) => {
            cache.put(guide.clone());
            let mut dto = TravelGuideDto::from_guide(&guide, origin, false, now);
            dto.notice = notice;
            Ok(dto)
        }
        Err(err) => match cache.get(&cell, now, true) {
            Some(old) => {
                let mut dto = TravelGuideDto::from_guide(&old, origin, true, now);
                dto.stale = true;
                dto.notice = Some(format!("Showing the last guide for here — {err}"));
                Ok(dto)
            }
            None => Err(err),
        },
    }
}

/// The network half of [`travel_guide`]. Returns the guide and the notice (if
/// any) about what did not answer.
#[cfg(feature = "travel-http")]
async fn fetch_travel_guide(
    eat_query: &TravelQuery,
    api_key: Option<String>,
    origin: (f64, f64),
    now: u64,
) -> Result<(TravelGuide, Option<String>), UiError> {
    use comrade_core::travel::{GooglePlaces, Overpass, PlaceProvider, WikipediaNearby};

    // Overpass and Wikipedia both ask that clients identify themselves, and a
    // generic agent is what gets a project rate-limited. No version: the string
    // would then change with every release for no benefit to them.
    const AGENT: &str = "comrade/1.0 (https://github.com/cmullu/comrade)";

    let do_query = TravelQuery {
        section: travel::Section::Do,
        ..eat_query.clone()
    };
    let osm = Overpass::new(AGENT);
    let wiki = WikipediaNearby::new(AGENT);

    // Concurrent, not sequential: these are three independent hosts and the
    // user is standing on a street. The `join!` is over futures that hold no
    // lock, so there is nothing here for them to contend on.
    let (osm_eat, osm_do, facts) = tokio::join!(
        osm.nearby(eat_query),
        osm.nearby(&do_query),
        wiki.facts(eat_query),
    );

    let mut notices: Vec<String> = Vec::new();
    let mut places: Vec<Place> = Vec::new();
    for (label, result) in [("places", osm_eat), ("attractions", osm_do)] {
        match result {
            Ok(found) => places.extend(found),
            Err(e) => notices.push(format!("OpenStreetMap {label} unavailable ({e})")),
        }
    }
    let facts = match facts {
        Ok(found) => travel::rank_facts(found, origin),
        Err(e) => {
            notices.push(format!("Wikipedia unavailable ({e})"));
            Vec::new()
        }
    };

    // The ratings half. Its absence is the expected state of a fresh install,
    // so it is a notice rather than a failure — but a *loud* one, because a
    // Travel tab with no review counts is not the feature that was asked for.
    match api_key.as_deref().map(travel::ApiKey::parse) {
        None | Some(Err(_)) => notices.push(TRAVEL_NO_KEY_NOTICE.to_string()),
        Some(Ok(key)) => {
            let google = GooglePlaces::new(key);
            let (rated_eat, rated_do) =
                tokio::join!(google.nearby(eat_query), google.nearby(&do_query));
            let mut rated: Vec<Place> = Vec::new();
            let mut google_failed = false;
            for result in [rated_eat, rated_do] {
                match result {
                    Ok(found) => rated.extend(found),
                    Err(e) => {
                        if !google_failed {
                            notices.push(format!("Google Maps ratings unavailable ({e})"));
                            google_failed = true;
                        }
                    }
                }
            }
            // OSM first so its cuisine tags and canonical object links survive
            // the merge; the rating always comes from whichever record has one.
            places = travel::merge_places(places, rated);
        }
    }

    if places.is_empty() && facts.is_empty() {
        return Err(UiError::Travel(if notices.is_empty() {
            "nothing found near here".to_string()
        } else {
            notices.join("; ")
        }));
    }

    let guide = travel::build_guide(origin, &eat_query.cell, places, facts, None, now);
    Ok((guide, (!notices.is_empty()).then(|| notices.join(" "))))
}

/// Without `travel-http` there is no socket to reach a provider through.
///
/// A distinct error rather than an empty guide, for the reason
/// [`UiError::CatalogueUnavailable`] exists: "this build cannot look places up"
/// and "there is nothing near you" must not render the same.
#[cfg(not(feature = "travel-http"))]
async fn fetch_travel_guide(
    _eat_query: &TravelQuery,
    _api_key: Option<String>,
    _origin: (f64, f64),
    _now: u64,
) -> Result<(TravelGuide, Option<String>), UiError> {
    Err(UiError::TravelUnavailable)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use comrade_core::nudge::NUDGE_SETTLE_SECS;
    use tempfile::TempDir;

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn runtime_is_send_sync_for_shared_state() {
        // The Tauri managed state and JNI global both require this bound; a
        // regression here is exactly the Send/Sync compile boundary M5 guards.
        assert_send_sync::<ComradeRuntime>();
        assert_send_sync::<std::sync::Arc<tokio::sync::RwLock<ComradeRuntime>>>();
        assert_send_sync::<BridgeEvent>();
    }

    #[test]
    fn bridge_futures_are_send() {
        // Tauri's #[tauri::command] requires every command future to be Send;
        // the workspace itself never demands that, so without this
        // compile-time probe a non-Send future (e.g. a borrowed iterator held
        // across an await deep inside an engine) only surfaces in the desktop
        // CI lane — which is exactly how the search_profiles regression
        // escaped local checks once.
        fn require_send<T: Send>(_t: T) {}
        #[allow(dead_code)]
        fn probe(rt: &ComradeRuntime, wrt: &mut ComradeRuntime, urt: &mut ComradeRuntime) {
            require_send(rt.search_profiles("q"));
            require_send(rt.refresh_peer_profiles());
            require_send(rt.send_dm("npub1x", "hi"));
            require_send(rt.broadcast_chitthi("x", None));
            require_send(rt.sync_ledger());
            require_send(rt.upload_and_send_media("x", vec![], "image/png", ""));
            require_send(rt.download_and_decrypt_media("x"));
            require_send(rt.sakha_add_entry("desc", 1.0, "sakha"));
            require_send(rt.sakha_read_ledger());
            require_send(wrt.set_username("neo"));
            require_send(wrt.pair_sakha("npub1x", "sakha"));
            require_send(urt.unlock_vault("/tmp/x", "p"));
            require_send(wrt.toggle_workspace("Base"));
            require_send(urt.back());
        }
        let _ = probe;
    }

    #[tokio::test]
    async fn toggle_workspace_enforces_state_machine() {
        let mut rt = ComradeRuntime::new();
        let dto = rt.toggle_workspace("OffGridTravel").await.unwrap();
        assert_eq!(dto.key, "OffGridTravel");
        assert!(dto.mesh_active);
        // OffGridTravel -> CoupleSandbox is blocked by the transition graph.
        assert!(matches!(
            rt.toggle_workspace("CoupleSandboxSakha").await,
            Err(UiError::Transition(_))
        ));
        // Unknown keys are a distinct typed error.
        assert!(matches!(
            rt.toggle_workspace("Nope").await,
            Err(UiError::UnknownWorkspace(_))
        ));
    }

    #[tokio::test]
    async fn toggle_workspace_starts_and_stops_the_mesh_engine() {
        let mut rt = ComradeRuntime::new();
        assert_eq!(
            rt.mesh_status(),
            MeshStatusDto {
                active: false,
                peer_count: 0
            }
        );

        rt.toggle_workspace("OffGridTravel").await.unwrap();
        assert_eq!(
            rt.mesh_status(),
            MeshStatusDto {
                active: true,
                peer_count: 0
            }
        );

        rt.toggle_workspace("Base").await.unwrap();
        assert_eq!(
            rt.mesh_status(),
            MeshStatusDto {
                active: false,
                peer_count: 0
            }
        );
    }

    #[tokio::test]
    async fn back_also_stops_the_mesh_engine() {
        let mut rt = ComradeRuntime::new();
        rt.toggle_workspace("OffGridTravel").await.unwrap();
        assert!(rt.mesh_status().active);

        let dto = rt.back().await;
        assert_eq!(dto.key, "Base");
        assert!(!rt.mesh_status().active);
    }

    #[test]
    fn commands_reject_gracefully_when_vault_locked() {
        let rt = ComradeRuntime::new();
        assert!(!rt.is_vault_unlocked());
        assert!(matches!(
            rt.fetch_sabha_timeline(),
            Err(UiError::VaultLocked)
        ));
    }

    #[tokio::test]
    async fn broadcast_rejects_when_locked_without_panicking() {
        let rt = ComradeRuntime::new();
        let err = rt.broadcast_chitthi("hello sabha", None).await;
        assert!(matches!(err, Err(UiError::VaultLocked)));
        let err = rt.sync_ledger().await;
        assert!(matches!(err, Err(UiError::VaultLocked)));
    }

    #[tokio::test]
    async fn unlock_vault_seeds_identity_and_builds_engines() {
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        let id = rt.unlock_vault(dir.path(), "passphrase").await.unwrap();
        assert!(id.npub.starts_with("npub1"));
        assert!(rt.is_vault_unlocked());
        assert!(rt.is_store_unlocked());
        // Timeline is reachable (empty cache) once unlocked.
        assert!(rt.fetch_sabha_timeline().unwrap().is_empty());
    }

    #[tokio::test]
    async fn feed_filter_spec_is_bounded_global_with_no_contacts_then_authors_scoped_once_one_exists(
    ) {
        // AUDIT.md COMMS-04: the feed subscription must never be the
        // unbounded relay-wide firehose — a fresh identity gets an explicit
        // capped bootstrap scope, and adding a contact narrows it to authors.
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();

        let bootstrap = rt.feed_filter_spec();
        assert_eq!(
            bootstrap.scope,
            FeedScope::BoundedGlobal {
                limit: FEED_BOOTSTRAP_LIMIT
            },
            "no contacts yet must never fall back to an unbounded author-less firehose"
        );
        assert_eq!(bootstrap.since_secs, FEED_SINCE_SECS);

        let (_hex, peer) = stranger();
        rt.add_contact(&peer, "friend").unwrap();
        let scoped = rt.feed_filter_spec();
        match scoped.scope {
            FeedScope::Authors(authors) => {
                // Self + the one contact just added.
                assert_eq!(authors.len(), 2);
            }
            other => panic!("expected an authors-scoped feed once a contact exists, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn second_unlock_is_idempotent_and_keeps_the_same_identity() {
        // A repeated unlock must return the existing identity without rebuilding
        // engines (which would orphan the running ones and duplicate loops).
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        let first = rt.unlock_vault(dir.path(), "pin").await.unwrap().npub;
        let sabha_ptr = Arc::as_ptr(rt.sabha.as_ref().unwrap()) as usize;
        // Second unlock — same or different args — is a no-op that returns the
        // current identity and leaves the engine instances untouched.
        let second = rt.unlock_vault(dir.path(), "pin").await.unwrap().npub;
        assert_eq!(first, second);
        assert_eq!(
            sabha_ptr,
            Arc::as_ptr(rt.sabha.as_ref().unwrap()) as usize,
            "engines must not be rebuilt on a repeated unlock"
        );
    }

    #[tokio::test]
    async fn unlock_then_reopen_restores_same_identity() {
        let dir = TempDir::new().unwrap();
        let first = {
            let mut rt = ComradeRuntime::new();
            rt.unlock_vault(dir.path(), "pin").await.unwrap().npub
        };
        let mut rt2 = ComradeRuntime::new();
        let second = rt2.unlock_vault(dir.path(), "pin").await.unwrap().npub;
        assert_eq!(first, second);
    }

    #[tokio::test]
    async fn lock_vault_drops_engines_and_store_and_relock_works() {
        // AUDIT.md COMMS-01: locking must actually remove decrypted state, not
        // just flip a UI flag — every store/engine-backed call must reject
        // exactly as it did before the first unlock.
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        let identity = rt.unlock_vault(dir.path(), "pin").await.unwrap();
        assert!(rt.is_vault_unlocked());
        assert!(rt.is_store_unlocked());

        rt.lock_vault().await;
        assert!(!rt.is_vault_unlocked());
        assert!(!rt.is_store_unlocked());
        assert!(matches!(
            rt.fetch_sabha_timeline(),
            Err(UiError::VaultLocked)
        ));
        assert!(matches!(rt.profile(), Err(UiError::NoIdentity)));

        // Locking twice in a row must not panic (idempotent).
        rt.lock_vault().await;
        assert!(!rt.is_vault_unlocked());

        // Unlocking again resumes normally with the same on-disk identity, and
        // the feed/DM loops can be (re)spawned — the `loops_spawned` guard
        // must have been reset by the lock, not left permanently tripped.
        let relocked = rt.unlock_vault(dir.path(), "pin").await.unwrap();
        assert_eq!(relocked.npub, identity.npub);
        assert!(rt.is_vault_unlocked());
        rt.spawn_event_loops();
    }

    #[tokio::test]
    async fn sakha_status_and_ledger_reject_gracefully_before_pairing() {
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();

        let status = rt.sakha_status().unwrap();
        assert!(!status.paired);
        assert_eq!(status.partner_npub, None);

        assert!(matches!(
            rt.sakha_add_entry("Coffee", 150.0, "Sakha").await,
            Err(UiError::Engine(_))
        ));
        // Reading the (empty) local ledger doesn't require pairing.
        assert_eq!(rt.sakha_read_ledger().await.unwrap(), "");
    }

    #[tokio::test]
    async fn pair_sakha_rejects_an_invalid_partner_key() {
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();
        assert!(rt.pair_sakha("not-a-valid-key", "sakha").await.is_err());
        assert!(!rt.sakha_status().unwrap().paired);
    }

    #[tokio::test]
    async fn pair_sakha_add_entry_and_status_roundtrip() {
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();

        let partner = comrade_core::crypto::KeyProfile::generate().unwrap();
        let status = rt.pair_sakha(&partner.npub, "Sakhi").await.unwrap();
        assert!(status.paired);
        assert_eq!(status.partner_npub.as_deref(), Some(partner.npub.as_str()));
        assert_eq!(status.role.as_deref(), Some("sakhi"));

        let ledger = rt
            .sakha_add_entry("Groceries", 300.0, "Sakhi")
            .await
            .unwrap();
        assert!(ledger.contains("Groceries"), "entry must appear: {ledger}");
        assert_eq!(rt.sakha_read_ledger().await.unwrap(), ledger);
    }

    #[test]
    fn sakha_pairing_and_ledger_survive_a_relaunch() {
        // Regression guard for AUDIT A3/A8: pairing state and the local
        // ledger must not evaporate on restart just because the in-memory
        // Yrs doc and the paired-partner key live nowhere but RAM otherwise.
        //
        // This uses two independent Tokio runtimes (rather than one shared
        // `#[tokio::test]` runtime) to actually simulate a process restart:
        // `pair_sakha` spawns a detached background sync task that holds its
        // own `Arc` clone of the encrypted store, so within a single runtime
        // that task outlives the `{ }` scope below and keeps the redb file
        // open — dropping the whole `Runtime` (unlike a scope exit) forcibly
        // tears down every task it owns, exactly as a real process exit
        // would, and only then is the file lock actually released.
        let dir = TempDir::new().unwrap();
        let partner = comrade_core::crypto::KeyProfile::generate().unwrap();

        {
            let rt_tokio = tokio::runtime::Runtime::new().unwrap();
            rt_tokio.block_on(async {
                let mut rt = ComradeRuntime::new();
                rt.unlock_vault(dir.path(), "pin").await.unwrap();
                rt.pair_sakha(&partner.npub, "sakha").await.unwrap();
                rt.sakha_add_entry("Rent", 12000.0, "Sakha").await.unwrap();
            });
        }

        let rt_tokio2 = tokio::runtime::Runtime::new().unwrap();
        rt_tokio2.block_on(async {
            let mut rt2 = ComradeRuntime::new();
            rt2.unlock_vault(dir.path(), "pin").await.unwrap();
            let status = rt2.sakha_status().unwrap();
            assert!(status.paired, "pairing must survive a relaunch");
            assert_eq!(status.partner_npub.as_deref(), Some(partner.npub.as_str()));
            let ledger = rt2.sakha_read_ledger().await.unwrap();
            assert!(
                ledger.contains("Rent"),
                "ledger snapshot must survive a relaunch: {ledger}"
            );
        });
    }

    #[tokio::test]
    async fn fetch_timeline_reads_from_encrypted_cache() {
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();

        // Seed the encrypted cache directly (the relay loop does this in prod).
        rt.ui
            .store_ref()
            .unwrap()
            .cache_chitthi(&comrade_storage::Chitthi {
                id: "abc123".into(),
                author_npub: "npub1author".into(),
                content: "Namaste".into(),
                created_at: 42,
                reply_to: None,
            })
            .unwrap();

        let feed = rt.fetch_sabha_timeline().unwrap();
        assert_eq!(feed.len(), 1);
        assert_eq!(feed[0].id, "abc123");
        assert_eq!(feed[0].content, "Namaste");
    }

    #[tokio::test]
    async fn event_bus_delivers_serialisable_events() {
        let rt = ComradeRuntime::new();
        let mut rx = rt.subscribe_events();

        let event = BridgeEvent::IncomingChitthi(ChitthiDto {
            id: "id1".into(),
            author: "npub1x".into(),
            content: "over the wire".into(),
            created_at: 7,
            reply_to: None,
        });
        rt.event_sender().send(event.clone()).unwrap();

        let received = rx.recv().await.unwrap();
        assert_eq!(received, event);

        // It must round-trip through serde_json (the IPC payload format).
        let json = serde_json::to_string(&received).unwrap();
        assert!(json.contains("\"type\":\"incoming_chitthi\""));
        let back: BridgeEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(back, event);
    }

    #[tokio::test]
    async fn feed_events_and_critical_events_are_independent_channels() {
        // AUDIT.md COMMS-04: pins the channel split itself (not just the
        // production wiring in `spawn_event_loops`) — a subscriber to one
        // channel must never see what was sent on the other, in either
        // direction, and each channel must accept a send with no subscriber
        // to the *other* one blocking or erroring.
        let rt = ComradeRuntime::new();
        let mut critical_rx = rt.subscribe_events();
        let mut feed_rx = rt.subscribe_feed_events();

        let chitthi = BridgeEvent::IncomingChitthi(ChitthiDto {
            id: "id1".into(),
            author: "npub1x".into(),
            content: "hi".into(),
            created_at: 1,
            reply_to: None,
        });
        rt.feed_events.send(chitthi.clone()).unwrap();
        assert_eq!(feed_rx.recv().await.unwrap(), chitthi);
        assert!(
            critical_rx.try_recv().is_err(),
            "IncomingChitthi sent on the feed bus must never reach a critical-bus subscriber"
        );

        let mesh = BridgeEvent::MeshStatusChanged(MeshStatusDto {
            active: true,
            peer_count: 1,
        });
        rt.events.send(mesh.clone()).unwrap();
        assert_eq!(critical_rx.recv().await.unwrap(), mesh);
        assert!(
            feed_rx.try_recv().is_err(),
            "a critical event must never reach a feed-bus subscriber"
        );
    }

    #[test]
    fn media_envelope_detection() {
        let env = MediaEnvelope {
            comrade_media: 1,
            event_id: "e1".into(),
            url: "https://blob/x".into(),
            mime: "image/png".into(),
            caption: "hi".into(),
            size: 10,
            sha256_hex: "a".repeat(64),
        };
        let json = serde_json::to_string(&env).unwrap();
        assert!(parse_media_envelope(&json).is_some());
        // A plain DM is not mistaken for a media envelope.
        assert!(parse_media_envelope("just a normal message").is_none());
        assert!(parse_media_envelope(r#"{"hello":"world"}"#).is_none());
        // An envelope written before the sha256_hex field still parses (the
        // field is #[serde(default)]) — back-compat for already-sent media.
        assert!(parse_media_envelope(
            r#"{"comrade_media":1,"event_id":"e","url":"https://b/x","mime":"image/png","caption":"","size":1}"#
        )
        .is_some());
        // The flush loop routes on this: a queued envelope must never be
        // re-keyed into a stored text message (that is how raw JSON ends up in
        // a chat bubble).
        assert!(is_media_envelope(&json));
        assert!(!is_media_envelope("just a normal message"));
    }

    #[test]
    fn mime_types_are_normalised_and_hostile_ones_refused() {
        // Case-insensitive per RFC 2045: without lowercasing, `IMAGE/PNG` fails
        // every frontend's `starts_with("image/")` and a photo renders as an
        // unopenable file.
        assert_eq!(validate_mime_type(" IMAGE/PNG ").unwrap(), "image/png");
        assert_eq!(
            validate_mime_type("application/vnd.oasis.opendocument.text").unwrap(),
            "application/vnd.oasis.opendocument.text"
        );
        // Blank, header-shaped, structureless, or oversized: refused, not patched.
        for bad in [
            "",
            "   ",
            "image/png\r\nX-Evil: 1",
            "image png",
            "notamimetype",
        ] {
            assert!(
                validate_mime_type(bad).is_err(),
                "must reject {bad:?} as a MIME type"
            );
        }
        assert!(validate_mime_type(&format!("image/{}", "x".repeat(MAX_MIME_LEN))).is_err());
    }

    #[test]
    fn captions_are_stripped_of_control_characters_and_bounded() {
        // A peer's caption is drawn verbatim in every frontend. Newlines would
        // let them forge extra UI lines; an unbounded one is a storage and
        // layout problem.
        assert_eq!(
            sanitise_untrusted_text("holiday\r\nphoto\u{0}", MAX_CAPTION_LEN),
            "holidayphoto"
        );
        assert_eq!(sanitise_untrusted_text("  spaced  ", 64), "spaced");
        let long = "é".repeat(MAX_CAPTION_LEN * 2);
        let cut = sanitise_untrusted_text(&long, MAX_CAPTION_LEN);
        // Truncation counts characters, not bytes — a multi-byte character is
        // never split in half.
        assert_eq!(cut.chars().count(), MAX_CAPTION_LEN);
        assert!(cut.chars().all(|c| c == 'é'));
    }

    #[test]
    fn attachment_previews_never_render_an_empty_line() {
        assert_eq!(attachment_preview("sunset.jpg"), "📎 sunset.jpg");
        assert_eq!(attachment_preview("   "), "📎 Attachment");
        assert_eq!(attachment_preview(""), "📎 Attachment");
    }

    #[test]
    fn to_npub_canonicalises_incoming_and_outgoing_to_the_same_key() {
        // Regression guard: incoming media/DM senders arrive as hex, outgoing
        // DTOs emit bech32. Both must normalise to the identical npub so the
        // frontend keys one conversation (and the couple panel) per peer.
        let keys = nostr_sdk::prelude::Keys::generate();
        let hex = keys.public_key().to_hex();
        let npub = keys.public_key().to_bech32().unwrap();
        assert_eq!(to_npub(&hex), npub, "hex must normalise to npub");
        assert_eq!(to_npub(&npub), npub, "npub is already canonical");
        // Unparseable input falls back unchanged rather than panicking.
        assert_eq!(to_npub("not-a-key"), "not-a-key");
    }

    #[test]
    fn incoming_media_event_serialises_with_tag() {
        let ev = BridgeEvent::IncomingMedia(MediaMessageDto {
            event_id: "e".into(),
            url: "https://blob/x".into(),
            mime_type: "image/jpeg".into(),
            caption: "pic".into(),
            sender: "npub1x".into(),
            created_at: 1,
            size: 42,
            outgoing: false,
        });
        let json = serde_json::to_string(&ev).unwrap();
        assert!(json.contains("\"type\":\"incoming_media\""));
        let back: BridgeEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(back, ev);
    }

    #[test]
    fn handle_normalisation_rules() {
        assert_eq!(normalize_handle("@Abc_User").unwrap(), "abc_user");
        assert_eq!(normalize_handle("  neo42 ").unwrap(), "neo42");
        assert!(normalize_handle("ab").is_err(), "too short");
        assert!(normalize_handle(&"x".repeat(25)).is_err(), "too long");
        assert!(normalize_handle("has space").is_err());
        assert!(normalize_handle("emoji🙂").is_err());
        assert!(normalize_handle("primary").is_err(), "reserved");
    }

    #[tokio::test]
    async fn username_round_trips_through_the_encrypted_store() {
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();
        assert_eq!(rt.profile().unwrap().username, None);

        let profile = rt.set_username("@Chandra_M").await.unwrap();
        assert_eq!(profile.username.as_deref(), Some("chandra_m"));

        // Reopen: the handle must survive alongside the identity.
        drop(rt);
        let mut rt2 = ComradeRuntime::new();
        rt2.unlock_vault(dir.path(), "pin").await.unwrap();
        assert_eq!(
            rt2.profile().unwrap().username.as_deref(),
            Some("chandra_m")
        );
    }

    #[tokio::test]
    async fn contacts_are_pinned_by_npub_not_by_alias() {
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();

        let a = nostr_sdk::prelude::Keys::generate()
            .public_key()
            .to_bech32()
            .unwrap();
        let b = nostr_sdk::prelude::Keys::generate()
            .public_key()
            .to_bech32()
            .unwrap();

        // Two different keys may claim the same alias — both entries survive,
        // because the store is keyed by npub (TOFU), not by the display name.
        rt.add_contact(&a, "abc_user").unwrap();
        rt.add_contact(&b, "abc_user").unwrap();
        let contacts = rt.list_contacts().unwrap();
        assert_eq!(contacts.len(), 2);
        assert!(contacts.iter().any(|c| c.npub == a));
        assert!(contacts.iter().any(|c| c.npub == b));

        // Re-adding the same npub renames in place instead of duplicating.
        rt.add_contact(&a, "renamed").unwrap();
        let contacts = rt.list_contacts().unwrap();
        assert_eq!(contacts.len(), 2);
        assert_eq!(
            contacts.iter().find(|c| c.npub == a).unwrap().alias,
            "renamed"
        );

        // An empty alias on re-add (opening an existing chat) must never wipe
        // the alias the user chose.
        rt.add_contact(&a, "  ").unwrap();
        assert_eq!(
            rt.list_contacts()
                .unwrap()
                .iter()
                .find(|c| c.npub == a)
                .unwrap()
                .alias,
            "renamed"
        );

        // Junk npubs are a typed error, not a stored contact.
        assert!(rt.add_contact("not-a-key", "x").is_err());
    }

    #[tokio::test]
    async fn contact_alias_lifecycle_set_clear_remove() {
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();
        let peer = nostr_sdk::prelude::Keys::generate()
            .public_key()
            .to_bech32()
            .unwrap();

        // Adding by key alone stores no alias (no fake npub-prefix names).
        let added = rt.add_contact(&peer, "").unwrap();
        assert_eq!(added.alias, "");

        // A conversation exists with this peer, so the chat list reflects the
        // alias lifecycle end to end.
        rt.ui
            .store_ref()
            .unwrap()
            .save_message(&comrade_storage::StoredMessage {
                id: "m1".into(),
                peer_npub: peer.clone(),
                content: "hello".into(),
                created_at: 1,
                outgoing: true,
                status: None,
                reply_to: None,
            })
            .unwrap();

        // The alias feature: set…
        let set = rt.set_contact_alias(&peer, "Charlie ❤").unwrap();
        assert_eq!(set.alias, "Charlie ❤");
        assert_eq!(
            rt.conversations().unwrap()[0].alias.as_deref(),
            Some("Charlie ❤")
        );

        // …and clear (empty = explicit clear, unlike add_contact).
        let cleared = rt.set_contact_alias(&peer, "").unwrap();
        assert_eq!(cleared.alias, "");
        assert_eq!(rt.conversations().unwrap()[0].alias, None);
        assert!(rt.remove_contact(&peer).unwrap());
        assert!(
            !rt.remove_contact(&peer).unwrap(),
            "second remove is a no-op"
        );
        assert_eq!(rt.messages_with(&peer).unwrap().len(), 1);
        assert!(matches!(
            rt.set_contact_alias("junk", "x"),
            Err(UiError::Engine(_))
        ));
    }

    #[tokio::test]
    async fn journal_lifecycle_add_list_delete() {
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();

        // Locked → typed errors, no panics.
        assert!(matches!(
            rt.add_journal_entry("hi", None),
            Err(UiError::VaultLocked)
        ));
        assert!(matches!(rt.journal_entries(), Err(UiError::VaultLocked)));
        assert!(matches!(
            rt.delete_journal_entry("x"),
            Err(UiError::VaultLocked)
        ));

        rt.unlock_vault(dir.path(), "pin").await.unwrap();

        let first = rt
            .add_journal_entry("  rough morning  ", Some("😕"))
            .unwrap();
        assert_eq!(first.text, "rough morning", "text is trimmed");
        assert_eq!(first.mood.as_deref(), Some("😕"));
        let second = rt.add_journal_entry("grateful today", Some("  ")).unwrap();
        assert_eq!(second.mood, None, "blank mood normalises to none");
        assert_ne!(first.id, second.id);

        // Whitespace-only text is rejected.
        assert!(matches!(
            rt.add_journal_entry("   ", None),
            Err(UiError::Engine(_))
        ));

        let entries = rt.journal_entries().unwrap();
        assert_eq!(entries.len(), 2);
        // Newest first; same-second entries fall back to id ordering.
        assert!(entries[0].created_at >= entries[1].created_at);

        assert!(rt.delete_journal_entry(&first.id).unwrap());
        assert!(!rt.delete_journal_entry(&first.id).unwrap());
        assert_eq!(rt.journal_entries().unwrap().len(), 1);

        // Entries survive a restart (encrypted at rest).
        drop(rt);
        let mut rt2 = ComradeRuntime::new();
        rt2.unlock_vault(dir.path(), "pin").await.unwrap();
        let entries = rt2.journal_entries().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].text, "grateful today");
    }

    #[tokio::test]
    async fn a_shared_note_reaches_the_thread_as_a_card_and_survives_a_reload() {
        let dir = TempDir::new().unwrap();
        let rt = offline_runtime(&dir).await;
        let (_hex, peer) = stranger();

        let entry = rt
            .add_journal_entry("rough morning, but I went for the walk", Some("😕"))
            .unwrap();
        let sent = rt.share_journal_entry(&peer, &entry.id).await.unwrap();

        // The bubble is drawable straight from the send, with the marker line
        // off the text and the mood beside it.
        let note = sent.shared_note.clone().expect("sent as a note");
        assert_eq!(note.text, "rough morning, but I went for the walk");
        assert_eq!(note.mood.as_deref(), Some("😕"));
        assert_eq!(sent.content, note.text, "content is the note, not the wire");
        assert_eq!(sent.author, MessageAuthor::Human);

        // The wire form is what is stored, exactly as Tara's marker is — so the
        // card is rebuilt from disk rather than from whatever the sending
        // session happened to hold.
        let stored = rt
            .ui
            .store_ref()
            .unwrap()
            .messages_with(&to_npub(&peer))
            .unwrap();
        assert!(stored[0]
            .content
            .starts_with(comrade_core::note::JOURNAL_NOTE_PREFIX));
        let thread = rt.messages_with(&peer).unwrap();
        assert_eq!(thread[0].shared_note, sent.shared_note);
        assert_eq!(thread[0].content, note.text);
    }

    /// A recording the frontend claims to have written, for the tests below.
    fn a_recording(file_name: &str) -> JournalRecordingDto {
        JournalRecordingDto {
            file_name: file_name.into(),
            mime: "video/mp4".into(),
            duration_ms: 47_000,
            size_bytes: 12_345_678,
        }
    }

    /// The same, spoken rather than filmed.
    fn a_voice_recording(file_name: &str) -> JournalRecordingDto {
        JournalRecordingDto {
            mime: "audio/aac".into(),
            duration_ms: 72_000,
            size_bytes: 348_160,
            ..a_recording(file_name)
        }
    }

    #[tokio::test]
    async fn a_recording_entry_keeps_its_title_and_needs_no_words() {
        let dir = TempDir::new().unwrap();
        let rt = offline_runtime(&dir).await;

        let entry = rt
            .add_journal_recording(
                Some("  The walk after the argument  "),
                "   ",
                Some("😕"),
                a_recording("jv-1723800000000-abc123.mp4"),
            )
            .unwrap();

        // Empty text is the normal case for a recording, unlike a typed entry.
        assert_eq!(entry.text, "");
        assert_eq!(entry.title.as_deref(), Some("The walk after the argument"));
        assert_eq!(entry.mood.as_deref(), Some("😕"));
        assert_eq!(
            entry.recording.as_ref().unwrap().file_name,
            "jv-1723800000000-abc123.mp4"
        );
        assert_eq!(entry.recording.as_ref().unwrap().duration_ms, 47_000);

        // …and it is in the one list the journal screen reads, alongside
        // typed entries rather than in a second place of its own.
        assert_eq!(rt.journal_entries().unwrap(), vec![entry]);
    }

    #[tokio::test]
    async fn a_voice_entry_takes_the_same_path_as_a_filmed_one() {
        // Audio and video differ in nothing this layer does. The point of the
        // test is that there is no second code path to keep in step — only the
        // mime comes back different, which is what the frontend switches on.
        let dir = TempDir::new().unwrap();
        let rt = offline_runtime(&dir).await;

        let spoken = rt
            .add_journal_recording(
                Some("Said it out loud"),
                "",
                Some("🙂"),
                a_voice_recording("ja-1723800000000-abc124.aac"),
            )
            .unwrap();
        let filmed = rt
            .add_journal_recording(None, "", None, a_recording("jv-1-a.mp4"))
            .unwrap();

        let spoken_recording = spoken.recording.as_ref().unwrap();
        assert_eq!(spoken_recording.mime, "audio/aac");
        assert_eq!(spoken_recording.duration_ms, 72_000);
        assert_eq!(spoken_recording.file_name, "ja-1723800000000-abc124.aac");
        assert_eq!(spoken.title.as_deref(), Some("Said it out loud"));
        assert_eq!(filmed.recording.as_ref().unwrap().mime, "video/mp4");

        // Both are in the one list, newest first, with nothing separating them.
        let entries = rt.journal_entries().unwrap();
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().all(|e| e.recording.is_some()));
    }

    #[tokio::test]
    async fn a_recording_entry_without_a_file_or_a_mime_is_refused() {
        let dir = TempDir::new().unwrap();
        let rt = offline_runtime(&dir).await;

        // Nothing to play: an entry like this would draw a card over a file
        // that does not exist, which reads as the app having lost a recording.
        assert!(matches!(
            rt.add_journal_recording(Some("titled"), "", None, a_recording("   ")),
            Err(UiError::Engine(_))
        ));
        // A path, not a name — the frontend's directory is not a suggestion.
        for escape in ["../../secrets.mp4", "sub/dir.mp4", "a\\b.mp4", "..mp4"] {
            assert!(
                matches!(
                    rt.add_journal_recording(None, "", None, a_recording(escape)),
                    Err(UiError::Engine(_))
                ),
                "{escape} should not be accepted as a file name"
            );
        }
        // No mime is no answer to "watched or listened to", and guessing here
        // would draw a silent black rectangle over somebody's voice entry.
        assert!(matches!(
            rt.add_journal_recording(
                None,
                "",
                None,
                JournalRecordingDto {
                    mime: "  ".into(),
                    ..a_recording("jv-1-a.mp4")
                },
            ),
            Err(UiError::Engine(_))
        ));
        assert!(rt.journal_entries().unwrap().is_empty());
    }

    #[tokio::test]
    async fn retitling_an_entry_changes_only_its_title() {
        let dir = TempDir::new().unwrap();
        let rt = offline_runtime(&dir).await;

        let entry = rt
            .add_journal_recording(
                Some("Untitled"),
                "a few words too",
                Some("🙂"),
                a_recording("jv-1-a.mp4"),
            )
            .unwrap();

        let renamed = rt
            .set_journal_entry_title(&entry.id, Some("  Sunday morning  "))
            .unwrap()
            .unwrap();
        assert_eq!(renamed.title.as_deref(), Some("Sunday morning"));
        assert_eq!(renamed.text, entry.text);
        assert_eq!(renamed.mood, entry.mood);
        assert_eq!(renamed.recording, entry.recording);
        // Renaming is not writing: the entry keeps its place in the history.
        assert_eq!(renamed.created_at, entry.created_at);

        // Whitespace clears the title rather than storing a blank heading.
        let cleared = rt
            .set_journal_entry_title(&entry.id, Some("   "))
            .unwrap()
            .unwrap();
        assert_eq!(cleared.title, None);
        assert_eq!(
            rt.set_journal_entry_title(&entry.id, None)
                .unwrap()
                .unwrap()
                .title,
            None
        );

        // …and the change is what the list reads back, not just what the call
        // returned.
        assert_eq!(rt.journal_entries().unwrap()[0].title, None);

        // A stale list on screen retitles nothing and says so.
        assert!(rt
            .set_journal_entry_title("00000000000000000001-gone", Some("x"))
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn sharing_a_note_leaves_the_journal_exactly_as_it_was() {
        // Sharing is a copy. The entry is not marked, not moved, and deleting it
        // afterwards still works — the chat message is a message from then on.
        let dir = TempDir::new().unwrap();
        let rt = offline_runtime(&dir).await;
        let (_hex, peer) = stranger();

        let entry = rt.add_journal_entry("a hard week", None).unwrap();
        rt.share_journal_entry(&peer, &entry.id).await.unwrap();
        assert_eq!(rt.journal_entries().unwrap(), vec![entry.clone()]);

        assert!(rt.delete_journal_entry(&entry.id).unwrap());
        assert!(rt.journal_entries().unwrap().is_empty());
        // …and the message stays: a delivered message belongs to both of them.
        let thread = rt.messages_with(&peer).unwrap();
        assert_eq!(thread.len(), 1);
        assert_eq!(thread[0].shared_note.as_ref().unwrap().text, "a hard week");
    }

    #[tokio::test]
    async fn a_recording_has_no_share_path_and_says_so() {
        // Sharing sends the words. Nothing in this app uploads a journal
        // recording, so an entry that is only a recording must fail loudly
        // rather than put a card carrying just a mood in somebody's chat.
        let dir = TempDir::new().unwrap();
        let rt = offline_runtime(&dir).await;
        let (_hex, peer) = stranger();

        let entry = rt
            .add_journal_recording(Some("The walk"), "", Some("😕"), a_recording("jv-1-a.mp4"))
            .unwrap();
        assert!(matches!(
            rt.share_journal_entry(&peer, &entry.id).await,
            Err(UiError::Engine(_))
        ));
        assert!(rt.messages_with(&peer).unwrap().is_empty());

        // A video entry the user *did* write words on shares those words.
        let with_words = rt
            .add_journal_recording(
                Some("The walk"),
                "said it out loud at last",
                None,
                a_recording("jv-2-b.mp4"),
            )
            .unwrap();
        rt.share_journal_entry(&peer, &with_words.id).await.unwrap();
        let thread = rt.messages_with(&peer).unwrap();
        assert_eq!(thread.len(), 1);
        assert_eq!(
            thread[0].shared_note.as_ref().unwrap().text,
            "said it out loud at last"
        );
    }

    #[tokio::test]
    async fn sharing_an_entry_that_is_gone_says_so_instead_of_sending_nothing() {
        let dir = TempDir::new().unwrap();
        let rt = offline_runtime(&dir).await;
        let (_hex, peer) = stranger();

        let entry = rt
            .add_journal_entry("deleted in the meantime", None)
            .unwrap();
        rt.delete_journal_entry(&entry.id).unwrap();

        assert!(matches!(
            rt.share_journal_entry(&peer, &entry.id).await,
            Err(UiError::Engine(_))
        ));
        assert!(
            rt.messages_with(&peer).unwrap().is_empty(),
            "no empty card was sent"
        );
    }

    #[tokio::test]
    async fn an_ordinary_message_is_never_drawn_as_a_journal_note() {
        let dir = TempDir::new().unwrap();
        let rt = offline_runtime(&dir).await;
        let (_hex, peer) = stranger();

        for text in [
            "from my journal: we should talk",
            "📓 notes from standup",
            "I wrote about this in my journal",
        ] {
            let sent = rt.send_dm(&peer, text).await.unwrap();
            assert_eq!(sent.shared_note, None, "{text:?}");
            assert_eq!(sent.content, text);
        }
    }

    #[test]
    fn a_live_arrival_carries_the_same_note_a_reload_would() {
        // The two paths into a thread are `messages_with` (this DTO's sibling)
        // and the live `IncomingDirectMessage` event. A frontend appends both to
        // one list, so a note parsed on only one of them would render as marker
        // text when it arrived and as a card after a restart.
        let wire = comrade_core::note::journal_note_line("a hard week", Some("😞"));
        let dto = DirectMessageDto::from(incoming("aa", "e1", &wire));
        let note = dto.shared_note.expect("parsed on the live path too");
        assert_eq!(note.text, "a hard week");
        assert_eq!(note.mood.as_deref(), Some("😞"));
        // …and the raw arrival keeps the marker: this DTO is the wire body, and
        // the card is built from `shared_note`.
        assert_eq!(dto.content, wire);

        assert_eq!(
            DirectMessageDto::from(incoming("aa", "e2", "an ordinary hello")).shared_note,
            None
        );
    }

    #[tokio::test]
    async fn sharing_a_note_needs_an_unlocked_vault() {
        let rt = ComradeRuntime::new();
        let (_hex, peer) = stranger();
        assert!(matches!(
            rt.share_journal_entry(&peer, "whatever").await,
            Err(UiError::VaultLocked)
        ));
    }

    #[tokio::test]
    async fn tara_lifecycle_send_thread_clear() {
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();

        // Locked → typed errors, no panics.
        assert!(matches!(rt.tara_send("hi"), Err(UiError::VaultLocked)));
        assert!(matches!(rt.tara_thread(), Err(UiError::VaultLocked)));
        assert!(matches!(rt.tara_opener(), Err(UiError::VaultLocked)));
        assert!(matches!(rt.clear_tara_thread(), Err(UiError::VaultLocked)));

        rt.unlock_vault(dir.path(), "pin").await.unwrap();

        // Fresh thread → honest opener; empty sends rejected.
        assert!(rt.tara_opener().unwrap().contains("not a therapist"));
        assert!(matches!(rt.tara_send("   "), Err(UiError::Engine(_))));

        let reply = rt.tara_send("  I've been anxious all week  ").unwrap();
        assert!(reply.from_tara);
        assert!(!reply.crisis);
        assert!(reply.text.contains("anxious"));

        // Thread is chat-ordered: trimmed user turn first, reply after it.
        let thread = rt.tara_thread().unwrap();
        assert_eq!(thread.len(), 2);
        assert_eq!(thread[0].text, "I've been anxious all week");
        assert!(!thread[0].from_tara);
        assert_eq!(thread[1].text, reply.text);

        // Distress → crisis flag on both turns, and resources to render.
        let crisis = rt.tara_send("I want to end it all").unwrap();
        assert!(crisis.crisis);
        assert!(!rt.tara_crisis_resources().is_empty());
        let thread = rt.tara_thread().unwrap();
        assert_eq!(thread.len(), 4);
        assert!(thread[2].crisis && thread[3].crisis);

        // The thread survives a restart (encrypted at rest)…
        drop(rt);
        let mut rt2 = ComradeRuntime::new();
        rt2.unlock_vault(dir.path(), "pin").await.unwrap();
        assert_eq!(rt2.tara_thread().unwrap().len(), 4);

        // …and clears on request.
        assert_eq!(rt2.clear_tara_thread().unwrap(), 4);
        assert!(rt2.tara_thread().unwrap().is_empty());
        assert_eq!(rt2.clear_tara_thread().unwrap(), 0);
    }

    #[tokio::test]
    async fn tara_opener_reflects_recent_low_journal_moods() {
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();

        rt.add_journal_entry("rough monday", Some("😞")).unwrap();
        rt.add_journal_entry("rough tuesday", Some("😕")).unwrap();
        assert!(rt.tara_opener().unwrap().contains("felt low"));
    }

    // ── Attention (wellbeing pillar #5) ───────────────────────────────────

    #[tokio::test]
    async fn attention_commands_reject_gracefully_when_vault_locked() {
        // Every store-backed command must fail closed rather than panic or
        // silently succeed — same contract the journal and Tara hold.
        let rt = ComradeRuntime::new();
        assert!(matches!(
            rt.record_attention_day("2026-07-31", 100, 40, 20),
            Err(UiError::VaultLocked)
        ));
        assert!(matches!(rt.attention_days(), Err(UiError::VaultLocked)));
        assert!(matches!(
            rt.attention_summary("2026-07-31"),
            Err(UiError::VaultLocked)
        ));
        assert!(matches!(rt.doom_apps(), Err(UiError::VaultLocked)));
        assert!(matches!(
            rt.set_doom_apps(vec![]),
            Err(UiError::VaultLocked)
        ));
        assert!(matches!(
            rt.start_focus_session("x", 25),
            Err(UiError::VaultLocked)
        ));
        assert!(matches!(
            rt.finish_focus_session(true),
            Err(UiError::VaultLocked)
        ));
        assert!(matches!(
            rt.active_focus_session(),
            Err(UiError::VaultLocked)
        ));
        assert!(matches!(rt.focus_sessions(), Err(UiError::VaultLocked)));
        assert!(matches!(
            rt.suggested_focus_minutes(),
            Err(UiError::VaultLocked)
        ));
        assert!(matches!(rt.focus_prompt(), Err(UiError::VaultLocked)));
        assert!(matches!(
            rt.focus_reflection("completed"),
            Err(UiError::VaultLocked)
        ));
        assert!(matches!(rt.save_read("t", "x"), Err(UiError::VaultLocked)));
        assert!(matches!(rt.saved_reads(), Err(UiError::VaultLocked)));
        assert!(matches!(
            rt.open_saved_read("id"),
            Err(UiError::VaultLocked)
        ));
        assert!(matches!(
            rt.set_saved_read_position("id", 0),
            Err(UiError::VaultLocked)
        ));
        assert!(matches!(
            rt.delete_saved_read("id"),
            Err(UiError::VaultLocked)
        ));
    }

    #[test]
    fn the_duration_chips_are_drawable_before_the_vault_is_open() {
        // The one attention call that is not vault-gated. A frontend paints its
        // focus surface on first frame; making it wait for a passphrase to know
        // what "25m / 45m / 90m" is would gate a constant behind a secret.
        let rt = ComradeRuntime::new();
        let presets = rt.focus_presets();
        assert_eq!(presets, attention::FOCUS_PRESETS.to_vec());
        assert!(!presets.is_empty());
        assert!(presets.windows(2).all(|w| w[0] < w[1]), "ascending");
    }

    #[test]
    fn the_stretch_routine_is_drawable_before_the_vault_is_open() {
        // Same reasoning as the duration chips: the routine is a constant of
        // the design, and a stretch break must not need a passphrase.
        let rt = ComradeRuntime::new();
        let routine = rt.stretch_routine();
        assert_eq!(routine.len(), attention::STRETCH_ROUTINE.len());
        for (dto, step) in routine.iter().zip(attention::STRETCH_ROUTINE) {
            assert_eq!(dto.key, step.key);
            assert_eq!(dto.seconds, step.seconds);
            assert_eq!(dto.mirrored, step.mirrored);
            assert!(!dto.name.is_empty());
            assert!(!dto.cue.is_empty());
        }
    }

    #[tokio::test]
    async fn attention_days_upsert_and_summarise_against_the_users_own_median() {
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();

        // A malformed date is refused rather than stored under a key that
        // would not sort chronologically.
        assert!(matches!(
            rt.record_attention_day("31-07-2026", 10, 1, 1),
            Err(UiError::Engine(_))
        ));

        for (date, screen, doom) in [
            ("2026-07-28", 100u32, 20u32),
            ("2026-07-29", 300, 90),
            ("2026-07-30", 200, 60),
            ("2026-07-31", 90, 15),
        ] {
            rt.record_attention_day(date, screen, 40, doom).unwrap();
        }
        // Same-date re-record updates in place as today's numbers grow.
        let today = rt.record_attention_day("2026-07-31", 120, 55, 25).unwrap();
        assert_eq!(today.screen_minutes, 120);
        assert_eq!(rt.attention_days().unwrap().len(), 4);
        assert_eq!(rt.attention_days().unwrap()[0].date, "2026-07-31");

        let summary = rt.attention_summary("2026-07-31").unwrap();
        assert_eq!(summary.today.as_ref().unwrap().screen_minutes, 120);
        assert_eq!(
            summary.sample_days, 3,
            "today is excluded from its baseline"
        );
        assert_eq!(summary.median_screen_minutes, 200);
        assert_eq!(summary.median_doom_minutes, 60);

        // A day with nothing recorded reports honestly rather than zeroes
        // masquerading as a measurement.
        let empty = rt.attention_summary("2026-08-05").unwrap();
        assert!(empty.today.is_none());
        assert_eq!(empty.sample_days, 4);
    }

    #[tokio::test]
    async fn doom_apps_are_the_users_own_list_cleaned_but_never_prefilled() {
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();

        // Comrade ships no built-in blacklist — the list starts empty.
        assert!(rt.doom_apps().unwrap().is_empty());
        let saved = rt
            .set_doom_apps(vec![
                "com.b".into(),
                "  ".into(),
                "com.a".into(),
                "com.b".into(),
            ])
            .unwrap();
        assert_eq!(saved, vec!["com.a".to_string(), "com.b".to_string()]);
        assert_eq!(rt.doom_apps().unwrap(), saved);
        // And it can be emptied again.
        assert!(rt.set_doom_apps(vec![]).unwrap().is_empty());
    }

    #[tokio::test]
    async fn focus_session_lifecycle_start_finish_and_history() {
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();

        assert!(rt.active_focus_session().unwrap().is_none());
        assert_eq!(rt.suggested_focus_minutes().unwrap(), 25, "starts small");
        assert!(!rt.focus_prompt().unwrap().is_empty());

        // Out-of-range durations are refused with a message, not clamped
        // silently — a five-hour "focus session" is not the practice.
        assert!(matches!(
            rt.start_focus_session("marathon", 600),
            Err(UiError::Engine(_))
        ));
        assert!(matches!(
            rt.start_focus_session("blink", 1),
            Err(UiError::Engine(_))
        ));

        let started = rt.start_focus_session("  draft the essay  ", 25).unwrap();
        assert_eq!(started.intent, "draft the essay");
        assert_eq!(started.outcome, None);
        assert!(started.remaining_secs > 24 * 60);
        assert_eq!(
            rt.active_focus_session().unwrap().map(|s| s.id),
            Some(started.id.clone())
        );

        let finished = rt.finish_focus_session(true).unwrap().unwrap();
        assert_eq!(finished.outcome.as_deref(), Some("completed"));
        assert_eq!(
            finished.remaining_secs, 0,
            "a finished session has no clock"
        );
        assert!(finished.ended_at.is_some());
        assert!(rt.active_focus_session().unwrap().is_none());
        // Finishing again is a clean None, not an error.
        assert!(rt.finish_focus_session(true).unwrap().is_none());
        assert_eq!(rt.focus_sessions().unwrap().len(), 1);

        // Abandoning is recorded plainly.
        rt.start_focus_session("second go", 25).unwrap();
        let abandoned = rt.finish_focus_session(false).unwrap().unwrap();
        assert_eq!(abandoned.outcome.as_deref(), Some("abandoned"));
        assert!(rt
            .focus_reflection("abandoned")
            .unwrap()
            .contains("not failure"));

        // Two completions unlock the next rung; the abandon costs nothing.
        rt.start_focus_session("third", 25).unwrap();
        rt.finish_focus_session(true).unwrap();
        assert_eq!(rt.suggested_focus_minutes().unwrap(), 45);
    }

    #[tokio::test]
    async fn starting_a_new_session_closes_the_one_left_running() {
        // Only one session runs at a time: the user has visibly moved on, so
        // the old one is closed rather than left dangling forever (which would
        // block every future start).
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();

        let first = rt.start_focus_session("one", 25).unwrap();
        let second = rt.start_focus_session("two", 45).unwrap();
        assert_ne!(first.id, second.id);
        assert_eq!(
            rt.active_focus_session().unwrap().map(|s| s.id),
            Some(second.id)
        );
        let history = rt.focus_sessions().unwrap();
        assert_eq!(history.len(), 2);
        let old = history.iter().find(|s| s.id == first.id).unwrap();
        assert_eq!(old.outcome.as_deref(), Some("abandoned"));
    }

    #[tokio::test]
    async fn a_session_nobody_came_back_to_is_lapsed_not_completed() {
        // The honesty rule the progressive-duration ladder depends on: a
        // session the user was absent for must not be able to claim a
        // completion, or the "practice" it measures never happened.
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();
        let store = rt.ui.store_ref().unwrap();

        let long_ago = now_secs() - 6 * 3600;
        store
            .save_focus_session(&comrade_storage::FocusSession {
                id: timestamped_store_id(long_ago),
                intent: "yesterday's plan".into(),
                planned_minutes: 25,
                started_at: long_ago,
                ended_at: None,
                outcome: None,
            })
            .unwrap();

        // Reading the active session resolves the stale one and reports none.
        assert!(rt.active_focus_session().unwrap().is_none());
        let history = rt.focus_sessions().unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].outcome.as_deref(), Some("lapsed"));
        // It ended when its plan did — not hours later when someone looked.
        assert_eq!(history[0].ended_at, Some(long_ago + 25 * 60));
        // And it earns no credit toward the next rung.
        assert_eq!(rt.suggested_focus_minutes().unwrap(), 25);
        assert!(rt
            .focus_reflection("lapsed")
            .unwrap()
            .contains("Nothing lost"));
    }

    #[tokio::test]
    async fn the_reading_library_roundtrips_chunked_with_clamped_positions() {
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();

        assert!(rt.saved_reads().unwrap().is_empty());
        assert!(matches!(rt.save_read("t", "   "), Err(UiError::Engine(_))));

        let text = "A paragraph worth a couple of minutes of attention.\n\n".repeat(80);
        let saved = rt.save_read("  Walden  ", &text).unwrap();
        assert_eq!(saved.title, "Walden");
        assert!(saved.chunks.len() > 1);
        assert_eq!(saved.position, 0);
        assert_eq!(saved.source, "", "pasted prose carries no source label");
        // Losslessness carries through the DTO: the reader shows exactly what
        // was saved (the trailing trim is the only edit, and it is announced).
        assert_eq!(saved.chunks.concat(), text.trim());

        // A second article, this one carried in with a link: the library keys
        // its source off the first URL's host, offline.
        let shared = rt
            .save_read("", "worth a read https://www.instagram.com/p/abc/ tonight")
            .unwrap();
        assert_eq!(shared.source, "instagram.com");

        // Both saves can land in the same second, which makes their relative
        // order a coin toss on the random id tail — newest-first ordering is
        // pinned in the storage tests with distinct timestamps instead.
        let rows = rt.saved_reads().unwrap();
        assert_eq!(rows.len(), 2);
        let saved_row = rows.iter().find(|r| r.id == saved.id).unwrap();
        assert_eq!(saved_row.chunk_count as usize, saved.chunks.len());

        let moved = rt.set_saved_read_position(&saved.id, 2).unwrap().unwrap();
        assert_eq!(moved.position, 2);
        assert_eq!(
            rt.open_saved_read(&saved.id).unwrap().unwrap().position,
            2,
            "each read keeps its own place"
        );
        assert_eq!(rt.open_saved_read(&shared.id).unwrap().unwrap().position, 0);
        // A position past the end is clamped, never trusted.
        let clamped = rt
            .set_saved_read_position(&saved.id, 9_999)
            .unwrap()
            .unwrap();
        assert_eq!(clamped.position as usize, clamped.chunks.len() - 1);

        assert!(rt.delete_saved_read(&saved.id).unwrap());
        assert!(!rt.delete_saved_read(&saved.id).unwrap());
        assert_eq!(rt.saved_reads().unwrap().len(), 1);
        // With the row gone, opening and moving are clean Nones.
        assert!(rt.open_saved_read(&saved.id).unwrap().is_none());
        assert!(rt.set_saved_read_position(&saved.id, 0).unwrap().is_none());
    }

    #[tokio::test]
    async fn a_pre_library_vaults_single_read_migrates_into_the_library() {
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();
        let store = rt.ui.store_ref().unwrap();

        // A vault written before the library existed: one read, mid-progress,
        // in the old single-slot tree.
        store
            .save_reading_state(&comrade_storage::ReadingState {
                title: "Walden".into(),
                // Long enough to chunk more than once, so the position below
                // is real progress and not something the clamp pulls back.
                text: "A paragraph worth a couple of minutes of attention.\n\n".repeat(80),
                position: 1,
                updated_at: 1_700_000_000,
            })
            .unwrap();

        let rows = rt.saved_reads().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].title, "Walden");
        assert_eq!(rows[0].position, 1, "progress survives the move");

        // The move happened once: the slot is empty and a second list does not
        // mint a duplicate.
        assert!(store.load_reading_state().unwrap().is_none());
        assert_eq!(rt.saved_reads().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn tara_opener_nudges_on_a_heavy_scroll_day_but_mood_still_wins() {
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();

        // Baseline days plus a heavy "yesterday". Dates are chosen relative to
        // the real clock so the today/yesterday split is what the code sees.
        let today = iso_date(now_secs());
        let yesterday = iso_date(now_secs() - 86_400);
        for (date, doom) in [
            (iso_date(now_secs() - 4 * 86_400), 30u32),
            (iso_date(now_secs() - 3 * 86_400), 30),
            (iso_date(now_secs() - 2 * 86_400), 30),
        ] {
            rt.record_attention_day(&date, 200, 40, doom).unwrap();
        }
        rt.record_attention_day(&yesterday, 400, 120, 150).unwrap();
        rt.record_attention_day(&today, 10, 2, 0).unwrap();

        let opener = rt.tara_opener().unwrap();
        assert!(opener.contains("150 minutes"), "got: {opener}");
        assert!(opener.contains("No judgement"), "got: {opener}");

        // …but two low journal days outrank it: the heavier signal wins.
        rt.add_journal_entry("rough monday", Some("😞")).unwrap();
        rt.add_journal_entry("rough tuesday", Some("😕")).unwrap();
        assert!(rt.tara_opener().unwrap().contains("felt low"));
    }

    #[tokio::test]
    async fn tara_opener_is_unchanged_when_no_usage_is_recorded() {
        // The mirror is opt-in; someone who never grants usage access must see
        // exactly the opener that shipped before this pillar existed.
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();
        assert!(rt.tara_opener().unwrap().contains("not a therapist"));
    }

    #[test]
    fn iso_date_matches_known_epochs_and_shape_check_agrees() {
        assert_eq!(iso_date(0), "1970-01-01");
        assert_eq!(iso_date(1_700_000_000), "2023-11-14");
        // A leap day, and the day after.
        assert_eq!(iso_date(1_709_164_800), "2024-02-29");
        assert_eq!(iso_date(1_709_251_200), "2024-03-01");
        for s in ["1970-01-01", "2026-07-31"] {
            assert!(is_iso_date(s), "{s} should be accepted");
        }
        for s in ["2026-7-31", "31-07-2026", "2026/07/31", "2026-07-3x", ""] {
            assert!(!is_iso_date(s), "{s} should be rejected");
        }
    }

    #[test]
    fn journal_ids_sort_chronologically_and_never_collide() {
        let a = timestamped_store_id(5);
        let b = timestamped_store_id(5);
        let later = timestamped_store_id(1_700_000_000);
        assert_ne!(a, b, "same-second ids must differ");
        assert!(a < later && b < later, "timestamp prefix sorts");
    }

    #[tokio::test]
    async fn peer_profile_joins_the_contact_the_cache_and_presence() {
        // The one call a profile page makes. Without it a frontend would have to
        // join three stores itself, and each one would join them differently.
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();
        let peer = nostr_sdk::prelude::Keys::generate()
            .public_key()
            .to_bech32()
            .unwrap();

        rt.add_contact(&peer, "Chas").unwrap();
        let store = rt.ui.store_ref().unwrap();
        merge_peer_profile(
            store,
            &peer,
            PeerProfilePatch {
                name: Some("charlie".into()),
                about: Some("gardener".into()),
                picture: Some("https://example.com/c.png".into()),
                nip05: Some("charlie@example.com".into()),
                ..Default::default()
            },
        );

        let p = rt.peer_profile(&peer).unwrap();
        assert_eq!(p.npub, peer);
        assert_eq!(p.alias, "Chas", "the alias the *user* chose comes first");
        assert_eq!(p.name.as_deref(), Some("charlie"));
        assert_eq!(p.about.as_deref(), Some("gardener"));
        assert_eq!(p.picture.as_deref(), Some("https://example.com/c.png"));
        assert_eq!(p.nip05.as_deref(), Some("charlie@example.com"));
        assert!(p.contact);
        assert!(!p.comrade);
        assert!(!p.blocked);
        assert!(
            !p.avatar_cached,
            "a URL is not bytes; nothing has been fetched"
        );
        assert!(
            !p.online,
            "presence only flows between comrades, so a plain contact is never online"
        );
        assert!(p.updated_at > 0, "the page can say how fresh this is");
    }

    #[tokio::test]
    async fn a_stranger_has_a_profile_too_rather_than_an_error() {
        // Opening the profile of someone who is not a contact is the ordinary
        // case for a message request — it must render, not fail.
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();
        let peer = nostr_sdk::prelude::Keys::generate()
            .public_key()
            .to_bech32()
            .unwrap();

        let p = rt.peer_profile(&peer).unwrap();
        assert_eq!(p.npub, peer, "the key is the one thing always known");
        assert_eq!(p.alias, "");
        assert_eq!(p.name, None);
        assert!(!p.contact);
        assert!(!p.avatar_cached);
    }

    #[tokio::test]
    async fn peer_profile_and_peer_avatar_need_an_unlocked_vault() {
        let rt = ComradeRuntime::new();
        assert!(matches!(
            rt.peer_profile("npub1anything"),
            Err(UiError::VaultLocked) | Err(UiError::Engine(_))
        ));
        assert!(matches!(
            rt.remote_avatars_enabled(),
            Err(UiError::VaultLocked)
        ));
        assert!(matches!(
            rt.set_remote_avatars_enabled(false),
            Err(UiError::VaultLocked)
        ));
    }

    #[tokio::test]
    async fn an_avatar_record_pointing_at_missing_bytes_reads_as_initials() {
        // Not an error: there is nothing the user could do, and nothing is broken.
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();
        let peer = nostr_sdk::prelude::Keys::generate()
            .public_key()
            .to_bech32()
            .unwrap();
        let store = rt.ui.store_ref().unwrap();
        merge_peer_profile(
            store,
            &peer,
            PeerProfilePatch {
                avatar: Some(("sha-with-no-blob".into(), "image/png".into())),
                ..Default::default()
            },
        );
        assert_eq!(rt.peer_avatar(&peer).unwrap(), None);
    }

    #[tokio::test]
    async fn a_cached_avatar_comes_back_base64_with_its_sniffed_type() {
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();
        let peer = nostr_sdk::prelude::Keys::generate()
            .public_key()
            .to_bech32()
            .unwrap();
        let store = rt.ui.store_ref().unwrap();
        store
            .put_bytes(PEER_AVATAR_BLOBS_TREE, "abc", b"\x89PNG-ish")
            .unwrap();
        merge_peer_profile(
            store,
            &peer,
            PeerProfilePatch {
                avatar: Some(("abc".into(), "image/png".into())),
                ..Default::default()
            },
        );

        let bytes = rt.peer_avatar(&peer).unwrap().expect("cached avatar");
        assert_eq!(bytes.mime_type, "image/png");
        assert_eq!(B64.decode(bytes.base64).unwrap(), b"\x89PNG-ish");
        assert!(rt.peer_profile(&peer).unwrap().avatar_cached);
    }

    #[tokio::test]
    async fn remote_avatars_default_to_on_and_survive_a_round_trip() {
        // Default on is an explicit owner decision, not an accident of `unwrap_or`.
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();
        assert!(rt.remote_avatars_enabled().unwrap(), "default is on");
        rt.set_remote_avatars_enabled(false).unwrap();
        assert!(!rt.remote_avatars_enabled().unwrap());
        rt.set_remote_avatars_enabled(true).unwrap();
        assert!(rt.remote_avatars_enabled().unwrap());
    }

    #[tokio::test]
    async fn set_about_strips_controls_bounds_length_and_clears_on_empty() {
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();
        rt.ui.generate_identity().unwrap();

        let p = rt.set_about("  gardener\n, occasionally  ").await.unwrap();
        assert_eq!(
            p.about.as_deref(),
            Some("gardener, occasionally"),
            "our own text should not carry a newline onto somebody else's page either"
        );

        let long = "x".repeat(MAX_ABOUT_LEN + 50);
        let p = rt.set_about(&long).await.unwrap();
        assert_eq!(p.about.as_deref().map(str::len), Some(MAX_ABOUT_LEN));

        // Empty clears — the case a plain `Option<&str>` could not express.
        let p = rt.set_about("   ").await.unwrap();
        assert_eq!(p.about, None);
        assert_eq!(rt.profile().unwrap().about, None, "and it stays cleared");
    }

    #[tokio::test]
    async fn a_profile_share_no_longer_erases_a_cached_bio() {
        // The bug this fixes, and it fails before the merge writer exists.
        //
        // `cache_pushed_peer_name` built a whole `PeerProfileRecord` with
        // `about: None`, so a peer's profile-share envelope — which arrives
        // exactly when a message request is accepted — wiped any bio already
        // cached for them. Nothing read `about`, so nothing ever noticed. The
        // moment a profile page renders one it becomes "their bio vanished when I
        // accepted them", reported as data loss and reproducible only by
        // accepting a request.
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();
        let store = rt.ui.store_ref().unwrap();

        let peer = nostr_sdk::prelude::Keys::generate()
            .public_key()
            .to_bech32()
            .unwrap();

        // A search or a refresh cached everything the relay knew.
        merge_peer_profile(
            store,
            &peer,
            PeerProfilePatch {
                name: Some("charlie".into()),
                about: Some("gardener, occasionally".into()),
                picture: Some("https://example.com/c.png".into()),
                nip05: Some("charlie@example.com".into()),
                ..Default::default()
            },
        );

        // Then they accepted, and pushed us their handle over the DM channel.
        cache_pushed_peer_name(store, &peer, "charlie");

        let after = cached_peer_profile(store, &peer).expect("record still there");
        assert_eq!(after.name.as_deref(), Some("charlie"));
        assert_eq!(
            after.about.as_deref(),
            Some("gardener, occasionally"),
            "a share that carries only a handle must not claim there is no bio"
        );
        assert_eq!(
            after.picture.as_deref(),
            Some("https://example.com/c.png"),
            "nor that there is no picture"
        );
        assert_eq!(after.nip05.as_deref(), Some("charlie@example.com"));
    }

    #[tokio::test]
    async fn profile_rows_written_by_older_builds_still_deserialise() {
        // The record grew from three fields to ten. Every one of them defaults, so
        // a row a shipped build wrote must still read back — this puts the exact
        // bytes an older build stored, rather than a struct this build built, which
        // is the only version of the test that proves anything.
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();
        let store = rt.ui.store_ref().unwrap();

        for (npub, json, name, about, updated) in [
            (
                "npub_three_field",
                br#"{"name":"charlie","about":"bio","updated_at":7}"#.as_slice(),
                Some("charlie"),
                Some("bio"),
                7u64,
            ),
            // Older still: before `about` was cached at all.
            (
                "npub_one_field",
                br#"{"name":"dana"}"#.as_slice(),
                Some("dana"),
                None,
                0,
            ),
        ] {
            store.put_bytes(PEER_PROFILES_TREE, npub, json).unwrap();
            let r = cached_peer_profile(store, npub)
                .unwrap_or_else(|| panic!("{npub} no longer deserialises"));
            assert_eq!(r.name.as_deref(), name);
            assert_eq!(r.about.as_deref(), about);
            assert_eq!(r.updated_at, updated);
            assert_eq!(
                r.picture, None,
                "a field the writer never knew reads as None"
            );
            assert_eq!(r.avatar_sha256, None);
            assert_eq!(r.avatar_failed_at, 0);
        }
    }

    #[tokio::test]
    async fn a_changed_picture_url_invalidates_the_cached_bytes() {
        // The bytes we hold are the *old* picture. Keeping them against a new URL
        // would show a contact's previous avatar indefinitely.
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();
        let store = rt.ui.store_ref().unwrap();
        let peer = "npub_picture_change";

        merge_peer_profile(
            store,
            peer,
            PeerProfilePatch {
                picture: Some("https://example.com/one.png".into()),
                avatar: Some(("deadbeef".into(), "image/png".into())),
                ..Default::default()
            },
        );
        assert!(cached_peer_profile(store, peer)
            .unwrap()
            .avatar_sha256
            .is_some());

        merge_peer_profile(
            store,
            peer,
            PeerProfilePatch {
                picture: Some("https://example.com/two.png".into()),
                ..Default::default()
            },
        );
        let after = cached_peer_profile(store, peer).unwrap();
        assert_eq!(
            after.picture.as_deref(),
            Some("https://example.com/two.png")
        );
        assert_eq!(
            after.avatar_sha256, None,
            "the cached bytes are the old picture and must not survive the URL change"
        );

        // Re-publishing the *same* URL must not throw away a good cache, or every
        // refresh sweep would re-download every avatar.
        merge_peer_profile(
            store,
            peer,
            PeerProfilePatch {
                avatar: Some(("cafe".into(), "image/png".into())),
                ..Default::default()
            },
        );
        merge_peer_profile(
            store,
            peer,
            PeerProfilePatch {
                picture: Some("https://example.com/two.png".into()),
                ..Default::default()
            },
        );
        assert_eq!(
            cached_peer_profile(store, peer)
                .unwrap()
                .avatar_sha256
                .as_deref(),
            Some("cafe"),
            "an unchanged URL is not a reason to refetch"
        );
    }

    #[tokio::test]
    async fn a_failed_avatar_fetch_keeps_the_picture_already_cached() {
        // A refresh that could not reach the host must not blank an avatar we
        // already have — the same discipline the name refresh already applies.
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();
        let store = rt.ui.store_ref().unwrap();
        let peer = "npub_failed_fetch";

        merge_peer_profile(
            store,
            peer,
            PeerProfilePatch {
                picture: Some("https://example.com/a.png".into()),
                avatar: Some(("abc123".into(), "image/webp".into())),
                ..Default::default()
            },
        );
        merge_peer_profile(
            store,
            peer,
            PeerProfilePatch {
                avatar_failed: true,
                ..Default::default()
            },
        );

        let after = cached_peer_profile(store, peer).unwrap();
        assert_eq!(after.avatar_sha256.as_deref(), Some("abc123"));
        assert_eq!(after.avatar_mime.as_deref(), Some("image/webp"));
        assert!(
            after.avatar_failed_at > 0,
            "the failure is stamped so the negative TTL can hold off a retry"
        );
    }

    #[tokio::test]
    async fn legacy_placeholder_petnames_no_longer_mask_published_names() {
        // Old builds auto-filled an empty alias with the first 12 chars of
        // the npub. Those placeholders must read as "no alias" so the peer's
        // published handle can title the chat after an upgrade.
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();
        let store = rt.ui.store_ref().unwrap();

        let peer = nostr_sdk::prelude::Keys::generate()
            .public_key()
            .to_bech32()
            .unwrap();
        let placeholder: String = peer.chars().take(12).collect();
        store
            .upsert_contact(&comrade_storage::Contact {
                npub: peer.clone(),
                petname: placeholder.clone(),
                relays: vec![],
                comrade: false,
            })
            .unwrap();
        store
            .save_message(&comrade_storage::StoredMessage {
                id: "m1".into(),
                peer_npub: peer.clone(),
                content: "hi".into(),
                created_at: 1,
                outgoing: false,
                status: None,
                reply_to: None,
            })
            .unwrap();
        store
            .put(
                PEER_PROFILES_TREE,
                &peer,
                &PeerProfileRecord {
                    name: Some("charlie".into()),
                    updated_at: 1,
                    ..Default::default()
                },
            )
            .unwrap();

        let convo = &rt.conversations().unwrap()[0];
        assert_eq!(convo.alias, None, "placeholder is not a user alias");
        assert_eq!(convo.peer_name.as_deref(), Some("charlie"));
        assert_eq!(rt.list_contacts().unwrap()[0].alias, "");

        // A real alias — even one that looks key-ish but isn't this npub's
        // prefix — still wins.
        assert_eq!(user_alias("Mom", &peer).as_deref(), Some("Mom"));
        assert_eq!(user_alias(&placeholder, &peer), None);
        assert_eq!(user_alias("  ", &peer), None);
        assert_eq!(
            user_alias("npub1someone", &peer).as_deref(),
            Some("npub1someone"),
            "a 12-char alias that is not this peer's prefix is kept"
        );
    }

    #[tokio::test]
    async fn conversations_and_contacts_carry_cached_peer_names() {
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();
        let store = rt.ui.store_ref().unwrap();

        let peer = nostr_sdk::prelude::Keys::generate()
            .public_key()
            .to_bech32()
            .unwrap();
        store
            .save_message(&comrade_storage::StoredMessage {
                id: "m1".into(),
                peer_npub: peer.clone(),
                content: "hi".into(),
                created_at: 10,
                outgoing: false,
                status: None,
                reply_to: None,
            })
            .unwrap();
        // Simulate a discovered/refreshed profile in the cache.
        store
            .put(
                PEER_PROFILES_TREE,
                &peer,
                &PeerProfileRecord {
                    name: Some("charlie".into()),
                    updated_at: 1,
                    ..Default::default()
                },
            )
            .unwrap();

        let convos = rt.conversations().unwrap();
        assert_eq!(convos.len(), 1);
        assert_eq!(convos[0].alias, None, "no user alias was set");
        assert_eq!(
            convos[0].peer_name.as_deref(),
            Some("charlie"),
            "published handle from the profile cache titles the chat"
        );

        rt.add_contact(&peer, "").unwrap();
        let contacts = rt.list_contacts().unwrap();
        assert_eq!(contacts[0].name.as_deref(), Some("charlie"));

        // A user alias always outranks the published handle in the DTO —
        // display precedence is enforced by returning both.
        rt.set_contact_alias(&peer, "My Buddy").unwrap();
        let convos = rt.conversations().unwrap();
        assert_eq!(convos[0].alias.as_deref(), Some("My Buddy"));
        assert_eq!(convos[0].peer_name.as_deref(), Some("charlie"));
    }

    #[tokio::test]
    async fn conversations_group_history_by_peer_newest_first() {
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();
        let store = rt.ui.store_ref().unwrap();

        let alice = nostr_sdk::prelude::Keys::generate()
            .public_key()
            .to_bech32()
            .unwrap();
        let bob = nostr_sdk::prelude::Keys::generate()
            .public_key()
            .to_bech32()
            .unwrap();
        for (id, peer, content, at, outgoing) in [
            ("m1", &alice, "hi alice", 10u64, true),
            ("m2", &alice, "hello back", 20, false),
            ("m3", &bob, "yo bob", 15, true),
        ] {
            store
                .save_message(&comrade_storage::StoredMessage {
                    id: id.into(),
                    peer_npub: peer.to_string(),
                    content: content.into(),
                    created_at: at,
                    outgoing,
                    status: None,
                    reply_to: None,
                })
                .unwrap();
        }
        rt.add_contact(&alice, "Alice").unwrap();

        let convos = rt.conversations().unwrap();
        assert_eq!(convos.len(), 2);
        // Alice's thread is newest (t=20) and carries her saved alias.
        assert_eq!(convos[0].peer, alice);
        assert_eq!(convos[0].alias.as_deref(), Some("Alice"));
        assert_eq!(convos[0].last_message, "hello back");
        assert!(!convos[0].last_outgoing);
        assert_eq!(convos[1].peer, bob);
        assert_eq!(convos[1].alias, None);

        // Per-thread history comes back oldest-first for rendering.
        let msgs = rt.messages_with(&alice).unwrap();
        assert_eq!(
            msgs.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            ["m1", "m2"]
        );
    }

    #[tokio::test]
    async fn dm_and_profile_commands_reject_when_locked() {
        let mut rt = ComradeRuntime::new();
        assert!(matches!(
            rt.send_dm("npub1x", "hi").await,
            Err(UiError::VaultLocked)
        ));
        assert!(matches!(rt.conversations(), Err(UiError::VaultLocked)));
        assert!(matches!(rt.messages_with("x"), Err(UiError::VaultLocked)));
        assert!(matches!(rt.list_contacts(), Err(UiError::VaultLocked)));
        assert!(matches!(rt.profile(), Err(UiError::NoIdentity)));
        assert!(matches!(
            rt.set_username("neo").await,
            Err(UiError::NoIdentity)
        ));
        assert!(matches!(
            rt.search_profiles("neo").await,
            Err(UiError::VaultLocked)
        ));
        assert!(matches!(
            rt.refresh_peer_profiles().await,
            Err(UiError::VaultLocked)
        ));
        assert!(matches!(
            rt.set_contact_alias("npub1x", "a"),
            Err(UiError::VaultLocked)
        ));
        assert!(matches!(
            rt.remove_contact("npub1x"),
            Err(UiError::VaultLocked)
        ));
    }

    #[tokio::test]
    async fn send_dm_rejects_empty_and_bad_recipient() {
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();
        assert!(rt.send_dm("npub1notvalid", "hello").await.is_err());
        let ok = nostr_sdk::prelude::Keys::generate()
            .public_key()
            .to_bech32()
            .unwrap();
        assert!(matches!(
            rt.send_dm(&ok, "   ").await,
            Err(UiError::Engine(_))
        ));
    }

    #[tokio::test]
    async fn media_send_rejects_when_locked() {
        // No identity/engines yet → graceful typed error, no panic.
        let rt = ComradeRuntime::new();
        let err = rt
            .upload_and_send_media("npub1xxx", vec![1, 2, 3], "image/png", "")
            .await;
        assert!(err.is_err());
        let err = rt.download_and_decrypt_media("deadbeef").await;
        assert!(matches!(err, Err(UiError::VaultLocked)));
    }

    #[tokio::test]
    async fn media_send_pipeline_to_self_without_http_feature() {
        // Exercises identity + ECDH key derivation + encrypt + local-ref logic
        // up to the network boundary. With `media-http` off, the upload step
        // returns a typed error (no panic); the run never touches a relay.
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        let id = rt.unlock_vault(dir.path(), "pin").await.unwrap();
        let res = rt
            .upload_and_send_media(&id.npub, vec![9, 8, 7, 6], "image/png", "selfie")
            .await;
        #[cfg(not(feature = "media-http"))]
        assert!(matches!(res, Err(UiError::Engine(_))));
        // (With the feature on this would attempt a real Blossom upload.)
        let _ = res;
    }

    #[tokio::test]
    async fn download_resolves_persisted_ref() {
        // A persisted ref is resolved and key-derivation runs; only the final
        // network fetch is gated by the feature.
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        let id = rt.unlock_vault(dir.path(), "pin").await.unwrap();
        let peer_hex = nostr_sdk::prelude::PublicKey::parse(&id.npub)
            .unwrap()
            .to_hex();
        let reff = MediaRef {
            event_id: "evt1".into(),
            url: "https://blob.example/abc".into(),
            peer_pubkey: peer_hex,
            mime_type: "image/png".into(),
            caption: "x".into(),
            size: 3,
            sha256_hex: String::new(),
            outgoing: false,
            created_at: 1,
        };
        rt.ui
            .store_ref()
            .unwrap()
            .put(MEDIA_REFS_TREE, "evt1", &reff)
            .unwrap();

        let out = rt.download_and_decrypt_media("evt1").await;
        // Unknown id is a clean error; known id reaches the (gated) fetch step.
        assert!(rt.download_and_decrypt_media("nope").await.is_err());
        #[cfg(not(feature = "media-http"))]
        assert!(matches!(out, Err(UiError::Engine(_))));
        let _ = out;
    }

    #[tokio::test]
    async fn media_with_lists_history_oldest_first_with_correct_direction() {
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        let id = rt.unlock_vault(dir.path(), "pin").await.unwrap();
        let (peer_hex, peer_npub) = stranger();

        let incoming = MediaRef {
            event_id: "evt_in".into(),
            url: "https://blob.example/in".into(),
            peer_pubkey: peer_hex.clone(),
            mime_type: "image/png".into(),
            caption: "from them".into(),
            size: 3,
            sha256_hex: String::new(),
            outgoing: false,
            created_at: 10,
        };
        let outgoing = MediaRef {
            event_id: "evt_out".into(),
            url: "https://blob.example/out".into(),
            peer_pubkey: peer_hex,
            mime_type: "audio/ogg".into(),
            caption: "from me".into(),
            size: 5,
            sha256_hex: String::new(),
            outgoing: true,
            created_at: 20,
        };
        let store = rt.ui.store_ref().unwrap();
        store.put(MEDIA_REFS_TREE, "evt_in", &incoming).unwrap();
        store.put(MEDIA_REFS_TREE, "evt_out", &outgoing).unwrap();

        let history = rt.media_with(&peer_npub).unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].event_id, "evt_in");
        assert!(!history[0].outgoing);
        assert_eq!(history[0].sender, peer_npub);
        assert_eq!(history[1].event_id, "evt_out");
        assert!(history[1].outgoing);
        assert_eq!(history[1].sender, id.npub);
    }

    #[tokio::test]
    async fn media_with_rejects_when_locked_and_is_empty_for_a_stranger() {
        let rt = ComradeRuntime::new();
        let (_, peer_npub) = stranger();
        assert!(matches!(
            rt.media_with(&peer_npub),
            Err(UiError::VaultLocked)
        ));

        let dir = TempDir::new().unwrap();
        let mut rt2 = ComradeRuntime::new();
        rt2.unlock_vault(dir.path(), "pin").await.unwrap();
        assert!(rt2.media_with(&peer_npub).unwrap().is_empty());
    }

    // ── Message actions: star, pin, delete-for-me/everyone, forward ──────────

    fn plain_message(
        id: &str,
        peer: &str,
        content: &str,
        at: u64,
        outgoing: bool,
    ) -> comrade_storage::StoredMessage {
        comrade_storage::StoredMessage {
            id: id.into(),
            peer_npub: peer.into(),
            content: content.into(),
            created_at: at,
            outgoing,
            status: outgoing.then(|| "sent".to_string()),
            reply_to: None,
        }
    }

    #[tokio::test]
    async fn messages_with_hides_a_tombstoned_message_and_a_backfill_cannot_undo_it() {
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();
        let (_hex, peer) = stranger();
        let store = rt.ui.store_ref().unwrap();
        let row = plain_message("m1", &peer, "hi", 1, false);
        store.save_message(&row).unwrap();
        assert_eq!(rt.messages_with(&peer).unwrap().len(), 1);

        rt.delete_message_for_me(&peer, "m1").unwrap();
        assert!(rt.messages_with(&peer).unwrap().is_empty());

        // A relay's cold-start rescan (or a mesh replay) redelivers the same
        // event id — the whole reason this is a tombstone and not a row
        // delete is that it must stay hidden anyway.
        store.save_message(&row).unwrap();
        assert!(
            store.get_message("m1").unwrap().is_some(),
            "the row is back — the cache does not know about the tombstone"
        );
        assert!(
            rt.messages_with(&peer).unwrap().is_empty(),
            "but the tombstone survives the replay"
        );
    }

    #[tokio::test]
    async fn star_and_pin_surface_on_messages_with_and_their_own_readers() {
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();
        let (_hex, peer) = stranger();
        let store = rt.ui.store_ref().unwrap();
        store
            .save_message(&plain_message("m1", &peer, "hi", 1, false))
            .unwrap();

        let before = rt.messages_with(&peer).unwrap();
        assert!(!before[0].actions.starred);
        assert!(!before[0].actions.pinned);

        assert!(rt.star_message(&peer, "m1", true).unwrap());
        // Starring again is not news.
        assert!(!rt.star_message(&peer, "m1", true).unwrap());
        assert!(rt.pin_message(&peer, "m1").unwrap());

        let after = rt.messages_with(&peer).unwrap();
        assert!(after[0].actions.starred);
        assert!(after[0].actions.pinned);
        assert_eq!(rt.pinned_messages(&peer).unwrap().len(), 1);
        assert_eq!(rt.starred_messages().unwrap().len(), 1);

        assert!(rt.unpin_message(&peer, "m1").unwrap());
        assert!(!rt.messages_with(&peer).unwrap()[0].actions.pinned);
        assert!(rt.pinned_messages(&peer).unwrap().is_empty());
    }

    #[tokio::test]
    async fn pin_message_refuses_past_the_cap_but_unpinning_frees_a_slot() {
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();
        let (_hex, peer) = stranger();
        let store = rt.ui.store_ref().unwrap();
        for i in 0..comrade_storage::EncryptedStore::MAX_PINNED_PER_CONVERSATION {
            let id = format!("m{i}");
            store
                .save_message(&plain_message(&id, &peer, "x", i as u64, false))
                .unwrap();
            assert!(rt.pin_message(&peer, &id).unwrap());
        }
        assert!(rt.pin_message(&peer, "one-too-many").is_err());
        rt.unpin_message(&peer, "m0").unwrap();
        store
            .save_message(&plain_message("one-too-many", &peer, "x", 999, false))
            .unwrap();
        assert!(rt.pin_message(&peer, "one-too-many").unwrap());
    }

    #[tokio::test]
    async fn only_the_sender_can_delete_a_message_for_everyone() {
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();
        let (_hex, peer) = stranger();
        let store = rt.ui.store_ref().unwrap();
        store
            .save_message(&plain_message("in1", &peer, "hello", 1, false))
            .unwrap();

        assert!(matches!(
            rt.delete_message_for_everyone(&peer, "in1").await,
            Err(UiError::Engine(_))
        ));
        assert!(!store.is_deleted_for_me(&peer, "in1").unwrap());
        assert_eq!(rt.messages_with(&peer).unwrap().len(), 1);
    }

    #[tokio::test]
    async fn delete_for_everyone_hides_it_here_even_if_no_relay_hears_the_courtesy() {
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();
        let (_hex, peer) = stranger();
        let store = rt.ui.store_ref().unwrap();
        store
            .save_message(&plain_message("out1", &peer, "oops sent this", 1, true))
            .unwrap();

        // No relay is reachable in this test, so the courtesy notice to the
        // peer cannot possibly succeed — the local hide must not depend on it.
        rt.delete_message_for_everyone(&peer, "out1").await.unwrap();
        assert!(store.is_deleted_for_me(&peer, "out1").unwrap());
        assert!(rt.messages_with(&peer).unwrap().is_empty());
    }

    #[tokio::test]
    async fn forwarding_strips_markers_and_labels_the_copy_not_the_author() {
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();
        let (_hex_a, peer_a) = stranger();
        let (_hex_b, peer_b) = stranger();
        let store = rt.ui.store_ref().unwrap();
        store
            .save_message(&plain_message("m1", &peer_a, "look at this", 1, false))
            .unwrap();

        assert!(matches!(
            rt.forward_message(&peer_a, "m1", &[]).await,
            Err(UiError::Engine(_))
        ));
        assert!(rt
            .forward_message(&peer_a, "nope", std::slice::from_ref(&peer_b))
            .await
            .is_err());

        let sent = rt
            .forward_message(&peer_a, "m1", std::slice::from_ref(&peer_b))
            .await
            .unwrap();
        assert_eq!(sent.len(), 1);
        assert!(sent[0].forwarded, "a forward must say so");
        assert_eq!(
            sent[0].content, "look at this",
            "the words, not the wire marker, is what a bubble shows"
        );
        assert_eq!(sent[0].peer, peer_b);
        assert_eq!(
            sent[0].author,
            MessageAuthor::Human,
            "forwarding never claims to be the original sender, or anyone else"
        );
    }

    #[tokio::test]
    async fn a_link_preview_attached_by_the_sender_surfaces_on_the_message() {
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();
        let (_hex, peer) = stranger();
        let store = rt.ui.store_ref().unwrap();
        let preview = comrade_core::unfurl::LinkPreview {
            url: "https://example.com/a".into(),
            canonical_url: "https://example.com/a".into(),
            title: Some("A title".into()),
            description: None,
            site_name: None,
            image_url: None,
            kind: comrade_core::unfurl::PreviewKind::Article,
        };
        let body = comrade_core::unfurl::attach_preview("check this out", &preview);
        store
            .save_message(&plain_message("m1", &peer, &body, 1, false))
            .unwrap();

        let msgs = rt.messages_with(&peer).unwrap();
        assert_eq!(msgs[0].content, "check this out");
        let card = msgs[0]
            .link_preview
            .as_ref()
            .expect("preview should surface");
        assert_eq!(card.title.as_deref(), Some("A title"));
        assert_eq!(
            card.display_domain.as_deref(),
            Some("example.com"),
            "the domain shown must come from the URL, not the page's own claims"
        );
    }

    #[tokio::test]
    async fn compose_link_preview_needs_a_link_to_even_try() {
        // With no link at all, this must never guess or attempt a fetch —
        // true in every build, feature or no feature.
        assert!(compose_link_preview("no links here").await.is_none());
    }

    // Not exercised here on purpose: whether `compose_link_preview("see
    // https://…")` returns `Some` depends on `comrade_core`'s `unfurl-http`
    // feature and on a live fetch actually succeeding, and this crate has no
    // `cfg` that reliably tracks the former (`comrade_ui` does not itself
    // gate on that feature name, and the workspace's own verification command
    // — `--features comrade_core/unfurl-http` — does not flip one here either).
    // `unfurl.rs`'s own tests already pin the hermetic parts (`fetch_preview`
    // refuses non-HTTPS with no socket touched at all); a real-network
    // success case belongs there if it is ever worth hermetically stubbing,
    // the way `media.rs`'s upload tests stub plain HTTP (its own tests explain
    // why HTTPS-only enforcement rules that out for a download path).

    #[test]
    fn attach_link_preview_round_trips_through_split_preview() {
        let preview = LinkPreviewDto {
            url: "https://example.com".into(),
            canonical_url: "https://example.com".into(),
            title: Some("t".into()),
            description: None,
            site_name: None,
            kind: PreviewKindDto::Unknown,
            display_domain: Some("example.com".into()),
        };
        let body = attach_link_preview("hello", &preview);
        let (text, parsed) = comrade_core::unfurl::split_preview(&body);
        assert_eq!(text, "hello");
        assert_eq!(parsed.unwrap().title.as_deref(), Some("t"));
    }

    // ── Message requests, receipts, and calls ────────────────────────────────

    fn stranger() -> (String, String) {
        let pk = nostr_sdk::prelude::Keys::generate().public_key();
        (pk.to_hex(), pk.to_bech32().unwrap())
    }

    #[tokio::test]
    async fn strangers_are_gated_into_requests_then_accepted() {
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();
        let (_hex, peer) = stranger();
        let store = rt.ui.store_ref().unwrap();

        // Simulate the inbox loop recording a stranger's DM: pending + message.
        store
            .set_conversation_meta(&comrade_storage::ConversationMeta {
                peer_npub: peer.clone(),
                state: "pending".into(),
                profile_shared: false,
                last_read_at: 0,
                updated_at: 1,
            })
            .unwrap();
        store
            .save_message(&comrade_storage::StoredMessage {
                id: "in1".into(),
                peer_npub: peer.clone(),
                content: "hi, can we talk?".into(),
                created_at: 5,
                outgoing: false,
                status: None,
                reply_to: None,
            })
            .unwrap();

        // Gated out of the chat list; present as a request.
        assert!(rt.conversations().unwrap().is_empty());
        let reqs = rt.message_requests().unwrap();
        assert_eq!(reqs.len(), 1);
        assert_eq!(reqs[0].peer, peer);
        assert_eq!(reqs[0].last_message, "hi, can we talk?");

        // Accept → into the chat list, out of requests.
        rt.accept_request(&peer).unwrap();
        assert!(rt.message_requests().unwrap().is_empty());
        let convos = rt.conversations().unwrap();
        assert_eq!(convos.len(), 1);
        assert_eq!(convos[0].peer, peer);
    }

    #[tokio::test]
    async fn marking_read_reports_the_previous_position_and_watermarks_the_newest() {
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();
        let (_hex, peer) = stranger();
        let store = rt.ui.store_ref().unwrap();
        let save = |id: &str, at: u64| {
            store
                .save_message(&comrade_storage::StoredMessage {
                    id: id.into(),
                    peer_npub: peer.clone(),
                    content: "hi".into(),
                    created_at: at,
                    outgoing: false,
                    status: None,
                    reply_to: None,
                })
                .unwrap();
        };
        save("in1", 100);
        save("in2", 200);

        // First visit: nothing had been read, so the UI gets 0 and opens at the
        // newest message with no divider.
        assert_eq!(rt.mark_conversation_read(&peer).unwrap(), 0);
        assert_eq!(store.read_position(&peer).unwrap(), 200);

        // Two more arrive while away; re-opening reports where they had been,
        // which is what the divider is drawn from.
        save("in3", 300);
        save("in4", 400);
        assert_eq!(rt.mark_conversation_read(&peer).unwrap(), 200);
        assert_eq!(store.read_position(&peer).unwrap(), 400);

        // Re-opening with nothing new still reports the position, so a thread
        // with no unread messages simply opens at the bottom.
        assert_eq!(rt.mark_conversation_read(&peer).unwrap(), 400);
    }

    #[tokio::test]
    async fn marking_a_pending_request_read_records_nothing() {
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();
        let (_hex, peer) = stranger();
        let store = rt.ui.store_ref().unwrap();
        store
            .set_conversation_meta(&comrade_storage::ConversationMeta {
                peer_npub: peer.clone(),
                state: "pending".into(),
                profile_shared: false,
                last_read_at: 0,
                updated_at: 1,
            })
            .unwrap();
        store
            .save_message(&comrade_storage::StoredMessage {
                id: "in1".into(),
                peer_npub: peer.clone(),
                content: "hi, can we talk?".into(),
                created_at: 5,
                outgoing: false,
                status: None,
                reply_to: None,
            })
            .unwrap();

        // Not just "no receipt": no *stored* position either. Recording one
        // would be a local trace that we read a request before deciding on it,
        // and it would survive into the accepted thread.
        assert_eq!(rt.mark_conversation_read(&peer).unwrap(), 0);
        assert_eq!(store.read_position(&peer).unwrap(), 0);
        let meta = store.get_conversation_meta(&peer).unwrap().unwrap();
        assert_eq!(meta.state, "pending");
    }

    #[tokio::test]
    async fn blocked_peer_is_hidden_from_both_lists() {
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();
        let (_hex, peer) = stranger();
        rt.ui
            .store_ref()
            .unwrap()
            .save_message(&comrade_storage::StoredMessage {
                id: "in1".into(),
                peer_npub: peer.clone(),
                content: "spam".into(),
                created_at: 1,
                outgoing: false,
                status: None,
                reply_to: None,
            })
            .unwrap();
        rt.block_conversation(&peer).unwrap();
        assert!(rt.conversations().unwrap().is_empty());
        assert!(rt.message_requests().unwrap().is_empty());
    }

    /// Persist an attachment reference exactly as the send/receive paths do.
    fn save_media_ref(
        store: &comrade_storage::EncryptedStore,
        event_id: &str,
        peer_hex: &str,
        caption: &str,
        created_at: u64,
        outgoing: bool,
    ) {
        store
            .put(
                MEDIA_REFS_TREE,
                event_id,
                &MediaRef {
                    event_id: event_id.into(),
                    url: "https://blob.example/x".into(),
                    peer_pubkey: peer_hex.into(),
                    mime_type: "image/jpeg".into(),
                    caption: caption.into(),
                    size: 1234,
                    sha256_hex: "a".repeat(64),
                    outgoing,
                    created_at,
                },
            )
            .unwrap();
        store.flush().unwrap();
    }

    #[tokio::test]
    async fn a_thread_of_only_attachments_still_appears_in_the_chat_list() {
        // Media references are not stored as messages, so a chat list built from
        // messages alone made a photo-only conversation invisible — the thread
        // existed, the photo was in the store, and the list showed nothing.
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();
        let (hex, peer) = stranger();
        let store = rt.ui.store_ref().unwrap();
        accepted_peer(store, &peer, false);
        save_media_ref(store, "m1", &hex, "sunset.jpg", 100, false);

        let convos = rt.conversations().unwrap();
        assert_eq!(convos.len(), 1);
        assert_eq!(convos[0].peer, peer);
        assert_eq!(convos[0].last_message, "📎 sunset.jpg");
        assert_eq!(convos[0].last_at, 100);
        assert!(!convos[0].last_outgoing);
    }

    #[tokio::test]
    async fn the_chat_list_preview_follows_whichever_is_newer() {
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();
        let (hex, peer) = stranger();
        let store = rt.ui.store_ref().unwrap();
        accepted_peer(store, &peer, false);
        store
            .save_message(&comrade_storage::StoredMessage {
                id: "in1".into(),
                peer_npub: peer.clone(),
                content: "look at this".into(),
                created_at: 100,
                outgoing: false,
                status: None,
                reply_to: None,
            })
            .unwrap();

        // Older attachment: the text still wins.
        save_media_ref(store, "m1", &hex, "old.jpg", 50, false);
        assert_eq!(rt.conversations().unwrap()[0].last_message, "look at this");

        // Newer attachment, sent by us: the row becomes the attachment, and
        // `last_outgoing` follows it (the list renders a "You: " prefix from it).
        save_media_ref(store, "m2", &hex, "", 150, true);
        let convo = &rt.conversations().unwrap()[0];
        assert_eq!(convo.last_message, "📎 Attachment");
        assert_eq!(convo.last_at, 150);
        assert!(convo.last_outgoing);
    }

    #[tokio::test]
    async fn a_stranger_whose_only_contact_was_an_attachment_is_a_previewable_request() {
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();
        let (hex, peer) = stranger();
        let store = rt.ui.store_ref().unwrap();
        store
            .set_conversation_meta(&comrade_storage::ConversationMeta {
                peer_npub: peer.clone(),
                state: STATE_PENDING.into(),
                profile_shared: false,
                last_read_at: 0,
                updated_at: 1,
            })
            .unwrap();
        save_media_ref(store, "m1", &hex, "receipt.pdf", 42, false);

        // Gated out of the chat list...
        assert!(rt.conversations().unwrap().is_empty());
        // ...and a request with a real preview rather than an empty line.
        let reqs = rt.message_requests().unwrap();
        assert_eq!(reqs.len(), 1);
        assert_eq!(reqs[0].peer, peer);
        assert_eq!(reqs[0].last_message, "📎 receipt.pdf");
        assert_eq!(reqs[0].last_at, 42);
    }

    #[tokio::test]
    async fn sending_media_refuses_an_empty_or_unusable_payload_before_uploading() {
        // These must fail *before* the network: the alternative is paying for an
        // upload and putting an undecodable bubble in both people's threads.
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();
        let (_hex, peer) = stranger();

        let empty = rt
            .upload_and_send_media(&peer, vec![], "image/png", "")
            .await;
        assert!(matches!(empty, Err(UiError::Engine(_))), "empty payload");

        let no_mime = rt
            .upload_and_send_media(&peer, vec![1, 2, 3], "  ", "")
            .await;
        assert!(matches!(no_mime, Err(UiError::Engine(_))), "blank MIME");

        let oversized = rt
            .upload_and_send_media(&peer, vec![0u8; MAX_MEDIA_BYTES + 1], "image/png", "")
            .await;
        assert!(matches!(oversized, Err(UiError::Engine(_))), "over the cap");
    }

    #[tokio::test]
    async fn incoming_media_is_sanitised_before_it_is_stored_or_surfaced() {
        let dir = TempDir::new().unwrap();
        let store = Arc::new(comrade_storage::EncryptedStore::open(dir.path(), "pin").unwrap());
        let vault = test_vault().await;
        let (tx, mut rx) = broadcast::channel(16);
        let dedup = SeenSet::new(CALL_SIGNAL_DEDUP_CAPACITY);
        let outbox = Arc::new(Outbox::new());
        let transport_dedup = SeenSet::with_ttl(
            CROSS_TRANSPORT_DEDUP_CAPACITY,
            std::time::Duration::from_secs(CROSS_TRANSPORT_DEDUP_SECS),
        );
        let route = relay_route(&transport_dedup);
        let (hex, peer) = stranger();
        accepted_peer(&store, &peer, false);

        let envelope = serde_json::to_string(&MediaEnvelope {
            comrade_media: 1,
            event_id: "m1".into(),
            url: "https://blob.example/x".into(),
            // A peer can claim anything; the type decides which renderer every
            // frontend reaches for.
            mime: "IMAGE/JPEG".into(),
            caption: "holiday\r\nDelivered ✓✓".into(),
            size: 10,
            sha256_hex: "a".repeat(64),
        })
        .unwrap();
        dispatch_incoming_dm(
            &vault,
            Some(&store),
            &tx,
            &dedup,
            &outbox,
            &route,
            incoming(&hex, "e1", &envelope),
        );

        let stored: MediaRef = store.get(MEDIA_REFS_TREE, "m1").unwrap().unwrap();
        assert_eq!(stored.mime_type, "image/jpeg");
        assert_eq!(stored.caption, "holidayDelivered ✓✓");
        match rx.try_recv().unwrap() {
            BridgeEvent::IncomingMedia(m) => {
                assert_eq!(m.mime_type, "image/jpeg");
                assert_eq!(m.caption, "holidayDelivered ✓✓");
                assert_eq!(m.sender, peer);
                assert!(!m.outgoing);
            }
            other => panic!("expected IncomingMedia, got {other:?}"),
        }

        // An unusable MIME type downgrades the renderer; it never drops the
        // attachment (the blob still downloads and opens).
        let odd = serde_json::to_string(&MediaEnvelope {
            comrade_media: 1,
            event_id: "m2".into(),
            url: "https://blob.example/y".into(),
            mime: "not a mime type".into(),
            caption: String::new(),
            size: 10,
            sha256_hex: String::new(),
        })
        .unwrap();
        dispatch_incoming_dm(
            &vault,
            Some(&store),
            &tx,
            &dedup,
            &outbox,
            &route,
            incoming(&hex, "e2", &odd),
        );
        let stored: MediaRef = store.get(MEDIA_REFS_TREE, "m2").unwrap().unwrap();
        assert_eq!(stored.mime_type, DEFAULT_MIME);
    }

    /// A delete request may retract only what its own sender wrote.
    ///
    /// Without the authorship check in `dispatch_incoming_dm` this passes the
    /// peer's `target_id` straight to `delete_message_for_me`, and a peer can
    /// name a message *we* sent — they know its id, we sent it to them — and
    /// erase it from our own transcript. A tombstone renders as absence, so the
    /// theft leaves nothing behind to notice.
    #[tokio::test]
    async fn a_delete_request_cannot_retract_a_message_its_sender_did_not_write() {
        let dir = TempDir::new().unwrap();
        let store = Arc::new(comrade_storage::EncryptedStore::open(dir.path(), "pin").unwrap());
        let vault = test_vault().await;
        let (tx, _rx) = broadcast::channel(16);
        let dedup = SeenSet::new(CALL_SIGNAL_DEDUP_CAPACITY);
        let outbox = Arc::new(Outbox::new());
        let transport_dedup = SeenSet::with_ttl(
            CROSS_TRANSPORT_DEDUP_CAPACITY,
            std::time::Duration::from_secs(CROSS_TRANSPORT_DEDUP_SECS),
        );
        let route = relay_route(&transport_dedup);
        let (hex, peer) = stranger();
        accepted_peer(&store, &peer, false);

        // One message we sent, one they sent, in the same conversation.
        for (id, outgoing) in [("ours", true), ("theirs", false)] {
            store
                .save_message(&comrade_storage::StoredMessage {
                    id: id.into(),
                    peer_npub: peer.clone(),
                    content: "hello".into(),
                    created_at: 1_700_000_000,
                    outgoing,
                    status: None,
                    reply_to: None,
                })
                .unwrap();
        }

        let ask = |target: &str| serde_json::to_string(&DeleteRequest::new(target)).unwrap();

        // Ours: refused. This is the whole point of the check.
        dispatch_incoming_dm(
            &vault,
            Some(&store),
            &tx,
            &dedup,
            &outbox,
            &route,
            incoming(&hex, "d1", &ask("ours")),
        );
        assert!(
            !store.is_deleted_for_me(&peer, "ours").unwrap(),
            "a peer retracted a message we wrote",
        );

        // A message we have never cached: also refused, rather than tombstoned
        // pre-emptively against whatever later arrives under that id.
        dispatch_incoming_dm(
            &vault,
            Some(&store),
            &tx,
            &dedup,
            &outbox,
            &route,
            incoming(&hex, "d2", &ask("unknown")),
        );
        assert!(!store.is_deleted_for_me(&peer, "unknown").unwrap());

        // Theirs: honoured, so the check refuses the attack without breaking
        // the feature it guards.
        dispatch_incoming_dm(
            &vault,
            Some(&store),
            &tx,
            &dedup,
            &outbox,
            &route,
            incoming(&hex, "d3", &ask("theirs")),
        );
        assert!(
            store.is_deleted_for_me(&peer, "theirs").unwrap(),
            "a peer could not retract their own message",
        );
    }

    #[tokio::test]
    async fn call_log_roundtrips_per_peer_and_globally() {
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();
        let (_hex, peer) = stranger();
        let rec = rt
            .log_call(&peer, "call1", "video", true, "connected", 100, 42)
            .unwrap();
        assert_eq!(rec.media, "video");
        assert_eq!(rec.outcome, "connected");
        assert_eq!(rec.duration_secs, 42);
        assert_eq!(rt.call_history(Some(&peer)).unwrap().len(), 1);
        assert_eq!(rt.call_history(None).unwrap().len(), 1);
        // Unknown media string is coerced to audio.
        let rec2 = rt
            .log_call(&peer, "call2", "hologram", false, "missed", 0, 0)
            .unwrap();
        assert_eq!(rec2.media, "audio");
    }

    #[tokio::test]
    async fn ice_servers_default_stun_and_configurable_turn() {
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();
        let defaults = rt.call_ice_servers();
        assert!(!defaults.is_empty());
        assert!(defaults.iter().all(|s| s.username.is_none()));
        rt.set_turn_server("turn:turn.example.com:3478", "u", "p")
            .unwrap();
        let with_turn = rt.call_ice_servers();
        assert_eq!(with_turn.len(), defaults.len() + 1);
        assert_eq!(with_turn.last().unwrap().username.as_deref(), Some("u"));
        rt.set_turn_server("", "", "").unwrap();
        assert_eq!(rt.call_ice_servers().len(), defaults.len());
    }

    #[tokio::test]
    async fn set_turn_server_rejects_a_malformed_url_and_does_not_persist_it() {
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();

        let err = rt.set_turn_server("not-a-turn-url", "u", "p");
        assert!(matches!(err, Err(UiError::Engine(_))));
        assert_eq!(
            rt.turn_server_status(),
            TurnServerStatusDto {
                configured: false,
                url: None
            },
            "a rejected URL must never be persisted"
        );
    }

    #[tokio::test]
    async fn turn_server_status_reports_url_but_never_the_credential() {
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();

        // Pre-unlock: an honest "nothing configured", not an error.
        assert_eq!(
            rt.turn_server_status(),
            TurnServerStatusDto {
                configured: false,
                url: None
            }
        );

        rt.unlock_vault(dir.path(), "pin").await.unwrap();
        rt.set_turn_server("turn:turn.example.com:3478", "u", "top-secret")
            .unwrap();
        let status = rt.turn_server_status();
        assert!(status.configured);
        assert_eq!(status.url.as_deref(), Some("turn:turn.example.com:3478"));
        // `TurnServerStatusDto` structurally has no credential field to leak —
        // this asserts the serialised form as a belt-and-suspenders check
        // that never regresses into adding one back.
        let json = serde_json::to_string(&status).unwrap();
        assert!(!json.contains("top-secret"));
        assert!(!json.contains("\"u\""));

        rt.set_turn_server("", "", "").unwrap();
        assert!(!rt.turn_server_status().configured);
    }

    #[tokio::test]
    async fn call_ice_servers_for_stun_only_never_leaks_turn_credentials() {
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();
        rt.set_turn_server("turn:turn.example.com:3478", "u", "p")
            .unwrap();

        // Even with a TURN relay configured, an explicit stun_only ask (what
        // place_call uses) must never include it.
        let stun_only = rt.call_ice_servers_for("stun_only");
        assert!(stun_only.iter().all(|s| s.username.is_none()));

        // The fallback a frontend calls after ICE fails to connect does
        // include it.
        let fallback = rt.call_ice_servers_for("stun_and_turn");
        assert_eq!(fallback.last().unwrap().username.as_deref(), Some("u"));
        assert_eq!(fallback.len(), stun_only.len() + 1);

        // Garbage input defaults to the private stun_only behavior.
        assert_eq!(rt.call_ice_servers_for("nonsense").len(), stun_only.len());
    }

    #[test]
    fn call_sas_delegates_to_comrade_core_and_needs_no_vault() {
        // Pure computation over the two SDP strings already in hand — unlike
        // the ICE-server methods above, it touches no store, so a bare
        // `ComradeRuntime::new()` (never unlocked) must work.
        let rt = ComradeRuntime::new();
        let sdp_a = "v=0\r\na=fingerprint:sha-256 AA:BB:CC:DD\r\n";
        let sdp_b = "v=0\r\na=fingerprint:sha-256 11:22:33:44\r\n";

        let sas = rt
            .call_sas(sdp_a, sdp_b)
            .expect("both sides have a fingerprint");
        assert_eq!(sas.len(), 4);
        // Same property `derive_sas` itself guarantees — checked again here so
        // a future refactor that swaps the delegation's argument order (the
        // one mistake that would silently break verification) fails a test.
        assert_eq!(rt.call_sas(sdp_a, sdp_b), rt.call_sas(sdp_b, sdp_a));

        let no_fingerprint = "v=0\r\no=- 1 1 IN IP4 0.0.0.0\r\n";
        assert_eq!(rt.call_sas(sdp_a, no_fingerprint), None);
    }

    #[tokio::test]
    async fn place_call_starts_stun_only_even_with_turn_configured() {
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();
        rt.set_turn_server("turn:turn.example.com:3478", "u", "p")
            .unwrap();

        let (_hex, peer) = stranger();
        let session = rt.place_call(&peer, "audio").unwrap();
        assert!(
            session.ice_servers.iter().all(|s| s.username.is_none()),
            "the initial offer must not contact the TURN relay unless STUN fails"
        );
    }

    #[tokio::test]
    async fn place_call_mints_session_and_validates() {
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        assert!(matches!(
            rt.place_call("npub1x", "audio"),
            Err(UiError::VaultLocked)
        ));
        rt.unlock_vault(dir.path(), "pin").await.unwrap();
        let (_hex, peer) = stranger();
        let session = rt.place_call(&peer, "video").unwrap();
        assert_eq!(session.peer, peer);
        assert_eq!(session.media, "video");
        assert_eq!(session.call_id.len(), 32);
        assert!(!session.ice_servers.is_empty());
        assert!(rt.place_call("not-a-key", "audio").is_err());
    }

    #[tokio::test]
    async fn send_call_signal_rejects_malformed_json() {
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();
        let (_hex, peer) = stranger();
        let err = rt
            .send_call_signal(&peer, "c1", "audio", "{not valid")
            .await;
        assert!(matches!(err, Err(UiError::Engine(_))));
    }

    async fn test_vault() -> Arc<VaultEngine> {
        let keys = nostr_sdk::prelude::Keys::generate();
        Arc::new(
            VaultEngine::new(&keys, vec!["wss://relay.damus.io".into()])
                .await
                .unwrap(),
        )
    }

    /// A relay-delivered route with no mesh — what most ingress tests want.
    fn relay_route(dedup: &SeenSet) -> DmRoute<'_> {
        DmRoute {
            label: TRANSPORT_RELAY,
            dedup,
            mesh: None,
            together: None,
        }
    }

    fn incoming(sender_hex: &str, event_id: &str, content: &str) -> VaultMessage {
        VaultMessage {
            event_id: event_id.into(),
            sender_pubkey: sender_hex.into(),
            content: content.into(),
            created_at: 3,
            upi_intents: vec![],
            reply_to: None,
        }
    }

    #[tokio::test]
    async fn dispatch_gates_unknown_sender_as_request() {
        let dir = TempDir::new().unwrap();
        let store = Arc::new(comrade_storage::EncryptedStore::open(dir.path(), "pin").unwrap());
        let vault = test_vault().await;
        let (tx, mut rx) = broadcast::channel(16);
        let dedup = SeenSet::new(CALL_SIGNAL_DEDUP_CAPACITY);
        let outbox = Arc::new(Outbox::new());
        let transport_dedup = SeenSet::with_ttl(
            CROSS_TRANSPORT_DEDUP_CAPACITY,
            std::time::Duration::from_secs(CROSS_TRANSPORT_DEDUP_SECS),
        );
        let route = relay_route(&transport_dedup);
        let (hex, peer) = stranger();

        dispatch_incoming_dm(
            &vault,
            Some(&store),
            &tx,
            &dedup,
            &outbox,
            &route,
            incoming(&hex, "e1", "hello?"),
        );

        assert_eq!(
            store.get_conversation_meta(&peer).unwrap().unwrap().state,
            "pending"
        );
        assert_eq!(store.messages_with(&peer).unwrap().len(), 1);
        match rx.try_recv().unwrap() {
            BridgeEvent::IncomingMessageRequest(r) => {
                assert_eq!(r.peer, peer);
                assert_eq!(r.last_message, "hello?");
            }
            other => panic!("expected request, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_delivers_accepted_and_advances_receipts() {
        let dir = TempDir::new().unwrap();
        let store = Arc::new(comrade_storage::EncryptedStore::open(dir.path(), "pin").unwrap());
        let vault = test_vault().await;
        let (tx, mut rx) = broadcast::channel(16);
        let dedup = SeenSet::new(CALL_SIGNAL_DEDUP_CAPACITY);
        let outbox = Arc::new(Outbox::new());
        let transport_dedup = SeenSet::with_ttl(
            CROSS_TRANSPORT_DEDUP_CAPACITY,
            std::time::Duration::from_secs(CROSS_TRANSPORT_DEDUP_SECS),
        );
        let route = relay_route(&transport_dedup);
        let (hex, peer) = stranger();
        store
            .set_conversation_meta(&comrade_storage::ConversationMeta {
                peer_npub: peer.clone(),
                state: "accepted".into(),
                profile_shared: true,
                last_read_at: 0,
                updated_at: 1,
            })
            .unwrap();

        // Plain text from an accepted peer is delivered (not gated).
        dispatch_incoming_dm(
            &vault,
            Some(&store),
            &tx,
            &dedup,
            &outbox,
            &route,
            incoming(&hex, "e1", "yo"),
        );
        assert!(matches!(
            rx.try_recv().unwrap(),
            BridgeEvent::IncomingDirectMessage(_)
        ));

        // A read receipt advances one of our outgoing messages.
        store
            .save_message(&comrade_storage::StoredMessage {
                id: "out1".into(),
                peer_npub: peer.clone(),
                content: "sup".into(),
                created_at: 2,
                outgoing: true,
                status: Some("sent".into()),
                reply_to: None,
            })
            .unwrap();
        let receipt = Receipt::new(ReceiptKind::Read, vec!["out1".into()])
            .to_json()
            .unwrap();
        dispatch_incoming_dm(
            &vault,
            Some(&store),
            &tx,
            &dedup,
            &outbox,
            &route,
            incoming(&hex, "e2", &receipt),
        );
        assert_eq!(
            store
                .get_message("out1")
                .unwrap()
                .unwrap()
                .status
                .as_deref(),
            Some("read")
        );
        match rx.try_recv().unwrap() {
            BridgeEvent::MessageStatus {
                message_ids,
                status,
                ..
            } => {
                assert_eq!(status, "read");
                assert_eq!(message_ids, vec!["out1".to_string()]);
            }
            other => panic!("expected MessageStatus, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_drops_blocked_and_caches_profile_share() {
        let dir = TempDir::new().unwrap();
        let store = Arc::new(comrade_storage::EncryptedStore::open(dir.path(), "pin").unwrap());
        let vault = test_vault().await;
        let (tx, mut rx) = broadcast::channel(16);
        let dedup = SeenSet::new(CALL_SIGNAL_DEDUP_CAPACITY);
        let outbox = Arc::new(Outbox::new());
        let transport_dedup = SeenSet::with_ttl(
            CROSS_TRANSPORT_DEDUP_CAPACITY,
            std::time::Duration::from_secs(CROSS_TRANSPORT_DEDUP_SECS),
        );
        let route = relay_route(&transport_dedup);
        let (hex, peer) = stranger();
        store
            .set_conversation_meta(&comrade_storage::ConversationMeta {
                peer_npub: peer.clone(),
                state: "blocked".into(),
                profile_shared: false,
                last_read_at: 0,
                updated_at: 1,
            })
            .unwrap();
        dispatch_incoming_dm(
            &vault,
            Some(&store),
            &tx,
            &dedup,
            &outbox,
            &route,
            incoming(&hex, "e1", "let me in"),
        );
        assert!(store.messages_with(&peer).unwrap().is_empty());
        assert!(rx.try_recv().is_err(), "blocked peer emits nothing");

        // A profile share (any non-blocked peer) caches the name + emits update.
        let (other_hex, other_npub) = stranger();
        let share = ProfileShare::new(Some("charlie".into())).to_json().unwrap();
        dispatch_incoming_dm(
            &vault,
            Some(&store),
            &tx,
            &dedup,
            &outbox,
            &route,
            incoming(&other_hex, "e2", &share),
        );
        match rx.try_recv().unwrap() {
            BridgeEvent::PeerProfileUpdated { peer, name } => {
                assert_eq!(peer, other_npub);
                assert_eq!(name.as_deref(), Some("charlie"));
            }
            other => panic!("expected PeerProfileUpdated, got {other:?}"),
        }
    }

    // ── Comrade presence ─────────────────────────────────────────────────────

    /// An incoming DM stamped with a caller-chosen send time — presence is
    /// all about freshness, so unlike [`incoming`] (fixed at epoch+3, which
    /// every beacon would read as long expired) these tests set it per case.
    fn incoming_at(
        sender_hex: &str,
        event_id: &str,
        content: &str,
        created_at: u64,
    ) -> VaultMessage {
        VaultMessage {
            created_at,
            ..incoming(sender_hex, event_id, content)
        }
    }

    /// A store with `peer` as an accepted conversation, optionally marked as
    /// one of our comrades — the two axes every presence rule turns on.
    fn accepted_peer(store: &comrade_storage::EncryptedStore, peer: &str, our_comrade: bool) {
        store
            .set_conversation_meta(&comrade_storage::ConversationMeta {
                peer_npub: peer.to_string(),
                state: "accepted".into(),
                profile_shared: true,
                last_read_at: 0,
                updated_at: 1,
            })
            .unwrap();
        if our_comrade {
            store.set_contact_comrade(peer, true).unwrap();
        }
    }

    #[tokio::test]
    async fn choosing_a_comrade_is_opt_in_reversible_and_visible_everywhere() {
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        assert!(matches!(rt.comrades(), Err(UiError::VaultLocked)));
        assert!(matches!(
            rt.set_comrade("npub1x", true),
            Err(UiError::VaultLocked)
        ));

        rt.unlock_vault(dir.path(), "pin").await.unwrap();
        let (_hex, peer) = stranger();
        assert!(
            rt.comrades().unwrap().is_empty(),
            "nobody is chosen by default"
        );

        // Marking works straight from a conversation — no prior contact row.
        let marked = rt.set_comrade(&peer, true).unwrap();
        assert!(marked.comrade);
        let comrades = rt.comrades().unwrap();
        assert_eq!(comrades.len(), 1);
        assert_eq!(comrades[0].npub, peer);
        assert!(!comrades[0].online, "no beacon yet ⇒ not online");
        assert_eq!(comrades[0].last_seen_at, 0);
        assert!(
            !comrades[0].peer_marked_us,
            "choosing someone says nothing about whether they chose us"
        );
        assert!(rt.list_contacts().unwrap()[0].comrade);

        // Editing the alias must not silently un-choose them.
        rt.set_contact_alias(&peer, "Ana").unwrap();
        assert!(rt.list_contacts().unwrap()[0].comrade);
        assert_eq!(rt.comrades().unwrap()[0].alias, "Ana");

        // …and un-choosing leaves the contact (and its alias) intact.
        let unmarked = rt.set_comrade(&peer, false).unwrap();
        assert!(!unmarked.comrade);
        assert!(rt.comrades().unwrap().is_empty());
        assert_eq!(rt.list_contacts().unwrap()[0].alias, "Ana");

        assert!(matches!(
            rt.set_comrade("junk", true),
            Err(UiError::Engine(_))
        ));
        assert!(matches!(rt.peer_presence("junk"), Err(UiError::Engine(_))));
    }

    #[tokio::test]
    async fn presence_follows_the_app_being_open_not_the_process_being_alive() {
        // "Online" is a claim about the person, not the process: a phone in a
        // pocket with a connection service running is reachable, but nobody
        // is at it. The heartbeat must therefore refresh only while the
        // frontend says the app is open — otherwise it would undo every
        // goodbye a backgrounded app sends.
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();
        let (_hex, peer) = stranger();
        rt.set_comrade(&peer, true).unwrap();

        assert!(
            rt.is_presence_active(),
            "a frontend that never says otherwise (desktop, CLI) is simply open"
        );

        // Backgrounded: the flag flips, and the heartbeat has nothing to say.
        rt.announce_presence(false).await;
        assert!(!rt.is_presence_active());
        assert_eq!(
            rt.handles().refresh_presence().await,
            0,
            "a backgrounded app must not re-announce itself online"
        );

        // Foregrounded again: the heartbeat resumes. (Both sends fail here —
        // there is no relay — so the count is 0 either way; what this pins is
        // the gate, which the two-peer suite then exercises for real.)
        rt.announce_presence(true).await;
        assert!(rt.is_presence_active());

        // Locking is its own kind of "not online", whatever the app was doing.
        rt.lock_vault().await;
        assert!(!rt.is_presence_active());

        // …and unlocking again is someone standing at the app, so presence
        // resumes. (Without this the heartbeat would stay silent for the rest
        // of the process's life after any lock/unlock cycle.)
        rt.unlock_vault(dir.path(), "pin").await.unwrap();
        assert!(rt.is_presence_active());
    }

    #[tokio::test]
    async fn a_presence_event_carries_a_usable_name_or_none_at_all() {
        // The name rides the event so a frontend can title a notification
        // without a store round-trip — which makes a blank one worse than
        // useless ("  is online"). Alias wins, a published handle is next,
        // and anything whitespace-only is no name at all.
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();
        let (_hex, peer) = stranger();
        let store = rt.ui.store_arc().unwrap();

        assert_eq!(presence_display_name(&store, &peer), None, "no contact yet");

        rt.set_comrade(&peer, true).unwrap();
        assert_eq!(
            presence_display_name(&store, &peer),
            None,
            "choosing someone doesn't invent a name for them"
        );

        store.set_contact_comrade(&peer, true).unwrap();
        rt.set_contact_alias(&peer, "   ").unwrap();
        assert_eq!(
            presence_display_name(&store, &peer),
            None,
            "a whitespace alias is not a name"
        );

        rt.set_contact_alias(&peer, "  Ana  ").unwrap();
        assert_eq!(presence_display_name(&store, &peer).as_deref(), Some("Ana"));
    }

    #[tokio::test]
    async fn a_comrade_coming_online_is_announced_once_and_a_heartbeat_is_not() {
        let dir = TempDir::new().unwrap();
        let store = Arc::new(comrade_storage::EncryptedStore::open(dir.path(), "pin").unwrap());
        let vault = test_vault().await;
        let (tx, mut rx) = broadcast::channel(16);
        let dedup = SeenSet::new(CALL_SIGNAL_DEDUP_CAPACITY);
        let outbox = Arc::new(Outbox::new());
        let transport_dedup = SeenSet::with_ttl(
            CROSS_TRANSPORT_DEDUP_CAPACITY,
            std::time::Duration::from_secs(CROSS_TRANSPORT_DEDUP_SECS),
        );
        let route = relay_route(&transport_dedup);
        let (hex, peer) = stranger();
        accepted_peer(&store, &peer, true);
        let now = now_secs();

        let beacon = PresenceBeacon::online().to_json().unwrap();
        dispatch_incoming_dm(
            &vault,
            Some(&store),
            &tx,
            &dedup,
            &outbox,
            &route,
            incoming_at(&hex, "e1", &beacon, now),
        );
        match rx.try_recv().unwrap() {
            BridgeEvent::ComradePresence {
                peer: p,
                online,
                at,
                ..
            } => {
                assert_eq!(p, peer);
                assert!(online);
                assert_eq!(at, now);
            }
            other => panic!("expected ComradePresence, got {other:?}"),
        }
        // A beacon is never a chat message, a request, or a stored DM.
        assert!(store.messages_with(&peer).unwrap().is_empty());

        // The heartbeat that follows is state, not news.
        dispatch_incoming_dm(
            &vault,
            Some(&store),
            &tx,
            &dedup,
            &outbox,
            &route,
            incoming_at(&hex, "e2", &beacon, now + 1),
        );
        assert!(
            rx.try_recv().is_err(),
            "a heartbeat from someone already online must not re-notify"
        );

        // Going offline is a transition, so it is announced.
        let bye = PresenceBeacon::offline().to_json().unwrap();
        dispatch_incoming_dm(
            &vault,
            Some(&store),
            &tx,
            &dedup,
            &outbox,
            &route,
            incoming_at(&hex, "e3", &bye, now + 2),
        );
        match rx.try_recv().unwrap() {
            BridgeEvent::ComradePresence { online, .. } => assert!(!online),
            other => panic!("expected an offline ComradePresence, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_replayed_or_out_of_order_beacon_never_resurrects_a_green_dot() {
        let dir = TempDir::new().unwrap();
        let store = Arc::new(comrade_storage::EncryptedStore::open(dir.path(), "pin").unwrap());
        let vault = test_vault().await;
        let (tx, mut rx) = broadcast::channel(16);
        let dedup = SeenSet::new(CALL_SIGNAL_DEDUP_CAPACITY);
        let outbox = Arc::new(Outbox::new());
        let transport_dedup = SeenSet::with_ttl(
            CROSS_TRANSPORT_DEDUP_CAPACITY,
            std::time::Duration::from_secs(CROSS_TRANSPORT_DEDUP_SECS),
        );
        let route = relay_route(&transport_dedup);
        let (hex, peer) = stranger();
        accepted_peer(&store, &peer, true);
        let now = now_secs();
        let beacon = PresenceBeacon::online().to_json().unwrap();

        // The inbox backfills up to two days on every launch; that replay
        // must not claim someone is online right now.
        dispatch_incoming_dm(
            &vault,
            Some(&store),
            &tx,
            &dedup,
            &outbox,
            &route,
            incoming_at(&hex, "old", &beacon, now - 2 * 24 * 60 * 60),
        );
        assert!(rx.try_recv().is_err(), "a stale beacon emits nothing");
        assert!(store.get_peer_presence(&peer).unwrap().is_none());

        // A fresh one does, and a late-arriving *older* beacon can't undo it.
        dispatch_incoming_dm(
            &vault,
            Some(&store),
            &tx,
            &dedup,
            &outbox,
            &route,
            incoming_at(&hex, "new", &beacon, now),
        );
        assert!(matches!(
            rx.try_recv().unwrap(),
            BridgeEvent::ComradePresence { online: true, .. }
        ));
        let stale_bye = PresenceBeacon::offline().to_json().unwrap();
        dispatch_incoming_dm(
            &vault,
            Some(&store),
            &tx,
            &dedup,
            &outbox,
            &route,
            incoming_at(&hex, "late-bye", &stale_bye, now - 60),
        );
        assert!(
            rx.try_recv().is_err(),
            "an out-of-order goodbye must not rewind fresher state"
        );
        assert!(store.get_peer_presence(&peer).unwrap().unwrap().online);
    }

    #[tokio::test]
    async fn presence_from_a_stranger_is_dropped_and_from_a_non_comrade_is_silent() {
        let dir = TempDir::new().unwrap();
        let store = Arc::new(comrade_storage::EncryptedStore::open(dir.path(), "pin").unwrap());
        let vault = test_vault().await;
        let (tx, mut rx) = broadcast::channel(16);
        let dedup = SeenSet::new(CALL_SIGNAL_DEDUP_CAPACITY);
        let outbox = Arc::new(Outbox::new());
        let transport_dedup = SeenSet::with_ttl(
            CROSS_TRANSPORT_DEDUP_CAPACITY,
            std::time::Duration::from_secs(CROSS_TRANSPORT_DEDUP_SECS),
        );
        let route = relay_route(&transport_dedup);
        let now = now_secs();
        let beacon = PresenceBeacon::online().to_json().unwrap();

        // An unaccepted stranger cannot push presence state at us — and their
        // beacon must not leak into the requests bucket as raw JSON either.
        let (stranger_hex, stranger_npub) = stranger();
        dispatch_incoming_dm(
            &vault,
            Some(&store),
            &tx,
            &dedup,
            &outbox,
            &route,
            incoming_at(&stranger_hex, "s1", &beacon, now),
        );
        assert!(rx.try_recv().is_err());
        assert!(store.get_peer_presence(&stranger_npub).unwrap().is_none());
        assert!(store.messages_with(&stranger_npub).unwrap().is_empty());
        assert!(store
            .get_conversation_meta(&stranger_npub)
            .unwrap()
            .is_none());

        // An accepted peer we have *not* chosen: recorded (it proves they
        // chose us — the reciprocity hint) but never announced.
        let (hex, peer) = stranger();
        accepted_peer(&store, &peer, false);
        dispatch_incoming_dm(
            &vault,
            Some(&store),
            &tx,
            &dedup,
            &outbox,
            &route,
            incoming_at(&hex, "a1", &beacon, now),
        );
        assert!(
            rx.try_recv().is_err(),
            "presence for someone we didn't choose is not news"
        );
        let recorded = store.get_peer_presence(&peer).unwrap().unwrap();
        assert!(recorded.online);
        assert!(recorded.peer_marked_us);
    }

    /// Everything `dispatch_incoming_dm` needs, for the nudge cases below —
    /// the presence tests above spell the same set out inline, which is
    /// exactly why the fourth copy became a helper.
    struct Ingress {
        vault: Arc<VaultEngine>,
        store: Arc<comrade_storage::EncryptedStore>,
        dedup: SeenSet,
        outbox: Arc<Outbox>,
        transport_dedup: SeenSet,
        events: broadcast::Sender<BridgeEvent>,
    }

    impl Ingress {
        async fn new(dir: &TempDir) -> (Self, broadcast::Receiver<BridgeEvent>) {
            let (events, rx) = broadcast::channel(16);
            let ingress = Self {
                vault: test_vault().await,
                store: Arc::new(comrade_storage::EncryptedStore::open(dir.path(), "pin").unwrap()),
                dedup: SeenSet::new(CALL_SIGNAL_DEDUP_CAPACITY),
                outbox: Arc::new(Outbox::new()),
                transport_dedup: SeenSet::with_ttl(
                    CROSS_TRANSPORT_DEDUP_CAPACITY,
                    std::time::Duration::from_secs(CROSS_TRANSPORT_DEDUP_SECS),
                ),
                events,
            };
            (ingress, rx)
        }

        fn deliver(&self, sender_hex: &str, event_id: &str, content: &str, created_at: u64) {
            dispatch_incoming_dm(
                &self.vault,
                Some(&self.store),
                &self.events,
                &self.dedup,
                &self.outbox,
                &relay_route(&self.transport_dedup),
                incoming_at(sender_hex, event_id, content, created_at),
            );
        }
    }

    #[tokio::test]
    async fn a_peers_reaction_lands_on_the_message_it_names() {
        let dir = TempDir::new().unwrap();
        let (ingress, mut rx) = Ingress::new(&dir).await;
        let (hex, peer) = stranger();
        accepted_peer(&ingress.store, &peer, false);
        let now = now_secs();

        let react = ReactionEnvelope::new("m1", "🔥").to_json().unwrap();
        ingress.deliver(&hex, "r1", &react, now);
        match rx.try_recv().unwrap() {
            BridgeEvent::IncomingReaction(r) => {
                assert_eq!(r.target_id, "m1");
                assert_eq!(r.emoji, "🔥");
                assert_eq!(r.peer, peer);
                assert_eq!(r.reactor, peer);
                assert!(!r.outgoing);
            }
            other => panic!("expected IncomingReaction, got {other:?}"),
        }

        // A reaction is a control envelope, never a chat bubble. Before the
        // parser existed it would have fallen through to the plain-text branch
        // and rendered as a message full of JSON.
        assert!(
            ingress.store.messages_with(&peer).unwrap().is_empty(),
            "a reaction must not become a message"
        );
        assert_eq!(ingress.store.reactions_with(&peer).unwrap().len(), 1);

        // Changing it replaces rather than stacks…
        ingress.deliver(
            &hex,
            "r2",
            &ReactionEnvelope::new("m1", "👍").to_json().unwrap(),
            now + 1,
        );
        assert!(matches!(
            rx.try_recv().unwrap(),
            BridgeEvent::IncomingReaction(_)
        ));
        let rows = ingress.store.reactions_with(&peer).unwrap();
        assert_eq!(
            rows.len(),
            1,
            "one reaction per person per message: {rows:?}"
        );
        assert_eq!(rows[0].emoji, "👍");

        // …and withdrawing it is an empty emoji, reported so the chip can go.
        ingress.deliver(
            &hex,
            "r3",
            &ReactionEnvelope::clearing("m1").to_json().unwrap(),
            now + 2,
        );
        match rx.try_recv().unwrap() {
            BridgeEvent::IncomingReaction(r) => assert!(r.emoji.is_empty()),
            other => panic!("expected IncomingReaction, got {other:?}"),
        }
        assert!(ingress.store.reactions_with(&peer).unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_replayed_reaction_redraws_nothing_even_under_a_fresh_wrapper_id() {
        // The two-day gift-wrap backfill re-scans on every launch, and a replay
        // can arrive under a *different* wrapper id than the first delivery — so
        // event-id dedup alone would not catch this. The store's timestamp check
        // is what does.
        let dir = TempDir::new().unwrap();
        let (ingress, mut rx) = Ingress::new(&dir).await;
        let (hex, peer) = stranger();
        accepted_peer(&ingress.store, &peer, false);
        let now = now_secs();

        ingress.deliver(
            &hex,
            "r1",
            &ReactionEnvelope::new("m1", "🔥").to_json().unwrap(),
            now,
        );
        assert!(rx.try_recv().is_ok());

        ingress.deliver(
            &hex,
            "r1-again",
            &ReactionEnvelope::new("m1", "🔥").to_json().unwrap(),
            now,
        );
        assert!(
            rx.try_recv().is_err(),
            "a replayed reaction is not news, whatever wrapper it came in"
        );

        // And the sharper case: an older reaction replayed after a newer one
        // must not resurrect itself.
        ingress.deliver(
            &hex,
            "r2",
            &ReactionEnvelope::new("m1", "👍").to_json().unwrap(),
            now + 5,
        );
        assert!(rx.try_recv().is_ok());
        ingress.deliver(
            &hex,
            "r1-replay",
            &ReactionEnvelope::new("m1", "🔥").to_json().unwrap(),
            now,
        );
        assert!(rx.try_recv().is_err());
        assert_eq!(ingress.store.reactions_with(&peer).unwrap()[0].emoji, "👍");
    }

    #[tokio::test]
    async fn a_stranger_cannot_react_before_their_request_is_accepted() {
        // Same gate a beacon and a nudge sit behind: an unaccepted peer must not
        // be able to decorate our messages, and must not surface as a message
        // request full of JSON either.
        let dir = TempDir::new().unwrap();
        let (ingress, mut rx) = Ingress::new(&dir).await;
        let (hex, peer) = stranger();
        let now = now_secs();

        ingress.deliver(
            &hex,
            "r1",
            &ReactionEnvelope::new("m1", "🔥").to_json().unwrap(),
            now,
        );
        assert!(
            rx.try_recv().is_err(),
            "no event at all from an unaccepted peer"
        );
        assert!(ingress.store.reactions_with(&peer).unwrap().is_empty());
        assert!(ingress.store.messages_with(&peer).unwrap().is_empty());
    }

    #[tokio::test]
    async fn an_oversized_reaction_is_not_mistaken_for_chat_text() {
        // The parser refuses it (see `MAX_REACTION_BYTES`). The thing worth
        // pinning is what happens *next*: falling through to the plain-text
        // branch would put a wall of JSON in the conversation.
        let dir = TempDir::new().unwrap();
        let (ingress, mut rx) = Ingress::new(&dir).await;
        let (hex, peer) = stranger();
        accepted_peer(&ingress.store, &peer, false);

        let huge = ReactionEnvelope::new("m1", "🔥".repeat(MAX_REACTION_BYTES))
            .to_json()
            .unwrap();
        ingress.deliver(&hex, "r1", &huge, now_secs());
        assert!(ingress.store.reactions_with(&peer).unwrap().is_empty());
        // It does arrive as a message, which is the honest outcome for a payload
        // this side cannot interpret — but it must not have been stored as a
        // reaction, and it must not have raised a reaction event.
        assert!(!matches!(
            rx.try_recv(),
            Ok(BridgeEvent::IncomingReaction(_))
        ));
    }

    #[tokio::test]
    async fn a_delete_request_hides_our_copy_and_a_replay_is_harmless() {
        let dir = TempDir::new().unwrap();
        let (ingress, _rx) = Ingress::new(&dir).await;
        let (hex, peer) = stranger();
        accepted_peer(&ingress.store, &peer, false);
        ingress
            .store
            .save_message(&plain_message("m1", &peer, "the original", 1, false))
            .unwrap();

        let json = DeleteRequest::new("m1").to_json().unwrap();
        ingress.deliver(&hex, "d1", &json, now_secs());
        assert!(ingress.store.is_deleted_for_me(&peer, "m1").unwrap());

        // A relay redelivering the same courtesy request is harmless — the
        // message is already hidden, same as the tombstone check itself.
        ingress.deliver(&hex, "d2", &json, now_secs());
        assert!(ingress.store.is_deleted_for_me(&peer, "m1").unwrap());
    }

    #[tokio::test]
    async fn an_ungated_delete_request_is_dropped() {
        // A stranger must not be able to reach into our history before their
        // request is accepted — same gating a reaction or a nudge gets.
        let dir = TempDir::new().unwrap();
        let (ingress, _rx) = Ingress::new(&dir).await;
        let (hex, peer) = stranger();
        ingress
            .store
            .save_message(&plain_message("m1", &peer, "hi", 1, false))
            .unwrap();

        let json = DeleteRequest::new("m1").to_json().unwrap();
        ingress.deliver(&hex, "d1", &json, now_secs());
        assert!(!ingress.store.is_deleted_for_me(&peer, "m1").unwrap());
    }

    #[tokio::test]
    async fn a_comrade_who_gave_up_on_a_message_is_announced_once() {
        let dir = TempDir::new().unwrap();
        let (ingress, mut rx) = Ingress::new(&dir).await;
        let (hex, peer) = stranger();
        accepted_peer(&ingress.store, &peer, true);
        let now = now_secs();
        let nudge = Nudge::new().to_json().unwrap();

        ingress.deliver(&hex, "n1", &nudge, now);
        match rx.try_recv().unwrap() {
            BridgeEvent::ComradeNudge { peer: p, .. } => assert_eq!(p, peer),
            other => panic!("expected ComradeNudge, got {other:?}"),
        }

        // A nudge is never a chat message, a request, or a stored DM…
        assert!(ingress.store.messages_with(&peer).unwrap().is_empty());
        // …and it writes no presence state: "last seen" and the dot belong to
        // beacons alone.
        assert!(ingress.store.get_peer_presence(&peer).unwrap().is_none());

        // Relays deliver at-least-once, and the same wrapper can land twice
        // well inside the nudge's own TTL.
        ingress.deliver(&hex, "n1", &nudge, now);
        assert!(
            rx.try_recv().is_err(),
            "a redelivered nudge must not page someone twice"
        );

        // A second, genuinely new hesitation is news again — the receiver does
        // not hold the cooldown, the sender does.
        ingress.deliver(&hex, "n2", &nudge, now + 1);
        assert!(matches!(
            rx.try_recv().unwrap(),
            BridgeEvent::ComradeNudge { .. }
        ));
    }

    #[tokio::test]
    async fn a_nudge_replayed_out_of_the_backfill_raises_nothing() {
        // The vault inbox re-scans two days on every launch. Without the
        // freshness rule, every launch would re-announce every hesitation in
        // that window — and each one would read as if it had just happened.
        let dir = TempDir::new().unwrap();
        let (ingress, mut rx) = Ingress::new(&dir).await;
        let (hex, peer) = stranger();
        accepted_peer(&ingress.store, &peer, true);
        let two_days_ago = now_secs() - 2 * 24 * 60 * 60;

        ingress.deliver(&hex, "old", &Nudge::new().to_json().unwrap(), two_days_ago);
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn a_nudge_from_a_stranger_or_someone_we_did_not_choose_raises_nothing() {
        let dir = TempDir::new().unwrap();
        let (ingress, mut rx) = Ingress::new(&dir).await;
        let now = now_secs();
        let nudge = Nudge::new().to_json().unwrap();

        // An unaccepted stranger cannot page us before we accept them — and
        // their envelope must not leak into the requests bucket as raw JSON.
        let (stranger_hex, stranger_npub) = stranger();
        ingress.deliver(&stranger_hex, "s1", &nudge, now);
        assert!(rx.try_recv().is_err());
        assert!(ingress
            .store
            .messages_with(&stranger_npub)
            .unwrap()
            .is_empty());
        assert!(ingress
            .store
            .get_conversation_meta(&stranger_npub)
            .unwrap()
            .is_none());

        // An accepted peer we never chose as a comrade: silent, and unlike a
        // presence beacon it records nothing either — a nudge is not the thing
        // that reveals a one-sided comrade relationship.
        let (hex, peer) = stranger();
        accepted_peer(&ingress.store, &peer, false);
        ingress.deliver(&hex, "a1", &nudge, now);
        assert!(
            rx.try_recv().is_err(),
            "someone we didn't choose is not our comrade to be paged about"
        );
        assert!(ingress.store.get_peer_presence(&peer).unwrap().is_none());
    }

    #[tokio::test]
    async fn watching_a_composer_is_safe_before_unlock_and_survives_junk_keys() {
        // Frontends call these on a keystroke and on every thread close, so
        // they must be harmless in every state the app can be in — a courtesy
        // feature has no business raising an error into a text field.
        let rt = ComradeRuntime::new();
        rt.note_draft("npub1definitelynotakey");
        rt.abandon_draft("npub1definitelynotakey");
        rt.note_draft("");
        rt.abandon_draft("");
        // Nothing to send to, and nothing anywhere to send it with.
        assert_eq!(rt.handles().nudge_abandoned_drafts(now_secs()).await, 0);
    }

    #[tokio::test]
    async fn a_hesitation_towards_someone_who_is_not_a_comrade_is_never_sent() {
        // The consent gate, checked at send time: presence — and this — flow
        // only to people the user deliberately chose.
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();
        let (_hex, peer) = stranger();

        let watch = rt.nudge_watch.clone();
        let at = now_secs() - NUDGE_SETTLE_SECS - 60;
        watch.writing(&peer, at);
        watch.abandoned(&peer, at + 30);
        assert_eq!(
            rt.handles().nudge_abandoned_drafts(now_secs()).await,
            0,
            "no comrade flag, no disclosure"
        );

        // And the decision is spent either way: the same abandoned draft is
        // not reconsidered on the next sweep just because they were marked in
        // between.
        rt.set_comrade(&peer, true).unwrap();
        assert_eq!(rt.handles().nudge_abandoned_drafts(now_secs()).await, 0);
    }

    #[tokio::test]
    async fn reaching_for_a_pause_with_no_comrades_tells_nobody() {
        // The whole feature has to be free for someone who never chose anyone
        // — no relay traffic, and nothing remembered either.
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();
        assert_eq!(rt.nudge_comrades().await, 0);

        // …and it is safe before there is a vault at all, since the breathing
        // screen has no idea what state the runtime is in.
        let locked = ComradeRuntime::new();
        assert_eq!(locked.nudge_comrades().await, 0);
    }

    #[tokio::test]
    async fn a_pause_claims_the_same_cooldown_an_abandoned_draft_would() {
        // The wiring behind `nudge_watch`'s shared cooldown: after a
        // deliberate nudge, a draft given up on moments later must not become
        // a second notification. (The relay is unreachable here, so nothing
        // actually sends — what this pins is that the *decision* was spent.)
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();
        let (_hex, peer) = stranger();
        rt.set_comrade(&peer, true).unwrap();

        rt.nudge_comrades().await;

        let watch = rt.nudge_watch.clone();
        let at = now_secs() - NUDGE_SETTLE_SECS - 60;
        watch.writing(&peer, at);
        watch.abandoned(&peer, at + 30);
        assert!(
            watch.due(now_secs()).is_empty(),
            "one hard half-hour is one notification, not two"
        );
    }

    #[tokio::test]
    async fn locking_the_vault_drops_a_pending_nudge() {
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();
        let (_hex, peer) = stranger();
        rt.set_comrade(&peer, true).unwrap();

        let watch = rt.nudge_watch.clone();
        let at = now_secs() - NUDGE_SETTLE_SECS - 60;
        watch.writing(&peer, at);
        watch.abandoned(&peer, at + 30);

        rt.lock_vault().await;
        assert!(
            watch.due(now_secs()).is_empty(),
            "the goodbye beacon has already said we are gone; a nudge after it \
             would claim the opposite"
        );
    }

    #[tokio::test]
    async fn an_online_claim_ages_out_on_its_own_when_the_peer_just_vanishes() {
        // The common case: no goodbye ever arrives (battery died, signal
        // lost, app force-killed). Both the sweep and every read must stop
        // claiming the peer is online once their own deadline passes.
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();
        let (_hex, peer) = stranger();
        rt.set_comrade(&peer, true).unwrap();

        let store = rt.ui.store_arc().unwrap();
        let expired_at = now_secs() - 5;
        store
            .set_peer_presence(&comrade_storage::PeerPresence {
                peer_npub: peer.clone(),
                online: true,
                last_seen_at: expired_at - 480,
                expires_at: expired_at,
                peer_marked_us: true,
            })
            .unwrap();

        // Reads are computed against the clock, so the stale row never shows
        // as online even before anything sweeps it.
        assert!(!rt.comrades().unwrap()[0].online);
        assert!(!rt.peer_presence(&peer).unwrap().unwrap().online);

        let (tx, mut rx) = broadcast::channel(16);
        expire_stale_presence(Some(&store), &tx);
        match rx.try_recv().unwrap() {
            BridgeEvent::ComradePresence {
                peer: p,
                online,
                at,
                ..
            } => {
                assert_eq!(p, peer);
                assert!(!online);
                assert_eq!(at, expired_at);
            }
            other => panic!("expected an aged-out ComradePresence, got {other:?}"),
        }
        assert!(!store.get_peer_presence(&peer).unwrap().unwrap().online);

        // Idempotent: a second sweep has nothing left to announce.
        expire_stale_presence(Some(&store), &tx);
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn the_chat_list_shows_presence_only_for_chosen_comrades() {
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();
        let (_hex, peer) = stranger();
        let store = rt.ui.store_arc().unwrap();
        store
            .save_message(&comrade_storage::StoredMessage {
                id: "m1".into(),
                peer_npub: peer.clone(),
                content: "hi".into(),
                created_at: 1,
                outgoing: false,
                status: None,
                reply_to: None,
            })
            .unwrap();
        store
            .set_peer_presence(&comrade_storage::PeerPresence {
                peer_npub: peer.clone(),
                online: true,
                last_seen_at: now_secs(),
                expires_at: now_secs() + 300,
                peer_marked_us: true,
            })
            .unwrap();

        // They've marked us, so we have live presence for them — but we
        // haven't chosen them, so the chat list claims nothing.
        let before = &rt.conversations().unwrap()[0];
        assert!(!before.comrade);
        assert!(!before.online);

        rt.set_comrade(&peer, true).unwrap();
        let after = &rt.conversations().unwrap()[0];
        assert!(after.comrade);
        assert!(after.online);
    }

    // ── T1: call-signal freshness + dedup ────────────────────────────────────

    /// A relay route carrying a live together session.
    fn together_route<'a>(dedup: &'a SeenSet, link: &'a TogetherLink) -> DmRoute<'a> {
        DmRoute {
            label: TRANSPORT_RELAY,
            dedup,
            mesh: None,
            together: Some(link),
        }
    }

    fn together_link() -> TogetherLink {
        TogetherLink {
            session: Arc::new(Mutex::new(None)),
            starts_seen: Arc::new(SeenSet::new(TOGETHER_START_DEDUP_CAPACITY)),
            shares_seen: Arc::new(SeenSet::new(TOGETHER_SHARE_DEDUP_CAPACITY)),
        }
    }

    fn together_json(session_id: &str, seq: u64, signal: TogetherSignal) -> String {
        TogetherEnvelope::new(session_id, seq, now_ms(), None, signal)
            .to_json()
            .unwrap()
    }

    use comrade_core::together::TOGETHER_DIRECT_SILENCE_MS;

    /// Plant a live session the way an invitation would, so the direct-channel
    /// tests do not need a vault or a relay to have something to be inside of.
    fn plant_session(rt: &ComradeRuntime, peer_npub: &str, peer_hex: &str) {
        let link = TogetherLink {
            session: rt.together.clone(),
            starts_seen: rt.together_starts_seen.clone(),
            shares_seen: rt.together_shares_seen.clone(),
        };
        let (tx, _rx) = broadcast::channel(16);
        let start = TogetherEnvelope::new(
            "s-direct",
            1,
            now_ms(),
            None,
            TogetherSignal::Start {
                content: a_film(),
                pos_ms: 0,
                playing: false,
            },
        );
        handle_together_envelope(
            &tx,
            &link,
            peer_npub,
            peer_hex,
            now_secs(),
            Some("e-plant"),
            start,
        );
        assert!(rt.together.lock().unwrap().is_some(), "no session planted");
    }

    // ── The direct low-latency rung ──────────────────────────────────────────

    /// The behaviour that makes the direct path safe to expose: no envelope
    /// arriving on it can produce a session.
    ///
    /// Note what this does *not* prove. Two independent rules deliver it — the
    /// `Start` refusal (`direct_signal_admissible`) and the fact that a sender
    /// cannot be attributed without a live session — and this test passes on
    /// either, so deleting one leaves it green. That was checked, not assumed.
    /// `together::tests::a_direct_channel_may_not_carry_an_invitation` is what
    /// pins the refusal itself.
    #[test]
    fn a_direct_channel_cannot_open_a_session() {
        let rt = ComradeRuntime::new();
        let mut rx = rt.subscribe_events();
        let invite = together_json(
            "s-hostile",
            1,
            TogetherSignal::Start {
                content: a_film(),
                pos_ms: 0,
                playing: true,
            },
        );
        rt.together_receive_direct(&invite);
        assert!(
            rt.together.lock().unwrap().is_none(),
            "a direct channel opened a session"
        );
        assert!(rx.try_recv().is_err(), "it also announced one");
    }

    /// Even inside a live session: a second invitation arriving down the channel
    /// must not be able to replace the session it is riding on.
    #[test]
    fn a_direct_invite_inside_a_session_changes_nothing() {
        let rt = ComradeRuntime::new();
        plant_session(&rt, "npub_peer", &"11".repeat(32));
        let before = rt.together.lock().unwrap().as_ref().map(|s| s.id.clone());
        let mut rx = rt.subscribe_events();
        rt.together_receive_direct(&together_json(
            "s-other",
            9,
            TogetherSignal::Start {
                content: a_film(),
                pos_ms: 600_000,
                playing: true,
            },
        ));
        let after = rt.together.lock().unwrap().as_ref().map(|s| s.id.clone());
        assert_eq!(before, after, "a direct invite replaced the live session");
        assert!(rx.try_recv().is_err());
    }

    /// With no session there is nothing to attribute a signal to, and nothing in
    /// the payload names a sender — so it is dropped rather than guessed at.
    #[test]
    fn a_direct_signal_with_no_session_reaches_nothing() {
        let rt = ComradeRuntime::new();
        let mut rx = rt.subscribe_events();
        rt.together_receive_direct(&together_json(
            "s-nobody",
            4,
            TogetherSignal::State {
                pos_ms: 90_000,
                playing: true,
                effective_at_ms: None,
            },
        ));
        assert!(rx.try_recv().is_err());
    }

    /// The point of the rung: a command that took the fast path is applied
    /// exactly as one that took the relay, because it is the same call.
    #[test]
    fn a_command_over_the_direct_channel_lands_like_one_over_a_relay() {
        let rt = ComradeRuntime::new();
        plant_session(&rt, "npub_peer", &"11".repeat(32));
        let mut rx = rt.subscribe_events();
        rt.together_receive_direct(&together_json(
            "s-direct",
            2,
            TogetherSignal::State {
                pos_ms: 42_000,
                playing: true,
                effective_at_ms: None,
            },
        ));
        let BridgeEvent::TogetherCommand(cmd) = rx.try_recv().unwrap() else {
            panic!("a direct command did not surface");
        };
        assert!(cmd.playing);
    }

    /// Garbage on the socket is not a session-ending event: the far end of a
    /// data channel is a peer, but the bytes on it are still unvalidated input.
    #[test]
    fn rubbish_on_the_direct_channel_is_ignored_not_fatal() {
        let rt = ComradeRuntime::new();
        plant_session(&rt, "npub_peer", &"11".repeat(32));
        let mut rx = rt.subscribe_events();
        for junk in [
            "",
            "{",
            "null",
            "{\"comrade_together\":99}",
            "not json at all",
        ] {
            rt.together_receive_direct(junk);
        }
        assert!(
            rt.together.lock().unwrap().is_some(),
            "junk ended the session"
        );
        assert!(rx.try_recv().is_err());
    }

    /// Declaring a channel is what routes traffic to it, and un-declaring it
    /// must put traffic back on the relay.
    #[test]
    fn declaring_and_dropping_a_channel_moves_the_traffic() {
        let rt = ComradeRuntime::new();
        plant_session(&rt, "npub_peer", &"11".repeat(32));
        let direct = || rt.together.lock().unwrap().as_ref().map(|s| s.direct_ready);
        assert_eq!(direct(), Some(false), "a session starts on the relay");
        rt.together_direct_ready(true);
        assert_eq!(direct(), Some(true));
        rt.together_direct_ready(false);
        assert_eq!(direct(), Some(false), "a dead channel must fall back");
    }

    /// Which rung a send took, read off the one thing that distinguishes them
    /// on a runtime with no vault: the direct path returns `Ok` and emits a
    /// `TogetherOutbound`, and the relay path can only reach `VaultLocked`.
    async fn send_took_the_direct_path(rt: &ComradeRuntime) -> bool {
        let mut rx = rt.subscribe_events();
        let sent = rt
            .handles()
            .send_together(TogetherSignal::Join)
            .await
            .is_ok();
        let announced = matches!(rx.try_recv(), Ok(BridgeEvent::TogetherOutbound { .. }));
        assert_eq!(sent, announced, "a rung that returned Ok said nothing");
        announced
    }

    /// The failure a frontend cannot report: it declares a channel, the channel
    /// dies, and the close handler never runs — a crashed webview, a killed
    /// process, a bug. Nothing arrives to say so, so before this the runtime
    /// kept posting every signal into a socket nobody read until the session
    /// died on its 45 s TTL. Two heartbeats of silence now put it back on the
    /// relay by itself.
    #[tokio::test]
    async fn a_channel_that_went_quiet_without_saying_so_falls_back_to_the_relay() {
        let rt = ComradeRuntime::new();
        plant_session(&rt, "npub_peer", &"11".repeat(32));
        rt.together_direct_ready(true);
        assert!(
            send_took_the_direct_path(&rt).await,
            "a freshly declared channel must be given its chance",
        );

        // Age the last sign of life past the watchdog. The frontend still says
        // the channel is up — that is the whole point; its claim is the thing
        // that stopped being true.
        {
            let mut guard = rt.together.lock().unwrap();
            let session = guard.as_mut().unwrap();
            session.direct_evidence_ms = now_ms() - TOGETHER_DIRECT_SILENCE_MS - 1;
            assert!(session.direct_ready, "the stale claim is still standing");
        }
        assert!(
            !send_took_the_direct_path(&rt).await,
            "signals kept going into a socket nobody was reading",
        );
    }

    /// And it heals without the frontend's help: one envelope arriving over the
    /// channel is proof enough, which matters because a frontend that never
    /// noticed the outage has nothing to re-declare.
    #[tokio::test]
    async fn traffic_arriving_on_the_channel_earns_the_fast_path_back() {
        let rt = ComradeRuntime::new();
        plant_session(&rt, "npub_peer", &"11".repeat(32));
        rt.together_direct_ready(true);
        {
            let mut guard = rt.together.lock().unwrap();
            guard.as_mut().unwrap().direct_evidence_ms = now_ms() - TOGETHER_DIRECT_SILENCE_MS - 1;
        }
        assert!(!send_took_the_direct_path(&rt).await);

        rt.together_receive_direct(&together_json("s-direct", 2, TogetherSignal::Join));
        assert!(
            send_took_the_direct_path(&rt).await,
            "the channel proved itself alive and was not believed",
        );
    }

    /// Announcing a channel with no session is a no-op rather than a panic: the
    /// frontend's connection callbacks are not synchronised with session teardown.
    #[test]
    fn declaring_a_channel_with_no_session_is_harmless() {
        let rt = ComradeRuntime::new();
        rt.together_direct_ready(true);
        assert!(rt.together.lock().unwrap().is_none());
    }

    fn a_film() -> TogetherContent {
        TogetherContent::local_file(
            7_200_000,
            Some(comrade_core::together::Recording::titled("Solaris")),
        )
    }

    /// The two-day inbox backfill is the case this exists for: an invitation is
    /// the only signal that can create a session from nothing, so the age gate
    /// is the only thing standing behind it.
    #[tokio::test]
    async fn dispatch_drops_a_two_day_old_invitation_and_dedups_a_fresh_one() {
        let dir = TempDir::new().unwrap();
        let store = Arc::new(comrade_storage::EncryptedStore::open(dir.path(), "pin").unwrap());
        let vault = test_vault().await;
        let (tx, mut rx) = broadcast::channel(16);
        let dedup = SeenSet::new(CALL_SIGNAL_DEDUP_CAPACITY);
        let outbox = Arc::new(Outbox::new());
        let transport_dedup = SeenSet::with_ttl(
            CROSS_TRANSPORT_DEDUP_CAPACITY,
            std::time::Duration::from_secs(CROSS_TRANSPORT_DEDUP_SECS),
        );
        let link = together_link();
        let route = together_route(&transport_dedup, &link);
        let (hex, peer) = stranger();
        accepted_peer(&store, &peer, false);

        let invite = together_json(
            "s1",
            1,
            TogetherSignal::Start {
                content: a_film(),
                pos_ms: 0,
                playing: false,
            },
        );

        let mut stale = incoming(&hex, "e1", &invite);
        stale.created_at = now_secs().saturating_sub(172_800);
        dispatch_incoming_dm(&vault, Some(&store), &tx, &dedup, &outbox, &route, stale);
        assert!(
            rx.try_recv().is_err(),
            "a two-day-old invitation must not open a session"
        );
        assert!(link.session.lock().unwrap().is_none());

        let mut fresh = incoming(&hex, "e2", &invite);
        fresh.created_at = now_secs();
        dispatch_incoming_dm(&vault, Some(&store), &tx, &dedup, &outbox, &route, fresh);
        assert!(matches!(
            rx.try_recv().unwrap(),
            BridgeEvent::TogetherInvited(_)
        ));

        // The same invitation, redelivered after we left: the invite dedup set
        // must stop it re-inviting us.
        *link.session.lock().unwrap() = None;
        let mut again = incoming(&hex, "e3", &invite);
        again.created_at = now_secs();
        dispatch_incoming_dm(&vault, Some(&store), &tx, &dedup, &outbox, &route, again);
        assert!(
            rx.try_recv().is_err(),
            "a redelivered invitation must not re-invite"
        );
    }

    /// The scenario the whole replay story exists for: a backfilled "seek to
    /// 42:00" must be unable to move anyone's playhead. It dies twice over —
    /// once on age, and again because after a relaunch there is no session for
    /// it to name.
    #[tokio::test]
    async fn a_two_day_old_seek_or_play_moves_nothing() {
        let dir = TempDir::new().unwrap();
        let store = Arc::new(comrade_storage::EncryptedStore::open(dir.path(), "pin").unwrap());
        let vault = test_vault().await;
        let (tx, mut rx) = broadcast::channel(16);
        let dedup = SeenSet::new(CALL_SIGNAL_DEDUP_CAPACITY);
        let outbox = Arc::new(Outbox::new());
        let transport_dedup = SeenSet::with_ttl(
            CROSS_TRANSPORT_DEDUP_CAPACITY,
            std::time::Duration::from_secs(CROSS_TRANSPORT_DEDUP_SECS),
        );
        let link = together_link();
        let route = together_route(&transport_dedup, &link);
        let (hex, peer) = stranger();
        accepted_peer(&store, &peer, false);

        for signal in [
            TogetherSignal::State {
                pos_ms: 2_520_000,
                playing: false,
                effective_at_ms: None,
            },
            TogetherSignal::State {
                pos_ms: 0,
                playing: true,
                effective_at_ms: None,
            },
        ] {
            let body = together_json("s1", 9, signal);
            let mut old = incoming(&hex, "old", &body);
            old.created_at = now_secs().saturating_sub(172_800);
            dispatch_incoming_dm(&vault, Some(&store), &tx, &dedup, &outbox, &route, old);
            assert!(rx.try_recv().is_err(), "a replayed command reached the bus");
            assert!(link.session.lock().unwrap().is_none());
        }
    }

    /// Even a perfectly fresh command is inert without a session naming it —
    /// which is what makes "sessions never outlive the process" load-bearing
    /// rather than merely tidy.
    #[tokio::test]
    async fn a_command_for_an_unknown_session_reaches_nothing() {
        let dir = TempDir::new().unwrap();
        let store = Arc::new(comrade_storage::EncryptedStore::open(dir.path(), "pin").unwrap());
        let vault = test_vault().await;
        let (tx, mut rx) = broadcast::channel(16);
        let dedup = SeenSet::new(CALL_SIGNAL_DEDUP_CAPACITY);
        let outbox = Arc::new(Outbox::new());
        let transport_dedup = SeenSet::with_ttl(
            CROSS_TRANSPORT_DEDUP_CAPACITY,
            std::time::Duration::from_secs(CROSS_TRANSPORT_DEDUP_SECS),
        );
        let link = together_link();
        let route = together_route(&transport_dedup, &link);
        let (hex, peer) = stranger();
        accepted_peer(&store, &peer, false);

        let body = together_json(
            "never-heard-of-it",
            4,
            TogetherSignal::State {
                pos_ms: 1_000,
                playing: true,
                effective_at_ms: None,
            },
        );
        let mut msg = incoming(&hex, "e1", &body);
        msg.created_at = now_secs();
        dispatch_incoming_dm(&vault, Some(&store), &tx, &dedup, &outbox, &route, msg);
        assert!(rx.try_recv().is_err());
    }

    /// The call channel's gate, restated for a playhead: a stranger must not be
    /// able to drive your player, and their JSON must not surface as a message
    /// request either.
    #[tokio::test]
    async fn a_stranger_cannot_start_a_session_or_leave_a_message_request() {
        let dir = TempDir::new().unwrap();
        let store = Arc::new(comrade_storage::EncryptedStore::open(dir.path(), "pin").unwrap());
        let vault = test_vault().await;
        let (tx, mut rx) = broadcast::channel(16);
        let dedup = SeenSet::new(CALL_SIGNAL_DEDUP_CAPACITY);
        let outbox = Arc::new(Outbox::new());
        let transport_dedup = SeenSet::with_ttl(
            CROSS_TRANSPORT_DEDUP_CAPACITY,
            std::time::Duration::from_secs(CROSS_TRANSPORT_DEDUP_SECS),
        );
        let link = together_link();
        let route = together_route(&transport_dedup, &link);
        let (hex, peer) = stranger(); // deliberately never accepted

        let invite = together_json(
            "s1",
            1,
            TogetherSignal::Start {
                content: a_film(),
                pos_ms: 0,
                playing: false,
            },
        );
        let mut msg = incoming(&hex, "e1", &invite);
        msg.created_at = now_secs();
        dispatch_incoming_dm(&vault, Some(&store), &tx, &dedup, &outbox, &route, msg);

        assert!(rx.try_recv().is_err(), "a stranger reached the event bus");
        assert!(link.session.lock().unwrap().is_none());
        assert!(
            store.get_conversation_meta(&peer).unwrap().is_none(),
            "a control envelope must not leave a message request behind"
        );
    }

    #[tokio::test]
    async fn an_invitation_naming_a_malformed_video_is_refused() {
        let dir = TempDir::new().unwrap();
        let store = Arc::new(comrade_storage::EncryptedStore::open(dir.path(), "pin").unwrap());
        let vault = test_vault().await;
        let (tx, mut rx) = broadcast::channel(16);
        let dedup = SeenSet::new(CALL_SIGNAL_DEDUP_CAPACITY);
        let outbox = Arc::new(Outbox::new());
        let transport_dedup = SeenSet::with_ttl(
            CROSS_TRANSPORT_DEDUP_CAPACITY,
            std::time::Duration::from_secs(CROSS_TRANSPORT_DEDUP_SECS),
        );
        let link = together_link();
        let route = together_route(&transport_dedup, &link);
        let (hex, peer) = stranger();
        accepted_peer(&store, &peer, false);

        let invite = together_json(
            "s1",
            1,
            TogetherSignal::Start {
                content: TogetherContent::Youtube {
                    video_id: "\"><script>".into(),
                },
                pos_ms: 0,
                playing: false,
            },
        );
        let mut msg = incoming(&hex, "e1", &invite);
        msg.created_at = now_secs();
        dispatch_incoming_dm(&vault, Some(&store), &tx, &dedup, &outbox, &route, msg);
        assert!(
            rx.try_recv().is_err(),
            "a video id no frontend could safely embed must not reach one"
        );
    }

    /// Relays deliver at least once. The Lamport order is what makes that a
    /// no-op — no dedup set, and so nothing that could be evicted by a long
    /// session.
    #[tokio::test]
    async fn a_redelivered_command_is_applied_exactly_once() {
        let dir = TempDir::new().unwrap();
        let store = Arc::new(comrade_storage::EncryptedStore::open(dir.path(), "pin").unwrap());
        let vault = test_vault().await;
        let (tx, mut rx) = broadcast::channel(16);
        let dedup = SeenSet::new(CALL_SIGNAL_DEDUP_CAPACITY);
        let outbox = Arc::new(Outbox::new());
        let transport_dedup = SeenSet::with_ttl(
            CROSS_TRANSPORT_DEDUP_CAPACITY,
            std::time::Duration::from_secs(CROSS_TRANSPORT_DEDUP_SECS),
        );
        let link = together_link();
        let route = together_route(&transport_dedup, &link);
        let (hex, peer) = stranger();
        accepted_peer(&store, &peer, false);

        let invite = together_json(
            "s1",
            1,
            TogetherSignal::Start {
                content: a_film(),
                pos_ms: 0,
                playing: false,
            },
        );
        let mut msg = incoming(&hex, "e1", &invite);
        msg.created_at = now_secs();
        dispatch_incoming_dm(&vault, Some(&store), &tx, &dedup, &outbox, &route, msg);
        assert!(matches!(
            rx.try_recv().unwrap(),
            BridgeEvent::TogetherInvited(_)
        ));

        let play = together_json(
            "s1",
            2,
            TogetherSignal::State {
                pos_ms: 5_000,
                playing: true,
                effective_at_ms: None,
            },
        );
        // Two different wrapper ids carrying the same command — which is what a
        // relay redelivery and the backfill both look like.
        for id in ["e2", "e3"] {
            let mut m = incoming(&hex, id, &play);
            m.created_at = now_secs();
            dispatch_incoming_dm(&vault, Some(&store), &tx, &dedup, &outbox, &route, m);
        }
        assert!(matches!(
            rx.try_recv().unwrap(),
            BridgeEvent::TogetherCommand(_)
        ));
        assert!(
            rx.try_recv().is_err(),
            "the same command must not be applied twice"
        );
    }

    /// The improvement over every position-swapping watch-party protocol,
    /// asserted end-to-end: a command that spent time in flight must land where
    /// the sender *is*, not where they *were*. Adopting the number verbatim
    /// leaves you behind by exactly the flight time, every time — and invisibly,
    /// because both sides agree on the number they exchanged.
    #[tokio::test]
    async fn a_command_that_spent_time_in_flight_lands_where_the_sender_is_now() {
        let dir = TempDir::new().unwrap();
        let store = Arc::new(comrade_storage::EncryptedStore::open(dir.path(), "pin").unwrap());
        let vault = test_vault().await;
        let (tx, mut rx) = broadcast::channel(16);
        let dedup = SeenSet::new(CALL_SIGNAL_DEDUP_CAPACITY);
        let outbox = Arc::new(Outbox::new());
        let transport_dedup = SeenSet::with_ttl(
            CROSS_TRANSPORT_DEDUP_CAPACITY,
            std::time::Duration::from_secs(CROSS_TRANSPORT_DEDUP_SECS),
        );
        let link = together_link();
        let route = together_route(&transport_dedup, &link);
        let (hex, peer) = stranger();
        accepted_peer(&store, &peer, false);

        let invite = together_json(
            "s1",
            1,
            TogetherSignal::Start {
                content: a_film(),
                pos_ms: 0,
                playing: false,
            },
        );
        let mut msg = incoming(&hex, "e1", &invite);
        msg.created_at = now_secs();
        dispatch_incoming_dm(&vault, Some(&store), &tx, &dedup, &outbox, &route, msg);
        assert!(matches!(
            rx.try_recv().unwrap(),
            BridgeEvent::TogetherInvited(_)
        ));

        // They started playing at 60.000s, and it took 400 ms to reach us.
        let flight_ms = 400u64;
        let body = TogetherEnvelope::new(
            "s1",
            2,
            now_ms(),
            None,
            TogetherSignal::State {
                pos_ms: 60_000,
                playing: true,
                effective_at_ms: Some(now_ms().saturating_sub(flight_ms)),
            },
        )
        .to_json()
        .unwrap();
        let mut m = incoming(&hex, "e2", &body);
        m.created_at = now_secs();
        dispatch_incoming_dm(&vault, Some(&store), &tx, &dedup, &outbox, &route, m);

        let BridgeEvent::TogetherCommand(cmd) = rx.try_recv().unwrap() else {
            panic!("expected a command");
        };
        assert!(cmd.playing);
        // Allow a little slack for the clock read between building and handling.
        let advanced = cmd.pos_ms as i64 - 60_000;
        assert!(
            (advanced - flight_ms as i64).abs() <= 50,
            "expected ~{flight_ms}ms of flight added back, got {advanced}ms — \
             adopting the sender's stale number is the bug this test exists for"
        );
        assert_eq!(cmd.apply_in_ms, 0, "the moment has already passed");
    }

    /// A sender on a transport fast enough to schedule ahead makes both players
    /// change state on the same instant, rather than one chasing the other.
    #[tokio::test]
    async fn a_command_scheduled_ahead_asks_the_player_to_wait() {
        let dir = TempDir::new().unwrap();
        let store = Arc::new(comrade_storage::EncryptedStore::open(dir.path(), "pin").unwrap());
        let vault = test_vault().await;
        let (tx, mut rx) = broadcast::channel(16);
        let dedup = SeenSet::new(CALL_SIGNAL_DEDUP_CAPACITY);
        let outbox = Arc::new(Outbox::new());
        let transport_dedup = SeenSet::with_ttl(
            CROSS_TRANSPORT_DEDUP_CAPACITY,
            std::time::Duration::from_secs(CROSS_TRANSPORT_DEDUP_SECS),
        );
        let link = together_link();
        let route = together_route(&transport_dedup, &link);
        let (hex, peer) = stranger();
        accepted_peer(&store, &peer, false);

        let invite = together_json(
            "s1",
            1,
            TogetherSignal::Start {
                content: a_film(),
                pos_ms: 0,
                playing: false,
            },
        );
        let mut msg = incoming(&hex, "e1", &invite);
        msg.created_at = now_secs();
        dispatch_incoming_dm(&vault, Some(&store), &tx, &dedup, &outbox, &route, msg);
        let _ = rx.try_recv();

        let body = TogetherEnvelope::new(
            "s1",
            2,
            now_ms(),
            None,
            TogetherSignal::State {
                pos_ms: 60_000,
                playing: true,
                effective_at_ms: Some(now_ms() + 250),
            },
        )
        .to_json()
        .unwrap();
        let mut m = incoming(&hex, "e2", &body);
        m.created_at = now_secs();
        dispatch_incoming_dm(&vault, Some(&store), &tx, &dedup, &outbox, &route, m);

        let BridgeEvent::TogetherCommand(cmd) = rx.try_recv().unwrap() else {
            panic!("expected a command");
        };
        assert!(
            cmd.apply_in_ms > 0 && cmd.apply_in_ms <= 250,
            "got {}",
            cmd.apply_in_ms
        );
        assert_eq!(
            cmd.pos_ms, 60_000,
            "a scheduled command needs no compensation"
        );
    }

    /// The claim that a ten-second heartbeat is not a periodic producer on the
    /// critical bus. If someone later makes a `Hold` verdict emit, this fails.
    #[tokio::test]
    async fn a_steady_heartbeat_produces_no_bus_traffic() {
        let dir = TempDir::new().unwrap();
        let store = Arc::new(comrade_storage::EncryptedStore::open(dir.path(), "pin").unwrap());
        let vault = test_vault().await;
        let (tx, mut rx) = broadcast::channel(16);
        let dedup = SeenSet::new(CALL_SIGNAL_DEDUP_CAPACITY);
        let outbox = Arc::new(Outbox::new());
        let transport_dedup = SeenSet::with_ttl(
            CROSS_TRANSPORT_DEDUP_CAPACITY,
            std::time::Duration::from_secs(CROSS_TRANSPORT_DEDUP_SECS),
        );
        let link = together_link();
        let route = together_route(&transport_dedup, &link);
        let (hex, peer) = stranger();
        accepted_peer(&store, &peer, false);

        let invite = together_json(
            "s1",
            1,
            TogetherSignal::Start {
                content: a_film(),
                pos_ms: 0,
                playing: true,
            },
        );
        let mut msg = incoming(&hex, "e1", &invite);
        msg.created_at = now_secs();
        dispatch_incoming_dm(&vault, Some(&store), &tx, &dedup, &outbox, &route, msg);
        assert!(matches!(
            rx.try_recv().unwrap(),
            BridgeEvent::TogetherInvited(_)
        ));

        // Both playing, in step. Ten heartbeats in a row must say nothing.
        {
            let mut guard = link.session.lock().unwrap();
            let session = guard.as_mut().unwrap();
            session.joined = true;
            session.local_playing = true;
            session.local_pos_ms = 60_000;
        }
        for i in 0..10 {
            let beat = together_json(
                "s1",
                1,
                TogetherSignal::Heartbeat {
                    pos_ms: 60_000,
                    playing: true,
                    applied_seq: 1,
                    output_latency_ms: 0,
                },
            );
            let mut m = incoming(&hex, &format!("hb{i}"), &beat);
            m.created_at = now_secs();
            dispatch_incoming_dm(&vault, Some(&store), &tx, &dedup, &outbox, &route, m);
        }
        assert!(
            rx.try_recv().is_err(),
            "a steady session must not put a single event on the critical bus"
        );
    }

    // ── Handing the file over ───────────────────────────────────────────────

    fn a_transfer_offer() -> ShareSignal {
        ShareSignal::Offer {
            offer: comrade_core::share::ShareOffer {
                total_bytes: 8_000_000,
                chunk_bytes: comrade_core::share::SHARE_CHUNK_BYTES,
                sha256: "b".repeat(64),
                duration_ms: 240_000,
            },
        }
    }

    /// The claim that justifies putting the transfer negotiation inside the
    /// session envelope instead of giving it a marker of its own: it inherits
    /// the session scoping, so an SDP offer naming no session negotiates
    /// nothing. Without this, one DM from anyone would be enough to make this
    /// device start gathering ICE candidates for a stranger.
    #[tokio::test]
    async fn a_transfer_cannot_be_negotiated_without_a_session_to_negotiate_it_in() {
        let dir = TempDir::new().unwrap();
        let store = Arc::new(comrade_storage::EncryptedStore::open(dir.path(), "pin").unwrap());
        let vault = test_vault().await;
        let (tx, mut rx) = broadcast::channel(16);
        let dedup = SeenSet::new(CALL_SIGNAL_DEDUP_CAPACITY);
        let outbox = Arc::new(Outbox::new());
        let transport_dedup = SeenSet::with_ttl(
            CROSS_TRANSPORT_DEDUP_CAPACITY,
            std::time::Duration::from_secs(CROSS_TRANSPORT_DEDUP_SECS),
        );
        let link = together_link();
        let route = together_route(&transport_dedup, &link);
        let (hex, peer) = stranger();
        accepted_peer(&store, &peer, false);

        // Accepted contact, perfectly fresh, well-formed — and still inert,
        // because there is no session it can name.
        let body = together_json(
            "s-nobody",
            1,
            TogetherSignal::Share {
                signal: ShareSignal::Transport {
                    signal: comrade_core::share::TransferSignal::Offer {
                        sdp: "v=0\r\n".into(),
                    },
                },
            },
        );
        let mut msg = incoming(&hex, "t1", &body);
        msg.created_at = now_secs();
        dispatch_incoming_dm(&vault, Some(&store), &tx, &dedup, &outbox, &route, msg);
        assert!(
            rx.try_recv().is_err(),
            "an SDP offer for no session reached the frontend"
        );
    }

    // ── Handing a large attachment over ─────────────────────────────────────

    fn handoff_json(transfer_id: &str, signal: HandoffSignal) -> String {
        HandoffEnvelope::new(transfer_id, signal).to_json().unwrap()
    }

    fn an_attachment_offer() -> HandoffSignal {
        HandoffSignal::Offer {
            attachment: comrade_core::handoff::AttachmentHandoff {
                shape: comrade_core::share::ShareOffer {
                    total_bytes: 400 * 1024 * 1024,
                    chunk_bytes: comrade_core::share::SHARE_CHUNK_BYTES,
                    sha256: "c".repeat(64),
                    duration_ms: 0,
                },
                mime_type: "video/mp4".into(),
                file_name: "holiday.mp4".into(),
                caption: "the last morning".into(),
            },
        }
    }

    /// The gate. A handoff has no session to hide behind, so the *only* thing
    /// standing between a stranger and this device gathering ICE candidates for
    /// them is the accepted-conversation check — the same one a call signal gets.
    #[tokio::test]
    async fn a_handoff_from_someone_not_accepted_negotiates_nothing() {
        let dir = TempDir::new().unwrap();
        let store = Arc::new(comrade_storage::EncryptedStore::open(dir.path(), "pin").unwrap());
        let vault = test_vault().await;
        let (tx, mut rx) = broadcast::channel(16);
        let dedup = SeenSet::new(CALL_SIGNAL_DEDUP_CAPACITY);
        let outbox = Arc::new(Outbox::new());
        let transport_dedup = SeenSet::with_ttl(
            CROSS_TRANSPORT_DEDUP_CAPACITY,
            std::time::Duration::from_secs(CROSS_TRANSPORT_DEDUP_SECS),
        );
        let route = relay_route(&transport_dedup);
        // Deliberately *not* `accepted_peer`: a pending request is the case.
        let (hex, _peer) = stranger();

        let body = handoff_json("t-stranger", an_attachment_offer());
        let mut msg = incoming(&hex, "h1", &body);
        msg.created_at = now_secs();
        dispatch_incoming_dm(&vault, Some(&store), &tx, &dedup, &outbox, &route, msg);
        assert!(
            rx.try_recv().is_err(),
            "an offer from a stranger reached the frontend"
        );
    }

    /// And it must not fall through into the message-request bucket either: a
    /// person should never see a chat request whose body is a wall of JSON.
    #[tokio::test]
    async fn an_ungated_handoff_is_dropped_rather_than_shown_as_a_request() {
        let dir = TempDir::new().unwrap();
        let store = Arc::new(comrade_storage::EncryptedStore::open(dir.path(), "pin").unwrap());
        let vault = test_vault().await;
        let (tx, mut rx) = broadcast::channel(16);
        let dedup = SeenSet::new(CALL_SIGNAL_DEDUP_CAPACITY);
        let outbox = Arc::new(Outbox::new());
        let transport_dedup = SeenSet::with_ttl(
            CROSS_TRANSPORT_DEDUP_CAPACITY,
            std::time::Duration::from_secs(CROSS_TRANSPORT_DEDUP_SECS),
        );
        let route = relay_route(&transport_dedup);
        let (hex, _peer) = stranger();

        let body = handoff_json("t-x", HandoffSignal::Accept);
        let mut msg = incoming(&hex, "h2", &body);
        msg.created_at = now_secs();
        dispatch_incoming_dm(&vault, Some(&store), &tx, &dedup, &outbox, &route, msg);
        match rx.try_recv() {
            Err(_) => {}
            Ok(BridgeEvent::IncomingMessageRequest(r)) => {
                panic!("surfaced as a message request: {}", r.last_message)
            }
            Ok(other) => panic!("unexpected event: {other:?}"),
        }
    }

    /// From an accepted contact it goes straight through, unchanged and with no
    /// runtime-side transfer state — the frontend owns the peer connection, so it
    /// owns which transfer ids are live.
    #[tokio::test]
    async fn a_handoff_from_an_accepted_contact_reaches_the_frontend_unchanged() {
        let dir = TempDir::new().unwrap();
        let store = Arc::new(comrade_storage::EncryptedStore::open(dir.path(), "pin").unwrap());
        let vault = test_vault().await;
        let (tx, mut rx) = broadcast::channel(16);
        let dedup = SeenSet::new(CALL_SIGNAL_DEDUP_CAPACITY);
        let outbox = Arc::new(Outbox::new());
        let transport_dedup = SeenSet::with_ttl(
            CROSS_TRANSPORT_DEDUP_CAPACITY,
            std::time::Duration::from_secs(CROSS_TRANSPORT_DEDUP_SECS),
        );
        let route = relay_route(&transport_dedup);
        let (hex, peer) = stranger();
        accepted_peer(&store, &peer, false);

        let body = handoff_json("t-live", an_attachment_offer());
        let mut msg = incoming(&hex, "h3", &body);
        msg.created_at = now_secs();
        dispatch_incoming_dm(&vault, Some(&store), &tx, &dedup, &outbox, &route, msg);

        let mut seen = None;
        while let Ok(event) = rx.try_recv() {
            if let BridgeEvent::AttachmentHandoff(dto) = event {
                seen = Some(dto);
            }
        }
        let dto = seen.expect("the handoff never reached the frontend");
        assert_eq!(dto.transfer_id, "t-live");
        assert_eq!(dto.peer, peer);
        match dto.signal {
            HandoffSignal::Offer { attachment } => {
                // The whole point of the offer arriving first: 400 MB is a
                // decision, and it is answerable before a byte moves.
                assert_eq!(attachment.shape.total_bytes, 400 * 1024 * 1024);
                assert_eq!(attachment.file_name, "holiday.mp4");
                assert_eq!(attachment.caption, "the last morning");
            }
            other => panic!("signal changed in transit: {other:?}"),
        }
    }

    /// A together envelope and a handoff envelope must not shadow each other:
    /// both are JSON DM bodies, and whichever is parsed first would swallow the
    /// other if the markers were not distinct.
    #[tokio::test]
    async fn a_together_envelope_is_not_mistaken_for_a_handoff() {
        let dir = TempDir::new().unwrap();
        let store = Arc::new(comrade_storage::EncryptedStore::open(dir.path(), "pin").unwrap());
        let vault = test_vault().await;
        let (tx, mut rx) = broadcast::channel(16);
        let dedup = SeenSet::new(CALL_SIGNAL_DEDUP_CAPACITY);
        let outbox = Arc::new(Outbox::new());
        let transport_dedup = SeenSet::with_ttl(
            CROSS_TRANSPORT_DEDUP_CAPACITY,
            std::time::Duration::from_secs(CROSS_TRANSPORT_DEDUP_SECS),
        );
        let link = together_link();
        let route = together_route(&transport_dedup, &link);
        let (hex, peer) = stranger();
        accepted_peer(&store, &peer, false);

        let invite = together_json(
            "s-not-a-handoff",
            1,
            TogetherSignal::Share {
                signal: a_transfer_offer(),
            },
        );
        let mut msg = incoming(&hex, "h4", &invite);
        msg.created_at = now_secs();
        dispatch_incoming_dm(&vault, Some(&store), &tx, &dedup, &outbox, &route, msg);
        while let Ok(event) = rx.try_recv() {
            assert!(
                !matches!(event, BridgeEvent::AttachmentHandoff(_)),
                "a together share was routed as an attachment handoff"
            );
        }
    }

    /// Inside a session it goes straight through, unchanged. The runtime keeps
    /// no transfer state on purpose — see [`TogetherShareDto`].
    #[tokio::test]
    async fn a_share_signal_inside_a_session_reaches_the_frontend_unchanged() {
        let dir = TempDir::new().unwrap();
        let store = Arc::new(comrade_storage::EncryptedStore::open(dir.path(), "pin").unwrap());
        let vault = test_vault().await;
        let (tx, mut rx) = broadcast::channel(16);
        let dedup = SeenSet::new(CALL_SIGNAL_DEDUP_CAPACITY);
        let outbox = Arc::new(Outbox::new());
        let transport_dedup = SeenSet::with_ttl(
            CROSS_TRANSPORT_DEDUP_CAPACITY,
            std::time::Duration::from_secs(CROSS_TRANSPORT_DEDUP_SECS),
        );
        let link = together_link();
        let route = together_route(&transport_dedup, &link);
        let (hex, peer) = stranger();
        accepted_peer(&store, &peer, false);

        let invite = together_json(
            "s-share",
            1,
            TogetherSignal::Start {
                content: a_film(),
                pos_ms: 0,
                playing: false,
            },
        );
        let mut msg = incoming(&hex, "s1", &invite);
        msg.created_at = now_secs();
        dispatch_incoming_dm(&vault, Some(&store), &tx, &dedup, &outbox, &route, msg);
        assert!(matches!(
            rx.try_recv().unwrap(),
            BridgeEvent::TogetherInvited(_)
        ));

        for signal in [ShareSignal::Ask, a_transfer_offer(), ShareSignal::Accept] {
            let body = together_json(
                "s-share",
                2,
                TogetherSignal::Share {
                    signal: signal.clone(),
                },
            );
            let mut msg = incoming(&hex, &format!("t-{}", signal.kind_str()), &body);
            msg.created_at = now_secs();
            dispatch_incoming_dm(&vault, Some(&store), &tx, &dedup, &outbox, &route, msg);
            match rx.try_recv().expect("the frontend must hear it") {
                BridgeEvent::TogetherShare(dto) => {
                    assert_eq!(dto.session_id, "s-share");
                    assert_eq!(dto.signal, signal, "the signal must arrive as it was sent");
                }
                other => panic!("expected a share signal, got {other:?}"),
            }
        }
    }

    /// AUDIT.md Q18. Relays deliver at least once and `inbox_since` widens the
    /// backfill floor back to the watermark on every reconnect, so the same
    /// wrapper genuinely arrives twice — and the frontend on the other end of
    /// this event re-arms its transfer when it does, dropping a live
    /// `PeerConnection` on the floor.
    ///
    /// The second half is the half that could be got wrong: the guard is by
    /// **event id**, so two different signals — which is what an ICE trickle
    /// is — both still get through. A guard keyed by anything coarser would
    /// silence a live negotiation.
    #[tokio::test]
    async fn a_redelivered_share_signal_reaches_the_frontend_once() {
        let dir = TempDir::new().unwrap();
        let store = Arc::new(comrade_storage::EncryptedStore::open(dir.path(), "pin").unwrap());
        let vault = test_vault().await;
        let (tx, mut rx) = broadcast::channel(16);
        let dedup = SeenSet::new(CALL_SIGNAL_DEDUP_CAPACITY);
        let outbox = Arc::new(Outbox::new());
        let transport_dedup = SeenSet::with_ttl(
            CROSS_TRANSPORT_DEDUP_CAPACITY,
            std::time::Duration::from_secs(CROSS_TRANSPORT_DEDUP_SECS),
        );
        let link = together_link();
        let route = together_route(&transport_dedup, &link);
        let (hex, peer) = stranger();
        accepted_peer(&store, &peer, false);

        let invite = together_json(
            "s-replay",
            1,
            TogetherSignal::Start {
                content: a_film(),
                pos_ms: 0,
                playing: false,
            },
        );
        let mut msg = incoming(&hex, "s1", &invite);
        msg.created_at = now_secs();
        dispatch_incoming_dm(&vault, Some(&store), &tx, &dedup, &outbox, &route, msg);
        assert!(matches!(
            rx.try_recv().unwrap(),
            BridgeEvent::TogetherInvited(_)
        ));

        let ask = together_json(
            "s-replay",
            2,
            TogetherSignal::Share {
                signal: ShareSignal::Ask,
            },
        );
        let deliver = |event_id: &str, body: &str| {
            let mut msg = incoming(&hex, event_id, body);
            msg.created_at = now_secs();
            dispatch_incoming_dm(&vault, Some(&store), &tx, &dedup, &outbox, &route, msg);
        };

        deliver("t-ask", &ask);
        assert!(matches!(
            rx.try_recv().expect("the first copy must arrive"),
            BridgeEvent::TogetherShare(_)
        ));
        deliver("t-ask", &ask);
        assert!(
            rx.try_recv().is_err(),
            "the backfilled copy would have re-armed a running transfer"
        );

        // Two candidates from one trickle: same session, same kind, different
        // events. Both are live signals and both have to land.
        for (event_id, candidate) in [
            ("t-ice-1", "candidate:1 1 udp 2 10.0.0.1 5 typ host"),
            ("t-ice-2", "candidate:2 1 udp 2 10.0.0.2 6 typ host"),
        ] {
            let body = together_json(
                "s-replay",
                3,
                TogetherSignal::Share {
                    signal: ShareSignal::Transport {
                        signal: comrade_core::share::TransferSignal::Ice {
                            candidate: candidate.into(),
                            sdp_mid: Some("0".into()),
                            sdp_m_line_index: Some(0),
                        },
                    },
                },
            );
            deliver(event_id, &body);
            match rx
                .try_recv()
                .expect("a distinct signal must not be deduped")
            {
                BridgeEvent::TogetherShare(dto) => assert_eq!(
                    dto.signal,
                    ShareSignal::Transport {
                        signal: comrade_core::share::TransferSignal::Ice {
                            candidate: candidate.into(),
                            sdp_mid: Some("0".into()),
                            sdp_m_line_index: Some(0),
                        },
                    },
                    "the signal must arrive as it was sent"
                ),
                other => panic!("expected a share signal, got {other:?}"),
            }
        }
    }

    /// A share signal must not be ranked against play and pause. A transfer
    /// negotiation trickles ICE candidates at its own pace; if each one counted
    /// as a command, a burst of them would outrank the pause button and the
    /// person pressing it would watch it do nothing.
    #[tokio::test]
    async fn negotiating_a_transfer_never_outranks_the_pause_button() {
        let dir = TempDir::new().unwrap();
        let store = Arc::new(comrade_storage::EncryptedStore::open(dir.path(), "pin").unwrap());
        let vault = test_vault().await;
        let (tx, mut rx) = broadcast::channel(16);
        let dedup = SeenSet::new(CALL_SIGNAL_DEDUP_CAPACITY);
        let outbox = Arc::new(Outbox::new());
        let transport_dedup = SeenSet::with_ttl(
            CROSS_TRANSPORT_DEDUP_CAPACITY,
            std::time::Duration::from_secs(CROSS_TRANSPORT_DEDUP_SECS),
        );
        let link = together_link();
        let route = together_route(&transport_dedup, &link);
        let (hex, peer) = stranger();
        accepted_peer(&store, &peer, false);

        let invite = together_json(
            "s-rank",
            1,
            TogetherSignal::Start {
                content: a_film(),
                pos_ms: 0,
                playing: true,
            },
        );
        let mut msg = incoming(&hex, "r1", &invite);
        msg.created_at = now_secs();
        dispatch_incoming_dm(&vault, Some(&store), &tx, &dedup, &outbox, &route, msg);
        let _ = rx.try_recv();

        // A high-seq share signal, then a pause at a *lower* seq. If the share
        // had been stamped as a command, the pause would lose.
        let noisy = together_json(
            "s-rank",
            99,
            TogetherSignal::Share {
                signal: ShareSignal::Ask,
            },
        );
        let mut msg = incoming(&hex, "r2", &noisy);
        msg.created_at = now_secs();
        dispatch_incoming_dm(&vault, Some(&store), &tx, &dedup, &outbox, &route, msg);
        assert!(matches!(
            rx.try_recv().unwrap(),
            BridgeEvent::TogetherShare(_)
        ));

        let pause = together_json(
            "s-rank",
            2,
            TogetherSignal::State {
                pos_ms: 30_000,
                playing: false,
                effective_at_ms: None,
            },
        );
        let mut msg = incoming(&hex, "r3", &pause);
        msg.created_at = now_secs();
        dispatch_incoming_dm(&vault, Some(&store), &tx, &dedup, &outbox, &route, msg);
        match rx.try_recv().expect("the pause must still land") {
            BridgeEvent::TogetherCommand(dto) => assert!(!dto.playing),
            other => panic!("expected the pause, got {other:?}"),
        }
    }

    // ── Transfer policy ─────────────────────────────────────────────────────

    /// The default, and the one that matters: nothing bulk goes through a
    /// relay, and the connection is not even offered TURN so the case cannot
    /// arise by accident.
    #[test]
    fn by_default_a_film_does_not_go_through_someone_elses_relay() {
        let rt = ComradeRuntime::new();
        assert_eq!(rt.share_relay_policy(), RelayPolicy::DirectOnly);
        assert!(
            !rt.share_ice_servers_allowed(),
            "a direct-only transfer connection must not be handed TURN"
        );
        let v = rt.share_transfer_verdict("relay", "host", 8_000_000_000, false);
        assert_eq!(v.verdict, "refuse");
        assert_eq!(v.path, "relay");
        assert_eq!(v.reason, Some(RefusalReason::RelayForbidden));
    }

    /// "We could not tell" must never read as "it was fine".
    #[test]
    fn a_path_ice_has_not_settled_on_is_refused_rather_than_assumed_direct() {
        let rt = ComradeRuntime::new();
        let v = rt.share_transfer_verdict("", "", 1, false);
        assert_eq!(v.path, "unknown");
        assert_eq!(v.reason, Some(RefusalReason::PathUnknown));
    }

    /// The policy is about relays. Two devices talking straight to each other
    /// are nobody else's cost, so the strictest policy still allows it.
    #[test]
    fn a_direct_path_carries_anything_under_any_policy() {
        let rt = ComradeRuntime::new();
        for (local, remote) in [("host", "host"), ("srflx", "host"), ("srflx", "srflx")] {
            assert_eq!(
                rt.share_transfer_verdict(local, remote, u64::MAX, false)
                    .verdict,
                "allow",
                "{local}/{remote}"
            );
        }
    }

    #[test]
    fn changing_the_policy_changes_the_answer_and_the_ice_list_together() {
        let rt = ComradeRuntime::new();
        // No store attached, so the choice holds for this process and says so
        // rather than reporting a save that did not happen.
        assert!(matches!(
            rt.set_share_relay_policy(RelayPolicy::UnderBytes { limit: 10_000_000 }),
            Err(UiError::VaultLocked)
        ));
        assert_eq!(
            rt.share_relay_policy(),
            RelayPolicy::UnderBytes { limit: 10_000_000 },
            "the cell still took the change"
        );
        assert!(
            rt.share_ice_servers_allowed(),
            "a policy that can use a relay must be allowed to gather one"
        );
        assert_eq!(
            rt.share_transfer_verdict("relay", "host", 9_000_000, false)
                .verdict,
            "allow"
        );
        let big = rt.share_transfer_verdict("relay", "host", 11_000_000, false);
        assert_eq!(big.verdict, "refuse");
        assert_eq!(
            big.reason,
            Some(RefusalReason::TooLargeForRelay { limit: 10_000_000 })
        );

        let _ = rt.set_share_relay_policy(RelayPolicy::AskEachTime);
        let ask = rt.share_transfer_verdict("relay", "relay", 500, false);
        assert_eq!(ask.verdict, "needs_consent");
        assert_eq!(
            ask.relayed_bytes,
            Some(500),
            "the question has to be able to name the size"
        );
    }

    /// The consent loop end to end: the runtime asks, the frontend answers,
    /// and the same call that asked now allows.
    #[test]
    fn a_yes_is_carried_back_into_the_call_that_asked_for_it() {
        let rt = ComradeRuntime::new();
        let _ = rt.set_share_relay_policy(RelayPolicy::AskEachTime);
        let asked = rt.share_transfer_verdict("relay", "relay", 500, false);
        assert_eq!(asked.verdict, "needs_consent");
        assert_eq!(
            rt.share_transfer_verdict("relay", "relay", 500, true)
                .verdict,
            "allow"
        );
    }

    /// The frontend is the least trustworthy caller this policy has, so the one
    /// thing it must not be able to do is talk its way past a refusal.
    #[test]
    fn a_frontend_claiming_consent_cannot_talk_past_a_refusal() {
        let rt = ComradeRuntime::new();
        // Default policy: relayed bulk is refused outright, consent or not.
        let v = rt.share_transfer_verdict("relay", "host", 1_000, true);
        assert_eq!(v.verdict, "refuse");
        assert_eq!(v.reason, Some(RefusalReason::RelayForbidden));

        // Over the allowance: the refusal names a limit, and a yes does not
        // raise it — changing the limit is a policy change, not a dialog.
        let _ = rt.set_share_relay_policy(RelayPolicy::UnderBytes { limit: 10 });
        let over = rt.share_transfer_verdict("relay", "host", 11, true);
        assert_eq!(over.verdict, "refuse");
        assert_eq!(
            over.reason,
            Some(RefusalReason::TooLargeForRelay { limit: 10 })
        );

        // And an unsettled path stays unsettled.
        assert_eq!(
            rt.share_transfer_verdict("", "", 1, true).reason,
            Some(RefusalReason::PathUnknown)
        );
    }

    /// Every policy survives a write and a read, so a choice made once is the
    /// choice the next launch enforces.
    #[tokio::test]
    async fn a_relay_policy_outlives_the_process_that_chose_it() {
        let dir = TempDir::new().unwrap();
        for policy in [
            RelayPolicy::UnderBytes { limit: 42 },
            RelayPolicy::AskEachTime,
            RelayPolicy::Always,
            RelayPolicy::DirectOnly,
        ] {
            let mut rt = ComradeRuntime::new();
            rt.unlock_vault(dir.path(), "pin").await.unwrap();
            rt.set_share_relay_policy(policy).unwrap();
            // redb holds the file exclusively, so the "next launch" cannot open
            // it until this one is gone — which is also the situation being
            // modelled.
            drop(rt);

            let mut next = ComradeRuntime::new();
            assert_eq!(
                next.share_relay_policy(),
                RelayPolicy::DirectOnly,
                "a locked vault has no preference to read yet"
            );
            next.unlock_vault(dir.path(), "pin").await.unwrap();
            assert_eq!(
                next.share_relay_policy(),
                policy,
                "{policy:?} did not survive"
            );
            drop(next);
        }
    }

    /// A stored value this build does not recognise — an older or newer write —
    /// must read as the policy that carries nobody's bytes, never as permission.
    #[test]
    fn an_unreadable_stored_policy_falls_back_to_refusing() {
        for stored in ["", "relay_everything", "ALWAYS", "always "] {
            assert_eq!(
                relay_policy_from_prefs(&comrade_storage::SharePrefs {
                    relay_policy: stored.to_string(),
                    relay_limit_bytes: 999,
                }),
                RelayPolicy::DirectOnly,
                "{stored:?} was read as something other than direct-only"
            );
        }
    }

    /// The pump's budget, from the runtime rather than a frontend's own copy.
    #[test]
    fn the_send_budget_empties_as_the_channel_fills() {
        let rt = ComradeRuntime::new();
        assert!(rt.share_chunks_to_send(0) > 0);
        assert_eq!(
            rt.share_chunks_to_send(share_transport::SHARE_BUFFER_HIGH_WATER),
            0,
            "a full channel must be told to wait, not to send one more"
        );
    }

    /// A file that is still arriving is playable before it is whole, and the
    /// runtime is where the thresholds live — the frontend brings its own
    /// tracker's numbers and gets the same answer core's tracker would give.
    #[test]
    fn a_partly_arrived_file_plays_and_holds_by_the_same_rule_core_uses() {
        let rt = ComradeRuntime::new();
        // Ten chunks, one second each — the same shape `share.rs` tests use.
        let mut tracker = comrade_core::share::ShareTracker::new(comrade_core::share::ShareOffer {
            total_bytes: 1000,
            chunk_bytes: 100,
            sha256: "a".repeat(64),
            duration_ms: 10_000,
        });
        for i in 0..3 {
            tracker.accept(i);
        }
        for (pos, playing) in [(0u64, false), (0, true), (3_000, true), (9_000, false)] {
            assert_eq!(
                rt.share_read_verdict(
                    tracker.runway_ms(pos),
                    tracker.tail_complete_at(pos),
                    playing
                ),
                tracker.read_verdict_at(pos, playing),
                "at {pos} ms, playing {playing}"
            );
        }
        // Spelled out, because these are the two answers the frontends act on:
        // three seconds is not enough to start on, and running out mid-playback
        // is a local hold and nothing on the wire.
        assert_eq!(
            rt.share_read_verdict(3_000, false, false),
            ReadVerdict::Hold
        );
        assert_eq!(rt.share_read_verdict(0, false, true), ReadVerdict::Hold);
        assert_eq!(
            rt.share_read_verdict(5_000, false, false),
            ReadVerdict::Start
        );
    }

    /// It has to be answerable from inside a `readAt` or a `Range` handler with
    /// a session running and its lock held, which is exactly the shape that
    /// froze calls on "Connecting…" twice before.
    #[test]
    fn the_read_verdict_answers_with_the_session_lock_held() {
        let rt = ComradeRuntime::new();
        let guard = rt.together.lock().unwrap();
        assert_eq!(rt.share_read_verdict(0, false, true), ReadVerdict::Hold);
        drop(guard);
    }

    /// Locking up ends the session for the same reason it sends a farewell
    /// beacon: a locked vault is not watching anything with anyone.
    #[tokio::test]
    async fn locking_the_vault_ends_any_together_session() {
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path().to_str().unwrap(), "pin-1234")
            .await
            .unwrap();
        *rt.together.lock().unwrap() = Some(TogetherSession {
            id: "s1".into(),
            peer: "npub1peer".into(),
            peer_hex: "ff".into(),
            content: a_film(),
            we_lead: true,
            our_npub: "npub1us".into(),
            joined: true,
            applied: CommandStamp::new(1, "npub1us", false),
            local_pos_ms: 0,
            local_playing: true,
            peer_pos_ms: 0,
            peer_playing: true,
            peer_at_ms: now_ms(),
            last_heard_ms: now_ms(),
            last_seek_ms: 0,
            direct_ready: false,
            direct_evidence_ms: 0,
            local_rate: 1.0,
            local_output_latency_ms: 0,
            peer_output_latency_ms: 0,
            clock: ClockFilter::new(),
            sent_at_ms: std::collections::VecDeque::new(),
            echo_back: None,
        });
        assert!(rt.together_session().is_some());
        rt.lock_vault().await;
        assert!(
            rt.together_session().is_none(),
            "a locked vault must not still be in a session"
        );
    }

    #[tokio::test]
    async fn dispatch_drops_a_stale_call_signal_and_dedups_a_fresh_one() {
        let dir = TempDir::new().unwrap();
        let store = Arc::new(comrade_storage::EncryptedStore::open(dir.path(), "pin").unwrap());
        let vault = test_vault().await;
        let (tx, mut rx) = broadcast::channel(16);
        let dedup = SeenSet::new(CALL_SIGNAL_DEDUP_CAPACITY);
        let outbox = Arc::new(Outbox::new());
        let transport_dedup = SeenSet::with_ttl(
            CROSS_TRANSPORT_DEDUP_CAPACITY,
            std::time::Duration::from_secs(CROSS_TRANSPORT_DEDUP_SECS),
        );
        let route = relay_route(&transport_dedup);
        let (hex, peer) = stranger();
        store
            .set_conversation_meta(&comrade_storage::ConversationMeta {
                peer_npub: peer.clone(),
                state: "accepted".into(),
                profile_shared: true,
                last_read_at: 0,
                updated_at: 1,
            })
            .unwrap();

        let envelope = CallEnvelope::new(
            "call-1",
            CallMediaKind::Audio,
            CallSignal::Offer {
                sdp: "v=0\r\n".into(),
            },
        )
        .to_json()
        .unwrap();

        // A days-old backfilled offer (e.g. relaunch re-scanning the 2-day
        // window) must never ring.
        let mut stale = incoming(&hex, "e1", &envelope);
        stale.created_at = now_secs().saturating_sub(7200);
        dispatch_incoming_dm(&vault, Some(&store), &tx, &dedup, &outbox, &route, stale);
        assert!(
            rx.try_recv().is_err(),
            "a call signal older than CALL_SIGNAL_MAX_AGE_SECS must not reach the bus"
        );

        // A fresh signal reaches the bus once; the exact same wrapper event id
        // redelivered (at-least-once relay delivery) must not fire twice.
        let mut fresh = incoming(&hex, "e2", &envelope);
        fresh.created_at = now_secs();
        dispatch_incoming_dm(
            &vault,
            Some(&store),
            &tx,
            &dedup,
            &outbox,
            &route,
            fresh.clone(),
        );
        assert!(matches!(
            rx.try_recv().unwrap(),
            BridgeEvent::IncomingCallSignal(_)
        ));
        dispatch_incoming_dm(&vault, Some(&store), &tx, &dedup, &outbox, &route, fresh);
        assert!(
            rx.try_recv().is_err(),
            "the same wrapper event id must only ever dispatch one IncomingCallSignal"
        );

        // The regression: this check compared `now` against the *sender's*
        // clock with no tolerance, so a peer whose clock ran a few minutes slow
        // had every call signal dropped as "stale" while their chat — which has
        // no age check — kept arriving. Calls dead, messages fine, from nothing
        // but clock drift.
        let mut skewed = incoming(&hex, "e3", &envelope);
        skewed.created_at = now_secs().saturating_sub(5 * 60);
        dispatch_incoming_dm(&vault, Some(&store), &tx, &dedup, &outbox, &route, skewed);
        assert!(
            matches!(rx.try_recv(), Ok(BridgeEvent::IncomingCallSignal(_))),
            "a live call signal from a peer whose clock is five minutes slow must still ring",
        );

        // …and the same signal from a peer whose clock is *ahead* of ours, which
        // the old `saturating_sub` happened to allow by accident rather than by
        // decision.
        let mut ahead = incoming(&hex, "e4", &envelope);
        ahead.created_at = now_secs() + 5 * 60;
        dispatch_incoming_dm(&vault, Some(&store), &tx, &dedup, &outbox, &route, ahead);
        assert!(
            matches!(rx.try_recv(), Ok(BridgeEvent::IncomingCallSignal(_))),
            "a call signal from a peer whose clock is ahead of ours must ring",
        );
    }

    // ── Ride signals (see `comrade_core::ride`) ───────────────────────────────

    #[tokio::test]
    async fn dispatch_drops_a_stale_ride_signal_and_dedups_a_fresh_one() {
        let dir = TempDir::new().unwrap();
        let store = Arc::new(comrade_storage::EncryptedStore::open(dir.path(), "pin").unwrap());
        let vault = test_vault().await;
        let (tx, mut rx) = broadcast::channel(16);
        let dedup = SeenSet::new(CALL_SIGNAL_DEDUP_CAPACITY);
        let outbox = Arc::new(Outbox::new());
        let transport_dedup = SeenSet::with_ttl(
            CROSS_TRANSPORT_DEDUP_CAPACITY,
            std::time::Duration::from_secs(CROSS_TRANSPORT_DEDUP_SECS),
        );
        let route = relay_route(&transport_dedup);
        let (hex, peer) = stranger();
        store
            .set_conversation_meta(&comrade_storage::ConversationMeta {
                peer_npub: peer.clone(),
                state: "accepted".into(),
                profile_shared: true,
                last_read_at: 0,
                updated_at: 1,
            })
            .unwrap();

        let envelope = RideEnvelope::new(
            RideSignal::Route {
                maneuver: RideManeuver::Left,
                distance_m: Some(400),
                note: Some("after the petrol pump".into()),
            },
            1_754_160_000_123,
        )
        .to_json()
        .unwrap();

        // The worst bug this feature could have: a two-day-old backfilled
        // "left in 400 m" rendered huge on a moving driver's screen.
        let mut stale = incoming(&hex, "r1", &envelope);
        stale.created_at = now_secs().saturating_sub(7200);
        dispatch_incoming_dm(&vault, Some(&store), &tx, &dedup, &outbox, &route, stale);
        assert!(
            rx.try_recv().is_err(),
            "a ride signal older than its TTL must not reach the bus"
        );

        // A fresh signal reaches the bus once, fully decomposed and carrying
        // core's urgency verdict; the same wrapper event id redelivered
        // (at-least-once relay delivery) must not buzz twice for one tap.
        let mut fresh = incoming(&hex, "r2", &envelope);
        fresh.created_at = now_secs();
        dispatch_incoming_dm(
            &vault,
            Some(&store),
            &tx,
            &dedup,
            &outbox,
            &route,
            fresh.clone(),
        );
        let BridgeEvent::RideSignal(dto) = rx.try_recv().unwrap() else {
            panic!("a fresh ride signal must surface as BridgeEvent::RideSignal");
        };
        assert_eq!(dto.peer, peer);
        assert_eq!(dto.kind, "route");
        assert_eq!(dto.maneuver.as_deref(), Some("left"));
        assert_eq!(dto.distance_m, Some(400));
        assert_eq!(dto.note.as_deref(), Some("after the petrol pump"));
        assert_eq!(dto.phrase, None);
        assert_eq!(dto.urgency, "notice");
        dispatch_incoming_dm(&vault, Some(&store), &tx, &dedup, &outbox, &route, fresh);
        assert!(
            rx.try_recv().is_err(),
            "the same wrapper event id must only ever dispatch one RideSignal"
        );
    }

    #[tokio::test]
    async fn one_ride_signal_on_two_transports_raises_one_card_but_a_repeat_still_raises_its_own() {
        // `ride_send` publishes on the local radios *and* the relay, because a
        // ride signal has no heartbeat or outbox to repair a loss. These are
        // the two properties that makes safe: the pair collapses, and a phrase
        // said twice on purpose does not.
        let dir = TempDir::new().unwrap();
        let store = Arc::new(comrade_storage::EncryptedStore::open(dir.path(), "pin").unwrap());
        let vault = test_vault().await;
        let (tx, mut rx) = broadcast::channel(16);
        let dedup = SeenSet::new(CALL_SIGNAL_DEDUP_CAPACITY);
        let outbox = Arc::new(Outbox::new());
        let transport_dedup = SeenSet::with_ttl(
            CROSS_TRANSPORT_DEDUP_CAPACITY,
            std::time::Duration::from_secs(CROSS_TRANSPORT_DEDUP_SECS),
        );
        let (hex, peer) = stranger();
        store
            .set_conversation_meta(&comrade_storage::ConversationMeta {
                peer_npub: peer.clone(),
                state: "accepted".into(),
                profile_shared: true,
                last_read_at: 0,
                updated_at: 1,
            })
            .unwrap();

        let said = |at_ms: u64| {
            RideEnvelope::new(
                RideSignal::Quick {
                    phrase: RidePhrase::PullOver,
                },
                at_ms,
            )
            .to_json()
            .unwrap()
        };

        // One tap, taking both roads. The wrapper event ids differ — they are
        // different deliveries — so only the content comparison can pair them.
        let first = said(1_754_160_000_123);
        let mut over_radio = incoming(&hex, "mesh-1", &first);
        over_radio.created_at = now_secs();
        dispatch_incoming_dm(
            &vault,
            Some(&store),
            &tx,
            &dedup,
            &outbox,
            &DmRoute {
                label: TRANSPORT_MESH,
                dedup: &transport_dedup,
                mesh: None,
                together: None,
            },
            over_radio,
        );
        assert!(
            matches!(rx.try_recv(), Ok(BridgeEvent::RideSignal(_))),
            "the copy that arrives first must raise the card"
        );

        let mut over_relay = incoming(&hex, "relay-1", &first);
        over_relay.created_at = now_secs();
        dispatch_incoming_dm(
            &vault,
            Some(&store),
            &tx,
            &dedup,
            &outbox,
            &relay_route(&transport_dedup),
            over_relay,
        );
        assert!(
            rx.try_recv().is_err(),
            "one tap must not buzz a driver twice because it took two roads"
        );

        // And the trap: the catalog is fixed, so "pull over" said again is the
        // same signal with a different instant. Inside the two-minute dedup
        // window, and it must still arrive — on a motorcycle the repeat is the
        // likely case, not a double-send.
        let mut again = incoming(&hex, "mesh-2", &said(1_754_160_004_500));
        again.created_at = now_secs();
        dispatch_incoming_dm(
            &vault,
            Some(&store),
            &tx,
            &dedup,
            &outbox,
            &DmRoute {
                label: TRANSPORT_MESH,
                dedup: &transport_dedup,
                mesh: None,
                together: None,
            },
            again,
        );
        assert!(
            matches!(rx.try_recv(), Ok(BridgeEvent::RideSignal(_))),
            "a phrase deliberately repeated must raise its own card"
        );
    }

    #[tokio::test]
    async fn a_strangers_ride_signal_is_dropped_not_surfaced() {
        // The gate that matters most: an unaccepted peer must not be able to
        // put "pull over" on a moving driver's screen — and the attempt must
        // not fall through to the message-request path either, which would
        // surface a request preview full of JSON.
        let dir = TempDir::new().unwrap();
        let store = Arc::new(comrade_storage::EncryptedStore::open(dir.path(), "pin").unwrap());
        let vault = test_vault().await;
        let (tx, mut rx) = broadcast::channel(16);
        let dedup = SeenSet::new(CALL_SIGNAL_DEDUP_CAPACITY);
        let outbox = Arc::new(Outbox::new());
        let transport_dedup = SeenSet::with_ttl(
            CROSS_TRANSPORT_DEDUP_CAPACITY,
            std::time::Duration::from_secs(CROSS_TRANSPORT_DEDUP_SECS),
        );
        let route = relay_route(&transport_dedup);
        let (hex, _peer) = stranger();

        let envelope = RideEnvelope::new(
            RideSignal::Quick {
                phrase: RidePhrase::PullOver,
            },
            1_754_160_000_123,
        )
        .to_json()
        .unwrap();
        let mut msg = incoming(&hex, "r3", &envelope);
        msg.created_at = now_secs();
        dispatch_incoming_dm(&vault, Some(&store), &tx, &dedup, &outbox, &route, msg);
        assert!(
            rx.try_recv().is_err(),
            "an unaccepted peer's ride signal must not surface at all"
        );
    }

    // ── T2: DM dedup + durable receive watermark ─────────────────────────────

    #[tokio::test]
    async fn dispatch_dedups_a_redelivered_plain_text_dm() {
        let dir = TempDir::new().unwrap();
        let store = Arc::new(comrade_storage::EncryptedStore::open(dir.path(), "pin").unwrap());
        let vault = test_vault().await;
        let (tx, mut rx) = broadcast::channel(16);
        let dedup = SeenSet::new(CALL_SIGNAL_DEDUP_CAPACITY);
        let outbox = Arc::new(Outbox::new());
        let transport_dedup = SeenSet::with_ttl(
            CROSS_TRANSPORT_DEDUP_CAPACITY,
            std::time::Duration::from_secs(CROSS_TRANSPORT_DEDUP_SECS),
        );
        let route = relay_route(&transport_dedup);
        let (hex, peer) = stranger();
        store
            .set_conversation_meta(&comrade_storage::ConversationMeta {
                peer_npub: peer.clone(),
                state: "accepted".into(),
                profile_shared: true,
                last_read_at: 0,
                updated_at: 1,
            })
            .unwrap();

        let msg = incoming(&hex, "dup1", "hello twice");
        dispatch_incoming_dm(
            &vault,
            Some(&store),
            &tx,
            &dedup,
            &outbox,
            &route,
            msg.clone(),
        );
        assert!(matches!(
            rx.try_recv().unwrap(),
            BridgeEvent::IncomingDirectMessage(_)
        ));
        assert_eq!(store.messages_with(&peer).unwrap().len(), 1);

        // Redelivered (same event id — relay at-least-once, or a backfill
        // re-scan on the next launch) must not re-notify or duplicate the row.
        dispatch_incoming_dm(&vault, Some(&store), &tx, &dedup, &outbox, &route, msg);
        assert!(
            rx.try_recv().is_err(),
            "an already-persisted event id must not re-fire IncomingDirectMessage"
        );
        assert_eq!(store.messages_with(&peer).unwrap().len(), 1);
    }

    #[tokio::test]
    async fn dispatch_dedups_a_redelivered_media_envelope() {
        let dir = TempDir::new().unwrap();
        let store = Arc::new(comrade_storage::EncryptedStore::open(dir.path(), "pin").unwrap());
        let vault = test_vault().await;
        let (tx, mut rx) = broadcast::channel(16);
        let dedup = SeenSet::new(CALL_SIGNAL_DEDUP_CAPACITY);
        let outbox = Arc::new(Outbox::new());
        let transport_dedup = SeenSet::with_ttl(
            CROSS_TRANSPORT_DEDUP_CAPACITY,
            std::time::Duration::from_secs(CROSS_TRANSPORT_DEDUP_SECS),
        );
        let route = relay_route(&transport_dedup);
        let (hex, peer) = stranger();
        store
            .set_conversation_meta(&comrade_storage::ConversationMeta {
                peer_npub: peer.clone(),
                state: "accepted".into(),
                profile_shared: true,
                last_read_at: 0,
                updated_at: 1,
            })
            .unwrap();

        let envelope = MediaEnvelope {
            comrade_media: 1,
            event_id: "media1".into(),
            url: "https://blob.example/x".into(),
            mime: "image/png".into(),
            caption: "pic".into(),
            size: 10,
            sha256_hex: "a".repeat(64),
        };
        let json = serde_json::to_string(&envelope).unwrap();

        dispatch_incoming_dm(
            &vault,
            Some(&store),
            &tx,
            &dedup,
            &outbox,
            &route,
            incoming(&hex, "w1", &json),
        );
        assert!(matches!(
            rx.try_recv().unwrap(),
            BridgeEvent::IncomingMedia(_)
        ));

        // Redelivered wrapper (same NIP-94 event id) must not re-notify.
        dispatch_incoming_dm(
            &vault,
            Some(&store),
            &tx,
            &dedup,
            &outbox,
            &route,
            incoming(&hex, "w2", &json),
        );
        assert!(
            rx.try_recv().is_err(),
            "an already-persisted media event id must not re-fire IncomingMedia"
        );
    }

    #[test]
    fn vault_watermark_round_trips_and_only_advances() {
        let dir = TempDir::new().unwrap();
        let store = comrade_storage::EncryptedStore::open(dir.path(), "pin").unwrap();
        assert_eq!(read_watermark(&store), None);

        advance_watermark(&store, 100);
        assert_eq!(read_watermark(&store), Some(100));

        // Out-of-order delivery must never move the watermark backwards.
        advance_watermark(&store, 50);
        assert_eq!(read_watermark(&store), Some(100));

        advance_watermark(&store, 200);
        assert_eq!(read_watermark(&store), Some(200));
    }

    #[tokio::test]
    async fn dispatch_advances_the_watermark_for_every_message_kind() {
        let dir = TempDir::new().unwrap();
        let store = Arc::new(comrade_storage::EncryptedStore::open(dir.path(), "pin").unwrap());
        let vault = test_vault().await;
        let (tx, _rx) = broadcast::channel(16);
        let dedup = SeenSet::new(CALL_SIGNAL_DEDUP_CAPACITY);
        let outbox = Arc::new(Outbox::new());
        let transport_dedup = SeenSet::with_ttl(
            CROSS_TRANSPORT_DEDUP_CAPACITY,
            std::time::Duration::from_secs(CROSS_TRANSPORT_DEDUP_SECS),
        );
        let route = relay_route(&transport_dedup);
        let (hex, _peer) = stranger();

        let mut msg = incoming(&hex, "e1", "hi");
        msg.created_at = 12_345;
        dispatch_incoming_dm(&vault, Some(&store), &tx, &dedup, &outbox, &route, msg);
        assert_eq!(read_watermark(&store), Some(12_345));
    }

    // ── T4: no runtime lock held across a network await ──────────────────────

    #[tokio::test]
    async fn toggle_workspace_is_not_blocked_by_a_slow_send_dm() {
        // AUDIT P2 regression guard: send_dm must not hold the shared runtime
        // lock across its relay round-trip, or one slow/unreachable relay
        // freezes every other bridge command behind it. Points the vault
        // engine at a non-routable address (RFC 5737 TEST-NET-1) and never
        // calls `spawn_event_loops`, so the relay never connects and
        // `send_dm`'s internal `wait_for_any_relay` blocks for its full ~5s
        // bound — long enough that this test would time out if the fix
        // regressed.
        let dir = TempDir::new().unwrap();
        let rt = Arc::new(tokio::sync::RwLock::new(ComradeRuntime::with_relays(vec![
            "wss://192.0.2.1:9".to_string(),
        ])));
        rt.write()
            .await
            .unlock_vault(dir.path(), "pin")
            .await
            .unwrap();

        let (_hex, peer) = stranger();
        let send_rt = rt.clone();
        let send_task = tokio::spawn(async move {
            // The guard-scoped snapshot bridges take — see `ComradeRuntime::handles`.
            let handles = send_rt.read().await.handles();
            handles.send_dm(&peer, "hello").await
        });

        // Give the send a moment to actually start (and start blocking on the
        // relay wait) before racing the write-locking command against it.
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        let toggle_rt = rt.clone();
        let toggled = tokio::time::timeout(std::time::Duration::from_secs(1), async move {
            toggle_rt
                .write()
                .await
                .toggle_workspace("OffGridTravel")
                .await
        })
        .await;

        assert!(
            toggled.is_ok(),
            "toggle_workspace must not be stuck behind a slow send_dm holding the runtime lock"
        );
        assert!(toggled.unwrap().is_ok());

        send_task.abort();
    }

    // ── Store and forward (adopted from bitchat, see docs/BITCHAT_ADOPTION.md) ──

    /// A runtime with no relays at all: every publish fails, which is exactly
    /// the "you are offline" case the outbox exists for.
    async fn offline_runtime(dir: &TempDir) -> ComradeRuntime {
        let mut rt = ComradeRuntime::with_relays(vec![]);
        rt.unlock_vault(dir.path(), "pin").await.unwrap();
        rt
    }

    #[tokio::test]
    async fn a_dm_that_no_relay_accepts_is_queued_not_lost() {
        let dir = TempDir::new().unwrap();
        let rt = offline_runtime(&dir).await;
        let (_hex, peer) = stranger();

        let dto = rt.send_dm(&peer, "i had a hard day").await.unwrap();

        assert_eq!(
            dto.status.as_deref(),
            Some("queued"),
            "the user must see it pending, not see an error and lose the text"
        );
        assert!(comrade_core::dak::outbox::is_local_message_id(&dto.id));
        assert_eq!(rt.outbox_pending(), 1);

        // It is in the conversation history too, so the thread renders it.
        let history = rt.messages_with(&peer).unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].content, "i had a hard day");
        assert_eq!(history[0].status.as_deref(), Some("queued"));
    }

    #[tokio::test]
    async fn queued_mail_survives_a_lock_and_unlock() {
        let dir = TempDir::new().unwrap();
        let (_hex, peer) = stranger();
        {
            let mut rt = offline_runtime(&dir).await;
            rt.send_dm(&peer, "are you around?").await.unwrap();
            assert_eq!(rt.outbox_pending(), 1);
            rt.lock_vault().await;
        }

        let mut rt = ComradeRuntime::with_relays(vec![]);
        rt.unlock_vault(dir.path(), "pin").await.unwrap();
        assert_eq!(
            rt.outbox_pending(),
            1,
            "queued mail must survive an app kill — that is the whole point"
        );
        assert_eq!(
            rt.messages_with(&peer).unwrap()[0].content,
            "are you around?"
        );
    }

    #[tokio::test]
    async fn a_receipt_clears_queued_mail_so_a_flush_cannot_resend_it() {
        let dir = TempDir::new().unwrap();
        let store = Arc::new(comrade_storage::EncryptedStore::open(dir.path(), "pin").unwrap());
        let vault = test_vault().await;
        let (tx, _rx) = broadcast::channel(16);
        let dedup = SeenSet::new(CALL_SIGNAL_DEDUP_CAPACITY);
        let outbox = Arc::new(Outbox::new());
        let transport_dedup = SeenSet::with_ttl(
            CROSS_TRANSPORT_DEDUP_CAPACITY,
            std::time::Duration::from_secs(CROSS_TRANSPORT_DEDUP_SECS),
        );
        let route = relay_route(&transport_dedup);
        let (hex, peer) = stranger();
        store
            .set_conversation_meta(&comrade_storage::ConversationMeta {
                peer_npub: peer.clone(),
                state: "accepted".into(),
                profile_shared: true,
                last_read_at: 0,
                updated_at: 1,
            })
            .unwrap();

        outbox.queue(QueuedMessage::new("out1", &peer, "sup", None, 1));
        store
            .save_message(&comrade_storage::StoredMessage {
                id: "out1".into(),
                peer_npub: peer.clone(),
                content: "sup".into(),
                created_at: 1,
                outgoing: true,
                status: Some("sent".into()),
                reply_to: None,
            })
            .unwrap();

        let receipt = Receipt::new(ReceiptKind::Read, vec!["out1".into()])
            .to_json()
            .unwrap();
        dispatch_incoming_dm(
            &vault,
            Some(&store),
            &tx,
            &dedup,
            &outbox,
            &route,
            incoming(&hex, "e-receipt", &receipt),
        );

        assert!(
            outbox.is_empty(),
            "the peer has the message; retrying it would send a duplicate"
        );
        assert_eq!(
            store
                .get_message("out1")
                .unwrap()
                .unwrap()
                .status
                .as_deref(),
            Some("read")
        );
    }

    #[tokio::test]
    async fn flush_marks_a_message_failed_once_attempts_run_out() {
        let dir = TempDir::new().unwrap();
        let rt = offline_runtime(&dir).await;
        let (_hex, peer) = stranger();
        let dto = rt.send_dm(&peer, "still trying").await.unwrap();

        // Burn the attempt budget directly: a real flush would take one relay
        // timeout per attempt.
        for _ in 0..comrade_core::dak::outbox::MAX_ATTEMPTS {
            rt.outbox.record_attempt(&peer, &dto.id);
        }
        assert_eq!(rt.outbox_pending(), 0, "the attempt cap drops the message");

        // The flush loop's reaping pass is what makes that visible in the UI.
        rt.handles()
            .mark_status(&peer, std::slice::from_ref(&dto.id), "failed");
        assert_eq!(
            rt.messages_with(&peer).unwrap()[0].status.as_deref(),
            Some("failed"),
            "a dropped message must stop showing as in flight"
        );
    }

    #[tokio::test]
    async fn a_queued_media_reference_retries_like_text_and_never_becomes_a_chat_bubble() {
        // The reference to an uploaded blob is ordinary DM content, so it rides
        // the same store-and-forward queue as text. What it must *not* do is
        // acquire a stored message row: the flush loop's re-key step would turn
        // the envelope's JSON into a chat bubble and the chat list's preview.
        let dir = TempDir::new().unwrap();
        let rt = offline_runtime(&dir).await;
        let (_hex, peer) = stranger();
        let envelope = serde_json::to_string(&MediaEnvelope {
            comrade_media: 1,
            event_id: "m1".into(),
            url: "https://blob.example/x".into(),
            mime: "image/jpeg".into(),
            caption: "sunset.jpg".into(),
            size: 1234,
            sha256_hex: "a".repeat(64),
        })
        .unwrap();
        rt.outbox
            .queue(QueuedMessage::new("m1", &peer, &envelope, None, now_secs()));

        // No relay will take it: kept and retried, same as text.
        assert_eq!(rt.flush_outbox().await.unwrap(), 0);
        assert_eq!(
            rt.outbox_pending(),
            1,
            "a media reference no relay accepted must be retried, not dropped"
        );
        assert!(
            rt.messages_with(&peer).unwrap().is_empty(),
            "a media envelope must never appear in the thread as text"
        );
    }

    #[tokio::test]
    async fn an_expired_media_reference_is_reported_failed_against_its_conversation() {
        // The reaping pass names a failed message by id and looks its peer up
        // from the stored row. A media reference has no stored row, so before
        // this its expiry was silent — the sender was never told the photo
        // never went out.
        let dir = TempDir::new().unwrap();
        let rt = offline_runtime(&dir).await;
        let (_hex, peer) = stranger();
        let envelope = serde_json::to_string(&MediaEnvelope {
            comrade_media: 1,
            event_id: "m1".into(),
            url: "https://blob.example/x".into(),
            mime: "image/jpeg".into(),
            caption: String::new(),
            size: 1,
            sha256_hex: String::new(),
        })
        .unwrap();
        let stale = now_secs() - comrade_core::dak::outbox::TTL_SECS - 1;
        rt.outbox
            .queue(QueuedMessage::new("m1", &peer, &envelope, None, stale));

        let mut rx = rt.subscribe_events();
        rt.flush_outbox().await.unwrap();
        assert_eq!(rt.outbox_pending(), 0, "expired mail is reaped");
        match rx.try_recv().unwrap() {
            BridgeEvent::MessageStatus {
                peer: event_peer,
                message_ids,
                status,
            } => {
                assert_eq!(event_peer, peer);
                assert_eq!(message_ids, vec!["m1".to_string()]);
                assert_eq!(status, STATUS_FAILED);
            }
            other => panic!("expected a failed MessageStatus, got {other:?}"),
        }
    }

    /// The other half of the off-grid report: mail queued with no network at
    /// all was marked failed after about eight minutes.
    ///
    /// [`comrade_core::dak::outbox::MAX_ATTEMPTS`] is 8 and the flush cadence
    /// is a minute, so eight ticks against a network that was not there
    /// exhausted the cap and turned the thread red — silently overriding the
    /// 24-hour [`comrade_core::dak::outbox::TTL_SECS`] that is supposed to
    /// decide how long off-grid mail waits. Two people out of range for a
    /// coffee break came back to failed messages.
    ///
    /// An attempt has to mean "a delivery that failed", not "a minute passed".
    #[tokio::test]
    async fn mail_with_nowhere_to_go_waits_for_the_ttl_instead_of_burning_the_attempt_cap() {
        let dir = TempDir::new().unwrap();
        let rt = offline_runtime(&dir).await;
        let (_hex, peer) = stranger();
        rt.outbox.queue(QueuedMessage::new(
            "q1",
            &peer,
            "still here?",
            None,
            now_secs(),
        ));

        // Comfortably more rounds than the cap. No relay is configured and
        // nobody is on the local network, so every one of them has nothing to
        // try.
        for _ in 0..(comrade_core::dak::outbox::MAX_ATTEMPTS + 3) {
            assert_eq!(rt.flush_outbox().await.unwrap(), 0);
        }

        assert_eq!(
            rt.outbox_pending(),
            1,
            "mail with no route must still be queued after more flushes than \
             the attempt cap allows"
        );
        assert_eq!(
            rt.outbox.pending_for(&peer)[0].attempts,
            0,
            "and none of those rounds should have counted as a failed delivery"
        );
    }

    // ── Bluetooth transport ─────────────────────────────────────────────────

    /// Envelopes must survive the BLE round trip byte-for-byte, because what
    /// comes out the far end is fed to `open_dm` — and a single flipped byte
    /// fails the AEAD rather than degrading gracefully.
    #[test]
    fn a_sealed_envelope_survives_fragmentation_and_reassembly() {
        use comrade_core::crypto::KeyProfile;

        let alice = KeyProfile::generate().unwrap().keys;
        let bob = KeyProfile::generate().unwrap().keys;
        let now = 1_700_000_000;
        let dm = MeshDm::new("queued:ble", "no router out here", None, now);
        let sealed = seal_dm(&bob.public_key(), &alice, &dm, now).expect("seal");

        let packets = ble::fragment(&sealed.encode(), 1, ble::DEFAULT_TTL, 185).expect("fragment");
        let mut r = Reassembler::new();
        let mut rebuilt = None;
        for p in packets {
            let decoded = ble::Fragment::decode(&p.encode()).expect("our own packet");
            rebuilt = r.accept(decoded, now).or(rebuilt);
        }
        let envelope = Envelope::decode(&rebuilt.expect("reassembled")).expect("an envelope");

        let opened = open_dm(&bob, &envelope, now)
            .expect("well-formed")
            .expect("addressed to bob");
        assert_eq!(opened.dm.content, "no router out here");
        assert_eq!(
            opened.sender,
            alice.public_key(),
            "the inner MAC must still identify the real sender after a trip \
             through the fragmenter"
        );
    }

    #[tokio::test]
    async fn bluetooth_is_not_a_route_until_a_radio_says_it_is() {
        let dir = TempDir::new().unwrap();
        let rt = offline_runtime(&dir).await;
        let ble = rt.ble_router();

        assert!(!ble.is_active(), "no platform radio has reported in");
        let (_hex, peer) = stranger();
        let dm = MeshDm::new("x", "hello", None, now_secs());
        let sealed = seal_dm(
            &parse_pubkey(&peer).unwrap(),
            &rt.ui.identity_keys().unwrap(),
            &dm,
            now_secs(),
        )
        .unwrap();
        assert!(
            !ble.enqueue(&sealed),
            "queueing packets for a radio that is not there would grow forever"
        );
        assert!(ble.drain_outbound().is_empty());

        ble.set_active(true);
        assert!(ble.enqueue(&sealed), "now there is somewhere for it to go");
        assert!(!ble.drain_outbound().is_empty());
    }

    #[tokio::test]
    async fn a_relay_forwards_every_fragment_of_a_packet_not_just_the_first() {
        // Regression test. The flood filter was keyed on `packet_id`, which
        // every fragment of one envelope shares — so a relay forwarded fragment
        // 0 and then dropped 1..n as echoes of it. One-hop delivery was
        // unaffected, which is why it looked fine; a message crossing a middle
        // device arrived permanently incomplete, and only when it was too big
        // for a single fragment, which is most messages.
        let dir = TempDir::new().unwrap();
        let rt = offline_runtime(&dir).await;
        let ble = rt.ble_router();
        ble.set_active(true);

        // Long enough to be certain it spans several fragments at any MTU the
        // radio might negotiate — the single-fragment case cannot show the bug.
        let (_hex, peer) = stranger();
        let dm = MeshDm::new("x", "a horse walked in ".repeat(200), None, now_secs());
        let sealed = seal_dm(
            &parse_pubkey(&peer).unwrap(),
            &rt.ui.identity_keys().unwrap(),
            &dm,
            now_secs(),
        )
        .unwrap();

        // Fragments as they would arrive at a device that is not the addressee.
        assert!(ble.enqueue(&sealed));
        let inbound = ble.drain_outbound();
        assert!(
            inbound.len() > 1,
            "this test is meaningless unless the envelope actually fragments"
        );

        let now = now_secs();
        for packet in &inbound {
            ble.deliver(packet, now);
        }

        let forwarded = ble.drain_outbound();
        assert_eq!(
            forwarded.len(),
            inbound.len(),
            "every fragment must be forwarded, not just the first — the \
             recipient cannot reassemble from one piece"
        );

        // And the filter still does its job: a second copy of the same
        // fragments is an echo and must not go round again.
        for packet in &inbound {
            ble.deliver(packet, now);
        }
        assert!(
            ble.drain_outbound().is_empty(),
            "re-hearing the same fragments must not re-flood them"
        );
    }

    #[tokio::test]
    async fn turning_the_radio_off_drops_what_it_never_sent() {
        // The outbox still holds every message, so keeping stale packets would
        // only replay an old burst on the next connection.
        let dir = TempDir::new().unwrap();
        let rt = offline_runtime(&dir).await;
        let ble = rt.ble_router();
        ble.set_active(true);

        let (_hex, peer) = stranger();
        let dm = MeshDm::new("x", "hello", None, now_secs());
        let sealed = seal_dm(
            &parse_pubkey(&peer).unwrap(),
            &rt.ui.identity_keys().unwrap(),
            &dm,
            now_secs(),
        )
        .unwrap();
        ble.enqueue(&sealed);

        ble.set_active(false);
        assert!(ble.drain_outbound().is_empty());
    }

    /// Forwarding what we cannot read *is* the mesh: a frame for someone two
    /// hops away only gets there because the device in the middle passes it on.
    #[tokio::test]
    async fn a_frame_for_someone_else_is_relayed_but_never_relayed_twice() {
        let dir = TempDir::new().unwrap();
        let rt = offline_runtime(&dir).await;
        let ble = rt.ble_router();
        ble.set_active(true);

        // Sealed between two strangers — we cannot open it, and must still
        // carry it.
        let a = comrade_core::crypto::KeyProfile::generate().unwrap().keys;
        let b = comrade_core::crypto::KeyProfile::generate().unwrap().keys;
        let now = now_secs();
        let dm = MeshDm::new("q:1", "not for you", None, now);
        let sealed = seal_dm(&b.public_key(), &a, &dm, now).unwrap();
        let packet =
            ble::fragment(&sealed.encode(), 99, ble::DEFAULT_TTL, 185).unwrap()[0].encode();

        ble.deliver(&packet, now);
        let relayed = ble.drain_outbound();
        assert_eq!(relayed.len(), 1, "we should have passed it on");
        let onward = ble::Fragment::decode(&relayed[0]).unwrap();
        assert_eq!(
            onward.ttl,
            ble::DEFAULT_TTL - 1,
            "one hop spent, so it cannot circulate forever"
        );

        // The same packet again — from another neighbour who also heard it.
        ble.deliver(&packet, now);
        assert!(
            ble.drain_outbound().is_empty(),
            "relaying a packet we already relayed is how a broadcast storm starts"
        );
    }

    #[tokio::test]
    async fn a_spent_packet_is_delivered_but_not_forwarded() {
        let dir = TempDir::new().unwrap();
        let rt = offline_runtime(&dir).await;
        let ble = rt.ble_router();
        ble.set_active(true);

        let a = comrade_core::crypto::KeyProfile::generate().unwrap().keys;
        let b = comrade_core::crypto::KeyProfile::generate().unwrap().keys;
        let now = now_secs();
        let dm = MeshDm::new("q:2", "last hop", None, now);
        let sealed = seal_dm(&b.public_key(), &a, &dm, now).unwrap();
        // TTL 1: this device is the end of the line.
        let packet = ble::fragment(&sealed.encode(), 7, 1, 185).unwrap()[0].encode();

        ble.deliver(&packet, now);
        assert!(
            ble.drain_outbound().is_empty(),
            "a packet with no hops left must stop here"
        );
    }

    /// The whole point: with no relay and no WiFi mesh, a message still leaves
    /// the device — over Bluetooth — and stays queued until a receipt clears it.
    #[tokio::test]
    async fn with_only_bluetooth_a_dm_still_goes_out_and_stays_queued() {
        let dir = TempDir::new().unwrap();
        let rt = offline_runtime(&dir).await;
        rt.ble_router().set_active(true);
        let (_hex, peer) = stranger();

        let dto = rt.send_dm(&peer, "no router for miles").await.unwrap();

        assert_eq!(
            dto.status.as_deref(),
            Some("queued"),
            "a BLE publish reaching *a* device is not proof the recipient got \
             it — only their receipt clears the outbox"
        );
        assert_eq!(rt.outbox_pending(), 1);
        assert!(
            !rt.ble_router().drain_outbound().is_empty(),
            "and the packets must actually be waiting for the radio"
        );
    }

    /// Bluetooth alone has to count as a local route, or precedence would keep
    /// leading with a relay that is not there.
    #[tokio::test]
    async fn bluetooth_alone_makes_the_local_route_available() {
        let dir = TempDir::new().unwrap();
        let rt = offline_runtime(&dir).await;
        let handles = rt.handles();
        let vault = rt.vault.clone().unwrap();

        assert!(
            !handles.reach(&vault).await.mesh,
            "no radio, no local route"
        );
        rt.ble_router().set_active(true);
        assert!(
            handles.reach(&vault).await.mesh,
            "with Bluetooth up, 'nearby' is reachable even with no WiFi at all"
        );
    }

    #[tokio::test]
    async fn flush_with_an_empty_outbox_is_a_no_op() {
        let dir = TempDir::new().unwrap();
        let rt = offline_runtime(&dir).await;
        assert_eq!(rt.flush_outbox().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn flush_requires_an_unlocked_vault() {
        let rt = ComradeRuntime::with_relays(vec![]);
        assert!(matches!(rt.flush_outbox().await, Err(UiError::VaultLocked)));
    }

    // ── Local-network delivery (the "no internet, same WiFi" path) ──────────

    #[tokio::test]
    async fn the_mesh_comes_up_on_unlock_without_choosing_a_workspace() {
        // The bug this fixes: LAN delivery used to require the user to switch to
        // the OffGridTravel workspace, and even then carried no DMs.
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::with_relays(vec![]);
        assert!(!rt.mesh_status().active, "nothing running before unlock");

        rt.unlock_vault(dir.path(), "pin").await.unwrap();
        assert_eq!(
            rt.current_workspace().key,
            "Base",
            "still the ordinary workspace"
        );
        assert!(
            rt.mesh_status().active,
            "an unlocked vault must have a local-network transport"
        );
        assert!(
            rt.handles().mesh.is_some(),
            "and the send path must be able to reach it"
        );

        // Locking takes it back down: no beacons, no frames, nothing listening.
        rt.lock_vault().await;
        assert!(!rt.mesh_status().active);
    }

    #[tokio::test]
    async fn a_dm_with_no_relay_is_sealed_onto_the_mesh_and_stays_queued() {
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::with_relays(vec![]);
        rt.unlock_vault(dir.path(), "pin").await.unwrap();
        let (_hex, peer) = stranger();

        let dto = rt.send_dm(&peer, "i'm outside, no signal").await.unwrap();

        // Queued, because a mesh publish is not proof the *recipient* got it —
        // only their receipt clears the outbox.
        assert_eq!(dto.status.as_deref(), Some("queued"));
        assert_eq!(rt.outbox_pending(), 1);
        assert_eq!(
            rt.messages_with(&peer).unwrap()[0].content,
            "i'm outside, no signal"
        );
    }

    const BOTH_UP: TransportReach = TransportReach {
        relay: true,
        mesh: true,
    };
    const ONLY_RELAY: TransportReach = TransportReach {
        relay: true,
        mesh: false,
    };
    const ONLY_MESH: TransportReach = TransportReach {
        relay: false,
        mesh: true,
    };
    const NOTHING_UP: TransportReach = TransportReach {
        relay: false,
        mesh: false,
    };

    /// The precedence policy behind the app-bar switch, as a table.
    #[test]
    fn precedence_orders_the_transports_and_stops_waiting_after_two_rounds() {
        // Relays lead by default, and a message a relay has *stored* is not
        // worth also flooding onto the WiFi — so relay precedence never sends
        // twice, however many rounds it takes.
        assert_eq!(
            SendPlan::for_attempt(false, BOTH_UP, 0),
            SendPlan {
                local_first: false,
                force_both: false
            }
        );
        assert!(!SendPlan::for_attempt(false, BOTH_UP, LOCAL_FIRST_PATIENCE + 5).force_both);

        // Local precedence: the mesh goes first, alone at first…
        assert_eq!(
            SendPlan::for_attempt(true, BOTH_UP, 0),
            SendPlan {
                local_first: true,
                force_both: false
            }
        );
        // …but a mesh publish only means *someone* took the frame, so a message
        // still unacknowledged after a couple of rounds stops waiting for the
        // recipient to walk into range and goes out over a relay too.
        assert!(SendPlan::for_attempt(true, BOTH_UP, LOCAL_FIRST_PATIENCE).force_both);
        assert!(SendPlan::for_attempt(true, BOTH_UP, LOCAL_FIRST_PATIENCE + 1).force_both);
    }

    /// Availability outranks the setting, because a route that is down is not
    /// a route — leading with it only buys the dead transport's timeout.
    #[test]
    fn a_route_that_is_down_never_goes_first_whatever_the_user_picked() {
        for prefer_local in [true, false] {
            assert!(
                SendPlan::for_attempt(prefer_local, ONLY_MESH, 0).local_first,
                "with no relay connected, the local network must lead \
                 (prefer_local={prefer_local}) — this is the airplane-mode case, \
                 where leading with a relay costs a five-second connect wait \
                 before the WiFi is even tried"
            );
            assert!(
                !SendPlan::for_attempt(prefer_local, ONLY_RELAY, 0).local_first,
                "with nobody on this network, a relay must lead \
                 (prefer_local={prefer_local})"
            );
        }
    }

    /// The setting is a tie-break, and it only has ties to break when both
    /// routes are actually up.
    #[test]
    fn the_app_bar_setting_decides_only_when_both_routes_work() {
        assert!(SendPlan::for_attempt(true, BOTH_UP, 0).local_first);
        assert!(!SendPlan::for_attempt(false, BOTH_UP, 0).local_first);
    }

    /// With neither route up the message is going to the outbox regardless, so
    /// the order must stay stable rather than flapping on a probe that means
    /// nothing.
    #[test]
    fn with_nothing_reachable_the_order_is_whatever_was_asked_for() {
        assert!(SendPlan::for_attempt(true, NOTHING_UP, 0).local_first);
        assert!(!SendPlan::for_attempt(false, NOTHING_UP, 0).local_first);
    }

    /// `force_both` follows the route that actually led, not the setting.
    /// Otherwise a relay-preferring user whose relays are down would sit on the
    /// mesh forever, never escalating — the exact stall the patience counter
    /// exists to prevent.
    #[test]
    fn patience_runs_out_on_whichever_route_led() {
        let plan = SendPlan::for_attempt(false, ONLY_MESH, LOCAL_FIRST_PATIENCE);
        assert!(plan.local_first, "availability put the mesh first");
        assert!(
            plan.force_both,
            "so the escalation to a relay must arm too, even though the user \
             never asked for local precedence"
        );
    }

    /// The switch has to reach the router, or the app-bar icons are decoration.
    #[tokio::test]
    async fn switching_precedence_changes_the_route_a_send_takes() {
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::with_relays(vec![]);
        rt.unlock_vault(dir.path(), "pin").await.unwrap();
        assert!(!rt.handles().prefer_local, "relays lead by default");

        rt.toggle_workspace("OffGridTravel").await.unwrap();
        assert!(rt.handles().prefer_local);

        // Precedence is an order, not an exclusion: with the preferred route
        // carrying nothing (no peers on this network) *and* no relay, the
        // message is still queued rather than dropped on the floor.
        let (_hex, peer) = stranger();
        let dto = rt.send_dm(&peer, "still going out").await.unwrap();
        assert_eq!(dto.status.as_deref(), Some("queued"));
        assert_eq!(rt.outbox_pending(), 1);
        assert_eq!(
            rt.messages_with(&peer).unwrap()[0].content,
            "still going out"
        );

        rt.toggle_workspace("Base").await.unwrap();
        assert!(!rt.handles().prefer_local);
    }

    /// The cross-transport case: the same message arrives sealed over the mesh
    /// and later over a relay, under two different ids. It must render once.
    #[tokio::test]
    async fn one_message_delivered_by_both_routes_appears_once() {
        let dir = TempDir::new().unwrap();
        let store = Arc::new(comrade_storage::EncryptedStore::open(dir.path(), "pin").unwrap());
        let vault = test_vault().await;
        let (tx, mut rx) = broadcast::channel(16);
        let dedup = SeenSet::new(CALL_SIGNAL_DEDUP_CAPACITY);
        let outbox = Arc::new(Outbox::new());
        let transport_dedup = SeenSet::with_ttl(
            CROSS_TRANSPORT_DEDUP_CAPACITY,
            std::time::Duration::from_secs(CROSS_TRANSPORT_DEDUP_SECS),
        );
        let (hex, peer) = stranger();
        store
            .set_conversation_meta(&comrade_storage::ConversationMeta {
                peer_npub: peer.clone(),
                state: "accepted".into(),
                profile_shared: true,
                updated_at: 1,
                last_read_at: 0,
            })
            .unwrap();

        // Over the mesh first, keyed by the sender's local id.
        let mesh_route = DmRoute {
            label: TRANSPORT_MESH,
            dedup: &transport_dedup,
            mesh: None,
            together: None,
        };
        dispatch_incoming_dm(
            &vault,
            Some(&store),
            &tx,
            &dedup,
            &outbox,
            &mesh_route,
            incoming(&hex, "queued:abc", "are you ok?"),
        );
        assert!(matches!(
            rx.try_recv().unwrap(),
            BridgeEvent::IncomingDirectMessage(_)
        ));

        // Then the relay comes back and delivers the same text under the event
        // id a relay assigned it.
        dispatch_incoming_dm(
            &vault,
            Some(&store),
            &tx,
            &dedup,
            &outbox,
            &relay_route(&transport_dedup),
            incoming(&hex, "e-relay-id", "are you ok?"),
        );
        assert!(
            rx.try_recv().is_err(),
            "the second copy must not reach the UI"
        );
        assert_eq!(
            store.messages_with(&peer).unwrap().len(),
            1,
            "and must not be stored twice"
        );
    }

    /// The flip side, and the reason the dedup key includes the transport:
    /// someone genuinely saying the same thing twice is not a duplicate.
    #[tokio::test]
    async fn the_same_text_sent_twice_over_one_route_is_two_messages() {
        let dir = TempDir::new().unwrap();
        let store = Arc::new(comrade_storage::EncryptedStore::open(dir.path(), "pin").unwrap());
        let vault = test_vault().await;
        let (tx, mut rx) = broadcast::channel(16);
        let dedup = SeenSet::new(CALL_SIGNAL_DEDUP_CAPACITY);
        let outbox = Arc::new(Outbox::new());
        let transport_dedup = SeenSet::with_ttl(
            CROSS_TRANSPORT_DEDUP_CAPACITY,
            std::time::Duration::from_secs(CROSS_TRANSPORT_DEDUP_SECS),
        );
        let (hex, peer) = stranger();
        store
            .set_conversation_meta(&comrade_storage::ConversationMeta {
                peer_npub: peer.clone(),
                state: "accepted".into(),
                profile_shared: true,
                updated_at: 1,
                last_read_at: 0,
            })
            .unwrap();

        for id in ["e1", "e2"] {
            dispatch_incoming_dm(
                &vault,
                Some(&store),
                &tx,
                &dedup,
                &outbox,
                &relay_route(&transport_dedup),
                incoming(&hex, id, "ok"),
            );
        }

        let delivered = std::iter::from_fn(|| rx.try_recv().ok())
            .filter(|e| matches!(e, BridgeEvent::IncomingDirectMessage(_)))
            .count();
        assert_eq!(
            delivered, 2,
            "\"ok\" twice is two messages, not a duplicate"
        );
        assert_eq!(store.messages_with(&peer).unwrap().len(), 2);
    }

    // ── Panic wipe ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn panic_wipe_destroys_local_state_and_relocks() {
        let dir = TempDir::new().unwrap();
        let mut rt = offline_runtime(&dir).await;
        let (_hex, peer) = stranger();
        rt.send_dm(&peer, "something private").await.unwrap();
        rt.add_journal_entry("a hard week", Some("low")).unwrap();
        assert_eq!(rt.outbox_pending(), 1);

        rt.panic_wipe().await.unwrap();

        assert!(!rt.is_vault_unlocked(), "the wipe re-locks the runtime");
        assert_eq!(rt.outbox_pending(), 0, "queued mail is destroyed too");

        // Reopening the store with the same passphrase finds nothing.
        let store = comrade_storage::EncryptedStore::open(dir.path(), "pin").unwrap();
        assert!(
            store.load_identity().unwrap().is_none(),
            "identity survived"
        );
        assert!(
            store.journal_entries().unwrap().is_empty(),
            "journal survived"
        );
        assert!(
            store.messages_with(&peer).unwrap().is_empty(),
            "DMs survived"
        );
        assert!(
            store
                .tree_names()
                .unwrap()
                .iter()
                .all(|tree| store.keys(tree).map(|k| k.is_empty()).unwrap_or(false)),
            "some tree kept its rows"
        );
    }

    #[tokio::test]
    async fn panic_wipe_needs_an_unlocked_vault() {
        let mut rt = ComradeRuntime::with_relays(vec![]);
        assert!(matches!(rt.panic_wipe().await, Err(UiError::VaultLocked)));
    }

    #[tokio::test]
    async fn a_wiped_store_can_be_onboarded_again() {
        let dir = TempDir::new().unwrap();
        let mut rt = offline_runtime(&dir).await;
        let first = rt.current_identity().unwrap().npub;
        rt.panic_wipe().await.unwrap();

        let second = rt.unlock_vault(dir.path(), "pin").await.unwrap().npub;
        assert_ne!(
            first, second,
            "a wipe must produce a new identity, not resurrect the old one"
        );
    }

    // ── Anonymous personas ──────────────────────────────────────────────────

    #[tokio::test]
    async fn scoped_personas_are_stable_per_scope_and_never_the_identity() {
        let dir = TempDir::new().unwrap();
        let rt = offline_runtime(&dir).await;
        let store = rt.ui.store_arc().unwrap();
        let identity = rt.current_identity().unwrap().npub;

        let seed = load_or_create_device_seed(&store).unwrap();
        let again = load_or_create_device_seed(&store).unwrap();
        let persona = anon::derive_scoped(&seed, anon::SCOPE_CHITTHI, "night-thoughts").unwrap();
        let persona_again =
            anon::derive_scoped(&again, anon::SCOPE_CHITTHI, "night-thoughts").unwrap();
        let other = anon::derive_scoped(&seed, anon::SCOPE_CHITTHI, "other-room").unwrap();

        assert_eq!(
            persona.public_key(),
            persona_again.public_key(),
            "the seed must be persisted, not regenerated per call"
        );
        assert_ne!(persona.public_key(), other.public_key());
        assert_ne!(
            persona.public_key().to_bech32().unwrap(),
            identity,
            "a persona must never be the identity key"
        );
    }

    #[tokio::test]
    async fn an_empty_anonymous_chitthi_is_refused_before_any_key_work() {
        let dir = TempDir::new().unwrap();
        let rt = offline_runtime(&dir).await;
        assert!(matches!(
            rt.broadcast_anonymous_chitthi("   ", None).await,
            Err(UiError::Engine(_))
        ));
    }

    // ── In-chat commands, tasks and offers ───────────────────────────────────

    #[tokio::test]
    async fn parsing_and_the_catalogue_need_no_vault() {
        // A composer calls these on every keystroke, including before unlock.
        let rt = ComradeRuntime::new();
        assert!(matches!(
            rt.parse_chat_command("/task ship it"),
            ChatCommand::Task { .. }
        ));
        assert!(matches!(
            rt.parse_chat_command("20/80 split"),
            ChatCommand::Plain
        ));
        assert!(!rt.chat_command_catalog().is_empty());
        assert_eq!(rt.chat_mentions("hi @ana").len(), 1);
    }

    /// An empty query must not reach a socket, and must not be an error either —
    /// a composer calls this as somebody clears the field.
    #[tokio::test]
    async fn an_empty_catalogue_query_is_no_results_rather_than_a_lookup() {
        assert_eq!(catalogue_lookup("   ", None).await.unwrap(), Vec::new());
    }

    /// A streaming search with no server saved is a setup step, not a failure
    /// — and never "no results", which would read as the library being empty.
    /// Under a lean build the build fact outranks the setup one, mirroring the
    /// catalogue test below.
    #[tokio::test]
    async fn a_streaming_search_without_a_config_asks_for_setup() {
        let got = subsonic_search(None, "kun faya kun".into()).await;
        #[cfg(feature = "catalogue-http")]
        assert_eq!(got, StreamSearchOutcome::NotConfigured);
        #[cfg(not(feature = "catalogue-http"))]
        assert_eq!(got, StreamSearchOutcome::BuildCannotSearch);
    }

    // ── The player's own library ─────────────────────────────────────────────

    fn a_track(key: &str) -> PlayerTrackDto {
        PlayerTrackDto {
            key: key.into(),
            title: format!("track {key}"),
            artist: "A. R. Rahman".into(),
            album: Some("Rockstar".into()),
            duration_ms: 327_000,
            url: None,
            kind: PlayerTrackKind::Local,
        }
    }

    #[tokio::test]
    async fn a_favourite_toggles_and_answers_what_it_now_is() {
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();

        let track = a_track("local:41");
        assert!(!rt.favourite_is(track.key.clone()).unwrap());
        assert!(
            rt.favourite_toggle(track.clone()).unwrap(),
            "first toggle adds"
        );
        assert!(rt.favourite_is(track.key.clone()).unwrap());
        assert_eq!(rt.favourites_list().unwrap(), vec![track.clone()]);
        assert!(
            !rt.favourite_toggle(track.clone()).unwrap(),
            "second removes"
        );
        assert!(rt.favourites_list().unwrap().is_empty());
    }

    #[tokio::test]
    async fn history_is_one_entry_per_track_newest_first_and_capped() {
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();

        rt.history_record(a_track("a"), 1_000).unwrap();
        rt.history_record(a_track("b"), 2_000).unwrap();
        // Same track again: updated in place, not duplicated.
        rt.history_record(a_track("a"), 3_000).unwrap();
        let got = rt.history_list().unwrap();
        assert_eq!(
            got.iter().map(|e| e.track.key.as_str()).collect::<Vec<_>>(),
            vec!["a", "b"],
            "most recent first, one row per track"
        );

        for i in 0..(HISTORY_MAX_ENTRIES + 10) {
            rt.history_record(a_track(&format!("n{i}")), 10_000 + i as u64)
                .unwrap();
        }
        let got = rt.history_list().unwrap();
        assert_eq!(got.len(), HISTORY_MAX_ENTRIES);
        assert!(
            !got.iter().any(|e| e.track.key == "a"),
            "the oldest entries fall off — that is what recent means"
        );
    }

    #[tokio::test]
    async fn history_clear_forgets_everything() {
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();
        rt.history_record(a_track("a"), 1).unwrap();
        rt.history_clear().unwrap();
        assert!(rt.history_list().unwrap().is_empty());
    }

    #[tokio::test]
    async fn playlists_round_trip_in_order_and_delete_leaves_the_music() {
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();

        assert!(
            rt.playlist_create("   ".into(), 0).is_err(),
            "an unnamed playlist is refused rather than invented"
        );
        let id = rt.playlist_create("Road trip".into(), 5).unwrap();
        rt.playlist_add_track(id.clone(), a_track("a")).unwrap();
        // A duplicate is allowed: a mixtape may say the same song twice.
        rt.playlist_add_track(id.clone(), a_track("a")).unwrap();
        rt.playlist_add_track(id.clone(), a_track("b")).unwrap();

        let lists = rt.playlists_list().unwrap();
        assert_eq!(lists.len(), 1);
        assert_eq!(lists[0].name, "Road trip");
        assert_eq!(
            lists[0]
                .tracks
                .iter()
                .map(|t| t.key.as_str())
                .collect::<Vec<_>>(),
            vec!["a", "a", "b"],
            "playlist order is insertion order"
        );

        // Removing by key takes out every copy — the key is the identity, so
        // "one of the two" was never a request this API could understand.
        rt.playlist_remove_track(id.clone(), "a".into()).unwrap();
        assert_eq!(
            rt.playlists_list().unwrap()[0]
                .tracks
                .iter()
                .map(|t| t.key.as_str())
                .collect::<Vec<_>>(),
            vec!["b"],
        );

        rt.playlist_delete(id).unwrap();
        assert!(rt.playlists_list().unwrap().is_empty());
    }

    #[test]
    fn reorder_tracks_moves_one_row_and_clamps_the_rest() {
        let ks = |ts: &[PlayerTrackDto]| ts.iter().map(|t| t.key.clone()).collect::<Vec<_>>();
        let base = vec![a_track("a"), a_track("b"), a_track("c"), a_track("d")];

        // First to last: everything else shuffles up one, the moved row lands *at* `to`.
        assert_eq!(
            ks(&reorder_tracks(base.clone(), 0, 3)),
            ["b", "c", "d", "a"]
        );
        // Last to first.
        assert_eq!(
            ks(&reorder_tracks(base.clone(), 3, 0)),
            ["d", "a", "b", "c"]
        );
        // A middle nudge.
        assert_eq!(
            ks(&reorder_tracks(base.clone(), 1, 2)),
            ["a", "c", "b", "d"]
        );
        // Out-of-range is a drag to the nearest end, not a panic.
        assert_eq!(
            ks(&reorder_tracks(base.clone(), 0, 99)),
            ["b", "c", "d", "a"]
        );
        assert_eq!(
            ks(&reorder_tracks(base.clone(), 99, 0)),
            ["d", "a", "b", "c"]
        );
        // No-ops round-trip untouched.
        assert_eq!(
            ks(&reorder_tracks(base.clone(), 2, 2)),
            ["a", "b", "c", "d"]
        );
        assert!(reorder_tracks(Vec::new(), 0, 1).is_empty());
    }

    #[tokio::test]
    async fn playlist_reorder_and_rename_leave_created_at_and_the_shelf_alone() {
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();

        // Two lists, created oldest-first so a rename that moved the shelf would show.
        let older = rt.playlist_create("Morning".into(), 10).unwrap();
        let newer = rt.playlist_create("Evening".into(), 20).unwrap();
        for k in ["a", "b", "c"] {
            rt.playlist_add_track(older.clone(), a_track(k)).unwrap();
        }

        rt.playlist_reorder(older.clone(), 0, 2).unwrap();
        let keys = |rt: &ComradeRuntime, id: &str| {
            rt.playlists_list()
                .unwrap()
                .into_iter()
                .find(|l| l.id == id)
                .unwrap()
                .tracks
                .iter()
                .map(|t| t.key.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            keys(&rt, &older),
            ["b", "c", "a"],
            "the dragged row lands at `to`"
        );

        // Out-of-range index clamps rather than erroring.
        rt.playlist_reorder(older.clone(), 99, 0).unwrap();
        assert_eq!(keys(&rt, &older), ["a", "b", "c"]);

        // Rename keeps the tracks, and keeps the shelf in creation order.
        assert!(
            rt.playlist_rename(older.clone(), "  ".into()).is_err(),
            "a blank name is refused here as it is at creation"
        );
        rt.playlist_rename(older.clone(), "Sunrise".into()).unwrap();
        let shelf = rt.playlists_list().unwrap();
        assert_eq!(
            shelf.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(),
            ["Sunrise", "Evening"],
            "rename edits in place — created_at_ms is untouched, so the order holds"
        );
        assert_eq!(
            keys(&rt, &older),
            ["a", "b", "c"],
            "rename does not touch the tracks"
        );
        let _ = newer;
    }

    #[tokio::test]
    async fn the_queue_survives_a_save_load_cycle_and_clamps_nothing_here() {
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();

        assert!(rt.queue_load().unwrap().is_none());
        let queue = SavedQueueDto {
            tracks: vec![a_track("a"), a_track("b")],
            index: 1,
            position_ms: 42_000,
            saved_at_ms: 7,
        };
        rt.queue_save(queue.clone()).unwrap();
        assert_eq!(rt.queue_load().unwrap(), Some(queue));
        // Saving again overwrites rather than archives.
        rt.queue_save(SavedQueueDto {
            tracks: vec![],
            index: 0,
            position_ms: 0,
            saved_at_ms: 9,
        })
        .unwrap();
        assert_eq!(rt.queue_load().unwrap().map(|q| q.saved_at_ms), Some(9));
        rt.queue_clear().unwrap();
        assert!(rt.queue_load().unwrap().is_none());
    }

    /// The whole library answers `VaultLocked` while locked — these are diary
    /// rows, and an unlocked vault is the app's own bar for reading diaries.
    #[tokio::test]
    async fn a_locked_vault_hides_the_player_library_behind_vault_locked() {
        let rt = ComradeRuntime::new();
        assert!(matches!(rt.favourites_list(), Err(UiError::VaultLocked)));
        assert!(matches!(rt.history_list(), Err(UiError::VaultLocked)));
        assert!(matches!(rt.playlists_list(), Err(UiError::VaultLocked)));
        assert!(matches!(
            rt.playlist_reorder("pl0".into(), 0, 1),
            Err(UiError::VaultLocked)
        ));
        assert!(matches!(
            rt.playlist_rename("pl0".into(), "x".into()),
            Err(UiError::VaultLocked)
        ));
        assert!(matches!(rt.queue_load(), Err(UiError::VaultLocked)));
    }

    /// A cleared search field must not reach the server, in every build that
    /// could reach one at all.
    #[cfg(feature = "catalogue-http")]
    #[tokio::test]
    async fn an_empty_streaming_query_is_an_empty_answer() {
        let cfg = SubsonicConfig {
            server: "https://music.example.com".into(),
            username: "u".into(),
            password: "p".into(),
        };
        assert_eq!(
            subsonic_search(Some(cfg), "   ".into()).await,
            StreamSearchOutcome::Found {
                candidates: Vec::new()
            }
        );
    }

    /// The distinction [`UiError::CatalogueUnavailable`] exists for: this build
    /// has no `catalogue-http`, so it must say it cannot search rather than
    /// reporting that the recording does not exist.
    ///
    /// Inverted under the feature, because the assertion worth making then is
    /// that the call is *reachable* — and the answer beyond that depends on
    /// MusicBrainz, which a test must not.
    #[tokio::test]
    async fn a_build_without_the_feature_says_so_instead_of_answering_nothing_found() {
        let got = catalogue_lookup("kun faya kun", None).await;
        #[cfg(not(feature = "catalogue-http"))]
        assert!(
            matches!(got, Err(UiError::CatalogueUnavailable)),
            "a build that cannot search must not report an empty catalogue: {got:?}"
        );
        #[cfg(feature = "catalogue-http")]
        assert!(
            !matches!(got, Err(UiError::CatalogueUnavailable)),
            "the feature is on, so this must not claim the build lacks a catalogue"
        );
    }

    /// The tier ladder, through the runtime wrapper rather than through
    /// `choose_audio_plan` directly — the wrapper's own job is mapping
    /// [`LibraryCandidateDto`] onto the pairs core wants, and swapping those two
    /// fields is the mistake the named record exists to prevent.
    #[test]
    fn the_audio_plan_prefers_this_device_then_the_peer_then_an_open_archive() {
        use comrade_core::catalogue::{CatalogueMatch, OpenLicence};

        let want = Recording {
            isrc: Some("GBAYE0601498".into()),
            title: "Kun Faya Kun".into(),
            artist: "A. R. Rahman".into(),
            album: None,
        };
        let mine = vec![LibraryCandidateDto {
            recording: want.clone(),
            duration_ms: 470_000,
        }];

        // Rung 1: it is already here, so nothing else is consulted.
        assert!(matches!(
            audio_plan(want.clone(), 470_000, mine.clone(), true, Vec::new()),
            AudioPlan::Library { .. }
        ));

        // Rung 2: not here, but the other side says they have it.
        assert_eq!(
            audio_plan(want.clone(), 470_000, Vec::new(), true, Vec::new()),
            AudioPlan::Peer
        );

        // Rung 3: nobody has it, and the archive's licence permits a copy.
        let open = CatalogueMatch {
            source: "Test".into(),
            recording: want.clone(),
            duration_ms: Some(470_000),
            audio_url: Some("https://archive.example/kfk.flac".into()),
            licence: OpenLicence::CreativeCommons,
        };
        assert_eq!(
            audio_plan(want.clone(), 470_000, Vec::new(), false, vec![open]),
            AudioPlan::OpenLicence {
                url: "https://archive.example/kfk.flac".into()
            }
        );

        // Rung 4, and the one that matters: an archive that serves audio has not
        // thereby licensed it. Same URL, undeclared licence, and the plan must
        // fall through to the embed rather than fetch it.
        let unlicensed = CatalogueMatch {
            source: "Test".into(),
            recording: want.clone(),
            duration_ms: Some(470_000),
            audio_url: Some("https://archive.example/kfk.flac".into()),
            licence: OpenLicence::Unknown,
        };
        assert_eq!(
            audio_plan(want, 470_000, Vec::new(), false, vec![unlicensed]),
            AudioPlan::EmbedOnly,
            "an unknown licence must not be fetched — the gate is the licence, not the URL"
        );
    }

    #[tokio::test]
    async fn a_handle_resolves_to_the_contact_the_user_named() {
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();
        let (_hex, ana) = stranger();
        rt.add_contact(&ana, "ana").unwrap();

        let found = rt.resolve_mentions("/task ship it @ana").unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].npub.as_deref(), Some(ana.as_str()));
        assert!(found[0].candidates.is_empty());
    }

    #[tokio::test]
    async fn an_unknown_handle_resolves_to_nobody_rather_than_a_guess() {
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();
        let (_hex, ana) = stranger();
        rt.add_contact(&ana, "ana").unwrap();

        let found = rt.resolve_mentions("@bina").unwrap();
        assert_eq!(found.len(), 1);
        assert!(found[0].npub.is_none());
        assert!(found[0].candidates.is_empty());
    }

    #[tokio::test]
    async fn two_contacts_answering_to_one_handle_come_back_as_a_question() {
        // Two people can publish the same name. Picking one is how a private
        // message reaches the wrong person, so the ambiguity is returned.
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();
        let (_h1, one) = stranger();
        let (_h2, two) = stranger();
        rt.add_contact(&one, "ana").unwrap();
        rt.add_contact(&two, "ana").unwrap();

        let found = rt.resolve_mentions("@ana").unwrap();
        assert!(found[0].npub.is_none(), "must not pick one of two");
        assert_eq!(found[0].candidates.len(), 2);
    }

    #[tokio::test]
    async fn a_note_to_self_never_needs_a_relay() {
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();

        let task = rt.assign_task(None, "water the plants").await.unwrap();
        assert_eq!(task.text, "water the plants");
        assert_eq!(task.state, TaskState::Open);
        assert!(task.assignee.is_none());
        assert!(task.assigned_by_me);
        assert!(task.mine_to_do, "the one person holding it may tick it off");

        assert_eq!(rt.tasks().unwrap().len(), 1);
        let done = rt.set_task_state(&task.id, TaskState::Done).await.unwrap();
        assert_eq!(done.state, TaskState::Done);
        assert_eq!(rt.tasks().unwrap()[0].state, TaskState::Done);
    }

    #[tokio::test]
    async fn an_empty_task_is_refused_rather_than_stored() {
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();
        assert!(matches!(
            rt.assign_task(None, "   ").await,
            Err(UiError::Engine(_))
        ));
        assert!(rt.tasks().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_finished_task_cannot_be_reopened_through_the_runtime() {
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();
        let task = rt.assign_task(None, "ship it").await.unwrap();
        rt.set_task_state(&task.id, TaskState::Done).await.unwrap();
        assert!(matches!(
            rt.set_task_state(&task.id, TaskState::Open).await,
            Err(UiError::Engine(_))
        ));
        assert!(matches!(
            rt.set_task_state("no-such-task", TaskState::Done).await,
            Err(UiError::Engine(_))
        ));
    }

    #[tokio::test]
    async fn tasks_and_offers_need_an_unlocked_vault() {
        let rt = ComradeRuntime::new();
        assert!(matches!(rt.tasks(), Err(UiError::VaultLocked)));
        assert!(matches!(
            rt.assign_task(None, "x").await,
            Err(UiError::VaultLocked)
        ));
        assert!(matches!(
            rt.offer_action(AppAction::Breathe, vec!["npub1x".into()])
                .await,
            Err(UiError::VaultLocked)
        ));
        assert!(matches!(rt.tara_aside("hello"), Err(UiError::VaultLocked)));
        assert!(matches!(
            rt.tara_in_chat("npub1x", "hello").await,
            Err(UiError::VaultLocked)
        ));
    }

    // ── Tara in the room (`@tara …`) ──────────────────────────────────────────

    #[tokio::test]
    async fn a_shared_ask_puts_the_question_and_the_answer_in_the_thread() {
        let dir = TempDir::new().unwrap();
        let rt = offline_runtime(&dir).await;
        let (_hex, peer) = stranger();

        let out = rt
            .tara_in_chat(&peer, "what should i do about this deadline")
            .await
            .unwrap();
        assert!(!out.kept_private);
        assert!(!out.crisis);

        // Order matters: the question, then her answer. A thread that showed
        // the reply first would read as though she spoke unprompted.
        let thread = rt.messages_with(&peer).unwrap();
        assert_eq!(thread.len(), 2);
        assert_eq!(thread[0].content, "what should i do about this deadline");
        assert_eq!(thread[0].author, MessageAuthor::Human);
        // Her words, with the wire marker already off them: a frontend renders
        // `content` as-is and reads `author` to decide whose bubble it is.
        assert_eq!(thread[1].content, out.reply);
        assert_eq!(thread[1].author, MessageAuthor::Tara);
        assert!(thread.iter().all(|m| m.outgoing), "this device sent both");
        assert_eq!(out.asked.unwrap().content, thread[0].content);
        assert_eq!(out.answered.unwrap().content, thread[1].content);
    }

    #[tokio::test]
    async fn her_line_is_still_marked_as_hers_after_a_reload() {
        // The DTO strips the marker; the store must not, or the author would be
        // whatever the last in-memory copy happened to say and a restart would
        // silently turn her answer into one of yours.
        let dir = TempDir::new().unwrap();
        let rt = offline_runtime(&dir).await;
        let (_hex, peer) = stranger();
        let out = rt
            .tara_in_chat(&peer, "what should i say to them")
            .await
            .unwrap();

        let stored = rt
            .ui
            .store_ref()
            .unwrap()
            .messages_with(&to_npub(&peer))
            .unwrap();
        assert_eq!(
            comrade_core::tara::tara_chat_answer(&stored[1].content),
            Some(out.reply.as_str()),
            "the wire form has to survive on disk"
        );
        // And reading it back through the DTO gives the same split as the send.
        let thread = rt.messages_with(&peer).unwrap();
        assert_eq!(thread[1].author, MessageAuthor::Tara);
        assert_eq!(thread[1].content, out.reply);
    }

    #[tokio::test]
    async fn an_ordinary_message_is_never_attributed_to_her() {
        let dir = TempDir::new().unwrap();
        let rt = offline_runtime(&dir).await;
        let (_hex, peer) = stranger();
        // Close enough to trip a sloppy match, and it must not: the marker is a
        // prefix, not a substring, and "Tara" as a topic is an ordinary word.
        let sent = rt
            .send_dm(&peer, "Tara said something like that too")
            .await
            .unwrap();
        assert_eq!(sent.author, MessageAuthor::Human);
        assert_eq!(sent.content, "Tara said something like that too");
    }

    #[tokio::test]
    async fn an_offline_shared_ask_does_not_tag_an_event_no_relay_has_seen() {
        // Both messages queue with a local outbox id, and the answer must not
        // claim to reply to one — an `e` tag naming a local id points at nothing.
        // (Online, the tag is what keeps her answer under the question when both
        // land in the same second; `created_at` alone cannot.)
        let dir = TempDir::new().unwrap();
        let rt = offline_runtime(&dir).await;
        let (_hex, peer) = stranger();

        let out = rt.tara_in_chat(&peer, "what now").await.unwrap();
        let answered = out.answered.unwrap();
        assert!(comrade_core::dak::outbox::is_local_message_id(
            &out.asked.unwrap().id
        ));
        assert_eq!(answered.reply_to, None);
    }

    #[tokio::test]
    async fn a_shared_ask_does_not_touch_the_private_thread() {
        // The private session is journal-adjacent. A question asked in front of
        // somebody is not a turn in it, in either direction.
        let dir = TempDir::new().unwrap();
        let rt = offline_runtime(&dir).await;
        let (_hex, peer) = stranger();

        rt.tara_in_chat(&peer, "help us pick a film").await.unwrap();
        assert!(
            rt.tara_thread().unwrap().is_empty(),
            "the shared ask leaked into the private thread"
        );
    }

    #[tokio::test]
    async fn distress_in_a_shared_ask_is_answered_but_never_sent() {
        // The safety property of the whole feature. Someone who types `@tara`
        // instead of `/tara` while in a bad place must not have their crisis
        // hand-off delivered into somebody else's chat.
        let dir = TempDir::new().unwrap();
        let rt = offline_runtime(&dir).await;
        let (_hex, peer) = stranger();

        let out = rt.tara_in_chat(&peer, "i want to die").await.unwrap();
        assert!(out.kept_private);
        assert!(out.crisis);
        assert!(out.asked.is_none() && out.answered.is_none());
        assert!(!out.reply.is_empty(), "she still answers, only privately");

        assert!(
            rt.messages_with(&peer).unwrap().is_empty(),
            "nothing at all may reach the thread"
        );
        assert_eq!(rt.outbox_pending(), 0, "nor the outbox — it is not a retry");
    }

    #[tokio::test]
    async fn naming_a_third_party_still_reframes_when_the_room_can_read_it() {
        // The gate that already guards the private aside has to hold here too,
        // and it matters more: a characterisation of @ana would now be sent to
        // the person you are talking to.
        let dir = TempDir::new().unwrap();
        let rt = offline_runtime(&dir).await;
        let (_hex, peer) = stranger();

        let out = rt
            .tara_in_chat(&peer, "what does @ana think of herself")
            .await
            .unwrap();
        assert!(
            out.reply.contains("what's coming up for you about @ana"),
            "no reframe: {}",
            out.reply
        );
        let thread = rt.messages_with(&peer).unwrap();
        assert_eq!(thread[1].content, out.reply);
        assert_eq!(thread[1].author, MessageAuthor::Tara);
    }

    #[tokio::test]
    async fn a_bare_shared_address_asks_nothing_and_sends_nothing() {
        let dir = TempDir::new().unwrap();
        let rt = offline_runtime(&dir).await;
        let (_hex, peer) = stranger();

        assert!(matches!(
            rt.tara_in_chat(&peer, "   ").await,
            Err(UiError::Engine(_))
        ));
        assert!(rt.messages_with(&peer).unwrap().is_empty());
    }

    #[tokio::test]
    async fn an_offer_to_someone_who_is_not_a_comrade_is_not_sent() {
        // Marking someone a comrade is the existing "may reach me" grant; an
        // offer is a notification, so it lives inside that grant.
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();
        let (_hex, ana) = stranger();
        rt.add_contact(&ana, "ana").unwrap();

        // …and it must be reported as "not a comrade", not as a cooldown: the UI
        // has to be able to suggest the actual fix.
        let outcome = rt
            .offer_action(AppAction::Breathe, vec![ana.clone()])
            .await
            .unwrap();
        assert!(outcome.sent.is_empty(), "a contact is not yet a comrade");
        assert_eq!(outcome.not_comrades, vec![ana.clone()]);
        assert!(outcome.on_cooldown.is_empty());

        let none = rt.offer_action(AppAction::Breathe, vec![]).await.unwrap();
        assert!(none.sent.is_empty() && none.not_comrades.is_empty());
    }

    #[tokio::test]
    async fn a_second_offer_inside_the_cooldown_tells_nobody_twice() {
        // The cooldown is a floor on notifications, shared with the nudge —
        // being able to send this repeatedly would make it a way to needle
        // somebody. What is pinned is that the *second* call reaches nobody and
        // says why.
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();
        let (_hex, ana) = stranger();
        rt.add_contact(&ana, "ana").unwrap();
        rt.set_comrade(&ana, true).unwrap();

        // The first call claims the cooldown for ana.
        let _ = rt.offer_action(AppAction::Breathe, vec![ana.clone()]).await;
        let second = rt
            .offer_action(AppAction::Breathe, vec![ana.clone()])
            .await
            .unwrap();
        assert!(
            second.sent.is_empty(),
            "the cooldown must swallow the second"
        );
        assert_eq!(
            second.on_cooldown,
            vec![ana.clone()],
            "and it must say the cooldown is why, not that ana is a stranger"
        );
        assert!(second.not_comrades.is_empty());
    }

    #[tokio::test]
    async fn an_aside_stays_on_this_device_and_reaches_the_tara_thread() {
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();

        let reply = rt.tara_aside("i keep putting this off").unwrap();
        assert!(reply.from_tara);
        // Same thread as the Tara tab — one companion, one history.
        let thread = rt.tara_thread().unwrap();
        assert_eq!(thread.len(), 2);
        assert_eq!(thread[0].text, "i keep putting this off");
        // And nothing was queued for anybody.
        assert_eq!(rt.outbox_pending(), 0);
    }

    #[tokio::test]
    async fn an_aside_about_a_named_person_is_turned_around() {
        // The request's own example, end to end through the runtime.
        let dir = TempDir::new().unwrap();
        let mut rt = ComradeRuntime::new();
        rt.unlock_vault(dir.path(), "pin").await.unwrap();

        let reply = rt
            .tara_aside("what does she @xyz thinking of herself")
            .unwrap();
        assert!(reply.text.contains("what's coming up for you about @xyz"));
        assert!(!reply.crisis);
    }

    #[tokio::test]
    async fn a_play_query_resolves_links_and_words_without_touching_a_network() {
        let rt = ComradeRuntime::new();

        // A YouTube link is the one thing we can drive ourselves.
        let yt = rt.play_query("https://youtu.be/dQw4w9WgXcQ", None);
        assert_eq!(yt.plan, PlayPlan::OpenNow);
        assert!(matches!(yt.content, Some(TogetherContent::Youtube { .. })));
        assert_eq!(yt.service, Some(MusicService::Youtube));

        // Spotify serves DRM audio no third party may decode, so the honest
        // answer is "here is what to open".
        let sp = rt.play_query(
            "https://open.spotify.com/track/1234567890abcdefghijkl",
            None,
        );
        assert_eq!(sp.plan, PlayPlan::NameOnly);
        assert!(sp.content.is_none());
        assert_eq!(sp.service, Some(MusicService::Spotify));

        // Free text names a recording to look for locally.
        let words = rt.play_query("Kun Faya Kun", Some(MusicService::Spotify));
        assert_eq!(words.plan, PlayPlan::FindLocally);
        assert_eq!(words.recording.unwrap().title, "Kun Faya Kun");

        assert_eq!(rt.play_query("  ", None).plan, PlayPlan::Empty);
    }

    #[tokio::test]
    async fn a_links_own_service_outranks_the_alias_that_was_typed() {
        // `/youtube <spotify url>` is still a Spotify URL.
        let rt = ComradeRuntime::new();
        let t = rt.play_query(
            "https://open.spotify.com/track/1234567890abcdefghijkl",
            Some(MusicService::Youtube),
        );
        assert_eq!(t.service, Some(MusicService::Spotify));
    }

    fn no_accounts() -> comrade_core::together::ServiceAccess {
        comrade_core::together::ServiceAccess::none()
    }

    fn spotify_link() -> comrade_core::together::MusicLink {
        comrade_core::together::MusicLink::Spotify {
            track_id: "6habFhsOp2NvshLv26DqMb".into(),
        }
    }

    #[test]
    fn a_local_copy_is_what_turns_a_query_into_a_session() {
        // The only branch the library answer decides.
        assert_eq!(
            play_route(PlayPlan::FindLocally, true, None, no_accounts()),
            PlayRoute::StartTogether
        );
        // Asking beats guessing: below the confidence bar we do not open a file
        // on somebody's behalf.
        assert_eq!(
            play_route(PlayPlan::FindLocally, false, None, no_accounts()),
            PlayRoute::AskForFile
        );
    }

    #[test]
    fn a_local_file_does_not_make_a_service_link_a_local_session() {
        // The plan describes what the *query* named. Someone with a similarly
        // titled mp3 on their phone has not been handed the Spotify track, and a
        // `/play <spotify url>` that quietly started a session on a different
        // file would put the two of them on different audio while the UI claimed
        // otherwise.
        for found in [true, false] {
            assert_eq!(
                play_route(
                    PlayPlan::NameOnly,
                    found,
                    Some(spotify_link()),
                    no_accounts()
                ),
                PlayRoute::OpenElsewhere,
                "found={found}",
            );
            assert_eq!(
                play_route(PlayPlan::OpenNow, found, None, no_accounts()),
                PlayRoute::PlayEmbed,
                "found={found}",
            );
            assert_eq!(
                play_route(PlayPlan::Empty, found, None, no_accounts()),
                PlayRoute::Nothing,
                "found={found}"
            );
        }
    }

    /// The Jam model, at the routing layer: the same link is a session on a
    /// device with the subscription behind it and a signpost on one without.
    #[test]
    fn the_same_link_routes_differently_on_two_devices() {
        let signed_in = comrade_core::together::ServiceAccess {
            spotify: true,
            apple_music: false,
        };
        assert_eq!(
            play_route(PlayPlan::NameOnly, false, Some(spotify_link()), signed_in),
            PlayRoute::PlayOnService,
        );
        assert_eq!(
            play_route(
                PlayPlan::NameOnly,
                false,
                Some(spotify_link()),
                no_accounts()
            ),
            PlayRoute::OpenElsewhere,
        );
    }

    /// Apple Music is signed in and still does not open a session, because
    /// `StartOnly` cannot be held — a ladder running against a player with no
    /// seek emits verdicts nothing applies.
    #[test]
    fn a_playhead_that_cannot_be_placed_does_not_open_a_session() {
        let apple = comrade_core::together::MusicLink::AppleMusic {
            storefront: "in".into(),
            track_id: "1440931493".into(),
        };
        let signed_in = comrade_core::together::ServiceAccess {
            spotify: false,
            apple_music: true,
        };
        assert_eq!(
            play_route(PlayPlan::NameOnly, false, Some(apple), signed_in),
            PlayRoute::OpenElsewhere,
        );
    }

    /// A `NameOnly` plan whose link went missing must not fall through to a
    /// player. The two travel together everywhere in the real call path; this
    /// pins what happens if a caller ever separates them.
    #[test]
    fn a_service_plan_with_no_link_is_a_signpost_not_a_session() {
        for access in [
            no_accounts(),
            comrade_core::together::ServiceAccess {
                spotify: true,
                apple_music: true,
            },
        ] {
            assert_eq!(
                play_route(PlayPlan::NameOnly, false, None, access),
                PlayRoute::OpenElsewhere,
            );
        }
    }

    #[test]
    fn every_plan_routes_somewhere_and_only_one_route_starts_a_local_session() {
        // A plan falling through to a route that opens a player would open one
        // on a query nobody resolved.
        let every_access = [
            no_accounts(),
            comrade_core::together::ServiceAccess {
                spotify: true,
                apple_music: false,
            },
            comrade_core::together::ServiceAccess {
                spotify: false,
                apple_music: true,
            },
            comrade_core::together::ServiceAccess {
                spotify: true,
                apple_music: true,
            },
        ];
        for plan in [
            PlayPlan::OpenNow,
            PlayPlan::FindLocally,
            PlayPlan::NameOnly,
            PlayPlan::Empty,
        ] {
            for found in [true, false] {
                for access in every_access {
                    let route = play_route(plan, found, Some(spotify_link()), access);
                    if route == PlayRoute::StartTogether {
                        assert_eq!(plan, PlayPlan::FindLocally, "only a library hit starts one");
                        assert!(found, "and only when a copy was actually found");
                    }
                    if route == PlayRoute::PlayOnService {
                        assert!(access.spotify, "a service route needs the account");
                    }
                }
            }
        }
    }

    // ── Threads and topics (see `comrade_core::topic`) ───────────────────────

    /// A conversation of `root` plus two replies into it, and one unrelated
    /// message — the smallest history in which "thread" means anything.
    async fn threaded_chat(rt: &ComradeRuntime, peer: &str) -> (String, String) {
        let root = rt
            .send_dm(peer, "the deposit still hasn't come back")
            .await
            .unwrap();
        let reply = rt
            .send_dm_reply(peer, "i'll chase them monday", Some(&root.id))
            .await
            .unwrap();
        // A reply to the *reply*, so the walk up the chain has something to do.
        rt.send_dm_reply(peer, "thanks", Some(&reply.id))
            .await
            .unwrap();
        rt.send_dm(peer, "unrelated: dinner?").await.unwrap();
        (root.id, reply.id)
    }

    #[tokio::test]
    async fn a_thread_is_the_root_and_everything_that_replied_into_it() {
        let dir = TempDir::new().unwrap();
        let rt = offline_runtime(&dir).await;
        let (_hex, peer) = stranger();
        let (root, reply) = threaded_chat(&rt, &peer).await;

        let thread = rt.thread(&peer, &root).unwrap();
        assert_eq!(thread.messages.len(), 3, "root plus both replies");
        assert_eq!(thread.messages[0].id, root, "oldest first");
        assert!(thread.media.is_empty());

        // Flat, Slack-style: a reply to a reply is in the same thread, and
        // opening from *any* member reaches the same sheet.
        assert_eq!(rt.thread(&peer, &reply).unwrap().root_id, root);
        assert_eq!(rt.thread_root(&peer, &reply).unwrap(), root);
    }

    #[tokio::test]
    async fn a_message_nobody_replied_to_is_a_thread_of_one() {
        let dir = TempDir::new().unwrap();
        let rt = offline_runtime(&dir).await;
        let (_hex, peer) = stranger();
        let (root, _) = threaded_chat(&rt, &peer).await;

        let threads = rt.threads(&peer, None).unwrap();
        assert_eq!(
            threads.len(),
            2,
            "the deposit thread and the dinner message"
        );
        let deposit = threads.iter().find(|t| t.root_id == root).unwrap();
        assert_eq!(deposit.reply_count, 2);
        assert_eq!(deposit.preview, "the deposit still hasn't come back");
        assert!(!deposit.root_missing);
        assert!(!deposit.root_is_media);
        let dinner = threads.iter().find(|t| t.root_id != root).unwrap();
        assert_eq!(
            dinner.reply_count, 0,
            "nobody replied, so it has no replies"
        );
    }

    #[tokio::test]
    async fn a_thread_is_filed_from_any_message_in_it_and_reaches_the_root() {
        // The reason `assign_thread` takes a message and not a root: filing
        // from a reply must land on the thread, not create a second one.
        let dir = TempDir::new().unwrap();
        let rt = offline_runtime(&dir).await;
        let (_hex, peer) = stranger();
        let (root, reply) = threaded_chat(&rt, &peer).await;

        let filed = rt
            .assign_thread(&peer, &reply, Some("#Flat Deposit".into()))
            .await
            .unwrap();
        assert_eq!(filed.root_id, root);
        assert_eq!(filed.topic_slug.as_deref(), Some("flat-deposit"));

        // And the topic exists, once, spelled the way it was typed.
        let topics = rt.topics(&peer).unwrap();
        assert_eq!(topics.len(), 1);
        assert_eq!(topics[0].slug, "flat-deposit");
        assert_eq!(topics[0].name, "Flat Deposit");
        assert!(topics[0].mine);
        assert_eq!(topics[0].thread_count, 1);
        assert_eq!(topics[0].message_count, 3);

        // Filtering by the topic finds it; the unrelated message stays out.
        let in_topic = rt.threads(&peer, Some("flat-deposit".into())).unwrap();
        assert_eq!(in_topic.len(), 1);
        assert_eq!(in_topic[0].root_id, root);
    }

    #[tokio::test]
    async fn filing_the_same_word_twice_is_one_topic() {
        // Both people typing `/assign #deposit` before either envelope lands is
        // the case the slug-as-id keying exists for.
        let dir = TempDir::new().unwrap();
        let rt = offline_runtime(&dir).await;
        let (_hex, peer) = stranger();
        let (root, _) = threaded_chat(&rt, &peer).await;

        rt.create_topic(&peer, "Deposit").await.unwrap();
        rt.assign_thread(&peer, &root, Some("deposit".into()))
            .await
            .unwrap();
        let topics = rt.topics(&peer).unwrap();
        assert_eq!(topics.len(), 1);
        assert_eq!(topics[0].name, "Deposit", "the first spelling is kept");
    }

    #[tokio::test]
    async fn a_thread_can_be_moved_and_taken_out_again() {
        let dir = TempDir::new().unwrap();
        let rt = offline_runtime(&dir).await;
        let (_hex, peer) = stranger();
        let (root, _) = threaded_chat(&rt, &peer).await;

        rt.assign_thread(&peer, &root, Some("deposit".into()))
            .await
            .unwrap();
        rt.assign_thread(&peer, &root, Some("repairs".into()))
            .await
            .unwrap();
        assert_eq!(
            rt.threads(&peer, Some("deposit".into())).unwrap().len(),
            0,
            "a thread is in one topic at a time"
        );
        assert_eq!(rt.threads(&peer, Some("repairs".into())).unwrap().len(), 1);

        let unfiled = rt.assign_thread(&peer, &root, None).await.unwrap();
        assert_eq!(unfiled.topic_slug, None);
        // Both topics survive the unfiling — nothing here deletes one.
        assert_eq!(rt.topics(&peer).unwrap().len(), 2);
    }

    #[tokio::test]
    async fn a_topic_name_that_cannot_be_a_key_is_refused_rather_than_mangled() {
        let dir = TempDir::new().unwrap();
        let rt = offline_runtime(&dir).await;
        let (_hex, peer) = stranger();
        let (root, _) = threaded_chat(&rt, &peer).await;

        // AUDIT TOPIC-1: a Devanagari topic name is a thing users will type, and it
        // must fail loudly rather than become `#--`.
        let err = rt
            .assign_thread(&peer, &root, Some("जमा".into()))
            .await
            .unwrap_err();
        assert!(matches!(err, UiError::Engine(_)));
        assert!(rt.topics(&peer).unwrap().is_empty());
    }

    #[tokio::test]
    async fn archiving_a_topic_keeps_its_threads_readable() {
        let dir = TempDir::new().unwrap();
        let rt = offline_runtime(&dir).await;
        let (_hex, peer) = stranger();
        let (root, _) = threaded_chat(&rt, &peer).await;
        rt.assign_thread(&peer, &root, Some("deposit".into()))
            .await
            .unwrap();

        let closed = rt.set_topic_closed(&peer, "deposit", true).await.unwrap();
        assert!(closed.closed);
        assert_eq!(
            closed.thread_count, 1,
            "the archive still holds its threads"
        );
        assert_eq!(
            rt.threads(&peer, Some("deposit".into())).unwrap().len(),
            1,
            "closing hides it from the picker, not from the reader"
        );
        assert!(
            !rt.set_topic_closed(&peer, "deposit", false)
                .await
                .unwrap()
                .closed
        );
    }

    #[tokio::test]
    async fn a_thread_reply_lands_in_the_thread_however_it_was_addressed() {
        // The sheet's composer replies to the *root*, not to whatever was last
        // read — that flatness is what makes a thread a thread rather than a
        // chain of quotes.
        let dir = TempDir::new().unwrap();
        let rt = offline_runtime(&dir).await;
        let (_hex, peer) = stranger();
        let (root, reply) = threaded_chat(&rt, &peer).await;

        let sent = rt
            .send_thread_reply(&peer, &reply, "any news?")
            .await
            .unwrap();
        assert_eq!(sent.reply_to.as_deref(), Some(root.as_str()));
        assert_eq!(rt.thread(&peer, &root).unwrap().messages.len(), 4);
    }

    #[tokio::test]
    async fn a_filing_for_a_thread_we_do_not_hold_is_not_an_error_but_says_so() {
        // A peer can file a thread whose root is outside our window; the row
        // has to survive until the backfill catches up.
        let dir = TempDir::new().unwrap();
        let rt = offline_runtime(&dir).await;
        let (_hex, peer) = stranger();
        threaded_chat(&rt, &peer).await;
        let handles = rt.handles();
        let store = handles.store.as_ref().unwrap();

        assert!(apply_topic_signal(
            Some(store),
            &peer,
            10,
            &TopicSignal::Create {
                slug: "deposit".into(),
                name: "deposit".into(),
            },
        ));
        assert!(apply_topic_signal(
            Some(store),
            &peer,
            11,
            &TopicSignal::Assign {
                root_id: "not-here-yet".into(),
                slug: Some("deposit".into()),
            },
        ));
        // The topic exists and counts nothing, because the thread is not here.
        let topics = rt.topics(&peer).unwrap();
        assert_eq!(topics[0].thread_count, 0);
        assert_eq!(topics[0].message_count, 0);
        assert!(!topics[0].mine, "the peer named it");
        // And the filing survived for when it arrives.
        assert_eq!(
            store.thread_topic("not-here-yet").unwrap().as_deref(),
            Some("deposit")
        );
    }

    #[tokio::test]
    async fn an_incoming_topic_signal_that_changes_nothing_raises_no_event() {
        // The two-day gift-wrap re-scan on every launch makes a replay the
        // normal case; redrawing on one is what `IncomingReaction` already
        // refuses to do.
        let dir = TempDir::new().unwrap();
        let rt = offline_runtime(&dir).await;
        let (_hex, peer) = stranger();
        let handles = rt.handles();
        let store = handles.store.as_ref().unwrap();
        let create = TopicSignal::Create {
            slug: "deposit".into(),
            name: "Deposit".into(),
        };

        assert!(apply_topic_signal(Some(store), &peer, 10, &create));
        assert!(
            !apply_topic_signal(Some(store), &peer, 10, &create),
            "a redelivered creation is not news"
        );
        let filing = TopicSignal::Assign {
            root_id: "r".into(),
            slug: Some("deposit".into()),
        };
        assert!(apply_topic_signal(Some(store), &peer, 20, &filing));
        assert!(!apply_topic_signal(Some(store), &peer, 15, &filing));
    }

    #[tokio::test]
    async fn a_topic_whose_slug_does_not_match_its_name_is_refused() {
        // The forgery shape this catches: an envelope naming `#deposit` but
        // carrying a slug nothing on this device could ever derive, which would
        // file threads under a key no user could type back.
        let dir = TempDir::new().unwrap();
        let rt = offline_runtime(&dir).await;
        let (_hex, peer) = stranger();
        let handles = rt.handles();
        let store = handles.store.as_ref().unwrap();

        assert!(!apply_topic_signal(
            Some(store),
            &peer,
            10,
            &TopicSignal::Create {
                slug: "something-else".into(),
                name: "Deposit".into(),
            },
        ));
        assert!(rt.topics(&peer).unwrap().is_empty());
    }

    #[tokio::test]
    async fn topics_do_not_leak_between_conversations() {
        let dir = TempDir::new().unwrap();
        let rt = offline_runtime(&dir).await;
        let (_hex_a, peer_a) = stranger();
        let (_hex_b, peer_b) = stranger();
        let (root_a, _) = threaded_chat(&rt, &peer_a).await;
        threaded_chat(&rt, &peer_b).await;

        rt.assign_thread(&peer_a, &root_a, Some("deposit".into()))
            .await
            .unwrap();
        assert_eq!(rt.topics(&peer_a).unwrap().len(), 1);
        assert!(
            rt.topics(&peer_b).unwrap().is_empty(),
            "`#deposit` with two people is two subjects"
        );
        assert_eq!(
            rt.threads(&peer_b, Some("deposit".into())).unwrap().len(),
            0
        );
    }
    // ── Travel guide (see `comrade_core::travel` and `docs/TRAVEL.md`) ───────

    /// Colaba, Mumbai.
    const TRAVEL_ORIGIN: (f64, f64) = (18.9220, 72.8347);

    fn travel_place(
        name: &str,
        kind: travel::PlaceKind,
        rating: Option<f64>,
        votes: Option<u32>,
    ) -> Place {
        Place {
            id: format!("gm:{name}"),
            name: name.to_string(),
            kind,
            lat: TRAVEL_ORIGIN.0,
            lon: TRAVEL_ORIGIN.1,
            rating,
            review_count: votes,
            address: None,
            cuisine: None,
            note: None,
            source: if rating.is_some() {
                travel::PlaceSource::GoogleMaps
            } else {
                travel::PlaceSource::OpenStreetMap
            },
            open_url: None,
        }
    }

    fn cached_guide(cell: &str, at: u64, places: Vec<Place>) -> TravelGuide {
        travel::build_guide(TRAVEL_ORIGIN, cell, places, Vec::new(), None, at)
    }

    #[tokio::test]
    async fn a_cached_guide_is_served_without_touching_the_network() {
        // The default test build has no `travel-http`, so *any* fetch is an
        // error — which makes this a proof that the cache short-circuits it.
        let cache = TravelCache::default();
        let cell = TravelQuery::around(
            TRAVEL_ORIGIN.0,
            TRAVEL_ORIGIN.1,
            2_000,
            travel::Section::Eat,
        )
        .cell;
        cache.put(cached_guide(
            &cell,
            now_secs(),
            vec![travel_place(
                "Britannia",
                travel::PlaceKind::Restaurant,
                Some(4.6),
                Some(9_000),
            )],
        ));

        let guide = travel_guide(&cache, None, TRAVEL_ORIGIN.0, TRAVEL_ORIGIN.1, 2_000, false)
            .await
            .expect("a fresh cache entry answers on its own");
        assert!(guide.from_cache);
        assert!(!guide.stale);
        assert_eq!(guide.eat.len(), 1);
        assert!(guide.eat[0].legendary);
        assert_eq!(guide.ratings_from.as_deref(), Some("google_maps"));
    }

    /// Both tests below need the fetch to *fail*, which is only deterministic
    /// in a build with no socket. Under `travel-http` they would make a real
    /// request to Overpass and Wikipedia — which is flaky, slow, and rude to
    /// two free public services that owe this project nothing. The behaviour
    /// they pin is the no-network half, so that is where they run.
    #[cfg(not(feature = "travel-http"))]
    #[tokio::test]
    async fn refresh_goes_past_a_fresh_cache_entry() {
        let cache = TravelCache::default();
        let cell = TravelQuery::around(
            TRAVEL_ORIGIN.0,
            TRAVEL_ORIGIN.1,
            2_000,
            travel::Section::Eat,
        )
        .cell;
        cache.put(cached_guide(&cell, now_secs(), Vec::new()));

        // Cache is fresh, so only `refresh` can reach the (absent) network —
        // and the stale-fallback path then returns the same entry with a notice
        // rather than an empty screen.
        let guide = travel_guide(&cache, None, TRAVEL_ORIGIN.0, TRAVEL_ORIGIN.1, 2_000, true)
            .await
            .expect("the stale fallback keeps the screen populated");
        assert!(
            guide.stale,
            "a fallback answer must not claim to be current"
        );
        assert!(
            guide.notice.is_some(),
            "and it has to say why it is not current"
        );
    }

    #[cfg(not(feature = "travel-http"))]
    #[tokio::test]
    async fn nothing_cached_and_nothing_fetchable_is_an_error_not_an_empty_guide() {
        // "This build cannot look places up" must never render as "there is
        // nothing near you" — the whole reason `TravelUnavailable` exists.
        let err = travel_guide(
            &TravelCache::default(),
            None,
            TRAVEL_ORIGIN.0,
            TRAVEL_ORIGIN.1,
            2_000,
            false,
        )
        .await
        .expect_err("no cache and no socket is a failure");
        assert!(matches!(err, UiError::TravelUnavailable), "got {err:?}");
    }

    #[tokio::test]
    async fn locking_the_vault_forgets_where_the_user_has_been() {
        let dir = TempDir::new().unwrap();
        let mut rt = offline_runtime(&dir).await;
        let cache = rt.travel_cache();
        let cell = TravelQuery::around(
            TRAVEL_ORIGIN.0,
            TRAVEL_ORIGIN.1,
            2_000,
            travel::Section::Eat,
        )
        .cell;
        cache.put(cached_guide(&cell, now_secs(), Vec::new()));
        assert!(cache.get(&cell, now_secs(), true).is_some());

        rt.lock_vault().await;
        assert!(
            cache.get(&cell, now_secs(), true).is_none(),
            "the cells someone has stood in are not something a locked app keeps"
        );
    }

    #[tokio::test]
    async fn a_blank_ratings_key_clears_rather_than_storing_an_unusable_one() {
        let dir = TempDir::new().unwrap();
        let rt = offline_runtime(&dir).await;
        assert!(!rt.travel_ratings_configured());

        rt.set_travel_api_key("  AIzaSy-example  ").unwrap();
        assert!(rt.travel_ratings_configured());
        assert_eq!(
            rt.travel_api_key().as_deref(),
            Some("AIzaSy-example"),
            "stored trimmed, so a pasted newline is not sent as part of the key"
        );

        rt.set_travel_api_key("   ").unwrap();
        assert!(
            !rt.travel_ratings_configured(),
            "a blank key clears the setting instead of failing every lookup with a 403"
        );
    }

    #[tokio::test]
    async fn changing_the_ratings_key_drops_guides_fetched_under_the_old_one() {
        let dir = TempDir::new().unwrap();
        let rt = offline_runtime(&dir).await;
        let cache = rt.travel_cache();
        let cell = TravelQuery::around(
            TRAVEL_ORIGIN.0,
            TRAVEL_ORIGIN.1,
            2_000,
            travel::Section::Eat,
        )
        .cell;
        cache.put(cached_guide(&cell, now_secs(), Vec::new()));

        rt.set_travel_api_key("AIzaSy-example").unwrap();
        assert!(
            cache.get(&cell, now_secs(), true).is_none(),
            "otherwise adding a key looks like it did nothing until the TTL expires"
        );
    }

    #[test]
    fn a_place_dto_measures_distance_from_the_real_fix_not_the_blurred_one() {
        // The privacy blur must not leak into what the screen says: a place
        // 200 m away has to read as 200 m, not as the cell-centre offset.
        let mut place = travel_place("Bademiya", travel::PlaceKind::StreetFood, None, None);
        place.lat = TRAVEL_ORIGIN.0 + 0.0018; // ~200 m north
        let dto = TravelPlaceDto::from_place(&place, TRAVEL_ORIGIN);
        assert!(
            (150..=250).contains(&dto.distance_m),
            "expected ~200 m, got {}",
            dto.distance_m
        );
        assert_eq!(dto.section, "eat", "a stall is somewhere you eat");
        assert_eq!(dto.kind, "street_food");
        assert!(!dto.legendary, "an unrated stall carries no badge");
    }
}
