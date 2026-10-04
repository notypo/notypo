//! Lossless parsing of POSIX-family command lines (sh, bash, zsh).
//!
//! Nothing is evaluated: parameter expansions, command and process
//! substitutions, arithmetic, globs, braces, and tildes make a word opaque
//! instead of being expanded. Every word, redirection, and comment keeps the
//! byte span it came from, so an edit replaces exactly those bytes and the
//! rest of the line stays byte-for-byte intact.
//!
//! Supported structure: simple commands (assignments, words, redirections)
//! joined into pipelines (`|`, `|&`) and lists (`&&`, `||`, `;`, `&`, and
//! newlines). Compound commands, groups, and here-documents are reported as
//! [`Diagnostic`]s next to the partial tree; callers must not edit them.

use crate::shlex;

/// A byte range of the source.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub const fn new(start: usize, end: usize) -> Span {
        Span { start, end }
    }

    pub fn of(self, src: &str) -> &str {
        &src[self.start..self.end]
    }

    pub fn overlaps(self, other: Span) -> bool {
        self.start < other.end && other.start < self.end
    }

    pub fn contains(self, other: Span) -> bool {
        self.start <= other.start && other.end <= self.end
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Word {
    pub span: Span,
    /// The word after quote removal, when nothing in it would be expanded.
    pub value: Option<String>,
    /// Some part of the word was quoted or escaped.
    pub quoted: bool,
}

impl Word {
    pub fn literal(&self) -> Option<&str> {
        self.value.as_deref()
    }

    /// The span of the name in an option word written without quotes, such
    /// as `--color` in `--color=auto`; the value part keeps its bytes.
    pub fn option_name_span(&self, src: &str) -> Option<Span> {
        let raw = self.span.of(src);
        let end = raw.find('=').unwrap_or(raw.len());
        let name = &raw[..end];
        (name.len() > 1
            && name.starts_with('-')
            && name
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.')))
        .then(|| Span::new(self.span.start, self.span.start + end))
    }
}

/// The operator joining a command to the one before it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Connector {
    Pipe,
    /// `|&`: stdout and stderr.
    PipeAll,
    And,
    Or,
    Sequence,
    Background,
    Newline,
}

impl Connector {
    pub fn is_pipe(self) -> bool {
        matches!(self, Connector::Pipe | Connector::PipeAll)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Redirection {
    pub span: Span,
    /// The operator including any file descriptor, such as `2>&` or `>>`.
    pub operator: Span,
    pub target: Option<Word>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SimpleCommand {
    pub span: Span,
    pub assignments: Vec<Word>,
    pub words: Vec<Word>,
    pub redirections: Vec<Redirection>,
    /// The operator before this command; `None` for the first one.
    pub connector: Option<Connector>,
}

impl SimpleCommand {
    fn is_empty(&self) -> bool {
        self.assignments.is_empty() && self.words.is_empty() && self.redirections.is_empty()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiagnosticKind {
    UnterminatedQuote,
    UnterminatedSubstitution,
    TrailingEscape,
    HereDocument,
    /// `if`, `for`, `case`, functions, subshells, and `{ ...; }` groups.
    CompoundCommand,
    MissingRedirectionTarget,
    /// An operator with no command on one side, such as `ls &&`.
    UnexpectedOperator,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub span: Span,
    pub kind: DiagnosticKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Script {
    pub source: String,
    pub commands: Vec<SimpleCommand>,
    pub comments: Vec<Span>,
    pub diagnostics: Vec<Diagnostic>,
}

impl Script {
    /// Every construct was understood, so edits have an unambiguous meaning.
    pub fn is_fully_supported(&self) -> bool {
        self.diagnostics.is_empty()
    }

    /// Commands that run as one pipeline with `index`.
    pub fn pipeline_of(&self, index: usize) -> std::ops::Range<usize> {
        let mut start = index;
        while start > 0
            && self.commands[start]
                .connector
                .is_some_and(Connector::is_pipe)
        {
            start -= 1;
        }
        let mut end = index + 1;
        while end < self.commands.len()
            && self.commands[end].connector.is_some_and(Connector::is_pipe)
        {
            end += 1;
        }
        start..end
    }
}

/// Words that start compound commands when they appear in command position.
const RESERVED: &[&str] = &[
    "!", "[[", "]]", "{", "}", "case", "coproc", "do", "done", "elif", "else", "esac", "fi", "for",
    "function", "if", "in", "select", "then", "until", "while",
];

pub fn parse(source: &str) -> Script {
    let mut parser = Parser {
        src: source,
        b: source.as_bytes(),
        i: 0,
        commands: Vec::new(),
        comments: Vec::new(),
        diagnostics: Vec::new(),
        current: SimpleCommand::default(),
        connector: None,
    };
    parser.run();
    Script {
        source: source.to_owned(),
        commands: parser.commands,
        comments: parser.comments,
        diagnostics: parser.diagnostics,
    }
}

struct Parser<'a> {
    src: &'a str,
    b: &'a [u8],
    i: usize,
    commands: Vec<SimpleCommand>,
    comments: Vec<Span>,
    diagnostics: Vec<Diagnostic>,
    current: SimpleCommand,
    /// The operator that ended the previous command.
    connector: Option<(Connector, Span)>,
}

fn is_blank(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\r')
}

fn is_meta(c: u8) -> bool {
    is_blank(c) || matches!(c, b'\n' | b';' | b'&' | b'|' | b'(' | b')' | b'<' | b'>')
}

impl Parser<'_> {
    fn peek(&self, k: usize) -> Option<u8> {
        self.b.get(self.i + k).copied()
    }

    fn diagnose(&mut self, start: usize, end: usize, kind: DiagnosticKind) {
        self.diagnostics.push(Diagnostic {
            span: Span::new(start, end),
            kind,
        });
    }

    fn run(&mut self) {
        while self.i < self.b.len() {
            let c = self.b[self.i];
            let start = self.i;
            match c {
                c if is_blank(c) => self.i += 1,
                b'\\' if self.peek(1) == Some(b'\n') => self.i += 2,
                b'#' => {
                    let end = self.src[start..]
                        .find('\n')
                        .map_or(self.b.len(), |k| start + k);
                    self.comments.push(Span::new(start, end));
                    self.i = end;
                }
                b'\n' => self.end_command(Connector::Newline, 1),
                b';' if self.peek(1) == Some(b';') => {
                    self.diagnose(start, start + 2, DiagnosticKind::CompoundCommand);
                    self.i += 2;
                }
                b';' => self.end_command(Connector::Sequence, 1),
                b'&' if self.peek(1) == Some(b'&') => self.end_command(Connector::And, 2),
                b'&' if self.peek(1) == Some(b'>') => self.redirection(start),
                b'&' => self.end_command(Connector::Background, 1),
                b'|' if self.peek(1) == Some(b'|') => self.end_command(Connector::Or, 2),
                b'|' if self.peek(1) == Some(b'&') => self.end_command(Connector::PipeAll, 2),
                b'|' => self.end_command(Connector::Pipe, 1),
                b'(' | b')' => {
                    self.diagnose(start, start + 1, DiagnosticKind::CompoundCommand);
                    self.i += 1;
                }
                b'<' | b'>' if self.peek(1) == Some(b'(') => self.word(),
                b'<' | b'>' => self.redirection(start),
                b'0'..=b'9' if self.fd_redirection_ahead() => self.redirection(start),
                _ => self.word(),
            }
        }
        self.finish();
    }

    /// `2>file`: digits immediately followed by a redirection operator.
    fn fd_redirection_ahead(&self) -> bool {
        let digits = self.b[self.i..]
            .iter()
            .take_while(|c| c.is_ascii_digit())
            .count();
        matches!(self.peek(digits), Some(b'<' | b'>'))
    }

    fn end_command(&mut self, connector: Connector, len: usize) {
        let span = Span::new(self.i, self.i + len);
        self.i += len;
        let empty = self.current.is_empty();
        if empty && connector != Connector::Newline {
            self.diagnose(span.start, span.end, DiagnosticKind::UnexpectedOperator);
        }
        // An empty line keeps the pending operator: `ls &&<newline>ls`.
        if !empty {
            self.push_current();
            self.connector = Some((connector, span));
        }
    }

    fn push_current(&mut self) {
        let mut command = std::mem::take(&mut self.current);
        let first = command
            .assignments
            .first()
            .map(|w| w.span)
            .into_iter()
            .chain(command.words.first().map(|w| w.span))
            .chain(command.redirections.first().map(|r| r.span))
            .min();
        let last = command
            .assignments
            .last()
            .map(|w| w.span)
            .into_iter()
            .chain(command.words.last().map(|w| w.span))
            .chain(command.redirections.last().map(|r| r.span))
            .max_by_key(|s| s.end);
        if let (Some(first), Some(last)) = (first, last) {
            command.span = Span::new(first.start, last.end);
        }
        command.connector = self.connector.take().map(|(c, _)| c);
        self.commands.push(command);
    }

    fn finish(&mut self) {
        if !self.current.is_empty() {
            self.push_current();
        } else if let Some((connector, span)) = self.connector
            && !matches!(
                connector,
                Connector::Sequence | Connector::Background | Connector::Newline
            )
        {
            self.diagnose(span.start, span.end, DiagnosticKind::UnexpectedOperator);
        }
    }

    fn word(&mut self) {
        let word = self.scan_word();
        let in_command_position = self.current.words.is_empty();
        if in_command_position {
            if is_assignment(&word, self.src) {
                self.current.assignments.push(word);
                return;
            }
            if word
                .literal()
                .is_some_and(|w| !word.quoted && RESERVED.contains(&w))
            {
                self.diagnose(
                    word.span.start,
                    word.span.end,
                    DiagnosticKind::CompoundCommand,
                );
            }
        }
        self.current.words.push(word);
    }

    fn redirection(&mut self, start: usize) {
        while self.peek(0).is_some_and(|c| c.is_ascii_digit()) {
            self.i += 1;
        }
        let rest = &self.src[self.i..];
        let operator = [
            "&>>", "&>", "<<<", "<<-", "<<", "<&", "<>", "<", ">>", ">&", ">|", ">",
        ]
        .into_iter()
        .find(|op| rest.starts_with(op))
        .unwrap_or("<");
        self.i += operator.len();
        let operator_span = Span::new(start, self.i);
        if matches!(operator, "<<" | "<<-") {
            self.diagnose(start, self.i, DiagnosticKind::HereDocument);
        }
        while self.peek(0).is_some_and(is_blank) {
            self.i += 1;
        }
        let starts_word = match self.peek(0) {
            Some(b'<' | b'>') => self.peek(1) == Some(b'('),
            Some(c) => !is_meta(c),
            None => false,
        };
        let target = if starts_word {
            Some(self.scan_word())
        } else {
            self.diagnose(start, self.i, DiagnosticKind::MissingRedirectionTarget);
            None
        };
        let end = target.as_ref().map_or(self.i, |t| t.span.end);
        self.current.redirections.push(Redirection {
            span: Span::new(start, end),
            operator: operator_span,
            target,
        });
    }

    fn scan_word(&mut self) -> Word {
        let start = self.i;
        let mut value = Some(String::new());
        let mut quoted = false;
        let push = |value: &mut Option<String>, s: &str| {
            if let Some(v) = value {
                v.push_str(s);
            }
        };
        while let Some(c) = self.peek(0) {
            match c {
                b'<' | b'>' if self.i == start && self.peek(1) == Some(b'(') => {
                    self.i += 1;
                    self.substitution(b'(', b')');
                    value = None;
                }
                c if is_meta(c) => break,
                b'\\' => match self.peek(1) {
                    None => {
                        self.diagnose(self.i, self.i + 1, DiagnosticKind::TrailingEscape);
                        self.i += 1;
                    }
                    Some(b'\n') => self.i += 2,
                    Some(_) => {
                        let ch = self.src[self.i + 1..].chars().next().unwrap_or('\\');
                        push(
                            &mut value,
                            &self.src[self.i + 1..self.i + 1 + ch.len_utf8()],
                        );
                        self.i += 1 + ch.len_utf8();
                        quoted = true;
                    }
                },
                b'\'' => {
                    quoted = true;
                    let open = self.i;
                    match self.src[open + 1..].find('\'') {
                        Some(k) => {
                            push(&mut value, &self.src[open + 1..open + 1 + k]);
                            self.i = open + k + 2;
                        }
                        None => {
                            self.diagnose(open, self.b.len(), DiagnosticKind::UnterminatedQuote);
                            self.i = self.b.len();
                            value = None;
                        }
                    }
                }
                b'"' => {
                    quoted = true;
                    self.i += 1;
                    self.double_quoted(&mut value);
                }
                b'$' => match self.dollar(&mut quoted) {
                    Dollar::Literal => {
                        push(&mut value, "$");
                        self.i += 1;
                    }
                    // `$"..."` is translated text; the quotes are scanned next.
                    Dollar::Translated => self.i += 1,
                    Dollar::Expansion => value = None,
                },
                b'`' => {
                    self.backticks();
                    value = None;
                }
                b'*' | b'?' => {
                    self.i += 1;
                    value = None;
                }
                b'[' if self.word_has_unquoted(b']') => {
                    self.i += 1;
                    value = None;
                }
                b'{' if self.is_brace_expansion() => {
                    self.i += 1;
                    value = None;
                }
                b'~' if self.i == start => {
                    self.i += 1;
                    value = None;
                }
                _ => {
                    let run = self.b[self.i..]
                        .iter()
                        .position(|&c| {
                            is_meta(c)
                                || matches!(
                                    c,
                                    b'\\' | b'\'' | b'"' | b'$' | b'`' | b'*' | b'?' | b'[' | b'{'
                                )
                        })
                        .map_or(self.b.len(), |k| self.i + k)
                        .max(self.i + 1);
                    let run = (run..=self.b.len())
                        .find(|&k| self.src.is_char_boundary(k))
                        .unwrap_or(self.b.len());
                    push(&mut value, &self.src[self.i..run]);
                    self.i = run;
                }
            }
        }
        Word {
            span: Span::new(start, self.i),
            value,
            quoted,
        }
    }

    /// Scans `"..."` from just after the opening quote.
    fn double_quoted(&mut self, value: &mut Option<String>) {
        let open = self.i - 1;
        loop {
            let Some(c) = self.peek(0) else {
                self.diagnose(open, self.b.len(), DiagnosticKind::UnterminatedQuote);
                *value = None;
                return;
            };
            match c {
                b'"' => {
                    self.i += 1;
                    return;
                }
                b'\\' => match self.peek(1) {
                    Some(b'\n') => self.i += 2,
                    Some(e @ (b'$' | b'`' | b'"' | b'\\')) => {
                        if let Some(v) = value {
                            v.push(e as char);
                        }
                        self.i += 2;
                    }
                    _ => {
                        if let Some(v) = value {
                            v.push('\\');
                        }
                        self.i += 1;
                    }
                },
                b'$' => {
                    let mut quoted = true;
                    if self.dollar(&mut quoted) == Dollar::Expansion {
                        *value = None;
                    } else {
                        if let Some(v) = value {
                            v.push('$');
                        }
                        self.i += 1;
                    }
                }
                b'`' => {
                    self.backticks();
                    *value = None;
                }
                _ => {
                    let run = self.b[self.i..]
                        .iter()
                        .position(|&c| matches!(c, b'"' | b'\\' | b'$' | b'`'))
                        .map_or(self.b.len(), |k| self.i + k);
                    if let Some(v) = value {
                        v.push_str(&self.src[self.i..run]);
                    }
                    self.i = run;
                }
            }
        }
    }

    /// Skips an expansion starting at `$`; leaves a literal `$` in place.
    fn dollar(&mut self, quoted: &mut bool) -> Dollar {
        match self.peek(1) {
            Some(b'(') => {
                self.i += 1;
                self.substitution(b'(', b')');
            }
            Some(b'{') => {
                self.i += 1;
                self.substitution(b'{', b'}');
            }
            Some(b'\'') => {
                // ANSI-C quoting: we don't decode it, so it stays opaque.
                *quoted = true;
                let open = self.i;
                self.i += 2;
                loop {
                    match self.peek(0) {
                        None => {
                            self.diagnose(open, self.b.len(), DiagnosticKind::UnterminatedQuote);
                            break;
                        }
                        Some(b'\\') => self.i += 2,
                        Some(b'\'') => {
                            self.i += 1;
                            break;
                        }
                        Some(_) => self.i += 1,
                    }
                }
                self.i = self.i.min(self.b.len());
            }
            Some(b'"') => return Dollar::Translated,
            Some(c) if c.is_ascii_alphabetic() || c == b'_' => {
                self.i += 1;
                while self
                    .peek(0)
                    .is_some_and(|c| c.is_ascii_alphanumeric() || c == b'_')
                {
                    self.i += 1;
                }
            }
            Some(c) if c.is_ascii_digit() || b"@*#?$!-".contains(&c) => self.i += 2,
            _ => return Dollar::Literal,
        }
        Dollar::Expansion
    }

    /// Skips a balanced `open ... close` region starting at `open`.
    fn substitution(&mut self, open: u8, close: u8) {
        let start = self.i;
        let mut depth = 0usize;
        while let Some(c) = self.peek(0) {
            match c {
                b'\\' => {
                    self.i += 2;
                    continue;
                }
                b'\'' => match self.src[self.i + 1..].find('\'') {
                    Some(k) => self.i += k + 1,
                    None => break,
                },
                b'"' => {
                    self.i += 1;
                    let mut ignored = None;
                    self.double_quoted(&mut ignored);
                    continue;
                }
                b'`' => {
                    self.backticks();
                    continue;
                }
                c if c == open => depth += 1,
                c if c == close => {
                    depth -= 1;
                    if depth == 0 {
                        self.i += 1;
                        return;
                    }
                }
                _ => {}
            }
            self.i += 1;
        }
        self.i = self.b.len();
        self.diagnose(
            start,
            self.b.len(),
            DiagnosticKind::UnterminatedSubstitution,
        );
    }

    fn backticks(&mut self) {
        let open = self.i;
        self.i += 1;
        while let Some(c) = self.peek(0) {
            match c {
                b'\\' => self.i += 2,
                b'`' => {
                    self.i += 1;
                    return;
                }
                _ => self.i += 1,
            }
        }
        self.i = self.b.len();
        self.diagnose(open, self.b.len(), DiagnosticKind::UnterminatedSubstitution);
    }

    /// Whether `c` appears unquoted later in the current word.
    fn word_has_unquoted(&self, c: u8) -> bool {
        self.b[self.i + 1..]
            .iter()
            .take_while(|&&x| !is_meta(x) && !matches!(x, b'\'' | b'"' | b'\\'))
            .any(|&x| x == c)
    }

    /// `{a,b}` or `{1..3}` in the rest of the current word.
    fn is_brace_expansion(&self) -> bool {
        let rest: Vec<u8> = self.b[self.i + 1..]
            .iter()
            .copied()
            .take_while(|&x| !is_meta(x) && !matches!(x, b'\'' | b'"' | b'\\'))
            .collect();
        rest.iter().position(|&x| x == b'}').is_some_and(|close| {
            let body = &rest[..close];
            body.contains(&b',') || body.windows(2).any(|w| w == b"..")
        })
    }
}

#[derive(PartialEq, Eq)]
enum Dollar {
    Literal,
    Expansion,
    /// `$"..."`: locale-translated text whose fallback is the literal.
    Translated,
}

fn is_assignment(word: &Word, src: &str) -> bool {
    let raw = word.span.of(src);
    let Some(eq) = raw.find('=') else {
        return false;
    };
    let name = raw[..eq].strip_suffix('+').unwrap_or(&raw[..eq]);
    !name.is_empty()
        && !name.as_bytes()[0].is_ascii_digit()
        && name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
}

/// Text that replaces a word so the shell reads it as exactly `value`.
pub fn quote_word(value: &str) -> String {
    shlex::quote(value).into_owned()
}

/// A replacement of the bytes in `span`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit {
    pub span: Span,
    pub replacement: String,
}

/// Applies non-overlapping edits; `None` if they overlap or split a character.
pub fn apply_edits(source: &str, edits: &[Edit]) -> Option<String> {
    let mut sorted: Vec<&Edit> = edits.iter().collect();
    sorted.sort_by_key(|e| (e.span.start, e.span.end));
    let mut out = String::with_capacity(source.len());
    let mut at = 0;
    for edit in sorted {
        let Span { start, end } = edit.span;
        if start < at
            || end > source.len()
            || start > end
            || !source.is_char_boundary(start)
            || !source.is_char_boundary(end)
        {
            return None;
        }
        out.push_str(&source[at..start]);
        out.push_str(&edit.replacement);
        at = end;
    }
    out.push_str(&source[at..]);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(script: &Script, command: usize) -> Vec<Option<&str>> {
        script.commands[command]
            .words
            .iter()
            .map(Word::literal)
            .collect()
    }

    #[test]
    fn pipelines_lists_redirections_and_comments() {
        let src = "git sttus | grep 'a b' > out.txt 2>&1 && ls -la; echo done # note";
        let s = parse(src);
        assert!(s.is_fully_supported(), "{:?}", s.diagnostics);
        assert_eq!(s.commands.len(), 4);
        assert_eq!(values(&s, 0), [Some("git"), Some("sttus")]);
        assert_eq!(values(&s, 1), [Some("grep"), Some("a b")]);
        assert_eq!(s.commands[1].connector, Some(Connector::Pipe));
        assert_eq!(s.commands[2].connector, Some(Connector::And));
        assert_eq!(s.commands[3].connector, Some(Connector::Sequence));
        let redirections = &s.commands[1].redirections;
        assert_eq!(redirections.len(), 2);
        assert_eq!(redirections[0].operator.of(src), ">");
        assert_eq!(
            redirections[0].target.as_ref().unwrap().literal(),
            Some("out.txt")
        );
        assert_eq!(redirections[1].operator.of(src), "2>&");
        assert_eq!(s.comments[0].of(src), "# note");
        assert_eq!(s.commands[0].span.of(src), "git sttus");
        assert_eq!(s.pipeline_of(1), 0..2);
        assert_eq!(s.pipeline_of(2), 2..3);
    }

    #[test]
    fn quotes_escapes_and_unicode() {
        let s = parse(r#"echo "a \"b\" \$x" 'c d' e\ f "ünï"λ"#);
        assert!(s.is_fully_supported());
        assert_eq!(
            values(&s, 0),
            [
                Some("echo"),
                Some("a \"b\" $x"),
                Some("c d"),
                Some("e f"),
                Some("ünïλ")
            ]
        );
        assert!(s.commands[0].words[2].quoted);
        assert!(!s.commands[0].words[0].quoted);
    }

    #[test]
    fn expansions_are_opaque_and_never_evaluated() {
        let s =
            parse("echo $HOME \"$(rm -rf x)\" `id` *.rs ~/x {a,b} a[12] ${X:-y} $((1+2)) <(ls)");
        assert!(s.is_fully_supported(), "{:?}", s.diagnostics);
        let words = values(&s, 0);
        assert_eq!(words[0], Some("echo"));
        assert!(words[1..].iter().all(Option::is_none), "{words:?}");
        assert_eq!(
            s.commands.len(),
            1,
            "substitution contents stay inside the word"
        );
        let s = parse("find . -name '*.rs' -exec ls {} \\; [ x ] a=b $ cost");
        assert_eq!(
            values(&s, 0),
            [
                Some("find"),
                Some("."),
                Some("-name"),
                Some("*.rs"),
                Some("-exec"),
                Some("ls"),
                Some("{}"),
                Some(";"),
                Some("["),
                Some("x"),
                Some("]"),
                Some("a=b"),
                Some("$"),
                Some("cost")
            ]
        );
    }

    #[test]
    fn assignments_and_wrappers() {
        let s = parse("FOO=1 BAR=\"x y\" make -j4");
        assert_eq!(s.commands[0].assignments.len(), 2);
        assert_eq!(s.commands[0].assignments[1].literal(), Some("BAR=x y"));
        assert_eq!(values(&s, 0), [Some("make"), Some("-j4")]);
        let s = parse("env A=1 ls");
        assert!(s.commands[0].assignments.is_empty());
    }

    #[test]
    fn unsupported_and_malformed_input_is_diagnosed() {
        for (src, kind) in [
            ("echo 'abc", DiagnosticKind::UnterminatedQuote),
            ("echo \"abc", DiagnosticKind::UnterminatedQuote),
            ("echo $(ls", DiagnosticKind::UnterminatedSubstitution),
            ("echo `ls", DiagnosticKind::UnterminatedSubstitution),
            ("ls &&", DiagnosticKind::UnexpectedOperator),
            ("| ls", DiagnosticKind::UnexpectedOperator),
            ("if true; then ls; fi", DiagnosticKind::CompoundCommand),
            ("(cd x && ls)", DiagnosticKind::CompoundCommand),
            ("{ ls; }", DiagnosticKind::CompoundCommand),
            ("f() { ls; }", DiagnosticKind::CompoundCommand),
            ("cat <<EOF", DiagnosticKind::HereDocument),
            ("ls >", DiagnosticKind::MissingRedirectionTarget),
            ("echo \\", DiagnosticKind::TrailingEscape),
        ] {
            let s = parse(src);
            assert!(
                s.diagnostics.iter().any(|d| d.kind == kind),
                "{src:?}: {:?}",
                s.diagnostics
            );
        }
        let s = parse("echo 'abc");
        assert_eq!(s.commands.len(), 1, "a partial tree is still returned");
        assert!(parse("cat <<<'here string'").is_fully_supported());
        assert!(parse("ls &").is_fully_supported());
        assert!(parse("ls;\nls\n").is_fully_supported());
        assert!(parse("ls &&\\\n ls").is_fully_supported());
    }

    #[test]
    fn edits_replace_only_their_span() {
        let src = "cat 'x y' | git sttus  > \"out file\" # keep";
        let s = parse(src);
        let target = &s.commands[1].words[1];
        let edited = apply_edits(
            src,
            &[Edit {
                span: target.span,
                replacement: quote_word("status"),
            }],
        )
        .unwrap();
        assert_eq!(edited, "cat 'x y' | git status  > \"out file\" # keep");
        assert!(
            apply_edits(
                src,
                &[
                    Edit {
                        span: Span::new(0, 5),
                        replacement: String::new()
                    },
                    Edit {
                        span: Span::new(3, 6),
                        replacement: String::new()
                    }
                ]
            )
            .is_none(),
            "overlapping edits are rejected"
        );
        assert_eq!(quote_word("a b"), "'a b'");
        assert_eq!(quote_word("~x"), "'~x'");
    }

    #[test]
    fn option_name_span_preserves_the_value() {
        let src = "ls --colro=auto --x='a b' 'quoted'";
        let s = parse(src);
        let words = &s.commands[0].words;
        assert_eq!(words[1].option_name_span(src).unwrap().of(src), "--colro");
        assert_eq!(words[2].option_name_span(src).unwrap().of(src), "--x");
        assert_eq!(words[3].option_name_span(src), None);
    }
}
