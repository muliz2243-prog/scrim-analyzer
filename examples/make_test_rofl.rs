//! Generates a synthetic test `.rofl` so the CLI can be exercised without a real match.
//!
//! ```powershell
//! cargo run --release --example make_test_rofl -- test.rofl
//! .\target\release\rofl-analyzer.exe test.rofl --json output.json
//! ```

use std::io::Write;

use flate2::write::ZlibEncoder;
use flate2::Compression;

const FOOTER_JSON: &str = r#"{"gameId":"999","gameLength":300500,"gameEndTimestamp":1700000300}"#;

fn dotnet_string(out: &mut Vec<u8>, s: &str) {
    out.push(s.len() as u8);
    out.extend_from_slice(s.as_bytes());
}

fn metadata_json() -> String {
    let mut players = String::new();
    for i in 0..10 {
        if i > 0 {
            players.push(',');
        }
        let blue = i < 5;
        players.push_str(&format!(
            r#"{{"championId":{},"summonerName":"Summoner{i}","spell1Id":4,"spell2Id":14,"skinId":{},"isBlueSide":{blue},"team":{},"stats":{{"champLevel":12,"kills":{},"numDeaths":{},"assists":{},"goldEarned":{},"goldSpent":{},"totalMinionsKilled":{},"neutralMinionsKilled":{},"totalDamageDealtToChampions":{},"totalDamageTaken":8000,"wardPlaced":6,"wardKilled":1,"item0":3078,"item1":3086,"item2":3111}}}}"#,
            100 + i,
            60 + i,
            if blue { 100 } else { 200 },
            i % 6,
            (i * 3) % 8,
            (i * 2) % 5,
            9_000 + i * 250,
            8_000 + i * 200,
            120 + i * 5,
            if blue { 8 } else { 0 },
            5_000 + i * 700
        ));
    }
    format!(
        r#"{{"gameId":"5555555555","gameLength":300500,"gameVersion":"14.21.620.3000","gameMode":"CLASSIC","gameType":"MATCHED_GAME","mapId":11,"queueId":420,"created":1700000000,"players":[{players}]}}"#
    )
}

fn participant(x: f32, y: f32, hp: u16, level: u8) -> Vec<u8> {
    let mut b = vec![0u8; 108];
    b[5..9].copy_from_slice(&x.to_le_bytes());
    b[9..13].copy_from_slice(&y.to_le_bytes());
    b[13..17].copy_from_slice(&0f32.to_le_bytes()); // z
    let o = 50;
    b[o..o + 2].copy_from_slice(&hp.to_le_bytes());
    b[o + 2..o + 4].copy_from_slice(&2_200u16.to_le_bytes());
    b[o + 4..o + 6].copy_from_slice(&900u16.to_le_bytes());
    b[o + 6..o + 8].copy_from_slice(&1_100u16.to_le_bytes());
    b[o + 8..o + 12].copy_from_slice(&2_000u32.to_le_bytes());
    b[o + 12] = level;
    b
}

fn main() {
    let out_path = std::env::args().nth(1).unwrap_or_else(|| "test.rofl".into());

    let mut payload = Vec::new();

    // 60 seconds of frames (1 per second), with all 10 champions moving.
    for sec in 0u32..=60 {
        payload.push(1u8); // keyframe
        payload.extend_from_slice(&(sec * 1000).to_le_bytes());
        payload.extend_from_slice(&sec.to_le_bytes());

        payload.push(3u8); // frame
        payload.extend_from_slice(&(sec * 1000).to_le_bytes());
        payload.extend_from_slice(&[0u8; 2]);
        for slot in 0..10 {
            let t = sec as f32;
            let base_x = if slot < 5 { 3_000.0 } else { 12_000.0 };
            let x = base_x + slot as f32 * 120.0 + t * 8.0;
            let y = 4_000.0 + slot as f32 * 90.0 + t * 4.0;
            let hp = if sec == 40 && slot == 2 { 0 } else { 1_800 };
            payload.extend_from_slice(&participant(x, y, hp, 12));
        }

        // A couple of vision events per second (alternating types).
        if sec % 3 == 0 {
            payload.push(4u8); // event block
            payload.extend_from_slice(&(sec * 1000).to_le_bytes());
            payload.extend_from_slice(&[0u8; 2]);
            payload.extend_from_slice(&2u16.to_le_bytes());

            payload.extend_from_slice(&5006u16.to_le_bytes()); // ward placed
            payload.extend_from_slice(&[0u8; 2]);
            dotnet_string(&mut payload, "YellowTrinket");
            payload.extend_from_slice(&(6_500.0 + sec as f32).to_le_bytes());
            payload.extend_from_slice(&(7_200.0 + sec as f32).to_le_bytes());

            payload.extend_from_slice(&5007u16.to_le_bytes()); // ward destroyed
            dotnet_string(&mut payload, "PinkTrinket");
            payload.extend_from_slice(&(3_300.0f32 + sec as f32).to_le_bytes());
            payload.extend_from_slice(&(4_400.0f32 + sec as f32).to_le_bytes());
        }
    }

    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(&payload).expect("compress");
    let compressed = encoder.finish().expect("finish");

    let meta = metadata_json().into_bytes();
    let mut file = Vec::new();
    file.extend_from_slice(b"RIOT");
    file.push(0x13);
    let footer_offset = (5 + 8 + 4 + meta.len() + 4 + 12 + compressed.len()) as u64;
    file.extend_from_slice(&footer_offset.to_le_bytes());
    file.extend_from_slice(&(meta.len() as u32).to_le_bytes());
    file.extend_from_slice(&meta);
    file.extend_from_slice(&1u32.to_le_bytes()); // 1 chunk

    file.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
    file.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    file.push(1); // zlib
    file.extend_from_slice(&[0u8; 3]);
    file.extend_from_slice(&compressed);

    file.extend_from_slice(&[0u8; 8]);
    file.extend_from_slice(&(FOOTER_JSON.len() as u32).to_le_bytes());
    file.extend_from_slice(FOOTER_JSON.as_bytes());
    file.extend_from_slice(&[0u8; 8]);

    std::fs::write(&out_path, &file).expect("write");
    println!(
        "replay sintético escrito: {out_path} ({} bytes, payload {} bytes)",
        file.len(),
        payload.len()
    );
}
