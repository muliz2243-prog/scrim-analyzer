//! Packs the logo into the formats Windows and the app need:
//!
//! * `assets/scrim-analyzer.ico` — multi-resolution icon (16→256) for the
//!   `.exe` and the installer.
//! * `assets/icon.rgba` — raw RGBA (64×64) for the window icon
//!   (egui loads it without runtime dependencies).
//!
//! Usage: `cargo run --release --example make_icons`

use std::fs;
use std::io::Cursor;
use std::path::Path;

use image::imageops::FilterType;
use image::DynamicImage;

/// `.ico` sizes (Windows uses 16/24/32/48/64/128/256).
const TAMANOS: [u32; 7] = [16, 24, 32, 48, 64, 128, 256];
/// Side length of the window icon.
const VENTANA: u32 = 64;

fn main() {
    let logo = Path::new("assets/logo.png");
    if !logo.is_file() {
        eprintln!("falta assets/logo.png");
        std::process::exit(1);
    }
    let img = image::open(logo).expect("leer assets/logo.png");

    // ---- multi-resolution .ico (entries compressed as PNG) ----
    let mut ico: Vec<u8> = Vec::new();
    ico.extend_from_slice(&0u16.to_le_bytes()); // reserved
    ico.extend_from_slice(&1u16.to_le_bytes()); // 1 = icon
    ico.extend_from_slice(&(TAMANOS.len() as u16).to_le_bytes());

    let mut datos: Vec<(u32, Vec<u8>)> = Vec::new();
    for lado in TAMANOS {
        let mut r = img.clone();
        // Crop to a square (the logo is square) and scale.
        let (w, h) = (r.width(), r.height());
        if w != h {
            let lado = w.min(h);
            r = r.crop_imm(0, 0, lado, lado);
        }
        r = r.resize_exact(lado, lado, FilterType::Lanczos3);
        let mut buf: Vec<u8> = Vec::new();
        DynamicImage::ImageRgba8(r.to_rgba8())
            .write_to(&mut Cursor::new(&mut buf), image::ImageFormat::Png)
            .expect("codificar PNG");
        datos.push((lado, buf));
    }

    let mut offset: u32 = 6 + 16 * TAMANOS.len() as u32;
    for (lado, buf) in &datos {
        ico.push(if *lado >= 256 { 0 } else { *lado as u8 });
        ico.push(if *lado >= 256 { 0 } else { *lado as u8 });
        ico.push(0); // palette
        ico.push(0); // reserved
        ico.extend_from_slice(&1u16.to_le_bytes()); // planes
        ico.extend_from_slice(&32u16.to_le_bytes()); // bits
        ico.extend_from_slice(&(buf.len() as u32).to_le_bytes());
        ico.extend_from_slice(&offset.to_le_bytes());
        offset += buf.len() as u32;
    }
    for (_, buf) in &datos {
        ico.extend_from_slice(buf);
    }
    fs::write("assets/scrim-analyzer.ico", &ico).expect("escribir el .ico");
    println!("assets/scrim-analyzer.ico ({} KB)", ico.len() / 1024);

    // ---- raw RGBA for the window ----
    let mut r = img;
    let (w, h) = (r.width(), r.height());
    if w != h {
        let lado = w.min(h);
        r = r.crop_imm(0, 0, lado, lado);
    }
    let r = r.resize_exact(VENTANA, VENTANA, FilterType::Lanczos3).to_rgba8();
    fs::write("assets/icon.rgba", r.as_raw()).expect("escribir el RGBA");
    println!("assets/icon.rgba ({} KB)", r.as_raw().len() / 1024);
}
