# notypo

`notypo` is a crossplatform Rust port of [thefuck](https://github.com/nvbn/thefuck). It suggests fixes for failed shell commands and can run the selected correction.
Mac/Linux/Windows/FreeBSD supported. x64 and ARM

## Shell setup

Install the binary, then add this line to your shell startup file (`~/.zshrc`, `~/.bashrc`, or equivalent):

```sh
eval "$(notypo --alias)"
```

Restart your shell or source the startup file. This installs both `fuck` and `typo`; after a command fails, use either to see suggested corrections. To install only a custom name, use `notypo --alias <name>` instead.

To try a local build in your current Bash or zsh session:

```sh
cargo build --release
eval "$(./target/release/notypo --alias)"
git sttus
typo
```

Press Enter to run the suggested `git status` command. The generated shell function passes your previous command to `notypo` and executes the correction after confirmation.

Running `./target/release/notypo` directly without arguments prints usage: the binary cannot read your shell's in-memory history. To correct an explicit command without shell setup, run:

```sh
./target/release/notypo -y 'git sttus'
```

This prints `git status`; the standalone binary does not execute the printed correction.

## Performance

`notypo` was **14–39× faster** than the original Python `thefuck` in these CLI correction benchmarks. Measured on an Apple M1 running macOS on October 4, 2026, using `thefuck` 3.32 with Python 3.14.8 and a Rust 1.98.1 release build with thin LTO. Each case used 100 fresh processes per implementation after five warmups, with warm filesystem caches and the Rust PATH cache enabled.

Median CLI times, including startup, default rule loading, failed-command output acquisition and printing the first correction:

| Command | Python | Rust | Speedup |
|---|---:|---:|---:|
| `cd..` | 179.8 ms | 4.6 ms | 38.9× |
| `mkdir missing/child` | 181.8 ms | 6.3 ms | 29.0× |
| `git sttus` | 189.0 ms | 13.9 ms | 13.6× |
| `gti status` | 184.1 ms | 6.4 ms | 28.8× |

Both implementations produced matching corrections. A separate warm engine benchmark using five preloaded rules and 2,000 executable candidates showed **10–62× speedups**, excluding startup, rule imports, log parsing and command reruns.

Median peak resident memory for the `cd..` correction was **40.6 MiB for Python versus 3.8 MiB for Rust**, measured over five runs. These results describe warm local macOS runs; performance on other platforms and with cold caches may differ.

See the [full report](benchmarks/results.md), [raw samples](benchmarks/results.json) and [benchmark setup](benchmarks/README.md). After setup, rerun the comparison with:

```sh
python3 benchmarks/compare.py
```

## License

Licensed under either MIT or APACHE 2.0, at your option.

## Attribution

The original [thefuck](https://github.com/nvbn/thefuck) was created by Vladimir Iakovlev. See [NOTICE](NOTICE)
