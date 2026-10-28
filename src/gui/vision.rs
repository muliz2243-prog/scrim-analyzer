//! Vision dashboard and the gold breakdown table per alert.
//!
//! Code moved verbatim from `gui_main.rs`; behavior is unchanged.

use super::*;

/// Vision dashboard in **2-minute** windows: vision points
/// (`wardScore`) for each team and who held the advantage.
///
/// The series comes from the 30 s blocks measured by the live poller. The
/// LCU timeline does not expose `wardScore` per minute in replays; when there
/// is no series it is stated explicitly instead of inventing values.
#[allow(dead_code)]

/// Vision dashboard: 2 min table on the left and best windows for
/// each team on the right (using the side gap).
pub(crate) fn vision_dashboard(ui: &mut egui::Ui, vision_blocks: &[ScrimBlock], dur: f32) {
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
            let (blue, red, con_datos) = vision_windows(vision_blocks, n, W);
            if !con_datos {
                vision_tutorial(ui);
                return;
            }

            let orden = vision_order(&blue, &red);
            let mut orden_rojo = orden.clone();
            orden_rojo.reverse();

            ui.columns(2, |cols| {
                vision_table(&mut cols[0], &blue, &red, dur, W, es_ward_score);
                vision_top_windows(
                    &mut cols[1],
                    &orden,
                    &orden_rojo,
                    &blue,
                    &red,
                    dur,
                    W,
                    es_ward_score,
                );
            });
        });
}

/// Adds the 30 s blocks into 2-minute windows: `(blue, red, hay_datos)`.
fn vision_windows(vision_blocks: &[ScrimBlock], n: usize, w: f32) -> (Vec<f64>, Vec<f64>, bool) {
    let mut blue = vec![0.0f64; n];
    let mut red = vec![0.0f64; n];
    let mut con_datos = false;
    for b in vision_blocks {
        let idx = (b.start_s / w).floor() as usize;
        if idx >= n {
            continue;
        }
        blue[idx] += b.ward_blue_delta;
        red[idx] += b.ward_red_delta;
        if b.ward_blue_delta.abs() + b.ward_red_delta.abs() > 0.0 {
            con_datos = true;
        }
    }
    (blue, red, con_datos)
}

/// How to get the per-minute vision when the capture does not have it yet.
fn vision_tutorial(ui: &mut egui::Ui) {
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
}

/// Vision value with the right number of decimals for the current source.
fn fmt_vision(v: f64, es_ward_score: bool) -> String {
    if es_ward_score {
        format!("{v:.1}")
    } else {
        format!("{v:.0}")
    }
}

/// Window indices sorted from best to worst for blue.
fn vision_order(blue: &[f64], red: &[f64]) -> Vec<usize> {
    let mut orden: Vec<usize> = (0..blue.len())
        .filter(|&i| blue[i].abs() + red[i].abs() > 0.0)
        .collect();
    let diff_at = |i: usize| blue[i] - red[i];
    orden.sort_by(|&a, &b| {
        diff_at(b)
            .partial_cmp(&diff_at(a))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    orden
}

/// 2-minute table of the vision of each team.
fn vision_table(
    col: &mut egui::Ui,
    blue: &[f64],
    red: &[f64],
    dur: f32,
    w: f32,
    es_ward_score: bool,
) {
    egui::Grid::new("vision_dashboard_grid")
        .num_columns(4)
        .spacing([8.0, 4.0])
        .striped(true)
        .show(col, |ui| {
            for h in ["VENTANA", "AZUL", "ROJO", "VENTAJA"] {
                ui.label(
                    RichText::new(i18n::tr(h))
                        .size(11.0)
                        .strong()
                        .color(GOLD_BRIGHT),
                );
            }
            ui.end_row();
            for i in 0..blue.len() {
                let (b, r) = (blue[i], red[i]);
                let start = i as f32 * w;
                let end = ((i + 1) as f32 * w).min(dur.max(w));
                ui.label(
                    RichText::new(format!("[{} – {}]", mmss(start), mmss(end)))
                        .size(11.0)
                        .color(TEXT),
                );
                ui.label(
                    RichText::new(fmt_vision(b, es_ward_score))
                        .size(11.5)
                        .strong()
                        .color(BLUE),
                );
                ui.label(
                    RichText::new(fmt_vision(r, es_ward_score))
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
    col.label(
        RichText::new(i18n::tr(
            "Diferencia = visión AZUL − ROJO. «—» = sin actividad; «0» = empate.",
        ))
        .size(10.5)
        .color(MUTED),
    );
}

/// Best vision windows of each team.
fn vision_top_windows(
    col: &mut egui::Ui,
    orden: &[usize],
    orden_rojo: &[usize],
    blue: &[f64],
    red: &[f64],
    dur: f32,
    w: f32,
    es_ward_score: bool,
) {
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
    for (titulo, indices, color) in [("AZUL", orden, BLUE), ("ROJO", orden_rojo, RED)] {
        col.label(RichText::new(i18n::tr(titulo)).size(15.0).strong().color(color));
        if indices.is_empty() {
            col.label(RichText::new(i18n::tr("—")).size(13.5).color(MUTED));
        }
        for &i in indices.iter().take(top) {
            let start = i as f32 * w;
            let end = ((i + 1) as f32 * w).min(dur.max(w));
            let d = if titulo == "AZUL" {
                blue[i] - red[i]
            } else {
                red[i] - blue[i]
            };
            col.label(
                RichText::new(i18n::trf(
                    "[{0} – {1}]  +{2}",
                    &[&mmss(start), &mmss(end), &fmt_vision(d.abs(), es_ward_score)],
                ))
                .size(14.5)
                .color(color),
            );
        }
        col.add_space(8.0);
    }
}

/// Aggregated rows of the breakdown table: `(blue, red, available)` per category.
type BreakdownMap = std::collections::BTreeMap<&'static str, (f64, f64, bool)>;

/// BLUE vs RED comparison table of what generated the gold in the window.
pub(crate) fn alert_breakdown_table(ui: &mut egui::Ui, block: &DominionBlock, es_oro: bool, show_diff: bool) {
    let extra_label = if es_oro {
        "Oro extra (wards, placas no detectadas, pasivo…)"
    } else {
        "Oro extra (no detectado)"
    };
    let mut map = breakdown_rows(block, extra_label);
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
    breakdown_grid(ui, block.index, &filas, &map, unidad, es_oro, show_diff);
    breakdown_footer(ui, &map, unidad, show_diff);
}

/// Adds up the axes of the block into the table categories.
fn breakdown_rows(block: &DominionBlock, extra_label: &'static str) -> BreakdownMap {
    use std::collections::BTreeMap;
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
    map
}

/// The table itself, with the window total as the last row.
fn breakdown_grid(
    ui: &mut egui::Ui,
    block_index: u32,
    filas: &[&str],
    map: &BreakdownMap,
    unidad: &str,
    es_oro: bool,
    show_diff: bool,
) {
    let fmt = |v: f64| -> String {
        if es_oro {
            format!("{v:+.0}g")
        } else {
            format!("{v:+.1} pts")
        }
    };

    egui::Grid::new(format!("breakdown_{}_{}", block_index, show_diff))
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
            for k in filas {
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
}

/// Total of both teams and the note that explains what "extra gold" is.
fn breakdown_footer(ui: &mut egui::Ui, map: &BreakdownMap, unidad: &str, show_diff: bool) {
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
