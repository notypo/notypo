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
