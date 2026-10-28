//! Palette, typography, colors, preference persistence and app visuals.
//!
//! Code moved verbatim from `gui_main.rs`; behavior is unchanged.

use super::*;

pub(crate) fn log_debug(msg: &str) {
    if let Ok(exe_dir) = std::env::current_exe().and_then(|p| Ok(p.parent().unwrap().to_path_buf())) {
        let log_path = exe_dir.join("debug.log");
        let _ = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .and_then(|mut f| writeln!(f, "[{:?}] {}", SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs(), msg));
    }
}

pub(crate) const BG: Color32 = Color32::from_rgb(6, 9, 22);

pub(crate) const PANEL: Color32 = Color32::from_rgb(12, 18, 42);

pub(crate) const PANEL2: Color32 = Color32::from_rgb(18, 27, 60);

pub(crate) const PANEL3: Color32 = Color32::from_rgb(30, 44, 88);

pub(crate) const INSET: Color32 = Color32::from_rgb(7, 11, 28);

pub(crate) const ACCENT: Color32 = Color32::from_rgb(56, 150, 255);

pub(crate) const ACCENT_SOFT: Color32 = Color32::from_rgb(140, 196, 255);

pub(crate) const CYAN: Color32 = Color32::from_rgb(0, 214, 255);

pub(crate) const GOLD: Color32 = Color32::from_rgb(214, 172, 92);

pub(crate) const GOLD_BRIGHT: Color32 = Color32::from_rgb(247, 216, 140);

pub(crate) const BLUE: Color32 = Color32::from_rgb(86, 160, 255);

pub(crate) const RED: Color32 = Color32::from_rgb(255, 94, 110);

/// Dark red for the RED team's half of an alert bar. It is deliberately
/// darker than the severity reds so the two halves are not confused.
pub(crate) const TEAM_RED_DARK: Color32 = Color32::from_rgb(122, 28, 44);

pub(crate) const TEAL: Color32 = Color32::from_rgb(64, 224, 214);

pub(crate) const GREEN: Color32 = Color32::from_rgb(110, 226, 148);

pub(crate) const AMBER: Color32 = Color32::from_rgb(255, 190, 90);

pub(crate) const MAGENTA: Color32 = Color32::from_rgb(255, 64, 150);

pub(crate) const TEXT: Color32 = Color32::from_rgb(240, 245, 255);

pub(crate) const MUTED: Color32 = Color32::from_rgb(150, 172, 210);

/// Title font family (Trajan-like serif: Palatino Linotype).
pub(crate) fn display_family() -> egui::FontFamily {
    egui::FontFamily::Name(std::sync::Arc::from("display"))
}

/// Font family for uppercase labels (Segoe UI Black).
pub(crate) fn strong_family() -> egui::FontFamily {
    egui::FontFamily::Name(std::sync::Arc::from("strong"))
}

/// Color of the team holding the advantage (to paint its alerts).
pub(crate) fn team_color(t: Team) -> Color32 {
    match t {
        Team::Blue => BLUE,
        Team::Red => RED,
    }
}

/// Color per severity level (same code for score and vision).
pub(crate) fn severity_color(s: Severity) -> Color32 {
    match s {
        Severity::VentajaMinima => AMBER,
        Severity::VentajaMedia => Color32::from_rgb(255, 138, 60),
        Severity::VentajaConsiderable => Color32::from_rgb(255, 78, 96),
        Severity::VentajaCritica => MAGENTA,
    }
}

pub(crate) fn severity_alpha(s: Severity) -> f32 {
    match s {
        Severity::VentajaMinima => 0.6,
        Severity::VentajaMedia => 0.75,
        Severity::VentajaConsiderable => 0.9,
        Severity::VentajaCritica => 1.0,
    }
}

pub(crate) fn mmss(secs: f32) -> String {
    let s = secs.max(0.0) as u32;
    format!("{:02}:{:02}", s / 60, s % 60)
}

pub(crate) fn basename(p: &str) -> String {
    Path::new(p)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| p.to_string())
}

pub(crate) fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."))
}

pub(crate) fn captures_dir() -> PathBuf {
    exe_dir().join("capturas")
}

pub(crate) fn config_path() -> PathBuf {
    exe_dir().join("config.json")
}

/// Folder chosen by the user for the `.json` captures (persisted in
/// `config.json` next to the executable).
pub(crate) fn load_capture_dir() -> Option<PathBuf> {
    let raw = std::fs::read_to_string(config_path()).ok()?;
    let value: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let dir = value.get("capture_dir")?.as_str()?;
    let path = PathBuf::from(dir);
    path.is_dir().then_some(path)
}

/// Persists the capture folder without touching other config keys (language…).
pub(crate) fn save_capture_dir(dir: &Path) {
    let mut value = std::fs::read_to_string(config_path())
        .ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    if let Some(obj) = value.as_object_mut() {
        obj.insert(
            "capture_dir".to_string(),
            serde_json::Value::String(dir.display().to_string()),
        );
    }
    if let Ok(json) = serde_json::to_string_pretty(&value) {
        let _ = std::fs::write(config_path(), json);
    }
}

/// Read or persist the language through the shared i18n config helpers.
pub(crate) fn load_lang() -> Lang {
    i18n::load_saved_lang()
}

pub(crate) fn save_lang(lang: &Lang) {
    i18n::save_saved_lang(lang);
}

/// App visuals: night blue, electric blue on the borders and white text.
///
/// They are applied separately from the theme because `egui` resets the visuals when it
/// changes theme (and the system may report a hot change).
pub(crate) fn apply_visuals(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    v.panel_fill = BG;
    v.window_fill = PANEL;
    v.extreme_bg_color = INSET;
    v.faint_bg_color = PANEL2;
    v.override_text_color = Some(TEXT);
    v.widgets.inactive.bg_fill = PANEL2;
    v.widgets.inactive.weak_bg_fill = PANEL2;
    v.widgets.inactive.corner_radius = egui::CornerRadius::same(6);
    v.widgets.hovered.bg_fill = PANEL3;
    v.widgets.hovered.weak_bg_fill = PANEL3;
    v.widgets.hovered.corner_radius = egui::CornerRadius::same(6);
    v.widgets.active.bg_fill = ACCENT.linear_multiply(0.55);
    v.widgets.active.weak_bg_fill = ACCENT.linear_multiply(0.55);
    v.widgets.active.corner_radius = egui::CornerRadius::same(6);
    v.selection.bg_fill = ACCENT.linear_multiply(0.35);
    v.selection.stroke = egui::Stroke::new(1.0, ACCENT_SOFT);
    v.hyperlink_color = CYAN;
    ctx.set_visuals(v);
}

pub(crate) fn setup_theme(ctx: &egui::Context) {
    // egui follows the system theme by default (light → white background).
    // We force it to dark as the base.
    ctx.set_theme(egui::Theme::Dark);

    // Fonts: Segoe UI (text), Palatino Linotype (serif headline) and
    // Segoe UI Black (uppercase labels).
    let mut fonts = egui::FontDefinitions::default();
    if let Ok(bytes) = std::fs::read(r"C:\Windows\Fonts\segoeui.ttf") {
        fonts.font_data.insert(
            "segoe".to_owned(),
            Arc::new(egui::FontData::from_owned(bytes)),
        );
        fonts
            .families
            .entry(egui::FontFamily::Proportional)
            .or_default()
            .insert(0, "segoe".to_owned());
    }
    if let Ok(bytes) = std::fs::read(r"C:\Windows\Fonts\pala.ttf") {
        fonts.font_data.insert(
            "display".to_owned(),
            Arc::new(egui::FontData::from_owned(bytes)),
        );
        fonts
            .families
            .insert(display_family(), vec!["display".to_owned()]);
    }
    if let Ok(bytes) = std::fs::read(r"C:\Windows\Fonts\seguibl.ttf") {
        fonts.font_data.insert(
            "strong".to_owned(),
            Arc::new(egui::FontData::from_owned(bytes)),
        );
        fonts
            .families
            .insert(strong_family(), vec!["strong".to_owned()]);
    }
    ctx.set_fonts(fonts);

    ctx.all_styles_mut(|s| {
        s.text_styles = std::collections::BTreeMap::from([
            (
                egui::TextStyle::Heading,
                egui::FontId::new(28.0, egui::FontFamily::Proportional),
            ),
            (
                egui::TextStyle::Body,
                egui::FontId::new(17.0, egui::FontFamily::Proportional),
            ),
            (
                egui::TextStyle::Button,
                egui::FontId::new(17.0, egui::FontFamily::Proportional),
            ),
            (
                egui::TextStyle::Small,
                egui::FontId::new(14.5, egui::FontFamily::Proportional),
            ),
            (
                egui::TextStyle::Monospace,
                egui::FontId::new(15.5, egui::FontFamily::Monospace),
            ),
        ]);
        s.spacing.item_spacing = Vec2::new(9.0, 8.0);
        s.spacing.button_padding = Vec2::new(14.0, 8.0);
    });

    let mut v = egui::Visuals::dark();
    v.panel_fill = BG;
    v.window_fill = PANEL;
    v.extreme_bg_color = INSET;
    v.faint_bg_color = PANEL2;
    v.override_text_color = Some(TEXT);
    v.widgets.inactive.bg_fill = PANEL2;
    v.widgets.inactive.weak_bg_fill = PANEL2;
    v.widgets.inactive.corner_radius = egui::CornerRadius::same(6);
    v.widgets.hovered.bg_fill = PANEL3;
    v.widgets.hovered.weak_bg_fill = PANEL3;
    v.widgets.hovered.corner_radius = egui::CornerRadius::same(6);
    v.widgets.active.bg_fill = ACCENT.linear_multiply(0.55);
    v.widgets.active.weak_bg_fill = ACCENT.linear_multiply(0.55);
    v.widgets.active.corner_radius = egui::CornerRadius::same(6);
    v.selection.bg_fill = ACCENT.linear_multiply(0.35);
    v.selection.stroke = egui::Stroke::new(1.0, ACCENT_SOFT);
    v.hyperlink_color = CYAN;
    ctx.set_visuals(v);
}

/// Window icon: raw 64×64 RGBA embedded in the binary.
///
/// It is generated with `cargo run --release --example make_icons` from
/// `assets/logo.png` (no runtime dependencies).
pub(crate) fn icono_app() -> egui::IconData {
    egui::IconData {
        rgba: include_bytes!("../../assets/icon.rgba").to_vec(),
        width: 64,
        height: 64,
    }
}
