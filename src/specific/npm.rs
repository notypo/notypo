use crate::utils::{run_stdout, which};
use regex::Regex;
use std::sync::{LazyLock, OnceLock};

pub fn npm_available() -> bool {
    which("npm").is_some()
}

/// Custom npm scripts (`npm run-script`), memoized.
pub fn get_scripts() -> &'static [String] {
    static SCRIPTS: OnceLock<Vec<String>> = OnceLock::new();
    SCRIPTS.get_or_init(|| parse_scripts(&run_stdout("npm", &["run-script"]).unwrap_or_default()))
}

pub fn parse_scripts(output: &str) -> Vec<String> {
    static SCRIPT: LazyLock<Regex> = LazyLock::new(|| Regex::new("^  [^ ]+").unwrap());
    let mut should_yield = false;
    let mut scripts = Vec::new();
    for line in output.split_inclusive('\n') {
        if line.contains("available via `npm run-script`:") {
            should_yield = true;
            continue;
        }
        if should_yield && SCRIPT.is_match(line) {
            scripts.extend(line.trim().split(' ').next().map(str::to_owned));
        }
    }
    scripts
}

#[cfg(test)]
mod tests {
    #[test]
    fn parses_run_script_output() {
        let out = "Lifecycle scripts included in code-view-web:\n  test\n    jest\n\navailable via `npm run-script`:\n  build\n    cp node_modules/ace-builds/src-min/ -a resources/ace/ && webpack --progress --colors -p --config ./webpack.production.config.js\n  develop\n    cp node_modules/ace-builds/src/ -a resources/ace/ && webpack-dev-server --progress --colors\n  watch-test\n    jest --verbose --watch\n\n";
        assert_eq!(
            super::parse_scripts(out),
            ["build", "develop", "watch-test"]
        );
    }
}
