//! RE: dumps the section signatures of a modern `.rofl` stream.
//!
//! Usage: `cargo run --release --example dump_signatures -- <match.rofl>`
//!
//! For each signature `(type, length)`, prints the number of instances, their
//! timestamps and which ones land next to a group of kills (a neutral objective
//! dying in the middle of a teamfight).

use std::collections::BTreeMap;

fn main() {
    let path = std::env::args().nth(1).expect("uso: dump_signatures <partida.rofl>");
    let bytes = std::fs::read(&path).expect("leer el .rofl");
    let mut warnings = Vec::new();
    let secs = rofl_analyzer::modern::todas_las_secciones(&bytes, &mut warnings)
        .expect("analizar el .rofl");
    println!("secciones: {}", secs.len());

    let mut by_ty: BTreeMap<u16, Vec<(usize, f32)>> = BTreeMap::new();
    for (ty, len, t) in &secs {
        by_ty.entry(*ty).or_default().push((*len, *t));
    }
    // Kill timestamps (type 166, len 5).
    let kills: Vec<f32> = secs
        .iter()
        .filter(|(ty, len, _)| *ty == 166 && *len == 5)
        .map(|(_, _, t)| *t)
        .collect();
    println!("kills detectadas: {}", kills.len());

    let mut rows: Vec<_> = by_ty.into_iter().filter(|(_, v)| v.len() <= 60).collect();
    rows.sort_by_key(|(ty, v)| (v.len(), *ty));
    for (ty, v) in rows {
        let mut lens: Vec<usize> = v.iter().map(|(l, _)| *l).collect();
        lens.sort_unstable();
        lens.dedup();
        let ts: Vec<f32> = v.iter().map(|(_, t)| *t).collect();
        let cerca: Vec<String> = ts
            .iter()
            .filter(|t| kills.iter().filter(|k| (**k - **t).abs() <= 3.0).count() >= 2)
            .map(|t| format!("{t:.1}"))
            .collect();
        println!(
            "tipo {:5} x{:3} longitudes {:?}  t=[{}]  junto a kills: {:?}",
            ty,
            v.len(),
            lens,
            ts.iter()
                .take(10)
                .map(|t| format!("{t:.1}"))
                .collect::<Vec<_>>()
                .join(", "),
            cerca
        );
    }
    if !warnings.is_empty() {
        eprintln!("avisos: {warnings:?}");
    }
}
