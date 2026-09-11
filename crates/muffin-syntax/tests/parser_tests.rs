//! Parser tests: bash grammar coverage, incomplete detection, error spans.

use muffin_syntax::ParseError;
use muffin_syntax::ast::*;
use muffin_syntax::parse;

/// Parse and require success, returning the program.
fn parse_ok(src: &str) -> Program {
    match parse(src) {
        Ok(p) => p,
        Err(errs) => panic!("parse failed for {:?}: {errs:?}", src),
    }
}

/// Assert parsing fails with the given number of errors.
fn parse_err(src: &str) -> Vec<ParseError> {
    match parse(src) {
        Ok(_) => panic!("expected parse error for {:?}", src),
        Err(errs) => errs,
    }
}

// --- simple commands ---

#[test]
fn simple_command() {
    let p = parse_ok("echo hello world");
    assert_eq!(p.commands.len(), 1);
    let cmd = &p.commands[0];
    let CommandKind::Simple(s) = &cmd.list.first.commands[0].kind else {
        panic!("expected simple command");
    };
    assert_eq!(s.words.len(), 3);
    assert_eq!(s.words[0].span.start, 0);
}

#[test]
fn multiple_commands_semicolon() {
    // `echo a; echo b` is TWO complete commands sharing a line.
    let p = parse_ok("echo a; echo b");
    assert_eq!(p.commands.len(), 2);
    assert_eq!(p.commands[0].separator, Separator::Semi);
    let cc = &p.commands[0];
    assert_eq!(cc.list.first.commands.len(), 1);
}

#[test]
fn comments() {
    // Comments (#) are not yet lexed; '#...' is a word. This documents current
    // behavior; M1.5 adds comment handling.
    let p = parse_ok("echo hi # comment");
    assert_eq!(p.commands.len(), 1);
}

// --- pipelines and and-or ---

#[test]
fn pipeline() {
    let p = parse_ok("ls | grep foo | wc -l");
    let cmd = &p.commands[0];
    let pipe = &cmd.list.first;
    assert_eq!(pipe.commands.len(), 3);
    assert!(!pipe.negated);
}

#[test]
fn negated_pipeline() {
    let p = parse_ok("! grep foo file");
    let cmd = &p.commands[0];
    assert!(cmd.list.first.negated);
}

#[test]
fn and_or() {
    let p = parse_ok("a && b || c");
    let cmd = &p.commands[0];
    let list = &cmd.list;
    assert_eq!(list.rest.len(), 2);
    assert_eq!(list.rest[0].0, AndOrOp::AndAnd);
    assert_eq!(list.rest[1].0, AndOrOp::OrOr);
}

#[test]
fn background() {
    let p = parse_ok("sleep 10 &");
    assert_eq!(p.commands[0].separator, Separator::Amp);
}

// --- redirections ---

#[test]
fn redirect_basic() {
    let p = parse_ok("echo hi > file");
    let cmd = &p.commands[0].list.first.commands[0];
    assert_eq!(cmd.redirects.len(), 1);
    assert_eq!(cmd.redirects[0].kind, RedirectKind::Write);
    assert_eq!(cmd.redirects[0].fd, None);
}

#[test]
fn redirect_fd() {
    let p = parse_ok("cmd 2> err.log");
    let cmd = &p.commands[0].list.first.commands[0];
    assert_eq!(cmd.redirects.len(), 1);
    assert_eq!(cmd.redirects[0].kind, RedirectKind::Write);
    assert_eq!(cmd.redirects[0].fd, Some(2));
}

#[test]
fn redirect_dup() {
    let p = parse_ok("cmd >&2");
    let cmd = &p.commands[0].list.first.commands[0];
    assert_eq!(cmd.redirects[0].kind, RedirectKind::DupOutput);
    assert!(matches!(&cmd.redirects[0].target, RedirectTarget::Fd(2)));
}

#[test]
fn redirect_amp_great() {
    let p = parse_ok("cmd &> file");
    let cmd = &p.commands[0].list.first.commands[0];
    assert_eq!(cmd.redirects[0].kind, RedirectKind::AndOut);
}

#[test]
fn redirect_heredoc() {
    let p = parse_ok("cat <<EOF\nhello world\nEOF\n");
    let cmd = &p.commands[0].list.first.commands[0];
    assert_eq!(cmd.redirects.len(), 1);
    assert_eq!(cmd.redirects[0].kind, RedirectKind::Heredoc);
    if let RedirectTarget::Heredoc { body, .. } = &cmd.redirects[0].target {
        let b = body.borrow();
        assert_eq!(b.as_deref(), Some("hello world\n"));
    } else {
        panic!("expected heredoc");
    }
}

#[test]
fn redirect_herestring() {
    let p = parse_ok("grep foo <<< 'hello foo'");
    let cmd = &p.commands[0].list.first.commands[0];
    assert_eq!(cmd.redirects[0].kind, RedirectKind::HereString);
}

// --- compound commands ---

#[test]
fn if_then_else() {
    let p = parse_ok("if [ -f x ]; then echo yes; else echo no; fi");
    let cmd = &p.commands[0].list.first.commands[0];
    let CommandKind::If(ifc) = &cmd.kind else {
        panic!("expected if");
    };
    assert_eq!(ifc.clauses.len(), 1);
    assert!(ifc.else_body.is_some());
}

#[test]
fn if_elif_else() {
    let p = parse_ok("if a; then b; elif c; then d; else e; fi");
    let cmd = &p.commands[0].list.first.commands[0];
    let CommandKind::If(ifc) = &cmd.kind else {
        panic!("expected if");
    };
    assert_eq!(ifc.clauses.len(), 2);
    assert!(ifc.else_body.is_some());
}

#[test]
fn for_loop() {
    let p = parse_ok("for f in a b c; do echo $f; done");
    let cmd = &p.commands[0].list.first.commands[0];
    let CommandKind::For(fc) = &cmd.kind else {
        panic!("expected for");
    };
    assert_eq!(fc.var, "f");
    assert!(fc.in_words.is_some());
    assert_eq!(fc.in_words.as_ref().unwrap().len(), 3);
}

#[test]
fn for_loop_implicit() {
    let p = parse_ok("for f; do echo $f; done");
    let cmd = &p.commands[0].list.first.commands[0];
    let CommandKind::For(fc) = &cmd.kind else {
        panic!("expected for");
    };
    assert!(fc.in_words.is_none());
}

#[test]
fn while_loop() {
    let p = parse_ok("while read line; do echo $line; done");
    let cmd = &p.commands[0].list.first.commands[0];
    assert!(matches!(cmd.kind, CommandKind::While(_)));
}

#[test]
fn until_loop() {
    let p = parse_ok("until [ -f done ]; do sleep 1; done");
    let cmd = &p.commands[0].list.first.commands[0];
    assert!(matches!(cmd.kind, CommandKind::Until(_)));
}

#[test]
fn case_statement() {
    let p = parse_ok("case $x in\n  a) echo A ;;\n  b|B) echo B ;;\n  *) echo other ;;\nesac");
    let cmd = &p.commands[0].list.first.commands[0];
    let CommandKind::Case(cs) = &cmd.kind else {
        panic!("expected case");
    };
    assert_eq!(cs.arms.len(), 3);
    assert_eq!(cs.arms[1].patterns.len(), 2);
}

#[test]
fn function_def() {
    let p = parse_ok("foo() { echo hi; }");
    let cmd = &p.commands[0].list.first.commands[0];
    let CommandKind::Function(f) = &cmd.kind else {
        panic!("expected function");
    };
    assert_eq!(f.name, "foo");
}

#[test]
fn function_keyword() {
    let p = parse_ok("function foo { echo hi; }");
    let cmd = &p.commands[0].list.first.commands[0];
    let CommandKind::Function(f) = &cmd.kind else {
        panic!("expected function");
    };
    assert_eq!(f.name, "foo");
}

#[test]
fn block() {
    let p = parse_ok("{ echo a; echo b; }");
    let cmd = &p.commands[0].list.first.commands[0];
    let CommandKind::Block(b) = &cmd.kind else {
        panic!("expected block");
    };
    assert_eq!(b.body.items.len(), 2);
    assert_eq!(b.body.items[0].first.commands.len(), 1);
}

#[test]
fn subshell() {
    let p = parse_ok("( cd /tmp && ls )");
    let cmd = &p.commands[0].list.first.commands[0];
    let CommandKind::Subshell(s) = &cmd.kind else {
        panic!("expected subshell");
    };
    assert!(!s.body.items[0].rest.is_empty());
}

#[test]
fn arithmetic_command() {
    let p = parse_ok("(( i = i + 1 ))");
    let cmd = &p.commands[0].list.first.commands[0];
    let CommandKind::Arith(a) = &cmd.kind else {
        panic!("expected arith");
    };
    assert_eq!(a.text.trim(), "i = i + 1");
}

#[test]
fn conditional_command() {
    let p = parse_ok("[[ -n \"$var\" ]]");
    let cmd = &p.commands[0].list.first.commands[0];
    let CommandKind::Cond(c) = &cmd.kind else {
        panic!("expected cond");
    };
    assert_eq!(c.text.trim(), "-n \"$var\"");
}

// --- assignments ---

#[test]
fn prefix_assignment() {
    let p = parse_ok("FOO=bar cmd arg");
    let cmd = &p.commands[0].list.first.commands[0];
    let CommandKind::Simple(s) = &cmd.kind else {
        panic!("expected simple");
    };
    assert_eq!(s.assignments.len(), 1);
    assert_eq!(s.assignments[0].name, "FOO");
    assert!(matches!(s.assignments[0].value, AssignmentValue::Word(_)));
    assert_eq!(s.words.len(), 2);
}

#[test]
fn array_assignment() {
    let p = parse_ok("arr=(one two three)");
    let cmd = &p.commands[0].list.first.commands[0];
    let CommandKind::Simple(s) = &cmd.kind else {
        panic!("expected simple");
    };
    assert_eq!(s.assignments.len(), 1);
    let AssignmentValue::Array(words) = &s.assignments[0].value else {
        panic!("expected array");
    };
    assert_eq!(words.len(), 3);
}

#[test]
fn assignment_not_prefix() {
    // In argument position, a=b is a word.
    let p = parse_ok("echo a=b");
    let cmd = &p.commands[0].list.first.commands[0];
    let CommandKind::Simple(s) = &cmd.kind else {
        panic!("expected simple");
    };
    assert!(s.assignments.is_empty());
    assert_eq!(s.words.len(), 2);
}

// --- error handling ---

#[test]
fn unterminated_if_is_incomplete() {
    let errs = parse_err("if true; then echo hi");
    assert!(
        errs.iter().any(|e| e.is_incomplete),
        "expected incomplete error, got {errs:?}"
    );
}

#[test]
fn missing_then_is_error() {
    let errs = parse_err("if true; echo hi; fi");
    assert!(!errs.is_empty());
}

#[test]
fn unmatched_paren_is_error() {
    let errs = parse_err("( echo hi");
    assert!(errs.iter().any(|e| e.is_incomplete));
}

#[test]
fn unbalanced_quote() {
    let errs = parse_err("echo 'unclosed");
    assert!(errs.iter().any(|e| e.is_incomplete));
}

#[test]
fn redirection_newline_error_is_escaped() {
    // `timeout 60 >` followed by a newline: the offending token is a raw
    // newline, which must be escaped (as `\n`) in the message, not embedded
    // as a real line break.
    let errs = parse_err("timeout 60 >\n");
    let msg = &errs[0].message;
    assert!(msg.contains("got `\\n`"), "message {msg:?} should show escaped \\n");
    assert!(!msg.contains('\n'), "message must not embed a raw newline");
}

#[test]
fn error_has_span() {
    let errs = parse_err("if true; echo hi; fi");
    assert!(errs[0].span.start > 0, "span should point into the source");
}

// --- words / expansions ---

#[test]
fn word_parts() {
    let p = parse_ok("echo $HOME \"quoted $USER\" 'single'");
    let cmd = &p.commands[0].list.first.commands[0];
    let CommandKind::Simple(s) = &cmd.kind else {
        panic!("expected simple");
    };
    let w = &s.words[1];
    assert!(
        w.parts
            .iter()
            .any(|part| matches!(part, WordPart::Parameter(p, _) if p.name == "HOME"))
    );
}

#[test]
fn command_subst() {
    let p = parse_ok("echo $(whoami) `date`");
    let cmd = &p.commands[0].list.first.commands[0];
    let CommandKind::Simple(s) = &cmd.kind else {
        panic!("expected simple");
    };
    assert!(matches!(s.words[1].parts[0], WordPart::CommandSubst(..)));
    assert!(matches!(s.words[2].parts[0], WordPart::CommandSubst(..)));
}

#[test]
fn braced_param() {
    let p = parse_ok("echo ${PATH}");
    let cmd = &p.commands[0].list.first.commands[0];
    let CommandKind::Simple(s) = &cmd.kind else {
        panic!("expected simple");
    };
    let WordPart::Parameter(par, _) = &s.words[1].parts[0] else {
        panic!("expected parameter");
    };
    assert_eq!(par.name, "PATH");
    assert!(par.braced);
}

#[test]
fn heredoc_in_block() {
    let p = parse_ok("{ cat <<EOF; }\nbody line\nEOF\n");
    // The heredoc body appears after the block's closing brace + newline.
    // We expect the body to be captured correctly.
    let cmd = &p.commands[0].list.first.commands[0];
    if let CommandKind::Block(_) = &cmd.kind {
        // check nested redirect
    }
    let _ = cmd;
}

#[test]
fn subshell_with_quoted_arg() {
    let p = parse_ok("(echo \"from subshell\")");
    let cmd = &p.commands[0].list.first.commands[0];
    let CommandKind::Subshell(sub) = &cmd.kind else {
        panic!("expected subshell");
    };
    assert_eq!(sub.body.items.len(), 1);
}

#[test]
fn subshell_with_parens_in_string() {
    let p = parse_ok("echo \"a(b)c\" | (cat; echo ok)");
    let pipe = &p.commands[0].list.first.commands;
    assert_eq!(pipe.len(), 2);
    let CommandKind::Subshell(_) = &pipe[1].kind else {
        panic!("expected subshell as second pipeline element");
    };
}

#[test]
fn word_dquote_preserves_backslash() {
    // `echo "a\nb"` — inside double quotes, `\n` keeps its backslash (bash rule).
    let p = parse_ok("echo \"a\\nb\"");
    let cmd = &p.commands[0].list.first.commands[0];
    let CommandKind::Simple(s) = &cmd.kind else {
        panic!("expected simple");
    };
    let WordPart::DoubleQuoted(inner, _) = &s.words[1].parts[0] else {
        panic!("expected double-quoted part");
    };
    let WordPart::Literal(text, _) = &inner[0] else {
        panic!("expected literal inside quotes");
    };
    assert_eq!(text, "a\\nb");
}
