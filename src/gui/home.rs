//! Home screen.
//!
//! Code moved verbatim from `gui_main.rs`; behavior is unchanged.

use super::*;

impl App {
    /// Minimal home screen: only the name and the live capture button.
    pub(crate) fn ui_home(&mut self, ui: &mut egui::Ui) {
        let h = ui.available_height();
        ui.add_space((h * 0.20).max(30.0));
        ui.vertical_centered(|ui| {
            home_title(ui);
            self.home_status_banner(ui);
            self.home_buttons(ui);
            self.home_auto_save(ui);
        });
    }

    /// Status banner of the last action, if any.
    fn home_status_banner(&mut self, ui: &mut egui::Ui) {
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
    }

    /// Live capture and "open capture" buttons.
    fn home_buttons(&mut self, ui: &mut egui::Ui) {
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
    }

    /// Option: automatic saving of the .json log.
    fn home_auto_save(&mut self, ui: &mut egui::Ui) {
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
    }
}

/// Title with the gold halo, the gold rule and the subtitle.
fn home_title(ui: &mut egui::Ui) {
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
}
