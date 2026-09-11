use alloc::borrow::ToOwned;
use alloc::string::String;

use crate::span::Span;

/// Kinds of lexical tokens.
///
/// Bash's lexical grammar is context-sensitive: characters like `(`, `)`,
/// `{`, `}` are operators in some positions and part of a word in others.
/// The lexer resolves this using context flags set by the parser.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    /// A command word or reserved word. The raw source spans the token.
    Word,
    /// `NAME=value` at the start of a command (may be an array assignment
    /// when followed by `(`).
    Assignment,
    /// A numeric fd prefix before a redirection operator (`2>`, `3<`).
    ///
    /// Only produced when the following char is a redirection operator; the
    /// raw text includes the number.
    IoNumber,

    // --- operators ---
    /// `&&`
    AndAnd,
    /// `||`
    OrOr,
    /// `|`
    Pipe,
    /// `|&`
    PipeAmp,
    /// `;`
    Semi,
    /// `;;`, `;&`, `;;&`
    SemiSemi,
    /// `&`
    Amp,
    /// `&>` (stdout+stderr to file)
    AmpGreat,
    /// `&>>`
    AmpGreatGreat,
    /// `(`
    Lparen,
    /// `)`
    Rparen,
    /// `{` as a reserved word (only at command position).
    Lbrace,
    /// `}` as a reserved word.
    Rbrace,
    /// `!`
    Bang,
    /// `[[` (only at command position).
    DoubleBracketOpen,
    /// `]]`
    DoubleBracketClose,
    /// `((` (only at command position).
    ArithOpen,
    /// `))` (at command position end).
    ArithClose,

    // --- redirection operators ---
    /// `>`
    Greater,
    /// `>&`
    GreaterAnd,
    /// `>>`
    GreatGreat,
    /// `>|` (overwrite, ignore noclobber)
    GreatBar,
    /// `<`
    Less,
    /// `<&`
    LessAnd,
    /// `<<`
    LessLess,
    /// `<<<`
    LessLessLess,
    /// `<>`
    LessGreat,
    /// `<<>>`
    LessGreatGreat,
    /// `<<-`
    LessLessDash,

    /// A newline that terminates a command.
    Newline,
    /// End of input.
    Eof,
}

impl TokenKind {
    /// Whether this token can start a command.
    pub fn can_start_command(self) -> bool {
        matches!(
            self,
            TokenKind::Word
                | TokenKind::Assignment
                | TokenKind::Bang
                | TokenKind::Lparen
                | TokenKind::Lbrace
                | TokenKind::DoubleBracketOpen
                | TokenKind::ArithOpen
                | TokenKind::IoNumber
        )
    }

    /// The literal text of an operator token, if any.
    pub fn operator_text(self) -> Option<&'static str> {
        Some(match self {
            TokenKind::AndAnd => "&&",
            TokenKind::OrOr => "||",
            TokenKind::Pipe => "|",
            TokenKind::PipeAmp => "|&",
            TokenKind::Semi => ";",
            TokenKind::SemiSemi => ";;",
            TokenKind::Amp => "&",
            TokenKind::AmpGreat => "&>",
            TokenKind::AmpGreatGreat => "&>>",
            TokenKind::Lparen => "(",
            TokenKind::Rparen => ")",
            TokenKind::Lbrace => "{",
            TokenKind::Rbrace => "}",
            TokenKind::Bang => "!",
            TokenKind::DoubleBracketOpen => "[[",
            TokenKind::DoubleBracketClose => "]]",
            TokenKind::ArithOpen => "((",
            TokenKind::ArithClose => "))",
            TokenKind::Greater => ">",
            TokenKind::GreaterAnd => ">&",
            TokenKind::GreatGreat => ">>",
            TokenKind::GreatBar => ">|",
            TokenKind::Less => "<",
            TokenKind::LessAnd => "<&",
            TokenKind::LessLess => "<<",
            TokenKind::LessLessLess => "<<<",
            TokenKind::LessGreat => "<>",
            TokenKind::LessGreatGreat => "<<>>",
            TokenKind::LessLessDash => "<<-",
            _ => return None,
        })
    }
}

/// A lexical token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
    /// For `Word`/`Assignment` tokens: the raw source text.
    pub text: String,
}

impl Token {
    pub fn new(kind: TokenKind, span: Span, text: String) -> Self {
        Self { kind, span, text }
    }

    pub fn operator(kind: TokenKind, span: Span) -> Self {
        let text = kind.operator_text().unwrap_or("").to_owned();
        Self::new(kind, span, text)
    }
}
