# notypo

`notypo` suggests corrections for failed shell commands. It began as a Rust port of [thefuck](https://github.com/nvbn/thefuck) and adds completion-driven diagnosis, safer execution, and structured reports. Supports macOS, Linux, Windows, and FreeBSD.

## Setup

Build and add the shell integration to `~/.bashrc` or `~/.zshrc`:

```sh
cargo build --release
eval "$(./target/release/notypo --alias)"
```

In fish, add `notypo --alias | source` to `~/.config/fish/config.fish`. Then run a failed command and enter `fuck` or `typo` to review its correction. Press Enter to run the selected command.

To correct an explicit command without shell integration:

```sh
notypo -y 'git sttus'
```

This prints `git status`; it does not execute it. Use `--explain` for a readable diagnosis or `--json` for a machine-readable report.

## Features beyond thefuck

- **Native completion:** asks installed CLIs and shell completion handlers for valid commands, options, and values. Supports built-in bridges (including Node.js's own option list) and argcomplete, Cobra, Posener, urfave/cli, kingpin, go-flags, yargs, click, generated clap, pip, npm, Cargo, and .NET SDK protocols, and oclif manifests. Installed handlers and generic protocols require `trusted_completers`. For fish users, the completion scripts that fish 4 embeds in its binary count as installed handlers. Completion functions your bash, zsh, or fish session defined itself (inline in a startup file, or with `eval "$(tool completion bash)"`, `source <(tool completion zsh)`, or `tool completion fish | source`) are passed by the shell function for the previous command's programs and preferred over installed files; notypo evaluates only those definitions, never the startup file, and still requires `trusted_completers`.
- **More evidence:** checks help pages, man pages, executable names, paths, shell history, and captured error hints. It can repair nested commands, option values, attached values, and commands inside supported compound shell syntax.
- **Safer corrections:** preserves shell syntax while editing only mistaken words; never reruns the failed command by default. Risky operations, including destructive commands, package changes, disk operations, database resets, and changed write targets, require confirmation even with `-y`.
- **Inspection and automation:** `--explain` and `--json` report evidence and safety decisions without running a command.
- **Shell integration:** passes failure and pipeline statuses, supports multiline commands, and handles history across supported shells.

## Settings

Settings use thefuck's compatible location, `~/.config/thefuck/settings.py` (or legacy `~/.thefuck/settings.py`), and can also be set with `NOTYPO_*` environment variables.

| Setting | Default | Purpose |
|---|---:|---|
| `trusted_completers` | `[]` | Allow generic CLI protocols and installed completion handlers. |
| `trusted_help` | `[]` | Allow selected programs to be run with `--help`. |
| `trusted_workspaces` | `[]` | Directories where programs may evaluate project files to list their commands; `*` trusts every directory. |
| `disabled_sources` | `[]` | Disable evidence sources such as `native`, `help`, or `history`. |
| `probe_timeout` | `3` seconds | Limit each completion or help probe. |
| `network_completion` | `False` | Allow read-only resource lookups using your credentials. |
| `replay_for_diagnosis` | `False` | Allow safe, selected commands to be rerun to capture output. |

See `notypo --help` for command-line options.

## Performance

In local macOS benchmarks, notypo was 6–33× faster end to end than Python thefuck 3.32 and used about 6× less memory. Native CLI completion adds the startup time of the installed app. Details: [benchmark results](benchmarks/results.md) and [completion measurements](benchmarks/structured-results.md).

## License

MIT or Apache-2.0. The original app was created by Vladimir Iakovlev; see [NOTICE](NOTICE).
