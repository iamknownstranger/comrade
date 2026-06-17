/*!
 * comrade_core — Protocol Handling, Crypto Primitives, and Engine Drivers
 *
 * Public API surface:
 *   crypto  — keypair generation, Bech32, DH shared secret, HKDF key derivation
 *   sabha   — NIP-10 thread-tree parser + public relay engine
 *   vault   — NIP-04 E2E DM engine + UPI /pay regex processor
 *   saathi  — Off-grid libp2p mesh (mDNS + Gossipsub)
 *   sakha   — Yrs CRDT shared ledger + DH-encrypted Nostr sync
 */

pub mod crypto;
pub mod error;
pub mod saathi;
pub mod sabha;
pub mod sakha;
pub mod vault;

pub use error::CoreError;
