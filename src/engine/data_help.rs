//! App-owned documentation for tools whose root help omits their verbs or
//! options, including data tools, FFmpeg, Go, Terraform, and npm. Fixed
//! list/help requests run in a private directory without user data, startup
//! configuration, scripts, passwords, or output paths.

use super::probe::{self, Budget, Capture, Probe};
use std::ffi::OsString;
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tool {
    Miller,
    Qsv,
    Ffmpeg,
    Go,
    Terraform,
    Npm,
}

pub(crate) fn tool(program: &str) -> Option<Tool> {
    match Path::new(program).file_name()?.to_str()? {
        "mlr" => Some(Tool::Miller),
        "qsv" | "qsvlite" | "qsvdp" => Some(Tool::Qsv),
        "ffmpeg" | "ffprobe" | "ffplay" => Some(Tool::Ffmpeg),
        "go" => Some(Tool::Go),
        "terraform" | "tofu" => Some(Tool::Terraform),
        "npm" => Some(Tool::Npm),
        _ => None,
    }
}

fn env(tool: Tool, dir: &Path) -> Vec<(OsString, Option<OsString>)> {
    let mut env = super::native::help_env();
    env.extend([
        ("HOME".into(), Some(dir.as_os_str().to_owned())),
        ("NO_COLOR".into(), Some("1".into())),
    ]);
    match tool {
        Tool::Miller | Tool::Qsv | Tool::Ffmpeg => env.extend([
            ("MLRRC".into(), Some("__none__".into())),
            // qsv's documented switch; an empty path is loaded and fails.
            ("QSV_DOTENV_PATH".into(), Some("<NONE>".into())),
            ("QSV_LOG_LEVEL".into(), Some("off".into())),
            ("QSV_MCP_LOG_LEVEL".into(), Some("off".into())),
            ("QSV_LOG_DIR".into(), Some(dir.as_os_str().to_owned())),
        ]),
        Tool::Go => env.extend([
            // Never download a toolchain or module to print help.
            ("GOTOOLCHAIN".into(), Some("local".into())),
            ("GOPROXY".into(), Some("off".into())),
            ("GOSUMDB".into(), Some("off".into())),
            ("GOWORK".into(), Some("off".into())),
            ("GOFLAGS".into(), None),
        ]),
        Tool::Terraform => env.extend([
            ("CHECKPOINT_DISABLE".into(), Some("1".into())),
            ("TF_IN_AUTOMATION".into(), Some("1".into())),
            ("TF_CLI_ARGS".into(), None),
            ("TF_WORKSPACE".into(), None),
            (
                "TF_CLI_CONFIG_FILE".into(),
                Some(if cfg!(windows) { "NUL" } else { "/dev/null" }.into()),
            ),
            (
                "TF_DATA_DIR".into(),
                Some(dir.join("terraform-data").into_os_string()),
            ),
        ]),
        Tool::Npm => {
            env.extend(super::native::offline_env(super::native::Flavor::Npm));
            env.push((
                "npm_config_cache".into(),
                Some(dir.join("npm-cache").into_os_string()),
            ));
        }
    }
    env
}

fn query(tool: Tool, path: &Path, args: &[&str], budget: &mut Budget) -> Result<String, String> {
    query_up_to(tool, path, args, budget, budget.max_output)
}

/// [`query`] with its own output limit: FFmpeg's full help is about 1 MiB.
fn query_up_to(
    tool: Tool,
    path: &Path,
    args: &[&str],
    budget: &mut Budget,
    limit: usize,
) -> Result<String, String> {
    let usual = std::mem::replace(&mut budget.max_output, limit);
    let text = query_once(tool, path, args, budget);
    budget.max_output = usual;
    text
}

fn query_once(
    tool: Tool,
    path: &Path,
    args: &[&str],
    budget: &mut Budget,
) -> Result<String, String> {
    let dir = crate::utils::cache_dir().join("data-help-probes");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let output = probe::run_in(
        &Probe {
            program: path,
            args: args.iter().map(OsString::from).collect(),
            env: env(tool, &dir),
            capture: Capture::Combined,
        },
        &dir,
        budget,
    )
    .map_err(|e| e.to_string())?;
    if output.truncated || output.status != Some(0) {
        return Err("data-tool metadata failed or exceeded the probe limit".into());
    }
    String::from_utf8(output.data)
        .map(|text| super::docs::strip_formatting(&text))
        .map_err(|_| "data-tool metadata is not UTF-8".into())
}

/// `npm -l` is the all-commands listing, but npm exits 1 after printing it.
fn query_npm_listing(path: &Path, budget: &mut Budget) -> Result<String, String> {
    let dir = crate::utils::cache_dir().join("data-help-probes");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let output = probe::run_in(
        &Probe {
            program: path,
            args: vec!["-l".into()],
            env: env(Tool::Npm, &dir),
            capture: Capture::Combined,
        },
        &dir,
        budget,
    )
    .map_err(|e| e.to_string())?;
    if output.truncated || !matches!(output.status, Some(0) | Some(1)) {
        return Err("npm command listing failed or exceeded the probe limit".into());
    }
    let text = String::from_utf8(output.data)
        .map_err(|_| "npm command listing is not UTF-8".to_owned())?;
    let text = super::docs::strip_formatting(&text);
    if !text.contains("All commands:") {
        return Err("unrecognized npm command listing".into());
    }
    Ok(text)
}

/// npm's command names and aliases from `npm -l`. Command entries are the
/// four-space names; the `alias:`/`aliases:` lines below each add more.
fn npm_commands(text: &str, limit: usize) -> Result<String, String> {
    let mut names: Vec<String> = Vec::new();
    let mut in_commands = false;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed == "All commands:" {
            in_commands = true;
            continue;
        }
        if !in_commands {
            continue;
        }
        let indent = line.len() - trimmed.len();
        let alias = trimmed
            .strip_prefix("alias:")
            .or_else(|| trimmed.strip_prefix("aliases:"));
        if let Some(aliases) = alias {
            for alias in aliases.split(',').map(str::trim) {
                if name(alias) && !names.iter().any(|known| known == alias) {
                    names.push(alias.to_owned());
                }
            }
        } else if indent == 4
            && let Some(command) = trimmed.split_whitespace().next()
            && name(command)
            && !names.iter().any(|known| known == command)
        {
            names.push(command.to_owned());
        }
    }
    if names.len() < 20 {
        return Err("unrecognized npm command listing".into());
    }
    let mut output = String::from("\nCommands:\n");
    for name in names.into_iter().take(limit) {
        output.push_str("    ");
        output.push_str(&name);
        output.push('\n');
    }
    Ok(output)
}

/// Long CLI flags from `npm config ls -l --json`. Every npm config key can
/// be passed as `--key`, and the listing includes aliases such as `call`
/// and `package`.
fn npm_option_names(value: &serde_json::Value, limit: usize) -> Result<Vec<String>, String> {
    let keys = value.as_object().ok_or("npm config is not an object")?;
    let mut names: Vec<String> = keys
        .keys()
        .filter(|key| name(key))
        .map(|key| format!("--{key}"))
        .take(limit)
        .collect();
    if names.len() < 20 {
        return Err("unrecognized npm configuration listing".into());
    }
    names.sort();
    Ok(names)
}

fn npm_options(path: &Path, budget: &mut Budget) -> Result<String, String> {
    let text = query(Tool::Npm, path, &["config", "ls", "-l", "--json"], budget)?;
    let value: serde_json::Value =
        serde_json::from_str(&text).map_err(|_| "malformed npm configuration JSON".to_owned())?;
    let names = npm_option_names(&value, budget.max_candidates)?;
    let mut output = String::from("\nOptions:\n");
    for name in names {
        output.push_str("    ");
        output.push_str(&name);
        output.push('\n');
    }
    Ok(output)
}

/// Go documents some flags only in its `usage:` line, as bracketed groups:
/// `usage: go env [-json] [-changed] [-w] [var ...]`. Add the option groups
/// (`[build flags]` and `[packages]` are not options).
fn go_usage_options(text: &str) -> String {
    let mut options: Vec<(String, bool)> = Vec::new();
    for line in text.lines() {
        let Some(usage) = line.trim().strip_prefix("usage:") else {
            continue;
        };
        let mut rest = usage;
        while let Some(start) = rest.find('[') {
            rest = &rest[start + 1..];
            let Some(end) = rest.find(']') else {
                break;
            };
            let group = rest[..end].trim();
            rest = &rest[end + 1..];
            if !group.starts_with('-') {
                continue;
            }
            let (name, rest) = group
                .split_once(|c: char| c.is_whitespace() || c == '=')
                .unwrap_or((group, ""));
            let name = name.trim_end_matches(|c: char| ",;:".contains(c));
            if name.len() < 2
                || !name.starts_with('-')
                || !name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-_".contains(c))
            {
                continue;
            }
            if !options.iter().any(|(known, _)| known == name) {
                options.push((name.to_owned(), !rest.trim().is_empty()));
            }
        }
    }
    let mut output = String::new();
    for (name, valued) in options {
        output.push_str("    ");
        output.push_str(&name);
        if valued {
            output.push_str(" VALUE");
        }
        output.push('\n');
    }
    output
}

fn name(word: &str) -> bool {
    !word.is_empty()
        && word.len() <= 128
        && word.starts_with(|c: char| c.is_ascii_lowercase())
        && word
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "-_".contains(c))
}

fn description(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).take(400).collect()
}

fn commands(tool: Tool, text: &str, limit: usize) -> Result<String, String> {
    let mut output = String::from("\nCommands:\n");
    let mut count = 0;
    let mut expected = None;
    let lines: Vec<&str> = text.lines().collect();
    let entries = if tool == Tool::Qsv {
        let header = lines
            .first()
            .and_then(|line| line.strip_prefix("Installed commands ("))
            .and_then(|s| s.strip_suffix("):"))
            .ok_or("unrecognized qsv command listing")?;
        expected = Some(
            header
                .parse::<usize>()
                .map_err(|_| "invalid qsv command count")?,
        );
        &lines[1..]
    } else {
        &lines[..]
    };
    for line in entries {
        if tool == Tool::Qsv && line.trim().is_empty() {
            break;
        }
        let (word, desc) = line
            .trim()
            .split_once(char::is_whitespace)
            .map_or((line.trim(), ""), |(word, desc)| (word, desc.trim()));
        if !name(word) || tool == Tool::Miller && !desc.is_empty() {
            return Err("malformed data-tool command listing".into());
        }
        count += 1;
        if count > limit {
            return Err("too many data-tool commands".into());
        }
        output.push_str(&format!("    {word}  {}\n", description(desc)));
    }
    if count == 0 || expected.is_some_and(|expected| count != expected) {
        return Err("incomplete data-tool command listing".into());
    }
    Ok(output)
}

fn flag(word: &str) -> bool {
    word.starts_with('-')
        && word != "--"
        && word.len() <= 128
        && word
            .trim_start_matches('-')
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_".contains(c))
}

/// Normalize Miller's JSON option declarations, preserving aliases and
/// arity. Its braces name arbitrary field lists, not finite enum choices.
fn json_options(value: &serde_json::Value, global: bool, limit: usize) -> Result<String, String> {
    let entries = value.as_array().ok_or("Miller options are not an array")?;
    if entries.len() > limit {
        return Err("too many Miller options".into());
    }
    let mut output = String::from("\nOptions:\n");
    for entry in entries {
        let primary = entry
            .get(if global { "name" } else { "flag" })
            .and_then(|s| s.as_str())
            .filter(|s| flag(s))
            .ok_or("invalid Miller option name")?;
        let aliases = entry.get(if global { "alt_names" } else { "aliases" });
        let mut names = vec![primary];
        if let Some(aliases) = aliases {
            for alias in aliases
                .as_array()
                .ok_or("Miller aliases are not an array")?
            {
                names.push(
                    alias
                        .as_str()
                        .filter(|s| flag(s))
                        .ok_or("invalid Miller option alias")?,
                );
            }
        }
        let valued = match entry.get("arg") {
            None => false,
            Some(arg) => !arg
                .as_str()
                .ok_or("invalid Miller option argument")?
                .is_empty(),
        };
        let desc = entry
            .get(if global { "help" } else { "desc" })
            .and_then(|s| s.as_str())
            .unwrap_or_default();
        output.push_str(&format!(
            "    {}{}  {}\n",
            names.join(", "),
            if valued { " VALUE" } else { "" },
            description(desc)
        ));
    }
    // Help itself is handled before a verb's option table.
    if !global {
        output.push_str("    -h, --help  Show this message\n");
    }
    Ok(output)
}

fn indent_options(text: &str) -> String {
    text.lines()
        .map(|line| {
            if line.starts_with('-') {
                format!("    {line}\n")
            } else {
                format!("{line}\n")
            }
        })
        .collect()
}

pub(crate) fn read(
    tool: Tool,
    path: &Path,
    commands_path: &[String],
    budget: &mut Budget,
) -> Result<String, String> {
    match (tool, commands_path) {
        (Tool::Qsv, []) => {
            let help = query(tool, path, &["--help"], budget)?;
            let listing = query(tool, path, &["--list"], budget)?;
            Ok(help + &commands(tool, &listing, budget.max_candidates)?)
        }
        (Tool::Qsv, path_words) => {
            let mut args: Vec<&str> = path_words.iter().map(String::as_str).collect();
            args.push("--help");
            query(tool, path, &args, budget)
        }
        (Tool::Miller, []) => {
            let verbs = query(tool, path, &["help", "list-verbs"], budget)?;
            let listing = commands(tool, &verbs, budget.max_candidates)?;
            let flags = match query(tool, path, &["help", "flag", "--as-json"], budget) {
                Ok(text) => {
                    let value: serde_json::Value =
                        serde_json::from_str(&text).map_err(|_| "malformed Miller JSON flags")?;
                    json_options(&value, true, budget.max_candidates)?
                }
                Err(_) => indent_options(&query(tool, path, &["help", "flags"], budget)?),
            };
            Ok(listing + &flags)
        }
        (Tool::Miller, [verb]) => {
            match query(tool, path, &["help", "verb", verb, "--as-json"], budget) {
                Ok(text) => {
                    let value: serde_json::Value =
                        serde_json::from_str(&text).map_err(|_| "malformed Miller JSON verb")?;
                    let entries = value.as_array().ok_or("Miller verb is not an array")?;
                    let [entry] = entries.as_slice() else {
                        return Err("Miller returned multiple verbs".into());
                    };
                    if entry.get("name").and_then(|s| s.as_str()) != Some(verb) {
                        return Err("Miller returned another verb".into());
                    }
                    let usage = entry
                        .get("usage_text")
                        .and_then(|s| s.as_str())
                        .ok_or("Miller verb has no usage")?;
                    let options = entry.get("options").ok_or("Miller verb has no options")?;
                    // The original usage is needed for required positional
                    // arguments; declarations below carry machine arity.
                    Ok(usage
                        .lines()
                        .find(|line| line.starts_with("Usage:"))
                        .unwrap_or_default()
                        .to_owned()
                        + &json_options(options, false, budget.max_candidates)?)
                }
                Err(_) => Ok(indent_options(&query(
                    tool,
                    path,
                    &["help", "verb", verb],
                    budget,
                )?)),
            }
        }
        (Tool::Ffmpeg, []) => {
            let full = query_up_to(
                tool,
                path,
                &["-hide_banner", "-h", "full"],
                budget,
                FFMPEG_HELP_LIMIT,
            )?;
            let codecs = query(tool, path, &["-hide_banner", "-codecs"], budget)?;
            ffmpeg_options(&full, &codecs, budget.max_candidates)
        }
        (Tool::Ffmpeg, _) => Err("FFmpeg has no subcommands".into()),
        // Go's own help subcommand is the only source: `go --help` exits 2.
        // `go help <command>` and `go help <command> <subcommand>` work.
        (Tool::Go, []) => {
            let text = query(tool, path, &["help"], budget)?;
            let options = go_usage_options(&text);
            Ok(text + &options)
        }
        (Tool::Go, path_words) => {
            let mut args = vec!["help"];
            args.extend(path_words.iter().map(String::as_str));
            let mut text = query(tool, path, &args, budget)?;
            // The build flags are shared by build, clean, get, install, list,
            // run, test, and vet; test's own flags live in testflag.
            if path_words.len() == 1
                && matches!(
                    path_words[0].as_str(),
                    "build" | "clean" | "get" | "install" | "list" | "run" | "test" | "vet"
                )
            {
                if path_words[0] != "build" {
                    text.push_str(&query(tool, path, &["help", "build"], budget)?);
                }
                if path_words[0] == "test" {
                    text.push_str(&query(tool, path, &["help", "testflag"], budget)?);
                }
            }
            text.push_str(&go_usage_options(&text));
            Ok(text)
        }
        // Terraform rejects nothing here, but its -help spelling is stable
        // for every command level: `terraform state list -help`.
        (Tool::Terraform, []) => query(tool, path, &["-help"], budget),
        (Tool::Terraform, path_words) => {
            let mut args: Vec<&str> = path_words.iter().map(String::as_str).collect();
            args.push("-help");
            let mut text = query(tool, path, &args, budget)?;
            // destroy is apply -destroy: its own page points at plan but
            // also accepts apply's flags, including -auto-approve.
            if path_words.len() == 1 && path_words[0] == "destroy" {
                text.push_str(&query(tool, path, &["plan", "-help"], budget)?);
                text.push_str(&query(tool, path, &["apply", "-help"], budget)?);
            }
            Ok(text)
        }
        // npm -l prints commands and their aliases but exits 1; its config
        // listing is the full option vocabulary. Deeper levels only need
        // the options: command nesting comes from the root commands and
        // npm's own completion.
        (Tool::Npm, []) => {
            let listing = query_npm_listing(path, budget)?;
            let listing = npm_commands(&listing, budget.max_candidates)?;
            Ok(listing + &npm_options(path, budget)?)
        }
        (Tool::Npm, _) => npm_options(path, budget),
        _ => Err("Miller verbs do not contain subcommands".into()),
    }
}

const FFMPEG_HELP_LIMIT: usize = 8 * 1024 * 1024;
/// FFmpeg's stream specifiers by stream type: `-c:v`, `-b:a`.
const FFMPEG_STREAMS: [char; 6] = ['v', 'a', 's', 'd', 't', 'V'];

/// FFmpeg's `-h full` as declarations the help reader understands. Main
/// options sit at the margin (`-c[:<stream_spec>] <codec>  select ...`);
/// per-stream ones also get a declaration per stream type. Codec, format,
/// and filter AVOptions are indented with their type (`  -preset <string>`)
/// and all take a value. Codec options list `-codecs`'s names, with the
/// decoders and encoders each implements (`libx264` for h264).
fn ffmpeg_options(full: &str, codecs: &str, limit: usize) -> Result<String, String> {
    let main = regex!(r"^(-[A-Za-z0-9_]+)(\[:<stream_spec>\])?((?: <[^>]*>)?)(?:\s{2,}|$)");
    let av = regex!(r"^  (-[A-Za-z0-9_]+)\s+<[A-Za-z0-9_]+>");
    let mut options: Vec<(String, bool, bool)> = Vec::new();
    for line in full.lines() {
        let (name, per_stream, valued) = if let Some(caps) = main.captures(line) {
            (
                caps[1].to_owned(),
                caps.get(2).is_some(),
                !caps[3].is_empty(),
            )
        } else if let Some(caps) = av.captures(line) {
            (caps[1].to_owned(), false, true)
        } else {
            continue;
        };
        if !options.iter().any(|(known, ..)| *known == name) {
            options.push((name, per_stream, valued));
        }
    }
    // `usage: ffmpeg [options] [[infile options] -i infile]...`
    if full
        .lines()
        .any(|line| line.starts_with("usage:") && line.contains(" -i "))
        && !options.iter().any(|(name, ..)| name == "-i")
    {
        options.push(("-i".into(), false, true));
    }
    if options.len() < 10 || options.len() > limit {
        return Err("unrecognized FFmpeg option listing".into());
    }
    let mut kinds: Vec<(char, Vec<String>)> = Vec::new();
    let mut separated = false;
    for line in codecs.lines() {
        if !separated {
            separated = line.trim_start().starts_with("---");
            continue;
        }
        let mut fields = line.split_whitespace();
        let (Some(flags), Some(codec)) = (fields.next(), fields.next()) else {
            continue;
        };
        let Some(kind) = flags.chars().nth(2).filter(|_| flags.len() == 6) else {
            continue;
        };
        let mut names = vec![codec.to_owned()];
        for group in regex!(r"\((?:de|en)coders: ([^)]*)\)").captures_iter(line) {
            names.extend(group[1].split_whitespace().map(str::to_owned));
        }
        let list = match kinds.iter_mut().find(|(known, _)| *known == kind) {
            Some((_, list)) => list,
            None => {
                kinds.push((kind, Vec::new()));
                &mut kinds.last_mut().unwrap().1
            }
        };
        for name in names {
            if name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "_-".contains(c))
                && !list.contains(&name)
            {
                list.push(name);
            }
        }
    }
    let codec_list = |kind: Option<char>| {
        let mut names = vec!["copy".to_owned()];
        for (known, list) in &kinds {
            if kind.is_none_or(|kind| kind.eq_ignore_ascii_case(known)) {
                names.extend(
                    list.iter()
                        .filter(|n| !names.contains(n))
                        .cloned()
                        .collect::<Vec<_>>(),
                );
            }
        }
        format!("{{{}}}", names.join(","))
    };
    let mut output = String::from("Options:\n");
    for (name, per_stream, valued) in &options {
        let codec = matches!(name.as_str(), "-c" | "-codec");
        let value = |kind: Option<char>| {
            if codec && !kinds.is_empty() {
                codec_list(kind)
            } else {
                "VALUE".into()
            }
        };
        let single = match name.as_str() {
            "-vcodec" => Some('V'),
            "-acodec" => Some('A'),
            "-scodec" => Some('S'),
            _ => None,
        };
        if let Some(kind) = single.filter(|_| !kinds.is_empty()) {
            output.push_str(&format!(
                "    {name} {}  FFmpeg option\n",
                codec_list(Some(kind))
            ));
        } else if *valued {
            output.push_str(&format!("    {name} {}  FFmpeg option\n", value(None)));
        } else {
            output.push_str(&format!("    {name}  FFmpeg option\n"));
        }
        if *per_stream {
            for stream in FFMPEG_STREAMS {
                let spelled = format!("{name}:{stream}");
                if *valued {
                    output.push_str(&format!(
                        "    {spelled} {}  FFmpeg option\n",
                        value(Some(stream))
                    ));
                } else {
                    output.push_str(&format!("    {spelled}  FFmpeg option\n"));
                }
            }
        }
    }
    Ok(output)
}

/// A chain separator follows a complete verb argument list. A first
/// required expression named 'then' is still data; option values were
/// consumed by the walk before this check. Unknown/variadic contexts fail
/// closed instead of promoting script or file words into verbs.
pub(crate) fn chain_ready(context: &[String], root: &str, verb: &str, help: &str) -> bool {
    if context.iter().any(|word| word == "--mload" || word == "--") {
        return false;
    }
    let options = super::docs::options(root);
    let mut i = 0;
    let mut start = None;
    while i < context.len() {
        let word = &context[i];
        if start.is_none() {
            if word == verb {
                start = Some(i + 1);
            } else if word.starts_with('-') {
                let (name, attached) = word
                    .split_once('=')
                    .map_or((word.as_str(), false), |(name, _)| (name, true));
                let Some(option) = options.iter().find(|option| option.value == name) else {
                    return false;
                };
                if option.takes_value == Some(true) && !attached {
                    i += 1;
                }
            } else {
                return false;
            }
        }
        i += 1;
    }
    let Some(start) = start else {
        return false;
    };
    let Some(usage) = help.lines().find_map(|line| line.strip_prefix("Usage:")) else {
        return false;
    };
    let usage = usage
        .trim()
        .split_once(char::is_whitespace)
        .map(|(_, rest)| rest.trim());
    let Some(rest) = usage
        .and_then(|usage| usage.strip_prefix(verb))
        .filter(|rest| rest.starts_with(char::is_whitespace))
    else {
        return false;
    };
    let rest = regex!(r"\[[^\]]*\]|\{flags\}").replace_all(rest, "");
    let required = regex!(r"\{[^}]+\}|<[^>]+>").find_iter(&rest).count();
    let options = super::docs::options(help);
    let mut positionals = 0;
    let mut expression_option = false;
    let mut i = start;
    while i < context.len() {
        let word = &context[i];
        if word.starts_with('-') {
            let (name, attached) = word
                .split_once('=')
                .map_or((word.as_str(), false), |(name, _)| (name, true));
            let Some(option) = options.iter().find(|option| option.value == name) else {
                return false;
            };
            if option.takes_value == Some(true) {
                if rest.contains("DSL expression") && matches!(name, "-e" | "-f") {
                    expression_option = true;
                }
                if !attached {
                    i += 1;
                    if i >= context.len() {
                        return false;
                    }
                }
            }
        } else {
            positionals += 1;
        }
        i += 1;
    }
    expression_option || positionals >= required
}

#[cfg(test)]
mod tests {
    use super::*;

    /// FFmpeg 8's `-h full` and `-codecs`: main options at the margin
    /// (some per stream), AVOptions indented with their type, and codec
    /// names with the decoders and encoders that implement them.
    #[test]
    fn ffmpeg_options_cover_streams_avoptions_and_codecs() {
        let mut full = String::from(
            "usage: ffmpeg [options] [[infile options] -i infile]... {[outfile options] outfile}...\n\nGetting help:\n    -h      -- print basic options\n\nGlobal options:\n-loglevel <loglevel>  set logging level\n-y                  overwrite output files\n-hide_banner <hide_banner>  do not show program banner\n-c[:<stream_spec>] <codec>  select encoder/decoder\n-vcodec <codec>     alias for -c:v\n-b <bitrate>        video bitrate (please use -b:v)\n",
        );
        for name in ["f", "map", "t", "ss", "to", "an"] {
            full.push_str(&format!("-{name} <x>  option\n"));
        }
        full.push_str("\nlibx264 AVOptions:\n  -preset            <string>     E..V....... Set the encoding preset (default \"medium\")\n  -crf               <float>      E..V....... Constant quality\n     slow                         E..V.......\n");
        let codecs = "Codecs:\n D..... = Decoding supported\n -------\n DEV.LS h264                 H.264 (encoders: libx264 libx264rgb h264_videotoolbox)\n DEA.L. aac                  AAC (decoders: aac aac_fixed) (encoders: aac aac_at)\n D.S... ass                  ASS subtitle\n DEV.L. $(id)                broken\n";
        let text = ffmpeg_options(&full, codecs, 5000).unwrap();
        let options = crate::engine::docs::options(&text);
        let arity = |name: &str| {
            options
                .iter()
                .find(|o| o.value == name)
                .unwrap_or_else(|| panic!("{name}: {text}"))
                .takes_value
        };
        assert_eq!(arity("-y"), Some(false));
        assert_eq!(arity("-loglevel"), Some(true));
        assert_eq!(arity("-c"), Some(true));
        assert_eq!(arity("-c:v"), Some(true));
        assert_eq!(arity("-preset"), Some(true));
        assert_eq!(arity("-crf"), Some(true));
        assert_eq!(arity("-i"), Some(true), "from the usage line");
        assert!(
            !options.iter().any(|o| o.value == "-b:v"),
            "-b has no stream form here"
        );
        assert!(!options.iter().any(|o| o.value == "-slow"));
        let values = |option: &str| {
            crate::engine::docs::option_values(&text, option)
                .into_iter()
                .map(|item| item.value)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            values("-c:v"),
            ["copy", "h264", "libx264", "libx264rgb", "h264_videotoolbox"]
        );
        assert_eq!(values("-vcodec"), values("-c:v"));
        assert_eq!(values("-c:a"), ["copy", "aac", "aac_fixed", "aac_at"]);
        assert_eq!(values("-c:s"), ["copy", "ass"]);
        assert_eq!(values("-c").len(), 9);
        assert!(values("-preset").is_empty());
        // Not FFmpeg's help.
        assert!(ffmpeg_options("Usage: other\n", codecs, 5000).is_err());
    }

    #[test]
    fn command_lists_reject_truncation_and_shell_shaped_names() {
        assert!(
            commands(
                Tool::Qsv,
                "Installed commands (2):\n    cat  Cats\n    sort  Sorts\n\nsponsor",
                10
            )
            .unwrap()
            .contains("sort")
        );
        for text in [
            "Installed commands (2):\n    cat  Cats\n",
            "Installed commands (1):\n    a;touch  bad\n",
            "sort\n",
        ] {
            assert!(commands(Tool::Qsv, text, 10).is_err());
        }
        assert!(commands(Tool::Miller, "cat\nsort\n", 1).is_err());
        assert!(commands(Tool::Miller, "cat\nnot a verb\n", 10).is_err());
    }

    #[test]
    fn machine_options_keep_aliases_and_fields_as_arbitrary_values() {
        let value = serde_json::json!([{"flag":"-tr", "aliases":["-rt"], "arg":"{a,b,c}", "type":"csv-list", "desc":"Fields"}, {"flag":"-b", "type":"bool"}]);
        let text = json_options(&value, false, 10).unwrap();
        let options = super::super::docs::options(&text);
        assert_eq!(
            options
                .iter()
                .find(|o| o.value == "-rt")
                .unwrap()
                .takes_value,
            Some(true)
        );
        assert_eq!(
            options
                .iter()
                .find(|o| o.value == "-b")
                .unwrap()
                .takes_value,
            Some(false)
        );
        assert!(super::super::docs::option_values(&text, "-tr").is_empty());
        assert!(json_options(&serde_json::json!([{"flag":"--x;touch"}]), false, 10).is_err());
    }

    #[test]
    fn probes_disable_startup_files_with_each_tools_own_switch() {
        let env = env(Tool::Qsv, Path::new("/private/probe"));
        let value = |name: &str| {
            env.iter()
                .rev()
                .find(|(key, _)| key == name)
                .and_then(|(_, value)| value.clone())
        };
        // An empty QSV_DOTENV_PATH names a file to load; qsv 23 then exits
        // 1 without printing anything.
        assert_eq!(value("QSV_DOTENV_PATH"), Some("<NONE>".into()));
        assert_eq!(value("MLRRC"), Some("__none__".into()));
        assert_eq!(value("HOME"), Some("/private/probe".into()));
    }

    #[cfg(unix)]
    #[test]
    fn go_terraform_and_npm_read_their_own_documentation() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("notypo-data-help-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let script = |name: &str, body: &str| {
            let path = dir.join(name);
            std::fs::write(&path, body).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            path
        };
        let go = script(
            "go",
            "#!/bin/sh\ncase \"$*\" in\n  \"help\") printf 'usage: go <command>\\n\\nThe commands are:\\n\\n    build  compile\\n';;\n  \"help build\") printf 'usage: go build [-o output] [packages]\\n';;\n  *) exit 1;;\nesac\n",
        );
        let terraform = script(
            "terraform",
            "#!/bin/sh\ncase \"$*\" in\n  \"-help\") printf 'Main commands:\\n  destroy  Destroy\\n';;\n  \"destroy -help\") printf 'Usage: terraform destroy\\n';;\n  \"plan -help\") printf '  -out=path  Write plan\\n';;\n  \"apply -help\") printf '  -auto-approve  Skip approval\\n';;\n  *) exit 1;;\nesac\n",
        );
        let npm = script(
            "npm",
            "#!/bin/sh\ncase \"$*\" in\n  \"-l\") printf 'All commands:\\n\\n    access  Access\\n    adduser  Add\\n    audit  Audit\\n    cache  Cache\\n    ci  CI\\n    config  Config\\n    dedupe  Dedupe\\n    exec  Exec\\n    init  Init\\n    install  Install\\n    link  Link\\n    publish  Publish\\n    run  Run\\n    test  Test\\n    uninstall  Uninstall\\n    update  Update\\n    version  Version\\n    view  View\\n    whoami  Who\\n    workspace  Workspace\\n                    aliases: tst, i\\n'; exit 1;;\n  \"config ls -l --json\") printf '{\"access\":false,\"all\":false,\"audit\":true,\"cache\":false,\"call\":\"\",\"ci\":false,\"depth\":0,\"dry-run\":false,\"force\":false,\"global\":false,\"json\":true,\"link\":false,\"location\":\"\",\"omit\":[],\"package\":\"\",\"package-lock-only\":true,\"prefix\":\"\",\"save-dev\":false,\"version\":false,\"workspace\":\"\"}\\n';;\n  *) exit 1;;\nesac\n",
        );
        let mut budget = Budget::for_timeout(5.0);
        let go_text = read(Tool::Go, &go, &["build".to_owned()], &mut budget).unwrap();
        assert!(go_text.contains("-o"), "{go_text}");
        let terraform_text = read(
            Tool::Terraform,
            &terraform,
            &["destroy".to_owned()],
            &mut budget,
        )
        .unwrap();
        assert!(terraform_text.contains("-auto-approve"), "{terraform_text}");
        let npm_text = read(Tool::Npm, &npm, &[], &mut budget).unwrap();
        assert!(npm_text.contains("tst"), "{npm_text}");
        assert!(npm_text.contains("--save-dev"), "{npm_text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn npm_listing_keeps_commands_and_aliases() {
        let mut listing = String::from("npm <command>\n\nAll commands:\n\n");
        for command in [
            "access",
            "adduser",
            "audit",
            "cache",
            "ci",
            "config",
            "dedupe",
            "exec",
            "init",
            "install",
            "link",
            "publish",
            "run",
            "test",
            "uninstall",
            "update",
            "version",
            "view",
            "whoami",
            "workspace",
        ] {
            listing.push_str(&format!("    {command}          description\n"));
        }
        listing.push_str("                    aliases: x, i\n");
        let commands = npm_commands(&listing, 100).unwrap();
        let options = super::super::docs::subcommands(&commands, "npm");
        for name in ["install", "x", "i", "test"] {
            assert!(
                options.iter().any(|item| item.value == name),
                "{name}: {commands}"
            );
        }
    }

    #[test]
    fn go_usage_lines_supply_option_groups() {
        let text = "usage: go env [-json] [-changed] [-u] [-w] [var ...]\nusage: go build [-o output] [build flags] [packages]\n";
        let output = go_usage_options(text);
        let options = super::super::docs::options(&output);
        let arity = |name: &str| {
            options
                .iter()
                .find(|item| item.value == name)
                .unwrap_or_else(|| panic!("{name}: {output}"))
                .takes_value
        };
        assert_eq!(arity("-json"), Some(false));
        assert_eq!(arity("-changed"), Some(false));
        assert_eq!(arity("-o"), Some(true));
        assert!(!options.iter().any(|item| item.value == "build"));
    }

    #[test]
    fn npm_config_keys_become_long_options() {
        let mut value = serde_json::Map::new();
        for key in [
            "access",
            "all",
            "audit",
            "cache",
            "call",
            "ci",
            "depth",
            "dry-run",
            "force",
            "global",
            "json",
            "link",
            "location",
            "omit",
            "package",
            "package-lock-only",
            "prefix",
            "save-dev",
            "version",
            "workspace",
        ] {
            value.insert(key.into(), serde_json::Value::Bool(false));
        }
        value.insert("init.author.email".into(), "".into());
        value.insert("//registry.example/:_authToken".into(), "secret".into());
        let value = serde_json::Value::Object(value);
        let names = npm_option_names(&value, 100).unwrap();
        for name in [
            "--audit",
            "--call",
            "--package",
            "--package-lock-only",
            "--save-dev",
        ] {
            assert!(names.iter().any(|known| known == name), "{name}: {names:?}");
        }
        assert!(!names.iter().any(|known| known.contains("authToken")));
        assert!(!names.iter().any(|known| known.contains("init.")));
    }

    #[test]
    fn chain_separator_waits_for_expressions_and_skips_option_values() {
        let root = "Options:\n    --csv  CSV\n    --from VALUE  Input\n";
        let ready = |words: &[&str], verb, usage| {
            chain_ready(
                &words.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
                root,
                verb,
                usage,
            )
        };
        let sort = "Usage: mlr sort {flags}\n    -f VALUE  Fields\n";
        // {flags} is the verb's option placeholder, not a required value.
        assert!(ready(
            &["sort", "-f", "then"],
            "sort",
            "Usage: mlr sort [options]\n    -f VALUE  Fields\n"
        ));
        assert!(!ready(
            &["put"],
            "put",
            "Usage: mlr put [options] {DSL expression}\n"
        ));
        assert!(ready(
            &["put", "then"],
            "put",
            "Usage: mlr put [options] {DSL expression}\n"
        ));
        assert!(ready(&["--from", "sort", "sort", "x"], "sort", sort));
        assert!(!ready(&["--mload", "script.mlr", "sort"], "sort", sort));
    }
}
