//! Replay screen: players, kill density and team totals.
//!
//! Code moved verbatim from `gui_main.rs`; behavior is unchanged.

use super::*;

pub(crate) fn replay_players_table(ui: &mut egui::Ui, players: &[Player]) {
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

pub(crate) fn kill_density_strip(ui: &mut egui::Ui, density: &[(u32, u32, u32)]) {
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

pub(crate) fn ui_replay(ui: &mut egui::Ui, r: &mut ReplayView) {
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

pub(crate) fn team_totals_table(ui: &mut egui::Ui, tt: &TeamTotals) {
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
