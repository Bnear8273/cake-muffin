//! Conditional expression evaluation for `[[ ... ]]`.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use cake_proc::ProcStatus;

use crate::executor::Executor;
use crate::expand::{expand_plain_string, ExpandCtx};

pub fn eval_cond(exec: &mut Executor, text: &str) -> ProcStatus {
    let result = {
        let ctx = ExpandCtx {
            env: &mut exec.env,
            last_status: exec.last_status,
            positional: &exec.positional,
            functions: &exec.functions,
            aliases: &exec.aliases,
            shell_pid: exec.shell_pid,
        };
        let mut p = CondParser::new(text, ctx);
        p.parse_expr()
    };
    match result {
        Ok(val) => {
            let st = ProcStatus::Exit(if val { 0 } else { 1 });
            exec.last_status = st;
            st
        }
        Err(e) => {
            exec.report_error(&alloc::format!("cake: [[ ... ]]: {e}"));
            let st = ProcStatus::Exit(1);
            exec.last_status = st;
            st
        }
    }
}

struct CondParser<'a> {
    tokens: Vec<String>,
    pos: usize,
    ctx: ExpandCtx<'a>,
}

impl<'a> CondParser<'a> {
    fn new(text: &str, ctx: ExpandCtx<'a>) -> Self {
        Self {
            tokens: tokenize_cond(text),
            pos: 0,
            ctx,
        }
    }

    fn peek(&self) -> Option<&str> {
        self.tokens.get(self.pos).map(String::as_str)
    }

    fn eat(&mut self) -> Option<String> {
        let t = self.tokens.get(self.pos).cloned()?;
        self.pos += 1;
        Some(t)
    }

    fn expect(&mut self, expected: &str) -> Result<String, String> {
        let t = self.tokens.get(self.pos).cloned();
        match t {
            Some(s) if s == expected => {
                self.pos += 1;
                Ok(s)
            }
            Some(other) => Err(alloc::format!("expected `{expected}`, got `{other}`")),
            None => Err(alloc::format!("expected `{expected}`, got end of expression")),
        }
    }

    fn parse_expr(&mut self) -> Result<bool, String> {
        self.parse_or()
    }

    fn parse_or(&mut self) -> Result<bool, String> {
        let mut val = self.parse_and()?;
        while self.peek() == Some("||") || self.peek() == Some("-o") {
            self.eat();
            val = val || self.parse_and()?;
        }
        Ok(val)
    }

    fn parse_and(&mut self) -> Result<bool, String> {
        let mut val = self.parse_not()?;
        while self.peek() == Some("&&") || self.peek() == Some("-a") {
            self.eat();
            val = val && self.parse_not()?;
        }
        Ok(val)
    }

    fn parse_not(&mut self) -> Result<bool, String> {
        if self.peek() == Some("!") {
            self.eat();
            return Ok(!self.parse_primary()?);
        }
        self.parse_primary()
    }

    fn parse_primary(&mut self) -> Result<bool, String> {
        if self.peek() == Some("(") {
            self.eat();
            let val = self.parse_expr()?;
            self.expect(")")?;
            return Ok(val);
        }

        // Unary tests.
        match self.peek() {
            Some("-n") => {
                self.eat();
                let word = self.expand_word()?;
                return Ok(!word.is_empty());
            }
            Some("-z") => {
                self.eat();
                let word = self.expand_word()?;
                return Ok(word.is_empty());
            }
            Some("-e") => {
                self.eat();
                let path = self.expand_word()?;
                return Ok(cake_platform::get().stat(&path).exists);
            }
            Some("-f") => {
                self.eat();
                let path = self.expand_word()?;
                return Ok(cake_platform::get().stat(&path).is_file);
            }
            Some("-d") => {
                self.eat();
                let path = self.expand_word()?;
                return Ok(cake_platform::get().stat(&path).is_dir);
            }
            Some("-r") => {
                self.eat();
                let path = self.expand_word()?;
                return Ok(cake_platform::get().stat(&path).is_readable);
            }
            Some("-w") => {
                self.eat();
                let path = self.expand_word()?;
                return Ok(cake_platform::get().stat(&path).is_writable);
            }
            Some("-x") => {
                self.eat();
                let path = self.expand_word()?;
                return Ok(cake_platform::get().stat(&path).is_executable);
            }
            Some("-s") => {
                self.eat();
                let path = self.expand_word()?;
                return Ok(cake_platform::get().stat(&path).size > 0);
            }
            Some("-L") => {
                self.eat();
                let path = self.expand_word()?;
                return Ok(cake_platform::get().stat(&path).is_symlink);
            }
            _ => {}
        }

        // Binary test.
        let lhs = self.expand_word()?;
        let op = self.eat().ok_or_else(|| "expected operator".to_string())?;
        let rhs = self.expand_word()?;

        match op.as_str() {
            "=" | "==" => Ok(lhs == rhs),
            "!=" => Ok(lhs != rhs),
            "<" => Ok(lhs < rhs),
            ">" => Ok(lhs > rhs),
            "-eq" => Ok(parse_num(&lhs) == parse_num(&rhs)),
            "-ne" => Ok(parse_num(&lhs) != parse_num(&rhs)),
            "-lt" => Ok(parse_num(&lhs) < parse_num(&rhs)),
            "-le" => Ok(parse_num(&lhs) <= parse_num(&rhs)),
            "-gt" => Ok(parse_num(&lhs) > parse_num(&rhs)),
            "-ge" => Ok(parse_num(&lhs) >= parse_num(&rhs)),
            other => Err(alloc::format!("unknown operator `{other}`")),
        }
    }

    fn expand_word(&mut self) -> Result<String, String> {
        let word = self.eat().ok_or_else(|| "expected word".to_string())?;
        let fields = expand_plain_string(&mut self.ctx, &word)
            .map_err(|e| alloc::format!("expansion error: {e}"))?;
        // An empty expansion (e.g. `""` or an unset variable) yields zero
        // fields; as a `[[ ]]` operand that is the empty string.
        Ok(fields.into_iter().next().unwrap_or_default())
    }
}

fn parse_num(s: &str) -> i64 {
    s.trim().parse().unwrap_or(0)
}

/// Tokenize a `[[ ... ]]` body into words and operators.
fn tokenize_cond(text: &str) -> Vec<String> {
    let mut tokens: Vec<String> = Vec::new();
    let mut chars = text.chars().peekable();
    let mut cur = String::new();
    let mut in_dquote = false;

    macro_rules! flush {
        () => {
            if !cur.is_empty() {
                tokens.push(core::mem::take(&mut cur));
            }
        };
    }

    while let Some(c) = chars.next() {
        if c == '"' {
            in_dquote = !in_dquote;
            cur.push(c);
            continue;
        }
        if in_dquote {
            cur.push(c);
            continue;
        }
        if c.is_whitespace() {
            flush!();
            continue;
        }
        if c == '&' && chars.peek() == Some(&'&') {
            flush!();
            chars.next();
            tokens.push("&&".into());
            continue;
        }
        if c == '|' && chars.peek() == Some(&'|') {
            flush!();
            chars.next();
            tokens.push("||".into());
            continue;
        }
        // Multi-char comparison operators before single-char handling.
        if matches!(c, '!' | '<' | '>') && chars.peek() == Some(&'=') {
            flush!();
            chars.next();
            let mut tok = String::new();
            tok.push(c);
            tok.push('=');
            tokens.push(tok);
            continue;
        }
        if matches!(c, '(' | ')' | '!') {
            flush!();
            tokens.push(c.to_string());
            continue;
        }
        cur.push(c);
    }
    flush!();
    tokens
}
