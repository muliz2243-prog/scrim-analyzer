# Changelog

All notable changes to Scrim Analyzer are documented here.

## [1.0.1] — 2026-10-28

Rebuilt from the current `main`. No behavior changes: this release only
publishes the binaries of the refactored code base and the updated docs.

### Changed

- The GUI is split into one module per screen and panel (`src/gui/`); the
  code was moved verbatim, so the behavior is identical.
- The README describes the metric as **real gold** from the official LCU
  timeline, and the reverse-engineering detail moved to
  [`docs/REVERSE_ENGINEERING.md`](docs/REVERSE_ENGINEERING.md).
- The live-path test now runs against a sanitized `allgamedata` fixture, so it
  also runs on CI instead of depending on a local, uncommitted capture.

## [1.0.0] — 2026-10-27

First public release.

### Added

- Native Windows desktop app (Rust + egui/eframe) with a dark theme.
- Home screen with **Detect live game**, **Open saved `.json` capture**,
  automatic JSON saving and a folder picker.
- Review-name dialog before capture; the name is shown at the top of the report
  and used for the saved `.json` file (`name (1).json`, `name (2).json`, … on
  conflict).
- 30-second gold windows with real net gold from the LCU timeline.
- Severity alerts (minor, medium, considerable, critical) with split bars:
  left half = severity, right half = the team that held the advantage.
- Best gold windows per team and a category breakdown for every window
  (kills, towers, minions, jungle, extra gold).
- Vision dashboard every 2 minutes, with the best vision windows per team.
- Objective markers on the timeline (dragons, heralds, void grubs, barons,
  inhibitors).
- Analyst notes and JSON export.
- Five UI languages: English (default), Spanish, French, German and Korean.
- Portable ZIP and Inno Setup installer, both published as release assets.

### Fixed

- A newly opened replay no longer ends the capture after three transient API
  failures: the app keeps recording while the LCU reports an active replay.
- Pressing **Detect live game** twice no longer starts two pollers; the running
  capture is only renamed.
- Clearer diagnostics in `debug.log` (poll failures and player count).

### Known limitations

- Per-minute vision requires playing the replay while the app records.
- The LCU timeline does not always include `WARD_PLACED`; in that case only the
  measured totals are shown.
- Some `gameId` values return 404 `match file not found`; the report then falls
  back to the basic `.rofl` analysis.
- Windows only.
