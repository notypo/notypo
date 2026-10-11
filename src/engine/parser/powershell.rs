//! PowerShell's argument-mode command lines: native and cmdlet invocations
//! joined by `|`, `;`, `&&`, `||`, `&`, and newlines, with redirections.
//! Expressions (`$x = 1`, `(...)`, `{...}`, `@(...)`, arrays with `,`),
//! statements (`if`, `foreach`, `function`, ...), the call operator `&`,
//! dot-sourcing, and `--%` are reported as unsupported.
//!
//! The rules follow PowerShell 7's own parser, checked against its AST:
//! single quotes (and their Unicode forms) end only at a quote that is not
//! doubled; double quotes process backtick escapes and expand `$`; a token
//! that begins with a quote ends at the closing quote, while quotes inside a
//! bareword join it (`a'b c'd` is `ab cd`); backtick escapes also apply in
//! barewords; `#` starts a comment only at a token's start; `>` redirects
//! only at a token's start; a bare leading `~` names the home directory.

use super::{
    Connector, Diagnostic, DiagnosticKind, Dialect, Redirection, Script, SimpleCommand, Span, Word,
};

const SINGLE: [char; 5] = ['\'', '\u{2018}', '\u{2019}', '\u{201a}', '\u{201b}'];
const DOUBLE: [char; 4] = ['"', '\u{201c}', '\u{201d}', '\u{201e}'];

/// Words that begin statements or language constructs in command position.
const KEYWORDS: &[&str] = &[
    "begin",
    "break",
    "catch",
    "class",
    "clean",
    "configuration",
    "continue",
    "data",
    "do",
    "dynamicparam",
    "else",
    "elseif",
    "end",
    "enum",
    "exit",
    "filter",
    "finally",
    "for",
    "foreach",
    "function",
    "hidden",
    "if",
    "inlinescript",
    "parallel",
    "param",
    "process",
    "return",
    "sequence",
    "static",
    "switch",
    "throw",
    "trap",
    "try",
    "until",
    "using",
    "while",
    "workflow",
];

pub(super) fn parse(source: &str) -> Script {
    let mut parser = Parser {
        src: source,
        i: 0,
        commands: Vec::new(),
        comments: Vec::new(),
        diagnostics: Vec::new(),
        current: SimpleCommand::default(),
        connector: None,
    };
    parser.run();
    Script {
        dialect: Dialect::PowerShell,
        source: source.to_owned(),
        commands: parser.commands,
        compounds: Vec::new(),
        comments: parser.comments,
        diagnostics: parser.diagnostics,
    }
}

struct Parser<'a> {
    src: &'a str,
    i: usize,
    commands: Vec<SimpleCommand>,
    comments: Vec<Span>,
    diagnostics: Vec<Diagnostic>,
    current: SimpleCommand,
    connector: Option<(Connector, Span)>,
}

fn is_blank(c: char) -> bool {
    c.is_whitespace() && c != '\n' && c != '\r'
}

/// Characters that end a bareword (besides blanks and newlines).
fn ends_word(c: char) -> bool {
    matches!(c, ';' | '|' | '&' | '(' | ')' | '{' | '}' | ',')
}

/// `$name`, `${...}`, `$(...)`, and the automatic variables expand; a `$`
/// before anything else is a literal dollar sign.
fn starts_expansion(next: Option<char>) -> bool {
    next.is_some_and(|c| {
        c.is_alphanumeric() || matches!(c, '_' | '{' | '(' | '?' | '^' | '$' | ':')
    })
}

/// A numeric literal (`5`, `0x10`, `1kb`, `.5e3d`): in command position it
/// starts an expression. Anything else, such as `7z`, names a command.
fn is_number(text: &str) -> bool {
    regex!(
        r"(?i)^[+-]?(0x[0-9a-f]+|0b[01]+|(\d+\.?\d*|\.\d+)(e[+-]?\d+)?)(u?[ysl]|u|n|d|ul|lu)?(kb|mb|gb|tb|pb)?$"
    )
    .is_match(text)
}

/// What a backtick followed by `c` stands for.
fn escaped(c: char) -> char {
    match c {
        '0' => '\0',
        'a' => '\u{7}',
        'b' => '\u{8}',
        'e' => '\u{1b}',
        'f' => '\u{c}',
        'n' => '\n',
        'r' => '\r',
        't' => '\t',
        'v' => '\u{b}',
        other => other,
    }
}

impl Parser<'_> {
    fn peek(&self) -> Option<char> {
        self.src[self.i..].chars().next()
    }

    fn peek_at(&self, offset: usize) -> Option<char> {
        self.src[self.i..].chars().nth(offset)
    }

    fn rest(&self) -> &str {
        &self.src[self.i..]
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.i += c.len_utf8();
        Some(c)
    }

    fn diagnose(&mut self, start: usize, kind: DiagnosticKind) {
        self.diagnostics.push(Diagnostic {
            span: Span::new(start, self.i.max(start)),
            kind,
        });
    }

    /// Everything from here to the end of the line is not understood.
    fn give_up(&mut self, start: usize, kind: DiagnosticKind) {
        while let Some(c) = self.peek() {
            if c == '\n' {
                break;
            }
            self.bump();
        }
        self.diagnose(start, kind);
    }

    fn finish_command(&mut self, connector: Option<(Connector, Span)>) {
        let command = std::mem::take(&mut self.current);
        if command.words.is_empty() && command.redirections.is_empty() {
            // `|`, `&&`, and `||` need a command after them (`x | `), and
            // every operator but a separator needs one before it (`| x`).
            if let Some((op, span)) = self.connector.take()
                && matches!(op, Connector::Pipe | Connector::And | Connector::Or)
            {
                self.diagnostics.push(Diagnostic {
                    span,
                    kind: DiagnosticKind::UnexpectedOperator,
                });
            }
            match connector {
                Some((
                    Connector::Pipe | Connector::And | Connector::Or | Connector::Background,
                    span,
                )) => {
                    self.diagnostics.push(Diagnostic {
                        span,
                        kind: DiagnosticKind::UnexpectedOperator,
                    });
                }
                other => self.connector = other,
            }
            return;
        }
        let mut command = command;
        command.connector = self.connector.take().map(|(op, _)| op);
        command.span = Span::new(
            command
                .words
                .first()
                .map(|w| w.span.start)
                .into_iter()
                .chain(command.redirections.first().map(|r| r.span.start))
                .min()
                .unwrap_or(0),
            command
                .words
                .last()
                .map(|w| w.span.end)
                .into_iter()
                .chain(command.redirections.last().map(|r| r.span.end))
                .max()
                .unwrap_or(0),
        );
        self.commands.push(command);
        self.connector = connector;
    }

    fn run(&mut self) {
        while let Some(c) = self.peek() {
            let start = self.i;
            if is_blank(c) {
                self.bump();
                continue;
            }
            if c == '`' && matches!(self.peek_at(1), Some('\n' | '\r')) {
                self.bump();
                if self.bump() == Some('\r') && self.peek() == Some('\n') {
                    self.bump();
                }
                continue;
            }
            match c {
                '\n' | '\r' => {
                    self.bump();
                    if c == '\r' && self.peek() == Some('\n') {
                        self.bump();
                    }
                    self.finish_command(Some((Connector::Newline, Span::new(start, self.i))));
                }
                '#' => {
                    while self.peek().is_some_and(|c| c != '\n' && c != '\r') {
                        self.bump();
                    }
                    self.comments.push(Span::new(start, self.i));
                }
                '<' if self.rest().starts_with("<#") => match self.rest().find("#>") {
                    Some(end) => {
                        self.i += end + 2;
                        self.comments.push(Span::new(start, self.i));
                    }
                    None => {
                        self.i = self.src.len();
                        self.diagnose(start, DiagnosticKind::UnterminatedQuote);
                    }
                },
                ';' => {
                    self.bump();
                    self.finish_command(Some((Connector::Sequence, Span::new(start, self.i))));
                }
                '|' => {
                    self.bump();
                    let op = if self.peek() == Some('|') {
                        self.bump();
                        Connector::Or
                    } else {
                        Connector::Pipe
                    };
                    self.finish_command(Some((op, Span::new(start, self.i))));
                }
                '&' => {
                    if self.peek_at(1) == Some('&') {
                        self.i += 2;
                        self.finish_command(Some((Connector::And, Span::new(start, self.i))));
                    } else if self.current.words.is_empty() && self.current.redirections.is_empty()
                    {
                        // The call operator runs whatever the next token evaluates to.
                        self.give_up(start, DiagnosticKind::CompoundCommand);
                    } else {
                        self.bump();
                        self.finish_command(Some((
                            Connector::Background,
                            Span::new(start, self.i),
                        )));
                    }
                }
                '(' | ')' | '{' | '}' | ',' => self.give_up(start, DiagnosticKind::CompoundCommand),
                '<' => {
                    self.bump();
                    self.diagnose(start, DiagnosticKind::UnexpectedOperator);
                }
                _ if self.redirection() => {}
                _ => self.word_or_construct(),
            }
        }
        self.finish_command(None);
        if let Some((op, span)) = self.connector.take()
            && !matches!(
                op,
                Connector::Newline | Connector::Sequence | Connector::Background
            )
        {
            self.diagnostics.push(Diagnostic {
                span,
                kind: DiagnosticKind::UnexpectedOperator,
            });
        }
    }

    /// `>`, `>>`, `N>`, `N>>`, `*>`, and merging `N>&1`/`N>&2`, at a token's
    /// start only (`x2>y` is one word).
    fn redirection(&mut self) -> bool {
        let rest = self.rest();
        let stream = rest
            .chars()
            .next()
            .filter(|c| *c == '*' || ('1'..='6').contains(c))
            .map_or(0, |_| 1);
        if !rest[stream..].starts_with('>') {
            return false;
        }
        let start = self.i;
        let mut length = stream + 1;
        if rest[length..].starts_with('>') {
            length += 1;
        }
        let merge = length == stream + 1
            && (rest[length..].starts_with("&1") || rest[length..].starts_with("&2"));
        if merge {
            length += 2;
        }
        self.i += length;
        let operator = Span::new(start, self.i);
        let target = if merge {
            None
        } else {
            while self.peek().is_some_and(is_blank) {
                self.bump();
            }
            match self.peek() {
                Some(c) if !matches!(c, '\n' | '\r' | '#') && !ends_word(c) && c != '>' => {
                    self.word()
                }
                _ => None,
            }
        };
        if !merge && target.is_none() {
            self.diagnose(start, DiagnosticKind::MissingRedirectionTarget);
        }
        self.current.redirections.push(Redirection {
            span: Span::new(start, target.as_ref().map_or(self.i, |t| t.span.end)),
            operator,
            target,
        });
        true
    }

    /// A word, or a construct this parser leaves to PowerShell.
    fn word_or_construct(&mut self) {
        let start = self.i;
        let command_position = self.current.words.is_empty();
        let rest = self.rest();
        let ends = |s: &str| {
            s.chars()
                .next()
                .is_none_or(|c| c.is_whitespace() || ends_word(c) || c == '#')
        };
        if rest.starts_with("--%") && ends(&rest[3..]) {
            self.give_up(start, DiagnosticKind::CompoundCommand);
            return;
        }
        if rest.starts_with('@') && starts_expansion(rest[1..].chars().next())
            || rest.starts_with("@'")
            || rest.starts_with("@\"")
        {
            // Splatting and array/hash literals can supply any number of
            // arguments; here-strings span lines.
            self.give_up(start, DiagnosticKind::CompoundCommand);
            return;
        }
        if command_position {
            let quoted = rest.starts_with(SINGLE) || rest.starts_with(DOUBLE);
            let expression = rest.starts_with('$')
                || rest.starts_with('[')
                || rest.starts_with(['-', '+', '!'])
                || (rest.starts_with('.') && ends(&rest[1..]));
            if quoted || expression {
                self.give_up(start, DiagnosticKind::CompoundCommand);
                return;
            }
        }
        let Some(word) = self.word() else {
            return;
        };
        // Native commands treat a `--%` argument as stop-parsing even when
        // it is quoted, and re-split everything after it (PowerShell 7.6).
        if word.literal() == Some("--%") {
            self.i = word.span.start;
            self.give_up(start, DiagnosticKind::CompoundCommand);
            return;
        }
        if command_position {
            let text = word.span.of(self.src);
            if KEYWORDS.iter().any(|k| k.eq_ignore_ascii_case(text)) || is_number(text) {
                self.i = word.span.start;
                self.give_up(start, DiagnosticKind::CompoundCommand);
                return;
            }
        }
        self.current.words.push(word);
    }

    /// One argument token. A token beginning with a quote ends at its
    /// closing quote; a bareword continues through embedded quoted parts.
    fn word(&mut self) -> Option<Word> {
        let start = self.i;
        let mut value = String::new();
        let mut literal = true;
        let mut quoted = false;
        let first = self.peek()?;
        if SINGLE.contains(&first) || DOUBLE.contains(&first) {
            quoted = true;
            if !self.quoted_part(&mut value, &mut literal) {
                return None;
            }
            return Some(Word {
                span: Span::new(start, self.i),
                value: literal.then_some(value),
                quoted,
            });
        }
        if first == '~'
            && self
                .peek_at(1)
                .is_none_or(|c| c == '/' || c == '\\' || c.is_whitespace() || ends_word(c))
        {
            // PowerShell expands a bare leading `~` for native commands.
            literal = false;
        }
        // `-name` is a parameter token until a colon or quote: `.` and `[`
        // end it, so `-Dprop.name=1` reaches a program as two arguments.
        let mut parameter = first == '-'
            && self
                .peek_at(1)
                .is_some_and(|c| c.is_alphabetic() || c == '_' || c == '?');
        while let Some(c) = self.peek() {
            if c.is_whitespace() || ends_word(c) {
                break;
            }
            if parameter {
                match c {
                    '.' | '[' => break,
                    ':' => parameter = false,
                    // PowerShell keeps the backtick in a parameter token.
                    '`' => {
                        self.give_up(start, DiagnosticKind::InvalidEscape);
                        return None;
                    }
                    _ => {}
                }
            }
            if SINGLE.contains(&c) || DOUBLE.contains(&c) {
                quoted = true;
                parameter = false;
                if !self.quoted_part(&mut value, &mut literal) {
                    return None;
                }
                continue;
            }
            match c {
                '`' => {
                    self.bump();
                    match self.peek() {
                        // A line continuation ends the word.
                        Some('\n' | '\r') => {
                            self.i -= 1;
                            break;
                        }
                        None => value.push('`'),
                        Some(_) => {
                            quoted = true;
                            self.escape(&mut value);
                        }
                    }
                }
                '$' if starts_expansion(self.peek_at(1)) => {
                    literal = false;
                    self.bump();
                    self.expansion();
                }
                _ => {
                    value.push(c);
                    self.bump();
                }
            }
        }
        Some(Word {
            span: Span::new(start, self.i),
            value: literal.then_some(value),
            quoted,
        })
    }

    /// The character after a backtick, at `self.i`.
    fn escape(&mut self, value: &mut String) {
        if self.peek() == Some('u') && self.peek_at(1) == Some('{') {
            let rest = &self.src[self.i + 2..];
            if let Some(end) = rest.find('}')
                && let Ok(code) = u32::from_str_radix(&rest[..end], 16)
                && let Some(c) = char::from_u32(code)
            {
                value.push(c);
                self.i += 2 + end + 1;
                return;
            }
        }
        if let Some(c) = self.bump() {
            value.push(escaped(c));
        }
    }

    /// A `$` expansion's remainder: `{...}`, `(...)` with nesting and quoted
    /// text, or a name. Its value is never computed.
    fn expansion(&mut self) {
        let start = self.i - 1;
        match self.peek() {
            Some('{') => match self.rest().find('}') {
                Some(end) => self.i += end + 1,
                None => {
                    self.i = self.src.len();
                    self.diagnose(start, DiagnosticKind::UnterminatedSubstitution);
                }
            },
            Some('(') => {
                let mut depth = 0usize;
                while let Some(c) = self.bump() {
                    match c {
                        '(' => depth += 1,
                        ')' => {
                            depth -= 1;
                            if depth == 0 {
                                return;
                            }
                        }
                        '`' => {
                            self.bump();
                        }
                        c if SINGLE.contains(&c) || DOUBLE.contains(&c) => {
                            while let Some(inner) = self.bump() {
                                if inner == '`' && DOUBLE.contains(&c) {
                                    self.bump();
                                } else if (SINGLE.contains(&c) && SINGLE.contains(&inner))
                                    || (DOUBLE.contains(&c) && DOUBLE.contains(&inner))
                                {
                                    break;
                                }
                            }
                        }
                        _ => {}
                    }
                }
                self.diagnose(start, DiagnosticKind::UnterminatedSubstitution);
            }
            _ => {
                self.bump();
                while self
                    .peek()
                    .is_some_and(|c| c.is_alphanumeric() || matches!(c, '_' | ':'))
                {
                    self.bump();
                }
            }
        }
    }

    /// A quoted part at `self.i`; false when it never closes.
    fn quoted_part(&mut self, value: &mut String, literal: &mut bool) -> bool {
        let start = self.i;
        let Some(open) = self.bump() else {
            return false;
        };
        let single = SINGLE.contains(&open);
        loop {
            let Some(c) = self.bump() else {
                self.diagnose(start, DiagnosticKind::UnterminatedQuote);
                return false;
            };
            if single && SINGLE.contains(&c) {
                if self.peek().is_some_and(|n| SINGLE.contains(&n)) {
                    self.bump();
                    value.push(c);
                    continue;
                }
                return true;
            }
            if !single && DOUBLE.contains(&c) {
                if self.peek().is_some_and(|n| DOUBLE.contains(&n)) {
                    self.bump();
                    value.push(c);
                    continue;
                }
                return true;
            }
            match c {
                '`' if !single => {
                    if self.peek().is_none() {
                        self.diagnose(start, DiagnosticKind::UnterminatedQuote);
                        return false;
                    }
                    self.escape(value);
                }
                '$' if !single && starts_expansion(self.peek()) => {
                    *literal = false;
                    self.expansion();
                }
                _ => value.push(c),
            }
        }
    }
}

/// A value as one PowerShell argument: bare when every character is plain
/// in argument mode, otherwise single-quoted with quote characters doubled.
pub(super) fn quote(value: &str) -> String {
    let bare = !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./\\:=+%".contains(c))
        && !value.starts_with("--%")
        && !value.starts_with(['+'])
        && !(value.starts_with('.') && value.len() == 1)
        // A parameter token would split at the dot: `-Dprop.name=1`.
        && !(value.starts_with('-')
            && value[1..].starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
            && value.split(':').next().is_some_and(|name| name.contains('.')));
    if bare {
        return value.to_owned();
    }
    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('\'');
    for c in value.chars() {
        if SINGLE.contains(&c) {
            quoted.push(c);
        }
        quoted.push(c);
    }
    quoted.push('\'');
    quoted
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(source: &str) -> Vec<Vec<Option<String>>> {
        let script = parse(source);
        assert!(
            script.is_fully_supported(),
            "{source}: {:?}",
            script.diagnostics
        );
        script
            .commands
            .iter()
            .map(|c| c.words.iter().map(|w| w.value.clone()).collect())
            .collect()
    }

    fn words(source: &str) -> Vec<String> {
        values(source)
            .into_iter()
            .flatten()
            .map(|w| w.unwrap_or_else(|| "<opaque>".into()))
            .collect()
    }

    #[test]
    fn argument_mode_words_follow_powershells_parser() {
        // Each expectation was read from PowerShell 7.6's AST.
        for (source, expected) in [
            ("git sttus", vec!["git", "sttus"]),
            (
                "git commit -m 'it''s fixed'",
                vec!["git", "commit", "-m", "it's fixed"],
            ),
            ("echo \"a`\"b\" \"\" a` b", vec!["echo", "a\"b", "", "a b"]),
            ("echo a'b c'd a\"b c\"d", vec!["echo", "ab cd", "ab cd"]),
            ("echo a`nb", vec!["echo", "a\nb"]),
            ("echo \"x`ny\"", vec!["echo", "x\ny"]),
            (
                "echo --color=auto -x:y --name='a b'",
                vec!["echo", "--color=auto", "-x:y", "--name=a b"],
            ),
            (
                "echo user@host a#b C:\\path\\f.txt",
                vec!["echo", "user@host", "a#b", "C:\\path\\f.txt"],
            ),
            (
                "echo \u{2018}curly\u{2019} \u{201c}dq\u{201d}",
                vec!["echo", "curly", "dq"],
            ),
            ("echo $", vec!["echo", "$"]),
            ("echo x2>y a`$b 'a`$b'", vec!["echo", "x2>y", "a$b", "a`$b"]),
            ("echo \"\"a\"\" '''x'''", vec!["echo", "", "a", "'x'"]),
            ("echo 'x'y \"x\"y", vec!["echo", "x", "y", "x", "y"]),
            ("echo a\"\"b \"a\"\"b\"", vec!["echo", "ab", "a\"b"]),
            ("echo a` `#x `u{263A}", vec!["echo", "a #x", "\u{263a}"]),
            (
                "echo [int] -- --x 0x10 .5",
                vec!["echo", "[int]", "--", "--x", "0x10", ".5"],
            ),
            ("echo 'a'#comment", vec!["echo", "a"]),
            (
                "echo $HOME \"$HOME/x\" a$b ~/x",
                vec!["echo", "<opaque>", "<opaque>", "<opaque>", "<opaque>"],
            ),
            (
                "./app x; .\\app.exe y",
                vec!["./app", "x", ".\\app.exe", "y"],
            ),
            ("7z x a.7z", vec!["7z", "x", "a.7z"]),
            // What a native program receives, checked by running one.
            (
                "x -o./out -Dprop.name=1 -x:a.b -a'b'.c -1.5 -.a --a.b -o/tmp",
                vec![
                    "x", "-o", "./out", "-Dprop", ".name=1", "-x:a.b", "-ab.c", "-1.5", "-.a",
                    "--a.b", "-o/tmp",
                ],
            ),
            ("in x", vec!["in", "x"]),
        ] {
            assert_eq!(words(source), expected, "{source}");
        }
    }

    #[test]
    fn operators_redirections_and_comments() {
        let script =
            parse("ls | sort; git status && echo ok || echo no\ngit log 2>&1 > out.txt *>>all &");
        assert!(script.is_fully_supported(), "{:?}", script.diagnostics);
        let connectors: Vec<_> = script.commands.iter().map(|c| c.connector).collect();
        assert_eq!(
            connectors,
            [
                None,
                Some(Connector::Pipe),
                Some(Connector::Sequence),
                Some(Connector::And),
                Some(Connector::Or),
                Some(Connector::Newline),
            ]
        );
        let last = script.commands.last().unwrap();
        let operators: Vec<_> = last
            .redirections
            .iter()
            .map(|r| {
                (
                    r.operator.of(&script.source),
                    r.target.as_ref().map(|t| t.span.of(&script.source)),
                )
            })
            .collect();
        assert_eq!(
            operators,
            [("2>&1", None), (">", Some("out.txt")), ("*>>", Some("all"))]
        );
        assert_eq!(
            values("echo a&b"),
            [
                vec![Some("echo".into()), Some("a".into())],
                vec![Some("b".into())]
            ]
        );
        let commented = parse("echo <# c #> x # tail");
        assert_eq!(commented.comments.len(), 2);
        assert_eq!(words("echo <# c #> x # tail"), ["echo", "x"]);
        assert_eq!(words("git `\n  status"), ["git", "status"]);
    }

    #[test]
    fn expressions_statements_and_unclosed_text_are_unsupported() {
        for source in [
            "echo a,b",
            "echo (1+2)",
            "echo {x}",
            "echo a(b)",
            "echo @args",
            "& 'C:\\app.exe' x",
            "'git' status",
            "\"git\" status",
            "$x = 1",
            "if ($x) { git status }",
            "foreach ($f in $files) { rm $f }",
            "function f { }",
            "5 x",
            "1kb x",
            "0x10 x",
            "If x",
            "echo --% raw stuff",
            "echo '--%' x",
            "echo \"--%\" x",
            "echo 'unterminated",
            "echo \"unterminated",
            "echo $(unclosed",
            "echo a | ",
            "| sort",
            "echo a && && b",
            "echo <x",
            "echo >",
            ". ./profile.ps1",
            "echo a)b",
        ] {
            assert!(!parse(source).is_fully_supported(), "{source}");
        }
    }

    /// PowerShell itself, when installed (or named by `NOTYPO_TEST_PWSH`).
    fn pwsh() -> Option<std::path::PathBuf> {
        std::env::var_os("NOTYPO_TEST_PWSH")
            .map(std::path::PathBuf::from)
            .or_else(|| crate::utils::which("pwsh"))
    }

    const ORACLE: &str = r#"
foreach ($line in [System.IO.File]::ReadAllLines($args[0])) {
  $tokens = $null; $errors = $null
  $ast = [System.Management.Automation.Language.Parser]::ParseInput($line, [ref]$tokens, [ref]$errors)
  # Commands inside `$(...)` belong to an opaque word, not to the line.
  $top = { param($node)
    if ($node -isnot [System.Management.Automation.Language.CommandAst]) { return $false }
    for ($p = $node.Parent; $p; $p = $p.Parent) {
      if ($p -is [System.Management.Automation.Language.SubExpressionAst]) { return $false }
    }
    $true }
  $commands = @(foreach ($cmd in $ast.FindAll($top, $true)) {
    $words = @(foreach ($el in $cmd.CommandElements) {
      if ($el -is [System.Management.Automation.Language.StringConstantExpressionAst]) {
        if ($el.StringConstantType -eq 'BareWord' -and $el.Value.StartsWith('~')) { $null } else { $el.Value }
      } elseif ($el -is [System.Management.Automation.Language.ConstantExpressionAst] -or
                $el -is [System.Management.Automation.Language.CommandParameterAst]) { $el.Extent.Text }
      else { $null }
    })
    $targets = @(foreach ($r in ($cmd.Redirections | Sort-Object { $_.Extent.StartOffset })) {
      if ($r -is [System.Management.Automation.Language.FileRedirectionAst]) { $r.Location.Extent.Text } else { '&' }
    })
    ,@{ words = $words; targets = $targets }
  })
  ConvertTo-Json -Compress -Depth 5 @{ errors = $errors.Count; commands = $commands }
}
"#;

    #[test]
    fn accepted_lines_match_powershells_own_parser() {
        let Some(pwsh) = pwsh() else {
            eprintln!("skipped: PowerShell is not installed");
            return;
        };
        let corpus = [
            "git sttus",
            "git commit -m 'it''s fixed' --amend",
            "git commit -m \"fix: it's done\"",
            "echo \"a`\"b\" \"\" a` b",
            "echo a'b c'd a\"b c\"d 'x'y \"x\"y",
            "echo a`nb \"x`ny\" `u{263A}",
            "echo --color=auto -x:y --name='a b' -x=y",
            "echo user@host a#b C:\\path\\file.txt [int] -- --x",
            "echo \u{2018}curly\u{2019} \u{201c}dq\u{201d}",
            "echo $ x2>y a`$b 'a`$b' 0x10 .5 1kb",
            "echo \"\"a\"\" '''x''' a\"\"b \"a\"\"b\"",
            "echo $HOME \"$HOME/x\" a$b ~/x $env:PATH ${x} $(git rev-parse HEAD)",
            "ls | sort; git status && echo ok || echo no",
            "git log 2>&1 > out.txt",
            "git status *>> all.txt 3>&1",
            "echo a >b",
            "echo 'a'#comment",
            "echo <# c #> x # tail",
            "kubectl get pods -n kube-system -o jsonpath='{.items[*].metadata.name}'",
            "aws s3 cp s3://bucket/key .\\local --recursive",
            "npm run build -- --watch",
            "7z x a.7z -o./out -a.b -C.. -x:a.b -a'b'.c -foo.bar=1 -1.5 -.a --a.b",
            "docker run --rm -e A=1 node:20 npm test",
            "git status &",
            "echo a&b",
            ".\\build.cmd -Configuration Release",
        ];
        let dir = std::env::temp_dir().join(format!("notypo-pwsh-oracle-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let lines = dir.join("lines.txt");
        let script = dir.join("oracle.ps1");
        std::fs::write(&lines, corpus.join("\n")).unwrap();
        std::fs::write(&script, ORACLE).unwrap();
        let output = std::process::Command::new(&pwsh)
            .args(["-NoProfile", "-NonInteractive", "-File"])
            .arg(&script)
            .arg(&lines)
            .output()
            .unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let answers: Vec<serde_json::Value> = String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(answers.len(), corpus.len());
        for (source, oracle) in corpus.iter().zip(answers) {
            let script = parse(source);
            assert!(
                script.is_fully_supported(),
                "{source}: {:?}",
                script.diagnostics
            );
            assert_eq!(oracle["errors"], 0, "{source}");
            let ours: Vec<serde_json::Value> = script
                .commands
                .iter()
                .map(|command| {
                    serde_json::json!({
                        "words": command.words.iter().map(|w| w.value.clone()).collect::<Vec<_>>(),
                        "targets": command
                            .redirections
                            .iter()
                            .map(|r| r.target.as_ref().map_or("&", |t| t.span.of(source)))
                            .collect::<Vec<_>>(),
                    })
                })
                .collect();
            assert_eq!(
                serde_json::Value::from(ours),
                oracle["commands"],
                "{source}"
            );
        }
    }

    #[test]
    fn quoted_values_reach_native_programs_unchanged_through_powershell() {
        let (Some(pwsh), Some(python)) = (pwsh(), crate::utils::which("python3")) else {
            eprintln!("skipped: PowerShell or python3 is not installed");
            return;
        };
        let values = [
            "status",
            "--color=auto",
            "it's",
            "a b",
            "",
            "$HOME",
            "a`b",
            "x,y",
            "@args",
            "#tag",
            "~/x",
            "a;b",
            "a|b",
            "a&b",
            "(x)",
            "{x}",
            "*.txt",
            "\u{2019}q\u{2018}",
            "\"dq\"",
            "5",
            "0x10",
            "-x",
            "-Dprop.name=1",
            "-o./out",
            "-x:a.b",
            "C:\\Program Files\\x",
            "1kb",
            "[0]",
            "\u{263a} \u{e9}",
        ];
        let line = format!(
            "& {} -c 'import sys, json; print(json.dumps(sys.argv[1:]))' {}",
            quote(python.to_str().unwrap()),
            values
                .iter()
                .map(|v| quote(v))
                .collect::<Vec<_>>()
                .join(" ")
        );
        // The line itself must be one this parser accepts after the call.
        let script = parse(line.strip_prefix("& ").unwrap());
        assert!(
            script.is_fully_supported() || line.contains("& '"),
            "{:?}",
            script.diagnostics
        );
        let output = std::process::Command::new(&pwsh)
            .args(["-NoProfile", "-NonInteractive", "-Command", &line])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let received: Vec<String> = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(received, values, "{line}");
    }

    #[test]
    fn quoting_round_trips_through_this_parser() {
        for value in [
            "status",
            "--color=auto",
            "C:\\Program Files\\x",
            "it's",
            "a b",
            "",
            "$HOME",
            "a`b",
            "x,y",
            "@args",
            "#tag",
            "~/x",
            "a;b",
            "a|b",
            "a&b",
            "(x)",
            "{x}",
            "*.txt",
            "\u{2019}q\u{2018}",
            "line\nbreak",
            "\"dq\"",
            "5",
            "-x",
            "+1",
            ".",
            "-Dprop.name=1",
            "-o./out",
            "-x:a.b",
        ] {
            let quoted = quote(value);
            let source = format!("echo {quoted}");
            let script = parse(&source);
            assert!(
                script.is_fully_supported(),
                "{value:?} -> {quoted}: {:?}",
                script.diagnostics
            );
            let words = &script.commands[0].words;
            assert_eq!(words.len(), 2, "{value:?} -> {quoted}");
            assert_eq!(words[1].literal(), Some(value), "{value:?} -> {quoted}");
        }
        // No quoting passes `--%` to a native command intact.
        assert!(!parse(&format!("echo {}", quote("--%"))).is_fully_supported());
        assert_eq!(quote("status"), "status");
        assert_eq!(quote("it's"), "'it''s'");
    }
}
