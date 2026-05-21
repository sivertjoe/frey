mod ast;
mod cli;
mod codegen;
mod driver;
mod hir;
mod lexer;
mod semantics;

use std::process::ExitCode;

use lexer::types::Span;

fn main() -> ExitCode {
    let args = match cli::parse() {
        cli::ParseOutcome::Run(a) => a,
        cli::ParseOutcome::Help => {
            print!("{}", cli::HELP);
            return ExitCode::SUCCESS;
        }
        cli::ParseOutcome::Version => {
            println!("frey {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        cli::ParseOutcome::Error(msg) => {
            eprintln!("error: {msg}");
            eprintln!();
            eprint!("{}", cli::HELP);
            return ExitCode::FAILURE;
        }
    };

    let file = args.input.to_string_lossy().into_owned();

    let src = match std::fs::read_to_string(&args.input) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: failed to read `{}`: {e}", args.input.display());
            return ExitCode::FAILURE;
        }
    };

    if args.verbose {
        eprintln!("[frey] lex");
    }
    let tokens = match lexer::tokenize(&src) {
        Ok(tokens) => tokens,
        Err(err) => {
            report(&file, &src, err.span, &err.kind.to_string());
            return ExitCode::FAILURE;
        }
    };

    if args.verbose {
        eprintln!("[frey] parse");
    }
    let ast = match ast::parse(tokens) {
        Ok(program) => program,
        Err(err) => {
            report(&file, &src, err.span, &err.kind.to_string());
            return ExitCode::FAILURE;
        }
    };

    if args.verbose {
        eprintln!("[frey] lower");
    }
    let hir = match hir::lower(ast) {
        Ok(program) => program,
        Err(err) => {
            report(&file, &src, err.span, &err.kind.to_string());
            return ExitCode::FAILURE;
        }
    };

    if args.verbose {
        eprintln!("[frey] typecheck");
    }
    if let Err(err) = semantics::type_check(&hir) {
        report(&file, &src, err.span, &err.kind.to_string());
        return ExitCode::FAILURE;
    }

    let output_path = args
        .output
        .clone()
        .unwrap_or_else(|| driver::default_output_path(&args.input, args.emit));

    let options = driver::BuildOptions {
        emit: args.emit,
        opt_level: args.opt_level,
        verbose: args.verbose,
        output_path,
    };

    if let Err(err) = driver::build(hir, &options) {
        eprintln!("error: {err}");
        return ExitCode::FAILURE;
    }

    ExitCode::SUCCESS
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
