//! POSIX `shlex.split` / `shlex.quote`, zero-copy: a token that needed no
//! unquoting is a slice of the input, only rewritten tokens allocate.

use std::borrow::Cow;

#[derive(Debug, PartialEq, Eq)]
pub enum SplitError {
    NoClosingQuotation,
    NoEscapedCharacter,
}

#[inline]
fn is_ws(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\r' | b'\n')
}

/// `shlex.split(s)` (posix=True, comments=False).
pub fn split(s: &str) -> Result<Vec<Cow<'_, str>>, SplitError> {
    split_impl(s, false)
}

/// `Generic.split_command` from thefuck: like `shlex.split`, but a
/// backslash-escaped space outside quotes keeps its backslash (`a\ b` stays
/// one token, `a\ b`); on a parse error it falls back to `str.split(' ')`.
pub fn split_command(s: &str) -> Vec<Cow<'_, str>> {
    split_impl(s, true).unwrap_or_else(|_| s.split(' ').map(Cow::Borrowed).collect())
}

struct Tok<'a> {
    src: &'a str,
    start: usize,
    end: usize,
    owned: Option<String>,
}

impl<'a> Tok<'a> {
    #[inline]
    fn own(&mut self) -> &mut String {
        let (src, start, end) = (self.src, self.start, self.end);
        self.owned.get_or_insert_with(|| src[start..end].to_owned())
    }
    /// Appends `src[from..to]` (the literal text right where we are).
    #[inline]
    fn push_lit(&mut self, from: usize, to: usize) {
        match &mut self.owned {
            Some(s) => s.push_str(&self.src[from..to]),
            None if self.end == from => self.end = to,
            None => {
                let src = self.src;
                self.own().push_str(&src[from..to]);
            }
        }
    }
    fn finish(self) -> Cow<'a, str> {
        match self.owned {
            Some(s) => Cow::Owned(s),
            None => Cow::Borrowed(&self.src[self.start..self.end]),
        }
    }
}

fn split_impl(s: &str, keep_escaped_space: bool) -> Result<Vec<Cow<'_, str>>, SplitError> {
    let b = s.as_bytes();
    let n = b.len();
    let mut out = Vec::new();
    let mut i = 0;
    loop {
        while i < n && is_ws(b[i]) {
            i += 1;
        }
        if i >= n {
            return Ok(out);
        }
        let mut tok = Tok {
            src: s,
            start: i,
            end: i,
            owned: None,
        };
        // Read one token.
        while i < n {
            let c = b[i];
            if is_ws(c) {
                break;
            }
            match c {
                b'\'' => {
                    tok.own();
                    let close = b[i + 1..]
                        .iter()
                        .position(|&c| c == b'\'')
                        .ok_or(SplitError::NoClosingQuotation)?
                        + i
                        + 1;
                    tok.push_lit(i + 1, close);
                    i = close + 1;
                }
                b'"' => {
                    tok.own();
                    i += 1;
                    loop {
                        if i >= n {
                            return Err(SplitError::NoClosingQuotation);
                        }
                        match b[i] {
                            b'"' => {
                                i += 1;
                                break;
                            }
                            b'\\' => {
                                if i + 1 >= n {
                                    return Err(SplitError::NoClosingQuotation);
                                }
                                let nx = b[i + 1];
                                if nx == b'"' || nx == b'\\' {
                                    tok.push_lit(i + 1, i + 2);
                                    i += 2;
                                } else {
                                    // Backslash stays literal inside "..."
                                    tok.push_lit(i, i + 1);
                                    i += 1;
                                }
                            }
                            _ => {
                                let j = b[i..]
                                    .iter()
                                    .position(|&c| c == b'"' || c == b'\\')
                                    .map_or(n, |k| i + k);
                                tok.push_lit(i, j);
                                i = j;
                            }
                        }
                    }
                }
                b'\\' => {
                    if i + 1 >= n {
                        return Err(SplitError::NoEscapedCharacter);
                    }
                    let ch_len = utf8_len(b[i + 1]);
                    if keep_escaped_space && b[i + 1] == b' ' {
                        tok.push_lit(i, i + 2);
                    } else {
                        tok.own();
                        tok.push_lit(i + 1, (i + 1 + ch_len).min(n));
                    }
                    i = (i + 1 + ch_len).min(n);
                }
                _ => {
                    let mut j = i + 1;
                    while j < n && !is_ws(b[j]) && !matches!(b[j], b'\'' | b'"' | b'\\') {
                        j += 1;
                    }
                    tok.push_lit(i, j);
                    i = j;
                }
            }
        }
        out.push(tok.finish());
    }
}

#[inline]
fn utf8_len(first: u8) -> usize {
    match first {
        0x00..=0x7F => 1,
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xF7 => 4,
        _ => 1,
    }
}

/// `shlex.quote(s)`.
pub fn quote(s: &str) -> Cow<'_, str> {
    if s.is_empty() {
        return Cow::Borrowed("''");
    }
    let safe = s.bytes().all(|c| {
        c.is_ascii_alphanumeric()
            || matches!(
                c,
                b'_' | b'@' | b'%' | b'+' | b'=' | b':' | b',' | b'.' | b'/' | b'-'
            )
    });
    if safe {
        return Cow::Borrowed(s);
    }
    let mut q = String::with_capacity(s.len() + 2);
    q.push('\'');
    q.push_str(&s.replace('\'', "'\"'\"'"));
    q.push('\'');
    Cow::Owned(q)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sp(s: &str) -> Vec<String> {
        split(s)
            .unwrap()
            .into_iter()
            .map(|c| c.into_owned())
            .collect()
    }

    #[test]
    fn split_like_python() {
        assert_eq!(
            sp("git commit -m \"new commit\""),
            ["git", "commit", "-m", "new commit"]
        );
        assert_eq!(sp("  a  b\tc\n"), ["a", "b", "c"]);
        assert_eq!(sp("a'b c'd"), ["ab cd"]);
        assert_eq!(sp("''"), [""]);
        assert_eq!(sp("echo \"a\\\"b\\\\c\\d\""), ["echo", "a\"b\\c\\d"]);
        assert_eq!(sp("a\\ b"), ["a b"]);
        assert_eq!(sp("'it'\"'\"'s'"), ["it's"]);
        assert_eq!(sp("ünï 'cødé'"), ["ünï", "cødé"]);
        assert_eq!(split("'abc"), Err(SplitError::NoClosingQuotation));
        assert_eq!(split("abc\\"), Err(SplitError::NoEscapedCharacter));
    }

    #[test]
    fn split_is_zero_copy_for_plain_words() {
        let parts = split("git push origin").unwrap();
        assert!(parts.iter().all(|p| matches!(p, Cow::Borrowed(_))));
    }

    #[test]
    fn split_command_semantics() {
        let v: Vec<_> = split_command("ls a\\ b 'c d'")
            .into_iter()
            .map(|c| c.into_owned())
            .collect();
        assert_eq!(v, ["ls", "a\\ b", "c d"]);
        let v: Vec<_> = split_command("git  \"x")
            .into_iter()
            .map(|c| c.into_owned())
            .collect();
        assert_eq!(v, ["git", "", "\"x"]);
    }

    #[test]
    fn quote_like_python() {
        assert_eq!(quote(""), "''");
        assert_eq!(quote("abc-1.2/x=y"), "abc-1.2/x=y");
        assert_eq!(quote("a b"), "'a b'");
        assert_eq!(quote("it's"), "'it'\"'\"'s'");
    }
}
