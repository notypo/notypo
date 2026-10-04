# Python versus Rust benchmark

Measured 2026-10-04T12:41:24.227893+00:00 on Apple M1 (arm64, macOS-27.0.1-arm64-arm-64bit-Mach-O).
Original local thefuck 3.32; Python 3.14.8; rustc 1.98.1 (48a229cea 2026-09-01); Rust release build with thin LTO.

## End-to-end CLI

100 fresh processes per cell after 5 warmups. Medians in milliseconds; p95 in parentheses.
Case and implementation order are shuffled each round. Both programs receive identical PATH, Bash history, settings and command text.
Default rules are enabled. Every correction is checked against the expected output; corrections are printed, never executed.
Captured cases read the same preallocated 1 MiB terminal log. Rerun cases include obtaining failed-command output.
Filesystem and bytecode caches are warm. Rust's PATH cache is enabled except in the explicitly labelled case.

| Case | Python ms (p95) | Rust ms (p95) | Speedup |
|---|---:|---:|---:|
| help_startup | 136.94 (140.45) | 4.03 (4.24) | 34.0× |
| cd_parent_captured | 177.48 (184.43) | 6.08 (6.34) | 29.2× |
| cd_parent_rerun | 179.76 (185.93) | 4.62 (4.85) | 38.9× |
| mkdir_p_captured | 178.29 (184.57) | 6.38 (6.62) | 27.9× |
| mkdir_p_rerun | 181.78 (191.20) | 6.28 (6.50) | 29.0× |
| git_not_command_captured | 178.41 (188.33) | 6.55 (6.86) | 27.2× |
| git_not_command_rerun | 189.02 (199.51) | 13.94 (14.94) | 13.6× |
| sudo_captured | 179.12 (191.01) | 7.18 (7.38) | 25.0× |
| no_command_captured | 182.32 (189.04) | 7.96 (8.23) | 22.9× |
| no_command_rerun | 184.06 (193.51) | 6.39 (6.77) | 28.8× |
| no_command_rerun_no_path_cache | 184.47 (191.75) | 8.65 (8.92) | 21.3× |

## Warm engine

Medians in microseconds per correction. Five common rules are preloaded in both implementations; executable candidates and history are injected identically.
Each iteration creates a fresh Command and requests the first correction. Rule discovery/imports, process startup, log parsing and command reruns are excluded.
15 batches per cell, calibrated to roughly 30 ms each. Candidate list contains 2,000 names. These are a focused microbenchmark, not full CLI timings.

| Case | Python µs | Rust µs | Speedup |
|---|---:|---:|---:|
| cd_parent | 3.39 | 0.14 | 23.9× |
| mkdir_p | 18.28 | 0.56 | 32.6× |
| git_not_command | 28.77 | 1.19 | 24.2× |
| sudo | 24.10 | 0.39 | 62.4× |
| no_command | 689.97 | 67.17 | 10.3× |

## Memory

Median peak resident memory for cd_parent_rerun over five separate `/usr/bin/time -l` runs: Python 40.58 MiB; Rust 3.84 MiB.
This measures the correction process, not aggregate memory of its child processes.

Results describe warm local macOS runs, not cold disk startup or other platforms. CLI gains include native startup, rule loading, PATH handling, terminal parsing and rerun optimizations; they do not isolate language execution speed.
Raw samples, dependency versions and source/binary fingerprints are in results.json.
