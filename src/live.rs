//! Live collector via Riot's **Live Client Data API**
//! (`https://127.0.0.1:2999/liveclientdata/allgamedata`).
//!
//! ## What it measures and what it does not (empirically verified, patch 16.x)
//!
//! The endpoint returns `allPlayers[10]`, `gameData.gameTime` and `events.Events`.
//! For each player the following can be read **directly and measurably**:
//!
//! * `scores.creepScore` (CS), `scores.kills/deaths/assists`, `scores.wardScore`
//! * `level` (measured proxy for XP), `isDead`, `respawnTimer`
//! * `team` (`ORDER`/`CHAOS`), `position` (role/lane), `items[].price`
//! * the `events.Events` feed with `EventName`, `EventTime`, `KillerName`,
//!   `VictimName`, `Assisters[]`, `TurretKilled`, … → **team attribution**
//!   resolvable by mapping name→`team`.
//!
//! In addition, `/liveclientdata/stats` is queried as a **second measured
//! source** of vision (`wardsPlaced`/`wardsKilled` per champion). This is
//! needed because the client may serve `scores` **all zeros** (observed in
//! spectator mode: CS, kills and `wardScore` all at 0); in that case the
//! vision share is computed from the measured placed wards, never from an
//! invented value.
//!
//! What the API does **not** provide (verified: the `gold`/`xp` string does not
//! appear in any endpoint and `activePlayer` responds `400 Spectator mode …`):
//!
//! * **Gold per player** and **XP per player** → **reconstructed** from
//!   measured inputs (CS, kills, assists) with external game constants,
//!   always with `reconstruido_por_modelo: true` and the exported parameters.
//! * **Map coordinates** (`position` is the lane; `screenPositionCenter` are
//!   screen pixels with an `f32::MAX` sentinel outside the viewport) → the heat
//!   maps do **not** come from here; they remain the responsibility of the
//!   `.rofl` parser (type 61 stream).
//!
//! ## Transport
//!
//! The local API is read **from inside the process** with a native HTTPS
//! client (`reqwest` + `rustls`, see [`crate::tls`]): no child process is
//! spawned and `curl.exe` is not needed on the user's PC. The League client
//! uses a **self-signed certificate**, so its fingerprint is **pinned** in
//! memory: stricter than the old `-k` flag of `curl`.
//! The module does not panic: every function returns `Result`.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::blocks::compute_gold_blocks;
use crate::tls::ClienteLocal;
use crate::model::{GoldBlocksReport, Team, TeamTop5};
use crate::score::{DominionReport, ScoreParams, VisionThresholds};

/// Default endpoint of the Live Client Data API (localhost only).
pub const LIVE_ENDPOINT: &str = "https://127.0.0.1:2999/liveclientdata/allgamedata";

/// Block length in seconds (same grid as the `.rofl` report).
pub const BLOCK_SECS: f32 = 30.0;

/// Margin (in seconds) for considering two events with the same
/// identity to be **the same event re-emitted** by the feed.
///
/// Measured: the same kill comes back with the `EventTime` shifted ~0.1–0.3 s
/// between polls. 2 s leaves plenty of margin without risk of merging two
/// real actions (a champion cannot die twice that quickly).
pub const LIVE_EVENT_EPSILON_S: f32 = 2.0;

/// Registers an event in the deduplication index and returns `true` if it is
/// a **re-emission** (the feed sends the same action again with a shifted
/// time) and therefore must be discarded.
///
/// Note: the key stores **all** the instances seen, not just the last one.
/// The feed is cumulative: on every poll it re-emits the whole history, so
/// the copy of an old kill arrives *after* other more recent ones have been
/// seen. Comparing only against the latest instant let the copies through.
fn es_evento_repetido(
    seen: &mut std::collections::HashMap<EventKey, Vec<f32>>,
    key: &EventKey,
    t: f32,
) -> bool {
    let instantes = seen.entry(key.clone()).or_default();
    if instantes.iter().any(|p| (t - *p).abs() <= LIVE_EVENT_EPSILON_S) {
        return true; // we had already seen it (same instant ± ε).
    }
    instantes.push(t);
    false
}

/// Identity of a feed event: what makes it unique.
///
/// For kills, the (killer, victim) pair is unique in the game; for turrets
/// and plates, the turret id; for objectives, the monster. Events with no
/// identifier at all (FirstBlood, Ace, GameStart…) are distinguished only by
/// their type.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct EventKey {
    kind: String,
    killer: Option<String>,
    victim: Option<String>,
    turret: Option<String>,
    monster: Option<String>,
    recipient: Option<String>,
}

impl EventKey {
    fn de(ev: &RawEvent) -> Self {
        let no_vacio = |o: &Option<String>| {
            o.clone().filter(|s| !s.is_empty())
        };
        let monster = no_vacio(&ev.dragon_killed)
            .or_else(|| no_vacio(&ev.baron_killed))
            .or_else(|| no_vacio(&ev.herald_killed))
            .or_else(|| no_vacio(&ev.epic_monster_kill))
            .or_else(|| no_vacio(&ev.monster_type));
        Self {
            kind: ev.event_name.clone().unwrap_or_default(),
            killer: no_vacio(&ev.killer_name),
            victim: no_vacio(&ev.victim_name),
            turret: no_vacio(&ev.turret_killed).or_else(|| no_vacio(&ev.inhib_killed)),
            monster,
            recipient: no_vacio(&ev.recipient),
        }
    }
}

// ---------------------------------------------------------------------------
// Gold reconstruction model (EXTERNAL game constants, adjustable)
// ---------------------------------------------------------------------------

/// Constants used to **reconstruct** gold from measured signals.
///
/// They are external game knowledge (wiki), **not** measurements from the
/// binary or the client. They are approximations: real gold per kill/CS varies
/// with bounties and with the type of minion. They are exported verbatim in the
/// JSON so the result is auditable and reproducible, and the resulting block is
/// tagged `reconstruido_por_modelo: true`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GoldModelParams {
    /// Approximate average gold per last hit.
    pub avg_cs_gold: f64,
    /// Approximate average gold per kill (ignores shutdowns/bounties).
    pub kill_gold: f64,
    /// Approximate average gold per assist.
    pub assist_gold: f64,
    /// Approximate passive gold per champion and minute.
    pub passive_gold_per_min: f64,
}

impl Default for GoldModelParams {
    fn default() -> Self {
        Self {
            avg_cs_gold: 19.0,
            kill_gold: 300.0,
            assist_gold: 150.0,
            passive_gold_per_min: 204.0,
        }
    }
}

// ---------------------------------------------------------------------------
// Transport
// ---------------------------------------------------------------------------

/// Performs the GET against the endpoint and returns the body as UTF-8.
///
/// An error (e.g. *connection refused*) means there is **no active game** in
/// the client: the caller decides whether to retry.
///
/// The request is made **inside the process** (native HTTPS client with the
/// local client certificate **pinned**), without spawning `curl.exe` as a
/// child process: that pattern (app -> runs curl -> URL, with `-k` and no
/// window) is the one antivirus software likes most. See [`crate::tls`].
fn fetch_raw(cliente: &ClienteLocal, endpoint: &str) -> Result<String, String> {
    cliente
        .get(endpoint)
        .map_err(|e| format!("no response from the client (is a game active?): {e}"))
}

// ---------------------------------------------------------------------------
// Defensive schema of the live JSON
// ---------------------------------------------------------------------------

/// Raw state of a player in a snapshot (optional fields: the schema differs
/// between spectator mode and player mode).
///
/// The API uses `camelCase` (`summonerName`, `creepScore`, …), hence the rename.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawPlayer {
    #[serde(default)]
    summoner_name: Option<String>,
    #[serde(default)]
    riot_id_game_name: Option<String>,
    #[serde(default)]
    champion_name: Option<String>,
    #[serde(default)]
    team: Option<String>,
    #[serde(default)]
    position: Option<String>,
    #[serde(default)]
    level: Option<u32>,
    #[serde(default)]
    is_dead: Option<bool>,
    #[serde(default)]
    respawn_timer: Option<f64>,
    #[serde(default)]
    scores: Option<RawScores>,
    #[serde(default)]
    items: Option<Vec<RawItem>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawScores {
    #[serde(default)]
    assists: Option<u32>,
    #[serde(default)]
    creep_score: Option<u32>,
    #[serde(default)]
    deaths: Option<u32>,
    #[serde(default)]
    kills: Option<u32>,
    #[serde(default)]
    ward_score: Option<f64>,
}

#[derive(Debug, Clone, Deserialize)]
struct RawItem {
    #[serde(default)]
    price: Option<i64>,
}

// ---------------------------------------------------------------------------
// Second measured source: `/liveclientdata/stats` (wards)
// ---------------------------------------------------------------------------

/// Response from `/liveclientdata/stats`: per-player statistics.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawStats {
    #[serde(default)]
    player_stats: Option<Vec<RawPlayerStat>>,
}

/// Row of `playerStats`. This endpoint does **not** include the summoner name:
/// the join key is `championName` (in a normal game every champion is unique;
/// if one were repeated, the row is discarded to avoid misattribution).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawPlayerStat {
    #[serde(default)]
    champion_name: Option<String>,
    /// The real client emits `wardsPlaced`; some versions `wardPlaced`.
    #[serde(default, alias = "wardPlaced")]
    wards_placed: Option<u32>,
    #[serde(default, alias = "wardKilled")]
    wards_killed: Option<u32>,
}

/// Derives the `/stats` endpoint from the `allgamedata` one.
fn stats_endpoint(endpoint: &str) -> String {
    if endpoint.contains("/allgamedata") {
        endpoint.replace("/allgamedata", "/stats")
    } else {
        "https://127.0.0.1:2999/liveclientdata/stats".to_string()
    }
}

/// Downloads `/stats` and returns `champion (lowercase) → (wards placed,
/// wards killed)`.
///
/// It is an **optional** source: if the client does not serve the endpoint,
/// does not include `playerStats`, or does not expose the fields, it returns
/// `None` and the players are left without ward data (never with an invented
/// zero).
fn fetch_wards(cliente: &ClienteLocal, endpoint: &str) -> Option<std::collections::HashMap<String, (u32, u32)>> {
    let raw = fetch_raw(cliente, &stats_endpoint(endpoint)).ok()?;
    let parsed: RawStats = serde_json::from_str(&raw).ok()?;
    let mut map = std::collections::HashMap::new();
    for s in parsed.player_stats.unwrap_or_default() {
        let Some(champ) = s.champion_name.filter(|c| !c.is_empty()) else {
            continue;
        };
        let Some(placed) = s.wards_placed else {
            continue;
        };
        map.insert(champ.to_lowercase(), (placed, s.wards_killed.unwrap_or(0)));
    }
    if map.is_empty() {
        None
    } else {
        Some(map)
    }
}

/// Raw event from the `events.Events` feed.
///
/// The feed uses `PascalCase` (`EventID`, `EventName`, `KillerName`, …), unlike
/// the rest of the objects, which use `camelCase`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
#[allow(dead_code)] // Deserialization schema: all feed fields are kept.
struct RawEvent {
    #[serde(rename = "EventID", default)]
    event_id: Option<u64>,
    #[serde(default)]
    event_name: Option<String>,
    #[serde(default)]
    event_time: Option<f64>,
    #[serde(default)]
    killer_name: Option<String>,
    #[serde(default)]
    victim_name: Option<String>,
    #[serde(default)]
    assisters: Option<Vec<String>>,
    #[serde(default)]
    turret_killed: Option<String>,
    #[serde(default)]
    inhib_killed: Option<String>,
    #[serde(default)]
    dragon_killed: Option<String>,
    /// The real feed uses `DragonType` ("Fire", "Hextech"…) in the
    /// `DragonKill` event; it is kept as the epic monster type.
    #[serde(default)]
    dragon_type: Option<String>,
    #[serde(default)]
    baron_killed: Option<String>,
    #[serde(default)]
    herald_killed: Option<String>,
    #[serde(default)]
    epic_monster_kill: Option<String>,
    #[serde(default)]
    monster_type: Option<String>,
    #[serde(default)]
    recipient: Option<String>,
    #[serde(default)]
    kill_streak: Option<u32>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawGameData {
    #[serde(default)]
    game_time: Option<f64>,
    #[serde(default)]
    game_mode: Option<String>,
    #[serde(default)]
    map_name: Option<String>,
    // mapNumber / mapTerrain are ignored.
}

#[derive(Debug, Clone, Deserialize)]
struct RawEvents {
    #[serde(rename = "Events", default)]
    events: Option<Vec<RawEvent>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawScoreboardTeam {
    #[serde(default)]
    dragon_kills: Option<u32>,
    #[serde(default)]
    baron_kills: Option<u32>,
    #[serde(default)]
    rift_herald_kills: Option<u32>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawScoreboard {
    #[serde(default)]
    team1: Option<RawScoreboardTeam>,
    #[serde(default)]
    team2: Option<RawScoreboardTeam>,
}

#[derive(Debug, Clone, Deserialize)]
struct RawAllGameData {
    #[serde(rename = "allPlayers", default)]
    all_players: Option<Vec<RawPlayer>>,
    #[serde(default)]
    events: Option<RawEvents>,
    #[serde(rename = "gameData", default)]
    game_data: Option<RawGameData>,
    #[serde(default)]
    scoreboard: Option<RawScoreboard>,
}

// ---------------------------------------------------------------------------
// Normalized snapshot
// ---------------------------------------------------------------------------

/// Normalized sample of a player in a snapshot (all measured, no model).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayerSample {
    /// Stable identifier: `riotIdGameName` if present, otherwise `summonerName`.
    pub id: String,
    pub summoner_name: String,
    pub champion_name: String,
    pub team: Team,
    pub lane: String,
    pub level: u32,
    pub cs: u32,
    pub kills: u32,
    pub deaths: u32,
    pub assists: u32,
    pub ward_score: f64,
    pub is_dead: bool,
    pub respawn_s: f64,
    /// Sum of `items[].price` = **measured** gold already converted into items
    /// (lower bound of invested gold; excludes used/sold consumables).
    pub item_gold: i64,
    /// Cumulative wards placed, measured from `/liveclientdata/stats`
    /// (`None` if the client did not expose the value in that poll).
    #[serde(default)]
    pub wards_placed: Option<u32>,
    /// Cumulative enemy wards destroyed (`/liveclientdata/stats`).
    #[serde(default)]
    pub wards_killed: Option<u32>,
}

/// Epic objective counters read from the **scoreboard**.
///
/// The event feed does **not** report dragon/herald/baron kills (only
/// `ChampionKill`, `TurretKilled`…), but the scoreboard does carry
/// `dragonKills`, `baronKills` and `riftHeraldKills` per team. The delta
/// between snapshots tells **who** took it and **in which minute**.
///
/// Everything is **measured** (not inferred from the feed).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct EpicCounts {
    pub dragons_blue: u32,
    pub dragons_red: u32,
    pub heralds_blue: u32,
    pub heralds_red: u32,
    pub barons_blue: u32,
    pub barons_red: u32,
}

impl EpicCounts {
    /// Extracts the counters from the scoreboard (`None` if the client did not provide them).
    fn from_scoreboard(sb: &Option<RawScoreboard>) -> Option<Self> {
        let sb = sb.as_ref()?;
        let t1 = sb.team1.as_ref();
        let t2 = sb.team2.as_ref();
        let d1 = t1.and_then(|t| t.dragon_kills);
        let d2 = t2.and_then(|t| t.dragon_kills);
        let b1 = t1.and_then(|t| t.baron_kills);
        let b2 = t2.and_then(|t| t.baron_kills);
        let h1 = t1.and_then(|t| t.rift_herald_kills);
        let h2 = t2.and_then(|t| t.rift_herald_kills);
        if d1.is_none() && d2.is_none() && b1.is_none() && b2.is_none() && h1.is_none() && h2.is_none() {
            return None;
        }
        Some(Self {
            dragons_blue: d1.unwrap_or(0),
            dragons_red: d2.unwrap_or(0),
            heralds_blue: h1.unwrap_or(0),
            heralds_red: h2.unwrap_or(0),
            barons_blue: b1.unwrap_or(0),
            barons_red: b2.unwrap_or(0),
        })
    }
}

/// Complete snapshot at one instant of the game clock.
///
/// This is **internal** poller state (it is not serialized; the exported report
/// is [`ScrimCapture`], which uses [`AttributedEvent`]).
#[derive(Debug, Clone)]
pub struct LiveSnapshot {
    /// `gameData.gameTime` (game seconds) — master clock for the grid.
    pub t_game_s: f32,
    /// Wall-clock timestamp (ms since epoch) for cadence diagnostics.
    pub wall_ms: u128,
    pub players: Vec<PlayerSample>,
    /// Cumulative epic objectives from the scoreboard at that instant.
    pub epics: Option<EpicCounts>,
}

/// Converts the API's raw `team` into our `Team`.
fn team_from_raw(raw: Option<&str>) -> Option<Team> {
    match raw? {
        "ORDER" => Some(Team::Blue),
        "CHAOS" => Some(Team::Red),
        _ => None,
    }
}

/// Normalizes a player row. Returns `None` if there is no usable data.
fn normalize_player(p: &RawPlayer) -> Option<PlayerSample> {
    let team = team_from_raw(p.team.as_deref())?;
    let id = p
        .riot_id_game_name
        .clone()
        .filter(|s| !s.is_empty())
        .or_else(|| p.summoner_name.clone())
        .unwrap_or_default();
    let item_gold = p
        .items
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .filter_map(|it| it.price)
        .sum();
    let sc = p.scores.as_ref();
    Some(PlayerSample {
        id,
        summoner_name: p.summoner_name.clone().unwrap_or_default(),
        champion_name: p.champion_name.clone().unwrap_or_default(),
        team,
        lane: p.position.clone().unwrap_or_default(),
        level: p.level.unwrap_or(0),
        cs: sc.and_then(|s| s.creep_score).unwrap_or(0),
        kills: sc.and_then(|s| s.kills).unwrap_or(0),
        deaths: sc.and_then(|s| s.deaths).unwrap_or(0),
        assists: sc.and_then(|s| s.assists).unwrap_or(0),
        ward_score: sc.and_then(|s| s.ward_score).unwrap_or(0.0),
        is_dead: p.is_dead.unwrap_or(false),
        respawn_s: p.respawn_timer.unwrap_or(0.0),
        item_gold,
        // Filled in a second step (`/liveclientdata/stats`), which is not
        // always available: in the meantime, no data (not zero).
        wards_placed: None,
        wards_killed: None,
    })
}

// ---------------------------------------------------------------------------
// Poller
// ---------------------------------------------------------------------------

/// Live collector: polls the endpoint in a loop, normalizes snapshots and
/// accumulates the scrim state. It does not panic; network failures are reported.
#[derive(Debug)]
pub struct LivePoller {
    endpoint: String,
    timeout_s: u32,
    poll_interval: Duration,
    params: GoldModelParams,
    /// Most recent snapshot (final state per player).
    latest: Vec<PlayerSample>,
    /// Identity key of an event (`kind` + killer + victim + turret) with
    /// **all** the instants at which it has been seen.
    ///
    /// Riot's feed is **cumulative and re-emits**: the same kill appears again
    /// in successive polls (sometimes with a new `EventID` and the `EventTime`
    /// slightly shifted). Deduplicating by `EventID` is not enough: it must be
    /// done by **content**. The key is unique per real event (a champion
    /// cannot die twice, a turret is destroyed once), so two matches within
    /// `LIVE_EVENT_EPSILON_S` are the same repeated action.
    ///
    /// **All** instances are stored, not just the last one: the feed re-emits
    /// the full history on every poll, so the copy of an old kill arrives after
    /// other more recent ones have been seen.
    seen_events: std::collections::HashMap<EventKey, Vec<f32>>,
    /// Native HTTPS client (created on the first poll and reused, so the TLS
    /// connection stays open between polls).
    cliente: Option<ClienteLocal>,
    /// Captured snapshots (one per poll).
    snapshots: Vec<LiveSnapshot>,
    /// Deduplicated events attributed across the whole scrim.
    events: Vec<AttributedEvent>,
    /// Last game clock seen (end/regression detection).
    last_game_time: f32,
    /// Consecutive network failures after data has been captured.
    consecutive_failures: u32,
    game_mode: String,
    map_name: String,
}

/// Live feed event with **team attribution** (resolved by mapping
/// name→team) and a timestamp in game seconds.
///
/// `#[serde(default)]` allows reading captures saved by builds whose schema
/// omitted optional fields (e.g. without `turret`/`monster`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AttributedEvent {
    pub t_game_s: f32,
    pub kind: String,
    pub team: Option<Team>,
    pub killer: Option<String>,
    pub victim: Option<String>,
    pub victim_team: Option<Team>,
    pub assister_count: usize,
    pub turret: Option<String>,
    pub monster: Option<String>,
}

impl LivePoller {
    /// Creates a poller with the default configuration.
    pub fn new() -> Self {
        Self::with_config(LIVE_ENDPOINT, 2, Duration::from_millis(1000), GoldModelParams::default())
    }

    /// Explicit configuration. `timeout_s` is the per-HTTP-request timeout;
    /// `poll_interval` is the spacing between polls.
    pub fn with_config(
        endpoint: &str,
        timeout_s: u32,
        poll_interval: Duration,
        params: GoldModelParams,
    ) -> Self {
        Self {
            endpoint: endpoint.to_string(),
            timeout_s,
            poll_interval,
            params,
            seen_events: std::collections::HashMap::new(),
            cliente: None,
            latest: Vec::new(),
            snapshots: Vec::new(),
            events: Vec::new(),
            last_game_time: -1.0,
            consecutive_failures: 0,
            game_mode: String::new(),
            map_name: String::new(),
        }
    }

    /// Interval between polls.
    pub fn poll_interval(&self) -> Duration {
        self.poll_interval
    }

    /// Number of captured snapshots.
    pub fn snapshot_count(&self) -> usize {
        self.snapshots.len()
    }

    /// Game duration seen (game seconds of the last poll).
    pub fn duration_s(&self) -> f32 {
        self.last_game_time.max(0.0)
    }

    /// Performs one poll. Returns `Ok(true)` if there was a snapshot, `Ok(false)`
    /// if there is no active game (network down), without treating it as a
    /// fatal error.
    pub fn poll_once(&mut self) -> Result<bool, String> {
        // The HTTPS client is created on the first poll and reused afterwards
        // (so the TLS connection stays alive between polls).
        if self.cliente.is_none() {
            self.cliente = Some(
                ClienteLocal::new(self.timeout_s as u64)
                    .map_err(|e| format!("could not create the local HTTPS client: {e}"))?,
            );
        }
        let raw = {
            let cliente = self.cliente.as_ref().expect("cliente HTTPS ya creado");
            match fetch_raw(cliente, &self.endpoint) {
                Ok(r) => r,
                Err(e) => {
                    self.consecutive_failures = self.consecutive_failures.saturating_add(1);
                    return Err(e);
                }
            }
        };
        self.consecutive_failures = 0;
        self.ingest_json(&raw)?;
        // Second measured vision source: `/stats` provides wardsPlaced/
        // wardsKilled (the `scores` block does not always expose them). If it
        // does not respond, we go with what we have: nothing is invented.
        let wards = {
            let cliente = self.cliente.as_ref().expect("cliente HTTPS ya creado");
            fetch_wards(cliente, &self.endpoint)
        };
        if let Some(wards) = wards {
            self.apply_wards(&wards);
        }
        Ok(true)
    }

    /// Applies `champion → (wards placed, wards killed)` to the latest
    /// snapshot. A repeated champion is left without data (it is impossible to
    /// know which player the `/stats` row belongs to, since it has no name).
    fn apply_wards(&mut self, wards: &std::collections::HashMap<String, (u32, u32)>) {
        let Some(last) = self.snapshots.last_mut() else {
            return;
        };
        let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        for p in &last.players {
            *counts.entry(p.champion_name.to_lowercase()).or_default() += 1;
        }
        for p in last.players.iter_mut() {
            let key = p.champion_name.to_lowercase();
            if counts.get(&key).copied().unwrap_or(0) != 1 {
                continue;
            }
            if let Some(&(placed, killed)) = wards.get(&key) {
                p.wards_placed = Some(placed);
                p.wards_killed = Some(killed);
            }
        }
        self.latest = last.players.clone();
    }

    /// Parses and absorbs an `allgamedata` JSON body (a **data-pure** function,
    /// no network → testable with fixtures).
    ///
    /// Returns an error only if the JSON does not match the expected schema.
    pub fn ingest_json(&mut self, raw: &str) -> Result<(), String> {
        let parsed: RawAllGameData =
            serde_json::from_str(raw).map_err(|e| format!("unexpected JSON: {e}"))?;

        if let Some(gd) = parsed.game_data.as_ref() {
            if let Some(m) = gd.game_mode.as_deref() {
                self.game_mode = m.to_string();
            }
            if let Some(m) = gd.map_name.as_deref() {
                self.map_name = m.to_string();
            }
        }
        let t_game_s = parsed
            .game_data
            .as_ref()
            .and_then(|g| g.game_time)
            .unwrap_or(0.0) as f32;

        let players: Vec<PlayerSample> = parsed
            .all_players
            .as_deref()
            .unwrap_or(&[])
            .iter()
            .filter_map(normalize_player)
            .collect();

        // name→team map to attribute the feed events.
        let mut name_team: std::collections::HashMap<&str, Team> =
            std::collections::HashMap::new();
        for p in &players {
            if !p.summoner_name.is_empty() {
                name_team.insert(p.summoner_name.as_str(), p.team);
            }
            if !p.id.is_empty() {
                name_team.insert(p.id.as_str(), p.team);
            }
        }

        // Feed deduplication **by content** (the feed is cumulative and
        // re-emits every kill with a new EventID) + attribution.
        for ev in parsed
            .events
            .as_ref()
            .and_then(|e| e.events.as_deref())
            .unwrap_or(&[])
        {
            // The event carries its own historical `EventTime`: the poll clock
            // is NOT used, or they would all fall into the current block.
            let t_ev = ev.event_time.unwrap_or(f64::from(t_game_s)) as f32;
            let key = EventKey::de(ev);
            if es_evento_repetido(&mut self.seen_events, &key, t_ev) {
                continue; // same event re-emitted by the feed.
            }
            let attributed = attribute_event(ev, t_ev, &name_team);
            self.events.push(attributed);
        }

        self.latest = players;
        self.last_game_time = self.last_game_time.max(t_game_s);
        self.snapshots.push(LiveSnapshot {
            t_game_s,
            wall_ms: now_ms(),
            players: self.latest.clone(),
            epics: EpicCounts::from_scoreboard(&parsed.scoreboard),
        });
        Ok(())
    }

    /// Number of consecutive failed polls (reset on any success).
    pub fn consecutive_failures(&self) -> u32 {
        self.consecutive_failures
    }

    /// Have enough network failures accumulated to consider the scrim over
    /// (client closed / replay finished)?
    pub fn looks_finished(&self, threshold: u32) -> bool {
        !self.snapshots.is_empty() && self.consecutive_failures >= threshold
    }

    /// Cumulative `(t_s, blue_gold, red_gold)` series **reconstructed** from
    /// the measured snapshots, to feed the block/Top 5 engine.
    ///
    /// Team gold is estimated by summing, over each team's players,
    /// `cs·avg_cs_gold + kills·kill_gold + assists·assist_gold +
    /// passive·min`. It is a **model** (not a measurement): the resulting
    /// report is tagged `reconstruido_por_modelo: true`.
    pub fn gold_series(&self) -> Vec<(f32, i64, i64)> {
        let p = &self.params;
        let mut series: Vec<(f32, i64, i64)> = self
            .snapshots
            .iter()
            .map(|s| {
                let mut blue = 0.0;
                let mut red = 0.0;
                for pl in &s.players {
                    let g = f64::from(pl.cs) * p.avg_cs_gold
                        + f64::from(pl.kills) * p.kill_gold
                        + f64::from(pl.assists) * p.assist_gold
                        + p.passive_gold_per_min * (f64::from(s.t_game_s) / 60.0);
                    match pl.team {
                        Team::Blue => blue += g,
                        Team::Red => red += g,
                    }
                }
                (s.t_game_s, blue as i64, red as i64)
            })
            .collect();
        // Guarantees non-decreasing monotonicity per team (sampling noise).
        let mut max_b = i64::MIN;
        let mut max_r = i64::MIN;
        for e in series.iter_mut() {
            max_b = max_b.max(e.1);
            max_r = max_r.max(e.2);
            e.1 = max_b;
            e.2 = max_r;
        }
        series
    }

    /// 30 s block engine + Top 5 fed by the reconstructed series.
    pub fn gold_blocks(&self) -> GoldBlocksReport {
        let owned = self.gold_series();
        compute_gold_blocks(&owned, self.duration_s(), "reconstruido_live")
    }

    /// Builds the complete scrim report (blocks + events + Top 5).
    pub fn build_capture(&self) -> ScrimCapture {
        let blocks = self.build_blocks();
        let gblocks = self.gold_blocks();
        // Score engine over 30 s windows: breakpoints + Top 5 + vision.
        let dominion = crate::score::compute_report(
            &blocks,
            ScoreParams::default(),
            VisionThresholds::default(),
            "live_client_data_api",
        );
        let mut omisiones = omissions();
        omisiones.extend(self.reconcile_feed());
        if blocks.iter().any(|b| b.vision_source == "wards_placed") {
            omisiones.push(
                "Visión medida con wards colocadas (/liveclientdata/stats): el \
                 cliente no expuso wardScore en esta partida; la cuota de visión \
                 usa el conteo medido de wards, no la puntuación de visión."
                    .to_string(),
            );
        } else if blocks.iter().all(|b| b.vision_source.is_empty()) {
            omisiones.push(
                "Sin datos de visión en esta captura: el cliente no expuso ni \
                 wardScore ni wardsPlaced."
                    .to_string(),
            );
        }

        let captura = ScrimCapture {
            fuente: "live_client_data_api".to_string(),
            endpoint: self.endpoint.clone(),
            game_id: None,
            vision_totals: None,
            reconstruido_por_modelo: true,
            game_mode: self.game_mode.clone(),
            map_name: self.map_name.clone(),
            duration_s: self.duration_s(),
            poll_count: self.snapshots.len() as u32,
            poll_interval_ms: self.poll_interval.as_millis() as u64,
            parametros: self.params,
            omisiones,
            players: self.latest.clone(),
            blocks,
            events: self.events.clone(),
            top5_azul: gblocks.top5_azul,
            top5_rojo: gblocks.top5_rojo,
            dominion,
        };
        captura
    }


    /// Reconciles the **accumulated attributed events** with the **measured
    /// KDA totals** from the latest snapshot (`scores`).
    ///
    /// It was observed in real games (replay `LA1-1755257919` + independent
    /// live captures) that Riot's `events.Events` feed **omits some events in
    /// certain polls** — two kills from that game never appeared in any feed.
    /// If the measured scores end up summing more than the events, those
    /// windows are incomplete and that must be stated.
    /// It returns an omission only when the feed accumulates **less** than the
    /// measurement (the observed direction of the loss).
    fn reconcile_feed(&self) -> Option<String> {
        if self.latest.is_empty() || self.events.is_empty() {
            return None;
        }
        let kills_of = |evs: &[AttributedEvent], team: Team| -> u32 {
            evs.iter()
                .filter(|e| e.kind == "ChampionKill" && e.team == Some(team))
                .count() as u32
        };
        let ev_kills = [Team::Blue, Team::Red].map(|team| kills_of(&self.events, team));
        let snap_kills = [Team::Blue, Team::Red].map(|team| {
            self.latest
                .iter()
                .filter(|p| p.team == team)
                .map(|p| p.kills)
                .sum::<u32>()
        });
        let diff = snap_kills
            .iter()
            .zip(ev_kills.iter())
            .map(|(s, e)| s.saturating_sub(*e))
            .sum::<u32>();
        if diff == 0 {
            return None;
        }
        Some(format!(
            "reconciliación events.Events vs scores: los eventos acumulan \
             {}/{} kills (Azul/Rojo) y los scores medidos suman {}/{}: el feed \
             de Riot omitió {diff} evento(s); alguna ventana carece de esa kill",
            ev_kills[0], ev_kills[1], snap_kills[0], snap_kills[1]
        ))
    }

    /// Aggregates the snapshots into 30 s blocks with **measured** per-team
    /// metrics and the block's **reconstructed** gold delta.
    fn build_blocks(&self) -> Vec<ScrimBlock> {
        if self.snapshots.is_empty() {
            return Vec::new();
        }
        let dur = self.duration_s();
        let n = ((dur / BLOCK_SECS).ceil() as u32).max(1);
        let mut blocks: Vec<ScrimBlock> = Vec::with_capacity(n as usize);

        // Reconstructed model per snapshot and team.
        let model: Vec<(f32, i64, i64)> = self.gold_series();
        // Deltas of placed wards (`/stats`) per block, for the vision fallback
        // when the client does not expose `wardScore`.
        let mut wplaced: Vec<(Option<f64>, Option<f64>)> = Vec::with_capacity(n as usize);

        for k in 0..n {
            let start = k as f32 * BLOCK_SECS;
            let end = (start + BLOCK_SECS).min(dur.max(start));
            // Snapshots inside the block (left edge included).
            let in_block: Vec<&LiveSnapshot> = self
                .snapshots
                .iter()
                .filter(|s| s.t_game_s >= start && s.t_game_s < end)
                .collect();
            // First/last accumulated state around the block for deltas.
            let before = self
                .snapshots
                .iter()
                .filter(|s| s.t_game_s < start)
                .last();
            // In the **last** block the interval is truncated by the actual
            // duration: the last state (t == dur) does count, otherwise its
            // delta would be lost (kills, CS, vision and objectives at the end).
            let at_end = if end >= dur {
                self.snapshots.last()
            } else {
                self.snapshots
                    .iter()
                    .filter(|s| s.t_game_s < end)
                    .last()
                    .or_else(|| self.snapshots.last())
            };

            let mut b = ScrimBlock {
                index: k,
                start_s: start,
                end_s: end,
                ..Default::default()
            };

            // Measured CS/KDA/vision delta per team inside the block.
            if let (Some(f), Some(l)) = (before, at_end) {
                b.cs_blue_delta = team_sum(l, Team::Blue, |p| i64::from(p.cs))
                    - team_sum(f, Team::Blue, |p| i64::from(p.cs));
                b.cs_red_delta = team_sum(l, Team::Red, |p| i64::from(p.cs))
                    - team_sum(f, Team::Red, |p| i64::from(p.cs));
                b.levels_blue_sum = team_sum(l, Team::Blue, |p| i64::from(p.level)) as u32;
                b.levels_red_sum = team_sum(l, Team::Red, |p| i64::from(p.level)) as u32;
                b.item_gold_blue_delta = team_sum(l, Team::Blue, |p| p.item_gold)
                    - team_sum(f, Team::Blue, |p| p.item_gold);
                b.item_gold_red_delta = team_sum(l, Team::Red, |p| p.item_gold)
                    - team_sum(f, Team::Red, |p| p.item_gold);
                // Vision: delta of `wardScore` accumulated per team (measured).
                b.ward_blue_delta = team_sum_f(l, Team::Blue, |p| p.ward_score)
                    - team_sum_f(f, Team::Blue, |p| p.ward_score);
                b.ward_red_delta = team_sum_f(l, Team::Red, |p| p.ward_score)
                    - team_sum_f(f, Team::Red, |p| p.ward_score);
            }

            // Measured fallback: wards placed per team (`/stats`).
            let wp = match (before, at_end) {
                (Some(f), Some(l)) => (
                    team_wards_delta(f, l, Team::Blue),
                    team_wards_delta(f, l, Team::Red),
                ),
                _ => (None, None),
            };
            wplaced.push(wp);

            // Block kills/deaths/assists/turrets/objectives: attributed events.
            for ev in &self.events {
                if ev.t_game_s >= start && ev.t_game_s < end {
                    accumulate_event(&mut b, ev);
                }
            }

            // Epic objectives: delta of the **scoreboard** counters
            // (`scoreboard.dragonKills` / `baronKills` / `riftHeraldKills`).
            // The event feed does **not** report them (only `ChampionKill`,
            // `TurretKilled`…), but the scoreboard does, and its delta says who
            // took it and in which window. If the feed already counted that
            // objective, it is not added twice.
            if let (Some(f), Some(l)) = (before, at_end) {
                if let (Some(fc), Some(lc)) = (f.epics, l.epics) {
                    b.dragons_blue = b
                        .dragons_blue
                        .max(lc.dragons_blue.saturating_sub(fc.dragons_blue));
                    b.dragons_red = b
                        .dragons_red
                        .max(lc.dragons_red.saturating_sub(fc.dragons_red));
                    b.heralds_blue = b
                        .heralds_blue
                        .max(lc.heralds_blue.saturating_sub(fc.heralds_blue));
                    b.heralds_red = b
                        .heralds_red
                        .max(lc.heralds_red.saturating_sub(fc.heralds_red));
                    b.barons_blue = b
                        .barons_blue
                        .max(lc.barons_blue.saturating_sub(fc.barons_blue));
                    b.barons_red = b
                        .barons_red
                        .max(lc.barons_red.saturating_sub(fc.barons_red));
                }
            }

            // Reconstructed gold delta of the block (model).
            let m0 = model_at(&model, start);
            let m1 = model_at(&model, end);
            b.gold_blue_delta = m1.0 - m0.0;
            b.gold_red_delta = m1.1 - m0.1;
            b.net_blue = b.gold_blue_delta - b.gold_red_delta;

            let _ = in_block; // reserved for future intra-block metrics
            blocks.push(b);
        }

        // Vision source: `wardScore` if the client exposed it (some block with
        // activity); otherwise (e.g. `scores` all zero in spectator mode), the
        // measured count of placed wards. The two metrics are never mixed.
        let wardscore_any = blocks
            .iter()
            .any(|b| b.ward_blue_delta != 0.0 || b.ward_red_delta != 0.0);
        let wplaced_any = wplaced.iter().any(|(b, r)| b.is_some() || r.is_some());
        if !wardscore_any && wplaced_any {
            for (b, (wb, wr)) in blocks.iter_mut().zip(wplaced) {
                b.ward_blue_delta = wb.unwrap_or(0.0);
                b.ward_red_delta = wr.unwrap_or(0.0);
                b.vision_source = "wards_placed".to_string();
            }
        } else if wardscore_any {
            for b in blocks.iter_mut() {
                b.vision_source = "wardScore".to_string();
            }
        }
        blocks
    }
}

impl Default for LivePoller {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Repair of saved captures (recalculation without replaying the scrim)
// ---------------------------------------------------------------------------

/// Default epsilon for deduplicating the cumulative feed of a capture saved by
/// a build **prior to** deduplication by `EventID`.
///
/// Measured on `validacion_scrim.json` (7 polls): duplicate copies of the same
/// event are within ≤ 0.7 s of each other, whereas the closest real repetition
/// (same `killer`→same `victim`) is ≥ 97 s away — because respawn prevents
/// killing twice before dozens of seconds. 5 s separates both regimes with an
/// enormous margin.
pub const SAVED_EVENT_EPSILON_S: f32 = 5.0;

/// Accumulates an attributed event into the block according to its type (same
/// classification as [`LivePoller::build_blocks`]).
fn accumulate_event(b: &mut ScrimBlock, ev: &AttributedEvent) {
    if ev.kind == "ChampionKill" {
        match ev.team {
            Some(Team::Blue) => b.kills_blue += 1,
            Some(Team::Red) => b.kills_red += 1,
            None => {}
        }
        match ev.victim_team {
            Some(Team::Blue) => b.deaths_blue += 1,
            Some(Team::Red) => b.deaths_red += 1,
            None => {}
        }
        if ev.assister_count > 0 {
            // We do not attribute assists to a team: the API does not reliably
            // provide each assister's team here.
            b.assists_total += ev.assister_count as u32;
        }
    }
    // Turrets / plates / objectives: only if the event carries a team.
    let is_turret = matches!(ev.kind.as_str(), "TurretKilled" | "TurretDestroyed");
    let is_plate = ev.kind.contains("Plate");
    // The documented feed uses `DragonKill`/`BaronKill`/`HeraldKill` and some
    // client variants emit the `-ed` form: both are accepted. Also
    // `EpicMonsterKill` (epic drakes/herald/rune) and any event that carries a
    // populated `monster`.
    let is_objective = matches!(
        ev.kind.as_str(),
        "DragonKill"
            | "DragonKilled"
            | "BaronKill"
            | "BaronKilled"
            | "HeraldKill"
            | "HeraldKilled"
            | "EpicMonsterKill"
    ) || ev.monster.is_some();
    if is_turret {
        match ev.team {
            Some(Team::Blue) => b.towers_blue += 1,
            Some(Team::Red) => b.towers_red += 1,
            None => {}
        }
    }
    if is_plate {
        match ev.team {
            Some(Team::Blue) => b.plates_blue += 1,
            Some(Team::Red) => b.plates_red += 1,
            None => {}
        }
    }
    // Inhibitors: the feed **does** report them (`InhibKilled`), with their
    // killer (attributable to a team) and their instant. Dragons/heralds/barons,
    // by contrast, do not arrive through any live API path.
    if matches!(ev.kind.as_str(), "InhibKilled" | "InhibitorKilled") {
        match ev.team {
            Some(Team::Blue) => b.inhibs_blue += 1,
            Some(Team::Red) => b.inhibs_red += 1,
            None => {}
        }
    }
    if is_objective {
        match ev.team {
            Some(Team::Blue) => b.objectives_blue += 1,
            Some(Team::Red) => b.objectives_red += 1,
            None => {}
        }
        // Objective type: each one has its own gold value and buff.
        let kind = ev.kind.to_ascii_lowercase();
        let monster = ev.monster.as_deref().unwrap_or("").to_ascii_lowercase();
        let es = |patron: &str| kind.contains(patron) || monster.contains(patron);
        let (es_dragon, es_herald, es_baron) = if es("baron") {
            (false, false, true)
        } else if es("herald") || es("eye") {
            (false, true, false)
        } else if es("dragon") || es("drake") {
            (true, false, false)
        } else {
            (false, false, false)
        };
        match ev.team {
            Some(Team::Blue) => {
                if es_dragon {
                    b.dragons_blue += 1;
                }
                if es_herald {
                    b.heralds_blue += 1;
                }
                if es_baron {
                    b.barons_blue += 1;
                }
            }
            Some(Team::Red) => {
                if es_dragon {
                    b.dragons_red += 1;
                }
                if es_herald {
                    b.heralds_red += 1;
                }
                if es_baron {
                    b.barons_red += 1;
                }
            }
            None => {}
        }
    }
}

/// Deduplicates the events of a **saved capture** from a build prior to
/// deduplication by `EventID` (the `events.Events` feed is cumulative: every
/// poll re-emits all previous events).
///
/// Merges copies of the same event — same key (`kind`, `killer`, `victim`,
/// `turret`) — separated by less than `epsilon_s`, keeping the **first** one
/// (the closest to the real instant: later copies drift with the clock of the
/// later poll). See [`SAVED_EVENT_EPSILON_S`] for the measurement that
/// justifies the default value.
///
/// It is **not** needed live: [`LivePoller`] deduplicates by exact `EventID`.
pub fn dedup_saved_events(
    events: &[AttributedEvent],
    epsilon_s: f32,
) -> Vec<AttributedEvent> {
    let mut sorted: Vec<AttributedEvent> = events.to_vec();
    sorted.sort_by(|a, b| {
        a.t_game_s
            .partial_cmp(&b.t_game_s)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut out: Vec<AttributedEvent> = Vec::with_capacity(sorted.len());
    for ev in sorted {
        let dup = out.iter().any(|kept| {
            kept.kind == ev.kind
                && kept.killer == ev.killer
                && kept.victim == ev.victim
                && kept.turret == ev.turret
                && (ev.t_game_s - kept.t_game_s).abs() <= epsilon_s
        });
        if !dup {
            out.push(ev);
        }
    }
    out
}

/// Recomputes the event-derived counters (kills/deaths/assists, turrets,
/// plates, objectives) of each block from `events`, leaving the
/// snapshot-measured fields (CS, wards, levels, gold) **intact**.
///
/// Used by `--recalc` on saved captures; live blocks are already built with
/// [`LivePoller::build_blocks`].
pub fn rebuild_event_counters(blocks: &mut [ScrimBlock], events: &[AttributedEvent]) {
    for b in blocks.iter_mut() {
        b.kills_blue = 0;
        b.kills_red = 0;
        b.deaths_blue = 0;
        b.deaths_red = 0;
        b.assists_total = 0;
        b.towers_blue = 0;
        b.towers_red = 0;
        b.plates_blue = 0;
        b.plates_red = 0;
        b.objectives_blue = 0;
        b.objectives_red = 0;
    }
    for ev in events {
        if let Some(b) = blocks
            .iter_mut()
            .find(|b| ev.t_game_s >= b.start_s && ev.t_game_s < b.end_s)
        {
            accumulate_event(b, ev);
        }
    }
}

// ---------------------------------------------------------------------------
// Internal utilities
// ---------------------------------------------------------------------------

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

fn team_sum(s: &LiveSnapshot, team: Team, f: impl Fn(&PlayerSample) -> i64) -> i64 {
    s.players.iter().filter(|p| p.team == team).map(f).sum()
}

/// Sums `f64` over a team's players (for `wardScore`).
fn team_sum_f(s: &LiveSnapshot, team: Team, f: impl Fn(&PlayerSample) -> f64) -> f64 {
    s.players.iter().filter(|p| p.team == team).map(f).sum()
}

/// Placed wards of the team in a snapshot, only if **all** of its players
/// carry the value (a partial total would be an invented datum).
fn team_wards(s: &LiveSnapshot, team: Team) -> Option<u32> {
    let mut total: u32 = 0;
    let mut any = false;
    for p in s.players.iter().filter(|p| p.team == team) {
        total += p.wards_placed?;
        any = true;
    }
    any.then_some(total)
}

/// Delta of placed wards per team between two snapshots (clamped to 0: the
/// counter is cumulative and should not decrease).
fn team_wards_delta(from: &LiveSnapshot, to: &LiveSnapshot, team: Team) -> Option<f64> {
    let a = team_wards(from, team)?;
    let b = team_wards(to, team)?;
    Some((f64::from(b) - f64::from(a)).max(0.0))
}

/// Interpolated `(blue_gold, red_gold)` value of the model series at `t`.
fn model_at(model: &[(f32, i64, i64)], t: f32) -> (i64, i64) {
    if model.is_empty() {
        return (0, 0);
    }
    if t <= model[0].0 {
        return (model[0].1, model[0].2);
    }
    let last = model.len() - 1;
    if t >= model[last].0 {
        return (model[last].1, model[last].2);
    }
    let mut lo = 0usize;
    let mut hi = last;
    while hi - lo > 1 {
        let mid = (lo + hi) / 2;
        if model[mid].0 <= t {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let (t0, b0, r0) = model[lo];
    let (t1, b1, r1) = model[hi];
    if t1 <= t0 {
        return (b1, r1);
    }
    let w = (f64::from(t - t0) / f64::from(t1 - t0)).clamp(0.0, 1.0);
    let b = (b0 as f64 + (b1 as f64 - b0 as f64) * w).round() as i64;
    let r = (r0 as f64 + (r1 as f64 - r0 as f64) * w).round() as i64;
    (b, r)
}

/// Attributes a raw event to a team using the name→team map.
fn attribute_event(
    ev: &RawEvent,
    t_game_s: f32,
    name_team: &std::collections::HashMap<&str, Team>,
) -> AttributedEvent {
    let lookup = |n: &Option<String>| n.as_deref().and_then(|s| name_team.get(s).copied());
    let monster = ev
        .dragon_type
        .clone()
        .or_else(|| ev.dragon_killed.clone())
        .or_else(|| ev.baron_killed.clone())
        .or_else(|| ev.herald_killed.clone())
        .or_else(|| ev.epic_monster_kill.clone())
        .or_else(|| ev.monster_type.clone());
    AttributedEvent {
        t_game_s,
        kind: ev.event_name.clone().unwrap_or_default(),
        team: lookup(&ev.killer_name),
        killer: ev.killer_name.clone(),
        victim: ev.victim_name.clone(),
        victim_team: lookup(&ev.victim_name),
        assister_count: ev.assisters.as_ref().map(|a| a.len()).unwrap_or(0),
        turret: ev.turret_killed.clone(),
        monster,
    }
}

/// Documented omissions exported with the report (transparency).
fn omissions() -> Vec<String> {
    vec![
        "Oro por jugador: NO expuesto por el cliente; el flujo de oro se \
         reconstruye a partir de CS/kills/aseses medidos (reconstruido_por_modelo: true)."
            .to_string(),
        "XP por jugador: NO expuesto; se usa `level` como proxy medido.".to_string(),
        "Asistencias por equipo en bloques: los eventos `Assisters` no traen equipo \
         fiable, se cuentan sin atribuir (assists_total)."
            .to_string(),
        "Coordenadas de mapa: NO expuesto (position=lane; screenPosition*=píxeles de \
         pantalla). Los mapas de calor salen del parser .rofl (stream tipo 61)."
            .to_string(),
    ]
}

// ---------------------------------------------------------------------------
// Output model
// ---------------------------------------------------------------------------

/// Metrics of a 30 s block. Measured and reconstructed data are kept separate.
///
/// `#[serde(default)]` allows recalculating captures saved by old builds whose
/// schema did not yet include the wards/structures fields.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ScrimBlock {
    pub index: u32,
    pub start_s: f32,
    pub end_s: f32,
    // -- Measured (directly from the API) --
    pub cs_blue_delta: i64,
    pub cs_red_delta: i64,
    pub kills_blue: u32,
    pub kills_red: u32,
    pub deaths_blue: u32,
    pub deaths_red: u32,
    /// Total assists of the block (without team attribution; see omissions).
    pub assists_total: u32,
    /// Sum of levels at the end of the block (measured proxy for team XP).
    pub levels_blue_sum: u32,
    pub levels_red_sum: u32,
    /// Delta of **measured** gold converted into items (`items[].price`).
    pub item_gold_blue_delta: i64,
    pub item_gold_red_delta: i64,
    // -- Vision (per-team delta in the block; measured) --
    /// `wardScore` accumulated at the end of the block minus at the start, for Blue.
    pub ward_blue_delta: f64,
    pub ward_red_delta: f64,
    /// Measured source of the block's vision metric: `"wardScore"`
    /// (`scores.wardScore`) or `"wards_placed"` (`/liveclientdata/stats`,
    /// fallback when the client does not expose wardScore). Empty in old
    /// captures or when there was no vision data.
    #[serde(default)]
    pub vision_source: String,
    // -- Structures and objectives measured from attributed events --
    pub towers_blue: u32,
    pub towers_red: u32,
    pub plates_blue: u32,
    pub plates_red: u32,
    pub objectives_blue: u32,
    pub objectives_red: u32,
    /// Inhibitors destroyed in the block (`InhibKilled` event from the feed).
    pub inhibs_blue: u32,
    pub inhibs_red: u32,
    /// Epic objectives **by type** (the feed says which one it is): each type
    /// has its own gold value and temporary buff.
    pub dragons_blue: u32,
    pub dragons_red: u32,
    pub heralds_blue: u32,
    pub heralds_red: u32,
    pub barons_blue: u32,
    pub barons_red: u32,
    // -- Reconstructed (model; reconstruido_por_modelo: true) --
    pub gold_blue_delta: i64,
    pub gold_red_delta: i64,
    pub net_blue: i64,
}

/// Wards placed per team over the whole game (fallback when the timeline does
/// not include `WARD_PLACED`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VisionTotals {
    pub blue: u64,
    pub red: u64,
}

/// Complete report of the scrim captured live.
#[derive(Debug, Clone, Serialize)]
pub struct ScrimCapture {
    pub fuente: String,
    pub endpoint: String,
    /// `gameId` of the game linked by `r3dlog` (if it could be detected).
    /// Old captures do not include it: `serde(default)`.
    #[serde(default)]
    pub game_id: Option<u64>,
    /// Whole-game vision totals measured by the client (fallback when the
    /// timeline does not expose per-minute wards).
    #[serde(default)]
    pub vision_totals: Option<VisionTotals>,
    /// `true`: the gold/XP flow is a model, not a direct measurement.
    pub reconstruido_por_modelo: bool,
    pub game_mode: String,
    pub map_name: String,
    pub duration_s: f32,
    pub poll_count: u32,
    pub poll_interval_ms: u64,
    pub parametros: GoldModelParams,
    pub omisiones: Vec<String>,
    pub players: Vec<PlayerSample>,
    pub blocks: Vec<ScrimBlock>,
    pub events: Vec<AttributedEvent>,
    pub top5_azul: TeamTop5,
    pub top5_rojo: TeamTop5,
    /// Dominion score over 30 s windows: breakpoints (severities), Top 5 and
    /// vision alerts by level (`crate::score::DominionReport`).
    pub dominion: DominionReport,
}

impl ScrimCapture {
    /// Serializes to pretty JSON.
    pub fn to_json(&self) -> Result<String, String> {
        serde_json::to_string_pretty(self).map_err(|e| format!("serialize: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fixture that replicates the observed real schema: `camelCase` in
    /// `gameData`/`allPlayers`, `PascalCase` in `events.Events`.
    ///
    /// `kill_t` is the kill's historical `EventTime`: in the real feed it does
    /// **not change** between polls (or moves ~0.2 s), which is why it can be
    /// deduplicated by content.
    fn fixture(t: f64) -> String {
        fixture_kill(t, 65.0, "Bob", 1)
    }

    /// Fixture with control over the kill's `EventTime`, its victim and its `EventID`.
    fn fixture_kill(t: f64, kill_t: f64, victim: &str, event_id: u64) -> String {
        format!(
            r#"{{
              "gameData": {{"gameTime": {t}, "gameMode": "CLASSIC", "mapName": "Map11"}},
              "allPlayers": [
                {{"summonerName": "Alice#1", "riotIdGameName": "Alice",
                  "championName": "Gwen", "team": "ORDER", "position": "TOP",
                  "level": 5, "isDead": false, "respawnTimer": 0.0,
                  "scores": {{"kills": 2, "deaths": 1, "assists": 3,
                              "creepScore": 40, "wardScore": 1.5}},
                  "items": [{{"price": 400}}, {{"price": 350}}]}},
                {{"summonerName": "Bob#2", "riotIdGameName": "Bob",
                  "championName": "Jinx", "team": "CHAOS", "position": "BOTTOM",
                  "level": 4, "isDead": true, "respawnTimer": 8.0,
                  "scores": {{"kills": 1, "deaths": 2, "assists": 0,
                              "creepScore": 25, "wardScore": 0.5}},
                  "items": [{{"price": 1086}}]}}
              ],
              "events": {{"Events": [
                {{"EventID": 0, "EventName": "GameStart", "EventTime": 0.0}},
                {{"EventID": {event_id}, "EventName": "ChampionKill", "EventTime": {kill_t},
                  "KillerName": "Alice", "VictimName": "{victim}",
                  "Assisters": ["Alice"]}}
              ]}}
            }}"#
        )
    }

    #[test]
    fn parsea_esquema_real_camelcase_y_pascalcase() {
        let mut p = LivePoller::new();
        p.ingest_json(&fixture(65.0)).unwrap();
        // gameData.gameTime and gameMode (camelCase) are read.
        assert_eq!(p.duration_s(), 65.0);
        assert_eq!(p.game_mode, "CLASSIC");
        assert_eq!(p.map_name, "Map11");
        // 2 players: scores.creepScore and team mapped (ORDER→Blue, CHAOS→Red).
        assert_eq!(p.latest.len(), 2);
        let alice = p.latest.iter().find(|x| x.id == "Alice").unwrap();
        assert_eq!(alice.team, Team::Blue);
        assert_eq!(alice.cs, 40);
        assert_eq!(alice.kills, 2);
        assert_eq!(alice.level, 5);
        // item_gold = sum of prices (measured).
        assert_eq!(alice.item_gold, 750);
        let bob = p.latest.iter().find(|x| x.id == "Bob").unwrap();
        assert_eq!(bob.team, Team::Red);
        assert!(bob.is_dead);
        assert_eq!(bob.cs, 25);
    }

    #[test]
    fn eventos_dedup_y_atribucion() {
        let mut p = LivePoller::new();
        // The feed is cumulative: the 1st ingest captures GameStart and the kill.
        p.ingest_json(&fixture(65.0)).unwrap();
        assert_eq!(p.events.len(), 2); // GameStart is NOT discarded
        // 2nd ingest with the same feed → no duplication.
        p.ingest_json(&fixture(70.0)).unwrap();
        assert_eq!(p.events.len(), 2);
        // Historical event timestamp, NOT the poll clock.
        let kill = p.events.iter().find(|e| e.kind == "ChampionKill").unwrap();
        assert_eq!(kill.t_game_s, 65.0);
        // Attribution: killer Alice→Blue, victim Bob→Red.
        assert_eq!(kill.team, Some(Team::Blue));
        assert_eq!(kill.victim_team, Some(Team::Red));
        assert_eq!(kill.assister_count, 1);
    }

    #[test]
    fn el_feed_reemitido_no_duplica_kills_ni_pierde_repeticiones() {
        // **Regression**: the feed re-emits the whole history on every poll.
        // And the same player can kill the same opponent several times in the
        // game (measured: 7 times, separated by minutes). Both together must
        // not lose a real kill nor count a copy.
        let feed = |t: f64, k1: f64, k2: f64, id1: u64, id2: u64| {
            format!(
                r#"{{
                  "gameData": {{"gameTime": {t}}},
                  "allPlayers": [
                    {{"summonerName": "Alice#1", "riotIdGameName": "Alice",
                      "championName": "Gwen", "team": "ORDER", "level": 5,
                      "scores": {{"kills": 1, "deaths": 0, "assists": 0, "creepScore": 0}}}},
                    {{"summonerName": "Bob#2", "riotIdGameName": "Bob",
                      "championName": "Jinx", "team": "CHAOS", "level": 5,
                      "scores": {{"kills": 0, "deaths": 1, "assists": 0, "creepScore": 0}}}}
                  ],
                  "events": {{"Events": [
                    {{"EventID": {id1}, "EventName": "ChampionKill", "EventTime": {k1},
                      "KillerName": "Alice", "VictimName": "Bob"}},
                    {{"EventID": {id2}, "EventName": "ChampionKill", "EventTime": {k2},
                      "KillerName": "Alice", "VictimName": "Bob"}}
                  ]}}
                }}"#
            )
        };

        let mut p = LivePoller::new();
        // The game: Alice kills Bob at t=167 and again at t=378.
        p.ingest_json(&feed(167.0, 167.0, 378.0, 1, 2)).unwrap();
        assert_eq!(
            p.events.iter().filter(|e| e.kind == "ChampionKill").count(),
            2
        );
        // The feed re-emits the full history (new EventIDs): nothing else is counted.
        p.ingest_json(&feed(380.0, 167.0, 378.0, 3, 4)).unwrap();
        p.ingest_json(&feed(381.0, 167.0, 378.0, 5, 6)).unwrap();
        let kills: Vec<f32> = p
            .events
            .iter()
            .filter(|e| e.kind == "ChampionKill")
            .map(|e| e.t_game_s)
            .collect();
        assert_eq!(kills.len(), 2, "las copias no se cuentan: {kills:?}");
        assert!((kills[0] - 167.0).abs() < 0.01);
        assert!((kills[1] - 378.0).abs() < 0.01);
    }

    #[test]
    fn dos_muertes_simultaneas_y_dos_torres_se_conservan() {
        // They are not omitted either: the key includes the victim (kills), the
        // turret and the monster (objectives).
        let mut seen = std::collections::HashMap::new();
        let ev = |kind: &str,
                  killer: &str,
                  victim: &str,
                  turret: &str|
         -> EventKey {
            EventKey {
                kind: kind.to_string(),
                killer: (!killer.is_empty()).then(|| killer.to_string()),
                victim: (!victim.is_empty()).then(|| victim.to_string()),
                turret: (!turret.is_empty()).then(|| turret.to_string()),
                monster: None,
                recipient: None,
            }
        };
        let k1 = ev("ChampionKill", "Alice", "Bob", "");
        let k2 = ev("ChampionKill", "Alice", "Carol", "");
        let t1 = ev("TurretKilled", "Alice", "", "Turret_TOrder_L1_P3_111_0");
        let t2 = ev("TurretKilled", "Bob", "", "Turret_TChaos_L2_P3_222_0");
        // Two deaths at the same instant: distinct keys ⇒ both are kept.
        assert!(!es_evento_repetido(&mut seen, &k1, 100.0));
        assert!(!es_evento_repetido(&mut seen, &k2, 100.05));
        // Two turrets at the same instant (different lanes) ⇒ both.
        assert!(!es_evento_repetido(&mut seen, &t1, 100.0));
        assert!(!es_evento_repetido(&mut seen, &t2, 100.02));
        // Re-emissions of those actions are indeed discarded.
        assert!(es_evento_repetido(&mut seen, &k1, 100.1));
        assert!(es_evento_repetido(&mut seen, &k2, 99.99));
        assert!(es_evento_repetido(&mut seen, &t1, 100.0));
        assert!(es_evento_repetido(&mut seen, &t2, 100.0));
        // But the same killer>victim AGAIN (minutes later) is kept.
        assert!(!es_evento_repetido(&mut seen, &k1, 378.0));
    }

    #[test]
    fn el_feed_no_omite_ni_duplica_eventos_en_vivo() {
        // **End-to-end**: a feed with 2 simultaneous kills (different victims)
        // and 2 simultaneous turrets (different lanes) must keep all 4 events,
        // even if the feed re-emits them later with new EventIDs.
        let feed = |t: f64, id: u64| {
            format!(
                r#"{{
                  "gameData": {{"gameTime": {t}}},
                  "allPlayers": [
                    {{"summonerName": "Alice#1", "riotIdGameName": "Alice",
                      "championName": "Gwen", "team": "ORDER", "level": 5,
                      "scores": {{"kills": 2, "deaths": 0, "assists": 0, "creepScore": 0}}}},
                    {{"summonerName": "Bob#2", "riotIdGameName": "Bob",
                      "championName": "Jinx", "team": "CHAOS", "level": 5,
                      "scores": {{"kills": 0, "deaths": 1, "assists": 0, "creepScore": 0}}}},
                    {{"summonerName": "Cleo#3", "riotIdGameName": "Cleo",
                      "championName": "Ahri", "team": "CHAOS", "level": 5,
                      "scores": {{"kills": 0, "deaths": 0, "assists": 0, "creepScore": 0}}}}
                  ],
                  "events": {{"Events": [
                    {{"EventID": {id}, "EventName": "ChampionKill", "EventTime": 100.0,
                      "KillerName": "Alice", "VictimName": "Bob"}},
                    {{"EventID": {}, "EventName": "ChampionKill", "EventTime": 100.1,
                      "KillerName": "Alice", "VictimName": "Cleo"}},
                    {{"EventID": {}, "EventName": "TurretKilled", "EventTime": 100.0,
                      "KillerName": "Alice", "TurretKilled": "Turret_TOrder_L1_P3_111_0"}},
                    {{"EventID": {}, "EventName": "TurretKilled", "EventTime": 100.05,
                      "KillerName": "Alice", "TurretKilled": "Turret_TChaos_L2_P3_222_0"}}
                  ]}}
                }}"#,
                id + 1,
                id + 2,
                id + 3
            )
        };

        let mut p = LivePoller::new();
        p.ingest_json(&feed(100.0, 1)).unwrap();
        assert_eq!(p.events.len(), 4, "2 kills + 2 torres simultáneas");
        // The feed re-emits all four with new EventIDs: they are not duplicated.
        p.ingest_json(&feed(101.0, 100)).unwrap();
        p.ingest_json(&feed(102.0, 200)).unwrap();
        assert_eq!(p.events.len(), 4, "las re-emisiones no se cuentan dos veces");
        let kills = p.events.iter().filter(|e| e.kind == "ChampionKill").count();
        let torres = p.events.iter().filter(|e| e.kind == "TurretKilled").count();
        assert_eq!(kills, 2);
        assert_eq!(torres, 2);
    }

    #[test]
    fn oro_reconstruido_es_modelo_y_deltas() {
        let mut p = LivePoller::new();
        p.ingest_json(&fixture(0.0)).unwrap(); // t=0
        p.ingest_json(&fixture(30.0)).unwrap(); // t=30
        let series = p.gold_series();
        assert_eq!(series.len(), 2);
        // Non-decreasing monotonic per team.
        assert!(series[1].1 >= series[0].1);
        assert!(series[1].2 >= series[0].2);
        // Report tagged as reconstructed.
        let cap = p.build_capture();
        assert!(cap.reconstruido_por_modelo);
        assert_eq!(cap.fuente, "live_client_data_api");
        // Documented omissions present (gold/XP/coords not measured).
        assert!(cap.omisiones.len() >= 3);
    }

    #[test]
    fn los_objetivos_epicos_se_leen_del_marcador() {
        // The event feed does **not** report dragons/barons: they are read from
        // the scoreboard, whose delta says who took them and when.
        let feed = |t: f64, dragones_b: u32, dragones_r: u32| {
            format!(
                r#"{{
                  "gameData": {{"gameTime": {t}}},
                  "allPlayers": [
                    {{"summonerName": "Alice#1", "riotIdGameName": "Alice",
                      "championName": "Gwen", "team": "ORDER", "level": 5,
                      "scores": {{"kills": 0, "deaths": 0, "assists": 0, "creepScore": 0}}}}
                  ],
                  "scoreboard": {{
                    "team1": {{"dragonKills": {dragones_b}, "baronKills": 0, "riftHeraldKills": 0}},
                    "team2": {{"dragonKills": {dragones_r}, "baronKills": 0, "riftHeraldKills": 0}}
                  }},
                  "events": {{"Events": []}}
                }}"#
            )
        };

        let mut p = LivePoller::new();
        p.ingest_json(&feed(0.0, 0, 0)).unwrap();
        p.ingest_json(&feed(40.0, 1, 0)).unwrap();
        let cap = p.build_capture();
        // The dragon entered the 30–60 s block.
        assert_eq!(cap.blocks[1].dragons_blue, 1);
        assert_eq!(cap.blocks[1].dragons_red, 0);
        // And it comes out as an objective notice (not as a score alert).
        assert_eq!(cap.dominion.avisos.len(), 1);
        assert_eq!(cap.dominion.avisos[0].tipo, "dragon");
        assert_eq!(cap.dominion.avisos[0].team, Team::Blue);
        assert!((cap.dominion.avisos[0].start_s - 40.0).abs() < 1e-6);
    }

    #[test]
    fn los_inhibidores_se_leen_del_feed() {
        // The feed **does** report `InhibKilled` with its killer: it is
        // attributed to the team and comes out as an informational notice.
        let feed = |t: f64| {
            format!(
                r#"{{
                  "gameData": {{"gameTime": {t}}},
                  "allPlayers": [
                    {{"summonerName": "Alice#1", "riotIdGameName": "Alice",
                      "championName": "Gwen", "team": "ORDER", "level": 5,
                      "scores": {{"kills": 0, "deaths": 0, "assists": 0, "creepScore": 0}}}},
                    {{"summonerName": "Bob#2", "riotIdGameName": "Bob",
                      "championName": "Jinx", "team": "CHAOS", "level": 5,
                      "scores": {{"kills": 0, "deaths": 0, "assists": 0, "creepScore": 0}}}}
                  ],
                  "events": {{"Events": [
                    {{"EventID": 1, "EventName": "InhibKilled", "EventTime": {t},
                      "KillerName": "Alice", "InhibKilled": "Inhib_TChaos_L2_P1_1_0"}}
                  ]}}
                }}"#
            )
        };
        let mut p = LivePoller::new();
        p.ingest_json(&feed(0.0)).unwrap();
        p.ingest_json(&feed(40.0)).unwrap();
        let cap = p.build_capture();
        assert_eq!(cap.blocks[0].inhibs_blue, 1);
        assert_eq!(cap.blocks[0].inhibs_red, 0);
        // It comes out as an informational notice (does not add to the score).
        assert_eq!(cap.dominion.avisos.len(), 1);
        assert_eq!(cap.dominion.avisos[0].tipo, "inhib");
        assert_eq!(cap.dominion.avisos[0].team, Team::Blue);
        assert!(cap.dominion.avisos[0].dur_s == 0.0);
    }

    #[test]
    fn json_invalido_no_paniquea() {
        let mut p = LivePoller::new();
        assert!(p.ingest_json("no es json").is_err());
        // Valid JSON but no players → empty snapshot, no panic.
        p.ingest_json(r#"{"gameData":{"gameTime":10.0}}"#).unwrap();
        assert!(p.latest.is_empty());
    }

    // -- Repair of saved captures (recalculation) -----------------------------

    fn ev(t: f32, kind: &str, killer: Option<&str>, victim: Option<&str>) -> AttributedEvent {
        AttributedEvent {
            t_game_s: t,
            kind: kind.to_string(),
            team: Some(Team::Red),
            killer: killer.map(str::to_string),
            victim: victim.map(str::to_string),
            victim_team: Some(Team::Blue),
            assister_count: 0,
            turret: None,
            monster: None,
        }
    }

    #[test]
    fn dedup_saved_events_fusiona_drift_y_conserva_repeticiones_reales() {
        // Distribution measured on validacion_scrim.json: duplicate copies of
        // an event are within ≤ 0.7 s; the closest real repetition (same
        // killer→victim) is ≥ 97 s away.
        let events = vec![
            ev(166.87, "ChampionKill", Some("Garen"), Some("Gwen")), // real
            ev(166.92, "ChampionKill", Some("Garen"), Some("Gwen")), // copy (0.05 s)
            ev(377.92, "ChampionKill", Some("Garen"), Some("Gwen")), // real
            ev(941.73, "ChampionKill", Some("Garen"), Some("Gwen")), // real
            ev(941.78, "ChampionKill", Some("Garen"), Some("Gwen")), // copy (0.05 s)
            ev(941.75, "TurretKilled", Some("Garen"), None),         // different turret
        ];
        let out = dedup_saved_events(&events, SAVED_EVENT_EPSILON_S);
        // 3 real kills + 1 turret; the copies are dropped.
        assert_eq!(out.len(), 4);
        let kills: Vec<f32> = out
            .iter()
            .filter(|e| e.kind == "ChampionKill")
            .map(|e| e.t_game_s)
            .collect();
        // Keeps the FIRST copy of each event.
        assert_eq!(kills, vec![166.87, 377.92, 941.73]);
    }

    #[test]
    fn rebuild_event_counters_reconstruye_por_ventana() {
        let mk = |index: u32, start: f32| ScrimBlock {
            index,
            start_s: start,
            end_s: start + 30.0,
            // Previous "inflated" counters that must be replaced.
            kills_blue: 9,
            kills_red: 9,
            towers_blue: 9,
            ..Default::default()
        };
        let mut blocks = vec![mk(0, 0.0), mk(1, 30.0), mk(2, 60.0)];
        let events = vec![
            ev(45.5, "ChampionKill", Some("Garen"), Some("Gwen")),
            {
                let mut t = ev(70.0, "TurretKilled", Some("Garen"), None);
                t.team = Some(Team::Blue);
                t
            },
        ];
        rebuild_event_counters(&mut blocks, &events);
        // Block 1: the kill falls in [30,60) — red kills, blue dies.
        assert_eq!(blocks[1].kills_red, 1);
        assert_eq!(blocks[1].kills_blue, 0);
        assert_eq!(blocks[1].deaths_blue, 1);
        // Block 2: the turret falls in [60,90) — blue destroys it.
        assert_eq!(blocks[2].towers_blue, 1);
        assert_eq!(blocks[2].towers_red, 0);
        // Nothing is left over in the rest.
        assert_eq!(blocks[0].kills_blue, 0);
        assert_eq!(blocks[0].kills_red, 0);
    }

    #[test]
    fn nombres_reales_del_feed_cuentan_objetivos_y_torres() {
        // The documented feed emits `DragonKill`/`BaronKill`/`HeraldKill`
        // (without `-ed`); recognition must cover both forms so the objective
        // axis is never lost to a name variant.
        let mut blocks = vec![ScrimBlock {
            index: 0,
            start_s: 0.0,
            end_s: 30.0,
            ..Default::default()
        }];
        let mut dragon = ev(10.0, "DragonKill", Some("Garen"), None);
        dragon.monster = Some("Fire".to_string());
        let events = vec![
            dragon,
            ev(20.0, "BaronKill", Some("Garen"), None),
            ev(25.0, "HeraldKill", Some("Garen"), None),
            ev(28.0, "DragonKilled", Some("Garen"), None), // variant with -ed
        ];
        rebuild_event_counters(&mut blocks, &events);
        assert_eq!(blocks[0].objectives_red, 4, "todos los nombres cuentan");
        assert_eq!(blocks[0].objectives_blue, 0);
    }

    #[test]
    fn reconciliacion_detecta_eventos_perdidos_del_feed() {
        // Fixture: the measured scores sum Blue=2 / Red=1 kills, but the feed
        // only brings 1 kill (Alice→Blue kills Bob→Red). The discrepancy must
        // be emitted as an omission with both counts.
        let mut p = LivePoller::new();
        p.ingest_json(&fixture(65.0)).unwrap();
        let cap = p.build_capture();
        let rec = cap
            .omisiones
            .iter()
            .find(|o| o.starts_with("reconciliación"))
            .expect("la discrepancia feed vs scores debe documentarse");
        assert!(rec.contains("1/0"), "eventos: {rec}");
        assert!(rec.contains("2/1"), "scores: {rec}");
    }

    #[test]
    fn parsea_wards_de_stats_con_variantes_de_campo() {
        let raw = r#"{"playerStats":[
            {"championName":"Gwen","wardsPlaced":7,"wardsKilled":2},
            {"championName":"Jinx","wardPlaced":3,"wardKilled":1},
            {"championName":"Lux"}
        ]}"#;
        let parsed: RawStats = serde_json::from_str(raw).unwrap();
        let v = parsed.player_stats.unwrap();
        assert_eq!(v[0].wards_placed, Some(7));
        assert_eq!(v[0].wards_killed, Some(2));
        // Singular variant from the client.
        assert_eq!(v[1].wards_placed, Some(3));
        assert_eq!(v[1].wards_killed, Some(1));
        // No field → no data (not 0).
        assert_eq!(v[2].wards_placed, None);
    }

    /// Fixture with `wardScore` at zero: replicates the real 19/10 capture in
    /// spectator mode, where `scores` arrived empty and vision was left
    /// unmeasured.
    fn fixture_cero_vision(t: f64) -> String {
        format!(
            r#"{{
              "gameData": {{"gameTime": {t}, "gameMode": "CLASSIC", "mapName": "Map11"}},
              "allPlayers": [
                {{"summonerName": "A#1", "riotIdGameName": "A",
                  "championName": "Gwen", "team": "ORDER", "position": "TOP", "level": 3,
                  "scores": {{"kills": 0, "deaths": 0, "assists": 0,
                              "creepScore": 20, "wardScore": 0.0}}}},
                {{"summonerName": "B#2", "riotIdGameName": "B",
                  "championName": "Jinx", "team": "CHAOS", "position": "BOTTOM", "level": 3,
                  "scores": {{"kills": 0, "deaths": 0, "assists": 0,
                              "creepScore": 18, "wardScore": 0.0}}}}
              ]
            }}"#
        )
    }

    #[test]
    fn sin_wardscore_la_vision_usa_wards_colocadas() {
        let mut p = LivePoller::new();
        p.ingest_json(&fixture_cero_vision(0.0)).unwrap();
        p.apply_wards(&std::collections::HashMap::from([
            ("gwen".to_string(), (2u32, 0u32)),
            ("jinx".to_string(), (0u32, 0u32)),
        ]));
        p.ingest_json(&fixture_cero_vision(35.0)).unwrap();
        p.apply_wards(&std::collections::HashMap::from([
            ("gwen".to_string(), (5u32, 0u32)),
            ("jinx".to_string(), (1u32, 0u32)),
        ]));
        p.ingest_json(&fixture_cero_vision(50.0)).unwrap();
        p.apply_wards(&std::collections::HashMap::from([
            ("gwen".to_string(), (5u32, 0u32)),
            ("jinx".to_string(), (1u32, 0u32)),
        ]));
        let cap = p.build_capture();
        assert_eq!(cap.blocks.len(), 2);
        // wardScore stays at 0 throughout the capture → the source is /stats.
        assert_eq!(cap.blocks[0].vision_source, "wards_placed");
        // Block 1 [30,60): blue 5−2 = 3 wards, red 1−0 = 1.
        assert_eq!(cap.blocks[1].ward_blue_delta, 3.0);
        assert_eq!(cap.blocks[1].ward_red_delta, 1.0);
        assert_eq!(cap.dominion.vision.fuente, "wards_placed");
        let a = &cap.dominion.vision.alerts[0];
        // Shrunk share: (3 + 1.5) / (4 + 3) = 0.643 (without shrinkage it would
        // be 0.75) ⇒ deviation 0.14 ⇒ "slight advantage".
        let esperado = (3.0 + 1.2) / (4.0 + 2.4);
        assert!((a.cuota_azul - esperado).abs() < 1e-9, "cuota {}", a.cuota_azul);
        assert_eq!(a.severity, crate::score::Severity::VentajaMedia);
        // The omission explains the metric substitution.
        assert!(cap.omisiones.iter().any(|o| o.contains("wards colocadas")));
    }

    #[test]
    fn scrim_block_deserializa_esquema_antiguo_de_captura() {
        // JSON with the schema of old builds: no wards/turrets/plates or
        // availability flags. Missing fields must be 0.
        let old = r#"{"index":5,"start_s":150.0,"end_s":180.0,
            "cs_blue_delta":12,"cs_red_delta":30,"kills_blue":0,"kills_red":1,
            "deaths_blue":1,"deaths_red":0,"assists_total":1,
            "levels_blue_sum":20,"levels_red_sum":21,
            "item_gold_blue_delta":0,"item_gold_red_delta":0,
            "gold_blue_delta":0,"gold_red_delta":0,"net_blue":0}"#;
        let b: ScrimBlock = serde_json::from_str(old).unwrap();
        assert_eq!(b.index, 5);
        assert_eq!(b.start_s, 150.0);
        assert_eq!(b.kills_red, 1);
        assert_eq!(b.cs_red_delta, 30);
        // New fields missing → 0 (measured in the game: no data).
        assert_eq!(b.ward_blue_delta, 0.0);
        assert_eq!(b.towers_blue, 0);
        assert_eq!(b.objectives_red, 0);
        // Vision source missing in old captures → empty string.
        assert_eq!(b.vision_source, "");
    }
}
