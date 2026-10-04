//! The installed app's identity, read from its own package metadata: a Go
//! binary's main package, the module a Python console script imports its
//! entry point from, or the npm package a Node bin belongs to. Programs that
//! share a basename (MongoDB's and Ariga's `atlas`, Cloud Foundry's and
//! Cloudflare's `cf`) differ here, so trust can follow the app rather than
//! its name, and a renamed copy keeps it.

use std::fs;
use std::path::{Component, Path};

/// `github.com/org/app/cmd/app`, `python:awscli`, or `npm:@scope/app`.
pub fn identify(path: &Path) -> Option<String> {
    let real = fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    if let Some(package) = npm_package(&real) {
        return Some(format!("npm:{package}"));
    }
    let head = super::head(&real, 4096);
    if head.starts_with("#!") {
        return python_module(&head)
            .or_else(|| python_launcher(&head))
            .map(|module| format!("python:{module}"));
    }
    super::go::module(&real).map(|module| module.path)
}

/// Whether `trusted` lets notypo probe the app named `name`: every app
/// (`*`), any app invoked by that name, or the app with that identity.
pub fn is_trusted(trusted: &[String], name: &str, identity: Option<&str>) -> bool {
    trusted
        .iter()
        .any(|t| t == "*" || t == name || identity == Some(t.as_str()))
}

/// The package a file inside `node_modules` belongs to, by its manifest.
fn npm_package(real: &Path) -> Option<String> {
    let parts: Vec<Component> = real.components().collect();
    let at = parts
        .iter()
        .rposition(|c| c.as_os_str() == "node_modules")?;
    let first = parts.get(at + 1)?.as_os_str().to_str()?;
    let depth = if first.starts_with('@') { 2 } else { 1 };
    let root: std::path::PathBuf = parts.get(..=at + depth)?.iter().collect();
    let manifest = fs::read(root.join("package.json")).ok()?;
    if manifest.len() > 1024 * 1024 {
        return None;
    }
    let manifest: serde_json::Value = serde_json::from_slice(&manifest).ok()?;
    let name = manifest.get("name")?.as_str()?;
    is_package_name(name).then(|| name.to_owned())
}

/// A shell wrapper that runs `python -m <module>`, as Homebrew's az does
/// (`python -Im azure.cli`). The whole module path is kept: `azure` alone
/// is a namespace that many packages share.
fn python_launcher(head: &str) -> Option<String> {
    let shebang = head.lines().next()?;
    if !shebang.ends_with("sh") && !shebang.contains("sh ") {
        return None;
    }
    head.lines().find_map(|line| {
        let mut words = line.split_whitespace().skip_while(|w| {
            !w.rsplit('/')
                .next()
                .is_some_and(|name| name.starts_with("python"))
        });
        words.next()?;
        let mut words =
            words.skip_while(|w| !(w.starts_with('-') && w.ends_with('m') && !w.starts_with("--")));
        words.next()?;
        let module = words.next()?;
        (module
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.'))
            && !module.is_empty())
        .then(|| module.to_owned())
    })
}

fn is_package_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 214
        && name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"@/._-".contains(&c))
}

/// The top-level module a Python console script runs: pip's and uv's
/// `from awscli.clidriver import main`, or setuptools' older
/// `load_entry_point('awscli==1.29.0', 'console_scripts', 'aws')`.
fn python_module(head: &str) -> Option<String> {
    let shebang = head.lines().next()?;
    if !shebang.contains("python") {
        return None;
    }
    // setuptools names the distribution; its loader import is not the app.
    let distribution = head.lines().find_map(|line| {
        let rest = line.split_once("load_entry_point(")?.1;
        let quoted = rest.trim_start().strip_prefix(['\'', '"'])?;
        quoted.split(['=', '\'', '"', '<', '>', '!', '~']).next()
    });
    // Standard modules a console script imports before its entry point.
    let loader = [
        "__future__",
        "pkg_resources",
        "importlib",
        "sys",
        "re",
        "os",
        "runpy",
        "site",
    ];
    let module = distribution
        .or_else(|| {
            head.lines().find_map(|line| {
                let (module, import) = line.trim().strip_prefix("from ")?.split_once(" import ")?;
                (!import.is_empty() && !loader.contains(&module)).then_some(module)
            })
        })
        .or_else(|| {
            // aws's `import awscli.clidriver`.
            head.lines().find_map(|line| {
                let module = line
                    .trim()
                    .strip_prefix("import ")?
                    .split([' ', ','])
                    .next()?;
                let top = module.split('.').next()?;
                (!loader.contains(&top)).then_some(module)
            })
        })?;
    let top = module.split('.').next()?.replace('-', "_");
    (!top.is_empty() && top.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')).then_some(top)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn python_entry_modules_are_read_from_console_scripts() {
        assert_eq!(
            python_module(
                "#!/opt/venv/bin/python3\n# -*- coding: utf-8 -*-\nimport re\nimport sys\nfrom awscli.clidriver import main\nif __name__ == '__main__':\n    sys.exit(main())\n"
            )
            .as_deref(),
            Some("awscli")
        );
        assert_eq!(
            python_module(
                "#!/usr/bin/python\n# EASY-INSTALL-ENTRY-SCRIPT: 'jdcloud-cli==1.2','console_scripts','jdc'\n__requires__ = 'jdcloud-cli==1.2'\nimport re\nfrom pkg_resources import load_entry_point\nsys.exit(load_entry_point('jdcloud-cli==1.2', 'console_scripts', 'jdc')())\n"
            )
            .as_deref(),
            Some("jdcloud_cli"),
            "setuptools names the distribution, not its loader"
        );
        assert_eq!(
            python_module("#!/bin/sh\nfrom x import y\n"),
            None,
            "only Python scripts"
        );
        assert_eq!(
            python_module("#!/usr/bin/env python3\nprint('no entry point')\n"),
            None
        );
        // Homebrew's aws and az, as installed.
        assert_eq!(
            python_module(
                "#!/opt/homebrew/Cellar/awscli/2.37.7/libexec/bin/python\n# Copyright\nimport os\nimport sys\nimport awscli.clidriver\ndef main():\n    return awscli.clidriver.main()\n"
            )
            .as_deref(),
            Some("awscli")
        );
        assert_eq!(
            python_launcher(
                "#!/usr/bin/env bash\nAZ_INSTALLER=HOMEBREW /opt/homebrew/Cellar/azure-cli/2.90.0/libexec/bin/python -Im azure.cli \"$@\"\n"
            )
            .as_deref(),
            Some("azure.cli")
        );
        assert_eq!(
            python_launcher("#!/bin/sh\nexec node cli.js \"$@\"\n"),
            None
        );
    }

    #[test]
    fn npm_bins_are_identified_by_their_package_manifest() {
        let dir = std::env::temp_dir().join(format!("notypo-identity-{}", std::process::id()));
        let package = dir.join("lib/node_modules/@serverless-devs/s");
        fs::create_dir_all(package.join("bin")).unwrap();
        fs::write(
            package.join("package.json"),
            r#"{"name": "@serverless-devs/s", "bin": {"s": "bin/s.js"}}"#,
        )
        .unwrap();
        fs::write(package.join("bin/s.js"), "#!/usr/bin/env node\n").unwrap();
        assert_eq!(
            npm_package(&package.join("bin/s.js")).as_deref(),
            Some("@serverless-devs/s")
        );
        fs::write(package.join("package.json"), r#"{"name": "$(touch x)"}"#).unwrap();
        assert_eq!(npm_package(&package.join("bin/s.js")), None);
        assert_eq!(npm_package(Path::new("/usr/bin/s")), None);
        let _ = fs::remove_dir_all(&dir);
    }
}
