# notypo

notypo is a crossplatform Rust port of thefuck. It suggests fixes for failed shell commands and can run the selected correction.
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

## Structured engine (experimental)

By default notypo uses thefuck's rule engine. An opt-in engine instead asks the installed app what is valid and repairs only the wrong words:

```sh
export NOTYPO_ENGINE=native        # or engine = 'native' in settings.py
notypo --explain 'aws ec2 describ-instances --regoin eu-west-1'
```

`--explain` prints the diagnosis, candidates, evidence, and safety decision without running anything. With the shell alias, `typo` offers `aws ec2 describe-instances --region eu-west-1`.

Where the corrections come from:

- **The app itself.** notypo asks the installed app's own completer what is valid at each level and only proposes words it lists, so new commands and installed extensions work without a notypo update. Built-in bridges cover `aws` (`aws_completer`), `gcloud` and `az` (argcomplete), and `git` (`--list-cmds` and `--git-completion-helper`, run in an empty repository). Go apps are recognized from the module information in their binaries: kubectl, helm, gh, Hetzner's hcloud, kind, and docker speak cobra; terraform, tofu, and packer speak posener/complete. Other argcomplete, cobra, or posener apps, and apps that only ship a bash completion script (such as brew or deno), are probed only when listed in `trusted_completers`.
- **Option values** such as regions or output formats are checked against the app's offline value lists. Resource names (instances, buckets) are looked up only with `network_completion`, using your credentials, read-only.
- **Without a completer**, the first level and options are checked against the man page (formatted by the system `man`; the program never runs), `--help` output for programs in `trusted_help`, and your shell history. These lists can be incomplete, so a close match is offered for confirmation rather than run.
- **Programs and paths:** misspelled programs are matched against `$PATH`, aliases, and builtins. A program whose own completer accepts the rest of the line ranks first. Missing paths, including `cd` targets, are repaired from the filesystem. Commands run through `npx`, `uvx`, `pipx run`, `bundle exec`, and similar are repaired as the installed program they run; nothing is downloaded.
- **Error output**, when the shell logger or instant mode captured it, confirms the diagnosis, and "did you mean" hints raise matching candidates. Printed hints are untrusted text: they only ever become a quoted word.
- If the engine finds nothing, legacy rules still run on any captured output, through the same safety gate.

How it stays safe:

- The command line is parsed losslessly: an edit replaces only the misspelled word, and quotes, pipelines, redirections, and comments keep their exact bytes. Compound commands, here-documents, and fish/PowerShell/tcsh lines get an explicit "unsupported" result.
- The failed command is never rerun, unless `replay_for_diagnosis` is on and the command is low-risk.
- Completer probes run offline by default: no credentials, HTTP to a closed local port, no kubeconfig or Docker daemon. Each probe is time-limited, its output is capped, and its whole process tree is killed when time runs out. Answers are cached by the app's fingerprint, but a cached list can only confirm a word: the app is asked again before anything is called invalid.
- Every candidate passes a safety gate. These all need explicit approval, even with `-y`: added `sudo`, deletions, force or auto-approve flags, destructive git or cloud operations, package changes, changed write targets, and new redirections, operators, or substitutions. Close or weakly supported candidates also need a choice. Without a terminal to ask on, nothing runs. The gate is a heuristic, not a proof that a command is harmless.
- The shell functions pass the failed command's exit status (and pipeline statuses) to notypo, which strengthens the diagnosis.

`--json` prints the same report as `--explain`, as JSON on stdout, for automation.

Engine settings (`settings.py` name, then environment variable):

| Setting | Default | Meaning |
|---|---|---|
| `engine` / `NOTYPO_ENGINE` | `legacy` | `native` enables the structured engine |
| `disabled_sources` / `NOTYPO_DISABLED_SOURCES` | `[]` | any of `native`, `executables`, `stderr`, `history`, `filesystem`, `man`, `help`, `legacy` |
| `trusted_completers` / `NOTYPO_TRUSTED_COMPLETERS` | `[]` | extra argcomplete/cobra/posener apps and bash completion scripts to use; `*` for all |
| `trusted_help` / `NOTYPO_TRUSTED_HELP` | `[]` | programs that may be run with `--help`; `*` for all |
| `network_completion` / `NOTYPO_NETWORK_COMPLETION` | `False` | let completers look up resource names with your credentials |
| `probe_timeout` / `NOTYPO_PROBE_TIMEOUT` | `3` | seconds per probe (three times that in total) |
| `replay_for_diagnosis` / `NOTYPO_REPLAY_FOR_DIAGNOSIS` | `False` | rerun low-risk commands to read their output |

Accuracy and speed: on a corpus of 715 typos in real aws, gcloud, az, git, kubectl, docker, helm, and system command names (`tests/corpus.rs`), the first suggestion is right 98.7% of the time and one of the first three 100%. The engine decides alone in 95.9% of cases with no wrong decisions, and asks otherwise. The engine itself takes about a millisecond. End to end, time is the app's completer: 20–70 ms for git and Go CLIs, 200–450 ms warm for the Python cloud CLIs (`benchmarks/structured-results.md`).

Compared with the rule engine, the structured engine never reruns the failed command, so rules that need output only work when the shell logger or instant mode captured it. `-y` no longer runs risky or uncertain corrections. Rule suggestions with side effects (`dirty_untar`, `dirty_unzip`, `ssh_known_hosts`) always ask first. To check the installed CLIs on your machine: `cargo test --test installed_clis -- --ignored --nocapture`.

Not yet supported: fish, PowerShell, and tcsh command lines; zsh, fish, and PowerShell completion functions as a source; other completion protocols (click, oclif, yargs).

## Performance

notypo **14–39× faster** than the original Python `thefuck` and uses **10x** less memory

## License

Licensed under either MIT or APACHE 2.0, at your option.

## Attribution

The original app was created by Vladimir Iakovlev. See [NOTICE](NOTICE)
