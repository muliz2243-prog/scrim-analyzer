//! Dominion score over 30 s windows + symmetric vision-share alert.
//!
//! Two independent products built on the same measured 30 s blocks
//! ([`crate::live::ScrimBlock`]):
//!
//! 1. **Dominion score**: the advantage of each measured window in **score
//!    points**. Each action contributes its weight (1 kill = 3.9 · 1 tower =
//!    3.25 · 1 plate = 3.25 · dragon = 2.6 · herald = 5.85 · baron = 3.9 ·
//!    1 CS = 0.26). The weights and cutoffs live in [`ScoreParams`],
//!    **exported and tunable** — no hidden magic.
//!
//!    The weights are **score equivalences**, not real gold: the game does not
//!    expose gold earned through the live API, so it is not presented as such.
//!
//!    Two design decisions:
//!
//!    * **The score is permanent**: the cutoffs do not depend on the minute
//!      (the same advantage is worth the same at minute 4 as at minute 25), so
//!      two games can be compared directly.
//!    * **Epic objective buffs DO NOT add to the score**: they are temporary
//!      and would pollute the curve. They are reported separately
//!      ([`ObjectiveEvent`]: "BLUE took the baron at 03:20") so that the
//!      analyst can match them with the later score rises.
//!
//! 2. **Vision-share alert**: `wardScore` is a cumulative *flow*; in each
//!    block we measure its delta per team and compute the **blue share**
//!    `Δazul / (Δazul + Δrojo)`. Parity is 50 %. It is **symmetric** by
//!    construction: same threshold for and against, the sign tells the side.
//!
//! ## Data transparency (nothing is filled in)
//!
//! The axes are **availability-aware**: if the feed emitted no tower/objective
//! event during the whole scrim, or the capture never measured CS activity,
//! that axis is marked `available: false` and its contribution is a
//! **documented** 0, not an invented zero that dilutes the score. It is listed
//! in `componentes_disponibles` of each block.
//!
//! Kills are always available (the `events.Events` and `scores.*` feed emits
//! them on every poll); CS depends on the snapshot having measured them, and
//! structures/objectives depend on the feed emitting those events.
//!
//! ## Window severity (where the game broke)
//!
//! Each 30 s window is classified by `|score|` using exported cutoffs
//! ([`SeverityThresholds`], tunable):
//!
//! | Interval | Level | Real anchor |
//! |---|---|---|
//! | [4, 6) | `slight advantage` | 1 kill + a bit of farm (≈4.0) |
//! | [6, 8) | `moderate advantage` | 2 kills (double kill, ≈7.8) |
//! | [8, 10) | `strong advantage` | 2 kills + extras, or 40 CS |
//! | ≥ 10 | `critical advantage` | 3 kills, or 2 kills + tower, or 60 CS |
//!
//! A lone kill (3.9) stays **below the floor**: it is noise, not a moment.
//! CS enters with its real value, so a big minion wave **can** be critical.
//!
//! Windows that exceed the floor are listed in `breakpoints`, **each block
//! separately and without merging**: two consecutive windows with an alert are
//! two independent decisions (two mistakes in a row ≠ a snowball), and both
//! notices must be kept.

use serde::{Deserialize, Serialize};

use crate::live::ScrimBlock;
use crate::model::Team;

/// Score calibration: weight of each action and severity cutoffs.
///
/// **Everything is exported and tunable**: there are no hidden constants.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ScoreParams {
    /// Points per net kill in your favor.
    pub kills: f64,
    /// Points per net tower in your favor.
    pub tower: f64,
    /// Points per net plate in your favor.
    pub plate: f64,
    /// Points per net elemental dragon in your favor.
    pub dragon: f64,
    /// Points per net herald in your favor.
    pub herald: f64,
    /// Points per net baron in your favor.
    pub baron: f64,
    /// Points per net CS (minion) in your favor.
    pub cs: f64,
    /// Cap of the CS axis per window (0 = no cap).
    ///
    /// By default there is **no cap**: farm enters with its real value and a
    /// big CS wave can be critical. It can be trimmed if it gets in the way.
    pub cs_cap: f64,
    /// Dragon buff duration (2 min): it is only **reported**, it does not add
    /// to the score (the analyst matches it with later rises).
    pub dragon_buff_dur_s: f32,
    /// Herald buff duration (2 min).
    pub herald_buff_dur_s: f32,
    /// Baron buff duration (3:30).
    pub baron_buff_dur_s: f32,
    /// Severity cutoffs, in score points.
    pub thresholds: SeverityThresholds,
}

impl Default for ScoreParams {
    fn default() -> Self {
        Self {
            // Score equivalences (they are not real gold: the live API does
            // not expose gold earned).
            kills: 3.9,
            tower: 3.25,
            plate: 3.25,
            dragon: 2.6,
            herald: 5.85,
            baron: 3.9,
            cs: 0.26,
            cs_cap: 0.0,
            dragon_buff_dur_s: 120.0,
            herald_buff_dur_s: 120.0,
            baron_buff_dur_s: 210.0,
            thresholds: SeverityThresholds::default(),
        }
    }
}

/// Contribution of one axis in a block.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AxisContribution {
    /// Axis name (`kills`, `towers`, `plates`, `dragons`, `heralds`,
    /// `barons`, `cs`).
    pub axis: String,
    /// `false` if the feed emitted no data of this kind in the scrim: the
    /// contribution is a **documented** 0, not an invented value.
    pub available: bool,
    /// Measured net differential in axis units (Blue − Red).
    pub net_blue: f64,
    /// Contribution to the score in points (`weight × net`; 0 if unavailable).
    pub contribution: f64,
    /// Blue team's input in the category (for the comparison table).
    #[serde(default)]
    pub blue: f64,
    /// Red team's input in the category.
    #[serde(default)]
    pub red: f64,
}

/// Dominion score of one 30 s block.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DominionBlock {
    pub index: u32,
    pub start_s: f32,
    pub end_s: f32,
    /// Blue's net score in the block (sum of axis contributions).
    pub score_net_blue: f64,
    /// Severity level of the window (`None` ⇒ below the floor, no alert).
    pub severity: Option<Severity>,
    /// Per-axis breakdown (transparency: what drove the score).
    pub axes: Vec<AxisContribution>,
    /// Axes with real data in this block.
    pub componentes_disponibles: Vec<String>,
}

/// Entry of a team's score Top 5.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ScoreTopBlock {
    pub block_index: u32,
    pub start_s: f32,
    pub end_s: f32,
    /// Net score of the block from the team's perspective (+ = in favor).
    pub net: f64,
}

/// Top 5 score blocks of a team.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ScoreTop5 {
    pub exitos: Vec<ScoreTopBlock>,
    pub desastres: Vec<ScoreTopBlock>,
}

/// Severity level of a window (score or vision).
///
/// Derived order: `VentajaMinima < VentajaMedia < VentajaConsiderable <
/// VentajaCritica`. It serializes to a readable label in `snake_case`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// ≥ 500 g (gold) or |deviation| ≥ 0.08 (vision): slight advantage.
    VentajaMinima,
    /// ≥ 800 g (gold): moderate advantage. Vision does not use this level.
    VentajaMedia,
    /// ≥ 1200 g (gold) or |deviation| ≥ 0.16 (vision): strong advantage.
    VentajaConsiderable,
    /// ≥ 1500 g (gold) or |deviation| ≥ 0.20 (vision): critical advantage.
    VentajaCritica,
}

impl Severity {
    /// Readable label in Spanish (used by CLI/summaries).
    pub fn label(&self) -> &'static str {
        match self {
            Severity::VentajaMinima => "ventaja leve",
            Severity::VentajaMedia => "ventaja moderada",
            Severity::VentajaConsiderable => "ventaja fuerte",
            Severity::VentajaCritica => "ventaja crítica",
        }
    }

    /// Readable label for the vision module (same levels, another reading).
    ///
    /// Same names as the score: *slight / moderate / strong / critical
    /// advantage*. The old labels ("total blindness", "lost map control")
    /// overstated what a deviation measured in a 30 s window with two or three
    /// wards actually means.
    pub fn label_vision(&self) -> &'static str {
        match self {
            Severity::VentajaMinima => "ventaja leve",
            Severity::VentajaMedia => "ventaja moderada",
            Severity::VentajaConsiderable => "ventaja fuerte",
            Severity::VentajaCritica => "ventaja crítica",
        }
    }
}

impl std::fmt::Display for Severity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// Score severity thresholds. **Exported and tunable.**
///
/// Since the migration to real gold, the cutoffs are in **net gold** of a
/// 30 s window: 1 kill ≈ 300 g, one plate ≈ 175 g, one tower ≈ 250 g.
///
/// | Interval | Level | Real anchor |
/// |---|---|---|
/// | [500, 800) | `slight advantage` | one plate + some farm |
/// | [800, 1200) | `moderate advantage` | ~3 kills or one tower + farm |
/// | [1200, 1500) | `strong advantage` | one tower + kills |
/// | ≥ 1500 | `critical advantage` | a baron or a won fight with objectives |
///
/// The floor (`semi_alerta`) decides how many windows enter `breakpoints`;
/// the higher cutoffs only label. The Top 5 does not depend on these values.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SeverityThresholds {
    /// Alert floor: `|gold| ≥ 500 g` ⇒ slight advantage.
    pub semi_alerta: f64,
    /// `|gold| ≥ 800 g` ⇒ moderate advantage.
    pub ventaja_media: f64,
    /// `|gold| ≥ 1200 g` ⇒ strong advantage.
    pub ventaja_considerable: f64,
    /// `|gold| ≥ 1500 g` ⇒ critical advantage.
    pub ventaja_critica: f64,
}

impl Default for SeverityThresholds {
    fn default() -> Self {
        Self {
            semi_alerta: 500.0,         // Slight advantage (≥ 500g)
            ventaja_media: 800.0,       // Moderate advantage (≥ 800g)
            ventaja_considerable: 1200.0,// Strong advantage (≥ 1200g)
            ventaja_critica: 1500.0,    // Critical advantage (≥ 1500g)
        }
    }
}

impl SeverityThresholds {
    /// Classifies a magnitude (`|score|` or `|deviation|`).
    /// Returns `None` if it does not reach the floor ⇒ it is not an alert.
    pub fn classify(&self, magnitude: f64) -> Option<Severity> {
        if magnitude >= self.ventaja_critica {
            Some(Severity::VentajaCritica)
        } else if magnitude >= self.ventaja_considerable {
            Some(Severity::VentajaConsiderable)
        } else if magnitude >= self.ventaja_media {
            Some(Severity::VentajaMedia)
        } else if magnitude >= self.semi_alerta {
            Some(Severity::VentajaMinima)
        } else {
            None
        }
    }
}

/// Break alert: a 30 s window that exceeded the severity floor.
///
/// Each block generates its own entry (**no merging** of consecutive ones).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct BreakAlert {
    pub block_index: u32,
    pub start_s: f32,
    pub end_s: f32,
    /// Blue's net score in the window (with sign).
    pub score_net_blue: f64,
    /// `|score_net_blue|`: used to classify and sort the list.
    pub magnitude: f64,
    pub severity: Severity,
    /// Team favored by the window (sign of the score).
    pub favorecido: Team,
}

/// **Notice** of an epic objective: a team killed a dragon/herald/baron.
///
/// It adds nothing to the score (the buff is temporary and is reported
/// separately: the analyst matches it with the later score rises).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObjectiveEvent {
    /// Team that killed it.
    pub team: Team,
    /// Instant when it happened (end of the window where it fell).
    pub start_s: f32,
    /// Instant when its buff expires.
    pub end_s: f32,
    /// Buff duration in seconds.
    pub dur_s: f32,
    /// Points the objective contributes in its window.
    pub puntos: f64,
    /// Objective type: `"dragon"`, `"herald"`, `"baron"`.
    pub tipo: String,
}

/// Vision-share severity thresholds. **Exported and tunable.**
///
/// Parity is 0.5; `desvio = cuota_azul − 0.5`. The cutoffs are on
/// `|deviation|`, symmetric for and against, and use **the same four levels
/// as the score** (slight / moderate / strong / critical).
///
/// `actividad_minima` requires a minimum wardScore activity in the window
/// (`Δazul + Δrojo`): a share computed over almost no activity is noise, not
/// information.
///
/// `prior_wards` is a **shrinkage**: the share is computed as
/// `(Δazul + p) / (Δazul + Δrojo + 2p)`, that is, as if each team had placed
/// `p` wards to begin with. Without it, a window with 2 wards from one team
/// and 0 from the other gave 100 % / 0 % and jumped to "critical" because of a
/// sampling artifact.
///
/// `window_secs` is the length of the vision window: **60 s**, not 30 s.
/// In 30 s a team places 1–2 wards and the split is noise; in a minute it
/// already means something (and there are still plenty of windows to alert).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct VisionThresholds {
    /// `|deviation| ≥ 0.08` ⇒ slight advantage.
    pub semi_alerta: f64,
    /// `|deviation| ≥ 0.12` ⇒ moderate advantage.
    pub ventaja_media: f64,
    /// `|deviation| ≥ 0.16` ⇒ strong advantage.
    pub ventaja_considerable: f64,
    /// `|deviation| ≥ 0.20` ⇒ critical advantage.
    pub ventaja_critica: f64,
    /// Minimum activity in the window (`Δazul + Δrojo`) to define a share.
    pub actividad_minima: f64,
    /// "Expected" wards per team (share shrinkage). `0` = no shrinkage (raw
    /// share, very aggressive with small samples).
    pub prior_wards: f64,
    /// Length of the vision window in seconds (60 = 1 minute).
    pub window_secs: f32,
}

impl Default for VisionThresholds {
    fn default() -> Self {
        Self {
            semi_alerta: 0.08,
            ventaja_media: 0.12,
            ventaja_considerable: 0.16,
            ventaja_critica: 0.20,
            actividad_minima: 4.0,
            prior_wards: 1.2,
            window_secs: 60.0,
        }
    }
}

impl VisionThresholds {
    /// Classifies a vision-share deviation (same 4 levels as the score).
    pub fn classify(&self, desvio: f64) -> Option<Severity> {
        if desvio >= self.ventaja_critica {
            Some(Severity::VentajaCritica)
        } else if desvio >= self.ventaja_considerable {
            Some(Severity::VentajaConsiderable)
        } else if desvio >= self.ventaja_media {
            Some(Severity::VentajaMedia)
        } else if desvio >= self.semi_alerta {
            Some(Severity::VentajaMinima)
        } else {
            None
        }
    }
}

/// A vision-share deviation alert in a block.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VisionAlert {
    pub block_index: u32,
    pub start_s: f32,
    pub end_s: f32,
    /// Blue's vision share in the block (0..1). Parity = 0.5.
    pub cuota_azul: f64,
    /// Measured `wardScore` delta for Blue in the block.
    pub delta_azul: f64,
    /// Measured `wardScore` delta for Red in the block.
    pub delta_rojo: f64,
    /// Deviation from parity (`cuota_azul − 0.5`). Positive ⇒ favors Blue.
    pub desvio: f64,
    /// Severity level of the deviation ([`VisionThresholds`] thresholds).
    pub severity: Severity,
    /// Team that dominates vision in this block.
    pub favorecido: Team,
}

/// Report of the vision signal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VisionReport {
    /// Thresholds used (exported, tunable).
    pub thresholds: VisionThresholds,
    /// Measured metric the share was computed from: `"wardScore"` or
    /// `"wards_placed"` (`/stats`). Empty if the capture has no vision.
    pub fuente: String,
    /// Blocks with vision activity considered: now **windows** of
    /// `thresholds.window_secs` (2 min) with activity (Δazul + Δrojo > 0).
    pub ventanas_con_visita: u32,
    /// Alerts, sorted by deviation magnitude (largest first).
    pub alerts: Vec<VisionAlert>,
}

/// Combined report: dominion score + vision.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DominionReport {
    pub fuente: String,
    pub block_secs: u32,
    /// Calibration used (weights and cutoffs). **Exported.**
    pub params: ScoreParams,
    /// Score severity thresholds (exported, tunable).
    pub severity_thresholds: SeverityThresholds,
    pub blocks: Vec<DominionBlock>,
    /// Windows with `|score| ≥ floor`, sorted by magnitude (largest first).
    /// Each block separately: **no merging** of consecutive ones.
    pub breakpoints: Vec<BreakAlert>,
    /// Epic objective notices (dragon/herald/baron): moment, team and buff
    /// duration. **They do not add to the score**.
    pub avisos: Vec<ObjectiveEvent>,
    pub top5_azul: ScoreTop5,
    pub top5_rojo: ScoreTop5,
    pub vision: VisionReport,
}

/// Did the feed emit any tower/plate? any objective? was CS measured?
/// (global flags).
///
/// CS is measured per snapshot (`scores.creepScore`): if during the whole
/// capture no block recorded CS activity, the capture lacks that measurement
/// (e.g. old builds with very few polls) and the axis is marked **not
/// available** instead of claiming availability with a silent zero.
fn feed_flags(blocks: &[ScrimBlock]) -> (bool, bool, bool) {
    let structures = blocks.iter().any(|b| {
        b.towers_blue + b.towers_red + b.plates_blue + b.plates_red > 0
    });
    let objectives = blocks.iter().any(|b| {
        b.objectives_blue + b.objectives_red > 0
            || b.dragons_blue + b.dragons_red > 0
            || b.heralds_blue + b.heralds_red > 0
            || b.barons_blue + b.barons_red > 0
    });
    let cs = blocks
        .iter()
        .any(|b| b.cs_blue_delta != 0 || b.cs_red_delta != 0);
    (structures, objectives, cs)
}

fn axis(name: &str, blue: f64, red: f64, contribution: f64) -> AxisContribution {
    AxisContribution {
        axis: name.to_string(),
        available: true,
        net_blue: blue - red,
        contribution,
        blue,
        red,
    }
}

fn unaxis(name: &str) -> AxisContribution {
    AxisContribution {
        axis: name.to_string(),
        available: false,
        net_blue: 0.0,
        contribution: 0.0,
        blue: 0.0,
        red: 0.0,
    }
}

/// Computes the dominion score per block.
///
/// - Each action contributes its weight: kill 3.9 · tower 3.25 · plate 3.25 ·
///   dragon 2.6 · herald 5.85 · baron 3.9 · CS 0.26 (15 CS ≈ 1 kill).
/// - CS has an optional cap (`cs_cap`; 0 = no cap) so that a big wave does
///   not sweep the score: by default farm enters with its real value and
///   **can** be a critical window.
/// - Epic objectives do **not** add their buff to the score: they are reported
///   separately ([`ObjectiveEvent`]).
pub fn compute_dominion(
    blocks: &[ScrimBlock],
    params: ScoreParams,
    block_secs: u32,
) -> (Vec<DominionBlock>, Vec<ObjectiveEvent>) {
    let (structures, objectives, cs_measured) = feed_flags(blocks);
    let mut avisos: Vec<ObjectiveEvent> = Vec::new();
    let mut out: Vec<DominionBlock> = Vec::with_capacity(blocks.len());

    for b in blocks {
        let mut axes: Vec<AxisContribution> = Vec::with_capacity(8);
        let mut score = 0.0f64;
        let mut avail: Vec<String> = Vec::new();

        // K — kills (always available).
        let kills_net = f64::from(b.kills_blue) - f64::from(b.kills_red);
        let c = params.kills * kills_net;
        score += c;
        axes.push(axis(
            "kills",
            f64::from(b.kills_blue) * params.kills,
            f64::from(b.kills_red) * params.kills,
            c,
        ));
        avail.push("kills".to_string());

        // E — structures (towers + plates): only if the feed emitted them.
        if structures {
            let towers_net = f64::from(b.towers_blue) - f64::from(b.towers_red);
            let c = params.tower * towers_net;
            score += c;
            axes.push(axis(
                "towers",
                f64::from(b.towers_blue) * params.tower,
                f64::from(b.towers_red) * params.tower,
                c,
            ));
            avail.push("towers".to_string());

            let plates_net = f64::from(b.plates_blue) - f64::from(b.plates_red);
            let c = params.plate * plates_net;
            score += c;
            axes.push(axis(
                "plates",
                f64::from(b.plates_blue) * params.plate,
                f64::from(b.plates_red) * params.plate,
                c,
            ));
            avail.push("plates".to_string());
        } else {
            axes.push(unaxis("towers"));
            axes.push(unaxis("plates"));
        }

        // O — epic objectives: dragon, herald and baron separately (the feed
        // says which one it is). They only contribute their own weight: the
        // buff is reported separately.
        if objectives {
            let dragones = f64::from(b.dragons_blue) - f64::from(b.dragons_red);
            let c = params.dragon * dragones;
            score += c;
            axes.push(axis(
                "dragons",
                f64::from(b.dragons_blue) * params.dragon,
                f64::from(b.dragons_red) * params.dragon,
                c,
            ));
            avail.push("dragons".to_string());
            registrar_aviso(
                &mut avisos,
                b,
                Team::Blue,
                b.dragons_blue,
                Team::Red,
                b.dragons_red,
                params.dragon_buff_dur_s,
                params.dragon,
                "dragon",
            );

            let heraldos = f64::from(b.heralds_blue) - f64::from(b.heralds_red);
            let c = params.herald * heraldos;
            score += c;
            axes.push(axis(
                "heralds",
                f64::from(b.heralds_blue) * params.herald,
                f64::from(b.heralds_red) * params.herald,
                c,
            ));
            avail.push("heralds".to_string());
            registrar_aviso(
                &mut avisos,
                b,
                Team::Blue,
                b.heralds_blue,
                Team::Red,
                b.heralds_red,
                params.herald_buff_dur_s,
                params.herald,
                "herald",
            );

            let barones = f64::from(b.barons_blue) - f64::from(b.barons_red);
            let c = params.baron * barones;
            score += c;
            axes.push(axis(
                "barons",
                f64::from(b.barons_blue) * params.baron,
                f64::from(b.barons_red) * params.baron,
                c,
            ));
            avail.push("barons".to_string());
            registrar_aviso(
                &mut avisos,
                b,
                Team::Blue,
                b.barons_blue,
                Team::Red,
                b.barons_red,
                params.baron_buff_dur_s,
                params.baron,
                "baron",
            );

            // Objectives of unknown type (old captures): valued as a dragon
            // (its weight, no buff: it is impossible to know how long it lasts).
            let otros_blue = (f64::from(b.objectives_blue)
                - f64::from(b.dragons_blue)
                - f64::from(b.heralds_blue)
                - f64::from(b.barons_blue))
                .max(0.0);
            let otros_red = (f64::from(b.objectives_red)
                - f64::from(b.dragons_red)
                - f64::from(b.heralds_red)
                - f64::from(b.barons_red))
                .max(0.0);
            let otros = otros_blue - otros_red;
            if otros != 0.0 {
                let c = params.dragon * otros;
                score += c;
                axes.push(axis(
                    "objectives",
                    otros_blue * params.dragon,
                    otros_red * params.dragon,
                    c,
                ));
            }
            avail.push("objectives".to_string());
        } else {
            axes.push(unaxis("dragons"));
            axes.push(unaxis("heralds"));
            axes.push(unaxis("barons"));
        }

        // Inhibitors: **informational** notice (they do not add to the score:
        // their gold value is small; what they provide is pressure and a late
        // respawn).
        if b.inhibs_blue != 0 || b.inhibs_red != 0 {
            registrar_aviso(
                &mut avisos,
                b,
                Team::Blue,
                b.inhibs_blue,
                Team::Red,
                b.inhibs_red,
                0.0,
                0.0,
                "inhib",
            );
        }

        // C — CS: enters with its real value (0.26 per minion): a big CS wave
        // **can** be a critical window.
        if cs_measured {
            let cs_net = (b.cs_blue_delta - b.cs_red_delta) as f64;
            let c = if params.cs_cap > 0.0 {
                (params.cs * cs_net).clamp(-params.cs_cap, params.cs_cap)
            } else {
                params.cs * cs_net
            };
            score += c;
            axes.push(axis(
                "cs",
                (b.cs_blue_delta as f64) * params.cs,
                (b.cs_red_delta as f64) * params.cs,
                c,
            ));
            avail.push("cs".to_string());
        } else {
            axes.push(unaxis("cs"));
        }

        let _ = block_secs;
        out.push(DominionBlock {
            index: b.index,
            start_s: b.start_s,
            end_s: b.end_s,
            score_net_blue: score,
            severity: params.thresholds.classify(score.abs()),
            axes,
            componentes_disponibles: avail,
        });
    }
    (out, avisos)
}

/// Records the epic objective notices of this block.
fn registrar_aviso(
    avisos: &mut Vec<ObjectiveEvent>,
    b: &ScrimBlock,
    team_blue: Team,
    n_blue: u32,
    team_red: Team,
    n_red: u32,
    dur_s: f32,
    puntos: f64,
    tipo: &str,
) {
    for (team, n) in [(team_blue, n_blue), (team_red, n_red)] {
        for _ in 0..n {
            avisos.push(ObjectiveEvent {
                team,
                start_s: b.end_s,
                end_s: b.end_s + dur_s,
                dur_s,
                puntos,
                tipo: tipo.to_string(),
            });
        }
    }
}

/// Top 5 scores per team (successes and disasters), from the team's perspective.
///
/// It is also used from the GUI after injecting the LCU's **real gold**
/// blocks: in that case `score_net_blue` is net gold and the Top 5 is in grams.
pub fn score_top5(blocks: &[DominionBlock]) -> (ScoreTop5, ScoreTop5) {
    let mk = |own_is_blue: bool| -> ScoreTop5 {
        if blocks.is_empty() {
            return ScoreTop5::default();
        }
        let mut entries: Vec<ScoreTopBlock> = blocks
            .iter()
            .map(|b| ScoreTopBlock {
                block_index: b.index,
                start_s: b.start_s,
                end_s: b.end_s,
                net: if own_is_blue {
                    b.score_net_blue
                } else {
                    -b.score_net_blue
                },
            })
            .collect();
        entries.sort_by(|a, c| {
            c.net.partial_cmp(&a.net)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.block_index.cmp(&c.block_index))
        });
        let n = entries.len().min(5);
        ScoreTop5 {
            exitos: entries[..n].to_vec(),
            desastres: entries[entries.len().saturating_sub(n)..].to_vec(),
        }
    };
    (mk(true), mk(false))
}

/// List of breaks: each window with `|score| ≥ floor`, **separately**.
///
/// No merging of consecutive ones: two blocks in a row with an alert are two
/// independent decision windows and both notices are kept.
/// Sorted by magnitude (largest first) and, on ties, by index.
pub fn compute_breakpoints(blocks: &[DominionBlock]) -> Vec<BreakAlert> {
    let mut out: Vec<BreakAlert> = blocks
        .iter()
        .filter_map(|b| {
            let magnitude = b.score_net_blue.abs();
            b.severity.map(|severity| BreakAlert {
                block_index: b.index,
                start_s: b.start_s,
                end_s: b.end_s,
                score_net_blue: b.score_net_blue,
                magnitude,
                severity,
                favorecido: if b.score_net_blue >= 0.0 {
                    Team::Blue
                } else {
                    Team::Red
                },
            })
        })
        .collect();
    out.sort_by(|a, c| {
        c.magnitude
            .partial_cmp(&a.magnitude)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.block_index.cmp(&c.block_index))
    });
    out
}

/// Computes the vision signal: share per window + symmetric alerts by level.
///
/// The vision window is `t.window_secs` long (2 minutes by default): the 30 s
/// blocks are grouped. In 30 s the ward split is noise; in 2 minutes it
/// already says something.
///
/// `t` contains the severity cutoffs (0.15 / 0.25 / 0.35 by default), the
/// minimum activity required from the window and the shrinkage
/// (`prior_wards`). Alerts are emitted **one per window, without merging**.
pub fn compute_vision(blocks: &[ScrimBlock], t: VisionThresholds) -> VisionReport {
    let mut alerts: Vec<VisionAlert> = Vec::new();
    let mut con_visita = 0u32;

    // Number of 30 s blocks per vision window.
    let por_ventana = ((t.window_secs / 30.0).round() as usize).max(1);

    for chunk in blocks.chunks(por_ventana) {
        let Some(first) = chunk.first() else { continue };
        let Some(last) = chunk.last() else { continue };
        let d_azul: f64 = chunk.iter().map(|b| b.ward_blue_delta).sum();
        let d_rojo: f64 = chunk.iter().map(|b| b.ward_red_delta).sum();
        let total = d_azul + d_rojo;
        if total <= 0.0 {
            continue; // no vision activity: share not defined.
        }
        con_visita += 1;
        if total < t.actividad_minima {
            continue; // insufficient activity: share over noise, no alert.
        }
        // **Shrunk** share: as if each team had placed `prior_wards` wards.
        // Without this, 2 wards against 0 gave 100 %/0 % and jumped to
        // "critical" because of a sampling artifact.
        let p = t.prior_wards.max(0.0);
        let cuota = (d_azul + p) / (total + 2.0 * p);
        let desvio = cuota - 0.5;
        if let Some(severity) = t.classify(desvio.abs()) {
            alerts.push(VisionAlert {
                block_index: first.index,
                start_s: first.start_s,
                end_s: last.end_s,
                cuota_azul: cuota,
                delta_azul: d_azul,
                delta_rojo: d_rojo,
                desvio,
                severity,
                favorecido: if desvio > 0.0 { Team::Blue } else { Team::Red },
            });
        }
    }
    // Largest deviation first (magnitude).
    alerts.sort_by(|a, c| {
        c.desvio
            .abs()
            .partial_cmp(&a.desvio.abs())
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.block_index.cmp(&c.block_index))
    });

    VisionReport {
        thresholds: t,
        fuente: blocks
            .iter()
            .map(|b| b.vision_source.as_str())
            .find(|s| !s.is_empty())
            .unwrap_or("")
            .to_string(),
        ventanas_con_visita: con_visita,
        alerts,
    }
}

/// Combined report: dominion score + breaks + Top 5 + vision alerts.
///
/// `fuente` is copied as-is (e.g. `"live_client_data_api"`).
pub fn compute_report(
    blocks: &[ScrimBlock],
    params: ScoreParams,
    vision: VisionThresholds,
    fuente: &str,
) -> DominionReport {
    let (dblocks, avisos) = compute_dominion(blocks, params, 30);
    let breakpoints = compute_breakpoints(&dblocks);
    let (top5_azul, top5_rojo) = score_top5(&dblocks);
    let vreport = compute_vision(blocks, vision);
    DominionReport {
        fuente: fuente.to_string(),
        block_secs: 30,
        params,
        severity_thresholds: params.thresholds,
        blocks: dblocks,
        breakpoints,
        avisos,
        top5_azul,
        top5_rojo,
        vision: vreport,
    }
}

/// Cumulative score series: `(second, accumulated advantage in points)`.
pub fn accumulated_score_series(report: &DominionReport) -> Vec<(f32, f64)> {
    let mut acc = 0.0;
    let mut serie: Vec<(f32, f64)> = vec![(0.0, 0.0)];
    for b in &report.blocks {
        acc += b.score_net_blue;
        serie.push((b.end_s, acc));
    }
    serie
}

/// Accumulated score advantage at instant `t`.
pub fn accumulated_score_at(report: &DominionReport, t: f32) -> f64 {
    report
        .blocks
        .iter()
        .filter(|b| b.end_s <= t + 0.001)
        .map(|b| b.score_net_blue)
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Synthetic block with controlled values.
    fn blk(index: u32, start: f32) -> ScrimBlock {
        ScrimBlock {
            index,
            start_s: start,
            end_s: start + 30.0,
            ..Default::default()
        }
    }

    /// Test report with **legacy cutoffs in points**.
    ///
    /// `SeverityThresholds::default()` is in gold (500/800/1200/1500) because
    /// the app delivers real LCU gold blocks. These unit tests exercise the
    /// score mechanics in points (kills, CS, towers) with the old cutoffs, so
    /// that the historical numbers remain readable; the gold path is covered
    /// in `gold_breakdown`.
    fn rep_def(blocks: &[ScrimBlock]) -> DominionReport {
        let params = ScoreParams {
            thresholds: SeverityThresholds {
                semi_alerta: 4.0,
                ventaja_media: 6.0,
                ventaja_considerable: 8.0,
                ventaja_critica: 10.0,
            },
            ..ScoreParams::default()
        };
        compute_report(blocks, params, VisionThresholds::default(), "test")
    }

    // -- Score --------------------------------------------------------------

    #[test]
    fn score_refleja_diferencial_medido() {
        let mut b = blk(0, 0.0);
        b.kills_blue = 3;
        b.kills_red = 1;
        b.cs_blue_delta = 20;
        b.cs_red_delta = 10;
        let rep = rep_def(&[b]);
        let d = &rep.blocks[0];
        // kills: (3−1)×3.9 = 7.8 ; cs: (20−10)×0.26 = 2.6 ⇒ 10.4
        assert!((d.score_net_blue - 10.4).abs() < 1e-9, "{}", d.score_net_blue);
        assert_eq!(d.componentes_disponibles, vec!["kills", "cs"]);
    }

    #[test]
    fn quince_cs_valen_una_kill() {
        // 15 CS × 0.26 = 3.9 = 1 kill (3.9).
        let mut b = blk(0, 0.0);
        b.cs_blue_delta = 15;
        let cs = rep_def(&[b]).blocks[0].score_net_blue;
        let mut b2 = blk(0, 0.0);
        b2.kills_blue = 1;
        let kill = rep_def(&[b2]).blocks[0].score_net_blue;
        assert!((cs - 3.9).abs() < 1e-9);
        assert!((kill - 3.9).abs() < 1e-9);
        assert!((kill - cs).abs() < 1e-9, "15 CS = 1 kill");
    }

    #[test]
    fn el_farm_puede_ser_critico() {
        // CS enters with its real value: a big wave CAN be critical.
        let mut b = blk(0, 0.0);
        b.cs_blue_delta = 60; // 15.6 points
        let rep = rep_def(&[b]);
        assert!((rep.blocks[0].score_net_blue - 15.6).abs() < 1e-9);
        assert_eq!(rep.blocks[0].severity, Some(Severity::VentajaCritica));
    }

    #[test]
    fn el_tope_del_cs_es_opcional() {
        // With a cap, a 60 CS wave is trimmed; by default there is no cap.
        let mut b = blk(0, 0.0);
        b.cs_blue_delta = 60;
        let mut params = ScoreParams::default();
        params.cs_cap = 5.0;
        let rep = compute_report(
            &[b],
            params,
            VisionThresholds::default(),
            "test",
        );
        assert!((rep.blocks[0].score_net_blue - 5.0).abs() < 1e-9);
    }

    #[test]
    fn ejes_no_disponibles_no_rellenan() {
        // No tower/objective events and no CS measurement ⇒ marked not
        // available (no silent zeros that dilute the score).
        let mut b = blk(0, 0.0);
        b.kills_blue = 1;
        let rep = rep_def(&[b]);
        let d = &rep.blocks[0];
        assert_eq!(d.componentes_disponibles, vec!["kills"]);
        let towers = d.axes.iter().find(|a| a.axis == "towers").unwrap();
        assert!(!towers.available);
        assert_eq!(towers.contribution, 0.0);
        let cs = d.axes.iter().find(|a| a.axis == "cs").unwrap();
        assert!(!cs.available);
        assert_eq!(cs.contribution, 0.0);
        // The score is not diluted by zeros from missing axes: kills only = 3.9.
        assert!((d.score_net_blue - 3.9).abs() < 1e-9);
    }

    #[test]
    fn torres_y_objetivos_disponibles_si_hay_eventos() {
        let mut b = blk(0, 0.0);
        b.towers_blue = 1;
        b.dragons_red = 1;
        let rep = rep_def(&[b]);
        let d = &rep.blocks[0];
        assert!(d.componentes_disponibles.contains(&"towers".to_string()));
        assert!(d.componentes_disponibles.contains(&"dragons".to_string()));
        // blue tower +3.25, red dragon −2.6 ⇒ +0.65 (its buff does NOT add).
        assert!((d.score_net_blue - 0.65).abs() < 1e-9, "{}", d.score_net_blue);
    }

    #[test]
    fn el_objetivo_solo_avisa_no_suma_su_buff() {
        // The baron contributes its weight (3.9) and generates a notice with
        // its buff duration, but the buff does **not** add to the score or to
        // the accumulated total.
        let mut b = blk(0, 0.0);
        b.barons_blue = 1;
        let rep = rep_def(&[b]);
        assert_eq!(rep.avisos.len(), 1);
        let av = &rep.avisos[0];
        assert_eq!(av.tipo, "baron");
        assert_eq!(av.team, Team::Blue);
        assert!((av.puntos - 3.9).abs() < 1e-9);
        assert!((av.dur_s - 210.0).abs() < 1e-9);
        assert!((rep.blocks[0].score_net_blue - 3.9).abs() < 1e-9);

        // The accumulated total does not carry the buff: it is the sum of the
        // windows.
        let serie = accumulated_score_series(&rep);
        assert!((serie.last().unwrap().1 - 3.9).abs() < 1e-9);
    }

    // -- Severity ----------------------------------------------------------

    #[test]
    fn severidad_con_los_cuatro_niveles_del_juego() {
        // Intervals: [4,6) slight · [6,8) moderate · [8,10) strong · ≥10 critical.

        // 1 kill (3.9) ⇒ below the floor: no alert.
        let mut b = blk(0, 0.0);
        b.kills_blue = 1;
        assert_eq!(rep_def(&[b]).blocks[0].severity, None);

        // 1 kill + 5 CS (3.9 + 1.3 = 5.2) ⇒ slight advantage.
        let mut b = blk(0, 0.0);
        b.kills_blue = 1;
        b.cs_blue_delta = 5;
        assert_eq!(
            rep_def(&[b]).blocks[0].severity,
            Some(Severity::VentajaMinima)
        );

        // 1 kill + 1 tower (3.9 + 3.25 = 7.15) ⇒ moderate advantage.
        let mut b = blk(0, 0.0);
        b.kills_blue = 1;
        b.towers_blue = 1;
        assert_eq!(
            rep_def(&[b]).blocks[0].severity,
            Some(Severity::VentajaMedia)
        );

        // 2 kills (7.8) ⇒ moderate advantage.
        let mut b = blk(0, 0.0);
        b.kills_blue = 2;
        assert_eq!(
            rep_def(&[b]).blocks[0].severity,
            Some(Severity::VentajaMedia)
        );

        // 2 kills + 4 CS (7.8 + 1.04 = 8.84) ⇒ strong advantage.
        let mut b = blk(0, 0.0);
        b.kills_blue = 2;
        b.cs_blue_delta = 4;
        assert_eq!(
            rep_def(&[b]).blocks[0].severity,
            Some(Severity::VentajaConsiderable)
        );

        // 2 kills + tower (7.8 + 3.25 = 11.05) ⇒ critical advantage.
        let mut b = blk(0, 0.0);
        b.kills_blue = 2;
        b.towers_blue = 1;
        assert_eq!(
            rep_def(&[b]).blocks[0].severity,
            Some(Severity::VentajaCritica)
        );

        // 3 kills (11.7) ⇒ critical advantage.
        let mut b = blk(0, 0.0);
        b.kills_blue = 3;
        assert_eq!(
            rep_def(&[b]).blocks[0].severity,
            Some(Severity::VentajaCritica)
        );
    }

    #[test]
    fn una_kill_suelta_no_avisa_y_una_doble_es_moderada() {
        let mut b = blk(0, 0.0);
        b.kills_blue = 1;
        let rep = rep_def(&[b]);
        assert!(rep.breakpoints.is_empty(), "una kill aislada no avisa");

        let mut b = blk(0, 0.0);
        b.kills_blue = 2;
        let rep = rep_def(&[b]);
        assert!((rep.breakpoints[0].score_net_blue - 7.8).abs() < 1e-9);
        assert_eq!(rep.breakpoints[0].severity, Severity::VentajaMedia);
    }

    #[test]
    fn quiebre_usa_el_signo_para_el_favorecido() {
        let mut b = blk(0, 0.0);
        b.kills_red = 2; // −7.8 ⇒ moderate for Red
        let rep = rep_def(&[b]);
        assert_eq!(rep.breakpoints.len(), 1);
        let q = &rep.breakpoints[0];
        assert_eq!(q.severity, Severity::VentajaMedia);
        assert_eq!(q.favorecido, Team::Red);
        assert!((q.score_net_blue - (-7.8)).abs() < 1e-9);
    }

    #[test]
    fn dos_bloques_consecutivos_con_alerta_se_emiten_por_separado() {
        // Two mistakes in a row in different windows: both notices are kept
        // (no merging of episodes).
        let mut b0 = blk(0, 0.0);
        b0.kills_blue = 2; // +7.8
        let mut b1 = blk(1, 30.0);
        b1.kills_red = 2; // −7.8
        let rep = rep_def(&[b0, b1]);
        assert_eq!(rep.breakpoints.len(), 2, "cada bloque genera su propia alerta");
        assert_eq!(rep.breakpoints[0].block_index, 0);
        assert_eq!(rep.breakpoints[1].block_index, 1);
        assert_eq!(rep.breakpoints[0].favorecido, Team::Blue);
        assert_eq!(rep.breakpoints[1].favorecido, Team::Red);
    }

    #[test]
    fn ventana_tranquila_no_genera_quiebre() {
        // No kills or structures: not a single alert.
        let b = blk(0, 0.0);
        assert!(rep_def(&[b]).breakpoints.is_empty());
    }

    #[test]
    fn quiebres_ordenados_por_magnitud() {
        let mut b0 = blk(0, 0.0);
        b0.kills_blue = 2; // 7.8 ⇒ moderate
        let mut b1 = blk(1, 30.0);
        b1.kills_blue = 3; // 11.7 ⇒ critical
        let mut b2 = blk(2, 60.0);
        b2.kills_blue = 1; // 3.9 ⇒ no alert
        let rep = rep_def(&[b0, b1, b2]);
        assert_eq!(rep.breakpoints.len(), 2);
        assert_eq!(rep.breakpoints[0].block_index, 1); // 11.7 first
        assert_eq!(rep.breakpoints[1].block_index, 0); // 7.8 afterwards
    }

    #[test]
    fn top5_score_perspectiva_por_equipo() {
        let mut b0 = blk(0, 0.0);
        b0.kills_blue = 5; // good block for Blue
        let mut b1 = blk(1, 30.0);
        b1.kills_red = 5; // bad for Blue, good for Red
        let rep = rep_def(&[b0, b1]);
        // Best Blue block = 0; Red's = 1.
        assert_eq!(rep.top5_azul.exitos[0].block_index, 0);
        assert_eq!(rep.top5_rojo.exitos[0].block_index, 1);
    }

    #[test]
    fn bloques_vacios_no_panic() {
        let rep = rep_def(&[]);
        assert!(rep.blocks.is_empty());
        assert!(rep.breakpoints.is_empty());
        assert!(rep.avisos.is_empty());
    }

    // -- Vision -------------------------------------------------------------

    #[test]
    fn vision_simetrica_y_favorece_al_equipo_correcto() {
        // Window with clearly blue vision (Δazul > Δrojo) and real activity.
        let mut b = blk(0, 0.0);
        b.ward_blue_delta = 5.0;
        b.ward_red_delta = 1.0;
        let rep = rep_def(&[b]);
        let v = &rep.vision;
        assert_eq!(v.ventanas_con_visita, 1);
        assert_eq!(v.alerts.len(), 1);
        let a = &v.alerts[0];
        // Shrunk share: (5 + 1.2) / (6 + 2.4) ≈ 0.74 ⇒ deviation ≈ +0.24.
        let esperado = (5.0 + 1.2) / (6.0 + 2.4);
        assert!((a.cuota_azul - esperado).abs() < 1e-9);
        assert!(a.desvio > 0.0);
        assert_eq!(a.favorecido, Team::Blue);
    }

    #[test]
    fn vision_en_contra_tambien_alerta() {
        // Symmetric: now Red dominates.
        let mut b = blk(1, 30.0);
        b.ward_blue_delta = 0.5;
        b.ward_red_delta = 5.0;
        let rep = rep_def(&[b]);
        let a = &rep.vision.alerts[0];
        assert!(a.desvio < 0.0);
        assert_eq!(a.favorecido, Team::Red);
    }

    #[test]
    fn vision_en_parity_no_alerta() {
        let mut b = blk(0, 0.0);
        b.ward_blue_delta = 4.0;
        b.ward_red_delta = 4.0; // same activity ⇒ deviation 0
        let rep = rep_def(&[b]);
        assert!(rep.vision.alerts.is_empty());
        assert_eq!(rep.vision.ventanas_con_visita, 1);
    }

    #[test]
    fn bloques_sin_visita_no_definen_cuota() {
        let b = blk(0, 0.0); // deltas = 0
        let rep = rep_def(&[b]);
        assert_eq!(rep.vision.ventanas_con_visita, 0);
        assert!(rep.vision.alerts.is_empty());
    }

    #[test]
    fn la_vision_se_mide_en_ventanas_de_un_minuto() {
        // The vision window is 1 min (2 blocks of 30 s): the 2 blocks are
        // grouped into ONE alert with the sum of their wards.
        let mut bloques = Vec::new();
        for k in 0..2 {
            let mut b = blk(k, k as f32 * 30.0);
            b.ward_blue_delta = 3.0;
            b.ward_red_delta = 1.0;
            bloques.push(b);
        }
        let rep = rep_def(&bloques);
        assert_eq!(rep.vision.alerts.len(), 1, "2 bloques = 1 ventana de 1 min");
        let a = &rep.vision.alerts[0];
        assert!((a.delta_azul - 6.0).abs() < 1e-9);
        assert!((a.delta_rojo - 2.0).abs() < 1e-9);
        assert!((a.start_s - 0.0).abs() < 1e-9);
        assert!((a.end_s - 60.0).abs() < 1e-9);
    }

    #[test]
    fn el_encogimiento_evita_alertas_agresivas() {
        // 2 wards against 0: insufficient activity (< 4 wards) ⇒ not even
        // measured.
        let mut b = blk(0, 0.0);
        b.ward_blue_delta = 2.0;
        b.ward_red_delta = 0.0;
        let rep = rep_def(&[b]);
        assert!(rep.vision.alerts.is_empty(), "con 2 wards no se puede afirmar nada");
        assert_eq!(rep.vision.ventanas_con_visita, 1);

        // 4 wards against 0: with the shrinkage the share does not reach 100 %.
        let mut b = blk(0, 0.0);
        b.ward_blue_delta = 4.0;
        b.ward_red_delta = 0.0;
        let rep = rep_def(&[b]);
        let a = &rep.vision.alerts[0];
        assert!(a.cuota_azul < 1.0, "la cuota no puede ser 100 %");
        assert_eq!(a.severity, Severity::VentajaCritica);
    }

    #[test]
    fn vision_por_niveles_de_severidad() {
        // 3 vs 1 (4 wards) ⇒ deviation ≈ 0.16 ⇒ moderate advantage.
        let mut b = blk(0, 0.0);
        b.ward_blue_delta = 3.0;
        b.ward_red_delta = 1.0;
        let rep = rep_def(&[b]);
        assert_eq!(
            rep.vision.alerts[0].severity,
            Severity::VentajaMedia,
            "4 wards con desvío medio ⇒ moderada"
        );

        // 7 vs 3 (10 wards) ⇒ deviation ≈ 0.16 ⇒ strong advantage.
        let mut b = blk(0, 0.0);
        b.ward_blue_delta = 7.0;
        b.ward_red_delta = 3.0;
        let rep = rep_def(&[b]);
        assert_eq!(
            rep.vision.alerts[0].severity,
            Severity::VentajaConsiderable,
            "desvío alto ⇒ ventaja fuerte"
        );

        // Real vision release (many wards on one side) ⇒ critical advantage.
        let mut b = blk(0, 0.0);
        b.ward_blue_delta = 14.0;
        b.ward_red_delta = 0.0;
        let rep = rep_def(&[b]);
        assert_eq!(
            rep.vision.alerts[0].severity,
            Severity::VentajaCritica,
            "14 wards contra 0 ⇒ ventaja crítica"
        );
    }

    #[test]
    fn vision_actividad_insuficiente_no_alerta() {
        let mut b = blk(0, 0.0);
        b.ward_blue_delta = 0.4;
        b.ward_red_delta = 0.1; // activity < 4 wards
        let rep = rep_def(&[b]);
        assert!(rep.vision.alerts.is_empty());
    }

    // -- Export -------------------------------------------------------------

    #[test]
    fn umbrales_exportados_en_el_informe() {
        // Production cutoffs are in GOLD (30 s windows).
        let d = SeverityThresholds::default();
        assert_eq!(d.semi_alerta, 500.0);
        assert_eq!(d.ventaja_media, 800.0);
        assert_eq!(d.ventaja_considerable, 1200.0);
        assert_eq!(d.ventaja_critica, 1500.0);
        // The report exports the thresholds used to compute it.
        let rep = rep_def(&[blk(0, 0.0)]);
        assert_eq!(rep.severity_thresholds.semi_alerta, 4.0);
        assert_eq!(rep.params.kills, 3.9);
        assert_eq!(rep.params.cs, 0.26);
    }

    #[test]
    fn informe_serializa_quiebres_y_niveles_en_json() {
        let mut b = blk(0, 0.0);
        b.kills_blue = 2;
        let rep = rep_def(&[b]);
        let json = serde_json::to_string(&rep).unwrap();
        assert!(json.contains("\"severity_thresholds\""));
        assert!(json.contains("\"params\""));
        assert!(json.contains("\"avisos\""));
        assert!(!json.contains("gold"), "el score no se presenta como oro");
    }
}
