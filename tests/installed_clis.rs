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
        ],
        // lefthook links urfave/cli v3.10+, whose completion runs Before hooks.
        trusted_help: vec!["sofka".into(), "rust:cargo".into(), "lefthook".into()],
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
                r#"["deno", "jq", "curl", "make", "gem", "python:pipx", "brew", "rsync", "python:ansible", "wget", "xz"]"#,
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
