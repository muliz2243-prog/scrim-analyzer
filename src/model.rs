//! Public data structures for the analysis.
//!
//! All of them are `Serialize`/`Deserialize` (serde) and are designed to be
//! lightweight: enums without redundant data, fixed arrays of 10 participants
//! per frame (zero heap allocations per tick) and `String` only where the data
//! is textual.

use serde::{Deserialize, Serialize};

/// Side of the map.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Team {
    Blue,
    Red,
}

impl Team {
    pub fn as_str(&self) -> &'static str {
        match self {
            Team::Blue => "Azul",
            Team::Red => "Rojo",
        }
    }
}

/// Complete result of the analysis of a `.rofl` file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Replay {
    /// Game metadata (patch version, mode, map, duration...).
    pub metadata: ReplayMetadata,
    /// Footer information (end of game), if present.
    pub footer: Option<GameEndInfo>,
    /// The 10 players. The index (`slot`) matches the order of the
    /// participants inside the telemetry frames.
    pub players: Vec<Player>,
    /// Sampled telemetry: position (x, y), health, mana and level per tick.
    pub frames: Vec<FrameSnapshot>,
    /// Vision events extracted from the payload (wards) and, in the modern
    /// format, the verified economy events (kills, assists, plates and
    /// turrets) with their exact timestamp.
    pub events: Vec<GameEvent>,
    /// Validation of the modern-format events: detected counts cross-checked
    /// against the `statsJson` totals. Informational only; the events are
    /// already verified before being exported.
    #[serde(default)]
    pub events_report: Option<EventsReport>,
    /// Per-team aggregates (gold, CS, kills, wards...).
    pub team_totals: TeamTotals,
    /// x/y position tracks per champion (modern format: section type 61,
    /// **experimental**). Layout validated by aggregate physical metrics
    /// (team separation, concentration at spawns, variance, continuity);
    /// they are only emitted if the `plausible_positions` gate passes.
    /// Sampled at `AnalyzeOptions::sample_ms` (1 Hz by default).
    #[serde(default)]
    pub positions: Vec<ChampionTrack>,
    /// Fixed 30 s blocks with per-team gold delta + Top 5 successes and
    /// disasters per team. `fuente` indicates where the gold series came
    /// from (`"telemetria"` or `"no_disponible"`).
    #[serde(default)]
    pub gold_blocks: Option<GoldBlocksReport>,
    /// Payload section summary (modern format only): histogram of section
    /// types with times and sizes. Useful for reverse engineering patches.
    #[serde(default)]
    pub sections: Option<SectionSummary>,
    /// Number of decompressed payload segments/chunks.
    #[serde(default)]
    pub segment_count: u32,
    /// Non-fatal warnings (unsupported patches, missing data...).
    pub warnings: Vec<String>,
}

/// Summary of the payload section stream (modern format).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SectionSummary {
    /// Total number of parsed sections.
    pub total_sections: u64,
    /// Last time seen in the stream (≈ match duration).
    pub duration_s: f32,
    /// Histogram by section type (sorted by bytes, descending).
    pub types: Vec<SectionTypeInfo>,
}

/// Statistics for one specific section type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SectionTypeInfo {
    /// Section ID (u16 in the format).
    pub type_id: u32,
    pub count: u64,
    pub total_bytes: u64,
    pub min_len: u32,
    pub max_len: u32,
    pub first_time_s: f32,
    pub last_time_s: f32,
}

/// Main replay metadata (header JSON).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayMetadata {
    /// Detected container variant: `"legacy"` (zlib + header JSON)
    /// or `"modern"` (zstd + footer).
    #[serde(default)]
    pub source_format: String,
    pub game_id: String,
    pub game_version: String,
    pub game_mode: String,
    pub game_type: String,
    pub map_id: u32,
    pub queue_id: u32,
    /// Duration declared in the header, in seconds.
    pub game_length_s: f32,
    /// Unix timestamp of the file creation.
    pub created_unix: u64,
}

/// End-of-game data (footer JSON).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GameEndInfo {
    pub game_length_s: f32,
    /// Unix timestamp of the end of the game (from approx. 2021).
    pub game_end_timestamp_unix: u64,
}

/// A player and their final statistics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Player {
    /// Participant index inside the frames (0..=9).
    pub slot: u8,
    pub summoner_name: String,
    pub champion_id: u32,
    /// Champion name (modern format: `SKIN` key; e.g. `"Gwen"`).
    #[serde(default)]
    pub champion_name: Option<String>,
    pub team: Team,
    pub summoner_spell_1: u32,
    pub summoner_spell_2: u32,
    pub skin_id: u32,
    /// Player result (`true` = victory), if the format exposes it.
    #[serde(default)]
    pub won: Option<bool>,
    pub stats: PlayerStats,
}

/// Final statistics per player (from the metadata JSON).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PlayerStats {
    pub champ_level: u32,
    pub kills: u32,
    pub deaths: u32,
    pub assists: u32,
    /// Accumulated gold earned.
    pub gold_earned: u32,
    pub gold_spent: u32,
    /// Total CS (lane minions + neutral monsters).
    pub cs: u32,
    pub cs_lane: u32,
    pub cs_jungle: u32,
    pub damage_to_champions: u64,
    pub damage_taken: u64,
    pub wards_placed: u32,
    pub wards_killed: u32,
    /// Turret plates destroyed by the player (`Missions_TurretPlatesDestroyed`).
    #[serde(default)]
    pub turret_plates_destroyed: u32,
    /// Turrets destroyed by the player (`TURRETS_KILLED`).
    #[serde(default)]
    pub turrets_destroyed: u32,
    /// Dragons killed by the player (`DRAGON_KILLS`).
    ///
    /// It is one of the few pieces of data the live API does **not**
    /// expose: the `.rofl` `statsJson` does have it (per player), and summing
    /// it per team gives the total dragons for each side.
    #[serde(default)]
    pub dragons_killed: u32,
    /// Barons killed by the player (`BARON_KILLS`).
    #[serde(default)]
    pub barons_killed: u32,
    /// Heralds killed by the player (`RIFT_HERALD_KILLS`).
    #[serde(default)]
    pub heralds_killed: u32,
    /// Instant, in seconds, of the last takedown the player took part in
    /// (kill or assist). Used to validate over time the events detected in
    /// the modern-format stream.
    #[serde(default)]
    pub last_takedown_s: f32,
    /// Final items (7 slots: 6 + bench).
    pub items: [u32; 7],
}

/// Aggregates for one team.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TeamAggregate {
    pub players: u32,
    pub gold_earned: u64,
    pub gold_spent: u64,
    pub cs: u64,
    pub kills: u32,
    pub deaths: u32,
    pub assists: u32,
    pub wards_placed: u32,
    pub wards_killed: u32,
    pub damage_to_champions: u64,
    /// Epic objectives taken by the team (sum over the players).
    ///
    /// The live API does not expose them; they come from the `.rofl`
    /// (`DRAGON_KILLS`, `BARON_KILLS`, `RIFT_HERALD_KILLS`).
    pub dragons_killed: u32,
    pub barons_killed: u32,
    pub heralds_killed: u32,
}

/// Totals per map side.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TeamTotals {
    pub blue: TeamAggregate,
    pub red: TeamAggregate,
}

/// State of a champion at a specific tick.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct ParticipantState {
    /// Map X coordinate (game units).
    pub x: f32,
    /// Map Y coordinate (game units).
    pub y: f32,
    pub health: u16,
    pub max_health: u16,
    pub mana: u16,
    pub max_mana: u16,
    pub level: u8,
    pub alive: bool,
}

/// One telemetry tick: the 10 champions at once.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct FrameSnapshot {
    /// Milliseconds since the start of the game.
    pub timestamp_ms: u32,
    /// State per participant, indexed the same as `Replay::players`.
    pub participants: [ParticipantState; crate::format::MAX_PLAYERS],
}

/// Approximate extent of the Summoner's Rift map (game units). The decoded
/// coordinates of the modern format fit inside a square of ~14,815 × 14,815
/// units; use it to map (x, y) to pixels in the GUI:
/// `px = x / MAP_EXTENT * ancho_mapa`.
pub const MAP_EXTENT: f32 = 14815.0;

/// One position sample taken from a champion.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct PositionSample {
    /// Milliseconds since the start of the game (sampling grid).
    pub t_ms: u32,
    /// X coordinate in map units (0..~14,815).
    pub x: f32,
    /// Y coordinate in map units (0..~14,815).
    pub y: f32,
}

/// Decoded track of a champion (section type 61 of the modern format;
/// **experimental**, only emitted if it passes the physical validation).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChampionTrack {
    /// Index of the entity inside the stream (order of appearance in the
    /// initial snapshot; network ids increase in blocks of 0x20).
    pub slot: u8,
    /// Team detected by position (the block of ids with coordinates closest
    /// to the origin is Blue).
    pub team: Team,
    /// Identity byte observed at the record anchor (not the full netid;
    /// used for RE traceability).
    pub entity_tag: u32,
    /// Position samples at `sample_ms` (maximum carry-over 5 s; if the
    /// champion does not appear again, the series ends).
    pub samples: Vec<PositionSample>,
}

/// Gold delta of both teams in a fixed time block.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct GoldBlock {
    /// Block index (0 = first 30 s).
    pub index: u32,
    pub start_s: f32,
    pub end_s: f32,
    /// Gold earned by Blue in the block.
    pub gold_blue_delta: i64,
    /// Gold earned by Red in the block.
    pub gold_red_delta: i64,
    /// Net advantage for Blue in the block (`blue - red`).
    pub net_blue: i64,
}

/// Entry of a team's Top 5 (success or disaster) in a block.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct TopBlock {
    pub block_index: u32,
    pub start_s: f32,
    pub end_s: f32,
    /// Gold earned by the team in the block.
    pub delta_own: i64,
    /// Gold earned by the opponent in the block.
    pub delta_enemy: i64,
    /// Net advantage for the team (`delta_own - delta_enemy`).
    pub net: i64,
}

/// Top 5 of 30 s blocks for one team.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TeamTop5 {
    /// 5 blocks with the largest net advantage in favor (green).
    pub exitos: Vec<TopBlock>,
    /// 5 blocks with the largest net loss against (red).
    pub desastres: Vec<TopBlock>,
}

/// Report of 30 s blocks with gold delta and Top 5 per team.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoldBlocksReport {
    /// Source of the gold series: `"telemetria"` (decoded from the payload)
    /// or `"no_disponible"` (empty blocks/top5; the engine is ready and
    /// covered by tests, the section that carries the gold still needs to be
    /// decoded).
    pub fuente: String,
    /// Length of each block, in seconds (30).
    pub block_secs: u32,
    /// All the contiguous blocks of the match, in order.
    pub blocks: Vec<GoldBlock>,
    pub top5_azul: TeamTop5,
    pub top5_rojo: TeamTop5,
}

/// Ward type, normalized.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WardType {
    /// Standard vision ward (yellow trinket / yellow ward).
    Stealth,
    /// Sentinel ward (pink / control).
    Control,
    /// Blue trinket.
    Blue,
    /// Zombie ward.
    Zombie,
    /// Any unrecognized type, kept as raw text.
    Other(String),
}

impl WardType {
    /// Normalizes the ward names seen in replays.
    pub fn from_raw(raw: &str) -> Self {
        let norm = raw.trim().to_ascii_lowercase();
        match norm.as_str() {
            "yellowtrinket" | "stealth" | "stealthward" | "yellow" | "ward" | "yellowtrinketnormal" => {
                WardType::Stealth
            }
            "pinktrinket" | "jammerflag" | "control" | "pink" | "centinel" | "controlward" => {
                WardType::Control
            }
            "bluetrinket" | "blue" => WardType::Blue,
            "zombie" | "zombieward" => WardType::Zombie,
            _ => WardType::Other(raw.to_string()),
        }
    }
}

/// An event from the payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GameEvent {
    /// Milliseconds since the start of the game.
    pub timestamp_ms: u32,
    pub kind: EventKind,
}

/// Event classification.
///
/// The economy events (`champion_kill`, `assist`, `turret_plate_destroyed`,
/// `turret_destroyed`) are detected in the modern-format event stream and are
/// **verified against the `statsJson`** (see [`EventsReport`]). They only
/// carry a timestamp: the attribution to player/team **is not encoded in a
/// decodable way** in these records, and faking it would be fabricating data.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventKind {
    WardPlaced {
        ward_type: WardType,
        x: f32,
        y: f32,
    },
    WardKill {
        ward_type: WardType,
        x: f32,
        y: f32,
    },
    /// Kill of a champion. Verified: the count matches
    /// `CHAMPIONS_KILLED` from the `statsJson`.
    ChampionKill,
    /// Assist. Verified against `ASSISTS`.
    Assist,
    /// Turret plate destroyed. Verified against `Missions_TurretPlatesDestroyed`.
    TurretPlateDestroyed,
    /// Turret destroyed. Verified against `TURRETS_KILLED`.
    TurretDestroyed,
}

/// Validation report of the events detected in the modern format.
///
/// The counts of each event type are cross-checked against the definitive
/// totals from the `statsJson` (end of game, 10 players). An event is only
/// accepted if **both counts match exactly**.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EventsReport {
    /// `"stream_de_eventos"` if the events are verified;
    /// `"no_disponible"` otherwise (`events` empty).
    pub fuente: String,
    /// Count detected per event type.
    pub kills: u32,
    pub assists: u32,
    pub turret_plates: u32,
    pub turrets_destroyed: u32,
    /// Equivalent totals read from the `statsJson` (ground truth).
    pub truth_kills: u32,
    pub truth_assists: u32,
    pub truth_turret_plates: u32,
    pub truth_turrets_destroyed: u32,
    /// `true` if the four detected counts match the `statsJson` exactly.
    pub verificado: bool,
    /// Why the per-team gold cannot be reconstructed from these events
    /// (team attribution not decodable).
    pub atribucion_equipo: String,
}

/// Adds up the totals of the 10 players per team.
pub fn compute_team_totals(players: &[Player]) -> TeamTotals {
    let mut totals = TeamTotals::default();
    for p in players {
        let agg = match p.team {
            Team::Blue => &mut totals.blue,
            Team::Red => &mut totals.red,
        };
        agg.players += 1;
        agg.gold_earned += u64::from(p.stats.gold_earned);
        agg.gold_spent += u64::from(p.stats.gold_spent);
        agg.cs += u64::from(p.stats.cs);
        agg.kills += p.stats.kills;
        agg.deaths += p.stats.deaths;
        agg.assists += p.stats.assists;
        agg.wards_placed += p.stats.wards_placed;
        agg.wards_killed += p.stats.wards_killed;
        agg.damage_to_champions += p.stats.damage_to_champions;
        agg.dragons_killed += p.stats.dragons_killed;
        agg.barons_killed += p.stats.barons_killed;
        agg.heralds_killed += p.stats.heralds_killed;
    }
    totals
}
