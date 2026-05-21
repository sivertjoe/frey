use std::ffi::OsString;
use std::path::PathBuf;

pub const HELP: &str = "\
frey — Frey language compiler

USAGE:
    frey [OPTIONS] <input>

OPTIONS:
    -o <PATH>             Write output to <PATH>
    -S                    Emit assembly, don't link
    -R, --emit-llvm-ir    Emit LLVM IR, don't link
    -O0 | -O1 | -O2 | -O3 Optimization level (default: -O0)
    -v, --verbose         Print compilation stages
    -h, --help            Show this help and exit
    -V, --version         Show version and exit
";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Emit {
    Executable,
    Assembly,
    LlvmIr,
}

#[derive(Debug)]
pub struct Args {
    pub input: PathBuf,
    pub output: Option<PathBuf>,
    pub emit: Emit,
    pub opt_level: u8,
    pub verbose: bool,
}

pub enum ParseOutcome {
    Run(Args),
    Help,
    Version,
    Error(String),
}

pub fn parse() -> ParseOutcome {
    let raw: Vec<OsString> = std::env::args_os().skip(1).collect();
    let (raw, opt_level) = match extract_opt_level(raw) {
        Ok(pair) => pair,
        Err(msg) => return ParseOutcome::Error(msg),
    };

    let mut pargs = pico_args::Arguments::from_vec(raw);

    if pargs.contains(["-h", "--help"]) {
        return ParseOutcome::Help;
    }
    if pargs.contains(["-V", "--version"]) {
        return ParseOutcome::Version;
    }

    let verbose = pargs.contains(["-v", "--verbose"]);

    let emit_s = pargs.contains("-S");
    let emit_ir = pargs.contains(["-R", "--emit-llvm-ir"]);
    let emit = match (emit_s, emit_ir) {
        (false, false) => Emit::Executable,
        (true, false) => Emit::Assembly,
        (false, true) => Emit::LlvmIr,
        (true, true) => {
            return ParseOutcome::Error("cannot combine -S and -R/--emit-llvm-ir".into());
        }
    };

    let output: Option<PathBuf> = match pargs.opt_value_from_str("-o") {
        Ok(v) => v,
        Err(e) => return ParseOutcome::Error(format!("invalid value for -o: {e}")),
    };

    let remaining = pargs.finish();
    if remaining.is_empty() {
        return ParseOutcome::Error("missing input file".into());
    }
    if remaining.len() > 1 {
        return ParseOutcome::Error(format!("unexpected extra arguments: {:?}", &remaining[1..]));
    }
    let input = PathBuf::from(&remaining[0]);

    ParseOutcome::Run(Args {
        input,
        output,
        emit,
        opt_level,
        verbose,
    })
}

fn extract_opt_level(args: Vec<OsString>) -> Result<(Vec<OsString>, u8), String> {
    let mut level: Option<u8> = None;
    let mut filtered: Vec<OsString> = Vec::with_capacity(args.len());
    for a in args {
        let matched = match a.to_str() {
            Some("-O0") => Some(0u8),
            Some("-O1") => Some(1),
            Some("-O2") => Some(2),
            Some("-O3") => Some(3),
            _ => None,
        };
        match matched {
            Some(n) => {
                if let Some(prev) = level {
                    if prev != n {
                        return Err(format!(
                            "conflicting optimization levels: -O{prev} and -O{n}"
                        ));
                    }
                }
                level = Some(n);
            }
            None => filtered.push(a),
        }
    }
    Ok((filtered, level.unwrap_or(0)))
}
