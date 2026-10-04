//! Interactive selection, port of `thefuck/ui.py` and the key reading in
//! `thefuck/system/unix.py`.

use crate::corrector::Corrector;
use crate::logs;
use crate::terminal::RawMode;
use crate::types::CorrectedCommand;
use std::io::{Read, Write};

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

/// `select_command`: the first suggestion when confirmation is off,
/// otherwise whatever the user picks (`None` on Ctrl+C or no suggestions).
pub fn select_command(
    corrector: &mut Corrector,
    require_confirmation: bool,
    alias: &str,
) -> Option<CorrectedCommand> {
    let Some(first) = corrector.first() else {
        logs::failed(if alias == "fuck" {
            "No fucks given"
        } else {
            "Nothing found"
        });
        return None;
    };
    if !require_confirmation {
        logs::show_corrected_command(&first.script, first.side_effect.is_some());
        return Some(first);
    }

    logs::confirm_text(&first.script, first.side_effect.is_some());
    let raw = RawMode::enter();
    let mut keys = std::io::stdin().lock().bytes().map_while(Result::ok);
    let mut commands = vec![first];
    let mut realised = false;
    let mut index = 0;
    loop {
        match read_action(&mut keys) {
            Action::Select => {
                drop(raw);
                let _ = std::io::stderr().write_all(b"\n");
                return Some(commands.swap_remove(index));
            }
            Action::Abort => {
                drop(raw);
                logs::failed("\nAborted");
                return None;
            }
            direction => {
                if !realised {
                    let rest = corrector.rest(&commands[0]);
                    commands.extend(rest);
                    realised = true;
                }
                let n = commands.len();
                index = if direction == Action::Previous {
                    (index + n - 1) % n
                } else {
                    (index + 1) % n
                };
                logs::confirm_text(
                    &commands[index].script,
                    commands[index].side_effect.is_some(),
                );
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
}
