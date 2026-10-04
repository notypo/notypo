//! App-native completion backends.
//!
//! The installed application answers what is valid at a position; notypo
//! keeps no copies of command trees. A backend is a small protocol bridge
//! that knows how to ask an app's own completer and how to read the answer,
//! so upgrades and newly installed extensions show up without a release.
//!
//! Discovery is offline by default. Completers can reach the network for
//! resource names or update checks, so every probe runs with outbound HTTP
//! routed to a closed local port and, where the protocol allows it, without
//! credentials. Only command, group, and option positions are queried.

use super::probe::{self, Budget, Capture, Probe, ProbeError};
use crate::shlex;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::{env, fs};

/// One valid word for a position, as reported by the app.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompletionItem {
    pub value: String,
    /// For options: whether a value follows, when the protocol says so.
    pub takes_value: Option<bool>,
}

impl CompletionItem {
    pub fn is_option(&self) -> bool {
        self.value.starts_with('-')
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CompletionError {
    /// The protocol can't answer for this position (for example, gcloud's
    /// static completion data has no positional arguments).
    Unsupported(String),
    /// The completer failed or misbehaved.
    Failed(String),
    BudgetExhausted,
}

impl std::fmt::Display for CompletionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CompletionError::Unsupported(why) => write!(f, "unsupported: {why}"),
            CompletionError::Failed(why) => write!(f, "failed: {why}"),
            CompletionError::BudgetExhausted => f.write_str("probe budget exhausted"),
        }
    }
}

/// What a backend can answer. Values and resources stay off until a network
/// policy allows dynamic completion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Capabilities {
    pub subcommands: bool,
    pub options: bool,
    /// Reports whether an option takes a value.
    pub option_arity: bool,
    pub values: bool,
    pub resources: bool,
    /// The prefix that enumerates every option at a position.
    pub option_prefix: &'static str,
}

pub trait NativeCompletionBackend: std::fmt::Debug {
    /// Stable identity, such as `aws` or `argcomplete`.
    fn id(&self) -> &str;
    fn capabilities(&self) -> Capabilities;
    /// Words valid after `words` (the arguments following the program name)
    /// that start with `prefix`. `Ok(vec![])` means the app knows of none.
    fn complete(
        &self,
        words: &[&str],
        prefix: &str,
        budget: &mut Budget,
    ) -> Result<Vec<CompletionItem>, CompletionError>;
}

/// The protocol family of a discovered backend.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flavor {
    /// `aws_completer`: `COMP_LINE`/`COMP_POINT` in, one word per line out.
    AwsCompleter,
    /// gcloud's static completion tree over the argcomplete protocol.
    Gcloud,
    /// Azure CLI (knack) over the argcomplete protocol.
    Azure,
    /// Any Python app declaring `PYTHON_ARGCOMPLETE_OK` (opt-in).
    Argcomplete,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Backend {
    pub flavor: Flavor,
    /// The name the app is invoked by on the command line.
    pub name: String,
    /// The executable that answers completion queries.
    pub completer: PathBuf,
}

/// The outcome of looking for an app's completion entry point.
#[derive(Debug, PartialEq, Eq)]
pub enum Discovery {
    Found(Backend),
    /// No supported completion protocol was found.
    None,
    /// A protocol exists, but probing it is not allowed.
    NotTrusted(String),
    /// The app has a protocol this platform can't run.
    Unavailable(String),
}

/// Finds the completion entry point of `name`, already resolved to `path`.
/// Built-in bridges identify the app from its installation; other apps that
/// declare argcomplete support are probed only when `trusted` lists them.
pub fn discover(name: &str, path: &Path, trusted: &[String]) -> Discovery {
    if !probe::is_trusted_location(path) {
        return Discovery::NotTrusted(format!(
            "{} was not resolved to an absolute path",
            path.display()
        ));
    }
    let name = app_name(name);
    let found = |flavor, completer: PathBuf| {
        Discovery::Found(Backend {
            flavor,
            name: name.to_owned(),
            completer,
        })
    };
    match name {
        "aws" => match aws_completer(path) {
            Some(completer) => found(Flavor::AwsCompleter, completer),
            None => Discovery::None,
        },
        "gcloud" if is_gcloud(path) => argcomplete(Flavor::Gcloud, name, path),
        "az" if head(path, 4096).contains("azure.cli") => argcomplete(Flavor::Azure, name, path),
        _ if head(path, 1024).contains("PYTHON_ARGCOMPLETE_OK") => {
            if trusted.iter().any(|t| t == "*" || t == name) {
                argcomplete(Flavor::Argcomplete, name, path)
            } else {
                Discovery::NotTrusted(format!(
                    "{name} declares argcomplete support; add it to trusted_completers to use it"
                ))
            }
        }
        _ => Discovery::None,
    }
}

fn argcomplete(flavor: Flavor, name: &str, path: &Path) -> Discovery {
    if cfg!(windows) {
        return Discovery::Unavailable(
            "the argcomplete protocol writes to file descriptor 8, which Windows lacks".into(),
        );
    }
    Discovery::Found(Backend {
        flavor,
        name: name.to_owned(),
        completer: path.to_owned(),
    })
}

/// The program name without directories or a Windows executable extension.
fn app_name(word: &str) -> &str {
    let base = word.rsplit(['/', '\\']).next().unwrap_or(word);
    #[cfg(windows)]
    for ext in [".exe", ".cmd", ".bat"] {
        if base.len() > ext.len() && base[base.len() - ext.len()..].eq_ignore_ascii_case(ext) {
            return &base[..base.len() - ext.len()];
        }
    }
    base
}

/// The start of a file, lossily decoded; empty when unreadable.
fn head(path: &Path, limit: usize) -> String {
    use std::io::Read;
    let mut data = Vec::new();
    if let Ok(file) = fs::File::open(path) {
        let _ = file.take(limit as u64).read_to_end(&mut data);
    }
    String::from_utf8_lossy(&data).into_owned()
}

/// `aws_completer` next to the `aws` that was resolved (or the file it links
/// to). A script must belong to `awscli`; the bundled v2 binary must sit in
/// the same directory as `aws`.
fn aws_completer(aws: &Path) -> Option<PathBuf> {
    let name = if cfg!(windows) {
        "aws_completer.exe"
    } else {
        "aws_completer"
    };
    let real = fs::canonicalize(aws).ok();
    [Some(aws), real.as_deref()]
        .into_iter()
        .flatten()
        .filter_map(Path::parent)
        .map(|dir| dir.join(name))
        .find(|candidate| {
            crate::utils::is_executable_file(candidate) && {
                let text = head(candidate, 4096);
                !text.starts_with("#!") || text.contains("awscli")
            }
        })
}

/// gcloud is a wrapper inside an SDK root that holds `lib/googlecloudsdk`.
fn is_gcloud(path: &Path) -> bool {
    fs::canonicalize(path)
        .ok()
        .and_then(|real| Some(real.parent()?.parent()?.join("lib/googlecloudsdk")))
        .is_some_and(|lib| lib.is_dir())
}

const BLOCKED_PROXY: &str = "http://127.0.0.1:9";

/// Variables that keep a completer offline: HTTP(S) clients get a proxy on
/// the closed discard port, and AWS/Azure clients get no credentials.
fn offline_env(flavor: Flavor) -> Vec<(OsString, Option<OsString>)> {
    let set = |k: &str, v: &str| (OsString::from(k), Some(OsString::from(v)));
    let unset = |k: &str| (OsString::from(k), None);
    let mut env: Vec<_> = [
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
    ]
    .into_iter()
    .map(|k| set(k, BLOCKED_PROXY))
    .chain(["NO_PROXY", "no_proxy"].map(unset))
    .collect();
    let null = if cfg!(windows) { "NUL" } else { "/dev/null" };
    match flavor {
        Flavor::AwsCompleter => {
            env.extend([
                set("AWS_CONFIG_FILE", null),
                set("AWS_SHARED_CREDENTIALS_FILE", null),
                set("AWS_EC2_METADATA_DISABLED", "true"),
            ]);
            env.extend(
                [
                    "AWS_PROFILE",
                    "AWS_DEFAULT_PROFILE",
                    "AWS_ACCESS_KEY_ID",
                    "AWS_SECRET_ACCESS_KEY",
                    "AWS_SESSION_TOKEN",
                    "AWS_SECURITY_TOKEN",
                    "AWS_WEB_IDENTITY_TOKEN_FILE",
                    "AWS_ROLE_ARN",
                    "AWS_CONTAINER_CREDENTIALS_RELATIVE_URI",
                    "AWS_CONTAINER_CREDENTIALS_FULL_URI",
                    "AWS_CONTAINER_AUTHORIZATION_TOKEN",
                    "AWS_CONTAINER_AUTHORIZATION_TOKEN_FILE",
                ]
                .map(unset),
            );
        }
        Flavor::Gcloud => env.extend([
            // Without this, gcloud falls back to its full (networked)
            // completer whenever the static tree can't answer.
            set("_ARGCOMPLETE_TRACE", "static"),
            set("CLOUDSDK_CORE_DISABLE_PROMPTS", "1"),
        ]),
        Flavor::Azure => {
            // A private config directory has no login, so resource completers
            // can't authenticate; extensions still come from the user's dir.
            let user_config = env::var_os("AZURE_CONFIG_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| crate::utils::expand_user("~/.azure"));
            let extensions = env::var_os("AZURE_EXTENSION_DIR")
                .unwrap_or_else(|| user_config.join("cliextensions").into_os_string());
            let config = crate::utils::cache_dir().join("azure");
            let _ = fs::create_dir_all(&config);
            env.extend([
                (OsString::from("AZURE_CONFIG_DIR"), Some(config.into())),
                (OsString::from("AZURE_EXTENSION_DIR"), Some(extensions)),
                set("AZURE_CORE_COLLECT_TELEMETRY", "false"),
                set("AZURE_CORE_NO_COLOR", "true"),
            ]);
        }
        Flavor::Argcomplete => {}
    }
    env
}

/// The completion line: the app name, the words so far, and the prefix.
fn completion_line(name: &str, words: &[&str], prefix: &str) -> String {
    let mut line = name.to_owned();
    for word in words {
        line.push(' ');
        line.push_str(&shlex::quote(word));
    }
    line.push(' ');
    line.push_str(prefix);
    line
}

impl NativeCompletionBackend for Backend {
    fn id(&self) -> &str {
        match self.flavor {
            Flavor::AwsCompleter => "aws",
            Flavor::Gcloud => "gcloud",
            Flavor::Azure => "az",
            Flavor::Argcomplete => "argcomplete",
        }
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            subcommands: true,
            options: true,
            option_arity: self.flavor == Flavor::Gcloud,
            values: false,
            resources: false,
            // gcloud's static tree only matches `--`; argparse apps list
            // every option (short ones too) for `-`.
            option_prefix: match self.flavor {
                Flavor::AwsCompleter | Flavor::Gcloud => "--",
                Flavor::Azure | Flavor::Argcomplete => "-",
            },
        }
    }

    fn complete(
        &self,
        words: &[&str],
        prefix: &str,
        budget: &mut Budget,
    ) -> Result<Vec<CompletionItem>, CompletionError> {
        let line = completion_line(&self.name, words, prefix);
        // aws_completer and gcloud's lookup slice the line as Python text;
        // argcomplete expects the byte offset bash supplies.
        let point = match self.flavor {
            Flavor::AwsCompleter | Flavor::Gcloud => line.chars().count(),
            Flavor::Azure | Flavor::Argcomplete => line.len(),
        };
        let mut env = offline_env(self.flavor);
        env.push(("COMP_LINE".into(), Some(line.into())));
        env.push(("COMP_POINT".into(), Some(point.to_string().into())));
        let capture = if self.flavor == Flavor::AwsCompleter {
            Capture::Stdout
        } else {
            env.extend([
                ("_ARGCOMPLETE".into(), Some("1".into())),
                ("_ARGCOMPLETE_IFS".into(), Some("\x0b".into())),
                ("_ARGCOMPLETE_SUPPRESS_SPACE".into(), Some("1".into())),
                (
                    "_ARGCOMPLETE_COMP_WORDBREAKS".into(),
                    Some(" \t\n\"'><;|&(".into()),
                ),
            ]);
            Capture::Descriptor(8)
        };
        let output = probe::run(
            &Probe {
                program: &self.completer,
                args: Vec::new(),
                env,
                capture,
            },
            budget,
        )
        .map_err(|error| match error {
            ProbeError::BudgetExhausted => CompletionError::BudgetExhausted,
            ProbeError::Unsupported(why) => CompletionError::Unsupported(why.into()),
            other => CompletionError::Failed(other.to_string()),
        })?;
        if output.status != Some(0) {
            return Err(match self.flavor {
                // With tracing on, the static tree raises for positions it
                // has no data for (positionals, dynamic flag values).
                Flavor::Gcloud if output.data.is_empty() => CompletionError::Unsupported(
                    "gcloud's static completion data does not cover this position".into(),
                ),
                _ => CompletionError::Failed(format!("completer exited with {:?}", output.status)),
            });
        }
        let text = String::from_utf8_lossy(&output.data);
        let separator = if self.flavor == Flavor::AwsCompleter {
            '\n'
        } else {
            '\x0b'
        };
        Ok(normalize(
            text.split(separator),
            self.flavor,
            budget.max_candidates,
        ))
    }
}

/// Trims protocol decoration and removes duplicates, keeping the app's order.
fn normalize<'a>(
    raw: impl Iterator<Item = &'a str>,
    flavor: Flavor,
    limit: usize,
) -> Vec<CompletionItem> {
    let mut items: Vec<CompletionItem> = Vec::new();
    for entry in raw {
        let entry = entry.trim();
        if entry.is_empty() || entry.contains(char::is_whitespace) {
            continue;
        }
        // `--flag=` marks an option that takes a value; gcloud's static
        // tree marks every valued option that way, so the rest take none.
        let item = match entry.strip_suffix('=') {
            Some(name) if name.starts_with('-') => CompletionItem {
                value: name.to_owned(),
                takes_value: Some(true),
            },
            _ => CompletionItem {
                value: entry.to_owned(),
                takes_value: (flavor == Flavor::Gcloud && entry.starts_with("--")).then_some(false),
            },
        };
        if !items.iter().any(|i| i.value == item.value) {
            items.push(item);
            if items.len() >= limit {
                break;
            }
        }
    }
    items
}

#[cfg(all(test, unix))]
pub(crate) mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::time::Duration;

    /// A scratch directory removed on drop.
    pub(crate) struct Dir(pub PathBuf);

    impl Dir {
        pub(crate) fn new(tag: &str) -> Dir {
            use std::sync::atomic::{AtomicUsize, Ordering};
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let path = env::temp_dir().join(format!(
                "notypo-native-{tag}-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Dir(path)
        }

        pub(crate) fn script(&self, name: &str, body: &str) -> PathBuf {
            let path = self.0.join(name);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(&path, body).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
            path
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn budget() -> Budget {
        Budget::new(Duration::from_secs(20), Duration::from_secs(10), 16)
    }

    /// A fake aws_completer answering from COMP_LINE like the real one.
    pub(crate) const FAKE_AWS_COMPLETER: &str = r#"#!/bin/sh
# awscli test double
[ -n "$AWS_PROFILE$AWS_ACCESS_KEY_ID" ] && exit 3
[ "$AWS_CONFIG_FILE" = /dev/null ] || exit 4
[ "$HTTPS_PROXY" = http://127.0.0.1:9 ] || exit 5
case "$COMP_LINE" in
  "aws ") printf 'ec2\ns3\nsts\n';;
  "aws ec2 ") printf 'describe-instances\ndescribe-instance-status\nrun-instances\n';;
  "aws ec2 describe-instances --") printf -- '--instance-ids\n--region\n--dry-run\n';;
  "aws ec2 describe-instances --region eu-west-1 --") printf -- '--instance-ids\n--region\n--dry-run\n';;
  "aws ec2 describe-instances ") printf -- '--instance-ids\n--region\n--dry-run\n';;
  "aws --") printf -- '--region\n--profile\n--output\n';;
  "aws --region eu-west-1 ") printf 'ec2\ns3\nsts\n';;
esac
"#;

    #[test]
    fn aws_completer_protocol_and_offline_environment() {
        let dir = Dir::new("aws");
        let aws = dir.script("bin/aws", "#!/bin/sh\nexit 0\n");
        dir.script("bin/aws_completer", FAKE_AWS_COMPLETER);
        let Discovery::Found(backend) = discover("aws", &aws, &[]) else {
            panic!("aws_completer next to aws should be found");
        };
        assert_eq!(backend.id(), "aws");
        let mut budget = budget();
        let words: Vec<String> = backend
            .complete(&["ec2"], "", &mut budget)
            .unwrap()
            .into_iter()
            .map(|i| i.value)
            .collect();
        assert_eq!(
            words,
            [
                "describe-instances",
                "describe-instance-status",
                "run-instances"
            ]
        );
        assert_eq!(
            backend.complete(&["nope"], "", &mut budget),
            Ok(Vec::new()),
            "no matches is not an error"
        );
        assert_eq!(budget.spawned(), 2);
    }

    #[test]
    fn argcomplete_protocol_reads_descriptor_8_and_option_arity() {
        let dir = Dir::new("gcloud");
        let gcloud = dir.script(
            "sdk/bin/gcloud",
            r#"#!/bin/sh
[ "$_ARGCOMPLETE" = 1 ] && [ "$_ARGCOMPLETE_TRACE" = static ] || exit 9
echo noise
case "$COMP_LINE" in
  "gcloud compute ") printf 'instances\vinstance-groups\vzones' >&8;;
  "gcloud compute instances list --") printf -- '--format=\v--uri\v--zones=' >&8;;
  "gcloud compute instances list ") exit 1;;
esac
"#,
        );
        fs::create_dir_all(dir.0.join("sdk/lib/googlecloudsdk")).unwrap();
        let Discovery::Found(backend) = discover("gcloud", &gcloud, &[]) else {
            panic!("an SDK layout identifies gcloud");
        };
        let mut budget = budget();
        let commands = backend.complete(&["compute"], "", &mut budget).unwrap();
        assert_eq!(commands.len(), 3);
        assert_eq!(commands[0].value, "instances");
        let options = backend
            .complete(&["compute", "instances", "list"], "--", &mut budget)
            .unwrap();
        assert_eq!(
            options,
            [
                CompletionItem {
                    value: "--format".into(),
                    takes_value: Some(true)
                },
                CompletionItem {
                    value: "--uri".into(),
                    takes_value: Some(false)
                },
                CompletionItem {
                    value: "--zones".into(),
                    takes_value: Some(true)
                },
            ]
        );
        assert!(matches!(
            backend.complete(&["compute", "instances", "list"], "", &mut budget),
            Err(CompletionError::Unsupported(_))
        ));
    }

    #[test]
    fn discovery_requires_identity_and_trust() {
        let dir = Dir::new("discovery");
        let fake_gcloud = dir.script("bin/gcloud", "#!/bin/sh\n");
        assert_eq!(discover("gcloud", &fake_gcloud, &[]), Discovery::None);
        let other_aws = dir.script("other/aws", "#!/bin/sh\n");
        dir.script("other/aws_completer", "#!/bin/sh\n# unrelated\n");
        assert_eq!(discover("aws", &other_aws, &[]), Discovery::None);
        let az = dir.script("bin/az", "#!/bin/sh\npython -m azure.cli \"$@\"\n");
        assert!(
            matches!(discover("az", &az, &[]), Discovery::Found(b) if b.flavor == Flavor::Azure)
        );
        let tool = dir.script(
            "bin/tool",
            "#!/usr/bin/env python\n# PYTHON_ARGCOMPLETE_OK\n",
        );
        assert!(matches!(
            discover("tool", &tool, &[]),
            Discovery::NotTrusted(_)
        ));
        assert!(matches!(
            discover("tool", &tool, &["tool".into()]),
            Discovery::Found(b) if b.flavor == Flavor::Argcomplete
        ));
        assert!(matches!(
            discover("aws", Path::new("bin/aws"), &[]),
            Discovery::NotTrusted(_)
        ));
    }

    #[test]
    fn completion_lines_quote_context_words() {
        assert_eq!(
            completion_line("aws", &["s3", "a b"], "--"),
            "aws s3 'a b' --"
        );
        assert_eq!(completion_line("az", &[], ""), "az ");
    }
}
