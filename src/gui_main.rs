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
// Modules
// ---------------------------------------------------------------------------
// The crate root is `gui_main.rs`, so a plain `mod` would look for
// `src/<name>.rs`. `#[path]` points each module at its file in `gui/`.

#[path = "gui/capture.rs"]
mod capture;
use capture::*;

#[path = "gui/charts.rs"]
mod charts;
use charts::*;

#[path = "gui/data.rs"]
mod data;
use data::*;

#[path = "gui/home.rs"]
mod home;

#[path = "gui/live.rs"]
mod live;
use live::*;

#[path = "gui/replay.rs"]
mod replay;
use replay::*;

#[path = "gui/report.rs"]
mod report;
use report::*;

#[path = "gui/state.rs"]
mod state;
use state::*;

#[path = "gui/theme.rs"]
mod theme;
use theme::*;

#[path = "gui/vision.rs"]
mod vision;
use vision::*;

// ---------------------------------------------------------------------------
// Application state
// ---------------------------------------------------------------------------

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
            Msg::LiveWaiting => self.on_live_waiting(),
            Msg::LiveGameId(id) => self.on_live_game_id(id),
            Msg::LiveTick(cap) => self.on_live_tick(cap),
            Msg::LivePreview(cap) => self.on_live_preview(cap),
            Msg::LiveDone(res) => self.on_live_done(res),
            Msg::ReplayDone { path, result, warning } => self.on_replay_done(path, result, warning),
            Msg::LiveVisionReady { game_id, blocks } => self.on_live_vision_ready(game_id, blocks),
            Msg::RoflReport { report } => self.on_rofl_report(report),
            Msg::CaptureDone { path, result } => self.on_capture_done(path, result),
        }
    }

    fn on_live_waiting(&mut self) {
        self.live_hint = i18n::tr("Esperando a que arranque una partida…");
    }

    fn on_live_game_id(&mut self, id: u64) {
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

    fn on_live_tick(&mut self, cap: Box<ScrimCapture>) {
        self.live_hint.clear();
        // If the user has not touched the time bar, we follow the
        // live edge.
        if !self.live_cursor_manual {
            self.live_cursor_s = cap.duration_s;
        }
        self.last_live = Some(cap);
    }

    fn on_live_preview(&mut self, cap: Box<ScrimCapture>) {
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

    fn on_live_done(&mut self, res: Result<Box<ScrimCapture>, String>) {
        self.live_running = false;
        match res {
            Ok(cap) => self.on_live_capture_finished(cap),
            Err(e) => {
                self.status = Some((true, i18n::trf("Error en vivo: {0}", &[&e.to_string()])));
                self.view = View::Home;
            }
        }
    }

    /// Decides what to do with a capture that finished: notice, save + Home or
    /// the final report.
    fn on_live_capture_finished(&mut self, cap: Box<ScrimCapture>) {
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
            let notes = self.live_capture_notes(&cap);
            self.last_live = None;
            let mut report = report_from_capture(cap, notes);
            report.revision = Some(self.revision_name.clone());
            self.view = View::Report(Box::new(report));
        }
    }

    /// Notes shown above the report of a live capture: game id, gold source,
    /// vision totals and where the JSON was saved.
    fn live_capture_notes(&mut self, cap: &ScrimCapture) -> Vec<String> {
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
            match save_capture(cap, &self.capture_dir, &self.revision_name) {
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
        notes
    }

    fn on_replay_done(&mut self, path: String, result: Result<Box<Replay>, String>, warning: Option<String>) {
        match result {
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
        }
    }

    fn on_live_vision_ready(&mut self, game_id: u64, blocks: Vec<ScrimBlock>) {
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

    fn on_rofl_report(&mut self, report: Box<ReportView>) {
        self.status = None;
        let mut report = report;
        if let Some((id, blocks)) = self.pending_vision.take() {
            if report.game_id == Some(id) {
                merge_vision_into_report(&mut *report, blocks);
            }
        }
        self.view = View::Report(report);
    }

    fn on_capture_done(&mut self, path: String, result: Result<Box<LoadedCapture>, String>) {
        match result {
            Ok(lc) => {
                self.view = View::Report(Box::new(report_from_loaded(&path, *lc)));
            }
            Err(e) => {
                self.status = Some((true, i18n::trf("No se pudo cargar la captura: {0}", &[&e.to_string()])));
                self.view = View::Home;
            }
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
// Startup
// ---------------------------------------------------------------------------

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
