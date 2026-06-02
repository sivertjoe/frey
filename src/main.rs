mod ast;
mod cli;
mod codegen;
mod driver;
mod hir;
mod lexer;
mod modules;
mod semantics;

use std::process::ExitCode;

use lexer::types::Span;

fn main() -> ExitCode {
    // Compiler passes recurse through deeply nested AST nodes and
    // `lower_expr_with_hint_inner` has a giant match whose stack frame is
    // tens of KB in debug builds. Windows' default 1 MB thread stack
    // overflows on moderately deep programs (~9 frames). Run the
    // compiler on a dedicated thread with a generous stack — release
    // builds collapse the frame but debug builds need the headroom.
    let h = std::thread::Builder::new()
        .stack_size(32 * 1024 * 1024)
        .spawn(real_main)
        .expect("spawn compiler worker");
    h.join().unwrap_or(ExitCode::FAILURE)
}

fn real_main() -> ExitCode {
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

    if args.verbose {
        eprintln!("[frey] load modules");
    }
    let (sources, parse_result) = modules::resolve(&args.input);
    let ast = match parse_result {
        Ok(program) => program,
        Err(err) => {
            match err.span {
                Some(span) => report(&sources, span, &err.message),
                None => eprintln!("\x1b[31;1merror\x1b[0m: {}", err.message),
            }
            return ExitCode::FAILURE;
        }
    };

    if args.verbose {
        eprintln!("[frey] lower");
    }
    let hir = match hir::lower(ast) {
        Ok(program) => program,
        Err(err) => {
            report(&sources, err.span, &err.kind.to_string());
            return ExitCode::FAILURE;
        }
    };

    if args.verbose {
        eprintln!("[frey] typecheck");
    }
    if let Err(err) = semantics::type_check(&hir) {
        report(&sources, err.span, &err.kind.to_string());
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

fn report(sources: &modules::SourceMap, span: Span, message: &str) {
    let red = "\x1b[31;1m";
    let blue = "\x1b[34;1m";
    let reset = "\x1b[0m";

    let line_num = span.start.line;
    let col = span.start.column;

    // Find which file this (global) offset belongs to; fall back to a bare
    // message for synthetic spans with no source.
    let Some(file) = sources.file_for_offset(span.start.offset).filter(|_| line_num > 0) else {
        eprintln!("{red}error{reset}: {message}");
        return;
    };

    let line_text = file
        .src
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
    let indent = " ".repeat(col.saturating_sub(1));
    let carets = "^".repeat(caret_len);

    // Strip the Windows verbatim prefix (`\\?\`) for nicer paths.
    let path = file.path.to_string_lossy();
    let path = path.strip_prefix(r"\\?\").unwrap_or(&path);

    eprintln!("{red}error{reset}: {message}");
    eprintln!("{pad} {blue}-->{reset} {path}:{line_num}:{col}");
    eprintln!("{pad} {blue}|{reset}");
    eprintln!("{blue}{gutter} |{reset} {line_text}");
    eprintln!("{pad} {blue}|{reset} {indent}{red}{carets}{reset}");
}
