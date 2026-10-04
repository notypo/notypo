//! Candidate ranking.
//!
//! Scores are ranking signals in `[0, 1]`, not calibrated probabilities. A
//! token score combines spelling similarity (transpositions count as one
//! edit), first-letter agreement, and explicit diagnostic hints; a command's
//! score is the product of its token scores, so each uncertain repair lowers
//! it. Callers abstain unless the best candidate is strong enough and clearly
//! ahead of the runner-up.

/// Minimum score for an automatic first choice.
pub const ACCEPT: f64 = 0.6;
/// Required lead over the second-best candidate.
pub const MARGIN: f64 = 0.08;
/// Token scores below this never become candidates.
pub const FLOOR: f64 = 0.5;

const FIRST_LETTER_BONUS: f64 = 0.05;
const HINT_BONUS: f64 = 0.25;
const CASE_PENALTY: f64 = 0.02;

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
    pub case: f64,
    pub total: f64,
}

pub fn score_token(typed: &str, candidate: &str, hinted: bool) -> ScoreBreakdown {
    let (a, b) = (typed.to_lowercase(), candidate.to_lowercase());
    let a_core = a.trim_start_matches('-');
    let b_core = b.trim_start_matches('-');
    let distance = osa_distance(&a, &b);
    let longest = a.chars().count().max(b.chars().count()).max(1);
    let similarity = 1.0 - distance as f64 / longest as f64;
    let first_letter = if !a_core.is_empty() && a_core.chars().next() == b_core.chars().next() {
        FIRST_LETTER_BONUS
    } else {
        0.0
    };
    let hint = if hinted { HINT_BONUS } else { 0.0 };
    let case = if typed != candidate && a == b {
        -CASE_PENALTY
    } else {
        0.0
    };
    let total = ((similarity + first_letter + hint).min(1.0) + case).max(0.0);
    ScoreBreakdown {
        similarity,
        distance,
        first_letter,
        hint,
        case,
        total,
    }
}

/// Optimal string alignment distance over characters.
pub fn osa_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let width = b.len() + 1;
    let mut d = vec![0usize; (a.len() + 1) * width];
    for i in 0..=a.len() {
        d[i * width] = i;
    }
    for (j, cell) in d.iter_mut().enumerate().take(width) {
        *cell = j;
    }
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            let mut best = (d[(i - 1) * width + j] + 1)
                .min(d[i * width + j - 1] + 1)
                .min(d[(i - 1) * width + j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                best = best.min(d[(i - 2) * width + j - 2] + 1);
            }
            d[i * width + j] = best;
        }
    }
    d[a.len() * width + b.len()]
}

/// The best `limit` replacements for `typed` among `vocabulary`, highest
/// first, ties broken by vocabulary order (the app's own ordering).
pub fn rank_tokens<'v>(
    typed: &str,
    vocabulary: impl IntoIterator<Item = &'v str>,
    hints: &[String],
    limit: usize,
) -> Vec<(&'v str, ScoreBreakdown)> {
    let mut scored: Vec<(usize, &str, ScoreBreakdown)> = vocabulary
        .into_iter()
        .filter(|word| *word != typed)
        .enumerate()
        .map(|(n, word)| {
            let hinted = hints.iter().any(|h| h == word);
            (n, word, score_token(typed, word, hinted))
        })
        .filter(|(_, _, score)| score.total >= FLOOR)
        .collect();
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
    match scores {
        [] => false,
        [best] => *best >= ACCEPT,
        [best, second, ..] => *best >= ACCEPT && best - second >= MARGIN,
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
        let plain = score_token("acount", "account", false);
        let hinted = score_token("acount", "account", true);
        assert!(hinted.total > plain.total);
        assert!(plain.total > score_token("acount", "discount", false).total);
        assert!(score_token("Status", "status", false).total < 1.0);
    }

    #[test]
    fn ranking_prefers_small_edits_and_keeps_app_order_for_ties() {
        let vocabulary = [
            "describe-instance-status",
            "describe-instances",
            "run-instances",
        ];
        let ranked = rank_tokens("describ-instances", vocabulary, &[], 3);
        assert_eq!(ranked[0].0, "describe-instances");
        assert!(is_decisive(&[ranked[0].1.total, ranked[1].1.total]));
        let ties = rank_tokens("ab", ["ac", "ad"], &[], 3);
        assert_eq!(ties[0].0, "ac");
        assert!(!is_decisive(&[ties[0].1.total, ties[1].1.total]));
        assert!(rank_tokens("zzzz", vocabulary, &[], 3).is_empty());
    }
}
