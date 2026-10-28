//! Deserialization of the header JSON (metadata) and of the footer.
//!
//! Riot's JSON field names are `camelCase`; they are mapped to Rust fields in
//! `snake_case` with `#[serde(rename)]`. All fields are `Option` +
//! `#[serde(default)]` to tolerate changes between game versions without
//! aborting the parse.

use serde::Deserialize;

use crate::model::{
    Player, PlayerStats, ReplayMetadata, Team, TeamTotals, compute_team_totals,
};

/// A field that may come as a string or as a number in Riot's JSON.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum StringOrNum {
    Str(String),
    Num(f64),
}

impl StringOrNum {
    pub fn to_string_lossy(&self) -> String {
        match self {
            StringOrNum::Str(s) => s.clone(),
            StringOrNum::Num(n) => {
                if n.fract() == 0.0 && n.is_finite() {
                    format!("{}", *n as i64)
                } else {
                    n.to_string()
                }
            }
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct RawMetadata {
    #[serde(rename = "gameId", default)]
    pub game_id: Option<StringOrNum>,
    #[serde(rename = "gameLength", default)]
    pub game_length: Option<f64>,
    #[serde(rename = "gameVersion", default)]
    pub game_version: Option<String>,
    #[serde(rename = "gameMode", default)]
    pub game_mode: Option<String>,
    #[serde(rename = "gameType", default)]
    pub game_type: Option<String>,
    #[serde(rename = "mapId", default)]
    pub map_id: Option<u32>,
    #[serde(rename = "queueId", default)]
    pub queue_id: Option<u32>,
    #[serde(rename = "created", default)]
    pub created: Option<u64>,
    #[serde(rename = "players", default)]
    pub players: Vec<RawPlayer>,
}

#[derive(Debug, Deserialize)]
pub struct RawPlayer {
    #[serde(rename = "championId", default)]
    pub champion_id: Option<u32>,
    #[serde(rename = "summonerName", default)]
    pub summoner_name: Option<String>,
    #[serde(rename = "spell1Id", default)]
    pub spell1_id: Option<u32>,
    #[serde(rename = "spell2Id", default)]
    pub spell2_id: Option<u32>,
    #[serde(rename = "skinId", default)]
    pub skin_id: Option<u32>,
    #[serde(rename = "isBlueSide", default)]
    pub is_blue_side: Option<bool>,
    #[serde(rename = "team", default)]
    pub team: Option<i64>,
    #[serde(rename = "teamId", default)]
    pub team_id: Option<i64>,
    #[serde(default)]
    pub stats: Option<RawStats>,
}

#[derive(Debug, Deserialize)]
pub struct RawStats {
    #[serde(rename = "champLevel", default)]
    pub champ_level: Option<u32>,
    #[serde(rename = "kills", default)]
    pub kills: Option<u32>,
    #[serde(rename = "numDeaths", default)]
    pub deaths: Option<u32>,
    #[serde(rename = "assists", default)]
    pub assists: Option<u32>,
    #[serde(rename = "goldEarned", default)]
    pub gold_earned: Option<u64>,
    #[serde(rename = "goldSpent", default)]
    pub gold_spent: Option<u64>,
    #[serde(rename = "totalMinionsKilled", default)]
    pub lane_cs: Option<u32>,
    #[serde(rename = "neutralMinionsKilled", default)]
    pub jungle_cs: Option<u32>,
    #[serde(rename = "totalDamageDealtToChampions", default)]
    pub damage_to_champions: Option<u64>,
    #[serde(rename = "totalDamageTaken", default)]
    pub damage_taken: Option<u64>,
    #[serde(rename = "wardPlaced", default)]
    pub wards_placed: Option<u32>,
    #[serde(rename = "wardKilled", default)]
    pub wards_killed: Option<u32>,
    #[serde(rename = "item0", default)]
    pub item0: Option<u32>,
    #[serde(rename = "item1", default)]
    pub item1: Option<u32>,
    #[serde(rename = "item2", default)]
    pub item2: Option<u32>,
    #[serde(rename = "item3", default)]
    pub item3: Option<u32>,
    #[serde(rename = "item4", default)]
    pub item4: Option<u32>,
    #[serde(rename = "item5", default)]
    pub item5: Option<u32>,
    #[serde(rename = "item6", default)]
    pub item6: Option<u32>,
}

#[derive(Debug, Deserialize)]
pub struct RawFooter {
    #[serde(rename = "gameLength", default)]
    pub game_length: Option<f64>,
    #[serde(rename = "gameEndTimestamp", default)]
    pub game_end_timestamp: Option<f64>,
}

/// Converts the raw metadata into our lightweight structures.
pub fn convert_metadata(raw: RawMetadata) -> (ReplayMetadata, Vec<Player>) {
    let metadata = ReplayMetadata {
        source_format: "legacy".into(),
        game_id: raw
            .game_id
            .as_ref()
            .map(StringOrNum::to_string_lossy)
            .unwrap_or_default(),
        game_version: raw.game_version.unwrap_or_default(),
        game_mode: raw.game_mode.unwrap_or_default(),
        game_type: raw.game_type.unwrap_or_default(),
        map_id: raw.map_id.unwrap_or(0),
        queue_id: raw.queue_id.unwrap_or(0),
        // `gameLength` in Riot's JSON is in MILLISECONDS.
        game_length_s: raw.game_length.unwrap_or(0.0) as f32 / 1000.0,
        created_unix: raw.created.unwrap_or(0),
    };

    let mut players = Vec::<Player>::with_capacity(raw.players.len());
    for (index, rp) in raw.players.into_iter().enumerate() {
        let stats_raw = rp.stats;
        let (cs_lane, cs_jungle) = stats_raw
            .as_ref()
            .map(|s| (s.lane_cs.unwrap_or(0), s.jungle_cs.unwrap_or(0)))
            .unwrap_or((0, 0));
        let stats = PlayerStats {
            champ_level: stats_raw.as_ref().and_then(|s| s.champ_level).unwrap_or(0),
            kills: stats_raw.as_ref().and_then(|s| s.kills).unwrap_or(0),
            deaths: stats_raw.as_ref().and_then(|s| s.deaths).unwrap_or(0),
            assists: stats_raw.as_ref().and_then(|s| s.assists).unwrap_or(0),
            gold_earned: stats_raw
                .as_ref()
                .and_then(|s| s.gold_earned)
                .unwrap_or(0)
                .min(u64::from(u32::MAX)) as u32,
            gold_spent: stats_raw
                .as_ref()
                .and_then(|s| s.gold_spent)
                .unwrap_or(0)
                .min(u64::from(u32::MAX)) as u32,
            cs: cs_lane.saturating_add(cs_jungle),
            cs_lane,
            cs_jungle,
            damage_to_champions: stats_raw
                .as_ref()
                .and_then(|s| s.damage_to_champions)
                .unwrap_or(0),
            damage_taken: stats_raw.as_ref().and_then(|s| s.damage_taken).unwrap_or(0),
            wards_placed: stats_raw.as_ref().and_then(|s| s.wards_placed).unwrap_or(0),
            wards_killed: stats_raw.as_ref().and_then(|s| s.wards_killed).unwrap_or(0),
            // The legacy footer does not expose turret plates/turrets, epic
            // objectives or `LAST_TAKEDOWN_TIME`: they are left at 0 (no
            // values are invented).
            turret_plates_destroyed: 0,
            turrets_destroyed: 0,
            dragons_killed: 0,
            barons_killed: 0,
            heralds_killed: 0,
            last_takedown_s: 0.0,
            items: [
                stats_raw.as_ref().and_then(|s| s.item0).unwrap_or(0),
                stats_raw.as_ref().and_then(|s| s.item1).unwrap_or(0),
                stats_raw.as_ref().and_then(|s| s.item2).unwrap_or(0),
                stats_raw.as_ref().and_then(|s| s.item3).unwrap_or(0),
                stats_raw.as_ref().and_then(|s| s.item4).unwrap_or(0),
                stats_raw.as_ref().and_then(|s| s.item5).unwrap_or(0),
                stats_raw.as_ref().and_then(|s| s.item6).unwrap_or(0),
            ],
        };

        // Team assignment: isBlueSide > team/teamId > heuristic by order.
        let team = if rp.is_blue_side == Some(true) {
            Team::Blue
        } else if rp.is_blue_side == Some(false) {
            Team::Red
        } else if let Some(t) = rp.team.or(rp.team_id) {
            match t {
                100 => Team::Blue,
                200 => Team::Red,
                _ => if index < 5 { Team::Blue } else { Team::Red },
            }
        } else if index < 5 {
            Team::Blue
        } else {
            Team::Red
        };

        players.push(Player {
            slot: index as u8,
            summoner_name: rp.summoner_name.unwrap_or_else(|| format!(" jugador-{index}")),
            champion_id: rp.champion_id.unwrap_or(0),
            champion_name: None,
            team,
            summoner_spell_1: rp.spell1_id.unwrap_or(0),
            summoner_spell_2: rp.spell2_id.unwrap_or(0),
            skin_id: rp.skin_id.unwrap_or(0),
            won: None,
            stats,
        });
    }

    // Ensures the invariant of 10 players for frame indexing.
    if players.len() < crate::format::MAX_PLAYERS {
        let have = players.len();
        for index in have..crate::format::MAX_PLAYERS {
            players.push(Player {
                slot: index as u8,
                summoner_name: format!(" desconocido-{index}"),
                champion_id: 0,
                champion_name: None,
                team: if index < 5 { Team::Blue } else { Team::Red },
                summoner_spell_1: 0,
                summoner_spell_2: 0,
                skin_id: 0,
                won: None,
                stats: PlayerStats::default(),
            });
        }
    }

    (metadata, players)
}

/// Normalizes the footer: returns recalculated totals and the final duration.
pub fn footer_duration(raw: &RawFooter) -> f32 {
    // `gameLength` is in milliseconds (both in real legacy files and in
    // the footer of the modern format).
    raw.game_length.unwrap_or(0.0) as f32 / 1000.0
}

pub fn footer_end_timestamp(raw: &RawFooter) -> u64 {
    raw.game_end_timestamp.unwrap_or(0.0) as u64
}

/// Recomputes the aggregates (used by lib after loading the players).
pub fn totals_for(players: &[Player]) -> TeamTotals {
    compute_team_totals(players)
}
