# Contributing

Thanks for taking the time to improve Scrim Analyzer. This project is a native
Rust desktop app for reviewing League of Legends scrims. It has no server, no
overlay and no telemetry.

## Before you start

- Read [`README.md`](README.md) and [`docs/VISUAL_GUIDE.md`](docs/VISUAL_GUIDE.md).
- For a bug, open an issue with the template and attach `debug.log` if possible.
- For a large change, open an issue first so we can agree on the approach.

## Requirements

- Windows 10/11.
- Rust (stable) via [rustup](https://rustup.rs).
- A linker: Visual Studio Build Tools with the C++ workload, or the GNU
  toolchain used by the project.
- League of Legends installed if you want to test the LCU/live paths.

## Build and run

```powershell
# Fast type check
cargo check --features gui

# Working build (GUI)
cargo build --release --features gui
# -> target\release\scrim-analyzer.exe

# CLI only (no GUI feature)
cargo build --release
# -> target\release\rofl-analyzer.exe
```

## Tests

```powershell
cargo test --features gui
```

Some tests use a synthetic `.rofl`; you do not need a real game. The TLS test
starts a local self-signed server.

## Code guidelines

- Comments, doc-comments, log messages and error messages are **English**.
- All user-facing text goes through the i18n dictionaries in
  `assets/i18n/*.json`. Do not hardcode UI strings.
- Keep the five dictionaries in sync (`en`, `es`, `fr`, `de`, `ko`).
- Do not rename serialized fields or historical identifiers (`fuente`, `avisos`,
  `puntos`, …) without a migration: old captures must keep loading.
- Do not commit captures, logs, `config.json`, `.rofl` files or the private
  project notes. See [`.gitignore`](.gitignore).
- Avoid new dependencies unless they are clearly justified.

## Pull requests

1. Fork the repository and create a branch.
2. Keep the change focused; explain the problem and the solution.
3. Run `cargo check --features gui` and `cargo test --features gui`.
4. Update `CHANGELOG.md` for user-visible changes.
5. Open the pull request with screenshots for UI changes.

## License

By contributing you agree that your contribution is licensed under the MIT
License (see [`LICENSE`](LICENSE)).
