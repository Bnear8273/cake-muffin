//! Typed AST for the bash grammar.
//!
//! Every node carries a [`Span`] into the source text so the syntax
//! highlighter and error reporter can locate it.

use alloc::boxed::Box;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::RefCell;

use crate::span::Span;

/// How a complete command is terminated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Separator {
    /// `;`
    Semi,
    /// `&`
    Amp,
    /// newline or end of input
    Newline,
}

/// A whole program: zero or more complete commands.
#[derive(Debug, Clone)]
pub struct Program {
    pub commands: Vec<CompleteCommand>,
    pub span: Span,
}

/// A complete command: an and-or list plus its terminator.
#[derive(Debug, Clone)]
pub struct CompleteCommand {
    pub list: AndOrList,
    pub separator: Separator,
    pub span: Span,
}

/// A command list (the body of compound commands): zero or more and-or
/// chains separated by `;`/`&`/newline, terminated by a keyword.
#[derive(Debug, Clone)]
pub struct List {
    pub items: Vec<AndOrList>,
    pub span: Span,
}

/// `pipeline (&& pipeline | || pipeline)*`
#[derive(Debug, Clone)]
pub struct AndOrList {
    pub first: Pipeline,
    pub rest: Vec<(AndOrOp, Pipeline)>,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AndOrOp {
    AndAnd,
    OrOr,
}

/// `[!] command (| command)*`
#[derive(Debug, Clone)]
pub struct Pipeline {
    pub negated: bool,
    pub commands: Vec<Command>,
    pub span: Span,
}

/// One command: a simple or compound command plus its redirections.
#[derive(Debug, Clone)]
pub struct Command {
    pub kind: CommandKind,
    pub redirects: Vec<Redirect>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum CommandKind {
    Simple(SimpleCommand),
    If(IfCommand),
    For(ForCommand),
    /// C-style `for (( expr1; expr2; expr3 )); do body; done`
    CStyleFor(CStyleForCommand),
    While(WhileCommand),
    Until(WhileCommand),
    Case(CaseCommand),
    /// `select name [in words]; do body; done`
    Select(SelectCommand),
    /// `coproc [name] command`
    Coproc(CoprocCommand),
    Function(FunctionCommand),
    /// `{ ...; }`
    Block(BlockCommand),
    /// `( ... )`
    Subshell(SubshellCommand),
    /// `(( ... ))`
    Arith(ArithCommand),
    /// `[[ ... ]]`
    Cond(CondCommand),
    /// An empty command (e.g. just redirections).
    Empty,
}

/// A simple command: optional assignments prefix, then words.
#[derive(Debug, Clone)]
pub struct SimpleCommand {
    pub assignments: Vec<Assignment>,
    pub words: Vec<Word>,
    pub span: Span,
}

/// `NAME=value` or `NAME=(...)` assignment.
#[derive(Debug, Clone)]
pub struct Assignment {
    pub name: String,
    pub value: AssignmentValue,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum AssignmentValue {
    /// `a=b`
    Word(Word),
    /// `a=(x y z)` array assignment
    Array(Vec<Word>),
    /// `a[expr]=value` indexed assignment
    Index { index: Word, value: Word },
}

#[derive(Debug, Clone)]
pub struct IfCommand {
    pub clauses: Vec<IfClause>,
    pub else_body: Option<List>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct IfClause {
    pub cond: AndOrList,
    pub body: List,
}

#[derive(Debug, Clone)]
pub struct ForCommand {
    pub var: String,
    /// `None` means `for var; do` == `for var in "$@"`.
    pub in_words: Option<Vec<Word>>,
    pub body: List,
    pub span: Span,
}

/// C-style `for (( expr1; expr2; expr3 )); do body; done`.
#[derive(Debug, Clone)]
pub struct CStyleForCommand {
    /// The initializer expression (raw text, e.g. `"i=0"`). Empty string if omitted.
    pub init: String,
    /// The condition expression (raw text, e.g. `"i < 10"`). Empty string if omitted
    /// (treated as always-true, matching bash).
    pub cond: String,
    /// The increment expression (raw text, e.g. `"i++"`). Empty string if omitted.
    pub incr: String,
    pub body: List,
    pub span: Span,
}

/// `select name [in words]; do body; done` — interactive menu selection.
#[derive(Debug, Clone)]
pub struct SelectCommand {
    pub var: String,
    /// `None` means `select var; do` == `select var in "$@"`.
    pub in_words: Option<Vec<Word>>,
    pub body: List,
    pub span: Span,
}

/// `coproc [name] command` — start a coprocess.
#[derive(Debug, Clone)]
pub struct CoprocCommand {
    /// Optional name; `None` means the default variable name "COPROC".
    pub name: Option<String>,
    /// The command to run (a simple or compound command).
    pub body: Box<Command>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct WhileCommand {
    pub cond: AndOrList,
    pub body: List,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct CaseCommand {
    pub word: Word,
    pub arms: Vec<CaseArm>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct CaseArm {
    pub patterns: Vec<Word>,
    pub body: List,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct FunctionCommand {
    pub name: String,
    pub body: Box<Command>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct BlockCommand {
    pub body: List,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct SubshellCommand {
    pub body: List,
    pub span: Span,
}

/// `(( ... ))`: the raw arithmetic text, parsed in M2.
#[derive(Debug, Clone)]
pub struct ArithCommand {
    pub text: String,
    pub span: Span,
}

/// `[[ ... ]]`: the raw conditional text, parsed in M2.
#[derive(Debug, Clone)]
pub struct CondCommand {
    pub text: String,
    pub span: Span,
}

/// A redirection: `[N]<op><target>`.
#[derive(Debug, Clone)]
pub struct Redirect {
    /// Explicit fd prefix, e.g. the `2` in `2>file`.
    pub fd: Option<u32>,
    pub kind: RedirectKind,
    pub target: RedirectTarget,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RedirectKind {
    /// `>`
    Write,
    /// `>>`
    Append,
    /// `<`
    Read,
    /// `<>`
    ReadWrite,
    /// `<&`
    DupInput,
    /// `>&`
    DupOutput,
    /// `<<` / `<<-`
    Heredoc,
    /// `<<<`
    HereString,
    /// `>|`
    Clobber,
    /// `&>` (stdout+stderr)
    AndOut,
    /// `&>>`
    AndAppend,
}

#[derive(Debug, Clone)]
pub enum RedirectTarget {
    /// A filename (expanded at execution time).
    Word(Word),
    /// `N>&M` — duplicate to another fd.
    Fd(u32),
    /// `>&-` or `<&-` — close the fd.
    Close,
    /// `<<EOF`. The body is filled in by the parser once the command line is
    /// complete (it may appear after the current line's terminator).
    Heredoc {
        delimiter: Word,
        strip_tabs: bool,
        body: Rc<RefCell<Option<String>>>,
    },
    /// `<<<word`
    HereString(Word),
}

/// A shell word: a sequence of parts produced by expansion-aware scanning.
#[derive(Debug, Clone)]
pub struct Word {
    pub parts: Vec<WordPart>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum WordPart {
    /// Plain unquoted text (may contain glob characters).
    Literal(String, Span),
    /// `'...'`
    SingleQuoted(String, Span),
    /// `"..."` with nested parts.
    DoubleQuoted(Vec<WordPart>, Span),
    /// `$'...'` ANSI-C quoted.
    AnsiCQuoted(String, Span),
    /// `$name` or `${...}`.
    Parameter(Parameter, Span),
    /// `$(...)` or `` `...` `` (body stored raw, parsed in M2).
    CommandSubst(String, Span),
    /// `$((...))` (body stored raw, parsed in M2).
    ArithExpansion(String, Span),
    /// Leading `~` / `~user`.
    Tilde(String, Span),
    /// Brace expansion `{a,b}` / `{1..5}` (raw, expanded in M2).
    Brace(String, Span),
    /// `<(cmd)` / `>(cmd)` process substitution (raw, M2).
    ProcessSubst(String, Span),
}

/// A parameter expansion.
#[derive(Debug, Clone)]
pub struct Parameter {
    /// The variable name: `var`, a digit for positional (`$1`), or a special
    /// char (`?`, `$`, `#`, `@`, `*`, `!`).
    pub name: String,
    /// Whether it used the `${...}` form.
    pub braced: bool,
    /// The raw text (`$name` or `${...}`), for M2's full parsing.
    pub text: String,
}
