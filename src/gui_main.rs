//! Local graphical interface (native, eframe/egui) for the scrim analyzer.
//!
//! The home screen is minimal and only has **live** capture (the only
//! reliable source: it attributes events by player name → team):
//!
//! * **Detect live game** — polls the Live Client Data API
//!   (`https://127.0.0.1:2999/liveclientdata/allgamedata`) every second and
//!   detects on its own when a game starts (or a replay is playing).
//!   When it ends, it computes the full report (score + vision) and **saves the
//!   capture** to `capturas\scrim_<epoch>.json` next to the executable. Also,
//!   the report has a **"Save .json log…"** button to export it
//!   wherever the user wants and review it later.
//!
//! Offline analysis of `.rofl` files and reloading saved captures are still
//! available through the command line (argument with the file path), but
//! they are no longer offered as options on the home screen: the modern `.rofl` does not expose
//! team attribution in its events, so it allows neither score nor vision.
//!
//! Build (only with the `gui` feature; the base CLI does not need it):
//!
//! ```text
//! cargo build --release --features gui
//! target\release\scrim-analyzer.exe            (or double-click the .exe)
//! ```

#![windows_subsystem = "windows"]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use std::fs::OpenOptions;
use std::io::Write;

fn log_debug(msg: &str) {
    if let Ok(exe_dir) = std::env::current_exe().and_then(|p| Ok(p.parent().unwrap().to_path_buf())) {
        let log_path = exe_dir.join("debug.log");
        let _ = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .and_then(|mut f| writeln!(f, "[{:?}] {}", SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs(), msg));
    }
}

use eframe::egui::{self, Color32, Layout, Pos2, Rect, RichText, Sense, Stroke, Vec2};

use rofl_analyzer::{
    analyze_file, dedup_saved_events, rebuild_event_counters, score, AnalyzeOptions,
    AttributedEvent, BreakAlert, DominionBlock, DominionReport, EventKind, GoldModelParams,
    LivePoller, ObjectiveEvent, Player, PlayerSample, Replay, ScrimBlock, ScrimCapture, ScoreParams,
    ScoreTop5, Severity, SeverityThresholds, Team, TeamTotals, VisionAlert, VisionThresholds,
    VisionTotals,
    LIVE_ENDPOINT,
    SAVED_EVENT_EPSILON_S,
    blocks::compute_gold_blocks,
    gold_breakdown::gold_blocks_with_axes,
    i18n::{self, Lang},
    lcu::{self, LcuClient},
};

// ---------------------------------------------------------------------------
// Palette (dark mode base + electric blue on the borders + title gold)
// ---------------------------------------------------------------------------

const BG: Color32 = Color32::from_rgb(6, 9, 22);
const PANEL: Color32 = Color32::from_rgb(12, 18, 42);
const PANEL2: Color32 = Color32::from_rgb(18, 27, 60);
const PANEL3: Color32 = Color32::from_rgb(30, 44, 88);
const INSET: Color32 = Color32::from_rgb(7, 11, 28);
const ACCENT: Color32 = Color32::from_rgb(56, 150, 255);
const ACCENT_SOFT: Color32 = Color32::from_rgb(140, 196, 255);
const CYAN: Color32 = Color32::from_rgb(0, 214, 255);
const GOLD: Color32 = Color32::from_rgb(214, 172, 92);
const GOLD_BRIGHT: Color32 = Color32::from_rgb(247, 216, 140);
const BLUE: Color32 = Color32::from_rgb(86, 160, 255);
const RED: Color32 = Color32::from_rgb(255, 94, 110);
/// Dark red for the RED team's half of an alert bar. It is deliberately
/// darker than the severity reds so the two halves are not confused.
const TEAM_RED_DARK: Color32 = Color32::from_rgb(122, 28, 44);
const TEAL: Color32 = Color32::from_rgb(64, 224, 214);
const GREEN: Color32 = Color32::from_rgb(110, 226, 148);
const AMBER: Color32 = Color32::from_rgb(255, 190, 90);
const MAGENTA: Color32 = Color32::from_rgb(255, 64, 150);
const TEXT: Color32 = Color32::from_rgb(240, 245, 255);
const MUTED: Color32 = Color32::from_rgb(150, 172, 210);

/// Title font family (Trajan-like serif: Palatino Linotype).
fn display_family() -> egui::FontFamily {
    egui::FontFamily::Name(std::sync::Arc::from("display"))
}

/// Font family for uppercase labels (Segoe UI Black).
fn strong_family() -> egui::FontFamily {
    egui::FontFamily::Name(std::sync::Arc::from("strong"))
}

/// Color of the team holding the advantage (to paint its alerts).
fn team_color(t: Team) -> Color32 {
    match t {
        Team::Blue => BLUE,
        Team::Red => RED,
    }
}

/// Color per severity level (same code for score and vision).
fn severity_color(s: Severity) -> Color32 {
    match s {
        Severity::VentajaMinima => AMBER,
        Severity::VentajaMedia => Color32::from_rgb(255, 138, 60),
        Severity::VentajaConsiderable => Color32::from_rgb(255, 78, 96),
        Severity::VentajaCritica => MAGENTA,
    }
}

fn severity_alpha(s: Severity) -> f32 {
    match s {
        Severity::VentajaMinima => 0.6,
        Severity::VentajaMedia => 0.75,
        Severity::VentajaConsiderable => 0.9,
        Severity::VentajaCritica => 1.0,
    }
}

fn mmss(secs: f32) -> String {
    let s = secs.max(0.0) as u32;
    format!("{:02}:{:02}", s / 60, s % 60)
}

fn basename(p: &str) -> String {
    Path::new(p)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| p.to_string())
}

fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn captures_dir() -> PathBuf {
    exe_dir().join("capturas")
}

fn config_path() -> PathBuf {
    exe_dir().join("config.json")
}

/// Folder chosen by the user for the `.json` captures (persisted in
/// `config.json` next to the executable).
fn load_capture_dir() -> Option<PathBuf> {
    let raw = std::fs::read_to_string(config_path()).ok()?;
    let value: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let dir = value.get("capture_dir")?.as_str()?;
    let path = PathBuf::from(dir);
    path.is_dir().then_some(path)
}

/// Persists the capture folder without touching other config keys (language…).
fn save_capture_dir(dir: &Path) {
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
fn load_lang() -> Lang {
    i18n::load_saved_lang()
}

fn save_lang(lang: &Lang) {
    i18n::save_saved_lang(lang);
}

// ---------------------------------------------------------------------------
// Working data
// ---------------------------------------------------------------------------

/// Messages sent by the background threads to the interface.
enum Msg {
    LiveWaiting,
    /// The poller has detected a different `gameId`: the previous game
    /// must be released before continuing to measure.
    LiveGameId(u64),
    LiveTick(Box<ScrimCapture>),
    /// Live preview of a replay: the LCU timeline is already available and the
    /// report is shown without waiting for playback to end.
    LivePreview(Box<ScrimCapture>),
    LiveDone(Result<Box<ScrimCapture>, String>),
    ReplayDone {
        path: String,
        result: Result<Box<Replay>, String>,
        warning: Option<String>,
    },
    /// Vision measured live for an open replay (merged with the LCU
    /// report without the user having to stop anything).
    LiveVisionReady {
        game_id: u64,
        blocks: Vec<ScrimBlock>,
    },
    /// Report of a `.rofl` enriched with the LCU timeline.
    RoflReport {
        report: Box<ReportView>,
    },
    CaptureDone {
        path: String,
        result: Result<Box<LoadedCapture>, String>,
    },
}

/// Recomputed saved capture.
struct LoadedCapture {
    report: DominionReport,
    blocks: Vec<ScrimBlock>,
    /// Original file contents (so it can be re-saved or exported).
    raw_json: String,
    game_mode: String,
    map_name: String,
    duration_s: f32,
    notes: Vec<String>,
    game_id: Option<u64>,
}

enum View {
    Home,
    LiveDash,
    Report(Box<ReportView>),
    Replay(Box<ReplayView>),
    Loading { label: String },
}

struct ReportView {
    title: String,
    subtitle: String,
    /// Revision name of a live capture (shown above the headline).
    revision: Option<String>,
    notes: Vec<String>,
    report: DominionReport,
    /// Original measured blocks (for the cumulative vision quota).
    vision_blocks: Vec<ScrimBlock>,
    /// `gameId` of the replay/game, used to merge the live capture.
    game_id: Option<u64>,
    /// Analyst annotations (saved separately, in their own file).
    anotaciones: Anotaciones,
    /// Instant selected with the time bar (game seconds).
    cursor_s: f32,
    /// Serialized log for "Save .json log…".
    json: Option<String>,
    /// Result of the last manual save `(ok, message)`.
    save_msg: Option<(bool, String)>,
}

struct ReplayView {
    path: String,
    replay: Box<Replay>,
    /// `(start_s, end_s, kills)` per 30 s window.
    density: Vec<(u32, u32, u32)>,
    /// Warning when enrichment with the LCU failed (e.g. client closed).
    warning: Option<String>,
}

/// Analyst annotations: free text saved in its own file.
#[derive(Debug, Clone, Default)]
struct Anotaciones {
    texto: String,
    /// Path of the notes file (or `None` if it has never been saved).
    ruta: Option<String>,
    /// Last message (error or confirmation).
    mensaje: Option<String>,
}

/// Modal shown before starting the live capture: the analyst names the
/// revision. That name is shown at the top and used for the `.json` file.
struct RevisionDialog {
    input: String,
    /// Focus the text field on the first frame.
    focus: bool,
    /// The user tried to continue with an empty name.
    empty_name: bool,
}

struct App {
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    view: View,
    live_stop: Arc<AtomicBool>,
    live_running: bool,
    /// The user left manually ("⌂ Home") while the capture is still running: the
    /// thread will finish and send the result, but it must not hijack the
    /// screen with the report.
    live_abandoned: bool,
    live_hint: String,
    last_live: Option<Box<ScrimCapture>>,
    /// `gameId` of the game being measured (detected by `r3dlog`).
    ///
    /// Used to **avoid pinning** the report/dashboard to the previous game: if
    /// the client starts another game or plays another replay, the id changes and
    /// `last_live` is cleared immediately.
    active_game_id: Option<u64>,
    /// Instant selected in the live dashboard (time bar).
    live_cursor_s: f32,
    /// Vision capture thread for an open replay (merged on close).
    vision_stop: Arc<AtomicBool>,
    /// Vision that arrived before the replay report.
    pending_vision: Option<(u64, Vec<ScrimBlock>)>,
    /// The user moved the bar: we stop following the live edge.
    live_cursor_manual: bool,
    /// Save the capture `.json` log **automatically** when it
    /// finishes (yes by default). If disabled, it is only saved manually.
    auto_guardar_json: bool,
    /// Folder chosen by the user for the `.json` captures.
    capture_dir: PathBuf,
    /// Name of the current revision (shown at the top and used as the file name).
    revision_name: String,
    /// Revision-name dialog, open while the user has not confirmed the name.
    revision_dialog: Option<RevisionDialog>,
    /// Active UI language (persisted in `config.json`).
    lang: Lang,
    /// Languages discovered at startup (embedded + external JSON).
    languages: Vec<Lang>,
    /// `(is_error, text)` — status banner on the home screen.
    status: Option<(bool, String)>,
}

// ---------------------------------------------------------------------------
// Background threads
// ---------------------------------------------------------------------------

/// Applies the LCU timeline to a capture: real gold, axes, Top 5, vision,
/// objectives and `gameId`.
fn apply_lcu_enrichment(
    cap: &mut ScrimCapture,
    game_id: u64,
    timeline: &lcu::LcuTimeline,
    summary: Option<lcu::MatchSummary>,
    set_duration: bool,
) {
    let gold_series = timeline.gold_series();
    let ward_evs = timeline.ward_events_with_team();
    log_debug(&format!(
        "enrich: game_id={game_id}, gold_series={} frames, ward_events={}",
        gold_series.len(),
        ward_evs.len()
    ));
    let dur = timeline.duration_s().max(cap.duration_s);
    let gold_report = compute_gold_blocks(&gold_series, dur, "lcu_timeline");
    let thresholds = SeverityThresholds::default();

    // 1. Real net gold + breakdown by axes.
    cap.dominion.blocks = gold_blocks_with_axes(timeline, &gold_report.blocks, &thresholds);
    cap.dominion.severity_thresholds = thresholds;

    // 2. Breakpoints and Top 5 recomputed on real gold.
    cap.dominion.breakpoints = score::compute_breakpoints(&cap.dominion.blocks);
    let (top5_a, top5_r) = score::score_top5(&cap.dominion.blocks);
    cap.dominion.top5_azul = top5_a;
    cap.dominion.top5_rojo = top5_r;

    // 3. Vision per window from WARD_PLACED (if the timeline provides it).
    let n_blocks = (dur.max(1.0) / 30.0).ceil() as u32;
    let mut vision_blocks: Vec<ScrimBlock> = (0..n_blocks)
        .map(|i| ScrimBlock {
            index: i,
            start_s: i as f32 * 30.0,
            end_s: (((i + 1) as f32) * 30.0).min(dur.max(1.0)),
            vision_source: "wards_placed_lcu".to_string(),
            ..Default::default()
        })
        .collect();
    for (t, _wtype, _x, _y, creator_id, team_id) in &ward_evs {
        let idx_f = (*t / 30.0).floor();
        if idx_f < 0.0 {
            continue;
        }
        let idx = idx_f as usize;
        if idx >= vision_blocks.len() {
            continue;
        }
        match lcu::team_from_participant(*creator_id).or(*team_id) {
            Some(100) => vision_blocks[idx].ward_blue_delta += 1.0,
            Some(200) => vision_blocks[idx].ward_red_delta += 1.0,
            _ => {}
        }
    }
    let total_wards_atribuidos: f64 = vision_blocks
        .iter()
        .map(|b| b.ward_blue_delta + b.ward_red_delta)
        .sum();
    // Metric priority: if the poller already measured `wardScore` (vision
    // points), that series is kept; the LCU ward counts are not the
    // same metric and must not replace it.
    let poller_tiene_score = cap.blocks.iter().any(|b| {
        b.vision_source == "wardScore"
            && (b.ward_blue_delta.abs() + b.ward_red_delta.abs()) > 0.0
    });
    let poller_wards: f64 = cap
        .blocks
        .iter()
        .map(|b| b.ward_blue_delta + b.ward_red_delta)
        .sum();
    if poller_tiene_score {
        log_debug("enrich: keeping poller wardScore series");
    } else if total_wards_atribuidos > 0.0 && total_wards_atribuidos >= poller_wards {
        cap.blocks = vision_blocks;
    } else {
        // Riot's timeline does not always include WARD_PLACED: keep the
        // wardScore/wards that the live poller did measure.
        log_debug("enrich: timeline has no useful wards; keeping poller vision");
        cap.omisiones.push(i18n::tr(
            "El timeline del LCU no incluye eventos WARD_PLACED en esta partida; la visión por ventana usa el wardScore/wards del Live Client Data (si lo hubo).",
        ));
    }
    // Measured totals for the whole game from /games/{id}: fallback when there
    // is no temporal breakdown (e.g. a replay opened directly from .rofl).
    if let Some(summary) = summary {
        if summary.vision.total() > 0 {
            cap.vision_totals = Some(VisionTotals {
                blue: summary.vision.blue,
                red: summary.vision.red,
            });
            cap.omisiones.push(i18n::trf(
                "Visión total medida por el cliente: {0} wards azules · {1} rojas (sin desglose por minuto).",
                &[&summary.vision.blue.to_string(), &summary.vision.red.to_string()],
            ));
        }
        if !summary.game_mode.is_empty() && summary.game_mode != "?" {
            cap.game_mode = summary.game_mode;
        }
        if !summary.map_name.is_empty() && summary.map_name != "?" {
            cap.map_name = summary.map_name;
        }
    }

    // 4. Exact epic objectives → chart alerts.
    let objs = timeline.objective_events();
    cap.dominion.avisos.clear();
    for (t_s, m_type, sub, _kid, team_opt) in objs {
        let team = match team_opt {
            Some(100) => Team::Blue,
            Some(200) => Team::Red,
            _ => continue,
        };
        let key = format!("{} {}", m_type, sub.as_deref().unwrap_or("")).to_lowercase();
        let tipo = if key.contains("baron") {
            "baron"
        } else if key.contains("herald") {
            "herald"
        } else if key.contains("horde") || key.contains("voidgrub") {
            "voidgrub"
        } else if key.contains("dragon") || key.contains("drake") {
            "dragon"
        } else {
            "objetivo"
        };
        cap.dominion.avisos.push(ObjectiveEvent {
            team,
            start_s: t_s,
            end_s: t_s + 120.0,
            dur_s: 120.0,
            puntos: 0.0,
            tipo: tipo.to_string(),
        });
    }

    if set_duration {
        cap.duration_s = dur;
    }
    cap.game_id = Some(game_id);
    cap.dominion.fuente = "lcu_timeline_oro_real".to_string();
    cap.dominion.vision = score::compute_vision(&cap.blocks, VisionThresholds::default());
    cap.omisiones
        .push(i18n::trf("Partida enlazada por r3dlog: gameId {0}", &[&game_id.to_string()]));
    log_debug(&format!(
        "enrich: breakpoints={}, vision_blocks={}, vision_alerts={}, avisos={}",
        cap.dominion.breakpoints.len(),
        cap.blocks.len(),
        cap.dominion.vision.alerts.len(),
        cap.dominion.avisos.len()
    ));
}

/// `gameId` candidates for enrichment when a capture ends.
fn game_id_candidates(
    announced: Option<u64>,
    session_start: SystemTime,
    include_any_age: bool,
) -> Vec<u64> {
    let mut v = Vec::new();
    let mut push = |id: u64| {
        if !v.contains(&id) {
            v.push(id);
        }
    };
    if let Some(id) = lcu::find_active_game_id() {
        push(id);
    }
    if let Some(id) = announced {
        push(id);
    }
    if let Some(id) = lcu::find_latest_rofl_game_id_since(session_start) {
        push(id);
    }
    // Last resort: the most recent log even if old (a long game whose
    // r3dlog was only written at start). Left for the last attempts.
    if include_any_age {
        if let Some(id) = lcu::find_latest_game_id_any_age() {
            push(id);
        }
    }
    v
}

fn spawn_live(tx: Sender<Msg>, stop: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        log_debug("spawn_live: started");
        let session_start = SystemTime::now();
        let mut poller = LivePoller::with_config(
            LIVE_ENDPOINT,
            2,
            Duration::from_millis(1000),
            GoldModelParams::default(),
        );
        // `gameId` already announced to the interface: if it changes, the previous
        // game is released (no getting stuck on the previous report).
        let mut announced_game_id: Option<u64> = None;
        // Replay timeline: it is available from the first minute and is
        // applied to the dashboard while playback runs.
        let mut replay_cache: Option<(u64, lcu::LcuTimeline, Option<lcu::MatchSummary>)> = None;
        let mut lcu_mid: Option<LcuClient> = None;
        let mut ticks: u64 = 0;
        loop {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            match poller.poll_once() {
                Ok(_) => {
                    let mut cap = poller.build_capture();
                    if let Some((id, timeline, summary)) = &replay_cache {
                        apply_lcu_enrichment(
                            &mut cap,
                            *id,
                            timeline,
                            summary.clone(),
                            false,
                        );
                    }
                    let _ = tx.send(Msg::LiveTick(Box::new(cap)));
                }
                Err(_) => {
                    // A newly opened replay (or a game still loading) makes the
                    // Live Client Data endpoint disappear for a few seconds
                    // while the client restarts. Do not mistake that hiccup for
                    // the end of the match: keep retrying while the LCU still
                    // reports a session, and only give up after a long outage.
                    let session = lcu::find_active_session();
                    let threshold = match session {
                        Some(s) if s.is_replay => 60,
                        Some(_) => 8,
                        None => 5,
                    };
                    if poller.looks_finished(threshold) {
                        break;
                    }
                    log_debug(&format!(
                        "spawn_live: poll failed (consecutive={}, threshold={threshold}); keeping capture",
                        poller.consecutive_failures()
                    ));
                    let _ = tx.send(Msg::LiveWaiting);
                }
            }
            ticks += 1;
            // The link is decided by the client's `r3dlog`, not by the most
            // recent .rofl: an old downloaded replay no longer hijacks the app.
            if ticks % 5 == 1 {
                if let Some(session) = lcu::find_active_session() {
                    let id = session.game_id;
                    if announced_game_id != Some(id) {
                        announced_game_id = Some(id);
                        log_debug(&format!("spawn_live: new game_id detected = {id}"));
                        let _ = tx.send(Msg::LiveGameId(id));
                    }
                    // Replay: the official timeline already exists. It is loaded once and
                    // applied on every tick so the dashboard shows gold,
                    // objectives and Top 5 without waiting for the end.
                    if session.is_replay
                        && replay_cache.as_ref().map(|(i, ..)| *i) != Some(id)
                    {
                        if lcu_mid.is_none() {
                            lcu_mid = LcuClient::connect().ok();
                        }
                        if let Some(client) = &lcu_mid {
                            match client.get_timeline(id) {
                                Ok(timeline) => {
                                    let summary = client.get_match_summary(id).ok();
                                    log_debug(&format!(
                                        "spawn_live: replay timeline {id} loaded live"
                                    ));
                                    replay_cache = Some((id, timeline, summary));
                                    let _ = tx.send(Msg::LiveGameId(id));
                                    // Immediate preview: replay gold/objectives
                                    // right away, without waiting for the end.
                                    let mut preview = poller.build_capture();
                                    if let Some((pid, ptl, psum)) = &replay_cache {
                                        apply_lcu_enrichment(
                                            &mut preview,
                                            *pid,
                                            ptl,
                                            psum.clone(),
                                            true,
                                        );
                                    }
                                    let _ = tx.send(Msg::LivePreview(Box::new(preview)));
                                }
                                Err(e) => {
                                    log_debug(&format!(
                                        "spawn_live: replay {id} has no timeline yet: {e}"
                                    ));
                                }
                            }
                        }
                    }
                }
            }
            // End of game: the API stops responding for several polls.
            if poller.looks_finished(3) {
                break;
            }
            std::thread::sleep(poller.poll_interval());
        }
        let mut cap = poller.build_capture();
        log_debug(&format!(
            "spawn_live: poll_count={}, duration_s={}, players={}",
            cap.poll_count,
            cap.duration_s,
            cap.players.len()
        ));

        // Wait for the client to finish writing the `.rofl`/`r3dlog` and for
        // the LCU to publish the timeline. In real games the timeline takes
        // a few seconds and the `r3dlog` may be locked while the game
        // is still open: that is why it is retried for ~40 s.
        std::thread::sleep(Duration::from_millis(1500));

        let mut enriquecida = false;
        if let Some((id, timeline, summary)) = &replay_cache {
            apply_lcu_enrichment(&mut cap, *id, timeline, summary.clone(), true);
            enriquecida = true;
        }

        if !enriquecida {
        match LcuClient::connect() {
            Err(e) => {
                log_debug(&format!("spawn_live: LCU unavailable: {e}"));
                cap.omisiones.push(i18n::trf(
                    "No se pudo conectar al LCU ({0}); el informe se queda con el score del modelo en vivo.",
                    &[&e.to_string()],
                ));
            }
            Ok(lcu_client) => {
                log_debug("spawn_live: LCU connected");
                let mut vistos: Vec<u64> = Vec::new();
                for attempt in 0..20 {
                    let candidates =
                        game_id_candidates(announced_game_id, session_start, attempt >= 10);
                    if candidates != vistos {
                        log_debug(&format!(
                            "spawn_live: attempt {} candidates = {:?}",
                            attempt + 1,
                            candidates
                        ));
                        vistos = candidates.clone();
                    }
                    for id in &candidates {
                        match lcu_client.get_timeline(*id) {
                            Ok(timeline) => {
                                let summary = match lcu_client.get_match_summary(*id) {
                                    Ok(s) => Some(s),
                                    Err(e) => {
                                        log_debug(&format!(
                                            "spawn_live: match summary {id} unavailable: {e}"
                                        ));
                                        None
                                    }
                                };
                                apply_lcu_enrichment(&mut cap, *id, &timeline, summary, true);
                                enriquecida = true;
                                break;
                            }
                            Err(e) => {
                                if attempt == 0 || attempt % 5 == 0 {
                                    log_debug(&format!(
                                        "spawn_live: get_timeline({id}) ERROR: {e}"
                                    ));
                                }
                            }
                        }
                    }
                    if enriquecida {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(2000));
                }
                if !enriquecida {
                    log_debug("spawn_live: no gameId produced a timeline after 40 s");
                    cap.omisiones.push(i18n::tr(
                        "Ningún gameId de esta sesión tiene timeline en el LCU; el informe se queda con el score del modelo en vivo.",
                    ));
                }
            }
        }
        }

        log_debug(&format!("spawn_live: final breakpoints={}, vision_alerts={}", cap.dominion.breakpoints.len(), cap.dominion.vision.alerts.len()));
        let _ = tx.send(Msg::LiveDone(Ok(Box::new(cap))));
    });
}

fn spawn_replay(tx: Sender<Msg>, path: String) {
    std::thread::spawn(move || {
        let result = analyze_file(&path, AnalyzeOptions::default())
            .map(Box::new)
            .map_err(|e| e.to_string());
        let _ = tx.send(Msg::ReplayDone {
            path,
            result,
            warning: None,
        });
    });
}

/// **Silent** vision capture for an open replay: polls the Live
/// Client Data API while the replay plays and, when it closes, sends the
/// 30 s blocks to merge them with the LCU report. It does not change
/// screen or require pressing "Stop".
fn spawn_vision_capture(tx: Sender<Msg>, stop: Arc<AtomicBool>, game_id: u64) {
    std::thread::spawn(move || {
        log_debug(&format!(
            "spawn_vision_capture: waiting for Live API for gameId {game_id}"
        ));
        let mut poller = LivePoller::with_config(
            LIVE_ENDPOINT,
            2,
            Duration::from_millis(1000),
            GoldModelParams::default(),
        );
        // Wait up to 120 s for the client to open **this** replay (not another
        // game in progress) and start serving the Live API.
        let mut started = false;
        for _ in 0..120 {
            if stop.load(Ordering::Relaxed) {
                return;
            }
            if lcu::find_active_game_id() == Some(game_id) && poller.poll_once().is_ok() {
                started = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(1000));
        }
        if !started {
            log_debug("spawn_vision_capture: client did not open this replay; no live vision");
            return;
        }
        log_debug("spawn_vision_capture: capturing live vision");
        // Safety cap of 4 h in case the client never stops responding.
        for tick in 0..14_400 {
            if stop.load(Ordering::Relaxed) {
                return;
            }
            // If the client switches game/replay, do not mix visions.
            if tick % 5 == 0 {
                if let Some(active) = lcu::find_active_game_id() {
                    if active != game_id {
                        log_debug("spawn_vision_capture: active gameId changed; discarding");
                        return;
                    }
                }
            }
            match poller.poll_once() {
                Ok(_) => {}
                Err(_) => {
                    if poller.looks_finished(5) {
                        break;
                    }
                }
            }
            std::thread::sleep(poller.poll_interval());
        }
        let cap = poller.build_capture();
        let blocks = cap.blocks;
        let con_vision = blocks
            .iter()
            .any(|b| (b.ward_blue_delta + b.ward_red_delta).abs() > 0.0);
        if con_vision {
            log_debug(&format!(
                "spawn_vision_capture: {} blocks with vision; sending to merge",
                blocks.len()
            ));
            let _ = tx.send(Msg::LiveVisionReady { game_id, blocks });
        } else {
            log_debug("spawn_vision_capture: capture had no vision");
        }
    });
}

/// Empty capture (no poller) to dump an LCU timeline from a `.rofl`.
fn empty_capture_for_timeline(
    game_id: u64,
    timeline: &lcu::LcuTimeline,
    summary: Option<&lcu::MatchSummary>,
) -> ScrimCapture {
    ScrimCapture {
        fuente: "lcu_timeline_oro_real".to_string(),
        endpoint: format!("lcu:{game_id}"),
        game_id: Some(game_id),
        vision_totals: None,
        reconstruido_por_modelo: false,
        game_mode: summary
            .map(|s| s.game_mode.clone())
            .filter(|s| !s.is_empty() && s != "?")
            .unwrap_or_else(|| "REPLAY".to_string()),
        map_name: summary
            .map(|s| s.map_name.clone())
            .filter(|s| !s.is_empty() && s != "?")
            .unwrap_or_else(|| "?".to_string()),
        duration_s: timeline.duration_s(),
        poll_count: 0,
        poll_interval_ms: 0,
        parametros: GoldModelParams::default(),
        omisiones: Vec::new(),
        players: Vec::new(),
        blocks: Vec::new(),
        events: Vec::new(),
        top5_azul: Default::default(),
        top5_rojo: Default::default(),
        dominion: score::compute_report(
            &[],
            ScoreParams::default(),
            VisionThresholds::default(),
            "lcu_timeline_oro_real",
        ),
    }
}

/// Opens a `.rofl`: first it tries the LCU timeline (complete and fast
/// gold/vision/objectives) using the `gameId` from the file name. If the
/// client is not available, it falls back to the basic forensic analysis of the `.rofl`.
fn spawn_rofl_enriched(tx: Sender<Msg>, path: String, game_id: u64) {
    std::thread::spawn(move || {
        log_debug(&format!(
            "spawn_rofl_enriched: path={path}, game_id={game_id}"
        ));
        let warning = match LcuClient::connect() {
            Ok(client) => match client.get_timeline(game_id) {
                Ok(timeline) => {
                    let summary = match client.get_match_summary(game_id) {
                        Ok(s) => Some(s),
                        Err(e) => {
                            log_debug(&format!("spawn_rofl_enriched: summary unavailable: {e}"));
                            None
                        }
                    };
                    let mut cap = empty_capture_for_timeline(game_id, &timeline, summary.as_ref());
                    apply_lcu_enrichment(&mut cap, game_id, &timeline, summary, true);
                    let notes = vec![
                        i18n::trf("Replay abierto desde {0}", &[&path]),
                        i18n::trf(
                            "Enriquecido con el timeline del LCU (gameId {0}).",
                            &[&game_id.to_string()],
                        ),
                    ];
                    let rv = report_view(
                        Box::new(cap),
                        notes,
                        "Informe del replay (oro + visión)",
                        "Replay · LCU timeline",
                    );
                    let _ = tx.send(Msg::RoflReport {
                        report: Box::new(rv),
                    });
                    return;
                }
                Err(e) => {
                    log_debug(&format!(
                        "spawn_rofl_enriched: get_timeline({game_id}) ERROR: {e}"
                    ));
                    Some(i18n::trf(
                        "No se pudo extraer el timeline del LCU para el gameId {0}: {1}. Se muestra el análisis básico del .rofl (sin oro/visión). Abre League of Legends y vuelve a abrir el replay para el informe completo.",
                        &[&game_id.to_string(), &e.to_string()],
                    ))
                }
            },
            Err(e) => {
                log_debug(&format!("spawn_rofl_enriched: LCU unavailable: {e}"));
                Some(i18n::trf(
                    "El cliente de League no está disponible ({0}). Se muestra el análisis básico del .rofl; para oro/visión, abre League y vuelve a abrir el replay.",
                    &[&e.to_string()],
                ))
            }
        };
        let result = analyze_file(&path, AnalyzeOptions::default())
            .map(Box::new)
            .map_err(|e| e.to_string());
        let _ = tx.send(Msg::ReplayDone {
            path,
            result,
            warning,
        });
    });
}

fn spawn_capture_load(tx: Sender<Msg>, path: String) {
    std::thread::spawn(move || {
        let result = load_capture(&path).map(Box::new);
        let _ = tx.send(Msg::CaptureDone { path, result });
    });
}

/// File name stem safe for Windows (and the other systems).
fn sanitize_file_stem(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for ch in name.chars() {
        out.push(match ch {
            '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if c.is_control() => '_',
            c => c,
        });
    }
    let trimmed = out.trim().trim_end_matches('.').trim();
    if trimmed.is_empty() {
        return "scrim".to_string();
    }
    // Windows reserved device names cannot be used as file names.
    const RESERVED: [&str; 22] = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7",
        "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    if RESERVED.iter().any(|r| trimmed.eq_ignore_ascii_case(r)) {
        format!("_{trimmed}")
    } else {
        trimmed.to_string()
    }
}

/// First free path: `name.json`, `name (1).json`, `name (2).json`…
fn unique_capture_path(dir: &Path, stem: &str) -> PathBuf {
    let mut path = dir.join(format!("{stem}.json"));
    let mut n = 1u32;
    while path.exists() {
        path = dir.join(format!("{stem} ({n}).json"));
        n += 1;
    }
    path
}

fn save_capture(cap: &ScrimCapture, dir: &Path, name: &str) -> Result<String, String> {
    std::fs::create_dir_all(dir)
        .map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    let stem = sanitize_file_stem(name);
    // If the name is already taken, the capture is saved anyway with a
    // Windows-style suffix: `name (1).json`, `name (2).json`…
    let path = unique_capture_path(dir, &stem);
    let json = cap.to_json()?;
    std::fs::write(&path, json)
        .map_err(|e| format!("could not write {}: {e}", path.display()))?;
    Ok(path.display().to_string())
}

/// Recomputes a report from a saved capture (same logic as `--recalc`).
fn load_capture(path: &str) -> Result<LoadedCapture, String> {
    let raw =
        std::fs::read_to_string(path).map_err(|e| format!("could not read {path}: {e}"))?;
    let v: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|e| format!("invalid JSON in {path}: {e}"))?;
    let null = serde_json::Value::Null;
    let mut blocks: Vec<ScrimBlock> = serde_json::from_value(
        v.get("blocks").cloned().unwrap_or(null.clone()),
    )
    .map_err(|e| format!("invalid blocks: {e}"))?;
    let events: Vec<AttributedEvent> = serde_json::from_value(
        v.get("events").cloned().unwrap_or(null.clone()),
    )
    .map_err(|e| format!("invalid events: {e}"))?;
    if blocks.is_empty() {
        return Err(format!("capture {path} has no blocks"));
    }
    let duration_s = v
        .get("duration_s")
        .and_then(serde_json::Value::as_f64)
        .map(|d| d as f32)
        .unwrap_or_else(|| blocks.last().map(|b| b.end_s).unwrap_or(0.0));
    let game_mode = v
        .get("game_mode")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("?")
        .to_string();
    let map_name = v
        .get("map_name")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("?")
        .to_string();
    let players: Vec<PlayerSample> =
        serde_json::from_value(v.get("players").cloned().unwrap_or(null)).unwrap_or_default();
    let game_id = v.get("game_id").and_then(serde_json::Value::as_u64);

    let mut notes = Vec::new();
    let deduped = dedup_saved_events(&events, SAVED_EVENT_EPSILON_S);
    notes.push(i18n::trf(
        "Deduplicación (epsilon = {0}s): {1} → {2} eventos.",
        &[
            &format!("{SAVED_EVENT_EPSILON_S:.1}"),
            &events.len().to_string(),
            &deduped.len().to_string(),
        ],
    ));
    rebuild_event_counters(&mut blocks, &deduped);

    // Feed vs scoreboard reconciliation (only if the capture includes players).
    let mut feed = (0u32, 0u32);
    for e in &deduped {
        if e.kind == "ChampionKill" {
            match e.team {
                Some(Team::Blue) => feed.0 += 1,
                Some(Team::Red) => feed.1 += 1,
                None => {}
            }
        }
    }
    let mut marcador = (0u32, 0u32);
    for p in &players {
        match p.team {
            Team::Blue => marcador.0 += p.kills,
            Team::Red => marcador.1 += p.kills,
        }
    }
    if marcador.0 > feed.0 || marcador.1 > feed.1 {
        notes.push(i18n::trf(
            "El feed omitió kills: el marcador mide {0}–{1} y el feed aporta {2}–{3}; esas ventanas no las cuentan.",
            &[
                &marcador.0.to_string(),
                &marcador.1.to_string(),
                &feed.0.to_string(),
                &feed.1.to_string(),
            ],
        ));
    }

    // If the capture already came enriched with the LCU, its real-gold report is kept
    // (recomputing with the points model would destroy the axes and the
    // Top 5). Old captures are recomputed as usual.
    let saved_dominion: Option<DominionReport> = v
        .get("dominion")
        .cloned()
        .and_then(|d| serde_json::from_value(d).ok());
    let report = match saved_dominion {
        Some(r) if r.fuente.contains("lcu") => {
            notes.push(i18n::tr(
                "Informe de oro real recuperado del JSON (sin recalcular).",
            ));
            r
        }
        _ => score::compute_report(
            &blocks,
            ScoreParams::default(),
            VisionThresholds::default(),
            "captura_guardada",
        ),
    };
    if let Some(id) = game_id {
        notes.push(i18n::trf("gameId de la captura: {0}", &[&id.to_string()]));
    }
    Ok(LoadedCapture {
        report,
        blocks,
        raw_json: raw,
        game_mode,
        map_name,
        duration_s,
        notes,
        game_id,
    })
}

fn kill_density(replay: &Replay) -> Vec<(u32, u32, u32)> {
    let dur = replay.metadata.game_length_s.max(0.0);
    let n = ((dur / 30.0).ceil() as usize).max(1);
    let mut counts = vec![0u32; n];
    for ev in &replay.events {
        if matches!(ev.kind, EventKind::ChampionKill) {
            let idx = (ev.timestamp_ms as f32 / 1000.0 / 30.0) as usize;
            if idx < n {
                counts[idx] += 1;
            }
        }
    }
    counts
        .iter()
        .enumerate()
        .map(|(i, &c)| (i as u32 * 30, i as u32 * 30 + 30, c))
        .collect()
}

/// Measured chips of the live dashboard.
struct LiveChips {
    cs: (u32, u32),
    kills: (u32, u32),
    towers: (u32, u32),
    objectives: (u32, u32),
    /// `(blue, red)` already formatted; "—" if the client did not expose vision.
    vision: (String, String),
}

/// Measured cumulative for the live game (last snapshot + events).
///
/// Vision uses `scores.wardScore` (measured vision score); if the
/// client does not expose it (all 0, e.g. in spectator mode), the
/// measured count of wards placed from `/stats` is shown, with the "w" suffix.
fn live_chips(cap: &ScrimCapture) -> LiveChips {
    let mut cs = (0u32, 0u32);
    let mut vis = (0.0f64, 0.0f64);
    let mut wp = (0u32, 0u32);
    let mut wp_any = false;
    for p in &cap.players {
        match p.team {
            Team::Blue => {
                cs.0 += p.cs;
                vis.0 += p.ward_score;
                if let Some(w) = p.wards_placed {
                    wp.0 += w;
                    wp_any = true;
                }
            }
            Team::Red => {
                cs.1 += p.cs;
                vis.1 += p.ward_score;
                if let Some(w) = p.wards_placed {
                    wp.1 += w;
                    wp_any = true;
                }
            }
        }
    }
    let mut k = (0u32, 0u32);
    let mut t = (0u32, 0u32);
    let mut o = (0u32, 0u32);
    for e in &cap.events {
        let is_kill = e.kind == "ChampionKill";
        let is_tower = e.kind == "TurretKilled" || e.kind == "TurretDestroyed";
        let is_obj = e.monster.is_some()
            || matches!(
                e.kind.as_str(),
                "DragonKill"
                    | "DragonKilled"
                    | "BaronKill"
                    | "BaronKilled"
                    | "HeraldKill"
                    | "HeraldKilled"
                    | "EpicMonsterKill"
            );
        if !(is_kill || is_tower || is_obj) {
            continue;
        }
        match e.team {
            Some(Team::Blue) => {
                if is_kill {
                    k.0 += 1;
                }
                if is_tower {
                    t.0 += 1;
                }
                if is_obj {
                    o.0 += 1;
                }
            }
            Some(Team::Red) => {
                if is_kill {
                    k.1 += 1;
                }
                if is_tower {
                    t.1 += 1;
                }
                if is_obj {
                    o.1 += 1;
                }
            }
            None => {}
        }
    }
    let vision = if vis.0 > 0.0 || vis.1 > 0.0 {
        (format!("{:.0}", vis.0), format!("{:.0}", vis.1))
    } else if wp_any {
        (format!("{}w", wp.0), format!("{}w", wp.1))
    } else {
        ("—".to_string(), "—".to_string())
    };
    LiveChips {
        cs,
        kills: k,
        towers: t,
        objectives: o,
        vision,
    }
}

/// Merges the live-measured vision with the LCU gold report.
fn merge_vision_into_report(r: &mut ReportView, blocks: Vec<ScrimBlock>) {
    if blocks.is_empty() {
        return;
    }
    let ventanas = blocks.len();
    r.vision_blocks = blocks;
    r.report.vision =
        score::compute_vision(&r.vision_blocks, VisionThresholds::default());
    let nota = i18n::trf(
        "Visión en vivo fusionada: {0} bloques de 30 s medidos con el cliente.",
        &[&ventanas.to_string()],
    );
    if !r
        .notes
        .iter()
        .any(|n| n.starts_with(&i18n::tr("Visión en vivo fusionada")))
    {
        r.notes.push(nota);
    }
    log_debug("merge_vision_into_report: vision merged");
}

fn report_from_capture(cap: Box<ScrimCapture>, notes: Vec<String>) -> ReportView {
    report_view(
        cap,
        notes,
        "Informe de la partida (oro + visión)",
        "En vivo",
    )
}

/// Builds the report view from a capture (live or replay).
fn report_view(
    cap: Box<ScrimCapture>,
    notes: Vec<String>,
    title: &str,
    modo: &str,
) -> ReportView {
    let subtitle = i18n::trf(
        "{0} · {1} · {2} · {3}s · {4} sondeos · {5} eventos",
        &[
            &i18n::tr(modo),
            &cap.game_mode,
            &cap.map_name,
            &format!("{:.1}", cap.duration_s),
            &cap.poll_count.to_string(),
            &cap.events.len().to_string(),
        ],
    );
    // Serialized log so it can be saved by hand whenever the user wants.
    let json = cap.to_json().ok();
    let cursor_s = cap
        .dominion
        .blocks
        .last()
        .map(|b| b.end_s)
        .or_else(|| cap.blocks.last().map(|b| b.end_s))
        .unwrap_or(cap.duration_s)
        .max(0.0);
    let game_id = cap.game_id;
    ReportView {
        title: i18n::tr(title),
        subtitle,
        revision: None,
        notes,
        report: cap.dominion,
        vision_blocks: cap.blocks,
        game_id,
        anotaciones: Anotaciones::default(),
        cursor_s,
        json,
        save_msg: None,
    }
}

fn report_from_loaded(path: &str, lc: LoadedCapture) -> ReportView {
    let subtitle = i18n::trf(
        "Captura guardada · {0} · {1} · {2}s · {3}",
        &[
            &lc.game_mode,
            &lc.map_name,
            &format!("{:.1}", lc.duration_s),
            path,
        ],
    );
    let cursor_s = lc
        .report
        .blocks
        .last()
        .map(|b| b.end_s)
        .or_else(|| lc.blocks.last().map(|b| b.end_s))
        .unwrap_or(lc.duration_s)
        .max(0.0);
    let mut notes = lc.notes;
    if let Some(id) = lc.game_id {
        if !notes.iter().any(|n| n.contains("gameId")) {
            notes.push(i18n::trf("gameId de la captura: {0}", &[&id.to_string()]));
        }
    }
    ReportView {
        title: i18n::tr("Informe recalculado desde captura"),
        subtitle,
        revision: None,
        notes,
        report: lc.report,
        vision_blocks: lc.blocks,
        game_id: lc.game_id,
        anotaciones: Anotaciones::default(),
        cursor_s,
        json: Some(lc.raw_json),
        save_msg: None,
    }
}

// ---------------------------------------------------------------------------
// Application
// ---------------------------------------------------------------------------

impl App {
    fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        let (tx, rx) = channel();
        let languages = i18n::available_languages();
        let lang = load_lang();
        i18n::set_lang(lang.clone());
        let mut app = Self {
            tx,
            rx,
            view: View::Home,
            live_stop: Arc::new(AtomicBool::new(false)),
            live_running: false,
            live_abandoned: false,
            live_hint: String::new(),
            last_live: None,
            active_game_id: None,
            live_cursor_s: 0.0,
            vision_stop: Arc::new(AtomicBool::new(false)),
            pending_vision: None,
            live_cursor_manual: false,
            auto_guardar_json: true,
            capture_dir: load_capture_dir().unwrap_or_else(captures_dir),
            revision_name: String::new(),
            revision_dialog: None,
            lang,
            languages,
            status: None,
        };
        // Startup via argument (e.g. double-click on an associated .rofl).
        if let Some(arg) = std::env::args().nth(1) {
            let lower = arg.to_lowercase();
            if lower.ends_with(".rofl") {
                app.open_replay(arg);
            } else if lower.ends_with(".json") {
                app.open_capture(arg);
            }
        }
        // The app always opens on the home screen. The live capture (and the
        // revision-name dialog) only starts when the user presses "Detect live
        // game"; nothing is launched automatically at startup.
        app
    }

    fn handle_msg(&mut self, msg: Msg) {
        match msg {
            Msg::LiveWaiting => {
                self.live_hint = i18n::tr("Esperando a que arranque una partida…");
            }
            Msg::LiveGameId(id) => {
                // New game/replay detected in the client: all state from the
                // previous one is released so neither the dashboard nor the
                // report is pinned to the previous gameId.
                if self.active_game_id != Some(id) {
                    log_debug(&format!(
                        "handle_msg: new game_id {id} (previous {:?})",
                        self.active_game_id
                    ));
                    self.active_game_id = Some(id);
                    self.last_live = None;
                    self.live_cursor_s = 0.0;
                    self.live_cursor_manual = false;
                }
            }
            Msg::LiveTick(cap) => {
                self.live_hint.clear();
                // If the user has not touched the time bar, we follow the
                // live edge.
                if !self.live_cursor_manual {
                    self.live_cursor_s = cap.duration_s;
                }
                self.last_live = Some(cap);
            }
            Msg::LivePreview(cap) => {
                if self.live_abandoned {
                    return;
                }
                self.live_hint.clear();
                let notes = vec![
                    i18n::tr("Vista previa en vivo: el oro y los objetivos vienen del timeline del LCU del replay."),
                    i18n::tr("La visión se sigue grabando y se completará al cerrar la reproducción."),
                ];
                let mut report = report_from_capture(cap, notes);
                report.revision = Some(self.revision_name.clone());
                self.view = View::Report(Box::new(report));
            }
            Msg::LiveDone(res) => {
                self.live_running = false;
                match res {
                    Ok(cap) => {
                        let abandoned = std::mem::take(&mut self.live_abandoned);
                        if cap.poll_count == 0 {
                            // No data: if the user cancelled by hand, we do not even
                            // show the "no game" notice.
                            if !abandoned {
                                self.status = Some((
                                    true,
                                    i18n::tr("No se detectó ninguna partida: el cliente no respondió. Abre League y arranca una partida, luego prueba otra vez."),
                                ));
                            }
                            self.view = View::Home;
                        } else if abandoned {
                            // Capture cancelled by hand: save it (the work is not
                            // lost) but return Home without hijacking the screen.
                            let note = if self.auto_guardar_json {
                                match save_capture(&cap, &self.capture_dir, &self.revision_name) {
                                    Ok(p) => i18n::trf("Captura en vivo terminada y guardada en {0}.", &[&p]),
                                    Err(e) => i18n::trf("No se pudo guardar la captura en vivo: {0}", &[&e.to_string()]),
                                }
                            } else {
                                i18n::tr("Captura en vivo terminada (guardado automático desactivado: no se ha escrito ningún archivo).")
                            };
                            self.status = Some((false, note));
                            self.view = View::Home;
                        } else {
                            let mut notes = Vec::new();
                            if let Some(id) = cap.game_id.or(self.active_game_id) {
                                notes.push(i18n::trf(
                                    "Partida enlazada por r3dlog: gameId {0}",
                                    &[&id.to_string()],
                                ));
                            }
                            if cap.dominion.fuente.contains("lcu") {
                                notes.push(i18n::tr(
                                    "Oro real del LCU: alertas, Top 5 y desglose por ejes calculados sobre el oro neto de cada ventana de 30 s.",
                                ));
                            } else {
                                notes.push(i18n::tr(
                                    "No se pudo enriquecer con el timeline del LCU: se muestra el score del modelo en vivo (puntos), no oro real.",
                                ));
                            }
                            if let Some(v) = cap.vision_totals {
                                if cap.dominion.vision.fuente.is_empty() {
                                    notes.push(i18n::trf(
                                        "Visión: {0} wards azules · {1} rojas (total medido; el timeline no da el desglose por minuto).",
                                        &[&v.blue.to_string(), &v.red.to_string()],
                                    ));
                                }
                            }
                            if self.auto_guardar_json {
                                match save_capture(&cap, &self.capture_dir, &self.revision_name) {
                                    Ok(p) => notes.push(i18n::trf("Captura guardada en {0}", &[&p])),
                                    Err(e) => {
                                        notes.push(i18n::trf("No se pudo guardar la captura: {0}", &[&e.to_string()]))
                                    }
                                }
                            } else {
                                notes.push(i18n::tr(
                                    "Guardado automático desactivado: esta captura no se ha escrito en disco; puedes guardarla a mano con «Guardar registro .json…».",
                                ));
                            }
                            self.last_live = None;
                            let mut report = report_from_capture(cap, notes);
                            report.revision = Some(self.revision_name.clone());
                            self.view = View::Report(Box::new(report));
                        }

                    }
                    Err(e) => {
                        self.status = Some((true, i18n::trf("Error en vivo: {0}", &[&e.to_string()])));
                        self.view = View::Home;
                    }
                }
            }
            Msg::ReplayDone {
                path,
                result,
                warning,
            } => match result {
                Ok(replay) => {
                    let density = kill_density(&replay);
                    self.view = View::Replay(Box::new(ReplayView {
                        path,
                        replay,
                        density,
                        warning,
                    }));
                }
                Err(e) => {
                    self.status = Some((
                        true,
                        i18n::trf(
                            "No se pudo analizar el replay: {0}{1}",
                            &[
                                &e.to_string(),
                                &warning
                                    .as_deref()
                                    .map(|w| format!(" · {w}"))
                                    .unwrap_or_default(),
                            ],
                        ),
                    ));
                    self.view = View::Home;
                }
            },
            Msg::LiveVisionReady { game_id, blocks } => {
                let mut merged = false;
                if let View::Report(r) = &mut self.view {
                    if r.game_id == Some(game_id) {
                        merge_vision_into_report(r, blocks.clone());
                        merged = true;
                    }
                }
                if !merged {
                    self.pending_vision = Some((game_id, blocks));
                }
            }
            Msg::RoflReport { report } => {
                self.status = None;
                let mut report = report;
                if let Some((id, blocks)) = self.pending_vision.take() {
                    if report.game_id == Some(id) {
                        merge_vision_into_report(&mut *report, blocks);
                    }
                }
                self.view = View::Report(report);
            }
            Msg::CaptureDone { path, result } => match result {
                Ok(lc) => {
                    self.view = View::Report(Box::new(report_from_loaded(&path, *lc)));
                }
                Err(e) => {
                    self.status = Some((true, i18n::trf("No se pudo cargar la captura: {0}", &[&e.to_string()])));
                    self.view = View::Home;
                }
            },
        }
    }

    /// Opens the revision-name dialog. The capture starts when the user
    /// confirms a non-empty name.
    fn request_live(&mut self) {
        let suggested = if self.revision_name.trim().is_empty() {
            i18n::tr("Revisión")
        } else {
            self.revision_name.clone()
        };
        self.revision_dialog = Some(RevisionDialog {
            input: suggested,
            focus: true,
            empty_name: false,
        });
    }

    /// Modal that asks for the revision name (translated in every language).
    fn revision_modal(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.revision_dialog.take() else {
            return;
        };
        let mut accept = false;
        let mut cancel = false;
        let response = egui::Modal::new(egui::Id::unique("revision_name_modal"))
            .frame(
                egui::Frame::default()
                    .fill(PANEL)
                    .inner_margin(egui::Margin::same(18))
                    .corner_radius(egui::CornerRadius::same(12))
                    .stroke(egui::Stroke::new(1.0, GOLD.linear_multiply(0.65))),
            )
            .show(ctx, |ui| {
                ui.set_width(480.0);
                ui.label(
                    RichText::new(i18n::tr("Nombre de la revisión"))
                        .family(strong_family())
                        .size(24.0)
                        .strong()
                        .color(GOLD_BRIGHT),
                );
                ui.add_space(5.0);
                ui.label(
                    RichText::new(i18n::tr(
                        "Escoge un nombre para esta revisión. Se mostrará arriba y será el nombre del archivo .json que se guarde en la carpeta elegida.",
                    ))
                    .size(14.0)
                    .color(MUTED),
                );
                ui.add_space(12.0);
                let edit = ui.add(
                    egui::TextEdit::singleline(&mut dialog.input)
                        .hint_text(i18n::tr("Nombre de la revisión…"))
                        .desired_width(f32::INFINITY)
                        .font(egui::TextStyle::Body),
                );
                if dialog.focus {
                    edit.request_focus();
                    dialog.focus = false;
                }
                if dialog.empty_name {
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(i18n::tr("Escribe un nombre para la revisión."))
                            .size(13.0)
                            .color(RED),
                    );
                }
                let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                ui.add_space(16.0);
                ui.horizontal(|ui| {
                    let start = egui::Button::new(
                        RichText::new(i18n::tr("Comenzar captura"))
                            .size(17.0)
                            .strong()
                            .color(Color32::WHITE),
                    )
                    .fill(ACCENT)
                    .corner_radius(egui::CornerRadius::same(9))
                    .min_size(Vec2::new(190.0, 40.0));
                    if ui.add(start).clicked() || enter {
                        accept = true;
                    }
                    let cancel_btn = egui::Button::new(
                        RichText::new(i18n::tr("Cancelar")).size(15.0).color(MUTED),
                    )
                    .fill(PANEL2)
                    .corner_radius(egui::CornerRadius::same(9))
                    .min_size(Vec2::new(110.0, 40.0));
                    if ui.add(cancel_btn).clicked() {
                        cancel = true;
                    }
                });
            });
        if response.should_close() {
            cancel = true;
        }
        if accept {
            let name = dialog.input.trim().to_string();
            if name.is_empty() {
                dialog.empty_name = true;
                self.revision_dialog = Some(dialog);
            } else {
                self.revision_name = name;
                if self.live_running {
                    // A capture is already running (e.g. the user pressed the
                    // button twice): only rename it and show the live screen,
                    // never spawn a second poller that would fight for the API.
                    self.view = View::LiveDash;
                } else {
                    self.start_live();
                }
            }
        } else if !cancel {
            self.revision_dialog = Some(dialog);
        }
    }

    fn start_live(&mut self) {
        self.last_live = None;
        self.active_game_id = None;
        self.live_cursor_s = 0.0;
        self.live_cursor_manual = false;
        self.status = None;
        self.live_hint = i18n::tr("Buscando partida…");
        self.live_running = true;
        self.live_abandoned = false;
        self.live_stop = Arc::new(AtomicBool::new(false));
        self.view = View::LiveDash;
        spawn_live(self.tx.clone(), self.live_stop.clone());
    }

    fn open_replay(&mut self, path: String) {
        self.view = View::Loading {
            label: i18n::trf("Analizando {0}…", &[&basename(&path)]),
        };
        // Reset the silent vision capture from a previous replay.
        self.vision_stop.store(true, Ordering::Relaxed);
        self.vision_stop = Arc::new(AtomicBool::new(false));
        self.pending_vision = None;
        match lcu::game_id_from_rofl_path(&path) {
            Some(game_id) => {
                spawn_rofl_enriched(self.tx.clone(), path, game_id);
                // In parallel, capture the real vision while the replay
                // plays. When it ends it is merged with the report without buttons.
                spawn_vision_capture(
                    self.tx.clone(),
                    self.vision_stop.clone(),
                    game_id,
                );
            }
            None => spawn_replay(self.tx.clone(), path),
        }
    }

    fn open_capture(&mut self, path: String) {
        self.view = View::Loading {
            label: i18n::trf("Cargando captura {0}…", &[&basename(&path)]),
        };
        spawn_capture_load(self.tx.clone(), path);
    }


    /// Dialog to choose a saved `.json` capture and recompute the report.
    fn pick_capture(&mut self) {
        let dir = self.capture_dir.clone();
        let mut dlg = rfd::FileDialog::new()
            .set_title(i18n::tr("Abrir captura de scrim (.json)"))
            .add_filter(i18n::tr("Captura JSON"), &["json"]);
        if dir.is_dir() {
            dlg = dlg.set_directory(&dir);
        }
        if let Some(p) = dlg.pick_file() {
            self.open_capture(p.display().to_string());
        }
    }

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("topbar")
            .frame(
                egui::Frame::default()
                    .fill(BG)
                    .inner_margin(egui::Margin::same(8))
                    .stroke(egui::Stroke::new(1.0, PANEL3)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                ui.label(
                    RichText::new(i18n::tr("SCRIM ANALYZER"))
                        .family(display_family())
                        .size(19.0)
                        .strong()
                        .color(GOLD_BRIGHT),
                );
                ui.label(RichText::new(i18n::tr("· League of Legends")).size(13.5).color(MUTED));
                ui.add_space(12.0);
                let section = match &self.view {
                    View::Home => i18n::tr("Inicio"),
                    View::LiveDash => i18n::tr("En vivo"),
                    View::Report(_) => i18n::tr("Informe"),
                    View::Replay(_) => i18n::tr("Replay"),
                    View::Loading { .. } => i18n::tr("Cargando…"),
                };
                ui.label(RichText::new(section).size(14.0).color(TEXT));
                if !self.revision_name.is_empty() && matches!(self.view, View::LiveDash) {
                    ui.label(
                        RichText::new(format!("· {}", self.revision_name))
                            .size(14.0)
                            .strong()
                            .color(GOLD_BRIGHT),
                    );
                }

                ui.with_layout(Layout::right_to_left(egui::Align::Center), |ui| {
                    let mut new_lang: Option<Lang> = None;
                    egui::ComboBox::from_id_salt("lang_selector")
                        .selected_text(self.lang.code().to_ascii_uppercase())
                        .width(78.0)
                        .show_ui(ui, |ui| {
                            for lang in &self.languages {
                                if ui
                                    .selectable_label(
                                        self.lang.code() == lang.code(),
                                        lang.native_name(),
                                    )
                                    .clicked()
                                {
                                    new_lang = Some(lang.clone());
                                }
                            }
                        });
                    if let Some(lang) = new_lang {
                        if lang.code() != self.lang.code() {
                            i18n::set_lang(lang.clone());
                            save_lang(&lang);
                            self.lang = lang;
                        }
                    }
                    if !matches!(self.view, View::Home) && ui.button(i18n::tr("⌂  Inicio")).clicked() {
                        self.live_stop.store(true, Ordering::Relaxed);
                        if self.live_running {
                            self.live_abandoned = true;
                        }
                        self.live_running = false;
                        self.last_live = None;
                        self.view = View::Home;
                    }
                    if self.live_running {
                        let txt = if self.last_live.is_some() {
                            i18n::tr("● en vivo")
                        } else {
                            i18n::tr("● esperando partida")
                        };
                        ui.label(RichText::new(txt).size(13.5).color(GREEN));
                    }
                });
            });
            ui.add_space(5.0);
        });
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        while let Ok(msg) = self.rx.try_recv() {
            self.handle_msg(msg);
        }
        // The threads send messages: we repaint often (cheap when there is no data).
        ui.ctx().request_repaint_after(Duration::from_millis(200));

        // egui follows the system theme: we always force it to dark.
        if ui.ctx().theme() != egui::Theme::Dark {
            ui.ctx().set_theme(egui::Theme::Dark);
            apply_visuals(ui.ctx());
        }

        self.top_bar(ui);

        // Revision-name dialog (live capture). It is drawn on top of any view.
        let ctx = ui.ctx().clone();
        self.revision_modal(&ctx);

        let view = std::mem::replace(&mut self.view, View::Home);
        egui::CentralPanel::default().show(ui, |ui| {
            // Background with a night-blue gradient (technical detail).
            ui.painter().add(egui::Shape::gradient_rect(
                ui.max_rect(),
                egui::Direction::TopDown,
                [Color32::from_rgb(9, 14, 36), Color32::from_rgb(4, 6, 16)],
            ));
            egui::ScrollArea::vertical().show(ui, |ui| match view {
                View::Home => self.ui_home(ui),
                View::LiveDash => {
                    self.ui_live_dash(ui);
                    // IMPORTANT: restore the view. The `replace` above leaves
                    // `self.view = View::Home`; if this arm does not put it back, the app
                    // returns to the home screen on the next frame and the live
                    // dashboard "flickers" and disappears.
                    self.view = View::LiveDash;
                }
                View::Report(mut r) => {
                    ui_report(
                        ui,
                        &mut r,
                        &mut self.auto_guardar_json,
                        &self.capture_dir,
                        &self.revision_name,
                    );
                    self.view = View::Report(r);
                }
                View::Replay(mut r) => {
                    ui_replay(ui, &mut r);
                    self.view = View::Replay(r);
                }
                View::Loading { label } => {
                    ui_loading(ui, &label);
                    // Same case as LiveDash: keep the loading screen.
                    self.view = View::Loading { label };
                }
            });
        });
    }
}

// ---------------------------------------------------------------------------
// Screens
// ---------------------------------------------------------------------------

impl App {
    /// Minimal home screen: only the name and the live capture button.
    fn ui_home(&mut self, ui: &mut egui::Ui) {
        let h = ui.available_height();
        ui.add_space((h * 0.20).max(30.0));
        ui.vertical_centered(|ui| {
            egui::Frame::default()
                .inner_margin(egui::Margin::same(0))
                .shadow(egui::Shadow {
                    offset: [0, 3],
                    blur: 20,
                    spread: 4,
                    color: GOLD.linear_multiply(0.35),
                })
                .show(ui, |ui| {
                    ui.label(
                        RichText::new(i18n::tr("SCRIM ANALYZER"))
                            .family(display_family())
                            .size(54.0)
                            .strong()
                            .color(GOLD_BRIGHT),
                    );
                });
            let (r, _) = ui.allocate_exact_size(Vec2::new(230.0, 3.0), Sense::hover());
            ui.painter().rect_filled(r, egui::CornerRadius::same(2), GOLD);
            ui.add_space(10.0);
            ui.label(
                RichText::new(i18n::tr("Momentos críticos de tus scrims de League of Legends"))
                    .size(17.0)
                    .color(MUTED),
            );
            ui.add_space(26.0);

            if let Some((is_err, text)) = self.status.clone() {
                let c = if is_err { RED } else { GREEN };
                egui::Frame::default()
                    .fill(if is_err {
                        Color32::from_rgb(52, 22, 30)
                    } else {
                        Color32::from_rgb(18, 44, 34)
                    })
                    .inner_margin(egui::Margin::same(12))
                    .corner_radius(egui::CornerRadius::same(8))
                    .stroke(egui::Stroke::new(1.0, c.linear_multiply(0.5)))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(if is_err { "⚠" } else { "✔" }).size(17.0).color(c));
                            ui.label(RichText::new(text).size(15.0).color(TEXT));
                            if ui.small_button(i18n::tr("descartar")).clicked() {
                                self.status = None;
                            }
                        });
                    });
                ui.add_space(14.0);
            }

            let b = egui::Button::new(
                RichText::new(i18n::tr("●  Detectar partida en vivo"))
                    .size(20.0)
                    .strong()
                    .color(Color32::WHITE),
            )
            .fill(ACCENT)
            .corner_radius(egui::CornerRadius::same(10))
            .min_size(Vec2::new(360.0, 56.0));
            if ui.add(b).clicked() {
                self.request_live();
            }
            ui.add_space(10.0);
            let b2 = egui::Button::new(
                RichText::new(i18n::tr("📂  Abrir captura .json guardada…"))
                    .size(17.0)
                    .color(ACCENT_SOFT),
            )
            .fill(PANEL2)
            .corner_radius(egui::CornerRadius::same(10))
            .min_size(Vec2::new(360.0, 42.0));
            if ui.add(b2).clicked() {
                self.pick_capture();
            }
            ui.add_space(10.0);
            // Option: automatic saving of the .json log.
            egui::Frame::default()
                .fill(PANEL)
                .inner_margin(egui::Margin::same(10))
                .corner_radius(egui::CornerRadius::same(8))
                .stroke(egui::Stroke::new(1.0, PANEL3))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.checkbox(
                            &mut self.auto_guardar_json,
                            RichText::new(i18n::tr("Guardar JSON automáticamente"))
                                .size(14.5)
                                .color(TEXT),
                        );
                        ui.label(
                            RichText::new(if self.auto_guardar_json {
                                i18n::tr("al terminar la partida se escribe sola en la carpeta elegida")
                            } else {
                                i18n::tr("no se escribe nada: guárdalo a mano desde el informe")
                            })
                            .size(12.5)
                            .color(MUTED),
                        );
                    });
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        let pick = egui::Button::new(
                            RichText::new(i18n::tr("📁  Elegir carpeta…"))
                                .size(14.0)
                                .color(ACCENT_SOFT),
                        )
                        .fill(PANEL2)
                        .corner_radius(egui::CornerRadius::same(8))
                        .min_size(Vec2::new(190.0, 32.0));
                        if ui.add(pick).clicked() {
                            if let Some(dir) = rfd::FileDialog::new()
                                .set_title(i18n::tr("Elegir carpeta de guardado de los .json"))
                                .set_directory(&self.capture_dir)
                                .pick_folder()
                            {
                                self.capture_dir = dir.clone();
                                save_capture_dir(&dir);
                            }
                        }
                        ui.label(
                            RichText::new(i18n::trf(
                                "Carpeta de guardado: {0}",
                                &[&self.capture_dir.display().to_string()],
                            ))
                            .size(12.5)
                            .color(MUTED),
                        );
                    });
                });
        });
    }

    fn ui_live_dash(&mut self, ui: &mut egui::Ui) {
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new(i18n::tr("CAPTURA EN VIVO")).size(22.0).strong().color(CYAN));
            if !self.revision_name.is_empty() {
                ui.label(
                    RichText::new(format!("· {}", self.revision_name))
                        .size(18.0)
                        .strong()
                        .color(GOLD_BRIGHT),
                );
            }
            if self.live_running {
                ui.add(egui::Spinner::new().size(20.0).color(GREEN));
            }
        });
        ui.add_space(6.0);

        // Disjoint borrows of the same `self`: the live capture and the time
        // bar (cursor) are different fields.
        let cap_ref = &self.last_live;
        let cursor = &mut self.live_cursor_s;
        let manual = &mut self.live_cursor_manual;

        match cap_ref {
            None => {
                let hint = if self.live_hint.is_empty() {
                    i18n::tr("Buscando partida…")
                } else {
                    self.live_hint.clone()
                };
                ui.label(RichText::new(hint).size(18.0).color(TEXT));
            }
            Some(cap) => {
                ui.label(
                    RichText::new(i18n::trf(
                        "Conectado — {0} · {1} · t = {2}   ·   {3} sondeos · {4} eventos",
                        &[
                            &cap.game_mode,
                            &cap.map_name,
                            &mmss(cap.duration_s),
                            &cap.poll_count.to_string(),
                            &cap.events.len().to_string(),
                        ],
                    ))
                    .size(15.0)
                    .color(GREEN),
                );
                ui.add_space(10.0);
                stats_row(ui, &live_chips(cap));
                ui.add_space(12.0);

                let dur = total_duration(&cap.dominion, &cap.blocks);
                if time_slider(ui, cursor, dur) {
                    *manual = true;
                }
                ui.add_space(4.0);
                advantage_cards(ui, &cap.dominion, &cap.blocks, *cursor, None, None);
                ui.add_space(10.0);

                let es_oro = cap.dominion.fuente.contains("lcu");
                ui.columns(2, |cols| {
                    let left = &mut cols[0];
                    left.label(
                        RichText::new(if es_oro {
                            i18n::tr("VENTAJA ACUMULADA DE ORO")
                        } else {
                            i18n::tr("VENTAJA ACUMULADA DE SCORE")
                        })
                        .family(strong_family())
                        .size(13.0)
                        .color(GOLD_BRIGHT),
                    );
                    score_chart(left, &cap.dominion, dur, 150.0, *cursor);
                    let right = &mut cols[1];
                    right.label(
                        RichText::new(i18n::tr("VENTAJA ACUMULADA DE VISIÓN"))
                            .family(strong_family())
                            .size(13.0)
                            .color(GOLD_BRIGHT),
                    );
                    vision_chart(right, &cap.blocks, dur, 150.0, *cursor, None);
                });
                ui.add_space(10.0);
                ui.label(
                    RichText::new(i18n::tr("Línea de tiempo de 30 s (barra = score · franja inferior = visión)"))
                        .size(13.5)
                        .color(MUTED),
                );
                timeline_strip(ui, &cap.dominion, 92.0);
                ui.add_space(12.0);
                live_alerts_preview(ui, &cap.dominion);
            }
        }

        if !self.live_running {
            ui.add_space(14.0);
            ui.separator();
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().size(20.0).color(ACCENT));
                ui.label(RichText::new(i18n::tr("Generando informe final…")).size(15.0).color(TEXT));
            });
        }
    }
}

fn ui_loading(ui: &mut egui::Ui, label: &str) {
    ui.add_space(60.0);
    ui.vertical_centered(|ui| {
        ui.add(egui::Spinner::new().size(46.0).color(ACCENT));
        ui.add_space(14.0);
        ui.label(RichText::new(label).size(18.0).color(TEXT));
        ui.add_space(6.0);
        ui.label(
            RichText::new(i18n::tr("Esto puede tardar unos segundos…")).size(14.0).color(MUTED),
        );
    });
}

// ---------------------------------------------------------------------------
// Components
// ---------------------------------------------------------------------------

fn chip(ui: &mut egui::Ui, label: &str, blue: String, red: String) {
    egui::Frame::default()
        .fill(PANEL)
        .inner_margin(egui::Margin::same(10))
        .corner_radius(egui::CornerRadius::same(6))
        .show(ui, |ui| {
            ui.label(RichText::new(i18n::tr(label)).size(12.5).color(MUTED));
            ui.horizontal(|ui| {
                ui.label(RichText::new(blue).size(20.0).strong().color(BLUE));
                ui.label(RichText::new(i18n::tr("–")).size(15.0).color(MUTED));
                ui.label(RichText::new(red).size(20.0).strong().color(RED));
            });
        });
}

fn stats_row(ui: &mut egui::Ui, c: &LiveChips) {
    ui.horizontal_wrapped(|ui| {
        chip(ui, "KILLS", c.kills.0.to_string(), c.kills.1.to_string());
        chip(ui, "CS", c.cs.0.to_string(), c.cs.1.to_string());
        chip(ui, "TORRES", c.towers.0.to_string(), c.towers.1.to_string());
        chip(ui, "OBJETIVOS", c.objectives.0.to_string(), c.objectives.1.to_string());
        chip(ui, "VISIÓN", c.vision.0.clone(), c.vision.1.clone());
    });
}

/// 30 s strip: score bar per window + vision alerts band.
fn timeline_strip(ui: &mut egui::Ui, report: &DominionReport, height: f32) {
    let n = report.blocks.len().max(1);
    let width = ui.available_width().max(240.0);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, egui::CornerRadius::same(6), INSET);

    let pad = 6.0;
    let inner_w = (rect.width() - 2.0 * pad).max(1.0);
    let col_w = inner_w / n as f32;
    let vrow_h = 12.0;
    let bar_h = (rect.height() - 2.0 * pad - vrow_h - 6.0).max(8.0);
    let base_y = rect.top() + pad + bar_h;

    // Bar scale: the critical cut-off (in points).
    let crit = report.severity_thresholds.ventaja_critica.max(1.0);
    for (i, b) in report.blocks.iter().enumerate() {
        let x = rect.left() + pad + i as f32 * col_w;
        let w = (col_w - 2.0).max(1.0);
        match b.severity {
            Some(sev) => {
                // Alert window: left half = severity, right half = team that
                // holds the advantage (blue / dark red). This way the bar
                // itself shows who the alert favors, not only its severity.
                let team = if b.score_net_blue >= 0.0 { BLUE } else { TEAM_RED_DARK };
                let half = (w * 0.5).max(1.0);
                painter.rect_filled(
                    Rect::from_min_size(
                        Pos2::new(x + 1.0, base_y - bar_h),
                        Vec2::new(half, bar_h),
                    ),
                    egui::CornerRadius { nw: 2, sw: 2, ne: 0, se: 0 },
                    severity_color(sev),
                );
                painter.rect_filled(
                    Rect::from_min_size(
                        Pos2::new(x + 1.0 + half, base_y - bar_h),
                        Vec2::new((w - half).max(1.0), bar_h),
                    ),
                    egui::CornerRadius { nw: 0, sw: 0, ne: 2, se: 2 },
                    team,
                );
            }
            None => {
                let mag = ((b.score_net_blue.abs() / crit).min(1.0)) as f32;
                let h = (bar_h * mag).max(3.0);
                let c = if b.score_net_blue >= 0.0 { BLUE } else { RED };
                painter.rect_filled(
                    Rect::from_min_size(Pos2::new(x + 1.0, base_y - h), Vec2::new(w, h)),
                    egui::CornerRadius::same(2),
                    c.linear_multiply(0.42),
                );
            }
        }
    }

    // Bottom band: vision alerts.
    let vy = rect.bottom() - pad - vrow_h;
    for a in &report.vision.alerts {
        let i = a.block_index as usize;
        if i >= n {
            continue;
        }
        let x = rect.left() + pad + i as f32 * col_w;
        let c = if a.cuota_azul >= 0.5 { BLUE } else { RED };
        painter.rect_filled(
            Rect::from_min_size(Pos2::new(x + 1.0, vy), Vec2::new((col_w - 2.0).max(1.0), vrow_h)),
            egui::CornerRadius::same(2),
            c.linear_multiply(severity_alpha(a.severity)),
        );
    }

    if resp.hovered() {
        if let Some(pos) = resp.hover_pos() {
            let idx = (((pos.x - rect.left() - pad) / col_w) as usize).min(n - 1);
            if let Some(b) = report.blocks.get(idx) {
                let fav = i18n::tr(if b.score_net_blue >= 0.0 { "Azul" } else { "Rojo" });
                let es_oro = report.fuente.contains("lcu");
                let magnitud = if es_oro {
                    format!("{:+.0}g", b.score_net_blue)
                } else {
                    format!("{:+.1}", b.score_net_blue)
                };
                let mut txt = i18n::trf(
                    "[{0} – {1}]\n{2} {3}",
                    &[
                        &mmss(b.start_s),
                        &mmss(b.end_s),
                        &i18n::tr(if es_oro { "oro" } else { "score" }),
                        &magnitud,
                    ],
                );
                if let Some(sev) = b.severity {
                    txt.push_str(&i18n::trf(
                        "\n{0} — {1}",
                        &[&severity_label(sev), &fav],
                    ));
                }
                if let Some(a) = report
                    .vision
                    .alerts
                    .iter()
                    .find(|a| a.block_index as usize == idx)
                {
                    txt.push_str(&i18n::trf(
                        "\nvisión azul {0} · visión roja {1} → {2}",
                        &[
                            &format!("{:.0}", a.delta_azul),
                            &format!("{:.0}", a.delta_rojo),
                            &severity_label(a.severity),
                        ],
                    ));
                }
                resp.on_hover_text(txt);
            }
        }
    }
}

/// Human-readable name of an epic objective.
fn objective_name(tipo: &str) -> String {
    i18n::tr(match tipo {
        "dragon" => "el DRAGÓN",
        "herald" => "el HERALDO",
        "baron" => "el BARÓN",
        "voidgrub" => "las LARVAS DEL VACÍO",
        "inhib" => "el INHIBIDOR",
        _ => "un objetivo épico",
    })
}

/// Objective verb: "killed the dragon" / "destroyed the inhibitor".
fn objective_verb(tipo: &str) -> String {
    i18n::tr(match tipo {
        "inhib" => "derribó",
        _ => "mató",
    })
}

/// A line of the chronological log: a score alert or an epic objective
/// notice (which **does not** add or subtract score, but is a key data point).
enum Linea {
    Alerta(BreakAlert),
    Objetivo(ObjectiveEvent),
}

impl Linea {
    fn instante(&self) -> f32 {
        match self {
            Linea::Alerta(a) => a.start_s,
            Linea::Objetivo(o) => o.start_s,
        }
    }
}

/// Chronological log: score alerts + epic objectives interleaved by
/// their instant (a baron falls at its minute, between the previous and the
/// following alert).
fn lineas_cronologicas(report: &DominionReport) -> Vec<Linea> {
    let mut v: Vec<Linea> = Vec::new();
    for a in sorted_breakpoints(report) {
        v.push(Linea::Alerta(a));
    }
    for o in &report.avisos {
        v.push(Linea::Objetivo(o.clone()));
    }
    v.sort_by(|a, b| {
        a.instante()
            .partial_cmp(&b.instante())
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    v
}

/// Score alerts sorted **chronologically** (oldest to most recent).
fn sorted_breakpoints(report: &DominionReport) -> Vec<BreakAlert> {
    let mut v = report.breakpoints.clone();
    v.sort_by(|a, b| {
        a.start_s
            .partial_cmp(&b.start_s)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.block_index.cmp(&b.block_index))
    });
    v
}

/// Vision alerts sorted **chronologically**.
fn sorted_vision_alerts(report: &DominionReport) -> Vec<VisionAlert> {
    let mut v = report.vision.alerts.clone();
    v.sort_by(|a, b| {
        a.start_s
            .partial_cmp(&b.start_s)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.block_index.cmp(&b.block_index))
    });
    v
}

/// Recent live alerts: gold/score on the left, vision on the right.
fn live_alerts_preview(ui: &mut egui::Ui, report: &DominionReport) {
    let es_oro = report.fuente.contains("lcu");
    ui.columns(2, |cols| {
        // ---- Gold / Score ----
        egui::Frame::default()
            .fill(PANEL)
            .inner_margin(egui::Margin::same(12))
            .corner_radius(egui::CornerRadius::same(10))
            .stroke(egui::Stroke::new(1.0, ACCENT_SOFT.linear_multiply(0.55)))
            .show(&mut cols[0], |ui| {
                let kind = i18n::tr(if es_oro { "ORO" } else { "SCORE" });
                ui.label(
                    RichText::new(i18n::trf(
                        "⚠  ALERTAS DE {0}  ({1})",
                        &[&kind, &report.breakpoints.len().to_string()],
                    ))
                    .size(16.0)
                    .strong()
                    .color(ACCENT_SOFT),
                );
                ui.add_space(4.0);
                if report.breakpoints.is_empty() {
                    ui.label(
                        RichText::new(i18n::tr("Ninguna ventana superó el piso de alerta."))
                            .size(14.0)
                            .color(MUTED),
                    );
                }
                // Last 6 lines of the chronological log (alerts + objectives).
                let lineas = lineas_cronologicas(report);
                let desde = lineas.len().saturating_sub(6);
                for linea in &lineas[desde..] {
                    match linea {
                        Linea::Objetivo(o) => {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(i18n::tr("◆")).size(14.5).color(GOLD));
                                ui.label(
                                    RichText::new(i18n::trf(
                                        "[{0}]  {1} {2} {3}",
                                        &[
                                            &mmss(o.start_s),
                                            &team_name(o.team),
                                            &objective_verb(&o.tipo),
                                            &objective_name(&o.tipo),
                                        ],
                                    ))
                                    .size(14.5)
                                    .color(team_color(o.team)),
                                );
                            });
                        }
                        Linea::Alerta(q) => {
                            let magnitud = if es_oro {
                                format!("{:+.0}g", q.score_net_blue)
                            } else {
                                format!("{:+.1}", q.score_net_blue)
                            };
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new(i18n::tr("▮"))
                                        .size(14.5)
                                        .color(severity_color(q.severity)),
                                );
                                ui.label(
                                    RichText::new(i18n::trf(
                                        "[{0} – {1}]   {2}   ·   Ventaja a favor del {3}   ·   {4}",
                                        &[
                                            &mmss(q.start_s),
                                            &mmss(q.end_s),
                                            &magnitud,
                                            &team_name(q.favorecido),
                                            &severity_label(q.severity),
                                        ],
                                    ))
                                    .size(14.5)
                                    .color(team_color(q.favorecido)),
                                );
                            });
                        }
                    }
                }
            });

        // ---- Vision ----
        egui::Frame::default()
            .fill(PANEL)
            .inner_margin(egui::Margin::same(12))
            .corner_radius(egui::CornerRadius::same(10))
            .stroke(egui::Stroke::new(1.0, TEAL.linear_multiply(0.55)))
            .show(&mut cols[1], |ui| {
                ui.label(
                    RichText::new(i18n::trf(
                        "👁  ALERTAS DE VISIÓN  ({0})",
                        &[&report.vision.alerts.len().to_string()],
                    ))
                    .size(16.0)
                    .strong()
                    .color(TEAL),
                );
                ui.add_space(4.0);
                if report.vision.alerts.is_empty() {
                    ui.label(
                        RichText::new(i18n::tr("Sin alertas de visión por ahora."))
                            .size(14.0)
                            .color(MUTED),
                    );
                    ui.label(
                        RichText::new(i18n::tr(
                            "Tutorial: reproduce toda la partida en 8x mientras la app graba; así detecta las variaciones de visión.",
                        ))
                        .size(12.5)
                        .color(AMBER),
                    );
                }
                let alerts = sorted_vision_alerts(report);
                let desde = alerts.len().saturating_sub(6);
                for a in &alerts[desde..] {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(i18n::tr("▮"))
                                .size(14.5)
                                .color(severity_color(a.severity)),
                        );
                        ui.label(
                            RichText::new(i18n::trf(
                                "[{0} – {1}]   visión azul {2} · visión roja {3}   ·   Δ {4} a favor del {5}   ·   {6}",
                                &[
                                    &mmss(a.start_s),
                                    &mmss(a.end_s),
                                    &format!("{:.1}", a.delta_azul),
                                    &format!("{:.1}", a.delta_rojo),
                                    &format!("{:.1}", (a.delta_azul - a.delta_rojo).abs()),
                                    &team_name(a.favorecido),
                                    &severity_label(a.severity),
                                ],
                            ))
                            .size(14.5)
                            .color(team_color(a.favorecido)),
                        );
                    });
                }
            });
    });
}

fn top5_column(
    ui: &mut egui::Ui,
    title: &str,
    top: &ScoreTop5,
    color: Color32,
    team: Team,
    es_oro: bool,
) {
    egui::Frame::default()
        .fill(PANEL)
        .inner_margin(egui::Margin::same(12))
        .corner_radius(egui::CornerRadius::same(10))
        .stroke(egui::Stroke::new(1.0, color.linear_multiply(0.55)))
        .show(ui, |ui| {
            ui.label(
                RichText::new(i18n::tr(title))
                    .family(strong_family())
                    .size(15.0)
                    .color(color),
            );
            ui.add_space(2.0);
            ui.label(
                RichText::new(if es_oro {
                    i18n::tr("MEJORES VENTANAS DE ORO")
                } else {
                    i18n::tr("MEJORES VENTANAS")
                })
                .family(strong_family())
                .size(13.0)
                .color(GOLD_BRIGHT),
            );
            ui.add_space(4.0);
            if top.exitos.is_empty() {
                ui.label(RichText::new(i18n::tr("—")).size(14.0).color(MUTED));
            }
            for t in &top.exitos {
                let valor = if es_oro {
                    format!("{:+.0}g", t.net)
                } else {
                    format!("{:+.1}", t.net)
                };
                ui.label(
                    RichText::new(i18n::trf(
                        "[{0} – {1}]   {2} a favor del {3}",
                        &[&mmss(t.start_s), &mmss(t.end_s), &valor, &team_name(team)],
                    ))
                    .size(14.5)
                    .color(color),
                );
            }
        });
}

fn team_name(t: Team) -> String {
    i18n::tr(match t {
        Team::Blue => "AZUL",
        Team::Red => "ROJO",
    })
}

fn severity_label(s: Severity) -> String {
    i18n::tr(s.label())
}

/// Advantage card (score or vision) with a border in the team color.
fn adv_card(ui: &mut egui::Ui, title: &str, headline: &str, detail: &str, color: Color32) {
    egui::Frame::default()
        .fill(PANEL)
        .inner_margin(egui::Margin::same(14))
        .corner_radius(egui::CornerRadius::same(10))
        .stroke(egui::Stroke::new(1.0, color.linear_multiply(0.55)))
        .show(ui, |ui| {
            ui.label(
                RichText::new(i18n::tr(title))
                    .family(strong_family())
                    .size(13.0)
                    .color(GOLD_BRIGHT),
            );
            ui.add_space(3.0);
            ui.label(RichText::new(headline).size(21.0).strong().color(color));
            ui.label(RichText::new(detail).size(14.0).color(MUTED));
        });
}

/// Accumulated score series (Blue − Red) per instant.
///
/// Since the score is permanent (gold × 0.013), the curve is directly the
/// accumulated gold advantage of the game.
fn score_series(report: &DominionReport) -> Vec<(f32, f64)> {
    score::accumulated_score_series(report)
}

/// Accumulated vision series (Δblue − Δred) per instant.
fn vision_series(vision_blocks: &[ScrimBlock]) -> Vec<(f32, f64)> {
    let mut acc = 0.0;
    let mut v = vec![(0.0f32, 0.0f64)];
    for b in vision_blocks {
        acc += b.ward_blue_delta - b.ward_red_delta;
        v.push((b.end_s, acc));
    }
    v
}

/// Accumulated up to instant `t`: (score, Δblue vision, Δred vision).
fn accumulated_at(
    report: &DominionReport,
    vision_blocks: &[ScrimBlock],
    t: f32,
) -> (f64, f64, f64) {
    let score = score::accumulated_score_at(report, t);
    let (mut va, mut vr) = (0.0, 0.0);
    for b in vision_blocks {
        if b.end_s <= t + 0.001 {
            va += b.ward_blue_delta;
            vr += b.ward_red_delta;
        }
    }
    (score, va, vr)
}

/// Total measured duration (seconds).
fn total_duration(report: &DominionReport, vision_blocks: &[ScrimBlock]) -> f32 {
    let a = report.blocks.last().map(|b| b.end_s).unwrap_or(0.0);
    let b = vision_blocks.last().map(|x| x.end_s).unwrap_or(0.0);
    a.max(b).max(0.0)
}

/// Time bar: walks the game in 30 s steps.
/// Returns `true` if the user moved it.
fn time_slider(ui: &mut egui::Ui, cursor: &mut f32, dur: f32) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(i18n::tr("INSTANTE"))
                .family(strong_family())
                .size(14.0)
                .color(GOLD_BRIGHT),
        );
        let resp = ui.add(
            egui::Slider::new(cursor, 0.0..=dur.max(30.0))
                .step_by(30.0)
                .custom_formatter(|v, _| mmss(v as f32))
                .show_value(true),
        );
        changed = resp.changed();
        ui.label(
            RichText::new(format!("→  {}", mmss(*cursor)))
                .size(16.0)
                .strong()
                .color(ACCENT_SOFT),
        );
    });
    changed
}

/// Accumulated advantage **up to the selected instant**: score and vision separately.
///
/// `vision_totals` is the fallback when there is no temporal breakdown: totals
/// measured for the whole game (`wards placed` or `wardScore`). That way the card
/// does not say "no data" when the data exists, even if it cannot be placed in
/// time.
fn advantage_cards(
    ui: &mut egui::Ui,
    report: &DominionReport,
    vision_blocks: &[ScrimBlock],
    cursor_s: f32,
    vision_totals: Option<(f64, f64, String)>,
    vision_note: Option<&str>,
) {
    let (score, vb, vr) = accumulated_at(report, vision_blocks, cursor_s);
    let total_w = vb + vr;
    let when = mmss(cursor_s);
    let is_lcu = report.fuente.contains("lcu");

    ui.columns(2, |cols| {
        let (headline, detail, color, title) = if score > 0.05 {
            (
                i18n::tr("Ventaja a favor del AZUL"),
                if is_lcu {
                    i18n::trf(
                        "{0} de oro hasta {1}",
                        &[&format!("{score:+.0}g"), &when],
                    )
                } else {
                    i18n::trf(
                        "{0} puntos hasta {1}",
                        &[&format!("{score:+.1}"), &when],
                    )
                },
                BLUE,
                if is_lcu {
                    i18n::tr("VENTAJA DE ORO ACUMULADO")
                } else {
                    i18n::tr("SCORE ACUMULADO")
                },
            )
        } else if score < -0.05 {
            (
                i18n::tr("Ventaja a favor del ROJO"),
                if is_lcu {
                    i18n::trf(
                        "{0} de oro hasta {1}",
                        &[&format!("{score:+.0}g"), &when],
                    )
                } else {
                    i18n::trf(
                        "{0} puntos hasta {1}",
                        &[&format!("{score:+.1}"), &when],
                    )
                },
                RED,
                if is_lcu {
                    i18n::tr("VENTAJA DE ORO ACUMULADO")
                } else {
                    i18n::tr("SCORE ACUMULADO")
                },
            )
        } else {
            (
                i18n::tr(if is_lcu {
                    "Empate en oro"
                } else {
                    "Empate en score"
                }),
                if is_lcu {
                    i18n::trf("0g de diferencia acumulada hasta {0}", &[&when])
                } else {
                    i18n::trf("0.0 de diferencia acumulada hasta {0}", &[&when])
                },
                MUTED,
                if is_lcu {
                    i18n::tr("VENTAJA DE ORO ACUMULADO")
                } else {
                    i18n::tr("SCORE ACUMULADO")
                },
            )
        };
        adv_card(
            &mut cols[0],
            &i18n::trf("{0} HASTA {1}", &[&title, &when]),
            &headline,
            &detail,
            color,
        );

        let (headline, detail, color, titulo) = if total_w > 0.0 {
            let diff = vb - vr;
            let titulo = i18n::tr("VISIÓN ACUMULADA HASTA");
            if diff.abs() < 1.0 {
                (
                    i18n::tr("Paridad de visión"),
                    i18n::trf(
                        "visión azul {0} · visión roja {1} hasta {2}",
                        &[&format!("{vb:.0}"), &format!("{vr:.0}"), &when],
                    ),
                    TEAL,
                    titulo,
                )
            } else if diff > 0.0 {
                (
                    i18n::tr("Visión a favor del AZUL"),
                    i18n::trf(
                        "visión azul {0} · visión roja {1}   ·   Δ +{2} a favor del AZUL",
                        &[&format!("{vb:.0}"), &format!("{vr:.0}"), &format!("{diff:.0}")],
                    ),
                    BLUE,
                    titulo,
                )
            } else {
                (
                    i18n::tr("Visión a favor del ROJO"),
                    i18n::trf(
                        "visión azul {0} · visión roja {1}   ·   Δ {2} a favor del ROJO",
                        &[
                            &format!("{vb:.0}"),
                            &format!("{vr:.0}"),
                            &format!("{:.0}", diff.abs()),
                        ],
                    ),
                    RED,
                    titulo,
                )
            }
        } else if let Some((tb, tr, unidad)) = vision_totals.filter(|(b, r, _)| b + r > 0.0) {
            let diff = tb - tr;
            let color = if diff.abs() < 0.5 {
                TEAL
            } else if diff > 0.0 {
                BLUE
            } else {
                RED
            };
            (
                i18n::tr("Visión total de la partida"),
                i18n::trf(
                    "azul {0} · rojo {1} {2}   ·   Δ {3} a favor del {4} (sin desglose por ventana)",
                    &[
                        &format!("{tb:.0}"),
                        &format!("{tr:.0}"),
                        &unidad,
                        &format!("{diff:+.0}"),
                        &i18n::tr(if diff >= 0.0 { "AZUL" } else { "ROJO" }),
                    ],
                ),
                color,
                i18n::tr("VISIÓN TOTAL (SIN DESGLOSE TEMPORAL)"),
            )
        } else {
            (
                i18n::tr("Sin datos de visión"),
                i18n::tr("Esta captura no trae actividad de wards"),
                MUTED,
                i18n::tr("VISIÓN ACUMULADA HASTA"),
            )
        };
        let titulo_card = if titulo == i18n::tr("VISIÓN TOTAL (SIN DESGLOSE TEMPORAL)") {
            titulo.to_string()
        } else {
            i18n::trf("{0} {1}", &[&titulo, &when])
        };
        adv_card(
            &mut cols[1],
            &titulo_card,
            &headline,
            &detail,
            color,
        );
        if let Some(note) = vision_note {
            cols[1].label(RichText::new(note).size(12.0).color(MUTED));
        }
    });
}

/// Accumulated advantage chart: blue above the tie, red below.
///
/// `serie` are the points `(second, accumulated value)`. The vertical gold line
/// marks the instant the user has selected with the time bar.
fn accumulated_chart(
    ui: &mut egui::Ui,
    serie: &[(f32, f64)],
    dur: f32,
    height: f32,
    cursor_s: Option<f32>,
    unidad: &str,
) {
    let width = ui.available_width().max(240.0);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, egui::CornerRadius::same(8), INSET);
    painter.rect_stroke(
        rect,
        egui::CornerRadius::same(8),
        Stroke::new(1.0, ACCENT.linear_multiply(0.55)),
        egui::StrokeKind::Inside,
    );

    if serie.len() < 2 {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            i18n::tr("Aún no hay ventanas medidas"),
            egui::FontId::proportional(15.0),
            MUTED,
        );
        return;
    }

    let pad_l = 54.0;
    let pad_r = 20.0;
    let pad_t = 22.0;
    let pad_b = 26.0;
    let plot = Rect::from_min_max(
        Pos2::new(rect.left() + pad_l, rect.top() + pad_t),
        Pos2::new(rect.right() - pad_r, rect.bottom() - pad_b),
    );
    let mid = plot.center().y;
    let half = (plot.height() * 0.5).max(1.0);

    let dur = dur.max(serie.last().map(|p| p.0).unwrap_or(0.0)).max(1.0);
    let max_abs = serie
        .iter()
        .map(|p| p.1.abs())
        .fold(0.0f64, f64::max)
        .max(500.0);
    // Scale in "round" steps for gold (500, 1000, 2000, 5000…).
    let step = {
        let raw = max_abs / 2.0;
        let pow = 10f64.powf(raw.log10().floor());
        let m = raw / pow;
        let nice = if m <= 1.0 {
            1.0
        } else if m <= 2.0 {
            2.0
        } else if m <= 5.0 {
            5.0
        } else {
            10.0
        };
        // Snap to realistic gold steps
        let base = nice * pow;
        if base < 500.0 { 500.0 } else { base }
    };
    let scale = step * 2.0;

    let x_of = |s: f32| plot.left() + (s / dur).clamp(0.0, 1.0) * plot.width();
    let y_of = |v: f64| mid - ((v / scale).clamp(-1.0, 1.0) as f32) * half;

    // Horizontal grid: the tie line is gold (it is the reference).
    let grid = Stroke::new(1.0, ACCENT.linear_multiply(0.18));
    for f in [-1.0f64, -0.5, 0.0, 0.5, 1.0] {
        let v = f * scale;
        let y = y_of(v);
        painter.line_segment(
            [Pos2::new(plot.left(), y), Pos2::new(plot.right(), y)],
            if f == 0.0 {
                Stroke::new(1.6, GOLD.linear_multiply(0.75))
            } else {
                grid
            },
        );
        painter.text(
            Pos2::new(plot.left() - 8.0, y),
            egui::Align2::RIGHT_CENTER,
            format!("{v:+.0}"),
            egui::FontId::proportional(12.0),
            if f == 0.0 { GOLD_BRIGHT } else { MUTED },
        );
    }

    // Vertical grid per minute.
    let total_min = (dur / 60.0).ceil().max(1.0) as u32;
    let label_every = if plot.width() / total_min as f32 >= 46.0 {
        1
    } else if total_min <= 20 {
        2
    } else {
        5
    };
    for m in 0..=total_min {
        let x = x_of(m as f32 * 60.0);
        painter.line_segment(
            [Pos2::new(x, plot.top()), Pos2::new(x, plot.bottom())],
            grid,
        );
        if m % label_every == 0 {
            painter.text(
                Pos2::new(x, plot.bottom() + 4.0),
                egui::Align2::CENTER_TOP,
                format!("{m}m"),
                egui::FontId::proportional(12.0),
                MUTED,
            );
        }
    }

    // Blue fill above the tie, red below.
    let fill = |a: (f32, f64), b: (f32, f64), color: Color32| {
        let p0 = Pos2::new(x_of(a.0), y_of(a.1));
        let p1 = Pos2::new(x_of(b.0), y_of(b.1));
        painter.add(egui::Shape::convex_polygon(
            vec![
                Pos2::new(p0.x, mid),
                p0,
                p1,
                Pos2::new(p1.x, mid),
            ],
            color,
            Stroke::NONE,
        ));
    };
    let blue_fill = BLUE.linear_multiply(0.26);
    let red_fill = RED.linear_multiply(0.26);
    for w in serie.windows(2) {
        let (a, b) = (w[0], w[1]);
        if a.1 >= 0.0 && b.1 >= 0.0 {
            fill(a, b, blue_fill);
        } else if a.1 <= 0.0 && b.1 <= 0.0 {
            fill(a, b, red_fill);
        } else if (a.1 - b.1).abs() > f64::EPSILON {
            // Tie crossing: split the segment at the zero point.
            let t = (a.1 / (a.1 - b.1)).abs();
            let cross = (a.0 + (b.0 - a.0) * t as f32, 0.0);
            if a.1 > 0.0 {
                fill(a, cross, blue_fill);
                fill(cross, b, red_fill);
            } else {
                fill(a, cross, red_fill);
                fill(cross, b, blue_fill);
            }
        }
    }

    // Series line.
    let line: Vec<Pos2> = serie
        .iter()
        .map(|p| Pos2::new(x_of(p.0), y_of(p.1)))
        .collect();
    painter.add(egui::Shape::line(
        line,
        Stroke::new(2.0, GOLD_BRIGHT.linear_multiply(0.95)),
    ));

    // Team labels and current value.
    painter.text(
        Pos2::new(plot.left() + 6.0, plot.top() + 2.0),
        egui::Align2::LEFT_TOP,
        i18n::tr("AZUL"),
        egui::FontId::proportional(13.0),
        BLUE,
    );
    painter.text(
        Pos2::new(plot.left() + 6.0, plot.bottom() - 2.0),
        egui::Align2::LEFT_BOTTOM,
        i18n::tr("ROJO"),
        egui::FontId::proportional(13.0),
        RED,
    );
    let last = *serie.last().unwrap_or(&(0.0, 0.0));
    let (now_txt, now_col) = if last.1.abs() < 0.05 {
        (i18n::tr("EMPATE"), MUTED)
    } else if last.1 > 0.0 {
        (format!("+{:.1} · AZUL", last.1), BLUE)
    } else {
        (format!("{:.1} · ROJO", last.1), RED)
    };
    painter.text(
        Pos2::new(plot.right(), plot.top() + 2.0),
        egui::Align2::RIGHT_TOP,
        now_txt,
        egui::FontId::proportional(14.0),
        now_col,
    );

    // Cursor: the instant selected with the time bar.
    if let Some(c) = cursor_s {
        let x = x_of(c);
        painter.line_segment(
            [Pos2::new(x, plot.top()), Pos2::new(x, plot.bottom())],
            Stroke::new(1.8, GOLD_BRIGHT),
        );
        painter.circle_filled(Pos2::new(x, plot.top() + 3.0), 4.5, GOLD_BRIGHT);
    }

    if resp.hovered() {
        if let Some(pos) = resp.hover_pos() {
            let mut best = (0usize, f32::INFINITY);
            for (i, p) in serie.iter().enumerate() {
                let d = (x_of(p.0) - pos.x).abs();
                if d < best.1 {
                    best = (i, d);
                }
            }
            let p = serie[best.0];
            let who = if p.1 > 0.05 {
                i18n::tr("ventaja a favor del AZUL")
            } else if p.1 < -0.05 {
                i18n::tr("ventaja a favor del ROJO")
            } else {
                i18n::tr("empate")
            };
            let c = if p.1 >= 0.0 { BLUE } else { RED };
            let hover = Pos2::new(x_of(p.0), y_of(p.1));
            painter.circle_filled(hover, 4.0, c);
            painter.circle_stroke(hover, 4.0, Stroke::new(1.0, TEXT));
            let unit = i18n::tr(unidad);
            resp.on_hover_text(i18n::trf(
                "{0}  ·  {1} acumulado {2}\n{3}",
                &[
                    &mmss(p.0),
                    &unit,
                    &format!("{:+.1}", p.1),
                    &who,
                ],
            ));
        }
    }
}

/// Accumulated score advantage chart.
fn score_chart(
    ui: &mut egui::Ui,
    report: &DominionReport,
    dur: f32,
    height: f32,
    cursor_s: f32,
) {
    let width = ui.available_width().max(240.0);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, egui::CornerRadius::same(8), INSET);
    painter.rect_stroke(
        rect,
        egui::CornerRadius::same(8),
        Stroke::new(1.0, ACCENT.linear_multiply(0.55)),
        egui::StrokeKind::Inside,
    );

    let serie = score_series(report);
    if serie.len() < 2 {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            i18n::tr("Aún no hay ventanas medidas"),
            egui::FontId::proportional(15.0),
            MUTED,
        );
        return;
    }

    let pad_l = 54.0;
    let pad_r = 20.0;
    let pad_t = 22.0;
    let pad_b = 26.0;
    let plot = Rect::from_min_max(
        Pos2::new(rect.left() + pad_l, rect.top() + pad_t),
        Pos2::new(rect.right() - pad_r, rect.bottom() - pad_b),
    );
    let mid = plot.center().y;
    let half = (plot.height() * 0.5).max(1.0);

    let dur = dur.max(serie.last().map(|p| p.0).unwrap_or(0.0)).max(1.0);
    let max_abs = serie
        .iter()
        .map(|p| p.1.abs())
        .fold(0.0f64, f64::max)
        .max(500.0);
    let step = {
        let raw = max_abs / 2.0;
        let pow = 10f64.powf(raw.log10().floor());
        let m = raw / pow;
        let nice = if m <= 1.0 {
            1.0
        } else if m <= 2.0 {
            2.0
        } else if m <= 5.0 {
            5.0
        } else {
            10.0
        };
        let base = nice * pow;
        if base < 500.0 { 500.0 } else { base }
    };
    let scale = step * 2.0;

    let x_of = |s: f32| plot.left() + (s / dur).clamp(0.0, 1.0) * plot.width();
    let y_of = |v: f64| mid - ((v / scale).clamp(-1.0, 1.0) as f32) * half;

    // Horizontal grid
    let grid = Stroke::new(1.0, ACCENT.linear_multiply(0.18));
    for f in [-1.0f64, -0.5, 0.0, 0.5, 1.0] {
        let v = f * scale;
        let y = y_of(v);
        painter.line_segment(
            [Pos2::new(plot.left(), y), Pos2::new(plot.right(), y)],
            if f == 0.0 {
                Stroke::new(1.6, GOLD.linear_multiply(0.75))
            } else {
                grid
            },
        );
        painter.text(
            Pos2::new(plot.left() - 8.0, y),
            egui::Align2::RIGHT_CENTER,
            format!("{v:+.0}"),
            egui::FontId::proportional(12.0),
            if f == 0.0 { GOLD_BRIGHT } else { MUTED },
        );
    }

    // Vertical grid
    let total_min = (dur / 60.0).ceil().max(1.0) as u32;
    let label_every = if plot.width() / total_min as f32 >= 46.0 {
        1
    } else if total_min <= 20 {
        2
    } else {
        5
    };
    for m in 0..=total_min {
        let x = x_of(m as f32 * 60.0);
        painter.line_segment(
            [Pos2::new(x, plot.top()), Pos2::new(x, plot.bottom())],
            grid,
        );
        if m % label_every == 0 {
            painter.text(
                Pos2::new(x, plot.bottom() + 4.0),
                egui::Align2::CENTER_TOP,
                format!("{m}m"),
                egui::FontId::proportional(12.0),
                MUTED,
            );
        }
    }

    // Blue/red fill
    let fill = |a: (f32, f64), b: (f32, f64), color: Color32| {
        let p0 = Pos2::new(x_of(a.0), y_of(a.1));
        let p1 = Pos2::new(x_of(b.0), y_of(b.1));
        painter.add(egui::Shape::convex_polygon(
            vec![
                Pos2::new(p0.x, mid),
                p0,
                p1,
                Pos2::new(p1.x, mid),
            ],
            color,
            Stroke::NONE,
        ));
    };
    let blue_fill = BLUE.linear_multiply(0.26);
    let red_fill = RED.linear_multiply(0.26);
    for w in serie.windows(2) {
        let (a, b) = (w[0], w[1]);
        if a.1 >= 0.0 && b.1 >= 0.0 {
            fill(a, b, blue_fill);
        } else if a.1 <= 0.0 && b.1 <= 0.0 {
            fill(a, b, red_fill);
        } else if (a.1 - b.1).abs() > f64::EPSILON {
            let t = (a.1 / (a.1 - b.1)).abs();
            let cross = (a.0 + (b.0 - a.0) * t as f32, 0.0);
            if a.1 > 0.0 {
                fill(a, cross, blue_fill);
                fill(cross, b, red_fill);
            } else {
                fill(a, cross, red_fill);
                fill(cross, b, blue_fill);
            }
        }
    }

    // Series line
    let line: Vec<Pos2> = serie
        .iter()
        .map(|p| Pos2::new(x_of(p.0), y_of(p.1)))
        .collect();
    painter.add(egui::Shape::line(
        line,
        Stroke::new(2.0, GOLD_BRIGHT.linear_multiply(0.95)),
    ));

    // Detection and drawing of EPIC OBJECTIVES as exact breaks in the chart
    for o in &report.avisos {
        let ox = x_of(o.start_s);
        let obj_col = team_color(o.team);
        // Subtle vertical line with a gradient (more transparent in the center)
        let mid_y = plot.center().y;
        let half_h = plot.height() * 0.5;
        // Top part
        painter.line_segment(
            [Pos2::new(ox, plot.top()), Pos2::new(ox, mid_y - half_h * 0.3)],
            Stroke::new(1.0, obj_col.linear_multiply(0.4)),
        );
        // Bottom part
        painter.line_segment(
            [Pos2::new(ox, mid_y + half_h * 0.3), Pos2::new(ox, plot.bottom())],
            Stroke::new(1.0, obj_col.linear_multiply(0.4)),
        );
        // Distinctive badge with the initial
        let badge_y = if o.team == Team::Blue { plot.top() + 10.0 } else { plot.bottom() - 10.0 };
        painter.circle_filled(Pos2::new(ox, badge_y), 7.0, obj_col);
        painter.circle_stroke(Pos2::new(ox, badge_y), 7.0, Stroke::new(1.5, TEXT));
        let badge_char = match o.tipo.as_str() {
            "dragon" => "D",
            "baron" => "B",
            "herald" => "H",
            "voidgrub" => "V",
            _ => "O",
        };
        painter.text(
            Pos2::new(ox, badge_y),
            egui::Align2::CENTER_CENTER,
            badge_char,
            egui::FontId::proportional(9.0),
            Color32::WHITE,
        );
    }

    // BLUE / RED labels
    painter.text(
        Pos2::new(plot.left() + 6.0, plot.top() + 2.0),
        egui::Align2::LEFT_TOP,
        i18n::tr("AZUL"),
        egui::FontId::proportional(13.0),
        BLUE,
    );
    painter.text(
        Pos2::new(plot.left() + 6.0, plot.bottom() - 2.0),
        egui::Align2::LEFT_BOTTOM,
        i18n::tr("ROJO"),
        egui::FontId::proportional(13.0),
        RED,
    );

    // Cursor
    let cx = x_of(cursor_s);
    painter.line_segment(
        [Pos2::new(cx, plot.top()), Pos2::new(cx, plot.bottom())],
        Stroke::new(1.8, GOLD_BRIGHT),
    );
    painter.circle_filled(Pos2::new(cx, plot.top() + 3.0), 4.5, GOLD_BRIGHT);

    if resp.hovered() {
        if let Some(pos) = resp.hover_pos() {
            let mut best = (0usize, f32::INFINITY);
            for (i, p) in serie.iter().enumerate() {
                let d = (x_of(p.0) - pos.x).abs();
                if d < best.1 {
                    best = (i, d);
                }
            }
            let p = serie[best.0];
            let who = if p.1 > 0.05 {
                i18n::tr("ventaja a favor del AZUL")
            } else if p.1 < -0.05 {
                i18n::tr("ventaja a favor del ROJO")
            } else {
                i18n::tr("empate")
            };
            let c = if p.1 >= 0.0 { BLUE } else { RED };
            let hover = Pos2::new(x_of(p.0), y_of(p.1));
            painter.circle_filled(hover, 4.0, c);
            painter.circle_stroke(hover, 4.0, Stroke::new(1.0, TEXT));

            // Tooltip if hovering over an epic objective
            let near_obj = report.avisos.iter().find(|o| (x_of(o.start_s) - pos.x).abs() <= 6.0);
            let is_lcu = report.fuente.contains("lcu");
            if let Some(o) = near_obj {
                if is_lcu {
                    resp.on_hover_text(i18n::trf(
                        "{0}  ·  OBJETIVO ÉPICO: {1} ({2})\nOro acumulado {3}g\n{4}",
                        &[
                            &mmss(o.start_s),
                            &objective_name(&o.tipo),
                            &team_name(o.team),
                            &format!("{:+.0}", p.1),
                            &who,
                        ],
                    ));
                } else {
                    resp.on_hover_text(i18n::trf(
                        "{0}  ·  OBJETIVO ÉPICO: {1} ({2})\nScore acumulado {3}\n{4}",
                        &[
                            &mmss(o.start_s),
                            &objective_name(&o.tipo),
                            &team_name(o.team),
                            &format!("{:+.1}", p.1),
                            &who,
                        ],
                    ));
                }
            } else if is_lcu {
                resp.on_hover_text(i18n::trf(
                    "{0}  ·  oro acumulado {1}g\n{2}",
                    &[&mmss(p.0), &format!("{:+.0}", p.1), &who],
                ));
            } else {
                resp.on_hover_text(i18n::trf(
                    "{0}  ·  score acumulado {1}\n{2}",
                    &[&mmss(p.0), &format!("{:+.1}", p.1), &who],
                ));
            }
        }
    }
}


/// Accumulated vision advantage chart (standalone module).
fn vision_chart(
    ui: &mut egui::Ui,
    vision_blocks: &[ScrimBlock],
    dur: f32,
    height: f32,
    cursor_s: f32,
    totals: Option<(f64, f64, String)>,
) {
    let serie = vision_series(vision_blocks);
    if serie.iter().all(|(_, v)| v.abs() < 1e-9) {
        egui::Frame::default()
            .fill(INSET)
            .inner_margin(egui::Margin::same(14))
            .corner_radius(egui::CornerRadius::same(8))
            .stroke(egui::Stroke::new(1.0, ACCENT.linear_multiply(0.55)))
            .show(ui, |ui| {
                if let Some((b, r, unidad)) = totals.filter(|(b, r, _)| b + r > 0.0) {
                    ui.label(
                        RichText::new(i18n::tr("Visión total de la partida"))
                            .size(15.0)
                            .strong()
                            .color(TEAL),
                    );
                    ui.label(
                        RichText::new(i18n::trf(
                            "azul {0} · rojo {1} {2}   ·   Δ {3} a favor del {4}",
                            &[
                                &format!("{b:.0}"),
                                &format!("{r:.0}"),
                                &unidad,
                                &format!("{:+.0}", b - r),
                                &i18n::tr(if b >= r { "AZUL" } else { "ROJO" }),
                            ],
                        ))
                        .size(14.0)
                        .color(TEXT),
                    );
                    ui.label(
                        RichText::new(i18n::tr(
                            "El timeline del LCU no expone las wards por minuto; no hay curva que dibujar.",
                        ))
                        .size(12.5)
                        .color(MUTED),
                    );
                } else {
                    ui.label(
                        RichText::new(i18n::tr("Sin datos de visión en esta captura"))
                            .size(15.0)
                            .color(MUTED),
                    );
                    ui.label(
                        RichText::new(i18n::tr("No hay actividad de wards que graficar."))
                            .size(13.5)
                            .color(MUTED),
                    );
                }
            });
        return;
    }
    accumulated_chart(
        ui,
        &serie,
        dur,
        height,
        Some(cursor_s),
        "visión",
    );
}


/// Criticality legend: explains what each alert level means and how much
/// it weighs, both in gold/score and in vision.
fn severity_legend(
    ui: &mut egui::Ui,
    params: &ScoreParams,
    vt: &VisionThresholds,
    es_oro: bool,
) {
    let st = &params.thresholds;
    let intervalo = |a: f64, b: f64| -> String {
        if es_oro {
            format!("[{a:.0}g, {b:.0}g)")
        } else {
            format!("[{a:.1}, {b:.1}) pts")
        }
    };
    egui::Frame::default()
        .fill(PANEL)
        .inner_margin(egui::Margin::same(14))
        .corner_radius(egui::CornerRadius::same(12))
        .stroke(egui::Stroke::new(1.0, GOLD.linear_multiply(0.55)))
        .show(ui, |ui| {
            ui.label(
                RichText::new(i18n::tr("ESCALA DE CRITICIDAD DE LAS ALERTAS"))
                    .family(strong_family())
                    .size(15.0)
                    .color(GOLD_BRIGHT),
            );
            ui.label(
                RichText::new(if es_oro {
                    i18n::tr("Las alertas se miden exclusivamente por el ORO NETO ganado en cada ventana de 30 s: 500 g ya es una ventaja leve, 1500 g es crítica. Sin puntos abstractos.")
                } else {
                    i18n::tr("El score mide la ventaja de cada ventana de 30 s. Los cortes son fijos: la misma ventaja vale igual en el minuto 4 que en el 25.")
                })
                .size(13.5)
                .color(MUTED),
            );
            ui.add_space(10.0);
            let rows: [(Severity, String, Option<f64>); 4] = [
                (
                    Severity::VentajaMinima,
                    intervalo(st.semi_alerta, st.ventaja_media),
                    Some(vt.semi_alerta),
                ),
                (
                    Severity::VentajaMedia,
                    intervalo(st.ventaja_media, st.ventaja_considerable),
                    Some(vt.ventaja_media),
                ),
                (
                    Severity::VentajaConsiderable,
                    intervalo(st.ventaja_considerable, st.ventaja_critica),
                    Some(vt.ventaja_considerable),
                ),
                (
                    Severity::VentajaCritica,
                    if es_oro {
                        format!("≥ {:.0}g", st.ventaja_critica)
                    } else {
                        format!("≥ {:.1} pts", st.ventaja_critica)
                    },
                    Some(vt.ventaja_critica),
                ),
            ];
            ui.columns(4, |cols| {
                for ((sev, intervalo, vis_min), col) in rows.iter().zip(cols.iter_mut()) {
                    let c = severity_color(*sev);
                    egui::Frame::default()
                        .fill(PANEL2)
                        .inner_margin(egui::Margin::same(10))
                        .corner_radius(egui::CornerRadius::same(8))
                        .stroke(egui::Stroke::new(1.0, c.linear_multiply(0.6)))
                        .show(col, |ui| {
                            let w = ui.available_width();
                            ui.set_min_width(w);
                            ui.label(
                                RichText::new(format!("●  {}", severity_label(*sev)))
                                    .size(15.0)
                                    .strong()
                                    .color(c),
                            );
                            ui.label(
                                RichText::new(match vis_min {
                                    Some(v) => i18n::trf(
                                        "{0}   ·   Visión ≥{1}",
                                        &[&intervalo, &format!("{v:.2}")],
                                    ),
                                    None => i18n::trf(
                                        "{0}   ·   Visión —",
                                        &[&intervalo],
                                    ),
                                })
                                .size(13.0)
                                .color(ACCENT_SOFT),
                            );
                        });
                }
            });
            ui.add_space(10.0);
            ui.label(
                RichText::new(if es_oro {
                    i18n::tr("El desglose de cada alerta (kills, CS, torres y objetivos) suma el 100 % del oro neto de la ventana; las placas no detectadas van a «Oro extra».")
                } else {
                    i18n::trf(
                        "Puntos por acción: kill {0} · torre {1} · placa {2} · dragón {3} (buff {4}) · heraldo {5} (buff {6}) · barón {7} (buff {8}) · CS {9}   |   los buffs NO suman al score: se informan aparte",
                        &[
                            &format!("{:.1}", params.kills),
                            &format!("{:.1}", params.tower),
                            &format!("{:.1}", params.plate),
                            &format!("{:.1}", params.dragon),
                            &mmss(params.dragon_buff_dur_s),
                            &format!("{:.1}", params.herald),
                            &mmss(params.herald_buff_dur_s),
                            &format!("{:.1}", params.baron),
                            &mmss(params.baron_buff_dur_s),
                            &format!("{:.2}", params.cs),
                        ],
                    )
                })
                .size(12.5)
                .color(MUTED),
            );
        });
}

/// Saves the analyst annotations to their own file (`.md` / `.txt`).
fn guardar_anotaciones(notas: &mut Anotaciones) {
    let dir = captures_dir();
    let _ = std::fs::create_dir_all(&dir);
    let Some(ruta) = rfd::FileDialog::new()
        .set_title(i18n::tr("Guardar anotaciones de la scrim"))
        .set_directory(&dir)
        .set_file_name("notas_scrim.md")
        .add_filter(i18n::tr("Texto"), &["md", "txt"])
        .save_file()
    else {
        return;
    };
    match std::fs::write(&ruta, &notas.texto) {
        Ok(()) => {
            notas.ruta = Some(ruta.display().to_string());
            notas.mensaje = Some(i18n::tr("Notas guardadas"));
        }
        Err(e) => notas.mensaje = Some(i18n::trf("No se pudieron guardar: {0}", &[&e.to_string()])),
    }
}

/// Loads the annotations from a file.
fn cargar_anotaciones(notas: &mut Anotaciones) {
    let dir = captures_dir();
    let mut dlg = rfd::FileDialog::new()
        .set_title(i18n::tr("Cargar anotaciones de la scrim"))
        .add_filter(i18n::tr("Texto"), &["md", "txt"]);
    if dir.is_dir() {
        dlg = dlg.set_directory(&dir);
    }
    let Some(ruta) = dlg.pick_file() else {
        return;
    };
    match std::fs::read_to_string(&ruta) {
        Ok(t) => {
            notas.texto = t;
            notas.ruta = Some(ruta.display().to_string());
            notas.mensaje = Some(i18n::tr("Notas cargadas"));
        }
        Err(e) => notas.mensaje = Some(i18n::trf("No se pudieron leer: {0}", &[&e.to_string()])),
    }
}

/// "Save as…" dialog for the capture JSON log.
fn save_json_dialog(json: &str, dir: &Path, default_name: &str) -> Option<(bool, String)> {
    let _ = std::fs::create_dir_all(dir);
    let name = format!("{}.json", sanitize_file_stem(default_name));
    let path = rfd::FileDialog::new()
        .set_title(i18n::tr("Guardar registro del scrim"))
        .set_directory(dir)
        .set_file_name(name)
        .add_filter("JSON", &["json"])
        .save_file()?;
    match std::fs::write(&path, json) {
        Ok(()) => Some((
            true,
            i18n::trf("Guardado en {0}", &[&path.display().to_string()]),
        )),
        Err(e) => Some((false, i18n::trf("No se pudo guardar: {0}", &[&e.to_string()]))),
    }
}

// ---------------------------------------------------------------------------
// Report screen
// ---------------------------------------------------------------------------

/// Vision dashboard in **2-minute** windows: vision points
/// (`wardScore`) for each team and who held the advantage.
///
/// The series comes from the 30 s blocks measured by the live poller. The
/// LCU timeline does not expose `wardScore` per minute in replays; when there
/// is no series it is stated explicitly instead of inventing values.
#[allow(dead_code)]
fn vision_dashboard_old(ui: &mut egui::Ui, vision_blocks: &[ScrimBlock], dur: f32) {
    const W: f32 = 120.0;
    let fuente = vision_blocks
        .iter()
        .map(|b| b.vision_source.as_str())
        .find(|s| !s.is_empty())
        .unwrap_or("");
    let es_ward_score = fuente == "wardScore";
    let metrica = i18n::tr(if es_ward_score {
        "Puntos de visión (wardScore) por equipo en cada ventana de 2 minutos."
    } else if fuente.is_empty() {
        "Puntos de visión por equipo en cada ventana de 2 minutos."
    } else {
        "Wards colocadas por equipo en cada ventana de 2 minutos (el cliente no expuso wardScore)."
    });
    egui::Frame::default()
        .fill(PANEL)
        .inner_margin(egui::Margin::same(12))
        .corner_radius(egui::CornerRadius::same(10))
        .stroke(egui::Stroke::new(1.0, TEAL.linear_multiply(0.55)))
        .show(ui, |ui| {
            ui.label(
                RichText::new(i18n::tr("👁  DASHBOARD DE VISIÓN (cada 2 min)"))
                    .family(strong_family())
                    .size(15.0)
                    .color(TEAL),
            );
            ui.label(RichText::new(metrica).size(12.5).color(MUTED));
            ui.add_space(6.0);

            let n = ((dur.max(W) / W).ceil() as usize).max(1);
            let mut blue = vec![0.0f64; n];
            let mut red = vec![0.0f64; n];
            let mut con_datos = false;
            for b in vision_blocks {
                let idx = (b.start_s / W).floor() as usize;
                if idx >= n {
                    continue;
                }
                blue[idx] += b.ward_blue_delta;
                red[idx] += b.ward_red_delta;
                if b.ward_blue_delta.abs() + b.ward_red_delta.abs() > 0.0 {
                    con_datos = true;
                }
            }

            if !con_datos {
                egui::Frame::default()
                    .fill(INSET)
                    .inner_margin(egui::Margin::same(10))
                    .corner_radius(egui::CornerRadius::same(6))
                    .show(ui, |ui| {
                        ui.label(
                            RichText::new(i18n::tr("TUTORIAL: cómo obtener visión por minuto"))
                                .size(13.5)
                                .strong()
                                .color(AMBER),
                        );
                        ui.label(
                            RichText::new(i18n::tr(
                                "1) Abre el replay en League. 2) Déjalo reproducir completo, puedes poner 8x. 3) La app graba `wardScore` en segundo plano. 4) Al cerrarlo, los datos se fusionan solos con este informe.",
                            ))
                            .size(12.5)
                            .color(TEXT),
                        );
                        ui.add_space(3.0);
                        ui.label(
                            RichText::new(i18n::tr(
                                "El timeline del LCU no expone `wardScore` por minuto y el .rofl sólo da el total. Sin reproducción en vivo no hay serie que mostrar.",
                            ))
                            .size(11.5)
                            .color(MUTED),
                        );
                    });
                return;
            }

            egui::Grid::new("vision_dashboard_grid")
                .num_columns(4)
                .spacing([10.0, 4.0])
                .striped(true)
                .show(ui, |ui| {
                    for h in ["VENTANA", "AZUL", "ROJO", "VENTAJA"] {
                        ui.label(
                            RichText::new(h)
                                .size(11.5)
                                .strong()
                                .color(GOLD_BRIGHT),
                        );
                    }
                    ui.end_row();
                    for i in 0..n {
                        let (b, r) = (blue[i], red[i]);
                        let start = i as f32 * W;
                        let end = ((i + 1) as f32 * W).min(dur.max(W));
                        ui.label(
                            RichText::new(format!("[{} – {}]", mmss(start), mmss(end)))
                                .size(12.0)
                                .color(TEXT),
                        );
                        let fmt_v = |v: f64| {
                            if es_ward_score {
                                format!("{v:.1}")
                            } else {
                                format!("{v:.0}")
                            }
                        };
                        ui.label(
                            RichText::new(fmt_v(b))
                                .size(12.5)
                                .strong()
                                .color(BLUE),
                        );
                        ui.label(
                            RichText::new(fmt_v(r))
                                .size(12.5)
                                .strong()
                                .color(RED),
                        );
                        let diff = b - r;
                        let (txt, col) = if b.abs() < 0.001 && r.abs() < 0.001 {
                            ("—".to_string(), MUTED)
                        } else if diff.abs() < 0.05 {
                            ("PARIDAD".to_string(), MUTED)
                        } else if diff > 0.0 {
                            (format!("AZUL +{diff:.1}"), BLUE)
                        } else {
                            (format!("ROJO +{:.1}", diff.abs()), RED)
                        };
                        ui.label(RichText::new(txt).size(12.0).color(col));
                        ui.end_row();
                    }
                });
            ui.add_space(4.0);
            ui.label(
                RichText::new(
                    "Diferencia = visión del AZUL − del ROJO en la ventana. Las ventanas sin actividad aparecen con «—».",
                )
                .size(11.0)
                .color(MUTED),
            );

            ui.add_space(8.0);
            ui.separator();
            ui.label(
                RichText::new(i18n::tr("MEJORES VENTANAS DE VISIÓN"))
                    .family(strong_family())
                    .size(13.5)
                    .color(GOLD_BRIGHT),
            );
            ui.label(
                RichText::new(i18n::tr("Ordenadas de mejor a peor para cada equipo (ventanas de 2 min)."))
                    .size(11.0)
                    .color(MUTED),
            );
            let mut orden: Vec<usize> = (0..n)
                .filter(|&i| blue[i].abs() + red[i].abs() > 0.0)
                .collect();
            let diff_at = |i: usize| blue[i] - red[i];
            orden.sort_by(|&a, &b| {
                diff_at(b)
                    .partial_cmp(&diff_at(a))
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            let mut orden_rojo = orden.clone();
            orden_rojo.reverse();
            let top = 5usize;
            let fmt_v = |v: f64| {
                if es_ward_score {
                    format!("{v:.1}")
                } else {
                    format!("{v:.0}")
                }
            };
            let show_list = |col: &mut egui::Ui, titulo: &str, indices: &[usize]| {
                col.label(
                    RichText::new(titulo)
                        .size(12.0)
                        .strong()
                        .color(if titulo == "AZUL" { BLUE } else { RED }),
                );
                if indices.is_empty() {
                    col.label(RichText::new(i18n::tr("—")).size(12.0).color(MUTED));
                    return;
                }
                for &i in indices.iter().take(top) {
                    let start = i as f32 * W;
                    let end = ((i + 1) as f32 * W).min(dur.max(W));
                    let d = if titulo == "AZUL" {
                        blue[i] - red[i]
                    } else {
                        red[i] - blue[i]
                    };
                    col.label(
                        RichText::new(format!(
                            "[{} – {}]   +{}",
                            mmss(start),
                            mmss(end),
                            fmt_v(d.abs())
                        ))
                        .size(11.5)
                        .color(if titulo == "AZUL" { BLUE } else { RED }),
                    );
                }
            };
            ui.columns(2, |cols| {
                show_list(&mut cols[0], "AZUL", &orden);
                show_list(&mut cols[1], "ROJO", &orden_rojo);
            });
        });
}

/// Vision dashboard: 2 min table on the left and best windows for
/// each team on the right (using the side gap).
fn vision_dashboard(ui: &mut egui::Ui, vision_blocks: &[ScrimBlock], dur: f32) {
    const W: f32 = 120.0;
    let fuente = vision_blocks
        .iter()
        .map(|b| b.vision_source.as_str())
        .find(|s| !s.is_empty())
        .unwrap_or("");
    let es_ward_score = fuente == "wardScore";
    let metrica = i18n::tr(if es_ward_score {
        "Puntos de visión (wardScore) por equipo en cada ventana de 2 minutos."
    } else if fuente.is_empty() {
        "Puntos de visión por equipo en cada ventana de 2 minutos."
    } else {
        "Wards colocadas por equipo en cada ventana de 2 minutos (el cliente no expuso wardScore)."
    });
    egui::Frame::default()
        .fill(PANEL)
        .inner_margin(egui::Margin::same(12))
        .corner_radius(egui::CornerRadius::same(10))
        .stroke(egui::Stroke::new(1.0, TEAL.linear_multiply(0.55)))
        .show(ui, |ui| {
            ui.label(
                RichText::new(i18n::tr("👁  DASHBOARD DE VISIÓN (cada 2 min)"))
                    .family(strong_family())
                    .size(15.0)
                    .color(TEAL),
            );
            ui.label(RichText::new(metrica).size(12.5).color(MUTED));
            ui.add_space(6.0);

            let n = ((dur.max(W) / W).ceil() as usize).max(1);
            let mut blue = vec![0.0f64; n];
            let mut red = vec![0.0f64; n];
            let mut con_datos = false;
            for b in vision_blocks {
                let idx = (b.start_s / W).floor() as usize;
                if idx >= n {
                    continue;
                }
                blue[idx] += b.ward_blue_delta;
                red[idx] += b.ward_red_delta;
                if b.ward_blue_delta.abs() + b.ward_red_delta.abs() > 0.0 {
                    con_datos = true;
                }
            }
            if !con_datos {
                egui::Frame::default()
                    .fill(INSET)
                    .inner_margin(egui::Margin::same(10))
                    .corner_radius(egui::CornerRadius::same(6))
                    .show(ui, |ui| {
                        ui.label(
                            RichText::new(i18n::tr("TUTORIAL: cómo obtener visión por minuto"))
                                .size(13.5)
                                .strong()
                                .color(AMBER),
                        );
                        ui.label(
                            RichText::new(i18n::tr(
                                "1) Abre el replay en League. 2) Déjalo reproducir completo, puedes poner 8x. 3) La app graba `wardScore` en segundo plano. 4) Al cerrarlo, los datos se fusionan solos con este informe.",
                            ))
                            .size(12.5)
                            .color(TEXT),
                        );
                        ui.add_space(3.0);
                        ui.label(
                            RichText::new(i18n::tr(
                                "El timeline del LCU no expone `wardScore` por minuto y el .rofl sólo da el total. Sin reproducción en vivo no hay serie que mostrar.",
                            ))
                            .size(11.5)
                            .color(MUTED),
                        );
                    });
                return;
            }

            let fmt_v = |v: f64| {
                if es_ward_score {
                    format!("{v:.1}")
                } else {
                    format!("{v:.0}")
                }
            };
            let mut orden: Vec<usize> = (0..n)
                .filter(|&i| blue[i].abs() + red[i].abs() > 0.0)
                .collect();
            let diff_at = |i: usize| blue[i] - red[i];
            orden.sort_by(|&a, &b| {
                diff_at(b)
                    .partial_cmp(&diff_at(a))
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            let mut orden_rojo = orden.clone();
            orden_rojo.reverse();

            ui.columns(2, |cols| {
                // Left: 2-minute table.
                egui::Grid::new("vision_dashboard_grid")
                    .num_columns(4)
                    .spacing([8.0, 4.0])
                    .striped(true)
                    .show(&mut cols[0], |ui| {
                        for h in ["VENTANA", "AZUL", "ROJO", "VENTAJA"] {
                            ui.label(
                                RichText::new(i18n::tr(h))
                                    .size(11.0)
                                    .strong()
                                    .color(GOLD_BRIGHT),
                            );
                        }
                        ui.end_row();
                        for i in 0..n {
                            let (b, r) = (blue[i], red[i]);
                            let start = i as f32 * W;
                            let end = ((i + 1) as f32 * W).min(dur.max(W));
                            ui.label(
                                RichText::new(format!("[{} – {}]", mmss(start), mmss(end)))
                                    .size(11.0)
                                    .color(TEXT),
                            );
                            ui.label(
                                RichText::new(fmt_v(b))
                                    .size(11.5)
                                    .strong()
                                    .color(BLUE),
                            );
                            ui.label(
                                RichText::new(fmt_v(r))
                                    .size(11.5)
                                    .strong()
                                    .color(RED),
                            );
                            let diff = b - r;
                            let (txt, col) = if b.abs() < 0.001 && r.abs() < 0.001 {
                                ("—".to_string(), MUTED)
                            } else if diff.abs() < 0.05 {
                                ("0".to_string(), MUTED)
                            } else if diff > 0.0 {
                                (format!("{} +{diff:.1}", i18n::tr("AZUL")), BLUE)
                            } else {
                                (format!("{} +{:.1}", i18n::tr("ROJO"), diff.abs()), RED)
                            };
                            ui.label(RichText::new(txt).size(11.0).color(col));
                            ui.end_row();
                        }
                    });
                cols[0].label(
                    RichText::new(i18n::tr(
                        "Diferencia = visión AZUL − ROJO. «—» = sin actividad; «0» = empate.",
                    ))
                    .size(10.5)
                    .color(MUTED),
                );

                // Right: best vision windows for each team.
                let col = &mut cols[1];
                col.label(
                    RichText::new(i18n::tr("MEJORES VENTANAS DE VISIÓN"))
                        .family(strong_family())
                        .size(17.5)
                        .color(GOLD_BRIGHT),
                );
                col.label(
                    RichText::new(i18n::tr("De mejor a peor para cada equipo (2 min)."))
                        .size(13.0)
                        .color(MUTED),
                );
                col.add_space(6.0);
                let top = 5usize;
                for (titulo, indices, color) in [
                    ("AZUL", &orden, BLUE),
                    ("ROJO", &orden_rojo, RED),
                ] {
                    col.label(RichText::new(i18n::tr(titulo)).size(15.0).strong().color(color));
                    if indices.is_empty() {
                        col.label(RichText::new(i18n::tr("—")).size(13.5).color(MUTED));
                    }
                    for &i in indices.iter().take(top) {
                        let start = i as f32 * W;
                        let end = ((i + 1) as f32 * W).min(dur.max(W));
                        let d = if titulo == "AZUL" {
                            blue[i] - red[i]
                        } else {
                            red[i] - blue[i]
                        };
                        col.label(
                            RichText::new(i18n::trf(
                                "[{0} – {1}]  +{2}",
                                &[&mmss(start), &mmss(end), &fmt_v(d.abs())],
                            ))
                            .size(14.5)
                            .color(color),
                        );
                    }
                    col.add_space(8.0);
                }
            });
        });
}

/// BLUE vs RED comparison table of what generated the gold in the window.
fn alert_breakdown_table(ui: &mut egui::Ui, block: &DominionBlock, es_oro: bool, show_diff: bool) {
    use std::collections::BTreeMap;
    let extra_label = if es_oro {
        "Oro extra (wards, placas no detectadas, pasivo…)"
    } else {
        "Oro extra (no detectado)"
    };
    let mut map: BTreeMap<&'static str, (f64, f64, bool)> = BTreeMap::new();
    let mut add = |k: &'static str, b: f64, r: f64, av: bool| {
        let e = map.entry(k).or_insert((0.0, 0.0, false));
        e.0 += b;
        e.1 += r;
        e.2 |= av;
    };
    for a in &block.axes {
        // Old captures do not carry `blue`/`red`: it is rebuilt from the net value.
        let (mut b, mut r) = (a.blue, a.red);
        if b.abs() < 1e-9 && r.abs() < 1e-9 && a.net_blue.abs() > 1e-9 {
            b = a.net_blue.max(0.0);
            r = (-a.net_blue).max(0.0);
        }
        match a.axis.as_str() {
            "kills" => add("Kills", b, r, a.available),
            "towers" => add("Torres", b, r, a.available),
            // Plates are not detected reliably: their gold goes to Extra.
            "plates" => add(extra_label, b, r, a.available),
            "dragons" | "heralds" | "barons" | "voidgrubs" | "objectives" => {
                add("Objetivos neutrales", b, r, a.available)
            }
            "minions" => add("Súbditos", b, r, a.available),
            "jungle" => add("Monstruos de jungla", b, r, a.available),
            "extra" => add(extra_label, b, r, a.available),
            "cs" => add("Súbditos / jungla / resto", b, r, a.available),
            _ => {}
        }
    }
    let filas: Vec<&str> = if es_oro {
        vec![
            "Kills",
            "Torres",
            "Objetivos neutrales",
            "Súbditos",
            "Monstruos de jungla",
            extra_label,
        ]
    } else {
        vec![
            "Kills",
            "Torres",
            "Objetivos neutrales",
            "Súbditos / jungla / resto",
            extra_label,
        ]
    };
    for k in &filas {
        map.entry(k).or_insert((0.0, 0.0, false));
    }

    let unidad = if es_oro { "g" } else { "pts" };
    let fmt = |v: f64| -> String {
        if es_oro {
            format!("{v:+.0}g")
        } else {
            format!("{v:+.1} pts")
        }
    };

    egui::Grid::new(format!("breakdown_{}_{}", block.index, show_diff))
        .num_columns(if show_diff { 4 } else { 3 })
        .spacing([8.0, 3.0])
        .striped(true)
        .show(ui, |ui| {
            let mut headers = vec!["CATEGORÍA", "AZUL", "ROJO"];
            if show_diff {
                headers.push("DIF.");
            }
            for h in headers {
                ui.label(
                    RichText::new(i18n::tr(h))
                        .size(11.0)
                        .strong()
                        .color(GOLD_BRIGHT),
                );
            }
            ui.end_row();
            for k in &filas {
                let (b, r, _av) = map.get(k).copied().unwrap_or((0.0, 0.0, false));
                ui.label(RichText::new(i18n::tr(k)).size(11.5).color(TEXT));
                ui.label(RichText::new(fmt(b)).size(11.5).color(BLUE));
                ui.label(RichText::new(fmt(r)).size(11.5).color(RED));
                if show_diff {
                    let d = b - r;
                    let (txt, col) = if d.abs() < 0.01 {
                        ("0".to_string(), MUTED)
                    } else if d > 0.0 {
                        (format!("+{d:.0}{unidad}"), BLUE)
                    } else {
                        (format!("{d:.0}{unidad}"), RED)
                    };
                    ui.label(RichText::new(txt).size(11.5).color(col));
                }
                ui.end_row();
            }
            let tb: f64 = map.values().map(|(b, _, _)| *b).sum();
            let tr: f64 = map.values().map(|(_, r, _)| *r).sum();
            ui.label(
                RichText::new(i18n::tr("TOTAL VENTANA"))
                    .size(11.5)
                    .strong()
                    .color(GOLD_BRIGHT),
            );
            ui.label(
                RichText::new(format!("{tb:.0}{unidad}"))
                    .size(11.5)
                    .strong()
                    .color(BLUE),
            );
            ui.label(
                RichText::new(format!("{tr:.0}{unidad}"))
                    .size(11.5)
                    .strong()
                    .color(RED),
            );
            if show_diff {
                ui.label(
                    RichText::new(format!("Σ {:.0}{unidad}", tb + tr))
                        .size(11.5)
                        .strong()
                        .color(GOLD_BRIGHT),
                );
            }
            ui.end_row();
        });
    if !show_diff {
        ui.label(
            RichText::new(i18n::trf(
                "Oro total de ambos equipos: Σ {0}{1}",
                &[
                    &format!("{:.0}", {
                        let tb: f64 = map.values().map(|(b, _, _)| *b).sum();
                        let tr: f64 = map.values().map(|(_, r, _)| *r).sum();
                        tb + tr
                    }),
                    unidad,
                ],
            ))
            .size(11.0)
            .color(GOLD_BRIGHT),
        );
    }
    ui.label(
        RichText::new(i18n::tr(
            "Σ = suma de ambos equipos en la ventana. «Oro extra» es exactamente el oro real del juego menos lo detectado: incluye wards eliminadas, placas/objetivos no detectados, pasivo, bounties y asistencias. Nada queda fuera.",
        ))
        .size(10.5)
        .color(MUTED),
    );
}

/// Cards of the 5 best gold windows for each team.
fn gold_top5_section(ui: &mut egui::Ui, report: &DominionReport, es_oro: bool) {
    ui.add_space(6.0);
    ui.label(
        RichText::new(i18n::tr("MEJORES VENTANAS POR EQUIPO"))
            .family(strong_family())
            .size(15.0)
            .color(GOLD_BRIGHT),
    );
    ui.label(
        RichText::new(i18n::tr(
            "Sin umbral: las 5 mejores ventanas de cada equipo (las peores de uno son las mejores del otro).",
        ))
        .size(13.5)
        .color(MUTED),
    );
    ui.add_space(6.0);
    ui.columns(2, |cols| {
        top5_column(
            &mut cols[0],
            "EQUIPO AZUL",
            &report.top5_azul,
            BLUE,
            Team::Blue,
            es_oro,
        );
        top5_column(
            &mut cols[1],
            "EQUIPO ROJO",
            &report.top5_rojo,
            RED,
            Team::Red,
            es_oro,
        );
    });
}

/// Informational panel: **all** 30 s blocks, in order, with the gold that
/// each team generated. It does not talk about advantage or severity.
fn gold_all_windows_panel(ui: &mut egui::Ui, report: &DominionReport, es_oro: bool) {
    egui::Frame::default()
        .fill(PANEL)
        .inner_margin(egui::Margin::same(12))
        .corner_radius(egui::CornerRadius::same(10))
        .stroke(egui::Stroke::new(1.0, GOLD.linear_multiply(0.45)))
        .show(ui, |ui| {
            ui.label(
                RichText::new(i18n::tr("📊  AVANCE DE ORO CADA 30 S (INFORMATIVO)"))
                    .family(strong_family())
                    .size(15.0)
                    .color(GOLD_BRIGHT),
            );
            ui.label(
                RichText::new(i18n::tr(
                    "Todos los tiempos de la partida: oro de cada equipo y diferencia neta de la ventana (0 si empatan).",
                ))
                .size(12.5)
                .color(MUTED),
            );
            ui.add_space(6.0);
            for b in &report.blocks {
                let mut blue: f64 = b.axes.iter().map(|a| a.blue).sum();
                let mut red: f64 = b.axes.iter().map(|a| a.red).sum();
                if blue.abs() < 1e-9 && red.abs() < 1e-9 && b.score_net_blue.abs() > 1e-9 {
                    blue = b.score_net_blue.max(0.0);
                    red = (-b.score_net_blue).max(0.0);
                }
                let (vb, vr) = if es_oro {
                    (format!("{blue:+.0}g"), format!("{red:+.0}g"))
                } else {
                    (format!("{blue:+.1}"), format!("{red:+.1}"))
                };
                let d = blue - red;
                let blue_label = i18n::tr("AZUL");
                let red_label = i18n::tr("ROJO");
                let ventaja = if d.abs() < 0.05 {
                    "0".to_string()
                } else if es_oro {
                    if d > 0.0 {
                        format!("{blue_label} +{d:.0}g")
                    } else {
                        format!("{red_label} +{:.0}g", d.abs())
                    }
                } else if d > 0.0 {
                    format!("{blue_label} +{d:.1}")
                } else {
                    format!("{red_label} +{:.1}", d.abs())
                };
                egui::CollapsingHeader::new(
                    RichText::new(i18n::trf(
                        "[{0} – {1}]   AZUL {2}   ·   ROJO {3}   ·   {4}",
                        &[&mmss(b.start_s), &mmss(b.end_s), &vb, &vr, &ventaja],
                    ))
                    .size(13.0)
                    .color(TEXT),
                )
                .id_salt(format!("gold_window_{}", b.index))
                .show(ui, |ui| {
                    alert_breakdown_table(ui, b, es_oro, false);
                });
            }
        });
}

/// Left panel of the report: gold alerts with a comparative breakdown.
fn gold_alerts_panel(ui: &mut egui::Ui, report: &DominionReport, es_oro: bool) {
    let breaks = sorted_breakpoints(report);
    egui::Frame::default()
        .fill(PANEL)
        .inner_margin(egui::Margin::same(12))
        .corner_radius(egui::CornerRadius::same(10))
        .stroke(egui::Stroke::new(1.0, ACCENT_SOFT.linear_multiply(0.55)))
        .show(ui, |left| {
            let kind = i18n::tr(if es_oro { "ORO" } else { "SCORE" });
            left.label(
                RichText::new(i18n::trf(
                    "⚠  ALERTAS DE {0}  ({1})",
                    &[&kind, &breaks.len().to_string()],
                ))
                .family(strong_family())
                .size(15.0)
                .color(ACCENT_SOFT),
            );
            left.label(
                RichText::new(i18n::trf(
                    "En orden cronológico · {0} ventanas sobre el piso (|{1}| ≥ {2}{3})",
                    &[
                        &breaks.len().to_string(),
                        &i18n::tr(if es_oro { "oro" } else { "score" }),
                        &format!("{:.0}", report.severity_thresholds.semi_alerta),
                        if es_oro { "g" } else { "" },
                    ],
                ))
                .size(13.5)
                .color(MUTED),
            );
            left.add_space(6.0);
            if breaks.is_empty() {
                left.label(
                    RichText::new(i18n::tr("Ninguna ventana superó el piso de alerta."))
                        .size(14.5)
                        .color(MUTED),
                );
            }
            for q in &breaks {
                let fav_col = team_color(q.favorecido);
                let valor = if es_oro {
                    format!("{:+.0}g", q.score_net_blue)
                } else {
                    format!("{:+.1} pts", q.score_net_blue)
                };
                egui::CollapsingHeader::new(
                    RichText::new(i18n::trf(
                        "[{0} – {1}]   {2}   ·   Ventaja a favor del {3}   ·   {4}",
                        &[
                            &mmss(q.start_s),
                            &mmss(q.end_s),
                            &valor,
                            &team_name(q.favorecido),
                            &severity_label(q.severity),
                        ],
                    ))
                    .size(15.0)
                    .color(fav_col),
                )
                .id_salt(format!("break_{}", q.block_index))
                .show(left, |ui| {
                    ui.indent("desglose_alerta", |ui| {
                        if let Some(block) = report.blocks.get(q.block_index as usize) {
                            alert_breakdown_table(ui, block, es_oro, true);
                        } else {
                            ui.label(
                                RichText::new(i18n::tr("Detalle no disponible para este bloque."))
                                    .size(12.0)
                                    .color(MUTED),
                            );
                        }
                    });
                });
            }
        });
}

fn ui_report(
    ui: &mut egui::Ui,
    r: &mut ReportView,
    auto_json: &mut bool,
    capture_dir: &Path,
    revision_name: &str,
) {
    // LCU-enriched source: the values of `score_net_blue` are **gold**.
    let es_oro = r.report.fuente.contains("lcu");
    ui.add_space(8.0);
    if let Some(revision) = &r.revision {
        if !revision.is_empty() {
            ui.label(
                RichText::new(revision)
                    .family(strong_family())
                    .size(21.0)
                    .strong()
                    .color(GOLD_BRIGHT),
            );
            ui.add_space(2.0);
        }
    }
    // Serif headline with a gold halo ("infographic" style from the reference).
    egui::Frame::default()
        .inner_margin(egui::Margin::same(0))
        .shadow(egui::Shadow {
            offset: [0, 2],
            blur: 16,
            spread: 3,
            color: GOLD.linear_multiply(0.30),
        })
        .show(ui, |ui| {
            ui.label(
                RichText::new(i18n::tr(&r.title))
                    .family(display_family())
                    .size(34.0)
                    .strong()
                    .color(GOLD_BRIGHT),
            );
        });
    ui.label(RichText::new(r.subtitle.clone()).size(14.5).color(MUTED));
    for n in &r.notes {
        ui.label(RichText::new(format!("• {n}")).size(13.5).color(ACCENT_SOFT));
    }

    // Save the capture log.
    if let Some(json) = r.json.clone() {
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            // With automatic saving enabled, the manual button would be redundant.
            if *auto_json {
                ui.label(
                    RichText::new(i18n::tr("💾  Registro JSON guardado automáticamente"))
                        .size(14.5)
                        .color(GREEN),
                );
            } else {
                let b = egui::Button::new(
                    RichText::new(i18n::tr("💾  Guardar registro .json…"))
                        .size(16.0)
                        .strong()
                        .color(Color32::WHITE),
                )
                .fill(ACCENT.linear_multiply(0.9))
                .corner_radius(egui::CornerRadius::same(8))
                .min_size(Vec2::new(250.0, 38.0));
                if ui.add(b).clicked() {
                    let default_name = r
                        .revision
                        .as_deref()
                        .filter(|s| !s.is_empty())
                        .unwrap_or(revision_name);
                    if let Some(msg) = save_json_dialog(&json, capture_dir, default_name) {
                        r.save_msg = Some(msg);
                    }
                }
            }
            ui.checkbox(
                auto_json,
                RichText::new(i18n::tr("Guardar JSON automáticamente"))
                    .size(13.5)
                    .color(MUTED),
            );
            if let Some((ok, msg)) = &r.save_msg {
                ui.label(
                    RichText::new(msg)
                        .size(14.0)
                        .color(if *ok { GREEN } else { RED }),
                );
            }
        });
    }

    ui.add_space(12.0);

    // 1) Criticality legend: what each alert level means.
    let vt = r.report.vision.thresholds;
    severity_legend(ui, &r.report.params, &vt, es_oro);
    ui.add_space(12.0);

    // 2) Time bar: the user chooses the instant to analyze.
    let dur = total_duration(&r.report, &r.vision_blocks);
    time_slider(ui, &mut r.cursor_s, dur);
    ui.add_space(6.0);

    // 3) Accumulated advantage **at that instant**: at the top, to see how the
    //    game is going while the bar is moved.
    ui.label(
        RichText::new(i18n::tr("VENTAJA ACUMULADA HASTA EL INSTANTE ELEGIDO"))
            .family(strong_family())
            .size(13.5)
            .color(GOLD_BRIGHT),
    );
    ui.add_space(4.0);
    advantage_cards(
        ui,
        &r.report,
        &r.vision_blocks,
        r.cursor_s,
        None,
        None,
    );
    ui.add_space(12.0);

    // 4) Real gold chart.
    ui.label(
        RichText::new(if es_oro {
            i18n::tr("VENTAJA ACUMULADA DE ORO REAL")
        } else {
            i18n::tr("VENTAJA ACUMULADA DE SCORE")
        })
        .family(strong_family())
        .size(13.5)
        .color(GOLD_BRIGHT),
    );
    score_chart(ui, &r.report, dur, 170.0, r.cursor_s);
    ui.add_space(10.0);

    ui.label(
        RichText::new(if es_oro {
            i18n::tr("Línea de tiempo de 30 s · barra = oro de la ventana · franja inferior = visión")
        } else {
            i18n::tr("Línea de tiempo de 30 s · barra = score de la ventana · franja inferior = visión")
        })
        .size(13.5)
        .color(MUTED),
    );
    timeline_strip(ui, &r.report, 100.0);
    ui.label(
        RichText::new(i18n::tr(
            "Alertas: mitad izquierda = gravedad · mitad derecha = equipo favorecido (azul/rojo).",
        ))
        .size(12.0)
        .color(MUTED),
    );
    ui.add_space(14.0);

    // 5) Best gold windows, right above the alerts.
    gold_top5_section(ui, &r.report, es_oro);
    ui.add_space(10.0);

    // 6) Gold alerts (left) + 2 min vision dashboard (right).
    ui.columns(2, |cols| {
        gold_alerts_panel(&mut cols[0], &r.report, es_oro);
        vision_dashboard(&mut cols[1], &r.vision_blocks, dur);
    });
    ui.add_space(14.0);
    gold_all_windows_panel(ui, &r.report, es_oro);
    // Analyst notes block: free text, saved separately.
    ui.add_space(16.0);
    ui.separator();
    ui.add_space(8.0);
    ui.label(
        RichText::new(i18n::tr("ANOTACIONES"))
            .family(strong_family())
            .size(15.0)
            .color(GOLD_BRIGHT),
    );
    ui.label(
        RichText::new(i18n::tr(
            "Tu libreta de la scrim: qué mirar en el VOD, timestamps, jugadas raras… Se guarda en su propio archivo (independiente del registro .json).",
        ))
        .size(13.0)
        .color(MUTED),
    );
    ui.add_space(4.0);
    let mut texto = r.anotaciones.texto.clone();
    egui::Frame::default()
        .fill(PANEL)
        .inner_margin(egui::Margin::same(10))
        .corner_radius(egui::CornerRadius::same(8))
        .stroke(egui::Stroke::new(1.0, PANEL3))
        .show(ui, |ui| {
            let respuesta = ui.add(
                egui::TextEdit::multiline(&mut texto)
                    .desired_width(f32::INFINITY)
                    .desired_rows(6)
                    .font(egui::TextStyle::Body)
                    .hint_text(i18n::tr("Escribe aquí tus notas de la partida…")),
            );
            if respuesta.changed() {
                r.anotaciones.texto = texto.clone();
            }
        });
    ui.horizontal(|ui| {
        if ui
            .add(
                egui::Button::new(
                    RichText::new(i18n::tr("💾  Guardar notas…"))
                        .size(14.0)
                        .strong()
                        .color(TEXT),
                )
                .fill(PANEL3)
                .corner_radius(egui::CornerRadius::same(8))
                .min_size(Vec2::new(190.0, 34.0)),
            )
            .clicked()
        {
            guardar_anotaciones(&mut r.anotaciones);
        }
        if ui
            .add(
                egui::Button::new(
                    RichText::new(i18n::tr("📂  Cargar notas…"))
                        .size(14.0)
                        .color(TEXT),
                )
                .fill(PANEL3)
                .corner_radius(egui::CornerRadius::same(8))
                .min_size(Vec2::new(180.0, 34.0)),
            )
            .clicked()
        {
            cargar_anotaciones(&mut r.anotaciones);
        }
        if ui
            .add(
                egui::Button::new(RichText::new(i18n::tr("🗑  Limpiar")).size(14.0).color(MUTED))
                    .fill(PANEL2)
                    .corner_radius(egui::CornerRadius::same(8))
                    .min_size(Vec2::new(130.0, 34.0)),
            )
            .clicked()
        {
            r.anotaciones.texto.clear();
            r.anotaciones.mensaje = Some(i18n::tr("Notas vaciadas"));
        }
        if let Some(ruta) = &r.anotaciones.ruta {
            ui.label(
                RichText::new(i18n::trf("Archivo: {0}", &[ruta]))
                    .size(12.0)
                    .color(MUTED),
            );
        }
    });
    if let Some(msg) = &r.anotaciones.mensaje {
        ui.label(
            RichText::new(format!("· {msg}"))
                .size(12.5)
                .color(GOLD_BRIGHT),
        );
    }



    // Calibration (quick reference).
    let p = &r.report.params;
    ui.add_space(14.0);
    ui.separator();
    ui.add_space(8.0);
    let calibracion = if es_oro {
        i18n::trf(
            "Alertas por oro neto en 30 s: leve ≥{0}g · moderada ≥{1}g · fuerte ≥{2}g · crítica ≥{3}g   |   visión: ventanas de {4} s, cortes {5} / {6} / {7} / {8}, actividad mínima ≥{9}",
            &[
                &format!("{:.0}", p.thresholds.semi_alerta),
                &format!("{:.0}", p.thresholds.ventaja_media),
                &format!("{:.0}", p.thresholds.ventaja_considerable),
                &format!("{:.0}", p.thresholds.ventaja_critica),
                &(vt.window_secs as i64).to_string(),
                &format!("{:.2}", vt.semi_alerta),
                &format!("{:.2}", vt.ventaja_media),
                &format!("{:.2}", vt.ventaja_considerable),
                &format!("{:.2}", vt.ventaja_critica),
                &format!("{:.0}", vt.actividad_minima),
            ],
        )
    } else {
        i18n::trf(
            "Puntos por acción: kill {0} · torre {1} · placa {2} · dragón {3} · heraldo {4} · barón {5} · CS {6}   |   cortes {7} / {8} / {9} / {10} pts   |   visión: ventanas de {11} s, cortes {12} / {13} / {14} / {15}, actividad mínima ≥{16}",
            &[
                &format!("{:.1}", p.kills),
                &format!("{:.1}", p.tower),
                &format!("{:.1}", p.plate),
                &format!("{:.1}", p.dragon),
                &format!("{:.1}", p.herald),
                &format!("{:.1}", p.baron),
                &format!("{:.2}", p.cs),
                &format!("{:.0}", p.thresholds.semi_alerta),
                &format!("{:.0}", p.thresholds.ventaja_media),
                &format!("{:.0}", p.thresholds.ventaja_considerable),
                &format!("{:.0}", p.thresholds.ventaja_critica),
                &(vt.window_secs as i64).to_string(),
                &format!("{:.2}", vt.semi_alerta),
                &format!("{:.2}", vt.ventaja_media),
                &format!("{:.2}", vt.ventaja_considerable),
                &format!("{:.2}", vt.ventaja_critica),
                &format!("{:.0}", vt.actividad_minima),
            ],
        )
    };
    ui.label(RichText::new(calibracion).size(12.5).color(MUTED));
}


// ---------------------------------------------------------------------------
// Replay screen
// ---------------------------------------------------------------------------

fn team_totals_table(ui: &mut egui::Ui, tt: &TeamTotals) {
    egui::Grid::new("team_totals")
        .num_columns(3)
        .spacing([20.0, 4.0])
        .show(ui, |ui| {
            ui.label(RichText::new(i18n::tr("")).size(14.0));
            ui.label(RichText::new(i18n::tr("AZUL")).size(14.0).strong().color(BLUE));
            ui.label(RichText::new(i18n::tr("ROJO")).size(14.0).strong().color(RED));
            ui.end_row();
            let rows: [(&str, u64, u64); 8] = [
                ("Oro ganado", tt.blue.gold_earned, tt.red.gold_earned),
                ("Oro gastado", tt.blue.gold_spent, tt.red.gold_spent),
                ("CS", tt.blue.cs, tt.red.cs),
                ("Kills", tt.blue.kills as u64, tt.red.kills as u64),
                ("Muertes", tt.blue.deaths as u64, tt.red.deaths as u64),
                ("Asistencias", tt.blue.assists as u64, tt.red.assists as u64),
                ("Wards puestas", tt.blue.wards_placed as u64, tt.red.wards_placed as u64),
                ("Daño a campeones", tt.blue.damage_to_champions, tt.red.damage_to_champions),
            ];
            for (label, b, r) in rows {
                ui.label(RichText::new(i18n::tr(label)).size(14.0).color(MUTED));
                ui.label(
                    RichText::new(format!("{b}"))
                        .size(14.0)
                        .color(if b >= r { BLUE } else { TEXT }),
                );
                ui.label(
                    RichText::new(format!("{r}"))
                        .size(14.0)
                        .color(if r > b { RED } else { TEXT }),
                );
                ui.end_row();
            }
        });
}

fn replay_players_table(ui: &mut egui::Ui, players: &[Player]) {
    for (team, name, col) in [
        (Team::Blue, i18n::tr("AZUL"), BLUE),
        (Team::Red, i18n::tr("ROJO"), RED),
    ] {
        ui.add_space(4.0);
        ui.label(RichText::new(name.clone()).size(15.0).strong().color(col));
        egui::Grid::new(format!("rofl_players_{name}"))
            .num_columns(8)
            .spacing([16.0, 3.0])
            .show(ui, |ui| {
                for h in ["Jugador", "Campeón", "Nv", "K/D/A", "CS", "Wards P/K", "Oro", "Daño"] {
                    ui.label(RichText::new(i18n::tr(h)).size(12.5).color(MUTED));
                }
                ui.end_row();
                for p in players.iter().filter(|p| p.team == team) {
                    let s = &p.stats;
                    ui.label(RichText::new(&p.summoner_name).size(14.0).color(TEXT));
                    ui.label(
                        RichText::new(
                            p.champion_name
                                .clone()
                                .unwrap_or_else(|| format!("id {}", p.champion_id)),
                        )
                        .size(14.0)
                        .color(TEXT),
                    );
                    ui.label(RichText::new(format!("{}", s.champ_level)).size(14.0).color(MUTED));
                    ui.label(
                        RichText::new(format!("{}/{}/{}", s.kills, s.deaths, s.assists))
                            .size(14.0)
                            .color(TEXT),
                    );
                    ui.label(RichText::new(format!("{}", s.cs)).size(14.0).color(TEXT));
                    ui.label(
                        RichText::new(format!("{}/{}", s.wards_placed, s.wards_killed))
                            .size(14.0)
                            .color(TEAL),
                    );
                    ui.label(RichText::new(format!("{}", s.gold_earned)).size(14.0).color(ACCENT_SOFT));
                    ui.label(
                        RichText::new(format!("{}", s.damage_to_champions)).size(14.0).color(TEXT),
                    );
                    ui.end_row();
                }
            });
    }
}

fn kill_density_strip(ui: &mut egui::Ui, density: &[(u32, u32, u32)]) {
    let n = density.len().max(1);
    let max = density.iter().map(|d| d.2).max().unwrap_or(0).max(1);
    let width = ui.available_width().max(240.0);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, 74.0), Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, egui::CornerRadius::same(6), INSET);

    let pad = 6.0;
    let col_w = (rect.width() - 2.0 * pad) / n as f32;
    let area_h = rect.height() - 2.0 * pad;
    for (i, d) in density.iter().enumerate() {
        if d.2 == 0 {
            continue;
        }
        let h = (area_h * d.2 as f32 / max as f32).max(3.0);
        let c = if d.2 >= 4 {
            Color32::from_rgb(240, 120, 70)
        } else if d.2 >= 2 {
            ACCENT
        } else {
            ACCENT.linear_multiply(0.5)
        };
        let x = rect.left() + pad + i as f32 * col_w;
        painter.rect_filled(
            Rect::from_min_size(
                Pos2::new(x + 1.0, rect.bottom() - pad - h),
                Vec2::new((col_w - 2.0).max(1.0), h),
            ),
            egui::CornerRadius::same(2),
            c,
        );
    }

    if let Some(pos) = resp.hover_pos() {
        if rect.contains(pos) {
            let idx = (((pos.x - rect.left() - pad) / col_w) as usize).min(n - 1);
            if let Some(d) = density.get(idx) {
                resp.on_hover_text(i18n::trf(
                    "[{0} – {1}]\n{2} bajas",
                    &[&mmss(d.0 as f32), &mmss(d.1 as f32), &d.2.to_string()],
                ));
            }
        }
    }
}

fn ui_replay(ui: &mut egui::Ui, r: &mut ReplayView) {
    let m = &r.replay.metadata;
    ui.add_space(8.0);
    ui.label(RichText::new(i18n::tr("ANÁLISIS DE REPLAY .ROFL")).size(22.0).strong().color(ACCENT));
    ui.label(RichText::new(basename(&r.path)).size(15.0).color(TEXT));
    ui.label(
        RichText::new(i18n::trf(
            "Partida {0} · versión {1} · modo {2} · mapa {3} · cola {4} · {5}s",
            &[
                &m.game_id.to_string(),
                &m.game_version,
                &m.game_mode,
                &m.map_id.to_string(),
                &m.queue_id.to_string(),
                &format!("{:.0}", m.game_length_s),
            ],
        ))
        .size(13.5)
        .color(MUTED),
    );
    if let Some(w) = &r.warning {
        ui.add_space(8.0);
        egui::Frame::default()
            .fill(Color32::from_rgb(52, 22, 30))
            .inner_margin(egui::Margin::same(10))
            .corner_radius(egui::CornerRadius::same(6))
            .stroke(egui::Stroke::new(1.0, RED.linear_multiply(0.5)))
            .show(ui, |ui| {
                ui.label(RichText::new(format!("⚠ {w}")).size(13.5).color(TEXT));
            });
    }
    ui.add_space(10.0);

    // Honest notice about what the format does not provide.
    egui::Frame::default()
        .fill(Color32::from_rgb(52, 40, 18))
        .inner_margin(egui::Margin::same(10))
        .corner_radius(egui::CornerRadius::same(6))
        .show(ui, |ui| {
            ui.label(RichText::new(i18n::tr("Aviso del formato replay")).size(15.0).strong().color(ACCENT_SOFT));
            ui.label(
                RichText::new(i18n::tr(
                    "Esta vista es el análisis forense básico del .rofl (sin timeline del LCU). El .rofl moderno no expone la atribución de equipo en los eventos; por eso aquí NO hay oro ni visión por ventana. Para el informe completo, abre League of Legends y vuelve a abrir el replay: la app pedirá el timeline oficial por el gameId del nombre del archivo.",
                ))
                .size(14.0)
                .color(TEXT),
            );
        });
    ui.add_space(8.0);

    for w in &r.replay.warnings {
        ui.label(RichText::new(format!("⚠ {w}")).size(13.5).color(ACCENT_SOFT));
    }
    if !r.replay.warnings.is_empty() {
        ui.add_space(6.0);
    }

    ui.label(RichText::new(i18n::tr("Totales por equipo (statsJson)")).size(16.0).strong().color(ACCENT_SOFT));
    ui.add_space(4.0);
    team_totals_table(ui, &r.replay.team_totals);
    ui.add_space(12.0);

    ui.label(RichText::new(i18n::tr("Jugadores")).size(16.0).strong().color(ACCENT_SOFT));
    replay_players_table(ui, &r.replay.players);
    ui.add_space(12.0);

    ui.label(
        RichText::new(i18n::tr("Densidad de bajas por ventana de 30 s (las mejores para revisar el VOD)"))
            .size(16.0)
            .strong()
            .color(ACCENT_SOFT),
    );
    ui.label(
        RichText::new(i18n::tr("Ventanas con más bajas. Sin atribución de equipo: el formato replay no la trae."))
            .size(13.5)
            .color(MUTED),
    );
    ui.add_space(6.0);
    kill_density_strip(ui, &r.density);
    ui.add_space(8.0);

    let mut top: Vec<&(u32, u32, u32)> = r.density.iter().filter(|d| d.2 > 0).collect();
    top.sort_by(|a, b| b.2.cmp(&a.2).then(a.0.cmp(&b.0)));
    top.truncate(12);
    if top.is_empty() {
        ui.label(
            RichText::new(i18n::tr("El replay no trae eventos de kills (formato no soportado)."))
                .size(14.0)
                .color(MUTED),
        );
    } else {
        egui::Grid::new("density_top")
            .num_columns(3)
            .spacing([18.0, 3.0])
            .show(ui, |ui| {
                for d in top {
                    ui.label(
                        RichText::new(i18n::trf(
                            "[{0} – {1}]",
                            &[&mmss(d.0 as f32), &mmss(d.1 as f32)],
                        ))
                        .size(14.0)
                        .color(TEXT),
                    );
                    ui.label(
                        RichText::new(i18n::trf("{0} bajas", &[&d.2.to_string()]))
                            .size(14.0)
                            .color(ACCENT_SOFT),
                    );
                    ui.label(
                        RichText::new(if d.2 >= 4 {
                            i18n::tr("foco de acción")
                        } else {
                            String::new()
                        })
                        .size(13.0)
                        .color(MUTED),
                    );
                    ui.end_row();
                }
            });
    }

    if let Some(er) = &r.replay.events_report {
        ui.add_space(10.0);
        ui.label(
            RichText::new(i18n::trf(
                "Eventos verificados contra statsJson — kills: {0} (stats {1}) · asistencias: {2} ({3}) · placas: {4} ({5}) · torres: {6} ({7})   ·   fuente: {8}",
                &[
                    &er.kills.to_string(),
                    &er.truth_kills.to_string(),
                    &er.assists.to_string(),
                    &er.truth_assists.to_string(),
                    &er.turret_plates.to_string(),
                    &er.truth_turret_plates.to_string(),
                    &er.turrets_destroyed.to_string(),
                    &er.truth_turrets_destroyed.to_string(),
                    &er.fuente,
                ],
            ))
            .size(13.0)
            .color(MUTED),
        );
    }
}

// ---------------------------------------------------------------------------
// Startup
// ---------------------------------------------------------------------------

/// App visuals: night blue, electric blue on the borders and white text.
///
/// They are applied separately from the theme because `egui` resets the visuals when it
/// changes theme (and the system may report a hot change).
fn apply_visuals(ctx: &egui::Context) {
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

fn setup_theme(ctx: &egui::Context) {
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
fn icono_app() -> egui::IconData {
    egui::IconData {
        rgba: include_bytes!("../assets/icon.rgba").to_vec(),
        width: 64,
        height: 64,
    }
}

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1320.0, 860.0])
            .with_min_inner_size([1040.0, 640.0])
            .with_icon(icono_app()),
        ..Default::default()
    };
    match eframe::run_native(
        "Scrim Analyzer — LoL",
        options,
        Box::new(|cc| {
            setup_theme(&cc.egui_ctx);
            Ok(Box::new(App::new(cc)) as Box<dyn eframe::App>)
        }),
    ) {
        Ok(()) => Ok(()),
        Err(e) => {
            rfd::MessageDialog::new()
                .set_level(rfd::MessageLevel::Error)
                .set_title("Scrim Analyzer")
                .set_description(i18n::trf("No se pudo iniciar la interfaz:\n{0}", &[&e.to_string()]))
                .show();
            Ok(())
        }
    }
}
