# Correction benchmark against installed CLIs

11 processes per mode; times in ms (median / p95). Cold runs
start with an empty cache directory; warm runs reuse one. Probes are
completer subprocesses (cold / warm). Peak RSS is the largest process in
the tree, which for cloud CLIs is the app's own completer. In-process
stage timings: `cargo bench --bench engine`.

| Case | Correction | Cold | Warm | Probes | Peak RSS |
|---|---|---|---|---|---|
| aws operation and option | `aws ec2 describ-instances --regoin eu-west-1` → `aws ec2 describe-instances --region eu-west-1` | 396 / 443 | 346 / 349 | 4 / 3 | 87.8 MiB |
| gcloud two groups | `gcloud compte instnaces list` → `gcloud compute instances list` | 271 / 280 | 183 / 184 | 3 / 2 | 35.4 MiB |
| az group | `az storage acount list` → `az storage account list` | 505 / 510 | 169 / 170 | 3 / 1 | 46.3 MiB |
| git subcommand | `git sttus` → `git status` | 19 / 19 | 18 / 19 | 2 / 2 | 7.1 MiB |
| kubectl (cobra) | `kubectl gt pods` → `kubectl get pods` | 66 / 67 | 58 / 58 | 2 / 2 | 44.6 MiB |
| terraform (posener) | `terraform plna` → `terraform plan` | 43 / 45 | 29 / 30 | 1 / 1 | 58.3 MiB |
| ls option (man page) | `ls --colro=auto` → `ls --color=auto` | 114 / 115 | 4 / 4 | 1 / 0 | 3.9 MiB |
| program name | `gti status` → `git status` | 24 / 24 | 20 / 20 | 2 / 2 | 12.8 MiB |

The help fallback was checked on macOS on 2026-10-04. With the installed
Cargo CLI, `cargo buidl --relase` becomes `cargo build --release` after two
help probes; it still requires confirmation because help lists are partial.
The new `read help vocabulary` stage in `cargo bench --locked --bench engine`
reads commands, option arity, and enum values from a small help fixture in
3.40 µs median / 3.48 µs p95 (20 samples of 2,000 iterations, excluding
subprocess startup). The existing nested correction stage remains at
0.95 ms median / 0.95 ms p95.

Fish parsing and quoting were measured on the same machine on 2026-10-04
with `cargo bench --locked --bench engine` (20 samples of 2,000 iterations).
The simple fish pipeline/list fixture takes 1.03 µs median / 1.21 µs p95;
quoting a value containing an apostrophe, backslash, and shell-shaped text
takes 0.24 µs / 0.28 µs. Nested correction with in-memory completers remains
0.94 ms / 0.95 ms. These stages exclude real shell/completer startup;
end-to-end fixture measurements for fish and Zsh native handlers are in
[shell-results.md](shell-results.md). Real app-specific shell handler startup
and resource completion costs remain unmeasured.
