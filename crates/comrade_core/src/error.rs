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

    #[error("store-and-forward: {0}")]
    Dak(#[from] DakError),

    #[error("padding: {0}")]
    Padding(#[from] PaddingError),

    #[error("media: {0}")]
    Media(#[from] MediaError),

    #[error("unfurl: {0}")]
    Unfurl(#[from] UnfurlError),

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

// ── unfurl: sender-built link previews ───────────────────────────────────────

#[derive(Debug, Error)]
pub enum UnfurlError {
    #[error("http error: {0}")]
    Http(String),

    /// The `unfurl-http` feature is off in this build — see `unfurl.rs`'s
    /// module doc for why the feature (not the parser) is what stays gated.
    #[error("fetching a link preview requires the `unfurl-http` cargo feature")]
    FeatureDisabled,

    #[error("preview body is {size} bytes, over the {max}-byte cap")]
    TooLarge { size: u64, max: u64 },
}

// ── dak: store-and-forward (outbox + couriers) ───────────────────────────────

#[derive(Debug, Error)]
pub enum DakError {
    #[error("payload is {size} bytes, over the {max}-byte courier cap")]
    Oversize { size: usize, max: usize },

    #[error("envelope has already expired")]
    Expired,

    #[error("envelope lifetime exceeds the {0}s policy limit")]
    LifetimeTooLong(u64),

    #[error("courier bag is full")]
    BagFull,

    #[error("this depositor's courier quota is used up")]
    DepositorQuotaFull,

    #[error("malformed envelope: {0}")]
    Malformed(String),

    #[error("sender authentication failed")]
    AuthFailed,

    #[error("crypto: {0}")]
    Crypto(#[from] CryptoError),

    #[error("padding: {0}")]
    Padding(#[from] PaddingError),
}

// ── link: browser sessions signed in by QR ──────────────────────────────────

/// Failures on the device-linking path ([`crate::link`]).
///
/// Deliberately fine-grained on the refusals, because every one of these is
/// shown to somebody standing there with a phone in their hand wondering why the
/// scan did nothing. "Malformed" is the only catch-all, and it carries the
/// reason.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum LinkError {
    #[error("this link code is from a newer version of Comrade (v{0})")]
    UnsupportedVersion(u8),

    #[error("this link code has expired — show a fresh one and scan again")]
    OfferExpired,

    #[error("malformed link code: {0}")]
    Malformed(String),

    #[error("relay {0} is not wss://; a linked browser needs an encrypted relay")]
    InsecureRelay(String),

    #[error("link code names {count} relays, over the limit of {max}")]
    TooManyRelays { count: usize, max: usize },

    #[error("a linked session must be granted at least one permission")]
    NoScopes,

    #[error("session lifetime of {requested}s is not between 1s and {max}s")]
    BadLifetime { requested: u64, max: u64 },

    #[error("already linked to {max} devices — revoke one first")]
    TooManySessions { max: usize },

    #[error("too many link attempts; {max} are allowed every {window_secs}s")]
    TooManyAttempts { max: usize, window_secs: u64 },

    #[error("this device is not linked to your vault")]
    UnknownSession,

    #[error("this linked session has expired — scan again to sign back in")]
    SessionExpired,

    #[error("a linked session is not permitted to call {method}")]
    NotPermitted { method: String },

    #[error("frame is {size} bytes, over the {max}-byte link cap")]
    FrameTooLarge { size: usize, max: usize },

    #[error("too many part-received frames; {max} are held at once")]
    TooManyPartialFrames { max: usize },
}

// ── pad: bucket length padding ───────────────────────────────────────────────

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PaddingError {
    #[error("padded buffer is shorter than the pad-length trailer")]
    TooShort,

    #[error("declared pad length {0} does not fit the buffer")]
    Invalid(usize),
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
