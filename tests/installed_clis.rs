//! Opt-in checks against the CLIs installed on this machine:
//! `cargo test --test installed_clis -- --ignored --nocapture`.
//!
//! Each case asks the structured engine to repair a typo using the app's
//! own completion, offline: no command is run, no cloud API is called
//! (completer probes get no credentials and a closed HTTP proxy), and apps
//! that aren't installed are skipped and reported.
//! Set NOTYPO_TEST_SKIP_CLIS to a colon-separated list to exclude specific
//! programs from local runs (for example, docker for macOS-only validation).
//! Run with `--test-threads=4` or fewer: apps that start a runtime (garden's
//! Node, RabbitMQ's Erlang VM) can exceed the probe timeout when every
//! check runs at once.

#[test]
#[ignore = "uses installed tar, GNU tar, unzip, zip, and 7-Zip on local fixture archives"]
fn installed_archive_members_are_literal_resources_and_never_extract() {
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command;

    struct Workspace(PathBuf);
    impl Drop for Workspace {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let root = Workspace(
        std::env::temp_dir().join(format!("notypo-installed-archives-{}", std::process::id())),
    );
    fs::create_dir_all(root.0.join("inputs/docs")).unwrap();
    fs::create_dir_all(root.0.join("output")).unwrap();
    fs::write(root.0.join("broken.tar"), "not an archive").unwrap();
    let mut programs = Vec::new();
    for name in ["tar", "gtar", "unzip", "zip", "7z"] {
        let Some(path) = notypo::utils::which(name) else {
            eprintln!("skipped archive members: {name} is not installed (unverified)");
            return;
        };
        programs.push((name, path));
    }
    let program = |name| programs.iter().find(|(n, _)| *n == name).unwrap().1.clone();
    for name in [
        "docs/report.txt",
        "notes.txt",
        "space and apostrophe's.txt",
        "unicode-𝄞.txt",
        "newline\nfile",
        "valid_under_score.txt",
        "literal;$(touch probe-marker).txt",
    ] {
        fs::write(root.0.join("inputs").join(name), "fixture content").unwrap();
    }
    let create = |name, args: &[&str]| {
        let output = Command::new(program(name))
            .current_dir(root.0.join("inputs"))
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{name}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    create(
        "tar",
        &[
            "-cf",
            "../backup.tar",
            "docs",
            "notes.txt",
            "space and apostrophe's.txt",
            "unicode-𝄞.txt",
            "newline\nfile",
            "valid_under_score.txt",
            "literal;$(touch probe-marker).txt",
        ],
    );
    create(
        "zip",
        &[
            "-q",
            "../photos.zip",
            "docs/report.txt",
            "notes.txt",
            "space and apostrophe's.txt",
            "unicode-𝄞.txt",
            "newline\nfile",
            "valid_under_score.txt",
            "literal;$(touch probe-marker).txt",
        ],
    );
    create(
        "7z",
        &[
            "a",
            "../data.7z",
            "docs",
            "notes.txt",
            "space and apostrophe's.txt",
            "unicode-𝄞.txt",
            "newline\nfile",
            "valid_under_score.txt",
            "literal;$(touch probe-marker).txt",
        ],
    );
    create(
        "7z",
        &[
            "a",
            "-mhe=on",
            "-pfixture-only-password",
            "../encrypted.7z",
            "docs",
        ],
    );
    let process = |source: &str, trusted: bool| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_notypo"));
        command
            .current_dir(&root.0)
            .env("XDG_CONFIG_HOME", root.0.join("config"))
            .env("XDG_CACHE_HOME", root.0.join("cache"))
            .env("TF_SHELL", "bash")
            .env("TF_HISTORY", source)
            .env("NOTYPO_EXIT_STATUS", "1")
            .env(
                "NOTYPO_TRUSTED_COMPLETERS",
                if trusted {
                    r#"["tar","gtar","unzip","7z"]"#
                } else {
                    "[]"
                },
            )
            .env("NOTYPO_TRUSTED_HELP", "[]")
            .env("NOTYPO_DISABLED_SOURCES", "help:man:history:legacy")
            .env("NOTYPO_REPLAY_FOR_DIAGNOSIS", "false")
            .env(
                "TAR_OPTIONS",
                "--checkpoint=1 --checkpoint-action=exec=touch should-never-run",
            )
            .env("UNZIPOPT", "-d should-never-extract")
            .env_remove("TF_SHELL_ALIASES")
            .env_remove("NOTYPO_SHELL_FUNCTIONS")
            .env_remove("NOTYPO_CAPTURED_OUTPUT")
            .env_remove("NOTYPO_OUTPUT_LOG")
            .env_remove("NOTYPO_PIPESTATUS");
        command
    };
    let correct = |source: &str, trusted: bool| {
        let output = process(source, trusted).arg("--json").output().unwrap();
        assert!(
            output.status.success(),
            "{source}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap()
    };
    for (source, expected) in [
        (
            "tar xf backup.tar docs/reprot.txt",
            "tar xf backup.tar docs/report.txt",
        ),
        (
            "tar -xfbackup.tar docs/reprot.txt",
            "tar -xfbackup.tar docs/report.txt",
        ),
        (
            "gtar --extract --file=backup.tar docs/reprot.txt",
            "gtar --extract --file=backup.tar docs/report.txt",
        ),
        (
            "unzip photos.zip docs/reprot.txt",
            "unzip photos.zip docs/report.txt",
        ),
        (
            "7z x data.7z docs/reprot.txt",
            "7z x data.7z docs/report.txt",
        ),
        (
            "7z x data.7z valid_under_socre.txt",
            "7z x data.7z valid_under_score.txt",
        ),
        (
            "tar xf backup.tar 'unicode-𝄞.tx'",
            "tar xf backup.tar 'unicode-𝄞.txt'",
        ),
        (
            "7z x data.7z 'unicode-𝄞.tx'",
            "7z x data.7z 'unicode-𝄞.txt'",
        ),
        (
            "tar xf backpu.tar docs/reprot.txt",
            "tar xf backup.tar docs/report.txt",
        ),
        (
            "unzip photso.zip docs/reprot.txt",
            "unzip photos.zip docs/report.txt",
        ),
        (
            "7z x dat.7z docs/reprot.txt",
            "7z x data.7z docs/report.txt",
        ),
        (
            "gtar -I 'touch should-never-run' -xf backup.tar docs/reprot.txt",
            "gtar -I 'touch should-never-run' -xf backup.tar docs/report.txt",
        ),
    ] {
        let report = correct(source, true);
        assert_eq!(
            report["candidates"][0]["command"], expected,
            "{source}: {report}"
        );
        assert_eq!(
            report["candidates"][0]["safety"]["decision"], "confirm",
            "{report}"
        );
        assert!(
            report["candidates"][0]["safety"]["reasons"]
                .as_array()
                .unwrap()
                .iter()
                .any(|reason| reason
                    .as_str()
                    .is_some_and(|reason| reason.contains("resource"))),
            "{report}"
        );
    }
    for prefix in [
        "tar xf backup.tar",
        "gtar xf backup.tar",
        "unzip photos.zip",
        "7z x data.7z",
    ] {
        for (typed, expected) in [
            (
                "\"space and apostrophe's.tx\"",
                "space and apostrophe's.txt",
            ),
            (
                "'literal;$(touch probe-marker).tx'",
                "literal;$(touch probe-marker).txt",
            ),
        ] {
            let source = format!("{prefix} {typed}");
            let report = correct(&source, true);
            let candidate = report["candidates"][0]["command"]
                .as_str()
                .unwrap_or_else(|| panic!("{report}"));
            let script = notypo::engine::parser::parse(candidate);
            assert!(script.is_fully_supported(), "{report}");
            assert_eq!(script.commands.len(), 1, "{report}");
            assert_eq!(
                script.commands[0].words.last().unwrap().literal(),
                Some(expected),
                "{report}"
            );
            assert_eq!(
                report["candidates"][0]["safety"]["decision"], "confirm",
                "{report}"
            );
        }
    }
    use notypo::engine::parser::{Dialect, parse_with_dialect};
    for (shell, dialect) in [
        ("bash", Dialect::Posix),
        ("zsh", Dialect::Posix),
        ("fish", Dialect::Fish),
        ("tcsh", Dialect::Tcsh),
        ("ksh", Dialect::Posix),
        ("powershell", Dialect::PowerShell),
    ] {
        let source = "7z x data.7z 'literal;$(touch probe-marker).tx'";
        let output = process(source, true)
            .env("TF_SHELL", shell)
            .arg("--json")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{shell}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        let candidate = report["candidates"][0]["command"]
            .as_str()
            .unwrap_or_else(|| panic!("{shell}: {report}"));
        let script = parse_with_dialect(candidate, dialect);
        assert!(script.is_fully_supported(), "{shell}: {report}");
        assert_eq!(script.commands.len(), 1, "{shell}: {report}");
        assert_eq!(
            script.commands[0].words.last().unwrap().literal(),
            Some("literal;$(touch probe-marker).txt"),
            "{shell}: {report}"
        );
        assert_eq!(
            report["candidates"][0]["safety"]["decision"], "confirm",
            "{shell}: {report}"
        );
    }
    for source in [
        "tar xf backup.tar docs/report.txt",
        "unzip photos.zip docs/report.txt",
        "7z x data.7z docs/report.txt",
        "tar xf backup.tar docs",
        "tar xf backup.tar 'docs/*'",
        "tar xf broken.tar docs/reprot.txt",
        "7z x data.7z newlien_file",
        "unzip photos.zip 'newlien^Jfile'",
        "7z x encrypted.7z docs/reprot.txt",
        "tar cf new.tar notse.txt",
        "7z a new.7z notse.txt",
        "7z x data.7z -ooutpt",
        "unzip photos.zip -doutpt",
    ] {
        let report = correct(source, true);
        assert!(
            report["candidates"].as_array().unwrap().is_empty(),
            "{source}: {report}"
        );
    }
    let report = correct("tar xf backup.tar docs/reprot.txt", false);
    assert_eq!(report["probes"], 0, "{report}");
    let password_report = correct("7z x -pnever-log-this data.7z docs/reprot.txt", true);
    assert!(
        password_report["candidates"][0]["command"]
            .as_str()
            .unwrap()
            .ends_with("docs/report.txt"),
        "{password_report}"
    );
    assert!(
        !password_report.to_string().contains("never-log-this"),
        "password was exposed in report"
    );
    let encrypted_report = correct(
        "7z x -pfixture-only-password encrypted.7z docs/reprot.txt",
        true,
    );
    assert!(
        encrypted_report["candidates"]
            .as_array()
            .unwrap()
            .is_empty(),
        "password must not be forwarded to listing: {encrypted_report}"
    );
    assert!(
        !encrypted_report
            .to_string()
            .contains("fixture-only-password"),
        "password was exposed in report"
    );
    assert!(
        report["candidates"].as_array().unwrap().is_empty(),
        "{report}"
    );
    for source in [
        "tar xf backup.tar docs/reprot.txt",
        "unzip photos.zip docs/reprot.txt",
        "7z x data.7z docs/reprot.txt",
        "7z x data.7z -ooutpt",
    ] {
        let output = process(source, true).arg("-y").output().unwrap();
        assert!(!output.status.success(), "{source}");
        assert!(
            output.stdout.is_empty(),
            "{source}: {}",
            String::from_utf8_lossy(&output.stdout)
        );
    }
    for name in [
        "docs",
        "notes.txt",
        "should-never-run",
        "should-never-extract",
        "probe-marker",
        "new.tar",
        "new.7z",
        "outpt",
    ] {
        assert!(
            !root.0.join(name).exists(),
            "a probe or automatic correction created {name}"
        );
    }
    assert!(
        fs::read_dir(root.0.join("output"))
            .unwrap()
            .next()
            .is_none()
    );
}

/// The wider archive inventory (section 5B): single-file compressors,
/// zipinfo, ar, pax, cpio, and ditto. Their inputs are repaired from a known
/// failure at the slots their syntax gives; zipinfo and ar members come from
/// the archive's own listing. Nothing is extracted, decompressed, or run.
#[cfg(unix)]
#[test]
#[ignore = "uses the installed compressors and archivers on local fixtures"]
fn installed_compressors_and_archivers_repair_inputs_without_writing() {
    use std::fs;
    use std::path::PathBuf;
    use std::process::{Command, Stdio};

    struct Workspace(PathBuf);
    impl Drop for Workspace {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let root = Workspace(std::env::temp_dir().join(format!(
        "notypo-installed-compressors-{}",
        std::process::id()
    )));
    fs::create_dir_all(root.0.join("dir")).unwrap();
    fs::write(root.0.join("notes.txt"), "fixture content").unwrap();
    fs::write(root.0.join("dir/a.txt"), "fixture content").unwrap();
    let run = |program: &str, args: &[&str], stdin: Option<&str>, stdout: Option<&str>| {
        let Some(path) = notypo::utils::which(program) else {
            eprintln!("skipped {program}: not installed (unverified)");
            return false;
        };
        let mut command = Command::new(path);
        command
            .current_dir(&root.0)
            .args(args)
            .stdin(if stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            });
        if let Some(name) = stdout {
            command.stdout(fs::File::create(root.0.join(name)).unwrap());
        }
        let mut child = command.spawn().unwrap();
        if let Some(text) = stdin {
            use std::io::Write;
            child
                .stdin
                .take()
                .unwrap()
                .write_all(text.as_bytes())
                .unwrap();
        }
        assert!(child.wait().unwrap().success(), "{program} {args:?}");
        true
    };
    let mut cases: Vec<(&str, &str, bool)> = Vec::new();
    // (creator, args, typo, repair); every tool reads a fixture it made.
    for (program, args, output, typo, expected) in [
        (
            "gzip",
            vec!["-c", "notes.txt"],
            "backup.txt.gz",
            "gunzip backpu.txt.gz",
            "gunzip backup.txt.gz",
        ),
        (
            "gzip",
            vec!["-c", "notes.txt"],
            "backup.txt.gz",
            "zcat backpu.txt.gz",
            "zcat backup.txt.gz",
        ),
        (
            "bzip2",
            vec!["-c", "notes.txt"],
            "backup.txt.bz2",
            "bunzip2 -k backpu.txt.bz2",
            "bunzip2 -k backup.txt.bz2",
        ),
        (
            "xz",
            vec!["-c", "notes.txt"],
            "backup.txt.xz",
            "xz -d -T0 backpu.txt.xz",
            "xz -d -T0 backup.txt.xz",
        ),
        (
            "zstd",
            vec!["-q", "-c", "notes.txt"],
            "backup.txt.zst",
            "zstd -d backpu.txt.zst -o out.txt",
            "zstd -d backup.txt.zst -o out.txt",
        ),
        (
            "lz4",
            vec!["-q", "-c", "notes.txt"],
            "backup.txt.lz4",
            "lz4 -d backpu.txt.lz4 out.txt",
            "lz4 -d backup.txt.lz4 out.txt",
        ),
        (
            "brotli",
            vec!["-c", "notes.txt"],
            "backup.txt.br",
            "brotli -d -o out.txt backpu.txt.br",
            "brotli -d -o out.txt backup.txt.br",
        ),
        (
            "compress",
            vec!["-c", "notes.txt"],
            "backup.txt.Z",
            "uncompress backpu.txt.Z",
            "uncompress backup.txt.Z",
        ),
        (
            "gzip",
            vec!["-c", "notes.txt"],
            "backup.txt.gz",
            "gzip notse.txt",
            "gzip notes.txt",
        ),
        (
            "pigz",
            vec!["-c", "notes.txt"],
            "backup.txt.gz",
            "unpigz -p 2 backpu.txt.gz",
            "unpigz -p 2 backup.txt.gz",
        ),
        (
            "pbzip2",
            vec!["-c", "notes.txt"],
            "backup.txt.bz2",
            "pbzip2 -d -p2 backpu.txt.bz2",
            "pbzip2 -d -p2 backup.txt.bz2",
        ),
        (
            "lzip",
            vec!["-c", "notes.txt"],
            "backup.txt.lz",
            "plzip -d -n 2 backpu.txt.lz",
            "plzip -d -n 2 backup.txt.lz",
        ),
        (
            "lzop",
            vec!["-c", "notes.txt"],
            "backup.txt.lzo",
            "lzop -x -p/nonexistent backpu.txt.lzo",
            "lzop -x -p/nonexistent backup.txt.lzo",
        ),
        (
            "xz",
            vec!["-c", "notes.txt"],
            "backup.txt.pxz",
            "pixz -d -i backpu.txt.pxz -o out.txt",
            "pixz -d -i backup.txt.pxz -o out.txt",
        ),
    ] {
        if run(program, &args, None, Some(output)) {
            cases.push((typo, expected, false));
        }
    }
    if run(
        "zip",
        &["-q", "photos.zip", "notes.txt", "dir/a.txt"],
        None,
        None,
    ) {
        cases.push(("zipinfo photso.zip", "zipinfo photos.zip", false));
        cases.push((
            "zipinfo -1 photos.zip dri/a.txt",
            "zipinfo -1 photos.zip dir/a.txt",
            true,
        ));
        // unar's members come from lsar's JSON listing.
        cases.push((
            "unar -o out -p secret photos.zip dri/a.txt",
            "unar -o out -p *** photos.zip dir/a.txt",
            true,
        ));
        // Trusted, so lsar's Homebrew bash handler is asked about -l.
        cases.push(("lsar -l photso.zip", "lsar -l photos.zip", true));
        cases.push((
            "ouch decompress -d out photso.zip",
            "ouch decompress -d out photos.zip",
            false,
        ));
        cases.push(("ouch list photso.zip", "ouch list photos.zip", false));
        cases.push(("aunpack photso.zip", "aunpack photos.zip", false));
        cases.push(("atool -l -e photso.zip", "atool -l -e photos.zip", false));
        cases.push(("dtrx -p secret photso.zip", "dtrx -p *** photos.zip", false));
    }
    if run(
        "mksquashfs",
        &[
            "dir",
            "image.sqfs",
            "-quiet",
            "-noappend",
            "-no-progress",
            "-no-xattrs",
        ],
        None,
        None,
    ) {
        cases.push((
            "unsquashfs -d out -l imgae.sqfs",
            "unsquashfs -d out -l image.sqfs",
            false,
        ));
    }
    // cabextract cannot create a cabinet; its input is only looked up.
    fs::write(root.0.join("drivers.cab"), "").unwrap();
    if notypo::utils::which("cabextract").is_some() {
        cases.push((
            "cabextract -l drviers.cab",
            "cabextract -l drivers.cab",
            false,
        ));
    }
    if run(
        "pax",
        &["-w", "-f", "archive.pax", "notes.txt", "dir"],
        None,
        None,
    ) {
        cases.push(("pax -f archve.pax", "pax -f archive.pax", false));
        cases.push((
            "pax -rvf archve.pax notes.txt",
            "pax -rvf archive.pax notes.txt",
            false,
        ));
    }
    if run(
        "cpio",
        &["-o"],
        Some("notes.txt\ndir/a.txt\n"),
        Some("archive.cpio"),
    ) {
        cases.push(("cpio -it -F archve.cpio", "cpio -it -F archive.cpio", false));
    }
    if cfg!(target_os = "macos") && run("ditto", &["-c", "-k", "dir", "dir.zip"], None, None) {
        cases.push(("ditto -x -k dri.zip out", "ditto -x -k dir.zip out", false));
    }
    // ar needs object files on macOS; cc builds two.
    fs::write(root.0.join("f.c"), "int f(void) { return 1; }\n").unwrap();
    fs::write(root.0.join("g.c"), "int g(void) { return 2; }\n").unwrap();
    if run("cc", &["-c", "f.c", "g.c"], None, None)
        && run("ar", &["rcs", "libfoo.a", "f.o", "g.o"], None, None)
    {
        fs::remove_file(root.0.join("f.o")).unwrap();
        fs::remove_file(root.0.join("g.o")).unwrap();
        cases.push(("ar t libfo.a", "ar t libfoo.a", false));
        cases.push(("ar x libfoo.a gg.o", "ar x libfoo.a g.o", true));
        cases.push(("ar x libfo.a gg.o", "ar x libfoo.a g.o", true));
    }
    // jar needs a JDK; NOTYPO_TEST_JDK names its bin directory (macOS's
    // /usr/bin/jar is a stub without a registered JDK).
    let jdk = std::env::var_os("NOTYPO_TEST_JDK")
        .map(PathBuf::from)
        .or_else(|| notypo::utils::which("jar").and_then(|jar| jar.parent().map(PathBuf::from)))
        .filter(|bin| {
            Command::new(bin.join("jar"))
                .arg("--version")
                .output()
                .is_ok_and(|output| output.status.success())
        });
    if let Some(bin) = &jdk {
        let output = Command::new(bin.join("jar"))
            .current_dir(root.0.join("dir"))
            .args(["cf", "../app.jar", "a.txt"])
            .output()
            .unwrap();
        assert!(output.status.success());
    }
    fs::remove_file(root.0.join("notes.txt")).unwrap();
    fs::write(root.0.join("notes.txt"), "fixture content").unwrap();
    let process = |source: &str| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_notypo"));
        command
            .current_dir(&root.0)
            .env("XDG_CONFIG_HOME", root.0.join("config"))
            .env("XDG_CACHE_HOME", root.0.join("cache"))
            .env("TF_SHELL", "bash")
            .env("TF_HISTORY", source)
            .env("NOTYPO_EXIT_STATUS", "1")
            .env(
                "NOTYPO_TRUSTED_COMPLETERS",
                r#"["zipinfo","ar","unar","lsar"]"#,
            )
            .env("NOTYPO_TRUSTED_HELP", "[]")
            .env("NOTYPO_DISABLED_SOURCES", "help:man:history:legacy")
            .env("NOTYPO_REPLAY_FOR_DIAGNOSIS", "false")
            .env("GZIP", "--should-never-run")
            .env("ZIPINFO", "-should-never-run")
            .env_remove("TF_SHELL_ALIASES")
            .env_remove("NOTYPO_SHELL_FUNCTIONS")
            .env_remove("NOTYPO_CAPTURED_OUTPUT")
            .env_remove("NOTYPO_OUTPUT_LOG")
            .env_remove("NOTYPO_PIPESTATUS");
        command
    };
    let snapshot = || {
        // notypo's own configuration and cache directories aside.
        let mut names: Vec<_> = fs::read_dir(&root.0)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .filter(|name| name != "cache" && name != "config")
            .collect();
        names.sort();
        names
    };
    let before = snapshot();
    for (source, expected, listed) in &cases {
        let output = process(source).arg("--json").output().unwrap();
        assert!(output.status.success(), "{source}");
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        let candidate = &report["candidates"][0];
        assert_eq!(candidate["command"], *expected, "{source}: {report}");
        assert_eq!(candidate["safety"]["decision"], "confirm", "{report}");
        // Inputs need no probe; members are read from the archive itself
        // (and trusted shell handlers are asked about options).
        assert_eq!(report["probes"] != 0, *listed, "{source}: {report}");
        // Approval cannot be skipped with -y.
        let output = process(source).arg("-y").output().unwrap();
        assert!(
            !output.status.success() && output.stdout.is_empty(),
            "{source}"
        );
        eprintln!("{source} -> {expected}; {} probes", report["probes"]);
    }
    if let Some(bin) = jdk {
        let path = std::env::join_paths(
            std::iter::once(bin.clone())
                .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
        )
        .unwrap();
        let before = snapshot();
        for (source, expected, listed) in [
            ("jar tf ap.jar", "jar tf app.jar", false),
            ("jar xf app.jar aa.txt", "jar xf app.jar a.txt", true),
            (
                "jar -xvf ap.jar META-INF/MANIFST.MF",
                "jar -xvf app.jar META-INF/MANIFEST.MF",
                true,
            ),
            (
                "jar --list --file=ap.jar",
                "jar --list --file=app.jar",
                false,
            ),
            (
                "jar cf new.jar notse.txt",
                "jar cf new.jar notes.txt",
                false,
            ),
        ] {
            let output = process(source)
                .env("NOTYPO_TRUSTED_COMPLETERS", r#"["jar"]"#)
                // The JVM refuses to start with this; listings must clear it.
                .env("JAVA_TOOL_OPTIONS", "-XX:+NotypoNoSuchVmOption")
                .env("PATH", &path)
                .arg("--json")
                .output()
                .unwrap();
            let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            let candidate = &report["candidates"][0];
            assert_eq!(candidate["command"], expected, "{source}: {report}");
            assert_eq!(candidate["safety"]["decision"], "confirm", "{report}");
            // The old-style cluster is never offered as a subcommand repair.
            assert!(
                report["candidates"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|c| c["edits"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .all(|e| e["role"] != "Subcommand")),
                "{source}: {report}"
            );
            // Trusted jar also asks zsh's _java handler; members need jar.
            assert!(
                !listed
                    || candidate["edits"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|e| e["via"] == "jar archive listing"),
                "{source}: {report}"
            );
            eprintln!("{source} -> {expected}; {} probes", report["probes"]);
        }
        assert_eq!(snapshot(), before, "jar listing extracted or wrote a file");
    } else {
        eprintln!("skipped jar: no JDK is installed (unverified)");
    }
    // PowerShell's archive cmdlets, described by the installed pwsh.
    let pwsh = std::env::var_os("NOTYPO_TEST_PWSH")
        .map(PathBuf::from)
        .or_else(|| notypo::utils::which("pwsh"));
    if let Some(pwsh) = pwsh.filter(|_| root.0.join("photos.zip").exists()) {
        let path = std::env::join_paths(
            std::iter::once(pwsh.parent().unwrap().to_path_buf())
                .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
        )
        .unwrap();
        for (source, expected) in [
            (
                "Expand-Archive -Path:photso.zip -DestinationPath out",
                "Expand-Archive -Path:photos.zip -DestinationPath out",
            ),
            (
                "expand-archive 'photso.zip' out -Force",
                "expand-archive photos.zip out -Force",
            ),
            (
                "Compress-Archive notse.txt new.zip",
                "Compress-Archive notes.txt new.zip",
            ),
        ] {
            let output = process(source)
                .env("TF_SHELL", "powershell")
                .env("PATH", &path)
                .arg("--json")
                .output()
                .unwrap();
            let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            let candidate = &report["candidates"][0];
            assert_eq!(candidate["command"], expected, "{source}: {report}");
            assert_eq!(candidate["safety"]["decision"], "confirm", "{report}");
            eprintln!("{source} -> {expected}; {} probes", report["probes"]);
        }
    } else {
        eprintln!("skipped Expand-Archive/Compress-Archive: pwsh is not installed (unverified)");
    }
    for source in [
        "zstd -d backup.txt.zst -o outptu.txt",
        "lz4 -d backup.txt.lz4 notse.txt",
        "pax -w -f archve.pax notes.txt",
        "ar x libfoo.a missing-member.o",
        "zipinfo photos.zip -x dri/a.txt",
    ] {
        let output = process(source).arg("--json").output().unwrap();
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert!(
            report["candidates"].as_array().unwrap().is_empty(),
            "{source}: {report}"
        );
    }
    assert_eq!(snapshot(), before, "a probe or correction wrote a file");
    assert!(fs::read_dir(root.0.join("dir")).unwrap().count() == 1);
}

#[test]
#[ignore = "uses installed PowerShell parameter callbacks and an isolated test module"]
fn installed_powershell_parameter_callbacks_preserve_context_and_require_confirmation() {
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command;

    let Some(pwsh) = std::env::var_os("NOTYPO_TEST_PWSH")
        .map(PathBuf::from)
        .or_else(|| notypo::utils::which("pwsh"))
    else {
        eprintln!("skipped PowerShell parameters: PowerShell is not installed (unverified)");
        return;
    };
    struct Workspace(PathBuf);
    impl Drop for Workspace {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let workspace = Workspace(std::env::temp_dir().join(format!(
        "notypo-installed-ps-parameters-{}",
        std::process::id()
    )));
    fs::create_dir_all(&workspace.0).unwrap();
    let fixture = workspace.0.join("parameters.ps1");
    fs::write(&fixture, include_str!("data/powershell/parameters.ps1")).unwrap();
    let capture = |extra: &str, source: &str| {
        let script = format!(
            ". $env:NOTYPO_TEST_PS_FIXTURE\n{extra}\n$history = $env:NOTYPO_TEST_LINE\n{}\n[Console]::Out.Write($env:NOTYPO_POWERSHELL_PARAMETERS)",
            notypo::shells::POWERSHELL_PARAMETER_COMPLETERS
        );
        let output = Command::new(&pwsh)
            .current_dir(&workspace.0)
            .env("NOTYPO_TEST_PS_FIXTURE", &fixture)
            .env("NOTYPO_TEST_PS_MARKERS", &workspace.0)
            .env("AWS_ACCESS_KEY_ID", "test-only-credential")
            .env("AZURE_CLIENT_SECRET", "test-only-credential")
            .env("NOTYPO_TEST_LINE", source)
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-NoLogo",
                "-Command",
                &script,
            ])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let text = String::from_utf8(output.stdout).unwrap();
        let snapshot: serde_json::Value = serde_json::from_str(&text)
            .unwrap_or_else(|e| panic!("{e}: {text}; {}", String::from_utf8_lossy(&output.stderr)));
        assert_eq!(snapshot["commands"][0]["portable"], true, "{snapshot}");
        text
    };
    let snapshot = capture("", "Invoke-Fixture -Scope west -Target wesst-one");
    let modules = workspace.0.join("modules");
    let module = modules.join("ParametersModule");
    fs::create_dir_all(&module).unwrap();
    fs::write(
        module.join("ParametersModule.psm1"),
        include_str!("data/powershell/ParametersModule.psm1"),
    )
    .unwrap();
    fs::write(module.join("ParametersModule.psd1"),
        "@{ RootModule='ParametersModule.psm1'; ModuleVersion='1.0.0'; GUID='273121cb-a1c9-415f-965b-abb1223df6fa'; FunctionsToExport=@('Invoke-ModuleFixture') }").unwrap();
    let module_path = std::env::join_paths(
        std::iter::once(modules).chain(
            std::env::var_os("PSModulePath")
                .map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
                .unwrap_or_default(),
        ),
    )
    .unwrap();
    let process = |memory: &str, trusted: &str| {
        let mut process = Command::new(env!("CARGO_BIN_EXE_notypo"));
        process
            .current_dir(&workspace.0)
            .env("XDG_CONFIG_HOME", &workspace.0)
            .env("XDG_CACHE_HOME", &workspace.0)
            .env("TF_SHELL", "powershell")
            .env("NOTYPO_POWERSHELL", &pwsh)
            .env("NOTYPO_POWERSHELL_PARAMETERS", memory)
            .env(
                "NOTYPO_POWERSHELL_COMMANDS",
                "fx\tAlias\tFunction\tInvoke-Fixture",
            )
            .env("PSModulePath", &module_path)
            .env("NOTYPO_TEST_PS_MARKERS", &workspace.0)
            .env("AWS_ACCESS_KEY_ID", "test-only-credential")
            .env("AZURE_CLIENT_SECRET", "test-only-credential")
            .env("NOTYPO_TRUSTED_COMPLETERS", trusted)
            .env("NOTYPO_TRUSTED_HELP", "")
            .env("NOTYPO_DISABLED_SOURCES", "help:man:history:legacy")
            .env("NOTYPO_REPLAY_FOR_DIAGNOSIS", "false")
            .env_remove("NOTYPO_POWERSHELL_COMPLETIONS")
            .env_remove("NOTYPO_POWERSHELL_WRAPPERS")
            .env_remove("TF_SHELL_ALIASES")
            .env_remove("NOTYPO_SHELL_FUNCTIONS");
        process
    };
    let correct = |source: &str, memory: &str, trusted: &str| {
        let output = process(memory, trusted)
            .args(["--json", "--force-command", source])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert!(!workspace.0.join("operation-marker").exists(), "{report}");
        report
    };
    let trust = r#"["Invoke-Fixture"]"#;
    let report = correct("Invoke-Fixture -Target wesst-one", &snapshot, "");
    assert_eq!(report["probes"], 0, "{report}");
    assert!(
        report["candidates"].as_array().unwrap().is_empty(),
        "{report}"
    );
    assert!(!workspace.0.join("callbacks").exists());
    let report = correct("Invoke-Fixture west wesst-one", &snapshot, "");
    assert_eq!(report["probes"], 0, "{report}");
    assert!(
        report["candidates"].as_array().unwrap().is_empty(),
        "{report}"
    );
    for (source, expected, resource) in [
        (
            "Invoke-Fixture west wesst-one",
            "Invoke-Fixture west west-one",
            true,
        ),
        (
            "Invoke-Fixture wesst-one -Scope west",
            "Invoke-Fixture west-one -Scope west",
            true,
        ),
        (
            "Invoke-Fixture -Scope east east-eno",
            "Invoke-Fixture -Scope east east-one",
            true,
        ),
        (
            "Invoke-Fixture -Loud west 'team oen' -Format json",
            "Invoke-Fixture -Loud west 'team one' -Format json",
            true,
        ),
        ("fx west 'õun/üsk'", "fx west 'õun/üks'", true),
        (
            "Invoke-Fixture west '-dash-valeu'",
            "Invoke-Fixture west '-dash-value'",
            true,
        ),
        (
            "Invoke-Fixture west west-one jsno",
            "Invoke-Fixture west west-one json",
            false,
        ),
        (
            "Invoke-Fixture -Scope west '-Target' jsno",
            "Invoke-Fixture -Scope west '-Target' json",
            false,
        ),
        (
            "Invoke-Fixture -Static 'õun😀' wesst-one -Scope west",
            "Invoke-Fixture -Static 'õun😀' west-one -Scope west",
            true,
        ),
        (
            "Invoke-Fixture -Scope west -Target wesst-one",
            "Invoke-Fixture -Scope west -Target west-one",
            true,
        ),
        (
            "Invoke-Fixture -Scoe west -Targte wesst-one",
            "Invoke-Fixture -Scope west -Target west-one",
            true,
        ),
        (
            "Invoke-Fixture -Loud -s:east -t east-eno",
            "Invoke-Fixture -Loud -s:east -t east-one",
            true,
        ),
        (
            "Invoke-Fixture -Target east-eno -s:east -Loud",
            "Invoke-Fixture -Target east-one -s:east -Loud",
            true,
        ),
        (
            "fx -Scope west -Target:wesst-one",
            "fx -Scope west -Target:west-one",
            true,
        ),
        (
            "Invoke-Fixture -Target 'team oen'",
            "Invoke-Fixture -Target 'team one'",
            true,
        ),
        (
            "Invoke-Fixture -Target 'itss;touch operation-marker'",
            "Invoke-Fixture -Target 'it''s;touch operation-marker'",
            true,
        ),
        (
            "Invoke-Fixture -Target 'õun/üsk'",
            "Invoke-Fixture -Target 'õun/üks'",
            true,
        ),
        (
            "Invoke-Fixture -Target zone:bleu",
            "Invoke-Fixture -Target zone:blue",
            true,
        ),
        (
            "Invoke-Fixture -Target '-dash-valeu'",
            "Invoke-Fixture -Target '-dash-value'",
            true,
        ),
        (
            "Invoke-Fixture -Static alpah",
            "Invoke-Fixture -Static alpha",
            true,
        ),
        (
            "Invoke-Fixture -Typed typed-eno",
            "Invoke-Fixture -Typed typed-one",
            true,
        ),
        (
            "Invoke-Fixture -Factory typed-eno",
            "Invoke-Fixture -Factory typed-one",
            true,
        ),
        (
            "Invoke-Fixture -Offline offline-corret",
            "Invoke-Fixture -Offline offline-correct",
            true,
        ),
        (
            "Invoke-Fixture -Format jsno",
            "Invoke-Fixture -Format json",
            false,
        ),
        (
            "Invoke-Fixture -Scope $unknown -Format jsno",
            "Invoke-Fixture -Scope $unknown -Format json",
            false,
        ),
    ] {
        let report = correct(source, &snapshot, trust);
        let candidate = &report["candidates"][0];
        assert_eq!(candidate["command"], expected, "{source}: {report}");
        assert_eq!(
            candidate["safety"]["reasons"]
                .to_string()
                .contains("acts on the resource"),
            resource,
            "{report}"
        );
        if resource {
            assert_eq!(candidate["safety"]["decision"], "confirm", "{report}");
            let automatic = process(&snapshot, trust)
                .args(["-y", "--force-command", source])
                .output()
                .unwrap();
            assert!(
                automatic.stdout.is_empty(),
                "{source}: {}",
                String::from_utf8_lossy(&automatic.stdout)
            );
        }
        eprintln!(
            "PowerShell parameter: {source} -> {expected}; {} probes",
            report["probes"]
        );
    }
    for source in [
        "Invoke-Fixture west west-one JSON",
        "Invoke-Fixture west west-one 3",
        "Invoke-Fixture $unknown wesst-one",
        "Invoke-Fixture wesst-one -Scope $unknown",
        "Invoke-Fixture -Enabled:$false west wesst-one",
        "Invoke-Fixture -Scope -Target wesst-one",
        "Invoke-Fixture -Scope -Loud wesst-one",
        "Invoke-Fixture -Target 'west-one'",
        "Invoke-Fixture -Empty alpah",
        "Invoke-Fixture -Broken alpah",
        "Invoke-Fixture -Unsafe file-eno",
        "Invoke-Fixture -Noise west-eno",
        "Invoke-Fixture -Scope $unknown -Target wesst-one",
        "Invoke-Fixture -Enabled:$false -Target wesst-one",
        "Invoke-Fixture -Target wesst-one -Scope $unknown",
    ] {
        let before = fs::read_to_string(workspace.0.join("callbacks"))
            .unwrap()
            .lines()
            .count();
        let report = correct(source, &snapshot, trust);
        assert!(
            report["candidates"].as_array().unwrap().is_empty(),
            "{source}: {report}"
        );
        if source.contains('$') || source.starts_with("Invoke-Fixture -Scope -") {
            assert_eq!(
                before,
                fs::read_to_string(workspace.0.join("callbacks"))
                    .unwrap()
                    .lines()
                    .count()
            );
        }
    }
    let before = fs::read_to_string(workspace.0.join("callbacks"))
        .unwrap()
        .lines()
        .count();
    let report = correct(
        "Invoke-Fixture -Target wesst-one -Scoe east",
        &snapshot,
        trust,
    );
    assert_eq!(
        report["candidates"][0]["command"], "Invoke-Fixture -Target wesst-one -Scope east",
        "{report}"
    );
    assert_eq!(
        before,
        fs::read_to_string(workspace.0.join("callbacks"))
            .unwrap()
            .lines()
            .count()
    );
    let report = correct("Invoke-Fixture -Slow late-eno", &snapshot, trust);
    assert!(
        report["candidates"].as_array().unwrap().is_empty(),
        "{report}"
    );
    assert!(
        report["notes"].to_string().contains("timed out"),
        "{report}"
    );
    assert_eq!(
        fs::read_to_string(workspace.0.join("parent-statements")).unwrap(),
        "parent\n"
    );

    let extra = "Register-ArgumentCompleter -ParameterName Target -ScriptBlock { param($a,$b,$c,$d,$e); 'global-one' }\n\
                 Register-ArgumentCompleter -CommandName Invoke-Fixture -ParameterName Target -ScriptBlock { param($a,$b,$c,$d,$e); 'registered-one' }";
    let registered = capture(extra, "fx -Target registerde-one");
    let report = correct("fx -Target registerde-one", &registered, trust);
    assert_eq!(
        report["candidates"][0]["command"], "fx -Target registered-one",
        "{report}"
    );
    let report = correct("fx west registerde-one", &registered, trust);
    assert_eq!(
        report["candidates"][0]["command"], "fx west registered-one",
        "{report}"
    );
    let global = capture(
        "Register-ArgumentCompleter -ParameterName Target -ScriptBlock { param($a,$b,$c,$d,$e); 'global-one' }",
        "Invoke-Fixture -Target gloabl-one",
    );
    let report = correct("Invoke-Fixture -Target gloabl-one", &global, trust);
    assert_eq!(
        report["candidates"][0]["command"], "Invoke-Fixture -Target global-one",
        "{report}"
    );
    let report = correct("Invoke-Fixture west gloabl-one", &global, trust);
    assert_eq!(
        report["candidates"][0]["command"], "Invoke-Fixture west global-one",
        "{report}"
    );

    let sets = capture("", "Invoke-SetFixture frist-one");
    let set_trust = r#"["Invoke-SetFixture"]"#;
    for (source, expected) in [
        ("Invoke-SetFixture frist-one", "Invoke-SetFixture first-one"),
        (
            "Invoke-SetFixture secnod-one -ChooseSecond",
            "Invoke-SetFixture second-one -ChooseSecond",
        ),
        (
            "Invoke-SetFixture -ChooseSecond secnod-one",
            "Invoke-SetFixture -ChooseSecond second-one",
        ),
    ] {
        let report = correct(source, &sets, set_trust);
        assert_eq!(report["candidates"][0]["command"], expected, "{report}");
        assert_eq!(
            report["candidates"][0]["safety"]["decision"], "confirm",
            "{report}"
        );
    }
    let ambiguous = capture("", "Invoke-AmbiguousFixture frist-one");
    let report = correct(
        "Invoke-AmbiguousFixture frist-one",
        &ambiguous,
        r#"["Invoke-AmbiguousFixture"]"#,
    );
    assert!(
        report["candidates"].as_array().unwrap().is_empty(),
        "{report}"
    );
    let scoped = capture(
        "$outer = 'must-not-guess'; Register-ArgumentCompleter -CommandName Invoke-Fixture -ParameterName Target -ScriptBlock { param($a,$b,$c,$d,$e); $outer + '-one' }",
        "Invoke-Fixture -Target must-not-guess-eno",
    );
    let report = correct("Invoke-Fixture -Target must-not-guess-eno", &scoped, trust);
    assert!(
        report["candidates"].as_array().unwrap().is_empty(),
        "{report}"
    );
    assert!(
        report["notes"]
            .to_string()
            .contains("uncaptured session state"),
        "{report}"
    );
    let report = correct("Invoke-ModuleFixture -Target private-eno", "", "");
    assert!(
        report["candidates"].as_array().unwrap().is_empty(),
        "{report}"
    );
    assert!(!workspace.0.join("module-callbacks").exists());
    let report = correct(
        "Invoke-ModuleFixture -Target private-eno",
        "",
        r#"["powershell:ParametersModule"]"#,
    );
    assert_eq!(
        report["candidates"][0]["command"], "Invoke-ModuleFixture -Target private-one",
        "{report}"
    );
    assert_eq!(
        report["candidates"][0]["edits"][0]["description"], "Private module state",
        "{report}"
    );
    assert_eq!(
        report["candidates"][0]["safety"]["decision"], "confirm",
        "{report}"
    );
    let report = correct(
        "Invoke-ModuleFixture private-eno",
        "",
        r#"["powershell:ParametersModule"]"#,
    );
    assert_eq!(
        report["candidates"][0]["command"], "Invoke-ModuleFixture private-one",
        "{report}"
    );
    assert_eq!(
        report["candidates"][0]["edits"][0]["role"], "ArgumentValue",
        "{report}"
    );
    assert_eq!(
        report["candidates"][0]["safety"]["decision"], "confirm",
        "{report}"
    );
}

#[cfg(unix)]
#[test]
#[ignore = "uses Click 8 installed in NOTYPO_TEST_CLICK_PYTHON"]
fn installed_click_preserves_literal_values_and_marks_callback_values_as_resources() {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::process::Command;

    let Some(python) = std::env::var_os("NOTYPO_TEST_CLICK_PYTHON").map(PathBuf::from) else {
        eprintln!("skipped Click: NOTYPO_TEST_CLICK_PYTHON is not set (unverified)");
        return;
    };
    assert!(python.is_file(), "{}", python.display());
    let version = Command::new(&python)
        .args([
            "-c",
            "import importlib.metadata; print(importlib.metadata.version('click'))",
        ])
        .output()
        .unwrap();
    assert!(version.status.success());
    let version = String::from_utf8(version.stdout).unwrap();
    assert!(version.starts_with("8."), "Click {version}");
    eprintln!("Click {}: {}", version.trim(), python.display());
    struct Workspace(PathBuf);
    impl Drop for Workspace {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let workspace = Workspace(
        std::env::temp_dir().join(format!("notypo-installed-click-{}", std::process::id())),
    );
    fs::create_dir_all(&workspace.0).unwrap();
    fs::write(
        workspace.0.join("clickfixture.py"),
        include_str!("data/click/cli.py"),
    )
    .unwrap();
    let app = workspace.0.join("click-fixture");
    fs::write(
        &app,
        format!(
            "#!{}\nfrom clickfixture import cli\ncli()\n",
            python.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&app, fs::Permissions::from_mode(0o755)).unwrap();
    let source = |args: &str| format!("{} {args}", app.display());
    let command = |completer: bool, help: bool| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_notypo"));
        command
            .current_dir(&workspace.0)
            .env("XDG_CONFIG_HOME", &workspace.0)
            .env("XDG_CACHE_HOME", &workspace.0)
            .env("TF_SHELL", "bash")
            .env("NOTYPO_TEST_CLICK_MARKERS", &workspace.0)
            .env("NOTYPO_TRUSTED_WORKSPACES", &workspace.0)
            .env(
                "NOTYPO_TRUSTED_COMPLETERS",
                if completer {
                    r#"["python:clickfixture"]"#
                } else {
                    ""
                },
            )
            .env(
                "NOTYPO_TRUSTED_HELP",
                if help {
                    r#"["python:clickfixture"]"#
                } else {
                    ""
                },
            )
            .env("NOTYPO_DISABLED_SOURCES", "help:man:history:legacy")
            .env("NOTYPO_REPLAY_FOR_DIAGNOSIS", "false")
            .env_remove("TF_SHELL_ALIASES")
            .env_remove("NOTYPO_SHELL_FUNCTIONS")
            .env_remove("_CLICK_FIXTURE_COMPLETE");
        command
    };
    let correct = |args: &str, completer: bool, help: bool| {
        let output = command(completer, help)
            .args(["--json", "--force-command", &source(args)])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert!(!workspace.0.join("operation-marker").exists(), "{report}");
        report
    };
    for (completer, help) in [(false, true), (true, false)] {
        let report = correct("deplyo", completer, help);
        assert_eq!(report["probes"], 0, "{report}");
        assert_eq!(report["candidates"], serde_json::json!([]), "{report}");
    }
    assert!(!workspace.0.join("queries").exists());
    for (typo, expected, resource) in [
        ("deplyo", "deploy", false),
        ("deploy statsu", "deploy status", true),
        (
            "deploy status --froamt json",
            "deploy status --format json",
            false,
        ),
        (
            "deploy status --format jsno",
            "deploy status --format json",
            true,
        ),
        ("deploy status prdo", "deploy status prod", true),
        (
            "deploy status --label 'report pakcage'",
            "deploy status --label 'report package'",
            true,
        ),
    ] {
        let report = correct(typo, true, true);
        let candidate = &report["candidates"][0];
        assert_eq!(candidate["command"], source(expected), "{typo}: {report}");
        assert!(
            candidate["edits"]
                .as_array()
                .unwrap()
                .iter()
                .all(|edit| edit["via"] == "click completion"),
            "{report}"
        );
        assert_eq!(
            candidate["safety"]["decision"],
            if resource { "confirm" } else { "allow" },
            "{report}"
        );
        if typo == "deplyo" {
            assert_eq!(
                candidate["edits"][0]["description"], "Manage deployments.",
                "{report}"
            );
        }
        eprintln!("Click: {typo} -> {expected}; {} probes", report["probes"]);
    }
    for (typed, expected) in [
        ("team oen", "team one"),
        ("itss;touch operation-marker", "it's;touch operation-marker"),
        ("$(touch operatoin-marker)", "$(touch operation-marker)"),
        ("team 😀 / westren", "team 😀 / western"),
        ("team:oen", "team:one"),
        ("team\\:tow", "team\\:two"),
    ] {
        let args = format!("deploy status --team {}", notypo::shlex::quote(typed));
        let report = correct(&args, true, true);
        let candidate = &report["candidates"][0];
        assert_eq!(candidate["safety"]["decision"], "confirm", "{report}");
        assert_eq!(
            candidate["edits"][0]["description"], "Current local team",
            "{report}"
        );
        let corrected = candidate["command"].as_str().unwrap();
        let round_trip = Command::new("/bin/bash")
            .current_dir(&workspace.0)
            .args([
                "--noprofile",
                "--norc",
                "-c",
                &format!("set -- {corrected}; printf '%s\\0' \"$@\""),
            ])
            .output()
            .unwrap();
        assert!(round_trip.status.success());
        let words: Vec<&str> = std::str::from_utf8(&round_trip.stdout)
            .unwrap()
            .split_terminator('\0')
            .collect();
        assert_eq!(
            words,
            [
                app.to_str().unwrap(),
                "deploy",
                "status",
                "--team",
                expected
            ]
        );
        assert!(!workspace.0.join("operation-marker").exists());
        let automatic = command(true, true)
            .args(["-y", "--force-command", &source(&args)])
            .output()
            .unwrap();
        assert!(!automatic.status.success());
        assert!(automatic.stdout.is_empty());
    }
    for args in [
        "deploy status --file missing-file",
        "deploy status --watch prod",
        "deploy status prod",
    ] {
        let report = correct(args, true, true);
        assert_eq!(
            report["candidates"],
            serde_json::json!([]),
            "{args}: {report}"
        );
    }
    let before = correct("extensoin", true, true);
    assert_eq!(before["candidates"], serde_json::json!([]), "{before}");
    fs::write(workspace.0.join("extension-enabled"), "installed").unwrap();
    let after = correct("extensoin", true, true);
    assert_eq!(
        after["candidates"][0]["command"],
        source("extension"),
        "{after}"
    );
    assert!(workspace.0.join("resource-marker").exists());
    assert!(workspace.0.join("parameter-marker").exists());
    let queries = fs::read_to_string(workspace.0.join("queries")).unwrap();
    for line in queries.lines() {
        let query: serde_json::Value = serde_json::from_str(line).unwrap();
        assert!(
            query["args"] == serde_json::json!(["--help"])
                || (query["args"] == serde_json::json!([])
                    && matches!(
                        query["instruction"].as_str(),
                        Some("zsh_complete" | "bash_complete")
                    )),
            "{query}"
        );
    }
    assert!(!workspace.0.join("__pycache__").exists());
}

#[cfg(unix)]
#[test]
#[ignore = "uses installed Flask with an isolated application"]
fn installed_flask_lists_project_commands_only_in_trusted_workspaces() {
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command;

    let Some(flask) = std::env::var_os("NOTYPO_TEST_FLASK")
        .map(PathBuf::from)
        .or_else(|| notypo::utils::which("flask"))
    else {
        eprintln!("skipped Flask: no installed flask (unverified)");
        return;
    };
    assert!(flask.is_file(), "{}", flask.display());
    struct Workspace(PathBuf);
    impl Drop for Workspace {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let workspace = Workspace(
        std::env::temp_dir().join(format!("notypo-installed-flask-{}", std::process::id())),
    );
    fs::create_dir_all(&workspace.0).unwrap();
    fs::write(
        workspace.0.join("app.py"),
        r#"from pathlib import Path
from flask import Flask
root = Path(__file__).parent
(root / 'import-marker').write_text('application loaded')
app = Flask(__name__)
@app.cli.command('seed-db')
def seed_db():
    '''Seed the local database.'''
    (root / 'operation-marker').write_text('executed')
"#,
    )
    .unwrap();
    let source = |args: &str| format!("{} {args}", flask.display());
    let correct = |args: &str, trusted: bool| {
        let output = Command::new(env!("CARGO_BIN_EXE_notypo"))
            .current_dir(&workspace.0)
            .env("XDG_CONFIG_HOME", &workspace.0)
            .env("XDG_CACHE_HOME", &workspace.0)
            .env("TF_SHELL", "bash")
            .env(
                "NOTYPO_TRUSTED_WORKSPACES",
                if trusted {
                    workspace.0.to_str().unwrap()
                } else {
                    ""
                },
            )
            .env("NOTYPO_TRUSTED_COMPLETERS", r#"["python:flask"]"#)
            .env("NOTYPO_TRUSTED_HELP", r#"["python:flask"]"#)
            .env("NOTYPO_DISABLED_SOURCES", "help:man:history:legacy")
            .env("NOTYPO_REPLAY_FOR_DIAGNOSIS", "false")
            .env_remove("FLASK_APP")
            .env_remove("FLASK_ENV")
            .env_remove("FLASK_DEBUG")
            .env_remove("TF_SHELL_ALIASES")
            .env_remove("NOTYPO_SHELL_FUNCTIONS")
            .args(["--json", "--force-command", &source(args)])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert!(!workspace.0.join("operation-marker").exists(), "{report}");
        report
    };
    let untrusted = correct("seed-bd", false);
    assert_eq!(untrusted["probes"], 0, "{untrusted}");
    assert_eq!(
        untrusted["candidates"],
        serde_json::json!([]),
        "{untrusted}"
    );
    assert!(!workspace.0.join("import-marker").exists());
    for (typo, expected) in [
        ("seed-bd", "seed-db"),
        ("rutes", "routes"),
        ("run --prot 8000", "run --port 8000"),
    ] {
        let report = correct(typo, true);
        let candidate = &report["candidates"][0];
        assert_eq!(candidate["command"], source(expected), "{typo}: {report}");
        assert!(
            candidate["edits"]
                .as_array()
                .unwrap()
                .iter()
                .all(|edit| edit["via"] == "click completion"),
            "{report}"
        );
        if typo == "seed-bd" {
            assert_eq!(
                candidate["edits"][0]["description"], "Seed the local database.",
                "{report}"
            );
        }
        eprintln!("Flask: {typo} -> {expected}; {} probes", report["probes"]);
    }
    assert!(workspace.0.join("import-marker").exists());
    assert!(!workspace.0.join("__pycache__").exists());
}

#[cfg(unix)]
#[test]
#[ignore = "uses the Go fixture named by NOTYPO_TEST_KINGPIN_FIXTURE"]
fn installed_kingpin_suppresses_default_commands_and_resource_hints() {
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command;

    let Some(fixture) = std::env::var_os("NOTYPO_TEST_KINGPIN_FIXTURE").map(PathBuf::from) else {
        eprintln!("skipped kingpin fixture: NOTYPO_TEST_KINGPIN_FIXTURE is not set (unverified)");
        return;
    };
    assert!(fixture.is_file(), "{}", fixture.display());
    struct Workspace(PathBuf);
    impl Drop for Workspace {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let workspace = Workspace(
        std::env::temp_dir().join(format!("notypo-installed-kingpin-{}", std::process::id())),
    );
    fs::create_dir_all(&workspace.0).unwrap();
    let app = workspace.0.join("kingpin-fixture");
    fs::copy(&fixture, &app).unwrap();
    let source = |args: &str| format!("{} {args}", app.display());
    let correct = |args: &str, completer: bool, help: bool| {
        let output = Command::new(env!("CARGO_BIN_EXE_notypo"))
            .current_dir(&workspace.0)
            .env("XDG_CONFIG_HOME", &workspace.0)
            .env("XDG_CACHE_HOME", &workspace.0)
            .env("TF_SHELL", "bash")
            .env("NOTYPO_TEST_KINGPIN_MARKERS", &workspace.0)
            .env("NOTYPO_TRUSTED_WORKSPACES", &workspace.0)
            .env(
                "NOTYPO_TRUSTED_COMPLETERS",
                if completer { "kingpin-fixture" } else { "" },
            )
            .env(
                "NOTYPO_TRUSTED_HELP",
                if help { "kingpin-fixture" } else { "" },
            )
            .env("NOTYPO_DISABLED_SOURCES", "help:man:history:legacy")
            .env("NOTYPO_REPLAY_FOR_DIAGNOSIS", "false")
            .env_remove("TF_SHELL_ALIASES")
            .env_remove("NOTYPO_SHELL_FUNCTIONS")
            .args(["--json", "--force-command", &source(args)])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert!(!workspace.0.join("operation-marker").exists(), "{report}");
        assert!(!workspace.0.join("hint-marker").exists(), "{report}");
        assert!(!workspace.0.join("default-value-file").exists(), "{report}");
        report
    };
    for (completer, help) in [(false, true), (true, false)] {
        let report = correct("snapshott", completer, help);
        assert_eq!(report["probes"], 0, "{report}");
        assert_eq!(report["candidates"], serde_json::json!([]), "{report}");
    }
    assert!(!workspace.0.join("queries").exists());
    for (typo, expected) in [
        ("snapshott", "snapshot"),
        ("snapshot lsit", "snapshot list"),
        (
            "snapshot restore --paralell 4 object-id",
            "snapshot restore --parallel 4 object-id",
        ),
        ("s r --paralell 4 object-id", "s r --parallel 4 object-id"),
        (
            "--config secret-config snapshot lsit",
            "--config secret-config snapshot list",
        ),
    ] {
        let report = correct(typo, true, true);
        assert_eq!(
            report["candidates"][0]["command"],
            source(expected),
            "{typo}: {report}"
        );
        assert!(
            report["candidates"][0]["edits"]
                .as_array()
                .unwrap()
                .iter()
                .all(|edit| edit["via"] == "kingpin completion"),
            "{report}"
        );
        eprintln!(
            "kingpin fixture: {typo} -> {expected}; {} probes",
            report["probes"]
        );
    }
    for args in [
        "snapshot",
        "s",
        "snapshot restore resource-typo",
        "snapshot restore @response-file",
    ] {
        let report = correct(args, true, true);
        assert_eq!(
            report["candidates"],
            serde_json::json!([]),
            "{args}: {report}"
        );
    }
    let before = correct("extensoin", true, true);
    assert_eq!(before["candidates"], serde_json::json!([]), "{before}");
    fs::write(workspace.0.join("extension-enabled"), "installed").unwrap();
    let after = correct("extensoin", true, true);
    assert_eq!(
        after["candidates"][0]["command"],
        source("extension"),
        "{after}"
    );
    let queries = fs::read_to_string(workspace.0.join("queries")).unwrap();
    for forbidden in [
        "object-id",
        "resource-typo",
        "@response-file",
        "secret-config",
        "--paralell",
    ] {
        assert!(!queries.contains(forbidden), "{queries}");
    }
    assert!(
        queries.lines().all(|line| {
            let args: Vec<String> = serde_json::from_str(line).unwrap();
            args.first().is_some_and(|arg| arg == "--help")
                || args.starts_with(&["--completion-bash".into(), "--help".into()])
        }),
        "{queries}"
    );
    let private = workspace.0.join("notypo/kingpin-probes");
    assert!(private.is_dir());
    // The built-in help flag also suppresses kingpin's default value setters.
    assert!(!private.join("default-value-file").exists());
}

#[cfg(unix)]
#[test]
#[ignore = "uses the RabbitMQ CLI tools installed on this machine"]
fn installed_rabbitmq_repairs_commands_and_options_from_its_own_help() {
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command;

    let tools = [
        "rabbitmqctl",
        "rabbitmq-diagnostics",
        "rabbitmq-plugins",
        "rabbitmq-queues",
        "rabbitmq-streams",
        "rabbitmq-upgrade",
    ];
    let ctx = Context::new(Settings::default(), Shell::Bash, "fuck".into());
    if let Some(missing) = tools.iter().find(|tool| ctx.which(tool).is_none()) {
        eprintln!("skipped RabbitMQ: {missing} is not installed (unverified)");
        return;
    }
    struct Workspace(PathBuf);
    impl Drop for Workspace {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let workspace = Workspace(
        std::env::temp_dir().join(format!("notypo-installed-rabbitmq-{}", std::process::id())),
    );
    let home = workspace.0.join("home");
    fs::create_dir_all(&home).unwrap();
    let identities = tools.map(|tool| format!("rabbitmq:{tool}"));
    let trust = serde_json::to_string(&identities).unwrap();
    let correct = |source: &str, completer: bool, help: bool| {
        let output = Command::new(env!("CARGO_BIN_EXE_notypo"))
            .current_dir(&workspace.0)
            .env("HOME", &home)
            .env("XDG_CONFIG_HOME", &workspace.0)
            .env("XDG_CACHE_HOME", &workspace.0)
            .env("TF_SHELL", "bash")
            .env(
                "NOTYPO_TRUSTED_COMPLETERS",
                if completer { trust.as_str() } else { "" },
            )
            .env(
                "NOTYPO_TRUSTED_HELP",
                if help { trust.as_str() } else { "" },
            )
            .env("NOTYPO_DISABLED_SOURCES", "help:man:history:legacy")
            .env("NOTYPO_REPLAY_FOR_DIAGNOSIS", "false")
            .env_remove("TF_SHELL_ALIASES")
            .env_remove("NOTYPO_SHELL_FUNCTIONS")
            .env_remove("RABBITMQ_CLI_ALIASES_FILE")
            .args(["--json", "--force-command", source])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        // The VM's cookie and any crash dump stay in notypo's probe directory.
        assert!(!home.join(".erlang.cookie").exists(), "{report}");
        assert!(!workspace.0.join("erl_crash.dump").exists(), "{report}");
        report
    };
    for (completer, help) in [(false, false), (false, true), (true, false)] {
        let report = correct("rabbitmqctl lsit_queues", completer, help);
        assert_eq!(report["probes"], 0, "{report}");
        assert_eq!(report["candidates"], serde_json::json!([]), "{report}");
    }
    // (typo, repair, decided without asking, needs approval)
    for (typo, expected, decided, approval) in [
        (
            "rabbitmqctl lsit_queues",
            "rabbitmqctl list_queues",
            true,
            false,
        ),
        (
            "rabbitmqctl --quiet delete_queue --if-emtpy orders",
            "rabbitmqctl --quiet delete_queue --if-empty orders",
            false,
            true,
        ),
        (
            "rabbitmqctl -p / list_queues --onlnie",
            "rabbitmqctl -p / list_queues --online",
            false,
            false,
        ),
        (
            "rabbitmqctl --vhots / list_queues",
            "rabbitmqctl --vhost / list_queues",
            false,
            false,
        ),
        (
            "rabbitmqctl help lsit_users",
            "rabbitmqctl help list_users",
            true,
            false,
        ),
        (
            "rabbitmq-diagnostics statsu",
            "rabbitmq-diagnostics status",
            true,
            false,
        ),
        (
            "rabbitmq-plugins enabel rabbitmq_shovel",
            "rabbitmq-plugins enable rabbitmq_shovel",
            true,
            true,
        ),
        (
            "rabbitmq-queues quorum_staus orders",
            "rabbitmq-queues quorum_status orders",
            true,
            false,
        ),
        // rabbitmq-streams and rabbitmq-upgrade have no autocomplete command.
        (
            "rabbitmq-streams delete_replcia orders rabbit@host",
            "rabbitmq-streams delete_replica orders rabbit@host",
            true,
            true,
        ),
        (
            "rabbitmq-upgrade drian",
            "rabbitmq-upgrade drain",
            true,
            true,
        ),
    ] {
        let report = correct(typo, true, true);
        let candidate = &report["candidates"][0];
        assert_eq!(candidate["command"], expected, "{typo}: {report}");
        assert!(
            candidate["edits"]
                .as_array()
                .unwrap()
                .iter()
                .all(|edit| edit["via"] == "rabbitmq completion"
                    && (edit["role"] != "Subcommand" || edit["description"].is_string())),
            "{report}"
        );
        assert_eq!(
            report["outcome"]["kind"] == "suggestion",
            decided,
            "{typo}: {report}"
        );
        assert_eq!(
            candidate["safety"]["decision"],
            if approval { "confirm" } else { "allow" },
            "{typo}: {report}"
        );
        eprintln!(
            "rabbitmq: {typo} -> {expected}; {} probes",
            report["probes"]
        );
    }
    // Arguments and option values are the user's, not command words.
    for valid in [
        "rabbitmqctl list_queues name messages",
        "rabbitmqctl --node rabbit@lsit list_queues",
        "rabbitmqctl delete_queue lsit_queues",
    ] {
        let report = correct(valid, true, true);
        assert_eq!(
            report["candidates"],
            serde_json::json!([]),
            "{valid}: {report}"
        );
    }
    assert!(workspace.0.join("notypo/rabbitmq-probes").is_dir());
}

#[cfg(unix)]
#[test]
#[ignore = "uses PHP and Symfony Console named by NOTYPO_TEST_SYMFONY_AUTOLOAD"]
fn installed_symfony_completes_project_commands_and_values_without_running_them() {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::process::Command;

    let Some(autoload) = std::env::var_os("NOTYPO_TEST_SYMFONY_AUTOLOAD").map(PathBuf::from) else {
        eprintln!("skipped Symfony Console: NOTYPO_TEST_SYMFONY_AUTOLOAD is not set (unverified)");
        return;
    };
    assert!(autoload.is_file(), "{}", autoload.display());
    struct Workspace(PathBuf);
    impl Drop for Workspace {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let workspace = Workspace(
        std::env::temp_dir().join(format!("notypo-installed-symfony-{}", std::process::id())),
    );
    fs::create_dir_all(&workspace.0).unwrap();
    let script = workspace.0.join("console");
    fs::write(
        &script,
        r#"#!/usr/bin/env php
<?php
require getenv('NOTYPO_TEST_SYMFONY_AUTOLOAD');
use Symfony\Component\Console\Application;
use Symfony\Component\Console\Command\Command;
use Symfony\Component\Console\Completion\CompletionInput;
use Symfony\Component\Console\Completion\CompletionSuggestions;
use Symfony\Component\Console\Input\InputArgument;
use Symfony\Component\Console\Input\InputInterface;
use Symfony\Component\Console\Input\InputOption;
use Symfony\Component\Console\Output\OutputInterface;
class FixtureCommand extends Command {
    protected function configure(): void {
        $this->setAliases(['cc'])
            ->addOption('format', null, InputOption::VALUE_REQUIRED, 'Format')
            ->addOption('package', null, InputOption::VALUE_REQUIRED, 'Package')
            ->addOption('cache', null, InputOption::VALUE_NEGATABLE, 'Use cache')
            ->addArgument('route', InputArgument::OPTIONAL, 'Route');
    }
    public function complete(CompletionInput $input, CompletionSuggestions $suggestions): void {
        if ($input->mustSuggestOptionValuesFor('format')) {
            $suggestions->suggestValues(['json', 'yaml']);
        } elseif ($input->mustSuggestOptionValuesFor('package')) {
            $suggestions->suggestValues(['vendor/example']);
        } elseif ($input->mustSuggestArgumentValuesFor('route')) {
            $suggestions->suggestValues(['alpha room', 'beta;literal', "café's region"]);
        }
    }
    protected function execute(InputInterface $input, OutputInterface $output): int {
        file_put_contents(__DIR__.'/operation-marker', 'executed');
        return 0;
    }
}
$app = new Application('notypo Symfony fixture', '1');
$app->add(new FixtureCommand('cache:clear'));
if (is_file(__DIR__.'/extension-enabled')) {
    $app->add(new FixtureCommand('extension:inspect'));
}
$app->run();
"#,
    )
    .unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    let correct = |source: &str, completer: bool, help: bool, trusted: bool| {
        let output = Command::new(env!("CARGO_BIN_EXE_notypo"))
            .current_dir(&workspace.0)
            .env("XDG_CONFIG_HOME", &workspace.0)
            .env("XDG_CACHE_HOME", &workspace.0)
            .env("TF_SHELL", "bash")
            .env(
                "NOTYPO_TRUSTED_COMPLETERS",
                if completer { "console" } else { "" },
            )
            .env("NOTYPO_TRUSTED_HELP", if help { "console" } else { "" })
            .env(
                "NOTYPO_TRUSTED_WORKSPACES",
                if trusted {
                    workspace.0.as_os_str()
                } else {
                    std::ffi::OsStr::new("")
                },
            )
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
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert!(!workspace.0.join("operation-marker").exists(), "{report}");
        report
    };
    for (completer, help, trusted) in [
        (false, true, true),
        (true, false, true),
        (true, true, false),
    ] {
        let report = correct("./console cache:claer", completer, help, trusted);
        assert_eq!(report["probes"], 0, "{report}");
        assert_eq!(report["candidates"], serde_json::json!([]), "{report}");
    }
    for (source, expected) in [
        (
            "./console cache:claer --format json",
            "./console cache:clear --format json",
        ),
        (
            "./console --no-interaction cache:claer",
            "./console --no-interaction cache:clear",
        ),
        (
            "./console cache:clear --no-cahce",
            "./console cache:clear --no-cache",
        ),
        (
            "./console cache:clear --format jsno",
            "./console cache:clear --format json",
        ),
        (
            "./console cache:clear --format=jsno",
            "./console cache:clear --format=json",
        ),
        (
            "./console cache:clear --package vendor/exampl",
            "./console cache:clear --package vendor/example",
        ),
        ("./console cc --no-cahce", "./console cc --no-cache"),
        ("./console c:c --no-cahce", "./console c:c --no-cache"),
        (
            "./console cache:clear 'alpha rom'",
            "./console cache:clear 'alpha room'",
        ),
        (
            "./console cache:clear 'beta;litera'",
            "./console cache:clear 'beta;literal'",
        ),
        (
            "./console cache:clear \"café's regoin\"",
            "./console cache:clear 'café'\"'\"'s region'",
        ),
    ] {
        let report = correct(source, true, true, true);
        assert_eq!(
            report["candidates"][0]["command"], expected,
            "{source}: {report}"
        );
        assert!(
            report["candidates"][0]["edits"]
                .as_array()
                .unwrap()
                .iter()
                .all(|edit| edit["via"] == "symfony completion"),
            "{report}"
        );
        eprintln!(
            "Symfony: {source} -> {expected}; {} probes",
            report["probes"]
        );
        if source.contains("café") {
            let parsed = notypo::engine::parser::parse(expected);
            let word = parsed.commands[0].words.last().unwrap();
            let check = format!(
                "set -- {}; printf '%s\\n' \"$#\" \"$1\"",
                word.span.of(expected)
            );
            let output = Command::new("bash")
                .env_remove("BASH_ENV")
                .args(["--noprofile", "--norc", "-c", &check])
                .output()
                .unwrap();
            assert!(output.status.success());
            assert_eq!(
                String::from_utf8(output.stdout).unwrap(),
                "1\ncafé's region\n"
            );
        }
    }
    for command in ["cc", "c:c"] {
        let report = correct(&format!("./console {command}"), true, true, true);
        assert_eq!(
            report["candidates"],
            serde_json::json!([]),
            "valid abbreviation: {report}"
        );
    }
    let before = correct("./console extension:inspcet", true, true, true);
    assert_eq!(before["candidates"], serde_json::json!([]), "{before}");
    fs::write(workspace.0.join("extension-enabled"), "installed").unwrap();
    let after = correct("./console extension:inspcet", true, true, true);
    assert_eq!(
        after["candidates"][0]["command"], "./console extension:inspect",
        "{after}"
    );
    // PHP accepts scripts without executable permission or a shebang.
    let source = fs::read_to_string(&script).unwrap();
    fs::write(
        &script,
        source.strip_prefix("#!/usr/bin/env php\n").unwrap(),
    )
    .unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o644)).unwrap();
    for (completer, help, trusted) in [
        (false, true, true),
        (true, false, true),
        (true, true, false),
    ] {
        let report = correct("php console cache:claer", completer, help, trusted);
        assert_eq!(report["probes"], 0, "{report}");
        assert_eq!(report["candidates"], serde_json::json!([]), "{report}");
    }
    for (source, expected) in [
        ("php console cache:claer", "php console cache:clear"),
        (
            "php ./console cache:clear --format jsno",
            "php ./console cache:clear --format json",
        ),
        (
            "env APP_ENV=test php console extension:inspcet",
            "env APP_ENV=test php console extension:inspect",
        ),
    ] {
        let report = correct(source, true, true, true);
        assert_eq!(report["candidates"][0]["command"], expected, "{report}");
        assert!(
            report["candidates"][0]["edits"]
                .as_array()
                .unwrap()
                .iter()
                .all(|edit| edit["via"] == "symfony completion"),
            "{report}"
        );
        eprintln!(
            "Symfony (PHP interpreter): {source} -> {expected}; {} probes",
            report["probes"]
        );
    }
}

use notypo::engine::safety::Decision;
use notypo::engine::{self, FailureContext, Outcome};
use notypo::settings::Settings;
use notypo::shells::Shell;
use notypo::types::Context;

#[cfg(unix)]
#[test]
#[ignore = "uses Laravel installed in NOTYPO_TEST_LARAVEL_DIR and PHP"]
fn installed_laravel_repairs_artisan_without_running_project_commands() {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    let Some(installed) = std::env::var_os("NOTYPO_TEST_LARAVEL_DIR").map(PathBuf::from) else {
        eprintln!("skipped Laravel: NOTYPO_TEST_LARAVEL_DIR is not set (unverified)");
        return;
    };
    assert!(installed.join("vendor/autoload.php").is_file());
    struct Workspace(PathBuf);
    impl Drop for Workspace {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn copy_directory(source: &Path, target: &Path) {
        fs::create_dir_all(target).unwrap();
        for entry in fs::read_dir(source).unwrap().flatten() {
            let target = target.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy_directory(&entry.path(), &target);
            } else {
                fs::copy(entry.path(), target).unwrap();
            }
        }
    }
    let workspace = Workspace(
        std::env::temp_dir().join(format!("notypo-installed-laravel-{}", std::process::id())),
    );
    fs::create_dir_all(&workspace.0).unwrap();
    for directory in ["app", "bootstrap", "config", "routes", "resources"] {
        copy_directory(&installed.join(directory), &workspace.0.join(directory));
    }
    for file in ["artisan", "composer.json"] {
        fs::copy(installed.join(file), workspace.0.join(file)).unwrap();
    }
    std::os::unix::fs::symlink(
        fs::canonicalize(installed.join("vendor")).unwrap(),
        workspace.0.join("vendor"),
    )
    .unwrap();
    for directory in [
        "storage/logs",
        "storage/framework/cache",
        "storage/framework/sessions",
        "storage/framework/views",
    ] {
        fs::create_dir_all(workspace.0.join(directory)).unwrap();
    }
    fs::write(
        workspace.0.join("routes/console.php"),
        r#"<?php
use Illuminate\Support\Facades\Artisan;
Artisan::command('notypo:inspect {--format=}', function () {
    file_put_contents(base_path('operation-marker'), 'executed');
})->purpose('notypo completion fixture');
"#,
    )
    .unwrap();
    let correct = |source: &str, trusted: bool| {
        let output = Command::new(env!("CARGO_BIN_EXE_notypo"))
            .current_dir(&workspace.0)
            .env("XDG_CONFIG_HOME", &workspace.0)
            .env("XDG_CACHE_HOME", &workspace.0)
            .env("TF_SHELL", "bash")
            .env("NOTYPO_TRUSTED_COMPLETERS", "artisan")
            .env("NOTYPO_TRUSTED_HELP", "artisan")
            .env(
                "NOTYPO_TRUSTED_WORKSPACES",
                if trusted {
                    workspace.0.as_os_str()
                } else {
                    std::ffi::OsStr::new("")
                },
            )
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
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert!(!workspace.0.join("operation-marker").exists(), "{report}");
        report
    };
    let refused = correct("php artisan notypo:inspcet", false);
    assert_eq!(refused["probes"], 0, "{refused}");
    for (source, expected, risky) in [
        ("php artisan route:lsit", "php artisan route:list", false),
        (
            "php artisan migrate --pretnde",
            "php artisan migrate --pretend",
            true,
        ),
        (
            "php artisan migrate:frseh",
            "php artisan migrate:fresh",
            true,
        ),
        (
            "php artisan notypo:inspcet",
            "php artisan notypo:inspect",
            false,
        ),
        (
            "php artisan notypo:inspect --fomat=json",
            "php artisan notypo:inspect --format=json",
            false,
        ),
    ] {
        let report = correct(source, true);
        assert_eq!(
            report["candidates"][0]["command"], expected,
            "{source}: {report}"
        );
        assert!(
            report["candidates"][0]["edits"]
                .as_array()
                .unwrap()
                .iter()
                .all(|edit| edit["via"] == "symfony completion"),
            "{report}"
        );
        if risky {
            assert_eq!(
                report["candidates"][0]["safety"]["decision"], "confirm",
                "{report}"
            );
        }
        eprintln!(
            "Laravel: {source} -> {expected}; {} probes",
            report["probes"]
        );
    }
    let valid = correct("php artisan notypo:inspect", true);
    assert_eq!(valid["candidates"], serde_json::json!([]), "{valid}");
}

/// vault, OpenBao, nomad, consul, and boundary (posener/complete, audited):
/// their predictors ask loopback servers with the user's token, and vault
/// runs a configured token helper. Here every address names a listening
/// socket and vault's configuration names a marker helper; a direct query
/// shows both would be reached, and notypo's repairs reach neither.
#[cfg(unix)]
#[test]
#[ignore = "uses the HashiCorp CLIs and OpenBao installed on this machine"]
fn installed_hashicorp_clients_repair_typos_without_servers_or_token_helpers() {
    use std::fs;
    use std::io::ErrorKind;
    use std::net::TcpListener;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::process::Command;

    let ctx = Context::new(Settings::default(), Shell::Bash, "fuck".into());
    let installed: Vec<&str> = [
        "vault",
        "bao",
        "nomad",
        "consul",
        "boundary",
        "waypoint",
        "terraform",
        "packer",
    ]
    .into_iter()
    .filter(|app| {
        let found = ctx.which(app).is_some();
        if !found {
            eprintln!("skipped {app}: not installed (unverified)");
        }
        found
    })
    .collect();
    struct Workspace(PathBuf);
    impl Drop for Workspace {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let workspace = Workspace(
        std::env::temp_dir().join(format!("notypo-installed-hashicorp-{}", std::process::id())),
    );
    let home = workspace.0.join("home");
    fs::create_dir_all(&home).unwrap();
    let marker = workspace.0.join("token-helper-ran");
    let helper = workspace.0.join("token-helper");
    fs::write(
        &helper,
        format!("#!/bin/sh\ntouch '{}'\necho s.helper\n", marker.display()),
    )
    .unwrap();
    fs::set_permissions(&helper, fs::Permissions::from_mode(0o755)).unwrap();
    let config = workspace.0.join("vault.hcl");
    fs::write(
        &config,
        format!("token_helper = \"{}\"\n", helper.display()),
    )
    .unwrap();
    let server = TcpListener::bind("127.0.0.1:0").unwrap();
    server.set_nonblocking(true).unwrap();
    let address = format!("http://{}", server.local_addr().unwrap());
    let user_env = |command: &mut Command| {
        for key in ["VAULT_ADDR", "BAO_ADDR", "NOMAD_ADDR", "BOUNDARY_ADDR"] {
            command.env(key, &address);
        }
        command
            .env("CONSUL_HTTP_ADDR", server.local_addr().unwrap().to_string())
            .env("VAULT_CONFIG_PATH", &config)
            .env("BAO_CONFIG_PATH", &config)
            .env("NOMAD_TOKEN", "nomad-secret")
            .env("CONSUL_HTTP_TOKEN", "consul-secret")
            .env_remove("VAULT_TOKEN")
            .env_remove("BAO_TOKEN")
            .env_remove("VAULT_AGENT_ADDR")
            .env_remove("BAO_AGENT_ADDR")
            .env("HOME", &home);
    };
    let connections = || {
        let mut count = 0;
        loop {
            match server.accept() {
                Ok(_) => count += 1,
                Err(error) if error.kind() == ErrorKind::WouldBlock => return count,
                Err(error) => panic!("{error}"),
            }
        }
    };
    if installed.contains(&"vault") {
        // The controls: vault itself, asked as a shell would ask it, runs
        // the helper; without a helper (and so without a token) it asks the
        // server anyway.
        let line = "vault policy read ";
        for helper in [true, false] {
            let mut control = Command::new("vault");
            user_env(&mut control);
            if !helper {
                control.env("VAULT_CONFIG_PATH", "/dev/null");
            }
            // The listener never answers; vault waits this long for it.
            control
                .env("VAULT_CLIENT_TIMEOUT", "1s")
                .env("COMP_LINE", line)
                .env("COMP_POINT", line.len().to_string())
                .output()
                .unwrap();
        }
        assert!(marker.exists(), "vault ran no token helper");
        assert!(connections() > 0, "vault asked no server");
        fs::remove_file(&marker).unwrap();
    }
    let correct = |source: &str| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_notypo"));
        user_env(&mut command);
        let output = command
            .current_dir(&workspace.0)
            .env("XDG_CONFIG_HOME", &workspace.0)
            .env("XDG_CACHE_HOME", &workspace.0)
            .env("TF_SHELL", "bash")
            .env("NOTYPO_DISABLED_SOURCES", "help:man:history:legacy")
            .env("NOTYPO_REPLAY_FOR_DIAGNOSIS", "false")
            .env_remove("NOTYPO_TRUSTED_COMPLETERS")
            .env_remove("NOTYPO_NETWORK_COMPLETION")
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
    // (typo, repair, needs approval)
    for (typo, expected, approval) in [
        ("vault secrets lsit", "vault secrets list", false),
        ("vault kv gte secret/app", "vault kv get secret/app", false),
        (
            "vault token lookup -formt json",
            "vault token lookup -format json",
            false,
        ),
        (
            "vault policy raed example",
            "vault policy read example",
            false,
        ),
        (
            "vault secrets disbale kv/",
            "vault secrets disable kv/",
            true,
        ),
        (
            "vault operator raft snapshot restor backup.snap",
            "vault operator raft snapshot restore backup.snap",
            true,
        ),
        ("bao secrets lsit", "bao secrets list", false),
        (
            "bao auth disbale userpass/",
            "bao auth disable userpass/",
            true,
        ),
        ("nomad job stauts", "nomad job status", false),
        ("nomad alloc lgos abc123", "nomad alloc logs abc123", false),
        (
            "nomad job stop -purg example",
            "nomad job stop -purge example",
            true,
        ),
        (
            "nomad node drian -enable example",
            "nomad node drain -enable example",
            true,
        ),
        ("consul membrs", "consul members", false),
        ("consul kv gt app/key", "consul kv get app/key", false),
        (
            "consul snapshot restor backup.snap",
            "consul snapshot restore backup.snap",
            true,
        ),
        ("boundary targts list", "boundary targets list", false),
        (
            "boundary authenticate pasword",
            "boundary authenticate password",
            false,
        ),
        (
            "boundary sessions cancle -id s_example",
            "boundary sessions cancel -id s_example",
            true,
        ),
        ("waypoint deplyo", "waypoint deploy", false),
        ("waypoint deploy -prnue", "waypoint deploy -prune", true),
        // Go's flag package: single-dash long flags with the same effect
        // as the operations the generic gate knows.
        ("terraform aply -destroy", "terraform apply -destroy", true),
        (
            "packer biuld -force example.pkr.hcl",
            "packer build -force example.pkr.hcl",
            true,
        ),
    ] {
        let app = typo.split(' ').next().unwrap();
        if !installed.contains(&app) {
            continue;
        }
        let report = correct(typo);
        let candidate = &report["candidates"][0];
        assert_eq!(candidate["command"], expected, "{typo}: {report}");
        assert!(
            candidate["edits"]
                .as_array()
                .unwrap()
                .iter()
                .all(|edit| edit["via"] == "posener completion"),
            "{report}"
        );
        assert_eq!(report["outcome"]["kind"], "suggestion", "{typo}: {report}");
        assert_eq!(
            candidate["safety"]["decision"],
            if approval { "confirm" } else { "allow" },
            "{typo}: {report}"
        );
        assert!(report["notes"].to_string().contains("audited"), "{report}");
        eprintln!("{typo} -> {expected}; {} probes", report["probes"]);
    }
    // Resource names (policies, jobs, keys) are never looked up offline,
    // so valid ones are left alone rather than judged against a server.
    // Valid argument values missing from a predictor's fixed list (vault
    // 2.1.1 still predicts a 2018 set of engine names) aren't misspellings.
    for valid in [
        "vault policy read example",
        "nomad job status example",
        "consul kv get app/key",
        "vault secrets enable kv",
        "vault secrets enable transform",
        "vault auth enable oidc",
        "bao secrets enable kv",
        "terraform workspace select prod",
    ] {
        if installed.contains(&valid.split(' ').next().unwrap()) {
            let report = correct(valid);
            assert_eq!(
                report["candidates"],
                serde_json::json!([]),
                "{valid}: {report}"
            );
        }
    }
    assert_eq!(connections(), 0, "a completion probe reached a server");
    assert!(
        !marker.exists(),
        "a completion probe ran vault's token helper"
    );
}

/// Backup tools and dotfile managers (section 5B): restic and chezmoi are
/// audited cobra apps, rclone too; borg, yadm, tarsnap, and stow have shell
/// handlers; borgmatic, duplicity, tmutil, direnv, and dotbot document their
/// commands in --help. Discovery never opens a repository: borg's handlers
/// would run `borg list` on one, restic and rclone could run a password
/// command, and here each would leave a marker.
#[cfg(unix)]
#[test]
#[ignore = "uses the backup tools and dotfile managers installed on this machine"]
fn installed_backup_and_dotfile_tools_repair_typos_without_opening_repositories() {
    use std::fs;
    use std::io::ErrorKind;
    use std::net::TcpListener;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::process::Command;

    let ctx = Context::new(Settings::default(), Shell::Bash, "fuck".into());
    struct Workspace(PathBuf);
    impl Drop for Workspace {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let workspace = Workspace(
        std::env::temp_dir().join(format!("notypo-installed-backup-{}", std::process::id())),
    );
    let home = workspace.0.join("home");
    fs::create_dir_all(&home).unwrap();
    let marker = workspace.0.join("password-asked");
    let password = workspace.0.join("password-command");
    fs::write(
        &password,
        format!(
            "#!/bin/sh\necho \"$0\" >> '{}'\necho secret\n",
            marker.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&password, fs::Permissions::from_mode(0o755)).unwrap();
    let server = TcpListener::bind("127.0.0.1:0").unwrap();
    server.set_nonblocking(true).unwrap();
    let repository = workspace.0.join("borg-repository");
    let borg_base = workspace.0.join("borg-base");
    // A real, encrypted borg repository with two archives.
    if ctx.which("borg").is_some() {
        for args in [
            vec!["init", "-e", "repokey", repository.to_str().unwrap()],
            vec!["create", &format!("{}::a1", repository.display()), "home"],
            vec!["create", &format!("{}::a2", repository.display()), "home"],
        ] {
            let status = Command::new("borg")
                .args(&args)
                .current_dir(&workspace.0)
                .env("BORG_BASE_DIR", &borg_base)
                .env("BORG_PASSPHRASE", "secret")
                .output()
                .unwrap();
            assert!(status.status.success(), "{status:?}");
        }
    }
    let rclone_config = workspace.0.join("rclone.conf");
    fs::write(
        &rclone_config,
        format!(
            "[web]\ntype = http\nurl = http://{}/\n",
            server.local_addr().unwrap()
        ),
    )
    .unwrap();
    let trusted = "borg:yadm:tarsnap:stow";
    let help = "borgmatic:duplicity:tmutil:direnv:dotbot:yadm";
    let correct = |source: &str| {
        let output = Command::new(env!("CARGO_BIN_EXE_notypo"))
            .current_dir(&workspace.0)
            .env("HOME", &home)
            .env("XDG_CONFIG_HOME", &workspace.0)
            .env("XDG_CACHE_HOME", &workspace.0)
            .env("TF_SHELL", "bash")
            .env("NOTYPO_TRUSTED_COMPLETERS", trusted)
            .env("NOTYPO_TRUSTED_HELP", help)
            .env("NOTYPO_DISABLED_SOURCES", "history:legacy")
            .env("NOTYPO_REPLAY_FOR_DIAGNOSIS", "false")
            .env("BORG_BASE_DIR", &borg_base)
            .env("BORG_PASSCOMMAND", &password)
            .env(
                "RESTIC_REPOSITORY",
                format!("rest:http://{}/", server.local_addr().unwrap()),
            )
            .env("RESTIC_PASSWORD_COMMAND", &password)
            .env("RCLONE_CONFIG", &rclone_config)
            .env("RCLONE_PASSWORD_COMMAND", &password)
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
    let repo = repository.display().to_string();
    // (typo, repair, decided without asking, needs approval, source)
    let cases = [
        (
            "restic snapshtos".to_owned(),
            "restic snapshots".to_owned(),
            true,
            false,
            "cobra completion",
        ),
        (
            "restic backup --exclud x .".into(),
            "restic backup --exclude x .".into(),
            true,
            false,
            "cobra completion",
        ),
        (
            "restic forgt --keep-last 3".into(),
            "restic forget --keep-last 3".into(),
            true,
            true,
            "cobra completion",
        ),
        (
            "rclone copy --dry-rnu web:a b".into(),
            "rclone copy --dry-run web:a b".into(),
            true,
            false,
            "cobra completion",
        ),
        (
            "rclone purg web:x".into(),
            "rclone purge web:x".into(),
            true,
            true,
            "cobra completion",
        ),
        (
            "chezmoi aply".into(),
            "chezmoi apply".into(),
            true,
            false,
            "cobra completion",
        ),
        (
            "chezmoi destory ~/.x".into(),
            "chezmoi destroy ~/.x".into(),
            true,
            true,
            "cobra completion",
        ),
        (
            format!("borg lsit {repo}"),
            format!("borg list {repo}"),
            false,
            false,
            "bash completion",
        ),
        (
            format!("borg delete --dry-rnu {repo}::a1 a2"),
            format!("borg delete --dry-run {repo}::a1 a2"),
            false,
            true,
            "bash completion",
        ),
        (
            format!("borg prnue {repo}"),
            format!("borg prune {repo}"),
            false,
            true,
            "bash completion",
        ),
        (
            "tarsnap --lsit-archives".into(),
            "tarsnap --list-archives".into(),
            false,
            false,
            "bash completion",
        ),
        (
            "borgmatic lsit".into(),
            "borgmatic list".into(),
            false,
            false,
            "--help output",
        ),
        (
            "duplicity verfy file:///x .".into(),
            "duplicity verify file:///x .".into(),
            false,
            false,
            "--help output",
        ),
        (
            "tmutil lsitbackups".into(),
            "tmutil listbackups".into(),
            false,
            false,
            "--help output",
        ),
        (
            "direnv alow".into(),
            "direnv allow".into(),
            false,
            true,
            "--help output",
        ),
        (
            "direnv stauts".into(),
            "direnv status".into(),
            false,
            false,
            "--help output",
        ),
        (
            "dotbot --verbsoe".into(),
            "dotbot --verbose".into(),
            false,
            false,
            "--help output",
        ),
        // zsh 5.9's own `_stow` (handlers of other shells follow bash's).
        (
            "stow --simluate vim".into(),
            "stow --simulate vim".into(),
            false,
            false,
            "zsh completion",
        ),
        // yadm's bash script registers nothing without git's handler.
        (
            "yadm bootstarp".into(),
            "yadm bootstrap".into(),
            false,
            false,
            "--help output",
        ),
    ];
    for (typo, expected, decided, approval, source) in cases {
        let app = typo.split(' ').next().unwrap();
        if ctx.which(app).is_none() {
            eprintln!("skipped {app}: not installed (unverified)");
            continue;
        }
        let report = correct(&typo);
        let candidate = &report["candidates"][0];
        assert_eq!(candidate["command"], expected, "{typo}: {report}");
        assert!(
            candidate["edits"]
                .as_array()
                .unwrap()
                .iter()
                .all(|edit| edit["via"] == source),
            "{typo}: {report}"
        );
        assert_eq!(
            report["outcome"]["kind"] == "suggestion",
            decided,
            "{typo}: {report}"
        );
        assert_eq!(
            candidate["safety"]["decision"],
            if approval { "confirm" } else { "allow" },
            "{typo}: {report}"
        );
        eprintln!("{typo} -> {expected}; {} probes", report["probes"]);
    }
    // An alias is a command, not a misspelling of the command it names.
    if ctx.which("borgmatic").is_some() {
        let report = correct("borgmatic rlist");
        assert_eq!(report["candidates"], serde_json::json!([]), "{report}");
    }
    // An encrypted rclone configuration stays closed: no password command.
    if ctx.which("rclone").is_some() {
        let status = Command::new("rclone")
            .args(["config", "encryption", "set"])
            .env("RCLONE_CONFIG", &rclone_config)
            .env("RCLONE_PASSWORD_COMMAND", "echo secret")
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap();
        assert!(status.status.success(), "{status:?}");
        let report = correct("rclone lsd web:");
        assert!(report["probes"].as_u64().unwrap() > 0, "{report}");
    }
    assert!(!marker.exists(), "a probe asked for a repository password");
    match server.accept() {
        Ok(_) => panic!("a probe reached a repository or remote"),
        Err(error) => assert_eq!(error.kind(), ErrorKind::WouldBlock, "{error}"),
    }
    if ctx.which("borg").is_some() {
        assert!(
            !fs::read_dir(&repository)
                .unwrap()
                .flatten()
                .any(|entry| entry.file_name().to_string_lossy().starts_with("lock")),
            "a probe locked the borg repository"
        );
    }
}

/// Security scanners and secret managers (section 5B). Cobra, kingpin, and
/// urfave/cli apps answer their own completion under trust; the rest
/// document themselves in --help, and bw has a zsh handler. gopass lists
/// secret names from its store without decrypting: a `gpg` that records
/// its calls stands first on PATH, and none may happen.
#[cfg(unix)]
#[test]
#[ignore = "uses the security tools and secret managers installed on this machine"]
fn installed_security_tools_repair_typos_without_unlocking_or_scanning() {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::process::Command;

    let ctx = Context::new(Settings::default(), Shell::Bash, "fuck".into());
    struct Workspace(PathBuf);
    impl Drop for Workspace {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let workspace = Workspace(
        std::env::temp_dir().join(format!("notypo-installed-security-{}", std::process::id())),
    );
    let home = workspace.0.join("home");
    let store = home.join(".local/share/gopass/stores/root");
    fs::create_dir_all(store.join("web")).unwrap();
    fs::create_dir_all(home.join(".config/gopass")).unwrap();
    fs::write(store.join(".gpg-id"), "notypo@example.invalid\n").unwrap();
    fs::write(store.join("bank.gpg"), "not really encrypted\n").unwrap();
    fs::write(store.join("web/site.gpg"), "not really encrypted\n").unwrap();
    fs::write(
        home.join(".config/gopass/config"),
        format!("[mounts]\n    path = {}\n", store.display()),
    )
    .unwrap();
    let bin = workspace.0.join("bin");
    fs::create_dir_all(&bin).unwrap();
    let gpg_calls = workspace.0.join("gpg-calls");
    let wrapper = bin.join("gpg");
    fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\necho \"$*\" >> '{}'\nexit 2\n",
            gpg_calls.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let trusted = serde_json::to_string(&[
        "github.com/zricethezav/gitleaks/v8",
        "github.com/aquasecurity/tfsec/cmd/tfsec",
        "github.com/aquasecurity/kube-bench",
        "github.com/sigstore/cosign/v3/cmd/cosign",
        "github.com/anchore/syft/cmd/syft",
        "github.com/anchore/grype/cmd/grype",
        "github.com/trufflesecurity/trufflehog/v3",
        "github.com/getsops/sops/v3/cmd/sops",
        "github.com/smallstep/cli/cmd/step",
        "github.com/gopasspw/gopass",
        "go.1password.io/op/op-cli",
        "bw",
    ])
    .unwrap();
    let help = "trufflehog:gopass:osv-scanner:age:bandit:pip-audit:semgrep:lynis:clamscan:nikto:openssl:gpg:snyk";
    // snyk's Node wrapper copies its 69 MB binary into a new HOME's cache on
    // first use, which outlasts the probe limit; a user's HOME has it.
    if ctx.which("snyk").is_some() {
        Command::new("snyk")
            .arg("--version")
            .env("HOME", &home)
            .env("HTTPS_PROXY", "http://127.0.0.1:9")
            .env("HTTP_PROXY", "http://127.0.0.1:9")
            .env("NODE_USE_ENV_PROXY", "1")
            .output()
            .unwrap();
    }
    let correct = |source: &str| {
        let output = Command::new(env!("CARGO_BIN_EXE_notypo"))
            .current_dir(&workspace.0)
            .env("HOME", &home)
            .env("PATH", &path)
            .env("XDG_CONFIG_HOME", &workspace.0)
            .env("XDG_CACHE_HOME", &workspace.0)
            .env("TF_SHELL", "bash")
            .env("NOTYPO_TRUSTED_COMPLETERS", &trusted)
            .env("NOTYPO_TRUSTED_HELP", help)
            .env("NOTYPO_DISABLED_SOURCES", "history:legacy")
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
    // (typo, repair, decided without asking, needs approval, source)
    let cobra = "cobra completion";
    let urfave = "urfave completion";
    let help_text = "--help output";
    for (typo, expected, decided, approval, source) in [
        (
            "gitleaks dir --no-baner .",
            "gitleaks dir --no-banner .",
            true,
            false,
            cobra,
        ),
        (
            "tfsec --minimum-severty HIGH .",
            "tfsec --minimum-severity HIGH .",
            true,
            false,
            cobra,
        ),
        (
            "kube-bench --benchmrak cis-1.8",
            "kube-bench --benchmark cis-1.8",
            true,
            false,
            cobra,
        ),
        ("cosign verfy img", "cosign verify img", true, false, cobra),
        ("syft scna img", "syft scan img", true, false, cobra),
        ("grype db upadte", "grype db update", true, false, cobra),
        ("op itme list", "op item list", true, false, cobra),
        (
            "trufflehog filesytem .",
            "trufflehog filesystem .",
            false,
            false,
            "kingpin completion",
        ),
        (
            "sops --decrpyt x.yaml",
            "sops --decrypt x.yaml",
            true,
            false,
            urfave,
        ),
        ("step ca healht", "step ca health", false, false, urfave),
        ("gopass lsit", "gopass list", false, false, urfave),
        // A secret's name is a resource: changing it needs approval.
        ("gopass show bnak", "gopass show bank", false, true, urfave),
        (
            "osv-scanner scna source .",
            "osv-scanner scan source .",
            false,
            false,
            help_text,
        ),
        (
            "pip-audit --requirment r.txt",
            "pip-audit --requirement r.txt",
            false,
            false,
            help_text,
        ),
        ("semgrep scna .", "semgrep scan .", false, false, help_text),
        // lynis exits 64 after its banner and usage.
        (
            "lynis audti system",
            "lynis audit system",
            false,
            false,
            help_text,
        ),
        // openssl lists its commands in columns.
        (
            "openssl x509x -in c",
            "openssl x509 -in c",
            false,
            false,
            help_text,
        ),
        ("snyk tset", "snyk test", false, false, help_text),
        (
            "clamscan --remvoe -r .",
            "clamscan --remove -r .",
            false,
            true,
            "--help output and man page",
        ),
        // bitwarden's `_bw` completes on the second call.
        (
            "bw lsit items",
            "bw list items",
            false,
            false,
            "zsh completion",
        ),
    ] {
        let app = typo.split(' ').next().unwrap();
        if ctx.which(app).is_none() {
            eprintln!("skipped {app}: not installed (unverified)");
            continue;
        }
        let report = correct(typo);
        let candidate = &report["candidates"][0];
        assert_eq!(candidate["command"], expected, "{typo}: {report}");
        assert!(
            candidate["edits"]
                .as_array()
                .unwrap()
                .iter()
                .all(|edit| edit["via"] == source),
            "{typo}: {report}"
        );
        assert_eq!(
            report["outcome"]["kind"] == "suggestion",
            decided,
            "{typo}: {report}"
        );
        assert_eq!(
            candidate["safety"]["decision"],
            if approval { "confirm" } else { "allow" },
            "{typo}: {report}"
        );
        eprintln!("{typo} -> {expected}; {} probes", report["probes"]);
    }
    assert!(
        !gpg_calls.exists(),
        "a probe called gpg: {:?}",
        fs::read_to_string(&gpg_calls)
    );
}

/// CI/CD and Kubernetes development tools (section 5B): cobra apps under
/// trusted_completers (devspace is audited), urfave/cli apps that don't
/// enable completion and mirrord through trusted help. cobra lists no
/// aliases: kubefwd's `svcs` is resolved, not repaired. devspace runs a
/// devspace.yaml's command variables even for --help, so a project needs
/// trusted_workspaces before anything is probed there.
#[cfg(unix)]
#[test]
#[ignore = "uses the CI/CD and Kubernetes development tools installed on this machine"]
fn installed_ci_and_kubernetes_tools_repair_typos_without_running_project_code() {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    let ctx = Context::new(Settings::default(), Shell::Bash, "fuck".into());
    struct Workspace(PathBuf);
    impl Drop for Workspace {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let workspace =
        Workspace(std::env::temp_dir().join(format!("notypo-installed-ci-{}", std::process::id())));
    let home = workspace.0.join("home");
    let project = workspace.0.join("devspace-project");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&project).unwrap();
    let marker = workspace.0.join("project-code-ran");
    let script = workspace.0.join("mark");
    fs::write(
        &script,
        format!("#!/bin/sh\necho \"$*\" >> '{}'\n", marker.display()),
    )
    .unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(
        project.join("devspace.yaml"),
        format!(
            "version: v2beta1\nname: example\nvars:\n  FROM_SHELL:\n    command: {}\n    args: [\"var\"]\n",
            script.display()
        ),
    )
    .unwrap();
    let trusted = serde_json::to_string(&[
        "github.com/GoogleContainerTools/skaffold/v2/cmd/skaffold",
        "github.com/tilt-dev/tilt/cmd/tilt",
        "github.com/okteto/okteto",
        "github.com/txn2/kubefwd/cmd/kubefwd",
        "github.com/argoproj/argo-cd/v3/cmd",
        "github.com/fluxcd/flux2/v2/cmd/flux",
        "github.com/tektoncd/cli/cmd/tkn",
        // act's build records only `command-line-arguments`: trust its name.
        "act",
        "github.com/CircleCI-Public/circleci-cli/cmd/circleci",
        "gitlab.com/gitlab-org/cli/cmd/glab",
        "gitlab.com/gitlab-org/gitlab-runner",
        "github.com/dagger/dagger/cmd/dagger",
        "github.com/goreleaser/goreleaser/v2",
        "github.com/telepresenceio/telepresence/v2/cmd/telepresence",
        "github.com/buildkite/agent/v3",
    ])
    .unwrap();
    let help = "gitlab-runner:buildkite-agent:mirrord:devspace:garden";
    let correct = |source: &str, cwd: &Path, workspaces: &str| {
        let output = Command::new(env!("CARGO_BIN_EXE_notypo"))
            .current_dir(cwd)
            .env("HOME", &home)
            .env("XDG_CONFIG_HOME", &workspace.0)
            .env("XDG_CACHE_HOME", &workspace.0)
            .env("TF_SHELL", "bash")
            .env("NOTYPO_TRUSTED_COMPLETERS", &trusted)
            .env("NOTYPO_TRUSTED_HELP", help)
            .env("NOTYPO_TRUSTED_WORKSPACES", workspaces)
            .env("NOTYPO_DISABLED_SOURCES", "history:legacy")
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
    let cobra = "cobra completion";
    let help_text = "--help output";
    // (typo, repair, decided without asking, needs approval, source)
    for (typo, expected, decided, approval, source) in [
        (
            "skaffold dev --profle x",
            "skaffold dev --profile x",
            true,
            false,
            cobra,
        ),
        ("skaffold biuld", "skaffold build", true, false, cobra),
        ("tilt up --stram", "tilt up --stream", true, false, cobra),
        ("okteto deplyo", "okteto deploy", true, false, cobra),
        ("devspace deplyo", "devspace deploy", true, false, cobra),
        (
            "kubefwd servics -n x",
            "kubefwd services -n x",
            true,
            false,
            cobra,
        ),
        ("argocd app lsit", "argocd app list", true, false, cobra),
        (
            "flux get kustomizatons",
            "flux get kustomizations",
            true,
            false,
            cobra,
        ),
        ("tkn pipline list", "tkn pipeline list", true, false, cobra),
        ("act --lsit", "act --list", true, false, cobra),
        (
            "circleci config valdate",
            "circleci config validate",
            true,
            false,
            cobra,
        ),
        ("glab mr lsit", "glab mr list", true, false, cobra),
        ("dagger cal", "dagger call", true, false, cobra),
        (
            "goreleaser relase --clean",
            "goreleaser release --clean",
            true,
            false,
            cobra,
        ),
        (
            "telepresence conect",
            "telepresence connect",
            true,
            true,
            cobra,
        ),
        (
            "gitlab-runner verfiy",
            "gitlab-runner verify",
            false,
            false,
            help_text,
        ),
        (
            "buildkite-agent pipline upload",
            "buildkite-agent pipeline upload",
            false,
            false,
            help_text,
        ),
        (
            "mirrord exce -- x",
            "mirrord exec -- x",
            false,
            false,
            help_text,
        ),
        // garden's nested help repeats the bound command (`get status`).
        ("garden deplyo", "garden deploy", false, false, help_text),
        (
            "garden get statsu",
            "garden get status",
            false,
            false,
            help_text,
        ),
        (
            "garden cloud secrets lsit",
            "garden cloud secrets list",
            false,
            false,
            help_text,
        ),
        (
            "garden cleanup namespce",
            "garden cleanup namespace",
            false,
            true,
            help_text,
        ),
    ] {
        let app = typo.split(' ').next().unwrap();
        if ctx.which(app).is_none() {
            eprintln!("skipped {app}: not installed (unverified)");
            continue;
        }
        let report = correct(typo, &workspace.0, "");
        let candidate = &report["candidates"][0];
        assert_eq!(candidate["command"], expected, "{typo}: {report}");
        assert!(
            candidate["edits"]
                .as_array()
                .unwrap()
                .iter()
                .all(|edit| edit["via"] == source),
            "{typo}: {report}"
        );
        assert_eq!(
            report["outcome"]["kind"] == "suggestion",
            decided,
            "{typo}: {report}"
        );
        assert_eq!(
            candidate["safety"]["decision"],
            if approval { "confirm" } else { "allow" },
            "{typo}: {report}"
        );
        eprintln!("{typo} -> {expected}; {} probes", report["probes"]);
    }
    // Aliases cobra doesn't list are commands, not misspellings.
    for valid in ["kubefwd svcs -n x", "helm ls"] {
        if ctx.which(valid.split(' ').next().unwrap()).is_some() {
            let report = correct(valid, &workspace.0, "");
            assert_eq!(
                report["candidates"],
                serde_json::json!([]),
                "{valid}: {report}"
            );
        }
    }
    // Root `garden --help` resolves the project configuration to list its
    // custom commands, so a garden project needs trusted_workspaces too.
    if ctx.which("garden").is_some() {
        let garden = workspace.0.join("garden-project/services");
        fs::create_dir_all(&garden).unwrap();
        fs::write(
            workspace.0.join("garden-project/project.garden.yml"),
            "apiVersion: garden.io/v1\nkind: Project\nname: [unterminated\n",
        )
        .unwrap();
        let report = correct("garden deplyo", &garden, "");
        assert_eq!(report["probes"], 0, "{report}");
        assert_eq!(report["candidates"], serde_json::json!([]), "{report}");
        // Analytics and release checks are off: only garden's runtime
        // cache is written.
        assert!(!home.join(".garden").exists(), "garden wrote its config");
    }
    if ctx.which("devspace").is_some() {
        let report = correct("devspace deplyo", &project, "");
        assert_eq!(report["probes"], 0, "{report}");
        assert!(!marker.exists(), "an untrusted devspace.yaml ran");
        let report = correct(
            "devspace deplyo",
            &project,
            &serde_json::to_string(&[project.to_str().unwrap()]).unwrap(),
        );
        assert_eq!(
            report["candidates"][0]["command"], "devspace deploy",
            "{report}"
        );
        assert!(
            marker.exists(),
            "a trusted workspace's devspace.yaml is evaluated"
        );
    }
}

/// Git helpers, terminal UIs, and remote access tools (section 5B). git-*
/// helpers on PATH come through the git bridge; TUIs answer only --help
/// and never start (probes have no terminal); tunnels and remote targets
/// need approval.
#[cfg(unix)]
#[test]
#[ignore = "uses the git helpers, TUIs, and remote access tools installed on this machine"]
fn installed_git_helpers_tuis_and_remote_tools_repair_typos_without_launching() {
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command;

    let ctx = Context::new(Settings::default(), Shell::Bash, "fuck".into());
    struct Workspace(PathBuf);
    impl Drop for Workspace {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let workspace = Workspace(
        std::env::temp_dir().join(format!("notypo-installed-tui-{}", std::process::id())),
    );
    let home = workspace.0.join("home");
    fs::create_dir_all(&home).unwrap();
    let trusted = "github.com/netbirdio/netbird/client:github.com/gravitational/teleport/tool/tctl";
    let help =
        "lazygit:lazydocker:ctop:tig:tsh:tctl:wg-quick:openvpn:openconnect:sshuttle:git-absorb";
    let correct = |source: &str| {
        let output = Command::new(env!("CARGO_BIN_EXE_notypo"))
            .current_dir(&workspace.0)
            .env("HOME", &home)
            .env("XDG_CONFIG_HOME", &workspace.0)
            .env("XDG_CACHE_HOME", &workspace.0)
            .env("TF_SHELL", "bash")
            .env("NOTYPO_TRUSTED_COMPLETERS", trusted)
            .env("NOTYPO_TRUSTED_HELP", help)
            .env("NOTYPO_DISABLED_SOURCES", "history:legacy")
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
    let help_text = "--help output";
    let both = "--help output and man page";
    // (typo, repair, decided without asking, needs approval, source)
    for (typo, expected, decided, approval, source) in [
        ("git sumary", "git summary", true, false, "git completion"),
        ("git absrob", "git absorb", true, false, "git completion"),
        (
            "git brnachless init",
            "git branchless init",
            true,
            false,
            "git completion",
        ),
        (
            "git flwo init",
            "git flow init",
            true,
            false,
            "git completion",
        ),
        (
            "lazygit --use-confg-file x",
            "lazygit --use-config-file x",
            false,
            false,
            help_text,
        ),
        (
            "lazydocker --confg",
            "lazydocker --config",
            false,
            false,
            help_text,
        ),
        (
            "ctop -connectr docker",
            "ctop -connector docker",
            false,
            false,
            help_text,
        ),
        ("tig --al", "tig --all", false, false, both),
        (
            "git-absorb --dry-rnu",
            "git-absorb --dry-run",
            false,
            false,
            both,
        ),
        (
            "tsh lgoin --proxy p",
            "tsh login --proxy p",
            false,
            false,
            help_text,
        ),
        // kingpin documents booleans as `--[no-]forward-agent`.
        (
            "tsh ssh --forwrad-agent user@node",
            "tsh ssh --forward-agent user@node",
            false,
            false,
            help_text,
        ),
        (
            "tctl usres ls",
            "tctl users ls",
            false,
            false,
            "kingpin completion",
        ),
        (
            "netbird stauts",
            "netbird status",
            true,
            false,
            "cobra completion",
        ),
        ("netbird upp", "netbird up", true, true, "cobra completion"),
        // wg-quick's usage line: `[ up | down | save | strip ]`.
        (
            "wg-quick upp wg0",
            "wg-quick up wg0",
            false,
            true,
            help_text,
        ),
        (
            "openconnect --protocl=gp vpn.example.com",
            "openconnect --protocol=gp vpn.example.com",
            false,
            true,
            both,
        ),
        (
            "sshuttle --remot host 0/0",
            "sshuttle --remote host 0/0",
            false,
            true,
            help_text,
        ),
    ] {
        let app = typo.split(' ').next().unwrap();
        let program = if app == "git" {
            format!("git-{}", typo.split(' ').nth(1).unwrap())
        } else {
            app.to_owned()
        };
        // A misspelled helper isn't installed under that name; its real
        // name is the expected word.
        let needed = if app == "git" {
            format!("git-{}", expected.split(' ').nth(1).unwrap())
        } else {
            program
        };
        if ctx.which(app).is_none() || ctx.which(&needed).is_none() {
            eprintln!("skipped {needed}: not installed (unverified)");
            continue;
        }
        let report = correct(typo);
        let candidate = &report["candidates"][0];
        assert_eq!(candidate["command"], expected, "{typo}: {report}");
        assert!(
            candidate["edits"]
                .as_array()
                .unwrap()
                .iter()
                .all(|edit| edit["via"] == source),
            "{typo}: {report}"
        );
        assert_eq!(
            report["outcome"]["kind"] == "suggestion",
            decided,
            "{typo}: {report}"
        );
        assert_eq!(
            candidate["safety"]["decision"],
            if approval { "confirm" } else { "allow" },
            "{typo}: {report}"
        );
        eprintln!("{typo} -> {expected}; {} probes", report["probes"]);
    }
    let written: Vec<_> = walk(&home);
    assert!(written.is_empty(), "a TUI wrote files: {written:?}");

    fn walk(dir: &std::path::Path) -> Vec<PathBuf> {
        fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .flat_map(|entry| {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path)
                } else {
                    vec![path]
                }
            })
            .collect()
    }
}

/// Database migration tools and AI assistants (section 5B). Migration
/// tools document themselves in --help (dbmate through urfave/cli; knex's
/// commander and alembic's argparse layouts); rollbacks need approval and
/// no probe connects to a database. ollama is audited cobra; assistants'
/// prompts are free text and never corrected. NOTYPO_TEST_MIGRATION_PATH
/// may add directories holding npm or pip installs (knex, alembic, llm).
#[cfg(unix)]
#[test]
#[ignore = "uses the migration tools and AI assistants installed on this machine"]
fn installed_migration_and_ai_tools_repair_typos_without_touching_prompts() {
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command;

    struct Workspace(PathBuf);
    impl Drop for Workspace {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let workspace = Workspace(
        std::env::temp_dir().join(format!("notypo-installed-migration-{}", std::process::id())),
    );
    let home = workspace.0.join("home");
    fs::create_dir_all(&home).unwrap();
    // The named install directories come first.
    let path = match std::env::var("NOTYPO_TEST_MIGRATION_PATH") {
        Ok(extra) => format!("{extra}:{}", std::env::var("PATH").unwrap_or_default()),
        Err(_) => std::env::var("PATH").unwrap_or_default(),
    };
    let which =
        |app: &str| std::env::split_paths(&path).any(|dir| crate_is_executable(&dir.join(app)));
    fn crate_is_executable(path: &std::path::Path) -> bool {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    }
    let trusted = r#"["github.com/amacneil/dbmate/v2","python:llm","github.com/cli/cli/v2"]"#;
    let help = "goose:migrate:dbmate:knex:alembic:typeorm:claude:codex:gemini:opencode:python:llm:aider:sgpt";
    // A configured sgpt keeps its key here; without one it would prompt
    // (and fail closed: probes have no terminal).
    fs::create_dir_all(home.join(".config/shell_gpt")).unwrap();
    fs::write(
        home.join(".config/shell_gpt/.sgptrc"),
        "OPENAI_API_KEY=sk-test-only-not-a-key\n",
    )
    .unwrap();
    let correct = |source: &str| {
        let output = Command::new(env!("CARGO_BIN_EXE_notypo"))
            .current_dir(&workspace.0)
            .env("HOME", &home)
            .env("PATH", &path)
            .env("LLM_USER_PATH", home.join("llm"))
            .env("XDG_CONFIG_HOME", &workspace.0)
            .env("XDG_CACHE_HOME", &workspace.0)
            .env("TF_SHELL", "bash")
            .env("NOTYPO_TRUSTED_COMPLETERS", trusted)
            .env("NOTYPO_TRUSTED_HELP", help)
            .env("NOTYPO_DISABLED_SOURCES", "history:legacy")
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
    let help_text = "--help output";
    // (typo, repair, decided without asking, needs approval, source)
    for (typo, expected, decided, approval, source) in [
        ("goose stauts", "goose status", false, false, help_text),
        (
            "goose -dri migrations up",
            "goose -dir migrations up",
            false,
            false,
            help_text,
        ),
        (
            "migrate -databse x up",
            "migrate -database x up",
            false,
            false,
            help_text,
        ),
        ("dbmate stauts", "dbmate status", false, false, help_text),
        (
            "knex migrate:latset",
            "knex migrate:latest",
            false,
            false,
            help_text,
        ),
        (
            "knex migrate:rollbck",
            "knex migrate:rollback",
            false,
            true,
            help_text,
        ),
        (
            "alembic upgarde head",
            "alembic upgrade head",
            false,
            false,
            help_text,
        ),
        (
            "alembic downgrde -1",
            "alembic downgrade -1",
            false,
            true,
            help_text,
        ),
        (
            "ollama lsit",
            "ollama list",
            true,
            false,
            "cobra completion",
        ),
        (
            "ollama run --verbse llama3",
            "ollama run --verbose llama3",
            true,
            false,
            "cobra completion",
        ),
        ("codex exce x", "codex exec x", false, false, help_text),
        ("opencode rnu", "opencode run", false, false, help_text),
        (
            "gemini --mdoel x",
            "gemini --model x",
            false,
            false,
            help_text,
        ),
        (
            "llm logs --cout 3",
            "llm logs --count 3",
            true,
            false,
            "click completion",
        ),
        (
            "aider --modle sonnet x.py",
            "aider --model sonnet x.py",
            false,
            false,
            help_text,
        ),
        // gh 2.102's built-in copilot command downloads and runs the
        // Copilot CLI, so running it needs approval.
        ("gh copilto", "gh copilot", true, true, "cobra completion"),
        (
            "gh copilot --remvoe",
            "gh copilot --remove",
            true,
            true,
            "cobra completion",
        ),
        // Typer's Rich help boxes; sgpt turns its completion off.
        (
            "sgpt --modle gpt-4o hello",
            "sgpt --model gpt-4o hello",
            false,
            false,
            help_text,
        ),
    ] {
        let app = typo.split(' ').next().unwrap();
        if !which(app) {
            eprintln!("skipped {app}: not installed (unverified)");
            continue;
        }
        let report = correct(typo);
        let candidate = &report["candidates"][0];
        assert_eq!(candidate["command"], expected, "{typo}: {report}");
        assert!(
            candidate["edits"]
                .as_array()
                .unwrap()
                .iter()
                .all(|edit| edit["via"] == source),
            "{typo}: {report}"
        );
        assert_eq!(
            report["outcome"]["kind"] == "suggestion",
            decided,
            "{typo}: {report}"
        );
        assert_eq!(
            candidate["safety"]["decision"],
            if approval { "confirm" } else { "allow" },
            "{typo}: {report}"
        );
        eprintln!("{typo} -> {expected}; {} probes", report["probes"]);
    }
    // Prompts are the user's words, not misspelled commands.
    for prompt in [
        "claude explain the bug",
        "codex fix tests",
        "gemini summarize this",
        "opencode refactor auth",
        "ollama run llama3 explain recursion",
        "llm modles",
        "aider fix the bug",
        "sgpt list the files here",
        "gh copilot -- explain the bug",
    ] {
        if which(prompt.split(' ').next().unwrap()) {
            let report = correct(prompt);
            assert_eq!(
                report["candidates"],
                serde_json::json!([]),
                "{prompt}: {report}"
            );
        }
    }
    // gh's telemetry is off for probes, so no device id is recorded.
    assert!(
        !home.join(".local/state/gh").exists(),
        "gh recorded telemetry state"
    );
    // API keys typed on the line never reach the report.
    if which("aider") {
        let report = correct("aider --anthropic-api-kye sk-ant-test-secret x.py");
        assert_eq!(
            report["candidates"][0]["command"], "aider --anthropic-api-key *** x.py",
            "{report}"
        );
        assert!(
            !report.to_string().contains("sk-ant-test-secret"),
            "{report}"
        );
    }
    // Block's goose shares the migration tool's name; Homebrew links one
    // of them, so NOTYPO_TEST_BLOCK_GOOSE names the other's bin directory.
    if let Ok(bin) = std::env::var("NOTYPO_TEST_BLOCK_GOOSE") {
        let block_path = format!("{bin}:{path}");
        for (typo, expected) in [
            ("goose sesion", Some("goose session")),
            (
                "goose recipe valdate x.yaml",
                Some("goose recipe validate x.yaml"),
            ),
            (
                "goose run --instrutions x.md",
                Some("goose run --instructions x.md"),
            ),
            ("goose explain the bug", None),
        ] {
            let output = Command::new(env!("CARGO_BIN_EXE_notypo"))
                .current_dir(&workspace.0)
                .env("HOME", &home)
                .env("PATH", &block_path)
                .env("XDG_CONFIG_HOME", &workspace.0)
                .env("XDG_CACHE_HOME", &workspace.0)
                .env("TF_SHELL", "bash")
                .env("NOTYPO_TRUSTED_HELP", "goose")
                .env("NOTYPO_DISABLED_SOURCES", "history:legacy")
                .env("NOTYPO_REPLAY_FOR_DIAGNOSIS", "false")
                .args(["--json", "--force-command", typo])
                .output()
                .unwrap();
            let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(
                report["candidates"][0]["command"].as_str(),
                expected,
                "{typo}: {report}"
            );
            eprintln!("{typo} -> {expected:?}; {} probes", report["probes"]);
        }
        // Its per-run log goes nowhere: the state root is the null device.
        assert!(
            !home.join(".local/state/goose").exists(),
            "goose wrote a log"
        );
    } else {
        eprintln!("skipped Block's goose: set NOTYPO_TEST_BLOCK_GOOSE (unverified)");
    }
}

/// Network, HTTP, and service tools (section 5B): zsh 5.9's and Homebrew's
/// handlers (ssh, rsync, curl, wget, dig, tmux, mtr), kingpin (logcli), and
/// trusted help (grpcurl, httpie). Remote targets need approval, and values
/// a handler offers again after any of them (dig's record types, mtr's
/// placeholders) are resources.
#[cfg(unix)]
#[test]
#[ignore = "uses the network and service tools installed on this machine"]
fn installed_network_and_service_tools_repair_typos() {
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command;

    let ctx = Context::new(Settings::default(), Shell::Bash, "fuck".into());
    struct Workspace(PathBuf);
    impl Drop for Workspace {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let workspace = Workspace(
        std::env::temp_dir().join(format!("notypo-installed-network-{}", std::process::id())),
    );
    let home = workspace.0.join("home");
    fs::create_dir_all(&home).unwrap();
    let trusted =
        "ssh:rsync:curl:wget:dig:mtr:tmux:tailscale:github.com/grafana/loki/v3/cmd/logcli";
    let help = "grpcurl:http:logcli";
    let correct = |source: &str, shell: &str| {
        let output = Command::new(env!("CARGO_BIN_EXE_notypo"))
            .current_dir(&workspace.0)
            .env("HOME", &home)
            .env("XDG_CONFIG_HOME", &workspace.0)
            .env("XDG_CACHE_HOME", &workspace.0)
            .env("TF_SHELL", shell)
            .env("NOTYPO_TRUSTED_COMPLETERS", trusted)
            .env("NOTYPO_TRUSTED_HELP", help)
            .env("NOTYPO_DISABLED_SOURCES", "history:legacy")
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
    let zsh = "zsh completion";
    // (typo, repair, needs approval, source, shell); handler lists are
    // partial, so every repair here is asked.
    for (typo, expected, approval, source, shell) in [
        (
            "rsync --archiv src dst",
            "rsync --archive src dst",
            true,
            zsh,
            "bash",
        ),
        (
            "curl --silnt https://example.com",
            "curl --silent https://example.com",
            false,
            zsh,
            "bash",
        ),
        (
            "wget --quite https://example.com",
            "wget --quiet https://example.com",
            false,
            zsh,
            "bash",
        ),
        ("tmux new-sesion", "tmux new-session", false, zsh, "bash"),
        ("tmux attahc -t x", "tmux attach -t x", false, zsh, "bash"),
        // dig's record types come back after any of them: values.
        (
            "dig +short example.com MXX",
            "dig +short example.com MX",
            true,
            zsh,
            "bash",
        ),
        // ssh's -o keys: zsh's _ssh adds `Name=` words; only the name is
        // judged and the typed value is kept. Remote targets need approval.
        (
            "ssh -o StrictHostKeyCheking=no host",
            "ssh -o StrictHostKeyChecking=no host",
            true,
            zsh,
            "bash",
        ),
        (
            "ssh -o 'ConectTimeout 5' host",
            "ssh -o 'ConnectTimeout 5' host",
            true,
            zsh,
            "bash",
        ),
        // dig's `+` query options, listed by _dig only after a `+`; they
        // attach values with `=` and never take the next word.
        (
            "dig example.com +shrot",
            "dig example.com +short",
            false,
            zsh,
            "bash",
        ),
        (
            "dig +noal +answr example.com",
            "dig +noall +answer example.com",
            false,
            zsh,
            "bash",
        ),
        (
            "dig +tiemout=5 example.com",
            "dig +timeout=5 example.com",
            false,
            zsh,
            "bash",
        ),
        // `--report` takes no value (the handler answers alike after it).
        (
            "mtr --reprot example.com",
            "mtr --report example.com",
            false,
            "bash completion",
            "bash",
        ),
        ("tailscale stauts", "tailscale status", false, zsh, "zsh"),
        (
            "logcli qeury x",
            "logcli query x",
            false,
            "kingpin completion",
            "bash",
        ),
        (
            "grpcurl -plaintxt host:443 list",
            "grpcurl -plaintext host:443 list",
            false,
            "--help output",
            "bash",
        ),
        (
            "http --verfy=no example.com",
            "http --verify=no example.com",
            false,
            "--help output and man page",
            "bash",
        ),
    ] {
        let app = typo.split(' ').next().unwrap();
        if ctx.which(app).is_none() {
            eprintln!("skipped {app}: not installed (unverified)");
            continue;
        }
        let report = correct(typo, shell);
        let candidate = &report["candidates"][0];
        assert_eq!(candidate["command"], expected, "{typo}: {report}");
        assert!(
            candidate["edits"]
                .as_array()
                .unwrap()
                .iter()
                .all(|edit| edit["via"] == source),
            "{typo}: {report}"
        );
        assert_ne!(report["outcome"]["kind"], "suggestion", "{typo}: {report}");
        assert_eq!(
            candidate["safety"]["decision"],
            if approval { "confirm" } else { "allow" },
            "{typo}: {report}"
        );
        eprintln!("{typo} -> {expected}; {} probes", report["probes"]);
    }
    // Valid query options are left alone.
    if ctx.which("dig").is_some() {
        for valid in [
            "dig +short +timeout=5 example.com MX",
            "dig example.com +nocmd",
            "ssh -o StrictHostKeyChecking=no host",
            "ssh -o 'BatchMode yes' host",
        ] {
            let report = correct(valid, "bash");
            assert_eq!(
                report["candidates"],
                serde_json::json!([]),
                "{valid}: {report}"
            );
        }
    }
}

/// Web servers, proxies, and certificates (section 5B): caddy (cobra; an
/// xcaddy build needs trust), nginx and httpd (the short help flag their man
/// pages document; `--help` would start httpd), traefik's dotted options,
/// certbot's `commands` help topic, mkcert (its startup `brew` is stubbed),
/// envoy (trusted help), and acme.sh and apachectl (shell handlers).
/// Stopping servers, trust-store changes, and certificate removal need
/// approval; nothing writes to HOME.
#[cfg(unix)]
#[test]
#[ignore = "uses the web servers and certificate tools installed on this machine"]
fn installed_web_servers_and_certificate_tools_repair_typos_without_starting_them() {
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command;

    let ctx = Context::new(Settings::default(), Shell::Bash, "fuck".into());
    struct Workspace(PathBuf);
    impl Drop for Workspace {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let workspace = Workspace(
        std::env::temp_dir().join(format!("notypo-installed-web-{}", std::process::id())),
    );
    let home = workspace.0.join("home");
    fs::create_dir_all(&home).unwrap();
    let correct = |source: &str| {
        let output = Command::new(env!("CARGO_BIN_EXE_notypo"))
            .current_dir(&workspace.0)
            .env("HOME", &home)
            .env("XDG_CONFIG_HOME", &workspace.0)
            .env("XDG_CACHE_HOME", &workspace.0)
            .env("TF_SHELL", "bash")
            .env("NOTYPO_TRUSTED_COMPLETERS", "caddy:acme.sh:apachectl")
            .env(
                "NOTYPO_TRUSTED_HELP",
                "nginx:httpd:traefik:certbot:mkcert:envoy",
            )
            .env("NOTYPO_DISABLED_SOURCES", "history:legacy")
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
    let help = "--help output";
    // (typo, repair, needs approval, source)
    for (typo, expected, approval, source) in [
        ("caddy valdate", "caddy validate", false, "cobra completion"),
        (
            "caddy run --confg Caddyfile",
            "caddy run --config Caddyfile",
            false,
            "cobra completion",
        ),
        ("caddy stpo", "caddy stop", true, "cobra completion"),
        ("caddy untrsut", "caddy untrust", true, "cobra completion"),
        // nginx rejects --help; its man page documents -h, whose `-s`
        // values close a description naming the placeholder.
        ("nginx -s relod", "nginx -s reload", true, help),
        // httpd's -k values come from its synopsis.
        ("httpd -k gracefl", "httpd -k graceful", true, help),
        (
            "traefik --log.levl=DEBUG",
            "traefik --log.level=DEBUG",
            false,
            help,
        ),
        ("traefik helthcheck", "traefik healthcheck", false, help),
        ("certbot renw", "certbot renew", false, help),
        ("certbot rollbak", "certbot rollback", true, help),
        (
            "certbot certonly --standalon -d example.com",
            "certbot certonly --standalone -d example.com",
            false,
            help,
        ),
        (
            "mkcert -pkcs21 example.test",
            "mkcert -pkcs12 example.test",
            false,
            help,
        ),
        (
            "envoy --mdoe validate -c envoy.yaml",
            "envoy --mode validate -c envoy.yaml",
            false,
            help,
        ),
        (
            "acme.sh --isue -d example.com",
            "acme.sh --issue -d example.com",
            false,
            "bash completion",
        ),
        (
            "acme.sh --revok -d example.com",
            "acme.sh --revoke -d example.com",
            true,
            "bash completion",
        ),
        (
            "apachectl configtets",
            "apachectl configtest",
            false,
            "zsh completion",
        ),
    ] {
        let app = typo.split(' ').next().unwrap();
        if ctx.which(app).is_none() {
            eprintln!("skipped {app}: not installed (unverified)");
            continue;
        }
        let report = correct(typo);
        let candidate = &report["candidates"][0];
        assert_eq!(candidate["command"], expected, "{typo}: {report}");
        assert!(
            candidate["edits"]
                .as_array()
                .unwrap()
                .iter()
                .all(|edit| edit["via"] == source),
            "{typo}: {report}"
        );
        assert_eq!(
            candidate["safety"]["decision"],
            if approval { "confirm" } else { "allow" },
            "{typo}: {report}"
        );
        eprintln!("{typo} -> {expected}; {} probes", report["probes"]);
    }
    for valid in ["nginx -s reload", "httpd -t -k graceful", "certbot renew"] {
        if ctx.which(valid.split(' ').next().unwrap()).is_some() {
            let report = correct(valid);
            assert_eq!(
                report["candidates"],
                serde_json::json!([]),
                "{valid}: {report}"
            );
        }
    }
    // No probe started a server, opened a trust store, or ran Homebrew.
    let written: Vec<_> = walk(&home);
    assert!(written.is_empty(), "probes wrote {written:?}");
}

/// Media, documents, and static sites (section 5B): FFmpeg's own full help
/// and codec list (preferred to zsh's _ffmpeg, which reads its basic help),
/// ImageMagick and GraphicsMagick, image optimizers, exiftool's single-dash
/// long options, pandoc and yt-dlp (bash handlers), qpdf, tectonic, pdftk,
/// hugo (cobra; its mkcert code starts no `brew`), and mkdocs (click, root
/// commands from trusted help). Nothing writes to HOME.
#[cfg(unix)]
#[test]
#[ignore = "uses the media, document, and static site tools installed on this machine"]
fn installed_media_and_document_tools_repair_typos_without_writing() {
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command;

    let ctx = Context::new(Settings::default(), Shell::Bash, "fuck".into());
    struct Workspace(PathBuf);
    impl Drop for Workspace {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let workspace = Workspace(
        std::env::temp_dir().join(format!("notypo-installed-media-{}", std::process::id())),
    );
    let home = workspace.0.join("home");
    fs::create_dir_all(&home).unwrap();
    let correct = |source: &str| {
        let output = Command::new(env!("CARGO_BIN_EXE_notypo"))
            .current_dir(&workspace.0)
            .env("HOME", &home)
            .env("XDG_CONFIG_HOME", &workspace.0)
            .env("XDG_CACHE_HOME", &workspace.0)
            .env("TF_SHELL", "bash")
            .env(
                "NOTYPO_TRUSTED_COMPLETERS",
                "ffmpeg:convert:gm:yt-dlp:pandoc:pdftk:hugo:mkdocs",
            )
            .env(
                "NOTYPO_TRUSTED_HELP",
                "ffmpeg:ffprobe:ffplay:magick:exiftool:gifsicle:optipng:jpegoptim:tectonic:qpdf:mkdocs",
            )
            .env("NOTYPO_DISABLED_SOURCES", "history:legacy")
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
    let ffmpeg = "-h full and -codecs";
    let ffmpeg_man = "-h full and -codecs and man page";
    let help_man = "--help output and man page";
    // (typo, repair, needs approval, source)
    for (typo, expected, approval, source) in [
        (
            "ffmpeg -hide_baner -i in.mp4 out.mp4",
            "ffmpeg -hide_banner -i in.mp4 out.mp4",
            false,
            ffmpeg_man,
        ),
        (
            "ffmpeg -i in.mp4 -c:v libx26 out.mp4",
            "ffmpeg -i in.mp4 -c:v libx264 out.mp4",
            false,
            ffmpeg,
        ),
        (
            "ffmpeg -i in.mp4 -c:a acc out.mp4",
            "ffmpeg -i in.mp4 -c:a aac out.mp4",
            false,
            ffmpeg,
        ),
        (
            "ffmpeg -loglevle quiet -i in.mp4 out.mp4",
            "ffmpeg -loglevel quiet -i in.mp4 out.mp4",
            false,
            ffmpeg_man,
        ),
        (
            "ffprobe -show_fromat in.mp4",
            "ffprobe -show_format in.mp4",
            false,
            ffmpeg_man,
        ),
        (
            "ffplay -autoexti in.mp4",
            "ffplay -autoexit in.mp4",
            false,
            ffmpeg_man,
        ),
        (
            "magick in.png -resiz 50% out.png",
            "magick in.png -resize 50% out.png",
            false,
            help_man,
        ),
        (
            "convert in.png -qualty 80 out.jpg",
            "convert in.png -quality 80 out.jpg",
            false,
            "zsh completion",
        ),
        (
            "gm conver in.png out.jpg",
            "gm convert in.png out.jpg",
            false,
            "zsh completion",
        ),
        (
            "exiftool -overwrite_orignal in.jpg",
            "exiftool -overwrite_original in.jpg",
            false,
            help_man,
        ),
        (
            "yt-dlp --extract-adio https://example.com/v",
            "yt-dlp --extract-audio https://example.com/v",
            false,
            "bash completion",
        ),
        (
            "gifsicle --optimze=3 in.gif",
            "gifsicle --optimize=3 in.gif",
            false,
            help_man,
        ),
        (
            "optipng -strp all x.png",
            "optipng -strip all x.png",
            false,
            help_man,
        ),
        (
            "jpegoptim --strip-al x.jpg",
            "jpegoptim --strip-all x.jpg",
            false,
            help_man,
        ),
        (
            "cwebp -q 80 -resiz 100 0 in.png -o out.webp",
            "cwebp -q 80 -resize 100 0 in.png -o out.webp",
            false,
            "man page",
        ),
        (
            "pandoc --from markdwn in.md -o out.html",
            "pandoc --from markdown in.md -o out.html",
            false,
            "bash completion",
        ),
        (
            "tectonic --outdr x doc.tex",
            "tectonic --outdir x doc.tex",
            false,
            "--help output",
        ),
        (
            "qpdf --linearise in.pdf out.pdf",
            "qpdf --linearize in.pdf out.pdf",
            false,
            help_man,
        ),
        // Values the handler offers again after any of them are resources.
        (
            "pdftk a.pdf b.pdf cta output out.pdf",
            "pdftk a.pdf b.pdf cat output out.pdf",
            true,
            "zsh completion",
        ),
        ("hugo serer", "hugo server", false, "cobra completion"),
        (
            "hugo new projcet blog",
            "hugo new project blog",
            false,
            "cobra completion",
        ),
        ("mkdocs serv", "mkdocs serve", false, "click completion"),
    ] {
        let app = typo.split(' ').next().unwrap();
        if ctx.which(app).is_none() {
            eprintln!("skipped {app}: not installed (unverified)");
            continue;
        }
        let report = correct(typo);
        let candidate = &report["candidates"][0];
        assert_eq!(candidate["command"], expected, "{typo}: {report}");
        assert!(
            candidate["edits"]
                .as_array()
                .unwrap()
                .iter()
                .all(|edit| edit["via"] == source),
            "{typo}: {report}"
        );
        assert_eq!(
            candidate["safety"]["decision"],
            if approval { "confirm" } else { "allow" },
            "{typo}: {report}"
        );
        eprintln!("{typo} -> {expected}; {} probes", report["probes"]);
    }
    // FFmpeg's codec, format, and stream options are valid as typed.
    for valid in [
        "ffmpeg -i a.mp4 -pix_fmt yuv420p -movflags +faststart b.mp4",
        "ffmpeg -i a.mp4 -c:v libx264 -crf 23 -preset slow -tune film b.mp4",
        "ffmpeg -i a.mp4 -b:v 2M -maxrate 2M -bufsize 1M b.mp4",
    ] {
        if ctx.which("ffmpeg").is_some() {
            let report = correct(valid);
            assert_eq!(
                report["candidates"],
                serde_json::json!([]),
                "{valid}: {report}"
            );
        }
    }
    let written = walk(&home);
    assert!(written.is_empty(), "probes wrote {written:?}");
}

/// Lint, format, and test runners (section 5B), run inside a git project
/// whose JavaScript, Python, and PHP configurations write markers: trusted
/// help reads no configuration and runs no test. husky has no help (any run
/// installs hooks), so probes set HUSKY=0. golangci-lint is cobra; swiftlint
/// has a bash handler; vegeta lists commands as `attack command:` sections.
/// NOTYPO_TEST_LINT_PATH may add npm install directories (jest, vitest,
/// mocha, playwright, stylelint, husky).
#[cfg(unix)]
#[test]
#[ignore = "uses the linters, formatters, and test runners installed on this machine"]
fn installed_linters_and_test_runners_repair_typos_without_running_tests() {
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command;

    struct Workspace(PathBuf);
    impl Drop for Workspace {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let workspace = Workspace(
        std::env::temp_dir().join(format!("notypo-installed-lint-{}", std::process::id())),
    );
    let home = workspace.0.join("home");
    let project = workspace.0.join("project");
    let markers = workspace.0.join("markers");
    for dir in [&home, &project, &markers] {
        fs::create_dir_all(dir).unwrap();
    }
    let js = |name: &str| {
        format!(
            "require('fs').writeFileSync({:?}, 'x'); module.exports = {{}};\n",
            markers.join(name).display().to_string()
        )
    };
    for (file, body) in [
        (".mocharc.js", js("mocha")),
        ("jest.config.js", js("jest")),
        ("vitest.config.js", js("vitest")),
        ("vite.config.js", js("vite")),
        ("playwright.config.js", js("playwright")),
        ("eslint.config.js", js("eslint")),
        ("prettier.config.js", js("prettier")),
        ("stylelint.config.js", js("stylelint")),
        (".markdownlint.js", js("markdownlint")),
        (
            "locustfile.py",
            format!(
                "open({:?}, 'w').write('x')\n",
                markers.join("locust").display().to_string()
            ),
        ),
        (
            "bootstrap.php",
            format!(
                "<?php file_put_contents({:?}, 'x');\n",
                markers.join("phpunit").display().to_string()
            ),
        ),
        (
            "phpunit.xml",
            "<?xml version=\"1.0\"?>\n<phpunit bootstrap=\"bootstrap.php\"></phpunit>\n".into(),
        ),
    ] {
        fs::write(project.join(file), body).unwrap();
    }
    let git = |args: &[&str]| {
        Command::new("git")
            .current_dir(&project)
            .args(args)
            .output()
            .unwrap()
    };
    git(&["init", "-q", "."]);
    let path = match std::env::var("NOTYPO_TEST_LINT_PATH") {
        Ok(extra) => format!("{extra}:{}", std::env::var("PATH").unwrap_or_default()),
        Err(_) => std::env::var("PATH").unwrap_or_default(),
    };
    let which = |app: &str| {
        use std::os::unix::fs::PermissionsExt;
        std::env::split_paths(&path).any(|dir| {
            fs::metadata(dir.join(app))
                .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
    };
    let help = "shellcheck:shfmt:hadolint:yamllint:markdownlint:staticcheck:mypy:pyright:ktlint:wrk:vegeta:biome:oxlint:prettier:eslint:phpunit:jest:vitest:mocha:playwright:stylelint:husky:locust";
    let correct = |source: &str| {
        let output = Command::new(env!("CARGO_BIN_EXE_notypo"))
            .current_dir(&project)
            .env("PATH", &path)
            .env("HOME", &home)
            .env("XDG_CONFIG_HOME", &workspace.0)
            .env("XDG_CACHE_HOME", &workspace.0)
            .env("TF_SHELL", "bash")
            .env("NOTYPO_TRUSTED_COMPLETERS", "golangci-lint:swiftlint")
            .env("NOTYPO_TRUSTED_HELP", help)
            .env("NOTYPO_DISABLED_SOURCES", "history:legacy")
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
    let help_man = "--help output and man page";
    let help_only = "--help output";
    // (typo, repair, needs approval, source)
    for (typo, expected, approval, source) in [
        (
            "shellcheck --severty=warning x.sh",
            "shellcheck --severity=warning x.sh",
            false,
            help_man,
        ),
        (
            "shfmt -w -i 2 --simplfy x.sh",
            "shfmt -w -i 2 --simplify x.sh",
            false,
            help_man,
        ),
        (
            "hadolint --ignroe DL3008 Dockerfile",
            "hadolint --ignore DL3008 Dockerfile",
            false,
            help_only,
        ),
        (
            "yamllint --strcit x.yaml",
            "yamllint --strict x.yaml",
            false,
            help_only,
        ),
        (
            "markdownlint --fx README.md",
            "markdownlint --fix README.md",
            false,
            help_only,
        ),
        (
            "golangci-lint rnu ./...",
            "golangci-lint run ./...",
            false,
            "cobra completion",
        ),
        (
            "staticcheck -chekcs all ./...",
            "staticcheck -checks all ./...",
            false,
            help_only,
        ),
        ("mypy --strcit x.py", "mypy --strict x.py", false, help_only),
        (
            "pyright --outputjsno x.py",
            "pyright --outputjson x.py",
            false,
            help_only,
        ),
        ("ktlint --formt", "ktlint --format", false, help_only),
        (
            "wrk -c 10 -d 5s --latancy http://localhost",
            "wrk -c 10 -d 5s --latency http://localhost",
            false,
            help_only,
        ),
        (
            "vegeta atack -rate 10",
            "vegeta attack -rate 10",
            false,
            help_only,
        ),
        ("biome chekc .", "biome check .", false, help_only),
        ("oxlint --fx .", "oxlint --fix .", false, help_only),
        ("prettier --wirte .", "prettier --write .", false, help_only),
        ("eslint --fxi .", "eslint --fix .", false, help_only),
        (
            "phpunit --filtre Foo",
            "phpunit --filter Foo",
            false,
            help_only,
        ),
        ("jest --watchh", "jest --watch", false, help_only),
        ("vitest rnu", "vitest run", false, help_only),
        // The generic gate asks about any --recursive.
        ("mocha --recursiv", "mocha --recursive", true, help_only),
        ("playwright tset", "playwright test", false, help_only),
        (
            "stylelint --fxi x.css",
            "stylelint --fix x.css",
            false,
            help_only,
        ),
        ("swiftlint lnit", "swiftlint lint", false, "bash completion"),
    ] {
        let app = typo.split(' ').next().unwrap();
        if !which(app) {
            eprintln!("skipped {app}: not installed (unverified)");
            continue;
        }
        let report = correct(typo);
        let candidate = &report["candidates"][0];
        assert_eq!(candidate["command"], expected, "{typo}: {report}");
        assert!(
            candidate["edits"]
                .as_array()
                .unwrap()
                .iter()
                .all(|edit| edit["via"] == source),
            "{typo}: {report}"
        );
        assert_eq!(
            candidate["safety"]["decision"],
            if approval { "confirm" } else { "allow" },
            "{typo}: {report}"
        );
        eprintln!("{typo} -> {expected}; {} probes", report["probes"]);
    }
    if which("husky") {
        // husky prints nothing to learn from; it must not install hooks.
        let report = correct("husky ini");
        assert_eq!(report["candidates"], serde_json::json!([]), "{report}");
        assert!(!project.join("--help").exists());
        assert!(!project.join(".husky").exists());
        let hooks = git(&["config", "--get", "core.hooksPath"]);
        assert!(hooks.stdout.is_empty(), "core.hooksPath was set");
    }
    let ran: Vec<_> = fs::read_dir(&markers)
        .unwrap()
        .flatten()
        .map(|entry| entry.file_name())
        .collect();
    assert!(ran.is_empty(), "project code ran: {ran:?}");
    // biome and golangci-lint create empty cache directories at startup.
    let files: Vec<_> = walk(&home).into_iter().filter(|p| p.is_file()).collect();
    assert!(files.is_empty(), "probes wrote {files:?}");
}

/// Network diagnostics, firewalls, and tunnels (section 5B, macOS): zsh's
/// handlers (iproute2mac's ip, route, tcpdump, pfctl, networksetup, scutil,
/// whois), nmap's man page, tshark's help, and ngrok (cobra). A changed
/// host, address, or server needs approval, as do network configuration
/// changes; a server far from a handler's few listed ones is left alone.
#[cfg(unix)]
#[test]
#[ignore = "uses the network diagnostic tools installed on this machine"]
fn installed_network_diagnostics_repair_typos_and_ask_before_changing_targets() {
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command;

    let ctx = Context::new(Settings::default(), Shell::Bash, "fuck".into());
    struct Workspace(PathBuf);
    impl Drop for Workspace {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let workspace = Workspace(
        std::env::temp_dir().join(format!("notypo-installed-netdiag-{}", std::process::id())),
    );
    let home = workspace.0.join("home");
    fs::create_dir_all(&home).unwrap();
    let correct = |source: &str| {
        let output = Command::new(env!("CARGO_BIN_EXE_notypo"))
            .current_dir(&workspace.0)
            .env("HOME", &home)
            .env("XDG_CONFIG_HOME", &workspace.0)
            .env("XDG_CACHE_HOME", &workspace.0)
            .env("TF_SHELL", "bash")
            .env(
                "NOTYPO_TRUSTED_COMPLETERS",
                "ip:route:whois:nmap:tcpdump:pfctl:networksetup:scutil:traceroute:go.ngrok.com/cmd/ngrok",
            )
            .env("NOTYPO_TRUSTED_HELP", "tshark")
            .env("NOTYPO_DISABLED_SOURCES", "history:legacy")
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
    let zsh = "zsh completion";
    // (typo, repair, needs approval, source)
    for (typo, expected, approval, source) in [
        ("ip addr shwo", "ip addr show", false, zsh),
        ("ip lnik", "ip link", false, zsh),
        ("route -n gte default", "route -n get default", false, zsh),
        (
            "whois -h whois.ripe.ent example.com",
            "whois -h whois.ripe.net example.com",
            true,
            zsh,
        ),
        (
            "nmap -sV --top-port 100 host",
            "nmap -sV --top-ports 100 host",
            false,
            "man page",
        ),
        (
            "tcpdump -i en0 -nn --immediate-mod",
            "tcpdump -i en0 -nn --immediate-mode",
            false,
            zsh,
        ),
        (
            "tshark -i en0 --colr",
            "tshark -i en0 --color",
            false,
            "--help output",
        ),
        ("pfctl -s rulse", "pfctl -s rules", false, zsh),
        (
            "networksetup -getdnsserver Wi-Fi",
            "networksetup -getdnsservers Wi-Fi",
            false,
            zsh,
        ),
        (
            "networksetup -setdnsserver Wi-Fi 1.1.1.1",
            "networksetup -setdnsservers Wi-Fi 1.1.1.1",
            true,
            zsh,
        ),
        ("scutil --dsn", "scutil --dns", false, zsh),
        (
            "ngrok htp 8080",
            "ngrok http 8080",
            false,
            "cobra completion",
        ),
    ] {
        let app = typo.split(' ').next().unwrap();
        if ctx.which(app).is_none() {
            eprintln!("skipped {app}: not installed (unverified)");
            continue;
        }
        let report = correct(typo);
        let candidate = &report["candidates"][0];
        assert_eq!(candidate["command"], expected, "{typo}: {report}");
        assert!(
            candidate["edits"]
                .as_array()
                .unwrap()
                .iter()
                .all(|edit| edit["via"] == source),
            "{typo}: {report}"
        );
        assert_eq!(
            candidate["safety"]["decision"],
            if approval { "confirm" } else { "allow" },
            "{typo}: {report}"
        );
        eprintln!("{typo} -> {expected}; {} probes", report["probes"]);
    }
    // Hosts and servers are not vocabulary: zsh's _whois lists a few
    // servers, and traceroute's handler lists known hosts.
    for valid in [
        "whois -h whois.example.com example.com",
        "traceroute -n exmaple.com",
    ] {
        if ctx.which(valid.split(' ').next().unwrap()).is_some() {
            let report = correct(valid);
            assert_eq!(
                report["candidates"],
                serde_json::json!([]),
                "{valid}: {report}"
            );
        }
    }
    let written = walk(&home);
    assert!(written.is_empty(), "probes wrote {written:?}");
}

/// Compilers, linkers, and debuggers (section 5B): clang's own
/// `--autocomplete` protocol under any name it answers to (macOS's cc and
/// gcc), its `-std=`/`-stdlib=` values, GNU gcc's help and man page, zsh
/// handlers (pkg-config, lldb), meson's bash handler, and trusted help or
/// man pages for the rest. Joined values (`-Iinclude`, `-lm`, `-DFOO=1`)
/// and lists clang does not enumerate (`-fsanitize=`, `-march=`) stay.
#[cfg(unix)]
#[test]
#[ignore = "uses the compilers and binary tools installed on this machine"]
fn installed_compilers_repair_flags_and_values_without_compiling() {
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command;

    let ctx = Context::new(Settings::default(), Shell::Bash, "fuck".into());
    struct Workspace(PathBuf);
    impl Drop for Workspace {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let workspace =
        Workspace(std::env::temp_dir().join(format!("notypo-installed-cc-{}", std::process::id())));
    let home = workspace.0.join("home");
    fs::create_dir_all(&home).unwrap();
    let correct = |source: &str| {
        let output = Command::new(env!("CARGO_BIN_EXE_notypo"))
            .current_dir(&workspace.0)
            .env("HOME", &home)
            .env("XDG_CONFIG_HOME", &workspace.0)
            .env("XDG_CACHE_HOME", &workspace.0)
            .env("TF_SHELL", "bash")
            .env("NOTYPO_TRUSTED_COMPLETERS", "pkg-config:lldb:meson")
            .env(
                "NOTYPO_TRUSTED_HELP",
                "gcc-16:gcc-15:gcc-14:swiftc:tsc:codesign:lipo:cmake:install_name_tool",
            )
            .env("NOTYPO_DISABLED_SOURCES", "history:legacy")
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
    let clang = "clang completion";
    let help_man = "--help output and man page";
    let mut cases = vec![
        ("clang -Wal -o x x.c", "clang -Wall -o x x.c", clang),
        ("clang --versoin", "clang --version", clang),
        ("clang -std=c++2O x.cpp", "clang -std=c++20 x.cpp", clang),
        (
            "clang++ -stdlib=libc+ x.cpp",
            "clang++ -stdlib=libc++ x.cpp",
            clang,
        ),
        ("cc -Wextr -O2 x.c", "cc -Wextra -O2 x.c", clang),
        (
            "swiftc -emit-executabel x.swift",
            "swiftc -emit-executable x.swift",
            "--help output",
        ),
        ("tsc --stirct x.ts", "tsc --strict x.ts", "--help output"),
        ("meson setp build", "meson setup build", "bash completion"),
        (
            "pkg-config --cflgas zlib",
            "pkg-config --cflags zlib",
            "zsh completion",
        ),
        (
            "lldb --batc -o run ./x",
            "lldb --batch -o run ./x",
            "zsh completion",
        ),
        ("codesign --sing - x", "codesign --sign - x", help_man),
        ("lipo -inf x", "lipo -info x", help_man),
        (
            "cmake --biuld build",
            "cmake --build build",
            "--help output",
        ),
        (
            "install_name_tool -add_rpth /x x",
            "install_name_tool -add_rpath /x x",
            "man page",
        ),
    ];
    // Homebrew's GNU gcc goes by its major version.
    let gnu = ["gcc-16", "gcc-15", "gcc-14"]
        .into_iter()
        .find(|gcc| ctx.which(gcc).is_some());
    let gnu_typo = gnu.map(|gcc| (format!("{gcc} -Wextr x.c"), format!("{gcc} -Wextra x.c")));
    if let Some((typo, expected)) = &gnu_typo {
        cases.push((typo, expected, help_man));
    }
    for (typo, expected, source) in cases {
        let app = typo.split(' ').next().unwrap();
        if ctx.which(app).is_none() {
            eprintln!("skipped {app}: not installed (unverified)");
            continue;
        }
        let report = correct(typo);
        let candidate = &report["candidates"][0];
        assert_eq!(candidate["command"], expected, "{typo}: {report}");
        assert!(
            candidate["edits"]
                .as_array()
                .unwrap()
                .iter()
                .all(|edit| edit["via"] == source),
            "{typo}: {report}"
        );
        assert_eq!(candidate["safety"]["decision"], "allow", "{typo}: {report}");
        eprintln!("{typo} -> {expected}; {} probes", report["probes"]);
    }
    if ctx.which("clang").is_some() {
        for valid in [
            "clang -O2 -g -Wall -Werror=format -I/usr/include -L/usr/lib -lm -o out x.c",
            "clang -Iinclude -DFOO=1 -c x.c -o x.o",
            "clang -fsanitize=address,undefined -march=native x.c",
        ] {
            let report = correct(valid);
            assert_eq!(
                report["candidates"],
                serde_json::json!([]),
                "{valid}: {report}"
            );
        }
    }
    // Nothing was compiled or linked.
    for output in ["x", "out", "x.o", "a.out"] {
        assert!(!workspace.0.join(output).exists(), "{output}");
    }
    let written = walk(&home);
    assert!(written.is_empty(), "probes wrote {written:?}");
}

/// Editors and IDE launchers (section 5B): zsh's handlers (vim, nvim,
/// VS Code's `code`, macOS's open), trusted help and man pages (emacs,
/// emacsclient, micro, helix). Probes have no terminal, so no editor opens;
/// Neovim's log goes to the null device. Extension IDs are marketplace
/// resources and are never checked; installing one needs approval.
#[cfg(unix)]
#[test]
#[ignore = "uses the editors installed on this machine"]
fn installed_editors_repair_options_without_opening() {
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command;

    struct Workspace(PathBuf);
    impl Drop for Workspace {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let workspace = Workspace(
        std::env::temp_dir().join(format!("notypo-installed-editors-{}", std::process::id())),
    );
    let home = workspace.0.join("home");
    fs::create_dir_all(&home).unwrap();
    // VS Code keeps its `code` launcher inside the application bundle.
    let vscode = PathBuf::from("/Applications/Visual Studio Code.app/Contents/Resources/app/bin");
    let path = match vscode.is_dir() {
        true => format!(
            "{}:{}",
            vscode.display(),
            std::env::var("PATH").unwrap_or_default()
        ),
        false => std::env::var("PATH").unwrap_or_default(),
    };
    let which = |app: &str| std::env::split_paths(&path).any(|dir| dir.join(app).is_file());
    let correct = |source: &str| {
        let output = Command::new(env!("CARGO_BIN_EXE_notypo"))
            .current_dir(&workspace.0)
            .env("PATH", &path)
            .env("HOME", &home)
            .env("XDG_CONFIG_HOME", &workspace.0)
            .env("XDG_CACHE_HOME", &workspace.0)
            .env("TF_SHELL", "bash")
            .env("NOTYPO_TRUSTED_COMPLETERS", "vim:nvim:code:open")
            .env(
                "NOTYPO_TRUSTED_HELP",
                "nvim:emacs:emacsclient:micro:hx:code",
            )
            .env("NOTYPO_DISABLED_SOURCES", "history:legacy")
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
    let zsh = "zsh completion";
    let help_man = "--help output and man page";
    // (typo, repair, needs approval, source)
    for (typo, expected, approval, source) in [
        ("vim --cleen x.txt", "vim --clean x.txt", false, zsh),
        ("nvim --headles -c q", "nvim --headless -c q", false, zsh),
        (
            "emacs --batc -l x.el",
            "emacs --batch -l x.el",
            false,
            help_man,
        ),
        (
            "emacsclient --no-wiat x",
            "emacsclient --no-wait x",
            false,
            help_man,
        ),
        (
            "micro -confg-dir x",
            "micro -config-dir x",
            false,
            "man page",
        ),
        ("hx --helth", "hx --health", false, "--help output"),
        ("code --new-windw .", "code --new-window .", false, zsh),
        (
            "code --install-extention ms-python.python",
            "code --install-extension ms-python.python",
            true,
            zsh,
        ),
        (
            "open -a Safarii x.html",
            "open -a Safari x.html",
            false,
            zsh,
        ),
    ] {
        let app = typo.split(' ').next().unwrap();
        if !which(app) {
            eprintln!("skipped {app}: not installed (unverified)");
            continue;
        }
        let report = correct(typo);
        let candidate = &report["candidates"][0];
        assert_eq!(candidate["command"], expected, "{typo}: {report}");
        assert!(
            candidate["edits"]
                .as_array()
                .unwrap()
                .iter()
                .all(|edit| edit["via"] == source),
            "{typo}: {report}"
        );
        assert_eq!(
            candidate["safety"]["decision"],
            if approval { "confirm" } else { "allow" },
            "{typo}: {report}"
        );
        eprintln!("{typo} -> {expected}; {} probes", report["probes"]);
    }
    for valid in [
        "vim -c 'set nu' +10 x.txt",
        "code --install-extension ms-python.pyhton",
    ] {
        if which(valid.split(' ').next().unwrap()) {
            let report = correct(valid);
            assert_eq!(
                report["candidates"],
                serde_json::json!([]),
                "{valid}: {report}"
            );
        }
    }
    let written = walk(&home);
    assert!(written.is_empty(), "probes wrote {written:?}");
}

/// System administration on macOS (section 5B): diskutil's verbs from its
/// bare usage (never `diskutil <verb>`), launchctl's from `launchctl help`
/// (never `<subcommand> --help`), hdiutil (zsh), tmutil, sysadminctl, and
/// systemsetup (help or man page). Erasing disks, removing users or
/// services, and changing system settings need approval.
#[cfg(target_os = "macos")]
#[test]
#[ignore = "uses macOS's administration tools"]
fn installed_macos_admin_tools_repair_verbs_and_ask_before_changes() {
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command;

    struct Workspace(PathBuf);
    impl Drop for Workspace {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let workspace = Workspace(
        std::env::temp_dir().join(format!("notypo-installed-admin-{}", std::process::id())),
    );
    let home = workspace.0.join("home");
    fs::create_dir_all(&home).unwrap();
    let correct = |source: &str| {
        let output = Command::new(env!("CARGO_BIN_EXE_notypo"))
            .current_dir(&workspace.0)
            .env("HOME", &home)
            .env("XDG_CONFIG_HOME", &workspace.0)
            .env("XDG_CACHE_HOME", &workspace.0)
            .env("TF_SHELL", "bash")
            .env("NOTYPO_TRUSTED_COMPLETERS", "hdiutil")
            .env(
                "NOTYPO_TRUSTED_HELP",
                "diskutil:launchctl:tmutil:sysadminctl:systemsetup:dscl",
            )
            .env("NOTYPO_DISABLED_SOURCES", "history:legacy")
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
    let help = "--help output";
    // (typo, repair, needs approval, source)
    for (typo, expected, approval, source) in [
        ("diskutil lsit", "diskutil list", false, help),
        (
            "diskutil informaton disk0",
            "diskutil information disk0",
            false,
            help,
        ),
        (
            "diskutil unmoutDisk disk4",
            "diskutil unmountDisk disk4",
            false,
            help,
        ),
        (
            "diskutil eraseDsk APFS Data disk9",
            "diskutil eraseDisk APFS Data disk9",
            true,
            help,
        ),
        ("launchctl lsit", "launchctl list", false, help),
        (
            "launchctl bootot gui/501/example",
            "launchctl bootout gui/501/example",
            true,
            help,
        ),
        (
            "launchctl kickstrat -k gui/501/example",
            "launchctl kickstart -k gui/501/example",
            true,
            help,
        ),
        (
            "hdiutil attch x.dmg",
            "hdiutil attach x.dmg",
            false,
            "zsh completion",
        ),
        ("tmutil lisbackups", "tmutil listbackups", false, help),
        (
            "tmutil delte -d /Volumes/B -t 2026-01-01",
            "tmutil delete -d /Volumes/B -t 2026-01-01",
            true,
            help,
        ),
        (
            "sysadminctl -deleteUsr example",
            "sysadminctl -deleteUser example",
            true,
            help,
        ),
        (
            "dscl . -delte /Users/example",
            "dscl . -delete /Users/example",
            true,
            "man page",
        ),
        (
            "systemsetup -settimezne Europe/Berlin",
            "systemsetup -settimezone Europe/Berlin",
            true,
            "man page",
        ),
    ] {
        let report = correct(typo);
        let candidate = &report["candidates"][0];
        assert_eq!(candidate["command"], expected, "{typo}: {report}");
        assert!(
            candidate["edits"]
                .as_array()
                .unwrap()
                .iter()
                .all(|edit| edit["via"] == source),
            "{typo}: {report}"
        );
        assert_eq!(
            candidate["safety"]["decision"],
            if approval { "confirm" } else { "allow" },
            "{typo}: {report}"
        );
        eprintln!("{typo} -> {expected}; {} probes", report["probes"]);
    }
    for valid in ["diskutil list", "launchctl print gui/501"] {
        let report = correct(valid);
        assert_eq!(
            report["candidates"],
            serde_json::json!([]),
            "{valid}: {report}"
        );
    }
    let written = walk(&home);
    assert!(written.is_empty(), "probes wrote {written:?}");
}

/// Mobile and device tools (section 5B, macOS): adb through zsh's _adb with
/// adb itself stubbed (its `adb devices -l` starts adb's server and writes
/// a key pair), Xcode's xcodebuild, xcrun, and xcode-select, xcodes and
/// carthage (bash handlers), CocoaPods' CLAide help, fastlane's commander
/// help, and libimobiledevice. Device installs, toolchain switches, and
/// dependency installs need approval.
#[cfg(target_os = "macos")]
#[test]
#[ignore = "uses the mobile and device tools installed on this machine"]
fn installed_mobile_tools_repair_typos_without_touching_devices() {
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command;

    let ctx = Context::new(Settings::default(), Shell::Bash, "fuck".into());
    struct Workspace(PathBuf);
    impl Drop for Workspace {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let workspace = Workspace(
        std::env::temp_dir().join(format!("notypo-installed-mobile-{}", std::process::id())),
    );
    let home = workspace.0.join("home");
    fs::create_dir_all(&home).unwrap();
    let adb_server = || {
        Command::new("pgrep")
            .args(["-f", "adb.*fork-server"])
            .output()
            .is_ok_and(|output| output.status.success())
    };
    let server_before = adb_server();
    let correct = |source: &str| {
        let output = Command::new(env!("CARGO_BIN_EXE_notypo"))
            .current_dir(&workspace.0)
            .env("HOME", &home)
            .env("XDG_CONFIG_HOME", &workspace.0)
            .env("XDG_CACHE_HOME", &workspace.0)
            .env("TF_SHELL", "bash")
            .env(
                "NOTYPO_TRUSTED_COMPLETERS",
                "adb:xcode-select:xcodes:carthage",
            )
            .env(
                "NOTYPO_TRUSTED_HELP",
                "xcodebuild:xcrun:pod:fastlane:ideviceinfo",
            )
            .env("NOTYPO_DISABLED_SOURCES", "history:legacy")
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
    let zsh = "zsh completion";
    let bash = "bash completion";
    let help = "--help output";
    let help_man = "--help output and man page";
    // (typo, repair, needs approval, source)
    for (typo, expected, approval, source) in [
        ("adb devcies", "adb devices", false, zsh),
        ("adb instal app.apk", "adb install app.apk", true, zsh),
        (
            "adb -s emulator-5554 shel ls",
            "adb -s emulator-5554 shell ls",
            false,
            zsh,
        ),
        (
            "xcodebuild -sheme App build",
            "xcodebuild -scheme App build",
            false,
            help_man,
        ),
        (
            "xcrun --sdk iphoneos --show-sdk-pth",
            "xcrun --sdk iphoneos --show-sdk-path",
            false,
            help_man,
        ),
        (
            "xcode-select --print-pth",
            "xcode-select --print-path",
            false,
            zsh,
        ),
        (
            "xcode-select --swtich /Applications/Xcode.app",
            "xcode-select --switch /Applications/Xcode.app",
            true,
            zsh,
        ),
        ("xcodes lsit", "xcodes list", false, bash),
        ("xcodes instal 16.0", "xcodes install 16.0", true, bash),
        ("carthage bootsrap", "carthage bootstrap", true, bash),
        ("pod instal", "pod install", true, help),
        ("pod repo updte", "pod repo update", true, help),
        ("fastlane lanse", "fastlane lanes", false, help),
        (
            "ideviceinfo --simpel",
            "ideviceinfo --simple",
            false,
            help_man,
        ),
    ] {
        let app = typo.split(' ').next().unwrap();
        if ctx.which(app).is_none() {
            eprintln!("skipped {app}: not installed (unverified)");
            continue;
        }
        let report = correct(typo);
        let candidate = &report["candidates"][0];
        assert_eq!(candidate["command"], expected, "{typo}: {report}");
        assert!(
            candidate["edits"]
                .as_array()
                .unwrap()
                .iter()
                .all(|edit| edit["via"] == source),
            "{typo}: {report}"
        );
        assert_eq!(
            candidate["safety"]["decision"],
            if approval { "confirm" } else { "allow" },
            "{typo}: {report}"
        );
        eprintln!("{typo} -> {expected}; {} probes", report["probes"]);
    }
    assert!(
        server_before || !adb_server(),
        "a probe started adb's server"
    );
    // adb --help leaves an empty ~/.android; nothing else is written.
    let files: Vec<_> = walk(&home).into_iter().filter(|p| p.is_file()).collect();
    assert!(files.is_empty(), "probes wrote {files:?}");
}

/// BSD, GNU, and uutils tools must use the installed implementation's own
/// documentation. All commands are inspected, never executed; destructive
/// and changed write targets still need approval.
#[cfg(target_os = "macos")]
#[test]
#[ignore = "uses macOS's BSD tools and installed GNU/uutils coreutils"]
fn installed_core_shell_implementations_use_their_own_options_without_effects() {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::process::{Command, Stdio};

    struct Workspace(PathBuf);
    impl Drop for Workspace {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let workspace = Workspace(std::env::temp_dir().join(format!(
        "notypo-installed-core-shell-{}",
        std::process::id()
    )));
    let input = workspace.0.join("input");
    fs::create_dir_all(&input).unwrap();
    let content = input.join("content.txt");
    fs::write(&content, "fixture\n").unwrap();
    let permissions = fs::metadata(&content).unwrap().permissions().mode();
    let package_bin = |entry: &str, directory: &str| {
        fs::canonicalize(notypo::utils::which(entry)?)
            .ok()?
            .parent()?
            .parent()
            .map(|root| root.join(directory))
    };
    let gnu = std::env::var_os("NOTYPO_TEST_GNU_COREUTILS")
        .map(PathBuf::from)
        .or_else(|| package_bin("gls", "libexec/gnubin"));
    let uutils = std::env::var_os("NOTYPO_TEST_UUTILS_COREUTILS")
        .map(PathBuf::from)
        .or_else(|| package_bin("uu-ls", "libexec/uubin"));
    let process = |prefix: &str, source: &str, help: bool| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_notypo"));
        command
            .current_dir(&workspace.0)
            .env(
                "PATH",
                format!(
                    "{prefix}:/usr/bin:/bin:/usr/sbin:/sbin:{}",
                    std::env::var("PATH").unwrap_or_default()
                ),
            )
            .env("XDG_CONFIG_HOME", workspace.0.join("config"))
            .env("XDG_CACHE_HOME", workspace.0.join("cache"))
            .env("TF_SHELL", "bash")
            .env("NOTYPO_TRUSTED_COMPLETERS", "[]")
            .env("NOTYPO_TRUSTED_HELP", if help { "*" } else { "[]" })
            .env("NOTYPO_DISABLED_SOURCES", "native:history:legacy")
            .env("NOTYPO_REPLAY_FOR_DIAGNOSIS", "false")
            .env_remove("TF_SHELL_ALIASES")
            .env_remove("TF_HISTORY")
            .env_remove("NOTYPO_CURRENT_COMMAND")
            .env_remove("NOTYPO_SHELL_FUNCTIONS")
            .env_remove("NOTYPO_EXIT_STATUS")
            .env_remove("NOTYPO_PIPESTATUS")
            .env_remove("NOTYPO_CAPTURED_OUTPUT")
            .env_remove("NOTYPO_OUTPUT_LOG")
            .stdin(Stdio::null())
            .args(["--force-command", source]);
        command
    };
    let correct = |prefix: &str, source: &str, help: bool| {
        let output = process(prefix, source, help)
            .arg("--json")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{source}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap()
    };
    let cases = [
        (
            r#"cp"#,
            r#"--presreve=mode ./input ./output"#,
            r#"--preserve=mode ./input ./output"#,
        ),
        (
            r#"mv"#,
            r#"--no-clobebr ./input ./output"#,
            r#"--no-clobber ./input ./output"#,
        ),
        (
            r#"rm"#,
            r#"--interative=always ./input"#,
            r#"--interactive=always ./input"#,
        ),
        (r#"mkdir"#, r#"--parnets ./output"#, r#"--parents ./output"#),
        (
            r#"rmdir"#,
            r#"--ignore-fail-on-non-emty ./input"#,
            r#"--ignore-fail-on-non-empty ./input"#,
        ),
        (
            r#"chmod"#,
            r#"--refernce=./input ./output"#,
            r#"--reference=./input ./output"#,
        ),
        (
            r#"chown"#,
            r#"--refernce=./input ./output"#,
            r#"--reference=./input ./output"#,
        ),
        (
            r#"chgrp"#,
            r#"--refernce=./input ./output"#,
            r#"--reference=./input ./output"#,
        ),
        (
            r#"ln"#,
            r#"--symoblic ./input ./output"#,
            r#"--symbolic ./input ./output"#,
        ),
        (
            r#"touch"#,
            r#"--no-cretae ./output"#,
            r#"--no-create ./output"#,
        ),
        (
            r#"cat"#,
            r#"--show-edns ./input/content.txt"#,
            r#"--show-ends ./input/content.txt"#,
        ),
        (
            r#"head"#,
            r#"--liens=10 ./input/content.txt"#,
            r#"--lines=10 ./input/content.txt"#,
        ),
        (
            r#"tail"#,
            r#"--bytse=10 ./input/content.txt"#,
            r#"--bytes=10 ./input/content.txt"#,
        ),
        (
            r#"sort"#,
            r#"--human-numeric-srot ./input/content.txt"#,
            r#"--human-numeric-sort ./input/content.txt"#,
        ),
        (
            r#"uniq"#,
            r#"--cuont ./input/content.txt"#,
            r#"--count ./input/content.txt"#,
        ),
        (
            r#"wc"#,
            r#"--bytse ./input/content.txt"#,
            r#"--bytes ./input/content.txt"#,
        ),
        (
            r#"cut"#,
            r#"--delimter=, -f 1 ./input/content.txt"#,
            r#"--delimiter=, -f 1 ./input/content.txt"#,
        ),
        (r#"tr"#, r#"--squeze-repeats a"#, r#"--squeeze-repeats a"#),
        (
            r#"paste"#,
            r#"--delimters=, ./input/content.txt"#,
            r#"--delimiters=, ./input/content.txt"#,
        ),
        (
            r#"join"#,
            r#"--ignore-csae ./input/content.txt ./input/content.txt"#,
            r#"--ignore-case ./input/content.txt ./input/content.txt"#,
        ),
        (r#"tee"#, r#"--appedn ./output"#, r#"--append ./output"#),
        (
            r#"comm"#,
            r#"--toatl ./input/content.txt ./input/content.txt"#,
            r#"--total ./input/content.txt ./input/content.txt"#,
        ),
        (
            r#"split"#,
            r#"--liens=10 ./input/content.txt ./output"#,
            r#"--lines=10 ./input/content.txt ./output"#,
        ),
        (
            r#"stat"#,
            r#"--pritnf='%s' ./input"#,
            r#"--printf='%s' ./input"#,
        ),
        (
            r#"readlink"#,
            r#"--canonicalzie ./input"#,
            r#"--canonicalize ./input"#,
        ),
        (
            r#"realpath"#,
            r#"--canonicalize-missng ./input"#,
            r#"--canonicalize-missing ./input"#,
        ),
        (r#"date"#, r#"--daet=2026-10-07"#, r#"--date=2026-10-07"#),
        (
            r#"df"#,
            r#"--human-readble ./input"#,
            r#"--human-readable ./input"#,
        ),
        (
            r#"du"#,
            r#"--human-readble ./input"#,
            r#"--human-readable ./input"#,
        ),
        (r#"uname"#, r#"--machien"#, r#"--machine"#),
        (r#"id"#, r#"--gruops"#, r#"--groups"#),
        (r#"printenv"#, r#"--nul"#, r#"--null"#),
        (
            r#"md5sum"#,
            r#"--chekc ./input/content.txt"#,
            r#"--check ./input/content.txt"#,
        ),
        (
            r#"sha256sum"#,
            r#"--chekc ./input/content.txt"#,
            r#"--check ./input/content.txt"#,
        ),
        (
            r#"base64"#,
            r#"--deocde ./input/content.txt"#,
            r#"--decode ./input/content.txt"#,
        ),
        (
            r#"nl"#,
            r#"--number-fomrat=rz ./input/content.txt"#,
            r#"--number-format=rz ./input/content.txt"#,
        ),
        (r#"seq"#, r#"--separtor=, 3"#, r#"--separator=, 3"#),
    ];
    for (implementation, prefix) in [("GNU", gnu), ("uutils", uutils)] {
        let Some(prefix) = prefix.filter(|path| path.is_dir()) else {
            eprintln!("skipped core shell: {implementation} tools are not installed (unverified)");
            continue;
        };
        let prefix = prefix.to_str().unwrap();
        for (program, typo, expected) in cases {
            assert!(
                PathBuf::from(prefix).join(program).is_file(),
                "{implementation}: missing {program}"
            );
            let source = format!("{program} {typo}");
            let expected = format!("{program} {expected}");
            let report = correct(prefix, &source, true);
            assert_eq!(
                report["candidates"][0]["command"], expected,
                "{implementation}: {source}: {report}"
            );
            if matches!(program, "rm" | "rmdir") {
                assert_eq!(
                    report["candidates"][0]["safety"]["decision"], "confirm",
                    "{report}"
                );
            }
            eprintln!("{implementation}: {source} -> {expected}");
        }
        // With no program execution authorized, each implementation's own
        // installed page alone must still repair its options.
        for (source, expected) in [
            ("ls --sortt=size", "ls --sort=size"),
            (
                "cp --recursve ./input ./output",
                "cp --recursive ./input ./output",
            ),
            ("date --daet=2026-10-07", "date --date=2026-10-07"),
        ] {
            let report = correct(prefix, source, false);
            assert_eq!(
                report["candidates"][0]["command"], expected,
                "{implementation}: {report}"
            );
            assert!(
                report["candidates"][0]["edits"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|edit| edit["via"] == "man page"),
                "{report}"
            );
        }
        for source in [
            "cat --show-ends ./input/content.txt",
            "cp -R ./input ./output",
            "rm -r ./input",
        ] {
            assert_eq!(
                correct(prefix, source, true)["candidates"],
                serde_json::json!([]),
                "{source}"
            );
        }
    }
    // macOS's BSD implementations don't acquire GNU-only flags from the
    // alternate versions installed elsewhere on this machine.
    for source in [
        "ls --sortt=size",
        "cp --recursve ./input ./output",
        "date --daet=2026-10-07",
        "stat --pritnf='%s' ./input",
    ] {
        let report = correct("/usr/bin:/bin:/usr/sbin:/sbin", source, true);
        assert_eq!(
            report["candidates"],
            serde_json::json!([]),
            "{source}: {report}"
        );
    }
    for (source, expected) in [
        (
            "tail --bytse=10 ./input/content.txt",
            "tail --bytes=10 ./input/content.txt",
        ),
        (
            "xargs --no-run-if-emty echo",
            "xargs --no-run-if-empty echo",
        ),
        (
            "uu-kill --singal TERM 99999999",
            "uu-kill --signal TERM 99999999",
        ),
        ("grm --verbsoe ./input", "grm --verbose ./input"),
        ("uu-rm --verbsoe ./input", "uu-rm --verbose ./input"),
        (
            "uu-coreutils cp --recursve ./input ./output",
            "uu-coreutils cp --recursive ./input ./output",
        ),
        (
            "uu-coreutils rm --verbsoe ./input",
            "uu-coreutils rm --verbose ./input",
        ),
    ] {
        if notypo::utils::which(source.split_whitespace().next().unwrap()).is_none() {
            eprintln!("skipped core shell: {source} is not installed (unverified)");
            continue;
        }
        let report = correct("/usr/bin:/bin:/usr/sbin:/sbin", source, true);
        assert_eq!(
            report["candidates"][0]["command"], expected,
            "{source}: {report}"
        );
        if source.starts_with("uu-") || source.starts_with("grm") {
            assert_eq!(
                report["candidates"][0]["safety"]["decision"], "confirm",
                "{report}"
            );
            let output = process("/usr/bin:/bin:/usr/sbin:/sbin", source, true)
                .arg("-y")
                .output()
                .unwrap();
            assert!(
                !output.status.success(),
                "{source} must require confirmation"
            );
            assert!(
                output.stdout.is_empty(),
                "{source} must not be emitted for execution"
            );
        }
    }
    for (source, expected, approval) in [
        (
            "gawk --re-intervla ./input/content.txt",
            "gawk --re-interval ./input/content.txt",
            false,
        ),
        (
            "watch --intervla=2 echo fixture",
            "watch --interval=2 echo fixture",
            false,
        ),
        (
            "gfind ./input -maxdpeth 2",
            "gfind ./input -maxdepth 2",
            false,
        ),
        (
            "gsed --quite p ./input/content.txt",
            "gsed --quiet p ./input/content.txt",
            false,
        ),
        (
            "ggrep --line-numebr fixture ./input/content.txt",
            "ggrep --line-number fixture ./input/content.txt",
            false,
        ),
        (
            "gxargs --no-run-if-emty echo",
            "gxargs --no-run-if-empty echo",
            false,
        ),
        ("gfind ./input -delte", "gfind ./input -delete", true),
        (
            "gsed --in-palce=.bak 's/fixture/changed/' ./input/content.txt",
            "gsed --in-place=.bak 's/fixture/changed/' ./input/content.txt",
            true,
        ),
    ] {
        if notypo::utils::which(source.split_whitespace().next().unwrap()).is_none() {
            eprintln!("skipped core shell: {source} is not installed (unverified)");
            continue;
        }
        let report = correct("/usr/bin:/bin:/usr/sbin:/sbin", source, true);
        assert_eq!(
            report["candidates"][0]["command"], expected,
            "{source}: {report}"
        );
        assert_eq!(
            report["candidates"][0]["safety"]["decision"],
            if approval { "confirm" } else { "allow" },
            "{report}"
        );
        eprintln!("{source} -> {expected}");
    }
    let info = notypo::utils::which("info").or_else(|| {
        [
            "/opt/homebrew/opt/texinfo/bin/info",
            "/usr/local/opt/texinfo/bin/info",
        ]
        .iter()
        .map(PathBuf::from)
        .find(|path| path.is_file())
    });
    if let Some(info) = info {
        let source = format!("{} --aprops=ls", info.display());
        let expected = format!("{} --apropos=ls", info.display());
        assert_eq!(
            correct("/usr/bin:/bin:/usr/sbin:/sbin", &source, true)["candidates"][0]["command"],
            expected
        );
    } else {
        eprintln!("skipped core shell: info is not installed (unverified)");
    }
    // mawk's -W values are not a finite enum documented in the formats we
    // currently read. It remains a recorded capability limit.
    if notypo::utils::which("mawk").is_some() {
        for source in ["mawk -W version", "mawk -W versoin"] {
            assert_eq!(
                correct("/usr/bin:/bin:/usr/sbin:/sbin", source, true)["candidates"],
                serde_json::json!([]),
                "{source}"
            );
        }
    } else {
        eprintln!("skipped core shell: mawk is not installed (unverified)");
    }
    assert!(
        !workspace.0.join("output").exists(),
        "discovery wrote the output target"
    );
    assert_eq!(fs::read_to_string(&content).unwrap(), "fixture\n");
    assert_eq!(
        fs::metadata(&content).unwrap().permissions().mode(),
        permissions
    );
    assert_eq!(fs::read_dir(&input).unwrap().count(), 1);
}

/// Version control and code hosting (section 5B): glab and git-lfs (cobra),
/// `git lfs` through git-lfs's own completer, hub (its fish script),
/// Mercurial (bash handler; a repository's .hg/hgrc can load extensions,
/// so it needs workspace trust), and pre-commit (help; installing hooks
/// needs approval).
#[cfg(unix)]
#[test]
#[ignore = "uses the version control tools installed on this machine"]
fn installed_vcs_tools_repair_typos_and_keep_repositories_closed() {
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command;

    let ctx = Context::new(Settings::default(), Shell::Bash, "fuck".into());
    struct Workspace(PathBuf);
    impl Drop for Workspace {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let workspace = Workspace(
        std::env::temp_dir().join(format!("notypo-installed-vcs-{}", std::process::id())),
    );
    let home = workspace.0.join("home");
    let markers = workspace.0.join("markers");
    for dir in [&home, &markers] {
        fs::create_dir_all(dir).unwrap();
    }
    let correct = |source: &str, cwd: &std::path::Path, trusted: &str| {
        let output = Command::new(env!("CARGO_BIN_EXE_notypo"))
            .current_dir(cwd)
            .env("HOME", &home)
            .env("XDG_CONFIG_HOME", &workspace.0)
            .env("XDG_CACHE_HOME", &workspace.0)
            .env("TF_SHELL", "bash")
            .env("NOTYPO_TRUSTED_COMPLETERS", "glab:hub:git-lfs:hg")
            .env("NOTYPO_TRUSTED_HELP", "pre-commit")
            .env("NOTYPO_TRUSTED_WORKSPACES", trusted)
            .env("NOTYPO_DISABLED_SOURCES", "history:legacy")
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
    let cobra = "cobra completion";
    // (typo, repair, needs approval, source)
    for (typo, expected, approval, source) in [
        ("glab mr lsit", "glab mr list", false, cobra),
        ("glab isue create", "glab issue create", false, cobra),
        (
            "git-lfs trak '*.psd'",
            "git-lfs track '*.psd'",
            false,
            cobra,
        ),
        (
            "git lfs trak '*.psd'",
            "git lfs track '*.psd'",
            false,
            cobra,
        ),
        (
            "git lfs track --lockabel x",
            "git lfs track --lockable x",
            false,
            cobra,
        ),
        (
            "hub pul-request",
            "hub pull-request",
            false,
            "fish completion",
        ),
        ("hg stauts", "hg status", false, "bash completion"),
        (
            "hg log --limt 3",
            "hg log --limit 3",
            false,
            "bash completion",
        ),
        (
            "pre-commit rnu --all-files",
            "pre-commit run --all-files",
            false,
            "--help output",
        ),
        (
            "pre-commit instal",
            "pre-commit install",
            true,
            "--help output",
        ),
    ] {
        let app = typo.split(' ').next().unwrap();
        let needs = if app == "git" { "git-lfs" } else { app };
        if ctx.which(needs).is_none() {
            eprintln!("skipped {needs}: not installed (unverified)");
            continue;
        }
        let report = correct(typo, &workspace.0, "");
        let candidate = &report["candidates"][0];
        assert_eq!(candidate["command"], expected, "{typo}: {report}");
        assert!(
            candidate["edits"]
                .as_array()
                .unwrap()
                .iter()
                .all(|edit| edit["via"] == source),
            "{typo}: {report}"
        );
        assert_eq!(
            candidate["safety"]["decision"],
            if approval { "confirm" } else { "allow" },
            "{typo}: {report}"
        );
        eprintln!("{typo} -> {expected}; {} probes", report["probes"]);
    }
    // A repository whose configuration enables an extension: nothing is
    // probed until the workspace is trusted.
    if ctx.which("hg").is_some() {
        let repo = workspace.0.join("repo");
        fs::create_dir_all(repo.join(".hg")).unwrap();
        let extension = workspace.0.join("ext.py");
        fs::write(
            &extension,
            format!(
                "open({:?}, 'w').write('x')\n",
                markers.join("hg").display().to_string()
            ),
        )
        .unwrap();
        fs::write(
            repo.join(".hg/hgrc"),
            format!("[extensions]\nmarker = {}\n", extension.display()),
        )
        .unwrap();
        fs::write(repo.join(".hg/requires"), "store\n").unwrap();
        let report = correct("hg stauts", &repo, "");
        assert_eq!(report["candidates"], serde_json::json!([]), "{report}");
        assert_eq!(report["probes"], 0, "{report}");
        assert!(!markers.join("hg").exists(), "the extension ran");
    }
    // fish creates an empty ~/.local/share/fish for hub's script.
    let files: Vec<_> = walk(&home).into_iter().filter(|p| p.is_file()).collect();
    assert!(files.is_empty(), "probes wrote {files:?}");
}

/// JavaScript package managers and runtimes (section 5B): yarn 1 (its help
/// bullets commands), pnpm (`pnpm completion-server` through its bash
/// script), bun (its static bash script), and deno, in a project whose
/// scripts would write markers. No script runs; installs need approval.
#[cfg(unix)]
#[test]
#[ignore = "uses the JavaScript tools installed on this machine"]
fn installed_js_package_managers_repair_typos_without_running_scripts() {
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command;

    let ctx = Context::new(Settings::default(), Shell::Bash, "fuck".into());
    struct Workspace(PathBuf);
    impl Drop for Workspace {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let workspace =
        Workspace(std::env::temp_dir().join(format!("notypo-installed-js-{}", std::process::id())));
    let home = workspace.0.join("home");
    let project = workspace.0.join("project");
    let markers = workspace.0.join("markers");
    for dir in [&home, &project, &markers] {
        fs::create_dir_all(dir).unwrap();
    }
    let touch = |name: &str| format!("touch {}", markers.join(name).display());
    fs::write(
        project.join("package.json"),
        serde_json::json!({
            "name": "x",
            "version": "1.0.0",
            "scripts": {
                "build": touch("build"),
                "test": touch("test"),
                "preinstall": touch("preinstall"),
            }
        })
        .to_string(),
    )
    .unwrap();
    let correct = |source: &str| {
        let output = Command::new(env!("CARGO_BIN_EXE_notypo"))
            .current_dir(&project)
            .env("HOME", &home)
            .env("XDG_CONFIG_HOME", &workspace.0)
            .env("XDG_CACHE_HOME", &workspace.0)
            .env("TF_SHELL", "bash")
            .env("NOTYPO_TRUSTED_COMPLETERS", "pnpm:bun:deno")
            .env("NOTYPO_TRUSTED_HELP", "yarn:deno")
            .env("NOTYPO_DISABLED_SOURCES", "history:legacy")
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
    let bash = "bash completion";
    // (typo, repair, needs approval, source)
    for (typo, expected, approval, source) in [
        ("yarn instal", "yarn install", true, "--help output"),
        (
            "yarn add --dve lodash",
            "yarn add --dev lodash",
            true,
            "--help output",
        ),
        ("pnpm instal", "pnpm install", true, bash),
        ("pnpm run biuld", "pnpm run build", false, bash),
        (
            "pnpm add --save-dve lodash",
            "pnpm add --save-dev lodash",
            true,
            bash,
        ),
        // bun's script offers its commands again after any of them.
        ("bun instal", "bun install", true, bash),
        ("bun tset", "bun test", true, bash),
        ("deno rnu x.ts", "deno run x.ts", false, bash),
        (
            "deno run --allow-ent x.ts",
            "deno run --allow-net x.ts",
            false,
            "--help output",
        ),
    ] {
        let app = typo.split(' ').next().unwrap();
        if ctx.which(app).is_none() {
            eprintln!("skipped {app}: not installed (unverified)");
            continue;
        }
        let report = correct(typo);
        let candidate = &report["candidates"][0];
        assert_eq!(candidate["command"], expected, "{typo}: {report}");
        assert!(
            candidate["edits"]
                .as_array()
                .unwrap()
                .iter()
                .all(|edit| edit["via"] == source),
            "{typo}: {report}"
        );
        assert_eq!(
            candidate["safety"]["decision"],
            if approval { "confirm" } else { "allow" },
            "{typo}: {report}"
        );
        eprintln!("{typo} -> {expected}; {} probes", report["probes"]);
    }
    let ran: Vec<_> = fs::read_dir(&markers)
        .unwrap()
        .flatten()
        .map(|entry| entry.file_name())
        .collect();
    assert!(ran.is_empty(), "a script ran: {ran:?}");
    let files: Vec<_> = walk(&home).into_iter().filter(|p| p.is_file()).collect();
    assert!(files.is_empty(), "probes wrote {files:?}");
}

#[cfg(unix)]
fn walk(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        found.push(entry.path());
        if entry.path().is_dir() {
            found.extend(walk(&entry.path()));
        }
    }
    found
}

/// Data and text CLIs chained in pipelines (section 5B): yq (cobra), jq
/// (zsh handler), fzf, gron, xsv, and csvkit (trusted help), and Miller and
/// qsv (their own verb/command listings), repaired inside a pipeline, and
/// in a Miller `then` chain, as well as alone.
#[cfg(unix)]
#[test]
#[ignore = "uses the data and text CLIs installed on this machine"]
fn installed_data_clis_repair_typos_inside_pipelines() {
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command;

    let ctx = Context::new(Settings::default(), Shell::Bash, "fuck".into());
    struct Workspace(PathBuf);
    impl Drop for Workspace {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let workspace = Workspace(
        std::env::temp_dir().join(format!("notypo-installed-data-{}", std::process::id())),
    );
    let home = workspace.0.join("home");
    fs::create_dir_all(&home).unwrap();
    let correct = |source: &str| {
        let output = Command::new(env!("CARGO_BIN_EXE_notypo"))
            .current_dir(&workspace.0)
            .env("HOME", &home)
            .env("XDG_CONFIG_HOME", &workspace.0)
            .env("XDG_CACHE_HOME", &workspace.0)
            .env("TF_SHELL", "bash")
            .env("NOTYPO_TRUSTED_COMPLETERS", "github.com/mikefarah/yq/v4:jq")
            .env("NOTYPO_TRUSTED_HELP", "fzf:gron:mlr:qsv:xsv:csvlook:csvcut")
            .env("NOTYPO_DISABLED_SOURCES", "history:legacy")
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
    let both = "--help output and man page";
    // (typo, repair, decided without asking, source)
    for (typo, expected, decided, source) in [
        (
            "yq evl .a f.yaml",
            "yq eval .a f.yaml",
            true,
            "cobra completion",
        ),
        (
            "cat f.json | yq --prettyPrnt",
            "cat f.json | yq --prettyPrint",
            true,
            "cobra completion",
        ),
        (
            "cat f.json | jq --raw-ouptut .a",
            "cat f.json | jq --raw-output .a",
            false,
            "zsh completion",
        ),
        ("fzf --multii", "fzf --multi", false, both),
        ("gron --ungorn x", "gron --ungron x", false, "--help output"),
        (
            "mlr --icvs --ojson cat f.csv",
            "mlr --icsv --ojson cat f.csv",
            false,
            "mlr help and man page",
        ),
        // Miller's verbs come from `mlr help`, including each verb after
        // `then` once the previous verb's arguments are complete.
        (
            "mlr --icsv --ojson sotr -f a f.csv",
            "mlr --icsv --ojson sort -f a f.csv",
            false,
            "mlr help",
        ),
        (
            "mlr --icsv sort -nf a then hed -n 1 f.csv",
            "mlr --icsv sort -nf a then head -n 1 f.csv",
            false,
            "mlr help",
        ),
        (
            "cat f.csv | mlr --icsv put '$c = 1' then cta",
            "cat f.csv | mlr --icsv put '$c = 1' then cat",
            false,
            "mlr help",
        ),
        // qsv's commands come from `qsv --list`, nested ones from help.
        (
            "qsv slect 1 f.csv",
            "qsv select 1 f.csv",
            false,
            "qsv --list and --help",
        ),
        (
            "qsv cat rwos --pda f.csv",
            "qsv cat rows --pad f.csv",
            false,
            "qsv --list and --help",
        ),
        (
            "xsv slect 1 f.csv",
            "xsv select 1 f.csv",
            false,
            "--help output",
        ),
        (
            "csvcut -c 1 f.csv | csvlook --no-inferr",
            "csvcut -c 1 f.csv | csvlook --no-inference",
            false,
            both,
        ),
    ] {
        let app = typo
            .rsplit(" | ")
            .next()
            .unwrap()
            .split(' ')
            .next()
            .unwrap();
        if ctx.which(app).is_none() {
            eprintln!("skipped {app}: not installed (unverified)");
            continue;
        }
        let report = correct(typo);
        let candidate = &report["candidates"][0];
        assert_eq!(candidate["command"], expected, "{typo}: {report}");
        assert!(
            candidate["edits"]
                .as_array()
                .unwrap()
                .iter()
                .all(|edit| edit["via"] == source),
            "{typo}: {report}"
        );
        assert_eq!(
            report["outcome"]["kind"] == "suggestion",
            decided,
            "{typo}: {report}"
        );
        eprintln!("{typo} -> {expected}; {} probes", report["probes"]);
    }
}

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
    ("composer", "composer instlal", "composer install"),
    (
        "composer",
        "composer install --dry-rnu",
        "composer install --dry-run",
    ),
    (
        "composer",
        "composer list --format jsno",
        "composer list --format json",
    ),
    (
        "promtool",
        "promtool chek config prometheus.yml",
        "promtool check config prometheus.yml",
    ),
    (
        "promtool",
        "promtool check config --syntax-onyl prometheus.yml",
        "promtool check config --syntax-only prometheus.yml",
    ),
    ("kopia", "kopia snapshot lsit", "kopia snapshot list"),
    (
        "kopia",
        "kopia snapshot restore --paralell 4 object-id output",
        "kopia snapshot restore --parallel 4 object-id output",
    ),
    (
        "kopia",
        "kopia snapshot ls --revesre",
        "kopia snapshot ls --reverse",
    ),
    // Kafka clients (cobra) and rabbitmqadmin's installed zsh handler.
    ("kaf", "kaf topcis", "kaf topics"),
    ("kaf", "kaf topic delte orders", "kaf topic delete orders"),
    ("kafkactl", "kafkactl get topcis", "kafkactl get topics"),
    (
        "kafkactl",
        "kafkactl get topics --outptu json",
        "kafkactl get topics --output json",
    ),
    (
        "rabbitmqadmin",
        "rabbitmqadmin list queus",
        "rabbitmqadmin list queues",
    ),
    (
        "rabbitmqadmin",
        "rabbitmqadmin delete queue --nmae orders",
        "rabbitmqadmin delete queue --name orders",
    ),
    ("nats", "nats strema ls", "nats stream ls"),
    ("nats", "nats stream ls --jsoon", "nats stream ls --json"),
    (
        "nats",
        "nats s rm --froce example",
        "nats s rm --force example",
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
            "composer".into(),
            "github.com/prometheus/prometheus/cmd/promtool".into(),
            "github.com/kopia/kopia".into(),
            "github.com/nats-io/natscli/nats".into(),
            "github.com/birdayz/kaf/cmd/kaf".into(),
            "github.com/deviceinsight/kafkactl/v5".into(),
            "rabbitmqadmin".into(),
        ],
        // lefthook links urfave/cli v3.10+, whose completion runs Before hooks.
        // A click app is recognized from its help. Kingpin help distinguishes
        // command groups from potentially networked argument completers.
        trusted_help: vec![
            "sofka".into(),
            "rust:cargo".into(),
            "lefthook".into(),
            "python:samcli".into(),
            "python:oci_cli".into(),
            "composer".into(),
            "promtool".into(),
            "kopia".into(),
            "nats".into(),
        ],
        ..Settings::default()
    };
    let ctx = Context::new(settings, Shell::Bash, "fuck".into()).with_history::<&str>(&[]);
    let mut failures = Vec::new();
    let excluded = std::env::var("NOTYPO_TEST_SKIP_CLIS").unwrap_or_default();
    for (app, typo, expected) in CASES {
        if excluded.split(':').any(|excluded| excluded == *app) {
            eprintln!("excluded {app} by NOTYPO_TEST_SKIP_CLIS (unverified)");
            continue;
        }
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
        if (*app == "heroku" && typo.contains("destory"))
            || (["kaf", "rabbitmqadmin"].contains(app) && typo.contains("del"))
        {
            let candidate = report.outcome.candidates().first().unwrap();
            assert_eq!(candidate.safety.decision, Decision::Confirm);
        }
        if *app == "composer" {
            let candidate = report.outcome.candidates().first().unwrap();
            assert!(
                candidate
                    .edits
                    .iter()
                    .all(|edit| edit.via == "symfony completion")
            );
            assert_eq!(candidate.safety.decision, Decision::Confirm);
        }
        if ["promtool", "kopia", "nats"].contains(app) && got.as_deref() == Some(*expected) {
            let candidate = report.outcome.candidates().first().unwrap();
            assert!(
                candidate
                    .edits
                    .iter()
                    .all(|edit| edit.via == "kingpin completion")
            );
            assert!(!decided, "kingpin lists are partial: {:?}", report.outcome);
            if *app == "nats" && typo.contains(" rm ") {
                assert_eq!(candidate.safety.decision, Decision::Confirm);
            }
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
        // nats-server prints its help and exits 1.
        (
            "nats-server",
            "nats-server --jetsream",
            "nats-server --jetstream",
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
