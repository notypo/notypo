# Repository Guidelines

## Project Structure & Module Organization

`notypo` is a Rust 2024 command correction CLI requiring Rust 1.98 or newer.

- `src/main.rs` starts the binary; `src/lib.rs` exposes the library. Core modules handle arguments, settings, shells, correction selection, and terminal interaction.
- `src/rules/` groups correction rules by tool category; `src/specific/` contains shared tool helpers. Platform implementations live under `src/platform/` and `src/terminal/`.
- `tests/rules.rs` uses fixtures in `tests/data/rules.rs`; `tests/cli.rs` exercises Unix CLI and shell behavior. Unit tests live beside implementation code.
- `benches/correction.rs` benchmarks the engine; `benchmarks/` contains Python comparison scripts, fixtures, and reports. The ignored `thefuck/` directory supplies upstream reference source.

## Build, Test, and Development Commands

- `cargo build --locked` — build with the committed dependency versions; add `--release` for optimized binaries.
- `cargo run -- -y 'git sttus'` — print a correction for an explicit command.
- `cargo test --locked --all-targets` — run unit tests, integration tests, and benchmark fixture checks.
- `cargo fmt --all -- --check` — check formatting; use `cargo fmt --all` to apply it.
- `cargo clippy --locked --all-targets --all-features -- -D warnings` — run the CI lint checks.
- `cargo bench --bench correction` — benchmark the correction engine. See `benchmarks/README.md` for Python comparisons.

## Coding Style & Naming Conventions

Use standard rustfmt formatting with four-space indentation. Use `snake_case` for modules, functions, and rule identifiers; `PascalCase` for types; and `SCREAMING_SNAKE_CASE` for constants. Preserve upstream rule names and metadata for configuration compatibility. Add rules to the appropriate category registry. Guard platform-specific code with the existing conditional compilation patterns.

## Testing Guidelines

Use Rust's built-in `#[test]` framework and descriptive behavior-based names. Add regression fixtures for rule changes, including nonmatching and malformed input cases. Update the registry completeness assertion when adding rules. Isolate CLI tests with temporary configuration, cache, and working directories. No numeric coverage threshold is configured. CI tests or compiles targets across Linux, macOS, Windows, and FreeBSD.

## Commit & Pull Request Guidelines

This checkout has no Git metadata to establish historical conventions. Use concise imperative subjects, such as `Fix zsh multiline history selection`. Keep changes focused. PR descriptions should explain the affected command or shell, expected correction, regression coverage, validation commands, and platform impact; link relevant issues.
