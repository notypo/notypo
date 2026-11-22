# notypo

**Mistype a command? Type `typo`, `t`, or old-school `fuck`.**

A Rust port of [thefuck](https://github.com/nvbn/thefuck) that goes further: native completions, precise edits inside complex shell commands, and safer execution. Runs on macOS, Linux, Windows, and FreeBSD.

<picture>
  <source srcset="demo/notypo-demo.avif" type="image/avif">
  <source srcset="demo/notypo-demo.webp" type="image/webp">
  <img src="demo/notypo-demo.gif" alt="notypo correcting git sttus with the t alias">
</picture>

## Beyond thefuck

| Capability | notypo | thefuck |
|---|---|---|
| Finding corrections | CLI/shell completions, help, man pages, history, and rules | [Rules and error output](https://github.com/nvbn/thefuck#how-it-works) |
| Shell edits | Parses, edits mistaken words, then reparses | [Rule-specific rewrites](https://github.com/nvbn/thefuck/blob/master/thefuck/types.py) |
| Inspection | `--explain`; `--json` with evidence and safety decisions | [`--debug` logs](https://github.com/nvbn/thefuck/blob/master/thefuck/argument_parser.py) |
| Risky corrections | Confirmation required even with `-y` | [`-y` skips confirmation](https://github.com/nvbn/thefuck#installation) |
| Replaying failed commands | Off by default | [Reruns to capture output unless logging is enabled](https://github.com/nvbn/thefuck/blob/master/thefuck/output_readers/__init__.py) |

## Install

Homebrew (macOS and Linux):

```sh
brew tap notypo/notypo
brew trust --formula notypo/notypo/notypo
brew install notypo
```

Prebuilt binaries for macOS, Linux, Windows, and FreeBSD are on [Releases](https://github.com/notypo/notypo/releases). From source (Rust 1.98+):

```sh
cargo install --git https://github.com/notypo/notypo
```

Then install the shell aliases (Bash/Zsh):

```sh
eval "$(notypo --alias)"
```

Add that line to `~/.bashrc` or `~/.zshrc`. `typo`, `fuck`, and `t` all work:

```sh
git sttus
typo        # suggests git status; press Enter to run it
fuck        # same correction under the familiar name
t           # and the shortest alias
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

Inspect a command directly:

```sh
notypo -y 'git sttus'          # print the correction
notypo --explain 'git sttus'   # show the evidence
notypo --json 'git sttus'      # get a JSON report
```

Direct calls print results; the shell functions run your selected correction after confirmation.

Settings: `~/.config/thefuck/settings.py`. All apps and workspaces are trusted by default, so no configuration is needed. To opt out, set `trusted_completers`, `trusted_help`, or `trusted_workspaces` to `[]`; generic completion, help probes, or project evaluation then require explicit entries again. More options: `notypo --help`.

## License

MIT or Apache-2.0
The original app was created by Vladimir Iakovlev; see [NOTICE](NOTICE).
