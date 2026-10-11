# Comparing the Python and Rust implementations

The original Python source must be available in `thefuck/`. The harness checks
matching corrections before recording any timings and does not execute the
suggested corrections. Runtime files, configuration and history stay in
`target/benchmark-work/`.

```sh
python3 -m venv target/bench-python
target/bench-python/bin/python -m pip install -r benchmarks/python-requirements.txt
cargo build --release
cargo build --release --bench correction
python3 benchmarks/compare.py
```

The default run measures 100 fresh CLI processes per implementation per case,
then 15 batches per implementation per warm engine case. Order is randomized
with a fixed seed. For a quick smoke run:

```sh
python3 benchmarks/compare.py --samples 3 --warmups 1 --engine-samples 2 --output target/benchmark-smoke.json
```

`results.md` contains the comparison; `results.json` contains every sample,
environment versions and source/binary fingerprints. CLI times include startup,
default rule discovery, output acquisition and selecting/printing the first
correction. Warm engine times use five common preloaded rules and a fixed 2,000
candidate list; they exclude startup and output acquisition. The engine harness
can also be run on its own with `cargo bench --bench correction`.

PATH caches and filesystem caches are warmed. A separate CLI case disables the
Rust PATH cache. Recorded-output fixtures use the same 1 MiB log in both programs.
Peak RSS is collected on macOS. No dependencies are installed globally.
The comparison uses `wait4` for RSS, so restricted macOS environments do
not need `/usr/bin/time -l` task inspection. Setuptools supplies upstream
thefuck's `distutils` compatibility on Python 3.14.

To measure the structured pipeline with installed fish/Zsh completion handlers,
use isolated fixture apps. This checks the expected corrections and confirms
their operations were never executed. Missing shells are skipped.

```sh
cargo build --locked --release
python3 benchmarks/structured.py --shell-fixtures --case shell --samples 11 --output benchmarks/shell-results.md
```

[shell-results.md](shell-results.md) records cold/warm latency, native probe
counts, and RSS. Partial shell handlers keep answers within one request, so
reusing a disk cache directory does not reduce their probe count.

The newer protocol bridges (pip, npm, Cargo, .NET SDK, clap, urfave/cli, user-trusted
cobra, and a bash handler) are measured against the apps installed on this
machine, each with the trust it needs. Missing apps are skipped, and every
suggestion is checked without being run. PowerShell cases run when `pwsh` is
on `PATH` or `NOTYPO_BENCH_PWSH` names a PowerShell executable; they measure
a misspelled parameter, a misspelled cmdlet name, and, when mdbook or rustup
is installed, its generated PowerShell completer registered in a session.
They also measure a captured local function's named and positional callbacks,
including a named argument after the positional slot. PowerShell's static
binder selects that slot's parameter before its completion binder queries the
callback. Use `--case 'PowerShell positional'` to measure that case alone.

```sh
cargo build --locked --release
python3 benchmarks/structured.py --protocols --case protocol --samples 11 --output benchmarks/protocol-results.md
```

[protocol-results.md](protocol-results.md) records the results. These bridges
keep answers within one request, so warm runs repeat their probes.

Archive member correction uses installed macOS tar, GNU tar, unzip, and 7-Zip
on temporary local archives. The harness verifies every correction and checks
that no extraction directory appeared. Listings repeat on warm requests.
Results are recorded in [archive-results.md](archive-results.md).

```sh
python3 benchmarks/structured.py --archive-fixtures --case 'archive members' --samples 15 --output benchmarks/archive-results.md
```

The refreshed macOS comparison after the structured-engine changes is in
[current-comparison.md](current-comparison.md), with raw samples and binary/
upstream-source fingerprints in current-comparison.json. It uses 20 fresh
processes per cell and checks every correction against the same expected
result. Its ratios apply to those local fixtures; native app startup and
overall correction accuracy are separate questions.
