//! Opt-in checks against the CLIs installed on this machine:
//! `cargo test --test installed_clis -- --ignored --nocapture`.
//!
//! Each case asks the structured engine to repair a typo using the app's
//! own completion, offline: no command is run, no cloud API is called
//! (completer probes get no credentials and a closed HTTP proxy), and apps
//! that aren't installed are skipped and reported.

use notypo::engine::{self, FailureContext, Outcome};
use notypo::settings::Settings;
use notypo::shells::Shell;
use notypo::types::Context;

const CASES: &[(&str, &str, &str)] = &[
    ("aws", "aws ec2 describ-instances --regoin eu-west-1", "aws ec2 describe-instances --region eu-west-1"),
    ("gcloud", "gcloud compte instnaces list", "gcloud compute instances list"),
    ("az", "az storage acount list", "az storage account list"),
    ("git", "git sttus", "git status"),
    ("kubectl", "kubectl gt pods", "kubectl get pods"),
    ("helm", "helm instal x", "helm install x"),
    ("gh", "gh pr chekout 12", "gh pr checkout 12"),
    ("hcloud", "hcloud servr list", "hcloud server list"),
    ("kind", "kind creat cluster", "kind create cluster"),
    ("docker", "docker conatiner ls", "docker container ls"),
    ("terraform", "terraform plna", "terraform plan"),
    ("tofu", "tofu valdate", "tofu validate"),
    ("packer", "packer biuld x", "packer build x"),
];

#[test]
#[ignore = "uses the CLIs installed on this machine"]
fn installed_clis_repair_typos_from_their_own_completion() {
    let ctx = Context::new(Settings::default(), Shell::Bash, "fuck".into())
        .with_history::<&str>(&[]);
    let mut failures = Vec::new();
    for (app, typo, expected) in CASES {
        if ctx.which(app).is_none() {
            eprintln!("skipped {app}: not installed");
            continue;
        }
        let failure = FailureContext {
            source: (*typo).into(),
            exit_status: Some(1),
            ..FailureContext::default()
        };
        let report = engine::correct(&failure, &ctx);
        let got = report.outcome.candidates().first().map(|c| c.script.clone());
        let decided = matches!(report.outcome, Outcome::Suggestion(_));
        eprintln!(
            "{app:>9}: {typo} -> {} ({}, {} probes)",
            got.as_deref().unwrap_or("-"),
            if decided { "decided" } else { "asked" },
            report.probes
        );
        if got.as_deref() != Some(*expected) {
            failures.push(format!("{app}: {:?} {:?}", report.outcome, report.notes));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
