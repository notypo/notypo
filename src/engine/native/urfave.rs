//! urfave/cli answers a completion request when its completion flag is the
//! last argument and the app enabled completion: `--generate-bash-completion`
//! (v1, v2) or `--generate-shell-completion` (v3). It prints one word per
//! line for the deepest command the preceding arguments name: its
//! subcommands, or its flags when the word before the flag starts with `-`.
//! v3 appends `:usage`; its own scripts take the text before the first colon
//! as the word. An app without completion rejects the flag as undefined
//! before running hooks or actions, so the first query is always the root's.
//!
//! The library declares no option arity, values, or positionals. Flag
//! actions run for options present on the line, and in v2 a `--` anywhere
//! turns the request back into a normal run of the command. Queries therefore
//! carry only command names the app listed (or, for v3's unlisted aliases,
//! a word whose answer differs from its parent's) and `-` for flags, never
//! options, their values, or `--`. Some releases also run Before hooks while
//! completing: ancestors' hooks for nested queries before v1.22.10 and
//! v2.13.0, and the whole chain for every query from v3.10.0. Those queries
//! need `trusted_help` as well, like other probes that start app code.

use super::{Backend, Capabilities, CompletionError, CompletionItem, Flavor, Trust};
use crate::engine::probe::Budget;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Major {
    V1,
    V2,
    V3,
}

impl Major {
    fn flag(self) -> &'static str {
        match self {
            Major::V1 | Major::V2 => "--generate-bash-completion",
            Major::V3 => "--generate-shell-completion",
        }
    }
}

const LIBRARIES: [(&str, Major); 3] = [
    ("github.com/urfave/cli", Major::V1),
    ("github.com/urfave/cli/v2", Major::V2),
    ("github.com/urfave/cli/v3", Major::V3),
];

/// The urfave/cli major a Go app links and its release, or why several
/// linked majors leave the completion flag undecided.
pub(super) fn linked(module: &super::go::GoModule) -> Result<Option<Protocol>, String> {
    let found: Vec<_> = LIBRARIES
        .iter()
        .filter(|(library, _)| module.uses(library))
        .collect();
    match found.as_slice() {
        [] => Ok(None),
        [(library, major)] => Ok(Some(Protocol {
            major: *major,
            version: module.version(library).map(str::to_owned),
            trusted_help: Vec::new(),
        })),
        _ => Err("links several urfave/cli major versions".into()),
    }
}

#[derive(Clone, Debug)]
pub(super) struct Protocol {
    major: Major,
    /// The linked release; `None` when a replace directive substituted it.
    version: Option<String>,
    pub trusted_help: Vec<String>,
}

/// One command level: the words it lists, and whether the answer also held
/// flags. Default completion never mixes them, so a mixed answer comes from
/// the app's own callback (lefthook lists configured hooks this way), whose
/// words are local resources rather than subcommands.
#[derive(Debug, PartialEq, Eq)]
struct Level {
    words: Vec<CompletionItem>,
    custom: bool,
}

struct Walk<'a> {
    path: Vec<&'a str>,
    /// A word that names no command was passed: the slot is an argument.
    positional: bool,
}

/// Whether a query may start the app or must reuse this request's answers.
enum Mode<'a> {
    Probe(&'a mut Budget),
    Memo,
}

/// `v1.22.10` → (1, 22, 10, true); a pre-release or pseudo-version sorts
/// before its release.
fn release(version: &str) -> Option<(u64, u64, u64, bool)> {
    let version = version.strip_prefix('v')?;
    let version = version.split('+').next()?;
    let (numbers, pre) = match version.split_once('-') {
        Some((numbers, _)) => (numbers, true),
        None => (version, false),
    };
    let mut parts = numbers.split('.').map(str::parse::<u64>);
    let (Some(Ok(major)), Some(Ok(minor)), Some(Ok(patch)), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return None;
    };
    Some((major, minor, patch, !pre))
}

impl Protocol {
    /// Whether answering a query at this depth runs the app's Before hooks.
    fn runs_hooks(&self, nested: bool) -> bool {
        let Some(version) = self.version.as_deref().and_then(release) else {
            // A substituted or unversioned library: assume hooks run.
            return nested || self.major == Major::V3;
        };
        match self.major {
            Major::V1 => nested && version < (1, 22, 10, true),
            Major::V2 => nested && version < (2, 13, 0, true),
            Major::V3 => (version.0, version.1, version.2) >= (3, 10, 0),
        }
    }

    fn permit(&self, backend: &Backend, nested: bool) -> Result<(), CompletionError> {
        let allowed = self.trusted_help.iter().any(|trusted| {
            trusted == "*" || *trusted == backend.name || Some(trusted) == backend.identity.as_ref()
        });
        if self.runs_hooks(nested) && !allowed {
            return Err(CompletionError::Failed(format!(
                "urfave/cli {} runs the app's Before hooks while completing{}; add {} to trusted_help to allow it",
                self.version.as_deref().unwrap_or("(replaced module)"),
                if nested { " a subcommand" } else { "" },
                backend.name
            )));
        }
        Ok(())
    }

    pub fn capabilities(&self, trust: Trust) -> Capabilities {
        Capabilities {
            subcommands: true,
            options: true,
            option_arity: false,
            values: false,
            resources: false,
            option_prefix: "-",
            // v3 prints each flag's and command's primary name only.
            short_options: self.major != Major::V3,
            complete_options: self.major == Major::V2,
            complete_subcommands: self.major != Major::V3,
            descriptions: self.major == Major::V3,
            query_dialect: None,
            trust,
        }
    }

    pub fn capabilities_for(&self, words: &[&str], trust: Trust) -> Capabilities {
        let mut capabilities = self.capabilities(trust);
        // v1 adds the running app's flags to nested commands' lists; only the
        // root's list is exact.
        if self.major == Major::V1 && words.is_empty() {
            capabilities.complete_options = true;
        }
        // A command with subcommands may also take arguments, which urfave/cli
        // never lists: below the root, an unlisted word can be one of them.
        if words.iter().any(|word| !word.starts_with('-')) {
            capabilities.complete_subcommands = false;
        }
        // An option without `=value` makes the next word's role a guess.
        if words
            .iter()
            .any(|word| word.starts_with('-') && !word.contains('='))
        {
            capabilities.complete_options = false;
            capabilities.complete_subcommands = false;
        }
        capabilities
    }

    fn raw(
        &self,
        backend: &Backend,
        path: &[&str],
        options: bool,
        mode: &mut Mode<'_>,
    ) -> Result<String, CompletionError> {
        let mut args: Vec<&str> = path.to_vec();
        if options {
            args.push("-");
        }
        args.push(self.major.flag());
        let mut key = vec!["urfave"];
        key.extend_from_slice(&args);
        let budget = match mode {
            Mode::Probe(budget) => budget,
            Mode::Memo => {
                let key: Vec<String> = key.iter().map(|k| k.to_string()).collect();
                return backend.memo.borrow().get(&key).cloned().unwrap_or_else(|| {
                    Err(CompletionError::Failed(
                        "urfave/cli level was not queried in this request".into(),
                    ))
                });
            }
        };
        self.permit(backend, !path.is_empty())?;
        backend.memoized(&key, || {
            super::run_stdout(
                &backend.completer,
                args.iter().map(Into::into).collect(),
                super::offline_env(Flavor::Urfave),
                budget,
                true,
            )
            .map_err(|error| {
                CompletionError::Failed(format!(
                    "urfave/cli completion {error} (the app may not enable completion)"
                ))
            })
        })
    }

    fn parse(&self, text: &str, limit: usize) -> Result<Vec<CompletionItem>, CompletionError> {
        let mut items: Vec<CompletionItem> = Vec::new();
        for line in text.lines().filter(|line| !line.is_empty()) {
            let (word, description) = match self.major {
                Major::V3 => match line.split_once(':') {
                    // `db:migrate:Run migrations`: the name held a colon, and
                    // v3's own scripts would offer `db`. Offer neither.
                    Some((_, usage))
                        if usage
                            .split(char::is_whitespace)
                            .next()
                            .is_some_and(|head| head.contains(':')) =>
                    {
                        continue;
                    }
                    Some((word, usage)) => (word, super::clean_description(usage)),
                    None => (line, None),
                },
                Major::V1 | Major::V2 => (line, None),
            };
            // Usage text means the request was not answered as completion.
            if word.is_empty()
                || matches!(word, "-" | "--")
                || word.contains(char::is_whitespace)
                || word.contains(char::is_control)
            {
                return Err(CompletionError::Failed(
                    "urfave/cli completion returned a line that is not one word".into(),
                ));
            }
            if items.iter().any(|item| item.value == word) {
                continue;
            }
            if items.len() >= limit {
                return Err(CompletionError::Failed(
                    "urfave/cli completion exceeded the candidate limit".into(),
                ));
            }
            items.push(CompletionItem {
                value: word.to_owned(),
                takes_value: None,
                description,
            });
        }
        Ok(items)
    }

    fn limit(mode: &Mode<'_>) -> usize {
        match mode {
            Mode::Probe(budget) => budget.max_candidates,
            Mode::Memo => usize::MAX,
        }
    }

    fn level(
        &self,
        backend: &Backend,
        path: &[&str],
        mode: &mut Mode<'_>,
    ) -> Result<Level, CompletionError> {
        let limit = Self::limit(mode);
        let items = self.parse(&self.raw(backend, path, false, mode)?, limit)?;
        let custom = items.iter().any(CompletionItem::is_option);
        Ok(Level {
            words: items.into_iter().filter(|item| !item.is_option()).collect(),
            custom,
        })
    }

    fn options(
        &self,
        backend: &Backend,
        path: &[&str],
        mode: &mut Mode<'_>,
    ) -> Result<Vec<CompletionItem>, CompletionError> {
        let limit = Self::limit(mode);
        let mut items: Vec<CompletionItem> = self
            .parse(&self.raw(backend, path, true, mode)?, limit)?
            .into_iter()
            .filter(CompletionItem::is_option)
            .collect();
        if self.major == Major::V1
            && let Some((_, parent)) = path.split_last()
        {
            // v1 prints the running app's flags (the root's, or the parent
            // command's) before the command's own. They are not accepted
            // after the command; drop everything the parent level lists.
            let parent = self.parse(&self.raw(backend, parent, true, mode)?, limit)?;
            items.retain(|item| !parent.iter().any(|other| other.value == item.value));
        }
        Ok(items)
    }

    fn walk<'a>(
        &self,
        backend: &Backend,
        words: &[&'a str],
        mode: &mut Mode<'_>,
    ) -> Result<Walk<'a>, CompletionError> {
        let mut path: Vec<&str> = Vec::new();
        let mut level = self.level(backend, &path, mode)?;
        let mut i = 0;
        while let Some(&word) = words.get(i) {
            i += 1;
            if word == "--" {
                return Ok(Walk {
                    path,
                    positional: true,
                });
            }
            if word.starts_with('-') && word != "-" {
                // Only `--name=value` states its value. Otherwise take the
                // next word as the value unless the app lists it as a command.
                if !word.contains('=')
                    && words.get(i).is_some_and(|next| {
                        !next.starts_with('-')
                            && !level.words.iter().any(|item| item.value == *next)
                    })
                {
                    i += 1;
                }
                continue;
            }
            if !level.custom && level.words.iter().any(|item| item.value == word) {
                path.push(word);
                level = self.level(backend, &path, mode)?;
                continue;
            }
            if self.major == Major::V3 && !level.custom {
                // v3 lists neither aliases nor hidden commands. A word naming
                // no command gets its parent's answer again.
                let mut child = path.clone();
                child.push(word);
                let answer = self.level(backend, &child, mode)?;
                if answer != level {
                    path = child;
                    level = answer;
                    continue;
                }
            }
            return Ok(Walk {
                path,
                positional: true,
            });
        }
        Ok(Walk {
            path,
            positional: false,
        })
    }

    pub fn complete(
        &self,
        backend: &Backend,
        words: &[&str],
        prefix: &str,
        budget: &mut Budget,
    ) -> Result<Vec<CompletionItem>, CompletionError> {
        if words
            .iter()
            .chain([&prefix])
            .any(|word| word.contains(char::is_control))
        {
            return Err(CompletionError::Unsupported(
                "urfave/cli completion context contains control characters".into(),
            ));
        }
        let mut mode = Mode::Probe(budget);
        let walk = self.walk(backend, words, &mut mode)?;
        let mut items = if prefix.starts_with('-') {
            self.options(backend, &walk.path, &mut mode)?
        } else if walk.positional {
            return Err(CompletionError::Unsupported(
                "urfave/cli does not list positional arguments".into(),
            ));
        } else {
            let level = self.level(backend, &walk.path, &mut mode)?;
            // Every command also lists the help command: one listing nothing
            // else has no subcommands, so this slot holds an argument.
            if !level.words.is_empty()
                && !level.custom
                && level
                    .words
                    .iter()
                    .all(|item| matches!(item.value.as_str(), "help" | "h"))
            {
                return Err(CompletionError::Unsupported(
                    "this urfave/cli command has no subcommands; its arguments are not listed"
                        .into(),
                ));
            }
            level.words
        };
        items.retain(|item| item.value.starts_with(prefix));
        Ok(items)
    }

    /// v3 lists neither aliases nor hidden commands: a word naming one gets
    /// a different answer than its parent's. Earlier majors list aliases.
    pub fn confirms(
        &self,
        backend: &Backend,
        words: &[&str],
        typed: &str,
        budget: &mut Budget,
    ) -> Option<bool> {
        if self.major != Major::V3
            || typed.is_empty()
            || typed.starts_with('-')
            || typed.contains(char::is_control)
            || words.iter().any(|word| word.contains(char::is_control))
        {
            return None;
        }
        let mut mode = Mode::Probe(budget);
        let walk = self.walk(backend, words, &mut mode).ok()?;
        if walk.positional {
            return None;
        }
        let level = self.level(backend, &walk.path, &mut mode).ok()?;
        if level.custom {
            return None;
        }
        let mut child = walk.path.clone();
        child.push(typed);
        let answer = self.level(backend, &child, &mut mode).ok()?;
        Some(answer != level)
    }

    /// Words from an app callback rather than the command tree, judged from
    /// this request's answers only; unknown levels count as resources.
    pub fn resources(&self, backend: &Backend, words: &[&str]) -> bool {
        let mut mode = Mode::Memo;
        match self.walk(backend, words, &mut mode) {
            Ok(walk) if !walk.positional => self
                .level(backend, &walk.path, &mut mode)
                .map_or(true, |level| level.custom),
            _ => true,
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::super::{
        Backend, Discovery, NativeCompletionBackend, discover, discover_for_shell_with_budget, go,
        tests::Dir,
    };
    use super::*;
    use crate::shells::Shell;
    use std::path::PathBuf;
    use std::time::Duration;

    const MODULE: &str = "example.com/urfave-fixture";
    const V1: &str = "github.com/urfave/cli";
    const V2: &str = "github.com/urfave/cli/v2";
    const V3: &str = "github.com/urfave/cli/v3";

    /// Answers like the real libraries' default completion did for a small
    /// app built against v1.22.16, v2.27.7, and v3.14.0: words per level,
    /// flags after `-`, usage text for an undefined flag, and the command's
    /// action whenever a `--` precedes the completion flag (v2).
    fn app(dir: &Dir, library: &str) -> PathBuf {
        let v3 = library == V3;
        let flag = if v3 {
            "--generate-shell-completion"
        } else {
            "--generate-bash-completion"
        };
        let path = dir.script(
            "bin/fixture",
            &format!(
                r#"#!/bin/sh
root=$(dirname "$0")
env | grep -q '^0=' && exit 7
[ -z "$_CLI_ZSH_AUTOCOMPLETE_HACK" ] || exit 7
[ "$HTTPS_PROXY" = http://127.0.0.1:9 ] || exit 8
for arg in "$@"; do printf '<%s>' "$arg" >> "$root/queries"; done
printf '\n' >> "$root/queries"
case " $* " in *' -- '*) touch "$root/operation-marker"; exit 0;; esac
last=""
words=""
for arg in "$@"; do last=$arg; [ "$arg" = {flag} ] || words="$words $arg"; done
if [ "$last" != {flag} ]; then
  echo "Incorrect Usage: flag provided but not defined"; touch "$root/operation-marker"; exit 1
fi
if [ -f "$root/mode" ]; then
  case "$(cat "$root/mode")" in
    usage) printf 'NAME:\n   fixture - usage text\n'; exit 0;;
    disabled) echo "Incorrect Usage: flag provided but not defined: -${{last#--}}"; exit 1;;
    utf8) printf '\377\n'; exit 0;;
    hanging) (sleep 1; touch "$root/delayed-marker") & wait; exit 0;;
  esac
fi
case "$words" in
  ' -') cat "$root/root-flags";;
  ' deploy -'|' d -') cat "$root/deploy-flags";;
  ' deploy'|' d') cat "$root/deploy";;
  ' deploy status -'|' d status -') cat "$root/status-flags";;
  ' deploy status'|' d status') cat "$root/status";;
  ' hooks'|' hooks -') printf -- '--force\npre-commit\npre-push\n';;
  *) cat "$root/root";;
esac
"#
            ),
        );
        let text = |plain: &'static str, usage: &'static str| if v3 { usage } else { plain };
        dir.script(
            "bin/root",
            text(
                "deploy\nd\nhooks\nhelp\nh\n",
                "deploy:Deploy an app\nhooks:Run hooks\nhelp:Shows help\n",
            ),
        );
        dir.script(
            "bin/root-flags",
            text(
                "--config\n-c\n--help\n-h\n",
                "--config:config file\n--help:show help\n",
            ),
        );
        dir.script(
            "bin/deploy",
            text("status\nhelp\nh\n", "status:Show status\nhelp:Shows help\n"),
        );
        dir.script(
            "bin/deploy-flags",
            text(
                "--region\n--force\n-f\n--help\n-h\n",
                "--region:target region\n--force\n--help:show help\n--config:config file\n",
            ),
        );
        // Leaves list only the help command (v2, v3) or nothing (v1).
        dir.script(
            "bin/status",
            match library {
                V1 => "",
                V2 => "help\nh\n",
                _ => "help:Shows help\n",
            },
        );
        // v1 prints the running (parent) app's flags before the command's.
        dir.script(
            "bin/status-flags",
            if library == V1 {
                "--region\n--force\n-f\n--help\n-h\n--watch\n"
            } else {
                "--watch\n--help\n"
            },
        );
        path
    }

    fn backend(path: PathBuf, library: &str, version: &str, help: &[&str]) -> Backend {
        let module = go::GoModule {
            path: MODULE.into(),
            libraries: vec![library.into()],
            versions: [(library.to_owned(), version.to_owned())].into(),
            force_posix: false,
        };
        let mut protocol = linked(&module).unwrap().unwrap();
        protocol.trusted_help = help.iter().map(|h| h.to_string()).collect();
        let mut backend = Backend::new(Flavor::Urfave, "fixture", path);
        backend.identity = Some(MODULE.into());
        backend.urfave = Some(Box::new(protocol));
        backend
    }

    fn budget() -> Budget {
        Budget::new(Duration::from_secs(10), Duration::from_secs(5), 16)
    }

    fn values(items: &[CompletionItem]) -> Vec<&str> {
        items.iter().map(|item| item.value.as_str()).collect()
    }

    fn queries(dir: &Dir) -> String {
        std::fs::read_to_string(dir.0.join("bin/queries")).unwrap_or_default()
    }

    #[test]
    fn versions_order_releases_after_their_prereleases() {
        assert_eq!(release("v2.27.7"), Some((2, 27, 7, true)));
        assert!(release("v2.13.0-rc1").unwrap() < (2, 13, 0, true));
        assert!(release("v1.22.11-0.20230101000000-abcdef").unwrap() > (1, 22, 10, true));
        assert_eq!(
            release("v0.0.0-20210324024421-b6ea8234fe3d"),
            Some((0, 0, 0, false))
        );
        assert_eq!(release("(devel)"), None);
        for (major, version, root, nested) in [
            (Major::V1, Some("v1.22.9"), false, true),
            (Major::V1, Some("v1.22.10"), false, false),
            (Major::V2, Some("v2.12.0"), false, true),
            (Major::V2, Some("v2.13.0"), false, false),
            (Major::V2, Some("v2.27.7"), false, false),
            (Major::V2, None, false, true),
            (Major::V3, Some("v3.9.1"), false, false),
            (Major::V3, Some("v3.10.0-rc1"), true, true),
            (Major::V3, Some("v3.14.0"), true, true),
            (Major::V3, None, true, true),
        ] {
            let protocol = Protocol {
                major,
                version: version.map(str::to_owned),
                trusted_help: Vec::new(),
            };
            assert_eq!(protocol.runs_hooks(false), root, "{major:?} {version:?}");
            assert_eq!(protocol.runs_hooks(true), nested, "{major:?} {version:?}");
        }
    }

    #[test]
    fn identity_trust_and_ambiguous_libraries_gate_discovery_without_running_the_app() {
        let dir = Dir::new("urfave-discovery");
        let path = dir.0.join("bin/tool");
        std::fs::create_dir_all(dir.0.join("bin")).unwrap();
        std::fs::write(
            &path,
            go::fake_binary_info(MODULE, &format!("dep\t{V3}\tv3.10.1\th1:x\n")),
        )
        .unwrap();
        assert!(matches!(
            discover("tool", &path, &[]),
            Discovery::NotTrusted(why) if why.contains("urfave/cli")
        ));
        let mut budget = budget();
        let Discovery::Found(found) = discover_for_shell_with_budget(
            "renamed",
            &path,
            &[MODULE.into()],
            &["renamed".into()],
            Shell::Bash,
            &mut budget,
        ) else {
            panic!()
        };
        assert_eq!(found.flavor, Flavor::Urfave);
        assert_eq!(found.identity.as_deref(), Some(MODULE));
        assert_eq!(found.cache_identity(), None);
        let protocol = found.urfave.as_deref().unwrap();
        assert_eq!(
            (protocol.major, protocol.version.as_deref()),
            (Major::V3, Some("v3.10.1"))
        );
        assert_eq!(protocol.trusted_help, ["renamed"]);
        for lines in [
            format!("dep\t{V2}\tv2.27.7\th1:x\ndep\t{V3}\tv3.14.0\th1:y\n"),
            format!("dep\tgithub.com/spf13/cobra\tv1.10.2\th1:x\ndep\t{V3}\tv3.10.1\th1:y\n"),
            format!("dep\tgithub.com/posener/complete\tv1.2.3\th1:x\ndep\t{V3}\tv3.10.1\th1:y\n"),
        ] {
            std::fs::write(&path, go::fake_binary_info(MODULE, &lines)).unwrap();
            assert!(
                matches!(
                    discover("tool", &path, &["*".into()]),
                    Discovery::Unavailable(_)
                ),
                "{lines}"
            );
        }
        // An audit settles which library parses the command line (tofu).
        std::fs::write(
            &path,
            go::fake_binary_info(
                "github.com/opentofu/opentofu/cmd/tofu",
                &format!(
                    "dep\tgithub.com/posener/complete\tv1.2.3\th1:x\ndep\t{V3}\tv3.10.1\th1:y\n"
                ),
            ),
        )
        .unwrap();
        assert!(matches!(
            discover("tofu", &path, &[]),
            Discovery::Found(backend) if backend.flavor == Flavor::Posener
        ));
    }

    #[test]
    fn v2_lists_verified_levels_and_flags_without_sending_options_or_double_dashes() {
        let dir = Dir::new("urfave-v2");
        let backend = backend(app(&dir, V2), V2, "v2.27.7", &[]);
        let mut budget = budget();
        assert_eq!(
            values(&backend.complete(&[], "", &mut budget).unwrap()),
            ["deploy", "d", "hooks", "help", "h"]
        );
        assert_eq!(
            values(&backend.complete(&[], "-", &mut budget).unwrap()),
            ["--config", "-c", "--help", "-h"]
        );
        let nested = backend
            .complete(
                &["--config", "$(touch operation-marker)", "deploy"],
                "",
                &mut budget,
            )
            .unwrap();
        assert_eq!(values(&nested), ["status", "help", "h"]);
        assert!(
            !backend
                .capabilities_for(&["--config", "x", "deploy"])
                .complete_subcommands
        );
        assert!(backend.capabilities_for(&[]).complete_subcommands);
        assert!(!backend.capabilities_for(&["deploy"]).complete_subcommands);
        assert!(backend.capabilities_for(&["deploy"]).complete_options);
        // A leaf lists only the help command: its slot is an argument.
        assert!(matches!(
            backend.complete(&["deploy", "status"], "", &mut budget),
            Err(CompletionError::Unsupported(_))
        ));
        assert_eq!(
            values(
                &backend
                    .complete(&["d", "status"], "-", &mut budget)
                    .unwrap()
            ),
            ["--watch", "--help"]
        );
        // Arguments and anything after `--` are never command words.
        for words in [
            &["deploy", "my-app"][..],
            &["deploy", "--", "status"],
            &["--config=x", "unknown"],
        ] {
            assert!(
                matches!(
                    backend.complete(words, "", &mut budget),
                    Err(CompletionError::Unsupported(_))
                ),
                "{words:?}"
            );
        }
        assert!(!backend.candidate_is_resource(&["deploy"], &nested[0]));
        let queries = queries(&dir);
        assert!(!queries.contains("<--config>"), "{queries}");
        assert!(!queries.contains("<-->"), "{queries}");
        assert!(!queries.contains("my-app"), "{queries}");
        assert!(!queries.contains("unknown"), "{queries}");
        assert!(!dir.0.join("bin/operation-marker").exists());
    }

    #[test]
    fn v1_nested_flags_drop_the_parent_levels_flags() {
        let dir = Dir::new("urfave-v1");
        let backend = backend(app(&dir, V1), V1, "v1.22.16", &[]);
        let mut budget = budget();
        assert_eq!(
            values(
                &backend
                    .complete(&["deploy", "status"], "-", &mut budget)
                    .unwrap()
            ),
            ["--watch"]
        );
        assert!(backend.capabilities_for(&[]).complete_options);
        assert!(!backend.capabilities_for(&["deploy"]).complete_options);
        assert!(backend.capabilities().short_options);
    }

    #[test]
    fn v3_reads_usage_follows_unlisted_aliases_and_needs_help_trust_for_hooks() {
        let dir = Dir::new("urfave-v3");
        let path = app(&dir, V3);
        let mut budget = budget();
        let untrusted = backend(path.clone(), V3, "v3.14.0", &[]);
        assert!(matches!(
            untrusted.complete(&[], "", &mut budget),
            Err(CompletionError::Failed(why)) if why.contains("trusted_help")
        ));
        assert!(
            queries(&dir).is_empty(),
            "hook-running queries need trusted_help"
        );
        let backend = self::backend(path.clone(), V3, "v3.14.0", &["fixture"]);
        let root = backend.complete(&[], "", &mut budget).unwrap();
        assert_eq!(values(&root), ["deploy", "hooks", "help"]);
        assert_eq!(root[0].description.as_deref(), Some("Deploy an app"));
        // `d` is not listed, but its answer differs from the root's.
        assert_eq!(
            values(&backend.complete(&["d"], "", &mut budget).unwrap()),
            ["status", "help"]
        );
        assert!(matches!(
            backend.complete(&["unknown"], "", &mut budget),
            Err(CompletionError::Unsupported(_))
        ));
        let capabilities = backend.capabilities();
        assert!(!capabilities.short_options);
        assert!(!capabilities.complete_subcommands);
        assert!(capabilities.descriptions);
        let identity = self::backend(path.clone(), V3, "v3.14.0", &[MODULE]);
        assert!(identity.complete(&[], "", &mut budget).is_ok());
        let older = self::backend(path, V3, "v3.9.1", &[]);
        assert!(older.complete(&[], "", &mut budget).is_ok());
    }

    #[test]
    fn mixed_callback_answers_are_resources_and_end_the_command_path() {
        let dir = Dir::new("urfave-callback");
        let backend = backend(app(&dir, V2), V2, "v2.27.7", &[]);
        let mut budget = budget();
        let hooks = backend.complete(&["hooks"], "", &mut budget).unwrap();
        assert_eq!(values(&hooks), ["pre-commit", "pre-push"]);
        assert!(backend.candidate_is_resource(&["hooks"], &hooks[0]));
        assert!(matches!(
            backend.complete(&["hooks", "pre-commit"], "", &mut budget),
            Err(CompletionError::Unsupported(_))
        ));
        // A level this request never asked about is judged conservatively.
        let fresh = self::backend(app(&dir, V2), V2, "v2.27.7", &[]);
        assert!(fresh.candidate_is_resource(&["deploy"], &hooks[0]));
    }

    #[test]
    fn old_releases_need_help_trust_for_nested_queries_only() {
        let dir = Dir::new("urfave-old");
        let path = app(&dir, V2);
        let mut budget = budget();
        let old = backend(path.clone(), V2, "v2.3.0", &[]);
        assert!(old.complete(&[], "", &mut budget).is_ok());
        assert!(matches!(
            old.complete(&["deploy"], "", &mut budget),
            Err(CompletionError::Failed(why)) if why.contains("Before hooks")
        ));
        assert!(!queries(&dir).contains("<deploy>"));
        let trusted = backend(path, V2, "v2.3.0", &["fixture"]);
        assert!(trusted.complete(&["deploy"], "", &mut budget).is_ok());
    }

    #[test]
    fn disabled_completion_usage_text_and_bad_output_fail_closed() {
        for (mode, why) in [
            ("disabled", "exited"),
            ("usage", "not one word"),
            ("utf8", "UTF-8"),
            ("hanging", "timed out"),
        ] {
            let dir = Dir::new("urfave-failures");
            let backend = backend(app(&dir, V2), V2, "v2.27.7", &[]);
            dir.script("bin/mode", mode);
            let mut budget = Budget::new(Duration::from_secs(3), Duration::from_millis(300), 4);
            let result = backend.complete(&["deploy"], "", &mut budget);
            assert!(
                matches!(&result, Err(CompletionError::Failed(message)) if message.contains(why)),
                "{mode}: {result:?}"
            );
            // The root is asked first; a failed root stops every deeper query.
            assert!(!queries(&dir).contains("<deploy>"), "{mode}");
            if mode == "hanging" {
                std::thread::sleep(Duration::from_millis(1100));
                assert!(!dir.0.join("bin/delayed-marker").exists());
            }
        }
        let dir = Dir::new("urfave-limit");
        let backend = backend(app(&dir, V2), V2, "v2.27.7", &[]);
        let mut budget = budget();
        budget.max_candidates = 2;
        assert!(matches!(
            backend.complete(&[], "", &mut budget),
            Err(CompletionError::Failed(why)) if why.contains("limit")
        ));
        assert!(matches!(
            backend.complete(&["line\nbreak"], "", &mut budget),
            Err(CompletionError::Unsupported(_))
        ));
    }

    #[test]
    fn the_engine_repairs_levels_and_flags_and_gates_callback_resources() {
        use crate::engine::{self, FailureContext, Outcome, safety::Decision};
        use crate::settings::Settings;
        use crate::types::Context;
        let dir = Dir::new("urfave-engine");
        let path = app(&dir, V2);
        let ctx = Context::new(Settings::default(), Shell::Bash, "fuck".into())
            .with_which("fixture", Some(path.to_str().unwrap()))
            .with_which("man", None)
            .with_history::<&str>(&[]);
        let run = |source: &str| {
            let backend: std::rc::Rc<dyn NativeCompletionBackend> =
                std::rc::Rc::new(backend(path.clone(), V2, "v2.27.7", &[]));
            let failure = FailureContext {
                source: source.into(),
                ..FailureContext::default()
            };
            engine::correct_with_backends(&failure, &ctx, vec![("fixture".into(), backend)])
        };
        let first = |source: &str| {
            let report = run(source);
            let candidate = report.outcome.candidates().first().cloned();
            (report, candidate)
        };
        let (report, candidate) = first("fixture deplyo");
        assert!(
            matches!(report.outcome, Outcome::Suggestion(_)),
            "{:?}",
            report.notes
        );
        assert_eq!(candidate.unwrap().script, "fixture deploy");
        // Below the root, lists are partial: an argument is not suspicious,
        // and a close subcommand is offered for confirmation.
        let (report, candidate) = first("fixture deploy statsu");
        assert!(
            matches!(report.outcome, Outcome::Ambiguous(_)),
            "{:?}",
            report.notes
        );
        assert_eq!(candidate.unwrap().script, "fixture deploy status");
        let (_, candidate) = first("fixture deploy my-app --regoin eu");
        assert_eq!(
            candidate.unwrap().script,
            "fixture deploy my-app --region eu"
        );
        let (_, candidate) = first("fixture --confg x deploy");
        assert_eq!(candidate.unwrap().script, "fixture --config x deploy");
        let (_, candidate) = first("fixture hooks pre-comit");
        let candidate = candidate.unwrap();
        assert_eq!(candidate.script, "fixture hooks pre-commit");
        assert_eq!(candidate.safety.decision, Decision::Confirm);
        assert!(candidate.edits.iter().all(|edit| edit.resource));
        let queries = queries(&dir);
        assert!(!queries.contains("<--config>"), "{queries}");
        assert!(!queries.contains("<x>"), "{queries}");
        assert!(!queries.contains("<my-app>"), "{queries}");
        assert!(!dir.0.join("bin/operation-marker").exists());
    }

    #[test]
    fn upgrades_are_answered_by_the_installed_app_on_the_next_request() {
        let dir = Dir::new("urfave-upgrade");
        let path = app(&dir, V2);
        let mut budget = budget();
        let before = backend(path.clone(), V2, "v2.27.7", &[]);
        assert!(!values(&before.complete(&[], "", &mut budget).unwrap()).contains(&"rollback"));
        dir.script("bin/root", "deploy\nd\nrollback\nhooks\nhelp\nh\n");
        let after = backend(path, V2, "v2.27.7", &[]);
        assert!(values(&after.complete(&[], "", &mut budget).unwrap()).contains(&"rollback"));
    }
}
