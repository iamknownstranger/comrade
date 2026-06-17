/*!
 * Track 3 — NIP-94/96 Encrypted Media Staging & Distributed Upload
 *
 * Nostr relays only store small JSON events, so media (photos, audio notes,
 * files) needs a separate pipeline. This module implements the client side:
 *
 *  1. **Stage**: AES-256-GCM-encrypt the file with a key derived from the
 *     recipient's DH shared secret (Couples) or a per-file random key shared
 *     over an E2E DM (Vault). The relay/host only ever sees opaque ciphertext.
 *  2. **Upload**: push the encrypted blob to decentralized storage through a
 *     pluggable [`MediaUploader`] (NIP-96 HTTP server / Blossom / mock).
 *  3. **Describe**: build a NIP-94 (kind-1063) file-metadata event referencing
 *     the returned URL plus content hashes, ready to paste into a note or DM.
 *
 * The decryption key is *never* placed in the public NIP-94 event — it is
 * returned separately as a [`MediaSecret`] for the caller to transmit over the
 * already-encrypted channel.
 *
 * The staging, metadata, and mock-upload paths are fully unit-tested. The real
 * HTTP NIP-96 uploader lives behind the `nip96-http` cargo feature.
 */

use std::collections::HashMap;
use std::sync::Arc;

use nostr_sdk::prelude::*;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tracing::info;

use crate::crypto::{aes256gcm_open, aes256gcm_seal, sha256_hex};
use crate::error::MediaError;

/// NIP-94 file metadata event kind.
pub const FILE_METADATA_KIND: u16 = 1063;

/// Symmetric algorithm label recorded in [`MediaSecret`].
pub const MEDIA_ALGORITHM: &str = "aes-256-gcm";

// ── Staged media & secret ───────────────────────────────────────────────────────

/// An encrypted blob ready to upload, plus the public hashes describing it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncryptedMedia {
    /// `[nonce | ciphertext+tag]` — exactly what gets uploaded.
    pub ciphertext: Vec<u8>,
    /// SHA-256 of `ciphertext` (NIP-94 `x` tag — hash of the served file).
    pub sha256_hex: String,
    /// Size of `ciphertext` in bytes.
    pub size: usize,
    pub mime_type: String,
}

/// The information the recipient needs to decrypt the blob. Transmit this over
/// an already-encrypted channel — never in the public NIP-94 event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaSecret {
    /// 32-byte AES-256 key, hex-encoded.
    pub key_hex: String,
    pub algorithm: String,
    /// SHA-256 of the original plaintext (NIP-94 `ox` tag).
    pub original_sha256_hex: String,
}

/// Encrypt `plaintext` for distribution. Returns the uploadable blob and the
/// out-of-band secret needed to decrypt it.
pub fn encrypt_media(
    plaintext: &[u8],
    mime_type: &str,
    key: &[u8; 32],
) -> Result<(EncryptedMedia, MediaSecret), MediaError> {
    let ciphertext =
        aes256gcm_seal(key, plaintext).map_err(|e| MediaError::Crypto(e.to_string()))?;
    let media = EncryptedMedia {
        sha256_hex: sha256_hex(&ciphertext),
        size: ciphertext.len(),
        mime_type: mime_type.to_string(),
        ciphertext,
    };
    let secret = MediaSecret {
        key_hex: hex::encode(key),
        algorithm: MEDIA_ALGORITHM.to_string(),
        original_sha256_hex: sha256_hex(plaintext),
    };
    Ok((media, secret))
}

/// Decrypt a downloaded blob using the out-of-band [`MediaSecret`], verifying
/// the recovered plaintext against the original hash.
pub fn decrypt_media(ciphertext: &[u8], secret: &MediaSecret) -> Result<Vec<u8>, MediaError> {
    let key_bytes = hex::decode(&secret.key_hex)
        .map_err(|e| MediaError::Crypto(format!("bad key hex: {e}")))?;
    let key: [u8; 32] = key_bytes
        .try_into()
        .map_err(|_| MediaError::Crypto("key must be 32 bytes".into()))?;

    let plaintext =
        aes256gcm_open(&key, ciphertext).map_err(|e| MediaError::Crypto(e.to_string()))?;

    if sha256_hex(&plaintext) != secret.original_sha256_hex {
        return Err(MediaError::Crypto("decrypted content hash mismatch".into()));
    }
    Ok(plaintext)
}

// ── NIP-94 file metadata ─────────────────────────────────────────────────────────

/// Parsed NIP-94 file metadata (kind 1063).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileMetadata {
    pub url: String,
    pub mime_type: String,
    /// SHA-256 of the served (encrypted) file — NIP-94 `x`.
    pub sha256_hex: String,
    /// SHA-256 of the original file — NIP-94 `ox`.
    pub original_sha256_hex: Option<String>,
    pub size: Option<usize>,
    /// Free-text caption (event content).
    pub caption: String,
}

/// Build a signed NIP-94 (kind-1063) file-metadata event.
pub fn build_file_metadata_event(keys: &Keys, meta: &FileMetadata) -> Result<Event, MediaError> {
    let mut tags: Vec<Tag> = Vec::new();
    let mut push = |parts: &[&str]| -> Result<(), MediaError> {
        let tag = Tag::parse(parts.iter().copied())
            .map_err(|e| MediaError::ParseFailed(e.to_string()))?;
        tags.push(tag);
        Ok(())
    };

    push(&["url", &meta.url])?;
    push(&["m", &meta.mime_type])?;
    push(&["x", &meta.sha256_hex])?;
    if let Some(ox) = &meta.original_sha256_hex {
        push(&["ox", ox])?;
    }
    if let Some(size) = meta.size {
        push(&["size", &size.to_string()])?;
    }

    EventBuilder::new(Kind::from(FILE_METADATA_KIND), meta.caption.clone())
        .tags(tags)
        .sign_with_keys(keys)
        .map_err(|e| MediaError::SigningFailed(e.to_string()))
}

/// Parse a NIP-94 event's tags into [`FileMetadata`].
pub fn parse_file_metadata(event: &Event) -> Result<FileMetadata, MediaError> {
    let val = serde_json::to_value(event)
        .map_err(|e| MediaError::ParseFailed(format!("serialise event: {e}")))?;
    let tags = val
        .get("tags")
        .and_then(|t| t.as_array())
        .ok_or_else(|| MediaError::ParseFailed("no tags array".into()))?;

    let mut map: HashMap<String, String> = HashMap::new();
    for tag in tags {
        let Some(arr) = tag.as_array() else { continue };
        let (Some(name), Some(value)) = (
            arr.first().and_then(|v| v.as_str()),
            arr.get(1).and_then(|v| v.as_str()),
        ) else {
            continue;
        };
        // First occurrence wins for each tag name.
        map.entry(name.to_string())
            .or_insert_with(|| value.to_string());
    }

    let url = map
        .get("url")
        .cloned()
        .ok_or_else(|| MediaError::ParseFailed("missing url tag".into()))?;
    let sha256_hex = map
        .get("x")
        .cloned()
        .ok_or_else(|| MediaError::ParseFailed("missing x (hash) tag".into()))?;

    Ok(FileMetadata {
        url,
        mime_type: map.get("m").cloned().unwrap_or_default(),
        sha256_hex,
        original_sha256_hex: map.get("ox").cloned(),
        size: map.get("size").and_then(|s| s.parse().ok()),
        caption: event.content.clone(),
    })
}

// ── Pluggable uploader ───────────────────────────────────────────────────────────

/// Result of a successful upload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadReceipt {
    pub url: String,
}

/// A backend that stores an opaque encrypted blob and returns a fetch URL.
///
/// Implementations: [`InMemoryUploader`] (testing/local), and `Nip96Uploader`
/// (behind the `nip96-http` feature) for real decentralized HTTP storage.
#[allow(async_fn_in_trait)] // internal trait; bounds are added at call sites
pub trait MediaUploader {
    async fn upload(&self, blob: &[u8], mime_type: &str) -> Result<UploadReceipt, MediaError>;
}

/// In-memory uploader for tests and offline use. Stores blobs keyed by their
/// SHA-256 and serves them back via [`InMemoryUploader::fetch`].
#[derive(Clone, Default)]
pub struct InMemoryUploader {
    store: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    base_url: String,
}

impl InMemoryUploader {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            store: Arc::new(Mutex::new(HashMap::new())),
            base_url: base_url.into(),
        }
    }

    /// Retrieve a previously uploaded blob by its URL.
    pub async fn fetch(&self, url: &str) -> Option<Vec<u8>> {
        let hash = url.rsplit('/').next().unwrap_or_default().to_string();
        self.store.lock().await.get(&hash).cloned()
    }
}

impl MediaUploader for InMemoryUploader {
    async fn upload(&self, blob: &[u8], _mime_type: &str) -> Result<UploadReceipt, MediaError> {
        let hash = sha256_hex(blob);
        self.store.lock().await.insert(hash.clone(), blob.to_vec());
        let base = self.base_url.trim_end_matches('/');
        Ok(UploadReceipt {
            url: format!("{base}/{hash}"),
        })
    }
}

// ── Media engine: stage → upload → describe ──────────────────────────────────────

/// Ties encryption, upload, and NIP-94 metadata into one call.
pub struct MediaEngine<U: MediaUploader> {
    uploader: U,
    keys: Keys,
}

impl<U: MediaUploader> MediaEngine<U> {
    pub fn new(uploader: U, keys: Keys) -> Self {
        Self { uploader, keys }
    }

    /// Encrypt `plaintext`, upload the ciphertext, and produce a signed NIP-94
    /// event plus the out-of-band [`MediaSecret`] for the recipient.
    pub async fn share_encrypted(
        &self,
        plaintext: &[u8],
        mime_type: &str,
        caption: &str,
        key: &[u8; 32],
    ) -> Result<(Event, MediaSecret), MediaError> {
        let (media, secret) = encrypt_media(plaintext, mime_type, key)?;
        let receipt = self.uploader.upload(&media.ciphertext, mime_type).await?;
        info!(url = %receipt.url, size = media.size, "media: encrypted blob uploaded");

        let meta = FileMetadata {
            url: receipt.url,
            mime_type: media.mime_type,
            sha256_hex: media.sha256_hex,
            original_sha256_hex: Some(secret.original_sha256_hex.clone()),
            size: Some(media.size),
            caption: caption.to_string(),
        };
        let event = build_file_metadata_event(&self.keys, &meta)?;
        Ok((event, secret))
    }
}

// ── Real NIP-96 HTTP uploader (feature-gated) ────────────────────────────────────

#[cfg(feature = "nip96-http")]
mod nip96 {
    use super::*;
    use base64::{engine::general_purpose::STANDARD as B64, Engine as _};

    /// NIP-98 HTTP Auth event kind.
    const HTTP_AUTH_KIND: u16 = 27235;

    /// Uploads encrypted blobs to a NIP-96 HTTP file-storage server.
    ///
    /// `api_url` is the server's upload endpoint (from its
    /// `/.well-known/nostr/nip96.json` `api_url` field). Each request is
    /// authenticated with a NIP-98 `Authorization: Nostr <base64-event>` header.
    pub struct Nip96Uploader {
        client: reqwest::Client,
        api_url: String,
        keys: Keys,
    }

    impl Nip96Uploader {
        pub fn new(api_url: impl Into<String>, keys: Keys) -> Self {
            Self {
                client: reqwest::Client::new(),
                api_url: api_url.into(),
                keys,
            }
        }

        /// Build the base64-encoded NIP-98 auth event for a POST to `url`.
        fn auth_header(&self, url: &str) -> Result<String, MediaError> {
            let tags = vec![
                Tag::parse(["u", url]).map_err(|e| MediaError::Http(e.to_string()))?,
                Tag::parse(["method", "POST"]).map_err(|e| MediaError::Http(e.to_string()))?,
            ];
            let event = EventBuilder::new(Kind::from(HTTP_AUTH_KIND), "")
                .tags(tags)
                .sign_with_keys(&self.keys)
                .map_err(|e| MediaError::SigningFailed(e.to_string()))?;
            let json =
                serde_json::to_string(&event).map_err(|e| MediaError::Http(e.to_string()))?;
            Ok(format!("Nostr {}", B64.encode(json)))
        }
    }

    impl MediaUploader for Nip96Uploader {
        async fn upload(&self, blob: &[u8], mime_type: &str) -> Result<UploadReceipt, MediaError> {
            let part = reqwest::multipart::Part::bytes(blob.to_vec())
                .file_name("comrade-media.bin")
                .mime_str(mime_type)
                .map_err(|e| MediaError::Http(e.to_string()))?;
            let form = reqwest::multipart::Form::new().part("file", part);

            let auth = self.auth_header(&self.api_url)?;
            let resp = self
                .client
                .post(&self.api_url)
                .header("Authorization", auth)
                .multipart(form)
                .send()
                .await
                .map_err(|e| MediaError::Http(e.to_string()))?;

            if !resp.status().is_success() {
                return Err(MediaError::UploadFailed(format!(
                    "status {}",
                    resp.status()
                )));
            }

            let body: serde_json::Value = resp
                .json()
                .await
                .map_err(|e| MediaError::Http(e.to_string()))?;

            // NIP-96 returns the download URL inside nip94_event's `url` tag.
            let url = body
                .get("nip94_event")
                .and_then(|e| e.get("tags"))
                .and_then(|t| t.as_array())
                .and_then(|tags| {
                    tags.iter().find_map(|tag| {
                        let arr = tag.as_array()?;
                        if arr.first()?.as_str()? == "url" {
                            arr.get(1)?.as_str().map(|s| s.to_string())
                        } else {
                            None
                        }
                    })
                })
                .ok_or_else(|| {
                    MediaError::UploadFailed("response missing nip94_event url tag".into())
                })?;

            Ok(UploadReceipt { url })
        }
    }
}

#[cfg(feature = "nip96-http")]
pub use nip96::Nip96Uploader;

// ── Blossom upload + fetch-and-decrypt (feature-gated) ───────────────────────────

/// Default Blossom server used by [`upload_encrypted_blob`]. Blossom servers are
/// content-addressed (blob URL = `<server>/<sha256>`) and accept raw `PUT`s.
pub const DEFAULT_BLOSSOM_SERVER: &str = "https://cdn.hackers.town";

#[cfg(feature = "media-http")]
mod http {
    use super::*;
    use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
    use std::time::{SystemTime, UNIX_EPOCH};

    /// Blossom authorization event kind (BUD-01).
    const BLOSSOM_AUTH_KIND: u16 = 24242;

    fn unix_now() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or_default()
    }

    /// Fetch the opaque blob at `url` and AES-256-GCM-decrypt it with `key`.
    ///
    /// The inverse of the stage→upload pipeline: the nonce travels prefixed to
    /// the ciphertext, so only the 32-byte `key` (re-derived from ECDH) is
    /// needed. Authenticated decryption rejects any tampered blob.
    pub async fn fetch_and_decrypt_media(url: &str, key: &[u8; 32]) -> Result<Vec<u8>, MediaError> {
        let resp = reqwest::get(url)
            .await
            .map_err(|e| MediaError::Http(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(MediaError::UploadFailed(format!(
                "fetch status {}",
                resp.status()
            )));
        }
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| MediaError::Http(e.to_string()))?;
        aes256gcm_open(key, &bytes).map_err(|e| MediaError::Crypto(e.to_string()))
    }

    /// Anonymous Blossom upload: `PUT <server>/upload` with the raw blob body.
    /// Returns the download URL. Servers that mandate auth will reject this;
    /// use [`BlossomUploader`] (signed) for those.
    pub async fn upload_encrypted_blob_to(
        server: &str,
        blob: Vec<u8>,
        mime_type: &str,
    ) -> Result<String, MediaError> {
        let endpoint = format!("{}/upload", server.trim_end_matches('/'));
        let resp = reqwest::Client::new()
            .put(&endpoint)
            .header(reqwest::header::CONTENT_TYPE, mime_type)
            .body(blob)
            .send()
            .await
            .map_err(|e| MediaError::Http(e.to_string()))?;
        parse_blob_url(resp).await
    }

    /// Upload an encrypted blob to [`DEFAULT_BLOSSOM_SERVER`] (anonymous).
    pub async fn upload_encrypted_blob(
        blob: Vec<u8>,
        mime_type: &str,
    ) -> Result<String, MediaError> {
        upload_encrypted_blob_to(DEFAULT_BLOSSOM_SERVER, blob, mime_type).await
    }

    /// Extract the download URL from a Blossom blob-descriptor JSON response.
    async fn parse_blob_url(resp: reqwest::Response) -> Result<String, MediaError> {
        if !resp.status().is_success() {
            return Err(MediaError::UploadFailed(format!(
                "upload status {}",
                resp.status()
            )));
        }
        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| MediaError::Http(e.to_string()))?;
        body.get("url")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| MediaError::UploadFailed("blob descriptor missing url".into()))
    }

    /// A Blossom uploader that signs each request with a BUD-01 `kind:24242`
    /// authorization event, so it works against servers that require auth.
    pub struct BlossomUploader {
        client: reqwest::Client,
        server: String,
        keys: Keys,
    }

    impl BlossomUploader {
        pub fn new(server: impl Into<String>, keys: Keys) -> Self {
            Self {
                client: reqwest::Client::new(),
                server: server.into(),
                keys,
            }
        }

        fn auth_header(&self, blob_sha256: &str) -> Result<String, MediaError> {
            let expiration = (unix_now() + 3600).to_string();
            let tags = vec![
                Tag::parse(["t", "upload"]).map_err(|e| MediaError::Http(e.to_string()))?,
                Tag::parse(["x", blob_sha256]).map_err(|e| MediaError::Http(e.to_string()))?,
                Tag::parse(["expiration", &expiration])
                    .map_err(|e| MediaError::Http(e.to_string()))?,
            ];
            let event = EventBuilder::new(Kind::from(BLOSSOM_AUTH_KIND), "Upload encrypted media")
                .tags(tags)
                .sign_with_keys(&self.keys)
                .map_err(|e| MediaError::SigningFailed(e.to_string()))?;
            let json =
                serde_json::to_string(&event).map_err(|e| MediaError::Http(e.to_string()))?;
            Ok(format!("Nostr {}", B64.encode(json)))
        }
    }

    impl MediaUploader for BlossomUploader {
        async fn upload(&self, blob: &[u8], mime_type: &str) -> Result<UploadReceipt, MediaError> {
            let sha = sha256_hex(blob);
            let endpoint = format!("{}/upload", self.server.trim_end_matches('/'));
            let resp = self
                .client
                .put(&endpoint)
                .header(reqwest::header::AUTHORIZATION, self.auth_header(&sha)?)
                .header(reqwest::header::CONTENT_TYPE, mime_type)
                .body(blob.to_vec())
                .send()
                .await
                .map_err(|e| MediaError::Http(e.to_string()))?;
            let url = parse_blob_url(resp).await?;
            Ok(UploadReceipt { url })
        }
    }
}

#[cfg(feature = "media-http")]
pub use http::{
    fetch_and_decrypt_media, upload_encrypted_blob, upload_encrypted_blob_to, BlossomUploader,
};

// Stubs so the rest of the workspace compiles (and degrades gracefully) when the
// `media-http` feature is off — the runtime calls these unconditionally.
#[cfg(not(feature = "media-http"))]
pub async fn fetch_and_decrypt_media(_url: &str, _key: &[u8; 32]) -> Result<Vec<u8>, MediaError> {
    Err(MediaError::Http(
        "media download requires the `media-http` cargo feature".into(),
    ))
}

#[cfg(not(feature = "media-http"))]
pub async fn upload_encrypted_blob(_blob: Vec<u8>, _mime_type: &str) -> Result<String, MediaError> {
    Err(MediaError::Http(
        "media upload requires the `media-http` cargo feature".into(),
    ))
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> [u8; 32] {
        [7u8; 32]
    }

    #[test]
    fn encrypt_decrypt_roundtrip() {
        let plaintext = b"fake JPEG bytes \x00\xFF\xD8\xFF";
        let (media, secret) = encrypt_media(plaintext, "image/jpeg", &key()).unwrap();
        assert_ne!(media.ciphertext, plaintext);
        assert_eq!(media.sha256_hex, sha256_hex(&media.ciphertext));
        let recovered = decrypt_media(&media.ciphertext, &secret).unwrap();
        assert_eq!(recovered, plaintext);
    }

    #[test]
    fn wrong_key_fails_to_decrypt() {
        let (media, mut secret) = encrypt_media(b"data", "image/png", &key()).unwrap();
        secret.key_hex = hex::encode([9u8; 32]);
        assert!(decrypt_media(&media.ciphertext, &secret).is_err());
    }

    #[test]
    fn tampered_hash_is_detected() {
        let (media, mut secret) = encrypt_media(b"data", "image/png", &key()).unwrap();
        secret.original_sha256_hex = sha256_hex(b"different");
        // Decryption succeeds but the integrity check against ox must fail.
        assert!(decrypt_media(&media.ciphertext, &secret).is_err());
    }

    #[test]
    fn x_and_ox_hashes_are_distinct_and_correct() {
        let plaintext = b"original content";
        let (media, secret) = encrypt_media(plaintext, "text/plain", &key()).unwrap();
        assert_eq!(secret.original_sha256_hex, sha256_hex(plaintext));
        assert_eq!(media.sha256_hex, sha256_hex(&media.ciphertext));
        assert_ne!(media.sha256_hex, secret.original_sha256_hex);
    }

    #[test]
    fn nip94_build_then_parse_roundtrip() {
        let keys = Keys::generate();
        let meta = FileMetadata {
            url: "https://host.example/abc".into(),
            mime_type: "image/jpeg".into(),
            sha256_hex: "a".repeat(64),
            original_sha256_hex: Some("b".repeat(64)),
            size: Some(2048),
            caption: "a sunset".into(),
        };
        let event = build_file_metadata_event(&keys, &meta).unwrap();
        assert_eq!(event.kind, Kind::from(FILE_METADATA_KIND));
        let parsed = parse_file_metadata(&event).unwrap();
        assert_eq!(parsed, meta);
    }

    #[test]
    fn zero_knowledge_event_leaks_no_key_or_plaintext_hash() {
        // The public NIP-94 event must carry only URL + ciphertext hash — never
        // the AES key (derived via ECDH) nor the plaintext (`ox`) hash.
        let keys = Keys::generate();
        let (media, _secret) = encrypt_media(b"secret photo", "image/jpeg", &key()).unwrap();
        let meta = FileMetadata {
            url: "https://blob.example/abc".into(),
            mime_type: "image/jpeg".into(),
            sha256_hex: media.sha256_hex.clone(),
            original_sha256_hex: None,
            size: Some(media.size),
            caption: String::new(),
        };
        let event = build_file_metadata_event(&keys, &meta).unwrap();
        let json = serde_json::to_string(&event).unwrap();
        assert!(!json.contains(&hex::encode(key())), "AES key must not leak");
        let parsed = parse_file_metadata(&event).unwrap();
        assert_eq!(parsed.original_sha256_hex, None);
        assert_eq!(parsed.sha256_hex, media.sha256_hex);
    }

    #[cfg(not(feature = "media-http"))]
    #[tokio::test]
    async fn http_paths_degrade_gracefully_without_feature() {
        // No panics, just typed errors, when the network feature is disabled.
        assert!(upload_encrypted_blob(vec![1, 2, 3], "image/png")
            .await
            .is_err());
        assert!(fetch_and_decrypt_media("https://x/y", &[0u8; 32])
            .await
            .is_err());
    }

    #[test]
    fn parse_rejects_event_without_url() {
        let keys = Keys::generate();
        let event = EventBuilder::new(Kind::from(FILE_METADATA_KIND), "no tags")
            .sign_with_keys(&keys)
            .unwrap();
        assert!(parse_file_metadata(&event).is_err());
    }

    #[tokio::test]
    async fn in_memory_uploader_stores_and_serves() {
        let uploader = InMemoryUploader::new("https://blob.example");
        let receipt = uploader
            .upload(b"opaque", "application/octet-stream")
            .await
            .unwrap();
        assert!(receipt.url.starts_with("https://blob.example/"));
        assert_eq!(uploader.fetch(&receipt.url).await, Some(b"opaque".to_vec()));
    }

    #[tokio::test]
    async fn full_pipeline_encrypt_upload_describe_recover() {
        let keys = Keys::generate();
        let uploader = InMemoryUploader::new("https://blob.example");
        let engine = MediaEngine::new(uploader.clone(), keys);

        let original = b"the secret photo bytes";
        let (event, secret) = engine
            .share_encrypted(original, "image/jpeg", "for your eyes only", &key())
            .await
            .unwrap();

        // The public event describes the upload...
        let meta = parse_file_metadata(&event).unwrap();
        assert_eq!(meta.caption, "for your eyes only");
        assert_eq!(meta.mime_type, "image/jpeg");

        // ...and the recipient can fetch + decrypt the blob back to the original.
        let blob = uploader.fetch(&meta.url).await.expect("blob present");
        assert_eq!(blob.len(), meta.size.unwrap());
        let recovered = decrypt_media(&blob, &secret).unwrap();
        assert_eq!(recovered, original);
    }
}
