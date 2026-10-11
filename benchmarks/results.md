# Python versus Rust benchmark

Measured 2026-10-04T16:20:16.541215+00:00 on Apple M1 (arm64, macOS-27.0.1-arm64-arm-64bit-Mach-O).
Original local thefuck 3.32; Python 3.14.8; rustc 1.98.1 (48a229cea 2026-09-01); Rust release build with thin LTO.

## End-to-end CLI

100 fresh processes per cell after 5 warmups. Medians in milliseconds; p95 in parentheses.
Case and implementation order are shuffled each round. Both programs receive identical PATH, Bash history, settings and command text.
Default rules are enabled. Every correction is checked against the expected output; corrections are printed, never executed.
Captured cases read the same preallocated 1 MiB terminal log. Rerun cases include obtaining failed-command output.
Filesystem and bytecode caches are warm. Rust's PATH cache is enabled except in the explicitly labelled case.

| Case | Python ms (p95) | Rust ms (p95) | Speedup |
|---|---:|---:|---:|
| help_startup | 135.93 (139.89) | 4.09 (4.36) | 33.2× |
| cd_parent_captured | 176.09 (180.98) | 10.25 (10.64) | 17.2× |
| cd_parent_rerun | 178.23 (189.48) | 8.78 (9.46) | 20.3× |
| mkdir_p_captured | 176.36 (182.36) | 10.08 (10.56) | 17.5× |
| mkdir_p_rerun | 180.47 (186.63) | 9.92 (10.35) | 18.2× |
| git_not_command_captured | 176.46 (187.36) | 24.71 (25.74) | 7.1× |
| git_not_command_rerun | 187.56 (192.16) | 31.57 (32.61) | 5.9× |
| no_command_captured | 180.15 (186.84) | 25.61 (26.46) | 7.0× |
| no_command_rerun | 182.93 (195.15) | 24.03 (25.18) | 7.6× |
| no_command_rerun_no_path_cache | 182.91 (194.46) | 27.13 (28.80) | 6.7× |

## Warm engine

Medians in microseconds per correction. Five common rules are preloaded in both implementations; executable candidates and history are injected identically.
Each iteration creates a fresh Command and requests the first correction. Rule discovery/imports, process startup, log parsing and command reruns are excluded.
15 batches per cell, calibrated to roughly 30 ms each. Candidate list contains 2,000 names. These are a focused microbenchmark, not full CLI timings.

| Case | Python µs | Rust µs | Speedup |
|---|---:|---:|---:|
| cd_parent | 3.37 | 0.11 | 29.9× |
| mkdir_p | 17.87 | 0.56 | 31.8× |
| git_not_command | 28.62 | 1.19 | 24.0× |
| sudo | 23.99 | 0.39 | 61.8× |
| no_command | 690.10 | 67.75 | 10.2× |

## Memory

Median peak resident memory for cd_parent_rerun over five separate `/usr/bin/time -l` runs: Python 40.50 MiB; Rust 6.80 MiB.
This measures the correction process, not aggregate memory of its child processes.

Results describe warm local macOS runs, not cold disk startup or other platforms. CLI gains include native startup, rule loading, PATH handling, terminal parsing and rerun optimizations; they do not isolate language execution speed.
Raw samples, dependency versions and source/binary fingerprints are in results.json.
