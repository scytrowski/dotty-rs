//! Shared-store implementation of the core symbol-resolution port.

use crate::binary_name::BinaryName;
use crate::class_path::{ClassPathEntry, safe_package_segments};
use crate::error::ClassLoadError;
use crate::loader::{ClassLoadProfile, ClassLoader};
use crate::session::{LoadingSession, SessionCheckpoint};
use dotty_core::{
    Definitions, MemberRequest, MemberSelector, MemberSpace, Name, Namespace, ResolutionError,
    ResolverCheckpoint, SemanticStore, Symbol, SymbolId, SymbolInfo, SymbolKind, SymbolResolver,
    Type, TypeId, TypeRefTarget,
};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::Instant;

/// Counts and inclusive elapsed time for one resolver phase.
///
/// Timings are diagnostic only. Parent phases may include nested work, so
/// callers must not sum inclusive values as though they were disjoint.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ResolverPhaseProfile {
    pub calls: u64,
    pub elapsed_ns: u128,
    /// Time attributed to this phase after subtracting explicitly timed child
    /// intervals. Phases without a measured child have `exclusive_ns == elapsed_ns`.
    pub exclusive_ns: u128,
}

/// Opt-in aggregate profile for bounded classpath member resolution.
///
/// The profile contains counters only; it does not retain semantic IDs or
/// per-request strings across a resolver transaction.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ClasspathResolverProfile {
    pub member_lookups: u64,
    pub member_successes: u64,
    pub member_misses: u64,
    pub member_errors: u64,
    pub rollbacks: u64,
    pub member_lookup_inclusive: ResolverPhaseProfile,
    pub prefix_classification: ResolverPhaseProfile,
    pub module_cache_validation: ResolverPhaseProfile,
    pub target_resolution: ResolverPhaseProfile,
    pub declaration_selection: ResolverPhaseProfile,
    pub package_term_selection: ResolverPhaseProfile,
    pub module_identity_resolution: ResolverPhaseProfile,
    pub module_completion: ResolverPhaseProfile,
    pub class_completion: ResolverPhaseProfile,
    pub class_completion_nested_classload_ns: u128,
    pub module_snapshot_validation: ResolverPhaseProfile,
    pub classpath_resource_lookup: ResolverPhaseProfile,
    pub classfile_decode: ResolverPhaseProfile,
    pub tasty_decode: ResolverPhaseProfile,
    pub transaction_bookkeeping: ResolverPhaseProfile,
    pub rollback: ResolverPhaseProfile,
    pub class_loading_inclusive: ResolverPhaseProfile,
    pub negative_member_cache_hits: u64,
    pub positive_module_cache_hits: u64,
    pub module_cache_misses: u64,
    pub module_completion_attempts: u64,
    pub module_completion_successes: u64,
    pub module_completion_errors: u64,
    pub class_completion_attempts: u64,
    pub class_completion_successes: u64,
    pub class_completion_errors: u64,
}

#[derive(Clone, Copy)]
enum ResolverProfilePhase {
    MemberLookupInclusive,
    PrefixClassification,
    ModuleCacheValidation,
    TargetResolution,
    DeclarationSelection,
    ModuleIdentityResolution,
    PackageTermSelection,
    ModuleCompletion,
    ClassCompletion,
    ModuleSnapshotValidation,
    ClasspathResourceLookup,
    ClassfileDecode,
    TastyDecode,
    TransactionBookkeeping,
    Rollback,
    ClassLoadingInclusive,
}

impl ClasspathResolverProfile {
    pub fn merge(&mut self, other: &Self) {
        self.member_lookups += other.member_lookups;
        self.member_successes += other.member_successes;
        self.member_misses += other.member_misses;
        self.member_errors += other.member_errors;
        self.rollbacks += other.rollbacks;
        self.member_lookup_inclusive
            .merge(&other.member_lookup_inclusive);
        self.prefix_classification
            .merge(&other.prefix_classification);
        self.module_cache_validation
            .merge(&other.module_cache_validation);
        self.target_resolution.merge(&other.target_resolution);
        self.declaration_selection
            .merge(&other.declaration_selection);
        self.package_term_selection
            .merge(&other.package_term_selection);
        self.module_identity_resolution
            .merge(&other.module_identity_resolution);
        self.module_completion.merge(&other.module_completion);
        self.class_completion.merge(&other.class_completion);
        self.class_completion_nested_classload_ns += other.class_completion_nested_classload_ns;
        self.module_snapshot_validation
            .merge(&other.module_snapshot_validation);
        self.classpath_resource_lookup
            .merge(&other.classpath_resource_lookup);
        self.classfile_decode.merge(&other.classfile_decode);
        self.tasty_decode.merge(&other.tasty_decode);
        self.transaction_bookkeeping
            .merge(&other.transaction_bookkeeping);
        self.rollback.merge(&other.rollback);
        self.class_loading_inclusive
            .merge(&other.class_loading_inclusive);
        self.negative_member_cache_hits += other.negative_member_cache_hits;
        self.positive_module_cache_hits += other.positive_module_cache_hits;
        self.module_cache_misses += other.module_cache_misses;
        self.module_completion_attempts += other.module_completion_attempts;
        self.module_completion_successes += other.module_completion_successes;
        self.module_completion_errors += other.module_completion_errors;
        self.class_completion_attempts += other.class_completion_attempts;
        self.class_completion_successes += other.class_completion_successes;
        self.class_completion_errors += other.class_completion_errors;
    }

    fn record(&mut self, phase: ResolverProfilePhase, started: Instant) {
        let profile = match phase {
            ResolverProfilePhase::MemberLookupInclusive => &mut self.member_lookup_inclusive,
            ResolverProfilePhase::PrefixClassification => &mut self.prefix_classification,
            ResolverProfilePhase::ModuleCacheValidation => &mut self.module_cache_validation,
            ResolverProfilePhase::TargetResolution => &mut self.target_resolution,
            ResolverProfilePhase::DeclarationSelection => &mut self.declaration_selection,
            ResolverProfilePhase::PackageTermSelection => &mut self.package_term_selection,
            ResolverProfilePhase::ModuleIdentityResolution => &mut self.module_identity_resolution,
            ResolverProfilePhase::ModuleCompletion => &mut self.module_completion,
            ResolverProfilePhase::ClassCompletion => &mut self.class_completion,
            ResolverProfilePhase::ModuleSnapshotValidation => &mut self.module_snapshot_validation,
            ResolverProfilePhase::ClasspathResourceLookup => &mut self.classpath_resource_lookup,
            ResolverProfilePhase::ClassfileDecode => &mut self.classfile_decode,
            ResolverProfilePhase::TastyDecode => &mut self.tasty_decode,
            ResolverProfilePhase::TransactionBookkeeping => &mut self.transaction_bookkeeping,
            ResolverProfilePhase::Rollback => &mut self.rollback,
            ResolverProfilePhase::ClassLoadingInclusive => &mut self.class_loading_inclusive,
        };
        profile.calls += 1;
        let elapsed_ns = started.elapsed().as_nanos();
        profile.elapsed_ns += elapsed_ns;
        profile.exclusive_ns += elapsed_ns;
    }

    fn record_elapsed(
        &mut self,
        phase: ResolverProfilePhase,
        calls: u64,
        elapsed_ns: u128,
        exclusive_ns: u128,
    ) {
        let profile = match phase {
            ResolverProfilePhase::MemberLookupInclusive => &mut self.member_lookup_inclusive,
            ResolverProfilePhase::PrefixClassification => &mut self.prefix_classification,
            ResolverProfilePhase::ModuleCacheValidation => &mut self.module_cache_validation,
            ResolverProfilePhase::TargetResolution => &mut self.target_resolution,
            ResolverProfilePhase::DeclarationSelection => &mut self.declaration_selection,
            ResolverProfilePhase::PackageTermSelection => &mut self.package_term_selection,
            ResolverProfilePhase::ModuleIdentityResolution => &mut self.module_identity_resolution,
            ResolverProfilePhase::ModuleCompletion => &mut self.module_completion,
            ResolverProfilePhase::ClassCompletion => &mut self.class_completion,
            ResolverProfilePhase::ModuleSnapshotValidation => &mut self.module_snapshot_validation,
            ResolverProfilePhase::ClasspathResourceLookup => &mut self.classpath_resource_lookup,
            ResolverProfilePhase::ClassfileDecode => &mut self.classfile_decode,
            ResolverProfilePhase::TastyDecode => &mut self.tasty_decode,
            ResolverProfilePhase::TransactionBookkeeping => &mut self.transaction_bookkeeping,
            ResolverProfilePhase::Rollback => &mut self.rollback,
            ResolverProfilePhase::ClassLoadingInclusive => &mut self.class_loading_inclusive,
        };
        profile.calls += calls;
        profile.elapsed_ns += elapsed_ns;
        profile.exclusive_ns += exclusive_ns;
    }
}

impl ResolverPhaseProfile {
    fn merge(&mut self, other: &Self) {
        self.calls += other.calls;
        self.elapsed_ns += other.elapsed_ns;
        self.exclusive_ns += other.exclusive_ns;
    }
}

struct ResolverUndo {
    session: SessionCheckpoint,
    object_class: Symbol,
}

/// Stable observations for package-term and external-module member requests.
/// Target keys use binary names and semantic member namespaces, never arena IDs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ModuleResolutionProfile {
    /// Unique package-term requests observed.
    pub package_term_requests: u64,
    /// Package-term requests returning canonical Object symbols.
    pub package_term_successes: u64,
    /// Package-term requests with no supported module evidence.
    pub package_term_unresolved: u64,
    /// Package-term requests that returned a non-Object term symbol.
    pub package_term_non_object_results: u64,
    /// Package-term requests rejected because module evidence or identity was malformed.
    pub package_term_errors: u64,
    /// Object identities newly materialized by successful package-term requests.
    pub new_object_identities: u64,
    /// Existing Object identities reused by successful package-term requests.
    pub reused_object_identities: u64,
    /// Sorted package/name keys that resolved to a canonical Object symbol.
    pub resolved_object_targets: std::collections::BTreeSet<String>,
    /// Field, method, and type-member successes through module prefixes.
    pub module_member_successes: u64,
    /// Sorted module/member/namespace/space keys that resolved successfully.
    pub resolved_member_targets: std::collections::BTreeSet<String>,
    /// Sorted package/name keys observed by package-term requests.
    pub package_term_targets: std::collections::BTreeSet<String>,
    /// Sorted binary names of canonical objects targeted by module-prefix requests.
    pub module_targets: std::collections::BTreeSet<String>,
    /// Sorted module/member/namespace/space keys attempted by module-prefix requests.
    pub member_targets: std::collections::BTreeSet<String>,
    /// Sorted module binary names whose ModuleClass completion succeeded.
    pub completed_module_classes: std::collections::BTreeSet<String>,
    /// Module completion requests served from the resolver-local decoded TASTy cache.
    pub module_decode_cache_hits: u64,
    /// Module completion requests that successfully populated the decoded TASTy cache.
    pub module_decode_cache_misses: u64,
    /// Member misses answered from declarations of a previously completed module.
    pub module_declaration_miss_shortcuts: u64,
}

/// Resolves core symbols from a classpath while sharing the caller's store,
/// canonical builtin definitions, and positive class/package identities.
///
/// Prefixes are supported when they are package/class `TypeRef`s, `TermRef`s
/// to validated external objects, `Applied` types over those references, or
/// `ThisType`s naming a class or package. External object members are read
/// from their canonical module classes. This resolver returns declarations
/// only; member adaptation, inheritance lookup, and overload ranking remain
/// the caller's responsibility.
pub struct ClasspathSymbolResolver<E: ClassPathEntry> {
    class_path: Option<E>,
    definitions: Definitions,
    session: LoadingSession,
    undo: Vec<ResolverUndo>,
    load_profiles: Vec<ClassLoadProfile>,
    collect_load_profiles: bool,
    module_profile: ModuleResolutionProfile,
    collect_module_resolution_profile: bool,
    resolver_profile: ClasspathResolverProfile,
    collect_resolver_profile: bool,
    negative_module_member_lookups: HashSet<(BinaryName, Name)>,
    // Decoded facts outlive semantic-store transactions, but every reuse is
    // guarded by the exact ClassResource snapshot from which they were decoded.
    // Entries never retain SymbolId, TypeId, or ScopeId across rollback.
    decoded_module_cache: HashMap<BinaryName, crate::loader::CachedDecodedModule>,
    // This proof is usable only while the current classpath resource equals
    // every resource snapshot used to validate the module's hierarchy.
    validated_module_declarations: HashSet<BinaryName>,
    validated_module_resources:
        HashMap<BinaryName, BTreeMap<BinaryName, crate::class_path::ClassResource>>,
    #[cfg(test)]
    package_failure_after_segment: Option<usize>,
}

impl<E: ClassPathEntry> ClasspathSymbolResolver<E> {
    pub fn new(class_path: E, definitions: Definitions, session: LoadingSession) -> Self {
        Self {
            class_path: Some(class_path),
            definitions,
            session,
            undo: Vec::new(),
            load_profiles: Vec::new(),
            collect_load_profiles: false,
            module_profile: ModuleResolutionProfile::default(),
            collect_module_resolution_profile: false,
            resolver_profile: ClasspathResolverProfile::default(),
            collect_resolver_profile: false,
            negative_module_member_lookups: HashSet::new(),
            decoded_module_cache: HashMap::new(),
            validated_module_declarations: HashSet::new(),
            validated_module_resources: HashMap::new(),
            #[cfg(test)]
            package_failure_after_segment: None,
        }
    }

    /// Enables per-request class dependency observations for corpus audits
    /// and diagnostics. Disabled by default to avoid collecting session data
    /// in ordinary compiler use.
    pub fn with_load_profile_collection(mut self) -> Self {
        self.collect_load_profiles = true;
        self
    }

    /// Enables package-term and external-module member observations for
    /// audits. Disabled by default to keep ordinary resolver requests lean.
    pub fn with_module_resolution_profile_collection(mut self) -> Self {
        self.collect_module_resolution_profile = true;
        self
    }

    /// Enables aggregate member-resolution phase timings for bounded audits.
    /// Disabled by default; normal resolution performs no timing calls.
    pub fn with_resolver_profile_collection(mut self) -> Self {
        self.collect_resolver_profile = true;
        self
    }

    /// Hands the shared positive class/package cache to another resolver.
    pub fn into_session(self) -> LoadingSession {
        self.session
    }

    /// Returns and clears class dependency profiles collected by resolver
    /// requests since the previous call.
    pub fn take_load_profiles(&mut self) -> Vec<ClassLoadProfile> {
        std::mem::take(&mut self.load_profiles)
    }

    /// Returns and clears stable package-term and module-member observations.
    pub fn take_module_resolution_profile(&mut self) -> ModuleResolutionProfile {
        std::mem::take(&mut self.module_profile)
    }

    /// Returns and clears member-resolution counters and timings.
    pub fn take_resolver_profile(&mut self) -> ClasspathResolverProfile {
        std::mem::take(&mut self.resolver_profile)
    }

    fn record_resolver_phase(&mut self, phase: ResolverProfilePhase, started: Option<Instant>) {
        if let Some(started) = started {
            self.resolver_profile.record(phase, started);
        }
    }

    fn record_resolver_phase_with_children(
        &mut self,
        phase: ResolverProfilePhase,
        started: Option<Instant>,
        child_ns: u128,
    ) -> u128 {
        let Some(started) = started else {
            return 0;
        };
        let elapsed_ns = started.elapsed().as_nanos();
        self.resolver_profile.record_elapsed(
            phase,
            1,
            elapsed_ns,
            elapsed_ns.saturating_sub(child_ns),
        );
        elapsed_ns
    }

    fn collect_class_load_profile(&mut self, profile: &ClassLoadProfile) {
        if !self.collect_resolver_profile {
            return;
        }
        self.resolver_profile.record_elapsed(
            ResolverProfilePhase::ClasspathResourceLookup,
            profile.resource_lookup_calls as u64,
            profile.resource_lookup_ns,
            profile.resource_lookup_ns,
        );
        self.resolver_profile.record_elapsed(
            ResolverProfilePhase::ClassfileDecode,
            profile.classfile_decode_calls as u64,
            profile.classfile_decode_ns,
            profile.classfile_decode_exclusive_ns,
        );
        self.resolver_profile.record_elapsed(
            ResolverProfilePhase::TastyDecode,
            profile.tasty_decode_calls as u64,
            profile.tasty_decode_ns,
            profile.tasty_decode_exclusive_ns,
        );
        self.resolver_profile.record_elapsed(
            ResolverProfilePhase::ModuleCompletion,
            profile.module_completion_calls as u64,
            profile.module_completion_inclusive_ns,
            profile.module_completion_residual_ns,
        );
        self.resolver_profile.module_completion_attempts += profile.module_completion_calls as u64;
        self.resolver_profile.module_completion_successes +=
            profile.module_completion_successes as u64;
        self.resolver_profile.module_completion_errors += profile.module_completion_errors as u64;
    }

    fn finish_member_profile(
        &mut self,
        started: Option<Instant>,
        result: &Result<Option<SymbolId>, ResolutionError>,
    ) {
        let Some(started) = started else {
            return;
        };
        self.resolver_profile
            .record(ResolverProfilePhase::MemberLookupInclusive, started);
        self.resolver_profile.member_lookups += 1;
        match result {
            Ok(Some(_)) => self.resolver_profile.member_successes += 1,
            Ok(None) => {
                self.resolver_profile.member_misses += 1;
                self.resolver_profile.rollbacks += 1;
            }
            Err(_) => {
                self.resolver_profile.member_errors += 1;
                self.resolver_profile.rollbacks += 1;
            }
        }
    }

    fn package_term_target_key(
        &self,
        store: &SemanticStore,
        request: &MemberRequest,
    ) -> Option<String> {
        if request.selector != MemberSelector::Unique
            || request.name.namespace() != Namespace::Term
            || !matches!(request.space, MemberSpace::Prefix)
        {
            return None;
        }
        let Some(PrefixTarget::Package(package)) =
            self.prefix_target(store, request.prefix).ok().flatten()
        else {
            return None;
        };
        let package = package_path_of_symbol(store, package)?.join(".");
        let name = store.names.resolve(request.name.text());
        Some(format!("{package}::{name}"))
    }

    fn module_prefix_key(
        &self,
        store: &SemanticStore,
        request: &MemberRequest,
    ) -> Option<(SymbolId, String)> {
        if request.selector != MemberSelector::Unique {
            return None;
        }
        let owner = match request.space {
            MemberSpace::Prefix => request.prefix,
            MemberSpace::Explicit(owner) => owner,
            _ => return None,
        };
        self.module_identity_for_type(store, owner, 0)
    }

    fn module_identity_for_type(
        &self,
        store: &SemanticStore,
        ty: TypeId,
        depth: u8,
    ) -> Option<(SymbolId, String)> {
        if depth >= 16 || !store.types.contains(ty) {
            return None;
        }
        match store.types.get(ty) {
            Type::TermRef {
                target: dotty_core::TermRefTarget::Symbol(object),
                ..
            } => self.session.module_for_object(*object).map(|identity| {
                (
                    identity.module_class,
                    identity.object_name.as_internal().to_owned(),
                )
            }),
            Type::TypeRef {
                target: TypeRefTarget::Symbol(module_class),
                ..
            }
            | Type::ThisType {
                class: module_class,
            } => self
                .session
                .module_for_class(*module_class)
                .map(|identity| {
                    (
                        identity.module_class,
                        identity.object_name.as_internal().to_owned(),
                    )
                }),
            Type::Applied { tycon, .. } => self.module_identity_for_type(store, *tycon, depth + 1),
            _ => None,
        }
    }
    fn transact<T>(
        &mut self,
        store: &mut SemanticStore,
        operation: impl FnOnce(&mut Self, &mut SemanticStore) -> Result<Option<T>, ResolutionError>,
    ) -> Result<Option<T>, ResolutionError> {
        let transaction_started = self.collect_resolver_profile.then(Instant::now);
        let store_checkpoint = store.checkpoint();
        let session_checkpoint = self.session.checkpoint();
        let object_class = store.symbols.get(self.definitions.object_class).clone();
        let rollback_before = self.resolver_profile.rollback.elapsed_ns;
        let operation_started = self.collect_resolver_profile.then(Instant::now);

        let operation_result = operation(self, store);
        let operation_elapsed_ns =
            operation_started.map_or(0, |started| started.elapsed().as_nanos());
        let result = match operation_result {
            Ok(Some(value)) => {
                let changed = self.session.checkpoint() != session_checkpoint
                    || *store.symbols.get(self.definitions.object_class) != object_class;
                if changed {
                    self.undo.push(ResolverUndo {
                        session: session_checkpoint,
                        object_class,
                    });
                }
                Ok(Some(value))
            }
            Ok(None) => {
                self.rollback_operation(store, store_checkpoint, session_checkpoint, object_class);
                Ok(None)
            }
            Err(error) => {
                self.rollback_operation(store, store_checkpoint, session_checkpoint, object_class);
                Err(error)
            }
        };
        if let Some(started) = transaction_started {
            let elapsed_ns = started.elapsed().as_nanos();
            let rollback_ns = self
                .resolver_profile
                .rollback
                .elapsed_ns
                .saturating_sub(rollback_before);
            self.resolver_profile.record_elapsed(
                ResolverProfilePhase::TransactionBookkeeping,
                1,
                elapsed_ns,
                elapsed_ns.saturating_sub(operation_elapsed_ns + rollback_ns),
            );
        }
        result
    }

    fn rollback_operation(
        &mut self,
        store: &mut SemanticStore,
        store_checkpoint: dotty_core::StoreCheckpoint,
        session_checkpoint: SessionCheckpoint,
        object_class: Symbol,
    ) {
        let rollback_started = self.collect_resolver_profile.then(Instant::now);
        self.session.rollback_to(store, session_checkpoint);
        *store.symbols.get_mut(self.definitions.object_class) = object_class;
        store.rollback_to(store_checkpoint);
        self.record_resolver_phase(ResolverProfilePhase::Rollback, rollback_started);
    }

    fn load_class(
        &mut self,
        store: &mut SemanticStore,
        name: &BinaryName,
    ) -> Result<SymbolId, ClassLoadError> {
        let profile_started = self.collect_resolver_profile.then(Instant::now);
        let class_path = self
            .class_path
            .take()
            .expect("resolver operation is exclusive");
        let session = std::mem::replace(&mut self.session, LoadingSession::new());
        let mut loader =
            ClassLoader::with_definitions(class_path, store, self.definitions, session);
        loader.set_load_profile_collection(
            self.collect_load_profiles || self.collect_resolver_profile,
        );
        let result = loader.load_class(name);
        for profile in loader.take_load_profiles() {
            self.collect_class_load_profile(&profile);
            self.load_profiles.push(profile);
        }
        let (class_path, session) = loader.into_parts();
        self.class_path = Some(class_path);
        self.session = session;
        self.record_resolver_phase(ResolverProfilePhase::ClassLoadingInclusive, profile_started);
        result
    }

    fn ensure_module_identity(
        &mut self,
        store: &mut SemanticStore,
        name: &BinaryName,
    ) -> Result<crate::loader::ModuleIdentity, ClassLoadError> {
        let class_path = self
            .class_path
            .take()
            .expect("resolver operation is exclusive");
        let session = std::mem::replace(&mut self.session, LoadingSession::new());
        let mut loader =
            ClassLoader::with_definitions(class_path, store, self.definitions, session);
        loader.set_load_profile_collection(
            self.collect_load_profiles || self.collect_resolver_profile,
        );
        let result = loader.ensure_module_identity(name);
        for profile in loader.take_load_profiles() {
            self.collect_class_load_profile(&profile);
            self.load_profiles.push(profile);
        }
        let (class_path, session) = loader.into_parts();
        self.class_path = Some(class_path);
        self.session = session;
        result
    }

    fn complete_module(
        &mut self,
        store: &mut SemanticStore,
        name: &BinaryName,
    ) -> Result<crate::loader::ModuleIdentity, ClassLoadError> {
        let class_path = self
            .class_path
            .take()
            .expect("resolver operation is exclusive");
        let session = std::mem::replace(&mut self.session, LoadingSession::new());
        let mut loader =
            ClassLoader::with_definitions(class_path, store, self.definitions, session);
        loader.set_load_profile_collection(
            self.collect_load_profiles || self.collect_resolver_profile,
        );
        let (result, decoded_cache_hit, resource_observations, resource_observations_complete) =
            loader.complete_module_with_tasty_cache(name, &mut self.decoded_module_cache);
        let load_profiles = loader.take_load_profiles();
        for profile in &load_profiles {
            self.collect_class_load_profile(profile);
        }
        self.load_profiles.extend(load_profiles);
        if result.is_ok() && resource_observations_complete {
            self.validated_module_declarations.insert(name.clone());
            self.validated_module_resources
                .insert(name.clone(), resource_observations);
        } else {
            self.validated_module_declarations.remove(name);
            self.validated_module_resources.remove(name);
        }
        if self.collect_module_resolution_profile {
            if decoded_cache_hit {
                self.module_profile.module_decode_cache_hits += 1;
            } else if self.decoded_module_cache.contains_key(name) {
                self.module_profile.module_decode_cache_misses += 1;
            }
        }
        let (class_path, session) = loader.into_parts();
        self.class_path = Some(class_path);
        self.session = session;
        result
    }

    fn try_ensure_module_identity(
        &mut self,
        store: &mut SemanticStore,
        name: &BinaryName,
    ) -> Result<Option<crate::loader::ModuleIdentity>, ResolutionError> {
        let lookup_started = self.collect_resolver_profile.then(Instant::now);
        let resource_result = self
            .class_path
            .as_ref()
            .expect("resolver operation is exclusive")
            .find_class(name)
            .map_err(|error| ResolutionError::Malformed {
                reason: format!("cannot inspect external module {name}: {error}"),
            });
        self.record_resolver_phase(
            ResolverProfilePhase::ClasspathResourceLookup,
            lookup_started,
        );
        let resource = resource_result?;
        let Some(resource) = resource else {
            return Ok(None);
        };
        if resource.format() != crate::class_path::ClassFormat::Tasty {
            return Ok(None);
        }
        match self.ensure_module_identity(store, name) {
            Ok(identity) => Ok(Some(identity)),
            Err(ClassLoadError::IdentityConflict { reason, .. })
                if reason
                    == "the TASTy file does not prove a top-level object and module class" =>
            {
                Ok(None)
            }
            Err(ClassLoadError::NotFound(_)) => Ok(None),
            Err(error) => Err(ResolutionError::Malformed {
                reason: format!("external module identity failed for {name}: {error}"),
            }),
        }
    }

    fn validate_module_object(
        &self,
        store: &SemanticStore,
        package: SymbolId,
        object: SymbolId,
    ) -> Result<(), ResolutionError> {
        let malformed = || ResolutionError::Malformed {
            reason: format!("package term {object:?} is not a valid canonical module object"),
        };
        let Some(identity) = self.session.module_for_object(object) else {
            return Err(malformed());
        };
        if !store.symbols.contains(object)
            || !store.symbols.contains(identity.module_class)
            || !store.symbols.contains(package)
            || store.symbols.get(package).kind != SymbolKind::Package
        {
            return Err(malformed());
        }
        let object_symbol = store.symbols.get(object).clone();
        let module_class = store.symbols.get(identity.module_class).clone();
        let expected_module_name = format!("{}$", store.names.resolve(object_symbol.name.text()));
        let package_path = package_path_of_symbol(store, package).ok_or_else(malformed)?;
        let expected_object_binary = if package_path.is_empty() {
            store.names.resolve(object_symbol.name.text()).to_owned()
        } else {
            format!(
                "{}/{}",
                package_path.join("/"),
                store.names.resolve(object_symbol.name.text())
            )
        };
        let Some(scope) = self.package_scope(package) else {
            return Err(malformed());
        };
        if !store.scopes.contains(scope) || store.scopes.get(scope).owner != Some(package) {
            return Err(malformed());
        }
        if object_symbol.kind != SymbolKind::Object
            || object_symbol.name.namespace() != Namespace::Term
            || object_symbol.owner != Some(package)
            || module_class.kind != SymbolKind::ModuleClass
            || module_class.name.namespace() != Namespace::Type
            || module_class.owner != Some(package)
            || store.names.resolve(module_class.name.text()) != expected_module_name
            || identity.object_name.as_internal() != expected_object_binary
            || identity.module_class_name.as_internal() != format!("{expected_object_binary}$")
            || store.scopes.get(scope).lookup_all(&object_symbol.name) != [object]
            || store.scopes.get(scope).lookup_all(&module_class.name) != [identity.module_class]
        {
            return Err(malformed());
        }
        let object_complete = match object_symbol.info {
            SymbolInfo::Missing => false,
            SymbolInfo::Complete(ty) if matches!(store.types.try_get(ty), Some(Type::TypeRef { target: TypeRefTarget::Symbol(symbol), .. }) if *symbol == identity.module_class) => {
                true
            }
            _ => return Err(malformed()),
        };
        let module_class_complete = match module_class.info {
            SymbolInfo::Missing => false,
            SymbolInfo::Complete(info) => {
                let Some(Type::ClassInfo(info)) = store.types.try_get(info) else {
                    return Err(malformed());
                };
                if info.class != identity.module_class
                    || !store.scopes.contains(info.declarations)
                    || store.scopes.get(info.declarations).owner != Some(identity.module_class)
                {
                    return Err(malformed());
                }
                true
            }
            SymbolInfo::Error | SymbolInfo::Deferred(_) => return Err(malformed()),
        };
        if object_complete != module_class_complete {
            return Err(malformed());
        }
        Ok(())
    }

    fn package_scope(&self, package: SymbolId) -> Option<dotty_core::ScopeId> {
        self.session.packages.scope_of(package)
    }

    fn complete_class_inner(
        &mut self,
        store: &mut SemanticStore,
        symbol: SymbolId,
    ) -> Result<Option<bool>, ResolutionError> {
        let started = self.collect_resolver_profile.then(Instant::now);
        let classload_before = self.resolver_profile.class_loading_inclusive.elapsed_ns;
        let result = self.complete_class_inner_impl(store, symbol);
        if let Some(started) = started {
            let elapsed_ns = started.elapsed().as_nanos();
            let classload_ns = self
                .resolver_profile
                .class_loading_inclusive
                .elapsed_ns
                .saturating_sub(classload_before);
            self.resolver_profile.record_elapsed(
                ResolverProfilePhase::ClassCompletion,
                1,
                elapsed_ns,
                elapsed_ns.saturating_sub(classload_ns),
            );
            self.resolver_profile.class_completion_nested_classload_ns += classload_ns;
            self.resolver_profile.class_completion_attempts += 1;
            match &result {
                Ok(Some(true)) => self.resolver_profile.class_completion_successes += 1,
                Ok(Some(false) | None) => {}
                Err(_) => self.resolver_profile.class_completion_errors += 1,
            }
        }
        result
    }

    fn complete_class_inner_impl(
        &mut self,
        store: &mut SemanticStore,
        symbol: SymbolId,
    ) -> Result<Option<bool>, ResolutionError> {
        let names: Vec<_> = self
            .session
            .identified
            .iter()
            .filter(|(_, id)| **id == symbol)
            .map(|(name, _)| name.clone())
            .collect();
        if names.is_empty() {
            return Ok(None);
        }
        let malformed = |reason: &str| ResolutionError::Malformed {
            reason: format!("cannot complete external class {symbol:?}: {reason}"),
        };
        if !store.symbols.contains(symbol) {
            return Err(malformed("identified symbol no longer exists"));
        }
        let item = store.symbols.get(symbol);
        if !matches!(
            item.origin,
            dotty_core::SymbolOrigin::Classfile(_) | dotty_core::SymbolOrigin::Tasty(_)
        ) {
            return Ok(None);
        }
        if !matches!(
            item.kind,
            SymbolKind::Class | SymbolKind::Trait | SymbolKind::ModuleClass
        ) {
            return Err(malformed("identified symbol is not class-like"));
        }
        let name = names
            .into_iter()
            .find(|name| {
                if !item.name.is_type()
                    || store.names.resolve(item.name.text()) != name.simple_name()
                {
                    return false;
                }
                let path: Vec<_> = name
                    .package_path()
                    .split('/')
                    .filter(|s| !s.is_empty())
                    .collect();
                let Some(package) = self.session.packages.get(&path) else {
                    return false;
                };
                let Some(scope) = self.package_scope(package) else {
                    return false;
                };
                store.symbols.contains(package)
                    && store.symbols.get(package).kind == SymbolKind::Package
                    && store.scopes.contains(scope)
                    && store.scopes.get(scope).owner == Some(package)
                    && store.scopes.get(scope).lookup_all(&item.name) == [symbol]
                    && (item.owner == Some(package)
                        || is_nested_class_indexed_in_package(store, symbol, package))
            })
            .ok_or_else(|| malformed("identity does not match its canonical package/name"))?;

        match item.info {
            SymbolInfo::Missing => {
                let loaded =
                    self.load_class(store, &name)
                        .map_err(|error| ResolutionError::Malformed {
                            reason: format!("external class completion failed for {name}: {error}"),
                        })?;
                if loaded != symbol || self.session.identified.get(&name) != Some(&symbol) {
                    return Err(malformed("loading changed the canonical symbol identity"));
                }
            }
            SymbolInfo::Complete(_) => {}
            SymbolInfo::Error => return Err(malformed("identified symbol has Error info")),
            SymbolInfo::Deferred(_) => {
                return Err(malformed("identified symbol has unsupported Deferred info"));
            }
        }
        let SymbolInfo::Complete(info) = store.symbols.get(symbol).info else {
            return Err(malformed("loading did not complete ClassInfo"));
        };
        let Some(Type::ClassInfo(info)) = store.types.try_get(info) else {
            return Err(malformed("completed info is not a valid ClassInfo"));
        };
        if info.class != symbol
            || !store.scopes.contains(info.declarations)
            || store.scopes.get(info.declarations).owner != Some(symbol)
        {
            return Err(malformed(
                "ClassInfo identity or declaration scope is invalid",
            ));
        }
        self.session.cache_resolved(name, symbol);
        Ok(Some(true))
    }

    fn declaration_scope(
        &self,
        store: &SemanticStore,
        class: SymbolId,
    ) -> Option<dotty_core::ScopeId> {
        if !store.symbols.contains(class) {
            return None;
        }
        let SymbolInfo::Complete(info) = store.symbols.get(class).info else {
            return None;
        };
        let Some(Type::ClassInfo(info)) = store.types.try_get(info) else {
            return None;
        };
        Some(info.declarations)
    }

    fn resolve_target(
        &mut self,
        store: &mut SemanticStore,
        target: PrefixTarget,
    ) -> Result<Option<(SymbolId, dotty_core::ScopeId)>, ResolutionError> {
        let started = self.collect_resolver_profile.then(Instant::now);
        let module_before = self.resolver_profile.module_completion.elapsed_ns;
        let class_before = self.resolver_profile.class_completion.elapsed_ns;
        let result = self.resolve_target_inner(store, target);
        if let Some(started) = started {
            let elapsed_ns = started.elapsed().as_nanos();
            let nested_ns = self
                .resolver_profile
                .module_completion
                .elapsed_ns
                .saturating_sub(module_before)
                + self
                    .resolver_profile
                    .class_completion
                    .elapsed_ns
                    .saturating_sub(class_before);
            self.resolver_profile.record_elapsed(
                ResolverProfilePhase::TargetResolution,
                1,
                elapsed_ns,
                elapsed_ns.saturating_sub(nested_ns),
            );
        }
        result
    }

    fn resolve_target_inner(
        &mut self,
        store: &mut SemanticStore,
        target: PrefixTarget,
    ) -> Result<Option<(SymbolId, dotty_core::ScopeId)>, ResolutionError> {
        let (symbol, scope) = match target {
            PrefixTarget::Package(package) => {
                let Some(scope) = self.package_scope(package) else {
                    return Ok(None);
                };
                if !store.scopes.contains(scope) {
                    return Err(ResolutionError::Malformed {
                        reason: format!("package {package:?} has an invalid declaration scope"),
                    });
                }
                (package, scope)
            }
            PrefixTarget::Class(class) => {
                let mut scope = self.declaration_scope(store, class);
                let module_identity = (store.symbols.get(class).kind == SymbolKind::ModuleClass)
                    .then(|| self.session.module_for_class(class).cloned())
                    .flatten();
                if let Some(identity) = module_identity {
                    let package = store.symbols.get(identity.object).owner.ok_or_else(|| {
                        ResolutionError::Malformed {
                            reason: format!(
                                "module class {class:?} has an ownerless object identity"
                            ),
                        }
                    })?;
                    self.validate_module_object(store, package, identity.object)?;
                    // `validate_module_object` checks the completed class info
                    // and the object/module-class completion state. Avoid
                    // calling `complete_module` for every member lookup: its
                    // idempotency check walks every declaration in the module
                    // scope, making repeated lookups quadratic in practice.
                    if matches!(store.symbols.get(class).info, SymbolInfo::Missing) {
                        let completed = self
                            .complete_module(store, &identity.object_name)
                            .map_err(|error| ResolutionError::Malformed {
                                reason: format!("module completion failed: {error}"),
                            })?;
                        if completed.object != identity.object || completed.module_class != class {
                            return Err(ResolutionError::Malformed {
                                reason: format!(
                                    "module completion changed canonical identity for {class:?}"
                                ),
                            });
                        }
                    }
                    scope = self.declaration_scope(store, class);
                } else if scope.is_none() {
                    // Stay inside resolve_member's transaction: None/ambiguity
                    // must undo even a successful prefix completion.
                    self.complete_class_inner(store, class)?;
                    scope = self.declaration_scope(store, class);
                }
                if scope.is_none() && class == self.definitions.object_class {
                    match self.load_class(store, &BinaryName::from_internal("java/lang/Object")) {
                        Ok(loaded) if loaded == class => {
                            scope = self.declaration_scope(store, class);
                        }
                        Ok(_) => {
                            return Err(ResolutionError::Malformed {
                                reason:
                                    "loading java/lang/Object changed its canonical symbol identity"
                                        .to_owned(),
                            });
                        }
                        Err(ClassLoadError::NotFound(_)) => return Ok(None),
                        Err(error) => {
                            return Err(ResolutionError::Malformed {
                                reason: error.to_string(),
                            });
                        }
                    }
                }
                let Some(scope) = scope else {
                    if store.symbols.contains(class)
                        && matches!(store.symbols.get(class).info, SymbolInfo::Complete(_))
                    {
                        return Err(ResolutionError::Malformed {
                            reason: format!("class {class:?} has invalid ClassInfo"),
                        });
                    }
                    // Unknown/source identities are declined by the hook.
                    return Ok(None);
                };
                if !store.scopes.contains(scope) {
                    return Err(ResolutionError::Malformed {
                        reason: format!("class {class:?} has an invalid declaration scope"),
                    });
                }
                let SymbolInfo::Complete(info) = store.symbols.get(class).info else {
                    return Err(ResolutionError::Malformed {
                        reason: format!("class {class:?} lost completed info"),
                    });
                };
                let Some(Type::ClassInfo(info)) = store.types.try_get(info) else {
                    return Err(ResolutionError::Malformed {
                        reason: format!("class {class:?} has invalid ClassInfo"),
                    });
                };
                if info.class != class || store.scopes.get(scope).owner != Some(class) {
                    return Err(ResolutionError::Malformed {
                        reason: format!(
                            "class {class:?} has inconsistent ClassInfo identity/scope"
                        ),
                    });
                }
                (class, scope)
            }
        };
        Ok(Some((symbol, scope)))
    }

    fn prefix_target(
        &self,
        store: &SemanticStore,
        ty: TypeId,
    ) -> Result<Option<PrefixTarget>, ResolutionError> {
        self.prefix_target_bounded(store, ty, 0)
    }

    fn prefix_target_bounded(
        &self,
        store: &SemanticStore,
        ty: TypeId,
        depth: u8,
    ) -> Result<Option<PrefixTarget>, ResolutionError> {
        if depth >= 16 || !store.types.contains(ty) {
            return Ok(None);
        }
        match store.types.get(ty) {
            Type::TermRef {
                target: dotty_core::TermRefTarget::Symbol(symbol),
                ..
            } if !store.symbols.contains(*symbol) => Err(ResolutionError::Malformed {
                reason: format!("term prefix refers to unknown symbol {symbol:?}"),
            }),
            Type::TermRef {
                target: dotty_core::TermRefTarget::Symbol(object),
                ..
            } if store.symbols.contains(*object)
                && store.symbols.get(*object).kind == SymbolKind::Object =>
            {
                let Some(package) = store.symbols.get(*object).owner else {
                    return Err(ResolutionError::Malformed {
                        reason: format!("object prefix {object:?} has no package owner"),
                    });
                };
                self.validate_module_object(store, package, *object)?;
                let identity = self
                    .session
                    .module_for_object(*object)
                    .expect("validated object has a module identity");
                Ok(Some(PrefixTarget::Class(identity.module_class)))
            }
            Type::TypeRef {
                target: TypeRefTarget::Symbol(symbol),
                ..
            } => self.symbol_target(store, *symbol),
            Type::ThisType { class } => self.symbol_target(store, *class),
            Type::Applied { tycon, .. } => self.prefix_target_bounded(store, *tycon, depth + 1),
            _ => Ok(None),
        }
    }

    fn symbol_target(
        &self,
        store: &SemanticStore,
        symbol: SymbolId,
    ) -> Result<Option<PrefixTarget>, ResolutionError> {
        let Some(symbol_data) = store
            .symbols
            .contains(symbol)
            .then(|| store.symbols.get(symbol))
        else {
            return Err(ResolutionError::Malformed {
                reason: format!("prefix refers to unknown symbol {symbol:?}"),
            });
        };
        if symbol_data.kind == SymbolKind::Package {
            return Ok(Some(PrefixTarget::Package(symbol)));
        }
        if !matches!(
            symbol_data.kind,
            SymbolKind::Class | SymbolKind::Trait | SymbolKind::ModuleClass
        ) {
            return Ok(None);
        }
        Ok(Some(PrefixTarget::Class(symbol)))
    }

    fn compiler_builtin_class_alias(
        &self,
        store: &SemanticStore,
        package: SymbolId,
        member: dotty_core::Name,
    ) -> Option<SymbolId> {
        if !self.is_compiler_builtin_alias_name(store, package, member) || !member.is_type() {
            return None;
        }

        match store.names.resolve(member.text()) {
            "Int" => store.types.get(self.definitions.int).reference_symbol(),
            "Any" => Some(self.definitions.any_class),
            "AnyRef" => Some(self.definitions.object_class),
            "Nothing" => Some(self.definitions.nothing_class),
            _ => None,
        }
    }

    fn is_compiler_builtin_alias_name(
        &self,
        store: &SemanticStore,
        package: SymbolId,
        member: dotty_core::Name,
    ) -> bool {
        package_path_of_symbol(store, package).is_some_and(|path| path.as_slice() == ["scala"])
            && matches!(
                store.names.resolve(member.text()),
                "Int" | "Any" | "AnyRef" | "Nothing"
            )
    }

    fn resolve_member_inner(
        &mut self,
        store: &mut SemanticStore,
        request: &MemberRequest,
    ) -> Result<Option<SymbolId>, ResolutionError> {
        if !matches!(request.selector, MemberSelector::Unique) {
            return Ok(None);
        }
        let owner_type = match request.space {
            MemberSpace::Prefix => request.prefix,
            MemberSpace::Explicit(owner) => owner,
            _ => return Ok(None),
        };
        let prefix_started = self.collect_resolver_profile.then(Instant::now);
        let target_result = self.prefix_target(store, owner_type);
        self.record_resolver_phase(ResolverProfilePhase::PrefixClassification, prefix_started);
        let target_result = target_result?;
        let Some(target) = target_result else {
            return Ok(None);
        };
        let module_identity_for_cache = match &target {
            PrefixTarget::Class(class)
                if store.symbols.get(*class).kind == SymbolKind::ModuleClass =>
            {
                self.session.module_for_class(*class).cloned()
            }
            _ => None,
        };
        let module_member_cache_key = module_identity_for_cache
            .as_ref()
            .map(|identity| (identity.object_name.clone(), request.name));
        let cache_validation_started = self.collect_resolver_profile.then(Instant::now);
        let mut snapshot_elapsed_ns = 0;
        if let (Some(cache_key), Some(identity), PrefixTarget::Class(class)) = (
            module_member_cache_key.as_ref(),
            module_identity_for_cache.as_ref(),
            &target,
        ) && matches!(store.symbols.get(*class).info, SymbolInfo::Missing)
        {
            let cached_miss = self.negative_module_member_lookups.contains(cache_key);
            let has_cached_evidence = cached_miss
                || self
                    .validated_module_declarations
                    .contains(&identity.object_name);
            let snapshot_started = self.collect_resolver_profile.then(Instant::now);
            let mut snapshot_lookup_calls = 0_u64;
            let mut snapshot_resource_ns = 0_u128;
            let cache_is_current = if has_cached_evidence {
                let class_path = self
                    .class_path
                    .as_ref()
                    .expect("resolver operation is exclusive");
                let mut is_current = false;
                if let Some(resources) = self
                    .validated_module_resources
                    .get(&identity.object_name)
                    .filter(|resources| !resources.is_empty())
                {
                    is_current = true;
                    for (name, previous) in resources {
                        snapshot_lookup_calls += 1;
                        let lookup_started = self.collect_resolver_profile.then(Instant::now);
                        let current = class_path.find_class(name);
                        snapshot_resource_ns +=
                            lookup_started.map_or(0, |started| started.elapsed().as_nanos());
                        if !current.is_ok_and(|current| current.as_ref() == Some(previous)) {
                            is_current = false;
                            break;
                        }
                    }
                }
                // Keep the decoded root snapshot check explicit: a module can
                // have been decoded before its hierarchy was successfully
                // completed, so only the validated proof makes it reusable.
                is_current &= self
                    .decoded_module_cache
                    .get(&identity.object_name)
                    .is_some_and(|cached| {
                        self.validated_module_resources
                            .get(&identity.object_name)
                            .and_then(|resources| resources.get(&identity.object_name))
                            == Some(&cached.resource)
                    });
                if !is_current {
                    self.decoded_module_cache.remove(&identity.object_name);
                    self.validated_module_declarations
                        .remove(&identity.object_name);
                    self.validated_module_resources
                        .remove(&identity.object_name);
                    self.negative_module_member_lookups
                        .retain(|(module, _)| module != &identity.object_name);
                }
                is_current
            } else {
                false
            };
            if self.collect_resolver_profile {
                self.resolver_profile.record_elapsed(
                    ResolverProfilePhase::ClasspathResourceLookup,
                    snapshot_lookup_calls,
                    snapshot_resource_ns,
                    snapshot_resource_ns,
                );
            }
            snapshot_elapsed_ns = self.record_resolver_phase_with_children(
                ResolverProfilePhase::ModuleSnapshotValidation,
                snapshot_started,
                snapshot_resource_ns,
            );
            if has_cached_evidence && cache_is_current {
                self.resolver_profile.positive_module_cache_hits += 1;
                if cached_miss {
                    self.resolver_profile.negative_member_cache_hits += 1;
                }
            } else {
                self.resolver_profile.module_cache_misses += 1;
            }
            let cached_miss = cached_miss && cache_is_current;
            let declaration_miss = !cached_miss
                && cache_is_current
                && self
                    .validated_module_declarations
                    .contains(&identity.object_name)
                && self
                    .decoded_module_cache
                    .get(&identity.object_name)
                    .is_some_and(|cached| {
                        // Module completion currently publishes ValDef/DefDef terms
                        // only. Inspect the same projection, not raw TASTy names or
                        // inherited members (this resolver returns declarations).
                        let wanted = store.names.resolve(request.name.text());
                        request.name.is_term()
                            && (!cached
                                .decoded
                                .fields
                                .iter()
                                .any(|field| field.name == wanted)
                                && !cached
                                    .decoded
                                    .methods
                                    .iter()
                                    .any(|method| method.name == wanted))
                    });
            if cached_miss || declaration_miss {
                let package = store.symbols.get(identity.object).owner.ok_or_else(|| {
                    ResolutionError::Malformed {
                        reason: format!(
                            "module class {:?} has an ownerless object identity",
                            identity.module_class
                        ),
                    }
                })?;
                self.validate_module_object(store, package, identity.object)?;
                if !cached_miss && declaration_miss && self.collect_module_resolution_profile {
                    self.module_profile.module_declaration_miss_shortcuts += 1;
                }
                self.record_resolver_phase_with_children(
                    ResolverProfilePhase::ModuleCacheValidation,
                    cache_validation_started,
                    snapshot_elapsed_ns,
                );
                return Ok(None);
            }
        }
        self.record_resolver_phase_with_children(
            ResolverProfilePhase::ModuleCacheValidation,
            cache_validation_started,
            snapshot_elapsed_ns,
        );
        let owner_result = self.resolve_target(store, target);
        let owner_result = owner_result?;
        let Some((resolved_owner, scope)) = owner_result else {
            return Ok(None);
        };
        if store.symbols.get(resolved_owner).kind == SymbolKind::Package
            && let Some(alias) =
                self.compiler_builtin_class_alias(store, resolved_owner, request.name)
        {
            return Ok(Some(alias));
        }
        if store.symbols.get(resolved_owner).kind == SymbolKind::Package
            && request.name.namespace() == Namespace::Term
            && self.is_compiler_builtin_alias_name(store, resolved_owner, request.name)
        {
            return Ok(None);
        }
        let selection_started = self.collect_resolver_profile.then(Instant::now);
        let candidates = store.scopes.get(scope).lookup_all(&request.name);
        self.record_resolver_phase(
            ResolverProfilePhase::DeclarationSelection,
            selection_started,
        );
        if candidates.is_empty()
            && request.name.is_term()
            && let Some(cache_key) = module_member_cache_key
        {
            // Module declarations come from immutable classpath TASTy. A
            // missing member stays missing even when this transaction rolls
            // back the expensive on-demand module completion.
            self.negative_module_member_lookups.insert(cache_key);
        }
        match candidates {
            [] if store.symbols.get(resolved_owner).kind == SymbolKind::Package
                && matches!(request.space, MemberSpace::Prefix)
                && request.name.namespace() == Namespace::Term =>
            {
                let package_path =
                    package_path_of_symbol(store, resolved_owner).ok_or_else(|| {
                        ResolutionError::Malformed {
                            reason: "package prefix has an invalid owner chain".to_owned(),
                        }
                    })?;
                let simple_name = store.names.resolve(request.name.text()).to_owned();
                let binary_name = if package_path.is_empty() {
                    simple_name.to_owned()
                } else {
                    format!("{}/{simple_name}", package_path.join("/"))
                };
                let binary_name = BinaryName::from_internal(binary_name);
                if !binary_name.is_path_safe() {
                    return Ok(None);
                }
                let module_identity_started = self.collect_resolver_profile.then(Instant::now);
                let identity = self.try_ensure_module_identity(store, &binary_name);
                self.record_resolver_phase(
                    ResolverProfilePhase::ModuleIdentityResolution,
                    module_identity_started,
                );
                match identity? {
                    Some(identity) => {
                        self.validate_module_object(store, resolved_owner, identity.object)?;
                        Ok(Some(identity.object))
                    }
                    None if package_path.as_slice() == ["scala"] => {
                        let package_object = BinaryName::from_internal("scala/package");
                        let class_path = self
                            .class_path
                            .as_ref()
                            .expect("resolver operation is exclusive");
                        let alias = crate::loader::resolve_tasty_term_alias(
                            class_path,
                            &package_object,
                            &simple_name,
                        )
                        .map_err(|error| ResolutionError::Malformed {
                            reason: format!("cannot resolve scala package term alias: {error:?}"),
                        })?;
                        let Some(alias) = alias else {
                            return Ok(None);
                        };
                        let Some(identity) = self.try_ensure_module_identity(store, &alias)? else {
                            return Ok(None);
                        };
                        let owner = self
                            .session
                            .packages
                            .resolve_package(store, alias.package_path());
                        self.validate_module_object(store, owner, identity.object)?;
                        Ok(Some(identity.object))
                    }
                    None => Ok(None),
                }
            }
            [] if store.symbols.get(resolved_owner).kind == SymbolKind::Package
                && request.name.is_type() =>
            {
                let package_path =
                    package_path_of_symbol(store, resolved_owner).ok_or_else(|| {
                        ResolutionError::Malformed {
                            reason: "package prefix has an invalid owner chain".to_owned(),
                        }
                    })?;
                let simple_name = store.names.resolve(request.name.text()).to_owned();
                let binary_name = if package_path.is_empty() {
                    simple_name.clone()
                } else {
                    format!("{}/{simple_name}", package_path.join("/"))
                };
                let binary_name = BinaryName::from_internal(binary_name);
                if !binary_name.is_path_safe() {
                    return Ok(None);
                }
                match self.load_class(store, &binary_name) {
                    Ok(symbol) => Ok(Some(symbol)),
                    Err(ClassLoadError::NotFound(_)) if package_path.as_slice() == ["scala"] => {
                        let package_object = BinaryName::from_internal("scala/package");
                        let alias_target = crate::loader::resolve_tasty_alias(
                            self.class_path
                                .as_ref()
                                .expect("resolver operation is exclusive"),
                            &package_object,
                            &simple_name,
                        )
                        .map_err(|error| ResolutionError::Malformed {
                            reason: format!("cannot resolve scala package type alias: {error:?}"),
                        })?;
                        let Some(alias_target) = alias_target else {
                            return Ok(None);
                        };
                        self.load_class(store, &alias_target)
                            .map(Some)
                            .map_err(|error| ResolutionError::Malformed {
                                reason: error.to_string(),
                            })
                    }
                    Err(ClassLoadError::NotFound(_)) => Ok(None),
                    Err(error) => Err(ResolutionError::Malformed {
                        reason: error.to_string(),
                    }),
                }
            }
            [] if store.symbols.get(resolved_owner).kind == SymbolKind::ModuleClass
                && request.name.is_type() =>
            {
                let Some(identity) = self.session.module_for_class(resolved_owner).cloned() else {
                    return Ok(None);
                };
                let simple_name = store.names.resolve(request.name.text()).to_owned();
                let binary_name = BinaryName::from_internal(format!(
                    "{}${simple_name}",
                    identity.object_name.as_internal()
                ));
                if !binary_name.is_path_safe() {
                    return Ok(None);
                }
                let symbol = match self.load_class(store, &binary_name) {
                    Ok(symbol) => symbol,
                    Err(ClassLoadError::NotFound(_)) => return Ok(None),
                    Err(error) => {
                        return Err(ResolutionError::Malformed {
                            reason: error.to_string(),
                        });
                    }
                };
                let member = store.symbols.get(symbol);
                if member.owner != Some(resolved_owner) {
                    return Ok(None);
                }
                let Some(metadata) = self.session.metadata.get(&symbol) else {
                    return Ok(None);
                };
                let nested_class_is_proven = metadata.inner_classes.iter().any(|entry| {
                    let inner_matches = match &entry.inner_class {
                        crate::symbol::ClassRef::Resolved(inner) => *inner == symbol,
                        crate::symbol::ClassRef::Unresolved(name) => name == &binary_name,
                    };
                    let outer_matches = match &entry.outer_class {
                        Some(crate::symbol::ClassRef::Resolved(outer)) => {
                            self.session.identified.iter().any(|(name, candidate)| {
                                candidate == outer && name == &identity.object_name
                            })
                        }
                        Some(crate::symbol::ClassRef::Unresolved(name)) => {
                            name == &identity.object_name
                        }
                        None => false,
                    };
                    inner_matches
                        && outer_matches
                        && entry.inner_name.as_deref() == Some(simple_name.as_str())
                });
                if !nested_class_is_proven {
                    return Ok(None);
                }
                let Some(visibility) = identity.nested_type_visibilities.get(&simple_name).copied()
                else {
                    // Classfile InnerClasses access flags can widen Scala
                    // protected/private declarations. Only project nested
                    // types whose declaration visibility is present in the
                    // authoritative module TASTy.
                    return Ok(None);
                };
                let SymbolInfo::Complete(info) = member.info else {
                    return Ok(None);
                };
                let Some(Type::ClassInfo(class_info)) = store.types.try_get(info) else {
                    return Ok(None);
                };
                if class_info.class != symbol
                    || !store.scopes.contains(class_info.declarations)
                    || store.scopes.get(class_info.declarations).owner != Some(symbol)
                {
                    return Ok(None);
                }
                self.session.record_symbol_update(store, symbol);
                store.symbols.get_mut(symbol).visibility = visibility;
                Ok(Some(symbol))
            }
            [] => Ok(None),
            [symbol]
                if store.symbols.get(resolved_owner).kind == SymbolKind::Package
                    && request.name.is_type() =>
            {
                let symbol = *symbol;
                let member = store.symbols.get(symbol);
                if matches!(
                    member.kind,
                    SymbolKind::Class | SymbolKind::Trait | SymbolKind::ModuleClass
                ) {
                    let owner_mismatch = member.owner != Some(resolved_owner)
                        && !is_nested_class_indexed_in_package(store, symbol, resolved_owner);
                    if owner_mismatch || member.name != request.name {
                        return Err(ResolutionError::Malformed {
                            reason: format!(
                                "package type member {symbol:?} does not match its canonical package/name"
                            ),
                        });
                    }
                    if matches!(member.info, SymbolInfo::Missing) {
                        // Member dependencies can already have published this
                        // package identity as a shell. Complete only identities
                        // owned by this session, inside the member transaction.
                        self.complete_class_inner(store, symbol)?;
                    }
                    let member = store.symbols.get(symbol);
                    let SymbolInfo::Complete(info) = member.info else {
                        // A source-owned class is already the canonical identity.
                        // This adapter cannot complete it in place, and loading a
                        // classpath twin would create a duplicate identity.
                        return Ok(None);
                    };
                    let Some(ty) = store.types.try_get(info) else {
                        return Err(ResolutionError::Malformed {
                            reason: format!(
                                "package class {symbol:?} has an invalid or unfilled completion type"
                            ),
                        });
                    };
                    let Type::ClassInfo(class_info) = ty else {
                        return Err(ResolutionError::Malformed {
                            reason: format!(
                                "package class {symbol:?} is complete without ClassInfo"
                            ),
                        });
                    };
                    if class_info.class != symbol
                        || !store.scopes.contains(class_info.declarations)
                        || store.scopes.get(class_info.declarations).owner != Some(symbol)
                    {
                        return Err(ResolutionError::Malformed {
                            reason: format!(
                                "package class {symbol:?} has invalid class identity or declaration scope"
                            ),
                        });
                    }
                }
                Ok(Some(symbol))
            }
            [symbol]
                if store.symbols.get(resolved_owner).kind == SymbolKind::Package
                    && request.name.namespace() == Namespace::Term
                    && store.symbols.get(*symbol).kind == SymbolKind::Object =>
            {
                self.validate_module_object(store, resolved_owner, *symbol)?;
                Ok(Some(*symbol))
            }
            [symbol] => {
                if store.symbols.get(resolved_owner).kind != SymbolKind::Package {
                    if !store.symbols.contains(*symbol) {
                        return Err(ResolutionError::Malformed {
                            reason: format!(
                                "class member scope contains unknown symbol {symbol:?}"
                            ),
                        });
                    }
                    let member = store.symbols.get(*symbol);
                    if member.owner != Some(resolved_owner) || member.name != request.name {
                        return Err(ResolutionError::Malformed {
                            reason: format!(
                                "class member {symbol:?} does not match its declaration owner/name"
                            ),
                        });
                    }
                }
                Ok(Some(*symbol))
            }
            many => Err(ResolutionError::Ambiguous {
                candidates: many.len(),
            }),
        }
    }

    fn resolve_package_inner(
        &mut self,
        store: &mut SemanticStore,
        requested_path: &[&str],
    ) -> Result<Option<SymbolId>, ResolutionError> {
        let path = if requested_path.first() == Some(&"_root_") {
            &requested_path[1..]
        } else {
            requested_path
        };
        if !safe_package_segments(path) || path.contains(&"_root_") {
            return Ok(None);
        }
        if let Some(package) = self.session.packages.get(path) {
            return Ok(Some(package));
        }
        if path.is_empty() {
            return Ok(Some(self.session.packages.resolve_package(store, "")));
        }
        if !self
            .class_path
            .as_ref()
            .expect("resolver operation is exclusive")
            .contains_package_prefix(path)
            .map_err(|error| ResolutionError::Malformed {
                reason: error.to_string(),
            })?
        {
            return Ok(None);
        }

        // Enter the root only after the full target has positive evidence.
        // This lets us detect a conflicting top-level term before Packages
        // adds the first package segment to the root scope.
        self.session.packages.resolve_package(store, "");
        let mut prefix = Vec::with_capacity(path.len());
        let mut resolved = None;
        for (index, segment) in path.iter().enumerate() {
            prefix.push(*segment);
            if let Some(package) = self.session.packages.get(&prefix) {
                resolved = Some(package);
                continue;
            }

            if let Some(parent) = self.session.packages.get(&prefix[..prefix.len() - 1]) {
                let scope = self
                    .session
                    .packages
                    .scope_of(parent)
                    .filter(|scope| store.scopes.contains(*scope))
                    .ok_or_else(|| ResolutionError::Malformed {
                        reason: format!("package {prefix:?} has no valid parent declaration scope"),
                    })?;
                let name = Name::new(store.names.intern(segment), Namespace::Term);
                let occupants = store.scopes.get(scope).lookup_all(&name);
                if !occupants.is_empty() {
                    let non_packages = occupants
                        .iter()
                        .filter(|symbol| {
                            !store.symbols.contains(**symbol)
                                || store.symbols.get(**symbol).kind != SymbolKind::Package
                        })
                        .count();
                    if non_packages > 0 {
                        return Err(ResolutionError::Ambiguous {
                            candidates: occupants.len() + 1,
                        });
                    }
                    return Err(ResolutionError::Malformed {
                        reason: format!(
                            "package path {prefix:?} has an unregistered package in its parent scope"
                        ),
                    });
                }
            } else if index > 0 {
                return Err(ResolutionError::Malformed {
                    reason: format!("package path {prefix:?} has no registered parent package"),
                });
            }

            let package = self
                .session
                .packages
                .resolve_package(store, &prefix.join("/"));
            resolved = Some(package);

            #[cfg(test)]
            if self.package_failure_after_segment == Some(index + 1) {
                return Err(ResolutionError::Malformed {
                    reason: "injected package-resolution failure".to_owned(),
                });
            }
        }
        Ok(resolved)
    }
}

impl<E: ClassPathEntry> SymbolResolver for ClasspathSymbolResolver<E> {
    fn package_object_collision(
        &mut self,
        store: &mut SemanticStore,
        package: SymbolId,
        name: Name,
    ) -> Result<bool, ResolutionError> {
        self.transact(store, |resolver, store| {
            if !store.symbols.contains(package)
                || store.symbols.get(package).kind != SymbolKind::Package
            {
                return Err(ResolutionError::Malformed {
                    reason: "package/Object collision probe requires a Package owner".to_owned(),
                });
            }
            let package_path = package_path_of_symbol(store, package).ok_or_else(|| {
                ResolutionError::Malformed {
                    reason: "package prefix has an invalid owner chain".to_owned(),
                }
            })?;
            let simple_name = store.names.resolve(name.text());
            let internal = if package_path.is_empty() {
                simple_name.to_owned()
            } else {
                format!("{}/{simple_name}", package_path.join("/"))
            };
            let binary_name = BinaryName::from_internal(internal);
            if !binary_name.is_path_safe() {
                return Ok(None);
            }
            let resource = resolver
                .class_path
                .as_ref()
                .expect("resolver operation is exclusive")
                .find_class(&binary_name)
                .map_err(|error| ResolutionError::Malformed {
                    reason: format!("cannot inspect external module {binary_name}: {error}"),
                })?;
            if !resource
                .is_some_and(|resource| resource.format() == crate::class_path::ClassFormat::Tasty)
            {
                return Ok(None);
            }
            match resolver.ensure_module_identity(store, &binary_name) {
                Ok(identity) => {
                    resolver.validate_module_object(store, package, identity.object)?;
                    Ok(Some(true))
                }
                Err(ClassLoadError::IdentityConflict { reason, .. })
                    if reason
                        == "the canonical package name is occupied by an incompatible symbol" =>
                {
                    Err(ResolutionError::Ambiguous { candidates: 2 })
                }
                Err(ClassLoadError::IdentityConflict { reason, .. })
                    if reason
                        == "the TASTy file does not prove a top-level object and module class" =>
                {
                    Ok(None)
                }
                Err(error) => Err(ResolutionError::Malformed {
                    reason: format!("external module identity failed for {binary_name}: {error}"),
                }),
            }
        })
        .map(|collision| collision.unwrap_or(false))
    }

    fn module_class_of_object(
        &self,
        store: &SemanticStore,
        object: SymbolId,
    ) -> Result<Option<SymbolId>, ResolutionError> {
        if !store.symbols.contains(object) {
            return Err(ResolutionError::Malformed {
                reason: format!("module object {object:?} is missing from the semantic store"),
            });
        }
        if store.symbols.get(object).kind != SymbolKind::Object {
            return Ok(None);
        }
        let Some(package) = store.symbols.get(object).owner else {
            return Ok(None);
        };
        let Some(name) = self.session.modules_by_object.get(&object) else {
            return Ok(None);
        };
        let Some(identity) = self.session.modules.get(name) else {
            return Err(ResolutionError::Malformed {
                reason: format!("module object {object:?} has no indexed module identity"),
            });
        };
        self.validate_module_object(store, package, object)?;
        Ok(Some(identity.module_class))
    }

    fn complete_module_class(
        &mut self,
        store: &mut SemanticStore,
        module_class: SymbolId,
    ) -> Result<bool, ResolutionError> {
        self.transact(store, |resolver, store| {
            let Some(name) = resolver.session.modules_by_module_class.get(&module_class) else {
                return Ok(None);
            };
            let Some(identity) = resolver.session.modules.get(name).cloned() else {
                return Err(ResolutionError::Malformed {
                    reason: format!("module class {module_class:?} has no indexed module identity"),
                });
            };
            resolver.validate_module_object(
                store,
                store.symbols.get(identity.object).owner.ok_or_else(|| {
                    ResolutionError::Malformed {
                        reason: format!("module object {:?} has no owner", identity.object),
                    }
                })?,
                identity.object,
            )?;
            let completed = resolver
                .complete_module(store, &identity.object_name)
                .map_err(|error| ResolutionError::Malformed {
                    reason: format!("module completion failed: {error}"),
                })?;
            if completed.object != identity.object || completed.module_class != module_class {
                return Err(ResolutionError::Malformed {
                    reason: format!(
                        "module completion changed canonical identity for {module_class:?}"
                    ),
                });
            }
            Ok(Some(true))
        })
        .map(|completed| completed.unwrap_or(false))
    }

    fn complete_class(
        &mut self,
        store: &mut SemanticStore,
        symbol: SymbolId,
    ) -> Result<bool, ResolutionError> {
        self.transact(store, |resolver, store| {
            resolver.complete_class_inner(store, symbol)
        })
        .map(|completed| completed.unwrap_or(false))
    }

    fn checkpoint(&self) -> ResolverCheckpoint {
        ResolverCheckpoint::new(self.undo.len() as u64)
    }

    fn rollback_to(&mut self, store: &mut SemanticStore, checkpoint: ResolverCheckpoint) {
        let target = checkpoint.token() as usize;
        while self.undo.len() > target {
            let undo = self.undo.pop().expect("length checked");
            self.session.rollback_to(store, undo.session);
            *store.symbols.get_mut(self.definitions.object_class) = undo.object_class;
        }
    }

    fn resolve_member(
        &mut self,
        store: &mut SemanticStore,
        request: &MemberRequest,
    ) -> Result<Option<SymbolId>, ResolutionError> {
        let profile_started = self.collect_resolver_profile.then(Instant::now);
        if !self.collect_module_resolution_profile {
            let result = self.transact(store, |resolver, store| {
                resolver.resolve_member_inner(store, request)
            });
            self.finish_member_profile(profile_started, &result);
            return result;
        }
        let package_selection_started = self.collect_resolver_profile.then(Instant::now);
        let package_target = self.package_term_target_key(store, request);
        self.record_resolver_phase(
            ResolverProfilePhase::PackageTermSelection,
            package_selection_started,
        );
        let package_target_preexisting = package_target.as_ref().is_some_and(|target| {
            let Some((package, name)) = target.split_once("::") else {
                return false;
            };
            let internal = if package.is_empty() {
                name.to_owned()
            } else {
                format!("{}/{name}", package.replace('.', "/"))
            };
            self.session
                .modules
                .contains_key(&BinaryName::from_internal(internal))
        });
        let module_target = self.module_prefix_key(store, request);
        let module_info_before = module_target.as_ref().and_then(|(module_class, _)| {
            store
                .symbols
                .contains(*module_class)
                .then(|| store.symbols.get(*module_class).info)
        });
        if let Some(target) = &package_target {
            self.module_profile.package_term_requests += 1;
            self.module_profile
                .package_term_targets
                .insert(target.clone());
        }
        let member_target = module_target.as_ref().map(|(_, binary_name)| {
            let name = store.names.resolve(request.name.text());
            let namespace = match request.name.namespace() {
                Namespace::Term => "term",
                Namespace::Type => "type",
            };
            let space = match request.space {
                MemberSpace::Prefix => "Prefix",
                MemberSpace::Explicit(_) => "Explicit",
                _ => "Unsupported",
            };
            format!(
                "{binary_name}::namespace={namespace}::name={name}::space={space}::selector=Unique"
            )
        });
        if let (Some((_, binary_name)), Some(member_target)) = (&module_target, &member_target) {
            self.module_profile
                .module_targets
                .insert(binary_name.clone());
            self.module_profile
                .member_targets
                .insert(member_target.clone());
        }

        let result = self.transact(store, |resolver, store| {
            resolver.resolve_member_inner(store, request)
        });

        if package_target.is_some() {
            match &result {
                Ok(Some(symbol)) if store.symbols.get(*symbol).kind == SymbolKind::Object => {
                    self.module_profile.package_term_successes += 1;
                    if let Some(target) = &package_target {
                        self.module_profile
                            .resolved_object_targets
                            .insert(target.clone());
                    }
                    if package_target_preexisting {
                        self.module_profile.reused_object_identities += 1;
                    } else {
                        self.module_profile.new_object_identities += 1;
                    }
                }
                Ok(Some(_)) => self.module_profile.package_term_non_object_results += 1,
                Ok(None) => self.module_profile.package_term_unresolved += 1,
                Err(_) => self.module_profile.package_term_errors += 1,
            }
        }
        if let Some((module_class, binary_name)) = module_target
            && let Ok(Some(member)) = &result
        {
            let symbol = store.symbols.get(*member);
            if symbol.owner == Some(module_class)
                && matches!(
                    symbol.kind,
                    SymbolKind::Field | SymbolKind::Method | SymbolKind::TypeAlias
                )
            {
                self.module_profile.module_member_successes += 1;
                if let Some(target) = &member_target {
                    self.module_profile
                        .resolved_member_targets
                        .insert(target.clone());
                }
            }
            if matches!(
                module_info_before,
                Some(SymbolInfo::Missing | SymbolInfo::Deferred(_))
            ) && matches!(
                store.symbols.get(module_class).info,
                SymbolInfo::Complete(_)
            ) {
                self.module_profile
                    .completed_module_classes
                    .insert(binary_name);
            }
        }
        self.finish_member_profile(profile_started, &result);
        result
    }

    fn resolve_package(
        &mut self,
        store: &mut SemanticStore,
        path: &[&str],
    ) -> Result<Option<SymbolId>, ResolutionError> {
        self.transact(store, |resolver, store| {
            resolver.resolve_package_inner(store, path)
        })
    }

    fn enter_synthetic_package_member(
        &mut self,
        store: &mut SemanticStore,
        package: SymbolId,
        name: dotty_core::Name,
        member: SymbolId,
    ) -> Result<bool, ResolutionError> {
        self.transact(store, |resolver, store| {
            if resolver.session.packages.scope_of(package).is_none() {
                return Ok(None);
            }
            resolver
                .session
                .packages
                .enter_class(store, package, name, member);
            Ok(Some(true))
        })
        .map(|entered| entered.unwrap_or(false))
    }
}

enum PrefixTarget {
    Package(SymbolId),
    Class(SymbolId),
}

fn package_path_of_symbol(store: &SemanticStore, package: SymbolId) -> Option<Vec<String>> {
    let mut current = package;
    let mut segments = Vec::new();
    let mut visited = std::collections::HashSet::new();
    loop {
        if !visited.insert(current) {
            return None;
        }
        let item = store.symbols.get(current);
        if item.kind != SymbolKind::Package {
            return None;
        }
        let text = store.names.resolve(item.name.text());
        if !text.is_empty() {
            segments.push(text.to_owned());
        }
        let Some(owner) = item.owner else {
            break;
        };
        current = owner;
    }
    segments.reverse();
    Some(segments)
}

/// Nested JVM classes are also indexed in their binary package scope (for
/// example, `Outer$Inner` under `example`), while their semantic owner is the
/// enclosing class. Accept that shape only when the enclosing class itself is
/// owned by the requested package and its binary name prefixes the member.
fn is_nested_class_indexed_in_package(
    store: &SemanticStore,
    member: SymbolId,
    package: SymbolId,
) -> bool {
    const MAX_OWNER_DEPTH: usize = 64;

    let mut current = member;
    let mut visited = std::collections::HashSet::new();
    for depth in 0..MAX_OWNER_DEPTH {
        if !store.symbols.contains(current) || !visited.insert(current) {
            return false;
        }
        let symbol = store.symbols.get(current);
        let Some(owner) = symbol.owner else {
            return false;
        };
        if !store.symbols.contains(owner) {
            return false;
        }
        let enclosing = store.symbols.get(owner);
        if enclosing.kind == SymbolKind::Package {
            return depth > 0 && owner == package;
        }
        if !matches!(
            enclosing.kind,
            SymbolKind::Class | SymbolKind::Trait | SymbolKind::ModuleClass
        ) {
            return false;
        }

        let enclosing_name = store.names.resolve(enclosing.name.text());
        let member_name = store.names.resolve(symbol.name.text());
        if !member_name
            .strip_prefix(enclosing_name)
            .is_some_and(|suffix| suffix.starts_with('$') && suffix.len() > 1)
        {
            return false;
        }
        current = owner;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::class_path::{
        ClassFormat, ClassOrigin, ClassPathEntry, ClassPathError, ClassResource,
        CompositeClassPath, DirectoryClassPath,
    };
    use crate::jar_class_path::JarClassPath;
    use crate::jdk_class_path::JdkClassPath;
    use crate::jmod_class_path::JmodClassPath;
    use dotty_core::{
        ClassInfo, MemberSelector, Name, Namespace, Packages, Scope, SourceId, SymbolFlags,
        SymbolOrigin, Visibility,
    };
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    fn fixture_path(relative: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(relative)
    }

    #[derive(Clone)]
    struct ModuleTastyPath {
        entries: Arc<Mutex<std::collections::HashMap<BinaryName, Vec<u8>>>>,
        object_missing: Arc<AtomicBool>,
        object_corrupted: Arc<AtomicBool>,
    }

    impl ModuleTastyPath {
        fn both_fixture() -> Self {
            let bytes = std::fs::read(fixture_path(
                "../dotty-tasty-unpickler/tests/fixtures/semantic/Both.tasty",
            ))
            .expect("Both.tasty fixture should exist");
            Self {
                entries: Arc::new(Mutex::new(std::collections::HashMap::from([(
                    BinaryName::from_internal("me/cytrowski/tastyfixtures/semantic/Both"),
                    bytes,
                )]))),
                object_missing: Arc::new(AtomicBool::new(false)),
                object_corrupted: Arc::new(AtomicBool::new(false)),
            }
        }
    }

    impl ClassPathEntry for ModuleTastyPath {
        fn find_class(&self, name: &BinaryName) -> Result<Option<ClassResource>, ClassPathError> {
            if name.as_internal() == "java/lang/Object" {
                if self.object_corrupted.load(Ordering::SeqCst) {
                    return Ok(Some(ClassResource::new(
                        vec![0xCA, 0xFE, 0xBA, 0xBE],
                        ClassFormat::Class,
                        ClassOrigin::Directory(PathBuf::from("<module-dependency>")),
                    )));
                }
                return Ok((!self.object_missing.load(Ordering::SeqCst)).then(|| {
                    ClassResource::new(
                        test_class_stub("java/lang/Object", 0x0021),
                        ClassFormat::Class,
                        ClassOrigin::Directory(PathBuf::from("<module-dependency>")),
                    )
                }));
            }
            Ok(self.entries.lock().unwrap().get(name).map(|bytes| {
                ClassResource::new(
                    bytes.clone(),
                    ClassFormat::Tasty,
                    ClassOrigin::Directory(PathBuf::from("<module-fixture>")),
                )
            }))
        }

        fn contains_package(&self, package: &[&str]) -> Result<bool, ClassPathError> {
            Ok(self
                .entries
                .lock()
                .unwrap()
                .keys()
                .any(|name| name.package_path().split('/').collect::<Vec<_>>() == package))
        }

        fn contains_package_prefix(&self, package: &[&str]) -> Result<bool, ClassPathError> {
            Ok(self.entries.lock().unwrap().keys().any(|name| {
                let resource_package = name.package_path().split('/').collect::<Vec<_>>();
                resource_package.starts_with(package)
            }))
        }
    }

    struct FixtureClassPath(JdkClassPath);

    impl ClassPathEntry for FixtureClassPath {
        fn find_class(&self, name: &BinaryName) -> Result<Option<ClassResource>, ClassPathError> {
            if name.as_internal() == "java/lang/Object" {
                return Ok(Some(ClassResource::new(
                    test_class_stub("java/lang/Object", 0x0021),
                    ClassFormat::Class,
                    ClassOrigin::Directory(PathBuf::from("<test-stub>")),
                )));
            }
            if name.as_internal() == "java/lang/Runnable" {
                return Ok(Some(ClassResource::new(
                    test_class_stub("java/lang/Runnable", 0x0601),
                    ClassFormat::Class,
                    ClassOrigin::Directory(PathBuf::from("<test-stub>")),
                )));
            }
            self.0.find_class(name)
        }

        fn contains_package(&self, package: &[&str]) -> Result<bool, ClassPathError> {
            if package == ["java", "lang"] {
                return Ok(true);
            }
            self.0.contains_package(package)
        }

        fn contains_package_prefix(&self, package: &[&str]) -> Result<bool, ClassPathError> {
            if package == ["java", "lang"] {
                return Ok(true);
            }
            self.0.contains_package_prefix(package)
        }
    }

    #[derive(Clone, Copy)]
    struct DescendantOnlyClassPath;

    impl ClassPathEntry for DescendantOnlyClassPath {
        fn find_class(&self, _name: &BinaryName) -> Result<Option<ClassResource>, ClassPathError> {
            Ok(None)
        }

        fn contains_package(&self, _package: &[&str]) -> Result<bool, ClassPathError> {
            Ok(false)
        }

        fn contains_package_prefix(&self, package: &[&str]) -> Result<bool, ClassPathError> {
            Ok(matches!(package, ["a"] | ["a", "b"] | ["a", "b", "c"]))
        }
    }

    struct EmptyCountingClassPath(Arc<AtomicUsize>);
    #[derive(Clone)]
    struct CompletionClassPath {
        classes: Arc<Mutex<std::collections::HashMap<BinaryName, Vec<u8>>>>,
        calls: Arc<AtomicUsize>,
    }

    fn member_class_path() -> CompletionClassPath {
        use dotty_classfile::{
            access_flags::{ClassAccessFlags, FieldAccessFlags, MethodAccessFlags},
            attribute::Attribute,
            class_file::{ClassFile, ClassFileVersion},
            constant_pool_builder::ConstantPoolBuilder,
            field::FieldInfo,
            method::MethodInfo,
        };
        let mut pool = ConstantPoolBuilder::new();
        pool.utf8("Signature").unwrap();
        let this_class = pool.class("sample/B").unwrap();
        let parent = pool.class("java/lang/Object").unwrap();
        let signature = pool
            .utf8("<T:Ljava/lang/Object;>Ljava/lang/Object;")
            .unwrap();
        let fields = vec![FieldInfo {
            access_flags: FieldAccessFlags(1),
            name_index: pool.utf8("value").unwrap(),
            descriptor_index: pool.utf8("I").unwrap(),
            attributes: Vec::new(),
        }];
        let methods = [
            ("get", "()Ljava/lang/String;", None),
            ("generic", "()Ljava/lang/Object;", Some("()TT;")),
            ("over", "()I", None),
            ("over", "(I)I", None),
        ]
        .into_iter()
        .map(|(name, descriptor, signature)| MethodInfo {
            access_flags: MethodAccessFlags(0x0401),
            name_index: pool.utf8(name).unwrap(),
            descriptor_index: pool.utf8(descriptor).unwrap(),
            attributes: signature
                .into_iter()
                .map(|sig| Attribute::Signature(pool.utf8(sig).unwrap()))
                .collect(),
        })
        .collect();
        let bytes = ClassFile {
            version: ClassFileVersion {
                major: 65,
                minor: 0,
            },
            constant_pool: pool.finish().unwrap(),
            access_flags: ClassAccessFlags(0x0421),
            this_class,
            super_class: Some(parent),
            interfaces: Vec::new(),
            fields,
            methods,
            attributes: vec![Attribute::Signature(signature)],
        }
        .encode()
        .unwrap();
        let path = CompletionClassPath::new();
        path.classes.lock().unwrap().extend([
            (BinaryName::from_internal("sample/B"), bytes),
            (
                BinaryName::from_internal("java/lang/Object"),
                test_class_stub("java/lang/Object", 0x0021),
            ),
            (
                BinaryName::from_internal("java/lang/String"),
                test_class_stub("java/lang/String", 0x0021),
            ),
        ]);
        path
    }

    #[test]
    fn resolver_phase_profile_is_opt_in_and_tracks_member_resolution() {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let path = member_class_path();
        let mut loader = ClassLoader::with_definitions(
            path.clone(),
            &mut store,
            definitions,
            LoadingSession::new(),
        );
        let class = loader
            .ensure_class_identity(&BinaryName::from_internal("sample/B"))
            .unwrap();
        let session = loader.into_session();
        let prefix = store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, class));
        let value = Name::new(store.names.intern("value"), Namespace::Term);
        let request = MemberRequest {
            prefix,
            name: value,
            selector: MemberSelector::Unique,
            space: MemberSpace::Prefix,
        };

        let mut profiled = ClasspathSymbolResolver::new(path.clone(), definitions, session)
            .with_resolver_profile_collection();
        let profiled_symbol = profiled
            .resolve_member(&mut store, &request)
            .unwrap()
            .unwrap();

        let profile = profiled.take_resolver_profile();
        assert_eq!(profile.member_lookups, 1);
        assert_eq!(profile.member_successes, 1);
        assert_eq!(profile.member_misses, 0);
        assert_eq!(profile.member_errors, 0);
        assert_eq!(profile.member_lookup_inclusive.calls, 1);
        assert_eq!(profile.prefix_classification.calls, 1);
        assert_eq!(profile.target_resolution.calls, 1);
        assert_eq!(profile.declaration_selection.calls, 1);
        assert_eq!(profile.class_loading_inclusive.calls, 1);
        assert_eq!(profile.transaction_bookkeeping.calls, 1);
        assert!(profile.transaction_bookkeeping.elapsed_ns > 0);
        assert!(
            profile.transaction_bookkeeping.exclusive_ns
                <= profile.transaction_bookkeeping.elapsed_ns
        );
        assert!(profile.member_lookup_inclusive.elapsed_ns > 0);
        assert_eq!(
            profiled.take_resolver_profile(),
            ClasspathResolverProfile::default()
        );

        let missing = Name::new(store.names.intern("missing"), Namespace::Term);
        assert!(
            profiled
                .resolve_member(
                    &mut store,
                    &MemberRequest {
                        prefix,
                        name: missing,
                        selector: MemberSelector::Unique,
                        space: MemberSpace::Prefix,
                    },
                )
                .unwrap()
                .is_none()
        );
        let miss_profile = profiled.take_resolver_profile();
        assert_eq!(miss_profile.member_lookups, 1);
        assert_eq!(miss_profile.member_misses, 1);
        assert_eq!(miss_profile.rollbacks, 1);
        assert_eq!(miss_profile.rollback.calls, 1);
        assert!(miss_profile.rollback.elapsed_ns > 0);

        let mut unprofiled =
            ClasspathSymbolResolver::new(path, definitions, profiled.into_session());
        let unprofiled_symbol = unprofiled
            .resolve_member(&mut store, &request)
            .unwrap()
            .unwrap();
        assert_eq!(unprofiled_symbol, profiled_symbol);
        assert_eq!(
            unprofiled.take_resolver_profile(),
            ClasspathResolverProfile::default()
        );
    }

    #[test]
    fn direct_shell_members_support_all_prefix_views_and_reuse_declarations() {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let path = member_class_path();
        let mut loader = ClassLoader::with_definitions(
            path.clone(),
            &mut store,
            definitions,
            LoadingSession::new(),
        );
        let b = loader
            .ensure_class_identity(&BinaryName::from_internal("sample/B"))
            .unwrap();
        let a = loader
            .ensure_class_identity(&BinaryName::from_internal("sample/A"))
            .unwrap();
        let session = loader.into_session();
        let b_ref = store.types.alloc(Type::type_ref(definitions.no_prefix, b));
        let a_ref = store.types.alloc(Type::type_ref(definitions.no_prefix, a));
        let applied = store.types.alloc(Type::Applied {
            tycon: b_ref,
            args: vec![definitions.int],
        });
        let this = store.types.alloc(Type::ThisType { class: b });
        let value = Name::new(store.names.intern("value"), Namespace::Term);
        let get = Name::new(store.names.intern("get"), Namespace::Term);
        let mut resolver = ClasspathSymbolResolver::new(path.clone(), definitions, session);
        let mut field = None;
        let mut method = None;
        for (prefix, space) in [
            (b_ref, MemberSpace::Prefix),
            (applied, MemberSpace::Prefix),
            (this, MemberSpace::Prefix),
            (a_ref, MemberSpace::Explicit(b_ref)),
        ] {
            for _ in 0..2 {
                let f = resolver
                    .resolve_member(
                        &mut store,
                        &MemberRequest {
                            prefix,
                            name: value,
                            selector: MemberSelector::Unique,
                            space: space.clone(),
                        },
                    )
                    .unwrap()
                    .unwrap();
                let m = resolver
                    .resolve_member(
                        &mut store,
                        &MemberRequest {
                            prefix,
                            name: get,
                            selector: MemberSelector::Unique,
                            space: space.clone(),
                        },
                    )
                    .unwrap()
                    .unwrap();
                assert_eq!(*field.get_or_insert(f), f);
                assert_eq!(*method.get_or_insert(m), m);
                assert_eq!(store.symbols.get(f).owner, Some(b));
                assert_eq!(store.symbols.get(f).kind, SymbolKind::Field);
                assert_eq!(
                    store.symbols.get(f).info,
                    SymbolInfo::Complete(definitions.int)
                );
                assert_eq!(store.symbols.get(m).owner, Some(b));
                assert_eq!(store.symbols.get(m).kind, SymbolKind::Method);
                assert_eq!(path.calls.load(Ordering::Relaxed), 5);
                assert_eq!(store.symbols.get(a).info, SymbolInfo::Missing);
            }
        }
        let SymbolInfo::Complete(info) = store.symbols.get(method.unwrap()).info else {
            panic!("method info")
        };
        let Type::Method(info) = store.types.get(info) else {
            panic!("Method")
        };
        let string = store.types.get(info.result).reference_symbol().unwrap();
        assert_eq!(
            store.names.resolve(store.symbols.get(string).name.text()),
            "String"
        );
        assert_eq!(store.symbols.get(string).info, SymbolInfo::Missing);
        let t_name = Name::new(store.names.intern("T"), Namespace::Type);
        let t = resolver
            .resolve_member(
                &mut store,
                &MemberRequest {
                    prefix: b_ref,
                    name: t_name,
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                },
            )
            .unwrap()
            .unwrap();
        assert_eq!(store.symbols.get(t).kind, SymbolKind::TypeParameter);
        let generic_name = Name::new(store.names.intern("generic"), Namespace::Term);
        let generic = resolver
            .resolve_member(
                &mut store,
                &MemberRequest {
                    prefix: applied,
                    name: generic_name,
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                },
            )
            .unwrap()
            .unwrap();
        let SymbolInfo::Complete(info) = store.symbols.get(generic).info else {
            panic!("generic info")
        };
        let Type::Method(info) = store.types.get(info) else {
            panic!("Method")
        };
        assert_eq!(
            store.types.get(info.result).reference_symbol(),
            Some(t),
            "resolver does not substitute applied receiver arguments"
        );
        let mark = store.checkpoint();
        assert_eq!(
            resolver
                .resolve_member(
                    &mut store,
                    &MemberRequest {
                        prefix: b_ref,
                        name: value,
                        selector: MemberSelector::Unique,
                        space: MemberSpace::Explicit(a_ref)
                    }
                )
                .unwrap(),
            None
        );
        assert_eq!(store.symbols.get(a).info, SymbolInfo::Missing);
        assert_eq!(store.checkpoint(), mark);
    }

    #[test]
    fn package_term_request_materializes_the_canonical_object_idempotently() {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let mut resolver = ClasspathSymbolResolver::new(
            ModuleTastyPath::both_fixture(),
            definitions,
            LoadingSession::new(),
        )
        .with_module_resolution_profile_collection();
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
            name: Name::new(store.names.intern("Both"), Namespace::Term),
            selector: MemberSelector::Unique,
            space: MemberSpace::Prefix,
        };

        let object = resolver
            .resolve_member(&mut store, &request)
            .unwrap()
            .unwrap();
        assert_eq!(store.symbols.get(object).kind, SymbolKind::Object);
        assert_eq!(store.symbols.get(object).owner, Some(package));
        let identity = resolver
            .session
            .modules
            .get(&BinaryName::from_internal(
                "me/cytrowski/tastyfixtures/semantic/Both",
            ))
            .expect("TASTy-proven module identity should be cached");
        assert_eq!(identity.object, object);
        assert_ne!(identity.object, identity.module_class);
        assert_eq!(
            store.symbols.get(identity.module_class).kind,
            SymbolKind::ModuleClass
        );
        assert_eq!(
            resolver.module_class_of_object(&store, object),
            Ok(Some(identity.module_class)),
            "only the cached TASTy-proven identity is exposed"
        );

        let checkpoint = store.checkpoint();
        assert_eq!(
            resolver.resolve_member(&mut store, &request),
            Ok(Some(object))
        );
        assert_eq!(store.checkpoint(), checkpoint);
        let class_name = Name::new(store.names.intern("Both"), Namespace::Type);
        let class = resolver
            .resolve_member(
                &mut store,
                &MemberRequest {
                    prefix,
                    name: class_name,
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                },
            )
            .unwrap()
            .unwrap();
        assert_ne!(class, object);
        assert_eq!(store.symbols.get(class).kind, SymbolKind::Class);
        let profile = resolver.take_module_resolution_profile();
        assert_eq!(profile.package_term_requests, 2);
        assert_eq!(profile.package_term_successes, 2);
        assert_eq!(profile.new_object_identities, 1);
        assert_eq!(profile.reused_object_identities, 1);
        assert_eq!(profile.package_term_targets.len(), 1);
        assert_eq!(
            profile.resolved_object_targets,
            std::collections::BTreeSet::from([
                "me.cytrowski.tastyfixtures.semantic::Both".to_owned()
            ])
        );
    }

    #[test]
    fn resolver_completes_a_validated_module_class_in_place() {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let mut resolver = ClasspathSymbolResolver::new(
            ModuleTastyPath::both_fixture(),
            definitions,
            LoadingSession::new(),
        );
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
        let object_name = Name::new(store.names.intern("Both"), Namespace::Term);
        let object = resolver
            .resolve_member(
                &mut store,
                &MemberRequest {
                    prefix,
                    name: object_name,
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                },
            )
            .unwrap()
            .unwrap();
        let module_class = resolver
            .module_class_of_object(&store, object)
            .unwrap()
            .unwrap();
        assert_eq!(store.symbols.get(module_class).info, SymbolInfo::Missing);

        assert_eq!(
            resolver.complete_module_class(&mut store, module_class),
            Ok(true)
        );
        let SymbolInfo::Complete(info) = store.symbols.get(module_class).info else {
            panic!("module class should complete in place")
        };
        let Type::ClassInfo(info) = store.types.get(info) else {
            panic!("completed module class should have ClassInfo")
        };
        assert_eq!(
            store.scopes.get(info.declarations).owner,
            Some(module_class)
        );
        assert!(matches!(
            store.symbols.get(object).info,
            SymbolInfo::Complete(ty)
                if matches!(store.types.get(ty), Type::TypeRef { target: TypeRefTarget::Symbol(target), .. } if *target == module_class)
        ));
        let checkpoint = store.checkpoint();
        assert_eq!(
            resolver.complete_module_class(&mut store, module_class),
            Ok(true)
        );
        assert_eq!(store.checkpoint(), checkpoint);
    }

    #[test]
    fn caller_rollback_removes_a_successful_package_module_materialization() {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let mut resolver = ClasspathSymbolResolver::new(
            ModuleTastyPath::both_fixture(),
            definitions,
            LoadingSession::new(),
        );
        let package = resolver
            .resolve_package(
                &mut store,
                &["me", "cytrowski", "tastyfixtures", "semantic"],
            )
            .unwrap()
            .unwrap();
        let package_ref = store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, package));
        let both_name = Name::new(store.names.intern("Both"), Namespace::Term);
        let resolver_mark = resolver.checkpoint();
        let store_mark = store.checkpoint();

        let object = resolver
            .resolve_member(
                &mut store,
                &MemberRequest {
                    prefix: package_ref,
                    name: both_name,
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                },
            )
            .unwrap()
            .unwrap();
        let module_class = resolver.session.modules
            [&BinaryName::from_internal("me/cytrowski/tastyfixtures/semantic/Both")]
            .module_class;
        assert!(store.symbols.contains(object));
        assert!(store.symbols.contains(module_class));

        resolver.rollback_to(&mut store, resolver_mark);
        store.rollback_to(store_mark);
        assert_eq!(store.checkpoint(), store_mark);
        assert!(!store.symbols.contains(object));
        assert!(!store.symbols.contains(module_class));
        assert!(resolver.session.modules.is_empty());
        let resolved_again = resolver
            .resolve_member(
                &mut store,
                &MemberRequest {
                    prefix: package_ref,
                    name: both_name,
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                },
            )
            .unwrap()
            .unwrap();
        assert_eq!(resolved_again, object);
    }

    #[test]
    fn package_term_resolves_a_second_real_standalone_module_fixture() {
        let binary = BinaryName::from_internal("scala/jdk/CollectionConverters");
        let bytes = std::fs::read(fixture_path(
            "../dotty-tasty/tests/fixtures/scala3-library/scala/jdk/CollectionConverters.tasty",
        ))
        .expect("CollectionConverters.tasty fixture should exist");
        let path = ModuleTastyPath {
            entries: Arc::new(Mutex::new(std::collections::HashMap::from([(
                binary.clone(),
                bytes,
            )]))),
            object_missing: Arc::new(AtomicBool::new(false)),
            object_corrupted: Arc::new(AtomicBool::new(false)),
        };
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let mut resolver = ClasspathSymbolResolver::new(path, definitions, LoadingSession::new())
            .with_module_resolution_profile_collection();
        let package = resolver
            .resolve_package(&mut store, &["scala", "jdk"])
            .unwrap()
            .unwrap();
        let package_ref = store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, package));
        let name = Name::new(store.names.intern("CollectionConverters"), Namespace::Term);

        let object = resolver
            .resolve_member(
                &mut store,
                &MemberRequest {
                    prefix: package_ref,
                    name,
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                },
            )
            .unwrap()
            .unwrap();
        let identity = &resolver.session.modules[&binary];
        assert_eq!(identity.object, object);
        assert_eq!(identity.companion_class, None);
        assert_eq!(store.symbols.get(object).kind, SymbolKind::Object);
        assert_eq!(
            store.symbols.get(identity.module_class).kind,
            SymbolKind::ModuleClass
        );
    }

    #[test]
    fn scala_default_package_term_resolves_through_its_tasty_alias() {
        let library_root = fixture_path("../dotty-tasty/tests/fixtures/scala3-library");
        let class_path = DirectoryClassPath::new(library_root);
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let mut resolver =
            ClasspathSymbolResolver::new(class_path, definitions, LoadingSession::new());
        let package = resolver
            .resolve_package(&mut store, &["scala"])
            .unwrap()
            .unwrap();
        let prefix = store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, package));
        let name = Name::new(store.names.intern("Nil"), Namespace::Term);

        let nil = resolver
            .resolve_member(
                &mut store,
                &MemberRequest {
                    prefix,
                    name,
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                },
            )
            .unwrap()
            .expect("scala.Nil should resolve through scala/package.tasty");
        let identity = resolver
            .session
            .modules
            .get(&BinaryName::from_internal("scala/collection/immutable/Nil"))
            .expect("the alias target should have canonical module identity");
        assert_eq!(identity.object, nil);
        assert_eq!(store.symbols.get(nil).kind, SymbolKind::Object);
        assert_eq!(
            store.names.resolve(store.symbols.get(nil).name.text()),
            "Nil"
        );
        let store_checkpoint = store.checkpoint();
        let resolver_checkpoint = resolver.checkpoint();
        let missing_name = Name::new(store.names.intern("NotAStandardTerm"), Namespace::Term);
        assert_eq!(
            resolver
                .resolve_member(
                    &mut store,
                    &MemberRequest {
                        prefix,
                        name: missing_name,
                        selector: MemberSelector::Unique,
                        space: MemberSpace::Prefix,
                    },
                )
                .unwrap(),
            None
        );
        assert_eq!(store.checkpoint(), store_checkpoint);
        assert_eq!(resolver.checkpoint(), resolver_checkpoint);
    }

    #[test]
    fn real_scala39_compiler_objects_have_canonical_module_identities() {
        let targets = [
            "dotty/tools/dotc/core/Types",
            "dotty/tools/dotc/core/Contexts",
            "dotty/tools/dotc/core/Symbols",
            "dotty/tools/dotc/parsing/Tokens",
            "dotty/tools/dotc/ast/tpd",
        ];
        let compiler_root = fixture_path("../dotty-tasty/tests/fixtures/scala3-compiler");
        let library_root = fixture_path("../dotty-tasty/tests/fixtures/scala3-library");

        for binary_name in targets {
            let binary = BinaryName::from_internal(binary_name);
            let package_path = binary.package_path().split('/').collect::<Vec<_>>();
            let mut store = SemanticStore::new();
            let definitions = Definitions::bootstrap(&mut store);
            let class_path = CompositeClassPath::new(vec![
                Box::new(DirectoryClassPath::new(compiler_root.clone())),
                Box::new(DirectoryClassPath::new(library_root.clone())),
                Box::new(ModuleTastyPath {
                    entries: Arc::new(Mutex::new(std::collections::HashMap::new())),
                    object_missing: Arc::new(AtomicBool::new(false)),
                    object_corrupted: Arc::new(AtomicBool::new(false)),
                }),
            ]);
            let resource = class_path
                .find_class(&binary)
                .unwrap()
                .expect("the pinned compiler resource should exist");
            assert_eq!(resource.format(), ClassFormat::Tasty, "{binary}");
            assert_eq!(
                resource.origin(),
                &ClassOrigin::Directory(compiler_root.clone()),
                "{binary}"
            );
            let mut resolver =
                ClasspathSymbolResolver::new(class_path, definitions, LoadingSession::new())
                    .with_module_resolution_profile_collection();
            let package = resolver
                .resolve_package(&mut store, &package_path)
                .unwrap()
                .expect("the real compiler package should be present");
            let prefix = store
                .types
                .alloc(Type::type_ref(definitions.no_prefix, package));
            let simple_name = binary.simple_name();
            let object_name = Name::new(store.names.intern(simple_name), Namespace::Term);
            let object = resolver
                .resolve_member(
                    &mut store,
                    &MemberRequest {
                        prefix,
                        name: object_name,
                        selector: MemberSelector::Unique,
                        space: MemberSpace::Prefix,
                    },
                )
                .unwrap()
                .unwrap_or_else(|| panic!("{binary} should prove a top-level Object"));
            assert_eq!(
                resolver.resolve_member(
                    &mut store,
                    &MemberRequest {
                        prefix,
                        name: object_name,
                        selector: MemberSelector::Unique,
                        space: MemberSpace::Prefix,
                    },
                ),
                Ok(Some(object)),
                "repeated Object lookup must reuse its canonical identity"
            );
            let module_class = resolver
                .module_class_of_object(&store, object)
                .unwrap()
                .unwrap_or_else(|| panic!("{binary} should prove a ModuleClass"));

            assert_eq!(
                store.symbols.get(object).kind,
                SymbolKind::Object,
                "{binary}"
            );
            assert_eq!(
                store.symbols.get(module_class).kind,
                SymbolKind::ModuleClass,
                "{binary}"
            );
            assert_eq!(store.symbols.get(object).owner, Some(package), "{binary}");
            assert_eq!(
                store.symbols.get(module_class).owner,
                Some(package),
                "{binary}"
            );
            assert_eq!(
                store.symbols.get(module_class).info,
                SymbolInfo::Missing,
                "identity lookup must stay lazy for {binary}"
            );

            let package_scope = resolver.package_scope(package).unwrap();
            let object_name = Name::new(store.names.intern(binary.simple_name()), Namespace::Term);
            let module_name = Name::new(
                store.names.intern(&format!("{}$", binary.simple_name())),
                Namespace::Type,
            );
            assert_eq!(
                store.scopes.get(package_scope).lookup_all(&object_name),
                [object]
            );
            assert_eq!(
                store.scopes.get(package_scope).lookup_all(&module_name),
                [module_class]
            );
            assert!(matches!(
                store.symbols.get(object).origin,
                dotty_core::SymbolOrigin::Tasty(_)
            ));
            assert!(matches!(
                store.symbols.get(module_class).origin,
                dotty_core::SymbolOrigin::Tasty(_)
            ));

            let identity = resolver.session.modules.get(&binary).unwrap();
            assert_eq!(identity.object, object);
            assert_eq!(identity.module_class, module_class);
            if let Some(companion_class) = identity.companion_class {
                assert_ne!(companion_class, object);
                assert_ne!(companion_class, module_class);
                assert_eq!(store.symbols.get(companion_class).kind, SymbolKind::Class);
                assert_eq!(
                    store.symbols.get(companion_class).links.companion,
                    Some(object)
                );
                assert_eq!(
                    store.symbols.get(object).links.companion,
                    Some(companion_class)
                );
                assert_eq!(store.symbols.get(module_class).links.companion, None);
            }

            // Symbols$ is in #1412's frozen unresolved-parent cohort. Resolve
            // it through its Object term prefix so this gate proves a real
            // post-parent-repair ModuleClass and member, not only an identity.
            if matches!(binary.simple_name(), "Types" | "Symbols" | "Tokens") {
                let symbols_member = if binary.simple_name() == "Symbols" {
                    let object_ref = store.types.alloc(Type::TermRef {
                        prefix,
                        target: dotty_core::TermRefTarget::Symbol(object),
                    });
                    let member_name = Name::new(store.names.intern("NoSymbol"), Namespace::Term);
                    let member = resolver
                        .resolve_member(
                            &mut store,
                            &MemberRequest {
                                prefix: object_ref,
                                name: member_name,
                                selector: MemberSelector::Unique,
                                space: MemberSpace::Prefix,
                            },
                        )
                        .unwrap()
                        .expect("Symbols declares the real NoSymbol member");
                    Some((object_ref, member_name, member))
                } else {
                    assert_eq!(
                        resolver.complete_module_class(&mut store, module_class),
                        Ok(true),
                        "the authoritative compiler parent should permit completion for {binary}"
                    );
                    None
                };
                let SymbolInfo::Complete(info) = store.symbols.get(module_class).info else {
                    panic!("{binary} ModuleClass should be complete after parent resolution");
                };
                let Type::ClassInfo(info) = store.types.get(info) else {
                    panic!("{binary} should complete to ClassInfo");
                };
                assert_eq!(info.class, module_class);
                if let Some((object_ref, member_name, member)) = symbols_member {
                    assert_eq!(info.parents.len(), 1);
                    let parent = store.types.get(info.parents[0]).reference_symbol().unwrap();
                    assert_eq!(
                        store.names.resolve(store.symbols.get(parent).name.text()),
                        "SymUtils"
                    );
                    assert_eq!(store.symbols.get(parent).owner, Some(package));
                    assert!(matches!(
                        store.symbols.get(member).kind,
                        SymbolKind::Field | SymbolKind::Method
                    ));
                    assert_eq!(store.symbols.get(member).owner, Some(module_class));
                    assert!(matches!(
                        store.symbols.get(member).origin,
                        dotty_core::SymbolOrigin::Tasty(_)
                    ));
                    assert_eq!(
                        resolver.resolve_member(
                            &mut store,
                            &MemberRequest {
                                prefix: object_ref,
                                name: member_name,
                                selector: MemberSelector::Unique,
                                space: MemberSpace::Prefix,
                            },
                        ),
                        Ok(Some(member)),
                        "repeat Object-member lookup must reuse the declaration"
                    );
                    let profile = resolver.take_module_resolution_profile();
                    assert_eq!(profile.module_member_successes, 2);
                    assert!(
                        profile
                            .completed_module_classes
                            .contains("dotty/tools/dotc/core/Symbols")
                    );
                }
                continue;
            }

            if binary.simple_name() == "tpd" {
                let store_mark = store.checkpoint();
                let session_mark = resolver.session.checkpoint();
                let error = resolver
                    .complete_module_class(&mut store, module_class)
                    .expect_err(
                        "tpd's nested Trees.Instance resource is not independently present",
                    );
                let reason = error.to_string();
                assert!(
                    reason.contains("class not found: dotty/tools/dotc/ast/Trees$Instance"),
                    "{reason}"
                );
                assert_eq!(store.checkpoint(), store_mark);
                assert_eq!(resolver.session.checkpoint(), session_mark);
                assert_eq!(store.symbols.get(object).info, SymbolInfo::Missing);
                assert_eq!(store.symbols.get(module_class).info, SymbolInfo::Missing);
                let retry = resolver
                    .complete_module_class(&mut store, module_class)
                    .expect_err("retry should preserve the same typed residual");
                assert_eq!(retry.to_string(), reason);
                assert_eq!(store.checkpoint(), store_mark);
                assert_eq!(resolver.session.checkpoint(), session_mark);
                continue;
            }

            if binary.simple_name() == "Contexts" {
                let object_ref = store.types.alloc(Type::TermRef {
                    prefix,
                    target: dotty_core::TermRefTarget::Symbol(object),
                });
                let missing_name =
                    Name::new(store.names.intern("notAContextsMember"), Namespace::Term);
                let store_mark = store.checkpoint();
                let session_mark = resolver.session.checkpoint();
                assert_eq!(
                    resolver.resolve_member(
                        &mut store,
                        &MemberRequest {
                            prefix: object_ref,
                            name: missing_name,
                            selector: MemberSelector::Unique,
                            space: MemberSpace::Prefix,
                        },
                    ),
                    Ok(None),
                    "a missing member on a proven Object must remain absent"
                );
                assert_eq!(store.checkpoint(), store_mark);
                assert_eq!(resolver.session.checkpoint(), session_mark);
                assert_eq!(store.symbols.get(module_class).info, SymbolInfo::Missing);

                let member_name =
                    Name::new(store.names.intern("withFreshTyperState"), Namespace::Term);
                let member = resolver
                    .resolve_member(
                        &mut store,
                        &MemberRequest {
                            prefix: object_ref,
                            name: member_name,
                            selector: MemberSelector::Unique,
                            space: MemberSpace::Prefix,
                        },
                    )
                    .unwrap()
                    .expect("Contexts declares withFreshTyperState");
                assert_eq!(store.symbols.get(member).kind, SymbolKind::Method);
                assert_eq!(store.symbols.get(member).owner, Some(module_class));
                assert!(matches!(
                    store.symbols.get(member).origin,
                    dotty_core::SymbolOrigin::Tasty(_)
                ));
                let SymbolInfo::Complete(module_info) = store.symbols.get(module_class).info else {
                    panic!("Contexts ModuleClass should complete in place");
                };
                let Type::ClassInfo(module_info) = store.types.get(module_info) else {
                    panic!("Contexts should complete to ClassInfo");
                };
                assert_eq!(module_info.class, module_class);
                assert_eq!(
                    store.scopes.get(module_info.declarations).owner,
                    Some(module_class)
                );
                assert_eq!(
                    store
                        .scopes
                        .get(module_info.declarations)
                        .lookup_all(&member_name),
                    [member]
                );
                let after_first_lookup = store.checkpoint();
                assert_eq!(
                    resolver.complete_module_class(&mut store, module_class),
                    Ok(true)
                );
                assert_eq!(store.checkpoint(), after_first_lookup);
                assert_eq!(
                    resolver.resolve_member(
                        &mut store,
                        &MemberRequest {
                            prefix: object_ref,
                            name: member_name,
                            selector: MemberSelector::Unique,
                            space: MemberSpace::Prefix,
                        },
                    ),
                    Ok(Some(member)),
                    "repeat member lookup must reuse the declaration identity"
                );
                let profile = resolver.take_module_resolution_profile();
                assert_eq!(profile.package_term_requests, 2);
                assert_eq!(profile.package_term_successes, 2);
                assert_eq!(profile.new_object_identities, 1);
                assert_eq!(profile.reused_object_identities, 1);
                assert_eq!(profile.module_member_successes, 2);
                assert_eq!(
                    profile.completed_module_classes,
                    std::collections::BTreeSet::from([binary.as_internal().to_owned()])
                );

                let class_path = resolver.class_path.take().unwrap();
                let session = std::mem::replace(&mut resolver.session, LoadingSession::new());
                let mut next_resolver =
                    ClasspathSymbolResolver::new(class_path, definitions, session);
                assert_eq!(
                    next_resolver.module_class_of_object(&store, object),
                    Ok(Some(module_class)),
                    "a later resolver over the same store/session must retain the pair"
                );
                assert_eq!(
                    next_resolver.resolve_member(
                        &mut store,
                        &MemberRequest {
                            prefix: object_ref,
                            name: member_name,
                            selector: MemberSelector::Unique,
                            space: MemberSpace::Prefix,
                        },
                    ),
                    Ok(Some(member))
                );
            } else {
                let store_mark = store.checkpoint();
                let session_mark = resolver.session.checkpoint();
                let error = resolver
                    .complete_module_class(&mut store, module_class)
                    .expect_err("unsupported parent shapes must remain explicit errors");
                let reason = error.to_string();
                let expected_parent = match binary.simple_name() {
                    "Types" => "TypeUtils",
                    "Symbols" => "SymUtils",
                    "Tokens" => "TokensCommon",
                    other => panic!("unexpected target {other}"),
                };
                assert!(
                    reason.contains("unresolved supertype reference"),
                    "{reason}"
                );
                assert!(reason.contains(expected_parent), "{reason}");
                assert_eq!(
                    store.checkpoint(),
                    store_mark,
                    "failed {binary} must roll back"
                );
                assert_eq!(
                    resolver.session.checkpoint(),
                    session_mark,
                    "failed {binary} must roll back session state"
                );
                assert_eq!(store.symbols.get(object).info, SymbolInfo::Missing);
                assert_eq!(store.symbols.get(module_class).info, SymbolInfo::Missing);
                let retry = resolver
                    .complete_module_class(&mut store, module_class)
                    .expect_err("retry must report the same unresolved parent");
                assert_eq!(retry.to_string(), reason);
                assert_eq!(store.checkpoint(), store_mark);
            }
        }
    }

    #[test]
    fn compiler_tasty_lookup_does_not_shadow_a_source_owned_module_pair() {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let source_origin = SymbolOrigin::Source(SourceId::from_index(42));
        let mut packages = Packages::new();
        let entries = packages.enter(
            &mut store,
            source_origin,
            &["dotty", "tools", "dotc", "core"],
        );
        let package = entries.last().unwrap().symbol;
        let package_scope = entries.last().unwrap().scope;
        let object_name = Name::new(store.names.intern("Contexts"), Namespace::Term);
        let object = store.symbols.alloc(Symbol {
            name: object_name,
            owner: Some(package),
            kind: SymbolKind::Object,
            flags: SymbolFlags::EMPTY,
            visibility: Visibility::Public,
            info: SymbolInfo::Missing,
            origin: source_origin,
            annotations: Vec::new(),
            position: None,
            links: dotty_core::SymbolLinks::default(),
        });
        store
            .scopes
            .get_mut(package_scope)
            .enter(object_name, object);
        let module_name = Name::new(store.names.intern("Contexts$"), Namespace::Type);
        let module_class = store.symbols.alloc(Symbol {
            name: module_name,
            owner: Some(package),
            kind: SymbolKind::ModuleClass,
            flags: SymbolFlags::EMPTY,
            visibility: Visibility::Public,
            info: SymbolInfo::Missing,
            origin: source_origin,
            annotations: Vec::new(),
            position: None,
            links: dotty_core::SymbolLinks::default(),
        });
        store
            .scopes
            .get_mut(package_scope)
            .enter(module_name, module_class);

        let compiler_root = fixture_path("../dotty-tasty/tests/fixtures/scala3-compiler");
        let class_path = CompositeClassPath::new(vec![
            Box::new(DirectoryClassPath::new(compiler_root)),
            Box::new(ModuleTastyPath {
                entries: Arc::new(Mutex::new(std::collections::HashMap::new())),
                object_missing: Arc::new(AtomicBool::new(false)),
                object_corrupted: Arc::new(AtomicBool::new(false)),
            }),
        ]);
        let mut resolver = ClasspathSymbolResolver::new(
            class_path,
            definitions,
            LoadingSession::with_packages(packages),
        );
        assert_eq!(
            resolver.resolve_package(&mut store, &["dotty", "tools", "dotc", "core"]),
            Ok(Some(package))
        );
        let prefix = store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, package));
        let store_mark = store.checkpoint();
        let session_mark = resolver.session.checkpoint();
        let error = resolver
            .resolve_member(
                &mut store,
                &MemberRequest {
                    prefix,
                    name: object_name,
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                },
            )
            .expect_err("an unregistered source Object must conflict instead of creating a twin");
        assert!(matches!(
            error,
            ResolutionError::Malformed { ref reason }
                if reason.contains("not a valid canonical module object")
        ));
        assert_eq!(store.checkpoint(), store_mark);
        assert_eq!(resolver.session.checkpoint(), session_mark);
        assert!(resolver.session.modules.is_empty());
        assert_eq!(store.symbols.get(package).origin, source_origin);
        assert_eq!(store.symbols.get(object).origin, source_origin);
        assert_eq!(store.symbols.get(module_class).origin, source_origin);
        assert_eq!(
            store.scopes.get(package_scope).lookup_all(&object_name),
            [object]
        );
        assert_eq!(
            store.scopes.get(package_scope).lookup_all(&module_name),
            [module_class]
        );
    }

    #[test]
    fn stable_object_term_prefix_resolves_a_declared_field() {
        let binary = BinaryName::from_internal("me/cytrowski/tastyfixtures/semantic/Constants");
        let bytes = std::fs::read(fixture_path(
            "../dotty-tasty-unpickler/tests/fixtures/semantic/Constants.tasty",
        ))
        .expect("Constants.tasty fixture should exist");
        let path = ModuleTastyPath {
            entries: Arc::new(Mutex::new(std::collections::HashMap::from([(
                binary.clone(),
                bytes,
            )]))),
            object_missing: Arc::new(AtomicBool::new(false)),
            object_corrupted: Arc::new(AtomicBool::new(false)),
        };
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let mut resolver = ClasspathSymbolResolver::new(path, definitions, LoadingSession::new());
        let package = resolver
            .resolve_package(
                &mut store,
                &["me", "cytrowski", "tastyfixtures", "semantic"],
            )
            .unwrap()
            .unwrap();
        let package_ref = store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, package));
        let object_name = Name::new(store.names.intern("Constants"), Namespace::Term);
        let object = resolver
            .resolve_member(
                &mut store,
                &MemberRequest {
                    prefix: package_ref,
                    name: object_name,
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                },
            )
            .unwrap()
            .unwrap();
        let object_ref = store.types.alloc(Type::TermRef {
            prefix: package_ref,
            target: dotty_core::TermRefTarget::Symbol(object),
        });
        let field_name = Name::new(store.names.intern("int"), Namespace::Term);
        let field = resolver
            .resolve_member(
                &mut store,
                &MemberRequest {
                    prefix: object_ref,
                    name: field_name,
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                },
            )
            .unwrap()
            .unwrap();
        let module_class = resolver.session.modules[&binary].module_class;
        assert_eq!(store.symbols.get(field).kind, SymbolKind::Field);
        assert_eq!(store.symbols.get(field).owner, Some(module_class));
    }

    #[test]
    #[ignore = "requires pinned Scala 3.9.0 compiler/library JARs and JDK 21.0.8"]
    fn issue_1336_real_compiler_roots_keep_package_term_and_type_identities_distinct() {
        let classpath = std::env::var_os("DOTTY_ISSUE_1336_CLASSPATH")
            .expect("set DOTTY_ISSUE_1336_CLASSPATH to the ordered Scala 3.9.0 JAR list");
        let jmods = std::env::var_os("DOTTY_ISSUE_1336_JMODS")
            .expect("set DOTTY_ISSUE_1336_JMODS to Temurin 21.0.8 jmods");
        let mut entries: Vec<Box<dyn ClassPathEntry>> = std::env::split_paths(&classpath)
            .map(|path| {
                Box::new(JarClassPath::new(path, 21).expect("pinned JAR should open"))
                    as Box<dyn ClassPathEntry>
            })
            .collect();
        entries.push(Box::new(
            JdkClassPath::new(jmods).expect("pinned JDK 21.0.8 modules should open"),
        ));

        let targets = [
            ("dotty/tools/dotc/core", "Types", "TypeRef"),
            ("dotty/tools/dotc/core", "Contexts", "FreshContext"),
            ("dotty/tools/dotc/core", "Symbols", "NoSymbol"),
            ("dotty/tools/dotc/parsing", "Tokens", "maxToken"),
            ("dotty/tools/dotc/typer", "ProtoTypes", "NoViewsAllowed"),
        ];
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let mut resolver = ClasspathSymbolResolver::new(
            CompositeClassPath::new(entries),
            definitions,
            LoadingSession::new(),
        )
        .with_module_resolution_profile_collection();

        let mut candidate_term_requests = 0usize;
        let mut candidate_type_requests = 0usize;
        let mut candidate_term_symbols = 0usize;
        let mut candidate_term_missing = 0usize;
        let mut candidate_term_errors = 0usize;
        let mut candidate_type_symbols = 0usize;
        let mut candidate_type_missing = 0usize;
        let mut candidate_type_errors = 0usize;
        let mut candidate_package_paths = 0usize;
        let mut candidate_package_hits = 0usize;
        for row in
            include_str!("../../../docs/typer-classpath-import-roots-1336-target-candidates.tsv")
                .lines()
                .skip(1)
        {
            let columns = row.split('\t').collect::<Vec<_>>();
            assert_eq!(columns.len(), 4, "invalid target candidate row: {row}");
            let candidate = columns[2];
            let (package_path, name) = candidate.rsplit_once('.').unwrap_or(("", candidate));
            let segments = package_path
                .split('.')
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>();
            let package = match resolver.resolve_package(&mut store, &segments).unwrap() {
                Some(package) => package,
                None => {
                    println!(
                        "{} package=missing term/type=request-not-applicable",
                        candidate
                    );
                    continue;
                }
            };
            let mut full_package_segments = segments.clone();
            full_package_segments.push(name);
            let full_package = resolver
                .resolve_package(&mut store, &full_package_segments)
                .unwrap();
            candidate_package_paths += 1;
            if full_package.is_some() {
                candidate_package_hits += 1;
            }
            println!(
                "ISSUE1336_PACKAGE\t{}\t{}\t{}",
                columns[0],
                candidate,
                if full_package.is_some() {
                    "present"
                } else {
                    "absent"
                }
            );
            let prefix = store
                .types
                .alloc(Type::type_ref(definitions.no_prefix, package));
            for (namespace, request_count) in [
                (Namespace::Term, &mut candidate_term_requests),
                (Namespace::Type, &mut candidate_type_requests),
            ] {
                let member_name = Name::new(store.names.intern(name), namespace);
                let result = resolver.resolve_member(
                    &mut store,
                    &MemberRequest {
                        prefix,
                        name: member_name,
                        selector: MemberSelector::Unique,
                        space: MemberSpace::Prefix,
                    },
                );
                *request_count += 1;
                let result_class = match &result {
                    Ok(Some(symbol)) => format!("symbol:{:?}", store.symbols.get(*symbol).kind),
                    Ok(None) => "none".to_owned(),
                    Err(_) => "error".to_owned(),
                };
                match (namespace, &result) {
                    (Namespace::Term, Ok(Some(_))) => candidate_term_symbols += 1,
                    (Namespace::Term, Ok(None)) => candidate_term_missing += 1,
                    (Namespace::Term, Err(_)) => candidate_term_errors += 1,
                    (Namespace::Type, Ok(Some(_))) => candidate_type_symbols += 1,
                    (Namespace::Type, Ok(None)) => candidate_type_missing += 1,
                    (Namespace::Type, Err(_)) => candidate_type_errors += 1,
                }
                println!(
                    "cohort_candidate={} namespace={namespace:?} result={result:?}",
                    candidate
                );
                println!(
                    "ISSUE1336_RESULT\t{}\t{}\t{namespace:?}\t{result_class}\t{result:?}",
                    columns[0], candidate,
                );
            }
        }
        println!(
            "cohort_candidate_term_requests={candidate_term_requests} term_symbols={candidate_term_symbols} term_missing={candidate_term_missing} term_errors={candidate_term_errors} cohort_candidate_type_requests={candidate_type_requests} type_symbols={candidate_type_symbols} type_missing={candidate_type_missing} type_errors={candidate_type_errors}"
        );
        println!(
            "cohort_candidate_package_paths={candidate_package_paths} package_path_hits={candidate_package_hits}"
        );
        assert_eq!(candidate_term_requests, 118);
        assert_eq!(candidate_term_symbols, 26);
        assert_eq!(candidate_term_missing, 92);
        assert_eq!(candidate_term_errors, 0);
        assert_eq!(candidate_type_requests, 118);
        assert_eq!(candidate_type_symbols, 4);
        assert_eq!(candidate_type_missing, 91);
        assert_eq!(candidate_type_errors, 23);
        assert_eq!(candidate_package_paths, 118);
        assert_eq!(candidate_package_hits, 1);

        for (package_path, target, selected_member) in targets {
            let segments = package_path.split('/').collect::<Vec<_>>();
            let package = resolver
                .resolve_package(&mut store, &segments)
                .unwrap()
                .expect("pinned package path should be present");
            let prefix = store
                .types
                .alloc(Type::type_ref(definitions.no_prefix, package));
            let term_name = Name::new(store.names.intern(target), Namespace::Term);
            let object = resolver.resolve_member(
                &mut store,
                &MemberRequest {
                    prefix,
                    name: term_name,
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                },
            );

            let type_name = Name::new(store.names.intern(target), Namespace::Type);
            let class = resolver.resolve_member(
                &mut store,
                &MemberRequest {
                    prefix,
                    name: type_name,
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                },
            );
            let term_result = match &object {
                Ok(Some(symbol)) => format!("ok:{:?}", store.symbols.get(*symbol).kind),
                Ok(None) => "ok:none".to_owned(),
                Err(error) => format!("error:{error:?}"),
            };
            if let Ok(Some(object)) = &object {
                let object_prefix = store.types.alloc(Type::TermRef {
                    prefix,
                    target: dotty_core::TermRefTarget::Symbol(*object),
                });
                let member_name = Name::new(store.names.intern(selected_member), Namespace::Term);
                let member_result = resolver.resolve_member(
                    &mut store,
                    &MemberRequest {
                        prefix: object_prefix,
                        name: member_name,
                        selector: MemberSelector::Unique,
                        space: MemberSpace::Prefix,
                    },
                );
                println!(
                    "  selected_member={} result={member_result:?}",
                    selected_member
                );
                println!(
                    "ISSUE1336_MEMBER\t{package_path}.{target}\t{selected_member}\tTerm\t{member_result:?}"
                );
            }
            let type_result = match class {
                Ok(Some(symbol)) => format!("ok:{:?}", store.symbols.get(symbol).kind),
                Ok(None) => "ok:none".to_owned(),
                Err(error) => format!("error:{error:?}"),
            };
            println!(
                "{}::{} term={} type={}",
                package_path, target, term_result, type_result
            );
        }

        let profile = resolver.take_module_resolution_profile();
        println!(
            "package_term_requests={} package_term_successes={} package_term_errors={} new_objects={} completed_module_classes={} module_member_successes={} selected_member_attempts={}",
            profile.package_term_requests,
            profile.package_term_successes,
            profile.package_term_errors,
            profile.new_object_identities,
            profile.completed_module_classes.len(),
            profile.module_member_successes,
            profile.member_targets.len(),
        );
        assert_eq!(profile.package_term_requests, 123);
        assert_eq!(profile.package_term_successes, 30);
        assert_eq!(profile.package_term_errors, 0);
        assert_eq!(profile.completed_module_classes.len(), 2);
        assert_eq!(profile.module_member_successes, 2);
        assert_eq!(profile.member_targets.len(), 5);
    }

    #[test]
    fn package_term_without_module_evidence_returns_none_without_mutation() {
        let name = BinaryName::from_internal("scala/annotation/StaticAnnotation");
        let bytes = std::fs::read(fixture_path(
            "../dotty-tasty/tests/fixtures/scala3-library/scala/annotation/StaticAnnotation.tasty",
        ))
        .expect("StaticAnnotation.tasty fixture should exist");
        let path = ModuleTastyPath {
            entries: Arc::new(Mutex::new(std::collections::HashMap::from([(name, bytes)]))),
            object_missing: Arc::new(AtomicBool::new(false)),
            object_corrupted: Arc::new(AtomicBool::new(false)),
        };
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let mut resolver = ClasspathSymbolResolver::new(path, definitions, LoadingSession::new())
            .with_module_resolution_profile_collection();
        let package = resolver
            .resolve_package(&mut store, &["scala", "annotation"])
            .unwrap()
            .unwrap();
        let prefix = store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, package));
        let request = MemberRequest {
            prefix,
            name: Name::new(store.names.intern("StaticAnnotation"), Namespace::Term),
            selector: MemberSelector::Unique,
            space: MemberSpace::Prefix,
        };
        let store_mark = store.checkpoint();
        let session_mark = resolver.session.checkpoint();

        assert_eq!(resolver.resolve_member(&mut store, &request), Ok(None));
        assert_eq!(store.checkpoint(), store_mark);
        assert_eq!(resolver.session.checkpoint(), session_mark);
        let profile = resolver.take_module_resolution_profile();
        assert_eq!(profile.package_term_requests, 1);
        assert_eq!(profile.package_term_unresolved, 1);
        assert_eq!(profile.package_term_targets.len(), 1);
    }

    #[test]
    fn stable_object_term_prefix_completes_and_resolves_module_members() {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let mut resolver = ClasspathSymbolResolver::new(
            ModuleTastyPath::both_fixture(),
            definitions,
            LoadingSession::new(),
        )
        .with_module_resolution_profile_collection();
        let package = resolver
            .resolve_package(
                &mut store,
                &["me", "cytrowski", "tastyfixtures", "semantic"],
            )
            .unwrap()
            .unwrap();
        let package_prefix = store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, package));
        let both_name = Name::new(store.names.intern("Both"), Namespace::Term);
        let object = resolver
            .resolve_member(
                &mut store,
                &MemberRequest {
                    prefix: package_prefix,
                    name: both_name,
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                },
            )
            .unwrap()
            .unwrap();
        let object_ref = store.types.alloc(Type::TermRef {
            prefix: package_prefix,
            target: dotty_core::TermRefTarget::Symbol(object),
        });
        let make_name = Name::new(store.names.intern("make"), Namespace::Term);
        let make = resolver
            .resolve_member(
                &mut store,
                &MemberRequest {
                    prefix: object_ref,
                    name: make_name,
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                },
            )
            .unwrap()
            .unwrap();
        let module_class = resolver.session.modules
            [&BinaryName::from_internal("me/cytrowski/tastyfixtures/semantic/Both")]
            .module_class;
        assert_eq!(store.symbols.get(make).kind, SymbolKind::Method);
        assert_eq!(store.symbols.get(make).owner, Some(module_class));
        assert!(matches!(
            store.symbols.get(module_class).info,
            SymbolInfo::Complete(info)
                if matches!(store.types.get(info), Type::ClassInfo(class_info)
                    if class_info.class == module_class
                        && store.scopes.get(class_info.declarations).owner == Some(module_class))
        ));

        let module_prefix = store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, module_class));
        assert_eq!(
            resolver.resolve_member(
                &mut store,
                &MemberRequest {
                    prefix: package_prefix,
                    name: make_name,
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Explicit(module_prefix),
                },
            ),
            Ok(Some(make))
        );
        assert_eq!(
            resolver.resolve_member(
                &mut store,
                &MemberRequest {
                    prefix: object_ref,
                    name: make_name,
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Explicit(package_prefix),
                },
            ),
            Ok(None),
            "Explicit owner must not fall back to the Object prefix"
        );
        let profile = resolver.take_module_resolution_profile();
        assert_eq!(profile.package_term_requests, 1);
        assert_eq!(profile.package_term_successes, 1);
        assert_eq!(profile.module_member_successes, 2);
        assert_eq!(profile.resolved_object_targets.len(), 1);
        assert_eq!(profile.resolved_member_targets.len(), 2);
        assert_eq!(profile.package_term_targets.len(), 1);
        assert_eq!(profile.module_targets.len(), 1);
        assert_eq!(profile.member_targets.len(), 2);
        assert_eq!(
            profile.completed_module_classes,
            std::collections::BTreeSet::from([
                "me/cytrowski/tastyfixtures/semantic/Both".to_owned()
            ])
        );

        let class_info = match store.symbols.get(module_class).info {
            SymbolInfo::Complete(info) => info,
            _ => panic!("module class should be complete"),
        };
        let Type::ClassInfo(class_info) = store.types.get(class_info) else {
            panic!("module class should have ClassInfo")
        };
        let declarations = class_info.declarations;
        let duplicate = store.symbols.alloc(Symbol {
            name: make_name,
            owner: Some(module_class),
            kind: SymbolKind::Method,
            flags: dotty_core::SymbolFlags::EMPTY,
            visibility: dotty_core::Visibility::Public,
            info: SymbolInfo::Missing,
            origin: dotty_core::SymbolOrigin::Synthetic,
            annotations: Vec::new(),
            position: None,
            links: dotty_core::SymbolLinks::default(),
        });
        store
            .scopes
            .get_mut(declarations)
            .enter(make_name, duplicate);
        let store_mark = store.checkpoint();
        let session_mark = resolver.session.checkpoint();
        assert_eq!(
            resolver.resolve_member(
                &mut store,
                &MemberRequest {
                    prefix: object_ref,
                    name: make_name,
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                },
            ),
            Err(ResolutionError::Ambiguous { candidates: 2 })
        );
        assert_eq!(store.checkpoint(), store_mark);
        assert_eq!(resolver.session.checkpoint(), session_mark);
    }

    #[test]
    fn missing_module_member_rolls_back_on_demand_completion() {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let mut resolver = ClasspathSymbolResolver::new(
            ModuleTastyPath::both_fixture(),
            definitions,
            LoadingSession::new(),
        )
        .with_module_resolution_profile_collection()
        .with_resolver_profile_collection();
        let package = resolver
            .resolve_package(
                &mut store,
                &["me", "cytrowski", "tastyfixtures", "semantic"],
            )
            .unwrap()
            .unwrap();
        let package_prefix = store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, package));
        let both_name = Name::new(store.names.intern("Both"), Namespace::Term);
        let object = resolver
            .resolve_member(
                &mut store,
                &MemberRequest {
                    prefix: package_prefix,
                    name: both_name,
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                },
            )
            .unwrap()
            .unwrap();
        let object_ref = store.types.alloc(Type::TermRef {
            prefix: package_prefix,
            target: dotty_core::TermRefTarget::Symbol(object),
        });
        let module_class = resolver.session.modules
            [&BinaryName::from_internal("me/cytrowski/tastyfixtures/semantic/Both")]
            .module_class;
        let absent_name = Name::new(store.names.intern("absent"), Namespace::Term);
        let store_mark = store.checkpoint();
        let session_mark = resolver.session.checkpoint();
        let result = resolver.resolve_member(
            &mut store,
            &MemberRequest {
                prefix: object_ref,
                name: absent_name,
                selector: MemberSelector::Unique,
                space: MemberSpace::Prefix,
            },
        );

        assert_eq!(result, Ok(None));
        assert_eq!(store.checkpoint(), store_mark);
        assert_eq!(resolver.session.checkpoint(), session_mark);
        assert_eq!(store.symbols.get(module_class).info, SymbolInfo::Missing);
        assert_eq!(store.symbols.get(object).info, SymbolInfo::Missing);
        assert!(resolver.negative_module_member_lookups.contains(&(
            BinaryName::from_internal("me/cytrowski/tastyfixtures/semantic/Both"),
            absent_name
        )));

        assert_eq!(
            resolver.resolve_member(
                &mut store,
                &MemberRequest {
                    prefix: object_ref,
                    name: absent_name,
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                },
            ),
            Ok(None),
            "a repeated missing-member lookup should use the cached classpath result"
        );
        assert_eq!(store.checkpoint(), store_mark);
        assert_eq!(resolver.session.checkpoint(), session_mark);

        let another_absent_name = Name::new(store.names.intern("alsoAbsent"), Namespace::Term);
        assert_eq!(
            resolver.resolve_member(
                &mut store,
                &MemberRequest {
                    prefix: object_ref,
                    name: another_absent_name,
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                },
            ),
            Ok(None),
            "a distinct missing member should reuse the immutable decoded module data"
        );
        assert_eq!(store.checkpoint(), store_mark);
        assert_eq!(resolver.session.checkpoint(), session_mark);
        let resolver_profile = resolver.take_resolver_profile();
        assert!(resolver_profile.module_snapshot_validation.calls >= 1);
        assert!(resolver_profile.classpath_resource_lookup.calls > 0);
        assert!(
            resolver_profile.module_snapshot_validation.exclusive_ns
                <= resolver_profile.module_snapshot_validation.elapsed_ns
        );
        let profile = resolver.take_module_resolution_profile();
        assert_eq!(profile.module_decode_cache_misses, 1);
        assert_eq!(profile.module_decode_cache_hits, 0);
        assert_eq!(profile.module_declaration_miss_shortcuts, 1);

        let make_text = store.names.intern("make");
        let make_type = MemberRequest {
            prefix: object_ref,
            name: Name::new(make_text, Namespace::Type),
            selector: MemberSelector::Unique,
            space: MemberSpace::Prefix,
        };
        assert_eq!(resolver.resolve_member(&mut store, &make_type), Ok(None));
        assert_eq!(store.checkpoint(), store_mark);
        assert_eq!(resolver.session.checkpoint(), session_mark);
        let make_term = MemberRequest {
            name: Name::new(make_text, Namespace::Term),
            ..make_type
        };
        let make_store_mark = store.checkpoint();
        let make_resolver_mark = resolver.checkpoint();
        let make = resolver
            .resolve_member(&mut store, &make_term)
            .unwrap()
            .unwrap();
        assert_eq!(store.symbols.get(make).kind, SymbolKind::Method);
        assert_eq!(store.symbols.get(make).owner, Some(module_class));
        resolver.rollback_to(&mut store, make_resolver_mark);
        store.rollback_to(make_store_mark);
        assert_eq!(store.symbols.get(module_class).info, SymbolInfo::Missing);
        assert!(
            resolver
                .validated_module_declarations
                .contains(&BinaryName::from_internal(
                    "me/cytrowski/tastyfixtures/semantic/Both"
                ))
        );

        assert!(
            resolver
                .complete_module_class(&mut store, module_class)
                .unwrap()
        );
        assert_eq!(
            resolver.resolve_member(
                &mut store,
                &MemberRequest {
                    prefix: object_ref,
                    name: absent_name,
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                },
            ),
            Ok(None),
            "a cached miss must still inspect a module completed through another resolver hook"
        );
        let SymbolInfo::Complete(info) = store.symbols.get(module_class).info else {
            panic!("the positive lookup should complete the module");
        };
        let Type::ClassInfo(info) = store.types.get(info) else {
            panic!("the module should retain ClassInfo");
        };
        let declarations = info.declarations;
        let member = Symbol {
            name: absent_name,
            owner: Some(module_class),
            kind: SymbolKind::Field,
            flags: SymbolFlags::EMPTY,
            visibility: Visibility::Public,
            info: SymbolInfo::Missing,
            origin: SymbolOrigin::Synthetic,
            annotations: Vec::new(),
            position: None,
            links: dotty_core::SymbolLinks::default(),
        };
        let added = store.symbols.alloc(member.clone());
        store.scopes.get_mut(declarations).enter(absent_name, added);
        let absent_request = MemberRequest {
            prefix: object_ref,
            name: absent_name,
            selector: MemberSelector::Unique,
            space: MemberSpace::Prefix,
        };
        assert_eq!(
            resolver.resolve_member(&mut store, &absent_request),
            Ok(Some(added))
        );
        let duplicate = store.symbols.alloc(member);
        store
            .scopes
            .get_mut(declarations)
            .enter(absent_name, duplicate);
        assert_eq!(
            resolver.resolve_member(&mut store, &absent_request),
            Err(ResolutionError::Ambiguous { candidates: 2 })
        );
    }

    #[test]
    fn failed_module_tasty_decode_is_included_in_load_profile() {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let path = ModuleTastyPath::both_fixture();
        let mut resolver =
            ClasspathSymbolResolver::new(path.clone(), definitions, LoadingSession::new())
                .with_resolver_profile_collection();
        let package = resolver
            .resolve_package(
                &mut store,
                &["me", "cytrowski", "tastyfixtures", "semantic"],
            )
            .unwrap()
            .unwrap();
        let package_prefix = store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, package));
        let both_name = Name::new(store.names.intern("Both"), Namespace::Term);
        let object = resolver
            .resolve_member(
                &mut store,
                &MemberRequest {
                    prefix: package_prefix,
                    name: both_name,
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                },
            )
            .unwrap()
            .unwrap();
        path.entries.lock().unwrap().insert(
            BinaryName::from_internal("me/cytrowski/tastyfixtures/semantic/Both"),
            vec![0],
        );
        let object_ref = store.types.alloc(Type::TermRef {
            prefix: package_prefix,
            target: dotty_core::TermRefTarget::Symbol(object),
        });
        let absent_name = Name::new(store.names.intern("absent"), Namespace::Term);

        let result = resolver.resolve_member(
            &mut store,
            &MemberRequest {
                prefix: object_ref,
                name: absent_name,
                selector: MemberSelector::Unique,
                space: MemberSpace::Prefix,
            },
        );
        assert!(matches!(result, Err(ResolutionError::Malformed { .. })));
        let profile = resolver
            .take_load_profiles()
            .into_iter()
            .find(|profile| profile.root == "me/cytrowski/tastyfixtures/semantic/Both")
            .expect("failed module completion should publish its load profile");
        assert_eq!(profile.tasty_decode_calls, 1);
        assert!(profile.tasty_decode_ns > 0);
        assert!(profile.tasty_decode_counters["parse_ns"] > 0);
        assert_eq!(profile.tasty_decode_counters["index_ns"], 0);
        assert_eq!(profile.module_completion_errors, 1);
        assert_eq!(profile.module_completion_successes, 0);
    }

    #[test]
    fn changed_module_resource_invalidates_rolled_back_decode_and_miss_caches() {
        let binary = BinaryName::from_internal("me/cytrowski/tastyfixtures/semantic/Both");
        let path = ModuleTastyPath::both_fixture();
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let mut resolver =
            ClasspathSymbolResolver::new(path.clone(), definitions, LoadingSession::new());
        let package = resolver
            .resolve_package(
                &mut store,
                &["me", "cytrowski", "tastyfixtures", "semantic"],
            )
            .unwrap()
            .unwrap();
        let package_prefix = store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, package));
        let both_name = Name::new(store.names.intern("Both"), Namespace::Term);
        let object = resolver
            .resolve_member(
                &mut store,
                &MemberRequest {
                    prefix: package_prefix,
                    name: both_name,
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                },
            )
            .unwrap()
            .unwrap();
        let object_prefix = store.types.alloc(Type::TermRef {
            prefix: package_prefix,
            target: dotty_core::TermRefTarget::Symbol(object),
        });
        let absent = |store: &mut SemanticStore, name: &str| MemberRequest {
            prefix: object_prefix,
            name: Name::new(store.names.intern(name), Namespace::Term),
            selector: MemberSelector::Unique,
            space: MemberSpace::Prefix,
        };

        let first_absent = absent(&mut store, "absentFirst");
        assert_eq!(resolver.resolve_member(&mut store, &first_absent), Ok(None));
        let module_class = resolver.session.modules[&binary].module_class;
        assert_eq!(store.symbols.get(module_class).info, SymbolInfo::Missing);

        path.entries
            .lock()
            .unwrap()
            .insert(binary.clone(), b"corrupted artifact".to_vec());

        for request in [
            absent(&mut store, "absentSecond"),
            absent(&mut store, "make"),
        ] {
            let store_checkpoint = store.checkpoint();
            let session_checkpoint = resolver.session.checkpoint();
            let result = resolver.resolve_member(&mut store, &request);
            assert!(
                matches!(&result, Err(ResolutionError::Malformed { reason }) if reason.contains("invalid TASTy")),
                "changed resource must be decoded and rejected, got {result:?}"
            );
            assert_eq!(store.checkpoint(), store_checkpoint);
            assert_eq!(resolver.session.checkpoint(), session_checkpoint);
            assert_eq!(store.symbols.get(module_class).info, SymbolInfo::Missing);
        }
        assert!(!resolver.validated_module_declarations.contains(&binary));
        assert!(
            !resolver
                .negative_module_member_lookups
                .iter()
                .any(|(module, _)| module == &binary)
        );
    }

    #[test]
    fn changed_module_parent_invalidates_rolled_back_hierarchy_proof() {
        let binary = BinaryName::from_internal("me/cytrowski/tastyfixtures/semantic/Both");
        let path = ModuleTastyPath::both_fixture();
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let mut resolver =
            ClasspathSymbolResolver::new(path.clone(), definitions, LoadingSession::new());
        let package = resolver
            .resolve_package(
                &mut store,
                &["me", "cytrowski", "tastyfixtures", "semantic"],
            )
            .unwrap()
            .unwrap();
        let package_prefix = store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, package));
        let both_name = Name::new(store.names.intern("Both"), Namespace::Term);
        let object = resolver
            .resolve_member(
                &mut store,
                &MemberRequest {
                    prefix: package_prefix,
                    name: both_name,
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                },
            )
            .unwrap()
            .unwrap();
        let object_prefix = store.types.alloc(Type::TermRef {
            prefix: package_prefix,
            target: dotty_core::TermRefTarget::Symbol(object),
        });
        let first_absent = MemberRequest {
            prefix: object_prefix,
            name: Name::new(store.names.intern("absentFirst"), Namespace::Term),
            selector: MemberSelector::Unique,
            space: MemberSpace::Prefix,
        };

        assert_eq!(resolver.resolve_member(&mut store, &first_absent), Ok(None));
        let module_class = resolver.session.modules[&binary].module_class;
        assert_eq!(store.symbols.get(module_class).info, SymbolInfo::Missing);
        assert!(
            resolver.validated_module_resources[&binary]
                .contains_key(&BinaryName::from_internal("java/lang/Object"))
        );

        // Keep the module TASTy unchanged while removing its hierarchy parent.
        path.object_missing.store(true, Ordering::SeqCst);
        for name in ["absentSecond", "make"] {
            let request = MemberRequest {
                prefix: object_prefix,
                name: Name::new(store.names.intern(name), Namespace::Term),
                selector: MemberSelector::Unique,
                space: MemberSpace::Prefix,
            };
            let store_checkpoint = store.checkpoint();
            let session_checkpoint = resolver.session.checkpoint();
            let result = resolver.resolve_member(&mut store, &request);
            assert!(
                matches!(&result, Err(ResolutionError::Malformed { reason }) if reason.contains("java/lang/Object")),
                "a changed hierarchy dependency must not be hidden by the miss shortcut: {result:?}"
            );
            assert_eq!(store.checkpoint(), store_checkpoint);
            assert_eq!(resolver.session.checkpoint(), session_checkpoint);
            assert_eq!(store.symbols.get(module_class).info, SymbolInfo::Missing);
        }
        assert!(!resolver.validated_module_declarations.contains(&binary));
        assert!(!resolver.validated_module_resources.contains_key(&binary));
    }

    #[test]
    fn precompleted_parent_is_part_of_rolled_back_hierarchy_proof() {
        let binary = BinaryName::from_internal("me/cytrowski/tastyfixtures/semantic/Both");
        let path = ModuleTastyPath::both_fixture();
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let mut resolver =
            ClasspathSymbolResolver::new(path.clone(), definitions, LoadingSession::new());
        let package = resolver
            .resolve_package(
                &mut store,
                &["me", "cytrowski", "tastyfixtures", "semantic"],
            )
            .unwrap()
            .unwrap();
        let package_prefix = store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, package));
        let both_name = Name::new(store.names.intern("Both"), Namespace::Term);
        let object = resolver
            .resolve_member(
                &mut store,
                &MemberRequest {
                    prefix: package_prefix,
                    name: both_name,
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                },
            )
            .unwrap()
            .unwrap();
        let object_prefix = store.types.alloc(Type::TermRef {
            prefix: package_prefix,
            target: dotty_core::TermRefTarget::Symbol(object),
        });
        let outer_store_checkpoint = store.checkpoint();
        let outer_session_checkpoint = resolver.session.checkpoint();

        resolver
            .load_class(&mut store, &BinaryName::from_internal("java/lang/Object"))
            .unwrap();
        let absent = MemberRequest {
            prefix: object_prefix,
            name: Name::new(store.names.intern("absentBeforeRollback"), Namespace::Term),
            selector: MemberSelector::Unique,
            space: MemberSpace::Prefix,
        };
        assert_eq!(resolver.resolve_member(&mut store, &absent), Ok(None));
        assert!(!resolver.validated_module_declarations.contains(&binary));
        assert!(!resolver.validated_module_resources.contains_key(&binary));

        // Simulate an outer caller rolling back the earlier completed parent
        // after module resolution itself succeeded and cached a declaration miss.
        resolver
            .session
            .rollback_to(&mut store, outer_session_checkpoint);
        store.rollback_to(outer_store_checkpoint);
        path.object_missing.store(true, Ordering::SeqCst);

        let absent_after_rollback = MemberRequest {
            prefix: object_prefix,
            name: Name::new(store.names.intern("absentAfterRollback"), Namespace::Term),
            selector: MemberSelector::Unique,
            space: MemberSpace::Prefix,
        };
        let result = resolver.resolve_member(&mut store, &absent_after_rollback);
        assert!(
            matches!(&result, Err(ResolutionError::Malformed { reason }) if reason.contains("java/lang/Object")),
            "a rolled-back precompleted parent must not leave a valid miss proof: {result:?}"
        );
    }

    #[test]
    fn changed_precompleted_parent_bytes_cannot_validate_a_module_miss() {
        let binary = BinaryName::from_internal("me/cytrowski/tastyfixtures/semantic/Both");
        let path = ModuleTastyPath::both_fixture();
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let mut resolver =
            ClasspathSymbolResolver::new(path.clone(), definitions, LoadingSession::new());
        let package = resolver
            .resolve_package(
                &mut store,
                &["me", "cytrowski", "tastyfixtures", "semantic"],
            )
            .unwrap()
            .unwrap();
        let package_prefix = store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, package));
        let both_name = Name::new(store.names.intern("Both"), Namespace::Term);
        let object = resolver
            .resolve_member(
                &mut store,
                &MemberRequest {
                    prefix: package_prefix,
                    name: both_name,
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                },
            )
            .unwrap()
            .unwrap();
        let object_prefix = store.types.alloc(Type::TermRef {
            prefix: package_prefix,
            target: dotty_core::TermRefTarget::Symbol(object),
        });
        let outer_store_checkpoint = store.checkpoint();
        let outer_session_checkpoint = resolver.session.checkpoint();

        resolver
            .load_class(&mut store, &BinaryName::from_internal("java/lang/Object"))
            .unwrap();
        path.object_corrupted.store(true, Ordering::SeqCst);
        let absent = MemberRequest {
            prefix: object_prefix,
            name: Name::new(store.names.intern("absentFirst"), Namespace::Term),
            selector: MemberSelector::Unique,
            space: MemberSpace::Prefix,
        };
        assert_eq!(resolver.resolve_member(&mut store, &absent), Ok(None));
        assert!(!resolver.validated_module_declarations.contains(&binary));

        resolver
            .session
            .rollback_to(&mut store, outer_session_checkpoint);
        store.rollback_to(outer_store_checkpoint);
        let absent_after_rollback = MemberRequest {
            prefix: object_prefix,
            name: Name::new(store.names.intern("absentSecond"), Namespace::Term),
            selector: MemberSelector::Unique,
            space: MemberSpace::Prefix,
        };
        let result = resolver.resolve_member(&mut store, &absent_after_rollback);
        assert!(
            matches!(&result, Err(ResolutionError::Malformed { reason }) if reason.contains("java/lang/Object")),
            "current corrupted bytes must not be hidden by an older cached ClassInfo: {result:?}"
        );
    }

    #[test]
    fn decoded_module_without_successful_completion_does_not_prove_member_absence() {
        let binary = BinaryName::from_internal("me/cytrowski/tastyfixtures/semantic/Both");
        let path = ModuleTastyPath::both_fixture();
        let bytes = path.entries.lock().unwrap()[&binary].clone();
        let decoded = crate::tasty_symbol::decode_module_class_with_resolvers(
            &bytes,
            &BinaryName::from_internal(format!("{}$", binary.as_internal())),
            |_, _| None,
            |_, _| None,
        )
        .unwrap();
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let mut resolver =
            ClasspathSymbolResolver::new(path.clone(), definitions, LoadingSession::new());
        let identity = resolver
            .ensure_module_identity(&mut store, &binary)
            .unwrap();
        resolver.decoded_module_cache.insert(
            binary.clone(),
            crate::loader::CachedDecodedModule {
                resource: ClassResource::new(
                    bytes,
                    ClassFormat::Tasty,
                    ClassOrigin::Directory(PathBuf::from("<module-fixture>")),
                ),
                decoded,
            },
        );
        path.entries.lock().unwrap().remove(&binary);
        let prefix = store.types.alloc(Type::TermRef {
            prefix: definitions.no_prefix,
            target: dotty_core::TermRefTarget::Symbol(identity.object),
        });
        let expected = Err(ResolutionError::Malformed {
            reason: format!(
                "module completion failed: {}",
                ClassLoadError::NotFound(binary.clone())
            ),
        });
        for text in ["absent", "alsoAbsent"] {
            let request = MemberRequest {
                prefix,
                name: Name::new(store.names.intern(text), Namespace::Term),
                selector: MemberSelector::Unique,
                space: MemberSpace::Prefix,
            };
            let store_mark = store.checkpoint();
            let session_mark = resolver.session.checkpoint();
            assert_eq!(resolver.resolve_member(&mut store, &request), expected);
            assert_eq!(store.checkpoint(), store_mark);
            assert_eq!(resolver.session.checkpoint(), session_mark);
            assert!(!resolver.validated_module_declarations.contains(&binary));
        }
    }

    #[test]
    fn package_term_does_not_infer_a_module_from_an_ordinary_class() {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let path = member_class_path();
        path.classes.lock().unwrap().insert(
            BinaryName::from_internal("sample/Foo"),
            test_class_stub("sample/Foo", 0x0021),
        );
        path.classes.lock().unwrap().insert(
            BinaryName::from_internal("sample/Foo$"),
            test_class_stub("sample/Foo$", 0x0021),
        );
        let mut resolver = ClasspathSymbolResolver::new(path, definitions, LoadingSession::new())
            .with_module_resolution_profile_collection();
        let package = resolver
            .resolve_package(&mut store, &["sample"])
            .unwrap()
            .unwrap();
        let prefix = store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, package));
        let request = MemberRequest {
            prefix,
            name: Name::new(store.names.intern("Foo"), Namespace::Term),
            selector: MemberSelector::Unique,
            space: MemberSpace::Prefix,
        };
        let store_mark = store.checkpoint();
        let session_mark = resolver.session.checkpoint();

        assert_eq!(resolver.resolve_member(&mut store, &request), Ok(None));
        assert_eq!(store.checkpoint(), store_mark);
        assert_eq!(resolver.session.checkpoint(), session_mark);
        let dollar_request = MemberRequest {
            prefix,
            name: Name::new(store.names.intern("Foo$"), Namespace::Term),
            selector: MemberSelector::Unique,
            space: MemberSpace::Prefix,
        };
        assert_eq!(
            resolver.resolve_member(&mut store, &dollar_request),
            Ok(None)
        );
    }

    #[test]
    fn explicit_package_term_request_does_not_create_module_identity() {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let mut resolver = ClasspathSymbolResolver::new(
            ModuleTastyPath::both_fixture(),
            definitions,
            LoadingSession::new(),
        );
        let package = resolver
            .resolve_package(
                &mut store,
                &["me", "cytrowski", "tastyfixtures", "semantic"],
            )
            .unwrap()
            .unwrap();
        let package_ref = store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, package));
        let both_name = Name::new(store.names.intern("Both"), Namespace::Term);

        assert_eq!(
            resolver.resolve_member(
                &mut store,
                &MemberRequest {
                    prefix: package_ref,
                    name: both_name,
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Explicit(package_ref),
                },
            ),
            Ok(None)
        );
        assert!(resolver.session.modules.is_empty());
        assert_eq!(
            resolver
                .take_module_resolution_profile()
                .package_term_requests,
            0
        );
    }

    #[test]
    fn package_term_does_not_replace_an_unbacked_source_owned_object() {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let mut resolver = ClasspathSymbolResolver::new(
            ModuleTastyPath::both_fixture(),
            definitions,
            LoadingSession::new(),
        );
        let package = resolver
            .resolve_package(
                &mut store,
                &["me", "cytrowski", "tastyfixtures", "semantic"],
            )
            .unwrap()
            .unwrap();
        let scope = resolver.package_scope(package).unwrap();
        let name = Name::new(store.names.intern("Both"), Namespace::Term);
        let source_object = store.symbols.alloc(Symbol {
            name,
            owner: Some(package),
            kind: SymbolKind::Object,
            flags: dotty_core::SymbolFlags::EMPTY,
            visibility: dotty_core::Visibility::Public,
            info: SymbolInfo::Missing,
            origin: dotty_core::SymbolOrigin::Synthetic,
            annotations: Vec::new(),
            position: None,
            links: dotty_core::SymbolLinks::default(),
        });
        store.scopes.get_mut(scope).enter(name, source_object);
        let package_ref = store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, package));
        let store_mark = store.checkpoint();

        assert!(matches!(
            resolver.resolve_member(
                &mut store,
                &MemberRequest {
                    prefix: package_ref,
                    name,
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                },
            ),
            Err(ResolutionError::Malformed { .. })
        ));
        assert_eq!(store.checkpoint(), store_mark);
        assert_eq!(store.scopes.get(scope).lookup_all(&name), &[source_object]);
        assert!(resolver.session.modules.is_empty());
    }

    #[test]
    fn malformed_module_tasty_errors_without_retaining_identity_or_package_changes() {
        let binary = BinaryName::from_internal("sample/Broken");
        let path = ModuleTastyPath {
            entries: Arc::new(Mutex::new(std::collections::HashMap::from([(
                binary,
                vec![0, 1, 2, 3],
            )]))),
            object_missing: Arc::new(AtomicBool::new(false)),
            object_corrupted: Arc::new(AtomicBool::new(false)),
        };
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let mut resolver = ClasspathSymbolResolver::new(path, definitions, LoadingSession::new())
            .with_module_resolution_profile_collection();
        let package = resolver
            .resolve_package(&mut store, &["sample"])
            .unwrap()
            .unwrap();
        let package_ref = store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, package));
        let broken_name = Name::new(store.names.intern("Broken"), Namespace::Term);
        let store_mark = store.checkpoint();
        let session_mark = resolver.session.checkpoint();

        assert!(matches!(
            resolver.resolve_member(
                &mut store,
                &MemberRequest {
                    prefix: package_ref,
                    name: broken_name,
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                },
            ),
            Err(ResolutionError::Malformed { .. })
        ));
        assert_eq!(store.checkpoint(), store_mark);
        assert_eq!(resolver.session.checkpoint(), session_mark);
        assert!(resolver.session.modules.is_empty());
        let profile = resolver.take_module_resolution_profile();
        assert_eq!(profile.package_term_requests, 1);
        assert_eq!(profile.package_term_errors, 1);
    }

    #[test]
    fn missing_ambiguous_and_failed_members_are_atomic_and_retryable() {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let path = member_class_path();
        let mut loader = ClassLoader::with_definitions(
            path.clone(),
            &mut store,
            definitions,
            LoadingSession::new(),
        );
        let b = loader
            .ensure_class_identity(&BinaryName::from_internal("sample/B"))
            .unwrap();
        let session = loader.into_session();
        let original = store.symbols.get(b).clone();
        let prefix = store.types.alloc(Type::type_ref(definitions.no_prefix, b));
        let mut resolver = ClasspathSymbolResolver::new(path.clone(), definitions, session);
        for name in ["absent", "over"] {
            let request = MemberRequest {
                prefix,
                name: Name::new(store.names.intern(name), Namespace::Term),
                selector: MemberSelector::Unique,
                space: MemberSpace::Prefix,
            };
            let mark = store.checkpoint();
            let resolver_mark = resolver.checkpoint();
            let session_mark = resolver.session.checkpoint();
            for _ in 0..2 {
                let result = resolver.resolve_member(&mut store, &request);
                if name == "absent" {
                    assert_eq!(result, Ok(None));
                } else {
                    assert_eq!(result, Err(ResolutionError::Ambiguous { candidates: 2 }));
                }
                assert_eq!(store.symbols.get(b), &original);
                assert_eq!(store.checkpoint(), mark);
                assert_eq!(resolver.checkpoint(), resolver_mark);
                assert_eq!(resolver.session.checkpoint(), session_mark);
            }
        }
        let request = MemberRequest {
            prefix,
            name: Name::new(store.names.intern("value"), Namespace::Term),
            selector: MemberSelector::Unique,
            space: MemberSpace::Prefix,
        };
        let bytes = path
            .classes
            .lock()
            .unwrap()
            .insert(BinaryName::from_internal("sample/B"), vec![0, 1, 2])
            .unwrap();
        let mark = store.checkpoint();
        assert!(
            matches!(resolver.resolve_member(&mut store, &request), Err(ResolutionError::Malformed { reason }) if reason.contains("invalid class file"))
        );
        assert_eq!(store.checkpoint(), mark);
        assert_eq!(store.symbols.get(b), &original);
        path.classes
            .lock()
            .unwrap()
            .insert(BinaryName::from_internal("sample/B"), bytes);
        assert!(
            resolver
                .resolve_member(&mut store, &request)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn direct_member_outer_rollback_removes_a_same_transaction_shell_and_member() {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let mut resolver =
            ClasspathSymbolResolver::new(member_class_path(), definitions, LoadingSession::new());
        let package = resolver
            .resolve_package(&mut store, &["sample"])
            .unwrap()
            .unwrap();
        let package_prefix = store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, package));
        let a_request = MemberRequest {
            prefix: package_prefix,
            name: Name::new(store.names.intern("A"), Namespace::Type),
            selector: MemberSelector::Unique,
            space: MemberSpace::Prefix,
        };
        let value = Name::new(store.names.intern("value"), Namespace::Term);
        for _ in 0..2 {
            let mark = resolver.checkpoint();
            let store_mark = store.checkpoint();
            resolver
                .resolve_member(&mut store, &a_request)
                .unwrap()
                .unwrap();
            let b = resolver.session.identified[&BinaryName::from_internal("sample/B")];
            assert_eq!(store.symbols.get(b).info, SymbolInfo::Missing);
            let prefix = store.types.alloc(Type::type_ref(definitions.no_prefix, b));
            let member = resolver
                .resolve_member(
                    &mut store,
                    &MemberRequest {
                        prefix,
                        name: value,
                        selector: MemberSelector::Unique,
                        space: MemberSpace::Prefix,
                    },
                )
                .unwrap()
                .unwrap();
            assert_eq!(store.symbols.get(member).owner, Some(b));
            resolver.rollback_to(&mut store, mark);
            store.rollback_to(store_mark);
            assert!(!store.symbols.contains(b));
            assert!(!store.symbols.contains(member));
            assert!(resolver.session.identified.is_empty());
            assert!(resolver.session.resolved.is_empty());
            assert_eq!(store.checkpoint(), store_mark);
        }
    }

    impl CompletionClassPath {
        fn new() -> Self {
            Self {
                classes: Arc::new(Mutex::new(std::collections::HashMap::from([
                    (
                        BinaryName::from_internal("sample/B"),
                        test_class_stub("sample/B", 0x0021),
                    ),
                    (
                        BinaryName::from_internal("sample/A"),
                        test_class_with_object_fields("sample/A", None, &[("b", "sample/B")]),
                    ),
                ]))),
                calls: Arc::new(AtomicUsize::new(0)),
            }
        }
    }

    impl ClassPathEntry for CompletionClassPath {
        fn find_class(&self, name: &BinaryName) -> Result<Option<ClassResource>, ClassPathError> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            Ok(self.classes.lock().unwrap().get(name).map(|bytes| {
                ClassResource::new(
                    bytes.clone(),
                    crate::class_path::ClassFormat::Class,
                    crate::class_path::ClassOrigin::Directory("<completion>".into()),
                )
            }))
        }

        fn contains_package(&self, package: &[&str]) -> Result<bool, ClassPathError> {
            let prefix = format!("{}/", package.join("/"));
            Ok(self
                .classes
                .lock()
                .unwrap()
                .keys()
                .any(|name| name.as_internal().starts_with(&prefix)))
        }
    }

    #[test]
    fn complete_class_outer_rollback_removes_shells_created_in_the_transaction() {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let mut resolver = ClasspathSymbolResolver::new(
            CompletionClassPath::new(),
            definitions,
            LoadingSession::new(),
        );
        let package = resolver
            .resolve_package(&mut store, &["sample"])
            .unwrap()
            .unwrap();
        let prefix = store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, package));
        let request = MemberRequest {
            prefix,
            name: Name::new(store.names.intern("A"), Namespace::Type),
            selector: MemberSelector::Unique,
            space: MemberSpace::Prefix,
        };
        let mark = resolver.checkpoint();
        let store_mark = store.checkpoint();
        let a = resolver
            .resolve_member(&mut store, &request)
            .unwrap()
            .unwrap();
        let b_name = BinaryName::from_internal("sample/B");
        let b = resolver.session.identified[&b_name];
        assert_eq!(store.symbols.get(b).info, SymbolInfo::Missing);
        assert_eq!(resolver.complete_class(&mut store, b), Ok(true));
        resolver.rollback_to(&mut store, mark);
        store.rollback_to(store_mark);
        assert!(!store.symbols.contains(a));
        assert!(!store.symbols.contains(b));
        assert!(resolver.session.identified.is_empty());
        assert!(resolver.session.resolved.is_empty());
        assert_eq!(
            resolver.resolve_member(&mut store, &request).unwrap(),
            Some(a)
        );
        let again = resolver.session.identified[&b_name];
        assert_eq!(resolver.complete_class(&mut store, again), Ok(true));
    }

    #[test]
    fn complete_class_handoff_is_idempotent_and_outer_rollback_restores_shell() {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let path = CompletionClassPath::new();
        let c_name = BinaryName::from_internal("sample/C");
        path.classes.lock().unwrap().extend([
            (
                BinaryName::from_internal("sample/B"),
                test_class_with_object_fields("sample/B", None, &[("c", "sample/C")]),
            ),
            (c_name.clone(), test_class_stub("sample/C", 0x0021)),
        ]);
        let mut loader = ClassLoader::with_definitions(
            path.clone(),
            &mut store,
            definitions,
            LoadingSession::new(),
        );
        let b_name = BinaryName::from_internal("sample/B");
        let b = loader.ensure_class_identity(&b_name).unwrap();
        let session = loader.into_session();
        let previous = store.symbols.get(b).clone();
        let mut resolver = ClasspathSymbolResolver::new(path.clone(), definitions, session);
        let mark = resolver.checkpoint();
        let store_mark = store.checkpoint();
        assert_eq!(resolver.complete_class(&mut store, b), Ok(true));
        assert_eq!(resolver.session.resolved.get(&b_name), Some(&b));
        let c = resolver.session.identified[&c_name];
        assert_eq!(store.symbols.get(c).info, SymbolInfo::Missing);
        let SymbolInfo::Complete(completed_info) = store.symbols.get(b).info else {
            panic!("completed B")
        };
        let completed = store.checkpoint();
        assert_eq!(resolver.complete_class(&mut store, b), Ok(true));
        assert_eq!(store.checkpoint(), completed);
        assert_eq!(path.calls.load(Ordering::Relaxed), 3);
        let package = previous.owner.unwrap();
        let scope = resolver.package_scope(package).unwrap();
        assert_eq!(store.scopes.get(scope).lookup_all(&previous.name), &[b]);
        resolver.rollback_to(&mut store, mark);
        store.rollback_to(store_mark);
        assert_eq!(store.symbols.get(b), &previous);
        assert_eq!(resolver.session.identified.get(&b_name), Some(&b));
        assert!(!resolver.session.resolved.contains_key(&b_name));
        assert!(!resolver.session.metadata.contains_key(&b));
        assert_eq!(store.checkpoint(), store_mark);
        assert!(!store.symbols.contains(c));
        assert!(!store.types.contains(completed_info));
        assert!(!resolver.session.identified.contains_key(&c_name));
        assert!(!resolver.session.origins.contains_key(&c));
        let c_text = store.names.intern("C");
        assert!(
            store
                .scopes
                .get(scope)
                .lookup_all(&Name::new(c_text, Namespace::Type))
                .is_empty()
        );
        assert_eq!(resolver.complete_class(&mut store, b), Ok(true));
        assert_eq!(path.calls.load(Ordering::Relaxed), 5);
    }

    #[test]
    fn complete_class_refuses_unowned_symbols_and_failed_completion_is_atomic() {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let path = CompletionClassPath::new();
        let name = BinaryName::from_internal("sample/B");
        let mut loader = ClassLoader::with_definitions(
            path.clone(),
            &mut store,
            definitions,
            LoadingSession::new(),
        );
        let b = loader.ensure_class_identity(&name).unwrap();
        assert_eq!(
            loader
                .ensure_class_identity(&BinaryName::from_internal("java/lang/Object"))
                .unwrap(),
            definitions.object_class
        );
        let session = loader.into_session();
        let previous = store.symbols.get(b).clone();
        let foreign = store.symbols.alloc(previous.clone());
        let mut resolver = ClasspathSymbolResolver::new(path.clone(), definitions, session);
        let mark = store.checkpoint();
        let session_mark = resolver.session.checkpoint();
        for unowned in [foreign, definitions.object_class] {
            assert_eq!(resolver.complete_class(&mut store, unowned), Ok(false));
            assert_eq!(store.checkpoint(), mark);
            assert_eq!(resolver.session.checkpoint(), session_mark);
        }
        path.classes
            .lock()
            .unwrap()
            .insert(name.clone(), vec![0, 1, 2]);
        assert!(
            matches!(resolver.complete_class(&mut store, b), Err(ResolutionError::Malformed { reason })
            if reason.contains("external class completion failed") && reason.contains("invalid class file"))
        );
        assert_eq!(store.symbols.get(b), &previous);
        assert_eq!(store.checkpoint(), mark);
        assert_eq!(resolver.session.checkpoint(), session_mark);
        path.classes
            .lock()
            .unwrap()
            .insert(name, test_class_stub("sample/B", 0x0021));
        assert_eq!(resolver.complete_class(&mut store, b), Ok(true));
    }

    #[test]
    fn complete_class_rejects_corrupt_owned_state_without_loading_or_mutation() {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let path = CompletionClassPath::new();
        let mut loader = ClassLoader::with_definitions(
            path.clone(),
            &mut store,
            definitions,
            LoadingSession::new(),
        );
        let b = loader
            .ensure_class_identity(&BinaryName::from_internal("sample/B"))
            .unwrap();
        let session = loader.into_session();
        let original = store.symbols.get(b).clone();
        let mut resolver = ClasspathSymbolResolver::new(path.clone(), definitions, session);
        for (info, expected) in [
            (SymbolInfo::Error, "Error info"),
            (
                SymbolInfo::Complete(definitions.no_prefix),
                "not a valid ClassInfo",
            ),
        ] {
            store.symbols.get_mut(b).info = info;
            let checkpoint = store.checkpoint();
            let session_checkpoint = resolver.session.checkpoint();
            assert!(
                matches!(resolver.complete_class(&mut store, b), Err(ResolutionError::Malformed { reason }) if reason.contains(expected))
            );
            assert_eq!(store.symbols.get(b).info, info);
            assert_eq!(store.checkpoint(), checkpoint);
            assert_eq!(resolver.session.checkpoint(), session_checkpoint);
        }
        *store.symbols.get_mut(b) = original.clone();
        let wrong_name = Name::new(store.names.intern("Wrong"), Namespace::Type);
        store.symbols.get_mut(b).name = wrong_name;
        assert!(
            matches!(resolver.complete_class(&mut store, b), Err(ResolutionError::Malformed { reason }) if reason.contains("canonical package/name"))
        );
        assert_eq!(store.symbols.get(b).name, wrong_name);
        *store.symbols.get_mut(b) = original;
        assert_eq!(path.calls.load(Ordering::Relaxed), 1);
        path.classes
            .lock()
            .unwrap()
            .remove(&BinaryName::from_internal("sample/B"));
        assert!(
            matches!(resolver.complete_class(&mut store, b), Err(ResolutionError::Malformed { reason }) if reason.contains("class not found: sample/B"))
        );
        assert_eq!(store.symbols.get(b).info, SymbolInfo::Missing);
        assert!(
            !resolver
                .session
                .resolved
                .contains_key(&BinaryName::from_internal("sample/B"))
        );
    }

    #[test]
    fn complete_class_rejects_invalid_owner_and_package_scope_without_panicking() {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let path = CompletionClassPath::new();
        let mut loader = ClassLoader::with_definitions(
            path.clone(),
            &mut store,
            definitions,
            LoadingSession::new(),
        );
        let b = loader
            .ensure_class_identity(&BinaryName::from_internal("sample/B"))
            .unwrap();
        let session = loader.into_session();
        let original = store.symbols.get(b).clone();
        let mut resolver = ClasspathSymbolResolver::new(path.clone(), definitions, session);
        let checkpoint = store.checkpoint();
        let invalid_owner = store.symbols.alloc(original.clone());
        store.rollback_to(checkpoint);
        store.symbols.get_mut(b).owner = Some(invalid_owner);
        assert!(
            matches!(resolver.complete_class(&mut store, b), Err(ResolutionError::Malformed { reason }) if reason.contains("canonical package/name"))
        );
        assert_eq!(store.symbols.get(b).owner, Some(invalid_owner));
        *store.symbols.get_mut(b) = original.clone();
        let scope = resolver.package_scope(original.owner.unwrap()).unwrap();
        store.scopes.get_mut(scope).owner = None;
        assert!(
            matches!(resolver.complete_class(&mut store, b), Err(ResolutionError::Malformed { reason }) if reason.contains("canonical package/name"))
        );
        assert_eq!(store.scopes.get(scope).owner, None);
        assert_eq!(store.checkpoint(), checkpoint);
        assert_eq!(path.calls.load(Ordering::Relaxed), 1);
    }

    impl ClassPathEntry for EmptyCountingClassPath {
        fn find_class(&self, _name: &BinaryName) -> Result<Option<ClassResource>, ClassPathError> {
            self.0.fetch_add(1, Ordering::Relaxed);
            Ok(None)
        }

        fn contains_package(&self, _package: &[&str]) -> Result<bool, ClassPathError> {
            self.0.fetch_add(1, Ordering::Relaxed);
            Ok(false)
        }
    }

    struct RealObjectBootstrapClassPath(JmodClassPath);

    impl ClassPathEntry for RealObjectBootstrapClassPath {
        fn find_class(&self, name: &BinaryName) -> Result<Option<ClassResource>, ClassPathError> {
            match name.as_internal() {
                "java/lang/Object" => self.0.find_class(name),
                "java/lang/Class" => Ok(Some(ClassResource::new(
                    test_class_stub_with_super("java/lang/Class", "java/lang/Object"),
                    ClassFormat::Class,
                    ClassOrigin::Directory(PathBuf::from("<bootstrap-stub>")),
                ))),
                "java/lang/String"
                | "java/lang/CloneNotSupportedException"
                | "java/lang/InterruptedException" => Ok(Some(ClassResource::new(
                    test_class_stub(name.as_internal(), 0x0021),
                    ClassFormat::Class,
                    ClassOrigin::Directory(PathBuf::from("<descriptor-stub>")),
                ))),
                _ => Ok(None),
            }
        }

        fn contains_package(&self, package: &[&str]) -> Result<bool, ClassPathError> {
            Ok(package == ["java", "lang"])
        }

        fn contains_package_prefix(&self, package: &[&str]) -> Result<bool, ClassPathError> {
            Ok(package == ["java"] || package == ["java", "lang"])
        }
    }

    struct Issue605ClassPath;

    impl ClassPathEntry for Issue605ClassPath {
        fn find_class(&self, name: &BinaryName) -> Result<Option<ClassResource>, ClassPathError> {
            let bytes = match name.as_internal() {
                "java/lang/Object" => test_class_with_object_fields(
                    "java/lang/Object",
                    None,
                    &[
                        ("class", "java/lang/Class"),
                        ("broken", "missing/NoSuchClass"),
                    ],
                ),
                "java/lang/Class" => {
                    test_class_stub_with_super("java/lang/Class", "java/lang/Object")
                }
                "cycle/A" => test_class_stub_with_super("cycle/A", "cycle/B"),
                "cycle/B" => test_class_stub_with_super("cycle/B", "cycle/A"),
                _ => return Ok(None),
            };
            Ok(Some(ClassResource::new(
                bytes,
                ClassFormat::Class,
                ClassOrigin::Directory(PathBuf::from("<issue-605-fixture>")),
            )))
        }

        fn contains_package(&self, package: &[&str]) -> Result<bool, ClassPathError> {
            Ok(package == ["java", "lang"] || package == ["cycle"])
        }
    }

    struct SyntheticTastyJdk;

    impl ClassPathEntry for SyntheticTastyJdk {
        fn find_class(&self, name: &BinaryName) -> Result<Option<ClassResource>, ClassPathError> {
            let (bytes, flags) = match name.as_internal() {
                "java/lang/Object" => (test_class_stub("java/lang/Object", 0x0021), 0x0021),
                "java/io/Closeable" => (test_class_stub("java/io/Closeable", 0x0601), 0x0601),
                _ => return Ok(None),
            };
            Ok(Some(ClassResource::new(
                bytes,
                ClassFormat::Class,
                ClassOrigin::Directory(PathBuf::from(format!("<synthetic-{flags:04x}>"))),
            )))
        }

        fn contains_package(&self, package: &[&str]) -> Result<bool, ClassPathError> {
            Ok(package == ["java", "lang"] || package == ["java", "io"])
        }
    }

    struct JdkCorpusDayOfWeek;

    impl ClassPathEntry for JdkCorpusDayOfWeek {
        fn find_class(&self, name: &BinaryName) -> Result<Option<ClassResource>, ClassPathError> {
            let (bytes, origin) = match name.as_internal() {
                "java/time/DayOfWeek" => (
                    include_bytes!("../../dotty-classfile/tests/fixtures/jdk_corpus/jdk26/java/time/DayOfWeek.class").to_vec(),
                    fixture_path("../dotty-classfile/tests/fixtures/jdk_corpus/jdk26"),
                ),
                "java/lang/Object" => (
                    test_class_stub("java/lang/Object", 0x0021),
                    PathBuf::from("<synthetic-JDK-dependency>"),
                ),
                _ => (
                    test_class_stub_with_super(name.as_internal(), "java/lang/Object"),
                    PathBuf::from("<synthetic-JDK-dependency>"),
                ),
            };
            Ok(Some(ClassResource::new(
                bytes,
                ClassFormat::Class,
                ClassOrigin::Directory(origin),
            )))
        }

        fn contains_package(&self, package: &[&str]) -> Result<bool, ClassPathError> {
            Ok(package == ["java", "time"])
        }
    }

    struct DelayedCloseableClassPath {
        inner: CompositeClassPath,
        allow_closeable: Arc<AtomicBool>,
    }

    impl ClassPathEntry for DelayedCloseableClassPath {
        fn find_class(&self, name: &BinaryName) -> Result<Option<ClassResource>, ClassPathError> {
            if name.as_internal() == "java/io/Closeable"
                && !self.allow_closeable.load(Ordering::Relaxed)
            {
                return Ok(None);
            }
            self.inner.find_class(name)
        }

        fn contains_package(&self, package: &[&str]) -> Result<bool, ClassPathError> {
            self.inner.contains_package(package)
        }
    }

    struct EnclosingGenericRetryClassPath(Mutex<bool>);

    impl ClassPathEntry for EnclosingGenericRetryClassPath {
        fn find_class(&self, name: &BinaryName) -> Result<Option<ClassResource>, ClassPathError> {
            let bytes = match name.as_internal() {
                "enclosing_generics/Outer" => {
                    include_bytes!("../tests/fixtures/enclosing_generics/Outer.class").to_vec()
                }
                "enclosing_generics/Outer$Inner" => {
                    include_bytes!("../tests/fixtures/enclosing_generics/Outer$Inner.class")
                        .to_vec()
                }
                "enclosing_generics/Outer$Middle" => {
                    include_bytes!("../tests/fixtures/enclosing_generics/Outer$Middle.class")
                        .to_vec()
                }
                "enclosing_generics/Outer$Middle$Deep" => {
                    include_bytes!("../tests/fixtures/enclosing_generics/Outer$Middle$Deep.class")
                        .to_vec()
                }
                "enclosing_generics/Outer$Middle$Deep$Leaf" => include_bytes!(
                    "../tests/fixtures/enclosing_generics/Outer$Middle$Deep$Leaf.class"
                )
                .to_vec(),
                "enclosing_generics/Outer$Static" => {
                    let mut bytes =
                        include_bytes!("../tests/fixtures/enclosing_generics/Outer$Static.class")
                            .to_vec();
                    if *self.0.lock().expect("fixture state lock") {
                        let signature = bytes
                            .windows(3)
                            .position(|window| window == b"TU;")
                            .expect("Static.value should have a TU; Signature");
                        bytes[signature..signature + 3].copy_from_slice(b"TK;");
                    }
                    bytes
                }
                "java/lang/Object" => test_class_stub("java/lang/Object", 0x0021),
                _ => return Ok(None),
            };
            Ok(Some(ClassResource::new(
                bytes,
                ClassFormat::Class,
                ClassOrigin::Directory(PathBuf::from("<enclosing-generics-fixture>")),
            )))
        }

        fn contains_package(&self, package: &[&str]) -> Result<bool, ClassPathError> {
            Ok(package == ["enclosing_generics"] || package == ["java", "lang"])
        }
    }

    #[test]
    fn failed_nested_class_load_rolls_back_and_retries_with_stable_symbols() {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let class_path = EnclosingGenericRetryClassPath(Mutex::new(true));
        let mut resolver =
            ClasspathSymbolResolver::new(class_path, definitions, LoadingSession::new());
        let package = resolver
            .resolve_package(&mut store, &["enclosing_generics"])
            .unwrap()
            .unwrap();
        let package_prefix = store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, package));
        let package_scope = resolver.session.packages.scope_of(package).unwrap();
        let store_checkpoint = store.checkpoint();
        let static_name = Name::new(store.names.intern("Outer$Static"), Namespace::Type);
        let request = MemberRequest {
            prefix: package_prefix,
            name: static_name,
            selector: MemberSelector::Unique,
            space: MemberSpace::Prefix,
        };

        let error = resolver
            .resolve_member(&mut store, &request)
            .expect_err("malformed static signature should fail after loading Outer and Inner");
        assert!(matches!(
            error,
            ResolutionError::Malformed { reason }
                if reason.contains("unresolved type variable K")
        ));
        assert_eq!(store.checkpoint(), store_checkpoint);
        for class_name in ["Outer", "Outer$Inner", "Outer$Static"] {
            assert!(
                store
                    .scopes
                    .get(package_scope)
                    .lookup_all(&Name::new(store.names.intern(class_name), Namespace::Type,))
                    .is_empty(),
                "failed transaction left {class_name} in the package scope"
            );
            assert!(
                !resolver
                    .session
                    .resolved
                    .contains_key(&BinaryName::from_internal(format!(
                        "enclosing_generics/{class_name}"
                    ),)),
                "failed transaction left {class_name} in the positive cache"
            );
        }

        *resolver.class_path.as_ref().unwrap().0.lock().unwrap() = false;
        let static_class = resolver
            .resolve_member(&mut store, &request)
            .expect("retry should succeed after fixing the classfile bytes")
            .expect("Outer$Static should resolve");
        let again = resolver
            .resolve_member(&mut store, &request)
            .unwrap()
            .unwrap();
        assert_eq!(static_class, again);
        let outer_name = BinaryName::from_internal("enclosing_generics/Outer");
        let outer = resolver.session.identified[&outer_name];
        assert_eq!(store.symbols.get(outer).info, SymbolInfo::Missing);
        assert!(!resolver.session.resolved.contains_key(&outer_name));
    }

    #[test]
    fn repeated_package_lookup_preserves_deeply_nested_class_identity() {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let mut resolver = ClasspathSymbolResolver::new(
            EnclosingGenericRetryClassPath(Mutex::new(false)),
            definitions,
            LoadingSession::new(),
        )
        .with_load_profile_collection();
        let package = resolver
            .resolve_package(&mut store, &["enclosing_generics"])
            .unwrap()
            .unwrap();
        let package_prefix = store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, package));
        let request = MemberRequest {
            prefix: package_prefix,
            name: Name::new(
                store.names.intern("Outer$Middle$Deep$Leaf"),
                Namespace::Type,
            ),
            selector: MemberSelector::Unique,
            space: MemberSpace::Prefix,
        };

        let first = resolver
            .resolve_member(&mut store, &request)
            .expect("the first request should load the nested class and its outers")
            .expect("the fixture contains the nested class");
        let load_profiles = resolver.take_load_profiles();
        assert_eq!(load_profiles.len(), 1);
        assert_eq!(
            load_profiles[0].root,
            "enclosing_generics/Outer$Middle$Deep$Leaf"
        );
        assert!(load_profiles[0].max_dependency_depth > 0);
        for class in [
            "enclosing_generics/Outer",
            "enclosing_generics/Outer$Middle",
            "enclosing_generics/Outer$Middle$Deep",
            "enclosing_generics/Outer$Middle$Deep$Leaf",
        ] {
            assert!(
                load_profiles[0].classes.contains(class),
                "dependency profile should include {class}"
            );
        }
        let second = resolver
            .resolve_member(&mut store, &request)
            .expect("the repeated request should pass package owner validation")
            .expect("the class should remain in the package's binary-name index");

        assert_eq!(first, second);
        assert!(resolver.take_load_profiles().is_empty());
        let package_scope = resolver.session.packages.scope_of(package).unwrap();
        assert_eq!(
            store.scopes.get(package_scope).lookup_all(&request.name),
            &[first]
        );
    }

    fn test_class_stub(binary_name: &str, access_flags: u16) -> Vec<u8> {
        let name = binary_name.as_bytes();
        let mut bytes = vec![0xCA, 0xFE, 0xBA, 0xBE, 0, 0, 0, 61, 0, 3, 1];
        bytes.extend_from_slice(&(name.len() as u16).to_be_bytes());
        bytes.extend_from_slice(name);
        bytes.extend_from_slice(&[7, 0, 1]);
        bytes.extend_from_slice(&access_flags.to_be_bytes());
        bytes.extend_from_slice(&[0, 2, 0, 0]);
        bytes.extend_from_slice(&[0; 8]);
        bytes
    }

    fn test_class_stub_with_super(binary_name: &str, super_name: &str) -> Vec<u8> {
        let this_name = binary_name.as_bytes();
        let super_name = super_name.as_bytes();
        let mut bytes = vec![0xCA, 0xFE, 0xBA, 0xBE, 0, 0, 0, 61, 0, 5, 1];
        bytes.extend_from_slice(&(this_name.len() as u16).to_be_bytes());
        bytes.extend_from_slice(this_name);
        bytes.extend_from_slice(&[7, 0, 1, 1]);
        bytes.extend_from_slice(&(super_name.len() as u16).to_be_bytes());
        bytes.extend_from_slice(super_name);
        bytes.extend_from_slice(&[7, 0, 3]);
        bytes.extend_from_slice(&[0, 0x21, 0, 2, 0, 4]);
        bytes.extend_from_slice(&[0; 8]);
        bytes
    }

    fn test_class_with_object_fields(
        binary_name: &str,
        super_name: Option<&str>,
        fields: &[(&str, &str)],
    ) -> Vec<u8> {
        fn push_utf8(pool: &mut Vec<u8>, next_index: &mut u16, text: &str) -> u16 {
            let index = *next_index;
            pool.push(1);
            pool.extend_from_slice(&(text.len() as u16).to_be_bytes());
            pool.extend_from_slice(text.as_bytes());
            *next_index += 1;
            index
        }

        fn push_class(pool: &mut Vec<u8>, next_index: &mut u16, name_index: u16) -> u16 {
            let index = *next_index;
            pool.extend_from_slice(&[7]);
            pool.extend_from_slice(&name_index.to_be_bytes());
            *next_index += 1;
            index
        }

        let mut pool = Vec::new();
        let mut next_index = 1;
        let this_name = push_utf8(&mut pool, &mut next_index, binary_name);
        let this_class = push_class(&mut pool, &mut next_index, this_name);
        let super_class = super_name.map(|super_name| {
            let super_name = push_utf8(&mut pool, &mut next_index, super_name);
            push_class(&mut pool, &mut next_index, super_name)
        });
        let mut field_entries = Vec::new();
        for (field_name, field_type) in fields {
            let field_name = push_utf8(&mut pool, &mut next_index, field_name);
            let descriptor = push_utf8(&mut pool, &mut next_index, &format!("L{field_type};"));
            field_entries.extend_from_slice(&[0, 1]);
            field_entries.extend_from_slice(&field_name.to_be_bytes());
            field_entries.extend_from_slice(&descriptor.to_be_bytes());
            field_entries.extend_from_slice(&[0, 0]);
        }

        let mut bytes = vec![0xCA, 0xFE, 0xBA, 0xBE, 0, 0, 0, 61];
        bytes.extend_from_slice(&next_index.to_be_bytes());
        bytes.extend_from_slice(&pool);
        bytes.extend_from_slice(&[0, 0x21]);
        bytes.extend_from_slice(&this_class.to_be_bytes());
        bytes.extend_from_slice(&super_class.unwrap_or(0).to_be_bytes());
        bytes.extend_from_slice(&[0, 0]);
        bytes.extend_from_slice(&(fields.len() as u16).to_be_bytes());
        bytes.extend_from_slice(&field_entries);
        bytes.extend_from_slice(&[0, 0, 0, 0]);
        bytes
    }

    fn class_with_missing_supertype() -> Vec<u8> {
        let this_name = b"bad/Broken";
        let super_name = b"missing/Dependency";
        let mut bytes = vec![0xCA, 0xFE, 0xBA, 0xBE, 0, 0, 0, 61, 0, 5];
        bytes.push(1);
        bytes.extend_from_slice(&(this_name.len() as u16).to_be_bytes());
        bytes.extend_from_slice(this_name);
        bytes.extend_from_slice(&[7, 0, 1, 1]);
        bytes.extend_from_slice(&(super_name.len() as u16).to_be_bytes());
        bytes.extend_from_slice(super_name);
        bytes.extend_from_slice(&[7, 0, 3]);
        bytes.extend_from_slice(&[0, 0x21, 0, 2, 0, 4]);
        bytes.extend_from_slice(&[0; 8]);
        bytes
    }

    fn setup() -> (
        SemanticStore,
        Definitions,
        ClasspathSymbolResolver<FixtureClassPath>,
    ) {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let path = FixtureClassPath(
            JdkClassPath::new(fixture_path("tests/fixtures/jdk_classpath")).unwrap(),
        );
        let resolver = ClasspathSymbolResolver::new(path, definitions, LoadingSession::new());
        (store, definitions, resolver)
    }
    #[test]
    fn resolves_only_packages_with_classpath_evidence_and_reuses_their_identity() {
        let (mut store, _, mut resolver) = setup();
        let first = resolver
            .resolve_package(&mut store, &["pool"])
            .unwrap()
            .unwrap();
        let second = resolver
            .resolve_package(&mut store, &["pool"])
            .unwrap()
            .unwrap();

        assert_eq!(first, second);
        assert_eq!(store.symbols.get(first).kind, SymbolKind::Package);
        assert!(
            resolver
                .resolve_package(&mut store, &["not_a_package"])
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn descendant_only_prefixes_materialize_one_canonical_package_per_segment() {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let mut resolver = ClasspathSymbolResolver::new(
            DescendantOnlyClassPath,
            definitions,
            LoadingSession::new(),
        );

        assert!(!DescendantOnlyClassPath.contains_package(&["a"]).unwrap());
        let leaf = resolver
            .resolve_package(&mut store, &["a", "b", "c"])
            .unwrap()
            .unwrap();
        let a = resolver
            .resolve_package(&mut store, &["a"])
            .unwrap()
            .unwrap();
        let ab = resolver
            .resolve_package(&mut store, &["a", "b"])
            .unwrap()
            .unwrap();
        let again = resolver
            .resolve_package(&mut store, &["a", "b", "c"])
            .unwrap()
            .unwrap();

        assert_eq!(leaf, again);
        assert_eq!(store.symbols.get(leaf).kind, SymbolKind::Package);
        assert_eq!(store.symbols.get(ab).kind, SymbolKind::Package);
        assert_eq!(store.symbols.get(a).kind, SymbolKind::Package);
        assert_eq!(store.symbols.get(leaf).name.namespace(), Namespace::Term);
        assert_eq!(store.names.resolve(store.symbols.get(a).name.text()), "a");
        assert_eq!(store.names.resolve(store.symbols.get(ab).name.text()), "b");
        assert_eq!(
            store.names.resolve(store.symbols.get(leaf).name.text()),
            "c"
        );
        assert_eq!(store.symbols.get(ab).owner, Some(a));
        assert_eq!(store.symbols.get(leaf).owner, Some(ab));
        assert_eq!(
            store.symbols.get(a).origin,
            dotty_core::SymbolOrigin::Synthetic
        );
        assert_eq!(store.symbols.get(leaf).info, SymbolInfo::Missing);

        let root = store.symbols.get(a).owner.expect("a is owned by root");
        assert_eq!(store.symbols.get(root).owner, None);
        for (package, name) in [(root, "a"), (a, "b"), (ab, "c")] {
            let scope = resolver.session.packages.scope_of(package).unwrap();
            assert_eq!(store.scopes.get(scope).owner, Some(package));
            let name = Name::new(store.names.intern(name), Namespace::Term);
            assert_eq!(
                store.scopes.get(scope).lookup_all(&name),
                [if package == root {
                    a
                } else if package == a {
                    ab
                } else {
                    leaf
                }]
            );
        }
    }

    #[test]
    fn missing_descendant_evidence_does_not_allocate_root_or_partial_ancestors() {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let mut resolver = ClasspathSymbolResolver::new(
            DescendantOnlyClassPath,
            definitions,
            LoadingSession::new(),
        );
        let checkpoint = store.checkpoint();

        assert!(
            resolver
                .resolve_package(&mut store, &["missing", "child"])
                .unwrap()
                .is_none()
        );

        assert_eq!(store.checkpoint(), checkpoint);
        assert!(resolver.session.packages.get::<&str>(&[]).is_none());
        assert!(resolver.session.packages.get(&["missing"]).is_none());
    }

    #[test]
    fn root_and_root_marker_use_the_canonical_root_without_a_marker_child() {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let mut resolver = ClasspathSymbolResolver::new(
            DescendantOnlyClassPath,
            definitions,
            LoadingSession::new(),
        );

        let root = resolver.resolve_package(&mut store, &[]).unwrap().unwrap();
        let a = resolver
            .resolve_package(&mut store, &["_root_", "a"])
            .unwrap()
            .unwrap();

        assert_eq!(store.symbols.get(a).owner, Some(root));
        assert_eq!(store.names.resolve(store.symbols.get(root).name.text()), "");
        assert!(resolver.session.packages.get(&["_root_"]).is_none());
    }

    #[test]
    fn source_and_tasty_packages_are_reused_with_their_original_identity_and_members() {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let mut packages = dotty_core::Packages::new();
        let source = packages.enter(
            &mut store,
            dotty_core::SymbolOrigin::Source(SourceId::from_index(4)),
            &["a"],
        );
        let source_a = source[0].symbol;
        let retained_name = Name::new(store.names.intern("Retained"), Namespace::Term);
        let retained_member = store.symbols.alloc(Symbol {
            name: retained_name,
            owner: Some(source_a),
            kind: SymbolKind::Value,
            flags: dotty_core::SymbolFlags::EMPTY,
            visibility: dotty_core::Visibility::Public,
            info: SymbolInfo::Missing,
            origin: dotty_core::SymbolOrigin::Source(SourceId::from_index(4)),
            annotations: Vec::new(),
            position: None,
            links: dotty_core::SymbolLinks::default(),
        });
        store
            .scopes
            .get_mut(source[0].scope)
            .enter(retained_name, retained_member);
        let tasty_origin = dotty_core::SymbolOrigin::Tasty(store.origins.register_tasty());
        let source_and_tasty = packages.enter(&mut store, tasty_origin, &["a", "b"]);
        let tasty_b = source_and_tasty[1].symbol;
        let mut resolver = ClasspathSymbolResolver::new(
            DescendantOnlyClassPath,
            definitions,
            LoadingSession::with_packages(packages),
        );

        assert_eq!(
            resolver.resolve_package(&mut store, &["a"]),
            Ok(Some(source_a))
        );
        assert_eq!(
            resolver.resolve_package(&mut store, &["a", "b"]),
            Ok(Some(tasty_b))
        );
        assert_eq!(
            store.symbols.get(source_a).origin,
            dotty_core::SymbolOrigin::Source(SourceId::from_index(4))
        );
        assert_eq!(store.symbols.get(tasty_b).origin, tasty_origin);
        assert_eq!(
            store.scopes.get(source[0].scope).lookup_all(&retained_name),
            [retained_member]
        );
    }

    #[test]
    fn a_non_package_term_collision_is_reported_without_shadowing_the_term() {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let mut packages = dotty_core::Packages::new();
        let entered = packages.enter(
            &mut store,
            dotty_core::SymbolOrigin::Source(SourceId::from_index(5)),
            &["a"],
        );
        let package_a = entered[0].symbol;
        let package_scope = entered[0].scope;
        let name_b = Name::new(store.names.intern("b"), Namespace::Term);
        let term_b = store.symbols.alloc(Symbol {
            name: name_b,
            owner: Some(package_a),
            kind: SymbolKind::Object,
            flags: dotty_core::SymbolFlags::EMPTY,
            visibility: dotty_core::Visibility::Public,
            info: SymbolInfo::Missing,
            origin: dotty_core::SymbolOrigin::Source(SourceId::from_index(5)),
            annotations: Vec::new(),
            position: None,
            links: dotty_core::SymbolLinks::default(),
        });
        store.scopes.get_mut(package_scope).enter(name_b, term_b);
        let checkpoint = store.checkpoint();
        let mut resolver = ClasspathSymbolResolver::new(
            DescendantOnlyClassPath,
            definitions,
            LoadingSession::with_packages(packages),
        );

        assert_eq!(
            resolver.resolve_package(&mut store, &["a", "b"]),
            Err(ResolutionError::Ambiguous { candidates: 2 })
        );

        assert_eq!(store.checkpoint(), checkpoint);
        assert_eq!(
            store.scopes.get(package_scope).lookup_all(&name_b),
            [term_b]
        );
        assert!(resolver.session.packages.get(&["a", "b"]).is_none());
    }

    #[test]
    fn a_top_level_term_collision_is_checked_before_entering_a_package() {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let mut packages = dotty_core::Packages::new();
        let root =
            packages.enter::<&str>(&mut store, dotty_core::SymbolOrigin::Synthetic, &[])[0].symbol;
        let root_scope = packages.scope_of(root).unwrap();
        let name_a = Name::new(store.names.intern("a"), Namespace::Term);
        let term_a = store.symbols.alloc(Symbol {
            name: name_a,
            owner: Some(root),
            kind: SymbolKind::Object,
            flags: dotty_core::SymbolFlags::EMPTY,
            visibility: dotty_core::Visibility::Public,
            info: SymbolInfo::Missing,
            origin: dotty_core::SymbolOrigin::Source(SourceId::from_index(6)),
            annotations: Vec::new(),
            position: None,
            links: dotty_core::SymbolLinks::default(),
        });
        store.scopes.get_mut(root_scope).enter(name_a, term_a);
        let mut resolver = ClasspathSymbolResolver::new(
            DescendantOnlyClassPath,
            definitions,
            LoadingSession::with_packages(packages),
        );

        assert_eq!(
            resolver.resolve_package(&mut store, &["a"]),
            Err(ResolutionError::Ambiguous { candidates: 2 })
        );
        assert_eq!(store.scopes.get(root_scope).lookup_all(&name_a), [term_a]);
        assert!(resolver.session.packages.get(&["a"]).is_none());
    }

    #[test]
    fn an_injected_failure_after_an_ancestor_is_entered_rolls_back_atomically() {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let mut resolver = ClasspathSymbolResolver::new(
            DescendantOnlyClassPath,
            definitions,
            LoadingSession::new(),
        );
        resolver.package_failure_after_segment = Some(1);
        let store_checkpoint = store.checkpoint();
        let resolver_checkpoint = resolver.checkpoint();

        assert_eq!(
            resolver.resolve_package(&mut store, &["a", "b"]),
            Err(ResolutionError::Malformed {
                reason: "injected package-resolution failure".to_owned()
            })
        );
        assert_eq!(store.checkpoint(), store_checkpoint);
        assert!(resolver.session.packages.get::<&str>(&[]).is_none());
        assert!(resolver.session.packages.get(&["a"]).is_none());

        resolver.package_failure_after_segment = None;
        let package = resolver
            .resolve_package(&mut store, &["a", "b"])
            .unwrap()
            .unwrap();
        assert_eq!(
            store.names.resolve(store.symbols.get(package).name.text()),
            "b"
        );
        resolver.rollback_to(&mut store, resolver_checkpoint);
        store.rollback_to(store_checkpoint);
        assert!(resolver.session.packages.get::<&str>(&[]).is_none());
        assert!(resolver.session.packages.get(&["a"]).is_none());
    }

    #[test]
    fn compiler_builtin_aliases_are_scala_type_members_without_classpath_probes() {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let probes = Arc::new(AtomicUsize::new(0));
        let class_path = EmptyCountingClassPath(Arc::clone(&probes));
        let mut resolver =
            ClasspathSymbolResolver::new(class_path, definitions, LoadingSession::new());
        let scala = resolver
            .session
            .packages
            .resolve_package(&mut store, "scala");
        let scala_prefix = store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, scala));
        let scala_scope = resolver.session.packages.scope_of(scala).unwrap();
        let any_ref_name = Name::new(store.names.intern("AnyRef"), Namespace::Type);
        let noncanonical_any_ref = store.symbols.alloc(Symbol {
            name: any_ref_name,
            owner: Some(scala),
            kind: SymbolKind::Class,
            flags: dotty_core::SymbolFlags::EMPTY,
            visibility: dotty_core::Visibility::Public,
            info: SymbolInfo::Missing,
            origin: dotty_core::SymbolOrigin::Synthetic,
            annotations: Vec::new(),
            position: None,
            links: dotty_core::SymbolLinks::default(),
        });
        store
            .scopes
            .get_mut(scala_scope)
            .enter(any_ref_name, noncanonical_any_ref);

        for (name, expected) in [
            ("Any", definitions.any_class),
            ("AnyRef", definitions.object_class),
            ("Nothing", definitions.nothing_class),
        ] {
            let name = Name::new(store.names.intern(name), Namespace::Type);
            let resolved = resolver
                .resolve_member(
                    &mut store,
                    &MemberRequest {
                        prefix: scala_prefix,
                        name,
                        selector: MemberSelector::Unique,
                        space: MemberSpace::Prefix,
                    },
                )
                .unwrap();
            assert_eq!(resolved, Some(expected));
        }

        let scala_int = Name::new(store.names.intern("Int"), Namespace::Type);
        assert_eq!(
            resolver
                .resolve_member(
                    &mut store,
                    &MemberRequest {
                        prefix: scala_prefix,
                        name: scala_int,
                        selector: MemberSelector::Unique,
                        space: MemberSpace::Prefix,
                    },
                )
                .unwrap()
                .unwrap(),
            store.types.get(definitions.int).reference_symbol().unwrap()
        );
        assert_eq!(
            probes.load(Ordering::Relaxed),
            0,
            "compiler-only aliases never reach classpath I/O"
        );

        let other = resolver
            .session
            .packages
            .resolve_package(&mut store, "example");
        let other_prefix = store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, other));
        let any_ref = Name::new(store.names.intern("AnyRef"), Namespace::Type);
        assert!(
            resolver
                .resolve_member(
                    &mut store,
                    &MemberRequest {
                        prefix: other_prefix,
                        name: any_ref,
                        selector: MemberSelector::Unique,
                        space: MemberSpace::Prefix,
                    },
                )
                .unwrap()
                .is_none()
        );
        assert_eq!(
            probes.load(Ordering::Relaxed),
            1,
            "alias ownership is exactly scala.AnyRef"
        );

        let any_ref_term = Name::new(store.names.intern("AnyRef"), Namespace::Term);
        assert!(
            resolver
                .resolve_member(
                    &mut store,
                    &MemberRequest {
                        prefix: scala_prefix,
                        name: any_ref_term,
                        selector: MemberSelector::Unique,
                        space: MemberSpace::Prefix,
                    },
                )
                .unwrap()
                .is_none()
        );
        assert_eq!(
            probes.load(Ordering::Relaxed),
            1,
            "aliases occupy only the type namespace"
        );
    }

    #[test]
    fn package_type_lookup_completes_an_owned_shell_atomically() {
        for fail in [false, true] {
            let mut store = SemanticStore::new();
            let definitions = Definitions::bootstrap(&mut store);
            let path = member_class_path();
            let name = BinaryName::from_internal("sample/B");
            let mut loader = ClassLoader::with_definitions(
                path.clone(),
                &mut store,
                definitions,
                LoadingSession::new(),
            );
            let b = loader.ensure_class_identity(&name).unwrap();
            let session = loader.into_session();
            let previous = store.symbols.get(b).clone();
            let package = previous.owner.unwrap();
            let scope = session.packages.scope_of(package).unwrap();
            let prefix = store
                .types
                .alloc(Type::type_ref(definitions.no_prefix, package));
            let request = MemberRequest {
                prefix,
                name: previous.name,
                selector: MemberSelector::Unique,
                space: MemberSpace::Prefix,
            };
            let valid = path.classes.lock().unwrap()[&name].clone();
            if fail {
                // A malformed selected artifact must not destroy the existing shell.
                path.classes.lock().unwrap().insert(name.clone(), vec![0]);
            }
            let mut resolver = ClasspathSymbolResolver::new(path.clone(), definitions, session);
            let checkpoint = resolver.checkpoint();
            let mark = store.checkpoint();
            let result = resolver.resolve_member(&mut store, &request);
            if fail {
                assert!(matches!(result, Err(ResolutionError::Malformed { .. })));
            } else {
                assert_eq!(result, Ok(Some(b)));
                assert!(matches!(store.symbols.get(b).info, SymbolInfo::Complete(_)));
                resolver.rollback_to(&mut store, checkpoint);
                store.rollback_to(mark);
            }
            assert_eq!(store.checkpoint(), mark);
            assert_eq!(resolver.checkpoint(), checkpoint);
            assert_eq!(store.symbols.get(b), &previous);
            assert_eq!(resolver.session.identified.get(&name), Some(&b));
            assert!(!resolver.session.resolved.contains_key(&name));
            assert_eq!(store.scopes.get(scope).lookup_all(&previous.name), &[b]);
            path.classes.lock().unwrap().insert(name.clone(), valid);
            assert_eq!(resolver.resolve_member(&mut store, &request), Ok(Some(b)));
            let calls = path.calls.load(Ordering::Relaxed);
            assert_eq!(resolver.resolve_member(&mut store, &request), Ok(Some(b)));
            assert_eq!(path.calls.load(Ordering::Relaxed), calls);
        }
    }

    #[test]
    fn package_type_lookup_does_not_return_an_incomplete_source_class() {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let probes = Arc::new(AtomicUsize::new(0));
        let mut resolver = ClasspathSymbolResolver::new(
            EmptyCountingClassPath(Arc::clone(&probes)),
            definitions,
            LoadingSession::new(),
        );
        let package = resolver
            .session
            .packages
            .resolve_package(&mut store, "example");
        let package_scope = resolver.package_scope(package).unwrap();
        let class_name = Name::new(store.names.intern("Pending"), Namespace::Type);
        let source_class = store.symbols.alloc(Symbol {
            name: class_name,
            owner: Some(package),
            kind: SymbolKind::Class,
            flags: dotty_core::SymbolFlags::EMPTY,
            visibility: dotty_core::Visibility::Public,
            info: SymbolInfo::Missing,
            origin: dotty_core::SymbolOrigin::Source(SourceId::from_index(0)),
            annotations: Vec::new(),
            position: None,
            links: dotty_core::SymbolLinks::default(),
        });
        store
            .scopes
            .get_mut(package_scope)
            .enter(class_name, source_class);
        let package_prefix = store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, package));

        let result = resolver.resolve_member(
            &mut store,
            &MemberRequest {
                prefix: package_prefix,
                name: class_name,
                selector: MemberSelector::Unique,
                space: MemberSpace::Prefix,
            },
        );

        assert_eq!(result, Ok(None));
        assert_eq!(probes.load(Ordering::Relaxed), 0);
        assert_eq!(
            store.scopes.get(package_scope).lookup_all(&class_name),
            &[source_class],
            "the incomplete source identity remains in place without a classpath twin"
        );
        assert!(resolver.session.resolved.is_empty());
    }

    #[test]
    fn package_type_lookup_rejects_a_class_with_a_foreign_declaration_scope() {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let mut resolver = ClasspathSymbolResolver::new(
            EmptyCountingClassPath(Arc::new(AtomicUsize::new(0))),
            definitions,
            LoadingSession::new(),
        );
        let package = resolver
            .session
            .packages
            .resolve_package(&mut store, "example");
        let package_scope = resolver.package_scope(package).unwrap();
        let class_name = Name::new(store.names.intern("Malformed"), Namespace::Type);
        let foreign_scope = store.scopes.alloc(Scope::new(Some(package)));
        let class = store.symbols.alloc(Symbol {
            name: class_name,
            owner: Some(package),
            kind: SymbolKind::Class,
            flags: dotty_core::SymbolFlags::EMPTY,
            visibility: dotty_core::Visibility::Public,
            info: SymbolInfo::Missing,
            origin: dotty_core::SymbolOrigin::Source(SourceId::from_index(0)),
            annotations: Vec::new(),
            position: None,
            links: dotty_core::SymbolLinks::default(),
        });
        let class_info = store.types.alloc(Type::ClassInfo(ClassInfo {
            prefix: definitions.no_prefix,
            class,
            parents: Vec::new(),
            declarations: foreign_scope,
            self_type: None,
        }));
        store.symbols.get_mut(class).info = SymbolInfo::Complete(class_info);
        store.scopes.get_mut(package_scope).enter(class_name, class);
        let package_prefix = store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, package));

        let error = resolver
            .resolve_member(
                &mut store,
                &MemberRequest {
                    prefix: package_prefix,
                    name: class_name,
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                },
            )
            .expect_err("a class cannot publish another owner's declaration scope");

        assert!(matches!(
            error,
            ResolutionError::Malformed { reason }
                if reason.contains("invalid class identity or declaration scope")
        ));
    }

    #[test]
    fn package_type_lookup_rejects_an_unfilled_completion_slot_without_mutation() {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let mut resolver = ClasspathSymbolResolver::new(
            EmptyCountingClassPath(Arc::new(AtomicUsize::new(0))),
            definitions,
            LoadingSession::new(),
        );
        let package = resolver
            .session
            .packages
            .resolve_package(&mut store, "example");
        let package_scope = resolver.package_scope(package).unwrap();
        let class_name = Name::new(store.names.intern("Reserved"), Namespace::Type);
        let reserved_type = store.types.reserve().id();
        let class = store.symbols.alloc(Symbol {
            name: class_name,
            owner: Some(package),
            kind: SymbolKind::Class,
            flags: dotty_core::SymbolFlags::EMPTY,
            visibility: dotty_core::Visibility::Public,
            info: SymbolInfo::Complete(reserved_type),
            origin: dotty_core::SymbolOrigin::Tasty(store.origins.register_tasty()),
            annotations: Vec::new(),
            position: None,
            links: dotty_core::SymbolLinks::default(),
        });
        store.scopes.get_mut(package_scope).enter(class_name, class);
        let package_prefix = store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, package));
        let checkpoint = store.checkpoint();

        let error = resolver
            .resolve_member(
                &mut store,
                &MemberRequest {
                    prefix: package_prefix,
                    name: class_name,
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                },
            )
            .expect_err("an unfilled type reservation is malformed class metadata");

        assert!(matches!(
            error,
            ResolutionError::Malformed { reason }
                if reason.contains("invalid or unfilled completion type")
        ));
        assert_eq!(store.checkpoint(), checkpoint);
        assert!(resolver.session.resolved.is_empty());
    }

    #[test]
    fn package_type_lookup_rejects_an_external_class_owned_by_another_package() {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let mut resolver = ClasspathSymbolResolver::new(
            EmptyCountingClassPath(Arc::new(AtomicUsize::new(0))),
            definitions,
            LoadingSession::new(),
        );
        let package = resolver
            .session
            .packages
            .resolve_package(&mut store, "example");
        let package_scope = resolver.package_scope(package).unwrap();
        let foreign_package = resolver
            .session
            .packages
            .resolve_package(&mut store, "foreign");
        let class_name = Name::new(store.names.intern("WrongOwner"), Namespace::Type);
        let class = store.symbols.alloc(Symbol {
            name: class_name,
            owner: Some(foreign_package),
            kind: SymbolKind::Class,
            flags: dotty_core::SymbolFlags::EMPTY,
            visibility: dotty_core::Visibility::Public,
            info: SymbolInfo::Missing,
            origin: dotty_core::SymbolOrigin::Tasty(store.origins.register_tasty()),
            annotations: Vec::new(),
            position: None,
            links: dotty_core::SymbolLinks::default(),
        });
        let class_scope = store.scopes.alloc(Scope::new(Some(class)));
        let class_info = store.types.alloc(Type::ClassInfo(ClassInfo {
            prefix: definitions.no_prefix,
            class,
            parents: Vec::new(),
            declarations: class_scope,
            self_type: None,
        }));
        store.symbols.get_mut(class).info = SymbolInfo::Complete(class_info);
        store.scopes.get_mut(package_scope).enter(class_name, class);
        let package_prefix = store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, package));
        let checkpoint = store.checkpoint();

        let error = resolver
            .resolve_member(
                &mut store,
                &MemberRequest {
                    prefix: package_prefix,
                    name: class_name,
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                },
            )
            .expect_err("an external package entry cannot return a class from another package");

        assert!(matches!(
            error,
            ResolutionError::Malformed { reason }
                if reason.contains("canonical package/name")
        ));
        assert_eq!(store.checkpoint(), checkpoint);
        assert!(resolver.session.resolved.is_empty());
    }

    #[test]
    #[ignore = "requires JAVA_HOME pointing to a JDK with jmods/java.base.jmod"]
    fn real_jdk_object_and_get_class_materialize_through_the_resolver() {
        let java_home = std::env::var_os("JAVA_HOME")
            .expect("set JAVA_HOME to run this real-JDK classpath regression");
        let java_base = PathBuf::from(java_home).join("jmods/java.base.jmod");
        let class_path = RealObjectBootstrapClassPath(
            crate::jmod_class_path::JmodClassPath::new(java_base)
                .expect("JAVA_HOME should contain a readable java.base.jmod"),
        );
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let object_class = definitions.object_class;
        let mut resolver =
            ClasspathSymbolResolver::new(class_path, definitions, LoadingSession::new());
        let get_class = Name::new(store.names.intern("getClass"), Namespace::Term);

        let java_lang = resolver
            .resolve_package(&mut store, &["java", "lang"])
            .expect("java.lang package lookup should succeed")
            .expect("the JDK classpath contains java.lang");
        let java_lang_prefix = store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, java_lang));
        let object_name = Name::new(store.names.intern("Object"), Namespace::Type);
        let object = resolver
            .resolve_member(
                &mut store,
                &MemberRequest {
                    prefix: java_lang_prefix,
                    name: object_name,
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                },
            )
            .expect("package type lookup should load java.lang.Object")
            .expect("java.lang.Object should materialize from the real JDK");

        assert_eq!(
            object, object_class,
            "JDK Object reuses Definitions identity"
        );
        assert!(matches!(
            store.symbols.get(object).info,
            SymbolInfo::Complete(info) if matches!(store.types.get(info), Type::ClassInfo(_))
        ));
        let object_scope = resolver
            .declaration_scope(&store, object)
            .expect("the real JDK Object has a declaration scope");
        assert!(store.scopes.contains(object_scope));
        assert_eq!(
            store
                .scopes
                .get(resolver.package_scope(java_lang).unwrap())
                .lookup(&object_name),
            Some(object),
            "package and loader must expose the same canonical Object symbol"
        );
        assert_eq!(
            resolver
                .resolve_member(
                    &mut store,
                    &MemberRequest {
                        prefix: java_lang_prefix,
                        name: object_name,
                        selector: MemberSelector::Unique,
                        space: MemberSpace::Prefix,
                    },
                )
                .unwrap(),
            Some(object),
            "repeated package type requests reuse the canonical identity"
        );

        let method = resolver
            .resolve_member(
                &mut store,
                &MemberRequest {
                    prefix: definitions.object_type,
                    name: get_class,
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                },
            )
            .expect("loading java/lang/Object through the JDK classpath should succeed")
            .expect("java.lang.Object should declare getClass");

        assert_eq!(store.symbols.get(method).owner, Some(object_class));
        assert!(matches!(
            store.symbols.get(method).origin,
            dotty_core::SymbolOrigin::Classfile(_)
        ));
        let class_name = BinaryName::from_internal("java/lang/Class");
        let class_symbol = *resolver
            .session
            .identified
            .get(&class_name)
            .expect("getClass's result should establish java/lang/Class identity");
        assert_ne!(class_symbol, object_class);
        assert_eq!(store.symbols.get(class_symbol).info, SymbolInfo::Missing);
        assert!(!resolver.session.resolved.contains_key(&class_name));
        let SymbolInfo::Complete(method_type) = store.symbols.get(method).info else {
            panic!("getClass should have complete method info");
        };
        let Type::Method(method_info) = store.types.get(method_type) else {
            panic!("getClass should have a Method type");
        };
        let Type::Applied { tycon, args } = store.types.get(method_info.result) else {
            panic!("getClass's generic result should be Class[?]");
        };
        assert_eq!(
            store.types.get(*tycon).reference_symbol(),
            Some(class_symbol)
        );
        assert_eq!(args.len(), 1);
        assert!(matches!(store.types.get(args[0]), Type::Wildcard { .. }));

        // Inspecting Class's parents requires explicit completion on this session.
        let class_path = resolver.class_path.take().unwrap();
        let session = resolver.into_session();
        let mut loader =
            ClassLoader::with_definitions(class_path, &mut store, definitions, session);
        assert_eq!(loader.load_class(&class_name).unwrap(), class_symbol);
        drop(loader);
        assert!(matches!(
            store.symbols.get(class_symbol).info,
            SymbolInfo::Complete(info)
                if matches!(store.types.get(info), Type::ClassInfo(class_info)
                    if class_info.parents.iter().any(|parent| {
                        store.types.get(*parent).reference_symbol() == Some(object_class)
                    }))
        ));
    }

    #[test]
    fn resolver_rollback_removes_a_successful_dependency_of_a_failed_object_load() {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let canonical_object = store.symbols.get(definitions.object_class).clone();
        let mut resolver =
            ClasspathSymbolResolver::new(Issue605ClassPath, definitions, LoadingSession::new());
        let java_lang = resolver
            .resolve_package(&mut store, &["java", "lang"])
            .unwrap()
            .unwrap();
        let package_prefix = store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, java_lang));
        let package_scope = resolver.session.packages.scope_of(java_lang).unwrap();

        for _ in 0..2 {
            let object_name = Name::new(store.names.intern("Object"), Namespace::Type);
            let error = resolver
                .resolve_member(
                    &mut store,
                    &MemberRequest {
                        prefix: package_prefix,
                        name: object_name,
                        selector: MemberSelector::Unique,
                        space: MemberSpace::Prefix,
                    },
                )
                .expect_err("Object's second field deliberately has no classpath class");
            assert!(matches!(
                error,
                ResolutionError::Malformed { reason }
                    if reason.contains("missing/NoSuchClass")
            ));

            assert_eq!(
                store.symbols.get(definitions.object_class),
                &canonical_object
            );
            for binary_name in ["java/lang/Object", "java/lang/Class"] {
                assert!(
                    !resolver
                        .session
                        .resolved
                        .contains_key(&BinaryName::from_internal(binary_name)),
                    "failed transaction left {binary_name} in the positive cache"
                );
            }
            assert!(resolver.session.metadata.is_empty());
            for class_name in ["Object", "Class"] {
                let name = Name::new(store.names.intern(class_name), Namespace::Type);
                assert!(store.scopes.get(package_scope).lookup_all(&name).is_empty());
            }
        }
    }

    #[test]
    fn tasty_parent_failure_rolls_back_loaded_dependencies_and_retry_succeeds() {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let allow_closeable = Arc::new(AtomicBool::new(false));
        let inner = CompositeClassPath::new(vec![
            Box::new(DirectoryClassPath::new(fixture_path(
                "../dotty-tasty/tests/fixtures/scala3-library",
            ))),
            Box::new(SyntheticTastyJdk),
        ]);
        let class_path = DelayedCloseableClassPath {
            inner,
            allow_closeable: Arc::clone(&allow_closeable),
        };
        let mut resolver =
            ClasspathSymbolResolver::new(class_path, definitions, LoadingSession::new());
        let scala_io = resolver
            .resolve_package(&mut store, &["scala", "io"])
            .unwrap()
            .expect("the Scala library fixture contains scala.io");
        let package_prefix = store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, scala_io));
        let package_scope = resolver.session.packages.scope_of(scala_io).unwrap();
        let source_name = Name::new(store.names.intern("Source"), Namespace::Type);

        let error = resolver
            .resolve_member(
                &mut store,
                &MemberRequest {
                    prefix: package_prefix,
                    name: source_name,
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                },
            )
            .expect_err("Source's later Closeable parent should fail its first load");
        assert!(matches!(
            error,
            ResolutionError::Malformed { reason } if reason.contains("java/io/Closeable")
        ));
        for name in ["scala/io/Source", "scala/collection/Iterator"] {
            assert!(
                !resolver
                    .session
                    .resolved
                    .contains_key(&BinaryName::from_internal(name)),
                "failed TASTy load left {name} in the positive cache"
            );
        }
        assert!(resolver.session.metadata.is_empty());
        assert!(
            store
                .scopes
                .get(package_scope)
                .lookup_all(&source_name)
                .is_empty()
        );
        if let Some(collection) = resolver.session.packages.get(&["scala", "collection"]) {
            let collection_scope = resolver.session.packages.scope_of(collection).unwrap();
            let iterator_name = Name::new(store.names.intern("Iterator"), Namespace::Type);
            assert!(
                store
                    .scopes
                    .get(collection_scope)
                    .lookup_all(&iterator_name)
                    .is_empty()
            );
        }

        allow_closeable.store(true, Ordering::Relaxed);
        let source = resolver
            .resolve_member(
                &mut store,
                &MemberRequest {
                    prefix: package_prefix,
                    name: source_name,
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                },
            )
            .expect("retry should load Source after the later parent becomes available")
            .expect("Source should be materialized on retry");
        assert_eq!(
            store.names.resolve(store.symbols.get(source).name.text()),
            "Source"
        );
    }

    #[test]
    fn real_scala_library_package_type_materializes_class_and_method() {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let class_path = CompositeClassPath::new(vec![
            Box::new(DirectoryClassPath::new(fixture_path(
                "../dotty-tasty/tests/fixtures/scala3-library",
            ))),
            Box::new(SyntheticTastyJdk),
        ]);
        let mut resolver =
            ClasspathSymbolResolver::new(class_path, definitions, LoadingSession::new());
        let package = resolver
            .resolve_package(&mut store, &["scala", "io"])
            .expect("Scala package lookup should succeed")
            .expect("the checked-in Scala library fixture has scala.io");
        let package_prefix = store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, package));
        let source_name = Name::new(store.names.intern("Source"), Namespace::Type);
        let request = MemberRequest {
            prefix: package_prefix,
            name: source_name,
            selector: MemberSelector::Unique,
            space: MemberSpace::Prefix,
        };

        let source = resolver
            .resolve_member(&mut store, &request)
            .expect("the real Scala 3.9 Source TASTy should load")
            .expect("scala.io.Source should be available as a package type");

        assert_eq!(store.symbols.get(source).owner, Some(package));
        assert_eq!(store.symbols.get(source).name, source_name);
        assert!(matches!(
            store.symbols.get(source).kind,
            SymbolKind::Class | SymbolKind::Trait | SymbolKind::ModuleClass
        ));
        assert!(matches!(
            store.symbols.get(source).info,
            SymbolInfo::Complete(info) if matches!(store.types.get(info), Type::ClassInfo(_))
        ));
        let declarations = resolver
            .declaration_scope(&store, source)
            .expect("Source has a declaration scope");
        assert!(store.scopes.contains(declarations));
        let package_scope = resolver.package_scope(package).unwrap();
        assert_eq!(
            store.scopes.get(package_scope).lookup(&source_name),
            Some(source),
            "the package scope and resolver must share the canonical symbol"
        );
        assert_eq!(
            resolver.resolve_member(&mut store, &request).unwrap(),
            Some(source),
            "repeated package type requests must reuse the same identity"
        );

        let source_prefix = store.types.alloc(Type::ThisType { class: source });
        let get_lines = Name::new(store.names.intern("getLines"), Namespace::Term);
        let method = resolver
            .resolve_member(
                &mut store,
                &MemberRequest {
                    prefix: source_prefix,
                    name: get_lines,
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                },
            )
            .expect("Source declaration lookup should succeed")
            .expect("real scala.io.Source should expose getLines");
        assert_eq!(store.symbols.get(method).owner, Some(source));
        assert!(store.scopes.get(declarations).lookup(&get_lines).is_some());
    }

    #[test]
    fn real_jdk_and_scala_methods_resolve_directly_from_identity_shells() {
        let scala = CompositeClassPath::new(vec![
            Box::new(DirectoryClassPath::new(fixture_path(
                "../dotty-tasty/tests/fixtures/scala3-library",
            ))),
            Box::new(SyntheticTastyJdk),
        ]);
        let jdk = CompositeClassPath::new(vec![Box::new(JdkCorpusDayOfWeek)]);
        for (name, member_name, path) in [
            ("scala/io/Source", "getLines", scala),
            ("java/time/DayOfWeek", "getValue", jdk),
        ] {
            let mut store = SemanticStore::new();
            let definitions = Definitions::bootstrap(&mut store);
            let mut loader =
                ClassLoader::with_definitions(path, &mut store, definitions, LoadingSession::new());
            let class = loader
                .ensure_class_identity(&BinaryName::from_internal(name))
                .unwrap();
            let (path, session) = loader.into_parts();
            assert_eq!(store.symbols.get(class).info, SymbolInfo::Missing);
            let prefix = store.types.alloc(Type::ThisType { class });
            let request = MemberRequest {
                prefix,
                name: Name::new(store.names.intern(member_name), Namespace::Term),
                selector: MemberSelector::Unique,
                space: MemberSpace::Prefix,
            };
            let mut resolver = ClasspathSymbolResolver::new(path, definitions, session);
            let member = resolver
                .resolve_member(&mut store, &request)
                .unwrap()
                .unwrap();
            assert_eq!(store.symbols.get(member).owner, Some(class));
            assert_eq!(store.symbols.get(member).kind, SymbolKind::Method);
            let SymbolInfo::Complete(method_type) = store.symbols.get(member).info else {
                panic!("method")
            };
            let Type::Method(method_info) = store.types.get(method_type) else {
                panic!("Method")
            };
            assert!(method_info.params.is_empty());
            if name == "java/time/DayOfWeek" {
                assert_eq!(method_info.result, definitions.int);
            }
            let checkpoint = store.checkpoint();
            assert_eq!(
                resolver.resolve_member(&mut store, &request).unwrap(),
                Some(member)
            );
            assert_eq!(store.checkpoint(), checkpoint);
            if name == "java/time/DayOfWeek" {
                let constructor_name = Name::new(store.names.intern("<init>"), Namespace::Term);
                let constructor = resolver
                    .resolve_member(
                        &mut store,
                        &MemberRequest {
                            prefix,
                            name: constructor_name,
                            selector: MemberSelector::Unique,
                            space: MemberSpace::Prefix,
                        },
                    )
                    .unwrap()
                    .unwrap();
                assert_eq!(store.symbols.get(constructor).kind, SymbolKind::Constructor);
                assert_eq!(store.symbols.get(constructor).owner, Some(class));
            }
        }
    }

    #[test]
    fn module_prefix_resolves_nested_type_from_classfile_and_tasty_evidence() {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let class_path = CompositeClassPath::new(vec![
            Box::new(DirectoryClassPath::new(fixture_path(
                "tests/fixtures/tasty_visibility",
            ))),
            Box::new(SyntheticTastyJdk),
        ]);
        let mut resolver =
            ClasspathSymbolResolver::new(class_path, definitions, LoadingSession::new());
        let package = resolver
            .resolve_package(
                &mut store,
                &["me", "cytrowski", "tastyfixtures", "visibility"],
            )
            .unwrap()
            .expect("visibility package fixture should be present");
        let package_prefix = store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, package));
        let holder_name = Name::new(store.names.intern("Holder"), Namespace::Term);
        let holder = resolver
            .resolve_member(
                &mut store,
                &MemberRequest {
                    prefix: package_prefix,
                    name: holder_name,
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                },
            )
            .unwrap()
            .expect("Holder object should resolve from its TASTy");
        let module_prefix = store.types.alloc(Type::TermRef {
            prefix: package_prefix,
            target: dotty_core::TermRefTarget::Symbol(holder),
        });
        let nested_name = Name::new(store.names.intern("InProtected"), Namespace::Type);
        let nested = resolver
            .resolve_member(
                &mut store,
                &MemberRequest {
                    prefix: module_prefix,
                    name: nested_name,
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                },
            )
            .unwrap()
            .expect("nested type should resolve from classfile evidence");
        let module_class = resolver
            .session
            .module_for_object(holder)
            .expect("Holder has canonical TASTy module identity")
            .module_class;
        assert_eq!(store.symbols.get(nested).owner, Some(module_class));
        assert_eq!(
            store.symbols.get(nested).visibility,
            dotty_core::Visibility::ProtectedWithin(package)
        );
        assert!(matches!(
            store.symbols.get(nested).info,
            SymbolInfo::Complete(info)
                if matches!(store.types.get(info), Type::ClassInfo(class_info)
                    if class_info.class == nested
                        && store.scopes.get(class_info.declarations).owner == Some(nested))
        ));

        let repeated = resolver
            .resolve_member(
                &mut store,
                &MemberRequest {
                    prefix: module_prefix,
                    name: nested_name,
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                },
            )
            .unwrap();
        assert_eq!(repeated, Some(nested));

        let mark = store.checkpoint();
        let absent_name = Name::new(store.names.intern("MissingNestedType"), Namespace::Type);
        assert_eq!(
            resolver
                .resolve_member(
                    &mut store,
                    &MemberRequest {
                        prefix: module_prefix,
                        name: absent_name,
                        selector: MemberSelector::Unique,
                        space: MemberSpace::Prefix,
                    },
                )
                .unwrap(),
            None
        );
        assert_eq!(store.checkpoint(), mark);
    }

    #[test]
    fn identity_only_nested_class_load_uses_the_proven_module_owner() {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let class_path = CompositeClassPath::new(vec![
            Box::new(DirectoryClassPath::new(fixture_path(
                "tests/fixtures/tasty_visibility",
            ))),
            Box::new(SyntheticTastyJdk),
        ]);
        let mut loader = crate::loader::ClassLoader::with_definitions(
            class_path,
            &mut store,
            definitions,
            LoadingSession::new(),
        );
        let nested_name =
            BinaryName::from_internal("me/cytrowski/tastyfixtures/visibility/Holder$InProtected");

        let identity = loader
            .ensure_class_identity(&nested_name)
            .expect("identity-only loading should use Holder's TASTy-proven module owner");
        let module = loader
            .ensure_module_identity(&BinaryName::from_internal(
                "me/cytrowski/tastyfixtures/visibility/Holder",
            ))
            .expect("Holder should have one canonical module identity");
        let (class_path, session) = loader.into_parts();
        assert_eq!(store.symbols.get(identity).owner, Some(module.module_class));
        assert_eq!(store.symbols.get(identity).info, SymbolInfo::Missing);

        let mut loader = crate::loader::ClassLoader::with_definitions(
            class_path,
            &mut store,
            definitions,
            session,
        );
        let loaded = loader
            .load_class(&nested_name)
            .expect("the identity shell should still complete through class loading");
        drop(loader);

        assert_eq!(store.symbols.get(identity).owner, Some(module.module_class));
        assert_eq!(
            loaded, identity,
            "completion must preserve canonical identity"
        );
        let SymbolInfo::Complete(info) = store.symbols.get(loaded).info else {
            panic!("nested class should have complete class info");
        };
        let Type::ClassInfo(class_info) = store.types.get(info) else {
            panic!("nested class completion should produce ClassInfo");
        };
        assert_eq!(class_info.class, loaded);
        assert!(store.scopes.contains(class_info.declarations));
        assert_eq!(
            store.scopes.get(class_info.declarations).owner,
            Some(loaded)
        );
    }

    #[test]
    fn nested_type_visibility_updates_rollback_with_the_resolver_session() {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let class_path = CompositeClassPath::new(vec![
            Box::new(DirectoryClassPath::new(fixture_path(
                "tests/fixtures/tasty_visibility",
            ))),
            Box::new(SyntheticTastyJdk),
        ]);
        let mut resolver =
            ClasspathSymbolResolver::new(class_path, definitions, LoadingSession::new());
        let package = resolver
            .resolve_package(
                &mut store,
                &["me", "cytrowski", "tastyfixtures", "visibility"],
            )
            .unwrap()
            .unwrap();
        let package_prefix = store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, package));
        let binary_name = Name::new(store.names.intern("Holder$InProtected"), Namespace::Type);
        let preloaded = resolver
            .resolve_member(
                &mut store,
                &MemberRequest {
                    prefix: package_prefix,
                    name: binary_name,
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                },
            )
            .unwrap()
            .unwrap();
        assert_eq!(
            store.symbols.get(preloaded).visibility,
            dotty_core::Visibility::Public
        );

        let holder_name = Name::new(store.names.intern("Holder"), Namespace::Term);
        let holder = resolver
            .resolve_member(
                &mut store,
                &MemberRequest {
                    prefix: package_prefix,
                    name: holder_name,
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                },
            )
            .unwrap()
            .unwrap();
        let module_prefix = store.types.alloc(Type::TermRef {
            prefix: package_prefix,
            target: dotty_core::TermRefTarget::Symbol(holder),
        });
        let store_checkpoint = store.checkpoint();
        let resolver_checkpoint = resolver.checkpoint();
        let nested_name = Name::new(store.names.intern("InProtected"), Namespace::Type);
        let nested = resolver
            .resolve_member(
                &mut store,
                &MemberRequest {
                    prefix: module_prefix,
                    name: nested_name,
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                },
            )
            .unwrap()
            .unwrap();
        assert_eq!(nested, preloaded);
        assert_eq!(
            store.symbols.get(nested).visibility,
            dotty_core::Visibility::ProtectedWithin(package)
        );

        resolver.rollback_to(&mut store, resolver_checkpoint);
        store.rollback_to(store_checkpoint);
        assert_eq!(
            store.symbols.get(preloaded).visibility,
            dotty_core::Visibility::Public
        );
    }

    #[test]
    fn inner_classes_metadata_does_not_invent_type_declarations() {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let mut loader = ClassLoader::with_definitions(
            EnclosingGenericRetryClassPath(Mutex::new(false)),
            &mut store,
            definitions,
            LoadingSession::new(),
        );
        let outer = loader
            .ensure_class_identity(&BinaryName::from_internal("enclosing_generics/Outer"))
            .unwrap();
        let (path, session) = loader.into_parts();
        let prefix = store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, outer));
        let k_name = Name::new(store.names.intern("K"), Namespace::Type);
        let inner_name = Name::new(store.names.intern("Inner"), Namespace::Type);
        let mut resolver = ClasspathSymbolResolver::new(path, definitions, session);
        let k = resolver
            .resolve_member(
                &mut store,
                &MemberRequest {
                    prefix,
                    name: k_name,
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                },
            )
            .unwrap()
            .unwrap();
        assert_eq!(store.symbols.get(k).kind, SymbolKind::TypeParameter);
        assert!(!resolver.session.metadata[&outer].inner_classes.is_empty());
        let mark = store.checkpoint();
        assert_eq!(
            resolver
                .resolve_member(
                    &mut store,
                    &MemberRequest {
                        prefix,
                        name: inner_name,
                        selector: MemberSelector::Unique,
                        space: MemberSpace::Prefix
                    }
                )
                .unwrap(),
            None
        );
        assert_eq!(store.checkpoint(), mark);
        let path = resolver.class_path.take().unwrap();
        let session = resolver.into_session();
        let mut loader = ClassLoader::with_definitions(path, &mut store, definitions, session);
        let binary = BinaryName::from_internal("enclosing_generics/Outer$Inner");
        let identity = loader.ensure_class_identity(&binary).unwrap();
        assert_eq!(loader.load_class(&binary).unwrap(), identity);
    }

    #[test]
    fn real_jdk_package_type_materializes_from_checked_in_classfile() {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let mut resolver =
            ClasspathSymbolResolver::new(JdkCorpusDayOfWeek, definitions, LoadingSession::new());
        let package = resolver
            .resolve_package(&mut store, &["java", "time"])
            .expect("java.time package lookup should succeed")
            .expect("the checked-in JDK fixture contains java.time");
        let package_prefix = store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, package));
        let name = Name::new(store.names.intern("DayOfWeek"), Namespace::Type);
        let request = MemberRequest {
            prefix: package_prefix,
            name,
            selector: MemberSelector::Unique,
            space: MemberSpace::Prefix,
        };

        let class = resolver
            .resolve_member(&mut store, &request)
            .expect("the JDK 26 classfile should load")
            .expect("java.time.DayOfWeek should materialize from the JDK classfile");

        assert_eq!(store.symbols.get(class).owner, Some(package));
        assert_eq!(store.symbols.get(class).name, name);
        assert_eq!(store.symbols.get(class).kind, SymbolKind::Class);
        assert_eq!(
            resolver.session.origins.get(&class),
            Some(&ClassOrigin::Directory(fixture_path(
                "../dotty-classfile/tests/fixtures/jdk_corpus/jdk26"
            ))),
            "the real class keeps the checked-in JDK fixture as its origin"
        );
        assert!(matches!(
            store.symbols.get(class).info,
            SymbolInfo::Complete(info) if matches!(store.types.get(info), Type::ClassInfo(_))
        ));
        let declarations = resolver
            .declaration_scope(&store, class)
            .expect("DayOfWeek has a declaration scope");
        assert!(store.scopes.contains(declarations));
        assert_eq!(
            resolver.resolve_member(&mut store, &request).unwrap(),
            Some(class),
            "repeated requests reuse the loaded JDK class identity"
        );

        let is_weekend = Name::new(store.names.intern("getValue"), Namespace::Term);
        let class_prefix = store.types.alloc(Type::ThisType { class });
        let method = resolver
            .resolve_member(
                &mut store,
                &MemberRequest {
                    prefix: class_prefix,
                    name: is_weekend,
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                },
            )
            .expect("DayOfWeek member lookup should succeed")
            .expect("the real JDK class declares getValue");
        assert_eq!(store.symbols.get(method).owner, Some(class));
    }

    #[test]
    fn scala_package_type_alias_resolves_to_its_validated_target() {
        struct ScalaPackageAliasPath;

        impl ClassPathEntry for ScalaPackageAliasPath {
            fn find_class(
                &self,
                name: &BinaryName,
            ) -> Result<Option<ClassResource>, ClassPathError> {
                let resource = match name.as_internal() {
                    "java/lang/Object" => Some(ClassResource::new(
                        test_class_stub("java/lang/Object", 0x0021),
                        ClassFormat::Class,
                        ClassOrigin::Directory(PathBuf::from("<alias-test>")),
                    )),
                    "scala/package" => Some(ClassResource::new(
                        std::fs::read(fixture_path(
                            "../dotty-tasty/tests/fixtures/scala3-library/scala/package.tasty",
                        ))
                        .expect("Scala package TASTy fixture should exist"),
                        ClassFormat::Tasty,
                        ClassOrigin::Directory(PathBuf::from("<alias-test>")),
                    )),
                    "scala/collection/Iterator" => Some(ClassResource::new(
                        test_class_stub("scala/collection/Iterator", 0x0601),
                        ClassFormat::Class,
                        ClassOrigin::Directory(PathBuf::from("<alias-test>")),
                    )),
                    _ => None,
                };
                Ok(resource)
            }

            fn contains_package(&self, package: &[&str]) -> Result<bool, ClassPathError> {
                Ok(package == ["scala"] || package == ["scala", "collection"])
            }

            fn contains_package_prefix(&self, package: &[&str]) -> Result<bool, ClassPathError> {
                Ok([&["scala"][..], &["scala", "collection"][..]]
                    .iter()
                    .any(|candidate| candidate.starts_with(package)))
            }
        }

        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let mut resolver =
            ClasspathSymbolResolver::new(ScalaPackageAliasPath, definitions, LoadingSession::new());
        let package = resolver
            .resolve_package(&mut store, &["scala"])
            .unwrap()
            .expect("the fixture contains scala");
        let prefix = store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, package));
        let name = Name::new(store.names.intern("Iterator"), Namespace::Type);

        let resolved = resolver
            .resolve_member(
                &mut store,
                &MemberRequest {
                    prefix,
                    name,
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                },
            )
            .unwrap()
            .expect("scala.Iterator resolves through the package-object alias");

        assert_eq!(store.symbols.get(resolved).kind, SymbolKind::Trait);
        assert_eq!(
            store.names.resolve(store.symbols.get(resolved).name.text()),
            "Iterator"
        );
        assert_eq!(
            store.names.resolve(
                store
                    .symbols
                    .get(store.symbols.get(resolved).owner.unwrap())
                    .name
                    .text()
            ),
            "collection"
        );
    }

    #[test]
    fn resolver_rollback_does_not_cache_a_genuine_inheritance_cycle() {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let object_before = store.symbols.get(definitions.object_class).clone();
        let mut resolver =
            ClasspathSymbolResolver::new(Issue605ClassPath, definitions, LoadingSession::new());
        let cycle_package = resolver
            .resolve_package(&mut store, &["cycle"])
            .unwrap()
            .unwrap();
        let package_prefix = store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, cycle_package));

        let class_a_name = Name::new(store.names.intern("A"), Namespace::Type);
        let error = resolver
            .resolve_member(
                &mut store,
                &MemberRequest {
                    prefix: package_prefix,
                    name: class_a_name,
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                },
            )
            .expect_err("A extends B, B extends A must remain an error");

        assert!(matches!(
            error,
            ResolutionError::Malformed { reason }
                if reason.contains("circular inheritance involving cycle/A")
        ));
        assert_eq!(store.symbols.get(definitions.object_class), &object_before);
        for binary_name in ["cycle/A", "cycle/B"] {
            assert!(
                !resolver
                    .session
                    .resolved
                    .contains_key(&BinaryName::from_internal(binary_name)),
                "failed cycle left {binary_name} in the positive cache"
            );
        }
        let package_scope = resolver.session.packages.scope_of(cycle_package).unwrap();
        for class_name in ["A", "B"] {
            let name = Name::new(store.names.intern(class_name), Namespace::Type);
            assert!(store.scopes.get(package_scope).lookup_all(&name).is_empty());
        }
    }

    #[test]
    fn package_and_class_prefixes_resolve_class_and_term_members() {
        let (mut store, definitions, mut resolver) = setup();
        let package = resolver
            .resolve_package(&mut store, &["pool"])
            .unwrap()
            .unwrap();
        let package_prefix = store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, package));
        let class_name = store.names.intern("PoolSample");
        let class = resolver
            .resolve_member(
                &mut store,
                &MemberRequest {
                    prefix: package_prefix,
                    name: Name::new(class_name, Namespace::Type),
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                },
            )
            .unwrap()
            .unwrap();
        let class_prefix = store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, class));
        let run_name = store.names.intern("run");
        let run = resolver
            .resolve_member(
                &mut store,
                &MemberRequest {
                    prefix: class_prefix,
                    name: Name::new(run_name, Namespace::Term),
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                },
            )
            .unwrap()
            .unwrap();
        let repeated = resolver
            .resolve_member(
                &mut store,
                &MemberRequest {
                    prefix: class_prefix,
                    name: Name::new(run_name, Namespace::Term),
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                },
            )
            .unwrap()
            .unwrap();

        assert_eq!(run, repeated);
        assert_eq!(
            store.names.resolve(store.symbols.get(run).name.text()),
            "run"
        );
    }

    #[test]
    fn applied_prefix_keeps_member_ownership_on_its_underlying_class() {
        let (mut store, definitions, mut resolver) = setup();
        let class = resolver
            .resolve_package(&mut store, &["pool"])
            .unwrap()
            .unwrap();
        let package_prefix = store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, class));
        let class_name = store.names.intern("PoolSample");
        let class_symbol = resolver
            .resolve_member(
                &mut store,
                &MemberRequest {
                    prefix: package_prefix,
                    name: Name::new(class_name, Namespace::Type),
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                },
            )
            .unwrap()
            .unwrap();
        let tycon = store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, class_symbol));
        let applied = store.types.alloc(Type::Applied {
            tycon,
            args: vec![definitions.int],
        });

        let run_name = store.names.intern("run");
        let run = resolver
            .resolve_member(
                &mut store,
                &MemberRequest {
                    prefix: applied,
                    name: Name::new(run_name, Namespace::Term),
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                },
            )
            .unwrap();
        assert!(run.is_some());
        assert!(
            matches!(store.types.get(applied), Type::Applied { args, .. } if args == &vec![definitions.int])
        );
        let this_type = store.types.alloc(Type::ThisType {
            class: class_symbol,
        });
        assert!(
            resolver
                .resolve_member(
                    &mut store,
                    &MemberRequest {
                        prefix: this_type,
                        name: Name::new(run_name, Namespace::Term),
                        selector: MemberSelector::Unique,
                        space: MemberSpace::Prefix,
                    },
                )
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn explicit_owner_is_used_without_falling_back_to_the_prefix() {
        let (mut store, definitions, mut resolver) = setup();
        let package = resolver
            .resolve_package(&mut store, &["pool"])
            .unwrap()
            .unwrap();
        let package_prefix = store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, package));
        let class_name = store.names.intern("PoolSample");
        let class = resolver
            .resolve_member(
                &mut store,
                &MemberRequest {
                    prefix: package_prefix,
                    name: Name::new(class_name, Namespace::Type),
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                },
            )
            .unwrap()
            .unwrap();
        let class_prefix = store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, class));

        let run_name = store.names.intern("run");
        let result = resolver
            .resolve_member(
                &mut store,
                &MemberRequest {
                    prefix: class_prefix,
                    name: Name::new(run_name, Namespace::Term),
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Explicit(package_prefix),
                },
            )
            .unwrap();

        assert_eq!(result, None);
    }

    #[test]
    fn resolver_rollback_discards_new_package_state_before_store_rollback() {
        let (mut store, _, mut resolver) = setup();
        let store_checkpoint = store.checkpoint();
        let resolver_checkpoint = resolver.checkpoint();
        let package = resolver
            .resolve_package(&mut store, &["pool"])
            .unwrap()
            .unwrap();

        resolver.rollback_to(&mut store, resolver_checkpoint);
        store.rollback_to(store_checkpoint);
        let resolved_again = resolver
            .resolve_package(&mut store, &["pool"])
            .unwrap()
            .unwrap();

        assert_eq!(package, resolved_again);
        assert!(store.symbols.contains(resolved_again));
    }

    #[test]
    fn term_and_type_namespaces_are_resolved_independently() {
        let (mut store, definitions, mut resolver) = setup();
        let package = resolver
            .resolve_package(&mut store, &["pool"])
            .unwrap()
            .unwrap();
        let package_scope = resolver.session.packages.scope_of(package).unwrap();
        let shared_text = store.names.intern("PoolSample");
        let term_name = Name::new(shared_text, Namespace::Term);
        let term_symbol = store.symbols.alloc(Symbol {
            name: term_name,
            owner: Some(package),
            kind: SymbolKind::Value,
            flags: dotty_core::SymbolFlags::EMPTY,
            visibility: dotty_core::Visibility::Public,
            info: SymbolInfo::Missing,
            origin: dotty_core::SymbolOrigin::Synthetic,
            annotations: Vec::new(),
            position: None,
            links: dotty_core::SymbolLinks::default(),
        });
        store
            .scopes
            .get_mut(package_scope)
            .enter(term_name, term_symbol);
        let package_prefix = store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, package));

        let type_symbol = resolver
            .resolve_member(
                &mut store,
                &MemberRequest {
                    prefix: package_prefix,
                    name: Name::new(shared_text, Namespace::Type),
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                },
            )
            .unwrap()
            .unwrap();
        let term_result = resolver
            .resolve_member(
                &mut store,
                &MemberRequest {
                    prefix: package_prefix,
                    name: term_name,
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                },
            )
            .unwrap();

        assert_ne!(type_symbol, term_symbol);
        assert_eq!(term_result, Some(term_symbol));
    }

    #[test]
    fn a_unique_overload_request_reports_ambiguity() {
        let (mut store, definitions, mut resolver) = setup();
        let package = resolver
            .resolve_package(&mut store, &["pool"])
            .unwrap()
            .unwrap();
        let package_prefix = store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, package));
        let class_name = store.names.intern("PoolSample");
        let class = resolver
            .resolve_member(
                &mut store,
                &MemberRequest {
                    prefix: package_prefix,
                    name: Name::new(class_name, Namespace::Type),
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                },
            )
            .unwrap()
            .unwrap();
        let declarations = resolver.declaration_scope(&store, class).unwrap();
        let run_text = store.names.intern("run");
        let run_name = Name::new(run_text, Namespace::Term);
        let overload = store.symbols.alloc(Symbol {
            name: run_name,
            owner: Some(class),
            kind: SymbolKind::Method,
            flags: dotty_core::SymbolFlags::EMPTY,
            visibility: dotty_core::Visibility::Public,
            info: SymbolInfo::Missing,
            origin: dotty_core::SymbolOrigin::Synthetic,
            annotations: Vec::new(),
            position: None,
            links: dotty_core::SymbolLinks::default(),
        });
        store.scopes.get_mut(declarations).enter(run_name, overload);
        let class_prefix = store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, class));

        let error = resolver
            .resolve_member(
                &mut store,
                &MemberRequest {
                    prefix: class_prefix,
                    name: run_name,
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                },
            )
            .unwrap_err();

        assert_eq!(error, ResolutionError::Ambiguous { candidates: 2 });
    }

    #[test]
    fn a_failed_class_load_leaves_no_store_or_session_changes() {
        struct BadClassPath;

        impl ClassPathEntry for BadClassPath {
            fn find_class(
                &self,
                name: &BinaryName,
            ) -> Result<Option<ClassResource>, ClassPathError> {
                if name.as_internal() == "bad/Broken" {
                    Ok(Some(ClassResource::new(
                        class_with_missing_supertype(),
                        ClassFormat::Class,
                        ClassOrigin::Directory(PathBuf::from("<bad-fixture>")),
                    )))
                } else {
                    Ok(None)
                }
            }

            fn contains_package(&self, package: &[&str]) -> Result<bool, ClassPathError> {
                Ok(package == ["bad"])
            }
        }

        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let mut resolver =
            ClasspathSymbolResolver::new(BadClassPath, definitions, LoadingSession::new());
        let package = resolver
            .resolve_package(&mut store, &["bad"])
            .unwrap()
            .unwrap();
        let prefix = store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, package));
        let broken = store.names.intern("Broken");
        let checkpoint = store.checkpoint();

        let result = resolver.resolve_member(
            &mut store,
            &MemberRequest {
                prefix,
                name: Name::new(broken, Namespace::Type),
                selector: MemberSelector::Unique,
                space: MemberSpace::Prefix,
            },
        );

        assert!(matches!(result, Err(ResolutionError::Malformed { .. })));
        assert_eq!(store.checkpoint(), checkpoint);
        let package_scope = resolver.session.packages.scope_of(package).unwrap();
        assert!(
            store
                .scopes
                .get(package_scope)
                .lookup_all(&Name::new(broken, Namespace::Type))
                .is_empty()
        );
        assert_eq!(
            resolver.resolve_package(&mut store, &["bad"]).unwrap(),
            Some(package)
        );
    }

    #[test]
    fn resolver_instances_reuse_class_and_package_identities_from_one_session() {
        let (mut store, definitions, resolver) = setup();
        let mut first = resolver;
        let package = first
            .resolve_package(&mut store, &["pool"])
            .unwrap()
            .unwrap();
        let package_prefix = store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, package));
        let class_name = store.names.intern("PoolSample");
        let class = first
            .resolve_member(
                &mut store,
                &MemberRequest {
                    prefix: package_prefix,
                    name: Name::new(class_name, Namespace::Type),
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                },
            )
            .unwrap()
            .unwrap();
        let session = first.into_session();
        let class_path = FixtureClassPath(
            JdkClassPath::new(fixture_path("tests/fixtures/jdk_classpath")).unwrap(),
        );
        let mut second = ClasspathSymbolResolver::new(class_path, definitions, session);
        let package_again = second
            .resolve_package(&mut store, &["pool"])
            .unwrap()
            .unwrap();
        let class_prefix = store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, class));
        let run_name = store.names.intern("run");
        let run = second
            .resolve_member(
                &mut store,
                &MemberRequest {
                    prefix: class_prefix,
                    name: Name::new(run_name, Namespace::Term),
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                },
            )
            .unwrap()
            .unwrap();

        assert_eq!(package, package_again);
        assert_eq!(store.symbols.get(class).owner, Some(package));
        assert_eq!(
            store.names.resolve(store.symbols.get(run).name.text()),
            "run"
        );
    }

    #[test]
    fn outer_rollback_removes_loaded_class_cache_and_package_scope_links() {
        let (mut store, definitions, mut resolver) = setup();
        let package = resolver
            .resolve_package(&mut store, &["pool"])
            .unwrap()
            .unwrap();
        let package_prefix = store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, package));
        let class_name = store.names.intern("PoolSample");
        let package_scope = resolver.session.packages.scope_of(package).unwrap();
        let object_before = store.symbols.get(definitions.object_class).clone();
        let store_checkpoint = store.checkpoint();
        let resolver_checkpoint = resolver.checkpoint();

        let class = resolver
            .resolve_member(
                &mut store,
                &MemberRequest {
                    prefix: package_prefix,
                    name: Name::new(class_name, Namespace::Type),
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                },
            )
            .unwrap()
            .unwrap();
        assert!(
            store
                .scopes
                .get(package_scope)
                .lookup_all(&Name::new(class_name, Namespace::Type))
                .contains(&class)
        );

        resolver.rollback_to(&mut store, resolver_checkpoint);
        store.rollback_to(store_checkpoint);
        assert!(
            store
                .scopes
                .get(package_scope)
                .lookup_all(&Name::new(class_name, Namespace::Type))
                .is_empty()
        );
        assert_eq!(store.symbols.get(definitions.object_class), &object_before);
        assert!(!store.symbols.contains(class));
        let reloaded = resolver
            .resolve_member(
                &mut store,
                &MemberRequest {
                    prefix: package_prefix,
                    name: Name::new(class_name, Namespace::Type),
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                },
            )
            .unwrap()
            .unwrap();
        assert_eq!(reloaded, class);
    }

    #[test]
    fn class_prefix_reuses_completed_symbol_already_in_the_shared_store() {
        let (mut store, definitions, mut resolver) = setup();
        let package = resolver
            .resolve_package(&mut store, &["pool"])
            .unwrap()
            .unwrap();
        let class_name = Name::new(store.names.intern("AlreadyThere"), Namespace::Type);
        let class = store.symbols.alloc(Symbol {
            name: class_name,
            owner: Some(package),
            kind: SymbolKind::Class,
            flags: dotty_core::SymbolFlags::EMPTY,
            visibility: dotty_core::Visibility::Public,
            info: SymbolInfo::Missing,
            origin: dotty_core::SymbolOrigin::Synthetic,
            annotations: Vec::new(),
            position: None,
            links: dotty_core::SymbolLinks::default(),
        });
        let declarations = store.scopes.alloc(Scope::new(Some(class)));
        let member_name = Name::new(store.names.intern("fromTasty"), Namespace::Term);
        let member = store.symbols.alloc(Symbol {
            name: member_name,
            owner: Some(class),
            kind: SymbolKind::Method,
            flags: dotty_core::SymbolFlags::EMPTY,
            visibility: dotty_core::Visibility::Public,
            info: SymbolInfo::Missing,
            origin: dotty_core::SymbolOrigin::Synthetic,
            annotations: Vec::new(),
            position: None,
            links: dotty_core::SymbolLinks::default(),
        });
        store
            .scopes
            .get_mut(declarations)
            .enter(member_name, member);
        let class_info = store.types.alloc(Type::ClassInfo(ClassInfo {
            prefix: definitions.no_prefix,
            class,
            parents: Vec::new(),
            declarations,
            self_type: None,
        }));
        store.symbols.get_mut(class).info = SymbolInfo::Complete(class_info);
        let prefix = store
            .types
            .alloc(Type::type_ref(definitions.no_prefix, class));
        let checkpoint = store.checkpoint();

        let resolved = resolver
            .resolve_member(
                &mut store,
                &MemberRequest {
                    prefix,
                    name: member_name,
                    selector: MemberSelector::Unique,
                    space: MemberSpace::Prefix,
                },
            )
            .unwrap();

        assert_eq!(resolved, Some(member));
        assert_eq!(store.checkpoint(), checkpoint);
        assert_eq!(store.symbols.get(member).owner, Some(class));
    }
}
