# notypo

notypo fixes failed shell commands. It began as a Rust port of thefuck; it now asks the installed app what is valid and repairs only the wrong words, with thefuck's rules as a fallback.
Mac/Linux/Windows/FreeBSD supported. x64 and ARM

## Shell setup

Install the binary, then add this line to your shell startup file (`~/.zshrc`, `~/.bashrc`, or equivalent):

```sh
eval "$(notypo --alias)"
```

Restart your shell or source the startup file. This installs both `fuck` and `typo`; after a command fails, use either to see suggested corrections. To install only a custom name, use `notypo --alias <name>` instead.

In fish, add `notypo --alias | source` to `~/.config/fish/config.fish`. The alias preserves multiline commands, exit and pipeline statuses, and runs the selected command in the parent shell. On fish 4 and newer it updates history through the [shell's history API](https://fishshell.com/docs/current/cmds/history.html); older fish keeps the original history entry. Private sessions write no correction history.

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

`--explain` prints the diagnosis, candidates, evidence, and safety decision without running anything, including each app's identity, protocol, trust policy, and its own description of a suggested word when its completion gives one (cobra and fish do); `--json` prints the same report as JSON on stdout. With the shell alias, `typo` offers `aws ec2 describe-instances --region eu-west-1`.

Where the corrections come from:

- **The app itself.** notypo asks the installed app's own completer what is valid at each level and only proposes words it lists, so new commands and installed extensions work without a notypo update. Built-in bridges cover `aws` (`aws_completer`), `gcloud` and `az` (argcomplete), and `git` (`--list-cmds` and `--git-completion-helper`, run in an empty repository). Go apps are recognized from the module information in their binaries: kubectl, helm, gh, Hetzner's hcloud, kind, and docker speak cobra; terraform, tofu, and packer speak posener/complete. Other argcomplete, cobra, or posener apps, and installed Bash/fish/Zsh completion handlers, are probed only when listed in `trusted_completers`. Apps are identified by their package metadata (a Go binary's main module, a Python console script's entry module, a Node bin's npm package), so trust given to an identity follows the app when it is renamed and never reaches an unrelated program with the same name (two different `atlas` or `cf` CLIs); a bare name trusts whatever is installed under it. Fish scripts use native `complete --do-complete` with their conditions, option arity, and value lists. On Unix, Zsh autoload handlers declared by `#compdef` run in a [real completion widget](https://zsh.sourceforge.io/Doc/Release/Completion-Widgets.html) on a private terminal; the command buffer is never executed. Handwritten shell handlers supply partial evidence and require confirmation. Fresh shells skip user startup configuration; loading handlers and their callbacks still executes trusted code.
- **Option values** such as regions or output formats are checked against the app's offline value lists. Resource names (instances, buckets) are looked up only with `network_completion`, using your credentials, read-only; a correction that picks a different resource by name always asks first.
- **Without a completer**, commands and options are checked against the man page (formatted by the system `man`; the program never runs), `--help` output for programs in `trusted_help`, and your shell history. Help probes follow only subcommands listed by each parent help page, using `<program> <subcommands> --help` without forwarding positional arguments or option values. Short option clusters (`tar -xvzf`, `ls -la`) and attached values (`make -j4`, `kubectl -owide`) are kept intact rather than shortened. Nested options, short option aliases, and explicit value lists (`{json,yaml}`, `[possible values: json, yaml]`, or `[choices: json, yaml]`) are supported. These lists can be incomplete, so a close match is offered for confirmation rather than run.
- **Programs and paths:** misspelled programs are matched against `$PATH`, aliases, and builtins, and a missing space is inserted (`cd..` → `cd ..`, `gitstatus` → `git status`). A program whose own completer accepts the rest of the line ranks first. Missing paths, including `cd` targets, are repaired from the filesystem. Commands run through `npx`, `uvx`, `pipx run`, `bundle exec`, and similar are repaired as the installed program they run; nothing is downloaded.
- **Error output**, when the shell logger or instant mode captured it, confirms the diagnosis, and "did you mean" hints raise matching candidates. Printed hints are untrusted text: they only ever become a quoted word.
- **thefuck's rules** still run, through the same safety gate: a matching rule leads when the engine isn't sure, and is offered as an alternative otherwise. Most rules need the failed command's output, which notypo has only when the shell logger or instant mode captured it, or when `replay_for_diagnosis` allows a rerun.

How it stays safe:

- Simple sh/Bash/zsh and fish commands are parsed losslessly: edits replace the affected word or option value, while other quotes, pipelines, redirections, and comments keep their exact bytes. Fish uses its own escaping, substitutions, descriptor pipes, and `; and`/`; or` operators. Expansions remain opaque. Commands inside compound commands are repaired in place: `{ ...; }` groups, subshells, `if`, `while`/`until`, `for`/`select`, `case`, function definitions, and `!`; in fish, `begin`, `if`/`else if`, `while`, `for`, `switch`, `function`, and `not`. A correction must leave the reserved words, loop items, case patterns, and compound redirections exactly as typed. tcsh lines (simple commands, pipelines, lists, and redirections, including the spaced `> &` and `| &` forms tcsh's history prints) use tcsh's quoting: no comments or `NAME=value` prefixes, `\!` for a literal `!`, and no descriptor numbers. tcsh records events after history substitution without the backslashes that stopped it, so a recorded line whose `!` would be substituted again is not rerun; neither are tcsh's control words, `( )`, or ambiguous redirects. Arithmetic commands, `[[ ... ]]`, `coproc`, zsh-only short forms (`for x (a b)`, `{ ls }`, `then;`, `always`), fish's `{ ...; }` blocks, here-documents, and PowerShell lines get an explicit "unsupported" result.
- The failed command is never rerun, unless `replay_for_diagnosis` is on and the command is low-risk. Compound commands (loops, conditions, groups, functions) never qualify.
- Audited completer protocols use offline settings by default: credentials or live lookups are disabled, HTTP is routed to a closed local port, and cluster/daemon access is disabled. Arbitrary trusted shell scripts inherit the user's environment with blocked HTTP proxies; this is not a sandbox. Each probe is time-limited, its output is capped, and its whole process tree is killed when time runs out. Failed, truncated, or invalid UTF-8 answers are discarded. Answers are cached by the app and handler fingerprints, but a cached list can only confirm a word: the app is asked again before anything is called invalid.
- Every candidate passes a safety gate. These all need explicit approval, even with `-y`: added `sudo`, deletions, force or auto-approve flags, destructive git or cloud operations, package changes, changed write targets, new redirections/operators, and command substitutions with unknown effects (including existing substitutions and redirection targets). Close or weakly supported candidates also need a choice. Without a terminal to ask on, nothing runs. The gate is a heuristic, not a proof that a command is harmless.
- Right before a chosen correction runs, notypo rechecks what its edits relied on: programs it introduced still resolve to executable files, and repaired paths still exist (as directories for `cd`, as executables when they name the program). This narrows the window between discovery and execution but can't close it; a file can still change after the check and before the shell runs the command.
- The shell functions pass the failed command's exit status (and pipeline statuses) to notypo, which strengthens the diagnosis. tcsh reports a missing command with status 1 rather than 127; since tcsh has no functions and its aliases and builtins are known, a failed name that resolves to none of them counts as missing.

Disk completion caches contain only command-only contexts from protocols with complete command/option lists. Queries with preceding option values or positional arguments, and handwritten Bash/fish/Zsh handlers, reuse answers only within the current request. This keeps argument values and partial handler resource lists out of persistent metadata at the cost of more probes for those paths.

Settings (`settings.py` name, then environment variable), besides thefuck's:

| Setting | Default | Meaning |
|---|---|---|
| `disabled_sources` / `NOTYPO_DISABLED_SOURCES` | `[]` | any of `native`, `executables`, `stderr`, `history`, `filesystem`, `man`, `help`, `legacy` |
| `trusted_completers` / `NOTYPO_TRUSTED_COMPLETERS` | `[]` | extra argcomplete/cobra/posener apps and installed Bash/fish/Zsh completion handlers to use, by name or by identity (a Go module path such as `ariga.io/atlas/cmd/atlas`, `python:<module>`, or `npm:<package>`); `*` for all |
| `trusted_help` / `NOTYPO_TRUSTED_HELP` | `[]` | programs that may be run with `--help`, including documented nested subcommands; `*` for all |
| `network_completion` / `NOTYPO_NETWORK_COMPLETION` | `False` | let completers look up resource names with your credentials |
| `probe_timeout` / `NOTYPO_PROBE_TIMEOUT` | `3` | seconds per probe (three times that in total) |
| `replay_for_diagnosis` / `NOTYPO_REPLAY_FOR_DIAGNOSIS` | `False` | rerun low-risk commands to read their output |

Accuracy: on a corpus of 934 typos in real aws, gcloud, az, git, kubectl, docker, helm, and system command names and long option names (`tests/corpus.rs`), the first suggestion is right 99.0% of the time and one of the first three 100%. notypo decides alone in 96.5% of cases with no wrong decisions, and asks otherwise. Option names are compared without their leading dashes. To check the CLIs installed on your machine: `cargo test --test installed_clis -- --ignored --nocapture`.

Differences from thefuck:

- The failed command is never rerun to read its output, so output-based rules (for example `git push` without an upstream) need the shell logger, instant mode, or `replay_for_diagnosis = True`.
- `-y` never runs risky or uncertain corrections: added `sudo`, package changes, close alternatives, and corrections without failure evidence all ask first, and are refused without a terminal.
- Rule suggestions with side effects (`dirty_untar`, `dirty_unzip`, `ssh_known_hosts`) always ask first.
- The tcsh alias passes the previous event through the environment, so its quotes and glob patterns are no longer expanded before notypo reads them, and it forwards the alias's own arguments (`fuck -y`).
- For other POSIX shells (ksh and the like), `--alias` prints a shell function rather than an alias: it passes the exit status and recent history, and forwards its arguments to notypo instead of appending them to the corrected command. It needs the shell's `fc` history; dash has none.

Not yet supported: PowerShell command lines; tcsh's control words and `( )`; arithmetic, `[[ ... ]]`, coprocesses, zsh-only compound forms, and fish brace blocks; PowerShell completion functions, fish's embedded completers or handlers registered only in memory; other completion protocols (click, oclif, yargs). Zsh supports installed autoload handlers from the parent shell's `fpath` or standard locations; rich replacement ranges and nonempty completion affixes are skipped, and option arity/descriptions are unavailable. Zsh, fish, and tcsh runtime regressions were checked with Zsh 5.9, fish 4.9.3, and tcsh 6.21 on macOS; other versions and platforms still need runtime validation.

After checking documented app protocols, discovery prefers the parent shell's installed handler and falls back to other installed Bash/fish/Zsh handlers. These handlers all require `trusted_completers`; choosing a different shell for the completion probe preserves the original command's editing dialect.

## Performance

Against the original Python thefuck 3.32 on the same commands, notypo is **6–33× faster** end to end and uses about **6× less memory** (6.8 MiB against 40.5 MiB; `benchmarks/results.md`). Asking an app's own completer adds the app's startup: git and Go CLIs answer in 20–70 ms; the Python cloud CLIs take 200–450 ms warm (`benchmarks/structured-results.md`). The engine itself takes about a millisecond.

## License

Licensed under either MIT or APACHE 2.0, at your option.

## Attribution

The original app was created by Vladimir Iakovlev. See [NOTICE](NOTICE)
