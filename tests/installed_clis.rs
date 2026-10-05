//! Opt-in checks against the CLIs installed on this machine:
//! `cargo test --test installed_clis -- --ignored --nocapture`.
//!
//! Each case asks the structured engine to repair a typo using the app's
//! own completion, offline: no command is run, no cloud API is called
//! (completer probes get no credentials and a closed HTTP proxy), and apps
//! that aren't installed are skipped and reported.

use notypo::engine::safety::Decision;
use notypo::engine::{self, FailureContext, Outcome};
use notypo::settings::Settings;
use notypo::shells::Shell;
use notypo::types::Context;

const CASES: &[(&str, &str, &str)] = &[
    (
        "aws",
        "aws ec2 describ-instances --regoin eu-west-1",
        "aws ec2 describe-instances --region eu-west-1",
    ),
    (
        "gcloud",
        "gcloud compte instnaces list",
        "gcloud compute instances list",
    ),
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
    ("sofka", "sofka --readoly", "sofka --readonly"),
    ("pip3", "pip3 config lsit", "pip3 config list"),
    ("npm", "npm config lsit", "npm config list"),
    ("npm", "npm pbulish", "npm publish"),
    ("npm", "npm trust list --jsno", "npm trust list --json"),
    ("cargo", "cargo biuld", "cargo build"),
    ("cargo", "cargo +stable biuld", "cargo +stable build"),
    ("cargo", "cargo build --releae", "cargo build --release"),
    (
        "cargo",
        "cargo report future-incompatibilites",
        "cargo report future-incompatibilities",
    ),
    ("cargo", "cargo +stabel version", "cargo +stable version"),
    ("lefthook", "lefthook instal", "lefthook install"),
    ("lefthook", "lefthook valdate", "lefthook validate"),
    // Cobra apps trusted by the user rather than audited by notypo.
    ("upctl", "upctl servr list", "upctl server list"),
    ("velero", "velero backp get", "velero backup get"),
    ("k9s", "k9s --readoly", "k9s --readonly"),
    ("trivy", "trivy imgae alpine", "trivy image alpine"),
    ("k6", "k6 rnu script.js", "k6 run script.js"),
    ("talosctl", "talosctl helth", "talosctl health"),
    ("popeye", "popeye --sav", "popeye --save"),
    ("gdu-go", "gdu-go --no-colr", "gdu-go --no-color"),
    // Homebrew installs `eval "$(JUST_COMPLETE=bash just)"`: clap's shim.
    ("just", "just --lsit", "just --list"),
    ("just", "just --dry-rnu", "just --dry-run"),
    // The Cloud SDK's one bash_completion.d file registers both.
    ("bq", "bq qeury x", "bq query x"),
    ("gsutil", "gsutil lss gs://bucket", "gsutil ls gs://bucket"),
    // Installed bash handlers (svn's lives in Homebrew's `subversion` file).
    (
        "yt-dlp",
        "yt-dlp --list-formts x",
        "yt-dlp --list-formats x",
    ),
    ("wg", "wg shwo", "wg show"),
    ("asciinema", "asciinema recrod", "asciinema record"),
    ("elan", "elan toolchian list", "elan toolchain list"),
    ("svn", "svn stauts", "svn status"),
    (
        "gsettings",
        "gsettings lsit-schemas",
        "gsettings list-schemas",
    ),
    // oclif apps are read from their own manifests; nothing runs.
    (
        "eas",
        "eas biuld --platfrom ios",
        "eas build --platform ios",
    ),
    ("eas", "eas buidl:list", "eas build:list"),
    // node's own option list, a built-in bridge.
    ("node", "node --inpsect app.js", "node --inspect app.js"),
    (
        "pip3",
        "pip3 install --dry-rnu example",
        "pip3 install --dry-run example",
    ),
    (
        "sofka",
        "sofka plugin instlal example",
        "sofka plugin install example",
    ),
    // AWS SAM CLI, a click app (pip's aws-sam-cli).
    (
        "sam",
        "sam build --use-contaner",
        "sam build --use-container",
    ),
    // Oracle Cloud's oci-cli: click, with a reworded help option.
    (
        "oci",
        "oci os bucket list --namespce x",
        "oci os bucket list --namespace x",
    ),
    // npm's heroku: oclif manifests, read without running heroku.
    ("heroku", "heroku apps:lsit", "heroku apps:list"),
    (
        "heroku",
        "heroku apps:destory --app x",
        "heroku apps:destroy --app x",
    ),
];

#[test]
#[ignore = "uses the CLIs installed on this machine"]
fn installed_clis_repair_typos_from_their_own_completion() {
    let settings = Settings {
        trusted_completers: vec![
            "sofka".into(),
            "python:pip".into(),
            "npm:npm".into(),
            "rust:cargo".into(),
            "github.com/evilmartians/lefthook/v2".into(),
            // upctl's build records only `command-line-arguments` as its
            // module, an identity shared by unrelated programs: trust its name.
            "upctl".into(),
            "github.com/vmware-tanzu/velero/cmd/velero".into(),
            "github.com/derailed/k9s".into(),
            "github.com/aquasecurity/trivy/cmd/trivy".into(),
            "go.k6.io/k6/v2".into(),
            "github.com/siderolabs/talos/cmd/talosctl".into(),
            "github.com/derailed/popeye".into(),
            "github.com/dundee/gdu/v5/cmd/gdu".into(),
            "just".into(),
            "bq".into(),
            "gsutil".into(),
            "yt-dlp".into(),
            "wg".into(),
            "asciinema".into(),
            "elan".into(),
            "svn".into(),
            "gsettings".into(),
            "python:samcli".into(),
            "python:oci_cli".into(),
        ],
        // lefthook links urfave/cli v3.10+, whose completion runs Before hooks.
        // A click app is recognized from its help.
        trusted_help: vec![
            "sofka".into(),
            "rust:cargo".into(),
            "lefthook".into(),
            "python:samcli".into(),
            "python:oci_cli".into(),
        ],
        ..Settings::default()
    };
    let ctx = Context::new(settings, Shell::Bash, "fuck".into()).with_history::<&str>(&[]);
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
        let got = report
            .outcome
            .candidates()
            .first()
            .map(|c| c.script.clone());
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
        if *app == "heroku" && typo.contains("destory") {
            let candidate = report.outcome.candidates().first().unwrap();
            assert_eq!(candidate.safety.decision, Decision::Confirm);
        }
        if *app == "sofka" {
            let candidate = report.outcome.candidates().first().unwrap();
            assert!(
                candidate
                    .edits
                    .iter()
                    .all(|edit| edit.via == "clap completion")
            );
            if typo.contains("plugin") {
                assert_eq!(candidate.safety.decision, Decision::Confirm);
            }
        }
        if *app == "pip3" {
            let candidate = report.outcome.candidates().first().unwrap();
            assert!(
                candidate
                    .edits
                    .iter()
                    .all(|edit| edit.via == "pip completion")
            );
            if typo.contains("install") {
                assert_eq!(candidate.safety.decision, Decision::Confirm);
            }
        }
        if *app == "npm" {
            let candidate = report.outcome.candidates().first().unwrap();
            assert!(
                candidate
                    .edits
                    .iter()
                    .all(|edit| edit.via == "npm completion")
            );
            if typo.contains("pbulish") {
                assert_eq!(candidate.safety.decision, Decision::Confirm);
            }
        }
        if *app == "cargo" {
            let candidate = report.outcome.candidates().first().unwrap();
            assert!(
                candidate
                    .edits
                    .iter()
                    .all(|edit| edit.via == "cargo completion")
            );
            assert_eq!(candidate.safety.decision, Decision::Confirm);
        }
        if *app == "just" {
            let candidate = report.outcome.candidates().first().unwrap();
            assert!(
                candidate
                    .edits
                    .iter()
                    .all(|edit| edit.via == "clap completion")
            );
        }
        if *app == "eas" {
            assert!(decided, "complete manifests decide: {:?}", report.outcome);
            assert_eq!(report.probes, 0);
        }
        if *app == "lefthook" {
            let candidate = report.outcome.candidates().first().unwrap();
            assert!(
                candidate
                    .edits
                    .iter()
                    .all(|edit| edit.via == "urfave completion")
            );
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Tools without a completer that notypo can find: their own `--help`
/// (trusted) supplies commands and options, and corrections are asked.
#[test]
#[ignore = "uses the CLIs installed on this machine"]
fn installed_help_fallback_repairs_tools_without_completers() {
    const HELPED: &[(&str, &str, &str)] = &[
        ("rustup", "rustup toolchian list", "rustup toolchain list"),
        (
            "rustc",
            "rustc --edtion 2024 x.rs",
            "rustc --edition 2024 x.rs",
        ),
        ("rustfmt", "rustfmt --chekc x.rs", "rustfmt --check x.rs"),
        ("cross", "cross biuld", "cross build"),
        ("wasm-pack", "wasm-pack biuld", "wasm-pack build"),
        ("mdbook", "mdbook serv", "mdbook serve"),
        ("tokei", "tokei --exculde x", "tokei --exclude x"),
        // Thor's `<program> <command>  # description` command list.
        ("bundle", "bundle instal", "bundle install"),
        // Apple's swift lists `  swift build  ...` in bold even when piped.
        ("swift", "swift biuld", "swift build"),
        // GNU Midnight Commander, not the MinIO client of the same name.
        ("mc", "mc --nocolr", "mc --nocolor"),
        // AWS CDK bundles yargs: help shows `[string] [choices: "never", ...]`.
        ("cdk", "cdk synht", "cdk synth"),
        (
            "cdk",
            "cdk deploy --require-approval nevr",
            "cdk deploy --require-approval never",
        ),
    ];
    let settings = Settings {
        trusted_help: HELPED.iter().map(|(app, _, _)| (*app).into()).collect(),
        ..Settings::default()
    };
    let ctx = Context::new(settings, Shell::Bash, "fuck".into()).with_history::<&str>(&[]);
    let mut failures = Vec::new();
    for (app, typo, expected) in HELPED {
        if ctx.which(app).is_none() {
            eprintln!("skipped {app}: not installed (unverified)");
            continue;
        }
        let failure = FailureContext {
            source: (*typo).into(),
            exit_status: Some(1),
            ..FailureContext::default()
        };
        let report = engine::correct(&failure, &ctx);
        let candidate = report.outcome.candidates().first().cloned();
        eprintln!(
            "{app:>9}: {typo} -> {} ({} probes)",
            candidate.as_ref().map_or("-", |c| c.script.as_str()),
            report.probes
        );
        match candidate {
            Some(candidate) if candidate.script == *expected => {
                if *app == "bundle" || typo.contains("--require-approval") {
                    // Installing gems changes packages; cdk then deploys
                    // without asking.
                    assert_eq!(candidate.safety.decision, Decision::Confirm);
                }
            }
            _ => failures.push(format!("{app}: {:?} {:?}", report.outcome, report.notes)),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[cfg(unix)]
#[test]
#[ignore = "uses installed npm with an isolated project"]
fn installed_npm_completes_literal_project_scripts_without_running_them() {
    use std::fs;
    use std::process::Command;

    let ctx = Context::new(Settings::default(), Shell::Bash, "fuck".into());
    if ctx.which("npm").is_none() {
        eprintln!("skipped npm scripts: not installed (unverified)");
        return;
    }
    struct Workspace(std::path::PathBuf);
    impl Drop for Workspace {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let workspace = Workspace(
        std::env::temp_dir().join(format!("notypo-installed-npm-{}", std::process::id())),
    );
    fs::create_dir(&workspace.0).unwrap();
    let project = workspace
        .0
        .join("project 😀'with spaces $(touch operation-marker)");
    fs::create_dir(&project).unwrap();
    fs::write(project.join("package.json"), r#"{"name":"completion-fixture","version":"1.0.0","scripts":{"build":"touch operation-marker","report package":"touch operation-marker"}}"#).unwrap();
    let prefix = format!(
        "npm --prefix {} run",
        notypo::shlex::quote(project.to_str().unwrap())
    );
    for (typo, corrected) in [("biuld", "build"), ("'report pakcage'", "'report package'")] {
        let source = format!("{prefix} {typo}");
        let expected = format!("{prefix} {corrected}");
        let output = Command::new(env!("CARGO_BIN_EXE_notypo"))
            .current_dir(&workspace.0)
            .env("XDG_CONFIG_HOME", &workspace.0)
            .env("XDG_CACHE_HOME", &workspace.0)
            .env("TF_SHELL", "bash")
            .env("NOTYPO_TRUSTED_COMPLETERS", r#"["npm:npm"]"#)
            .env("NOTYPO_TRUSTED_HELP", "")
            .env("NOTYPO_DISABLED_SOURCES", "help:man:history:legacy")
            .env("NOTYPO_REPLAY_FOR_DIAGNOSIS", "false")
            .env_remove("TF_SHELL_ALIASES")
            .env_remove("NOTYPO_SHELL_FUNCTIONS")
            .args(["--json", "--force-command", &source])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["candidates"][0]["command"], expected, "{report}");
        assert_eq!(
            report["candidates"][0]["safety"]["decision"], "confirm",
            "{report}"
        );
        assert!(
            report["candidates"][0]["edits"]
                .as_array()
                .unwrap()
                .iter()
                .all(|edit| edit["via"] == "npm completion"),
            "{report}"
        );
        eprintln!(
            "npm script {typo} -> {corrected}; {} probes; approval required",
            report["probes"]
        );
        assert!(!workspace.0.join("operation-marker").exists());
        assert!(!project.join("operation-marker").exists());
    }
}

#[cfg(unix)]
#[test]
#[ignore = "uses installed Cargo with an isolated project"]
fn installed_cargo_reads_project_values_aliases_and_extensions_without_building_or_running_them() {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;

    let ctx = Context::new(Settings::default(), Shell::Bash, "fuck".into());
    if ctx.which("cargo").is_none() {
        eprintln!("skipped Cargo project: not installed (unverified)");
        return;
    }
    struct Workspace(std::path::PathBuf);
    impl Drop for Workspace {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let workspace = Workspace(
        std::env::temp_dir().join(format!("notypo-installed-cargo-{}", std::process::id())),
    );
    fs::create_dir(&workspace.0).unwrap();
    let project = workspace
        .0
        .join("project 😀'with spaces $(touch operation-marker)");
    for directory in ["src", "examples", "tests", "benches"] {
        fs::create_dir_all(project.join(directory)).unwrap();
    }
    fs::write(
        project.join("Cargo.toml"),
        r#"[package]
name = "completion-example"
version = "0.1.0"
edition = "2024"
build = "build.rs"
[features]
default = []
quiet-mode = []
"#,
    )
    .unwrap();
    fs::write(project.join("src/main.rs"), "fn main() {}\n").unwrap();
    fs::write(project.join("examples/report.rs"), "fn main() {}\n").unwrap();
    fs::write(project.join("tests/smoke.rs"), "#[test] fn smoke() {}\n").unwrap();
    fs::write(project.join("benches/measure.rs"), "fn main() {}\n").unwrap();
    fs::write(
        project.join("build.rs"),
        r#"fn main() { std::fs::write("operation-marker", "built").unwrap(); }"#,
    )
    .unwrap();
    let home = workspace.0.join("cargo-home");
    let bin = workspace.0.join("bin");
    fs::create_dir(&home).unwrap();
    fs::create_dir(&bin).unwrap();
    fs::write(
        home.join("config.toml"),
        "[alias]\ndanger = \"!touch operation-marker\"\n",
    )
    .unwrap();
    let extension = bin.join("cargo-future");
    fs::write(&extension, "#!/bin/sh\ntouch operation-marker\nexit 9\n").unwrap();
    fs::set_permissions(&extension, fs::Permissions::from_mode(0o755)).unwrap();
    let path = std::env::join_paths(std::iter::once(bin).chain(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    )))
    .unwrap();
    let manifest = notypo::shlex::quote(project.join("Cargo.toml").to_str().unwrap()).into_owned();
    let correct = |source: &str| {
        let output = Command::new(env!("CARGO_BIN_EXE_notypo"))
            .current_dir(&workspace.0)
            .env("PATH", &path)
            .env("CARGO_HOME", &home)
            .env("XDG_CONFIG_HOME", &workspace.0)
            .env("XDG_CACHE_HOME", &workspace.0)
            .env("TF_SHELL", "bash")
            .env("NOTYPO_TRUSTED_COMPLETERS", r#"["rust:cargo"]"#)
            .env("NOTYPO_TRUSTED_HELP", r#"["rust:cargo"]"#)
            .env("NOTYPO_DISABLED_SOURCES", "help:man:history:legacy")
            .env("NOTYPO_REPLAY_FOR_DIAGNOSIS", "false")
            .env_remove("TF_SHELL_ALIASES")
            .env_remove("NOTYPO_SHELL_FUNCTIONS")
            .args(["--json", "--force-command", source])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap()
    };
    for (typo, corrected, resource) in [
        ("--bin completion-exampel", "--bin completion-example", true),
        ("-p completion-exampel", "-p completion-example", true),
        ("--example rpeort", "--example report", true),
        ("--test smoek", "--test smoke", true),
        ("--bench measrue", "--bench measure", true),
        ("--features quiet-mdoe", "--features quiet-mode", true),
        (
            "--features=default,quiet-mdoe",
            "--features=default,quiet-mode",
            true,
        ),
        (
            "--features completion-example/quiet-mdoe",
            "--features completion-example/quiet-mode",
            true,
        ),
        (
            "--features 'default completion-example/quiet-mdoe'",
            "--features 'default completion-example/quiet-mode'",
            true,
        ),
        ("--color nveer", "--color never", false),
    ] {
        let prefix = format!("cargo build --manifest-path {manifest}");
        let source = format!("{prefix} {typo}");
        let expected = format!("{prefix} {corrected}");
        let report = correct(&source);
        assert_eq!(report["candidates"][0]["command"], expected, "{report}");
        assert_eq!(
            report["candidates"][0]["safety"]["decision"], "confirm",
            "{report}"
        );
        assert!(
            report["candidates"][0]["edits"]
                .as_array()
                .unwrap()
                .iter()
                .all(|edit| edit["via"] == "cargo completion"),
            "{report}"
        );
        if resource {
            assert!(
                report["candidates"][0]["safety"]["reasons"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|reason| reason.as_str().unwrap().contains("acts on the resource")),
                "{report}"
            );
        }
        eprintln!(
            "Cargo project {typo} -> {corrected}; {} probes; approval required",
            report["probes"]
        );
        assert!(!workspace.0.join("operation-marker").exists());
        assert!(!project.join("operation-marker").exists());
        assert!(!project.join("Cargo.lock").exists());
        assert!(!project.join("target").exists());
    }
    for (source, expected) in [
        ("cargo dagner", "cargo danger"),
        ("cargo futrue", "cargo future"),
    ] {
        let report = correct(source);
        assert_eq!(report["candidates"][0]["command"], expected, "{report}");
        assert_eq!(
            report["candidates"][0]["safety"]["decision"], "confirm",
            "{report}"
        );
        eprintln!("{source} -> {expected}; listed without execution");
        assert!(!workspace.0.join("operation-marker").exists());
    }
    let original = fs::read_to_string(project.join("Cargo.toml")).unwrap();
    fs::write(
        project.join("Cargo.toml"),
        format!("{original}\n[workspace]\nmembers = ['helper']\ndefault-members = ['.']\n"),
    )
    .unwrap();
    fs::create_dir_all(project.join("helper/src")).unwrap();
    fs::write(
        project.join("helper/Cargo.toml"),
        "[package]\nname = 'completion-helper'\nversion = '0.1.0'\nedition = '2024'\n",
    )
    .unwrap();
    fs::write(project.join("helper/src/main.rs"), "fn main() {}\n").unwrap();
    let prefix = format!("cargo build --manifest-path {manifest}");
    let report = correct(&format!("{prefix} --bin completion-helpre"));
    assert!(
        report["candidates"]
            .as_array()
            .unwrap()
            .iter()
            .all(|candidate| !candidate["command"]
                .as_str()
                .unwrap()
                .ends_with("--bin completion-helper")),
        "a nondefault workspace target must not be proposed: {report}"
    );
    for scope in ["--workspace", "-p completion-helper"] {
        let source = format!("{prefix} {scope} --bin completion-helpre");
        let expected = format!("{prefix} {scope} --bin completion-helper");
        let report = correct(&source);
        assert_eq!(report["candidates"][0]["command"], expected, "{report}");
        assert_eq!(
            report["candidates"][0]["safety"]["decision"], "confirm",
            "{report}"
        );
        eprintln!(
            "Cargo workspace {scope}: completion-helpre -> completion-helper; {} probes",
            report["probes"]
        );
    }
    assert!(!workspace.0.join("operation-marker").exists());
    assert!(!project.join("operation-marker").exists());
    assert!(!project.join("Cargo.lock").exists());
    assert!(!project.join("target").exists());
}

#[cfg(unix)]
#[test]
#[ignore = "uses installed lefthook with an isolated git repository"]
fn installed_lefthook_lists_configured_hooks_as_resources_without_running_them() {
    use std::fs;
    use std::process::Command;

    let ctx = Context::new(Settings::default(), Shell::Bash, "fuck".into());
    if ctx.which("lefthook").is_none() || ctx.which("git").is_none() {
        eprintln!("skipped lefthook hooks: lefthook or git not installed (unverified)");
        return;
    }
    struct Workspace(std::path::PathBuf);
    impl Drop for Workspace {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let workspace = Workspace(
        std::env::temp_dir().join(format!("notypo-installed-lefthook-{}", std::process::id())),
    );
    let repo = workspace.0.join("repo 😀 $(touch operation-marker)");
    fs::create_dir_all(&repo).unwrap();
    assert!(
        Command::new("git")
            .args(["init", "-q", "."])
            .current_dir(&repo)
            .status()
            .unwrap()
            .success()
    );
    let marker = workspace.0.join("operation-marker");
    fs::write(
        repo.join("lefthook.yml"),
        format!(
            "pre-commit:\n  jobs:\n    - run: touch {0}\npre-push:\n  jobs:\n    - run: touch {0}\n",
            notypo::shlex::quote(marker.to_str().unwrap())
        ),
    )
    .unwrap();
    let correct = |source: &str, help: &str| {
        let output = Command::new(env!("CARGO_BIN_EXE_notypo"))
            .current_dir(&repo)
            .env("XDG_CONFIG_HOME", &workspace.0)
            .env("XDG_CACHE_HOME", &workspace.0)
            .env("TF_SHELL", "bash")
            .env(
                "NOTYPO_TRUSTED_COMPLETERS",
                r#"["github.com/evilmartians/lefthook/v2"]"#,
            )
            .env("NOTYPO_TRUSTED_HELP", help)
            .env("NOTYPO_DISABLED_SOURCES", "help:man:history:legacy")
            .env("NOTYPO_REPLAY_FOR_DIAGNOSIS", "false")
            .env_remove("TF_SHELL_ALIASES")
            .env_remove("NOTYPO_SHELL_FUNCTIONS")
            .args(["--json", "--force-command", source])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap()
    };
    // urfave/cli v3.10+ runs Before hooks while completing: help trust first.
    let report = correct("lefthook instal", "");
    assert_eq!(report["probes"], 0, "{report}");
    assert!(report["candidates"].as_array().unwrap().is_empty());
    for (typo, corrected, reason) in [
        (
            "lefthook run pre-comit",
            "lefthook run pre-commit",
            "resource",
        ),
        (
            "lefthook run pre-commit --forse",
            "lefthook run pre-commit --force",
            "--force",
        ),
    ] {
        let report = correct(typo, "lefthook");
        let candidate = &report["candidates"][0];
        assert_eq!(candidate["command"], corrected, "{report}");
        assert_eq!(candidate["safety"]["decision"], "confirm", "{report}");
        assert!(
            candidate["safety"]["reasons"]
                .as_array()
                .unwrap()
                .iter()
                .any(|why| why.as_str().unwrap().contains(reason)),
            "{report}"
        );
        assert!(
            candidate["edits"]
                .as_array()
                .unwrap()
                .iter()
                .all(|edit| edit["via"] == "urfave completion"),
            "{report}"
        );
        eprintln!(
            "lefthook {typo} -> {corrected}; {} probes; approval required",
            report["probes"]
        );
    }
    assert!(!marker.exists(), "a configured hook ran");
    assert!(!repo.join("operation-marker").exists());
    assert!(
        !repo.join(".git/hooks/pre-commit").exists(),
        "hooks were installed"
    );
}

#[cfg(unix)]
#[test]
#[ignore = "uses installed zsh completion functions"]
fn installed_zsh_handlers_repair_typos_from_partial_lists() {
    use std::process::Command;

    let ctx = Context::new(Settings::default(), Shell::Zsh, "fuck".into());
    let Some(zsh) = ctx.which("zsh") else {
        eprintln!("skipped zsh handlers: zsh is not installed (unverified)");
        return;
    };
    let fpath = Command::new(&zsh)
        .args(["-fc", "print -r -- ${(j.:.)fpath}"])
        .output()
        .unwrap();
    let fpath = String::from_utf8(fpath.stdout).unwrap();
    let home = std::env::temp_dir().join(format!("notypo-installed-zsh-{}", std::process::id()));
    std::fs::create_dir_all(&home).unwrap();
    // (app, typo, first candidate, needs approval)
    let cases = [
        ("deno", "deno rnu main.ts", "deno run main.ts", false),
        ("jq", "jq --raw-ouptut .", "jq --raw-output .", false),
        (
            "curl",
            "curl --silen https://example.com",
            "curl --silent https://example.com",
            false,
        ),
        ("make", "make --dyr-run", "make --dry-run", false),
        ("gem", "gem lsit", "gem list", false),
        ("pipx", "pipx lsit", "pipx list", false),
        ("brew", "brew instal jq", "brew install jq", true),
        ("rsync", "rsync --arcive a b", "rsync --archive a b", true),
        // zsh's own handlers, one shared by the ansible-* programs.
        (
            "ansible-vault",
            "ansible-vault encrpyt secrets.yml",
            "ansible-vault encrypt secrets.yml",
            false,
        ),
        (
            "ansible-playbook",
            "ansible-playbook --chekc site.yml",
            "ansible-playbook --check site.yml",
            false,
        ),
        (
            "wget",
            "wget --quite https://example.com",
            "wget --quiet https://example.com",
            false,
        ),
        (
            "xz",
            "xz --decompres notes.xz",
            "xz --decompress notes.xz",
            false,
        ),
        ("java", "java -versoin", "java -version", false),
        (
            "javac",
            "javac -verbos Main.java",
            "javac -verbose Main.java",
            false,
        ),
        ("rake", "rake --taks", "rake --tasks", false),
        ("sqlite3", "sqlite3 -versoin", "sqlite3 -version", false),
    ];
    let mut failures = Vec::new();
    for (app, typo, expected, approval) in cases {
        if ctx.which(app).is_none() {
            eprintln!("skipped {app}: not installed (unverified)");
            continue;
        }
        let output = Command::new(env!("CARGO_BIN_EXE_notypo"))
            .current_dir(&home)
            .env("XDG_CONFIG_HOME", &home)
            .env("XDG_CACHE_HOME", &home)
            .env("TF_SHELL", "zsh")
            .env("NOTYPO_ZSH_FPATH", fpath.trim())
            .env(
                "NOTYPO_TRUSTED_COMPLETERS",
                r#"["deno", "jq", "curl", "make", "gem", "python:pipx", "brew", "rsync", "python:ansible", "wget", "xz", "java", "javac", "rake", "sqlite3"]"#,
            )
            .env("NOTYPO_TRUSTED_HELP", "")
            .env("NOTYPO_DISABLED_SOURCES", "help:man:history:legacy")
            .env("NOTYPO_REPLAY_FOR_DIAGNOSIS", "false")
            .env("HOMEBREW_NO_AUTO_UPDATE", "1")
            .env_remove("TF_SHELL_ALIASES")
            .env_remove("NOTYPO_SHELL_FUNCTIONS")
            .args(["--json", "--force-command", typo])
            .output()
            .unwrap();
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        let candidate = &report["candidates"][0];
        eprintln!(
            "{app:>9}: {typo} -> {} ({}, {} probes)",
            candidate["command"], report["outcome"]["kind"], report["probes"]
        );
        let via_zsh = candidate["edits"]
            .as_array()
            .is_some_and(|edits| edits.iter().all(|edit| edit["via"] == "zsh completion"));
        let decision = if approval { "confirm" } else { "allow" };
        if candidate["command"] != expected
            || !via_zsh
            || candidate["safety"]["decision"] != decision
        {
            failures.push(format!("{app}: {report}"));
        }
    }
    let _ = std::fs::remove_dir_all(&home);
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[cfg(unix)]
#[test]
#[ignore = "uses the completion scripts embedded in the installed fish"]
fn installed_fish_embedded_handlers_repair_typos_for_fish_users() {
    use std::process::Command;

    let ctx = Context::new(Settings::default(), Shell::Fish, "fuck".into());
    let Some(fish) = ctx.which("fish") else {
        eprintln!("skipped embedded fish handlers: fish is not installed (unverified)");
        return;
    };
    let embedded = Command::new(&fish)
        .args(["--no-config", "-c", "status get-file completions/jq.fish"])
        .output()
        .is_ok_and(|output| output.status.success());
    if !embedded {
        eprintln!("skipped embedded fish handlers: this fish embeds no completions");
        return;
    }
    let home = std::env::temp_dir().join(format!("notypo-installed-fish-{}", std::process::id()));
    std::fs::create_dir_all(&home).unwrap();
    // (app, typo, first candidate, needs approval)
    let cases = [
        ("jq", "jq --raw-ouptut .", "jq --raw-output .", false),
        (
            "curl",
            "curl --silen https://example.com",
            "curl --silent https://example.com",
            false,
        ),
        (
            "tar",
            "tar --extarct -f x.tar",
            "tar --extract -f x.tar",
            false,
        ),
        ("rsync", "rsync --arcive a b", "rsync --archive a b", true),
    ];
    let mut failures = Vec::new();
    for (app, typo, expected, approval) in cases {
        if ctx.which(app).is_none() {
            eprintln!("skipped {app}: not installed (unverified)");
            continue;
        }
        let output = Command::new(env!("CARGO_BIN_EXE_notypo"))
            .current_dir(&home)
            .env("XDG_CONFIG_HOME", &home)
            .env("XDG_CACHE_HOME", &home)
            .env("XDG_DATA_HOME", &home)
            .env("TF_SHELL", "fish")
            .env(
                "NOTYPO_TRUSTED_COMPLETERS",
                r#"["jq", "curl", "tar", "rsync"]"#,
            )
            .env("NOTYPO_TRUSTED_HELP", "")
            .env("NOTYPO_DISABLED_SOURCES", "help:man:history:legacy")
            .env("NOTYPO_REPLAY_FOR_DIAGNOSIS", "false")
            .env_remove("NOTYPO_FISH_COMPLETE_PATH")
            .env_remove("TF_SHELL_ALIASES")
            .env_remove("NOTYPO_SHELL_FUNCTIONS")
            .args(["--json", "--force-command", typo])
            .output()
            .unwrap();
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        let candidate = &report["candidates"][0];
        eprintln!(
            "{app:>9}: {typo} -> {} ({}, {} probes)",
            candidate["command"], report["outcome"]["kind"], report["probes"]
        );
        let via_fish = candidate["edits"]
            .as_array()
            .is_some_and(|edits| edits.iter().all(|edit| edit["via"] == "fish completion"));
        let decision = if approval { "confirm" } else { "allow" };
        if candidate["command"] != expected
            || !via_fish
            || candidate["safety"]["decision"] != decision
        {
            failures.push(format!("{app}: {report}"));
        }
    }
    let _ = std::fs::remove_dir_all(&home);
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// fish's embedded make handler lists targets with `make -pRrq`, which runs
/// the Makefile's `$(shell ...)`: only in a trusted workspace.
#[test]
#[ignore = "runs installed fish and make"]
fn installed_make_targets_are_listed_only_in_trusted_workspaces() {
    use std::process::Command;

    let ctx = Context::new(Settings::default(), Shell::Fish, "fuck".into());
    let (Some(fish), Some(_)) = (ctx.which("fish"), ctx.which("make")) else {
        eprintln!("skipped make targets: fish or make is not installed (unverified)");
        return;
    };
    let embedded = Command::new(&fish)
        .args(["--no-config", "-c", "status get-file completions/make.fish"])
        .output()
        .is_ok_and(|output| output.status.success());
    if !embedded {
        eprintln!("skipped make targets: this fish embeds no make completion");
        return;
    }
    let project =
        std::env::temp_dir().join(format!("notypo-installed-make-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&project);
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(
        project.join("Makefile"),
        "X := $(shell touch evaluated)\nbuild:\n\t@echo building\ntest:\n\t@echo testing\n",
    )
    .unwrap();
    let run = |trusted: &str| {
        let output = Command::new(env!("CARGO_BIN_EXE_notypo"))
            .current_dir(&project)
            .env("XDG_CONFIG_HOME", &project)
            .env("XDG_CACHE_HOME", &project)
            .env("XDG_DATA_HOME", &project)
            .env("TF_SHELL", "fish")
            .env("NOTYPO_TRUSTED_COMPLETERS", "make")
            .env("NOTYPO_TRUSTED_WORKSPACES", trusted)
            .env("NOTYPO_TRUSTED_HELP", "")
            .env("NOTYPO_DISABLED_SOURCES", "help:man:history:legacy")
            .env("NOTYPO_REPLAY_FOR_DIAGNOSIS", "false")
            .env_remove("NOTYPO_FISH_COMPLETE_PATH")
            .env_remove("TF_SHELL_ALIASES")
            .env_remove("NOTYPO_SHELL_FUNCTIONS")
            .args(["--json", "--force-command", "make biuld"])
            .output()
            .unwrap();
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap()
    };
    let refused = run("");
    let evaluated = project.join("evaluated").exists();
    let trusted = run(&project.display().to_string());
    let _ = std::fs::remove_dir_all(&project);
    assert!(!evaluated, "the Makefile ran without trust: {refused}");
    assert!(
        refused["notes"].as_array().is_some_and(|notes| notes
            .iter()
            .any(|n| n.as_str().is_some_and(|n| n.contains("trusted_workspaces")))),
        "{refused}"
    );
    assert_ne!(
        refused["candidates"][0]["command"], "make build",
        "{refused}"
    );
    assert_eq!(
        trusted["candidates"][0]["command"], "make build",
        "{trusted}"
    );
}

#[cfg(unix)]
#[test]
#[ignore = "uses installed dotnet with an isolated project and template hive"]
fn installed_dotnet_completes_project_and_installed_template_values_without_building() {
    use std::process::Command;
    let ctx = Context::new(Settings::default(), Shell::Bash, "fuck".into());
    let Some(dotnet) = ctx.which("dotnet") else {
        eprintln!("skipped dotnet: SDK is not installed (unverified)");
        return;
    };
    let root = std::env::temp_dir().join(format!("notypo-installed-dotnet-{}", std::process::id()));
    struct Cleanup(std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    std::fs::create_dir(&root).unwrap();
    let _cleanup = Cleanup(root.clone());
    let project = root.join("project");
    let home = root.join("home");
    let template = root.join("template");
    let context_template = root.join("context-template");
    let clean = root.join("clean");
    let selected = root.join("selected-project");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::create_dir_all(template.join(".template.config")).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(context_template.join(".template.config")).unwrap();
    std::fs::create_dir_all(&clean).unwrap();
    std::fs::create_dir_all(&selected).unwrap();
    std::fs::write(template.join(".template.config/template.json"), r#"{
        "$schema":"http://json.schemastore.org/template","author":"notypo regression tests",
        "classifications":["test"],"identity":"Notypo.CompletionRegression",
        "name":"Notypo completion regression","shortName":"notypo-fixture","sourceName":"Example",
        "symbols":{"flavor":{"type":"parameter","datatype":"choice","defaultValue":"plain",
        "choices":[{"choice":"plain","description":"Plain"},{"choice":"special","description":"Special"}]}}
    }"#).unwrap();
    std::fs::write(template.join("Example.txt"), "template fixture\n").unwrap();
    std::fs::write(context_template.join(".template.config/template.json"), r#"{
        "author":"notypo regression tests","identity":"Notypo.ProjectContextRegression",
        "name":"Notypo project context regression","shortName":"notypo-context",
        "sourceName":"Example","constraints":{"project":{"type":"project-capability","args":"NotypoTest"}}
    }"#).unwrap();
    std::fs::write(context_template.join("Example.txt"), "context fixture\n").unwrap();
    let initialize = |args: &[&str]| {
        Command::new(&dotnet)
            .current_dir(&project)
            .args(args)
            .env("DOTNET_CLI_HOME", &home)
            .env("DOTNET_CLI_TELEMETRY_OPTOUT", "1")
            .env("DOTNET_NOLOGO", "1")
            .env("DOTNET_GENERATE_ASPNET_CERTIFICATE", "false")
            .env("DOTNET_ADD_GLOBAL_TOOLS_TO_PATH", "false")
            .env("DOTNET_CLI_WORKLOAD_UPDATE_NOTIFY_DISABLE", "true")
            .env_remove("DOTNET_STARTUP_HOOKS")
            .env_remove("DOTNET_HOST_TRACE")
            .env_remove("COREHOST_TRACE")
            .env("HTTP_PROXY", "http://127.0.0.1:9")
            .env("HTTPS_PROXY", "http://127.0.0.1:9")
            .env("ALL_PROXY", "http://127.0.0.1:9")
            .env_remove("NO_PROXY")
            .env_remove("no_proxy")
            .output()
            .unwrap()
    };
    let version = initialize(&["--version"]);
    assert!(
        version.status.success(),
        "{}",
        String::from_utf8_lossy(&version.stderr)
    );
    eprintln!(
        "dotnet SDK {}",
        String::from_utf8_lossy(&version.stdout).trim()
    );
    // Install only this local data-only fixture into this test's private hive.
    let install = initialize(&[
        "new",
        "install",
        template.to_str().unwrap(),
        context_template.to_str().unwrap(),
    ]);
    assert!(
        install.status.success(),
        "{} {}",
        String::from_utf8_lossy(&install.stdout),
        String::from_utf8_lossy(&install.stderr)
    );
    std::fs::write(
        project.join("app.csproj"),
        format!(
            r#"<Project Sdk="Microsoft.NET.Sdk"><PropertyGroup>
      <TargetFrameworks>net10.0;net9.0</TargetFrameworks>
      <RuntimeIdentifiers>osx-arm64;linux-x64</RuntimeIdentifiers>
      <Configurations>Debug;Release;Preview</Configurations>
      <EvaluationMarker>$([System.IO.File]::WriteAllText('{}', 'evaluated'))</EvaluationMarker>
      </PropertyGroup><Target Name="BuildMarker" BeforeTargets="Build">
      <WriteLinesToFile File="built" Lines="operation ran" /></Target></Project>"#,
            project.join("evaluated").display()
        ),
    )
    .unwrap();
    std::fs::write(selected.join("app.csproj"), format!(r#"<Project Sdk="Microsoft.NET.Sdk"><PropertyGroup>
      <TargetFramework>net10.0</TargetFramework><RestoreSuccess>true</RestoreSuccess>
      <EvaluationMarker>$([System.IO.File]::WriteAllText('{}', 'selected project evaluated'))</EvaluationMarker>
      </PropertyGroup><ItemGroup><ProjectCapability Include="NotypoTest" /></ItemGroup></Project>"#, selected.join("evaluated").display())).unwrap();
    let run_in = |source: &str, directory: &std::path::Path, trusted: &str, yes: bool| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_notypo"));
        command
            .current_dir(directory)
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    dotnet.parent().unwrap().display(),
                    std::env::var("PATH").unwrap_or_default()
                ),
            )
            .env("XDG_CONFIG_HOME", root.join("config"))
            .env("XDG_CACHE_HOME", root.join("cache"))
            .env("XDG_DATA_HOME", root.join("data"))
            .env("DOTNET_CLI_HOME", &home)
            .env("TF_SHELL", "bash")
            .env("THEFUCK_RULES", "")
            .env("NOTYPO_TRUSTED_COMPLETERS", r#"["dotnet:sdk"]"#)
            .env("NOTYPO_TRUSTED_WORKSPACES", trusted)
            .env("NOTYPO_TRUSTED_HELP", "")
            .env("NOTYPO_DISABLED_SOURCES", "help:man:history:legacy")
            .env("NOTYPO_NO_CACHE", "1")
            .env("NOTYPO_REPLAY_FOR_DIAGNOSIS", "false")
            .env(
                "DOTNET_STARTUP_HOOKS",
                root.join("missing-startup-hook.dll"),
            )
            .env("DOTNET_HOST_TRACE", "1")
            .env("DOTNET_HOST_TRACEFILE", root.join("host-trace-marker"))
            .env("COREHOST_TRACE", "1")
            .env("COREHOST_TRACEFILE", root.join("old-host-trace-marker"))
            // Deliberately permit a property function that proves evaluation.
            .env("MSBUILDENABLEALLPROPERTYFUNCTIONS", "1")
            .env_remove("NOTYPO_EXIT_STATUS")
            .env_remove("TF_HISTORY");
        if yes {
            command.arg("-y");
        } else {
            command.arg("--json");
        }
        command.args(["--force-command", source]).output().unwrap()
    };
    let run = |source: &str, trusted: bool, yes: bool| {
        run_in(
            source,
            &project,
            if trusted {
                project.to_str().unwrap()
            } else {
                ""
            },
            yes,
        )
    };
    let refused = run("dotnet build --configuration Preveiw", false, false);
    let report: serde_json::Value = serde_json::from_slice(&refused.stdout).unwrap();
    assert_eq!(report["probes"], 0, "{report}");
    assert!(
        report["candidates"].as_array().unwrap().is_empty(),
        "{report}"
    );
    assert!(!project.join("evaluated").exists());
    fn files(path: &std::path::Path) -> Vec<(std::path::PathBuf, Vec<u8>)> {
        let mut result = Vec::new();
        for entry in std::fs::read_dir(path).unwrap().map(Result::unwrap) {
            if entry.path().is_dir() {
                result.extend(files(&entry.path()));
            } else {
                result.push((entry.path(), std::fs::read(entry.path()).unwrap()));
            }
        }
        result.sort_by(|a, b| a.0.cmp(&b.0));
        result
    }
    let original_state = files(&home);
    // Seed another real hive using the existing installed state. Its path
    // contains a space and is selected relative to the failed command.
    fn copy_tree(source: &std::path::Path, target: &std::path::Path) {
        std::fs::create_dir_all(target).unwrap();
        for entry in std::fs::read_dir(source).unwrap().map(Result::unwrap) {
            let destination = target.join(entry.file_name());
            if entry.path().is_dir() {
                copy_tree(&entry.path(), &destination);
            } else {
                std::fs::copy(entry.path(), destination).unwrap();
            }
        }
    }
    let custom = root.join("custom hive");
    copy_tree(&home.join(".templateengine"), &custom);
    let original_custom = files(&custom);
    let cases = [
        ("dotnet biuld", "dotnet build"),
        (
            "dotnet nuget loclas all --list",
            "dotnet nuget locals all --list",
        ),
        (
            "dotnet build --configuraton Preview",
            "dotnet build --configuration Preview",
        ),
        (
            "dotnet build --configuration Preveiw",
            "dotnet build --configuration Preview",
        ),
        (
            "dotnet build --framework net10.",
            "dotnet build --framework net10.0",
        ),
        (
            "dotnet build --runtime osx-aram64",
            "dotnet build --runtime osx-arm64",
        ),
        (
            "dotnet build --verbosity qiuet",
            "dotnet build --verbosity quiet",
        ),
        (
            "dotnet build /verbosoty:quiet",
            "dotnet build /verbosity:quiet",
        ),
        (
            "dotnet build /verbosity:qiuet",
            "dotnet build /verbosity:quiet",
        ),
        (
            "dotnet build --configuration:Preveiw",
            "dotnet build --configuration:Preview",
        ),
        (
            "dotnet build --self-contained --configuraton Preview",
            "dotnet build --self-contained --configuration Preview",
        ),
        (
            "dotnet build --self-contained treu --configuration Preview",
            "dotnet build --self-contained True --configuration Preview",
        ),
        ("dotnet new notypo-fixtur", "dotnet new notypo-fixture"),
        (
            "dotnet new notypo-fixture --flavr special",
            "dotnet new notypo-fixture --flavor special",
        ),
        (
            "dotnet new notypo-fixture --flavor spceial",
            "dotnet new notypo-fixture --flavor special",
        ),
        (
            "dotnet new notypo-fixture --name 'a b😀' --flavr special",
            "dotnet new notypo-fixture --name 'a b😀' --flavor special",
        ),
        (
            "dotnet new notypo-fixture --name '' --flavr special",
            "dotnet new notypo-fixture --name '' --flavor special",
        ),
        (
            "dotnet new --debug:custom-hive '../custom hive' notypo-fixtur",
            "dotnet new --debug:custom-hive '../custom hive' notypo-fixture",
        ),
        (
            "dotnet new notypo-fixture --debug:custom-hive '../custom hive' --flavr special",
            "dotnet new notypo-fixture --debug:custom-hive '../custom hive' --flavor special",
        ),
        (
            "dotnet new --debug:custom-hvie '../custom hive' notypo-fixtur",
            "dotnet new --debug:custom-hive '../custom hive' notypo-fixture",
        ),
    ];
    for (source, expected) in cases {
        let output = run(source, true, false);
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            report["candidates"][0]["command"], expected,
            "{source}: {report}"
        );
        assert_eq!(
            report["candidates"][0]["safety"]["decision"], "confirm",
            "{report}"
        );
        assert!(
            report["candidates"][0]["edits"]
                .as_array()
                .unwrap()
                .iter()
                .all(|edit| edit["via"] == "dotnet completion"),
            "{report}"
        );
        eprintln!(
            "dotnet: {source} -> {expected} ({} probes)",
            report["probes"]
        );
    }
    for source in [
        "dotnet build --self-contained true",
        "dotnet build --self-contained FALSE",
        "dotnet build --self-contained tRuE",
    ] {
        let output = run(source, true, false);
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert!(
            report["candidates"].as_array().unwrap().is_empty(),
            "valid Boolean was rewritten: {report}"
        );
        assert_eq!(report["outcome"]["kind"], "no_correction", "{report}");
    }
    for source in [
        "dotnet new --output ../selected-project notypo-contex",
        "dotnet new -o ../selected-project/missing/nested notypo-contex",
        "dotnet new --output=../selected-project notypo-contex",
        "dotnet new --project ../selected-project/app.csproj notypo-contex",
    ] {
        let output = run_in(source, &clean, clean.to_str().unwrap(), false);
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert!(
            report["notes"].as_array().unwrap().iter().any(|note| note
                .as_str()
                .is_some_and(|note| note.contains("trusted_workspaces"))),
            "{source}: {report}"
        );
        assert!(
            !selected.join("evaluated").exists(),
            "selected project evaluated without trust: {source}"
        );
    }
    for source in [
        "dotnet new --output ../selected-project notypo-contex",
        "dotnet new --project ../selected-project/app.csproj notypo-contex",
    ] {
        let output = run_in(source, &clean, root.to_str().unwrap(), false);
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            report["candidates"][0]["command"],
            source.replace("notypo-contex", "notypo-context"),
            "{report}"
        );
        assert!(
            selected.join("evaluated").exists(),
            "trusted constraint callback was not evaluated: {source}"
        );
        std::fs::remove_file(selected.join("evaluated")).unwrap();
    }
    for source in [
        "dotnet biuld",
        "dotnet build --configuration Preveiw",
        "dotnet new notypo-fixtur",
    ] {
        let refused = run(source, true, true);
        assert!(
            !refused.status.success() && refused.stdout.is_empty(),
            "{source}"
        );
    }
    assert!(
        project.join("evaluated").exists(),
        "trusted property callbacks were not consulted"
    );
    assert!(!project.join("built").exists());
    assert!(
        !root.join("host-trace-marker").exists() && !root.join("old-host-trace-marker").exists()
    );
    assert!(!project.join("Example.txt").exists());
    assert!(!project.join("obj").exists() && !project.join("bin").exists());
    assert!(!selected.join("obj").exists() && !selected.join("bin").exists());
    assert!(!selected.join("Example.txt").exists() && !selected.join("missing").exists());
    assert_eq!(
        files(&custom),
        original_custom,
        "completion changed the explicitly selected template hive"
    );
    assert_eq!(
        files(&home),
        original_state,
        "completion changed the user's template hive or SDK state"
    );
}

/// Apps' generated PowerShell completers (`rustup completions powershell`),
/// registered in a PowerShell session the way a profile does; the session
/// passes them as the integration function would, and the failed command
/// itself never runs. deno's (203 KiB) is over the 64 KiB the session may
/// pass, so it isn't checked here.
#[test]
#[ignore = "uses installed PowerShell and the PowerShell completers apps generate"]
fn installed_powershell_session_completers_repair_typos() {
    use std::process::Command;

    let Some(pwsh) = std::env::var_os("NOTYPO_TEST_PWSH")
        .map(std::path::PathBuf::from)
        .or_else(|| notypo::utils::which("pwsh"))
    else {
        eprintln!("skipped PowerShell completers: PowerShell is not installed (unverified)");
        return;
    };
    let ctx = Context::new(Settings::default(), Shell::Powershell, "fuck".into());
    let home = std::env::temp_dir().join(format!("notypo-installed-pwsh-{}", std::process::id()));
    std::fs::create_dir_all(&home).unwrap();
    // (app, generator, typo, first candidate)
    let cases = [
        (
            "rustup",
            "rustup completions powershell",
            "rustup toolchian list",
            "rustup toolchain list",
        ),
        (
            "rustup",
            "rustup completions powershell",
            "rustup toolchain list --verbos",
            "rustup toolchain list --verbose",
        ),
        (
            "mdbook",
            "mdbook completions powershell",
            "mdbook serv",
            "mdbook serve",
        ),
        (
            "mdbook",
            "mdbook completions powershell",
            "mdbook build --dest-dri out",
            "mdbook build --dest-dir out",
        ),
        (
            "elan",
            "elan completions powershell",
            "elan toolchian list",
            "elan toolchain list",
        ),
        (
            "dua",
            "dua completions powershell",
            "dua agregate .",
            "dua aggregate .",
        ),
    ];
    let mut failures = Vec::new();
    for (app, generator, typo, expected) in cases {
        if ctx.which(app).is_none() {
            eprintln!("skipped {app}: not installed (unverified)");
            continue;
        }
        let script = format!(
            "{generator} | Out-String | Invoke-Expression\n$history = $env:NOTYPO_TEST_LINE\n{}\n[Console]::Out.Write($env:NOTYPO_POWERSHELL_COMPLETIONS)",
            notypo::shells::POWERSHELL_COMPLETERS
        );
        let session = Command::new(&pwsh)
            .current_dir(&home)
            .env("NOTYPO_TEST_LINE", typo)
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-NoLogo",
                "-Command",
                &script,
            ])
            .output()
            .unwrap();
        let completions = String::from_utf8(session.stdout).unwrap();
        if !completions.contains(&format!("#notypo-command {app}")) {
            failures.push(format!("{app}: the session passed no completer"));
            continue;
        }
        let output = Command::new(env!("CARGO_BIN_EXE_notypo"))
            .current_dir(&home)
            .env("XDG_CONFIG_HOME", &home)
            .env("XDG_CACHE_HOME", &home)
            .env("TF_SHELL", "powershell")
            .env("NOTYPO_POWERSHELL", &pwsh)
            .env("NOTYPO_POWERSHELL_COMPLETIONS", &completions)
            .env(
                "NOTYPO_TRUSTED_COMPLETERS",
                r#"["rustup", "mdbook", "elan", "dua"]"#,
            )
            .env("NOTYPO_TRUSTED_HELP", "")
            .env("NOTYPO_DISABLED_SOURCES", "help:man:history:legacy")
            .env("NOTYPO_REPLAY_FOR_DIAGNOSIS", "false")
            .env_remove("NOTYPO_POWERSHELL_COMMANDS")
            .env_remove("TF_SHELL_ALIASES")
            .env_remove("NOTYPO_SHELL_FUNCTIONS")
            .args(["--json", "--force-command", typo])
            .output()
            .unwrap();
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        let candidate = &report["candidates"][0];
        eprintln!(
            "{app:>9}: {typo} -> {} ({}, {} probes)",
            candidate["command"], report["outcome"]["kind"], report["probes"]
        );
        let via = candidate["edits"].as_array().is_some_and(|edits| {
            edits
                .iter()
                .all(|edit| edit["via"] == "powershell-completer completion")
        });
        if candidate["command"] != expected || !via || candidate["safety"]["decision"] != "allow" {
            failures.push(format!("{app}: {report}"));
        }
    }
    let _ = std::fs::remove_dir_all(&home);
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// AWS CLI v1 (pip's `awscli`, whose `aws_completer` walks the CLI driver
/// rather than v2's index), when NOTYPO_TEST_AWS_V1 names the directory
/// holding its `aws` and `aws_completer`, such as a virtualenv's bin.
#[test]
#[ignore = "uses an installed AWS CLI v1 named by NOTYPO_TEST_AWS_V1"]
fn installed_aws_cli_v1_repairs_typos_through_its_completer() {
    use std::process::Command;

    let Some(bin) = std::env::var_os("NOTYPO_TEST_AWS_V1").map(std::path::PathBuf::from) else {
        eprintln!("skipped AWS CLI v1: NOTYPO_TEST_AWS_V1 is not set (unverified)");
        return;
    };
    let home = std::env::temp_dir().join(format!("notypo-installed-awsv1-{}", std::process::id()));
    std::fs::create_dir_all(&home).unwrap();
    let path = std::env::join_paths(
        std::iter::once(bin.clone()).chain(["/usr/bin", "/bin"].map(std::path::PathBuf::from)),
    )
    .unwrap();
    // (typo, first candidate, decided without asking)
    let cases = [
        (
            "aws ec2 describ-instances",
            "aws ec2 describe-instances",
            true,
        ),
        (
            "aws ec2 describe-instances --regoin eu-west-1",
            "aws ec2 describe-instances --region eu-west-1",
            true,
        ),
        ("aws s3 lss", "aws s3 ls", true),
        ("aws iam lsit-users", "aws iam list-users", true),
        (
            "aws ec2 describe-instances --output tabel",
            "aws ec2 describe-instances --output table",
            false,
        ),
    ];
    let mut failures = Vec::new();
    for (typo, expected, decided) in cases {
        let output = Command::new(env!("CARGO_BIN_EXE_notypo"))
            .current_dir(&home)
            .env("PATH", &path)
            .env("XDG_CONFIG_HOME", &home)
            .env("XDG_CACHE_HOME", &home)
            .env("TF_SHELL", "bash")
            .env("NOTYPO_EXIT_STATUS", "2")
            .env("NOTYPO_DISABLED_SOURCES", "help:man:history:legacy")
            .env("NOTYPO_REPLAY_FOR_DIAGNOSIS", "false")
            .env_remove("TF_SHELL_ALIASES")
            .env_remove("NOTYPO_SHELL_FUNCTIONS")
            .args(["--json", "--force-command", typo])
            .output()
            .unwrap();
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        let candidate = &report["candidates"][0];
        eprintln!(
            "aws v1: {typo} -> {} ({}, {} probes)",
            candidate["command"], report["outcome"]["kind"], report["probes"]
        );
        let via = candidate["edits"]
            .as_array()
            .is_some_and(|edits| edits.iter().all(|edit| edit["via"] == "aws completion"));
        let kind = if decided { "suggestion" } else { "ambiguous" };
        if candidate["command"] != expected || !via || report["outcome"]["kind"] != kind {
            failures.push(format!("{typo}: {report}"));
        }
    }
    let _ = std::fs::remove_dir_all(&home);
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
