# Reverse engineering notes

Forensic detail of the `.rofl` formats. These notes back the claims in the
[README](../README.md) and explain what is measured, what is decoded and what
is deliberately left out instead of being invented.

## Modern format (reverse-engineering summary)

```text
[RIOT][u16 ver=2][u64 gameId][.NET string gameVersion]
[segment]*   each one: [u32 keyframe][u32 chunk][u8 type][u32 uncomp][u32 comp] + data
              comp = 0 ⇒ data stored uncompressed; comp > 0 ⇒ Zstandard frame
[footer JSON] {"gameLength":ms,"lastGameChunkId":…,"statsJson":"[ …10 players… ]"} + signature
```

Inside each segment, the data is a stream of sections
`[u8 H][time][len][type u16][params][data]` (with type inheritance and short
lengths according to the bits of `H`). The parser validates that the sections
fill each segment exactly and that the final time matches the
footer duration. In a real 9.7 MB replay: 71 segments and 1,157,983
sections parsed without errors.

## Status of the modern format reverse engineering

1,157,983 sections have been parsed empirically from a real replay
(16.20.824.8524, 1352.3 s) and several hypotheses have been tested. Result:

| Section | Hypothesis tested | Result |
|---|---|---|
| **61** (positions, ~23 Hz) | Mask in `tail[3]`: `0x0B`(3), `0x80`(1), `0xC8`(3), `0xE8`(4 fields) | **Solved (experimental)**: layout with mask E8, fields at bits 13 and 30 of the `tail[4..]` block (14 bits each, wrap +8192). Passes the physical gate (team separation 3748 u, 6+5 champions at fountains, high variance, 100 % continuity). Emits `positions` (10 champions, ~876–1191 samples). |
| **61** | CS/kills/assists as integer fields in the champion record | **Negative** (direct search against the final totals of `statsJson`). |
| **723** (380,778 × 2 B, ~282 Hz) | `(dx,dy)` i8 or absolute `u16` per entity | **Discarded**: it only alternates two values (`45 9A`/`43 …`) ⇒ a flags/tick stream. |
| **All types** (cross-scan) | Gold in any section | **Definitive negative**: scan of 251 section types, aligned and unaligned u32/u16/f32, exact ±5 and range [4000,13000]. Only type 769 has 21% in range (no exact hits). **Gold is not in the replay as a readable numeric field** — only the final totals in `statsJson`. |
| **1140** (entity stats) | Same grammar as 61 | Open: it contains champion anchors, variable structure (bitmask). |
| **166 / 131 / 1183 / 850** (events) | Game event streams | **Solved and verified**: kills (51), assists (53), plates (58) and turrets (6) with exact timestamps. |
| **Wards (all paths)** | Ward events (placement + destruction) with `(t, x, y, owner)` in the modern format | **Definitive negative** (RE campaign v84–v90): no section with count 74/9, they are not entities of stream 61 (only 10 champions + 6 structures, verified over 30,995 records and 23 keyframes), nor do they carry their own coordinates (f32/14-bit sweep: the only f32s are `44 44 44 44` padding). `statsJson` provides **only totals** (74/9, per player). In modern `.rofl` spatial vision is left **documented as omitted**. |

## Economic events (solved and verified)

The modern-format stream **does** carry game events with their exact
timestamp. Each event type is detected by `(section type,
length)`:

| Type | Event | Records | Length | Validation |
|---|---|---|---|---|
| **166** | Champion kill | 51 | fixed 5 B | `CHAMPIONS_KILLED` = 51 |
| **131** | Assist | 53 | fixed 3 B | `ASSISTS` = 53 |
| **1183** | Turret plate | 58 | 1–2 B | `Missions_TurretPlatesDestroyed` = 58 |
| **850** | Turret destroyed | 6 | fixed 5 B | `TURRETS_KILLED` = 6 |

Additional corroboration:

- Types **166 / 421 / 563 / 1166** are **four redundant views** of the
  same stream of 51 kills: their timestamps are **bit-for-bit identical**
  (max|Δt| = 0.0000 s).
- Spacing is **irregular** (CV = 1.30), not periodic ⇒ they are real
  events, not snapshots.
- The last kill falls at **t = 1337.441 s**, and `statsJson` publishes
  max `LAST_TAKEDOWN_TIME` = **1337 s** (rounded to seconds). It matches.
- Types **232** and **850** share the same 6 timestamps; 850 is used because
  it has a fixed length.

**Verification is dual**: the four counts must match the `statsJson`
*exactly* **and** the last kill must fall at
`LAST_TAKEDOWN_TIME`. If anything does not add up, no event is emitted
(`events_report.fuente = "no_disponible"`), so as not to export false events.

## Wards in the modern format (definitive negative, RE v84–v90)

The coach's requirement was the ward map: **distributing over time the
spatial distribution of wards alive at minute X + the face of the champion that
placed it, along with their destructions**. In the modern format this **cannot be
extracted**, and it is documented as an omission (it is never filled with zeros nor
is a position invented):

| Path tested | What was looked for | Result |
|---|---|---|
| Section with count = 74 (placements) or 9 (kills) | Census of the ~150 types in the stream | **None exists**: no type with n=74 (nor 73/75). The n=9 ones (722, 984, 151) are other events: 722/866 are **exact 60 s periodic** (buffs), 984 clusters at the start, 151 are kill side-events. |
| Wards as **entities** in the position stream (type 61) | New entities with their own ID, stationary position, ~90–180 s lifetime | **They are not entities**: the whole stream (30,995 records) contains only **16 IDs** = 10 champions + 6 structures (turrets/nexus, already present at t=0). |
| Wards in the **keyframes** (23 full snapshots, every 60 s) | Additional live entities mid-game | **No**: each keyframe lists 11–19 entities (champions + structures). |
| **Coordinate sweep** (definitive test) | Any `(x,y)` in map range with f32 encoding (like legacy) or 14-bit | **No wards**: the "plausible" f32s are **padding** (`44 44 44 44` = 785.0), not positions. |
| `statsJson` | Ward totals | **Only scalar totals**: 74 placed / 9 destroyed, per player and team (`WARD_PLACED`, `WARD_KILLED`). No coordinates or timestamps. |

**Consequence**: in modern `.rofl` **spatial vision is left documented as
omitted**. The vision axis (V) uses the *per-player totals*
from `statsJson` (measured), not the spatial distribution. The `legacy` fallback
(`payload.rs`) **does** extract `WardPlaced{x,y}` and `WardKill{x,y}` with
coordinates, but without an owner.
