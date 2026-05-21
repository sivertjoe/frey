use std::path::{Path, PathBuf};
use std::process::Command;

use inkwell::context::Context;

use crate::cli::Emit;
use crate::codegen::{self, Codegen};
use crate::hir::Program;

#[derive(Debug)]
pub enum Error {
    Codegen(codegen::Error),
    Io(std::io::Error),
    ClangNotFound,
    ClangFailed { code: Option<i32> },
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
            Error::ClangNotFound => write!(f, "`clang` not found on PATH"),
            Error::ClangFailed { code } => match code {
                Some(c) => write!(f, "clang exited with status {c}"),
                None => write!(f, "clang terminated by signal"),
            },
        }
    }
}

impl std::error::Error for Error {}

pub struct BuildOptions {
    pub emit: Emit,
    pub opt_level: u8,
    pub verbose: bool,
    pub output_path: PathBuf,
}

pub fn build(program: Program, options: &BuildOptions) -> Result<(), Error> {
    if options.verbose {
        eprintln!("[frey] codegen: emit LLVM IR in memory");
    }
    let context = Context::create();
    let mut codegen = Codegen::new(&context, "frey");
    codegen.lower(program)?;

    let ir_path: PathBuf = match options.emit {
        Emit::LlvmIr => options.output_path.clone(),
        Emit::Assembly | Emit::Executable => options.output_path.with_extension("ll"),
    };

    if options.verbose {
        eprintln!("[frey] write IR: {}", ir_path.display());
    }
    codegen.write_ir_to_file(&ir_path)?;

    if matches!(options.emit, Emit::LlvmIr) {
        return Ok(());
    }

    if Command::new("clang").arg("--version").output().is_err() {
        return Err(Error::ClangNotFound);
    }

    let mut cmd = Command::new("clang");

    #[cfg(target_os = "windows")]
    if let Some(tool) = cc::windows_registry::find_tool("x86_64-pc-windows-msvc", "cl.exe") {
        for (k, v) in tool.env() {
            cmd.env(k, v);
        }
    }

    cmd.arg("-Wno-override-module")
        .arg(format!("-O{}", options.opt_level));
    if matches!(options.emit, Emit::Assembly) {
        cmd.arg("-S");
    }
    cmd.arg(&ir_path).arg("-o").arg(&options.output_path);

    if options.verbose {
        eprintln!("[frey] clang: {cmd:?}");
    }

    let status = cmd.status()?;
    if !status.success() {
        return Err(Error::ClangFailed {
            code: status.code(),
        });
    }

    // Clean up the intermediate .ll when it's not the requested output.
    if !matches!(options.emit, Emit::LlvmIr) && ir_path != options.output_path {
        let _ = std::fs::remove_file(&ir_path);
    }

    Ok(())
}

pub fn default_output_path(input: &Path, emit: Emit) -> PathBuf {
    let stem = input
        .file_stem()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("out"));
    let ext = match emit {
        Emit::Executable => std::env::consts::EXE_EXTENSION,
        Emit::Assembly => "s",
        Emit::LlvmIr => "ll",
    };
    if ext.is_empty() {
        stem
    } else {
        stem.with_extension(ext)
    }
}
