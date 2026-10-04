//! Colored terminal output, port of `thefuck/logs.py`.

use std::fmt::Display;
use std::io::{IsTerminal, Write};
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::time::Instant;

static COLORS: AtomicBool = AtomicBool::new(true);
static DEBUG: AtomicBool = AtomicBool::new(false);

/// Ten zero-width spaces; instant mode uses them to find commands in the log.
pub const USER_COMMAND_MARK: &str =
    "\u{200B}\u{200B}\u{200B}\u{200B}\u{200B}\u{200B}\u{200B}\u{200B}\u{200B}\u{200B}";

const BRIGHT: &str = "\x1b[1m";
const RESET: &str = "\x1b[0m";
const RED: &str = "\x1b[31m";
const GREEN: &str = "\x1b[32m";
const BLUE: &str = "\x1b[34m";
const WARN: &str = "\x1b[41m\x1b[37m\x1b[1m";

/// Applies `no_colors`/`debug` settings. Like colorama, colors are also
/// stripped when stderr isn't a terminal.
pub fn configure(no_colors: bool, debug: bool) {
    COLORS.store(!no_colors && std::io::stderr().is_terminal(), Relaxed);
    DEBUG.store(debug, Relaxed);
}

fn color(code: &'static str) -> &'static str {
    if COLORS.load(Relaxed) { code } else { "" }
}

pub fn debug_enabled() -> bool {
    DEBUG.load(Relaxed)
}

fn emit(s: &str) {
    let _ = std::io::stderr().lock().write_all(s.as_bytes());
}

pub fn warn(title: &str) {
    emit(&format!("{}[WARN] {title}{}\n", color(WARN), color(RESET)));
}

pub fn rule_failed(rule: &str, error: impl Display) {
    emit(&format!(
        "{w}[WARN] Rule {rule}:{r}\n{error}\n{w}----------------------------{r}\n\n",
        w = color(WARN),
        r = color(RESET)
    ));
}

pub fn failed(msg: &str) {
    emit(&format!("{}{msg}{}\n", color(RED), color(RESET)));
}

fn side_effect_suffix(side_effect: bool) -> &'static str {
    if side_effect { " (+side effect)" } else { "" }
}

pub fn show_corrected_command(script: &str, side_effect: bool) {
    emit(&format!(
        "{USER_COMMAND_MARK}{}{script}{}{}\n",
        color(BRIGHT),
        color(RESET),
        side_effect_suffix(side_effect)
    ));
}

pub fn confirm_text(script: &str, side_effect: bool) {
    let (b, r, g, bl, rd) = (
        color(BRIGHT),
        color(RESET),
        color(GREEN),
        color(BLUE),
        color(RED),
    );
    emit(&format!(
        "{USER_COMMAND_MARK}\x1b[1K\r{b}{script}{r}{} [{g}enter{r}/{bl}↑{r}/{bl}↓{r}/{rd}ctrl+c{r}]",
        side_effect_suffix(side_effect)
    ));
}

pub fn debug(msg: impl Display) {
    if debug_enabled() {
        emit(&format!(
            "{}{}DEBUG:{} {msg}\n",
            color(BLUE),
            color(BRIGHT),
            color(RESET)
        ));
    }
}

/// `with logs.debug_time(msg):` — logs how long `f` took when debugging.
pub fn debug_time<T>(msg: impl Display, f: impl FnOnce() -> T) -> T {
    if !debug_enabled() {
        return f();
    }
    let started = Instant::now();
    let result = f();
    debug(format_args!("{msg} took: {:?}", started.elapsed()));
    result
}

pub fn how_to_configure_alias(details: Option<&crate::shells::ShellConfiguration>) {
    let (b, r) = (color(BRIGHT), color(RESET));
    let mut out = format!("Seems like {b}fuck{r} alias isn't configured!\n");
    if let Some(d) = details {
        out += &format!(
            "Please put {b}{}{r} in your {b}{}{r} and apply changes with {b}{}{r} or restart your shell.\n",
            d.content, d.path, d.reload
        );
        if d.can_configure_automatically {
            out += &format!("Or run {b}fuck{r} a second time to configure it automatically.\n");
        }
    }
    out += "More details - https://github.com/nvbn/thefuck#manual-installation\n";
    print!("{out}");
}

pub fn already_configured(details: &crate::shells::ShellConfiguration) {
    let (b, r) = (color(BRIGHT), color(RESET));
    println!(
        "Seems like {b}fuck{r} alias already configured!\nFor applying changes run {b}{}{r} or restart your shell.",
        details.reload
    );
}

pub fn configured_successfully(details: &crate::shells::ShellConfiguration) {
    let (b, r) = (color(BRIGHT), color(RESET));
    println!(
        "{b}fuck{r} alias configured successfully!\nFor applying changes run {b}{}{r} or restart your shell.",
        details.reload
    );
}
