//! Capture files: save, load, rebuild the report and analyst annotations.
//!
//! Code moved verbatim from `gui_main.rs`; behavior is unchanged.

use super::*;

pub(crate) fn spawn_capture_load(tx: Sender<Msg>, path: String) {
    std::thread::spawn(move || {
        let result = load_capture(&path).map(Box::new);
        let _ = tx.send(Msg::CaptureDone { path, result });
    });
}

/// File name stem safe for Windows (and the other systems).
pub(crate) fn sanitize_file_stem(name: &str) -> String {
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
pub(crate) fn unique_capture_path(dir: &Path, stem: &str) -> PathBuf {
    let mut path = dir.join(format!("{stem}.json"));
    let mut n = 1u32;
    while path.exists() {
        path = dir.join(format!("{stem} ({n}).json"));
        n += 1;
    }
    path
}

pub(crate) fn save_capture(cap: &ScrimCapture, dir: &Path, name: &str) -> Result<String, String> {
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
pub(crate) fn load_capture(path: &str) -> Result<LoadedCapture, String> {
    let (raw, v) = read_capture_json(path)?;
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
    let (feed, marcador) = feed_vs_scoreboard(&deduped, &players);
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

    let report = saved_or_computed_report(&v, &blocks, &mut notes);
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

/// Reads the JSON of a capture from disk.
fn read_capture_json(path: &str) -> Result<(String, serde_json::Value), String> {
    let raw =
        std::fs::read_to_string(path).map_err(|e| format!("could not read {path}: {e}"))?;
    let v: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|e| format!("invalid JSON in {path}: {e}"))?;
    Ok((raw, v))
}

/// Kills in the event feed and in the scoreboard, as `(blue, red)` each.
fn feed_vs_scoreboard(
    events: &[AttributedEvent],
    players: &[PlayerSample],
) -> ((u32, u32), (u32, u32)) {
    let mut feed = (0u32, 0u32);
    for e in events {
        if e.kind == "ChampionKill" {
            match e.team {
                Some(Team::Blue) => feed.0 += 1,
                Some(Team::Red) => feed.1 += 1,
                None => {}
            }
        }
    }
    let mut marcador = (0u32, 0u32);
    for p in players {
        match p.team {
            Team::Blue => marcador.0 += p.kills,
            Team::Red => marcador.1 += p.kills,
        }
    }
    (feed, marcador)
}

/// If the capture already came enriched with the LCU, its real-gold report is
/// kept (recomputing with the points model would destroy the axes and the
/// Top 5). Old captures are recomputed as usual.
fn saved_or_computed_report(
    v: &serde_json::Value,
    blocks: &[ScrimBlock],
    notes: &mut Vec<String>,
) -> DominionReport {
    let saved_dominion: Option<DominionReport> = v
        .get("dominion")
        .cloned()
        .and_then(|d| serde_json::from_value(d).ok());
    match saved_dominion {
        Some(r) if r.fuente.contains("lcu") => {
            notes.push(i18n::tr(
                "Informe de oro real recuperado del JSON (sin recalcular).",
            ));
            r
        }
        _ => score::compute_report(
            blocks,
            ScoreParams::default(),
            VisionThresholds::default(),
            "captura_guardada",
        ),
    }
}

pub(crate) fn kill_density(replay: &Replay) -> Vec<(u32, u32, u32)> {
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

/// Measured cumulative for the live game (last snapshot + events).
///
/// Vision uses `scores.wardScore` (measured vision score); if the
/// client does not expose it (all 0, e.g. in spectator mode), the
/// measured count of wards placed from `/stats` is shown, with the "w" suffix.
pub(crate) fn live_chips(cap: &ScrimCapture) -> LiveChips {
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
pub(crate) fn merge_vision_into_report(r: &mut ReportView, blocks: Vec<ScrimBlock>) {
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

pub(crate) fn report_from_capture(cap: Box<ScrimCapture>, notes: Vec<String>) -> ReportView {
    report_view(
        cap,
        notes,
        "Informe de la partida (oro + visión)",
        "En vivo",
    )
}

/// Builds the report view from a capture (live or replay).
pub(crate) fn report_view(
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

pub(crate) fn report_from_loaded(path: &str, lc: LoadedCapture) -> ReportView {
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

/// Saves the analyst annotations to their own file (`.md` / `.txt`).
pub(crate) fn guardar_anotaciones(notas: &mut Anotaciones) {
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
pub(crate) fn cargar_anotaciones(notas: &mut Anotaciones) {
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
pub(crate) fn save_json_dialog(json: &str, dir: &Path, default_name: &str) -> Option<(bool, String)> {
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
