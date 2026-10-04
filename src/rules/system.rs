use super::{one, output_contains};
use crate::types::{Command, Rule};
use crate::utils::{self, capture, replace_command, run_stdout};
use std::path::Path;
use std::{env, fs};

pub(super) const RULES: &[Rule] = &[
    Rule::new("fix_file", |c| env::var_os("EDITOR").is_some() && file_location(c).is_some(), fix_file),
    Rule::new("ifconfig_device_not_found", |c| c.is_app(&["ifconfig"]) && c.output().contains("error fetching interface information: Device not found"), |c| {
        let interface = c.output().split(' ').next().unwrap_or_default().trim_end_matches(':');
        let output = run_stdout("ifconfig", &["-a"]).unwrap_or_default();
        let possible: Vec<_> = output.lines().filter(|line| !line.is_empty() && !line.starts_with(' ')).filter_map(|line| line.split(' ').next()).collect();
        replace_command(c, interface, &possible)
    }),
    Rule::new("port_already_in_use", |c| used_port(c).and_then(pid_for_port).is_some(), |c| {
        used_port(c).and_then(pid_for_port).map_or_else(Vec::new, |pid| one(c.shell().and_(&[&format!("kill {pid}"), &c.script])))
    }).enabled_by_default(|| utils::which("lsof").is_some()),
    Rule::new("ssh_known_hosts", |c| c.is_app(&["ssh", "scp"]) && (c.script.starts_with("ssh") || c.script.starts_with("scp"))
        && (output_contains(c, &["WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED!", "WARNING: POSSIBLE DNS SPOOFING DETECTED!"])
            || regex!(r"Warning: the \S+ host key for '([^']+)' differs from the key for the IP address '([^']+)'").is_match(c.output())),
        |c| one(&c.script)).side_effect(remove_host_keys),
];

fn file_location<'a>(c: &Command<'a>) -> Option<regex::Captures<'a>> {
    const PATTERNS: &[&str] = &[
        r"(?m)^    at (?P<file>[^:\n]+):(?P<line>[0-9]+):(?P<col>[0-9]+)",
        r"(?m)^   (?P<file>[^:\n]+):(?P<line>[0-9]+):(?P<col>[0-9]+)",
        r#"(?m)^  File "(?P<file>[^:\n]+)", line (?P<line>[0-9]+)"#,
        r"(?m)^awk: (?P<file>[^:\n]+):(?P<line>[0-9]+):",
        r"(?m)^fatal: bad config file line (?P<line>[0-9]+) in (?P<file>[^:\n]+)",
        r"(?m)^llc: (?P<file>[^:\n]+):(?P<line>[0-9]+):(?P<col>[0-9]+):",
        r"(?m)^lua: (?P<file>[^:\n]+):(?P<line>[0-9]+):",
        r"(?m)^(?P<file>[^:\n]+) \(line (?P<line>[0-9]+)\):",
        r"(?m)^(?P<file>[^:\n]+): line (?P<line>[0-9]+): ",
        r"(?m)^(?P<file>[^:\n]+):(?P<line>[0-9]+):(?P<col>[0-9]+)",
        r"(?m)^(?P<file>[^:\n]+):(?P<line>[0-9]+):",
        r"at (?P<file>[^:\n]+) line (?P<line>[0-9]+)",
    ];
    static REGEXES: std::sync::LazyLock<Vec<regex::Regex>> = std::sync::LazyLock::new(|| {
        PATTERNS
            .iter()
            .map(|pattern| regex::Regex::new(pattern).unwrap())
            .collect()
    });
    REGEXES.iter().find_map(|re| {
        re.captures(c.output())
            .filter(|caps| Path::new(&caps["file"]).is_file())
    })
}

fn fix_file(c: &Command) -> Vec<String> {
    let Some(caps) = file_location(c) else {
        return Vec::new();
    };
    let editor = env::var("EDITOR").unwrap_or_default();
    let template = if caps.name("col").is_some() {
        c.settings()
            .fixcolcmd
            .as_deref()
            .unwrap_or(&c.settings().fixlinecmd)
    } else {
        &c.settings().fixlinecmd
    };
    let call = template
        .replace("{editor}", &editor)
        .replace("{file}", &caps["file"])
        .replace("{line}", &caps["line"])
        .replace("{col}", caps.name("col").map_or("", |m| m.as_str()));
    one(c.shell().and_(&[&call, &c.script]))
}

fn used_port<'a>(c: &Command<'a>) -> Option<&'a str> {
    [
        regex!(r"bind on address \('.*', (\d+)\)"),
        regex!(r"Unable to bind [^ ]*:(\d+)"),
        regex!(r"can't listen on port (\d+)"),
        regex!(r"listen EADDRINUSE [^ ]*:(\d+)"),
    ]
    .into_iter()
    .find_map(|re| capture(re, c.output()))
}

fn pid_for_port(port: &str) -> Option<String> {
    // Only numeric pids reach the shell command.
    let output = run_stdout("lsof", &["-i", &format!(":{port}")])?;
    output
        .lines()
        .nth(1)?
        .split_whitespace()
        .nth(1)?
        .parse::<u32>()
        .ok()
        .map(|pid| pid.to_string())
}

fn remove_host_keys(c: &Command, _: &str) {
    let mut files: std::collections::HashMap<&str, Vec<usize>> = std::collections::HashMap::new();
    for caps in
        regex!(r"(?m)(?:Offending (?:key for IP|\S+ key)|Matching host key) in ([^:]+):(\d+)")
            .captures_iter(c.output())
    {
        if let Ok(line) = caps[2].parse::<usize>() {
            files
                .entry(caps.get(1).unwrap().as_str())
                .or_default()
                .push(line);
        }
    }
    for (file, lines) in files {
        let Ok(contents) = fs::read_to_string(file) else {
            continue;
        };
        let corrected: String = contents
            .split_inclusive('\n')
            .enumerate()
            .filter(|(index, _)| !lines.contains(&(index + 1)))
            .map(|(_, line)| line)
            .collect();
        if corrected != contents {
            let _ = fs::write(file, corrected);
        }
    }
}
