//! Diagnostics: dumps the sections of a `type_id` from a modern `.rofl`.
//!
//! ```text
//! cargo run --example dump_section -- "path.rofl" 1063
//! ```

use rofl_analyzer::modern::section_records;

fn main() -> Result<(), String> {
    let path = std::env::args().nth(1).ok_or("uso: dump_section <ruta.rofl> <type_id>")?;
    let ty: u16 = std::env::args()
        .nth(2)
        .ok_or("falta type_id")?
        .parse()
        .map_err(|e| format!("type_id inválido: {e}"))?;
    let bytes = std::fs::read(&path).map_err(|e| format!("leer {path}: {e}"))?;
    let recs = section_records(&bytes, ty).map_err(|e| e.to_string())?;
    println!("tipo {ty}: {} registros", recs.len());
    for (i, (t, len, data)) in recs.iter().take(40).enumerate() {
        let hex: Vec<String> = data.iter().map(|b| format!("{b:02x}")).collect();
        println!(
            "#{i:03} t={t:.3}s len={len} bytes=[{}]",
            hex.join(" ")
        );
    }
    Ok(())
}
