//! Context-sensitive lexer for the bash grammar.
//!
//! The lexer turns source text into a token stream. Words are the hard part:
//! a "word" runs until an unquoted metacharacter, but quotes, escapes,
//! `$( )`, `${ }`, `$(( ))`, backticks and `$'...'` must be balanced while
//! scanning, so operators inside them never terminate the word.
//!
//! Bash's grammar is context-sensitive, so the parser nudges the lexer via
//! [`LexContext`] before each token is requested.

use alloc::borrow::ToOwned;
use alloc::format;
use alloc::string::String;

use crate::error::ParseError;
use crate::span::{Offset, Span};
use crate::token::{Token, TokenKind};

/// Parser-driven context that disambiguates tokens.
#[derive(Debug, Clone, Copy, Default)]
pub struct LexContext {
    /// The next token is in command position: `(`, `{`, `!`, `[[`, `((`
    /// are operators rather than word characters.
    pub cmd_pos: bool,
    /// The next token is a redirection target: `(`/`{` are word chars.
    pub in_redir: bool,
    /// Inside `[[ ... ]]`: `<`, `>`, `(`, `)`, `]]` are operators.
    pub in_cond: bool,
    /// Inside `(( ... ))` arithmetic: `(`/`)` are operators.
    pub in_arith: bool,
    /// Inside a `case` pattern list.
    pub in_case: bool,
    /// After `declare`/`local`/`typeset`/`export`/`readonly`: allow
    /// `name=(...)` array assignments.
    pub in_typeset: bool,
}

#[derive(Debug)]
pub struct Lexer<'a> {
    src: &'a str,
    pos: Offset,
    pub ctx: LexContext,
}

/// Characters that terminate a word when unquoted and at top level.
fn is_metachar(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | ';' | '&' | '|' | '<' | '>')
}

impl<'a> Lexer<'a> {
    pub fn new(src: &'a str) -> Self {
        Self {
            src,
            pos: 0,
            ctx: LexContext::default(),
        }
    }

    // --- low-level cursor helpers ---

    fn peek(&self) -> Option<char> {
        self.src[self.pos as usize..].chars().next()
    }

    fn peek2(&self) -> Option<char> {
        let mut it = self.src[self.pos as usize..].chars();
        it.next();
        it.next()
    }

    fn advance(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += c.len_utf8() as Offset;
        Some(c)
    }

    fn at_end(&self) -> bool {
        self.pos as usize >= self.src.len()
    }

    fn rest(&self) -> &'a str {
        &self.src[self.pos as usize..]
    }

    fn span_from(&self, start: Offset) -> Span {
        Span::new(start, self.pos)
    }

    // --- public API ---

    /// Produce the next token.
    ///
    /// Callers should set `self.ctx` before calling.
    pub fn next_token(&mut self) -> Result<Token, ParseError> {
        // Skip blanks (but keep newlines, which terminate commands).
        while let Some(c) = self.peek() {
            if c == ' ' || c == '\t' {
                self.advance();
            } else {
                break;
            }
        }

        if self.at_end() {
            return Ok(self.mk(TokenKind::Eof, self.pos));
        }

        let c = self.peek().unwrap();

        // A `#` that begins a word is a comment (bash): run to end of line.
        // The newline after it is still a command separator. `#` mid-word is
        // literal and never reaches here.
        if c == '#' && !self.ctx.in_cond && !self.ctx.in_arith {
            while let Some(ch) = self.peek() {
                if ch == '\n' {
                    break;
                }
                self.advance();
            }
            return self.next_token();
        }

        // Redirection operators (and io numbers) are recognized everywhere
        // except inside [[ ]] and (( )).
        if !self.ctx.in_cond && !self.ctx.in_arith && (c == '<' || c == '>') {
            return self.read_redirect();
        }

        // A numeric fd prefix immediately followed by `<`/`>` is an io number
        // (`2>file`). Digits not glued to a redirection are ordinary words.
        if !self.ctx.in_cond && !self.ctx.in_arith && c.is_ascii_digit() {
            let rest = self.rest();
            let mut digits = 0;
            for ch in rest.chars() {
                if ch.is_ascii_digit() {
                    digits += 1;
                } else {
                    break;
                }
            }
            if digits < rest.len()
                && let Some(after) = rest[digits..].chars().next()
                && matches!(after, '<' | '>')
            {
                let start = self.pos;
                for _ in 0..digits {
                    self.advance();
                }
                return Ok(self.mk(TokenKind::IoNumber, start));
            }
        }

        // Semicolons, ampersands, pipes always terminate a word / are ops.
        match c {
            ';' => return Ok(self.read_semis()),
            '&' => return Ok(self.read_amp()),
            '|' => return Ok(self.read_pipe()),
            _ => {}
        }

        if c == '\n' {
            self.advance();
            return Ok(self.mk(TokenKind::Newline, self.pos - 1));
        }

        // `(` and `)` are always operators at token start (they are never
        // word chars in bash). `((` → ArithOpen needs cmd_pos.
        if !self.ctx.in_cond && !self.ctx.in_arith && matches!(c, '(' | ')') {
            let start = self.pos;
            let is_arith = c == '(' && self.ctx.cmd_pos && self.peek2() == Some('(');
            self.advance();
            if is_arith {
                self.advance();
                return Ok(self.mk(TokenKind::ArithOpen, start));
            }
            let kind = if c == '(' { TokenKind::Lparen } else { TokenKind::Rparen };
            return Ok(self.mk(kind, start));
        }

        // Command-position operators.
        if self.ctx.cmd_pos && !self.ctx.in_cond {
            match c {
                '{' => {
                    // `{` is a reserved word only when followed by a blank.
                    if matches!(self.peek2(), None | Some(' ' | '\t' | '\n' | ';' | '&' | '|')) {
                        let start = self.pos;
                        self.advance();
                        return Ok(self.mk(TokenKind::Lbrace, start));
                    }
                }
                '}' => {
                    let start = self.pos;
                    self.advance();
                    return Ok(self.mk(TokenKind::Rbrace, start));
                }
                '!' => {
                    let start = self.pos;
                    // `!` negates the next pipeline only when followed by a
                    // blank or at the end of input.
                    if matches!(self.peek2(), None | Some(' ' | '\t' | '\n' | ';' | '&' | '|' | '(')) {
                        self.advance();
                        return Ok(self.mk(TokenKind::Bang, start));
                    }
                }
                '[' => {
                    let start = self.pos;
                    if self.rest().starts_with("[[") {
                        self.advance();
                        self.advance();
                        return Ok(self.mk(TokenKind::DoubleBracketOpen, start));
                    }
                }
                _ => {}
            }
        }

        // `]]` closes a conditional expression.
        if self.ctx.in_cond && self.rest().starts_with("]]") {
            let start = self.pos;
            self.advance();
            self.advance();
            return Ok(self.mk(TokenKind::DoubleBracketClose, start));
        }

        // In conditional or arithmetic context, `(`/`)` are operators.
        if (self.ctx.in_cond || self.ctx.in_arith) && matches!(c, '(' | ')') {
            let start = self.pos;
            self.advance();
            return Ok(self.mk(if c == '(' { TokenKind::Lparen } else { TokenKind::Rparen }, start));
        }

        // Anything else: a word (possibly `name=value` in command position).
        self.read_word()
    }

    /// Read a heredoc body: lines until a line equal to `delimiter`.
    ///
    /// `pos` must be positioned just after the `<<WORD` tokens.
    /// Returns the body without the final delimiter line.
    pub fn read_heredoc_body(&mut self, delimiter: &str, strip_tabs: bool) -> Result<String, ParseError> {
        let mut out = String::new();
        loop {
            if self.at_end() {
                // A heredoc that never reaches its delimiter is incomplete
                // (the interactive reader keeps reading for the body).
                return Err(ParseError::incomplete(
                    format!("here-document delimited by end-of-file (wanted '{delimiter}')"),
                    self.pos,
                ));
            }
            let line_start = self.pos;
            // Read one line.
            let mut line = String::new();
            while let Some(c) = self.peek() {
                if c == '\n' {
                    self.advance();
                    break;
                }
                self.advance();
                line.push(c);
            }
            let mut stripped = line.clone();
            if strip_tabs {
                stripped = stripped.trim_start_matches('\t').to_owned();
            }
            if stripped == delimiter {
                let _ = line_start;
                return Ok(out);
            }
            out.push_str(&line);
            out.push('\n');
        }
    }

    /// The current byte offset.
    pub fn pos(&self) -> Offset {
        self.pos
    }

    fn mk(&self, kind: TokenKind, start: Offset) -> Token {
        Token::new(kind, self.span_from(start), self.src[start as usize..self.pos as usize].to_owned())
    }

    // --- operator readers ---

    fn read_semis(&mut self) -> Token {
        let start = self.pos;
        self.advance(); // consume first ;
        if self.peek() == Some(';') {
            self.advance(); // ;;
            if self.peek() == Some(';') {
                self.advance(); // ;;;
            }
            self.mk(TokenKind::SemiSemi, start)
        } else if self.peek() == Some('&') {
            self.advance(); // ;&
            self.mk(TokenKind::SemiSemi, start)
        } else {
            self.mk(TokenKind::Semi, start)
        }
    }

    fn read_amp(&mut self) -> Token {
        let start = self.pos;
        self.advance(); // consume &
        match self.peek() {
            Some('&') => {
                self.advance();
                self.mk(TokenKind::AndAnd, start)
            }
            Some('>') => {
                self.advance();
                if self.peek() == Some('>') {
                    self.advance();
                    self.mk(TokenKind::AmpGreatGreat, start)
                } else {
                    self.mk(TokenKind::AmpGreat, start)
                }
            }
            _ => self.mk(TokenKind::Amp, start),
        }
    }

    fn read_pipe(&mut self) -> Token {
        let start = self.pos;
        self.advance();
        if self.peek() == Some('&') {
            self.advance();
            return self.mk(TokenKind::PipeAmp, start);
        }
        if self.peek() == Some('|') {
            self.advance();
            return self.mk(TokenKind::OrOr, start);
        }
        self.mk(TokenKind::Pipe, start)
    }

    fn read_redirect(&mut self) -> Result<Token, ParseError> {
        let start = self.pos;
        let c = self.advance().unwrap();
        let kind = match c {
            '<' => {
                if self.peek() == Some('(') {
                    // Process substitution `<(cmd)`: consume to the matching
                    // `)` and yield the whole thing as one word.
                    self.advance(); // (
                    self.skip_command_subst_after_open()?;
                    return Ok(self.mk(TokenKind::Word, start));
                } else if self.peek() == Some('<') {
                    self.advance();
                    if self.peek() == Some('<') {
                        self.advance(); // <<<
                        TokenKind::LessLessLess
                    } else if self.peek() == Some('-') {
                        self.advance(); // <<-
                        TokenKind::LessLessDash
                    } else if self.peek() == Some('<') {
                        self.advance(); // <<<  (extra)
                        TokenKind::LessLessLess
                    } else if self.peek() == Some('>') {
                        self.advance(); // <<>
                        TokenKind::LessGreatGreat
                    } else {
                        TokenKind::LessLess
                    }
                } else if self.peek() == Some('&') {
                    self.advance(); // <&
                    TokenKind::LessAnd
                } else if self.peek() == Some('>') {
                    self.advance(); // <>
                    TokenKind::LessGreat
                } else {
                    TokenKind::Less
                }
            }
            '>' => {
                if self.peek() == Some('(') {
                    // Process substitution `>(cmd)`.
                    self.advance(); // (
                    self.skip_command_subst_after_open()?;
                    return Ok(self.mk(TokenKind::Word, start));
                } else if self.peek() == Some('>') {
                    self.advance();
                    if self.peek() == Some('&') {
                        self.advance(); // >>&
                        TokenKind::GreaterAnd
                    } else if self.peek() == Some('|') {
                        self.advance(); // >>|
                        TokenKind::GreatGreat
                    } else {
                        TokenKind::GreatGreat
                    }
                } else if self.peek() == Some('&') {
                    self.advance(); // >&
                    TokenKind::GreaterAnd
                } else if self.peek() == Some('|') {
                    self.advance(); // >|
                    TokenKind::GreatBar
                } else {
                    TokenKind::Greater
                }
            }
            _ => unreachable!(),
        };

        Ok(self.mk(kind, start))
    }

    // --- word reader ---

    fn read_word(&mut self) -> Result<Token, ParseError> {
        let start = self.pos;
        let mut in_dquote = false;
        // Top-level nesting of literal `(` used as word chars: irrelevant to
        // balancing; only expansions introduce balancing.

        loop {
            if self.at_end() {
                break;
            }
            let c = self.peek().unwrap();

            // End of word: unquoted metachar at top level.
            if !in_dquote && is_metachar(c) {
                // `<(cmd)` / `>(cmd)` process substitution stays in the word.
                if (c == '<' || c == '>') && self.rest()[1..].starts_with('(') {
                    self.advance(); // < or >
                    self.advance(); // (
                    self.skip_command_subst_after_open()?;
                    continue;
                }
                break;
            }
            // `(` and `)` are always word terminators in bash (they are
            // metacharacters): `foo()`, `arr=(...)`, `a) pattern`, `echo a(b)`
            // is a syntax error. Redirection targets may carry `>(...)`
            // process substitution, so parens are kept there.
            if !in_dquote && matches!(c, '(' | ')') && !self.ctx.in_redir {
                break;
            }
            // `]]` closes a conditional even inside a word boundary when in_cond.
            if self.ctx.in_cond && !in_dquote && self.rest().starts_with("]]") {
                break;
            }
            if (self.ctx.in_cond || self.ctx.in_arith) && !in_dquote && matches!(c, '(' | ')') {
                break;
            }
            // In command position, an unquoted `!` that begins a fresh word
            // followed by blank already handled; within a word it's ordinary.
            // Unquoted parens at top level of a word are ordinary chars.

            match c {
                '\\' => {
                    self.advance();
                    if !self.at_end() {
                        self.advance();
                    }
                }
                '\'' if !in_dquote => {
                    self.skip_single_quoted()?;
                }
                '"' => {
                    in_dquote = !in_dquote;
                    self.advance();
                }
                '`' => {
                    self.skip_backtick()?;
                }
                '$' => {
                    self.advance();
                    match self.peek() {
                        Some('(') => {
                            self.advance();
                            if self.peek() == Some('(') {
                                self.advance();
                                self.skip_arith()?;
                            } else {
                                self.skip_command_subst_after_open()?;
                            }
                        }
                        Some('{') => {
                            self.advance();
                            self.skip_braced()?;
                        }
                        Some('\'') => {
                            self.advance();
                            self.skip_ansi_c()?;
                        }
                        Some('"') => {
                            self.advance();
                            // $"..." is like a double-quoted string.
                            let mut closed = false;
                            while let Some(c2) = self.advance() {
                                if c2 == '\\' {
                                    if !self.at_end() {
                                        self.advance();
                                    }
                                    continue;
                                }
                                if c2 == '"' {
                                    closed = true;
                                    break;
                                }
                            }
                            if !closed {
                                return Err(ParseError::incomplete("unterminated quoted string", self.pos));
                            }
                        }
                        _ => {}
                    }
                }
                _ => {
                    self.advance();
                }
            }
        }

        let text = self.src[start as usize..self.pos as usize].to_owned();

        // In command position (and not a redirect target), a word of the form
        // NAME=... is an assignment; `NAME=(...)` an array assignment, and
        // `NAME[expr]=...` an indexed assignment.
        if (self.ctx.cmd_pos || self.ctx.in_typeset)
            && !self.ctx.in_redir
            && let Some(eq) = text.find('=')
            && is_assignment_target(&text[..eq])
        {
            return Ok(Token::new(TokenKind::Assignment, self.span_from(start), text));
        }

        Ok(Token::new(TokenKind::Word, self.span_from(start), text))
    }

    // --- balanced sub-construct scanners ---
    //
    // Each of these assumes the opener has been consumed and `self.pos`
    // points just after it, and consumes through the matching closer.

    fn skip_single_quoted(&mut self) -> Result<(), ParseError> {
        self.advance(); // consume '
        while let Some(c) = self.advance() {
            if c == '\'' {
                return Ok(());
            }
        }
        Err(ParseError::incomplete("unterminated quoted string", self.pos))
    }

    /// Scan a double-quoted string body. `self.pos` is just after `"`.
    fn skip_double_quoted(&mut self) -> Result<(), ParseError> {
        while let Some(c) = self.advance() {
            match c {
                '\\' => {
                    if !self.at_end() {
                        self.advance();
                    }
                }
                '$' => {
                    match self.peek() {
                        Some('(') => {
                            self.advance();
                            if self.peek() == Some('(') {
                                self.advance();
                                self.skip_arith()?;
                            } else {
                                self.skip_command_subst_after_open()?;
                            }
                        }
                        Some('{') => {
                            self.advance();
                            self.skip_braced()?;
                        }
                        _ => {}
                    }
                }
                '`' => self.skip_backtick()?,
                '"' => return Ok(()),
                _ => {}
            }
        }
        Err(ParseError::incomplete("unterminated quoted string", self.pos))
    }

    fn skip_backtick(&mut self) -> Result<(), ParseError> {
        // self.pos is just after the opening backtick.
        while let Some(c) = self.advance() {
            match c {
                '\\' => {
                    if !self.at_end() {
                        self.advance();
                    }
                }
                '`' => return Ok(()),
                _ => {}
            }
        }
        Err(ParseError::incomplete("unexpected EOF while looking for matching `'`", self.pos))
    }

    fn skip_ansi_c(&mut self) -> Result<(), ParseError> {
        // self.pos is just after `$'`.
        while let Some(c) = self.advance() {
            match c {
                '\\' => {
                    if !self.at_end() {
                        self.advance();
                    }
                }
                '\'' => return Ok(()),
                _ => {}
            }
        }
        Err(ParseError::incomplete("unterminated quoted string", self.pos))
    }

    /// `self.pos` is just after `$(` — scan to the matching `)`.
    fn skip_command_subst_after_open(&mut self) -> Result<(), ParseError> {
        let mut depth: i32 = 1;
        loop {
            if self.at_end() {
                return Err(ParseError::incomplete(
                    "unexpected EOF while looking for matching `)'",
                    self.pos,
                ));
            }
            let c = self.advance().unwrap();
            match c {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        return Ok(());
                    }
                }
                '\\' => {
                    if !self.at_end() {
                        self.advance();
                    }
                }
                '\'' => self.skip_single_quoted()?,
                '"' => self.skip_double_quoted()?,
                '`' => self.skip_backtick()?,
                '$' => match self.peek() {
                    Some('(') => {
                        self.advance();
                        if self.peek() == Some('(') {
                            self.advance();
                            self.skip_arith()?;
                        } else {
                            self.skip_command_subst_after_open()?;
                        }
                    }
                    Some('{') => {
                        self.advance();
                        self.skip_braced()?;
                    }
                    Some('\'') => {
                        self.advance();
                        self.skip_ansi_c()?;
                    }
                    _ => {}
                },
                _ => {}
            }
        }
    }

    /// `self.pos` is just after `$((` — scan to the matching `))`.
    fn skip_arith(&mut self) -> Result<(), ParseError> {
        let mut depth: i32 = 0;
        loop {
            if self.at_end() {
                return Err(ParseError::incomplete(
                    "unexpected EOF while looking for matching `))'",
                    self.pos,
                ));
            }
            let c = self.advance().unwrap();
            match c {
                '(' => depth += 1,
                ')' => {
                    if depth == 0 {
                        // Need one more `)` to close `((` ... `))`.
                        if self.peek() == Some(')') {
                            self.advance();
                            return Ok(());
                        }
                        // A single `)` — for `((` this is unbalanced but bash
                        // tolerates `$(( 1 + (2) ))`? The inner `)` closes the
                        // `(` which incremented depth. depth==0 here means we
                        // saw the first closing paren of the outer `))`.
                        // Handle `))` strictly.
                        return Err(ParseError::new("arithmetic: expected '))'", Span::at(self.pos)));
                    }
                    depth -= 1;
                }
                '\'' => self.skip_single_quoted()?,
                '"' => self.skip_double_quoted()?,
                '`' => self.skip_backtick()?,
                '$' => match self.peek() {
                    Some('(') => {
                        self.advance();
                        if self.peek() == Some('(') {
                            self.advance();
                            self.skip_arith()?;
                        }
                    }
                    Some('{') => {
                        self.advance();
                        self.skip_braced()?;
                    }
                    _ => {}
                },
                _ => {}
            }
        }
    }

    /// `self.pos` is just after `${` — scan to the matching `}`.
    fn skip_braced(&mut self) -> Result<(), ParseError> {
        let mut depth: i32 = 1;
        loop {
            if self.at_end() {
                return Err(ParseError::incomplete(
                    "unexpected EOF while looking for matching `}'",
                    self.pos,
                ));
            }
            let c = self.advance().unwrap();
            match c {
                '$' => {
                    if self.peek() == Some('{') {
                        self.advance();
                        depth += 1;
                    } else if self.peek() == Some('(') {
                        self.advance();
                        if self.peek() == Some('(') {
                            self.advance();
                            self.skip_arith()?;
                        } else {
                            self.skip_command_subst_after_open()?;
                        }
                    }
                }
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Ok(());
                    }
                }
                '\\' => {
                    if !self.at_end() {
                        self.advance();
                    }
                }
                '\'' => self.skip_single_quoted()?,
                '"' => self.skip_double_quoted()?,
                '`' => self.skip_backtick()?,
                _ => {}
            }
        }
    }
}

/// Whether `s` is a valid bash variable/function name (also used for
/// `name=value` assignments).
pub fn is_valid_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c == '_' || c.is_ascii_alphabetic() => {}
        _ => return false,
    }
    chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

/// Whether the part before `=` of an assignment is `NAME` or `NAME[expr]`
/// (with balanced brackets).
fn is_assignment_target(s: &str) -> bool {
    match s.find('[') {
        None => is_valid_identifier(s),
        Some(open) => {
            if !is_valid_identifier(&s[..open]) || !s.ends_with(']') {
                return false;
            }
            let inner = &s[open + 1..s.len() - 1];
            // Index expression: balanced brackets (rough check).
            let mut depth = 0i32;
            for c in inner.chars() {
                match c {
                    '[' => depth += 1,
                    ']' => {
                        depth -= 1;
                        if depth < 0 {
                            return false;
                        }
                    }
                    _ => {}
                }
            }
            depth == 0
        }
    }
}
