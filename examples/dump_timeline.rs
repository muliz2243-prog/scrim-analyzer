//! LCU timeline diagnostics.
//!
//! Usage:
//! ```text
//! cargo run --example dump_timeline -- 1632883165
//! ```
//!
//! Prints the event type counts and dumps the keys of the vision events
//! (`WARD_PLACED` / `WARD_KILL`) to check what the client of a specific match
//! actually exposes.

use rofl_analyzer::lcu::LcuClient;
use serde_json::Value;
use std::collections::BTreeMap;

fn main() -> Result<(), String> {
    let id: u64 = std::env::args()
        .nth(1)
        .ok_or("uso: dump_timeline <gameId>")?
        .parse()
        .map_err(|e| format!("gameId inválido: {e}"))?;

    let client = LcuClient::connect()?;
    let raw = client.get_json(&format!("/lol-match-history/v1/game-timelines/{id}"))?;
    println!("raw len={} head={}", raw.len(), &raw[..raw.len().min(500)]);
    let v: Value = serde_json::from_str(&raw).map_err(|e| format!("JSON inválido: {e}"))?;

    let frames = v
        .get("frames")
        .and_then(Value::as_array)
        .ok_or("el timeline no tiene frames")?;
    println!("gameId {id}: {} frames", frames.len());

    let mut counts: BTreeMap<String, u64> = BTreeMap::new();
    let mut wards: Vec<&Value> = Vec::new();
    for frame in frames {
        let Some(events) = frame.get("events").and_then(Value::as_array) else {
            continue;
        };
        for ev in events {
            let kind = ev
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("?")
                .to_string();
            *counts.entry(kind.clone()).or_insert(0) += 1;
            if kind.contains("WARD") {
                wards.push(ev);
            }
        }
    }

    println!("\n-- eventos por tipo --");
    for (k, n) in &counts {
        println!("{n:5}  {k}");
    }

    // What fields does a participantFrame really carry? (in case there are wards/minute)
    if let Some(frame) = frames.first() {
        if let Some(pf) = frame.get("participantFrames").and_then(Value::as_object) {
            if let Some((_, first)) = pf.iter().next() {
                println!("\n-- claves de participantFrame --");
                if let Some(obj) = first.as_object() {
                    let mut keys: Vec<&String> = obj.keys().collect();
                    keys.sort();
                    println!("{}", keys.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", "));
                }
            }
        }
    }

    println!("\n-- eventos de visión ({} ) --", wards.len());
    for ev in wards.iter().take(12) {
        println!(
            "{}",
            serde_json::to_string(ev).unwrap_or_else(|_| "<no serializable>".into())
        );
    }

    // Keys present in the vision events (to detect a missing `position`).
    let mut ward_keys: BTreeMap<String, u64> = BTreeMap::new();
    for ev in &wards {
        if let Some(obj) = ev.as_object() {
            for k in obj.keys() {
                *ward_keys.entry(k.clone()).or_insert(0) += 1;
            }
        }
    }
    println!("\n-- claves en eventos de visión --");
    for (k, n) in &ward_keys {
        println!("{n:5}  {k}");
    }

    // Match summary data: vision totals per player (Riot's timeline
    // does not always include WARD_PLACED).
    if let Ok(game) = client.get_game(id) {
        println!("\n-- resumen de partida (stats de visión) --");
        if let Some(parts) = game
            .get("participants")
            .and_then(Value::as_array)
        {
            let mut blue = 0u64;
            let mut red = 0u64;
            for p in parts {
                let team = p.get("teamId").and_then(Value::as_u64).unwrap_or(0);
                let stats = p.get("stats");
                let placed = stats
                    .and_then(|s| s.get("wardsPlaced"))
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                let killed = stats
                    .and_then(|s| s.get("wardsKilled"))
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                if team == 100 {
                    blue += placed;
                } else if team == 200 {
                    red += placed;
                }
                println!(
                    "team {team} champ {} wardsPlaced={placed} wardsKilled={killed}",
                    p.get("championId").and_then(Value::as_u64).unwrap_or(0)
                );
            }
            println!("TOTAL wards colocadas: azul {blue} · rojo {red}");
            if let Some(p0) = parts.first() {
                if let Some(pt) = p0.get("participantTimeline").and_then(Value::as_object) {
                    let mut keys: Vec<&String> = pt.keys().collect();
                    keys.sort();
                    println!(
                        "participantTimeline keys: {}",
                        keys.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
                    );
                }
                if let Some(st) = p0.get("stats").and_then(Value::as_object) {
                    let mut keys: Vec<&String> = st.keys().collect();
                    keys.sort();
                    println!(
                        "stats keys (vision/ward): {}",
                        keys.iter()
                            .filter(|k| k.to_lowercase().contains("vision") || k.to_lowercase().contains("ward"))
                            .map(|s| s.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                }
            }
        } else {
            println!(
                "sin `participants`; claves: {:?}",
                game.as_object().map(|o| o.keys().collect::<Vec<_>>())
            );
        }
    }

    Ok(())
}
