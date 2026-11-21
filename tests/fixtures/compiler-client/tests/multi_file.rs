//! The consumer owns phase ordering, plans and output for classes with a no-arg constructor.
#[path = "support/class_planning.rs"]
mod class_planning;
#[path = "support/parsing.rs"]
mod parsing;
#[path = "support/pipeline_timing.rs"]
mod pipeline_timing;
#[path = "support/typer_error_spans.rs"]
mod typer_error_spans;

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use dotty::core::{
    AstArena, Definitions, Packages, SemanticStore, SourceId, SourceSemanticIndex, SymbolId,
    SymbolInfo, SymbolKind, TreeKind, Typed, Untyped,
};
use dotty::jvm::{
    GeneratedClass, JvmClassPlan, JvmCompilationUnit, PreparedJvmEmission, emit_class,
};
use dotty::jvm_class_plan::{
    ClassHeaderBindings, ClassPlanError, JvmClassPlanner, TypedClassBodies,
};
use dotty::jvm_method_plan::JvmMethodPlanner;
use dotty::namer::{NamingOutcome, SourceParseStatus, name_compilation_unit_with_diagnostics};
use dotty::parser::ParseResult;
use dotty::source_declarations::{Declaration, SourceClass, source_classes};
use dotty::typer::{SignatureCompletion, SourceTyper, TyperError};
use dotty::{
    DetachedSource, ParsedSource, SourceDeclarationIndex, SourceDiagnostic, SourceFileCalls,
};

struct NamedUnit {
    source: SourceId,
    ast: AstArena<Untyped>,
    calls: SourceFileCalls,
    index: SourceSemanticIndex,
    classes: Vec<SourceClass>,
}

impl NamedUnit {
    fn methods(&self) -> impl Iterator<Item = Declaration> + '_ {
        self.classes.iter().flat_map(SourceClass::methods)
    }

    fn method_symbol(&self) -> SymbolId {
        let mut methods = self.methods();
        let method = methods.next().expect("expected one method");
        assert!(methods.next().is_none());
        method.symbol
    }

    fn method_symbols(&self) -> impl Iterator<Item = SymbolId> + '_ {
        self.methods().map(|method| method.symbol)
    }

    fn typer<'a>(
        &'a self,
        store: &'a mut SemanticStore,
        definitions: Definitions,
        packages: &'a Packages,
    ) -> SourceTyper<'a> {
        SourceTyper::new(
            &self.ast,
            self.source,
            &self.index,
            store,
            definitions,
            packages,
        )
    }
}

fn name_source(
    id: SourceId,
    name: &str,
    source: ParsedSource,
    store: &mut SemanticStore,
    packages: &mut Packages,
) -> Result<(ParseResult, SourceSemanticIndex), Vec<SourceDiagnostic>> {
    if !source.diagnostics.is_empty() {
        return Err(source.diagnostics);
    }
    let parsed = source.syntax;
    match name_compilation_unit_with_diagnostics(
        &parsed.ast,
        parsed.root,
        id,
        name,
        SourceParseStatus::Clean,
        store,
        packages,
    )
    .unwrap()
    {
        NamingOutcome::Named { index, diagnostics } => {
            assert!(diagnostics.is_empty(), "{diagnostics:?}");
            Ok((parsed, *index))
        }
        NamingOutcome::Rejected { diagnostics } => Err(diagnostics
            .iter()
            .map(SourceDiagnostic::from_namer)
            .collect()),
    }
}

fn enter(
    id: SourceId,
    name: &str,
    parsed: ParsedSource,
    store: &mut SemanticStore,
    packages: &mut Packages,
) -> Result<NamedUnit, Vec<SourceDiagnostic>> {
    let (parsed, index) = name_source(id, name, parsed, store, packages)?;
    let classes = source_classes(&parsed.ast, parsed.root, id, &index, store).unwrap();
    let calls = SourceFileCalls::new(parsed.call_names, &parsed.ast, &index, &classes).unwrap();
    Ok(NamedUnit {
        source: id,
        ast: parsed.ast,
        calls,
        index,
        classes,
    })
}

#[test]
fn source_diagnostics_follow_a_field_from_parsing_through_typing() {
    let source = SourceId::from_index(47);
    let file = "Outer.scala";
    let broken = "class Outer {
  /* 😀 */ private[Missing] val hidden: Int 1
}";
    let parsed_ok = broken.replace("Int 1", "Int = 1");
    let fixed = parsed_ok.replace("[Missing]", "");
    let parse = |text: &str, store: &mut SemanticStore| {
        parsing::parse_files(&[(source, file, text)], 1)
            .pop()
            .unwrap()
            .into_session(&mut store.names)
    };
    let mut store = SemanticStore::new();
    let defs = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    for (text, token, code, message) in [
        (
            broken,
            "1",
            "parser.declaration.expected_value_equals",
            "Expected '=' before the initializer",
        ),
        (
            parsed_ok.as_str(),
            "Missing",
            "namer.visibility.invalid_qualifier",
            "Invalid 'private' qualifier 'Missing': expected an enclosing class, object or package",
        ),
    ] {
        let parsed = parse(text, &mut store);
        let reports = name_source(source, file, parsed, &mut store, &mut packages).unwrap_err();
        let start = text.find(token).unwrap() as u32;
        let range = dotty::core::TextRange::new(start, start + token.len() as u32).unwrap();
        let location =
            dotty::core::SourceSpan::new(source, dotty::core::Span::without_point(range));
        let [report] = reports.as_slice() else {
            panic!("expected one diagnostic: {reports:?}")
        };
        assert_eq!(report.code, Some(code));
        assert_eq!(report.message, message);
        assert_eq!(report.severity, dotty::core::DiagnosticSeverity::Error);
        assert_eq!(report.location, Some(location));
    }
    let parsed = parse(&fixed, &mut store);
    let units = [enter(source, file, parsed, &mut store, &mut packages).unwrap()];
    let class = &units[0].classes[0];
    let headers = complete_signatures(&units, &mut store, defs, &packages).unwrap();
    let (_, bodies) = type_bodies(&units, &mut store, defs, &packages, headers);
    let field = class.fields().next().unwrap();
    assert_eq!(
        store.symbols.info(field.symbol),
        &SymbolInfo::Complete(defs.int)
    );
    bodies.get(class.symbol).unwrap();
}

fn name_units_with_diagnostics(
    files: &[(SourceId, &str, &str)],
    parsed: Vec<DetachedSource>,
    store: &mut SemanticStore,
    packages: &mut Packages,
) -> (Vec<NamedUnit>, Vec<SourceDiagnostic>) {
    let mut units = Vec::new();
    let mut reports = Vec::new();
    for (&(id, name, _), detached) in files.iter().zip(parsed) {
        let parsed = detached.into_session(&mut store.names);
        match enter(id, name, parsed, store, packages) {
            Ok(unit) => units.push(unit),
            Err(diagnostics) => reports.extend(diagnostics),
        }
    }
    (units, reports)
}

fn name_units(
    files: &[(SourceId, &str, &str)],
    parsed: Vec<DetachedSource>,
    store: &mut SemanticStore,
    packages: &mut Packages,
) -> Vec<NamedUnit> {
    let (units, reports) = name_units_with_diagnostics(files, parsed, store, packages);
    assert!(reports.is_empty(), "{reports:?}");
    units
}

#[test]
fn mixed_syntax_errors_leave_the_other_file_executable_in_either_order() {
    let bad = (
        SourceId::from_index(17),
        "Broken.scala",
        "class Broken { val hidden: Int 1 } /* open",
    );
    let good = (
        SourceId::from_index(2),
        "A.scala",
        "package demo\nclass A { def value(): Int = 40 }",
    );
    for files in [[bad, good], [good, bad]] {
        for workers in [1, 2] {
            let mut store = SemanticStore::new();
            let defs = Definitions::bootstrap(&mut store);
            let mut packages = Packages::new();
            let (units, reports) = name_units_with_diagnostics(
                &files,
                parsing::parse_files(&files, workers),
                &mut store,
                &mut packages,
            );
            assert_eq!(reports.len(), 2, "{reports:?}");
            for (report, (code, site)) in reports.iter().zip([
                ("parser.declaration.expected_value_equals", "1"),
                ("lexer.unterminated_block_comment", "/* open"),
            ]) {
                assert_eq!(report.code, Some(code));
                let span = report.location.unwrap();
                assert_eq!(span.source(), bad.0);
                assert_eq!(
                    dotty::core::SourceText::new(bad.2)
                        .unwrap()
                        .slice(span.span().range())
                        .unwrap(),
                    site
                );
            }
            let [unit] = units.as_slice() else {
                panic!("expected the clean file")
            };
            assert_eq!(unit.source, good.0);
            let headers = complete_signatures(&units, &mut store, defs, &packages).unwrap();
            let (output, bodies) = type_bodies(&units, &mut store, defs, &packages, headers);
            let plan = JvmCompilationUnit {
                classes: class_plans(&units, &bodies, &output, &store).unwrap(),
                store: &store,
                typed_ast: &output.arena,
            };
            let artifact = emit_class(&plan, unit.classes[0].symbol, &defs).unwrap();
            assert_eq!(
                run_java_client("mixed-syntax", "SurvivingClassClient", &[artifact]).trim(),
                "40"
            );
        }
    }
}

// Scheduling belongs to the consumer. The index identifies the owning source
// even when different files reuse the same input tree slots.
#[derive(Debug, Default)]
struct TypedHeaders {
    arena: AstArena<Typed>,
    bindings: ClassHeaderBindings,
}

#[derive(Debug)]
enum SignatureError {
    Typing(TyperError),
    Field(dotty::ClassFieldSignatureError),
    Member(dotty::MemberSignatureError),
    HeaderCycle(Vec<SymbolId>),
    MemberCycle(Vec<SymbolId>),
}

impl From<TyperError> for SignatureError {
    fn from(error: TyperError) -> Self {
        Self::Typing(error)
    }
}

struct SignatureSources<'a> {
    units: &'a [NamedUnit],
    by_source: HashMap<SourceId, &'a NamedUnit>,
    declarations: SourceDeclarationIndex,
}

impl<'a> SignatureSources<'a> {
    fn error_span(&self, error: &TyperError) -> Option<dotty::core::SourceSpan> {
        dotty::typer_error_span_with(error, |source| {
            self.by_source.get(&source).map(|unit| &unit.ast)
        })
    }

    fn new(units: &'a [NamedUnit], store: &SemanticStore) -> Self {
        Self {
            units,
            by_source: units.iter().map(|unit| (unit.source, unit)).collect(),
            declarations: SourceDeclarationIndex::from_classes(
                store,
                units.iter().flat_map(|unit| &unit.classes),
            )
            .expect("valid named class views"),
        }
    }
}

fn complete_headers(
    sources: &SignatureSources,
    store: &mut SemanticStore,
    definitions: Definitions,
    packages: &Packages,
    observe: &mut impl FnMut(SymbolId, SignatureCompletion),
) -> Result<TypedHeaders, SignatureError> {
    // Parent applications need the declaring file's constructor signature.
    for unit in sources.units {
        let mut typer = unit.typer(store, definitions, packages);
        for class in &unit.classes {
            typer.complete_symbol(class.constructor)?;
        }
    }
    let mut headers = TypedHeaders::default();
    let mut path = Vec::new();
    let mut active = HashMap::new();
    for (_, root) in named_classes(sources.units) {
        if matches!(store.symbols.info(root.symbol), SymbolInfo::Complete(_)) {
            continue;
        }
        path.push(root.symbol);
        active.insert(root.symbol, 0);
        while let Some(&symbol) = path.last() {
            let header = sources.declarations.get(symbol).expect("indexed class");
            let unit = sources.by_source[&header.source];
            let completion = dotty::try_complete_class_header(
                symbol,
                &unit.ast,
                &unit.index,
                store,
                definitions,
                packages,
                &mut headers.arena,
                &mut headers.bindings,
                |dep| {
                    sources
                        .declarations
                        .get(dep)
                        .is_some_and(|d| d.kind == SymbolKind::Class)
                },
            )?;
            observe(symbol, completion);
            match completion {
                SignatureCompletion::Complete(_) => {
                    path.pop();
                    active.remove(&symbol);
                }
                SignatureCompletion::BlockedOn(parent) => {
                    if let Some(&start) = active.get(&parent) {
                        let mut cycle = path[start..].to_vec();
                        cycle.push(parent);
                        return Err(SignatureError::HeaderCycle(cycle));
                    }
                    active.insert(parent, path.len());
                    path.push(parent);
                }
            }
        }
    }
    Ok(headers)
}

fn complete_signatures(
    units: &[NamedUnit],
    store: &mut SemanticStore,
    definitions: Definitions,
    packages: &Packages,
) -> Result<TypedHeaders, SignatureError> {
    complete_signatures_in_order(units, units, store, definitions, packages, |_, _| {})
}

fn complete_signatures_in_order<'a>(
    units: &'a [NamedUnit],
    roots: impl IntoIterator<Item = &'a NamedUnit>,
    store: &mut SemanticStore,
    definitions: Definitions,
    packages: &Packages,
    observe: impl FnMut(SymbolId, SignatureCompletion),
) -> Result<TypedHeaders, SignatureError> {
    complete_methods_in_order(
        units,
        roots.into_iter().flat_map(NamedUnit::method_symbols),
        store,
        definitions,
        packages,
        observe,
    )
}

fn complete_methods_in_order(
    units: &[NamedUnit],
    roots: impl IntoIterator<Item = SymbolId>,
    store: &mut SemanticStore,
    definitions: Definitions,
    packages: &Packages,
    mut observe: impl FnMut(SymbolId, SignatureCompletion),
) -> Result<TypedHeaders, SignatureError> {
    let sources = SignatureSources::new(units, store);
    let headers = complete_headers(&sources, store, definitions, packages, &mut observe)?;

    for (unit, class) in named_classes(units) {
        while let dotty::ClassFieldSignatureCompletion::BlockedOn { field, dependency } =
            dotty::try_complete_class_field_signatures(
                class.symbol,
                &unit.ast,
                &unit.index,
                store,
                definitions,
                packages,
                |dep| sources.declarations.member(dep).is_some(),
            )
            .map_err(SignatureError::Field)?
        {
            observe(field, SignatureCompletion::BlockedOn(dependency));
            complete_member_path(
                &[field, dependency],
                &sources,
                store,
                definitions,
                packages,
                &mut observe,
            )?;
        }
    }
    for symbol in roots {
        if !matches!(store.symbols.info(symbol), SymbolInfo::Complete(_)) {
            complete_member_path(
                &[symbol],
                &sources,
                store,
                definitions,
                packages,
                &mut observe,
            )?;
        }
    }
    Ok(headers)
}

fn complete_member_path(
    path: &[SymbolId],
    sources: &SignatureSources,
    store: &mut SemanticStore,
    definitions: Definitions,
    packages: &Packages,
    observe: &mut impl FnMut(SymbolId, SignatureCompletion),
) -> Result<(), SignatureError> {
    let mut pending = path.to_vec();
    let mut active: HashMap<_, _> = path.iter().enumerate().map(|(i, &id)| (id, i)).collect();
    while let Some(&symbol) = pending.last() {
        let member = sources.declarations.get(symbol).expect("indexed member");
        let unit = sources.by_source[&member.source];
        let completion = dotty::try_complete_member_signature(
            symbol,
            &sources.declarations,
            &unit.ast,
            &unit.index,
            store,
            definitions,
            packages,
        )
        .map_err(SignatureError::Member)?;
        observe(symbol, completion);
        match completion {
            SignatureCompletion::Complete(_) => {
                pending.pop();
                active.remove(&symbol);
            }
            SignatureCompletion::BlockedOn(dependency) => {
                if let Some(start) = active.insert(dependency, pending.len()) {
                    let cycle = &pending[start..];
                    if cycle.iter().any(|&id| {
                        sources
                            .declarations
                            .get(id)
                            .is_some_and(|d| d.kind == SymbolKind::Field)
                    }) {
                        let mut path = cycle.to_vec();
                        path.push(dependency);
                        return Err(SignatureError::MemberCycle(path));
                    }
                    return Err(
                        TyperError::RecursiveInferredMethodResult { symbol: dependency }.into(),
                    );
                }
                pending.push(dependency);
            }
        }
    }
    Ok(())
}

fn with_plan<R>(
    files: &[(SourceId, &str, &str)],
    run: impl FnOnce(&JvmCompilationUnit<'_>, &Definitions) -> R,
) -> R {
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let units = name_units(
        files,
        parsing::parse_files(files, 4),
        &mut store,
        &mut packages,
    );
    let headers = complete_signatures(&units, &mut store, definitions, &packages).unwrap();

    let (output, typed_classes) = type_bodies(&units, &mut store, definitions, &packages, headers);
    let classes = class_plans(&units, &typed_classes, &output, &store).unwrap();
    let plan = JvmCompilationUnit {
        store: &store,
        typed_ast: &output.arena,
        classes,
    };
    run(&plan, &definitions)
}

fn named_classes(
    units: &[NamedUnit],
) -> impl DoubleEndedIterator<Item = (&NamedUnit, &SourceClass)> {
    units
        .iter()
        .flat_map(|unit| unit.classes.iter().map(move |class| (unit, class)))
}

fn type_bodies(
    units: &[NamedUnit],
    store: &mut SemanticStore,
    definitions: Definitions,
    packages: &Packages,
    mut headers: TypedHeaders,
) -> (TypedHeaders, TypedClassBodies) {
    let mut typed_classes = TypedClassBodies::default();
    for (unit, class) in named_classes(units) {
        let body = dotty::type_class_body(
            class,
            &unit.ast,
            &unit.index,
            store,
            definitions,
            packages,
            &mut headers.arena,
        )
        .unwrap();
        typed_classes.record(body).unwrap();
    }

    (headers, typed_classes)
}

fn class_plans(
    units: &[NamedUnit],
    typed_classes: &TypedClassBodies,
    output: &TypedHeaders,
    store: &SemanticStore,
) -> Result<Vec<JvmClassPlan>, ClassPlanError> {
    let mut classes = Vec::new();
    for (unit, class) in named_classes(units) {
        let body = typed_classes.get(class.symbol)?;
        let planner = JvmClassPlanner::new(store, &unit.ast, &unit.index)
            .with_header_bindings(&output.arena, &output.bindings);
        classes.push(planner.plan_typed_class(class, &output.arena, body)?);
    }
    Ok(classes)
}

fn emit_batch(
    prepared: &PreparedJvmEmission<'_>,
    classes: &[JvmClassPlan],
    workers: usize,
) -> (Vec<GeneratedClass>, Duration) {
    let chunk_size = classes.len().div_ceil(workers).max(1);
    std::thread::scope(|scope| {
        let tasks: Vec<_> = classes
            .chunks(chunk_size)
            .map(|chunk| {
                scope.spawn(|| {
                    let start = Instant::now();
                    let artifacts = chunk
                        .iter()
                        .map(|class| prepared.emit_class(class.class).unwrap())
                        .collect::<Vec<_>>();
                    (artifacts, start.elapsed())
                })
            })
            .collect();
        let mut artifacts = Vec::with_capacity(classes.len());
        let mut worker_time = Duration::ZERO;
        for task in tasks {
            let (chunk, elapsed) = task.join().unwrap();
            artifacts.extend(chunk);
            worker_time += elapsed;
        }
        (artifacts, worker_time)
    })
}

fn compile<const N: usize>(files: [(SourceId, &str, &str); N]) -> Vec<GeneratedClass> {
    with_plan(&files, |plan, definitions| {
        let prepared = plan.prepare_emission(definitions).unwrap();
        let (concurrent, _) = emit_batch(&prepared, &plan.classes, 2);
        for (class, artifact) in plan.classes.iter().zip(&concurrent) {
            assert_eq!(
                &emit_class(plan, class.class, definitions).unwrap(),
                artifact
            );
        }
        concurrent
    })
}

const OVERLOADS: &str = "class A {
  def take(n: Int): Int = n
  def take(flag: Boolean): Int = 7
}";

fn assert_overload_target(
    sources: &SignatureSources<'_>,
    method: SymbolId,
    provider: &NamedUnit,
    expected: Declaration,
    declaration: &str,
) {
    use dotty::core::{SourceDefinition, TextRange};

    assert_eq!(method, expected.symbol);
    let location = sources.declarations.get(method).unwrap();
    assert_eq!(
        (location.source, location.class),
        (provider.source, provider.classes[0].symbol)
    );
    let target = sources.by_source[&location.source];
    let Some(SourceDefinition::Canonical { source, tree }) = target.index.definition_of(method)
    else {
        panic!("expected source method")
    };
    assert_eq!((source, tree), (provider.source, expected.tree));
    let span = target.ast.get(tree).position.unwrap();
    assert_eq!(span.source(), provider.source);
    let start = OVERLOADS.find(declaration).unwrap() as u32;
    assert_eq!(
        span.span().range(),
        TextRange::new(start, start + declaration.len() as u32).unwrap()
    );
}

#[test]
fn selected_method_tracks_an_edited_overload_in_both_file_orders() {
    const BEFORE: &str = "class B { def read(a: A): Int = a.take(42) }";
    let a = SourceId::from_index(7);
    let b = SourceId::from_index(2);
    let edited = BEFORE.replace("42", "false");
    for reverse in [false, true] {
        for (text, overload, declaration) in [
            (BEFORE, 0, "def take(n: Int): Int = n"),
            (edited.as_str(), 1, "def take(flag: Boolean): Int = 7"),
        ] {
            let mut files = [(a, "A.scala", OVERLOADS), (b, "B.scala", text)];
            if reverse {
                files.reverse();
            }
            let mut store = SemanticStore::new();
            let defs = Definitions::bootstrap(&mut store);
            let mut packages = Packages::new();
            let units = name_units(
                &files,
                parsing::parse_files(&files, 2),
                &mut store,
                &mut packages,
            );
            let headers = complete_signatures(&units, &mut store, defs, &packages).unwrap();
            let sources = SignatureSources::new(&units, &store);
            let (output, bodies) = type_bodies(&units, &mut store, defs, &packages, headers);
            let caller = sources.by_source[&b];
            let provider = sources.by_source[&a];
            let calls = &caller.calls;
            let method_at = |offset| {
                calls.selected_method_at(offset, &caller.ast, &bodies, &output.arena, &store)
            };
            let name_start = text.find("take(").unwrap() as u32;
            let method = method_at(name_start).unwrap();
            for offset in name_start..name_start + 4 {
                assert_eq!(method_at(offset), Some(method));
            }
            // Receiver, dot, opening parenthesis, argument and closing parenthesis.
            let close = text.rfind(')').unwrap() as u32;
            for offset in (name_start - 2..name_start).chain(name_start + 4..=close) {
                assert_eq!(method_at(offset), None, "offset {offset} in {text}");
            }
            assert_eq!(method_at(text.len() as u32), None);
            let expected = provider.methods().nth(overload).unwrap();
            assert_overload_target(&sources, method, provider, expected, declaration);

            let body = bodies.get(caller.classes[0].symbol).unwrap();
            let read = caller.methods().next().unwrap();
            let TreeKind::DefDef(def) = &caller.ast.get(read.tree).kind else {
                panic!("expected B.read")
            };
            let call = def.rhs.unwrap();
            assert_eq!(
                body.selected_method(read.tree, &caller.ast, &output.arena, &store),
                None
            );
            let other_body = bodies.get(provider.classes[0].symbol).unwrap();
            assert_eq!(
                other_body.selected_method_at(
                    name_start,
                    caller.calls.call_names(),
                    &caller.ast,
                    &output.arena,
                    &store
                ),
                None
            );
            assert_eq!(
                other_body.selected_method(call, &caller.ast, &output.arena, &store),
                None
            );
        }
    }
}

#[test]
fn checked_calls_survive_a_failing_sibling_in_both_file_orders() {
    let a = SourceId::from_index(7);
    let b = SourceId::from_index(2);
    for reverse in [false, true] {
        for (argument, broken) in [("false, 1", true), ("1", false)] {
            let text = format!(
                "class B {{ def good(a: A): Int = a.take(42); def bad(a: A): Int = a.take({argument}) }}"
            );
            let mut files = [(a, "A.scala", OVERLOADS), (b, "B.scala", text.as_str())];
            if reverse {
                files.reverse();
            }
            let mut store = SemanticStore::new();
            let defs = Definitions::bootstrap(&mut store);
            let mut packages = Packages::new();
            let units = name_units(
                &files,
                parsing::parse_files(&files, 2),
                &mut store,
                &mut packages,
            );
            let mut headers = complete_signatures(&units, &mut store, defs, &packages).unwrap();
            let sources = SignatureSources::new(&units, &store);
            let caller = sources.by_source[&b];
            let class = &caller.classes[0];
            let check = dotty::check_class_body(
                class,
                &caller.ast,
                &caller.index,
                &mut store,
                defs,
                &packages,
                &mut headers.arena,
                dotty::core::NoResolver,
            );
            let at = |offset| {
                caller.calls.checked_method_at(
                    offset,
                    &caller.ast,
                    &headers.arena,
                    &store,
                    |owner| (owner == class.symbol).then_some(&check),
                )
            };
            let provider = sources.by_source[&a];
            let method = at(text.find("take").unwrap() as u32).unwrap();
            assert_overload_target(
                &sources,
                method,
                provider,
                provider.methods().next().unwrap(),
                "def take(n: Int): Int = n",
            );
            assert_eq!(
                at(text.rfind("take").unwrap() as u32),
                (!broken).then_some(method)
            );
            let target = caller
                .calls
                .definition_at(
                    text.find("take").unwrap() as u32,
                    &caller.ast,
                    &headers.arena,
                    &store,
                    |owner| (owner == class.symbol).then_some(&check),
                    |id| sources.by_source.get(&id).map(|unit| &unit.calls),
                )
                .unwrap();
            assert_eq!(target.source, a);
            let slice = |range: dotty::core::TextRange| {
                &OVERLOADS[range.start() as usize..range.end() as usize]
            };
            assert_eq!(slice(target.declaration), "def take(n: Int): Int = n");
            assert_eq!(slice(target.name), "take");
            assert_eq!(target.name.start(), OVERLOADS.find("take").unwrap() as u32);
            if broken {
                let errors = check.body.unwrap_err();
                let [dotty::ClassBodyError::Member { member, .. }] = errors.as_slice() else {
                    panic!("expected one failed member")
                };
                assert_eq!(
                    *member,
                    dotty::source_declarations::ClassMember::Method(
                        class.methods().nth(1).unwrap()
                    )
                );
            } else {
                JvmClassPlanner::new(&store, &caller.ast, &caller.index)
                    .with_header_bindings(&headers.arena, &headers.bindings)
                    .plan_typed_class(class, &headers.arena, &check.body.unwrap())
                    .unwrap();
            }
        }
    }
}

#[test]
fn selected_method_routes_two_classes_in_one_file_as_their_bodies_arrive() {
    const B: &str = "class First { def read(a: A): Int = a.take(42) }
class Second { def read(a: A): Int = a.take(false) }";
    let a = SourceId::from_index(7);
    let b = SourceId::from_index(2);
    for reverse in [false, true] {
        let mut files = [(a, "A.scala", OVERLOADS), (b, "B.scala", B)];
        if reverse {
            files.reverse();
        }
        let mut store = SemanticStore::new();
        let defs = Definitions::bootstrap(&mut store);
        let mut packages = Packages::new();
        let units = name_units(
            &files,
            parsing::parse_files(&files, 2),
            &mut store,
            &mut packages,
        );
        let mut headers = complete_signatures(&units, &mut store, defs, &packages).unwrap();
        let sources = SignatureSources::new(&units, &store);
        let caller = sources.by_source[&b];
        let provider = sources.by_source[&a];
        let calls = &caller.calls;
        let offsets = [
            B.find("take(42)").unwrap() as u32,
            B.find("take(false)").unwrap() as u32,
        ];
        let targets = [
            provider.methods().next().unwrap(),
            provider.methods().nth(1).unwrap(),
        ];
        let mut bodies = TypedClassBodies::default();
        for (n, class) in caller.classes.iter().enumerate() {
            let body = dotty::type_class_body(
                class,
                &caller.ast,
                &caller.index,
                &mut store,
                defs,
                &packages,
                &mut headers.arena,
            )
            .unwrap();
            bodies.record(body).unwrap();
            assert_eq!(
                offsets.map(|offset| calls.selected_method_at(
                    offset,
                    &caller.ast,
                    &bodies,
                    &headers.arena,
                    &store
                )),
                [
                    Some(targets[0].symbol),
                    (n == 1).then_some(targets[1].symbol)
                ]
            );
        }
        for ((offset, expected), declaration) in offsets.into_iter().zip(targets).zip([
            "def take(n: Int): Int = n",
            "def take(flag: Boolean): Int = 7",
        ]) {
            let method = calls
                .selected_method_at(offset, &caller.ast, &bodies, &headers.arena, &store)
                .unwrap();
            assert_overload_target(&sources, method, provider, expected, declaration);
        }
    }
}

#[test]
fn selected_method_distinguishes_a_direct_call_from_an_inserted_apply() {
    let text = "class Callable {
           def apply(n: Int): Int = n
           def direct(): Int = apply(42)
           def indirect(c: Callable): Int = c(42)
         }";
    let (mut store, defs, packages, unit) = class_planning::named(text);
    let units = std::slice::from_ref(&unit);
    let headers = complete_signatures(units, &mut store, defs, &packages).unwrap();
    let (output, bodies) = type_bodies(units, &mut store, defs, &packages, headers);
    let body = bodies.get(unit.classes[0].symbol).unwrap();
    let mut methods = unit.methods();
    let apply = methods.next().unwrap().symbol;
    for (spelling, expected) in [("apply(42)", Some(apply)), ("c(42)", None)] {
        let TreeKind::DefDef(def) = &unit.ast.get(methods.next().unwrap().tree).kind else {
            panic!("expected method")
        };
        let call = def.rhs.unwrap();
        assert!(matches!(unit.ast.get(call).kind, TreeKind::Apply(_)));
        assert_eq!(
            body.selected_method(call, &unit.ast, &output.arena, &store),
            expected
        );
        let offset = text.find(spelling).unwrap() as u32;
        assert_eq!(unit.calls.call_names().call_at(offset), Some(call));
        assert_eq!(
            body.selected_method_at(
                offset,
                unit.calls.call_names(),
                &unit.ast,
                &output.arena,
                &store
            ),
            expected
        );
    }
}

#[test]
fn separate_sources_compile_in_either_order_and_execute_on_jdk21() {
    let a = (
        SourceId::from_index(0),
        "A.scala",
        include_str!("../A.scala"),
    );
    let b = (
        SourceId::from_index(1),
        "B.scala",
        include_str!("../B.scala"),
    );
    let first = compile([a, b]);
    let second = compile([b, a]);
    assert!(
        first
            .iter()
            .map(|c| c.internal_name())
            .eq(["demo/A", "demo/B"])
    );
    assert!(first.iter().eq(second.iter().rev()));

    assert_eq!(run_java_client("separate", "Client", &first).trim(), "7");
}

#[test]
fn same_named_methods_in_one_source_execute_on_jdk21_in_either_file_order() {
    let providers = (
        SourceId::from_index(0),
        "Providers.scala",
        include_str!("../Providers.scala"),
    );
    let consumer = (
        SourceId::from_index(1),
        "Use.scala",
        include_str!("../Use.scala"),
    );
    for (order, files) in [
        ("providers-first", [providers, consumer]),
        ("consumer-first", [consumer, providers]),
    ] {
        let classes = compile(files);
        let mut names: Vec<_> = classes.iter().map(|class| class.internal_name()).collect();
        names.sort_unstable();
        assert_eq!(names, ["demo/A", "demo/B", "demo/Use"], "{order}");
        let stdout = run_java_client(order, "ProviderClient", &classes);
        assert_eq!(
            stdout.lines().collect::<Vec<_>>(),
            ["17", "42", "5"],
            "{order}"
        );
    }
}

#[test]
fn method_plan_rejects_another_methods_body_in_the_same_source() {
    use dotty::jvm_method_plan::MethodPlanError;

    let files = [(
        SourceId::from_index(0),
        "Pair.scala",
        "class Pair { def first(): Int = { var n = 1; n }; def second(): Int = { val n = 2; n } }",
    )];
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let units = name_units(
        &files,
        parsing::parse_files(&files, 1),
        &mut store,
        &mut packages,
    );
    let unit = &units[0];
    let [first, second] = unit.methods().collect::<Vec<_>>()[..] else {
        panic!("expected two methods")
    };
    let mut typer = unit.typer(&mut store, definitions, &packages);
    let first_root = typer.type_method_definition(first.tree).unwrap();
    typer.type_method_definition(second.tree).unwrap();
    let mut output = typer.into_typed_output();
    let planner = JvmMethodPlanner::new(&store, &unit.ast, &unit.index, &output);
    let first_plan = planner.plan(first.symbol).unwrap();
    let second_plan = planner.plan(second.symbol).unwrap();
    assert_eq!(first_plan.locals.len(), 1);
    assert_eq!(second_plan.locals.len(), 1);
    let first_local = first_plan.locals[0];
    let second_local = second_plan.locals[0];
    assert_eq!(store.symbols.owner(first_local.symbol), Some(first.symbol));
    assert_eq!(
        store.symbols.owner(second_local.symbol),
        Some(second.symbol)
    );
    assert!(first_local.mutable);
    assert!(!second_local.mutable);
    assert_ne!(first_plan.body, second_plan.body);

    let TreeKind::DefDef(definition) = &mut output.typed_ast.get_mut(first_root).kind else {
        panic!("expected typed method")
    };
    definition.rhs = second_plan.body;
    assert_eq!(
        JvmMethodPlanner::new(&store, &unit.ast, &unit.index, &output).plan(first.symbol),
        Err(MethodPlanError::BodyMismatch {
            method: first.symbol,
            expected: first_plan.body.unwrap(),
            actual: second_plan.body.unwrap()
        })
    );
}

fn run_java_client(label: &str, main_class: &str, classes: &[GeneratedClass]) -> String {
    let output = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("compiler-client-{label}-{}", std::process::id()));
    fs::create_dir(&output).unwrap();
    fs::create_dir(output.join("demo")).unwrap();
    for class in classes {
        fs::write(
            output.join(format!("{}.class", class.internal_name())),
            class.bytes(),
        )
        .unwrap();
    }
    let java_source = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("{main_class}.java"));
    let javac = Command::new("javac")
        .current_dir(&output)
        .args(["--release", "21", "-cp", ".", "-d", "."])
        .arg(java_source)
        .output()
        .unwrap();
    assert!(
        javac.status.success(),
        "{}",
        String::from_utf8_lossy(&javac.stderr)
    );
    let java = Command::new("java")
        .current_dir(&output)
        .args(["-Xverify:all", "-cp", ".", main_class])
        .output()
        .unwrap();
    assert!(
        java.status.success(),
        "{}",
        String::from_utf8_lossy(&java.stderr)
    );
    fs::remove_dir_all(output).unwrap();
    String::from_utf8(java.stdout).unwrap()
}

#[test]
#[ignore = "release-mode JVM scaling measurement"]
fn measure_jvm_batch() {
    use std::{hint::black_box, time::Instant};
    for count in [1, 10, 100] {
        let sources: Vec<_> = (0..count)
            .map(|i| format!("package demo\nclass C{i}:\n  def value(): Int = 7\n"))
            .collect();
        let files: Vec<_> = sources
            .iter()
            .enumerate()
            .map(|(i, text)| (SourceId::from_index(i as u32), "Batch.scala", text.as_str()))
            .collect();
        with_plan(&files, |plan, defs| {
            for _ in 0..5 {
                let start = Instant::now();
                plan.validate().unwrap();
                let validated = Instant::now();
                black_box(plan.metadata(defs).unwrap());
                let projected = Instant::now();
                for class in &plan.classes {
                    black_box(emit_class(plan, class.class, defs).unwrap());
                }
                let emitted = Instant::now();
                let prepared = plan.prepare_emission(defs).unwrap();
                let ready = Instant::now();
                for class in &plan.classes {
                    black_box(prepared.emit_class(class.class).unwrap());
                }
                eprintln!(
                    "classes={count} validate={:?} metadata_inclusive={:?} one_shot_batch={:?} prepare={:?} prepared_batch={:?}",
                    validated - start,
                    projected - validated,
                    emitted - projected,
                    ready - emitted,
                    ready.elapsed()
                );
            }
        });
    }
}

fn signature_pair(
    caller: &str,
    store: &mut SemanticStore,
    packages: &mut Packages,
) -> Vec<NamedUnit> {
    let files = [
        (SourceId::from_index(7), "A.scala", caller),
        (
            SourceId::from_index(2),
            "B.scala",
            "class B { def g() = 7 }",
        ),
    ];
    name_units(&files, parsing::parse_files(&files, 2), store, packages)
}

#[test]
fn cross_file_signature_completes_the_dependency_then_retries_the_original_method() {
    let mut store = SemanticStore::new();
    let defs = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let units = signature_pair("class A { def f(b: B) = b.g() }", &mut store, &mut packages);
    let f = units[0].method_symbol();
    let g = units[1].method_symbol();
    let mut attempts = Vec::new();
    complete_methods_in_order(
        &units,
        [f],
        &mut store,
        defs,
        &packages,
        |method, outcome| {
            if method == f || method == g {
                attempts.push((method, outcome));
            }
        },
    )
    .unwrap();
    let signature = |method| {
        let SymbolInfo::Complete(ty) = *store.symbols.info(method) else {
            panic!("missing signature")
        };
        let dotty::core::Type::Method(info) = store.types.get(ty) else {
            panic!("expected method")
        };
        assert_eq!(info.result, defs.int);
        SignatureCompletion::Complete(ty)
    };
    assert_eq!(
        attempts,
        [
            (f, SignatureCompletion::BlockedOn(g)),
            (g, signature(g)),
            (f, signature(f))
        ]
    );
}

#[test]
fn cross_file_bad_call_reports_arity_at_the_call_after_dependency_completion() {
    let mut store = SemanticStore::new();
    let defs = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let units = signature_pair(
        "class A { def f(b: B) = b.g(1) }",
        &mut store,
        &mut packages,
    );
    let caller = &units[0];
    let f = caller.method_symbol();
    let g = units[1].method_symbol();
    let TreeKind::DefDef(definition) = &caller.ast.get(caller.methods().next().unwrap().tree).kind
    else {
        panic!("expected method")
    };
    let call = definition.rhs.unwrap();
    let mut dependencies = Vec::new();
    let err = complete_methods_in_order(
        &units,
        [f],
        &mut store,
        defs,
        &packages,
        |method, outcome| {
            if let SignatureCompletion::BlockedOn(dependency) = outcome {
                dependencies.push((method, dependency));
            }
        },
    )
    .unwrap_err();
    assert_eq!(dependencies, [(f, g)]);
    assert!(
        matches!(err, SignatureError::Member(dotty::MemberSignatureError::Typing { member, declaration, error: TyperError::ApplicationArityMismatch { source, tree_index, expected: 0, actual: 1 } })
        if member == f && declaration.class == caller.classes[0].symbol && source == caller.source && tree_index == call.index()),
        "{err:?}"
    );
    assert!(matches!(store.symbols.info(f), SymbolInfo::Missing));
    assert!(matches!(store.symbols.info(g), SymbolInfo::Complete(_)));
}

#[test]
fn cross_file_inference_cycle_reports_the_reentered_method_and_rolls_back() {
    let a = (
        SourceId::from_index(0),
        "A.scala",
        "class A { def left(b: B) = b.right(this) }",
    );
    let b = (
        SourceId::from_index(1),
        "B.scala",
        "class B { def right(a: A) = a.left(this) }",
    );
    for files in [[a, b], [b, a]] {
        for first in [0, 1] {
            let mut store = SemanticStore::new();
            let definitions = Definitions::bootstrap(&mut store);
            let mut packages = Packages::new();
            let units = name_units(
                &files,
                parsing::parse_files(&files, 4),
                &mut store,
                &mut packages,
            );
            let error = complete_signatures_in_order(
                &units,
                [&units[first], &units[1 - first]],
                &mut store,
                definitions,
                &packages,
                |_, _| {},
            )
            .unwrap_err();
            assert!(
                matches!(error, SignatureError::Typing(TyperError::RecursiveInferredMethodResult { symbol }) if symbol == units[first].method_symbol()),
                "{error:?}"
            );
            for unit in &units {
                assert!(matches!(
                    store.symbols.info(unit.method_symbol()),
                    SymbolInfo::Missing
                ));
            }
        }
    }
}
