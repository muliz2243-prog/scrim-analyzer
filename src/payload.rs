//! Payload walk: telemetry frames and event blocks.
//!
//! Layout of each element (community reverse engineering):
//!
//! ```text
//! [u8 type] [u32 LE timestamp_ms] [body]
//!   1 = keyframe   : [u32 number]
//!   2 = gameflow   : [u32 state]
//!   3 = frame      : [u8; 2] + 10 × participant block (108 fixed bytes)
//!   4 = events     : [u8; 2] + [u16 number] + N × ([u16 id] + data)
//! ```
//!
//! The parser is *defensive*: if it finds an event of unknown size or
//! unexpected bytes, it emits a warning, tries to re-synchronize at the next
//! valid element and continues. It never panics on input data.

use crate::cursor::Cursor;
use crate::error::RoflError;
use crate::format::*;
use crate::model::{EventKind, FrameSnapshot, GameEvent, ParticipantState, WardType};

/// Maximum plausible timestamp: 10 hours in milliseconds.
const MAX_TIMESTAMP_MS: u32 = 36_000_000;
/// Search window when re-synchronizing after corrupt bytes.
const RESYNC_WINDOW: usize = 8 * 1024;

/// Result of the payload walk.
pub struct PayloadData {
    /// Sampled frames (position of the 10 champions per tick).
    pub frames: Vec<FrameSnapshot>,
    /// Vision events (wards placed / destroyed).
    pub events: Vec<GameEvent>,
}

/// Walks the whole payload with time sampling.
///
/// * `sample_ms` — keeps one frame every N milliseconds (the first frame is
///   always kept). Use 0 or 1 for all frames.
pub fn parse_payload(
    payload: &[u8],
    sample_ms: u32,
    warnings: &mut Vec<String>,
) -> PayloadData {
    let sample_ms = sample_ms.max(1);
    let mut frames: Vec<FrameSnapshot> = Vec::with_capacity(4096);
    let mut events: Vec<GameEvent> = Vec::with_capacity(256);
    let mut last_kept_ms: Option<u32> = None;

    let mut c = Cursor::new(payload);

    while c.remaining() > 0 {
        let elem_start = c.pos();

        let kind = match c.read_u8() {
            Ok(k) => k,
            Err(_) => break, // clean end of the buffer
        };
        let timestamp_ms = match c.read_u32_le() {
            Ok(t) => t,
            Err(_) => {
                warnings.push(format!(
                    "payload: elemento sin timestamp en offset {elem_start}; se corta el recorrido"
                ));
                break;
            }
        };

        match kind {
            PAYLOAD_KEYFRAME | PAYLOAD_GAMEFLOW => {
                if c.skip(4).is_err() {
                    warnings.push(format!(
                        "payload: elemento tipo {kind} truncado en offset {elem_start}"
                    ));
                    break;
                }
            }
            PAYLOAD_FRAME => match parse_frame(&mut c, timestamp_ms) {
                Ok(frame) => {
                    let due = match last_kept_ms {
                        None => true,
                        Some(prev) => timestamp_ms.saturating_sub(prev) >= sample_ms,
                    };
                    if due {
                        last_kept_ms = Some(timestamp_ms);
                        frames.push(frame);
                    }
                }
                Err(err) => {
                    warnings.push(format!(
                        "payload: frame truncado en offset {elem_start} ({err}); re-sincronizando"
                    ));
                    if !resync(&mut c, elem_start + 5, payload, timestamp_ms) {
                        warnings.push("payload: no se pudo re-sincronizar; se corta".to_string());
                        break;
                    }
                }
            },
            PAYLOAD_EVENT_BLOCK => {
                if let Err(err) = parse_event_block(&mut c, timestamp_ms, &mut events) {
                    warnings.push(format!(
                        "payload: bloque de eventos en offset {elem_start} con dato no soportado \
                         ({err}); re-sincronizando"
                    ));
                    if !resync(&mut c, elem_start + 5, payload, timestamp_ms) {
                        warnings.push("payload: no se pudo re-sincronizar; se corta".to_string());
                        break;
                    }
                }
            }
            other => {
                warnings.push(format!(
                    "payload: tipo de elemento desconocido {other} en offset {elem_start}; se corta \
                     el recorrido"
                ));
                break;
            }
        }
    }

    PayloadData { frames, events }
}

/// Parses a frame (type 3): 10 participants of fixed block size.
fn parse_frame(c: &mut Cursor<'_>, timestamp_ms: u32) -> Result<FrameSnapshot, RoflError> {
    c.skip(FRAME_HEADER_SIZE)?;

    let mut participants = std::array::from_fn(|_| ParticipantState::default());
    for slot in 0..MAX_PLAYERS {
        let block_start = c.pos();

        c.skip(FRAME_PARTICIPANT_SKIP_BEFORE)?;
        let x = c.read_f32_le()?;
        let y = c.read_f32_le()?;
        let _z = c.read_f32_le()?;
        c.skip(FRAME_PARTICIPANT_SKIP_MID)?;
        let health = c.read_u16_le()?;
        let max_health = c.read_u16_le()?;
        let mana = c.read_u16_le()?;
        let max_mana = c.read_u16_le()?;
        let _xp = c.read_u32_le()?;
        let level = c.read_u8()?;

        // Fixed-size block: alignment with the next participant is guaranteed
        // even if the internal layout changes in a future patch.
        c.seek_to(block_start.saturating_add(FRAME_PARTICIPANT_BLOCK))?;

        participants[slot] = ParticipantState {
            x,
            y,
            health,
            max_health,
            mana,
            max_mana,
            level,
            alive: health > 0,
        };
    }

    Ok(FrameSnapshot {
        timestamp_ms,
        participants,
    })
}

/// Parses an event block (type 4) and extracts the vision events.
fn parse_event_block(
    c: &mut Cursor<'_>,
    timestamp_ms: u32,
    events: &mut Vec<GameEvent>,
) -> Result<(), RoflError> {
    c.skip(2)?; // 2 unknown bytes of block header
    let count = c.read_u16_le()?;

    for _ in 0..count {
        let id = c.read_u16_le()?;
        match id {
            EVENT_ID_WARD_PLACED => {
                let (ward_type, x, y) = read_ward_data(c)?;
                events.push(GameEvent {
                    timestamp_ms,
                    kind: EventKind::WardPlaced { ward_type, x, y },
                });
            }
            EVENT_ID_WARD_KILL => {
                let (ward_type, x, y) = read_ward_data(c)?;
                events.push(GameEvent {
                    timestamp_ms,
                    kind: EventKind::WardKill { ward_type, x, y },
                });
            }
            _ => {
                // Unknown size: we cannot stay aligned.
                return Err(RoflError::Payload {
                    offset: c.pos(),
                    reason: format!("evento no soportado id={id}"),
                });
            }
        }
    }
    Ok(())
}

/// Data of a ward: `[u8; 2]` + ward type string + `f32 x` + `f32 y`.
///
/// Both observed layout variants are tolerated (with and without the 2
/// preceding bytes) and the result is validated to discard false positives.
fn read_ward_data(c: &mut Cursor<'_>) -> Result<(WardType, f32, f32), RoflError> {
    let mark = c.pos();

    if let Ok(out) = read_ward_variant(c, 2) {
        return Ok(out);
    }
    c.seek_to(mark)?;
    read_ward_variant(c, 0)
}

fn read_ward_variant(c: &mut Cursor<'_>, prefix: usize) -> Result<(WardType, f32, f32), RoflError> {
    c.skip(prefix)?;
    let raw = c.read_dotnet_string()?;
    if raw.is_empty()
        || raw.len() > MAX_WARD_TYPE_LEN
        || !raw.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
    {
        return Err(RoflError::Payload {
            offset: c.pos(),
            reason: format!("tipo de ward inválido {raw:?}"),
        });
    }
    let x = c.read_f32_le()?;
    let y = c.read_f32_le()?;
    if !x.is_finite() || !y.is_finite() || !in_map_bounds(x) || !in_map_bounds(y) {
        return Err(RoflError::Payload {
            offset: c.pos(),
            reason: format!("coordenadas de ward fuera de rango ({x}, {y})"),
        });
    }
    Ok((WardType::from_raw(raw), x, y))
}

/// Plausible Summoner's Rift map coordinates (0..=15000 with margin).
#[inline]
fn in_map_bounds(v: f32) -> bool {
    (-2_000.0..=20_000.0).contains(&v)
}

/// Searches from `from` for the next valid payload element and jumps to it.
fn resync(c: &mut Cursor<'_>, from: usize, payload: &[u8], min_ts: u32) -> bool {
    let end = (from.saturating_add(RESYNC_WINDOW)).min(payload.len().saturating_sub(5));
    let mut i = from;
    while i <= end {
        let kind = payload[i];
        if (PAYLOAD_KEYFRAME..=PAYLOAD_EVENT_BLOCK).contains(&kind) {
            let ts = u32::from_le_bytes([
                payload[i + 1],
                payload[i + 2],
                payload[i + 3],
                payload[i + 4],
            ]);
            if ts <= MAX_TIMESTAMP_MS && ts >= min_ts.saturating_sub(2_000) {
                return c.seek_to(i).is_ok();
            }
        }
        i += 1;
    }
    false
}
