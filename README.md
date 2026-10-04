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

How it works:

- The command line is parsed losslessly: an edit replaces only the misspelled word, and quotes, pipelines, redirections, and comments keep their exact bytes. Compound commands, here-documents, and fish/PowerShell/tcsh lines are reported as unsupported.
- For `aws`, `gcloud`, and `az`, the app's own completer (`aws_completer`, or argcomplete for gcloud and az) lists the valid subcommands and options at each level, so new commands and installed extensions work without a notypo update. Other argcomplete apps are probed only when listed in `trusted_completers`.
- Completer probes run offline. They have no credentials, HTTP goes to a closed local port, and only command and option positions are queried, never resource names. The failed command is never rerun unless `replay_for_diagnosis` is on and the command is low-risk.
- Missing executables are matched against `$PATH`, aliases, and builtins. "Did you mean" hints in captured output help apps without a completer. The shell functions pass the failed command's exit status, which strengthens the diagnosis.
- Every candidate passes a safety gate. Added `sudo`, deletions, force flags, destructive git or cloud operations, package changes, new redirections, operators, or substitutions all need explicit approval, even with `-y`. Without a terminal to ask on, nothing runs. Close candidates also need a choice. The gate is a heuristic, not a proof that a command is harmless.
- If the engine finds nothing, legacy rules still run on any captured output, through the same gate.

Engine settings (`settings.py` name, then environment variable):

| Setting | Default | Meaning |
|---|---|---|
| `engine` / `NOTYPO_ENGINE` | `legacy` | `native` enables the structured engine |
| `disabled_sources` / `NOTYPO_DISABLED_SOURCES` | `[]` | any of `native`, `executables`, `stderr`, `legacy` |
| `trusted_completers` / `NOTYPO_TRUSTED_COMPLETERS` | `[]` | extra argcomplete apps to probe; `*` for all |
| `probe_timeout` / `NOTYPO_PROBE_TIMEOUT` | `3` | seconds per completer probe (three times that in total) |
| `replay_for_diagnosis` / `NOTYPO_REPLAY_FOR_DIAGNOSIS` | `False` | rerun low-risk commands to read their output |

Not yet supported: completion caches, history/filesystem/help/man sources, git and other app bridges, option values and resource names, and dialects other than sh/bash/zsh.

## Performance

notypo **14–39× faster** than the original Python `thefuck` and uses **10x** less memory

## License

Licensed under either MIT or APACHE 2.0, at your option.

## Attribution

The original app was created by Vladimir Iakovlev. See [NOTICE](NOTICE)
