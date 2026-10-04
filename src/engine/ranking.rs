//! Candidate ranking.
//!
//! Scores are ranking signals in `[0, 1]`, not calibrated probabilities. A
//! token score combines spelling similarity (transpositions count as one
//! edit; option names compare without their dashes), first-letter
//! agreement, explicit diagnostic hints, and how often the user typed the
//! replacement before; a command's
//! score is the product of its token scores, so each uncertain repair lowers
//! it. Callers abstain unless the best candidate is strong enough and clearly
//! ahead of the runner-up.

// Calibrated on tests/corpus.rs (934 labeled typos of real aws, gcloud,
// az, git, kubectl, docker, helm, and system command and option names): with
// these values 96.5% are decided alone and none wrongly; a smaller margin starts
// choosing wrong commands, a larger one only asks more often.

/// Minimum score for an automatic first choice.
pub const ACCEPT: f64 = 0.6;
/// Required lead over the second-best candidate.
pub const MARGIN: f64 = 0.08;
/// Token scores below this never become candidates.
pub const FLOOR: f64 = 0.5;

const FIRST_LETTER_BONUS: f64 = 0.05;
const HINT_BONUS: f64 = 0.25;
/// History: a small bonus for any use, growing up to twenty uses.
const HISTORY_BONUS: (f64, f64) = (0.03, 0.07);
const CASE_PENALTY: f64 = 0.02;

/// How a token's role shapes the comparison of spellings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Comparison {
    /// Executables, subcommands, values, and path components.
    Word,
    /// Option names compare after their leading dashes, which mark the
    /// role rather than spell the name: `-x` is not half of `-v`. A
    /// different dash style (`-region` for `--region`) is one edit.
    OptionName,
}

impl Comparison {
    /// The leading dashes set aside before comparing.
    fn dashes(self, word: &str) -> usize {
        match self {
            Comparison::Word => 0,
            Comparison::OptionName => word.bytes().take_while(|&c| c == b'-').count(),
        }
    }

    /// The lowercased characters compared, and the dashes set aside.
    fn split(self, word: &str) -> (Vec<char>, usize) {
        let dashes = self.dashes(word);
        (word[dashes..].to_lowercase().chars().collect(), dashes)
    }
}

/// How a token replacement was scored.
#[derive(Clone, Debug, PartialEq)]
pub struct ScoreBreakdown {
    /// `1 - distance / longer length`, ignoring case.
    pub similarity: f64,
    /// Optimal string alignment distance (adjacent swaps cost one).
    pub distance: usize,
    pub first_letter: f64,
    /// The failed command's own output suggested this replacement.
    pub hint: f64,
    /// The user typed this replacement in the same place before.
    pub history: f64,
    pub case: f64,
    pub total: f64,
}

pub fn score_token(typed: &str, candidate: &str, hinted: bool, uses: u32) -> ScoreBreakdown {
    score_token_as(Comparison::Word, typed, candidate, hinted, uses)
}

pub fn score_token_as(
    comparison: Comparison,
    typed: &str,
    candidate: &str,
    hinted: bool,
    uses: u32,
) -> ScoreBreakdown {
    let (a, dashes_a) = comparison.split(typed);
    let (b, dashes_b) = comparison.split(candidate);
    let distance = osa_chars(&a, &b, usize::MAX, &mut Rows::default())
        .map_or(usize::MAX, |d| d + usize::from(dashes_a != dashes_b));
    breakdown(typed, candidate, &a, &b, distance, hinted, uses)
}

fn breakdown(
    typed: &str,
    candidate: &str,
    a: &[char],
    b: &[char],
    distance: usize,
    hinted: bool,
    uses: u32,
) -> ScoreBreakdown {
    let longest = a.len().max(b.len()).max(1);
    let similarity = 1.0 - distance as f64 / longest as f64;
    let core = |s: &[char]| s.iter().copied().find(|c| *c != '-');
    let first_letter = if core(a).is_some() && core(a) == core(b) {
        FIRST_LETTER_BONUS
    } else {
        0.0
    };
    let hint = if hinted { HINT_BONUS } else { 0.0 };
    let history = if uses > 0 {
        HISTORY_BONUS.0 + HISTORY_BONUS.1 * f64::from(uses.min(20)) / 20.0
    } else {
        0.0
    };
    let case = if typed != candidate && distance == 0 {
        -CASE_PENALTY
    } else {
        0.0
    };
    let total = ((similarity + first_letter + hint + history).min(1.0) + case).max(0.0);
    ScoreBreakdown {
        similarity,
        distance,
        first_letter,
        hint,
        history,
        case,
        total,
    }
}

/// Optimal string alignment distance over characters.
pub fn osa_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    osa_chars(&a, &b, usize::MAX, &mut Rows::default()).unwrap_or(usize::MAX)
}

/// Three rolling rows of the distance table, reused between candidates.
#[derive(Default)]
struct Rows(Vec<usize>, Vec<usize>, Vec<usize>);

/// The OSA distance, or `None` once it must exceed `bound`.
fn osa_chars(a: &[char], b: &[char], bound: usize, rows: &mut Rows) -> Option<usize> {
    if a.len().abs_diff(b.len()) > bound {
        return None;
    }
    let width = b.len() + 1;
    let Rows(before, previous, current) = rows;
    for row in [&mut *before, &mut *previous, &mut *current] {
        row.clear();
        row.resize(width, 0);
    }
    for (j, cell) in previous.iter_mut().enumerate() {
        *cell = j;
    }
    for i in 1..=a.len() {
        current[0] = i;
        let mut row_min = i;
        for j in 1..width {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            let mut best = (previous[j] + 1)
                .min(current[j - 1] + 1)
                .min(previous[j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                best = best.min(before[j - 2] + 1);
            }
            current[j] = best;
            row_min = row_min.min(best);
        }
        if row_min > bound {
            return None;
        }
        std::mem::swap(before, previous);
        std::mem::swap(previous, current);
    }
    Some(previous[b.len()]).filter(|d| *d <= bound)
}

/// The best `limit` replacements for `typed` among `vocabulary`, highest
/// first, ties broken by vocabulary order (the app's own ordering). `uses`
/// says how often the user typed a word in this place before.
pub fn rank_tokens<'v>(
    typed: &str,
    vocabulary: impl IntoIterator<Item = &'v str>,
    hints: &[String],
    uses: &dyn Fn(&str) -> u32,
    limit: usize,
) -> Vec<(&'v str, ScoreBreakdown)> {
    rank_tokens_as(Comparison::Word, typed, vocabulary, hints, uses, limit)
}

/// [`rank_tokens`], comparing spellings as `comparison` says.
pub fn rank_tokens_as<'v>(
    comparison: Comparison,
    typed: &str,
    vocabulary: impl IntoIterator<Item = &'v str>,
    hints: &[String],
    uses: &dyn Fn(&str) -> u32,
    limit: usize,
) -> Vec<(&'v str, ScoreBreakdown)> {
    let (a, dashes_a) = comparison.split(typed);
    let mut b: Vec<char> = Vec::new();
    let mut rows = Rows::default();
    let mut scored: Vec<(usize, &str, ScoreBreakdown)> = Vec::new();
    for (n, word) in vocabulary.into_iter().filter(|w| *w != typed).enumerate() {
        let hinted = hints.iter().any(|h| h == word);
        let used = uses(word);
        let dashes = comparison.dashes(word);
        let restyled = usize::from(dashes != dashes_a);
        // The largest distance that could still reach the floor with every
        // bonus this word can get.
        b.clear();
        b.extend(word[dashes..].chars().flat_map(char::to_lowercase));
        let bonus = FIRST_LETTER_BONUS
            + if hinted { HINT_BONUS } else { 0.0 }
            + if used > 0 {
                HISTORY_BONUS.0 + HISTORY_BONUS.1
            } else {
                0.0
            };
        let longest = a.len().max(b.len()).max(1) as f64;
        let Some(bound) =
            (((1.0 - FLOOR + bonus) * longest).floor() as usize).checked_sub(restyled)
        else {
            continue;
        };
        let Some(distance) = osa_chars(&a, &b, bound, &mut rows) else {
            continue;
        };
        let score = breakdown(typed, word, &a, &b, distance + restyled, hinted, used);
        if score.total >= FLOOR {
            scored.push((n, word, score));
        }
    }
    scored.sort_by(|(na, _, a), (nb, _, b)| b.total.total_cmp(&a.total).then(na.cmp(nb)));
    scored.dedup_by(|(_, a, _), (_, b, _)| a == b);
    scored
        .into_iter()
        .take(limit)
        .map(|(_, word, score)| (word, score))
        .collect()
}

/// Whether the best of `scores` (sorted, highest first) can be chosen
/// without asking: strong enough and clearly ahead of the next one.
pub fn is_decisive(scores: &[f64]) -> bool {
    is_decisive_with(scores, ACCEPT, MARGIN)
}

/// [`is_decisive`] with explicit thresholds, for calibration.
pub fn is_decisive_with(scores: &[f64], accept: f64, margin: f64) -> bool {
    match scores {
        [] => false,
        [best] => *best >= accept,
        [best, second, ..] => *best >= accept && best - second >= margin,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transpositions_count_as_one_edit() {
        assert_eq!(osa_distance("instnaces", "instances"), 1);
        assert_eq!(osa_distance("gti", "git"), 1);
        assert_eq!(osa_distance("--regoin", "--region"), 1);
        assert_eq!(osa_distance("sttus", "status"), 1);
        assert_eq!(osa_distance("", "abc"), 3);
        assert_eq!(osa_distance("λσ", "σλ"), 1);
    }

    #[test]
    fn hints_and_first_letters_raise_scores() {
        let plain = score_token("acount", "account", false, 0);
        let hinted = score_token("acount", "account", true, 0);
        assert!(hinted.total > plain.total);
        assert!(score_token("acount", "account", false, 3).total > plain.total);
        assert!(
            score_token("acount", "account", false, 50).history
                > score_token("acount", "account", false, 1).history
        );
        assert!(plain.total > score_token("acount", "discount", false, 0).total);
        assert!(score_token("Status", "status", false, 0).total < 1.0);
    }

    #[test]
    fn option_names_compare_without_their_dashes() {
        let none = |_: &str| 0;
        assert!(score_token("-x", "-v", false, 0).total >= FLOOR);
        assert!(
            score_token_as(Comparison::OptionName, "-x", "-v", false, 0).total < FLOOR,
            "one wrong letter is the whole name"
        );
        assert!(
            rank_tokens_as(Comparison::OptionName, "-x", ["-v", "-q"], &[], &none, 3).is_empty()
        );
        let restyled = score_token_as(Comparison::OptionName, "-region", "--region", false, 0);
        assert_eq!(restyled.distance, 1, "a different dash style is one edit");
        assert_eq!(restyled.case, 0.0);
        assert!(restyled.total >= ACCEPT);
        let swapped = score_token_as(Comparison::OptionName, "--regoin", "--region", false, 0);
        assert!(swapped.similarity < score_token("--regoin", "--region", false, 0).similarity);
        assert!(swapped.total >= ACCEPT);
        let ranked = rank_tokens_as(
            Comparison::OptionName,
            "--regoin",
            ["--profile", "-r", "--region", "region"],
            &[],
            &none,
            3,
        );
        assert_eq!(ranked[0].0, "--region");
        assert!(ranked.iter().all(|(word, _)| *word != "-r"));
        assert!(
            score_token_as(Comparison::OptionName, "--Region", "--region", false, 0).case < 0.0
        );
    }

    #[test]
    fn ranking_prefers_small_edits_and_keeps_app_order_for_ties() {
        let vocabulary = [
            "describe-instance-status",
            "describe-instances",
            "run-instances",
        ];
        let none = |_: &str| 0;
        let ranked = rank_tokens("describ-instances", vocabulary, &[], &none, 3);
        assert_eq!(ranked[0].0, "describe-instances");
        assert!(is_decisive(&[ranked[0].1.total, ranked[1].1.total]));
        let ties = rank_tokens("ab", ["ac", "ad"], &[], &none, 3);
        let used = rank_tokens("ab", ["ac", "ad"], &[], &|w: &str| u32::from(w == "ad"), 3);
        assert_eq!(used[0].0, "ad", "history breaks the tie");
        assert_eq!(ties[0].0, "ac");
        assert!(!is_decisive(&[ties[0].1.total, ties[1].1.total]));
        assert!(rank_tokens("zzzz", vocabulary, &[], &none, 3).is_empty());
    }
}
