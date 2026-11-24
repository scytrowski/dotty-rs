//! Fresh semantic session for one set of open document versions.
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::Arc;

use dotty::classloader::{ClasspathSymbolResolver, CompositeClassPath, LoadingSession};
use dotty::core::{
    AstArena, Definitions, DocumentText, NoResolver, Packages, SemanticStore, SourceId, SourceSpan,
    SymbolId, SymbolResolver, Typed,
};
use dotty::{
    CheckedClassBodies, ClassHeaderBindings, ClassHeaderPhase, MemberSignaturePhase,
    NamedClassSource, NamedClassSources, NamedClassSourcesError, PreparationDiagnostic,
    ResolvedMemberIndex, SemanticDiagnostic, SignatureBlockage, SignatureBlockageReport,
    SignatureCycle,
};

#[derive(Clone)]
pub struct Document {
    pub version: i32,
    pub text: Arc<DocumentText>,
}

pub type Documents = BTreeMap<String, Document>;

pub enum Problem {
    Semantic(SemanticDiagnostic),
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
}

fn record_semantic(results: &mut HashMap<SourceId, FileResult>, diagnostic: SemanticDiagnostic) {
    let source = diagnostic
        .source()
        .expect("only indexed declarations were scheduled");
    results
        .get_mut(&source)
        .expect("open source document")
        .problems
        .push(Problem::Semantic(diagnostic));
}

#[derive(Default)]
pub struct FileResult {
    pub problems: Vec<Problem>,
}

#[derive(Default)]
pub struct Analysis {
    pub files: BTreeMap<String, FileResult>,
    pub members: ResolvedMemberIndex,
    // Source IDs follow the open documents' URI order in this revision.
    uris: Vec<String>,
}

impl Analysis {
    pub fn uri(&self, source: SourceId) -> Option<&str> {
        self.uris.get(source.index() as usize).map(String::as_str)
    }

    pub fn source(&self, uri: &str) -> Option<SourceId> {
        let i = self
            .uris
            .binary_search_by(|candidate| candidate.as_str().cmp(uri))
            .ok()?;
        Some(SourceId::from_index(i as u32))
    }
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

// Report one marker at each participant. A downstream waiter already has a provider report.
fn report_blockages(report: SignatureBlockageReport, results: &mut HashMap<SourceId, FileResult>) {
    for (cycle, participants) in report.cycles() {
        for participant in participants {
            let source = participant
                .declaration
                .expect("indexed cycle participant")
                .source;
            results
                .get_mut(&source)
                .expect("open source document")
                .problems
                .push(Problem::Cycle {
                    cycle: Arc::clone(cycle),
                    location: participant.location,
                });
        }
    }
    for (waiter, cause) in report.waiters() {
        let SignatureBlockage::Pending(provider) = cause else {
            continue;
        };
        let source = waiter.declaration.expect("indexed waiter").source;
        let name = report
            .declaration(*provider)
            .and_then(|d| d.name.as_deref())
            .expect("provider in selected source files");
        results
            .get_mut(&source)
            .expect("open source document")
            .problems
            .push(Problem::Pending {
                provider: *provider,
                name: name.to_owned(),
                location: waiter.location,
            });
    }
}

pub fn analyze(docs: &Documents) -> Analysis {
    analyze_with_classpath(docs, None)
}

pub fn analyze_with_classpath(
    docs: &Documents,
    classpath: Option<&CompositeClassPath>,
) -> Analysis {
    let mut store = SemanticStore::new();
    let defs = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let mut uris = Vec::with_capacity(docs.len());
    let mut results = HashMap::new();
    let mut units = Vec::new();
    for (i, (uri, doc)) in docs.iter().enumerate() {
        let source = SourceId::from_index(i as u32);
        uris.push(uri.clone());
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
    let members = match NamedClassSources::new(&units, &store) {
        Ok(sources) => {
            let mut external = classpath.map(|path| {
                ClasspathSymbolResolver::new(
                    path,
                    defs,
                    LoadingSession::with_packages(packages.snapshot()),
                )
            });
            let resolver: &mut dyn SymbolResolver = match &mut external {
                Some(resolver) => resolver,
                None => &mut NoResolver,
            };
            analyze_sources(
                &sources,
                &mut store,
                defs,
                &packages,
                &mut results,
                resolver,
            )
        }
        Err(error) => {
            for result in results.values_mut() {
                result.problems.push(Problem::Sources(error));
            }
            ResolvedMemberIndex::default()
        }
    };
    let files = results
        .into_iter()
        .map(|(id, file)| (uris[id.index() as usize].clone(), file))
        .collect();
    Analysis {
        files,
        members,
        uris,
    }
}

fn analyze_sources(
    sources: &NamedClassSources<'_>,
    store: &mut SemanticStore,
    defs: Definitions,
    packages: &Packages,
    results: &mut HashMap<SourceId, FileResult>,
    resolver: &mut dyn SymbolResolver,
) -> ResolvedMemberIndex {
    let mut arena = AstArena::<Typed>::new();
    let mut bindings = ClassHeaderBindings::default();
    let mut phase = ClassHeaderPhase::prepare_with_constructor_resolver(
        sources,
        store,
        defs,
        packages,
        &mut arena,
        &mut bindings,
        &mut *resolver,
    );
    let mut queue: VecDeque<_> = sources.classes().map(|(_, class)| class.symbol).collect();
    while let Some(symbol) = queue.pop_front() {
        if let Ok(attempt) = phase.attempt(symbol) {
            queue.extend(attempt.retry);
        }
    }
    let headers = phase.finish();
    let blocked = headers.blockage_report(sources.declarations(), store);
    for failure in headers.failures {
        record_semantic(results, failure.into_semantic_diagnostic());
    }
    report_blockages(blocked, results);
    let selection = sources
        .member_signatures(headers.ready.iter().copied(), store)
        .expect("classes from this session's header report");
    let mut signatures = MemberSignaturePhase::new(&selection, store, defs, packages);
    queue.extend(selection.members().iter().copied());
    while let Some(member) = queue.pop_front() {
        if let Ok(attempt) = signatures.attempt_with_resolver(member, &mut *resolver) {
            queue.extend(attempt.retry);
        }
    }
    let (signatures, work) = signatures.finish_with_body_work();
    let blocked = signatures.blockage_report(sources.declarations(), store);
    for failure in signatures.failures {
        record_semantic(results, failure.into_semantic_diagnostic(sources));
    }
    report_blockages(blocked, results);
    let mut bodies = CheckedClassBodies::default();
    for symbol in headers.ready {
        let (body, report) = work.check(symbol, store, defs, packages, &mut arena, &mut *resolver);
        for diagnostic in report.into_semantic_diagnostics(sources) {
            record_semantic(results, diagnostic);
        }
        bodies.record(body).expect("one check per ready class");
    }
    let index = ResolvedMemberIndex::build_with_headers(sources, &bodies, &bindings, &arena, store);
    for diagnostic in index.diagnostics() {
        results
            .get_mut(&diagnostic.error.source_id())
            .expect("open source document")
            .problems
            .push(Problem::Preparation(PreparationDiagnostic::Declaration(
                *diagnostic,
            )));
    }
    index
}

#[cfg(test)]
mod tests {
    use super::*;
    use dotty::SemanticIssue;
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
            results.files["A.scala"].problems.as_slice()
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
        ] = results.files["T.scala"].problems.as_slice()
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
        assert!(cycle.files["Leaf.scala"].problems.is_empty());
        let [
            Problem::Cycle { cycle: a, .. },
            Problem::Semantic(SemanticDiagnostic {
                issue: SemanticIssue::Typing(independent),
                ..
            }),
        ] = cycle.files["Consumer.scala"].problems.as_slice()
        else {
            panic!("expected one participant and one independent call")
        };
        let [Problem::Cycle { cycle: b, .. }] = cycle.files["Provider.scala"].problems.as_slice()
        else {
            panic!("expected the other participant")
        };
        assert!(Arc::ptr_eq(a, b));
        assert_eq!(a.code(), "signature.member_cycle");
        assert_eq!(independent.code(), Some("typer.application_arity_mismatch"));

        let failed = check("class Provider { def value(): Int = 1; val seed = value(1) }");
        assert!(failed.files["Leaf.scala"].problems.is_empty());
        for file in ["Consumer.scala", "Provider.scala"] {
            let [
                Problem::Semantic(SemanticDiagnostic {
                    issue: SemanticIssue::Typing(call),
                    ..
                }),
            ] = failed.files[file].problems.as_slice()
            else {
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
        let [
            Problem::Semantic(SemanticDiagnostic {
                issue: SemanticIssue::Warning { source, diagnostic },
                phase: dotty::SemanticPhase::ClassBody,
                ..
            }),
        ] = files.files[uri].problems.as_slice()
        else {
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
        ] = files.files[uri].problems.as_slice()
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
    fn constructor_failure_keeps_an_independent_class_navigable() {
        let text = "class Bad(x: Missing) { def only(): Int = 1; val broken = only(false) }; class Good { def get() = 7; def use() = get() }";
        let uri = "Good.scala";
        let doc = Document {
            version: 1,
            text: Arc::new(DocumentText::new(text.into()).unwrap()),
        };
        let results = analyze(&[(uri.to_owned(), doc)].into());
        let file = &results.files[uri];
        let [
            Problem::Semantic(SemanticDiagnostic {
                issue: SemanticIssue::Typing(error),
                ..
            }),
        ] = file.problems.as_slice()
        else {
            panic!("expected only the Bad constructor error");
        };
        assert!(matches!(error.error(), TyperError::TypeNameNotFound { .. }));
        let source = results.source(uri).unwrap();
        let start = text.rfind("get").unwrap() as u32;
        let call = results.members.call_at(source, start).unwrap();
        assert_eq!(call.target.source, source);
        assert_eq!(call.origin.span().range().start(), start);
        assert_eq!(call.target.name.start() as usize, text.find("get").unwrap());
        assert!(
            results
                .members
                .call_at(source, text.rfind("only").unwrap() as u32)
                .is_none()
        );
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
        let [
            Problem::Semantic(SemanticDiagnostic {
                issue: SemanticIssue::Typing(problem),
                ..
            }),
        ] = files.files[uri].problems.as_slice()
        else {
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
        let problems = &files.files[uri].problems;
        assert_eq!(problems.len(), 2);
        for (problem, call) in problems.iter().zip(["value(1)", "value(2)"]) {
            let Problem::Semantic(SemanticDiagnostic {
                issue: SemanticIssue::Typing(problem),
                ..
            }) = problem
            else {
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
        let caller = &files.files["file:///B.scala"];
        assert_eq!(caller.problems.len(), 1);
        let Problem::Semantic(SemanticDiagnostic {
            issue: SemanticIssue::Typing(d),
            ..
        }) = &caller.problems[0]
        else {
            panic!("expected a type error")
        };
        let range = d.location().expect("argument position").span().range();
        assert_eq!(&b[range.start() as usize..range.end() as usize], "x");
        let source = files.source("file:///B.scala").unwrap();
        let call = files
            .members
            .call_at(source, b.find("take").unwrap() as u32)
            .unwrap();
        assert_eq!(files.uri(call.target.source), Some("file:///A.scala"));
        assert_eq!(call.target.name.start(), a.rfind("take").unwrap() as u32);
        assert!(
            files
                .members
                .call_at(source, b.rfind("good").unwrap() as u32)
                .is_none()
        );
    }
}
