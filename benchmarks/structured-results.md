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
