//! Fixed 30 s blocks with gold delta per team and Top 5.
//!
//! The engine is agnostic about the origin of the gold series: it receives a
//! cumulative series `[(t_s, oro_azul, oro_rojo)]` (non-decreasing per team, in
//! seconds) and computes:
//!
//! 1. the gold deltas of each team in **strictly contiguous** blocks of
//!    [`BLOCK_SECS`] seconds (`[0,30)`, `[30,60)`, …; the last block is
//!    truncated to the real duration of the game);
//! 2. for each team, the Top 5 **successes** (blocks with the largest net
//!    advantage in their favor: `delta_propio - delta_rival`) and **disasters**
//!    (the 5 worst, largest net loss).
//!
//! The values at block boundaries are obtained by **linear interpolation**
//! between the two contiguous samples of the series, so the precision of the
//! delta depends only on the density of the series.

use crate::model::{GoldBlock, GoldBlocksReport, TeamTop5, TopBlock};

/// Fixed block length, in seconds.
pub const BLOCK_SECS: f32 = 30.0;

/// Cumulative gold series: `(t_s, oro_acumulado_azul, oro_acumulado_rojo)`.
pub type GoldSeries = [(f32, i64, i64)];

/// Value of the series at instant `t` by linear interpolation between
/// contiguous samples. The series is assumed to be sorted by time.
fn interp(series: &GoldSeries, t: f32, team_blue: bool) -> i64 {
    if series.is_empty() {
        return 0;
    }
    let pick = |s: &(f32, i64, i64)| if team_blue { s.1 } else { s.2 };
    if t <= series[0].0 {
        return pick(&series[0]);
    }
    let last = series.len() - 1;
    if t >= series[last].0 {
        return pick(&series[last]);
    }
    // Binary search for the pair of contiguous samples.
    let mut lo = 0usize;
    let mut hi = last;
    while hi - lo > 1 {
        let mid = (lo + hi) / 2;
        if series[mid].0 <= t {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let (t0, ..) = series[lo];
    let (t1, ..) = series[hi];
    let a = pick(&series[lo]) as f64;
    let b = pick(&series[hi]) as f64;
    if t1 <= t0 {
        return b as i64;
    }
    let w = (f64::from(t - t0) / f64::from(t1 - t0)).clamp(0.0, 1.0);
    (a + (b - a) * w).round() as i64
}

/// Builds the 30 s blocks report from a gold series.
///
/// `fuente` is copied as is into the report (e.g. `"telemetria"`).
pub fn compute_gold_blocks(series: &GoldSeries, duration_s: f32, fuente: &str) -> GoldBlocksReport {
    let n_blocks = if duration_s > 0.0 {
        (f64::from(duration_s) / f64::from(BLOCK_SECS)).ceil() as u32
    } else {
        0
    };
    let mut blocks: Vec<GoldBlock> = Vec::with_capacity(n_blocks as usize);
    for k in 0..n_blocks {
        let start = k as f32 * BLOCK_SECS;
        let end = (start + BLOCK_SECS).min(duration_s.max(start));
        let b0 = interp(series, start, true);
        let b1 = interp(series, end, true);
        let r0 = interp(series, start, false);
        let r1 = interp(series, end, false);
        blocks.push(GoldBlock {
            index: k,
            start_s: start,
            end_s: end,
            gold_blue_delta: b1 - b0,
            gold_red_delta: r1 - r0,
            net_blue: (b1 - b0) - (r1 - r0),
        });
    }

    let top5 = |own_is_blue: bool| -> TeamTop5 {
        // Without a series there are no successes or disasters to show: empty
        // lists are better than blocks filled with zeros (nothing invented).
        if series.is_empty() {
            return TeamTop5::default();
        }
        let mut entries: Vec<TopBlock> = blocks
            .iter()
            .map(|b| {
                let (own, enemy) = if own_is_blue {
                    (b.gold_blue_delta, b.gold_red_delta)
                } else {
                    (b.gold_red_delta, b.gold_blue_delta)
                };
                TopBlock {
                    block_index: b.index,
                    start_s: b.start_s,
                    end_s: b.end_s,
                    delta_own: own,
                    delta_enemy: enemy,
                    net: own - enemy,
                }
            })
            .collect();
        // Successes: largest net advantage; disasters: the same list reversed.
        entries.sort_by(|a, c| c.net.cmp(&a.net).then(a.block_index.cmp(&c.block_index)));
        let n = entries.len().min(5);
        let exitos = entries[..n].to_vec();
        let desastres = entries[entries.len().saturating_sub(n)..].to_vec();
        TeamTop5 {
            exitos,
            desastres,
        }
    };

    let top5_azul = top5(true);
    let top5_rojo = top5(false);

    GoldBlocksReport {
        fuente: fuente.to_string(),
        block_secs: BLOCK_SECS as u32,
        blocks,
        top5_azul,
        top5_rojo,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Synthetic series: blue gains 100 gold/min, red 60 gold/min except for a
    /// blue "disaster" in block 3 (red spikes +2000 in that block).
    fn serie() -> Vec<(f32, i64, i64)> {
        let mut s = Vec::new();
        for k in 0..=10u32 {
            let t = k as f32 * 10.0; // samples every 10 s
            let mut blue = (f64::from(t) * 1.6667) as i64; // ≈100/min
            let mut red = (f64::from(t) * 1.0) as i64; // 60/min
            if t >= 40.0 {
                blue -= 1500; // slump: blue loses relative gold at t=40..50
            }
            if t >= 40.0 {
                red += 500;
            }
            s.push((t, blue.max(0), red));
        }
        s
    }

    #[test]
    fn bloques_contiguos_y_deltas() {
        let r = compute_gold_blocks(&serie(), 100.0, "test");
        assert_eq!(r.block_secs, 30);
        assert_eq!(r.blocks.len(), 4); // ceil(100/30)
        // Exact contiguity: end of one block = start of the next.
        for w in r.blocks.windows(2) {
            assert_eq!(w[0].end_s, w[1].start_s);
        }
        assert_eq!(r.blocks[0].start_s, 0.0);
        assert_eq!(r.blocks[3].end_s, 100.0); // the last one is truncated to the duration
    }

    #[test]
    fn interpolacion_en_bordes() {
        // Exact linear series: blue = 10*t, red = 4*t.
        let s: Vec<(f32, i64, i64)> =
            (0..=100).step_by(5).map(|t| (t as f32, (10 * t) as i64, (4 * t) as i64)).collect();
        let r = compute_gold_blocks(&s, 100.0, "test");
        for b in &r.blocks {
            let dur = b.end_s - b.start_s;
            assert!((b.gold_blue_delta - (10.0 * f64::from(dur)) as i64).abs() <= 1);
            assert!((b.gold_red_delta - (4.0 * f64::from(dur)) as i64).abs() <= 1);
            assert_eq!(b.net_blue, b.gold_blue_delta - b.gold_red_delta);
        }
    }

    #[test]
    fn top5_ordena_por_ventaja_neta() {
        let r = compute_gold_blocks(&serie(), 100.0, "test");
        // Block 3 (90..100? no: index 3 = 90..100) — the slump is at
        // t∈[40,50] ⇒ it falls in block index 1 (30..60). Checks that the
        // worst blue block is 1.
        let peor = r.top5_azul.desastres.last().unwrap();
        assert_eq!(peor.block_index, 1);
        assert!(peor.net < 0, "el bache debe dar ventaja neta negativa: {}", peor.net);
        // Successes sorted from largest to smallest.
        for w in r.top5_azul.exitos.windows(2) {
            assert!(w[0].net >= w[1].net);
        }
        // Red: its best block is where blue fails (index 1).
        assert_eq!(r.top5_rojo.exitos[0].block_index, 1);
    }

    #[test]
    fn serie_vacia_no_panic() {
        let r = compute_gold_blocks(&[], 1352.3, "no_disponible");
        assert_eq!(r.blocks.len(), 46); // ceil(1352.3/30)
        assert!(r.top5_azul.exitos.is_empty());
        assert!(r.top5_rojo.desastres.is_empty());
        // Without a series, all deltas are zero.
        assert!(r.blocks.iter().all(|b| b.net_blue == 0));
    }
}
