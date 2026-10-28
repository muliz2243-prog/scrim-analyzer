//! Probe v98 — PHASE 1: dump the PE sections of League of Legends.exe.
//! Mechanical step (no RE): parses the PE header and extracts .text/.data/.rdata
//! to disk, just like the text.bin/data.bin/rdata.bin used by ROFL-master.
//! It also prints the build timestamp and the ImageBase.
use std::fs;

fn u16le(b: &[u8], o: usize) -> u16 { u16::from_le_bytes([b[o], b[o+1]]) }
fn u32le(b: &[u8], o: usize) -> u32 { u32::from_le_bytes([b[o], b[o+1], b[o+2], b[o+3]]) }
fn u64le(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes([b[o],b[o+1],b[o+2],b[o+3],b[o+4],b[o+5],b[o+6],b[o+7]])
}

fn main() {
    let path = std::env::args().nth(1).unwrap_or_else(
        || r"C:\Riot Games\League of Legends\Game\League of Legends.exe".into());
    let b = fs::read(&path).expect("no se pudo leer el binario");
    println!("binario: {} ({} bytes)", path, b.len());

    if &b[0..2] != b"MZ" { println!("no es PE (falta MZ)"); return; }
    let e_lfanew = u32le(&b, 0x3C) as usize;
    if &b[e_lfanew..e_lfanew+4] != b"PE\0\0" { println!("firma PE no encontrada"); return; }

    let coff = e_lfanew + 4;
    let nsec = u16le(&b, coff + 2) as usize;
    let opt_size = u16le(&b, coff + 16) as usize;
    let opt = coff + 20;
    let magic = u16le(&b, opt);
    println!("PE{:?}  nº secciones={nsec}  tamaño_opcional={opt_size}", if magic == 0x20b { "64" } else { "32" });

    // useful fields
    let (image_base, entry) = if magic == 0x20b {
        (u64le(&b, opt + 24), u32le(&b, opt + 16))
    } else {
        (u32le(&b, opt + 28) as u64, u32le(&b, opt + 16))
    };
    let sect_align = u32le(&b, opt + 32);
    let file_align = u32le(&b, opt + 36);
    println!("ImageBase=0x{image_base:X}  EntryPoint RVA=0x{entry:X}");
    println!("SectionAlignment=0x{sect_align:X}  FileAlignment=0x{file_align:X}");

    let timestamp = u32le(&b, coff + 8);
    // convert the UNIX timestamp to a date (approx UTC)
    let secs = timestamp as i64;
    let days = secs / 86400; let rem = secs % 86400;
    let (y, m, d) = civil_from_days(days);
    let hh = rem / 3600; let mm = (rem % 3600) / 60;
    println!("TimeDateStamp=0x{timestamp:X} = {y:04}-{m:02}-{d:02} {hh:02}:{mm:02} UTC");

    let sect_off = opt + opt_size;
    println!("\n=== secciones ===");
    println!("{:<10} {:<12} {:<12} {:<12} {:<12}", "nombre", "RVA", "vsize", "raw_off", "raw_size");
    let mut outdir = std::env::temp_dir();
    outdir.push("rofl_secciones");
    let _ = fs::create_dir_all(&outdir);

    for i in 0..nsec {
        let s = sect_off + i * 40;
        let name = String::from_utf8_lossy(&b[s..s+8]).trim_end_matches('\0').to_string();
        let vsize = u32le(&b, s + 8);
        let rva = u32le(&b, s + 12);
        let raw_size = u32le(&b, s + 16);
        let raw_off = u32le(&b, s + 20);
        println!("{:<10} 0x{:<10X} 0x{:<10X} 0x{:<10X} 0x{:<10X}", name, rva, vsize, raw_off, raw_size);

        // dump the three used by ROFL-master
        if matches!(name.as_str(), ".text" | ".data" | ".rdata") {
            let start = raw_off as usize;
            let end = (raw_off as usize + raw_size as usize).min(b.len());
            if start < b.len() {
                let fname = outdir.join(format!("{}.bin", &name[1..]));
                fs::write(&fname, &b[start..end]).expect("error escribiendo sección");
                println!("            → volcada a {} ({} bytes)", fname.display(), end - start);
            }
        }
    }
    println!("\nvuelcos en: {}", outdir.display());
    println!("(fin)");
}

/// civil_from_days: days since epoch → (year, month, day). Hinnant's algorithm.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe/1460 + doe/36524 - doe/146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365*yoe + yoe/4 - yoe/100);
    let mp = (5*doy + 2)/153;
    let d = (doy - (153*mp+2)/5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}
