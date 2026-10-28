//! Shared interface types: messages, views and analyst annotations.
//!
//! Code moved verbatim from `gui_main.rs`; behavior is unchanged.

use super::*;

/// Messages sent by the background threads to the interface.
pub(crate) enum Msg {
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
pub(crate) struct LoadedCapture {
    pub(crate) report: DominionReport,
    pub(crate) blocks: Vec<ScrimBlock>,
    /// Original file contents (so it can be re-saved or exported).
    pub(crate) raw_json: String,
    pub(crate) game_mode: String,
    pub(crate) map_name: String,
    pub(crate) duration_s: f32,
    pub(crate) notes: Vec<String>,
    pub(crate) game_id: Option<u64>,
}

pub(crate) enum View {
    Home,
    LiveDash,
    Report(Box<ReportView>),
    Replay(Box<ReplayView>),
    Loading { label: String },
}

pub(crate) struct ReportView {
    pub(crate) title: String,
    pub(crate) subtitle: String,
    /// Revision name of a live capture (shown above the headline).
    pub(crate) revision: Option<String>,
    pub(crate) notes: Vec<String>,
    pub(crate) report: DominionReport,
    /// Original measured blocks (for the cumulative vision quota).
    pub(crate) vision_blocks: Vec<ScrimBlock>,
    /// `gameId` of the replay/game, used to merge the live capture.
    pub(crate) game_id: Option<u64>,
    /// Analyst annotations (saved separately, in their own file).
    pub(crate) anotaciones: Anotaciones,
    /// Instant selected with the time bar (game seconds).
    pub(crate) cursor_s: f32,
    /// Serialized log for "Save .json log…".
    pub(crate) json: Option<String>,
    /// Result of the last manual save `(ok, message)`.
    pub(crate) save_msg: Option<(bool, String)>,
}

pub(crate) struct ReplayView {
    pub(crate) path: String,
    pub(crate) replay: Box<Replay>,
    /// `(start_s, end_s, kills)` per 30 s window.
    pub(crate) density: Vec<(u32, u32, u32)>,
    /// Warning when enrichment with the LCU failed (e.g. client closed).
    pub(crate) warning: Option<String>,
}

/// Analyst annotations: free text saved in its own file.
#[derive(Debug, Clone, Default)]
pub(crate) struct Anotaciones {
    pub(crate) texto: String,
    /// Path of the notes file (or `None` if it has never been saved).
    pub(crate) ruta: Option<String>,
    /// Last message (error or confirmation).
    pub(crate) mensaje: Option<String>,
}

/// Modal shown before starting the live capture: the analyst names the
/// revision. That name is shown at the top and used for the `.json` file.
pub(crate) struct RevisionDialog {
    pub(crate) input: String,
    /// Focus the text field on the first frame.
    pub(crate) focus: bool,
    /// The user tried to continue with an empty name.
    pub(crate) empty_name: bool,
}

/// Measured chips of the live dashboard.
pub(crate) struct LiveChips {
    pub(crate) cs: (u32, u32),
    pub(crate) kills: (u32, u32),
    pub(crate) towers: (u32, u32),
    pub(crate) objectives: (u32, u32),
    /// `(blue, red)` already formatted; "—" if the client did not expose vision.
    pub(crate) vision: (String, String),
}
