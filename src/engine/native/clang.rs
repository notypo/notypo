//! Clang's own completion protocol, which its bash script uses:
//! `clang --autocomplete=<prefix>` lists the driver's options starting with
//! the prefix (every `-W` warning included), one per line with an optional
//! tab and description, and `--autocomplete=<option>,` lists the values of
//! an option that has them (`-std=`, `-stdlib=`). The driver compiles
//! nothing for these requests. GCC rejects the flag, so the answer to
//! `--autocomplete=--version` identifies clang under any name.

use super::{CompletionError, CompletionItem, run_stdout};
use crate::engine::probe::Budget;
use std::ffi::OsString;
use std::path::Path;

/// Names a C or C++ compiler driver goes by; whether it is clang is asked.
pub(super) fn is_candidate(name: &str) -> bool {
    let versioned = |base: &str| {
        name.strip_prefix(base).is_some_and(|rest| {
            rest.is_empty()
                || rest
                    .strip_prefix('-')
                    .is_some_and(|v| !v.is_empty() && v.chars().all(|c| c.is_ascii_digit()))
        })
    };
    versioned("clang") || versioned("clang++") || matches!(name, "cc" | "c++" | "gcc" | "g++")
}

fn ask(
    path: &Path,
    request: &str,
    env: Vec<(OsString, Option<OsString>)>,
    budget: &mut Budget,
) -> Result<String, CompletionError> {
    run_stdout(
        path,
        vec![format!("--autocomplete={request}").into()],
        env,
        budget,
        true,
    )
}

/// Whether the driver at `path` answers clang's protocol.
pub(super) fn confirms(
    path: &Path,
    env: Vec<(OsString, Option<OsString>)>,
    budget: &mut Budget,
) -> bool {
    ask(path, "--version", env, budget).is_ok_and(|text| {
        text.lines()
            .any(|line| line.split('\t').next() == Some("--version"))
    })
}

/// Options (`prefix` starts with `-`) or the values of the option that ends
/// `words`; other positions hold input files.
pub(super) fn complete(
    path: &Path,
    words: &[&str],
    prefix: &str,
    env: Vec<(OsString, Option<OsString>)>,
    budget: &mut Budget,
) -> Result<Vec<CompletionItem>, CompletionError> {
    if prefix.starts_with('-') {
        let text = ask(path, prefix, env, budget)?;
        return parse_options(&text, budget.max_candidates).map_err(CompletionError::Failed);
    }
    let Some(option) = words.last().filter(|word| word.starts_with('-')) else {
        return Ok(Vec::new());
    };
    // Joined options are spelled with their `=` (`-std=`); options with a
    // separate value without it.
    for request in [format!("{option}=,"), format!("{option},")] {
        let text = ask(path, &request, env.clone(), budget)?;
        let values = parse_values(&text, budget.max_candidates).map_err(CompletionError::Failed)?;
        if !values.is_empty() {
            return Ok(values);
        }
    }
    Ok(Vec::new())
}

fn word(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 256
        && !text
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || "'\"`$;&|<>()\\".contains(c))
}

/// `-std=` is reported as `-std` taking a value, as the command line spells
/// it with its value attached; a name listed both ways keeps unknown arity.
fn parse_options(text: &str, limit: usize) -> Result<Vec<CompletionItem>, String> {
    let mut items: Vec<CompletionItem> = Vec::new();
    for line in text.lines() {
        // Some descriptions span lines (`-dD`'s "Only valid with -E.").
        if !line.starts_with('-') && !items.is_empty() {
            continue;
        }
        let (value, description) = line.split_once('\t').unwrap_or((line, ""));
        if !value.starts_with('-') || !word(value) {
            return Err(format!(
                "unexpected clang option `{}`",
                value.escape_debug()
            ));
        }
        let (value, joined) = match value.strip_suffix('=') {
            Some(name) if name.len() > 1 => (name, true),
            _ => (value, false),
        };
        let description = super::clean_description(description);
        match items.iter_mut().find(|item| item.value == value) {
            Some(item) => {
                if item.takes_value != Some(joined) {
                    item.takes_value = None;
                }
                if item.description.is_none() {
                    item.description = description;
                }
            }
            None => {
                if items.len() >= limit {
                    return Err("too many clang options".into());
                }
                items.push(CompletionItem {
                    value: value.to_owned(),
                    takes_value: Some(joined),
                    description,
                });
            }
        }
    }
    // A plain flag's arity is unknown: `-o out` and `-c` look alike here.
    for item in &mut items {
        if item.takes_value == Some(false) {
            item.takes_value = None;
        }
    }
    Ok(items)
}

fn parse_values(text: &str, limit: usize) -> Result<Vec<CompletionItem>, String> {
    let mut items: Vec<CompletionItem> = Vec::new();
    for line in text.lines() {
        let value = line.split('\t').next().unwrap_or(line);
        if value.is_empty() {
            continue;
        }
        if !word(value) || value.starts_with('-') {
            return Err(format!("unexpected clang value `{}`", value.escape_debug()));
        }
        if !items.iter().any(|item| item.value == value) {
            if items.len() >= limit {
                return Err("too many clang values".into());
            }
            items.push(CompletionItem {
                value: value.to_owned(),
                takes_value: None,
                description: None,
            });
        }
    }
    Ok(items)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::engine::native::tests::Dir;
    use std::time::Duration;

    fn budget() -> Budget {
        Budget::new(Duration::from_secs(5), Duration::from_secs(2), 8)
    }

    /// A stand-in driver answering as Apple clang 21 does; anything else
    /// would be a compile.
    const CLANG: &str = r#"#!/bin/sh
case "$*" in
  --autocomplete=--version) printf -- '--version\tPrint version information\n';;
  --autocomplete=-) printf -- '-Wall\n-Wall\t\n-Wextra\n-std=\tLanguage standard to compile for\n-o\tWrite output to <file>\n-I\tAdd directory to include search path\n-c\tOnly run preprocess, compile, and assemble steps\n';;
  --autocomplete=-std=,) printf 'c++17\nc++20\ngnu17\n';;
  --autocomplete=-o=,|--autocomplete=-o,) ;;
  *) touch "$(dirname "$0")/compiled"; exit 1;;
esac
"#;

    #[test]
    fn options_and_joined_values_come_from_the_driver() {
        let dir = Dir::new("clang-protocol");
        let clang = dir.script("clang", CLANG);
        let env = Vec::new();
        assert!(confirms(&clang, env.clone(), &mut budget()));
        let options = complete(&clang, &[], "-", env.clone(), &mut budget()).unwrap();
        let names: Vec<_> = options.iter().map(|i| i.value.as_str()).collect();
        assert_eq!(names, ["-Wall", "-Wextra", "-std", "-o", "-I", "-c"]);
        let std = options.iter().find(|i| i.value == "-std").unwrap();
        assert_eq!(std.takes_value, Some(true));
        assert_eq!(
            std.description.as_deref(),
            Some("Language standard to compile for")
        );
        assert_eq!(
            options[0].takes_value, None,
            "arity of plain flags is unknown"
        );
        let values = complete(&clang, &["-std"], "", env.clone(), &mut budget()).unwrap();
        let values: Vec<_> = values.iter().map(|i| i.value.as_str()).collect();
        assert_eq!(values, ["c++17", "c++20", "gnu17"]);
        assert!(
            complete(&clang, &["-o"], "", env.clone(), &mut budget())
                .unwrap()
                .is_empty()
        );
        // Input files are not vocabulary.
        assert!(
            complete(&clang, &[], "", env, &mut budget())
                .unwrap()
                .is_empty()
        );
        assert!(!dir.0.join("compiled").exists());
    }

    #[test]
    fn gcc_and_malformed_answers_are_not_clang() {
        let dir = Dir::new("clang-gcc");
        let gcc = dir.script(
            "gcc",
            "#!/bin/sh\necho \"gcc: error: unrecognized command-line option '$1'\" >&2\nexit 1\n",
        );
        assert!(!confirms(&gcc, Vec::new(), &mut budget()));
        // Lines after an option continue its description; an option line
        // must be a plain word.
        assert!(parse_options("-Wall\n-$(id)\n", 10).is_err());
        assert!(parse_options("not-an-option\n", 10).is_err());
        // Apple clang 21's `-dD` and `-fveclib=` descriptions span lines.
        let wrapped = parse_options(
            "-dD\tPrint macro definitions\n\nOnly valid with -E.\n-fveclib=\tUse the given vector library\n  Note: -fveclib=libmvec on AArch64 requires GLIBC 2.40.\n",
            10,
        )
        .unwrap();
        assert_eq!(
            wrapped.iter().map(|i| i.value.as_str()).collect::<Vec<_>>(),
            ["-dD", "-fveclib"]
        );
        assert!(parse_values("c++20\n-x\n", 10).is_err());
        assert!(parse_options("-a\n-b\n-c\n", 2).is_err());
        for name in [
            "clang",
            "clang++",
            "clang-21",
            "clang++-18",
            "cc",
            "c++",
            "gcc",
            "g++",
        ] {
            assert!(is_candidate(name), "{name}");
        }
        for name in ["clang-format", "clang-tidy", "gcc-16", "clangd", "ccache"] {
            assert!(!is_candidate(name), "{name}");
        }
    }
}
