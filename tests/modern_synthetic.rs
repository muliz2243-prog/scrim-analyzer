//! Integration test for the **modern** container (zstd + footer).
//!
//! Since the modern parser accepts "stored" segments (compressed = 0), the
//! synthetic file does not need a zstd compressor: everything is built in
//! memory and the format detection, the segment walk, the stream of sections
//! (with inherited types and short lengths) and the footer with `statsJson`
//! are validated.

use rofl_analyzer::{analyze_bytes, AnalyzeOptions, Team};

const VERSION: &str = "16.20.824.8524";

fn u32le(v: u32) -> [u8; 4] {
    v.to_le_bytes()
}

/// Modern segment header: [u32 A][u32 B][u8 type][u32 uncomp][u32 comp]
fn segment_header(a: u32, b: u32, ty: u8, uncomp: u32, comp: u32) -> Vec<u8> {
    let mut h = Vec::with_capacity(17);
    h.extend_from_slice(&u32le(a));
    h.extend_from_slice(&u32le(b));
    h.push(ty);
    h.extend_from_slice(&u32le(uncomp));
    h.extend_from_slice(&u32le(comp));
    h
}

/// Section: [u8 H][time][len][type?][params][data]
fn section(h: u8, time_s: f32, data: &[u8], ty: Option<u16>, params: u32) -> Vec<u8> {
    let mut out = vec![h];
    if h & 0x80 == 0 {
        out.extend_from_slice(&time_s.to_le_bytes());
    } else {
        // relative time in ms (the tests use it relative to the last absolute one)
        out.push((time_s * 1000.0) as u8);
    }
    if h & 0x10 == 0 {
        out.extend_from_slice(&u32le(data.len() as u32));
    } else {
        out.push(data.len() as u8);
    }
    if h & 0x40 == 0 {
        out.extend_from_slice(&ty.unwrap_or(0).to_le_bytes());
    }
    if h & 0x20 == 0 {
        out.extend_from_slice(&u32le(params));
    } else {
        out.push(params as u8);
    }
    out.extend_from_slice(data);
    out
}

fn stats_json() -> String {
    let mut players = String::new();
    for i in 0..10 {
        let team = if i < 5 { 100 } else { 200 };
        let win = if i < 5 { "Win" } else { "Fail" };
        let champ = ["Gwen", "Ahri", "Jinx", "Thresh", "LeeSin", "Zed", "Syndra", "Vayne", "Naut", "Ori"][i];
        if i > 0 {
            players.push(',');
        }
        players.push_str(&format!(
            r#"{{"SKIN":"{champ}","TEAM":{team},"WIN":"{win}","LEVEL":12,"CHAMPIONS_KILLED":{i},"NUM_DEATHS":{i2},"ASSISTS":3,"GOLD_EARNED":{g},"GOLD_SPENT":9000,"MINIONS_KILLED":150,"NEUTRAL_MINIONS_KILLED":10,"WARD_PLACED":7,"WARD_KILLED":2,"TOTAL_DAMAGE_DEALT_TO_CHAMPIONS":12345}}"#,
            i2 = (i * 2) % 7,
            g = 10_000 + i * 100,
        ));
    }
    players
}

fn build_modern_rofl() -> (Vec<u8>, usize) {
    let mut file = Vec::new();
    file.extend_from_slice(b"RIOT");
    file.extend_from_slice(&2u16.to_le_bytes()); // container version
    file.extend_from_slice(&9_876_543_210u64.to_le_bytes()); // game id
    file.push(VERSION.len() as u8); // .NET string
    file.extend_from_slice(VERSION.as_bytes());
    let seg_start = file.len();
    assert_eq!(seg_start, 0x1D);

    // ---- Segment 1 (stored): 2 sections ----
    let mut seg1 = Vec::new();
    // Section type 61, absolute time 0.0, params 0
    seg1.extend_from_slice(&section(0x01, 0.0, &[1, 2, 3, 4], Some(61), 0));
    // Section with INHERITED type (H & 0x40), absolute time 0.5
    seg1.extend_from_slice(&section(0x41, 0.5, &[9, 9, 9], None, 7));
    file.extend_from_slice(&segment_header(1, 1, 1, seg1.len() as u32, 0));
    file.extend_from_slice(&seg1);
    let payload_len = seg1.len();

    // ---- Segment 2 (stored): short lengths + relative time ----
    let mut seg2 = Vec::new();
    // len in u8 (H & 0x10), absolute time 1.0, type 1080
    seg2.extend_from_slice(&section(0x11, 1.0, &[7, 7], Some(1080), 0));
    // RELATIVE time (H & 0x80) +200 ms ⇒ 1.2 s, short len, inherited type
    seg2.extend_from_slice(&section(0xD1, 0.2, &[5], None, 0));
    file.extend_from_slice(&segment_header(2, 2, 1, seg2.len() as u32, 0));
    file.extend_from_slice(&seg2);
    let payload_len = payload_len + seg2.len();

    // ---- Footer JSON + 4-byte signature ----
    let inner = format!("[{}]", stats_json());
    let escaped = inner.replace('\\', "\\\\").replace('"', "\\\"");
    let footer = format!(
        r#"{{"gameLength":120500,"lastGameChunkId":2,"lastKeyFrameId":1,"statsJson":"{escaped}"}}"#
    );
    file.extend_from_slice(footer.as_bytes());
    file.extend_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]);
    (file, payload_len)
}

#[test]
fn parses_modern_rofl_end_to_end() {
    let (file, _payload_len) = build_modern_rofl();
    let replay = analyze_bytes(&file, AnalyzeOptions::default())
        .expect("el replay moderno sintético debe analizarse");

    // Header / container
    assert_eq!(replay.metadata.source_format, "modern");
    assert_eq!(replay.metadata.game_id, "9876543210");
    assert_eq!(replay.metadata.game_version, VERSION);
    assert_eq!(replay.segment_count, 2);
    assert_eq!(replay.frames.len(), 0); // positions not decoded yet

    // Duration from the footer (ms → s)
    assert!((replay.metadata.game_length_s - 120.5).abs() < 0.01);

    // Players from statsJson
    assert_eq!(replay.players.len(), 10);
    let p0 = &replay.players[0];
    assert_eq!(p0.champion_name.as_deref(), Some("Gwen"));
    assert_eq!(p0.team, Team::Blue);
    assert_eq!(p0.won, Some(true));
    assert_eq!(p0.stats.kills, 0);
    assert_eq!(p0.stats.gold_earned, 10_000);
    assert_eq!(p0.stats.cs, 160); // 150 lane + 10 neutral
    assert_eq!(p0.stats.wards_placed, 7);
    assert_eq!(p0.stats.champ_level, 12);

    let p9 = &replay.players[9];
    assert_eq!(p9.champion_name.as_deref(), Some("Ori"));
    assert_eq!(p9.team, Team::Red);
    assert_eq!(p9.won, Some(false));

    // Per-team totals
    let expected_blue: u64 = (0..5).map(|i| 10_000 + i * 100u64).sum();
    assert_eq!(replay.team_totals.blue.gold_earned, expected_blue);
    assert_eq!(replay.team_totals.blue.players, 5);
    assert_eq!(replay.team_totals.red.players, 5);
    assert_eq!(replay.team_totals.blue.wards_placed, 35);

    // Section stream: 4 sections, 2 types, duration 1.2 s
    let s = replay.sections.as_ref().expect("resumen de secciones");
    assert_eq!(s.total_sections, 4);
    assert!(
        (s.duration_s - 1.2).abs() < 0.01,
        "duration_s = {} (tipos: {:?})",
        s.duration_s,
        s.types.iter().map(|t| (t.type_id, t.count, t.first_time_s, t.last_time_s)).collect::<Vec<_>>()
    );
    let t61 = s.types.iter().find(|t| t.type_id == 61).unwrap();
    assert_eq!(t61.count, 2); // the 2nd section inherits the type of the 1st
    assert_eq!(t61.total_bytes, 7); // 4 + 3 bytes of data
    let t1080 = s.types.iter().find(|t| t.type_id == 1080).unwrap();
    assert_eq!(t1080.count, 2);
    assert!((t1080.last_time_s - 1.2).abs() < 0.01);

    // Must still be serializable to JSON and read back
    let json = rofl_analyzer::to_json(&replay).expect("JSON");
    let back = rofl_analyzer::from_json(&json).expect("desde JSON");
    assert_eq!(back.players.len(), 10);
    assert_eq!(back.players[0].champion_name.as_deref(), Some("Gwen"));
}

#[test]
fn modern_payload_extraction() {
    let (file, payload_len) = build_modern_rofl();
    let payload = rofl_analyzer::extract_payload_bytes(&file).expect("payload");
    assert_eq!(payload.len(), payload_len);
}

#[test]
fn detects_format_variant() {
    let (file, _) = build_modern_rofl();
    assert!(rofl_analyzer::modern::looks_modern(&file));
    // A synthetic legacy file must not be detected as modern
    let mut legacy = Vec::new();
    legacy.extend_from_slice(b"RIOT");
    legacy.push(0x13);
    legacy.extend_from_slice(&[0u8; 8]);
    legacy.extend_from_slice(&1u32.to_le_bytes());
    legacy.extend_from_slice(br#"{"gameId":"1","players":[]}"#);
    assert!(!rofl_analyzer::modern::looks_modern(&legacy));
}
