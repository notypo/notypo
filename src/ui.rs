//! Interactive selection, port of `thefuck/ui.py` and the key reading in
//! `thefuck/system/unix.py`.

use crate::corrector::Corrector;
use crate::logs;
use crate::terminal::RawMode;
use crate::types::{CorrectedCommand, SideEffect};
use std::io::{IsTerminal, Read, Write};

#[derive(Debug, PartialEq, Eq)]
enum Action {
    Select,
    Abort,
    Previous,
    Next,
}

fn read_action(keys: &mut impl Iterator<Item = u8>) -> Action {
    loop {
        let Some(key) = keys.next() else {
            return Action::Abort;
        };
        match key {
            // ↑/↓ arrive as ESC [ A/B (or ESC O A/B in application mode).
            0x1b => {
                if matches!(keys.next(), Some(b'[' | b'O')) {
                    match keys.next() {
                        Some(b'A') => return Action::Previous,
                        Some(b'B') => return Action::Next,
                        _ => {}
                    }
                }
            }
            // k/e (qwerty/colemak) and Ctrl+P go up, j/n and Ctrl+N go down.
            b'k' | b'e' | 0x10 => return Action::Previous,
            b'j' | b'n' | 0x0e => return Action::Next,
            0x03 | b'q' => return Action::Abort,
            b'\n' | b'\r' => return Action::Select,
            _ => {}
        }
    }
}

/// One correction offered to the user.
#[derive(Clone, Debug)]
pub struct Choice {
    pub script: String,
    pub side_effect: Option<SideEffect>,
    /// Why it is proposed, shown under the prompt.
    pub reason: Option<String>,
    /// Why it can't run without explicit approval; empty when it can.
    pub concerns: Vec<String>,
}

impl PartialEq for Choice {
    fn eq(&self, other: &Self) -> bool {
        self.script == other.script
            && self.side_effect.map(|f| f as usize) == other.side_effect.map(|f| f as usize)
    }
}

impl From<CorrectedCommand> for Choice {
    fn from(c: CorrectedCommand) -> Choice {
        Choice {
            script: c.script,
            side_effect: c.side_effect,
            reason: None,
            concerns: Vec::new(),
        }
    }
}

/// Where the offered corrections come from. `rest` runs only when the user
/// asks for alternatives, so accepting the first one never pays for them.
pub trait Suggestions {
    fn first(&mut self) -> Option<Choice>;
    fn rest(&mut self, first: &Choice) -> Vec<Choice>;
}

impl Suggestions for Corrector<'_, '_> {
    fn first(&mut self) -> Option<Choice> {
        Corrector::first(self).map(Choice::from)
    }

    fn rest(&mut self, first: &Choice) -> Vec<Choice> {
        let first = CorrectedCommand {
            script: first.script.clone(),
            side_effect: first.side_effect,
            priority: 0,
            rule: "",
        };
        Corrector::rest(self, &first)
            .into_iter()
            .map(Choice::from)
            .collect()
    }
}

fn show(choice: &Choice) {
    let details: Vec<String> = choice
        .reason
        .iter()
        .cloned()
        .chain(
            choice
                .concerns
                .iter()
                .map(|c| format!("needs approval: {c}")),
        )
        .collect();
    if details.is_empty() {
        logs::confirm_text(&choice.script, choice.side_effect.is_some());
    } else {
        logs::confirm_details(&choice.script, choice.side_effect.is_some(), &details);
    }
}

/// `select_command`: the first suggestion when confirmation is off,
/// otherwise whatever the user picks (`None` on Ctrl+C or no suggestions).
/// A suggestion with concerns always needs a person to pick it: without a
/// terminal to ask on, nothing is selected.
pub fn select_command(
    suggestions: &mut dyn Suggestions,
    require_confirmation: bool,
    alias: &str,
) -> Option<Choice> {
    let Some(first) = suggestions.first() else {
        logs::failed(if alias == "fuck" {
            "No fucks given"
        } else {
            "Nothing found"
        });
        return None;
    };
    if !require_confirmation {
        if first.concerns.is_empty() {
            logs::show_corrected_command(&first.script, first.side_effect.is_some());
            return Some(first);
        }
        if !std::io::stdin().is_terminal() {
            logs::failed(&format!(
                "Not running `{}` without confirmation: {}",
                first.script,
                first.concerns.join("; ")
            ));
            return None;
        }
    }

    show(&first);
    let raw = RawMode::enter();
    let mut keys = std::io::stdin().lock().bytes().map_while(Result::ok);
    let mut commands = vec![first];
    let mut realised = false;
    let mut index = 0;
    loop {
        match read_action(&mut keys) {
            Action::Select => {
                logs::clear_details();
                drop(raw);
                let _ = std::io::stderr().write_all(b"\n");
                return Some(commands.swap_remove(index));
            }
            Action::Abort => {
                logs::clear_details();
                drop(raw);
                logs::failed("\nAborted");
                return None;
            }
            direction => {
                if !realised {
                    let rest = suggestions.rest(&commands[0]);
                    commands.extend(rest);
                    realised = true;
                }
                let n = commands.len();
                index = if direction == Action::Previous {
                    (index + n - 1) % n
                } else {
                    (index + 1) % n
                };
                show(&commands[index]);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn actions(input: &[u8]) -> Vec<Action> {
        let mut keys = input.iter().copied();
        let mut out = Vec::new();
        loop {
            let action = read_action(&mut keys);
            let done = action == Action::Abort;
            out.push(action);
            if done {
                return out;
            }
        }
    }

    #[test]
    fn keys_map_to_actions() {
        assert_eq!(
            actions(b"\x1b[A\x1b[Bkjen\x10\x0e\rq"),
            [
                Action::Previous,
                Action::Next,
                Action::Previous,
                Action::Next,
                Action::Previous,
                Action::Next,
                Action::Previous,
                Action::Next,
                Action::Select,
                Action::Abort
            ]
        );
        assert_eq!(actions(b"x\x03"), [Action::Abort]);
    }

    struct Fixed(Vec<Choice>);

    impl Suggestions for Fixed {
        fn first(&mut self) -> Option<Choice> {
            self.0.first().cloned()
        }
        fn rest(&mut self, _: &Choice) -> Vec<Choice> {
            self.0[1..].to_vec()
        }
    }

    fn choice(script: &str, concerns: &[&str]) -> Choice {
        Choice {
            script: script.into(),
            side_effect: None,
            reason: None,
            concerns: concerns.iter().map(|c| c.to_string()).collect(),
        }
    }

    #[test]
    fn confirmation_off_accepts_only_unconcerning_choices() {
        let mut ok = Fixed(vec![choice("git status", &[])]);
        assert_eq!(
            select_command(&mut ok, false, "fuck").map(|c| c.script),
            Some("git status".into())
        );
        // Test stdin is not a terminal: a concerning choice is refused.
        if !std::io::stdin().is_terminal() {
            let mut risky = Fixed(vec![choice("sudo rm -rf x", &["adds elevated privileges"])]);
            assert!(select_command(&mut risky, false, "fuck").is_none());
        }
        assert!(select_command(&mut Fixed(Vec::new()), false, "fuck").is_none());
    }
}
