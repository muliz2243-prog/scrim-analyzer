//! Extraction of **economic events** from the modern `.rofl` format.
//!
//! # What is detected and how it was verified
//!
//! While scanning the section stream, four section types were located that
//! represent real game events (not periodic snapshots):
//!
//! | Type | Event                 | Validation signal                                    |
//! |------|-----------------------|------------------------------------------------------|
//! | 166  | Champion kill         | 51 instances, irregular spacing (CV = 1.30)          |
//! | 131  | Assist                | 53 instances                                         |
//! | 1183 | Turret plate          | 58 instances                                         |
//! | 232  | Tower destroyed       | 6 instances (identical to those of type 850)         |
//!
//! Validation is **dual**: every count must match *exactly* the corresponding
//! total from the `statsJson` (`CHAMPIONS_KILLED`, `ASSISTS`,
//! `Missions_TurretPlatesDestroyed`, `TURRETS_KILLED`) and, in addition, the last
//! kill event must fall at most at `LAST_TAKEDOWN_TIME`. If anything does not add
//! up, no event is emitted (`fuente = "no_disponible"`).
//!
//! # Why there is no team attribution
//!
//! Each event record only carries a timestamp; the fields that look like entity
//! identifiers **do not** map to the 10 champions:
//!
//! * the histograms per value do not reproduce kills/assists per player;
//! * the leftover fields of the E8 mask (bits 47 and 64) are not health (they
//!   drop to ~0 for seven simultaneous champions);
//! * the position gaps do not mark deaths (558 noise gaps).
//!
//! Splitting the gold per team would require splitting each event, and that
//! would be **fabricating**. So the gold per team stays at `no_disponible`.

use crate::model::{EventKind, EventsReport, GameEvent};

/// Section type of a champion kill.
const SEC_KILL: u16 = 166;
/// Section type of an assist.
const SEC_ASSIST: u16 = 131;
/// Section type of a destroyed turret plate.
const SEC_PLATE: u16 = 1183;
/// Section type of a destroyed tower.
///
/// There are two views of these 6 events (232 and 850) with identical
/// timestamps; the **850** is used because its records have a fixed length
/// (5 B), while those of 232 measure 28/33 B and mix in more state.
const SEC_TURRET: u16 = 850;

/// Tolerance, in seconds, when matching the last kill against
/// `LAST_TAKEDOWN_TIME` (which the `statsJson` publishes rounded to seconds).
const LAST_TAKEDOWN_TOLERANCE_S: f32 = 2.0;

/// `true` if `ty` is a section type that would carry an economic event.
///
/// It is used while scanning the sections to collect only the candidates
/// (~170 instances in the whole game) without retaining the rest.
pub(crate) const fn is_event_type(ty: u16) -> bool {
    matches!(ty, SEC_KILL | SEC_ASSIST | SEC_PLATE | SEC_TURRET)
}

/// Definitive totals read from the `statsJson`, used as ground truth.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct EventTruth {
    pub kills: u32,
    pub assists: u32,
    pub turret_plates: u32,
    pub turrets_destroyed: u32,
    /// Maximum `LAST_TAKEDOWN_TIME` of the 10 players, in seconds.
    pub last_takedown_s: f32,
}

/// Count of detected events, per type.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct EventCounts {
    pub kills: u32,
    pub assists: u32,
    pub turret_plates: u32,
    pub turrets_destroyed: u32,
}

impl EventCounts {
    /// `true` if the four counts match the truth exactly.
    fn matches(&self, t: &EventTruth) -> bool {
        self.kills == t.kills
            && self.assists == t.assists
            && self.turret_plates == t.turret_plates
            && self.turrets_destroyed == t.turrets_destroyed
    }
}

/// Scans the already parsed sections counting events per type.
///
/// `secs` is a list of `(type, time_s)`. A type is counted only if its length is
/// the expected one for that event (this avoids confusing it with other uses of
/// the same type identifier in a future patch).
pub(crate) fn count_events(secs: &[(u16, usize, f32)]) -> EventCounts {
    let mut c = EventCounts::default();
    for (ty, len, _t) in secs {
        match *ty {
            // The kill records of the validated stream measure 5 bytes.
            SEC_KILL if *len == 5 => c.kills += 1,
            // Assists: 3 bytes.
            SEC_ASSIST if *len == 3 => c.assists += 1,
            // Plates: 1–2 bytes.
            SEC_PLATE if (1..=2).contains(len) => c.turret_plates += 1,
            // Towers: 5 bytes (view 850, fixed length).
            SEC_TURRET if *len == 5 => c.turrets_destroyed += 1,
            _ => {}
        }
    }
    c
}

/// Checks that the last kill falls at `LAST_TAKEDOWN_TIME`.
fn last_takedown_ok(secs: &[(u16, usize, f32)], truth: &EventTruth) -> bool {
    if truth.last_takedown_s <= 0.0 {
        return true; // no data: it does not invalidate by itself
    }
    secs.iter()
        .filter(|(ty, len, _)| *ty == SEC_KILL && *len == 5)
        .map(|(_, _, t)| *t)
        .fold(None, |acc: Option<f32>, t| {
            Some(acc.map_or(t, |a| a.max(t)))
        })
        .map(|last| (last - truth.last_takedown_s).abs() <= LAST_TAKEDOWN_TOLERANCE_S)
        .unwrap_or(false)
}

/// Extracts the verified economic events, or an empty vector if they do not match.
///
/// Returns `(events, report)`. If verification fails, `events` is empty and
/// `report.fuente == "no_disponible"`: events are never exported without having
/// been cross-checked against the `statsJson`.
pub(crate) fn extract_verified_events(
    secs: &[(u16, usize, f32)],
    truth: &EventTruth,
) -> (Vec<GameEvent>, EventsReport) {
    let counts = count_events(secs);
    let verified = counts.matches(truth) && last_takedown_ok(secs, truth);

    let report = EventsReport {
        fuente: if verified {
            "stream_de_eventos".into()
        } else {
            "no_disponible".into()
        },
        kills: counts.kills,
        assists: counts.assists,
        turret_plates: counts.turret_plates,
        turrets_destroyed: counts.turrets_destroyed,
        truth_kills: truth.kills,
        truth_assists: truth.assists,
        truth_turret_plates: truth.turret_plates,
        truth_turrets_destroyed: truth.turrets_destroyed,
        verificado: verified,
        atribucion_equipo: "los registros de evento no codifican jugador ni \
         equipo de forma decodificable: sin esa atribución, partir el oro por \
         equipo sería fabricar datos"
            .into(),
    };

    if !verified {
        return (Vec::new(), report);
    }

    // Verification passed: emit the events in chronological order.
    let mut events: Vec<GameEvent> = Vec::with_capacity(
        (counts.kills + counts.assists + counts.turret_plates + counts.turrets_destroyed) as usize,
    );
    for (ty, len, t) in secs {
        let kind = match (*ty, *len) {
            (SEC_KILL, 5) => Some(EventKind::ChampionKill),
            (SEC_ASSIST, 3) => Some(EventKind::Assist),
            (SEC_PLATE, l) if (1..=2).contains(&l) => Some(EventKind::TurretPlateDestroyed),
            (SEC_TURRET, 5) => Some(EventKind::TurretDestroyed),
            _ => None,
        };
        if let Some(kind) = kind {
            events.push(GameEvent {
                timestamp_ms: (t.max(0.0) * 1000.0).min(u32::MAX as f32) as u32,
                kind,
            });
        }
    }
    events.sort_by_key(|e| e.timestamp_ms);
    (events, report)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// (type, length, time) of a synthetic game that reproduces the real
    /// counts of the reference replay.
    fn synthetic() -> Vec<(u16, usize, f32)> {
        let mut v = Vec::new();
        for i in 0..51 {
            v.push((SEC_KILL, 5, 100.0 + i as f32 * 20.0));
        }
        for i in 0..53 {
            v.push((SEC_ASSIST, 3, 110.0 + i as f32 * 20.0));
        }
        for i in 0..58 {
            v.push((SEC_PLATE, 2, 200.0 + i as f32 * 18.0));
        }
        for i in 0..6 {
            v.push((SEC_TURRET, 5, 700.0 + i as f32 * 100.0));
        }
        // unrelated sections that must be ignored
        v.push((61, 100, 0.0));
        v.push((SEC_KILL, 9, 500.0)); // unexpected length ⇒ not counted
        v.push((SEC_TURRET, 28, 900.0)); // unexpected length ⇒ not counted
        v
    }

    fn truth() -> EventTruth {
        EventTruth {
            kills: 51,
            assists: 53,
            turret_plates: 58,
            turrets_destroyed: 6,
            // The last synthetic kill falls at t = 100 + 50·20 = 1100 s.
            last_takedown_s: 1100.0,
        }
    }

    #[test]
    fn recuentos_validos_verificados() {
        let secs = synthetic();
        let (events, report) = extract_verified_events(&secs, &truth());
        assert!(report.verificado);
        assert_eq!(report.fuente, "stream_de_eventos");
        assert_eq!(events.len(), (51 + 53 + 58 + 6) as usize);
    }

    #[test]
    fn recuento_incoherente_no_emite_eventos() {
        let secs = synthetic();
        let mut t = truth();
        t.kills = 999; // the statsJson does not match what was detected
        let (events, report) = extract_verified_events(&secs, &t);
        assert!(!report.verificado);
        assert_eq!(report.fuente, "no_disponible");
        assert!(events.is_empty(), "no debe exportar eventos sin verificar");
    }

    #[test]
    fn longitud_inesperada_no_se_cuenta() {
        let secs = vec![(SEC_KILL, 9, 10.0), (SEC_ASSIST, 7, 20.0)];
        let c = count_events(&secs);
        assert_eq!(c.kills, 0);
        assert_eq!(c.assists, 0);
    }

    #[test]
    fn orden_cronologico() {
        let secs = synthetic();
        let (events, _) = extract_verified_events(&secs, &truth());
        assert!(events.windows(2).all(|w| w[0].timestamp_ms <= w[1].timestamp_ms));
    }

    #[test]
    fn ultimo_kill_debe_casar_con_last_takedown() {
        let mut secs = synthetic();
        // shift all kills far away from the truth
        for s in secs.iter_mut() {
            if s.0 == SEC_KILL {
                s.2 += 500.0;
            }
        }
        let (events, report) = extract_verified_events(&secs, &truth());
        assert!(!report.verificado);
        assert!(events.is_empty());
    }
}
