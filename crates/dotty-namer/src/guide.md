## Enter declarations before completing signatures

This example uses `dotty-lexer` and `dotty-parser` as source adapters. They
are test dependencies here, not production dependencies of the naming phase.

```rust
use dotty_core::{Definitions, Packages, SemanticStore, SourceId, SourceText, TreeKind};
use dotty_lexer::ContextualScanner;
use dotty_parser::Parser;
use dotty_namer::{NamingOutcome, SourceParseStatus, name_compilation_unit_with_diagnostics};

let source = SourceId::from_index(0);
let mut store = SemanticStore::new();
let _definitions = Definitions::bootstrap(&mut store);
let mut packages = Packages::new();
let text = "class Answer { def value: Int = 42 }";
let mut scanner = ContextualScanner::new(text)?;
let lexer_diagnostics = scanner.take_diagnostics();
let parser = Parser::new(
    SourceText::new(text)?, source, scanner, &mut store.names,
);
let parsed = parser.source_compilation_unit();
assert!(parsed.diagnostics.is_empty());
assert!(lexer_diagnostics.is_empty());
let outcome = name_compilation_unit_with_diagnostics(
    &parsed.ast, parsed.root, source, "Answer.scala", SourceParseStatus::Clean,
    &mut store, &mut packages,
)?;
let index = match outcome {
    NamingOutcome::Named { index, diagnostics } => {
        assert!(diagnostics.is_empty());
        index
    }
    NamingOutcome::Rejected { diagnostics } => {
        return Err(format!("source rejected: {diagnostics:?}").into());
    }
};
let (method, _) = parsed.ast.iter()
    .find(|(_, tree)| matches!(tree.kind, TreeKind::DefDef(_)))
    .ok_or("expected method declaration")?;
assert!(index.symbol_at(source, method).is_some());
# Ok::<(), Box<dyn std::error::Error>>(())
```

`SourceParseStatus::Clean` is a caller assertion: inspect both lexer and parser
reports before choosing it. Confirmed source rejections are returned as
[`NamingOutcome::Rejected`]; structural and invariant failures are
[`NamerError`] values. Both failed paths roll back naming's transaction and
publish no partial semantic index. Successful naming records declaration
identities, owners, scopes, and lexical contexts; it does not establish
complete signatures or accept arbitrary expression semantics.

Keep the original AST, source ID, returned index, package registry, and semantic
store together for typing. Do not reconstruct declaration identity from names
or arena allocation order.
