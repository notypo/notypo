//! Configuration, port of `thefuck/conf.py` and `thefuck/const.py`.
//!
//! Precedence: defaults < `settings.py` < `THEFUCK_*` env vars < CLI flags.
//! `settings.py` is Python, so we parse the subset people actually write
//! (`name = <literal>`, see [`pylit`]) instead of executing it.

use crate::args::Args;
use crate::logs;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::{env, fs};

#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    /// `ALL_ENABLED in settings.rules` (thefuck's `DEFAULT_RULES`).
    pub rules_all: bool,
    pub rules: Vec<String>,
    pub exclude_rules: Vec<String>,
    pub wait_command: f64,
    pub require_confirmation: bool,
    pub no_colors: bool,
    pub debug: bool,
    pub priority: HashMap<String, i64>,
    pub history_limit: Option<usize>,
    pub alter_history: bool,
    pub wait_slow_command: f64,
    pub slow_commands: Vec<String>,
    pub repeat: bool,
    pub instant_mode: bool,
    pub num_close_matches: usize,
    pub env: Vec<(String, String)>,
    pub excluded_search_path_prefixes: Vec<String>,
    /// Used by the `fix_file` rule (`@default_settings` in Python).
    pub fixlinecmd: String,
    pub fixcolcmd: Option<String>,
    /// Candidate sources the correction engine must not use (see
    /// [`crate::engine::Source::setting_name`]).
    pub disabled_sources: Vec<String>,
    /// Programs whose generic completion protocol may be probed; `*` trusts
    /// every program that declares one. Built-in backends need no entry.
    pub trusted_completers: Vec<String>,
    /// Programs the engine may run with `--help`, including documented nested
    /// subcommands, to read options and values; `*` trusts all. Running a
    /// program is never free of risk.
    pub trusted_help: Vec<String>,
    /// Seconds a single discovery probe (such as a native completer) may run.
    pub probe_timeout: f64,
    /// Lets the structured engine rerun the failed command to read its
    /// output when none was captured, if the safety gate allows the command.
    pub replay_for_diagnosis: bool,
    /// Lets native completers look up resource names (instances, buckets,
    /// clusters) with the user's credentials. Read-only, but it reaches the
    /// network, so it is off by default.
    pub network_completion: bool,
}

impl Default for Settings {
    fn default() -> Self {
        let strings = |v: &[&str]| v.iter().map(|s| s.to_string()).collect();
        Settings {
            rules_all: true,
            rules: Vec::new(),
            exclude_rules: Vec::new(),
            wait_command: 3.0,
            require_confirmation: true,
            no_colors: false,
            debug: false,
            priority: HashMap::new(),
            history_limit: None,
            alter_history: true,
            wait_slow_command: 15.0,
            slow_commands: strings(&["lein", "react-native", "gradle", "./gradlew", "vagrant"]),
            repeat: false,
            instant_mode: false,
            num_close_matches: 3,
            env: vec![
                ("LC_ALL".into(), "C".into()),
                ("LANG".into(), "C".into()),
                ("GIT_TRACE".into(), "1".into()),
            ],
            excluded_search_path_prefixes: Vec::new(),
            fixlinecmd: "{editor} {file} +{line}".into(),
            fixcolcmd: None,
            disabled_sources: Vec::new(),
            trusted_completers: Vec::new(),
            trusted_help: Vec::new(),
            probe_timeout: 3.0,
            replay_for_diagnosis: false,
            network_completion: false,
        }
    }
}

const SETTINGS_HEADER: &str = "# The Fuck settings file
#
# The rules are defined as in the example bellow:
#
# rules = ['cd_parent', 'git_push', 'python_command', 'sudo']
#
# The default values are as follows. Uncomment and change to fit your needs.
# See https://github.com/nvbn/thefuck#settings for more information.
#

";

const DEFAULTS_DOC: &str = "# rules = [<const: All rules enabled>]
# exclude_rules = []
# wait_command = 3
# require_confirmation = True
# no_colors = False
# debug = False
# priority = {}
# history_limit = None
# alter_history = True
# wait_slow_command = 15
# slow_commands = ['lein', 'react-native', 'gradle', './gradlew', 'vagrant']
# repeat = False
# instant_mode = False
# num_close_matches = 3
# env = {'LC_ALL': 'C', 'LANG': 'C', 'GIT_TRACE': '1'}
# excluded_search_path_prefixes = []
# disabled_sources = []
# trusted_completers = []
# trusted_help = []
# probe_timeout = 3
# replay_for_diagnosis = False
# network_completion = False
";

impl Settings {
    pub fn is_rule_enabled(&self, name: &str, enabled_by_default: impl FnOnce() -> bool) -> bool {
        if self.exclude_rules.iter().any(|r| r == name) {
            return false;
        }
        self.rules.iter().any(|r| r == name) || (self.rules_all && enabled_by_default())
    }

    pub fn priority_for(&self, name: &str, default: i64) -> i64 {
        self.priority.get(name).copied().unwrap_or(default)
    }

    /// `settings.init(args)`.
    pub fn init(args: Option<&Args>) -> Settings {
        let mut settings = Settings::default();
        let user_dir = user_dir();
        setup_user_dir(&user_dir);
        match fs::read_to_string(user_dir.join("settings.py")) {
            Ok(src) => settings.apply_file(&src),
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                logs::warn(&format!("Can't load settings from file: {e}"));
            }
            Err(_) => {}
        }
        if let Err(e) = settings.apply_env(|k| env::var(k).ok()) {
            logs::warn(&format!("Can't load settings from env: {e}"));
        }
        if let Some(args) = args {
            if args.yes {
                settings.require_confirmation = false;
            }
            if args.debug {
                settings.debug = true;
            }
            if args.repeat {
                settings.repeat = true;
            }
        }
        settings
    }

    pub fn is_source_enabled(&self, name: &str) -> bool {
        !self.disabled_sources.iter().any(|s| s == name)
    }

    /// Applies `THEFUCK_*` (and notypo's own `NOTYPO_*`) variables. Like the
    /// Python dict comprehension, one bad value discards the whole env layer.
    pub fn apply_env(&mut self, var: impl Fn(&str) -> Option<String>) -> Result<(), String> {
        let mut next = self.clone();
        let int = |v: &str| -> Result<i64, String> {
            v.trim()
                .replace('_', "")
                .parse()
                .map_err(|_| format!("invalid literal for int(): '{v}'"))
        };
        let flag = |v: &str| v.eq_ignore_ascii_case("true");
        let list = |v: &str| v.split(':').map(str::to_owned).collect::<Vec<_>>();

        if let Some(v) = var("THEFUCK_RULES") {
            (next.rules_all, next.rules) = rules_from_env(&v);
        }
        if let Some(v) = var("THEFUCK_EXCLUDE_RULES") {
            next.exclude_rules = rules_from_env(&v).1;
        }
        if let Some(v) = var("THEFUCK_WAIT_COMMAND") {
            next.wait_command = int(&v)? as f64;
        }
        if let Some(v) = var("THEFUCK_REQUIRE_CONFIRMATION") {
            next.require_confirmation = flag(&v);
        }
        if let Some(v) = var("THEFUCK_NO_COLORS") {
            next.no_colors = flag(&v);
        }
        if let Some(v) = var("THEFUCK_DEBUG") {
            next.debug = flag(&v);
        }
        if let Some(v) = var("THEFUCK_PRIORITY") {
            next.priority = v
                .split(':')
                .filter_map(|part| {
                    let (rule, n) = part.split_once('=')?;
                    if n.contains('=') {
                        return None;
                    }
                    Some((rule.to_owned(), n.trim().parse().ok()?))
                })
                .collect();
        }
        if let Some(v) = var("THEFUCK_HISTORY_LIMIT") {
            next.history_limit = usize::try_from(int(&v)?).ok();
        }
        if let Some(v) = var("THEFUCK_ALTER_HISTORY") {
            next.alter_history = flag(&v);
        }
        if let Some(v) = var("THEFUCK_WAIT_SLOW_COMMAND") {
            next.wait_slow_command = int(&v)? as f64;
        }
        if let Some(v) = var("THEFUCK_SLOW_COMMANDS") {
            next.slow_commands = list(&v);
        }
        if let Some(v) = var("THEFUCK_REPEAT") {
            next.repeat = flag(&v);
        }
        if let Some(v) = var("THEFUCK_INSTANT_MODE") {
            next.instant_mode = flag(&v);
        }
        if let Some(v) = var("THEFUCK_NUM_CLOSE_MATCHES") {
            next.num_close_matches = usize::try_from(int(&v)?).unwrap_or(0);
        }
        if let Some(v) = var("THEFUCK_EXCLUDED_SEARCH_PATH_PREFIXES") {
            next.excluded_search_path_prefixes = list(&v);
        }
        if let Some(v) = var("NOTYPO_DISABLED_SOURCES") {
            next.disabled_sources = list(&v).into_iter().filter(|s| !s.is_empty()).collect();
        }
        if let Some(v) = var("NOTYPO_TRUSTED_COMPLETERS") {
            next.trusted_completers = list(&v).into_iter().filter(|s| !s.is_empty()).collect();
        }
        if let Some(v) = var("NOTYPO_TRUSTED_HELP") {
            next.trusted_help = list(&v).into_iter().filter(|s| !s.is_empty()).collect();
        }
        if let Some(v) = var("NOTYPO_PROBE_TIMEOUT") {
            next.probe_timeout = v
                .trim()
                .parse()
                .map_err(|_| format!("could not convert string to float: '{v}'"))?;
        }
        if let Some(v) = var("NOTYPO_REPLAY_FOR_DIAGNOSIS") {
            next.replay_for_diagnosis = flag(&v);
        }
        if let Some(v) = var("NOTYPO_NETWORK_COMPLETION") {
            next.network_completion = flag(&v);
        }
        *self = next;
        Ok(())
    }

    pub fn apply_file(&mut self, src: &str) {
        for (key, value) in pylit::assignments(src) {
            if !self.apply_value(&key, &value) {
                logs::warn(&format!(
                    "Can't load setting `{key}` from settings.py: unsupported value"
                ));
            }
        }
    }

    fn apply_value(&mut self, key: &str, v: &pylit::Value) -> bool {
        use pylit::Value as V;
        let strings = |v: &V| -> Option<Vec<String>> {
            match v {
                V::List(items) => items
                    .iter()
                    .map(|x| x.as_str().map(str::to_owned))
                    .collect(),
                _ => None,
            }
        };
        match key {
            "rules" | "exclude_rules" => {
                let V::List(items) = v else { return false };
                let mut all = false;
                let mut names = Vec::new();
                for item in items {
                    match item {
                        V::DefaultRules => all = true,
                        other => match other.as_str() {
                            Some(s) => names.push(s.to_owned()),
                            None => return false,
                        },
                    }
                }
                if key == "rules" {
                    (self.rules_all, self.rules) = (all, names);
                } else {
                    self.exclude_rules = names;
                }
            }
            "wait_command" | "wait_slow_command" => {
                let Some(n) = v.as_f64() else { return false };
                if key == "wait_command" {
                    self.wait_command = n;
                } else {
                    self.wait_slow_command = n;
                }
            }
            "require_confirmation" => self.require_confirmation = v.truthy(),
            "no_colors" => self.no_colors = v.truthy(),
            "debug" => self.debug = v.truthy(),
            "alter_history" => self.alter_history = v.truthy(),
            "repeat" => self.repeat = v.truthy(),
            "instant_mode" => self.instant_mode = v.truthy(),
            "priority" => {
                let V::Dict(items) = v else { return false };
                let mut out = HashMap::new();
                for (k, n) in items {
                    match (k.as_str(), n) {
                        (Some(k), V::Int(n)) => out.insert(k.to_owned(), *n),
                        _ => return false,
                    };
                }
                self.priority = out;
            }
            "history_limit" => match v {
                V::None => self.history_limit = None,
                V::Int(n) => self.history_limit = usize::try_from(*n).ok(),
                _ => return false,
            },
            "num_close_matches" => match v {
                V::Int(n) => self.num_close_matches = usize::try_from(*n).unwrap_or(0),
                _ => return false,
            },
            "slow_commands" | "excluded_search_path_prefixes" => {
                let Some(list) = strings(v) else { return false };
                if key == "slow_commands" {
                    self.slow_commands = list;
                } else {
                    self.excluded_search_path_prefixes = list;
                }
            }
            "env" => {
                let V::Dict(items) = v else { return false };
                let mut out = Vec::new();
                for (k, val) in items {
                    match (k.as_str(), val.as_str()) {
                        (Some(k), Some(val)) => out.push((k.to_owned(), val.to_owned())),
                        _ => return false,
                    }
                }
                self.env = out;
            }
            "fixlinecmd" => match v.as_str() {
                Some(s) => self.fixlinecmd = s.to_owned(),
                None => return false,
            },
            "disabled_sources" | "trusted_completers" | "trusted_help" => {
                let Some(list) = strings(v) else { return false };
                match key {
                    "disabled_sources" => self.disabled_sources = list,
                    "trusted_completers" => self.trusted_completers = list,
                    _ => self.trusted_help = list,
                }
            }
            "probe_timeout" => match v.as_f64() {
                Some(n) => self.probe_timeout = n,
                None => return false,
            },
            "replay_for_diagnosis" => self.replay_for_diagnosis = v.truthy(),
            "network_completion" => self.network_completion = v.truthy(),
            "fixcolcmd" => match v {
                V::None => self.fixcolcmd = None,
                other => match other.as_str() {
                    Some(s) => self.fixcolcmd = Some(s.to_owned()),
                    None => return false,
                },
            },
            _ => {}
        }
        true
    }
}

fn rules_from_env(v: &str) -> (bool, Vec<String>) {
    let all = v.split(':').any(|p| p == "DEFAULT_RULES");
    (
        all,
        v.split(':')
            .filter(|p| *p != "DEFAULT_RULES")
            .map(str::to_owned)
            .collect(),
    )
}

/// `$XDG_CONFIG_HOME/thefuck`, or the legacy `~/.thefuck` if it exists.
pub fn user_dir() -> PathBuf {
    let xdg = env::var("XDG_CONFIG_HOME").unwrap_or_else(|_| "~/.config".into());
    let user_dir = crate::utils::expand_user(&format!("{xdg}/thefuck"));
    let legacy = crate::utils::expand_user("~/.thefuck");
    if legacy.is_dir() {
        logs::warn(&format!(
            "Config path {} is deprecated. Please move to {}",
            legacy.display(),
            user_dir.display()
        ));
        return legacy;
    }
    user_dir
}

/// Creates `<user_dir>/rules` and a commented `settings.py`, like thefuck.
fn setup_user_dir(user_dir: &Path) {
    let rules = user_dir.join("rules");
    if !rules.is_dir() {
        let _ = fs::create_dir_all(&rules);
    }
    let settings = user_dir.join("settings.py");
    if !settings.exists() {
        let _ = fs::write(settings, format!("{SETTINGS_HEADER}{DEFAULTS_DOC}"));
    }
}

/// A parser for the Python literals found in `settings.py`.
pub mod pylit {
    #[derive(Debug, Clone, PartialEq)]
    pub enum Value {
        Str(String),
        Int(i64),
        Float(f64),
        Bool(bool),
        None,
        List(Vec<Value>),
        Dict(Vec<(Value, Value)>),
        /// The `DEFAULT_RULES` name (thefuck's `[ALL_ENABLED]`), flattened into lists.
        DefaultRules,
    }

    impl Value {
        pub fn as_str(&self) -> Option<&str> {
            match self {
                Value::Str(s) => Some(s),
                _ => None,
            }
        }

        pub fn as_f64(&self) -> Option<f64> {
            match *self {
                Value::Int(n) => Some(n as f64),
                Value::Float(f) => Some(f),
                _ => None,
            }
        }

        pub fn truthy(&self) -> bool {
            match self {
                Value::Bool(b) => *b,
                Value::Int(n) => *n != 0,
                Value::Float(f) => *f != 0.0,
                Value::None => false,
                Value::Str(s) => !s.is_empty(),
                Value::List(l) => !l.is_empty(),
                Value::Dict(d) => !d.is_empty(),
                Value::DefaultRules => true,
            }
        }
    }

    /// Every `name = <literal>` statement starting at column 0 that parses;
    /// anything else (imports, expressions) is skipped.
    pub fn assignments(src: &str) -> Vec<(String, Value)> {
        let mut out = Vec::new();
        let mut line_start = 0;
        while line_start < src.len() {
            let mut p = Parser {
                s: src,
                i: line_start,
                depth: 0,
            };
            let next_from = match p.assignment() {
                Some((name, value)) => {
                    out.push((name.to_owned(), value));
                    p.i
                }
                None => line_start,
            };
            match src[next_from..].find('\n') {
                Some(k) => line_start = next_from + k + 1,
                None => break,
            }
        }
        out
    }

    struct Parser<'a> {
        s: &'a str,
        i: usize,
        depth: usize,
    }

    impl<'a> Parser<'a> {
        fn peek(&self) -> Option<u8> {
            self.s.as_bytes().get(self.i).copied()
        }

        fn peek_at(&self, k: usize) -> Option<u8> {
            self.s.as_bytes().get(self.i + k).copied()
        }

        fn skip_ws(&mut self) {
            while let Some(c) = self.peek() {
                match c {
                    b' ' | b'\t' | b'\r' => self.i += 1,
                    b'\n' if self.depth > 0 => self.i += 1,
                    b'\\' if self.peek_at(1) == Some(b'\n') => self.i += 2,
                    b'#' => self.i += self.s[self.i..].find('\n').unwrap_or(self.s.len() - self.i),
                    _ => break,
                }
            }
        }

        fn ident(&mut self) -> Option<&'a str> {
            let start = self.i;
            while self
                .peek()
                .is_some_and(|c| c.is_ascii_alphanumeric() || c == b'_')
            {
                self.i += 1;
            }
            let id = &self.s[start..self.i];
            (!id.is_empty() && !id.as_bytes()[0].is_ascii_digit()).then_some(id)
        }

        fn assignment(&mut self) -> Option<(&'a str, Value)> {
            let name = self.ident()?;
            self.skip_ws();
            if self.peek()? != b'=' || self.peek_at(1) == Some(b'=') {
                return None;
            }
            self.i += 1;
            self.skip_ws();
            let value = self.expr()?;
            self.skip_ws();
            matches!(self.peek(), None | Some(b'\n' | b';')).then_some((name, value))
        }

        fn expr(&mut self) -> Option<Value> {
            let mut value = self.term()?;
            loop {
                self.skip_ws();
                if self.peek() != Some(b'+') {
                    return Some(value);
                }
                self.i += 1;
                self.skip_ws();
                value = match (value, self.term()?) {
                    (Value::List(mut a), Value::List(b)) => {
                        a.extend(b);
                        Value::List(a)
                    }
                    (Value::Int(a), Value::Int(b)) => Value::Int(a.checked_add(b)?),
                    (Value::Str(a), Value::Str(b)) => Value::Str(a + &b),
                    _ => return None,
                };
            }
        }

        fn sequence(&mut self, close: u8) -> Option<Vec<Value>> {
            self.i += 1;
            self.depth += 1;
            let mut items = Vec::new();
            loop {
                self.skip_ws();
                if self.peek()? == close {
                    self.i += 1;
                    break;
                }
                items.push(self.expr()?);
                self.skip_ws();
                match self.peek()? {
                    b',' => self.i += 1,
                    c if c == close => {}
                    _ => return None,
                }
            }
            self.depth -= 1;
            Some(items)
        }

        fn dict(&mut self) -> Option<Value> {
            self.i += 1;
            self.depth += 1;
            let mut items = Vec::new();
            loop {
                self.skip_ws();
                if self.peek()? == b'}' {
                    self.i += 1;
                    break;
                }
                let key = self.expr()?;
                self.skip_ws();
                if self.peek()? != b':' {
                    return None;
                }
                self.i += 1;
                self.skip_ws();
                items.push((key, self.expr()?));
                self.skip_ws();
                match self.peek()? {
                    b',' => self.i += 1,
                    b'}' => {}
                    _ => return None,
                }
            }
            self.depth -= 1;
            Some(Value::Dict(items))
        }

        fn term(&mut self) -> Option<Value> {
            match self.peek()? {
                b'[' => self.sequence(b']').map(Value::List),
                b'(' => self.sequence(b')').map(Value::List),
                b'{' => self.dict(),
                b'\'' | b'"' => self.strings(false),
                b'-' | b'+' | b'0'..=b'9' | b'.' => self.number(),
                _ => {
                    let start = self.i;
                    let id = self.ident()?;
                    if matches!(self.peek(), Some(b'\'' | b'"')) {
                        let prefix = id.to_ascii_lowercase();
                        return if matches!(prefix.as_str(), "r" | "u" | "b" | "br" | "rb") {
                            self.strings(prefix.contains('r'))
                        } else {
                            self.i = start;
                            None
                        };
                    }
                    match id {
                        "True" => Some(Value::Bool(true)),
                        "False" => Some(Value::Bool(false)),
                        "None" => Some(Value::None),
                        "DEFAULT_RULES" => Some(Value::List(vec![Value::DefaultRules])),
                        _ => None,
                    }
                }
            }
        }

        fn number(&mut self) -> Option<Value> {
            let start = self.i;
            if matches!(self.peek(), Some(b'-' | b'+')) {
                self.i += 1;
            }
            while self
                .peek()
                .is_some_and(|c| c.is_ascii_alphanumeric() || c == b'.' || c == b'_')
            {
                self.i += 1;
            }
            let text = self.s[start..self.i].replace('_', "");
            match text.parse::<i64>() {
                Ok(n) => Some(Value::Int(n)),
                Err(_) => text.parse().ok().map(Value::Float),
            }
        }

        /// One or more adjacent string literals (implicit concatenation).
        fn strings(&mut self, raw_first: bool) -> Option<Value> {
            let mut acc = self.string(raw_first)?;
            loop {
                self.skip_ws();
                let save = self.i;
                let raw = match self.peek() {
                    Some(b'\'' | b'"') => false,
                    Some(c) if c.is_ascii_alphabetic() => {
                        let prefix = self.ident().unwrap_or("").to_ascii_lowercase();
                        if !matches!(self.peek(), Some(b'\'' | b'"'))
                            || !matches!(prefix.as_str(), "r" | "u" | "b" | "br" | "rb")
                        {
                            self.i = save;
                            break;
                        }
                        prefix.contains('r')
                    }
                    _ => break,
                };
                acc.push_str(&self.string(raw)?);
            }
            Some(Value::Str(acc))
        }

        fn string(&mut self, raw: bool) -> Option<String> {
            let quote = self.peek()?;
            let triple = self.peek_at(1) == Some(quote) && self.peek_at(2) == Some(quote);
            self.i += if triple { 3 } else { 1 };
            let mut out = String::new();
            loop {
                let rest = &self.s[self.i..];
                let c = rest.chars().next()?;
                if c as u32 == quote as u32
                    && (!triple || rest.as_bytes().get(1..3) == Some(&[quote, quote]))
                {
                    self.i += if triple { 3 } else { 1 };
                    return Some(out);
                }
                if c == '\n' && !triple {
                    return None;
                }
                if c == '\\' && !raw {
                    let e = rest[1..].chars().next()?;
                    self.i += 1 + e.len_utf8();
                    match e {
                        'n' => out.push('\n'),
                        't' => out.push('\t'),
                        'r' => out.push('\r'),
                        '0' => out.push('\0'),
                        '\\' | '\'' | '"' => out.push(e),
                        '\n' => {}
                        'x' | 'u' | 'U' => {
                            let n = match e {
                                'x' => 2,
                                'u' => 4,
                                _ => 8,
                            };
                            let hex = self.s.get(self.i..self.i + n)?;
                            out.push(char::from_u32(u32::from_str_radix(hex, 16).ok()?)?);
                            self.i += n;
                        }
                        other => {
                            out.push('\\');
                            out.push(other);
                        }
                    }
                    continue;
                }
                out.push(c);
                self.i += c.len_utf8();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::pylit::{Value, assignments};
    use super::*;

    #[test]
    fn parses_generated_and_custom_settings() {
        let src = r#"# The Fuck settings file
#
# rules = ['cd_parent', 'git_push', 'python_command', 'sudo']
import os
rules = DEFAULT_RULES + ['my_rule']
exclude_rules = ["sudo",
                 'cd_parent',  # trailing comment
]
require_confirmation = False
wait_command = 10
wait_slow_command = 2.5
priority = {'no_command': 9999, "sudo": 10}
history_limit = None
env = {'LC_ALL': 'C', 'LANG': 'C', 'GIT_TRACE': '1', 'X': 'a\tb'}
num_close_matches = 5
weird = os.environ['HOME']
no_colors = True
"#;
        let mut s = Settings::default();
        s.apply_file(src);
        assert!(s.rules_all);
        assert_eq!(s.rules, ["my_rule"]);
        assert_eq!(s.exclude_rules, ["sudo", "cd_parent"]);
        assert!(!s.require_confirmation);
        assert_eq!(s.wait_command, 10.0);
        assert_eq!(s.wait_slow_command, 2.5);
        assert_eq!(s.priority_for("no_command", 3000), 9999);
        assert_eq!(s.priority_for("sudo", 1000), 10);
        assert_eq!(s.history_limit, None);
        assert_eq!(s.env.last(), Some(&("X".to_owned(), "a\tb".to_owned())));
        assert_eq!(s.num_close_matches, 5);
        assert!(s.no_colors);
        assert!(!s.is_rule_enabled("sudo", || true));
        assert!(s.is_rule_enabled("git_push", || true));
        assert!(!s.is_rule_enabled("git_push", || false));
        assert!(s.is_rule_enabled("my_rule", || false));
    }

    #[test]
    fn structured_engine_settings() {
        let mut s = Settings::default();
        s.apply_file(
            "disabled_sources = ['stderr']\ntrusted_completers = ['mytool']\ntrusted_help = ['cargo']\nprobe_timeout = 1.5\nreplay_for_diagnosis = True\n",
        );
        assert!(!s.is_source_enabled("stderr"));
        assert!(s.is_source_enabled("native"));
        assert_eq!(s.trusted_completers, ["mytool"]);
        assert_eq!(s.trusted_help, ["cargo"]);
        assert_eq!(s.probe_timeout, 1.5);
        assert!(s.replay_for_diagnosis);

        let env: HashMap<&str, &str> = [
            ("NOTYPO_DISABLED_SOURCES", "native:legacy"),
            ("NOTYPO_PROBE_TIMEOUT", "0.25"),
            ("NOTYPO_REPLAY_FOR_DIAGNOSIS", "false"),
        ]
        .into();
        s.apply_env(|k| env.get(k).map(|v| v.to_string())).unwrap();
        assert_eq!(s.disabled_sources, ["native", "legacy"]);
        assert_eq!(s.probe_timeout, 0.25);
        assert!(!s.replay_for_diagnosis);
        assert!(
            s.apply_env(|k| (k == "NOTYPO_PROBE_TIMEOUT").then(|| "soon".into()))
                .is_err()
        );
    }

    #[test]
    fn string_forms() {
        let v = assignments("a = 'x' \"y\" r'\\d'\nb = \"\"\"multi\nline\"\"\"\nc = (1, 2,)\n");
        assert_eq!(v[0], ("a".into(), Value::Str("xy\\d".into())));
        assert_eq!(v[1], ("b".into(), Value::Str("multi\nline".into())));
        assert_eq!(
            v[2],
            ("c".into(), Value::List(vec![Value::Int(1), Value::Int(2)]))
        );
    }

    #[test]
    fn env_layer_like_python() {
        let env: HashMap<&str, &str> = [
            ("THEFUCK_RULES", "DEFAULT_RULES:bash"),
            ("THEFUCK_EXCLUDE_RULES", "git:vim"),
            ("THEFUCK_WAIT_COMMAND", "55"),
            ("THEFUCK_REQUIRE_CONFIRMATION", "true"),
            ("THEFUCK_NO_COLORS", "false"),
            ("THEFUCK_PRIORITY", "bash=10:lisp=wrong:vim=15"),
            ("THEFUCK_WAIT_SLOW_COMMAND", "999"),
            ("THEFUCK_SLOW_COMMANDS", "lein:react-native:./gradlew"),
            ("THEFUCK_NUM_CLOSE_MATCHES", "359"),
            ("THEFUCK_EXCLUDED_SEARCH_PATH_PREFIXES", "/media/:/mnt/"),
        ]
        .into();
        let mut s = Settings::default();
        s.apply_env(|k| env.get(k).map(|v| v.to_string())).unwrap();
        assert!(s.rules_all);
        assert_eq!(s.rules, ["bash"]);
        assert_eq!(s.exclude_rules, ["git", "vim"]);
        assert_eq!(s.wait_command, 55.0);
        assert!(s.require_confirmation);
        assert!(!s.no_colors);
        assert_eq!(
            s.priority,
            HashMap::from([("bash".into(), 10), ("vim".into(), 15)])
        );
        assert_eq!(s.wait_slow_command, 999.0);
        assert_eq!(s.slow_commands, ["lein", "react-native", "./gradlew"]);
        assert_eq!(s.num_close_matches, 359);
        assert_eq!(s.excluded_search_path_prefixes, ["/media/", "/mnt/"]);

        let mut s = Settings::default();
        assert!(
            s.apply_env(|k| (k == "THEFUCK_WAIT_COMMAND").then(|| "nope".into()))
                .is_err()
        );
        assert_eq!(
            s,
            Settings::default(),
            "a bad value discards the whole env layer"
        );
    }
}
