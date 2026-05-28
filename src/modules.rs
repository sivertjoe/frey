//! Module loading: resolves `import "..."` by reading the referenced files and
//! merging their top-level declarations into one program (a flat namespace,
//! each file included at most once). Builds a [`SourceMap`] so diagnostics can
//! point at the right file even when declarations come from several files.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::ast;
use crate::lexer;
use crate::lexer::types::Span;

/// One loaded source file, tagged with the global byte offset where it starts.
pub struct SourceFile {
    pub path: PathBuf,
    pub src: String,
    pub base: usize,
}

/// Maps a global `Position::offset` back to the file it came from.
pub struct SourceMap {
    files: Vec<SourceFile>,
    total: usize,
}

impl SourceMap {
    fn new() -> Self {
        Self {
            files: Vec::new(),
            total: 0,
        }
    }

    fn add(&mut self, path: PathBuf, src: String) -> usize {
        let base = self.total;
        self.total += src.len();
        self.files.push(SourceFile { path, src, base });
        base
    }

    /// The file containing a global byte offset — the last file whose base is
    /// `<= offset` (files are stored in increasing base order).
    pub fn file_for_offset(&self, offset: usize) -> Option<&SourceFile> {
        self.files.iter().rev().find(|f| f.base <= offset)
    }
}

/// A failure while loading the module graph. `span` (when present) uses global
/// offsets, so it can be reported against the [`SourceMap`].
pub struct ModuleError {
    pub span: Option<Span>,
    pub message: String,
}

struct Loader {
    sources: SourceMap,
    loaded: HashSet<PathBuf>,
    declarations: Vec<ast::Declaration>,
    // Node ids are chained across files so they stay globally unique.
    next_node_id: u32,
}

/// Loads `entry` and everything it transitively imports, merging all top-level
/// declarations into one program. The source map is always returned (even on
/// error) so the caller can report diagnostics.
pub fn resolve(entry: &Path) -> (SourceMap, Result<ast::Program, ModuleError>) {
    let mut loader = Loader {
        sources: SourceMap::new(),
        loaded: HashSet::new(),
        declarations: Vec::new(),
        next_node_id: 0,
    };
    let result = loader.load(entry, None).map(|()| ast::Program {
        span: Span::default(),
        declarations: std::mem::take(&mut loader.declarations),
        imports: Vec::new(),
    });
    (loader.sources, result)
}

impl Loader {
    fn load(&mut self, path: &Path, import_span: Option<Span>) -> Result<(), ModuleError> {
        let canonical = std::fs::canonicalize(path).map_err(|_| ModuleError {
            span: import_span,
            message: format!("cannot find module `{}`", path.display()),
        })?;

        // Each file is included at most once; this also makes import cycles safe.
        if !self.loaded.insert(canonical.clone()) {
            return Ok(());
        }

        let src = std::fs::read_to_string(&canonical).map_err(|e| ModuleError {
            span: import_span,
            message: format!("failed to read `{}`: {e}", canonical.display()),
        })?;
        let base = self.sources.add(canonical.clone(), src.clone());

        let tokens = lexer::tokenize_at(&src, base).map_err(|e| ModuleError {
            span: Some(e.span),
            message: e.kind.to_string(),
        })?;
        let (program, next_node_id) =
            ast::parse_at(tokens, self.next_node_id).map_err(|e| ModuleError {
                span: Some(e.span),
                message: e.kind.to_string(),
            })?;
        self.next_node_id = next_node_id;

        // Resolve imports first so a dependency's declarations come earlier.
        let dir = canonical
            .parent()
            .unwrap_or_else(|| Path::new(""))
            .to_path_buf();
        for import in &program.imports {
            let target = resolve_import_path(&dir, &import.path);
            self.load(&target, Some(import.span))?;
        }
        self.declarations.extend(program.declarations);
        Ok(())
    }
}

/// Resolves `import "x"` relative to the importing file's directory, appending
/// the `.frey` extension unless it's already there.
fn resolve_import_path(dir: &Path, path: &str) -> PathBuf {
    let with_ext = if path.ends_with(".frey") {
        path.to_string()
    } else {
        format!("{path}.frey")
    };
    dir.join(with_ext)
}
