use std::collections::{BTreeMap, BTreeSet, HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};

use dotty_classloader::classloader::{
    ClassPathEntry, ClasspathSymbolResolver, CompositeClassPath, JarClassPath, JdkClassPath,
    LoadingSession,
};
use dotty_core::ast::{Match, Modifier, Tree, TreeKind, Untyped, UntypedNode, VisibilitySyntax};
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
    resolver_package_requests: usize,
    package_successes: usize,
    package_unresolved: usize,
    package_errors: usize,
    package_source_reuse: usize,
    resolver_member_requests: usize,
    member_successes: usize,
    class_symbol_successes: usize,
    member_source_reuse: usize,
    member_unresolved: usize,
    member_errors: usize,
    packages: BTreeSet<String>,
    classes: BTreeSet<String>,
    members: BTreeSet<String>,
    unresolved_member_names: BTreeMap<String, usize>,
    member_error_kinds: BTreeMap<String, usize>,
}

impl ResolverMetrics {
    fn external_package_requests(&self) -> usize {
        debug_assert!(self.resolver_package_requests >= self.package_source_reuse);
        self.resolver_package_requests - self.package_source_reuse
    }

    fn external_member_requests(&self) -> usize {
        debug_assert!(self.resolver_member_requests >= self.member_source_reuse);
        self.resolver_member_requests - self.member_source_reuse
    }
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
        metrics.resolver_member_requests += 1;
        match &result {
            Ok(Some(symbol)) => {
                let path = audit_symbol_path(store, *symbol);
                if matches!(
                    store.symbols.get(*symbol).origin,
                    dotty_core::SymbolOrigin::Source(_)
                ) {
                    metrics.member_source_reuse += 1;
                } else if is_external_symbol(store.symbols.get(*symbol).origin) {
                    metrics.member_successes += 1;
                    match store.symbols.get(*symbol).kind {
                        dotty_core::SymbolKind::Class
                        | dotty_core::SymbolKind::Trait
                        | dotty_core::SymbolKind::Object
                        | dotty_core::SymbolKind::ModuleClass => {
                            metrics.class_symbol_successes += 1;
                            metrics.classes.insert(path);
                        }
                        _ => {
                            metrics.members.insert(path);
                        }
                    }
                }
            }
            Ok(None) => {
                metrics.member_unresolved += 1;
                let name = store.names.resolve(request.name.text()).to_owned();
                *metrics.unresolved_member_names.entry(name).or_default() += 1;
            }
            Err(error) => {
                metrics.member_errors += 1;
                *metrics
                    .member_error_kinds
                    .entry(format!("{error:?}"))
                    .or_default() += 1;
            }
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
        metrics.resolver_package_requests += 1;
        match &result {
            Ok(Some(symbol)) => {
                if matches!(
                    store.symbols.get(*symbol).origin,
                    dotty_core::SymbolOrigin::Source(_)
                ) {
                    metrics.package_source_reuse += 1;
                } else {
                    metrics.package_successes += 1;
                    metrics.packages.insert(path.join("."));
                }
            }
            Ok(None) => metrics.package_unresolved += 1,
            Err(_) => metrics.package_errors += 1,
        }
        result
    }

    fn enter_synthetic_package_member(
        &mut self,
        store: &mut SemanticStore,
        package: SymbolId,
        name: dotty_core::Name,
        member: SymbolId,
    ) -> Result<bool, ResolutionError> {
        self.inner
            .enter_synthetic_package_member(store, package, name, member)
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

fn is_external_symbol(origin: dotty_core::SymbolOrigin) -> bool {
    matches!(
        origin,
        dotty_core::SymbolOrigin::Classfile(_) | dotty_core::SymbolOrigin::Tasty(_)
    )
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
struct RootFailure {
    range: TextRange,
    enclosing_method_tree: u32,
    failure: FailureClassification,
    missing_type_tree: Option<u32>,
    singleton_reference_tree: Option<u32>,
    local_method_signature: Option<(u32, String)>,
    local_value_declaration: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Audit {
    files_attempted: usize,
    parser_failed_files: usize,
    namer_failed_files: usize,
    file_read_failed_files: usize,
    file_read_failed_paths: BTreeSet<String>,
    recovered_parser_files: usize,
    local_definitions: usize,
    buckets: BTreeMap<String, usize>,
    local_defdefs: usize,
    typed_local_defdefs: usize,
    failures: BTreeMap<String, FailureBucket>,
    expression_forms: BTreeMap<String, usize>,
    prefix_operator_forms: BTreeMap<String, usize>,
    type_tree_forms: BTreeMap<String, usize>,
    type_tree_form_files: BTreeMap<String, BTreeSet<String>>,
    parser_diagnostics: BTreeMap<String, usize>,
    match_readiness: MatchReadiness,
    match_profile: MatchProfile,
    patdef_profile: PatDefProfile,
    missing_declared_type_profile: MissingDeclaredTypeProfile,
    local_method_first_blockers: BTreeMap<String, String>,
    local_value_blocker_profile: LocalValueBlockerProfile,
    by_name_method_outcomes: BTreeMap<String, String>,
    local_method_signature_profile: LocalMethodSignatureProfile,
    singleton_reference_profile: SingletonReferenceProfile,
    singleton_source_inventory: SingletonSourceInventory,
    source_function_method_outcomes: BTreeSet<String>,
    source_function_outcomes: BTreeMap<String, FailureBucket>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct MissingDeclaredTypeProfile {
    buckets: BTreeMap<String, FailureBucket>,
    records: BTreeSet<String>,
    declarations: BTreeSet<String>,
}

impl Default for Audit {
    fn default() -> Self {
        Self {
            files_attempted: 0,
            parser_failed_files: 0,
            namer_failed_files: 0,
            file_read_failed_files: 0,
            file_read_failed_paths: BTreeSet::new(),
            recovered_parser_files: 0,
            local_definitions: 0,
            buckets: BTreeMap::new(),
            local_defdefs: 0,
            typed_local_defdefs: 0,
            failures: BTreeMap::new(),
            expression_forms: empty_expression_histogram(),
            prefix_operator_forms: BTreeMap::new(),
            type_tree_forms: empty_type_tree_histogram(),
            type_tree_form_files: empty_type_tree_form_files(),
            parser_diagnostics: BTreeMap::new(),
            match_readiness: MatchReadiness::default(),
            match_profile: MatchProfile::default(),
            patdef_profile: PatDefProfile::default(),
            missing_declared_type_profile: MissingDeclaredTypeProfile::default(),
            local_method_first_blockers: BTreeMap::new(),
            local_value_blocker_profile: LocalValueBlockerProfile::default(),
            by_name_method_outcomes: BTreeMap::new(),
            local_method_signature_profile: LocalMethodSignatureProfile::default(),
            singleton_reference_profile: SingletonReferenceProfile::default(),
            singleton_source_inventory: SingletonSourceInventory::default(),
            source_function_method_outcomes: BTreeSet::new(),
            source_function_outcomes: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct LocalValueBlockerObservation {
    path: String,
    enclosing_method: String,
    enclosing_method_tree: u32,
    blocker_origin_method: String,
    declaration_tree: u32,
    line: usize,
    span: String,
    node_kind: String,
    declaration_name: String,
    declaration_kind: String,
    dispatch_path: String,
    modifiers: String,
    visibility: String,
    annotations: bool,
    type_form: String,
    rhs_present: bool,
    pattern_roots: String,
    source_pattern_count: Option<usize>,
    binder_count: Option<usize>,
    attribution: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct LocalValueBlockerProfile {
    observations: BTreeSet<LocalValueBlockerObservation>,
}

impl LocalValueBlockerProfile {
    fn record(&mut self, observation: LocalValueBlockerObservation) {
        self.observations.insert(observation);
    }

    fn distinct_declaration_count(&self) -> usize {
        self.observations
            .iter()
            .map(|row| (row.path.as_str(), row.declaration_tree))
            .collect::<BTreeSet<_>>()
            .len()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PatDefProfile {
    total: usize,
    structural_patdef_ids: HashSet<u32>,
    root_shapes: BTreeMap<String, usize>,
    root_shape_files: BTreeMap<String, BTreeSet<String>>,
    source_pattern_counts: BTreeMap<String, usize>,
    binder_counts: BTreeMap<String, usize>,
    modifiers: BTreeMap<String, usize>,
    explicit_tpt: BTreeMap<String, usize>,
    rhs_states: BTreeMap<String, usize>,
    typing_outcomes: BTreeMap<String, FailureBucket>,
    representative_files: BTreeSet<String>,
}

impl Default for PatDefProfile {
    fn default() -> Self {
        Self {
            total: 0,
            structural_patdef_ids: HashSet::new(),
            root_shapes: [
                "Tuple",
                "Apply / extractor-looking",
                "Bind",
                "InfixOp",
                "Typed",
                "Alternative",
                "Ident",
                "wildcard",
                "other",
            ]
            .into_iter()
            .map(|shape| (shape.to_owned(), 0))
            .collect(),
            root_shape_files: BTreeMap::new(),
            source_pattern_counts: BTreeMap::new(),
            binder_counts: ["0", "1", "2", "3+"]
                .into_iter()
                .map(|count| (count.to_owned(), 0))
                .collect(),
            modifiers: ["val", "var", "lazy val"]
                .into_iter()
                .map(|modifier| (modifier.to_owned(), 0))
                .collect(),
            explicit_tpt: ["explicit PatDef-wide tpt", "synthetic inferred TypeTree"]
                .into_iter()
                .map(|form| (form.to_owned(), 0))
                .collect(),
            rhs_states: ["present", "missing", "recovery error"]
                .into_iter()
                .map(|state| (state.to_owned(), 0))
                .collect(),
            typing_outcomes: BTreeMap::new(),
            representative_files: BTreeSet::new(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct MatchReadiness {
    first_blocker_methods: usize,
    matches: usize,
    cases: usize,
    guarded_cases: usize,
    first_blocker_errors: BTreeMap<String, usize>,
    first_blocker_error_files: BTreeMap<String, BTreeSet<String>>,
    pattern_shapes: BTreeMap<String, usize>,
    pattern_shape_files: BTreeMap<String, BTreeSet<String>>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct MatchProfile {
    matches: usize,
    cases: usize,
    guarded_cases: usize,
    pattern_roots: BTreeMap<String, usize>,
    pattern_root_files: BTreeMap<String, BTreeSet<String>>,
    typed_case_successes: BTreeMap<String, usize>,
    typed_case_success_files: BTreeMap<String, BTreeSet<String>>,
    typed_pattern_boundaries: BTreeMap<String, usize>,
    typed_case_failures: BTreeMap<String, usize>,
    typed_case_failure_files: BTreeMap<String, BTreeSet<String>>,
    extractor_protocol_successes: BTreeMap<String, usize>,
    extractor_roots: BTreeMap<String, usize>,
    extractor_dispatch: BTreeMap<String, usize>,
    extractor_type_applied: usize,
    extractor_argument_counts: BTreeMap<usize, usize>,
    extractor_nested_roots: BTreeMap<String, usize>,
    sequence_wildcards: usize,
    named_pattern_arguments: usize,
    empty_tuple_unit_patterns: usize,
    infix_pattern_forms: usize,
    extractor_files: BTreeSet<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct FailureBucket {
    family: FailureFamily,
    count: usize,
    files: BTreeSet<String>,
    examples: BTreeSet<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct LocalMethodSignatureFeatureBucket {
    count: usize,
    direct_methods: BTreeSet<String>,
    inherited_methods: BTreeSet<String>,
    files: BTreeSet<String>,
    methods: BTreeSet<String>,
    blocker_origins: BTreeSet<String>,
    records: BTreeSet<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct LocalMethodSignatureProfile {
    total: usize,
    features: BTreeMap<String, LocalMethodSignatureFeatureBucket>,
}

impl LocalMethodSignatureProfile {
    fn record(
        &mut self,
        feature: String,
        path: &str,
        method: String,
        blocker_origin: String,
        direct: bool,
        record: String,
    ) {
        let bucket = self.features.entry(feature).or_default();
        if bucket.methods.insert(method.clone()) {
            self.total += 1;
            bucket.count += 1;
        }
        if direct {
            bucket.direct_methods.insert(method);
        } else {
            bucket.inherited_methods.insert(method);
        }
        bucket.files.insert(path.to_owned());
        bucket.blocker_origins.insert(blocker_origin);
        bucket.records.insert(record);
    }

    fn merge(&mut self, other: Self) {
        self.total += other.total;
        for (feature, other_bucket) in other.features {
            let bucket = self.features.entry(feature).or_default();
            bucket.count += other_bucket.count;
            bucket.direct_methods.extend(other_bucket.direct_methods);
            bucket
                .inherited_methods
                .extend(other_bucket.inherited_methods);
            bucket.files.extend(other_bucket.files);
            bucket.methods.extend(other_bucket.methods);
            bucket.blocker_origins.extend(other_bucket.blocker_origins);
            bucket.records.extend(other_bucket.records);
        }
    }
}

impl Audit {
    fn merge(&mut self, other: Self) {
        self.files_attempted += other.files_attempted;
        self.parser_failed_files += other.parser_failed_files;
        self.namer_failed_files += other.namer_failed_files;
        self.file_read_failed_files += other.file_read_failed_files;
        self.file_read_failed_paths
            .extend(other.file_read_failed_paths);
        self.recovered_parser_files += other.recovered_parser_files;
        self.local_definitions += other.local_definitions;
        self.local_defdefs += other.local_defdefs;
        self.typed_local_defdefs += other.typed_local_defdefs;
        for (name, count) in other.expression_forms {
            *self.expression_forms.entry(name).or_default() += count;
        }
        merge_counts(&mut self.prefix_operator_forms, other.prefix_operator_forms);
        for (name, count) in other.type_tree_forms {
            *self.type_tree_forms.entry(name).or_default() += count;
        }
        for (name, files) in other.type_tree_form_files {
            self.type_tree_form_files
                .entry(name)
                .or_default()
                .extend(files);
        }
        for (name, count) in other.parser_diagnostics {
            *self.parser_diagnostics.entry(name).or_default() += count;
        }
        self.match_readiness.merge(other.match_readiness);
        self.match_profile.merge(other.match_profile);
        self.patdef_profile.merge(other.patdef_profile);
        self.missing_declared_type_profile
            .merge(other.missing_declared_type_profile);
        self.local_method_first_blockers
            .extend(other.local_method_first_blockers);
        self.local_value_blocker_profile
            .observations
            .extend(other.local_value_blocker_profile.observations);
        self.by_name_method_outcomes
            .extend(other.by_name_method_outcomes);
        self.local_method_signature_profile
            .merge(other.local_method_signature_profile);
        self.singleton_reference_profile
            .merge(other.singleton_reference_profile);
        self.singleton_source_inventory
            .merge(other.singleton_source_inventory);
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
        self.source_function_method_outcomes
            .extend(other.source_function_method_outcomes);
        for (key, bucket) in other.source_function_outcomes {
            let target = self.source_function_outcomes.entry(key).or_default();
            target.family = bucket.family;
            target.count += bucket.count;
            target.files.extend(bucket.files);
            target.examples.extend(bucket.examples);
            target.examples = target.examples.iter().take(5).cloned().collect();
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct SingletonReferenceProfile {
    observations: usize,
    first_blockers: BTreeSet<String>,
    singleton_source_trees: BTreeSet<String>,
    enclosing_declarations: BTreeSet<String>,
    reference_shapes: BTreeSet<String>,
}

impl SingletonReferenceProfile {
    fn merge(&mut self, other: Self) {
        self.observations += other.observations;
        self.first_blockers.extend(other.first_blockers);
        self.singleton_source_trees
            .extend(other.singleton_source_trees);
        self.enclosing_declarations
            .extend(other.enclosing_declarations);
        self.reference_shapes.extend(other.reference_shapes);
    }

    fn record(&mut self, detail: SingletonReferenceDetail) {
        self.observations += 1;
        self.first_blockers.insert(detail.first_blocker);
        self.singleton_source_trees.insert(detail.singleton_tree);
        self.enclosing_declarations
            .insert(detail.enclosing_declaration);
        self.reference_shapes.insert(detail.reference_shape);
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct SingletonSourceInventory {
    total: usize,
    categories: BTreeMap<String, usize>,
    category_files: BTreeMap<String, BTreeSet<String>>,
    reference_shapes: BTreeMap<String, usize>,
    reference_shape_files: BTreeMap<String, BTreeSet<String>>,
}

impl SingletonSourceInventory {
    fn merge(&mut self, other: Self) {
        self.total += other.total;
        merge_counts(&mut self.categories, other.categories);
        for (category, files) in other.category_files {
            self.category_files
                .entry(category)
                .or_default()
                .extend(files);
        }
        merge_counts(&mut self.reference_shapes, other.reference_shapes);
        for (shape, files) in other.reference_shape_files {
            self.reference_shape_files
                .entry(shape)
                .or_default()
                .extend(files);
        }
    }

    fn record(&mut self, category: String, shape: String, path: &str) {
        self.total += 1;
        *self.categories.entry(category.clone()).or_default() += 1;
        self.category_files
            .entry(category)
            .or_default()
            .insert(path.to_owned());
        *self.reference_shapes.entry(shape.clone()).or_default() += 1;
        self.reference_shape_files
            .entry(shape)
            .or_default()
            .insert(path.to_owned());
    }
}

#[derive(Debug)]
struct SingletonReferenceDetail {
    first_blocker: String,
    singleton_tree: String,
    enclosing_declaration: String,
    reference_shape: String,
}

impl MissingDeclaredTypeProfile {
    fn merge(&mut self, other: Self) {
        for (key, bucket) in other.buckets {
            let target = self.buckets.entry(key).or_default();
            target.family = bucket.family;
            target.count += bucket.count;
            target.files.extend(bucket.files);
            target.examples.extend(bucket.examples);
        }
        self.records.extend(other.records);
        self.declarations.extend(other.declarations);
    }

    fn record(&mut self, key: String, path: &str, detail: String, declaration: String) {
        let bucket = self.buckets.entry(key).or_default();
        bucket.family = FailureFamily::TypeRelationInferenceCompletion;
        bucket.count += 1;
        bucket.files.insert(path.to_owned());
        bucket.examples.insert(path.to_owned());
        self.records.insert(detail);
        self.declarations.insert(declaration);
    }
}

impl PatDefProfile {
    fn merge(&mut self, other: Self) {
        self.total += other.total;
        merge_counts(&mut self.root_shapes, other.root_shapes);
        merge_file_sets(&mut self.root_shape_files, other.root_shape_files);
        merge_counts(&mut self.source_pattern_counts, other.source_pattern_counts);
        merge_counts(&mut self.binder_counts, other.binder_counts);
        merge_counts(&mut self.modifiers, other.modifiers);
        merge_counts(&mut self.explicit_tpt, other.explicit_tpt);
        merge_counts(&mut self.rhs_states, other.rhs_states);
        for (key, bucket) in other.typing_outcomes {
            let target = self.typing_outcomes.entry(key).or_default();
            target.family = bucket.family;
            target.count += bucket.count;
            target.files.extend(bucket.files);
            target.examples.extend(bucket.examples);
            target.examples = target.examples.iter().take(5).cloned().collect();
        }
        self.representative_files.extend(other.representative_files);
    }
}

impl MatchReadiness {
    fn merge(&mut self, other: Self) {
        self.first_blocker_methods += other.first_blocker_methods;
        self.matches += other.matches;
        self.cases += other.cases;
        self.guarded_cases += other.guarded_cases;
        merge_counts(&mut self.first_blocker_errors, other.first_blocker_errors);
        for (error, files) in other.first_blocker_error_files {
            self.first_blocker_error_files
                .entry(error)
                .or_default()
                .extend(files);
        }
        for (shape, count) in other.pattern_shapes {
            *self.pattern_shapes.entry(shape).or_default() += count;
        }
        for (shape, files) in other.pattern_shape_files {
            self.pattern_shape_files
                .entry(shape)
                .or_default()
                .extend(files);
        }
    }
}

impl MatchProfile {
    fn merge(&mut self, other: Self) {
        self.matches += other.matches;
        self.cases += other.cases;
        self.guarded_cases += other.guarded_cases;
        self.sequence_wildcards += other.sequence_wildcards;
        self.named_pattern_arguments += other.named_pattern_arguments;
        self.empty_tuple_unit_patterns += other.empty_tuple_unit_patterns;
        self.infix_pattern_forms += other.infix_pattern_forms;
        self.extractor_type_applied += other.extractor_type_applied;
        self.extractor_files.extend(other.extractor_files);
        merge_counts(&mut self.pattern_roots, other.pattern_roots);
        merge_counts(&mut self.typed_case_successes, other.typed_case_successes);
        merge_file_sets(
            &mut self.typed_case_success_files,
            other.typed_case_success_files,
        );
        merge_counts(
            &mut self.typed_pattern_boundaries,
            other.typed_pattern_boundaries,
        );
        merge_counts(&mut self.typed_case_failures, other.typed_case_failures);
        merge_file_sets(
            &mut self.typed_case_failure_files,
            other.typed_case_failure_files,
        );
        merge_counts(
            &mut self.extractor_protocol_successes,
            other.extractor_protocol_successes,
        );
        merge_counts(&mut self.extractor_roots, other.extractor_roots);
        merge_counts(&mut self.extractor_dispatch, other.extractor_dispatch);
        merge_counts(
            &mut self.extractor_argument_counts,
            other.extractor_argument_counts,
        );
        merge_counts(
            &mut self.extractor_nested_roots,
            other.extractor_nested_roots,
        );
        for (shape, files) in other.pattern_root_files {
            self.pattern_root_files
                .entry(shape)
                .or_default()
                .extend(files);
        }
    }
}

fn merge_file_sets<K: Ord>(
    target: &mut BTreeMap<K, BTreeSet<String>>,
    source: BTreeMap<K, BTreeSet<String>>,
) {
    for (key, files) in source {
        target.entry(key).or_default().extend(files);
    }
}

fn merge_counts<K: Ord>(target: &mut BTreeMap<K, usize>, source: BTreeMap<K, usize>) {
    for (key, count) in source {
        *target.entry(key).or_default() += count;
    }
}

#[test]
#[ignore = "run tools/typer-classpath-corpus-audit/run with the pinned Scala checkout and explicit JAVA_HOME"]
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
            .unwrap_or(file)
            .to_string_lossy()
            .replace('\\', "/");
        let source = match fs::read_to_string(file) {
            Ok(source) => source,
            Err(_) => {
                audit.files_attempted += 1;
                audit.file_read_failed_files += 1;
                audit.file_read_failed_paths.insert(relative);
                continue;
            }
        };
        audit.merge(audit_source_with_classpath(
            &source,
            &relative,
            classpath.clone(),
            Rc::clone(&resolver_metrics),
        ));
    }

    assert_eq!(
        audit
            .source_function_outcomes
            .get("expression::Function::UnsupportedFunctionLiteralParameter")
            .map(|bucket| bucket.count),
        Some(9),
        "the same nine inferred-lambda baseline methods should remain explicitly deferred"
    );
    assert_eq!(
        audit
            .source_function_outcomes
            .get("expression::Function::ImportQualifierNotFound")
            .map(|bucket| bucket.count),
        Some(4),
        "the four Scala2Unpickler lambda methods should expose their classpath blocker"
    );
    assert_eq!(
        audit
            .source_function_outcomes
            .get("type::Function::SymbolResolution")
            .map(|bucket| bucket.count),
        Some(12),
        "all ordinary function-type baseline methods should retain their resolver blocker"
    );
    assert_eq!(audit.source_function_method_outcomes.len(), 25);

    let missing_declared_type = audit
        .failures
        .get("MissingDeclaredType")
        .expect("the pinned audit should retain MissingDeclaredType first blockers");
    assert_eq!(missing_declared_type.count, 10);
    assert_eq!(missing_declared_type.files.len(), 3);
    assert_eq!(
        audit.missing_declared_type_profile.records.len(),
        missing_declared_type.count,
        "every existing MissingDeclaredType occurrence must have one profile row"
    );
    assert_eq!(
        audit
            .missing_declared_type_profile
            .buckets
            .values()
            .map(|bucket| bucket.count)
            .sum::<usize>(),
        missing_declared_type.count,
        "profile buckets must preserve the top-level MissingDeclaredType count"
    );
    let singleton_reference_failure_count = audit
        .failures
        .get("UnsupportedSingletonReference")
        .map_or(0, |bucket| bucket.count);
    assert_eq!(
        audit.singleton_reference_profile.observations, singleton_reference_failure_count,
        "every singleton-reference blocker must have a profile observation"
    );
    assert_eq!(
        audit.singleton_reference_profile.first_blockers.len(),
        singleton_reference_failure_count,
        "every singleton-reference blocker must have a distinct profile row"
    );
    let local_method_signature_failure_count = audit
        .failures
        .get("LocalMethodSignatureDeferred")
        .map_or(0, |bucket| bucket.count);
    let local_value_blocker_count = audit
        .failures
        .get("LocalBlockDeclarationDeferred::val/var definition")
        .map_or(0, |bucket| bucket.count);
    assert_eq!(local_value_blocker_count, 21);
    assert_eq!(audit.local_value_blocker_profile.observations.len(), 21);
    assert_eq!(
        audit
            .local_value_blocker_profile
            .distinct_declaration_count(),
        14
    );
    assert_eq!(
        audit
            .local_value_blocker_profile
            .observations
            .iter()
            .map(|row| row.path.as_str())
            .collect::<BTreeSet<_>>()
            .len(),
        8
    );
    let rows = &audit.local_value_blocker_profile.observations;
    assert!(rows.iter().all(|row| row.node_kind == "ValDef"));
    assert!(rows.iter().all(|row| row.rhs_present));
    assert!(rows.iter().all(|row| !row.annotations));
    assert!(rows.iter().all(|row| row.visibility == "default"));
    assert!(rows.iter().all(|row| row.attribution == "inherited"));
    let count_kind = |kind: &str| {
        rows.iter()
            .filter(|row| row.declaration_kind == kind)
            .count()
    };
    assert_eq!(count_kind("given"), 5);
    assert_eq!(count_kind("implicit val"), 12);
    assert_eq!(count_kind("inline val"), 1);
    assert_eq!(count_kind("lazy val"), 3);
    assert_eq!(
        rows.iter()
            .filter(|row| row.type_form == "explicit")
            .count(),
        7
    );
    assert_eq!(
        rows.iter()
            .filter(|row| row.type_form == "inferred")
            .count(),
        14
    );
    let count_modifier = |modifier: &str| {
        rows.iter()
            .filter(|row| row.modifiers.split(',').any(|value| value == modifier))
            .count()
    };
    assert_eq!(count_modifier("Given"), 5);
    assert_eq!(count_modifier("Implicit"), 12);
    assert_eq!(count_modifier("Lazy"), 8);
    assert_eq!(count_modifier("Inline"), 1);
    assert_eq!(count_modifier("Final"), 5);
    assert_eq!(count_modifier("Var"), 0);
    assert_eq!(
        audit.local_method_signature_profile.total, local_method_signature_failure_count,
        "every LocalMethodSignatureDeferred first blocker must retain its exact payload and source shape"
    );
    assert_eq!(local_method_signature_failure_count, 0);
    assert_eq!(audit.local_method_signature_profile.features.len(), 0);
    assert!(
        !audit
            .local_method_signature_profile
            .features
            .contains_key("by-name parameters"),
        "the by-name signature blocker should be resolved by the local-method signature increment"
    );
    let local_method_signature_files = audit
        .local_method_signature_profile
        .features
        .values()
        .flat_map(|bucket| bucket.files.iter())
        .collect::<BTreeSet<_>>();
    assert_eq!(local_method_signature_files.len(), 0);
    let inline_signature_methods = [
        (
            "compiler/src/dotty/tools/dotc/transform/MegaPhase.scala",
            1125,
        ),
        (
            "compiler/src/dotty/tools/dotc/transform/MegaPhase.scala",
            1440,
        ),
        (
            "compiler/src/dotty/tools/dotc/transform/MegaPhase.scala",
            1224,
        ),
        (
            "compiler/src/dotty/tools/dotc/transform/MegaPhase.scala",
            1301,
        ),
        (
            "compiler/src/dotty/tools/dotc/transform/MegaPhase.scala",
            2407,
        ),
        (
            "compiler/src/dotty/tools/dotc/transform/MegaPhase.scala",
            2116,
        ),
    ];
    for (path, tree_index) in inline_signature_methods {
        let outcome = audit
            .local_method_first_blockers
            .get(&format!("{path}#tree={tree_index}"))
            .unwrap_or_else(|| panic!("missing post-#902 outcome for {path}#tree={tree_index}"));
        assert_ne!(
            outcome, "LocalMethodSignatureDeferred",
            "inline signature row {path}#tree={tree_index} should move past its old blocker"
        );
    }
    let baseline_signature_methods = [
        ("compiler/src/dotty/tools/dotc/core/SymUtils.scala", 1839),
        ("compiler/src/dotty/tools/dotc/core/SymUtils.scala", 1874),
        ("compiler/src/dotty/tools/dotc/typer/Typer.scala", 5163),
        ("compiler/src/dotty/tools/dotc/typer/Typer.scala", 5232),
        ("compiler/src/dotty/tools/dotc/typer/Typer.scala", 5296),
        ("compiler/src/dotty/tools/dotc/typer/Typer.scala", 5380),
        ("compiler/src/dotty/tools/dotc/typer/Typer.scala", 5420),
        ("compiler/src/dotty/tools/dotc/typer/Typer.scala", 5491),
        (
            "compiler/src/dotty/tools/dotc/transform/MegaPhase.scala",
            1125,
        ),
        (
            "compiler/src/dotty/tools/dotc/transform/MegaPhase.scala",
            1440,
        ),
        (
            "compiler/src/dotty/tools/dotc/transform/MegaPhase.scala",
            1224,
        ),
        (
            "compiler/src/dotty/tools/dotc/transform/MegaPhase.scala",
            1301,
        ),
        (
            "compiler/src/dotty/tools/dotc/transform/MegaPhase.scala",
            2407,
        ),
        (
            "compiler/src/dotty/tools/dotc/transform/MegaPhase.scala",
            2116,
        ),
    ];
    for (path, tree_index) in baseline_signature_methods {
        let key = format!("{path}#tree={tree_index}");
        assert_eq!(
            audit
                .local_method_first_blockers
                .get(&key)
                .map(String::as_str),
            Some("ImportQualifierNotFound"),
            "the #899 baseline method should retain its measured current first blocker: {key}"
        );
    }
    assert_eq!(audit.by_name_method_outcomes.len(), 2);
    assert_eq!(
        audit
            .by_name_method_outcomes
            .get("compiler/src/dotty/tools/dotc/core/SymUtils.scala::instantiateCFT")
            .map(String::as_str),
        Some("ImportQualifierNotFound")
    );
    assert_eq!(
        audit
            .by_name_method_outcomes
            .get("compiler/src/dotty/tools/dotc/typer/Typer.scala::cases")
            .map(String::as_str),
        Some("ImportQualifierNotFound")
    );
    let singleton_projection_outcomes =
        singleton_projection_baseline_outcomes(&audit.local_method_first_blockers);
    let singleton_no_longer_first_blocked = singleton_projection_outcomes
        .values()
        .filter(|outcome| outcome.as_str() != "UnsupportedSingletonReference")
        .count();
    assert_eq!(singleton_projection_outcomes.len(), 22);
    assert_eq!(
        singleton_no_longer_first_blocked, 22,
        "no #880 singleton baseline method may retain UnsupportedSingletonReference as its first blocker"
    );

    println!("AUDIT_REPORT_BEGIN");
    println!("scala_revision={revision}");
    println!("files={}", audit.files_attempted);
    println!("parser_failed_files={}", audit.parser_failed_files);
    println!("namer_failed_files={}", audit.namer_failed_files);
    println!("file_read_failed_files={}", audit.file_read_failed_files);
    println!(
        "file_read_failed_paths={}",
        audit
            .file_read_failed_paths
            .iter()
            .cloned()
            .collect::<Vec<_>>()
            .join(", ")
    );
    println!("recovered_parser_files={}", audit.recovered_parser_files);
    print_v1_deltas(&audit);
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
    println!("prefix_operator_forms:");
    for (operator, count) in &audit.prefix_operator_forms {
        println!("  operator={operator:?} count={count}");
    }
    println!("type_tree_forms:");
    for (form, count) in &audit.type_tree_forms {
        println!("  {form}={count}");
    }
    println!("type_tree_form_files:");
    for (form, files) in &audit.type_tree_form_files {
        println!(
            "  {form}={} [{}]",
            files.len(),
            files.iter().take(5).cloned().collect::<Vec<_>>().join(", ")
        );
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
    print_local_patdefs(&audit.patdef_profile);
    print_local_value_blocker_profile(&audit.local_value_blocker_profile);
    println!(
        "unsupported_expression_total={}",
        sum_buckets_with_prefix(&audit.failures, "UnsupportedExpression::")
    );
    println!("expression_sprint_first_blockers:");
    for (name, bucket) in [
        (
            "UnsupportedExpression::PrefixOp",
            audit.failures.get("UnsupportedExpression::PrefixOp"),
        ),
        (
            "UnsupportedExpression::Annotated",
            audit.failures.get("UnsupportedExpression::Annotated"),
        ),
        ("MemberNotFound", audit.failures.get("MemberNotFound")),
        ("MemberLookup", audit.failures.get("MemberLookup")),
        ("TypeNameNotFound", audit.failures.get("TypeNameNotFound")),
        (
            "SourceAnnotationClassDeferred",
            audit.failures.get("SourceAnnotationClassDeferred"),
        ),
        (
            "SourceAnnotationNotAnnotationClass",
            audit.failures.get("SourceAnnotationNotAnnotationClass"),
        ),
        (
            "SourceAnnotationArgumentNotConstant",
            audit.failures.get("SourceAnnotationArgumentNotConstant"),
        ),
        (
            "SourceAnnotationConstructorDeferred",
            audit.failures.get("SourceAnnotationConstructorDeferred"),
        ),
        (
            "SourceAnnotationConstructorArgumentMismatch",
            audit
                .failures
                .get("SourceAnnotationConstructorArgumentMismatch"),
        ),
        (
            "SourceAnnotationArgumentTypeDeferred",
            audit.failures.get("SourceAnnotationArgumentTypeDeferred"),
        ),
        (
            "ExpressionTypeCannotBeWidened",
            audit.failures.get("ExpressionTypeCannotBeWidened"),
        ),
        (
            "ExpectedExpressionTypeMismatch",
            audit.failures.get("ExpectedExpressionTypeMismatch"),
        ),
        (
            "ExpectedExpressionConformanceUnsupported",
            audit
                .failures
                .get("ExpectedExpressionConformanceUnsupported"),
        ),
    ] {
        println!(
            "  {name}={} files={}",
            bucket.map_or(0, |bucket| bucket.count),
            bucket.map_or(0, |bucket| bucket.files.len())
        );
    }
    let unsupported_type_tree_files = audit
        .failures
        .iter()
        .filter(|(name, _)| name.starts_with("UnsupportedTypeTree::"))
        .flat_map(|(_, bucket)| bucket.files.iter().cloned())
        .collect::<BTreeSet<_>>();
    println!(
        "UnsupportedTypeTree={} files={}",
        sum_buckets_with_prefix(&audit.failures, "UnsupportedTypeTree::"),
        unsupported_type_tree_files.len()
    );
    println!("unsupported_type_tree_failures:");
    for (name, bucket) in ordered_type_tree_failures(&audit) {
        println!(
            "  {name}: count={}, files={} [{}]",
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
    println!("source_function_method_outcomes:");
    for (name, bucket) in &audit.source_function_outcomes {
        println!(
            "  {name}: count={}, files={}, examples=[{}]",
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
    for record in &audit.source_function_method_outcomes {
        println!("  method={record}");
    }
    print_missing_declared_type_profile(&audit.missing_declared_type_profile);
    print_ranked_gaps(&audit.failures);
    print_match_readiness(&audit.match_readiness);
    print_match_profile(&audit.match_profile);
    print_resolver_metrics(&resolver_metrics.borrow());
    print_pinned_immutable_field_completion_outcomes(&root, classpath.clone());
    print_pinned_mutable_field_completion_outcomes(&root, classpath);
    print_pinned_immutable_field_method_outcomes(&audit.local_method_first_blockers);
    print_pinned_mutable_field_method_outcomes(&audit.local_method_first_blockers);
    print_local_method_signature_profile(&audit.local_method_signature_profile);
    print_inline_signature_outcomes(&audit.local_method_first_blockers);
    println!("by_name_method_outcomes:");
    for (method, outcome) in &audit.by_name_method_outcomes {
        println!("  {method}={outcome}");
    }
    print_singleton_reference_profile(&audit.singleton_reference_profile);
    print_singleton_source_inventory(&audit.singleton_source_inventory);
    print_singleton_projection_baseline(&singleton_projection_outcomes);
    println!("AUDIT_REPORT_END");
}

fn print_singleton_reference_profile(profile: &SingletonReferenceProfile) {
    println!("singleton_reference_profile:");
    println!("  total_first_blockers={}", profile.observations);
    println!("  profile_entries={}", profile.first_blockers.len());
    println!(
        "  distinct_singleton_source_trees={}",
        profile.singleton_source_trees.len()
    );
    println!(
        "  distinct_enclosing_declarations={}",
        profile.enclosing_declarations.len()
    );
    println!(
        "  distinct_reference_shapes={}",
        profile.reference_shapes.len()
    );
    println!("  first_blockers:");
    for record in &profile.first_blockers {
        println!("    {record}");
    }
    println!("  singleton_source_trees:");
    for tree in &profile.singleton_source_trees {
        println!("    {tree}");
    }
    println!("  enclosing_declarations:");
    for declaration in &profile.enclosing_declarations {
        println!("    {declaration}");
    }
    println!("  reference_shapes:");
    for shape in &profile.reference_shapes {
        println!("    {shape}");
    }
}

fn print_local_method_signature_profile(profile: &LocalMethodSignatureProfile) {
    println!("local_method_signature_profile:");
    println!("  total_occurrences={}", profile.total);
    println!("  distinct_files={}", {
        profile
            .features
            .values()
            .flat_map(|bucket| bucket.files.iter())
            .collect::<BTreeSet<_>>()
            .len()
    });
    let mut features = profile.features.iter().collect::<Vec<_>>();
    features.sort_by(|(feature_a, a), (feature_b, b)| {
        b.count.cmp(&a.count).then(feature_a.cmp(feature_b))
    });
    println!("  features:");
    for (feature, bucket) in features {
        println!(
            "    {feature:?}: affected_local_methods={} files={} distinct_methods={} direct_origins={} inherited_methods={} blocker_origins={} representatives=[{}]",
            bucket.count,
            bucket.files.len(),
            bucket.methods.len(),
            bucket.direct_methods.len(),
            bucket.inherited_methods.len(),
            bucket.blocker_origins.len(),
            bucket
                .records
                .iter()
                .take(3)
                .cloned()
                .collect::<Vec<_>>()
                .join(" | ")
        );
    }
    println!("  records:");
    for record in profile
        .features
        .values()
        .flat_map(|bucket| bucket.records.iter())
    {
        println!("    {record}");
    }
}

fn print_inline_signature_outcomes(outcomes: &BTreeMap<String, String>) {
    const METHODS: [(&str, u32); 6] = [
        (
            "compiler/src/dotty/tools/dotc/transform/MegaPhase.scala",
            1125,
        ),
        (
            "compiler/src/dotty/tools/dotc/transform/MegaPhase.scala",
            1440,
        ),
        (
            "compiler/src/dotty/tools/dotc/transform/MegaPhase.scala",
            1224,
        ),
        (
            "compiler/src/dotty/tools/dotc/transform/MegaPhase.scala",
            1301,
        ),
        (
            "compiler/src/dotty/tools/dotc/transform/MegaPhase.scala",
            2407,
        ),
        (
            "compiler/src/dotty/tools/dotc/transform/MegaPhase.scala",
            2116,
        ),
    ];

    println!("inline_parameter_signature_outcomes_after_902:");
    for (path, tree_index) in METHODS {
        let key = format!("{path}#tree={tree_index}");
        let outcome = outcomes
            .get(&key)
            .unwrap_or_else(|| panic!("missing post-#902 outcome for {key}"));
        println!("  {key}={outcome}");
    }
}

fn print_singleton_source_inventory(inventory: &SingletonSourceInventory) {
    println!("singleton_source_inventory:");
    println!("  total_singleton_type_trees={}", inventory.total);
    for category in ["literal", "this", "identifier", "selection", "unsupported"] {
        let count = inventory
            .categories
            .get(category)
            .copied()
            .unwrap_or_default();
        let files = inventory.category_files.get(category);
        let file_count = files.map_or(0, BTreeSet::len);
        let examples = files
            .into_iter()
            .flat_map(|files| files.iter().take(5).cloned())
            .collect::<Vec<_>>()
            .join(", ");
        println!("  {category}_references={count} files={file_count} examples=[{examples}]");
    }
    println!("  reference_shapes:");
    for (shape, count) in &inventory.reference_shapes {
        let files = inventory.reference_shape_files.get(shape);
        let file_count = files.map_or(0, BTreeSet::len);
        let examples = files
            .into_iter()
            .flat_map(|files| files.iter().take(5).cloned())
            .collect::<Vec<_>>()
            .join(", ");
        println!("    {shape}={count} files={file_count} examples=[{examples}]");
    }
}

const BASELINE_LITERAL_SINGLETON_METHODS: [(&str, u32); 22] = [
    (
        "compiler/src/dotty/tools/dotc/transform/CheckUnused.scala",
        2930,
    ),
    (
        "compiler/src/dotty/tools/dotc/transform/CheckUnused.scala",
        2955,
    ),
    (
        "compiler/src/dotty/tools/dotc/transform/CheckUnused.scala",
        3022,
    ),
    (
        "compiler/src/dotty/tools/dotc/transform/CheckUnused.scala",
        3107,
    ),
    (
        "compiler/src/dotty/tools/dotc/transform/CheckUnused.scala",
        3331,
    ),
    (
        "compiler/src/dotty/tools/dotc/transform/CheckUnused.scala",
        3155,
    ),
    (
        "compiler/src/dotty/tools/dotc/transform/CheckUnused.scala",
        3313,
    ),
    (
        "compiler/src/dotty/tools/dotc/transform/CheckUnused.scala",
        3396,
    ),
    (
        "compiler/src/dotty/tools/dotc/transform/CheckUnused.scala",
        3577,
    ),
    (
        "compiler/src/dotty/tools/dotc/transform/CheckUnused.scala",
        3468,
    ),
    (
        "compiler/src/dotty/tools/dotc/transform/CheckUnused.scala",
        3619,
    ),
    (
        "compiler/src/dotty/tools/dotc/transform/CheckUnused.scala",
        3799,
    ),
    (
        "compiler/src/dotty/tools/dotc/transform/CheckUnused.scala",
        4881,
    ),
    (
        "compiler/src/dotty/tools/dotc/transform/CheckUnused.scala",
        3817,
    ),
    (
        "compiler/src/dotty/tools/dotc/transform/CheckUnused.scala",
        3865,
    ),
    (
        "compiler/src/dotty/tools/dotc/transform/CheckUnused.scala",
        4084,
    ),
    (
        "compiler/src/dotty/tools/dotc/transform/CheckUnused.scala",
        4125,
    ),
    (
        "compiler/src/dotty/tools/dotc/transform/CheckUnused.scala",
        4138,
    ),
    (
        "compiler/src/dotty/tools/dotc/transform/CheckUnused.scala",
        4149,
    ),
    (
        "compiler/src/dotty/tools/dotc/transform/CheckUnused.scala",
        4202,
    ),
    (
        "compiler/src/dotty/tools/dotc/transform/CheckUnused.scala",
        4181,
    ),
    (
        "compiler/src/dotty/tools/dotc/transform/CheckUnused.scala",
        4981,
    ),
];

fn singleton_projection_baseline_outcomes(
    current_outcomes: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    BASELINE_LITERAL_SINGLETON_METHODS
        .into_iter()
        .map(|(path, tree_index)| {
            let key = format!("{path}#tree={tree_index}");
            let outcome = current_outcomes
                .get(&key)
                .unwrap_or_else(|| panic!("missing pinned singleton baseline method {key}"))
                .clone();
            (key, outcome)
        })
        .collect()
}

fn print_singleton_projection_baseline(outcomes: &BTreeMap<String, String>) {
    let moved = outcomes
        .values()
        .filter(|outcome| outcome.as_str() != "UnsupportedSingletonReference")
        .count();
    let mut blockers = BTreeMap::<&str, usize>::new();
    for outcome in outcomes.values() {
        *blockers.entry(outcome.as_str()).or_default() += 1;
    }
    println!("singleton_projection_baseline:");
    println!("  baseline_observations={}", outcomes.len());
    println!("  no_longer_first_blocked_by_singleton_projection={moved}");
    println!(
        "  remaining_UnsupportedSingletonReference={}",
        outcomes.len() - moved
    );
    println!("  current_first_blockers:");
    for (blocker, count) in blockers {
        println!("    {blocker}={count}");
    }
    println!("  methods:");
    for (method, outcome) in outcomes {
        println!("    {method} first_blocker={outcome}");
    }
}

fn print_pinned_mutable_field_method_outcomes(outcomes: &BTreeMap<String, String>) {
    const BASELINE_ATTEMPTS: [(&str, u32); 10] = [
        ("compiler/src/dotty/tools/dotc/cc/CheckCaptures.scala", 5089),
        ("compiler/src/dotty/tools/dotc/cc/CheckCaptures.scala", 5879),
        ("compiler/src/dotty/tools/dotc/parsing/Scanners.scala", 1474),
        ("compiler/src/dotty/tools/dotc/parsing/Scanners.scala", 998),
        ("compiler/src/dotty/tools/dotc/reporting/Message.scala", 291),
        ("compiler/src/dotty/tools/dotc/reporting/Message.scala", 349),
        (
            "compiler/src/dotty/tools/dotc/typer/Applications.scala",
            4248,
        ),
        ("compiler/src/dotty/tools/dotc/util/WeakHashSet.scala", 708),
        ("library/src/scala/collection/Iterator.scala", 3805),
        ("library/src/scala/collection/Iterator.scala", 3865),
    ];

    println!("mutable_class_field_baseline_method_outcomes:");
    for (path, method_tree) in BASELINE_ATTEMPTS {
        let key = format!("{path}#tree={method_tree}");
        let outcome = outcomes
            .get(&key)
            .unwrap_or_else(|| panic!("mutable baseline local method attempt is missing: {key}"));
        println!("  {key} outcome={outcome}");
    }
}

fn print_pinned_immutable_field_method_outcomes(outcomes: &BTreeMap<String, String>) {
    const BASELINE_ATTEMPTS: [(&str, u32); 14] = [
        ("compiler/src/dotty/tools/dotc/core/TypeErrors.scala", 695),
        ("compiler/src/dotty/tools/dotc/inlines/Inliner.scala", 1538),
        (
            "compiler/src/dotty/tools/dotc/printing/ReplPrinter.scala",
            396,
        ),
        ("compiler/src/dotty/tools/dotc/reporting/Profile.scala", 470),
        ("compiler/src/dotty/tools/dotc/reporting/Profile.scala", 531),
        ("compiler/src/dotty/tools/dotc/reporting/Profile.scala", 549),
        ("compiler/src/dotty/tools/dotc/reporting/Profile.scala", 660),
        ("compiler/src/dotty/tools/dotc/reporting/Profile.scala", 811),
        ("compiler/src/dotty/tools/dotc/rewrites/Rewrites.scala", 235),
        ("compiler/src/dotty/tools/dotc/rewrites/Rewrites.scala", 292),
        ("compiler/src/dotty/tools/dotc/transform/Bridges.scala", 204),
        ("compiler/src/dotty/tools/dotc/transform/Bridges.scala", 212),
        ("compiler/src/dotty/tools/dotc/transform/Bridges.scala", 247),
        ("compiler/src/dotty/tools/io/FileWriters.scala", 1156),
    ];

    println!("immutable_class_field_baseline_method_outcomes:");
    for (path, method_tree) in BASELINE_ATTEMPTS {
        let key = format!("{path}#tree={method_tree}");
        let outcome = outcomes
            .get(&key)
            .unwrap_or_else(|| panic!("baseline local method attempt is missing: {key}"));
        assert_eq!(
            outcome, "ImportQualifierNotFound",
            "the pinned baseline method should now reach its classpath blocker: {key}"
        );
        println!("  {key} outcome={outcome}");
    }
}

fn print_pinned_immutable_field_completion_outcomes(root: &Path, classpath: SharedClassPath) {
    const BASELINE_FIELDS: [(&str, u32, usize); 7] = [
        (
            "compiler/src/dotty/tools/dotc/core/TypeErrors.scala",
            584,
            1,
        ),
        (
            "compiler/src/dotty/tools/dotc/inlines/Inliner.scala",
            966,
            1,
        ),
        (
            "compiler/src/dotty/tools/dotc/printing/ReplPrinter.scala",
            74,
            1,
        ),
        (
            "compiler/src/dotty/tools/dotc/reporting/Profile.scala",
            231,
            5,
        ),
        (
            "compiler/src/dotty/tools/dotc/rewrites/Rewrites.scala",
            82,
            2,
        ),
        (
            "compiler/src/dotty/tools/dotc/transform/Bridges.scala",
            152,
            3,
        ),
        ("compiler/src/dotty/tools/io/FileWriters.scala", 1129, 1),
    ];

    println!("immutable_class_field_completion_outcomes:");
    for (relative_path, expected_tree_index, baseline_occurrences) in BASELINE_FIELDS {
        let text = fs::read_to_string(root.join(relative_path))
            .unwrap_or_else(|error| panic!("cannot read {relative_path}: {error}"));
        let source = SourceId::from_index(0);
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let scanner = ContextualScanner::new(&text)
            .unwrap_or_else(|error| panic!("cannot scan {relative_path}: {error}"));
        let parsed = parse_compilation_unit(
            SourceText::new(&text).expect("Scala source should be valid UTF-8"),
            source,
            scanner,
            &mut store.names,
        );
        let mut namer_packages = Packages::new();
        let index = name_compilation_unit(
            &parsed.ast,
            parsed.root,
            source,
            relative_path,
            &mut store,
            &mut namer_packages,
        )
        .unwrap_or_else(|error| panic!("cannot name {relative_path}: {error}"));
        let (field_tree, field_name, field_type_tree) = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| {
                if tree.index() != expected_tree_index {
                    return None;
                }
                let TreeKind::ValDef(definition) = &node.kind else {
                    return None;
                };
                Some((
                    tree,
                    store
                        .names
                        .resolve(definition.name.as_name().text())
                        .to_owned(),
                    definition.tpt,
                ))
            })
            .unwrap_or_else(|| {
                panic!(
                    "{relative_path} no longer has the baseline field at tree {expected_tree_index}"
                )
            });
        let field = index
            .symbol_at(source, field_tree)
            .expect("baseline field should retain its semantic symbol");
        let owner = store
            .symbols
            .get(field)
            .owner
            .expect("baseline field should retain its class owner");
        assert_eq!(
            store.symbols.get(field).kind,
            dotty_core::symbols::SymbolKind::Field
        );
        assert_eq!(
            store.symbols.get(owner).kind,
            dotty_core::symbols::SymbolKind::Class
        );
        assert!(
            !store
                .symbols
                .get(field)
                .flags
                .contains(dotty_core::SymbolFlags::MUTABLE)
        );

        let typer_packages = Packages::new();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &typer_packages,
        );
        typer = typer.with_resolver(Box::new(AuditResolver {
            inner: ClasspathSymbolResolver::new(
                classpath.clone(),
                definitions,
                LoadingSession::with_packages(namer_packages),
            ),
            metrics: Rc::new(RefCell::new(ResolverMetrics::default())),
        }));
        let outcome = match typer.complete_symbol(field) {
            Ok(_) => {
                assert!(matches!(
                    *typer.store().symbols.info(field),
                    dotty_core::symbols::SymbolInfo::Complete(_)
                ));
                assert!(
                    typer
                        .source_type_index()
                        .type_at(source, field_type_tree)
                        .is_some()
                );
                "completed".to_owned()
            }
            Err(error) => format!("blocked::{}::{error:?}", typer_error_name(&error)),
        };
        println!(
            "  {relative_path}: tree={expected_tree_index} field={field_name} baseline_occurrences={baseline_occurrences} outcome={outcome}"
        );
    }
}

fn print_pinned_mutable_field_completion_outcomes(root: &Path, classpath: SharedClassPath) {
    const BASELINE_FIELDS: [(&str, u32, usize); 7] = [
        (
            "compiler/src/dotty/tools/dotc/cc/CheckCaptures.scala",
            895,
            2,
        ),
        (
            "compiler/src/dotty/tools/dotc/parsing/Scanners.scala",
            524,
            1,
        ),
        (
            "compiler/src/dotty/tools/dotc/parsing/Scanners.scala",
            937,
            1,
        ),
        (
            "compiler/src/dotty/tools/dotc/reporting/Message.scala",
            222,
            2,
        ),
        (
            "compiler/src/dotty/tools/dotc/typer/Applications.scala",
            4151,
            1,
        ),
        (
            "compiler/src/dotty/tools/dotc/util/WeakHashSet.scala",
            82,
            1,
        ),
        ("library/src/scala/collection/Iterator.scala", 3718, 2),
    ];

    assert_eq!(
        BASELINE_FIELDS
            .iter()
            .map(|(_, _, occurrences)| occurrences)
            .sum::<usize>(),
        10,
        "the seven distinct mutable fields should account for all 10 baseline attempts"
    );

    println!("mutable_class_field_completion_outcomes:");
    for (relative_path, expected_tree_index, baseline_occurrences) in BASELINE_FIELDS {
        let text = fs::read_to_string(root.join(relative_path))
            .unwrap_or_else(|error| panic!("cannot read {relative_path}: {error}"));
        let source = SourceId::from_index(0);
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let scanner = ContextualScanner::new(&text)
            .unwrap_or_else(|error| panic!("cannot scan {relative_path}: {error}"));
        let parsed = parse_compilation_unit(
            SourceText::new(&text).expect("Scala source should be valid UTF-8"),
            source,
            scanner,
            &mut store.names,
        );
        let mut namer_packages = Packages::new();
        let index = name_compilation_unit(
            &parsed.ast,
            parsed.root,
            source,
            relative_path,
            &mut store,
            &mut namer_packages,
        )
        .unwrap_or_else(|error| panic!("cannot name {relative_path}: {error}"));
        let (field_tree, field_name, field_type_tree, source_mutable) = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| {
                if tree.index() != expected_tree_index {
                    return None;
                }
                let TreeKind::ValDef(definition) = &node.kind else {
                    return None;
                };
                Some((
                    tree,
                    store
                        .names
                        .resolve(definition.name.as_name().text())
                        .to_owned(),
                    definition.tpt,
                    definition
                        .metadata
                        .modifiers
                        .contains(&dotty_core::ast::Modifier::Var),
                ))
            })
            .unwrap_or_else(|| {
                panic!(
                    "{relative_path} no longer has the baseline field at tree {expected_tree_index}"
                )
            });
        assert!(
            source_mutable,
            "{relative_path} tree {expected_tree_index} should be a var"
        );
        let field = index
            .symbol_at(source, field_tree)
            .expect("baseline field should retain its semantic symbol");
        let owner = store
            .symbols
            .get(field)
            .owner
            .expect("baseline field should retain its class owner");
        assert_eq!(
            store.symbols.get(field).kind,
            dotty_core::symbols::SymbolKind::Field
        );
        assert_eq!(
            store.symbols.get(owner).kind,
            dotty_core::symbols::SymbolKind::Class
        );
        assert!(
            store
                .symbols
                .get(field)
                .flags
                .contains(dotty_core::SymbolFlags::MUTABLE)
        );

        let typer_packages = Packages::new();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &typer_packages,
        );
        typer = typer.with_resolver(Box::new(AuditResolver {
            inner: ClasspathSymbolResolver::new(
                classpath.clone(),
                definitions,
                LoadingSession::with_packages(namer_packages),
            ),
            metrics: Rc::new(RefCell::new(ResolverMetrics::default())),
        }));
        let outcome = match typer.complete_symbol(field) {
            Ok(inferred) => {
                assert!(matches!(
                    *typer.store().symbols.info(field),
                    dotty_core::symbols::SymbolInfo::Complete(existing) if existing == inferred
                ));
                assert_eq!(
                    typer.source_type_index().type_at(source, field_type_tree),
                    Some(inferred)
                );
                "completed".to_owned()
            }
            Err(TyperError::MissingDeclaredType { .. }) => {
                panic!(
                    "eligible mutable class field remained deferred: {relative_path} tree {expected_tree_index}"
                )
            }
            Err(error) => format!("blocked::{}::{error:?}", typer_error_name(&error)),
        };
        println!(
            "  {relative_path}: tree={expected_tree_index} field={field_name} baseline_occurrences={baseline_occurrences} outcome={outcome}"
        );
    }
}

fn print_v1_deltas(audit: &Audit) {
    const V1_FILES: usize = 1_236;
    const V1_LOCAL_DEFINITIONS: usize = 23_218;
    const V1_LOCAL_METHODS: usize = 3_778;
    const V1_TYPED_LOCAL_METHODS: usize = 0;
    const V1_IMPORT_QUALIFIER_NOT_FOUND: usize = 2_046;
    const V1_UNSUPPORTED_EXPRESSION: usize = 572;
    const V1_LOCAL_DECLARATION_DEFERRED: usize = 272;
    const V1_NO_SUCCESSFUL_ENCLOSING_METHOD: usize = 483;
    const V1_PARENS: usize = 146;
    const V1_INFIX: usize = 146;
    const V1_LOCAL_IMPORT: usize = 139;

    let failures = &audit.failures;
    let current_import = failures
        .get("ImportQualifierNotFound")
        .map_or(0, |bucket| bucket.count);
    let current_unsupported = sum_buckets_with_prefix(failures, "UnsupportedExpression::");
    let current_deferred = sum_buckets_with_prefix(failures, "LocalBlockDeclarationDeferred::");
    let current_no_success = failures
        .get("NoSuccessfulEnclosingMethodTyping")
        .map_or(0, |bucket| bucket.count);
    println!("audit_v1_comparison:");
    println!(
        "  files_attempted={} (delta={})",
        audit.files_attempted,
        signed_delta(audit.files_attempted, V1_FILES)
    );
    println!(
        "  local_declarations={} (delta={})",
        audit.local_definitions,
        signed_delta(audit.local_definitions, V1_LOCAL_DEFINITIONS)
    );
    println!(
        "  local_methods={} (delta={})",
        audit.local_defdefs,
        signed_delta(audit.local_defdefs, V1_LOCAL_METHODS)
    );
    println!(
        "  typed_local_methods={} (delta={})",
        audit.typed_local_defdefs,
        signed_delta(audit.typed_local_defdefs, V1_TYPED_LOCAL_METHODS)
    );
    println!(
        "  ImportQualifierNotFound={} (delta={})",
        current_import,
        signed_delta(current_import, V1_IMPORT_QUALIFIER_NOT_FOUND)
    );
    println!(
        "  UnsupportedExpression_total={} (delta={})",
        current_unsupported,
        signed_delta(current_unsupported, V1_UNSUPPORTED_EXPRESSION)
    );
    println!(
        "  LocalBlockDeclarationDeferred={} (delta={})",
        current_deferred,
        signed_delta(current_deferred, V1_LOCAL_DECLARATION_DEFERRED)
    );
    println!(
        "  NoSuccessfulEnclosingMethodTyping={} (delta={})",
        current_no_success,
        signed_delta(current_no_success, V1_NO_SUCCESSFUL_ENCLOSING_METHOD)
    );
    println!("audit_581_feature_comparison:");
    for (bucket, baseline) in [
        ("UnsupportedExpression::Parens", V1_PARENS),
        ("UnsupportedExpression::InfixOp", V1_INFIX),
        ("LocalBlockDeclarationDeferred::import", V1_LOCAL_IMPORT),
    ] {
        let current = failures.get(bucket).map_or(0, |entry| entry.count);
        println!(
            "  {bucket}={current} (baseline={baseline}, delta={})",
            signed_delta(current, baseline)
        );
    }
}

fn signed_delta(current: usize, baseline: usize) -> String {
    let delta = current as i128 - baseline as i128;
    format!("{delta:+}")
}

fn sum_buckets_with_prefix(failures: &BTreeMap<String, FailureBucket>, prefix: &str) -> usize {
    failures
        .iter()
        .filter(|(name, _)| name.starts_with(prefix))
        .map(|(_, bucket)| bucket.count)
        .sum()
}

fn ordered_type_tree_failures(audit: &Audit) -> Vec<(&String, &FailureBucket)> {
    let mut failures = audit
        .failures
        .iter()
        .filter(|(name, _)| name.starts_with("UnsupportedTypeTree::"))
        .collect::<Vec<_>>();
    failures.sort_by(|(name_a, a), (name_b, b)| b.count.cmp(&a.count).then(name_a.cmp(name_b)));
    failures
}

fn print_ranked_gaps(failures: &BTreeMap<String, FailureBucket>) {
    let mut ranked = failures
        .iter()
        .filter(|(name, bucket)| {
            !matches!(
                bucket.family,
                FailureFamily::ParserNamer | FailureFamily::ResolutionClasspathEnvironment
            ) && name.as_str() != "NoSuccessfulEnclosingMethodTyping"
        })
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
    println!("top_gap_implementation_scope_notes:");
    for (name, bucket) in ranked.iter().take(10) {
        let (slice, owner, prerequisites, non_goals) = scope_note_for_bucket(name);
        println!(
            "  {name} ({} occurrences, {} files): first_slice={slice}; owner={owner}; prerequisites={prerequisites}; non_goals={non_goals}",
            bucket.count,
            bucket.files.len()
        );
    }
    if let Some((name, bucket)) = ranked.first() {
        println!(
            "{}; keep classpath materialization as a separate gate because the pinned audit resolved no external members",
            highest_ranked_semantic_gap(name, bucket.count, bucket.files.len())
        );
    }
}

fn highest_ranked_semantic_gap(name: &str, count: usize, files: usize) -> String {
    format!(
        "highest_ranked_semantic_gap: {name} ({count} occurrences in {files} files); count ranks the audit only and does not select a sprint increment"
    )
}

#[test]
fn highest_ranked_gap_output_does_not_claim_to_recommend_a_sprint() {
    let summary = highest_ranked_semantic_gap("MissingDeclaredType", 34, 15);

    assert_eq!(
        summary,
        "highest_ranked_semantic_gap: MissingDeclaredType (34 occurrences in 15 files); count ranks the audit only and does not select a sprint increment"
    );
    assert!(!summary.contains("recommendation"));
}

fn scope_note_for_bucket(bucket: &str) -> (&'static str, &'static str, &'static str, &'static str) {
    match bucket {
        "MissingDeclaredType" => (
            "infer one ordinary or inline inferred module-class field after source class val/var inference; the remaining bucket is 10 occurrences across 3 files",
            "dotty-typer/src/typer/completion/mod.rs, completion/declarations.rs, type_projection.rs, and existing expression typing",
            "module initialization context, RHS typing and widening, cycle behavior, and completion rollback",
            "class fields, method results, local PatDef, and generalized expected-type inference",
        ),
        "AnonymousClassInstantiationDeferred" => (
            "support one anonymous new with one concrete parent and explicit member ownership",
            "dotty-typer/src/typer/expression/new.rs",
            "ordinary New typing, parent projection, and stable anonymous class identity",
            "closure capture, refinement synthesis, and general anonymous-class members",
        ),
        "UnsupportedSingletonReference" => (
            "literal singleton projection landed in #881; next compare constant payloads and relate them to underlying types",
            "dotty-typer/src/typer/type_projection.rs and the bounded type relation",
            "Type::Constant projection, exact constant equality, and expected-type adaptation",
            "arbitrary paths, unstable prefixes, literal unions, and path-dependent relation redesign",
        ),
        "LocalBlockDeclarationDeferred::val/var definition" => (
            "split remaining local definitions by PatDef root and binder shape before adding one form",
            "dotty-typer/src/typer/expression/blocks.rs",
            "transactional PatDef lowering, local binders, and assignment support",
            "general destructuring or reopening already supported PatDef forms",
        ),
        "LocalMethodSignatureDeferred" => (
            "split the feature payload and add a fixture for the most frequent unsupported signature",
            "dotty-typer/src/typer/completion/local_methods.rs",
            "the shared signature builder and existing parameter/type-parameter scopes",
            "general dependent-result, erased/by-name, or method-inference redesign",
        ),
        "UnsupportedExpression::Function" => (
            "lower one explicitly typed single-parameter function expression",
            "dotty-typer/src/typer/expression",
            "Method types, local parameter scopes, and existing closure AST nodes",
            "lambda inference, polymorphism, capture checking, and contextual functions",
        ),
        "UnsupportedTypeTree::FunctionWithMods" => (
            "inspect the remaining modifier-bearing function types and keep erased/capture-specific forms deferred",
            "dotty-typer/src/typer/type_projection.rs",
            "plain contextual `Given` forms now use the canonical ContextFunction identity and existing Applied types",
            "capture checking, erased-function semantics, and arbitrary modifiers",
        ),
        "UnsupportedTypeTree::Function" => (
            "project one ordinary explicit Function type tree",
            "dotty-typer/src/typer/type_projection.rs",
            "canonical FunctionN identity and existing Applied types",
            "lambda expressions, inference, and relation redesign",
        ),
        "UnsupportedExpression::Match" => (
            "type the scrutinee, each case pattern, and each case body against one expected result type",
            "dotty-typer/src/typer/expression/mod.rs, with a focused pattern helper",
            "the current Match/CaseDef AST and typed pattern representation",
            "exhaustivity checking, GADT refinement, and inferred match result unions",
        ),
        "UnsupportedExpression::InfixOp" => (
            "lower one infix node to the existing selected-member application path",
            "dotty-typer/src/typer/expression/mod.rs",
            "operator name, receiver, and right operand already present in the AST",
            "precedence parsing or general extension-method search",
        ),
        "UnsupportedExpression::Parens" => (
            "type the enclosed expression and preserve the wrapper source span",
            "dotty-typer/src/typer/expression",
            "the inner expression's ordinary typing context",
            "new syntax and semantic changes to the enclosed expression",
        ),
        "LocalBlockDeclarationDeferred::import" => (
            "resolve one local import qualifier and install its selector in the active block scope",
            "dotty-typer/src/typer/expression/blocks.rs",
            "the classpath resolver and transactional scope updates",
            "local type/class/object declarations and wildcard semantics beyond existing imports",
        ),
        "UnsupportedTypeTree" => (
            "lower the encountered type-tree shape into the existing core type model",
            "dotty-typer/src/typer/type_projection.rs",
            "the source type AST node and its named symbol/type metadata",
            "new parser syntax or broad type-model changes",
        ),
        _ => scope_note(implementation_category(bucket)),
    }
}

fn implementation_category(bucket: &str) -> &'static str {
    if bucket.starts_with("UnsupportedExpression::") {
        "expression typing"
    } else if bucket == "UnsupportedTypeTree" {
        "simple source lowering"
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
        "simple source lowering" => (
            "lower the exact reported type-tree form into the existing core type model",
            "dotty-typer/src/typer/type_tree.rs",
            "the source type AST node and symbol/type metadata",
            "new parser syntax or broad type-model changes",
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
fn resolver_request_metrics_exclude_source_reuse_from_external_attempts() {
    let metrics = ResolverMetrics {
        resolver_package_requests: 12,
        package_successes: 3,
        package_unresolved: 4,
        package_errors: 0,
        package_source_reuse: 5,
        resolver_member_requests: 8,
        member_successes: 2,
        member_source_reuse: 1,
        member_unresolved: 4,
        member_errors: 1,
        ..ResolverMetrics::default()
    };

    assert_eq!(metrics.external_package_requests(), 7);
    assert_eq!(
        metrics.external_package_requests(),
        metrics.package_successes + metrics.package_unresolved + metrics.package_errors
    );
    assert_eq!(
        metrics.resolver_package_requests,
        metrics.external_package_requests() + metrics.package_source_reuse
    );
    assert_eq!(metrics.external_member_requests(), 7);
    assert_eq!(
        metrics.external_member_requests(),
        metrics.member_successes + metrics.member_unresolved + metrics.member_errors
    );
    assert_eq!(
        metrics.resolver_member_requests,
        metrics.external_member_requests() + metrics.member_source_reuse
    );
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
fn local_value_blocker_profiler_classifies_declaration_shapes_and_deduplicates() {
    let source = "object Audit { def sample: Int = { val plain = 1; var mutable = 2; given context: Int = 3; implicit val legacy: Int = 4; lazy val delayed = 5; inline val constant = 6; val (left, right): (Int, Int) = (1, 2); val explicit: Int = 7; 0 }; def sibling: Int = 0 }";
    let source_id = SourceId::from_index(0);
    let mut store = SemanticStore::new();
    let scanner = ContextualScanner::new(source).expect("test source should scan");
    let parsed = parse_compilation_unit(
        SourceText::new(source).expect("test source should be valid UTF-8"),
        source_id,
        scanner,
        &mut store.names,
    );
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let method = |name: &str| {
        parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::DefDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == name =>
                {
                    Some(tree)
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("missing fixture method {name}"))
    };
    let sample = method("sample");
    let sibling = method("sibling");
    let observation = |name: &str| {
        let declaration = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::ValDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == name =>
                {
                    Some(tree)
                }
                TreeKind::PhaseSpecific(UntypedNode::PatDef(definition)) => {
                    let mut binders = BTreeSet::new();
                    for pattern in &definition.patterns {
                        collect_patdef_binders(&parsed.ast, &store.names, *pattern, &mut binders);
                    }
                    binders.contains(name).then_some(tree)
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("missing fixture declaration {name}"));
        local_value_blocker_observation(
            &parsed.ast,
            &store.names,
            source,
            "Audit.scala",
            sample,
            declaration.index(),
            sample.index(),
        )
        .unwrap()
    };

    let plain = observation("plain");
    assert_eq!(plain.node_kind, "ValDef");
    assert_eq!(plain.declaration_kind, "val");
    assert_eq!(plain.type_form, "inferred");
    assert!(plain.rhs_present);
    assert_eq!(plain.visibility, "default");
    assert!(!plain.annotations);

    let mutable = observation("mutable");
    assert_eq!(mutable.declaration_kind, "var");
    assert_eq!(mutable.modifiers, "Var");
    let context = observation("context");
    assert_eq!(context.declaration_kind, "given");
    assert_eq!(
        context.dispatch_path,
        "type_block_stat_expansion -> type_value_expression_inner -> type_local_value: LocalValueModifierDeferred (audit bucket mapped to LocalBlockDeclarationDeferred::val/var definition)"
    );
    assert_eq!(observation("legacy").declaration_kind, "implicit val");
    assert_eq!(observation("delayed").declaration_kind, "lazy val");
    assert_eq!(observation("constant").declaration_kind, "inline val");

    let patdef = observation("left");
    assert_eq!(patdef.node_kind, "PatDef");
    assert_eq!(patdef.declaration_name, "left,right");
    assert_eq!(patdef.pattern_roots, "Tuple");
    assert_eq!(patdef.source_pattern_count, Some(1));
    assert_eq!(patdef.binder_count, Some(2));
    assert_eq!(patdef.type_form, "explicit");
    assert_eq!(observation("explicit").type_form, "explicit");

    let mut inherited = plain.clone();
    inherited.enclosing_method_tree = sibling.index();
    inherited.enclosing_method = "Audit.scala#tree=sibling:sibling".to_owned();
    inherited.attribution = "inherited".to_owned();
    let mut first = LocalValueBlockerProfile::default();
    first.record(inherited.clone());
    first.record(plain.clone());
    let mut second = LocalValueBlockerProfile::default();
    second.record(plain.clone());
    let mut inherited_again = plain;
    inherited_again.enclosing_method_tree = sibling.index();
    inherited_again.enclosing_method = "Audit.scala#tree=sibling:sibling".to_owned();
    inherited_again.attribution = "inherited".to_owned();
    second.record(inherited_again);
    assert_eq!(first, second, "profile rows should have stable ordering");
    assert_eq!(first.observations.len(), 2);
    assert_eq!(first.distinct_declaration_count(), 1);
}

#[test]
fn local_patdef_audit_profiles_roots_binders_modifiers_and_type_trees() {
    let source = "object Audit { def pair = (1, \"x\"); def outer(value: Option[(Int, String)]): Int = { val Some((a, b)) = value; val (c, d) = pair; val (_, _) = pair; var (e, f) = pair; lazy val (g, h) = pair; val only @ Some(_) = value; val (i, j): (Int, String) = pair; val first, second = pair; 0 } }";
    let audit = audit_source(source, "PatDef.scala");
    let profile = audit.patdef_profile;

    assert_eq!(profile.total, 8, "{profile:?}");
    assert_eq!(
        profile.root_shapes.get("Apply / extractor-looking"),
        Some(&1)
    );
    assert_eq!(profile.root_shapes.get("Bind"), Some(&1));
    assert_eq!(profile.root_shapes.get("Ident"), Some(&2));
    assert_eq!(profile.root_shapes.get("Tuple"), Some(&5));
    assert_eq!(profile.source_pattern_counts.get("1"), Some(&7));
    assert_eq!(profile.source_pattern_counts.get("2"), Some(&1));
    assert_eq!(profile.binder_counts.get("0"), Some(&1));
    assert_eq!(profile.binder_counts.get("1"), Some(&1));
    assert_eq!(profile.binder_counts.get("2"), Some(&6));
    assert_eq!(profile.modifiers.get("val"), Some(&6));
    assert_eq!(profile.modifiers.get("var"), Some(&1));
    assert_eq!(profile.modifiers.get("lazy val"), Some(&1));
    assert_eq!(
        profile.explicit_tpt.get("explicit PatDef-wide tpt"),
        Some(&1)
    );
    assert_eq!(
        profile.explicit_tpt.get("synthetic inferred TypeTree"),
        Some(&7)
    );
    assert_eq!(profile.rhs_states.get("present"), Some(&8));
    assert_eq!(
        profile.representative_files,
        BTreeSet::from(["PatDef.scala".to_owned()])
    );
}

#[test]
fn local_patdef_audit_collects_nested_pattern_binders_structurally() {
    let source = "object Audit { def outer(value: Option[(Int, Int)]): Int = { val Some((a, b)) = value; val left @ Some(_) = value; val (_, _) = (1, 2); 0 } }";
    let audit = audit_source(source, "NestedPatDef.scala");
    let profile = audit.patdef_profile;

    assert_eq!(profile.total, 3, "{profile:?}");
    assert_eq!(
        profile.root_shapes.get("Apply / extractor-looking"),
        Some(&1)
    );
    assert_eq!(profile.root_shapes.get("Bind"), Some(&1));
    assert_eq!(profile.root_shapes.get("Tuple"), Some(&1));
    assert_eq!(profile.binder_counts.get("0"), Some(&1));
    assert_eq!(profile.binder_counts.get("1"), Some(&1));
    assert_eq!(profile.binder_counts.get("2"), Some(&1));
}

#[test]
fn local_patdef_audit_does_not_collect_binders_from_alternatives() {
    let source = "object Audit { def outer(value: Option[Int]): Int = value match { case Some(x) | None => 0 } }";
    let mut store = SemanticStore::new();
    Definitions::bootstrap(&mut store);
    let scanner = ContextualScanner::new(source).expect("test source should scan");
    let parsed = parse_compilation_unit(
        SourceText::new(source).expect("test source should be valid UTF-8"),
        SourceId::from_index(0),
        scanner,
        &mut store.names,
    );
    let alternative = parsed
        .ast
        .iter()
        .find_map(|(tree, node)| matches!(node.kind, TreeKind::Alternative(_)).then_some(tree))
        .expect("fixture should contain a pattern alternative");
    let mut binders = BTreeSet::new();

    collect_patdef_binders(&parsed.ast, &store.names, alternative, &mut binders);

    assert!(binders.is_empty(), "{binders:?}");
}

#[test]
fn local_patdef_audit_is_deterministic() {
    let source = "object Audit { def outer(value: Option[Int]): Int = { val Some(x) = value; val (_, _) = (1, 2); 0 } }";
    let first = audit_source(source, "PatDef.scala");
    let second = audit_source(source, "PatDef.scala");

    assert_eq!(first.patdef_profile, second.patdef_profile);
}

#[test]
fn local_patdef_audit_records_typing_successes_with_file_counts() {
    let audit = audit_source(
        "class MaybeInt { def isEmpty: Boolean = false; def get: Int = 1 }; object Extractor { def unapply(value: Any): MaybeInt = new MaybeInt }; object Audit { def outer(value: Any): Int = { var Extractor(result) = value; result } }",
        "MutablePatDef.scala",
    );

    let (key, outcome) = audit
        .patdef_profile
        .typing_outcomes
        .iter()
        .find(|(key, _)| key.starts_with("success::var::binders=1::root=Apply / extractor-looking"))
        .unwrap_or_else(|| {
            panic!(
                "the mutable PatDef should be typed successfully: {:#?}",
                audit.patdef_profile
            )
        });
    assert!(key.contains("tpt=synthetic inferred TypeTree"), "{key}");
    assert_eq!(outcome.count, 1);
    assert_eq!(
        outcome.files,
        BTreeSet::from(["MutablePatDef.scala".to_owned()])
    );

    let deferred = audit_source(
        "object Audit { def outer(value: Option[Int]): Int = { lazy val Some(result) = value; result } }",
        "LazyPatDef.scala",
    );
    let (key, outcome) = deferred
        .patdef_profile
        .typing_outcomes
        .iter()
        .find(|(key, _)| {
            key.starts_with(
                "failure::LocalValueModifierDeferred::declaration semantics deferred::lazy val",
            )
        })
        .expect("lazy PatDef should be counted in its local modifier deferral bucket");
    assert!(key.contains("binders=1"), "{key}");
    assert_eq!(outcome.count, 1);
    assert_eq!(
        outcome.files,
        BTreeSet::from(["LazyPatDef.scala".to_owned()])
    );
}

#[test]
fn local_patdef_audit_preserves_attempts_across_method_failures() {
    let audit = audit_source(
        "class MaybeInt { def isEmpty: Boolean = false; def get: Int = 1 }; object Extractor { def unapply(value: Any): MaybeInt = new MaybeInt }; object Audit { def later(value: Any): Int = { var Extractor(reached) = value; missingAfter; reached }; def earlier(value: Any): Int = { missingBefore; var Extractor(unreached) = value; unreached } }",
        "PatDefAttempt.scala",
    );

    let successful = audit
        .patdef_profile
        .typing_outcomes
        .iter()
        .find(|(key, _)| key.starts_with("success::var::binders=1"))
        .unwrap_or_else(|| {
            panic!(
                "the PatDef before the later error should retain its own success: {:#?}",
                audit.patdef_profile.typing_outcomes
            )
        });
    assert_eq!(successful.1.count, 1);

    let not_attempted = audit
        .patdef_profile
        .typing_outcomes
        .iter()
        .find(|(key, _)| key.starts_with("not_attempted::var::binders=1"))
        .expect("the PatDef after an earlier error should not inherit that error");
    assert_eq!(not_attempted.1.count, 1);
    assert_eq!(
        successful.1.files,
        BTreeSet::from(["PatDefAttempt.scala".to_owned()])
    );
}

#[test]
fn local_patdef_audit_distinguishes_missing_and_recovered_rhs() {
    let source = "object Audit { def outer: Unit = { val (left, right) = 1; () } }";
    let source_id = SourceId::from_index(0);
    let mut store = SemanticStore::new();
    Definitions::bootstrap(&mut store);
    let scanner = ContextualScanner::new(source).expect("test source should scan");
    let mut parsed = parse_compilation_unit(
        SourceText::new(source).expect("test source should be valid UTF-8"),
        source_id,
        scanner,
        &mut store.names,
    );
    let patdef_tree = parsed
        .ast
        .iter()
        .find_map(|(tree, node)| {
            matches!(node.kind, TreeKind::PhaseSpecific(UntypedNode::PatDef(_))).then_some(tree)
        })
        .expect("fixture should contain a PatDef");
    let TreeKind::PhaseSpecific(UntypedNode::PatDef(definition)) =
        &mut parsed.ast.get_mut(patdef_tree).kind
    else {
        unreachable!("selected tree is a PatDef")
    };
    definition.rhs = None;
    let missing = collect_local_patdefs(&parsed.ast, &store.names, "MissingRhs.scala");
    let recovered = audit_source(
        "object Audit { def outer: Unit = { val (left, right) = ; () } }",
        "RecoveredRhs.scala",
    );

    assert_eq!(missing.total, 1, "{missing:?}");
    assert_eq!(missing.rhs_states.get("missing"), Some(&1));
    assert_eq!(recovered.patdef_profile.total, 1, "{recovered:?}");
    assert_eq!(
        recovered.patdef_profile.rhs_states.get("recovery error"),
        Some(&1)
    );
}

#[test]
fn local_patdef_attempts_exclude_fields_inside_method_spans() {
    let audit = audit_source(
        "object Audit { def outer: Unit = { class Local { val (field, other) = (1, 2) }; val (left, right) = (1, 2); () } }",
        "LocalField.scala",
    );
    let profiled = audit
        .patdef_profile
        .typing_outcomes
        .values()
        .map(|bucket| bucket.count)
        .sum::<usize>();

    assert_eq!(audit.patdef_profile.total, 1, "{audit:?}");
    assert_eq!(profiled, 1, "{audit:?}");
}

#[test]
fn local_expression_audit_types_infix_calls_and_counts_them_structurally() {
    let source = "class Box { def combine(other: Box): Box = this }; object Audit { def outer(left: Box, right: Box): Box = { def local: Box = left `combine` right; local } }";
    let audit = audit_source(source, "Infix.scala");

    assert_eq!(audit.expression_forms.get("InfixOp"), Some(&1));
    assert_eq!(audit.local_defdefs, 1);
    assert_eq!(audit.typed_local_defdefs, 1, "{audit:?}");
    assert!(audit.failures.is_empty(), "{audit:?}");
}

#[test]
fn local_expression_audit_types_supported_prefix_calls() {
    let source = "class Box { def unary_! : Int = 1 }; object Audit { def outer(value: Box): Int = { def local: Int = !value; local } }";
    let audit = audit_source(source, "Prefix.scala");

    assert_eq!(audit.expression_forms.get("PrefixOp"), Some(&1));
    assert_eq!(audit.prefix_operator_forms.get("!"), Some(&1));
    assert_eq!(audit.local_defdefs, 1);
    assert_eq!(audit.typed_local_defdefs, 1, "{audit:?}");
    assert!(audit.failures.is_empty(), "{audit:?}");
}

#[test]
fn local_expression_audit_reports_prefix_member_failure_by_actual_error() {
    let source = "class Box; object Audit { def outer(value: Box): Box = { def local: Box = !value; local } }";
    let audit = audit_source(source, "PrefixMissingMember.scala");

    assert_eq!(audit.expression_forms.get("PrefixOp"), Some(&1));
    assert_eq!(
        audit
            .failures
            .get("MemberNotFound")
            .map(|failure| failure.count),
        Some(1),
        "{audit:?}"
    );
    assert!(
        !audit
            .failures
            .contains_key("UnsupportedExpression::PrefixOp")
    );
}

#[test]
fn local_expression_audit_types_supported_term_annotations() {
    let source = "package scala.annotation { abstract class Annotation }; package scala { class unchecked extends scala.annotation.Annotation }; class Audit { import scala.unchecked; def outer: Int = { def local: Int = 1: @unchecked; local } }";
    let audit = audit_source(source, "Annotated.scala");

    assert_eq!(audit.local_defdefs, 1);
    assert_eq!(audit.typed_local_defdefs, 1, "{audit:?}");
    assert_eq!(audit.expression_forms.get("Annotated"), Some(&1));
    assert!(
        !audit
            .failures
            .contains_key("UnsupportedExpression::Annotated")
    );
    assert!(audit.failures.is_empty(), "{audit:?}");
}

#[test]
fn local_expression_audit_keeps_annotation_argument_errors_specific() {
    let source = "package scala.annotation { abstract class Annotation }; class TermAnnotation(val value: Int) extends scala.annotation.Annotation; object Audit { def outer(value: Int): Int = { def local: Int = 1: @TermAnnotation(value); local } }";
    let audit = audit_source(source, "AnnotatedArgument.scala");

    assert_eq!(
        audit
            .failures
            .get("SourceAnnotationArgumentNotConstant")
            .map(|failure| failure.count),
        Some(1),
        "{audit:?}"
    );
    assert!(
        !audit
            .failures
            .contains_key("UnsupportedExpression::Annotated")
    );
}

#[test]
fn match_readiness_counts_unsupported_case_shapes_for_match_first_blockers() {
    let source = "object Audit { def outer(value: Int): Int = { def local: Int = value match { case Extractor(_) if true => 1; case _ => 2 }; local } }";
    let audit = audit_source(source, "Match.scala");

    assert_eq!(
        audit
            .failures
            .get("ExtractorQualifierNotFound")
            .map(|failure| failure.count),
        Some(1),
        "{audit:?}"
    );
    assert_eq!(audit.match_readiness.first_blocker_methods, 1);
    assert_eq!(audit.match_readiness.matches, 1);
    assert_eq!(audit.match_readiness.cases, 2);
    assert_eq!(audit.match_readiness.guarded_cases, 1);
    assert_eq!(
        audit
            .match_readiness
            .pattern_shapes
            .get("extractor-looking Apply/TypeApply"),
        Some(&1)
    );
    assert_eq!(
        audit
            .match_readiness
            .pattern_shapes
            .get("wildcard/identifier/bind"),
        Some(&1)
    );
}

#[test]
fn match_profile_counts_tuple_infix_and_nested_nary_extractor_shapes() {
    let source = "object Audit { def outer(value: Any): Any = value match { case Extractor(first, left | right, Nested(a, b, c)) => first; case (first, second) => first; case () => value; case left op right => left; case _ => value } }";
    let audit = audit_source(source, "PatternProfile.scala");

    assert_eq!(audit.match_profile.matches, 1);
    assert_eq!(
        audit
            .match_profile
            .pattern_roots
            .get("extractor-looking Apply"),
        Some(&1)
    );
    assert_eq!(audit.match_profile.pattern_roots.get("tuple"), Some(&2));
    assert_eq!(audit.match_profile.empty_tuple_unit_patterns, 1);
    assert_eq!(
        audit.match_profile.pattern_roots.get("infix pattern"),
        Some(&1)
    );
    assert_eq!(
        audit.match_profile.extractor_argument_counts.get(&3),
        Some(&2)
    );
    assert_eq!(
        audit
            .match_profile
            .extractor_nested_roots
            .get("alternative"),
        Some(&1)
    );
    assert_eq!(audit.match_profile.infix_pattern_forms, 1);
}

#[test]
fn local_expression_audit_types_supported_typed_patterns() {
    let source = "object Audit { def outer(value: Any): Int = { def local: Int = value match { case item: Int if true => item }; local } }";
    let audit = audit_source(source, "TypedPattern.scala");

    assert_eq!(audit.local_defdefs, 1);
    assert_eq!(audit.typed_local_defdefs, 1, "{audit:?}");
    assert!(audit.failures.is_empty(), "{audit:?}");
}

#[test]
fn local_expression_audit_reports_lambda_projection_and_deferred_declaration_subkinds() {
    let lambda = audit_source(
        "object Audit { def outer: Int = { def local: Int = (x: Int) => x; 0 } }",
        "Lambda.scala",
    );
    assert!(
        lambda.failures.contains_key("SourceFunctionClassNotFound"),
        "{lambda:?}"
    );

    let import = audit_source(
        "object Audit { def outer: Int = { import scala.collection; def local: Int = 1; local } }",
        "Import.scala",
    );
    assert!(
        import.failures.contains_key("ImportQualifierNotFound"),
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
fn local_extension_audit_splits_group_shape_and_receiver_type_failures() {
    let group = audit_source(
        "class Audit { def outer: Int = { extension (using context: Int) (receiver: Int) { def choose: Int = receiver }; def marker: Int = 1; 0 } }",
        "UnsupportedExtensionGroup.scala",
    );
    assert!(
        group
            .failures
            .contains_key("LocalExtensionGroupShapeDeferred"),
        "{group:?}"
    );

    let receiver = audit_source(
        "class Audit { def outer: Int = { extension (receiver: MissingReceiverType) { def choose: Int = 1 }; def marker: Int = 1; 0 } }",
        "MissingExtensionReceiver.scala",
    );
    let failure = receiver
        .failures
        .get("LocalExtensionReceiverTypeNotFound")
        .expect("extension receiver name should have its own audit bucket");
    assert_eq!(failure.count, 1);
    assert_eq!(
        failure.files,
        BTreeSet::from(["MissingExtensionReceiver.scala".to_owned()])
    );

    let nested_receiver = audit_source(
        "class Box[A] {}; class Audit { def outer: Int = { extension (receiver: Box[MissingReceiverType]) { def choose: Int = 1 }; def marker: Int = 1; 0 } }",
        "NestedMissingExtensionReceiver.scala",
    );
    assert!(
        nested_receiver
            .failures
            .contains_key("LocalExtensionReceiverTypeNotFound"),
        "nested receiver type errors should stay in the focused bucket: {nested_receiver:?}"
    );
}

#[test]
fn local_extension_audit_separates_nonapplicable_and_ambiguous_calls() {
    let nonapplicable = audit_source(
        "class Audit { def outer: Int = { extension (receiver: Audit) { def choose(argument: Int): Int = argument }; this.choose(false); def marker: Int = 1; 0 } }",
        "NonapplicableExtension.scala",
    );
    let failure = nonapplicable
        .failures
        .get("LocalExtensionNotApplicable")
        .expect("inapplicable local extensions should be classified separately");
    assert_eq!(failure.count, 1);
    assert_eq!(
        failure.files,
        BTreeSet::from(["NonapplicableExtension.scala".to_owned()])
    );

    let shadowed = audit_source(
        "class Audit { def outer: Int = { extension (receiver: Audit) { def choose(argument: Int): Int = argument }; { val choose: Int = 0; this.choose(1) } } }",
        "ShadowedExtension.scala",
    );
    assert!(
        !shadowed
            .failures
            .contains_key("LocalExtensionNotApplicable"),
        "a nested local value hides the enclosing extension: {shadowed:?}"
    );

    let unsupported_group = audit_source(
        "class Audit { def outer: Int = { this.choose(1); extension (receiver: Audit) { def choose(argument: Int): Int = argument; def choose(other: Boolean): Int = 0 }; def marker: Int = 1; 0 } }",
        "UnsupportedExtensionGroup.scala",
    );
    assert!(
        !unsupported_group
            .failures
            .contains_key("LocalExtensionNotApplicable"),
        "a group that is not preindexed is not an extension candidate: {unsupported_group:?}"
    );

    let right_associative = audit_source(
        "class Audit { def outer: Int = { this.++:(1); extension (receiver: Audit) { def ++:(argument: Int): Int = argument }; def marker: Int = 1; 0 } }",
        "RightAssociativeExtension.scala",
    );
    assert!(
        !right_associative
            .failures
            .contains_key("LocalExtensionNotApplicable"),
        "right-associative extensions are not preindexed: {right_associative:?}"
    );

    let tuple_shadow = audit_source(
        "class Audit { def outer: Int = { extension (receiver: Audit) { def choose(argument: Int): Int = argument }; { val (choose, other) = (0, 1); this.choose(1) } } }",
        "TuplePatternShadow.scala",
    );
    assert!(
        !tuple_shadow
            .failures
            .contains_key("LocalExtensionNotApplicable"),
        "tuple pattern binders shadow enclosing extensions: {tuple_shadow:?}"
    );

    let ambiguous = audit_source(
        "class Audit { def outer: Int = { extension (left: Audit) { def choose(argument: Int): Int = 1 }; extension (right: Audit) { def choose(argument: Int): Int = 2 }; this.choose(1); def marker: Int = 1; 0 } }",
        "AmbiguousExtension.scala",
    );
    assert!(
        ambiguous
            .failures
            .contains_key("LocalExtensionAmbiguityDeferred"),
        "{ambiguous:?}"
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
fn unsupported_type_tree_failures_keep_exact_source_shapes() {
    let fixtures = [
        ("class C { def f(x: List[?]): Unit = () }", "TypeBoundsTree"),
        ("class C { def f(x: A | B): Unit = () }", "InfixOp::|"),
        ("class C { def f(x: A & B): Unit = () }", "InfixOp::&"),
        ("class C { def f(x: A + B): Unit = () }", "InfixOp::<other>"),
        (
            "class C { def f(x: Any, y: x.type): Unit = () }",
            "SingletonTypeTree",
        ),
        ("class C { def f(xs: T*): Unit = () }", "PostfixOp::*"),
        (
            "class C { def f(x: A { type X = B }): Unit = () }",
            "RefinedTypeTree",
        ),
        ("object C { type F = [X] =>> X }", "LambdaTypeTree"),
    ];

    for (source_text, expected_shape) in fixtures {
        let mut store = SemanticStore::new();
        let source = SourceId::from_index(0);
        let scanner = ContextualScanner::new(source_text).unwrap();
        let parsed = parse_compilation_unit(
            SourceText::new(source_text).unwrap(),
            source,
            scanner,
            &mut store.names,
        );
        assert!(
            parsed.diagnostics.is_empty(),
            "{source_text}: {:?}",
            parsed.diagnostics
        );
        let operators = source_operator_spellings(&parsed.ast, &store.names);
        let (tree, node) = parsed
            .ast
            .iter()
            .find(|(tree, node)| {
                type_tree_shape_label(&node.kind, tree.index(), &operators) == expected_shape
            })
            .unwrap_or_else(|| panic!("{source_text} did not produce {expected_shape}"));
        let error = TyperError::UnsupportedTypeTree {
            source,
            tree_index: tree.index(),
            tree_kind: tree_kind_label(&node.kind),
        };
        let failure = classify_typer_error(&error, &parsed.ast, &operators, &store.names);

        assert_eq!(
            failure.bucket,
            format!("UnsupportedTypeTree::{expected_shape}")
        );
        assert_eq!(failure.family, FailureFamily::Other);
        assert_eq!(
            typed_case_failure_label(&error, &parsed.ast, &operators, &store.names),
            format!("UnsupportedTypeTree::{expected_shape}")
        );
    }
}

#[test]
fn missing_declared_type_profile_separates_inferred_fields_methods_and_explicit_types() {
    let source_text = "object Audit { val inferred = 1; var mutable = 2; val explicit: Int = 3; def inferredMethod = 4; def declaredMethod: Int = 5 }";
    let source = SourceId::from_index(0);
    let mut store = SemanticStore::new();
    let scanner = ContextualScanner::new(source_text).unwrap();
    let parsed = parse_compilation_unit(
        SourceText::new(source_text).unwrap(),
        source,
        scanner,
        &mut store.names,
    );
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let mut packages = Packages::new();
    let index = name_compilation_unit(
        &parsed.ast,
        parsed.root,
        source,
        "MissingDeclaredType.scala",
        &mut store,
        &mut packages,
    )
    .unwrap();
    let val_tpt = |name: &str| {
        parsed
            .ast
            .iter()
            .find_map(|(_, node)| match &node.kind {
                TreeKind::ValDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == name =>
                {
                    Some(definition.tpt)
                }
                _ => None,
            })
            .unwrap()
    };
    let def_tpt = |name: &str| {
        parsed
            .ast
            .iter()
            .find_map(|(_, node)| match &node.kind {
                TreeKind::DefDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == name =>
                {
                    Some(definition.tpt)
                }
                _ => None,
            })
            .unwrap()
    };

    let inferred = missing_declared_type_detail(
        &parsed.ast,
        &index,
        &store,
        source,
        val_tpt("inferred").index(),
    );
    assert!(inferred.bucket.contains("value::Field::"), "{inferred:?}");
    assert!(
        inferred.bucket.contains("synthetic inferred TypeTree"),
        "{inferred:?}"
    );
    assert!(inferred.record.contains("rhs=true"));
    assert!(inferred.record.contains("type_of_tpt_inner_journaled"));

    let mutable = missing_declared_type_detail(
        &parsed.ast,
        &index,
        &store,
        source,
        val_tpt("mutable").index(),
    );
    assert!(mutable.bucket.contains("variable::Field::"), "{mutable:?}");
    assert!(mutable.record.contains("modifiers=[Var]"));
    assert!(mutable.record.contains("semantic_mutable=true"));

    let method_result = missing_declared_type_detail(
        &parsed.ast,
        &index,
        &store,
        source,
        def_tpt("inferredMethod").index(),
    );
    assert!(method_result.bucket.contains("method result::Method::"));
    assert!(method_result.record.contains("complete_method_signature"));

    assert!(!matches!(
        &parsed.ast.get(val_tpt("explicit")).kind,
        TreeKind::TypeTree(_)
    ));
    assert!(!matches!(
        &parsed.ast.get(def_tpt("declaredMethod")).kind,
        TreeKind::TypeTree(_)
    ));

    let unknown = missing_declared_type_detail(&parsed.ast, &index, &store, source, u32::MAX);
    assert!(unknown.bucket.starts_with("unknown declaration::unknown::"));
    assert!(unknown.record.contains("declaration_tree=unknown"));
}

#[test]
fn missing_declared_type_profile_orders_records_and_paths_deterministically() {
    let mut profile = MissingDeclaredTypeProfile::default();
    profile.record(
        "field::inferred".to_owned(),
        "z.scala",
        "z record".to_owned(),
        "z.scala#1".to_owned(),
    );
    profile.record(
        "field::inferred".to_owned(),
        "a.scala",
        "a record".to_owned(),
        "a.scala#1".to_owned(),
    );

    assert_eq!(
        profile.records.iter().cloned().collect::<Vec<_>>(),
        ["a record", "z record"]
    );
    assert_eq!(
        profile.buckets["field::inferred"]
            .files
            .iter()
            .cloned()
            .collect::<Vec<_>>(),
        ["a.scala", "z.scala"]
    );
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
fn declared_type_tree_histogram_is_structural_and_deterministic() {
    let source = "object Audit { def outer[A: Show](value: List[?], union: A | B, intersection: A & B, singleton: value.type, function: (A) => B, parens: (A), selected: pkg.Type, repeated: A*): A = { val refined: A { type Member = B } = value; type Higher = [X] =>> X; value.member + refined.member } }";
    let mut store = SemanticStore::new();
    let source_id = SourceId::from_index(0);
    let scanner = ContextualScanner::new(source).unwrap();
    let parsed = parse_compilation_unit(
        SourceText::new(source).unwrap(),
        source_id,
        scanner,
        &mut store.names,
    );
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);

    let first = collect_declared_type_tree_histogram(&parsed.ast, &store.names);
    let second = collect_declared_type_tree_histogram(&parsed.ast, &store.names);
    let (inventory, forms, _) = collect_declared_type_tree_inventory(&parsed.ast, &store.names);
    assert_eq!(first, second);
    assert_eq!(first, inventory);
    assert!(forms.contains("InfixOp::|"));
    assert!(forms.contains("RefinedTypeTree"));
    assert_eq!(first.get("InfixOp::|"), Some(&1));
    assert_eq!(first.get("InfixOp::&"), Some(&1));
    assert_eq!(first.get("InfixOp::<other>"), Some(&0));
    assert_eq!(first.get("PostfixOp::*"), Some(&1));
    assert_eq!(first.get("SingletonTypeTree"), Some(&1));
    assert!(first["TypeBoundsTree"] > 0);
    assert!(first["RefinedTypeTree"] > 0);
    assert!(first["LambdaTypeTree"] > 0);
    assert!(first["ContextBoundTypeTree"] > 0);
    assert!(first["Function"] > 0);
    assert!(first["Parens"] > 0);
    assert_eq!(first.get("Select"), Some(&1));
}

#[test]
fn singleton_type_histogram_does_not_count_its_term_path() {
    let source = "class C { def f(value: Any, singleton: value.type): Unit = () }";
    let mut store = SemanticStore::new();
    let scanner = ContextualScanner::new(source).unwrap();
    let parsed = parse_compilation_unit(
        SourceText::new(source).unwrap(),
        SourceId::from_index(0),
        scanner,
        &mut store.names,
    );
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);

    let histogram = collect_declared_type_tree_histogram(&parsed.ast, &store.names);
    assert_eq!(histogram.get("SingletonTypeTree"), Some(&1));
    assert_eq!(histogram.get("Ident"), Some(&2));
}

#[test]
fn declared_type_tree_histogram_counts_pattern_types_and_omits_empty_placeholders() {
    let source = "object Audit { def inferred = { val x = 1; x }; def patterned = { val (a, b): (A | B, C) = pair; val (x: A, y: B) = pair; val Some(z: D) = option } }";
    let mut store = SemanticStore::new();
    let scanner = ContextualScanner::new(source).unwrap();
    let parsed = parse_compilation_unit(
        SourceText::new(source).unwrap(),
        SourceId::from_index(0),
        scanner,
        &mut store.names,
    );
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);

    let histogram = collect_declared_type_tree_histogram(&parsed.ast, &store.names);
    assert_eq!(histogram.get("InfixOp::|"), Some(&1));
    assert_eq!(histogram.get("TypeTree"), Some(&0));
    assert_eq!(histogram.get("Tuple"), Some(&1));
    assert_eq!(histogram.get("Ident"), Some(&6));
}

#[test]
fn local_expression_audit_schema_keeps_zero_count_forms_without_an_ast() {
    let audit = Audit::default();

    assert_eq!(audit.expression_forms.len(), EXPRESSION_FORMS.len());
    assert!(audit.expression_forms.values().all(|count| *count == 0));
    assert_eq!(audit.type_tree_forms.len(), TYPE_TREE_FORMS.len());
    assert!(audit.type_tree_forms.values().all(|count| *count == 0));
    assert_eq!(audit.type_tree_form_files.len(), TYPE_TREE_FORMS.len());
    assert!(audit.type_tree_form_files.values().all(BTreeSet::is_empty));
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

#[test]
fn unsupported_type_tree_failure_order_is_stable() {
    let fixtures = [
        (
            "UnsupportedTypeTree::TypeBoundsTree",
            2,
            ["b.scala", "a.scala"],
        ),
        ("UnsupportedTypeTree::InfixOp::|", 3, ["d.scala", "c.scala"]),
        (
            "UnsupportedTypeTree::SingletonTypeTree",
            2,
            ["f.scala", "e.scala"],
        ),
    ];
    let mut first = Audit::default();
    for (bucket, count, paths) in fixtures {
        let failure = FailureClassification {
            bucket: bucket.to_owned(),
            family: FailureFamily::Other,
        };
        for path in paths.into_iter().take(count.min(2)) {
            record_failure(&mut first, failure.clone(), path);
        }
        for _ in 2..count {
            record_failure(&mut first, failure.clone(), paths[0]);
        }
    }
    let mut second = Audit::default();
    for (bucket, count, paths) in fixtures.into_iter().rev() {
        let failure = FailureClassification {
            bucket: bucket.to_owned(),
            family: FailureFamily::Other,
        };
        for _ in 2..count {
            record_failure(&mut second, failure.clone(), paths[0]);
        }
        for path in paths.into_iter().take(count.min(2)).rev() {
            record_failure(&mut second, failure.clone(), path);
        }
    }

    assert_eq!(
        ordered_type_tree_failures(&first)
            .into_iter()
            .map(|(name, bucket)| (name.clone(), bucket.count))
            .collect::<Vec<_>>(),
        ordered_type_tree_failures(&second)
            .into_iter()
            .map(|(name, bucket)| (name.clone(), bucket.count))
            .collect::<Vec<_>>()
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

fn probe_supported_match_cases(
    text: &str,
    path: &str,
    classpath: SharedClassPath,
    profile: &mut MatchProfile,
) -> BTreeMap<String, usize> {
    let source = SourceId::from_index(0);
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let Ok(scanner) = ContextualScanner::new(text) else {
        return BTreeMap::new();
    };
    let mut parsed = parse_compilation_unit(
        SourceText::new(text).expect("source text should be valid"),
        source,
        scanner,
        &mut store.names,
    );
    let type_operator_spellings = source_operator_spellings(&parsed.ast, &store.names);
    let mut namer_packages = Packages::new();
    let Ok(index) = name_compilation_unit(
        &parsed.ast,
        parsed.root,
        source,
        path,
        &mut store,
        &mut namer_packages,
    ) else {
        return BTreeMap::new();
    };
    let method_ranges = parsed
        .ast
        .iter()
        .filter_map(|(tree, node)| {
            let TreeKind::DefDef(definition) = &node.kind else {
                return None;
            };
            let method = index.symbol_at(source, tree)?;
            let rhs = definition.rhs?;
            let range = parsed.ast.get(rhs).position?.span().range();
            Some((method, range))
        })
        .collect::<Vec<_>>();
    let mut probe_specs = Vec::new();
    for (_, node) in parsed.ast.iter() {
        let TreeKind::Match(matched) = &node.kind else {
            continue;
        };
        let Some(match_range) = node.position.map(|position| position.span().range()) else {
            continue;
        };
        let Some((method, _)) = method_ranges
            .iter()
            .filter(|(_, range)| {
                range.start() <= match_range.start() && match_range.end() <= range.end()
            })
            .min_by_key(|(_, range)| range.end().saturating_sub(range.start()))
        else {
            continue;
        };
        for case_tree in &matched.cases {
            let Some(case_node) = parsed.ast.try_get(*case_tree) else {
                continue;
            };
            let TreeKind::CaseDef(case) = &case_node.kind else {
                continue;
            };
            let family = pattern_root_shape(&parsed.ast, &store.names, case.pattern);
            if !is_supported_case_family(&family) {
                continue;
            }
            probe_specs.push((
                *method,
                matched.selector,
                *case_tree,
                node.position,
                family,
                case.guard.is_some(),
                extractor_protocol_success(&parsed.ast, case.pattern),
            ));
        }
    }
    let probes = probe_specs
        .into_iter()
        .map(
            |(method, selector, case, position, family, guarded, protocol)| {
                let synthetic_match = parsed.ast.alloc(Tree {
                    kind: TreeKind::Match(Match {
                        selector,
                        cases: vec![case],
                    }),
                    position,
                    ty: (),
                });
                (method, synthetic_match, family, guarded, protocol)
            },
        )
        .collect::<Vec<_>>();

    let packages = Packages::new();
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );
    typer = typer.with_resolver(Box::new(AuditResolver {
        inner: ClasspathSymbolResolver::new(
            classpath,
            definitions,
            LoadingSession::with_packages(Packages::new()),
        ),
        metrics: Rc::new(RefCell::new(ResolverMetrics::default())),
    }));

    let mut successes = [
        "wildcard",
        "variable/bind",
        "literal",
        "stable identifier/selection",
        "guarded supported case",
        "typed wildcard",
        "typed variable",
        "typed explicit Bind",
        "tuple",
        "alternative",
        "extractor-looking Apply",
        "extractor-looking TypeApply",
        "infix pattern",
    ]
    .into_iter()
    .map(|family| (family.to_owned(), 0))
    .collect::<BTreeMap<_, _>>();
    for (method, match_tree, family, guarded, protocol) in probes {
        let outcome = typer
            .expression_context_for(method)
            .and_then(|context| typer.type_expression(match_tree, context));
        match outcome {
            Ok(_) => {
                let bucket = family_success_bucket(&family).to_owned();
                *successes.entry(bucket.clone()).or_default() += 1;
                profile
                    .typed_case_success_files
                    .entry(bucket)
                    .or_default()
                    .insert(path.to_owned());
                if let Some(protocol) = protocol {
                    *profile
                        .extractor_protocol_successes
                        .entry(protocol.to_owned())
                        .or_default() += 1;
                }
                if guarded {
                    *successes
                        .entry("guarded supported case".to_owned())
                        .or_default() += 1;
                }
            }
            Err(error) => {
                let error_name = typed_case_failure_label(
                    &error,
                    &parsed.ast,
                    &type_operator_spellings,
                    &typer.store().names,
                );
                *profile
                    .typed_case_failures
                    .entry(error_name.clone())
                    .or_default() += 1;
                profile
                    .typed_case_failure_files
                    .entry(error_name)
                    .or_default()
                    .insert(path.to_owned());
                let boundary = match &error {
                    TyperError::TypedPatternRuntimeTestDeferred { .. } => {
                        Some("generic/erased or unsupported runtime test")
                    }
                    TyperError::TypedPatternRelationDeferred { .. } => {
                        Some("unsupported typed-pattern relation")
                    }
                    TyperError::TypedPatternTypeMismatch { .. } => {
                        Some("typed-pattern type mismatch")
                    }
                    TyperError::UnsupportedTypeTree { .. } => {
                        Some("unsupported type-tree projection")
                    }
                    _ => None,
                };
                if family.starts_with("typed")
                    && let Some(boundary) = boundary
                {
                    *profile
                        .typed_pattern_boundaries
                        .entry(boundary.to_owned())
                        .or_default() += 1;
                }
            }
        }
    }
    successes
}

fn is_supported_case_family(family: &str) -> bool {
    matches!(
        family,
        "wildcard"
            | "variable identifier"
            | "wildcard/identifier/bind"
            | "literal"
            | "stable identifier"
            | "stable selection"
            | "typed wildcard"
            | "typed variable"
            | "typed explicit Bind"
            | "tuple"
            | "alternative"
            | "extractor-looking Apply"
            | "extractor-looking TypeApply"
            | "infix pattern"
    )
}

fn family_success_bucket(family: &str) -> &'static str {
    match family {
        "wildcard" => "wildcard",
        "variable identifier" | "wildcard/identifier/bind" => "variable/bind",
        "literal" => "literal",
        "stable identifier" | "stable selection" => "stable identifier/selection",
        "typed wildcard" => "typed wildcard",
        "typed variable" => "typed variable",
        "typed explicit Bind" => "typed explicit Bind",
        "tuple" => "tuple",
        "alternative" => "alternative",
        "extractor-looking Apply" => "extractor-looking Apply",
        "extractor-looking TypeApply" => "extractor-looking TypeApply",
        "infix pattern" => "infix pattern",
        _ => "other",
    }
}

fn extractor_protocol_success(
    arena: &dotty_core::AstArena<Untyped>,
    pattern: dotty_core::TreeId<Untyped>,
) -> Option<&'static str> {
    match arena.try_get(pattern).map(|node| &node.kind) {
        Some(TreeKind::PhaseSpecific(UntypedNode::Tuple(tuple))) if !tuple.elements.is_empty() => {
            Some("tuple extractor")
        }
        Some(TreeKind::Apply(application)) => {
            let dispatch = peel_type_applications(arena, application.function);
            let selected = matches!(
                arena.try_get(dispatch).map(|node| &node.kind),
                Some(TreeKind::Select(_))
            );
            match application.args.len() {
                0 => Some("Boolean extractor"),
                1 if selected => Some("selected unary extractor"),
                1 => Some("unary Option-like extractor"),
                2 => Some("binary product extractor"),
                _ => Some("N-ary product extractor"),
            }
        }
        Some(TreeKind::TypeApply(_)) => {
            let function = peel_type_applications(arena, pattern);
            match arena.try_get(function).map(|node| &node.kind) {
                Some(TreeKind::Ident(_)) | Some(TreeKind::Select(_)) => {
                    Some("type-applied extractor (unsupported by design)")
                }
                _ => None,
            }
        }
        _ => None,
    }
}

fn typed_case_failure_label(
    error: &TyperError,
    arena: &dotty_core::AstArena<Untyped>,
    operator_spellings: &BTreeMap<u32, String>,
    names: &dotty_core::names::NameInterner,
) -> String {
    match error {
        TyperError::TuplePatternResolutionDeferred { issue, .. } => {
            format!("tuple::{issue:?}")
        }
        TyperError::ExtractorPatternArgumentUnsupported { issue, .. } => {
            format!("extractor argument::{issue:?}")
        }
        TyperError::UnsupportedTypeTree { .. } => {
            classify_typer_error(error, arena, operator_spellings, names).bucket
        }
        _ => typer_error_name(error).to_owned(),
    }
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
            return Audit {
                files_attempted: 1,
                parser_failed_files: 1,
                ..Audit::default()
            };
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
    audit.patdef_profile = collect_local_patdefs(&parsed.ast, &store.names, path);
    audit.expression_forms = collect_expression_histogram(&parsed.ast);
    audit.singleton_source_inventory = collect_singleton_source_inventory(&parsed.ast, path);
    audit.prefix_operator_forms = collect_prefix_operator_histogram(&parsed.ast, &store.names);
    let (type_tree_forms, type_tree_form_names, _type_tree_nodes) =
        collect_declared_type_tree_inventory(&parsed.ast, &store.names);
    audit.type_tree_forms = type_tree_forms;
    if !type_tree_form_names.is_empty() {
        for form in type_tree_form_names {
            audit
                .type_tree_form_files
                .entry(form)
                .or_default()
                .insert(path.to_owned());
        }
    }
    audit.recovered_parser_files = usize::from(!parsed.diagnostics.is_empty());
    collect_match_profile(&parsed.ast, path, &store, &mut audit.match_profile);
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
    if audit.local_defdefs == 0 && audit.patdef_profile.total == 0 {
        return audit;
    }
    let probe_classpath = classpath.as_ref().map(|(classpath, _)| classpath.clone());

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
            Some((method, tree, rhs, range))
        })
        .collect::<Vec<_>>();

    let local_method_trees = local_method_trees(&parsed.ast);
    let match_pattern_shapes = parsed
        .ast
        .iter()
        .filter_map(|(_, node)| {
            let TreeKind::CaseDef(case) = &node.kind else {
                return None;
            };
            Some((
                case.pattern.index(),
                pattern_root_shape(&parsed.ast, &store.names, case.pattern),
            ))
        })
        .collect::<BTreeMap<_, _>>();

    let (typer_packages, resolver_packages) = if classpath.is_some() {
        (Packages::new(), Some(packages))
    } else {
        (packages, None)
    };
    let type_operator_spellings = source_operator_spellings(&parsed.ast, &store.names);
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &typer_packages,
    );
    if let (Some((classpath, metrics)), Some(packages)) = (classpath, resolver_packages) {
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
    let mut method_ranges = root_methods
        .iter()
        .map(|(_, _, _, range)| *range)
        .collect::<Vec<_>>();
    method_ranges.extend(parsed.ast.iter().filter_map(|(_, node)| {
        matches!(&node.kind, TreeKind::DefDef(definition) if definition.rhs.is_some())
            .then(|| node.position.map(|position| position.span().range()))
            .flatten()
    }));
    for (method, method_tree, rhs, range) in root_methods {
        let outcome = typer
            .expression_context_for(method)
            .and_then(|context| typer.type_expression(rhs, context));
        if let Err(error) = outcome {
            let failure = classify_typer_error(
                &error,
                &parsed.ast,
                &type_operator_spellings,
                &typer.store().names,
            );
            if matches!(
                error,
                TyperError::UnsupportedPattern { .. }
                    | TyperError::MalformedCaseDef { .. }
                    | TyperError::EmptyMatchCases { .. }
                    | TyperError::MatchSelectorTypeCannotBeAdapted { .. }
                    | TyperError::MatchCaseResultTypeCannotBeWidened { .. }
                    | TyperError::MatchCaseJoinUnsupported { .. }
                    | TyperError::MalformedVariablePattern { .. }
                    | TyperError::UnsupportedBindPatternBody { .. }
                    | TyperError::PatternTypeMismatch { .. }
                    | TyperError::PatternTypeRelationDeferred { .. }
                    | TyperError::TypedPatternTypeMismatch { .. }
                    | TyperError::TypedPatternRelationDeferred { .. }
                    | TyperError::TypedPatternRuntimeTestDeferred { .. }
                    | TyperError::MalformedStablePatternTarget { .. }
                    | TyperError::UnstablePatternValue { .. }
                    | TyperError::MalformedPatternBinding { .. }
                    | TyperError::WildcardPatternBindingRejected { .. }
                    | TyperError::PatternBindingOutsideCaseScope { .. }
                    | TyperError::PatternBindingScopeConflict { .. }
                    | TyperError::DuplicatePatternBinding { .. }
                    | TyperError::ExtractorQualifierNotFound { .. }
                    | TyperError::ExtractorQualifierMemberNotFound { .. }
                    | TyperError::ExtractorQualifierMemberAmbiguous { .. }
                    | TyperError::ExtractorQualifierShapeUnsupported { .. }
                    | TyperError::ExtractorQualifierNotValueLike { .. }
                    | TyperError::ExtractorQualifierNotStable { .. }
                    | TyperError::ExtractorUnapplyNotFound { .. }
                    | TyperError::ExtractorUnapplyOverloaded { .. }
                    | TyperError::ExtractorUnapplyPolymorphic { .. }
                    | TyperError::ExtractorUnapplyShapeUnsupported { .. }
                    | TyperError::ExtractorPatternConstraintDeferred { .. }
                    | TyperError::UnsupportedExtractorResultProtocol { .. }
                    | TyperError::ExtractorPatternArityUnsupported { .. }
                    | TyperError::BooleanExtractorPatternArityUnsupported { .. }
                    | TyperError::ExtractorResultMemberNotFound { .. }
                    | TyperError::ExtractorResultMemberOverloaded { .. }
                    | TyperError::ExtractorResultMemberUnsupported { .. }
                    | TyperError::ExtractorProductSelectorCountMismatch { .. }
                    | TyperError::UnsupportedExtractorProductProtocol { .. }
                    | TyperError::ExtractorPatternArgumentUnsupported { .. }
            ) {
                collect_match_readiness(
                    &parsed.ast,
                    rhs,
                    path,
                    match_first_error_label(&error, &match_pattern_shapes),
                    &mut audit.match_readiness,
                );
            }
            let missing_type_tree = match &error {
                TyperError::MissingDeclaredType { tree_index, .. } => Some(*tree_index),
                _ => None,
            };
            let singleton_reference_tree = match &error {
                TyperError::UnsupportedSingletonReference { tree_index, .. } => Some(*tree_index),
                _ => None,
            };
            let local_method_signature = match &error {
                TyperError::LocalMethodSignatureDeferred {
                    tree_index,
                    feature,
                    ..
                } => Some((*tree_index, (*feature).to_owned())),
                _ => None,
            };
            let local_value_declaration = match &error {
                TyperError::LocalBlockDeclarationDeferred {
                    tree_index,
                    kind: "val/var definition",
                    ..
                } => Some(*tree_index),
                TyperError::LocalValueModifierDeferred {
                    tree_index,
                    classification: "declaration semantics deferred",
                    ..
                } => Some(*tree_index),
                _ => None,
            };
            root_failures.push(RootFailure {
                range,
                enclosing_method_tree: method_tree.index(),
                failure,
                missing_type_tree,
                singleton_reference_tree,
                local_method_signature,
                local_value_declaration,
            });
        }
    }

    for (tree, node) in parsed.ast.iter() {
        let TreeKind::PhaseSpecific(UntypedNode::PatDef(definition)) = &node.kind else {
            continue;
        };
        if !audit
            .patdef_profile
            .structural_patdef_ids
            .contains(&tree.index())
        {
            continue;
        }
        let Some(range) = node.position.map(|position| position.span().range()) else {
            continue;
        };
        if !method_ranges
            .iter()
            .any(|method| method.start() <= range.start() && range.end() <= method.end())
        {
            continue;
        }
        let mut binders = BTreeSet::new();
        for pattern in &definition.patterns {
            collect_patdef_binders(&parsed.ast, &typer.store().names, *pattern, &mut binders);
        }
        let binder_bucket = match binders.len() {
            0 => "0",
            1 => "1",
            2 => "2",
            _ => "3+",
        };
        let root = definition.patterns.first().map_or_else(
            || "none".to_owned(),
            |pattern| patdef_root_shape(&parsed.ast, &typer.store().names, *pattern),
        );
        let tpt = patdef_type_annotation(&parsed.ast, definition.tpt);
        let (outcome, failure) = match typer.patdef_typing_attempt_at(source, tree) {
            Some(Ok(())) => ("success".to_owned(), None),
            Some(Err(bucket)) => {
                let family = if bucket.starts_with("LocalPatDefDeferred::")
                    || bucket.starts_with("PatDefAggregateArityDeferred::")
                {
                    FailureFamily::LocalDeclarationDeferral
                } else {
                    FailureFamily::Other
                };
                let failure = FailureClassification {
                    bucket: bucket.clone(),
                    family,
                };
                (format!("failure::{bucket}"), Some(failure))
            }
            None => ("not_attempted".to_owned(), None),
        };
        let key = format!(
            "{outcome}::{}::binders={binder_bucket}::root={root}::tpt={tpt}",
            patdef_modifier(&definition.modifiers)
        );
        let bucket = audit.patdef_profile.typing_outcomes.entry(key).or_default();
        bucket.family = failure.map_or(FailureFamily::Other, |failure| failure.family);
        bucket.count += 1;
        bucket.files.insert(path.to_owned());
        bucket.examples.insert(path.to_owned());
        bucket.examples = bucket.examples.iter().take(5).cloned().collect();
    }

    for (tree, range) in local_method_trees.clone() {
        if typer.source_typed_index().get(source, tree).is_some() {
            audit.typed_local_defdefs += 1;
            audit
                .local_method_first_blockers
                .insert(format!("{path}#tree={}", tree.index()), "typed".to_owned());
            if (path == "compiler/src/dotty/tools/dotc/core/SymUtils.scala"
                || path == "compiler/src/dotty/tools/dotc/typer/Typer.scala")
                && let TreeKind::DefDef(definition) = &parsed.ast.get(tree).kind
            {
                let name = typer
                    .store()
                    .names
                    .resolve(definition.name.as_name().text());
                if matches!(
                    (path, name),
                    (
                        "compiler/src/dotty/tools/dotc/core/SymUtils.scala",
                        "instantiateCFT"
                    ) | ("compiler/src/dotty/tools/dotc/typer/Typer.scala", "cases")
                ) {
                    audit
                        .by_name_method_outcomes
                        .insert(format!("{path}::{name}"), "typed".to_owned());
                }
            }
        } else {
            let (
                kind,
                missing_type_tree,
                singleton_reference_tree,
                local_method_signature,
                local_value_declaration,
                blocker_method_tree,
            ) = root_failures
                .iter()
                .filter(|failure| {
                    failure.range.start() <= range.start() && range.end() <= failure.range.end()
                })
                .min_by_key(|failure| failure.range.end().saturating_sub(failure.range.start()))
                .map(|failure| {
                    (
                        failure.failure.clone(),
                        failure.missing_type_tree,
                        failure.singleton_reference_tree,
                        failure.local_method_signature.clone(),
                        failure.local_value_declaration,
                        Some(failure.enclosing_method_tree),
                    )
                })
                .unwrap_or_else(|| {
                    (
                        FailureClassification {
                            bucket: "NoSuccessfulEnclosingMethodTyping".to_owned(),
                            family: FailureFamily::Other,
                        },
                        None,
                        None,
                        None,
                        None,
                        None,
                    )
                });
            if kind.bucket == "UnsupportedSingletonReference" {
                let detail = singleton_reference_tree
                    .and_then(|reference_tree| {
                        singleton_reference_detail(
                            &parsed.ast,
                            &typer.store().names,
                            path,
                            text,
                            tree,
                            reference_tree,
                        )
                    })
                    .unwrap_or_else(|| {
                        singleton_reference_fallback_detail(path, tree, singleton_reference_tree)
                    });
                audit.singleton_reference_profile.record(detail);
            }
            audit
                .local_method_first_blockers
                .insert(format!("{path}#tree={}", tree.index()), kind.bucket.clone());
            if kind.bucket == "LocalBlockDeclarationDeferred::val/var definition"
                && let (Some(declaration_tree), Some(blocker_method_tree)) =
                    (local_value_declaration, blocker_method_tree)
                && let Some(observation) = local_value_blocker_observation(
                    &parsed.ast,
                    &typer.store().names,
                    text,
                    path,
                    tree,
                    declaration_tree,
                    blocker_method_tree,
                )
            {
                audit.local_value_blocker_profile.record(observation);
            }
            if (path == "compiler/src/dotty/tools/dotc/core/SymUtils.scala"
                || path == "compiler/src/dotty/tools/dotc/typer/Typer.scala")
                && let TreeKind::DefDef(definition) = &parsed.ast.get(tree).kind
            {
                let name = typer
                    .store()
                    .names
                    .resolve(definition.name.as_name().text());
                if matches!(
                    (path, name),
                    (
                        "compiler/src/dotty/tools/dotc/core/SymUtils.scala",
                        "instantiateCFT"
                    ) | ("compiler/src/dotty/tools/dotc/typer/Typer.scala", "cases")
                ) {
                    audit
                        .by_name_method_outcomes
                        .insert(format!("{path}::{name}"), kind.bucket.clone());
                }
            }
            if kind.bucket == "LocalMethodSignatureDeferred" {
                let (origin_tree_index, feature) = local_method_signature.unwrap_or_else(|| {
                    panic!(
                        "missing exact local method signature blocker origin for {path}#tree={}",
                        tree.index()
                    )
                });
                let direct = origin_tree_index == tree.index();
                let origin_name = parsed
                    .ast
                    .iter()
                    .find(|(origin_tree, _)| origin_tree.index() == origin_tree_index)
                    .and_then(|(_, origin_node)| match &origin_node.kind {
                        TreeKind::DefDef(definition) => Some(
                            typer
                                .store()
                                .names
                                .resolve(definition.name.as_name().text()),
                        ),
                        _ => None,
                    })
                    .unwrap_or("<unknown>");
                let (method_key, mut record) = local_method_signature_record(
                    &parsed.ast,
                    &typer.store().names,
                    text,
                    path,
                    tree,
                    &feature,
                )
                .unwrap_or_else(|| {
                    panic!(
                        "missing local method source shape for {path}#tree={}",
                        tree.index()
                    )
                });
                let origin_key = format!("{path}#tree={origin_tree_index}:{origin_name}");
                record.push_str(&format!(
                    " blocker_origin={origin_key} attribution={}",
                    if direct { "direct" } else { "inherited" }
                ));
                audit
                    .local_method_signature_profile
                    .record(feature, path, method_key, origin_key, direct, record);
            }
            if kind.bucket == "MissingDeclaredType"
                && let Some(tree_index) = missing_type_tree
            {
                let detail = missing_declared_type_detail(
                    &parsed.ast,
                    &index,
                    typer.store(),
                    source,
                    tree_index,
                );
                audit.missing_declared_type_profile.record(
                    detail.bucket,
                    path,
                    format!(
                        "{path}: failed_local_method_tree={} {}",
                        tree.index(),
                        detail.record
                    ),
                    format!("{path}#{}", detail.tree_index),
                );
            }
            record_failure(&mut audit, kind, path);
        }
    }
    collect_source_function_method_outcomes(
        SourceFunctionMethodAudit {
            arena: &parsed.ast,
            source,
            path,
            source_text: text,
            names: &typer.store().names,
            methods: &local_method_trees,
            root_failures: &root_failures,
            typer: &typer,
        },
        &mut audit.source_function_method_outcomes,
        &mut audit.source_function_outcomes,
    );
    if let Some(classpath) = probe_classpath {
        let successes =
            probe_supported_match_cases(text, path, classpath, &mut audit.match_profile);
        audit.match_profile.typed_case_successes = successes;
    }
    audit
}

fn print_match_readiness(readiness: &MatchReadiness) {
    println!("match_readiness:");
    println!(
        "  first_blocker_methods={}",
        readiness.first_blocker_methods
    );
    println!(
        "  structural_matches_in_first_blocker_methods={}",
        readiness.matches
    );
    println!("  cases_in_first_blocker_methods={}", readiness.cases);
    println!("  guarded_cases={}", readiness.guarded_cases);
    println!(
        "  unguarded_cases={}",
        readiness.cases - readiness.guarded_cases
    );
    println!("  first_blocker_errors:");
    for (error, count) in &readiness.first_blocker_errors {
        let files = readiness
            .first_blocker_error_files
            .get(error)
            .into_iter()
            .flatten()
            .take(5)
            .cloned()
            .collect::<Vec<_>>();
        println!("    {error}={count} files=[{}]", files.join(", "));
    }
    println!("  pattern_root_shapes:");
    for (shape, count) in &readiness.pattern_shapes {
        let files = readiness
            .pattern_shape_files
            .get(shape)
            .into_iter()
            .flatten()
            .take(5)
            .cloned()
            .collect::<Vec<_>>();
        println!("    {shape}={count} files=[{}]", files.join(", "));
    }
}

fn collect_match_readiness(
    arena: &dotty_core::AstArena<Untyped>,
    root: dotty_core::TreeId<Untyped>,
    path: &str,
    first_error: String,
    readiness: &mut MatchReadiness,
) {
    readiness.first_blocker_methods += 1;
    *readiness
        .first_blocker_errors
        .entry(first_error.clone())
        .or_default() += 1;
    readiness
        .first_blocker_error_files
        .entry(first_error)
        .or_default()
        .insert(path.to_owned());
    let nodes = arena
        .iter()
        .map(|(tree, node)| (tree.index(), node))
        .collect::<BTreeMap<_, _>>();
    let mut visited = HashSet::new();
    let mut pending = VecDeque::from([root]);
    while let Some(tree) = pending.pop_front() {
        if !visited.insert(tree) {
            continue;
        }
        let Some(node) = nodes.get(&tree.index()) else {
            continue;
        };
        if let TreeKind::Match(matched) = &node.kind {
            readiness.matches += 1;
            for case_tree in &matched.cases {
                let Some(case_node) = nodes.get(&case_tree.index()) else {
                    continue;
                };
                let TreeKind::CaseDef(case) = &case_node.kind else {
                    continue;
                };
                readiness.cases += 1;
                if case.guard.is_some() {
                    readiness.guarded_cases += 1;
                }
                let Some(pattern_node) = nodes.get(&case.pattern.index()) else {
                    continue;
                };
                let shape = match &pattern_node.kind {
                    TreeKind::Ident(_) | TreeKind::Bind(_) => "wildcard/identifier/bind",
                    TreeKind::Literal(_) | TreeKind::PhaseSpecific(UntypedNode::Number(_)) => {
                        "literal"
                    }
                    TreeKind::Typed(_) => "typed pattern",
                    TreeKind::Alternative(_) => "alternative",
                    TreeKind::PhaseSpecific(UntypedNode::Tuple(_)) => "tuple",
                    TreeKind::Apply(_) | TreeKind::TypeApply(_) => {
                        "extractor-looking Apply/TypeApply"
                    }
                    TreeKind::UnApply(_) => "UnApply",
                    _ => "other",
                };
                *readiness
                    .pattern_shapes
                    .entry(shape.to_owned())
                    .or_default() += 1;
                readiness
                    .pattern_shape_files
                    .entry(shape.to_owned())
                    .or_default()
                    .insert(path.to_owned());
            }
        }
        pending.extend(term_expression_children(&node.kind));
        if let TreeKind::DefDef(definition) = &node.kind {
            pending.extend(definition.rhs);
        }
    }
}

fn collect_match_profile(
    arena: &dotty_core::AstArena<Untyped>,
    path: &str,
    store: &SemanticStore,
    profile: &mut MatchProfile,
) {
    for (_, node) in arena.iter() {
        let TreeKind::Match(matched) = &node.kind else {
            continue;
        };
        profile.matches += 1;
        for case_tree in &matched.cases {
            let Some(case_node) = arena.try_get(*case_tree) else {
                continue;
            };
            let TreeKind::CaseDef(case) = &case_node.kind else {
                continue;
            };
            profile.cases += 1;
            profile.guarded_cases += usize::from(case.guard.is_some());
            let shape = pattern_root_shape(arena, &store.names, case.pattern);
            *profile.pattern_roots.entry(shape.clone()).or_default() += 1;
            profile
                .pattern_root_files
                .entry(shape)
                .or_default()
                .insert(path.to_owned());
            if matches!(
                arena.try_get(case.pattern).map(|pattern| &pattern.kind),
                Some(TreeKind::Apply(_)) | Some(TreeKind::TypeApply(_))
            ) {
                profile.extractor_files.insert(path.to_owned());
            }
            collect_pattern_features(arena, &store.names, case.pattern, profile);
        }
    }
}

fn pattern_root_shape(
    arena: &dotty_core::AstArena<Untyped>,
    names: &dotty_core::NameInterner,
    tree: dotty_core::TreeId<Untyped>,
) -> String {
    let Some(node) = arena.try_get(tree) else {
        return "other".to_owned();
    };
    match &node.kind {
        TreeKind::Ident(ident) if names.resolve(ident.name.text()) == "_" && !ident.backquoted => {
            "wildcard".to_owned()
        }
        TreeKind::Ident(ident) if is_variable_pattern_ident(names, ident.name, ident.backquoted) => {
            "variable identifier".to_owned()
        }
        TreeKind::Ident(_) => "stable identifier".to_owned(),
        TreeKind::Select(_) => "stable selection".to_owned(),
        TreeKind::Bind(binding)
            if arena.try_get(binding.body).is_some_and(|body| {
                matches!(&body.kind, TreeKind::Typed(typed) if is_wildcard_or_variable_pattern(arena, names, typed.expr))
            }) =>
        {
            "typed explicit Bind".to_owned()
        }
        TreeKind::Bind(_) => "wildcard/identifier/bind".to_owned(),
        TreeKind::Literal(_) | TreeKind::PhaseSpecific(UntypedNode::Number(_)) => {
            "literal".to_owned()
        }
        TreeKind::Typed(typed) => {
            let Some(expr) = arena.try_get(typed.expr) else {
                return "typed pattern".to_owned();
            };
            match &expr.kind {
                TreeKind::Ident(ident)
                    if names.resolve(ident.name.text()) == "_" && !ident.backquoted =>
                {
                    "typed wildcard".to_owned()
                }
                TreeKind::Ident(ident)
                    if is_variable_pattern_ident(names, ident.name, ident.backquoted) =>
                {
                    "typed variable".to_owned()
                }
                _ => "typed pattern".to_owned(),
            }
        }
        TreeKind::Alternative(_) => "alternative".to_owned(),
        TreeKind::PhaseSpecific(UntypedNode::Tuple(_)) => "tuple".to_owned(),
        TreeKind::Apply(_) => "extractor-looking Apply".to_owned(),
        TreeKind::TypeApply(_) => "extractor-looking TypeApply".to_owned(),
        TreeKind::UnApply(_) => "UnApply".to_owned(),
        TreeKind::PhaseSpecific(UntypedNode::InfixOp(_)) => "infix pattern".to_owned(),
        _ => "other".to_owned(),
    }
}

fn is_variable_pattern_ident(
    names: &dotty_core::NameInterner,
    name: dotty_core::Name,
    backquoted: bool,
) -> bool {
    if backquoted || !name.is_term() {
        return false;
    }
    let spelling = names.resolve(name.text());
    if matches!(spelling, "_" | "true" | "false" | "null") {
        return false;
    }
    spelling
        .chars()
        .next()
        .is_some_and(|first| first == '_' || (first.is_alphabetic() && first.is_lowercase()))
}

fn is_wildcard_or_variable_pattern(
    arena: &dotty_core::AstArena<Untyped>,
    names: &dotty_core::NameInterner,
    tree: dotty_core::TreeId<Untyped>,
) -> bool {
    arena.try_get(tree).is_some_and(|node| match &node.kind {
        TreeKind::Ident(ident) => {
            !ident.backquoted
                && (names.resolve(ident.name.text()) == "_"
                    || is_variable_pattern_ident(names, ident.name, false))
        }
        _ => false,
    })
}

fn collect_pattern_features(
    arena: &dotty_core::AstArena<Untyped>,
    names: &dotty_core::NameInterner,
    root: dotty_core::TreeId<Untyped>,
    profile: &mut MatchProfile,
) {
    if matches!(
        arena.try_get(root).map(|node| &node.kind),
        Some(TreeKind::TypeApply(_))
    ) {
        collect_extractor_profile(arena, names, root, profile);
    }
    let mut visited = HashSet::new();
    let mut pending = vec![root];
    while let Some(tree) = pending.pop() {
        if !visited.insert(tree) {
            continue;
        }
        let Some(node) = arena.try_get(tree) else {
            continue;
        };
        match &node.kind {
            TreeKind::NamedArg(named) => {
                profile.named_pattern_arguments += 1;
                pending.push(named.arg);
            }
            TreeKind::Apply(application) => {
                collect_extractor_profile(arena, names, tree, profile);
                pending.extend(application.args.iter().copied());
            }
            TreeKind::TypeApply(application) => pending.push(application.function),
            TreeKind::Typed(typed) => pending.push(typed.expr),
            TreeKind::Bind(binding) => pending.push(binding.body),
            TreeKind::Alternative(alternative) => {
                pending.extend(alternative.alternatives.iter().copied());
            }
            TreeKind::UnApply(unapply) => pending.extend(unapply.patterns.iter().copied()),
            TreeKind::PhaseSpecific(UntypedNode::Tuple(tuple)) => {
                if tuple.elements.is_empty() {
                    profile.empty_tuple_unit_patterns += 1;
                }
                pending.extend(tuple.elements.iter().copied());
            }
            TreeKind::PhaseSpecific(UntypedNode::Parens(parens)) => pending.push(parens.inner),
            TreeKind::PhaseSpecific(UntypedNode::InfixOp(infix)) => {
                profile.infix_pattern_forms += 1;
                pending.extend([infix.left, infix.right]);
            }
            TreeKind::PhaseSpecific(UntypedNode::PostfixOp(postfix)) => {
                if names.resolve(postfix.op.text()) == "*" {
                    profile.sequence_wildcards += 1;
                }
                pending.push(postfix.operand);
            }
            _ => {}
        }
    }
}

fn collect_extractor_profile(
    arena: &dotty_core::AstArena<Untyped>,
    names: &dotty_core::NameInterner,
    root: dotty_core::TreeId<Untyped>,
    profile: &mut MatchProfile,
) {
    let root_is_type_apply = matches!(
        arena.try_get(root).map(|node| &node.kind),
        Some(TreeKind::TypeApply(_))
    );
    *profile
        .extractor_roots
        .entry(
            if root_is_type_apply {
                "TypeApply root"
            } else {
                "Apply root"
            }
            .to_owned(),
        )
        .or_default() += 1;
    let mut cursor = root;
    let mut type_applied = false;
    while let Some(node) = arena.try_get(cursor) {
        match &node.kind {
            TreeKind::TypeApply(application) => {
                type_applied = true;
                cursor = application.function;
            }
            TreeKind::Apply(application) => {
                type_applied |= count_type_applications(arena, application.function) > 0;
                *profile
                    .extractor_argument_counts
                    .entry(application.args.len())
                    .or_default() += 1;
                for argument in &application.args {
                    let shape = pattern_root_shape(arena, names, *argument);
                    *profile.extractor_nested_roots.entry(shape).or_default() += 1;
                }
                let function = peel_type_applications(arena, application.function);
                let dispatch = match arena.try_get(function).map(|node| &node.kind) {
                    Some(TreeKind::Ident(_)) => "simple extractor identifier",
                    Some(TreeKind::Select(_)) => "selected extractor",
                    _ => "other extractor function shape",
                };
                *profile
                    .extractor_dispatch
                    .entry(dispatch.to_owned())
                    .or_default() += 1;
                if type_applied {
                    profile.extractor_type_applied += 1;
                }
                return;
            }
            _ => return,
        }
    }
    if type_applied {
        profile.extractor_type_applied += 1;
    }
}

fn count_type_applications(
    arena: &dotty_core::AstArena<Untyped>,
    mut tree: dotty_core::TreeId<Untyped>,
) -> usize {
    let mut count = 0;
    while let Some(node) = arena.try_get(tree) {
        let TreeKind::TypeApply(application) = &node.kind else {
            break;
        };
        count += 1;
        tree = application.function;
    }
    count
}

fn peel_type_applications(
    arena: &dotty_core::AstArena<Untyped>,
    mut tree: dotty_core::TreeId<Untyped>,
) -> dotty_core::TreeId<Untyped> {
    while let Some(node) = arena.try_get(tree) {
        let TreeKind::TypeApply(application) = &node.kind else {
            break;
        };
        tree = application.function;
    }
    tree
}

fn print_match_profile(profile: &MatchProfile) {
    println!("match_corpus_profile:");
    println!("  matches={}", profile.matches);
    println!("  cases={}", profile.cases);
    println!("  guarded_cases={}", profile.guarded_cases);
    println!(
        "  unguarded_cases={}",
        profile.cases - profile.guarded_cases
    );
    println!("  pattern_root_shapes:");
    for (shape, count) in &profile.pattern_roots {
        let files = profile
            .pattern_root_files
            .get(shape)
            .into_iter()
            .flatten()
            .take(5)
            .cloned()
            .collect::<Vec<_>>();
        println!("    {shape}={count} files=[{}]", files.join(", "));
    }
    println!("  typed_case_successes:");
    for (family, count) in &profile.typed_case_successes {
        let files = profile
            .typed_case_success_files
            .get(family)
            .into_iter()
            .flatten()
            .take(5)
            .cloned()
            .collect::<Vec<_>>();
        println!("    {family}={count} files=[{}]", files.join(", "));
    }
    println!("  successful_extractor_protocols:");
    for (protocol, count) in &profile.extractor_protocol_successes {
        println!("    {protocol}={count}");
    }
    println!("  typed_pattern_boundaries:");
    for (boundary, count) in &profile.typed_pattern_boundaries {
        println!("    {boundary}={count}");
    }
    println!("  typed_case_first_failures:");
    let unsupported_type_tree_total = profile
        .typed_case_failures
        .iter()
        .filter(|(failure, _)| failure.starts_with("UnsupportedTypeTree::"))
        .map(|(_, count)| *count)
        .sum::<usize>();
    println!("    UnsupportedTypeTree={unsupported_type_tree_total}");
    for (failure, count) in &profile.typed_case_failures {
        let files = profile
            .typed_case_failure_files
            .get(failure)
            .into_iter()
            .flatten()
            .take(5)
            .cloned()
            .collect::<Vec<_>>();
        println!("    {failure}={count} files=[{}]", files.join(", "));
    }
    let mut ranked = profile
        .typed_case_failures
        .iter()
        .map(|(failure, count)| (failure.as_str(), *count))
        .collect::<Vec<_>>();
    ranked.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(right.0)));
    println!("  typed_case_first_failure_top_10:");
    for (failure, count) in ranked.into_iter().take(10) {
        println!("    {failure}={count}");
    }
    println!("  extractor_root_shapes:");
    for (shape, count) in &profile.extractor_roots {
        println!("    {shape}={count}");
    }
    println!("  extractor_dispatch:");
    for (shape, count) in &profile.extractor_dispatch {
        println!("    {shape}={count}");
    }
    println!(
        "  extractor_type_applied={}",
        profile.extractor_type_applied
    );
    println!("  extractor_argument_counts:");
    for (count, occurrences) in &profile.extractor_argument_counts {
        println!("    {count}={occurrences}");
    }
    println!("  extractor_nested_argument_roots:");
    for (shape, count) in &profile.extractor_nested_roots {
        println!("    {shape}={count}");
    }
    println!(
        "  sequence_wildcard_occurrences={}",
        profile.sequence_wildcards
    );
    println!(
        "  named_pattern_arguments={}",
        profile.named_pattern_arguments
    );
    println!(
        "  empty_tuple_unit_patterns={}",
        profile.empty_tuple_unit_patterns
    );
    println!("  infix_pattern_forms={}", profile.infix_pattern_forms);
    println!(
        "  extractor_representative_files=[{}]",
        profile
            .extractor_files
            .iter()
            .take(5)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ")
    );
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
    println!(
        "  resolver_package_requests={}",
        metrics.resolver_package_requests
    );
    println!(
        "  external_package_requests={}",
        metrics.external_package_requests()
    );
    println!("  external_package_successes={}", metrics.package_successes);
    println!(
        "  external_package_unresolved={}",
        metrics.package_unresolved
    );
    println!("  external_package_errors={}", metrics.package_errors);
    println!("  source_package_reuse={}", metrics.package_source_reuse);
    println!(
        "  resolver_member_requests={}",
        metrics.resolver_member_requests
    );
    println!(
        "  external_member_requests={}",
        metrics.external_member_requests()
    );
    println!("  external_member_successes={}", metrics.member_successes);
    println!(
        "  external_class_symbol_successes={}",
        metrics.class_symbol_successes
    );
    println!(
        "  external_non_class_member_successes={}",
        metrics
            .member_successes
            .saturating_sub(metrics.class_symbol_successes)
    );
    println!("  source_member_reuse={}", metrics.member_source_reuse);
    println!("  external_member_unresolved={}", metrics.member_unresolved);
    println!("  external_member_errors={}", metrics.member_errors);
    println!("  distinct_packages={}", metrics.packages.len());
    println!("  distinct_classes={}", metrics.classes.len());
    println!("  distinct_members={}", metrics.members.len());
    println!(
        "  classloader_success_gate={}",
        if !metrics.classes.is_empty() && !metrics.members.is_empty() {
            "passed"
        } else {
            "BLOCKED: external members not materialized"
        }
    );
    println!("resolver_581_comparison:");
    for (name, current, baseline) in [
        (
            "external_package_successes",
            metrics.package_successes,
            1_965,
        ),
        (
            "external_package_unresolved",
            metrics.package_unresolved,
            5_816,
        ),
        ("external_package_errors", metrics.package_errors, 0),
        ("external_class_materializations", metrics.classes.len(), 0),
        (
            "external_non_class_member_successes",
            metrics
                .member_successes
                .saturating_sub(metrics.class_symbol_successes),
            0,
        ),
        (
            "external_member_unresolved",
            metrics.member_unresolved,
            3_884,
        ),
        ("external_member_errors", metrics.member_errors, 91),
        ("distinct_packages", metrics.packages.len(), 23),
    ] {
        println!(
            "  {name}={current} (baseline={baseline}, delta={})",
            signed_delta(current, baseline)
        );
    }
    println!("  member_error_kinds:");
    for (kind, count) in &metrics.member_error_kinds {
        println!("    {kind}={count}");
    }
    println!("  most_requested_unresolved_member_names:");
    let mut unresolved = metrics.unresolved_member_names.iter().collect::<Vec<_>>();
    unresolved.sort_by(|(name_a, count_a), (name_b, count_b)| {
        count_b.cmp(count_a).then(name_a.cmp(name_b))
    });
    for (name, count) in unresolved.into_iter().take(20) {
        println!("    {name}={count}");
    }
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

fn collect_local_patdefs(
    arena: &dotty_core::AstArena<Untyped>,
    names: &dotty_core::names::NameInterner,
    path: &str,
) -> PatDefProfile {
    let nodes = arena
        .iter()
        .map(|(tree, node)| (tree.index(), node))
        .collect::<BTreeMap<_, _>>();
    let mut profile = PatDefProfile::default();
    let mut visited = HashSet::new();
    for (_, node) in arena.iter() {
        let TreeKind::DefDef(definition) = &node.kind else {
            continue;
        };
        let Some(rhs) = definition.rhs else {
            continue;
        };
        let mut pending = VecDeque::from([rhs]);
        while let Some(tree) = pending.pop_front() {
            if !visited.insert(tree) {
                continue;
            }
            let Some(node) = nodes.get(&tree.index()) else {
                continue;
            };
            if let TreeKind::PhaseSpecific(UntypedNode::PatDef(definition)) = &node.kind {
                profile.total += 1;
                profile.structural_patdef_ids.insert(tree.index());
                *profile
                    .source_pattern_counts
                    .entry(definition.patterns.len().to_string())
                    .or_default() += 1;
                *profile
                    .modifiers
                    .entry(patdef_modifier(&definition.modifiers).to_owned())
                    .or_default() += 1;
                *profile
                    .explicit_tpt
                    .entry(patdef_type_annotation(arena, definition.tpt).to_owned())
                    .or_default() += 1;
                *profile
                    .rhs_states
                    .entry(patdef_rhs_state(&nodes, definition.rhs).to_owned())
                    .or_default() += 1;

                let mut binders = BTreeSet::new();
                for pattern in &definition.patterns {
                    let shape = patdef_root_shape(arena, names, *pattern);
                    *profile.root_shapes.entry(shape.clone()).or_default() += 1;
                    profile
                        .root_shape_files
                        .entry(shape)
                        .or_default()
                        .insert(path.to_owned());
                    collect_patdef_binders(arena, names, *pattern, &mut binders);
                }
                let binder_bucket = match binders.len() {
                    0 => "0",
                    1 => "1",
                    2 => "2",
                    _ => "3+",
                };
                *profile
                    .binder_counts
                    .entry(binder_bucket.to_owned())
                    .or_default() += 1;
                profile.representative_files.insert(path.to_owned());
            }
            pending.extend(term_expression_children(&node.kind));
        }
    }
    profile
}

fn local_value_blocker_observation(
    arena: &dotty_core::AstArena<Untyped>,
    names: &dotty_core::names::NameInterner,
    source_text: &str,
    path: &str,
    enclosing_method_tree: dotty_core::TreeId<Untyped>,
    declaration_tree_index: u32,
    blocker_method_tree_index: u32,
) -> Option<LocalValueBlockerObservation> {
    let declaration_tree = arena
        .iter()
        .find_map(|(tree, _)| (tree.index() == declaration_tree_index).then_some(tree))?;
    let declaration = arena.try_get(declaration_tree)?;
    let span = declaration.position.map(|position| position.span().range());
    let declaration_start = span.map_or(0, |range| range.start() as usize);
    let line = source_text
        .get(..declaration_start.min(source_text.len()))
        .map_or(1, |prefix| {
            prefix.bytes().filter(|byte| *byte == b'\n').count() + 1
        });
    let (
        node_kind,
        declaration_name,
        declaration_kind,
        modifiers,
        visibility,
        annotations,
        tpt,
        rhs,
        pattern_roots,
        source_pattern_count,
        binder_count,
    ) = match &declaration.kind {
        TreeKind::ValDef(definition) => {
            let name = names.resolve(definition.name.as_name().text());
            (
                "ValDef".to_owned(),
                if name.is_empty() {
                    "<anonymous>".to_owned()
                } else {
                    name.to_owned()
                },
                source_declaration_kind(&definition.metadata),
                source_modifier_list(&definition.metadata),
                source_visibility(names, definition.metadata.visibility),
                !definition.metadata.annotations.is_empty(),
                Some(definition.tpt),
                definition.rhs.is_some(),
                String::new(),
                None,
                None,
            )
        }
        TreeKind::PhaseSpecific(UntypedNode::PatDef(definition)) => {
            let mut binders = BTreeSet::new();
            for pattern in &definition.patterns {
                collect_patdef_binders(arena, names, *pattern, &mut binders);
            }
            let binder_count = binders.len();
            let roots = definition
                .patterns
                .iter()
                .map(|pattern| patdef_root_shape(arena, names, *pattern))
                .collect::<Vec<_>>()
                .join(",");
            (
                "PatDef".to_owned(),
                if binders.is_empty() {
                    "<pattern>".to_owned()
                } else {
                    binders.into_iter().collect::<Vec<_>>().join(",")
                },
                source_declaration_kind(&definition.modifiers),
                source_modifier_list(&definition.modifiers),
                source_visibility(names, definition.modifiers.visibility),
                !definition.modifiers.annotations.is_empty(),
                Some(definition.tpt),
                definition.rhs.is_some(),
                roots,
                Some(definition.patterns.len()),
                Some(binder_count),
            )
        }
        _ => (
            format!("{:?}", declaration.kind),
            "<none>".to_owned(),
            "other".to_owned(),
            String::new(),
            "default".to_owned(),
            false,
            None,
            false,
            String::new(),
            None,
            None,
        ),
    };
    let type_form = tpt.map_or_else(
        || "unavailable".to_owned(),
        |tree| source_type_form(arena, tree),
    );
    let dispatch_path = match node_kind.as_str() {
        "ValDef" => "type_block_stat_expansion -> type_value_expression_inner -> type_local_value: LocalValueModifierDeferred (audit bucket mapped to LocalBlockDeclarationDeferred::val/var definition)".to_owned(),
        "PatDef" => "type_block_stat_expansion -> type_local_patdef".to_owned(),
        _ => "type_block_stat_expansion: generic declaration fallback".to_owned(),
    };
    let range = span.map_or_else(
        || "unknown".to_owned(),
        |range| format!("{}..{}", range.start(), range.end()),
    );
    let method_name = arena
        .try_get(enclosing_method_tree)
        .and_then(|node| match &node.kind {
            TreeKind::DefDef(definition) => Some(names.resolve(definition.name.as_name().text())),
            _ => None,
        })
        .unwrap_or("<unknown>");
    let blocker_origin_method = arena
        .iter()
        .find_map(|(tree, node)| {
            (tree.index() == blocker_method_tree_index).then(|| match &node.kind {
                TreeKind::DefDef(definition) => {
                    names.resolve(definition.name.as_name().text()).to_owned()
                }
                _ => "<unknown>".to_owned(),
            })
        })
        .unwrap_or_else(|| "<unknown>".to_owned());

    Some(LocalValueBlockerObservation {
        path: path.to_owned(),
        enclosing_method: format!(
            "{path}#tree={}:{}",
            enclosing_method_tree.index(),
            method_name
        ),
        enclosing_method_tree: enclosing_method_tree.index(),
        blocker_origin_method: format!(
            "{path}#tree={blocker_method_tree_index}:{blocker_origin_method}"
        ),
        declaration_tree: declaration_tree_index,
        line,
        span: range,
        node_kind,
        declaration_name,
        declaration_kind,
        dispatch_path,
        modifiers,
        visibility,
        annotations,
        type_form,
        rhs_present: rhs,
        pattern_roots,
        source_pattern_count,
        binder_count,
        attribution: if enclosing_method_tree.index() == blocker_method_tree_index {
            "direct".to_owned()
        } else {
            "inherited".to_owned()
        },
    })
}

fn source_declaration_kind(modifiers: &dotty_core::ast::Modifiers) -> String {
    let kind = if modifiers.modifiers.contains(&Modifier::Given) {
        "given"
    } else if modifiers.modifiers.contains(&Modifier::Implicit) {
        "implicit val"
    } else if modifiers.modifiers.contains(&Modifier::Lazy) {
        "lazy val"
    } else if modifiers.modifiers.contains(&Modifier::Inline) {
        "inline val"
    } else if modifiers.modifiers.contains(&Modifier::Var) {
        "var"
    } else {
        "val"
    };
    kind.to_owned()
}

fn source_modifier_list(modifiers: &dotty_core::ast::Modifiers) -> String {
    modifiers
        .modifiers
        .iter()
        .map(|modifier| format!("{modifier:?}"))
        .collect::<Vec<_>>()
        .join(",")
}

fn source_visibility(
    names: &dotty_core::names::NameInterner,
    visibility: Option<VisibilitySyntax>,
) -> String {
    match visibility {
        None => "default".to_owned(),
        Some(VisibilitySyntax::Private { qualifier }) => {
            format!(
                "private{}",
                qualifier.map_or_else(String::new, |name| {
                    format!("[{}]", names.resolve(name.text()))
                })
            )
        }
        Some(VisibilitySyntax::Protected { qualifier }) => format!(
            "protected{}",
            qualifier.map_or_else(String::new, |name| {
                format!("[{}]", names.resolve(name.text()))
            })
        ),
    }
}

fn source_type_form(
    arena: &dotty_core::AstArena<Untyped>,
    tree: dotty_core::TreeId<Untyped>,
) -> String {
    let Some(node) = arena.try_get(tree) else {
        return "unknown".to_owned();
    };
    if matches!(node.kind, TreeKind::TypeTree(_))
        && node
            .position
            .is_none_or(|position| position.span().range().is_empty())
    {
        "inferred".to_owned()
    } else {
        "explicit".to_owned()
    }
}

fn patdef_modifier(modifiers: &dotty_core::ast::Modifiers) -> &'static str {
    use dotty_core::ast::Modifier;
    if modifiers.modifiers.contains(&Modifier::Lazy) {
        "lazy val"
    } else if modifiers.modifiers.contains(&Modifier::Var) {
        "var"
    } else {
        "val"
    }
}

fn patdef_type_annotation(
    arena: &dotty_core::AstArena<Untyped>,
    tree: dotty_core::TreeId<Untyped>,
) -> &'static str {
    let node = arena.try_get(tree);
    if node.is_some_and(|node| {
        matches!(node.kind, TreeKind::TypeTree(_))
            && node
                .position
                .is_none_or(|position| position.span().range().is_empty())
    }) {
        "synthetic inferred TypeTree"
    } else {
        "explicit PatDef-wide tpt"
    }
}

fn patdef_rhs_state(
    nodes: &BTreeMap<u32, &dotty_core::Tree<Untyped>>,
    rhs: Option<dotty_core::TreeId<Untyped>>,
) -> &'static str {
    let Some(rhs) = rhs else {
        return "missing";
    };
    let mut pending = vec![rhs];
    let mut visited = HashSet::new();
    while let Some(tree) = pending.pop() {
        if !visited.insert(tree) {
            continue;
        }
        let Some(node) = nodes.get(&tree.index()) else {
            continue;
        };
        if matches!(node.kind, TreeKind::PhaseSpecific(UntypedNode::Error(_))) {
            return "recovery error";
        }
        pending.extend(term_expression_children(&node.kind));
    }
    "present"
}

fn patdef_root_shape(
    arena: &dotty_core::AstArena<Untyped>,
    names: &dotty_core::names::NameInterner,
    mut tree: dotty_core::TreeId<Untyped>,
) -> String {
    loop {
        match arena.try_get(tree).map(|node| &node.kind) {
            Some(TreeKind::PhaseSpecific(UntypedNode::Parens(parens))) => tree = parens.inner,
            Some(TreeKind::PhaseSpecific(UntypedNode::Tuple(_))) => return "Tuple".to_owned(),
            Some(TreeKind::Apply(_))
            | Some(TreeKind::TypeApply(_))
            | Some(TreeKind::UnApply(_)) => {
                return "Apply / extractor-looking".to_owned();
            }
            Some(TreeKind::Bind(_)) => return "Bind".to_owned(),
            Some(TreeKind::PhaseSpecific(UntypedNode::InfixOp(_))) => {
                return "InfixOp".to_owned();
            }
            Some(TreeKind::Typed(_)) => return "Typed".to_owned(),
            Some(TreeKind::Alternative(_)) => return "Alternative".to_owned(),
            Some(TreeKind::Ident(ident))
                if !ident.backquoted && names.resolve(ident.name.text()) == "_" =>
            {
                return "wildcard".to_owned();
            }
            Some(TreeKind::Ident(_)) => return "Ident".to_owned(),
            _ => return "other".to_owned(),
        }
    }
}

fn collect_patdef_binders(
    arena: &dotty_core::AstArena<Untyped>,
    names: &dotty_core::names::NameInterner,
    root: dotty_core::TreeId<Untyped>,
    binders: &mut BTreeSet<String>,
) {
    let mut pending = vec![root];
    let mut visited = HashSet::new();
    while let Some(tree) = pending.pop() {
        if !visited.insert(tree) {
            continue;
        }
        let Some(node) = arena.try_get(tree) else {
            continue;
        };
        match &node.kind {
            TreeKind::Ident(ident)
                if is_variable_pattern_ident(names, ident.name, ident.backquoted) =>
            {
                binders.insert(names.resolve(ident.name.text()).to_owned());
            }
            TreeKind::Bind(binding) => {
                let name = names.resolve(binding.name.text());
                if name != "_" {
                    binders.insert(name.to_owned());
                }
                pending.push(binding.body);
            }
            TreeKind::Typed(typed) => pending.push(typed.expr),
            // Dotty 3.9 reports variables under Alternative as illegal and
            // does not expose them as PatDef binders (Desugar.getVariables).
            TreeKind::Alternative(_) => {}
            TreeKind::Apply(application) => pending.extend(application.args.iter().copied()),
            TreeKind::UnApply(unapply) => pending.extend(unapply.patterns.iter().copied()),
            TreeKind::NamedArg(argument) => pending.push(argument.arg),
            TreeKind::Annotated(annotated) => pending.push(annotated.expr),
            TreeKind::PhaseSpecific(UntypedNode::Parens(parens)) => pending.push(parens.inner),
            TreeKind::PhaseSpecific(UntypedNode::Tuple(tuple)) => {
                pending.extend(tuple.elements.iter().copied());
            }
            TreeKind::PhaseSpecific(UntypedNode::InfixOp(infix)) => {
                pending.extend([infix.left, infix.right]);
            }
            _ => {}
        }
    }
}

fn print_local_patdefs(profile: &PatDefProfile) {
    println!("local_patdefs:");
    println!("  total={}", profile.total);
    println!("  root_shapes:");
    for (shape, count) in &profile.root_shapes {
        let files = profile
            .root_shape_files
            .get(shape)
            .into_iter()
            .flatten()
            .take(5)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        println!("    {shape}={count} files=[{files}]");
    }
    print_patdef_counts("source_pattern_counts", &profile.source_pattern_counts);
    print_patdef_counts("binder_counts", &profile.binder_counts);
    print_patdef_counts("modifiers", &profile.modifiers);
    print_patdef_counts("explicit_tpt", &profile.explicit_tpt);
    print_patdef_counts("rhs", &profile.rhs_states);
    let mut outcome_totals = BTreeMap::<String, (usize, BTreeSet<String>)>::new();
    for (key, bucket) in &profile.typing_outcomes {
        let status_end = ["::lazy val::", "::val::", "::var::"]
            .iter()
            .filter_map(|marker| key.find(marker))
            .min()
            .unwrap_or(key.len());
        let status = &key[..status_end];
        let total = outcome_totals.entry(status.to_owned()).or_default();
        total.0 += bucket.count;
        total.1.extend(bucket.files.iter().cloned());
    }
    println!("  typing_outcome_totals:");
    for (outcome, (count, files)) in outcome_totals {
        println!("    {outcome}={count} files={}", files.len());
    }
    let mut attempt_statuses = BTreeMap::<String, (usize, BTreeSet<String>)>::new();
    for (key, bucket) in &profile.typing_outcomes {
        let status = if key == "success" || key.starts_with("success::") {
            "success"
        } else if key == "not_attempted" || key.starts_with("not_attempted::") {
            "not_attempted"
        } else {
            "failure"
        };
        let total = attempt_statuses.entry(status.to_owned()).or_default();
        total.0 += bucket.count;
        total.1.extend(bucket.files.iter().cloned());
    }
    let profiled_count = attempt_statuses
        .values()
        .map(|(count, _)| count)
        .sum::<usize>();
    println!("  typing_attempt_statuses:");
    println!("    profiled={profiled_count}");
    println!(
        "    outside_typed_method_ranges_or_without_source_range={}",
        profile.total.saturating_sub(profiled_count)
    );
    for status in ["success", "failure", "not_attempted"] {
        let (count, files) = attempt_statuses.get(status).cloned().unwrap_or_default();
        println!("    {status}={count} files={}", files.len());
    }
    println!("  typing_outcomes:");
    for (outcome, bucket) in &profile.typing_outcomes {
        println!(
            "    {outcome}={} files={} [{}]",
            bucket.count,
            bucket.files.len(),
            bucket
                .examples
                .iter()
                .take(5)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    println!(
        "  representative_files=[{}]",
        profile
            .representative_files
            .iter()
            .take(5)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ")
    );
}

fn print_local_value_blocker_profile(profile: &LocalValueBlockerProfile) {
    println!("local_value_blocker_profile:");
    println!("  occurrences={}", profile.observations.len());
    println!(
        "  distinct_declarations={}",
        profile.distinct_declaration_count()
    );
    print_local_value_groups(&profile.observations, "node_kind", |row| {
        row.node_kind.clone()
    });
    print_local_value_groups(&profile.observations, "declaration_kind", |row| {
        row.declaration_kind.clone()
    });
    print_local_value_groups(&profile.observations, "modifier_set", |row| {
        row.modifiers.clone()
    });
    print_local_value_modifier_presence(&profile.observations);
    print_local_value_groups(&profile.observations, "inferred_vs_explicit", |row| {
        row.type_form.clone()
    });
    print_local_value_groups(&profile.observations, "direct_vs_inherited", |row| {
        row.attribution.clone()
    });
    print_local_value_groups(&profile.observations, "direct_valdef_groups", |row| {
        format!(
            "kind={} modifiers=[{}] type={} name={} annotations={} visibility={}",
            row.declaration_kind,
            row.modifiers,
            row.type_form,
            if row.declaration_kind == "given" {
                if row.declaration_name == "<anonymous>" {
                    "anonymous given"
                } else {
                    "named given"
                }
            } else {
                "ordinary named local"
            },
            row.annotations,
            row.visibility
        )
    });
    print_local_value_groups(&profile.observations, "patdef_groups", |row| {
        format!(
            "root={} source_patterns={} binders={} kind={} type={} modifiers=[{}]",
            row.pattern_roots,
            row.source_pattern_count
                .map_or_else(|| "n/a".to_owned(), |count| count.to_string()),
            row.binder_count
                .map_or_else(|| "n/a".to_owned(), |count| count.to_string()),
            row.declaration_kind,
            row.type_form,
            row.modifiers
        )
    });
    println!("  observations:");
    for row in &profile.observations {
        println!(
            "    {}:method={} blocker_origin={} declaration_tree={} line={} span={} node={} name={} kind={} dispatch={} modifiers=[{}] visibility={} annotations={} type={} rhs={} pattern_roots=[{}] source_patterns={} binders={} attribution={}",
            row.path,
            row.enclosing_method,
            row.blocker_origin_method,
            row.declaration_tree,
            row.line,
            row.span,
            row.node_kind,
            row.declaration_name,
            row.declaration_kind,
            row.dispatch_path,
            row.modifiers,
            row.visibility,
            row.annotations,
            row.type_form,
            row.rhs_present,
            row.pattern_roots,
            row.source_pattern_count
                .map_or_else(|| "n/a".to_owned(), |count| count.to_string()),
            row.binder_count
                .map_or_else(|| "n/a".to_owned(), |count| count.to_string()),
            row.attribution,
        );
    }
}

fn print_local_value_modifier_presence(rows: &BTreeSet<LocalValueBlockerObservation>) {
    println!("  modifier_presence:");
    for modifier in ["Var", "Given", "Implicit", "Lazy", "Inline", "Final"] {
        let matching = rows
            .iter()
            .filter(|row| row.modifiers.split(',').any(|value| value == modifier))
            .collect::<Vec<_>>();
        let declarations = matching
            .iter()
            .map(|row| (row.path.as_str(), row.declaration_tree))
            .collect::<BTreeSet<_>>();
        let files = matching
            .iter()
            .map(|row| row.path.as_str())
            .collect::<BTreeSet<_>>();
        println!(
            "    {modifier}={} distinct_declarations={} files={}",
            matching.len(),
            declarations.len(),
            files.len()
        );
    }
    let unmodified = rows.iter().filter(|row| row.modifiers.is_empty()).count();
    println!("    (no modifiers)={unmodified}");
}

fn print_local_value_groups(
    rows: &BTreeSet<LocalValueBlockerObservation>,
    label: &str,
    key: impl Fn(&LocalValueBlockerObservation) -> String,
) {
    let mut groups = BTreeMap::<String, (usize, BTreeSet<(String, u32)>, BTreeSet<String>)>::new();
    for row in rows {
        if label == "direct_valdef_groups" && row.node_kind != "ValDef" {
            continue;
        }
        if label == "patdef_groups" && row.node_kind != "PatDef" {
            continue;
        }
        let group = groups.entry(key(row)).or_default();
        group.0 += 1;
        group.1.insert((row.path.clone(), row.declaration_tree));
        group.2.insert(row.path.clone());
    }
    println!("  {label}:");
    if groups.is_empty() {
        println!("    (none)");
        return;
    }
    for (name, (occurrences, declarations, files)) in groups {
        println!(
            "    {name}: occurrences={occurrences} distinct_declarations={} files={} [{}]",
            declarations.len(),
            files.len(),
            files.into_iter().collect::<Vec<_>>().join(", ")
        );
    }
}

fn print_patdef_counts(name: &str, counts: &BTreeMap<String, usize>) {
    println!("  {name}:");
    for (shape, count) in counts {
        println!("    {shape}={count}");
    }
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

struct SourceFunctionMethodAudit<'a, 'typer> {
    arena: &'a dotty_core::AstArena<Untyped>,
    source: SourceId,
    path: &'a str,
    source_text: &'a str,
    names: &'a dotty_core::names::NameInterner,
    methods: &'a [(dotty_core::TreeId<Untyped>, TextRange)],
    root_failures: &'a [RootFailure],
    typer: &'a SourceTyper<'typer>,
}

fn collect_source_function_method_outcomes(
    audit: SourceFunctionMethodAudit<'_, '_>,
    records: &mut BTreeSet<String>,
    outcomes: &mut BTreeMap<String, FailureBucket>,
) {
    const BASELINE_METHODS: &[(&str, usize, &str, &str)] = &[
        (
            "expression::Function",
            737,
            "refersTo",
            "compiler/src/dotty/tools/dotc/core/unpickleScala2/Scala2Unpickler.scala",
        ),
        (
            "expression::Function",
            752,
            "removeSingleton",
            "compiler/src/dotty/tools/dotc/core/unpickleScala2/Scala2Unpickler.scala",
        ),
        (
            "expression::Function",
            754,
            "mapArg",
            "compiler/src/dotty/tools/dotc/core/unpickleScala2/Scala2Unpickler.scala",
        ),
        (
            "expression::Function",
            758,
            "elim",
            "compiler/src/dotty/tools/dotc/core/unpickleScala2/Scala2Unpickler.scala",
        ),
        (
            "expression::Function",
            710,
            "factoryManifest",
            "compiler/src/dotty/tools/dotc/typer/Synthesizer.scala",
        ),
        (
            "expression::Function",
            718,
            "singletonManifest",
            "compiler/src/dotty/tools/dotc/typer/Synthesizer.scala",
        ),
        (
            "expression::Function",
            721,
            "synthArrayManifest",
            "compiler/src/dotty/tools/dotc/typer/Synthesizer.scala",
        ),
        (
            "expression::Function",
            727,
            "synthWildcardManifest",
            "compiler/src/dotty/tools/dotc/typer/Synthesizer.scala",
        ),
        (
            "expression::Function",
            731,
            "synthArgManifests",
            "compiler/src/dotty/tools/dotc/typer/Synthesizer.scala",
        ),
        (
            "expression::Function",
            741,
            "canManifest",
            "compiler/src/dotty/tools/dotc/typer/Synthesizer.scala",
        ),
        (
            "expression::Function",
            748,
            "synthManifest",
            "compiler/src/dotty/tools/dotc/typer/Synthesizer.scala",
        ),
        (
            "expression::Function",
            770,
            "manifestOfType",
            "compiler/src/dotty/tools/dotc/typer/Synthesizer.scala",
        ),
        (
            "expression::Function",
            774,
            "synthesize",
            "compiler/src/dotty/tools/dotc/typer/Synthesizer.scala",
        ),
        (
            "type::Function",
            671,
            "ifInit",
            "compiler/src/dotty/tools/backend/jvm/BTypes.scala",
        ),
        (
            "type::Function",
            673,
            "isJLO",
            "compiler/src/dotty/tools/backend/jvm/BTypes.scala",
        ),
        (
            "type::Function",
            2387,
            "genArgs",
            "compiler/src/dotty/tools/backend/sjs/JSCodeGen.scala",
        ),
        (
            "type::Function",
            2388,
            "genArgsAsClassCaptures",
            "compiler/src/dotty/tools/backend/sjs/JSCodeGen.scala",
        ),
        (
            "type::Function",
            3386,
            "genScalaArgs",
            "compiler/src/dotty/tools/backend/sjs/JSCodeGen.scala",
        ),
        (
            "type::Function",
            3387,
            "genJSArgs",
            "compiler/src/dotty/tools/backend/sjs/JSCodeGen.scala",
        ),
        (
            "type::Function",
            310,
            "argStr",
            "compiler/src/dotty/tools/dotc/core/Denotations.scala",
        ),
        (
            "type::Function",
            3448,
            "normalize",
            "compiler/src/dotty/tools/dotc/core/Types.scala",
        ),
        (
            "type::Function",
            3395,
            "maybeAscription",
            "compiler/src/dotty/tools/dotc/parsing/Parsers.scala",
        ),
        (
            "type::Function",
            366,
            "unusable",
            "compiler/src/dotty/tools/dotc/transform/PostTyper.scala",
        ),
        (
            "type::Function",
            409,
            "isPoly",
            "compiler/src/dotty/tools/dotc/typer/ProtoTypes.scala",
        ),
        (
            "type::Function",
            416,
            "fun",
            "library/src/scala/util/control/Exception.scala",
        ),
    ];
    for (method_tree, method_range) in audit.methods {
        let node = audit.arena.get(*method_tree);
        let TreeKind::DefDef(definition) = &node.kind else {
            continue;
        };
        let method_name = audit
            .names
            .resolve(definition.name.as_name().text())
            .to_owned();
        let line = audit.source_text.as_bytes()[..method_range.start() as usize]
            .iter()
            .filter(|byte| **byte == b'\n')
            .count()
            + 1;
        let Some((form, _, _, _)) =
            BASELINE_METHODS
                .iter()
                .find(|(_, baseline_line, baseline_name, baseline_path)| {
                    *baseline_line == line
                        && *baseline_name == method_name
                        && *baseline_path == audit.path
                })
        else {
            continue;
        };
        let typed = audit
            .typer
            .source_typed_index()
            .get(audit.source, *method_tree)
            .is_some();
        let blocker = if typed {
            "Typed"
        } else {
            audit
                .root_failures
                .iter()
                .filter(|failure| {
                    failure.range.start() <= method_range.start()
                        && method_range.end() <= failure.range.end()
                })
                .min_by_key(|failure| failure.range.len())
                .map_or("NoSuccessfulEnclosingMethodTyping", |failure| {
                    failure.failure.bucket.as_str()
                })
        };
        records.insert(format!(
            "{}:{line}:{method_name} baseline_form={form} first_blocker={blocker}",
            audit.path
        ));
        let key = format!("{form}::{blocker}");
        let bucket = outcomes.entry(key).or_default();
        bucket.count += 1;
        bucket.files.insert(audit.path.to_owned());
        bucket
            .examples
            .insert(format!("{}:{line}:{method_name}", audit.path));
        bucket.examples = bucket.examples.iter().take(5).cloned().collect();
    }
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

fn local_pattern_type_trees(
    arena: &dotty_core::AstArena<Untyped>,
) -> HashSet<dotty_core::TreeId<Untyped>> {
    let mut pending = local_stat_trees(arena)
        .into_iter()
        .flat_map(|tree| match &arena.get(tree).kind {
            TreeKind::PhaseSpecific(UntypedNode::PatDef(definition)) => definition.patterns.clone(),
            _ => Vec::new(),
        })
        .collect::<Vec<_>>();
    let mut visited = HashSet::new();
    let mut types = HashSet::new();
    while let Some(tree) = pending.pop() {
        if !visited.insert(tree) {
            continue;
        }
        match &arena.get(tree).kind {
            TreeKind::Typed(typed) => {
                types.insert(typed.tpt);
                pending.push(typed.expr);
            }
            TreeKind::Bind(binding) => pending.push(binding.body),
            TreeKind::Alternative(alternative) => {
                pending.extend(alternative.alternatives.iter().copied());
            }
            TreeKind::UnApply(unapply) => pending.extend(unapply.patterns.iter().copied()),
            TreeKind::Apply(application) => pending.extend(application.args.iter().copied()),
            TreeKind::NamedArg(argument) => pending.push(argument.arg),
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
    types
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

fn singleton_reference_detail(
    arena: &dotty_core::AstArena<Untyped>,
    names: &dotty_core::NameInterner,
    path: &str,
    source_text: &str,
    method: dotty_core::TreeId<Untyped>,
    reference_index: u32,
) -> Option<SingletonReferenceDetail> {
    let (reference, reference_node) = arena
        .iter()
        .find(|(tree, _)| tree.index() == reference_index)?;
    let singleton = arena.iter().find_map(|(tree, node)| match &node.kind {
        TreeKind::SingletonTypeTree(singleton) if singleton.reference == reference => {
            Some((tree, singleton))
        }
        _ => None,
    })?;
    let method_node = arena.try_get(method)?;
    let TreeKind::DefDef(method_def) = &method_node.kind else {
        return None;
    };
    let method_name = names.resolve(method_def.name.as_name().text());
    let method_range = method_node.position?.span().range();
    let method_line = source_text.as_bytes()[..method_range.start() as usize]
        .iter()
        .filter(|byte| **byte == b'\n')
        .count()
        + 1;

    let (declaration, declaration_shape, snippet) = arena
        .iter()
        .find_map(|(tree, node)| match &node.kind {
            TreeKind::ValDef(definition) if definition.tpt == singleton.0 => {
                let name = names.resolve(definition.name.as_name().text());
                let mutable = definition
                    .metadata
                    .modifiers
                    .contains(&dotty_core::ast::Modifier::Var);
                let shape = format!(
                    "tree={} ValDef(name={name},rhs={},mutable={mutable})",
                    tree.index(),
                    definition.rhs.is_some()
                );
                let snippet = node
                    .position
                    .and_then(|position| {
                        let range = position.span().range();
                        source_text.get(range.start() as usize..range.end() as usize)
                    })
                    .unwrap_or("")
                    .trim()
                    .to_owned();
                Some((shape.clone(), shape, snippet))
            }
            TreeKind::TypeDef(definition) if definition.rhs == singleton.0 => {
                let name = names.resolve(definition.name.as_name().text());
                let shape = format!("tree={} TypeDef(name={name})", tree.index());
                let snippet = node
                    .position
                    .and_then(|position| {
                        let range = position.span().range();
                        source_text.get(range.start() as usize..range.end() as usize)
                    })
                    .unwrap_or("")
                    .trim()
                    .to_owned();
                Some((shape.clone(), shape, snippet))
            }
            _ => None,
        })
        .unwrap_or_else(|| {
            (
                format!("unowned singleton tree {}", singleton.0.index()),
                "unknown enclosing declaration".to_owned(),
                String::new(),
            )
        });
    let reference_shape = singleton_reference_shape(&reference_node.kind, names);
    let reference_span = reference_node.position;
    let singleton_span = arena.get(singleton.0).position;
    Some(SingletonReferenceDetail {
        first_blocker: format!(
            "{path}:{method_line}:{method_name} method_tree={} singleton_tree={} reference_tree={} reference_shape={reference_shape} reference_span={reference_span:?} singleton_span={singleton_span:?} declaration={declaration} snippet={snippet:?}",
            method.index(),
            singleton.0.index(),
            reference.index()
        ),
        singleton_tree: format!(
            "{path} tree={} reference_tree={} reference_shape={reference_shape} span={singleton_span:?}",
            singleton.0.index(),
            reference.index()
        ),
        enclosing_declaration: format!("{path} {declaration_shape} snippet={snippet:?}"),
        reference_shape,
    })
}

fn singleton_reference_fallback_detail(
    path: &str,
    method: dotty_core::TreeId<Untyped>,
    reference_index: Option<u32>,
) -> SingletonReferenceDetail {
    let reason = match reference_index {
        Some(index) => format!("details unavailable for reference_tree={index}"),
        None => "singleton reference tree index unavailable".to_owned(),
    };
    SingletonReferenceDetail {
        first_blocker: format!("{path}:fallback method_tree={} {reason}", method.index()),
        singleton_tree: format!("{path} fallback method_tree={} {reason}", method.index()),
        enclosing_declaration: format!("{path} fallback declaration ({reason})"),
        reference_shape: format!("Fallback({reason})"),
    }
}

fn singleton_reference_shape(kind: &TreeKind<Untyped>, names: &dotty_core::NameInterner) -> String {
    match kind {
        TreeKind::Literal(literal) => match &literal.value {
            dotty_core::Constant::Unit => "Literal(Unit)".to_owned(),
            dotty_core::Constant::Null => "Literal(Null)".to_owned(),
            dotty_core::Constant::Boolean(value) => format!("Literal(Boolean({value}))"),
            dotty_core::Constant::Byte(value) => format!("Literal(Byte({value}))"),
            dotty_core::Constant::Short(value) => format!("Literal(Short({value}))"),
            dotty_core::Constant::Char(value) => format!("Literal(Char({value}))"),
            dotty_core::Constant::Int(value) => format!("Literal(Int({value}))"),
            dotty_core::Constant::Long(value) => format!("Literal(Long({value}))"),
            dotty_core::Constant::FloatBits(value) => format!("Literal(FloatBits({value}))"),
            dotty_core::Constant::DoubleBits(value) => format!("Literal(DoubleBits({value}))"),
            dotty_core::Constant::String(value) => {
                format!("Literal(String({:?}))", names.resolve(*value))
            }
            dotty_core::Constant::StringUtf16(value) => {
                format!("Literal(StringUtf16({value:?}))")
            }
            dotty_core::Constant::Class(value) => format!("Literal(Class({value:?}))"),
        },
        TreeKind::Ident(_) => "Ident".to_owned(),
        TreeKind::Select(_) => "Select".to_owned(),
        TreeKind::This(_) => "This".to_owned(),
        TreeKind::PhaseSpecific(UntypedNode::Parens(_)) => "Parens".to_owned(),
        kind => format!("Other({})", tree_kind_label(kind)),
    }
}

fn singleton_reference_category(kind: &TreeKind<Untyped>) -> &'static str {
    match kind {
        TreeKind::Literal(_) => "literal",
        TreeKind::This(_) => "this",
        TreeKind::Ident(_) => "identifier",
        TreeKind::Select(_) => "selection",
        _ => "unsupported",
    }
}

fn singleton_reference_inventory_shape(kind: &TreeKind<Untyped>) -> String {
    match kind {
        TreeKind::Literal(literal) => {
            let constant = match literal.value {
                dotty_core::Constant::Unit => "Unit",
                dotty_core::Constant::Null => "Null",
                dotty_core::Constant::Boolean(_) => "Boolean",
                dotty_core::Constant::Byte(_) => "Byte",
                dotty_core::Constant::Short(_) => "Short",
                dotty_core::Constant::Char(_) => "Char",
                dotty_core::Constant::Int(_) => "Int",
                dotty_core::Constant::Long(_) => "Long",
                dotty_core::Constant::FloatBits(_) => "Float",
                dotty_core::Constant::DoubleBits(_) => "Double",
                dotty_core::Constant::String(_) => "String",
                dotty_core::Constant::StringUtf16(_) => "StringUtf16",
                dotty_core::Constant::Class(_) => "Class",
            };
            format!("Literal({constant})")
        }
        TreeKind::This(_) => "This".to_owned(),
        TreeKind::Ident(_) => "Ident".to_owned(),
        TreeKind::Select(_) => "Select".to_owned(),
        TreeKind::PhaseSpecific(UntypedNode::Parens(_)) => "Parens".to_owned(),
        kind => format!("Other({})", tree_kind_label(kind)),
    }
}

fn collect_singleton_source_inventory(
    arena: &dotty_core::AstArena<Untyped>,
    path: &str,
) -> SingletonSourceInventory {
    let mut inventory = SingletonSourceInventory::default();
    for (_, node) in arena.iter() {
        let TreeKind::SingletonTypeTree(singleton) = &node.kind else {
            continue;
        };
        let (category, shape) = match arena.try_get(singleton.reference) {
            Some(reference) => (
                singleton_reference_category(&reference.kind).to_owned(),
                singleton_reference_inventory_shape(&reference.kind),
            ),
            None => ("unsupported".to_owned(), "MissingReference".to_owned()),
        };
        inventory.record(category, shape, path);
    }
    inventory
}

fn local_method_signature_record(
    arena: &dotty_core::AstArena<Untyped>,
    names: &dotty_core::names::NameInterner,
    source_text: &str,
    path: &str,
    method_tree: dotty_core::TreeId<Untyped>,
    feature: &str,
) -> Option<(String, String)> {
    let node = arena.try_get(method_tree)?;
    let TreeKind::DefDef(definition) = &node.kind else {
        return None;
    };
    let position = node.position?;
    let range = position.span().range();
    let start = range.start() as usize;
    let end = range.end() as usize;
    let line = source_text
        .get(..start)?
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count()
        + 1;
    let method_name = names.resolve(definition.name.as_name().text());
    let method_key = format!("{path}#tree={}", method_tree.index());
    let type_parameter_clauses = definition
        .source_param_clause_order
        .as_ref()
        .map(|clauses| {
            clauses
                .iter()
                .filter(|clause| {
                    matches!(clause, dotty_core::ast::DefParamClauseOrder::TypeParams(_))
                })
                .count()
        })
        .unwrap_or(usize::from(!definition.type_params.is_empty()));
    let mut parameter_names = Vec::new();
    let mut parameter_modifiers = BTreeSet::new();
    let mut by_name_parameter = false;
    let mut clause_kinds = Vec::new();
    for clause in &definition.value_param_clauses {
        let mut clause_kind = BTreeSet::new();
        for parameter_tree in clause {
            let Some(TreeKind::ValDef(parameter)) =
                arena.try_get(*parameter_tree).map(|node| &node.kind)
            else {
                continue;
            };
            parameter_names.push(*parameter.name.as_name());
            for modifier in &parameter.metadata.modifiers {
                if *modifier != dotty_core::ast::Modifier::Param {
                    parameter_modifiers.insert(format!("{modifier:?}"));
                }
                match modifier {
                    dotty_core::ast::Modifier::Given => {
                        clause_kind.insert("contextual");
                    }
                    dotty_core::ast::Modifier::Implicit => {
                        clause_kind.insert("implicit");
                    }
                    _ => {}
                }
            }
            if matches!(
                arena.try_get(parameter.tpt).map(|node| &node.kind),
                Some(TreeKind::ByNameTypeTree(_))
            ) {
                by_name_parameter = true;
            }
        }
        clause_kinds.push(match clause_kind.len() {
            0 if clause.is_empty() => "empty",
            0 => "plain",
            1 => *clause_kind.first().unwrap(),
            _ => "mixed",
        });
    }
    let is_extension = is_extension_method_tree(arena, method_tree.index());
    if is_extension {
        for (_, extension_node) in arena.iter() {
            let TreeKind::PhaseSpecific(UntypedNode::ExtensionMethods(extension)) =
                &extension_node.kind
            else {
                continue;
            };
            if !extension.methods.contains(&method_tree) {
                continue;
            }
            for receiver in extension.param_clauses.iter().flatten() {
                if let Some(TreeKind::ValDef(parameter)) =
                    arena.try_get(*receiver).map(|node| &node.kind)
                {
                    parameter_names.push(*parameter.name.as_name());
                }
            }
        }
    }
    let result_depends_on_parameter =
        local_result_depends_on_parameters(arena, definition.tpt, &parameter_names);
    let result_kind = if matches!(
        arena.try_get(definition.tpt).map(|node| &node.kind),
        Some(TreeKind::TypeTree(_))
    ) {
        "inferred"
    } else {
        "explicit"
    };
    let record = format!(
        "{path}:line={line} span={start}..{end} tree={} method={method_name} feature={feature:?} type_parameter_clauses={type_parameter_clauses} value_parameter_clauses={} clause_kinds=[{}] parameter_modifiers=[{}] by_name_parameter={by_name_parameter} result_depends_on_parameter={result_depends_on_parameter} extension={is_extension} result={result_kind}",
        method_tree.index(),
        definition.value_param_clauses.len(),
        clause_kinds.join(","),
        parameter_modifiers
            .into_iter()
            .collect::<Vec<_>>()
            .join(",")
    );
    Some((method_key, record))
}

fn local_result_depends_on_parameters(
    arena: &dotty_core::AstArena<Untyped>,
    result_tree: dotty_core::TreeId<Untyped>,
    parameter_names: &[dotty_core::Name],
) -> bool {
    let mut pending = vec![result_tree];
    let mut visited = HashSet::new();
    while let Some(tree) = pending.pop() {
        if !visited.insert(tree) {
            continue;
        }
        let Some(node) = arena.try_get(tree) else {
            continue;
        };
        match &node.kind {
            TreeKind::SingletonTypeTree(singleton) => {
                if local_parameter_path(arena, singleton.reference, parameter_names) {
                    return true;
                }
            }
            TreeKind::Select(selection) => {
                if local_parameter_path(arena, selection.qualifier, parameter_names) {
                    return true;
                }
            }
            TreeKind::AppliedTypeTree(applied) => {
                pending.push(applied.tpt);
                pending.extend(applied.args.iter().copied());
            }
            TreeKind::ByNameTypeTree(by_name) => pending.push(by_name.result),
            TreeKind::RefinedTypeTree(refined) => {
                pending.push(refined.tpt);
                pending.extend(refined.refinements.iter().copied());
            }
            TreeKind::PhaseSpecific(UntypedNode::Parens(parens)) => pending.push(parens.inner),
            _ => {}
        }
    }
    false
}

fn local_parameter_path(
    arena: &dotty_core::AstArena<Untyped>,
    mut tree: dotty_core::TreeId<Untyped>,
    parameter_names: &[dotty_core::Name],
) -> bool {
    let mut visited = HashSet::new();
    loop {
        if !visited.insert(tree) {
            return false;
        }
        let Some(node) = arena.try_get(tree) else {
            return false;
        };
        match &node.kind {
            TreeKind::Ident(ident) => return parameter_names.contains(&ident.name),
            TreeKind::Select(selection) => tree = selection.qualifier,
            TreeKind::SingletonTypeTree(singleton) => tree = singleton.reference,
            TreeKind::PhaseSpecific(UntypedNode::Parens(parens)) => tree = parens.inner,
            _ => return false,
        }
    }
}

#[test]
fn singleton_profile_shapes_are_distinct_and_deterministically_ordered() {
    let source_text = "class Inner { val field: Int = 0 }; class C { val truth: true = true; val numeric: 1 = 1; val stable: Int = 1; val alias: stable.type = stable; val inner: Inner = new Inner; val selected: inner.field.type = inner.field }";
    let source = SourceId::from_index(0);
    let mut store = SemanticStore::new();
    let scanner = ContextualScanner::new(source_text).unwrap();
    let parsed = parse_compilation_unit(
        SourceText::new(source_text).unwrap(),
        source,
        scanner,
        &mut store.names,
    );
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);

    let mut shapes = parsed
        .ast
        .iter()
        .filter_map(|(_, node)| match &node.kind {
            TreeKind::SingletonTypeTree(singleton) => Some(singleton_reference_shape(
                &parsed.ast.get(singleton.reference).kind,
                &store.names,
            )),
            _ => None,
        })
        .collect::<Vec<_>>();
    shapes.sort();
    assert_eq!(
        shapes,
        [
            "Ident",
            "Literal(Boolean(true))",
            "Literal(Int(1))",
            "Select",
        ]
    );
    assert_eq!(
        singleton_reference_shape(&TreeKind::TypeTree(Default::default()), &store.names),
        "Other(TypeTree)"
    );

    let method_tree = parsed.ast.iter().next().unwrap().0;
    let mut first = SingletonReferenceProfile::default();
    for id in [2, 1] {
        first.record(SingletonReferenceDetail {
            first_blocker: format!("method={id}"),
            singleton_tree: "source-tree=7".to_owned(),
            enclosing_declaration: "declaration=3".to_owned(),
            reference_shape: "Literal(Boolean(true))".to_owned(),
        });
    }
    first.record(singleton_reference_fallback_detail(
        "fixture.scala",
        method_tree,
        None,
    ));
    let records = first.first_blockers.iter().cloned().collect::<Vec<_>>();
    assert_eq!(records.len(), 3);
    assert!(records[0].contains(&format!(
        "fixture.scala:fallback method_tree={}",
        method_tree.index()
    )));
    assert_eq!(first.observations, 3);
    assert_eq!(first.singleton_source_trees.len(), 2);
    assert_eq!(first.enclosing_declarations.len(), 2);
    assert_eq!(
        first.reference_shapes,
        [
            "Fallback(singleton reference tree index unavailable)".to_owned(),
            "Literal(Boolean(true))".to_owned()
        ]
        .into_iter()
        .collect()
    );
}

#[test]
fn singleton_source_inventory_counts_reference_categories_and_files() {
    let source_text = "class Inner { val value: Int = 1 }; class C { val literal: true = true; val self: this.type = this; val stable: Int = 1; val identifier: stable.type = stable; val child: Inner = new Inner; val selected: child.value.type = child.value }";
    let source = SourceId::from_index(0);
    let mut store = SemanticStore::new();
    let scanner = ContextualScanner::new(source_text).unwrap();
    let mut parsed = parse_compilation_unit(
        SourceText::new(source_text).unwrap(),
        source,
        scanner,
        &mut store.names,
    );
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);

    let unsupported_reference = parsed
        .ast
        .iter()
        .find_map(|(_, node)| match &node.kind {
            TreeKind::SingletonTypeTree(singleton) => {
                matches!(parsed.ast.get(singleton.reference).kind, TreeKind::Ident(_))
                    .then_some(singleton.reference)
            }
            _ => None,
        })
        .unwrap();
    parsed.ast.get_mut(unsupported_reference).kind = TreeKind::TypeTree(Default::default());

    let first = collect_singleton_source_inventory(&parsed.ast, "fixture.scala");
    let second = collect_singleton_source_inventory(&parsed.ast, "fixture.scala");

    assert_eq!(first, second);
    assert_eq!(first.total, 4);
    assert_eq!(first.categories.get("literal"), Some(&1));
    assert_eq!(first.categories.get("this"), Some(&1));
    assert_eq!(first.categories.get("selection"), Some(&1));
    assert_eq!(first.categories.get("unsupported"), Some(&1));
    assert_eq!(first.categories.get("identifier"), None);
    assert_eq!(
        first.category_files.get("unsupported"),
        Some(&BTreeSet::from(["fixture.scala".to_owned()]))
    );
    assert_eq!(first.reference_shapes.get("Literal(Boolean)"), Some(&1));
    assert_eq!(first.reference_shapes.get("This"), Some(&1));
    assert_eq!(first.reference_shapes.get("Select"), Some(&1));
    assert_eq!(first.reference_shapes.get("Other(TypeTree)"), Some(&1));
}

#[test]
fn local_method_signature_profile_classifies_feature_fixtures() {
    let by_name_source =
        "class C { def outer: Int = { def byName(value: => Int): Int = value; 0 } }";
    let by_name = audit_source_inner(by_name_source, "ByName.scala", None);
    let repeated_by_name = audit_source_inner(by_name_source, "ByName.scala", None);
    assert_eq!(
        by_name.local_method_signature_profile, repeated_by_name.local_method_signature_profile,
        "local method signature profile should be deterministic across repeated audits"
    );
    assert!(by_name.local_method_signature_profile.features.is_empty());

    let dependent = audit_source_inner(
        "class C { def outer: Int = { def dependent(value: Int): value.type = value; 0 } }",
        "DependentResult.scala",
        None,
    );
    let dependent_feature = dependent
        .local_method_signature_profile
        .features
        .get("dependent result types")
        .expect("dependent result fixture should be distinct from by-name parameters");
    assert_eq!(dependent_feature.count, 1);
    assert!(
        dependent_feature
            .records
            .iter()
            .any(|record| record.contains("result_depends_on_parameter=true"))
    );

    let inline_parameter = audit_source_inner(
        "class C { def outer: Int = { inline def modified(inline value: Int): Int = value; 0 } }",
        "InlineParameter.scala",
        None,
    );
    assert!(
        inline_parameter
            .local_method_signature_profile
            .features
            .is_empty()
    );

    let supported = audit_source_inner(
        "class C { def outer: Int = { def supported(value: Int): Int = value; supported(1) } }",
        "SupportedLocalMethod.scala",
        None,
    );
    assert!(supported.local_method_signature_profile.features.is_empty());

    let extension_source = "class C { def outer: Int = { extension (receiver: Int) { def generic[A]: Int = receiver }; 0 } }";
    let extension_source_id = SourceId::from_index(0);
    let mut extension_store = SemanticStore::new();
    let extension_parsed = parse_compilation_unit(
        SourceText::new(extension_source).unwrap(),
        extension_source_id,
        ContextualScanner::new(extension_source).unwrap(),
        &mut extension_store.names,
    );
    let extension_method = extension_parsed
        .ast
        .iter()
        .find_map(|(tree, node)| {
            matches!(&node.kind, TreeKind::DefDef(definition)
                if extension_store.names.resolve(definition.name.as_name().text()) == "generic")
            .then_some(tree)
        })
        .expect("extension method fixture should parse");
    assert!(is_extension_method_tree(
        &extension_parsed.ast,
        extension_method.index()
    ));
    let extension_classification = local_method_signature_classification(
        &extension_parsed.ast,
        extension_method.index(),
        "extension type parameters",
    );
    assert_eq!(
        extension_classification.bucket,
        "LocalExtensionSignatureDeferred::extension type parameters"
    );

    let by_name_call = audit_source_inner(
        "class C { def accept(value: => Int): Int = value; def outer: Int = { def call: Int = accept(1); call } }",
        "ByNameCall.scala",
        None,
    );
    assert!(
        !by_name_call
            .failures
            .contains_key("ByNameApplicationParameterDeferred"),
        "a local by-name call should pass application typing"
    );
    assert_eq!(by_name_call.typed_local_defdefs, 1);
}

#[derive(Debug)]
struct MissingDeclaredTypeDetail {
    bucket: String,
    record: String,
    tree_index: u32,
}

fn missing_declared_type_detail(
    arena: &dotty_core::AstArena<Untyped>,
    index: &dotty_core::SourceSemanticIndex,
    store: &SemanticStore,
    source: SourceId,
    type_tree_index: u32,
) -> MissingDeclaredTypeDetail {
    let tree = arena
        .iter()
        .find_map(|(tree, node)| (tree.index() == type_tree_index).then_some(node));
    let tree_shape = tree.map_or("unknown", |tree| match tree.kind {
        TreeKind::TypeTree(_)
            if tree.position.is_none_or(|position| {
                let range = position.span().range();
                range.start() == range.end()
            }) =>
        {
            "synthetic inferred TypeTree"
        }
        TreeKind::TypeTree(_) => "explicit or positioned TypeTree",
        _ => "non-TypeTree error target",
    });
    let error_tree_kind = tree.map_or("unknown", |tree| match &tree.kind {
        TreeKind::TypeTree(_) => "TypeTree",
        TreeKind::Ident(_) => "Ident",
        TreeKind::PhaseSpecific(_) => "PhaseSpecific",
        _ => "other",
    });
    let declaration = arena
        .iter()
        .find_map(|(declaration_tree, node)| match &node.kind {
            TreeKind::ValDef(definition) if definition.tpt.index() == type_tree_index => {
                let has_param = definition
                    .metadata
                    .modifiers
                    .contains(&dotty_core::ast::Modifier::Param);
                let has_accessor = definition
                    .metadata
                    .modifiers
                    .contains(&dotty_core::ast::Modifier::ParamAccessor);
                let kind = if has_param || has_accessor {
                    "parameter/accessor"
                } else if definition
                    .metadata
                    .modifiers
                    .contains(&dotty_core::ast::Modifier::Var)
                {
                    "variable"
                } else {
                    "value"
                };
                Some((
                    declaration_tree,
                    kind,
                    definition.rhs.is_some(),
                    definition.metadata.modifiers.clone(),
                ))
            }
            TreeKind::DefDef(definition) if definition.tpt.index() == type_tree_index => Some((
                declaration_tree,
                "method result",
                definition.rhs.is_some(),
                definition.metadata.modifiers.clone(),
            )),
            _ => None,
        });
    let (decl_tree, declaration_kind, rhs_present, modifiers) = declaration
        .map(|(tree, kind, rhs, modifiers)| (Some(tree.index()), kind, rhs, modifiers))
        .unwrap_or((None, "unknown declaration", false, Default::default()));
    let declaration_symbol = decl_tree.and_then(|tree_index| {
        arena
            .iter()
            .find_map(|(tree, _)| (tree.index() == tree_index).then_some(tree))
            .and_then(|tree| index.symbol_at(source, tree))
    });
    let symbol = declaration_symbol;
    let symbol_kind = symbol.map_or("unknown", |symbol| match store.symbols.get(symbol).kind {
        dotty_core::symbols::SymbolKind::Field => "Field",
        dotty_core::symbols::SymbolKind::Value => "Value",
        dotty_core::symbols::SymbolKind::Variable => "Variable",
        dotty_core::symbols::SymbolKind::Parameter => "Parameter",
        dotty_core::symbols::SymbolKind::Method => "Method",
        kind => match kind {
            dotty_core::symbols::SymbolKind::Class => "Class",
            dotty_core::symbols::SymbolKind::Trait => "Trait",
            dotty_core::symbols::SymbolKind::Object => "Object",
            dotty_core::symbols::SymbolKind::ModuleClass => "ModuleClass",
            dotty_core::symbols::SymbolKind::Constructor => "Constructor",
            dotty_core::symbols::SymbolKind::Package => "Package",
            dotty_core::symbols::SymbolKind::TypeParameter => "TypeParameter",
            dotty_core::symbols::SymbolKind::TypeAlias => "TypeAlias",
            dotty_core::symbols::SymbolKind::Local => "Local",
            _ => "other",
        },
    });
    let context_owner_kind = symbol
        .and_then(|symbol| index.declaration_context_of(symbol))
        .and_then(|context| index.try_source_context(context))
        .map(|context| store.symbols.get(context.owner).kind)
        .map(symbol_kind_label)
        .unwrap_or("unknown");
    let owner_kind = symbol
        .and_then(|symbol| store.symbols.get(symbol).owner)
        .map(|owner| symbol_kind_label(store.symbols.get(owner).kind))
        .unwrap_or(context_owner_kind);
    let modifier_labels = modifiers
        .iter()
        .map(|modifier| format!("{modifier:?}"))
        .collect::<Vec<_>>()
        .join(",");
    let semantic_mutable = symbol.is_some_and(|symbol| {
        store
            .symbols
            .get(symbol)
            .flags
            .contains(dotty_core::symbols::SymbolFlags::MUTABLE)
    });
    let entry_path = match declaration_kind {
        "value" | "variable" | "parameter/accessor" => {
            "complete_symbol_inner -> type_of_tpt_inner_journaled"
        }
        "method result" if owner_kind == "Method" => {
            "complete_symbol_inner -> complete_local_method_signature"
        }
        "method result" => "complete_symbol_inner -> complete_method_signature",
        _ => "unknown typer entry path",
    };
    let owner_name = owner_kind;
    MissingDeclaredTypeDetail {
        bucket: format!(
            "{declaration_kind}::{symbol_kind}::owner={owner_name}::{tree_shape}::rhs={rhs_present}::modifiers={modifier_labels}::semantic_mutable={semantic_mutable}"
        ),
        record: format!(
            "tree={type_tree_index} error_tree_kind={error_tree_kind} declaration_tree={} declaration_tree_kind={} declaration={declaration_kind} symbol_kind={symbol_kind} owner_kind={owner_name} context_owner_kind={context_owner_kind} shape={tree_shape} rhs={rhs_present} modifiers=[{modifier_labels}] semantic_mutable={semantic_mutable} entry={entry_path} span={:?}",
            decl_tree.map_or_else(|| "unknown".to_owned(), |tree| tree.to_string()),
            declaration_kind_tree(declaration_kind),
            tree.and_then(|tree| tree.position)
        ),
        tree_index: type_tree_index,
    }
}

fn declaration_kind_tree(kind: &str) -> &'static str {
    match kind {
        "value" | "variable" | "parameter/accessor" => "ValDef",
        "method result" => "DefDef",
        _ => "unknown",
    }
}

fn symbol_kind_label(kind: dotty_core::symbols::SymbolKind) -> &'static str {
    match kind {
        dotty_core::symbols::SymbolKind::Package => "Package",
        dotty_core::symbols::SymbolKind::Class => "Class",
        dotty_core::symbols::SymbolKind::Trait => "Trait",
        dotty_core::symbols::SymbolKind::Object => "Object",
        dotty_core::symbols::SymbolKind::ModuleClass => "ModuleClass",
        dotty_core::symbols::SymbolKind::Method => "Method",
        dotty_core::symbols::SymbolKind::Constructor => "Constructor",
        dotty_core::symbols::SymbolKind::Field => "Field",
        dotty_core::symbols::SymbolKind::Value => "Value",
        dotty_core::symbols::SymbolKind::Variable => "Variable",
        dotty_core::symbols::SymbolKind::Parameter => "Parameter",
        dotty_core::symbols::SymbolKind::TypeParameter => "TypeParameter",
        dotty_core::symbols::SymbolKind::TypeAlias => "TypeAlias",
        dotty_core::symbols::SymbolKind::Local => "Local",
    }
}

fn print_missing_declared_type_profile(profile: &MissingDeclaredTypeProfile) {
    println!("missing_declared_type_profile:");
    println!(
        "  first_blockers={} distinct_declarations={}",
        profile.records.len(),
        profile.declarations.len()
    );
    let mut buckets = profile.buckets.iter().collect::<Vec<_>>();
    buckets.sort_by(|(name_a, a), (name_b, b)| b.count.cmp(&a.count).then(name_a.cmp(name_b)));
    for (name, bucket) in buckets {
        println!(
            "  {name}: {} ({} files) [{}]",
            bucket.count,
            bucket.files.len(),
            bucket.files.iter().cloned().collect::<Vec<_>>().join(", ")
        );
    }
    println!("missing_declared_type_records:");
    for record in &profile.records {
        println!("  {record}");
    }
}

fn classify_typer_error(
    error: &TyperError,
    arena: &dotty_core::AstArena<Untyped>,
    operator_spellings: &BTreeMap<u32, String>,
    names: &dotty_core::names::NameInterner,
) -> FailureClassification {
    match error {
        TyperError::LocalBlockDeclarationDeferred {
            kind: "extension methods",
            ..
        } => FailureClassification {
            bucket: "LocalExtensionGroupShapeDeferred".to_owned(),
            family: FailureFamily::LocalDeclarationDeferral,
        },
        TyperError::LocalMethodSignatureDeferred {
            tree_index,
            feature,
            ..
        } => local_method_signature_classification(arena, *tree_index, feature),
        TyperError::TypeNameNotFound { tree_index, .. }
            if is_extension_receiver_tree(arena, *tree_index) =>
        {
            FailureClassification {
                bucket: "LocalExtensionReceiverTypeNotFound".to_owned(),
                family: FailureFamily::ResolutionClasspathEnvironment,
            }
        }
        TyperError::MemberNotFound {
            tree_index, name, ..
        } if has_lexical_extension_candidate(arena, *tree_index, *name, names) => {
            FailureClassification {
                bucket: "LocalExtensionNotApplicable".to_owned(),
                family: FailureFamily::LocalDeclarationDeferral,
            }
        }
        TyperError::OverloadedSelectionDeferred {
            tree_index, name, ..
        } if has_lexical_extension_candidate(arena, *tree_index, *name, names) => {
            FailureClassification {
                bucket: "LocalExtensionAmbiguityDeferred".to_owned(),
                family: FailureFamily::LocalDeclarationDeferral,
            }
        }
        TyperError::ApplicationArgumentConformanceUnsupported { tree_index, .. }
            if selected_name(arena, *tree_index).is_some_and(|name| {
                has_lexical_extension_candidate(arena, *tree_index, name, names)
            }) =>
        {
            FailureClassification {
                bucket: "LocalExtensionConformanceUnsupported".to_owned(),
                family: FailureFamily::TypeRelationInferenceCompletion,
            }
        }
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
        TyperError::LocalValueModifierDeferred {
            tree_index,
            modifiers,
            classification,
            ..
        } => {
            let node = arena
                .iter()
                .find(|(tree, _)| tree.index() == *tree_index)
                .map(|(_, node)| &node.kind);
            let bucket = if matches!(node, Some(TreeKind::PhaseSpecific(UntypedNode::PatDef(_)))) {
                if modifiers.contains(&dotty_core::ast::Modifier::Lazy) {
                    "LocalPatDefDeferred::lazy"
                } else {
                    "LocalPatDefDeferred::modifiers"
                }
            } else if *classification == "declaration semantics deferred" {
                "LocalBlockDeclarationDeferred::val/var definition"
            } else {
                return FailureClassification {
                    bucket: format!("LocalValueModifierDeferred::{classification}"),
                    family: FailureFamily::LocalDeclarationDeferral,
                };
            };
            FailureClassification {
                bucket: bucket.to_owned(),
                family: FailureFamily::LocalDeclarationDeferral,
            }
        }
        TyperError::LocalPatDefDeferred { kind, .. } => FailureClassification {
            bucket: format!("LocalPatDefDeferred::{kind}"),
            family: FailureFamily::LocalDeclarationDeferral,
        },
        TyperError::UnsupportedTypeTree { tree_index, .. } => {
            let kind = arena
                .iter()
                .find(|(tree, _)| tree.index() == *tree_index)
                .map(|(_, node)| &node.kind);
            let shape = kind.map_or("<other>", |kind| {
                type_tree_shape_label(kind, *tree_index, operator_spellings)
            });
            FailureClassification {
                bucket: format!("UnsupportedTypeTree::{shape}"),
                family: FailureFamily::Other,
            }
        }
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

fn local_method_signature_classification(
    arena: &dotty_core::AstArena<Untyped>,
    tree_index: u32,
    feature: &str,
) -> FailureClassification {
    let bucket = if is_extension_method_tree(arena, tree_index) {
        format!("LocalExtensionSignatureDeferred::{feature}")
    } else {
        "LocalMethodSignatureDeferred".to_owned()
    };
    FailureClassification {
        bucket,
        family: FailureFamily::TypeRelationInferenceCompletion,
    }
}

fn is_extension_method_tree(arena: &dotty_core::AstArena<Untyped>, tree_index: u32) -> bool {
    arena.iter().any(|(_, node)| match &node.kind {
        TreeKind::PhaseSpecific(UntypedNode::ExtensionMethods(extension)) => extension
            .methods
            .iter()
            .any(|method| method.index() == tree_index),
        _ => false,
    })
}

fn is_extension_receiver_tree(arena: &dotty_core::AstArena<Untyped>, tree_index: u32) -> bool {
    arena.iter().any(|(_, node)| {
        let TreeKind::PhaseSpecific(UntypedNode::ExtensionMethods(extension)) = &node.kind else {
            return false;
        };
        extension.param_clauses.iter().flatten().any(|parameter| {
            parameter.index() == tree_index
                || matches!(
                    arena.try_get(*parameter).map(|node| &node.kind),
                    Some(TreeKind::ValDef(definition))
                        if type_tree_contains(arena, definition.tpt, tree_index)
                )
        })
    })
}

fn type_tree_contains(
    arena: &dotty_core::AstArena<Untyped>,
    root: dotty_core::TreeId<Untyped>,
    target_index: u32,
) -> bool {
    let mut pending = vec![root];
    let mut visited = BTreeSet::new();
    while let Some(tree) = pending.pop() {
        if tree.index() == target_index {
            return true;
        }
        if !visited.insert(tree.index()) {
            continue;
        }
        if let Some(node) = arena.try_get(tree) {
            pending.extend(type_tree_children(&node.kind));
        }
    }
    false
}

fn selected_name(
    arena: &dotty_core::AstArena<Untyped>,
    tree_index: u32,
) -> Option<dotty_core::Name> {
    let tree = arena
        .iter()
        .find_map(|(tree, _)| (tree.index() == tree_index).then_some(tree))?;
    let function = match &arena.get(tree).kind {
        TreeKind::Apply(application) => application.function,
        TreeKind::Select(_) => tree,
        _ => return None,
    };
    match &arena.try_get(function)?.kind {
        TreeKind::Select(selection) => Some(selection.name),
        _ => None,
    }
}

fn has_lexical_extension_candidate(
    arena: &dotty_core::AstArena<Untyped>,
    tree_index: u32,
    name: dotty_core::Name,
    names: &dotty_core::names::NameInterner,
) -> bool {
    let Some(selection_tree) = arena
        .iter()
        .find_map(|(tree, _)| (tree.index() == tree_index).then_some(tree))
    else {
        return false;
    };
    let selection_tree = match &arena.get(selection_tree).kind {
        TreeKind::Apply(application) => application.function,
        TreeKind::Select(_) => selection_tree,
        _ => return false,
    };
    let Some(selection_node) = arena.try_get(selection_tree) else {
        return false;
    };
    let TreeKind::Select(selection) = &selection_node.kind else {
        return false;
    };
    if selection.name != name {
        return false;
    }
    let Some(selection_range) = selection_node
        .position
        .map(|position| position.span().range())
    else {
        return false;
    };

    let mut containing_blocks = arena
        .iter()
        .filter_map(|(_, node)| {
            let TreeKind::Block(block) = &node.kind else {
                return None;
            };
            let range = node.position?.span().range();
            (range.start() <= selection_range.start() && selection_range.end() <= range.end())
                .then(|| (range.end() - range.start(), block.stats.clone()))
        })
        .collect::<Vec<_>>();
    containing_blocks.sort_by_key(|(length, _)| *length);

    for (_, statements) in containing_blocks {
        let mut has_extension = false;
        let mut has_ordinary_method = false;
        let mut has_shadowing_value = false;
        for statement in statements {
            match arena.try_get(statement).map(|node| &node.kind) {
                Some(TreeKind::PhaseSpecific(UntypedNode::ExtensionMethods(extension))) => {
                    has_extension |= is_preindexable_extension(arena, extension, name, names);
                }
                Some(TreeKind::DefDef(definition)) => {
                    has_ordinary_method |= *definition.name.as_name() == name;
                }
                Some(TreeKind::ValDef(definition)) if *definition.name.as_name() == name => {
                    has_shadowing_value |=
                        statement_precedes_selection(arena, statement, selection_range.start());
                }
                Some(TreeKind::PhaseSpecific(UntypedNode::PatDef(definition))) => {
                    has_shadowing_value |=
                        statement_precedes_selection(arena, statement, selection_range.start())
                            && definition
                                .patterns
                                .iter()
                                .any(|pattern| pattern_binds_name(arena, *pattern, name, names));
                }
                Some(TreeKind::PhaseSpecific(UntypedNode::ModuleDef(definition)))
                    if *definition.name.as_name() == name =>
                {
                    has_shadowing_value |=
                        statement_precedes_selection(arena, statement, selection_range.start());
                }
                _ => {}
            }
        }
        if has_extension {
            return true;
        }
        if has_ordinary_method || has_shadowing_value {
            return false;
        }
    }
    false
}

fn is_preindexable_extension(
    arena: &dotty_core::AstArena<Untyped>,
    extension: &dotty_core::ast::ExtensionMethods,
    name: dotty_core::Name,
    names: &dotty_core::names::NameInterner,
) -> bool {
    if extension.methods.len() != 1
        || extension.param_clauses.len() != 1
        || extension.param_clauses[0].len() != 1
    {
        return false;
    }
    let parameter = extension.param_clauses[0][0];
    let supported_receiver = matches!(
        arena.try_get(parameter).map(|node| &node.kind),
        Some(TreeKind::ValDef(definition))
            if !definition.metadata.modifiers.iter().any(|modifier| {
                matches!(modifier, dotty_core::ast::Modifier::Given | dotty_core::ast::Modifier::Implicit)
            })
    );
    if !supported_receiver {
        return false;
    }
    matches!(
        arena
            .try_get(extension.methods[0])
            .map(|node| &node.kind),
        Some(TreeKind::DefDef(definition))
            if *definition.name.as_name() == name
                && definition.type_params.is_empty()
                && definition.rhs.is_some()
                && !names.resolve(definition.name.as_name().text()).ends_with(':')
    )
}

fn statement_precedes_selection(
    arena: &dotty_core::AstArena<Untyped>,
    statement: dotty_core::TreeId<Untyped>,
    selection_start: u32,
) -> bool {
    arena
        .try_get(statement)
        .and_then(|node| node.position)
        .is_some_and(|position| position.span().range().start() < selection_start)
}

fn pattern_binds_name(
    arena: &dotty_core::AstArena<Untyped>,
    pattern: dotty_core::TreeId<Untyped>,
    name: dotty_core::Name,
    names: &dotty_core::names::NameInterner,
) -> bool {
    let mut pending = vec![pattern];
    while let Some(tree) = pending.pop() {
        let Some(node) = arena.try_get(tree) else {
            continue;
        };
        match &node.kind {
            TreeKind::Ident(ident)
                if !ident.backquoted
                    && ident.name == name
                    && names
                        .resolve(ident.name.text())
                        .chars()
                        .next()
                        .is_some_and(char::is_lowercase) =>
            {
                return true;
            }
            TreeKind::Bind(binding) => {
                if binding.name == name && names.resolve(binding.name.text()) != "_" {
                    return true;
                }
                pending.push(binding.body);
            }
            TreeKind::NamedArg(argument) => pending.push(argument.arg),
            TreeKind::Typed(typed) => pending.push(typed.expr),
            TreeKind::Apply(application) => {
                pending.extend(application.args.iter().rev().copied());
            }
            TreeKind::Alternative(alternative) => {
                if let Some(first) = alternative.alternatives.first() {
                    pending.push(*first);
                }
            }
            TreeKind::PhaseSpecific(UntypedNode::Parens(parens)) => pending.push(parens.inner),
            TreeKind::PhaseSpecific(UntypedNode::Tuple(tuple)) => {
                pending.extend(tuple.elements.iter().rev().copied());
            }
            TreeKind::PhaseSpecific(UntypedNode::InfixOp(infix)) => {
                pending.push(infix.right);
                pending.push(infix.left);
            }
            TreeKind::UnApply(unapply) => {
                pending.extend(unapply.patterns.iter().rev().copied());
            }
            _ => {}
        }
    }
    false
}

fn source_operator_spellings(
    arena: &dotty_core::AstArena<Untyped>,
    names: &dotty_core::names::NameInterner,
) -> BTreeMap<u32, String> {
    arena
        .iter()
        .filter_map(|(tree, node)| {
            let operator = match &node.kind {
                TreeKind::PhaseSpecific(UntypedNode::InfixOp(infix)) => Some(infix.op),
                TreeKind::PhaseSpecific(UntypedNode::PostfixOp(postfix)) => Some(postfix.op),
                _ => None,
            }?;
            Some((tree.index(), names.resolve(operator.text()).to_owned()))
        })
        .collect()
}

fn type_tree_shape_label(
    kind: &TreeKind<Untyped>,
    tree_index: u32,
    operator_spellings: &BTreeMap<u32, String>,
) -> &'static str {
    match kind {
        TreeKind::PhaseSpecific(UntypedNode::InfixOp(_)) => {
            match operator_spellings.get(&tree_index).map(String::as_str) {
                Some("|") => "InfixOp::|",
                Some("&") => "InfixOp::&",
                _ => "InfixOp::<other>",
            }
        }
        TreeKind::PhaseSpecific(UntypedNode::PostfixOp(_)) => {
            match operator_spellings.get(&tree_index).map(String::as_str) {
                Some("*") => "PostfixOp::*",
                _ => "PostfixOp::<other>",
            }
        }
        _ => tree_kind_label(kind),
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
        NamerError::DuplicateFieldInitializerContext { .. } => {
            "NamerError::DuplicateFieldInitializerContext"
        }
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

fn match_first_error_label(error: &TyperError, pattern_shapes: &BTreeMap<u32, String>) -> String {
    match error {
        TyperError::UnsupportedPattern { pattern_kind, .. } => {
            format!("UnsupportedPattern::{}", pattern_kind.as_str())
        }
        TyperError::UnsupportedBindPatternBody { .. } => {
            "UnsupportedPattern::binding body".to_owned()
        }
        TyperError::MalformedVariablePattern { .. }
        | TyperError::MalformedPatternBinding { .. }
        | TyperError::WildcardPatternBindingRejected { .. }
        | TyperError::PatternBindingOutsideCaseScope { .. }
        | TyperError::PatternBindingScopeConflict { .. }
        | TyperError::DuplicatePatternBinding { .. }
        | TyperError::PatternBindingInAlternative { .. } => {
            format!("pattern binding: {}", typer_error_name(error))
        }
        TyperError::PatternTypeMismatch { tree_index, .. }
        | TyperError::PatternTypeRelationDeferred { tree_index, .. } => {
            let shape = pattern_shapes
                .get(tree_index)
                .cloned()
                .unwrap_or_else(|| "unknown".to_owned());
            format!("{shape}: {}", typer_error_name(error))
        }
        TyperError::TypedPatternTypeMismatch { .. }
        | TyperError::TypedPatternRelationDeferred { .. }
        | TyperError::TypedPatternRuntimeTestDeferred { .. } => {
            format!("typed pattern: {}", typer_error_name(error))
        }
        _ => typer_error_name(error).to_owned(),
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
        TyperError::FieldInitializerContextInvalid { .. } => "FieldInitializerContextInvalid",
        TyperError::FieldMutabilityMismatch { .. } => "FieldMutabilityMismatch",
        TyperError::ExpressionLocalScopeMissing { .. } => "ExpressionLocalScopeMissing",
        TyperError::ExpressionLocalScopeStackMissing { .. } => "ExpressionLocalScopeStackMissing",
        TyperError::ExpressionLocalScopeStackForeign { .. } => "ExpressionLocalScopeStackForeign",
        TyperError::UnsupportedImportContext { .. } => "UnsupportedImportContext",
        TyperError::ImportQualifierNotFound { .. } => "ImportQualifierNotFound",
        TyperError::ImportQualifierNotStable { .. } => "ImportQualifierNotStable",
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
        TyperError::RecursiveInferredFieldType { .. } => "RecursiveInferredFieldType",
        TyperError::InvalidInferredFieldType { .. } => "InvalidInferredFieldType",
        TyperError::InferredFieldTypeDepthExceeded { .. } => "InferredFieldTypeDepthExceeded",
        TyperError::RecursiveInferredMethodResult { .. } => "RecursiveInferredMethodResult",
        TyperError::InferredMethodResultRightHandSideMissing { .. } => {
            "InferredMethodResultRightHandSideMissing"
        }
        TyperError::InvalidInferredMethodResult { .. } => "InvalidInferredMethodResult",
        TyperError::RightAssociativeExtensionDeferred { .. } => "RightAssociativeExtensionDeferred",
        TyperError::RightAssociativeInfixDeferred { .. } => "RightAssociativeInfixDeferred",
        TyperError::InfixOperatorMustBeTerm { .. } => "InfixOperatorMustBeTerm",
        TyperError::UnsupportedPrefixOperator { .. } => "UnsupportedPrefixOperator",
        TyperError::PrefixMethodNeedsArgumentList { .. } => "PrefixMethodNeedsArgumentList",
        TyperError::PrefixPolymorphicDeferred { .. } => "PrefixPolymorphicDeferred",
        TyperError::MalformedSourceAnnotation { .. } => "MalformedSourceAnnotation",
        TyperError::SourceAnnotationClassDeferred { .. } => "SourceAnnotationClassDeferred",
        TyperError::SourceAnnotationNotAnnotationClass { .. } => {
            "SourceAnnotationNotAnnotationClass"
        }
        TyperError::SourceAnnotationArgumentNotConstant { .. } => {
            "SourceAnnotationArgumentNotConstant"
        }
        TyperError::SourceAnnotationDuplicateNamedArgument { .. } => {
            "SourceAnnotationDuplicateNamedArgument"
        }
        TyperError::SourceAnnotationConstructorDeferred { .. } => {
            "SourceAnnotationConstructorDeferred"
        }
        TyperError::SourceAnnotationConstructorArgumentMismatch { .. } => {
            "SourceAnnotationConstructorArgumentMismatch"
        }
        TyperError::SourceAnnotationArgumentTypeDeferred { .. } => {
            "SourceAnnotationArgumentTypeDeferred"
        }
        TyperError::MethodParameterSymbolMissing { .. } => "MethodParameterSymbolMissing",
        TyperError::LocalMethodSignatureDeferred { .. } => "LocalMethodSignatureDeferred",
        TyperError::LocalMethodInferredResultDeferred { .. } => "LocalMethodInferredResultDeferred",
        TyperError::MethodTypeParameterSymbolMissing { .. } => "MethodTypeParameterSymbolMissing",
        TyperError::ExtensionPrefixClausesMissing { .. } => "ExtensionPrefixClausesMissing",
        TyperError::MalformedMethodClause { .. } => "MalformedMethodClause",
        TyperError::RepeatedParameterClauseUnsupported { .. } => {
            "RepeatedParameterClauseUnsupported"
        }
        TyperError::RepeatedParameterNotFinal { .. } => "RepeatedParameterNotFinal",
        TyperError::RepeatedParameterSignatureContextMissing { .. } => {
            "RepeatedParameterSignatureContextMissing"
        }
        TyperError::ConstructorOwnerNotClassLike { .. } => "ConstructorOwnerNotClassLike",
        TyperError::MalformedConstructorOwnerInfo { .. } => "MalformedConstructorOwnerInfo",
        TyperError::MalformedConstructorOwner { .. } => "MalformedConstructorOwner",
        TyperError::SecondaryConstructorTypeParametersUnsupported { .. } => {
            "SecondaryConstructorTypeParametersUnsupported"
        }
        TyperError::DeferredSymbolCompletion { .. } => "DeferredSymbolCompletion",
        TyperError::SymbolAlreadyErrored { .. } => "SymbolAlreadyErrored",
        TyperError::UnsupportedTypeTree { .. } => "UnsupportedTypeTree",
        TyperError::SourceTypeProjectionDepthExceeded { .. } => "SourceTypeProjectionDepthExceeded",
        TyperError::MissingDeclaredType { .. } => "MissingDeclaredType",
        TyperError::MalformedSourceAst { .. } => "MalformedSourceAst",
        TyperError::SymbolSourceKindMismatch { .. } => "SymbolSourceKindMismatch",
        TyperError::TreeOutsideArena { .. } => "TreeOutsideArena",
        TyperError::TypeNameNotFound { .. } => "TypeNameNotFound",
        TyperError::UnsupportedSourceFunctionArity { .. } => "UnsupportedSourceFunctionArity",
        TyperError::SourceFunctionClassNotFound { .. } => "SourceFunctionClassNotFound",
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
        TyperError::UnsupportedFunctionLiteralParameter { .. } => {
            "UnsupportedFunctionLiteralParameter"
        }
        TyperError::FunctionLiteralCaptureUnsupported { .. } => "FunctionLiteralCaptureUnsupported",
        TyperError::UnsupportedPattern { .. } => "UnsupportedPattern",
        TyperError::TuplePatternResolutionDeferred { .. } => "TuplePatternResolutionDeferred",
        TyperError::TuplePatternTypeMismatch { .. } => "TuplePatternTypeMismatch",
        TyperError::TuplePatternRelationDeferred { .. } => "TuplePatternRelationDeferred",
        TyperError::InfixPatternDeferred { .. } => "InfixPatternDeferred",
        TyperError::PatternBindingInAlternative { .. } => "PatternBindingInAlternative",
        TyperError::PatternAlternativeJoinUnsupported { .. } => "PatternAlternativeJoinUnsupported",
        TyperError::ExtractorQualifierNotFound { .. } => "ExtractorQualifierNotFound",
        TyperError::ExtractorQualifierMemberNotFound { .. } => "ExtractorQualifierMemberNotFound",
        TyperError::ExtractorQualifierMemberAmbiguous { .. } => "ExtractorQualifierMemberAmbiguous",
        TyperError::ExtractorQualifierShapeUnsupported { .. } => {
            "ExtractorQualifierShapeUnsupported"
        }
        TyperError::ExtractorQualifierNotValueLike { .. } => "ExtractorQualifierNotValueLike",
        TyperError::ExtractorQualifierNotStable { .. } => "ExtractorQualifierNotStable",
        TyperError::ExtractorUnapplyNotFound { .. } => "ExtractorUnapplyNotFound",
        TyperError::ExtractorUnapplyOverloaded { .. } => "ExtractorUnapplyOverloaded",
        TyperError::ExtractorUnapplyPolymorphic { .. } => "ExtractorUnapplyPolymorphic",
        TyperError::ExtractorUnapplyShapeUnsupported { .. } => "ExtractorUnapplyShapeUnsupported",
        TyperError::ExtractorPatternConstraintDeferred { .. } => {
            "ExtractorPatternConstraintDeferred"
        }
        TyperError::UnsupportedExtractorResultProtocol { .. } => {
            "UnsupportedExtractorResultProtocol"
        }
        TyperError::ExtractorPatternArityUnsupported { .. } => "ExtractorPatternArityUnsupported",
        TyperError::BooleanExtractorPatternArityUnsupported { .. } => {
            "BooleanExtractorPatternArityUnsupported"
        }
        TyperError::ExtractorResultMemberNotFound { .. } => "ExtractorResultMemberNotFound",
        TyperError::ExtractorResultMemberOverloaded { .. } => "ExtractorResultMemberOverloaded",
        TyperError::ExtractorResultMemberUnsupported { .. } => "ExtractorResultMemberUnsupported",
        TyperError::ExtractorProductSelectorCountMismatch { .. } => {
            "ExtractorProductSelectorCountMismatch"
        }
        TyperError::UnsupportedExtractorProductProtocol { .. } => {
            "UnsupportedExtractorProductProtocol"
        }
        TyperError::ExtractorPatternArgumentUnsupported { .. } => {
            "ExtractorPatternArgumentUnsupported"
        }
        TyperError::MalformedCaseDef { .. } => "MalformedCaseDef",
        TyperError::MalformedPatternBinding { .. } => "MalformedPatternBinding",
        TyperError::WildcardPatternBindingRejected { .. } => "WildcardPatternBindingRejected",
        TyperError::PatternBindingOutsideCaseScope { .. } => "PatternBindingOutsideCaseScope",
        TyperError::PatternBindingScopeConflict { .. } => "PatternBindingScopeConflict",
        TyperError::DuplicatePatternBinding { .. } => "DuplicatePatternBinding",
        TyperError::EmptyMatchCases { .. } => "EmptyMatchCases",
        TyperError::MatchSelectorTypeCannotBeAdapted { .. } => "MatchSelectorTypeCannotBeAdapted",
        TyperError::MatchCaseResultTypeCannotBeWidened { .. } => {
            "MatchCaseResultTypeCannotBeWidened"
        }
        TyperError::MatchCaseJoinUnsupported { .. } => "MatchCaseJoinUnsupported",
        TyperError::MalformedVariablePattern { .. } => "MalformedVariablePattern",
        TyperError::UnsupportedBindPatternBody { .. } => "UnsupportedBindPatternBody",
        TyperError::PatternTypeMismatch { .. } => "PatternTypeMismatch",
        TyperError::PatternTypeRelationDeferred { .. } => "PatternTypeRelationDeferred",
        TyperError::TypedPatternTypeMismatch { .. } => "TypedPatternTypeMismatch",
        TyperError::TypedPatternRelationDeferred { .. } => "TypedPatternRelationDeferred",
        TyperError::TypedPatternRuntimeTestDeferred { .. } => "TypedPatternRuntimeTestDeferred",
        TyperError::MalformedStablePatternTarget { .. } => "MalformedStablePatternTarget",
        TyperError::UnstablePatternValue { .. } => "UnstablePatternValue",
        TyperError::LocalBlockDeclarationDeferred { .. } => "LocalBlockDeclarationDeferred",
        TyperError::LocalValueModifierDeferred { .. } => "LocalValueModifierDeferred",
        TyperError::LocalPatDefDeferred { .. } => "LocalPatDefDeferred",
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
        TyperError::UnsupportedSingletonReference { .. } => "UnsupportedSingletonReference",
        TyperError::UnsupportedSingletonLiteralKind { .. } => "UnsupportedSingletonLiteralKind",
        TyperError::PatDefExpansionConflict { .. } => "PatDefExpansionConflict",
        TyperError::PatDefBinderInventoryConflict { .. } => "PatDefBinderInventoryConflict",
        TyperError::PatDefAggregateArityDeferred { .. } => "PatDefAggregateArityDeferred",
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
            | "RecursiveInferredFieldType"
            | "InvalidInferredFieldType"
            | "InferredFieldTypeDepthExceeded"
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
            UntypedNode::InlineIf(_) => "InlineIf",
            UntypedNode::InlineMatch(_) => "InlineMatch",
            UntypedNode::MacroTree(_) => "MacroTree",
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
        TreeKind::Annotated(_) => Some("Annotated"),
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
        TreeKind::PhaseSpecific(UntypedNode::InlineIf(_)) => Some("InlineIf"),
        TreeKind::PhaseSpecific(UntypedNode::InlineMatch(_)) => Some("InlineMatch"),
        TreeKind::PhaseSpecific(UntypedNode::MacroTree(_)) => Some("MacroTree"),
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
    "Annotated",
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

fn collect_prefix_operator_histogram(
    arena: &dotty_core::AstArena<Untyped>,
    names: &dotty_core::names::NameInterner,
) -> BTreeMap<String, usize> {
    let nodes = arena
        .iter()
        .map(|(tree, node)| (tree.index(), node))
        .collect::<BTreeMap<_, _>>();
    let mut histogram = BTreeMap::new();
    for (_, node) in arena.iter() {
        let TreeKind::DefDef(definition) = &node.kind else {
            continue;
        };
        let Some(rhs) = definition.rhs else {
            continue;
        };
        let mut visited = HashSet::new();
        let mut pending = VecDeque::from([rhs]);
        while let Some(tree) = pending.pop_front() {
            if !visited.insert(tree) {
                continue;
            }
            let Some(node) = nodes.get(&tree.index()) else {
                continue;
            };
            if let TreeKind::PhaseSpecific(UntypedNode::PrefixOp(prefix)) = &node.kind {
                *histogram
                    .entry(names.resolve(prefix.op.text()).to_owned())
                    .or_default() += 1;
            }
            pending.extend(term_expression_children(&node.kind));
        }
    }
    histogram
}

const TYPE_TREE_FORMS: &[&str] = &[
    "Ident",
    "Select",
    "TypeTree",
    "AppliedTypeTree",
    "ByNameTypeTree",
    "TypeBoundsTree",
    "SingletonTypeTree",
    "RefinedTypeTree",
    "LambdaTypeTree",
    "MatchTypeTree",
    "Annotated",
    "InfixOp::|",
    "InfixOp::&",
    "InfixOp::<other>",
    "PostfixOp::*",
    "PostfixOp::<other>",
    "Function",
    "FunctionWithMods",
    "ContextBoundTypeTree",
    "Parens",
    "Tuple",
];

fn empty_type_tree_histogram() -> BTreeMap<String, usize> {
    TYPE_TREE_FORMS
        .iter()
        .map(|form| ((*form).to_owned(), 0))
        .collect()
}

fn empty_type_tree_form_files() -> BTreeMap<String, BTreeSet<String>> {
    TYPE_TREE_FORMS
        .iter()
        .map(|form| ((*form).to_owned(), BTreeSet::new()))
        .collect()
}

fn collect_declared_type_tree_histogram(
    arena: &dotty_core::AstArena<Untyped>,
    names: &dotty_core::names::NameInterner,
) -> BTreeMap<String, usize> {
    collect_declared_type_tree_inventory(arena, names).0
}

fn collect_declared_type_tree_inventory(
    arena: &dotty_core::AstArena<Untyped>,
    names: &dotty_core::names::NameInterner,
) -> (
    BTreeMap<String, usize>,
    BTreeSet<String>,
    HashSet<dotty_core::TreeId<Untyped>>,
) {
    let operators = source_operator_spellings(arena, names);
    let local_stats = local_stat_trees(arena);
    let mut roots = Vec::new();
    roots.extend(local_pattern_type_trees(arena));
    for (tree, node) in arena.iter() {
        match &node.kind {
            TreeKind::DefDef(definition) => {
                roots.push(definition.tpt);
                roots.extend(definition.type_params.iter().copied());
                roots.extend(definition.value_param_clauses.iter().flatten().filter_map(
                    |parameter| match &arena.get(*parameter).kind {
                        TreeKind::ValDef(parameter) => Some(parameter.tpt),
                        _ => None,
                    },
                ));
            }
            TreeKind::ValDef(definition) if local_stats.contains(&tree) => {
                roots.push(definition.tpt);
            }
            TreeKind::PhaseSpecific(UntypedNode::PatDef(definition))
                if local_stats.contains(&tree) =>
            {
                roots.push(definition.tpt);
            }
            TreeKind::TypeDef(definition)
                if local_stats.contains(&tree)
                    && !matches!(arena.get(definition.rhs).kind, TreeKind::Template(_)) =>
            {
                roots.push(definition.rhs);
            }
            _ => {}
        }
    }

    let nodes = arena
        .iter()
        .map(|(tree, node)| (tree.index(), node))
        .collect::<BTreeMap<_, _>>();
    let mut histogram = empty_type_tree_histogram();
    let mut forms = BTreeSet::new();
    let mut visited = HashSet::new();
    let mut pending = roots;
    while let Some(tree) = pending.pop() {
        if !visited.insert(tree) {
            continue;
        }
        let Some(node) = nodes.get(&tree.index()) else {
            continue;
        };
        if matches!(node.kind, TreeKind::TypeTree(_))
            && node
                .position
                .is_some_and(|position| position.span().range().is_empty())
        {
            continue;
        }
        if let Some(form) = type_tree_form(&node.kind, tree.index(), &operators) {
            *histogram.entry(form.to_owned()).or_default() += 1;
            forms.insert(form.to_owned());
        }
        pending.extend(type_tree_children(&node.kind));
    }
    (histogram, forms, visited)
}

fn type_tree_form(
    kind: &TreeKind<Untyped>,
    tree_index: u32,
    operators: &BTreeMap<u32, String>,
) -> Option<&'static str> {
    match kind {
        TreeKind::Ident(_) => Some("Ident"),
        TreeKind::Select(_) => Some("Select"),
        TreeKind::TypeTree(_) => Some("TypeTree"),
        TreeKind::AppliedTypeTree(_) => Some("AppliedTypeTree"),
        TreeKind::ByNameTypeTree(_) => Some("ByNameTypeTree"),
        TreeKind::TypeBoundsTree(_) => Some("TypeBoundsTree"),
        TreeKind::SingletonTypeTree(_) => Some("SingletonTypeTree"),
        TreeKind::RefinedTypeTree(_) => Some("RefinedTypeTree"),
        TreeKind::LambdaTypeTree(_) => Some("LambdaTypeTree"),
        TreeKind::MatchTypeTree(_) => Some("MatchTypeTree"),
        TreeKind::Annotated(_) => Some("Annotated"),
        TreeKind::PhaseSpecific(UntypedNode::InfixOp(_)) => {
            Some(match operators.get(&tree_index).map(String::as_str) {
                Some("|") => "InfixOp::|",
                Some("&") => "InfixOp::&",
                _ => "InfixOp::<other>",
            })
        }
        TreeKind::PhaseSpecific(UntypedNode::PostfixOp(_)) => {
            Some(match operators.get(&tree_index).map(String::as_str) {
                Some("*") => "PostfixOp::*",
                _ => "PostfixOp::<other>",
            })
        }
        TreeKind::PhaseSpecific(UntypedNode::Function(_)) => Some("Function"),
        TreeKind::PhaseSpecific(UntypedNode::FunctionWithMods(_)) => Some("FunctionWithMods"),
        TreeKind::PhaseSpecific(UntypedNode::ContextBoundTypeTree(_)) => {
            Some("ContextBoundTypeTree")
        }
        TreeKind::PhaseSpecific(UntypedNode::Parens(_)) => Some("Parens"),
        TreeKind::PhaseSpecific(UntypedNode::Tuple(_)) => Some("Tuple"),
        _ => None,
    }
}

fn type_tree_children(kind: &TreeKind<Untyped>) -> Vec<dotty_core::TreeId<Untyped>> {
    match kind {
        TreeKind::Select(node) => vec![node.qualifier],
        TreeKind::AppliedTypeTree(node) => std::iter::once(node.tpt)
            .chain(node.args.iter().copied())
            .collect(),
        TreeKind::ByNameTypeTree(node) => vec![node.result],
        TreeKind::TypeBoundsTree(node) => [node.low, node.high, node.alias]
            .into_iter()
            .flatten()
            .collect(),
        // The reference is a term path inside the singleton type, not another
        // type-tree node for this structural histogram.
        TreeKind::SingletonTypeTree(_) => Vec::new(),
        TreeKind::RefinedTypeTree(node) => std::iter::once(node.tpt)
            .chain(node.refinements.iter().copied())
            .collect(),
        TreeKind::LambdaTypeTree(node) => node
            .type_params
            .iter()
            .copied()
            .chain([node.body])
            .collect(),
        TreeKind::MatchTypeTree(node) => node
            .bound
            .into_iter()
            .chain([node.selector])
            .chain(node.cases.iter().copied())
            .collect(),
        TreeKind::Annotated(node) => vec![node.expr],
        TreeKind::PhaseSpecific(UntypedNode::InfixOp(node)) => vec![node.left, node.right],
        TreeKind::PhaseSpecific(UntypedNode::PostfixOp(node)) => vec![node.operand],
        TreeKind::PhaseSpecific(UntypedNode::Function(node)) => {
            node.params.iter().copied().chain([node.body]).collect()
        }
        TreeKind::PhaseSpecific(UntypedNode::FunctionWithMods(node)) => {
            node.params.iter().copied().chain([node.result]).collect()
        }
        TreeKind::PhaseSpecific(UntypedNode::ContextBounds(node)) => std::iter::once(node.bounds)
            .chain(node.context_bounds.iter().copied())
            .collect(),
        TreeKind::PhaseSpecific(UntypedNode::ContextBoundTypeTree(node)) => vec![node.bound],
        TreeKind::PhaseSpecific(UntypedNode::Parens(node)) => vec![node.inner],
        TreeKind::PhaseSpecific(UntypedNode::Tuple(node)) => node.elements.clone(),
        TreeKind::ValDef(node) => vec![node.tpt],
        TreeKind::DefDef(node) => node
            .type_params
            .iter()
            .copied()
            .chain(node.value_param_clauses.iter().flatten().copied())
            .chain([node.tpt])
            .collect(),
        TreeKind::TypeDef(node) => vec![node.rhs],
        TreeKind::CaseDef(node) => vec![node.pattern, node.body],
        _ => Vec::new(),
    }
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
        TreeKind::Annotated(node) => children.push(node.expr),
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
        TreeKind::PhaseSpecific(UntypedNode::InlineIf(node)) => {
            children.extend([node.cond, node.then_branch, node.else_branch]);
        }
        TreeKind::PhaseSpecific(UntypedNode::InlineMatch(node)) => {
            children.push(node.selector);
            children.extend(node.cases.iter().copied());
        }
        TreeKind::PhaseSpecific(UntypedNode::MacroTree(node)) => children.push(node.expr),
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
