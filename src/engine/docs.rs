//! Reading options and subcommands from local documentation: formatted man
//! pages and `--help` output. Documentation can lag the installed program,
//! so what it lists is never treated as complete.

use super::native::CompletionItem;

/// Removes terminal overstrike formatting (`X\bX` bold, `_\bX` underline).
pub fn strip_overstrike(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
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
/// placeholder takes one. Long options mentioned elsewhere are included with
/// unknown arity.
pub fn options(text: &str) -> Vec<CompletionItem> {
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
            }),
        }
    };
    for line in text.lines() {
        let trimmed = line.trim_start();
        if line.len() > trimmed.len() && trimmed.starts_with('-') {
            // `-l, --long=VALUE   Description` or `-D format`.
            let head = trimmed.split("  ").next().unwrap_or(trimmed);
            for part in head.split([',', '|']) {
                let part = part.trim();
                let (name, rest) = part.split_once([' ', '=', '[']).unwrap_or((part, ""));
                let takes_value = if part.contains('[') {
                    Some(false)
                } else if part.contains('=') || !rest.trim().is_empty() {
                    Some(true)
                } else {
                    Some(false)
                };
                add(name, takes_value);
            }
        }
        for word in line.split(|c: char| c.is_whitespace() || "[]()'\"`".contains(c)) {
            if word.starts_with("--") {
                add(word.split('=').next().unwrap_or(word), None);
            }
        }
    }
    items
}

/// Subcommands listed under a `Commands:` style heading in help output.
pub fn subcommands(help: &str) -> Vec<CompletionItem> {
    let heading = regex!(
        r"(?i)^\s*(?:available |core |other |management |additional )?(?:sub)?commands?:?\s*$"
    );
    let entry = regex!(r"^\s{1,8}([a-z][a-z0-9_-]*)(?:,\s*[a-z][a-z0-9_-]*)*(?:\s{2,}|\s*$)");
    let mut items: Vec<CompletionItem> = Vec::new();
    let mut in_section = false;
    for line in help.lines() {
        if heading.is_match(line) {
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
            in_section = heading.is_match(line);
            continue;
        }
        if let Some(caps) = entry.captures(line)
            && !items.iter().any(|i| i.value == caps[1])
        {
            items.push(CompletionItem {
                value: caps[1].to_owned(),
                takes_value: None,
            });
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
        let text = strip_overstrike(bsd);
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
        assert_eq!(names(&subcommands(help)), ["build", "check", "test"]);
        let cobra = "Available Commands:\n  completion  Generate completion\n  get         Display resources\n\nFlags:\n  -h, --help   help\n";
        assert_eq!(names(&subcommands(cobra)), ["completion", "get"]);
    }
}
