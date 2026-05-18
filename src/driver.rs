use std::path::Path;
use std::process::Command;

use inkwell::context::Context;

use crate::codegen::{self, Codegen};
use crate::hir::Program;

#[derive(Debug)]
pub enum Error {
    Codegen(codegen::Error),
    Io(std::io::Error),
    LlcNotFound,
    LlcFailed { code: Option<i32> },
    LinkerNotFound,
    LinkerFailed { command: String, code: Option<i32> },
}

impl From<codegen::Error> for Error {
    fn from(e: codegen::Error) -> Self {
        Error::Codegen(e)
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Codegen(e) => write!(f, "codegen: {e}"),
            Error::Io(e) => write!(f, "io: {e}"),
            Error::LlcNotFound => write!(f, "`llc` not found on PATH"),
            Error::LlcFailed { code } => match code {
                Some(c) => write!(f, "llc exited with status {c}"),
                None => write!(f, "llc terminated by signal"),
            },
            Error::LinkerNotFound => write!(
                f,
                "no C compiler found on PATH (looked for $CC, cc, clang, gcc)"
            ),
            Error::LinkerFailed { command, code } => match code {
                Some(c) => write!(f, "linker `{command}` exited with status {c}"),
                None => write!(f, "linker `{command}` terminated by signal"),
            },
        }
    }
}

impl std::error::Error for Error {}

pub fn build(program: Program, output_path: &Path) -> Result<(), Error> {
    let context = Context::create();
    let mut codegen = Codegen::new(&context, "frey");
    codegen.lower(program)?;

    let ir_path = output_path.with_extension("ll");
    std::fs::write(&ir_path, codegen.module_ir())?;

    let object_path = output_path.with_extension("o");
    let llc = find_llc().ok_or(Error::LlcNotFound)?;
    let llc_status = Command::new(&llc)
        .arg(&ir_path)
        .arg("-filetype=obj")
        .arg("-o")
        .arg(&object_path)
        .status()?;
    if !llc_status.success() {
        return Err(Error::LlcFailed {
            code: llc_status.code(),
        });
    }

    let linker = find_linker().ok_or(Error::LinkerNotFound)?;
    let status = Command::new(&linker)
        .arg(&object_path)
        .arg("-o")
        .arg(output_path)
        .status()?;
    if !status.success() {
        return Err(Error::LinkerFailed {
            command: linker,
            code: status.code(),
        });
    }

    Ok(())
}

fn find_llc() -> Option<String> {
    if let Ok(llc) = std::env::var("LLC") {
        return Some(llc);
    }
    if Command::new("llc").arg("--version").output().is_ok() {
        return Some("llc".to_string());
    }
    None
}

fn find_linker() -> Option<String> {
    if let Ok(cc) = std::env::var("CC") {
        return Some(cc);
    }
    for candidate in ["cc", "clang", "gcc"] {
        if Command::new(candidate).arg("--version").output().is_ok() {
            return Some(candidate.to_string());
        }
    }
    None
}
