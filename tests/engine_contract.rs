//! Platform-independent contracts for contextual native discovery and edits.
use notypo::engine::native::{
    Capabilities, CompletionError, CompletionItem, NativeCompletionBackend, Trust,
};
use notypo::engine::probe::Budget;
use notypo::engine::providers::{Answer, CandidateProvider, NativeCompletion, Slot};
use notypo::engine::{self, FailureContext, TokenRole};
use notypo::settings::Settings;
use notypo::shells::Shell;
use notypo::types::Context;
use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

#[derive(Debug, Default)]
struct Contextual {
    queries: Cell<usize>,
}

fn item(value: &str, takes_value: Option<bool>) -> CompletionItem {
    CompletionItem {
        value: value.into(),
        takes_value,
        description: None,
    }
}

impl NativeCompletionBackend for Contextual {
    fn id(&self) -> &str {
        "context fixture"
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            subcommands: true,
            options: true,
            option_arity: true,
            values: false,
            resources: false,
            option_prefix: "--",
            short_options: true,
            complete_options: true,
            complete_subcommands: true,
            descriptions: false,
            query_dialect: None,
            trust: Trust::Bridge,
        }
    }
    fn complete(
        &self,
        words: &[&str],
        prefix: &str,
        budget: &mut Budget,
    ) -> Result<Vec<CompletionItem>, CompletionError> {
        self.complete_at(words, prefix, &[], budget)
    }
    fn complete_at(
        &self,
        words: &[&str],
        prefix: &str,
        following: &[&str],
        _: &mut Budget,
    ) -> Result<Vec<CompletionItem>, CompletionError> {
        self.queries.set(self.queries.get() + 1);
        Ok(if prefix.starts_with('-') {
            vec![item("--scope", Some(true))]
        } else if words.is_empty() {
            match following {
                ["--scope", "west"] => vec![item("build", None)],
                ["--scope", "east"] => vec![item("deploy", None)],
                _ => Vec::new(),
            }
        } else {
            Vec::new()
        })
    }
    fn requires_literal_completion_context(&self) -> bool {
        true
    }
}

#[test]
fn native_corrections_use_later_arguments_in_every_shell_dialect() {
    for shell in [
        Shell::Bash,
        Shell::Zsh,
        Shell::Fish,
        Shell::Tcsh,
        Shell::Powershell,
        Shell::Generic,
    ] {
        let settings = Settings {
            disabled_sources: vec!["help".into(), "man".into(), "filesystem".into()],
            ..Settings::default()
        };
        let ctx = Context::new(settings, shell, "typo".into())
            .with_path_listing(&["tool"])
            .with_aliases(&[])
            .with_history::<&str>(&[]);
        for (source, expected) in [
            ("tool  biuld --scope west", "tool  build --scope west"),
            ("tool\tdploy --scope east", "tool\tdeploy --scope east"),
        ] {
            let failure = FailureContext {
                source: source.into(),
                exit_status: Some(1),
                ..FailureContext::default()
            };
            let report = engine::correct_with_backends(
                &failure,
                &ctx,
                vec![("tool".into(), Rc::new(Contextual::default()))],
            );
            assert_eq!(
                report
                    .outcome
                    .candidates()
                    .first()
                    .map(|c| c.script.as_str()),
                Some(expected),
                "{shell:?}: {report:?}"
            );
        }
    }
}

#[test]
fn request_memoization_separates_suffix_context_and_refuses_opaque_context() {
    let backend = Rc::new(Contextual::default());
    let mut native = NativeCompletion::new(&[], false).with_backend("tool", backend.clone());
    let mut budget = Budget::new(Duration::from_secs(5), Duration::from_secs(1), 10);
    let slot = Slot {
        role: TokenRole::Subcommand,
        program: "tool",
        path: None,
        context: &[],
        following: None,
        command_path: &[],
        typed: "biuld",
        literal_arguments: (&[], &[]),
        fresh: false,
    };
    assert!(matches!(
        native.vocabulary(&slot, &mut budget),
        Answer::Failed(_)
    ));
    assert_eq!(
        backend.queries.get(),
        0,
        "opaque context must not invoke a callback"
    );
    for (scope, expected) in [("west", "build"), ("east", "deploy"), ("west", "build")] {
        let following = vec!["--scope".into(), scope.into()];
        let query = Slot {
            following: Some(&following),
            ..slot
        };
        let Answer::Words(answer) = native.vocabulary(&query, &mut budget) else {
            panic!("missing answer")
        };
        assert_eq!(answer.words[0].value, expected);
    }
    assert_eq!(
        backend.queries.get(),
        2,
        "the repeated west query reuses only its own answer"
    );
}
