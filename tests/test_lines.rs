fn main() {
    let content = "Line 1\nLine 2\nLine 3";
    let lines: Vec<&str> = content.lines().collect();
    for l in lines {
        println!("{}", l);
    }
}
