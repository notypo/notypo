# Correction benchmark against installed CLIs

11 processes per mode for the original rows, 15 for kingpin, Click,
RabbitMQ, and the PowerShell parameter and positional callbacks; times in ms
(median / p95). Cold runs
start with an empty cache directory; warm runs reuse one. Probes are
completer subprocesses (cold / warm). RSS uses the process usage returned
by wait4; child accounting depends on the OS. In-process
stage timings: `cargo bench --bench engine`.

| Case | Correction | Cold | Warm | Probes | Peak RSS |
|---|---|---|---|---|---|
| pip protocol | `pip3 config lsit` → `pip3 config list` | 230 / 286 | 230 / 234 | 2 / 2 | 36.0 MiB |
| npm protocol | `npm config lsit` → `npm config list` | 181 / 348 | 181 / 185 | 2 / 2 | 71.2 MiB |
| cargo protocol | `cargo build --releae` → `cargo build --release` | 74 / 76 | 74 / 75 | 4 / 4 | 34.4 MiB |
| dotnet protocol | `dotnet build --configuraton Release` → `dotnet build --configuration Release` | 607 / 1147 | 607 / 614 | 6 / 6 | 87.7 MiB |
| clap protocol (sofka) | `sofka --readoly` → `sofka --readonly` | 63 / 92 | 57 / 58 | 5 / 5 | 21.6 MiB |
| clap protocol (just shim) | `just --dry-rnu` → `just --dry-run` | 20 / 23 | 17 / 17 | 3 / 3 | 15.1 MiB |
| urfave/cli protocol (lefthook) | `lefthook valdate` → `lefthook validate` | 18 / 26 | 15 / 16 | 2 / 2 | 22.6 MiB |
| cobra protocol (k9s, user-trusted) | `k9s --readoly` → `k9s --readonly` | 64 / 171 | 47 / 48 | 1 / 1 | 76.3 MiB |
| bash handler protocol (bq) | `bq qeury x` → `bq query x` | 1108 / 1140 | 1106 / 1125 | 2 / 2 | 70.0 MiB |
| Click protocol (SAM) | `sam build --use-contaner` → `sam build --use-container` | 1721 / 1761 | 1712 / 1798 | 4 / 4 | 108.2 MiB |
| Click protocol (OCI) | `oci os bucket list --namespce x` → `oci os bucket list --namespace x` | 3156 / 3542 | 3173 / 3530 | 9 / 9 | 94.1 MiB |
| oclif manifests (eas) | `eas build --platfrom ios` → `eas build --platform ios` | 10 / 298 | 9 / 9 | 0 / 0 | 7.7 MiB |
| Symfony Console protocol (Composer) | `composer install --dry-rnu` → `composer install --dry-run` | 655 / 1052 | 645 / 699 | 5 / 5 | 35.8 MiB |
| kingpin protocol (promtool) | `promtool check config --syntax-onyl prometheus.yml` → `promtool check config --syntax-only prometheus.yml` | 106 / 522 | 96 / 116 | 4 / 4 | 53.8 MiB |
| kingpin protocol (kopia) | `kopia snapshot restore --paralell 4 object-id output` → `kopia snapshot restore --parallel 4 object-id output` | 126 / 132 | 119 / 120 | 4 / 4 | 35.9 MiB |
| kingpin/fisk protocol (nats) | `nats stream ls --jsoon` → `nats stream ls --json` | 66 / 72 | 61 / 64 | 4 / 4 | 28.8 MiB |
| RabbitMQ CLI protocol (command) | `rabbitmqctl lsit_queues` → `rabbitmqctl list_queues` | 530 / 900 | 531 / 539 | 1 / 1 | 99.0 MiB |
| RabbitMQ CLI protocol (option) | `rabbitmqctl delete_queue --if-emtpy orders` → `rabbitmqctl delete_queue --if-empty orders` | 1444 / 1473 | 1442 / 1530 | 3 / 3 | 99.5 MiB |
| PowerShell protocol (parameter) | `Get-ChildItem -Recrse` → `Get-ChildItem -Recurse` | 447 / 1006 | 398 / 406 | 1 / 1 | 166.9 MiB |
| PowerShell protocol (cmdlet name) | `Get-ChildItme -Name` → `Get-ChildItem -Name` | 829 / 837 | 790 / 804 | 2 / 2 | 167.2 MiB |
| PowerShell completer protocol (mdbook) | `mdbook serv` → `mdbook serve` | 404 / 407 | 353 / 402 | 1 / 1 | 141.3 MiB |
| PowerShell completer protocol (rustup) | `rustup toolchian list` → `rustup toolchain list` | 766 / 775 | 720 / 735 | 2 / 2 | 145.1 MiB |
| PowerShell parameter callback protocol | `Get-NotypoBench -Scope west -Target wesst-one` → `Get-NotypoBench -Scope west -Target west-one` | 752 / 811 | 700 / 737 | 2 / 2 | 154.6 MiB |
| PowerShell positional callback protocol | `Get-NotypoBench wesst-one -Scope west` → `Get-NotypoBench west-one -Scope west` | 904 / 909 | 870 / 1015 | 3 / 3 | 155.6 MiB |
| posener protocol (vault, audited) | `vault secrets lsit` → `vault secrets list` | 253 / 1007 | 187 / 193 | 4 / 3 | 135.0 MiB |
| cobra protocol (helm, audited, alias check) | `helm instal x` → `helm install x` | 92 / 95 | 92 / 94 | 3 / 3 | 43.6 MiB |
| cobra boolean flag (restic, audited) | `restic backup --exclud x .` → `restic backup --exclude x .` | 222 / 522 | 195 / 197 | 8 / 7 | 26.3 MiB |
| zsh handler protocol (tmux) | `tmux attahc -t x` → `tmux attach -t x` | 1342 / 1374 | 1337 / 1370 | 5 / 5 | 7.6 MiB |

Installed apps behind the newer bridges, each with the trust it needs
(`trusted_completers`, plus `trusted_help` for Cargo, Sofka's generator,
and lefthook's urfave/cli v3 hooks); help, man, history, and legacy
sources are off except OCI's trusted help fallback for its unsupported root
reply. The harness checks every suggestion; none is executed.
These bridges keep answers in request memory, so warm runs repeat their
probes; cobra answers for command-only contexts reach the disk cache.
The bq case runs the Cloud SDK's bash helper, which starts Python for
`bq help`. PowerShell cases start a profile-free PowerShell once to
describe the cmdlet, and once more to list command names when the
name itself is wrong; their peak RSS is that PowerShell's. The PowerShell
rows were measured on 2026-10-05 with portable PowerShell 7.6.6
(`NOTYPO_BENCH_PWSH`), separately from the rows above.

The PowerShell completer rows were measured later on 2026-10-05, with the
same PowerShell: mdbook 0.5.2's and rustup 1.29.1's generated (clap)
completers, registered in a session and passed as the integration
function passes them (12 and 48 KiB). Each query starts a profile-free
PowerShell that defines the script's functions and asks
`CommandCompletion` for the line: one for mdbook's root, two for rustup
(root, then `toolchain`). Answers stay in request memory, so warm runs
repeat the probes. In the same run, the two older PowerShell rows
re-measured 484/435 and 908/858 ms (cold/warm medians).

The parameter callback row was measured on macOS on 2026-10-06 with
PowerShell 7.6.6 and 15 processes per mode. The integration captures a
callback-bearing test function; PowerShell binds its literal `-Scope west`
context and lists `west-one`. Every correction is checked and never run.
The resource edit needs confirmation. One process reads metadata and one
queries the callback; both repeat on warm requests. This measures a local
fixture, not an installed cloud module or an authenticated resource lookup.

The positional callback row was measured on macOS on 2026-10-06 with the
same PowerShell 7.6.6 and 15 processes per mode. One probe describes the
function, one asks StaticParameterBinder which parameter owns the empty
positional slot, and one calls that parameter's completer. The later
`-Scope west` argument participates in binding and reaches the callback.
Every suggestion is checked and requires confirmation; the function body
never runs. All three probes repeat on warm requests. Reproduce with
`NOTYPO_BENCH_PWSH=/path/to/pwsh python3 benchmarks/structured.py --protocols --case 'PowerShell positional' --samples 15`.

The .NET row was measured separately on 2026-10-05 with the official
osx-arm64 SDK 10.0.401 in a temporary directory. Each request starts
six SDK processes: selected-version and SDK-path checks, parser schema,
root commands, build options, and configuration values. The SDK bridge
keeps answers in memory for the request; warm runs repeat those probes.

The oclif row was measured separately on 2026-10-05 with eas-cli 24.10.0
installed into a temporary npm prefix. No process starts: the bridge
reads the CLI's and its core plugins' `oclif.manifest.json` files (about
470 KiB). The cold p95 is the first sample's file-cache warmup.

The Symfony row was measured separately on 2026-10-05 on macOS with
Composer 2.10.3 and PHP 8.5.11. Both completion and help are trusted;
fallback help, man pages, history, and legacy rules are disabled. Each
request reads application help, verifies the hidden completion command's
JSON definition, enumerates commands and options through `_complete`, and
uses the target command's JSON help for option arity (five probes). Composer
stays offline with script execution disabled; suggestions are never run.
Answers remain in request memory, so warm runs repeat the same probes.

The kingpin rows were measured on macOS on 2026-10-05 with promtool 3.15.0,
Kopia 0.23.1, and NATS 0.5.0 (fisk 0.9.1), using 15 samples per mode.
Both trust settings are enabled. Each request reads the root and two command
levels' help, then asks native completion for long option names with a final
`--`. Command names come from help because argument hints, including hints
reached through default commands, can open a repository or contact a server.
The native option list is checked against help to exclude flags belonging only
to an implicit default command. All four probes repeat in warm requests;
no operation or argument callback is invoked.

The Click rows were measured on macOS on 2026-10-05, with 15 samples per
mode: SAM CLI 1.166.2/Click 8.1.8 and OCI CLI 3.94.1/Click 8.4.2 in separate
temporary virtualenvs. These cases simulate the shell integration's captured
failure with TF_HISTORY and status 1, with diagnostic replay disabled.
OCI's bare-root completion exits 1, so its trusted help establishes the
existing command path; the edited option still comes from native completion.
OCI's multiline descriptions require literal Bash replies after Zsh's
unframed reply fails validation. SAM uses four probes and OCI nine; warm
requests repeat them all. Interpreter/application startup dominates these
1.7/3.2-second corrections, while the common engine path remains inexpensive.

The RabbitMQ rows were measured on macOS on 2026-10-06 with Homebrew's
RabbitMQ 4.3.6 (Erlang/OTP 28), using 15 samples per mode and no running
broker. Both trust settings are enabled. A command repair reads the root
`help` listing (one probe). An option repair also asks `autocomplete -- --`
for the global switches and reads the target command's `help` usage (three
probes). Each probe starts an Erlang VM in notypo's private probe directory,
which is also its HOME; VM startup dominates. Answers stay in request
memory, so warm runs repeat the probes. Neither command contacts a node,
and the corrected operation is never run.

The last four rows were measured on macOS on 2026-10-06 with 15 samples per
mode: vault 2.1.1 (hashicorp tap) and helm 4, restic 0.19.1 (audited cobra and
posener apps, no trust settings), and tmux 3.7c through zsh 5.9's handler
under trusted_completers. They show the probes added that day: a word a
posener or handler list lacks is checked against the answers after a listed
word (argument slots are not judged); cobra's resolution confirms aliases
before calling a word misspelled (helm's third probe); and an option
followed by an argument is asked whether it takes a value (two probes per
such option for cobra and shell handlers, memoized; restic's seven or
eight). Each ZLE probe costs about 270 ms, so tmux's two extra probes bring
it from about 0.8 s to 1.35 s. vault's cold p95 (about 1 s) comes from
a single slow start among the cold samples; its 135 MiB peak RSS is
vault's own.
