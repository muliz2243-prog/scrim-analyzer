//! Native connector for the League Client (LCU - League Client Update).
//!
//! Allows connecting directly to the running League of Legends client by
//! reading the local credentials from `lockfile` (port and random password).
//!
//! With this module you can fetch matches and the FULL TIMELINE in one go
//! (`lol-match-history/v1/game-timelines/{gameId}`), including:
//! - Gold curve per minute (`totalGold`, `currentGold`).
//! - (x, y) coordinates of the 10 champions and wards.
//! - Exact events for kills, towers, plates, dragons and barons.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use serde::{Deserialize, Serialize};

use crate::tls::ClienteLocal;

/// Credentials extracted from the `lockfile` of the LoL client.
#[derive(Debug, Clone)]
pub struct LcuCredentials {
    pub process_name: String,
    pub pid: u32,
    pub port: u16,
    pub password: String,
    pub protocol: String,
}

impl LcuCredentials {
    /// Tries to find and read the `lockfile` in the usual LoL locations.
    pub fn find_and_read() -> Result<Self, String> {
        let rutas_candidatas = [
            PathBuf::from(r"C:\Riot Games\League of Legends\lockfile"),
            PathBuf::from(r"D:\Riot Games\League of Legends\lockfile"),
            PathBuf::from(r"E:\Riot Games\League of Legends\lockfile"),
        ];

        for ruta in &rutas_candidatas {
            if ruta.is_file() {
                if let Ok(creds) = Self::from_file(ruta) {
                    return Ok(creds);
                }
            }
        }

        Err("League of Legends lockfile not found. Make sure the LoL client is running.".to_string())
    }

    /// Reads and parses a specific `lockfile` (format: `Process:PID:Port:Password:Protocol`).
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, String> {
        let contenido = fs::read_to_string(path).map_err(|e| format!("Error reading lockfile: {e}"))?;
        let partes: Vec<&str> = contenido.trim().split(':').collect();
        if partes.len() < 5 {
            return Err("Invalid lockfile format (expected 5 fields separated by ':')".to_string());
        }

        let pid = partes[1].parse::<u32>().map_err(|e| format!("Invalid PID: {e}"))?;
        let port = partes[2].parse::<u16>().map_err(|e| format!("Invalid port: {e}"))?;

        Ok(Self {
            process_name: partes[0].to_string(),
            pid,
            port,
            password: partes[3].to_string(),
            protocol: partes[4].to_string(),
        })
    }

    /// Builds the HTTP Basic authorization header (user: riot, pass: password).
    pub fn auth_header_value(&self) -> String {
        let raw = format!("riot:{}", self.password);
        let b64 = base64_encode(raw.as_bytes());
        format!("Basic {b64}")
    }

    /// Base URL for calls to the LCU.
    pub fn base_url(&self) -> String {
        format!("https://127.0.0.1:{}", self.port)
    }
}

fn base64_encode(data: &[u8]) -> String {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = String::new();
    let mut i = 0;
    while i < data.len() {
        let b0 = data[i] as usize;
        let b1 = if i + 1 < data.len() { data[i + 1] as usize } else { 0 };
        let b2 = if i + 2 < data.len() { data[i + 2] as usize } else { 0 };

        let triple = (b0 << 16) | (b1 << 8) | b2;

        result.push(TABLE[(triple >> 18) & 0x3F] as char);
        result.push(TABLE[(triple >> 12) & 0x3F] as char);

        if i + 1 < data.len() {
            result.push(TABLE[(triple >> 6) & 0x3F] as char);
        } else {
            result.push('=');
        }

        if i + 2 < data.len() {
            result.push(TABLE[triple & 0x3F] as char);
        } else {
            result.push('=');
        }

        i += 3;
    }
    result
}

/// Client for connecting to the League of Legends LCU.
pub struct LcuClient {
    creds: LcuCredentials,
    cliente: ClienteLocal,
}

impl LcuClient {
    /// Connects by detecting the lockfile locally.
    pub fn connect() -> Result<Self, String> {
        let creds = LcuCredentials::find_and_read()?;
        let cliente = ClienteLocal::new(10)?;
        Ok(Self { creds, cliente })
    }

    /// Connects with already-extracted credentials.
    pub fn with_credentials(creds: LcuCredentials) -> Result<Self, String> {
        let cliente = ClienteLocal::new(10)?;
        Ok(Self { creds, cliente })
    }

    /// Performs an authenticated GET to the LCU.
    pub fn get_json(&self, endpoint: &str) -> Result<String, String> {
        let path = if endpoint.starts_with('/') {
            endpoint.to_string()
        } else {
            format!("/{endpoint}")
        };

        let auth_url = format!(
            "https://riot:{}@127.0.0.1:{}{}",
            self.creds.password,
            self.creds.port,
            path
        );

        self.cliente.get(&auth_url)
    }

    /// Gets a match timeline by its ID (e.g. 1632440991 or taken from the .rofl).
    pub fn get_timeline(&self, game_id: u64) -> Result<LcuTimeline, String> {
        let json_str = self.get_json(&format!("/lol-match-history/v1/game-timelines/{}", game_id))?;
        serde_json::from_str::<LcuTimeline>(&json_str)
            .map_err(|e| format!("Error deserializing timeline: {e}"))
    }

    /// Gets the match summary by its ID.
    pub fn get_game(&self, game_id: u64) -> Result<serde_json::Value, String> {
        let json_str = self.get_json(&format!("/lol-match-history/v1/games/{}", game_id))?;
        serde_json::from_str::<serde_json::Value>(&json_str)
            .map_err(|e| format!("Error deserializing match data: {e}"))
    }

    /// Match summary with the **vision totals** per team.
    ///
    /// Riot's timeline does not always include `WARD_PLACED` events (verified
    /// in real matches: it only brings kills, towers and monsters),
    /// but `/games/{id}` does expose `stats.wardsPlaced` per participant. This
    /// summary is the fallback vision source.
    pub fn get_match_summary(&self, game_id: u64) -> Result<MatchSummary, String> {
        let game = self.get_game(game_id)?;
        match_summary_from_game(game_id, &game)
    }

    /// Gets the list of recent matches for the current summoner.
    pub fn get_recent_matches(&self) -> Result<serde_json::Value, String> {
        let json_str = self.get_json("/lol-match-history/v1/products/lol/current-summoner/matches")?;
        serde_json::from_str::<serde_json::Value>(&json_str)
            .map_err(|e| format!("Error deserializing matches: {e}"))
    }
}

// ---------------------------------------------------------------------------
// Match summary: vision totals and metadata
// ---------------------------------------------------------------------------

/// Wards placed by each team over the whole match (measured data).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MatchVision {
    pub blue: u64,
    pub red: u64,
}

/// Summary of `/lol-match-history/v1/games/{gameId}`.
#[derive(Debug, Clone, Default)]
pub struct MatchSummary {
    pub game_id: u64,
    pub duration_s: f32,
    pub game_mode: String,
    pub map_name: String,
    pub vision: MatchVision,
}

impl MatchVision {
    pub fn total(&self) -> u64 {
        self.blue + self.red
    }
}

/// Extracts the vision totals and metadata from the `/games/{id}` JSON.
///
/// Returns an error if the JSON has no `participants` (match not available yet
/// or unexpected response): better to say so than to show 0 wards.
pub fn match_summary_from_game(
    game_id: u64,
    game: &serde_json::Value,
) -> Result<MatchSummary, String> {
    let participants = game
        .get("participants")
        .and_then(serde_json::Value::as_array)
        .ok_or("match summary has no `participants`")?;
    let mut vision = MatchVision::default();
    for p in participants {
        let team = p
            .get("teamId")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        let placed = p
            .get("stats")
            .and_then(|s| s.get("wardsPlaced"))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        match team {
            100 => vision.blue += placed,
            200 => vision.red += placed,
            _ => {}
        }
    }
    let duration_s = game
        .get("gameDuration")
        .and_then(serde_json::Value::as_f64)
        .or_else(|| {
            game.get("gameLength")
                .and_then(serde_json::Value::as_f64)
        })
        .unwrap_or(0.0) as f32;
    Ok(MatchSummary {
        game_id,
        duration_s,
        game_mode: game
            .get("gameMode")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("?")
            .to_string(),
        map_name: game
            .get("gameMapName")
            .and_then(serde_json::Value::as_str)
            .or_else(|| game.get("mapName").and_then(serde_json::Value::as_str))
            .unwrap_or("?")
            .to_string(),
        vision,
    })
}

// ---------------------------------------------------------------------------
// Detecting the gameId of the active match
// ---------------------------------------------------------------------------
//
// The `.rofl` in `Documents/League of Legends/Replays/` is not a reliable source:
// its modification date only changes when the file is closed, and an old
// replay played back later becomes the "most recent" one. The client, on the
// other hand, always writes a `*_r3dlog.txt` in
// `Logs/GameLogs/<date-time>/` when the match **or the replay** starts, and the
// real `GameID` is there.

/// Candidate game log folders (typical installations).
pub fn game_log_dirs() -> Vec<PathBuf> {
    [
        PathBuf::from(r"C:\Riot Games\League of Legends\Logs\GameLogs"),
        PathBuf::from(r"D:\Riot Games\League of Legends\Logs\GameLogs"),
        PathBuf::from(r"E:\Riot Games\League of Legends\Logs\GameLogs"),
    ]
    .into_iter()
    .filter(|p| p.is_dir())
    .collect()
}

/// Extracts the `gameId` from a replay path (`…/LA2-1632883165.rofl`).
pub fn game_id_from_rofl_path(path: &str) -> Option<u64> {
    let name = path
        .rsplit(|c| c == '/' || c == '\\')
        .find(|s| !s.trim().is_empty())?
        .trim();
    let lower = name.to_ascii_lowercase();
    let stem = if lower.ends_with(".rofl") {
        &name[..name.len() - ".rofl".len()]
    } else {
        name
    };
    stem.rsplit('-').next()?.trim().parse::<u64>().ok()
}

/// Team (100/200) of a `participantId` 1..=10; `None` if not attributable.
pub fn team_from_participant(participant_id: i32) -> Option<i32> {
    if (1..=5).contains(&participant_id) {
        Some(100)
    } else if (6..=10).contains(&participant_id) {
        Some(200)
    } else {
        None
    }
}

/// Reads a decimal number right after `marker` on a line.
fn digits_after(line: &str, marker: &str) -> Option<u64> {
    let start = line.find(marker)? + marker.len();
    let rest = line[start..].trim_start();
    let end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    if end == 0 {
        return None;
    }
    rest[..end].parse::<u64>().ok()
}

/// Extracts the `gameId` from the contents of a `*_r3dlog.txt`.
///
/// Priority:
/// 1. `GameStartData::GameID=<n>` (live match or spectator; `0` in replays).
/// 2. `-GameID=<n>` from the command line.
/// 3. `GAME SYSTEM INFO: GameID: <n>` / `Starting GameId: <n>`.
/// 4. `Replay filepath: …\LA2-<n>.rofl` (loaded replay).
///
/// A `GameID=0` is the marker the client uses for replays: it is **not** a
/// valid identifier, so it is ignored and the replay path is used instead.
pub fn game_id_from_r3dlog(content: &str) -> Option<u64> {
    let mut live: Option<u64> = None;
    let mut replay: Option<u64> = None;

    for line in content.lines() {
        let mut found_live = None;
        for marker in [
            "GameStartData::GameID=",
            "-GameID=",
            "GAME SYSTEM INFO: GameID:",
            "Starting GameId:",
        ] {
            if let Some(id) = digits_after(line, marker) {
                found_live = Some(id);
                break;
            }
        }
        if let Some(id) = found_live {
            if id > 0 {
                // The first live identifier belongs to this game run.
                live.get_or_insert(id);
            }
        }
        if replay.is_none() {
            if let Some(pos) = line.find("Replay filepath:") {
                if let Some(id) = game_id_from_rofl_path(&line[pos + "Replay filepath:".len()..]) {
                    replay = Some(id);
                }
            }
        }
    }

    live.or(replay)
}

/// Most recent subfolder of `GameLogs` (by modification date).
fn latest_game_log_subdir(base: &Path) -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = fs::read_dir(base)
        .ok()?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .map(|e| e.path())
        .collect();
    dirs.sort_by_key(|p| modified(p).unwrap_or(SystemTime::UNIX_EPOCH));
    dirs.pop()
}

/// Maximum age of an `r3dlog` for it to be considered part of the **active** session.
///
/// During a match or a replay the engine rewrites the log continuously, so
/// its modification date stays recent. A log from more than 5 minutes ago
/// belongs to a previous session: using it was the bug that linked the previous
/// match (e.g. `GameID=1755132395` from 02:49 while another replay was playing).
const ACTIVE_LOG_MAX_AGE: Duration = Duration::from_secs(300);

fn modified(path: &Path) -> Option<SystemTime> {
    fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// Is the game engine (`League of Legends.exe`) running?
///
/// While the game/replay is open, the session's `r3dlog` is valid
/// even if it has not been written for a while (e.g. a pause). Without this
/// check, a long match could exceed the freshness window and be left without a
/// link. `tasklist` is used (no new dependencies and no console window).
fn game_process_running() -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let mut cmd = std::process::Command::new("tasklist");
        cmd.args([
            "/FI",
            "IMAGENAME eq League of Legends.exe",
            "/NH",
            "/FO",
            "CSV",
        ]);
        cmd.creation_flags(CREATE_NO_WINDOW);
        match cmd.output() {
            Ok(out) => String::from_utf8_lossy(&out.stdout).contains("League of Legends.exe"),
            Err(_) => false,
        }
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// Reads the `r3dlog` tolerating momentary locks and **non-UTF-8 bytes**.
///
/// The engine logs include names/IDs with arbitrary bytes; using
/// `read_to_string` made the read fail and the `gameId` be lost.
fn read_r3dlog(path: &Path) -> Result<String, String> {
    let mut last = String::new();
    for attempt in 0..4 {
        match fs::read(path) {
            Ok(bytes) => return Ok(String::from_utf8_lossy(&bytes).into_owned()),
            Err(e) => {
                last = e.to_string();
                if attempt < 3 {
                    std::thread::sleep(Duration::from_millis(150));
                }
            }
        }
    }
    Err(last)
}

/// `gameId` of the match or replay the client currently has open **now**.
///
/// Step 1: the most recent subfolder of `Logs/GameLogs/`; inside it, its
/// `*_r3dlog.txt`. It is only accepted if that folder/log has been touched in the
/// last [`ACTIVE_LOG_MAX_AGE`] (live session). If the folder is old, `None` is
/// returned: the previous match is **never** linked just because it is the latest.
///
/// Step 2 (only if no log folder exists): the most recent `.rofl`
/// in `Documents/League of Legends/Replays/` ([`find_latest_rofl_game_id`]).
pub fn find_active_game_id() -> Option<u64> {
    let now = SystemTime::now();
    let mut hubo_carpetas_de_log = false;
    for base in game_log_dirs() {
        let Some(dir) = latest_game_log_subdir(&base) else {
            continue;
        };
        hubo_carpetas_de_log = true;
        let dir_mtime = modified(&dir).unwrap_or(SystemTime::UNIX_EPOCH);
        let log = fs::read_dir(&dir).ok().and_then(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| {
                    p.is_file()
                        && p.file_name()
                            .map(|n| n.to_string_lossy().ends_with("_r3dlog.txt"))
                            .unwrap_or(false)
                })
                .max_by_key(|p| modified(p).unwrap_or(SystemTime::UNIX_EPOCH))
        });
        let Some(log) = log else {
            continue;
        };
        let newest = modified(&log).unwrap_or(dir_mtime).max(dir_mtime);
        let age = now.duration_since(newest).unwrap_or_default();
        if age > ACTIVE_LOG_MAX_AGE && !game_process_running() {
            // Log from a previous session: it is not used, and we do not fall
            // back to older folders (they would all be even older).
            continue;
        }
        if let Ok(text) = read_r3dlog(&log) {
            if let Some(id) = game_id_from_r3dlog(&text) {
                return Some(id);
            }
        }
    }
    // If the client has already created its log folder but has not written the id
    // yet, do NOT fall back to the most recent .rofl: that would be the previous match.
    if hubo_carpetas_de_log {
        return None;
    }
    find_latest_rofl_game_id()
}

/// Most recent `.rofl` in `Documents/League of Legends/Replays/` (fallback).
pub fn find_latest_rofl_game_id() -> Option<u64> {
    find_latest_rofl_game_id_since(SystemTime::UNIX_EPOCH)
}

/// Active client session: `gameId` and whether it is a replay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActiveSession {
    pub game_id: u64,
    pub is_replay: bool,
}

/// Active client session, distinguishing a live match from a replay.
///
/// For a replay the LCU timeline is already available and can enrich the
/// dashboard **while it plays back**; for a live match it is not available
/// until the match ends.
pub fn find_active_session() -> Option<ActiveSession> {
    let now = SystemTime::now();
    for base in game_log_dirs() {
        let Some(dir) = latest_game_log_subdir(&base) else {
            continue;
        };
        let dir_mtime = modified(&dir).unwrap_or(SystemTime::UNIX_EPOCH);
        let log = fs::read_dir(&dir).ok().and_then(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| {
                    p.is_file()
                        && p.file_name()
                            .map(|n| n.to_string_lossy().ends_with("_r3dlog.txt"))
                            .unwrap_or(false)
                })
                .max_by_key(|p| modified(p).unwrap_or(SystemTime::UNIX_EPOCH))
        });
        let Some(log) = log else {
            continue;
        };
        let newest = modified(&log).unwrap_or(dir_mtime).max(dir_mtime);
        let age = now.duration_since(newest).unwrap_or_default();
        if age > ACTIVE_LOG_MAX_AGE && !game_process_running() {
            continue;
        }
        if let Ok(text) = read_r3dlog(&log) {
            if let Some(game_id) = game_id_from_r3dlog(&text) {
                return Some(ActiveSession {
                    game_id,
                    is_replay: text.contains("Replay filepath:"),
                });
            }
        }
    }
    None
}

/// `gameId` of the most recent log folder **without a freshness filter**.
///
/// Last resort when a long match ends: the `r3dlog` may have been written
/// only at start-up and be left with an old date. It is used after exhausting
/// the fresh candidates, never before.
pub fn find_latest_game_id_any_age() -> Option<u64> {
    for base in game_log_dirs() {
        let Some(dir) = latest_game_log_subdir(&base) else {
            continue;
        };
        let log = fs::read_dir(&dir).ok().and_then(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| {
                    p.is_file()
                        && p.file_name()
                            .map(|n| n.to_string_lossy().ends_with("_r3dlog.txt"))
                            .unwrap_or(false)
                })
                .max_by_key(|p| modified(p).unwrap_or(SystemTime::UNIX_EPOCH))
        });
        if let Some(log) = log {
            if let Ok(text) = read_r3dlog(&log) {
                if let Some(id) = game_id_from_r3dlog(&text) {
                    return Some(id);
                }
            }
        }
    }
    None
}

/// Most recent `.rofl` modified **since** `since`.
///
/// For the fallback when a capture ends: only the replay the client has just
/// written matters, not an old one the user opened earlier.
pub fn find_latest_rofl_game_id_since(since: SystemTime) -> Option<u64> {
    let doc_dir = std::env::var_os("USERPROFILE")
        .map(|p| PathBuf::from(p).join("Documents"))?;
    let replays_dir = doc_dir.join("League of Legends").join("Replays");
    let mut entries: Vec<PathBuf> = fs::read_dir(replays_dir)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .map(|ext| ext.eq_ignore_ascii_case("rofl"))
                .unwrap_or(false)
        })
        .filter(|p| modified(p).map(|t| t >= since).unwrap_or(false))
        .collect();
    entries.sort_by_key(|p| modified(p).unwrap_or(SystemTime::UNIX_EPOCH));
    entries
        .last()
        .and_then(|p| game_id_from_rofl_path(&p.to_string_lossy()))
}

// ---------------------------------------------------------------------------
// Structures for parsing the Riot Timeline
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LcuTimeline {
    pub frames: Vec<LcuFrame>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LcuFrame {
    pub timestamp: i64,
    #[serde(default)]
    pub events: Vec<LcuTimelineEvent>,
    #[serde(default, rename = "participantFrames")]
    pub participant_frames: std::collections::HashMap<String, LcuParticipantFrame>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LcuParticipantFrame {
    #[serde(rename = "participantId")]
    pub participant_id: i32,
    #[serde(default, rename = "currentGold")]
    pub current_gold: i32,
    #[serde(default, rename = "totalGold")]
    pub total_gold: i32,
    #[serde(default)]
    pub level: i32,
    #[serde(default)]
    pub xp: i32,
    #[serde(default, rename = "minionsKilled")]
    pub minions_killed: i32,
    #[serde(default, rename = "jungleMinionsKilled")]
    pub jungle_minions_killed: i32,
    pub position: Option<LcuPosition>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LcuPosition {
    pub x: i32,
    pub y: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LcuTimelineEvent {
    #[serde(rename = "type")]
    pub event_type: String,
    pub timestamp: i64,
    pub position: Option<LcuPosition>,
    #[serde(default, rename = "killerId")]
    pub killer_id: i32,
    #[serde(default, rename = "victimId")]
    pub victim_id: i32,
    #[serde(default, rename = "creatorId")]
    pub creator_id: i32,
    #[serde(default, rename = "wardType")]
    pub ward_type: Option<String>,
    #[serde(default, rename = "monsterType")]
    pub monster_type: Option<String>,
    #[serde(default, rename = "monsterSubType")]
    pub monster_sub_type: Option<String>,
    #[serde(default, rename = "teamId")]
    pub team_id: Option<i32>,
    #[serde(default, rename = "towerType")]
    pub tower_type: Option<String>,
    #[serde(default, rename = "buildingType")]
    pub building_type: Option<String>,
    #[serde(default, rename = "assistingParticipantIds")]
    pub assisting_participant_ids: Vec<i32>,
    #[serde(default, rename = "laneType")]
    pub lane_type: Option<String>,
}

impl LcuTimeline {
    /// Generates the accumulated gold series `(t_seconds, accumulated_blue_gold, accumulated_red_gold)`.
    /// In the LoL client, participants 1..=5 belong to the Blue team (100)
    /// and participants 6..=10 belong to the Red team (200).
    pub fn gold_series(&self) -> Vec<(f32, i64, i64)> {
        let mut series = Vec::with_capacity(self.frames.len());
        for frame in &self.frames {
            let t_s = (frame.timestamp as f32) / 1000.0;
            let mut blue_gold: i64 = 0;
            let mut red_gold: i64 = 0;

            for pf in frame.participant_frames.values() {
                if pf.participant_id <= 5 {
                    blue_gold += pf.total_gold as i64;
                } else {
                    red_gold += pf.total_gold as i64;
                }
            }

            series.push((t_s, blue_gold, red_gold));
        }
        series.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        series
    }

    /// Extracts every ward-placed event with its exact coordinates on the rift.
    /// Returns: `(timestamp_s, ward_type, x, y, creator_id)`
    pub fn ward_events(&self) -> Vec<(f32, String, i32, i32, i32)> {
        self.ward_events_with_team()
            .into_iter()
            .map(|(t, w, x, y, c, _)| (t, w, x, y, c))
            .collect()
    }

    /// Same as [`Self::ward_events`], but keeps the event's `teamId`.
    ///
    /// Many `WARD_PLACED` events from replays/scrims arrive with `creatorId == 0`; the
    /// `teamId` (100/200) makes it possible to attribute the ward to a side without losing it.
    /// Returns: `(timestamp_s, ward_type, x, y, creator_id, team_id)`.
    pub fn ward_events_with_team(&self) -> Vec<(f32, String, i32, i32, i32, Option<i32>)> {
        let mut wards = Vec::new();
        for frame in &self.frames {
            for ev in &frame.events {
                if ev.event_type == "WARD_PLACED" {
                    if let Some(pos) = &ev.position {
                        let t_s = (ev.timestamp as f32) / 1000.0;
                        let w_type = ev
                            .ward_type
                            .clone()
                            .unwrap_or_else(|| "UNDEFINED".to_string());
                        wards.push((t_s, w_type, pos.x, pos.y, ev.creator_id, ev.team_id));
                    }
                }
            }
        }
        wards.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        wards
    }

    /// Extracts epic objectives (Dragons, Heralds, Barons, Atakhan) with their exact moment and team.
    /// Returns: `(timestamp_s, monster_type, monster_sub_type, killer_id, killer_team)`
    pub fn objective_events(&self) -> Vec<(f32, String, Option<String>, i32, Option<i32>)> {
        let mut objs = Vec::new();
        for frame in &self.frames {
            for ev in &frame.events {
                if ev.event_type == "ELITE_MONSTER_KILL" {
                    let t_s = (ev.timestamp as f32) / 1000.0;
                    let m_type = ev.monster_type.clone().unwrap_or_else(|| "MONSTER".to_string());
                    let team = team_from_participant(ev.killer_id).or(ev.team_id);
                    objs.push((t_s, m_type, ev.monster_sub_type.clone(), ev.killer_id, team));
                }
            }
        }
        objs.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        objs
    }

    /// Total estimated duration in seconds according to the last frame.
    pub fn duration_s(&self) -> f32 {
        self.frames.last().map(|f| (f.timestamp as f32) / 1000.0).unwrap_or(0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn game_id_de_replay_desde_r3dlog() {
        // Real replay header (LA2-1632883165): GameID=0 is not valid.
        let log = "\
000000.242| ALWAYS|   CFG| Command Line:  \"C:/Users/x/Replays/LA2-1632883165.rofl\"
000000.611| ALWAYS|   CFG| Replay filepath: C:/Users/x/Replays/LA2-1632883165.rofl
000002.093| ALWAYS| GameStartData::GameID=0
";
        assert_eq!(game_id_from_r3dlog(log), Some(1632883165));
    }

    #[test]
    fn game_id_de_partida_en_vivo_desde_r3dlog() {
        let log = "\
000000.340| ALWAYS|   CFG| Command Line:  \"spectator 1.2.3.4 7467 key\" \"-GameID=1632456654\"
000002.896| ALWAYS| GameStartData::GameID=1632456654
";
        assert_eq!(game_id_from_r3dlog(log), Some(1632456654));
    }

    #[test]
    fn game_id_ignora_cero_y_usa_replay() {
        let log = "GameStartData::GameID=0\nReplay filepath: C:\\Replays\\LA2-1632919309.rofl\n";
        assert_eq!(game_id_from_r3dlog(log), Some(1632919309));
    }

    #[test]
    fn game_id_sin_pistas() {
        assert_eq!(game_id_from_r3dlog("nada útil\n"), None);
    }

    #[test]
    fn game_id_de_ruta_de_replay() {
        assert_eq!(
            game_id_from_rofl_path("C:/Users/x/Documents/League of Legends/Replays/LA2-1632883165.rofl"),
            Some(1632883165)
        );
        assert_eq!(
            game_id_from_rofl_path("C:\\Replays\\EUW1-1234567890.ROFL"),
            Some(1234567890)
        );
        assert_eq!(game_id_from_rofl_path("no-es-un-rofl.txt"), None);
    }

    #[test]
    fn totales_de_vision_del_resumen_de_partida() {
        let game = serde_json::json!({
            "gameMode": "CLASSIC",
            "gameMapName": "Map11",
            "gameDuration": 2064,
            "participants": [
                {"teamId": 100, "stats": {"wardsPlaced": 12}},
                {"teamId": 100, "stats": {"wardsPlaced": 9}},
                {"teamId": 200, "stats": {"wardsPlaced": 7}},
                {"teamId": 200, "stats": {"wardsPlaced": 3}}
            ]
        });
        let s = match_summary_from_game(123, &game).expect("resumen");
        assert_eq!(s.vision.blue, 21);
        assert_eq!(s.vision.red, 10);
        assert_eq!(s.vision.total(), 31);
        assert_eq!(s.game_mode, "CLASSIC");
        assert_eq!(s.map_name, "Map11");
        assert!((s.duration_s - 2064.0).abs() < 0.01);
    }

    #[test]
    fn resumen_sin_participantes_es_error() {
        let game = serde_json::json!({"gameMode": "CLASSIC"});
        assert!(match_summary_from_game(1, &game).is_err());
    }
}
