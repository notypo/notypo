//! Reading options and subcommands from local documentation: formatted man
//! pages and `--help` output. Documentation can lag the installed program,
//! so what it lists is never treated as complete.

use super::native::CompletionItem;

fn option_declaration(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    (line.len() > trimmed.len() && trimmed.starts_with('-')).then(|| {
        regex!(r"(?: {2,}|\t+)")
            .split(trimmed)
            .next()
            .unwrap_or(trimmed)
    })
}

/// Separate aliases without splitting commas/pipes inside value placeholders.
fn aliases(head: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut start = 0;
    for (i, c) in head.char_indices() {
        match c {
            '[' | '{' | '<' => depth += 1,
            ']' | '}' | '>' => depth = depth.saturating_sub(1),
            ',' | '|' if depth == 0 => {
                parts.push(head[start..i].trim());
                start = i + c.len_utf8();
            }
            _ => {}
        }
    }
    parts.push(head[start..].trim());
    parts
}

/// Removes terminal formatting: overstrike (`X\bX` bold, `_\bX`
/// underline) and ANSI CSI sequences, which some programs print even when
/// their output is not a terminal (swift's help is bold).
pub fn strip_formatting(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' && chars.peek() == Some(&'[') {
            chars.next();
            // Parameters and intermediates, then one final byte.
            for c in chars.by_ref() {
                if ('\x40'..='\x7e').contains(&c) {
                    break;
                }
            }
            continue;
        }
        if chars.peek() == Some(&'\x08') {
            chars.next();
            continue;
        }
        if c != '\x08' {
            out.push(c);
        }
    }
    out
}

/// Options documented in `text`. A line starting with options defines them:
/// an option followed only by spacing takes no value, one followed by a
/// placeholder takes one, and so does one whose description carries yargs's
/// `[string]`, `[number]`, or `[array]` type (yargs prints no placeholder).
/// Long options mentioned elsewhere are included with unknown arity.
pub fn options(text: &str) -> Vec<CompletionItem> {
    // The type, then only other tags (`[required]`, `[choices: ...]`).
    let yargs_value = regex!(r"\[(?:string|number|array)\](?:\s+\[[^\[\]]*\])*\s*$");
    // The options of the declaration being read, its indent, and those
    // yargs types as taking a value.
    let mut block: Vec<String> = Vec::new();
    let mut block_indent = 0;
    let mut typed: Vec<String> = Vec::new();
    let mut items: Vec<CompletionItem> = Vec::new();
    let mut add = |name: &str, takes_value: Option<bool>| {
        let name = name.trim_end_matches(|c: char| ",.;:)]".contains(c));
        let valid = name.len() > 1
            && name.starts_with('-')
            && name != "--"
            && name
                .chars()
                .nth(if name.starts_with("--") { 2 } else { 1 })
                .is_some_and(|c| c.is_ascii_alphanumeric() || "@%".contains(c))
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_@%".contains(c));
        if !valid {
            return;
        }
        match items.iter_mut().find(|i| i.value == name) {
            Some(existing) => {
                if existing.takes_value.is_none() {
                    existing.takes_value = takes_value;
                }
            }
            None => items.push(CompletionItem {
                value: name.to_owned(),
                takes_value,
                description: None,
            }),
        }
    };
    for line in text.lines() {
        if let Some(head) = option_declaration(line) {
            // `-l, --long=VALUE   Description` or `-D format`.
            let declarations: Vec<_> = aliases(head)
                .into_iter()
                .map(|part| {
                    let (name, rest) = part.split_once([' ', '=', '[']).unwrap_or((part, ""));
                    (
                        name,
                        !part[name.len()..].starts_with('[')
                            && (part.contains('=') || !rest.trim().is_empty()),
                    )
                })
                .collect();
            // A placeholder on the long spelling applies to its short alias.
            let takes_value = Some(declarations.iter().any(|(_, valued)| *valued));
            block = declarations
                .iter()
                .map(|(name, _)| (*name).to_owned())
                .collect();
            block_indent = line.len() - line.trim_start().len();
            for (name, _) in declarations {
                add(name, takes_value);
            }
        } else if line.trim().is_empty() || line.len() - line.trim_start().len() <= block_indent {
            block.clear();
        }
        if yargs_value.is_match(line) {
            typed.extend(block.iter().cloned());
        }
        for word in line.split(|c: char| c.is_whitespace() || "[]()'\"`".contains(c)) {
            if word.starts_with("--") {
                add(word.split('=').next().unwrap_or(word), None);
            }
        }
    }
    for item in &mut items {
        if typed.contains(&item.value) {
            item.takes_value = Some(true);
        }
    }
    items
}

/// Finite option choices explicitly documented as `{json,yaml}` or
/// `[possible values: json, yaml]` / `[choices: json, yaml]` (yargs quotes
/// strings: `[choices: "json", "yaml"]`). Descriptive
/// prose and arbitrary placeholders are not a vocabulary of values.
pub fn option_values(text: &str, option: &str) -> Vec<CompletionItem> {
    let choices = regex!(r"(?i)\[(?:possible values|choices):\s*([^\[\]]+)\]");
    let braces = regex!(r"\{([^{}]+)\}");
    let mut active_indent = None;
    let mut values = Vec::new();
    for line in text.lines() {
        let declaration = option_declaration(line);
        if let Some(head) = declaration {
            active_indent = aliases(head)
                .iter()
                .any(|part| part.split([' ', '=', '[']).next() == Some(option))
                .then_some(line.len() - line.trim_start().len());
        } else if line.trim().is_empty()
            || active_indent.is_some_and(|indent| line.len() - line.trim_start().len() <= indent)
        {
            active_indent = None;
        }
        if active_indent.is_none() {
            continue;
        }
        let list = choices.captures(line).map(|c| c[1].to_owned()).or_else(|| {
            declaration.and_then(|head| braces.captures(head).map(|c| c[1].to_owned()))
        });
        let Some(list) = list else { continue };
        // yargs quotes string choices: `[choices: "never", "any-change"]`.
        let words: Vec<_> = list
            .split([',', '|'])
            .map(str::trim)
            .map(|w| {
                w.strip_prefix('"')
                    .and_then(|w| w.strip_suffix('"'))
                    .unwrap_or(w)
            })
            .collect();
        // Broken or shell-shaped metadata must not introduce executable text.
        if words.is_empty()
            || words.iter().any(|w| {
                w.is_empty()
                    || !w
                        .chars()
                        .all(|c| c.is_alphanumeric() || "-_.:@%+/".contains(c))
            })
        {
            continue;
        }
        for word in words {
            if !values
                .iter()
                .any(|item: &CompletionItem| item.value == word)
            {
                values.push(CompletionItem {
                    value: word.to_owned(),
                    takes_value: None,
                    description: None,
                });
            }
        }
    }
    values
}

/// Subcommands listed under a `Commands:` style heading in help output.
/// Some lists repeat the program before each command: Thor's (Ruby)
/// `  bundle install [OPTIONS]  # Description` under `Bundler commands:`,
/// and swift's `  swift build      Build Swift packages`. When every entry
/// starts with `program`'s name, the second word is the command.
pub fn subcommands(help: &str, program: &str) -> Vec<CompletionItem> {
    let program = program.rsplit(['/', '\\']).next().unwrap_or(program);
    let heading = regex!(
        r"(?i)^\s*(?:available |core |other |management |additional )?(?:sub)?commands?:?\s*$"
    );
    let thor_heading = regex!(r"^[A-Z][A-Za-z0-9_-]* commands:\s*$");
    let entry = regex!(r"^\s{1,8}([a-z][a-z0-9_-]*)(?:,\s*[a-z][a-z0-9_-]*)*(?:\s{2,}|\s*$)");
    let prefixed = regex!(r"^\s{1,8}([a-z][a-z0-9_.-]*) ([a-z][a-z0-9_:-]*)(?:\s|$)");
    let mut items: Vec<CompletionItem> = Vec::new();
    let mut pairs: Vec<(String, String)> = Vec::new();
    let mut in_section = false;
    for line in help.lines() {
        if heading.is_match(line) || thor_heading.is_match(line) {
            in_section = true;
            continue;
        }
        if !in_section {
            continue;
        }
        if line.trim().is_empty() {
            continue;
        }
        if !line.starts_with(char::is_whitespace) {
            in_section = heading.is_match(line) || thor_heading.is_match(line);
            continue;
        }
        if let Some(caps) = prefixed.captures(line) {
            pairs.push((caps[1].to_owned(), caps[2].to_owned()));
        }
        if let Some(caps) = entry.captures(line)
            && !items.iter().any(|i| i.value == caps[1])
        {
            items.push(CompletionItem {
                value: caps[1].to_owned(),
                takes_value: None,
                description: None,
            });
        }
    }
    // Entries must name the program; other leading words are aliases,
    // nested commands (`db migrate`), or prose.
    pairs.retain(|(word, _)| word == program);
    if pairs.len() >= 2 {
        items.clear();
        for (_, command) in pairs {
            if !items.iter().any(|i| i.value == command) {
                items.push(CompletionItem {
                    value: command,
                    takes_value: None,
                    description: None,
                });
            }
        }
    }
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(items: &[CompletionItem]) -> Vec<&str> {
        items.iter().map(|i| i.value.as_str()).collect()
    }

    #[test]
    fn reads_bsd_and_gnu_style_man_pages() {
        let bsd = "LS(1)\n\nS\x08SY\x08YN\x08NO\x08OP\x08PS\x08SI\x08IS\x08S\n     ls [-\x08-@\x08@A\x08A] [-\x08--\x08-c\x08co\x08ol\x08lo\x08or\x08r=_\x08w_\x08h_\x08e_\x08n]\n\n     -\x08-A\x08A      Include dot entries.\n\n     -\x08-D\x08D _\x08f_\x08o_\x08r_\x08m_\x08a_\x08t\n             Format dates.\n";
        let text = strip_formatting(bsd);
        assert!(text.contains("[--color=when]"), "{text}");
        let items = options(&text);
        assert_eq!(names(&items), ["--color", "-A", "-D"]);
        assert_eq!(items[1].takes_value, Some(false));
        assert_eq!(items[2].takes_value, Some(true));
        let gnu = "  -a, --all                  do not ignore entries starting with .\n      --block-size=SIZE      with -l, scale sizes\n      --color[=WHEN]         color the output WHEN\n  -I, --ignore=PATTERN       do not list implied entries\n  see --help-all.\n";
        let items = options(gnu);
        assert_eq!(
            names(&items),
            [
                "-a",
                "--all",
                "--block-size",
                "--color",
                "-I",
                "--ignore",
                "--help-all"
            ]
        );
        let color = items.iter().find(|i| i.value == "--color").unwrap();
        assert_eq!(
            color.takes_value,
            Some(false),
            "an optional value is attached"
        );
        assert_eq!(items[2].takes_value, Some(true));
    }

    #[test]
    fn reads_subcommands_from_help() {
        let help = "Usage: cargo [OPTIONS] [COMMAND]\n\nOptions:\n  -V, --version  Print version\n\nCommands:\n    build, b    Compile the current package\n    check, c    Analyze\n    test, t     Run the tests\n    ...         See all commands with --list\n\nSee 'cargo help <command>'\n";
        assert_eq!(
            names(&subcommands(help, "cargo")),
            ["build", "check", "test"]
        );
        let cobra = "Available Commands:\n  completion  Generate completion\n  get         Display resources\n\nFlags:\n  -h, --help   help\n";
        assert_eq!(names(&subcommands(cobra, "kubectl")), ["completion", "get"]);
    }

    #[test]
    fn reads_thor_command_lists_after_the_program_name() {
        // bundler 1.17.2's `bundle --help`.
        let bundler = "Bundler commands:\n  bundle add GEM VERSION         # Add gem to Gemfile and run bundle install\n  bundle install [OPTIONS]       # Install the current environment to the system\n  bundle plugin                  # Manage the bundler plugins\n  bundle plugin help [COMMAND]   # Describe subcommands or one specific subco...\n  bundle version                 # Prints the bundler's version information\n\nOptions:\n      [--no-color]  # Disable colorization in output\n";
        assert_eq!(
            names(&subcommands(bundler, "/usr/bin/bundle")),
            ["add", "install", "plugin", "version"]
        );
        // Lines naming different first words are not a Thor list.
        let mixed = "Commands:\n  bundle add GEM  # Add\n  rails new APP  # New\n";
        assert!(subcommands(mixed, "bundle").is_empty());
        // A list of nested commands under one group is not program-prefixed.
        let nested = "Commands:\n  db migrate   Run migrations\n  db seed      Seed data\n";
        assert!(subcommands(nested, "app").is_empty());
        // swift 6.4's help, bold even when piped.
        let swift = strip_formatting(
            "\x1b[1mSubcommands:\x1b[0m\n\n  \x1b[1mswift build\x1b[0m      Build Swift packages\n  \x1b[1mswift package\x1b[0m    Create and work on packages\n\n  Use \x1b[1m`swift --version`\x1b[0m for Swift version information.\n",
        );
        assert_eq!(names(&subcommands(&swift, "swift")), ["build", "package"]);
    }

    #[test]
    fn option_aliases_share_the_documented_value_relationship() {
        let items = options("  -o, --output <FORMAT>\tOutput format\n  -v, --verbose\tTalk\n");
        assert_eq!(names(&items), ["-o", "--output", "-v", "--verbose"]);
        assert_eq!(
            items.iter().map(|i| i.takes_value).collect::<Vec<_>>(),
            [Some(true), Some(true), Some(false), Some(false)]
        );
    }

    #[test]
    fn optional_separate_values_can_follow_cargo_selectors() {
        let items = options(
            "  -p, --package [<SPEC>]  Package to build\n      --bin [<NAME>]  Build the named binary\n      --color[=WHEN]  Color output\n",
        );
        assert_eq!(
            items
                .iter()
                .map(|item| item.takes_value)
                .collect::<Vec<_>>(),
            [Some(true), Some(true), Some(true), Some(false)]
        );
    }

    #[test]
    fn reads_only_explicit_choices_for_the_requested_option() {
        let help = "Options:\n  -o, --output <FORMAT>  Output\n                        [possible values: json, yaml]\n  --color {auto,always,never}  Color\n  --mode <MODE>  Mode [choices: local, remote]\n  --path <PATH>  A path such as local or remote\n";
        assert_eq!(names(&option_values(help, "--output")), ["json", "yaml"]);
        assert_eq!(names(&option_values(help, "-o")), ["json", "yaml"]);
        assert_eq!(
            names(&option_values(help, "--color")),
            ["auto", "always", "never"]
        );
        assert_eq!(names(&option_values(help, "--mode")), ["local", "remote"]);
        assert!(option_values(help, "--path").is_empty());
        assert!(option_values(help, "--absent").is_empty());
        // yargs types say which options take a value, here or below.
        let typed = "  -v, --verbose   Show debug logs  [count]\n  --profile       Use the profile  [string]\n  --require-approval   What needs approval\n             [string] [choices: \"never\"]\n  --json  JSON  [boolean] [default: false]\n  --tags  Tags   [array]\n  --notes  Mentions [string] in prose\n";
        let items = options(typed);
        let arity = |name: &str| items.iter().find(|i| i.value == name).unwrap().takes_value;
        assert_eq!(arity("--verbose"), Some(false));
        assert_eq!(arity("-v"), Some(false));
        assert_eq!(arity("--profile"), Some(true));
        assert_eq!(arity("--require-approval"), Some(true));
        assert_eq!(arity("--json"), Some(false));
        assert_eq!(arity("--tags"), Some(true));
        assert_eq!(arity("--notes"), Some(false), "prose isn't a type tag");
        // yargs (aws-cdk 2.1144.0): quoted strings on a continuation line.
        let yargs = "  --require-approval                        What changes require manual approval\n                         [string] [choices: \"never\", \"any-change\", \"broadening\"]\n  --notification-arns                       ARNs\n";
        assert_eq!(
            names(&option_values(yargs, "--require-approval")),
            ["never", "any-change", "broadening"]
        );
        for list in ["\"a,b\"", "\"a\"b\"", "\"$(id)\""] {
            assert!(
                option_values(&format!("  --x <X>  [choices: {list}]\n"), "--x").is_empty(),
                "{list}"
            );
        }
    }

    #[test]
    fn ignores_malformed_or_shell_shaped_help_choices() {
        for list in [
            "json,",
            "json;touch marker",
            "$(touch marker)",
            "json, `id`",
            "json, \u{1b}[31myaml",
        ] {
            assert!(
                option_values(
                    &format!("  --output <FORMAT>  [possible values: {list}]\n"),
                    "--output"
                )
                .is_empty(),
                "{list}"
            );
        }
    }

    #[test]
    fn choice_lists_do_not_define_aliases_or_belong_to_options_mentioned_in_prose() {
        let help = "  -m, --mode {fast,--other,slow}  Mode [possible values: fast, slow]\n  --optional[=one,two words]  Optional\n  --path <PATH>  Unlike --mode [possible values: local, remote]\n";
        let items = options(help);
        assert!(
            !names(&items).contains(&"--other"),
            "an enum word is not an option"
        );
        assert_eq!(
            items
                .iter()
                .find(|i| i.value == "--optional")
                .unwrap()
                .takes_value,
            Some(false)
        );
        assert_eq!(names(&option_values(help, "--mode")), ["fast", "slow"]);
        assert_eq!(names(&option_values(help, "-m")), ["fast", "slow"]);
        let help = "  --output <FORMAT>  Format\n  Arguments:\n    FILE  [possible values: local, remote]\n";
        assert!(
            option_values(help, "--output").is_empty(),
            "another section's choices do not describe the option"
        );
    }
}
