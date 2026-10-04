//! `@sudo_support`: rules see the command without a leading `sudo `, and
//! their fixes get it back.

use crate::types::Command;

pub fn sudo_support_match(command: &Command, matches: impl FnOnce(&Command) -> bool) -> bool {
    match command.script.strip_prefix("sudo ") {
        Some(rest) => matches(&command.with_script(rest)),
        None => matches(command),
    }
}

pub fn sudo_support(
    command: &Command,
    get_new_command: impl FnOnce(&Command) -> Vec<String>,
) -> Vec<String> {
    match command.script.strip_prefix("sudo ") {
        Some(rest) => get_new_command(&command.with_script(rest))
            .into_iter()
            .map(|fix| format!("sudo {fix}"))
            .collect(),
        None => get_new_command(command),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_and_restores_sudo() {
        let cmd = Command::test("sudo ls -l", "");
        assert!(sudo_support_match(&cmd, |c| c.script == "ls -l"));
        assert_eq!(
            sudo_support(&cmd, |c| vec![format!("{} -a", c.script)]),
            ["sudo ls -l -a"]
        );
        let cmd = Command::test("ls", "");
        assert_eq!(sudo_support(&cmd, |c| vec![c.script.clone()]), ["ls"]);
    }
}
