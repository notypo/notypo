//! Port of `thefuck/utils.py`: fuzzy matching, argument replacement, `which`,
//! and the on-disk cache used for slow subprocess-derived command lists.

use crate::difflib;
use crate::types::Command;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::UNIX_EPOCH;
use std::{env, fs};

/// `os.path.expanduser` for `~` and `~/...`.
pub fn expand_user(path: &str) -> PathBuf {
    let home = || {
        env::var("HOME")
            .or_else(|_| env::var("USERPROFILE"))
            .unwrap_or_else(|_| "/".into())
    };
    if path == "~" {
        PathBuf::from(home())
    } else if let Some(rest) = path.strip_prefix("~/") {
        Path::new(&home()).join(rest)
    } else {
        PathBuf::from(path)
    }
}

/// `get_alias()`: the name the shell alias was installed under.
pub fn get_alias() -> String {
    env::var("TF_ALIAS").unwrap_or_else(|_| "fuck".into())
}

/// `shutil.which(program)`, memoized for the life of the process.
pub fn which(program: &str) -> Option<PathBuf> {
    static MEMO: LazyLock<Mutex<HashMap<String, Option<PathBuf>>>> =
        LazyLock::new(Default::default);
    if let Some(hit) = MEMO.lock().ok().and_then(|m| m.get(program).cloned()) {
        return hit;
    }
    let found = which_uncached(program);
    if let Ok(mut m) = MEMO.lock() {
        m.insert(program.to_owned(), found.clone());
    }
    found
}

/// `os.path.exists(p) and os.access(p, X_OK) and not os.path.isdir(p)`.
pub fn is_executable_file(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|m| m.is_file()) && has_execute_permission(path)
}

pub fn has_execute_permission(path: &Path) -> bool {
    #[cfg(unix)]
    {
        access(path, libc::X_OK)
    }
    #[cfg(windows)]
    {
        windows_executable_name(&path.to_string_lossy()).is_some()
    }
}

/// `os.access(path, mode)`.
#[cfg(unix)]
pub fn access(path: &Path, mode: libc::c_int) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c_path) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    // SAFETY: `c_path` is a valid NUL-terminated string.
    unsafe { libc::access(c_path.as_ptr(), mode) == 0 }
}

#[cfg(unix)]
fn which_uncached(program: &str) -> Option<PathBuf> {
    if program.contains('/') {
        let p = PathBuf::from(program);
        return is_executable_file(&p).then_some(p);
    }
    let path = env::var_os("PATH").unwrap_or_else(|| "/bin:/usr/bin".into());
    if path.is_empty() {
        return None;
    }
    let mut seen = Vec::new();
    env::split_paths(&path)
        .filter(|d| {
            !seen.contains(d) && {
                seen.push(d.clone());
                true
            }
        })
        .find_map(|dir| {
            let candidate = dir.join(program);
            is_executable_file(&candidate).then_some(candidate)
        })
}

#[cfg(windows)]
pub(crate) fn windows_executable_name(name: &str) -> Option<String> {
    let extensions = env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
    extensions
        .split(';')
        .filter(|ext| !ext.is_empty())
        .find_map(|ext| {
            let split = name.len().checked_sub(ext.len())?;
            name.get(split..)
                .filter(|suffix| suffix.eq_ignore_ascii_case(ext))
                .map(|_| name[..split].to_owned())
        })
}

#[cfg(windows)]
fn which_uncached(program: &str) -> Option<PathBuf> {
    let extensions = env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
    let candidates = |dir: &Path| {
        let path = dir.join(program);
        if is_executable_file(&path) {
            return Some(path);
        }
        extensions
            .split(';')
            .filter(|ext| !ext.is_empty())
            .map(|ext| dir.join(format!("{program}{ext}")))
            .find(|path| is_executable_file(path))
    };
    if program.contains(['/', '\\']) || Path::new(program).is_absolute() {
        return candidates(Path::new(""));
    }
    let path = env::var_os("PATH").unwrap_or_default();
    env::split_paths(&path).find_map(|dir| candidates(&dir))
}

/// `get_closest(word, possibilities)`: the best match above 0.6, falling
/// back to the first possibility.
pub fn get_closest<S: AsRef<str>>(word: &str, possibilities: &[S]) -> Option<String> {
    closest_match(word, possibilities, 0.6)
        .or_else(|| possibilities.first().map(|s| s.as_ref().to_owned()))
}

/// `get_closest(word, possibilities, cutoff, fallback_to_first=False)`.
pub fn closest_match<S: AsRef<str>>(
    word: &str,
    possibilities: &[S],
    cutoff: f64,
) -> Option<String> {
    get_close_matches(word, possibilities, 1, cutoff)
        .into_iter()
        .next()
}

/// `difflib.get_close_matches` with owned results.
pub fn get_close_matches<S: AsRef<str>>(
    word: &str,
    possibilities: &[S],
    n: usize,
    cutoff: f64,
) -> Vec<String> {
    difflib::get_close_matches(word, possibilities.iter().map(AsRef::as_ref), n, cutoff)
        .into_iter()
        .map(str::to_owned)
        .collect()
}

/// `utils.replace_argument`: replaces a whole argument, preferring the last one.
pub fn replace_argument(script: &str, from: &str, to: &str) -> String {
    // re.sub(' {from}$', ' {to}', script, count=1), where `$` also matches
    // right before a final newline.
    let end = format!(" {from}");
    let (body, newline) = match script.strip_suffix('\n') {
        Some(b) if b.ends_with(&end) && !script.ends_with(&end) => (b, "\n"),
        _ => (script, ""),
    };
    if let Some(head) = body.strip_suffix(&end) {
        let replaced = format!("{head} {to}{newline}");
        if replaced != script {
            return replaced;
        }
    }
    script.replacen(&format!(" {from} "), &format!(" {to} "), 1)
}

/// `utils.get_all_matched_commands`: lines after any of `separators`.
pub fn get_all_matched_commands(output: &str, separators: &[&str]) -> Vec<String> {
    let mut should_yield = false;
    let mut commands = Vec::new();
    for line in output.split('\n') {
        if separators.iter().any(|sep| line.contains(sep)) {
            should_yield = true;
        } else if should_yield && !line.is_empty() {
            commands.push(py_strip(line).to_owned());
        }
    }
    commands
}

/// `utils.replace_command`: up to `num_close_matches` fixes for `broken`.
pub fn replace_command<S: AsRef<str>>(
    command: &Command,
    broken: &str,
    matched: &[S],
) -> Vec<String> {
    get_close_matches(broken, matched, command.settings().num_close_matches, 0.1)
        .iter()
        .map(|m| replace_argument(&command.script, broken, py_strip(m)))
        .collect()
}

/// `re.findall(pattern, text)[0]`: the first match's first group (or the
/// whole match when the pattern has no groups).
pub fn capture<'t>(re: &regex::Regex, text: &'t str) -> Option<&'t str> {
    let caps = re.captures(text)?;
    caps.get(1).or_else(|| caps.get(0)).map(|m| m.as_str())
}

/// `re.findall(pattern, text)` for patterns with at most one group.
pub fn capture_all<'t>(re: &regex::Regex, text: &'t str) -> Vec<&'t str> {
    re.captures_iter(text)
        .filter_map(|caps| caps.get(1).or_else(|| caps.get(0)))
        .map(|m| m.as_str())
        .collect()
}

/// Python's `str.strip()`: Unicode whitespace plus the \x1c-\x1f separators.
pub fn py_strip(s: &str) -> &str {
    s.trim_matches(|c: char| c.is_whitespace() || ('\x1c'..='\x1f').contains(&c))
}

/// `str.split()` with no arguments.
pub fn py_split(s: &str) -> impl Iterator<Item = &str> {
    s.split(|c: char| c.is_whitespace() || ('\x1c'..='\x1f').contains(&c))
        .filter(|p| !p.is_empty())
}

/// `format_raw_script`: joins argv parts into one script.
pub fn format_raw_script<S: AsRef<str>>(raw: &[S]) -> String {
    let joined = raw.iter().map(AsRef::as_ref).collect::<Vec<_>>().join(" ");
    joined
        .trim_start_matches(|c: char| c.is_whitespace() || ('\x1c'..='\x1f').contains(&c))
        .to_owned()
}

pub fn include_path_in_search(path: &str, excluded_prefixes: &[String]) -> bool {
    !excluded_prefixes
        .iter()
        .any(|prefix| path.starts_with(prefix.as_str()))
}

/// `system.open_command`: the platform's "open this URL" command.
pub fn open_command(arg: &str) -> String {
    #[cfg(windows)]
    {
        format!("cmd /c start \"\" {arg}")
    }
    #[cfg(unix)]
    if which("xdg-open").is_some() {
        format!("xdg-open {arg}")
    } else {
        format!("open {arg}")
    }
}

/// Runs `program args`, returning its stdout as text (stderr discarded).
pub fn run_stdout(program: &str, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Like [`run_stdout`] but returns stdout and stderr separately.
pub fn run_output(program: &str, args: &[&str]) -> Option<(String, String)> {
    let out = std::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    Some((
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    ))
}

pub(crate) fn cache_dir() -> PathBuf {
    match env::var("XDG_CACHE_HOME") {
        Ok(d) if !d.is_empty() => PathBuf::from(d).join("notypo"),
        _ => expand_user("~/.cache/notypo"),
    }
}

/// `@cache(*depends_on)`: memoizes a string list on disk, invalidated when
/// the modification time of any file in `depends_on` changes.
pub fn cached(
    key: &str,
    depends_on: &[PathBuf],
    compute: impl FnOnce() -> Vec<String>,
) -> Vec<String> {
    if cfg!(test) || env::var_os("NOTYPO_NO_CACHE").is_some() {
        return compute();
    }
    let etag: Vec<String> = depends_on
        .iter()
        .map(|p| {
            let path = std::path::absolute(expand_user(&p.to_string_lossy()))
                .unwrap_or_else(|_| p.clone());
            let mtime = fs::metadata(&path).and_then(|m| m.modified()).ok();
            let nanos = mtime
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_nanos());
            format!("{}={nanos}", path.display())
        })
        .collect();
    let etag = etag.join(";");
    let safe_key: String = key
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let file = cache_dir().join(format!("cache-{safe_key}"));
    if let Ok(text) = fs::read_to_string(&file)
        && let Some((head, body)) = text.split_once('\n')
        && head == etag
    {
        return body.lines().map(str::to_owned).collect();
    }
    let value = compute();
    if value.iter().all(|v| !v.contains('\n')) {
        let _ = fs::create_dir_all(cache_dir());
        let mut text = etag;
        for v in &value {
            text.push('\n');
            text.push_str(v);
        }
        let tmp = file.with_extension(format!("tmp{}", std::process::id()));
        if fs::write(&tmp, text).is_ok() {
            let _ = fs::rename(&tmp, &file);
        }
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closest() {
        assert_eq!(
            get_closest("brnch", &["branch", "status"]).as_deref(),
            Some("branch")
        );
        assert_eq!(
            get_closest("st", &["status", "reset"]).as_deref(),
            Some("status")
        );
        assert_eq!(closest_match("st", &["status", "reset"], 0.6), None);
    }

    #[test]
    fn replace_argument_like_python() {
        assert_eq!(
            replace_argument("apt-get instol vim", "instol", "install"),
            "apt-get install vim"
        );
        assert_eq!(
            replace_argument("git brnch", "brnch", "branch"),
            "git branch"
        );
        assert_eq!(
            replace_argument("git brnch\n", "brnch", "branch"),
            "git branch\n"
        );
    }

    #[test]
    fn matched_commands_like_python() {
        let out = "git: 're' is not a git command. See 'git --help'.\n\nDid you mean one of these?\n\trebase\n\treset\n\tgrep\n\trm";
        assert_eq!(
            get_all_matched_commands(out, &["Did you mean"]),
            ["rebase", "reset", "grep", "rm"]
        );
        let out = "tsuru: \"target\" is not a tsuru command. See \"tsuru help\".\n\nDid you mean one of these?\n\tservice-add\n\tservice-bind";
        assert_eq!(
            get_all_matched_commands(out, &["Did you mean"]),
            ["service-add", "service-bind"]
        );
    }

    #[test]
    #[cfg(unix)]
    fn which_finds_sh() {
        assert!(which("sh").is_some());
        assert!(which("definitely-not-a-command-xyz").is_none());
        assert_eq!(which("/bin/sh"), Some(PathBuf::from("/bin/sh")));
        assert!(which("/bin").is_none());
    }

    #[test]
    fn raw_script() {
        assert_eq!(format_raw_script(&["git", "push"]), "git push");
        assert_eq!(format_raw_script(&["  ls", "-la"]), "ls -la");
    }
}
