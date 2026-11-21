# LSP integration fixture

This standalone Rust package exercises the public `dotty` API as a language
server would. Cargo builds a test executable named `dotty-lsp`. The fixture
owns its workspace and lockfile, so its LSP dependencies stay outside the
compiler library. The production language server belongs with the future
compiler in a separate repository.

From this directory, run the protocol regression with:

```sh
cargo test --locked
```

The integration test starts the executable and exchanges LSP messages over
standard input and output. It opens two CRLF documents, edits a call in memory
to select a different overload, and checks the declaration and name ranges in
the other document. A type error in a sibling method produces a versioned
diagnostic while navigation still works. Repairing the method clears the
diagnostic. The test runs with UTF-8 and UTF-16 positions and an emoji before
the call. A second scenario checks an inferred field in Consumer.scala against
a method in Provider.scala in both file orders. Signature and body checking
report one error at `provider.value(1)`. Editing it to `provider.value()`
clears the diagnostic. The fixture gets these reports from
`MemberSignatureError::diagnostic` and `ClassBodyError::diagnostic`.
For a header error in `extends Base(start)`, the fixture uses
`TypingDiagnostic::from_typer`. A protocol test checks that the marker on the
parent argument clears after an unsaved edit to the constructor parameter type.
When no source range is available, it sends the message to the editor log
and leaves the document free of a marker for that error.

The field-declaration scenario opens `private[Missing] val hidden: Int 1`.
It checks for one parser report at the literal. Inserting `=` exposes the
naming report at `Missing`, and removing the qualifier clears the document.
A subsequent edit puts two declarations on one line. Inserting a semicolon
clears the resulting separator diagnostic.
Both clients bind the result of `dotty::parse_source` with `DetachedSource::bind`
to obtain owned `SyntaxDiagnostic::Lexer` and `SyntaxDiagnostic::Parser` values.
The server renders these during publication and uses
`SourceDiagnostic::from_namer` for naming issues. The fixture publishes
their stable codes in the LSP diagnostic's `code` field.

A separate protocol test opens a field with a missing equals sign and a trailing
unterminated comment. Closing the comment removes its marker while the parser
marker stays on the literal. Inserting the equals sign clears the document.
Reopening the comment produces a lexer diagnostic even when parsing is clean.
The test checks the document version and exact UTF-8 and UTF-16 positions.

For a quick interoperability check with Neovim 0.10+, run from this directory:

```sh
cargo build --locked
DOTTY_LSP_BIN="$PWD/target/debug/dotty-lsp" nvim --clean --headless -l tests/editor.lua
```

The editor command jumps from a call in B.scala to the method name in A.scala.
