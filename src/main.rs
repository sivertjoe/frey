mod lexer;

use lexer::types::Span;

fn main() {
    let args = std::env::args().collect::<Vec<_>>();
    let file = args[1].clone();

    let src = std::fs::read_to_string(&file).unwrap();

    let tokens = match lexer::tokenize(&src) {
        Ok(tokens) => tokens,
        Err(err) => {
            report(&file, &src, err.span, &err.kind.to_string());
            std::process::exit(1);
        }
    };

    //println!("{:#?}", tokens);
}

fn report(file: &str, src: &str, span: Span, message: &str) {
    let line_num = span.start.line;
    let col = span.start.column;

    let line_text = src
        .split('\n')
        .nth(line_num - 1)
        .unwrap_or("")
        .trim_end_matches('\r');

    let caret_len = if span.end.line == span.start.line {
        span.end.column.saturating_sub(span.start.column).max(1)
    } else {
        line_text.chars().count().saturating_sub(col - 1).max(1)
    };

    let gutter = line_num.to_string();
    let pad = " ".repeat(gutter.len());
    let indent = " ".repeat(col - 1);
    let carets = "^".repeat(caret_len);

    let red = "\x1b[31;1m";
    let blue = "\x1b[34;1m";
    let reset = "\x1b[0m";

    eprintln!("{red}error{reset}: {message}");
    eprintln!("{pad} {blue}-->{reset} {file}:{line_num}:{col}");
    eprintln!("{pad} {blue}|{reset}");
    eprintln!("{blue}{gutter} |{reset} {line_text}");
    eprintln!("{pad} {blue}|{reset} {indent}{red}{carets}{reset}");
}
