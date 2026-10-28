//! Idiomatic error types for the parser (`thiserror`).
//!
//! Every function in this crate returns `Result<_, RoflError>`; `unwrap`/`panic`
//! is never used on unvalidated input data.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum RoflError {
    /// System I/O error (missing file, permissions, etc.).
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// The file does not start with `RIOT`.
    #[error("invalid magic: expected \"RIOT\", found {found:?}")]
    InvalidMagic { found: [u8; 4] },

    /// Read outside the buffer bounds (truncated or corrupt file).
    #[error(
        "read out of bounds at offset {offset}: needed {needed} bytes but only {remaining} remain"
    )]
    OutOfBounds {
        offset: usize,
        needed: usize,
        remaining: usize,
    },

    /// Size declared in the header that makes no sense (corrupt file).
    #[error("implausible size at offset {offset}: {len} bytes (maximum allowed {max})")]
    ImplausibleSize { offset: u64, len: u64, max: u64 },

    /// A JSON document (metadata or footer) is not valid.
    #[error("invalid JSON: {0}")]
    Json(#[from] serde_json::Error),

    /// The payload is corrupt or contains unsupported data.
    #[error("corrupt payload at offset {offset}: {reason}")]
    Payload { offset: usize, reason: String },

    /// A chunk could not be decompressed.
    #[error("chunk {index} (offset {offset}) could not be decompressed: {reason}")]
    ChunkDecompress {
        index: usize,
        offset: u64,
        reason: String,
    },

    /// The end-of-game footer is corrupt.
    #[error("invalid footer at offset {offset}: {reason}")]
    Footer { offset: u64, reason: String },

    /// The path does not point to a file.
    #[error("not an accessible file: {0}")]
    NotAFile(String),
}
