//! Recursive-descent parser for the bash grammar.
//!
//! Produces a typed AST ([`Program`]) with source spans. The parser drives
//! the [`Lexer`](crate::lexer::Lexer)'s context flags and handles heredoc
//! collection (body lines are read after the command line's terminating
//! newline).

use alloc::borrow::ToOwned;
use alloc::boxed::Box;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::RefCell;

use crate::ast::*;
use crate::error::ParseError;
use crate::lexer::{LexContext, Lexer};
use crate::span::Span;
use crate::token::{Token, TokenKind};
use crate::word::parse_word;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Parse `src` as a shell program.
pub fn parse(src: &str) -> Result<Program, Vec<ParseError>> {
    let mut lexer = Lexer::new(src);
    let mut parser = Parser::new(&mut lexer, src);
    parser.parse_program()
}

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

struct Parser<'a> {
    src: &'a str,
    lexer: &'a mut Lexer<'a>,
    current: Token,
    peeked: Option<Token>,
    errors: Vec<ParseError>,
    pending_heredocs: Vec<PendingHeredoc>,
}

struct PendingHeredoc {
    delimiter: String,
    strip_tabs: bool,
    slot: Rc<RefCell<Option<String>>>,
}

impl<'a> Parser<'a> {
    fn new(lexer: &'a mut Lexer<'a>, src: &'a str) -> Self {
        let mut p = Self {
            src,
            lexer,
            current: Token::new(TokenKind::Eof, Span::UNKNOWN, String::new()),
            peeked: None,
            errors: Vec::new(),
            pending_heredocs: Vec::new(),
        };
        p.lexer.ctx.cmd_pos = true;
        p.advance();
        // Skip leading newlines.
        while p.current.kind == TokenKind::Newline {
            p.advance();
        }
        p
    }

    // --- token helpers ---

    fn advance(&mut self) {
        self.current = self.peeked.take().unwrap_or_else(|| {
            self.lexer
                .next_token()
                .unwrap_or_else(|e| {
                    self.errors.push(e);
                    Token::new(TokenKind::Eof, Span::UNKNOWN, String::new())
                })
        });
    }

    fn peek(&mut self) -> TokenKind {
        if self.peeked.is_none() {
            let t = self
                .lexer
                .next_token()
                .unwrap_or_else(|e| {
                    self.errors.push(e);
                    Token::new(TokenKind::Eof, Span::UNKNOWN, String::new())
                });
            self.peeked = Some(t);
        }
        self.peeked.as_ref().unwrap().kind
    }

    fn set_cmd(&mut self) {
        self.lexer.ctx = LexContext {
            cmd_pos: true,
            ..Default::default()
        };
    }

    fn set_arg(&mut self) {
        self.lexer.ctx = LexContext::default();
    }

    fn set_cond(&mut self) {
        self.lexer.ctx = LexContext {
            in_cond: true,
            ..Default::default()
        };
    }

    fn set_arith(&mut self) {
        self.lexer.ctx = LexContext {
            in_arith: true,
            ..Default::default()
        };
    }

    /// Parse a word token into a [`Word`] AST node.
    fn word_from_token(&self, tok: &Token) -> Word {
        parse_word(&tok.text, tok.span)
    }

    // --- main entry ---

    fn parse_program(&mut self) -> Result<Program, Vec<ParseError>> {
        if !self.errors.is_empty() {
            return Err(core::mem::take(&mut self.errors));
        }
        let start = self.current.span.start;
        let mut commands = Vec::new();

        loop {
            // Skip stray newlines.
            while self.current.kind == TokenKind::Newline {
                self.advance();
            }
            if self.current.kind == TokenKind::Eof {
                break;
            }
            match self.parse_complete_command() {
                Ok(cmd) => commands.push(cmd),
                Err(()) => {
                    // Recover: skip to the next command boundary.
                    self.lexer.ctx = LexContext::default();
                    loop {
                        match self.current.kind {
                            TokenKind::Newline | TokenKind::Semi | TokenKind::Eof => break,
                            _ => self.advance(),
                        }
                    }
                }
            }
        }

        if !self.errors.is_empty() {
            Err(core::mem::take(&mut self.errors))
        } else {
            Ok(Program {
                commands,
                span: Span::new(start, self.src.len() as u32),
            })
        }
    }

    fn parse_complete_command(&mut self) -> Result<CompleteCommand, ()> {
        self.set_cmd();
        let start = self.current.span.start;
        let list = self.parse_and_or_list()?;
        let separator = match self.current.kind {
            TokenKind::Semi => {
                self.advance();
                Separator::Semi
            }
            TokenKind::Amp => {
                self.advance();
                Separator::Amp
            }
            TokenKind::Newline => {
                // The lexer is positioned right after the newline — the
                // ideal spot to read pending heredoc bodies. Do this before
                // advancing, so `current` doesn't consume the first body line.
                if !self.pending_heredocs.is_empty() {
                    self.flush_heredocs();
                }
                self.advance();
                Separator::Newline
            }
            TokenKind::Eof => {
                if !self.pending_heredocs.is_empty() {
                    self.flush_heredocs();
                }
                Separator::Newline
            }
            _ => {
                self.errors.push(ParseError::new(
                    alloc::format!("expected `;`, `&`, or newline, got `{}`", self.current.text),
                    self.current.span,
                ));
                return Err(());
            }
        };

        Ok(CompleteCommand {
            list,
            separator,
            span: Span::new(start, self.current.span.start),
        })
    }

    fn parse_and_or_list(&mut self) -> Result<AndOrList, ()> {
        self.set_cmd();
        let start = self.current.span.start;
        let first = self.parse_pipeline()?;
        let mut rest = Vec::new();

        loop {
            self.set_cmd();
            let op = match self.current.kind {
                TokenKind::AndAnd => AndOrOp::AndAnd,
                TokenKind::OrOr => AndOrOp::OrOr,
                _ => break,
            };
            self.advance();
            let pipeline = self.parse_pipeline()?;
            rest.push((op, pipeline));
        }

        Ok(AndOrList {
            first,
            rest,
            span: Span::new(start, self.current.span.start),
        })
    }

    /// Parse a command list (compound command body): one or more and-or
    /// chains separated by `;`/`&`/newline, stopping when `stop` matches the
    /// current token (e.g. `done`, `fi`, `}`).
    fn parse_list(&mut self, stop: impl Fn(&Token) -> bool) -> Result<List, ()> {
        let start = self.current.span.start;
        let mut items = Vec::new();
        loop {
            // Skip separators between chains.
            while matches!(self.current.kind, TokenKind::Semi | TokenKind::Newline) {
                self.advance();
            }
            if stop(&self.current) || self.current.kind == TokenKind::Eof {
                break;
            }
            if self.current.kind == TokenKind::Rbrace || self.current.kind == TokenKind::Rparen {
                break;
            }
            let chain = self.parse_and_or_list()?;
            items.push(chain);
        }
        Ok(List {
            items,
            span: Span::new(start, self.current.span.start),
        })
    }

    fn parse_pipeline(&mut self) -> Result<Pipeline, ()> {
        self.set_cmd();
        let start = self.current.span.start;
        let mut negated = false;

        if self.current.kind == TokenKind::Bang {
            negated = true;
            self.advance();
        }

        let mut commands = Vec::new();
        commands.push(self.parse_command()?);

        loop {
            self.set_cmd();
            match self.current.kind {
                TokenKind::Pipe => {
                    self.advance();
                    commands.push(self.parse_command()?);
                }
                TokenKind::PipeAmp => {
                    self.advance();
                    // |& sends both stdout and stderr — we treat it as a
                    // regular pipe for now.
                    commands.push(self.parse_command()?);
                }
                _ => break,
            }
        }

        Ok(Pipeline {
            negated,
            commands,
            span: Span::new(start, self.current.span.start),
        })
    }

    fn parse_command(&mut self) -> Result<Command, ()> {
        self.set_cmd();
        let start = self.current.span.start;

        let kind = if let Some(k) = self.try_compound_command()? {
            k
        } else {
            CommandKind::Simple(self.parse_simple_command()?)
        };

        // Collect redirections after the command.
        let redirects = self.parse_redirect_list();

        Ok(Command {
            kind,
            redirects,
            span: Span::new(start, self.current.span.start),
        })
    }

    // --- compound commands ---

    fn try_compound_command(&mut self) -> Result<Option<CommandKind>, ()> {
        self.set_cmd();
        Ok(match self.current.kind {
            TokenKind::Lbrace => Some(self.parse_block()?),
            TokenKind::Lparen => Some(self.parse_subshell()?),
            TokenKind::ArithOpen => Some(self.parse_arith()?),
            TokenKind::DoubleBracketOpen => Some(self.parse_cond()?),
            _ => {
                let kw = self.current.text.as_str();
                match kw {
                    "if" => Some(self.parse_if()?),
                    "for" => Some(self.parse_for()?),
                    "while" => Some(self.parse_while_or_until(false)?),
                    "until" => Some(self.parse_while_or_until(true)?),
                    "case" => Some(self.parse_case()?),
                    "function" => Some(self.parse_function()?),
                    "{" => Some(self.parse_block()?),
                    _ => {
                        // Check for 'name ()' function definition.
                        if self.current.kind == TokenKind::Word
                            && self.peek() == TokenKind::Lparen
                            && self.current.text.chars().all(|c| c.is_alphanumeric() || c == '_')
                        {
                            Some(self.parse_function()?)
                        } else {
                            None
                        }
                    }
                }
            }
        })
    }

    fn parse_if(&mut self) -> Result<CommandKind, ()> {
        self.advance(); // consume 'if'
        let start = self.current.span.start;
        let mut clauses = Vec::new();

        // 'if' condition
        let cond = self.parse_and_or_list()?;
        self.skip_optional_sep();
        self.expect_keyword("then")?;
        let body = self.parse_list(|t| {
            t.kind == TokenKind::Word
                && matches!(t.text.as_str(), "else" | "elif" | "fi")
        })?;
        clauses.push(IfClause { cond, body });

        // 'elif' clauses
        loop {
            self.skip_optional_sep();
            self.set_cmd();
            if self.current.kind == TokenKind::Word && self.current.text == "elif" {
                self.advance();
                let cond = self.parse_and_or_list()?;
                self.skip_optional_sep();
                self.expect_keyword("then")?;
                let body = self.parse_list(|t| {
                    t.kind == TokenKind::Word
                        && matches!(t.text.as_str(), "else" | "elif" | "fi")
                })?;
                clauses.push(IfClause { cond, body });
            } else {
                break;
            }
        }

        // 'else' clause
        self.skip_optional_sep();
        let else_body = if self.current.kind == TokenKind::Word && self.current.text == "else" {
            self.advance();
            self.skip_optional_sep();
            Some(self.parse_list(|t| {
                t.kind == TokenKind::Word && t.text == "fi"
            })?)
        } else {
            None
        };

        self.skip_optional_sep();
        self.expect_keyword("fi")?;

        Ok(CommandKind::If(IfCommand {
            clauses,
            else_body,
            span: Span::new(start, self.current.span.start),
        }))
    }

    fn parse_for(&mut self) -> Result<CommandKind, ()> {
        self.advance(); // consume 'for'
        let start = self.current.span.start;

        // Variable name.
        let var = self.expect_word()?;

        // Optional 'in word ...'
        let in_words = if self.current.kind == TokenKind::Word && self.current.text == "in" {
            self.advance();
            let mut words = Vec::new();
            while self.current.kind == TokenKind::Word {
                words.push(self.word_from_token(&self.current));
                self.advance();
            }
            Some(words)
        } else {
            None
        };

        // The user may omit the `in` list entirely. In that case `for var; do`
        // is equivalent to `for var in "$@"`. The semicolon (or newline) is
        // part of the `in`-less form and doesn't need special handling here.

        // 'do' body 'done'
        // A `;` before `do` is optional.
        self.skip_optional_sep();
        self.expect_keyword("do")?;
        let body = self.parse_list(|t| {
            t.kind == TokenKind::Word && t.text == "done"
        })?;
        self.skip_optional_sep();
        self.expect_keyword("done")?;

        Ok(CommandKind::For(ForCommand {
            var,
            in_words,
            body,
            span: Span::new(start, self.current.span.start),
        }))
    }

    fn parse_while_or_until(&mut self, is_until: bool) -> Result<CommandKind, ()> {
        self.advance(); // consume 'while'/'until'
        let start = self.current.span.start;
        let cond = self.parse_and_or_list()?;
        self.skip_optional_sep();
        self.expect_keyword("do")?;
        let body = self.parse_list(|t| {
            t.kind == TokenKind::Word && t.text == "done"
        })?;
        self.skip_optional_sep();
        self.expect_keyword("done")?;

        let cmd = if is_until {
            CommandKind::Until(WhileCommand { cond, body, span: Span::new(start, self.current.span.start) })
        } else {
            CommandKind::While(WhileCommand { cond, body, span: Span::new(start, self.current.span.start) })
        };
        Ok(cmd)
    }

    fn parse_case(&mut self) -> Result<CommandKind, ()> {
        self.advance(); // consume 'case'
        let start = self.current.span.start;
        let word = self.parse_word_primary()?;
        self.expect_keyword("in")?;

        let mut arms = Vec::new();
        // case arms end with `esac`. Patterns are separated by `|`.
        // Each arm: pattern ) body ;;
        loop {
            // Must be set BEFORE fetching any arm token: with in_case false,
            // a pattern like `*)` would swallow the `)` into one word.
            self.lexer.ctx.in_case = true;
            self.lexer.ctx.cmd_pos = true;
            // Skip newlines before arms.
            while self.current.kind == TokenKind::Newline {
                self.advance();
            }
            if self.current.kind == TokenKind::Word && self.current.text == "esac" {
                self.advance();
                break;
            }
            if self.current.kind == TokenKind::Eof {
                self.lexer.ctx.in_case = false;
                self.errors.push(ParseError::incomplete("unexpected EOF in case statement", self.current.span.start));
                return Err(());
            }

            let arm_start = self.current.span.start;
            // Patterns: word (| word)*
            let mut patterns = Vec::new();
            patterns.push(self.parse_word_primary()?);
            while self.current.kind == TokenKind::Pipe {
                self.advance();
                patterns.push(self.parse_word_primary()?);
            }
            // Expect `)` after patterns.
            if self.current.kind != TokenKind::Rparen {
                self.errors.push(ParseError::new("expected `)` after case pattern", self.current.span));
                return Err(());
            }
            self.advance(); // consume )
            // Body: list (stops at ;;)
            let body = self.parse_list(|t| t.kind == TokenKind::SemiSemi)?;
            // Expect `;;`.
            if self.current.kind != TokenKind::SemiSemi {
                self.errors.push(ParseError::new("expected `;;` after case arm", self.current.span));
                return Err(());
            }
            self.advance(); // consume ;;
            arms.push(CaseArm {
                patterns,
                body,
                span: Span::new(arm_start, self.current.span.start),
            });
        }
        self.lexer.ctx.in_case = false;

        Ok(CommandKind::Case(CaseCommand {
            word,
            arms,
            span: Span::new(start, self.current.span.start),
        }))
    }

    fn parse_function(&mut self) -> Result<CommandKind, ()> {
        let start = self.current.span.start;
        let name = if self.current.kind == TokenKind::Word && self.current.text == "function" {
            self.advance();
            self.expect_word()?
        } else {
            // name () { ... }
            let n = self.expect_word()?;
            self.set_cmd();
            if self.current.kind == TokenKind::Lparen {
                self.advance();
                if self.current.kind == TokenKind::Rparen {
                    self.advance();
                } else {
                    self.errors.push(ParseError::new(
                        "expected `()` after function name",
                        self.current.span,
                    ));
                    return Err(());
                }
            }
            n
        };
        let body = self.parse_command()?;
        Ok(CommandKind::Function(FunctionCommand {
            name,
            body: Box::new(body),
            span: Span::new(start, self.current.span.start),
        }))
    }

    fn parse_block(&mut self) -> Result<CommandKind, ()> {
        let start = self.current.span.start;
        self.advance(); // consume {
        let body = self.parse_list(|t| t.kind == TokenKind::Rbrace)?;
        // Expect }
        self.set_cmd();
        if self.current.kind != TokenKind::Rbrace {
            if self.current.kind == TokenKind::Eof {
                self.errors
                    .push(ParseError::incomplete("unexpected EOF, expected `}`", self.current.span.start));
            } else {
                self.errors.push(ParseError::new("expected `}`", self.current.span));
            }
            return Err(());
        }
        self.advance();
        Ok(CommandKind::Block(BlockCommand {
            body,
            span: Span::new(start, self.current.span.start),
        }))
    }

    fn parse_subshell(&mut self) -> Result<CommandKind, ()> {
        let start = self.current.span.start;
        self.advance(); // consume (
        let body = self.parse_list(|t| t.kind == TokenKind::Rparen)?;
        self.set_cmd();
        if self.current.kind != TokenKind::Rparen {
            if self.current.kind == TokenKind::Eof {
                self.errors
                    .push(ParseError::incomplete("unexpected EOF, expected `)`", self.current.span.start));
            } else {
                self.errors.push(ParseError::new("expected `)`", self.current.span));
            }
            return Err(());
        }
        self.advance();
        Ok(CommandKind::Subshell(SubshellCommand {
            body,
            span: Span::new(start, self.current.span.start),
        }))
    }

    fn parse_arith(&mut self) -> Result<CommandKind, ()> {
        let start = self.current.span.start;
        self.advance(); // consume ((
        // Collect tokens until )) using in_arith context.
        let tok_start = self.current.span.start;
        self.set_arith();
        let mut depth: i32 = 0;
        loop {
            self.advance();
            match self.current.kind {
                TokenKind::Lparen => depth += 1,
                TokenKind::Rparen => {
                    if depth == 0 {
                        // Check for closing ))
                        if self.peek() == TokenKind::Rparen {
                            let text = self.src[tok_start as usize..self.current.span.start as usize].to_owned();
                            self.advance(); // consume first )
                            let span = Span::new(start, self.current.span.start);
                            self.advance(); // consume token after ))
                            return Ok(CommandKind::Arith(ArithCommand { text, span }));
                        }
                        // Single ) inside arithmetic, not balanced.
                        depth -= 1;
                    } else {
                        depth -= 1;
                    }
                }
                TokenKind::Eof => {
                    self.errors
                        .push(ParseError::incomplete("unexpected EOF in arithmetic command", self.current.span.start));
                    let text = self.src[tok_start as usize..self.current.span.start as usize].to_owned();
                    return Ok(CommandKind::Arith(ArithCommand { text, span: Span::new(start, self.current.span.start) }));
                }
                _ => {}
            }
        }
    }

    fn parse_cond(&mut self) -> Result<CommandKind, ()> {
        let start = self.current.span.start;
        self.advance(); // consume [[
        let tok_start = self.current.span.start;
        self.set_cond();
        // Collect tokens until ]] at the outermost level.
        loop {
            self.advance();
            match self.current.kind {
                TokenKind::DoubleBracketClose => {
                    let text = self.src[tok_start as usize..self.current.span.start as usize].to_owned();
                    self.advance(); // consume ]]
                    return Ok(CommandKind::Cond(CondCommand {
                        text,
                        span: Span::new(start, self.current.span.start),
                    }));
                }
                TokenKind::Eof => {
                    self.errors.push(ParseError::incomplete(
                        "unexpected EOF in conditional expression",
                        self.current.span.start,
                    ));
                    let text = self.src[tok_start as usize..self.current.span.start as usize].to_owned();
                    return Ok(CommandKind::Cond(CondCommand { text, span: Span::new(start, self.current.span.start) }));
                }
                _ => {}
            }
        }
    }

    // --- simple command ---

    fn parse_simple_command(&mut self) -> Result<SimpleCommand, ()> {
        let start = self.current.span.start;
        let mut assignments = Vec::new();
        let mut words = Vec::new();

        // Collect prefix assignments and the command word.
        loop {
            self.set_cmd();
            match self.current.kind {
                TokenKind::Assignment => {
                    let a = self.parse_assignment()?;
                    assignments.push(a);
                    // Continue for more prefix assignments.
                }
                TokenKind::Word => {
                    // Check if this is a function definition: name followed
                    // by `()` before `{` or any other body.
                    if self.peek() == TokenKind::Lparen {
                        // Let the caller handle function definition via
                        // try_compound_command. Return what we have so far.
                        break;
                    }
                    words.push(self.word_from_token(&self.current));
                    self.advance();
                    // Once we have a command word, remaining tokens are args.
                    break;
                }
                TokenKind::IoNumber => {
                    // An io number at command start is a redirect.
                    break;
                }
                _ => break,
            }
        }

        // Parse arguments and interleaved redirections.
        loop {
            self.set_arg();
            match self.current.kind {
                TokenKind::Word => {
                    words.push(self.word_from_token(&self.current));
                    self.advance();
                }
                TokenKind::Assignment => {
                    // Assignment in non-command position is a word.
                    words.push(self.word_from_token(&self.current));
                    self.advance();
                }
                _ => break,
            }
        }

        Ok(SimpleCommand {
            assignments,
            words,
            span: Span::new(start, self.current.span.start),
        })
    }

    fn parse_assignment(&mut self) -> Result<Assignment, ()> {
        let start = self.current.span.start;
        let text = self.current.text.clone();
        let eq_pos = text.find('=').unwrap();
        let name = text[..eq_pos].to_owned();

        let value = if self.peek() == TokenKind::Lparen {
            // Array assignment: a=(x y z)
            self.advance(); // consume = (already in text)
            self.advance(); // consume (
            let mut words = Vec::new();
            self.set_arg();
            loop {
                match self.current.kind {
                    TokenKind::Word => {
                        words.push(self.word_from_token(&self.current));
                        self.advance();
                    }
                    TokenKind::Rparen => {
                        self.advance();
                        break;
                    }
                    _ => break,
                }
            }
            AssignmentValue::Array(words)
        } else {
            let rhs = &text[eq_pos + 1..];
            let word = parse_word(rhs, Span::new(start + eq_pos as u32 + 1, self.current.span.end));
            self.advance();
            AssignmentValue::Word(word)
        };

        Ok(Assignment {
            name,
            value,
            span: Span::new(start, self.current.span.start),
        })
    }

    // --- redirections ---

    fn parse_redirect_list(&mut self) -> Vec<Redirect> {
        let mut redirects = Vec::new();
        while matches!(
            self.current.kind,
            TokenKind::Greater
                | TokenKind::GreatGreat
                | TokenKind::GreaterAnd
                | TokenKind::GreatBar
                | TokenKind::Less
                | TokenKind::LessAnd
                | TokenKind::LessLess
                | TokenKind::LessLessLess
                | TokenKind::LessGreat
                | TokenKind::LessGreatGreat
                | TokenKind::LessLessDash
                | TokenKind::AmpGreat
                | TokenKind::AmpGreatGreat
                | TokenKind::IoNumber
        ) {
            match self.parse_redirect() {
                Ok(r) => redirects.push(r),
                Err(()) => break,
            }
        }
        redirects
    }

    fn parse_redirect(&mut self) -> Result<Redirect, ()> {
        let start = self.current.span.start;
        let mut fd = None;

        // Optional io-number prefix.
        if self.current.kind == TokenKind::IoNumber {
            fd = self.current.text.parse().ok();
            self.advance();
        }

        let kind = match self.current.kind {
            TokenKind::Greater => RedirectKind::Write,
            TokenKind::GreatGreat => RedirectKind::Append,
            TokenKind::GreaterAnd => RedirectKind::DupOutput,
            TokenKind::GreatBar => RedirectKind::Clobber,
            TokenKind::Less => RedirectKind::Read,
            TokenKind::LessAnd => RedirectKind::DupInput,
            TokenKind::LessLess | TokenKind::LessLessDash => RedirectKind::Heredoc,
            TokenKind::LessLessLess => RedirectKind::HereString,
            TokenKind::LessGreat => RedirectKind::ReadWrite,
            TokenKind::LessGreatGreat => RedirectKind::ReadWrite,
            TokenKind::AmpGreat => RedirectKind::AndOut,
            TokenKind::AmpGreatGreat => RedirectKind::AndAppend,
            _ => {
                self.errors.push(ParseError::new(
                    alloc::format!("expected redirection operator, got `{}`", self.current.text),
                    self.current.span,
                ));
                return Err(());
            }
        };

        let strip_tabs = self.current.kind == TokenKind::LessLessDash;

        self.advance(); // consume the operator

        let target = if kind == RedirectKind::Heredoc {
            // Next token is the delimiter word.
            let delim_tok = self.current.clone();
            self.advance(); // consume delimiter
            let slot = Rc::new(RefCell::new(None));
            self.pending_heredocs.push(PendingHeredoc {
                delimiter: unquote_heredoc_delimiter(&delim_tok.text),
                strip_tabs,
                slot: slot.clone(),
            });
            RedirectTarget::Heredoc {
                delimiter: self.word_from_token(&delim_tok),
                strip_tabs,
                body: slot,
            }
        } else if matches!(kind, RedirectKind::DupOutput | RedirectKind::DupInput) {
            self.parse_dup_target()?
        } else {
            self.parse_redirect_target()?
        };

        Ok(Redirect {
            fd,
            kind,
            target,
            span: Span::new(start, self.current.span.start),
        })
    }

    /// Parse a dup redirect target: `N` → fd, `-` → close.
    fn parse_dup_target(&mut self) -> Result<RedirectTarget, ()> {
        let target = match self.current.kind {
            TokenKind::Word => {
                let text = self.current.text.clone();
                self.advance();
                if text == "-" {
                    RedirectTarget::Close
                } else if text.chars().all(|c| c.is_ascii_digit()) {
                    RedirectTarget::Fd(text.parse().unwrap_or(1))
                } else {
                    self.errors.push(ParseError::new(
                        alloc::format!("expected fd number or `-`, got `{text}`"),
                        self.current.span,
                    ));
                    return Err(());
                }
            }
            TokenKind::IoNumber => {
                let n: u32 = self.current.text.parse().unwrap_or(1);
                self.advance();
                RedirectTarget::Fd(n)
            }
            _ => {
                self.errors.push(ParseError::new(
                    alloc::format!("expected redirection target, got `{}`", self.current.text),
                    self.current.span,
                ));
                return Err(());
            }
        };
        Ok(target)
    }

    /// Parse a general redirect target (filename word or here-string word).
    fn parse_redirect_target(&mut self) -> Result<RedirectTarget, ()> {
        match self.current.kind {
            TokenKind::Word => {
                let word = self.word_from_token(&self.current);
                self.advance();
                Ok(RedirectTarget::Word(word))
            }
            TokenKind::IoNumber => {
                let n: u32 = self.current.text.parse().unwrap_or(1);
                self.advance();
                Ok(RedirectTarget::Fd(n))
            }
            _ => {
                self.errors.push(ParseError::new(
                    alloc::format!("expected redirection target, got `{}`", self.current.text),
                    self.current.span,
                ));
                Err(())
            }
        }
    }

    // --- heredoc flushing ---

    fn flush_heredocs(&mut self) {
        for h in &self.pending_heredocs {
            match self.lexer.read_heredoc_body(&h.delimiter, h.strip_tabs) {
                Ok(body) => {
                    *h.slot.borrow_mut() = Some(body);
                }
                Err(e) => {
                    self.errors.push(e);
                }
            }
        }
        self.pending_heredocs.clear();
    }

    // --- helpers ---

    /// Expect the current token to be a word with the given text, and advance.
    fn expect_keyword(&mut self, kw: &str) -> Result<(), ()> {
        if self.current.kind == TokenKind::Word && self.current.text == kw {
            self.advance();
            Ok(())
        } else if self.current.kind == TokenKind::Eof {
            self.errors
                .push(ParseError::incomplete(alloc::format!("unexpected EOF, expected `{kw}`"), self.current.span.start));
            Err(())
        } else {
            self.errors.push(ParseError::new(
                alloc::format!("expected `{kw}`, got `{}`", self.current.text),
                self.current.span,
            ));
            Err(())
        }
    }

    /// Consume optional `;` and newline tokens (separators between parts of
    /// compound commands, e.g. `if cond; then`).
    fn skip_optional_sep(&mut self) {
        while matches!(self.current.kind, TokenKind::Semi | TokenKind::Newline) {
            self.advance();
        }
    }

    /// Expect the current token to be a word, return its text, and advance.
    fn expect_word(&mut self) -> Result<String, ()> {
        if self.current.kind == TokenKind::Word {
            let text = self.current.text.clone();
            self.advance();
            Ok(text)
        } else if self.current.kind == TokenKind::Eof {
            self.errors
                .push(ParseError::incomplete("unexpected EOF, expected a word", self.current.span.start));
            Err(())
        } else {
            self.errors.push(ParseError::new(
                alloc::format!("expected a word, got `{}`", self.current.text),
                self.current.span,
            ));
            Err(())
        }
    }

    /// Parse a primary word (the next token must be a word).
    fn parse_word_primary(&mut self) -> Result<Word, ()> {
        if self.current.kind == TokenKind::Word {
            let w = self.word_from_token(&self.current);
            self.advance();
            Ok(w)
        } else {
            self.errors.push(ParseError::new(
                "expected a word",
                self.current.span,
            ));
            Err(())
        }
    }
}
/// `<<'X'` / `<<"X"` / `<<X` all match a body line equal to `X`; strip one
/// level of surrounding quotes from a heredoc delimiter word.
fn unquote_heredoc_delimiter(text: &str) -> String {
    let b = text.as_bytes();
    if b.len() >= 2 && matches!(b[0], b'\'' | b'"') && b[0] == b[b.len() - 1] {
        text[1..text.len() - 1].to_owned()
    } else {
        text.to_owned()
    }
}
