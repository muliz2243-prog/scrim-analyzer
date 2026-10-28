//! RE probe: do the bytes of event records encode identity?
//!
//! Histogram every byte position of the kill records (type 166, 5 B),
//! assist records (131, 3 B) and turret records (850, 5 B), and contrast them
//! against the ground truths from the `statsJson` of the reference replay:
//!
//! * deaths per slot  [9,4,5,6,8,1,6,6,3,3]  (potential victim)
//! * kills per slot    [0,3,11,2,3,15,6,4,6,1] (potential killer)
//! * entity tags of the 10 champions from stream 61
//!   [0x56,0x76,0x96,0xB6,0xD6,0xF6,0x19,0x39,0x59,0x79]
//!
//! Validation criterion (the same one used across the project): a hypothesis
//! only counts if the counts per value match a truth array **exactly**.

use std::collections::HashMap;
use std::io::Read;

fn u32le(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

struct Sec {
    time: f32,
    ty: u16,
    len: usize,
    off: usize,
}

/// Parses the modern payload (same code as examples/probe.rs).
fn parse_payload(path: &str) -> Vec<u8> {
    let b = std::fs::read(path).expect("read");
    let ver_len = b[0x0E] as usize;
    let mut pos = 0x0F + ver_len;
    let mut payload: Vec<u8> = Vec::new();
    let mut nseg = 0;
    while pos + 17 <= b.len() && nseg < 200 {
        let a = u32le(&b, pos);
        let uncomp = u32le(&b, pos + 9) as usize;
        let comp = u32le(&b, pos + 13) as usize;
        let ds = pos + 17;
        let _ = a;
        if comp == 0 {
            if uncomp > 0 {
                if ds + uncomp > b.len() {
                    break;
                }
                payload.extend_from_slice(&b[ds..ds + uncomp]);
                pos = ds + uncomp;
            } else {
                pos = ds;
            }
        } else {
            if ds + 4 > b.len() || &b[ds..ds + 4] != [0x28, 0xB5, 0x2F, 0xFD] {
                break;
            }
            let mut d = ruzstd::decoding::StreamingDecoder::new(&b[ds..ds + comp]).unwrap();
            d.read_to_end(&mut payload).unwrap();
            pos = ds + comp;
        }
        nseg += 1;
    }
    payload
}

fn sections(payload: &[u8]) -> Vec<Sec> {
    let mut out = Vec::new();
    let mut p = 0usize;
    let mut last_ty: u16 = 0;
    let mut last_time = 0.0f32;
    while p < payload.len() {
        let h = payload[p];
        let mut q = p + 1;
        let time = if h & 0x80 == 0 {
            if q + 4 > payload.len() {
                break;
            }
            f32::from_le_bytes([payload[q], payload[q + 1], payload[q + 2], payload[q + 3]])
        } else {
            last_time + payload.get(q).copied().unwrap_or(0) as f32 / 1000.0
        };
        q += if h & 0x80 == 0 { 4 } else { 1 };
        let len = if h & 0x10 == 0 {
            if q + 4 > payload.len() {
                break;
            }
            u32le(payload, q) as usize
        } else {
            payload.get(q).copied().unwrap_or(0) as usize
        };
        q += if h & 0x10 == 0 { 4 } else { 1 };
        let ty = if h & 0x40 == 0 {
            if q + 2 > payload.len() {
                break;
            }
            let t = u16::from_le_bytes([payload[q], payload[q + 1]]);
            q += 2;
            t
        } else {
            last_ty
        };
        let params_len = if h & 0x20 == 0 { 4 } else { 1 };
        q += params_len;
        if q + len > payload.len() {
            break;
        }
        last_ty = ty;
        last_time = time;
        out.push(Sec { time, ty, len, off: q });
        p = q + len;
    }
    out
}

/// Does `counts per value` reproduce `truth` exactly (as a multiset)?
fn matches_exactly(counts: &HashMap<u8, u32>, truth: &[u32]) -> bool {
    let mut a: Vec<u32> = counts.values().copied().collect();
    let mut b = truth.to_vec();
    a.sort_unstable();
    b.sort_unstable();
    a == b
}

fn hist_bytes(records: &[Vec<u8>]) -> Vec<HashMap<u8, u32>> {
    let mut hs: Vec<HashMap<u8, u32>> = vec![HashMap::new(); records[0].len()];
    for r in records {
        for (i, &byte) in r.iter().enumerate() {
            *hs[i].entry(byte).or_insert(0) += 1;
        }
    }
    hs
}

fn show(label: &str, h: &HashMap<u8, u32>, truth: &[&str], arrays: &[(&str, &[u32])]) {
    let mut v: Vec<(u8, u32)> = h.iter().map(|(&k, &c)| (k, c)).collect();
    v.sort_unstable();
    let vals: Vec<String> = v.iter().map(|(k, c)| format!("{k}:{c}")).collect();
    let exact: Vec<&str> = arrays
        .iter()
        .filter(|(_, arr)| matches_exactly(h, arr))
        .map(|(n, _)| *n)
        .collect();
    println!(
        "  {label} ({} valores distintos) ⇒ {}{}",
        v.len(),
        vals.join(" "),
        if exact.is_empty() {
            String::new()
        } else {
            format!("  ✅ EXACTO: {}", exact.join(", "))
        }
    );
    let _ = truth;
}

fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "LA1-1755257919.rofl".into());
    let payload = parse_payload(&path);
    let secs = sections(&payload);
    println!("payload {} B, {} secciones", payload.len(), secs.len());

    // Ground truths (statsJson of the reference replay, slots 0..9).
    let deaths: [u32; 10] = [9, 4, 5, 6, 8, 1, 6, 6, 3, 3];
    let kills: [u32; 10] = [0, 3, 11, 2, 3, 15, 6, 4, 6, 1];
    let assists: [u32; 10] = [0, 5, 2, 2, 5, 2, 6, 8, 5, 18];
    // Totals "how many events there are" (global validation).
    let truth_total = [("kills", 51u32), ("assists", 53), ("placas", 58), ("torres", 6)];

    let collect = |ty: u16, len_ok: &dyn Fn(usize) -> bool| -> Vec<Vec<u8>> {
        secs.iter()
            .filter(|s| s.ty == ty && len_ok(s.len))
            .map(|s| payload[s.off..s.off + s.len].to_vec())
            .collect()
    };

    // Kill records: exact length 5.
    let kill_recs = collect(166, &|l| l == 5);
    println!(
        "\n== tipo 166 (kill, 5 B): {} registros (truth: {}) ==",
        kill_recs.len(),
        truth_total[0].1
    );
    println!("primeros 8 registros (hex):");
    for r in kill_recs.iter().take(8) {
        let hex: String = r.iter().map(|c| format!("{c:02X} ")).collect();
        println!("  {hex}  t={:.1}", secs.iter().find(|s| s.ty == 166 && payload[s.off..s.off + s.len] == **r).map(|s| s.time).unwrap_or(0.0));
    }
    let kh = hist_bytes(&kill_recs);
    let truths: [(&str, &[u32]); 3] = [
        ("muertes", &deaths),
        ("kills", &kills),
        ("asistencias", &assists),
    ];
    for (i, h) in kh.iter().enumerate() {
        show(&format!("byte[{i}]"), h, &[], &truths);
    }

    // Assists: 3 B.
    let assist_recs = collect(131, &|l| l == 3);
    println!(
        "\n== tipo 131 (asistencia, 3 B): {} registros (truth: {}) ==",
        assist_recs.len(),
        truth_total[1].1
    );
    for r in assist_recs.iter().take(5) {
        let hex: String = r.iter().map(|c| format!("{c:02X} ")).collect();
        println!("  {hex}");
    }
    let ah = hist_bytes(&assist_recs);
    for (i, h) in ah.iter().enumerate() {
        show(&format!("byte[{i}]"), h, &[], &truths);
    }

    // Turrets: 5 B (view 850).
    let turret_recs = collect(850, &|l| l == 5);
    println!(
        "\n== tipo 850 (torre, 5 B): {} registros (truth: {}) ==",
        turret_recs.len(),
        truth_total[3].1
    );
    for r in turret_recs.iter() {
        let hex: String = r.iter().map(|c| format!("{c:02X} ")).collect();
        println!("  {hex}");
    }

    // Plates: 1–2 B.
    let plate_recs = secs
        .iter()
        .filter(|s| s.ty == 1183 && (1..=2).contains(&s.len))
        .map(|s| payload[s.off..s.off + s.len].to_vec())
        .collect::<Vec<_>>();
    println!(
        "\n== tipo 1183 (placa): {} registros (truth: {}) ==",
        plate_recs.len(),
        truth_total[2].1
    );
    let ph = hist_bytes(&plate_recs);
    for (i, h) in ph.iter().enumerate() {
        show(&format!("byte[{i}]"), h, &[], &truths);
    }

    // Entity tags of the 10 champions (stream 61): do they show up in any byte?
    let tags: [u8; 10] = [0x56, 0x76, 0x96, 0xB6, 0xD6, 0xF6, 0x19, 0x39, 0x59, 0x79];
    println!("\n== ¿tags de campeón en algún byte de kill? ==");
    let mut found = false;
    for (i, h) in kh.iter().enumerate() {
        let hits = tags.iter().filter(|t| h.contains_key(t)).count();
        if hits > 0 {
            println!("  byte[{i}]: {hits}/10 tags de campeón presentes ✅");
            found = true;
        }
    }
    if !found {
        println!("  ningún byte contiene tags de campeón");
    }
}
