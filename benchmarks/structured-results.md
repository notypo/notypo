# Structured engine benchmark

11 processes per mode; times in ms (median / p95). Cold runs
start with an empty cache directory; warm runs reuse one. Probes are
completer subprocesses (cold / warm). Peak RSS is the largest process in
the tree, which for cloud CLIs is the app's own completer. The rule
engine column reruns the command and is measured only where that is
harmless. In-process stage timings: `cargo bench --bench engine`.

| Case | Correction | Cold | Warm | Probes | Peak RSS | Rule engine |
|---|---|---|---|---|---|---|
| aws operation and option | `aws ec2 describ-instances --regoin eu-west-1` → `aws ec2 describe-instances --region eu-west-1` | 493 / 511 | 438 / 445 | 4 / 3 | 87.8 MiB | n/a |
| gcloud two groups | `gcloud compte instnaces list` → `gcloud compute instances list` | 333 / 340 | 222 / 228 | 3 / 2 | 35.4 MiB | n/a |
| az group | `az storage acount list` → `az storage account list` | 633 / 644 | 207 / 216 | 3 / 1 | 46.3 MiB | n/a |
| git subcommand | `git sttus` → `git status` | 22 / 23 | 23 / 23 | 2 / 2 | 7.1 MiB | 15 / 16 |
| kubectl (cobra) | `kubectl gt pods` → `kubectl get pods` | 74 / 77 | 67 / 69 | 2 / 2 | 44.2 MiB | n/a |
| terraform (posener) | `terraform plna` → `terraform plan` | 47 / 49 | 34 / 35 | 1 / 1 | 58.4 MiB | n/a |
| ls option (man page) | `ls --colro=auto` → `ls --color=auto` | 136 / 143 | 4 / 4 | 1 / 0 | 4.0 MiB | 8 / 9 |
| program name | `gti status` → `git status` | 29 / 30 | 23 / 24 | 2 / 2 | 12.9 MiB | 6 / 7 |
