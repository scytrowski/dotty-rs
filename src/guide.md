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
| Prepare one bound file and retain typed diagnostics | [`ParsedSource::prepare_classes`], [`ClassPreparation`], [`PreparationDiagnostic`] |
| Name a bound file and inspect its declarations | [`ParsedSource::name`], [`NamedSource::into_classes`], [`NamedSource::project_classes`] |
| Track constructor failures, header attempts and ready classes across files | [`ClassHeaderPhase`], [`ClassHeaderReport`] |
| Prepare primary constructors and attempt class headers by class symbol | [`NamedClassSources::complete_constructor_signature`], [`NamedClassSources::try_complete_class_header_reported`], [`ClassHeaderBindings`] |
| Select ready classes and track member signatures, retries and failures | [`NamedClassSources::member_signatures`], [`MemberSignaturePhase`], [`MemberSignatureReport`] |
| Resolve source symbols and attempt signatures across files | [`NamedClassSources`], [`NamedClassSources::declaration`], [`NamedClassSources::try_complete_member_signature`] |
| Retain checked bodies for source navigation and JVM planning | [`CheckedClassBodies::record`], [`NamedClassSources::definition_at`], [`CheckedClassBodies::complete`] |
| Resolve a checked call or field access and find its uses | [`NamedClassSources::resolved_call_at`], [`NamedClassSources::resolved_field_at`], [`ResolvedMemberIndex`] |
| Prepare direct-call navigation for an ordinary class file | [`NamedClassSource::navigation`], [`SourceFileCalls::definition_at`] |
| Check bodies of signature-ready members | [`MemberSignaturePhase::finish_with_body_work`], [`ClassBodyWork::members`], [`ClassBodyWork::check`] |
| Type or check an ordinary class by symbol | [`NamedClassSources::type_class_body`], [`NamedClassSources::check_class_body`], [`ClassBodyCheck::into_parts`] |
| Complete signatures and type expressions/methods | [`typer::SourceTyper`], [`try_complete_member_signature`], [`check_class_body`] |
| Collect direct diagnostics from ordinary class phases | [`SemanticDiagnostic`], [`ClassHeaderFailure::into_semantic_diagnostic`], [`MemberSignatureFailure::into_semantic_diagnostic`], [`ClassBodyReport::into_semantic_diagnostics`] |
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

## Prepare ordinary classes for a compiler or editor

Call [`ParsedSource::prepare_classes`] after binding each detached parse to the
session interner. The caller chooses how to schedule parsing and in which order
to bind the results. A [`ClassPreparation::Prepared`] outcome may carry
diagnostics. Even a file consisting entirely of omitted traits has a prepared
view with zero classes. A [`ClassPreparation::Rejected`] carries its file ID and
diagnostics. Scanning failures returned by [`parse_source`] precede this step.

This client keeps the valid classes and gives an unsupported declaration its
own message. It can still identify the parser rejection from a different file.

```rust
use dotty::{NamedClassSources, PreparationDiagnostic, SyntaxDiagnostic, parse_source};
use dotty::core::{Packages, SemanticStore, SourceId};
use dotty::source_declarations::DeclarationError;

let mut store = SemanticStore::new();
let mut packages = Packages::new();
let mut files = Vec::new();
let mut messages = Vec::new();
for (index, filename, text) in [
    (7, "Model.scala", "class A; trait T; class B"),
    (3, "Broken.scala", "class Broken { val n: Int 1 }"),
] {
    let source = SourceId::from_index(index);
    let parsed = parse_source(source, text, Default::default())?.bind(&mut store.names);
    let (file, reports) = parsed.prepare_classes(filename, &mut store, &mut packages).into_parts();
    files.extend(file);
    for report in reports {
        let message = match &report {
            PreparationDiagnostic::Declaration(d)
                if matches!(d.error, DeclarationError::Unsupported { .. }) => {
                    let range = report.location().ok_or("expected a declaration position")?.span().range();
                    format!("Skipped {}", &text[range.start() as usize..range.end() as usize])
                }
            PreparationDiagnostic::Syntax(SyntaxDiagnostic::Parser(_)) => {
                assert_eq!(report.code(), Some("parser.declaration.expected_value_equals"));
                report.render().message
            }
            _ => return Err("unexpected preparation diagnostic".into()),
        };
        messages.push((report.source(), message));
    }
}
assert_eq!(messages[0], (SourceId::from_index(7), "Skipped trait T".to_owned()));
assert_eq!(messages[1].0, SourceId::from_index(3));
let sources = NamedClassSources::new(&files, &store)?;
assert_eq!(sources.files().count(), 1);
assert_eq!(sources.classes().count(), 2);
# Ok::<(), Box<dyn std::error::Error>>(())
```

Preparation reports retain their payloads, codes and original byte ranges after
the session ends. Resolve IDs only in their originating session and apply a range
to the corresponding document version. For a recovered tree on syntax rejection,
call [`ParsedSource::name`] and match [`NameSourceError::Syntax`]. To require
complete coverage after successful naming, use [`NamedSource::into_classes`].

## Select ordinary classes from a mixed source file

After naming, choose [`NamedSource::into_classes`] when every top-level declaration
must be supported. Choose [`NamedSource::project_classes`] to inspect the supported
classes alongside typed reasons for omitting their siblings. For example, a
source client can keep navigating to methods in classes on either side of a trait.

```rust
use dotty::{NamedClassSources, parse_source};
use dotty::core::{Packages, SemanticStore, SourceId};
use dotty::source_declarations::DeclarationError;

let text = "class A; trait T; class B { def value(): Int = 7 }";
let mut store = SemanticStore::new();
let source = SourceId::from_index(7);
let named = parse_source(source, text, Default::default())?.bind(&mut store.names)
    .name("Model.scala", &mut store, &mut Packages::new())?;
// A caller that initially requires full coverage still owns the named file on failure.
let strict = named.into_classes(&store).expect_err("trait outside the ordinary-class subset");
let projection = strict.named.project_classes(&store)?;
let [report] = projection.rejections.as_slice() else { return Err("expected one omission".into()) };
assert_eq!(report.error, strict.error);
assert!(matches!(report.error, DeclarationError::Unsupported { .. }));
let location = report.location.ok_or("expected a declaration position")?;
assert_eq!(location.source(), source);
let range = location.span().range();
assert_eq!(&text[range.start() as usize..range.end() as usize], "trait T");

let files = NamedClassSources::new([&projection.file], &store)?;
let [_, b] = projection.file.classes() else { return Err("expected A and B".into()) };
let value = b.methods().next().ok_or("expected B.value")?;
let (_, declaration) = files.declaration(value.symbol).ok_or("missing projected method")?;
assert_eq!(declaration.class, b.symbol);
# Ok::<(), Box<dyn std::error::Error>>(())
```

Call `project_classes` directly when partial coverage is intended. It records
the first unsupported tree in each omitted top-level statement. An unsupported
class member excludes its enclosing class. An encountered inconsistent binding
rejects the projection and returns the original named file. A file containing
only unsupported declarations produces an empty class view with rejections.

The original naming index and semantic store still contain symbols for omitted
declarations. [`NamedClassSources::declaration`] indexes only the selected classes
and their declarations. Clients decide how to report omissions and whether
independent classes should proceed to typing or emission. Reports retain byte
ranges for the original document version. Use their tree IDs with that parse.

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

## Check members of ready classes after header completion

[`ClassHeaderPhase::prepare`] checks the constructors of the selected sources.
An editor can call [`ClassHeaderPhase::complete_all`] and
[`MemberSignaturePhase::complete_all`] to process independent declarations and
retain the original errors and cycle reports. A compiler can instead use
[`ClassHeaderPhase::try_prepare`] and pass its ordered roots to
[`ClassHeaderPhase::try_complete_roots`] or [`MemberSignaturePhase::try_complete_roots`].
These operations follow dependencies immediately and return a [`SignatureRunError`]
on the first rejection or cycle. The single-attempt API also supports custom scheduling.

```rust
use dotty::core::{AstArena, Definitions, Packages, SemanticStore, SourceId};
use dotty::typer::TyperError;
use dotty::{MemberSignatureFailure, MemberSignaturePhase};
use dotty::{ClassHeaderBindings, ClassHeaderError, ClassHeaderPhase, NamedClassSources, parse_source};

let mut store = SemanticStore::new();
let defs = Definitions::bootstrap(&mut store);
let mut packages = Packages::new();
let mut files = Vec::new();
for (i, name, text) in [
    (8, "Children.scala", "class Good(x: Int) extends Base(x) { def value() = 7; val broken = value(1); def f(implicit i: Int): Int = i; def call(): Int = f(1) }; class Bad(y: Missing)"),
    (2, "Base.scala", "class Base(seed: Int)"),
] {
    let parsed = parse_source(SourceId::from_index(i), text, Default::default())?
        .bind(&mut store.names);
    let (file, diagnostics) = parsed.prepare_classes(name, &mut store, &mut packages).into_parts();
    assert!(diagnostics.is_empty());
    files.push(file.expect("named ordinary classes"));
}

let sources = NamedClassSources::new(&files, &store)?;
let mut arena = AstArena::new();
let mut bindings = ClassHeaderBindings::default();
let mut phase = ClassHeaderPhase::prepare(&sources, &mut store, defs, &packages, &mut arena, &mut bindings);
phase.complete_all();
let report = phase.finish();
let good = &files[0].classes()[0];
assert_eq!(report.ready, [good.symbol, files[1].classes()[0].symbol]);
assert!(report.blockages.is_empty() && report.remaining.is_empty());
let [failure] = report.failures.as_slice() else { panic!("expected the Bad constructor") };
assert_eq!(failure.source, Some(files[0].source()));
assert!(matches!(&failure.error, ClassHeaderError::Typing(e)
    if matches!(e.error(), TyperError::TypeNameNotFound { .. })));

let selection = sources.member_signatures(report.ready, &store)?;
let mut signatures = MemberSignaturePhase::new(&selection, &mut store, defs, &packages);
signatures.complete_all(dotty::core::NoResolver);
let (signatures, bodies) = signatures.finish_with_body_work();
let value = good.methods().next().unwrap();
assert!(signatures.ready.contains(&value.symbol));
let [MemberSignatureFailure::Typing { declaration, report }] = signatures.failures.as_slice()
    else { panic!("expected one rejected field signature") };
assert_eq!(declaration.class, good.symbol);
assert!(matches!(report.error(), TyperError::ApplicationArityMismatch { expected: 0, actual: 1, .. }));
assert_eq!(report.location().unwrap().source(), files[0].source());

let broken = good.fields().next().unwrap();
assert!(bodies.members(good.symbol).unwrap().any(|(id, reason)|
    id == broken.symbol && reason == Some(&dotty::BodySkipReason::Rejected)));
let (body, body_report) = bodies.check(good.symbol, &mut store, defs, &packages,
    &mut arena, dotty::core::NoResolver);
let dotty::CheckedClassBody::Partial(partial) = body else { panic!("skipped field") };
assert!(body_report.errors.is_empty());
assert!(partial.bindings().get(files[0].source(), value.tree).is_some());
assert!(partial.bindings().get(files[0].source(), broken.tree).is_none());
let [(source, warning)] = body_report.diagnostics.as_slice() else { panic!("one committed warning") };
assert_eq!(*source, files[0].source());
assert_eq!(warning.issue(), &dotty::typer::TyperWarning::ImplicitClauseWithoutUsing);
assert!(bindings.get(files[0].source(), good.parents[0]).is_some());

let base = &files[1].classes()[0];
let (body, report) = bodies.check(base.symbol, &mut store, defs, &packages,
    &mut arena, dotty::core::NoResolver);
assert!(report.errors.is_empty());
let dotty::CheckedClassBody::Complete(body) = body else { panic!("complete Base") };
let plan = dotty::jvm_class_plan::JvmClassPlanner::new(&store, files[1].ast(), files[1].index())
    .with_header_bindings(&arena, &bindings).plan_typed_class(base, &arena, &body)?;
assert_eq!(plan.class, base.symbol);
# Ok::<(), Box<dyn std::error::Error>>(())
```

The selection includes constructor fields and all declared body fields and methods.
[`MemberSignatureSelection::field_roots`] gives a compiler its ordered field pass
before its caller-chosen method roots. Before emitting a class, complete the
remaining signatures as well, including private fields. An editor may finish
earlier and use the retained bindings from ready members for navigation. Cross-file retries target missing eligible
providers within the selection. An unavailable field outside it retains the
original typer error. An editor can keep independent members of a class available
when another signature fails. Inspect `blockages` for the provider of a failed
chain or its cycle and `remaining` for work that still needs an attempt.

Keep the same typed arena and bindings for subsequent body typing and JVM
planning. A header can recursively complete another class in its file. The phase
includes these successes and wakes observed waiters. Blockages describe actual
dependencies reported by the typer. When same-file recursive typing returns
the original error directly for two requested classes, both attempts retain it.
Clients choose how to publish these errors. Start a fresh session with new IDs,
bindings and phase state after an edit. A retained error range refers to the
source version that produced it.

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

### Supply compiled dependencies during signature completion

After naming all source files, create a [`classloader::ClasspathSymbolResolver`]
using the same definitions and a snapshot of the source package registry.
A client can borrow an already-open [`classloader::CompositeClassPath`] across
revisions. Each new semantic store needs a fresh loading session and resolver.
A successful binary lookup can then share canonical symbols between constructor
parameters, method signatures and bodies in that revision.

The example uses an empty classpath to inspect the typed failure without a
JDK. A Java client classpath can contain a [`classloader::DirectoryClassPath`]
for compiled dependencies and a [`classloader::JmodClassPath`] for
`java.base.jmod`, which supplies `java/lang/Object` and its dependencies.

```rust
use dotty::{ClassHeaderError, ClassHeaderPhase, NamedClassSources, parse_source};
use dotty::classloader::{ClasspathSymbolResolver, CompositeClassPath, LoadingSession};
use dotty::core::{Definitions, Packages, SemanticStore, SourceId};
use dotty::typer::TyperError;

let text = "package demo\nclass Holder(x: External)";
# let source = SourceId::from_index(0);
# let mut store = SemanticStore::new();
# let defs = Definitions::bootstrap(&mut store);
# let mut packages = Packages::new();
# let file = parse_source(source, text, Default::default())?.bind(&mut store.names)
#     .name("Holder.scala", &mut store, &mut packages)?.into_classes(&store)?;
let sources = NamedClassSources::new([&file], &store)?;
let path = CompositeClassPath::new(vec![]);
let mut resolver = ClasspathSymbolResolver::new(
    &path, defs, LoadingSession::with_packages(packages.snapshot()),
);
let mut arena = Default::default();
let mut bindings = Default::default();
let failure = ClassHeaderPhase::try_prepare_with_constructor_resolver(
    &sources, &mut store, defs, &packages, &mut arena, &mut bindings, &mut resolver,
).expect_err("demo.External is missing");
let ClassHeaderError::Typing(report) = failure.error else { panic!("expected a type error") };
assert!(matches!(report.error(), TyperError::TypeNameNotFound { .. }));
let range = report.location().ok_or("missing type location")?.span().range();
assert_eq!(&text[range.start() as usize..range.end() as usize], "External");
# Ok::<(), Box<dyn std::error::Error>>(())
```

[`ClassHeaderPhase::prepare_with_constructor_resolver`] keeps constructor
failures while processing independent siblings. Both preparation policies borrow
the resolver for constructor signatures. When checking selected members, pass it
again to [`MemberSignaturePhase::attempt_with_resolver`] and
[`ClassBodyWork::check`]. [`ResolvedMemberIndex`] reports source targets in
the selected files. A class loaded from bytecode has no defining source range
there.

## Report a semantic diagnostic after analysis

The compiler can stop at the first rejected constructor and inspect the same
[`SemanticDiagnostic`] an editor obtains from a collected header failure.
This example keeps the report and its source text after dropping the named
files, semantic store and arenas.

```rust
use dotty::core::{Definitions, Packages, SemanticStore, SourceId};
use dotty::typer::TyperError;
use dotty::{ClassHeaderPhase, NamedClassSources, SemanticIssue, SemanticPhase, parse_source};

let text = "class Broken(start: Missing)";
let source = SourceId::from_index(7);
let report = {
    let mut store = SemanticStore::new();
    let defs = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let file = parse_source(source, text, Default::default())?.bind(&mut store.names)
        .name("Broken.scala", &mut store, &mut packages)?.into_classes(&store)?;
    let sources = NamedClassSources::new([&file], &store)?;
    let mut arena = Default::default();
    let mut headers = Default::default();
    let failure = ClassHeaderPhase::try_prepare(
        &sources, &mut store, defs, &packages, &mut arena, &mut headers,
    ).expect_err("unknown constructor parameter type");
    failure.into_semantic_diagnostic()
};

assert_eq!(report.phase, SemanticPhase::Constructor);
assert_eq!(report.owner_source, Some(source));
assert!(report.class.is_some() && report.member().is_some());
let location = report.location().ok_or("missing type name range")?;
assert_eq!(location.source(), source);
let range = location.span().range();
let spelling = &text[range.start() as usize..range.end() as usize];
let message = match &report.issue {
    SemanticIssue::Typing(e) if matches!(e.error(), TyperError::TypeNameNotFound { .. }) =>
        format!("No type named {spelling}"),
    other => return Err(format!("unexpected constructor diagnostic: {other:?}").into()),
};
assert_eq!(message, "No type named Missing");
# Ok::<(), Box<dyn std::error::Error>>(())
```

[`SemanticDiagnostic::source`] gives the file to route the diagnostic to. The
captured site can differ from the declaration's file; an unavailable range
still permits logging the typed issue. Symbols belong to the original session,
and failed attempts may roll back type slots. Apply byte ranges only to the
document version checked and convert them to editor positions at publication.

For signatures use [`MemberSignatureError::into_semantic_diagnostic`] on a
direct rejection or [`MemberSignatureFailure::into_semantic_diagnostic`] on
a collected failure. For bodies, [`ClassBodyReport::into_semantic_diagnostics`]
yields committed warnings followed by direct errors. Each warning retains its
original [`typer::TyperDiagnostic`]. Consuming the report leaves the separately
returned [`CheckedClassBody`] available for navigation or planning. Dependencies
and cycles remain available in the signature phase reports.

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
a `trait`, that strict projection returns [`ClassProjectionError`] with the
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
use dotty::{CheckedBodyError, CheckedClassBodies, ClassBodyError, NamedClassSources, parse_source};
use dotty::core::{AstArena, Definitions, NoResolver, Packages, SemanticStore, SourceId};
use dotty::ClassHeaderBindings;
use dotty::jvm_class_plan::JvmClassPlanner;
use dotty::typer::{SignatureCompletion, TyperError};

# let mut store = SemanticStore::new();
# let defs = Definitions::bootstrap(&mut store);
# let mut packages = Packages::new();
let text = "class A { def only(x: Int): Int = x }; class B { var count: Int = 0; def bad(a: A): Int = a.only(false); def good(a: A): Int = a.only(7); def copy(b: B): Unit = b.count = b.count; private var current: Int = 0; def set(current: Int): Unit = this.current = current; def value(): Int = current }";
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
#     for method in class.methods().chain(class.fields()) {
#         assert!(matches!(sources.try_complete_member_signature(method.symbol, &mut store, defs, &packages)?, SignatureCompletion::Complete(_)));
#     }
# }
let a = file.classes()[0].symbol;
let b = file.classes()[1].symbol;
let mut bodies = CheckedClassBodies::default();
for symbol in [b, a] {
    let (body, report) = sources.check_class_body(symbol, &mut store, defs, &packages, &mut arena, NoResolver)
        .into_parts();
    bodies.record(body)?;
    if symbol == a {
        assert!(report.errors.is_empty());
        continue;
    }
    let mut errors = report.errors.into_iter();
    assert!(matches!(errors.next(), Some(ClassBodyError::Member {
        error: TyperError::ApplicationArgumentTypeMismatch { .. }, ..
    })));
    assert!(errors.next().is_none());
}
assert_eq!(bodies.complete(b).unwrap_err(), CheckedBodyError::Partial(b));
let (owner, class) = sources.class(a).ok_or("missing class")?;
let plan = JvmClassPlanner::new(&store, owner.ast(), owner.index())
    .with_header_bindings(&arena, &headers).plan_typed_class(class, &arena, bodies.complete(a)?)?;
assert_eq!(plan.class, a);

let offset = text.find("only(7)").ok_or("missing caller")? as u32;
let call = sources.resolved_call_at(id, offset, &bodies, &arena, &store)?
    .ok_or("missing call")?;
assert_eq!(call.target.name.start() as usize, text.find("only(x").unwrap());

let write = text.find("b.count").unwrap() as u32 + 2;
let field = sources.resolved_field_at(id, write, &bodies, &arena, &store)?.unwrap();
assert_eq!(field.kind, dotty::FieldAccessKind::Write);
let index = dotty::ResolvedMemberIndex::build(&sources, &bodies, &arena, &store);
assert!(index.diagnostics().is_empty());
drop((bodies, arena, store));
let (count, _) = index.member_at(id, field.target.name.start()).unwrap();
let sites = index.references_to(count);
assert_eq!(sites.len(), 2);
assert_eq!(sites[0], field.origin);
let read = index.field_at(id, sites[1].span().range().start()).unwrap();
assert_eq!((read.field, read.kind), (count, dotty::FieldAccessKind::Read));
let private_name = text.find("var current").unwrap() as u32 + 4;
let (current, _) = index.member_at(id, private_name).unwrap();
let write = text.find("this.current").unwrap() as u32 + 5;
let read = text.rfind("current").unwrap() as u32;
let sites: Vec<_> = index.references_to(current).iter().map(|s| s.span().range().start()).collect();
assert_eq!(sites, [write, read]);
let parameter_use = text.find("= current;").unwrap() as u32 + 2;
assert!(index.member_at(id, parameter_use).is_none());
let (method, _) = index.method_at(id, call.target.name.start()).ok_or("missing declaration")?;
assert_eq!(method, call.method);
assert_eq!(index.references_to(method), &[call.origin]);
assert!(index.call_at(id, text.find("only(false)").unwrap() as u32).is_none());
# Ok::<(), Box<dyn std::error::Error>>(())
```

For a single lookup, [`NamedClassSources::resolved_call_at`] returns the written
name, selected method symbol and source definition together. To collect one
file's calls in source order, use [`NamedClassSources::resolved_calls_in`].
[`NamedClassSources::resolved_field_at`] returns the canonical field symbol and
distinguishes a read from a direct assignment. Public body and constructor
fields support explicit selections and bare uses in the declaring class.
Ordinary private body fields support bare uses and `this.field` in that class.
A parameter or local that shadows the field has its own symbol and contributes
no field reference. For repeated queries across files, build
one [`ResolvedMemberIndex`] per checked revision and inspect its diagnostics.
[`ResolvedMemberIndex::references_to`] returns committed usages in input file
order and then byte order. [`ResolvedMemberIndex::member_at`] accepts both
usage and declaration name tokens, including fields without usages.
[`ResolvedCallIndex`] remains available for existing direct-call clients.

A complete check contains [`CheckedClassBody::Complete`] with its planning
body. [`CheckedClassBody::Partial`] holds committed source-to-typed
mappings and ordinary local declarations from successful members.
[`CheckedClassBody::local_bindings`] borrows those declarations in typed order.
[`CheckedClassBody::Invalid`] carries the requested symbol when validation
rejects it before typing. [`CheckedClassBody::class`] returns the symbol
for each outcome. [`ClassBodyReport`] owns member errors and committed warnings.
The collection borrows neither the report nor the arenas. Keep it with its original
named files, semantic store and typed arena. After an edit, build a fresh
collection and navigation indices from the new session. Numeric IDs and byte
ranges from the previous version cannot identify declarations in the new one.

### Resolve a name after an edit

[`ResolvedMemberIndex::symbol_at`] returns the symbol kind, canonical ID,
declaration and optional usage token for methods, fields, ordinary named
method parameters in a single value list, and simple method-local `val` and `var`
declarations. [`ResolvedMemberIndex::value_access_at`] distinguishes reads
from direct assignment targets. A local enters the index when its method
commits, even if a sibling method fails. [`ResolvedMemberIndex::parameter_at`]
also gives the owning method. Both work after the semantic arenas are released.
The compiler can compare the parameter IDs with its method plan in the same
session. An editor rebuilds the index for each revision.

Two edits change first the parameter name and then the local name. The final
`count` stays at the same offset while its target moves from field to parameter
to local. Undoing the edits restores the preceding targets.

```rust
use dotty::{ResolvedMemberIndex, core::{SourceId, SymbolKind}};
# use dotty::{CheckedClassBodies, ClassHeaderBindings, NamedClassSources, parse_source};
# use dotty::core::{AstArena, Definitions, NoResolver, Packages, SemanticStore};
# use dotty::typer::SignatureCompletion;
# fn check(id: SourceId, text: &str) -> Result<ResolvedMemberIndex, Box<dyn std::error::Error>> {
#     let mut store = SemanticStore::new();
#     let defs = Definitions::bootstrap(&mut store);
#     let mut packages = Packages::new();
#     let file = parse_source(id, text, Default::default())?.bind(&mut store.names)
#         .name("A.scala", &mut store, &mut packages)?.into_classes(&store)?;
#     let sources = NamedClassSources::new([&file], &store)?;
#     let mut arena = AstArena::new();
#     let class = &file.classes()[0];
#     sources.complete_constructor_signature(class.symbol, &mut store, defs, &packages)?;
#     assert!(matches!(sources.try_complete_class_header_reported(class.symbol, &mut store, defs, &packages, &mut arena, &mut ClassHeaderBindings::default())?, SignatureCompletion::Complete(_)));
#     for member in class.methods().chain(class.fields()) {
#         assert!(matches!(sources.try_complete_member_signature(member.symbol, &mut store, defs, &packages)?, SignatureCompletion::Complete(_)));
#     }
#     let (body, report) = sources.check_class_body(class.symbol, &mut store, defs, &packages, &mut arena, NoResolver).into_parts();
#     assert!(report.errors.is_empty());
#     let mut bodies = CheckedClassBodies::default();
#     bodies.record(body)?;
#     Ok(ResolvedMemberIndex::build(&sources, &bodies, &arena, &store))
# }
let id = SourceId::from_index(0);
let original = "class A { val count: Int = 1; def read(other: Int): Int = { val spare = 2; count } }";
let parameter = original.replace("other", "count");
let local = parameter.replace("spare", "count");
let offset = original.rfind("count").unwrap() as u32;
for (text, kind, declaration) in [
    (original, SymbolKind::Field, "count:"),
    (parameter.as_str(), SymbolKind::Parameter, "count: Int)"),
    (local.as_str(), SymbolKind::Local, "count = 2"),
    (parameter.as_str(), SymbolKind::Parameter, "count: Int)"),
    (original, SymbolKind::Field, "count:"),
] {
    let index = check(id, text)?;
    let site = index.symbol_at(id, offset).ok_or("missing read")?;
    assert_eq!(site.kind, kind);
    assert_eq!(site.target.name.start(), text.find(declaration).unwrap() as u32);
    assert_eq!(index.references_to(site.symbol), &[site.usage.unwrap()]);
}
# Ok::<(), Box<dyn std::error::Error>>(())
```

### Follow a superclass and its constructor argument

[`ResolvedMemberIndex::build_with_headers`] includes parent argument reads
from the supplied [`ClassHeaderBindings`]. The body-only builder remains
available for clients that retain just body results. A plain primary
constructor parameter has one written declaration and two symbols. Use
[`SourceConstructorParameter::parameter`] for source references and
[`SourceConstructorParameter::formal`] to compare with the constructor plan.
Explicit constructor `val` and `var` declarations use field navigation.

[`ResolvedMemberIndex::parent_at`] uses the same completed class information
as the JVM planner for both `extends Base` and `extends Base(left)`. The
[`ResolvedSourceParent`] gives the child, superclass and both source locations.
Class references include these parents, simple parameter annotations in completed
constructors and methods, explicit method result types, and construction sites in checked bodies.

The example indexes a header before checking any class bodies. Changing
`Base(left)` to `Peer(left)` redirects the superclass reference. Changing the
argument to `next` redirects the parameter read. Undo restores both targets.

```rust
use dotty::{CheckedClassBodies, ClassHeaderBindings, NamedClassSources, ResolvedMemberIndex, parse_source};
use dotty::core::{AstArena, Definitions, Packages, SemanticStore, SourceId};
use dotty::typer::SignatureCompletion;

let original = "class Base(seed: Int); class Peer(seed: Int); class Derived(left: Int, next: Int) extends Base(left)";
let peer = original.replace("Base(left)", "Peer(left)");
let edit = peer.replace("Peer(left)", "Peer(next)");
let parent_pos = original.rfind("Base").unwrap() as u32;
let arg = original.rfind("left").unwrap() as u32;
for (text, base, selected, unused) in [
    (original, "Base", "left:", "next:"),
    (peer.as_str(), "Peer", "left:", "next:"),
    (edit.as_str(), "Peer", "next:", "left:"),
    (original, "Base", "left:", "next:"),
] {
    let source = SourceId::from_index(0);
#   let mut store = SemanticStore::new();
#   let defs = Definitions::bootstrap(&mut store);
#   let mut packages = Packages::new();
#   let file = parse_source(source, text, Default::default())?.bind(&mut store.names)
#       .name("Example.scala", &mut store, &mut packages)?.into_classes(&store)?;
#   let sources = NamedClassSources::new([&file], &store)?;
#   let mut arena = AstArena::new();
    let mut headers = ClassHeaderBindings::default();
#   for (_, class) in sources.classes() {
#       sources.complete_constructor_signature(class.symbol, &mut store, defs, &packages)?;
#   }
#   for (_, class) in sources.classes() {
#       assert!(matches!(sources.try_complete_class_header_reported(class.symbol, &mut store, defs, &packages, &mut arena, &mut headers)?, SignatureCompletion::Complete(_)));
#   }
    let index = ResolvedMemberIndex::build_with_headers(&sources, &CheckedClassBodies::default(), &headers, &arena, &store);
    let relation = index.parent_at(source, parent_pos).ok_or("missing superclass")?;
    let declaration = index.symbol_at(source, text.find(base).unwrap() as u32).unwrap();
    assert_eq!(relation.parent, declaration.symbol);
    assert_eq!(index.references_to(relation.parent), &[relation.origin]);

    let p = index.constructor_parameter_at(source, arg).ok_or("missing parent argument")?;
    assert_eq!(p.target.name.start() as usize, text.find(selected).unwrap());
    assert_ne!(p.parameter, p.formal);
    let site = index.symbol_at(source, arg).unwrap();
    assert_eq!(index.references_to(p.parameter), &[site.usage.unwrap()]);
    let other = index.constructor_parameter_at(source, text.find(unused).unwrap() as u32).unwrap();
    assert!(index.references_to(other.parameter).is_empty());
}
# Ok::<(), Box<dyn std::error::Error>>(())
```

### Navigate from a construction to the class and its constructor

Use [`ResolvedMemberIndex::construction_at`] on the class token of a
simple explicit `new`. Its [`ResolvedSourceConstruction`] identifies the
constructed class and the selected primary constructor separately. Use the
class symbol for references. A neighboring argument or method selection has
its own target. Rebuild the index against the new revision after editing.

```rust
use dotty::{CheckedClassBodies, ClassHeaderBindings, NamedClassSources, ResolvedMemberIndex, parse_source};
use dotty::core::{AstArena, Definitions, Packages, SemanticStore, SourceId, NoResolver};
# use dotty::typer::SignatureCompletion;

let original = "class Base(n: Int) { def read(): Int = n }; class Peer(n: Int) { def read(): Int = n }; class Factory { def build(start: Int): Int = new Base(start).read() }";
let edited = original.replace("new Base", "new Peer");
let offset = original.find("new Base").unwrap() as u32 + 4;
for (text, selected) in [(original, 0), (edited.as_str(), 1), (original, 0)] {
    let id = SourceId::from_index(0);
#   let mut store = SemanticStore::new();
#   let defs = Definitions::bootstrap(&mut store);
#   let mut packages = Packages::new();
#   let file = parse_source(id, text, Default::default())?.bind(&mut store.names)
#       .name("Example.scala", &mut store, &mut packages)?.into_classes(&store)?;
#   let sources = NamedClassSources::new([&file], &store)?;
#   let mut arena = AstArena::new();
#   let mut headers = ClassHeaderBindings::default();
#   for (_,class) in sources.classes() {
#       sources.complete_constructor_signature(class.symbol,&mut store,defs,&packages)?;
#       assert!(matches!(sources.try_complete_class_header_reported(class.symbol,&mut store,defs,&packages,&mut arena,&mut headers)?, SignatureCompletion::Complete(_)));
#       for method in class.methods() {
#           assert!(matches!(sources.try_complete_member_signature(method.symbol,&mut store,defs,&packages)?, SignatureCompletion::Complete(_)));
#       }
#   }
#   let (body,report) = sources.check_class_body(file.classes()[2].symbol,&mut store,defs,&packages,&mut arena,NoResolver).into_parts();
#   assert!(report.errors.is_empty());
#   let mut bodies = CheckedClassBodies::default();
#   bodies.record(body)?;
    let index = ResolvedMemberIndex::build(&sources, &bodies, &arena, &store);
    let site = index.construction_at(id, offset).ok_or("missing construction")?;
    let expected = &file.classes()[selected];
    assert_eq!((site.class, site.constructor), (expected.symbol, expected.constructor));
    assert_eq!(index.references_to(site.class), &[site.origin]);
}
# Ok::<(), Box<dyn std::error::Error>>(())
```

### Resolve a class type in a primary constructor

After [`NamedClassSources::complete_constructor_signature`] succeeds, the
two written uses of `Base` below are already available to an editor.
[`ResolvedMemberIndex::constructor_type_at`] identifies the class-owned
parameter or field and the constructor-owned formal for each use. A compiler
can compare the latter with the corresponding JVM constructor parameter.
A rejected constructor contributes no annotation sites. A later failure in
a method body preserves the successful constructor's sites.

```rust
use dotty::{CheckedClassBodies, NamedClassSources, ResolvedMemberIndex, parse_source};
use dotty::core::{AstArena, Definitions, Packages, SemanticStore, SourceId};

let text = "class Base; class Holder(seed: Base, val first: Base)";
# let source = SourceId::from_index(0);
# let mut store = SemanticStore::new();
# let defs = Definitions::bootstrap(&mut store);
# let mut packages = Packages::new();
# let file = parse_source(source, text, Default::default())?.bind(&mut store.names)
#     .name("Holder.scala", &mut store, &mut packages)?.into_classes(&store)?;
let sources = NamedClassSources::new([&file], &store)?;
let holder = &file.classes()[1];
sources.complete_constructor_signature(holder.symbol, &mut store, defs, &packages)?;
let index = ResolvedMemberIndex::build(&sources, &CheckedClassBodies::default(), &AstArena::new(), &store);
let offset = text.rfind("Base").unwrap() as u32;
let site = index.constructor_type_at(source, offset).ok_or("missing type annotation")?;
assert_eq!(site.class, file.classes()[0].symbol);
assert_eq!(site.parameter, holder.constructor_fields[0]);
assert_eq!(store.symbols.get(site.formal).owner, Some(holder.constructor));
assert_eq!(index.references_to(site.class).len(), 2);
# Ok::<(), Box<dyn std::error::Error>>(())
```

### Navigate through a completed method signature

[`ResolvedMemberIndex::method_type_at`] returns the canonical method, the
referenced class and a [`MethodTypeRole`] for each supported written type.
A parameter site identifies the exact formal. Both parameter and result
sites are available as soon as the method signature succeeds, including when
its body later fails. An unfinished signature supplies no annotation sites.
The query covers single unqualified class names in ordinary non-generic
methods with one plain value list.

In this example the parameter changes from `Base` to `Peer` while the result
and the allocation still refer to `Base`. Undo restores the first revision.

```rust
use dotty::{CheckedClassBodies, MethodTypeRole, ResolvedMemberIndex};
# use dotty::{ClassHeaderBindings, NamedClassSources, parse_source};
# use dotty::core::{AstArena, Definitions, NoResolver, Packages, SemanticStore, SourceId};
# use dotty::typer::SignatureCompletion;

let original = "class Base; class Peer; class Use { def copy(x: Base): Base = new Base() }";
let edit = original.replace("x: Base", "x: Peer");
let input = original.find("Base)").unwrap() as u32;
let result = original.find("Base =").unwrap() as u32;
for (text, param_class) in [(original, 0), (edit.as_str(), 1), (original, 0)] {
#   let source = SourceId::from_index(0);
#   let mut store = SemanticStore::new();
#   let defs = Definitions::bootstrap(&mut store);
#   let mut packages = Packages::new();
#   let file = parse_source(source, text, Default::default())?.bind(&mut store.names)
#       .name("Example.scala", &mut store, &mut packages)?.into_classes(&store)?;
#   let sources = NamedClassSources::new([&file], &store)?;
#   let mut arena = AstArena::new();
#   let mut headers = ClassHeaderBindings::default();
#   for (_, class) in sources.classes() {
#       sources.complete_constructor_signature(class.symbol, &mut store, defs, &packages)?;
#       assert!(matches!(sources.try_complete_class_header_reported(class.symbol, &mut store, defs, &packages, &mut arena, &mut headers)?, SignatureCompletion::Complete(_)));
#   }
#   let owner = file.classes()[2].symbol;
#   let method = file.classes()[2].methods().next().unwrap().symbol;
#   assert!(matches!(sources.try_complete_member_signature(method, &mut store, defs, &packages)?, SignatureCompletion::Complete(_)));
    let mut bodies = CheckedClassBodies::default();
    let signatures = ResolvedMemberIndex::build(&sources, &bodies, &arena, &store);
    let arg = signatures.method_type_at(source, input).unwrap();
    let ret = signatures.method_type_at(source, result).unwrap();
    assert!(matches!(arg.role, MethodTypeRole::Parameter(_)));
    assert_eq!((ret.method, ret.role), (arg.method, MethodTypeRole::Result));
    assert_eq!(arg.class, file.classes()[param_class].symbol);
    assert_eq!(ret.class, file.classes()[0].symbol);

    let (body, report) = sources.check_class_body(owner, &mut store, defs, &packages, &mut arena, NoResolver).into_parts();
    assert!(report.errors.is_empty());
    bodies.record(body)?;
    let checked = ResolvedMemberIndex::build(&sources, &bodies, &arena, &store);
    let allocation = checked.construction_at(source, text.rfind("Base()").unwrap() as u32).unwrap();
    assert_eq!(allocation.class, ret.class);
    assert_eq!(checked.references_to(ret.class).len(), if arg.class == ret.class { 3 } else { 2 });
}
# Ok::<(), Box<dyn std::error::Error>>(())
```

## Follow signature dependencies between source files

A compiler following one provider can query [`SignatureDependencies::blockage`]
after each blocked attempt. The query follows the recorded path from that symbol
and leaves the retry order intact. An editor that finishes the phase can use
[`ClassHeaderReport::blockage_report`] or [`MemberSignatureReport::blockage_report`]
to retain the causes and source positions together.

This example retries `Leaf` after its base completes, then reports a separate
two-file cycle after releasing the semantic store.

```rust
use dotty::{ClassHeaderBindings, ClassHeaderPhase, NamedClassSources, SignatureBlockage};
use dotty::core::{AstArena, Definitions, Packages, SemanticStore, SourceId};
use dotty::typer::SignatureCompletion;

let report = {
    let files = [
        ("Leaf.scala", "class Leaf(x: Int) extends Base(x)"),
        ("Base.scala", "class Base(x: Int)"),
        ("A.scala", "class A(x: Int) extends B(x)"),
        ("B.scala", "class B(x: Int) extends A(x)"),
    ];
#     let mut store = SemanticStore::new();
#     let defs = Definitions::bootstrap(&mut store);
#     let mut packages = Packages::new();
#     let mut units = Vec::new();
#     for (i, (filename, text)) in files.into_iter().enumerate() {
#         let source = SourceId::from_index(i as u32);
#         let parsed = dotty::parse_source(source, text, Default::default())?.bind(&mut store.names);
#         let file = parsed.name(filename, &mut store, &mut packages)?.into_classes(&store)?;
#         units.push(file);
#     }
    let sources = NamedClassSources::new(&units, &store)?;
#     let mut arena = AstArena::new();
#     let mut bindings = ClassHeaderBindings::default();
    let phase = ClassHeaderPhase::try_prepare(
        &sources, &mut store, defs, &packages, &mut arena, &mut bindings,
    )?;
    let [leaf, base, a, b] = [0, 1, 2, 3].map(|i| units[i].classes()[0].symbol);

    let mut attempts = Vec::new();
    let mut phase = phase.try_complete_roots([leaf], |id, result| attempts.push((id, result)))?;
    assert!(matches!(attempts.as_slice(), [(l, SignatureCompletion::BlockedOn(p)),
        (b, SignatureCompletion::Complete(_)), (retry, SignatureCompletion::Complete(_))]
        if (*l, *p, *b, *retry) == (leaf, base, base, leaf)));
    phase.attempt(a).unwrap();
    phase.attempt(b).unwrap();
    let Some(SignatureBlockage::CycleMember(cycle)) = phase.dependencies().blockage(a) else {
        return Err("expected a header cycle".into());
    };
    assert_eq!(cycle.path(), [a, b, a]);

    phase.finish().blockage_report(sources.declarations(), &store)
};

assert_eq!(report.waiters().len(), 2);
let mut cycles = report.cycles();
let (cycle, participants) = cycles.next().unwrap();
assert!(cycles.next().is_none());
assert_eq!(cycle.code(), "signature.header_cycle");
let sites: Vec<_> = participants.map(|site| (site.name.as_deref(), site.location.unwrap().source())).collect();
assert_eq!(sites, [(Some("A"), SourceId::from_index(2)), (Some("B"), SourceId::from_index(3))]);
# Ok::<(), Box<dyn std::error::Error>>(())
```

Finishing a phase classifies waiting roots together, sharing traversal of
common paths. Capturing the report uses those results, visits each distinct
cycle once and copies each referenced declaration's spelling once. The
[`SignatureBlockageReport::cycles`] iterator visits each participant once per
cycle. Its closed symbol path is shared with the waiter causes.
[`SignatureBlockageReport::waiters`] distinguishes cycle participants, downstream
waiters, pending providers and failed providers. For a failed provider, publish
the direct diagnostic retained by the phase. A missing position supplies no
source range. Retained positions refer to the captured document versions and
symbol IDs belong to the original session.

Clients driving the lower-level completion APIs directly can record each
outcome in [`SignatureDependencies`] and each direct rejection with
[`SignatureDependencies::record_failure`]. Use one tracker for each phase and
set of named files.

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
