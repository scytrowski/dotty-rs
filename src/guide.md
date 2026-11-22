# Dotty compiler libraries

The `dotty-rs` Cargo package exposes the Rust library `dotty`: experimental
Scala **3.9.0** compiler infrastructure. It is a collection of components and
bounded integration helpers, with evolving pre-1.0 APIs. It does not provide a
general source compiler, compiler session/driver, or source-compilation CLI.

## Choose an entry point

| Task | Public API |
| --- | --- |
| Parse a source and collect typed lexer/parser diagnostics | [`parse_source`], [`DetachedSource::bind`], [`ParsedSource`], [`SyntaxDiagnostic`] |
| Control scanning and parsing separately | [`lexer::ContextualScanner`], [`parser::parse_compilation_unit`], [`parser::DetachedParse`] |
| Enter source declarations | [`namer::name_compilation_unit_with_diagnostics`], [`namer::NamingOutcome`] |
| Complete signatures and type expressions/methods | [`typer::SourceTyper`], [`try_complete_member_signature`], [`check_class_body`] |
| Plan source methods and classes for the JVM | [`jvm_method_plan`], [`jvm_class_plan`], [`jvm`] |
| Read or write structural TASTy | [`tasty::TastyFile`], [`tasty::TastyFileBuilder`] |
| Project decoded TASTy into shared semantics | [`tasty_unpickler`] |
| Read/write class files or load classpaths | [`classfile`], [`classloader`] |
| Validate/emit the audited paired JVM/TASTy subset | [`jvm_tasty_plan`], [`jvm_tasty_artifacts`] |

Semantic TASTy pickling is available directly from the separate
[`dotty_tasty_pickler`] crate. Structural validation, semantic completion, JVM
emission, and Scala interoperability are distinct guarantees. A successful
parse does not imply that naming, typing, or either producer accepts the source.

## Parse and bind one source

Parsing can run with worker-local names. Bind each detached result into the
session's interner in a stable source order. The resulting trees and diagnostics
use that interner's names; source positions remain UTF-8 byte ranges.

```rust
use dotty::core::{NameInterner, SourceId};
use dotty::parse_source;
use dotty::parser::ParserFeatures;

let mut names = NameInterner::new();
let detached = parse_source(
    SourceId::from_index(0),
    "class Answer { def value: Int = 42 }",
    ParserFeatures::default(),
)?;
let parsed = detached.bind(&mut names);
assert!(parsed.diagnostics.is_empty());
// The original AST and package root are in parsed.syntax.
# Ok::<(), Box<dyn std::error::Error>>(())
```

`Err` from [`parse_source`] means the source could not be represented or scanned.
Recoverable syntax failures still return trees. Inspect [`ParsedSource::diagnostics`]
before naming; it contains **both** lexer and parser reports, sorted by starting
byte offset. Equal offsets keep lexer reports first. Checking only
`parsed.syntax.diagnostics` misses lexer failures:

```rust
use dotty::core::{NameInterner, SourceId};
use dotty::lexer::LexerIssue;
use dotty::{SyntaxDiagnostic, parse_source};
use dotty::parser::ParserFeatures;

let mut names = NameInterner::new();
let parsed = parse_source(
    SourceId::from_index(1),
    "class Broken {} /* unclosed comment",
    ParserFeatures::default(),
)?
.bind(&mut names);
assert!(parsed.syntax.diagnostics.is_empty());
assert_eq!(parsed.diagnostics.len(), 1);
let diagnostic = &parsed.diagnostics[0];
assert!(matches!(
    diagnostic,
    SyntaxDiagnostic::Lexer { diagnostic, .. }
        if matches!(diagnostic.issue(), LexerIssue::UnterminatedBlockComment)
));
assert_eq!(diagnostic.location().source(), SourceId::from_index(1));
let report = diagnostic.render();
assert_eq!(report.code, Some("lexer.unterminated_block_comment"));
# Ok::<(), Box<dyn std::error::Error>>(())
```

[`DetachedSource::bind`] returns `ParsedSource<SyntaxDiagnostic>` without
formatting messages. Match the original lexer/parser payload for machine
decisions and call [`SyntaxDiagnostic::render`] at the presentation boundary.
For clients that only need formatted reports, [`DetachedSource::into_session`]
binds and eagerly renders them as `ParsedSource<SourceDiagnostic>`.

[`SourceDiagnostic`] carries an optional stable phase code, severity, owned
message, and optional location. Codes can be absent for typer/adapter failures;
message wording is for presentation. Keep the original source text when
converting byte ranges to line/column coordinates for an editor.

## Name declarations and type a method

Bootstrap primitive definitions once per semantic store. Parse into that store's
interner, name the source, and preserve the returned source semantic index.
This example types a single concrete method, including its signature and body.
It does not compile the whole class or emit binary artifacts.

```rust
use dotty::core::{Definitions, Packages, SemanticStore, SourceId, TreeKind};
use dotty::namer::{NamingOutcome, SourceParseStatus, name_compilation_unit_with_diagnostics};
use dotty::parse_source;
use dotty::parser::ParserFeatures;
use dotty::source_declarations::source_classes;
use dotty::typer::SourceTyper;

let source = SourceId::from_index(0);
let mut store = SemanticStore::new();
let definitions = Definitions::bootstrap(&mut store);
let mut packages = Packages::new();
let parsed = parse_source(
    source,
    "class Answer { def value: Int = 42 }",
    ParserFeatures::default(),
)?
.bind(&mut store.names);
if !parsed.diagnostics.is_empty() {
    return Err("handle syntax diagnostics before naming".into());
}
let syntax = parsed.syntax;
let index = match name_compilation_unit_with_diagnostics(
    &syntax.ast,
    syntax.root,
    source,
    "Answer.scala",
    SourceParseStatus::Clean,
    &mut store,
    &mut packages,
)? {
    NamingOutcome::Named { index, .. } => index,
    NamingOutcome::Rejected { diagnostics } => {
        return Err(format!("naming rejected the source: {diagnostics:?}").into());
    }
};
let classes = source_classes(&syntax.ast, syntax.root, source, &index, &store)?;
let class = classes.first().ok_or("expected a supported source class")?;
let method = class.methods().next().ok_or("expected a concrete method")?;
let mut typer = SourceTyper::new(
    &syntax.ast, source, &index, &mut store, definitions, &packages,
);
let typed_method = typer.type_method_definition(method.tree)?;
assert!(matches!(typer.typed_ast().get(typed_method).kind, TreeKind::DefDef(_)));
# Ok::<(), Box<dyn std::error::Error>>(())
```

For supported ordinary class bodies, [`type_class_body`] retains the exact
source-to-typed bindings required by [`jvm_class_plan::JvmClassPlanner`].
[`check_class_body`] additionally presents a source rejection as diagnostics.
Neither helper is a general Scala compiler. An external [`core::SymbolResolver`]
can provide symbols not already in the source session; classpath/JDK discovery
belongs in [`classloader`], not in the source frontend.

## Preserve one-session identity and failure contracts

- Keep all names, symbols, types, scopes, source indexes and typed roots in the
  same canonical session. [`core::SemanticStore`] owns semantic arenas; syntax
  and typed arenas and [`core::Packages`] are supplied separately. Allocate a
  distinct [`core::SourceId`] for each source in the session.
- Arena IDs identify slots, not originating stores. A same-slot ID from another
  store can appear valid. Do not reconstruct identity from spelling, allocation
  order, or independently rebuilt stores.
- Preserve declaration/parameter order and source-to-typed bindings when building
  backend or pickler plans. Retain the exact typed arena alongside
  [`jvm_class_plan::TypedClassBodies`].
- Naming rolls back its changes on failure. Individual typer transactions roll
  back failed attempts. A class-body pass can retain earlier successful members
  when a later member fails; inspect [`type_class_body`]'s contract before retrying.
  IDs allocated after a rolled-back checkpoint must not be reused as live handles.
- Errors from unsupported or ambiguous input are part of the API contract. Do
  not invent missing semantic information to bypass a rejection. Checked arena
  access and validation are appropriate at input boundaries; unchecked getters
  rely on callers preserving valid IDs.

## Validate and round-trip structural TASTy

[`tasty::TastyFile`] borrows its input bytes; builders and encoded outputs own
their data. The following minimal **structural container** has no declarations
or typed body. It demonstrates binary validation and a byte-for-byte raw round
trip, not a semantic pickler output or a Scala-consumable compilation unit.

```rust
use dotty::tasty::{EncodedSection, Header, NameTableBuilder, RawName, TastyFile, TastyFileBuilder};

let header = Header {
    major_version: 28,
    minor_version: 9,
    experimental_version: 0,
    tooling_version: "dotty-rs API example".into(),
    uuid: [0; 16],
};
let mut names = NameTableBuilder::new();
let asts = names.intern(RawName::Utf8("ASTs".into()))?;
let builder = TastyFileBuilder::new(header, names.finish()?)
    .with_section(EncodedSection::raw(asts, Vec::new()));
let bytes = builder.encode_validated()?;
let file = TastyFile::parse_and_validate_scala_3_9(&bytes)?;
assert_eq!(file.header().minor_version, 9);
assert_eq!(file.encode()?, bytes);
assert!(TastyFile::parse_and_validate_scala_3_9(b"malformed").is_err());
# Ok::<(), Box<dyn std::error::Error>>(())
```

The Scala-specific reader and semantic pickler target TASTy **28.9.0**. Older
fixture versions require the explicit compatible-version APIs. Structural
relocation and semantic re-emission have different guarantees from the raw
round trip above. Use the codec's explicit limits when processing untrusted
input with a tighter resource budget.

## JVM output and paired artifacts

[`jvm`] accepts already-typed borrowed plans; it does not assemble the frontend.
The current backend emits class-file major **65** (Java 21). This is independent
of the broader structural class-file codec's supported versions.

[`jvm_tasty_plan`] and [`jvm_tasty_artifacts`] coordinate specific audited source
shapes in one session. Some planning APIs use callbacks so borrowed plans cannot
outlive their stores. Owned artifact bytes can survive the callback; filesystem
publication is the caller's responsibility. These bounded pairs/batches do not
establish general Scala ABI compatibility or a combined compiler API.

The parsing, typing, and TASTy examples above need no JDK or Scala installation.
External JVM verification and pinned Scala-consumer oracle checks are separate
validation steps for the supported producer shapes.
