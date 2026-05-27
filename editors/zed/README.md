# Frey support for Zed

Tree-sitter–based syntax highlighting for the Frey language.

- The **grammar** lives in [`../tree-sitter-frey`](../tree-sitter-frey)
  (`grammar.js` + the generated `src/parser.c`).
- This folder is the **Zed extension**: `extension.toml` plus the queries in
  `languages/frey/` (`highlights.scm`, `brackets.scm`) and `config.toml`.

## Install

Zed builds grammars from a git repo, so the grammar has to be pushed first.

1. Commit and push this repository (make sure
   `editors/tree-sitter-frey/src/parser.c` is committed — `node_modules/` is
   ignored, but `src/` is kept).
2. In `extension.toml`, set:
   - `repository` → your repo's URL
   - `rev` → the commit SHA you just pushed
   - `path` stays `editors/tree-sitter-frey`
3. In Zed: open the command palette and run **`zed: install dev extension`**,
   then choose this `editors/zed` folder.
4. Open any `.frey` file.

When you edit the grammar, you do **not** need to re-push to see query changes —
Zed reloads the local dev extension's queries. But grammar (`grammar.js`)
changes require regenerating and re-pushing (and bumping `rev`).

## Working on the grammar

```sh
cd editors/tree-sitter-frey
npm install            # one-time: installs tree-sitter-cli locally
npx tree-sitter generate
npx tree-sitter parse path/to/file.frey      # inspect the parse tree
npx tree-sitter query ../zed/languages/frey/highlights.scm path/to/file.frey
```

The grammar parses the full surface syntax (declarations, `struct<$K,$V>`,
generics, `#comptime`, `defer`, pointers, generic calls/struct literals, UFCS
method calls, `//` and `/* */` comments, …).
