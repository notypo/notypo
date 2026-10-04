//! Exact port of the parts of Python's `difflib` thefuck relies on:
//! `SequenceMatcher(None, a, b).ratio()` and `get_close_matches`.
//!
//! Results must match CPython bit-for-bit (same float expression, same
//! tie-breaking in `find_longest_match`), so suggestions and their order are
//! identical to the Python version. ASCII inputs run on bytes; anything else
//! is compared by code point, like Python `str`.

use std::cmp::Ordering;

trait Elem: Copy + Eq {}
impl Elem for u8 {}
impl Elem for char {}

/// `SequenceMatcher` with `b` fixed (as in `get_close_matches`). No junk
/// function; `autojunk` popular-element pruning for `len(b) >= 200`.
struct Matcher<'b, T: Elem> {
    b: &'b [T],
    popular: Vec<T>,
    /// Scratch rows for `find_longest_match` (indexed by j + 1).
    row: Vec<u32>,
    row2: Vec<u32>,
}

impl<'b, T: Elem> Matcher<'b, T> {
    fn new(b: &'b [T]) -> Self {
        let mut popular = Vec::new();
        let n = b.len();
        if n >= 200 {
            let ntest = n / 100 + 1;
            let mut seen: Vec<T> = Vec::new();
            for &e in b {
                if !seen.contains(&e) {
                    seen.push(e);
                    if b.iter().filter(|&&x| x == e).count() > ntest {
                        popular.push(e);
                    }
                }
            }
        }
        Matcher {
            b,
            popular,
            row: vec![0; n + 1],
            row2: vec![0; n + 1],
        }
    }

    #[inline]
    fn in_b2j(&self, e: T) -> bool {
        self.popular.is_empty() || !self.popular.contains(&e)
    }

    fn find_longest_match(
        &mut self,
        a: &[T],
        alo: usize,
        ahi: usize,
        blo: usize,
        bhi: usize,
    ) -> (usize, usize, usize) {
        let b = self.b;
        let (mut besti, mut bestj, mut bestsize) = (alo, blo, 0usize);
        // j2len[j] lives at row[j + 1]; both rows start (and are kept) zeroed
        // over [blo, bhi] so `row[j]` reads j2len.get(j - 1, 0).
        for x in &mut self.row[blo..=bhi] {
            *x = 0;
        }
        for x in &mut self.row2[blo..=bhi] {
            *x = 0;
        }
        for (i, &ai) in a.iter().enumerate().take(ahi).skip(alo) {
            let usable = self.in_b2j(ai);
            let (row, new) = (&self.row, &mut self.row2);
            if usable {
                for j in blo..bhi {
                    if b[j] == ai {
                        let k = row[j] + 1;
                        new[j + 1] = k;
                        if k as usize > bestsize {
                            besti = i + 1 - k as usize;
                            bestj = j + 1 - k as usize;
                            bestsize = k as usize;
                        }
                    }
                }
            }
            // j2len = newj2len; then clear the new scratch row.
            std::mem::swap(&mut self.row, &mut self.row2);
            for x in &mut self.row2[blo..=bhi] {
                *x = 0;
            }
        }
        // No junk: these extensions only fire for popular (autojunk) elements.
        while besti > alo && bestj > blo && a[besti - 1] == b[bestj - 1] {
            besti -= 1;
            bestj -= 1;
            bestsize += 1;
        }
        while besti + bestsize < ahi
            && bestj + bestsize < bhi
            && a[besti + bestsize] == b[bestj + bestsize]
        {
            bestsize += 1;
        }
        (besti, bestj, bestsize)
    }

    /// Sum of sizes of `get_matching_blocks()`.
    fn matches(&mut self, a: &[T]) -> usize {
        let mut total = 0;
        let mut stack = vec![(0usize, a.len(), 0usize, self.b.len())];
        while let Some((alo, ahi, blo, bhi)) = stack.pop() {
            let (i, j, k) = self.find_longest_match(a, alo, ahi, blo, bhi);
            if k > 0 {
                total += k;
                if alo < i && blo < j {
                    stack.push((alo, i, blo, j));
                }
                if i + k < ahi && j + k < bhi {
                    stack.push((i + k, ahi, j + k, bhi));
                }
            }
        }
        total
    }

    fn ratio(&mut self, a: &[T]) -> f64 {
        calc_ratio(self.matches(a), a.len() + self.b.len())
    }
}

#[inline]
fn calc_ratio(matches: usize, length: usize) -> f64 {
    if length == 0 {
        1.0
    } else {
        2.0 * matches as f64 / length as f64
    }
}

/// `SequenceMatcher(None, a, b).ratio()`.
pub fn ratio(a: &str, b: &str) -> f64 {
    if a.is_ascii() && b.is_ascii() {
        Matcher::new(b.as_bytes()).ratio(a.as_bytes())
    } else {
        let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
        Matcher::new(&b).ratio(&a)
    }
}

/// Precomputed state for scoring many candidates against one `word`.
pub struct CloseMatcher<'w> {
    word: &'w str,
    word_ascii: bool,
    bytes: Matcher<'w, u8>,
    chars: Option<CharacterCounts>,
    /// Byte histogram of `word` for `quick_ratio` (ASCII path).
    hist: [u16; 256],
    cutoff: f64,
}

struct CharacterCounts {
    word: Vec<char>,
    counts: Vec<(char, u32)>,
}

impl<'w> CloseMatcher<'w> {
    pub fn new(word: &'w str, cutoff: f64) -> Self {
        let mut hist = [0u16; 256];
        for &c in word.as_bytes() {
            hist[c as usize] = hist[c as usize].saturating_add(1);
        }
        CloseMatcher {
            word,
            word_ascii: word.is_ascii(),
            bytes: Matcher::new(word.as_bytes()),
            chars: None,
            hist,
            cutoff,
        }
    }

    /// Returns `Some(ratio)` iff the candidate passes the three cascading
    /// filters of `get_close_matches` (`real_quick_ratio`, `quick_ratio`,
    /// `ratio`), each `>= cutoff`.
    pub fn score(&mut self, cand: &str) -> Option<f64> {
        if self.word_ascii && cand.is_ascii() {
            let (la, lb) = (cand.len(), self.word.len());
            // real_quick_ratio: from lengths alone, rejects most candidates.
            if calc_ratio(la.min(lb), la + lb) < self.cutoff {
                return None;
            }
            // quick_ratio: multiset intersection via a 256-entry histogram.
            let mut avail = self.hist;
            let mut m = 0usize;
            for &c in cand.as_bytes() {
                let slot = &mut avail[c as usize];
                if *slot > 0 {
                    *slot -= 1;
                    m += 1;
                }
            }
            if calc_ratio(m, la + lb) < self.cutoff {
                return None;
            }
            let r = self.bytes.ratio(cand.as_bytes());
            (r >= self.cutoff).then_some(r)
        } else {
            self.score_chars(cand)
        }
    }

    #[cold]
    fn score_chars(&mut self, cand: &str) -> Option<f64> {
        let word = self.word;
        let CharacterCounts { word: wb, counts } = self.chars.get_or_insert_with(|| {
            let wb: Vec<char> = word.chars().collect();
            let mut counts: Vec<(char, u32)> = Vec::new();
            for &c in &wb {
                match counts.iter_mut().find(|(x, _)| *x == c) {
                    Some((_, n)) => *n += 1,
                    None => counts.push((c, 1)),
                }
            }
            CharacterCounts { word: wb, counts }
        });
        let a: Vec<char> = cand.chars().collect();
        let (la, lb) = (a.len(), wb.len());
        if calc_ratio(la.min(lb), la + lb) < self.cutoff {
            return None;
        }
        let mut avail = counts.clone();
        let mut m = 0usize;
        for c in &a {
            if let Some((_, n)) = avail.iter_mut().find(|(x, _)| x == c)
                && *n > 0
            {
                *n -= 1;
                m += 1;
            }
        }
        if calc_ratio(m, la + lb) < self.cutoff {
            return None;
        }
        let r = Matcher::new(wb).ratio(&a);
        (r >= self.cutoff).then_some(r)
    }
}

/// Orders like Python's `heapq.nlargest` on `(score, x)` tuples: score
/// descending, then string descending (code point order == UTF-8 byte order).
#[inline]
pub fn cmp_scored(a: &(f64, &str), b: &(f64, &str)) -> Ordering {
    b.0.partial_cmp(&a.0)
        .unwrap_or(Ordering::Equal)
        .then_with(|| b.1.cmp(a.1))
}

/// All candidates passing `cutoff`, best first. Callers take the first `n`
/// (possibly after extra validation), which equals `get_close_matches`.
pub fn scored_matches<'a>(
    word: &str,
    possibilities: impl IntoIterator<Item = &'a str>,
    cutoff: f64,
) -> Vec<(f64, &'a str)> {
    let mut m = CloseMatcher::new(word, cutoff);
    let mut out: Vec<(f64, &'a str)> = possibilities
        .into_iter()
        .filter_map(|x| m.score(x).map(|r| (r, x)))
        .collect();
    out.sort_by(cmp_scored);
    out
}

/// `difflib.get_close_matches(word, possibilities, n, cutoff)`.
pub fn get_close_matches<'a>(
    word: &str,
    possibilities: impl IntoIterator<Item = &'a str>,
    n: usize,
    cutoff: f64,
) -> Vec<&'a str> {
    let mut v = scored_matches(word, possibilities, cutoff);
    v.truncate(n);
    v.into_iter().map(|(_, x)| x).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ratios_match_cpython() {
        // Values computed with CPython 3.12 difflib.
        assert_eq!(ratio("reset", "st"), 4.0 / 7.0);
        assert_eq!(ratio("status", "st"), 0.5);
        assert_eq!(ratio("fuck", "fuck"), 1.0);
        assert_eq!(ratio("", ""), 1.0);
        assert_eq!(ratio("abcd", "bcde"), 0.75);
        assert_eq!(ratio("fuck", "git status"), 2.0 * 1.0 / 14.0);
        assert_eq!(ratio("qabxcd", "abycdf"), 2.0 * 4.0 / 12.0);
        // Order-sensitive case: SequenceMatcher is not symmetric.
        assert_eq!(ratio("tide", "diet"), 0.25);
        assert_eq!(ratio("diet", "tide"), 0.5);
    }

    #[test]
    fn close_matches_like_thefuck_tests() {
        let st = ["status", "reset", "stage", "stash", "stats"];
        assert_eq!(
            get_close_matches("st", st, 3, 0.1),
            ["stats", "stash", "stage"]
        );
        assert_eq!(
            get_close_matches("tags", ["stage", "tag"], 3, 0.1),
            ["tag", "stage"]
        );
        assert_eq!(
            get_close_matches("brnch", ["branch", "status"], 1, 0.6),
            ["branch"]
        );
        assert!(get_close_matches("st", ["status", "reset"], 1, 0.6).is_empty());
        let exes = ["vim", "fsck", "git", "go", "python"];
        assert_eq!(get_close_matches("vom", exes, 3, 0.6), ["vim"]);
        assert_eq!(get_close_matches("fucck", exes, 3, 0.6), ["fsck"]);
        assert_eq!(get_close_matches("got", exes, 3, 0.6), ["go", "git"]);
        assert_eq!(get_close_matches("gti", exes, 3, 0.6), ["git"]);
        assert!(get_close_matches("qweqwe", exes, 3, 0.6).is_empty());
    }

    #[test]
    fn non_ascii_by_code_point() {
        assert_eq!(ratio("café", "cafe"), 0.75);
        assert_eq!(
            get_close_matches("cafè", ["café", "cafe", "xyz"], 3, 0.6),
            ["café", "cafe"]
        );
    }

    #[test]
    fn autojunk_long_b() {
        // len(b) >= 200 triggers popular-element pruning in CPython.
        let b = "a".repeat(250) + "xyz";
        let r = ratio("aaxyz", &b);
        // CPython: SequenceMatcher(None, "aaxyz", "a"*250 + "xyz").ratio()
        assert_eq!(r, 2.0 * 5.0 / 258.0);
    }
}
