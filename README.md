# notypo

**Mistype a command? Type `typo`.**

A Rust port of [thefuck](https://github.com/nvbn/thefuck) that goes further: native completions, precise edits inside complex shell commands, and safer execution. Runs on macOS, Linux, Windows, and FreeBSD.

## Beyond thefuck

| Capability | notypo | thefuck |
|---|---|---|
| Finding corrections | CLI/shell completions, help, history, and rules | [Rules and error output](https://github.com/nvbn/thefuck#how-it-works) |
| Shell edits | Parses, edits mistaken words, then reparses | [Rule-specific rewrites](https://github.com/nvbn/thefuck/blob/master/thefuck/types.py) |
| Inspection | `--explain`; `--json` with evidence and safety decisions | [`--debug` logs](https://github.com/nvbn/thefuck/blob/master/thefuck/argument_parser.py) |
| Risky corrections | Confirmation required even with `-y` | [`-y` skips confirmation](https://github.com/nvbn/thefuck#installation) |
| Replaying failed commands | Off by default | [Reruns to capture output unless logging is enabled](https://github.com/nvbn/thefuck/blob/master/thefuck/output_readers/__init__.py) |

## Quick start

From this repository, with Rust 1.98+ (Bash or Zsh):

```sh
cargo install --path .
eval "$(notypo --alias)"
```

Add the `eval` line to `~/.bashrc` or `~/.zshrc` to keep it. Both `typo` and `fuck` work:

```sh
git sttus
typo        # suggests git status; press Enter to run it
```

For fish, use `notypo --alias | source`. For PowerShell, use ``Invoke-Expression ((notypo --alias) -join "`n")``.

## Examples

| You typed | Suggested correction |
|---|---|
| `env RUST_LOG=debug cargo biuld --color=alwyas` | `env RUST_LOG=debug cargo build --color=always` |
| `kubectl get pods --ouptut=jsno` | `kubectl get pods --output=json` |
| `cargo build -p notyop --release` | `cargo build -p notypo --release` (in this repo) |
| `git status --shrot -- 'src/**/*.rs' \| head -n 5` | `git status --short -- 'src/**/*.rs' \| head -n 5` |
| `if test -d .git; then git status --shrot; fi` | `if test -d .git; then git status --short; fi` |

Tools must be installed. Cargo examples need `cargo` in `trusted_completers` and `trusted_help`; package names also need this repository in `trusted_workspaces`.

Inspect a command directly:

```sh
notypo -y 'git sttus'          # print the correction
notypo --explain 'git sttus'   # show the evidence
notypo --json 'git sttus'      # get a JSON report
```

Direct calls print results; the shell functions run your selected correction after confirmation.

Settings: `~/.config/thefuck/settings.py`. Generic and shell completers require `trusted_completers`; help probes require `trusted_help`. Only trust tools you would run yourself. More options: `notypo --help`.

## License

MIT or Apache-2.0
The original app was created by Vladimir Iakovlev; see [NOTICE](NOTICE).
