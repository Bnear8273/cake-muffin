use cake_syntax::lexer::{LexContext, Lexer};

fn main() {
    for src in std::env::args().skip(1) {
        println!("=== {:?} ===", src);
        let mut lexer = Lexer::new(&src);
        lexer.ctx = LexContext {
            cmd_pos: true,
            in_case: true,
            ..Default::default()
        };
        loop {
            let tok = lexer.next_token().unwrap();
            println!("  {:?} {:?} span={:?}", tok.kind, tok.text, tok.span);
            if tok.kind == cake_syntax::TokenKind::Eof {
                break;
            }
        }
    }
}
