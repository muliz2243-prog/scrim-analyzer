//! Forensic probe v6: parses the modern payload as a stream of SECTIONS
//! (format documented by lolrofl-rs):
//!   [u8 H][time][len][type u16][params u32][data]
//!   H & 0x80 -> time   = u8 (relative ms) if set, otherwise f32 (absolute s)
//!   H & 0x10 -> len    = u8 if set, otherwise u32
//!   H & 0x40 -> type   absent if set (reuses the previous one)
//!   H & 0x20 -> params = u8 if set, otherwise u32
//! Validates that the sections fill EVERY segment EXACTLY and histograms types.

use std::collections::HashMap;
use std::io::Read;

fn u32le(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

struct Sec {
    time: f32,
    ty: u16,
    params: u32,
    len: usize,
    off: usize, // offset within the concatenated payload
}

fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "LA1-1755257919.rofl".into());
    let b = std::fs::read(&path).expect("read");

    // ---- segments ----
    let ver_len = b[0x0E] as usize;
    let mut pos = 0x0F + ver_len;
    let mut payload: Vec<u8> = Vec::new();
    let mut seg_sizes: Vec<usize> = Vec::new(); // size of each segment in the payload
    let mut nseg = 0;
    while pos + 17 <= b.len() && nseg < 200 {
        let a = u32le(&b, pos);
        let chunk_id = u32le(&b, pos + 4);
        let ty = b[pos + 8];
        let uncomp = u32le(&b, pos + 9) as usize;
        let comp = u32le(&b, pos + 13) as usize;
        let ds = pos + 17;
        let before = payload.len();
        if comp == 0 {
            if uncomp > 0 {
                if ds + uncomp > b.len() {
                    println!("seg {a} stored fuera de límite");
                    break;
                }
                payload.extend_from_slice(&b[ds..ds + uncomp]);
                pos = ds + uncomp;
            } else {
                pos = ds;
            }
        } else {
            if ds + 4 > b.len() || &b[ds..ds + 4] != [0x28, 0xB5, 0x2F, 0xFD] {
                let mut f = None;
                let mut i = ds;
                while i + 4 < (ds + 64 * 1024).min(b.len()) {
                    if &b[i..i + 4] == [0x28, 0xB5, 0x2F, 0xFD] {
                        f = Some(i);
                        break;
                    }
                    i += 1;
                }
                match f {
                    Some(m) => {
                        pos = m - 17;
                        continue;
                    }
                    None => break,
                }
            }
            let mut d = ruzstd::decoding::StreamingDecoder::new(&b[ds..ds + comp]).unwrap();
            d.read_to_end(&mut payload).unwrap();
            pos = ds + comp;
        }
        let got = payload.len() - before;
        if got != uncomp {
            println!(
                "seg a={a} chunk={chunk_id} type={ty}: descomprimió {got}, esperaba {uncomp}"
            );
        }
        seg_sizes.push(got);
        nseg += 1;
    }
    println!("payload: {} bytes / {nseg} segmentos", payload.len());

    // ---- sections ----
    // accumulated offsets where each segment ends (to validate and reset the type)
    let mut ends: Vec<usize> = Vec::new();
    let mut acc = 0usize;
    for &s in &seg_sizes {
        acc += s;
        ends.push(acc);
    }
    println!(
        "último fin de segmento: {:#X} vs payload len {:#X} -> {}",
        ends.last().copied().unwrap_or(0),
        payload.len(),
        if ends.last() == Some(&payload.len()) {
            "COINCIDE"
        } else {
            "NO COINCIDE"
        }
    );

    let mut sections: Vec<Sec> = Vec::new();
    let mut p = 0usize;
    let mut last_ty: u16 = 0;
    let mut bad = 0usize;
    let mut last_time = 0.0f32;
    while p < payload.len() {
        if ends.contains(&p) {
            last_ty = 0;
        }
        let h = payload[p];
        let mut q = p + 1;
        let time = if h & 0x80 == 0 {
            if q + 4 > payload.len() {
                break;
            }
            let t = f32::from_le_bytes([payload[q], payload[q + 1], payload[q + 2], payload[q + 3]]);
            t
        } else {
            // relative: u8 in ms since the last section
            last_time + payload.get(q).copied().unwrap_or(0) as f32 / 1000.0
        };
        q += if h & 0x80 == 0 { 4 } else { 1 };
        let len = if h & 0x10 == 0 {
            if q + 4 > payload.len() {
                break;
            }
            u32le(&payload, q) as usize
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
        let params = if h & 0x20 == 0 {
            if q + 4 > payload.len() {
                break;
            }
            let v = u32le(&payload, q);
            q += 4;
            v
        } else {
            payload.get(q).copied().unwrap_or(0) as u32
        };
        if h & 0x20 != 0 {
            q += 1;
        }
        if q + len > payload.len() {
            println!(
                "sección @{p:#X} H={h:#04X} type={ty} len={len} se sale del payload (q={q:#X})"
            );
            bad += 1;
            break;
        }
        last_ty = ty;
        last_time = time;
        sections.push(Sec {
            time,
            ty,
            params,
            len,
            off: q,
        });
        p = q + len;
    }
    println!("secciones: {} (malformadas: {bad})", sections.len());

    // ---- histogram by type ----
    let mut hist: HashMap<u16, (usize, usize, usize, usize)> = HashMap::new(); // (n, total, min, max)
    for s in &sections {
        let e = hist.entry(s.ty).or_insert((0, 0, usize::MAX, 0));
        e.0 += 1;
        e.1 += s.len;
        e.2 = e.2.min(s.len);
        e.3 = e.3.max(s.len);
    }
    let mut rows: Vec<(u16, (usize, usize, usize, usize))> = hist.into_iter().collect();
    rows.sort_by_key(|&(_, v)| std::cmp::Reverse(v.1));
    println!("\ntipo   nº      bytes     min   max    t0      t1");
    for (ty, (n, tot, mn, mx)) in rows.iter().take(25) {
        let times: Vec<f32> = sections.iter().filter(|s| s.ty == *ty).map(|s| s.time).collect();
        println!(
            "{ty:<6} {n:<7} {tot:<9} {mn:<5} {mx:<5} {:<7.1} {:<7.1}",
            times.first().copied().unwrap_or(0.0),
            times.last().copied().unwrap_or(0.0)
        );
    }

    // ---- data dump for the largest types (first occurrences) ----
    println!("\n--- data de cabecera por tipo (2 apariciones) ---");
    let mut shown: HashMap<u16, usize> = HashMap::new();
    for s in &sections {
        let c = shown.entry(s.ty).or_insert(0);
        if *c >= 2 {
            continue;
        }
        *c += 1;
        let d = &payload[s.off..s.off + s.len.min(96)];
        let hex: String = d.iter().map(|c| format!("{c:02X} ")).collect();
        println!(
            "type={:<5} t={:<7.2} len={:<6} params={} | {}",
            s.ty,
            s.time,
            s.len,
            s.params,
            &hex[..hex.len().min(230)]
        );
    }
}
