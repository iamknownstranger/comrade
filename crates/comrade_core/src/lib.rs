/*!
 * comrade_core — Protocol Handling, Crypto Primitives, and Engine Drivers
 *
 * Public API surface:
 *   crypto  — keypair generation, Bech32, DH shared secret, HKDF key derivation
 *   sabha   — NIP-10 ChitthiThread parser + public relay engine (Chitthi Feed)
 *   vault   — NIP-04 E2E DM engine + UPI /pay regex processor
 *   dm      — DM control envelopes (profile-share on accept, read/delivered receipts)
 *   command — in-chat command grammar + @mentions (one parser for every frontend)
 *   catalogue — name a recording, then decide where its audio may come from
 *   karya   — tasks: name a piece of work, yours or a comrade's
 *   call    — voice/video call signaling (WebRTC over the Vault DM channel)
 *   presence— comrade presence beacons (online/offline, to chosen peers only)
 *   nudge   — "they nearly wrote to you": one signal for an abandoned draft
 *   note    — hand one journal entry to one person: the wire form of a shared note
 *   together— sync-play control channel (watch/listen together; no media moves)
 *   topic   — threads inside a conversation, and the topics they are filed under
 *   share   — chunked, resumable, playable-early P2P file transfer (no server)
 *   saathi  — Off-grid libp2p mesh (mDNS + Gossipsub)
 *   sakha   — Yrs CRDT shared ledger + DH-encrypted Nostr sync
 *   tara    — reflective-companion engine (deterministic, on-device, not therapy)
 *   attention — attention-restoration engine (usage mirror, focus sessions, long-read chunking)
 *   relay   — NIP-65 relay-list metadata + outbox-model routing
 *   media   — NIP-94/96 encrypted media staging + pluggable uploaders
 *   link    — sign a keyless client (a browser tab) in to a device holding the vault
 *   travel  — where the locals eat, and what this place is: legendary-restaurant
 *             ranking, attractions, and facts from one (deliberately blurred)
 *             coordinate
 *
 * Store-and-forward, privacy, and anonymity primitives adapted from
 * permissionlesstech/bitchat (see `docs/BITCHAT_ADOPTION.md` for the full
 * take/adapt/reject analysis):
 *   dak     — sender outbox + courier envelopes ("the recipient is not here")
 *   gcs     — Golomb-coded set filters for reconciling public history
 *   anon    — unlinkable scoped + ephemeral identities for anonymous posting
 *   geo     — geohash location channels and presence counting
 *   pad     — bucket length padding, so ciphertext size leaks less
 *   seen    — bounded dedup seen-set shared by every ingress path
 *   metrics — privacy-safe, device-local delivery counters
 */

// Gives this crate its own `UniFfiTag`, so the plain-data types the engines
// expose (`UpiPaymentIntent`, `CallSignal`, …) can derive `uniffi::Record` /
// `uniffi::Enum` directly — no parallel FFI-only mirror type to keep in sync
// as the data model grows. `comrade_jni` is the only crate that turns this
// (plus `comrade_ui`'s own namespace) into an actual cdylib.
uniffi::setup_scaffolding!("comrade_core");

pub mod anon;
pub mod attention;
pub mod avatar;
pub mod call;
pub mod catalogue;
pub mod command;
pub mod crypto;
pub mod dak;
pub mod dm;
/// Carrying out the catalogue's OpenLicence tier — the download half of §20.
pub mod download;
pub mod error;
pub mod gcs;
pub mod geo;
pub mod handoff;
pub mod karya;
pub mod link;
pub mod media;
pub mod metrics;
pub mod note;
pub mod nudge;
pub mod pad;
pub mod presence;
pub mod public_sources;
pub mod relay;
pub mod ride;
pub mod saathi;
pub mod sabha;
pub mod sakha;
pub mod seen;
pub mod share;
pub mod subsonic;
pub mod tara;
pub mod together;
pub mod topic;
pub mod travel;
pub mod vault;

pub use error::CoreError;
