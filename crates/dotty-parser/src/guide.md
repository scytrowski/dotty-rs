## Parse with a concrete token source

This example uses `dotty-lexer` alongside `dotty-parser` and `dotty-core`.
The lexer is only a test dependency of this crate; consumers choose their own
implementation of [`dotty_core::TokenSource`].

```rust
use dotty_core::{NameInterner, SourceId, SourceText, TreeKind};
use dotty_lexer::ContextualScanner;
use dotty_parser::Parser;

let text = "class Answer { def value: Int = 42 }";
let mut names = NameInterner::new();
let mut scanner = ContextualScanner::new(text)?;
let lexer_diagnostics = scanner.take_diagnostics();
let parser = Parser::new(
    SourceText::new(text)?, SourceId::from_index(0), scanner, &mut names,
);
let result = parser.source_compilation_unit();
assert!(result.diagnostics.is_empty());
assert!(matches!(result.ast.get(result.root).kind, TreeKind::PackageDef(_)));
assert!(lexer_diagnostics.is_empty());
# Ok::<(), Box<dyn std::error::Error>>(())
```

Successful parsing can return recovered trees and parser diagnostics. Lexer
diagnostics remain on the token source and must also be handled. Neither an
empty diagnostic list nor a structurally valid AST proves that naming, typing,
or code generation supports the program.

For parallel parsing, [`DetachedParse`] owns local names until binding into a
session interner. Bind sources in stable input order and retain their source
identity. Keep parser features and effective language-import policy with the
parse result when reproducing dialect-dependent behavior.


For navigation, `ParseResult::member_names` exposes half-open UTF-8 name
tokens for bare expression identifiers, explicit selections and simple value
declarations, including explicit constructor accessors. A bare identifier
or selector on the left of a direct assignment identifies its assignment tree.
[`SourceMemberNames::term_names`] visits both expression forms in token order. Resolve field identity against
the committed typed bindings for the same parse.

Use [`SourceMemberNames::parameter_name`] with a named method parameter or plain constructor
parameter declaration tree to obtain its exact name, including any backticks.
