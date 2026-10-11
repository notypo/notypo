//! App-owned documentation for data tools whose root help omits their verbs,
//! and for FFmpeg, whose basic help omits most of its options. Fixed
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
}

pub(crate) fn tool(program: &str) -> Option<Tool> {
    match Path::new(program).file_name()?.to_str()? {
        "mlr" => Some(Tool::Miller),
        "qsv" | "qsvlite" | "qsvdp" => Some(Tool::Qsv),
        "ffmpeg" | "ffprobe" | "ffplay" => Some(Tool::Ffmpeg),
        _ => None,
    }
}

fn env(dir: &Path) -> Vec<(OsString, Option<OsString>)> {
    let mut env = super::native::help_env();
    env.extend([
        ("HOME".into(), Some(dir.as_os_str().to_owned())),
        ("MLRRC".into(), Some("__none__".into())),
        // qsv's documented switch; an empty path is loaded and fails.
        ("QSV_DOTENV_PATH".into(), Some("<NONE>".into())),
        ("QSV_LOG_LEVEL".into(), Some("off".into())),
        ("QSV_MCP_LOG_LEVEL".into(), Some("off".into())),
        ("QSV_LOG_DIR".into(), Some(dir.as_os_str().to_owned())),
        ("NO_COLOR".into(), Some("1".into())),
    ]);
    env
}

fn query(path: &Path, args: &[&str], budget: &mut Budget) -> Result<String, String> {
    query_up_to(path, args, budget, budget.max_output)
}

/// [`query`] with its own output limit: FFmpeg's full help is about 1 MiB.
fn query_up_to(
    path: &Path,
    args: &[&str],
    budget: &mut Budget,
    limit: usize,
) -> Result<String, String> {
    let usual = std::mem::replace(&mut budget.max_output, limit);
    let text = query_once(path, args, budget);
    budget.max_output = usual;
    text
}

fn query_once(path: &Path, args: &[&str], budget: &mut Budget) -> Result<String, String> {
    let dir = crate::utils::cache_dir().join("data-help-probes");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let output = probe::run_in(
        &Probe {
            program: path,
            args: args.iter().map(OsString::from).collect(),
            env: env(&dir),
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
            let help = query(path, &["--help"], budget)?;
            let listing = query(path, &["--list"], budget)?;
            Ok(help + &commands(tool, &listing, budget.max_candidates)?)
        }
        (Tool::Qsv, path_words) => {
            let mut args: Vec<&str> = path_words.iter().map(String::as_str).collect();
            args.push("--help");
            query(path, &args, budget)
        }
        (Tool::Miller, []) => {
            let verbs = query(path, &["help", "list-verbs"], budget)?;
            let listing = commands(tool, &verbs, budget.max_candidates)?;
            let flags = match query(path, &["help", "flag", "--as-json"], budget) {
                Ok(text) => {
                    let value: serde_json::Value =
                        serde_json::from_str(&text).map_err(|_| "malformed Miller JSON flags")?;
                    json_options(&value, true, budget.max_candidates)?
                }
                Err(_) => indent_options(&query(path, &["help", "flags"], budget)?),
            };
            Ok(listing + &flags)
        }
        (Tool::Miller, [verb]) => {
            match query(path, &["help", "verb", verb, "--as-json"], budget) {
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
                    path,
                    &["help", "verb", verb],
                    budget,
                )?)),
            }
        }
        (Tool::Ffmpeg, []) => {
            let full = query_up_to(
                path,
                &["-hide_banner", "-h", "full"],
                budget,
                FFMPEG_HELP_LIMIT,
            )?;
            let codecs = query(path, &["-hide_banner", "-codecs"], budget)?;
            ffmpeg_options(&full, &codecs, budget.max_candidates)
        }
        (Tool::Ffmpeg, _) => Err("FFmpeg has no subcommands".into()),
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
        let env = env(Path::new("/private/probe"));
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
