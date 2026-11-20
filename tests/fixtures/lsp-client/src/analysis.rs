//! Fresh semantic session for one set of open document versions.
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use dotty::core::{
    AstArena, Definitions, DiagnosticSeverity, DocumentText, NoResolver, Packages, SemanticStore,
    SourceId, SourceSemanticIndex, SourceText, SymbolId, TextRange, Typed, Untyped,
};
use dotty::jvm_class_plan::ClassHeaderBindings;
use dotty::namer::{NamingOutcome, SourceParseStatus, name_compilation_unit_with_diagnostics};
use dotty::source_declarations::{ClassMember, SourceClass, source_classes};
use dotty::typer::{SignatureCompletion, SourceTyper, TyperError};
use dotty::{ClassBodyError, SourceDeclarationIndex, SourceFileCalls, TypingDiagnostic};

#[derive(Clone)]
pub struct Document {
    pub version: i32,
    pub text: Arc<DocumentText>,
}

pub type Documents = BTreeMap<String, Document>;

#[derive(PartialEq, Eq)]
pub struct Problem {
    pub range: Option<TextRange>,
    pub message: String,
    pub severity: DiagnosticSeverity,
}

fn typing_problem(owner_file: usize, diagnostic: TypingDiagnostic) -> (usize, Problem) {
    let file = diagnostic
        .location
        .map_or(owner_file, |span| span.source().index() as usize);
    let problem = Problem {
        range: diagnostic.location.map(|span| span.span().range()),
        severity: diagnostic.severity,
        message: diagnostic.message,
    };
    (file, problem)
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

impl FileResult {
    fn error(&mut self, range: Option<TextRange>, message: String) {
        self.problems.push(Problem {
            range,
            message,
            severity: DiagnosticSeverity::Error,
        });
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
    let parsed = dotty::parser::parse_compilation_unit(
        SourceText::new(text).ok()?,
        source,
        dotty::lexer::ContextualScanner::new(text).ok()?,
        &mut store.names,
    );
    if !parsed.diagnostics.is_empty() {
        for issue in parsed.diagnostics {
            result.error(Some(issue.span()), format!("{:?}", issue.issue()));
        }
        return None;
    }
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
    for d in diagnostics {
        result.error(Some(d.span()), format!("{:?}", d.issue()));
    }
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

// Each pass removes all completed or rejected entries. A pass with no progress ends cycles.
// Report None for a dependency still blocked after a full pass.
fn complete<E>(
    mut pending: Vec<(usize, SymbolId)>,
    mut attempt: impl FnMut(usize, SymbolId) -> Result<SignatureCompletion, E>,
    mut report: impl FnMut(usize, SymbolId, Option<E>),
) {
    while !pending.is_empty() {
        let before = pending.len();
        pending.retain(|&(file, symbol)| match attempt(file, symbol) {
            Ok(SignatureCompletion::Complete(_)) => false,
            Ok(SignatureCompletion::BlockedOn(_)) => true,
            Err(error) => {
                report(file, symbol, Some(error));
                false
            }
        });
        if pending.len() == before {
            for (file, symbol) in pending {
                report(file, symbol, None);
            }
            break;
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
    let mut ranges = HashMap::new();
    let ast_for = |id: SourceId| units.get(&(id.index() as usize)).map(|unit| &unit.ast);
    let typer_problem = |file, error: &TyperError| {
        typing_problem(file, TypingDiagnostic::from_typer(error, ast_for))
    };
    for (&i, u) in &units {
        let mut typer = SourceTyper::new(&u.ast, u.source, &u.index, &mut store, defs, &packages);
        for c in &u.classes {
            let range = u.ast.get(c.tree).position.map(|p| p.span().range());
            ranges.insert(c.symbol, range);
            match typer.complete_symbol(c.constructor) {
                Ok(_) => classes.push((i, c.symbol)),
                Err(e) => {
                    unavailable.insert(c.symbol);
                    let (file, problem) = typer_problem(i, &e);
                    results[file].problems.push(problem);
                }
            }
        }
    }
    complete(
        classes,
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
        |i, symbol, e| {
            unavailable.insert(symbol);
            if let Some(error) = e {
                let (file, problem) = typer_problem(i, &error);
                results[file].problems.push(problem);
            } else {
                results[i].error(ranges[&symbol], "Unresolved signature dependency".into());
            }
        },
    );
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
                ranges.insert(
                    d.symbol,
                    u.ast.get(d.tree).position.map(|p| p.span().range()),
                );
                members.push((i, d.symbol));
            }
        }
    }
    let mut signature_problems = HashMap::new();
    complete(
        members,
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
        |i, symbol, error| {
            if let Some(error) = error {
                let (file, problem) = typing_problem(i, error.diagnostic(ast_for));
                signature_problems.insert(symbol, (file, results[file].problems.len()));
                results[file].problems.push(problem);
            } else {
                results[i].error(ranges[&symbol], "Unresolved signature dependency".into());
            }
        },
    );
    let mut checks = HashMap::new();
    for (&i, u) in &units {
        for c in &u.classes {
            if unavailable.contains(&c.symbol) {
                continue;
            }
            let check = dotty::check_class_body(
                c, &u.ast, &u.index, &mut store, defs, &packages, &mut arena, NoResolver,
            );
            for (source, d) in &check.diagnostics {
                results[source.index() as usize].problems.push(Problem {
                    range: Some(d.span()),
                    message: d.message().to_owned(),
                    severity: d.severity(),
                });
            }
            if let Err(errors) = &check.body {
                for error in errors {
                    let (file, problem) = typing_problem(i, error.diagnostic(ast_for));
                    // Body checking can retry a failed signature. Suppress only its own repeated report.
                    if let ClassBodyError::Member {
                        member: ClassMember::Field(d) | ClassMember::Method(d),
                        ..
                    } = error
                        && let Some(&(previous_file, index)) = signature_problems.get(&d.symbol)
                        && previous_file == file
                        && results[file].problems[index] == problem
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
    fn unknown_constructor_parameter_type_points_to_its_name() {
        let uri = "file:///Broken.scala";
        let text = "class Broken(start: Missing)";
        let doc = Document {
            version: 1,
            text: Arc::new(DocumentText::new(text.into()).unwrap()),
        };
        let files = analyze(&[(uri.to_owned(), doc)].into());
        let [problem] = files[uri].problems.as_slice() else {
            panic!("expected one constructor diagnostic");
        };
        let range = problem.range.expect("position of the unknown type");
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
            let range = problem.range.unwrap();
            assert_eq!(&text[range.start() as usize..range.end() as usize], call);
            assert_eq!(problem.message, "Expected 0 arguments, got 1");
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
        let d = &caller.problems[0];
        let range = d.range.expect("argument position");
        assert_eq!(&b[range.start() as usize..range.end() as usize], "x");
        assert_eq!(caller.links.len(), 1);
        let l = &caller.links[0];
        assert_eq!(l.uri, "file:///A.scala");
        assert_eq!(l.name.start(), a.rfind("take").unwrap() as u32);
    }
}
