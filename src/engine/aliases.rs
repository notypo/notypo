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
            let name = w.literal()?;
            (aliases.contains_key(name) && !seen.iter().any(|s| s == name))
                .then_some((name, w.span))
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
        let value = &aliases[name];
        // tcsh aliases place arguments with history references.
        if dialect == Dialect::Tcsh && value.contains('!') {
            return Expansion::Unanalyzable(name.to_owned());
        }
        let Some(count) = simple_words(value, dialect) else {
            return Expansion::Unanalyzable(name.to_owned());
        };
        let name = name.to_owned();
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
