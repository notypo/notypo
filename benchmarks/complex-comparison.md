# Complex command comparison

Identical shell command and identical synthetic supplied failure text; all Python default rules enabled; offline notypo native completion/help with explicit trust; no failed/corrected shell command executed. Unchanged thefuck output is not a usable correction.

Compared with thefuck 3.32 (155 enabled default rules). These are constructed regression fixtures, not a claim about every possible error message or upstream version.

Each listed case has the expected first notypo candidate and no usable thefuck suggestion. Raw results, unchanged suggestions, captured text, trust settings, and binary fingerprint are in complex-comparison.json.

1.

```sh
# Input
AWS_PAGER='' aws ec2 describ-instances --regoin eu-west-1 --filters 'Name=tag:Team,Values=platform' --query 'Reservations[].Instances[].InstanceId' --output json | jq -r '.[]' > 'reports/instance ids.txt'
# notypo
AWS_PAGER='' aws ec2 describe-instances --region eu-west-1 --filters 'Name=tag:Team,Values=platform' --query 'Reservations[].Instances[].InstanceId' --output json | jq -r '.[]' > 'reports/instance ids.txt'
```

2.

```sh
# Input
if test -f 'config/prod.env'; then aws s3api list-objcts-v2 --bucket example-reports --prefix 'daily reports/' --max-items 50 --output json | jq '.Contents[].Key'; fi
# notypo
if test -f 'config/prod.env'; then aws s3api list-objects-v2 --bucket example-reports --prefix 'daily reports/' --max-items 50 --output json | jq '.Contents[].Key'; fi
```

3.

```sh
# Input
gcloud compte instnaces list --project example-prod --filter 'labels.team=platform AND status=RUNNING' --format 'table(name,zone,status)' | tee 'reports/cloud instances.txt'
# notypo
gcloud compute instances list --project example-prod --filter 'labels.team=platform AND status=RUNNING' --format 'table(name,zone,status)' | tee 'reports/cloud instances.txt'
```

4.

```sh
# Input
az storage acount list --resource-group example-platform --query '[].{name:name,location:location}' --output json | jq -r '.[].name' > 'reports/storage accounts.txt'
# notypo
az storage account list --resource-group example-platform --query '[].{name:name,location:location}' --output json | jq -r '.[].name' > 'reports/storage accounts.txt'
```

5.

```sh
# Input
kubectl --context example-prod gt pods --namespace platform --selector 'app=api,tier=backend' --output json | jq -r '.items[].metadata.name' > 'reports/api pods.txt'
# notypo
kubectl --context example-prod get pods --namespace platform --selector 'app=api,tier=backend' --output json | jq -r '.items[].metadata.name' > 'reports/api pods.txt'
```

6.

```sh
# Input
helm template api './charts/api service' --namespce platform --values 'config/prod values.yaml' --set 'image.tag=v2.4.0' | tee 'reports/rendered manifests.yaml'
# notypo
helm template api './charts/api service' --namespace platform --values 'config/prod values.yaml' --set 'image.tag=v2.4.0' | tee 'reports/rendered manifests.yaml'
```

7.

```sh
# Input
gh pr lsit --repo example/platform --state open --limit 50 --json number,title,author --jq '.[] | [.number,.title,.author.login] | @tsv' > 'reports/open PRs.tsv'
# notypo
gh pr list --repo example/platform --state open --limit 50 --json number,title,author --jq '.[] | [.number,.title,.author.login] | @tsv' > 'reports/open PRs.tsv'
```

8.

```sh
# Input
hcloud servr list --selector 'team=platform,env=prod' --output json | jq -r '.[] | [.name,.status] | @tsv' > 'reports/servers.tsv'
# notypo
hcloud server list --selector 'team=platform,env=prod' --output json | jq -r '.[] | [.name,.status] | @tsv' > 'reports/servers.tsv'
```

9.

```sh
# Input
if test -f 'config/cluster config.yaml'; then kind creat cluster --name integration --config 'config/cluster config.yaml' --wait 90s 2>&1 | tee 'logs/cluster setup.log'; fi
# notypo
if test -f 'config/cluster config.yaml'; then kind create cluster --name integration --config 'config/cluster config.yaml' --wait 90s 2>&1 | tee 'logs/cluster setup.log'; fi
```

10.

```sh
# Input
if test -d 'infra/production'; then tofu -chdir='infra/production' valdate -no-color 2>&1 | tee 'logs/infrastructure validation.log'; fi
# notypo
if test -d 'infra/production'; then tofu -chdir='infra/production' validate -no-color 2>&1 | tee 'logs/infrastructure validation.log'; fi
```

11.

```sh
# Input
packer biuld -only='amazon-ebs.api' -var-file='config/prod values.pkrvars.hcl' 'images/api image.pkr.hcl' 2>&1 | tee 'logs/image build.log'
# notypo
packer build -only='amazon-ebs.api' -var-file='config/prod values.pkrvars.hcl' 'images/api image.pkr.hcl' 2>&1 | tee 'logs/image build.log'
```

12.

```sh
# Input
pip3 config lsit --user --verbose 2>&1 | tee 'reports/python package settings.txt' | sed -n '1,40p'
# notypo
pip3 config list --user --verbose 2>&1 | tee 'reports/python package settings.txt' | sed -n '1,40p'
```

13.

```sh
# Input
npm config lsit --json --location=project | jq '{registry: .registry, cache: .cache}' > 'reports/npm project config.json'
# notypo
npm config list --json --location=project | jq '{registry: .registry, cache: .cache}' > 'reports/npm project config.json'
```

14.

```sh
# Input
cargo biuld --releae --workspace --locked --target aarch64-apple-darwin --package notypo 2>&1 | tee 'logs/release build.log'
# notypo
cargo build --release --workspace --locked --target aarch64-apple-darwin --package notypo 2>&1 | tee 'logs/release build.log'
```

15.

```sh
# Input
if test -f 'config/git hooks.yml'; then lefthook valdate --config 'config/git hooks.yml' 2>&1 | tee 'logs/hook validation.log'; fi
# notypo
if test -f 'config/git hooks.yml'; then lefthook validate --config 'config/git hooks.yml' 2>&1 | tee 'logs/hook validation.log'; fi
```

16.

```sh
# Input
upctl servr list --output json | jq -r '.[] | [.name,.status] | @tsv' > 'reports/cloud server inventory.tsv'
# notypo
upctl server list --output json | jq -r '.[] | [.name,.status] | @tsv' > 'reports/cloud server inventory.tsv'
```

17.

```sh
# Input
velero backp get --namespace velero --output json | jq -r '.items[] | [.metadata.name,.status.phase] | @tsv' > 'reports/backups.tsv'
# notypo
velero backup get --namespace velero --output json | jq -r '.items[] | [.metadata.name,.status.phase] | @tsv' > 'reports/backups.tsv'
```

18.

```sh
# Input
if test -f 'config/prod kubeconfig'; then k9s --readoly --context example-prod --namespace platform --kubeconfig 'config/prod kubeconfig'; fi
# notypo
if test -f 'config/prod kubeconfig'; then k9s --readonly --context example-prod --namespace platform --kubeconfig 'config/prod kubeconfig'; fi
```

19.

```sh
# Input
trivy imgae --format json --output 'reports/container vulnerabilities.json' --severity HIGH,CRITICAL --ignore-unfixed alpine:3.22
# notypo
trivy image --format json --output 'reports/container vulnerabilities.json' --severity HIGH,CRITICAL --ignore-unfixed alpine:3.22
```

20.

```sh
# Input
k6 rnu --vus 20 --duration 30s --summary-export 'reports/load test summary.json' 'tests/api load.js' 2>&1 | tee 'logs/load test.log'
# notypo
k6 run --vus 20 --duration 30s --summary-export 'reports/load test summary.json' 'tests/api load.js' 2>&1 | tee 'logs/load test.log'
```

21.

```sh
# Input
talosctl helth --nodes 192.0.2.10 --endpoints 192.0.2.10 --talosconfig 'config/cluster credentials.yaml' 2>&1 | tee 'reports/cluster health.txt'
# notypo
talosctl health --nodes 192.0.2.10 --endpoints 192.0.2.10 --talosconfig 'config/cluster credentials.yaml' 2>&1 | tee 'reports/cluster health.txt'
```

22.

```sh
# Input
if test -f 'tasks/project.just'; then just --dry-rnu --justfile 'tasks/project.just' build release 2>&1 | tee 'logs/planned build commands.txt'; fi
# notypo
if test -f 'tasks/project.just'; then just --dry-run --justfile 'tasks/project.just' build release 2>&1 | tee 'logs/planned build commands.txt'; fi
```

23.

```sh
# Input
yt-dlp --list-formts --no-playlist --cookies 'config/browser cookies.txt' 'https://example.com/watch?v=demo&lang=en' | tee 'reports/video formats.txt'
# notypo
yt-dlp --list-formats --no-playlist --cookies 'config/browser cookies.txt' 'https://example.com/watch?v=demo&lang=en' | tee 'reports/video formats.txt'
```

24.

```sh
# Input
eas biuld --platfrom ios --profile production --non-interactive --json | jq -r '.[].id' > 'reports/mobile build ids.txt'
# notypo
eas build --platform ios --profile production --non-interactive --json | jq -r '.[].id' > 'reports/mobile build ids.txt'
```

25.

```sh
# Input
if test -f 'scripts/dev server.mjs'; then node --inpsect=127.0.0.1:9229 --enable-source-maps 'scripts/dev server.mjs' 2>&1 | tee 'logs/dev server.log'; fi
# notypo
if test -f 'scripts/dev server.mjs'; then node --inspect=127.0.0.1:9229 --enable-source-maps 'scripts/dev server.mjs' 2>&1 | tee 'logs/dev server.log'; fi
```

26.

```sh
# Input
sam build --use-contaner --template-file 'infra/template prod.yaml' --base-dir 'services/api service' --parallel 2>&1 | tee 'logs/serverless build.log'
# notypo
sam build --use-container --template-file 'infra/template prod.yaml' --base-dir 'services/api service' --parallel 2>&1 | tee 'logs/serverless build.log'
```

27.

```sh
# Input
oci os bucket list --namespce example --compartment-id ocid1.compartment.oc1..example --all --output json | jq -r '.data[].name' > 'reports/object buckets.txt'
# notypo
oci os bucket list --namespace example --compartment-id ocid1.compartment.oc1..example --all --output json | jq -r '.data[].name' > 'reports/object buckets.txt'
```

28.

```sh
# Input
heroku apps:lsit --team example-platform --json | jq -r '.[] | [.name,.region.name] | @tsv' > 'reports/hosted applications.tsv'
# notypo
heroku apps:list --team example-platform --json | jq -r '.[] | [.name,.region.name] | @tsv' > 'reports/hosted applications.tsv'
```

29.

```sh
# Input
composer install --dry-rnu --no-interaction --prefer-dist --no-progress 2>&1 | tee 'logs/planned PHP dependency changes.log'
# notypo
composer install --dry-run --no-interaction --prefer-dist --no-progress 2>&1 | tee 'logs/planned PHP dependency changes.log'
```

30.

```sh
# Input
curl --silnt --show-error --fail --header 'Accept: application/json' 'https://example.com/api/status?team=platform&env=prod' | jq '.services' > 'reports/service status.json'
# notypo
curl --silent --show-error --fail --header 'Accept: application/json' 'https://example.com/api/status?team=platform&env=prod' | jq '.services' > 'reports/service status.json'
```

31.

```sh
# Input
rsync --archiv --dry-run --exclude 'node_modules/' --exclude '.git/' 'build output/' 'release staging/' 2>&1 | tee 'logs/planned file sync.log'
# notypo
rsync --archive --dry-run --exclude 'node_modules/' --exclude '.git/' 'build output/' 'release staging/' 2>&1 | tee 'logs/planned file sync.log'
```

32.

```sh
# Input
rclone copy --dry-rnu --exclude '*.tmp' --transfers 4 --checkers 8 'build output/' 'archive:releases/current/' 2>&1 | tee 'logs/planned upload.log'
# notypo
rclone copy --dry-run --exclude '*.tmp' --transfers 4 --checkers 8 'build output/' 'archive:releases/current/' 2>&1 | tee 'logs/planned upload.log'
```

33.

```sh
# Input
cat 'reports/raw inventory.json' | jq --raw-ouptut '.items[] | select(.enabled == true) | [.name,.owner] | @tsv' > 'reports/enabled services.tsv'
# notypo
cat 'reports/raw inventory.json' | jq --raw-output '.items[] | select(.enabled == true) | [.name,.owner] | @tsv' > 'reports/enabled services.tsv'
```

34.

```sh
# Input
cat 'reports/available services.txt' | fzf --multii --height 40% --layout reverse --prompt 'Choose services > ' > 'reports/selected services.txt'
# notypo
cat 'reports/available services.txt' | fzf --multi --height 40% --layout reverse --prompt 'Choose services > ' > 'reports/selected services.txt'
```

35.

```sh
# Input
if test -f 'config/prometheus prod.yml'; then promtool chek config --syntax-only 'config/prometheus prod.yml' 2>&1 | tee 'logs/monitoring validation.log'; fi
# notypo
if test -f 'config/prometheus prod.yml'; then promtool check config --syntax-only 'config/prometheus prod.yml' 2>&1 | tee 'logs/monitoring validation.log'; fi
```

36.

```sh
# Input
if test -d 'projects/api service'; then tmux new-sesion -d -s api-dev -c 'projects/api service' 'npm run dev -- --port 3000'; fi
# notypo
if test -d 'projects/api service'; then tmux new-session -d -s api-dev -c 'projects/api service' 'npm run dev -- --port 3000'; fi
```

37.

```sh
# Input
for pass in first second; do AWS_PAGER='' aws ec2 describ-instances --regoin eu-west-1 --filters 'Name=tag:Team,Values=platform' --query 'Reservations[].Instances[].InstanceId' --output json | jq -r '.[]' > 'reports/instance ids.txt'; done
# notypo
for pass in first second; do AWS_PAGER='' aws ec2 describe-instances --region eu-west-1 --filters 'Name=tag:Team,Values=platform' --query 'Reservations[].Instances[].InstanceId' --output json | jq -r '.[]' > 'reports/instance ids.txt'; done
```

38.

```sh
# Input
for pass in first second; do gcloud compte instnaces list --project example-prod --filter 'labels.team=platform AND status=RUNNING' --format 'table(name,zone,status)' | tee 'reports/cloud instances.txt'; done
# notypo
for pass in first second; do gcloud compute instances list --project example-prod --filter 'labels.team=platform AND status=RUNNING' --format 'table(name,zone,status)' | tee 'reports/cloud instances.txt'; done
```

39.

```sh
# Input
for pass in first second; do az storage acount list --resource-group example-platform --query '[].{name:name,location:location}' --output json | jq -r '.[].name' > 'reports/storage accounts.txt'; done
# notypo
for pass in first second; do az storage account list --resource-group example-platform --query '[].{name:name,location:location}' --output json | jq -r '.[].name' > 'reports/storage accounts.txt'; done
```

40.

```sh
# Input
for pass in first second; do kubectl --context example-prod gt pods --namespace platform --selector 'app=api,tier=backend' --output json | jq -r '.items[].metadata.name' > 'reports/api pods.txt'; done
# notypo
for pass in first second; do kubectl --context example-prod get pods --namespace platform --selector 'app=api,tier=backend' --output json | jq -r '.items[].metadata.name' > 'reports/api pods.txt'; done
```
