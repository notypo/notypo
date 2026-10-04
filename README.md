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

`notypo` was **14–39× faster** than the original Python `thefuck` in these CLI correction benchmarks.

## License

Licensed under either MIT or APACHE 2.0, at your option.

## Attribution

The original [thefuck](https://github.com/nvbn/thefuck) was created by Vladimir Iakovlev. See [NOTICE](NOTICE)
