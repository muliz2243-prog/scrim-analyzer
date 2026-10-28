//! Diagnostics for a replay report using the LCU timeline.
//!
//! ```text
//! cargo run --example replay_report -- 1632883165
//! ```

use rofl_analyzer::{
    blocks::compute_gold_blocks,
    gold_breakdown::gold_blocks_with_axes,
    score::{compute_breakpoints, compute_vision, score_top5},
    lcu::LcuClient,
    SeverityThresholds, VisionThresholds,
};

fn main() -> Result<(), String> {
    let id: u64 = std::env::args()
        .nth(1)
        .ok_or("uso: replay_report <gameId>")?
        .parse()
        .map_err(|e| format!("gameId inválido: {e}"))?;
    let client = LcuClient::connect()?;
    let timeline = client.get_timeline(id)?;
    let summary = client.get_match_summary(id)?;
    let dur = timeline.duration_s();
    let gold = compute_gold_blocks(&timeline.gold_series(), dur, "lcu_timeline");
    let blocks = gold_blocks_with_axes(&timeline, &gold.blocks, &SeverityThresholds::default());
    let breaks = compute_breakpoints(&blocks);
    let (t5a, t5r) = score_top5(&blocks);
    let vision = compute_vision(&[], VisionThresholds::default());

    println!("gameId {id} · {:.1}s · {} bloques de 30s", dur, blocks.len());
    println!(
        "alertas={} · top5 azul={} rojo={} · avisos={} · wards timeline={} · total={}/{}",
        breaks.len(),
        t5a.exitos.len(),
        t5r.exitos.len(),
        timeline.objective_events().len(),
        timeline.ward_events().len(),
        summary.vision.blue,
        summary.vision.red
    );
    if let Some(b) = breaks.first() {
        println!(
            "primera alerta: [{} - {}] {:+.0}g a favor del {:?} ({:?})",
            b.start_s, b.end_s, b.score_net_blue, b.favorecido, b.severity
        );
        for a in &blocks[b.block_index as usize].axes {
            println!(
                "  {:<10} azul={:+.0} rojo={:+.0} net={:+.0} disp={}",
                a.axis, a.blue, a.red, a.net_blue, a.available
            );
        }
    }
    if let Some(b) = t5a.exitos.first() {
        println!(
            "mejor ventana azul: [{} - {}] {:+.0}g · ejes={}",
            b.start_s,
            b.end_s,
            b.net,
            blocks[b.block_index as usize].axes.len()
        );
    }
    println!("vision alerts={} fuente={:?}", vision.alerts.len(), vision.fuente);
    Ok(())
}
