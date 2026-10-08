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
//! credentials. Local resource answers stay offline and require approval;
//! network resource queries require the separate network policy.

use super::probe::{self, Budget, Capture, Probe, ProbeError};
use crate::shlex;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::{env, fs};

mod bash;
mod cargo;
mod clang;
mod clap;
mod click;
mod dotnet;
mod fish;
mod git;
mod go;
mod go_flags;
mod identity;
mod kingpin;
mod node;
mod npm;
mod oclif;
mod pip;
pub mod powershell;
mod powershell_completer;
mod powershell_values;
mod rabbitmq;
mod symfony;
mod urfave;
mod yargs;
mod zsh;

pub use identity::{identify as app_identity, package_manager_name};

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

/// What a list of words in a command's argument slot is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArgumentList {
    /// Predictions of a partial kind (vault's fixed engine names): the
    /// typed word, which they lack, is an argument they never judge.
    Unjudged,
    /// Values offered again after any of them (make targets, dig's record
    /// types, mtr's placeholders): judged, but a repair from them is a
    /// resource the user must approve.
    Resources,
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

/// Literal arguments retain whether quoting made a dash-prefixed word
/// data rather than a parameter. The target word is omitted.
#[derive(Clone, Copy, Debug)]
pub struct ValueContext<'a> {
    pub words: &'a [&'a str],
    pub following: &'a [&'a str],
    pub literal_arguments: (&'a [usize], &'a [usize]),
    pub positional: bool,
}

pub trait NativeCompletionBackend: std::fmt::Debug {
    /// Stable identity, such as `aws` or `argcomplete`.
    fn id(&self) -> &str;
    fn capabilities(&self) -> Capabilities;
    /// Some protocols enumerate every top-level command but return partial
    /// option or resource lists deeper in the command. Describe that position.
    fn capabilities_for(&self, _words: &[&str]) -> Capabilities {
        self.capabilities()
    }
    /// Protocol-specific option syntax, beyond the shell's usual `-`/`=`.
    fn option_separator(&self, _typed: &str) -> Option<char> {
        None
    }

    fn is_short_option(&self, typed: &str) -> bool {
        typed.len() > 1 && !typed.starts_with("--")
    }

    /// Length of an option name whose characters include a value separator,
    /// such as the SDK's `--debug:custom-hive`.
    fn option_name_len(&self, _typed: &str) -> Option<usize> {
        None
    }
    fn option_prefix(&self, _typed: &str) -> &'static str {
        self.capabilities().option_prefix
    }
    fn is_option(&self, _words: &[&str], item: &CompletionItem) -> bool {
        item.is_option()
    }
    /// Required/optional value handling when the app reports precise arity.
    fn option_requires_value(
        &self,
        _words: &[&str],
        _typed: &str,
        _next: Option<&str>,
        _budget: &mut Budget,
    ) -> Option<bool> {
        None
    }
    /// A positional list known to contain resource names rather than command
    /// vocabulary, even when the resources are local and require no network.
    fn candidates_are_resources(&self, _words: &[&str]) -> bool {
        false
    }
    /// A mixed list may include command words and local selectors/resources.
    fn candidate_is_resource(&self, words: &[&str], _item: &CompletionItem) -> bool {
        self.candidates_are_resources(words)
    }
    /// Only a protocol with a declared value grammar may interpret slashes
    /// as something other than paths or URIs.
    fn value_syntax_contains_slashes(&self, _words: &[&str]) -> bool {
        false
    }
    /// A callback can depend on earlier bound arguments. Do not substitute
    /// placeholders for expansions the shell has not evaluated.
    fn values_require_literal_context(&self, _words: &[&str]) -> bool {
        false
    }
    /// Adapt a declared value list to compound values, preserving separators
    /// and already valid components. The default uses individual words.
    fn prepare_value_candidates(
        &self,
        _words: &[&str],
        _typed: &str,
        items: Vec<CompletionItem>,
    ) -> Result<Vec<CompletionItem>, CompletionError> {
        Ok(items)
    }
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

    /// Some binders inspect parameters after the slot being corrected too.
    /// `following` contains those literal words, in their original order.
    fn complete_values_with_following(
        &self,
        words: &[&str],
        _following: &[&str],
        budget: &mut Budget,
    ) -> Result<Vec<CompletionItem>, CompletionError> {
        self.complete_values(words, budget)
    }

    /// Positional arguments bound by the app, rather than subcommands.
    fn supports_positional_values(&self) -> bool {
        false
    }

    fn complete_value_context(
        &self,
        context: &ValueContext<'_>,
        budget: &mut Budget,
    ) -> Result<Vec<CompletionItem>, CompletionError> {
        if context.positional {
            self.complete_positionals_with_following(context.words, context.following, budget)
        } else {
            self.complete_values_with_following(context.words, context.following, budget)
        }
    }

    fn complete_positionals_with_following(
        &self,
        _words: &[&str],
        _following: &[&str],
        _budget: &mut Budget,
    ) -> Result<Vec<CompletionItem>, CompletionError> {
        Ok(Vec::new())
    }

    fn prepare_positional_value_candidates(
        &self,
        _context: &ValueContext<'_>,
        _typed: &str,
        items: Vec<CompletionItem>,
    ) -> Result<Vec<CompletionItem>, CompletionError> {
        Ok(items)
    }

    fn positional_candidate_is_resource(
        &self,
        _context: &ValueContext<'_>,
        _item: &CompletionItem,
    ) -> bool {
        true
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

    /// Whether `typed`, missing from a partial list after `words`, is still
    /// a command the app accepts (an alias or hidden name it doesn't list).
    /// `None` when the protocol can't tell.
    fn confirms(&self, _words: &[&str], _typed: &str, _budget: &mut Budget) -> Option<bool> {
        None
    }

    /// Complete command lists that still omit aliases and hidden commands
    /// (cobra's), so [`Self::confirms`] is worth asking about a word they
    /// lack.
    fn lists_omit_aliases(&self) -> bool {
        false
    }

    /// Whether the words `listed` after `words` are the command's argument
    /// predictions or values rather than its subcommands, for protocols that
    /// answer both in one list. `None` when the protocol can't tell or the
    /// words name subcommands.
    fn lists_arguments(
        &self,
        _words: &[&str],
        _typed: &str,
        _listed: &[&str],
        _budget: &mut Budget,
    ) -> Option<ArgumentList> {
        None
    }

    /// The first word that isn't an option starts arguments of another
    /// program (node's script), so later words are not the app's to check.
    fn arguments_end_options(&self) -> bool {
        false
    }

    /// The listed option `typed` names under the protocol's own matching
    /// rules, when they go beyond exact spelling (PowerShell parameters
    /// ignore case and accept aliases and unambiguous prefixes). `None`
    /// leaves exact matching in charge.
    fn resolve_option(
        &self,
        _words: &[&str],
        _typed: &str,
        _budget: &mut Budget,
    ) -> Option<CompletionItem> {
        None
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
    /// clap_complete's dynamic protocol, declared by an app-generated script.
    ClapDynamic,
    /// pip's `PIP_AUTO_COMPLETE`/`COMP_WORDS` protocol (opt-in).
    Pip,
    /// npm's `completion -- <words>` and COMP_* protocol (opt-in).
    Npm,
    /// Cargo's installed command list and bounded documentation/metadata.
    Cargo,
    /// The installed .NET SDK's `dotnet complete` and parser schema.
    Dotnet,
    /// Go apps built with urfave/cli: `app <commands> [-] <completion flag>`.
    Urfave,
    /// Click 8 apps: `<VAR>=zsh_complete` with COMP_WORDS/COMP_CWORD.
    Click,
    /// Go apps built with kingpin or fisk: `app --completion-bash <words>`.
    Kingpin,
    /// Go apps built with go-flags: `GO_FLAGS_COMPLETION=verbose app <words>`.
    GoFlags,
    /// Node apps built with yargs: `app --get-yargs-completions <words>`.
    Yargs,
    /// oclif apps, read from their packages' `oclif.manifest.json` files.
    Oclif,
    /// Symfony Console apps (Composer, framework consoles): `app _complete`.
    Symfony,
    /// RabbitMQ's CLI tools: `help` listings and `autocomplete -- --`.
    RabbitMQ,
    /// Node.js's own option list, from `node --completion-bash`.
    Node,
    /// The clang driver's `--autocomplete=<prefix>` (see [`clang`]).
    Clang,
    /// A function registered by the app's bash completion script (opt-in).
    BashFunction,
    /// An installed fish completion script, using `complete --do-complete`.
    FishScript,
    /// An installed zsh autoload function, queried inside a ZLE widget.
    ZshFunction,
    /// An argument completer registered in the user's PowerShell session,
    /// asked through `TabExpansion2` (opt-in).
    PowerShellCompleter,
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
    // Completion opens no repository and runs no password command; snapshot
    // IDs are not completed.
    ("github.com/restic/restic/cmd/restic", Flavor::Cobra),
    // Its own command line is cobra; urfave/cli/v3 arrives through a
    // dependency. Completion reads no source state: no templates, scripts,
    // or hooks run.
    ("chezmoi.io/chezmoi/v2", Flavor::Cobra),
    // Completion lists remote names from the local configuration and never
    // a remote's paths; decrypting an encrypted configuration would run
    // the user's password command, which offline probes don't receive.
    ("github.com/rclone/rclone", Flavor::Cobra),
    // Its command line is cobra; go-flags arrives through a dependency.
    // Outside a project its completion touches nothing; a devspace.yaml's
    // command variables run, so projects need trusted_workspaces.
    ("github.com/loft-sh/devspace", Flavor::Cobra),
    // Completes no model names; never contacts or starts a model server.
    ("github.com/ollama/ollama", Flavor::Cobra),
    // caddycmd's cobra tree completes only command and flag names (no
    // completion callbacks or pre-run hooks); urfave/cli arrives through
    // smallstep. Module init only registers modules and reads CADDY_ADMIN.
    ("github.com/caddyserver/caddy/v2/cmd/caddy", Flavor::Cobra),
    ("github.com/hashicorp/terraform", Flavor::Posener),
    ("github.com/hashicorp/packer", Flavor::Posener),
    ("github.com/opentofu/opentofu/cmd/tofu", Flavor::Posener),
    // Their predictors ask the server (mounts, policies, jobs, ACL objects)
    // and vault runs a configured token helper; the offline environment
    // points every address at a missing socket and disables the helper.
    ("github.com/hashicorp/vault", Flavor::Posener),
    ("github.com/openbao/openbao/v2", Flavor::Posener),
    ("github.com/hashicorp/nomad", Flavor::Posener),
    ("github.com/hashicorp/consul", Flavor::Posener),
    (
        "github.com/hashicorp/boundary/cmd/boundary",
        Flavor::Posener,
    ),
    // Archived at 0.11.4; its completion reads neither server nor project.
    (
        "github.com/hashicorp/waypoint/cmd/waypoint",
        Flavor::Posener,
    ),
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
    /// The invocation learned from a generated script, never a guessed variable.
    clap: Option<Box<clap::Protocol>>,
    cargo: Option<Box<cargo::Protocol>>,
    dotnet: Option<Box<dotnet::Protocol>>,
    urfave: Option<Box<urfave::Protocol>>,
    click: Option<Box<click::Protocol>>,
    kingpin: Option<Box<kingpin::Protocol>>,
    go_flags: Option<Box<go_flags::Protocol>>,
    /// State of the later bridges, boxed together so `Backend` stays small
    /// (Clippy limits `Discovery::Found`'s size on Windows targets).
    bridge: Option<Box<Bridge>>,
    memo: ProbeMemo,
    /// cobra contexts whose answer let the shell complete files too.
    cobra_files: Rc<RefCell<HashSet<Vec<String>>>>,
}

#[derive(Clone, Debug)]
enum Bridge {
    Yargs(yargs::Protocol),
    Oclif(oclif::Protocol),
    Symfony(symfony::Protocol),
    RabbitMQ(rabbitmq::Protocol),
    /// A completion function the parent bash session defined in memory:
    /// its registration and the definitions from the same file.
    BashMemory(String),
    /// A zsh session's handler: definitions from its source and its name.
    ZshMemory {
        definitions: String,
        function: String,
    },
    /// A fish session's `complete` lines and the user functions they call.
    FishMemory(String),
    /// A PowerShell session's argument completer and its script's functions.
    PowerShellMemory(powershell_completer::Registration),
}

impl PartialEq for Backend {
    fn eq(&self, other: &Self) -> bool {
        (self.flavor, &self.name, &self.completer) == (other.flavor, &other.name, &other.completer)
    }
}

impl Eq for Backend {}

impl Backend {
    pub fn new(flavor: Flavor, name: &str, completer: PathBuf) -> Backend {
        let cargo = (flavor == Flavor::Cargo).then(|| Box::new(cargo::Protocol::new(&completer)));
        let dotnet = (flavor == Flavor::Dotnet)
            .then(|| dotnet::Protocol::new(&completer).map(Box::new))
            .flatten();
        Backend {
            flavor,
            name: name.to_owned(),
            completer,
            identity: None,
            network: false,
            helper: None,
            application: None,
            clap: None,
            cargo,
            dotnet,
            urfave: None,
            click: None,
            kingpin: (flavor == Flavor::Kingpin).then(Box::default),
            go_flags: (flavor == Flavor::GoFlags).then(Box::default),
            bridge: None,
            trust: match flavor {
                Flavor::AwsCompleter | Flavor::Gcloud | Flavor::Azure | Flavor::Git => {
                    Trust::Bridge
                }
                _ => Trust::UserTrusted,
            },
            memo: ProbeMemo::default(),
            cobra_files: Rc::default(),
        }
    }

    fn yargs(&self) -> Option<&yargs::Protocol> {
        match self.bridge.as_deref() {
            Some(Bridge::Yargs(protocol)) => Some(protocol),
            _ => None,
        }
    }

    fn oclif(&self) -> Option<&oclif::Protocol> {
        match self.bridge.as_deref() {
            Some(Bridge::Oclif(protocol)) => Some(protocol),
            _ => None,
        }
    }

    fn symfony(&self) -> Option<&symfony::Protocol> {
        match self.bridge.as_deref() {
            Some(Bridge::Symfony(protocol)) => Some(protocol),
            _ => None,
        }
    }

    fn rabbitmq(&self) -> Option<&rabbitmq::Protocol> {
        match self.bridge.as_deref() {
            Some(Bridge::RabbitMQ(protocol)) => Some(protocol),
            _ => None,
        }
    }

    fn zsh_memory(&self) -> Option<(&str, &str)> {
        match self.bridge.as_deref() {
            Some(Bridge::ZshMemory {
                definitions,
                function,
            }) => Some((definitions, function)),
            _ => None,
        }
    }

    fn fish_memory(&self) -> Option<&str> {
        match self.bridge.as_deref() {
            Some(Bridge::FishMemory(script)) => Some(script),
            _ => None,
        }
    }

    fn bash_memory(&self) -> Option<&str> {
        match self.bridge.as_deref() {
            Some(Bridge::BashMemory(script)) => Some(script),
            _ => None,
        }
    }

    fn powershell_memory(&self) -> Option<&powershell_completer::Registration> {
        match self.bridge.as_deref() {
            Some(Bridge::PowerShellMemory(registration)) => Some(registration),
            _ => None,
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

    pub fn set_workspace(&mut self, trusted: &[String], cwd: Option<&Path>) {
        if let Some(protocol) = &mut self.go_flags {
            protocol.cwd = cwd.map(Path::to_owned);
        }
        if let Some(protocol) = &mut self.dotnet {
            protocol.trusted_workspaces = trusted.to_vec();
            protocol.cwd = cwd.map(Path::to_owned);
        }
    }

    /// What identifies this installation's answers in the disk cache:
    /// completer and app files, extension and component directories.
    /// `None` when answers depend on the working directory (git reads
    /// aliases from the repository's configuration).
    pub fn cache_identity(&self) -> Option<String> {
        // A session's own functions are not identified by any file.
        if self.bash_memory().is_some()
            || self.zsh_memory().is_some()
            || self.fish_memory().is_some()
            || self.powershell_memory().is_some()
        {
            return None;
        }
        // urfave/cli and click callbacks can read the working directory's
        // project (lefthook's hooks, flask's application).
        if matches!(
            self.flavor,
            Flavor::Git
                | Flavor::ClapDynamic
                | Flavor::Pip
                | Flavor::Npm
                | Flavor::Cargo
                | Flavor::Dotnet
                | Flavor::Urfave
                | Flavor::Click
                | Flavor::Kingpin
                | Flavor::GoFlags
                | Flavor::Yargs
                | Flavor::Oclif
                | Flavor::Symfony
                | Flavor::RabbitMQ
        ) {
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
            // The session's own registration is what its Tab runs; other
            // shells' functions come after it.
            let handlers: [HandlerDiscovery; 4] = match shell {
                crate::shells::Shell::Zsh => [
                    zsh_function,
                    bash_function,
                    fish_script,
                    powershell_completer,
                ],
                crate::shells::Shell::Fish => [
                    fish_script,
                    bash_function,
                    zsh_function,
                    powershell_completer,
                ],
                crate::shells::Shell::Powershell => [
                    powershell_completer,
                    bash_function,
                    fish_script,
                    zsh_function,
                ],
                _ => [
                    bash_function,
                    fish_script,
                    zsh_function,
                    powershell_completer,
                ],
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

/// Discovery that may read a trusted app's documented completion generator.
/// Metadata probes consume the same budget as completion queries. Existing
/// protocol discovery stays read-only and available to library callers.
pub fn discover_for_shell_with_budget(
    name: &str,
    path: &Path,
    trusted: &[String],
    trusted_help: &[String],
    shell: crate::shells::Shell,
    budget: &mut Budget,
) -> Discovery {
    // A compiler driver answering clang's protocol is clang under any name
    // (macOS's cc and gcc); GCC rejects the request.
    if clang::is_candidate(app_name(name))
        && probe::is_trusted_location(path)
        && clang::confirms(path, offline_env(Flavor::Clang), budget)
    {
        let mut backend = Backend::new(Flavor::Clang, app_name(name), path.to_owned());
        backend.trust = Trust::Bridge;
        return Discovery::Found(backend);
    }
    let mut found = discover_for_shell(name, path, trusted, shell);
    if let Discovery::Found(backend) = &mut found
        && let Some(protocol) = &mut backend.cargo
    {
        protocol.trusted = trusted.to_vec();
        protocol.trusted_help = trusted_help.to_vec();
        protocol.shell = shell;
        return found;
    }
    if let Discovery::Found(backend) = &mut found
        && let Some(protocol) = &mut backend.urfave
    {
        protocol.trusted_help = trusted_help.to_vec();
        return found;
    }
    if let Discovery::Found(backend) = &mut found
        && let Some(protocol) = &mut backend.kingpin
    {
        protocol.trusted_help = trusted_help.to_vec();
        let protocol = protocol.clone();
        if !protocol.allowed(backend) {
            return Discovery::NotTrusted(format!(
                "{} uses kingpin, whose completion runs the app's pre-actions and flag defaults; add it to trusted_help as well to use it",
                described(&backend.name, backend.identity.as_deref())
            ));
        }
        return found;
    }
    if let Discovery::Found(backend) = &mut found
        && let Some(protocol) = &mut backend.go_flags
    {
        protocol.trusted_help = trusted_help.to_vec();
        let protocol = protocol.clone();
        if !protocol.allowed(backend) {
            return Discovery::NotTrusted(format!(
                "{} uses go-flags, whose completion can run default marshalers, value callbacks, and code after a custom completion handler; add it to trusted_help as well to use it",
                described(&backend.name, backend.identity.as_deref())
            ));
        }
        return found;
    }
    if let Discovery::Found(backend) = &mut found
        && let Some(Bridge::RabbitMQ(protocol)) = backend.bridge.as_deref_mut()
    {
        protocol.trusted_help = trusted_help.to_vec();
        let protocol = protocol.clone();
        if !protocol.allowed(backend) {
            return Discovery::NotTrusted(format!(
                "{} is a RabbitMQ CLI tool, whose help starts the Erlang VM and loads plugins; add it to trusted_help as well to use it",
                described(&backend.name, backend.identity.as_deref())
            ));
        }
        return found;
    }
    if !matches!(
        &found,
        Discovery::None
            | Discovery::Found(Backend {
                flavor: Flavor::BashFunction
                    | Flavor::FishScript
                    | Flavor::ZshFunction
                    | Flavor::PowerShellCompleter,
                ..
            })
    ) {
        return found;
    }
    if !probe::is_trusted_location(path) {
        return found;
    }
    let app_identity = match &found {
        Discovery::Found(backend) => backend.identity.clone(),
        _ => identity::identify(path),
    };
    if !identity::is_trusted(trusted, name, app_identity.as_deref())
        && !identity::is_trusted(trusted, app_name(name), app_identity.as_deref())
    {
        return found;
    }
    // Trusting a shell handler doesn't authorize starting its application to
    // ask for help. Use the established help-probe permission separately.
    let helped = !runs_on_long_help(name, path)
        && trusted_help.iter().any(|trusted| {
            trusted == "*"
                || trusted == name
                || trusted == app_name(name)
                || Some(trusted) == app_identity.as_ref()
        });
    // Prefer a direct bridge for installed dynamic scripts; no shell needs to
    // load or execute their registration code.
    if let Discovery::Found(backend) = &found
        && let Some(script) = &backend.helper
        && let Some(text) = clap::read_script(script, budget.max_output)
    {
        if let Some(protocol) = clap::Protocol::from_script(&text, name, path) {
            return Discovery::Found(
                clap::backend(name, path, app_identity, protocol).with_helper(script.clone()),
            );
        }
        if let Some(mut protocol) = click::Protocol::from_script(&text, name, path) {
            // Root commands from trusted help are commands, not values that
            // might name resources (mkdocs's bash script).
            if helped {
                protocol.read_root_commands(path, budget);
            }
            return Discovery::Found(
                click::backend(name, path, app_identity, protocol).with_helper(script.clone()),
            );
        }
        if let Some(protocol) = yargs::Protocol::from_script(&text, name, path) {
            return Discovery::Found(
                yargs::backend(name, path, app_identity, protocol).with_helper(script.clone()),
            );
        }
        // Loading this file runs the app's generator; run it directly.
        if let Ok(Some(protocol)) = clap::shim_protocol(&text, name, path, budget) {
            return Discovery::Found(
                clap::backend(name, path, app_identity, protocol).with_helper(script.clone()),
            );
        }
    }
    // fish 4 embeds its completion scripts. For fish users they come before
    // another shell's handler, and otherwise when nothing else answers.
    let fish_user = shell == crate::shells::Shell::Fish;
    let embedded = |budget: &mut Budget, identity: Option<String>| {
        if !fish_user {
            return None;
        }
        fish::embedded(app_name(name), budget).map(|(fish, script)| {
            let mut backend =
                Backend::new(Flavor::FishScript, app_name(name), fish).with_helper(script);
            backend.identity = identity;
            Discovery::Found(backend.with_application(path.to_owned()))
        })
    };
    if fish_user
        && matches!(
            &found,
            Discovery::Found(backend)
                if matches!(backend.flavor, Flavor::BashFunction | Flavor::ZshFunction)
        )
        && let Some(found) = embedded(budget, app_identity.clone())
    {
        return found;
    }
    // A Python console script may be a click app; its help says so. Python
    // apps don't generate clap scripts.
    let generated = if !helped {
        Ok(None)
    } else if app_identity
        .as_deref()
        .is_some_and(|identity| identity.starts_with("python:"))
    {
        click::discover_from_help(path, app_identity.as_deref(), budget).map(|found| {
            found.map(|protocol| click::backend(name, path, app_identity.clone(), protocol))
        })
    } else if symfony::is_php_script(path) {
        // A PHP script may be a Symfony Console app; its help says so.
        symfony::discover_from_help(path, budget).map(|found| {
            found.map(|protocol| symfony::backend(name, path, app_identity.clone(), protocol))
        })
    } else if let Some(found) = app_identity
        .as_deref()
        .is_some_and(|identity| identity.starts_with("npm:"))
        .then(|| yargs::discover_from_help(path, budget))
        .and_then(Result::transpose)
    {
        // A Node bin depending on yargs whose help is yargs's.
        found.map(|protocol| Some(yargs::backend(name, path, app_identity.clone(), protocol)))
    } else {
        clap::discover_generated(name, path, budget).map(|found| {
            found.map(|protocol| clap::backend(name, path, app_identity.clone(), protocol))
        })
    };
    match generated {
        Ok(Some(backend)) => Discovery::Found(backend),
        Ok(None) if matches!(found, Discovery::None) => {
            embedded(budget, app_identity).unwrap_or(found)
        }
        Ok(None) => found,
        Err(error) => match found {
            Discovery::None => embedded(budget, app_identity)
                .unwrap_or_else(|| Discovery::Unavailable(format!("completion metadata: {error}"))),
            other => other,
        },
    }
}

/// Programs that don't reject `--help` but go on to run: Apache's httpd
/// starts the server, and apachectl hands words it doesn't know to httpd.
/// No discovery asks them `--help`; their help comes only from the short
/// flag their own man page documents.
pub(crate) fn runs_on_long_help(name: &str, path: &Path) -> bool {
    let apache = |name: &str| {
        matches!(
            app_name(name),
            "httpd" | "apache2" | "apachectl" | "apache2ctl"
        )
    };
    apache(name)
        || [Some(path.to_owned()), fs::canonicalize(path).ok()]
            .into_iter()
            .flatten()
            .any(|path| {
                path.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(apache)
            })
}

/// A literal PHP script, recognized without importing it.
pub(crate) fn is_php_file(path: &Path) -> bool {
    symfony::is_php_file(path)
}

pub(crate) fn is_php_interpreter_name(name: &str) -> bool {
    let name = name.rsplit(['/', '\\']).next().unwrap_or(name);
    name == "php"
        || name.strip_prefix("php").is_some_and(|version| {
            version.starts_with(|c: char| c.is_ascii_digit())
                && version.chars().all(|c| c.is_ascii_digit() || c == '.')
        })
}

/// `php artisan` and `php bin/console`: metadata and completion use the
/// same interpreter as the original line, never execute its operation.
pub(crate) fn discover_php_script(
    name: &str,
    script: &Path,
    interpreter: &Path,
    trusted: &[String],
    trusted_help: &[String],
    budget: &mut Budget,
) -> Discovery {
    if !probe::is_trusted_location(script) || !probe::is_trusted_location(interpreter) {
        return Discovery::Unavailable("PHP completion requires absolute resolved paths".into());
    }
    let permitted = |entries: &[String]| {
        identity::is_trusted(entries, name, None)
            || identity::is_trusted(entries, app_name(name), None)
    };
    if !permitted(trusted) || !permitted(trusted_help) {
        return Discovery::NotTrusted(format!(
            "PHP script {name} needs both trusted_completers and trusted_help to boot its completion application"
        ));
    }
    match symfony::discover_with_interpreter(script, Some(interpreter), budget) {
        Ok(Some(protocol)) => Discovery::Found(symfony::backend(name, script, None, protocol)),
        Ok(None) => Discovery::None,
        Err(error) => Discovery::Unavailable(error.to_string()),
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
    // The parent zsh session's own handler is what its Tab runs.
    if let Some((definitions, function)) = zsh::memory(name) {
        if !identity::is_trusted(trusted, name, identity) {
            return Discovery::NotTrusted(format!(
                "{} has a completion function in your zsh session; add it to trusted_completers to use it",
                described(name, identity)
            ));
        }
        if !cfg!(unix) {
            return Discovery::Unavailable("ZLE completion requires a Unix terminal".into());
        }
        return match crate::utils::which("zsh").filter(|p| probe::is_trusted_location(p)) {
            Some(zsh) => {
                let mut backend = Backend::new(Flavor::ZshFunction, name, zsh);
                backend.bridge = Some(Box::new(Bridge::ZshMemory {
                    definitions,
                    function,
                }));
                Discovery::Found(backend)
            }
            None => Discovery::Unavailable("zsh is not installed".into()),
        };
    }
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
    // The parent fish session's own completions are what its Tab offers.
    if let Some(script) = fish::memory_script(name) {
        if !identity::is_trusted(trusted, name, identity) {
            return Discovery::NotTrusted(format!(
                "{} has completions in your fish session; add it to trusted_completers to use them",
                described(name, identity)
            ));
        }
        return match crate::utils::which("fish").filter(|f| probe::is_trusted_location(f)) {
            Some(fish) => {
                let mut backend = Backend::new(Flavor::FishScript, name, fish);
                backend.bridge = Some(Box::new(Bridge::FishMemory(script)));
                Discovery::Found(backend)
            }
            None => Discovery::Unavailable("fish is not installed".into()),
        };
    }
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

/// The argument completer the parent PowerShell session registered for
/// `name`; PowerShell has no installed completion files to look for.
fn powershell_completer(name: &str, trusted: &[String], identity: Option<&str>) -> Discovery {
    let name = app_name(name);
    let Some(registration) = powershell_completer::memory(name) else {
        return Discovery::None;
    };
    if !identity::is_trusted(trusted, name, identity) {
        return Discovery::NotTrusted(format!(
            "{} has an argument completer in your PowerShell session; add it to trusted_completers to use it",
            described(name, identity)
        ));
    }
    match powershell::executable(crate::utils::which) {
        Some(shell) => {
            let mut backend = Backend::new(Flavor::PowerShellCompleter, name, shell);
            backend.bridge = Some(Box::new(Bridge::PowerShellMemory(registration)));
            Discovery::Found(backend)
        }
        None => Discovery::Unavailable("PowerShell is not installed".into()),
    }
}

/// A trusted program's bash completion function, when it has nothing else.
fn bash_function(name: &str, trusted: &[String], identity: Option<&str>) -> Discovery {
    let name = app_name(name);
    // The parent bash session's own registration is what its Tab runs.
    if let Some(script) = bash::memory_script(name) {
        if !identity::is_trusted(trusted, name, identity) {
            return Discovery::NotTrusted(format!(
                "{} has a completion function in your bash session; add it to trusted_completers to use it",
                described(name, identity)
            ));
        }
        return match crate::utils::which("bash").filter(|b| probe::is_trusted_location(b)) {
            Some(bash) => {
                let mut backend = Backend::new(Flavor::BashFunction, name, bash);
                backend.bridge = Some(Box::new(Bridge::BashMemory(script)));
                Discovery::Found(backend)
            }
            None => Discovery::Unavailable("bash is not installed".into()),
        };
    }
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
    // Console-script metadata establishes the entry module, including
    // versioned or renamed pip launchers. A basename alone proves nothing.
    if identity == Some("python:pip") {
        return if identity::is_trusted(trusted, name, identity) {
            found(Flavor::Pip, path.to_owned())
        } else {
            Discovery::NotTrusted(format!(
                "{} supports pip completion; add it to trusted_completers to use it",
                described(name, identity)
            ))
        };
    }
    // An oclif app's manifests describe it; reading them runs nothing.
    if identity.is_some_and(|identity| identity.starts_with("npm:"))
        && let Some(protocol) = oclif::discover(path)
    {
        let mut backend = Backend::new(Flavor::Oclif, name, path.to_owned());
        backend.trust = Trust::Bridge;
        backend.bridge = Some(Box::new(Bridge::Oclif(protocol)));
        return Discovery::Found(backend);
    }
    if identity == Some("npm:npm") {
        if identity::npm_entrypoint(path) != Some("npm") {
            return Discovery::Unavailable(
                "this npm package entry point does not declare npm completion".into(),
            );
        }
        // npm.cmd wrappers need a shell on Windows, and npm itself only
        // supports this protocol in Git Bash. Do not interpolate raw words.
        if cfg!(windows) {
            return Discovery::Unavailable(
                "native npm completion is unsupported on Windows".into(),
            );
        }
        return if identity::is_trusted(trusted, name, identity) {
            found(Flavor::Npm, path.to_owned())
        } else {
            Discovery::NotTrusted(format!(
                "{} supports npm completion; add it to trusted_completers to use it",
                described(name, identity)
            ))
        };
    }
    // RabbitMQ's installed scripts name the tool; reading them runs nothing.
    if let Some(tool) = identity.and_then(|identity| identity.strip_prefix("rabbitmq:")) {
        return if identity::is_trusted(trusted, name, identity) {
            Discovery::Found(rabbitmq::backend(name, path, tool.to_owned()))
        } else {
            Discovery::NotTrusted(format!(
                "{} supports RabbitMQ CLI completion; add it to trusted_completers to use it",
                described(name, identity)
            ))
        };
    }
    if identity == Some("rust:cargo") {
        return if identity::is_trusted(trusted, name, identity) {
            found(Flavor::Cargo, path.to_owned())
        } else {
            Discovery::NotTrusted(format!(
                "{} provides its installed command list; add it to trusted_completers to use it",
                described(name, identity)
            ))
        };
    }
    if identity == Some("dotnet:sdk") {
        return if identity::is_trusted(trusted, name, identity) {
            found(Flavor::Dotnet, path.to_owned())
        } else {
            Discovery::NotTrusted(format!(
                "{} supports SDK completion; add it to trusted_completers to use it",
                described(name, identity)
            ))
        };
    }
    match name {
        "git" => found(Flavor::Git, path.to_owned()),
        // The answer must be node's own completion script.
        "node" | "nodejs" => found(Flavor::Node, path.to_owned()),
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
            if flavor == Flavor::Urfave {
                backend.urfave = urfave::linked(&module).ok().flatten().map(Box::new);
            }
            if let Some(protocol) = &mut backend.go_flags {
                protocol.windows = cfg!(windows) && !module.force_posix;
                protocol.identify(path);
            }
            backend.identity = Some(module.path);
            Discovery::Found(backend)
        }
        Err(GoRefusal::NoProtocol) => Discovery::None,
        Err(GoRefusal::NotTrusted(why)) => Discovery::NotTrusted(why),
        Err(GoRefusal::Ambiguous(why)) => Discovery::Unavailable(why),
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
    /// It links libraries whose protocols differ, and no audit says which
    /// one parses its command line. A wrong guess could run the app.
    Ambiguous(String),
}

/// Builds whose command line is a library's, under a main module name that
/// doesn't identify one app: xcaddy names every Caddy build's main module
/// `caddy`, whatever plugins it adds (Homebrew's caddy is one). The library
/// settles the protocol; trust still comes from trusted_completers.
const GO_COMMAND_LIBRARIES: &[(&str, &str, Flavor)] =
    &[("caddy", "github.com/caddyserver/caddy/v2", Flavor::Cobra)];

fn go_flavor(module: &go::GoModule, name: &str, trusted: &[String]) -> Result<Flavor, GoRefusal> {
    let audited = AUDITED_GO_APPS
        .iter()
        .find(|(app, _)| *app == module.path)
        .map(|(_, flavor)| *flavor);
    let settled = audited.or_else(|| {
        GO_COMMAND_LIBRARIES
            .iter()
            .find(|(path, library, _)| module.path == *path && module.uses(library))
            .map(|(_, _, flavor)| *flavor)
    });
    let posener = module.uses("github.com/posener/complete");
    let cobra = module.uses("github.com/spf13/cobra");
    let kingpin = kingpin::linked(module);
    let go_flags = module.uses(go_flags::LIBRARY);
    // An app that doesn't speak one library's protocol runs normally when
    // given its request (cobra's `__complete`, posener's empty command line,
    // urfave/cli's or kingpin's flag). Only an audit settles which library
    // parses the command line when several are linked.
    let urfave = urfave::linked(module);
    if settled.is_none() {
        if let Err(why) = &urfave {
            return Err(GoRefusal::Ambiguous(format!(
                "{name} ({}) {why}",
                module.path
            )));
        }
        let linked: Vec<&str> = [
            (posener, "posener/complete"),
            (cobra, "cobra"),
            (matches!(urfave, Ok(Some(_))), "urfave/cli"),
            (kingpin, "kingpin"),
            (go_flags, "go-flags"),
        ]
        .into_iter()
        .filter_map(|(linked, library)| linked.then_some(library))
        .collect();
        // posener with cobra keeps its long-standing posener choice.
        if linked.len() > 1 && linked != ["posener/complete", "cobra"] {
            return Err(GoRefusal::Ambiguous(format!(
                "{name} ({}) links {}; notypo can't tell which one parses its command line",
                module.path,
                linked.join(" and ")
            )));
        }
    }
    let flavor = if let Some(flavor) = settled {
        flavor
    } else if posener {
        Flavor::Posener
    } else if cobra {
        Flavor::Cobra
    } else if matches!(urfave, Ok(Some(_))) {
        Flavor::Urfave
    } else if kingpin {
        Flavor::Kingpin
    } else if go_flags {
        Flavor::GoFlags
    } else {
        return Err(GoRefusal::NoProtocol);
    };
    if audited.is_none() && !identity::is_trusted(trusted, name, Some(&module.path)) {
        return Err(GoRefusal::NotTrusted(format!(
            "{name} ({}) supports {} completion; add it to trusted_completers to use it",
            module.path,
            match flavor {
                Flavor::Cobra => "cobra",
                Flavor::Urfave => "urfave/cli",
                Flavor::Kingpin => "kingpin",
                Flavor::GoFlags => "go-flags",
                _ => "posener",
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
pub(crate) fn offline_env(flavor: Flavor) -> Vec<(OsString, Option<OsString>)> {
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
    // Node CLIs: update-notifier checks the npm registry and writes its state
    // under HOME (docusaurus does even for --help).
    env.push(set("NO_UPDATE_NOTIFIER", "1"));
    // husky has no help: any run sets core.hooksPath and writes hooks,
    // `husky --help` into a `--help` directory, unless this is 0.
    env.push(set("HUSKY", "0"));
    // Neovim writes ~/.local/state/nvim/nvim.log even for --help.
    env.push(set(
        "NVIM_LOG_FILE",
        if cfg!(windows) { "NUL" } else { "/dev/null" },
    ));
    let null = if cfg!(windows) { "NUL" } else { "/dev/null" };
    // HashiCorp clients and OpenBao default to loopback servers, which Go
    // reaches without a proxy: vault's and nomad's predictors list mounts,
    // policies, and jobs there with the user's token, and vault first runs
    // a configured token helper. Agent addresses and client proxies take
    // precedence over the address; zero retries keep failures immediate.
    env.extend(
        [
            "VAULT_ADDR",
            "BAO_ADDR",
            "NOMAD_ADDR",
            "CONSUL_HTTP_ADDR",
            "BOUNDARY_ADDR",
        ]
        .map(|k| set(k, "unix:///nonexistent/notypo-offline.sock")),
    );
    env.extend([
        set("VAULT_CONFIG_PATH", null),
        set("BAO_CONFIG_PATH", null),
        set("BOUNDARY_KEYRING_TYPE", "none"),
    ]);
    env.extend(
        [
            "VAULT_AGENT_ADDR",
            "BAO_AGENT_ADDR",
            "VAULT_PROXY_ADDR",
            "BAO_PROXY_ADDR",
            "VAULT_HTTP_PROXY",
            "BAO_HTTP_PROXY",
            "VAULT_MAX_RETRIES",
            "BAO_MAX_RETRIES",
            "VAULT_TOKEN",
            "BAO_TOKEN",
            "NOMAD_TOKEN",
            "CONSUL_HTTP_TOKEN",
            "CONSUL_HTTP_TOKEN_FILE",
            "CONSUL_GRPC_ADDR",
            "BOUNDARY_TOKEN",
        ]
        .map(unset),
    );
    // Backup tools never get a repository password or a way to ask for one,
    // and borg reaches no remote repository.
    env.push(set("BORG_RSH", "false"));
    env.extend(
        [
            "BORG_PASSPHRASE",
            "BORG_PASSCOMMAND",
            "BORG_PASSPHRASE_FD",
            "BORG_NEW_PASSPHRASE",
            "RESTIC_PASSWORD",
            "RESTIC_PASSWORD_FILE",
            "RESTIC_PASSWORD_COMMAND",
            // rclone's completion decrypts an encrypted configuration to
            // list remotes, running the user's password command.
            "RCLONE_PASSWORD_COMMAND",
            "RCLONE_CONFIG_PASS",
        ]
        .map(unset),
    );
    env.push(set("RCLONE_ASK_PASSWORD", "false"));
    // Password managers stay locked: no 1Password session, service account,
    // Connect server, or biometric unlock through the desktop app, and no
    // Bitwarden session or API key.
    env.push(set("OP_BIOMETRIC_UNLOCK_ENABLED", "false"));
    env.extend(
        [
            "OP_SERVICE_ACCOUNT_TOKEN",
            "OP_CONNECT_HOST",
            "OP_CONNECT_TOKEN",
            "BW_SESSION",
            "BW_CLIENTID",
            "BW_CLIENTSECRET",
            "BW_PASSWORD",
        ]
        .map(unset),
    );
    env.extend(std::env::vars_os().filter_map(|(key, _)| {
        key.to_str()
            .is_some_and(|key| key.starts_with("OP_SESSION_"))
            .then_some((key, None))
    }));
    // AI assistants' API keys: help and completion need none.
    env.extend(
        [
            "OPENAI_API_KEY",
            "ANTHROPIC_API_KEY",
            "GEMINI_API_KEY",
            "GOOGLE_GENAI_API_KEY",
            "OPENROUTER_API_KEY",
            "MISTRAL_API_KEY",
            "GROQ_API_KEY",
            "DEEPSEEK_API_KEY",
            "OLLAMA_API_KEY",
        ]
        .map(unset),
    );
    // Teleport's tsh checks for managed client-tool updates (and locks
    // ~/.tsh/bin) even to print help.
    env.push(set("TELEPORT_TOOLS_VERSION", "off"));
    // garden reports usage and checks for releases on every command.
    env.extend([
        set("GARDEN_DISABLE_ANALYTICS", "true"),
        set("GARDEN_DISABLE_VERSION_CHECK", "true"),
    ]);
    // gh 2.102 records a device id (and reports usage) on every command;
    // DO_NOT_TRACK is the shared convention other tools honor too.
    env.extend([set("GH_TELEMETRY", "0"), set("DO_NOT_TRACK", "1")]);
    // Block's goose writes a log file under its state root on every run,
    // even for --help; with its root on the null device it only warns.
    // Its keyring holds provider keys and is never opened.
    env.extend([
        set("GOOSE_PATH_ROOT", null),
        set("GOOSE_DISABLE_KEYRING", "1"),
    ]);
    // Node's fetch honors the blocked proxy only when asked to (snyk's
    // wrapper, bw, and other Node programs run through handlers and help).
    env.push(set("NODE_USE_ENV_PROXY", "1"));
    match flavor {
        Flavor::Dotnet => {
            env.extend([
                set("DOTNET_CLI_TELEMETRY_OPTOUT", "1"),
                set("DOTNET_NOLOGO", "1"),
                set("DOTNET_SKIP_FIRST_TIME_EXPERIENCE", "1"),
                set("DOTNET_GENERATE_ASPNET_CERTIFICATE", "false"),
                set("DOTNET_ADD_GLOBAL_TOOLS_TO_PATH", "false"),
                set("DOTNET_CLI_WORKLOAD_UPDATE_NOTIFY_DISABLE", "true"),
                set("DOTNET_EnableDiagnostics", "0"),
                set("COMPlus_EnableDiagnostics", "0"),
                set("CORECLR_ENABLE_PROFILING", "0"),
                set("COR_ENABLE_PROFILING", "0"),
                unset("DOTNET_STARTUP_HOOKS"),
            ]);
            env.extend(
                [
                    "DOTNET_HOST_TRACE",
                    "DOTNET_HOST_TRACEFILE",
                    "DOTNET_HOST_TRACE_VERBOSITY",
                    "COREHOST_TRACE",
                    "COREHOST_TRACEFILE",
                    "COREHOST_TRACE_VERBOSITY",
                ]
                .map(unset),
            );
        }
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
            set("CLOUDSDK_CORE_CHECK_GCE_METADATA", "false"),
            set("CLOUDSDK_AUTH_DISABLE_CREDENTIALS", "true"),
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
        Flavor::Cobra | Flavor::ClapDynamic => env.extend([
            // Cluster, daemon, and API lookups (resource names) stay off.
            set("KUBECONFIG", null),
            set("DOCKER_HOST", "unix:///nonexistent/notypo-offline.sock"),
            set("GH_PROMPT_DISABLED", "1"),
        ]),
        Flavor::Click => env.extend([
            set("PYTHONDONTWRITEBYTECODE", "1"),
            set("NO_COLOR", "1"),
            set("TERM", "dumb"),
            unset("PYTHONINSPECT"),
            unset("PYTHONSTARTUP"),
        ]),
        Flavor::Node => env.extend([unset("NODE_OPTIONS"), unset("NODE_REPL_EXTERNAL_MODULE")]),
        // The driver prepends or rewrites arguments from this variable.
        Flavor::Clang => env.push(unset("CCC_OVERRIDE_OPTIONS")),
        Flavor::Yargs => env.extend([
            unset("NODE_OPTIONS"),
            unset("NODE_REPL_EXTERNAL_MODULE"),
            set("NO_COLOR", "1"),
            set("FORCE_COLOR", "0"),
            set("KUBECONFIG", null),
            set("DOCKER_HOST", "unix:///nonexistent/notypo-offline.sock"),
            set("GIT_TERMINAL_PROMPT", "0"),
        ]),
        Flavor::Kingpin | Flavor::GoFlags => env.extend([
            set("KUBECONFIG", null),
            set("DOCKER_HOST", "unix:///nonexistent/notypo-offline.sock"),
            set("GIT_TERMINAL_PROMPT", "0"),
        ]),
        Flavor::Urfave => env.extend([
            set("KUBECONFIG", null),
            set("DOCKER_HOST", "unix:///nonexistent/notypo-offline.sock"),
            set("GIT_TERMINAL_PROMPT", "0"),
            // Either variable switches the answer to `name:usage` lines,
            // whose words are ambiguous when a name contains a colon.
            unset("0"),
            unset("_CLI_ZSH_AUTOCOMPLETE_HACK"),
        ]),
        Flavor::Pip => env.extend([
            set("PIP_CONFIG_FILE", null),
            set("PIP_NO_INDEX", "1"),
            set("PIP_DISABLE_PIP_VERSION_CHECK", "1"),
            set("PIP_NO_INPUT", "1"),
        ]),
        Flavor::Npm => {
            env.extend([
                set("npm_config_offline", "true"),
                set("npm_config_ignore_scripts", "true"),
                set("npm_config_update_notifier", "false"),
                set("npm_config_audit", "false"),
                set("npm_config_fund", "false"),
                set("npm_config_fetch_retries", "0"),
                set("npm_config_logs_max", "0"),
                set("npm_config_timing", "false"),
                set("npm_config_loglevel", "silent"),
                set("npm_config_color", "false"),
                unset("COMP_FISH"),
            ]);
            // npm creates its cache directory during startup, even when
            // logging and network requests are disabled. Keep it private.
            env.push((
                "npm_config_cache".into(),
                Some(crate::utils::cache_dir().join("npm").into()),
            ));
            // npm rejects loading the same null file as both config layers.
            // Missing, distinct paths disable each user's config separately.
            for (key, file) in [
                ("npm_config_userconfig", "disabled-user.npmrc"),
                ("npm_config_globalconfig", "disabled-global.npmrc"),
            ] {
                env.push((
                    key.into(),
                    Some(crate::utils::cache_dir().join("npm").join(file).into()),
                ));
            }
            // npm's config names ignore case. Remove inherited duplicates
            // rather than depend on their environment iteration order.
            let protected: Vec<_> = env
                .iter()
                .filter_map(|(key, _)| key.to_str())
                .filter(|key| key.starts_with("npm_config_"))
                .map(str::to_owned)
                .collect();
            env.extend(std::env::vars_os().filter_map(|(key, _)| {
                key.to_str().and_then(|text| {
                    let prefix = text.get(.."npm_config_".len())?;
                    if !prefix.eq_ignore_ascii_case("npm_config_") {
                        return None;
                    }
                    // npm also treats interior underscores as hyphens.
                    let canonical = format!(
                        "npm_config_{}",
                        text["npm_config_".len()..]
                            .replace('-', "_")
                            .to_ascii_lowercase()
                    );
                    protected
                        .iter()
                        .any(|name| text != name && canonical == *name)
                        .then(|| (key.clone(), None))
                })
            }));
        }
        Flavor::Cargo => env.extend([
            set("RUSTUP_AUTO_INSTALL", "0"),
            set("RUSTUP_NO_UPDATE_CHECK", "1"),
            set("CARGO_NET_OFFLINE", "true"),
            set("CARGO_HTTP_PROXY", BLOCKED_PROXY),
            set("CARGO_TERM_COLOR", "never"),
            set("CARGO_TERM_PROGRESS_WHEN", "never"),
            unset("CARGO_COMPLETE"),
            unset("CARGO_LOG"),
            unset("CARGO_LOG_PROFILE"),
            unset("CARGO_LOG_PROFILE_CAPTURE_ARGS"),
        ]),
        // Handwritten handlers run whatever their app does at startup. The
        // Cloud SDK's wrappers (bq, gsutil) load credentials, probing the GCE
        // metadata server without a proxy, and check for updates unless told
        // not to.
        Flavor::BashFunction
        | Flavor::FishScript
        | Flavor::ZshFunction
        | Flavor::PowerShellCompleter => env.extend([
            set("KUBECONFIG", null),
            set("DOCKER_HOST", "unix:///nonexistent/notypo-offline.sock"),
            set("GH_PROMPT_DISABLED", "1"),
            set("CLOUDSDK_CORE_DISABLE_PROMPTS", "1"),
            set("CLOUDSDK_CORE_CHECK_GCE_METADATA", "false"),
            set("CLOUDSDK_COMPONENT_MANAGER_DISABLE_UPDATE_CHECK", "true"),
            set("CLOUDSDK_AUTH_DISABLE_CREDENTIALS", "true"),
        ]),
        // Composer stays offline and quiet; no completion debug log.
        Flavor::Symfony => env.extend([
            set("COMPOSER_DISABLE_NETWORK", "1"),
            set("COMPOSER_NO_INTERACTION", "1"),
            set("COMPOSER_NO_AUDIT", "1"),
            set("SHELL_VERBOSITY", "0"),
            set("NO_COLOR", "1"),
            unset("SYMFONY_COMPLETION_DEBUG"),
            set("KUBECONFIG", null),
            set("DOCKER_HOST", "unix:///nonexistent/notypo-offline.sock"),
        ]),
        // Crash dumps stay unwritten; user VM flags could start
        // distribution or evaluate code before the CLI parses its words.
        Flavor::RabbitMQ => env.extend([
            set("ERL_CRASH_DUMP_BYTES", "0"),
            unset("ERL_AFLAGS"),
            unset("ERL_ZFLAGS"),
        ]),
        // oclif manifests are read, never run.
        Flavor::Argcomplete | Flavor::Git | Flavor::Posener | Flavor::Oclif => {}
    }
    env
}

/// The environment for `--help` and man probes of trusted programs: their
/// startup code gets what handwritten completion handlers get (no proxy,
/// cluster, daemon, server address, or repository password), and no pager.
/// dig and its relatives take query options after `+` (`+short`,
/// `+timeout=5`); zsh's _dig lists them only for a `+` prefix.
fn plus_option(program: &str, word: &str) -> bool {
    matches!(
        Path::new(program).file_name().and_then(|n| n.to_str()),
        Some("dig" | "delv" | "mdig" | "kdig")
    ) && word.len() > 1
        && word.starts_with('+')
        && word[1..].starts_with(|c: char| c.is_ascii_alphabetic())
}

pub(crate) fn help_env() -> Vec<(OsString, Option<OsString>)> {
    let mut env = offline_env(Flavor::BashFunction);
    env.push(("PAGER".into(), Some("cat".into())));
    env
}

/// Apps whose own completion handlers run the app on its data stores or
/// devices, by the name the handlers call. borg's bash, zsh, and fish
/// handlers list archives with `borg list` (and keys with `borg config`),
/// which locks the repository, can run BORG_PASSCOMMAND, and reaches remote
/// repositories over ssh. zsh's _adb lists device serials with `adb devices
/// -l`, which starts adb's server (listening on port 5037) and writes its
/// RSA key pair to ~/.android. Their handlers' fixed words are all notypo
/// uses.
const CLOSED_APPS: &[(&str, &str)] = &[("borg", "python:borg"), ("adb", "")];

/// Inside such an app's handler, the app is a private stub that fails: it
/// shadows the real program on PATH for every call the handler makes (zsh's
/// `_borg` calls the command word, so the typed name is stubbed too).
fn close_data_stores(
    env: &mut Vec<(OsString, Option<OsString>)>,
    name: &str,
    identity: Option<&str>,
) -> Result<(), CompletionError> {
    let Some((program, _)) = CLOSED_APPS
        .iter()
        .find(|(program, id)| *program == name || identity == Some(*id))
    else {
        return Ok(());
    };
    let refuse = |why: String| {
        CompletionError::Unsupported(format!(
            "{program}'s handler would run {program} on its data or devices, and {why}"
        ))
    };
    if !cfg!(unix) {
        return Err(refuse("no stub can replace it here".into()));
    }
    stub_programs(env, program, &[program, name]).map_err(refuse)
}

/// Go modules whose package initialization starts a helper on every run,
/// even for `--help` or a completion request; probes replace the helper
/// with a failing stub. On macOS, mkcert looks for NSS with `brew --prefix
/// nss`, and hugo links mkcert as bep/mclib for `server --tlsAuto`.
/// Homebrew then writes its caches and can outlast the probe.
const STARTUP_HELPERS: &[(&str, &str)] = &[
    ("filippo.io/mkcert", "brew"),
    ("github.com/bep/mclib", "brew"),
];

thread_local! {
    static STARTUP_MEMO: RefCell<HashMap<PathBuf, Vec<&'static str>>> =
        RefCell::new(HashMap::new());
}

/// The PATH a probe of `program` needs when the app starts helpers at
/// startup: their stubs first, then `env`'s or the inherited PATH.
pub(crate) fn startup_stubs(
    program: &Path,
    env: &[(OsString, Option<OsString>)],
) -> Option<OsString> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let real = fs::canonicalize(program).ok()?;
    let helpers = STARTUP_MEMO.with(|memo| {
        memo.borrow_mut()
            .entry(real.clone())
            .or_insert_with(|| {
                go::module(&real).map_or_else(Vec::new, |module| {
                    STARTUP_HELPERS
                        .iter()
                        .filter(|(library, _)| module.path == *library || module.uses(library))
                        .map(|(_, helper)| *helper)
                        .collect()
                })
            })
            .clone()
    });
    if helpers.is_empty() {
        return None;
    }
    let mut env = env.to_vec();
    // Without a stub, a PATH that names no directory keeps the helper from
    // being found.
    Some(match stub_programs(&mut env, "startup", &helpers) {
        Ok(()) => env
            .into_iter()
            .rev()
            .find(|(name, _)| name == "PATH")
            .and_then(|(_, value)| value)?,
        Err(_) => "/nonexistent".into(),
    })
}

/// Puts failing stubs named `stubs` first on the probe's PATH.
fn stub_programs(
    env: &mut Vec<(OsString, Option<OsString>)>,
    dir: &str,
    stubs: &[&str],
) -> Result<(), String> {
    let dir = crate::utils::cache_dir().join("stubs").join(dir);
    let write = || -> std::io::Result<()> {
        fs::create_dir_all(&dir)?;
        for stub in stubs {
            let path = dir.join(stub);
            fs::write(&path, "#!/bin/sh\nexit 1\n")?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&path, fs::Permissions::from_mode(0o755))?;
            }
        }
        Ok(())
    };
    write().map_err(|error| format!("its stub could not be written: {error}"))?;
    let inherited = env
        .iter()
        .rev()
        .find(|(name, _)| name == "PATH")
        .and_then(|(_, value)| value.clone())
        .or_else(|| std::env::var_os("PATH"))
        .unwrap_or_default();
    let path = std::env::join_paths(std::iter::once(dir).chain(std::env::split_paths(&inherited)))
        .map_err(|error| error.to_string())?;
    env.push(("PATH".into(), Some(path)));
    Ok(())
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
            Flavor::ClapDynamic => "clap",
            Flavor::Pip => "pip",
            Flavor::Npm => "npm",
            Flavor::Cargo => "cargo",
            Flavor::Dotnet => "dotnet",
            Flavor::Urfave => "urfave",
            Flavor::Click => "click",
            Flavor::Kingpin => "kingpin",
            Flavor::GoFlags => "go-flags",
            Flavor::Yargs => "yargs",
            Flavor::Oclif => "oclif",
            Flavor::Symfony => "symfony",
            Flavor::RabbitMQ => "rabbitmq",
            Flavor::Node => "node",
            Flavor::Clang => "clang",
            Flavor::BashFunction => "bash",
            Flavor::FishScript => "fish",
            Flavor::ZshFunction => "zsh",
            Flavor::PowerShellCompleter => "powershell-completer",
        }
    }

    fn location(&self) -> String {
        if self.bash_memory().is_some() {
            return format!(
                "your bash session's completion function (via {})",
                self.completer.display()
            );
        }
        if self.fish_memory().is_some() {
            return format!(
                "your fish session's completions (via {})",
                self.completer.display()
            );
        }
        if let Some((_, function)) = self.zsh_memory() {
            return format!(
                "your zsh session's {function} (via {})",
                self.completer.display()
            );
        }
        if self.powershell_memory().is_some() {
            return format!(
                "your PowerShell session's argument completer (via {})",
                self.completer.display()
            );
        }
        self.helper.as_ref().map_or_else(
            || self.completer.display().to_string(),
            |script| format!("{} (via {})", script.display(), self.completer.display()),
        )
    }

    fn cache_identity(&self) -> Option<String> {
        Backend::cache_identity(self)
    }

    fn capabilities(&self) -> Capabilities {
        if let Some(protocol) = &self.dotnet {
            return protocol.capabilities(self.trust);
        }
        if let Some(protocol) = &self.cargo {
            return protocol.capabilities(&self.name);
        }
        if let Some(protocol) = &self.urfave {
            return protocol.capabilities(self.trust);
        }
        if let Some(protocol) = &self.click {
            return protocol.capabilities(self.trust);
        }
        if let Some(protocol) = &self.kingpin {
            return protocol.capabilities(self.trust);
        }
        if let Some(protocol) = &self.go_flags {
            return protocol.capabilities(self.trust);
        }
        if let Some(protocol) = self.yargs() {
            return protocol.capabilities(self.trust);
        }
        if let Some(protocol) = self.oclif() {
            return protocol.capabilities();
        }
        if let Some(protocol) = self.symfony() {
            return protocol.capabilities(self.trust);
        }
        if let Some(protocol) = self.rabbitmq() {
            return protocol.capabilities(self.trust);
        }
        if self.flavor == Flavor::Clang {
            // Every driver option is listed, warnings included; other words
            // are input files.
            return Capabilities {
                subcommands: false,
                options: true,
                option_arity: false,
                values: true,
                resources: false,
                option_prefix: "-",
                short_options: true,
                complete_options: true,
                complete_subcommands: true,
                descriptions: true,
                query_dialect: None,
                trust: self.trust,
            };
        }
        Capabilities {
            subcommands: true,
            options: true,
            option_arity: matches!(
                self.flavor,
                Flavor::Gcloud | Flavor::Git | Flavor::FishScript | Flavor::Node
            ),
            // pip's empty-prefix answers can be options or nested command
            // names while an enum argument is pending, not its valid choices.
            values: !matches!(self.flavor, Flavor::Git | Flavor::Pip | Flavor::Npm),
            resources: self.network
                && !matches!(self.flavor, Flavor::Git | Flavor::Pip | Flavor::Npm),
            // gcloud's static tree only matches `--`; argparse apps list
            // every option (short ones too) for `-`; git lists long ones.
            option_prefix: match self.flavor {
                Flavor::AwsCompleter | Flavor::Gcloud | Flavor::Npm => "--",
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
                    | Flavor::PowerShellCompleter
                    | Flavor::ClapDynamic
                    | Flavor::Pip
                    | Flavor::Node
            ),
            // git's helper and hand-written completion scripts omit options;
            // node's list omits V8 flags it passes through.
            complete_options: !matches!(
                self.flavor,
                Flavor::Git
                    | Flavor::BashFunction
                    | Flavor::FishScript
                    | Flavor::ZshFunction
                    | Flavor::PowerShellCompleter
                    | Flavor::Pip
                    | Flavor::Npm
                    | Flavor::Node
            ),
            complete_subcommands: !matches!(
                self.flavor,
                Flavor::BashFunction
                    | Flavor::FishScript
                    | Flavor::ZshFunction
                    | Flavor::PowerShellCompleter
                    | Flavor::Pip
                    | Flavor::Npm
            ),
            descriptions: matches!(
                self.flavor,
                Flavor::Cobra | Flavor::FishScript | Flavor::PowerShellCompleter
            ) || self
                .clap
                .as_deref()
                .is_some_and(clap::Protocol::descriptions),
            query_dialect: match self.flavor {
                Flavor::Cobra
                | Flavor::Git
                | Flavor::ClapDynamic
                | Flavor::Pip
                | Flavor::Npm
                | Flavor::Node => None,
                Flavor::Cargo => None,
                Flavor::FishScript => Some(super::parser::Dialect::Fish),
                Flavor::PowerShellCompleter => Some(super::parser::Dialect::PowerShell),
                _ => Some(super::parser::Dialect::Posix),
            },
            trust: self.trust,
        }
    }

    fn option_separator(&self, typed: &str) -> Option<char> {
        if let Some(protocol) = &self.go_flags {
            return protocol.option_separator(typed);
        }
        if plus_option(&self.name, typed) {
            return Some('=');
        }
        self.dotnet.as_ref().and_then(|protocol| {
            protocol
                .option_syntax(typed)
                .map(|(_, separator)| separator)
        })
    }

    fn is_short_option(&self, typed: &str) -> bool {
        if plus_option(&self.name, typed) {
            return false;
        }
        self.go_flags.as_ref().map_or_else(
            || typed.len() > 1 && !typed.starts_with("--"),
            |protocol| protocol.is_short_option(typed),
        )
    }

    fn option_name_len(&self, typed: &str) -> Option<usize> {
        self.dotnet
            .as_ref()?
            .option_syntax(typed)
            .map(|(length, _)| length)
    }

    fn resolve_option(
        &self,
        words: &[&str],
        typed: &str,
        _budget: &mut Budget,
    ) -> Option<CompletionItem> {
        self.dotnet.as_ref()?.resolve_option(words, typed)
    }

    fn option_prefix(&self, typed: &str) -> &'static str {
        if let Some(protocol) = &self.go_flags {
            return protocol.option_prefix(typed);
        }
        if plus_option(&self.name, typed) {
            "+"
        } else if self.flavor == Flavor::Dotnet && typed.starts_with('/') {
            "/"
        } else {
            self.capabilities().option_prefix
        }
    }

    fn is_option(&self, words: &[&str], item: &CompletionItem) -> bool {
        item.is_option()
            || plus_option(&self.name, &item.value)
            || self
                .go_flags
                .as_ref()
                .is_some_and(|protocol| protocol.is_option(&item.value))
            || self
                .dotnet
                .as_ref()
                .is_some_and(|protocol| protocol.is_option(words, &item.value))
    }

    fn option_requires_value(
        &self,
        words: &[&str],
        typed: &str,
        next: Option<&str>,
        budget: &mut Budget,
    ) -> Option<bool> {
        // dig's `+name=value` options attach their values.
        if plus_option(&self.name, typed) {
            return Some(false);
        }
        // Arity only matters when an argument follows the option.
        let argument_follows = next.is_some_and(|next| !next.starts_with('-'));
        if self.flavor == Flavor::Cobra {
            return (argument_follows && self.cobra_flag_is_boolean(words, typed, budget))
                .then_some(false);
        }
        if matches!(
            self.flavor,
            Flavor::BashFunction | Flavor::ZshFunction | Flavor::FishScript
        ) {
            return (argument_follows && self.handler_flag_is_boolean(words, typed, budget))
                .then_some(false);
        }
        self.dotnet.as_ref()?.requires_value(words, typed, next)
    }

    fn capabilities_for(&self, words: &[&str]) -> Capabilities {
        if let Some(protocol) = &self.cargo {
            let mut capabilities = protocol.capabilities(&self.name);
            capabilities.complete_subcommands = cargo::root_context(words);
            return capabilities;
        }
        if let Some(protocol) = &self.urfave {
            return protocol.capabilities_for(words, self.trust);
        }
        if let Some(protocol) = &self.click {
            return protocol.capabilities_for(words, self.trust);
        }
        if let Some(protocol) = self.oclif() {
            return protocol.capabilities_for(words);
        }
        if let Some(protocol) = self.symfony() {
            return protocol.capabilities_for(words, self.trust);
        }
        if let Some(protocol) = self.rabbitmq() {
            return protocol.capabilities_for(words, self.trust);
        }
        let mut capabilities = self.capabilities();
        if matches!(self.flavor, Flavor::Pip | Flavor::Npm) && words.is_empty() {
            capabilities.complete_subcommands = true;
        }
        // A cobra root lists its commands (rclone's says files may follow,
        // but its commands are all there is); deeper, a level where files
        // are also valid is partial.
        if self.flavor == Flavor::Cobra
            && !words.is_empty()
            && self
                .cobra_files
                .borrow()
                .iter()
                .any(|context| context.iter().map(String::as_str).eq(words.iter().copied()))
        {
            capabilities.complete_subcommands = false;
        }
        capabilities
    }

    fn candidates_are_resources(&self, words: &[&str]) -> bool {
        if let Some(protocol) = &self.dotnet {
            return protocol.resources(words);
        }
        match self.flavor {
            Flavor::Pip => pip::resources(self, words),
            Flavor::Npm => npm::resources(self, words),
            _ => false,
        }
    }

    fn candidate_is_resource(&self, words: &[&str], item: &CompletionItem) -> bool {
        if let Some(protocol) = &self.click {
            return protocol.resource(words, item);
        }
        if self.flavor == Flavor::GoFlags {
            // This wire format cannot distinguish command names from
            // callback-provided filenames or other local resources.
            return !self.is_option(words, item);
        }
        if let Some(protocol) = self.yargs() {
            return protocol.resource(self, words, item);
        }
        if let Some(protocol) = self.oclif() {
            return protocol.resource(words, item);
        }
        if let Some(protocol) = self.symfony() {
            return protocol.resource(words, item);
        }
        if let Some(protocol) = &self.dotnet {
            return !self.is_option(words, item) && protocol.resources(words);
        }
        if self.flavor == Flavor::Cargo {
            return item.value.starts_with('+') || cargo::resource_value(words);
        }
        if let Some(protocol) = &self.urfave {
            return !item.is_option() && protocol.resources(self, words);
        }
        self.candidates_are_resources(words)
    }

    fn value_syntax_contains_slashes(&self, words: &[&str]) -> bool {
        matches!(
            self.flavor,
            Flavor::GoFlags | Flavor::Symfony | Flavor::Click
        ) || self
            .cargo
            .as_ref()
            .is_some_and(|protocol| protocol.feature_value(words))
    }

    fn prepare_value_candidates(
        &self,
        words: &[&str],
        typed: &str,
        items: Vec<CompletionItem>,
    ) -> Result<Vec<CompletionItem>, CompletionError> {
        if let Some(protocol) = &self.dotnet {
            return Ok(protocol.prepare_values(words, typed, items));
        }
        if self
            .cargo
            .as_ref()
            .is_some_and(|protocol| protocol.feature_value(words))
        {
            return cargo::feature_items(items, typed);
        }
        Ok(items)
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
        if let Some(protocol) = &self.cargo {
            return protocol.values(self, words, budget);
        }
        if let Some(protocol) = &self.click {
            return protocol.values(self, words, budget);
        }
        if let Some(protocol) = self.yargs() {
            return protocol.values(self, words, budget);
        }
        if let Some(protocol) = self.oclif() {
            return protocol.values(words);
        }
        if let Some(protocol) = self.symfony() {
            return protocol.values(self, words, budget);
        }
        if matches!(
            self.flavor,
            Flavor::Pip | Flavor::Npm | Flavor::Urfave | Flavor::Kingpin | Flavor::RabbitMQ
        ) {
            return Err(CompletionError::Unsupported(format!(
                "{} does not enumerate finite option-value choices",
                self.id()
            )));
        }
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
        if matches!(
            self.flavor,
            Flavor::Pip
                | Flavor::Npm
                | Flavor::Cargo
                | Flavor::Dotnet
                | Flavor::Urfave
                | Flavor::Click
                | Flavor::Kingpin
                | Flavor::Yargs
                | Flavor::Oclif
                | Flavor::Symfony
                | Flavor::RabbitMQ
        ) {
            return self.complete_values(words, budget);
        }
        self.query(words, "", budget, offline_env(self.flavor))
    }

    fn arguments_end_options(&self) -> bool {
        self.flavor == Flavor::Node
    }

    fn lists_omit_aliases(&self) -> bool {
        self.flavor == Flavor::Cobra
    }

    fn confirms(&self, words: &[&str], typed: &str, budget: &mut Budget) -> Option<bool> {
        if self.flavor == Flavor::Cobra {
            return self.cobra_confirms(words, typed, budget);
        }
        if let Some(protocol) = self.yargs() {
            return protocol.confirms(self, words, typed, budget);
        }
        if let Some(protocol) = self.symfony() {
            return protocol.confirms(self, words, typed, budget);
        }
        if let Some(protocol) = &self.go_flags {
            return protocol.confirms(self, words, typed, budget);
        }
        if let Some(protocol) = &self.dotnet {
            return protocol.confirms(words, typed);
        }
        if let Some(protocol) = &self.kingpin {
            return protocol.confirms(self, words, typed, budget);
        }
        self.urfave.as_ref()?.confirms(self, words, typed, budget)
    }

    /// posener/complete answers a command's subcommands, its argument
    /// predictions (vault's fixed engine names, terraform's workspaces), and
    /// its flags together. A listed subcommand selects another command,
    /// which answers differently than the same command does after `typed`;
    /// a listed argument leaves the command at the same position answering
    /// as it does after any other argument (terraform then predicts
    /// directories either way). Shell handlers mix them the same way: mtr's
    /// offers the placeholders `ip_address hostname` wherever a host goes.
    fn lists_arguments(
        &self,
        words: &[&str],
        typed: &str,
        listed: &[&str],
        budget: &mut Budget,
    ) -> Option<ArgumentList> {
        if !matches!(
            self.flavor,
            Flavor::Posener | Flavor::BashFunction | Flavor::ZshFunction | Flavor::FishScript
        ) {
            return None;
        }
        let word = *listed.iter().find(|word| !word.starts_with('-'))?;
        let mut after = |next: &str| -> Option<Vec<String>> {
            let words: Vec<&str> = words.iter().copied().chain([next]).collect();
            let mut names: Vec<String> = self
                .complete(&words, "", budget)
                .ok()?
                .into_iter()
                .map(|item| item.value)
                .filter(|value| !value.starts_with('-'))
                .collect();
            names.sort();
            Some(names)
        };
        let deeper = after(word)?;
        // The same words again after one of them: values or placeholders.
        let mut level: Vec<String> = listed
            .iter()
            .filter(|word| !word.starts_with('-'))
            .map(|word| word.to_string())
            .collect();
        level.sort();
        let posener = self.flavor == Flavor::Posener;
        if !deeper.is_empty() && deeper == level {
            return Some(if posener {
                ArgumentList::Unjudged
            } else {
                ArgumentList::Resources
            });
        }
        // A handler's subcommand and an unknown word can both complete
        // files; only posener's answers are compared that way.
        (posener && deeper == after(typed)?).then_some(ArgumentList::Unjudged)
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
        if self.flavor == Flavor::Clang {
            return clang::complete(&self.completer, words, prefix, env, budget);
        }
        if self.flavor == Flavor::Node {
            // Every other word is a file: the script and its arguments.
            if !prefix.starts_with('-') {
                return Ok(Vec::new());
            }
            let text = self.memoized(&["--completion-bash"], || {
                node::print(&self.completer, "--completion-bash", env.clone(), budget)
            })?;
            let mut items =
                node::parse(&text, budget.max_candidates).map_err(CompletionError::Failed)?;
            // Without help, arity stays unknown rather than failing the list.
            let help = self
                .memoized(&["--help"], || {
                    node::print(&self.completer, "--help", env, budget)
                })
                .unwrap_or_default();
            let arity = node::arity(&help);
            for item in &mut items {
                if let Some(takes) = arity.get(&item.value) {
                    item.takes_value = *takes;
                }
            }
            return Ok(items);
        }
        if self.flavor == Flavor::Cobra {
            return self.complete_cobra(words, prefix, budget, env);
        }
        if self.flavor == Flavor::ClapDynamic {
            let protocol = self.clap.as_ref().ok_or_else(|| {
                CompletionError::Unsupported("no generated completion protocol".into())
            })?;
            return protocol.complete(self, words, prefix, env, budget);
        }
        if self.flavor == Flavor::Pip {
            // pip completion only reads local parser and environment metadata.
            return pip::complete(self, words, prefix, budget);
        }
        if self.flavor == Flavor::Npm {
            return npm::complete(self, words, prefix, budget);
        }
        if let Some(protocol) = &self.cargo {
            return protocol.complete(self, words, prefix, budget);
        }
        if self.flavor == Flavor::Dotnet {
            let protocol = self.dotnet.as_ref().ok_or_else(|| {
                CompletionError::Unsupported("no verified .NET SDK installation".into())
            })?;
            return protocol.complete(self, words, prefix, budget);
        }
        if self.flavor == Flavor::Urfave {
            let protocol = self.urfave.as_ref().ok_or_else(|| {
                CompletionError::Unsupported("no urfave/cli release was identified".into())
            })?;
            return protocol.complete(self, words, prefix, budget);
        }
        if let Some(protocol) = &self.kingpin {
            return protocol.complete(self, words, prefix, budget);
        }
        if let Some(protocol) = &self.go_flags {
            return protocol.complete(self, words, prefix, budget);
        }
        if let Some(protocol) = self.oclif() {
            return protocol.complete(words, prefix);
        }
        if self.flavor == Flavor::Symfony {
            let protocol = self.symfony().ok_or_else(|| {
                CompletionError::Unsupported("no Symfony Console help was read".into())
            })?;
            return protocol.complete(self, words, prefix, budget);
        }
        if self.flavor == Flavor::RabbitMQ {
            let protocol = self.rabbitmq().ok_or_else(|| {
                CompletionError::Unsupported("no RabbitMQ CLI tool was identified".into())
            })?;
            return protocol.complete(self, words, prefix, budget);
        }
        if self.flavor == Flavor::Yargs {
            let protocol = self.yargs().ok_or_else(|| {
                CompletionError::Unsupported("no yargs completion evidence is known".into())
            })?;
            return protocol.complete(self, words, prefix, budget);
        }
        if self.flavor == Flavor::Click {
            let protocol = self.click.as_ref().ok_or_else(|| {
                CompletionError::Unsupported("no click completion variable is known".into())
            })?;
            return protocol.complete(self, words, prefix, budget);
        }
        if matches!(
            self.flavor,
            Flavor::BashFunction
                | Flavor::FishScript
                | Flavor::ZshFunction
                | Flavor::PowerShellCompleter
        ) {
            close_data_stores(&mut env, &self.name, self.identity.as_deref())?;
            // A handler that calls the app starts its helpers too.
            if let Some(path) = self
                .application
                .as_deref()
                .and_then(|app| startup_stubs(app, &env))
            {
                env.push(("PATH".into(), Some(path)));
            }
        }
        if let Some(registration) = self.powershell_memory() {
            let key = ["powershell-completer"]
                .into_iter()
                .chain(words.iter().copied())
                .chain([prefix])
                .collect::<Vec<_>>();
            let text = self.memoized(&key, || {
                powershell_completer::query(
                    &self.completer,
                    registration,
                    &self.name,
                    words,
                    prefix,
                    env,
                    budget,
                )
            })?;
            return powershell_completer::parse(&text, &self.name, budget.max_candidates);
        }
        if self.flavor == Flavor::BashFunction {
            let source = match (self.bash_memory(), self.helper.as_deref()) {
                (Some(text), _) => bash::Source::Text(text),
                (None, Some(script)) => bash::Source::File(script),
                (None, None) => {
                    return Err(CompletionError::Unsupported("no completion script".into()));
                }
            };
            return bash::complete(
                &self.completer,
                &source,
                &self.name,
                words,
                prefix,
                env,
                budget,
            );
        }
        if self.flavor == Flavor::FishScript {
            let source = match (self.fish_memory(), self.helper.as_deref()) {
                (Some(text), _) => fish::Source::Text(text),
                (None, Some(script)) => fish::Source::File(script),
                (None, None) => {
                    return Err(CompletionError::Unsupported("no completion script".into()));
                }
            };
            return fish::complete(
                &self.completer,
                &source,
                &self.name,
                words,
                prefix,
                env,
                budget,
            );
        }
        if self.flavor == Flavor::ZshFunction {
            let source = match (self.zsh_memory(), self.helper.as_deref()) {
                (Some((definitions, function)), _) => zsh::Source::Text {
                    definitions,
                    function,
                },
                (None, Some(script)) => zsh::Source::File(script),
                (None, None) => {
                    return Err(CompletionError::Unsupported(
                        "no completion function".into(),
                    ));
                }
            };
            return zsh::complete(
                &self.completer,
                &source,
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
            Flavor::FishScript
            | Flavor::ZshFunction
            | Flavor::PowerShellCompleter
            | Flavor::ClapDynamic
            | Flavor::Pip
            | Flavor::Npm => {
                unreachable!("shell queries use their own driver")
            }
            Flavor::Cargo
            | Flavor::Dotnet
            | Flavor::Urfave
            | Flavor::Click
            | Flavor::Kingpin
            | Flavor::GoFlags
            | Flavor::Yargs
            | Flavor::Oclif
            | Flavor::Symfony
            | Flavor::RabbitMQ
            | Flavor::Node
            | Flavor::Clang => {
                unreachable!("these queries use their own bridge")
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
        normalize_bounded(text.split(separator), self.flavor, budget.max_candidates)
    }
}

impl Backend {
    /// cobra's hidden `__complete` command: the words are passed as
    /// arguments (no shell parsing), the answer is `word<TAB>description`
    /// lines and a final `:<directive>` line.
    /// cobra completes a flag's value after a flag that takes one; after a
    /// boolean flag (one with a no-option default) it completes exactly what
    /// it completes without the flag. Equal non-empty answers prove the flag
    /// takes no value; anything else leaves its arity unknown.
    fn cobra_flag_is_boolean(&self, words: &[&str], flag: &str, budget: &mut Budget) -> bool {
        if !flag.starts_with('-') || flag.contains('=') || flag == "-" || flag == "--" {
            return false;
        }
        let Some(after_flag) = self.cobra_answer(words, Some(flag), "", budget) else {
            return false;
        };
        !after_flag.0.is_empty() && self.cobra_answer(words, None, "", budget) == Some(after_flag)
    }

    /// Shell handlers don't say which options take values, but complete a
    /// value after one that does (mtr's handler offers `ADDRESS` after
    /// `--address`) and, after one that doesn't, what they complete without
    /// it (its placeholder words `ip_address hostname`). Equal non-empty
    /// answers prove the option takes no value.
    fn handler_flag_is_boolean(&self, words: &[&str], flag: &str, budget: &mut Budget) -> bool {
        if !flag.starts_with('-') || flag.contains('=') || flag == "-" || flag == "--" {
            return false;
        }
        let mut answer = |words: &[&str]| -> Option<String> {
            let key: Vec<&str> = ["handler-words"]
                .into_iter()
                .chain(words.iter().copied())
                .collect();
            self.memoized(&key, || {
                let mut values: Vec<String> = self
                    .complete(words, "", budget)?
                    .into_iter()
                    .map(|item| item.value)
                    .collect();
                values.sort();
                Ok(values.join("\n"))
            })
            .ok()
        };
        let with: Vec<&str> = words.iter().copied().chain([flag]).collect();
        let Some(after) = answer(&with).filter(|answer| !answer.is_empty()) else {
            return false;
        };
        answer(words) == Some(after)
    }

    /// cobra lists neither aliases nor hidden commands, but resolves them:
    /// the flags offered after such a word are its command's (every command
    /// offers at least its help flag). After a word naming nothing, the root
    /// finds no command and offers nothing, and any other level takes the
    /// word as an argument and offers its own flags again.
    fn cobra_confirms(&self, words: &[&str], typed: &str, budget: &mut Budget) -> Option<bool> {
        if typed.is_empty() || typed.starts_with('-') || typed.contains(char::is_control) {
            return None;
        }
        let Some(child) = self.cobra_answer(words, Some(typed), "-", budget) else {
            return Some(false);
        };
        if child.0.is_empty() {
            return Some(false);
        }
        let parent = self.cobra_answer(words, None, "-", budget)?;
        Some(child.0 != parent.0)
    }

    /// cobra's sorted answer words and directive for `words` (plus `extra`)
    /// and `prefix`, or `None` if it fails or reports a completion error.
    fn cobra_answer(
        &self,
        words: &[&str],
        extra: Option<&str>,
        prefix: &str,
        budget: &mut Budget,
    ) -> Option<(Vec<String>, String)> {
        let args: Vec<&str> = ["__complete"]
            .into_iter()
            .chain(words.iter().copied())
            .chain(extra)
            .chain([prefix])
            .collect();
        let text = self
            .memoized(&args, || {
                run_stdout(
                    &self.completer,
                    args[..].iter().map(OsString::from).collect(),
                    offline_env(Flavor::Cobra),
                    budget,
                    true,
                )
            })
            .ok()?;
        let mut lines: Vec<&str> = text.lines().collect();
        let directive = lines.pop().filter(|line| line.starts_with(':'))?.to_owned();
        if directive[1..]
            .trim()
            .parse::<u32>()
            .is_ok_and(|bits| bits & 1 != 0)
        {
            return None;
        }
        let mut words: Vec<String> = lines
            .into_iter()
            .map(|line| line.split('\t').next().unwrap_or(line).to_owned())
            .collect();
        words.sort();
        Some((words, directive))
    }

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
        // Without ShellCompDirectiveNoFileComp (bit 4), the shell offers
        // files besides the answer: rclone's remotes after `copy` are not
        // the only valid arguments, local paths are too.
        if directive & 4 == 0 && prefix.is_empty() {
            self.cobra_files
                .borrow_mut()
                .insert(words.iter().map(|word| word.to_string()).collect());
        }
        normalize_bounded(lines.into_iter(), Flavor::Cobra, budget.max_candidates)
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

/// [`normalize`], failing when the answer holds more distinct words than the
/// limit: a cut list would make every word past the cut look invalid.
fn normalize_bounded<'a>(
    raw: impl Iterator<Item = &'a str>,
    flavor: Flavor,
    limit: usize,
) -> Result<Vec<CompletionItem>, CompletionError> {
    let items = normalize(raw, flavor, limit.saturating_add(1));
    if items.len() > limit {
        return Err(over_limit());
    }
    Ok(items)
}

fn over_limit() -> CompletionError {
    CompletionError::Failed("the completion answer exceeded the candidate limit".into())
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
    fn offline_probes_cannot_reach_hashicorp_servers_tokens_or_token_helpers() {
        use std::ffi::OsStr;
        let socket = Some(OsString::from("unix:///nonexistent/notypo-offline.sock"));
        let null = Some(OsString::from(if cfg!(windows) {
            "NUL"
        } else {
            "/dev/null"
        }));
        for flavor in [Flavor::Posener, Flavor::Cobra, Flavor::BashFunction] {
            let env: HashMap<_, _> = offline_env(flavor).into_iter().collect();
            for key in [
                "VAULT_ADDR",
                "BAO_ADDR",
                "NOMAD_ADDR",
                "CONSUL_HTTP_ADDR",
                "BOUNDARY_ADDR",
            ] {
                assert_eq!(env.get(OsStr::new(key)), Some(&socket), "{flavor:?} {key}");
            }
            assert_eq!(env.get(OsStr::new("VAULT_CONFIG_PATH")), Some(&null));
            assert_eq!(env.get(OsStr::new("BAO_CONFIG_PATH")), Some(&null));
            for key in [
                "VAULT_AGENT_ADDR",
                "BAO_AGENT_ADDR",
                "VAULT_HTTP_PROXY",
                "VAULT_MAX_RETRIES",
                "VAULT_TOKEN",
                "BAO_TOKEN",
                "NOMAD_TOKEN",
                "CONSUL_HTTP_TOKEN",
                "BOUNDARY_TOKEN",
            ] {
                assert_eq!(env.get(OsStr::new(key)), Some(&None), "{flavor:?} {key}");
            }
        }
        // Lookups the user allowed keep their own servers and credentials.
        assert!(
            network_env(Flavor::Posener)
                .iter()
                .all(|(key, _)| !key.to_string_lossy().contains("VAULT"))
        );
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
            versions: Default::default(),
            force_posix: false,
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
        // Caddy links urfave/cli through smallstep. Its own module is
        // audited; an xcaddy build (`caddy`, plugins unknown) is settled by
        // the library it builds but trusted only by the user.
        let urfave = "github.com/urfave/cli";
        let library = "github.com/caddyserver/caddy/v2";
        assert_eq!(
            go_flavor(
                &module(
                    "github.com/caddyserver/caddy/v2/cmd/caddy",
                    &[cobra, urfave]
                ),
                "caddy",
                &[]
            ),
            Ok(Flavor::Cobra)
        );
        let xcaddy = module("caddy", &[cobra, urfave, library]);
        assert!(matches!(
            go_flavor(&xcaddy, "caddy", &[]),
            Err(GoRefusal::NotTrusted(_))
        ));
        assert_eq!(
            go_flavor(&xcaddy, "caddy", &["caddy".into()]),
            Ok(Flavor::Cobra)
        );
        assert!(matches!(
            go_flavor(
                &module("caddy", &[cobra, urfave]),
                "caddy",
                &["caddy".into()]
            ),
            Err(GoRefusal::Ambiguous(_))
        ));
    }

    /// mkcert's package init runs `brew --prefix nss` on macOS, linked as a
    /// program or as bep/mclib (hugo): probes of such apps find a failing
    /// stub first on PATH, and other apps keep their PATH.
    #[test]
    fn apps_that_start_helpers_at_startup_get_failing_stubs() {
        let dir = Dir::new("startup-helpers");
        let write = |name: &str, module: &str, dependencies: &[&str]| {
            let path = dir.0.join(name);
            fs::write(&path, go::fake_binary(module, dependencies)).unwrap();
            path
        };
        let mkcert = write("mkcert", "filippo.io/mkcert", &[]);
        let hugo = write(
            "hugo",
            "github.com/gohugoio/hugo",
            &["github.com/spf13/cobra", "github.com/bep/mclib"],
        );
        let other = write("tool", "example.com/tool", &["github.com/spf13/cobra"]);
        let env = vec![("PATH".into(), Some("/usr/bin:/bin".into()))];
        for app in [&mkcert, &hugo] {
            let path = startup_stubs(app, &env);
            if !cfg!(target_os = "macos") {
                assert_eq!(path, None);
                continue;
            }
            let path = path.unwrap();
            let mut dirs = std::env::split_paths(&path);
            let stubs = dirs.next().unwrap();
            assert!(stubs.ends_with("stubs/startup"), "{stubs:?}");
            let brew = fs::read_to_string(stubs.join("brew")).unwrap();
            assert_eq!(brew, "#!/bin/sh\nexit 1\n");
            assert_eq!(
                dirs.collect::<Vec<_>>(),
                [PathBuf::from("/usr/bin"), PathBuf::from("/bin")],
                "the probe's own PATH follows"
            );
        }
        assert_eq!(startup_stubs(&other, &env), None);
        assert_eq!(startup_stubs(&dir.0.join("missing"), &env), None);
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
    fn answers_past_the_candidate_limit_fail_instead_of_being_cut() {
        let dir = Dir::new("candidate-limit");
        let app = dir.script(
            "bin/app",
            "#!/bin/sh\nprintf 'alpha\\nbeta\\ngamma\\n:4\\n'\n",
        );
        let backend = Backend::new(Flavor::Cobra, "app", app);
        let mut budget = budget();
        budget.max_candidates = 3;
        assert_eq!(backend.complete(&[], "", &mut budget).unwrap().len(), 3);
        budget.max_candidates = 2;
        // A cut list would call `gamma` invalid and offer `beta` for it.
        assert!(matches!(
            backend.complete(&["x"], "", &mut budget),
            Err(CompletionError::Failed(why)) if why.contains("candidate limit")
        ));
        assert!(normalize_bounded(["a", "a", "b"].into_iter(), Flavor::Posener, 2).is_ok());
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
