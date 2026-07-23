/*!
 * comrade_core — Protocol Handling, Crypto Primitives, and Engine Drivers
 *
 * Public API surface:
 *   crypto  — keypair generation, Bech32, DH shared secret, HKDF key derivation
 *   sabha   — NIP-10 ChitthiThread parser + public relay engine (Chitthi Feed)
 *   vault   — NIP-04 E2E DM engine + UPI /pay regex processor
 *   dm      — DM control envelopes (profile-share on accept, read/delivered receipts)
 *   call    — voice/video call signaling (WebRTC over the Vault DM channel)
 *   saathi  — Off-grid libp2p mesh (mDNS + Gossipsub)
 *   sakha   — Yrs CRDT shared ledger + DH-encrypted Nostr sync
 *   tara    — reflective-companion engine (deterministic, on-device, not therapy)
 *   relay   — NIP-65 relay-list metadata + outbox-model routing
 *   media   — NIP-94/96 encrypted media staging + pluggable uploaders
 */

// Gives this crate its own `UniFfiTag`, so the plain-data types the engines
// expose (`UpiPaymentIntent`, `CallSignal`, …) can derive `uniffi::Record` /
// `uniffi::Enum` directly — no parallel FFI-only mirror type to keep in sync
// as the data model grows. `comrade_jni` is the only crate that turns this
// (plus `comrade_ui`'s own namespace) into an actual cdylib.
uniffi::setup_scaffolding!("comrade_core");

pub mod call;
pub mod crypto;
pub mod dm;
pub mod error;
pub mod media;
pub mod relay;
pub mod saathi;
pub mod sabha;
pub mod sakha;
pub mod tara;
pub mod vault;

pub use error::CoreError;
