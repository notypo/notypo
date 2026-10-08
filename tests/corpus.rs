//! A labeled correction corpus for the structured engine.
//!
//! Real command vocabularies, captured once from installed aws, gcloud,
//! az, git, kubectl, docker, and helm (`data/corpus/vocabulary.txt`), long
//! option names of fifteen of their commands (`options.txt`, from their own
//! offline completion), plus the system's `/bin` and `/usr/bin` names
//! (`executables.txt`), are served by in-memory completers. Deterministic
//! typos (deletion, insertion, substitution, transposition) of sampled
//! words become failed commands whose intended correction is known.
//!
//! The test reports top-1/top-3 accuracy, how often the engine decides
//! alone, asks, or abstains, and how often a decision alone would run the
//! wrong command (an unsafe automatic acceptance). It also sweeps the
//! decision thresholds; `ranking::{ACCEPT, MARGIN}` were set from it.
//! Run with `--nocapture` to see the report.

use notypo::engine::native::{
    Capabilities, CompletionError, CompletionItem, NativeCompletionBackend, Trust,
};
use notypo::engine::probe::Budget;
use notypo::engine::{self, FailureContext, Outcome, ranking};
use notypo::settings::Settings;
use notypo::shells::Shell;
use notypo::types::Context;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;

const VOCABULARY: &str = include_str!("data/corpus/vocabulary.txt");
const OPTIONS: &str = include_str!("data/corpus/options.txt");
const EXECUTABLES: &str = include_str!("data/corpus/executables.txt");

/// Answers completion from a captured vocabulary: words keyed by the
/// subcommands before the slot.
#[derive(Debug)]
struct Tree {
    id: String,
    levels: HashMap<String, Vec<String>>,
}

impl NativeCompletionBackend for Tree {
    fn id(&self) -> &str {
        &self.id
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            subcommands: true,
            options: true,
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
        let key = words.join(" ");
        Ok(self
            .levels
            .get(&key)
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

/// xorshift64*: deterministic across platforms and runs.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

fn typo(word: &str, rng: &mut Rng) -> String {
    let mut chars: Vec<char> = word.chars().collect();
    let letters: Vec<char> = "abcdefghijklmnopqrstuvwxyz".chars().collect();
    let at = rng.below(chars.len());
    match rng.below(4) {
        0 if chars.len() > 3 => {
            chars.remove(at);
        }
        1 => chars.insert(at, letters[rng.below(26)]),
        2 => chars[at] = letters[rng.below(26)],
        _ if chars.len() > 1 => {
            let at = at.min(chars.len() - 2);
            chars.swap(at, at + 1);
        }
        _ => chars.insert(at, letters[rng.below(26)]),
    }
    chars.into_iter().collect()
}

struct Case {
    tier: &'static str,
    command: String,
    expected: String,
    status: i32,
}

fn cases(trees: &HashMap<String, Tree>, executables: &[&str]) -> Vec<Case> {
    let mut rng = Rng(0x5eed_1234_abcd_0001);
    let mut cases = Vec::new();
    let mut groups: Vec<(&String, &String, &Vec<String>)> = trees
        .iter()
        .flat_map(|(app, tree)| {
            tree.levels
                .iter()
                .map(move |(ctx, words)| (app, ctx, words))
        })
        .collect();
    groups.sort();
    for (app, context, words) in groups {
        let words: Vec<&String> = words.iter().filter(|w| !w.starts_with('-')).collect();
        let valid: HashSet<&str> = words.iter().map(|w| w.as_str()).collect();
        // About a tenth of each level, at least three words.
        let step = (words.len() / 10).max(1);
        for word in words.iter().step_by(step).take(30) {
            if word.chars().count() < 3 {
                continue;
            }
            let typed = typo(word, &mut rng);
            if valid.contains(typed.as_str()) {
                continue;
            }
            let prefix = if context.is_empty() {
                app.clone()
            } else {
                format!("{app} {context}")
            };
            cases.push(Case {
                tier: "one edit",
                command: format!("{prefix} {typed}"),
                expected: format!("{prefix} {word}"),
                status: 1,
            });
            // Two edits in one longer word.
            let twice = typo(&typo(word, &mut rng), &mut rng);
            if word.chars().count() >= 7 && !valid.contains(twice.as_str()) && twice != **word {
                cases.push(Case {
                    tier: "two edits",
                    command: format!("{prefix} {twice}"),
                    expected: format!("{prefix} {word}"),
                    status: 1,
                });
            }
            // A typo in the group too: `aws ec22 describ-instances`.
            if let Some((group, rest)) = context.split_once(' ').or(Some((context.as_str(), "")))
                && !group.is_empty()
                && group.chars().count() >= 4
            {
                let group_typo = typo(group, &mut rng);
                let top: HashSet<&str> = trees[app.as_str()].levels[""]
                    .iter()
                    .map(String::as_str)
                    .collect();
                if !top.contains(group_typo.as_str()) {
                    let tail = if rest.is_empty() {
                        String::new()
                    } else {
                        format!(" {rest}")
                    };
                    cases.push(Case {
                        tier: "two words",
                        command: format!("{app} {group_typo}{tail} {typed}"),
                        expected: format!("{prefix} {word}"),
                        status: 1,
                    });
                }
            }
        }
    }
    // A missing space after the program: `gitstatus`, `kubectlget`.
    let mut apps: Vec<&String> = trees.keys().collect();
    apps.sort();
    for app in apps {
        let top = &trees[app.as_str()].levels[""];
        for word in top.iter().step_by((top.len() / 8).max(1)).take(8) {
            cases.push(Case {
                tier: "missing space",
                command: format!("{app}{word}"),
                expected: format!("{app} {word}"),
                status: 127,
            });
        }
    }
    let path: HashSet<&str> = executables.iter().copied().collect();
    for name in executables.iter().step_by(6) {
        if name.chars().count() < 3 {
            continue;
        }
        let typed = typo(name, &mut rng);
        if path.contains(typed.as_str()) {
            continue;
        }
        cases.push(Case {
            tier: "program",
            command: format!("{typed} --version"),
            expected: format!("{name} --version"),
            status: 127,
        });
    }
    // A typo in a long option's name: `kubectl get --namspace`.
    let mut levels: Vec<(&String, &String, Vec<&String>)> = trees
        .iter()
        .flat_map(|(app, tree)| {
            tree.levels.iter().map(move |(context, words)| {
                let options = words.iter().filter(|w| w.starts_with("--")).collect();
                (app, context, options)
            })
        })
        .filter(|(_, _, options): &(_, _, Vec<_>)| !options.is_empty())
        .collect();
    levels.sort();
    for (app, context, options) in levels {
        let valid: HashSet<&str> = options.iter().map(|w| w.as_str()).collect();
        for option in options.iter().step_by((options.len() / 10).max(1)).take(30) {
            let name = &option[2..];
            if name.chars().count() < 3 {
                continue;
            }
            let typed = format!("--{}", typo(name, &mut rng));
            if valid.contains(typed.as_str()) {
                continue;
            }
            cases.push(Case {
                tier: "option",
                command: format!("{app} {context} {typed}"),
                expected: format!("{app} {context} {option}"),
                status: 1,
            });
        }
    }
    cases
}

#[derive(Default)]
struct Tally {
    cases: usize,
    top1: usize,
    top3: usize,
    decided: usize,
    decided_wrong: usize,
    asked: usize,
    abstained: usize,
}

impl Tally {
    fn rate(&self, n: usize) -> f64 {
        100.0 * n as f64 / self.cases.max(1) as f64
    }
}

#[test]
fn labeled_corpus_accuracy_and_threshold_calibration() {
    let mut trees: HashMap<String, Tree> = HashMap::new();
    for line in VOCABULARY.lines().chain(OPTIONS.lines()) {
        let mut fields = line.splitn(3, '|');
        let (Some(app), Some(context), Some(word)) = (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        trees
            .entry(app.to_owned())
            .or_insert_with(|| Tree {
                id: app.to_owned(),
                levels: HashMap::new(),
            })
            .levels
            .entry(context.to_owned())
            .or_default()
            .push(word.to_owned());
    }
    let executables: Vec<&str> = EXECUTABLES.lines().filter(|l| !l.is_empty()).collect();
    let cases = cases(&trees, &executables);
    assert!(cases.len() > 300, "{} cases", cases.len());

    let ctx = Context::new(Settings::default(), Shell::Bash, "fuck".into())
        .with_path_listing(&executables)
        .with_which("man", None)
        .with_history::<&str>(&[]);
    let trees: HashMap<String, Rc<Tree>> =
        trees.into_iter().map(|(k, v)| (k, Rc::new(v))).collect();

    // Per case: whether each candidate is the expected one, the scores, and
    // whether the engine held back for reasons besides the thresholds
    // (weak evidence, equally supported alternatives).
    let mut observed: Vec<(Vec<bool>, Vec<f64>, bool)> = Vec::new();
    let mut tally = Tally::default();
    let mut tiers: BTreeMap<&str, Tally> = BTreeMap::new();
    let mut misses: BTreeMap<String, String> = BTreeMap::new();
    for case in &cases {
        let failure = FailureContext {
            source: case.command.clone(),
            exit_status: Some(case.status),
            ..FailureContext::default()
        };
        let backends = trees
            .iter()
            .map(|(app, tree)| (app.clone(), tree.clone() as Rc<dyn NativeCompletionBackend>))
            .collect();
        let report = engine::correct_with_backends(&failure, &ctx, backends);
        let candidates = report.outcome.candidates();
        let hits: Vec<bool> = candidates
            .iter()
            .map(|c| c.script == case.expected)
            .collect();
        if hits.first() != Some(&true) {
            let decided = matches!(report.outcome, Outcome::Suggestion(_));
            misses.insert(
                case.command.clone(),
                format!(
                    "{}{}",
                    candidates.first().map_or("-".into(), |c| c.script.clone()),
                    if decided { "  (DECIDED WRONG)" } else { "" }
                ),
            );
        }
        for t in [&mut tally, tiers.entry(case.tier).or_default()] {
            t.cases += 1;
            if hits.first() == Some(&true) {
                t.top1 += 1;
            }
            if hits.iter().take(3).any(|h| *h) {
                t.top3 += 1;
            }
            match &report.outcome {
                Outcome::Suggestion(_) => {
                    t.decided += 1;
                    if hits.first() != Some(&true) {
                        t.decided_wrong += 1;
                    }
                }
                Outcome::Ambiguous(_) => t.asked += 1,
                _ => t.abstained += 1,
            }
        }
        if std::env::var_os("CORPUS_DEBUG").is_some()
            && case.tier == "program"
            && matches!(report.outcome, Outcome::Ambiguous(_))
        {
            eprintln!(
                "asked: {} -> {:?}",
                case.command,
                candidates
                    .iter()
                    .take(3)
                    .map(|c| format!("{} {:.3}", c.script, c.score))
                    .collect::<Vec<_>>()
            );
        }
        let scores: Vec<f64> = candidates.iter().map(|c| c.score).collect();
        let held = matches!(report.outcome, Outcome::Ambiguous(_)) && ranking::is_decisive(&scores);
        observed.push((hits, scores, held));
    }

    for (name, t) in std::iter::once(("all", &tally)).chain(tiers.iter().map(|(k, v)| (*k, v))) {
        eprintln!(
            "{name:>9}: {:3} cases | top-1 {:5.1}% | top-3 {:5.1}% | decided alone {:5.1}% (wrong {:.2}%) | asked {:4.1}% | abstained {:4.1}%",
            t.cases,
            t.rate(t.top1),
            t.rate(t.top3),
            t.rate(t.decided),
            t.rate(t.decided_wrong),
            t.rate(t.asked),
            t.rate(t.abstained),
        );
    }
    eprintln!("threshold sweep (accept, margin): decided alone / wrong decisions");
    for accept in [0.5, 0.6, 0.7, 0.8] {
        let row: Vec<String> = [0.0, 0.04, 0.08, 0.12, 0.16]
            .iter()
            .map(|&margin| {
                let (mut decided, mut wrong) = (0, 0);
                for (hits, scores, held) in &observed {
                    if !held && ranking::is_decisive_with(scores, accept, margin) {
                        decided += 1;
                        if hits.first() != Some(&true) {
                            wrong += 1;
                        }
                    }
                }
                format!(
                    "m={margin:.2}: {:5.1}%/{:4.2}%",
                    tally.rate(decided),
                    tally.rate(wrong)
                )
            })
            .collect();
        eprintln!("  a={accept:.1}  {}", row.join("  "));
    }
    for (typed, got) in &misses {
        eprintln!("  miss: {typed} -> {got}");
    }

    assert!(tally.rate(tally.top1) >= 85.0, "top-1 accuracy regressed");
    assert!(tally.rate(tally.top3) >= 90.0, "top-3 accuracy regressed");
    assert!(
        tally.rate(tally.decided_wrong) <= 1.0,
        "too many wrong commands would run without a choice"
    );
}
