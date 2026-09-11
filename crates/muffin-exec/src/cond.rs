//! Conditional expression evaluation for `[[ ... ]]`.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::ProcStatus;

use crate::executor::Executor;
use crate::expand::{ExpandCtx, expand_plain_string};

pub fn eval_cond(exec: &mut Executor, text: &str) -> ProcStatus {
    let result = {
        let ctx = ExpandCtx {
            platform: exec.platform,
            env: &mut exec.env,
            last_status: exec.last_status,
            positional: &exec.positional,
            functions: &exec.functions,
            aliases: &exec.aliases,
            shell_pid: exec.shell_pid,
            nounset: exec.nounset,
            errexit: exec.errexit,
            noglob: exec.noglob,
            shopt: exec.shopt,
            last_bg_pid: exec.last_bg_pid,
            random_state: &mut exec.random_state,
            start_time: exec.start_time,
            lineno: exec.cmd_lineno,
            proc_subst_fds: &mut exec.proc_subst,
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
            exec.report_error(&alloc::format!("muffin: [[ ... ]]: {e}"));
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
            None => Err(alloc::format!(
                "expected `{expected}`, got end of expression"
            )),
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
            Some("-e") | Some("-f") | Some("-d") | Some("-r") | Some("-w") | Some("-x")
            | Some("-s") | Some("-L") | Some("-h") | Some("-S") | Some("-b") | Some("-c")
            | Some("-p") | Some("-u") | Some("-g") | Some("-k") | Some("-N") | Some("-O")
            | Some("-G") => {
                let op = self.eat().unwrap();
                let path = self.expand_word()?;
                let info = self.ctx.platform.file_info(&path);
                let my_uid = self.ctx.platform.effective_user_id();
                let my_gid = self.ctx.platform.effective_group_id();
                return Ok(crate::test_ops::eval_unary_file_test(&info, &op, my_uid, my_gid));
            }
            Some("-t") => {
                self.eat();
                let fd_word = self.expand_word()?;
                let fd: u32 = fd_word.trim().parse().unwrap_or(0);
                return Ok(self.ctx.platform.is_terminal_fd(fd));
            }
            _ => {}
        }

        // Binary test.
        let lhs = self.expand_word()?;
        let op = self.eat().ok_or_else(|| "expected operator".to_string())?;
        // For `=~`, expand variables but don't glob the RHS (bash behavior).
        let rhs = if op == "=~" {
            let raw = self
                .eat()
                .ok_or_else(|| "expected regex pattern".to_string())?;
            let fields = expand_plain_string(&mut self.ctx, &raw)
                .map_err(|e| alloc::format!("expansion error: {e}"))?;
            fields.into_iter().next().unwrap_or_default()
        } else {
            self.expand_word()?
        };

        match op.as_str() {
            // `[[ x == pat ]]` pattern-matches (extglob always active, bash).
            "=" | "==" => Ok(crate::glob::glob_match_ext(&rhs, &lhs, false, true)),
            "!=" => Ok(!crate::glob::glob_match_ext(&rhs, &lhs, false, true)),
            "=~" => {
                // `[[ x =~ regex ]]` — POSIX extended regex match.
                let re =
                    regex::Regex::new(&rhs).map_err(|e| alloc::format!("muffin: [[ =~ ]]: {e}"))?;
                Ok(re.is_match(&lhs))
            }
            "<" => Ok(lhs < rhs),
            ">" => Ok(lhs > rhs),
            "-eq" => Ok(crate::test_ops::parse_num(&lhs) == crate::test_ops::parse_num(&rhs)),
            "-ne" => Ok(crate::test_ops::parse_num(&lhs) != crate::test_ops::parse_num(&rhs)),
            "-lt" => Ok(crate::test_ops::parse_num(&lhs) < crate::test_ops::parse_num(&rhs)),
            "-le" => Ok(crate::test_ops::parse_num(&lhs) <= crate::test_ops::parse_num(&rhs)),
            "-gt" => Ok(crate::test_ops::parse_num(&lhs) > crate::test_ops::parse_num(&rhs)),
            "-ge" => Ok(crate::test_ops::parse_num(&lhs) >= crate::test_ops::parse_num(&rhs)),
            "-nt" | "-ot" | "-ef" => {
                let info_lhs = self.ctx.platform.file_info(&lhs);
                let info_rhs = self.ctx.platform.file_info(&rhs);
                Ok(crate::test_ops::eval_binary_file_test(&info_lhs, &info_rhs, &op).unwrap_or(false))
            }
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
        if matches!(c, '!' | '<' | '>' | '=') && chars.peek() == Some(&'=') {
            flush!();
            chars.next();
            let mut tok = String::new();
            tok.push(c);
            tok.push('=');
            tokens.push(tok);
            continue;
        }
        // `!(` at the start of a word is an extglob opener, not `!`.
        if c == '!' && chars.peek() == Some(&'(') {
            cur.push(c);
            continue;
        }
        // Extglob group `?(...)` `*(...)` `+(...)` `@(...)` `!(...)` stays
        // one token.
        if c == '('
            && matches!(
                cur.chars().last(),
                Some('?') | Some('*') | Some('+') | Some('@') | Some('!')
            )
        {
            let mut depth = 1;
            cur.push(c);
            for ch in chars.by_ref() {
                cur.push(ch);
                if ch == '(' {
                    depth += 1;
                } else if ch == ')' {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::boxed::Box;
    use muffin_env::EnvStack;
    use crate::executor::Executor;

    fn setup() -> (Executor<'static>, &'static muffin_platform_mock::MockPlatform) {
        let p: &'static muffin_platform_mock::MockPlatform =
            Box::leak(Box::new(muffin_platform_mock::MockPlatform::new()));
        (Executor::new(EnvStack::new(), p), p)
    }

    #[test]
    fn cond_file_tests_use_platform() {
        let (mut exec, p) = setup();
        p.set_file_info(
            "/f",
            muffin_platform::FileInfo {
                exists: true,
                is_file: true,
                ..Default::default()
            },
        );
        p.set_file_info(
            "/d",
            muffin_platform::FileInfo {
                exists: true,
                is_dir: true,
                ..Default::default()
            },
        );
        assert_eq!(eval_cond(&mut exec, "-f /f"), ProcStatus::Exit(0));
        assert_eq!(eval_cond(&mut exec, "-f /d"), ProcStatus::Exit(1));
        assert_eq!(eval_cond(&mut exec, "-d /d"), ProcStatus::Exit(0));
        assert_eq!(eval_cond(&mut exec, "-e /missing"), ProcStatus::Exit(1));
        // `&&` / `||` / `!` compose.
        assert_eq!(eval_cond(&mut exec, "-f /f && -d /d"), ProcStatus::Exit(0));
        assert_eq!(
            eval_cond(&mut exec, "! -f /missing"),
            ProcStatus::Exit(0)
        );
    }

    #[test]
    fn cond_binary_file_tests() {
        let (mut exec, p) = setup();
        let stamp = |mtime: i64| muffin_platform::FileInfo {
            exists: true,
            is_file: true,
            mtime,
            ..Default::default()
        };
        p.set_file_info("/new", stamp(300));
        p.set_file_info("/old", stamp(100));
        assert_eq!(
            eval_cond(&mut exec, "/new -nt /old"),
            ProcStatus::Exit(0)
        );
        assert_eq!(
            eval_cond(&mut exec, "/old -nt /new"),
            ProcStatus::Exit(1)
        );
        assert_eq!(
            eval_cond(&mut exec, "/old -ot /new"),
            ProcStatus::Exit(0)
        );
    }

    #[test]
    fn cond_string_and_numeric_comparisons() {
        let (mut exec, _p) = setup();
        assert_eq!(eval_cond(&mut exec, "a == a"), ProcStatus::Exit(0));
        assert_eq!(eval_cond(&mut exec, "a != b"), ProcStatus::Exit(0));
        assert_eq!(eval_cond(&mut exec, "2 -gt 1"), ProcStatus::Exit(0));
        assert_eq!(eval_cond(&mut exec, "1 -gt 2"), ProcStatus::Exit(1));
        // `=~` regex match.
        assert_eq!(eval_cond(&mut exec, "foobar =~ foo.*"), ProcStatus::Exit(0));
        assert_eq!(eval_cond(&mut exec, "foobar =~ ^bar"), ProcStatus::Exit(1));
    }

    #[test]
    fn cond_sees_execution_environment() {
        let (mut exec, _p) = setup();
        // `$?` reflects the last status through the shared ExpandCtx.
        exec.last_status = ProcStatus::Exit(3);
        assert_eq!(eval_cond(&mut exec, "$? -eq 3"), ProcStatus::Exit(0));
    }
}
