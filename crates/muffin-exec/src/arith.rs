//! Arithmetic evaluation for `(( ... ))` and (in M2c) `$(( ... ))`.
//!
//! A small recursive-descent parser over the bash arithmetic grammar with the
//! common operators, variables and assignments.

use alloc::string::String;
use alloc::vec::Vec;

use muffin_env::{EnvStack, EnvVar};
use crate::ProcStatus;

use crate::executor::Executor;

/// Evaluate an arithmetic expression to its string value (`$(( ... ))`).
///
/// Assignments inside the expression are not applied (M2c will thread them
/// back through the expansion context).
pub fn eval_arith_value(env: &EnvStack, text: &str) -> Result<String, String> {
    let mut p = ArithParser::new(text, env);
    let val = p.parse_expr()?;
    Ok(alloc::format!("{val}"))
}

pub fn eval_arith(exec: &mut Executor, text: &str) -> ProcStatus {
    let (result, assigns) = {
        let env = &exec.env;
        let mut p = ArithParser::new(text, env);
        let r = p.parse_expr();
        (r, p.assigns)
    };
    match result {
        Ok(val) => {
            for (name, v) in assigns {
                let _ = exec.env.set(&name, EnvVar::new(alloc::format!("{v}")));
            }
            let st = ProcStatus::Exit(if val == 0 { 1 } else { 0 });
            exec.last_status = st;
            st
        }
        Err(e) => {
            exec.report_error(&alloc::format!("muffin: arithmetic: {e}"));
            let st = ProcStatus::Exit(1);
            exec.last_status = st;
            st
        }
    }
}

struct ArithParser<'a> {
    bytes: &'a [u8],
    pos: usize,
    env: &'a EnvStack,
    assigns: Vec<(String, i64)>,
}

impl<'a> ArithParser<'a> {
    fn new(text: &'a str, env: &'a EnvStack) -> Self {
        Self {
            bytes: text.as_bytes(),
            pos: 0,
            env,
            assigns: Vec::new(),
        }
    }

    fn skip_ws(&mut self) {
        while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_whitespace() {
            self.pos += 1;
        }
    }

    fn peek(&mut self) -> Option<u8> {
        self.skip_ws();
        self.bytes.get(self.pos).copied()
    }

    fn eat(&mut self) -> Option<u8> {
        self.skip_ws();
        let c = self.bytes.get(self.pos).copied()?;
        self.pos += 1;
        Some(c)
    }

    fn starts(&mut self, s: &str) -> bool {
        self.skip_ws();
        self.bytes[self.pos..].starts_with(s.as_bytes())
    }

    fn eat_str(&mut self, s: &str) -> bool {
        if self.starts(s) {
            self.pos += s.len();
            true
        } else {
            false
        }
    }

    fn error(&self, msg: &str) -> String {
        alloc::format!("{msg} at offset {}", self.pos)
    }

    fn parse_expr(&mut self) -> Result<i64, String> {
        self.parse_comma()
    }

    fn parse_comma(&mut self) -> Result<i64, String> {
        let mut val = self.parse_assign()?;
        while self.eat_str(",") {
            val = self.parse_assign()?;
        }
        Ok(val)
    }

    // assignment: lvalue op= expr
    fn parse_assign(&mut self) -> Result<i64, String> {
        let save = self.pos;
        self.skip_ws();
        let name_start = self.pos;
        let name = self.try_read_name();
        if let Some(name) = name {
            if let Some(op) = self.try_assignment_op() {
                let rhs = self.parse_assign()?;
                let cur = self.read_var(&name).unwrap_or(0);
                let val = match op {
                    "=" => rhs,
                    "+=" => cur.wrapping_add(rhs),
                    "-=" => cur.wrapping_sub(rhs),
                    "*=" => cur.wrapping_mul(rhs),
                    "/=" => {
                        if rhs == 0 {
                            return Err(self.error("division by zero"));
                        }
                        cur / rhs
                    }
                    "%=" => {
                        if rhs == 0 {
                            return Err(self.error("division by zero"));
                        }
                        cur % rhs
                    }
                    "<<=" => cur << rhs,
                    ">>=" => cur >> rhs,
                    "&=" => cur & rhs,
                    "|=" => cur | rhs,
                    "^=" => cur ^ rhs,
                    _ => unreachable!(),
                };
                self.assigns.push((name, val));
                return Ok(val);
            }
            let _ = name_start;
        }
        self.pos = save;
        self.parse_ternary()
    }

    fn try_read_name(&mut self) -> Option<String> {
        self.skip_ws();
        let start = self.pos;
        while self.pos < self.bytes.len() {
            let c = self.bytes[self.pos];
            if c.is_ascii_alphanumeric() || c == b'_' {
                self.pos += 1;
            } else {
                break;
            }
        }
        if self.pos == start {
            None
        } else {
            Some(String::from_utf8_lossy(&self.bytes[start..self.pos]).into_owned())
        }
    }

    fn try_assignment_op(&mut self) -> Option<&'static str> {
        let save = self.pos;
        let op = if self.eat_str("<<=") {
            Some("<<=")
        } else if self.eat_str(">>=") {
            Some(">>=")
        } else if self.eat_str("+=") {
            Some("+=")
        } else if self.eat_str("-=") {
            Some("-=")
        } else if self.eat_str("*=") {
            Some("*=")
        } else if self.eat_str("/=") {
            Some("/=")
        } else if self.eat_str("%=") {
            Some("%=")
        } else if self.eat_str("&=") {
            Some("&=")
        } else if self.eat_str("|=") {
            Some("|=")
        } else if self.eat_str("^=") {
            Some("^=")
        } else if self.eat_str("=") {
            Some("=")
        } else {
            None
        };
        if op.is_none() {
            self.pos = save;
        }
        op
    }

    fn parse_ternary(&mut self) -> Result<i64, String> {
        let cond = self.parse_logical_or()?;
        if self.eat_str("?") {
            let a = self.parse_assign()?;
            self.expect_str(":")?;
            let b = self.parse_assign()?;
            Ok(if cond != 0 { a } else { b })
        } else {
            Ok(cond)
        }
    }

    fn expect_str(&mut self, s: &str) -> Result<(), String> {
        if self.eat_str(s) {
            Ok(())
        } else {
            Err(self.error(&alloc::format!("expected `{s}`")))
        }
    }

    fn parse_logical_or(&mut self) -> Result<i64, String> {
        let mut val = self.parse_logical_and()?;
        while self.eat_str("||") {
            let rhs = self.parse_logical_and()?;
            val = if val != 0 || rhs != 0 { 1 } else { 0 };
        }
        Ok(val)
    }

    fn parse_logical_and(&mut self) -> Result<i64, String> {
        let mut val = self.parse_bit_or()?;
        while self.eat_str("&&") {
            let rhs = self.parse_bit_or()?;
            val = if val != 0 && rhs != 0 { 1 } else { 0 };
        }
        Ok(val)
    }

    fn parse_bit_or(&mut self) -> Result<i64, String> {
        let mut val = self.parse_bit_xor()?;
        while self.eat_str("|") {
            val |= self.parse_bit_xor()?;
        }
        Ok(val)
    }

    fn parse_bit_xor(&mut self) -> Result<i64, String> {
        let mut val = self.parse_bit_and()?;
        while self.eat_str("^") {
            val ^= self.parse_bit_and()?;
        }
        Ok(val)
    }

    fn parse_bit_and(&mut self) -> Result<i64, String> {
        let mut val = self.parse_equality()?;
        while self.eat_str("&") {
            val &= self.parse_equality()?;
        }
        Ok(val)
    }

    fn parse_equality(&mut self) -> Result<i64, String> {
        let mut val = self.parse_relational()?;
        loop {
            if self.eat_str("==") {
                let rhs = self.parse_relational()?;
                val = if val == rhs { 1 } else { 0 };
            } else if self.eat_str("!=") {
                let rhs = self.parse_relational()?;
                val = if val != rhs { 1 } else { 0 };
            } else {
                break;
            }
        }
        Ok(val)
    }

    fn parse_relational(&mut self) -> Result<i64, String> {
        let mut val = self.parse_shift()?;
        loop {
            if self.eat_str("<=") {
                let rhs = self.parse_shift()?;
                val = if val <= rhs { 1 } else { 0 };
            } else if self.eat_str(">=") {
                let rhs = self.parse_shift()?;
                val = if val >= rhs { 1 } else { 0 };
            } else if self.eat_str("<") {
                let rhs = self.parse_shift()?;
                val = if val < rhs { 1 } else { 0 };
            } else if self.eat_str(">") {
                let rhs = self.parse_shift()?;
                val = if val > rhs { 1 } else { 0 };
            } else {
                break;
            }
        }
        Ok(val)
    }

    fn parse_shift(&mut self) -> Result<i64, String> {
        let mut val = self.parse_additive()?;
        loop {
            if self.eat_str("<<") {
                val <<= self.parse_additive()?;
            } else if self.eat_str(">>") {
                val >>= self.parse_additive()?;
            } else {
                break;
            }
        }
        Ok(val)
    }

    fn parse_additive(&mut self) -> Result<i64, String> {
        let mut val = self.parse_multiplicative()?;
        loop {
            if self.eat_str("+") {
                val = val.wrapping_add(self.parse_multiplicative()?);
            } else if self.eat_str("-") {
                val = val.wrapping_sub(self.parse_multiplicative()?);
            } else {
                break;
            }
        }
        Ok(val)
    }

    fn parse_multiplicative(&mut self) -> Result<i64, String> {
        let mut val = self.parse_unary()?;
        loop {
            if self.eat_str("*") {
                val = val.wrapping_mul(self.parse_unary()?);
            } else if self.eat_str("/") {
                let rhs = self.parse_unary()?;
                if rhs == 0 {
                    return Err(self.error("division by zero"));
                }
                val /= rhs;
            } else if self.eat_str("%") {
                let rhs = self.parse_unary()?;
                if rhs == 0 {
                    return Err(self.error("division by zero"));
                }
                val %= rhs;
            } else {
                break;
            }
        }
        Ok(val)
    }

    fn parse_unary(&mut self) -> Result<i64, String> {
        if self.eat_str("++") {
            let name = self
                .try_read_name()
                .ok_or_else(|| self.error("expected variable after `++`"))?;
            let cur = self.read_var(&name).unwrap_or(0) + 1;
            self.assigns.push((name, cur));
            return Ok(cur);
        }
        if self.eat_str("--") {
            let name = self
                .try_read_name()
                .ok_or_else(|| self.error("expected variable after `--`"))?;
            let cur = self.read_var(&name).unwrap_or(0) - 1;
            self.assigns.push((name, cur));
            return Ok(cur);
        }
        if self.eat_str("-") {
            return Ok(-self.parse_power()?);
        }
        if self.eat_str("+") {
            return self.parse_power();
        }
        if self.eat_str("~") {
            return Ok(!self.parse_power()?);
        }
        if self.eat_str("!") {
            return Ok(if self.parse_power()? == 0 { 1 } else { 0 });
        }
        self.parse_power()
    }

    /// `**` exponentiation: binds tighter than unary `-`, right-associative.
    fn parse_power(&mut self) -> Result<i64, String> {
        let base = self.parse_primary()?;
        if self.eat_str("**") {
            let exp = self.parse_unary()?;
            if exp < 0 {
                // Integer division: a ** -n is 0 except for |a| == 1.
                return Ok(if base.abs() == 1 { base } else { 0 });
            }
            return Ok(base.wrapping_pow(exp as u32));
        }
        Ok(base)
    }

    fn parse_primary(&mut self) -> Result<i64, String> {
        let c = match self.peek() {
            Some(c) => c,
            None => return Err(self.error("unexpected end of arithmetic expression")),
        };
        if c == b'(' {
            self.eat();
            let val = self.parse_expr()?;
            self.expect_str(")")?;
            return Ok(val);
        }
        if c.is_ascii_digit() {
            let start = self.pos;
            while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_alphanumeric() {
                self.pos += 1;
            }
            let tok = String::from_utf8_lossy(&self.bytes[start..self.pos]).into_owned();
            let val = if tok.starts_with("0x") || tok.starts_with("0X") {
                i64::from_str_radix(&tok[2..], 16).map_err(|_| self.error("bad hex literal"))?
            } else if tok.starts_with('0') && tok.len() > 1 {
                i64::from_str_radix(&tok[1..], 8).map_err(|_| self.error("bad octal literal"))?
            } else {
                tok.parse::<i64>().map_err(|_| self.error("bad number"))?
            };
            return Ok(val);
        }
        if c == b'\'' {
            self.eat();
            if let Some(ch) = self.eat() {
                self.expect_str("'")?;
                return Ok(ch as i64);
            }
            return Err(self.error("bad char literal"));
        }
        // Variable name.
        if let Some(name) = self.try_read_name() {
            let val = self.read_var(&name).unwrap_or(0);
            // Postfix `name++` / `name--`: yield the old value, then assign.
            if self.eat_str("++") {
                self.assigns.push((name, val + 1));
                return Ok(val);
            }
            if self.eat_str("--") {
                self.assigns.push((name, val - 1));
                return Ok(val);
            }
            return Ok(val);
        }
        Err(self.error("unexpected character in arithmetic"))
    }

    fn read_var(&self, name: &str) -> Option<i64> {
        self.env
            .get(name)
            .and_then(|v| v.value().trim().parse::<i64>().ok())
    }
}
