//! Fresh semantic session for one set of open document versions.
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::sync::Arc;

use dotty::core::{
    AstArena, Definitions, DiagnosticSeverity, DocumentText, NoResolver, Packages, SemanticStore,
    SourceId, SourceSemanticIndex, SourceSpan, SymbolId, TextRange, Typed, Untyped,
};
use dotty::jvm_class_plan::ClassHeaderBindings;
use dotty::namer::{NamingOutcome, SourceParseStatus, name_compilation_unit_with_diagnostics};
use dotty::source_declarations::{ClassMember, SourceClass, source_classes};
use dotty::typer::{SignatureCompletion, SourceTyper, TyperDiagnostic, TyperError};
use dotty::{
    CallDiagnostic, ClassBodyError, SignatureBlockage, SignatureCycle, SignatureDependencies,
    SignaturePhase, SourceDeclarationIndex, SourceDiagnostic, SourceFileCalls, SyntaxDiagnostic,
};

#[derive(Clone)]
pub struct Document {
    pub version: i32,
    pub text: Arc<DocumentText>,
}

pub type Documents = BTreeMap<String, Document>;

pub enum Problem {
    Call(CallDiagnostic),
    Cycle {
        cycle: Arc<SignatureCycle>,
        location: Option<SourceSpan>,
    },
    Pending {
        provider: SymbolId,
        name: String,
        location: Option<SourceSpan>,
    },
    Reported(SourceDiagnostic),
    TyperWarning {
        source: SourceId,
        diagnostic: TyperDiagnostic,
    },
}

impl From<SourceDiagnostic> for Problem {
    fn from(diagnostic: SourceDiagnostic) -> Self {
        Self::Reported(diagnostic)
    }
}

fn typing_problem(owner_file: usize, diagnostic: SourceDiagnostic) -> (usize, Problem) {
    let file = diagnostic
        .location
        .map_or(owner_file, |span| span.source().index() as usize);
    (file, diagnostic.into())
}

fn call_problem<E>(
    owner_file: usize,
    result: Result<CallDiagnostic, E>,
    fallback: impl FnOnce(E) -> SourceDiagnostic,
) -> (usize, Problem) {
    match result {
        Ok(call) => (call.source().index() as usize, Problem::Call(call)),
        Err(error) => typing_problem(owner_file, fallback(error)),
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
    pub syntax_diagnostics: Vec<SyntaxDiagnostic>,
    pub problems: Vec<Problem>,
    pub links: Vec<Link>,
}

impl FileResult {
    fn error(&mut self, location: Option<SourceSpan>, message: String) {
        self.problems.push(Problem::Reported(SourceDiagnostic {
            code: None,
            location,
            message,
            severity: DiagnosticSeverity::Error,
        }));
    }
}

struct Unit {
    source: SourceId,
    ast: AstArena<Untyped>,
    index: SourceSemanticIndex,
    classes: Vec<SourceClass>,
    calls: SourceFileCalls,
}

fn name(
    text: &str,
    source: SourceId,
    filename: &str,
    store: &mut SemanticStore,
    packages: &mut Packages,
    result: &mut FileResult,
) -> Option<Unit> {
    let parsed = dotty::parse_source(source, text, Default::default())
        .map_err(|e| result.error(None, e.to_string()))
        .ok()?
        .bind(&mut store.names);
    if !parsed.diagnostics.is_empty() {
        result.syntax_diagnostics = parsed.diagnostics;
        return None;
    }
    let parsed = parsed.syntax;
    let named = name_compilation_unit_with_diagnostics(
        &parsed.ast,
        parsed.root,
        source,
        filename,
        SourceParseStatus::Clean,
        store,
        packages,
    );
    let (index, diagnostics) = match named {
        Ok(NamingOutcome::Named { index, diagnostics }) => (Some(index), diagnostics),
        Ok(NamingOutcome::Rejected { diagnostics }) => (None, diagnostics),
        Err(error) => {
            result.error(None, error.to_string());
            return None;
        }
    };
    result.problems.extend(
        diagnostics
            .iter()
            .map(|d| SourceDiagnostic::from_namer(d).into()),
    );
    let index = *index?;
    let classes = source_classes(&parsed.ast, parsed.root, source, &index, store)
        .map_err(|e| result.error(None, e.to_string()))
        .ok()?;
    let calls = SourceFileCalls::new(parsed.call_names, &parsed.ast, &index, &classes)
        .map_err(|e| result.error(None, e.to_string()))
        .ok()?;
    Some(Unit {
        source,
        ast: parsed.ast,
        index,
        classes,
        calls,
    })
}

// Seed every eligible declaration once. Subsequent attempts come from completed providers.
fn complete<E>(
    dependencies: &mut SignatureDependencies,
    roots: &[(usize, SymbolId)],
    mut attempt: impl FnMut(usize, SymbolId) -> Result<SignatureCompletion, E>,
    mut report: impl FnMut(usize, SymbolId, E),
) {
    let files: HashMap<_, _> = roots.iter().map(|&(file, symbol)| (symbol, file)).collect();
    let mut ready: VecDeque<_> = roots.iter().copied().collect();
    while let Some((file, symbol)) = ready.pop_front() {
        match attempt(file, symbol) {
            Ok(outcome) => {
                for waiter in dependencies.record(symbol, outcome) {
                    ready.push_back((files[&waiter], waiter));
                }
            }
            Err(error) => {
                dependencies.record_failure(symbol);
                report(file, symbol, error);
            }
        }
    }
}

// Report one marker at each participant. A downstream waiter already has a provider report.
fn report_blockages(
    dependencies: &SignatureDependencies,
    roots: &[(usize, SymbolId)],
    declarations: &SourceDeclarationIndex,
    store: &SemanticStore,
    results: &mut [FileResult],
) {
    for (symbol, cause) in dependencies.blockages(roots.iter().map(|&(_, s)| s)) {
        match cause {
            SignatureBlockage::CycleMember(cycle) if cycle.path()[0] == symbol => {
                for (member, location) in cycle.declarations(store) {
                    let file = declarations.get(member).unwrap().source.index() as usize;
                    results[file].problems.push(Problem::Cycle {
                        cycle: cycle.clone(),
                        location,
                    });
                }
            }
            SignatureBlockage::Pending(provider) => {
                let file = declarations.get(symbol).unwrap().source.index() as usize;
                results[file].problems.push(Problem::Pending {
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
    let mut results: Vec<_> = docs.keys().map(|_| FileResult::default()).collect();
    let mut store = SemanticStore::new();
    let defs = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let uris: Vec<_> = docs.keys().cloned().collect();
    let mut units = BTreeMap::new();
    for (i, (uri, doc)) in docs.iter().enumerate() {
        if let Some(unit) = name(
            doc.text.as_str(),
            SourceId::from_index(i as u32),
            uri.rsplit('/').next().unwrap_or(uri),
            &mut store,
            &mut packages,
            &mut results[i],
        ) {
            units.insert(i, unit);
        }
    }
    let declarations =
        match SourceDeclarationIndex::from_classes(&store, units.values().flat_map(|u| &u.classes))
        {
            Ok(d) => d,
            Err(e) => {
                for f in &mut results {
                    f.error(None, e.to_string());
                }
                return uris.into_iter().zip(results).collect();
            }
        };
    let mut arena = AstArena::<Typed>::new();
    let mut bindings = ClassHeaderBindings::default();
    let mut unavailable = HashSet::new();
    let mut classes = Vec::new();
    let mut headers = SignatureDependencies::new(SignaturePhase::Headers);
    let ast_for = |id: SourceId| units.get(&(id.index() as usize)).map(|unit| &unit.ast);
    let typer_problem = |file, error: TyperError| {
        call_problem(
            file,
            CallDiagnostic::from_typer(error, None, ast_for),
            |error| SourceDiagnostic::from_typer(&error, ast_for),
        )
    };
    for (&i, u) in &units {
        let mut typer = SourceTyper::new(&u.ast, u.source, &u.index, &mut store, defs, &packages);
        for c in &u.classes {
            match typer.complete_symbol(c.constructor) {
                Ok(_) => classes.push((i, c.symbol)),
                Err(e) => {
                    headers.record_failure(c.symbol);
                    unavailable.insert(c.symbol);
                    let (file, problem) = typer_problem(i, e);
                    results[file].problems.push(problem);
                }
            }
        }
    }
    complete(
        &mut headers,
        &classes,
        |i, symbol| {
            let u = &units[&i];
            dotty::try_complete_class_header(
                symbol,
                &u.ast,
                &u.index,
                &mut store,
                defs,
                &packages,
                &mut arena,
                &mut bindings,
                |id| declarations.get(id).is_some(),
            )
        },
        |i, symbol, error| {
            unavailable.insert(symbol);
            let (file, problem) = typer_problem(i, error);
            results[file].problems.push(problem);
        },
    );
    unavailable.extend(
        classes
            .iter()
            .map(|&(_, s)| s)
            .filter(|&s| headers.waiting_on(s).is_some()),
    );
    report_blockages(&headers, &classes, &declarations, &store, &mut results);
    let mut members = Vec::new();
    for (&i, u) in &units {
        for c in &u.classes {
            if unavailable.contains(&c.symbol) {
                continue;
            }
            for member in &c.members {
                let d = match member {
                    ClassMember::Field(d) | ClassMember::Method(d) => d,
                };
                members.push((i, d.symbol));
            }
        }
    }
    let mut signature_calls = HashSet::new();
    let mut signatures = SignatureDependencies::new(SignaturePhase::Members);
    complete(
        &mut signatures,
        &members,
        |i, s| {
            let u = &units[&i];
            dotty::try_complete_member_signature(
                s,
                &declarations,
                &u.ast,
                &u.index,
                &mut store,
                defs,
                &packages,
            )
        },
        |i, _, error| {
            let (file, problem) = call_problem(i, error.into_call_diagnostic(ast_for), |e| {
                e.diagnostic(ast_for)
            });
            if let Problem::Call(call) = &problem
                && let Some(key) = call.identity(&defs)
            {
                signature_calls.insert(key);
            }
            results[file].problems.push(problem);
        },
    );
    report_blockages(&signatures, &members, &declarations, &store, &mut results);
    let mut checks = HashMap::new();
    for (&i, u) in &units {
        for c in &u.classes {
            if unavailable.contains(&c.symbol) {
                continue;
            }
            let mut check = dotty::check_class_body(
                c, &u.ast, &u.index, &mut store, defs, &packages, &mut arena, NoResolver,
            );
            for (source, diagnostic) in check.diagnostics.drain(..) {
                results[source.index() as usize]
                    .problems
                    .push(Problem::TyperWarning { source, diagnostic });
            }
            if let Err(errors) = &mut check.body {
                for error in errors.drain(..) {
                    if let ClassBodyError::Member {
                        member: ClassMember::Method(d) | ClassMember::Field(d),
                        ..
                    } = &error
                        && signatures.waiting_on(d.symbol).is_some()
                    {
                        continue;
                    }
                    let (file, problem) =
                        call_problem(i, error.into_call_diagnostic(ast_for), |e| {
                            e.diagnostic(ast_for)
                        });
                    if let Problem::Call(call) = &problem
                        && call
                            .identity(&defs)
                            .is_some_and(|key| signature_calls.contains(&key))
                    {
                        continue;
                    }
                    results[file].problems.push(problem);
                }
            }
            checks.insert(c.symbol, check);
        }
    }
    for (&i, u) in &units {
        for (origin, _) in u.calls.call_names().iter() {
            if let Some(t) = u.calls.definition_at(
                origin.start(),
                &u.ast,
                &arena,
                &store,
                |s| checks.get(&s),
                |id| units.get(&(id.index() as usize)).map(|x| &x.calls),
            ) {
                results[i].links.push(Link {
                    origin,
                    uri: uris[t.source.index() as usize].clone(),
                    declaration: t.declaration,
                    name: t.name,
                });
            }
        }
    }
    uris.into_iter().zip(results).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let [Problem::Cycle { cycle: a, .. }, Problem::Call(independent)] =
            cycle["Consumer.scala"].problems.as_slice()
        else {
            panic!("expected one participant and one independent call")
        };
        let [Problem::Cycle { cycle: b, .. }] = cycle["Provider.scala"].problems.as_slice() else {
            panic!("expected the other participant")
        };
        assert!(Arc::ptr_eq(a, b));
        assert_eq!(a.code(), "signature.member_cycle");
        assert_eq!(independent.code(), "typer.application_arity_mismatch");

        let failed = check("class Provider { def value(): Int = 1; val seed = value(1) }");
        assert!(failed["Leaf.scala"].problems.is_empty());
        for file in ["Consumer.scala", "Provider.scala"] {
            let [Problem::Call(call)] = failed[file].problems.as_slice() else {
                panic!("expected only the original call in {file}")
            };
            assert_eq!(call.code(), "typer.application_arity_mismatch");
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
            SyntaxDiagnostic::Lexer { diagnostic, .. },
            SyntaxDiagnostic::Parser(parser),
        ] = files[uri].syntax_diagnostics.as_slice()
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
        let [Problem::Reported(problem)] = files[uri].problems.as_slice() else {
            panic!("expected one constructor diagnostic");
        };
        let range = problem
            .location
            .expect("position of the unknown type")
            .span()
            .range();
        assert_eq!(
            &text[range.start() as usize..range.end() as usize],
            "Missing"
        );
        assert_eq!(problem.severity, DiagnosticSeverity::Error);
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
            let Problem::Call(problem) = problem else {
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
        let Problem::Call(d) = &caller.problems[0] else {
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
