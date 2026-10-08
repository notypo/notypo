//! Readers for existing terminal output. These paths never rerun a command.

use crate::{logs, shlex, terminal};
use std::env;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
#[cfg(unix)]
use std::io::{BufRead, BufReader, Write};
#[cfg(unix)]
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::time::Duration;

pub(crate) const LOG_SIZE: usize = 1024 * 1024;

#[cfg(unix)]
pub(super) fn logger_socket() -> Option<PathBuf> {
    env::var_os("SHELL_LOGGER_SOCKET")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .filter(|path| path.exists())
}

#[cfg(windows)]
pub(super) fn logger_socket() -> Option<PathBuf> {
    None
}

pub(super) fn from_logger(script: &str, socket: &Path) -> Option<String> {
    match read_logger(script, socket) {
        Ok(output) => output.or_else(|| {
            logs::warn("Output isn't available in shell logger");
            None
        }),
        Err(error) => {
            logs::warn(&format!("Can't read output from shell logger: {error}"));
            None
        }
    }
}

#[cfg(windows)]
fn read_logger(_script: &str, _socket: &Path) -> io::Result<Option<String>> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "Unix shell logger sockets are unavailable on Windows",
    ))
}

#[cfg(unix)]
fn read_logger(script: &str, socket: &Path) -> io::Result<Option<String>> {
    let mut client = UnixStream::connect(socket)?;
    client.set_read_timeout(Some(Duration::from_secs(1)))?;
    client.set_write_timeout(Some(Duration::from_secs(1)))?;
    client.write_all(b"{\"type\":\"list\",\"count\":5}\n")?;
    let mut response = String::new();
    BufReader::new(client)
        .take(LOG_SIZE as u64 + 1)
        .read_line(&mut response)?;
    if response.len() > LOG_SIZE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "logger response is too large",
        ));
    }
    let response: serde_json::Value = serde_json::from_str(&response)?;
    let output = response
        .get("commands")
        .and_then(serde_json::Value::as_array)
        .and_then(|commands| {
            commands.iter().find_map(|command| {
                (command.get("command").and_then(serde_json::Value::as_str) == Some(script))
                    .then(|| command.get("output").and_then(serde_json::Value::as_str))
                    .flatten()
            })
        });
    Ok(output.map(render_terminal))
}

pub(super) fn from_log(script: &str) -> Option<String> {
    let Some(path) = env::var_os("THEFUCK_OUTPUT_LOG") else {
        logs::warn("Output log isn't specified");
        return None;
    };
    let prompt = env::var("PS1").unwrap_or_default();
    if !prompt.contains(logs::USER_COMMAND_MARK) {
        logs::warn(
            "PS1 doesn't contain user command mark, please ensure that PS1 is not changed after the alias initialization",
        );
        return None;
    }
    let log = match read_log(Path::new(&path)) {
        Ok(log) => log,
        Err(error) => {
            logs::warn(&format!("Can't read output log: {error}"));
            return None;
        }
    };
    let output = output_for_script(script, &prompt, &log, &crate::utils::get_alias());
    if output.is_none() {
        logs::warn("The last command in the output log isn't the one being corrected");
    }
    output
}

fn read_log(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let size = file.metadata()?.len();
    if size > LOG_SIZE as u64 {
        file.seek(SeekFrom::Start(size - LOG_SIZE as u64))?;
    }
    let mut data = Vec::new();
    file.take(LOG_SIZE as u64).read_to_end(&mut data)?;
    // Rolling logs are preallocated. A clipped UTF-8 sequence at the beginning
    // of a large log does not invalidate later command boundaries.
    Ok(String::from_utf8_lossy(&data)
        .trim_end_matches('\0')
        .to_owned())
}

/// The output of the last command before the correction call, when that
/// command is `script`. An older run of the same command, or a different
/// command, never lends its output: it may have failed differently.
fn output_for_script(script: &str, prompt: &str, log: &str, alias: &str) -> Option<String> {
    let first = script.lines().next()?.trim();
    if first.is_empty() || shlex::split(script).is_err() {
        return None;
    }
    let prompt_lines = prompt.matches("\\n").count() + prompt.matches('\n').count();
    let mut continuation = 0;
    let mut groups: Vec<(&str, Vec<&str>)> = Vec::new();
    for line in log.split('\n') {
        if line.contains(logs::USER_COMMAND_MARK) || continuation > 0 {
            if continuation == 0 {
                groups.push((line, vec![line]));
            } else if let Some((script_line, lines)) = groups.last_mut() {
                *script_line = line;
                *lines = vec![line];
            }
            if prompt_lines > 0 {
                continuation = if continuation == 0 {
                    prompt_lines
                } else {
                    continuation - 1
                };
            }
        } else if let Some((_, lines)) = groups.last_mut() {
            lines.push(line);
        }
    }
    let (_, lines) = groups
        .iter()
        .rev()
        .find(|(script_line, _)| !is_correction_call(&render_terminal(script_line), alias))?;
    let typed = render_terminal(lines[0]);
    names(&typed, first).then(|| render_terminal(&lines.join("\n")))
}

/// Whether the visible prompt line contains `command` as whole words.
/// Prompts can surround it (a right-hand prompt), but `ls` is not `lsof`.
fn names(line: &str, command: &str) -> bool {
    let word = |c: char| c.is_alphanumeric() || matches!(c, '-' | '_' | '.' | '/');
    line.match_indices(command).any(|(at, _)| {
        !line[..at].chars().next_back().is_some_and(word)
            && line[at + command.len()..]
                .chars()
                .next()
                .is_none_or(char::is_whitespace)
    })
}

/// A prompt line running the correction alias itself, possibly with flags.
fn is_correction_call(line: &str, alias: &str) -> bool {
    let words: Vec<&str> = line.split_whitespace().collect();
    words
        .iter()
        .rposition(|w| *w == alias || crate::shells::DEFAULT_ALIASES.contains(w))
        .is_some_and(|at| words[at + 1..].iter().all(|w| w.starts_with('-')))
}

fn render_terminal(output: &str) -> String {
    render_terminal_at_width(output, terminal::size().ws_col.clamp(2, 4096))
}

fn render_terminal_at_width(output: &str, cols: u16) -> String {
    // Wrapped lines need additional screen rows. UTF-8 byte lengths bound
    // character widths; tabs can advance up to eight columns. Keep a spare
    // row so a one-line screen never hits vt100's scroll-on-wrap edge case.
    let rows = output
        .split('\n')
        .fold(1usize, |rows, line| {
            let columns = line.bytes().fold(0usize, |columns, byte| {
                columns.saturating_add(if byte == b'\t' { 8 } else { 1 })
            });
            rows.saturating_add(columns.div_ceil(usize::from(cols)).max(1))
        })
        .clamp(2, 4096) as u16;
    let mut parser = vt100::Parser::new(rows, cols, 0);
    // Text-only loggers can supply LF without the CR normally emitted by a
    // PTY. Treat both forms as the line endings visible on the terminal.
    parser.process(output.replace('\n', "\r\n").as_bytes());
    parser.screen().contents().trim().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::os::unix::net::UnixListener;

    #[test]
    fn renders_control_sequences_and_unicode() {
        assert_eq!(
            render_terminal("old\r\x1b[2K\x1b[31mpermission denied\x1b[0m\r\nλσ"),
            "permission denied\nλσ"
        );
        assert_eq!(render_terminal("a\nb"), "a\nb");
    }

    #[test]
    fn renders_wrapped_lines_without_losing_text_or_panicking() {
        let command = format!(
            "aws ec2 describe-instances {}",
            "--filter 'a b' ".repeat(30)
        );
        assert_eq!(render_terminal_at_width(&command, 80), command.trim());
        let unicode = "õun/üks ".repeat(60);
        assert_eq!(render_terminal_at_width(&unicode, 80), unicode.trim());
        assert_eq!(render_terminal_at_width("a\tend", 80), "a       end");
    }

    #[test]
    fn matches_captured_commands_longer_than_the_terminal_width() {
        let command = format!(
            "aws ec2 describ-instances --filter '{}'",
            "a b ".repeat(1100)
        );
        let mark = logs::USER_COMMAND_MARK;
        let log = format!("{mark}$ {command}\r\nunknown command\r\n{mark}$ fuck");
        assert_eq!(
            output_for_script(&command, mark, &log, "fuck"),
            Some(format!("$ {command}\nunknown command"))
        );
    }

    #[test]
    fn chooses_newest_record_and_handles_multiline_prompts() {
        let mark = logs::USER_COMMAND_MARK;
        let log = format!(
            "{mark}$ git push\r\nold error\r\n{mark}$ ls\r\na\r\n{mark}$ git push\r\nnew error\r\n{mark}$ fuck"
        );
        let output = output_for_script("git push", mark, &log, "fuck").unwrap();
        assert!(output.contains("new error"));
        assert!(!output.contains("old error"));
        assert!(!output.contains("fuck"));
        let log = format!("{mark}path\r\n$ git push\r\nnew error\r\n{mark}path\r\n$ fuck");
        assert_eq!(
            output_for_script("git push", &format!("{mark}path\\n$ "), &log, "fuck").as_deref(),
            Some("$ git push\nnew error")
        );
        assert!(output_for_script("git pull", mark, &log, "fuck").is_none());
    }

    #[test]
    fn output_belongs_to_the_event_right_before_the_call() {
        let mark = logs::USER_COMMAND_MARK;
        let log =
            format!("{mark}$ ls\r\nmissing\r\n{mark}$ lsof -i\r\nCOMMAND PID\r\n{mark}$ fuck -y");
        assert_eq!(
            output_for_script("lsof -i", mark, &log, "fuck").as_deref(),
            Some("$ lsof -i\nCOMMAND PID")
        );
        assert!(
            output_for_script("ls", mark, &log, "fuck").is_none(),
            "neither a later `lsof` nor an older `ls` lends its output"
        );
        // A right-hand prompt and line-editor redraws around the command.
        let log = format!(
            "{mark}~ ❯ \x1b[32mgit\x1b[39m pss\x08h  [12:01]\r\ngit: 'psh' is not a git command\r\n{mark}~ ❯ fix"
        );
        assert_eq!(
            output_for_script("git psh", mark, &log, "fix").as_deref(),
            Some("~ ❯ git psh  [12:01]\ngit: 'psh' is not a git command")
        );
        // A pasted block: each line has its own prompt.
        let log = format!(
            "{mark}$ cd /tmp\r\n{mark}$ gti status\r\ngti: command not found\r\n{mark}$ fuck"
        );
        assert_eq!(
            output_for_script("gti status", mark, &log, "fuck").as_deref(),
            Some("$ gti status\ngti: command not found")
        );
    }

    #[test]
    #[cfg(unix)]
    fn reads_matching_record_from_socket() {
        let path = env::temp_dir().join(format!("notypo-logger-test-{}.sock", std::process::id()));
        let listener = UnixListener::bind(&path).unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = String::new();
            BufReader::new(stream.try_clone().unwrap())
                .read_line(&mut request)
                .unwrap();
            let request: serde_json::Value = serde_json::from_str(&request).unwrap();
            assert_eq!(request["count"], 5);
            stream.write_all(b"{\"commands\":[{\"command\":\"ls\",\"output\":\"other\"},{\"command\":\"git push\",\"output\":\"fatal: no upstream\\n\"}]}\n").unwrap();
        });
        assert_eq!(
            read_logger("git push", &path).unwrap().as_deref(),
            Some("fatal: no upstream")
        );
        server.join().unwrap();
        fs_cleanup(&path);
    }

    #[cfg(unix)]
    fn fs_cleanup(path: &Path) {
        std::fs::remove_file(path).unwrap();
    }
}
