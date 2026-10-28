//! Integration test: builds a complete synthetic `.rofl` in memory
//! (header + metadata + zlib chunk with frames and events + footer) and validates
//! the end-to-end pipeline without needing a real replay.

use std::io::Write;

use flate2::write::ZlibEncoder;
use flate2::Compression;
use rofl_analyzer::{analyze_bytes, AnalyzeOptions, EventKind, RoflError, Team, WardType};

const FOOTER_JSON: &str =
    r#"{"gameId":"999","gameLength":120500,"gameEndTimestamp":1700000120}"#;

fn metadata_json() -> String {
    let mut players = String::new();
    for i in 0..10 {
        let team = if i < 5 { 100 } else { 200 };
        let blue = i < 5;
        if i > 0 {
            players.push(',');
        }
        players.push_str(&format!(
            r#"{{"championId":{},"summonerName":"Player{i}","spell1Id":4,"spell2Id":14,"skinId":{},"isBlueSide":{blue},"team":{team},"stats":{{"champLevel":11,"kills":{i},"numDeaths":{},"assists":3,"goldEarned":{},"goldSpent":9000,"totalMinionsKilled":150,"neutralMinionsKilled":{},"totalDamageDealtToChampions":12345,"totalDamageTaken":9999,"wardPlaced":5,"wardKilled":1,"item0":3078,"item1":3086}}}}"#,
            100 + i,
            60 + i,
            (i * 2) % 10,
            10_000 + i * 100,
            if blue { 10 } else { 0 }
        ));
    }
    format!(
        r#"{{"gameId":"4242424242","gameLength":120500,"gameVersion":"14.20.612.3948","gameMode":"CLASSIC","gameType":"MATCHED_GAME","mapId":11,"queueId":420,"created":1700000000,"players":[{players}]}}"#
    )
}

/// String with the "7-bit encoded int" encoding used by .NET.
fn write_dotnet_string(out: &mut Vec<u8>, s: &str) {
    debug_assert!(s.len() < 128);
    out.push(s.len() as u8);
    out.extend_from_slice(s.as_bytes());
}

/// 108-byte block per participant with position/health filled in.
fn participant_block(x: f32, y: f32, health: u16, level: u8) -> Vec<u8> {
    let mut b = vec![0u8; 108];
    // 5 unknown bytes + x,y,z (f32 LE) at offset 5
    b[5..9].copy_from_slice(&x.to_le_bytes());
    b[9..13].copy_from_slice(&y.to_le_bytes());
    b[13..17].copy_from_slice(&0.0f32.to_le_bytes()); // z
    // After 33 unknown bytes: health, max_health, mana, max_mana, xp, level
    let o = 17 + 33;
    b[o..o + 2].copy_from_slice(&health.to_le_bytes());
    b[o + 2..o + 4].copy_from_slice(&2000u16.to_le_bytes());
    b[o + 4..o + 6].copy_from_slice(&800u16.to_le_bytes());
    b[o + 6..o + 8].copy_from_slice(&1000u16.to_le_bytes());
    b[o + 8..o + 12].copy_from_slice(&1234u32.to_le_bytes()); // xp
    b[o + 12] = level;
    b
}

fn frame(ts: u32, x0: f32) -> Vec<u8> {
    let mut out = vec![3u8];
    out.extend_from_slice(&ts.to_le_bytes());
    out.extend_from_slice(&[0u8; 2]); // frame header
    for slot in 0..10 {
        let alive = slot != 4; // slot 4 is "dead"
        out.extend_from_slice(&participant_block(
            x0 + slot as f32 * 100.0,
            5_000.0,
            if alive { 1500 } else { 0 },
            11,
        ));
    }
    out
}

fn event_block() -> Vec<u8> {
    let mut out = vec![4u8];
    out.extend_from_slice(&2_000u32.to_le_bytes()); // timestamp
    out.extend_from_slice(&[0u8; 2]); // block header
    out.extend_from_slice(&2u16.to_le_bytes()); // 2 events

    // WardPlaced (5006): [u8;2] + string + x + y
    out.extend_from_slice(&5006u16.to_le_bytes());
    out.extend_from_slice(&[0u8; 2]);
    write_dotnet_string(&mut out, "YellowTrinket");
    out.extend_from_slice(&6_500f32.to_le_bytes());
    out.extend_from_slice(&7_200f32.to_le_bytes());

    // WardKill (5007): string + x + y
    out.extend_from_slice(&5007u16.to_le_bytes());
    write_dotnet_string(&mut out, "PinkTrinket");
    out.extend_from_slice(&3_300f32.to_le_bytes());
    out.extend_from_slice(&4_400f32.to_le_bytes());
    out
}

fn build_synthetic_rofl() -> Vec<u8> {
    // Decompressed payload: keyframe + 2 frames + event block
    let mut payload = Vec::new();
    payload.extend_from_slice(&[1u8]); // keyframe
    payload.extend_from_slice(&0u32.to_le_bytes());
    payload.extend_from_slice(&0u32.to_le_bytes()); // keyframe number
    payload.extend_from_slice(&frame(0, 1_000.0));
    payload.extend_from_slice(&frame(1_000, 1_100.0));
    payload.extend_from_slice(&event_block());

    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(&payload).unwrap();
    let compressed = encoder.finish().unwrap();

    let meta = metadata_json().into_bytes();

    let mut file = Vec::new();
    file.extend_from_slice(b"RIOT");
    file.push(0x13); // header_length (informational)
    let footer_offset = (5 + 8 + 4 + meta.len() + 4 + 12 + compressed.len()) as u64;
    file.extend_from_slice(&footer_offset.to_le_bytes());
    file.extend_from_slice(&(meta.len() as u32).to_le_bytes());
    file.extend_from_slice(&meta);
    file.extend_from_slice(&1u32.to_le_bytes()); // 1 chunk

    // Chunk header
    file.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
    file.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    file.push(1); // zlib
    file.extend_from_slice(&[0u8; 3]);
    file.extend_from_slice(&compressed);

    debug_assert_eq!(file.len() as u64, footer_offset);

    // Footer
    file.extend_from_slice(&[0u8; 8]);
    file.extend_from_slice(&(FOOTER_JSON.len() as u32).to_le_bytes());
    file.extend_from_slice(FOOTER_JSON.as_bytes());
    file.extend_from_slice(&[0u8; 8]);
    file
}

#[test]
fn parses_synthetic_rofl_end_to_end() {
    let file = build_synthetic_rofl();
    let replay = analyze_bytes(&file, AnalyzeOptions { sample_ms: 500 })
        .expect("el replay sintético debe analizarse");

    // Header / metadata
    assert_eq!(replay.metadata.game_id, "4242424242");
    assert_eq!(replay.metadata.game_version, "14.20.612.3948");
    assert_eq!(replay.metadata.game_mode, "CLASSIC");
    assert_eq!(replay.metadata.map_id, 11);
    assert_eq!(replay.players.len(), 10);

    // Teams: 5 blue + 5 red
    assert_eq!(
        replay.players.iter().filter(|p| p.team == Team::Blue).count(),
        5
    );
    assert_eq!(
        replay.players.iter().filter(|p| p.team == Team::Red).count(),
        5
    );
    assert_eq!(replay.players[0].summoner_name, "Player0");
    assert_eq!(replay.players[9].team, Team::Red);

    // Individual and per-team total gold / CS
    assert_eq!(replay.players[0].stats.gold_earned, 10_000);
    assert_eq!(replay.players[0].stats.cs, 160); // 150 lane + 10 neutral
    assert_eq!(replay.team_totals.blue.gold_earned, 10_000 + 10_100 + 10_200 + 10_300 + 10_400);
    assert_eq!(replay.team_totals.blue.players, 5);
    assert_eq!(replay.team_totals.red.players, 5);

    // Sampled frames: ts=0 and ts=1000 (500 ms sampling)
    assert_eq!(replay.frames.len(), 2);
    let f0 = &replay.frames[0];
    assert_eq!(f0.timestamp_ms, 0);
    assert_eq!(f0.participants.len(), 10);
    assert_eq!(f0.participants[0].x, 1_000.0);
    assert_eq!(f0.participants[9].x, 1_900.0);
    assert!(f0.participants[0].alive);
    assert!(!f0.participants[4].alive);
    assert_eq!(f0.participants[0].level, 11);
    assert_eq!(replay.frames[1].participants[0].x, 1_100.0);

    // Vision events
    assert_eq!(replay.events.len(), 2);
    match &replay.events[0].kind {
        EventKind::WardPlaced { ward_type, x, y } => {
            assert_eq!(*ward_type, WardType::Stealth);
            assert_eq!(*x, 6_500.0);
            assert_eq!(*y, 7_200.0);
        }
        other => panic!("evento inesperado: {other:?}"),
    }
    assert_eq!(replay.events[0].timestamp_ms, 2_000);
    match &replay.events[1].kind {
        EventKind::WardKill { ward_type, .. } => assert_eq!(*ward_type, WardType::Control),
        other => panic!("evento inesperado: {other:?}"),
    }

    // Duration taken from the footer
    assert_eq!(replay.footer.as_ref().unwrap().game_length_s, 120.5);

    // The result must serialize to JSON without errors
    let json = rofl_analyzer::to_json(&replay).expect("serialización JSON");
    assert!(json.contains("\"game_version\": \"14.20.612.3948\""));
    let back = rofl_analyzer::from_json(&json).expect("deserialización JSON");
    assert_eq!(back.players.len(), replay.players.len());
}

#[test]
fn rejects_invalid_magic() {
    let err = analyze_bytes(b"NOPE_not_a_rofl", AnalyzeOptions::default()).unwrap_err();
    assert!(matches!(err, RoflError::InvalidMagic { .. }), "{err:?}");
}

#[test]
fn rejects_truncated_file() {
    let mut file = build_synthetic_rofl();
    file.truncate(40); // cut in the middle of the metadata
    assert!(analyze_bytes(&file, AnalyzeOptions::default()).is_err());
}

#[test]
fn ward_type_normalization() {
    assert_eq!(WardType::from_raw("YellowTrinket"), WardType::Stealth);
    assert_eq!(WardType::from_raw("PinkTrinket"), WardType::Control);
    assert_eq!(WardType::from_raw("BlueTrinket"), WardType::Blue);
    assert_eq!(WardType::from_raw("ZombieWard"), WardType::Zombie);
    assert_eq!(
        WardType::from_raw("Raro"),
        WardType::Other("Raro".to_string())
    );
}
