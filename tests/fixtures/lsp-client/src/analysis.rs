//! Fresh semantic session for one set of open document versions.
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::sync::Arc;

use dotty::core::{
    AstArena, Definitions, DocumentText, NoResolver, Packages, SemanticStore, SourceId, SourceSpan,
    SymbolId, TextRange, Typed,
};
use dotty::source_declarations::ClassMember;
use dotty::typer::{SignatureCompletion, TyperDiagnostic};
use dotty::{
    ClassBodyError, ClassHeaderBindings, ClassHeaderError, MemberSignatureError, NamedClassSource,
    NamedClassSources, NamedClassSourcesError, PreparationDiagnostic, ReportedTyperError,
    SignatureBlockage, SignatureCycle, SignatureDependencies, SignaturePhase,
    SourceDeclarationIndex,
};

#[derive(Clone)]
pub struct Document {
    pub version: i32,
    pub text: Arc<DocumentText>,
}

pub type Documents = BTreeMap<String, Document>;

pub enum Problem {
    Typing(ReportedTyperError),
    Header(ClassHeaderError),
    MemberSignature(MemberSignatureError),
    Body(ClassBodyError),
    Sources(NamedClassSourcesError),
    Scan(dotty::lexer::RawLexerError),
    Preparation(PreparationDiagnostic),
    Cycle {
        cycle: Arc<SignatureCycle>,
        location: Option<SourceSpan>,
    },
    Pending {
        provider: SymbolId,
        name: String,
        location: Option<SourceSpan>,
    },
    TyperWarning {
        source: SourceId,
        diagnostic: TyperDiagnostic,
    },
}

fn typing_problem(owner: SourceId, report: ReportedTyperError) -> (SourceId, Problem) {
    (
        report.location().map_or(owner, |span| span.source()),
        Problem::Typing(report),
    )
}

fn header_problem(owner: SourceId, error: ClassHeaderError) -> (SourceId, Problem) {
    match error {
        ClassHeaderError::Typing(report) => typing_problem(owner, *report),
        invalid @ ClassHeaderError::InvalidClass(_) => (owner, Problem::Header(invalid)),
    }
}

pub struct Link {
    pub origin: TextRange,
    pub uri: String,
    pub declaration: TextRange,
    pub name: TextRange,
}

#[derive(Default)]
pub struct FileResult {
    pub problems: Vec<Problem>,
    pub links: Vec<Link>,
}

fn name(
    text: &str,
    source: SourceId,
    filename: &str,
    store: &mut SemanticStore,
    packages: &mut Packages,
    result: &mut FileResult,
) -> Option<NamedClassSource> {
    let parsed = dotty::parse_source(source, text, Default::default())
        .map_err(|error| result.problems.push(Problem::Scan(error)))
        .ok()?
        .bind(&mut store.names);
    let (file, diagnostics) = parsed
        .prepare_classes(filename, store, packages)
        .into_parts();
    result
        .problems
        .extend(diagnostics.into_iter().map(Problem::Preparation));
    file
}

// Seed every eligible declaration once. Subsequent attempts come from completed providers.
fn complete<E>(
    dependencies: &mut SignatureDependencies,
    roots: &[SymbolId],
    mut attempt: impl FnMut(SymbolId) -> Result<SignatureCompletion, E>,
    mut report: impl FnMut(SymbolId, E),
) {
    let mut ready: VecDeque<_> = roots.iter().copied().collect();
    while let Some(symbol) = ready.pop_front() {
        match attempt(symbol) {
            Ok(outcome) => {
                ready.extend(dependencies.record(symbol, outcome));
            }
            Err(error) => {
                dependencies.record_failure(symbol);
                report(symbol, error);
            }
        }
    }
}

// Report one marker at each participant. A downstream waiter already has a provider report.
fn report_blockages(
    dependencies: &SignatureDependencies,
    roots: &[SymbolId],
    declarations: &SourceDeclarationIndex,
    store: &SemanticStore,
    results: &mut HashMap<SourceId, FileResult>,
) {
    for (symbol, cause) in dependencies.blockages(roots.iter().copied()) {
        match cause {
            SignatureBlockage::CycleMember(cycle) if cycle.path()[0] == symbol => {
                for (member, location) in cycle.declarations(store) {
                    let file = declarations.get(member).unwrap().source;
                    results
                        .get_mut(&file)
                        .unwrap()
                        .problems
                        .push(Problem::Cycle {
                            cycle: cycle.clone(),
                            location,
                        });
                }
            }
            SignatureBlockage::Pending(provider) => {
                let file = declarations.get(symbol).unwrap().source;
                results
                    .get_mut(&file)
                    .unwrap()
                    .problems
                    .push(Problem::Pending {
                        provider,
                        name: store
                            .names
                            .resolve(store.symbols.get(provider).name.text())
                            .to_owned(),
                        location: store.symbols.get(symbol).position,
                    });
            }
            SignatureBlockage::CycleMember(_)
            | SignatureBlockage::DependsOnCycle(_)
            | SignatureBlockage::Failed(_) => {}
        }
    }
}

pub fn analyze(docs: &Documents) -> BTreeMap<String, FileResult> {
    let mut store = SemanticStore::new();
    let defs = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let mut uris = HashMap::new();
    let mut results = HashMap::new();
    let mut units = Vec::new();
    for (i, (uri, doc)) in docs.iter().enumerate() {
        let source = SourceId::from_index(i as u32);
        uris.insert(source, uri.as_str());
        if let Some(unit) = name(
            doc.text.as_str(),
            source,
            uri.rsplit('/').next().unwrap_or(uri),
            &mut store,
            &mut packages,
            results.entry(source).or_default(),
        ) {
            units.push(unit);
        }
    }
    match NamedClassSources::new(&units, &store) {
        Ok(sources) => analyze_sources(&sources, &mut store, defs, &packages, &uris, &mut results),
        Err(error) => {
            for result in results.values_mut() {
                result.problems.push(Problem::Sources(error));
            }
        }
    }
    results
        .into_iter()
        .map(|(id, file)| (uris[&id].to_owned(), file))
        .collect()
}

fn analyze_sources(
    sources: &NamedClassSources<'_>,
    store: &mut SemanticStore,
    defs: Definitions,
    packages: &Packages,
    uris: &HashMap<SourceId, &str>,
    results: &mut HashMap<SourceId, FileResult>,
) {
    let mut arena = AstArena::<Typed>::new();
    let mut bindings = ClassHeaderBindings::default();
    let mut unavailable = HashSet::new();
    let mut classes = Vec::new();
    let mut headers = SignatureDependencies::new(SignaturePhase::Headers);
    let ast_for = |id: SourceId| sources.file(id).map(NamedClassSource::ast);
    for (file, class) in sources.classes() {
        match sources.complete_constructor_signature(class.symbol, store, defs, packages) {
            Ok(_) => classes.push(class.symbol),
            Err(error) => {
                headers.record_failure(class.symbol);
                unavailable.insert(class.symbol);
                let (source, problem) = header_problem(file.source(), error);
                results.get_mut(&source).unwrap().problems.push(problem);
            }
        }
    }
    complete(
        &mut headers,
        &classes,
        |symbol| {
            sources.try_complete_class_header_reported(
                symbol,
                store,
                defs,
                packages,
                &mut arena,
                &mut bindings,
            )
        },
        |symbol, error| {
            let source = sources.declarations().get(symbol).unwrap().source;
            unavailable.insert(symbol);
            let (file, problem) = header_problem(source, error);
            results.get_mut(&file).unwrap().problems.push(problem);
        },
    );
    unavailable.extend(
        classes
            .iter()
            .copied()
            .filter(|&s| headers.waiting_on(s).is_some()),
    );
    report_blockages(&headers, &classes, sources.declarations(), store, results);
    let mut members = Vec::new();
    for (_, class) in sources.classes() {
        if unavailable.contains(&class.symbol) {
            continue;
        }
        for member in &class.members {
            let (ClassMember::Field(d) | ClassMember::Method(d)) = member;
            members.push(d.symbol);
        }
    }
    let mut signature_calls = HashSet::new();
    let mut signatures = SignatureDependencies::new(SignaturePhase::Members);
    complete(
        &mut signatures,
        &members,
        |s| sources.try_complete_member_signature(s, store, defs, packages),
        |s, error| {
            let source = sources.declarations().get(s).unwrap().source;
            let (file, problem) = match error.into_reported_error(ast_for) {
                Ok(report) => {
                    if let Some(key) = report.call_identity(&defs) {
                        signature_calls.insert(key);
                    }
                    typing_problem(source, report)
                }
                Err(invalid) => (source, Problem::MemberSignature(invalid)),
            };
            results.get_mut(&file).unwrap().problems.push(problem);
        },
    );
    report_blockages(
        &signatures,
        &members,
        sources.declarations(),
        store,
        results,
    );
    let mut bodies = HashMap::new();
    for (file, c) in sources.classes() {
        let source = file.source();
        if unavailable.contains(&c.symbol) {
            continue;
        }
        let (body, report) = sources
            .check_class_body(c.symbol, store, defs, packages, &mut arena, NoResolver)
            .into_parts();
        for (source, diagnostic) in report.diagnostics {
            results
                .get_mut(&source)
                .unwrap()
                .problems
                .push(Problem::TyperWarning { source, diagnostic });
        }
        for error in report.errors {
            if let ClassBodyError::Member {
                member: ClassMember::Method(d) | ClassMember::Field(d),
                ..
            } = &error
                && signatures.waiting_on(d.symbol).is_some()
            {
                continue;
            }
            let (file, problem) = match error.into_reported_error(ast_for) {
                Ok(report) => {
                    if report
                        .call_identity(&defs)
                        .is_some_and(|key| signature_calls.contains(&key))
                    {
                        continue;
                    }
                    typing_problem(source, report)
                }
                Err(invalid) => (source, Problem::Body(invalid)),
            };
            results.get_mut(&file).unwrap().problems.push(problem);
        }
        bodies.insert(c.symbol, body);
    }
    for u in sources.files() {
        let source = u.source();
        let calls = match u.navigation() {
            Ok(calls) => calls,
            Err(error) => {
                results
                    .get_mut(&source)
                    .unwrap()
                    .problems
                    .push(Problem::Preparation(PreparationDiagnostic::Declaration(
                        error.diagnostic(u.ast()),
                    )));
                continue;
            }
        };
        for (origin, _) in calls.call_names().iter() {
            if let Some(t) = calls.definition_at_with_bindings(
                origin.start(),
                u.ast(),
                &arena,
                store,
                |s| bodies.get(&s)?.bindings(),
                |id| sources.file(id).and_then(|x| x.navigation().ok()),
            ) {
                results.get_mut(&source).unwrap().links.push(Link {
                    origin,
                    uri: uris[&t.source].to_owned(),
                    declaration: t.declaration,
                    name: t.name,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dotty::typer::TyperError;
    use dotty::{SyntaxDiagnostic, source_declarations::DeclarationDiagnostic};

    #[test]
    fn naming_and_projection_rejections_remain_typed_until_publication() {
        let files = [
            ("A.scala", "class A { private[Missing] val x = 1 }"),
            ("T.scala", "/* 😀 */ trait T"),
        ]
        .into_iter()
        .map(|(uri, text)| {
            (
                uri.to_owned(),
                Document {
                    version: 1,
                    text: Arc::new(DocumentText::new(text.into()).unwrap()),
                },
            )
        })
        .collect();
        let results = analyze(&files);
        let [Problem::Preparation(PreparationDiagnostic::Naming(d))] =
            results["A.scala"].problems.as_slice()
        else {
            panic!("expected a namer diagnostic")
        };
        assert!(
            matches!(d.issue(),dotty::namer::NamerIssue::InvalidVisibilityQualifier {qualifier,..} if qualifier == "Missing")
        );
        let [
            Problem::Preparation(PreparationDiagnostic::Declaration(DeclarationDiagnostic {
                error: dotty::source_declarations::DeclarationError::Unsupported { source, .. },
                location: Some(span),
            })),
        ] = results["T.scala"].problems.as_slice()
        else {
            panic!("expected a located projection failure")
        };
        assert_eq!(*source, span.source());
        let range = span.span().range();
        assert_eq!(
            &files["T.scala"].text.as_str()[range.start() as usize..range.end() as usize],
            "trait T"
        );
    }

    #[test]
    fn downstream_fields_share_the_provider_cause_while_siblings_are_checked() {
        let consumer = "class Consumer(p: Provider) { val answer = p.seed; def value(): Int = 1; def bad(): Int = value(1) }";
        let downstream = "class Leaf(c: Consumer) { val copy = c.answer }";
        let check = |provider: &str| {
            let docs = [
                ("Consumer.scala", consumer),
                ("Provider.scala", provider),
                ("Leaf.scala", downstream),
            ]
            .into_iter()
            .map(|(uri, text)| {
                (
                    uri.to_owned(),
                    Document {
                        version: 1,
                        text: Arc::new(DocumentText::new(text.into()).unwrap()),
                    },
                )
            })
            .collect();
            analyze(&docs)
        };
        let cycle = check("class Provider(c: Consumer) { val seed = c.answer }");
        assert!(cycle["Leaf.scala"].problems.is_empty());
        let [
            Problem::Cycle { cycle: a, .. },
            Problem::Typing(independent),
        ] = cycle["Consumer.scala"].problems.as_slice()
        else {
            panic!("expected one participant and one independent call")
        };
        let [Problem::Cycle { cycle: b, .. }] = cycle["Provider.scala"].problems.as_slice() else {
            panic!("expected the other participant")
        };
        assert!(Arc::ptr_eq(a, b));
        assert_eq!(a.code(), "signature.member_cycle");
        assert_eq!(independent.code(), Some("typer.application_arity_mismatch"));

        let failed = check("class Provider { def value(): Int = 1; val seed = value(1) }");
        assert!(failed["Leaf.scala"].problems.is_empty());
        for file in ["Consumer.scala", "Provider.scala"] {
            let [Problem::Typing(call)] = failed[file].problems.as_slice() else {
                panic!("expected only the original call in {file}")
            };
            assert_eq!(call.code(), Some("typer.application_arity_mismatch"));
        }
    }

    #[test]
    fn analysis_retains_the_committed_warning_payload() {
        let uri = "file:///C.scala";
        let text = "class C { def f(implicit x: Int): Int = x; def use(): Int = f(1) }";
        let doc = Document {
            version: 1,
            text: Arc::new(DocumentText::new(text.into()).unwrap()),
        };
        let files = analyze(&[(uri.into(), doc)].into());
        let [Problem::TyperWarning { source, diagnostic }] = files[uri].problems.as_slice() else {
            panic!("expected one typed warning");
        };
        assert_eq!(*source, SourceId::from_index(0));
        assert_eq!(
            diagnostic.issue(),
            &dotty::typer::TyperWarning::ImplicitClauseWithoutUsing
        );
    }

    #[test]
    fn syntax_issues_survive_analysis_until_publication() {
        use dotty::lexer::{LexerIssue, NumericComponent, NumericIssue};
        use dotty::parser::{DeclarationIssue, ParseIssue};

        let uri = "file:///Broken.scala";
        let doc = Document {
            version: 1,
            text: Arc::new(DocumentText::new("class Broken { val x: Int 1e+ }".into()).unwrap()),
        };
        let files = analyze(&[(uri.to_owned(), doc)].into());
        let [
            Problem::Preparation(PreparationDiagnostic::Syntax(SyntaxDiagnostic::Lexer {
                diagnostic,
                ..
            })),
            Problem::Preparation(PreparationDiagnostic::Syntax(SyntaxDiagnostic::Parser(parser))),
        ] = files[uri].problems.as_slice()
        else {
            panic!("expected both phases")
        };
        assert!(matches!(
            diagnostic.issue(),
            LexerIssue::Numeric(NumericIssue::MissingDigits {
                component: NumericComponent::Exponent
            })
        ));
        assert!(matches!(
            parser.issue(),
            ParseIssue::Declaration(DeclarationIssue::ExpectedValueEquals { .. })
        ));
    }

    #[test]
    fn unknown_constructor_parameter_type_points_to_its_name() {
        let uri = "file:///Broken.scala";
        let text = "class Broken(start: Missing)";
        let doc = Document {
            version: 1,
            text: Arc::new(DocumentText::new(text.into()).unwrap()),
        };
        let files = analyze(&[(uri.to_owned(), doc)].into());
        let [Problem::Typing(problem)] = files[uri].problems.as_slice() else {
            panic!("expected one constructor diagnostic");
        };
        let range = problem
            .location()
            .expect("position of the unknown type")
            .span()
            .range();
        assert_eq!(
            &text[range.start() as usize..range.end() as usize],
            "Missing"
        );
        assert!(matches!(
            problem.error(),
            TyperError::TypeNameNotFound { .. }
        ));
        assert!(problem.class().is_some() && problem.member().is_some());
        assert_eq!(problem.code(), Some("typer.type_name_not_found"));
    }

    #[test]
    fn repeated_signature_failure_preserves_an_independent_body_error() {
        let uri = "file:///Consumer.scala";
        let text = "class Consumer { def value(): Int = 40; val answer = value(1); def next(): Int = value(2) }";
        let doc = Document {
            version: 1,
            text: Arc::new(DocumentText::new(text.into()).unwrap()),
        };
        let files = analyze(&[(uri.to_owned(), doc)].into());
        let problems = &files[uri].problems;
        assert_eq!(problems.len(), 2);
        for (problem, call) in problems.iter().zip(["value(1)", "value(2)"]) {
            let Problem::Typing(problem) = problem else {
                panic!("expected an error")
            };
            let range = problem.location().unwrap().span().range();
            assert_eq!(&text[range.start() as usize..range.end() as usize], call);
            assert!(matches!(
                problem.error(),
                TyperError::ApplicationArityMismatch {
                    expected: 0,
                    actual: 1,
                    ..
                }
            ));
        }
    }

    #[test]
    fn valid_method_navigates_while_sibling_reports_its_argument() {
        let a = "class A { def take(n: Int): Int = n; def take(b: Boolean): Int = 7 }";
        let b = "class B { def good(a: A): Int = a.take(false); def bad(x: Int): Int = good(x) }";
        let docs = [("file:///B.scala", b), ("file:///A.scala", a)]
            .into_iter()
            .map(|(uri, text)| {
                (
                    uri.to_owned(),
                    Document {
                        version: 1,
                        text: Arc::new(DocumentText::new(text.into()).unwrap()),
                    },
                )
            })
            .collect();
        let files = analyze(&docs);
        let caller = &files["file:///B.scala"];
        assert_eq!(caller.problems.len(), 1);
        let Problem::Typing(d) = &caller.problems[0] else {
            panic!("expected a type error")
        };
        let range = d.location().expect("argument position").span().range();
        assert_eq!(&b[range.start() as usize..range.end() as usize], "x");
        assert_eq!(caller.links.len(), 1);
        let l = &caller.links[0];
        assert_eq!(l.uri, "file:///A.scala");
        assert_eq!(l.name.start(), a.rfind("take").unwrap() as u32);
    }
}
