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
let text = "class Answer { def value: Int = { val n = 42; val result: Int = n; result } }";
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
let owner = index.symbol_at(source, method).ok_or("expected method symbol")?;
let TreeKind::DefDef(def) = &parsed.ast.get(method).kind else { unreachable!() };
let TreeKind::Block(body) = &parsed.ast.get(def.rhs.unwrap()).kind else { unreachable!() };
let TreeKind::ValDef(result) = &parsed.ast.get(body.stats[1]).kind else { unreachable!() };
let initializer = result.rhs.ok_or("expected initializer")?;
let value_type = typer.inspect_method_expression(owner, initializer, |probe, ctx| {
    assert!(probe.is_member_accessible_at(owner, initializer, ctx)?);
    let candidates = probe.lookup_identifier_candidates(initializer, ctx)?;
    assert_eq!(candidates, [probe.local_symbol_at(source, body.stats[0]).unwrap()]);
    let result = probe.type_expression(initializer, ctx)?;
    probe.widen_expression_type(probe.typed_ast().get(result).ty)
})?.ok_or("unsupported expression path")??;
assert_eq!(value_type, definitions.int);
let typed = typer.type_method_definition(method)?;
assert!(matches!(typer.typed_ast().get(typed).kind, TreeKind::DefDef(_)));
# Ok::<(), Box<dyn std::error::Error>>(())
```

For a direct superclass application, [`SourceTyper::inspect_parent_arguments`]
provides the primary constructor's scope and guards the uninitialized child
receiver. Pass the exact parent tree and inspect only the already written operands.
The probe can run before the child header completes.

Unsupported or ambiguous cases return [`TyperError`], rather than choosing a
plausible declaration or manufacturing a type. A failed public typing
transaction restores its semantic and typed state; earlier successful calls
remain. Preserve the exact typed arena and source-to-typed bindings for later
backend or pickler planning. A same-slot foreign arena ID cannot be
authenticated by range checks.

For a rejected term application, match [`TyperError::ApplicationArityMismatch`]
to inspect the argument counts or [`TyperError::ApplicationArgumentTypeMismatch`]
to inspect the zero-based argument index. Both expose stable categories through
[`TyperError::call_error_code`]. An argument type mismatch owns its expected and
actual type descriptions at the point of failure. Use those for presentation
after rollback, since the failed attempt's type slots may have been reused.

[`TyperError::diagnostic_code`] also classifies missing type and term names
and an expression that does not conform to its expected type. Other variants
can return `None`. Match the error directly to inspect its payload.

Read committed warnings with [`SourceTyper::diagnostics`] or drain them with
[`SourceTyper::take_diagnostics`]. Match the [`TyperWarning`] payload to
classify a warning. Its range is measured in UTF-8 bytes in the typer’s source
file. Failed typing attempts discard their own warnings.

Use [`SourceTyper::with_resolver`] to supply external symbols through the core
resolver contract. Loading class files, discovering JDK modules, and decoding
TASTy remain adapter responsibilities. Signature completion, expression typing,
whole-class checking, and producer support have different supported subsets.

Each [`TypedLocalBinding`] in [`TypedSourceOutput::local_bindings`] identifies
the original file and declaration tree, its typed declaration, its symbol and
its block scope. Resolve the written name against that file’s original parse.
Bindings from a failed typing transaction are absent from the output.
