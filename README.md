# notypo

notypo fixes failed shell commands. It began as a Rust port of thefuck; it now asks the installed app what is valid and repairs only the wrong words, with thefuck's rules as a fallback.
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

This prints `git status`; the standalone binary does not execute the printed correction. An explicit command carries no exit status, so corrections the evidence doesn't settle are refused under `-y` instead of guessed.

## How it corrects

```sh
notypo --explain 'aws ec2 describ-instances --regoin eu-west-1'
```

`--explain` prints the diagnosis, candidates, evidence, and safety decision without running anything; `--json` prints the same report as JSON on stdout. With the shell alias, `typo` offers `aws ec2 describe-instances --region eu-west-1`.

Where the corrections come from:

- **The app itself.** notypo asks the installed app's own completer what is valid at each level and only proposes words it lists, so new commands and installed extensions work without a notypo update. Built-in bridges cover `aws` (`aws_completer`), `gcloud` and `az` (argcomplete), and `git` (`--list-cmds` and `--git-completion-helper`, run in an empty repository). Go apps are recognized from the module information in their binaries: kubectl, helm, gh, Hetzner's hcloud, kind, and docker speak cobra; terraform, tofu, and packer speak posener/complete. Other argcomplete, cobra, or posener apps, and apps that only ship a bash completion script (such as brew or deno), are probed only when listed in `trusted_completers`.
- **Option values** such as regions or output formats are checked against the app's offline value lists. Resource names (instances, buckets) are looked up only with `network_completion`, using your credentials, read-only.
- **Without a completer**, the first level and options are checked against the man page (formatted by the system `man`; the program never runs), `--help` output for programs in `trusted_help`, and your shell history. These lists can be incomplete, so a close match is offered for confirmation rather than run.
- **Programs and paths:** misspelled programs are matched against `$PATH`, aliases, and builtins, and a missing space is inserted (`cd..` → `cd ..`, `gitstatus` → `git status`). A program whose own completer accepts the rest of the line ranks first. Missing paths, including `cd` targets, are repaired from the filesystem. Commands run through `npx`, `uvx`, `pipx run`, `bundle exec`, and similar are repaired as the installed program they run; nothing is downloaded.
- **Error output**, when the shell logger or instant mode captured it, confirms the diagnosis, and "did you mean" hints raise matching candidates. Printed hints are untrusted text: they only ever become a quoted word.
- **thefuck's rules** still run, through the same safety gate: a matching rule leads when the engine isn't sure, and is offered as an alternative otherwise. Most rules need the failed command's output, which notypo has only when the shell logger or instant mode captured it, or when `replay_for_diagnosis` allows a rerun.

How it stays safe:

- The command line is parsed losslessly: an edit replaces only the misspelled word, and quotes, pipelines, redirections, and comments keep their exact bytes. Compound commands, here-documents, and fish/PowerShell/tcsh lines get an explicit "unsupported" result.
- The failed command is never rerun, unless `replay_for_diagnosis` is on and the command is low-risk.
- Completer probes run offline by default: no credentials, HTTP to a closed local port, no kubeconfig or Docker daemon. Each probe is time-limited, its output is capped, and its whole process tree is killed when time runs out. Answers are cached by the app's fingerprint, but a cached list can only confirm a word: the app is asked again before anything is called invalid.
- Every candidate passes a safety gate. These all need explicit approval, even with `-y`: added `sudo`, deletions, force or auto-approve flags, destructive git or cloud operations, package changes, changed write targets, and new redirections, operators, or substitutions. Close or weakly supported candidates also need a choice. Without a terminal to ask on, nothing runs. The gate is a heuristic, not a proof that a command is harmless.
- The shell functions pass the failed command's exit status (and pipeline statuses) to notypo, which strengthens the diagnosis.

Settings (`settings.py` name, then environment variable), besides thefuck's:

| Setting | Default | Meaning |
|---|---|---|
| `disabled_sources` / `NOTYPO_DISABLED_SOURCES` | `[]` | any of `native`, `executables`, `stderr`, `history`, `filesystem`, `man`, `help`, `legacy` |
| `trusted_completers` / `NOTYPO_TRUSTED_COMPLETERS` | `[]` | extra argcomplete/cobra/posener apps and bash completion scripts to use; `*` for all |
| `trusted_help` / `NOTYPO_TRUSTED_HELP` | `[]` | programs that may be run with `--help`; `*` for all |
| `network_completion` / `NOTYPO_NETWORK_COMPLETION` | `False` | let completers look up resource names with your credentials |
| `probe_timeout` / `NOTYPO_PROBE_TIMEOUT` | `3` | seconds per probe (three times that in total) |
| `replay_for_diagnosis` / `NOTYPO_REPLAY_FOR_DIAGNOSIS` | `False` | rerun low-risk commands to read their output |

Accuracy: on a corpus of 771 typos in real aws, gcloud, az, git, kubectl, docker, helm, and system command names (`tests/corpus.rs`), the first suggestion is right 98.8% of the time and one of the first three 100%. notypo decides alone in 95.7% of cases with no wrong decisions, and asks otherwise. To check the CLIs installed on your machine: `cargo test --test installed_clis -- --ignored --nocapture`.

Differences from thefuck:

- The failed command is never rerun to read its output, so output-based rules (for example `git push` without an upstream) need the shell logger, instant mode, or `replay_for_diagnosis = True`.
- `-y` never runs risky or uncertain corrections: added `sudo`, package changes, close alternatives, and corrections without failure evidence all ask first, and are refused without a terminal.
- Rule suggestions with side effects (`dirty_untar`, `dirty_unzip`, `ssh_known_hosts`) always ask first.

Not yet supported: fish, PowerShell, and tcsh command lines; zsh, fish, and PowerShell completion functions as a source; other completion protocols (click, oclif, yargs).

## Performance

Against the original Python thefuck 3.32 on the same commands, notypo is **6–33× faster** end to end and uses about **6× less memory** (6.8 MiB against 40.5 MiB; `benchmarks/results.md`). Asking an app's own completer adds the app's startup: git and Go CLIs answer in 20–70 ms; the Python cloud CLIs take 200–450 ms warm (`benchmarks/structured-results.md`). The engine itself takes about a millisecond.

## License

Licensed under either MIT or APACHE 2.0, at your option.

## Attribution

The original app was created by Vladimir Iakovlev. See [NOTICE](NOTICE)
