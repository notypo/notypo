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

- **Native completion:** asks installed CLIs and shell completion handlers for valid commands, options, and values. Supports built-in bridges (including Node.js's own option list) and argcomplete, Cobra, Posener, urfave/cli, kingpin, click, generated clap, pip, npm, and Cargo protocols. Installed handlers and generic protocols require `trusted_completers`. For fish users, the completion scripts that fish 4 embeds in its binary count as installed handlers.
- **More evidence:** checks help pages, man pages, executable names, paths, shell history, and captured error hints. It can repair nested commands, option values, attached values, and commands inside supported compound shell syntax.
- **Safer corrections:** preserves shell syntax while editing only mistaken words; never reruns the failed command by default. Risky operations, including destructive commands, package changes, disk operations, database resets, and changed write targets, require confirmation even with `-y`.
- **Inspection and automation:** `--explain` and `--json` report evidence and safety decisions without running a command.
- **Shell integration:** passes failure and pipeline statuses, supports multiline commands, and handles history across supported shells.

Bash, Zsh, and tcsh aliases reported by the shell integration are resolved. When an alias runs one simple command, such as `ls='eza --icons'`, notypo checks the line with that program's completion, help, and man pages, using the alias's own arguments as context, and keeps the alias in the correction. The safety gate judges what an alias runs, so a correction through `rmf='rm -rf'` needs confirmation. Aliases whose expansion uses shell operators or expansions are not corrected through, and running one in a correction needs confirmation. Fish aliases are functions whose bodies are not read. Legacy rules still see thefuck's alias expansion.

PowerShell 7 command lines are parsed with PowerShell's own rules: native and cmdlet invocations joined by `|`, `;`, `&&`, `||`, and `&`, with redirections and comments. Corrections keep PowerShell quoting. PowerShell splits some arguments before a program sees them (`-Dprop.name=1` arrives as `-Dprop` and `.name=1`), and notypo checks the words a program actually receives. Statements, expressions, script blocks, splatting, the call operator `&`, and `--%` are reported as unsupported. Cmdlets that delete, stop, or run code, such as `Remove-Item`, `Stop-Process`, and `iex`, need confirmation, as do `-Force` and `-Recurse` outside reading cmdlets such as `Get-ChildItem`.

The PowerShell function passes `$?` and the error records PowerShell logged for the last history entry. They stand in for an exit status (127 when a command wasn't found, a native program's exit code when only it failed, 1 otherwise) and for output. It also passes what each command name in the line resolves to in the session. notypo asks PowerShell itself about cmdlets and functions: a profile-free, non-interactive copy of the session's PowerShell lists command names from module manifests without importing anything, and describes one command's parameters, aliases, switches, and enumeration and `ValidateSet` values. `Get-ChildItme` becomes `Get-ChildItem`, `gci -Dpth 2` becomes `gci -Depth 2`, and `-ExecutionPolicy RemoteSignd` becomes `RemoteSigned`. Parameters are matched as PowerShell's binder matches them: case is ignored, aliases and unambiguous prefixes are valid, and `-Name:value` attaches a value. Describing a command imports its module, which runs the module's code, so only commands built into PowerShell or shipped under `$PSHOME` are described automatically. List other modules in `trusted_completers` as `powershell:<Module>` (for example, `powershell:Az.Accounts`). Simple functions that take undeclared arguments get no parameter checks. PowerShell's `TabExpansion2` completers and the session's profile-defined functions are not consulted. Some shell constructs are reported as unsupported; see the diagnosis for details. Completion probing runs installed programs or trusted handlers, so only trust programs you control. Network resource completion and diagnostic replay are off by default.

Thefuck's correction rules are included. Rules that need the failed command's output require captured output (shell logger or instant mode) unless safe diagnostic replay is enabled.

Clap dynamic completion reads its activation variable and argument layout from an installed Bash/Zsh script or a documented script generator. An installed registration in clap's recommended form, `source <(VAR=zsh app)` or Homebrew's `eval "$(VAR=bash app)"`, names the variable; notypo runs that generator directly instead of sourcing the file. Generator discovery requires both `trusted_completers` and `trusted_help`; for example, list `sofka` in both. Unknown layouts are skipped. Generated scripts are read without sourcing, and their completion answers are reused only within the current request.

Pip launchers are recognized from their Python entry module, including versioned or renamed executables. Trust `python:pip` to use its offline command and option completion. Pip's protocol cannot preserve context words containing whitespace or empty arguments, and it does not enumerate option enum values. Nested and option lists can be incomplete, so their corrections need confirmation.

Trust `npm:npm` for npm's offline command and project-script completion. Discovery verifies the package's declared npm executable, including renamed symlinks; npx is excluded. Script names come from npm without running scripts. Spaces, quotes, and explicit `--prefix` scopes are preserved. Workspace selection and flags outside the offline input policy are skipped. Option lists can be incomplete, and enum values are unavailable. Script, lifecycle, package, and publication changes require confirmation.

Trust `rust:cargo` for Cargo's command list and installed toolchain names. Also list it in `trusted_help` for nested commands, options, enums, and offline workspace package, target, and feature values. Discovery requires a Rust installer receipt or verified rustup proxy link. Aliases are listed without executing them; extensions need their own trust entries. Probes disable toolchain auto-installation. Builds, aliases, extensions, and changed project values require confirmation.

Go apps built with urfave/cli are recognized from the module information in their binaries. Trust the app's module path, such as `github.com/evilmartians/lefthook/v2`, or its name. Queries contain only command names the app listed, plus `-` for flags; options, their values, and `--` are never sent. Apps that link urfave/cli together with Cobra or Posener are skipped unless audited. Some urfave/cli releases run the app's `Before` hooks while completing: every query from v3.10, and subcommand queries before v1.22.10 and v2.13. Those apps also need `trusted_help`. Lists below the root, and all v3 lists, can be incomplete, so their corrections need confirmation. Words that an app's own completion callback mixes with flags are treated as local resources. v3 does not list aliases, so notypo asks the app whether an unlisted word is a command.

Go apps built with kingpin or its fisk fork answer `--completion-bash`. Answering still runs the app's pre-actions and sets every flag's default value, which can create files, so these apps need both `trusted_completers` and `trusted_help`, and their probes run in a private empty directory. Only command names the app listed are sent. kingpin lists no aliases, short flags, or `--no-` forms, so its corrections need confirmation. Go apps that link more than one completion library are skipped unless audited.

Node.js lists its options with `node --completion-bash` and their arity in `node --help`; both print without running a script, and `NODE_OPTIONS` is cleared. V8 accepts flags the list omits, so node corrections need confirmation unless node's error names the option. The first word that isn't an option is the script, and the words after it are the script's, so notypo doesn't check them.

Click 8 apps (including Typer apps) answer completion through an environment variable named after the program, such as `_BLACK_COMPLETE`. notypo learns the variable from an installed bash, zsh, or fish script that click generated, without sourcing it; Homebrew installs such scripts for many click tools. Without a script, a Python console script listed in both `trusted_completers` and `trusted_help` is asked for `--help`, and the variable is used only if the help shows click's signature. Trust the identity, for example `python:black`. Completion never passes arguments to the app and runs no command callbacks, but click runs parameter callbacks, and apps that load commands lazily can import code while listing them: `flask` imports the project's application, so it follows the workspace trust below. Lists below the root can be incomplete, so their corrections need confirmation.

Some programs evaluate project files just to list their tasks or commands: make runs a Makefile's `$(shell ...)`, rake and fastlane load Ruby, gradle, sbt, and lein run build scripts, gulp and grunt load JavaScript, and flask imports the application. In a directory containing such a project file (`Makefile`, `Rakefile`, `build.gradle`, `gulpfile.js`, `app.py`, ...), notypo doesn't run these listings unless `trusted_workspaces` names that directory or one above it. This covers completion handlers, `--help` probes, and the legacy rules that list gulp, grunt, gradle, and react-native tasks. Programs inside the directory, such as `./gradlew`, are never run there to list anything without that trust. `just` lists recipes without evaluating backticks, so it needs no workspace trust.

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

`NOTYPO_TRUSTED_COMPLETERS` and `NOTYPO_TRUSTED_HELP` accept colon-separated names or a JSON array. `NOTYPO_TRUSTED_WORKSPACES` takes directories in your platform's path-list form (`:`-separated, or `;` on Windows) or a JSON array. Use JSON for identities containing colons, for example `NOTYPO_TRUSTED_COMPLETERS='["python:pip", "npm:npm", "rust:cargo"]'`. Completion bridges that need `trusted_help` (Cargo, urfave/cli, click, and generated clap scripts) also accept the app's identity there; the `--help` fallback source matches program names. Package and operation checks also recognize pip, npm, and receipt-bound Cargo when invoked through renamed executables.

## Performance

In local macOS benchmarks, notypo was 6–33× faster end to end than Python thefuck 3.32 and used about 6× less memory. Native CLI completion adds the startup time of the installed app. Details: [benchmark results](benchmarks/results.md) and [completion measurements](benchmarks/structured-results.md).

## License

MIT or Apache-2.0. The original app was created by Vladimir Iakovlev; see [NOTICE](NOTICE).
