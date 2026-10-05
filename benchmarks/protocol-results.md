# Correction benchmark against installed CLIs

11 processes per mode; times in ms (median / p95). Cold runs
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
| oclif manifests (eas) | `eas build --platfrom ios` → `eas build --platform ios` | 10 / 298 | 9 / 9 | 0 / 0 | 7.7 MiB |
| PowerShell protocol (parameter) | `Get-ChildItem -Recrse` → `Get-ChildItem -Recurse` | 447 / 1006 | 398 / 406 | 1 / 1 | 166.9 MiB |
| PowerShell protocol (cmdlet name) | `Get-ChildItme -Name` → `Get-ChildItem -Name` | 829 / 837 | 790 / 804 | 2 / 2 | 167.2 MiB |
| PowerShell completer protocol (mdbook) | `mdbook serv` → `mdbook serve` | 404 / 407 | 353 / 402 | 1 / 1 | 141.3 MiB |
| PowerShell completer protocol (rustup) | `rustup toolchian list` → `rustup toolchain list` | 766 / 775 | 720 / 735 | 2 / 2 | 145.1 MiB |

Installed apps behind the newer bridges, each with the trust it needs
(`trusted_completers`, plus `trusted_help` for Cargo, Sofka's generator,
and lefthook's urfave/cli v3 hooks); help, man, history, and legacy
sources are off. The harness checks every suggestion; none is executed.
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

The .NET row was measured separately on 2026-10-05 with the official
osx-arm64 SDK 10.0.401 in a temporary directory. Each request starts
six SDK processes: selected-version and SDK-path checks, parser schema,
root commands, build options, and configuration values. The SDK bridge
keeps answers in memory for the request; warm runs repeat those probes.

The oclif row was measured separately on 2026-10-05 with eas-cli 24.10.0
installed into a temporary npm prefix. No process starts: the bridge
reads the CLI's and its core plugins' `oclif.manifest.json` files (about
470 KiB). The cold p95 is the first sample's file-cache warmup.
