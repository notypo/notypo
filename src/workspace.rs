//! Workspace trust. Some programs evaluate project files from the working
//! directory just to list what they can do: make runs a Makefile's
//! `$(shell ...)`, rake and fastlane load Ruby, gradle, sbt, and lein run
//! build scripts, gulp and grunt load JavaScript, flask imports the
//! application, devspace runs its configuration's command variables, and
//! garden's root help resolves the project's configuration to list its
//! custom commands. notypo runs such listings (completion handlers, `--help`
//! probes, and the legacy rules that list tasks) only in directories that
//! `trusted_workspaces` names, and never runs a program that lives in an
//! untrusted directory to list anything.
//!
//! A directory without the program's project file is no project of its, so
//! listing there evaluates nothing of the user's.

use std::path::{Path, PathBuf};

/// Program names, the project files their listings evaluate, and whether
/// they look for those files in parent directories too.
const EVALUATORS: &[(&[&str], &[&str], bool)] = &[
    (
        &["make", "gmake", "bmake", "remake"],
        &["GNUmakefile", "makefile", "Makefile", "BSDmakefile"],
        false,
    ),
    (
        &["rake"],
        &["Rakefile", "rakefile", "Rakefile.rb", "rakefile.rb"],
        true,
    ),
    (
        &["gradle", "gradlew"],
        &[
            "build.gradle",
            "build.gradle.kts",
            "settings.gradle",
            "settings.gradle.kts",
        ],
        true,
    ),
    (&["sbt"], &["build.sbt"], false),
    (&["lein"], &["project.clj"], true),
    (&["mix"], &["mix.exs"], false),
    (&["fastlane"], &["fastlane/Fastfile", "Fastfile"], false),
    (&["mage"], &["magefile.go", "magefiles"], false),
    (
        &["bazel", "bazelisk"],
        &["MODULE.bazel", "WORKSPACE", "WORKSPACE.bazel"],
        true,
    ),
    // go-task evaluates `sh:` variables while listing.
    (
        &["task", "go-task"],
        &[
            "Taskfile.yml",
            "Taskfile.yaml",
            "taskfile.yml",
            "taskfile.yaml",
            "Taskfile.dist.yml",
            "Taskfile.dist.yaml",
        ],
        true,
    ),
    (
        &["gulp"],
        &[
            "gulpfile.js",
            "gulpfile.mjs",
            "gulpfile.cjs",
            "gulpfile.ts",
            "gulpfile.babel.js",
            "Gulpfile.js",
        ],
        true,
    ),
    (
        &["grunt"],
        &["Gruntfile.js", "Gruntfile.coffee", "gruntfile.js"],
        true,
    ),
    // Loads the CLI and configuration installed in the project.
    (&["react-native"], &["package.json"], true),
    (&["nx"], &["nx.json"], true),
    (&["nox"], &["noxfile.py"], false),
    (&["invoke", "inv"], &["tasks.py", "tasks"], false),
    (&["fab"], &["fabfile.py", "fabfile"], false),
    (&["vagrant"], &["Vagrantfile"], true),
    // Loads plugins from the project's vendor directory.
    (&["composer"], &["composer.json"], true),
    // Also `FLASK_APP`, which can name an application anywhere.
    (&["flask"], &[".flaskenv", "app.py", "wsgi.py"], false),
    // Runs the configuration's `vars` commands for completion and --help.
    (&["devspace"], &["devspace.yaml", "devspace.yml"], false),
    // Even --help loads `.sequelizerc` (JavaScript) and the project's package.
    (&["sequelize", "sequelize-cli"], &[".sequelizerc"], false),
    // Even --help loads conftest.py files and plugins the configuration
    // names (`-p` in addopts, with `pythonpath`).
    (
        &["pytest", "py.test"],
        &[
            "conftest.py",
            "pytest.ini",
            ".pytest.ini",
            "pyproject.toml",
            "tox.ini",
            "setup.cfg",
        ],
        true,
    ),
    // A repository's configuration can enable extensions (Python) that
    // load for help and for the handler's `hg debugcomplete`.
    (&["hg", "chg"], &[".hg/hgrc"], true),
    // Even --help loads the site's configuration to register plugin commands.
    (
        &["docusaurus"],
        &[
            "docusaurus.config.ts",
            "docusaurus.config.mts",
            "docusaurus.config.cts",
            "docusaurus.config.js",
            "docusaurus.config.mjs",
            "docusaurus.config.cjs",
        ],
        false,
    ),
];

/// The name a program goes by: its file name, without Windows' executable
/// extensions.
fn base_name(program: &str) -> String {
    let name = program
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(program)
        .to_owned();
    match name.rsplit_once('.') {
        Some((stem, extension))
            if ["exe", "bat", "cmd", "com"]
                .iter()
                .any(|e| extension.eq_ignore_ascii_case(e)) =>
        {
            stem.to_owned()
        }
        _ => name,
    }
}

/// The project file listing with `program` would evaluate from `cwd`.
fn project_file(program: &str, cwd: &Path) -> Option<PathBuf> {
    let name = base_name(program);
    if name == "dotnet" {
        // Completion evaluates a project in cwd. Parent-level SDK selection
        // and imports can also supply custom SDKs and property functions.
        let project = cwd.ancestors().find_map(|directory| {
            std::fs::read_dir(directory).ok().and_then(|entries| {
                entries
                    .filter_map(Result::ok)
                    .map(|entry| entry.path())
                    .find(|path| {
                        path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
                            let extension = e.to_ascii_lowercase();
                            extension.ends_with("proj")
                                || ["sln", "slnx"].contains(&extension.as_str())
                        })
                    })
            })
        });
        return project.or_else(|| {
            cwd.ancestors()
                .flat_map(|dir| {
                    [
                        "global.json",
                        "Directory.Build.props",
                        "Directory.Build.targets",
                        "Directory.Packages.props",
                    ]
                    .map(|file| dir.join(file))
                })
                .find(|path| path.exists())
        });
    }
    if name == "garden" {
        // garden finds its project root upwards: `project.garden.yml`,
        // `garden.yml`, or any `*.garden.yml` configuration.
        return cwd.ancestors().find_map(|dir| {
            let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
                .ok()?
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| {
                    path.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
                        ["garden.yml", "garden.yaml"].contains(&n)
                            || n.ends_with(".garden.yml")
                            || n.ends_with(".garden.yaml")
                    })
                })
                .collect();
            entries.sort();
            entries.into_iter().next()
        });
    }
    let (_, files, upward) = EVALUATORS
        .iter()
        .find(|(names, _, _)| names.contains(&name.as_str()))?;
    if name == "flask"
        && let Some(app) = std::env::var_os("FLASK_APP").filter(|a| !a.is_empty())
    {
        return Some(cwd.join(app));
    }
    let dirs: Vec<&Path> = if *upward {
        cwd.ancestors().collect()
    } else {
        vec![cwd]
    };
    dirs.into_iter()
        .flat_map(|dir| files.iter().map(move |file| dir.join(file)))
        .find(|path| path.exists())
}

/// `trusted` names `cwd` or a directory above it (`*` trusts every one).
pub fn is_trusted(cwd: &Path, trusted: &[String]) -> bool {
    let cwd = std::fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_owned());
    trusted.iter().any(|entry| {
        if entry == "*" {
            return true;
        }
        let dir = crate::utils::expand_user(entry);
        dir.is_absolute()
            && std::fs::canonicalize(&dir)
                .ok()
                .is_some_and(|dir| cwd.starts_with(dir))
    })
}

/// Why listing with `program` from `cwd` would run project code that
/// `trusted` doesn't allow: the program lives in that directory, or reads
/// a project file there. `None` when it may run.
pub fn untrusted_project(program: &str, cwd: &Path, trusted: &[String]) -> Option<String> {
    if trusted.iter().any(|entry| entry == "*") {
        return None;
    }
    let reason = if program.contains(['/', '\\']) {
        let path = cwd.join(program);
        let inside = match (std::fs::canonicalize(&path), std::fs::canonicalize(cwd)) {
            (Ok(path), Ok(cwd)) => path.starts_with(cwd),
            _ => Path::new(program).is_relative(),
        };
        inside.then(|| format!("{program} is a program from this directory"))
    } else {
        None
    }
    .or_else(|| {
        let file = project_file(program, cwd)?;
        Some(format!(
            "{} would evaluate {} to list its commands",
            base_name(program),
            file.display()
        ))
    })?;
    if is_trusted(cwd, trusted) {
        return None;
    }
    Some(format!(
        "{reason}; add {} to trusted_workspaces to allow that",
        cwd.display()
    ))
}

/// Whether a legacy rule may run `program` from the current directory to
/// list a project's tasks; a refusal is logged for debugging.
pub fn allows(program: &str, trusted: &[String]) -> bool {
    let Ok(cwd) = std::env::current_dir() else {
        return false;
    };
    match untrusted_project(program, &cwd, trusted) {
        Some(why) => {
            crate::logs::debug(format_args!("Not listing tasks: {why}"));
            false
        }
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Dir(PathBuf);

    impl Dir {
        fn new(tag: &str) -> Dir {
            let dir = std::env::temp_dir().join(format!("notypo-ws-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Dir(std::fs::canonicalize(&dir).unwrap())
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn garden_configurations_in_parent_directories_require_workspace_trust() {
        let root = Dir::new("garden");
        let nested = root.0.join("services/api");
        std::fs::create_dir_all(&nested).unwrap();
        assert_eq!(untrusted_project("garden", &nested, &[]), None);
        for file in ["project.garden.yml", "garden.yml", "api.garden.yaml"] {
            std::fs::write(root.0.join(file), "kind: Project\n").unwrap();
            let why = untrusted_project("/opt/homebrew/bin/garden", &nested, &[]).unwrap();
            assert!(why.contains(file), "{why}");
            assert_eq!(
                untrusted_project("garden", &nested, &[root.0.display().to_string()]),
                None
            );
            std::fs::remove_file(root.0.join(file)).unwrap();
        }
        // A file that merely mentions garden is no configuration.
        std::fs::write(root.0.join("garden.yml.bak"), "").unwrap();
        assert_eq!(untrusted_project("garden", &nested, &[]), None);
    }

    #[test]
    fn dotnet_project_and_parent_imports_require_workspace_trust() {
        let root = Dir::new("dotnet");
        let project = root.0.join("app");
        std::fs::create_dir_all(project.join("nested")).unwrap();
        assert_eq!(untrusted_project("dotnet", &project, &[]), None);
        for file in [
            "app.csproj",
            "app.fsproj",
            "app.vbproj",
            "app.proj",
            "app.vcxproj",
            "app.sqlproj",
            "app.sln",
            "app.slnx",
        ] {
            let path = project.join(file);
            std::fs::write(&path, "").unwrap();
            assert!(
                untrusted_project("dotnet.exe", &project, &[]).is_some(),
                "{file}"
            );
            assert_eq!(
                untrusted_project("dotnet", &project, &[project.display().to_string()]),
                None
            );
            assert!(
                untrusted_project("dotnet", &project.join("nested"), &[]).is_some(),
                "parent {file}"
            );
            std::fs::remove_file(path).unwrap();
        }
        for file in [
            "global.json",
            "Directory.Build.props",
            "Directory.Build.targets",
            "Directory.Packages.props",
        ] {
            let path = root.0.join(file);
            std::fs::write(&path, "").unwrap();
            assert!(
                untrusted_project("dotnet", &project.join("nested"), &[]).is_some(),
                "{file}"
            );
            std::fs::remove_file(path).unwrap();
        }
    }

    #[test]
    fn project_listings_need_a_trusted_workspace() {
        let root = Dir::new("project");
        let project = root.0.join("app");
        std::fs::create_dir_all(project.join("src/deep")).unwrap();
        std::fs::write(project.join("Makefile"), "all:\n").unwrap();
        std::fs::write(project.join("build.gradle"), "").unwrap();
        let nested = project.join("src/deep");
        let none: &[String] = &[];

        let why = untrusted_project("make", &project, none).unwrap();
        // make's own order finds `makefile` first on a case-insensitive disk.
        assert!(
            why.to_lowercase().contains("makefile") && why.contains("trusted_workspaces"),
            "{why}"
        );
        // make reads only the working directory; gradle looks upward.
        assert_eq!(untrusted_project("make", &nested, none), None);
        assert!(untrusted_project("gradle", &nested, none).is_some());
        assert!(untrusted_project("gradlew.bat", &nested, none).is_some());
        // A program from the directory is the project's own code.
        assert!(untrusted_project("./gradlew", &project, none).is_some());
        assert!(untrusted_project("bin/console", &project, none).is_some());
        // Programs that evaluate nothing, and directories without project
        // files, need no trust.
        assert_eq!(untrusted_project("git", &project, none), None);
        assert_eq!(untrusted_project("just", &project, none), None);
        assert_eq!(untrusted_project("make", &root.0, none), None);
        assert_eq!(untrusted_project("/bin/ls", &project, none), None);
        // pytest loads conftest.py for --help, from parent directories too.
        std::fs::write(project.join("conftest.py"), "").unwrap();
        assert!(untrusted_project("pytest", &nested, none).is_some());
        // A Mercurial repository's own configuration names extensions.
        std::fs::create_dir_all(project.join(".hg")).unwrap();
        std::fs::write(project.join(".hg/hgrc"), "[extensions]\n").unwrap();
        assert!(untrusted_project("hg", &nested, none).is_some());
        assert_eq!(untrusted_project("hg", &root.0, none), None);
        // docusaurus loads the site's configuration even for --help.
        std::fs::write(project.join("docusaurus.config.mjs"), "").unwrap();
        assert!(untrusted_project("docusaurus", &project, none).is_some());
        assert_eq!(untrusted_project("docusaurus", &nested, none), None);

        for trusted in [
            vec![project.display().to_string()],
            vec![root.0.display().to_string()],
            vec!["*".to_owned()],
        ] {
            assert_eq!(untrusted_project("make", &project, &trusted), None);
            assert_eq!(untrusted_project("./gradlew", &project, &trusted), None);
        }
        // A sibling, or a relative entry, trusts nothing here.
        for trusted in [
            vec![root.0.join("other").display().to_string()],
            vec!["app".to_owned()],
        ] {
            std::fs::create_dir_all(root.0.join("other")).unwrap();
            assert!(untrusted_project("make", &project, &trusted).is_some());
        }
    }
}
