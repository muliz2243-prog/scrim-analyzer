//! # rofl-analyzer
//!
//! High-performance offline binary parser for `.rofl` replays from
//! League of Legends (`RIOT` format).
//!
//! ## What it exposes
//!
//! * [`analyze_file`] / [`analyze_bytes`] — full analysis.
//! * [`extract_payload`] — dumps the decompressed payload (for debugging).
//! * [`Replay`] — result serializable to JSON with serde.
//!
//! ## Example
//!
//! ```no_run
//! use rofl_analyzer::{analyze_file, AnalyzeOptions};
//!
//! let replay = analyze_file("partida.rofl", AnalyzeOptions::default()).unwrap();
//! println!("{} | {} | {} jugadores", replay.metadata.game_version,
//!          replay.metadata.game_mode, replay.players.len());
//! for ev in &replay.events {
//!     println!("{:?} @ {} ms", ev.kind, ev.timestamp_ms);
//! }
//! ```

pub mod blocks;
pub mod error;
pub mod gold_breakdown;
pub mod i18n;
pub mod lcu;
pub mod live;
pub mod model;
pub mod modern;
pub mod score;
pub mod tls;

mod chunks;
mod cursor;
mod events_extract;
mod format;
mod header;
mod metadata;
pub mod payload;

pub use blocks::{compute_gold_blocks, GoldSeries, BLOCK_SECS as GOLD_BLOCK_SECS};
pub use error::RoflError;
pub use lcu::{
    find_active_game_id, find_active_session, find_latest_game_id_any_age, find_latest_rofl_game_id,
    find_latest_rofl_game_id_since,
    game_id_from_r3dlog, game_id_from_rofl_path, match_summary_from_game, LcuClient,
    LcuCredentials, LcuFrame, LcuPosition, LcuTimeline, LcuTimelineEvent, MatchSummary,
    MatchVision,
};
pub use gold_breakdown::{
    blocks_with_axes, events_from_timeline, gold_blocks_with_axes, EventGold, GoldCategory,
};
pub use i18n::{Lang, Translator};
pub use live::{
    dedup_saved_events, rebuild_event_counters, AttributedEvent, GoldModelParams, LivePoller,
    LiveSnapshot, PlayerSample, ScrimBlock, ScrimCapture, VisionTotals, BLOCK_SECS as LIVE_BLOCK_SECS,
    LIVE_ENDPOINT, SAVED_EVENT_EPSILON_S,
};
pub use model::{
    ChampionTrack, EventKind, FrameSnapshot, GameEndInfo, GameEvent, GoldBlock, GoldBlocksReport,
    ParticipantState, Player, PlayerStats, PositionSample, Replay, ReplayMetadata, SectionSummary,
    SectionTypeInfo, Team, TeamAggregate, TeamTotals, TeamTop5, TopBlock, WardType, MAP_EXTENT,
};
pub use score::{
    BreakAlert, DominionBlock, DominionReport, ObjectiveEvent, ScoreParams, ScoreTop5, Severity,
    SeverityThresholds, VisionAlert, VisionReport, VisionThresholds,
};

use std::path::Path;

/// Analysis options.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnalyzeOptions {
    /// Frame sampling: keeps 1 frame every N milliseconds.
    /// `1` keeps every frame; `1000` (the default) keeps 1 per second.
    pub sample_ms: u32,
}

impl Default for AnalyzeOptions {
    fn default() -> Self {
        Self { sample_ms: 1000 }
    }
}

/// Analyzes a `.rofl` from disk.
pub fn analyze_file(
    path: impl AsRef<Path>,
    opts: AnalyzeOptions,
) -> Result<Replay, RoflError> {
    let path = path.as_ref();
    if !path.is_file() {
        return Err(RoflError::NotAFile(path.display().to_string()));
    }
    let bytes = std::fs::read(path)?;
    analyze_bytes(&bytes, opts)
}

/// Analyzes a `.rofl` already loaded in memory (pure function over the buffer).
///
/// Automatically detects the container variant:
///
/// * **legacy** — header JSON + chunks compressed with zlib (telemetry frames
///   with x/y, hp, level and ward events);
/// * **modern** — Zstandard segments + JSON footer with `statsJson` (e.g.
///   patch 16.x and later). Extracts metadata, players, gold, CS, KDA, wards,
///   duration and the champions' **x/y trajectories** (type 61 stream,
///   experimental layout validated by aggregate physical metrics: team
///   separation, concentration at fountains, variance and continuity). The
///   [`GoldBlocksReport`] report is always emitted; today its source is
///   `"no_disponible"` (the section carrying the time-based gold series still
///   needs to be decoded).
pub fn analyze_bytes(bytes: &[u8], opts: AnalyzeOptions) -> Result<Replay, RoflError> {
    if modern::looks_modern(bytes) {
        let mut warnings = Vec::new();
        return modern::analyze(bytes, opts, &mut warnings);
    }

    let mut warnings: Vec<String> = Vec::new();

    // 1. Header: magic, footer_offset, metadata JSON, chunk count.
    let header = header::parse_header(bytes, &mut warnings)?;

    // 2. Chunks: zlib decompression of the payload.
    let raw_payload = chunks::decompress_payload(
        bytes,
        header.payload_start,
        header.footer_offset,
        header.chunk_count,
        &mut warnings,
    )?;

    // 3. Payload: telemetry frames + vision events.
    let data = payload::parse_payload(&raw_payload, opts.sample_ms, &mut warnings);

    // 4. Footer: end of the match (failure-tolerant).
    let footer = header::parse_footer(bytes, header.footer_offset, &mut warnings)?;

    // 5. Duration consistency: the footer wins if present.
    let mut metadata = header.metadata;
    if let Some(f) = &footer {
        if f.game_length_s > 0.0 {
            metadata.game_length_s = f.game_length_s;
        }
    }

    let players = header.players;
    let team_totals = metadata::totals_for(&players);
    let segment_count = header.chunk_count;

    Ok(Replay {
        metadata,
        footer,
        players,
        frames: data.frames,
        events: data.events,
        // The legacy path has no economic-events stream verifiable
        // against statsJson: we do not emit the report.
        events_report: None,
        team_totals,
        positions: Vec::new(),
        // The legacy variant does not yet expose a time-based gold series in
        // frames (only final totals in the footer): we do not emit the report.
        gold_blocks: None,
        sections: None,
        segment_count,
        warnings,
    })
}

/// Serializes the result to JSON (pretty, UTF-8).
pub fn to_json(replay: &Replay) -> Result<String, RoflError> {
    Ok(serde_json::to_string_pretty(replay)?)
}

/// Serializes the result to compact JSON (minimum size).
pub fn to_json_compact(replay: &Replay) -> Result<String, RoflError> {
    Ok(serde_json::to_string(replay)?)
}

/// Deserializes a `Replay` from JSON (to resume analysis without re-parsing).
pub fn from_json(json: &str) -> Result<Replay, RoflError> {
    Ok(serde_json::from_str(json)?)
}

/// Extracts the decompressed payload from an already-loaded buffer.
pub fn extract_payload_bytes(bytes: &[u8]) -> Result<Vec<u8>, RoflError> {
    if modern::looks_modern(bytes) {
        return modern::extract_payload(bytes);
    }
    let mut warnings = Vec::new();
    let header = header::parse_header(bytes, &mut warnings)?;
    chunks::extract_payload(
        bytes,
        header.payload_start,
        header.footer_offset,
        header.chunk_count,
    )
}

/// Extracts the decompressed payload from a `.rofl` (debugging / patch RE).
pub fn extract_payload_file(path: impl AsRef<Path>) -> Result<Vec<u8>, RoflError> {
    let path = path.as_ref();
    if !path.is_file() {
        return Err(RoflError::NotAFile(path.display().to_string()));
    }
    let bytes = std::fs::read(path)?;
    extract_payload_bytes(&bytes)
}
