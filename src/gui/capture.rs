//! Background threads: live poller, replay, vision and LCU enrichment.
//!
//! Code moved verbatim from `gui_main.rs`; behavior is unchanged.

use super::*;

/// Applies the LCU timeline to a capture: real gold, axes, Top 5, vision,
/// objectives and `gameId`.
pub(crate) fn apply_lcu_enrichment(
    cap: &mut ScrimCapture,
    game_id: u64,
    timeline: &lcu::LcuTimeline,
    summary: Option<lcu::MatchSummary>,
    set_duration: bool,
) {
    let gold_series = timeline.gold_series();
    let ward_evs = timeline.ward_events_with_team();
    log_debug(&format!(
        "enrich: game_id={game_id}, gold_series={} frames, ward_events={}",
        gold_series.len(),
        ward_evs.len()
    ));
    let dur = timeline.duration_s().max(cap.duration_s);
    let gold_report = compute_gold_blocks(&gold_series, dur, "lcu_timeline");
    let thresholds = SeverityThresholds::default();

    // 1. Real net gold + breakdown by axes.
    cap.dominion.blocks = gold_blocks_with_axes(timeline, &gold_report.blocks, &thresholds);
    cap.dominion.severity_thresholds = thresholds;

    // 2. Breakpoints and Top 5 recomputed on real gold.
    cap.dominion.breakpoints = score::compute_breakpoints(&cap.dominion.blocks);
    let (top5_a, top5_r) = score::score_top5(&cap.dominion.blocks);
    cap.dominion.top5_azul = top5_a;
    cap.dominion.top5_rojo = top5_r;

    // 3. Vision per window from WARD_PLACED (if the timeline provides it).
    let n_blocks = (dur.max(1.0) / 30.0).ceil() as u32;
    let mut vision_blocks: Vec<ScrimBlock> = (0..n_blocks)
        .map(|i| ScrimBlock {
            index: i,
            start_s: i as f32 * 30.0,
            end_s: (((i + 1) as f32) * 30.0).min(dur.max(1.0)),
            vision_source: "wards_placed_lcu".to_string(),
            ..Default::default()
        })
        .collect();
    for (t, _wtype, _x, _y, creator_id, team_id) in &ward_evs {
        let idx_f = (*t / 30.0).floor();
        if idx_f < 0.0 {
            continue;
        }
        let idx = idx_f as usize;
        if idx >= vision_blocks.len() {
            continue;
        }
        match lcu::team_from_participant(*creator_id).or(*team_id) {
            Some(100) => vision_blocks[idx].ward_blue_delta += 1.0,
            Some(200) => vision_blocks[idx].ward_red_delta += 1.0,
            _ => {}
        }
    }
    let total_wards_atribuidos: f64 = vision_blocks
        .iter()
        .map(|b| b.ward_blue_delta + b.ward_red_delta)
        .sum();
    // Metric priority: if the poller already measured `wardScore` (vision
    // points), that series is kept; the LCU ward counts are not the
    // same metric and must not replace it.
    let poller_tiene_score = cap.blocks.iter().any(|b| {
        b.vision_source == "wardScore"
            && (b.ward_blue_delta.abs() + b.ward_red_delta.abs()) > 0.0
    });
    let poller_wards: f64 = cap
        .blocks
        .iter()
        .map(|b| b.ward_blue_delta + b.ward_red_delta)
        .sum();
    if poller_tiene_score {
        log_debug("enrich: keeping poller wardScore series");
    } else if total_wards_atribuidos > 0.0 && total_wards_atribuidos >= poller_wards {
        cap.blocks = vision_blocks;
    } else {
        // Riot's timeline does not always include WARD_PLACED: keep the
        // wardScore/wards that the live poller did measure.
        log_debug("enrich: timeline has no useful wards; keeping poller vision");
        cap.omisiones.push(i18n::tr(
            "El timeline del LCU no incluye eventos WARD_PLACED en esta partida; la visión por ventana usa el wardScore/wards del Live Client Data (si lo hubo).",
        ));
    }
    // Measured totals for the whole game from /games/{id}: fallback when there
    // is no temporal breakdown (e.g. a replay opened directly from .rofl).
    if let Some(summary) = summary {
        if summary.vision.total() > 0 {
            cap.vision_totals = Some(VisionTotals {
                blue: summary.vision.blue,
                red: summary.vision.red,
            });
            cap.omisiones.push(i18n::trf(
                "Visión total medida por el cliente: {0} wards azules · {1} rojas (sin desglose por minuto).",
                &[&summary.vision.blue.to_string(), &summary.vision.red.to_string()],
            ));
        }
        if !summary.game_mode.is_empty() && summary.game_mode != "?" {
            cap.game_mode = summary.game_mode;
        }
        if !summary.map_name.is_empty() && summary.map_name != "?" {
            cap.map_name = summary.map_name;
        }
    }

    // 4. Exact epic objectives → chart alerts.
    let objs = timeline.objective_events();
    cap.dominion.avisos.clear();
    for (t_s, m_type, sub, _kid, team_opt) in objs {
        let team = match team_opt {
            Some(100) => Team::Blue,
            Some(200) => Team::Red,
            _ => continue,
        };
        let key = format!("{} {}", m_type, sub.as_deref().unwrap_or("")).to_lowercase();
        let tipo = if key.contains("baron") {
            "baron"
        } else if key.contains("herald") {
            "herald"
        } else if key.contains("horde") || key.contains("voidgrub") {
            "voidgrub"
        } else if key.contains("dragon") || key.contains("drake") {
            "dragon"
        } else {
            "objetivo"
        };
        cap.dominion.avisos.push(ObjectiveEvent {
            team,
            start_s: t_s,
            end_s: t_s + 120.0,
            dur_s: 120.0,
            puntos: 0.0,
            tipo: tipo.to_string(),
        });
    }

    if set_duration {
        cap.duration_s = dur;
    }
    cap.game_id = Some(game_id);
    cap.dominion.fuente = "lcu_timeline_oro_real".to_string();
    cap.dominion.vision = score::compute_vision(&cap.blocks, VisionThresholds::default());
    cap.omisiones
        .push(i18n::trf("Partida enlazada por r3dlog: gameId {0}", &[&game_id.to_string()]));
    log_debug(&format!(
        "enrich: breakpoints={}, vision_blocks={}, vision_alerts={}, avisos={}",
        cap.dominion.breakpoints.len(),
        cap.blocks.len(),
        cap.dominion.vision.alerts.len(),
        cap.dominion.avisos.len()
    ));
}

/// `gameId` candidates for enrichment when a capture ends.
pub(crate) fn game_id_candidates(
    announced: Option<u64>,
    session_start: SystemTime,
    include_any_age: bool,
) -> Vec<u64> {
    let mut v = Vec::new();
    let mut push = |id: u64| {
        if !v.contains(&id) {
            v.push(id);
        }
    };
    if let Some(id) = lcu::find_active_game_id() {
        push(id);
    }
    if let Some(id) = announced {
        push(id);
    }
    if let Some(id) = lcu::find_latest_rofl_game_id_since(session_start) {
        push(id);
    }
    // Last resort: the most recent log even if old (a long game whose
    // r3dlog was only written at start). Left for the last attempts.
    if include_any_age {
        if let Some(id) = lcu::find_latest_game_id_any_age() {
            push(id);
        }
    }
    v
}

pub(crate) fn spawn_live(tx: Sender<Msg>, stop: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        log_debug("spawn_live: started");
        let session_start = SystemTime::now();
        let mut poller = LivePoller::with_config(
            LIVE_ENDPOINT,
            2,
            Duration::from_millis(1000),
            GoldModelParams::default(),
        );
        // `gameId` already announced to the interface: if it changes, the previous
        // game is released (no getting stuck on the previous report).
        let mut announced_game_id: Option<u64> = None;
        // Replay timeline: it is available from the first minute and is
        // applied to the dashboard while playback runs.
        let mut replay_cache: Option<(u64, lcu::LcuTimeline, Option<lcu::MatchSummary>)> = None;
        let mut lcu_mid: Option<LcuClient> = None;
        let mut ticks: u64 = 0;
        loop {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            match poller.poll_once() {
                Ok(_) => {
                    let mut cap = poller.build_capture();
                    if let Some((id, timeline, summary)) = &replay_cache {
                        apply_lcu_enrichment(
                            &mut cap,
                            *id,
                            timeline,
                            summary.clone(),
                            false,
                        );
                    }
                    let _ = tx.send(Msg::LiveTick(Box::new(cap)));
                }
                Err(_) => {
                    // A newly opened replay (or a game still loading) makes the
                    // Live Client Data endpoint disappear for a few seconds
                    // while the client restarts. Do not mistake that hiccup for
                    // the end of the match: keep retrying while the LCU still
                    // reports a session, and only give up after a long outage.
                    let session = lcu::find_active_session();
                    let threshold = match session {
                        Some(s) if s.is_replay => 60,
                        Some(_) => 8,
                        None => 5,
                    };
                    if poller.looks_finished(threshold) {
                        break;
                    }
                    log_debug(&format!(
                        "spawn_live: poll failed (consecutive={}, threshold={threshold}); keeping capture",
                        poller.consecutive_failures()
                    ));
                    let _ = tx.send(Msg::LiveWaiting);
                }
            }
            ticks += 1;
            // The link is decided by the client's `r3dlog`, not by the most
            // recent .rofl: an old downloaded replay no longer hijacks the app.
            if ticks % 5 == 1 {
                if let Some(session) = lcu::find_active_session() {
                    let id = session.game_id;
                    if announced_game_id != Some(id) {
                        announced_game_id = Some(id);
                        log_debug(&format!("spawn_live: new game_id detected = {id}"));
                        let _ = tx.send(Msg::LiveGameId(id));
                    }
                    // Replay: the official timeline already exists. It is loaded once and
                    // applied on every tick so the dashboard shows gold,
                    // objectives and Top 5 without waiting for the end.
                    if session.is_replay
                        && replay_cache.as_ref().map(|(i, ..)| *i) != Some(id)
                    {
                        if lcu_mid.is_none() {
                            lcu_mid = LcuClient::connect().ok();
                        }
                        if let Some(client) = &lcu_mid {
                            match client.get_timeline(id) {
                                Ok(timeline) => {
                                    let summary = client.get_match_summary(id).ok();
                                    log_debug(&format!(
                                        "spawn_live: replay timeline {id} loaded live"
                                    ));
                                    replay_cache = Some((id, timeline, summary));
                                    let _ = tx.send(Msg::LiveGameId(id));
                                    // Immediate preview: replay gold/objectives
                                    // right away, without waiting for the end.
                                    let mut preview = poller.build_capture();
                                    if let Some((pid, ptl, psum)) = &replay_cache {
                                        apply_lcu_enrichment(
                                            &mut preview,
                                            *pid,
                                            ptl,
                                            psum.clone(),
                                            true,
                                        );
                                    }
                                    let _ = tx.send(Msg::LivePreview(Box::new(preview)));
                                }
                                Err(e) => {
                                    log_debug(&format!(
                                        "spawn_live: replay {id} has no timeline yet: {e}"
                                    ));
                                }
                            }
                        }
                    }
                }
            }
            // End of game: the API stops responding for several polls.
            if poller.looks_finished(3) {
                break;
            }
            std::thread::sleep(poller.poll_interval());
        }
        let mut cap = poller.build_capture();
        log_debug(&format!(
            "spawn_live: poll_count={}, duration_s={}, players={}",
            cap.poll_count,
            cap.duration_s,
            cap.players.len()
        ));

        // Wait for the client to finish writing the `.rofl`/`r3dlog` and for
        // the LCU to publish the timeline. In real games the timeline takes
        // a few seconds and the `r3dlog` may be locked while the game
        // is still open: that is why it is retried for ~40 s.
        std::thread::sleep(Duration::from_millis(1500));

        let mut enriquecida = false;
        if let Some((id, timeline, summary)) = &replay_cache {
            apply_lcu_enrichment(&mut cap, *id, timeline, summary.clone(), true);
            enriquecida = true;
        }

        if !enriquecida {
        match LcuClient::connect() {
            Err(e) => {
                log_debug(&format!("spawn_live: LCU unavailable: {e}"));
                cap.omisiones.push(i18n::trf(
                    "No se pudo conectar al LCU ({0}); el informe se queda con el score del modelo en vivo.",
                    &[&e.to_string()],
                ));
            }
            Ok(lcu_client) => {
                log_debug("spawn_live: LCU connected");
                let mut vistos: Vec<u64> = Vec::new();
                for attempt in 0..20 {
                    let candidates =
                        game_id_candidates(announced_game_id, session_start, attempt >= 10);
                    if candidates != vistos {
                        log_debug(&format!(
                            "spawn_live: attempt {} candidates = {:?}",
                            attempt + 1,
                            candidates
                        ));
                        vistos = candidates.clone();
                    }
                    for id in &candidates {
                        match lcu_client.get_timeline(*id) {
                            Ok(timeline) => {
                                let summary = match lcu_client.get_match_summary(*id) {
                                    Ok(s) => Some(s),
                                    Err(e) => {
                                        log_debug(&format!(
                                            "spawn_live: match summary {id} unavailable: {e}"
                                        ));
                                        None
                                    }
                                };
                                apply_lcu_enrichment(&mut cap, *id, &timeline, summary, true);
                                enriquecida = true;
                                break;
                            }
                            Err(e) => {
                                if attempt == 0 || attempt % 5 == 0 {
                                    log_debug(&format!(
                                        "spawn_live: get_timeline({id}) ERROR: {e}"
                                    ));
                                }
                            }
                        }
                    }
                    if enriquecida {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(2000));
                }
                if !enriquecida {
                    log_debug("spawn_live: no gameId produced a timeline after 40 s");
                    cap.omisiones.push(i18n::tr(
                        "Ningún gameId de esta sesión tiene timeline en el LCU; el informe se queda con el score del modelo en vivo.",
                    ));
                }
            }
        }
        }

        log_debug(&format!("spawn_live: final breakpoints={}, vision_alerts={}", cap.dominion.breakpoints.len(), cap.dominion.vision.alerts.len()));
        let _ = tx.send(Msg::LiveDone(Ok(Box::new(cap))));
    });
}

pub(crate) fn spawn_replay(tx: Sender<Msg>, path: String) {
    std::thread::spawn(move || {
        let result = analyze_file(&path, AnalyzeOptions::default())
            .map(Box::new)
            .map_err(|e| e.to_string());
        let _ = tx.send(Msg::ReplayDone {
            path,
            result,
            warning: None,
        });
    });
}

/// **Silent** vision capture for an open replay: polls the Live
/// Client Data API while the replay plays and, when it closes, sends the
/// 30 s blocks to merge them with the LCU report. It does not change
/// screen or require pressing "Stop".
pub(crate) fn spawn_vision_capture(tx: Sender<Msg>, stop: Arc<AtomicBool>, game_id: u64) {
    std::thread::spawn(move || {
        log_debug(&format!(
            "spawn_vision_capture: waiting for Live API for gameId {game_id}"
        ));
        let mut poller = LivePoller::with_config(
            LIVE_ENDPOINT,
            2,
            Duration::from_millis(1000),
            GoldModelParams::default(),
        );
        // Wait up to 120 s for the client to open **this** replay (not another
        // game in progress) and start serving the Live API.
        let mut started = false;
        for _ in 0..120 {
            if stop.load(Ordering::Relaxed) {
                return;
            }
            if lcu::find_active_game_id() == Some(game_id) && poller.poll_once().is_ok() {
                started = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(1000));
        }
        if !started {
            log_debug("spawn_vision_capture: client did not open this replay; no live vision");
            return;
        }
        log_debug("spawn_vision_capture: capturing live vision");
        // Safety cap of 4 h in case the client never stops responding.
        for tick in 0..14_400 {
            if stop.load(Ordering::Relaxed) {
                return;
            }
            // If the client switches game/replay, do not mix visions.
            if tick % 5 == 0 {
                if let Some(active) = lcu::find_active_game_id() {
                    if active != game_id {
                        log_debug("spawn_vision_capture: active gameId changed; discarding");
                        return;
                    }
                }
            }
            match poller.poll_once() {
                Ok(_) => {}
                Err(_) => {
                    if poller.looks_finished(5) {
                        break;
                    }
                }
            }
            std::thread::sleep(poller.poll_interval());
        }
        let cap = poller.build_capture();
        let blocks = cap.blocks;
        let con_vision = blocks
            .iter()
            .any(|b| (b.ward_blue_delta + b.ward_red_delta).abs() > 0.0);
        if con_vision {
            log_debug(&format!(
                "spawn_vision_capture: {} blocks with vision; sending to merge",
                blocks.len()
            ));
            let _ = tx.send(Msg::LiveVisionReady { game_id, blocks });
        } else {
            log_debug("spawn_vision_capture: capture had no vision");
        }
    });
}

/// Empty capture (no poller) to dump an LCU timeline from a `.rofl`.
pub(crate) fn empty_capture_for_timeline(
    game_id: u64,
    timeline: &lcu::LcuTimeline,
    summary: Option<&lcu::MatchSummary>,
) -> ScrimCapture {
    ScrimCapture {
        fuente: "lcu_timeline_oro_real".to_string(),
        endpoint: format!("lcu:{game_id}"),
        game_id: Some(game_id),
        vision_totals: None,
        reconstruido_por_modelo: false,
        game_mode: summary
            .map(|s| s.game_mode.clone())
            .filter(|s| !s.is_empty() && s != "?")
            .unwrap_or_else(|| "REPLAY".to_string()),
        map_name: summary
            .map(|s| s.map_name.clone())
            .filter(|s| !s.is_empty() && s != "?")
            .unwrap_or_else(|| "?".to_string()),
        duration_s: timeline.duration_s(),
        poll_count: 0,
        poll_interval_ms: 0,
        parametros: GoldModelParams::default(),
        omisiones: Vec::new(),
        players: Vec::new(),
        blocks: Vec::new(),
        events: Vec::new(),
        top5_azul: Default::default(),
        top5_rojo: Default::default(),
        dominion: score::compute_report(
            &[],
            ScoreParams::default(),
            VisionThresholds::default(),
            "lcu_timeline_oro_real",
        ),
    }
}

/// Opens a `.rofl`: first it tries the LCU timeline (complete and fast
/// gold/vision/objectives) using the `gameId` from the file name. If the
/// client is not available, it falls back to the basic forensic analysis of the `.rofl`.
pub(crate) fn spawn_rofl_enriched(tx: Sender<Msg>, path: String, game_id: u64) {
    std::thread::spawn(move || {
        log_debug(&format!(
            "spawn_rofl_enriched: path={path}, game_id={game_id}"
        ));
        let warning = match LcuClient::connect() {
            Ok(client) => match client.get_timeline(game_id) {
                Ok(timeline) => {
                    let summary = match client.get_match_summary(game_id) {
                        Ok(s) => Some(s),
                        Err(e) => {
                            log_debug(&format!("spawn_rofl_enriched: summary unavailable: {e}"));
                            None
                        }
                    };
                    let mut cap = empty_capture_for_timeline(game_id, &timeline, summary.as_ref());
                    apply_lcu_enrichment(&mut cap, game_id, &timeline, summary, true);
                    let notes = vec![
                        i18n::trf("Replay abierto desde {0}", &[&path]),
                        i18n::trf(
                            "Enriquecido con el timeline del LCU (gameId {0}).",
                            &[&game_id.to_string()],
                        ),
                    ];
                    let rv = report_view(
                        Box::new(cap),
                        notes,
                        "Informe del replay (oro + visión)",
                        "Replay · LCU timeline",
                    );
                    let _ = tx.send(Msg::RoflReport {
                        report: Box::new(rv),
                    });
                    return;
                }
                Err(e) => {
                    log_debug(&format!(
                        "spawn_rofl_enriched: get_timeline({game_id}) ERROR: {e}"
                    ));
                    Some(i18n::trf(
                        "No se pudo extraer el timeline del LCU para el gameId {0}: {1}. Se muestra el análisis básico del .rofl (sin oro/visión). Abre League of Legends y vuelve a abrir el replay para el informe completo.",
                        &[&game_id.to_string(), &e.to_string()],
                    ))
                }
            },
            Err(e) => {
                log_debug(&format!("spawn_rofl_enriched: LCU unavailable: {e}"));
                Some(i18n::trf(
                    "El cliente de League no está disponible ({0}). Se muestra el análisis básico del .rofl; para oro/visión, abre League y vuelve a abrir el replay.",
                    &[&e.to_string()],
                ))
            }
        };
        let result = analyze_file(&path, AnalyzeOptions::default())
            .map(Box::new)
            .map_err(|e| e.to_string());
        let _ = tx.send(Msg::ReplayDone {
            path,
            result,
            warning,
        });
    });
}
