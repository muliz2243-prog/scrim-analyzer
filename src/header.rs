//! Header of the `.rofl`: magic, offsets, metadata JSON and footer.

use crate::cursor::Cursor;
use crate::error::RoflError;
use crate::format::*;
use crate::metadata::{
    RawFooter, RawMetadata, convert_metadata, footer_duration, footer_end_timestamp,
};
use crate::model::{GameEndInfo, Player, ReplayMetadata};

/// Result of header parsing.
pub struct ParsedHeader {
    pub metadata: ReplayMetadata,
    pub players: Vec<Player>,
    /// Absolute offset of the footer within the file.
    pub footer_offset: u64,
    /// Declared number of chunks (0 if unreliable).
    pub chunk_count: u32,
    /// Absolute offset where the first chunk header starts.
    pub payload_start: usize,
}

/// Parses the header of the whole file.
pub fn parse_header(file: &[u8], warnings: &mut Vec<String>) -> Result<ParsedHeader, RoflError> {
    let mut c = Cursor::new(file);

    // 1. Magic "RIOT"
    let magic = c.read_bytes(4)?;
    let mut found = [0u8; 4];
    found.copy_from_slice(magic);
    if &found != MAGIC {
        return Err(RoflError::InvalidMagic { found });
    }

    // 2. header_length (u8) — length of the header block after the magic.
    let _header_length = c.read_u8()?;

    // 3. Footer offset (u64 LE)
    let footer_offset = c.read_u64_le()?;
    if footer_offset > file.len() as u64 {
        return Err(RoflError::ImplausibleSize {
            offset: 0x05,
            len: footer_offset,
            max: file.len() as u64,
        });
    }

    // 4. Metadata JSON
    let metadata_offset = c.pos() as u64;
    let metadata_len = u64::from(c.read_u32_le()?);
    if metadata_len == 0 || metadata_len > MAX_METADATA_LEN {
        return Err(RoflError::ImplausibleSize {
            offset: metadata_offset,
            len: metadata_len,
            max: MAX_METADATA_LEN,
        });
    }
    let metadata_bytes = c.read_bytes(metadata_len as usize)?;
    let metadata_str: std::borrow::Cow<'_, str> = match std::str::from_utf8(metadata_bytes) {
        Ok(s) => std::borrow::Cow::Borrowed(s),
        Err(_) => {
            warnings.push(
                "metadata: la cadena JSON no es UTF-8 válido; se usa decodificación lossy"
                    .to_string(),
            );
            std::borrow::Cow::Owned(String::from_utf8_lossy(metadata_bytes).into_owned())
        }
    };

    let raw_meta: RawMetadata = serde_json::from_str(metadata_str.trim_end_matches('\0'))?;
    let (metadata, players) = convert_metadata(raw_meta);

    // 5. Number of chunks (u32 LE). Some files declare it incorrectly: if it
    //    is not plausible, we trust footer_offset to delimit the payload.
    let chunk_count_offset = c.pos() as u64;
    let chunk_count = c.read_u32_le().unwrap_or(0);
    let chunk_count = if chunk_count == 0 || chunk_count > MAX_CHUNKS {
        warnings.push(format!(
            "cabecera: recuento de chunks no fiable ({chunk_count}) en offset {chunk_count_offset}; \
             se delimita el payload por el footer_offset"
        ));
        0
    } else {
        chunk_count
    };

    Ok(ParsedHeader {
        metadata,
        players,
        footer_offset,
        chunk_count,
        payload_start: c.pos(),
    })
}

/// Parses the footer (end-of-game JSON). It is tolerant: on failure it
/// returns `None` and adds a warning, because the rest of the analysis is valid.
pub fn parse_footer(
    file: &[u8],
    footer_offset: u64,
    warnings: &mut Vec<String>,
) -> Result<Option<GameEndInfo>, RoflError> {
    if footer_offset == 0 || footer_offset as usize >= file.len() {
        warnings.push(format!(
            "footer: offset {footer_offset} fuera del archivo; se omite la info de fin de partida"
        ));
        return Ok(None);
    }

    let mut c = Cursor::new(file);
    c.seek_to(footer_offset as usize)?;

    let read = |c: &mut Cursor<'_>| -> Result<GameEndInfo, RoflError> {
        c.skip(FOOTER_KNOWN_SIZE)?; // 8 unknown bytes
        let len = u64::from(c.read_u32_le()?);
        if len == 0 || len > MAX_METADATA_LEN {
            return Err(RoflError::ImplausibleSize {
                offset: footer_offset,
                len,
                max: MAX_METADATA_LEN,
            });
        }
        let bytes = c.read_bytes(len as usize)?;
        let text = match std::str::from_utf8(bytes) {
            Ok(s) => s,
            Err(_) => return Err(RoflError::Footer { offset: footer_offset, reason: "JSON no UTF-8".into() }),
        };
        let raw: RawFooter = serde_json::from_str(text.trim_end_matches('\0'))?;
        Ok(GameEndInfo {
            game_length_s: footer_duration(&raw),
            game_end_timestamp_unix: footer_end_timestamp(&raw),
        })
    };

    match read(&mut c) {
        Ok(info) => Ok(Some(info)),
        Err(err) => {
            warnings.push(format!("footer: no se pudo interpretar ({err}); se omite"));
            Ok(None)
        }
    }
}
