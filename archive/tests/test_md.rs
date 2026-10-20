fn main() {
    let content = "First line\n\nSecond line\n\nThird line";
    let parser = pulldown_cmark::Parser::new(content);
    for event in parser {
        println!("{:?}", event);
    }
}
