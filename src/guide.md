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
| Name a bound file and inspect its declarations | [`ParsedSource::name`], [`NamedSource`], [`NamedSource::into_classes`] |
| Prepare primary constructors and attempt class headers by class symbol | [`NamedClassSources::complete_constructor_signature`], [`NamedClassSources::try_complete_class_header_reported`], [`ClassHeaderBindings`] |
| Resolve source symbols and attempt signatures across files | [`NamedClassSources`], [`NamedClassSources::declaration`], [`NamedClassSources::try_complete_member_signature`] |
| Prepare direct-call navigation for an ordinary class file | [`NamedClassSource::navigation`], [`SourceFileCalls::definition_at`] |
| Type or check an ordinary class by symbol | [`NamedClassSources::type_class_body`], [`NamedClassSources::check_class_body`], [`ClassBodyCheck::into_parts`] |
| Complete signatures and type expressions/methods | [`typer::SourceTyper`], [`try_complete_member_signature`], [`check_class_body`] |
| Keep a typed failure and its original byte range for later publication | [`ReportedTyperError`], [`ClassBodyError::into_reported_error`], [`MemberSignatureError::into_reported_error`] |
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

## Follow a signature dependency into another source file

[`NamedClassSources`] borrows [`NamedClassSource`] files from one session and owns one
declaration index. File order comes from the input iterator. IDs can be sparse.
If two files have the same ID, construction returns [`NamedClassSourcesError::DuplicateSource`].
Construction leaves the store available for typing. Each file prepares its call
index on demand. Build a new collection for the files in each analysis session.

This client asks for a field in `B.scala`. The typer identifies the field it
needs from `A.scala`, so the client can complete that declaration and retry.
The same lookup supplies the original AST for a diagnostic in either file.

```rust
use dotty::core::{AstArena, Definitions, Packages, SemanticStore, SourceId, Typed};
use dotty::ClassHeaderBindings;
use dotty::typer::SignatureCompletion;
use dotty::{NamedClassSources, parse_source};

let mut store = SemanticStore::new();
let defs = Definitions::bootstrap(&mut store);
let mut packages = Packages::new();
let mut files = Vec::new();
for (id, name, text) in [
    (42, "B.scala", "class B(a: A) { val answer = a.seed }"),
    (7, "A.scala", "class A { val seed = 40 }"),
] {
    files.push(parse_source(SourceId::from_index(id), text, Default::default())?
        .bind(&mut store.names)
        .name(name, &mut store, &mut packages)?
        .into_classes(&store)?);
}
let sources = NamedClassSources::new(&files, &store)?;
let mut arena = AstArena::<Typed>::new();
let mut bindings = ClassHeaderBindings::default();
for (_, class) in sources.classes() {
    sources.complete_constructor_signature(class.symbol, &mut store, defs, &packages)?;
}
for (_, class) in sources.classes() {
    let outcome = sources.try_complete_class_header_reported(
        class.symbol, &mut store, defs, &packages, &mut arena, &mut bindings,
    )?;
    assert!(matches!(outcome, SignatureCompletion::Complete(_)));
}

let answer = files[0].classes()[0].fields().next().unwrap().symbol;
let mut attempt = |symbol| sources.try_complete_member_signature(symbol, &mut store, defs, &packages);
let SignatureCompletion::BlockedOn(seed) = attempt(answer)? else {
    return Err("expected a dependency on A.seed".into());
};
let (file, declaration) = sources.declaration(seed).ok_or("missing provider")?;
assert_eq!(file.source(), SourceId::from_index(7));
assert_eq!(declaration.class, files[1].classes()[0].symbol);
assert_eq!(attempt(seed)?, SignatureCompletion::Complete(defs.int));
assert_eq!(attempt(answer)?, SignatureCompletion::Complete(defs.int));
# Ok::<(), Box<dyn std::error::Error>>(())
```

Clients choose the order of attempts and which failures to report. Use
[`SignatureDependencies`] to track multiple waiters and cycles. All semantic
IDs, including supplied symbols, must come from the store used to name the
files. Preserve the same typed arena and header bindings for later planning.

## Prepare constructors, retry a header and retain an error

Call [`NamedClassSources::complete_constructor_signature`] for each eligible
class first. A child header can still need its parent's class information.
Here two classes in the same file extend a base declared in a second file.
The client retries a blocked child and retains the other child's rejected
parent argument for publication after the session ends.

```rust
use dotty::{ClassHeaderBindings, ClassHeaderError, NamedClassSources, parse_source};
use dotty::core::{AstArena, Definitions, Packages, SemanticStore, SourceId};
use dotty::typer::{SignatureCompletion, TyperError};

let text = "class Good(x: Int) extends Base(x); class Bad(start: Boolean) extends Base(start)";
let source = SourceId::from_index(7);
let report = {
    let mut store = SemanticStore::new();
    let defs = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let mut files = Vec::new();
    for (id, name, content) in [(source, "Children.scala", text),
                                (SourceId::from_index(42), "Base.scala", "class Base(seed: Int)")] {
        files.push(parse_source(id, content, Default::default())?.bind(&mut store.names)
            .name(name, &mut store, &mut packages)?.into_classes(&store)?);
    }
    let sources = NamedClassSources::new(&files, &store)?;
    for (_, class) in sources.classes() {
        sources.complete_constructor_signature(class.symbol, &mut store, defs, &packages)?;
    }
    let mut arena = AstArena::new();
    let mut bindings = ClassHeaderBindings::default();
    let mut attempt = |class| sources.try_complete_class_header_reported(
        class, &mut store, defs, &packages, &mut arena, &mut bindings,
    );
    let good = files[0].classes()[0].symbol;
    let SignatureCompletion::BlockedOn(parent) = attempt(good)? else {
        return Err("expected a dependency on Base".into());
    };
    assert_eq!(parent, files[1].classes()[0].symbol);
    assert!(matches!(attempt(parent)?, SignatureCompletion::Complete(_)));
    assert!(matches!(attempt(good)?, SignatureCompletion::Complete(_)));
    match attempt(files[0].classes()[1].symbol) {
        Err(ClassHeaderError::Typing(report)) => report,
        other => return Err(format!("expected a parent argument failure, got {other:?}").into()),
    }
};

assert!(matches!(report.error(), TyperError::ApplicationArgumentTypeMismatch { argument_index: 0, .. }));
let location = report.location().ok_or("expected a parent argument location")?;
assert_eq!(location.source(), source);
let range = location.span().range();
let argument = &text[range.start() as usize..range.end() as usize];
assert_eq!(format!("Check the type of {argument}"), "Check the type of start");
assert_eq!(report.render().message, "Expected Int, found Boolean");
# Ok::<(), Box<dyn std::error::Error>>(())
```

An invalid class selection yields [`ClassHeaderError::InvalidClass`]. A
[`ClassHeaderError::Typing`] report identifies the requested class. Constructor
failures also identify its primary constructor. Use the report's location to
find the failing expression when a parent is completed recursively.
A lower-level typer caller can use [`ReportedTyperError::from_class`] directly.

## Report a typer error after analysis

Capture a [`ReportedTyperError`] while the original untyped tree is available.
A client can then match the original [`typer::TyperError`] and use its own
wording or request the default presentation. This example keeps just the
report and the matching source text after checking a constructor parameter.

```rust
use dotty::core::{Definitions, Packages, SemanticStore, SourceId};
use dotty::typer::{SourceTyper, TyperError};
use dotty::{ReportedTyperError, parse_source};

let text = "class Broken(start: Missing)";
let source = SourceId::from_index(7);
let report = {
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let file = parse_source(source, text, Default::default())?
        .bind(&mut store.names)
        .name("Broken.scala", &mut store, &mut packages)?
        .into_classes(&store)?;
    let class = &file.classes()[0];
    let mut typer = SourceTyper::new(
        file.ast(), source, file.index(), &mut store, definitions, &packages,
    );
    let error = typer.complete_symbol(class.constructor).expect_err("unknown parameter type");
    ReportedTyperError::from_class(error, class.symbol, Some(class.constructor), |id| {
        (id == source).then_some(file.ast())
    })
};

assert!(matches!(report.error(), TyperError::TypeNameNotFound { .. }));
assert_eq!(report.code(), Some("typer.type_name_not_found"));
let location = report.location().ok_or("expected a type name location")?;
assert_eq!(location.source(), source);
let range = location.span().range();
let spelling = &text[range.start() as usize..range.end() as usize];
assert_eq!(format!("No type named {spelling}"), "No type named Missing");
assert_eq!(report.render().message, "Unknown type name");
# Ok::<(), Box<dyn std::error::Error>>(())
```

Apply byte ranges only to the original document version. Semantic IDs belong
to their original session and a failed typing attempt may roll back type slots.
If the tree is unavailable, clients can still inspect the error and its code.
[`ClassBodyError::into_reported_error`], [`MemberSignatureError::into_reported_error`]
and [`ClassFieldSignatureError::into_reported_error`] retain the enclosing symbols
for wrapped typer failures and return structural errors to the caller intact.

## Type named methods and retain a call error

Bootstrap primitive definitions once per semantic store. Parse into that store's
interner, name the source, and preserve the returned source semantic index.
The example types a successful call, inspects its warning, then checks a
sibling method and retains its rejected argument for later reporting.

```rust
use dotty::core::{Definitions, Packages, SemanticStore, SourceId, TreeKind};
use dotty::parse_source;
use dotty::parser::ParserFeatures;
use dotty::typer::{SourceTyper, TyperError, TyperWarning};
use dotty::{CallDiagnostic, SourceDiagnostic};

let source = SourceId::from_index(0);
let mut store = SemanticStore::new();
let definitions = Definitions::bootstrap(&mut store);
let mut packages = Packages::new();
let text = "class Answer { def f(implicit n: Int): Int = n; def value: Int = f(42); def bad: Int = f(false) }";
let named = parse_source(source, text, ParserFeatures::default())?
    .bind(&mut store.names)
    .name("Answer.scala", &mut store, &mut packages)?;
let file = named.into_classes(&store)?;
let class = file.classes().first().ok_or("expected a supported source class")?;
let mut methods = class.methods();
let f = methods.next().ok_or("expected f")?;
let value = methods.next().ok_or("expected value")?;
let bad = methods.next().ok_or("expected bad")?;
let mut typer = SourceTyper::new(
    file.ast(), file.source(), file.index(), &mut store, definitions, &packages,
);
typer.complete_symbol(f.symbol)?;
let typed_method = typer.type_method_definition(value.tree)?;
assert!(matches!(typer.typed_ast().get(typed_method).kind, TreeKind::DefDef(_)));
let [warning] = typer.diagnostics() else { return Err("expected one warning".into()) };
assert_eq!(warning.issue(), &TyperWarning::ImplicitClauseWithoutUsing);
let report = SourceDiagnostic::from_typer_warning(source, warning);
assert_eq!(report.code, Some("typer.implicit_clause_without_using"));

let failure = typer.type_method_definition(bad.tree).expect_err("Boolean passed to f");
let call = CallDiagnostic::from_typer(failure, Some(bad.symbol), |id| {
    (id == source).then_some(file.ast())
})?;
assert!(matches!(call.error(), TyperError::ApplicationArgumentTypeMismatch {
    argument_index: 0, ..
}));
assert!(call.identity(&definitions).is_some());
let range = call.location().ok_or("expected an argument position")?.span().range();
assert_eq!(&text[range.start() as usize..range.end() as usize], "false");
let report = call.render();
assert_eq!(report.code, Some("typer.application_argument_type_mismatch"));
assert_eq!(report.message, "Expected Int, found Boolean");
# Ok::<(), Box<dyn std::error::Error>>(())
```

[`ParsedSource::name`] takes the bound parse by value and uses its original source ID.
On a syntax rejection, [`NameSourceError::Syntax`] returns the recovered tree and all
lexer/parser diagnostics. A rejected naming transaction rolls back symbols, scope
insertions and packages. The earlier `bind` operation has already interned names.

A successful [`NamedSource`] owns its syntax and index for this analysis.
[`NamedSource::into_classes`] reads the existing semantic store. If a file contains
a `trait`, the ordinary-class projection returns [`ClassProjectionError`] with the
original named file and the rejected tree. To locate the declaration, call
[`source_declarations::DeclarationError::location`] with that file's AST. For files
in the supported subset, [`NamedClassSource::navigation`] builds the call index
once on demand. Edits produce a new parse and a new analysis with its own IDs.

[`MemberSignatureError::into_call_diagnostic`] and
[`ClassBodyError::into_call_diagnostic`] supply the enclosing member automatically.
Both return the original wrapper on an unsupported error. Keep matching
[`CallErrorIdentity`] keys only within the current analysis of unchanged files.
Discard the keys on reanalysis. If a key is unavailable, keep the report.
An absent AST removes the location and key, while the payload, source and code
remain available.

For supported ordinary class bodies, [`type_class_body`] retains the exact
source-to-typed bindings required by [`jvm_class_plan::JvmClassPlanner`].
[`check_class_body`] additionally presents a source rejection as diagnostics.
Neither helper is a general Scala compiler. An external [`core::SymbolResolver`]
can provide symbols not already in the source session; classpath/JDK discovery
belongs in [`classloader`], not in the source frontend.

## Use class bodies for planning and navigation

After signature completion, look up classes by symbol in [`NamedClassSources`].
Choose [`NamedClassSources::type_class_body`] to stop at the first bad member,
or [`NamedClassSources::check_class_body`] to check every member. Both accept
a borrowed session resolver or [`core::NoResolver`]. Each failed member
rolls back its own typing attempt. Keep the typed arena and store because
successful members have committed their trees and bindings there.

This client plans a valid class and reports a rejected call in another class
from the same source file. Moving the typed error into a report leaves the
successful sibling's definition link available.

```rust
use dotty::{CheckedClassBody, ClassBodyError, NamedClassSources, parse_source};
use dotty::core::{AstArena, Definitions, NoResolver, Packages, SemanticStore, SourceId};
use dotty::ClassHeaderBindings;
use dotty::jvm_class_plan::JvmClassPlanner;
use dotty::typer::{SignatureCompletion, TyperError};

# let mut store = SemanticStore::new();
# let defs = Definitions::bootstrap(&mut store);
# let mut packages = Packages::new();
let text = "class A { def only(x: Int): Int = x }; class B { def bad(a: A): Int = a.only(false); def good(a: A): Int = a.only(7) }";
let id = SourceId::from_index(42);
let file = parse_source(id, text, Default::default())?.bind(&mut store.names)
    .name("Example.scala", &mut store, &mut packages)?.into_classes(&store)?;
let sources = NamedClassSources::new([&file], &store)?;
let mut arena = AstArena::new();
# let mut headers = ClassHeaderBindings::default();
# for (_, class) in sources.classes() {
#     sources.complete_constructor_signature(class.symbol, &mut store, defs, &packages)?;
# }
# for (_, class) in sources.classes() {
#     assert!(matches!(sources.try_complete_class_header_reported(class.symbol, &mut store, defs, &packages, &mut arena, &mut headers)?, SignatureCompletion::Complete(_)));
#     for method in class.methods() {
#         assert!(matches!(sources.try_complete_member_signature(method.symbol, &mut store, defs, &packages)?, SignatureCompletion::Complete(_)));
#     }
# }
let a = file.classes()[0].symbol;
let b = file.classes()[1].symbol;
let complete = sources.type_class_body(a, &mut store, defs, &packages, &mut arena, NoResolver)?;
let (owner, class) = sources.class(a).ok_or("missing class")?;
JvmClassPlanner::new(&store, owner.ast(), owner.index())
    .with_header_bindings(&arena, &headers).plan_typed_class(class, &arena, &complete)?;

let (body, report) = sources.check_class_body(b, &mut store, defs, &packages, &mut arena, NoResolver)
    .into_parts();
assert!(matches!(body, CheckedClassBody::Partial(_)));
let mut errors = report.errors.into_iter();
assert!(matches!(errors.next(), Some(ClassBodyError::Member {
    error: TyperError::ApplicationArgumentTypeMismatch { .. }, ..
})));
assert!(errors.next().is_none());
let offset = text.find("only(7)").ok_or("missing caller")? as u32;
let target = file.navigation()?.definition_at_with_bindings(offset, file.ast(), &arena, &store,
    |symbol| (symbol == b).then(|| body.bindings()).flatten(),
    |source| sources.file(source)?.navigation().ok(),
).ok_or("missing definition")?;
assert_eq!(target.name.start() as usize, text.find("only(x").unwrap());
# Ok::<(), Box<dyn std::error::Error>>(())
```

A complete check contains [`CheckedClassBody::Complete`] with its planning
body. [`CheckedClassBody::Partial`] holds only committed source-to-typed
mappings, and [`CheckedClassBody::Invalid`] represents a class rejected
before typing. [`ClassBodyReport`] owns member errors and committed warnings.
Rebuild the semantic data when a new edit starts a new session.

## Follow signature dependencies between source files

Use [`SignatureDependencies`] with the outcomes of
[`NamedClassSources::try_complete_class_header`] or
[`NamedClassSources::try_complete_member_signature`]. Keep one tracker per phase for one set of
named files. Record attempts in observation order. The tracker borrows no
arenas, owns no worker threads, and leaves the choice of the next file to the
client. Drop it on edits or renaming, since a fresh session can reuse symbol IDs.

This example tries `Leaf` before `Base`, then inspects a separate two-file cycle.

```rust
use dotty::{NamedClassSources, SignatureBlockage, SignatureDependencies, SignaturePhase};
use dotty::core::{AstArena, Definitions, Packages, SemanticStore, SourceId};
use dotty::ClassHeaderBindings;
use dotty::typer::SignatureCompletion;

let files = [
    ("Leaf.scala", "class Leaf(x: Int) extends Base(x)"),
    ("Base.scala", "class Base(x: Int)"),
    ("A.scala", "class A(x: Int) extends B(x)"),
    ("B.scala", "class B(x: Int) extends A(x)"),
];
# let mut store = SemanticStore::new();
# let defs = Definitions::bootstrap(&mut store);
# let mut packages = Packages::new();
# let mut units = Vec::new();
# for (i, (filename, text)) in files.into_iter().enumerate() {
#     let source = SourceId::from_index(i as u32);
#     let parsed = dotty::parse_source(source, text, Default::default())?.bind(&mut store.names);
#     let file = parsed.name(filename, &mut store, &mut packages)?.into_classes(&store)?;
#     units.push(file);
# }
let sources = NamedClassSources::new(&units, &store)?;
# for (_, class) in sources.classes() {
#     sources.complete_constructor_signature(class.symbol, &mut store, defs, &packages)?;
# }
# let mut arena = AstArena::new();
# let mut bindings = ClassHeaderBindings::default();
let mut attempt = |symbol| {
    sources.try_complete_class_header_reported(symbol, &mut store, defs, &packages, &mut arena, &mut bindings)
};
let mut headers = SignatureDependencies::new(SignaturePhase::Headers);
let [leaf, base, a, b] = [0, 1, 2, 3].map(|i| units[i].classes()[0].symbol);

let outcome = attempt(leaf)?;
assert_eq!(outcome, SignatureCompletion::BlockedOn(base));
assert!(headers.record(leaf, outcome).is_empty());
assert_eq!(headers.record(base, attempt(base)?), [leaf]);
let retry = attempt(leaf)?;
assert!(matches!(retry, SignatureCompletion::Complete(_)));
headers.record(leaf, retry);

headers.record(a, attempt(a)?);
headers.record(b, attempt(b)?);
let blocked = headers.blockages([a, b]);
let SignatureBlockage::CycleMember(cycle) = &blocked[0].1 else {
    return Err("expected a header cycle".into());
};
assert_eq!(cycle.path(), [a, b, a]);
assert_eq!(cycle.code(), "signature.header_cycle");
# Ok::<(), Box<dyn std::error::Error>>(())
```

When an attempt returns an error, keep the original phase error under the
attempted symbol and call [`SignatureDependencies::record_failure`]. A waiter
can then identify that symbol through [`SignatureBlockage::Failed`]. At the end
of a pass, query all waiting roots together to share traversal of common
paths. Only [`SignatureBlockage::CycleMember`] identifies a declaration taking
part in a cycle. [`SignatureCycle::declarations`] provides the optional source
position of each participant for reporting.

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
