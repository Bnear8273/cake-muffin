fn main() {
    let src = "for ((i=0; i<3; ++i)); do echo hi; done";
    match cake_syntax::parse(src) {
        Ok(p) => println!("PARSED: {:#?}", p),
        Err(e) => println!("ERR: {e:?}"),
    }
}
