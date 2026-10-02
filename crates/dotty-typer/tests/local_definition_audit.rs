use std::collections::{BTreeMap, BTreeSet, HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};

use dotty_classloader::classloader::{
    ClassPathEntry, ClasspathSymbolResolver, CompositeClassPath, JarClassPath, JdkClassPath,
    LoadingSession,
};
use dotty_core::ast::{TreeKind, Untyped, UntypedNode};
use dotty_core::{
    Definitions, MemberRequest, Packages, ResolutionError, ResolverCheckpoint, SemanticStore,
    SourceId, SourceText, SymbolId, SymbolResolver, TextRange,
};
use dotty_lexer::ContextualScanner;
use dotty_namer::{NamerError, name_compilation_unit};
use dotty_parser::{ParseDiagnosticKind, parse_compilation_unit};
use dotty_typer::{SourceTyper, TyperError};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct ResolverMetrics {
    package_requests: usize,
    package_successes: usize,
    package_unresolved: usize,
    package_errors: usize,
    member_requests: usize,
    member_successes: usize,
    member_unresolved: usize,
    member_errors: usize,
    packages: BTreeSet<String>,
    classes: BTreeSet<String>,
    members: BTreeSet<String>,
}

struct AuditResolver<E: ClassPathEntry> {
    inner: ClasspathSymbolResolver<E>,
    metrics: Rc<RefCell<ResolverMetrics>>,
}

#[derive(Clone)]
struct SharedClassPath(Arc<CompositeClassPath>);

impl ClassPathEntry for SharedClassPath {
    fn find_class(
        &self,
        name: &dotty_classloader::classloader::BinaryName,
    ) -> Result<
        Option<dotty_classloader::classloader::ClassResource>,
        dotty_classloader::classloader::ClassPathError,
    > {
        self.0.find_class(name)
    }

    fn contains_package(
        &self,
        package: &[&str],
    ) -> Result<bool, dotty_classloader::classloader::ClassPathError> {
        self.0.contains_package(package)
    }
}

impl<E: ClassPathEntry> SymbolResolver for AuditResolver<E> {
    fn checkpoint(&self) -> ResolverCheckpoint {
        self.inner.checkpoint()
    }

    fn rollback_to(&mut self, store: &mut SemanticStore, checkpoint: ResolverCheckpoint) {
        self.inner.rollback_to(store, checkpoint);
    }

    fn resolve_member(
        &mut self,
        store: &mut SemanticStore,
        request: &MemberRequest,
    ) -> Result<Option<SymbolId>, ResolutionError> {
        let result = self.inner.resolve_member(store, request);
        let mut metrics = self.metrics.borrow_mut();
        metrics.member_requests += 1;
        match &result {
            Ok(Some(symbol)) => {
                metrics.member_successes += 1;
                let path = audit_symbol_path(store, *symbol);
                match store.symbols.get(*symbol).kind {
                    dotty_core::SymbolKind::Class
                    | dotty_core::SymbolKind::Trait
                    | dotty_core::SymbolKind::Object
                    | dotty_core::SymbolKind::ModuleClass => {
                        metrics.classes.insert(path);
                    }
                    _ => {
                        metrics.members.insert(path);
                    }
                }
            }
            Ok(None) => metrics.member_unresolved += 1,
            Err(_) => metrics.member_errors += 1,
        }
        result
    }

    fn resolve_package(
        &mut self,
        store: &mut SemanticStore,
        path: &[&str],
    ) -> Result<Option<SymbolId>, ResolutionError> {
        let result = self.inner.resolve_package(store, path);
        let mut metrics = self.metrics.borrow_mut();
        metrics.package_requests += 1;
        match &result {
            Ok(Some(_)) => {
                metrics.package_successes += 1;
                metrics.packages.insert(path.join("."));
            }
            Ok(None) => metrics.package_unresolved += 1,
            Err(_) => metrics.package_errors += 1,
        }
        result
    }
}

fn audit_symbol_path(store: &SemanticStore, symbol: SymbolId) -> String {
    let mut parts = Vec::new();
    let mut current = Some(symbol);
    let mut visited = HashSet::new();
    while let Some(symbol) = current {
        if !visited.insert(symbol) {
            parts.push("<owner-cycle>".to_owned());
            break;
        }
        let entry = store.symbols.get(symbol);
        let name = store.names.resolve(entry.name.text());
        if !name.is_empty() {
            parts.push(name.to_owned());
        }
        current = entry.owner;
    }
    parts.reverse();
    parts.join(".")
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
enum FailureFamily {
    ResolutionClasspathEnvironment,
    ParserNamer,
    UnsupportedExpressionSyntaxSemantics,
    LocalDeclarationDeferral,
    TypeRelationInferenceCompletion,
    #[default]
    Other,
}

impl FailureFamily {
    fn label(self) -> &'static str {
        match self {
            Self::ResolutionClasspathEnvironment => "resolution/classpath environment",
            Self::ParserNamer => "parser/namer",
            Self::UnsupportedExpressionSyntaxSemantics => "unsupported expression syntax/semantics",
            Self::LocalDeclarationDeferral => "local declaration deferral",
            Self::TypeRelationInferenceCompletion => "type relation/inference/completion",
            Self::Other => "other",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct FailureClassification {
    bucket: String,
    family: FailureFamily,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Audit {
    files_attempted: usize,
    parser_failed_files: usize,
    namer_failed_files: usize,
    recovered_parser_files: usize,
    local_definitions: usize,
    buckets: BTreeMap<String, usize>,
    local_defdefs: usize,
    typed_local_defdefs: usize,
    failures: BTreeMap<String, FailureBucket>,
    expression_forms: BTreeMap<String, usize>,
    parser_diagnostics: BTreeMap<String, usize>,
}

impl Default for Audit {
    fn default() -> Self {
        Self {
            files_attempted: 0,
            parser_failed_files: 0,
            namer_failed_files: 0,
            recovered_parser_files: 0,
            local_definitions: 0,
            buckets: BTreeMap::new(),
            local_defdefs: 0,
            typed_local_defdefs: 0,
            failures: BTreeMap::new(),
            expression_forms: empty_expression_histogram(),
            parser_diagnostics: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct FailureBucket {
    family: FailureFamily,
    count: usize,
    files: BTreeSet<String>,
    examples: BTreeSet<String>,
}

impl Audit {
    fn merge(&mut self, other: Self) {
        self.files_attempted += other.files_attempted;
        self.parser_failed_files += other.parser_failed_files;
        self.namer_failed_files += other.namer_failed_files;
        self.recovered_parser_files += other.recovered_parser_files;
        self.local_definitions += other.local_definitions;
        self.local_defdefs += other.local_defdefs;
        self.typed_local_defdefs += other.typed_local_defdefs;
        for (name, count) in other.expression_forms {
            *self.expression_forms.entry(name).or_default() += count;
        }
        for (name, count) in other.parser_diagnostics {
            *self.parser_diagnostics.entry(name).or_default() += count;
        }
        for (name, count) in other.buckets {
            *self.buckets.entry(name).or_default() += count;
        }
        for (name, bucket) in other.failures {
            let target = self.failures.entry(name).or_default();
            target.family = bucket.family;
            target.count += bucket.count;
            target.files.extend(bucket.files);
            target.examples.extend(bucket.examples);
            target.examples = target.examples.iter().take(5).cloned().collect();
        }
    }
}

#[test]
#[ignore = "run with explicit SCALA39_ROOT, JAVA_HOME, SCALA39_JDK_RELEASE, and SCALA39_CLASSPATH"]
fn pinned_scala39_local_definition_audit() {
    let root = PathBuf::from(std::env::var_os("SCALA39_ROOT").expect("SCALA39_ROOT is required"));
    let revision = git_revision(&root);
    assert_eq!(
        revision, "777528f19a58e794c9954a42f433373472ec57f8",
        "audit requires the repository's pinned Scala 3.9.0 source revision"
    );
    let classpath = audit_classpath_from_environment();
    let resolver_metrics = Rc::new(RefCell::new(ResolverMetrics::default()));
    let files = scala_files(&[root.join("library/src"), root.join("compiler/src")]);
    let mut audit = Audit::default();
    for (index, file) in files.iter().enumerate() {
        if index % 100 == 0 {
            eprintln!("audited {index}/{} Scala files", files.len());
        }
        let relative = file
            .strip_prefix(&root)
            .unwrap_or(&file)
            .to_string_lossy()
            .replace('\\', "/");
        let source = fs::read_to_string(&file).expect("Scala source should be readable");
        audit.merge(audit_source_with_classpath(
            &source,
            &relative,
            classpath.clone(),
            Rc::clone(&resolver_metrics),
        ));
    }

    println!("scala_revision={revision}");
    println!("files={}", audit.files_attempted);
    println!("parser_failed_files={}", audit.parser_failed_files);
    println!("namer_failed_files={}", audit.namer_failed_files);
    println!("recovered_parser_files={}", audit.recovered_parser_files);
    println!("local_definitions={}", audit.local_definitions);
    let mut local_buckets = audit.buckets.iter().collect::<Vec<_>>();
    local_buckets.sort_by(|(name_a, count_a), (name_b, count_b)| {
        count_b.cmp(count_a).then(name_a.cmp(name_b))
    });
    for (bucket, count) in local_buckets {
        println!("{bucket}={count}");
    }
    println!("expression_forms:");
    let mut expression_forms = audit.expression_forms.iter().collect::<Vec<_>>();
    expression_forms.sort_by(|(name_a, count_a), (name_b, count_b)| {
        count_b.cmp(count_a).then(name_a.cmp(name_b))
    });
    for (form, count) in expression_forms {
        println!("  {form}={count}");
    }
    println!("parser_diagnostics:");
    let mut parser_diagnostics = audit.parser_diagnostics.iter().collect::<Vec<_>>();
    parser_diagnostics.sort_by(|(name_a, count_a), (name_b, count_b)| {
        count_b.cmp(count_a).then(name_a.cmp(name_b))
    });
    for (kind, count) in parser_diagnostics {
        println!("  {kind}={count}");
    }
    println!("local_defdefs={}", audit.local_defdefs);
    println!("typed_local_defdefs={}", audit.typed_local_defdefs);
    println!(
        "unsupported_expression_total={}",
        sum_buckets_with_prefix(&audit.failures, "UnsupportedExpression::")
    );
    println!(
        "local_block_declaration_deferred={}",
        sum_buckets_with_prefix(&audit.failures, "LocalBlockDeclarationDeferred::")
    );
    println!(
        "no_successful_enclosing_method_typing={}",
        audit
            .failures
            .get("NoSuccessfulEnclosingMethodTyping")
            .map_or(0, |bucket| bucket.count)
    );
    println!(
        "import_qualifier_not_found={}",
        audit
            .failures
            .get("ImportQualifierNotFound")
            .map_or(0, |bucket| bucket.count)
    );
    println!(
        "external_name_or_member_resolution_failures={}",
        sum_buckets_with_prefix(&audit.failures, "TypeNameNotFound")
            + sum_buckets_with_prefix(&audit.failures, "TermNameNotFound")
            + sum_buckets_with_prefix(&audit.failures, "MemberNotFound")
            + sum_buckets_with_prefix(&audit.failures, "MemberLookup")
            + sum_buckets_with_prefix(&audit.failures, "SymbolResolution")
    );
    let mut family_counts = BTreeMap::<FailureFamily, usize>::new();
    for bucket in audit.failures.values() {
        *family_counts.entry(bucket.family).or_default() += bucket.count;
    }
    println!("failure_families:");
    let mut family_counts = family_counts.into_iter().collect::<Vec<_>>();
    family_counts.sort_by(|(family_a, count_a), (family_b, count_b)| {
        count_b
            .cmp(count_a)
            .then(family_a.label().cmp(family_b.label()))
    });
    for (family, count) in family_counts {
        println!("  {}={count}", family.label());
    }
    println!("local_defdef_failures:");
    let mut failures = audit.failures.iter().collect::<Vec<_>>();
    failures.sort_by(|(name_a, a), (name_b, b)| b.count.cmp(&a.count).then(name_a.cmp(name_b)));
    for (name, bucket) in failures {
        println!(
            "  {name} [{}]: {} ({} files) [{}]",
            bucket.family.label(),
            bucket.count,
            bucket.files.len(),
            bucket
                .examples
                .iter()
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    print_ranked_gaps(&audit.failures);
    print_resolver_metrics(&resolver_metrics.borrow());
}

fn sum_buckets_with_prefix(failures: &BTreeMap<String, FailureBucket>, prefix: &str) -> usize {
    failures
        .iter()
        .filter(|(name, _)| name.starts_with(prefix))
        .map(|(_, bucket)| bucket.count)
        .sum()
}

fn print_ranked_gaps(failures: &BTreeMap<String, FailureBucket>) {
    let mut ranked = failures
        .iter()
        .filter(|(_, bucket)| bucket.family != FailureFamily::ParserNamer)
        .collect::<Vec<_>>();
    ranked.sort_by(|(name_a, a), (name_b, b)| b.count.cmp(&a.count).then(name_a.cmp(name_b)));
    println!("top_semantic_gaps:");
    for (rank, (name, bucket)) in ranked.iter().take(10).enumerate() {
        let category = implementation_category(name);
        println!(
            "  {}. {name}: count={}, files={}, category={}, examples={}",
            rank + 1,
            bucket.count,
            bucket.files.len(),
            category,
            bucket
                .examples
                .iter()
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    println!("implementation_scope_notes:");
    let mut seen = BTreeSet::new();
    for (name, _) in ranked.iter().take(5) {
        let category = implementation_category(name);
        if !seen.insert(category) {
            continue;
        }
        let (slice, owner, prerequisites, non_goals) = scope_note(category);
        println!(
            "  {category}: first_slice={slice}; owner={owner}; prerequisites={prerequisites}; non_goals={non_goals}"
        );
    }
}

fn implementation_category(bucket: &str) -> &'static str {
    if bucket.starts_with("UnsupportedExpression::") {
        "expression typing"
    } else if bucket.starts_with("LocalBlockDeclarationDeferred::") {
        "local declaration support"
    } else if bucket.contains("Pattern") || bucket.contains("Match") {
        "pattern typing"
    } else if bucket.contains("Inference")
        || bucket.contains("TypeRelation")
        || bucket.contains("Subtype")
    {
        "inference/type-relation support"
    } else if bucket.contains("Implicit") || bucket.contains("Contextual") {
        "implicit/contextual search"
    } else if bucket.starts_with("ImportQualifierNotFound")
        || bucket.starts_with("TypeNameNotFound")
        || bucket.starts_with("TermNameNotFound")
        || bucket.starts_with("MemberNotFound")
        || bucket.starts_with("MemberLookup")
    {
        "external resolution"
    } else {
        "other"
    }
}

fn scope_note(category: &str) -> (&'static str, &'static str, &'static str, &'static str) {
    match category {
        "expression typing" => (
            "lower one reported expression node through existing expression typing",
            "dotty-typer/src/typer/expression",
            "the parsed AST node and its child typing rules",
            "control-flow or inference redesign",
        ),
        "local declaration support" => (
            "enter one local declaration kind transactionally in block typing",
            "dotty-typer/src/typer/expression/blocks.rs",
            "source symbol and scope metadata from dotty-core",
            "local classes, imports, or type definitions beyond the selected kind",
        ),
        "pattern typing" => (
            "type one pattern form against an already known expected type",
            "dotty-typer/src/typer/patterns",
            "the expected type and existing pattern AST shape",
            "exhaustivity analysis and match-result inference",
        ),
        "inference/type-relation support" => (
            "add the smallest missing relation or inference case with exact unsupported errors",
            "dotty-typer/src/types",
            "focused type relation regression cases",
            "general-purpose constraint solving",
        ),
        "implicit/contextual search" => (
            "resolve one explicit contextual argument shape with unique-candidate checks",
            "dotty-typer/src/typer/application",
            "typed contextual parameter and scope lookup",
            "implicit scope derivation and recursive search",
        ),
        "external resolution" => (
            "resolve the exact missing package, type, or member through the classpath port",
            "dotty-classloader plus the existing typer resolver boundary",
            "classpath fixture and canonical package/session identity",
            "source lowering and classloader redesign",
        ),
        _ => (
            "reproduce the exact error bucket with a focused semantic fixture",
            "the narrow module producing that TyperError",
            "the relevant source semantic metadata",
            "adjacent unsupported language features",
        ),
    }
}

#[test]
fn local_definition_audit_is_deterministic() {
    let source =
        "object Audit { def outer: Int = { val n = 1; def local[A](x: A) = x; local(n) } }";
    let first = audit_source(source, "Audit.scala");
    let second = audit_source(source, "Audit.scala");
    assert_eq!(first, second);
    assert_eq!(first.local_defdefs, 1);
    assert_eq!(first.typed_local_defdefs, 1);
    assert_eq!(first.buckets.get("local_val_defs"), Some(&1));
}

#[test]
fn local_definition_audit_excludes_local_class_members() {
    let source = "object Audit { def outer: Int = { class Local { def member = 1 }; def local = 2; local } }";
    let audit = audit_source(source, "Audit.scala");

    assert_eq!(audit.local_defdefs, 1);
    assert_eq!(audit.buckets.get("local_def_defs"), Some(&1));
    assert_eq!(audit.buckets.get("local_classes"), Some(&1));
}

#[test]
fn local_definition_audit_counts_pattern_bindings_in_local_definitions() {
    let source = "object Audit { def outer: Int = { val captured @ _ = 1; 0 } }";
    let audit = audit_source(source, "Audit.scala");

    assert_eq!(
        audit.buckets.get("local_pattern_bindings"),
        Some(&1),
        "{audit:?}"
    );
}

#[test]
fn local_expression_audit_reports_exact_infix_failure_and_counts_it_structurally() {
    let source = "object Audit { def outer: Int = { def local: Int = 1 + 2; local } }";
    let audit = audit_source(source, "Infix.scala");

    assert_eq!(audit.expression_forms.get("InfixOp"), Some(&1));
    assert_eq!(
        audit
            .failures
            .get("UnsupportedExpression::InfixOp")
            .map(|bucket| bucket.count),
        Some(1),
        "{audit:?}"
    );
    assert_eq!(
        audit
            .failures
            .get("UnsupportedExpression::InfixOp")
            .map(|bucket| bucket.family),
        Some(FailureFamily::UnsupportedExpressionSyntaxSemantics)
    );
}

#[test]
fn local_expression_audit_reports_lambda_and_deferred_declaration_subkinds() {
    let lambda = audit_source(
        "object Audit { def outer: Int = { def local: Int = (x: Int) => x; 0 } }",
        "Lambda.scala",
    );
    assert!(
        lambda
            .failures
            .contains_key("UnsupportedExpression::Function"),
        "{lambda:?}"
    );

    let import = audit_source(
        "object Audit { def outer: Int = { import scala.collection; def local: Int = 1; local } }",
        "Import.scala",
    );
    assert!(
        import
            .failures
            .contains_key("LocalBlockDeclarationDeferred::import"),
        "{import:?}"
    );

    let type_definition = audit_source(
        "object Audit { def outer: Int = { type Local = Int; def local: Int = 1; local } }",
        "TypeDefinition.scala",
    );
    assert!(
        type_definition
            .failures
            .contains_key("LocalBlockDeclarationDeferred::type definition"),
        "{type_definition:?}"
    );
}

#[test]
fn local_expression_audit_classifies_missing_names_as_resolution_failures() {
    let audit = audit_source(
        "object Audit { def outer: Int = { def local: Missing = 1; local } }",
        "MissingType.scala",
    );

    let failure = audit
        .failures
        .get("TypeNameNotFound")
        .expect("missing type bucket");
    assert_eq!(
        failure.family,
        FailureFamily::ResolutionClasspathEnvironment
    );
}

#[test]
fn local_expression_audit_keeps_parser_and_namer_failures_distinct() {
    let namer = classify_namer_error(&NamerError::RootIsNotPackage { tree_index: 4 });
    assert_eq!(namer.bucket, "NamerError::RootIsNotPackage");
    assert_eq!(namer.family, FailureFamily::ParserNamer);

    let parser = classify_parse_diagnostic(ParseDiagnosticKind::ExpectedExpression);
    assert_eq!(parser.bucket, "ParserDiagnostic::ExpectedExpression");
    assert_eq!(parser.family, FailureFamily::ParserNamer);
    assert_ne!(namer.bucket, parser.bucket);
}

#[test]
fn local_expression_histogram_excludes_parameters_and_type_trees() {
    let audit = audit_source("object Audit { def outer(x: Int): Int = x }", "Types.scala");

    assert_eq!(audit.expression_forms.get("Ident"), Some(&1));
    assert_eq!(audit.expression_forms.len(), EXPRESSION_FORMS.len());
    assert!(
        audit
            .expression_forms
            .values()
            .filter(|count| **count == 0)
            .count()
            > 0
    );
}

#[test]
fn local_expression_audit_schema_keeps_zero_count_forms_without_an_ast() {
    let audit = Audit::default();

    assert_eq!(audit.expression_forms.len(), EXPRESSION_FORMS.len());
    assert!(audit.expression_forms.values().all(|count| *count == 0));
}

#[test]
fn local_expression_failure_examples_keep_the_smallest_five_paths() {
    let failure = FailureClassification {
        bucket: "UnsupportedExpression::InfixOp".to_owned(),
        family: FailureFamily::UnsupportedExpressionSyntaxSemantics,
    };
    let mut first = Audit::default();
    for path in [
        "z.scala", "y.scala", "x.scala", "w.scala", "v.scala", "a.scala",
    ] {
        record_failure(&mut first, failure.clone(), path);
    }
    let mut second = Audit::default();
    for path in [
        "a.scala", "v.scala", "w.scala", "x.scala", "y.scala", "z.scala",
    ] {
        record_failure(&mut second, failure.clone(), path);
    }

    assert_eq!(first, second);
    assert_eq!(
        first
            .failures
            .get("UnsupportedExpression::InfixOp")
            .unwrap()
            .examples
            .iter()
            .cloned()
            .collect::<Vec<_>>(),
        ["a.scala", "v.scala", "w.scala", "x.scala", "y.scala"]
    );
    assert_eq!(
        first
            .failures
            .get("UnsupportedExpression::InfixOp")
            .unwrap()
            .files
            .len(),
        6
    );
}

fn audit_source(text: &str, path: &str) -> Audit {
    audit_source_inner(text, path, None)
}

fn audit_source_with_classpath(
    text: &str,
    path: &str,
    classpath: SharedClassPath,
    metrics: Rc<RefCell<ResolverMetrics>>,
) -> Audit {
    audit_source_inner(text, path, Some((classpath, metrics)))
}

fn audit_source_inner(
    text: &str,
    path: &str,
    classpath: Option<(SharedClassPath, Rc<RefCell<ResolverMetrics>>)>,
) -> Audit {
    let source = SourceId::from_index(0);
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let scanner = match ContextualScanner::new(text) {
        Ok(scanner) => scanner,
        Err(_) => {
            let mut audit = Audit::default();
            audit.files_attempted = 1;
            audit.parser_failed_files = 1;
            return audit;
        }
    };
    let parsed = parse_compilation_unit(
        SourceText::new(text).expect("source text should be valid"),
        source,
        scanner,
        &mut store.names,
    );
    let mut audit = collect_local_nodes(&parsed.ast);
    audit.files_attempted = 1;
    audit.expression_forms = collect_expression_histogram(&parsed.ast);
    audit.recovered_parser_files = usize::from(!parsed.diagnostics.is_empty());
    for diagnostic in &parsed.diagnostics {
        let failure = classify_parse_diagnostic(diagnostic.kind());
        *audit.parser_diagnostics.entry(failure.bucket).or_default() += 1;
    }
    let mut packages = Packages::new();
    let index = match name_compilation_unit(
        &parsed.ast,
        parsed.root,
        source,
        path,
        &mut store,
        &mut packages,
    ) {
        Ok(index) => index,
        Err(error) => {
            audit.namer_failed_files = 1;
            if audit.local_defdefs != 0 {
                let failure = classify_namer_error(&error);
                for _ in 0..audit.local_defdefs {
                    record_failure(&mut audit, failure.clone(), path);
                }
            }
            return audit;
        }
    };
    if audit.local_defdefs == 0 {
        return audit;
    }

    let root_methods = parsed
        .ast
        .iter()
        .filter_map(|(tree, node)| {
            let TreeKind::DefDef(definition) = &node.kind else {
                return None;
            };
            let rhs = definition.rhs?;
            let method = index.symbol_at(source, tree)?;
            let range = parsed.ast.get(rhs).position?.span().range();
            Some((method, rhs, range))
        })
        .collect::<Vec<_>>();

    let local_method_trees = local_method_trees(&parsed.ast);

    let typer_packages = Packages::new();
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &typer_packages,
    );
    if let Some((classpath, metrics)) = classpath {
        let resolver = ClasspathSymbolResolver::new(
            classpath,
            definitions,
            LoadingSession::with_packages(packages),
        );
        typer = typer.with_resolver(Box::new(AuditResolver {
            inner: resolver,
            metrics,
        }));
    }
    let mut root_failures = Vec::new();
    for (method, rhs, range) in root_methods {
        let outcome = typer
            .expression_context_for(method)
            .and_then(|context| typer.type_expression(rhs, context));
        if let Err(error) = outcome {
            root_failures.push((range, classify_typer_error(&error, &parsed.ast)));
        }
    }

    for (tree, range) in local_method_trees {
        if typer.source_typed_index().get(source, tree).is_some() {
            audit.typed_local_defdefs += 1;
        } else {
            let kind = root_failures
                .iter()
                .filter(|(parent, _)| {
                    parent.start() <= range.start() && range.end() <= parent.end()
                })
                .min_by_key(|(parent, _)| parent.end().saturating_sub(parent.start()))
                .map(|(_, failure)| failure.clone())
                .unwrap_or_else(|| FailureClassification {
                    bucket: "NoSuccessfulEnclosingMethodTyping".to_owned(),
                    family: FailureFamily::Other,
                });
            record_failure(&mut audit, kind, path);
        }
    }
    audit
}

fn audit_classpath_from_environment() -> SharedClassPath {
    let java_home = PathBuf::from(
        std::env::var_os("JAVA_HOME").expect("JAVA_HOME must identify the JDK used by the audit"),
    );
    let release = std::env::var("SCALA39_JDK_RELEASE")
        .expect("SCALA39_JDK_RELEASE must explicitly select the JDK classfile release")
        .parse::<u16>()
        .expect("SCALA39_JDK_RELEASE must be a numeric Java feature release");
    let classpath = std::env::var_os("SCALA39_CLASSPATH").expect(
        "SCALA39_CLASSPATH must list the pinned Scala artifact jars; ambient CLASSPATH is ignored",
    );
    let mut entries: Vec<Box<dyn ClassPathEntry>> = vec![Box::new(
        JdkClassPath::new(java_home.join("jmods"))
            .expect("JAVA_HOME/jmods should be readable by the classpath loader"),
    )];
    let jars = std::env::split_paths(&classpath).collect::<Vec<_>>();
    assert!(
        !jars.is_empty(),
        "SCALA39_CLASSPATH must contain Scala jars"
    );
    for jar in jars {
        assert!(
            jar.is_file(),
            "classpath jar does not exist: {}",
            jar.display()
        );
        entries.push(Box::new(
            JarClassPath::new(jar.clone(), release)
                .unwrap_or_else(|error| panic!("cannot index {}: {error}", jar.display())),
        ));
    }
    SharedClassPath(Arc::new(CompositeClassPath::new(entries)))
}

fn print_resolver_metrics(metrics: &ResolverMetrics) {
    println!("resolver_metrics:");
    println!("  external_package_requests={}", metrics.package_requests);
    println!("  external_package_successes={}", metrics.package_successes);
    println!(
        "  external_package_unresolved={}",
        metrics.package_unresolved
    );
    println!("  external_package_errors={}", metrics.package_errors);
    println!("  external_member_requests={}", metrics.member_requests);
    println!("  external_member_successes={}", metrics.member_successes);
    println!("  external_member_unresolved={}", metrics.member_unresolved);
    println!("  external_member_errors={}", metrics.member_errors);
    println!("  distinct_packages={}", metrics.packages.len());
    println!("  distinct_classes={}", metrics.classes.len());
    println!("  distinct_members={}", metrics.members.len());
}

fn collect_local_nodes(arena: &dotty_core::AstArena<Untyped>) -> Audit {
    let local_stats = local_stat_trees(arena);
    let local_pattern_binds = local_pattern_bind_trees(arena);
    let parameter_trees = arena
        .iter()
        .flat_map(|(_, node)| match &node.kind {
            TreeKind::DefDef(definition) => definition
                .type_params
                .iter()
                .copied()
                .chain(definition.value_param_clauses.iter().flatten().copied())
                .collect::<Vec<_>>(),
            _ => Vec::new(),
        })
        .collect::<HashSet<_>>();
    let mut audit = Audit::default();
    for (tree, node) in arena.iter() {
        if parameter_trees.contains(&tree) {
            continue;
        }
        if !local_stats.contains(&tree) && !local_pattern_binds.contains(&tree) {
            continue;
        }
        let bucket = match &node.kind {
            TreeKind::ValDef(_) => Some("local_val_defs"),
            TreeKind::DefDef(_) => Some("local_def_defs"),
            TreeKind::TypeDef(definition) => {
                if matches!(arena.get(definition.rhs).kind, TreeKind::Template(_)) {
                    Some("local_classes")
                } else {
                    Some("local_type_defs")
                }
            }
            TreeKind::PhaseSpecific(UntypedNode::ModuleDef(_)) => Some("local_objects"),
            TreeKind::Import(_) => Some("local_imports"),
            TreeKind::Bind(_) => Some("local_pattern_bindings"),
            _ => None,
        };
        if let Some(bucket) = bucket {
            *audit.buckets.entry(bucket.to_owned()).or_default() += 1;
            audit.local_definitions += 1;
            if bucket == "local_def_defs" {
                audit.local_defdefs += 1;
            }
        }
    }
    audit
}

fn local_method_trees(
    arena: &dotty_core::AstArena<Untyped>,
) -> Vec<(dotty_core::TreeId<Untyped>, TextRange)> {
    local_stat_trees(arena)
        .into_iter()
        .filter_map(|tree| {
            let node = arena.get(tree);
            matches!(node.kind, TreeKind::DefDef(_))
                .then_some((tree, node.position?.span().range()))
        })
        .collect()
}

fn local_stat_trees(arena: &dotty_core::AstArena<Untyped>) -> HashSet<dotty_core::TreeId<Untyped>> {
    arena
        .iter()
        .flat_map(|(_, node)| match &node.kind {
            TreeKind::Block(block) => block.stats.clone(),
            _ => Vec::new(),
        })
        .collect()
}

fn local_pattern_bind_trees(
    arena: &dotty_core::AstArena<Untyped>,
) -> HashSet<dotty_core::TreeId<Untyped>> {
    let roots = local_stat_trees(arena)
        .into_iter()
        .flat_map(|tree| match &arena.get(tree).kind {
            TreeKind::PhaseSpecific(UntypedNode::PatDef(definition)) => definition.patterns.clone(),
            _ => Vec::new(),
        });
    let mut pending = roots.collect::<Vec<_>>();
    let mut visited = HashSet::new();
    let mut binds = HashSet::new();
    while let Some(tree) = pending.pop() {
        if !visited.insert(tree) {
            continue;
        }
        match &arena.get(tree).kind {
            TreeKind::Bind(binding) => {
                binds.insert(tree);
                pending.push(binding.body);
            }
            TreeKind::Alternative(alternative) => {
                pending.extend(alternative.alternatives.iter().copied());
            }
            TreeKind::UnApply(unapply) => pending.extend(unapply.patterns.iter().copied()),
            TreeKind::Typed(typed) => pending.push(typed.expr),
            TreeKind::Annotated(annotated) => pending.push(annotated.expr),
            TreeKind::PhaseSpecific(UntypedNode::Parens(parens)) => pending.push(parens.inner),
            TreeKind::PhaseSpecific(UntypedNode::Tuple(tuple)) => {
                pending.extend(tuple.elements.iter().copied());
            }
            TreeKind::PhaseSpecific(UntypedNode::InfixOp(infix)) => {
                pending.push(infix.left);
                pending.push(infix.right);
            }
            _ => {}
        }
    }
    binds
}

fn record_failure(audit: &mut Audit, failure: FailureClassification, path: &str) {
    let bucket = audit.failures.entry(failure.bucket).or_default();
    bucket.family = failure.family;
    bucket.count += 1;
    bucket.files.insert(path.to_owned());
    bucket.examples.insert(path.to_owned());
    bucket.examples = bucket.examples.iter().take(5).cloned().collect();
}

fn classify_typer_error(
    error: &TyperError,
    arena: &dotty_core::AstArena<Untyped>,
) -> FailureClassification {
    match error {
        TyperError::UnsupportedExpression { tree_index, .. } => {
            let node = arena
                .iter()
                .find(|(tree, _)| tree.index() == *tree_index)
                .map(|(_, node)| &node.kind);
            if let Some(TreeKind::PhaseSpecific(UntypedNode::Error(error))) = node {
                return FailureClassification {
                    bucket: format!("Parser::ErrorNode::{}", parser_error_node_name(error.kind)),
                    family: FailureFamily::ParserNamer,
                };
            }
            let form = node.map_or("Unknown", tree_kind_label);
            FailureClassification {
                bucket: format!("UnsupportedExpression::{form}"),
                family: FailureFamily::UnsupportedExpressionSyntaxSemantics,
            }
        }
        TyperError::LocalBlockDeclarationDeferred { kind, .. } => FailureClassification {
            bucket: format!("LocalBlockDeclarationDeferred::{kind}"),
            family: FailureFamily::LocalDeclarationDeferral,
        },
        TyperError::ImportQualifierNotFound { .. }
        | TyperError::TypeNameNotFound { .. }
        | TyperError::TermNameNotFound { .. }
        | TyperError::SymbolResolution { .. }
        | TyperError::MemberNotFound { .. }
        | TyperError::MemberLookup(_) => FailureClassification {
            bucket: typer_error_name(error).to_owned(),
            family: FailureFamily::ResolutionClasspathEnvironment,
        },
        _ => {
            let bucket = typer_error_name(error);
            let family = if is_type_relation_inference_or_completion(bucket) {
                FailureFamily::TypeRelationInferenceCompletion
            } else {
                FailureFamily::Other
            };
            FailureClassification {
                bucket: bucket.to_owned(),
                family,
            }
        }
    }
}

fn classify_namer_error(error: &NamerError) -> FailureClassification {
    let bucket = match error {
        NamerError::RootIsNotPackage { .. } => "NamerError::RootIsNotPackage",
        NamerError::DuplicateSourceTreeSymbol { .. } => "NamerError::DuplicateSourceTreeSymbol",
        NamerError::DuplicateDerivedSourceTreeSymbol { .. } => {
            "NamerError::DuplicateDerivedSourceTreeSymbol"
        }
        NamerError::ConflictingSourceProvenance { .. } => "NamerError::ConflictingSourceProvenance",
        NamerError::DuplicateExtensionPrefixClauses { .. } => {
            "NamerError::DuplicateExtensionPrefixClauses"
        }
        NamerError::DuplicateSourceExportSite { .. } => "NamerError::DuplicateSourceExportSite",
        NamerError::DuplicateDeclarationScope { .. } => "NamerError::DuplicateDeclarationScope",
        NamerError::DuplicateDeclarationContext { .. } => "NamerError::DuplicateDeclarationContext",
        NamerError::MalformedAstShape { .. } => "NamerError::MalformedAstShape",
        NamerError::InvalidVisibilityQualifier { protected, .. } => {
            if *protected {
                "NamerError::InvalidVisibilityQualifier::protected"
            } else {
                "NamerError::InvalidVisibilityQualifier::private"
            }
        }
        NamerError::UnsupportedGivenNameShape {
            unsupported_kind, ..
        } => {
            return FailureClassification {
                bucket: format!("NamerError::UnsupportedGivenNameShape::{unsupported_kind}"),
                family: FailureFamily::ParserNamer,
            };
        }
        NamerError::AnonymousGivenWithoutParents { .. } => {
            "NamerError::AnonymousGivenWithoutParents"
        }
    };
    FailureClassification {
        bucket: bucket.to_owned(),
        family: FailureFamily::ParserNamer,
    }
}

fn typer_error_name(error: &TyperError) -> &'static str {
    match error {
        TyperError::UnknownSymbol { .. } => "UnknownSymbol",
        TyperError::SourceProvenanceMissing { .. } => "SourceProvenanceMissing",
        TyperError::DeclarationContextMissing { .. } => "DeclarationContextMissing",
        TyperError::SourceContextMissing { .. } => "SourceContextMissing",
        TyperError::ExpressionOwnerMissing { .. } => "ExpressionOwnerMissing",
        TyperError::ExpressionOwnerKindUnsupported { .. } => "ExpressionOwnerKindUnsupported",
        TyperError::ExpressionMethodScopeMissing { .. } => "ExpressionMethodScopeMissing",
        TyperError::ExpressionMethodScopeOwnerMismatch { .. } => {
            "ExpressionMethodScopeOwnerMismatch"
        }
        TyperError::ExpressionOwnerSourceContextMissing { .. } => {
            "ExpressionOwnerSourceContextMissing"
        }
        TyperError::ExpressionOwnerDeclarationContextMissing { .. } => {
            "ExpressionOwnerDeclarationContextMissing"
        }
        TyperError::ExpressionLocalScopeMissing { .. } => "ExpressionLocalScopeMissing",
        TyperError::ExpressionLocalScopeStackMissing { .. } => "ExpressionLocalScopeStackMissing",
        TyperError::ExpressionLocalScopeStackForeign { .. } => "ExpressionLocalScopeStackForeign",
        TyperError::UnsupportedImportContext { .. } => "UnsupportedImportContext",
        TyperError::ImportQualifierNotFound { .. } => "ImportQualifierNotFound",
        TyperError::MalformedSourceImport { .. } => "MalformedSourceImport",
        TyperError::UnsupportedSymbolCompletion { .. } => "UnsupportedSymbolCompletion",
        TyperError::MissingClassScope { .. } => "MissingClassScope",
        TyperError::MalformedClassInfo { .. } => "MalformedClassInfo",
        TyperError::AnonymousClassInstantiationDeferred { .. } => {
            "AnonymousClassInstantiationDeferred"
        }
        TyperError::NewTargetNotClass { .. } => "NewTargetNotClass",
        TyperError::TraitInstantiation { .. } => "TraitInstantiation",
        TyperError::AbstractClassInstantiation { .. } => "AbstractClassInstantiation",
        TyperError::ModuleInstantiation { .. } => "ModuleInstantiation",
        TyperError::PackageInstantiation { .. } => "PackageInstantiation",
        TyperError::TypeParameterInstantiation { .. } => "TypeParameterInstantiation",
        TyperError::NewClassInfoUnavailable { .. } => "NewClassInfoUnavailable",
        TyperError::ConstructorLookupNormalization { .. } => "ConstructorLookupNormalization",
        TyperError::MalformedConstructorBucket { .. } => "MalformedConstructorBucket",
        TyperError::MalformedConstructorCandidate { .. } => "MalformedConstructorCandidate",
        TyperError::ConstructorApplicationUnavailable { .. } => "ConstructorApplicationUnavailable",
        TyperError::ConstructorOverloadResolutionDeferred { .. } => {
            "ConstructorOverloadResolutionDeferred"
        }
        TyperError::ConstructorApplicationNoApplicable { .. } => {
            "ConstructorApplicationNoApplicable"
        }
        TyperError::AmbiguousConstructorApplication { .. } => "AmbiguousConstructorApplication",
        TyperError::ConstructorOverloadResolutionRequiresUnsupportedCandidate { .. } => {
            "ConstructorOverloadResolutionRequiresUnsupportedCandidate"
        }
        TyperError::ConstructorPolymorphicApplicationDeferred { .. } => {
            "ConstructorPolymorphicApplicationDeferred"
        }
        TyperError::ConstructorCallableNotPolymorphic { .. } => "ConstructorCallableNotPolymorphic",
        TyperError::ConstructorTypeArgumentArityMismatch { .. } => {
            "ConstructorTypeArgumentArityMismatch"
        }
        TyperError::ConstructorInstanceClassMismatch { .. } => "ConstructorInstanceClassMismatch",
        TyperError::ConstructorPolyInstantiationFailed { .. } => {
            "ConstructorPolyInstantiationFailed"
        }
        TyperError::ConstructorResultTypeMismatch { .. } => "ConstructorResultTypeMismatch",
        TyperError::ConstructorResultTypeCheckUnsupported { .. } => {
            "ConstructorResultTypeCheckUnsupported"
        }
        TyperError::ConstructorTypeArgumentBoundViolation { .. } => {
            "ConstructorTypeArgumentBoundViolation"
        }
        TyperError::UnsupportedConstructorTypeArgumentBounds { .. } => {
            "UnsupportedConstructorTypeArgumentBounds"
        }
        TyperError::ConstructorTypeArgumentBoundCheckUnsupported { .. } => {
            "ConstructorTypeArgumentBoundCheckUnsupported"
        }
        TyperError::UnconstrainedConstructorTypeParameter { .. } => {
            "UnconstrainedConstructorTypeParameter"
        }
        TyperError::UnsupportedRawGenericSecondaryConstructorInference { .. } => {
            "UnsupportedRawGenericSecondaryConstructorInference"
        }
        TyperError::ConflictingConstructorInferenceConstraints { .. } => {
            "ConflictingConstructorInferenceConstraints"
        }
        TyperError::UnsupportedConstructorInferenceShape { .. } => {
            "UnsupportedConstructorInferenceShape"
        }
        TyperError::UnableToFinalizeRawGenericNewInstanceType { .. } => {
            "UnableToFinalizeRawGenericNewInstanceType"
        }
        TyperError::MalformedClassParent { .. } => "MalformedClassParent",
        TyperError::UnresolvedParentClassKind { .. } => "UnresolvedParentClassKind",
        TyperError::HigherKindedTypeParameterDeferred { .. } => "HigherKindedTypeParameterDeferred",
        TyperError::HigherKindedTypeAliasDeferred { .. } => "HigherKindedTypeAliasDeferred",
        TyperError::InvalidCompletedBounds { .. } => "InvalidCompletedBounds",
        TyperError::OpaqueAliasDeferred { .. } => "OpaqueAliasDeferred",
        TyperError::RecursiveInferredMethodResult { .. } => "RecursiveInferredMethodResult",
        TyperError::InferredMethodResultRightHandSideMissing { .. } => {
            "InferredMethodResultRightHandSideMissing"
        }
        TyperError::InvalidInferredMethodResult { .. } => "InvalidInferredMethodResult",
        TyperError::RightAssociativeExtensionDeferred { .. } => "RightAssociativeExtensionDeferred",
        TyperError::MethodParameterSymbolMissing { .. } => "MethodParameterSymbolMissing",
        TyperError::LocalMethodSignatureDeferred { .. } => "LocalMethodSignatureDeferred",
        TyperError::LocalMethodInferredResultDeferred { .. } => "LocalMethodInferredResultDeferred",
        TyperError::MethodTypeParameterSymbolMissing { .. } => "MethodTypeParameterSymbolMissing",
        TyperError::ExtensionPrefixClausesMissing { .. } => "ExtensionPrefixClausesMissing",
        TyperError::MalformedMethodClause { .. } => "MalformedMethodClause",
        TyperError::ConstructorOwnerNotClassLike { .. } => "ConstructorOwnerNotClassLike",
        TyperError::MalformedConstructorOwnerInfo { .. } => "MalformedConstructorOwnerInfo",
        TyperError::MalformedConstructorOwner { .. } => "MalformedConstructorOwner",
        TyperError::SecondaryConstructorTypeParametersUnsupported { .. } => {
            "SecondaryConstructorTypeParametersUnsupported"
        }
        TyperError::DeferredSymbolCompletion { .. } => "DeferredSymbolCompletion",
        TyperError::SymbolAlreadyErrored { .. } => "SymbolAlreadyErrored",
        TyperError::UnsupportedTypeTree { .. } => "UnsupportedTypeTree",
        TyperError::MissingDeclaredType { .. } => "MissingDeclaredType",
        TyperError::MalformedSourceAst { .. } => "MalformedSourceAst",
        TyperError::SymbolSourceKindMismatch { .. } => "SymbolSourceKindMismatch",
        TyperError::TreeOutsideArena { .. } => "TreeOutsideArena",
        TyperError::TypeNameNotFound { .. } => "TypeNameNotFound",
        TyperError::WrongTypeNameKind { .. } => "WrongTypeNameKind",
        TyperError::SymbolResolution { .. } => "SymbolResolution",
        TyperError::AmbiguousTypeName { .. } => "AmbiguousTypeName",
        TyperError::DuplicateSourceTypeCacheEntry { .. } => "DuplicateSourceTypeCacheEntry",
        TyperError::TypeRebinding { .. } => "TypeRebinding",
        TyperError::SourceClassTypeParametersProvenanceMissing { .. } => {
            "SourceClassTypeParametersProvenanceMissing"
        }
        TyperError::MalformedSourceClassTypeParameters { .. } => {
            "MalformedSourceClassTypeParameters"
        }
        TyperError::ClassTypeParameterSymbolMissing { .. } => "ClassTypeParameterSymbolMissing",
        TyperError::ReceiverDoesNotDenoteExpectedClass { .. } => {
            "ReceiverDoesNotDenoteExpectedClass"
        }
        TyperError::ReceiverGenericArityMismatch { .. } => "ReceiverGenericArityMismatch",
        TyperError::ConstructorTargetGenericArityMismatch { .. } => {
            "ConstructorTargetGenericArityMismatch"
        }
        TyperError::RawGenericSourceReceiverUnsupported { .. } => {
            "RawGenericSourceReceiverUnsupported"
        }
        TyperError::ExternalGenericInstantiationDeferred { .. } => {
            "ExternalGenericInstantiationDeferred"
        }
        TyperError::MemberTypeUnavailable { .. } => "MemberTypeUnavailable",
        TyperError::TypeSubstitution(..) => "TypeSubstitution",
        TyperError::TypeNormalization(..) => "TypeNormalization",
        TyperError::ConflictingTypedExpression { .. } => "ConflictingTypedExpression",
        TyperError::StringLiteralTypingDeferred { .. } => "StringLiteralTypingDeferred",
        TyperError::NullLiteralTypingDeferred { .. } => "NullLiteralTypingDeferred",
        TyperError::IntegerLiteralOutOfRange { .. } => "IntegerLiteralOutOfRange",
        TyperError::FloatingLiteralInvalid { .. } => "FloatingLiteralInvalid",
        TyperError::ThisOwnerNotEnclosing { .. } => "ThisOwnerNotEnclosing",
        TyperError::ThisOwnerCycle { .. } => "ThisOwnerCycle",
        TyperError::TermNameNotFound { .. } => "TermNameNotFound",
        TyperError::AmbiguousTermReference { .. } => "AmbiguousTermReference",
        TyperError::OverloadedReferenceDeferred { .. } => "OverloadedReferenceDeferred",
        TyperError::ObjectTermReferenceDeferred { .. } => "ObjectTermReferenceDeferred",
        TyperError::ObjectModuleClassUnavailable { .. } => "ObjectModuleClassUnavailable",
        TyperError::UnsupportedTermReference { .. } => "UnsupportedTermReference",
        TyperError::MemberNotFound { .. } => "MemberNotFound",
        TyperError::OverloadedSelectionDeferred { .. } => "OverloadedSelectionDeferred",
        TyperError::ApplicationCalleeNotMethod { .. } => "ApplicationCalleeNotMethod",
        TyperError::UnconstrainedTypeParameter { .. } => "UnconstrainedTypeParameter",
        TyperError::ConflictingInferenceConstraints { .. } => "ConflictingInferenceConstraints",
        TyperError::UnsupportedInferenceShape { .. } => "UnsupportedInferenceShape",
        TyperError::UnsupportedPolymorphicApplicationShape { .. } => {
            "UnsupportedPolymorphicApplicationShape"
        }
        TyperError::GenericOverloadResolutionDeferred { .. } => "GenericOverloadResolutionDeferred",
        TyperError::InferredTypeArgumentBoundViolation { .. } => {
            "InferredTypeArgumentBoundViolation"
        }
        TyperError::UnsupportedInferredTypeArgumentBounds { .. } => {
            "UnsupportedInferredTypeArgumentBounds"
        }
        TyperError::InferredTypeArgumentBoundCheckUnsupported { .. } => {
            "InferredTypeArgumentBoundCheckUnsupported"
        }
        TyperError::ExplicitTypeApplicationCalleeNotPoly { .. } => {
            "ExplicitTypeApplicationCalleeNotPoly"
        }
        TyperError::ExplicitTypeApplicationArityMismatch { .. } => {
            "ExplicitTypeApplicationArityMismatch"
        }
        TyperError::ExplicitTypeArgumentBoundViolation { .. } => {
            "ExplicitTypeArgumentBoundViolation"
        }
        TyperError::UnsupportedExplicitTypeArgumentBounds { .. } => {
            "UnsupportedExplicitTypeArgumentBounds"
        }
        TyperError::ExplicitTypeArgumentBoundCheckUnsupported { .. } => {
            "ExplicitTypeArgumentBoundCheckUnsupported"
        }
        TyperError::OverloadedTypeApplicationDeferred { .. } => "OverloadedTypeApplicationDeferred",
        TyperError::PolyInstantiation(..) => "PolyInstantiation",
        TyperError::TypeArgumentTreeCannotBeReified { .. } => "TypeArgumentTreeCannotBeReified",
        TyperError::TypeAscriptionTreeCannotBeReified { .. } => "TypeAscriptionTreeCannotBeReified",
        TyperError::ApplicationMethodKindMismatch { .. } => "ApplicationMethodKindMismatch",
        TyperError::UnsupportedApplicationMethodKind { .. } => "UnsupportedApplicationMethodKind",
        TyperError::UsingApplicationDeferred { .. } => "UsingApplicationDeferred",
        TyperError::ApplicationArityMismatch { .. } => "ApplicationArityMismatch",
        TyperError::ErasedApplicationParameterDeferred { .. } => {
            "ErasedApplicationParameterDeferred"
        }
        TyperError::VarargsApplicationParameterDeferred { .. } => {
            "VarargsApplicationParameterDeferred"
        }
        TyperError::VarargsParameterReferenceDeferred { .. } => "VarargsParameterReferenceDeferred",
        TyperError::ByNameApplicationParameterDeferred { .. } => {
            "ByNameApplicationParameterDeferred"
        }
        TyperError::DependentMethodApplicationDeferred { .. } => {
            "DependentMethodApplicationDeferred"
        }
        TyperError::ApplicationArgumentTypeMismatch { .. } => "ApplicationArgumentTypeMismatch",
        TyperError::ApplicationArgumentConformanceUnsupported { .. } => {
            "ApplicationArgumentConformanceUnsupported"
        }
        TyperError::ExpectedExpressionTypeMismatch { .. } => "ExpectedExpressionTypeMismatch",
        TyperError::ExpectedExpressionConformanceUnsupported { .. } => {
            "ExpectedExpressionConformanceUnsupported"
        }
        TyperError::AssignmentLhsNotAssignable { .. } => "AssignmentLhsNotAssignable",
        TyperError::AssignmentTargetImmutable { .. } => "AssignmentTargetImmutable",
        TyperError::AssignmentTargetKindUnsupported { .. } => "AssignmentTargetKindUnsupported",
        TyperError::WritableAssignmentTypeUnavailable { .. } => "WritableAssignmentTypeUnavailable",
        TyperError::IfConditionTypeMismatch { .. } => "IfConditionTypeMismatch",
        TyperError::IfConditionConformanceUnsupported { .. } => "IfConditionConformanceUnsupported",
        TyperError::IfBranchTypeCannotBeWidened { .. } => "IfBranchTypeCannotBeWidened",
        TyperError::IfBranchJoinUnsupported { .. } => "IfBranchJoinUnsupported",
        TyperError::IfChildTreeOutsideArena { .. } => "IfChildTreeOutsideArena",
        TyperError::WhileConditionTypeMismatch { .. } => "WhileConditionTypeMismatch",
        TyperError::WhileConditionConformanceUnsupported { .. } => {
            "WhileConditionConformanceUnsupported"
        }
        TyperError::WhileChildTreeOutsideArena { .. } => "WhileChildTreeOutsideArena",
        TyperError::ReturnOutsideSupportedMethod { .. } => "ReturnOutsideSupportedMethod",
        TyperError::ReturnMethodProvenanceMalformed { .. } => "ReturnMethodProvenanceMalformed",
        TyperError::ReturnInInferredResultMethodDeferred { .. } => {
            "ReturnInInferredResultMethodDeferred"
        }
        TyperError::MalformedReturnTarget { .. } => "MalformedReturnTarget",
        TyperError::NonLocalReturnDeferred { .. } => "NonLocalReturnDeferred",
        TyperError::ReturnExpressionTypeMismatch { .. } => "ReturnExpressionTypeMismatch",
        TyperError::ReturnExpressionConformanceUnsupported { .. } => {
            "ReturnExpressionConformanceUnsupported"
        }
        TyperError::MalformedReturnOwnerChain { .. } => "MalformedReturnOwnerChain",
        TyperError::MalformedReturnMethodSignature { .. } => "MalformedReturnMethodSignature",
        TyperError::OverloadApplicationNoApplicable { .. } => "OverloadApplicationNoApplicable",
        TyperError::AmbiguousOverloadApplication { .. } => "AmbiguousOverloadApplication",
        TyperError::OverloadResolutionRequiresUnsupportedCandidate { .. } => {
            "OverloadResolutionRequiresUnsupportedCandidate"
        }
        TyperError::MalformedOverloadCandidate { .. } => "MalformedOverloadCandidate",
        TyperError::MixedApplicationCandidateKinds { .. } => "MixedApplicationCandidateKinds",
        TyperError::TypeSelectionInExpression { .. } => "TypeSelectionInExpression",
        TyperError::MemberLookup(..) => "MemberLookup",
        TyperError::UnsupportedExpression { .. } => "UnsupportedExpression",
        TyperError::LocalBlockDeclarationDeferred { .. } => "LocalBlockDeclarationDeferred",
        TyperError::InvalidInferredLocalValueType { .. } => "InvalidInferredLocalValueType",
        TyperError::LocalValueRightHandSideMissing { .. } => "LocalValueRightHandSideMissing",
        TyperError::RecursiveLocalValueInitializer { .. } => "RecursiveLocalValueInitializer",
        TyperError::LocalValueOutsideBlock { .. } => "LocalValueOutsideBlock",
        TyperError::DuplicateLocalValue { .. } => "DuplicateLocalValue",
        TyperError::LocalValueTypeMismatch { .. } => "LocalValueTypeMismatch",
        TyperError::LocalValueConformanceUnsupported { .. } => "LocalValueConformanceUnsupported",
        TyperError::ExpressionTypeCannotBeWidened { .. } => "ExpressionTypeCannotBeWidened",
        TyperError::TermReferenceCannotBeWidened { .. } => "TermReferenceCannotBeWidened",
        TyperError::TermReferencePrefixMismatch { .. } => "TermReferencePrefixMismatch",
        TyperError::UnstableSelectionPrefix { .. } => "UnstableSelectionPrefix",
    }
}

fn is_type_relation_inference_or_completion(bucket: &str) -> bool {
    matches!(
        bucket.split("::").next().unwrap_or(bucket),
        "TypeRebinding"
            | "TypeSubstitution"
            | "TypeNormalization"
            | "ConstructorLookupNormalization"
            | "ConstructorResultTypeCheckUnsupported"
            | "ConstructorTypeArgumentBoundCheckUnsupported"
            | "ConflictingConstructorInferenceConstraints"
            | "UnsupportedConstructorInferenceShape"
            | "UnableToFinalizeRawGenericNewInstanceType"
            | "InvalidCompletedBounds"
            | "RecursiveInferredMethodResult"
            | "InvalidInferredMethodResult"
            | "MethodParameterSymbolMissing"
            | "LocalMethodSignatureDeferred"
            | "LocalMethodInferredResultDeferred"
            | "MethodTypeParameterSymbolMissing"
            | "MalformedMethodClause"
            | "DeferredSymbolCompletion"
            | "ClassTypeParameterSymbolMissing"
            | "ReceiverGenericArityMismatch"
            | "ConstructorTargetGenericArityMismatch"
            | "ExternalGenericInstantiationDeferred"
            | "MemberTypeUnavailable"
            | "UnconstrainedTypeParameter"
            | "ConflictingInferenceConstraints"
            | "UnsupportedInferenceShape"
            | "UnsupportedPolymorphicApplicationShape"
            | "GenericOverloadResolutionDeferred"
            | "InferredTypeArgumentBoundViolation"
            | "UnsupportedInferredTypeArgumentBounds"
            | "InferredTypeArgumentBoundCheckUnsupported"
            | "ExplicitTypeApplicationCalleeNotPoly"
            | "ExplicitTypeApplicationArityMismatch"
            | "ExplicitTypeArgumentBoundViolation"
            | "UnsupportedExplicitTypeArgumentBounds"
            | "ExplicitTypeArgumentBoundCheckUnsupported"
            | "OverloadedTypeApplicationDeferred"
            | "PolyInstantiation"
            | "ApplicationMethodKindMismatch"
            | "UnsupportedApplicationMethodKind"
            | "ApplicationArityMismatch"
            | "ErasedApplicationParameterDeferred"
            | "VarargsApplicationParameterDeferred"
            | "VarargsParameterReferenceDeferred"
            | "ByNameApplicationParameterDeferred"
            | "DependentMethodApplicationDeferred"
            | "ApplicationArgumentTypeMismatch"
            | "ApplicationArgumentConformanceUnsupported"
            | "ExpectedExpressionTypeMismatch"
            | "ExpectedExpressionConformanceUnsupported"
            | "WritableAssignmentTypeUnavailable"
            | "IfConditionTypeMismatch"
            | "IfConditionConformanceUnsupported"
            | "IfBranchTypeCannotBeWidened"
            | "IfBranchJoinUnsupported"
            | "WhileConditionTypeMismatch"
            | "WhileConditionConformanceUnsupported"
            | "ReturnExpressionTypeMismatch"
            | "ReturnExpressionConformanceUnsupported"
            | "OverloadApplicationNoApplicable"
            | "AmbiguousOverloadApplication"
            | "OverloadResolutionRequiresUnsupportedCandidate"
            | "MalformedOverloadCandidate"
            | "MixedApplicationCandidateKinds"
            | "TypeArgumentTreeCannotBeReified"
            | "TypeAscriptionTreeCannotBeReified"
            | "InvalidInferredLocalValueType"
            | "LocalValueTypeMismatch"
            | "LocalValueConformanceUnsupported"
            | "ExpressionTypeCannotBeWidened"
            | "TermReferenceCannotBeWidened"
            | "TermReferencePrefixMismatch"
            | "UnstableSelectionPrefix"
    ) || bucket.starts_with("Constructor")
        || bucket.starts_with("Class")
        || bucket.starts_with("Method")
        || bucket.starts_with("Application")
        || bucket.starts_with("Overload")
}

fn parser_error_node_name(kind: dotty_core::ast::ErrorNodeKind) -> &'static str {
    match kind {
        dotty_core::ast::ErrorNodeKind::MissingExpression => "MissingExpression",
        dotty_core::ast::ErrorNodeKind::MissingType => "MissingType",
        dotty_core::ast::ErrorNodeKind::MissingPattern => "MissingPattern",
        dotty_core::ast::ErrorNodeKind::UnexpectedToken => "UnexpectedToken",
    }
}

fn parse_diagnostic_name(kind: ParseDiagnosticKind) -> &'static str {
    match kind {
        ParseDiagnosticKind::ExpectedToken => "ExpectedToken",
        ParseDiagnosticKind::UnexpectedToken => "UnexpectedToken",
        ParseDiagnosticKind::ExpectedExpression => "ExpectedExpression",
        ParseDiagnosticKind::ExpectedType => "ExpectedType",
        ParseDiagnosticKind::ExpectedPattern => "ExpectedPattern",
        ParseDiagnosticKind::UnsupportedSyntax => "UnsupportedSyntax",
        ParseDiagnosticKind::UnboundPlaceholderParameter => "UnboundPlaceholderParameter",
    }
}

fn classify_parse_diagnostic(kind: ParseDiagnosticKind) -> FailureClassification {
    FailureClassification {
        bucket: format!("ParserDiagnostic::{}", parse_diagnostic_name(kind)),
        family: FailureFamily::ParserNamer,
    }
}

fn tree_kind_label(kind: &TreeKind<Untyped>) -> &'static str {
    match kind {
        TreeKind::Ident(_) => "Ident",
        TreeKind::Select(_) => "Select",
        TreeKind::This(_) => "This",
        TreeKind::Super(_) => "Super",
        TreeKind::Literal(_) => "Literal",
        TreeKind::Apply(_) => "Apply",
        TreeKind::TypeApply(_) => "TypeApply",
        TreeKind::New(_) => "New",
        TreeKind::Typed(_) => "Typed",
        TreeKind::NamedArg(_) => "NamedArg",
        TreeKind::Assign(_) => "Assign",
        TreeKind::Block(_) => "Block",
        TreeKind::If(_) => "If",
        TreeKind::Match(_) => "Match",
        TreeKind::CaseDef(_) => "CaseDef",
        TreeKind::Return(_) => "Return",
        TreeKind::While(_) => "While",
        TreeKind::Try(_) => "Try",
        TreeKind::Closure(_) => "Closure",
        TreeKind::ValDef(_) => "ValDef",
        TreeKind::DefDef(_) => "DefDef",
        TreeKind::TypeDef(_) => "TypeDef",
        TreeKind::Template(_) => "Template",
        TreeKind::PackageDef(_) => "PackageDef",
        TreeKind::Import(_) => "Import",
        TreeKind::Export(_) => "Export",
        TreeKind::TypeTree(_) => "TypeTree",
        TreeKind::SingletonTypeTree(_) => "SingletonTypeTree",
        TreeKind::AppliedTypeTree(_) => "AppliedTypeTree",
        TreeKind::RefinedTypeTree(_) => "RefinedTypeTree",
        TreeKind::LambdaTypeTree(_) => "LambdaTypeTree",
        TreeKind::MatchTypeTree(_) => "MatchTypeTree",
        TreeKind::ByNameTypeTree(_) => "ByNameTypeTree",
        TreeKind::TypeBoundsTree(_) => "TypeBoundsTree",
        TreeKind::Bind(_) => "Bind",
        TreeKind::Alternative(_) => "Alternative",
        TreeKind::UnApply(_) => "UnApply",
        TreeKind::Annotated(_) => "Annotated",
        TreeKind::Quote(_) => "Quote",
        TreeKind::Splice(_) => "Splice",
        TreeKind::QuotePattern(_) => "QuotePattern",
        TreeKind::SplicePattern(_) => "SplicePattern",
        TreeKind::Inlined(_) => "Inlined",
        TreeKind::PhaseSpecific(extra) => match extra {
            UntypedNode::Error(_) => "Error",
            UntypedNode::ModuleDef(_) => "ModuleDef",
            UntypedNode::Function(_) => "Function",
            UntypedNode::FunctionWithMods(_) => "FunctionWithMods",
            UntypedNode::CapturesAndResult(_) => "CapturesAndResult",
            UntypedNode::PolyFunction(_) => "PolyFunction",
            UntypedNode::InfixOp(_) => "InfixOp",
            UntypedNode::PrefixOp(_) => "PrefixOp",
            UntypedNode::PostfixOp(_) => "PostfixOp",
            UntypedNode::Parens(_) => "Parens",
            UntypedNode::Tuple(_) => "Tuple",
            UntypedNode::ForYield(_) => "ForYield",
            UntypedNode::ForDo(_) => "ForDo",
            UntypedNode::GenFrom(_) => "GenFrom",
            UntypedNode::GenAlias(_) => "GenAlias",
            UntypedNode::PatDef(_) => "PatDef",
            UntypedNode::ExtensionMethods(_) => "ExtensionMethods",
            UntypedNode::InterpolatedString(_) => "InterpolatedString",
            UntypedNode::ContextBounds(_) => "ContextBounds",
            UntypedNode::ContextBoundTypeTree(_) => "ContextBoundTypeTree",
            UntypedNode::Number(_) => "Number",
            UntypedNode::Throw(_) => "Throw",
            UntypedNode::ParsedTry(_) => "ParsedTry",
        },
    }
}

fn expression_form(kind: &TreeKind<Untyped>) -> Option<&'static str> {
    match kind {
        TreeKind::Ident(_) => Some("Ident"),
        TreeKind::Select(_) => Some("Select"),
        TreeKind::Apply(_) => Some("Apply"),
        TreeKind::TypeApply(_) => Some("TypeApply"),
        TreeKind::New(_) => Some("New"),
        TreeKind::Typed(_) => Some("Typed"),
        TreeKind::Assign(_) => Some("Assign"),
        TreeKind::Block(_) => Some("Block"),
        TreeKind::If(_) => Some("If"),
        TreeKind::Match(_) => Some("Match"),
        TreeKind::Return(_) => Some("Return"),
        TreeKind::While(_) => Some("While"),
        TreeKind::Try(_) => Some("Try"),
        TreeKind::PhaseSpecific(UntypedNode::Function(_)) => Some("Function"),
        TreeKind::PhaseSpecific(UntypedNode::PolyFunction(_)) => Some("PolyFunction"),
        TreeKind::PhaseSpecific(UntypedNode::InfixOp(_)) => Some("InfixOp"),
        TreeKind::PhaseSpecific(UntypedNode::PrefixOp(_)) => Some("PrefixOp"),
        TreeKind::PhaseSpecific(UntypedNode::PostfixOp(_)) => Some("PostfixOp"),
        TreeKind::PhaseSpecific(UntypedNode::Parens(_)) => Some("Parens"),
        TreeKind::PhaseSpecific(UntypedNode::Tuple(_)) => Some("Tuple"),
        TreeKind::PhaseSpecific(UntypedNode::ForYield(_)) => Some("ForYield"),
        TreeKind::PhaseSpecific(UntypedNode::ForDo(_)) => Some("ForDo"),
        TreeKind::PhaseSpecific(UntypedNode::Throw(_)) => Some("Throw"),
        TreeKind::PhaseSpecific(UntypedNode::ParsedTry(_)) => Some("ParsedTry"),
        TreeKind::PhaseSpecific(UntypedNode::InterpolatedString(_)) => Some("InterpolatedString"),
        _ => None,
    }
}

const EXPRESSION_FORMS: &[&str] = &[
    "Ident",
    "Select",
    "Apply",
    "TypeApply",
    "New",
    "Typed",
    "Assign",
    "Block",
    "If",
    "Match",
    "Return",
    "While",
    "Try",
    "Function",
    "PolyFunction",
    "InfixOp",
    "PrefixOp",
    "PostfixOp",
    "Parens",
    "Tuple",
    "ForYield",
    "ForDo",
    "Throw",
    "ParsedTry",
    "InterpolatedString",
];

fn empty_expression_histogram() -> BTreeMap<String, usize> {
    EXPRESSION_FORMS
        .iter()
        .map(|form| ((*form).to_owned(), 0))
        .collect()
}

fn collect_expression_histogram(arena: &dotty_core::AstArena<Untyped>) -> BTreeMap<String, usize> {
    let mut histogram = empty_expression_histogram();
    let nodes = arena
        .iter()
        .map(|(tree, node)| (tree.index(), node))
        .collect::<BTreeMap<_, _>>();
    for (_, node) in arena.iter() {
        let TreeKind::DefDef(definition) = &node.kind else {
            continue;
        };
        if let Some(rhs) = definition.rhs {
            let mut visited = HashSet::new();
            let mut pending = VecDeque::from([rhs]);
            while let Some(tree) = pending.pop_front() {
                if !visited.insert(tree) {
                    continue;
                }
                let Some(node) = nodes.get(&tree.index()) else {
                    continue;
                };
                if let Some(form) = expression_form(&node.kind) {
                    *histogram.entry(form.to_owned()).or_default() += 1;
                }
                pending.extend(term_expression_children(&node.kind));
            }
        }
    }
    histogram
}

fn term_expression_children(kind: &TreeKind<Untyped>) -> Vec<dotty_core::TreeId<Untyped>> {
    let mut children = Vec::new();
    match kind {
        TreeKind::Select(node) => children.push(node.qualifier),
        TreeKind::Super(node) => children.push(node.qual),
        TreeKind::Apply(node) => {
            children.push(node.function);
            children.extend(node.args.iter().copied());
        }
        TreeKind::TypeApply(node) => children.push(node.function),
        TreeKind::Typed(node) => children.push(node.expr),
        TreeKind::NamedArg(node) => children.push(node.arg),
        TreeKind::Assign(node) => children.extend([node.lhs, node.rhs]),
        TreeKind::Block(node) => {
            children.extend(node.stats.iter().copied());
            children.push(node.expr);
        }
        TreeKind::If(node) => children.extend([node.cond, node.then_branch, node.else_branch]),
        TreeKind::Match(node) => {
            children.push(node.selector);
            children.extend(node.cases.iter().copied());
        }
        TreeKind::CaseDef(node) => {
            if let Some(guard) = node.guard {
                children.push(guard);
            }
            children.push(node.body);
        }
        TreeKind::Return(node) => children.extend(node.expr),
        TreeKind::While(node) => children.extend([node.cond, node.body]),
        TreeKind::Try(node) => {
            children.push(node.expr);
            children.extend(node.cases.iter().copied());
            children.extend(node.finalizer);
        }
        TreeKind::Closure(node) => children.extend(node.env.iter().copied()),
        TreeKind::ValDef(node) => children.extend(node.rhs),
        TreeKind::DefDef(_) | TreeKind::TypeDef(_) | TreeKind::Template(_) => {}
        TreeKind::PhaseSpecific(UntypedNode::Function(node)) => children.push(node.body),
        TreeKind::PhaseSpecific(UntypedNode::PolyFunction(node)) => children.push(node.body),
        TreeKind::PhaseSpecific(UntypedNode::InfixOp(node)) => {
            children.extend([node.left, node.right]);
        }
        TreeKind::PhaseSpecific(UntypedNode::PrefixOp(node)) => children.push(node.operand),
        TreeKind::PhaseSpecific(UntypedNode::PostfixOp(node)) => children.push(node.operand),
        TreeKind::PhaseSpecific(UntypedNode::Parens(node)) => children.push(node.inner),
        TreeKind::PhaseSpecific(UntypedNode::Tuple(node)) => {
            children.extend(node.elements.iter().copied());
        }
        TreeKind::PhaseSpecific(UntypedNode::ForYield(node)) => {
            children.extend(node.enums.iter().copied());
            children.push(node.body);
        }
        TreeKind::PhaseSpecific(UntypedNode::ForDo(node)) => {
            children.extend(node.enums.iter().copied());
            children.push(node.body);
        }
        TreeKind::PhaseSpecific(UntypedNode::GenFrom(node)) => children.push(node.expr),
        TreeKind::PhaseSpecific(UntypedNode::GenAlias(node)) => children.push(node.expr),
        TreeKind::PhaseSpecific(UntypedNode::PatDef(node)) => children.extend(node.rhs),
        TreeKind::PhaseSpecific(UntypedNode::InterpolatedString(node)) => {
            children.extend(node.parts.iter().copied());
        }
        TreeKind::PhaseSpecific(UntypedNode::Throw(node)) => children.push(node.expr),
        TreeKind::PhaseSpecific(UntypedNode::ParsedTry(node)) => {
            children.push(node.expr);
            children.extend(node.handler);
            children.extend(node.finalizer);
        }
        TreeKind::PhaseSpecific(
            UntypedNode::Error(_)
            | UntypedNode::ModuleDef(_)
            | UntypedNode::FunctionWithMods(_)
            | UntypedNode::CapturesAndResult(_)
            | UntypedNode::ExtensionMethods(_)
            | UntypedNode::ContextBounds(_)
            | UntypedNode::ContextBoundTypeTree(_)
            | UntypedNode::Number(_),
        ) => {}
        TreeKind::Quote(node) => children.push(node.body),
        TreeKind::Splice(node) => children.push(node.expr),
        TreeKind::Inlined(node) => {
            children.extend(node.bindings.iter().copied());
            children.push(node.expansion);
        }
        TreeKind::Annotated(node) => children.push(node.expr),
        TreeKind::Ident(_)
        | TreeKind::This(_)
        | TreeKind::Literal(_)
        | TreeKind::New(_)
        | TreeKind::PackageDef(_)
        | TreeKind::Import(_)
        | TreeKind::Export(_)
        | TreeKind::TypeTree(_)
        | TreeKind::SingletonTypeTree(_)
        | TreeKind::AppliedTypeTree(_)
        | TreeKind::RefinedTypeTree(_)
        | TreeKind::LambdaTypeTree(_)
        | TreeKind::MatchTypeTree(_)
        | TreeKind::ByNameTypeTree(_)
        | TreeKind::TypeBoundsTree(_)
        | TreeKind::Bind(_)
        | TreeKind::Alternative(_)
        | TreeKind::UnApply(_)
        | TreeKind::QuotePattern(_)
        | TreeKind::SplicePattern(_) => {}
    }
    children
}

fn scala_files(roots: &[PathBuf]) -> Vec<PathBuf> {
    fn visit(path: &Path, files: &mut Vec<PathBuf>) {
        let Ok(metadata) = fs::metadata(path) else {
            return;
        };
        if metadata.is_file() {
            if path
                .extension()
                .is_some_and(|extension| extension == "scala")
            {
                files.push(path.to_owned());
            }
            return;
        }
        let Ok(entries) = fs::read_dir(path) else {
            return;
        };
        let mut entries = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .collect::<Vec<_>>();
        entries.sort();
        for entry in entries {
            visit(&entry, files);
        }
    }
    let mut files = Vec::new();
    for root in roots {
        visit(root, &mut files);
    }
    files.sort();
    files.dedup();
    files
}

fn git_revision(root: &Path) -> String {
    std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "HEAD"])
        .output()
        .expect("git should be available")
        .stdout
        .into_iter()
        .map(char::from)
        .collect::<String>()
        .trim()
        .to_owned()
}
