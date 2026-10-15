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
        // Hierarchical long options (traefik's `--log.level`) keep their
        // dots; a trailing one ends a sentence and is already trimmed.
        let valid = name.len() > 1
            && name.starts_with('-')
            && name != "--"
            && name
                .chars()
                .nth(if name.starts_with("--") { 2 } else { 1 })
                .is_some_and(|c| c.is_ascii_alphanumeric() || "@%".contains(c))
            && name.chars().all(|c| {
                c.is_ascii_alphanumeric()
                    || "-_@%".contains(c)
                    || c == '.' && name.starts_with("--")
                    // FFmpeg's stream specifiers: `-c:v`.
                    || c == ':'
            })
            && !name.contains("..")
            && !name.contains("::");
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
            let declarations: Vec<(String, bool)> = aliases(head)
                .into_iter()
                .flat_map(|part| {
                    // kingpin's `--[no-]name`: a boolean with both spellings.
                    if let Some(negatable) = part.strip_prefix("--[no-]") {
                        let name = negatable.split([' ', '=']).next().unwrap_or(negatable);
                        return vec![
                            (format!("--{name}"), false),
                            (format!("--no-{name}"), false),
                        ];
                    }
                    let (name, rest) = part.split_once([' ', '=', '[']).unwrap_or((part, ""));
                    vec![(
                        name.to_owned(),
                        !part[name.len()..].starts_with('[')
                            && (part.contains('=') || !rest.trim().is_empty()),
                    )]
                })
                .collect();
            // A placeholder on the long spelling applies to its short alias.
            let takes_value = Some(declarations.iter().any(|(_, valued)| *valued));
            block = declarations.iter().map(|(name, _)| name.clone()).collect();
            block_indent = line.len() - line.trim_start().len();
            for (name, _) in &declarations {
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

/// A list closing an option's description that names its placeholder
/// (nginx's `-s signal : send signal to a master process: stop, quit,
/// reopen, reload`). Examples (`e.g.`, `such as`) are not choices.
fn prose_choices(description: &str, placeholder: &str) -> Option<String> {
    let (before, list) = description.trim_end_matches('.').rsplit_once(':')?;
    let placeholder = placeholder
        .trim_matches(|c: char| "<>[]".contains(c))
        .to_lowercase();
    let named = placeholder.len() > 1
        && placeholder
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
        && before
            .to_lowercase()
            .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
            .any(|word| word == placeholder);
    if !named || regex!(r"(?i)\b(?:e\.g|for example|such as|example)\b").is_match(before) {
        return None;
    }
    let list = regex!(r",?\s+(?:or|and)\s+").replace(list.trim(), ", ");
    let words: Vec<&str> = list.split(',').map(str::trim).collect();
    (words.len() > 1
        && words.iter().all(|word| {
            !word.is_empty()
                && word
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-_".contains(c))
        }))
    .then(|| words.join(","))
}

/// The short spelling a man page documents as printing help or usage
/// (nginx's `-?, -h  Print help.`), for programs that reject `--help`. A
/// spelling with a value, or described otherwise (varnishd's `-h <type>`
/// picks a hash algorithm), is not one.
pub fn documented_help_flag(page: &str) -> Option<&'static str> {
    const FLAGS: [&str; 3] = ["-h", "-?", "-help"];
    // httpd: "Output a short summary of available command line options."
    let help = regex!(r"(?i)\b(?:help|usage)\b|\b(?:summary|list) of (?:\w+ ){0,3}options\b");
    let lines: Vec<&str> = page.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        let Some(head) = option_declaration(line) else {
            continue;
        };
        let names = aliases(head);
        let Some(flag) = FLAGS.iter().find(|flag| names.contains(flag)) else {
            continue;
        };
        // The description follows on the same line or, indented, below.
        let trimmed = line.trim_start();
        let mut description = trimmed[head.len()..].trim().to_owned();
        if description.is_empty() {
            let indent = line.len() - trimmed.len();
            for next in lines[i + 1..]
                .iter()
                .take_while(|next| next.len() - next.trim_start().len() > indent)
                .take(3)
            {
                description.push(' ');
                description.push_str(next.trim());
            }
        }
        if help.is_match(&description) {
            return Some(flag);
        }
    }
    None
}

/// Finite option choices explicitly documented as `{json,yaml}` or
/// `[possible values: json, yaml]` / `[choices: json, yaml]` (yargs quotes
/// strings: `[choices: "json", "yaml"]`). Descriptive
/// prose and arbitrary placeholders are not a vocabulary of values.
pub fn option_values(text: &str, option: &str) -> Vec<CompletionItem> {
    let choices = regex!(r"(?i)\[(?:possible values|choices):\s*([^\[\]]+)\]");
    let braces = regex!(r"\{([^{}]+)\}");
    // A synopsis's alternatives: httpd's `[-k start|restart|stop]`.
    let synopsis =
        regex!(r"\[(-{1,2}[A-Za-z][A-Za-z0-9-]*)[ =]([a-z][a-z0-9-]*(?:\|[a-z][a-z0-9-]*)+)\]");
    let mut active_indent = None;
    let mut values = Vec::new();
    for line in text.lines() {
        for caps in synopsis.captures_iter(line) {
            if &caps[1] != option {
                continue;
            }
            for word in caps[2].split('|') {
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
        let list = choices
            .captures(line)
            .map(|c| c[1].to_owned())
            .or_else(|| declaration.and_then(|head| braces.captures(head).map(|c| c[1].to_owned())))
            .or_else(|| {
                let head = declaration?;
                let placeholder = aliases(head)
                    .into_iter()
                    .find_map(|part| part.strip_prefix(option)?.strip_prefix([' ', '=']))?;
                prose_choices(line.trim_start()[head.len()..].trim(), placeholder)
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
/// starts with `program`'s name, the second word is the command. Aliases
/// are commands too: after commas (`build, b`) or in parentheses, as
/// argparse prints subparsers (borgmatic's `repo-list (rlist)` under
/// `actions:`). An underlined heading may list entries at the margin
/// (direnv's `allow [PATH_TO_RC]:`). Without such a section, usage lines
/// that each name the program and a command (duplicity, tmutil) list them.
pub fn subcommands(help: &str, program: &str) -> Vec<CompletionItem> {
    subcommands_at(help, program, &[])
}

/// A nested usage names the whole command path. Remove the already bound
/// words before reading the next documented level (qsv cat rows/columns).
pub(crate) fn subcommands_at(help: &str, program: &str, path: &[String]) -> Vec<CompletionItem> {
    let program = program.rsplit(['/', '\\']).next().unwrap_or(program);
    // fastlane's `Commands: (* default)`.
    let heading = regex!(
        r"(?i)^\s*(?:available |core |other |management |additional )?(?:sub)?(?:commands?|actions?):?(?:\s*\([^)]*\))?\s*$"
    );
    let thor_heading = regex!(r"^[A-Z][A-Za-z0-9_-]* commands:\s*$");
    // buildkite-agent's `Available commands are:` and `Commands that can be
    // run within a Buildkite job:`.
    let phrase_heading = regex!(r"(?i)^(?:[a-z][^:]*\s)?commands\b[^:]*:\s*$");
    // Bundler 4 answers --help with its man page: `PRIMARY COMMANDS`,
    // `UTILITIES`, and entries such as `bundle install(1) bundle-install.1.html`.
    let man_heading = regex!(r"^(?:[A-Z][A-Z ]* )?(?:COMMANDS|UTILITIES)\s*$");
    let is_heading = |line: &str| {
        heading.is_match(line)
            || thor_heading.is_match(line)
            || phrase_heading.is_match(line)
            || man_heading.is_match(line)
    };
    let underline = regex!(r"^[-=]{3,}\s*$");
    // Also commander.js's `migrate:make|mm [options] <name>   Description`:
    // `:` in names, `|` before an alias, and argument placeholders.
    // CocoaPods (CLAide) marks commands with subcommands `+ cache`; yarn 1
    // bullets them `- access` (an option has no space after its dash).
    let entry = regex!(
        r"^\s{1,8}(?:[+>-]\s+)?([a-z][a-z0-9_:-]*(?:[,|]\s*[a-z][a-z0-9_:-]*)*)(?:\s+\(([^)]*)\))?(?:\s+(?:\[[^\]]*\]|<[^>]*>|\.\.\.))*(?:\s{2,}|\s*$)"
    );
    // argparse names its subparsers `{branches,check,...}` (alembic).
    let choices = regex!(r"^\s{1,8}\{([a-z0-9_-]+(?:,[a-z0-9_-]+)+)\}\s*$");
    let margin_entry = regex!(r"^([a-z][a-z0-9_-]*)(?:\s[^:]*)?:\s*$");
    let prefixed = regex!(r"^\s{1,8}([a-z][a-z0-9_.-]*) ([a-z][a-z0-9_:-]*)(?:\(\d+\))?(?:\s|$)");
    let word = regex!(r"^[a-z][a-z0-9_:-]*$");
    // openssl's `Standard commands` and `Cipher commands (see ...)`: names
    // in columns at the margin.
    let column_heading = regex!(r"^[A-Z][A-Za-z ]* commands(?: \(.*\))?\s*$");
    let column_name = regex!(r"^[a-z0-9][a-z0-9_.-]*$");
    // certbot's `--help commands`: `The full list of available SUBCOMMANDS
    // is:`, then names at the margin. Wrapped descriptions continue at the
    // margin too, but alone or as options.
    let listing_heading = regex!(r"(?i)\bsubcommands (?:is|are):\s*$");
    let listing_entry = regex!(r"^([a-z][a-z0-9_-]*)(?:\s+\([a-z]+\))?\s{2,}\S");
    let mut listing: Option<usize> = None;
    // vegeta's per-command flag sections: `attack command:`.
    let command_section = regex!(r"^([a-z][a-z0-9_-]*) command:\s*$");
    // diskutil's camelCase verbs with optional parts: `info[rmation]`,
    // `u[n]mount`, `rename[Volume]`, `apfs <verb>`, each described in
    // parentheses below `where <verb> is as follows:`.
    let verb_heading = regex!(r"where <verb> is as follows:\s*$");
    let verb_entry = regex!(
        r"^\s{2,8}([a-z][A-Za-z0-9]*)(?:\[([A-Za-z]+)\]([A-Za-z0-9]*))?\s+(?:<[a-z]+>\s+)?\("
    );
    let mut verbs = false;
    let mut columns = false;
    // uutils and BusyBox document their compiled-in applets in a wrapped,
    // comma-separated list. Read that installed list, never a tool catalog.
    let mut functions = false;
    let mut items: Vec<CompletionItem> = Vec::new();
    let add = |items: &mut Vec<CompletionItem>, value: &str| {
        if !items.iter().any(|i| i.value == value) {
            items.push(CompletionItem {
                value: value.to_owned(),
                takes_value: None,
                description: None,
            });
        }
    };
    let mut pairs: Vec<(String, String)> = Vec::new();
    let mut in_section = false;
    // The heading was underlined, so entries may start at the margin.
    let mut margin = false;
    let mut first = false;
    for line in help.lines() {
        if line.trim() == "Currently defined functions:" {
            functions = true;
            continue;
        }
        if functions {
            if line.trim().is_empty() {
                continue;
            }
            if line.starts_with(char::is_whitespace) {
                for name in line.split(',').map(str::trim) {
                    if word.is_match(name) {
                        add(&mut items, name);
                    }
                }
                continue;
            }
            functions = false;
        }
        if listing_heading.is_match(line) {
            listing = Some(0);
            continue;
        }
        if let Some(listed) = &mut listing {
            if let Some(caps) = listing_entry.captures(line) {
                add(&mut items, &caps[1]);
                *listed += 1;
            } else if line.trim().is_empty() && *listed > 0 {
                listing = None;
            }
            continue;
        }
        if verb_heading.is_match(line) {
            verbs = true;
            continue;
        }
        if verbs {
            if let Some(caps) = verb_entry.captures(line) {
                let tail = caps.get(3).map_or("", |m| m.as_str());
                add(&mut items, &format!("{}{tail}", &caps[1]));
                if let Some(optional) = caps.get(2) {
                    add(
                        &mut items,
                        &format!("{}{}{tail}", &caps[1], optional.as_str()),
                    );
                }
                continue;
            }
            if line.trim().is_empty() || line.starts_with(char::is_whitespace) {
                continue;
            }
            verbs = false;
        }
        if let Some(caps) = command_section.captures(line) {
            add(&mut items, &caps[1]);
            in_section = false;
            continue;
        }
        if let Some(caps) = choices.captures(line) {
            for name in caps[1].split(',') {
                add(&mut items, name);
            }
            continue;
        }
        if column_heading.is_match(line) && !is_heading(line) {
            columns = true;
            in_section = false;
            continue;
        }
        if columns {
            let names: Vec<&str> = line.split_whitespace().collect();
            if !line.starts_with(char::is_whitespace)
                && !names.is_empty()
                && names.iter().all(|name| column_name.is_match(name))
            {
                for name in names {
                    add(&mut items, name);
                }
                continue;
            }
            columns = line.trim().is_empty();
            if columns {
                continue;
            }
        }
        if is_heading(line) {
            in_section = true;
            margin = false;
            first = true;
            continue;
        }
        if !in_section {
            continue;
        }
        if line.trim().is_empty() {
            continue;
        }
        if std::mem::take(&mut first) && underline.is_match(line) {
            margin = true;
            continue;
        }
        if !line.starts_with(char::is_whitespace) {
            if margin && let Some(caps) = margin_entry.captures(line) {
                add(&mut items, &caps[1]);
                continue;
            }
            in_section = is_heading(line);
            continue;
        }
        if let Some(caps) = prefixed.captures(line) {
            pairs.push((caps[1].to_owned(), caps[2].to_owned()));
        }
        if let Some(caps) = entry.captures(line) {
            let aliases = caps.get(2).map_or("", |aliases| aliases.as_str());
            for name in caps[1]
                .split([',', '|'])
                .chain(aliases.split(','))
                .map(str::trim)
            {
                if word.is_match(name) {
                    add(&mut items, name);
                }
            }
        }
    }
    // Entries must name the program, or the command whose help this is
    // (garden's `get status` under `garden get`, `cloud secrets list` under
    // `garden cloud`); other leading words are aliases, nested commands
    // (`db migrate`), or prose.
    pairs.retain(|(word, _)| word == program || path.last() == Some(word));
    if pairs.len() >= 2 {
        items.clear();
        for (_, command) in pairs {
            add(&mut items, &command);
        }
    }
    if items.is_empty() {
        items = usage_commands(help, program, path);
    }
    items
}

/// Commands named by usage lines (`usage: duplicity backup [options] ...`
/// and its indented continuations, or tmutil's repeated `Usage: tmutil
/// addexclusion ...`). At least two different commands must appear, so one
/// usage line's first argument isn't taken for a command.
fn usage_commands(help: &str, program: &str, path: &[String]) -> Vec<CompletionItem> {
    let usage = regex!(r"(?i)^\s*usage:\s*(.*)$");
    let command = regex!(r"^(\S+)\s+([a-z][a-z0-9_-]*)(?:\s|$)");
    // wg-quick's `Usage: wg-quick [ up | down | save | strip ] [ ... ]`.
    let choice =
        regex!(r"^(\S+)\s+[\[{]\s*([a-z][a-z0-9_-]*(?:\s*\|\s*[a-z][a-z0-9_-]*)+)\s*[\]}]");
    let mut names: Vec<String> = Vec::new();
    let mut in_usage = false;
    for line in help.lines() {
        let text = if let Some(caps) = usage.captures(line) {
            in_usage = true;
            caps.get(1).map_or("", |text| text.as_str())
        } else if in_usage && line.starts_with(char::is_whitespace) && !line.trim().is_empty() {
            line.trim_start()
        } else {
            in_usage = false;
            continue;
        };
        let normalized;
        let text = if path.is_empty() {
            text
        } else {
            let Some((app, mut rest)) = text.split_once(char::is_whitespace) else {
                continue;
            };
            if app.rsplit(['/', '\\']).next() != Some(program) {
                continue;
            }
            let mut matches = true;
            for word in path {
                let Some(tail) = rest
                    .trim_start()
                    .strip_prefix(word.as_str())
                    .filter(|tail| tail.is_empty() || tail.starts_with(char::is_whitespace))
                else {
                    matches = false;
                    break;
                };
                rest = tail;
            }
            if !matches {
                continue;
            }
            normalized = format!("{program} {}", rest.trim_start());
            &normalized
        };
        if let Some(caps) = choice.captures(text)
            && caps[1].rsplit(['/', '\\']).next() == Some(program)
        {
            for name in caps[2].split('|').map(str::trim) {
                if !names.iter().any(|known| known == name) {
                    names.push(name.to_owned());
                }
            }
        } else if let Some(caps) = command.captures(text)
            && caps[1].rsplit(['/', '\\']).next() == Some(program)
            && !names.iter().any(|name| *name == caps[2])
        {
            names.push(caps[2].to_owned());
        }
    }
    if names.len() < 2 {
        return Vec::new();
    }
    names
        .into_iter()
        .map(|value| CompletionItem {
            value,
            takes_value: None,
            description: None,
        })
        .collect()
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
    fn hierarchical_long_options_keep_their_dots() {
        // traefik 3.7's `--help`: each option on its own line, its
        // description below.
        let traefik = "Flags:\n    --log  (Default: \"false\")\n        Traefik log settings.\n\n    --log.level  (Default: \"ERROR\")\n        Log level set to traefik logs.\n\n    --providers.docker.endpoint  (Default: \"unix:///var/run/docker.sock\")\n        Docker server endpoint. Can be a TCP or a Unix socket endpoint.\n\nSee --help.\n";
        assert_eq!(
            names(&options(traefik)),
            [
                "--log",
                "--log.level",
                "--providers.docker.endpoint",
                "--help"
            ]
        );
        // Short options and ellipses never take dots.
        assert_eq!(
            names(&options("  -x.y  bad\n  --a..b  bad\n")),
            [] as [&str; 0]
        );
    }

    #[test]
    fn short_help_flags_are_taken_only_from_their_documented_meaning() {
        // nginx 1.31's and varnishadm 9.1's pages (BSD mandoc layout).
        let nginx = "SYNOPSIS\n     nginx [-?hqTtVv] [-c file]\n\n     -?, -h\t\t  Print help.\n\n     -c file\t\t  Use an alternative configuration file.\n";
        assert_eq!(documented_help_flag(nginx), Some("-h"));
        let varnishadm = "     -h     Print program usage and exit.\n\n     -n workdir\n";
        assert_eq!(documented_help_flag(varnishadm), Some("-h"));
        let httpd = "     -h     Output a short summary of available command line options.\n\n     -l     Output a list of modules compiled into the server.\n";
        assert_eq!(documented_help_flag(httpd), Some("-h"));
        // varnishd's -h selects a hash algorithm; its -? prints usage on
        // the next line.
        let varnishd = "     -?\n            Print the usage message.\n\n     -h <type[,options]>\n            Specifies the hash algorithm.\n";
        assert_eq!(documented_help_flag(varnishd), Some("-?"));
        assert_eq!(
            documented_help_flag("     -h <type>\n            Specifies the hash algorithm.\n"),
            None
        );
        assert_eq!(
            documented_help_flag("     -h     Human-readable sizes.\n     -x     See help.\n"),
            None
        );
        // `ls -h` (human-readable) is never asked, even mentioned near help.
        assert_eq!(
            documented_help_flag(
                "     -h      Use unit suffixes.\n\n     Run with --help for help.\n"
            ),
            None
        );
    }

    #[test]
    fn reads_subcommands_from_help() {
        let help = "Usage: cargo [OPTIONS] [COMMAND]\n\nOptions:\n  -V, --version  Print version\n\nCommands:\n    build, b    Compile the current package\n    check, c    Analyze\n    test, t     Run the tests\n    ...         See all commands with --list\n\nSee 'cargo help <command>'\n";
        assert_eq!(
            names(&subcommands(help, "cargo")),
            ["build", "b", "check", "c", "test", "t"],
            "aliases are commands too"
        );
        let cobra = "Available Commands:\n  completion  Generate completion\n  get         Display resources\n\nFlags:\n  -h, --help   help\n";
        assert_eq!(names(&subcommands(cobra, "kubectl")), ["completion", "get"]);
    }

    #[test]
    fn reads_multicall_function_lists_without_turning_flags_or_prose_into_applets() {
        let help = "uu-coreutils 0.12.0 (multi-call binary)\n\nOptions:\n      --list    lists all defined functions, one per row\n\nCurrently defined functions:\n\n    [, cat, cp,\n    ls, rm, sort, --bogus, $(touch marker)\n\nExamples:\n    echo --list\n";
        assert_eq!(
            names(&subcommands(help, "uu-coreutils")),
            ["cat", "cp", "ls", "rm", "sort"]
        );
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
        // garden 0.14.20 repeats the bound command, even two levels up.
        let words = |path: &[&str]| path.iter().map(|w| w.to_string()).collect::<Vec<_>>();
        assert_eq!(
            names(&subcommands_at(nested, "app", &words(&["db"]))),
            ["migrate", "seed"]
        );
        let cloud = "COMMANDS\n  cloud secrets list    List secrets.\n  cloud secrets create  Create secrets.\n  cloud users list      List users.\n";
        assert_eq!(
            names(&subcommands_at(cloud, "garden", &words(&["cloud"]))),
            ["secrets", "users"]
        );
        // swift 6.4's help, bold even when piped.
        let swift = strip_formatting(
            "\x1b[1mSubcommands:\x1b[0m\n\n  \x1b[1mswift build\x1b[0m      Build Swift packages\n  \x1b[1mswift package\x1b[0m    Create and work on packages\n\n  Use \x1b[1m`swift --version`\x1b[0m for Swift version information.\n",
        );
        assert_eq!(names(&subcommands(&swift, "swift")), ["build", "package"]);
    }

    #[test]
    fn reads_argparse_actions_margin_entries_and_usage_lines() {
        // borgmatic 2.1.9: argparse subparsers titled `actions`.
        let borgmatic = "options:\n  -h, --help  show help\n\nactions:\n                        Specify zero or more actions.\n    repo-create (rcreate, init, -I)\n                        Create a new, empty Borg repository\n    prune (-p)          Prune archives\n    repo-list (rlist)   List repository\n    borg                Run an arbitrary Borg command\n";
        assert_eq!(
            names(&subcommands(borgmatic, "borgmatic")),
            [
                "repo-create",
                "rcreate",
                "init",
                "prune",
                "repo-list",
                "rlist",
                "borg"
            ]
        );
        // direnv 2.37.1: an underlined heading, entries at the margin.
        let direnv = "direnv v2.37.1\nUsage: direnv COMMAND [...ARGS]\n\nAvailable commands\n------------------\nallow [PATH_TO_RC]:\npermit [PATH_TO_RC]:\n  Grants direnv permission to load the given .envrc or .env file.\nexec DIR COMMAND [...ARGS]:\n  Executes a command\nprune:\n  Removes old allowed files\n";
        assert_eq!(
            names(&subcommands(direnv, "direnv")),
            ["allow", "permit", "exec", "prune"]
        );
        // Without an underline, a margin line still ends the section.
        let plain = "Commands:\n  build  Build\noptions:\n  -v  verbose\n";
        assert_eq!(names(&subcommands(plain, "tool")), ["build"]);
        // duplicity 3.2.1 and macOS's tmutil.
        let duplicity = "duplicity 3.2.1\n\nusage: duplicity backup [options] source_path target_url\n       duplicity cleanup [options] target_url\n       duplicity remove-older-than [options] remove_time target_url\n\noptions:\n  -h, --help  Show this help\n";
        assert_eq!(
            names(&subcommands(duplicity, "/opt/homebrew/bin/duplicity")),
            ["backup", "cleanup", "remove-older-than"]
        );
        let tmutil = "Usage: tmutil addexclusion [-p|-v] item ...\n\nUsage: tmutil compare [-@acd] [-D depth]\n       tmutil compare [-@acd] snapshot_path\n\nUsage: tmutil delete [-d backup_mount_point -t timestamp] [-p path]\n";
        assert_eq!(
            names(&subcommands(tmutil, "tmutil")),
            ["addexclusion", "compare", "delete"]
        );
        let wg = "Usage: wg-quick [ up | down | save | strip ] [ CONFIG_FILE | INTERFACE ]\n\n  CONFIG_FILE is a configuration file\n";
        assert_eq!(
            names(&subcommands(wg, "wg-quick")),
            ["up", "down", "save", "strip"]
        );
        // Placeholders after the program, a single usage line, and another
        // program's usage are not command lists.
        for (help, program) in [
            (
                "usage: ln [-Ffhinsv] source_file [target_file]\n       ln [-Ffhinsv] source_file ... target_dir\n",
                "ln",
            ),
            ("Usage: tool file\n", "tool"),
            ("usage: other build\n       other test\n", "tool"),
        ] {
            assert!(subcommands(help, program).is_empty(), "{help}");
        }
    }

    #[test]
    fn reads_command_sections_under_phrase_headings() {
        // buildkite-agent 3's `--help`.
        let buildkite = "Usage:\n  buildkite-agent <command> [options...]\n\nAvailable commands are: \n  start             Starts a Buildkite agent\n  help, h           Shows a list of commands\n\nCommands that can be run within a Buildkite job:\n  annotate    Annotate the build page\n  pipeline    Make changes to the pipeline\n\nUse \"buildkite-agent <command> --help\" for more information.\n";
        assert_eq!(
            names(&subcommands(buildkite, "buildkite-agent")),
            ["start", "help", "h", "annotate", "pipeline"]
        );
    }

    #[test]
    fn reads_commander_entries_and_argparse_choice_lines() {
        // knex 3's commander help.
        let knex = "Usage: knex [options] [command]\n\nOptions:\n  -V, --version   output the version number\n\nCommands:\n  init [options]                           Create a fresh knexfile.\n  migrate:make [options] <name>            Create a named migration file.\n  migrate:latest [options]                 Run all migrations that have not yet\n                                   been run.\n  install|i [options] [packages...]        Install packages\n";
        assert_eq!(
            names(&subcommands(knex, "knex")),
            ["init", "migrate:make", "migrate:latest", "install", "i"]
        );
        // alembic 1.x's argparse help.
        let alembic = "usage: alembic [-h] {branches,check,upgrade} ...\n\npositional arguments:\n  {branches,check,upgrade}\n    branches            Show current branch points.\n    upgrade             Upgrade to a later version.\n\noptions:\n  -h, --help            show this help message and exit\n";
        assert_eq!(
            names(&subcommands(alembic, "alembic")),
            ["branches", "check", "upgrade"]
        );
    }

    #[test]
    fn reads_margin_listings_under_a_subcommands_heading() {
        // certbot 5.8's `--help commands`: wrapped descriptions continue at
        // the margin, alone or as an option.
        let certbot = "  certbot [SUBCOMMAND] [options] [-d DOMAIN] [-d DOMAIN] ...\n\ncertificate. The full list of available SUBCOMMANDS is:\n\ncertificates       List certificates managed by Certbot\nreconfigure        Update renewal configuration for a certificate specified by\n--cert-name\nrenew              Renew all certificates (or one specified with --cert-name)\nrollback           Roll back server conf changes made during certificate\ninstallation\nrun (default)      Obtain/renew a certificate, and install it\nupdate_account     Update existing account with Let's Encrypt / other ACME\nserver\n\nYou can get more help on a specific subcommand with --help SUBCOMMAND\n";
        assert_eq!(
            names(&subcommands(certbot, "certbot")),
            [
                "certificates",
                "reconfigure",
                "renew",
                "rollback",
                "run",
                "update_account"
            ]
        );
    }

    #[test]
    fn reads_camel_case_verbs_with_optional_parts() {
        // macOS diskutil's usage, printed when it runs without a verb.
        let diskutil = "Disk Utility Tool\nUsage:  diskutil [quiet] <verb> <options>, where <verb> is as follows:\n\n     list                 (List the partitions of a disk)\n     info[rmation]        (Get information on a specific disk or partition)\n\n     u[n]mount            (Unmount a single volume)\n     unmountDisk          (Unmount an entire disk (all volumes))\n     rename[Volume]       (Rename a volume)\n     apfs <verb>          (Perform additional verbs related to APFS)\n\ndiskutil <verb> with no options will provide help on that verb\n";
        assert_eq!(
            names(&subcommands(diskutil, "diskutil")),
            [
                "list",
                "info",
                "information",
                "umount",
                "unmount",
                "unmountDisk",
                "rename",
                "renameVolume",
                "apfs"
            ]
        );
    }

    #[test]
    fn reads_fastlane_and_claide_command_lists() {
        // fastlane 2's commander help and CocoaPods 1.16's CLAide help.
        let fastlane = "  fastlane\n\n  Commands: (* default)\n    action                 Shows more information\n    lanes                  Lists all available lanes\n    trigger              * Run a specific lane.\n\n  Global Options:\n    --verbose\n";
        assert_eq!(
            names(&subcommands(fastlane, "fastlane")),
            ["action", "lanes", "trigger"]
        );
        let pod = "Usage:\n\n    $ pod COMMAND\n\nCommands:\n\n    + cache         Manipulate the CocoaPods cache\n    + install       Install project dependencies according to versions from a\n                    Podfile.lock\n    + repo          Manage spec-repositories\n\nOptions:\n\n    --silent        Show nothing\n";
        assert_eq!(
            names(&subcommands(pod, "pod")),
            ["cache", "install", "repo"]
        );
        // yarn 1.22's `--help`.
        let yarn = "  Usage: yarn [command] [flags]\n\n  Options:\n\n    --cache-folder <path>   specify a cache folder\n    -v, --version           output the version number\n\n  Commands:\n    - access\n    - add\n    - run\n\n  Run `yarn help COMMAND` for more information on specific commands.\n";
        assert_eq!(names(&subcommands(yarn, "yarn")), ["access", "add", "run"]);
    }

    #[test]
    fn reads_man_page_command_sections() {
        // Bundler 4.0's `bundle --help`, its formatted man page.
        let bundle = "SYNOPSIS\n     bundle COMMAND [--no-color] [--verbose] [ARGS]\n\nPRIMARY COMMANDS\n     bundle install(1) bundle-install.1.html\n            Install the gems specified by the Gemfile\n     bundle update(1) bundle-update.1.html\n            Update dependencies\nUTILITIES\n     bundle add(1) bundle-add.1.html\n            Add the named gem to the Gemfile and run bundle install\n     bundle console(1) (deprecated)\n            Start an IRB session\n";
        assert_eq!(
            names(&subcommands(bundle, "bundle")),
            ["install", "update", "add", "console"]
        );
    }

    #[test]
    fn reads_per_command_flag_sections() {
        // vegeta 12's `--help`.
        let vegeta = "Usage: vegeta [global flags] <command> [command flags]\n\nglobal flags:\n  -cpus int\n    \tNumber of CPUs to use (default 8)\n\nattack command:\n  -body string\n    \tRequests body file\n\nencode command:\n  -to string\n\nexamples:\n  echo \"GET http://localhost/\" | vegeta attack -duration=5s | tee results.bin | vegeta report\n";
        assert_eq!(names(&subcommands(vegeta, "vegeta")), ["attack", "encode"]);
    }

    #[test]
    fn reads_command_columns_under_openssl_headings() {
        let openssl = "help:\n\nStandard commands\nasn1parse         ca                ciphers\nx509\n\nMessage Digest commands (see the `dgst' command for more details)\nblake2b512        md5\n\nCipher commands (see the `enc' command for more details)\naes-128-cbc       base64\n";
        assert_eq!(
            names(&subcommands(openssl, "openssl")),
            [
                "asn1parse",
                "ca",
                "ciphers",
                "x509",
                "blake2b512",
                "md5",
                "aes-128-cbc",
                "base64"
            ]
        );
        // Prose under such a heading ends the list.
        let prose =
            "Standard commands\nbuild   test\nSee the manual for more.\n  deploy  Ship it\n";
        assert_eq!(names(&subcommands(prose, "tool")), ["build", "test"]);
    }

    #[test]
    fn kingpin_negatable_flags_have_both_spellings_and_no_value() {
        // teleport 18's `tsh ssh --help`.
        let items = options(
            "  -A, --[no-]forward-agent       Forward agent to target node.\n  -L, --forward=FORWARD ...      Forward localhost connections\n",
        );
        assert_eq!(
            names(&items),
            [
                "-A",
                "--forward-agent",
                "--no-forward-agent",
                "-L",
                "--forward"
            ]
        );
        assert_eq!(
            items
                .iter()
                .map(|item| item.takes_value)
                .collect::<Vec<_>>(),
            [
                Some(false),
                Some(false),
                Some(false),
                Some(true),
                Some(true)
            ]
        );
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
        // httpd 2.4's synopsis.
        let httpd = "Usage: /usr/sbin/httpd [-D name] [-d directory] [-f file]\n                       [-k start|restart|graceful|graceful-stop|stop]\n                       [-v] [-V] [-h] [-l] [-L] [-t] [-T] [-S] [-X]\nOptions:\n  -D name            : define a name for use in <IfDefine name> directives\n";
        assert_eq!(
            names(&option_values(httpd, "-k")),
            ["start", "restart", "graceful", "graceful-stop", "stop"]
        );
        assert!(option_values(httpd, "-D").is_empty());
        assert!(option_values("  [-k start|$(id)]\n  [-k a]\n", "-k").is_empty());
        // nginx 1.31's `-h`: a list closing the description that names the
        // placeholder.
        let nginx = "Options:\n  -s signal     : send signal to a master process: stop, quit, reopen, reload\n  -p prefix     : set prefix path (default: /opt/homebrew/)\n  -g directives : set global directives out of configuration file\n";
        assert_eq!(
            names(&option_values(nginx, "-s")),
            ["stop", "quit", "reopen", "reload"]
        );
        assert!(option_values(nginx, "-p").is_empty());
        for prose in [
            // The placeholder isn't named before the list.
            "  --color WHEN  colorize: always, never, auto\n",
            // Examples aren't choices.
            "  --format FORMAT  a format, e.g.: json, yaml\n",
            "  --format FORMAT  format such as: json, yaml\n",
            // One word, or words that aren't plain names.
            "  --level LEVEL  log level: debug\n",
            "  --level LEVEL  log level: debug, $(id)\n",
        ] {
            let option = prose.split_whitespace().next().unwrap();
            assert!(option_values(prose, option).is_empty(), "{prose}");
        }
        assert_eq!(
            names(&option_values(
                "  --level LEVEL  log level: debug, info or warn.\n",
                "--level"
            )),
            ["debug", "info", "warn"]
        );
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
