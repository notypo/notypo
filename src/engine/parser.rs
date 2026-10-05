//! Lossless parsing of POSIX-family and fish command lines.
//!
//! Nothing is evaluated: parameter expansions, command and process
//! substitutions, arithmetic, globs, braces, and tildes make a word opaque
//! instead of being expanded. Every word, redirection, and comment keeps the
//! byte span it came from, so an edit replaces exactly those bytes and the
//! rest of the line stays byte-for-byte intact.
//!
//! Supported structure: simple commands (assignments, words, redirections)
//! joined into pipelines (`|`, `|&`) and lists (`&&`, `||`, `;`, `&`, and
//! newlines), inside compound commands: `{ ...; }` groups, subshells, `if`,
//! `while`, `until`, `for`, `select`, `case`, function definitions, and `!`;
//! fish's `begin`, `if`/`else if`, `while`, `for`, `switch`, `function`, and
//! `not`. Simple commands stay a flat list; each [`Compound`] records the
//! reserved words and non-command words around a range of them.
//!
//! Arithmetic commands, `[[ ... ]]`, `coproc`, zsh's short loop forms and
//! `always`, fish's `{ ...; }` blocks, and here-documents are reported as
//! [`Diagnostic`]s next to the partial tree; callers must not edit them.
//! Where Bash, zsh, and fish disagree about a form (zsh's `{ ls }` or
//! `then;`), it is rejected rather than read one way.

use crate::shlex;

mod powershell;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Dialect {
    #[default]
    Posix,
    Fish,
    /// tcsh's interactive syntax: simple commands, pipelines, lists, and
    /// redirections. Its control words and `( )` are not read.
    Tcsh,
    /// PowerShell's argument-mode command lines (see [`powershell`]).
    PowerShell,
}

impl Dialect {
    pub fn for_shell(shell: crate::shells::Shell) -> Option<Self> {
        use crate::shells::Shell;
        match shell {
            Shell::Bash | Shell::Zsh | Shell::Generic => Some(Self::Posix),
            Shell::Fish => Some(Self::Fish),
            Shell::Tcsh => Some(Self::Tcsh),
            Shell::Powershell => Some(Self::PowerShell),
        }
    }
}

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
        self.option_name_span_with(src, '=')
    }

    /// [`Word::option_name_span`] where `separator` attaches the value
    /// (`:` for PowerShell's `-Path:value`).
    pub fn option_name_span_with(&self, src: &str, separator: char) -> Option<Span> {
        let raw = self.span.of(src);
        let end = raw.find(separator).unwrap_or(raw.len());
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
    /// fish's `N>|`, piping a specific descriptor (1 when omitted).
    PipeFd(u32),
    And,
    Or,
    Sequence,
    Background,
    Newline,
}

impl Connector {
    pub fn is_pipe(self) -> bool {
        matches!(
            self,
            Connector::Pipe | Connector::PipeAll | Connector::PipeFd(_)
        )
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CompoundKind {
    /// `{ ...; }`, or fish's `begin ... end`.
    Group,
    Subshell,
    If,
    While,
    Until,
    For,
    Select,
    /// `case ... esac`, or fish's `switch ... end`.
    Case,
    Function,
    /// `!`, or fish's `not`.
    Negation,
}

/// A compound command, block, function definition, or negation. The simple
/// commands inside it stay in [`Script::commands`]; this records the syntax
/// around them, so an edit can be checked against it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Compound {
    pub kind: CompoundKind,
    /// From the opening word through the closing word and redirections.
    pub span: Span,
    /// Reserved words and grouping operators: `if`, `then`, `(`, `end`.
    pub keywords: Vec<Span>,
    /// Words that aren't commands: a loop's variable and items, a case
    /// subject and patterns, a function's name (and fish options).
    pub words: Vec<Word>,
    /// Redirections after the closing word, as in `{ ...; } > out`.
    pub redirections: Vec<Redirection>,
    /// The commands inside it, as indices into [`Script::commands`]. A
    /// negation's range is empty and starts at the pipeline it negates.
    pub commands: std::ops::Range<usize>,
}

/// A compound's syntax as text, independent of where it sits in the line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompoundShape {
    pub kind: CompoundKind,
    pub commands: std::ops::Range<usize>,
    pub keywords: Vec<String>,
    pub words: Vec<String>,
    pub redirections: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiagnosticKind {
    UnterminatedQuote,
    UnterminatedSubstitution,
    TrailingEscape,
    HereDocument,
    /// Compound syntax that isn't parsed: arithmetic commands, `[[ ... ]]`,
    /// `coproc`, zsh-only forms, and fish's `{ ...; }` blocks.
    CompoundCommand,
    /// A block, group, or subshell still open at the end of the line.
    UnterminatedCompound,
    /// A reserved word or grouping operator out of place, such as `fi`
    /// without `if`, an empty `then` clause, or a word after `}`.
    UnexpectedKeyword,
    MissingRedirectionTarget,
    /// An operator with no command on one side, such as `ls &&`.
    UnexpectedOperator,
    InvalidEscape,
    InvalidExpansion,
    /// tcsh would substitute history at this `!`. Its history keeps events
    /// after substitution, without the backslashes that stopped it, so the
    /// recorded text can't be run again as it stands.
    HistorySubstitution,
    /// A quoted newline, which tcsh's alias can't pass back intact.
    QuotedNewline,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub span: Span,
    pub kind: DiagnosticKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Script {
    pub dialect: Dialect,
    pub source: String,
    pub commands: Vec<SimpleCommand>,
    /// In the order they open; nested ones follow their parent.
    pub compounds: Vec<Compound>,
    pub comments: Vec<Span>,
    pub diagnostics: Vec<Diagnostic>,
}

impl Script {
    /// Every construct was understood, so edits have an unambiguous meaning.
    pub fn is_fully_supported(&self) -> bool {
        self.diagnostics.is_empty()
    }

    /// A word's value, or its source text marked as opaque.
    pub fn word_text(&self, word: &Word) -> String {
        word.literal().map_or_else(
            || format!("\0{}", word.span.of(&self.source)),
            str::to_owned,
        )
    }

    pub fn compound_shapes(&self) -> Vec<CompoundShape> {
        self.compounds
            .iter()
            .map(|c| CompoundShape {
                kind: c.kind,
                commands: c.commands.clone(),
                keywords: c
                    .keywords
                    .iter()
                    .map(|k| k.of(&self.source).to_owned())
                    .collect(),
                words: c.words.iter().map(|w| self.word_text(w)).collect(),
                redirections: c
                    .redirections
                    .iter()
                    .map(|r| {
                        let target = r.target.as_ref().map(|t| self.word_text(t));
                        format!(
                            "{} {}",
                            r.operator.of(&self.source),
                            target.unwrap_or_default()
                        )
                    })
                    .collect(),
            })
            .collect()
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

/// tcsh's control words, which take over the lines that follow them.
const TCSH_RESERVED: &[&str] = &[
    "if", "then", "else", "endif", "foreach", "while", "end", "switch", "case", "default",
    "breaksw", "endsw", "repeat", "goto", "onintr",
];

pub fn parse(source: &str) -> Script {
    parse_with_dialect(source, Dialect::Posix)
}

pub fn parse_with_dialect(source: &str, dialect: Dialect) -> Script {
    if dialect == Dialect::PowerShell {
        return powershell::parse(source);
    }
    let mut parser = Parser {
        dialect,
        src: source,
        b: source.as_bytes(),
        i: 0,
        commands: Vec::new(),
        compounds: Vec::new(),
        comments: Vec::new(),
        diagnostics: Vec::new(),
        current: SimpleCommand::default(),
        connector: None,
        stack: Vec::new(),
        closed: None,
        after_keyword: false,
        expect_command: false,
    };
    parser.run();
    Script {
        dialect,
        source: source.to_owned(),
        commands: parser.commands,
        compounds: parser.compounds,
        comments: parser.comments,
        diagnostics: parser.diagnostics,
    }
}

struct Parser<'a> {
    dialect: Dialect,
    src: &'a str,
    b: &'a [u8],
    i: usize,
    commands: Vec<SimpleCommand>,
    compounds: Vec<Compound>,
    comments: Vec<Span>,
    diagnostics: Vec<Diagnostic>,
    current: SimpleCommand,
    /// The operator that ended the previous command.
    connector: Option<(Connector, Span)>,
    /// Open compounds, innermost last.
    stack: Vec<Frame>,
    /// The compound that just closed: only redirections, operators, and
    /// (in POSIX shells) the parent's next reserved word may follow.
    closed: Option<usize>,
    /// Nothing but blanks or separators since the last reserved word.
    after_keyword: bool,
    /// A command must follow on this line: after `!`, or fish's `if`.
    expect_command: bool,
}

struct Frame {
    compound: usize,
    state: State,
    /// `commands.len()` when the current clause began.
    clause: usize,
}

/// Where the parser is inside an open compound.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    IfCondition,
    IfBody,
    ElseBody,
    LoopCondition,
    LoopBody,
    LoopName,
    /// After a loop variable: `in`, `do`, or a separator.
    LoopIn,
    LoopItems,
    LoopDo,
    CaseSubject,
    CaseIn,
    /// Before a pattern list, or `esac`.
    CasePattern,
    /// Inside a pattern list; `true` right after a pattern.
    CasePatternWords(bool),
    CaseBody,
    Group,
    Subshell,
    FunctionName,
    /// After `function name`: `()` or the body.
    FunctionParens,
    /// The body compound must come next.
    FunctionBody,
    /// The body compound is open above this frame.
    FunctionAwait,
    FishBlock,
    FishIf,
    FishElse,
    FishForName,
    FishForIn,
    FishItems,
    /// A function's name and options.
    FishHeader,
    FishSwitchSubject,
    /// Between `switch` and the first `case`.
    FishSwitch,
    FishCasePatterns,
    FishCaseBody,
}

impl State {
    /// Words here belong to the compound rather than to a command.
    fn takes_words(self) -> bool {
        matches!(
            self,
            State::LoopName
                | State::LoopItems
                | State::CaseSubject
                | State::CasePatternWords(_)
                | State::FunctionName
                | State::FishForName
                | State::FishItems
                | State::FishHeader
                | State::FishSwitchSubject
                | State::FishCasePatterns
        )
    }

    /// Only particular reserved words may come next; newlines are skipped.
    fn expects_keyword(self) -> bool {
        matches!(
            self,
            State::LoopIn
                | State::LoopDo
                | State::CaseIn
                | State::CasePattern
                | State::FunctionParens
                | State::FunctionBody
                | State::FishForIn
                | State::FishSwitch
        )
    }
}

fn is_name(word: &str) -> bool {
    word.bytes().next().is_some_and(|c| !c.is_ascii_digit())
        && word.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
}

fn is_blank(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\r')
}

fn is_meta(c: u8) -> bool {
    is_blank(c) || matches!(c, b'\n' | b';' | b'&' | b'|' | b'(' | b')' | b'<' | b'>')
}

impl Parser<'_> {
    fn fish(&self) -> bool {
        self.dialect == Dialect::Fish
    }

    fn tcsh(&self) -> bool {
        self.dialect == Dialect::Tcsh
    }

    fn literal_ampersand(&self) -> bool {
        self.fish() && self.peek(0) == Some(b'&') && self.peek(1).is_some_and(|c| !is_meta(c))
    }

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
            if self.compound_token() {
                continue;
            }
            let c = self.b[self.i];
            let start = self.i;
            match c {
                c if is_blank(c) => self.i += 1,
                b'\\' if self.peek(1) == Some(b'\n') => self.i += 2,
                // Interactive tcsh has no comments: `#` is an ordinary character.
                b'#' if !self.tcsh() => {
                    let end = self.src[start..]
                        .find('\n')
                        .map_or(self.b.len(), |k| start + k);
                    self.comments.push(Span::new(start, end));
                    self.i = end;
                }
                b'\n' => self.end_command(Connector::Newline, 1),
                // Case terminators outside a case item.
                b';' if self.peek(1) == Some(b';') => {
                    self.diagnose(start, start + 2, DiagnosticKind::UnexpectedOperator);
                    self.i += 2;
                }
                b';' => self.end_command(Connector::Sequence, 1),
                b'&' if self.peek(1) == Some(b'&') => self.end_command(Connector::And, 2),
                b'&' if self.fish() && self.peek(1) == Some(b'|') => {
                    self.end_command(Connector::PipeAll, 2)
                }
                b'&' if !self.tcsh() && self.peek(1) == Some(b'>') => self.redirection(start),
                b'&' if self.literal_ampersand() => self.word(),
                b'&' => self.end_command(Connector::Background, 1),
                b'|' if self.peek(1) == Some(b'|') => self.end_command(Connector::Or, 2),
                b'|' if self.peek(1) == Some(b'&') => self.end_command(Connector::PipeAll, 2),
                // tcsh's history prints `|&` as `| &`.
                b'|' if self.tcsh() && self.peek(1).is_some_and(is_blank) => {
                    let len = 1 + self.b[self.i + 1..]
                        .iter()
                        .take_while(|&&c| is_blank(c))
                        .count();
                    if self.peek(len) == Some(b'&') && self.peek(len + 1) != Some(b'&') {
                        self.end_command(Connector::PipeAll, len + 1);
                    } else {
                        self.end_command(Connector::Pipe, 1);
                    }
                }
                b'|' => self.end_command(Connector::Pipe, 1),
                b'(' if self.fish() => self.word(),
                // tcsh subshells and `if ( expr )` conditions.
                b'(' | b')' if self.tcsh() => {
                    self.diagnose(start, start + 1, DiagnosticKind::CompoundCommand);
                    self.i += 1;
                }
                b'(' => self.open_paren(start),
                b')' => self.close_paren(start),
                b'{' if self.fish() && self.command_position() => {
                    self.diagnose(start, start + 1, DiagnosticKind::CompoundCommand);
                    self.i += 1;
                }
                b'>' if self.fish() && self.peek(1) == Some(b'|') => {
                    self.end_command(Connector::PipeFd(1), 2)
                }
                b'<' | b'>' if self.dialect == Dialect::Posix && self.peek(1) == Some(b'(') => {
                    self.word()
                }
                b'<' | b'>' => self.redirection(start),
                b'0'..=b'9' if self.fish() && self.fd_pipe_ahead() => {
                    let count = self.b[self.i..]
                        .iter()
                        .take_while(|c| c.is_ascii_digit())
                        .count();
                    match self.src[start..start + count].parse() {
                        Ok(fd) => self.end_command(Connector::PipeFd(fd), count + 2),
                        Err(_) => {
                            self.diagnose(
                                start,
                                start + count + 2,
                                DiagnosticKind::UnexpectedOperator,
                            );
                            self.i += count + 2;
                        }
                    }
                }
                // tcsh has no descriptor numbers: `2>x` is the word `2`, then `>x`.
                b'0'..=b'9' if !self.tcsh() && self.fd_redirection_ahead() => {
                    self.redirection(start)
                }
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

    fn fd_pipe_ahead(&self) -> bool {
        let digits = self.b[self.i..]
            .iter()
            .take_while(|c| c.is_ascii_digit())
            .count();
        self.peek(digits) == Some(b'>') && self.peek(digits + 1) == Some(b'|')
    }

    fn end_command(&mut self, connector: Connector, len: usize) {
        let span = Span::new(self.i, self.i + len);
        self.i += len;
        // A closed compound counts as the command before the operator.
        let closed = self.closed.take().is_some();
        let empty = self.current.is_empty() && !closed;
        // fish allows empty blocks and clauses: `begin; end`.
        let allowed = connector == Connector::Newline
            || self.fish() && self.after_keyword && connector == Connector::Sequence;
        if empty && !allowed || self.expect_command {
            self.diagnose(span.start, span.end, DiagnosticKind::UnexpectedOperator);
            self.expect_command = false;
        }
        if self.tcsh() && connector.is_pipe() && self.redirects(b'>') {
            // tcsh: "Ambiguous output redirect."
            self.diagnose(span.start, span.end, DiagnosticKind::UnexpectedOperator);
        }
        if !self.current.is_empty() {
            self.push_current();
        }
        // An empty line keeps the pending operator: `ls &&<newline>ls`.
        if !empty {
            self.connector = Some((connector, span));
        }
    }

    /// The next word would be a command name.
    fn command_position(&self) -> bool {
        self.current.is_empty()
            && self.closed.is_none()
            && !self
                .stack
                .last()
                .is_some_and(|f| f.state.takes_words() || f.state.expects_keyword())
    }

    /// Handles tokens whose meaning depends on the open compound: header
    /// separators, case pattern syntax, and function parentheses.
    fn compound_token(&mut self) -> bool {
        let Some(state) = self.stack.last().map(|f| f.state) else {
            return false;
        };
        let c = self.b[self.i];
        let start = self.i;
        if state == State::CaseBody && c == b';' {
            let terminator = [";;&", ";;", ";&", ";|"]
                .into_iter()
                .find(|op| self.src[start..].starts_with(op));
            if let Some(op) = terminator {
                let had_command = !self.current.is_empty();
                if had_command {
                    self.push_current();
                } else if self.closed.is_none() && self.dangling_operator() {
                    self.diagnose(start, start + op.len(), DiagnosticKind::UnexpectedOperator);
                }
                self.i += op.len();
                self.closed = None;
                self.keyword_in_frame(Span::new(start, self.i));
                self.set_state(State::CasePattern);
                // The next item's commands follow this one in sequence.
                self.connector = Some((Connector::Sequence, Span::new(start, self.i)));
                return true;
            }
        }
        if !(state.takes_words() || state.expects_keyword()) {
            return false;
        }
        match c {
            b'\n' | b';' => {
                self.i += 1;
                self.header_separator(state, Span::new(start, self.i));
                true
            }
            b'(' if state == State::CasePattern => {
                self.i += 1;
                self.keyword_in_frame(Span::new(start, self.i));
                self.set_state(State::CasePatternWords(false));
                true
            }
            b'|' if state == State::CasePatternWords(true) => {
                self.i += 1;
                self.set_state(State::CasePatternWords(false));
                true
            }
            b')' if state == State::CasePatternWords(true) => {
                self.i += 1;
                self.keyword_in_frame(Span::new(start, self.i));
                self.set_state(State::CaseBody);
                self.begin_clause();
                true
            }
            b'(' if state == State::FunctionParens && self.empty_parens_ahead() => {
                let close = self.empty_parens_end();
                self.keyword_in_frame(Span::new(start, start + 1));
                self.keyword_in_frame(Span::new(close - 1, close));
                self.i = close;
                self.set_state(State::FunctionBody);
                true
            }
            b'(' if matches!(state, State::FunctionParens | State::FunctionBody)
                && self.peek(1) != Some(b'(') =>
            {
                self.i += 1;
                self.open(
                    CompoundKind::Subshell,
                    Span::new(start, self.i),
                    State::Subshell,
                );
                true
            }
            c if is_blank(c) || c == b'#' || c == b'\\' && self.peek(1) == Some(b'\n') => false,
            // zsh's `for x (a b)` and other forms we don't read.
            c if is_meta(c) => {
                self.diagnose(start, start + 1, DiagnosticKind::CompoundCommand);
                false
            }
            _ => false,
        }
    }

    /// A `;` or newline inside a compound's header.
    fn header_separator(&mut self, state: State, span: Span) {
        let newline = span.of(self.src) == "\n";
        let words = self
            .stack
            .last()
            .map_or(0, |f| self.compounds[f.compound].words.len());
        match state {
            State::LoopIn
            | State::LoopDo
            | State::CaseIn
            | State::CasePattern
            | State::FunctionParens
            | State::FunctionBody
                if newline => {}
            State::FishSwitch => {}
            State::LoopIn | State::LoopItems => self.set_state(State::LoopDo),
            State::FishItems => {
                self.set_state(State::FishBlock);
                self.begin_clause();
            }
            State::FishHeader if words > 0 => {
                self.set_state(State::FishBlock);
                self.begin_clause();
            }
            State::FishSwitchSubject if words == 1 => self.set_state(State::FishSwitch),
            State::FishCasePatterns => {
                self.set_state(State::FishCaseBody);
                self.begin_clause();
            }
            _ => self.diagnose(span.start, span.end, DiagnosticKind::UnexpectedKeyword),
        }
    }

    /// A word inside a compound's header, or a reserved word it expects.
    fn compound_word(&mut self, state: State, word: Word) {
        let text = word.literal().filter(|_| !word.quoted).map(str::to_owned);
        let span = word.span;
        let Some(index) = self.stack.last().map(|f| f.compound) else {
            return;
        };
        match (state, text.as_deref()) {
            (State::LoopName | State::FishForName, text) => {
                if !text.is_some_and(is_name) {
                    self.diagnose(span.start, span.end, DiagnosticKind::UnexpectedKeyword);
                }
                self.compounds[index].words.push(word);
                self.set_state(if state == State::LoopName {
                    State::LoopIn
                } else {
                    State::FishForIn
                });
            }
            (State::LoopIn, Some("in")) => {
                self.keyword_in_frame(span);
                self.set_state(State::LoopItems);
            }
            (State::LoopIn | State::LoopDo, Some("do")) => {
                self.keyword_in_frame(span);
                self.set_state(State::LoopBody);
                self.begin_clause();
            }
            (State::FishForIn, Some("in")) => {
                self.keyword_in_frame(span);
                self.set_state(State::FishItems);
            }
            (
                State::LoopItems
                | State::FishItems
                | State::FishHeader
                | State::FishSwitchSubject
                | State::FishCasePatterns,
                _,
            ) => self.compounds[index].words.push(word),
            (State::CaseSubject, _) => {
                self.compounds[index].words.push(word);
                self.set_state(State::CaseIn);
            }
            (State::CaseIn, Some("in")) => {
                self.keyword_in_frame(span);
                self.set_state(State::CasePattern);
            }
            (State::CasePattern, Some("esac")) => self.close_frame(span),
            (State::CasePattern | State::CasePatternWords(false), _) => {
                self.compounds[index].words.push(word);
                self.set_state(State::CasePatternWords(true));
            }
            (State::FunctionName, text) => {
                if text.is_none() {
                    self.diagnose(span.start, span.end, DiagnosticKind::UnexpectedKeyword);
                }
                self.compounds[index].words.push(word);
                self.set_state(State::FunctionParens);
            }
            (
                State::FunctionParens | State::FunctionBody,
                Some(keyword @ ("{" | "if" | "while" | "until" | "for" | "select" | "case")),
            ) => {
                self.posix_keyword(keyword, span);
            }
            (State::FishSwitch, Some("case")) => {
                self.keyword_in_frame(span);
                self.set_state(State::FishCasePatterns);
            }
            (State::FishSwitch, Some("end")) => self.close_frame(span),
            _ => {
                self.diagnose(span.start, span.end, DiagnosticKind::UnexpectedKeyword);
                self.compounds[index].words.push(word);
            }
        }
    }

    /// Handles a reserved word in command position; `false` if `word` isn't one.
    fn keyword(&mut self, word: &Word) -> bool {
        let Some(text) = word.literal().filter(|_| !word.quoted) else {
            return false;
        };
        match self.dialect {
            Dialect::Fish => self.fish_keyword(text, word.span),
            Dialect::Posix => self.posix_keyword(text, word.span),
            // PowerShell lines have their own parser.
            Dialect::Tcsh | Dialect::PowerShell => false,
        }
    }

    fn posix_keyword(&mut self, text: &str, span: Span) -> bool {
        match text {
            "if" => self.open(CompoundKind::If, span, State::IfCondition),
            "while" => self.open(CompoundKind::While, span, State::LoopCondition),
            "until" => self.open(CompoundKind::Until, span, State::LoopCondition),
            "for" => self.open(CompoundKind::For, span, State::LoopName),
            "select" => self.open(CompoundKind::Select, span, State::LoopName),
            "case" => self.open(CompoundKind::Case, span, State::CaseSubject),
            "{" => self.open(CompoundKind::Group, span, State::Group),
            "function" => self.open(CompoundKind::Function, span, State::FunctionName),
            "!" => self.negate(span),
            "then" => self.next_clause(span, &[State::IfCondition], State::IfBody),
            "elif" => self.next_clause(span, &[State::IfBody], State::IfCondition),
            "else" => self.next_clause(span, &[State::IfBody], State::ElseBody),
            "do" => self.next_clause(span, &[State::LoopCondition], State::LoopBody),
            "fi" => self.close(span, &[State::IfBody, State::ElseBody]),
            "done" => self.close(span, &[State::LoopBody]),
            "esac" => self.close(span, &[State::CaseBody]),
            "}" => self.close(span, &[State::Group]),
            _ => return false,
        }
        true
    }

    fn fish_keyword(&mut self, text: &str, span: Span) -> bool {
        match text {
            "begin" => self.open(CompoundKind::Group, span, State::FishBlock),
            "if" if self.else_if(span) => {}
            "if" => self.open(CompoundKind::If, span, State::FishIf),
            "while" => self.open(CompoundKind::While, span, State::FishBlock),
            "for" => self.open(CompoundKind::For, span, State::FishForName),
            "switch" => self.open(CompoundKind::Case, span, State::FishSwitchSubject),
            "function" => self.open(CompoundKind::Function, span, State::FishHeader),
            "not" | "!" => self.negate(span),
            "else" => self.next_clause(span, &[State::FishIf], State::FishElse),
            "case" => self.next_clause(span, &[State::FishCaseBody], State::FishCasePatterns),
            "end" => self.close(
                span,
                &[
                    State::FishBlock,
                    State::FishIf,
                    State::FishElse,
                    State::FishCaseBody,
                ],
            ),
            _ => return false,
        }
        true
    }

    /// fish's `else if` on one line continues the same `if`.
    fn else_if(&mut self, span: Span) -> bool {
        let Some(frame) = self.stack.last() else {
            return false;
        };
        let follows_else = frame.state == State::FishElse
            && frame.clause == self.commands.len()
            && self.compounds[frame.compound]
                .keywords
                .last()
                .is_some_and(|k| {
                    k.of(self.src) == "else"
                        && self.b[k.end..span.start].iter().all(|&c| is_blank(c))
                });
        if follows_else {
            self.keyword_in_frame(span);
            self.set_state(State::FishIf);
            self.begin_clause();
            self.expect_command = true;
        }
        follows_else
    }

    fn open(&mut self, kind: CompoundKind, span: Span, state: State) {
        if let Some(frame) = self.stack.last_mut()
            && matches!(frame.state, State::FunctionParens | State::FunctionBody)
        {
            frame.state = State::FunctionAwait;
        }
        let n = self.commands.len();
        self.compounds.push(Compound {
            kind,
            span,
            keywords: vec![span],
            words: Vec::new(),
            redirections: Vec::new(),
            commands: n..n,
        });
        self.stack.push(Frame {
            compound: self.compounds.len() - 1,
            state,
            clause: n,
        });
        self.after_keyword = true;
        self.expect_command = self.fish() && matches!(kind, CompoundKind::If | CompoundKind::While);
    }

    fn negate(&mut self, span: Span) {
        let n = self.commands.len();
        self.compounds.push(Compound {
            kind: CompoundKind::Negation,
            span,
            keywords: vec![span],
            words: Vec::new(),
            redirections: Vec::new(),
            commands: n..n,
        });
        self.after_keyword = true;
        self.expect_command = true;
    }

    /// A clause-ending reserved word follows a separator, a closed
    /// compound, or another reserved word (whose clause is then empty).
    fn keyword_placed(&self) -> bool {
        self.closed.is_some()
            || self.after_keyword
            || matches!(
                self.connector,
                Some((
                    Connector::Sequence | Connector::Newline | Connector::Background,
                    _
                ))
            )
    }

    /// `a &&` before a closing word or `)`.
    fn dangling_operator(&self) -> bool {
        self.connector
            .is_some_and(|(c, _)| c.is_pipe() || matches!(c, Connector::And | Connector::Or))
            && !self.after_keyword
    }

    /// `then`, `else`, `do`, fish's `case`: ends one clause, starts the next.
    fn next_clause(&mut self, span: Span, from: &[State], to: State) {
        let placed = self.keyword_placed();
        let n = self.commands.len();
        match self.stack.last() {
            Some(frame) if from.contains(&frame.state) => {
                // POSIX clauses need a command; fish's may be empty, except
                // that a condition can't be.
                if !placed || n == frame.clause && (!self.fish() || self.expect_command) {
                    self.diagnose(span.start, span.end, DiagnosticKind::UnexpectedKeyword);
                }
                self.keyword_in_frame(span);
                self.set_state(to);
                self.begin_clause();
            }
            _ => self.diagnose(span.start, span.end, DiagnosticKind::UnexpectedKeyword),
        }
        self.closed = None;
        self.after_keyword = true;
    }

    /// `fi`, `done`, `esac`, `}`, fish's `end`.
    fn close(&mut self, span: Span, from: &[State]) {
        let placed = self.keyword_placed();
        let n = self.commands.len();
        match self.stack.last() {
            Some(frame) if from.contains(&frame.state) => {
                let empty_allowed = self.fish() || frame.state == State::CaseBody;
                if !placed || n == frame.clause && !empty_allowed {
                    self.diagnose(span.start, span.end, DiagnosticKind::UnexpectedKeyword);
                }
                self.close_frame(span);
            }
            _ => {
                self.diagnose(span.start, span.end, DiagnosticKind::UnexpectedKeyword);
                self.closed = None;
            }
        }
    }

    fn close_frame(&mut self, span: Span) {
        let Some(frame) = self.stack.pop() else {
            return;
        };
        let n = self.commands.len();
        let compound = &mut self.compounds[frame.compound];
        compound.keywords.push(span);
        compound.span.end = span.end;
        compound.commands.end = n;
        let mut closed = frame.compound;
        // A function definition ends with its body.
        if self
            .stack
            .last()
            .is_some_and(|f| f.state == State::FunctionAwait)
            && let Some(parent) = self.stack.pop()
        {
            let function = &mut self.compounds[parent.compound];
            function.span.end = span.end;
            function.commands.end = n;
            closed = parent.compound;
        }
        self.closed = Some(closed);
        self.after_keyword = false;
        self.expect_command = false;
    }

    fn keyword_in_frame(&mut self, span: Span) {
        if let Some(frame) = self.stack.last() {
            self.compounds[frame.compound].keywords.push(span);
        }
    }

    fn set_state(&mut self, state: State) {
        if let Some(frame) = self.stack.last_mut() {
            frame.state = state;
        }
    }

    fn begin_clause(&mut self) {
        let n = self.commands.len();
        if let Some(frame) = self.stack.last_mut() {
            frame.clause = n;
        }
        self.after_keyword = true;
    }

    /// `(` followed by optional blanks and `)`.
    fn empty_parens_ahead(&self) -> bool {
        self.peek(0) == Some(b'(')
            && self.b[self.i + 1..]
                .iter()
                .find(|&&c| !is_blank(c))
                .is_some_and(|&c| c == b')')
    }

    /// The offset just past the `)` of an empty `( )`.
    fn empty_parens_end(&self) -> usize {
        self.b[self.i + 1..]
            .iter()
            .position(|&c| c == b')')
            .map_or(self.b.len(), |k| self.i + 1 + k + 1)
    }

    /// A subshell, or `name()` starting a function definition.
    fn open_paren(&mut self, start: usize) {
        let span = Span::new(start, start + 1);
        if self.command_position() && self.peek(1) != Some(b'(') {
            self.i += 1;
            self.open(CompoundKind::Subshell, span, State::Subshell);
            return;
        }
        let defines_function = self.closed.is_none()
            && self.current.assignments.is_empty()
            && self.current.redirections.is_empty()
            && self.current.words.len() == 1
            && self.current.words[0].literal().is_some()
            && !self.current.words[0].quoted
            && self.empty_parens_ahead();
        if defines_function && let Some(name) = self.current.words.pop() {
            let close = self.empty_parens_end();
            self.i = close;
            self.open(CompoundKind::Function, span, State::FunctionBody);
            if let Some(function) = self.compounds.last_mut() {
                function.span.start = name.span.start;
                function.keywords.push(Span::new(close - 1, close));
                function.words.push(name);
            }
            return;
        }
        // Arithmetic `((...))`, zsh glob qualifiers, and the like.
        self.diagnose(start, start + 1, DiagnosticKind::CompoundCommand);
        self.i += 1;
    }

    fn close_paren(&mut self, start: usize) {
        self.i += 1;
        let span = Span::new(start, self.i);
        if !self
            .stack
            .last()
            .is_some_and(|f| f.state == State::Subshell)
        {
            self.diagnose(start, self.i, DiagnosticKind::UnexpectedKeyword);
            return;
        }
        let had_command = !self.current.is_empty();
        if had_command {
            self.push_current();
        } else if self.closed.is_none() && self.dangling_operator() {
            self.diagnose(start, self.i, DiagnosticKind::UnexpectedOperator);
        }
        if self
            .stack
            .last()
            .is_some_and(|f| f.clause == self.commands.len())
        {
            self.diagnose(start, self.i, DiagnosticKind::UnexpectedKeyword);
        }
        self.close_frame(span);
    }

    /// The current command redirects with an operator starting with `c`.
    fn redirects(&self, c: u8) -> bool {
        self.current
            .redirections
            .iter()
            .any(|r| self.b[r.operator.start] == c)
    }

    fn push_current(&mut self) {
        if self.tcsh() && self.redirects(b'<') && self.connector.is_some_and(|(c, _)| c.is_pipe()) {
            // tcsh: "Ambiguous input redirect."
            let span = self.current.redirections[0].span;
            self.diagnose(span.start, span.end, DiagnosticKind::UnexpectedOperator);
        }
        let mut command = std::mem::take(&mut self.current);
        if self.tcsh()
            && command.words.is_empty()
            && let Some(first) = command.redirections.first()
        {
            // tcsh: "Invalid null command."
            self.diagnose(
                first.span.start,
                first.span.end,
                DiagnosticKind::UnexpectedOperator,
            );
        }
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
        let closed = self.closed.take().is_some();
        if !self.current.is_empty() {
            self.push_current();
        } else if !closed
            && let Some((connector, span)) = self.connector
            && !matches!(
                connector,
                Connector::Sequence | Connector::Background | Connector::Newline
            )
        {
            self.diagnose(span.start, span.end, DiagnosticKind::UnexpectedOperator);
        }
        if self.expect_command {
            self.diagnose(
                self.b.len(),
                self.b.len(),
                DiagnosticKind::UnexpectedOperator,
            );
        }
        let n = self.commands.len();
        for frame in std::mem::take(&mut self.stack) {
            let compound = &mut self.compounds[frame.compound];
            compound.span.end = self.b.len();
            compound.commands.end = n;
            let start = compound.span.start;
            self.diagnose(start, self.b.len(), DiagnosticKind::UnterminatedCompound);
        }
    }

    fn word(&mut self) {
        let word = self.scan_word();
        if let Some(state) = self.stack.last().map(|f| f.state)
            && (state.takes_words() || state.expects_keyword())
        {
            self.compound_word(state, word);
            return;
        }
        if self.closed.is_some() {
            // Only the enclosing compound's next reserved word may follow a
            // closed one directly, as in `if { a; } then`.
            let continues = !self.fish()
                && !word.quoted
                && matches!(
                    word.literal(),
                    Some("then" | "elif" | "else" | "fi" | "do" | "done" | "esac" | "}")
                );
            if !continues || !self.keyword(&word) {
                self.diagnose(
                    word.span.start,
                    word.span.end,
                    DiagnosticKind::UnexpectedKeyword,
                );
                self.closed = None;
                self.current.words.push(word);
            }
            return;
        }
        let in_command_position = self.current.words.is_empty();
        if in_command_position {
            if self.current.assignments.is_empty()
                && self.current.redirections.is_empty()
                && self.keyword(&word)
            {
                return;
            }
            if self.fish() && !word.quoted && matches!(word.literal(), Some("and" | "or")) {
                if self.commands.is_empty()
                    || !self.current.is_empty()
                    || !self.connector.is_some_and(|(c, _)| {
                        matches!(
                            c,
                            Connector::Sequence | Connector::Newline | Connector::Background
                        )
                    })
                {
                    self.diagnose(
                        word.span.start,
                        word.span.end,
                        DiagnosticKind::UnexpectedOperator,
                    );
                }
                let connector = if word.literal() == Some("and") {
                    Connector::And
                } else {
                    Connector::Or
                };
                self.connector = Some((connector, word.span));
                return;
            }
            // tcsh has no `NAME=value command` prefix: it is a command name.
            if !self.tcsh()
                && is_assignment(&word, self.src)
                && (!self.fish()
                    || !word
                        .span
                        .of(self.src)
                        .split('=')
                        .next()
                        .unwrap_or("")
                        .ends_with('+'))
            {
                self.current.assignments.push(word);
                return;
            }
            // `[[`, `coproc`, a reserved word after an assignment, and
            // POSIX syntax typed into fish.
            if word.literal().is_some_and(|w| {
                !word.quoted
                    && match self.dialect {
                        Dialect::Tcsh => TCSH_RESERVED.contains(&w),
                        Dialect::Fish => {
                            RESERVED.contains(&w) || ["begin", "end", "switch", "not"].contains(&w)
                        }
                        Dialect::Posix => RESERVED.contains(&w),
                        Dialect::PowerShell => false,
                    }
            }) {
                self.diagnose(
                    word.span.start,
                    word.span.end,
                    DiagnosticKind::CompoundCommand,
                );
            }
        }
        self.after_keyword = false;
        self.expect_command = false;
        self.current.words.push(word);
    }

    fn redirection(&mut self, start: usize) {
        while self.peek(0).is_some_and(|c| c.is_ascii_digit()) {
            self.i += 1;
        }
        let rest = &self.src[self.i..];
        let operators = if self.fish() {
            &[
                "&>>", "&>", "<<<", "<<-", "<<", "<&", "<>", "<?", "<", ">>", ">&", ">?", ">",
            ][..]
        } else if self.tcsh() {
            &[">>", ">", "<<", "<"][..]
        } else {
            &[
                "&>>", "&>", "<<<", "<<-", "<<", "<&", "<>", "<", ">>", ">&", ">|", ">",
            ][..]
        };
        let operator = operators
            .iter()
            .copied()
            .find(|op| rest.starts_with(op))
            .unwrap_or("<");
        self.i += operator.len();
        if self.tcsh() && operator.starts_with('>') {
            // `&` adds stderr and `!` overrides noclobber. tcsh's history
            // prints the parts apart (`> & ! file`), and reads them so.
            for &part in b"&!" {
                let after_blanks = self.i
                    + self.b[self.i..]
                        .iter()
                        .take_while(|&&c| is_blank(c))
                        .count();
                if self.b.get(after_blanks) == Some(&part) {
                    self.i = after_blanks;
                    if part == b'!' && self.history_reference() {
                        self.diagnose(self.i, self.i + 1, DiagnosticKind::HistorySubstitution);
                    }
                    self.i += 1;
                }
            }
        }
        let operator_span = Span::new(start, self.i);
        if matches!(operator, "<<" | "<<-") || self.fish() && matches!(operator, "<<<" | "<>") {
            self.diagnose(start, self.i, DiagnosticKind::HereDocument);
        }
        while self.peek(0).is_some_and(is_blank) {
            self.i += 1;
        }
        let starts_word = match self.peek(0) {
            Some(b'<' | b'>') => self.peek(1) == Some(b'('),
            Some(b'(') if self.fish() => true,
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
        let redirection = Redirection {
            span: Span::new(start, end),
            operator: operator_span,
            target,
        };
        if let Some(index) = self.closed {
            let compound = &mut self.compounds[index];
            compound.span.end = end;
            compound.redirections.push(redirection);
        } else {
            self.after_keyword = false;
            self.expect_command = false;
            self.current.redirections.push(redirection);
        }
    }

    fn scan_word(&mut self) -> Word {
        let start = self.i;
        let mut value = Some(String::new());
        let mut quoted = false;
        let mut fish_delimiters = Vec::new();
        let push = |value: &mut Option<String>, s: &str| {
            if let Some(v) = value {
                v.push_str(s);
            }
        };
        while let Some(c) = self.peek(0) {
            match c {
                b'<' | b'>'
                    if self.dialect == Dialect::Posix
                        && self.i == start
                        && self.peek(1) == Some(b'(') =>
                {
                    self.i += 1;
                    self.substitution(b'(', b')');
                    value = None;
                }
                b'(' if self.fish() => {
                    self.substitution(b'(', b')');
                    while self.peek(0) == Some(b'[') {
                        self.substitution(b'[', b']');
                    }
                    value = None;
                }
                c if is_meta(c) && !self.literal_ampersand() => break,
                b'\\' if self.fish() => {
                    quoted = true;
                    self.fish_escape(&mut value);
                }
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
                    self.single_quoted(&mut value);
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
                b'`' if !self.fish() => {
                    self.backticks();
                    value = None;
                }
                b'!' if self.tcsh() => {
                    if self.history_reference() {
                        self.diagnose(self.i, self.i + 1, DiagnosticKind::HistorySubstitution);
                        value = None;
                    } else {
                        push(&mut value, "!");
                    }
                    self.i += 1;
                }
                b'*' | b'?' => {
                    self.i += 1;
                    value = None;
                }
                b'[' | b'{' if self.fish() => {
                    fish_delimiters.push((c, self.i));
                    push(&mut value, &self.src[self.i..self.i + 1]);
                    self.i += 1;
                }
                b']' | b'}' if self.fish() => {
                    let expected = if c == b']' { b'[' } else { b'{' };
                    if !fish_delimiters
                        .pop()
                        .is_some_and(|(open, _)| open == expected)
                    {
                        self.diagnose(self.i, self.i + 1, DiagnosticKind::InvalidExpansion);
                        value = None;
                    }
                    push(&mut value, &self.src[self.i..self.i + 1]);
                    self.i += 1;
                }
                b',' if self.fish() && fish_delimiters.iter().any(|(c, _)| *c == b'{') => {
                    value = None;
                    self.i += 1;
                }
                b'[' if !self.fish() && self.word_has_unquoted(b']') => {
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
                                || self.fish() && matches!(c, b']' | b'}' | b',')
                                || self.tcsh() && c == b'!'
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
        for (_, open) in fish_delimiters {
            self.diagnose(open, self.i, DiagnosticKind::UnterminatedSubstitution);
            value = None;
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
                // tcsh: a backslash only quotes `!` and newlines here.
                b'\\' if self.tcsh() => match self.peek(1) {
                    Some(e @ (b'!' | b'\n')) => {
                        if e == b'\n' {
                            self.diagnose(self.i, self.i + 2, DiagnosticKind::QuotedNewline);
                        }
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
                b'!' if self.tcsh() => {
                    if self.history_reference() {
                        self.diagnose(self.i, self.i + 1, DiagnosticKind::HistorySubstitution);
                        *value = None;
                    } else if let Some(v) = value {
                        v.push('!');
                    }
                    self.i += 1;
                }
                b'\\' => match self.peek(1) {
                    Some(b'\n') => self.i += 2,
                    Some(e @ (b'$' | b'`' | b'"' | b'\\')) if e != b'`' || !self.fish() => {
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
                b'`' if !self.fish() => {
                    self.backticks();
                    *value = None;
                }
                _ => {
                    let run = self.b[self.i..]
                        .iter()
                        .position(|&c| {
                            matches!(c, b'"' | b'\\' | b'$')
                                || c == b'`' && !self.fish()
                                || c == b'!' && self.tcsh()
                        })
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
        if self.tcsh() {
            match self.peek(1) {
                Some(c) if c.is_ascii_alphabetic() || c == b'_' => self.i += 1,
                Some(b'{') => {
                    self.i += 1;
                    self.substitution(b'{', b'}');
                    return Dollar::Expansion;
                }
                // `$?name`, `$#name`, `$<`, `$$`, `$0`.
                Some(b'?' | b'#') => self.i += 2,
                Some(c) if c.is_ascii_digit() || matches!(c, b'$' | b'<') => {
                    self.i += 2;
                    return Dollar::Expansion;
                }
                // "Illegal variable name.": `$`, `$(`, `$'`.
                _ => {
                    self.diagnose(self.i, self.i + 1, DiagnosticKind::InvalidExpansion);
                    self.i += 1;
                    return Dollar::Expansion;
                }
            }
            while self
                .peek(0)
                .is_some_and(|c| c.is_ascii_alphanumeric() || c == b'_')
            {
                self.i += 1;
            }
            // Subscripts and `:h`-style modifiers stay in the (opaque) word.
            return Dollar::Expansion;
        }
        if self.fish()
            && !self
                .peek(1)
                .is_some_and(|c| c.is_ascii_alphabetic() || matches!(c, b'_' | b'$' | b'('))
        {
            self.diagnose(self.i, self.i + 1, DiagnosticKind::InvalidExpansion);
            self.i += 1;
            return Dollar::Expansion;
        }
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
        if self.fish() {
            while self.peek(0) == Some(b'[') {
                self.substitution(b'[', b']');
            }
        }
        Dollar::Expansion
    }

    /// Skips a balanced `open ... close` region starting at `open`.
    fn substitution(&mut self, open: u8, close: u8) {
        let start = self.i;
        let mut depth = 0usize;
        while let Some(c) = self.peek(0) {
            match c {
                b'#' if self.fish() && self.i > start && is_meta(self.b[self.i - 1]) => {
                    self.i = self.src[self.i..]
                        .find('\n')
                        .map_or(self.b.len(), |offset| self.i + offset);
                    continue;
                }
                b'\\' => {
                    self.i += 2;
                    continue;
                }
                b'\'' => {
                    let mut ignored = None;
                    self.single_quoted(&mut ignored);
                    continue;
                }
                b'"' => {
                    self.i += 1;
                    let mut ignored = None;
                    self.double_quoted(&mut ignored);
                    continue;
                }
                b'`' if !self.fish() => {
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

    /// tcsh's `!` starts a history substitution unless a blank, newline,
    /// `=`, `(`, a quote, or the end of the word follows it.
    fn history_reference(&self) -> bool {
        self.peek(1)
            .is_some_and(|c| !is_meta(c) && !matches!(c, b'=' | b'"' | b'\''))
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
            body.contains(&b',') || !self.fish() && body.windows(2).any(|w| w == b"..")
        })
    }

    fn single_quoted(&mut self, value: &mut Option<String>) {
        let open = self.i;
        self.i += 1;
        while let Some(c) = self.peek(0) {
            if c == b'\'' {
                self.i += 1;
                return;
            }
            if self.tcsh() && (c == b'!' && self.history_reference() || c == b'\n') {
                let kind = if c == b'!' {
                    DiagnosticKind::HistorySubstitution
                } else {
                    DiagnosticKind::QuotedNewline
                };
                self.diagnose(self.i, self.i + 1, kind);
            }
            if self.tcsh() && c == b'\\' && matches!(self.peek(1), Some(b'!' | b'\n'))
                || self.fish() && c == b'\\' && matches!(self.peek(1), Some(b'\'' | b'\\'))
            {
                if self.tcsh() && self.peek(1) == Some(b'\n') {
                    self.diagnose(self.i, self.i + 2, DiagnosticKind::QuotedNewline);
                }
                if let Some(v) = value {
                    v.push(self.peek(1).unwrap() as char);
                }
                self.i += 2;
            } else {
                let ch = self.src[self.i..].chars().next().unwrap();
                if let Some(v) = value {
                    v.push(ch);
                }
                self.i += ch.len_utf8();
            }
        }
        self.diagnose(open, self.b.len(), DiagnosticKind::UnterminatedQuote);
        *value = None;
    }

    fn fish_escape(&mut self, value: &mut Option<String>) {
        let start = self.i;
        self.i += 1;
        let Some(c) = self.peek(0) else {
            self.diagnose(start, self.i, DiagnosticKind::TrailingEscape);
            *value = None;
            return;
        };
        if c == b'\n' {
            self.i += 1;
            return;
        }
        let decoded = match c {
            b'a' => Some('\x07'),
            b'b' => Some('\x08'),
            b'e' => Some('\x1b'),
            b'f' => Some('\x0c'),
            b'n' => Some('\n'),
            b'r' => Some('\r'),
            b't' => Some('\t'),
            b'v' => Some('\x0b'),
            b'x' | b'X' | b'u' | b'U' | b'0'..=b'7' => {
                let octal = c.is_ascii_digit();
                let width = if octal {
                    3
                } else {
                    match c {
                        b'u' => 4,
                        b'U' => 8,
                        _ => 2,
                    }
                };
                if !octal {
                    self.i += 1;
                }
                let digits_start = self.i;
                while self.i - digits_start < width
                    && self.peek(0).is_some_and(|d| {
                        if octal {
                            (b'0'..=b'7').contains(&d)
                        } else {
                            d.is_ascii_hexdigit()
                        }
                    })
                {
                    self.i += 1;
                }
                let number = u32::from_str_radix(
                    &self.src[digits_start..self.i],
                    if octal { 8 } else { 16 },
                )
                .ok();
                let decoded = number.and_then(char::from_u32);
                if decoded.is_none() || octal && number.is_some_and(|n| n > 255) {
                    self.diagnose(start, self.i, DiagnosticKind::InvalidEscape);
                    *value = None;
                } else if number == Some(0)
                    || (octal || matches!(c, b'x' | b'X')) && number.is_some_and(|n| n > 127)
                {
                    // Byte escapes can produce non-UTF-8 names. Keep them opaque.
                    *value = None;
                } else if let (Some(v), Some(ch)) = (value, decoded) {
                    v.push(ch);
                }
                return;
            }
            b'c' => {
                self.i += 1;
                let Some(letter) = self.peek(0).filter(u8::is_ascii_alphabetic) else {
                    self.diagnose(start, self.i, DiagnosticKind::InvalidEscape);
                    *value = None;
                    return;
                };
                self.i += 1;
                if let Some(v) = value {
                    v.push((letter.to_ascii_uppercase() & 31) as char);
                }
                return;
            }
            _ => self.src[self.i..].chars().next(),
        };
        if let Some(ch) = decoded {
            if let Some(v) = value {
                v.push(ch);
            }
            self.i += if c.is_ascii() { 1 } else { ch.len_utf8() };
        }
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

pub fn quote_word_with_dialect(value: &str, dialect: Dialect) -> String {
    if dialect == Dialect::Posix {
        return quote_word(value);
    }
    if dialect == Dialect::PowerShell {
        return powershell::quote(value);
    }
    if !value.is_empty()
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_@%+=:,./-".contains(&c))
    {
        return value.to_owned();
    }
    if dialect == Dialect::Tcsh {
        // Single quotes stop everything but history substitution and the
        // end of the line; `\!` and `\<newline>` are literal inside them.
        let mut quoted = String::from("'");
        for c in value.chars() {
            match c {
                '\'' => quoted.push_str("'\\''"),
                '!' => quoted.push_str("\\!"),
                '\n' => quoted.push_str("\\\n"),
                c => quoted.push(c),
            }
        }
        quoted.push('\'');
        return quoted;
    }
    format!("'{}'", value.replace('\\', "\\\\").replace('\'', "\\'"))
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
    fn fish_quotes_escapes_and_literals_keep_their_actual_meaning() {
        let source = r#"echo 'it\'s\\done' "a\`b" \n \x41 \u00e4 [abc] `printf` {1..3} a&b"#;
        let script = parse_with_dialect(source, Dialect::Fish);
        assert!(script.is_fully_supported(), "{:?}", script.diagnostics);
        assert_eq!(
            values(&script, 0),
            [
                Some("echo"),
                Some("it's\\done"),
                Some("a\\`b"),
                Some("\n"),
                Some("A"),
                Some("ä"),
                Some("[abc]"),
                Some("`printf`"),
                Some("{1..3}"),
                Some("a&b")
            ]
        );
        for word in &script.commands[0].words {
            assert!(!word.span.of(source).is_empty());
        }
        let script = parse_with_dialect(r"echo \xFF", Dialect::Fish);
        assert!(script.is_fully_supported());
        assert_eq!(
            values(&script, 0),
            [Some("echo"), None],
            "non-UTF-8 bytes stay opaque"
        );
        let script = parse_with_dialect(r"echo \377", Dialect::Fish);
        assert!(script.is_fully_supported());
        assert_eq!(values(&script, 0), [Some("echo"), None]);
    }

    #[test]
    fn fish_operators_assignments_and_redirections_preserve_boundaries() {
        let source = r#"A='x\'y' git sttus | cat >? 'out\'s.txt'; and echo "[literal]"; or echo '(not run)' # note"#;
        let script = parse_with_dialect(source, Dialect::Fish);
        assert!(script.is_fully_supported(), "{:?}", script.diagnostics);
        assert_eq!(script.commands.len(), 4);
        assert_eq!(script.commands[0].assignments[0].literal(), Some("A=x'y"));
        assert_eq!(script.commands[1].connector, Some(Connector::Pipe));
        assert_eq!(script.commands[2].connector, Some(Connector::And));
        assert_eq!(script.commands[3].connector, Some(Connector::Or));
        assert_eq!(script.commands[1].redirections[0].operator.of(source), ">?");
        assert_eq!(
            script.commands[1].redirections[0]
                .target
                .as_ref()
                .unwrap()
                .literal(),
            Some("out's.txt")
        );
        assert_eq!(script.comments[0].of(source), "# note");
        for (source, connector) in [
            ("echo x 2>| cat", Connector::PipeFd(2)),
            ("echo x >| cat", Connector::PipeFd(1)),
            ("echo x &| cat", Connector::PipeAll),
        ] {
            let script = parse_with_dialect(source, Dialect::Fish);
            assert!(
                script.is_fully_supported(),
                "{source}: {:?}",
                script.diagnostics
            );
            assert_eq!(script.commands[1].connector, Some(connector));
            assert!(script.commands[0].redirections.is_empty());
            assert_eq!(script.pipeline_of(0), 0..2);
        }
    }

    #[test]
    fn fish_substitutions_and_variable_slices_are_opaque_and_blocks_are_explicit() {
        let source = r#"echo (printf 'x\'y') $(printf x) "$HOME" "$(printf x)" {a,b} $items[1 2]"#;
        let script = parse_with_dialect(source, Dialect::Fish);
        assert!(script.is_fully_supported(), "{:?}", script.diagnostics);
        assert_eq!(
            values(&script, 0),
            [Some("echo"), None, None, None, None, None, None]
        );
        for source in [
            "echo 'oops",
            "echo (printf x",
            "echo \\x",
            "echo \\xG",
            "echo \\uZZZZ",
            "echo \\400",
            "echo {a,b",
            "echo ${HOME}",
            "echo }",
            "echo *[",
            "echo (true))",
            "true; and",
            "echo $?",
        ] {
            assert!(
                !parse_with_dialect(source, Dialect::Fish).is_fully_supported(),
                "{source}"
            );
        }
        for source in [
            "echo (printf x # ) is a comment\n)",
            "echo (printf x)[1 2]",
            "cat <(printf x)",
            "echo '[unbalanced' \\{",
        ] {
            let parsed = parse_with_dialect(source, Dialect::Fish);
            assert!(
                parsed.is_fully_supported(),
                "{source}: {:?}",
                parsed.diagnostics
            );
        }
    }

    #[test]
    fn fish_quoting_round_trips_and_never_turns_literal_text_into_expansion() {
        for value in [
            "",
            "a'b",
            "a\\b",
            "$(touch marker)",
            "(touch marker)",
            "`touch marker`",
            "a&b",
            "{a,b}",
            "ümlaut",
            "a\nb",
            "[abc]",
        ] {
            let source = format!("echo {}", quote_word_with_dialect(value, Dialect::Fish));
            let parsed = parse_with_dialect(&source, Dialect::Fish);
            assert!(
                parsed.is_fully_supported(),
                "{source}: {:?}",
                parsed.diagnostics
            );
            assert_eq!(values(&parsed, 0), [Some("echo"), Some(value)], "{source}");
            #[cfg(unix)]
            if let Some(fish) = crate::utils::which("fish") {
                let source = format!(
                    "printf '%s' {}",
                    quote_word_with_dialect(value, Dialect::Fish)
                );
                let output = std::process::Command::new(fish)
                    .args(["--no-config", "--private", "-c", &source])
                    .output()
                    .unwrap();
                assert!(output.status.success(), "{source}: {:?}", output.stderr);
                assert_eq!(String::from_utf8(output.stdout).unwrap(), value, "{source}");
            }
        }
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
            ("[[ -f x ]] && ls", DiagnosticKind::CompoundCommand),
            ("(( x++ ))", DiagnosticKind::CompoundCommand),
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

    /// Compound kinds, each command's name, and every compound word.
    fn outline(script: &Script) -> (Vec<CompoundKind>, Vec<String>, Vec<String>) {
        (
            script.compounds.iter().map(|c| c.kind).collect(),
            script
                .commands
                .iter()
                .map(|c| script.word_text(&c.words[0]))
                .collect(),
            script
                .compounds
                .iter()
                .flat_map(|c| &c.words)
                .map(|w| script.word_text(w))
                .collect(),
        )
    }

    /// Whether the installed shell accepts `source` without running it.
    #[cfg(unix)]
    fn shell_accepts(shell: &str, source: &str) -> Option<bool> {
        let path = crate::utils::which(shell)?;
        let args: &[&str] = match shell {
            "fish" => &["--no-config", "--private", "-n", "-c"],
            "zsh" => &["-f", "-n", "-c"],
            "tcsh" => &["-f", "-n", "-c"],
            _ => &["--norc", "--noprofile", "-n", "-c"],
        };
        let output = std::process::Command::new(path)
            .args(args)
            .arg(source)
            .output()
            .ok()?;
        // Not the status: under -n, zsh's `! cmd` still inverts a zero.
        Some(output.stderr.is_empty())
    }

    /// A source, its compound kinds, command names, and compound words.
    type Outline = (
        &'static str,
        &'static [CompoundKind],
        &'static [&'static str],
        &'static [&'static str],
    );

    const POSIX_COMPOUNDS: &[Outline] = {
        use CompoundKind::*;
        &[
            (
                "for f in a \"b c\" $x; do cat \"$f\"; done",
                &[For],
                &["cat"],
                &["f", "a", "b c", "\0$x"],
            ),
            ("for x; do :; done", &[For], &[":"], &["x"]),
            ("for x\nin a\ndo :\ndone", &[For], &[":"], &["x", "a"]),
            ("for x do :; done", &[For], &[":"], &["x"]),
            (
                "select x in a b; do break; done",
                &[Select],
                &["break"],
                &["x", "a", "b"],
            ),
            (
                "while read -r x; do echo \"$x\"; done < list",
                &[While],
                &["read", "echo"],
                &[],
            ),
            ("until false; do :; done", &[Until], &["false", ":"], &[]),
            (
                "case $1 in a|b) git sttus;; (c) ;; *) ls;& esac",
                &[Case],
                &["git", "ls"],
                &["\0$1", "a", "b", "c", "\0*"],
            ),
            ("case x in esac", &[Case], &[], &["x"]),
            ("case a in a) esac", &[Case], &[], &["a", "a"]),
            ("{ git sttus; } | cat", &[Group], &["git", "cat"], &[]),
            (
                "(cd x && make) || echo failed",
                &[Subshell],
                &["cd", "make", "echo"],
                &[],
            ),
            ("f() { git sttus; }", &[Function, Group], &["git"], &["f"]),
            (
                "f () if true; then :; fi",
                &[Function, If],
                &["true", ":"],
                &["f"],
            ),
            ("function g { ls; }", &[Function, Group], &["ls"], &["g"]),
            ("function h() { ls; }", &[Function, Group], &["ls"], &["h"]),
            ("{ { a; } }", &[Group, Group], &["a"], &[]),
            ("if { a; } then b; fi", &[If, Group], &["a", "b"], &[]),
            ("if (a) then b; fi", &[If, Subshell], &["a", "b"], &[]),
            ("! git sttus | grep x", &[Negation], &["git", "grep"], &[]),
            ("if true & then echo; fi", &[If], &["true", "echo"], &[]),
            (
                "x && if a; then b; fi; y",
                &[If],
                &["x", "a", "b", "y"],
                &[],
            ),
            (
                "while true; do if a; then break; fi; done",
                &[While, If],
                &["true", "a", "break"],
                &[],
            ),
        ]
    };

    #[test]
    fn posix_compound_commands_keep_simple_commands_and_their_syntax() {
        let src =
            "if git sttus; then echo ok; elif false; then :; else ls; fi > out 2>&1 && echo done";
        let s = parse(src);
        assert!(s.is_fully_supported(), "{:?}", s.diagnostics);
        assert_eq!(
            outline(&s).1,
            ["git", "echo", "false", ":", "ls", "echo"],
            "reserved words are not command words"
        );
        assert_eq!(values(&s, 0), [Some("git"), Some("sttus")]);
        assert_eq!(s.commands[5].connector, Some(Connector::And));
        let shape = &s.compound_shapes()[0];
        assert_eq!(shape.kind, CompoundKind::If);
        assert_eq!(shape.keywords, ["if", "then", "elif", "then", "else", "fi"]);
        assert_eq!(shape.redirections, ["> out", "2>& 1"]);
        assert_eq!(shape.commands, 0..5);
        assert_eq!(
            s.compounds[0].span.of(src),
            "if git sttus; then echo ok; elif false; then :; else ls; fi > out 2>&1"
        );
        assert!(s.commands[4].redirections.is_empty());

        let s = parse("while true; do if a; then break; fi; done");
        assert_eq!(s.compounds[0].commands, 0..3);
        assert_eq!(s.compounds[1].commands, 1..3);
        let s = parse("{ git sttus; } | cat");
        assert_eq!(s.commands[1].connector, Some(Connector::Pipe));
        assert_eq!(s.pipeline_of(1), 0..2);

        for (src, kinds, commands, words) in POSIX_COMPOUNDS {
            let s = parse(src);
            assert!(s.is_fully_supported(), "{src:?}: {:?}", s.diagnostics);
            let (k, c, w) = outline(&s);
            let c: Vec<&str> = c.iter().map(String::as_str).collect();
            let w: Vec<&str> = w.iter().map(String::as_str).collect();
            assert_eq!(
                (&k[..], &c[..], &w[..]),
                (*kinds, *commands, *words),
                "{src:?}"
            );
        }

        let src = "f() { git sttus; } > log";
        let s = parse(src);
        let target = s.commands[0].words[1].span;
        let edited = apply_edits(
            src,
            &[Edit {
                span: target,
                replacement: "status".into(),
            }],
        )
        .unwrap();
        assert_eq!(edited, "f() { git status; } > log");
        assert_eq!(
            parse(&edited).compound_shapes(),
            s.compound_shapes(),
            "an edit inside a body leaves the syntax around it alone"
        );
    }

    /// Malformed input, then valid forms notypo doesn't read (zsh-only
    /// syntax, arithmetic, `[[`, coprocesses).
    const POSIX_REJECTED: (&[&str], &[&str]) = (
        &[
            "if true; then fi",
            "fi",
            "then",
            "if true then echo; fi",
            "{ a; } b",
            "{ a; } { b; }",
            "( )",
            "(a &&)",
            "while true; do a;",
            "case x in a) b",
            "for x in a; :; done",
            "esac",
            "}",
            ")",
            "echo a ;; echo b",
            "if ; then a; fi",
            "if a; then b && fi",
            "case x in a b) c;; esac",
            "case x a) c;; esac",
            "f() ls",
            "f();",
        ],
        &[
            "!",
            "for 1x in a; do :; done",
            "{ ls }",
            "if true; then; ls; fi",
            "{ a; } always { b; }",
            "for x (a b) echo $x",
            "for ((i=0; i<3; i++)); do :; done",
            "A=1 if true; then :; fi",
            "coproc cat",
            "[[ -f x ]]",
        ],
    );

    #[test]
    fn malformed_and_unread_posix_compounds_are_diagnosed() {
        for src in POSIX_REJECTED.0.iter().chain(POSIX_REJECTED.1) {
            assert!(!parse(src).is_fully_supported(), "{src:?}");
        }
        assert!(
            parse("if true; then fi")
                .diagnostics
                .iter()
                .any(|d| d.kind == DiagnosticKind::UnexpectedKeyword)
        );
        assert!(
            parse("while true; do a;")
                .diagnostics
                .iter()
                .any(|d| d.kind == DiagnosticKind::UnterminatedCompound)
        );
    }

    const FISH_COMPOUNDS: &[Outline] = {
        use CompoundKind::*;
        &[
            (
                "begin; git sttus; end | cat",
                &[Group],
                &["git", "cat"],
                &[],
            ),
            (
                "if test -f x; echo a; else if test -d x; echo b; else; echo c; end",
                &[If],
                &["test", "echo", "test", "echo", "echo"],
                &[],
            ),
            ("while true; break; end", &[While], &["true", "break"], &[]),
            (
                "for x in a b; echo $x; end > out",
                &[For],
                &["echo"],
                &["x", "a", "b"],
            ),
            (
                "switch $x; case a b; echo y; case '*'; echo z; end",
                &[Case],
                &["echo", "echo"],
                &["\0$x", "a", "b", "*"],
            ),
            (
                "function f --description 'a b'; git sttus; end",
                &[Function],
                &["git"],
                &["f", "--description", "a b"],
            ),
            ("not git sttus", &[Negation], &["git"], &[]),
            ("! git sttus", &[Negation], &["git"], &[]),
            ("begin; end", &[Group], &[], &[]),
            ("if true; end", &[If], &["true"], &[]),
            ("if false; else end", &[If], &["false"], &[]),
            ("for x in; end", &[For], &[], &["x"]),
            ("switch a; end", &[Case], &[], &["a"]),
            ("if a; else; if b; end; end", &[If, If], &["a", "b"], &[]),
            (
                "true; and begin; echo x; end",
                &[Group],
                &["true", "echo"],
                &[],
            ),
        ]
    };

    const FISH_REJECTED: (&[&str], &[&str]) = (
        &[
            "if; end",
            "switch; end",
            "begin; end foo",
            "else; echo x",
            "begin; case x; end",
            "function; end",
            "switch a; echo bad; end",
            "if\ntrue; end",
            "not\ntrue",
            "for x a; end",
            "begin; git sttus",
            "if a; else; if b; end",
            "end",
        ],
        &["{ echo a; }"],
    );

    #[test]
    fn fish_blocks_keep_simple_commands_and_their_syntax() {
        let src = "if test -f x; echo a; else if test -d x; echo b; else; echo c; end";
        let s = parse_with_dialect(src, Dialect::Fish);
        assert_eq!(
            s.compound_shapes()[0].keywords,
            ["if", "else", "if", "else", "end"],
            "`else if` continues the same block"
        );
        for (src, kinds, commands, words) in FISH_COMPOUNDS {
            let s = parse_with_dialect(src, Dialect::Fish);
            assert!(s.is_fully_supported(), "{src:?}: {:?}", s.diagnostics);
            let (k, c, w) = outline(&s);
            let c: Vec<&str> = c.iter().map(String::as_str).collect();
            let w: Vec<&str> = w.iter().map(String::as_str).collect();
            assert_eq!(
                (&k[..], &c[..], &w[..]),
                (*kinds, *commands, *words),
                "{src:?}"
            );
        }
        for src in FISH_REJECTED.0.iter().chain(FISH_REJECTED.1) {
            assert!(
                !parse_with_dialect(src, Dialect::Fish).is_fully_supported(),
                "{src:?}"
            );
        }
    }

    /// The parser's view of what is valid agrees with the installed shells:
    /// every form it reads parses there, and every form it calls malformed
    /// is a syntax error there too.
    #[cfg(unix)]
    #[test]
    fn compound_syntax_agrees_with_installed_shells() {
        for (src, ..) in POSIX_COMPOUNDS {
            for shell in ["bash", "zsh"] {
                assert_ne!(shell_accepts(shell, src), Some(false), "{shell}: {src:?}");
            }
        }
        for src in POSIX_REJECTED.0 {
            assert_ne!(shell_accepts("bash", src), Some(true), "bash: {src:?}");
        }
        for (src, ..) in FISH_COMPOUNDS {
            assert_ne!(shell_accepts("fish", src), Some(false), "fish: {src:?}");
        }
        for src in FISH_REJECTED.0 {
            assert_ne!(shell_accepts("fish", src), Some(true), "fish: {src:?}");
        }
    }

    #[test]
    fn tcsh_words_operators_and_redirections_follow_tcsh() {
        let src = "echo ls 2>x ; grep a |& cat >& log && FOO=1 make # no comment; ls >>! out";
        let s = parse_with_dialect(src, Dialect::Tcsh);
        assert!(s.is_fully_supported(), "{:?}", s.diagnostics);
        #[cfg(unix)]
        assert_ne!(shell_accepts("tcsh", src), Some(false));
        assert_eq!(values(&s, 0), [Some("echo"), Some("ls"), Some("2")]);
        assert_eq!(s.commands[0].redirections[0].operator.of(src), ">");
        assert_eq!(s.commands[2].connector, Some(Connector::PipeAll));
        assert_eq!(s.commands[2].redirections[0].operator.of(src), ">&");
        assert_eq!(
            values(&s, 3),
            [
                Some("FOO=1"),
                Some("make"),
                Some("#"),
                Some("no"),
                Some("comment")
            ],
            "no prefix assignments and no comments in interactive tcsh"
        );
        assert!(s.commands[3].assignments.is_empty() && s.comments.is_empty());
        assert_eq!(s.commands[4].redirections[0].operator.of(src), ">>!");

        // tcsh's `history` prints operators apart, and tcsh reads them so.
        let src =
            "echo hi > & x ; echo hi | & cat ; echo hi >> & ! y ; echo a > ! z ; cat < w | cat";
        let s = parse_with_dialect(src, Dialect::Tcsh);
        assert!(s.is_fully_supported(), "{:?}", s.diagnostics);
        let operators: Vec<&str> = s
            .commands
            .iter()
            .flat_map(|c| &c.redirections)
            .map(|r| r.operator.of(src))
            .collect();
        assert_eq!(operators, ["> &", ">> & !", "> !", "<"]);
        assert_eq!(s.commands[2].connector, Some(Connector::PipeAll));
        assert_eq!(s.commands[6].connector, Some(Connector::Pipe));
        for ambiguous in ["echo a > z | cat", "echo a | cat < z"] {
            assert!(
                !parse_with_dialect(ambiguous, Dialect::Tcsh).is_fully_supported(),
                "{ambiguous}"
            );
            #[cfg(unix)]
            assert_ne!(shell_accepts("tcsh", ambiguous), Some(true), "{ambiguous}");
        }
        assert!(
            parse_with_dialect("echo hi >!x", Dialect::Tcsh)
                .diagnostics
                .iter()
                .any(|d| d.kind == DiagnosticKind::HistorySubstitution),
            "`!x` right after `>` is a history reference"
        );
        #[cfg(unix)]
        assert_ne!(shell_accepts("tcsh", src), Some(false));

        let s = parse_with_dialect(
            r#"printf '[%s]' 'a\!b' x\!y "q\!r" 'it'\''s' "p\q" 'p\q' hi! a!=b 'hi!' "x!""#,
            Dialect::Tcsh,
        );
        assert!(s.is_fully_supported(), "{:?}", s.diagnostics);
        assert_eq!(
            values(&s, 0),
            [
                Some("printf"),
                Some("[%s]"),
                Some("a!b"),
                Some("x!y"),
                Some("q!r"),
                Some("it's"),
                Some("p\\q"),
                Some("p\\q"),
                Some("hi!"),
                Some("a!=b"),
                Some("hi!"),
                Some("x!")
            ]
        );
        let s = parse_with_dialect(
            "echo $HOME ${x} $?x $#argv $< `date` *.rs ~/x {a,b} $x:q $x[1]",
            Dialect::Tcsh,
        );
        assert!(s.is_fully_supported(), "{:?}", s.diagnostics);
        assert!(values(&s, 0)[1..].iter().all(Option::is_none));

        for (src, kind) in [
            ("if ( -e x ) echo", DiagnosticKind::CompoundCommand),
            ("foreach f ( * )", DiagnosticKind::CompoundCommand),
            ("(cd x; ls)", DiagnosticKind::CompoundCommand),
            ("echo $(date)", DiagnosticKind::InvalidExpansion),
            ("echo $", DiagnosticKind::InvalidExpansion),
            ("cat <<EOF", DiagnosticKind::HereDocument),
            ("> f", DiagnosticKind::UnexpectedOperator),
            ("ls &> f", DiagnosticKind::UnexpectedOperator),
            ("echo 'a!b'", DiagnosticKind::HistorySubstitution),
            ("echo x!y", DiagnosticKind::HistorySubstitution),
            ("echo \"q!r\"", DiagnosticKind::HistorySubstitution),
            ("echo !!", DiagnosticKind::HistorySubstitution),
            ("echo 'a\\\nb'", DiagnosticKind::QuotedNewline),
            ("echo 'a", DiagnosticKind::UnterminatedQuote),
        ] {
            let s = parse_with_dialect(src, Dialect::Tcsh);
            assert!(
                s.diagnostics.iter().any(|d| d.kind == kind),
                "{src:?}: {:?}",
                s.diagnostics
            );
        }
    }

    /// Words notypo writes for tcsh read back as the same value, both typed
    /// at an interactive prompt and through `eval` of backquoted output, as
    /// the alias runs a correction.
    #[test]
    fn tcsh_quoting_round_trips_through_the_parser_and_real_tcsh() {
        let values = [
            "",
            "a b",
            "a'b",
            "a\\b",
            "$(touch marker)",
            "`touch marker`",
            "a&b",
            "{a,b}",
            "ümlaut",
            "a!b",
            "a\\!b",
            "!!",
            "x!",
            "[abc]",
            "*",
            "~x",
            "#x",
            "$HOME",
            "a\"b",
        ];
        let mut lines = String::new();
        for value in values {
            let quoted = quote_word_with_dialect(value, Dialect::Tcsh);
            let source = format!("printf '[%s]\\n' {quoted}");
            let parsed = parse_with_dialect(&source, Dialect::Tcsh);
            assert!(
                parsed.is_fully_supported(),
                "{source}: {:?}",
                parsed.diagnostics
            );
            assert_eq!(
                parsed.commands[0].words[2].literal(),
                Some(value),
                "{source}"
            );
            lines.push_str(&source);
            lines.push('\n');
        }
        assert!(
            !parse_with_dialect(
                &format!("echo {}", quote_word_with_dialect("a\nb", Dialect::Tcsh)),
                Dialect::Tcsh
            )
            .is_fully_supported(),
            "a newline can't pass through the alias"
        );
        #[cfg(unix)]
        if let Some(tcsh) = crate::utils::which("tcsh") {
            let dir = std::env::temp_dir().join(format!("notypo-tcsh-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let commands = dir.join("commands");
            std::fs::write(&commands, &lines).unwrap();
            let input = format!(
                "set prompt=''\nset history=100\n{lines}foreach i (`seq {}`)\neval \"`sed -n ${{i}}p {}`\"\nend\nexit\n",
                values.len(),
                commands.display()
            );
            let mut child = std::process::Command::new(tcsh)
                .args(["-f", "-i"])
                .current_dir(&dir)
                .env_clear()
                .env("PATH", "/usr/bin:/bin")
                .env("HOME", &dir)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            use std::io::Write as _;
            child
                .stdin
                .take()
                .unwrap()
                .write_all(input.as_bytes())
                .unwrap();
            let output = child.wait_with_output().unwrap();
            let _ = std::fs::remove_dir_all(&dir);
            let stdout = String::from_utf8_lossy(&output.stdout);
            let printed: Vec<&str> = stdout
                .lines()
                .map(|l| l.trim_start_matches("> "))
                .filter_map(|l| l.strip_prefix('[')?.strip_suffix(']'))
                .collect();
            let expected: Vec<&str> = values.iter().chain(&values).copied().collect();
            assert_eq!(
                printed,
                expected,
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
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
