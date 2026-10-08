# Python versus Rust benchmark

Measured 2026-10-06T16:33:01.700837+00:00 on arm (arm64, macOS-27.0.1-arm64-arm-64bit-Mach-O).
Original local thefuck 3.32; Python 3.14.8; rustc 1.98.1 (48a229cea 2026-09-01); Rust release build with thin LTO.

## End-to-end CLI

20 fresh processes per cell after 3 warmups. Medians in milliseconds; p95 in parentheses.
Case and implementation order are shuffled each round. Both programs receive identical PATH, Bash history, settings and command text.
Default rules are enabled. Every correction is checked against the expected output; corrections are printed, never executed.
Captured cases read the same preallocated 1 MiB terminal log. Rerun cases include obtaining failed-command output.
Filesystem and bytecode caches are warm. Rust's PATH cache is enabled except in the explicitly labelled case.

| Case | Python ms (p95) | Rust ms (p95) | Speedup |
|---|---:|---:|---:|
| help_startup | 137.41 (147.14) | 4.21 (5.85) | 32.6× |
| cd_parent_captured | 253.16 (259.80) | 20.62 (21.01) | 12.3× |
| cd_parent_rerun | 254.52 (295.79) | 19.20 (21.44) | 13.3× |
| mkdir_p_captured | 279.28 (313.04) | 20.37 (20.66) | 13.7× |
| mkdir_p_rerun | 282.38 (290.22) | 20.32 (60.65) | 13.9× |
| git_not_command_captured | 266.34 (323.04) | 30.52 (33.49) | 8.7× |
| git_not_command_rerun | 277.91 (316.27) | 39.56 (40.61) | 7.0× |
| no_command_captured | 299.71 (309.99) | 68.09 (91.23) | 4.4× |
| no_command_rerun | 301.73 (308.97) | 66.66 (69.19) | 4.5× |
| no_command_rerun_no_path_cache | 301.71 (324.48) | 75.77 (76.71) | 4.0× |

## Warm engine

Medians in microseconds per correction. Five common rules are preloaded in both implementations; executable candidates and history are injected identically.
Each iteration creates a fresh Command and requests the first correction. Rule discovery/imports, process startup, log parsing and command reruns are excluded.
5 batches per cell, calibrated to roughly 30 ms each. Candidate list contains 2,000 names. These are a focused microbenchmark, not full CLI timings.

| Case | Python µs | Rust µs | Speedup |
|---|---:|---:|---:|
| cd_parent | 318.53 | 0.14 | 2294.4× |
| mkdir_p | 971.88 | 0.58 | 1664.8× |
| git_not_command | 668.42 | 1.18 | 568.5× |
| sudo | 1298.11 | 0.38 | 3456.4× |
| no_command | 2298.79 | 67.13 | 34.2× |

## Memory

Median peak resident memory for cd_parent_rerun over five separate `wait4` samples: Python 40.72 MiB; Rust 8.66 MiB.
RSS is the process usage reported by the OS; child accounting depends on the OS. This is not aggregate process-tree memory.

Results describe warm local macOS runs, not cold disk startup or other platforms. CLI gains include native startup, rule loading, PATH handling, terminal parsing and rerun optimizations; they do not isolate language execution speed.
Raw samples, dependency versions and source/binary fingerprints are in current-comparison.json.
