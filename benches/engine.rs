//! Structured engine stages, in process: parsing, output diagnosis, ranking
//! against a large vocabulary, and whole corrections answered by in-memory
//! completers (so app startup, which dominates real runs, is excluded).
//! Driven by `benchmarks/structured.py`; prints one JSON line per stage.

use notypo::engine::native::{
    Capabilities, CompletionError, CompletionItem, NativeCompletionBackend, Trust,
};
use notypo::engine::probe::Budget;
use notypo::engine::{self, FailureContext, diagnosis, docs, parser, ranking};
use notypo::settings::Settings;
use notypo::shells::Shell;
use notypo::types::Context;
use serde_json::json;
use std::collections::HashMap;
use std::hint::black_box;
use std::rc::Rc;
use std::time::Instant;

const VOCABULARY: &str = include_str!("../tests/data/corpus/vocabulary.txt");
const EXECUTABLES: &str = include_str!("../tests/data/corpus/executables.txt");

#[derive(Debug)]
struct Tree(HashMap<String, Vec<String>>);

impl NativeCompletionBackend for Tree {
    fn id(&self) -> &str {
        "tree"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            subcommands: true,
            options: false,
            option_arity: false,
            values: false,
            resources: false,
            option_prefix: "--",
            short_options: false,
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
        _: &mut Budget,
    ) -> Result<Vec<CompletionItem>, CompletionError> {
        Ok(self
            .0
            .get(&words.join(" "))
            .into_iter()
            .flatten()
            .filter(|w| w.starts_with(prefix))
            .map(|w| CompletionItem {
                value: w.clone(),
                takes_value: None,
                description: None,
            })
            .collect())
    }
}

/// Median and p95 of `samples` timings of `iterations` calls, in µs per call.
fn measure(name: &str, iterations: u32, samples: usize, mut f: impl FnMut()) {
    for _ in 0..iterations.min(10) {
        f();
    }
    let mut times: Vec<f64> = (0..samples)
        .map(|_| {
            let started = Instant::now();
            for _ in 0..iterations {
                f();
            }
            started.elapsed().as_secs_f64() * 1e6 / f64::from(iterations)
        })
        .collect();
    times.sort_by(f64::total_cmp);
    let p95 = times[((times.len() as f64 * 0.95).ceil() as usize).clamp(1, times.len()) - 1];
    println!(
        "{}",
        json!({"stage": name, "median_us": times[times.len() / 2], "p95_us": p95, "iterations": iterations, "samples": samples})
    );
}

fn main() {
    // `cargo test --all-targets` runs harness-free benches: keep it short.
    let smoke = cfg!(debug_assertions) || std::env::args().any(|a| a == "--test");
    let (iterations, samples) = if smoke { (1, 1) } else { (2000, 20) };

    let mut trees: HashMap<String, HashMap<String, Vec<String>>> = HashMap::new();
    for line in VOCABULARY.lines() {
        let mut fields = line.splitn(3, '|');
        if let (Some(app), Some(context), Some(word)) =
            (fields.next(), fields.next(), fields.next())
        {
            trees
                .entry(app.into())
                .or_default()
                .entry(context.into())
                .or_default()
                .push(word.into());
        }
    }
    let executables: Vec<&str> = EXECUTABLES.lines().collect();
    let ctx = Context::new(Settings::default(), Shell::Bash, "fuck".into())
        .with_path_listing(&executables)
        .with_which("man", None)
        .with_history::<&str>(&[]);
    let backends: Vec<(String, Rc<dyn NativeCompletionBackend>)> = trees
        .into_iter()
        .map(|(app, levels)| {
            (
                app,
                Rc::new(Tree(levels)) as Rc<dyn NativeCompletionBackend>,
            )
        })
        .collect();
    let ec2: Vec<String> = VOCABULARY
        .lines()
        .filter_map(|l| l.strip_prefix("aws|ec2|").map(str::to_owned))
        .collect();

    let line = "AWS_PAGER='' aws --region eu-west-1 ec2 describ-instances --filters Name=x,Values=y | jq '.Reservations[]' > out.json # list";
    measure("parse", iterations, samples, || {
        black_box(parser::parse(black_box(line)));
    });
    let fish_line = "AWS_PAGER='' aws --region eu-west-1 ec2 describ-instances --filters 'Name=x,Values=y' 2>| cat >? 'out\\'s.json'; and echo '[literal]' # list";
    measure("parse fish", iterations, samples, || {
        black_box(parser::parse_with_dialect(
            black_box(fish_line),
            parser::Dialect::Fish,
        ));
    });
    measure("quote fish", iterations, samples, || {
        black_box(parser::quote_word_with_dialect(
            black_box("it's a \\ path; (literal)"),
            parser::Dialect::Fish,
        ));
    });
    let output = "aws: [ERROR]: argument operation: Found invalid choice 'describ-instances'\n\nusage: aws [options] <command> <subcommand> [<subcommand> ...] [parameters]\nTo see help text, you can run:\n\n  aws help\n";
    measure("diagnose output", iterations, samples, || {
        black_box(diagnosis::diagnose_output(black_box(output)));
    });
    let help = "Usage: tool nodes [COMMAND] [OPTIONS]\n\nCommands:\n  list    List nodes\n  show    Show a node\n\nOptions:\n  -p, --profile <PROFILE>  Select profile\n  -o, --output <FORMAT>  Output [possible values: json, yaml, text]\n  --region <REGION>  Select region\n  --verbose         Talk\n";
    measure("read help vocabulary", iterations, samples, || {
        black_box(docs::options(black_box(help)));
        black_box(docs::subcommands(black_box(help), "tool"));
        black_box(docs::option_values(black_box(help), "--output"));
    });
    let none = |_: &str| 0;
    measure(
        "rank 806 operations",
        (iterations / 10).max(1),
        samples,
        || {
            black_box(ranking::rank_tokens(
                black_box("describ-instances"),
                ec2.iter().map(String::as_str),
                &[],
                &none,
                3,
            ));
        },
    );
    measure(
        "rank 962 executables",
        (iterations / 10).max(1),
        samples,
        || {
            black_box(ranking::rank_tokens(
                black_box("gti"),
                executables.iter().copied(),
                &[],
                &none,
                3,
            ));
        },
    );
    for (name, command, status) in [
        (
            "correct: 2 typos, nested groups",
            "aws ec22 describ-instances",
            1,
        ),
        ("correct: program typo", "gti status", 127),
        ("correct: valid command", "gcloud compute instances", 1),
    ] {
        let failure = FailureContext {
            source: command.into(),
            exit_status: Some(status),
            ..FailureContext::default()
        };
        measure(name, (iterations / 10).max(1), samples, || {
            black_box(engine::correct_with_backends(
                &failure,
                &ctx,
                backends.clone(),
            ));
        });
    }
}
