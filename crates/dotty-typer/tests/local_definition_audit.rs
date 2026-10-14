use std::collections::{BTreeMap, BTreeSet, HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};

use dotty_classloader::classloader::{
    BinaryName, ClassFormat, ClassLoadProfile, ClassLoader, ClassOrigin, ClassPathEntry,
    ClassPathError, ClassResource, ClasspathSymbolResolver, CompositeClassPath, JarClassPath,
    JdkClassPath, LoadingSession, ModuleResolutionProfile,
};
use dotty_core::ast::{Match, Modifier, Tree, TreeKind, Untyped, UntypedNode, VisibilitySyntax};
use dotty_core::ids::TreeId;
use dotty_core::types::{TermRefTarget, Type, TypeRefTarget};
use dotty_core::{
    Definitions, MemberRequest, MemberSelector, MemberSpace, Namespace, Packages, ResolutionError,
    ResolverCheckpoint, SemanticStore, SourceId, SourceText, SymbolId, SymbolResolver, TextRange,
};
use dotty_lexer::ContextualScanner;
use dotty_namer::{NamerError, name_compilation_unit};
use dotty_parser::parse_compilation_unit;
use dotty_tasty_unpickler::tasty_unpickler::TastyUnpickler;
use dotty_typer::{SourceTyper, TyperError};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct ResolverMetrics {
    current_file: String,
    current_method: String,
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
    member_error_wrappers: BTreeMap<String, usize>,
    member_error_distinct_targets: BTreeMap<String, BTreeSet<String>>,
    member_error_files: BTreeMap<String, BTreeSet<String>>,
    member_request_shapes: BTreeMap<String, usize>,
    member_request_unknown_identity_shapes: BTreeMap<String, usize>,
    member_request_targets: BTreeMap<String, RequestTargetMetrics>,
    member_requests_by_method: BTreeMap<String, BTreeMap<String, usize>>,
    resolver_requests_by_method: BTreeMap<String, Vec<ResolverRequestObservation>>,
    member_request_outcome_targets: BTreeMap<String, BTreeSet<String>>,
    member_request_outcome_files: BTreeMap<String, BTreeSet<String>>,
    member_error_roots: BTreeMap<String, usize>,
    dependency_failure_chains: BTreeMap<String, usize>,
    tasty_supertype_failures: BTreeMap<String, usize>,
    incomplete_prefix_states: BTreeMap<String, usize>,
    incomplete_prefix_binary_resources: BTreeMap<String, usize>,
    unresolved_member_kinds: BTreeMap<String, usize>,
    external_member_first_seen: usize,
    external_member_session_reuse: usize,
    seen_external_symbols: BTreeSet<String>,
    package_term_type_class_resource: usize,
    package_term_without_type_class_resource: usize,
    package_classpath_probe_errors: usize,
    package_term_type_class_targets: BTreeSet<String>,
    package_term_companion_resource: usize,
    package_term_companion_targets: BTreeSet<String>,
    package_term_object_looking_targets: BTreeSet<String>,
    package_term_request_rows: Vec<PackageTermRequestObservation>,
    module_package_term_observations: Vec<ModulePackageTermObservation>,
    module_operation_observations: Vec<ModuleOperationObservation>,
    module_member_observations: Vec<ModuleMemberObservation>,
    module_objects_by_class: BTreeMap<String, String>,
    module_operation_checkpoints: BTreeMap<u64, (ResolverCheckpoint, usize, usize)>,
    next_module_operation_checkpoint: u64,
    module_identity_resolver_calls: usize,
    module_identity_resolver_outcomes: BTreeMap<&'static str, usize>,
    module_completion_resolver_calls: usize,
    module_completion_resolver_outcomes: BTreeMap<&'static str, usize>,
    package_type_class_resource: usize,
    package_type_class_targets: BTreeSet<String>,
    dependency_profiles: Vec<DependencyObservation>,
    class_member_successes_by_kind: BTreeMap<&'static str, usize>,
    class_member_after_shell_completion: usize,
    class_member_completed_prefix_reuse: usize,
    class_member_prefixes: BTreeSet<String>,
    class_member_keys: BTreeSet<String>,
    module_resolution: ModuleResolutionProfile,
}

fn merge_module_resolution_profile(
    target: &mut ModuleResolutionProfile,
    source: ModuleResolutionProfile,
) {
    target.package_term_requests += source.package_term_requests;
    target.package_term_successes += source.package_term_successes;
    target.package_term_unresolved += source.package_term_unresolved;
    target.package_term_non_object_results += source.package_term_non_object_results;
    target.package_term_errors += source.package_term_errors;
    target.new_object_identities += source.new_object_identities;
    target.reused_object_identities += source.reused_object_identities;
    target.module_member_successes += source.module_member_successes;
    target
        .resolved_object_targets
        .extend(source.resolved_object_targets);
    target
        .resolved_member_targets
        .extend(source.resolved_member_targets);
    target
        .package_term_targets
        .extend(source.package_term_targets);
    target.module_targets.extend(source.module_targets);
    target.member_targets.extend(source.member_targets);
    target
        .completed_module_classes
        .extend(source.completed_module_classes);
}

#[test]
fn merged_module_resolution_profiles_union_only_resolved_object_and_member_targets() {
    let mut aggregate = ModuleResolutionProfile {
        package_term_successes: 1,
        resolved_object_targets: BTreeSet::from(["fixture::First".to_owned()]),
        resolved_member_targets: BTreeSet::from([
            "fixture/First::namespace=term::name=value::space=Prefix::selector=Unique".to_owned(),
        ]),
        ..ModuleResolutionProfile::default()
    };

    let next = ModuleResolutionProfile {
        package_term_successes: 2,
        resolved_object_targets: BTreeSet::from([
            "fixture::First".to_owned(),
            "fixture::Second".to_owned(),
        ]),
        resolved_member_targets: BTreeSet::from([
            "fixture/Second::namespace=term::name=value::space=Prefix::selector=Unique".to_owned(),
        ]),
        member_targets: BTreeSet::from(["fixture/Failed::name=missing".to_owned()]),
        ..ModuleResolutionProfile::default()
    };

    merge_module_resolution_profile(&mut aggregate, next);

    assert_eq!(aggregate.package_term_successes, 3);
    assert_eq!(
        aggregate.resolved_object_targets,
        BTreeSet::from(["fixture::First".to_owned(), "fixture::Second".to_owned()])
    );
    assert_eq!(aggregate.resolved_member_targets.len(), 2);
    assert_eq!(aggregate.member_targets.len(), 1);
}

#[test]
fn audit_tsv_rows_render_missing_cells_without_trailing_tabs() {
    assert_eq!(tsv_or_dash(""), "-");
    assert_eq!(
        tsv_or_dash("scala.collection.mutable"),
        "scala.collection.mutable"
    );
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ResolverRequestObservation {
    target: String,
    outcome: String,
    cause: Option<String>,
    materialized_prefix: Option<String>,
    enclosing_attempt: String,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct PackageTermRequestObservation {
    request_key: String,
    prefix_kind: &'static str,
    package: String,
    name: String,
    member_space: String,
    selector: &'static str,
    source_file: String,
    enclosing_attempt: String,
    outcome: String,
    type_resource: String,
    companion_resource: String,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct ModulePackageTermObservation {
    request_key: String,
    source_file: String,
    enclosing_attempt: String,
    resolution: &'static str,
    object_origin: &'static str,
    object_identity: String,
    module_class_identity: String,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct ModuleOperationObservation {
    operation: &'static str,
    source_file: String,
    enclosing_attempt: String,
    object_identity: String,
    object_origin: &'static str,
    module_class_identity: String,
    result: &'static str,
    result_state: &'static str,
    module_state: &'static str,
    transaction_state: &'static str,
    error_category: Option<&'static str>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct ModuleMemberObservation {
    source_file: String,
    enclosing_attempt: String,
    module_class_identity: String,
    request_key: String,
    result: &'static str,
    result_symbol: String,
    module_state_before: &'static str,
    module_state_after: &'static str,
    transaction_state: &'static str,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct ResolutionBlockerObservation {
    blocker: String,
    cause: String,
    file: String,
    method: String,
    failed_qualifier: String,
    request_attempts: BTreeSet<String>,
    targets: BTreeSet<String>,
    candidate_causes: BTreeSet<String>,
    materialized_prefixes: BTreeSet<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct ResolutionBlockerProfile {
    observations: BTreeSet<ResolutionBlockerObservation>,
}

#[derive(Default)]
struct ResolverCauseGroup {
    count: usize,
    files: BTreeSet<String>,
    methods_with_materialized_prefix: BTreeSet<String>,
    targets: BTreeSet<String>,
    candidate_causes: BTreeSet<String>,
    materialized_prefixes: BTreeSet<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct DependencyObservation {
    file: String,
    method: String,
    request: String,
    profile: ClassLoadProfile,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct RequestTargetMetrics {
    requests: usize,
    identity_known: bool,
    files: BTreeSet<String>,
    owners: BTreeSet<String>,
    names: BTreeSet<String>,
    methods: BTreeSet<String>,
    outcomes: BTreeMap<String, usize>,
    families: BTreeMap<String, RequestFamilyMetrics>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct RequestFamilyMetrics {
    requests: usize,
    files: BTreeSet<String>,
    owners: BTreeSet<String>,
    names: BTreeSet<String>,
    outcomes: BTreeMap<String, usize>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct RequestShapeSummary {
    requests: usize,
    known_requests: usize,
    unknown_identity_requests: usize,
    distinct_keys: BTreeSet<String>,
    owners: BTreeSet<String>,
    names: BTreeSet<String>,
    files: BTreeSet<String>,
    outcomes: BTreeMap<String, usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PrefixProfile {
    shape: &'static str,
    target_kind: &'static str,
    identity: String,
    identity_known: bool,
    symbol: Option<SymbolId>,
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
    class_path_probe: Option<SharedClassPath>,
}

#[derive(Clone)]
struct SharedClassPath(Arc<CompositeClassPath>);

struct RequestTargetObservation<'a> {
    target_key: String,
    shape: String,
    owner: Option<&'a str>,
    name: &'a str,
    file: &'a str,
    method: &'a str,
    outcome: &'a str,
}

fn record_member_request_target(
    metrics: &mut ResolverMetrics,
    observation: RequestTargetObservation<'_>,
) {
    let target = metrics
        .member_request_targets
        .entry(observation.target_key)
        .or_default();
    target.requests += 1;
    target.identity_known = observation.owner.is_some();
    target.files.insert(observation.file.to_owned());
    if let Some(owner) = observation.owner {
        target.owners.insert(owner.to_owned());
    }
    target.names.insert(observation.name.to_owned());
    target.methods.insert(observation.method.to_owned());
    *target
        .outcomes
        .entry(observation.outcome.to_owned())
        .or_default() += 1;
    let family = target.families.entry(observation.shape).or_default();
    family.requests += 1;
    family.files.insert(observation.file.to_owned());
    if let Some(owner) = observation.owner {
        family.owners.insert(owner.to_owned());
    }
    family.names.insert(observation.name.to_owned());
    *family
        .outcomes
        .entry(observation.outcome.to_owned())
        .or_default() += 1;
}

fn member_request_shape_summaries(
    metrics: &ResolverMetrics,
) -> BTreeMap<String, RequestShapeSummary> {
    let mut by_shape = BTreeMap::<String, RequestShapeSummary>::new();
    for (key, target) in &metrics.member_request_targets {
        for (shape, family) in &target.families {
            let summary = by_shape.entry(shape.clone()).or_default();
            summary.requests += family.requests;
            if target.identity_known {
                summary.known_requests += family.requests;
                summary.distinct_keys.insert(key.clone());
            } else {
                summary.unknown_identity_requests += family.requests;
            }
            summary.owners.extend(family.owners.iter().cloned());
            summary.names.extend(family.names.iter().cloned());
            summary.files.extend(family.files.iter().cloned());
            for (outcome, count) in &family.outcomes {
                *summary.outcomes.entry(outcome.clone()).or_default() += count;
            }
        }
    }
    by_shape
}

fn member_request_repeat_factors(summary: &RequestShapeSummary) -> (String, String) {
    let known_repeat = (!summary.distinct_keys.is_empty())
        .then(|| summary.known_requests as f64 / summary.distinct_keys.len() as f64)
        .map_or_else(|| "unavailable".to_owned(), |factor| format!("{factor:.3}"));
    let repeat = if summary.unknown_identity_requests > 0 {
        "unavailable (unknown identities included)".to_owned()
    } else {
        known_repeat.clone()
    };
    (repeat, known_repeat)
}

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
    // Module-port maintenance contract: module_class_of_object and
    // complete_module_class are forwarded here. The trait also has default
    // hooks, so changes to module-specific SymbolResolver operations must be
    // reviewed against the dyn SymbolResolver regression tests below.
    fn module_class_of_object(
        &self,
        store: &SemanticStore,
        object: SymbolId,
    ) -> Result<Option<SymbolId>, ResolutionError> {
        let result = self.inner.module_class_of_object(store, object);
        let (source_file, enclosing_attempt) = {
            let mut metrics = self.metrics.borrow_mut();
            metrics.module_identity_resolver_calls += 1;
            let outcome = match &result {
                Ok(Some(_)) => "Ok(Some)",
                Ok(None) => "Ok(None)",
                Err(_) => "Err",
            };
            *metrics
                .module_identity_resolver_outcomes
                .entry(outcome)
                .or_default() += 1;
            if metrics.module_identity_resolver_calls.is_multiple_of(1_000) {
                eprintln!(
                    "module identity resolver calls={} file={}",
                    metrics.module_identity_resolver_calls, metrics.current_file
                );
            }
            (metrics.current_file.clone(), metrics.current_method.clone())
        };
        // The corpus profile concerns first-blocker method attempts. Avoid
        // building stable symbol paths for unrelated source/setup lookups.
        if enclosing_attempt.is_empty() {
            return result;
        }
        let object_identity = if store.symbols.contains(object) {
            audit_module_object_candidate(store, object)
                .unwrap_or_else(|| "<unknown-object>".to_owned())
        } else {
            "<unknown-object>".to_owned()
        };
        let object_origin = symbol_origin_category(store, object);
        let (module_class_identity, outcome, result_state, error_category) = match &result {
            Ok(Some(module_class))
                if store.symbols.contains(*module_class)
                    && store.symbols.get(*module_class).kind
                        == dotty_core::SymbolKind::ModuleClass =>
            {
                let module_class_identity = audit_binary_candidate(store, *module_class)
                    .unwrap_or_else(|| "<unknown-module-class>".to_owned());
                (
                    module_class_identity,
                    "resolved",
                    "validated_module_class",
                    None,
                )
            }
            Ok(Some(module_class)) if store.symbols.contains(*module_class) => (
                audit_binary_candidate(store, *module_class)
                    .unwrap_or_else(|| "<unknown-module-class>".to_owned()),
                "resolved_wrong_kind",
                "wrong_symbol_kind",
                None,
            ),
            Ok(Some(_)) => (
                "<unknown-module-class>".to_owned(),
                "resolved_unknown_symbol",
                "unknown_symbol",
                None,
            ),
            Ok(None) => (String::new(), "unresolved", "no_module_class", None),
            Err(error) => (
                String::new(),
                "error",
                "lookup_error",
                Some(resolution_error_category(error)),
            ),
        };
        let module_state = match &result {
            Ok(Some(module_class)) => module_class_observed_state(store, *module_class),
            _ => "Unavailable",
        };
        let mut metrics = self.metrics.borrow_mut();
        if outcome == "resolved"
            && !object_identity.starts_with('<')
            && !module_class_identity.starts_with('<')
        {
            metrics
                .module_objects_by_class
                .insert(module_class_identity.clone(), object_identity.clone());
        }
        metrics
            .module_operation_observations
            .push(ModuleOperationObservation {
                operation: "module_class_of_object",
                source_file,
                enclosing_attempt,
                object_identity,
                object_origin,
                module_class_identity,
                result: outcome,
                result_state,
                module_state,
                transaction_state: "observed",
                error_category,
            });
        result
    }

    fn complete_module_class(
        &mut self,
        store: &mut SemanticStore,
        module_class: SymbolId,
    ) -> Result<bool, ResolutionError> {
        let module_class_identity = if store.symbols.contains(module_class) {
            audit_binary_candidate(store, module_class)
                .unwrap_or_else(|| "<unknown-module-class>".to_owned())
        } else {
            "<unknown-module-class>".to_owned()
        };
        let object_identity = self
            .metrics
            .borrow()
            .module_objects_by_class
            .get(&module_class_identity)
            .cloned()
            .unwrap_or_default();
        let object_origin = symbol_origin_category(store, module_class);
        let request = format!("module-completion::{module_class_identity}");
        let result = self.inner.complete_module_class(store, module_class);
        let profiles = self.inner.take_load_profiles();
        let result_state = module_class_observed_state(store, module_class);
        let (outcome, error_category) = match &result {
            Ok(true) if result_state == "Complete(ClassInfo)" => ("completed", None),
            Ok(true) => ("invalid_success", None),
            Ok(false) => ("unsupported", None),
            Err(error) => ("error", Some(resolution_error_category(error))),
        };
        let mut metrics = self.metrics.borrow_mut();
        metrics.module_completion_resolver_calls += 1;
        *metrics
            .module_completion_resolver_outcomes
            .entry(outcome)
            .or_default() += 1;
        let file = metrics.current_file.clone();
        let method = metrics.current_method.clone();
        for profile in profiles {
            metrics.dependency_profiles.push(DependencyObservation {
                file: file.clone(),
                method: method.clone(),
                request: request.clone(),
                profile,
            });
        }
        if !method.is_empty() {
            metrics
                .module_operation_observations
                .push(ModuleOperationObservation {
                    operation: "complete_module_class",
                    source_file: file,
                    enclosing_attempt: method,
                    object_identity,
                    object_origin,
                    module_class_identity,
                    result: outcome,
                    result_state,
                    module_state: result_state,
                    transaction_state: "observed",
                    error_category,
                });
        }
        result
    }

    fn complete_class(
        &mut self,
        store: &mut SemanticStore,
        symbol: SymbolId,
    ) -> Result<bool, ResolutionError> {
        let request = if store.symbols.contains(symbol) {
            format!("completion::{}", audit_symbol_path(store, symbol))
        } else {
            "completion::unknown-symbol".to_owned()
        };
        let result = self.inner.complete_class(store, symbol);
        let profiles = self.inner.take_load_profiles();
        let mut metrics = self.metrics.borrow_mut();
        let file = metrics.current_file.clone();
        let method = metrics.current_method.clone();
        for profile in profiles {
            metrics.dependency_profiles.push(DependencyObservation {
                file: file.clone(),
                method: method.clone(),
                request: request.clone(),
                profile,
            });
        }
        result
    }

    fn checkpoint(&self) -> ResolverCheckpoint {
        let inner_checkpoint = self.inner.checkpoint();
        let mut metrics = self.metrics.borrow_mut();
        metrics.next_module_operation_checkpoint = metrics
            .next_module_operation_checkpoint
            .checked_add(1)
            .expect("audit resolver checkpoint token overflow");
        let token = metrics.next_module_operation_checkpoint;
        let offsets = (
            metrics.module_operation_observations.len(),
            metrics.module_member_observations.len(),
        );
        metrics
            .module_operation_checkpoints
            .insert(token, (inner_checkpoint, offsets.0, offsets.1));
        ResolverCheckpoint::new(token)
    }

    fn rollback_to(&mut self, store: &mut SemanticStore, checkpoint: ResolverCheckpoint) {
        let metrics = self.metrics.borrow_mut();
        let Some((inner_checkpoint, operation_offset, member_offset)) = metrics
            .module_operation_checkpoints
            .get(&checkpoint.token())
            .copied()
        else {
            return;
        };
        drop(metrics);
        self.inner.rollback_to(store, inner_checkpoint);
        let mut metrics = self.metrics.borrow_mut();
        for observation in metrics
            .module_operation_observations
            .iter_mut()
            .skip(operation_offset)
        {
            observation.transaction_state = "rolled_back";
        }
        for observation in metrics
            .module_member_observations
            .iter_mut()
            .skip(member_offset)
        {
            observation.transaction_state = "rolled_back";
        }
        metrics
            .module_operation_checkpoints
            .retain(|token, _| *token <= checkpoint.token());
    }

    fn resolve_member(
        &mut self,
        store: &mut SemanticStore,
        request: &MemberRequest,
    ) -> Result<Option<SymbolId>, ResolutionError> {
        let prefix = prefix_profile(store, request.prefix);
        let module_class = match prefix.target_kind {
            "ModuleClass" => prefix.symbol,
            "Object" => prefix.symbol.and_then(|object| {
                canonical_module_class_of_object(store, object)
                    .or_else(|| self.module_class_of_object(store, object).ok().flatten())
            }),
            _ => None,
        };
        let module_class_identity =
            module_class.map_or_else(String::new, |symbol| audit_symbol_path(store, symbol));
        let module_state_before = module_class.map_or("NotModuleClass", |symbol| {
            module_class_observed_state(store, symbol)
        });
        let name = store.names.resolve(request.name.text()).to_owned();
        let namespace = match request.name.namespace() {
            Namespace::Type => "type",
            Namespace::Term => "term",
        };
        let space = request_space(store, &request.space);
        let shape = format!(
            "space={space}::prefix={}::target={}::namespace={namespace}::selector={}::site=not-exposed-by-SymbolResolver",
            prefix.shape,
            prefix.target_kind,
            request_selector(&request.selector)
        );
        let target_key = format!(
            "prefix={}::namespace={namespace}::name={name}::space={space}",
            prefix.identity
        );
        let mut type_resource = "not-probed".to_owned();
        let mut companion_resource = "not-probed".to_owned();
        if prefix.target_kind == "Package"
            && let Some(class_path) = &self.class_path_probe
        {
            let internal = if prefix.identity.is_empty() {
                name.clone()
            } else {
                format!("{}/{name}", prefix.identity.replace('.', "/"))
            };
            let binary =
                dotty_classloader::classloader::BinaryName::from_internal(internal.clone());
            let mut metrics = self.metrics.borrow_mut();
            match class_path.find_class(&binary) {
                Ok(Some(resource)) => {
                    type_resource = class_resource_label(&resource);
                    if request.name.is_term() {
                        metrics.package_term_type_class_resource += 1;
                        metrics
                            .package_term_type_class_targets
                            .insert(target_key.clone());
                    } else {
                        metrics.package_type_class_resource += 1;
                        metrics
                            .package_type_class_targets
                            .insert(target_key.clone());
                    }
                }
                Ok(None) if request.name.is_term() => {
                    type_resource = "absent".to_owned();
                    metrics.package_term_without_type_class_resource += 1
                }
                Ok(None) => type_resource = "absent".to_owned(),
                Err(error) => {
                    type_resource = format!("error:{}", error.code());
                    metrics.package_classpath_probe_errors += 1;
                }
            }
            if request.name.is_term() && (name == "package" || name.ends_with('$')) {
                metrics
                    .package_term_object_looking_targets
                    .insert(target_key.clone());
            }
            if request.name.is_term() {
                let companion = dotty_classloader::classloader::BinaryName::from_internal(format!(
                    "{internal}$"
                ));
                match class_path.find_class(&companion) {
                    Ok(Some(resource)) => {
                        companion_resource = class_resource_label(&resource);
                        metrics.package_term_companion_resource += 1;
                        metrics
                            .package_term_companion_targets
                            .insert(target_key.clone());
                    }
                    Ok(None) => companion_resource = "absent".to_owned(),
                    Err(error) => {
                        companion_resource = format!("error:{}", error.code());
                        metrics.package_classpath_probe_errors += 1;
                    }
                }
            }
        }
        if let Some(symbol) = prefix.symbol
            && matches!(prefix.target_kind, "Class" | "Trait" | "ModuleClass")
            && !matches!(
                store.symbols.get(symbol).info,
                dotty_core::SymbolInfo::Complete(_)
            )
            && let Some(class_path) = &self.class_path_probe
        {
            let origin = match store.symbols.get(symbol).origin {
                dotty_core::SymbolOrigin::Source(_) => "source",
                dotty_core::SymbolOrigin::Classfile(_) => "classloader",
                dotty_core::SymbolOrigin::Tasty(_) => "TASTy",
                dotty_core::SymbolOrigin::Builtin => "builtin",
                dotty_core::SymbolOrigin::Synthetic => "synthetic",
            };
            let candidate = audit_binary_candidate(store, symbol);
            let status = match candidate.as_deref() {
                Some(name) => match class_path.find_class(
                    &dotty_classloader::classloader::BinaryName::from_internal(name),
                ) {
                    Ok(Some(_)) => "resource-found",
                    Ok(None) => "resource-absent",
                    Err(_) => "probe-error",
                },
                None => "binary-name-unrecoverable",
            };
            let key = format!(
                "state={}::origin={origin}::candidate={}::status={status}",
                symbol_info_label(store.symbols.get(symbol).info),
                candidate.as_deref().unwrap_or("<none>")
            );
            *self
                .metrics
                .borrow_mut()
                .incomplete_prefix_binary_resources
                .entry(key)
                .or_default() += 1;
        }
        let target_type = match request.space {
            MemberSpace::Explicit(owner) => owner,
            _ => request.prefix,
        };
        let actual_target = prefix_profile(store, target_type);
        let target_info_before = actual_target
            .symbol
            .filter(|id| store.symbols.contains(*id))
            .map(|id| store.symbols.get(id).info);
        let external_class_target = actual_target.symbol.is_some_and(|id| {
            store.symbols.contains(id)
                && is_external_symbol(store.symbols.get(id).origin)
                && matches!(
                    store.symbols.get(id).kind,
                    dotty_core::SymbolKind::Class
                        | dotty_core::SymbolKind::Trait
                        | dotty_core::SymbolKind::ModuleClass
                )
        });
        let target_binary = external_class_target
            .then(|| {
                actual_target
                    .symbol
                    .and_then(|id| audit_binary_candidate(store, id))
            })
            .flatten();
        let result = self.inner.resolve_member(store, request);
        let load_profiles = self.inner.take_load_profiles();
        let module_profile = self.inner.take_module_resolution_profile();
        // Profiles describe work performed even when Ok(None) rolls it back.
        // Only the actual lookup owner can establish that its scope was available.
        let target_completed = target_binary.as_ref().is_some_and(|name| {
            load_profiles
                .iter()
                .any(|profile| &profile.root == name && profile.completed_classes.contains(name))
        });
        let mut metrics = self.metrics.borrow_mut();
        metrics.resolver_member_requests += 1;
        *metrics
            .member_request_shapes
            .entry(shape.clone())
            .or_default() += 1;
        if !prefix.identity_known {
            *metrics
                .member_request_unknown_identity_shapes
                .entry(shape.clone())
                .or_default() += 1;
        }
        let outcome = member_outcome(store, &result, request, &actual_target, target_completed);
        let current_file = metrics.current_file.clone();
        let current_method = metrics.current_method.clone();
        if module_class.is_some() && !current_method.is_empty() {
            let (result, result_symbol) = match &result {
                Ok(Some(symbol)) if store.symbols.contains(*symbol) => {
                    ("success", audit_symbol_path(store, *symbol))
                }
                Ok(Some(_)) => ("unknown_symbol", String::new()),
                Ok(None) => ("unresolved", String::new()),
                Err(_) => ("error", String::new()),
            };
            metrics
                .module_member_observations
                .push(ModuleMemberObservation {
                    source_file: current_file.clone(),
                    enclosing_attempt: current_method.clone(),
                    module_class_identity: module_class_identity.clone(),
                    request_key: format!(
                        "prefix={}::namespace={namespace}::name={name}::space={space}::selector={}",
                        module_class_identity,
                        request_selector(&request.selector)
                    ),
                    result,
                    result_symbol,
                    module_state_before,
                    module_state_after: module_class.map_or("UnknownSymbol", |symbol| {
                        module_class_observed_state(store, symbol)
                    }),
                    transaction_state: "observed",
                });
        }
        merge_module_resolution_profile(&mut metrics.module_resolution, module_profile);
        if prefix.target_kind == "Package"
            && request.name.is_term()
            && matches!(&request.space, MemberSpace::Prefix)
            && matches!(&request.selector, dotty_core::MemberSelector::Unique)
        {
            let (resolution, object_identity, object_origin, module_class_identity) = match &result
            {
                Ok(Some(symbol)) if store.symbols.contains(*symbol) => {
                    if store.symbols.get(*symbol).kind == dotty_core::SymbolKind::Object {
                        let object_origin = symbol_origin_category(store, *symbol);
                        match self.inner.module_class_of_object(store, *symbol) {
                            Ok(Some(module_class)) if store.symbols.contains(module_class) => {
                                if let (Some(object_target), Some(module_class_target)) = (
                                    audit_module_object_candidate(store, *symbol),
                                    audit_binary_candidate(store, module_class),
                                ) {
                                    metrics
                                        .module_objects_by_class
                                        .insert(module_class_target, object_target);
                                }
                                (
                                    "validated_module_identity",
                                    audit_symbol_path(store, *symbol),
                                    object_origin,
                                    audit_symbol_path(store, module_class),
                                )
                            }
                            Ok(_) => (
                                "object_without_validated_module_identity",
                                audit_symbol_path(store, *symbol),
                                object_origin,
                                String::new(),
                            ),
                            Err(_) => (
                                "module_identity_validation_error",
                                audit_symbol_path(store, *symbol),
                                object_origin,
                                String::new(),
                            ),
                        }
                    } else {
                        (
                            "resolved_non_object_term",
                            audit_symbol_path(store, *symbol),
                            symbol_origin_category(store, *symbol),
                            String::new(),
                        )
                    }
                }
                Ok(Some(_)) => (
                    "resolved_unknown_symbol",
                    String::new(),
                    "unknown",
                    String::new(),
                ),
                Ok(None) => ("unresolved", String::new(), "unknown", String::new()),
                Err(_) => ("resolver_error", String::new(), "unknown", String::new()),
            };
            metrics
                .module_package_term_observations
                .push(ModulePackageTermObservation {
                    request_key: target_key.clone(),
                    source_file: current_file.clone(),
                    enclosing_attempt: current_method.clone(),
                    resolution,
                    object_origin,
                    object_identity,
                    module_class_identity,
                });
        }
        if prefix.target_kind == "Package"
            && request.name.is_term()
            && outcome == "package term member unsupported"
        {
            metrics
                .package_term_request_rows
                .push(PackageTermRequestObservation {
                    request_key: target_key.clone(),
                    prefix_kind: "Package",
                    package: prefix.identity.clone(),
                    name: name.clone(),
                    member_space: space.clone(),
                    selector: request_selector(&request.selector),
                    source_file: current_file.clone(),
                    enclosing_attempt: current_method.clone(),
                    outcome: outcome.clone(),
                    type_resource,
                    companion_resource,
                });
        }
        for profile in load_profiles {
            metrics.dependency_profiles.push(DependencyObservation {
                file: current_file.clone(),
                method: current_method.clone(),
                request: format!("{}::{}", target_key, name),
                profile,
            });
        }
        if let Ok(Some(symbol)) = &result {
            let entry = store.symbols.get(*symbol);
            if is_external_symbol(entry.origin) {
                if external_class_target {
                    record_external_class_member_success(
                        &mut metrics,
                        &actual_target.identity,
                        &name,
                        namespace,
                        entry.kind,
                        target_info_before,
                    );
                }
                let identity = format!("{current_file}::{}", audit_symbol_path(store, *symbol));
                if metrics.seen_external_symbols.insert(identity) {
                    metrics.external_member_first_seen += 1;
                } else {
                    metrics.external_member_session_reuse += 1;
                }
            }
        }
        let method = current_method;
        let materialized_prefix = materialized_request_prefix(store, &prefix);
        if !method.is_empty() {
            metrics
                .resolver_requests_by_method
                .entry(method.clone())
                .or_default()
                .push(ResolverRequestObservation {
                    target: target_key.clone(),
                    outcome: outcome.clone(),
                    cause: member_request_cause(&outcome, &actual_target, &result),
                    materialized_prefix,
                    enclosing_attempt: method.clone(),
                });
        }
        let target_key = if prefix.identity_known {
            target_key
        } else {
            format!(
                "{target_key}::unknown-request={}",
                metrics.resolver_member_requests
            )
        };
        record_member_request_target(
            &mut metrics,
            RequestTargetObservation {
                target_key: target_key.clone(),
                shape: shape.clone(),
                owner: prefix.identity_known.then_some(prefix.identity.as_str()),
                name: &name,
                file: &current_file,
                method: &method,
                outcome: &outcome,
            },
        );
        *metrics
            .member_requests_by_method
            .entry(method)
            .or_default()
            .entry(format!("{target_key}::outcome={outcome}"))
            .or_default() += 1;
        if prefix.identity_known {
            metrics
                .member_request_outcome_targets
                .entry(format!("{shape}::outcome={outcome}"))
                .or_default()
                .insert(target_key.clone());
        }
        metrics
            .member_request_outcome_files
            .entry(format!("{shape}::outcome={outcome}"))
            .or_default()
            .insert(current_file.clone());
        if let Some(symbol) = prefix.symbol {
            let entry = store.symbols.get(symbol);
            let state = match entry.info {
                dotty_core::SymbolInfo::Missing => "Missing",
                dotty_core::SymbolInfo::Error => "Error",
                dotty_core::SymbolInfo::Deferred(_) => "Deferred",
                dotty_core::SymbolInfo::Complete(_) => "Complete",
            };
            if state != "Complete"
                && matches!(prefix.target_kind, "Class" | "Trait" | "ModuleClass")
            {
                let origin = match entry.origin {
                    dotty_core::SymbolOrigin::Source(_) => "source",
                    dotty_core::SymbolOrigin::Classfile(_) => "classloader",
                    dotty_core::SymbolOrigin::Tasty(_) => "TASTy",
                    dotty_core::SymbolOrigin::Builtin => "builtin",
                    dotty_core::SymbolOrigin::Synthetic => "synthetic",
                };
                *metrics
                    .incomplete_prefix_states
                    .entry(format!(
                        "state={state}::origin={origin}::identity={}",
                        prefix.identity
                    ))
                    .or_default() += 1;
            }
        }
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
                let kind = outcome.clone();
                *metrics.unresolved_member_kinds.entry(kind).or_default() += 1;
                let name = store.names.resolve(request.name.text()).to_owned();
                *metrics.unresolved_member_names.entry(name).or_default() += 1;
            }
            Err(error) => {
                metrics.member_errors += 1;
                let (category, requested, root_cause) = normalized_error(error);
                *metrics
                    .member_error_kinds
                    .entry(category.clone())
                    .or_default() += 1;
                metrics
                    .member_error_distinct_targets
                    .entry(category.clone())
                    .or_default()
                    .insert(requested.clone());
                metrics
                    .member_error_files
                    .entry(category.clone())
                    .or_default()
                    .insert(current_file.clone());
                *metrics
                    .member_error_roots
                    .entry(format!(
                        "root_cause={category}::requested={requested}::root={root_cause}"
                    ))
                    .or_default() += 1;
                if category == "InvalidTastyFile::UnresolvedSupertype" {
                    let package = requested
                        .rsplit_once('/')
                        .map_or("<default-package>", |(package, _)| package);
                    let parent_profile = match error {
                        ResolutionError::Malformed { reason } => {
                            unresolved_tasty_parent_profile(reason)
                        }
                        _ => None,
                    };
                    let parent_profile = parent_profile.unwrap_or_else(|| {
                        "parent_index=unknown::parent_shape=not-exposed::semantic_reference=unknown::reference_prefix=unknown::visible_names=[]::raw_shape=unknown".to_owned()
                    });
                    *metrics
                        .tasty_supertype_failures
                        .entry(format!(
                            "requested={requested}::source_package={package}::{parent_profile}"
                        ))
                        .or_default() += 1;
                }
                if let ResolutionError::Malformed { reason } = error
                    && reason
                        .to_ascii_lowercase()
                        .contains("failed to load because")
                {
                    *metrics
                        .member_error_wrappers
                        .entry("DependencyFailure".to_owned())
                        .or_default() += 1;
                    let classes = error_binary_names(reason);
                    let distinct_dependencies =
                        classes.iter().skip(1).collect::<BTreeSet<_>>().len();
                    let depth = reason.matches(" failed to load because ").count();
                    *metrics
                        .dependency_failure_chains
                        .entry(format!(
                            "requested={requested}::root={root_cause}::root_cause={category}::depth={depth}::distinct_dependencies={distinct_dependencies}"
                        ))
                        .or_default() += 1;
                }
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
        let (target, outcome, cause) = match &result {
            Ok(Some(_)) => (path.join("."), "package success".to_owned(), None),
            Ok(None) => {
                let (cause, target) =
                    unresolved_package_details(self.class_path_probe.as_ref(), path);
                (target, "package unresolved".to_owned(), Some(cause))
            }
            Err(_) => (
                path.join("."),
                "package error".to_owned(),
                Some("classloader error".to_owned()),
            ),
        };
        if !metrics.current_method.is_empty() {
            let method = metrics.current_method.clone();
            metrics
                .resolver_requests_by_method
                .entry(method.clone())
                .or_default()
                .push(ResolverRequestObservation {
                    target,
                    outcome,
                    cause,
                    materialized_prefix: None,
                    enclosing_attempt: method.clone(),
                });
        }
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

fn symbol_info_label(info: dotty_core::SymbolInfo) -> &'static str {
    match info {
        dotty_core::SymbolInfo::Missing => "Missing",
        dotty_core::SymbolInfo::Error => "Error",
        dotty_core::SymbolInfo::Deferred(_) => "Deferred",
        dotty_core::SymbolInfo::Complete(_) => "Complete",
    }
}

fn audit_binary_candidate(store: &SemanticStore, symbol: SymbolId) -> Option<String> {
    let mut chain = Vec::new();
    let mut current = Some(symbol);
    let mut visited = HashSet::new();
    while let Some(current_symbol) = current {
        if !visited.insert(current_symbol) || !store.symbols.contains(current_symbol) {
            return None;
        }
        let entry = store.symbols.get(current_symbol);
        chain.push((
            store.names.resolve(entry.name.text()).to_owned(),
            entry.kind,
        ));
        current = entry.owner;
    }
    chain.reverse();
    let mut packages = Vec::new();
    let mut classes = Vec::new();
    for (name, kind) in chain {
        if kind == dotty_core::SymbolKind::Package {
            if classes.is_empty() && !name.is_empty() {
                packages.push(name);
            }
        } else if matches!(
            kind,
            dotty_core::SymbolKind::Class
                | dotty_core::SymbolKind::Trait
                | dotty_core::SymbolKind::ModuleClass
        ) {
            classes.push(name);
        }
    }
    let class = classes.join("$");
    if class.is_empty() {
        return None;
    }
    let package = packages.join("/");
    Some(if package.is_empty() {
        class
    } else {
        format!("{package}/{class}")
    })
}

fn audit_module_object_candidate(store: &SemanticStore, object: SymbolId) -> Option<String> {
    if !store.symbols.contains(object)
        || store.symbols.get(object).kind != dotty_core::SymbolKind::Object
    {
        return None;
    }
    let object_name = store.names.resolve(store.symbols.get(object).name.text());
    if object_name.is_empty() {
        return None;
    }
    let mut packages = Vec::new();
    let mut enclosing_classes = Vec::new();
    let mut current = store.symbols.get(object).owner;
    let mut visited = HashSet::from([object]);
    while let Some(symbol) = current {
        if !visited.insert(symbol) || !store.symbols.contains(symbol) {
            return None;
        }
        let entry = store.symbols.get(symbol);
        let name = store.names.resolve(entry.name.text());
        match entry.kind {
            dotty_core::SymbolKind::Package => {
                if !name.is_empty() {
                    packages.push(name.to_owned());
                }
            }
            dotty_core::SymbolKind::Class
            | dotty_core::SymbolKind::Trait
            | dotty_core::SymbolKind::ModuleClass => {
                if !name.is_empty() {
                    enclosing_classes.push(name.to_owned());
                }
            }
            _ => return None,
        }
        current = entry.owner;
    }
    packages.reverse();
    enclosing_classes.reverse();
    enclosing_classes.push(object_name.to_owned());
    let class = enclosing_classes.join("$");
    let package = packages.join("/");
    Some(if package.is_empty() {
        class
    } else {
        format!("{package}/{class}")
    })
}

fn symbol_origin_category(store: &SemanticStore, symbol: SymbolId) -> &'static str {
    if !store.symbols.contains(symbol) {
        return "unknown";
    }
    match store.symbols.get(symbol).origin {
        dotty_core::SymbolOrigin::Source(source) if source == SourceId::from_index(0) => {
            "source_local"
        }
        dotty_core::SymbolOrigin::Source(_) => "cross_source",
        dotty_core::SymbolOrigin::Classfile(_) => "classpath_classfile",
        dotty_core::SymbolOrigin::Tasty(_) => "classpath_tasty",
        dotty_core::SymbolOrigin::Synthetic => "synthetic",
        dotty_core::SymbolOrigin::Builtin => "builtin",
    }
}

fn record_external_class_member_success(
    metrics: &mut ResolverMetrics,
    prefix: &str,
    name: &str,
    namespace: &str,
    kind: dotty_core::SymbolKind,
    before: Option<dotty_core::SymbolInfo>,
) {
    let label = match kind {
        dotty_core::SymbolKind::Field => "field",
        dotty_core::SymbolKind::Method => "method",
        dotty_core::SymbolKind::Constructor => "constructor",
        _ if namespace == "type" => "type_or_nested_class",
        _ => "other",
    };
    *metrics
        .class_member_successes_by_kind
        .entry(label)
        .or_default() += 1;
    metrics.class_member_prefixes.insert(prefix.to_owned());
    metrics
        .class_member_keys
        .insert(format!("{prefix}::namespace={namespace}::name={name}"));
    match before {
        Some(dotty_core::SymbolInfo::Missing) => metrics.class_member_after_shell_completion += 1,
        Some(dotty_core::SymbolInfo::Complete(_)) => {
            metrics.class_member_completed_prefix_reuse += 1
        }
        _ => {}
    }
}

#[test]
fn class_member_success_metrics_distinguish_declarations_and_canonical_keys() {
    use dotty_core::{SymbolInfo, SymbolKind};
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let mut metrics = ResolverMetrics::default();
    let complete = Some(SymbolInfo::Complete(definitions.no_prefix));
    let missing = Some(SymbolInfo::Missing);
    for (name, namespace, kind, state) in [
        ("value", "term", SymbolKind::Field, missing),
        ("value", "term", SymbolKind::Field, complete),
        ("get", "term", SymbolKind::Method, missing),
        ("<init>", "term", SymbolKind::Constructor, complete),
        ("T", "type", SymbolKind::TypeParameter, complete),
        ("Nested", "type", SymbolKind::Class, complete),
        ("other", "term", SymbolKind::Value, complete),
    ] {
        record_external_class_member_success(&mut metrics, "p.Owner", name, namespace, kind, state);
    }
    record_external_class_member_success(
        &mut metrics,
        "p.Other",
        "value",
        "term",
        SymbolKind::Field,
        missing,
    );
    assert_eq!(
        metrics.class_member_successes_by_kind,
        BTreeMap::from([
            ("field", 3),
            ("method", 1),
            ("constructor", 1),
            ("type_or_nested_class", 2),
            ("other", 1),
        ])
    );
    assert_eq!(metrics.class_member_after_shell_completion, 3);
    assert_eq!(metrics.class_member_completed_prefix_reuse, 5);
    assert_eq!(metrics.class_member_prefixes.len(), 2);
    assert_eq!(metrics.class_member_keys.len(), 7);
}

struct MetricPath;

struct ModuleAuditPath {
    include_object_class: bool,
}

// Source.tasty and scala/package.tasty are the checked-in Scala 3.9.0 bytes
// pinned by the fixture manifest. The few Java/collection dependencies below
// are intentionally class stubs, matching the classloader fixture setup; the
// module and its declarations themselves are never synthesized.
struct ScalaSourceAuditPath;

impl ClassPathEntry for ModuleAuditPath {
    fn find_class(&self, name: &BinaryName) -> Result<Option<ClassResource>, ClassPathError> {
        let (bytes, format) = match name.as_internal() {
            "me/cytrowski/tastyfixtures/semantic/Both" => (
                fs::read(
                    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                        .join("../dotty-tasty-unpickler/tests/fixtures/semantic/Both.tasty"),
                )
                .unwrap(),
                ClassFormat::Tasty,
            ),
            "java/lang/Object" if self.include_object_class => {
                let internal_name = b"java/lang/Object";
                let mut bytes = vec![0xCA, 0xFE, 0xBA, 0xBE, 0, 0, 0, 61, 0, 3, 1];
                bytes.extend_from_slice(&(internal_name.len() as u16).to_be_bytes());
                bytes.extend_from_slice(internal_name);
                bytes.extend_from_slice(&[7, 0, 1, 0, 0x21, 0, 2, 0, 0]);
                bytes.extend_from_slice(&[0; 8]);
                (bytes, ClassFormat::Class)
            }
            _ => return Ok(None),
        };
        Ok(Some(ClassResource::new(
            bytes,
            format,
            ClassOrigin::Directory("<module-audit>".into()),
        )))
    }

    fn contains_package(&self, package: &[&str]) -> Result<bool, ClassPathError> {
        Ok(package == ["me", "cytrowski", "tastyfixtures", "semantic"])
    }
}

impl ClassPathEntry for ScalaSourceAuditPath {
    fn find_class(&self, name: &BinaryName) -> Result<Option<ClassResource>, ClassPathError> {
        let (bytes, format) = match name.as_internal() {
            "scala/io/Source" => (
                include_bytes!(
                    "../../dotty-tasty/tests/fixtures/scala3-library/scala/io/Source.tasty"
                )
                .to_vec(),
                ClassFormat::Tasty,
            ),
            "scala/package" => (
                include_bytes!(
                    "../../dotty-tasty/tests/fixtures/scala3-library/scala/package.tasty"
                )
                .to_vec(),
                ClassFormat::Tasty,
            ),
            "java/lang/Object" | "java/io/Closeable" | "scala/collection/Iterator" => {
                (audit_class_stub(name.as_internal()), ClassFormat::Class)
            }
            _ => return Ok(None),
        };
        Ok(Some(ClassResource::new(
            bytes,
            format,
            ClassOrigin::Directory("<scala-3.9.0-library-fixture>".into()),
        )))
    }

    fn contains_package(&self, package: &[&str]) -> Result<bool, ClassPathError> {
        Ok(package == ["scala"] || package == ["scala", "io"])
    }
}

fn audit_class_stub(internal_name: &str) -> Vec<u8> {
    let name = internal_name.as_bytes();
    let mut bytes = vec![0xCA, 0xFE, 0xBA, 0xBE, 0, 0, 0, 61, 0, 3, 1];
    bytes.extend_from_slice(&(name.len() as u16).to_be_bytes());
    bytes.extend_from_slice(name);
    bytes.extend_from_slice(&[7, 0, 1, 0, 0x21, 0, 2, 0, 0]);
    bytes.extend_from_slice(&[0; 8]);
    bytes
}

fn module_audit_resolver(
    store: &mut SemanticStore,
    include_object_class: bool,
    file: &str,
    method: &str,
) -> (Definitions, AuditResolver<ModuleAuditPath>) {
    let definitions = Definitions::bootstrap(store);
    (
        definitions,
        AuditResolver {
            inner: ClasspathSymbolResolver::new(
                ModuleAuditPath {
                    include_object_class,
                },
                definitions,
                LoadingSession::new(),
            )
            .with_load_profile_collection()
            .with_module_resolution_profile_collection(),
            metrics: Rc::new(RefCell::new(ResolverMetrics {
                current_file: file.to_owned(),
                current_method: method.to_owned(),
                ..ResolverMetrics::default()
            })),
            class_path_probe: None,
        },
    )
}

fn direct_module_resolver(
    store: &mut SemanticStore,
    include_object_class: bool,
) -> (Definitions, ClasspathSymbolResolver<ModuleAuditPath>) {
    let definitions = Definitions::bootstrap(store);
    (
        definitions,
        ClasspathSymbolResolver::new(
            ModuleAuditPath {
                include_object_class,
            },
            definitions,
            LoadingSession::new(),
        ),
    )
}

fn source_audit_resolver(
    store: &mut SemanticStore,
    file: &str,
    method: &str,
) -> (Definitions, AuditResolver<ScalaSourceAuditPath>) {
    let definitions = Definitions::bootstrap(store);
    (
        definitions,
        AuditResolver {
            inner: ClasspathSymbolResolver::new(
                ScalaSourceAuditPath,
                definitions,
                LoadingSession::new(),
            )
            .with_load_profile_collection()
            .with_module_resolution_profile_collection(),
            metrics: Rc::new(RefCell::new(ResolverMetrics {
                current_file: file.to_owned(),
                current_method: method.to_owned(),
                ..ResolverMetrics::default()
            })),
            class_path_probe: None,
        },
    )
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ScalaSourceModuleSnapshot {
    object: String,
    class: String,
    module_class: String,
    module_parent_types: Vec<String>,
    module_declaration_buckets: Vec<String>,
    module_info_shape: String,
    stdin: String,
}

fn symbol_type_shape(store: &SemanticStore, symbol: SymbolId, depth: u8) -> String {
    match store.symbols.get(symbol).info {
        dotty_core::SymbolInfo::Complete(ty) => semantic_type_shape(store, ty, depth + 1),
        dotty_core::SymbolInfo::Missing => "Missing".to_owned(),
        dotty_core::SymbolInfo::Deferred(_) => "Deferred".to_owned(),
        dotty_core::SymbolInfo::Error => "Error".to_owned(),
    }
}

fn declaration_bucket_shapes(
    store: &SemanticStore,
    scope: &dotty_core::Scope,
    depth: u8,
) -> Vec<String> {
    let mut buckets = BTreeMap::<(String, String), Vec<String>>::new();
    for symbol in scope.entered_symbols() {
        let entry = store.symbols.get(symbol);
        let name = store.names.resolve(entry.name.text()).to_owned();
        let key = (format!("{:?}", entry.name.namespace()), name);
        buckets.entry(key).or_default().push(format!(
            "kind={:?};owner={};flags={:?};visibility={:?};type={}",
            entry.kind,
            entry
                .owner
                .map(|owner| audit_symbol_path(store, owner))
                .unwrap_or_default(),
            entry.flags,
            entry.visibility,
            symbol_type_shape(store, symbol, depth)
        ));
    }
    buckets
        .into_iter()
        .map(|((namespace, name), mut declarations)| {
            declarations.sort();
            format!("{namespace}:{name}=[{}]", declarations.join(";"))
        })
        .collect()
}

fn class_info_components(
    store: &SemanticStore,
    info: &dotty_core::ClassInfo,
    depth: u8,
) -> (Vec<String>, Vec<String>) {
    let parents = info
        .parents
        .iter()
        .map(|parent| semantic_type_shape(store, *parent, depth + 1))
        .collect();
    let declarations =
        declaration_bucket_shapes(store, store.scopes.get(info.declarations), depth + 1);
    (parents, declarations)
}

fn semantic_type_shape(store: &SemanticStore, ty: dotty_core::TypeId, depth: u8) -> String {
    if depth >= 32 || !store.types.contains(ty) {
        return "<invalid-or-too-deep>".to_owned();
    }
    let nested = |ty| semantic_type_shape(store, ty, depth + 1);
    match store.types.get(ty) {
        Type::NoType => "NoType".to_owned(),
        Type::NoPrefix => "NoPrefix".to_owned(),
        Type::Error(_) => "Error".to_owned(),
        Type::TermRef { target, .. } => match target {
            dotty_core::TermRefTarget::Symbol(symbol) => {
                format!("TermRef({})", audit_symbol_path(store, *symbol))
            }
            dotty_core::TermRefTarget::Name(name) => {
                format!("TermRef({})", store.names.resolve(name.as_name().text()))
            }
        },
        Type::TypeRef { target, .. } => match target {
            TypeRefTarget::Symbol(symbol) => {
                format!("TypeRef({})", audit_symbol_path(store, *symbol))
            }
            TypeRefTarget::Name(name) => {
                format!("TypeRef({})", store.names.resolve(name.as_name().text()))
            }
        },
        Type::ThisType { class } => format!("This({})", audit_symbol_path(store, *class)),
        Type::Constant(constant) => format!("Constant({constant:?})"),
        Type::Applied { tycon, args } => format!(
            "{}[{}]",
            nested(*tycon),
            args.iter()
                .map(|arg| nested(*arg))
                .collect::<Vec<_>>()
                .join(",")
        ),
        Type::ByName { result } => format!("ByName({})", nested(*result)),
        Type::Flexible { underlying } => format!("Flexible({})", nested(*underlying)),
        Type::JavaArray { element } => format!("Array({})", nested(*element)),
        Type::Repeated { element } => format!("Repeated({})", nested(*element)),
        Type::Method(method) => format!(
            "Method({:?};[{}])->{}",
            method.kind,
            method
                .params
                .iter()
                .map(|param| nested(param.ty))
                .collect::<Vec<_>>()
                .join(","),
            nested(method.result)
        ),
        Type::Poly(poly) => format!("Poly({})->{}", poly.params.len(), nested(poly.result)),
        Type::TypeLambda(lambda) => {
            format!(
                "TypeLambda({})->{}",
                lambda.params.len(),
                nested(lambda.result)
            )
        }
        Type::Bounds { low, high } => format!("Bounds({}..{})", nested(*low), nested(*high)),
        Type::AliasingBounds { alias } => format!("Alias({})", nested(*alias)),
        Type::ClassInfo(info) => {
            let (parents, declarations) = class_info_components(store, info, depth + 1);
            format!(
                "ClassInfo({};parents=[{}];declarations=[{}])",
                audit_symbol_path(store, info.class),
                parents.join(","),
                declarations.join(",")
            )
        }
        Type::SuperType { .. } => "SuperType".to_owned(),
        Type::And { .. } => "And".to_owned(),
        Type::Or { .. } => "Or".to_owned(),
        Type::Refined { .. } => "Refined".to_owned(),
        Type::Recursive { .. } => "Recursive".to_owned(),
        Type::RecThis { .. } => "RecThis".to_owned(),
        Type::ParamRef { index, .. } => format!("ParamRef({index})"),
        Type::Match(_) => "Match".to_owned(),
        Type::MatchCase { .. } => "MatchCase".to_owned(),
        Type::Annotated { underlying, .. } => format!("Annotated({})", nested(*underlying)),
        Type::Wildcard { bounds } => format!("Wildcard({})", nested(*bounds)),
    }
}

fn scala_source_module_snapshot(
    store: &mut SemanticStore,
    definitions: Definitions,
    resolver: &mut impl SymbolResolver,
) -> (ScalaSourceModuleSnapshot, SymbolId) {
    let resolver: &mut dyn SymbolResolver = resolver;
    let package = resolver
        .resolve_package(store, &["scala", "io"])
        .unwrap()
        .expect("the checked-in Scala 3.9 fixture contains scala.io");
    let prefix = store
        .types
        .alloc(Type::type_ref(definitions.no_prefix, package));
    let source_term_name = dotty_core::Name::new(store.names.intern("Source"), Namespace::Term);
    let source_term = resolver
        .resolve_member(
            store,
            &MemberRequest {
                prefix,
                name: source_term_name,
                selector: MemberSelector::Unique,
                space: MemberSpace::Prefix,
            },
        )
        .unwrap()
        .expect("scala.io.Source term should resolve");
    let source_type_name = dotty_core::Name::new(store.names.intern("Source"), Namespace::Type);
    let source_type = resolver
        .resolve_member(
            store,
            &MemberRequest {
                prefix,
                name: source_type_name,
                selector: MemberSelector::Unique,
                space: MemberSpace::Prefix,
            },
        )
        .unwrap()
        .expect("scala.io.Source type should resolve");
    assert_eq!(
        store.symbols.get(source_term).kind,
        dotty_core::SymbolKind::Object
    );
    assert!(matches!(
        store.symbols.get(source_type).kind,
        dotty_core::SymbolKind::Class | dotty_core::SymbolKind::Trait
    ));
    assert_ne!(
        source_term, source_type,
        "term and type namespaces stay distinct"
    );

    let module_class = resolver
        .module_class_of_object(store, source_term)
        .unwrap()
        .expect("Source object has a canonical module class");
    assert_eq!(
        store.symbols.get(source_term).links.companion,
        Some(source_type),
        "the package term must retain the canonical companion class"
    );
    assert_eq!(
        store.symbols.get(module_class).kind,
        dotty_core::SymbolKind::ModuleClass
    );
    assert!(resolver.complete_module_class(store, module_class).unwrap());
    assert!(valid_module_class_info(store, module_class));
    let source_value_type = match store.symbols.get(source_term).info {
        dotty_core::SymbolInfo::Complete(ty) => store.types.get(ty),
        _ => panic!("the Source object must carry its canonical value type"),
    };
    assert!(matches!(
        source_value_type,
        Type::TypeRef {
            target: TypeRefTarget::Symbol(target),
            ..
        } | Type::TermRef {
            target: dotty_core::TermRefTarget::Symbol(target),
            ..
        } if *target == module_class
    ));
    let completed_checkpoint = store.checkpoint();
    assert!(resolver.complete_module_class(store, module_class).unwrap());
    assert_eq!(
        store.checkpoint(),
        completed_checkpoint,
        "repeated completion must not allocate duplicate module state"
    );
    let dotty_core::SymbolInfo::Complete(info_type) = store.symbols.get(module_class).info else {
        unreachable!("completion check above establishes complete module information");
    };
    let Type::ClassInfo(info) = store.types.get(info_type) else {
        unreachable!("valid module class info is a ClassInfo");
    };
    let (module_parent_types, module_declaration_buckets) = class_info_components(store, info, 0);
    let module_info_shape = semantic_type_shape(store, info_type, 0);

    let module_prefix = store.types.alloc(Type::TermRef {
        prefix,
        target: dotty_core::TermRefTarget::Symbol(source_term),
    });
    let stdin_name = dotty_core::Name::new(store.names.intern("stdin"), Namespace::Term);
    let stdin = resolver
        .resolve_member(
            store,
            &MemberRequest {
                prefix: module_prefix,
                name: stdin_name,
                selector: MemberSelector::Unique,
                space: MemberSpace::Prefix,
            },
        )
        .unwrap()
        .expect("the real Source module declares stdin");
    let missing_name = dotty_core::Name::new(store.names.intern("noSuchMember"), Namespace::Term);
    let missing_checkpoint = store.checkpoint();
    assert_eq!(
        resolver.resolve_member(
            store,
            &MemberRequest {
                prefix: module_prefix,
                name: missing_name,
                selector: MemberSelector::Unique,
                space: MemberSpace::Prefix,
            },
        ),
        Ok(None),
        "a missing Source member stays unresolved"
    );
    assert_eq!(
        store.checkpoint(),
        missing_checkpoint,
        "missing lookup is atomic"
    );

    let describe = |symbol: SymbolId| {
        let entry = store.symbols.get(symbol);
        let semantic_type = match entry.info {
            dotty_core::SymbolInfo::Complete(ty) => semantic_type_shape(store, ty, 0),
            dotty_core::SymbolInfo::Missing => "Missing".to_owned(),
            dotty_core::SymbolInfo::Deferred(_) => "Deferred".to_owned(),
            dotty_core::SymbolInfo::Error => "Error".to_owned(),
        };
        format!(
            "{}|{:?}|{:?}|{}|{:?}|{}",
            audit_symbol_path(store, symbol),
            entry.kind,
            entry.name.namespace(),
            symbol_info_label(entry.info),
            entry.origin,
            semantic_type
        )
    };
    (
        ScalaSourceModuleSnapshot {
            object: describe(source_term),
            class: describe(source_type),
            module_class: describe(module_class),
            module_parent_types,
            module_declaration_buckets,
            module_info_shape,
            stdin: describe(stdin),
        },
        module_class,
    )
}

#[test]
fn audit_and_classpath_resolvers_match_for_real_scala_source_module() {
    let mut direct_store = SemanticStore::new();
    let direct_definitions = Definitions::bootstrap(&mut direct_store);
    let mut direct = ClasspathSymbolResolver::new(
        ScalaSourceAuditPath,
        direct_definitions,
        LoadingSession::new(),
    );
    let (expected, direct_module_class) =
        scala_source_module_snapshot(&mut direct_store, direct_definitions, &mut direct);

    let mut audit_store = SemanticStore::new();
    let (audit_definitions, mut audit) =
        source_audit_resolver(&mut audit_store, "scala.io.Source.scala", "Source.stdin");
    let (actual, audit_module_class) =
        scala_source_module_snapshot(&mut audit_store, audit_definitions, &mut audit);

    assert_eq!(
        actual, expected,
        "the audit wrapper must preserve production semantics"
    );

    let dotty_core::SymbolInfo::Complete(class_info_type) =
        direct_store.symbols.get(direct_module_class).info
    else {
        unreachable!("the direct Source module was completed");
    };
    let Type::ClassInfo(class_info) = direct_store.types.get(class_info_type).clone() else {
        unreachable!("completed Source module carries ClassInfo");
    };
    assert!(!class_info.parents.is_empty());
    let mut changed_parent = class_info.clone();
    changed_parent.parents[0] = direct_definitions.int;
    let changed_parent_type = direct_store.types.alloc(Type::ClassInfo(changed_parent));
    assert_ne!(
        semantic_type_shape(&direct_store, changed_parent_type, 0),
        expected.module_info_shape,
        "normalized ClassInfo must observe changed parent types"
    );

    let declared_members = direct_store
        .scopes
        .get(class_info.declarations)
        .entered_symbols()
        .collect::<Vec<_>>();
    assert!(declared_members.len() > 1);
    let mut changed_scope = dotty_core::Scope::new(Some(direct_module_class));
    for symbol in declared_members.iter().skip(1) {
        changed_scope.enter(direct_store.symbols.get(*symbol).name, *symbol);
    }
    let changed_scope_id = direct_store.scopes.alloc(changed_scope);
    let mut changed_declarations = class_info;
    changed_declarations.declarations = changed_scope_id;
    let changed_declarations_type = direct_store
        .types
        .alloc(Type::ClassInfo(changed_declarations));
    assert_ne!(
        semantic_type_shape(&direct_store, changed_declarations_type, 0),
        expected.module_info_shape,
        "normalized ClassInfo must observe changed declaration buckets"
    );

    let audit_metrics = audit.metrics.borrow();
    let observations = &audit_metrics.module_operation_observations;
    let identity_lookups = observations
        .iter()
        .filter(|observation| observation.operation == "module_class_of_object")
        .collect::<Vec<_>>();
    assert_eq!(identity_lookups.len(), 1);
    assert!(identity_lookups.iter().all(|observation| {
        observation.result == "resolved"
            && observation.source_file == "scala.io.Source.scala"
            && observation.enclosing_attempt == "Source.stdin"
    }));
    let completions = observations
        .iter()
        .filter(|observation| observation.operation == "complete_module_class")
        .collect::<Vec<_>>();
    assert_eq!(completions.len(), 2, "one observation is recorded per call");
    assert!(completions.iter().all(|observation| {
        observation.result == "completed"
            && observation.source_file == "scala.io.Source.scala"
            && observation.enclosing_attempt == "Source.stdin"
    }));
    let members = audit_metrics.module_member_observations.clone();
    assert_eq!(members.len(), 2);
    assert!(members.iter().all(|observation| {
        observation.source_file == "scala.io.Source.scala"
            && observation.enclosing_attempt == "Source.stdin"
            && observation.module_class_identity
                == audit_symbol_path(&audit_store, audit_module_class)
            && observation.module_state_before == "Complete(ClassInfo)"
            && observation.module_state_after == "Complete(ClassInfo)"
            && observation.transaction_state == "observed"
    }));
    assert!(members.iter().any(|observation| {
        observation.request_key.contains("name=stdin::") && observation.result == "success"
    }));
    assert!(members.iter().any(|observation| {
        observation.request_key.contains("name=noSuchMember::")
            && observation.result == "unresolved"
    }));
}

#[test]
fn source_expression_failure_rolls_back_module_before_successful_member_retry() {
    let text = "object Audit { def use: Int = { scala.io.Source.stdin; true } }";
    let source = SourceId::from_index(0);
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let scanner = ContextualScanner::new(text).unwrap();
    let parsed = parse_compilation_unit(
        SourceText::new(text).unwrap(),
        source,
        scanner,
        &mut store.names,
    );
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let mut namer_packages = Packages::new();
    let index = name_compilation_unit(
        &parsed.ast,
        parsed.root,
        source,
        "module-transaction.scala",
        &mut store,
        &mut namer_packages,
    )
    .unwrap();
    let (rhs, method) = parsed
        .ast
        .iter()
        .find_map(|(tree, node)| {
            let TreeKind::DefDef(definition) = &node.kind else {
                return None;
            };
            Some((definition.rhs?, index.symbol_at(source, tree)?))
        })
        .expect("fixture contains one method body");
    let stdin_selection = parsed
        .ast
        .iter()
        .find_map(|(tree, node)| match &node.kind {
            TreeKind::Select(select) if store.names.resolve(select.name.text()) == "stdin" => {
                Some(tree)
            }
            _ => None,
        })
        .expect("fixture contains the real Source.stdin selection");
    let metrics = Rc::new(RefCell::new(ResolverMetrics {
        current_file: "module-transaction.scala".to_owned(),
        current_method: "Audit.use".to_owned(),
        ..ResolverMetrics::default()
    }));
    let resolver = AuditResolver {
        inner: ClasspathSymbolResolver::new(
            ScalaSourceAuditPath,
            definitions,
            LoadingSession::with_packages(namer_packages),
        )
        .with_load_profile_collection()
        .with_module_resolution_profile_collection(),
        metrics: Rc::clone(&metrics),
        class_path_probe: None,
    };
    let typer_packages = Packages::new();
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &typer_packages,
    )
    .with_resolver(Box::new(resolver));
    let context = typer.expression_context_for(method).unwrap();
    let store_before = typer.store().checkpoint();
    let typed_before = typer.typed_ast().checkpoint();

    let error = typer
        .type_expression_expected(rhs, context, definitions.int)
        .expect_err("the final Boolean should fail the enclosing Int conformance check");
    assert!(matches!(
        error,
        TyperError::ExpectedExpressionTypeMismatch {
            source: error_source,
            tree_index,
            actual,
            expected,
        } if error_source == source
            && tree_index == rhs.index()
            && actual == definitions.boolean
            && expected == definitions.int
    ));
    assert_eq!(typer.store().checkpoint(), store_before);
    assert_eq!(typer.typed_ast().checkpoint(), typed_before);
    assert_eq!(typer.source_type_index().type_at(source, rhs), None);
    assert_eq!(
        typer.source_typed_index().get(source, stdin_selection),
        None
    );

    let typed_stdin = typer
        .type_expression(stdin_selection, context)
        .expect("Source.stdin must type through SourceTyper and AuditResolver");
    let Type::TermRef {
        target: TermRefTarget::Symbol(stdin),
        ..
    } = typer
        .store()
        .types
        .get(typer.typed_ast().get(typed_stdin).ty)
    else {
        panic!("Source.stdin should preserve the canonical member symbol");
    };
    let stdin = *stdin;
    let stdin_entry = typer.store().symbols.get(stdin);
    assert_eq!(
        typer.store().names.resolve(stdin_entry.name.text()),
        "stdin"
    );
    assert_eq!(
        audit_symbol_path(typer.store(), stdin_entry.owner.unwrap()),
        "scala.io.Source$"
    );
    assert_eq!(
        typer.source_typed_index().get(source, stdin_selection),
        Some(typed_stdin)
    );

    let metrics = metrics.borrow();
    let source_module_completions = metrics
        .module_operation_observations
        .iter()
        .filter(|observation| {
            observation.operation == "complete_module_class"
                && observation
                    .module_class_identity
                    .contains("scala/io/Source")
        })
        .collect::<Vec<_>>();
    assert_eq!(source_module_completions.len(), 2);
    assert!(source_module_completions.iter().all(|observation| {
        observation.result == "completed"
            && observation.result_state == "Complete(ClassInfo)"
            && observation.module_state == "Complete(ClassInfo)"
    }));
    assert_eq!(
        source_module_completions[0].transaction_state,
        "rolled_back"
    );
    assert_eq!(source_module_completions[1].transaction_state, "observed");
}

fn module_audit_object(
    store: &mut SemanticStore,
    definitions: Definitions,
    resolver: &mut dyn SymbolResolver,
) -> SymbolId {
    let package = resolver
        .resolve_package(store, &["me", "cytrowski", "tastyfixtures", "semantic"])
        .unwrap()
        .unwrap();
    let prefix = store
        .types
        .alloc(Type::type_ref(definitions.no_prefix, package));
    let name = dotty_core::Name::new(store.names.intern("Both"), Namespace::Term);
    resolver
        .resolve_member(
            store,
            &MemberRequest {
                prefix,
                name,
                selector: MemberSelector::Unique,
                space: MemberSpace::Prefix,
            },
        )
        .unwrap()
        .unwrap()
}

#[test]
fn audit_resolver_forwards_authoritative_module_identity_lookup() {
    let mut store = SemanticStore::new();
    let (definitions, mut resolver) =
        module_audit_resolver(&mut store, true, "fixture.scala", "fixture()");
    let package = resolver
        .resolve_package(
            &mut store,
            &["me", "cytrowski", "tastyfixtures", "semantic"],
        )
        .unwrap()
        .unwrap();
    let prefix = store
        .types
        .alloc(Type::type_ref(definitions.no_prefix, package));
    let request = MemberRequest {
        prefix,
        name: dotty_core::Name::new(store.names.intern("Both"), Namespace::Term),
        selector: MemberSelector::Unique,
        space: MemberSpace::Prefix,
    };
    let store_checkpoint = store.checkpoint();
    let resolver_checkpoint = resolver.checkpoint();
    let object = resolver
        .resolve_member(&mut store, &request)
        .unwrap()
        .unwrap();

    // Keep these calls behind the port: concrete-method calls would not catch
    // a future AuditResolver implementation that silently inherits defaults.
    let direct = {
        let direct: &mut dyn SymbolResolver = &mut resolver.inner;
        direct.module_class_of_object(&store, object)
    };
    assert!(direct.as_ref().unwrap().is_some());
    assert_eq!(
        module_class_of_object_through_port(&mut resolver, &store, object),
        direct,
        "the audit adapter must return the authoritative identity unchanged"
    );

    let unowned = definitions.object_class;
    assert_eq!(
        module_class_of_object_through_port(&mut resolver, &store, unowned),
        module_class_of_object_through_port(&mut resolver.inner, &store, unowned)
    );
    resolver.rollback_to(&mut store, resolver_checkpoint);
    store.rollback_to(store_checkpoint);
    let unknown = object;
    assert!(!store.symbols.contains(unknown));
    assert_eq!(
        module_class_of_object_through_port(&mut resolver, &store, unknown),
        module_class_of_object_through_port(&mut resolver.inner, &store, unknown)
    );
}

fn module_class_of_object_through_port(
    resolver: &mut impl SymbolResolver,
    store: &SemanticStore,
    object: SymbolId,
) -> Result<Option<SymbolId>, ResolutionError> {
    let resolver: &mut dyn SymbolResolver = resolver;
    resolver.module_class_of_object(store, object)
}

fn complete_module_class_through_port(
    resolver: &mut impl SymbolResolver,
    store: &mut SemanticStore,
    module_class: SymbolId,
) -> Result<bool, ResolutionError> {
    let resolver: &mut dyn SymbolResolver = resolver;
    resolver.complete_module_class(store, module_class)
}

#[test]
fn audit_resolver_forwards_module_completion_and_preserves_caller_rollback() {
    let mut store = SemanticStore::new();
    let (definitions, mut resolver) =
        module_audit_resolver(&mut store, true, "fixture.scala", "fixture()");
    let object = module_audit_object(&mut store, definitions, &mut resolver);
    let module_class = resolver
        .module_class_of_object(&store, object)
        .unwrap()
        .unwrap();
    assert_eq!(
        store.symbols.get(module_class).info,
        dotty_core::SymbolInfo::Missing
    );

    let store_checkpoint = store.checkpoint();
    let resolver_checkpoint = resolver.checkpoint();
    assert_eq!(
        complete_module_class_through_port(&mut resolver.inner, &mut store, module_class),
        Ok(true),
        "the direct resolver establishes the expected completion result"
    );
    assert!(valid_module_class_info(&store, module_class));
    resolver.rollback_to(&mut store, resolver_checkpoint);
    store.rollback_to(store_checkpoint);
    assert_eq!(
        store.symbols.get(module_class).info,
        dotty_core::SymbolInfo::Missing
    );

    assert_eq!(
        complete_module_class_through_port(&mut resolver, &mut store, module_class),
        Ok(true)
    );
    assert!(valid_module_class_info(&store, module_class));
    let completed_checkpoint = store.checkpoint();
    assert_eq!(
        complete_module_class_through_port(&mut resolver, &mut store, module_class),
        Ok(true)
    );
    assert_eq!(store.checkpoint(), completed_checkpoint);

    resolver.rollback_to(&mut store, resolver_checkpoint);
    store.rollback_to(store_checkpoint);
    assert_eq!(
        store.symbols.get(module_class).info,
        dotty_core::SymbolInfo::Missing,
        "caller rollback restores the preexisting module shell"
    );
}

#[test]
fn audit_resolver_rolls_back_a_newly_created_module_identity_pair() {
    let mut store = SemanticStore::new();
    let (definitions, mut resolver) =
        module_audit_resolver(&mut store, true, "fixture.scala", "fixture()");
    let package = resolver
        .resolve_package(
            &mut store,
            &["me", "cytrowski", "tastyfixtures", "semantic"],
        )
        .unwrap()
        .unwrap();
    let prefix = store
        .types
        .alloc(Type::type_ref(definitions.no_prefix, package));
    let request = MemberRequest {
        prefix,
        name: dotty_core::Name::new(store.names.intern("Both"), Namespace::Term),
        selector: MemberSelector::Unique,
        space: MemberSpace::Prefix,
    };
    let store_checkpoint = store.checkpoint();
    let resolver_checkpoint = resolver.checkpoint();

    let object = resolver
        .resolve_member(&mut store, &request)
        .unwrap()
        .unwrap();
    let module_class = resolver
        .module_class_of_object(&store, object)
        .unwrap()
        .unwrap();
    resolver.rollback_to(&mut store, resolver_checkpoint);
    store.rollback_to(store_checkpoint);

    assert_eq!(store.checkpoint(), store_checkpoint);
    assert!(!store.symbols.contains(object));
    assert!(!store.symbols.contains(module_class));
}

#[test]
fn audit_module_completion_failure_is_atomic() {
    let mut store = SemanticStore::new();
    let (definitions, mut resolver) =
        module_audit_resolver(&mut store, false, "fixture.scala", "fixture()");
    let object = module_audit_object(&mut store, definitions, &mut resolver);
    let module_class = resolver
        .module_class_of_object(&store, object)
        .unwrap()
        .unwrap();
    let checkpoint = store.checkpoint();

    assert!(
        resolver
            .complete_module_class(&mut store, module_class)
            .is_err()
    );
    assert_eq!(store.checkpoint(), checkpoint);
    assert_eq!(
        store.symbols.get(module_class).info,
        dotty_core::SymbolInfo::Missing
    );
    let observations = &resolver.metrics.borrow().module_operation_observations;
    assert_eq!(observations.len(), 2);
    assert_eq!(observations[1].operation, "complete_module_class");
    assert_eq!(observations[1].result, "error");
    assert_eq!(observations[1].result_state, "Missing");
    assert_eq!(observations[1].error_category, Some("malformed"));
}

#[test]
fn audit_module_completion_records_an_unowned_false_result() {
    let mut store = SemanticStore::new();
    let (definitions, mut direct) = direct_module_resolver(&mut store, true);
    let object = module_audit_object(&mut store, definitions, &mut direct);
    let module_class = direct
        .module_class_of_object(&store, object)
        .unwrap()
        .unwrap();

    let metrics = Rc::new(RefCell::new(ResolverMetrics {
        current_file: "fixture.scala".to_owned(),
        current_method: "unowned-module-completion()".to_owned(),
        ..ResolverMetrics::default()
    }));
    let mut audit = AuditResolver {
        inner: ClasspathSymbolResolver::new(
            ModuleAuditPath {
                include_object_class: true,
            },
            definitions,
            LoadingSession::new(),
        )
        .with_load_profile_collection(),
        metrics: Rc::clone(&metrics),
        class_path_probe: None,
    };
    let checkpoint = store.checkpoint();
    assert_eq!(
        audit.complete_module_class(&mut store, module_class),
        Ok(false)
    );
    assert_eq!(store.checkpoint(), checkpoint);

    let metrics = metrics.borrow();
    assert_eq!(metrics.module_completion_resolver_calls, 1);
    assert_eq!(
        metrics
            .module_completion_resolver_outcomes
            .get("unsupported"),
        Some(&1)
    );
    let [observation] = metrics.module_operation_observations.as_slice() else {
        panic!("the attempted unsupported completion should be retained");
    };
    assert_eq!(observation.operation, "complete_module_class");
    assert_eq!(observation.result, "unsupported");
    assert_eq!(observation.result_state, "Missing");
    assert_eq!(observation.transaction_state, "observed");
}

#[derive(Debug, PartialEq, Eq)]
struct ModuleFailureSnapshot {
    unowned_result: String,
    unknown_error: String,
    malformed_error: String,
    malformed_owner_error: String,
    wrong_kind_result: String,
    missing_member: bool,
}

fn module_failure_snapshot(
    store: &mut SemanticStore,
    definitions: Definitions,
    resolver: &mut impl SymbolResolver,
) -> ModuleFailureSnapshot {
    let resolver: &mut dyn SymbolResolver = resolver;
    let object = module_audit_object(store, definitions, resolver);
    let orphan_object = store.symbols.alloc(store.symbols.get(object).clone());
    let unowned_result = match resolver.module_class_of_object(store, orphan_object) {
        Ok(None) => "unresolved".to_owned(),
        Ok(Some(module_class)) => format!("resolved:{}", audit_symbol_path(store, module_class)),
        Err(error) => format!("error:{}", resolution_error_category(&error)),
    };

    let symbol_checkpoint = store.checkpoint();
    let resolver_checkpoint = resolver.checkpoint();
    let unknown = store.symbols.alloc(store.symbols.get(object).clone());
    resolver.rollback_to(store, resolver_checkpoint);
    store.rollback_to(symbol_checkpoint);
    let unknown_error = resolver
        .module_class_of_object(store, unknown)
        .expect_err("an unknown symbol ID must not be accepted");
    let unknown_error = resolution_error_category(&unknown_error).to_owned();

    let original_name = store.symbols.get(object).name;
    let wrong_name = dotty_core::Name::new(store.names.intern("WrongObjectName"), Namespace::Term);
    store.symbols.get_mut(object).name = wrong_name;
    let malformed_checkpoint = store.checkpoint();
    let malformed_error = resolver
        .module_class_of_object(store, object)
        .expect_err("an object whose name disagrees with module identity must be rejected");
    let malformed_error = resolution_error_category(&malformed_error).to_owned();
    assert_eq!(store.checkpoint(), malformed_checkpoint);
    store.symbols.get_mut(object).name = original_name;

    let original_owner = store.symbols.get(object).owner;
    store.symbols.get_mut(object).owner = Some(definitions.object_class);
    let malformed_owner_checkpoint = store.checkpoint();
    let malformed_owner_error = resolver
        .module_class_of_object(store, object)
        .expect_err("a module object with an incompatible owner must be rejected");
    let malformed_owner_error = resolution_error_category(&malformed_owner_error).to_owned();
    assert_eq!(store.checkpoint(), malformed_owner_checkpoint);
    store.symbols.get_mut(object).owner = original_owner;

    let original_kind = store.symbols.get(object).kind;
    store.symbols.get_mut(object).kind = dotty_core::SymbolKind::Class;
    let wrong_kind_result = match resolver.module_class_of_object(store, object) {
        Ok(None) => "unresolved".to_owned(),
        Ok(Some(module_class)) => format!("resolved:{}", audit_symbol_path(store, module_class)),
        Err(error) => format!("error:{}", resolution_error_category(&error)),
    };
    store.symbols.get_mut(object).kind = original_kind;

    let module_class = resolver
        .module_class_of_object(store, object)
        .unwrap()
        .expect("restoring the object identity restores module lookup");
    assert!(resolver.complete_module_class(store, module_class).unwrap());
    let prefix = store
        .types
        .alloc(Type::type_ref(definitions.no_prefix, module_class));
    let missing_checkpoint = store.checkpoint();
    let missing_name = dotty_core::Name::new(store.names.intern("notDeclared"), Namespace::Term);
    let missing_member = resolver
        .resolve_member(
            store,
            &MemberRequest {
                prefix,
                name: missing_name,
                selector: MemberSelector::Unique,
                space: MemberSpace::Prefix,
            },
        )
        .unwrap()
        .is_none();
    assert!(missing_member);
    assert_eq!(store.checkpoint(), missing_checkpoint);

    ModuleFailureSnapshot {
        unowned_result,
        unknown_error,
        malformed_error,
        malformed_owner_error,
        wrong_kind_result,
        missing_member,
    }
}

#[test]
fn audit_and_classpath_resolvers_match_for_module_failures_and_atomicity() {
    let mut direct_store = SemanticStore::new();
    let (direct_definitions, mut direct) = direct_module_resolver(&mut direct_store, true);
    let expected = module_failure_snapshot(&mut direct_store, direct_definitions, &mut direct);

    let mut audit_store = SemanticStore::new();
    let (audit_definitions, mut audit) =
        module_audit_resolver(&mut audit_store, true, "fixture.scala", "failure-parity()");
    let actual = module_failure_snapshot(&mut audit_store, audit_definitions, &mut audit);

    assert_eq!(actual, expected);
    assert_eq!(actual.unowned_result, "unresolved");
    assert_eq!(actual.unknown_error, "malformed");
    assert_eq!(actual.malformed_error, "malformed");
    assert_eq!(actual.malformed_owner_error, "malformed");
    assert_eq!(actual.wrong_kind_result, "unresolved");

    let observations = &audit.metrics.borrow().module_operation_observations;
    assert!(observations.iter().any(|observation| {
        observation.result == "error" && observation.error_category == Some("malformed")
    }));
    assert!(observations.iter().any(|observation| {
        observation.operation == "module_class_of_object"
            && observation.result == "unresolved"
            && observation.error_category.is_none()
    }));
    assert!(observations.iter().all(|observation| {
        observation.result != "resolved" || observation.result_state == "validated_module_class"
    }));
}

fn new_module_pair_rollback_snapshot(
    store: &mut SemanticStore,
    definitions: Definitions,
    resolver: &mut dyn SymbolResolver,
) -> String {
    let package = resolver
        .resolve_package(store, &["me", "cytrowski", "tastyfixtures", "semantic"])
        .unwrap()
        .unwrap();
    let prefix = store
        .types
        .alloc(Type::type_ref(definitions.no_prefix, package));
    let object_name = dotty_core::Name::new(store.names.intern("Both"), Namespace::Term);
    let request = MemberRequest {
        prefix,
        name: object_name,
        selector: MemberSelector::Unique,
        space: MemberSpace::Prefix,
    };
    let store_checkpoint = store.checkpoint();
    let resolver_checkpoint = resolver.checkpoint();
    let first_object = resolver
        .resolve_member(store, &request)
        .unwrap()
        .expect("Both object should be created inside the transaction");
    let first_module_class = resolver
        .module_class_of_object(store, first_object)
        .unwrap()
        .unwrap();
    resolver.rollback_to(store, resolver_checkpoint);
    store.rollback_to(store_checkpoint);
    assert_eq!(store.checkpoint(), store_checkpoint);
    assert!(!store.symbols.contains(first_object));
    assert!(!store.symbols.contains(first_module_class));

    let object = resolver
        .resolve_member(store, &request)
        .unwrap()
        .expect("retry should create the canonical Object");
    let module_class = resolver
        .module_class_of_object(store, object)
        .unwrap()
        .unwrap();
    let repeated_object = resolver.resolve_member(store, &request).unwrap().unwrap();
    let repeated_module_class = resolver
        .module_class_of_object(store, repeated_object)
        .unwrap()
        .unwrap();
    assert_eq!(object, repeated_object);
    assert_eq!(module_class, repeated_module_class);
    format!(
        "{}|{}|{:?}|{:?}",
        audit_symbol_path(store, object),
        audit_symbol_path(store, module_class),
        store.symbols.get(object).kind,
        store.symbols.get(module_class).kind
    )
}

fn existing_module_shell_rollback_snapshot(
    store: &mut SemanticStore,
    definitions: Definitions,
    resolver: &mut dyn SymbolResolver,
) -> String {
    let object = module_audit_object(store, definitions, resolver);
    let module_class = resolver
        .module_class_of_object(store, object)
        .unwrap()
        .unwrap();
    assert_eq!(
        store.symbols.get(module_class).info,
        dotty_core::SymbolInfo::Missing
    );
    let store_checkpoint = store.checkpoint();
    let resolver_checkpoint = resolver.checkpoint();
    assert!(resolver.complete_module_class(store, module_class).unwrap());
    assert!(valid_module_class_info(store, module_class));
    resolver.rollback_to(store, resolver_checkpoint);
    store.rollback_to(store_checkpoint);
    assert_eq!(store.checkpoint(), store_checkpoint);
    assert_eq!(
        store.symbols.get(module_class).info,
        dotty_core::SymbolInfo::Missing,
        "rollback restores the existing module shell"
    );
    assert!(resolver.complete_module_class(store, module_class).unwrap());
    let completed_checkpoint = store.checkpoint();
    assert!(resolver.complete_module_class(store, module_class).unwrap());
    assert_eq!(store.checkpoint(), completed_checkpoint);
    format!(
        "{}|{}|{:?}",
        audit_symbol_path(store, object),
        audit_symbol_path(store, module_class),
        store.symbols.get(module_class).kind
    )
}

fn existing_module_member_rollback_snapshot(
    store: &mut SemanticStore,
    definitions: Definitions,
    resolver: &mut dyn SymbolResolver,
) -> String {
    let object = module_audit_object(store, definitions, resolver);
    let module_class = resolver
        .module_class_of_object(store, object)
        .unwrap()
        .unwrap();
    assert_eq!(
        store.symbols.get(module_class).info,
        dotty_core::SymbolInfo::Missing
    );

    let package = store.symbols.get(object).owner.unwrap();
    let package_prefix = store
        .types
        .alloc(Type::type_ref(definitions.no_prefix, package));
    let object_prefix = store.types.alloc(Type::TermRef {
        prefix: package_prefix,
        target: dotty_core::TermRefTarget::Symbol(object),
    });
    let make_request = MemberRequest {
        prefix: object_prefix,
        name: dotty_core::Name::new(store.names.intern("make"), Namespace::Term),
        selector: MemberSelector::Unique,
        space: MemberSpace::Prefix,
    };

    let store_checkpoint = store.checkpoint();
    let resolver_checkpoint = resolver.checkpoint();
    assert!(resolver.complete_module_class(store, module_class).unwrap());
    let first_member = resolver
        .resolve_member(store, &make_request)
        .unwrap()
        .expect("Both.make should resolve from the completed module");
    assert_eq!(store.symbols.get(first_member).owner, Some(module_class));
    let completed_checkpoint = store.checkpoint();
    assert_eq!(
        resolver.resolve_member(store, &make_request),
        Ok(Some(first_member)),
        "a repeated member lookup reuses its canonical symbol"
    );
    assert_eq!(store.checkpoint(), completed_checkpoint);

    resolver.rollback_to(store, resolver_checkpoint);
    store.rollback_to(store_checkpoint);
    assert_eq!(store.checkpoint(), store_checkpoint);
    assert_eq!(
        store.symbols.get(module_class).info,
        dotty_core::SymbolInfo::Missing,
        "rollback restores the existing module shell"
    );
    assert!(!store.symbols.contains(first_member));
    assert_eq!(
        resolver.module_class_of_object(store, object),
        Ok(Some(module_class))
    );

    assert!(resolver.complete_module_class(store, module_class).unwrap());
    let retried_member = resolver
        .resolve_member(store, &make_request)
        .unwrap()
        .expect("the same module member should resolve after rollback");
    assert_eq!(store.symbols.get(retried_member).owner, Some(module_class));
    let retried_checkpoint = store.checkpoint();
    assert_eq!(
        resolver.resolve_member(store, &make_request),
        Ok(Some(retried_member)),
        "a successful retry is stable and idempotent"
    );
    assert_eq!(store.checkpoint(), retried_checkpoint);
    let entry = store.symbols.get(retried_member);
    format!(
        "{}|{}|{:?}|{}",
        audit_symbol_path(store, object),
        audit_symbol_path(store, module_class),
        entry.kind,
        audit_symbol_path(store, entry.owner.unwrap())
    )
}

#[test]
fn audit_and_classpath_resolvers_match_for_module_rollbacks_and_retry() {
    let mut direct_new_store = SemanticStore::new();
    let (direct_new_definitions, mut direct_new) =
        direct_module_resolver(&mut direct_new_store, true);
    let expected_new = new_module_pair_rollback_snapshot(
        &mut direct_new_store,
        direct_new_definitions,
        &mut direct_new,
    );

    let mut audit_new_store = SemanticStore::new();
    let (audit_new_definitions, mut audit_new) = module_audit_resolver(
        &mut audit_new_store,
        true,
        "fixture.scala",
        "new-pair-rollback()",
    );
    let actual_new = new_module_pair_rollback_snapshot(
        &mut audit_new_store,
        audit_new_definitions,
        &mut audit_new,
    );
    assert_eq!(actual_new, expected_new);
    assert!(
        audit_new
            .metrics
            .borrow()
            .module_operation_observations
            .iter()
            .any(
                |observation| observation.operation == "module_class_of_object"
                    && observation.result == "resolved"
            )
    );

    let mut direct_existing_store = SemanticStore::new();
    let (direct_existing_definitions, mut direct_existing) =
        direct_module_resolver(&mut direct_existing_store, true);
    let expected_existing = existing_module_shell_rollback_snapshot(
        &mut direct_existing_store,
        direct_existing_definitions,
        &mut direct_existing,
    );

    let mut audit_existing_store = SemanticStore::new();
    let (audit_existing_definitions, mut audit_existing) = module_audit_resolver(
        &mut audit_existing_store,
        true,
        "fixture.scala",
        "existing-shell-rollback()",
    );
    let actual_existing = existing_module_shell_rollback_snapshot(
        &mut audit_existing_store,
        audit_existing_definitions,
        &mut audit_existing,
    );
    assert_eq!(actual_existing, expected_existing);
    let completion_observations = audit_existing
        .metrics
        .borrow()
        .module_operation_observations
        .iter()
        .filter(|observation| observation.operation == "complete_module_class")
        .count();
    assert_eq!(completion_observations, 3);

    let mut direct_member_store = SemanticStore::new();
    let (direct_member_definitions, mut direct_member) =
        direct_module_resolver(&mut direct_member_store, true);
    let expected_member = existing_module_member_rollback_snapshot(
        &mut direct_member_store,
        direct_member_definitions,
        &mut direct_member,
    );

    let mut audit_member_store = SemanticStore::new();
    let (audit_member_definitions, mut audit_member) = module_audit_resolver(
        &mut audit_member_store,
        true,
        "fixture.scala",
        "existing-shell-member-rollback()",
    );
    let actual_member = existing_module_member_rollback_snapshot(
        &mut audit_member_store,
        audit_member_definitions,
        &mut audit_member,
    );
    assert_eq!(actual_member, expected_member);
    let member_observations = audit_member.metrics.borrow();
    let module_members = &member_observations.module_member_observations;
    assert_eq!(module_members.len(), 4);
    assert!(
        module_members[..2]
            .iter()
            .all(|observation| observation.transaction_state == "rolled_back")
    );
    assert!(
        module_members[2..]
            .iter()
            .all(|observation| observation.transaction_state == "observed")
    );
    assert!(module_members.iter().all(|observation| {
        observation.request_key.contains("name=make::")
            && observation.result == "success"
            && observation.module_state_after == "Complete(ClassInfo)"
    }));
}

#[test]
fn audit_module_operations_keep_stable_attribution_and_deduplicate_completion() {
    let mut store = SemanticStore::new();
    let (definitions, mut resolver) =
        module_audit_resolver(&mut store, true, "src/Fixture.scala", "Fixture.run()");
    let object = module_audit_object(&mut store, definitions, &mut resolver);
    let module_class = resolver
        .module_class_of_object(&store, object)
        .unwrap()
        .unwrap();
    let module_prefix = store
        .types
        .alloc(Type::type_ref(definitions.no_prefix, module_class));
    let make = dotty_core::Name::new(store.names.intern("make"), Namespace::Term);
    let member_request = MemberRequest {
        prefix: module_prefix,
        name: make,
        selector: MemberSelector::Unique,
        space: MemberSpace::Prefix,
    };
    let store_checkpoint = store.checkpoint();
    let resolver_checkpoint = resolver.checkpoint();

    assert!(
        resolver
            .resolve_member(&mut store, &member_request)
            .unwrap()
            .is_some()
    );
    assert!(valid_module_class_info(&store, module_class));
    assert_eq!(
        resolver
            .metrics
            .borrow()
            .module_resolution
            .module_member_successes,
        1
    );
    assert!(
        completed_module_class_targets(&resolver.metrics.borrow())
            .contains("me/cytrowski/tastyfixtures/semantic/Both$")
    );

    resolver.rollback_to(&mut store, resolver_checkpoint);
    store.rollback_to(store_checkpoint);
    assert_eq!(
        store.symbols.get(module_class).info,
        dotty_core::SymbolInfo::Missing
    );
    assert_eq!(
        resolver.complete_module_class(&mut store, module_class),
        Ok(true)
    );
    assert!(valid_module_class_info(&store, module_class));
    assert_eq!(
        resolver
            .metrics
            .borrow()
            .module_resolution
            .module_member_successes,
        1,
        "completion observation must not manufacture a module-member success"
    );

    let metrics = resolver.metrics.borrow();
    assert_eq!(metrics.module_member_observations.len(), 1);
    assert_eq!(metrics.module_member_observations[0].result, "success");
    assert_eq!(
        metrics.module_member_observations[0].transaction_state, "rolled_back",
        "an attempted member read is retained but not counted as committed after rollback"
    );
    let mut observations = metrics.module_operation_observations.clone();
    observations.sort();
    assert_eq!(observations.len(), 2);
    let identity = observations
        .iter()
        .find(|observation| observation.operation == "module_class_of_object")
        .unwrap();
    assert_eq!(identity.source_file, "src/Fixture.scala");
    assert_eq!(identity.enclosing_attempt, "Fixture.run()");
    assert_eq!(identity.result, "resolved");
    assert_eq!(identity.result_state, "validated_module_class");
    assert_eq!(identity.object_origin, "classpath_tasty");
    assert_eq!(identity.module_state, "Missing");
    assert_eq!(
        identity.object_identity,
        "me/cytrowski/tastyfixtures/semantic/Both"
    );
    assert_eq!(
        identity.module_class_identity,
        "me/cytrowski/tastyfixtures/semantic/Both$"
    );
    let completion = observations
        .iter()
        .find(|observation| observation.operation == "complete_module_class")
        .unwrap();
    assert_eq!(completion.source_file, identity.source_file);
    assert_eq!(completion.module_state, "Complete(ClassInfo)");
    assert_eq!(completion.transaction_state, "observed");
    assert_eq!(completion.enclosing_attempt, identity.enclosing_attempt);
    assert_eq!(completion.result, "completed");
    assert_eq!(completion.result_state, "Complete(ClassInfo)");
    assert_eq!(completion.object_identity, identity.object_identity);
    assert_eq!(
        completion.module_class_identity,
        identity.module_class_identity
    );
    assert_eq!(completion.error_category, None);
    assert_eq!(
        completed_module_class_targets(&metrics),
        BTreeSet::from(["me/cytrowski/tastyfixtures/semantic/Both$".to_owned()])
    );
}

fn valid_module_class_info(store: &SemanticStore, module_class: SymbolId) -> bool {
    if !store.symbols.contains(module_class)
        || store.symbols.get(module_class).kind != dotty_core::SymbolKind::ModuleClass
    {
        return false;
    }
    let dotty_core::SymbolInfo::Complete(info) = store.symbols.get(module_class).info else {
        return false;
    };
    if !store.types.contains(info) {
        return false;
    }
    let Type::ClassInfo(info) = store.types.get(info) else {
        return false;
    };
    info.class == module_class
        && store.scopes.contains(info.declarations)
        && store.scopes.get(info.declarations).owner == Some(module_class)
}

fn resolution_error_category(error: &ResolutionError) -> &'static str {
    match error {
        ResolutionError::Ambiguous { .. } => "ambiguous",
        ResolutionError::Malformed { .. } => "malformed",
        _ => "other",
    }
}

fn module_class_observed_state(store: &SemanticStore, module_class: SymbolId) -> &'static str {
    if !store.symbols.contains(module_class) {
        return "UnknownSymbol";
    }
    if store.symbols.get(module_class).kind != dotty_core::SymbolKind::ModuleClass {
        return "WrongSymbolKind";
    }
    match store.symbols.get(module_class).info {
        dotty_core::SymbolInfo::Missing => "Missing",
        dotty_core::SymbolInfo::Error => "Error",
        dotty_core::SymbolInfo::Deferred(_) => "Deferred",
        dotty_core::SymbolInfo::Complete(info) if !store.types.contains(info) => {
            "Complete(InvalidType)"
        }
        dotty_core::SymbolInfo::Complete(info) => match store.types.get(info) {
            Type::ClassInfo(info)
                if info.class == module_class
                    && store.scopes.contains(info.declarations)
                    && store.scopes.get(info.declarations).owner == Some(module_class) =>
            {
                "Complete(ClassInfo)"
            }
            Type::ClassInfo(_) => "Complete(InvalidClassInfo)",
            _ => "Complete(NonClassInfo)",
        },
    }
}

fn canonical_module_class_of_object(store: &SemanticStore, object: SymbolId) -> Option<SymbolId> {
    if !store.symbols.contains(object)
        || store.symbols.get(object).kind != dotty_core::SymbolKind::Object
    {
        return None;
    }
    let dotty_core::SymbolInfo::Complete(info) = store.symbols.get(object).info else {
        return None;
    };
    if !store.types.contains(info) {
        return None;
    }
    let module_class = match store.types.get(info) {
        Type::TypeRef {
            target: TypeRefTarget::Symbol(symbol),
            ..
        }
        | Type::TermRef {
            target: dotty_core::TermRefTarget::Symbol(symbol),
            ..
        } => *symbol,
        _ => return None,
    };
    (store.symbols.contains(module_class)
        && store.symbols.get(module_class).kind == dotty_core::SymbolKind::ModuleClass)
        .then_some(module_class)
}

fn completed_module_class_targets(metrics: &ResolverMetrics) -> BTreeSet<String> {
    let mut targets = metrics
        .module_resolution
        .completed_module_classes
        .iter()
        // The resolver profile stores the canonical Object binary name;
        // ModuleClass has that same name with Scala's module suffix.
        .map(|object| format!("{object}$"))
        .collect::<BTreeSet<_>>();
    targets.extend(
        metrics
            .module_operation_observations
            .iter()
            .filter(|observation| {
                observation.operation == "complete_module_class"
                    && observation.result == "completed"
                    && observation.result_state == "Complete(ClassInfo)"
                    && !observation.module_class_identity.starts_with('<')
            })
            .map(|observation| observation.module_class_identity.clone()),
    );
    targets
}

impl ClassPathEntry for MetricPath {
    fn find_class(&self, name: &BinaryName) -> Result<Option<ClassResource>, ClassPathError> {
        let bytes = match name.as_internal() {
            "Ping" | "Pong" => fs::read(
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("../dotty-classloader/tests/fixtures/ping_pong")
                    .join(format!("{}.class", name.simple_name())),
            )
            .unwrap(),
            "java/lang/Object" => {
                let name = b"java/lang/Object";
                let mut bytes = vec![0xCA, 0xFE, 0xBA, 0xBE, 0, 0, 0, 61, 0, 3, 1];
                bytes.extend_from_slice(&(name.len() as u16).to_be_bytes());
                bytes.extend_from_slice(name);
                bytes.extend_from_slice(&[7, 0, 1, 0, 0x21, 0, 2, 0, 0]);
                bytes.extend_from_slice(&[0; 8]);
                bytes
            }
            _ => return Ok(None),
        };
        Ok(Some(ClassResource::new(
            bytes,
            ClassFormat::Class,
            ClassOrigin::Directory("<metrics>".into()),
        )))
    }
}

#[test]
fn class_member_metrics_use_explicit_owner_and_observe_real_shell_transition() {
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let mut loader =
        ClassLoader::with_definitions(MetricPath, &mut store, definitions, LoadingSession::new());
    let b = loader
        .ensure_class_identity(&BinaryName::from_internal("Ping"))
        .unwrap();
    let (path, session) = loader.into_parts();
    let owner = store.types.alloc(Type::type_ref(definitions.no_prefix, b));
    let metrics = Rc::new(RefCell::new(ResolverMetrics::default()));
    let mut resolver = AuditResolver {
        inner: ClasspathSymbolResolver::new(path, definitions, session),
        metrics: metrics.clone(),
        class_path_probe: None,
    };
    let request = MemberRequest {
        prefix: definitions.int,
        name: dotty_core::Name::new(store.names.intern("other"), Namespace::Term),
        selector: MemberSelector::Unique,
        space: MemberSpace::Explicit(owner),
    };
    let first = resolver
        .resolve_member(&mut store, &request)
        .unwrap()
        .unwrap();
    assert_eq!(
        resolver.resolve_member(&mut store, &request).unwrap(),
        Some(first)
    );
    let metrics = metrics.borrow();
    assert_eq!(metrics.class_member_after_shell_completion, 1);
    assert_eq!(metrics.class_member_completed_prefix_reuse, 1);
    assert_eq!(
        metrics.class_member_successes_by_kind.get("field"),
        Some(&2)
    );
    assert_eq!(
        metrics.class_member_prefixes,
        BTreeSet::from(["Ping".to_owned()])
    );
    assert_eq!(
        metrics.class_member_keys,
        BTreeSet::from(["Ping::namespace=term::name=other".to_owned()])
    );
}

#[test]
fn missing_shell_member_observations_survive_completion_rollback() {
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let mut loader =
        ClassLoader::with_definitions(MetricPath, &mut store, definitions, LoadingSession::new());
    let ping = loader
        .ensure_class_identity(&BinaryName::from_internal("Ping"))
        .unwrap();
    let (path, session) = loader.into_parts();
    let owner = store
        .types
        .alloc(Type::type_ref(definitions.no_prefix, ping));
    let metrics = Rc::new(RefCell::new(ResolverMetrics {
        current_method: "test".to_owned(),
        ..ResolverMetrics::default()
    }));
    let mut resolver = AuditResolver {
        inner: ClasspathSymbolResolver::new(path, definitions, session)
            .with_load_profile_collection(),
        metrics: metrics.clone(),
        class_path_probe: None,
    };
    let name = dotty_core::Name::new(store.names.intern("absent"), Namespace::Term);
    for space in [MemberSpace::Prefix, MemberSpace::Explicit(owner)] {
        let request = MemberRequest {
            prefix: if space == MemberSpace::Prefix {
                owner
            } else {
                definitions.int
            },
            name,
            selector: MemberSelector::Unique,
            space,
        };
        let checkpoint = store.checkpoint();
        assert_eq!(resolver.resolve_member(&mut store, &request), Ok(None));
        assert_eq!(
            store.symbols.get(ping).info,
            dotty_core::SymbolInfo::Missing
        );
        assert_eq!(store.checkpoint(), checkpoint);
    }
    let metrics = metrics.borrow();
    assert_eq!(
        metrics
            .unresolved_member_kinds
            .get("other resolver Ok(None)"),
        Some(&2)
    );
    assert!(
        !metrics
            .unresolved_member_kinds
            .contains_key("class member scope unavailable: incomplete class info")
    );
    let observations = &metrics.resolver_requests_by_method["test"];
    assert_eq!(observations.len(), 2);
    for observation in observations {
        assert_eq!(observation.outcome, "other resolver Ok(None)");
        assert_eq!(observation.cause.as_deref(), Some("other"));
    }
    assert!(metrics.dependency_profiles.iter().all(|observation| {
        observation.profile.root == "Ping" && observation.profile.full_completions > 0
    }));
}

#[test]
fn audit_module_observations_keep_nested_checkpoints_with_repeated_inner_tokens() {
    let mut store = SemanticStore::new();
    let (definitions, mut resolver) =
        module_audit_resolver(&mut store, true, "src/Fixture.scala", "Fixture.run()");
    let object = module_audit_object(&mut store, definitions, &mut resolver);

    let repeated_inner_token = resolver.inner.checkpoint().token();
    let outer = resolver.checkpoint();
    resolver
        .module_class_of_object(&store, object)
        .unwrap()
        .expect("the fixture object has a canonical module class");
    assert_eq!(
        resolver.inner.checkpoint().token(),
        repeated_inner_token,
        "the identity lookup does not mutate the classpath resolver checkpoint"
    );
    let inner = resolver.checkpoint();
    assert_ne!(
        outer.token(),
        inner.token(),
        "audit checkpoints must distinguish nested observation boundaries even when the inner resolver token repeats"
    );

    resolver.rollback_to(&mut store, inner);
    assert_eq!(
        resolver.metrics.borrow().module_operation_observations[0].transaction_state,
        "observed",
        "rolling back the inner checkpoint must preserve observations that predate it"
    );

    resolver.rollback_to(&mut store, outer);
    assert_eq!(
        resolver.metrics.borrow().module_operation_observations[0].transaction_state,
        "rolled_back",
        "rolling back the outer checkpoint must include observations before the nested checkpoint"
    );
}

#[test]
fn unowned_shell_member_remains_an_incomplete_identity_observation() {
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let mut loader =
        ClassLoader::with_definitions(MetricPath, &mut store, definitions, LoadingSession::new());
    let ping = loader
        .ensure_class_identity(&BinaryName::from_internal("Ping"))
        .unwrap();
    let (path, session) = loader.into_parts();
    // The same binary spelling is insufficient: the resolver owns only the
    // canonical identity, not this second externally originated symbol.
    let unowned = store.symbols.alloc(store.symbols.get(ping).clone());
    let owner = store
        .types
        .alloc(Type::type_ref(definitions.no_prefix, unowned));
    let metrics = Rc::new(RefCell::new(ResolverMetrics {
        current_method: "test".to_owned(),
        ..ResolverMetrics::default()
    }));
    let mut resolver = AuditResolver {
        inner: ClasspathSymbolResolver::new(path, definitions, session)
            .with_load_profile_collection(),
        metrics: metrics.clone(),
        class_path_probe: None,
    };
    // A previous completion of the same binary must not affect this request.
    assert_eq!(resolver.complete_class(&mut store, ping), Ok(true));
    let request = MemberRequest {
        prefix: definitions.int,
        name: dotty_core::Name::new(store.names.intern("absent"), Namespace::Term),
        selector: MemberSelector::Unique,
        space: MemberSpace::Explicit(owner),
    };
    let checkpoint = store.checkpoint();
    assert_eq!(resolver.resolve_member(&mut store, &request), Ok(None));
    assert_eq!(store.checkpoint(), checkpoint);
    assert_eq!(
        store.symbols.get(unowned).info,
        dotty_core::SymbolInfo::Missing
    );
    let metrics = metrics.borrow();
    let observations = &metrics.resolver_requests_by_method["test"];
    assert_eq!(observations.len(), 1);
    assert_eq!(
        observations[0].outcome,
        "class member scope unavailable: incomplete class info"
    );
    assert_eq!(
        observations[0].cause.as_deref(),
        Some("incomplete class identity")
    );
    assert_eq!(
        metrics
            .unresolved_member_kinds
            .get(&observations[0].outcome),
        Some(&1)
    );
}

fn is_external_symbol(origin: dotty_core::SymbolOrigin) -> bool {
    matches!(
        origin,
        dotty_core::SymbolOrigin::Classfile(_) | dotty_core::SymbolOrigin::Tasty(_)
    )
}

fn prefix_profile(store: &SemanticStore, ty: dotty_core::ids::TypeId) -> PrefixProfile {
    fn profile(
        store: &SemanticStore,
        ty: dotty_core::ids::TypeId,
        applied: bool,
        depth: u8,
    ) -> PrefixProfile {
        if depth >= 16 || !store.types.contains(ty) {
            return PrefixProfile {
                shape: "unsupported/other",
                target_kind: "unsupported",
                identity: "<unresolved-prefix>".to_owned(),
                identity_known: false,
                symbol: None,
            };
        }
        match store.types.get(ty) {
            Type::Applied { tycon, .. } => profile(store, *tycon, true, depth + 1),
            Type::TypeRef {
                target: TypeRefTarget::Symbol(symbol),
                ..
            } => {
                let entry = store.symbols.get(*symbol);
                let (shape, target_kind) = match entry.kind {
                    dotty_core::SymbolKind::Package => (
                        if applied {
                            "Applied"
                        } else {
                            "package TypeRef"
                        },
                        "Package",
                    ),
                    dotty_core::SymbolKind::Class => {
                        (if applied { "Applied" } else { "class TypeRef" }, "Class")
                    }
                    dotty_core::SymbolKind::Trait => {
                        (if applied { "Applied" } else { "trait TypeRef" }, "Trait")
                    }
                    dotty_core::SymbolKind::ModuleClass => (
                        if applied {
                            "Applied"
                        } else {
                            "module-class TypeRef"
                        },
                        "ModuleClass",
                    ),
                    _ => ("unsupported/other", "unsupported"),
                };
                PrefixProfile {
                    shape,
                    target_kind,
                    identity: audit_symbol_path(store, *symbol),
                    identity_known: true,
                    symbol: Some(*symbol),
                }
            }
            Type::ThisType { class } => {
                let entry = store.symbols.get(*class);
                let target_kind = match entry.kind {
                    dotty_core::SymbolKind::Package => "Package",
                    dotty_core::SymbolKind::Class => "Class",
                    dotty_core::SymbolKind::Trait => "Trait",
                    dotty_core::SymbolKind::ModuleClass => "ModuleClass",
                    _ => "unsupported",
                };
                PrefixProfile {
                    shape: if applied { "Applied" } else { "ThisType" },
                    target_kind,
                    identity: audit_symbol_path(store, *class),
                    identity_known: true,
                    symbol: Some(*class),
                }
            }
            Type::TermRef { prefix, target } => match target {
                dotty_core::types::TermRefTarget::Symbol(symbol)
                    if store.symbols.get(*symbol).kind == dotty_core::SymbolKind::Object =>
                {
                    PrefixProfile {
                        shape: "object TermRef",
                        target_kind: "Object",
                        identity: audit_symbol_path(store, *symbol),
                        identity_known: true,
                        symbol: Some(*symbol),
                    }
                }
                dotty_core::types::TermRefTarget::Symbol(symbol) => PrefixProfile {
                    shape: "unsupported/other",
                    target_kind: "unsupported",
                    identity: audit_symbol_path(store, *symbol),
                    identity_known: true,
                    symbol: Some(*symbol),
                },
                dotty_core::types::TermRefTarget::Name(name) => {
                    let owner = profile(store, *prefix, false, depth + 1);
                    PrefixProfile {
                        shape: "unsupported/other",
                        target_kind: "unsupported",
                        identity: format!(
                            "{}::term-ref:{}",
                            owner.identity,
                            store.names.resolve(name.as_name().text())
                        ),
                        identity_known: owner.identity_known,
                        symbol: None,
                    }
                }
            },
            Type::TypeRef {
                prefix,
                target: TypeRefTarget::Name(name),
            } => {
                let owner = profile(store, *prefix, false, depth + 1);
                PrefixProfile {
                    shape: if applied {
                        "Applied"
                    } else {
                        "unsupported/other"
                    },
                    target_kind: "unsupported",
                    identity: format!(
                        "{}::unresolved-type:{}",
                        owner.identity,
                        store.names.resolve(name.as_name().text())
                    ),
                    identity_known: owner.identity_known,
                    symbol: None,
                }
            }
            _ => PrefixProfile {
                shape: "unsupported/other",
                target_kind: "unsupported",
                identity: "<unsupported-prefix>".to_owned(),
                identity_known: false,
                symbol: None,
            },
        }
    }
    profile(store, ty, false, 0)
}

fn request_selector(selector: &MemberSelector) -> &'static str {
    match selector {
        MemberSelector::Unique => "Unique",
        _ => "other",
    }
}

fn class_resource_label(resource: &ClassResource) -> String {
    let format = match resource.format() {
        ClassFormat::Class => "class",
        ClassFormat::Tasty => "tasty",
    };
    let stable_path = |path: &Path, prefix: &str| {
        let maven_root = std::env::var_os("COURSIER_MAVEN_ROOT").map(PathBuf::from);
        let path = maven_root
            .as_deref()
            .and_then(|root| path.strip_prefix(root).ok())
            .map(|relative| relative.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|| {
                path.file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "unknown".to_owned())
            });
        format!("{prefix}/{path}")
    };
    let origin = match resource.origin() {
        ClassOrigin::Directory(path) => stable_path(path, "directory"),
        ClassOrigin::Jar(path) => stable_path(path, "jar"),
        ClassOrigin::Jmod(path) => stable_path(path, "jmods"),
        _ => "unknown-origin".to_owned(),
    };
    format!("{format}@{origin}")
}

fn request_space(store: &SemanticStore, space: &MemberSpace) -> String {
    match space {
        MemberSpace::Prefix => "Prefix".to_owned(),
        MemberSpace::Explicit(ty) => {
            let prefix = prefix_profile(store, *ty);
            format!("Explicit:{}:{}", prefix.target_kind, prefix.identity)
        }
        _ => "other".to_owned(),
    }
}

fn normalized_error(error: &ResolutionError) -> (String, String, String) {
    match error {
        ResolutionError::Ambiguous { .. } => (
            "ambiguous result".to_owned(),
            "<resolver>".to_owned(),
            "<resolver>".to_owned(),
        ),
        ResolutionError::Malformed { reason } => {
            let lower = reason.to_ascii_lowercase();
            let root_reason = deepest_dependency_cause(&lower);
            let category = if root_reason.contains("unresolved type variable") {
                "UnresolvedTypeVariable"
            } else if root_reason.contains("unresolvedsupertype")
                || root_reason.contains("unresolved supertype")
                || root_reason.contains("supertype reference could not be resolved")
            {
                "InvalidTastyFile::UnresolvedSupertype"
            } else if root_reason.contains("invalid .tasty file") {
                "InvalidTastyFile"
            } else if root_reason.contains("class not found:") {
                "unresolved class"
            } else if root_reason.contains("malformed class file")
                || root_reason.contains("malformed signature")
                || root_reason.contains("invalid class file")
            {
                "malformed classfile/signature"
            } else if root_reason.contains("circular inheritance") {
                "circular inheritance"
            } else if root_reason.contains("invalid declaration scope")
                || root_reason.contains("invalid class info")
                || root_reason.contains("prefix refers to unknown symbol")
            {
                "invalid prefix/owner/class info"
            } else {
                "other"
            };
            let classes = error_binary_names(reason);
            let top = classes
                .first()
                .cloned()
                .unwrap_or_else(|| "<unknown>".to_owned());
            let root = classes.last().cloned().unwrap_or_else(|| top.clone());
            (category.to_owned(), top, root)
        }
        _ => (
            "other".to_owned(),
            "<resolver>".to_owned(),
            "<resolver>".to_owned(),
        ),
    }
}

fn unresolved_tasty_parent_profile(reason: &str) -> Option<String> {
    let details = reason
        .rsplit("unresolved supertype reference at parent ")
        .next()?;
    let (index, details) = details.split_once(": shape=")?;
    let (shape, details) = details.split_once(", semantic_reference=")?;
    let (semantic_reference, details) = details.split_once(", reference_prefix=")?;
    let (reference_prefix, details) = details.split_once(", names=")?;
    let (visible_names, raw_shape) = details.split_once(", raw=")?;
    Some(format!(
        "parent_index={index}::parent_shape={shape}::semantic_reference={semantic_reference}::reference_prefix={reference_prefix}::visible_names={visible_names}::raw_shape={raw_shape}"
    ))
}

fn deepest_dependency_cause(reason: &str) -> &str {
    let mut cause = reason;
    while let Some((_, nested)) = cause.rsplit_once(" failed: ") {
        cause = nested;
    }
    cause
}

fn error_binary_names(reason: &str) -> Vec<String> {
    // TASTy unresolved-parent errors append a diagnostic profile after the
    // class-loading chain. That profile can contain package-qualified names
    // (for example `names=["Annotation", "scala/annotation"]`) that are not
    // classes in the failure chain. Keep identity extraction on the stable
    // error prefix and let `unresolved_tasty_parent_profile` consume the
    // diagnostic payload separately.
    let identity_chain = reason
        .split_once("unresolved supertype reference at parent ")
        .map_or(reason, |(identity_chain, _)| identity_chain);
    identity_chain
        .split(|ch: char| !(ch.is_ascii_alphanumeric() || matches!(ch, '/' | '$' | '_' | '.')))
        .filter(|part| part.contains('/') || part.starts_with("java.") || part.contains('$'))
        .map(str::to_owned)
        .collect()
}

fn member_outcome(
    store: &SemanticStore,
    result: &Result<Option<SymbolId>, ResolutionError>,
    request: &MemberRequest,
    prefix: &PrefixProfile,
    target_completed: bool,
) -> String {
    match result {
        Ok(Some(symbol))
            if matches!(
                store.symbols.get(*symbol).origin,
                dotty_core::SymbolOrigin::Source(_)
            ) =>
        {
            "source reuse".to_owned()
        }
        Ok(Some(symbol)) if is_external_symbol(store.symbols.get(*symbol).origin) => {
            if matches!(
                store.symbols.get(*symbol).kind,
                dotty_core::SymbolKind::Class
                    | dotty_core::SymbolKind::Trait
                    | dotty_core::SymbolKind::Object
                    | dotty_core::SymbolKind::ModuleClass
            ) {
                "external class success".to_owned()
            } else {
                "external non-class success".to_owned()
            }
        }
        Ok(Some(_)) => "other success".to_owned(),
        Ok(None) if target_completed => "other resolver Ok(None)".to_owned(),
        Ok(None) => unresolved_member_kind(store, request, prefix),
        Err(_) => "error".to_owned(),
    }
}

fn member_request_cause(
    outcome: &str,
    prefix: &PrefixProfile,
    result: &Result<Option<SymbolId>, ResolutionError>,
) -> Option<String> {
    if let Err(error) = result {
        let (category, _, _) = normalized_error(error);
        return Some(
            if matches!(
                category.as_str(),
                "UnresolvedTypeVariable"
                    | "InvalidTastyFile::UnresolvedSupertype"
                    | "InvalidTastyFile"
                    | "unresolved class"
                    | "malformed classfile/signature"
                    | "circular inheritance"
            ) {
                "classloader error"
            } else {
                "other"
            }
            .to_owned(),
        );
    }
    match outcome {
        "error" => Some("other".to_owned()),
        "package term member unsupported" => Some("term/object materialization gap".to_owned()),
        "class member scope unavailable: incomplete class info" => {
            Some("incomplete class identity".to_owned())
        }
        "package type member unresolved" => {
            Some("package resolved but member unresolved".to_owned())
        }
        "other resolver Ok(None)" if prefix.target_kind == "Package" => {
            Some("package resolved but member unresolved".to_owned())
        }
        "unsupported prefix shape" | "other resolver Ok(None)" => Some("other".to_owned()),
        _ => None,
    }
}

fn materialized_class_prefix(store: &SemanticStore, symbol: SymbolId) -> Option<String> {
    let entry = store.symbols.get(symbol);
    if !is_external_symbol(entry.origin)
        || !matches!(
            entry.kind,
            dotty_core::SymbolKind::Class
                | dotty_core::SymbolKind::Trait
                | dotty_core::SymbolKind::Object
                | dotty_core::SymbolKind::ModuleClass
        )
    {
        return None;
    }
    let dotty_core::SymbolInfo::Complete(info) = entry.info else {
        return None;
    };
    if !matches!(store.types.get(info), Type::ClassInfo(_)) {
        return None;
    }
    audit_binary_candidate(store, symbol)
}

fn materialized_request_prefix(store: &SemanticStore, prefix: &PrefixProfile) -> Option<String> {
    prefix
        .symbol
        .and_then(|symbol| materialized_class_prefix(store, symbol))
}

fn unresolved_package_details(
    class_path: Option<&SharedClassPath>,
    path: &[&str],
) -> (String, String) {
    let mut resolved = 0;
    if let Some(class_path) = class_path {
        for length in 1..=path.len() {
            match class_path.contains_package(&path[..length]) {
                Ok(true) => resolved = length,
                Ok(false) => break,
                Err(_) => return ("classloader error".to_owned(), path.join(".")),
            }
        }
    }
    if resolved == 0 {
        (
            "first segment/package unresolved".to_owned(),
            format!("{}::<first-segment-unresolved>", path.join(".")),
        )
    } else {
        (
            "first segment/package unresolved".to_owned(),
            format!(
                "{}::resolved-prefix={}::first-missing={}",
                path.join("."),
                path[..resolved].join("."),
                path.get(resolved).copied().unwrap_or("<unknown>")
            ),
        )
    }
}

fn unresolved_member_kind(
    store: &SemanticStore,
    request: &MemberRequest,
    prefix: &PrefixProfile,
) -> String {
    if prefix.target_kind == "Package" {
        return if request.name.is_type() {
            "package type member unresolved".to_owned()
        } else {
            "package term member unsupported".to_owned()
        };
    }
    if let Some(symbol) = prefix.symbol
        && !matches!(
            store.symbols.get(symbol).info,
            dotty_core::SymbolInfo::Complete(_)
        )
        && matches!(prefix.target_kind, "Class" | "Trait" | "ModuleClass")
    {
        return "class member scope unavailable: incomplete class info".to_owned();
    }
    if prefix.target_kind == "unsupported" {
        "unsupported prefix shape".to_owned()
    } else {
        "other resolver Ok(None)".to_owned()
    }
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
    import_qualifier_path: Option<Vec<String>>,
    import_tree_index: Option<u32>,
    unsupported_type_tree: Option<u32>,
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
    local_method_attempts: BTreeSet<String>,
    local_value_blocker_profile: LocalValueBlockerProfile,
    local_value_baseline_outcomes: BTreeSet<LocalValueBaselineOutcome>,
    by_name_method_outcomes: BTreeMap<String, String>,
    local_method_signature_profile: LocalMethodSignatureProfile,
    singleton_reference_profile: SingletonReferenceProfile,
    singleton_source_inventory: SingletonSourceInventory,
    source_function_method_outcomes: BTreeSet<String>,
    source_function_outcomes: BTreeMap<String, FailureBucket>,
    function_with_mods_profile: FunctionWithModsProfile,
    function_with_mods_inventory: FunctionWithModsInventory,
    impure_function_baseline_outcomes: BTreeSet<ImpureFunctionBaselineOutcome>,
    resolution_blocker_profile: ResolutionBlockerProfile,
    import_prefix_profile: ImportPrefixProfile,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct ImportPrefixProfile {
    // A Vec intentionally retains repeated source observations. Rendering sorts
    // a copy so corpus traversal order cannot affect the normalized report.
    observations: Vec<ImportPrefixObservation>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct ImportPrefixObservation {
    source_file: String,
    method: String,
    blocker_origin: String,
    import_tree: String,
    import_line: usize,
    qualifier_source: String,
    qualifier_segments: String,
    context_owner: String,
    context_package: String,
    namespace: String,
    source_lookup: String,
    source_package_registry_lookup: String,
    resolved_prefix: String,
    first_failing_segment: String,
    request_kind: String,
    resolver_outcome: String,
    target_key: String,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct BaselineImportAttempt {
    file: String,
    method_tree: u32,
    blocker_origin_tree: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct FunctionWithModsObservation {
    path: String,
    method_tree: u32,
    method: String,
    blocker_origin_tree: u32,
    blocker_origin_method: String,
    first_failure_tree: u32,
    first_failure_method: String,
    function_tree: u32,
    line: usize,
    span: String,
    syntax: String,
    parameter_count: usize,
    parameters: String,
    result_kind: String,
    source_kind: String,
    modifiers: String,
    visibility: String,
    annotations: bool,
    erased_params: String,
    attribution: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct FunctionWithModsProfile {
    observations: BTreeSet<FunctionWithModsObservation>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct FunctionWithModsInventory {
    total: usize,
    plain_function_trees: usize,
    modifiers: BTreeMap<String, usize>,
    source_kinds: BTreeMap<String, usize>,
    source_kind_files: BTreeMap<String, BTreeSet<String>>,
    plain_function_files: BTreeSet<String>,
    erased: usize,
    capture_result_adjacency: usize,
    files: BTreeSet<String>,
}

impl FunctionWithModsInventory {
    fn merge(&mut self, other: Self) {
        self.total += other.total;
        self.plain_function_trees += other.plain_function_trees;
        self.erased += other.erased;
        self.capture_result_adjacency += other.capture_result_adjacency;
        self.files.extend(other.files);
        self.plain_function_files.extend(other.plain_function_files);
        merge_counts(&mut self.modifiers, other.modifiers);
        merge_counts(&mut self.source_kinds, other.source_kinds);
        for (kind, files) in other.source_kind_files {
            self.source_kind_files
                .entry(kind)
                .or_default()
                .extend(files);
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct ImpureFunctionBaselineOutcome {
    path: String,
    method_tree: u32,
    method: String,
    function_tree: u32,
    line: usize,
    span: String,
    syntax: String,
    semantic_family: String,
    expected_semantic_family: String,
    modifiers: String,
    erased_params: String,
    arity: usize,
    source_kind: String,
    attribution: String,
    projection_completed: bool,
    projection_error: Option<String>,
    first_blocker: String,
    method_typed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ImpureFunctionProjection {
    completed: bool,
    semantic_family: String,
    error: Option<String>,
}

#[derive(Clone, Copy)]
struct ImpureFunctionBaselineRow {
    path: &'static str,
    method_tree: u32,
    method: &'static str,
    function_tree: u32,
    line: usize,
    span: &'static str,
    syntax: &'static str,
    attribution: &'static str,
}

// Frozen from the #946 audit. Keep these identities independent of the current
// first-blocker profile so moving a blocker cannot silently erase a row.
const IMPURE_FUNCTION_BASELINE: &[ImpureFunctionBaselineRow] = &[
    ImpureFunctionBaselineRow {
        path: "library/src/scala/collection/StringParsers.scala",
        method_tree: 750,
        method: "rec",
        function_tree: 724,
        line: 185,
        span: "5576..5591",
        syntax: "Char => Boolean",
        attribution: "sibling/inherited",
    },
    ImpureFunctionBaselineRow {
        path: "library/src/scala/collection/StringParsers.scala",
        method_tree: 755,
        method: "forAllBetween",
        function_tree: 724,
        line: 185,
        span: "5576..5591",
        syntax: "Char => Boolean",
        attribution: "direct",
    },
    ImpureFunctionBaselineRow {
        path: "library/src/scala/collection/StringParsers.scala",
        method_tree: 799,
        method: "rec",
        function_tree: 724,
        line: 185,
        span: "5576..5591",
        syntax: "Char => Boolean",
        attribution: "sibling/inherited",
    },
    ImpureFunctionBaselineRow {
        path: "library/src/scala/collection/StringParsers.scala",
        method_tree: 804,
        method: "skipIndexWhile",
        function_tree: 724,
        line: 185,
        span: "5576..5591",
        syntax: "Char => Boolean",
        attribution: "sibling/inherited",
    },
    ImpureFunctionBaselineRow {
        path: "library/src/scala/collection/StringParsers.scala",
        method_tree: 840,
        method: "isHexDigit",
        function_tree: 724,
        line: 185,
        span: "5576..5591",
        syntax: "Char => Boolean",
        attribution: "sibling/inherited",
    },
    ImpureFunctionBaselineRow {
        path: "library/src/scala/collection/StringParsers.scala",
        method_tree: 908,
        method: "prefixOK",
        function_tree: 724,
        line: 185,
        span: "5576..5591",
        syntax: "Char => Boolean",
        attribution: "sibling/inherited",
    },
    ImpureFunctionBaselineRow {
        path: "library/src/scala/collection/StringParsers.scala",
        method_tree: 957,
        method: "postfixOK",
        function_tree: 724,
        line: 185,
        span: "5576..5591",
        syntax: "Char => Boolean",
        attribution: "sibling/inherited",
    },
    ImpureFunctionBaselineRow {
        path: "library/src/scala/collection/StringParsers.scala",
        method_tree: 991,
        method: "isHexFloatLiteral",
        function_tree: 724,
        line: 185,
        span: "5576..5591",
        syntax: "Char => Boolean",
        attribution: "sibling/inherited",
    },
    ImpureFunctionBaselineRow {
        path: "library/src/scala/collection/StringParsers.scala",
        method_tree: 1007,
        method: "isExp",
        function_tree: 724,
        line: 185,
        span: "5576..5591",
        syntax: "Char => Boolean",
        attribution: "sibling/inherited",
    },
    ImpureFunctionBaselineRow {
        path: "library/src/scala/collection/StringParsers.scala",
        method_tree: 1058,
        method: "expOK",
        function_tree: 724,
        line: 185,
        span: "5576..5591",
        syntax: "Char => Boolean",
        attribution: "sibling/inherited",
    },
    ImpureFunctionBaselineRow {
        path: "library/src/scala/collection/StringParsers.scala",
        method_tree: 1175,
        method: "isDecFloatLiteral",
        function_tree: 724,
        line: 185,
        span: "5576..5591",
        syntax: "Char => Boolean",
        attribution: "sibling/inherited",
    },
    ImpureFunctionBaselineRow {
        path: "library/src/scala/collection/convert/JavaCollectionWrappers.scala",
        method_tree: 1973,
        method: "remap",
        function_tree: 1937,
        line: 399,
        span: "15034..15056",
        syntax: "Option[V] => Option[V]",
        attribution: "sibling/inherited",
    },
    ImpureFunctionBaselineRow {
        path: "library/src/scala/collection/convert/JavaCollectionWrappers.scala",
        method_tree: 2559,
        method: "remap",
        function_tree: 2523,
        line: 510,
        span: "19443..19465",
        syntax: "Option[V] => Option[V]",
        attribution: "sibling/inherited",
    },
];

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
            local_method_attempts: BTreeSet::new(),
            local_value_blocker_profile: LocalValueBlockerProfile::default(),
            local_value_baseline_outcomes: BTreeSet::new(),
            by_name_method_outcomes: BTreeMap::new(),
            local_method_signature_profile: LocalMethodSignatureProfile::default(),
            singleton_reference_profile: SingletonReferenceProfile::default(),
            singleton_source_inventory: SingletonSourceInventory::default(),
            source_function_method_outcomes: BTreeSet::new(),
            source_function_outcomes: BTreeMap::new(),
            function_with_mods_profile: FunctionWithModsProfile::default(),
            function_with_mods_inventory: FunctionWithModsInventory::default(),
            impure_function_baseline_outcomes: BTreeSet::new(),
            resolution_blocker_profile: ResolutionBlockerProfile::default(),
            import_prefix_profile: ImportPrefixProfile::default(),
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

#[derive(Clone, Copy)]
struct LocalValueBaselineRow {
    path: &'static str,
    method_tree: u32,
    blocker_origin_tree: u32,
    declaration_tree: u32,
}

#[derive(Clone, Copy)]
struct LocalValueBaselineSignature {
    path: &'static str,
    method_tree: u32,
    method: &'static str,
    declaration_tree: u32,
    declaration: &'static str,
    declaration_kind: &'static str,
    modifiers: &'static str,
}

// Independent #929 snapshot for identities and declaration shapes. Keep this
// separate from the tree-index routing table: indexes alone can silently point
// at a different AST node after parser changes.
const LOCAL_VALUE_BASELINE_SIGNATURES: &[LocalValueBaselineSignature] = &[
    LocalValueBaselineSignature {
        path: "compiler/src/dotty/tools/backend/ScalaPrimitives.scala",
        method_tree: 167,
        method: "elementType",
        declaration_tree: 95,
        declaration: "<anonymous>",
        declaration_kind: "given",
        modifiers: "Given,Final,Lazy",
    },
    LocalValueBaselineSignature {
        path: "compiler/src/dotty/tools/backend/ScalaPrimitives.scala",
        method_tree: 367,
        method: "addPrimitive",
        declaration_tree: 338,
        declaration: "<anonymous>",
        declaration_kind: "given",
        modifiers: "Given,Final,Lazy",
    },
    LocalValueBaselineSignature {
        path: "compiler/src/dotty/tools/backend/ScalaPrimitives.scala",
        method_tree: 440,
        method: "addPrimitives",
        declaration_tree: 338,
        declaration: "<anonymous>",
        declaration_kind: "given",
        modifiers: "Given,Final,Lazy",
    },
    LocalValueBaselineSignature {
        path: "compiler/src/dotty/tools/backend/jvm/opt/ClosureOptimizer.scala",
        method_tree: 236,
        method: "addRewrite",
        declaration_tree: 195,
        declaration: "<anonymous>",
        declaration_kind: "given",
        modifiers: "Given,Final,Lazy",
    },
    LocalValueBaselineSignature {
        path: "compiler/src/dotty/tools/backend/sjs/JSCodeGen.scala",
        method_tree: 10367,
        method: "isStringMethodFromObject",
        declaration_tree: 10316,
        declaration: "pos",
        declaration_kind: "implicit val",
        modifiers: "Implicit",
    },
    LocalValueBaselineSignature {
        path: "compiler/src/dotty/tools/backend/sjs/JSCodeGen.scala",
        method_tree: 11524,
        method: "abortMatch",
        declaration_tree: 11493,
        declaration: "pos",
        declaration_kind: "implicit val",
        modifiers: "Implicit",
    },
    LocalValueBaselineSignature {
        path: "compiler/src/dotty/tools/backend/sjs/JSCodeGen.scala",
        method_tree: 11629,
        method: "invalidCase",
        declaration_tree: 11493,
        declaration: "pos",
        declaration_kind: "implicit val",
        modifiers: "Implicit",
    },
    LocalValueBaselineSignature {
        path: "compiler/src/dotty/tools/backend/sjs/JSCodeGen.scala",
        method_tree: 11651,
        method: "genMatchableLiteral",
        declaration_tree: 11493,
        declaration: "pos",
        declaration_kind: "implicit val",
        modifiers: "Implicit",
    },
    LocalValueBaselineSignature {
        path: "compiler/src/dotty/tools/backend/sjs/JSCodeGen.scala",
        method_tree: 11745,
        method: "isInt",
        declaration_tree: 11493,
        declaration: "pos",
        declaration_kind: "implicit val",
        modifiers: "Implicit",
    },
    LocalValueBaselineSignature {
        path: "compiler/src/dotty/tools/backend/sjs/JSCodeGen.scala",
        method_tree: 4923,
        method: "isIgnorableDefaultParam",
        declaration_tree: 4843,
        declaration: "pos",
        declaration_kind: "implicit val",
        modifiers: "Implicit",
    },
    LocalValueBaselineSignature {
        path: "compiler/src/dotty/tools/backend/sjs/JSCodeGen.scala",
        method_tree: 4975,
        method: "jsParams",
        declaration_tree: 4843,
        declaration: "pos",
        declaration_kind: "implicit val",
        modifiers: "Implicit",
    },
    LocalValueBaselineSignature {
        path: "compiler/src/dotty/tools/backend/sjs/JSCodeGen.scala",
        method_tree: 5228,
        method: "genBody",
        declaration_tree: 5197,
        declaration: "pos",
        declaration_kind: "implicit val",
        modifiers: "Implicit",
    },
    LocalValueBaselineSignature {
        path: "compiler/src/dotty/tools/backend/sjs/JSCodeGen.scala",
        method_tree: 6279,
        method: "genRhs",
        declaration_tree: 5662,
        declaration: "pos",
        declaration_kind: "implicit val",
        modifiers: "Implicit",
    },
    LocalValueBaselineSignature {
        path: "compiler/src/dotty/tools/backend/sjs/JSCodeGen.scala",
        method_tree: 6327,
        method: "ctorAssignment",
        declaration_tree: 5662,
        declaration: "pos",
        declaration_kind: "implicit val",
        modifiers: "Implicit",
    },
    LocalValueBaselineSignature {
        path: "compiler/src/dotty/tools/backend/sjs/JSCodeGen.scala",
        method_tree: 6966,
        method: "isJSDefaultParam",
        declaration_tree: 6904,
        declaration: "pos",
        declaration_kind: "implicit val",
        modifiers: "Implicit",
    },
    LocalValueBaselineSignature {
        path: "compiler/src/dotty/tools/backend/sjs/JSExportsGen.scala",
        method_tree: 2075,
        method: "hasDefaultParam",
        declaration_tree: 1862,
        declaration: "pos",
        declaration_kind: "implicit val",
        modifiers: "Implicit",
    },
    LocalValueBaselineSignature {
        path: "compiler/src/dotty/tools/dotc/ast/Trees.scala",
        method_tree: 10108,
        method: "typeParamCount",
        declaration_tree: 10035,
        declaration: "ctx",
        declaration_kind: "given",
        modifiers: "Given,Final,Lazy",
    },
    LocalValueBaselineSignature {
        path: "compiler/src/dotty/tools/dotc/core/NameOps.scala",
        method_tree: 895,
        method: "collectDigits",
        declaration_tree: 851,
        declaration: "MaxSafeInt",
        declaration_kind: "inline val",
        modifiers: "Inline",
    },
    LocalValueBaselineSignature {
        path: "compiler/src/dotty/tools/dotc/transform/MixinOps.scala",
        method_tree: 355,
        method: "generateJUnitForwarder",
        declaration_tree: 297,
        declaration: "competingMethods",
        declaration_kind: "lazy val",
        modifiers: "Lazy",
    },
    LocalValueBaselineSignature {
        path: "compiler/src/dotty/tools/dotc/transform/MixinOps.scala",
        method_tree: 375,
        method: "generateSerializationForwarder",
        declaration_tree: 297,
        declaration: "competingMethods",
        declaration_kind: "lazy val",
        modifiers: "Lazy",
    },
    LocalValueBaselineSignature {
        path: "compiler/src/dotty/tools/dotc/typer/Namer.scala",
        method_tree: 8664,
        method: "infoDontForceAnnots",
        declaration_tree: 8645,
        declaration: "accessorSyms",
        declaration_kind: "lazy val",
        modifiers: "Lazy",
    },
];

const LOCAL_VALUE_BASELINE_TYPE_FORMS: &[(&str, u32, &str)] = &[
    (
        "compiler/src/dotty/tools/backend/ScalaPrimitives.scala",
        95,
        "explicit",
    ),
    (
        "compiler/src/dotty/tools/backend/ScalaPrimitives.scala",
        338,
        "explicit",
    ),
    (
        "compiler/src/dotty/tools/backend/jvm/opt/ClosureOptimizer.scala",
        195,
        "explicit",
    ),
    (
        "compiler/src/dotty/tools/backend/sjs/JSCodeGen.scala",
        10316,
        "inferred",
    ),
    (
        "compiler/src/dotty/tools/backend/sjs/JSCodeGen.scala",
        11493,
        "inferred",
    ),
    (
        "compiler/src/dotty/tools/backend/sjs/JSCodeGen.scala",
        4843,
        "inferred",
    ),
    (
        "compiler/src/dotty/tools/backend/sjs/JSCodeGen.scala",
        5197,
        "inferred",
    ),
    (
        "compiler/src/dotty/tools/backend/sjs/JSCodeGen.scala",
        5662,
        "explicit",
    ),
    (
        "compiler/src/dotty/tools/backend/sjs/JSCodeGen.scala",
        6904,
        "inferred",
    ),
    (
        "compiler/src/dotty/tools/backend/sjs/JSExportsGen.scala",
        1862,
        "inferred",
    ),
    (
        "compiler/src/dotty/tools/dotc/ast/Trees.scala",
        10035,
        "explicit",
    ),
    (
        "compiler/src/dotty/tools/dotc/core/NameOps.scala",
        851,
        "inferred",
    ),
    (
        "compiler/src/dotty/tools/dotc/transform/MixinOps.scala",
        297,
        "inferred",
    ),
    (
        "compiler/src/dotty/tools/dotc/typer/Namer.scala",
        8645,
        "inferred",
    ),
];

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct LocalValueBaselineOutcome {
    observation: LocalValueBlockerObservation,
    baseline_first_blocker: &'static str,
    current_first_blocker: String,
    declaration_typed: bool,
    classification: &'static str,
}

const LOCAL_VALUE_BASELINE_FIRST_BLOCKER: &str =
    "LocalBlockDeclarationDeferred::val/var definition";

struct LocalValueBaselineAuditContext<'typer, 'source> {
    arena: &'typer dotty_core::AstArena<Untyped>,
    names: &'typer dotty_core::names::NameInterner,
    source_text: &'typer str,
    path: &'typer str,
    method_tree: TreeId<Untyped>,
    typer: &'typer SourceTyper<'source>,
    source: SourceId,
}

const LOCAL_VALUE_CONTEXTUAL_BASELINE_ROWS: &[LocalValueBaselineRow] = &[
    LocalValueBaselineRow {
        path: "compiler/src/dotty/tools/backend/ScalaPrimitives.scala",
        method_tree: 167,
        blocker_origin_tree: 331,
        declaration_tree: 95,
    },
    LocalValueBaselineRow {
        path: "compiler/src/dotty/tools/backend/ScalaPrimitives.scala",
        method_tree: 367,
        blocker_origin_tree: 1640,
        declaration_tree: 338,
    },
    LocalValueBaselineRow {
        path: "compiler/src/dotty/tools/backend/ScalaPrimitives.scala",
        method_tree: 440,
        blocker_origin_tree: 1640,
        declaration_tree: 338,
    },
    LocalValueBaselineRow {
        path: "compiler/src/dotty/tools/backend/jvm/opt/ClosureOptimizer.scala",
        method_tree: 236,
        blocker_origin_tree: 453,
        declaration_tree: 195,
    },
    LocalValueBaselineRow {
        path: "compiler/src/dotty/tools/backend/sjs/JSCodeGen.scala",
        method_tree: 10367,
        blocker_origin_tree: 10456,
        declaration_tree: 10316,
    },
    LocalValueBaselineRow {
        path: "compiler/src/dotty/tools/backend/sjs/JSCodeGen.scala",
        method_tree: 11524,
        blocker_origin_tree: 11809,
        declaration_tree: 11493,
    },
    LocalValueBaselineRow {
        path: "compiler/src/dotty/tools/backend/sjs/JSCodeGen.scala",
        method_tree: 11629,
        blocker_origin_tree: 11809,
        declaration_tree: 11493,
    },
    LocalValueBaselineRow {
        path: "compiler/src/dotty/tools/backend/sjs/JSCodeGen.scala",
        method_tree: 11651,
        blocker_origin_tree: 11809,
        declaration_tree: 11493,
    },
    LocalValueBaselineRow {
        path: "compiler/src/dotty/tools/backend/sjs/JSCodeGen.scala",
        method_tree: 11745,
        blocker_origin_tree: 11809,
        declaration_tree: 11493,
    },
    LocalValueBaselineRow {
        path: "compiler/src/dotty/tools/backend/sjs/JSCodeGen.scala",
        method_tree: 4923,
        blocker_origin_tree: 5172,
        declaration_tree: 4843,
    },
    LocalValueBaselineRow {
        path: "compiler/src/dotty/tools/backend/sjs/JSCodeGen.scala",
        method_tree: 4975,
        blocker_origin_tree: 5172,
        declaration_tree: 4843,
    },
    LocalValueBaselineRow {
        path: "compiler/src/dotty/tools/backend/sjs/JSCodeGen.scala",
        method_tree: 5228,
        blocker_origin_tree: 5328,
        declaration_tree: 5197,
    },
    LocalValueBaselineRow {
        path: "compiler/src/dotty/tools/backend/sjs/JSCodeGen.scala",
        method_tree: 6279,
        blocker_origin_tree: 6447,
        declaration_tree: 5662,
    },
    LocalValueBaselineRow {
        path: "compiler/src/dotty/tools/backend/sjs/JSCodeGen.scala",
        method_tree: 6327,
        blocker_origin_tree: 6447,
        declaration_tree: 5662,
    },
    LocalValueBaselineRow {
        path: "compiler/src/dotty/tools/backend/sjs/JSCodeGen.scala",
        method_tree: 6966,
        blocker_origin_tree: 7061,
        declaration_tree: 6904,
    },
    LocalValueBaselineRow {
        path: "compiler/src/dotty/tools/backend/sjs/JSExportsGen.scala",
        method_tree: 2075,
        blocker_origin_tree: 2169,
        declaration_tree: 1862,
    },
    LocalValueBaselineRow {
        path: "compiler/src/dotty/tools/dotc/ast/Trees.scala",
        method_tree: 10108,
        blocker_origin_tree: 10270,
        declaration_tree: 10035,
    },
];

const LOCAL_VALUE_LAZY_BASELINE_ROWS: &[LocalValueBaselineRow] = &[
    LocalValueBaselineRow {
        path: "compiler/src/dotty/tools/dotc/transform/MixinOps.scala",
        method_tree: 355,
        blocker_origin_tree: 439,
        declaration_tree: 297,
    },
    LocalValueBaselineRow {
        path: "compiler/src/dotty/tools/dotc/transform/MixinOps.scala",
        method_tree: 375,
        blocker_origin_tree: 439,
        declaration_tree: 297,
    },
    LocalValueBaselineRow {
        path: "compiler/src/dotty/tools/dotc/typer/Namer.scala",
        method_tree: 8664,
        blocker_origin_tree: 8719,
        declaration_tree: 8645,
    },
];

const LOCAL_VALUE_INLINE_BASELINE_ROWS: &[LocalValueBaselineRow] = &[LocalValueBaselineRow {
    path: "compiler/src/dotty/tools/dotc/core/NameOps.scala",
    method_tree: 895,
    blocker_origin_tree: 903,
    declaration_tree: 851,
}];

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

fn local_value_baseline_outcome_classification(
    blocker: &str,
    declaration_typed: bool,
) -> &'static str {
    if blocker == "typed" {
        if declaration_typed {
            "declaration typed and enclosing method typed"
        } else {
            "enclosing method typed but baseline declaration was not entered in the local symbol map"
        }
    } else if declaration_typed
        && (blocker == "ImportQualifierNotFound"
            || blocker == "TermNameNotFound"
            || blocker == "TypeNameNotFound"
            || blocker == "MemberNotFound"
            || blocker == "MemberLookup"
            || blocker == "SymbolResolution")
    {
        "declaration typed; enclosing method stopped at resolution/classpath blocker"
    } else if declaration_typed {
        "declaration typed; enclosing method moved to a deeper blocker"
    } else if blocker.starts_with("LocalValueModifierDeferred")
        || blocker == "LocalValueMetadataDeferred"
        || blocker.starts_with("LocalValueType")
        || blocker.starts_with("LocalValueRightHandSide")
        || blocker.starts_with("LocalValueOutsideBlock")
        || blocker.starts_with("LocalBlockDeclarationDeferred::val/var definition")
        || blocker == "DuplicateLocalValue"
    {
        "residual local-value declaration blocker"
    } else if blocker == "RecursiveLazyLocalValueInitializer" {
        "intentional local lazy recursion deferral"
    } else if blocker == "UsingApplicationDeferred"
        || blocker == "ApplicationMethodKindMismatch"
        || blocker == "ApplicationCalleeNotMethod"
        || blocker == "UnsupportedApplicationMethodKind"
    {
        "downstream contextual-search/application blocker"
    } else if blocker == "ImportQualifierNotFound"
        || blocker == "TermNameNotFound"
        || blocker == "TypeNameNotFound"
        || blocker == "MemberNotFound"
        || blocker == "MemberLookup"
        || blocker == "SymbolResolution"
    {
        "declaration not observed as typed; enclosing method stopped at resolution/classpath blocker"
    } else {
        "declaration not observed as typed; enclosing method stopped at another blocker"
    }
}

fn record_local_value_baseline_outcomes(
    audit: &mut Audit,
    context: LocalValueBaselineAuditContext<'_, '_>,
    current_first_blocker: &str,
) {
    for baseline in LOCAL_VALUE_CONTEXTUAL_BASELINE_ROWS
        .iter()
        .chain(LOCAL_VALUE_LAZY_BASELINE_ROWS)
        .chain(LOCAL_VALUE_INLINE_BASELINE_ROWS)
        .filter(|row| row.path == context.path && row.method_tree == context.method_tree.index())
    {
        let signature = LOCAL_VALUE_BASELINE_SIGNATURES
            .iter()
            .find(|signature| {
                signature.path == baseline.path
                    && signature.method_tree == baseline.method_tree
                    && signature.declaration_tree == baseline.declaration_tree
            })
            .unwrap_or_else(|| {
                panic!(
                    "pinned local-value row has no independent signature: path={} method={} declaration={}",
                    baseline.path, baseline.method_tree, baseline.declaration_tree
                )
            });
        let expected_type_form = LOCAL_VALUE_BASELINE_TYPE_FORMS
            .iter()
            .find_map(|(path, declaration_tree, type_form)| {
                (*path == baseline.path && *declaration_tree == baseline.declaration_tree)
                    .then_some(*type_form)
            })
            .unwrap_or_else(|| {
                panic!(
                    "pinned local-value declaration has no frozen type form: path={} declaration={}",
                    baseline.path, baseline.declaration_tree
                )
            });
        let observation = local_value_blocker_observation(
            context.arena,
            context.names,
            context.source_text,
            context.path,
            context.method_tree,
            baseline.declaration_tree,
            baseline.blocker_origin_tree,
        )
        .unwrap_or_else(|| {
            panic!(
                "pinned local-value baseline tree path={} method={} declaration={} must remain observable",
                baseline.path, baseline.method_tree, baseline.declaration_tree
            )
        });
        assert_eq!(
            observation.path, signature.path,
            "pinned source path changed"
        );
        assert_eq!(
            observation.enclosing_method,
            format!(
                "{}#tree={}:{}",
                signature.path, signature.method_tree, signature.method
            ),
            "pinned enclosing method identity changed"
        );
        assert_eq!(observation.declaration_tree, signature.declaration_tree);
        assert_eq!(
            observation.node_kind, "ValDef",
            "pinned declaration node kind changed"
        );
        assert_eq!(observation.declaration_name, signature.declaration);
        assert_eq!(observation.declaration_kind, signature.declaration_kind);
        assert_eq!(observation.modifiers, signature.modifiers);
        assert_eq!(observation.type_form, expected_type_form);
        assert_eq!(observation.attribution, "inherited");
        let declaration_tree = context
            .arena
            .iter()
            .find_map(|(tree, _)| (tree.index() == baseline.declaration_tree).then_some(tree))
            .expect("pinned local-value baseline declaration must remain in the AST");
        let declaration_typed = context
            .typer
            .local_symbol_at(context.source, declaration_tree)
            .is_some();
        audit
            .local_value_baseline_outcomes
            .insert(LocalValueBaselineOutcome {
                observation,
                baseline_first_blocker: LOCAL_VALUE_BASELINE_FIRST_BLOCKER,
                current_first_blocker: current_first_blocker.to_owned(),
                declaration_typed,
                classification: local_value_baseline_outcome_classification(
                    current_first_blocker,
                    declaration_typed,
                ),
            });
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
        self.local_method_attempts
            .extend(other.local_method_attempts);
        self.resolution_blocker_profile
            .observations
            .extend(other.resolution_blocker_profile.observations);
        self.import_prefix_profile
            .observations
            .extend(other.import_prefix_profile.observations);
        self.local_value_blocker_profile
            .observations
            .extend(other.local_value_blocker_profile.observations);
        self.local_value_baseline_outcomes
            .extend(other.local_value_baseline_outcomes);
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
        self.function_with_mods_profile
            .observations
            .extend(other.function_with_mods_profile.observations);
        self.function_with_mods_inventory
            .merge(other.function_with_mods_inventory);
        self.impure_function_baseline_outcomes
            .extend(other.impure_function_baseline_outcomes);
    }
}

impl ResolutionBlockerProfile {
    fn record_attempt(
        &mut self,
        blocker: &str,
        file: &str,
        method_tree: u32,
        trace: &[ResolverRequestObservation],
        import_qualifier: Option<&[String]>,
    ) {
        if !matches!(
            blocker,
            "ImportQualifierNotFound"
                | "SymbolResolution"
                | "TypeNameNotFound"
                | "MemberLookup"
                | "TermNameNotFound"
        ) {
            return;
        }

        let relevant_trace = if blocker == "ImportQualifierNotFound" {
            import_qualifier.map_or_else(Vec::new, |qualifier| {
                trace
                    .iter()
                    .filter(|request| request_matches_qualifier(request, qualifier))
                    .cloned()
                    .collect()
            })
        } else {
            trace.to_vec()
        };
        let failed_requests = relevant_trace
            .iter()
            .filter_map(|request| {
                request
                    .cause
                    .as_ref()
                    .map(|cause| (cause.clone(), request.target.clone()))
            })
            .collect::<Vec<_>>();
        let distinct_failures = failed_requests.iter().cloned().collect::<BTreeSet<_>>();
        let candidate_causes = distinct_failures
            .iter()
            .map(|(cause, _)| cause.clone())
            .collect::<BTreeSet<_>>();
        let cause = if blocker == "ImportQualifierNotFound" {
            failed_requests
                .last()
                .map(|(cause, _)| cause.clone())
                .unwrap_or_else(|| "unknown: no matching failed resolver request".to_owned())
        } else {
            match distinct_failures.len() {
                1 => distinct_failures
                    .first()
                    .map(|(cause, _)| cause.clone())
                    .unwrap_or_else(|| "unknown: no failed resolver request".to_owned()),
                0 => "unknown: no failed resolver request".to_owned(),
                _ => "unknown: multiple failed resolver requests".to_owned(),
            }
        };
        let targets = relevant_trace
            .iter()
            .filter(|request| request.cause.is_some())
            .map(|request| request.target.clone())
            .filter(|target| !target.is_empty())
            .collect();
        let materialized_prefixes = relevant_trace
            .iter()
            .filter_map(|request| request.materialized_prefix.clone())
            .collect();
        let request_attempts = relevant_trace
            .iter()
            .filter(|request| request.cause.is_some())
            .map(|request| request.enclosing_attempt.clone())
            .filter(|attempt| !attempt.is_empty())
            .collect();
        let failed_qualifier = import_qualifier
            .map(|parts| parts.join("."))
            .or_else(|| {
                (blocker == "TermNameNotFound")
                    .then(|| {
                        relevant_trace.iter().rev().find_map(|request| {
                            (request.cause.as_deref() == Some("term/object materialization gap"))
                                .then(|| request_target_package_term_qualifier(&request.target))
                                .flatten()
                        })
                    })
                    .flatten()
            })
            .unwrap_or_default();
        self.observations.insert(ResolutionBlockerObservation {
            blocker: blocker.to_owned(),
            cause,
            file: file.to_owned(),
            method: format!("{file}#tree={method_tree}"),
            failed_qualifier,
            request_attempts,
            targets,
            candidate_causes,
            materialized_prefixes,
        });
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
        if index % 25 == 0 {
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
        Some(5),
        "the current main baseline retains five ordinary function-type rows at SymbolResolution"
    );
    assert_eq!(audit.source_function_method_outcomes.len(), 25);
    assert_eq!(IMPURE_FUNCTION_BASELINE.len(), 13);
    assert_eq!(
        audit.impure_function_baseline_outcomes.len(),
        13,
        "every frozen #946 row must still be accounted for individually"
    );
    assert_eq!(
        audit.function_with_mods_profile.observations.len(),
        0,
        "#949 should move the impure ordinary corpus cohort past FunctionWithMods"
    );
    assert!(
        audit.function_with_mods_inventory.total
            >= *audit.type_tree_forms.get("FunctionWithMods").unwrap_or(&0),
        "the full structural census must include the declared-type inventory"
    );
    assert_eq!(
        audit.function_with_mods_inventory.plain_function_trees,
        *audit.type_tree_forms.get("Function").unwrap_or(&0),
        "plain source function types must match the declared-type inventory"
    );
    assert_eq!(
        audit
            .function_with_mods_inventory
            .modifiers
            .values()
            .sum::<usize>(),
        audit.function_with_mods_inventory.total
    );
    assert_eq!(
        audit
            .function_with_mods_inventory
            .source_kinds
            .values()
            .sum::<usize>(),
        audit.function_with_mods_inventory.total
    );
    assert_eq!(
        audit
            .impure_function_baseline_outcomes
            .iter()
            .map(|row| (row.path.as_str(), row.function_tree))
            .collect::<BTreeSet<_>>()
            .len(),
        3,
        "the frozen cohort should retain three source function-type trees"
    );
    for row in &audit.impure_function_baseline_outcomes {
        assert_eq!(row.source_kind, "ordinary/impure", "{row:?}");
        assert_eq!(row.modifiers, "Impure", "{row:?}");
        assert_eq!(row.erased_params, "ordinary", "{row:?}");
        assert_eq!(row.arity, 1, "{row:?}");
        if row.projection_completed {
            assert!(row.projection_error.is_none(), "{row:?}");
            assert_eq!(row.semantic_family, "ImpureFunction1", "{row:?}");
        } else {
            assert_eq!(row.semantic_family, "unselected", "{row:?}");
            assert!(row.projection_error.is_some(), "{row:?}");
        }
        assert_eq!(row.expected_semantic_family, "ImpureFunction1", "{row:?}");
        assert_eq!(row.first_blocker, "SymbolResolution", "{row:?}");
        assert!(!row.method_typed, "{row:?}");
    }
    assert_eq!(
        audit
            .impure_function_baseline_outcomes
            .iter()
            .filter(|row| row.attribution == "direct")
            .count(),
        1
    );
    assert_eq!(
        audit
            .impure_function_baseline_outcomes
            .iter()
            .filter(|row| row.attribution == "sibling/inherited")
            .count(),
        12
    );
    assert_eq!(
        audit
            .impure_function_baseline_outcomes
            .iter()
            .map(|row| row.path.as_str())
            .collect::<BTreeSet<_>>()
            .len(),
        2,
        "the frozen impure-function cohort should remain in the two known files"
    );

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
    assert_eq!(local_value_blocker_count, 1);
    assert_eq!(audit.local_value_blocker_profile.observations.len(), 1);
    assert_eq!(
        audit
            .local_value_blocker_profile
            .distinct_declaration_count(),
        1
    );
    assert_eq!(
        audit
            .local_value_blocker_profile
            .observations
            .iter()
            .map(|row| row.path.as_str())
            .collect::<BTreeSet<_>>()
            .len(),
        1
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
    assert_eq!(count_kind("given"), 0);
    assert_eq!(count_kind("implicit val"), 0);
    assert_eq!(count_kind("inline val"), 1);
    assert_eq!(count_kind("lazy val"), 0);
    assert_eq!(
        rows.iter()
            .filter(|row| row.type_form == "explicit")
            .count(),
        0
    );
    assert_eq!(
        rows.iter()
            .filter(|row| row.type_form == "inferred")
            .count(),
        1
    );
    let count_modifier = |modifier: &str| {
        rows.iter()
            .filter(|row| row.modifiers.split(',').any(|value| value == modifier))
            .count()
    };
    assert_eq!(count_modifier("Given"), 0);
    assert_eq!(count_modifier("Implicit"), 0);
    assert_eq!(count_modifier("Lazy"), 0);
    assert_eq!(count_modifier("Inline"), 1);
    assert_eq!(count_modifier("Final"), 0);
    assert_eq!(count_modifier("Var"), 0);
    assert_eq!(audit.local_value_baseline_outcomes.len(), 21);
    assert_eq!(LOCAL_VALUE_BASELINE_SIGNATURES.len(), 21);
    assert_eq!(
        audit
            .local_value_baseline_outcomes
            .iter()
            .filter(|outcome| outcome.observation.type_form == "explicit")
            .count(),
        7,
        "the frozen cohort should retain its seven explicit local value types"
    );
    assert_eq!(
        audit
            .local_value_baseline_outcomes
            .iter()
            .filter(|outcome| outcome.observation.type_form == "inferred")
            .count(),
        14,
        "the frozen cohort should retain its fourteen inferred local value types"
    );
    assert_eq!(
        audit
            .local_value_baseline_outcomes
            .iter()
            .map(|outcome| (
                outcome.observation.path.as_str(),
                outcome.observation.declaration_tree
            ))
            .collect::<BTreeSet<_>>()
            .len(),
        14,
        "all 14 source declarations in the pinned #929 cohort must remain observable"
    );
    assert_eq!(
        audit
            .local_value_baseline_outcomes
            .iter()
            .map(|outcome| outcome.observation.path.as_str())
            .collect::<BTreeSet<_>>()
            .len(),
        8,
        "the complete pinned #929 cohort must retain all eight source files"
    );
    assert!(
        audit.local_value_baseline_outcomes.iter().all(|outcome| {
            outcome.baseline_first_blocker == LOCAL_VALUE_BASELINE_FIRST_BLOCKER
        })
    );
    let inline_local_value_rows = audit
        .local_value_baseline_outcomes
        .iter()
        .filter(|outcome| outcome.observation.declaration_kind == "inline val")
        .collect::<Vec<_>>();
    assert_eq!(inline_local_value_rows.len(), 1);
    assert_eq!(
        inline_local_value_rows[0].current_first_blocker,
        "LocalBlockDeclarationDeferred::val/var definition"
    );
    assert_eq!(
        inline_local_value_rows[0].classification,
        "residual local-value declaration blocker"
    );
    assert!(!inline_local_value_rows[0].declaration_typed);
    assert_eq!(
        audit
            .local_value_baseline_outcomes
            .iter()
            .filter(|outcome| outcome.observation.declaration_kind == "given")
            .count(),
        5
    );
    assert_eq!(
        audit
            .local_value_baseline_outcomes
            .iter()
            .filter(|outcome| outcome.observation.declaration_kind == "implicit val")
            .count(),
        12
    );
    assert_eq!(
        audit
            .local_value_baseline_outcomes
            .iter()
            .filter(|outcome| outcome.observation.declaration_kind == "lazy val")
            .count(),
        3
    );
    assert!(audit.local_value_baseline_outcomes.iter().all(|outcome| {
        outcome.observation.declaration_kind != "lazy val"
            || (outcome.current_first_blocker == "ImportQualifierNotFound"
                && !outcome.declaration_typed)
    }));
    let remaining_generic_local_value_rows = audit
        .local_value_baseline_outcomes
        .iter()
        .filter(|outcome| {
            outcome.current_first_blocker == "LocalBlockDeclarationDeferred::val/var definition"
        })
        .collect::<Vec<_>>();
    assert_eq!(
        remaining_generic_local_value_rows.len(),
        1,
        "the only remaining generic local-value blocker should be the pinned inline val"
    );
    assert_eq!(
        remaining_generic_local_value_rows[0]
            .observation
            .declaration_kind,
        "inline val"
    );
    assert_eq!(
        audit
            .failures
            .get("LocalValueModifierDeferred::anonymous given has no local term identity")
            .map_or(0, |bucket| bucket.count),
        4
    );
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
        let current = audit
            .local_method_first_blockers
            .get(&key)
            .map(String::as_str);
        assert!(
            current.is_some_and(|blocker| !blocker.is_empty()),
            "the #899 baseline method must retain a traceable current outcome: {key}"
        );
    }
    assert_eq!(audit.by_name_method_outcomes.len(), 2);
    let instantiate_cft = audit
        .by_name_method_outcomes
        .get("compiler/src/dotty/tools/dotc/core/SymUtils.scala::instantiateCFT")
        .map(String::as_str);
    assert!(instantiate_cft.is_some_and(|outcome| !outcome.is_empty()));
    assert!(
        audit
            .by_name_method_outcomes
            .get("compiler/src/dotty/tools/dotc/typer/Typer.scala::cases")
            .is_some_and(|outcome| !outcome.is_empty()),
        "the Typer.cases cohort row must retain its current outcome"
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
    print_local_value_baseline_outcomes(&audit.local_value_baseline_outcomes);
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
    print_function_with_mods_profile(&audit.function_with_mods_profile);
    print_function_with_mods_inventory(&audit.function_with_mods_inventory);
    print_impure_function_baseline_outcomes(&audit.impure_function_baseline_outcomes);
    print_missing_declared_type_profile(&audit.missing_declared_type_profile);
    print_ranked_gaps(&audit.failures);
    print_match_readiness(&audit.match_readiness);
    print_match_profile(&audit.match_profile);
    {
        let metrics = resolver_metrics.borrow();
        println!(
            "#1050 current external member metrics: requests={}, errors={}",
            metrics.external_member_requests(),
            metrics.member_errors
        );
        // Keep the #974 values as historical report data. This issue
        // re-profiles current resolver traffic, so current request/error totals
        // must be emitted and internally classified rather than pinned here.
        assert_eq!(
            metrics.member_request_shapes.values().sum::<usize>(),
            metrics.resolver_member_requests,
            "every member request must have exactly one request-shape classification"
        );
        assert_eq!(
            metrics
                .member_request_targets
                .values()
                .map(|target| target.requests)
                .sum::<usize>(),
            metrics.resolver_member_requests,
            "request volume must equal the sum of target request counts"
        );
        assert_eq!(
            metrics.member_error_kinds.values().sum::<usize>(),
            metrics.member_errors,
            "every resolver error must have one normalized root category"
        );
    }
    print_resolver_metrics(&resolver_metrics.borrow());
    print_module_import_cohort_current_outcomes(
        &resolver_metrics.borrow(),
        &audit.local_method_first_blockers,
    );
    print_package_term_module_evidence(&resolver_metrics.borrow(), classpath.clone());
    print_resolution_blocker_profile(&audit.resolution_blocker_profile);
    print_pinned_immutable_field_completion_outcomes(&root, classpath.clone());
    print_pinned_mutable_field_completion_outcomes(&root, classpath);
    println!("local_method_attempts:");
    for row in &audit.local_method_attempts {
        println!("  {row}");
    }
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

#[test]
#[ignore = "run tools/typer-classpath-corpus-audit/run with the pinned Scala checkout and explicit JAVA_HOME"]
fn pinned_scala39_import_prefix_typing_probe() {
    let root = PathBuf::from(std::env::var_os("SCALA39_ROOT").expect("SCALA39_ROOT is required"));
    assert_eq!(
        git_revision(&root),
        "777528f19a58e794c9954a42f433373472ec57f8",
        "import audit requires the repository's pinned Scala 3.9.0 source revision"
    );
    let report = PathBuf::from(
        std::env::var_os("SCALA39_IMPORT_PREFIX_BASELINE_REPORT")
            .expect("SCALA39_IMPORT_PREFIX_BASELINE_REPORT is required"),
    );
    let report = fs::read_to_string(report).expect("corrected baseline report should be readable");
    let baseline_rows = baseline_import_blocker_attempts(&report);
    assert_eq!(baseline_rows.len(), 2_573);
    let mut methods_by_file = BTreeMap::<String, BTreeSet<u32>>::new();
    for attempt in baseline_rows {
        methods_by_file
            .entry(attempt.file)
            .or_default()
            .insert(attempt.blocker_origin_tree);
    }
    let classpath = audit_classpath_from_environment();
    let metrics = Rc::new(RefCell::new(ResolverMetrics::default()));
    let mut audit = Audit::default();
    let source_package_paths = audit_source_package_inventory_for_roots(&root);
    for (index, (relative, method_trees)) in methods_by_file.iter().enumerate() {
        if index % 25 == 0 {
            eprintln!(
                "profiled {index}/{} baseline import files",
                methods_by_file.len()
            );
        }
        let path = root.join(relative);
        let text = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("cannot read {relative}: {error}"));
        audit.merge(audit_import_prefix_source_for_methods(
            &text,
            relative,
            classpath.clone(),
            Rc::clone(&metrics),
            method_trees,
            &source_package_paths,
        ));
    }
    let all_observations = audit.import_prefix_profile.observations;
    let first_segment_observations = all_observations
        .iter()
        .filter(|row| {
            row.resolver_outcome
                .ends_with("::first segment/package unresolved")
        })
        .cloned()
        .collect::<Vec<_>>();
    let files = first_segment_observations
        .iter()
        .map(|row| row.source_file.as_str())
        .collect::<BTreeSet<_>>();
    let targets = first_segment_observations
        .iter()
        .map(|row| row.target_key.as_str())
        .collect::<BTreeSet<_>>();
    eprintln!(
        "import-prefix totals: all={}, first-segment={}, files={}, targets={}",
        all_observations.len(),
        first_segment_observations.len(),
        files.len(),
        targets.len()
    );
    assert_eq!(all_observations.len(), 2_573);
    assert_eq!(first_segment_observations.len(), 2_394);
    assert_eq!(files.len(), 209);
    assert_eq!(targets.len(), 30);
    println!("AUDIT_IMPORT_PREFIX_BEGIN");
    print_import_prefix_profile(&ImportPrefixProfile {
        observations: first_segment_observations,
    });
    println!("AUDIT_IMPORT_PREFIX_END");
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
        let metrics = Rc::new(RefCell::new(ResolverMetrics {
            current_method: format!("field:{relative_path}#tree={expected_tree_index}"),
            ..ResolverMetrics::default()
        }));
        typer = typer.with_resolver(Box::new(AuditResolver {
            inner: ClasspathSymbolResolver::new(
                classpath.clone(),
                definitions,
                LoadingSession::with_packages(namer_packages),
            )
            .with_load_profile_collection(),
            metrics: metrics.clone(),
            class_path_probe: None,
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
        let metrics = metrics.borrow();
        let targets = metrics
            .resolver_requests_by_method
            .values()
            .flatten()
            .map(|observation| format!("{}::outcome={}", observation.target, observation.outcome))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>()
            .join(" | ");
        let shells = metrics
            .dependency_profiles
            .iter()
            .flat_map(|x| x.profile.identity_shells.iter().cloned())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>()
            .join(",");
        let completed = metrics
            .dependency_profiles
            .iter()
            .flat_map(|x| x.profile.completed_classes.iter().cloned())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>()
            .join(",");
        println!(
            "    field:{relative_path}#tree={expected_tree_index}::targets=[{targets}]::shells=[{shells}]::completed=[{completed}]"
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
        let metrics = Rc::new(RefCell::new(ResolverMetrics {
            current_method: format!("field:{relative_path}#tree={expected_tree_index}"),
            ..ResolverMetrics::default()
        }));
        typer = typer.with_resolver(Box::new(AuditResolver {
            inner: ClasspathSymbolResolver::new(
                classpath.clone(),
                definitions,
                LoadingSession::with_packages(namer_packages),
            )
            .with_load_profile_collection(),
            metrics: metrics.clone(),
            class_path_probe: None,
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
        let metrics = metrics.borrow();
        let targets = metrics
            .resolver_requests_by_method
            .values()
            .flatten()
            .map(|observation| format!("{}::outcome={}", observation.target, observation.outcome))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>()
            .join(" | ");
        let shells = metrics
            .dependency_profiles
            .iter()
            .flat_map(|x| x.profile.identity_shells.iter().cloned())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>()
            .join(",");
        let completed = metrics
            .dependency_profiles
            .iter()
            .flat_map(|x| x.profile.completed_classes.iter().cloned())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>()
            .join(",");
        println!(
            "    field:{relative_path}#tree={expected_tree_index}::targets=[{targets}]::shells=[{shells}]::completed=[{completed}]"
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

    let parser = classify_parse_diagnostic("parser.expected_expression");
    assert_eq!(
        parser.bucket,
        "ParserDiagnostic::parser.expected_expression"
    );
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
fn function_with_mods_profile_records_parser_shape_and_method_attribution() {
    let source_text =
        "object C { def f(x: (y: Int) ?=> Int): Int = 1; def g(x: Int ?=> Int): Int = 1 }";
    let mut store = SemanticStore::new();
    let source = SourceId::from_index(0);
    let scanner = ContextualScanner::new(source_text).unwrap();
    let parsed = parse_compilation_unit(
        SourceText::new(source_text).unwrap(),
        source,
        scanner,
        &mut store.names,
    );
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let (method_tree, _) = parsed
        .ast
        .iter()
        .find(|(_, node)| matches!(&node.kind, TreeKind::DefDef(def) if store.names.resolve(def.name.as_name().text()) == "f"))
        .expect("method f");
    let method_range = parsed.ast.get(method_tree).position.unwrap().span().range();
    let (function_tree, _) = parsed
        .ast
        .iter()
        .find(|(_, node)| {
            matches!(
                &node.kind,
                TreeKind::PhaseSpecific(UntypedNode::FunctionWithMods(_))
            ) && node.position.is_some_and(|position| {
                let range = position.span().range();
                method_range.start() <= range.start() && range.end() <= method_range.end()
            })
        })
        .expect("contextual function type");

    let observation = function_with_mods_observation(
        &parsed.ast,
        &store.names,
        source_text,
        "Fixture.scala",
        method_tree,
        function_tree.index(),
        method_tree.index(),
    )
    .expect("function observation");
    assert_eq!(observation.method, "f");
    assert_eq!(observation.parameter_count, 1);
    assert!(
        observation.parameters.contains("named:y/ValDef"),
        "{}",
        observation.parameters
    );
    assert_eq!(observation.source_kind, "contextual/pure");
    assert_eq!(observation.modifiers, "Given");
    assert_eq!(observation.attribution, "direct");
    assert_eq!(observation.blocker_origin_method, "f");
    assert_eq!(observation.first_failure_method, "f");
    assert_eq!(
        format_blocker_identity(
            &observation.blocker_origin_method,
            observation.blocker_origin_tree
        ),
        format!("f#tree={}", observation.blocker_origin_tree)
    );
    assert_eq!(
        format_blocker_identity(
            &observation.first_failure_method,
            observation.first_failure_tree
        ),
        format!("f#tree={}", observation.first_failure_tree)
    );
    assert_eq!(observation.line, 1);

    let (unnamed_method, _) = parsed
        .ast
        .iter()
        .find(|(_, node)| matches!(&node.kind, TreeKind::DefDef(def) if store.names.resolve(def.name.as_name().text()) == "g"))
        .expect("method g");
    let unnamed_range = parsed
        .ast
        .get(unnamed_method)
        .position
        .unwrap()
        .span()
        .range();
    let (unnamed_tree, _) = parsed
        .ast
        .iter()
        .find(|(_, node)| {
            matches!(
                &node.kind,
                TreeKind::PhaseSpecific(UntypedNode::FunctionWithMods(_))
            ) && node.position.is_some_and(|position| {
                let range = position.span().range();
                unnamed_range.start() <= range.start() && range.end() <= unnamed_range.end()
            })
        })
        .expect("unnamed contextual function type");
    let unnamed = function_with_mods_observation(
        &parsed.ast,
        &store.names,
        source_text,
        "Fixture.scala",
        unnamed_method,
        unnamed_tree.index(),
        unnamed_method.index(),
    )
    .expect("unnamed function observation");
    assert!(unnamed.parameters.contains("unnamed:Int/Ident"));
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
    audit_source_inner(text, path, None, false)
}

fn audit_source_with_classpath(
    text: &str,
    path: &str,
    classpath: SharedClassPath,
    metrics: Rc<RefCell<ResolverMetrics>>,
) -> Audit {
    audit_source_inner(text, path, Some((classpath, metrics)), false)
}

fn audit_import_prefix_source_for_methods(
    text: &str,
    path: &str,
    classpath: SharedClassPath,
    metrics: Rc<RefCell<ResolverMetrics>>,
    method_trees: &BTreeSet<u32>,
    source_package_paths: &BTreeSet<String>,
) -> Audit {
    audit_source_inner_filtered(
        text,
        path,
        Some((classpath, metrics)),
        true,
        Some(method_trees),
        Some(source_package_paths),
    )
}

fn audit_source_package_inventory_for_roots(root: &Path) -> BTreeSet<String> {
    let mut files = Vec::new();
    for relative_root in ["compiler/src", "library/src", "tasty/src"] {
        collect_scala_sources(&root.join(relative_root), &mut files);
    }
    files.sort();
    files
        .into_iter()
        .flat_map(|path| {
            let relative = path
                .strip_prefix(root)
                .expect("source inventory path must be under its pinned root")
                .to_string_lossy()
                .into_owned();
            let text = fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("cannot read source inventory {relative}: {error}"));
            audit_source_package_inventory(&text, &relative)
        })
        .collect()
}

fn collect_scala_sources(directory: &Path, files: &mut Vec<PathBuf>) {
    let entries = fs::read_dir(directory).unwrap_or_else(|error| {
        panic!(
            "cannot scan source inventory {}: {error}",
            directory.display()
        )
    });
    for entry in entries {
        let entry = entry.unwrap_or_else(|error| {
            panic!(
                "cannot enumerate source inventory {}: {error}",
                directory.display()
            )
        });
        let path = entry.path();
        let file_type = entry
            .file_type()
            .unwrap_or_else(|error| panic!("cannot inspect {}: {error}", path.display()));
        if file_type.is_dir() {
            collect_scala_sources(&path, files);
        } else if path
            .extension()
            .is_some_and(|extension| extension == "scala")
        {
            files.push(path);
        }
    }
}

fn audit_source_package_inventory(text: &str, path: &str) -> BTreeSet<String> {
    let source = SourceId::from_index(0);
    let mut store = SemanticStore::new();
    let _definitions = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let scanner = ContextualScanner::new(text).expect("pinned Scala source should scan");
    let parsed = parse_compilation_unit(
        SourceText::new(text).expect("pinned source should be valid UTF-8"),
        source,
        scanner,
        &mut store.names,
    );
    let index = name_compilation_unit(
        &parsed.ast,
        parsed.root,
        source,
        path,
        &mut store,
        &mut packages,
    )
    .unwrap_or_else(|error| panic!("cannot name source inventory {path}: {error:?}"));
    let mut result = BTreeSet::new();
    for (tree, node) in parsed.ast.iter() {
        if matches!(&node.kind, TreeKind::PackageDef(_))
            && let Some(symbol) = index.symbol_at(source, tree)
            && store.symbols.get(symbol).kind == dotty_core::SymbolKind::Package
        {
            let package = source_package_path(&store, symbol);
            if !package.starts_with('<') {
                let segments = package.split('.').collect::<Vec<_>>();
                for length in 1..=segments.len() {
                    result.insert(segments[..length].join("."));
                }
            }
        }
        if !matches!(&node.kind, TreeKind::DefDef(_)) {
            continue;
        }
        let Some(symbol) = index.symbol_at(source, tree) else {
            continue;
        };
        let Some(context) = index.declaration_context_of(symbol) else {
            continue;
        };
        let Some(source_context) = index.try_source_context(context) else {
            continue;
        };
        let package = source_package_path(&store, source_context.owner);
        if package.starts_with('<') {
            continue;
        }
        let segments = package.split('.').collect::<Vec<_>>();
        for length in 1..=segments.len() {
            result.insert(segments[..length].join("."));
        }
    }
    result
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
            classpath.clone(),
            definitions,
            LoadingSession::with_packages(Packages::new()),
        )
        .with_load_profile_collection(),
        metrics: Rc::new(RefCell::new(ResolverMetrics::default())),
        class_path_probe: Some(classpath),
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
    import_prefix_only: bool,
) -> Audit {
    audit_source_inner_filtered(text, path, classpath, import_prefix_only, None, None)
}

fn audit_source_inner_filtered(
    text: &str,
    path: &str,
    classpath: Option<(SharedClassPath, Rc<RefCell<ResolverMetrics>>)>,
    import_prefix_only: bool,
    method_tree_filter: Option<&BTreeSet<u32>>,
    source_package_inventory: Option<&BTreeSet<String>>,
) -> Audit {
    if let Some((_, metrics)) = &classpath {
        metrics.borrow_mut().current_file = path.to_owned();
    }
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
    let (type_tree_forms, type_tree_form_names, type_tree_nodes) =
        collect_declared_type_tree_inventory(&parsed.ast, &store.names);
    audit.type_tree_forms = type_tree_forms;
    audit.function_with_mods_inventory =
        collect_function_with_mods_inventory(&parsed.ast, &type_tree_nodes, path);
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
        let failure = classify_parse_diagnostic(diagnostic.issue().code());
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
    if !import_prefix_only && audit.local_defdefs == 0 && audit.patdef_profile.total == 0 {
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
    let root_methods = method_tree_filter.map_or(root_methods.clone(), |allowed| {
        root_methods
            .into_iter()
            .filter(|(_, tree, _, _)| allowed.contains(&tree.index()))
            .collect()
    });

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

    let empty_source_package_inventory = BTreeSet::new();
    let source_package_paths = source_package_inventory.unwrap_or(&empty_source_package_inventory);
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
    let request_metrics = classpath.as_ref().map(|(_, metrics)| Rc::clone(metrics));
    if let (Some((classpath, metrics)), Some(packages)) = (classpath, resolver_packages) {
        let resolver = ClasspathSymbolResolver::new(
            classpath.clone(),
            definitions,
            LoadingSession::with_packages(packages),
        )
        .with_load_profile_collection()
        .with_module_resolution_profile_collection();
        typer = typer.with_resolver(Box::new(AuditResolver {
            inner: resolver,
            metrics,
            class_path_probe: Some(classpath),
        }));
    }
    let mut impure_projection_cache = BTreeMap::new();
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
        if let Some(metrics) = &request_metrics {
            metrics.borrow_mut().current_method = format!(
                "{}#tree={}::{}",
                path,
                method_tree.index(),
                audit_symbol_path(typer.store(), method)
            );
        }
        let outcome = typer
            .expression_context_for(method)
            .and_then(|context| typer.type_expression(rhs, context));
        if let Some(metrics) = &request_metrics {
            let mut metrics = metrics.borrow_mut();
            metrics.current_method.clear();
            // Any checkpoint not rolled back during this method attempt has
            // reached the method's commit boundary.
            metrics.module_operation_checkpoints.clear();
        }
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
            let unsupported_type_tree = match &error {
                TyperError::UnsupportedTypeTree { tree_index, .. } => Some(*tree_index),
                _ => None,
            };
            let import_qualifier_path = match &error {
                TyperError::ImportQualifierNotFound {
                    import_tree_index, ..
                } => import_qualifier_path(&parsed.ast, &typer.store().names, *import_tree_index),
                _ => None,
            };
            let import_tree_index = match &error {
                TyperError::ImportQualifierNotFound {
                    import_tree_index, ..
                } => Some(*import_tree_index),
                _ => None,
            };
            root_failures.push(RootFailure {
                range,
                enclosing_method_tree: method_tree.index(),
                failure,
                import_qualifier_path,
                import_tree_index,
                unsupported_type_tree,
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
        let TreeKind::DefDef(definition) = &parsed.ast.get(tree).kind else {
            unreachable!()
        };
        let name = typer
            .store()
            .names
            .resolve(definition.name.as_name().text());
        let start = parsed
            .ast
            .get(tree)
            .position
            .unwrap()
            .span()
            .range()
            .start() as usize;
        let line = text[..start].bytes().filter(|byte| *byte == b'\n').count() + 1;
        let site = format!("{path}:{line}:{name}");
        if typer.source_typed_index().get(source, tree).is_some() {
            if import_prefix_only {
                continue;
            }
            audit.typed_local_defdefs += 1;
            audit.local_method_attempts.insert(format!(
                "method={path}#tree={}::site={site}::attempt={path}#tree={}::blocker=typed::typed=true",
                tree.index(), tree.index()));
            audit
                .local_method_first_blockers
                .insert(format!("{path}#tree={}", tree.index()), "typed".to_owned());
            record_local_value_baseline_outcomes(
                &mut audit,
                LocalValueBaselineAuditContext {
                    arena: &parsed.ast,
                    names: &typer.store().names,
                    source_text: text,
                    path,
                    method_tree: tree,
                    typer: &typer,
                    source,
                },
                "typed",
            );
            record_impure_function_baseline_outcome(
                &mut audit,
                ImpureFunctionBaselineAuditContext {
                    arena: &parsed.ast,
                    typer: &typer,
                    source_text: text,
                    path,
                    classpath: probe_classpath.clone(),
                    projection_cache: &mut impure_projection_cache,
                    method_tree: tree,
                    first_blocker: "typed",
                    method_typed: true,
                },
            );
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
                import_qualifier_path,
                import_tree_index,
                unsupported_type_tree,
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
                        failure.import_qualifier_path.clone(),
                        failure.import_tree_index,
                        failure.unsupported_type_tree,
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
                        None,
                        None,
                        None,
                    )
                });
            if import_prefix_only {
                if kind.bucket == "ImportQualifierNotFound"
                    && let (Some(import_tree_index), Some(qualifier), Some(metrics)) = (
                        import_tree_index,
                        import_qualifier_path.as_deref(),
                        request_metrics.as_ref(),
                    )
                {
                    let request_method = blocker_method_tree
                        .map(|method_tree| format!("{path}#tree={method_tree}::"));
                    let trace = request_method.map_or_else(Vec::new, |prefix| {
                        metrics
                            .borrow()
                            .resolver_requests_by_method
                            .iter()
                            .filter(|(method, _)| method.starts_with(&prefix))
                            .flat_map(|(_, requests)| requests.iter().cloned())
                            .collect()
                    });
                    if let Some(observation) = import_prefix_observation(
                        &parsed.ast,
                        typer.store(),
                        &index,
                        source,
                        text,
                        path,
                        tree,
                        blocker_method_tree,
                        import_tree_index,
                        qualifier,
                        &trace,
                        source_package_paths,
                    ) {
                        audit.import_prefix_profile.observations.push(observation);
                    }
                }
                continue;
            }
            let attempt = blocker_method_tree
                .map_or_else(|| "<none>".to_owned(), |id| format!("{path}#tree={id}"));
            audit.local_method_attempts.insert(format!(
                "method={path}#tree={}::site={site}::attempt={attempt}::blocker={}::typed=false",
                tree.index(),
                kind.bucket
            ));
            record_impure_function_baseline_outcome(
                &mut audit,
                ImpureFunctionBaselineAuditContext {
                    arena: &parsed.ast,
                    typer: &typer,
                    source_text: text,
                    path,
                    classpath: probe_classpath.clone(),
                    projection_cache: &mut impure_projection_cache,
                    method_tree: tree,
                    first_blocker: &kind.bucket,
                    method_typed: false,
                },
            );
            if kind.bucket == "UnsupportedTypeTree::FunctionWithMods"
                && let (Some(blocker_tree), Some(origin_tree)) =
                    (unsupported_type_tree, blocker_method_tree)
                && let Some(observation) = function_with_mods_observation(
                    &parsed.ast,
                    &typer.store().names,
                    text,
                    path,
                    tree,
                    blocker_tree,
                    origin_tree,
                )
            {
                audit
                    .function_with_mods_profile
                    .observations
                    .insert(observation);
            }
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
            if let Some(metrics) = &request_metrics {
                let request_method =
                    blocker_method_tree.map(|method_tree| format!("{path}#tree={method_tree}::"));
                let trace = request_method.map_or_else(Vec::new, |prefix| {
                    metrics
                        .borrow()
                        .resolver_requests_by_method
                        .iter()
                        .filter(|(method, _)| method.starts_with(&prefix))
                        .flat_map(|(_, requests)| requests.iter().cloned())
                        .collect()
                });
                audit.resolution_blocker_profile.record_attempt(
                    &kind.bucket,
                    path,
                    tree.index(),
                    &trace,
                    import_qualifier_path.as_deref(),
                );
            }
            audit
                .local_method_first_blockers
                .insert(format!("{path}#tree={}", tree.index()), kind.bucket.clone());
            record_local_value_baseline_outcomes(
                &mut audit,
                LocalValueBaselineAuditContext {
                    arena: &parsed.ast,
                    names: &typer.store().names,
                    source_text: text,
                    path,
                    method_tree: tree,
                    typer: &typer,
                    source,
                },
                &kind.bucket,
            );
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
    if import_prefix_only {
        return audit;
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

fn print_function_with_mods_profile(profile: &FunctionWithModsProfile) {
    let mut groups = BTreeMap::<(String, String, usize), Vec<&FunctionWithModsObservation>>::new();
    for observation in &profile.observations {
        groups
            .entry((
                observation.modifiers.clone(),
                observation.erased_params.clone(),
                observation.parameter_count,
            ))
            .or_default()
            .push(observation);
    }
    println!("function_with_mods_first_blocker_profile:");
    for ((modifiers, erased, arity), observations) in groups {
        let distinct_trees = observations
            .iter()
            .map(|row| (row.path.as_str(), row.function_tree))
            .collect::<BTreeSet<_>>();
        let files = observations
            .iter()
            .map(|row| row.path.as_str())
            .collect::<BTreeSet<_>>();
        let locations = observations
            .iter()
            .map(|row| format!("{}:{}", row.path, row.line))
            .collect::<BTreeSet<_>>();
        let examples = observations
            .iter()
            .map(|row| format!("{}#tree={}", row.path, row.function_tree))
            .collect::<BTreeSet<_>>();
        println!(
            "  modifiers=[{modifiers}] erased=[{erased}] arity={arity}: observations={} distinct_trees={} files={} arities=[{arity}] examples=[{}] locations=[{}]",
            observations.len(),
            distinct_trees.len(),
            files.len(),
            examples.into_iter().take(5).collect::<Vec<_>>().join(", "),
            locations.into_iter().collect::<Vec<_>>().join(", ")
        );
    }
    println!("  observations:");
    for row in &profile.observations {
        println!(
            "    path={} method={}#tree={} function_tree={} blocker_origin={} first_failure={} attribution={} line={} span={} syntax={:?} params={} [{}] result={} kind={} modifiers=[{}] visibility={} annotations={} erased=[{}]",
            row.path,
            row.method,
            row.method_tree,
            row.function_tree,
            format_blocker_identity(&row.blocker_origin_method, row.blocker_origin_tree),
            format_blocker_identity(&row.first_failure_method, row.first_failure_tree),
            row.attribution,
            row.line,
            row.span,
            row.syntax,
            row.parameter_count,
            row.parameters,
            row.result_kind,
            row.source_kind,
            row.modifiers,
            row.visibility,
            row.annotations,
            row.erased_params
        );
    }
}

fn print_function_with_mods_inventory(inventory: &FunctionWithModsInventory) {
    println!("function_with_mods_structural_inventory:");
    println!("  total={}", inventory.total);
    println!(
        "  plain_ordinary_function_trees={} files={}",
        inventory.plain_function_trees,
        inventory.plain_function_files.len()
    );
    println!("  files={}", inventory.files.len());
    println!("  erased_parameters={}", inventory.erased);
    println!(
        "  captures_and_result_adjacency={}",
        inventory.capture_result_adjacency
    );
    println!("  modifier_sets:");
    for (modifiers, count) in &inventory.modifiers {
        println!("    [{modifiers}]={count}");
    }
    println!("  source_kinds:");
    for (kind, count) in &inventory.source_kinds {
        let files = inventory
            .source_kind_files
            .get(kind)
            .map_or(0, BTreeSet::len);
        println!("    {kind}={count} files={files}");
    }
}

fn print_impure_function_baseline_outcomes(rows: &BTreeSet<ImpureFunctionBaselineOutcome>) {
    println!("impure_function_baseline_before_after:");
    for row in rows {
        let classification = if row.method_typed {
            "fully typed"
        } else if row.first_blocker == "UnsupportedTypeTree::FunctionWithMods" {
            "residual FunctionWithMods-specific blocker"
        } else if matches!(
            row.first_blocker.as_str(),
            "ImportQualifierNotFound"
                | "TypeNameNotFound"
                | "TermNameNotFound"
                | "MemberNotFound"
                | "MemberLookup"
                | "SymbolResolution"
        ) {
            "resolution/classpath blocker"
        } else {
            "deeper Typer semantic blocker"
        };
        println!(
            "  path={} method={}#tree={} function_tree={} line={} span={} syntax={:?} modifiers=[{}] erased=[{}] arity={} kind={} semantic_family={} expected_family={} attribution={} projection={} projection_error={:?} first_blocker={} method_typed={} classification={}",
            row.path,
            row.method,
            row.method_tree,
            row.function_tree,
            row.line,
            row.span,
            row.syntax,
            row.modifiers,
            row.erased_params,
            row.arity,
            row.source_kind,
            row.semantic_family,
            row.expected_semantic_family,
            row.attribution,
            if row.projection_completed {
                "completed"
            } else {
                "not_completed"
            },
            row.projection_error,
            row.first_blocker,
            row.method_typed,
            classification
        );
    }
}

fn format_blocker_identity(method: &str, tree: u32) -> String {
    format!("{method}#tree={tree}")
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
        "  external_member_first_seen_in_file_session={}",
        metrics.external_member_first_seen
    );
    println!(
        "  external_member_same_file_session_reuse={}",
        metrics.external_member_session_reuse
    );
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
    println!(
        "  class_prefix_member_successes_by_kind={:?}",
        metrics.class_member_successes_by_kind
    );
    println!(
        "  class_prefix_requests_after_shell_completion={}",
        metrics.class_member_after_shell_completion
    );
    println!(
        "  class_prefix_distinct_classes={}",
        metrics.class_member_prefixes.len()
    );
    println!(
        "  class_prefix_distinct_member_keys={}",
        metrics.class_member_keys.len()
    );
    println!(
        "  class_prefix_completed_reuse_requests={}",
        metrics.class_member_completed_prefix_reuse
    );
    println!("  source_member_reuse={}", metrics.member_source_reuse);
    println!("  external_member_unresolved={}", metrics.member_unresolved);
    println!("  external_member_errors={}", metrics.member_errors);
    println!(
        "  package_term_requests_with_type_class_resource={}",
        metrics.package_term_type_class_resource
    );
    println!(
        "  package_term_distinct_targets_with_type_class_resource={}",
        metrics.package_term_type_class_targets.len()
    );
    println!(
        "  package_term_requests_without_type_class_resource={}",
        metrics.package_term_without_type_class_resource
    );
    println!(
        "  package_type_requests_with_class_resource={}",
        metrics.package_type_class_resource
    );
    println!(
        "  package_type_distinct_targets_with_class_resource={}",
        metrics.package_type_class_targets.len()
    );
    println!(
        "  package_classpath_probe_errors={}",
        metrics.package_classpath_probe_errors
    );
    println!(
        "  package_term_companion_object_resource_requests={}",
        metrics.package_term_companion_resource
    );
    println!(
        "  package_term_distinct_targets_with_companion_object_resource={}",
        metrics.package_term_companion_targets.len()
    );
    println!(
        "  package_object_or_dollar_looking_distinct_targets={}",
        metrics.package_term_object_looking_targets.len()
    );
    println!(
        "  typer_module_identity_resolver_calls={} outcomes={:?}",
        metrics.module_identity_resolver_calls, metrics.module_identity_resolver_outcomes
    );
    println!(
        "  typer_module_completion_resolver_calls={} outcomes={:?}",
        metrics.module_completion_resolver_calls, metrics.module_completion_resolver_outcomes
    );
    let module_profile = &metrics.module_resolution;
    let completed_module_classes = completed_module_class_targets(metrics);
    println!("module_resolution_profile:");
    println!(
        "  package_term_requests={} object_successes={} non_object_results={} unresolved={} errors={}",
        module_profile.package_term_requests,
        module_profile.package_term_successes,
        module_profile.package_term_non_object_results,
        module_profile.package_term_unresolved,
        module_profile.package_term_errors
    );
    println!(
        "  new_object_identities={} reused_object_identities={}",
        module_profile.new_object_identities, module_profile.reused_object_identities
    );
    println!(
        "  completed_module_classes={} module_member_successes={}",
        completed_module_classes.len(),
        module_profile.module_member_successes
    );
    println!(
        "  external_package_term_successes={} distinct_external_objects={}",
        module_profile.package_term_successes,
        module_profile.resolved_object_targets.len()
    );
    println!(
        "  external_module_class_completions={} external_module_member_successes={} distinct_external_module_members={}",
        completed_module_classes.len(),
        module_profile.module_member_successes,
        module_profile.resolved_member_targets.len()
    );
    println!(
        "  distinct_package_term_targets={} distinct_module_targets={} distinct_member_targets={}",
        module_profile.package_term_targets.len(),
        module_profile.module_targets.len(),
        module_profile.member_targets.len()
    );
    for target in &module_profile.package_term_targets {
        println!("  package_term_target={target}");
    }
    for target in &module_profile.module_targets {
        println!("  module_target={target}");
    }
    for target in &module_profile.member_targets {
        println!("  member_target={target}");
    }
    let mut module_observations = metrics.module_package_term_observations.clone();
    module_observations.sort();
    println!("module_package_term_observations:");
    println!("  observations={}", module_observations.len());
    println!(
        "  request_key\tsource_file\tenclosing_attempt\tresolution\tobject_origin\tobject_identity\tmodule_class_identity"
    );
    for observation in module_observations {
        println!(
            "  {}\t{}\t{}\t{}\t{}\t{}\t{}",
            observation.request_key,
            observation.source_file,
            observation.enclosing_attempt,
            observation.resolution,
            observation.object_origin,
            tsv_or_dash(&observation.object_identity),
            tsv_or_dash(&observation.module_class_identity)
        );
    }
    let mut module_operation_observations = metrics.module_operation_observations.clone();
    module_operation_observations.sort();
    println!("module_operation_observations:");
    println!("  observations={}", module_operation_observations.len());
    println!(
        "  operation\tsource_file\tenclosing_attempt\tresult\tresult_state\tmodule_state\ttransaction_state\terror_category\tobject_origin\tobject_identity\tmodule_class_identity"
    );
    for observation in module_operation_observations {
        println!(
            "  {}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            observation.operation,
            observation.source_file,
            observation.enclosing_attempt,
            observation.result,
            observation.result_state,
            observation.module_state,
            observation.transaction_state,
            observation.error_category.unwrap_or("-"),
            observation.object_origin,
            tsv_or_dash(&observation.object_identity),
            tsv_or_dash(&observation.module_class_identity)
        );
    }
    let mut module_member_observations = metrics.module_member_observations.clone();
    module_member_observations.sort();
    println!("module_member_observations:");
    println!("  observations={}", module_member_observations.len());
    println!(
        "  source_file\tenclosing_attempt\tmodule_class_identity\trequest_key\tresult\tresult_symbol\tmodule_state_before\tmodule_state_after\ttransaction_state"
    );
    for observation in module_member_observations {
        println!(
            "  {}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            observation.source_file,
            observation.enclosing_attempt,
            observation.module_class_identity,
            observation.request_key,
            observation.result,
            tsv_or_dash(&observation.result_symbol),
            observation.module_state_before,
            observation.module_state_after,
            observation.transaction_state
        );
    }
    let mut package_term_rows = metrics.package_term_request_rows.clone();
    package_term_rows.sort();
    println!("package_term_request_rows:");
    println!("  observations={}", package_term_rows.len());
    println!(
        "  distinct_request_keys={}",
        package_term_rows
            .iter()
            .map(|row| row.request_key.as_str())
            .collect::<BTreeSet<_>>()
            .len()
    );
    println!(
        "  request_key\tprefix_kind\tpackage\tname\tmember_space\tselector\tsource_file\tenclosing_attempt\tresolver_outcome\ttype_resource\tcompanion_resource"
    );
    for row in package_term_rows {
        println!(
            "  {}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            row.request_key,
            row.prefix_kind,
            row.package,
            row.name,
            row.member_space,
            row.selector,
            row.source_file,
            row.enclosing_attempt,
            row.outcome,
            tsv_or_dash(&row.type_resource),
            tsv_or_dash(&row.companion_resource)
        );
    }
    println!("  distinct_packages={}", metrics.packages.len());
    println!("  distinct_classes={}", metrics.classes.len());
    println!("  distinct_members={}", metrics.members.len());
    print_dependency_profiles(metrics);
    println!("resolver_observations_by_enclosing_method:");
    for (method, observations) in &metrics.resolver_requests_by_method {
        let mut counts = BTreeMap::new();
        for observation in observations {
            *counts
                .entry((&observation.target, &observation.outcome))
                .or_insert(0) += 1;
        }
        println!(
            "    {method}: requests={}, distinct_request_outcomes={}",
            observations.len(),
            counts.len()
        );
        for ((target, outcome), count) in counts {
            println!("      target={target}::outcome={outcome}={count}");
        }
    }

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
    println!("  member_error_distinct_targets:");
    for (kind, targets) in &metrics.member_error_distinct_targets {
        println!(
            "    {kind}={}::files={}",
            targets.len(),
            metrics
                .member_error_files
                .get(kind)
                .map_or(0, BTreeSet::len)
        );
    }
    println!("  most_requested_unresolved_member_names:");
    let mut unresolved = metrics.unresolved_member_names.iter().collect::<Vec<_>>();
    unresolved.sort_by(|(name_a, count_a), (name_b, count_b)| {
        count_b.cmp(count_a).then(name_a.cmp(name_b))
    });
    for (name, count) in unresolved.into_iter().take(20) {
        println!("    {name}={count}");
    }
    println!("  member_request_shapes:");
    for (shape, count) in &metrics.member_request_shapes {
        println!("    {shape}={count}");
    }
    println!("  member_request_unknown_prefix_identity:");
    for (shape, count) in &metrics.member_request_unknown_identity_shapes {
        println!("    {shape}={count}");
    }
    println!("  member_request_outcomes:");
    let mut outcomes = BTreeMap::<String, usize>::new();
    for target in metrics.member_request_targets.values() {
        for (outcome, count) in &target.outcomes {
            *outcomes.entry(outcome.clone()).or_default() += count;
        }
    }
    for (outcome, count) in outcomes {
        println!("    {outcome}={count}");
    }
    println!("  member_request_outcome_distinct_targets:");
    for (family, targets) in &metrics.member_request_outcome_targets {
        println!(
            "    {family}={}::files={}",
            targets.len(),
            metrics
                .member_request_outcome_files
                .get(family)
                .map_or(0, BTreeSet::len)
        );
    }
    println!("  unresolved_member_kinds:");
    for (kind, count) in &metrics.unresolved_member_kinds {
        println!("    {kind}={count}");
    }
    println!("  member_request_key_summary:");
    for (shape, summary) in member_request_shape_summaries(metrics) {
        let distinct = summary.distinct_keys.len();
        let (repeat, known_repeat) = member_request_repeat_factors(&summary);
        println!(
            "    {shape}: requests={}, known_requests={}, distinct_keys={distinct}, unknown_identity_requests={}, owners={}, names={}, files={}, repeated_hit_factor={repeat}, known_repeated_hit_factor={known_repeat}, outcomes={:?}",
            summary.requests,
            summary.known_requests,
            summary.unknown_identity_requests,
            summary.owners.len(),
            summary.names.len(),
            summary.files.len(),
            summary.outcomes
        );
    }
    println!("  member_requests_by_enclosing_method:");
    for (method, requests) in &metrics.member_requests_by_method {
        let volume = requests.values().sum::<usize>();
        println!(
            "    {method}: requests={volume}, distinct_request_outcomes={}",
            requests.len()
        );
        for (key, count) in requests {
            println!("      {key}={count}");
        }
    }
    println!("  normalized_member_error_roots:");
    for (key, count) in &metrics.member_error_roots {
        println!("    {key}={count}");
    }
    println!("  member_error_wrappers:");
    for (key, count) in &metrics.member_error_wrappers {
        println!("    {key}={count}");
    }
    println!("  dependency_failure_chains:");
    for (key, count) in &metrics.dependency_failure_chains {
        println!("    {key}={count}");
    }
    println!("  tasty_unresolved_supertype_failures:");
    for (key, count) in &metrics.tasty_supertype_failures {
        println!("    {key}={count}");
    }
    println!("  incomplete_prefix_member_requests:");
    println!(
        "    distinct_incomplete_prefix_targets={}",
        metrics.incomplete_prefix_states.len()
    );
    for (key, count) in &metrics.incomplete_prefix_states {
        println!("    {key}={count}");
    }
    println!("  incomplete_prefix_classpath_binary_recovery:");
    for (key, count) in &metrics.incomplete_prefix_binary_resources {
        println!("    {key}={count}");
    }
}

fn print_package_term_module_evidence(metrics: &ResolverMetrics, classpath: SharedClassPath) {
    let mut targets = BTreeMap::<String, (String, String)>::new();
    for row in &metrics.package_term_request_rows {
        targets.insert(
            row.request_key.clone(),
            (row.package.clone(), row.name.clone()),
        );
    }

    println!("package_term_module_evidence:");
    println!("  distinct_request_keys={}", targets.len());
    println!(
        "  request_key\tselected_resource\tclassification\tobject_identity\tmodule_class_identity\tclass_identity\tvisibility\tdetail"
    );
    for (key, (package, name)) in targets {
        let internal = if package.is_empty() {
            name.clone()
        } else {
            format!("{}/{name}", package.replace('.', "/"))
        };
        let binary = BinaryName::from_internal(internal);
        let resource = match classpath.0.find_class(&binary) {
            Ok(Some(resource)) => resource,
            Ok(None) => {
                println!(
                    "  {key}\tabsent\tno canonical object evidence\t-\t-\t-\t-\tno exact type-side classpath resource"
                );
                continue;
            }
            Err(error) => {
                println!(
                    "  {key}\terror\tunsupported evidence\t-\t-\t-\t-\t{}",
                    error.code()
                );
                continue;
            }
        };
        let selected_resource = class_resource_label(&resource);
        if resource.format() != ClassFormat::Tasty {
            println!(
                "  {key}\t{selected_resource}\tunsupported evidence\t-\t-\t-\t-\tclass-only resource; no TASTy object identity"
            );
            continue;
        }
        let file = match dotty_tasty::tasty::TastyFile::parse_scala_3_9(resource.bytes()) {
            Ok(file) => file,
            Err(error) => {
                println!(
                    "  {key}\t{selected_resource}\tmalformed/ambiguous\t-\t-\t-\t-\tTASTy parse failed: {error}"
                );
                continue;
            }
        };
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let (entered, failure) = {
            let mut unpickler = TastyUnpickler::new(&file, &mut store, definitions);
            match unpickler.enter_symbols() {
                Ok(index) => (Some(index.entered_symbols().to_vec()), None),
                Err(error) => (None, Some(format!("TASTy semantic entry failed: {error}"))),
            }
        };
        let Some(entered) = entered else {
            println!(
                "  {key}\t{selected_resource}\tunsupported evidence\t-\t-\t-\t-\t{}",
                failure
                    .unwrap_or_else(|| "unknown TASTy entry failure".to_owned())
                    .replace(['\t', '\n'], " ")
            );
            continue;
        };

        let requested_object = format!("{package}.{name}");
        let requested_module_class = format!("{requested_object}$");
        let mut objects = BTreeSet::new();
        let mut module_classes = BTreeSet::new();
        let mut classes = BTreeSet::new();
        let mut visibility = BTreeSet::new();
        for symbol in entered {
            let entry = store.symbols.get(symbol);
            let path = audit_symbol_path(&store, symbol);
            match entry.kind {
                dotty_core::SymbolKind::Object if path == requested_object => {
                    objects.insert(path);
                    visibility.insert(format!("{:?}", entry.visibility));
                }
                dotty_core::SymbolKind::ModuleClass if path == requested_module_class => {
                    module_classes.insert(path);
                }
                dotty_core::SymbolKind::Class | dotty_core::SymbolKind::Trait
                    if path == requested_object =>
                {
                    classes.insert(path);
                }
                _ => {}
            }
        }
        let classification = match objects.len() {
            0 => "TASTy has no matching object definition",
            1 if name == "package" => "package object",
            1 if !classes.is_empty() => "class with companion object",
            1 => "standalone object",
            _ => "malformed/ambiguous",
        };
        let detail = if objects.len() == 1 && module_classes.len() != 1 {
            "object identity found without a unique module-class identity".to_owned()
        } else {
            String::new()
        };
        println!(
            "  {key}\t{selected_resource}\t{classification}\t{}\t{}\t{}\t{}\t{}",
            tsv_or_dash(&objects.into_iter().collect::<Vec<_>>().join(",")),
            tsv_or_dash(&module_classes.into_iter().collect::<Vec<_>>().join(",")),
            tsv_or_dash(&classes.into_iter().collect::<Vec<_>>().join(",")),
            tsv_or_dash(&visibility.into_iter().collect::<Vec<_>>().join(",")),
            tsv_or_dash(&detail)
        );
    }
}

fn tsv_or_dash(value: &str) -> &str {
    if value.is_empty() { "-" } else { value }
}

fn print_module_import_cohort_current_outcomes(
    metrics: &ResolverMetrics,
    current_first_blockers: &BTreeMap<String, String>,
) {
    const FROZEN: &str = include_str!(
        "../../../docs/typer-classpath-corpus-audit-3.9.0-package-term-first-blockers.tsv"
    );
    let validated_targets = metrics
        .module_package_term_observations
        .iter()
        .filter(|observation| observation.resolution == "validated_module_identity")
        .map(|observation| observation.request_key.as_str())
        .collect::<BTreeSet<_>>();
    let rows = FROZEN.lines().skip(1).filter(|line| !line.is_empty());
    let mut outcomes = Vec::new();
    for row in rows {
        let columns = row.split('\t').collect::<Vec<_>>();
        assert_eq!(columns.len(), 7, "frozen #1046 first-blocker row shape");
        let blocker = columns[0];
        let file = columns[1];
        let method = columns[2];
        let qualifier = columns[5];
        let Some((package, name)) = qualifier.rsplit_once('.') else {
            continue;
        };
        let request_key = format!("prefix={package}::namespace=term::name={name}::space=Prefix");
        let current = current_first_blockers
            .get(method)
            .map(String::as_str)
            .unwrap_or("method_not_observed");
        let identity_validated = validated_targets.contains(request_key.as_str());
        let progressed_past_baseline = current != blocker;
        let fully_typed = current == "typed";
        let deeper_failure = progressed_past_baseline && !fully_typed;
        outcomes.push((
            blocker,
            file,
            method,
            qualifier,
            request_key,
            identity_validated,
            progressed_past_baseline,
            deeper_failure,
            fully_typed,
            current,
        ));
    }
    let validated = outcomes.iter().filter(|row| row.5).count();
    let progressed = outcomes.iter().filter(|row| row.6).count();
    let deeper = outcomes.iter().filter(|row| row.7).count();
    let typed = outcomes.iter().filter(|row| row.8).count();
    let current_blockers = outcomes.iter().fold(BTreeMap::new(), |mut counts, row| {
        *counts.entry(row.9).or_insert(0usize) += 1;
        counts
    });
    assert_eq!(
        outcomes.len(),
        369,
        "the frozen #1046 cohort must stay intact"
    );
    assert!(validated <= outcomes.len());
    assert!(progressed <= outcomes.len());
    assert!(deeper <= progressed);
    assert!(typed <= outcomes.len());
    assert_eq!(current_blockers.values().sum::<usize>(), outcomes.len());
    println!("frozen_module_import_cohort_current_outcomes:");
    println!("  baseline_rows={}", outcomes.len());
    println!("  rows_with_validated_module_identity={validated}");
    println!("  rows_past_their_baseline_first_blocker={progressed}");
    println!("  rows_with_a_new_first_blocker_but_not_fully_typed={deeper}");
    println!("  rows_fully_typed={typed}");
    println!(
        "  baseline_first_blocker\tfile\tmethod\tfailed_qualifier\trequest_key\tmodule_identity_validated\tpast_baseline_blocker\tnew_blocker_not_typed\tfully_typed\tcurrent_first_blocker"
    );
    for row in outcomes {
        println!(
            "  {}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            row.0, row.1, row.2, row.3, row.4, row.5, row.6, row.7, row.8, row.9
        );
    }
}

fn print_resolution_blocker_profile(profile: &ResolutionBlockerProfile) {
    println!("resolution_blocker_attribution:");
    println!("  observations={}", profile.observations.len());
    let package_term_first_blockers = profile
        .observations
        .iter()
        .filter(|observation| {
            matches!(
                observation.blocker.as_str(),
                "ImportQualifierNotFound" | "TermNameNotFound"
            ) && observation.cause == "term/object materialization gap"
        })
        .collect::<Vec<_>>();
    println!("package_term_first_blocker_rows:");
    println!("  observations={}", package_term_first_blockers.len());
    println!(
        "  blocker\tfile\tmethod\tattribution\trequest_attempts\tfailed_qualifier\trequest_targets"
    );
    for observation in package_term_first_blockers {
        let has_direct = observation.request_attempts.iter().any(|attempt| {
            attempt == &observation.method
                || attempt.starts_with(&format!("{}::", observation.method))
        });
        let has_inherited = observation.request_attempts.iter().any(|attempt| {
            attempt != &observation.method
                && !attempt.starts_with(&format!("{}::", observation.method))
        });
        let attribution = match (has_direct, has_inherited) {
            (true, true) => "mixed",
            (true, false) => "direct",
            (false, true) => "inherited",
            (false, false) => "unknown",
        };
        println!(
            "  {}\t{}\t{}\t{}\t{}\t{}\t{}",
            observation.blocker,
            observation.file,
            observation.method,
            attribution,
            observation
                .request_attempts
                .iter()
                .cloned()
                .collect::<Vec<_>>()
                .join(";"),
            if observation.failed_qualifier.is_empty() {
                "<not-applicable-or-unavailable>"
            } else {
                &observation.failed_qualifier
            },
            observation
                .targets
                .iter()
                .cloned()
                .collect::<Vec<_>>()
                .join(";")
        );
    }
    let mut groups = BTreeMap::<(String, String), ResolverCauseGroup>::new();
    for observation in &profile.observations {
        let group = groups
            .entry((observation.blocker.clone(), observation.cause.clone()))
            .or_default();
        group.count += 1;
        group.files.insert(observation.file.clone());
        group.targets.extend(observation.targets.iter().cloned());
        group
            .candidate_causes
            .extend(observation.candidate_causes.iter().cloned());
        group
            .materialized_prefixes
            .extend(observation.materialized_prefixes.iter().cloned());
        if !observation.materialized_prefixes.is_empty() {
            group
                .methods_with_materialized_prefix
                .insert(observation.method.clone());
        }
    }
    for ((blocker, cause), group) in groups {
        let targets = group.targets.iter().take(12).cloned().collect::<Vec<_>>();
        let prefixes = group
            .materialized_prefixes
            .iter()
            .take(12)
            .cloned()
            .collect::<Vec<_>>();
        let candidates = group.candidate_causes.iter().cloned().collect::<Vec<_>>();
        println!(
            "  blocker={blocker}::cause={cause} count={} files={} distinct_targets={} targets=[{}] methods_with_materialized_prefix={} distinct_materialized_prefixes={} prefixes=[{}] candidate_causes=[{}]",
            group.count,
            group.files.len(),
            group.targets.len(),
            targets.join(", "),
            group.methods_with_materialized_prefix.len(),
            group.materialized_prefixes.len(),
            prefixes.join(", "),
            candidates.join(", ")
        );
    }
}

fn print_import_prefix_profile(profile: &ImportPrefixProfile) {
    let mut observations = profile.observations.clone();
    observations.sort();
    println!("import_prefix_observations:");
    println!("  observations={}", observations.len());
    println!(
        "  source_file\tmethod\tblocker_origin\timport_tree\tline\tqualifier_source\tqualifier_segments\tcontext_owner\tcontext_package\tnamespace\tsource_lookup\tsource_package_registry_lookup\tresolved_prefix\tfirst_failing_segment\trequest_kind\tresolver_outcome\ttarget_key"
    );
    for row in observations {
        println!(
            "  {}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            row.source_file,
            row.method,
            row.blocker_origin,
            row.import_tree,
            row.import_line,
            tsv_or_dash(&row.qualifier_source),
            tsv_or_dash(&row.qualifier_segments),
            tsv_or_dash(&row.context_owner),
            tsv_or_dash(&row.context_package),
            tsv_or_dash(&row.namespace),
            tsv_or_dash(&row.source_lookup),
            tsv_or_dash(&row.source_package_registry_lookup),
            tsv_or_dash(&row.resolved_prefix),
            tsv_or_dash(&row.first_failing_segment),
            tsv_or_dash(&row.request_kind),
            tsv_or_dash(&row.resolver_outcome),
            tsv_or_dash(&row.target_key),
        );
    }
}

fn baseline_import_blocker_attempts(report: &str) -> BTreeSet<BaselineImportAttempt> {
    let start = report
        .rfind("local_method_attempts:")
        .expect("corrected audit contains the local_method_attempts section");
    let mut rows = BTreeSet::new();
    for line in report[start..].lines().skip(1) {
        let Some(line) = line.strip_prefix("  method=") else {
            if line.starts_with("  ") {
                continue;
            }
            break;
        };
        let fields = line.split("::").collect::<Vec<_>>();
        if fields.get(3).copied() != Some("blocker=ImportQualifierNotFound") {
            continue;
        }
        let (file, tree) = fields[0]
            .rsplit_once("#tree=")
            .expect("method attempt has a stable source tree key");
        let method_tree = tree
            .parse::<u32>()
            .expect("method tree key contains a numeric source tree index");
        let attempt = fields
            .get(2)
            .and_then(|field| field.strip_prefix("attempt="))
            .expect("method attempt records its first-blocker origin");
        let (_, origin_tree) = attempt
            .rsplit_once("#tree=")
            .expect("blocker origin contains a source tree index");
        let blocker_origin_tree = origin_tree
            .parse::<u32>()
            .expect("blocker origin tree is numeric");
        rows.insert(BaselineImportAttempt {
            file: file.to_owned(),
            method_tree,
            blocker_origin_tree,
        });
    }
    rows
}

fn print_dependency_profiles(metrics: &ResolverMetrics) {
    let mut depths = metrics
        .dependency_profiles
        .iter()
        .map(|observation| observation.profile.max_dependency_depth)
        .collect::<Vec<_>>();
    depths.sort_unstable();
    let median_depth = if depths.is_empty() {
        0.0
    } else if depths.len() % 2 == 0 {
        (depths[depths.len() / 2 - 1] + depths[depths.len() / 2]) as f64 / 2.0
    } else {
        depths[depths.len() / 2] as f64
    };
    let p95_depth = if depths.is_empty() {
        0
    } else {
        depths[(depths.len() * 95).div_ceil(100).saturating_sub(1)]
    };
    let max_classes = metrics
        .dependency_profiles
        .iter()
        .map(|observation| observation.profile.classes.len())
        .max()
        .unwrap_or(0);

    println!("dependency_load_profile:");
    println!("  top_level_class_loads={}", depths.len());
    println!(
        "  on_demand_completions={}",
        metrics
            .dependency_profiles
            .iter()
            .filter(|x| x.profile.root_was_shell
                && x.profile.completed_classes.contains(&x.profile.root))
            .count()
    );
    for (label, count) in [
        (
            "identity_shell_creations",
            metrics
                .dependency_profiles
                .iter()
                .map(|x| x.profile.shells_created)
                .sum::<usize>(),
        ),
        (
            "identity_shell_reuses",
            metrics
                .dependency_profiles
                .iter()
                .map(|x| x.profile.shells_reused)
                .sum::<usize>(),
        ),
        (
            "member_type_dependencies_deferred",
            metrics
                .dependency_profiles
                .iter()
                .map(|x| x.profile.member_type_references)
                .sum::<usize>(),
        ),
        (
            "hierarchy_eager_loads",
            metrics
                .dependency_profiles
                .iter()
                .map(|x| x.profile.hierarchy_loads)
                .sum::<usize>(),
        ),
    ] {
        println!("  {label}={count}");
    }
    for (label, maximum) in [
        (
            "max_distinct_fully_completed_classes_per_request",
            metrics
                .dependency_profiles
                .iter()
                .map(|x| x.profile.completed_classes.len())
                .max()
                .unwrap_or(0),
        ),
        (
            "max_identity_shells_created_per_request",
            metrics
                .dependency_profiles
                .iter()
                .map(|x| x.profile.shells_created)
                .max()
                .unwrap_or(0),
        ),
    ] {
        println!("  {label}={maximum}");
    }
    for (label, total) in [
        (
            "completed_cache_hits",
            metrics
                .dependency_profiles
                .iter()
                .map(|x| x.profile.completed_cache_hits)
                .sum::<usize>(),
        ),
        (
            "full_completions",
            metrics
                .dependency_profiles
                .iter()
                .map(|x| x.profile.full_completions)
                .sum::<usize>(),
        ),
        (
            "shells_created",
            metrics
                .dependency_profiles
                .iter()
                .map(|x| x.profile.shells_created)
                .sum::<usize>(),
        ),
        (
            "shells_reused",
            metrics
                .dependency_profiles
                .iter()
                .map(|x| x.profile.shells_reused)
                .sum::<usize>(),
        ),
        (
            "member_type_references",
            metrics
                .dependency_profiles
                .iter()
                .map(|x| x.profile.member_type_references)
                .sum::<usize>(),
        ),
        (
            "hierarchy_loads",
            metrics
                .dependency_profiles
                .iter()
                .map(|x| x.profile.hierarchy_loads)
                .sum::<usize>(),
        ),
    ] {
        println!("  {label}={total}");
    }

    println!(
        "  max_dependency_depth={}",
        depths.last().copied().unwrap_or(0)
    );
    println!("  median_dependency_depth={median_depth:.1}");
    println!("  p95_dependency_depth={p95_depth}");
    println!("  max_distinct_classes_touched_by_one_request={max_classes}");

    let mut observed_by_method: BTreeMap<&str, (BTreeSet<&str>, BTreeSet<&str>)> = BTreeMap::new();
    for observation in &metrics.dependency_profiles {
        let (shells, completed) = observed_by_method.entry(&observation.method).or_default();
        shells.extend(
            observation
                .profile
                .identity_shells
                .iter()
                .map(String::as_str),
        );
        completed.extend(
            observation
                .profile
                .completed_classes
                .iter()
                .map(String::as_str),
        );
    }
    let mut demand_roots = BTreeMap::new();
    for observation in &metrics.dependency_profiles {
        if observation.profile.root_was_shell
            && observation
                .profile
                .completed_classes
                .contains(&observation.profile.root)
        {
            *demand_roots.entry(&observation.profile.root).or_insert(0) += 1;
        }
    }
    println!("  on_demand_completion_roots:");
    for (root, count) in demand_roots {
        println!("    root={root}::completions={count}");
    }
    println!("  lifecycle_observations_by_method:");
    for (method, (shells, completed)) in observed_by_method {
        println!(
            "    method={method}::shells=[{}]::completed=[{}]",
            shells.into_iter().collect::<Vec<_>>().join(","),
            completed.into_iter().collect::<Vec<_>>().join(",")
        );
    }

    let mut largest = metrics.dependency_profiles.iter().collect::<Vec<_>>();
    largest.sort_by(|left, right| {
        right
            .profile
            .completed_classes
            .len()
            .cmp(&left.profile.completed_classes.len())
            .then(left.profile.root.cmp(&right.profile.root))
            .then(left.file.cmp(&right.file))
            .then(left.method.cmp(&right.method))
            .then(left.request.cmp(&right.request))
    });
    println!("  largest_top_level_loads:");
    for observation in largest.into_iter().take(10) {
        println!(
            "    root={}::file={}::method={}::request={}::depth={}::distinct_classes={}::full_completions={}::shells_created={}::shells_reused={}::member_type_references={}::hierarchy_loads={}::completed_cache_hits={}::completed_classes=[{}]::classes=[{}]",
            observation.profile.root,
            observation.file,
            observation.method,
            observation.request,
            observation.profile.max_dependency_depth,
            observation.profile.classes.len(),
            observation.profile.full_completions,
            observation.profile.shells_created,
            observation.profile.shells_reused,
            observation.profile.member_type_references,
            observation.profile.hierarchy_loads,
            observation.profile.completed_cache_hits,
            observation
                .profile
                .completed_classes
                .iter()
                .cloned()
                .collect::<Vec<_>>()
                .join(","),
            observation
                .profile
                .classes
                .iter()
                .cloned()
                .collect::<Vec<_>>()
                .join(",")
        );
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

fn print_local_value_baseline_outcomes(outcomes: &BTreeSet<LocalValueBaselineOutcome>) {
    println!("local_value_baseline_outcomes:");
    println!("  baseline_observations={}", outcomes.len());
    let mut classifications = BTreeMap::<&str, usize>::new();
    for outcome in outcomes {
        *classifications.entry(outcome.classification).or_default() += 1;
    }
    println!("  classifications:");
    for (classification, count) in classifications {
        println!("    {classification}={count}");
    }
    println!("  rows:");
    for outcome in outcomes {
        let row = &outcome.observation;
        println!(
            "    {}:method={} declaration_tree={} kind={} modifiers=[{}] baseline_first_blocker={} declaration_typed={} current_first_blocker={} classification={}",
            row.path,
            row.enclosing_method,
            row.declaration_tree,
            row.declaration_kind,
            row.modifiers,
            outcome.baseline_first_blocker,
            outcome.declaration_typed,
            outcome.current_first_blocker,
            outcome.classification,
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

fn function_with_mods_observation(
    arena: &dotty_core::AstArena<Untyped>,
    names: &dotty_core::names::NameInterner,
    source_text: &str,
    path: &str,
    method_tree: dotty_core::TreeId<Untyped>,
    function_tree: u32,
    blocker_origin_tree: u32,
) -> Option<FunctionWithModsObservation> {
    let (_tree, node) = arena
        .iter()
        .find(|(tree, _)| tree.index() == function_tree)?;
    let TreeKind::PhaseSpecific(UntypedNode::FunctionWithMods(function)) = &node.kind else {
        return None;
    };
    let TreeKind::DefDef(method) = &arena.get(method_tree).kind else {
        return None;
    };
    let (_, first_failure_node) = arena
        .iter()
        .find(|(origin_tree, _)| origin_tree.index() == blocker_origin_tree)?;
    let TreeKind::DefDef(first_failure_method) = &first_failure_node.kind else {
        return None;
    };
    let method_name = names.resolve(method.name.as_name().text()).to_owned();
    let position = node.position?;
    let span = position.span().range();
    let (owner_tree, owner_node) = arena
        .iter()
        .filter_map(|(candidate_tree, candidate_node)| {
            let TreeKind::DefDef(_) = &candidate_node.kind else {
                return None;
            };
            let range = candidate_node.position?.span().range();
            (range.start() <= span.start() && span.end() <= range.end()).then_some((
                candidate_tree,
                candidate_node,
                range,
            ))
        })
        .min_by_key(|(_, _, range)| range.end().saturating_sub(range.start()))
        .map(|(candidate_tree, candidate_node, _)| (candidate_tree, candidate_node))?;
    let TreeKind::DefDef(owner_method) = &owner_node.kind else {
        return None;
    };
    let blocker_origin_method = names.resolve(owner_method.name.as_name().text()).to_owned();
    let first_failure_method = names
        .resolve(first_failure_method.name.as_name().text())
        .to_owned();
    let source_prefix = source_text.get(..span.start() as usize)?;
    let syntax = source_text
        .get(span.start() as usize..span.end() as usize)?
        .to_owned();
    let line = source_prefix.bytes().filter(|byte| *byte == b'\n').count() + 1;
    let parameters = function
        .params
        .iter()
        .map(|parameter| {
            let node = arena.get(*parameter);
            let kind = tree_kind_label(&node.kind);
            let name = match &node.kind {
                TreeKind::Ident(ident) => format!("unnamed:{}", names.resolve(ident.name.text())),
                TreeKind::ValDef(definition) => {
                    format!("named:{}", names.resolve(definition.name.as_name().text()))
                }
                _ => format!("unnamed:{kind}"),
            };
            format!("{name}/{kind}")
        })
        .collect::<Vec<_>>()
        .join(",");
    let result_kind = tree_kind_label(&arena.get(function.result).kind).to_owned();
    let modifiers = source_modifier_list(&function.modifiers);
    let visibility = source_visibility(names, function.modifiers.visibility);
    let annotations = !function.modifiers.annotations.is_empty();
    let source_kind = match (
        function.modifiers.modifiers.contains(&Modifier::Given),
        function.modifiers.modifiers.contains(&Modifier::Impure),
    ) {
        (false, false) => "ordinary/pure",
        (false, true) => "ordinary/impure",
        (true, false) => "contextual/pure",
        (true, true) => "contextual/impure",
    };
    Some(FunctionWithModsObservation {
        path: path.to_owned(),
        method_tree: method_tree.index(),
        method: method_name,
        blocker_origin_tree: owner_tree.index(),
        blocker_origin_method,
        first_failure_tree: blocker_origin_tree,
        first_failure_method,
        function_tree,
        line,
        span: format!("{}..{}", span.start(), span.end()),
        syntax,
        parameter_count: function.params.len(),
        parameters,
        result_kind,
        source_kind: source_kind.to_owned(),
        modifiers,
        visibility,
        annotations,
        erased_params: function
            .erased_params
            .iter()
            .map(|erased| if *erased { "erased" } else { "ordinary" })
            .collect::<Vec<_>>()
            .join(","),
        attribution: if method_tree == owner_tree {
            "direct".to_owned()
        } else {
            "sibling/inherited".to_owned()
        },
    })
}

struct ImpureFunctionBaselineAuditContext<'ctx, 'store> {
    arena: &'ctx dotty_core::AstArena<Untyped>,
    typer: &'ctx SourceTyper<'store>,
    source_text: &'ctx str,
    path: &'ctx str,
    classpath: Option<SharedClassPath>,
    projection_cache: &'ctx mut BTreeMap<u32, ImpureFunctionProjection>,
    method_tree: TreeId<Untyped>,
    first_blocker: &'ctx str,
    method_typed: bool,
}

fn record_impure_function_baseline_outcome(
    audit: &mut Audit,
    context: ImpureFunctionBaselineAuditContext<'_, '_>,
) {
    let ImpureFunctionBaselineAuditContext {
        arena,
        typer,
        source_text,
        path,
        classpath,
        projection_cache,
        method_tree,
        first_blocker,
        method_typed,
    } = context;
    let Some(baseline) = IMPURE_FUNCTION_BASELINE
        .iter()
        .find(|row| row.path == path && row.method_tree == method_tree.index())
    else {
        return;
    };
    let (_, method_node) = arena
        .iter()
        .find(|(tree, _)| tree.index() == baseline.method_tree)
        .expect("pinned impure-function method tree should remain in the corpus");
    let TreeKind::DefDef(method) = &method_node.kind else {
        panic!(
            "pinned impure-function method identity changed at tree {}",
            baseline.method_tree
        );
    };
    let method_name = typer
        .store()
        .names
        .resolve(method.name.as_name().text())
        .to_owned();
    assert_eq!(
        method_name, baseline.method,
        "pinned method identity changed"
    );
    let (function_tree, function_node) = arena
        .iter()
        .find(|(tree, _)| tree.index() == baseline.function_tree)
        .expect("pinned FunctionWithMods source tree should remain in the corpus");
    let TreeKind::PhaseSpecific(UntypedNode::FunctionWithMods(function)) = &function_node.kind
    else {
        panic!("pinned function tree changed away from FunctionWithMods");
    };
    let position = function_node
        .position
        .expect("pinned function tree has a span");
    let span = position.span().range();
    let syntax = source_text
        .get(span.start() as usize..span.end() as usize)
        .expect("pinned function source span is valid");
    let line = source_text[..span.start() as usize]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count()
        + 1;
    let span_text = format!("{}..{}", span.start(), span.end());
    assert_eq!(line, baseline.line, "pinned function line changed");
    assert_eq!(span_text, baseline.span, "pinned function span changed");
    assert_eq!(syntax, baseline.syntax, "pinned function syntax changed");
    assert!(function.modifiers.modifiers.contains(&Modifier::Impure));
    let source_kind = if function.modifiers.modifiers.contains(&Modifier::Given) {
        "contextual/impure"
    } else {
        "ordinary/impure"
    };
    let semantic_family = if source_kind == "contextual/impure" {
        format!("ImpureContextFunction{}", function.params.len())
    } else {
        format!("ImpureFunction{}", function.params.len())
    };
    // Probe the type tree independently after the enclosing method attempt.
    // A failed method transaction rolls back its source-type cache, so reading
    // that cache here would conflate projection with the later method failure.
    let projection = projection_cache
        .entry(function_tree.index())
        .or_insert_with(|| {
            isolated_impure_function_projection(source_text, path, function_tree.index(), classpath)
        })
        .clone();
    audit
        .impure_function_baseline_outcomes
        .insert(ImpureFunctionBaselineOutcome {
            path: path.to_owned(),
            method_tree: baseline.method_tree,
            method: method_name,
            function_tree: baseline.function_tree,
            line,
            span: span_text,
            syntax: syntax.to_owned(),
            semantic_family: projection.semantic_family,
            expected_semantic_family: semantic_family,
            modifiers: source_modifier_list(&function.modifiers),
            erased_params: function
                .erased_params
                .iter()
                .map(|erased| if *erased { "erased" } else { "ordinary" })
                .collect::<Vec<_>>()
                .join(","),
            arity: function.params.len(),
            source_kind: source_kind.to_owned(),
            attribution: baseline.attribution.to_owned(),
            projection_completed: projection.completed,
            projection_error: projection.error,
            first_blocker: first_blocker.to_owned(),
            method_typed,
        });
}

fn isolated_impure_function_projection(
    source_text: &str,
    path: &str,
    function_tree_index: u32,
    classpath: Option<SharedClassPath>,
) -> ImpureFunctionProjection {
    let failed = |error: String| ImpureFunctionProjection {
        completed: false,
        semantic_family: "unselected".to_owned(),
        error: Some(error),
    };
    let source = SourceId::from_index(0);
    let scanner = match ContextualScanner::new(source_text) {
        Ok(scanner) => scanner,
        Err(error) => return failed(format!("scanner: {error:?}")),
    };
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let source_text = match SourceText::new(source_text) {
        Ok(source_text) => source_text,
        Err(error) => return failed(format!("source text: {error:?}")),
    };
    let parsed = parse_compilation_unit(source_text, source, scanner, &mut store.names);
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
        Err(error) => return failed(format!("namer: {error:?}")),
    };
    let Some((function_tree, function_span)) = parsed.ast.iter().find_map(|(tree, node)| {
        if tree.index() != function_tree_index {
            return None;
        }
        Some((tree, node.position?.span().range()))
    }) else {
        return failed(format!("missing source tree index {function_tree_index}"));
    };
    let Some((_, context)) = parsed
        .ast
        .iter()
        .filter_map(|(tree, node)| {
            let span = node.position?.span().range();
            if span.start() > function_span.start() || span.end() < function_span.end() {
                return None;
            }
            let symbol = index.symbol_at(source, tree)?;
            let context = index.declaration_context_of(symbol)?;
            Some((span.end() - span.start(), context))
        })
        .min_by_key(|(span_len, _)| *span_len)
    else {
        return failed(format!(
            "no declaration context contains source tree {function_tree_index}"
        ));
    };
    let (typer_packages, resolver_packages) = if classpath.is_some() {
        (Packages::new(), Some(packages))
    } else {
        (packages, None)
    };
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &typer_packages,
    );
    if let (Some(classpath), Some(packages)) = (classpath, resolver_packages) {
        let resolver = ClasspathSymbolResolver::new(
            classpath,
            definitions,
            LoadingSession::with_packages(packages),
        )
        .with_load_profile_collection();
        typer = typer.with_resolver(Box::new(AuditResolver {
            inner: resolver,
            metrics: Rc::new(RefCell::new(ResolverMetrics::default())),
            class_path_probe: None,
        }));
    }
    let projection = match typer.type_of_tpt(function_tree, context) {
        Ok(projection) => projection,
        Err(error) => return failed(format!("type projection: {error:?}")),
    };
    let Type::Applied { tycon, .. } = typer.store().types.get(projection) else {
        return ImpureFunctionProjection {
            completed: true,
            semantic_family: "unexpected_non_applied_type".to_owned(),
            error: Some("projected root type is not Applied".to_owned()),
        };
    };
    let Type::TypeRef {
        target: TypeRefTarget::Symbol(symbol),
        ..
    } = typer.store().types.get(*tycon)
    else {
        return ImpureFunctionProjection {
            completed: true,
            semantic_family: "unexpected_non_symbol_constructor".to_owned(),
            error: Some("Applied type constructor is not a symbol TypeRef".to_owned()),
        };
    };
    let constructor = typer
        .store()
        .names
        .resolve(typer.store().symbols.get(*symbol).name.text())
        .to_owned();
    ImpureFunctionProjection {
        completed: true,
        semantic_family: constructor,
        error: None,
    }
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

fn import_qualifier_path(
    arena: &dotty_core::AstArena<Untyped>,
    names: &dotty_core::NameInterner,
    import_tree_index: u32,
) -> Option<Vec<String>> {
    let (_, import) = arena.iter().find(|(tree, node)| {
        tree.index() == import_tree_index && matches!(&node.kind, TreeKind::Import(_))
    })?;
    let TreeKind::Import(import) = &import.kind else {
        return None;
    };
    let mut segments = Vec::new();
    let mut current = import.expr;
    loop {
        match &arena.try_get(current)?.kind {
            TreeKind::Ident(ident) => {
                segments.push(names.resolve(ident.name.text()).to_owned());
                break;
            }
            TreeKind::Select(select) => {
                segments.push(names.resolve(select.name.text()).to_owned());
                current = select.qualifier;
            }
            TreeKind::PhaseSpecific(UntypedNode::Parens(parens)) => current = parens.inner,
            _ => return None,
        }
    }
    segments.reverse();
    Some(segments)
}

#[allow(clippy::too_many_arguments)]
fn import_prefix_observation(
    arena: &dotty_core::AstArena<Untyped>,
    store: &SemanticStore,
    index: &dotty_core::SourceSemanticIndex,
    source: SourceId,
    text: &str,
    path: &str,
    method_tree: TreeId<Untyped>,
    blocker_method_tree: Option<u32>,
    import_tree_index: u32,
    qualifier: &[String],
    trace: &[ResolverRequestObservation],
    source_package_paths: &BTreeSet<String>,
) -> Option<ImportPrefixObservation> {
    let (import_tree, import_node) = arena.iter().find(|(tree, node)| {
        tree.index() == import_tree_index && matches!(&node.kind, TreeKind::Import(_))
    })?;
    let TreeKind::Import(import) = &import_node.kind else {
        return None;
    };
    let qualifier_node = arena.try_get(import.expr)?;
    let qualifier_range = qualifier_node.position?.span().range();
    let qualifier_start = qualifier_range.start() as usize;
    let qualifier_end = qualifier_range.end() as usize;
    let qualifier_source = text.get(qualifier_start..qualifier_end)?.to_owned();
    let context_tree = blocker_method_tree
        .and_then(|index_to_find| {
            arena
                .iter()
                .find(|(tree, _)| tree.index() == index_to_find)
                .map(|(tree, _)| tree)
        })
        .unwrap_or(method_tree);
    let method_symbol = index
        .symbol_at(source, method_tree)
        .or_else(|| index.symbol_at(source, context_tree))?;
    let mut context_id = index.declaration_context_of(method_symbol);
    let mut selected_context = None;
    while let Some(id) = context_id {
        let context = index.try_source_context(id)?;
        if context
            .import
            .is_some_and(|tree| tree.index() == import_tree_index)
        {
            selected_context = Some((id, context));
            break;
        }
        if selected_context.is_none() {
            selected_context = Some((id, context));
        }
        context_id = context.parent;
    }
    let context = selected_context.map(|(_, context)| context);
    let context_owner = context.map_or_else(
        || "<unavailable>".to_owned(),
        |context| audit_symbol_path(store, context.owner),
    );
    let context_package = context.map_or_else(
        || "<unavailable>".to_owned(),
        |context| source_package_path(store, context.owner),
    );
    let lookup = qualifier_node_name(arena, import.expr);
    let namespace = lookup.map_or_else(
        || "<unavailable>".to_owned(),
        |name| format!("{:?}", name.namespace()),
    );
    let source_lookup = match (selected_context, lookup) {
        (Some((context_id, _)), Some(name)) => source_scoped_lookup(index, store, context_id, name),
        _ => "unavailable".to_owned(),
    };
    let request = trace
        .iter()
        .rfind(|request| request.cause.is_some() && request_matches_qualifier(request, qualifier));
    let target_key = request.map_or_else(
        || format!("{}::<no-matching-request>", qualifier.join(".")),
        |request| request.target.clone(),
    );
    let resolved_prefix = target_key
        .split("::resolved-prefix=")
        .nth(1)
        .and_then(|suffix| suffix.split("::").next())
        .unwrap_or("");
    let first_failing_segment = target_key
        .split("::first-missing=")
        .nth(1)
        .and_then(|suffix| suffix.split("::").next())
        .or_else(|| qualifier.first().map(String::as_str))
        .unwrap_or("<unknown>");
    let failed_prefix = [
        resolved_prefix
            .split('.')
            .filter(|segment| !segment.is_empty())
            .collect::<Vec<_>>(),
        first_failing_segment.split('.').collect::<Vec<_>>(),
    ]
    .concat()
    .join(".");
    let rooted = qualifier.first().is_some_and(|segment| segment == "_root_");
    let relative_candidate = if rooted || context_package.starts_with('<') {
        None
    } else {
        let candidate = format!("{context_package}.{failed_prefix}");
        source_package_paths
            .contains(&candidate)
            .then_some(candidate)
    };
    let root_candidate = failed_prefix;
    let source_package_registry_lookup = format!(
        "relative={};root={}",
        relative_candidate.as_deref().map_or_else(
            || "missing".to_owned(),
            |path| format!("found:Package@{path}")
        ),
        if source_package_paths.contains(&root_candidate) {
            format!("found:Package@{root_candidate}")
        } else {
            "missing".to_owned()
        }
    );
    let method = format!("{path}#tree={}", method_tree.index());
    let blocker_origin = blocker_method_tree.map_or_else(
        || "<unknown>".to_owned(),
        |tree| {
            if tree == method_tree.index() {
                format!("direct:{path}#tree={tree}")
            } else {
                format!("inherited:{path}#tree={tree}")
            }
        },
    );
    let import_range = import_node.position?.span().range();
    let import_start = import_range.start() as usize;
    let import_line = text[..import_start.min(text.len())]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count()
        + 1;
    let resolver_outcome = request.map_or_else(
        || "unknown:no matching failed request".to_owned(),
        |request| {
            let result = match request.outcome.as_str() {
                "package unresolved" => "Ok(None)",
                "package error" => "Err",
                "package success" => "Ok(Some)",
                _ => "unknown",
            };
            format!(
                "{result}::{}::{}",
                request.outcome,
                request.cause.as_deref().unwrap_or("-")
            )
        },
    );
    Some(ImportPrefixObservation {
        source_file: path.to_owned(),
        method,
        blocker_origin,
        import_tree: format!("{path}#tree={}", import_tree.index()),
        import_line,
        qualifier_source,
        qualifier_segments: qualifier.join("."),
        context_owner,
        context_package,
        namespace,
        source_lookup,
        source_package_registry_lookup,
        resolved_prefix: resolved_prefix.to_owned(),
        first_failing_segment: first_failing_segment.to_owned(),
        request_kind: "resolve_package".to_owned(),
        resolver_outcome,
        target_key,
    })
}

fn qualifier_node_name(
    arena: &dotty_core::AstArena<Untyped>,
    mut tree: TreeId<Untyped>,
) -> Option<dotty_core::Name> {
    loop {
        match &arena.try_get(tree)?.kind {
            TreeKind::Ident(ident) => return Some(ident.name),
            TreeKind::Select(select) => tree = select.qualifier,
            TreeKind::PhaseSpecific(UntypedNode::Parens(parens)) => tree = parens.inner,
            _ => return None,
        }
    }
}

fn source_scoped_lookup(
    index: &dotty_core::SourceSemanticIndex,
    store: &SemanticStore,
    mut context: dotty_core::SourceContextId,
    name: dotty_core::Name,
) -> String {
    let mut visited = HashSet::new();
    while visited.insert(context.index()) {
        let Some(source_context) = index.try_source_context(context) else {
            return "unavailable".to_owned();
        };
        if let Some(symbol) = store.scopes.get(source_context.lexical_scope).lookup(&name) {
            return format!(
                "found:{:?}@{}",
                store.symbols.get(symbol).kind,
                audit_symbol_path(store, source_context.owner)
            );
        }
        let Some(parent) = source_context.parent else {
            break;
        };
        context = parent;
    }
    "missing".to_owned()
}

fn source_package_path(store: &SemanticStore, mut owner: SymbolId) -> String {
    let mut parts = Vec::new();
    let mut current = Some(owner);
    let mut visited = HashSet::new();
    while let Some(symbol) = current {
        if !visited.insert(symbol) {
            return "<owner-cycle>".to_owned();
        }
        let entry = store.symbols.get(symbol);
        if matches!(entry.kind, dotty_core::SymbolKind::Package) {
            let name = store.names.resolve(entry.name.text());
            if !name.is_empty() {
                parts.push(name.to_owned());
            }
        } else if !parts.is_empty() {
            break;
        }
        owner = symbol;
        current = store.symbols.get(owner).owner;
    }
    parts.reverse();
    if parts.is_empty() {
        "<root-or-unavailable>".to_owned()
    } else {
        parts.join(".")
    }
}

fn request_matches_qualifier(request: &ResolverRequestObservation, qualifier: &[String]) -> bool {
    let qualifier = if qualifier.first().map(String::as_str) == Some("_root_") {
        &qualifier[1..]
    } else {
        qualifier
    };
    for length in 1..=qualifier.len() {
        let package = qualifier[..length].join(".");
        if request.target == package || request.target.starts_with(&format!("{package}::")) {
            return true;
        }
        if length > 1 {
            let parent = qualifier[..length - 1].join(".");
            let prefix = format!("prefix={parent}::");
            let name = format!("::name={}::", qualifier[length - 1]);
            if request.target.starts_with(&prefix) && request.target.contains(&name) {
                return true;
            }
        }
    }
    false
}

fn request_target_package_term_qualifier(target: &str) -> Option<String> {
    let target = target.strip_prefix("prefix=")?;
    let (package, tail) = target.split_once("::namespace=term::name=")?;
    let (name, _) = tail.split_once("::space=")?;
    Some(if package.is_empty() {
        name.to_owned()
    } else {
        format!("{package}.{name}")
    })
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
    let by_name = audit_source_inner(by_name_source, "ByName.scala", None, false);
    let repeated_by_name = audit_source_inner(by_name_source, "ByName.scala", None, false);
    assert_eq!(
        by_name.local_method_signature_profile, repeated_by_name.local_method_signature_profile,
        "local method signature profile should be deterministic across repeated audits"
    );
    assert!(by_name.local_method_signature_profile.features.is_empty());

    let dependent = audit_source_inner(
        "class C { def outer: Int = { def dependent(value: Int): value.type = value; 0 } }",
        "DependentResult.scala",
        None,
        false,
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
        false,
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
        false,
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
        false,
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
        TyperError::ObjectModuleClassResolution { .. } => "ObjectModuleClassResolution",
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
        TyperError::LocalValueMetadataDeferred { .. } => "LocalValueMetadataDeferred",
        TyperError::LocalPatDefDeferred { .. } => "LocalPatDefDeferred",
        TyperError::InvalidInferredLocalValueType { .. } => "InvalidInferredLocalValueType",
        TyperError::LocalValueRightHandSideMissing { .. } => "LocalValueRightHandSideMissing",
        TyperError::RecursiveLocalValueInitializer { .. } => "RecursiveLocalValueInitializer",
        TyperError::RecursiveLazyLocalValueInitializer { .. } => {
            "RecursiveLazyLocalValueInitializer"
        }
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

fn classify_parse_diagnostic(code: &str) -> FailureClassification {
    FailureClassification {
        bucket: format!("ParserDiagnostic::{code}"),
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

fn collect_function_with_mods_inventory(
    arena: &dotty_core::AstArena<Untyped>,
    type_tree_nodes: &HashSet<TreeId<Untyped>>,
    path: &str,
) -> FunctionWithModsInventory {
    let mut inventory = FunctionWithModsInventory::default();
    for (tree, node) in arena.iter() {
        if type_tree_nodes.contains(&tree)
            && let TreeKind::PhaseSpecific(UntypedNode::Function(function)) = &node.kind
        {
            inventory.plain_function_trees += 1;
            inventory.plain_function_files.insert(path.to_owned());
            inventory.capture_result_adjacency += usize::from(matches!(
                arena.try_get(function.body).map(|result| &result.kind),
                Some(TreeKind::PhaseSpecific(UntypedNode::CapturesAndResult(_)))
            ));
        }
        let TreeKind::PhaseSpecific(UntypedNode::FunctionWithMods(function)) = &node.kind else {
            continue;
        };
        inventory.total += 1;
        inventory.files.insert(path.to_owned());
        let modifiers = source_modifier_list(&function.modifiers);
        *inventory.modifiers.entry(modifiers).or_default() += 1;
        let source_kind = match (
            function.modifiers.modifiers.contains(&Modifier::Given),
            function.modifiers.modifiers.contains(&Modifier::Impure),
        ) {
            (false, false) => "ordinary/pure",
            (false, true) => "ordinary/impure",
            (true, false) => "contextual/pure",
            (true, true) => "contextual/impure",
        };
        *inventory
            .source_kinds
            .entry(source_kind.to_owned())
            .or_default() += 1;
        inventory
            .source_kind_files
            .entry(source_kind.to_owned())
            .or_default()
            .insert(path.to_owned());
        inventory.erased += function
            .erased_params
            .iter()
            .filter(|erased| **erased)
            .count();
        inventory.capture_result_adjacency += usize::from(matches!(
            arena.try_get(function.result).map(|result| &result.kind),
            Some(TreeKind::PhaseSpecific(UntypedNode::CapturesAndResult(_)))
        ));
    }
    inventory
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

#[cfg(test)]
mod resolver_profile_tests {
    use super::*;

    fn package_prefix(store: &mut SemanticStore) -> dotty_core::ids::TypeId {
        let mut packages = Packages::new();
        let package = packages
            .enter(store, dotty_core::SymbolOrigin::Synthetic, &["sample"])
            .pop()
            .expect("package is entered")
            .symbol;
        let no_prefix = store.types.alloc(Type::NoPrefix);
        store.types.alloc(Type::type_ref(no_prefix, package))
    }

    fn request(
        store: &mut SemanticStore,
        prefix: dotty_core::ids::TypeId,
        namespace: Namespace,
    ) -> MemberRequest {
        let name = store.names.intern("Missing");
        MemberRequest {
            prefix,
            name: dotty_core::Name::new(name, namespace),
            selector: MemberSelector::Unique,
            space: MemberSpace::Prefix,
        }
    }

    #[test]
    fn package_type_request_is_profiled_as_type_member_miss() {
        let mut store = SemanticStore::new();
        let prefix = package_prefix(&mut store);
        let request = request(&mut store, prefix, Namespace::Type);
        let profile = prefix_profile(&store, prefix);
        assert_eq!(profile.shape, "package TypeRef");
        assert_eq!(
            unresolved_member_kind(&store, &request, &profile),
            "package type member unresolved"
        );
    }

    #[test]
    fn package_term_request_is_profiled_as_unsupported_term_miss() {
        let mut store = SemanticStore::new();
        let prefix = package_prefix(&mut store);
        let request = request(&mut store, prefix, Namespace::Term);
        let profile = prefix_profile(&store, prefix);
        assert_eq!(
            unresolved_member_kind(&store, &request, &profile),
            "package term member unsupported"
        );
    }

    #[test]
    fn complete_class_info_is_not_misclassified_as_incomplete() {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let scope = store
            .scopes
            .alloc(dotty_core::Scope::new(Some(definitions.object_class)));
        let info = store.types.alloc(Type::ClassInfo(dotty_core::ClassInfo {
            prefix: definitions.no_prefix,
            class: definitions.object_class,
            parents: Vec::new(),
            declarations: scope,
            self_type: None,
        }));
        store.symbols.get_mut(definitions.object_class).info =
            dotty_core::SymbolInfo::Complete(info);
        let request = request(&mut store, definitions.object_type, Namespace::Term);
        let profile = prefix_profile(&store, definitions.object_type);
        assert_eq!(
            unresolved_member_kind(&store, &request, &profile),
            "other resolver Ok(None)"
        );
    }

    #[test]
    fn failed_member_lookup_keeps_its_already_materialized_class_prefix() {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let scope = store
            .scopes
            .alloc(dotty_core::Scope::new(Some(definitions.object_class)));
        let info = store.types.alloc(Type::ClassInfo(dotty_core::ClassInfo {
            prefix: definitions.no_prefix,
            class: definitions.object_class,
            parents: Vec::new(),
            declarations: scope,
            self_type: None,
        }));
        let origin = store.origins.register_tasty();
        let object_class = store.symbols.get_mut(definitions.object_class);
        object_class.origin = dotty_core::SymbolOrigin::Tasty(origin);
        object_class.info = dotty_core::SymbolInfo::Complete(info);

        let request = request(&mut store, definitions.object_type, Namespace::Term);
        let metrics = Rc::new(RefCell::new(ResolverMetrics {
            current_file: "Use.scala".to_owned(),
            current_method: "Use.scala#tree=1".to_owned(),
            ..ResolverMetrics::default()
        }));
        let mut resolver = AuditResolver {
            inner: ClasspathSymbolResolver::new(
                CompositeClassPath::new(Vec::new()),
                definitions,
                LoadingSession::new(),
            ),
            metrics: metrics.clone(),
            class_path_probe: None,
        };
        assert_eq!(resolver.resolve_member(&mut store, &request), Ok(None));

        let trace = metrics
            .borrow()
            .resolver_requests_by_method
            .get("Use.scala#tree=1")
            .unwrap()
            .clone();
        assert_eq!(trace.len(), 1);
        assert_eq!(trace[0].outcome, "other resolver Ok(None)");
        assert_eq!(trace[0].materialized_prefix.as_deref(), Some("Object"));
    }

    #[test]
    fn missing_class_info_is_reported_as_unavailable_scope() {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let request = request(&mut store, definitions.object_type, Namespace::Term);
        let profile = prefix_profile(&store, definitions.object_type);
        assert_eq!(
            unresolved_member_kind(&store, &request, &profile),
            "class member scope unavailable: incomplete class info"
        );
    }

    #[test]
    fn unresolved_classpath_name_has_a_stable_error_category() {
        let error = ResolutionError::Malformed {
            reason: "class not found: sample/Missing".to_owned(),
        };
        assert_eq!(normalized_error(&error).0, "unresolved class");
    }

    #[test]
    fn nested_dependency_error_retains_top_and_root_classes() {
        let error = ResolutionError::Malformed { reason: "class sample/Outer failed to load because sample/Inner failed: class not found: java/lang/Object".to_owned() };
        let (category, requested, root) = normalized_error(&error);
        assert_eq!(category, "unresolved class");
        assert_eq!(requested, "sample/Outer");
        assert_eq!(root, "java/lang/Object");
    }

    #[test]
    fn nested_circular_inheritance_is_classified_by_its_root_cause() {
        let error = ResolutionError::Malformed { reason: "class sample/A failed to load because sample/B failed: circular inheritance involving sample/B".to_owned() };
        let (category, requested, root) = normalized_error(&error);
        assert_eq!(category, "circular inheritance");
        assert_eq!(requested, "sample/A");
        assert_eq!(root, "sample/B");
    }

    #[test]
    fn deeply_nested_dependency_error_is_classified_by_its_deepest_cause() {
        let error = ResolutionError::Malformed {
            reason: "class sample/A failed to load because sample/B failed: class sample/C failed to load because circular inheritance involving sample/C".to_owned(),
        };
        let (category, requested, root) = normalized_error(&error);
        assert_eq!(category, "circular inheritance");
        assert_eq!(requested, "sample/A");
        assert_eq!(root, "sample/C");
    }

    #[test]
    fn unresolved_type_variable_is_a_stable_root_cause() {
        let error = ResolutionError::Malformed { reason: "signature in class file for java/lang/invoke/ClassSpecializer$SpeciesData references unresolved type variable K".to_owned() };
        let (category, _, root) = normalized_error(&error);
        assert_eq!(category, "UnresolvedTypeVariable");
        assert_eq!(root, "java/lang/invoke/ClassSpecializer$SpeciesData");
    }

    #[test]
    fn unresolved_tasty_supertype_is_separate_from_classfile_failure() {
        let error = ResolutionError::Malformed {
            reason: "invalid .tasty file for scala/Foo: unresolved supertype reference at parent 1: shape=applied, semantic_reference=true, reference_prefix=true, names=[\"scala/Parent\"], raw=LengthNode(tag=162)"
                .to_owned(),
        };
        assert_eq!(
            normalized_error(&error).0,
            "InvalidTastyFile::UnresolvedSupertype"
        );
        assert_eq!(
            unresolved_tasty_parent_profile(match &error {
                ResolutionError::Malformed { reason } => reason,
                _ => unreachable!(),
            })
            .as_deref(),
            Some(
                "parent_index=1::parent_shape=applied::semantic_reference=true::reference_prefix=true::visible_names=[\"scala/Parent\"]::raw_shape=LengthNode(tag=162)"
            )
        );
    }

    #[test]
    fn tasty_parent_diagnostic_names_do_not_replace_the_failing_class_root() {
        let error = ResolutionError::Malformed {
            reason: "invalid .tasty file for scala/annotation/retainsCap: unresolved supertype reference at parent 0: shape=package-qualified, semantic_reference=true, reference_prefix=true, names=[\"Annotation\", \"scala/annotation\"], raw=LengthNode(tag=136)".to_owned(),
        };
        let (category, requested, root) = normalized_error(&error);
        assert_eq!(category, "InvalidTastyFile::UnresolvedSupertype");
        assert_eq!(requested, "scala/annotation/retainsCap");
        assert_eq!(root, "scala/annotation/retainsCap");
    }

    #[test]
    fn malformed_classfile_signature_is_grouped_without_payload_text() {
        let error = ResolutionError::Malformed {
            reason: "malformed signature in class file for java/util/List: bad bound".to_owned(),
        };
        assert_eq!(normalized_error(&error).0, "malformed classfile/signature");
    }

    #[test]
    fn classfile_and_tasty_failures_remain_distinct() {
        let classfile = ResolutionError::Malformed {
            reason: "invalid class file for sample/Foo: invalid magic".to_owned(),
        };
        let tasty = ResolutionError::Malformed {
            reason: "invalid .tasty file for sample/Foo: bad header".to_owned(),
        };
        assert_ne!(normalized_error(&classfile).0, normalized_error(&tasty).0);
    }

    #[test]
    fn duplicate_request_volume_is_distinct_from_target_count() {
        let mut metrics = ResolverMetrics::default();
        let key = "prefix=sample::namespace=type::name=Missing::space=Prefix".to_owned();
        {
            let target = metrics.member_request_targets.entry(key).or_default();
            target.requests += 2;
            target
                .files
                .extend(["A.scala".to_owned(), "B.scala".to_owned()]);
            target.owners.insert("sample".to_owned());
            target.names.insert("Missing".to_owned());
            assert_eq!(target.requests, 2);
            assert_eq!(target.files.len(), 2);
        }
        assert_eq!(metrics.member_request_targets.len(), 1);
    }

    #[test]
    fn request_shape_distribution_does_not_depend_on_arrival_order() {
        fn profile(shapes: [&str; 2]) -> ResolverMetrics {
            let mut metrics = ResolverMetrics::default();
            for shape in shapes {
                record_member_request_target(
                    &mut metrics,
                    RequestTargetObservation {
                        target_key: "target=sample/Foo::name=bar".to_owned(),
                        shape: shape.to_owned(),
                        owner: Some("sample/Foo"),
                        name: "bar",
                        file: "Use.scala",
                        method: "run",
                        outcome: "external success",
                    },
                );
            }
            metrics
        }

        let forward = profile(["ThisType", "TypeRef"]);
        let reverse = profile(["TypeRef", "ThisType"]);
        assert_eq!(forward, reverse);
        assert_eq!(forward.member_request_targets.len(), 1);
        let target = forward.member_request_targets.values().next().unwrap();
        assert_eq!(target.requests, 2);
        assert_eq!(target.families["ThisType"].requests, 1);
        assert_eq!(target.families["TypeRef"].requests, 1);
        let forward_summary = member_request_shape_summaries(&forward);
        let reverse_summary = member_request_shape_summaries(&reverse);
        assert_eq!(forward_summary["ThisType"].requests, 1);
        assert_eq!(forward_summary["TypeRef"].requests, 1);
        assert_eq!(forward_summary["ThisType"], reverse_summary["ThisType"]);
        assert_eq!(forward_summary["TypeRef"], reverse_summary["TypeRef"]);
    }

    #[test]
    fn unknown_prefix_requests_are_not_counted_as_distinct_known_targets() {
        let mut metrics = ResolverMetrics::default();
        for request_number in 1..=2 {
            record_member_request_target(
                &mut metrics,
                RequestTargetObservation {
                    target_key: format!("prefix=<unknown>::request={request_number}"),
                    shape: "unsupported/other".to_owned(),
                    owner: None,
                    name: "Missing",
                    file: "Use.scala",
                    method: "run",
                    outcome: "unsupported prefix shape",
                },
            );
        }

        let summary = member_request_shape_summaries(&metrics)["unsupported/other"].clone();
        assert_eq!(summary.requests, 2);
        assert_eq!(summary.known_requests, 0);
        assert_eq!(summary.unknown_identity_requests, 2);
        assert!(summary.distinct_keys.is_empty());
        assert!(summary.owners.is_empty());
        assert_eq!(
            member_request_repeat_factors(&summary),
            (
                "unavailable (unknown identities included)".to_owned(),
                "unavailable".to_owned()
            )
        );
        assert_eq!(
            metrics
                .member_request_targets
                .values()
                .map(|target| target.requests)
                .sum::<usize>(),
            2,
            "unknown requests remain available for per-request attribution"
        );
    }

    #[test]
    fn term_ref_prefixes_to_distinct_symbols_keep_distinct_identities() {
        let mut store = SemanticStore::new();
        let mut packages = Packages::new();
        let owner = packages
            .enter(&mut store, dotty_core::SymbolOrigin::Synthetic, &["sample"])
            .last()
            .expect("sample package exists")
            .symbol;
        let mut symbols = Vec::new();
        for name in ["a", "b"] {
            let name = dotty_core::Name::new(store.names.intern(name), Namespace::Term);
            symbols.push(store.symbols.alloc(dotty_core::Symbol {
                name,
                owner: Some(owner),
                kind: dotty_core::SymbolKind::Value,
                flags: dotty_core::SymbolFlags::EMPTY,
                visibility: dotty_core::Visibility::Public,
                info: dotty_core::SymbolInfo::Missing,
                origin: dotty_core::SymbolOrigin::Synthetic,
                annotations: Vec::new(),
                position: None,
                links: dotty_core::SymbolLinks::default(),
            }));
        }
        let no_prefix = store.types.alloc(Type::NoPrefix);
        let prefixes = symbols
            .into_iter()
            .map(|symbol| {
                store.types.alloc(Type::TermRef {
                    prefix: no_prefix,
                    target: dotty_core::types::TermRefTarget::Symbol(symbol),
                })
            })
            .collect::<Vec<_>>();
        let first = prefix_profile(&store, prefixes[0]);
        let second = prefix_profile(&store, prefixes[1]);
        assert!(first.identity_known && second.identity_known);
        assert_ne!(first.identity, second.identity);
        let mut metrics = ResolverMetrics::default();
        for profile in [first, second] {
            record_member_request_target(
                &mut metrics,
                RequestTargetObservation {
                    target_key: format!(
                        "prefix={}::namespace=term::name=Missing::space=Prefix",
                        profile.identity
                    ),
                    shape: profile.shape.to_owned(),
                    owner: Some(profile.identity.as_str()),
                    name: "Missing",
                    file: "Use.scala",
                    method: "run",
                    outcome: "unsupported prefix shape",
                },
            );
        }
        assert_eq!(metrics.member_request_targets.len(), 2);
        assert_eq!(
            metrics
                .member_request_targets
                .values()
                .map(|target| target.requests)
                .sum::<usize>(),
            2
        );
    }

    #[test]
    fn profile_tables_sort_deterministically() {
        let mut table = BTreeMap::new();
        table.insert("zeta".to_owned(), 1);
        table.insert("alpha".to_owned(), 2);
        assert_eq!(table.keys().cloned().collect::<Vec<_>>(), ["alpha", "zeta"]);
    }

    #[test]
    fn import_qualifier_causes_follow_member_resolution_outcomes() {
        let package = PrefixProfile {
            shape: "package TypeRef",
            target_kind: "Package",
            identity: "scala.collection".to_owned(),
            identity_known: true,
            symbol: None,
        };
        let incomplete_class = PrefixProfile {
            shape: "class TypeRef",
            target_kind: "Class",
            identity: "java.util.List".to_owned(),
            identity_known: true,
            symbol: None,
        };

        assert_eq!(
            member_request_cause("package term member unsupported", &package, &Ok(None)).as_deref(),
            Some("term/object materialization gap")
        );
        assert_eq!(
            member_request_cause(
                "class member scope unavailable: incomplete class info",
                &incomplete_class,
                &Ok(None)
            )
            .as_deref(),
            Some("incomplete class identity")
        );
        assert_eq!(
            member_request_cause(
                "error",
                &package,
                &Err(ResolutionError::Ambiguous { candidates: 2 }),
            )
            .as_deref(),
            Some("other")
        );
        assert_eq!(
            member_request_cause("package type member unresolved", &package, &Ok(None)).as_deref(),
            Some("package resolved but member unresolved")
        );
    }

    #[test]
    fn blocker_attribution_marks_multiple_failed_targets_unknown() {
        let mut profile = ResolutionBlockerProfile::default();
        let trace = [
            ResolverRequestObservation {
                target: "scala.Option".to_owned(),
                outcome: "package term member unsupported".to_owned(),
                cause: Some("term/object materialization gap".to_owned()),
                materialized_prefix: None,
                enclosing_attempt: "Use.scala#tree=12::Use.test".to_owned(),
            },
            ResolverRequestObservation {
                target: "scala.collection.List".to_owned(),
                outcome: "package type member unresolved".to_owned(),
                cause: Some("package resolved but member unresolved".to_owned()),
                materialized_prefix: None,
                enclosing_attempt: "Use.scala#tree=12::Use.test".to_owned(),
            },
        ];

        profile.record_attempt("SymbolResolution", "Use.scala", 12, &trace, None);

        let observation = profile.observations.first().unwrap();
        assert_eq!(
            observation.cause,
            "unknown: multiple failed resolver requests"
        );
        assert_eq!(observation.candidate_causes.len(), 2);
        assert_eq!(observation.targets.len(), 2);
    }

    #[test]
    fn import_qualifier_attribution_uses_the_matching_terminal_request() {
        let mut profile = ResolutionBlockerProfile::default();
        let qualifier = [
            "scala".to_owned(),
            "collection".to_owned(),
            "List".to_owned(),
        ];
        let trace = [
            ResolverRequestObservation {
                target: "scala".to_owned(),
                outcome: "package success".to_owned(),
                cause: None,
                materialized_prefix: None,
                enclosing_attempt: "Use.scala#tree=12::Use.test".to_owned(),
            },
            ResolverRequestObservation {
                target: "scala.collection".to_owned(),
                outcome: "package success".to_owned(),
                cause: None,
                materialized_prefix: None,
                enclosing_attempt: "Use.scala#tree=12::Use.test".to_owned(),
            },
            ResolverRequestObservation {
                target: "scala.collection.List".to_owned(),
                outcome: "package unresolved".to_owned(),
                cause: Some("first segment/package unresolved".to_owned()),
                materialized_prefix: None,
                enclosing_attempt: "Use.scala#tree=12::Use.test".to_owned(),
            },
            ResolverRequestObservation {
                target: "prefix=scala.collection::namespace=type::name=List::space=Prefix"
                    .to_owned(),
                outcome: "package type member unresolved".to_owned(),
                cause: Some("package resolved but member unresolved".to_owned()),
                materialized_prefix: Some("scala/Function1".to_owned()),
                enclosing_attempt: "Use.scala#tree=12::Use.test".to_owned(),
            },
            ResolverRequestObservation {
                target: "prefix=java.util::namespace=type::name=Map::space=Prefix".to_owned(),
                outcome: "class member scope unavailable: incomplete class info".to_owned(),
                cause: Some("incomplete class identity".to_owned()),
                materialized_prefix: None,
                enclosing_attempt: "Use.scala#tree=12::Use.test".to_owned(),
            },
        ];

        profile.record_attempt(
            "ImportQualifierNotFound",
            "Use.scala",
            12,
            &trace,
            Some(&qualifier),
        );

        let observation = profile.observations.first().unwrap();
        assert_eq!(observation.cause, "package resolved but member unresolved");
        assert_eq!(observation.targets.len(), 2);
        assert_eq!(observation.failed_qualifier, "scala.collection.List");
        assert_eq!(
            observation.request_attempts,
            BTreeSet::from(["Use.scala#tree=12::Use.test".to_owned()])
        );
        assert_eq!(
            observation.materialized_prefixes,
            BTreeSet::from(["scala/Function1".to_owned()])
        );
        assert_eq!(
            observation.candidate_causes,
            BTreeSet::from([
                "first segment/package unresolved".to_owned(),
                "package resolved but member unresolved".to_owned()
            ])
        );
    }

    #[test]
    fn import_prefix_profile_keeps_repeated_source_observations() {
        let mut profile = ImportPrefixProfile::default();
        let observation = ImportPrefixObservation {
            source_file: "Use.scala".to_owned(),
            method: "Use.scala#tree=7".to_owned(),
            blocker_origin: "direct:Use.scala#tree=7".to_owned(),
            import_tree: "Use.scala#tree=12".to_owned(),
            import_line: 3,
            qualifier_source: "missing.pkg".to_owned(),
            qualifier_segments: "missing.pkg".to_owned(),
            context_owner: "Use".to_owned(),
            context_package: "<root-or-unavailable>".to_owned(),
            namespace: "Type".to_owned(),
            source_lookup: "missing".to_owned(),
            source_package_registry_lookup: "missing".to_owned(),
            resolved_prefix: "".to_owned(),
            first_failing_segment: "missing".to_owned(),
            request_kind: "resolve_package".to_owned(),
            resolver_outcome: "package unresolved:first segment/package unresolved".to_owned(),
            target_key: "missing.pkg::first-segment-unresolved".to_owned(),
        };
        profile.observations.push(observation.clone());
        profile.observations.push(observation);
        assert_eq!(profile.observations.len(), 2);
        assert_eq!(
            profile
                .observations
                .iter()
                .map(|row| row.target_key.as_str())
                .collect::<BTreeSet<_>>()
                .len(),
            1
        );
    }

    #[test]
    fn source_package_inventory_contains_namer_entered_package_paths() {
        let inventory = audit_source_package_inventory(
            "package alpha.beta\nobject Fixture { def method = 1 }\n",
            "Fixture.scala",
        );

        assert!(inventory.contains("alpha"));
        assert!(inventory.contains("alpha.beta"));
    }

    #[test]
    fn term_name_blocker_records_the_failed_package_term_qualifier() {
        let mut profile = ResolutionBlockerProfile::default();
        let trace = [ResolverRequestObservation {
            target: "prefix=scala.util::namespace=term::name=Failure::space=Prefix".to_owned(),
            outcome: "package term member unsupported".to_owned(),
            cause: Some("term/object materialization gap".to_owned()),
            materialized_prefix: None,
            enclosing_attempt: "Use.scala#tree=12::Use.test".to_owned(),
        }];

        profile.record_attempt("TermNameNotFound", "Use.scala", 12, &trace, None);

        let observation = profile.observations.first().unwrap();
        assert_eq!(observation.failed_qualifier, "scala.util.Failure");
        assert_eq!(observation.targets.len(), 1);
    }
}
