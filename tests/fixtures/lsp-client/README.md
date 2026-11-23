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
clears the diagnostic. The fixture uses
`ReportedTyperError` for both call and ordinary typing failures. Signature and
body errors supply the enclosing class and member through `into_reported_error`.
Within one analysis the fixture suppresses a body retry when the signature
already produced the same `CallErrorIdentity`. It renders the message at
publication. Two inferred fields with matching bad calls retain independent
markers as each field is repaired.

For constructor and header errors the fixture receives an owned report from
`NamedClassSources::complete_constructor_signature` or
`NamedClassSources::try_complete_class_header_reported`. A protocol test checks that the
marker on the parent argument clears after an unsaved change to its type.
Another test opens a constructor with an unknown type after an emoji. It checks
the range in both position encodings, repairs the type in memory and resolves
a call to the same overload in a separate file before and after the repair.
A class waiting on the broken constructor stays free of secondary diagnostics.
Its own call becomes navigable once the base class is repaired.
When no source range is available, it sends the message to the editor log
and leaves the document free of a marker for that error.

The fixture records header and member dependencies separately. Completion wakes
direct waiters, and diagnostics for a cycle point to each participating
declaration. A stdio regression repairs a cross-file inheritance cycle and
keeps an independent call error in the same document through the edit.

The field-declaration scenario opens `private[Missing] val hidden: Int 1`.
It checks for one parser report at the literal. Inserting `=` exposes the
naming report at `Missing`, and removing the qualifier clears the document.
A subsequent edit puts two declarations on one line. Inserting a semicolon
clears the resulting separator diagnostic.
Both clients bind the result of `dotty::parse_source` with `DetachedSource::bind`
to obtain owned `SyntaxDiagnostic::Lexer` and `SyntaxDiagnostic::Parser` values.
The LSP fixture names clean files through `ParsedSource::name` and collects their
ordinary-class views in `NamedClassSources`. It uses this collection to resolve signatures by
symbol and find the caller and target files for direct-call navigation.
The stdio test continues navigating between two clean files while a third
moves through parser rejection, namer rejection and repair.
The server renders syntax diagnostics during publication and uses
`SourceDiagnostic::from_namer` for naming issues. The fixture publishes
their stable codes in the LSP diagnostic's `code` field.

A separate protocol test opens a field with a missing equals sign and a trailing
unterminated comment. Closing the comment removes its marker while the parser
marker stays on the literal. Inserting the equals sign clears the document.
Reopening the comment produces a lexer diagnostic even when parsing is clean.
The test checks the document version and exact UTF-8 and UTF-16 positions.

For a positional call to a legacy implicit clause, analysis retains the typed
warning until publication. The stdio test checks its stable code, warning
severity and argument range alongside an independent argument type error. After
the type error is repaired, editing `f(1)` to `f(using 1)` clears the warning.

For a quick interoperability check with Neovim 0.10+, run from this directory:

```sh
cargo build --locked
DOTTY_LSP_BIN="$PWD/target/debug/dotty-lsp" nvim --clean --headless -l tests/editor.lua
```

The editor command jumps from a call in B.scala to the method name in A.scala.
