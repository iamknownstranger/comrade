/*!
 * Milestone 2 — Cryptographic Profiles & Key Management
 *
 * Handles:
 *  • Local keypair generation (never leaves device)
 *  • Bech32 serialisation / deserialisation (nsec / npub)
 *  • secp256k1 Diffie-Hellman shared-secret derivation for the Sakha/Sakhi realm
 *  • HKDF-SHA256 key stretching for AES-GCM symmetric keys
 */

use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Key, Nonce,
};
use hkdf::Hkdf;
use nostr_sdk::{Keys, PublicKey, SecretKey, ToBech32};
use rand::RngCore;
use sha2::{Digest, Sha256};
use tracing::instrument;

use crate::error::CryptoError;

/// Length of the AES-256-GCM nonce prepended to sealed buffers.
const AEAD_NONCE_LEN: usize = 12;

// ── Key profile ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct KeyProfile {
    pub keys: Keys,
    pub npub: String,
    pub nsec: String,
}

impl KeyProfile {
    /// Generate a brand-new keypair, entirely on-device.
    #[instrument(name = "keygen")]
    pub fn generate() -> Result<Self, CryptoError> {
        let keys = Keys::generate();
        Self::from_keys(keys)
    }

    /// Restore a profile from an existing `nostr_sdk::Keys`.
    pub fn from_keys(keys: Keys) -> Result<Self, CryptoError> {
        let npub = keys
            .public_key()
            .to_bech32()
            .map_err(|e| CryptoError::Bech32(e.to_string()))?;
        let nsec = keys
            .secret_key()
            .to_bech32()
            .map_err(|e| CryptoError::Bech32(e.to_string()))?;

        Ok(Self { keys, npub, nsec })
    }

    /// Load a profile from a raw nsec Bech32 string.
    pub fn from_nsec(nsec: &str) -> Result<Self, CryptoError> {
        let secret_key = SecretKey::parse(nsec).map_err(|e| CryptoError::Bech32(e.to_string()))?;
        let keys = Keys::new(secret_key);
        Self::from_keys(keys)
    }

    pub fn public_key(&self) -> PublicKey {
        self.keys.public_key()
    }
}

// ── Diffie-Hellman shared-secret derivation ──────────────────────────────────

/// Compute an ECDH shared secret between our secret key and a partner's XOnly
/// public key (Nostr format, 32 bytes).
///
/// The XOnly key is lifted to a compressed 33-byte SEC1 representation by
/// prepending the even-parity prefix byte `0x02`, then secp256k1 ECDH is
/// performed. The raw output is then passed through SHA-256 to produce a
/// uniform 32-byte value safe to use as an AES-256 key seed.
pub fn compute_dh_shared_secret(
    our_secret: &SecretKey,
    their_xonly_pubkey: &PublicKey,
) -> Result<[u8; 32], CryptoError> {
    // XOnly public key (32 bytes) → compressed SEC1 (33 bytes, even parity)
    let xonly_bytes = their_xonly_pubkey.to_bytes();
    let mut compressed = [0u8; 33];
    compressed[0] = 0x02;
    compressed[1..].copy_from_slice(&xonly_bytes);

    let raw_pk = secp256k1::PublicKey::from_slice(&compressed)
        .map_err(|e| CryptoError::Secp256k1(e.to_string()))?;

    let raw_sk = secp256k1::SecretKey::from_slice(&our_secret.to_secret_bytes())
        .map_err(|e| CryptoError::Secp256k1(e.to_string()))?;

    // shared_secret_point returns [x (32 bytes) | y (32 bytes)] of scalar * point.
    // Hashing only the x-coordinate makes the result parity-independent:
    // both P and -P have the same x, so both sides get the same secret even
    // when the X-only public key is lifted with the wrong parity.
    let shared_point = secp256k1::ecdh::shared_secret_point(&raw_pk, &raw_sk);

    let mut hasher = Sha256::new();
    hasher.update(&shared_point[..32]);
    let result: [u8; 32] = hasher.finalize().into();
    Ok(result)
}

/// Derive a media-encryption key for a peer in one step: ECDH shared secret →
/// HKDF-labelled AES-256 key.
///
/// Because ECDH is symmetric, the recipient derives the *same* key from their
/// own secret key and the sender's public key — so the key never has to travel
/// over the wire. The public NIP-94 event can therefore omit the key entirely;
/// the encrypted blob is unreadable without one side's private key.
pub fn derive_media_key(
    our_secret: &SecretKey,
    their_pubkey: &PublicKey,
    label: &str,
) -> Result<[u8; 32], CryptoError> {
    let shared = compute_dh_shared_secret(our_secret, their_pubkey)?;
    Ok(derive_symmetric_key(&shared, label))
}

/// Derive a labelled AES-256-GCM key from a shared secret using HKDF-SHA256.
///
/// The `label` parameter acts as the HKDF `info` field so that different
/// applications of the same shared secret produce cryptographically independent keys.
/// 32-byte output is always within HKDF-SHA256's maximum (8160 bytes), so expand
/// cannot fail.
pub fn derive_symmetric_key(shared_secret: &[u8; 32], label: &str) -> [u8; 32] {
    let hk = Hkdf::<Sha256>::new(None, shared_secret);
    let mut output = [0u8; 32];
    hk.expand(label.as_bytes(), &mut output)
        .expect("32-byte output is always within HKDF-SHA256 limit");
    output
}

// ── AEAD helpers (shared across engines) ──────────────────────────────────────

/// Seal `plaintext` with AES-256-GCM. Output is `[nonce (12) | ciphertext+tag]`.
pub fn aes256gcm_seal(key: &[u8; 32], plaintext: &[u8]) -> Result<Vec<u8>, CryptoError> {
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let mut nonce_bytes = [0u8; AEAD_NONCE_LEN];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    let mut ciphertext = cipher
        .encrypt(nonce, plaintext)
        .map_err(|e| CryptoError::Aead(e.to_string()))?;

    let mut out = nonce_bytes.to_vec();
    out.append(&mut ciphertext);
    Ok(out)
}

/// Open an AES-256-GCM `[nonce (12) | ciphertext+tag]` buffer.
pub fn aes256gcm_open(key: &[u8; 32], sealed: &[u8]) -> Result<Vec<u8>, CryptoError> {
    if sealed.len() <= AEAD_NONCE_LEN {
        return Err(CryptoError::Aead("sealed buffer too short".into()));
    }
    let (nonce_bytes, ciphertext) = sealed.split_at(AEAD_NONCE_LEN);
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let nonce = Nonce::from_slice(nonce_bytes);
    cipher
        .decrypt(nonce, ciphertext)
        .map_err(|e| CryptoError::Aead(e.to_string()))
}

/// Lowercase hex SHA-256 digest of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut s = String::with_capacity(64);
    for b in digest {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_keypair_produces_valid_bech32() {
        let profile = KeyProfile::generate().expect("keygen");
        assert!(profile.npub.starts_with("npub1"), "npub prefix");
        assert!(profile.nsec.starts_with("nsec1"), "nsec prefix");
    }

    #[test]
    fn roundtrip_from_nsec() {
        let original = KeyProfile::generate().expect("keygen");
        let restored = KeyProfile::from_nsec(&original.nsec).expect("restore");
        assert_eq!(original.npub, restored.npub);
    }

    #[test]
    fn dh_is_symmetric() {
        let alice = KeyProfile::generate().expect("alice keygen");
        let bob = KeyProfile::generate().expect("bob keygen");

        let alice_shared =
            compute_dh_shared_secret(alice.keys.secret_key(), &bob.public_key()).expect("alice DH");

        let bob_shared =
            compute_dh_shared_secret(bob.keys.secret_key(), &alice.public_key()).expect("bob DH");

        assert_eq!(
            alice_shared, bob_shared,
            "DH must produce the same shared secret on both sides"
        );
    }

    #[test]
    fn derive_key_different_labels_differ() {
        let secret = [0xABu8; 32];
        let k1 = derive_symmetric_key(&secret, "sakha-ledger");
        let k2 = derive_symmetric_key(&secret, "sakha-audit");
        assert_ne!(k1, k2);
    }

    #[test]
    fn media_key_is_symmetric_between_peers() {
        // The recipient must derive the same media key from their own secret and
        // the sender's pubkey — the property that lets us omit the key from the
        // public event entirely.
        let alice = KeyProfile::generate().unwrap();
        let bob = KeyProfile::generate().unwrap();
        let from_alice = derive_media_key(
            alice.keys.secret_key(),
            &bob.public_key(),
            "comrade-media-v1",
        )
        .unwrap();
        let from_bob = derive_media_key(
            bob.keys.secret_key(),
            &alice.public_key(),
            "comrade-media-v1",
        )
        .unwrap();
        assert_eq!(from_alice, from_bob);
    }
}
