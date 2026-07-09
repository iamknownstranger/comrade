/*!
 * comrade_core — Protocol Handling, Crypto Primitives, and Engine Drivers
 *
 * Public API surface:
 *   crypto  — keypair generation, Bech32, DH shared secret, HKDF key derivation
 *   companion — private on-device journaling / vent / reflect companion + safety
 *   sabha   — NIP-10 ChitthiThread parser + public relay engine (Chitthi Feed)
 *   vault   — NIP-04 E2E DM engine + UPI /pay regex processor
 *   saathi  — Off-grid libp2p mesh (mDNS + Gossipsub)
 *   sakha   — Yrs CRDT shared ledger + DH-encrypted Nostr sync
 *   relay   — NIP-65 relay-list metadata + outbox-model routing
 *   media   — NIP-94/96 encrypted media staging + pluggable uploaders
 */

pub mod companion;
pub mod crypto;
pub mod error;
pub mod media;
pub mod relay;
pub mod saathi;
pub mod sabha;
pub mod sakha;
pub mod vault;

pub use error::CoreError;
