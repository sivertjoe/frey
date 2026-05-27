# Frey syntax highlighting (VS Code)

A minimal VS Code extension providing TextMate-based syntax highlighting for
`.frey` files. No build step — it's a pure declarative grammar.

## Install (local)

Symlink or copy this folder into your VS Code extensions directory, then
reload VS Code:

- Windows: `%USERPROFILE%\.vscode\extensions\frey`
- macOS / Linux: `~/.vscode/extensions/frey`

```sh
# from the repo root
ln -s "$PWD/editors/vscode" ~/.vscode/extensions/frey   # macOS/Linux
```

On Windows (PowerShell):

```powershell
New-Item -ItemType SymbolicLink -Path "$env:USERPROFILE\.vscode\extensions\frey" -Target "$PWD\editors\vscode"
```

Open any `.frey` file and it will be highlighted.

## What it highlights

- Keywords: `let`, `mut`, `if`, `else`, `while`, `return`, `break`, `defer`,
  `struct`, `as`, and the `#comptime` directive
- Built-in types (`Int`, `i32`, `f64`, …) and capitalized type names
- Generic type parameters (`$T`)
- Intrinsics (`alloc`, `realloc`, `free`, `comperror`)
- Function/struct definition names, function & method calls
- `//` line and `/* */` (nesting) block comments, strings, numbers, operators

## Other editors

For Neovim / Helix / Zed, highlighting uses Tree-sitter (`highlights.scm`),
which needs a Tree-sitter grammar for Frey. That's a separate, larger piece of
work; ask if you'd like it built.
