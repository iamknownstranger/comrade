use thiserror::Error;

// ── Top-level umbrella ───────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("crypto: {0}")]
    Crypto(#[from] CryptoError),

    #[error("sabha: {0}")]
    Sabha(#[from] SabhaError),

    #[error("vault: {0}")]
    Vault(#[from] VaultError),

    #[error("saathi: {0}")]
    Saathi(#[from] SaathiError),

    #[error("sakha: {0}")]
    Sakha(#[from] SakhaError),

    #[error("gossip: {0}")]
    Gossip(#[from] GossipError),

    #[error("media: {0}")]
    Media(#[from] MediaError),

    #[error("pukar: {0}")]
    Pukar(#[from] PukarError),

    #[error("serialization: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("nostr: {0}")]
    Nostr(String),

    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

// ── Milestone 2: Cryptographic profiles ─────────────────────────────────────

#[derive(Debug, Error)]
pub enum CryptoError {
    #[error("bech32 encoding failed: {0}")]
    Bech32(String),

    #[error("secp256k1 error: {0}")]
    Secp256k1(String),

    #[error("invalid key material")]
    InvalidKey,

    #[error("key derivation failed: {0}")]
    Derivation(String),

    #[error("AEAD error: {0}")]
    Aead(String),
}

// ── Milestone 3: Sabha feed ──────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum SabhaError {
    #[error("relay connection failed: {0}")]
    RelayError(String),

    #[error("subscription error: {0}")]
    SubscriptionError(String),

    #[error("event parse failed: {0}")]
    ParseError(String),
}

// ── Milestone 3: Vault DMs ───────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum VaultError {
    #[error("encryption failed: {0}")]
    EncryptionFailed(String),

    #[error("decryption failed: {0}")]
    DecryptionFailed(String),

    #[error("invalid recipient public key: {0}")]
    InvalidRecipient(String),

    #[error("UPI payment string parse error: {0}")]
    UpiParseFailed(String),
}

// ── Milestone 4: Saathi mesh ─────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum SaathiError {
    #[error("transport error: {0}")]
    TransportError(String),

    #[error("swarm initialisation failed: {0}")]
    SwarmInit(String),

    #[error("message broadcast failed: {0}")]
    BroadcastFailed(String),

    #[error("peer discovery error: {0}")]
    DiscoveryError(String),

    #[error("message cache overflow")]
    CacheOverflow,
}

// ── Track 4: NIP-65 relay gossip ─────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum GossipError {
    #[error("relay list parse failed: {0}")]
    ParseFailed(String),

    #[error("relay connection failed: {0}")]
    RelayError(String),

    #[error("subscription error: {0}")]
    SubscriptionError(String),

    #[error("event signing failed: {0}")]
    SigningFailed(String),
}

// ── Track 3: NIP-94/96 encrypted media ───────────────────────────────────────

#[derive(Debug, Error)]
pub enum MediaError {
    #[error("media encryption error: {0}")]
    Crypto(String),

    #[error("upload failed: {0}")]
    UploadFailed(String),

    #[error("file metadata parse failed: {0}")]
    ParseFailed(String),

    #[error("event signing failed: {0}")]
    SigningFailed(String),

    #[error("http error: {0}")]
    Http(String),
}

// ── Pukar: real-time call signaling ──────────────────────────────────────────

#[derive(Debug, Error)]
pub enum PukarError {
    #[error("a call is already in progress")]
    AlreadyInCall,

    #[error("no live call with id {0}")]
    NoSuchCall(String),

    #[error("invalid call state: {0}")]
    InvalidState(String),

    #[error("malformed signal payload: {0}")]
    Malformed(String),

    #[error("unsupported signal version: {0}")]
    UnsupportedVersion(u64),

    #[error("signaling transport error: {0}")]
    Signaling(String),
}

// ── Milestone 5: Sakha/Sakhi CRDT ledger ────────────────────────────────────

#[derive(Debug, Error)]
pub enum SakhaError {
    #[error("CRDT operation failed: {0}")]
    CrdtError(String),

    #[error("sync encoding failed: {0}")]
    SyncEncodeFailed(String),

    #[error("sync decoding failed: {0}")]
    SyncDecodeFailed(String),

    #[error("encryption error: {0}")]
    EncryptionError(String),

    #[error("no shared secret available — pairing handshake incomplete")]
    NoSharedSecret,

    #[error("nostr relay error: {0}")]
    RelayError(String),
}
