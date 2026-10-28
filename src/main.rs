//! CLI: `rofl-analyzer <file.rofl> [options]`
//!
//! Usage:
//! ```text
//! rofl-analyzer match.rofl
//! rofl-analyzer match.rofl --json output.json
//! rofl-analyzer match.rofl --sample-ms 500 --dump-payload payload.bin
//! ```

use std::process::ExitCode;

use rofl_analyzer::{
    analyze_file, compute_gold_blocks, dedup_saved_events, extract_payload_file, i18n, live,
    rebuild_event_counters, to_json, AnalyzeOptions, AttributedEvent, DominionReport, EventKind,
    LcuClient, LivePoller, Replay, ScrimBlock, Team, SAVED_EVENT_EPSILON_S, Lang,
};

fn help_text() -> String {
    let mut text = format!(
        "{}\n\n{}\n  rofl-analyzer <archivo.rofl> [opciones]\n  rofl-analyzer --scrim [opciones]          # {}\n  rofl-analyzer --recalc <captura.json>     # {}\n  rofl-analyzer --lcu [gameId]              # {}\n\n{}\n  -j, --json <ruta>         {}\n  -s, --sample-ms <n>       {}\n      --dump-payload <ruta> {}\n      --scrim               {}\n      --interval-ms <n>     {}\n      --salir-al-fin        {}\n      --recalc <ruta.json>  {}\n                            {}\n                            {}\n                            {}\n      --lcu [gameId]        {}\n                            {}\n  -h, --help                {}\n      --lang <codigo>       {}",
        i18n::tr("rofl-analyzer — analizador forense offline de replays .rofl"),
        i18n::tr("Uso:"),
        i18n::tr("captura en vivo"),
        i18n::tr("recalcula score desde una captura guardada"),
        i18n::tr("extrae timeline directo del cliente LoL (LCU)"),
        i18n::tr("Opciones:"),
        i18n::tr("Exporta el análisis completo a JSON"),
        i18n::tr("Muestreo de frames en ms (def. 1000 = 1 por segundo)"),
        i18n::tr("Vuelca el payload descomprimido (depuración / RE)"),
        i18n::tr("Modo scrim: sondea 127.0.0.1:2999 en vivo"),
        i18n::tr("Intervalo de sondeo en vivo (def. 1000)"),
        i18n::tr("Termina cuando el cliente deja de responder (partida acabada)"),
        i18n::tr("Recalcula quiebres/visión/Top5 desde una captura --scrim"),
        i18n::tr("guardada (deduplica el feed acumulativo y reconstruye"),
        i18n::tr("los contadores por ventana; sirve para calibrar pesos"),
        i18n::tr("y umbrales sin volver a jugar la partida)"),
        i18n::tr("Obtiene el timeline exacto del LCU (si se omite el ID,"),
        i18n::tr("se usa la partida más reciente del historial)"),
        i18n::tr("Muestra esta ayuda"),
        i18n::tr("Fuerza el idioma de la CLI (es, en, …)"),
    );
    for key in [
        "<archivo.rofl>",
        "<captura.json>",
        "<ruta.json>",
        "<ruta>",
        "<codigo>",
        "[opciones]",
    ] {
        text = text.replace(key, &i18n::tr(key));
    }
    text
}

fn requested_lang(args: &[String]) -> Option<String> {
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        if arg == "--lang" {
            return it.next().cloned();
        }
        if let Some(value) = arg.strip_prefix("--lang=") {
            return Some(value.to_string());
        }
    }
    None
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let lang = requested_lang(&args)
        .map(|code| Lang::from_code(&code))
        .unwrap_or_else(i18n::load_saved_lang);
    i18n::set_lang(lang);
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("{}", help_text());
        return ExitCode::SUCCESS;
    }
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("{} {msg}", i18n::tr("error:"));
            eprintln!("\n{}", help_text());
            ExitCode::FAILURE
        }
    }
}

struct Cli {
    input: String,
    json_out: Option<String>,
    sample_ms: u32,
    dump_payload: Option<String>,
    scrim: bool,
    interval_ms: u64,
    salir_al_fin: bool,
    recalc: Option<String>,
    lcu: Option<Option<u64>>,
}

fn parse_args(args: &[String]) -> Result<Cli, String> {
    let mut it = args.iter().peekable();
    let mut cli = Cli {
        input: String::new(),
        json_out: None,
        sample_ms: 1000,
        dump_payload: None,
        scrim: false,
        interval_ms: 1000,
        salir_al_fin: false,
        recalc: None,
        lcu: None,
    };

    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--json" | "-j" => {
                let v = it.next().ok_or("--json requires an output path")?;
                cli.json_out = Some(v.clone());
            }
            "--sample-ms" | "-s" => {
                let v = it.next().ok_or("--sample-ms requires a number")?;
                cli.sample_ms = v
                    .parse()
                    .map_err(|_| format!("invalid --sample-ms: {v}"))?;
            }
            "--dump-payload" => {
                let v = it.next().ok_or("--dump-payload requires a path")?;
                cli.dump_payload = Some(v.clone());
            }
            "--scrim" => cli.scrim = true,
            "--interval-ms" => {
                let v = it.next().ok_or("--interval-ms requires a number")?;
                cli.interval_ms = v
                    .parse()
                    .map_err(|_| format!("invalid --interval-ms: {v}"))?;
            }
            "--salir-al-fin" => cli.salir_al_fin = true,
            "--lang" => {
                let _ = it.next();
            }
            other if other.starts_with("--lang=") => {}
            "--recalc" => {
                let v = it.next().ok_or("--recalc requires the JSON capture path")?;
                cli.recalc = Some(v.clone());
            }
            "--lcu" => {
                if let Some(next) = it.peek() {
                    if !next.starts_with('-') {
                        if let Ok(id) = next.parse::<u64>() {
                            it.next();
                            cli.lcu = Some(Some(id));
                        } else {
                            cli.lcu = Some(None);
                        }
                    } else {
                        cli.lcu = Some(None);
                    }
                } else {
                    cli.lcu = Some(None);
                }
            }
            other if other.starts_with('-') => {
                return Err(format!("unknown option: {other}"));
            }
            other => {
                if !cli.input.is_empty() {
                    return Err("only one input file is allowed".to_string());
                }
                cli.input = other.to_string();
            }
        }
    }

    if !cli.scrim && cli.recalc.is_none() && cli.lcu.is_none() && cli.input.is_empty() {
        return Err("missing .rofl path (or use --scrim / --recalc / --lcu)".to_string());
    }
    Ok(cli)
}

fn run(args: &[String]) -> Result<(), String> {
    let cli = parse_args(args)?;

    if let Some(lcu_opt) = cli.lcu {
        return run_lcu(lcu_opt, cli.json_out.as_deref());
    }

    if let Some(path) = &cli.recalc {
        return run_recalc(path, cli.json_out.as_deref());
    }

    if cli.scrim {
        return run_scrim(&cli);
    }

    let opts = AnalyzeOptions {
        sample_ms: cli.sample_ms,
    };

    if let Some(path) = &cli.dump_payload {
        let payload =
            extract_payload_file(&cli.input).map_err(|e| format!("extract payload: {e}"))?;
        std::fs::write(path, &payload).map_err(|e| format!("write {path}: {e}"))?;
        println!(
            "{}",
            i18n::trf(
                "payload descomprimido volcado: {0} ({1} bytes)",
                &[&path, &payload.len().to_string()],
            )
        );
    }

    let replay =
        analyze_file(&cli.input, opts).map_err(|e| format!("analyzing {}: {e}", cli.input))?;

    print_summary(&replay);

    if let Some(path) = &cli.json_out {
        let json = to_json(&replay).map_err(|e| format!("serializing JSON: {e}"))?;
        std::fs::write(path, &json).map_err(|e| format!("write {path}: {e}"))?;
        println!(
            "{}",
            i18n::trf(
                "JSON exportado: {0} ({1} bytes)",
                &[path, &json.len().to_string()],
            )
        );
    }

    Ok(())
}

/// Scrim mode: polls the Live Client Data API while a match is active.
///
/// The output JSON is rewritten on every poll, so if it is interrupted with
/// Ctrl+C at any instant the file stays valid and up to date.
fn run_scrim(cli: &Cli) -> Result<(), String> {
    let interval = std::time::Duration::from_millis(cli.interval_ms.max(250));
    let mut poller = LivePoller::with_config(
        live::LIVE_ENDPOINT,
        2,
        interval,
        live::GoldModelParams::default(),
    );

    println!("{}", i18n::tr("=== Modo scrim ==="));
    println!("{} {}", i18n::tr("Endpoint:"), live::LIVE_ENDPOINT);
    println!(
        "{}",
        i18n::trf(
            "Sondeo cada {0} ms. Ctrl+C para detener y guardar.\n",
            &[&cli.interval_ms.to_string()],
        )
    );

    loop {
        match poller.poll_once() {
            Ok(true) => {
                let cap = poller.build_capture();
                let last = cap.blocks.last();
                print!(
                    "{}",
                    i18n::trf(
                        "\r[t={0}s] muestras={1} eventos={2} | CS {3}–{4} | Kills {5}–{6} | Nv {7}–{8}   ",
                        &[
                            &format!("{:6.1}", cap.duration_s),
                            &poller.snapshot_count().to_string(),
                            &cap.events.len().to_string(),
                            &cap.players
                                .iter()
                                .filter(|p| p.team == Team::Blue)
                                .map(|p| p.cs)
                                .sum::<u32>()
                                .to_string(),
                            &cap.players
                                .iter()
                                .filter(|p| p.team == Team::Red)
                                .map(|p| p.cs)
                                .sum::<u32>()
                                .to_string(),
                            &last.map(|b| b.kills_blue).unwrap_or(0).to_string(),
                            &last.map(|b| b.kills_red).unwrap_or(0).to_string(),
                            &last.map(|b| b.levels_blue_sum).unwrap_or(0).to_string(),
                            &last.map(|b| b.levels_red_sum).unwrap_or(0).to_string(),
                        ],
                    )
                );
                if let Some(path) = &cli.json_out {
                    let json = cap.to_json()?;
                    std::fs::write(path, &json).map_err(|e| format!("write {path}: {e}"))?;
                }
            }
            Ok(false) => {}
            Err(e) => {
                if poller.snapshot_count() == 0 {
                    // No match yet: retry silently.
                } else if cli.salir_al_fin && poller.looks_finished(3) {
                    println!("\n{}", i18n::tr("Partida terminada (el cliente dejó de responder)."));
                    break;
                }
                let _ = e;
            }
        }

        if cli.salir_al_fin && poller.looks_finished(3) {
            println!("\n{}", i18n::tr("Partida terminada (el cliente dejó de responder)."));
            break;
        }

        std::thread::sleep(poller.poll_interval());
    }

    let cap = poller.build_capture();
    println!(
        "{}",
        i18n::trf(
            "\n=== Resumen de la scrim ===\nModo {0} | Mapa {1} | {2}s | {3} muestras | {4} eventos",
            &[
                &cap.game_mode,
                &cap.map_name,
                &format!("{:.1}", cap.duration_s),
                &cap.poll_count.to_string(),
                &cap.events.len().to_string(),
            ],
        )
    );
    println!(
        "{}",
        i18n::trf(
            "Oro/XP: reconstruido (reconstruido_por_modelo={0}). CS/KDA/niveles: medidos.",
            &[&cap.reconstruido_por_modelo.to_string()],
        )
    );
    for (tt, name) in [(&cap.top5_azul, "Azul"), (&cap.top5_rojo, "Rojo")] {
        println!(
            "{}",
            i18n::trf("Top 5 desastres {0}:", &[&i18n::tr(name)])
        );
        for b in &tt.desastres {
            println!(
                "{}",
                i18n::trf(
                    "  #{0} [{1}s–{2}s] oro {3} vs {4} ⇒ neto {5}",
                    &[
                        &b.block_index.to_string(),
                        &format!("{:6.0}", b.start_s),
                        &format!("{:6.0}", b.end_s),
                        &format!("{:+}", b.delta_own),
                        &format!("{:+}", b.delta_enemy),
                        &format!("{:+}", b.net),
                    ],
                )
            );
        }
    }

    // Where the match broke down: each 30 s window with an alert, separately.
    print_dominion(&cap.dominion);
    if let Some(path) = &cli.json_out {
        let json = cap.to_json()?;
        std::fs::write(path, &json).map_err(|e| format!("write {path}: {e}"))?;
        println!(
            "{}",
            i18n::trf(
                "JSON exportado: {0} ({1} bytes)",
                &[path, &json.len().to_string()],
            )
        );
    }
    Ok(())
}

/// Prints the score breakpoints, the vision alerts and the score Top 5
/// of a dominion report (used by `--scrim` and `--recalc`).
fn print_dominion(d: &DominionReport) {
    println!(
        "{}",
        i18n::trf(
            "Quiebres de score ({0} ventanas con |score| ≥ {1}):",
            &[
                &d.breakpoints.len().to_string(),
                &format!("{:.0}", d.severity_thresholds.semi_alerta),
            ],
        )
    );
    for q in &d.breakpoints {
        println!(
            "{}",
            i18n::trf(
                "  [{0}–{1}] score {2} ⇒ {3} ({4})",
                &[
                    &fmt_mmss(q.start_s),
                    &fmt_mmss(q.end_s),
                    &format!("{:+6.1}", q.score_net_blue),
                    &i18n::tr(q.severity.label()),
                    &i18n::tr(q.favorecido.as_str()),
                ],
            )
        );
    }
    if d.breakpoints.is_empty() {
        println!("{}", i18n::tr("  (ninguna: ninguna ventana alcanzó el piso)"));
    }
    println!(
        "{}",
        i18n::trf(
            "Alertas de visión ({0}):",
            &[&d.vision.alerts.len().to_string()],
        )
    );
    for a in &d.vision.alerts {
        println!(
            "{}",
            i18n::trf(
                "  [{0}–{1}] visión azul {2} · visión roja {3} (Δ {4} a favor de {5}) ⇒ {6} ({7})",
                &[
                    &fmt_mmss(a.start_s),
                    &fmt_mmss(a.end_s),
                    &format!("{:.1}", a.delta_azul),
                    &format!("{:.1}", a.delta_rojo),
                    &format!("{:.1}", (a.delta_azul - a.delta_rojo).abs()),
                    &i18n::tr(a.favorecido.as_str()),
                    &i18n::tr(a.severity.label_vision()),
                    &i18n::tr(a.favorecido.as_str()),
                ],
            )
        );
    }
    if d.vision.alerts.is_empty() {
        println!("{}", i18n::tr("  (ninguna)"));
    }
    // Score Top 5 per team: no threshold, always emitted.
    for (t, name) in [(&d.top5_azul, "Azul"), (&d.top5_rojo, "Rojo")] {
        println!(
            "{}",
            i18n::trf("Top 5 score — mejores ventanas de {0}:", &[&i18n::tr(name)])
        );
        for b in &t.exitos {
            println!(
                "{}",
                i18n::trf(
                    "  [{0}–{1}] neto {2}",
                    &[
                        &fmt_mmss(b.start_s),
                        &fmt_mmss(b.end_s),
                        &format!("{:+6.1}", b.net),
                    ],
                )
            );
        }
        println!(
            "{}",
            i18n::trf("Top 5 score — peores ventanas de {0}:", &[&i18n::tr(name)])
        );
        for b in &t.desastres {
            println!(
                "{}",
                i18n::trf(
                    "  [{0}–{1}] neto {2}",
                    &[
                        &fmt_mmss(b.start_s),
                        &fmt_mmss(b.end_s),
                        &format!("{:+6.1}", b.net),
                    ],
                )
            );
        }
    }
}

/// Recomputes the dominion report from a saved `--scrim` capture.
///
/// The capture is **deduplicated** (builds before deduplication by `EventID`
/// accumulated copies of the cumulative feed: every poll re-emits all events),
/// the per-window counters are rebuilt from the attributed events and
/// breakpoints, vision and Top 5 are re-emitted. It allows calibrating weights
/// and thresholds without replaying the match.
fn run_recalc(path: &str, json_out: Option<&str>) -> Result<(), String> {
    let raw =
        std::fs::read_to_string(path).map_err(|e| format!("read {path}: {e}"))?;
    let v: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("parse {path}: {e}"))?;

    let mut blocks: Vec<ScrimBlock> = serde_json::from_value(
        v.get("blocks").cloned().unwrap_or(serde_json::Value::Null),
    )
    .map_err(|e| format!("invalid blocks in {path}: {e}"))?;
    let events: Vec<AttributedEvent> = serde_json::from_value(
        v.get("events").cloned().unwrap_or(serde_json::Value::Null),
    )
    .map_err(|e| format!("invalid events in {path}: {e}"))?;

    if blocks.is_empty() {
        return Err(format!("capture {path} has no blocks"));
    }
    let duration_s = v
        .get("duration_s")
        .and_then(serde_json::Value::as_f64)
        .map(|d| d as f32)
        .unwrap_or_else(|| blocks.last().map(|b| b.end_s).unwrap_or(0.0));

    let count_kills = |evs: &[AttributedEvent]| -> (u32, u32) {
        evs.iter()
            .filter(|e| e.kind == "ChampionKill")
            .fold((0, 0), |(b, r), e| match e.team {
                Some(Team::Blue) => (b + 1, r),
                Some(Team::Red) => (b, r + 1),
                None => (b, r),
            })
    };
    let raw_kills = count_kills(&events);
    let deduped = dedup_saved_events(&events, SAVED_EVENT_EPSILON_S);
    rebuild_event_counters(&mut blocks, &deduped);
    let kills = count_kills(&deduped);

    println!("{}", i18n::tr("=== Recálculo de captura guardada ==="));
    println!("{} {path}", i18n::tr("Captura:"));
    println!(
        "{}",
        i18n::trf(
            "Duración {0}s | {1} bloques de 30 s | {2} eventos → {3} tras dedup (ε = {4}s)",
            &[
                &format!("{:.1}", duration_s),
                &blocks.len().to_string(),
                &events.len().to_string(),
                &deduped.len().to_string(),
                &format!("{:.1}", SAVED_EVENT_EPSILON_S),
            ],
        )
    );
    println!(
        "{}",
        i18n::trf(
            "Kills en el feed: {0} (Azul {1}, Rojo {2}) → deduplicadas: {3} (Azul {4}, Rojo {5})",
            &[
                &(raw_kills.0 + raw_kills.1).to_string(),
                &raw_kills.0.to_string(),
                &raw_kills.1.to_string(),
                &(kills.0 + kills.1).to_string(),
                &kills.0.to_string(),
                &kills.1.to_string(),
            ],
        )
    );
    // Measured reconciliation: deduplicated events vs measured final scores.
    let mut score_kills = (0u32, 0u32);
    if let Some(players) = v.get("players").and_then(serde_json::Value::as_array) {
        for p in players {
            let team = p
                .get("team")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            let k = p
                .get("kills")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0) as u32;
            match team {
                "blue" => score_kills.0 += k,
                "red" => score_kills.1 += k,
                _ => {}
            }
        }
    }
    if score_kills.0 > kills.0 || score_kills.1 > kills.1 {
        println!(
            "{}",
            i18n::trf(
                "Aviso: los scores finales miden {0}–{1} kills (Azul–Rojo) y el feed sólo aporta {2}–{3}: faltan {4} de Azul y {5} de Rojo en events.Events (omisión observada del feed de Riot). Esas ventanas carecen de esa kill.",
                &[
                    &score_kills.0.to_string(),
                    &score_kills.1.to_string(),
                    &kills.0.to_string(),
                    &kills.1.to_string(),
                    &score_kills.0.saturating_sub(kills.0).to_string(),
                    &score_kills.1.saturating_sub(kills.1).to_string(),
                ],
            )
        );
    }

    let report = rofl_analyzer::score::compute_report(
        &blocks,
        Default::default(),
        Default::default(),
        "captura_guardada_recalculada",
    );
    print_dominion(&report);

    if let Some(out) = json_out {
        let doc = serde_json::json!({
            "fuente": "captura_guardada_recalculada",
            "captura_origen": path,
            "duracion_s": duration_s,
            "dedup": {
                "epsilon_s": SAVED_EVENT_EPSILON_S,
                "eventos_brutos": events.len(),
                "eventos_deduplicados": deduped.len(),
                "kills_brutas_azul": raw_kills.0,
                "kills_brutas_rojo": raw_kills.1,
                "kills_azul": kills.0,
                "kills_rojo": kills.1,
            },
            "dominion": report,
        });
        let json = serde_json::to_string_pretty(&doc)
            .map_err(|e| format!("serializing JSON: {e}"))?;
        std::fs::write(out, &json).map_err(|e| format!("write {out}: {e}"))?;
        println!(
            "{}",
            i18n::trf(
                "JSON exportado: {0} ({1} bytes)",
                &[out, &json.len().to_string()],
            )
        );
    }
    Ok(())
}

fn run_lcu(game_id_opt: Option<u64>, json_out: Option<&str>) -> Result<(), String> {
    println!("{}", i18n::tr("Conectando con el League Client (LCU)..."));
    let client = LcuClient::connect()?;

    let game_id = match game_id_opt {
        Some(id) => id,
        None => {
            println!("{}", i18n::tr("Buscando última partida en el historial del cliente..."));
            let matches_val = client.get_recent_matches()?;
            let games = matches_val
                .get("games")
                .and_then(|g| g.get("games"))
                .and_then(|arr| arr.as_array())
                .ok_or("No matches found in the client history")?;

            let first_game = games.first().ok_or("The match history is empty")?;
            first_game
                .get("gameId")
                .and_then(|id| id.as_u64())
                .ok_or("Could not extract the gameId of the last match")?
        }
    };

    println!(
        "{}",
        i18n::trf(
            "Descargando timeline completo para la partida {0}...",
            &[&game_id.to_string()],
        )
    );
    let timeline = client.get_timeline(game_id)?;

    let gold_series = timeline.gold_series();
    let wards = timeline.ward_events();
    let objectives = timeline.objective_events();
    let duration_s = timeline.duration_s();

    println!("{}", i18n::tr("=== Timeline LCU obtenido exitosamente ==="));
    println!("{} {game_id}", i18n::tr("ID de partida:"));
    println!(
        "{}",
        i18n::trf(
            "Duración estimada: {0} min ({1} s)",
            &[
                &format!("{:.1}", duration_s / 60.0),
                &format!("{:.0}", duration_s),
            ],
        )
    );
    println!(
        "{} {}",
        i18n::tr("Muestras de oro por minuto:"),
        gold_series.len()
    );
    println!(
        "{} {}",
        i18n::tr("Wards registrados con coordenadas exactas:"),
        wards.len()
    );
    println!(
        "{} {}",
        i18n::tr("Objetivos épicos registrados:"),
        objectives.len()
    );

    let gold_blocks = compute_gold_blocks(&gold_series, duration_s, "lcu_timeline");

    println!(
        "\n{}",
        i18n::tr("--- Curva de Oro Real por Ventanas de 30s (Top 5 Éxitos Azul) ---")
    );
    for (i, b) in gold_blocks.top5_azul.exitos.iter().take(5).enumerate() {
        println!(
            "{}",
            i18n::trf(
                "  #{0}: [{1}:{2} – {3}:{4}] Delta Azul: +{5}g | Delta Rojo: +{6}g ⇒ Ventaja Neta: {7}g",
                &[
                    &(i + 1).to_string(),
                    &format!("{:02.0}", b.start_s / 60.0),
                    &format!("{:02.0}", b.start_s % 60.0),
                    &format!("{:02.0}", b.end_s / 60.0),
                    &format!("{:02.0}", b.end_s % 60.0),
                    &b.delta_own.to_string(),
                    &b.delta_enemy.to_string(),
                    &format!("{:+}", b.net),
                ],
            )
        );
    }

    if !objectives.is_empty() {
        println!("\n{}", i18n::tr("--- Objetivos Épicos (Breakpoints) ---"));
        for (t_s, m_type, sub_type, killer_id, team_id) in &objectives {
            let team_name = match team_id {
                Some(100) => i18n::tr("AZUL"),
                Some(200) => i18n::tr("ROJO"),
                _ => i18n::tr("DESCONOCIDO"),
            };
            let desc = match sub_type {
                Some(sub) => format!("{m_type} ({sub})"),
                None => m_type.clone(),
            };
            println!(
                "{}",
                i18n::trf(
                    "  [{0}:{1}] {2} abatido por jugador {3} (Equipo {4})",
                    &[
                        &format!("{:02.0}", t_s / 60.0),
                        &format!("{:02.0}", t_s % 60.0),
                        &desc,
                        &killer_id.to_string(),
                        &team_name,
                    ],
                )
            );
        }
    }

    if let Some(out) = json_out {
        let doc = serde_json::json!({
            "fuente": "lcu_timeline",
            "game_id": game_id,
            "duration_s": duration_s,
            "gold_series": gold_series,
            "gold_blocks": gold_blocks,
            "wards": wards.iter().map(|(t, wtype, x, y, cid)| {
                serde_json::json!({
                    "timestamp_s": t,
                    "ward_type": wtype,
                    "x": x,
                    "y": y,
                    "creator_id": cid,
                })
            }).collect::<Vec<_>>(),
            "objectives": objectives.iter().map(|(t, mtype, subtype, kid, tid)| {
                serde_json::json!({
                    "timestamp_s": t,
                    "monster_type": mtype,
                    "sub_type": subtype,
                    "killer_id": kid,
                    "team_id": tid,
                })
            }).collect::<Vec<_>>(),
        });

        let json = serde_json::to_string_pretty(&doc)
            .map_err(|e| format!("serializing JSON: {e}"))?;
        std::fs::write(out, &json).map_err(|e| format!("write {out}: {e}"))?;
        println!(
            "\n{}",
            i18n::trf(
                "JSON completo exportado a: {0} ({1} bytes)",
                &[out, &json.len().to_string()],
            )
        );
    }

    Ok(())
}

/// Seconds as `mm:ss` for the CLI summaries (840.0 ⇒ `"14:00"`).
fn fmt_mmss(s: f32) -> String {
    let total = s.max(0.0).round() as u64;
    format!("{:02}:{:02}", total / 60, total % 60)
}

fn print_summary(r: &Replay) {
    let secs = r.metadata.game_length_s;
    println!("{}", i18n::tr("=== Resumen del replay ==="));
    println!(
        "{}",
        i18n::trf(
            "Formato: {0} | Juego {1} | versión {2} | modo {3} ({4}) | mapa {5} | cola {6}",
            &[
                &r.metadata.source_format,
                &r.metadata.game_id.to_string(),
                &r.metadata.game_version,
                &r.metadata.game_mode,
                &r.metadata.game_type,
                &r.metadata.map_id.to_string(),
                &r.metadata.queue_id.to_string(),
            ],
        )
    );
    println!(
        "{}",
        i18n::trf(
            "Duración: {0} s ({1} min) | segmentos: {2} | frames muestreados: {3} | eventos de visión: {4}",
            &[
                &format!("{:.1}", secs),
                &((secs / 60.0).round() as u32).to_string(),
                &r.segment_count.to_string(),
                &r.frames.len().to_string(),
                &r.events.len().to_string(),
            ],
        )
    );
    println!();

    println!(
        "{:<4} {:<20} {:<14} {:<6} {:>2} {:>3} {:>3} {:>6} {:>7} {:>6} {:>6}",
        i18n::tr("Slot"),
        i18n::tr("Invocador"),
        i18n::tr("Champ"),
        i18n::tr("Equipo"),
        i18n::tr("W"),
        i18n::tr("K"),
        i18n::tr("D"),
        i18n::tr("A"),
        i18n::tr("Oro"),
        i18n::tr("CS"),
        i18n::tr("Wards")
    );
    for p in &r.players {
        let champ = p
            .champion_name
            .clone()
            .unwrap_or_else(|| p.champion_id.to_string());
        let name = if p.summoner_name.is_empty() {
            "-"
        } else {
            &p.summoner_name
        };
        let win = match p.won {
            Some(true) => i18n::tr("V"),
            Some(false) => i18n::tr("D"),
            None => i18n::tr("-"),
        };
        println!(
            "{:<4} {:<20} {:<14} {:<6} {:>2} {:>3} {:>3} {:>6} {:>7} {:>6} {:>6}",
            p.slot,
            truncate(name, 20),
            truncate(&champ, 14),
            i18n::tr(p.team.as_str()),
            win,
            p.stats.kills,
            p.stats.deaths,
            p.stats.assists,
            p.stats.gold_earned,
            p.stats.cs,
            p.stats.wards_placed
        );
    }
    println!();

    let (b, red) = (&r.team_totals.blue, &r.team_totals.red);
    println!("{}", i18n::tr("=== Totales por equipo ==="));
    println!(
        "{:<6} {:>8} {:>7} {:>7} {:>7} {:>8} {:>8}",
        i18n::tr("Equipo"),
        i18n::tr("Oro"),
        i18n::tr("CS"),
        i18n::tr("Kills"),
        i18n::tr("Assists"),
        i18n::tr("Wards"),
        i18n::tr("Daño")
    );
    for (name, t) in [("Azul", b), ("Rojo", red)] {
        println!(
            "{:<6} {:>8} {:>7} {:>7} {:>7} {:>8} {:>8}",
            i18n::tr(name),
            t.gold_earned,
            t.cs,
            t.kills,
            t.assists,
            t.wards_placed,
            t.damage_to_champions
        );
    }
    println!();

    // Wards per minute (extracted from the payload, with coordinates).
    if r.metadata.source_format == "legacy" {
        let placed = r
            .events
            .iter()
            .filter(|e| matches!(e.kind, EventKind::WardPlaced { .. }))
            .count();
        let killed = r.events.len() - placed;
        println!(
            "{}",
            i18n::trf(
                "Telemetría: {0} wards colocados, {1} destruidos (payload, con coordenadas x,y)",
                &[&placed.to_string(), &killed.to_string()],
            )
        );
    }

    // Payload section summary (modern format only).
    if let Some(s) = &r.sections {
        println!(
            "{}",
            i18n::trf(
                "Secciones del payload: {0} (duración {1} s), {2} tipos distintos:",
                &[
                    &s.total_sections.to_string(),
                    &format!("{:.1}", s.duration_s),
                    &s.types.len().to_string(),
                ],
            )
        );
        for t in s.types.iter().take(8) {
            println!(
                "{}",
                i18n::trf(
                    "  tipo {0} × {1} bytes {2} ({3}–{4} B) t=[{5}s, {6}s]",
                    &[
                        &format!("{:<5}", t.type_id),
                        &format!("{:<7}", t.count),
                        &format!("{:<9}", t.total_bytes),
                        &t.min_len.to_string(),
                        &t.max_len.to_string(),
                        &format!("{:.1}", t.first_time_s),
                        &format!("{:.1}", t.last_time_s),
                    ],
                )
            );
        }
        println!(
            "{}",
            i18n::tr("  (pendiente de decodificar: wards del formato moderno)")
        );
    }

    // x/y trajectories per champion (modern format: type 61 stream,
    // experimental — only shown if they passed physical validation).
    if !r.positions.is_empty() {
        println!(
            "{}",
            i18n::tr("=== Posiciones por campeón (stream tipo 61, experimental ✓ validado) ===")
        );
        for t in &r.positions {
            match (t.samples.first(), t.samples.last()) {
                (Some(f), Some(l)) if t.samples.len() > 1 => println!(
                    "{}",
                    i18n::trf(
                        "  slot {0} [{1}] id=0x{2} | {3} muestras | ({4}, {5}) → ({6}, {7})",
                        &[
                            &t.slot.to_string(),
                            &i18n::tr(t.team.as_str()),
                            &format!("{:02X}", t.entity_tag),
                            &t.samples.len().to_string(),
                            &format!("{:.0}", f.x),
                            &format!("{:.0}", f.y),
                            &format!("{:.0}", l.x),
                            &format!("{:.0}", l.y),
                        ],
                    )
                ),
                _ => println!(
                    "{}",
                    i18n::trf(
                        "  slot {0} [{1}] id=0x{2} | sin muestras",
                        &[
                            &t.slot.to_string(),
                            &i18n::tr(t.team.as_str()),
                            &format!("{:02X}", t.entity_tag),
                        ],
                    )
                ),
            }
        }
    }

    // 30 s block report with gold delta and Top 5.
    if let Some(g) = &r.gold_blocks {
        println!(
            "{}",
            i18n::trf(
                "=== Bloques de oro de {0} s (fuente: {1}) ===",
                &[&g.block_secs.to_string(), &g.fuente],
            )
        );
        if g.fuente == "no_disponible" {
            println!(
                "{}",
                i18n::tr(
                    "  la serie de oro temporal aún no está decodificada en este formato: la estructura (bloques + top5) se exporta vacía en el JSON, sin datos inventados"
                )
            );
        } else {
            for (tt, name) in [(&g.top5_azul, "Azul"), (&g.top5_rojo, "Rojo")] {
                println!(
                    "{}",
                    i18n::trf("  Top 5 éxitos {0} (verde):", &[&i18n::tr(name)])
                );
                for b in &tt.exitos {
                    println!(
                        "{}",
                        i18n::trf(
                            "    #{0} [{1}s–{2}s] oro {3} vs {4} ⇒ neto {5}",
                            &[
                                &b.block_index.to_string(),
                                &format!("{:6.0}", b.start_s),
                                &format!("{:6.0}", b.end_s),
                                &format!("{:+}", b.delta_own),
                                &format!("{:+}", b.delta_enemy),
                                &format!("{:+}", b.net),
                            ],
                        )
                    );
                }
                println!(
                    "{}",
                    i18n::trf("  Top 5 desastres {0} (rojo):", &[&i18n::tr(name)])
                );
                for b in &tt.desastres {
                    println!(
                        "{}",
                        i18n::trf(
                            "    #{0} [{1}s–{2}s] oro {3} vs {4} ⇒ neto {5}",
                            &[
                                &b.block_index.to_string(),
                                &format!("{:6.0}", b.start_s),
                                &format!("{:6.0}", b.end_s),
                                &format!("{:+}", b.delta_own),
                                &format!("{:+}", b.delta_enemy),
                                &format!("{:+}", b.net),
                            ],
                        )
                    );
                }
            }
        }
    }

    // First frame as a telemetry sample.
    if let Some(f) = r.frames.first() {
        println!(
            "{}",
            i18n::trf("Muestra de frame @ {0} ms:", &[&f.timestamp_ms.to_string()])
        );
        for (i, st) in f.participants.iter().enumerate() {
            let side = if i < 5 { Team::Blue } else { Team::Red };
            println!(
                "{}",
                i18n::trf(
                    "  [{0}][{1}] x={2} y={3} hp={4}/{5} lvl={6} {7}",
                    &[
                        &i.to_string(),
                        &i18n::tr(side.as_str()),
                        &format!("{:.0}", st.x),
                        &format!("{:.0}", st.y),
                        &st.health.to_string(),
                        &st.max_health.to_string(),
                        &st.level.to_string(),
                        &i18n::tr(if st.alive { "vivo" } else { "muerto" }),
                    ],
                )
            );
        }
    }

    if !r.warnings.is_empty() {
        println!(
            "\n{}",
            i18n::trf("=== Avisos ({0}) ===", &[&r.warnings.len().to_string()])
        );
        for w in &r.warnings {
            println!(" - {w}");
        }
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{cut}…")
    }
}
