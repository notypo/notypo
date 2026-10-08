//! Approval policies for operations whose names aren't generic delete verbs.
//!
//! These names never supply correction candidates. They only describe effects
//! after providers have proposed a command. Inspect operation positions and
//! flags separately so a resource named `stop` or `reset` isn't an operation.

const DISK: &str = "can format, partition, or wipe disks and volumes";
const USER: &str = "removes users or groups";
const SHUTDOWN: &str = "can shut down or suspend the system";
const FIREWALL: &str = "changes firewall rules";
const SERVER: &str = "can stop or restart a service";
const CERTIFICATE: &str = "deletes or revokes certificates or deactivates ACME accounts";
const TRUST_STORE: &str = "changes the system's trusted certificate authorities";
const SERVER_CONFIG: &str = "rolls back the web server's configuration";
const HOOKS: &str = "installs or removes git hooks, or changes where git looks for them";
const ACCOUNTS: &str = "creates or changes users, groups, or their passwords";
const SYSTEM: &str = "changes system settings";
const DEVICE: &str =
    "installs, removes, or writes software or files on a connected device, or reboots it";
const FLASH: &str = "can flash, erase, or unlock a device's partitions";
const MIGRATION: &str = "can discard database data or roll back migrations";
const BACKUP: &str = "can remove backup snapshots or stored data";
const NETWORK: &str = "changes tunnels, network configuration, or intercepted traffic";
const PACKAGES: &str = "can install, remove, or switch packages or tool versions";
const MESSAGING: &str = "can delete or purge messaging data, close connections, reset or stop nodes, or run code on them";
const PLUGINS: &str = "changes the broker's enabled plugins";
const OFFSETS: &str = "moves or deletes consumer group offsets";
const CLUSTER: &str =
    "deletes deployed resources or namespaces, or runs provider operations on the cluster";
const SECRETS: &str = "can disable or move secrets engines, auth methods, or audit devices, roll back or restore data, or seal, rekey, or step down the cluster";
const SCHEDULER: &str =
    "can stop, revert, drain, or garbage-collect workloads, or remove servers from the cluster";
const AGENTS: &str = "can make agents leave the cluster, restore a snapshot, put nodes in maintenance, or run commands on them";
const SESSIONS: &str = "cancels sessions";
const INFRASTRUCTURE: &str =
    "can destroy or replace infrastructure, overwrite state, or break a state lock";
const ARTIFACTS: &str = "can delete existing build artifacts or old deployments";
const ENVRC: &str = "trusts a project's .envrc and runs it";
const LINKS: &str = "removes the package's links";

/// chezmoi's value options that redirect where it reads or writes.
const CHEZMOI_PLACES: &[&str] = &[
    "-S",
    "--source",
    "-D",
    "--destination",
    "-W",
    "--working-tree",
    "-c",
    "--config",
];
/// stow's value options naming the stow and target directories.
const STOW_PLACES: &[&str] = &["-t", "--target", "-d", "--dir"];
/// Teleport's `tsh` options naming the proxy and cluster it connects to.
const TSH_PLACES: &[&str] = &["--proxy", "--cluster"];
/// tsh operations whose following words name nodes, clusters, or databases.
const TSH_CONNECTIONS: &[&str] = &["ssh", "scp", "join", "login", "kube", "db", "app", "proxy"];
/// trufflehog's options naming what a scan reads (repositories, buckets).
const TRUFFLEHOG_PLACES: &[&str] = &["--repo", "--org", "--bucket", "--endpoint", "--uri"];
const QUARANTINE: &str = "deletes or moves the files it flags";
const KEYS: &str = "deletes keys";
const OVERWRITES: &str = "overwrites existing files";
const MOVES: &str = "deletes its input files after archiving them";

/// HashiCorp's CLIs (and OpenBao) parse each command's flags with Go's
/// flag package, after the command path: `-force` and `--force` are one
/// flag, and `-h`, `-help`, or `--help` anywhere before `--` prints help.
fn go_flag_names<'a>(args: &[&'a str]) -> Vec<&'a str> {
    args.iter()
        .take_while(|arg| **arg != "--")
        .filter_map(|arg| {
            let name = arg.strip_prefix("--").or_else(|| arg.strip_prefix('-'))?;
            Some(name.split_once('=').map_or(name, |(name, _)| name))
        })
        .filter(|name| !name.is_empty())
        .collect()
}

fn hashicorp_help(args: &[&str]) -> bool {
    args.iter()
        .take_while(|arg| **arg != "--")
        .any(|arg| ["-h", "-help", "--help"].contains(arg))
}

/// RabbitMQ's CLI tools, which share one parser and its global options.
const RABBITMQ: &[&str] = &[
    "rabbitmqctl",
    "rabbitmq-diagnostics",
    "rabbitmq-plugins",
    "rabbitmq-queues",
    "rabbitmq-streams",
    "rabbitmq-upgrade",
];

/// Skip only known option values. A value containing a sensitive verb or flag
/// must not be mistaken for an operation. `--` makes all following words data.
fn positionals<'a>(args: &[&'a str], value_options: &[&str]) -> Vec<&'a str> {
    let mut result = Vec::new();
    let mut iter = args.iter().copied();
    while let Some(arg) = iter.next() {
        if arg == "--" {
            result.extend(iter);
            break;
        }
        if value_options.contains(&arg) {
            iter.next();
        } else if !arg.starts_with('-') {
            result.push(arg);
        }
        // Attached short values and --option=value are already one word.
    }
    result
}

/// Visit options without reinterpreting their values as options. Short values
/// can be attached (`-tfilter`); earlier short flags may be clustered (`-vnf`).
fn options<'a>(args: &[&'a str], value_options: &[&str]) -> Vec<(&'a str, Option<&'a str>)> {
    let mut result = Vec::new();
    let mut iter = args.iter().copied();
    while let Some(arg) = iter.next() {
        if arg == "--" {
            break;
        }
        if let Some((name, value)) = arg.strip_prefix("--").and_then(|a| a.split_once('=')) {
            result.push((&arg[..name.len() + 2], Some(value)));
        } else if arg.starts_with("--") {
            let value = value_options.contains(&arg).then(|| iter.next()).flatten();
            result.push((arg, value));
        } else if let Some(shorts) = arg.strip_prefix('-') {
            for (offset, ch) in shorts.char_indices() {
                let end = offset + ch.len_utf8();
                let name = if offset == 0 {
                    &arg[..end + 1]
                } else {
                    &shorts[offset..end]
                };
                // Subsequent clustered flags have no leading '-'. Keeping each
                // character's own slice also handles non-ASCII malformed flags.
                let takes_value = value_options.iter().any(|option| {
                    option.len() == 2 && option.starts_with('-') && option.ends_with(ch)
                });
                let value = if takes_value {
                    if end < shorts.len() {
                        Some(&shorts[end..])
                    } else {
                        iter.next()
                    }
                } else {
                    None
                };
                result.push((name, value));
                if takes_value {
                    break;
                }
            }
        }
    }
    result
}

fn short_is(name: &str, flag: char) -> bool {
    !name.starts_with("--") && name.ends_with(flag)
}

fn first_in(words: &[&str], operations: &[&str]) -> bool {
    words.first().is_some_and(|word| operations.contains(word))
}

fn path_is(words: &[&str], parent: &str, operations: &[&str]) -> bool {
    words.first() == Some(&parent) && first_in(&words[1..], operations)
}

// Value arity needed by the approval policy, independent of providers.
fn value_options(program: &str) -> &'static [&'static str] {
    match program {
        "pip" | "pip3" => &[
            "--python",
            "--log",
            "--keyring-provider",
            "--proxy",
            "--retries",
            "--timeout",
            "--exists-action",
            "--trusted-host",
            "--cert",
            "--client-cert",
            "--cache-dir",
            "--use-feature",
            "--use-deprecated",
            "--resume-retries",
        ],
        "npm" => &[
            "--prefix",
            "--workspace",
            "-w",
            "--location",
            "--depth",
            "--omit",
            "--include",
            "--registry",
            "--cache",
            "--userconfig",
            "--globalconfig",
            "--script-shell",
            "--loglevel",
            "--logs-max",
            "--logs-dir",
            "--proxy",
            "--https-proxy",
            "--noproxy",
            "--tag",
            "--access",
            "--otp",
            "--scope",
            "--before",
            "--user-agent",
        ],
        "cargo" => &[
            "--config",
            "--color",
            "-Z",
            "-C",
            "--explain",
            "--manifest-path",
            "--lockfile-path",
            "-p",
            "--package",
            "--bin",
            "--example",
            "--test",
            "--bench",
            "-F",
            "--features",
            "--target",
            "--target-dir",
            "--profile",
            "-j",
            "--jobs",
            "--registry",
            "--index",
            "--token",
        ],
        "zpool" => &["-o", "-R"],
        "zfs" => &["-o"],
        "cryptsetup" => &["--type", "--device", "--key-file", "-d"],
        "iptables" | "ip6tables" | "iptables-legacy" | "ip6tables-legacy" | "iptables-nft"
        | "ip6tables-nft" => &[
            "-t",
            "--table",
            "-j",
            "--jump",
            "-g",
            "--goto",
            "-m",
            "--match",
            "-s",
            "--source",
            "-d",
            "--destination",
            "-i",
            "--in-interface",
            "-o",
            "--out-interface",
            "-p",
            "--protocol",
        ],
        "nft" => &["-I", "--includepath", "-D", "--define", "-f", "--file"],
        "artisan" | "console" => &["--env", "-e", "--connection", "--database", "--path"],
        "firewall-cmd" => &["--zone", "--policy", "--name", "--path"],
        "pfctl" => &["-a", "-p", "-s", "-T", "-t", "-f", "-F", "-o"],
        "systemctl" => &[
            "-H",
            "--host",
            "-M",
            "--machine",
            "-p",
            "--property",
            "--root",
            "--image",
            "--image-policy",
            "-t",
            "--type",
            "--state",
            "--job-mode",
            "--message",
            "--preset-mode",
            "--kill-whom",
            "--kill-value",
            "-s",
            "--signal",
            "--legend",
            "-n",
            "--lines",
            "--output",
            "-o",
            "--timestamp",
            "--boot-loader-entry",
            "--boot-loader-menu",
        ],
        "nginx" => &["-c", "-p", "-g", "-e", "-s"],
        "httpd" | "apache2" => &["-d", "-f", "-C", "-c", "-D", "-k"],
        "apachectl" | "apache2ctl" => &["-d", "-f", "-C", "-c", "-D", "-k"],
        "caddy" => &["--config", "--adapter"],
        "pre-commit" => &["-c", "--config", "-t", "--hook-type"],
        "varnishadm" => &["-n", "-S", "-T", "-t"],
        "docker" | "podman" | "nerdctl" | "docker-compose" | "podman-compose" => &[
            "-H",
            "--host",
            "--context",
            "--config",
            "--connection",
            "--url",
            "--namespace",
            "-n",
            "-f",
            "--file",
            "-p",
            "--project-name",
            "--project-directory",
            "--env-file",
            "--profile",
        ],
        "certbot" => &[
            "--config",
            "-c",
            "--cert-name",
            "--work-dir",
            "--logs-dir",
            "--config-dir",
        ],
        "acme.sh" => &["--home", "--config-home", "--cert-home", "--domain", "-d"],
        "liquibase" => &[
            "--defaults-file",
            "--changelog-file",
            "--url",
            "--username",
            "--password",
            "--search-path",
        ],
        "prisma" => &["--schema", "--config"],
        "alembic" => &["-c", "--config", "-n", "--name", "-x"],
        "dbmate" => &[
            "-u",
            "--url",
            "-e",
            "--env",
            "-d",
            "--migrations-dir",
            "-s",
            "--schema-file",
        ],
        "knex" => &["--knexfile", "--cwd", "--env"],
        "goose" => &[
            "-dir",
            "--dir",
            "-table",
            "--table",
            "-certfile",
            "--certfile",
            "-ssl-cert",
            "-ssl-key",
            "-env",
            "--env",
        ],
        "migrate" => &[
            "-path",
            "--path",
            "-database",
            "--database",
            "-source",
            "--source",
            "-prefetch",
            "-lock-timeout",
        ],
        "typeorm" => &["-d", "--dataSource"],
        "sequelize-cli" | "sequelize" => &["--config", "--env", "--url", "--migrations-path"],
        "rails" | "rake" => &["-f", "--rakefile"],
        "restic" => &[
            "-r",
            "--repo",
            "--repository-file",
            "-p",
            "--password-file",
            "--password-command",
            "-o",
            "--option",
            "--cache-dir",
            "--host",
            "--tag",
            "--path",
        ],
        "borg" => &["--repo", "-r", "--remote-path", "--rsh", "--lock-wait"],
        "borgmatic" => &[
            "--config",
            "-c",
            "--repository",
            "--archive",
            "-a",
            "--match-archives",
            "--log-file",
            "--verbosity",
            "-v",
            "--override",
        ],
        "tarsnap" => &[
            "-f",
            "--keyfile",
            "--cachedir",
            "--configfile",
            "--exclude",
            "--include",
            "--exclude-from",
            "--include-from",
            "-C",
            "--maxbw",
            "--checkpoint-bytes",
        ],
        "rsnapshot" => &["-c"],
        "kopia" => &["--config-file", "--password", "--log-dir"],
        "kafkactl" => &["-C", "--config-file", "--context", "-o", "--output"],
        "kaf" => &[
            "-b",
            "--brokers",
            "-c",
            "--cluster",
            "--config",
            "--schema-registry",
            "-t",
            "--topic",
        ],
        "rpk" => &["--config", "-X", "--profile", "--brokers", "-o", "--format"],
        program if RABBITMQ.contains(&program) => &[
            "-n",
            "--node",
            "-p",
            "--vhost",
            "-t",
            "--timeout",
            "--formatter",
            "--printer",
            "--file",
            "--script-name",
            "--rabbitmq-home",
            "--data-dir",
            "--plugins-dir",
            "--enabled-plugins-file",
            "--aliases-file",
            "--erlang-cookie",
        ],
        "telepresence" => &["--context", "--namespace", "-n", "--output"],
        "garden" => &[
            "-e",
            "--env",
            "-l",
            "--log-level",
            "--logger-type",
            "-o",
            "--output",
            "--root",
            "--var",
        ],
        "tsh" => &[
            "--proxy",
            "--cluster",
            "--user",
            "-l",
            "--login",
            "--auth",
            "-i",
            "--identity",
            "--format",
            "-L",
            "--forward",
        ],
        "chezmoi" => &[
            "-S",
            "--source",
            "-D",
            "--destination",
            "-W",
            "--working-tree",
            "-c",
            "--config",
            "--config-format",
            "--cache",
            "--color",
            "--mode",
            "--persistent-state",
            "-o",
            "--output",
            "--progress",
            "-x",
            "--exclude",
            "-i",
            "--include",
            "--interactive-template-funcs",
            "--use-builtin-age",
            "--use-builtin-diff",
            "--use-builtin-git",
        ],
        "stow" => &[
            "-t",
            "--target",
            "-d",
            "--dir",
            "--ignore",
            "--defer",
            "--override",
        ],
        "vault" | "bao" => &[
            "-address",
            "-agent-address",
            "-namespace",
            "-ns",
            "-format",
            "-field",
            "-mount",
            "-header",
            "-wrap-ttl",
            "-mfa",
            "-ca-cert",
            "-ca-path",
            "-client-cert",
            "-client-key",
            "-tls-server-name",
            "-path",
            "-description",
            "-plugin-name",
        ],
        "nomad" => &[
            "-address",
            "-region",
            "-namespace",
            "-token",
            "-ca-cert",
            "-ca-path",
            "-client-cert",
            "-client-key",
            "-tls-server-name",
            "-t",
            "-eval-priority",
            "-deadline",
            "-job",
        ],
        "consul" => &[
            "-http-addr",
            "-grpc-addr",
            "-token",
            "-token-file",
            "-datacenter",
            "-namespace",
            "-partition",
            "-ca-file",
            "-ca-path",
            "-client-cert",
            "-client-key",
            "-tls-server-name",
            "-format",
            "-node",
            "-service",
            "-reason",
            "-prefix",
        ],
        "boundary" => &[
            "-addr",
            "-token",
            "-format",
            "-keyring-type",
            "-token-name",
            "-scope-id",
            "-id",
        ],
        "wg-quick" | "netbird" => &["--config", "-c"],
        "tailscale" => &["--socket"],
        "zerotier-cli" => &["-D", "-p", "-T"],
        "nmcli" => &[
            "-f",
            "--fields",
            "-g",
            "--get-values",
            "-m",
            "--mode",
            "-w",
            "--wait",
        ],
        "ip" => &["-n", "-netns", "-rcvbuf"],
        _ => &[],
    }
}

/// Known option values aren't destructive verbs. This also supplies the
/// existing generic delete/package checks, rather than giving them a different
/// interpretation of a known tool's arguments.
pub(super) fn verbs<'a>(program: &str, args: &[&'a str]) -> Vec<&'a str> {
    if crate::engine::native::is_php_interpreter_name(program)
        && let Some(name) = args.first().and_then(|script| php_application(script))
    {
        return verbs(name, &args[1..]);
    }
    positionals(args, value_options(program))
        .into_iter()
        .take(if matches!(program, "artisan" | "console") {
            1
        } else {
            3
        })
        .collect()
}

/// What an invocation writes or scans, by the words that name it: chezmoi's
/// targets after a writing subcommand, stow's packages, the directories both
/// are pointed at, and the hosts, URLs, and repositories nikto and trufflehog
/// scan. `None` when nothing is so named.
pub(super) fn targets<'a>(program: &str, args: &[&'a str]) -> Option<Vec<&'a str>> {
    let words = target_words(program, args);
    match program {
        "7z" | "7za" | "7zr" | "7zz" => {
            let directories: Vec<_> = args
                .iter()
                .copied()
                .filter(|arg| arg.starts_with("-o") && arg.len() > 2)
                .collect();
            (!directories.is_empty()).then_some(directories)
        }
        "chezmoi" => {
            let operation = positionals(args, value_options(program)).first().copied()?;
            [
                "apply",
                "add",
                "re-add",
                "update",
                "import",
                "chattr",
                "merge",
                "merge-all",
                "edit",
                "init",
                "forget",
            ]
            .contains(&operation)
            .then_some(words)
        }
        "stow" | "nikto" | "trufflehog" => Some(words),
        // Every argument of a network diagnostic or scanner may name the
        // host, address, port, or server it reaches (whois's `-h`); dig's
        // `+short` is a query option.
        "ping" | "ping6" | "traceroute" | "traceroute6" | "tracepath" | "mtr" | "whois"
        | "telnet" | "nc" | "ncat" | "netcat" | "socat" | "nmap" | "masscan" | "nslookup"
        | "host" | "dig" | "drill" | "arping" => Some(
            args.iter()
                .copied()
                .filter(|arg| !arg.is_empty() && !arg.starts_with(['-', '+']))
                .collect(),
        ),
        "tsh" => {
            first_in(&positionals(args, value_options(program)), TSH_CONNECTIONS).then_some(words)
        }
        _ => None,
    }
}

/// The words naming what [`targets`] reports, whatever the operation.
fn target_words<'a>(program: &str, args: &[&'a str]) -> Vec<&'a str> {
    let (places, skip) = match program {
        "chezmoi" => (CHEZMOI_PLACES, 1),
        "stow" => (STOW_PLACES, 0),
        "trufflehog" => (TRUFFLEHOG_PLACES, 1),
        "tsh" => (TSH_PLACES, 1),
        // nikto's options are single-dash words: `-h`, `-host`, `-url`.
        "nikto" => {
            let mut words = Vec::new();
            let mut iter = args.iter().copied();
            while let Some(arg) = iter.next() {
                let (name, value) = arg.split_once('=').unwrap_or((arg, ""));
                if ["-h", "-host", "--host", "-url", "--url"].contains(&name) {
                    words.extend(if value.is_empty() {
                        iter.next()
                    } else {
                        Some(value)
                    });
                }
            }
            return words;
        }
        _ => return Vec::new(),
    };
    let mut words: Vec<&str> = positionals(args, value_options(program))
        .into_iter()
        .skip(skip)
        .collect();
    words.extend(
        options(args, value_options(program))
            .into_iter()
            .filter(|(name, _)| places.contains(name))
            .filter_map(|(_, value)| value),
    );
    words
}

fn php_application(script: &str) -> Option<&'static str> {
    match script.rsplit(['/', '\\']).next()? {
        "artisan" => Some("artisan"),
        "console" => Some("console"),
        "composer" | "composer.phar" => Some("composer"),
        _ => None,
    }
}

pub(super) fn risk(program: &str, args: &[&str]) -> Option<&'static str> {
    match program {
        "crontab" => {
            let flags = options(args, &["-u"]);
            (flags
                .iter()
                .any(|(name, _)| short_is(name, 'r') || short_is(name, 'e'))
                || !positionals(args, &["-u"]).is_empty())
            .then_some("removes, edits, or installs scheduled commands")
        }
        "sed" => options(
            args,
            &["-e", "--expression", "-f", "--file", "-l", "--line-length"],
        )
        .iter()
        .any(|(name, _)| *name == "--in-place" || short_is(name, 'i'))
        .then_some("edits its input files in place"),
        "find" => {
            let mut words = args.iter().copied();
            let mut effect = None;
            while let Some(word) = words.next() {
                match word {
                    "-name" | "-iname" | "-path" | "-ipath" | "-wholename" | "-iwholename"
                    | "-regex" | "-iregex" | "-lname" | "-ilname" | "-newer" | "-anewer"
                    | "-cnewer" | "-newermt" | "-user" | "-group" | "-uid" | "-gid" | "-type"
                    | "-xtype" | "-perm" | "-size" | "-mtime" | "-atime" | "-ctime" | "-mmin"
                    | "-amin" | "-cmin" | "-maxdepth" | "-mindepth" | "-regextype" | "-printf" => {
                        words.next();
                    }
                    "-delete" => {
                        effect = Some("deletes matching files and directories");
                        break;
                    }
                    "-exec" | "-execdir" | "-ok" | "-okdir" => {
                        effect = Some("executes commands for matching paths");
                        break;
                    }
                    "-fprint" | "-fprint0" | "-fprintf" | "-fls" => {
                        effect = Some("writes matching paths to an output file");
                        break;
                    }
                    _ => {}
                }
            }
            effect
        }
        interpreter if crate::engine::native::is_php_interpreter_name(interpreter) => {
            // The first argument is a script application, not an option value.
            let script = args.first().filter(|script| !script.starts_with('-'))?;
            risk(php_application(script)?, &args[1..])
        }
        "artisan" | "console" => first_in(
            &positionals(args, value_options(program)),
            &[
                "migrate",
                "migrate:fresh",
                "migrate:refresh",
                "migrate:reset",
                "migrate:rollback",
                "db:wipe",
                "db:seed",
                "doctrine:migrations:migrate",
                "doctrine:database:drop",
                "doctrine:schema:drop",
                "doctrine:migrations:execute",
            ],
        )
        .then_some(MIGRATION),
        "npm" => npm_risk(args),
        "cargo" => cargo_risk(args),
        "dotnet" => {
            // Builds evaluate MSBuild; run/test and external tools run code;
            // new, package, workload, and configuration operations can write.
            // Only an exact host information query has no operation to run.
            (!(args.len() == 1
                && ["--version", "--info", "--list-sdks", "--list-runtimes"].contains(&args[0]))
                && !args.is_empty())
            .then_some(
                "can evaluate project code, execute tools, or change .NET files and packages",
            )
        }
        "npx" => Some("can execute package code"),
        "pip" | "pip3" => options(args, value_options(program))
            .iter()
            .any(|(name, _)| *name == "--log")
            .then_some("writes to a log file"),
        "newfs" | "gdisk" | "cgdisk" | "sgdisk" | "lvremove" | "vgremove" | "pvremove" => {
            Some(DISK)
        }
        p if p.starts_with("newfs_") => Some(DISK),
        "userdel" | "deluser" | "groupdel" | "delgroup" => Some(USER),
        "diskutil" => {
            let words = positionals(args, value_options(program));
            (first_in(
                &words,
                &[
                    "eraseDisk",
                    "eraseVolume",
                    "secureErase",
                    "zeroDisk",
                    "partitionDisk",
                    "partitionVolume",
                    "resizeVolume",
                    "mergePartitions",
                ],
            ) || path_is(
                &words,
                "apfs",
                &[
                    "deleteContainer",
                    "deleteVolume",
                    "eraseVolume",
                    "resizeContainer",
                ],
            ) || path_is(&words, "coreStorage", &["delete", "deleteVolume", "revert"]))
            .then_some(DISK)
        }
        "zpool" => first_in(
            &positionals(args, value_options(program)),
            &["destroy", "remove", "labelclear", "clear"],
        )
        .then_some(DISK),
        "zfs" => first_in(
            &positionals(args, value_options(program)),
            &["destroy", "rollback"],
        )
        .then_some(DISK),
        "cryptsetup" => first_in(
            &positionals(args, value_options(program)),
            &[
                "luksFormat",
                "erase",
                "luksErase",
                "luksKillSlot",
                "remove",
                "close",
            ],
        )
        .then_some(DISK),
        "iptables" | "ip6tables" | "iptables-legacy" | "ip6tables-legacy" | "iptables-nft"
        | "ip6tables-nft" => options(args, value_options(program))
            .iter()
            .any(|(name, _)| {
                [
                    "--append",
                    "--delete",
                    "--insert",
                    "--replace",
                    "--flush",
                    "--delete-chain",
                    "--policy",
                    "--new-chain",
                    "--rename-chain",
                    "--zero",
                ]
                .contains(name)
                    || ['A', 'D', 'I', 'R', 'F', 'X', 'P', 'N', 'E', 'Z']
                        .iter()
                        .any(|ch| short_is(name, *ch))
            })
            .then_some(FIREWALL),
        "nft" => {
            let words = positionals(args, value_options(program));
            // nft joins its argv into its own language. A quoted script or
            // multi-statement argument must not hide operations from this gate.
            (first_in(
                &words,
                &[
                    "add", "delete", "destroy", "insert", "replace", "flush", "reset",
                ],
            ) || words.iter().any(|word| {
                word.chars()
                    .any(|ch| ch.is_whitespace() || matches!(ch, ';' | '{' | '}' | '\\'))
            }) || options(args, value_options(program))
                .iter()
                .any(|(name, _)| *name == "--file" || short_is(name, 'f')))
            .then_some(FIREWALL)
        }
        "ufw" => first_in(
            &positionals(args, value_options(program)),
            &[
                "enable", "disable", "reset", "default", "allow", "deny", "reject", "limit",
                "delete", "insert", "prepend", "route", "reload",
            ],
        )
        .then_some(FIREWALL),
        "firewall-cmd" => options(args, value_options(program))
            .iter()
            .any(|(name, _)| {
                name.starts_with("--add-")
                    || name.starts_with("--remove-")
                    || name.starts_with("--set-")
                    || [
                        "--reload",
                        "--complete-reload",
                        "--panic-on",
                        "--panic-off",
                        "--runtime-to-permanent",
                        "--new-zone",
                        "--delete-zone",
                        "--new-policy",
                        "--delete-policy",
                    ]
                    .contains(name)
            })
            .then_some(FIREWALL),
        "pfctl" => options(args, value_options(program))
            .iter()
            .any(|(name, value)| {
                ['F', 'f', 'd', 'e', 'k', 'K']
                    .iter()
                    .any(|ch| short_is(name, *ch))
                    || short_is(name, 'T')
                        && value.is_some_and(|v| {
                            ["add", "delete", "flush", "kill", "replace", "load", "zero"]
                                .contains(&v)
                        })
            })
            .then_some(FIREWALL),
        "systemctl" => {
            let words = positionals(args, value_options(program));
            if first_in(
                &words,
                &[
                    "poweroff",
                    "reboot",
                    "halt",
                    "kexec",
                    "suspend",
                    "hibernate",
                    "hybrid-sleep",
                    "suspend-then-hibernate",
                    "soft-reboot",
                ],
            ) {
                Some(SHUTDOWN)
            } else {
                first_in(
                    &words,
                    &[
                        "stop",
                        "restart",
                        "try-restart",
                        "condrestart",
                        "reload-or-restart",
                        "try-reload-or-restart",
                        "reload-or-try-restart",
                        "isolate",
                        "kill",
                        "disable",
                        "mask",
                    ],
                )
                .then_some(SERVER)
            }
        }
        // macOS's directory service: records are users and groups.
        "dscl" => {
            let operations: Vec<&str> =
                args.iter().map(|arg| arg.trim_start_matches('-')).collect();
            if operations.contains(&"delete") {
                Some(USER)
            } else {
                operations
                    .iter()
                    .any(|operation| {
                        ["create", "change", "append", "merge", "passwd"].contains(operation)
                    })
                    .then_some(ACCOUNTS)
            }
        }
        "sysadminctl" => {
            let set = |name: &str| {
                args.windows(2)
                    .any(|pair| pair[0] == name && pair[1] != "status")
            };
            if args.contains(&"-deleteUser") {
                Some(USER)
            } else if args.iter().any(|arg| {
                [
                    "-addUser",
                    "-resetPasswordFor",
                    "-secureTokenOn",
                    "-secureTokenOff",
                ]
                .contains(arg)
            }) {
                Some(ACCOUNTS)
            } else {
                [
                    "-guestAccount",
                    "-afpGuestAccess",
                    "-smbGuestAccess",
                    "-automaticTime",
                    "-screenLock",
                    "-autologin",
                ]
                .into_iter()
                .any(set)
                .then_some(SYSTEM)
            }
        }
        // `-setremotelogin on` among them.
        "systemsetup" => args
            .iter()
            .any(|arg| arg.starts_with("-set") || *arg == "-deletenetworktimeserver")
            .then_some(SYSTEM),
        // `pmset -g` reads; everything else sets, schedules, or sleeps.
        "pmset" => {
            if args
                .iter()
                .any(|arg| ["sleepnow", "displaysleepnow"].contains(arg))
            {
                Some(SHUTDOWN)
            } else {
                args.first()
                    .is_some_and(|first| *first != "-g")
                    .then_some(SYSTEM)
            }
        }
        "sysctl" => args
            .iter()
            .take_while(|arg| **arg != "--")
            .any(|arg| {
                arg.contains('=')
                    || arg.starts_with('-') && !arg.starts_with("--") && arg.contains('w')
            })
            .then_some(SYSTEM),
        "csrutil" => first_in(
            &positionals(args, value_options(program)),
            &[
                "disable",
                "enable",
                "clear",
                "authenticated-root",
                "netboot",
            ],
        )
        .then_some(SYSTEM),
        "nvram" => args
            .iter()
            .any(|arg| arg.contains('=') || ["-d", "-c", "-f"].contains(arg))
            .then_some(SYSTEM),
        "adb" => first_in(
            &positionals(args, &["-s", "-t", "-H", "-P", "-L"]),
            &[
                "install",
                "install-multiple",
                "install-multi-package",
                "uninstall",
                "push",
                "sync",
                "sideload",
                "restore",
                "reboot",
                "root",
                "unroot",
                "remount",
                "disable-verity",
                "enable-verity",
            ],
        )
        .then_some(DEVICE),
        "fastboot" => first_in(
            &positionals(
                args,
                &["-s", "-S", "--slot", "-b", "--base", "-c", "--cmdline"],
            ),
            &[
                "flash",
                "flashall",
                "flashing",
                "erase",
                "format",
                "update",
                "boot",
                "oem",
                "set_active",
                "wipe-super",
                "create-logical-partition",
                "delete-logical-partition",
                "resize-logical-partition",
                "reboot",
                "-w",
            ],
        )
        .then_some(FLASH),
        // The active developer directory is system-wide.
        "xcode-select" => args
            .iter()
            .any(|arg| {
                let name = arg.split('=').next().unwrap_or(arg);
                ["-s", "--switch", "-r", "--reset", "--install"].contains(&name)
            })
            .then_some(SYSTEM),
        "xcodes" => first_in(
            &positionals(args, value_options(program)),
            &["install", "uninstall", "select", "update", "runtimes"],
        )
        .then_some(PACKAGES),
        "pod" => first_in(
            &positionals(args, value_options(program)),
            &[
                "install",
                "update",
                "deintegrate",
                "cache",
                "repo",
                "setup",
                "trunk",
            ],
        )
        .then_some(PACKAGES),
        "carthage" => first_in(
            &positionals(args, value_options(program)),
            &["bootstrap", "update", "checkout", "build"],
        )
        .then_some(PACKAGES),
        // Editors that install extensions from their marketplaces; `tunnel`
        // opens remote access to this machine.
        "code" | "code-insiders" | "codium" | "cursor" | "windsurf" => {
            if args.iter().take_while(|arg| **arg != "--").any(|arg| {
                let name = arg.split('=').next().unwrap_or(arg);
                [
                    "--install-extension",
                    "--uninstall-extension",
                    "--update-extensions",
                ]
                .contains(&name)
            }) {
                Some(PACKAGES)
            } else {
                first_in(&positionals(args, value_options(program)), &["tunnel"]).then_some(NETWORK)
            }
        }
        // husky has no help: every run but its deprecated commands sets
        // core.hooksPath and writes hooks there (`init` edits package.json).
        "husky" => (!args
            .first()
            .is_some_and(|word| ["add", "set", "uninstall"].contains(word)))
        .then_some(HOOKS),
        "lefthook" => first_in(
            &positionals(args, value_options(program)),
            &["install", "uninstall", "add"],
        )
        .then_some(HOOKS),
        "pre-commit" => first_in(
            &positionals(args, value_options(program)),
            &["install", "uninstall", "install-hooks", "init-templatedir"],
        )
        .then_some(HOOKS),
        "service" => {
            let words = positionals(args, value_options(program));
            words
                .get(1)
                .is_some_and(|word| ["stop", "restart", "force-reload"].contains(word))
                .then_some(SERVER)
        }
        "launchctl" => {
            let words = positionals(args, value_options(program));
            // `kickstart -k` kills a running instance before restarting it.
            (first_in(
                &words,
                &[
                    "bootout", "remove", "stop", "kill", "unload", "reboot", "disable",
                ],
            ) || first_in(&words, &["kickstart"])
                && args
                    .iter()
                    .any(|arg| arg.starts_with('-') && !arg.starts_with("--") && arg.contains('k')))
            .then_some(SERVER)
        }
        "nginx" => options(args, value_options(program))
            .iter()
            .any(|(name, value)| {
                short_is(name, 's')
                    && value.is_some_and(|v| ["stop", "quit", "reload", "reopen"].contains(&v))
            })
            .then_some(SERVER),
        "httpd" | "apache2" => options(args, value_options(program))
            .iter()
            .any(|(name, value)| {
                short_is(name, 'k')
                    && value.is_some_and(|v| {
                        ["stop", "restart", "graceful", "graceful-stop"].contains(&v)
                    })
            })
            .then_some(SERVER),
        "apachectl" | "apache2ctl" => (first_in(
            &positionals(args, value_options(program)),
            &["stop", "restart", "graceful", "graceful-stop"],
        ) || options(args, value_options(program)).iter().any(
            |(name, value)| {
                short_is(name, 'k')
                    && value.is_some_and(|v| {
                        ["stop", "restart", "graceful", "graceful-stop"].contains(&v)
                    })
            },
        ))
        .then_some(SERVER),
        "caddy" => {
            let words = positionals(args, value_options(program));
            if first_in(&words, &["stop", "reload"]) {
                Some(SERVER)
            } else if first_in(&words, &["trust", "untrust"]) {
                Some(TRUST_STORE)
            } else {
                // They download a new build and replace the running binary.
                first_in(&words, &["upgrade", "add-package", "remove-package"]).then_some(PACKAGES)
            }
        }
        // `-sf`/`-st` tell the old processes to finish or terminate.
        "haproxy" => args
            .iter()
            .take_while(|arg| **arg != "--")
            .any(|arg| ["-sf", "-st"].contains(arg))
            .then_some(SERVER),
        "varnishadm" => {
            first_in(&positionals(args, value_options(program)), &["stop"]).then_some(SERVER)
        }
        "docker" | "podman" | "nerdctl" | "docker-compose" | "podman-compose" => {
            let words = positionals(args, value_options(program));
            (first_in(&words, &["stop", "restart", "kill", "down"])
                || path_is(&words, "container", &["stop", "restart", "kill"])
                || path_is(&words, "pod", &["stop", "restart", "kill"])
                || path_is(&words, "compose", &["stop", "restart", "kill", "down"]))
            .then_some(SERVER)
        }
        "certbot" => {
            let words = positionals(args, value_options(program));
            if first_in(&words, &["delete", "revoke", "unregister"]) {
                Some(CERTIFICATE)
            } else {
                first_in(&words, &["rollback"]).then_some(SERVER_CONFIG)
            }
        }
        "acme.sh" => options(args, value_options(program))
            .iter()
            .any(|(name, _)| {
                [
                    "--remove",
                    "--revoke",
                    "--deactivate",
                    "--deactivate-account",
                ]
                .contains(name)
            })
            .then_some(CERTIFICATE),
        // Go flags: `-install` and `--install` are one flag.
        "mkcert" => go_flag_names(args)
            .iter()
            .any(|name| ["install", "uninstall"].contains(name))
            .then_some(TRUST_STORE),
        "flyway" => {
            first_in(&positionals(args, value_options(program)), &["clean"]).then_some(MIGRATION)
        }
        "liquibase" => {
            let words = positionals(args, value_options(program));
            first_in(
                &words,
                &[
                    "dropAll",
                    "drop-all",
                    "rollback",
                    "rollbackCount",
                    "rollback-count",
                    "rollbackToDate",
                    "rollback-to-date",
                    "rollbackOneChangeset",
                    "rollback-one-changeset",
                    "rollbackOneUpdate",
                    "rollback-one-update",
                ],
            )
            .then_some(MIGRATION)
        }
        "prisma" => {
            let words = positionals(args, value_options(program));
            (path_is(&words, "migrate", &["reset"])
                || path_is(&words, "db", &["push"])
                    && args
                        .iter()
                        .any(|arg| ["--force-reset", "--accept-data-loss"].contains(arg)))
            .then_some(MIGRATION)
        }
        "alembic" => first_in(&positionals(args, value_options(program)), &["downgrade"])
            .then_some(MIGRATION),
        "dbmate" => first_in(
            &positionals(args, value_options(program)),
            &["drop", "down", "rollback"],
        )
        .then_some(MIGRATION),
        // `goose [options] [DRIVER DBSTRING] COMMAND`: the operation can
        // follow the driver and connection string.
        "goose" => positionals(args, value_options(program))
            .iter()
            .take(3)
            .any(|word| ["down", "down-to", "reset", "redo"].contains(word))
            .then_some(MIGRATION),
        // golang-migrate: `force` and `goto` move the recorded version too.
        "migrate" => first_in(
            &positionals(args, value_options(program)),
            &["down", "force", "goto"],
        )
        .then_some(MIGRATION),
        // knex spells its commands `migrate:rollback` (knex 3).
        "knex" => {
            let words = positionals(args, value_options(program));
            (first_in(&words, &["migrate:rollback", "migrate:down"])
                || path_is(&words, "migrate", &["rollback"]))
            .then_some(MIGRATION)
        }
        "typeorm" => first_in(
            &positionals(args, value_options(program)),
            &["migration:revert", "schema:drop"],
        )
        .then_some(MIGRATION),
        "sequelize-cli" | "sequelize" => first_in(
            &positionals(args, value_options(program)),
            &["db:migrate:undo", "db:migrate:undo:all", "db:drop"],
        )
        .then_some(MIGRATION),
        "rails" | "rake" => first_in(
            &positionals(args, value_options(program)),
            &[
                "db:drop",
                "db:reset",
                "db:rollback",
                "db:schema:load",
                "db:structure:load",
            ],
        )
        .then_some(MIGRATION),
        "restic" => first_in(
            &positionals(args, value_options(program)),
            &["forget", "prune"],
        )
        .then_some(BACKUP),
        "borg" => first_in(
            &positionals(args, value_options(program)),
            &["delete", "prune", "compact"],
        )
        .then_some(BACKUP),
        "borgmatic" => {
            let words = positionals(args, value_options(program));
            let flags = options(args, value_options(program));
            let print_only = flags.iter().any(|(name, _)| {
                [
                    "--help",
                    "--version",
                    "--bash-completion",
                    "--fish-completion",
                ]
                .contains(name)
                    || short_is(name, 'h')
            });
            // With no explicit actions, borgmatic's default actions include
            // prune and compact. It can also run multiple explicit actions.
            (!print_only
                && (!first_in(
                    &words,
                    &[
                        "list",
                        "repo-list",
                        "rlist",
                        "info",
                        "repo-info",
                        "rinfo",
                        "diff",
                    ],
                ) || words.iter().any(|word| {
                    [
                        "delete",
                        "repo-delete",
                        "rdelete",
                        "prune",
                        "compact",
                        "recreate",
                    ]
                    .contains(word)
                }) || flags.iter().any(|(name, _)| {
                    !value_options(program).contains(name)
                        && !["--dry-run", "--no-environment-interpolation"].contains(name)
                        && !short_is(name, 'n')
                }) || flags
                    .iter()
                    .any(|(name, _)| ["--default-actions", "--prune", "--compact"].contains(name))))
            .then_some(BACKUP)
        }
        "tarsnap" => options(args, value_options(program))
            .iter()
            .any(|(name, _)| short_is(name, 'd') || ["--nuke", "--fsck-prune"].contains(name))
            .then_some(BACKUP),
        "rsnapshot" => {
            let words = positionals(args, value_options(program));
            let dry_run = options(args, value_options(program))
                .iter()
                .any(|(name, _)| short_is(name, 't'));
            (!dry_run
                && words
                    .first()
                    .is_some_and(|word| !["configtest", "du", "diff"].contains(word)))
            .then_some(BACKUP)
        }
        "duplicity" => positionals(args, value_options(program))
            .first()
            .is_some_and(|word| word.starts_with("remove-"))
            .then_some(BACKUP),
        "kopia" => {
            let words = positionals(args, value_options(program));
            (path_is(&words, "snapshot", &["delete"]) || path_is(&words, "repository", &["delete"]))
                .then_some(BACKUP)
        }
        "tmutil" => first_in(
            &positionals(args, value_options(program)),
            &["delete", "deletelocalsnapshots", "thinlocalsnapshots"],
        )
        .then_some(BACKUP),
        "kafkactl" => {
            first_in(&positionals(args, value_options(program)), &["reset"]).then_some(OFFSETS)
        }
        "kaf" => path_is(
            &positionals(args, value_options(program)),
            "group",
            &["commit"],
        )
        .then_some(OFFSETS),
        "rpk" => path_is(
            &positionals(args, value_options(program)),
            "group",
            &["seek"],
        )
        .then_some(OFFSETS),
        // Without --execute, --reset-offsets only prints the planned offsets.
        "kafka-consumer-groups" | "kafka-consumer-groups.sh" => {
            let names: Vec<&str> = options(args, &[]).iter().map(|(name, _)| *name).collect();
            (names.contains(&"--delete-offsets")
                || names.contains(&"--reset-offsets") && names.contains(&"--execute"))
            .then_some(OFFSETS)
        }
        "rabbitmq-plugins"
            if !options(args, value_options(program))
                .iter()
                .any(|(name, _)| ["--help", "-?"].contains(name)) =>
        {
            first_in(
                &positionals(args, value_options(program)),
                &["enable", "disable", "set"],
            )
            .then_some(PLUGINS)
        }
        // The tools only print help when asked for it anywhere on the line.
        program
            if RABBITMQ.contains(&program)
                && options(args, value_options(program))
                    .iter()
                    .any(|(name, _)| ["--help", "-?"].contains(name)) =>
        {
            None
        }
        program if RABBITMQ.contains(&program) => positionals(args, value_options(program))
            .first()
            .is_some_and(|verb| {
                [
                    "delete_", "purge_", "clear_", "close_", "force_", "forget_", "reset_",
                    "restart_", "stop_", "eval_",
                ]
                .iter()
                .any(|prefix| verb.starts_with(prefix))
                    || [
                        "reset",
                        "stop",
                        "shutdown",
                        "eval",
                        "exec",
                        "remote_shell",
                        "import_definitions",
                        "suspend_listeners",
                        "join_cluster",
                        "rename_cluster_node",
                        "update_cluster_nodes",
                        "change_password",
                        "disable_vhost_deletion_protection",
                        "drain",
                        "post_upgrade",
                        "shrink",
                        "rebalance",
                        "transfer_leadership",
                    ]
                    .contains(verb)
            })
            .then_some(MESSAGING),
        "vault" | "bao" if !hashicorp_help(args) => {
            let words = positionals(args, value_options(program));
            let flags = go_flag_names(args);
            (["secrets", "auth", "audit"]
                .iter()
                .any(|mount| path_is(&words, mount, &["disable"]))
                || ["secrets", "auth"]
                    .iter()
                    .any(|mount| path_is(&words, mount, &["move"]))
                || path_is(&words, "kv", &["rollback"])
                || path_is(
                    &words,
                    "operator",
                    &["seal", "step-down", "rekey", "generate-root"],
                )
                || words.starts_with(&["operator", "raft", "snapshot", "restore"])
                || flags.iter().any(|flag| ["force", "f"].contains(flag)))
            .then_some(SECRETS)
        }
        "nomad" if !hashicorp_help(args) => {
            let words = positionals(args, value_options(program));
            let flags = go_flag_names(args);
            (first_in(&words, &["stop"])
                || path_is(&words, "job", &["stop", "revert"])
                || path_is(&words, "alloc", &["stop", "restart", "signal"])
                || path_is(&words, "node", &["drain", "eligibility"])
                || path_is(&words, "system", &["gc"])
                || path_is(&words, "server", &["force-leave"])
                || path_is(&words, "deployment", &["fail"])
                || words.starts_with(&["operator", "snapshot", "restore"])
                || flags.iter().any(|flag| ["purge", "force"].contains(flag)))
            .then_some(SCHEDULER)
        }
        "consul" if !hashicorp_help(args) => {
            let words = positionals(args, value_options(program));
            // Without -enable or -disable, maint only reports the status.
            (first_in(&words, &["leave", "force-leave", "exec", "lock"])
                || first_in(&words, &["maint"])
                    && go_flag_names(args)
                        .iter()
                        .any(|flag| ["enable", "disable"].contains(flag))
                || path_is(&words, "snapshot", &["restore"]))
            .then_some(AGENTS)
        }
        // `apply -destroy` is `destroy`; `plan -destroy` only plans it.
        "terraform" | "tofu" if !hashicorp_help(args) => {
            let words = positionals(args, &["-chdir", "-var", "-var-file", "-target"]);
            (first_in(&words, &["apply"])
                && go_flag_names(args)
                    .iter()
                    .any(|flag| ["destroy", "replace"].contains(flag))
                || first_in(&words, &["force-unlock", "taint"])
                || path_is(&words, "state", &["push", "replace-provider"]))
            .then_some(INFRASTRUCTURE)
        }
        "packer" if !hashicorp_help(args) => (first_in(&positionals(args, &[]), &["build"])
            && go_flag_names(args).contains(&"force"))
        .then_some(ARTIFACTS),
        // -prune-retain only matters with -prune.
        "waypoint" if !hashicorp_help(args) => {
            go_flag_names(args).contains(&"prune").then_some(ARTIFACTS)
        }
        "gpg" | "gpg2" => options(args, &[])
            .iter()
            .any(|(name, _)| name.starts_with("--delete-"))
            .then_some(KEYS),
        // -R removes a host's keys from known_hosts (-f names another file).
        "ssh-keygen" => options(args, &["-f", "-F", "-t", "-b", "-C", "-N", "-P"])
            .iter()
            .any(|(name, _)| short_is(name, 'R'))
            .then_some(KEYS),
        // zip's short options are whole words (`-ds` is not `-d -s`).
        "zip" => args
            .iter()
            .take_while(|arg| **arg != "--")
            .any(|arg| *arg == "-m" || *arg == "--move")
            .then_some(MOVES),
        // Extraction or compression that replaces existing files silently.
        "unzip" => options(args, &["-d", "-P", "-x"])
            .iter()
            .any(|(name, _)| short_is(name, 'o'))
            .then_some(OVERWRITES),
        "7z" | "7za" | "7zr" | "7zz" => args
            .iter()
            .take_while(|arg| **arg != "--")
            .any(|arg| *arg == "-y" || arg.starts_with("-aoa") || arg.starts_with("-aou"))
            .then_some(OVERWRITES),
        "tar" | "bsdtar" | "gtar" => args
            .iter()
            .take_while(|arg| **arg != "--")
            .any(|arg| ["--overwrite", "--unlink-first", "--recursive-unlink"].contains(arg))
            .then_some(OVERWRITES),
        "gzip" | "gunzip" | "pigz" | "unpigz" | "bzip2" | "bunzip2" | "pbzip2" | "xz" | "unxz"
        | "lzma" | "unlzma" | "zstd" | "unzstd" | "zstdmt" | "lz4" | "unlz4" => options(
            args,
            &[
                "-S", "--suffix", "-o", "-T", "-D", "-p", "-b", "-C", "-F", "-M",
            ],
        )
        .iter()
        .any(|(name, _)| short_is(name, 'f') || *name == "--force" || *name == "--rm")
        .then_some(OVERWRITES),
        // brotli's -j removes its sources; compress -f replaces outputs.
        "brotli" => options(args, &["-o", "-q", "-w", "-C", "-D", "-S"])
            .iter()
            .any(|(name, _)| {
                short_is(name, 'f') || short_is(name, 'j') || ["--force", "--rm"].contains(name)
            })
            .then_some(OVERWRITES),
        "compress" | "uncompress" => options(args, &["-b"])
            .iter()
            .any(|(name, _)| short_is(name, 'f'))
            .then_some(OVERWRITES),
        "lzip" | "plzip" | "lunzip" | "clzip" => {
            options(args, &["-b", "-m", "-o", "-s", "-S", "-B", "-n"])
                .iter()
                .any(|(name, _)| short_is(name, 'f') || *name == "--force")
                .then_some(OVERWRITES)
        }
        // -U deletes the inputs.
        "lzop" => options(args, &["-o", "-S"])
            .iter()
            .any(|(name, _)| {
                short_is(name, 'f') || short_is(name, 'U') || ["--force", "--delete"].contains(name)
            })
            .then_some(OVERWRITES),
        // ouch -y answers its overwrite prompts; -r removes the sources.
        "ouch" => options(
            args,
            &[
                "-f",
                "-p",
                "-c",
                "-d",
                "-l",
                "--format",
                "--password",
                "--threads",
                "--dir",
                "--level",
            ],
        )
        .iter()
        .any(|(name, _)| {
            short_is(name, 'y') || short_is(name, 'r') || ["--yes", "--remove"].contains(name)
        })
        .then_some(OVERWRITES),
        "unar" => args
            .iter()
            .any(|arg| ["-f", "-force-overwrite", "--force-overwrite"].contains(arg))
            .then_some(OVERWRITES),
        "dtrx" => options(args, &["-p", "--one", "--one-entry", "--password"])
            .iter()
            .any(|(name, _)| short_is(name, 'o') || *name == "--overwrite")
            .then_some(OVERWRITES),
        "atool" | "aunpack" | "apack" | "arepack" => options(
            args,
            &[
                "-X",
                "-F",
                "-O",
                "-V",
                "-o",
                "--extract-to",
                "--format",
                "--format-option",
                "--verbosity",
                "--option",
                "--config",
                "--save-outdir",
            ],
        )
        .iter()
        .any(|(name, _)| short_is(name, 'f') || *name == "--force")
        .then_some(OVERWRITES),
        "unsquashfs" => args
            .iter()
            .any(|arg| ["-f", "-force"].contains(arg))
            .then_some(OVERWRITES),
        "clamscan" | "clamdscan" => options(args, &[])
            .iter()
            .any(|(name, _)| ["--remove", "--move", "--copy"].contains(name))
            .then_some(QUARANTINE),
        "direnv" => first_in(
            &positionals(args, &[]),
            &["allow", "permit", "grant", "edit", "exec"],
        )
        .then_some(ENVRC),
        // `--delete` is already a generic deletion flag.
        "stow" => options(args, value_options(program))
            .iter()
            .any(|(name, _)| short_is(name, 'D'))
            .then_some(LINKS),
        "boundary" if !hashicorp_help(args) => path_is(
            &positionals(args, value_options(program)),
            "sessions",
            &["cancel"],
        )
        .then_some(SESSIONS),
        "telepresence" => first_in(
            &positionals(args, value_options(program)),
            &[
                "connect",
                "intercept",
                "replace",
                "wiretap",
                "leave",
                "quit",
                "disconnect",
            ],
        )
        .then_some(NETWORK),
        // garden cleanup deletes Deploys or the whole namespace; plugin
        // commands are provider operations (cluster-init, registry cleanup).
        "garden" => {
            let words = positionals(args, value_options(program));
            if first_in(&words, &["self-update"]) {
                Some(PACKAGES)
            } else {
                first_in(&words, &["cleanup", "plugins"]).then_some(CLUSTER)
            }
        }
        "wg-quick" | "netbird" => {
            first_in(&positionals(args, value_options(program)), &["up", "down"]).then_some(NETWORK)
        }
        "wg" => first_in(
            &positionals(args, value_options(program)),
            &["set", "setconf", "addconf", "syncconf"],
        )
        .then_some(NETWORK),
        "tailscale" => first_in(
            &positionals(args, value_options(program)),
            &["up", "down", "set", "switch", "logout"],
        )
        .then_some(NETWORK),
        "zerotier-cli" => first_in(
            &positionals(args, value_options(program)),
            &["join", "leave", "set"],
        )
        .then_some(NETWORK),
        "nmcli" => {
            let words = positionals(args, value_options(program));
            (path_is(
                &words,
                "connection",
                &["up", "down", "modify", "delete", "add", "clone", "import"],
            ) || path_is(
                &words,
                "con",
                &["up", "down", "modify", "delete", "add", "clone", "import"],
            ) || path_is(
                &words,
                "device",
                &["connect", "disconnect", "modify", "reapply"],
            ) || path_is(
                &words,
                "dev",
                &["connect", "disconnect", "modify", "reapply"],
            ))
            .then_some(NETWORK)
        }
        // macOS: services, locations, DNS, proxies, and Wi-Fi settings.
        "networksetup" => args
            .iter()
            .any(|arg| {
                [
                    "-set",
                    "-create",
                    "-delete",
                    "-remove",
                    "-add",
                    "-rename",
                    "-import",
                    "-order",
                    "-switchto",
                ]
                .iter()
                .any(|verb| arg.starts_with(verb))
            })
            .then_some(NETWORK),
        "route" => first_in(
            &positionals(args, value_options(program)),
            &["add", "delete", "change", "flush"],
        )
        .then_some(NETWORK),
        // `ifconfig en0` reads; anything after the interface configures it.
        "ifconfig" => (positionals(args, value_options(program)).len() > 1).then_some(NETWORK),
        "arp" => args
            .iter()
            .take_while(|arg| **arg != "--")
            .filter(|arg| arg.starts_with('-') && !arg.starts_with("--"))
            .any(|arg| arg[1..].chars().any(|c| matches!(c, 'd' | 's' | 'S' | 'f')))
            .then_some(NETWORK),
        "scutil" => (args.contains(&"--set")
            || args.windows(2).any(|pair| {
                pair[0] == "--nc" && ["start", "stop", "enablepreference"].contains(&pair[1])
            }))
        .then_some(NETWORK),
        "ip" => {
            let words = positionals(args, value_options(program));
            (path_is(&words, "link", &["set", "add", "delete"])
                || ["address", "addr", "route", "rule", "tunnel"]
                    .iter()
                    .any(|object| {
                        path_is(
                            &words,
                            object,
                            &["add", "delete", "del", "change", "replace", "flush"],
                        )
                    }))
            .then_some(NETWORK)
        }
        // gh copilot downloads the Copilot CLI on first use and runs it
        // (or removes it with --remove); extensions install programs.
        "gh" if !args
            .iter()
            .take_while(|arg| **arg != "--")
            .any(|arg| matches!(*arg, "-h" | "--help")) =>
        {
            let words = positionals(args, &["-R", "--repo", "--hostname"]);
            (first_in(&words, &["copilot"])
                || words
                    .first()
                    .is_some_and(|w| matches!(*w, "extension" | "extensions" | "ext"))
                    && first_in(&words[1..], &["install", "remove", "upgrade"]))
            .then_some(PACKAGES)
        }
        "bundle" | "bundler" => first_in(
            &positionals(args, &["--gemfile", "--path", "--retry", "--jobs", "-j"]),
            &[
                "install", "add", "update", "remove", "clean", "pristine", "cache", "package",
                "plugin", "binstubs",
            ],
        )
        .then_some(PACKAGES),
        "composer" => first_in(
            &positionals(args, &["--working-dir", "-d"]),
            &[
                "install",
                "i",
                "require",
                "update",
                "u",
                "upgrade",
                "remove",
                "rm",
                "reinstall",
                "create-project",
                "global",
                "self-update",
                "selfupdate",
                "bump",
            ],
        )
        .then_some(PACKAGES),
        // uv's installers sit below `pip`, `tool`, `python`, and `self` too.
        "uv" => {
            let words = positionals(
                args,
                &[
                    "--directory",
                    "--project",
                    "--python",
                    "-p",
                    "--index",
                    "--with",
                ],
            );
            (first_in(&words, &["add", "remove", "sync", "lock"])
                || path_is(&words, "pip", &["install", "uninstall", "sync"])
                || path_is(
                    &words,
                    "tool",
                    &["install", "uninstall", "upgrade", "update-shell"],
                )
                || path_is(
                    &words,
                    "python",
                    &["install", "uninstall", "upgrade", "pin"],
                )
                || path_is(&words, "self", &["update"]))
            .then_some(PACKAGES)
        }
        "pdm" => first_in(
            &positionals(args, &["-p", "--project", "-g", "--global"]),
            &[
                "add", "remove", "install", "update", "sync", "lock", "self", "python", "use",
            ],
        )
        .then_some(PACKAGES),
        "hatch" => {
            let words = positionals(args, &["-e", "--env", "-p", "--project"]);
            (path_is(&words, "env", &["create", "prune", "remove"])
                || path_is(&words, "python", &["install", "remove", "update"])
                || path_is(&words, "self", &["update"]))
            .then_some(PACKAGES)
        }
        "poetry" => first_in(
            &positionals(args, &["--directory", "-C", "--project", "-P"]),
            &["add", "remove", "install", "update", "lock", "sync", "self"],
        )
        .then_some(PACKAGES),
        // The first short letter selects dpkg's, rpm's, and nix-env's mode:
        // `rpm -qi` queries, `rpm -ivh` installs.
        "dpkg" => mode(
            args,
            &['i', 'r', 'P'],
            &[
                "--install",
                "--remove",
                "--purge",
                "--unpack",
                "--configure",
            ],
        )
        .then_some(PACKAGES),
        "rpm" => mode(
            args,
            &['i', 'U', 'F', 'e'],
            &[
                "--install",
                "--upgrade",
                "--freshen",
                "--erase",
                "--reinstall",
                "--rollback",
            ],
        )
        .then_some(PACKAGES),
        "nix-env" => mode(
            args,
            &['i', 'e', 'u', 'G'],
            &[
                "--install",
                "--uninstall",
                "--upgrade",
                "--set",
                "--rollback",
                "--switch-generation",
                "--delete-generations",
            ],
        )
        .then_some(PACKAGES),
        "nix" => {
            let words = positionals(
                args,
                &["--profile", "--option", "--extra-experimental-features"],
            );
            (path_is(
                &words,
                "profile",
                &[
                    "install",
                    "add",
                    "remove",
                    "upgrade",
                    "rollback",
                    "wipe-history",
                ],
            ) || first_in(&words, &["upgrade-nix"]))
            .then_some(PACKAGES)
        }
        "guix" => {
            let words = positionals(args, &["--profile", "-p"]);
            (first_in(&words, &["install", "remove", "upgrade", "pull"])
                || words.first() == Some(&"package")
                    && options(args, &[]).iter().any(|(name, _)| {
                        [
                            "--install",
                            "--remove",
                            "--upgrade",
                            "--roll-back",
                            "--switch-generation",
                        ]
                        .contains(name)
                            || ["-i", "-r", "-u"].contains(name)
                    })
                || path_is(
                    &words,
                    "system",
                    &["reconfigure", "roll-back", "switch-generation"],
                )
                || path_is(
                    &words,
                    "home",
                    &["reconfigure", "roll-back", "switch-generation"],
                ))
            .then_some(PACKAGES)
        }
        // These install or remove whatever they are given; only help,
        // version, and dry-run forms change nothing.
        "emerge" => (!options(args, &[]).iter().any(|(name, _)| {
            [
                "--pretend",
                "--search",
                "--searchdesc",
                "--info",
                "--help",
                "--version",
                "--list-sets",
                "--check-news",
            ]
            .contains(name)
                || ['p', 's', 'S', 'h', 'V']
                    .iter()
                    .any(|flag| short_is(name, *flag))
        }))
        .then_some(PACKAGES),
        "xbps-install" | "xbps-remove" | "pkg_add" | "pkg_delete" => {
            (!options(args, &[]).iter().any(|(name, _)| {
                ["--help", "--version", "-h", "-V"].contains(name)
                    || ["xbps-install", "xbps-remove"].contains(&program) && *name == "-n"
                    || ["pkg_add", "pkg_delete"].contains(&program) && *name == "-n"
            }))
            .then_some(PACKAGES)
        }
        "makepkg" => options(args, &["-p", "--config"])
            .iter()
            .any(|(name, _)| {
                ["--install", "--syncdeps", "--rmdeps"].contains(name)
                    || !name.starts_with("--")
                        && ['i', 's', 'r'].iter().any(|flag| short_is(name, *flag))
            })
            .then_some(PACKAGES),
        "eopkg" => first_in(
            &positionals(args, &[]),
            &[
                "install",
                "it",
                "remove",
                "rm",
                "upgrade",
                "up",
                "remove-orphans",
                "rmo",
                "emerge",
                "em",
            ],
        )
        .then_some(PACKAGES),
        "pkg" => first_in(
            &positionals(args, &["-j", "-c", "-r", "-C", "-R", "-o"]),
            &[
                "install",
                "add",
                "delete",
                "remove",
                "upgrade",
                "autoremove",
                "lock",
                "unlock",
            ],
        )
        .then_some(PACKAGES),
        "pkgin" => first_in(
            &positionals(args, &[]),
            &[
                "install",
                "in",
                "remove",
                "rm",
                "upgrade",
                "ug",
                "full-upgrade",
                "fug",
                "autoremove",
                "ar",
                "import",
                "im",
            ],
        )
        .then_some(PACKAGES),
        "mas" => first_in(
            &positionals(args, &[]),
            &[
                "install",
                "uninstall",
                "upgrade",
                "purchase",
                "lucky",
                "get",
                "reset",
            ],
        )
        .then_some(PACKAGES),
        "softwareupdate" => options(args, &[])
            .iter()
            .any(|(name, _)| {
                [
                    "--install",
                    "--install-rosetta",
                    "--fetch-full-installer",
                    "--schedule",
                    "--background",
                    "--set-catalog",
                    "--clear-catalog",
                    "--download",
                ]
                .contains(name)
                    || !name.starts_with("--")
                        && ['i', 'd'].iter().any(|flag| short_is(name, *flag))
            })
            .then_some(PACKAGES),
        "asdf" => {
            let words = positionals(args, &[]);
            (first_in(
                &words,
                &[
                    "install",
                    "uninstall",
                    "global",
                    "local",
                    "set",
                    "update",
                    "reshim",
                    "plugin-add",
                    "plugin-remove",
                    "plugin-update",
                ],
            ) || path_is(&words, "plugin", &["add", "remove", "update"]))
            .then_some(PACKAGES)
        }
        "mise" | "rtx" => {
            let words = positionals(args, &["--cd", "-C", "--env", "-E", "--jobs", "-j"]);
            (first_in(
                &words,
                &[
                    "install",
                    "i",
                    "uninstall",
                    "rm",
                    "remove",
                    "use",
                    "u",
                    "upgrade",
                    "up",
                    "prune",
                    "self-update",
                    "implode",
                    "link",
                    "sync",
                ],
            ) || ["plugins", "plugin", "p"].iter().any(|parent| {
                path_is(
                    &words,
                    parent,
                    &[
                        "install",
                        "i",
                        "add",
                        "a",
                        "uninstall",
                        "rm",
                        "remove",
                        "update",
                        "upgrade",
                        "link",
                    ],
                )
            }))
            .then_some(PACKAGES)
        }
        "sdk" => first_in(
            &positionals(args, &[]),
            &[
                "install",
                "i",
                "uninstall",
                "rm",
                "use",
                "u",
                "default",
                "d",
                "upgrade",
                "ug",
                "selfupdate",
                "flush",
                "env",
            ],
        )
        .then_some(PACKAGES),
        _ => None,
    }
}

/// A mode selected by the first short option's first letter, or a long
/// option naming the mode.
fn mode(args: &[&str], letters: &[char], long: &[&str]) -> bool {
    let mut first_short = true;
    for arg in args {
        if *arg == "--" {
            break;
        }
        let name = arg.split_once('=').map_or(*arg, |(name, _)| name);
        if long.contains(&name) {
            return true;
        }
        if first_short
            && let Some(shorts) = arg.strip_prefix('-')
            && !arg.starts_with("--")
            && let Some(letter) = shorts.chars().next()
        {
            if letters.contains(&letter) {
                return true;
            }
            first_short = false;
        }
    }
    false
}

fn npm_risk(args: &[&str]) -> Option<&'static str> {
    let words = positionals(args, value_options("npm"));
    let operation = words.first().copied()?;
    match operation {
        "run" | "run-script" | "rum" | "urn" => {
            (words.len() > 1).then_some("can execute project scripts")
        }
        "start" | "stop" | "restart" | "test" | "t" | "tst" => Some("can execute project scripts"),
        "completion" | "help" | "help-search" | "ls" | "list" | "la" | "ll" | "explain" | "why"
        | "outdated" | "view" | "v" | "info" | "show" | "search" | "find" | "s" | "se" | "root"
        | "prefix" | "ping" | "whoami" | "query" => None,
        "config" | "c"
            if words
                .get(1)
                .is_none_or(|word| matches!(*word, "get" | "list" | "ls")) =>
        {
            None
        }
        "pkg" if words.get(1) == Some(&"get") => None,
        "cache"
            if words
                .get(1)
                .is_some_and(|word| matches!(*word, "ls" | "list")) =>
        {
            None
        }
        "audit" if words.get(1).is_none_or(|word| *word != "fix") => None,
        // Unknown aliases and new operations are conservatively covered.
        // This table classifies effects; completion supplies every candidate.
        _ => Some("can change packages, project files, or registry state"),
    }
}

fn cargo_risk(args: &[&str]) -> Option<&'static str> {
    let words = positionals(args, value_options("cargo"));
    if words.first().is_some_and(|word| word.starts_with('+')) {
        return Some("selects a Rust toolchain which rustup may install");
    }
    let commands = &words[..];
    let operation = commands.first().copied()?;
    match operation {
        "version" | "locate-project" => None,
        "help" if commands.len() == 1 => None,
        "metadata"
            if args.contains(&"--no-deps")
                && (args.contains(&"--offline") || args.contains(&"--frozen")) =>
        {
            None
        }
        _ => Some("can execute package code or change project, artifact, or registry state"),
    }
}

#[cfg(test)]
mod tests {
    use crate::engine::parser::{self, Dialect};
    use crate::engine::safety::{self, Decision};

    #[test]
    fn core_tools_require_approval_for_in_place_edits_scheduled_jobs_and_find_actions() {
        for source in [
            "crontab -r",
            "crontab -u root -ir",
            "crontab -e",
            "crontab jobs.txt",
            "sed -i '' 's/a/b/' file",
            "gsed --in-place=.bak 's/a/b/' file",
            "sed -ni.bak -e 's/a/b/' file",
            "find . -name '*.tmp' -delete",
            "gfind . -name '-delete' -delete",
            "find . -exec echo '{}' ';'",
            "find . -fprintf output.txt '%p'",
            "busybox find . -delete",
        ] {
            assert_eq!(
                safety::assess_replay(source).decision,
                Decision::Confirm,
                "{source}"
            );
        }
        for source in [
            "crontab -l",
            "crontab -u root -l",
            "sed -e '-i' file",
            "gsed --quiet -e 's/a/b/' file",
            "find . -name '-delete' -print",
            "gfind . -path '-exec' -print",
            "find . -printf '-delete'",
        ] {
            assert_eq!(
                safety::assess_replay(source).decision,
                Decision::Allow,
                "{source}"
            );
        }
    }

    #[test]
    fn cargo_builds_aliases_plugins_and_toolchain_selection_require_approval() {
        for source in [
            "cargo build",
            "cargo --config build build --bin example",
            "cargo --color test test --package example",
            "cargo run --bin example",
            "cargo clean",
            "cargo publish",
            "cargo install example",
            "cargo uninstall example",
            "cargo fmt",
            "cargo nextest run",
            "cargo danger",
            "cargo --config alias.danger='!touch marker' danger",
            "cargo help nextest",
            "cargo metadata",
            "cargo metadata --offline",
            "cargo metadata --no-deps",
            "cargo +stable version",
            "cargo +missing --version",
            "env X=1 cargo test",
            "sudo cargo build",
        ] {
            for dialect in [Dialect::Posix, Dialect::Fish, Dialect::Tcsh] {
                let script = parser::parse_with_dialect(source, dialect);
                let expected = if dialect == Dialect::Tcsh && source.contains('!') {
                    Decision::Refuse
                } else {
                    Decision::Confirm
                };
                assert_eq!(
                    safety::assess(&script, source, &[]).decision,
                    expected,
                    "{dialect:?}: {source}"
                );
                assert_eq!(
                    safety::assess_replay_with_dialect(source, dialect).decision,
                    expected,
                    "replay {dialect:?}: {source}"
                );
            }
        }
        for source in [
            "cargo version",
            "cargo --version",
            "cargo --list",
            "cargo help",
            "cargo locate-project --manifest-path project/Cargo.toml",
            "cargo metadata --offline --no-deps",
            "cargo metadata --frozen --no-deps",
        ] {
            let script = parser::parse(source);
            assert_eq!(
                safety::assess(&script, source, &[]).decision,
                Decision::Allow,
                "{source}"
            );
        }
    }

    #[test]
    fn npm_scripts_publication_and_unknown_aliases_require_approval() {
        for source in [
            "npm run build",
            "npm run-script build",
            "npm rum build",
            "npm urn build",
            "npm --prefix project run build",
            "npm --workspace project test",
            "npm -w project start",
            "npm --registry https://example.invalid publish",
            "npm ci",
            "npm ic",
            "npm instal example",
            "npm approve-scripts",
            "npm version patch",
            "npm pkg set scripts.test=example",
            "npm config set registry https://example.invalid",
            "npm cache verify",
            "npm audit fix",
            "npm future-operation",
            "npm --prefix=project restart",
        ] {
            let script = parser::parse(source);
            assert_eq!(
                safety::assess(&script, source, &[]).decision,
                Decision::Confirm,
                "{source}"
            );
            assert_eq!(
                safety::assess_replay(source).decision,
                Decision::Confirm,
                "{source}"
            );
        }
        for source in [
            "npm run",
            "npm --prefix install run",
            "npm view install",
            "npm config get registry",
            "npm c list",
            "npm pkg get scripts",
            "npm cache ls",
            "npm audit",
            "npm --workspace test list",
        ] {
            let script = parser::parse(source);
            assert_eq!(
                safety::assess(&script, source, &[]).decision,
                Decision::Allow,
                "{source}"
            );
        }
    }

    #[test]
    fn developer_operations_need_approval_even_when_only_a_target_is_corrected() {
        for command in [
            "newfs /dev/example",
            "newfs_msdos /dev/example",
            "gdisk /dev/example",
            "sgdisk --zap-all /dev/example",
            "lvremove vg/example",
            "vgremove example",
            "pvremove /dev/example",
            "userdel example",
            "deluser example",
            "groupdel example",
            "diskutil eraseDisk APFS Example /dev/example",
            "diskutil apfs deleteVolume example",
            "diskutil partitionDisk /dev/example 1 GPT APFS Example 100%",
            "diskutil apfs resizeContainer example 0",
            "cryptsetup luksFormat /dev/example",
            "zpool labelclear /dev/example",
            "zfs rollback example/snapshot",
            "shutdown -h now",
            "reboot",
            "systemctl reboot",
            "systemctl suspend",
            "iptables -F",
            "iptables -t filter --flush INPUT",
            "iptables -tfilter -vF",
            "iptables --table=filter --flush=INPUT",
            "ip6tables -w 5 -F",
            "iptables --wait -F",
            "iptables --wait-interval -F",
            "iptables -w -F",
            "iptables -A INPUT -j DROP",
            "iptables -P INPUT DROP",
            "iptables -N example",
            "nft flush ruleset",
            "nft 'flush ruleset'",
            "nft 'list ruleset; flush ruleset'",
            "nft list 'ruleset;flush ruleset'",
            "nft -f example.nft",
            "ufw reset",
            "ufw disable",
            "ufw allow 443",
            "firewall-cmd --zone=public --remove-service=https",
            "firewall-cmd --reload",
            "pfctl -F all",
            "pfctl -a example -f example.conf",
            "pfctl -t example -T flush",
            "systemctl stop example",
            "systemctl --host stop restart example",
            "systemctl -Hstop stop example",
            "systemctl --property stop stop example",
            "systemctl --message example poweroff",
            "systemctl --preset-mode disable stop example",
            "service example restart",
            "launchctl bootout gui/501/example",
            "launchctl kickstart -k gui/501/example",
            "launchctl disable gui/501/example",
            "nginx -s stop",
            "nginx -squit",
            "httpd -k graceful-stop",
            "httpd -krestart",
            "apachectl stop",
            "apachectl -k stop",
            "caddy stop",
            "caddy --config example trust",
            "caddy untrust",
            "caddy upgrade",
            "caddy add-package github.com/example/plugin",
            "haproxy -f example.cfg -sf 1234",
            "haproxy -st 1234 -f example.cfg",
            "varnishadm -n example stop",
            "varnishadm -T localhost:6082 -S secret stop",
            "docker stop example",
            "docker --context stop container restart example",
            "docker compose -f stop down",
            "podman --connection stop stop example",
            "podman pod stop example",
            "nerdctl --namespace stop stop example",
            "docker-compose down",
            "certbot delete --cert-name example",
            "certbot --config example revoke",
            "acme.sh --home example --remove -d example.test",
            "acme.sh --revoke -d example.test",
            "mkcert -uninstall",
            "mkcert -install",
            "mkcert --install",
            "certbot unregister",
            "adb install app.apk",
            "adb -s emulator-5554 uninstall com.example",
            "adb push x /sdcard/x",
            "adb reboot bootloader",
            "fastboot flash boot boot.img",
            "fastboot -s serial erase userdata",
            "fastboot flashing unlock",
            "fastboot oem unlock",
            "xcode-select --switch /Applications/Xcode.app",
            "xcode-select -s /Applications/Xcode.app",
            "xcodes install 16.0",
            "pod install",
            "uv pip install requests",
            "uv sync",
            "uv --directory x tool install ruff",
            "uv python install 3.13",
            "pdm install",
            "pdm add requests",
            "hatch env create",
            "pod repo update",
            "carthage bootstrap",
            "dscl . -delete /Users/example",
            "dscl . delete /Groups/example",
            "dscl . -create /Users/example",
            "dscl . -passwd /Users/example",
            "sysadminctl -deleteUser example",
            "sysadminctl -addUser example",
            "sysadminctl -guestAccount off",
            "systemsetup -setremotelogin on",
            "pmset -a sleep 0",
            "pmset sleepnow",
            "pmset schedule wake '01/01/2027 08:00:00'",
            "sysctl -w kern.maxfiles=10000",
            "sysctl kern.maxfiles=10000",
            "csrutil disable",
            "nvram boot-args=-v",
            "nvram -d boot-args",
            "code --install-extension ms-python.python",
            "cursor --uninstall-extension example.example",
            "code --update-extensions",
            "code tunnel",
            "networksetup -setdnsservers Wi-Fi 1.1.1.1",
            "networksetup -createnetworkservice example en0",
            "networksetup -removenetworkservice example",
            "networksetup -setairportpower en0 off",
            "route add default 10.0.0.1",
            "route -n delete 10.0.0.0/8",
            "ifconfig en0 down",
            "ifconfig en0 alias 10.0.0.2",
            "arp -d 10.0.0.1",
            "arp -an -d 10.0.0.1",
            "scutil --set HostName example",
            "scutil --nc stop example",
            "husky",
            "husky --help",
            "husky init",
            "husky .config/husky",
            "lefthook install",
            "lefthook --verbose uninstall",
            "pre-commit install",
            "pre-commit -c example.yaml install --hook-type pre-push",
            "certbot --config example rollback --checkpoints 1",
            "acme.sh --deactivate -d example.test",
            "acme.sh --deactivate-account",
            "flyway clean",
            "flyway -url=jdbc:example clean",
            "liquibase dropAll",
            "liquibase --url example drop-all",
            "liquibase rollback-count 1",
            "prisma migrate reset",
            "prisma --schema example.prisma migrate reset",
            "prisma db push --force-reset",
            "prisma db push --accept-data-loss",
            "alembic -c example.ini downgrade base",
            "dbmate --url example drop",
            "knex --knexfile example.js migrate rollback",
            "typeorm migration:revert -d example.js",
            "sequelize-cli db:migrate:undo:all",
            "rake db:reset",
            "rails db:drop",
            "restic forget example",
            "restic -r forget forget example",
            "restic --repo=example prune",
            "borg --repo example prune",
            "borg compact example",
            "borgmatic -c example.yaml prune",
            "borgmatic --config example.yaml",
            "borgmatic",
            "borgmatic list prune",
            "borgmatic --default-actions list",
            "borgmatic --config one.yaml two.yaml",
            "borgmatic --sources list",
            "borgmatic list rdelete",
            "tarsnap -d -f example",
            "tarsnap -dfexample",
            "tarsnap --nuke",
            "tarsnap --fsck-prune",
            "rsnapshot daily",
            "rsnapshot -c example.conf custom-interval",
            "duplicity remove-older-than 30D example",
            "kopia snapshot delete example",
            "rabbitmqctl delete_queue orders",
            "rabbitmqctl -p / purge_queue orders",
            "rabbitmqctl --node rabbit@host --quiet force_reset",
            "rabbitmqctl stop_app",
            "rabbitmqctl close_all_connections reason",
            "rabbitmqctl eval 'halt().'",
            "rabbitmqctl --vhost=/ delete_vhost example",
            "rabbitmq-plugins enable rabbitmq_shovel",
            "rabbitmq-plugins --node rabbit@host disable rabbitmq_management",
            "rabbitmq-queues shrink rabbit@host",
            "rabbitmq-queues delete_member orders rabbit@host",
            "rabbitmq-streams delete_replica orders rabbit@host",
            "rabbitmq-streams reset_offset --stream orders",
            "rabbitmq-upgrade drain",
            "rabbitmqctl delete_queue --vhost --help orders",
            "kafkactl reset consumer-group-offset orders-group --topic orders --oldest --execute",
            "kafkactl --context prod reset offset orders-group --topic orders --newest",
            "kafkactl delete topic orders",
            "kaf group commit orders-group --topic orders --offset oldest",
            "kaf -c prod group commit orders-group --offset 0",
            "kaf group delete-offsets orders-group",
            "kaf topic delete orders",
            "rpk group seek orders-group --to start",
            "kafka-consumer-groups --bootstrap-server b:9092 --group g --reset-offsets --to-earliest --topic t --execute",
            "kafka-consumer-groups.sh --bootstrap-server b:9092 --group g --topic t --delete-offsets",
            "kafka-topics --bootstrap-server b:9092 --delete --topic orders",
            "tmutil deletelocalsnapshots example",
            "telepresence connect --context example",
            "telepresence --context connect intercept example",
            "telepresence leave example",
            "wg-quick up example",
            "wg-quick down example",
            "wg set example peer key remove",
            "wg syncconf example example.conf",
            "tailscale up --login-server example",
            "netbird --config example.json down",
            "zerotier-cli join example",
            "nmcli connection down example",
            "nmcli --fields down device disconnect example",
            "ip -n example link set example down",
            "ip route flush table example",
            "vault secrets disable kv/",
            "vault auth disable -namespace=team userpass/",
            "vault audit disable file/",
            "vault secrets move kv/ archive/",
            "vault kv rollback -version=2 secret/app",
            "vault operator seal",
            "vault operator step-down",
            "vault operator rekey -init",
            "vault operator raft snapshot restore backup.snap",
            "vault lease revoke --force -prefix aws/",
            "vault token revoke example",
            "vault kv destroy -versions=1 secret/app",
            "bao secrets disable kv/",
            "nomad stop example",
            "nomad job stop -purge example",
            "nomad job revert example 3",
            "nomad alloc restart abc123",
            "nomad node drain -enable example",
            "nomad node eligibility -disable example",
            "nomad system gc",
            "nomad server force-leave example",
            "nomad deployment fail abc123",
            "nomad operator snapshot restore backup.snap",
            "nomad volume delete -force example",
            "consul leave",
            "consul force-leave example",
            "consul exec -node example uptime",
            "consul lock locks/example ./run.sh",
            "consul maint -enable -reason upgrade",
            "consul snapshot restore backup.snap",
            "consul kv delete -recurse app/",
            "boundary sessions cancel -id s_example",
            "boundary targets delete -id ttcp_example",
            "terraform apply -destroy",
            "terraform -chdir=infra apply -destroy -auto-approve",
            "terraform apply -replace=aws_instance.example",
            "tofu apply --destroy",
            "terraform force-unlock 1234",
            "terraform taint aws_instance.example",
            "terraform state push terraform.tfstate",
            "packer build -force example.pkr.hcl",
            "waypoint deploy -prune",
            "waypoint up -prune -prune-retain=1",
            "direnv allow",
            "direnv permit ./project",
            "direnv grant",
            "direnv edit .",
            "direnv exec ./project make",
            "stow -D vim",
            "stow -t ~/home -D vim",
            "clamscan --remove -r .",
            "clamscan --move=/quarantine -r .",
            "knex migrate:rollback --all",
            "unzip -o photos.zip",
            "7z x -y archive.7z",
            "7z x -aoa archive.7z",
            "tar --overwrite -xf backup.tar",
            "gzip -f notes.txt",
            "zstd --rm notes.txt",
            "unpigz -f notes.txt.gz",
            "unlz4 -f notes.txt.lz4",
            "brotli -j notes.txt",
            "brotli -q 5 --rm notes.txt",
            "compress -f notes.txt",
            "zip -m out.zip notes.txt",
            "zip -r --move out.zip dir",
            "lzip -df notes.txt.lz",
            "lzop -U notes.txt",
            "ouch decompress -y photos.zip",
            "ouch d --remove photos.zip",
            "unar -force-overwrite photos.zip",
            "dtrx -o photos.zip",
            "aunpack -f photos.zip",
            "unsquashfs -f -d out image.sqfs",
            "garden cleanup namespace",
            "garden --env dev cleanup deploy api",
            "garden plugins kubernetes cluster-init",
            "garden self-update",
            "goose down",
            "goose -dir db postgres \"user=x\" reset",
            "goose down-to 20260101",
            "migrate -path db -database postgres://x down 1",
            "migrate -database postgres://x force 3",
            "knex migrate:down",
            "gpg --delete-secret-keys example",
            "gpg --batch --delete-keys example",
            "ssh-keygen -R example.com",
            "ssh-keygen -f ~/.ssh/known_hosts -R example.com",
        ] {
            // Correcting the executable or a target cannot make the operation
            // harmless. Check every supported shell adapter, and replay too.
            for dialect in [Dialect::Posix, Dialect::Fish, Dialect::Tcsh] {
                let original = parser::parse_with_dialect(&format!("{command}e"), dialect);
                let gate = safety::assess(&original, command, &[]);
                assert_eq!(
                    gate.decision,
                    Decision::Confirm,
                    "{dialect:?}: {command}: {:?}",
                    gate.reasons
                );
                assert_eq!(
                    safety::assess_replay_with_dialect(command, dialect).decision,
                    Decision::Confirm,
                    "replay {dialect:?}: {command}"
                );
            }
        }
    }

    #[test]
    fn package_managers_installs_removals_and_version_switches_require_approval() {
        for command in [
            "bundle install",
            "bundle --gemfile install update",
            "bundler add example",
            "composer require example/example",
            "composer -d install update",
            "poetry add example",
            "poetry self update",
            "dpkg -i example.deb",
            "dpkg --purge example",
            "rpm -ivh example.rpm",
            "rpm -e example",
            "rpm --upgrade example.rpm",
            "microdnf install example",
            "dnf5 remove example",
            "nix-env -iA nixpkgs.example",
            "nix-env --uninstall example",
            "nix profile install nixpkgs#example",
            "guix install example",
            "guix package -r example",
            "guix system reconfigure example.scm",
            "emerge example",
            "emerge -uDN @world",
            "emerge --depclean",
            "xbps-install -S example",
            "xbps-remove example",
            "pkg_add example",
            "pkg_delete example",
            "makepkg -si",
            "makepkg --install",
            "eopkg it example",
            "pkg delete example",
            "pkg -j jail install example",
            "pkgin fug",
            "mas install 123",
            "softwareupdate -ia",
            "softwareupdate --install-rosetta",
            "asdf install nodejs latest",
            "asdf global nodejs 22",
            "asdf plugin add example",
            "mise use node@22",
            "mise plugins install example",
            "sdk install java",
            "sdk use java 21",
            "gh copilot",
            "gh copilot --remove",
            "gh copilot -- -p explain",
            "gh extension install owner/gh-example",
            "gh ext remove example",
        ] {
            for dialect in [Dialect::Posix, Dialect::Fish, Dialect::Tcsh] {
                let original = parser::parse_with_dialect(&format!("{command}e"), dialect);
                let gate = safety::assess(&original, command, &[]);
                assert_eq!(
                    gate.decision,
                    Decision::Confirm,
                    "{dialect:?}: {command}: {:?}",
                    gate.reasons
                );
            }
            assert_eq!(
                safety::assess_replay_with_dialect(command, Dialect::Posix).decision,
                Decision::Confirm,
                "replay: {command}"
            );
        }
        for command in [
            "bundle exec rake test",
            "bundle --gemfile install list",
            "composer show",
            "poetry show",
            "dpkg -l",
            "dpkg -L example",
            "rpm -qi example",
            "rpm -qa",
            "rpm -Va",
            "nix-env -qa",
            "nix profile list",
            "nix search nixpkgs example",
            "guix search example",
            "guix package --list-installed",
            "emerge --search example",
            "emerge -pv example",
            "xbps-install -n example",
            "pkg_add -n example",
            "makepkg --printsrcinfo",
            "eopkg list-installed",
            "pkg info example",
            "pkgin search example",
            "mas list",
            "softwareupdate --list",
            "asdf list",
            "asdf current",
            "mise ls",
            "mise plugins ls",
            "sdk list java",
            "sdk current",
            "gh copilot --help",
            "gh extension list",
            "gh pr list --repo copilot/x",
        ] {
            let script = parser::parse(command);
            let gate = safety::assess(&script, command, &[]);
            assert_eq!(
                gate.decision,
                Decision::Allow,
                "{command}: {:?}",
                gate.reasons
            );
        }
    }

    #[test]
    fn resource_names_option_values_and_read_only_operations_are_not_actions() {
        for command in [
            "diskutil list eraseDisk",
            "diskutil apfs list deleteVolume",
            "systemctl status stop",
            "systemctl --property stop show example",
            "systemctl -Hstop status restart",
            "systemctl --host=stop status restart",
            "service stop status",
            "nginx -t -c stop",
            "nginx -T -g stop",
            "httpd -t -f stop",
            "apachectl configtest",
            "apachectl -t -f stop",
            "caddy validate --config stop",
            "docker --context stop inspect restart",
            "docker compose -f down ps stop",
            "podman --connection stop inspect restart",
            "nerdctl --namespace stop inspect restart",
            "iptables -t FORWARD -L",
            "iptables -tFORWARD -L",
            "iptables --table=-F -L",
            "iptables -t -F -L",
            "iptables -m FILTER -L",
            "iptables -L -t filter",
            "iptables -L -- -F",
            "iptables -á -L",
            "iptables -áé -L",
            "nft list ruleset",
            "nft --define flush list ruleset",
            "ufw status",
            "firewall-cmd --zone=reset --list-all",
            "firewall-cmd --query-service=https",
            "pfctl -s rules",
            "pfctl -a flush -s rules",
            "pfctl -t flush -T show",
            "certbot certificates --cert-name example",
            "acme.sh --domain --revoke --list",
            "mkcert -CAROOT",
            "mkcert example.test",
            "adb devices",
            "adb -s install shell ls",
            "adb logcat",
            "fastboot devices",
            "fastboot getvar all",
            "xcode-select --print-path",
            "xcodes list",
            "pod search example",
            "uv run x.py",
            "uv pip list",
            "pdm run serve",
            "hatch env show",
            "carthage version",
            "dscl . -list /Users",
            "dscl . -read /Users/example",
            "sysadminctl -guestAccount status",
            "systemsetup -gettimezone",
            "pmset -g batt",
            "sysctl -n hw.ncpu",
            "sysctl -a",
            "csrutil status",
            "nvram -p",
            "code --list-extensions",
            "code --new-window -- --install-extension",
            "launchctl kickstart gui/501/example",
            "launchctl print gui/501",
            "networksetup -listallnetworkservices",
            "networksetup -getdnsservers Wi-Fi",
            "route -n get default",
            "ifconfig",
            "ifconfig -a",
            "ifconfig en0",
            "arp -an",
            "scutil --dns",
            "scutil --nc list",
            "lefthook run pre-commit",
            "lefthook validate",
            "pre-commit run --all-files",
            "pre-commit --config install run",
            "caddy list-modules --packages",
            "caddy storage export --config untrust",
            "haproxy -c -f example.cfg",
            "haproxy -f -sf.cfg -c -- -st",
            "varnishadm -n stop status",
            "varnishadm -S stop vcl.list",
            "flyway info",
            "liquibase rollback-sql example",
            "liquibase rollbackCountSQL 1",
            "prisma --schema reset migrate status",
            "alembic -c downgrade current",
            "dbmate --url drop status",
            "knex --knexfile rollback migrate list",
            "typeorm migration:show",
            "sequelize-cli db:migrate:status",
            "php artisan migrate:status",
            "php artisan --env migrate:fresh list",
            "php bin/console list doctrine:database:drop",
            "php composer.phar show install",
            "restic -r forget snapshots",
            "restic snapshots --tag forget",
            "borg --repo prune list",
            "borgmatic --config prune list",
            "borgmatic list --repository prune",
            "borgmatic --help",
            "borgmatic --version",
            "tarsnap -tv -f example",
            "tarsnap -f -d -t",
            "tarsnap --cachedir=-d --list-archives",
            "rsnapshot configtest",
            "rsnapshot -t daily",
            "rsnapshot -vt daily",
            "kopia snapshot list",
            "rabbitmqctl list_queues",
            "rabbitmqctl -p delete_queue list_queues",
            "rabbitmqctl --vhost purge_queue list_exchanges",
            "rabbitmqctl help delete_queue",
            "rabbitmqctl delete_queue --help",
            "rabbitmq-plugins disable --help",
            "rabbitmq-diagnostics status",
            "rabbitmq-plugins list",
            "rabbitmq-plugins --node enable list",
            "rabbitmq-streams list_stream_connections",
            "kafkactl get consumer-groups",
            "kafkactl --context reset get topics",
            "kaf group describe orders-group",
            "kaf --cluster commit group ls",
            "rpk group describe orders-group",
            "kafka-consumer-groups --bootstrap-server b:9092 --group g --reset-offsets --to-earliest --topic t --dry-run",
            "kafka-consumer-groups --bootstrap-server b:9092 --describe --group g",
            "tmutil listlocalsnapshots example",
            "telepresence --context intercept status",
            "telepresence list",
            "wg-quick strip down",
            "wg show set",
            "tailscale status",
            "netbird --config down status",
            "zerotier-cli listnetworks",
            "nmcli --fields down connection show",
            "nmcli connection show down",
            "ip -n flush link show down",
            "ip route show table flush",
            "vault secrets list",
            "vault secrets disable -help",
            "vault kv get -mount=disable secret/app",
            "vault kv get -field seal secret/app",
            "vault operator raft list-peers",
            "vault secrets enable -path=disable kv",
            "bao policy read example",
            "nomad job status example",
            "nomad job stop -h",
            "nomad node status -verbose drain",
            "nomad alloc logs -job stop",
            "consul members",
            "consul maint",
            "consul kv get leave",
            "consul snapshot save backup.snap",
            "boundary sessions list",
            "boundary targets read -id cancel",
            "terraform plan -destroy",
            "terraform apply",
            "terraform apply -help -destroy",
            "terraform -chdir apply plan",
            "terraform state list",
            "tofu validate",
            "packer build example.pkr.hcl",
            "packer validate -force",
            "waypoint deploy",
            "waypoint deploy -prune-retain=1",
            "direnv status",
            "direnv deny",
            "direnv export zsh",
            "stow --simulate vim",
            "stow -n -v vim",
            "chezmoi status",
            "chezmoi diff",
            "clamscan --infected .",
            "knex migrate:latest",
            "unzip -l photos.zip",
            "7z l archive.7z",
            "tar -tf backup.tar",
            "gzip -k notes.txt",
            "zstd -d notes.zst -o out",
            "brotli -q 11 -o out.br notes.txt",
            "brotli -d -o j notes.txt.br",
            "compress -b 12 notes.txt",
            "unlz4 notes.txt.lz4 out",
            "zip -r out.zip -m.txt",
            "zip -mx out.zip notes.txt",
            "lzip -k notes.txt",
            "ouch list photos.zip",
            "ouch d -d y photos.zip",
            "unar -o f photos.zip",
            "dtrx -p o photos.zip",
            "als photos.zip",
            "unsquashfs -l image.sqfs",
            "garden deploy",
            "garden --env cleanup get status",
            "garden get deploys",
            "goose -dir down status",
            "goose up",
            "migrate -path down -database x up",
            "migrate -database x version",
            "knex migrate:list",
            "gpg --list-keys",
            "gpg --export example",
            "ssh-keygen -F example.com",
            "ssh-keygen -t ed25519 -f R",
        ] {
            let script = parser::parse(command);
            let gate = safety::assess(&script, command, &[]);
            assert_eq!(
                gate.decision,
                Decision::Allow,
                "{command}: {:?}",
                gate.reasons
            );
        }
    }

    #[test]
    fn repaired_chezmoi_stow_and_scanner_targets_need_approval() {
        for (original, candidate, decision) in [
            (
                "chezmoi aply ~/.bashrc",
                "chezmoi apply ~/.bashrc",
                Decision::Allow,
            ),
            ("chezmoi aply", "chezmoi apply", Decision::Allow),
            ("chezmoi stauts", "chezmoi status", Decision::Allow),
            (
                "chezmoi cat ~/.bashrx",
                "chezmoi cat ~/.bashrc",
                Decision::Allow,
            ),
            (
                "chezmoi apply ~/.bashrx",
                "chezmoi apply ~/.bashrc",
                Decision::Confirm,
            ),
            (
                "chezmoi add ~/.zshrx",
                "chezmoi add ~/.zshrc",
                Decision::Confirm,
            ),
            (
                "chezmoi apply -D ~/hmoe",
                "chezmoi apply -D ~/home",
                Decision::Confirm,
            ),
            (
                "chezmoi --destination=~/hmoe apply",
                "chezmoi --destination=~/home apply",
                Decision::Confirm,
            ),
            (
                "stow --simluate vim",
                "stow --simulate vim",
                Decision::Allow,
            ),
            ("stow vmi", "stow vim", Decision::Confirm),
            (
                "stow -t ~/hmoe vim",
                "stow -t ~/home vim",
                Decision::Confirm,
            ),
            (
                "stow --target=~/hmoe vim",
                "stow --target=~/home vim",
                Decision::Confirm,
            ),
            (
                "nikto -hots example.com",
                "nikto -host example.com",
                Decision::Allow,
            ),
            (
                "nikto -host exmaple.com",
                "nikto -host example.com",
                Decision::Confirm,
            ),
            (
                "nikto -h=exmaple.com",
                "nikto -h=example.com",
                Decision::Confirm,
            ),
            (
                "trufflehog gti https://example.com/repo",
                "trufflehog git https://example.com/repo",
                Decision::Allow,
            ),
            (
                "trufflehog git https://exmaple.com/repo",
                "trufflehog git https://example.com/repo",
                Decision::Confirm,
            ),
            (
                "trufflehog github --org=exmaple",
                "trufflehog github --org=example",
                Decision::Confirm,
            ),
            ("tsh sh user@node", "tsh ssh user@node", Decision::Allow),
            ("tsh ssh user@ndoe", "tsh ssh user@node", Decision::Confirm),
            (
                "tsh login --proxy=prxy.example.com",
                "tsh login --proxy=proxy.example.com",
                Decision::Confirm,
            ),
            ("tsh lsit", "tsh ls", Decision::Allow),
            // Network diagnostics reach whatever their arguments name.
            (
                "nmap --scirpt vuln example.com",
                "nmap --script vuln example.com",
                Decision::Allow,
            ),
            (
                "nmap -sV exmaple.com",
                "nmap -sV example.com",
                Decision::Confirm,
            ),
            (
                "whois -h whois.ripe.ent example.com",
                "whois -h whois.ripe.net example.com",
                Decision::Confirm,
            ),
            (
                "ping -c 3 exmaple.com",
                "ping -c 3 example.com",
                Decision::Confirm,
            ),
            (
                "dig example.com +shrot",
                "dig example.com +short",
                Decision::Allow,
            ),
            (
                "dig exmaple.com +short",
                "dig example.com +short",
                Decision::Confirm,
            ),
            (
                "traceroute -n exmaple.com",
                "traceroute -n example.com",
                Decision::Confirm,
            ),
            (
                "masscan --rat 1000 -p80 10.0.0.0/8",
                "masscan --rate 1000 -p80 10.0.0.0/8",
                Decision::Allow,
            ),
        ] {
            let gate = safety::assess(&parser::parse(original), candidate, &[]);
            assert_eq!(
                gate.decision, decision,
                "{original} -> {candidate}: {:?}",
                gate.reasons
            );
        }
    }

    #[test]
    fn wrappers_do_not_hide_developer_operations() {
        for command in [
            "sudo diskutil eraseDisk APFS Example /dev/example",
            "env EXAMPLE=1 systemctl stop example",
            "timeout 2 nginx -s quit",
            "npx --no-install prisma migrate reset",
            "npm exec -- prisma migrate reset",
            "pnpm exec prisma migrate reset",
            "uv run --project example alembic downgrade base",
            "pipx run alembic downgrade base",
            "bundle exec rails db:reset",
            "php artisan migrate:fresh",
            "php8.5 artisan migrate:fresh",
            "env APP_ENV=prod php bin/console doctrine:database:drop",
            "php composer.phar install",
        ] {
            let script = parser::parse(command);
            let gate = safety::assess(&script, command, &[]);
            assert_eq!(
                gate.decision,
                Decision::Confirm,
                "{command}: {:?}",
                gate.reasons
            );
            assert!(
                gate.reasons.iter().any(|reason| {
                    [
                        super::DISK,
                        super::SERVER,
                        super::MIGRATION,
                        super::PACKAGES,
                    ]
                    .iter()
                    .any(|risk| reason.contains(risk))
                }),
                "the wrapped operation itself needs approval: {command}: {:?}",
                gate.reasons
            );
        }
    }
}
