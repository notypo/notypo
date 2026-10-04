//! `@git_support`: only for `git`/`hub`, with git aliases expanded using
//! the `GIT_TRACE` output (`trace: alias expansion: co => checkout`).

use crate::types::Command;
use regex::Regex;
use std::sync::LazyLock;

static ALIAS_EXPANSION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new("trace: alias expansion: ([^ ]*) => ([^\n]*)").unwrap());

pub fn git_support<T: Default>(command: &Command, f: impl FnOnce(&Command) -> T) -> T {
    if !command.is_app(&["git", "hub"]) {
        return T::default();
    }
    if command.output().contains("trace: alias expansion:")
        && let Some(caps) = ALIAS_EXPANSION.captures(command.output())
    {
        let shell = command.shell();
        // git quotes everything ('commit' '--amend'); re-quote it our way.
        let expansion: Vec<String> = shell
            .split_command(&caps[2])
            .iter()
            .map(|part| shell.quote(part))
            .collect();
        if let Ok(alias) = Regex::new(&format!(r"\b{}\b", &caps[1])) {
            let script = alias.replace_all(&command.script, regex::NoExpand(&expansion.join(" ")));
            return f(&command.with_script(script.into_owned()));
        }
    }
    f(command)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_aliases() {
        let cmd = Command::test(
            "git co",
            "19:22:36.299340 git.c:282   trace: alias expansion: co => 'checkout'",
        );
        assert_eq!(git_support(&cmd, |c| c.script.clone()), "git checkout");
        let cmd = Command::test(
            "git com file",
            "trace: alias expansion: com => 'commit' '--verbose'",
        );
        assert_eq!(
            git_support(&cmd, |c| c.script.clone()),
            "git commit --verbose file"
        );
        let cmd = Command::test(
            "git ci -m 'x'",
            "trace: alias expansion: ci => 'commit' '-m' 'a message'",
        );
        assert_eq!(
            git_support(&cmd, |c| c.script.clone()),
            "git commit -m 'a message' -m 'x'"
        );
    }

    #[test]
    fn only_git_and_hub() {
        assert!(git_support(&Command::test("hub push", ""), |_| true));
        assert!(!git_support(&Command::test("svn push", ""), |_| true));
    }
}
