//! Lexer integration tests.

use cake_syntax::lexer::{LexContext, Lexer};
use cake_syntax::token::{Token, TokenKind};

fn tokenize(src: &str) -> Vec<Token> {
    let mut lexer = Lexer::new(src);
    let mut tokens = Vec::new();
    loop {
        let tok = lexer.next_token().unwrap();
        let kind = tok.kind;
        tokens.push(tok);
        if kind == TokenKind::Eof {
            break;
        }
    }
    tokens
}

fn tokenize_with_ctx(src: &str, ctx: LexContext) -> Vec<Token> {
    let mut lexer = Lexer::new(src);
    lexer.ctx = ctx;
    let mut tokens = Vec::new();
    loop {
        let tok = lexer.next_token().unwrap();
        let kind = tok.kind;
        tokens.push(tok);
        if kind == TokenKind::Eof {
            break;
        }
    }
    tokens
}

#[test]
fn simple_words() {
    let t = tokenize("echo hello");
    assert_eq!(t.len(), 3); // echo, hello, EOF
    assert_eq!(t[0].kind, TokenKind::Word);
    assert_eq!(t[0].text, "echo");
    assert_eq!(t[1].kind, TokenKind::Word);
    assert_eq!(t[1].text, "hello");
}

#[test]
fn operators() {
    let t = tokenize("a && b || c | d ; e & f");
    assert_eq!(t[0].kind, TokenKind::Word); // a
    assert_eq!(t[1].kind, TokenKind::AndAnd); // &&
    assert_eq!(t[2].kind, TokenKind::Word); // b
    assert_eq!(t[3].kind, TokenKind::OrOr); // ||
    assert_eq!(t[4].kind, TokenKind::Word); // c
    assert_eq!(t[5].kind, TokenKind::Pipe); // |
    assert_eq!(t[6].kind, TokenKind::Word); // d
    assert_eq!(t[7].kind, TokenKind::Semi); // ;
    assert_eq!(t[8].kind, TokenKind::Word); // e
    assert_eq!(t[9].kind, TokenKind::Amp); // &
    assert_eq!(t[10].kind, TokenKind::Word); // f
}

#[test]
fn redirections() {
    // Test basic redirections
    let t = tokenize("echo > file");
    assert_eq!(t[1].kind, TokenKind::Greater);
    assert_eq!(t[2].kind, TokenKind::Word);
    assert_eq!(t[2].text, "file");

    // >>
    let t = tokenize("echo >> file");
    assert_eq!(t[1].kind, TokenKind::GreatGreat);

    // 2>&1
    let t = tokenize("2>&1");
    assert_eq!(t[0].kind, cake_syntax::TokenKind::IoNumber);
    assert_eq!(t[0].text, "2");
    assert_eq!(t[1].kind, TokenKind::GreaterAnd);
    assert_eq!(t[1].text, ">&");

    // &> (bash extension)
    let t = tokenize("&>");
    assert_eq!(t[0].kind, TokenKind::AmpGreat);
    assert_eq!(t[0].text, "&>");

    // <<< here-string
    let t = tokenize("<<< word");
    assert_eq!(t[0].kind, TokenKind::LessLessLess);
    assert_eq!(t[1].kind, TokenKind::Word);
    assert_eq!(t[1].text, "word");
}

#[test]
fn quotes() {
    let t = tokenize("echo 'single' \"double\"");
    assert_eq!(t[0].text, "echo");
    assert_eq!(t[1].text, "'single'"); // quotes preserved in raw text
    assert_eq!(t[2].text, "\"double\"");
}

#[test]
fn command_position_operators() {
    let ctx = LexContext {
        cmd_pos: true,
        ..Default::default()
    };

    // ( subshell
    let t = tokenize_with_ctx("( echo hi )", ctx);
    assert_eq!(t[0].kind, TokenKind::Lparen);
    assert_eq!(t[1].kind, TokenKind::Word);
    assert_eq!(t[2].kind, TokenKind::Word);
    assert_eq!(t[3].kind, TokenKind::Rparen);

    // ! pipeline
    let t = tokenize_with_ctx("! true", ctx);
    assert_eq!(t[0].kind, TokenKind::Bang);
    assert_eq!(t[1].kind, TokenKind::Word);

    // { block
    let t = tokenize_with_ctx("{ echo hi; }", ctx);
    assert_eq!(t[0].kind, TokenKind::Lbrace);
    assert_eq!(t[1].kind, TokenKind::Word);
    assert_eq!(t[2].kind, TokenKind::Word);
    assert_eq!(t[3].kind, TokenKind::Semi);
    assert_eq!(t[4].kind, TokenKind::Rbrace);
}

#[test]
fn expansions() {
    // $var
    let t = tokenize("echo $HOME");
    assert_eq!(t[1].text, "$HOME");

    // ${var}
    let t = tokenize("echo ${PATH}");
    assert_eq!(t[1].text, "${PATH}");

    // $(cmd)
    let t = tokenize("echo $(whoami)");
    assert_eq!(t[1].text, "$(whoami)");

    // $((arith))
    let t = tokenize("echo $(( 1 + 2 ))");
    assert_eq!(t[1].text, "$(( 1 + 2 ))");
}

#[test]
fn double_bracket() {
    let ctx = LexContext {
        cmd_pos: true,
        ..Default::default()
    };
    let t = tokenize_with_ctx("[[ -n \"$var\" ]]", ctx);
    assert_eq!(t[0].kind, TokenKind::DoubleBracketOpen);
    assert_eq!(t[1].kind, TokenKind::Word);
    assert_eq!(t[2].text, "\"$var\"");
    // The ]] is recognized when in_cond context
    // But we need to set in_cond for the lexer to recognize ]]
    // Let me test with in_cond set
}

#[test]
fn double_bracket_close() {
    let ctx = LexContext {
        in_cond: true,
        ..Default::default()
    };
    let t = tokenize_with_ctx("]]", ctx);
    assert_eq!(t[0].kind, TokenKind::DoubleBracketClose);
}

#[test]
fn arith_command() {
    let ctx = LexContext {
        cmd_pos: true,
        in_arith: true,
        ..Default::default()
    };
    let t = tokenize_with_ctx("i = i + 1", ctx);
    assert_eq!(t[0].kind, TokenKind::Word);
    assert_eq!(t[0].text, "i");
    assert_eq!(t[1].kind, TokenKind::Word);
    assert_eq!(t[1].text, "=");
    assert_eq!(t[2].kind, TokenKind::Word);
    assert_eq!(t[2].text, "i");
    assert_eq!(t[3].kind, TokenKind::Word);
    assert_eq!(t[3].text, "+");
    assert_eq!(t[4].kind, TokenKind::Word);
    assert_eq!(t[4].text, "1");
}

#[test]
fn unterminated_quote_errors() {
    let mut lexer = Lexer::new("echo 'unfinished");
    let result = lexer.next_token();
    assert!(result.is_ok()); // echo
    let result = lexer.next_token();
    match result {
        Err(e) => assert!(e.is_incomplete),
        Ok(_) => panic!("expected error"),
    }
}

#[test]
fn assignment_detection() {
    let ctx = LexContext {
        cmd_pos: true,
        ..Default::default()
    };
    let t = tokenize_with_ctx("FOO=bar cmd", ctx);
    assert_eq!(t[0].kind, TokenKind::Assignment);
    assert_eq!(t[0].text, "FOO=bar");
    assert_eq!(t[1].kind, TokenKind::Word);
    assert_eq!(t[1].text, "cmd");
}

#[test]
fn heredoc_body() {
    // Position the lexer just after the `<<EOF` tokens: at the start of the
    // first body line.
    let mut lexer = Lexer::new("hello world\nEOF\n");
    let body = lexer.read_heredoc_body("EOF", false).unwrap();
    assert_eq!(body, "hello world\n");
}

#[test]
fn newline_terminator() {
    let t = tokenize("echo hello\n");
    // Should have: echo, hello, Newline, EOF
    assert_eq!(t.len(), 4);
    assert_eq!(t[2].kind, TokenKind::Newline);
}

#[test]
fn pipe_amp() {
    let t = tokenize("cmd |& cmd2");
    assert_eq!(t[1].kind, TokenKind::PipeAmp);
}

#[test]
fn amp_great() {
    let t = tokenize("&> file");
    assert_eq!(t[0].kind, TokenKind::AmpGreat);
    assert_eq!(t[1].kind, TokenKind::Word);
    assert_eq!(t[1].text, "file");
}

#[test]
fn amp_great_great() {
    let t = tokenize("&>> file");
    assert_eq!(t[0].kind, TokenKind::AmpGreatGreat);
    assert_eq!(t[1].kind, TokenKind::Word);
    assert_eq!(t[1].text, "file");
}

#[test]
fn ansi_c_quote() {
    let t = tokenize("echo $'hello\\nworld'");
    assert_eq!(t[1].text, "$'hello\\nworld'");
}

#[test]
fn nested_expansions() {
    // ${var:-$(echo hi)}
    let t = tokenize("echo ${var:-$(echo hi)}");
    assert_eq!(t[1].text, "${var:-$(echo hi)}");
}

#[test]
fn assignment_not_in_argument_position() {
    // "FOO=bar" as argument (not command position) should be a Word
    let ctx = LexContext::default();
    let t = tokenize_with_ctx("echo FOO=bar", ctx);
    assert_eq!(t[0].kind, TokenKind::Word);
    assert_eq!(t[0].text, "echo");
    assert_eq!(t[1].kind, TokenKind::Word);
    assert_eq!(t[1].text, "FOO=bar");
}
