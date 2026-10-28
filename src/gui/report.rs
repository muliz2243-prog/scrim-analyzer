//! Report screen: header, charts, gold windows and totals.
//!
//! Code moved verbatim from `gui_main.rs`; behavior is unchanged.

use super::*;

/// Cards of the 5 best gold windows for each team.
pub(crate) fn gold_top5_section(ui: &mut egui::Ui, report: &DominionReport, es_oro: bool) {
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
pub(crate) fn gold_all_windows_panel(ui: &mut egui::Ui, report: &DominionReport, es_oro: bool) {
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
pub(crate) fn gold_alerts_panel(ui: &mut egui::Ui, report: &DominionReport, es_oro: bool) {
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

pub(crate) fn ui_report(
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
