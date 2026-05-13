mod lexer;

fn main() {
    let args = std::env::args().collect::<Vec<_>>();
    let file = args[1].clone();

    let s = std::fs::read_to_string(file).unwrap();
    let tokens = lexer::tokenize(&s).unwrap();
    println!("{:#?}", tokens);
}
