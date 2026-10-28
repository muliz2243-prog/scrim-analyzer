//! Live dashboard: chips, timeline, alerts and advantage cards.
//!
//! Code moved verbatim from `gui_main.rs`; behavior is unchanged.

use super::*;

impl App {
    pub(crate) fn ui_live_dash(&mut self, ui: &mut egui::Ui) {
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

pub(crate) fn ui_loading(ui: &mut egui::Ui, label: &str) {
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

pub(crate) fn chip(ui: &mut egui::Ui, label: &str, blue: String, red: String) {
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

pub(crate) fn stats_row(ui: &mut egui::Ui, c: &LiveChips) {
    ui.horizontal_wrapped(|ui| {
        chip(ui, "KILLS", c.kills.0.to_string(), c.kills.1.to_string());
        chip(ui, "CS", c.cs.0.to_string(), c.cs.1.to_string());
        chip(ui, "TORRES", c.towers.0.to_string(), c.towers.1.to_string());
        chip(ui, "OBJETIVOS", c.objectives.0.to_string(), c.objectives.1.to_string());
        chip(ui, "VISIÓN", c.vision.0.clone(), c.vision.1.clone());
    });
}

/// 30 s strip: score bar per window + vision alerts band.
pub(crate) fn timeline_strip(ui: &mut egui::Ui, report: &DominionReport, height: f32) {
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
            Some(sev) => timeline_strip_alert_bar(painter.clone(), x, w, base_y, bar_h, sev, b.score_net_blue),
            None => timeline_strip_score_bar(painter.clone(), x, w, base_y, bar_h, b.score_net_blue, crit),
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
pub(crate) fn objective_name(tipo: &str) -> String {
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
pub(crate) fn objective_verb(tipo: &str) -> String {
    i18n::tr(match tipo {
        "inhib" => "derribó",
        _ => "mató",
    })
}

fn timeline_strip_alert_bar(painter: egui::Painter, x: f32, w: f32, base_y: f32, bar_h: f32, sev: Severity, score_net_blue: f64) {
    let team = if score_net_blue >= 0.0 { BLUE } else { TEAM_RED_DARK };
    let half = (w * 0.5).max(1.0);
    painter.rect_filled(
        Rect::from_min_size(Pos2::new(x + 1.0, base_y - bar_h), Vec2::new(half, bar_h)),
        egui::CornerRadius { nw: 2, sw: 2, ne: 0, se: 0 },
        severity_color(sev),
    );
    painter.rect_filled(
        Rect::from_min_size(Pos2::new(x + 1.0 + half, base_y - bar_h), Vec2::new((w - half).max(1.0), bar_h)),
        egui::CornerRadius { nw: 0, sw: 0, ne: 2, se: 2 },
        team,
    );
}

fn timeline_strip_score_bar(painter: egui::Painter, x: f32, w: f32, base_y: f32, bar_h: f32, score_net_blue: f64, crit: f64) {
    let mag = ((score_net_blue.abs() / (crit as f64)).min(1.0)) as f32;
    let h = (bar_h * mag).max(3.0);
    let c = if score_net_blue >= 0.0 { BLUE } else { RED };
    painter.rect_filled(
        Rect::from_min_size(Pos2::new(x + 1.0, base_y - h), Vec2::new(w, h)),
        egui::CornerRadius::same(2),
        c.linear_multiply(0.42),
    );
}

/// A line of the chronological log: a score alert or an epic objective
/// notice (which **does not** add or subtract score, but is a key data point).
pub(crate) enum Linea {
    Alerta(BreakAlert),
    Objetivo(ObjectiveEvent),
}

/// Chronological log: score alerts + epic objectives interleaved by
/// their instant (a baron falls at its minute, between the previous and the
/// following alert).
pub(crate) fn lineas_cronologicas(report: &DominionReport) -> Vec<Linea> {
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
pub(crate) fn sorted_breakpoints(report: &DominionReport) -> Vec<BreakAlert> {
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
pub(crate) fn sorted_vision_alerts(report: &DominionReport) -> Vec<VisionAlert> {
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
pub(crate) fn live_alerts_preview(ui: &mut egui::Ui, report: &DominionReport) {
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

pub(crate) fn top5_column(
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

pub(crate) fn team_name(t: Team) -> String {
    i18n::tr(match t {
        Team::Blue => "AZUL",
        Team::Red => "ROJO",
    })
}

pub(crate) fn severity_label(s: Severity) -> String {
    i18n::tr(s.label())
}

/// Advantage card (score or vision) with a border in the team color.
pub(crate) fn adv_card(ui: &mut egui::Ui, title: &str, headline: &str, detail: &str, color: Color32) {
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

/// Accumulated advantage **up to the selected instant**: score and vision separately.
///
/// `vision_totals` is the fallback when there is no temporal breakdown: totals
/// measured for the whole game (`wards placed` or `wardScore`). That way the card
/// does not say "no data" when the data exists, even if it cannot be placed in
/// time.
pub(crate) fn advantage_cards(
    ui: &mut egui::Ui,
    report: &DominionReport,
    vision_blocks: &[ScrimBlock],
    cursor_s: f32,
    vision_totals: Option<(f64, f64, String)>,
    vision_note: Option<&str>,
) {
    let (score, vb, vr) = accumulated_at(report, vision_blocks, cursor_s);
    let when = mmss(cursor_s);
    let is_lcu = report.fuente.contains("lcu");
    let gold = gold_advantage_card(score, &when, is_lcu);
    let vision = vision_advantage_card(vb, vr, &when, vision_totals);

    ui.columns(2, |cols| {
        adv_card(
            &mut cols[0],
            &i18n::trf("{0} HASTA {1}", &[&gold.3, &when]),
            &gold.0,
            &gold.1,
            gold.2,
        );

        let titulo_card = if vision.3 == i18n::tr("VISIÓN TOTAL (SIN DESGLOSE TEMPORAL)") {
            vision.3.to_string()
        } else {
            i18n::trf("{0} {1}", &[&vision.3, &when])
        };
        adv_card(&mut cols[1], &titulo_card, &vision.0, &vision.1, vision.2);
        if let Some(note) = vision_note {
            cols[1].label(RichText::new(note).size(12.0).color(MUTED));
        }
    });
}

/// Headline, detail, color and title of the gold card.
fn gold_advantage_card(score: f64, when: &str, is_lcu: bool) -> (String, String, Color32, String) {
    if score > 0.05 {
        (
            i18n::tr("Ventaja a favor del AZUL"),
            if is_lcu {
                i18n::trf("{0} de oro hasta {1}", &[&format!("{score:+.0}g"), &when])
            } else {
                i18n::trf("{0} puntos hasta {1}", &[&format!("{score:+.1}"), &when])
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
                i18n::trf("{0} de oro hasta {1}", &[&format!("{score:+.0}g"), &when])
            } else {
                i18n::trf("{0} puntos hasta {1}", &[&format!("{score:+.1}"), &when])
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
    }
}

/// Headline, detail, color and title of the vision card.
fn vision_advantage_card(
    vb: f64,
    vr: f64,
    when: &str,
    vision_totals: Option<(f64, f64, String)>,
) -> (String, String, Color32, String) {
    if vb + vr > 0.0 {
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
    }
}

impl Linea {
    fn instante(&self) -> f32 {
        match self {
            Linea::Alerta(a) => a.start_s,
            Linea::Objetivo(o) => o.start_s,
        }
    }
}
