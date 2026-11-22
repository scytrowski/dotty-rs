## Type one named method in its original session

The source adapters and namer below are test dependencies of this crate.
Applications assemble these phases explicitly. This example types one method;
it does not claim whole-program compilation or binary emission.

```rust
use dotty_core::{Definitions, Packages, SemanticStore, SourceId, SourceText, TreeKind};
use dotty_lexer::ContextualScanner;
use dotty_parser::Parser;
use dotty_namer::{name_compilation_unit_with_diagnostics, NamingOutcome, SourceParseStatus};
use dotty_typer::SourceTyper;

let source = SourceId::from_index(0);
let text = "class Answer { def value: Int = 42 }";
let mut store = SemanticStore::new();
let definitions = Definitions::bootstrap(&mut store);
let mut packages = Packages::new();
let mut scanner = ContextualScanner::new(text)?;
let lexer_diagnostics = scanner.take_diagnostics();
let parser = Parser::new(
    SourceText::new(text)?, source, scanner, &mut store.names,
);
let parsed = parser.source_compilation_unit();
assert!(parsed.diagnostics.is_empty());
assert!(lexer_diagnostics.is_empty());
let index = match name_compilation_unit_with_diagnostics(
    &parsed.ast, parsed.root, source, "Answer.scala", SourceParseStatus::Clean,
    &mut store, &mut packages,
)? {
    NamingOutcome::Named { index, .. } => index,
    NamingOutcome::Rejected { diagnostics } => {
        return Err(format!("naming rejected: {diagnostics:?}").into());
    }
};
let (method, _) = parsed.ast.iter()
    .find(|(_, tree)| matches!(tree.kind, TreeKind::DefDef(_)))
    .ok_or("expected method declaration")?;
let mut typer = SourceTyper::new(
    &parsed.ast, source, &index, &mut store, definitions, &packages,
);
let typed = typer.type_method_definition(method)?;
assert!(matches!(typer.typed_ast().get(typed).kind, TreeKind::DefDef(_)));
# Ok::<(), Box<dyn std::error::Error>>(())
```

Unsupported or ambiguous cases return [`TyperError`], rather than choosing a
plausible declaration or manufacturing a type. A failed public typing
transaction restores its semantic and typed state; earlier successful calls
remain. Preserve the exact typed arena and source-to-typed bindings for later
backend or pickler planning. A same-slot foreign arena ID cannot be
authenticated by range checks.

Read committed warnings with [`SourceTyper::diagnostics`] or drain them with
[`SourceTyper::take_diagnostics`]. Match the [`TyperWarning`] payload to
classify a warning. Its range is measured in UTF-8 bytes in the typer’s source
file. Failed typing attempts discard their own warnings.

Use [`SourceTyper::with_resolver`] to supply external symbols through the core
resolver contract. Loading class files, discovering JDK modules, and decoding
TASTy remain adapter responsibilities. Signature completion, expression typing,
whole-class checking, and producer support have different supported subsets.
