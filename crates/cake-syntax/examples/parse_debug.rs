use cake_syntax::parse;

fn main() {
    let src = std::env::args().skip(1).collect::<Vec<_>>().join(" ");
    println!("input len: {}", src.len());
    match parse(&src) {
        Ok(p) => {
            for cc in &p.commands {
                println!(
                    "  complete: {} pipelines",
                    cc.list.first.commands.len() + cc.list.rest.len()
                );
                let cmd = &cc.list.first.commands[0];
                if let cake_syntax::CommandKind::Case(cs) = &cmd.kind {
                    println!("  case: {} arms", cs.arms.len());
                    for (i, arm) in cs.arms.iter().enumerate() {
                        println!(
                            "    arm {}: {} patterns, body {} items",
                            i,
                            arm.patterns.len(),
                            arm.body.items.len()
                        );
                    }
                }
            }
        }
        Err(errs) => println!("{} errors: {:?}", errs.len(), errs),
    }
}
