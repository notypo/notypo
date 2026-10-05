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
const CERTIFICATE: &str = "deletes or revokes certificates";
const MIGRATION: &str = "can discard database data or roll back migrations";
const BACKUP: &str = "can remove backup snapshots or stored data";
const NETWORK: &str = "changes tunnels, network configuration, or intercepted traffic";

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
        "telepresence" => &["--context", "--namespace", "-n", "--output"],
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
    positionals(args, value_options(program))
        .into_iter()
        .take(3)
        .collect()
}

pub(super) fn risk(program: &str, args: &[&str]) -> Option<&'static str> {
    match program {
        "npm" => npm_risk(args),
        "cargo" => cargo_risk(args),
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
        "service" => {
            let words = positionals(args, value_options(program));
            words
                .get(1)
                .is_some_and(|word| ["stop", "restart", "force-reload"].contains(word))
                .then_some(SERVER)
        }
        "launchctl" => first_in(
            &positionals(args, value_options(program)),
            &["bootout", "remove", "stop", "kill", "unload", "reboot"],
        )
        .then_some(SERVER),
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
        "caddy" => first_in(
            &positionals(args, value_options(program)),
            &["stop", "reload"],
        )
        .then_some(SERVER),
        "docker" | "podman" | "nerdctl" | "docker-compose" | "podman-compose" => {
            let words = positionals(args, value_options(program));
            (first_in(&words, &["stop", "restart", "kill", "down"])
                || path_is(&words, "container", &["stop", "restart", "kill"])
                || path_is(&words, "pod", &["stop", "restart", "kill"])
                || path_is(&words, "compose", &["stop", "restart", "kill", "down"]))
            .then_some(SERVER)
        }
        "certbot" => first_in(
            &positionals(args, value_options(program)),
            &["delete", "revoke"],
        )
        .then_some(CERTIFICATE),
        "acme.sh" => options(args, value_options(program))
            .iter()
            .any(|(name, _)| ["--remove", "--revoke"].contains(name))
            .then_some(CERTIFICATE),
        "mkcert" => args.contains(&"-uninstall").then_some(CERTIFICATE),
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
        "knex" => path_is(
            &positionals(args, value_options(program)),
            "migrate",
            &["rollback"],
        )
        .then_some(MIGRATION),
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
        _ => None,
    }
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
            "nginx -s stop",
            "nginx -squit",
            "httpd -k graceful-stop",
            "httpd -krestart",
            "apachectl stop",
            "apachectl -k stop",
            "caddy stop",
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
            "flyway info",
            "liquibase rollback-sql example",
            "liquibase rollbackCountSQL 1",
            "prisma --schema reset migrate status",
            "alembic -c downgrade current",
            "dbmate --url drop status",
            "knex --knexfile rollback migrate list",
            "typeorm migration:show",
            "sequelize-cli db:migrate:status",
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
                    [super::DISK, super::SERVER, super::MIGRATION]
                        .iter()
                        .any(|risk| reason.contains(risk))
                }),
                "the wrapped operation itself needs approval: {command}: {:?}",
                gate.reasons
            );
        }
    }
}
