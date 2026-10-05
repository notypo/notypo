//! Shell aliases as the parent shell reported them. An alias expands only
//! in command position and only when written without quotes; its expansion's
//! first word is expanded again unless it names an alias already expanded
//! there, and an expansion ending in a blank makes the next word eligible
//! too (bash and zsh). Expansions are parsed as data, never run.

use super::diagnosis::effective_program;
use super::parser::{self, Dialect};
use std::collections::HashMap;
use std::path::PathBuf;

/// Splices allowed for one command line before giving up.
const SPLICES: usize = 32;

#[derive(Debug, PartialEq, Eq)]
pub enum Expansion {
    Unchanged,
    Expanded(String),
    /// The named alias expands to something notypo can't analyze (shell
    /// operators, redirections, expansions, tcsh history references).
    Unanalyzable(String),
}

/// The installed program an alias runs, and the words the alias adds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub program: String,
    pub path: PathBuf,
    pub args: Vec<String>,
}

/// One simple command of literal words, without assignments or redirections.
fn simple_words(text: &str, dialect: Dialect) -> Option<usize> {
    let script = parser::parse_with_dialect(text, dialect);
    let [command] = script.commands.as_slice() else {
        return None;
    };
    (script.is_fully_supported()
        && script.compounds.is_empty()
        && command.assignments.is_empty()
        && command.redirections.is_empty()
        && command.words.iter().all(|word| word.literal().is_some()))
    .then_some(command.words.len())
}

/// How `aliases` names a command: PowerShell ignores case, and its
/// wrappers are keyed in lowercase.
fn key(name: &str, dialect: Dialect) -> String {
    if dialect == Dialect::PowerShell {
        name.to_lowercase()
    } else {
        name.to_owned()
    }
}

/// `source` with every alias in command position expanded.
pub fn expand(source: &str, dialect: Dialect, aliases: &HashMap<String, String>) -> Expansion {
    if aliases.is_empty() {
        return Expansion::Unchanged;
    }
    let mut text = source.to_owned();
    let mut changed = false;
    let (mut command, mut word) = (0, 0);
    // Names expanded at the current position, the word after the region
    // expanded so far, and whether the last expansion ended in a blank.
    let mut seen: Vec<String> = Vec::new();
    let mut after = 1;
    let mut blank = false;
    for _ in 0..SPLICES {
        let script = parser::parse_with_dialect(&text, dialect);
        let Some(current) = script.commands.get(command) else {
            return if changed {
                Expansion::Expanded(text)
            } else {
                Expansion::Unchanged
            };
        };
        let alias = current.words.get(word).filter(|w| !w.quoted).and_then(|w| {
            let name = key(w.literal()?, dialect);
            (aliases.contains_key(&name) && !seen.contains(&name)).then_some((name, w.span))
        });
        let Some((name, span)) = alias else {
            if blank && after < current.words.len() {
                (word, after, blank) = (after, after + 1, false);
                seen.clear();
            } else {
                (command, word, after, blank) = (command + 1, 0, 1, false);
                seen.clear();
            }
            continue;
        };
        let value = &aliases[&name];
        // tcsh aliases place arguments with history references.
        if dialect == Dialect::Tcsh && value.contains('!') {
            return Expansion::Unanalyzable(name);
        }
        let Some(count) = simple_words(value, dialect) else {
            return Expansion::Unanalyzable(name);
        };
        text.replace_range(span.start..span.end, value.trim_end());
        if count == 0 {
            // An empty alias leaves the next word in command position.
            after = after.saturating_sub(1);
        } else {
            after += count - 1;
        }
        blank = value.ends_with(char::is_whitespace);
        seen.push(name);
        changed = true;
    }
    Expansion::Unanalyzable(
        source
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .to_owned(),
    )
}

/// Separates the function definitions the shell integration passes.
pub const WRAPPER_SEPARATOR: &str = "#notypo-function";

/// Shell functions that only run one command with all their arguments
/// (`g() { git "$@"; }`, fish's `function g; git $argv; end`), read from
/// their definitions as the shell prints them (`declare -f`, `functions`),
/// one per [`WRAPPER_SEPARATOR`] section. Each maps to the command it runs,
/// in the shell's own syntax, which expands like an alias's value: the
/// arguments follow it. Definitions are parsed, never run.
pub fn wrappers(definitions: &str, dialect: Dialect) -> HashMap<String, String> {
    definitions
        .split(WRAPPER_SEPARATOR)
        .filter_map(|definition| wrapper(definition, dialect))
        .collect()
}

fn wrapper(definition: &str, dialect: Dialect) -> Option<(String, String)> {
    let script = parser::parse_with_dialect(definition, dialect);
    if !script.is_fully_supported() {
        return None;
    }
    // The definition, and in POSIX shells the `{ ... }` body it prints.
    let (function, body) = match script.compounds.as_slice() {
        [function] if dialect == Dialect::Fish => (function, None),
        [function, body] if dialect != Dialect::Fish => (function, Some(body)),
        _ => return None,
    };
    if function.kind != parser::CompoundKind::Function
        || !function.redirections.is_empty()
        || body.is_some_and(|b| b.kind != parser::CompoundKind::Group || !b.redirections.is_empty())
    {
        return None;
    }
    let name = function.words.first()?.literal()?;
    // fish options on the header line (`--wraps=git`) don't change the body.
    if name.is_empty() || name.starts_with('-') {
        return None;
    }
    let [command] = script.commands.as_slice() else {
        return None;
    };
    if !command.assignments.is_empty() || !command.redirections.is_empty() {
        return None;
    }
    let [words @ .., arguments] = command.words.as_slice() else {
        return None;
    };
    let forwards = match dialect {
        Dialect::Fish => ["$argv"].as_slice(),
        _ => ["\"$@\"", "$@"].as_slice(),
    };
    if words.is_empty()
        || !forwards.contains(&arguments.span.of(definition))
        || words.iter().any(|word| word.literal().is_none())
    {
        return None;
    }
    let value = &definition[words[0].span.start..words[words.len() - 1].span.end];
    Some((name.to_owned(), value.to_owned()))
}

/// The installed program `name` runs when it is an alias for one simple
/// command, with the words the alias adds before the user's arguments.
/// Wrappers, package runners, functions, and builtins are not followed.
pub fn target(
    name: &str,
    dialect: Dialect,
    aliases: &HashMap<String, String>,
    which: impl Fn(&str) -> Option<PathBuf>,
    shell_defined: impl Fn(&str) -> bool,
) -> Option<Target> {
    let Expansion::Expanded(text) = expand(name, dialect, aliases) else {
        return None;
    };
    simple_words(&text, dialect)?;
    let script = parser::parse_with_dialect(&text, dialect);
    let command = script.commands.first()?;
    if effective_program(&command.words) != Some(0) {
        return None;
    }
    let words: Vec<&str> = command
        .words
        .iter()
        .map(parser::Word::literal)
        .collect::<Option<_>>()?;
    let program = *words.first()?;
    if program.contains('/') || shell_defined(program) {
        return None;
    }
    Some(Target {
        program: program.to_owned(),
        path: which(program)?,
        args: words[1..].iter().map(|word| word.to_string()).collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn aliases(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect()
    }

    fn expanded(source: &str, pairs: &[(&str, &str)]) -> Expansion {
        expand(source, Dialect::Posix, &aliases(pairs))
    }

    #[test]
    fn aliases_expand_in_command_position_recursively_and_after_blanks() {
        let set = [
            ("ls", "eza --icons"),
            ("ll", "ls -la"),
            ("rmf", "rm -rf"),
            ("rm", "rm -i"),
            ("sudo", "sudo "),
            ("g", "git"),
        ];
        for (source, expected) in [
            ("ls --colro", "eza --icons --colro"),
            ("ll docs", "eza --icons -la docs"),
            ("rmf build", "rm -i -rf build"),
            ("sudo rmf build", "sudo rm -i -rf build"),
            (
                "g status && ls x | rmf y",
                "git status && eza --icons x | rm -i -rf y",
            ),
            ("FOO=1 ls x", "FOO=1 eza --icons x"),
        ] {
            assert_eq!(
                expanded(source, &set),
                Expansion::Expanded(expected.into()),
                "{source}"
            );
        }
        // Quoted names, arguments, and unknown words are not aliases.
        for source in ["'ls' x", "echo ls", "git ls", "\\ls x"] {
            assert_eq!(expanded(source, &set), Expansion::Unchanged, "{source}");
        }
        assert_eq!(
            expanded("ls -G", &[("ls", "ls -G")]),
            Expansion::Expanded("ls -G -G".into()),
            "an alias does not expand into itself again"
        );
    }

    #[test]
    fn expansions_with_shell_syntax_are_unanalyzable() {
        for value in [
            "git pull && git push",
            "ls | less",
            "ls > out",
            "ls $(pwd)",
            "X=1 ls",
            "for f in *; do echo $f; done",
        ] {
            assert_eq!(
                expanded("a x", &[("a", value)]),
                Expansion::Unanalyzable("a".into()),
                "{value}"
            );
        }
        assert_eq!(
            expand("ll x", Dialect::Tcsh, &aliases(&[("ll", "ls -l \\!*")])),
            Expansion::Unanalyzable("ll".into())
        );
        let cycle = aliases(&[("a", "b x"), ("b", "a y")]);
        assert!(matches!(
            expand("a", Dialect::Posix, &cycle),
            Expansion::Expanded(_)
        ));
    }

    /// PowerShell wrapper functions are keyed in lowercase and found
    /// whatever the case of the typed name, as PowerShell finds them.
    #[test]
    fn powershell_wrappers_expand_without_regard_to_case() {
        let set = aliases(&[("gst", "git status"), ("g", "git"), ("gs", "g status")]);
        for (source, expected) in [
            ("GST --short", "git status --short"),
            ("gs -s; Gst", "git status -s; git status"),
            ("& gst", "& gst"),
        ] {
            let result = expand(source, Dialect::PowerShell, &set);
            let text = match result {
                Expansion::Expanded(text) => text,
                Expansion::Unchanged => source.to_owned(),
                other => panic!("{source}: {other:?}"),
            };
            assert_eq!(text, expected, "{source}");
        }
        assert_eq!(
            expand("GST", Dialect::Posix, &set),
            Expansion::Unchanged,
            "other shells compare names exactly"
        );
    }

    /// Definitions as bash's `declare -f`, zsh's `functions`, and fish's
    /// `functions` print them.
    #[test]
    fn wrapper_functions_are_read_from_their_printed_definitions() {
        let bash = "#notypo-function\ng () \n{ \n    git \"$@\"\n}\n\
                    #notypo-function\ngs () \n{ \n    g status \"$@\"\n}\n\
                    #notypo-function\nx () \n{ \n    echo a;\n    git \"$@\"\n}\n\
                    #notypo-function\nq () \n{ \n    git log \"$*\"\n}\n\
                    #notypo-function\nr () \n{ \n    git \"$@\" > log\n}\n\
                    #notypo-function\nv () \n{ \n    \"$EDITOR\" \"$@\"\n}\n\
                    #notypo-function\nl () \n{ \n    ls -la 'my dir' \"$@\"\n}\n";
        let found = wrappers(bash, Dialect::Posix);
        assert_eq!(found["g"], "git");
        assert_eq!(found["gs"], "g status");
        assert_eq!(found["l"], "ls -la 'my dir'", "quoting is kept");
        assert_eq!(found.len(), 3, "{found:?}");
        let zsh = "#notypo-function\ng () {\n\tgit \"$@\"\n}\n#notypo-function\ngs () {\n\tg status $@\n}\n";
        let found = wrappers(zsh, Dialect::Posix);
        assert_eq!(found["gs"], "g status");
        assert_eq!(found.len(), 2);
        let fish = "#notypo-function\n# Defined interactively\nfunction g\n     git $argv; \nend\n\
                    #notypo-function\nfunction gs --wraps=git\n     g status $argv; \nend\n\
                    #notypo-function\nfunction two\n     git status; git log $argv\nend\n";
        let found = wrappers(fish, Dialect::Fish);
        assert_eq!(found["g"], "git");
        assert_eq!(found["gs"], "g status");
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(wrappers(bash, Dialect::Fish).is_empty());
    }

    #[test]
    fn targets_follow_simple_aliases_to_installed_programs_only() {
        let set = aliases(&[
            ("ls", "eza --icons"),
            ("ll", "ls -la"),
            ("gs", "git status"),
            ("please", "sudo"),
            ("up", "cd .."),
            ("pp", "git pull && git push"),
            ("local", "./run.sh"),
        ]);
        let which = |name: &str| {
            (["eza", "git", "sudo"].contains(&name)).then(|| PathBuf::from(format!("/bin/{name}")))
        };
        let builtin = |name: &str| name == "cd";
        let target = |name: &str| target(name, Dialect::Posix, &set, which, builtin);
        assert_eq!(
            target("ll"),
            Some(Target {
                program: "eza".into(),
                path: "/bin/eza".into(),
                args: vec!["--icons".into(), "-la".into()],
            })
        );
        assert_eq!(target("gs").unwrap().args, ["status"]);
        for name in ["please", "up", "pp", "local", "missing"] {
            assert_eq!(target(name), None, "{name}");
        }
    }
}
