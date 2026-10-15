# notypo

`notypo` suggests corrections for failed shell commands. It began as a Rust port of [thefuck](https://github.com/nvbn/thefuck) and adds completion-driven diagnosis, safer execution, and structured reports. Supports macOS, Linux, Windows, and FreeBSD.

## Setup

Build and add the shell integration to `~/.bashrc` or `~/.zshrc`:

```sh
cargo build --release
eval "$(./target/release/notypo --alias)"
```

In fish, add `notypo --alias | source` to `~/.config/fish/config.fish`. Then run a failed command and enter `fuck` or `typo` to review its correction. Press Enter to run the selected command.

In PowerShell, add ``Invoke-Expression ((notypo --alias) -join "`n")`` to your profile. In tcsh, use `` eval `notypo --alias` ``. Each integration captures the preceding command and its status before collecting completion metadata.

To correct an explicit command without shell integration:

```sh
notypo -y 'git sttus'
```

This prints `git status`; it does not execute it. Use `--explain` for a readable diagnosis or `--json` for a machine-readable report.

## Features beyond thefuck

- **Native completion:** asks installed CLIs and shell completion handlers for valid commands, options, and values. Supports built-in bridges (including Node.js's own option list) and argcomplete, Cobra, Posener, urfave/cli, kingpin, go-flags, yargs, click, generated clap, pip, npm, Cargo, and .NET SDK protocols, and oclif manifests. Installed handlers and generic protocols require `trusted_completers`. Bash function, word-list, and external completer registrations are supported. For fish users, the completion scripts that fish 4 embeds in its binary count as installed handlers. Completion functions your bash, zsh, or fish session defined itself (inline in a startup file, or with `eval "$(tool completion bash)"`, `source <(tool completion zsh)`, or `tool completion fish | source`) are passed by the shell function for the previous command's programs and preferred over installed files; notypo evaluates only those definitions, never the startup file, and still requires `trusted_completers`.
- **More evidence:** checks help pages, man pages, executable names, paths, shell history, and captured error hints. It can repair nested commands, option values, attached values, and commands inside supported compound shell syntax.
- **Safer corrections:** preserves shell syntax while editing only mistaken words; never reruns the failed command by default. Risky operations, including destructive commands, package changes, disk operations, database resets, and changed write targets, require confirmation even with `-y`.
- **Inspection and automation:** `--explain` and `--json` report evidence and safety decisions without running a command.
- **Shell integration:** passes failure and pipeline statuses, supports multiline commands, and handles history across supported shells.

## Settings

Settings use thefuck's compatible location, `~/.config/thefuck/settings.py` (or legacy `~/.thefuck/settings.py`), and can also be set with `NOTYPO_*` environment variables.

| Setting | Default | Purpose |
|---|---:|---|
| `trusted_completers` | `[]` | Allow generic CLI protocols and installed completion handlers. |
| `trusted_help` | `[]` | Allow selected programs to provide help and legacy metadata listings. |
| `trusted_workspaces` | `[]` | Directories where programs may evaluate project files to list their commands; `*` trusts every directory. |
| `disabled_sources` | `[]` | Disable evidence sources such as `native`, `help`, or `history`. |
| `probe_timeout` | `3` seconds | Limit each completion or help probe. |
| `network_completion` | `False` | Allow read-only resource lookups using your credentials. |
| `replay_for_diagnosis` | `False` | Allow safe, selected commands to be rerun to capture output. |

See `notypo --help` for command-line options.

Completion handlers execute code. The trust settings authorize that code to run during discovery. Probe timeouts and the offline environment limit discovery, but do not provide an operating system sandbox. Native and legacy discovery share one request budget. Legacy listings require `trusted_help`, with an audited exception for local Git branch names; project task listings also require workspace trust.

## Shell syntax

The engine edits source spans and reparses each correction, preserving unchanged whitespace, quotes, comments, and redirections. Bash, Zsh, and POSIX shells support simple commands, pipelines, lists, and supported compound commands. Fish uses its own parser and quoting, including supported blocks. tcsh supports simple commands and operators; history substitutions that could be reinterpreted are refused. The generic integration uses the POSIX subset and needs a shell with `fc` to read the preceding command.

PowerShell supports literal commands in argument mode, parameter names and values, aliases, pipelines, and redirections. Expressions and statement syntax remain unsupported. Here-documents, Bash arithmetic and `[[ ... ]]`, and Zsh forms outside the POSIX subset are also reported as unsupported. Substitutions and expansions are never evaluated during parsing or discovery.

PowerShell session completers can retain bounded primitive closure and module state, including arrays, dictionaries, and private helper functions. Live objects and oversized snapshots are unsupported. Zsh completion results retain descriptions and literal prefixes/suffixes; insertion text that requires evaluating shell code is rejected.

## License

MIT or Apache-2.0. The original app was created by Vladimir Iakovlev; see [NOTICE](NOTICE).
