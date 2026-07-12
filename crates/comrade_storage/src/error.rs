use thiserror::Error;

/// Errors surfaced by the encrypted local storage layer.
#[derive(Debug, Error)]
pub enum StorageError {
    #[error("database error: {0}")]
    Db(String),

    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("encryption failed")]
    Encrypt,

    /// Returned when AES-GCM authentication fails — either the PIN is wrong
    /// or the on-disk record was tampered with / corrupted.
    #[error("decryption failed: wrong PIN or corrupted data")]
    Decrypt,

    #[error("key derivation failed: {0}")]
    KeyDerivation(String),

    /// The supplied PIN did not match the one the store was created with.
    #[error("invalid PIN")]
    InvalidPin,

    #[error("stored record is malformed: {0}")]
    Corrupt(String),
}

impl From<sled::Error> for StorageError {
    fn from(e: sled::Error) -> Self {
        StorageError::Db(e.to_string())
    }
}

impl From<redb::DatabaseError> for StorageError {
    fn from(e: redb::DatabaseError) -> Self {
        StorageError::Db(e.to_string())
    }
}

impl From<redb::TransactionError> for StorageError {
    fn from(e: redb::TransactionError) -> Self {
        StorageError::Db(e.to_string())
    }
}

impl From<redb::TableError> for StorageError {
    fn from(e: redb::TableError) -> Self {
        StorageError::Db(e.to_string())
    }
}

impl From<redb::CommitError> for StorageError {
    fn from(e: redb::CommitError) -> Self {
        StorageError::Db(e.to_string())
    }
}

impl From<redb::StorageError> for StorageError {
    fn from(e: redb::StorageError) -> Self {
        StorageError::Db(e.to_string())
    }
}
