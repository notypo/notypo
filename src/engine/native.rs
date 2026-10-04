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
use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::{env, fs};

mod bash;
mod fish;
mod git;
mod go;
mod identity;
mod zsh;

/// One valid word for a position, as reported by the app.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompletionItem {
    pub value: String,
    /// For options: whether a value follows, when the protocol says so.
    pub takes_value: Option<bool>,
    /// The app's own one-line description, when its protocol gives one.
    /// Untrusted text: control characters are removed and length capped.
    pub description: Option<String>,
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
    /// Short options (`-m`) are listed too; otherwise an unlisted short
    /// option says nothing about validity.
    pub short_options: bool,
    /// Option lists include every option. git's helper omits options parsed
    /// outside its option table (`git log --graph`).
    pub complete_options: bool,
    /// The handler enumerates every subcommand, rather than a partial shell list.
    pub complete_subcommands: bool,
    /// Words come with the app's own descriptions.
    pub descriptions: bool,
    /// How the query line is quoted for the completer; `None` when words are
    /// passed as separate arguments (cobra, git), so no dialect applies.
    pub query_dialect: Option<super::parser::Dialect>,
    pub trust: Trust,
}

/// Why notypo may run an app's completer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Trust {
    /// A built-in bridge identified the app from its installation
    /// (aws, gcloud, az, git).
    #[default]
    Bridge,
    /// The app's module is on the audited list.
    Audited,
    /// `trusted_completers` names the app or its identity.
    UserTrusted,
}

pub trait NativeCompletionBackend: std::fmt::Debug {
    /// Stable identity, such as `aws` or `argcomplete`.
    fn id(&self) -> &str;
    fn capabilities(&self) -> Capabilities;
    /// Words valid after `words` (the arguments following the program name)
    /// that start with `prefix`. `Ok(vec![])` means the app knows of none.
    /// Always offline.
    fn complete(
        &self,
        words: &[&str],
        prefix: &str,
        budget: &mut Budget,
    ) -> Result<Vec<CompletionItem>, CompletionError>;
    /// Where the answers come from, for reports (the completer's path).
    fn location(&self) -> String {
        self.id().to_owned()
    }
    /// What identifies these answers in the disk cache; `None` to skip it.
    fn cache_identity(&self) -> Option<String> {
        None
    }
    /// Values for the option that ends `words`. Offline unless resource
    /// lookups were allowed (see [`Capabilities::resources`]).
    fn complete_values(
        &self,
        words: &[&str],
        budget: &mut Budget,
    ) -> Result<Vec<CompletionItem>, CompletionError> {
        self.complete(words, "", budget)
    }

    /// The app's fixed values for that option, with resource lookups off,
    /// to tell enum values from resource names when lookups are on.
    fn complete_offline_values(
        &self,
        words: &[&str],
        budget: &mut Budget,
    ) -> Result<Vec<CompletionItem>, CompletionError> {
        self.complete_values(words, budget)
    }
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
    /// git's `--list-cmds` and `--git-completion-helper` (see [`git`]).
    Git,
    /// Go apps built with cobra: `app __complete <words> <prefix>`.
    Cobra,
    /// Go apps built with posener/complete (HashiCorp tools): `COMP_LINE`
    /// in, one word per line out.
    Posener,
    /// A function registered by the app's bash completion script (opt-in).
    BashFunction,
    /// An installed fish completion script, using `complete --do-complete`.
    FishScript,
    /// An installed zsh autoload function, queried inside a ZLE widget.
    ZshFunction,
}

/// Go apps whose completion was audited: the probe only lists words, and
/// the offline environment keeps their lookups off the network, clusters,
/// and daemons. Other apps using these libraries need `trusted_completers`.
const AUDITED_GO_APPS: &[(&str, Flavor)] = &[
    ("k8s.io/kubernetes/cmd/kubectl", Flavor::Cobra),
    ("helm.sh/helm/v3/cmd/helm", Flavor::Cobra),
    ("helm.sh/helm/v4/cmd/helm", Flavor::Cobra),
    ("github.com/cli/cli/v2/cmd/gh", Flavor::Cobra),
    ("github.com/hetznercloud/cli/cmd/hcloud", Flavor::Cobra),
    ("sigs.k8s.io/kind", Flavor::Cobra),
    // Lists plugin commands by asking each installed CLI plugin for its
    // metadata, as `docker help` does.
    ("github.com/docker/cli/cmd/docker", Flavor::Cobra),
    ("github.com/hashicorp/terraform", Flavor::Posener),
    ("github.com/hashicorp/packer", Flavor::Posener),
    ("github.com/opentofu/opentofu/cmd/tofu", Flavor::Posener),
];

/// Raw probe answers within one run, keyed by the arguments that asked.
type ProbeMemo = Rc<RefCell<HashMap<Vec<String>, Result<String, CompletionError>>>>;

#[derive(Clone, Debug)]
pub struct Backend {
    pub flavor: Flavor,
    /// The name the app is invoked by on the command line.
    pub name: String,
    /// The executable that answers completion queries.
    pub completer: PathBuf,
    /// The app's identity when the installation states it (a Go module).
    pub identity: Option<String>,
    pub trust: Trust,
    /// Value queries may use the network and the user's credentials.
    pub network: bool,
    /// A file the protocol needs besides the completer (a completion script).
    pub helper: Option<PathBuf>,
    /// The actual app when another executable (a shell/helper) performs completion.
    pub application: Option<PathBuf>,
    memo: ProbeMemo,
}

impl PartialEq for Backend {
    fn eq(&self, other: &Self) -> bool {
        (self.flavor, &self.name, &self.completer) == (other.flavor, &other.name, &other.completer)
    }
}

impl Eq for Backend {}

impl Backend {
    pub fn new(flavor: Flavor, name: &str, completer: PathBuf) -> Backend {
        Backend {
            flavor,
            name: name.to_owned(),
            completer,
            identity: None,
            network: false,
            helper: None,
            application: None,
            trust: match flavor {
                Flavor::AwsCompleter | Flavor::Gcloud | Flavor::Azure | Flavor::Git => {
                    Trust::Bridge
                }
                _ => Trust::UserTrusted,
            },
            memo: ProbeMemo::default(),
        }
    }

    pub fn with_helper(mut self, helper: PathBuf) -> Backend {
        self.helper = Some(helper);
        self
    }

    pub fn with_application(mut self, application: PathBuf) -> Backend {
        self.application = Some(application);
        self
    }

    /// What identifies this installation's answers in the disk cache:
    /// completer and app files, extension and component directories.
    /// `None` when answers depend on the working directory (git reads
    /// aliases from the repository's configuration).
    pub fn cache_identity(&self) -> Option<String> {
        if self.flavor == Flavor::Git {
            return None;
        }
        let real = fs::canonicalize(&self.completer).ok();
        let mut files = vec![self.completer.clone()];
        files.extend(real.clone());
        if let Some(app) = &self.application {
            files.push(app.clone());
            files.extend(fs::canonicalize(app).ok());
        }
        if let Some(helper) = &self.helper {
            // Completion functions may read local files: cache per directory.
            files.push(helper.clone());
            let cwd = env::current_dir().unwrap_or_default();
            return Some(super::cache::fingerprint(
                &files,
                &[self.id(), &self.name, &cwd.to_string_lossy()],
            ));
        }
        match self.flavor {
            Flavor::Gcloud => {
                if let Some(root) = real
                    .as_deref()
                    .and_then(Path::parent)
                    .and_then(Path::parent)
                {
                    files.push(root.join(".install"));
                    files.push(root.join("VERSION"));
                }
            }
            Flavor::Azure => files.push(azure_extension_dir()),
            _ => {}
        }
        Some(super::cache::fingerprint(&files, &[self.id(), &self.name]))
    }

    /// Runs `probe` once per run for the same `key`.
    fn memoized(
        &self,
        key: &[&str],
        probe: impl FnOnce() -> Result<String, CompletionError>,
    ) -> Result<String, CompletionError> {
        let key: Vec<String> = key.iter().map(|k| k.to_string()).collect();
        if let Some(hit) = self.memo.borrow().get(&key) {
            return hit.clone();
        }
        let result = probe();
        self.memo.borrow_mut().insert(key, result.clone());
        result
    }
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
    discover_for_shell(name, path, trusted, crate::shells::Shell::Bash)
}

pub fn discover_for_shell(
    name: &str,
    path: &Path,
    trusted: &[String],
    shell: crate::shells::Shell,
) -> Discovery {
    // Only an executable notypo may inspect is identified.
    let identity = probe::is_trusted_location(path)
        .then(|| identity::identify(path))
        .flatten();
    let identity = identity.as_deref();
    let found = match discover_protocol(name, path, trusted, identity) {
        Discovery::None => {
            let handlers: [HandlerDiscovery; 3] = match shell {
                crate::shells::Shell::Zsh => [zsh_function, bash_function, fish_script],
                crate::shells::Shell::Fish => [fish_script, bash_function, zsh_function],
                _ => [bash_function, fish_script, zsh_function],
            };
            handlers
                .into_iter()
                .map(|discover| discover(name, trusted, identity))
                .find(|found| !matches!(found, Discovery::None))
                .unwrap_or(Discovery::None)
        }
        found => found,
    };
    match found {
        Discovery::Found(mut backend) => {
            if backend.identity.is_none() {
                backend.identity = identity.map(str::to_owned);
            }
            Discovery::Found(backend.with_application(path.to_owned()))
        }
        other => other,
    }
}

/// Looks for an installed shell completion handler: name, trust, identity.
type HandlerDiscovery = fn(&str, &[String], Option<&str>) -> Discovery;

/// `name`, followed by the identity that distinguishes it from other apps
/// sharing the name, for messages.
fn described(name: &str, identity: Option<&str>) -> String {
    match identity {
        Some(identity) => format!("{name} ({identity})"),
        None => name.to_owned(),
    }
}

fn zsh_function(name: &str, trusted: &[String], identity: Option<&str>) -> Discovery {
    let name = app_name(name);
    let Some(script) = zsh::script_for(name) else {
        return Discovery::None;
    };
    if !identity::is_trusted(trusted, name, identity) {
        return Discovery::NotTrusted(format!(
            "{} has a zsh completion function ({}); add it to trusted_completers to use it",
            described(name, identity),
            script.display()
        ));
    }
    if !cfg!(unix) {
        return Discovery::Unavailable("ZLE completion requires a Unix terminal".into());
    }
    match crate::utils::which("zsh").filter(|p| probe::is_trusted_location(p)) {
        Some(zsh) => {
            Discovery::Found(Backend::new(Flavor::ZshFunction, name, zsh).with_helper(script))
        }
        None => Discovery::Unavailable("zsh is not installed".into()),
    }
}

fn fish_script(name: &str, trusted: &[String], identity: Option<&str>) -> Discovery {
    let name = app_name(name);
    let Some(script) = fish::script_for(name) else {
        return Discovery::None;
    };
    if !identity::is_trusted(trusted, name, identity) {
        return Discovery::NotTrusted(format!(
            "{} has a fish completion script ({}); add it to trusted_completers to use it",
            described(name, identity),
            script.display()
        ));
    }
    match crate::utils::which("fish").filter(|f| probe::is_trusted_location(f)) {
        Some(fish) => {
            Discovery::Found(Backend::new(Flavor::FishScript, name, fish).with_helper(script))
        }
        None => Discovery::Unavailable("fish is not installed".into()),
    }
}

/// A trusted program's bash completion function, when it has nothing else.
fn bash_function(name: &str, trusted: &[String], identity: Option<&str>) -> Discovery {
    let name = app_name(name);
    let Some(script) = bash::script_for(name) else {
        return Discovery::None;
    };
    if !identity::is_trusted(trusted, name, identity) {
        return Discovery::NotTrusted(format!(
            "{} has a bash completion script ({}); add it to trusted_completers to use it",
            described(name, identity),
            script.display()
        ));
    }
    match crate::utils::which("bash").filter(|b| probe::is_trusted_location(b)) {
        Some(bash) => {
            Discovery::Found(Backend::new(Flavor::BashFunction, name, bash).with_helper(script))
        }
        None => Discovery::Unavailable("bash is not installed".into()),
    }
}

fn discover_protocol(
    name: &str,
    path: &Path,
    trusted: &[String],
    identity: Option<&str>,
) -> Discovery {
    if !probe::is_trusted_location(path) {
        return Discovery::NotTrusted(format!(
            "{} was not resolved to an absolute path",
            path.display()
        ));
    }
    let name = app_name(name);
    let found =
        |flavor, completer: PathBuf| Discovery::Found(Backend::new(flavor, name, completer));
    match name {
        "git" => found(Flavor::Git, path.to_owned()),
        "aws" => match aws_completer(path) {
            Some(completer) => found(Flavor::AwsCompleter, completer),
            None => Discovery::None,
        },
        "gcloud" if is_gcloud(path) => argcomplete(Flavor::Gcloud, name, path),
        "az" if head(path, 4096).contains("azure.cli") => argcomplete(Flavor::Azure, name, path),
        _ if head(path, 1024).contains("PYTHON_ARGCOMPLETE_OK") => {
            if identity::is_trusted(trusted, name, identity) {
                argcomplete(Flavor::Argcomplete, name, path)
            } else {
                Discovery::NotTrusted(format!(
                    "{} declares argcomplete support; add it to trusted_completers to use it",
                    described(name, identity)
                ))
            }
        }
        _ if !head(path, 2).starts_with("#!") => go_app(name, path, trusted),
        _ => Discovery::None,
    }
}

/// A Go binary built with a completion library notypo speaks.
fn go_app(name: &str, path: &Path, trusted: &[String]) -> Discovery {
    let real = fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    let Some(module) = go::module(&real) else {
        return Discovery::None;
    };
    match go_flavor(&module, name, trusted) {
        Ok(flavor) => {
            let mut backend = Backend::new(flavor, name, path.to_owned());
            if AUDITED_GO_APPS.iter().any(|(app, _)| *app == module.path) {
                backend.trust = Trust::Audited;
            }
            backend.identity = Some(module.path);
            Discovery::Found(backend)
        }
        Err(GoRefusal::NoProtocol) => Discovery::None,
        Err(GoRefusal::NotTrusted(why)) => Discovery::NotTrusted(why),
    }
}

/// The protocol a Go app speaks, if notypo may use it: audited apps by
/// module path, others only when `trusted` names them.
/// Why a Go app's completion can't be used.
#[derive(Debug, PartialEq, Eq)]
enum GoRefusal {
    /// It doesn't use a completion library notypo speaks.
    NoProtocol,
    NotTrusted(String),
}

fn go_flavor(module: &go::GoModule, name: &str, trusted: &[String]) -> Result<Flavor, GoRefusal> {
    let audited = AUDITED_GO_APPS
        .iter()
        .find(|(app, _)| *app == module.path)
        .map(|(_, flavor)| *flavor);
    let flavor = if module.uses("github.com/posener/complete") {
        Flavor::Posener
    } else if module.uses("github.com/spf13/cobra") {
        Flavor::Cobra
    } else if let Some(flavor) = audited {
        flavor
    } else {
        return Err(GoRefusal::NoProtocol);
    };
    if audited.is_none() && !identity::is_trusted(trusted, name, Some(&module.path)) {
        return Err(GoRefusal::NotTrusted(format!(
            "{name} ({}) supports {} completion; add it to trusted_completers to use it",
            module.path,
            if flavor == Flavor::Cobra {
                "cobra"
            } else {
                "posener"
            }
        )));
    }
    Ok(flavor)
}

fn argcomplete(flavor: Flavor, name: &str, path: &Path) -> Discovery {
    if cfg!(windows) {
        return Discovery::Unavailable(
            "the argcomplete protocol writes to file descriptor 8, which Windows lacks".into(),
        );
    }
    Discovery::Found(Backend::new(flavor, name, path.to_owned()))
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
            let extensions = azure_extension_dir().into_os_string();
            let config = crate::utils::cache_dir().join("azure");
            let _ = fs::create_dir_all(&config);
            env.extend([
                (OsString::from("AZURE_CONFIG_DIR"), Some(config.into())),
                (OsString::from("AZURE_EXTENSION_DIR"), Some(extensions)),
                set("AZURE_CORE_COLLECT_TELEMETRY", "false"),
                set("AZURE_CORE_NO_COLOR", "true"),
            ]);
        }
        Flavor::Cobra => env.extend([
            // Cluster, daemon, and API lookups (resource names) stay off.
            set("KUBECONFIG", null),
            set("DOCKER_HOST", "unix:///nonexistent/notypo-offline.sock"),
            set("GH_PROMPT_DISABLED", "1"),
        ]),
        Flavor::Argcomplete
        | Flavor::Git
        | Flavor::Posener
        | Flavor::BashFunction
        | Flavor::FishScript
        | Flavor::ZshFunction => {}
    }
    env
}

/// The environment for resource lookups the user allowed: their own
/// credentials and network, but never a prompt or a pager.
fn network_env(flavor: Flavor) -> Vec<(OsString, Option<OsString>)> {
    let set = |k: &str, v: &str| (OsString::from(k), Some(OsString::from(v)));
    let mut env = vec![set("AWS_PAGER", ""), set("GIT_TERMINAL_PROMPT", "0")];
    if flavor == Flavor::Gcloud {
        env.push(set("CLOUDSDK_CORE_DISABLE_PROMPTS", "1"));
    }
    env
}

/// Runs a completion helper with explicit arguments and returns its stdout.
fn run_stdout(
    program: &Path,
    args: Vec<OsString>,
    env: Vec<(OsString, Option<OsString>)>,
    budget: &mut Budget,
    require_success: bool,
) -> Result<String, CompletionError> {
    let output = probe::run(
        &Probe {
            program,
            args,
            env,
            capture: Capture::Stdout,
        },
        budget,
    )
    .map_err(probe_error)?;
    if require_success && output.status != Some(0) {
        return Err(CompletionError::Failed(format!(
            "exited with {:?}",
            output.status
        )));
    }
    if output.truncated {
        return Err(CompletionError::Failed(
            "completion output exceeded the probe limit".into(),
        ));
    }
    String::from_utf8(output.data)
        .map_err(|_| CompletionError::Failed("completion output is not valid UTF-8".into()))
}

fn probe_error(error: ProbeError) -> CompletionError {
    match error {
        ProbeError::BudgetExhausted => CompletionError::BudgetExhausted,
        ProbeError::Unsupported(why) => CompletionError::Unsupported(why.into()),
        other => CompletionError::Failed(other.to_string()),
    }
}

/// Where the user's Azure CLI extensions are installed.
fn azure_extension_dir() -> PathBuf {
    env::var_os("AZURE_EXTENSION_DIR").map_or_else(
        || {
            env::var_os("AZURE_CONFIG_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| crate::utils::expand_user("~/.azure"))
                .join("cliextensions")
        },
        PathBuf::from,
    )
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
            Flavor::Git => "git",
            Flavor::Cobra => "cobra",
            Flavor::Posener => "posener",
            Flavor::BashFunction => "bash",
            Flavor::FishScript => "fish",
            Flavor::ZshFunction => "zsh",
        }
    }

    fn location(&self) -> String {
        self.helper.as_ref().map_or_else(
            || self.completer.display().to_string(),
            |script| format!("{} (via {})", script.display(), self.completer.display()),
        )
    }

    fn cache_identity(&self) -> Option<String> {
        Backend::cache_identity(self)
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            subcommands: true,
            options: true,
            option_arity: matches!(
                self.flavor,
                Flavor::Gcloud | Flavor::Git | Flavor::FishScript
            ),
            values: self.flavor != Flavor::Git,
            resources: self.network && self.flavor != Flavor::Git,
            // gcloud's static tree only matches `--`; argparse apps list
            // every option (short ones too) for `-`; git lists long ones.
            option_prefix: match self.flavor {
                Flavor::AwsCompleter | Flavor::Gcloud => "--",
                _ => "-",
            },
            short_options: matches!(
                self.flavor,
                Flavor::Azure
                    | Flavor::Argcomplete
                    | Flavor::Cobra
                    | Flavor::Posener
                    | Flavor::BashFunction
                    | Flavor::FishScript
                    | Flavor::ZshFunction
            ),
            // git's helper and hand-written completion scripts omit options.
            complete_options: !matches!(
                self.flavor,
                Flavor::Git | Flavor::BashFunction | Flavor::FishScript | Flavor::ZshFunction
            ),
            complete_subcommands: !matches!(
                self.flavor,
                Flavor::BashFunction | Flavor::FishScript | Flavor::ZshFunction
            ),
            descriptions: matches!(self.flavor, Flavor::Cobra | Flavor::FishScript),
            query_dialect: match self.flavor {
                Flavor::Cobra | Flavor::Git => None,
                Flavor::FishScript => Some(super::parser::Dialect::Fish),
                _ => Some(super::parser::Dialect::Posix),
            },
            trust: self.trust,
        }
    }

    fn complete(
        &self,
        words: &[&str],
        prefix: &str,
        budget: &mut Budget,
    ) -> Result<Vec<CompletionItem>, CompletionError> {
        if self.flavor == Flavor::Git {
            let items = git::complete(self, words, prefix, budget)?;
            return Ok(items
                .into_iter()
                .filter(|i| i.value.starts_with(prefix))
                .take(budget.max_candidates)
                .collect());
        }
        self.query(words, prefix, budget, offline_env(self.flavor))
    }

    fn complete_values(
        &self,
        words: &[&str],
        budget: &mut Budget,
    ) -> Result<Vec<CompletionItem>, CompletionError> {
        let env = if self.network {
            network_env(self.flavor)
        } else {
            offline_env(self.flavor)
        };
        self.query(words, "", budget, env)
    }

    fn complete_offline_values(
        &self,
        words: &[&str],
        budget: &mut Budget,
    ) -> Result<Vec<CompletionItem>, CompletionError> {
        self.query(words, "", budget, offline_env(self.flavor))
    }
}

impl Backend {
    fn query(
        &self,
        words: &[&str],
        prefix: &str,
        budget: &mut Budget,
        mut env: Vec<(OsString, Option<OsString>)>,
    ) -> Result<Vec<CompletionItem>, CompletionError> {
        if self.flavor == Flavor::Git {
            return Err(CompletionError::Unsupported(
                "git completion does not list option values".into(),
            ));
        }
        if self.flavor == Flavor::Cobra {
            return self.complete_cobra(words, prefix, budget, env);
        }
        if self.flavor == Flavor::BashFunction {
            let script = self
                .helper
                .as_deref()
                .ok_or_else(|| CompletionError::Unsupported("no completion script".into()))?;
            return bash::complete(
                &self.completer,
                script,
                &self.name,
                words,
                prefix,
                env,
                budget,
            );
        }
        if self.flavor == Flavor::FishScript {
            let script = self
                .helper
                .as_deref()
                .ok_or_else(|| CompletionError::Unsupported("no completion script".into()))?;
            return fish::complete(
                &self.completer,
                script,
                &self.name,
                words,
                prefix,
                env,
                budget,
            );
        }
        if self.flavor == Flavor::ZshFunction {
            let script = self
                .helper
                .as_deref()
                .ok_or_else(|| CompletionError::Unsupported("no completion function".into()))?;
            return zsh::complete(
                &self.completer,
                script,
                &self.name,
                words,
                prefix,
                env,
                budget,
            );
        }
        let line = completion_line(&self.name, words, prefix);
        // aws_completer and gcloud's lookup slice the line as Python text;
        // argcomplete expects the byte offset bash supplies.
        let point = match self.flavor {
            Flavor::AwsCompleter
            | Flavor::Gcloud
            | Flavor::Git
            | Flavor::Cobra
            | Flavor::BashFunction => line.chars().count(),
            Flavor::FishScript | Flavor::ZshFunction => {
                unreachable!("shell queries use their own driver")
            }
            Flavor::Azure | Flavor::Argcomplete | Flavor::Posener => line.len(),
        };
        env.push(("COMP_LINE".into(), Some(line.into())));
        env.push(("COMP_POINT".into(), Some(point.to_string().into())));
        let capture = if matches!(self.flavor, Flavor::AwsCompleter | Flavor::Posener) {
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
        .map_err(probe_error)?;
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
        if output.truncated {
            return Err(CompletionError::Failed(
                "completion output exceeded the probe limit".into(),
            ));
        }
        let text = String::from_utf8(output.data)
            .map_err(|_| CompletionError::Failed("completion output is not valid UTF-8".into()))?;
        let separator = if matches!(self.flavor, Flavor::AwsCompleter | Flavor::Posener) {
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

impl Backend {
    /// cobra's hidden `__complete` command: the words are passed as
    /// arguments (no shell parsing), the answer is `word<TAB>description`
    /// lines and a final `:<directive>` line.
    fn complete_cobra(
        &self,
        words: &[&str],
        prefix: &str,
        budget: &mut Budget,
        env: Vec<(OsString, Option<OsString>)>,
    ) -> Result<Vec<CompletionItem>, CompletionError> {
        let mut args: Vec<OsString> = vec!["__complete".into()];
        args.extend(words.iter().map(OsString::from));
        args.push(prefix.into());
        let text = run_stdout(&self.completer, args, env, budget, true)?;
        let mut lines: Vec<&str> = text.lines().collect();
        let directive = lines
            .iter()
            .rposition(|l| l.starts_with(':'))
            .and_then(|i| lines.drain(i..).next())
            .and_then(|d| d[1..].trim().parse::<u32>().ok())
            .ok_or_else(|| CompletionError::Failed("no cobra completion directive".into()))?;
        // Bit 1 is ShellCompDirectiveError.
        if directive & 1 != 0 {
            return Err(CompletionError::Unsupported(
                "the app reported a completion error here".into(),
            ));
        }
        Ok(normalize(
            lines.into_iter(),
            Flavor::Cobra,
            budget.max_candidates,
        ))
    }
}

/// An app's description made safe to print: control and bidirectional
/// formatting characters removed, one line, at most 120 characters.
pub(crate) fn clean_description(text: &str) -> Option<String> {
    let invisible = |c: char| {
        c.is_control()
            || matches!(c, '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{feff}')
    };
    let cleaned: String = text
        .chars()
        .map(|c| if invisible(c) { ' ' } else { c })
        .collect();
    let line = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    match line.chars().count() {
        0 => None,
        n if n > 120 => Some(line.chars().take(119).chain(['…']).collect()),
        _ => Some(line),
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
        // cobra answers `word<TAB>description`.
        let (entry, description) = match entry.split_once('\t') {
            Some((word, description)) if flavor == Flavor::Cobra => {
                (word, clean_description(description))
            }
            _ => (entry, None),
        };
        let entry = entry.trim();
        if entry.is_empty()
            || entry.contains(char::is_whitespace)
            || entry.contains(char::is_control)
        {
            continue;
        }
        // `--flag=` marks an option that takes a value; gcloud's static
        // tree marks every valued option that way, so the rest take none.
        let item = match entry.strip_suffix('=') {
            Some(name) if name.starts_with('-') => CompletionItem {
                value: name.to_owned(),
                takes_value: Some(true),
                description,
            },
            _ => CompletionItem {
                value: entry.to_owned(),
                takes_value: (flavor == Flavor::Gcloud && entry.starts_with("--")).then_some(false),
                description,
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

    /// A git double that records every invocation, so tests can check that
    /// only listing and helper calls on builtins ever happen.
    pub(crate) const FAKE_GIT: &str = r#"#!/bin/sh
dir=$(dirname "$0")
echo "$*" >> "$dir/calls"
case "$*" in
  "--list-cmds=main,others,alias,nohelpers") printf 'add\ncommit\nstatus\nstash\npush\nco\n';;
  "--list-cmds=builtins") printf 'add\ncommit\nstatus\nstash\npush\n';;
  "-h") printf 'usage: git [-v | --version] [-C <path>] [-c <name>=<value>]\n           [--exec-path[=<path>]] [-p | --paginate | -P | --no-pager] [--git-dir=<path>]\n\nmore\n'; exit 129;;
  init*) mkdir -p "$4/.git" && touch "$4/.git/HEAD";;
  *"status --git-completion-helper") printf -- '--short --branch --porcelain -- --no-short\n';;
  *"stash --git-completion-helper") printf 'apply clear drop pop push\n';;
  *"stash pop --git-completion-helper") printf -- '--index --quiet -- --no-index\n';;
  *"commit --git-completion-helper") printf -- '--message= --amend --all\n';;
  *) touch "$dir/ran"; exit 1;;
esac
"#;

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
                    takes_value: Some(true),
                    description: None,
                },
                CompletionItem {
                    value: "--uri".into(),
                    takes_value: Some(false),
                    description: None,
                },
                CompletionItem {
                    value: "--zones".into(),
                    takes_value: Some(true),
                    description: None,
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
        // Installed shell handlers may exist independently of these fakes.
        // The audited app protocol still requires the app's own identity.
        assert_eq!(
            discover_protocol("gcloud", &fake_gcloud, &[], None),
            Discovery::None
        );
        let other_aws = dir.script("other/aws", "#!/bin/sh\n");
        dir.script("other/aws_completer", "#!/bin/sh\n# unrelated\n");
        assert_eq!(
            discover_protocol("aws", &other_aws, &[], None),
            Discovery::None
        );
        assert!(!matches!(
            discover("aws", &other_aws, &[]),
            Discovery::Found(_)
        ));
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

    /// How a fixture app is packaged.
    enum Install {
        Go(&'static str),
        /// A Python console script declaring argcomplete, by entry module.
        Python(&'static str),
        Npm(&'static str),
    }

    impl Install {
        fn create(&self, dir: &Dir, name: &str, tag: &str) -> (PathBuf, String) {
            match self {
                Install::Go(module) => {
                    let path = dir.0.join(tag).join(name);
                    fs::create_dir_all(path.parent().unwrap()).unwrap();
                    fs::write(&path, go::fake_binary(module, &["github.com/spf13/cobra"]))
                        .unwrap();
                    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
                    (path, module.to_string())
                }
                Install::Python(module) => (
                    dir.script(
                        &format!("{tag}/{name}"),
                        &format!(
                            "#!/usr/bin/env python3\n# PYTHON_ARGCOMPLETE_OK\nimport sys\nfrom {module}.cli import main\nsys.exit(main())\n"
                        ),
                    ),
                    format!("python:{module}"),
                ),
                Install::Npm(package) => {
                    let root = dir.0.join(tag).join("lib/node_modules").join(package);
                    let target = dir.script(
                        &format!("{tag}/lib/node_modules/{package}/bin/cli.js"),
                        "#!/usr/bin/env node\n",
                    );
                    fs::write(
                        root.join("package.json"),
                        format!(r#"{{"name": "{package}", "bin": {{"{name}": "bin/cli.js"}}}}"#),
                    )
                    .unwrap();
                    let link = dir.0.join(tag).join("bin").join(name);
                    fs::create_dir_all(link.parent().unwrap()).unwrap();
                    std::os::unix::fs::symlink(&target, &link).unwrap();
                    (link, format!("npm:{package}"))
                }
            }
        }

        fn has_protocol(&self) -> bool {
            !matches!(self, Install::Npm(_))
        }
    }

    /// Unrelated apps sharing a basename are told apart by their package
    /// metadata, never by the name: trust given to one app's identity
    /// doesn't reach the other, follows the app when it's renamed, and an
    /// audit (Hetzner's hcloud) belongs to the audited module only. The
    /// fixture identities stand in for the real packages; each real client's
    /// identity is recorded when it is verified (section 5A of the plan).
    #[test]
    fn basename_collisions_are_resolved_by_app_identity() {
        let dir = Dir::new("collisions");
        let pairs = [
            (
                "hcloud",
                Install::Go("github.com/hetznercloud/cli/cmd/hcloud"),
                Install::Go("example.com/huawei/koocli/cmd/hcloud"),
            ),
            (
                "cf",
                Install::Go("example.com/cloudfoundry/cli/cmd/cf"),
                Install::Npm("example-cloudflare-cf"),
            ),
            (
                "metal",
                Install::Go("example.com/equinix/metal-cli/cmd/metal"),
                Install::Go("example.com/metal-stack-cloud/cli/cmd/metal"),
            ),
            (
                "atlas",
                Install::Go("example.com/mongodb/atlas-cli/cmd/atlas"),
                Install::Go("ariga.io/atlas/cmd/atlas"),
            ),
            (
                "exo",
                Install::Go("example.com/exoscale/cli"),
                Install::Python("exo"),
            ),
            (
                "stackit",
                Install::Go("example.com/stackitcloud/stackit-cli"),
                Install::Python("stackit_tools"),
            ),
            (
                "s",
                Install::Npm("@serverless-devs/s"),
                Install::Python("s_tool"),
            ),
        ];
        for (name, first, second) in &pairs {
            let (a, id_a) = first.create(&dir, name, &format!("{name}-a"));
            let (b, id_b) = second.create(&dir, name, &format!("{name}-b"));
            assert_eq!(
                identity::identify(&a).as_deref(),
                Some(id_a.as_str()),
                "{name}"
            );
            assert_eq!(
                identity::identify(&b).as_deref(),
                Some(id_b.as_str()),
                "{name}"
            );
            assert_ne!(id_a, id_b);

            // Trust pinned to one identity: the other app is not probed.
            let pinned = [id_b.clone()];
            match discover(name, &b, &pinned) {
                Discovery::Found(backend) if second.has_protocol() => {
                    assert_eq!(backend.identity.as_deref(), Some(id_b.as_str()));
                }
                Discovery::None if !second.has_protocol() => {}
                other => panic!("{name} b: {other:?}"),
            }
            let audited = *name == "hcloud";
            match discover(name, &a, &pinned) {
                Discovery::Found(backend) if audited => {
                    assert_eq!(backend.identity.as_deref(), Some(id_a.as_str()));
                }
                Discovery::NotTrusted(why) if first.has_protocol() && !audited => {
                    assert!(why.contains(&id_a), "{why}");
                }
                Discovery::None if !first.has_protocol() => {}
                other => panic!("{name} a: {other:?}"),
            }

            // Trust follows the app under another name.
            if first.has_protocol() && !audited {
                let renamed = dir.0.join(format!("{name}-a/renamed-{name}"));
                fs::copy(&a, &renamed).unwrap();
                assert!(matches!(
                    discover(
                        &format!("renamed-{name}"),
                        &renamed,
                        std::slice::from_ref(&id_a)
                    ),
                    Discovery::Found(_)
                ));
                assert!(matches!(
                    discover(&format!("renamed-{name}"), &renamed, &[name.to_string()]),
                    Discovery::NotTrusted(_)
                ));
            }
        }
        // The audit names Hetzner's module; Huawei's hcloud needs trust.
        let (huawei, _) = pairs[0].2.create(&dir, "hcloud", "hcloud-c");
        assert!(matches!(
            discover("hcloud", &huawei, &[]),
            Discovery::NotTrusted(_)
        ));
    }

    #[test]
    fn go_apps_are_identified_by_module_and_trusted_by_audit() {
        let module = |path: &str, libraries: &[&str]| go::GoModule {
            path: path.into(),
            libraries: libraries.iter().map(|l| l.to_string()).collect(),
        };
        let cobra = "github.com/spf13/cobra";
        assert_eq!(
            go_flavor(
                &module("k8s.io/kubernetes/cmd/kubectl", &[cobra]),
                "kubectl",
                &[]
            ),
            Ok(Flavor::Cobra)
        );
        assert_eq!(
            go_flavor(
                &module("github.com/docker/cli/cmd/docker", &[]),
                "docker",
                &[]
            ),
            Ok(Flavor::Cobra),
            "an audited app needs no dependency list"
        );
        assert_eq!(
            go_flavor(
                &module(
                    "github.com/hashicorp/packer",
                    &[cobra, "github.com/posener/complete"]
                ),
                "packer",
                &[]
            ),
            Ok(Flavor::Posener)
        );
        let other = module("example.com/huawei/hcloud", &[cobra]);
        assert!(matches!(
            go_flavor(&other, "hcloud", &[]),
            Err(GoRefusal::NotTrusted(_))
        ));
        assert_eq!(
            go_flavor(&other, "hcloud", &["hcloud".into()]),
            Ok(Flavor::Cobra)
        );
        assert!(matches!(
            go_flavor(&module("example.com/tool", &[]), "tool", &["*".into()]),
            Err(GoRefusal::NoProtocol)
        ));
    }

    #[test]
    fn descriptions_are_printable_single_lines() {
        assert_eq!(
            clean_description("  Display\tone\n or many  ").as_deref(),
            Some("Display one or many")
        );
        assert_eq!(
            clean_description("\x1b]0;pwned\x07Create").as_deref(),
            Some("]0;pwned Create")
        );
        assert_eq!(clean_description("a\u{202e}b").as_deref(), Some("a b"));
        assert_eq!(clean_description(" \t "), None);
        let long = clean_description(&"x".repeat(500)).unwrap();
        assert_eq!(long.chars().count(), 120);
        assert!(long.ends_with('…'));
    }

    #[test]
    fn cobra_and_posener_protocols() {
        let dir = Dir::new("go-protocols");
        let cobra = dir.script(
            "bin/app",
            "#!/bin/sh\n[ \"$KUBECONFIG\" = /dev/null ] || exit 7\n[ \"$1\" = __complete ] || { touch \"$(dirname \"$0\")/ran\"; exit 1; }\nshift\ncase \"$*\" in\n  \"\") printf 'get\\tDisplay one or many\\ncreate\\tCreate \\033]0;title\\007it\\n:4\\n';;\n  \"get -\") printf -- '--namespace\\tns\\n-n\\tns\\n--watch\\twatch\\n:4\\n';;\n  \"get pods \") printf ':1\\n';;\n  *) printf 'garbage\\n';;\nesac\n",
        );
        let backend = Backend::new(Flavor::Cobra, "app", cobra);
        let mut budget = budget();
        let words: Vec<String> = backend
            .complete(&[], "", &mut budget)
            .unwrap()
            .into_iter()
            .map(|i| i.value)
            .collect();
        assert_eq!(words, ["get", "create"]);
        let descriptions: Vec<Option<String>> = backend
            .complete(&[], "", &mut budget)
            .unwrap()
            .into_iter()
            .map(|i| i.description)
            .collect();
        assert_eq!(
            descriptions,
            [
                Some("Display one or many".into()),
                Some("Create ]0;title it".into())
            ],
            "the app's descriptions are kept, without terminal controls"
        );
        let options = backend.complete(&["get"], "-", &mut budget).unwrap();
        assert_eq!(options.len(), 3);
        assert!(matches!(
            backend.complete(&["get", "pods"], " ", &mut budget),
            Err(CompletionError::Failed(_))
        ));
        assert!(
            matches!(
                backend.complete(&["get", "pods"], "", &mut budget),
                Err(CompletionError::Unsupported(_))
            ),
            "the error directive marks a position the app can't complete"
        );
        assert!(!dir.0.join("bin/ran").exists());

        let posener = dir.script(
            "bin/tf",
            "#!/bin/sh\n[ -n \"$COMP_LINE\" ] || { touch \"$(dirname \"$0\")/ran\"; exit 1; }\ncase \"$COMP_LINE\" in\n  \"tf \") printf 'plan\\napply\\n';;\n  \"tf apply -\") printf -- '-auto-approve\\n-var\\n';;\nesac\n",
        );
        let backend = Backend::new(Flavor::Posener, "tf", posener);
        assert_eq!(backend.complete(&[], "", &mut budget).unwrap().len(), 2);
        assert_eq!(
            backend.complete(&["apply"], "-", &mut budget).unwrap()[0].value,
            "-auto-approve"
        );
        assert!(!dir.0.join("bin/ran").exists());
    }

    #[test]
    fn completion_lines_quote_context_words() {
        assert_eq!(
            completion_line("aws", &["s3", "a b"], "--"),
            "aws s3 'a b' --"
        );
        assert_eq!(completion_line("az", &[], ""), "az ");
    }

    #[test]
    fn bounded_protocol_outputs_reject_truncation_and_control_characters() {
        let dir = Dir::new("malformed-completion");
        for flavor in [Flavor::AwsCompleter, Flavor::Argcomplete, Flavor::Cobra] {
            let command = match flavor {
                Flavor::Argcomplete => "printf 'build\\vpublish\\vdeploy' >&8",
                Flavor::Cobra => "printf 'build\\npublish\\ndeploy\\n:4\\n'",
                _ => "printf 'build\\npublish\\ndeploy\\n'",
            };
            let app = dir.script("app", &format!("#!/bin/sh\n{command}\n"));
            let backend = Backend::new(flavor, "app", app);
            let mut budget = budget();
            budget.max_output = 8;
            let result = backend.complete(&[], "", &mut budget);
            assert!(
                matches!(result, Err(CompletionError::Failed(ref why)) if why.contains("probe limit")),
                "{flavor:?}: {result:?}"
            );
        }
        let items = normalize(
            [
                "build",
                "bad\0word",
                "bad\x1b[31m",
                "two words",
                "build",
                "publish;echo_marker",
            ]
            .into_iter(),
            Flavor::Argcomplete,
            8,
        );
        assert_eq!(
            items.iter().map(|i| i.value.as_str()).collect::<Vec<_>>(),
            ["build", "publish;echo_marker"]
        );
    }

    #[test]
    fn malformed_protocol_output_is_not_lossily_reinterpreted() {
        let dir = Dir::new("completion-encoding");
        for flavor in [Flavor::AwsCompleter, Flavor::Argcomplete, Flavor::Cobra] {
            let redirect = if flavor == Flavor::Argcomplete {
                ">&8"
            } else {
                ""
            };
            let app = dir.script(
                "app",
                &format!("#!/bin/sh\nprintf 'build\\n\\377broken\\n:4\\n' {redirect}\n"),
            );
            let backend = Backend::new(flavor, "app", app);
            let result = backend.complete(&[], "", &mut budget());
            assert!(
                matches!(result, Err(CompletionError::Failed(why)) if why.contains("UTF-8")),
                "{flavor:?}"
            );
        }
    }
}
