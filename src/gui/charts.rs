//! Charts: accumulated gold/vision, time bar and severity legend.
//!
//! Code moved verbatim from `gui_main.rs`; behavior is unchanged.

use super::*;

/// Accumulated score series (Blue − Red) per instant.
///
/// Since the score is permanent (gold × 0.013), the curve is directly the
/// accumulated gold advantage of the game.
pub(crate) fn score_series(report: &DominionReport) -> Vec<(f32, f64)> {
    score::accumulated_score_series(report)
}

/// Accumulated vision series (Δblue − Δred) per instant.
pub(crate) fn vision_series(vision_blocks: &[ScrimBlock]) -> Vec<(f32, f64)> {
    let mut acc = 0.0;
    let mut v = vec![(0.0f32, 0.0f64)];
    for b in vision_blocks {
        acc += b.ward_blue_delta - b.ward_red_delta;
        v.push((b.end_s, acc));
    }
    v
}

/// Accumulated up to instant `t`: (score, Δblue vision, Δred vision).
pub(crate) fn accumulated_at(
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
pub(crate) fn total_duration(report: &DominionReport, vision_blocks: &[ScrimBlock]) -> f32 {
    let a = report.blocks.last().map(|b| b.end_s).unwrap_or(0.0);
    let b = vision_blocks.last().map(|x| x.end_s).unwrap_or(0.0);
    a.max(b).max(0.0)
}

/// Time bar: walks the game in 30 s steps.
/// Returns `true` if the user moved it.
pub(crate) fn time_slider(ui: &mut egui::Ui, cursor: &mut f32, dur: f32) -> bool {
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

/// Accumulated advantage chart: blue above the tie, red below.
///
/// `serie` are the points `(second, accumulated value)`. The vertical gold line
/// marks the instant the user has selected with the time bar.
pub(crate) fn accumulated_chart(
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
pub(crate) fn score_chart(
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
pub(crate) fn vision_chart(
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
pub(crate) fn severity_legend(
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
