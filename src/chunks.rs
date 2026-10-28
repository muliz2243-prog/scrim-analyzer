//! Decomposition and decompression of the payload chunks.
//!
//! The payload is the concatenation of all chunks, each one compressed with
//! zlib (method 1) or uncompressed (method 0). The result is written into a
//! single `Vec<u8>` reusing the reserved capacity: no per-chunk allocations
//! and no double copies.

use std::io::Read;

use flate2::read::{DeflateDecoder, ZlibDecoder};

use crate::cursor::Cursor;
use crate::error::RoflError;
use crate::format::*;

/// Decompresses all chunks `[payload_start, footer_offset)` into a buffer.
pub fn decompress_payload(
    file: &[u8],
    payload_start: usize,
    footer_offset: u64,
    chunk_count: u32,
    warnings: &mut Vec<String>,
) -> Result<Vec<u8>, RoflError> {
    let end_limit = (footer_offset as usize).min(file.len()).max(payload_start);
    let mut c = Cursor::new(file);
    c.seek_to(payload_start)?;

    // Initial estimated reservation: ~1 MiB avoids early reallocations.
    let mut payload: Vec<u8> = Vec::with_capacity(1024 * 1024);
    let mut index: usize = 0;

    while c.pos().saturating_add(CHUNK_HEADER_SIZE) <= end_limit {
        if payload.len() >= MAX_PAYLOAD_LEN {
            warnings.push(format!(
                "chunks: se alcanzó el límite de payload ({MAX_PAYLOAD_LEN} bytes); se corta"
            ));
            break;
        }

        let chunk_offset = c.pos() as u64;
        let compressed_len = c.read_u32_le()? as usize;
        let uncompressed_len = c.read_u32_le()? as usize;
        let method = c.read_u8()?;
        c.skip(3)?; // 3 unknown bytes / padding

        // Empty chunk: end of the data block.
        if compressed_len == 0 && uncompressed_len == 0 {
            break;
        }
        if uncompressed_len > MAX_CHUNK_UNCOMPRESSED {
            warnings.push(format!(
                "chunks: chunk {index} declara {uncompressed_len} bytes descomprimidos (> \
                 {MAX_CHUNK_UNCOMPRESSED}); se ignora"
            ));
            break;
        }
        let available = end_limit.saturating_sub(c.pos());
        if compressed_len > available {
            warnings.push(format!(
                "chunks: chunk {index} declara {compressed_len} bytes comprimidos pero sólo quedan \
                 {available}; archivo truncado"
            ));
            break;
        }

        let data = c.read_bytes(compressed_len)?;
        match method {
            COMPRESSION_NONE => {
                payload.extend_from_slice(data);
            }
            COMPRESSION_ZLIB => {
                let before = payload.len();
                payload.reserve(uncompressed_len);
                let ok = read_limited(&mut ZlibDecoder::new(data), &mut payload, uncompressed_len)
                    || read_limited(&mut DeflateDecoder::new(data), &mut payload, uncompressed_len);
                if !ok {
                    payload.truncate(before);
                    warnings.push(format!(
                        "chunks: chunk {index} (offset {chunk_offset}) no pudo descomprimirse con \
                         zlib ni deflate"
                    ));
                }
            }
            other => {
                warnings.push(format!(
                    "chunks: chunk {index} usa método de compresión desconocido {other}; se ignora"
                ));
            }
        }

        index += 1;
        if chunk_count > 0 && index as u32 >= chunk_count {
            break;
        }
    }

    if payload.is_empty() {
        return Err(RoflError::ChunkDecompress {
            index: 0,
            offset: payload_start as u64,
            reason: "no se extrajo ningún chunk útil".to_string(),
        });
    }
    Ok(payload)
}

/// Decompresses while limiting the output to `max_out` bytes (protects memory
/// against "zip bombs"). Writes directly into `out` (append).
fn read_limited<R: Read>(reader: &mut R, out: &mut Vec<u8>, max_out: usize) -> bool {
    let before = out.len();
    let mut limited = reader.take(max_out as u64 + 1);
    match limited.read_to_end(out) {
        // +1 to detect that the declared limit was exceeded.
        Ok(_) => out.len() - before <= max_out,
        Err(_) => {
            out.truncate(before);
            false
        }
    }
}

/// Extracts only the decompressed payload (for debugging / patch RE).
pub fn extract_payload(
    file: &[u8],
    payload_start: usize,
    footer_offset: u64,
    chunk_count: u32,
) -> Result<Vec<u8>, RoflError> {
    let mut warnings = Vec::new();
    decompress_payload(file, payload_start, footer_offset, chunk_count, &mut warnings)
}
