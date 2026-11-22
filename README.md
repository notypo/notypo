# notypo

**Mistype a command? Type `typo` or `t`.**

A Rust port of [thefuck](https://github.com/nvbn/thefuck) with native completions for npm, Go, Terraform, and other CLIs, precise edits, and safer execution. Runs on macOS, Linux, Windows, and FreeBSD.

## Beyond thefuck

| Capability | notypo | thefuck |
|---|---|---|
| Corrections | Completions, help, man pages, history, and rules | Rules and error output |
| Edits | Parses, edits, reparses | Rule-specific rewrites |
| Inspection | `--explain`, `--json` | `--debug` logs |
| Risky fixes | Always confirmed | `-y` skips confirmation |

## Quick start

Rust 1.98+:

```sh
cargo install --path .
eval "$(notypo --alias)"
```

Add it to `~/.bashrc` or `~/.zshrc`. `typo`, `fuck`, and `t` all work:

```sh
git sttus
typo        # suggests git status; press Enter to run it
t           # shortest alias
```

Fish: `notypo --alias | source`. PowerShell: ``Invoke-Expression ((notypo --alias) -join "`n")``.

## Examples

| You typed | Suggested |
|---|---|
| `env RUST_LOG=debug cargo biuld --color=alwyas` | `env RUST_LOG=debug cargo build --color=always` |
| `kubectl get pods --ouptut=jsno` | `kubectl get pods --output=json` |
| `git status --shrot -- 'src/**/*.rs' \| head -n 5` | `git status --short -- 'src/**/*.rs' \| head -n 5` |
| `if test -d .git; then git status --shrot; fi` | `if test -d .git; then git status --short; fi` |

Tools must be installed. Apps and workspaces are trusted by default.

Inspect:

```sh
notypo -y 'git sttus'          # print fix
notypo --explain 'git sttus'   # evidence
notypo --json 'git sttus'      # JSON
```

Settings: `~/.config/thefuck/settings.py`. Trust is on by default; set `trusted_completers`, `trusted_help`, or `trusted_workspaces` to `[]` for explicit entries. More: `notypo --help`.

## License

MIT or Apache-2.0. Created by Vladimir Iakovlev; see [NOTICE](NOTICE).
