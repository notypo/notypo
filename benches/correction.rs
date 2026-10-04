//! Warm correction engine: no process startup, rule imports, or command reruns.
//! Driven by `benchmarks/compare.py`; input and output are JSON.

use notypo::corrector::Corrector;
use notypo::rules::RULES;
use notypo::settings::Settings;
use notypo::shells::Shell;
use notypo::types::{Command, Context, Rule};
use serde_json::{Value, json};
use std::hint::black_box;
use std::io::{self, IsTerminal, Read};
use std::sync::LazyLock;
use std::time::Instant;

const NAMES: &[&str] = &[
    "cd_parent",
    "mkdir_p",
    "git_not_command",
    "sudo",
    "no_command",
];
static BENCH_RULES: LazyLock<Vec<Rule>> = LazyLock::new(|| {
    RULES
        .iter()
        .filter(|rule| NAMES.contains(&rule.name))
        .copied()
        .collect()
});

fn main() {
    // Cargo also executes harness-free benches during `test --all-targets`.
    // Debug builds validate fixtures without running timing batches.
    let smoke = cfg!(debug_assertions) || std::env::args().any(|arg| arg == "--test");
    let mut input = String::new();
    if !smoke && !io::stdin().is_terminal() {
        io::stdin().read_to_string(&mut input).unwrap();
    }
    if input.trim().is_empty() {
        let fixtures: Value =
            serde_json::from_str(include_str!("../benchmarks/fixtures.json")).unwrap();
        let mut executables: Vec<String> = ["git", "gzip", "grep", "go", "python", "mkdir", "ls"]
            .into_iter()
            .map(str::to_owned)
            .collect();
        executables.extend((0..1993).map(|i| format!("benchmark_tool_{i:04}")));
        for fixture in fixtures.as_array().unwrap() {
            println!(
                "{}",
                measure(&json!({
                    "fixture": fixture,
                    "executables": executables,
                    "iterations": if smoke { 1 } else { 5000 },
                    "samples": if smoke { 1 } else { 10 },
                }))
            );
        }
        return;
    }
    let input: Value = serde_json::from_str(&input).unwrap();
    println!("{}", measure(&input));
}

fn measure(input: &Value) -> Value {
    let script = input["fixture"]["script"].as_str().unwrap();
    let output = input["fixture"]["output"].as_str().unwrap();
    let expected = input["fixture"]["expected"].as_str().unwrap();
    let iterations = input["iterations"].as_u64().unwrap();
    let samples = input["samples"].as_u64().unwrap();
    let executables: Vec<&str> = input["executables"]
        .as_array()
        .unwrap()
        .iter()
        .map(|name| name.as_str().unwrap())
        .collect();
    let ctx = Context::new(
        Settings {
            rules_all: false,
            rules: NAMES.iter().map(|name| (*name).to_owned()).collect(),
            ..Settings::default()
        },
        Shell::Bash,
        "fuck".into(),
    )
    .with_executables(&executables)
    .with_history(&["git status"])
    .with_which("gti", None);
    notypo::logs::configure(true, false);

    let correct = || {
        let command = Command::new(black_box(script), Some(black_box(output)), &ctx);
        Corrector::with_rules(&command, &BENCH_RULES)
            .first()
            .expect("fixture must have a correction")
            .script
    };
    for _ in 0..10 {
        assert_eq!(correct(), expected);
    }
    let mut times = Vec::new();
    for _ in 0..samples {
        let start = Instant::now();
        for _ in 0..iterations {
            black_box(correct());
        }
        times.push(start.elapsed().as_nanos() as f64 / iterations as f64);
    }
    json!({"name": input["fixture"]["name"], "ns_per_correction": times, "iterations": iterations, "rule_count": BENCH_RULES.len(), "correction": expected})
}
