# Correction benchmark with installed shell handlers

11 processes per mode; times in ms (median / p95). Cold runs
start with an empty cache directory; warm runs reuse one. Probes are
completer subprocesses (cold / warm). RSS uses the process usage returned
by wait4; child accounting depends on the OS. In-process
stage timings: `cargo bench --bench engine`.

| Case | Correction | Cold | Warm | Probes | Peak RSS |
|---|---|---|---|---|---|
| fish shell handler | `notypo-shell-fixture nodes lsit --foramt jsno` → `notypo-shell-fixture nodes list --format json` | 42 / 43 | 43 / 126 | 4 / 4 | 5.8 MiB |
| zsh shell handler | `notypo-shell-fixture nodes lsit --foramt jsno` → `notypo-shell-fixture nodes list --format json` | 587 / 617 | 600 / 633 | 4 / 4 | 5.2 MiB |

Isolated fixture apps use the installed shells' real native completion
handlers. Each correction fixes a nested command, option, and enum value.
The harness checks every result and an operation marker; suggestions are
never executed. This measures shell-handler overhead, not a cloud CLI.

Handwritten handlers provide partial evidence and use request-local memoization.
Warm runs reuse the cache directory but repeat all four native probes.
Zsh's completion-system initialization dominates its elapsed time.

Measured on macOS on 2026-10-04 with Zsh 5.9, fish 4.9.3, and a release
build using Rust 1.98.1. p95 uses the nearest-rank definition; with eleven
samples, it is the maximum. The fish warm p95 includes one slow sample.

```sh
cargo build --locked --release
python3 benchmarks/structured.py --shell-fixtures --case shell --samples 11 --output benchmarks/shell-results.md
```
