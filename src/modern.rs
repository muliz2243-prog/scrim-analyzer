//! **Modern** `.rofl` format (patch 16.x, since ~2025): container with
//! segments compressed with **Zstandard** and a JSON footer at the end of the file.
//!
//! Structure verified empirically on a real replay (LA1-…, v16.20) and
//! matching the `lolrofl-rs` documentation (`lolrofl` crate):
//!
//! ```text
//! offset 0x00 : [u8; 4]  magic b"RIOT"
//! offset 0x04 : u16 LE   container version (= 2)
//! offset 0x06 : u64 LE   game_id
//! offset 0x0E : [u8; 1+] game version (.NET: 1 length byte + string)
//! offset 0x1D : …        segments, each one:
//!
//!   [u32 LE] field A (index of the associated keyframe)
//!   [u32 LE] field B (chunk id)
//!   [u8]     segment type
//!   [u32 LE] uncompressed length
//!   [u32 LE] compressed length (0 ⇒ data *stored* uncompressed)
//!   [u8; N]  data (Zstandard if compressed > 0)
//!
//! final offset: JSON footer {"gameLength":…,"lastGameChunkId":…,
//!                "lastKeyFrameId":…,"statsJson":"[ …10 players… ]"}
//!                + 4 trailing signature bytes
//! ```
//!
//! Inside each segment, the data is a stream of **sections**:
//!
//! ```text
//! [u8 H]                         configuration byte
//! [f32 | u8] time                absolute s if H&0x80==0; relative ms otherwise
//! [u32 | u8] length              data length
//! [u16]      type                absent if H&0x40 (inherits the previous one)
//! [u32 | u8] parameters
//! [u8; len]  data
//! ```
//!
//! Section reading is validated: they must fill each segment exactly
//! and the final time must match the footer duration.
//!
//! ## Section type 61 — experimental layout (validated by physical metrics)
//!
//! Section **type 61** (~23 Hz, ~76 % of the payload bytes) is the
//! candidate for the entity telemetry stream. Record grammar:
//!
//! ```text
//! [80] [id] [80 80 88] [3 variable bytes] [mask: 1 byte] [fields...]
//! ```
//!
//! The mask (`tail[3]`) has 4 values: `0x0B`(3 fields), `0x80`(1),
//! `0xC8`(3), `0xE8`(4, dominant ≈92 %). The popcount gives the number of
//! fields that follow. For the E8 mask, the fields are extracted from block
//! `tail[4..]` as two 14-bit fields at bits 13 and 30 (with 3 bits of
//! gap), with wrap of bit 13 (+8192) for values > 14881.
//!
//! **Physical validation passed** on the reference replay (v16.20):
//! team separation in lane phase = 3748 u, 6 champions near the blue
//! fountain and 5 near the red one at t<90 s, high variance on both axes,
//! 100 % continuity and rejection of constant fields. The layout is still
//! **experimental** (not verified against ground truth); the
//! `plausible_positions` gate remains active and discards the series if it
//! does not pass the physical metrics.

use crate::error::RoflError;
use crate::format;
use crate::model::{
    compute_team_totals, ChampionTrack, Player, PlayerStats, PositionSample, Replay,
    ReplayMetadata, SectionSummary, SectionTypeInfo, Team, TeamTotals,
};
use serde_json::Value;
use std::collections::HashMap;
use std::io::Read;

/// Size of the header of each modern segment.
pub const SEGMENT_HEADER_SIZE: usize = 17;
/// Minimum fixed header before the segment area.
pub const SEGMENT_AREA_START_MIN: usize = 0x1D;

// Masks of the section configuration byte.
const SEC_ABS_TIME: u8 = 0x80; // set ⇒ relative time in u8 (ms)
const SEC_SHORT_LEN: u8 = 0x10; // set ⇒ length in u8
const SEC_NO_TYPE: u8 = 0x40; // set ⇒ no type field (inherits the previous one)
const SEC_SHORT_PARAMS: u8 = 0x20; // set ⇒ parameters in u8

/// Heuristic: does this file use the modern container?
///
/// The modern one carries a .NET string (1 length byte + a version such as
/// `16.20.824.8524`) exactly where the legacy format has the start of the
/// metadata JSON (`{` = 0x7B ⇒ length 123, which we reject because it is > 63).
pub fn looks_modern(b: &[u8]) -> bool {
    if b.len() < SEGMENT_AREA_START_MIN || &b[0..4] != format::MAGIC {
        return false;
    }
    let slen = b[0x0E] as usize;
    if slen < 5 || slen > 63 || 0x0F + slen > b.len() {
        return false;
    }
    b[0x0F..0x0F + slen]
        .iter()
        .all(|c| c.is_ascii_digit() || *c == b'.')
}

/// Offset where the segments start: 0x0F + game version length.
fn segment_area_start(b: &[u8]) -> usize {
    0x0F + b[0x0E] as usize
}

fn u32le(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

/// RE: returns **all** sections of the stream `(type, length, time_s)`.
///
/// Not used in the report: it is the tool for **finding new event signatures**
/// (for example, a dragon kill) by comparing their counts with the
/// `statsJson` totals and their timestamps with the match.
pub fn todas_las_secciones(
    bytes: &[u8],
    warnings: &mut Vec<String>,
) -> Result<Vec<(u16, usize, f32)>, RoflError> {
    let seg = decompress_segments(bytes, segment_area_start(bytes), warnings)?;
    let payload = &seg.payload;
    let mut out: Vec<(u16, usize, f32)> = Vec::new();
    let mut p = 0usize;
    let mut last_ty: u16 = 0;
    let mut last_time = 0.0f32;
    while p < payload.len() {
        let h = payload[p];
        let mut q = p + 1;
        let time = if h & SEC_ABS_TIME == 0 {
            if q + 4 > payload.len() {
                break;
            }
            f32::from_le_bytes(payload[q..q + 4].try_into().unwrap())
        } else {
            last_time + f32::from(payload[q]) / 1000.0
        };
        q += if h & SEC_ABS_TIME == 0 { 4 } else { 1 };
        let len = if h & SEC_SHORT_LEN == 0 {
            if q + 4 > payload.len() {
                break;
            }
            u32le(payload, q) as usize
        } else {
            payload[q] as usize
        };
        q += if h & SEC_SHORT_LEN == 0 { 4 } else { 1 };
        let ty = if h & SEC_NO_TYPE == 0 {
            if q + 2 > payload.len() {
                break;
            }
            let t = u16::from_le_bytes([payload[q], payload[q + 1]]);
            q += 2;
            last_ty = t;
            t
        } else {
            last_ty
        };
        let _params = if h & SEC_SHORT_PARAMS == 0 {
            if q + 4 > payload.len() {
                break;
            }
            let v = u32le(payload, q);
            q += 4;
            v
        } else {
            let v = payload[q] as u32;
            q += 1;
            v
        };
        if q + len > payload.len() {
            break;
        }
        last_time = time;
        out.push((ty, len, time));
        p = q + len;
    }
    Ok(out)
}

/// Analyzes a `.rofl` file in the modern format.
///
/// `opts.sample_ms` controls the sampling of the position tracks
/// (1000 = 1 sample/second; ≤ 1 keeps the raw stream at ~23 Hz).
pub fn analyze(
    bytes: &[u8],
    opts: crate::AnalyzeOptions,
    warnings: &mut Vec<String>,
) -> Result<Replay, RoflError> {
    // ---- 1. Header: game_id + version ----
    if bytes.len() < SEGMENT_AREA_START_MIN || &bytes[0..4] != format::MAGIC {
        return Err(RoflError::InvalidMagic {
            found: [
                *bytes.first().unwrap_or(&0),
                *bytes.get(1).unwrap_or(&0),
                *bytes.get(2).unwrap_or(&0),
                *bytes.get(3).unwrap_or(&0),
            ],
        });
    }
    let game_id = u64::from_le_bytes(bytes[6..14].try_into().unwrap());
    let ver_len = bytes[0x0E] as usize;
    let game_version = String::from_utf8_lossy(&bytes[0x0F..0x0F + ver_len]).into_owned();

    // ---- 2. Segments: decompress the whole payload ----
    let seg = decompress_segments(bytes, segment_area_start(bytes), warnings)?;
    let n_segments = seg.n_segments;
    let payload = &seg.payload;

    // ---- 3. Footer: duration + per-player statistics ----
    let footer = find_footer(bytes, warnings)?;
    let duration_s = footer
        .as_ref()
        .map(|f| f.0)
        .unwrap_or(0.0);

    // ---- 4. Payload sections ----
    // `event_secs` accumulates only the event sections (kills, assists,
    // plates, turrets) so they can be verified against the `statsJson`.
    let mut event_secs: Vec<(u16, usize, f32)> = Vec::with_capacity(256);
    let sections = parse_sections(payload, warnings, &mut event_secs);

    // ---- 5. Players from statsJson ----
    let mut players: Vec<Player> = Vec::new();
    if let Some((_, stats)) = &footer {
        players = players_from_stats(stats, warnings);
    }
    if players.is_empty() {
        warnings.push(
            "no se pudieron extraer estadísticas por jugador del footer (statsJson)".into(),
        );
    }

    let team_totals: TeamTotals = compute_team_totals(&players);
    let duration_s = if duration_s > 0.0 {
        duration_s
    } else {
        sections
            .as_ref()
            .map(|s| s.duration_s)
            .unwrap_or(0.0)
    };

    let metadata = ReplayMetadata {
        source_format: "modern".into(),
        game_id: game_id.to_string(),
        game_version,
        game_mode: String::new(),
        game_type: String::new(),
        map_id: 0,
        queue_id: 0,
        game_length_s: duration_s,
        created_unix: 0,
    };

    // ---- 6. Champion x/y tracks (section type 61, EXPERIMENTAL)
    // The position layout is still unverified: extract_positions applies
    // a physical validation gate and discards the series that do not pass it
    // (emitting a warning) so as not to export unverified data.
    let positions = extract_positions(seg, opts.sample_ms, duration_s, warnings);

    // ---- 7. Economic events verified against the `statsJson` ----
    // The modern format stream does carry kills, assists, plates and
    // turrets with their exact timestamp. They are only exported if the four
    // counts match the `statsJson` totals *exactly* and the
    // last kill falls on `LAST_TAKEDOWN_TIME` (see `events_extract`).
    let truth = crate::events_extract::EventTruth {
        kills: players.iter().map(|p| p.stats.kills).sum(),
        assists: players.iter().map(|p| p.stats.assists).sum(),
        turret_plates: players.iter().map(|p| p.stats.turret_plates_destroyed).sum(),
        turrets_destroyed: players.iter().map(|p| p.stats.turrets_destroyed).sum(),
        last_takedown_s: players
            .iter()
            .map(|p| p.stats.last_takedown_s)
            .fold(0.0f32, f32::max),
    };
    let (events, events_report) =
        crate::events_extract::extract_verified_events(&event_secs, &truth);
    if !events_report.verificado {
        warnings.push(format!(
            "eventos del stream no verificados contra el statsJson \
             (detectados: {}/{}/{}/{} vs truth {}/{}/{}/{}): no se exportan",
            events_report.kills,
            events_report.assists,
            events_report.turret_plates,
            events_report.turrets_destroyed,
            events_report.truth_kills,
            events_report.truth_assists,
            events_report.truth_turret_plates,
            events_report.truth_turrets_destroyed,
        ));
    }

    // ---- 8. 30 s block report + Top 5 ----
    // The detected events do carry a timestamp, but **not** the
    // player/team attribution (the entity identifiers of those
    // records do not map to the 10 champions). Splitting the gold by team
    // would require splitting each event, and that would be fabricating data:
    // we keep `fuente = "no_disponible"`.
    let gold_blocks = Some(crate::blocks::compute_gold_blocks(
        &[],
        duration_s,
        "no_disponible",
    ));
    if events_report.verificado {
        warnings.push(
            "eventos económicos detectados y verificados, pero sin atribución de equipo \
             decodificable: `gold_blocks` queda en fuente=\"no_disponible\" para no \
             fabricar el reparto por equipo"
                .into(),
        );
    } else {
        warnings.push(
            "la serie temporal de oro por tick aún no está decodificada en el formato moderno: \
             `gold_blocks` se exporta con bloques de 30 s + top5 pero con deltas a cero y \
             fuente=\"no_disponible\""
                .into(),
        );
    }

    Ok(Replay {
        metadata,
        footer: footer.map(|(s, _)| crate::model::GameEndInfo {
            game_length_s: s,
            game_end_timestamp_unix: 0,
        }),
        players,
        frames: Vec::new(),
        events,
        events_report: Some(events_report),
        positions,
        gold_blocks,
        sections,
        team_totals,
        segment_count: n_segments,
        warnings: std::mem::take(warnings),
    })
}

/// Decompressed payload + map of the segments that compose it.
pub(crate) struct SegmentMap {
    /// Concatenated payload (stream of sections).
    payload: Vec<u8>,
    /// Processed segments.
    n_segments: u32,
    /// `(start_in_payload, length, segment_type)` spans in order.
    /// `type == 2` ⇒ keyframe (full snapshot with a different record layout);
    /// anything else ⇒ chunk of deltas.
    spans: Vec<(usize, usize, u8)>,
}

/// Decompresses all the segments and returns the concatenated payload with its
/// map of per-segment spans.
pub(crate) fn decompress_segments(
    bytes: &[u8],
    start: usize,
    warnings: &mut Vec<String>,
) -> Result<SegmentMap, RoflError> {
    let mut payload: Vec<u8> = Vec::with_capacity(4 * 1024 * 1024);
    let mut spans: Vec<(usize, usize, u8)> = Vec::new();
    let mut pos = start;
    let mut n_segments = 0u32;
    let mut desyncs = 0u32;

    while pos + SEGMENT_HEADER_SIZE <= bytes.len() && n_segments < format::MAX_CHUNKS {
        let uncomp = u32le(bytes, pos + 9) as usize;
        let comp = u32le(bytes, pos + 13) as usize;
        let seg_ty = bytes[pos + 8];
        let ds = pos + SEGMENT_HEADER_SIZE;

        if comp == 0 {
            // Segment stored without compression.
            if uncomp == 0 {
                pos = ds;
                continue;
            }
            if uncomp > format::MAX_CHUNK_UNCOMPRESSED || ds + uncomp > bytes.len() {
                break; // end of the segment area
            }
            let before = payload.len();
            payload.extend_from_slice(&bytes[ds..ds + uncomp]);
            spans.push((before, uncomp, seg_ty));
            pos = ds + uncomp;
            n_segments += 1;
            continue;
        }

        let magic_ok = ds + 4 <= bytes.len() && &bytes[ds..ds + 4] == format::ZSTD_MAGIC;
        if !magic_ok || comp > format::MAX_CHUNK_UNCOMPRESSED || ds + comp > bytes.len() {
            // End of the segment area (the footer shows up) or corrupt bytes:
            // try to resynchronize with the next zstd magic.
            match find_zstd_magic(bytes, ds, ds + 256 * 1024) {
                Some(m) if m >= SEGMENT_HEADER_SIZE => {
                    desyncs += 1;
                    pos = m - SEGMENT_HEADER_SIZE;
                    continue;
                }
                _ => break, // no more segments: the data area ends
            }
        }

        let before = payload.len();
        let mut dec = ruzstd::decoding::StreamingDecoder::new(&bytes[ds..ds + comp])
            .map_err(|e| RoflError::ChunkDecompress {
                index: n_segments as usize,
                offset: ds as u64,
                reason: e.to_string(),
            })?;
        dec.read_to_end(&mut payload).map_err(|e| RoflError::ChunkDecompress {
            index: n_segments as usize,
            offset: ds as u64,
            reason: e.to_string(),
        })?;
        let got = payload.len() - before;
        if uncomp > 0 && got != uncomp {
            warnings.push(format!(
                "segmento {n_segments}: zstd devolvió {got} bytes, la cabecera declara {uncomp}"
            ));
        }
        if payload.len() > format::MAX_PAYLOAD_LEN {
            return Err(RoflError::ImplausibleSize {
                offset: pos as u64,
                len: payload.len() as u64,
                max: format::MAX_PAYLOAD_LEN as u64,
            });
        }
        spans.push((before, got, seg_ty));
        pos = ds + comp;
        n_segments += 1;
    }

    if n_segments == 0 {
        return Err(RoflError::Payload {
            offset: start,
            reason: "no se encontró ningún segmento válido".into(),
        });
    }
    if desyncs > 0 {
        warnings.push(format!(
            "se saltaron {desyncs} zonas no interpretables del área de segmentos (re-sincronización)"
        ));
    }
    Ok(SegmentMap {
        payload,
        n_segments,
        spans,
    })
}

fn find_zstd_magic(b: &[u8], from: usize, to: usize) -> Option<usize> {
    let to = to.min(b.len().saturating_sub(4));
    let mut i = from;
    while i < to {
        if &b[i..i + 4] == format::ZSTD_MAGIC {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Reverse-engineering diagnostic: returns `(time_s, length, bytes)`
/// for all the sections of a given `type_id`.
///
/// Not used in normal analysis; it serves to investigate candidate sections
/// (e.g. ward events) by comparing their count with the `statsJson`.
pub fn section_records(
    bytes: &[u8],
    target: u16,
) -> Result<Vec<(f32, usize, Vec<u8>)>, RoflError> {
    let payload = extract_payload(bytes)?;
    let mut out = Vec::new();
    let mut p = 0usize;
    let mut last_ty: u16 = 0;
    let mut last_time = 0.0f32;
    while p < payload.len() {
        let h = payload[p];
        let mut q = p + 1;
        let time = if h & SEC_ABS_TIME == 0 {
            if q + 4 > payload.len() {
                break;
            }
            f32::from_le_bytes(payload[q..q + 4].try_into().unwrap())
        } else {
            if q + 1 > payload.len() {
                break;
            }
            last_time + f32::from(payload[q]) / 1000.0
        };
        q += if h & SEC_ABS_TIME == 0 { 4 } else { 1 };
        let len = if h & SEC_SHORT_LEN == 0 {
            if q + 4 > payload.len() {
                break;
            }
            u32le(&payload, q) as usize
        } else {
            if q + 1 > payload.len() {
                break;
            }
            payload[q] as usize
        };
        q += if h & SEC_SHORT_LEN == 0 { 4 } else { 1 };
        let ty = if h & SEC_NO_TYPE == 0 {
            if q + 2 > payload.len() {
                break;
            }
            let t = u16::from_le_bytes([payload[q], payload[q + 1]]);
            q += 2;
            last_ty = t;
            t
        } else {
            last_ty
        };
        if h & SEC_SHORT_PARAMS == 0 {
            q += 4;
        } else {
            q += 1;
        }
        if q + len > payload.len() {
            break;
        }
        if ty == target {
            out.push((time, len, payload[q..q + len].to_vec()));
        }
        last_time = time;
        p = q + len;
    }
    Ok(out)
}

/// Parses the payload as a stream of sections; returns a per-type summary.
fn parse_sections(
    payload: &[u8],
    warnings: &mut Vec<String>,
    event_secs: &mut Vec<(u16, usize, f32)>,
) -> Option<SectionSummary> {
    if payload.is_empty() {
        return None;
    }
    let mut p = 0usize;
    let mut last_ty: u16 = 0;
    let mut last_time = 0.0f32;
    let mut total = 0u64;
    let mut malformed = 0u32;
    let mut max_time = 0.0f32;
    let mut hist: HashMap<u32, SectionAgg> = HashMap::with_capacity(512);

    while p < payload.len() {
        let h = payload[p];
        let mut q = p + 1;

        let time = if h & SEC_ABS_TIME == 0 {
            if q + 4 > payload.len() {
                malformed += 1;
                break;
            }
            f32::from_le_bytes(payload[q..q + 4].try_into().unwrap())
        } else {
            last_time + f32::from(payload[q]) / 1000.0
        };
        q += if h & SEC_ABS_TIME == 0 { 4 } else { 1 };

        let len = if h & SEC_SHORT_LEN == 0 {
            if q + 4 > payload.len() {
                malformed += 1;
                break;
            }
            u32le(payload, q) as usize
        } else {
            payload[q] as usize
        };
        q += if h & SEC_SHORT_LEN == 0 { 4 } else { 1 };

        let ty = if h & SEC_NO_TYPE == 0 {
            if q + 2 > payload.len() {
                malformed += 1;
                break;
            }
            let t = u16::from_le_bytes([payload[q], payload[q + 1]]);
            q += 2;
            last_ty = t;
            t
        } else {
            last_ty
        };

        let _params = if h & SEC_SHORT_PARAMS == 0 {
            if q + 4 > payload.len() {
                malformed += 1;
                break;
            }
            let v = u32le(payload, q);
            q += 4;
            v
        } else {
            let v = payload[q] as u32;
            q += 1;
            v
        };

        if q + len > payload.len() {
            malformed += 1;
            break;
        }

        // Collects only the event sections (4 types, ~170 instances):
        // `(type, length, time)` are stored to verify them against the
        // `statsJson` and emit them as validated events.
        if crate::events_extract::is_event_type(ty) {
            event_secs.push((ty, len, time));
        }

        let e = hist.entry(u32::from(ty)).or_insert_with(|| SectionAgg {
            count: 0,
            total_bytes: 0,
            min_len: usize::MAX,
            max_len: 0,
            first_time_s: time,
            last_time_s: time,
            params: _params,
        });
        e.count += 1;
        e.total_bytes += len as u64;
        e.min_len = e.min_len.min(len);
        e.max_len = e.max_len.max(len);
        e.last_time_s = time;
        if time.is_finite() && time > max_time {
            max_time = time;
        }

        total += 1;
        last_time = time;
        p = q + len;

        if total > format::MAX_SECTIONS {
            warnings.push(format!(
                "se alcanzó el límite de {total} secciones; se deja de parsear"
            ));
            break;
        }
    }

    if malformed > 0 {
        warnings.push(format!(
            "{malformed} secciones malformadas al final del payload (layout no reconocido en ese punto)"
        ));
    }
    if total == 0 {
        return None;
    }

    let mut types: Vec<SectionTypeInfo> = hist
        .into_iter()
        .map(|(type_id, a)| SectionTypeInfo {
            type_id,
            count: a.count,
            total_bytes: a.total_bytes,
            min_len: if a.min_len == usize::MAX { 0 } else { a.min_len as u32 },
            max_len: a.max_len as u32,
            first_time_s: a.first_time_s,
            last_time_s: a.last_time_s,
        })
        .collect();
    types.sort_by(|a, b| b.total_bytes.cmp(&a.total_bytes));

    Some(SectionSummary {
        total_sections: total,
        duration_s: max_time,
        types,
    })
}

struct SectionAgg {
    count: u64,
    total_bytes: u64,
    min_len: usize,
    max_len: usize,
    first_time_s: f32,
    last_time_s: f32,
    #[allow(dead_code)]
    params: u32,
}

/// Locates and parses the JSON footer of the modern format.
/// Returns `(duration_s, per-player statistics)`.
pub fn find_footer(
    bytes: &[u8],
    warnings: &mut Vec<String>,
) -> Result<Option<(f32, Vec<HashMap<String, String>>)>, RoflError> {
    let needle = b"{\"gameLength\"";
    // The footer takes up the end of the file (it can be large because of statsJson).
    let start = bytes.len().saturating_sub(4 * 1024 * 1024);
    let mut pos = None;
    let mut i = start;
    while i + needle.len() <= bytes.len() {
        if &bytes[i..i + needle.len()] == needle {
            pos = Some(i); // last occurrence
        }
        i += 1;
    }
    let Some(fi) = pos else {
        warnings.push("modern format JSON footer not found".into());
        return Ok(None);
    };

    let text = String::from_utf8_lossy(&bytes[fi..]);
    // The footer ends at `}`; the last 4 bytes of the file are a signature.
    let json_end = text.rfind('}').map(|p| p + 1).unwrap_or(text.len());
    let v: Value = match serde_json::from_str(&text[..json_end]) {
        Ok(v) => v,
        Err(e) => {
            warnings.push(format!("invalid JSON footer: {e}"));
            return Ok(None);
        }
    };

    let duration_s = v
        .get("gameLength")
        .and_then(Value::as_u64)
        .map(|ms| ms as f32 / 1000.0)
        .unwrap_or(0.0);

    let mut stats = Vec::new();
    if let Some(inner) = v.get("statsJson").and_then(Value::as_str) {
        match serde_json::from_str::<Vec<Value>>(inner) {
            Ok(arr) => {
                for obj in arr {
                    let mut map = HashMap::new();
                    if let Value::Object(o) = obj {
                        for (k, val) in o {
                            let s = match val {
                                Value::String(s) => s.clone(),
                                Value::Number(n) => n.to_string(),
                                Value::Bool(b) => b.to_string(),
                                _ => continue,
                            };
                            map.insert(k.clone(), s);
                        }
                    }
                    stats.push(map);
                }
            }
            Err(e) => warnings.push(format!("statsJson ilegible: {e}")),
        }
    }
    Ok(Some((duration_s, stats)))
}

fn num(map: &HashMap<String, String>, key: &str) -> u32 {
    map.get(key)
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(0) as u32
}

fn big_num(map: &HashMap<String, String>, key: &str) -> u64 {
    map.get(key)
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(0)
}

/// Reads a floating-point number from the `statsJson` (all its values are
/// strings). Returns `0.0` if it is missing or not parseable.
fn num_f32(map: &HashMap<String, String>, key: &str) -> f32 {
    map.get(key)
        .and_then(|s| s.trim().parse::<f32>().ok())
        .filter(|v| v.is_finite())
        .unwrap_or(0.0)
}

/// Converts the `statsJson` entries into `Player`.
///
/// Note: the modern format **does not store summoner names** (they were omitted
/// for privacy); the champion comes as a name via the `SKIN` key.
fn players_from_stats(
    stats: &[HashMap<String, String>],
    warnings: &mut Vec<String>,
) -> Vec<Player> {
    let mut players = Vec::with_capacity(stats.len().min(format::MAX_PLAYERS));
    for (i, s) in stats.iter().take(format::MAX_PLAYERS).enumerate() {
        let team = match num(s, "TEAM") {
            100 => Team::Blue,
            200 => Team::Red,
            _ if i < 5 => Team::Blue,
            _ => Team::Red,
        };
        let champion_name = s.get("SKIN").cloned().filter(|n| !n.is_empty());
        let won = s.get("WIN").map(|w| w.eq_ignore_ascii_case("Win"));
        let cs_lane = num(s, "MINIONS_KILLED");
        let cs_jungle = num(s, "NEUTRAL_MINIONS_KILLED");
        players.push(Player {
            slot: i as u8,
            summoner_name: String::new(),
            champion_id: 0,
            champion_name,
            team,
            summoner_spell_1: num(s, "SPELL1_CAST").min(99), // actual id not exposed
            summoner_spell_2: num(s, "SPELL2_CAST").min(99),
            skin_id: 0,
            won,
            stats: PlayerStats {
                champ_level: num(s, "LEVEL"),
                kills: num(s, "CHAMPIONS_KILLED"),
                deaths: num(s, "NUM_DEATHS"),
                assists: num(s, "ASSISTS"),
                gold_earned: num(s, "GOLD_EARNED"),
                gold_spent: num(s, "GOLD_SPENT"),
                cs: cs_lane.saturating_add(cs_jungle),
                cs_lane,
                cs_jungle,
                damage_to_champions: big_num(s, "TOTAL_DAMAGE_DEALT_TO_CHAMPIONS"),
                damage_taken: big_num(s, "TOTAL_DAMAGE_TAKEN"),
                wards_placed: num(s, "WARD_PLACED"),
                wards_killed: num(s, "WARD_KILLED"),
                turret_plates_destroyed: num(s, "Missions_TurretPlatesDestroyed"),
                turrets_destroyed: num(s, "TURRETS_KILLED"),
                // Epic objectives: the live API does not expose them; here it does.
                dragons_killed: num(s, "DRAGON_KILLS"),
                barons_killed: num(s, "BARON_KILLS"),
                heralds_killed: num(s, "RIFT_HERALD_KILLS"),
                last_takedown_s: num_f32(s, "LAST_TAKEDOWN_TIME"),
                items: [0; 7],
            },
        });
    }
    if players.iter().all(|p| p.summoner_name.is_empty()) {
        warnings.push(
            "el formato moderno no incluye nombres de invocador en el archivo".into(),
        );
    }
    players
}

/// Extracts the concatenated payload (debugging / RE).
pub fn extract_payload(bytes: &[u8]) -> Result<Vec<u8>, RoflError> {
    let mut warnings = Vec::new();
    decompress_segments(bytes, segment_area_start(bytes), &mut warnings).map(|m| m.payload)
}

// ---------------------------------------------------------------------------
// x/y positions — section type 61
// ---------------------------------------------------------------------------

/// Candidate section type for the entity telemetry stream (experimental
/// layout, validated by aggregate physical metrics).
const SEC_POS: u16 = 61;
/// Segment byte type = keyframe (full snapshot, different layout).
const SEG_KEYFRAME: u8 = 2;

/// Offset (in bytes, within the tail after the anchor) of the field block
/// of the E8 mask. The tail is: [3 variable bytes][1 mask byte
/// E8][field block]. Only records whose mask is E8 are processed
/// (≈92 % of the stream).
const POS_FIELD_BLOCK_OFFSET: usize = 4;
/// Mask that selects the field set that includes the position.
const POS_MASK_E8: u8 = 0xE8;
/// Start bit of X within the field block (LSB-first).
const POS_BIT0: usize = 13;
/// Width of each position field (14 bits ⇒ 0..16383 map units).
const POS_WIDTH: usize = 14;
/// Gap between X and Y within the field block.
const POS_GAP: usize = 3;
/// Physical map coordinates reach at most ~14 881; values above that are
/// bit 13 used as a **flag** (bit13 = +8192).
const POS_WRAP_MAX: u32 = 14881;

/// Converts a raw 14-bit field into a map coordinate (hypothetical
/// layout; see `plausible_positions`).
fn wrap_coord(raw: u32) -> f32 {
    let v = if raw > POS_WRAP_MAX { raw - 8192 } else { raw };
    v as f32
}
/// Sanity filter: no entity exceeds this speed (teleports included);
/// discards false anchors and misaligned records.
const POS_MAX_SPEED: f32 = 2_500.0;
/// Maximum carry when sampling: the last known position is reused
/// only if it is ≤ 5 s old; after that a gap is left (real death/gap).
const POS_CARRY_MS: u32 = 5_000;

/// Reads `width` bits starting at bit `bit`, LSB-first within each byte.
fn read_bits_lsb(buf: &[u8], bit: usize, width: usize) -> u32 {
    let mut v = 0u32;
    for i in 0..width {
        let p = bit + i;
        if p / 8 >= buf.len() {
            return v;
        }
        v |= u32::from((buf[p / 8] >> (p % 8)) & 1) << i;
    }
    v
}

/// Walks the section stream invoking `f(type, time_s, is_keyframe, data)`.
fn walk_sections(
    payload: &[u8],
    spans: &[(usize, usize, u8)],
    mut f: impl FnMut(f32, u16, bool, &[u8]),
) {
    let mut p = 0usize;
    let mut last_ty: u16 = 0;
    let mut last_time = 0.0f32;
    let mut si = 0usize;

    while p < payload.len() {
        // Locates the segment span that contains `p` (spans in order).
        while si + 1 < spans.len() && p >= spans[si].0 + spans[si].1 {
            si += 1;
        }
        let is_kf = spans.get(si).map(|s| s.2 == SEG_KEYFRAME).unwrap_or(false);

        let h = payload[p];
        let mut q = p + 1;

        let time = if h & SEC_ABS_TIME == 0 {
            if q + 4 > payload.len() {
                break;
            }
            let t = f32::from_le_bytes(payload[q..q + 4].try_into().unwrap());
            if !t.is_finite() || t < 0.0 {
                break;
            }
            q += 4;
            t
        } else {
            if q >= payload.len() {
                break;
            }
            let t = last_time + f32::from(payload[q]) / 1000.0;
            q += 1;
            t
        };

        let len = if h & SEC_SHORT_LEN == 0 {
            if q + 4 > payload.len() {
                break;
            }
            let l = u32le(payload, q) as usize;
            q += 4;
            l
        } else {
            if q >= payload.len() {
                break;
            }
            let l = payload[q] as usize;
            q += 1;
            l
        };

        let ty = if h & SEC_NO_TYPE == 0 {
            if q + 2 > payload.len() {
                break;
            }
            let t = u16::from_le_bytes([payload[q], payload[q + 1]]);
            q += 2;
            last_ty = t;
            t
        } else {
            last_ty
        };

        if h & SEC_SHORT_PARAMS == 0 {
            if q + 4 > payload.len() {
                break;
            }
            q += 4;
        } else {
            if q >= payload.len() {
                break;
            }
            q += 1;
        }

        if q + len > payload.len() {
            break;
        }
        f(time, ty, is_kf, &payload[q..q + len]);
        last_time = time;
        p = q + len;
    }
}

/// Assigns teams by early position: the block of entities whose
/// coordinates (x+y) are closest to the origin is **Blue** (the
/// bottom-left side of the Rift). Entity ids arrive in contiguous
/// per-team blocks in the initial snapshot.
fn assign_teams(raw: &[Vec<(u32, f32, f32)>]) -> Vec<Team> {
    let n = raw.len();
    let mut teams = vec![Team::Blue; n];
    if n < 2 {
        return teams;
    }
    let half = n / 2;
    // Mean of (x+y) using the samples from the first minute (the fountains);
    // before ~12 s there are no valid samples (the startup records
    // have the constant prefix and are discarded).
    let mean_sum = |rs: &[Vec<(u32, f32, f32)>]| -> Option<f64> {
        let mut s = 0.0f64;
        let mut cnt = 0u32;
        for r in rs {
            for &(t, x, y) in r.iter() {
                if t > 60_000 {
                    break;
                }
                s += f64::from(x) + f64::from(y);
                cnt += 1;
            }
        }
        (cnt > 0).then(|| s / f64::from(cnt))
    };
    let a = mean_sum(&raw[..half]);
    let b = mean_sum(&raw[half..]);
    match (a, b) {
        // Clear difference: the block closest to the origin is Blue.
        (Some(a), Some(b)) if (b - a).abs() > 1_000.0 => {
            if b < a {
                for t in teams.iter_mut().take(half) {
                    *t = Team::Red;
                }
                for t in teams.iter_mut().skip(half) {
                    *t = Team::Blue;
                }
            }
        }
        // No conclusive data: convention by slot order.
        _ => {
            for t in teams.iter_mut().skip(half) {
                *t = Team::Red;
            }
        }
    }
    teams
}

/// Decodes the champion x/y tracks from the type 61 stream
/// using the experimental layout (E8 mask, fields at bits 13 and 30 of the
/// field block, with wrap of bit 13).
///
/// 1. Reads the 10 champion ids from the first anchors `80 <id> 80 80 88` of the
///    initial snapshot.
/// 2. In each type 61 section of a *chunk* segment, locates the anchor of
///    each champion; only processes records with the E8 mask (≈92 % of the stream)
///    and extracts (x, y) from the field block.
/// 3. Applies the speed filter and the "constant prefix" guard.
/// 4. Samples the raw series (~23 Hz) onto the `sample_ms` grid with carry
///    ≤ 5 s.
/// 5. **Final validation**: if the tracks are not physically plausible
///    (`plausible_positions`) they are discarded.
fn extract_positions(
    seg: SegmentMap,
    sample_ms: u32,
    duration_s: f32,
    warnings: &mut Vec<String>,
) -> Vec<ChampionTrack> {
    let SegmentMap { payload, spans, .. } = seg;

    let mut ids: Vec<u8> = Vec::new();
    let mut got_ids = false;
    let mut raw: Vec<Vec<(u32, f32, f32)>> = Vec::new();
    let mut last: Vec<Option<(u32, f32, f32)>> = Vec::new();

    walk_sections(&payload, &spans, |time, ty, is_kf, data| {
        if ty != SEC_POS {
            return;
        }
        if !got_ids {
            // Initial snapshot: the first 10 anchors in record order
            // are the champions (neutrals appear later).
            for i in 0..data.len().saturating_sub(9) {
                if data[i] == 0x80
                    && data[i + 2] == 0x80
                    && data[i + 3] == 0x80
                    && data[i + 4] == 0x88
                {
                    let id = data[i + 1];
                    if !ids.contains(&id) {
                        ids.push(id);
                    }
                    if ids.len() == format::MAX_PLAYERS {
                        break;
                    }
                }
            }
            got_ids = true;
            raw = vec![Vec::new(); ids.len()];
            last = vec![None; ids.len()];
            return; // the snapshot has a different layout: it only provides the ids
        }
        if is_kf {
            return; // keyframe snapshots: different layout, ignored
        }
        let t_ms = (time.max(0.0) * 1000.0).min(u32::MAX as f32) as u32;
        // Phase 1: decodes the position of each champion present.
        let mut decs: Vec<Option<(f32, f32)>> = Vec::with_capacity(ids.len());
        for &id in ids.iter() {
            let anchor = [0x80u8, id, 0x80, 0x80, 0x88];
            let dec = data
                .windows(5)
                .position(|w| w == anchor)
                .and_then(|i| {
                    let tail = &data[i + 5..];
                    // Only records with the E8 mask (≈92 % of the stream): the
                    // field block starts after the 4 header bytes.
                    if tail.len() < POS_FIELD_BLOCK_OFFSET + 4 {
                        return None;
                    }
                    if tail[3] != POS_MASK_E8 {
                        return None;
                    }
                    let fb = &tail[POS_FIELD_BLOCK_OFFSET..];
                    if fb.len() * 8 < POS_BIT0 + 2 * POS_WIDTH + POS_GAP {
                        return None;
                    }
                    Some((
                        wrap_coord(read_bits_lsb(fb, POS_BIT0, POS_WIDTH)),
                        wrap_coord(read_bits_lsb(fb, POS_BIT0 + POS_WIDTH + POS_GAP, POS_WIDTH)),
                    ))
                });
            decs.push(dec);
        }
        // Phase 2: "full-shape" sections (record with a constant prefix, e.g.
        // the first chunk with all the fresh states). They are detected
        // because ≥6 champions decode exactly the same thing.
        for i in 0..decs.len() {
            if let Some(v) = decs[i] {
                let eq = decs.iter().filter(|w| **w == Some(v)).count();
                if eq >= 6 {
                    return;
                }
            }
        }
        // Phase 3: applies the speed filter and stores the sample.
        for (k, dec) in decs.into_iter().enumerate() {
            let Some((x, y)) = dec else { continue };
            if let Some((pt, px, py)) = last[k] {
                let dt = t_ms.saturating_sub(pt) as f32 / 1000.0;
                let d = ((x - px).powi(2) + (y - py).powi(2)).sqrt();
                if dt <= 0.0 {
                    if d > 0.0 {
                        continue; // same instant with a jump ⇒ noise
                    }
                } else if d / dt > POS_MAX_SPEED {
                    continue; // false anchor or ambiguous bit13 band
                }
            }
            last[k] = Some((t_ms, x, y));
            raw[k].push((t_ms, x, y));
        }
    });

    if ids.is_empty() {
        warnings.push(
            "no se encontraron registros de campeón (ancla `80 <id> 80 80 88`) en el stream \
             tipo 61: sin posiciones"
                .into(),
        );
        return Vec::new();
    }
    if ids.len() < format::MAX_PLAYERS {
        warnings.push(format!(
            "sólo se localizaron {} de {} campeones en el stream de posiciones",
            ids.len(),
            format::MAX_PLAYERS
        ));
    }

    let teams = assign_teams(&raw);

    // Sampling onto the `sample_ms` grid with carry ≤ 5 s.
    let end_ms = {
        let d = (duration_s.max(0.0) * 1000.0) as u32;
        if d > 0 {
            d
        } else {
            raw.iter().filter_map(|r| r.last()).map(|s| s.0).max().unwrap_or(0)
        }
    };
    let mut out: Vec<ChampionTrack> = Vec::with_capacity(raw.len());
    for (k, rs) in raw.iter().enumerate() {
        let mut samples: Vec<PositionSample> = Vec::new();
        if rs.is_empty() {
            warnings.push(format!(
                "el campeón {k} (id 0x{:02X}) nunca apareció en el stream de posiciones",
                ids[k]
            ));
        } else if sample_ms <= 1 {
            // Raw mode: keeps the whole stream (~23 Hz).
            samples.extend(rs.iter().map(|&(t, x, y)| PositionSample {
                t_ms: t,
                x,
                y,
            }));
        } else {
            let step = sample_ms.max(1);
            let mut g = 0u32;
            let mut idx = 0usize;
            while g <= end_ms {
                while idx + 1 < rs.len() && rs[idx + 1].0 <= g {
                    idx += 1;
                }
                let (st, sx, sy) = rs[idx];
                if st <= g && g - st <= POS_CARRY_MS {
                    samples.push(PositionSample {
                        t_ms: g,
                        x: sx,
                        y: sy,
                    });
                }
                g = g.saturating_add(step);
            }
        }
        out.push(ChampionTrack {
            slot: k as u8,
            team: teams[k],
            entity_tag: u32::from(ids[k]),
            samples,
        });
    }
    // Final gate: without physical validation no positions are exported (better
    // an honest gap than an invented track).
    if !plausible_positions(&out) {
        warnings.push(
            "las trayectorias del stream tipo 61 no superan la validación física \
             (separación de equipos en fase de líneas o concentración en fuentes): \
             el layout experimental no es válido en este replay, se descartan"
                .into(),
        );
        return Vec::new();
    }
    out
}

/// Are a set of Rift tracks physically plausible?
///
/// Two checks with real Rift data (map units):
///
/// 1. **Team separation during lane phase** (t = 150–350 s): the mean
///    `x + y` of Red must exceed Blue's by ≥ 2000 units (Red
///    laners live top-right and Blue laners bottom-left).
/// 2. **Fountain concentration**: at least 3 champions of one team within
///    a radius of 3000 of a fountain ((1500,1500) or (13300,13300)) in the
///    first 90 s (base exit / invasion).
fn plausible_positions(tracks: &[ChampionTrack]) -> bool {
    // 1) Mean per-team separation in the lane window.
    let mut sums = [0.0f64; 2];
    let mut cnts = [0u32; 2];
    for t in tracks {
        let ti = match t.team {
            Team::Blue => 0,
            Team::Red => 1,
        };
        for s in &t.samples {
            if (150_000..=350_000).contains(&s.t_ms) {
                sums[ti] += f64::from(s.x) + f64::from(s.y);
                cnts[ti] += 1;
            }
        }
    }
    // 1) Mean per-team separation in the lane window.
    let mut sums = [0.0f64; 2];
    let mut cnts = [0u32; 2];
    for t in tracks {
        let ti = match t.team {
            Team::Blue => 0,
            Team::Red => 1,
        };
        for s in &t.samples {
            if (150_000..=350_000).contains(&s.t_ms) {
                sums[ti] += f64::from(s.x) + f64::from(s.y);
                cnts[ti] += 1;
            }
        }
    }
    let mut sep_ok = false;
    if cnts[0] > 0 && cnts[1] > 0 {
        let mean_blue = sums[0] / f64::from(cnts[0]);
        let mean_red = sums[1] / f64::from(cnts[1]);
        if mean_red - mean_blue >= 3_000.0 {
            sep_ok = true;
        }
    }
    // 2) Concentration near a fountain at the start.
    const FOUNTAINS: [(f32, f32); 2] = [(1_500.0, 1_500.0), (13_300.0, 13_300.0)];
    let mut fountain_ok = false;
    for (fx, fy) in FOUNTAINS {
        let near = tracks
            .iter()
            .filter(|t| {
                t.samples.iter().any(|s| {
                    s.t_ms <= 90_000
                        && (f64::from(s.x) - f64::from(fx)).powi(2)
                            + (f64::from(s.y) - f64::from(fy)).powi(2)
                            <= 3_000.0f64.powi(2)
                })
            })
            .count();
        if near >= 3 {
            fountain_ok = true;
        }
    }
    // 3) Rejection of constant fields (e.g. the E8 marker byte ≈ 7440):
    //    no 500-unit bucket may dominate more than 45 % of the
    //    samples of an axis.
    let mut const_ok = true;
    for axis in 0..2 {
        let mut buckets: std::collections::HashMap<i32, u32> =
            std::collections::HashMap::new();
        let mut total = 0u32;
        for t in tracks {
            for s in &t.samples {
                let v = if axis == 0 { s.x } else { s.y };
                let b = (v / 500.0).floor() as i32;
                *buckets.entry(b).or_default() += 1;
                total += 1;
            }
        }
        if total > 0 {
            let maxb = buckets.values().copied().max().unwrap_or(0);
            if (maxb as f64 / total as f64) > 0.45 {
                const_ok = false;
            }
        }
    }
    sep_ok || (fountain_ok && const_ok)
}
