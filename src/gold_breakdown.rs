//! Net gold breakdown of each 30 s window by **category**.
//!
//! The LCU timeline gives the exact cumulative gold per team
//! (`participantFrames[].totalGold`), but **does not** say what generated it. The
//! events of the same timeline do identify the source (kills, towers, plates and
//! epic monsters). This module turns those events into estimated gold per
//! category and assigns the **remainder** of the measured gold delta to `cs`
//! (waves, jungle minions, passive gold and unaccounted assists).
//!
//! Honesty rules:
//! * The total is **never** invented: the sum of the contributions of all
//!   categories is exactly the gold delta of the block (for each team).
//! * If the event estimates exceed the measured delta, they are scaled
//!   proportionally. Thus the breakdown explains 100 % of the window without
//!   contradicting the measurement.
//! * The remaining gold is labeled `cs`, the only category that cannot be
//!   isolated with the timeline data.

use crate::lcu::{team_from_participant, LcuTimeline};
use crate::model::{GoldBlock, Team};
use crate::score::{AxisContribution, DominionBlock, SeverityThresholds};

/// Standard gold per kill (not counting bounties or shutdowns).
pub const KILL_GOLD: f64 = 300.0;
/// Assist pool of a kill (split among the assistants).
pub const ASSIST_GOLD: f64 = 150.0;
/// Outer tower.
pub const OUTER_TOWER_GOLD: f64 = 250.0;
/// Inner tower.
pub const INNER_TOWER_GOLD: f64 = 275.0;
/// Base tower / nexus.
pub const BASE_TOWER_GOLD: f64 = 300.0;
/// Each turret plate.
pub const PLATE_GOLD: f64 = 175.0;
/// Dragon (estimated team gold; the main value is the buff).
pub const DRAGON_GOLD: f64 = 500.0;
/// Rift Herald.
pub const HERALD_GOLD: f64 = 300.0;
/// Baron Nashor.
pub const BARON_GOLD: f64 = 1500.0;
/// Void larvae (Voidgrubs).
pub const VOIDGRUB_GOLD: f64 = 200.0;
/// Average estimated gold of a lane minion.
pub const MINION_GOLD: f64 = 20.0;
/// Average estimated gold of a jungle monster.
pub const JUNGLE_GOLD: f64 = 65.0;

/// Categories of the gold breakdown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoldCategory {
    Kills,
    Towers,
    Plates,
    Dragons,
    Heralds,
    Barons,
    Voidgrubs,
    /// Remainder: minions, jungle, passive gold and everything the timeline does not separate.
    Cs,
    /// Lane minions (computed from `participantFrames.minionsKilled`).
    Minions,
    /// Jungle monsters (`jungleMinionsKilled`).
    Jungle,
    /// Enemy wards killed (no temporal breakdown in the timeline).
    WardKills,
    /// Gold not attributable to events (passive, bounties, assists…).
    Extra,
}

impl GoldCategory {
    /// Axis name as consumed by the interface (`score::AxisContribution`).
    pub fn axis(&self) -> &'static str {
        match self {
            GoldCategory::Kills => "kills",
            GoldCategory::Towers => "towers",
            GoldCategory::Plates => "plates",
            GoldCategory::Dragons => "dragons",
            GoldCategory::Heralds => "heralds",
            GoldCategory::Barons => "barons",
            GoldCategory::Voidgrubs => "voidgrubs",
            GoldCategory::Cs => "cs",
            GoldCategory::Minions => "minions",
            GoldCategory::Jungle => "jungle",
            GoldCategory::WardKills => "ward_kills",
            GoldCategory::Extra => "extra",
        }
    }

    /// The seven event categories (the rest is computed separately).
    pub const EVENT_CATEGORIES: [GoldCategory; 7] = [
        GoldCategory::Kills,
        GoldCategory::Towers,
        GoldCategory::Plates,
        GoldCategory::Dragons,
        GoldCategory::Heralds,
        GoldCategory::Barons,
        GoldCategory::Voidgrubs,
    ];

    fn index(&self) -> usize {
        Self::EVENT_CATEGORIES
            .iter()
            .position(|c| c == self)
            .unwrap_or(0)
    }
}

/// Minion and jungle monster deltas of a block.
#[derive(Debug, Clone, Copy, Default)]
pub struct CreepDelta {
    pub blue_minions: f64,
    pub red_minions: f64,
    pub blue_jungle: f64,
    pub red_jungle: f64,
}

/// A timeline event translated into gold of a category.
#[derive(Debug, Clone, Copy)]
pub struct EventGold {
    pub t_s: f32,
    pub team: Team,
    pub category: GoldCategory,
    pub gold: f64,
}

/// Attributable team of the event: `participantId` first and `teamId` as fallback.
fn team_of(participant_id: i32, team_id: Option<i32>) -> Option<Team> {
    match team_from_participant(participant_id).or(team_id) {
        Some(100) => Some(Team::Blue),
        Some(200) => Some(Team::Red),
        _ => None,
    }
}

fn contains(haystack: &str, needle: &str) -> bool {
    haystack.to_ascii_uppercase().contains(needle)
}

/// Gold of a tower according to its type (`OUTER_TURRET`, `INNER_TURRET`, …).
fn tower_gold(tower_type: Option<&str>) -> f64 {
    let s = tower_type.unwrap_or("");
    if contains(s, "INNER") {
        INNER_TOWER_GOLD
    } else if contains(s, "BASE") || contains(s, "NEXUS") {
        BASE_TOWER_GOLD
    } else {
        OUTER_TOWER_GOLD
    }
}

/// Translates the timeline events into gold per category and team.
///
/// Ignores inhibitors and structures without an attributable killer: their gold
/// cannot be separated from the rest and will remain in `cs` (it is never
/// invented).
pub fn events_from_timeline(timeline: &LcuTimeline) -> Vec<EventGold> {
    let mut out: Vec<EventGold> = Vec::new();
    for frame in &timeline.frames {
        for ev in &frame.events {
            let t_s = (ev.timestamp as f32) / 1000.0;
            match ev.event_type.as_str() {
                "CHAMPION_KILL" => {
                    let Some(team) = team_of(ev.killer_id, ev.team_id) else {
                        continue;
                    };
                    out.push(EventGold {
                        t_s,
                        team,
                        category: GoldCategory::Kills,
                        gold: KILL_GOLD,
                    });
                    if !ev.assisting_participant_ids.is_empty() {
                        out.push(EventGold {
                            t_s,
                            team,
                            category: GoldCategory::Kills,
                            gold: ASSIST_GOLD,
                        });
                    }
                }
                "BUILDING_KILL" => {
                    if ev
                        .building_type
                        .as_deref()
                        .map(|b| contains(b, "INHIBITOR"))
                        .unwrap_or(false)
                    {
                        continue; // inhibitors do not pay relevant team gold
                    }
                    let Some(team) = team_of(ev.killer_id, ev.team_id) else {
                        continue;
                    };
                    out.push(EventGold {
                        t_s,
                        team,
                        category: GoldCategory::Towers,
                        gold: tower_gold(ev.tower_type.as_deref()),
                    });
                }
                "TURRET_PLATE_DESTROYED" => {
                    let Some(team) = team_of(ev.killer_id, ev.team_id) else {
                        continue;
                    };
                    out.push(EventGold {
                        t_s,
                        team,
                        category: GoldCategory::Plates,
                        gold: PLATE_GOLD,
                    });
                }
                "ELITE_MONSTER_KILL" => {
                    let Some(team) = team_of(ev.killer_id, ev.team_id) else {
                        continue;
                    };
                    let kind = format!(
                        "{} {}",
                        ev.monster_type.as_deref().unwrap_or(""),
                        ev.monster_sub_type.as_deref().unwrap_or("")
                    );
                    let (category, gold) = if contains(&kind, "BARON") {
                        (GoldCategory::Barons, BARON_GOLD)
                    } else if contains(&kind, "HERALD") {
                        (GoldCategory::Heralds, HERALD_GOLD)
                    } else if contains(&kind, "HORDE") || contains(&kind, "VOIDGRUB") {
                        (GoldCategory::Voidgrubs, VOIDGRUB_GOLD)
                    } else if contains(&kind, "DRAGON") || contains(&kind, "DRAKE") {
                        (GoldCategory::Dragons, DRAGON_GOLD)
                    } else {
                        continue; // Atakhan and monsters without a documented value
                    };
                    out.push(EventGold {
                        t_s,
                        team,
                        category,
                        gold,
                    });
                }
                _ => {}
            }
        }
    }
    out
}

/// Cumulative series `(t, blue_minions, red_minions, blue_jungle, red_jungle)`.
fn creep_series(timeline: &LcuTimeline) -> Vec<(f32, f64, f64, f64, f64)> {
    let mut out: Vec<(f32, f64, f64, f64, f64)> = timeline
        .frames
        .iter()
        .map(|f| {
            let (mut bm, mut rm, mut bj, mut rj) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
            for pf in f.participant_frames.values() {
                if pf.participant_id <= 5 {
                    bm += f64::from(pf.minions_killed);
                    bj += f64::from(pf.jungle_minions_killed);
                } else {
                    rm += f64::from(pf.minions_killed);
                    rj += f64::from(pf.jungle_minions_killed);
                }
            }
            (f.timestamp as f32 / 1000.0, bm, rm, bj, rj)
        })
        .collect();
    out.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    out
}

fn interp_creep(
    series: &[(f32, f64, f64, f64, f64)],
    t: f32,
) -> (f64, f64, f64, f64) {
    if series.is_empty() {
        return (0.0, 0.0, 0.0, 0.0);
    }
    if t <= series[0].0 {
        return (series[0].1, series[0].2, series[0].3, series[0].4);
    }
    let last = series.len() - 1;
    if t >= series[last].0 {
        return (series[last].1, series[last].2, series[last].3, series[last].4);
    }
    let mut lo = 0usize;
    let mut hi = last;
    while hi - lo > 1 {
        let mid = (lo + hi) / 2;
        if series[mid].0 <= t {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let (t0, bm0, rm0, bj0, rj0) = series[lo];
    let (t1, bm1, rm1, bj1, rj1) = series[hi];
    let w = if t1 > t0 {
        ((t - t0) / (t1 - t0)).clamp(0.0, 1.0) as f64
    } else {
        1.0
    };
    (
        bm0 + (bm1 - bm0) * w,
        rm0 + (rm1 - rm0) * w,
        bj0 + (bj1 - bj0) * w,
        rj0 + (rj1 - rj0) * w,
    )
}

/// Minion and jungle deltas per block from the timeline frames.
pub fn creep_deltas(timeline: &LcuTimeline, blocks: &[GoldBlock]) -> Vec<CreepDelta> {
    let series = creep_series(timeline);
    blocks
        .iter()
        .map(|b| {
            let a = interp_creep(&series, b.start_s);
            let z = interp_creep(&series, b.end_s);
            CreepDelta {
                blue_minions: (z.0 - a.0).max(0.0),
                red_minions: (z.1 - a.1).max(0.0),
                blue_jungle: (z.2 - a.2).max(0.0),
                red_jungle: (z.3 - a.3).max(0.0),
            }
        })
        .collect()
}

/// Scales the estimates so that they do not exceed the gold actually measured.
fn scale_to_budget(values: &mut [f64], budget: f64) {
    let sum: f64 = values.iter().sum();
    if sum > budget && sum > 0.0 {
        let k = (budget / sum).max(0.0);
        for v in values.iter_mut() {
            *v *= k;
        }
    }
}

/// Builds the dominion blocks with the per-axis breakdown from the
/// timeline, the gold blocks and the severity thresholds (in gold).
pub fn blocks_with_axes(
    events: &[EventGold],
    blocks: &[GoldBlock],
    thresholds: &SeverityThresholds,
) -> Vec<DominionBlock> {
    blocks_with_axes_full(events, &[], blocks, thresholds)
}

/// Same as [`blocks_with_axes`] but with the real minion/jungle deltas.
pub fn blocks_with_axes_full(
    events: &[EventGold],
    creep: &[CreepDelta],
    blocks: &[GoldBlock],
    thresholds: &SeverityThresholds,
) -> Vec<DominionBlock> {
    let last = blocks.len().saturating_sub(1);
    blocks
        .iter()
        .enumerate()
        .map(|(i, b)| {
            let c = creep.get(i).copied().unwrap_or_default();
            // 7 event categories + minions + jungle.
            let mut blue = [0.0f64; 9];
            let mut red = [0.0f64; 9];
            for e in events {
                let inside = e.t_s >= b.start_s
                    && (e.t_s < b.end_s || (i == last && e.t_s <= b.end_s + 0.001));
                if !inside {
                    continue;
                }
                let idx = e.category.index();
                match e.team {
                    Team::Blue => blue[idx] += e.gold,
                    Team::Red => red[idx] += e.gold,
                }
            }
            blue[7] = c.blue_minions * MINION_GOLD;
            red[7] = c.red_minions * MINION_GOLD;
            blue[8] = c.blue_jungle * JUNGLE_GOLD;
            red[8] = c.red_jungle * JUNGLE_GOLD;

            // Measured gold is the truth; the estimates are adjusted to it.
            let budget_blue = (b.gold_blue_delta as f64).max(0.0);
            let budget_red = (b.gold_red_delta as f64).max(0.0);
            scale_to_budget(&mut blue, budget_blue);
            scale_to_budget(&mut red, budget_red);

            let mut axes: Vec<AxisContribution> = Vec::new();
            let mut push = |axis: &str, blue_v: f64, red_v: f64, available: bool| {
                let net = blue_v - red_v;
                if available || net.abs() > 0.01 {
                    axes.push(AxisContribution {
                        axis: axis.to_string(),
                        available,
                        net_blue: net,
                        contribution: net,
                        blue: blue_v,
                        red: red_v,
                    });
                }
            };
            for (idx, category) in GoldCategory::EVENT_CATEGORIES.iter().enumerate() {
                push(category.axis(), blue[idx], red[idx], true);
            }
            push(
                GoldCategory::Minions.axis(),
                blue[7],
                red[7],
                c.blue_minions + c.red_minions > 0.0,
            );
            push(
                GoldCategory::Jungle.axis(),
                blue[8],
                red[8],
                c.blue_jungle + c.red_jungle > 0.0,
            );
            // No ward events in the timeline: the row is shown as not
            // available, never with a zero that looks measured.
            push(GoldCategory::WardKills.axis(), 0.0, 0.0, false);

            let used_blue: f64 = blue.iter().sum();
            let used_red: f64 = red.iter().sum();
            let extra_blue = (budget_blue - used_blue).max(0.0);
            let extra_red = (budget_red - used_red).max(0.0);
            if extra_blue + extra_red > 0.0 {
                push(
                    GoldCategory::Extra.axis(),
                    extra_blue,
                    extra_red,
                    true,
                );
            }

            let net_blue = b.net_blue as f64;
            let mut componentes = vec!["oro_real_lcu".to_string()];
            componentes.extend(axes.iter().map(|a| a.axis.clone()));

            DominionBlock {
                index: b.index,
                start_s: b.start_s,
                end_s: b.end_s,
                score_net_blue: net_blue,
                severity: thresholds.classify(net_blue.abs()),
                axes,
                componentes_disponibles: componentes,
            }
        })
        .collect()
}

/// Shortcut: extracts the events from the timeline and builds the blocks with axes.
pub fn gold_blocks_with_axes(
    timeline: &LcuTimeline,
    blocks: &[GoldBlock],
    thresholds: &SeverityThresholds,
) -> Vec<DominionBlock> {
    let events = events_from_timeline(timeline);
    let creep = creep_deltas(timeline, blocks);
    blocks_with_axes_full(&events, &creep, blocks, thresholds)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(index: u32, blue: i64, red: i64) -> GoldBlock {
        GoldBlock {
            index,
            start_s: index as f32 * 30.0,
            end_s: (index + 1) as f32 * 30.0,
            gold_blue_delta: blue,
            gold_red_delta: red,
            net_blue: blue - red,
        }
    }

    #[test]
    fn suma_de_ejes_igual_al_oro_neto() {
        let blocks = vec![block(0, 1500, 900), block(1, 800, 1200)];
        let events = vec![
            EventGold {
                t_s: 10.0,
                team: Team::Blue,
                category: GoldCategory::Kills,
                gold: KILL_GOLD,
            },
            EventGold {
                t_s: 20.0,
                team: Team::Red,
                category: GoldCategory::Towers,
                gold: OUTER_TOWER_GOLD,
            },
            EventGold {
                t_s: 40.0,
                team: Team::Blue,
                category: GoldCategory::Plates,
                gold: PLATE_GOLD,
            },
        ];
        let thresholds = SeverityThresholds::default();
        let out = blocks_with_axes(&events, &blocks, &thresholds);
        assert_eq!(out.len(), 2);
        for (d, g) in out.iter().zip(blocks.iter()) {
            let suma: f64 = d.axes.iter().map(|a| a.contribution).sum();
            assert!(
                (suma - g.net_blue as f64).abs() < 0.01,
                "bloque {}: ejes {suma} != neto {}",
                g.index,
                g.net_blue
            );
        }
        // Block 0 has a blue kill and a red tower broken down.
        let axes0: Vec<&str> = out[0].axes.iter().map(|a| a.axis.as_str()).collect();
        assert!(axes0.contains(&"kills"));
        assert!(axes0.contains(&"towers"));
        assert!(axes0.contains(&"extra"));
        // 600 net g ⇒ slight advantage with the default gold thresholds.
        assert_eq!(out[0].severity, Some(crate::score::Severity::VentajaMinima));
    }

    #[test]
    fn eventos_que_superan_el_oro_se_escalan() {
        let blocks = vec![block(0, 100, 100)];
        let events = vec![EventGold {
            t_s: 5.0,
            team: Team::Blue,
            category: GoldCategory::Barons,
            gold: BARON_GOLD,
        }];
        let out = blocks_with_axes(&events, &blocks, &SeverityThresholds::default());
        let suma: f64 = out[0].axes.iter().map(|a| a.contribution).sum();
        assert!((suma - 0.0).abs() < 0.01, "suma {suma}");
        let baron = out[0]
            .axes
            .iter()
            .find(|a| a.axis == "barons")
            .expect("barón");
        assert!((baron.contribution - 100.0).abs() < 0.01);
    }

    #[test]
    fn categoria_axis_es_la_esperada() {
        assert_eq!(GoldCategory::Kills.axis(), "kills");
        assert_eq!(GoldCategory::Voidgrubs.axis(), "voidgrubs");
        assert_eq!(GoldCategory::Cs.axis(), "cs");
    }

    #[test]
    fn traduce_eventos_reales_del_timeline() {
        // Real shape of the LCU timeline (camelCase), with a blue kill, a
        // red tower, a blue plate and a blue dragon.
        let json = r#"{
          "frames": [{
            "timestamp": 60000,
            "events": [
              {"type":"CHAMPION_KILL","timestamp":10000,"killerId":2,
               "victimId":7,"assistingParticipantIds":[3,4]},
              {"type":"BUILDING_KILL","timestamp":20000,"killerId":8,
               "teamId":200,"buildingType":"TOWER_BUILDING",
               "towerType":"INNER_TURRET"},
              {"type":"TURRET_PLATE_DESTROYED","timestamp":25000,"killerId":1,
               "teamId":100},
              {"type":"ELITE_MONSTER_KILL","timestamp":40000,"killerId":5,
               "monsterType":"DRAGON","monsterSubType":"FIRE_DRAGON"},
              {"type":"ELITE_MONSTER_KILL","timestamp":45000,"killerId":6,
               "monsterType":"HORDE"}
            ],
            "participantFrames": {}
          }]
        }"#;
        let tl: LcuTimeline = serde_json::from_str(json).unwrap();
        let evs = events_from_timeline(&tl);
        assert_eq!(evs.len(), 6, "kill + asistencia + torre + placa + dragón + larvas");
        let blue: f64 = evs
            .iter()
            .filter(|e| e.team == Team::Blue)
            .map(|e| e.gold)
            .sum();
        let red: f64 = evs
            .iter()
            .filter(|e| e.team == Team::Red)
            .map(|e| e.gold)
            .sum();
        // Blue: kill 300 + assist 150 + plate 175 + dragon 500.
        assert!((blue - 1125.0).abs() < 1e-9, "azul {blue}");
        // Red: inner tower 275 + voidgrubs 200.
        assert!((red - 475.0).abs() < 1e-9, "rojo {red}");
        assert!(evs
            .iter()
            .any(|e| e.category == GoldCategory::Voidgrubs && e.team == Team::Red));
    }
}
