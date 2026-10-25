Warning: truncated output (original token count: 115962)
Total output lines: 11030

use crate::annotation::{AnnotationValue, SemanticAnnotation};
use crate::binary_name::BinaryName;
use crate::class_path::{ClassFormat, ClassOrigin, ClassPathEntry, ClassPathError, ClassResource};
use crate::error::{ClassLoadError, ClassLoadFailure};
use crate::field_symbol::FieldSymbol;
use crate::method_symbol::MethodSymbol;
use crate::nesting::{EnclosingMethodRef, InnerClassEntry};
use crate::record_component::RecordComponentSymbol;
use crate::repository::{ClassEntry, ClassRepository};
use crate::session::LoadingSession;
use crate::symbol::{ClassRef, ClassfileMetadata};
use crate::tasty_symbol;
use dotty_classfile::access_flags::{
    ACC_PRIVATE, ACC_PROTECTED, ACC_STATIC, ClassAccessFlags, FieldAccessFlags, MethodAccessFlags,
};
use dotty_classfile::attribute::{Annotation, Attribute, ElementValue};
use dotty_classfile::class_file::ClassFile;
use dotty_classfile::constant_pool::{
    ConstantPool, ConstantPoolEntry, ConstantPoolIndex, EntryKind, PoolRefError,
};
use dotty_classfile::descriptor::{FieldType, MethodDescriptor};
use dotty_classfile::reader::Reader;
use dotty_classfile::signature::{
    ClassSignature, ClassTypeSignature, FieldSignature, MethodSignature, ReferenceTypeSignature,
    SignatureError, TypeArgument, TypeSignature,
};
use dotty_core::{
    Annotation as CoreAnnotation, ClassInfo, Definitions, ErrorType, MethodKind, MethodParam,
    MethodType, Name, Namespace, PolyType, Scope, ScopeId, SemanticStore, Symbol, SymbolFlags,
    SymbolId, SymbolInfo, SymbolKind, SymbolLinks, SymbolOrigin, TermName, Type, TypeId, TypeName,
    TypeParam, Visibility,
};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::rc::Rc;
use std::time::Instant;

const MAX_INHERITANCE_DEPTH: usize = 256;

pub(crate) struct CachedDecodedModule {
    pub(crate) resource: ClassResource,
    pub(crate) decoded: tasty_symbol::DecodedTastyClass,
}

#[derive(Debug)]
pub(crate) enum TastyAliasResolutionError {
    ClassPath(BinaryName, ClassPathError),
    InvalidTasty(
        BinaryName,
        tasty_symbol::TastyDecodeError,
        ClassOrigin,
        ClassFormat,
    ),
}

impl TastyAliasResolutionError {
    fn into_class_load_error(
        self,
    ) -> (
        ClassLoadError,
        Option<(BinaryName, ClassOrigin, ClassFormat)>,
    ) {
        match self {
            Self::ClassPath(name, error) => (ClassLoadError::Io(name, Rc::new(error)), None),
            Self::InvalidTasty(name, error, origin, format) => (
                ClassLoadError::InvalidTastyFile(name.clone(), error),
                Some((name, origin, format)),
            ),
        }
    }
}

/// Resolves an alias in a package object's TASTy and accepts its target only
/// when exactly one plausible binary name is present on the configured
/// classpath. Bare names in alias RHS trees can be relative to the defining
/// package, so both the decoded name and that package-qualified name are
/// checked; ambiguity remains unresolved instead of choosing by guess.
pub(crate) fn resolve_tasty_alias<E: ClassPathEntry>(
    class_path: &E,
    owner: &BinaryName,
    alias: &str,
) -> Result<Option<BinaryName>, TastyAliasResolutionError> {
    let Some((target_owner, candidate)) =
        follow_tasty_alias_chain(owner.clone(), alias.to_owned(), |owner, alias| {
            resolve_tasty_alias_target(class_path, owner, alias)
        })?
    else {
        return Ok(None);
    };
    validate_alias_candidate(class_path, target_owner.package_path(), candidate)
}

/// Resolves an unqualified reference against the package and Scala/Java
/// implicit-import namespaces, but only when exactly one candidate is
/// present on the classpath.
pub(crate) fn resolve_tasty_bare_reference<E: ClassPathEntry>(
    class_path: &E,
    package: &str,
    simple_name: &str,
) -> Result<Option<BinaryName>, TastyAliasResolutionError> {
    let mut candidates = Vec::with_capacity(3);
    for candidate_package in [package, "scala", "java/lang"] {
        let internal = if candidate_package.is_empty() {
            simple_name.to_owned()
        } else {
            format!("{candidate_package}/{simple_name}")
        };
        let candidate = BinaryName::from_internal(internal);
        if !candidates.contains(&candidate) {
            candidates.push(candidate);
        }
    }

    let mut matches = Vec::new();
    for candidate in candidates {
        if class_path
            .find_class(&candidate)
            .map_err(|error| TastyAliasResolutionError::ClassPath(candidate.clone(), error))?
            .is_some()
        {
            matches.push(candidate);
            if matches.len() > 1 {
                return Ok(None);
            }
        }
    }
    Ok(matches.pop())
}

fn follow_tasty_alias_chain<E>(
    owner: BinaryName,
    alias: String,
    mut lookup: impl FnMut(&BinaryName, &str) -> Result<Option<tasty_symbol::TypeAliasTarget>, E>,
) -> Result<Option<(BinaryName, BinaryName)>, E> {
    let mut seen = HashSet::new();
    let mut owner = owner;
    let mut alias = alias;
    const MAX_PACKAGE_ALIAS_DEPTH: usize = 64;

    for _ in 0..MAX_PACKAGE_ALIAS_DEPTH {
        if !seen.insert((owner.clone(), alias.clone())) {
            return Ok(None);
        }
        match lookup(&owner, &alias)? {
            None => return Ok(None),
            Some(tasty_symbol::TypeAliasTarget::Alias {
                owner: next_owner,
                name: next_alias,
            }) => {
                owner = next_owner;
                alias = next_alias;
            }
            Some(tasty_symbol::TypeAliasTarget::Candidate(candidate)) => {
                return Ok(Some((owner, candidate)));
            }
        }
    }
    Ok(None)
}

fn resolve_tasty_alias_target<E: ClassPathEntry>(
    class_path: &E,
    owner: &BinaryName,
    alias: &str,
) -> Result<Option<tasty_symbol::TypeAliasTarget>, TastyAliasResolutionError> {
    if !matches!(owner.simple_name(), "package" | "package$") {
        return Ok(None);
    }
    let package_object = owner
        .as_internal()
        .strip_suffix('$')
        .unwrap_or(owner.as_internal());
    let package_name = BinaryName::from_internal(package_object);
    let package_tasty = class_path
        .find_class(&package_name)
        .map_err(|error| TastyAliasResolutionError::ClassPath(package_name.clone(), error))?;
    let Some(resource) = package_tasty.filter(|resource| resource.format() == ClassFormat::Tasty)
    else {
        return Ok(None);
    };
    let file = tasty_symbol::parse_classloader_tasty(resource.bytes()).map_err(|error| {
        TastyAliasResolutionError::InvalidTasty(
            package_name.clone(),
            error,
            resource.origin().clone(),
            resource.format(),
        )
    })?;
    tasty_symbol::resolve_type_alias_target(&file, alias, owner.package_path()).map_err(|error| {
        TastyAliasResolutionError::InvalidTasty(
            package_name,
            error,
            resource.origin().clone(),
            resource.format(),
        )
    })
}

pub(crate) fn resolve_tasty_term_alias<E: ClassPathEntry>(
    class_path: &E,
    owner: &BinaryName,
    alias: &str,
) -> Result<Option<BinaryName>, TastyAliasResolutionError> {
    if !matches!(owner.simple_name(), "package" | "package$") {
        return Ok(None);
    }
    let package_object = owner
        .as_internal()
        .strip_suffix('$')
        .unwrap_or(owner.as_internal());
    let package_name = BinaryName::from_internal(package_object);
    let resource = class_path
        .find_class(&package_name)
        .map_err(|error| TastyAliasResolutionError::ClassPath(package_name.clone(), error))?;
    let Some(resource) = resource.filter(|resource| resource.format() == ClassFormat::Tasty) else {
        return Ok(None);
    };
    let file = tasty_symbol::parse_classloader_tasty(resource.bytes()).map_err(|error| {
        TastyAliasResolutionError::InvalidTasty(
            package_name.clone(),
            error,
            resource.origin().clone(),
            resource.format(),
        )
    })?;
    let target = tasty_symbol::resolve_term_alias_target(&file, alias, owner.package_path())
        .map_err(|error| {
            TastyAliasResolutionError::InvalidTasty(
                package_name,
                error,
                resource.origin().clone(),
                resource.format(),
            )
        })?;
    let Some(tasty_symbol::TypeAliasTarget::Candidate(candidate)) = target else {
        return Ok(None);
    };
    validate_alias_candidate(class_path, owner.package_path(), candidate)
}

fn validate_alias_candidate<E: ClassPathEntry>(
    class_path: &E,
    package: &str,
    candidate: BinaryName,
) -> Result<Option<BinaryName>, TastyAliasResolutionError> {
    let raw = candidate.clone();
    let relative = if package.is_empty() || raw.as_internal().starts_with(&format!("{package}/")) {
        None
    } else {
        Some(BinaryName::from_internal(format!(
            "{package}/{}",
            raw.as_internal()
        )))
    };
    let mut resolved = None;
    for candidate in std::iter::once(raw).chain(relative) {
        if class_path
            .find_class(&candidate)
            .map_err(|error| TastyAliasResolutionError::ClassPath(candidate.clone(), error))?
            .is_some()
        {
            if resolved.is_some() {
                return Ok(None);
            }
            resolved = Some(candidate);
        }
    }
    Ok(resolved)
}

/// Loads `.class`/`.tasty`-backed classes from a [`ClassPathEntry`] into a
/// [`SemanticStore`], caching results in a `ClassRepository`.
///
/// Implements `dotty-core`'s "enter before complete" algorithm
/// (`docs/classloader.md` §5): a class's `SymbolId` is allocated before its
/// superclass/interfaces are resolved, so a class that (in)directly
/// references itself as a supertype is reported as
/// [`ClassLoadError::CircularInheritance`] instead of recursing forever.
///
/// JVM-specific data that doesn't belong in `dotty-core`'s canonical
/// semantic model (fields/methods, raw signatures, nesting metadata,
/// annotations — see [`ClassfileMetadata`]) lives in this loader's own
/// `SymbolId`-keyed metadata table instead.
pub struct ClassLoader<'store, E> {
    class_path: E,
    store: &'store mut SemanticStore,
    /// The positive-only, `SymbolId`-allocating state (successfully
    /// resolved classes/packages) this loader shares with every other
    /// `ClassLoader` sequentially loading against the same `store` — see
    /// [`LoadingSession`]'s own doc comment for why this must be threaded
    /// through by value rather than each loader owning its own, and for
    /// why it deliberately does not also share negative (`Failed`) or
    /// in-progress (`Loading`) state.
    session: LoadingSession,
    /// This loader's own `Loading`/`Loaded`/`Failed` cache, covering
    /// circular-inheritance detection and negative-result caching — kept
    /// loader-local (unlike `session`) because a name this loader's own
    /// `class_path` doesn't have is not evidence that a differently
    /// configured loader sharing `session` (the documented
    /// one-per-classpath-root pattern) doesn't have it either; see
    /// [`LoadingSession`]'s doc comment.
    repository: ClassRepository,
    /// Declaration scopes for classfile shells currently being loaded by
    /// this loader. `SymbolInfo` is completed only after member dependencies
    /// resolve, but nested signatures may need an enclosing class's already-
    /// entered type parameters while that enclosing class is still loading.
    in_progress_classes: HashMap<SymbolId, InProgressClass>,
    /// Resource context for a failed identity-header or TASTy alias decode.
    /// The requesting class consumes it when recording its detailed failure;
    /// successful loads discard context from tolerated best-effort failures.
    dependency_failure_context: Option<(BinaryName, ClassOrigin, ClassFormat)>,
    /// Resource context for classes whose decoding is currently in progress.
    /// Inheritance-cycle detection can fail a class before its own load
    /// returns, so its selected resource must be available to that branch.
    loading_context: HashMap<BinaryName, (ClassOrigin, ClassFormat)>,
    resource_observations: BTreeMap<BinaryName, ClassResource>,
    collect_resource_observations: bool,
    resource_observations_complete: bool,
    active_load_profile: Option<ActiveLoadProfile>,
    load_profiles: Vec<ClassLoadProfile>,
    collect_load_profiles: bool,
    /// The canonical `TypeId`/`SymbolId` every JVM primitive and
    /// `Object`/`Any`/`Nothing` reference lowers against — see
    /// [`Definitions::bootstrap`]. `definitions.no_prefix` is this
    /// loader's `Type::NoPrefix` too (see [`Self::with_definitions`]):
    /// there is no separate loader-owned copy, so every `ClassLoader`
    /// sharing one `Definitions` also agrees on prefix identity.
    definitions: Definitions,
}

/// Canonical term/type identities for a top-level Scala object.
///
/// The object term and its backing module class are distinct symbols. Their
/// common owner and source name define the relationship; `companion_class`
/// is populated only when the same TASTy proves a real class companion.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModuleIdentity {
    pub object: SymbolId,
    pub module_class: SymbolId,
    pub companion_class: Option<SymbolId>,
    pub object_name: BinaryName,
    pub module_class_name: BinaryName,
    pub nested_type_visibilities: HashMap<String, Visibility>,
}

/// Dependency work observed during one top-level class load.
///
/// `max_dependency_depth` is zero when the request only touches its root
/// class. `classes` includes the root and every distinct class reached by
/// superclass, interface, member-type, signature, or annotation identity work.
/// Depth counts eager load edges only. Counters describe work performed,
/// including work later rolled back when the root operation fails.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ClassLoadProfile {
    /// Binary name requested at the root of this load operation.
    pub root: String,
    /// Whether the root was a persistent identified shell before this operation.
    pub root_was_shell: bool,
    /// Greatest number of eager dependency-load edges traversed from the root.
    pub max_dependency_depth: usize,
    /// Sorted binary names of the root and every distinct class reached.
    pub classes: BTreeSet<String>,
    /// Classes newly completed in this request, excluding completed cache hits.
    pub full_completions: usize,
    /// Sorted classes whose full load succeeded during this request.
    pub completed_classes: BTreeSet<String>,
    /// Identities created or reused while incomplete during this request.
    pub identity_shells: BTreeSet<String>,
    /// Full-load requests satisfied by an already completed session entry.
    pub completed_cache_hits: usize,
    /// Newly allocated external identity shells (including enclosing owners).
    pub shells_created: usize,
    /// Requests reusing an incomplete external identity, including in-progress loads.
    pub shells_reused: usize,
    /// Successful member/signature/annotation references resolved without full loading.
    pub member_type_references: usize,
    /// Eager superclass/interface load requests, including cache hits and failures.
    pub hierarchy_loads: usize,
    /// Inclusive time spent querying the classpath for resources.
    pub resource_lookup_ns: u128,
    /// Number of ClassPathEntry resource lookup attempts included above.
    pub resource_lookup_calls: usize,
    /// Resource lookup attempts returning a resource.
    pub resources_found: usize,
    /// Resource lookup attempts returning no resource.
    pub resources_missing: usize,
    /// Resource lookup attempts returning an error.
    pub resource_lookup_errors: usize,
    /// Logical bytes returned by successful resource lookups.
    pub resource_bytes: usize,
    /// Time spent decoding class-file payloads; nested class loads are counted separately.
    pub classfile_decode_ns: u128,
    pub classfile_decode_calls: usize,
    /// Class-file decoder time after subtracting any explicitly timed child lookup.
    pub classfile_decode_exclusive_ns: u128,
    /// Time spent decoding TASTy payloads; nested class loads are counted separately.
    pub tasty_decode_ns: u128,
    pub tasty_decode_calls: usize,
    /// TASTy decoder time after subtracting resource accesses measured in its callbacks.
    pub tasty_decode_exclusive_ns: u128,
    /// Inclusive time in uncached loads, including decode, semantic projection, and dependencies.
    pub load_uncached_inclusive_ns: u128,
    /// Direct module completion attempts represented by this profile.
    pub module_completion_calls: usize,
    /// Direct module completions that returned a canonical completed identity.
    pub module_completion_successes: usize,
    /// Direct module completions that failed and rolled back.
    pub module_completion_errors: usize,
    /// Inclusive direct module completion time.
    pub module_completion_inclusive_ns: u128,
    /// Direct module completion time outside its explicitly timed resource/decode intervals.
    pub module_completion_residual_ns: u128,
    /// Dependency edges grouped by reason. Counts include repeated visits and cache hits.
    pub dependency_edges: Vec<ClassLoadDependencyEdge>,
}

/// Why one class load or identity lookup referenced another class.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ClassLoadDependencyReason {
    /// A superclass or interface required by the completed `ClassInfo`.
    Hierarchy,
    /// A member, signature, or annotation type that needs identity but not completion.
    MemberTypeIdentity,
    /// An enclosing class identity read from JVM nesting metadata.
    EnclosingOwnerIdentity,
    /// An enclosing class declaration scope needed for captured type parameters.
    EnclosingOwnerCompletion,
}

/// An aggregated directed edge in one profiled class-load request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClassLoadDependencyEdge {
    pub owner: String,
    pub dependency: String,
    pub reason: ClassLoadDependencyReason,
    pub requests: usize,
}

#[derive(Default)]
struct ActiveLoadProfile {
    root: String,
    root_was_shell: bool,
    depth: usize,
    max_dependency_depth: usize,
    classes: BTreeSet<String>,
    full_completions: usize,
    completed_classes: BTreeSet<String>,
    identity_shells: BTreeSet<String>,
    completed_cache_hits: usize,
    shells_created: usize,
    shells_reused: usize,
    member_type_references: usize,
    hierarchy_loads: usize,
    resource_lookup_ns: u128,
    resource_lookup_calls: usize,
    resources_found: usize,
    resources_missing: usize,
    resource_lookup_errors: usize,
    resource_bytes: usize,
    nested_resource_lookup_ns: u128,
    classfile_decode_ns: u128,
    classfile_decode_calls: usize,
    classfile_decode_exclusive_ns: u128,
    tasty_decode_ns: u128,
    tasty_decode_calls: usize,
    tasty_decode_exclusive_ns: u128,
    nested_tasty_decode_ns: u128,
    load_uncached_inclusive_ns: u128,
    module_completion_calls: usize,
    module_completion_successes: usize,
    module_completion_errors: usize,
    module_completion_inclusive_ns: u128,
    module_completion_residual_ns: u128,
    dependency_edges: BTreeMap<(ClassLoadDependencyReason, String, String), usize>,
}

/// Reconstructs a [`ClassTypeSignature`]'s binary name (JVMS §4.2.1): its
/// package plus simple name, with any `.`-qualified inner-class suffix
/// joined onto the simple name with `$` — the same convention a compiled
/// inner class's own binary name already uses (`docs/classloader.md`'s
/// nesting section). Used only to resolve which class a signature names;
/// the class's *own* binary name (as loaded) is never reconstructed from
/// this — see `resolve_inner_classes`/`InnerClasses` for that.
/// Resolves a `TypeVariable` (JVMS §4.7.9.1) while lowering a signature.
///
/// Class scopes are ordered from the declaring class outwards. A method-level
/// type parameter (`method_type_params`, `name ->` its already-allocated
/// `Type::ParamRef` into the enclosing `Type::Poly`) shadows every class
/// scope. Each class scope contains the real `SymbolKind::TypeParameter`
/// symbols entered by [`ClassLoader::enter_class_type_parameters`].
struct TypeVarEnv<'a> {
    class_scopes: Vec<ScopeId>,
    method_type_params: Option<&'a HashMap<String, TypeId>>,
}

#[derive(Clone, Copy)]
struct InProgressClass {
    declarations: ScopeId,
    captures_enclosing_type_parameters: bool,
}

fn metadata_captures_enclosing_type_parameters(metadata: &ClassfileMetadata) -> bool {
    metadata.inner_classes.iter().any(|entry| {
        matches!(&entry.inner_class, ClassRef::Unresolved(name) if name == &metadata.binary_name)
            && entry.outer_class.is_some()
            && !entry.access_flags.contains(ACC_STATIC)
    })
}

fn class_type_signature_binary_name(signature: &ClassTypeSignature) -> BinaryName {
    let mut simple_name = signature.simple_name.clone();
    for suffix in &signature.suffix {
        simple_name.push('$');
        simple_name.push_str(&suffix.name);
    }
    if signature.package.is_empty() {
        BinaryName::from_internal(simple_name)
    } else {
        BinaryName::from_internal(format!("{}/{}", signature.package.join("/"), simple_name))
    }
}

fn error_binary_name(error: &ClassLoadError) -> Option<&BinaryName> {
    match error {
        ClassLoadError::NotFound(name)
        | ClassLoadError::Io(name, _)
        | ClassLoadError::InvalidClassFile(name, _)
        | ClassLoadError::UnsupportedClassVersion(name, _)
        | ClassLoadError::MalformedReference(name, _)
        | ClassLoadError::MalformedDescriptor(name, _)
        | ClassLoadError::MalformedSignature(name, _)
        | ClassLoadError::InvalidTastyFile(name, _)
        | ClassLoadError::CircularInheritance(name)
        | ClassLoadError::InheritanceDepthExceeded(name)
        | ClassLoadError::UnresolvedTypeVariable(name, _) => Some(name),
        ClassLoadError::NameMismatch { requested, .. } => Some(requested),
        ClassLoadError::IdentityConflict { name, .. } => Some(name),
        ClassLoadError::DependencyFailure { source, .. } => error_binary_name(source),
    }
}

impl<'store, E: ClassPathEntry> ClassLoader<'store, E> {
    /// Convenience for the common case: one `ClassLoader` for the whole
    /// lifetime of `store`. Bootstraps its own [`Definitions`], which is
    /// only correct when nothing else ever bootstraps another one
    /// against the same `store` — [`Definitions::bootstrap`]'s own doc
    /// comment requires exactly one per `SemanticStore`/session, and
    /// calling this twice on the same store would silently mint two
    /// independent, non-`==` sets of `Object`/`Any`/`Nothing`/primitive
    /// `SymbolId`s in it. When several `ClassLoader`s need to share one
    /// `SemanticStore` (e.g. one per classpath root, or one per
    /// incremental recompilation unit), bootstrap [`Definitions`] once
    /// up front and use [`Self::with_definitions`] for every loader
    /// instead — and, to also share ordinary class/package `SymbolId`
    /// identity (not just the builtins), thread one [`LoadingSession`]
    /// through every loader's `with_definitions` call via
    /// [`Self::into_session`], the way this method already threads a
    /// fresh one through internally for its own single-loader use.
    pub fn new(class_path: E, store: &'store mut SemanticStore) -> Self {
        let definitions = Definitions::bootstrap(store);
        Self::with_definitions(class_path, store, definitions, LoadingSession::new())
    }

    /// Builds a `ClassLoader` against an already-bootstrapped, shared
    /// [`Definitions`] and [`LoadingSession`] — the construction path
    /// multiple `ClassLoader`s sharing one `SemanticStore` must use, so
    /// every one of them resolves `Object`/`Any`/`Nothing`/primitives (via
    /// `definitions`) and ordinary classes/packages (via `session`) to the
    /// exact same canonical `SymbolId`s/`TypeId`s instead of each minting
    /// its own (see [`Self::new`]'s doc comment). Get `session` back out
    /// via [`Self::into_session`] once this loader is done, to hand to the
    /// next one.
    pub fn with_definitions(
        class_path: E,
        store: &'store mut SemanticStore,
        definitions: Definitions,
        session: LoadingSession,
    ) -> Self {
        Self {
            class_path,
            store,
            session,
            repository: ClassRepository::new(),
            in_progress_classes: HashMap::new(),
            dependency_failure_context: None,
            loading_context: HashMap::new(),
            resource_observations: BTreeMap::new(),
            collect_resource_observations: false,
            resource_observations_complete: true,
            active_load_profile: None,
            load_profiles: Vec::new(),
            collect_load_profiles: false,
            definitions,
        }
    }

    /// This loader's canonical [`Definitions`] — cheap to read back
    /// ([`Definitions`] is `Copy`), e.g. to hand to
    /// [`Self::with_definitions`] for a second `ClassLoader` sharing this
    /// one's `store`.
    pub fn definitions(&self) -> Definitions {
        self.definitions
    }

    /// Hands this loader's [`LoadingSession`] back out, once it is done, so
    /// a second `ClassLoader` sharing the same `store` can pick up its
    /// class/package `SymbolId` identities via
    /// [`Self::with_definitions`] instead of starting a fresh, disagreeing
    /// session of its own.
    pub fn into_session(self) -> LoadingSession {
        self.session
    }

    /// Returns this loader's classpath and session when ownership needs to be
    /// threaded through a short-lived adapter operation.
    pub fn into_parts(self) -> (E, LoadingSession) {
        (self.class_path, self.session)
    }

    /// Returns and clears dependency profiles collected by this loader.
    pub(crate) fn take_load_profiles(&mut self) -> Vec<ClassLoadProfile> {
        std::mem::take(&mut self.load_profiles)
    }

    pub(crate) fn take_resource_observations(&mut self) -> BTreeMap<BinaryName, ClassResource> {
        std::mem::take(&mut self.resource_observations)
    }

    pub(crate) fn set_load_profile_collection(&mut self, enabled: bool) {
        self.collect_load_profiles = enabled;
    }

    fn begin_direct_load_profile(&mut self, name: &BinaryName) -> bool {
        if !self.collect_load_profiles || self.active_load_profile.is_some() {
            return false;
        }
        let root = name.as_internal().to_owned();
        let root_was_shell = self
            .session
            .identified
            .get(name)
            .is_some_and(|id| self.store.symbols.get(*id).info == SymbolInfo::Missing);
        self.active_load_profile = Some(ActiveLoadProfile {
            root: root.clone(),
            root_was_shell,
            identity_shells: if root_was_shell {
                BTreeSet::from([root])
            } else {
                BTreeSet::new()
            },
            ..ActiveLoadProfile::default()
        });
        true
    }

    fn finish_direct_load_profile(&mut self, owns_profile: bool) {
        if owns_profile {
            self.finish_load_profile();
        }
    }

    fn finish_load_profile(&mut self) {
        let Some(profile) = self.active_load_profile.take() else {
            return;
        };
        self.load_profiles.push(ClassLoadProfile {
            root: profile.root,
            root_was_shell: profile.root_was_shell,
            max_dependency_depth: profile.max_dependency_depth,
            classes: profile.classes,
            full_completions: profile.full_completions,
            completed_classes: profile.completed_classes,
            identity_shells: profile.identity_shells,
            completed_cache_hits: profile.completed_cache_hits,
            shells_created: profile.shells_created,
            shells_reused: profile.shells_reused,
            member_type_references: profile.member_type_references,
            hierarchy_loads: profile.hierarchy_loads,
            resource_lookup_ns: profile.resource_lookup_ns,
            resource_lookup_calls: profile.resource_lookup_calls,
            resources_found: profile.resources_found,
            resources_missing: profile.resources_missing,
            resource_lookup_errors: profile.resource_lookup_errors,
            resource_bytes: profile.resource_bytes,
            classfile_decode_ns: profile.classfile_decode_ns,
            classfile_decode_calls: profile.classfile_decode_calls,
            classfile_decode_exclusive_ns: profile.classfile_decode_exclusive_ns,
            tasty_decode_ns: profile.tasty_decode_ns,
            tasty_decode_calls: profile.tasty_decode_calls,
            tasty_decode_exclusive_ns: profile.tasty_decode_exclusive_ns,
            load_uncached_inclusive_ns: profile.load_uncached_inclusive_ns,
            module_completion_calls: profile.module_completion_calls,
            module_completion_successes: profile.module_completion_successes,
            module_completion_errors: profile.module_completion_errors,
            module_completion_inclusive_ns: profile.module_completion_inclusive_ns,
            module_completion_residual_ns: profile.module_completion_residual_ns,
            dependency_edges: profile
                .dependency_edges
                .into_iter()
                .map(
                    |((reason, owner, dependency), requests)| ClassLoadDependencyEdge {
                        owner,
                        dependency,
                        reason,
                        requests,
                    },
                )
                .collect(),
        });
    }

    fn record_dependency_edge(
        &mut self,
        owner: &BinaryName,
        dependency: &BinaryName,
        reason: ClassLoadDependencyReason,
    ) {
        if !self.collect_load_profiles {
            return;
        }
        if let Some(profile) = &mut self.active_load_profile {
            *profile
                .dependency_edges
                .entry((
                    reason,
                    owner.as_internal().to_owned(),
                    dependency.as_internal().to_owned(),
                ))
                .or_default() += 1;
        }
    }

    fn record_resource_lookup<LookupError>(
        &mut self,
        started: Option<Instant>,
        result: &Result<Option<ClassResource>, LookupError>,
    ) {
        let (Some(started), Some(profile)) = (started, self.active_load_profile.as_mut()) else {
            return;
        };
        let elapsed_ns = started.elapsed().as_nanos();
        profile.resource_lookup_ns += elapsed_ns;
        profile.resource_lookup_calls += 1;
        if profile.depth > 0 {
            profile.nested_resource_lookup_ns += elapsed_ns;
        }
        match result {
            Ok(Some(resource)) => {
                profile.resources_found += 1;
                profile.resource_bytes += resource.bytes().len();
            }
            Ok(None) => profile.resources_missing += 1,
            Err(_) => profile.resource_lookup_errors += 1,
        }
    }

    fn resource_lookup_total_ns(&self) -> u128 {
        self.active_load_profile
            .as_ref()
            .map_or(0, |profile| profile.resource_lookup_ns)
    }

    fn record_classfile_decode(&mut self, started: Option<Instant>) {
        if let (Some(started), Some(profile)) = (started, self.active_load_profile.as_mut()) {
            let elapsed_ns = started.elapsed().as_nanos();
            profile.classfile_decode_ns += elapsed_ns;
            profile.classfile_decode_exclusive_ns += elapsed_ns;
            profile.classfile_decode_calls += 1;
        }
    }

    fn record_tasty_decode(&mut self, started: Option<Instant>, resource_before: u128) {
        if let (Some(started), Some(profile)) = (started, self.active_load_profile.as_mut()) {
            let elapsed_ns = started.elapsed().as_nanos();
            let resource_delta = profile.resource_lookup_ns.saturating_sub(resource_before);
            let exclusive_ns = elapsed_ns.saturating_sub(resource_delta);
            profile.tasty_decode_ns += elapsed_ns;
            profile.tasty_decode_exclusive_ns += exclusive_ns;
            profile.tasty_decode_calls += 1;
            if profile.depth > 0 {
                profile.nested_tasty_decode_ns += exclusive_ns;
            }
        }
    }

    /// JVM-specific metadata for a `.class`-backed symbol previously
    /// returned by [`Self::load_class`] — `None` for a `.tasty`-backed
    /// symbol (not yet reconstructed from `.tasty`, see
    /// [`ClassfileMetadata`]'s doc comment), or a `SymbolId` no loader
    /// sharing this `session` has produced. Session-wide, not
    /// loader-local, like [`Self::origin`] — see [`LoadingSession`]'s own
    /// doc comment for why that is safe for `SymbolId`-keyed data.
    pub fn metadata(&self, id: SymbolId) -> Option<&ClassfileMetadata> {
        self.session.metadata.get(&id)
    }

    /// Where a class's bytes were read from (a directory root/JAR/JMOD
    /// path) — `None` for a `SymbolId` no loader sharing this `session`
    /// has produced (e.g. a `Definitions` builtin, which has no backing
    /// classpath resource).
    pub fn origin(&self, id: SymbolId) -> Option<&ClassOrigin> {
        self.session.origins.get(&id)
    }

    /// Loads (or returns the cached result for) the class named `name`.
    /// Hierarchy parents are completed eagerly; member/signature references
    /// need only class identities. Failed loads roll back their allocations
    /// and session updates, preserving any identities published beforehand.
    /// The typed failure remains cached in this loader instance.
    pub fn load_class(&mut self, name: &BinaryName) -> Result<SymbolId, ClassLoadError> {
        self.load_class_detailed(name)
            .map_err(|failure| failure.error)
    }

    /// Establishes the canonical symbol identity for an external class
    /// without loading its supertypes or members. The returned symbol stays
    /// `SymbolInfo::Missing` until [`Self::load_class`] completes it.
    /// Repeated calls in this loading session return the same identity.
    ///
    /// Class files and TASTy are decoded only far enough to determine the
    /// class kind and visibility. For nested JVM classes the enclosing
    /// class identity is also established from class-file metadata; `$` in
    /// a name alone is never treated as evidence of nesting.
    pub fn ensure_class_identity(&mut self, name: &BinaryName) -> Result<SymbolId, ClassLoadError> {
        self.dependency_failure_context = None;
        let store_checkpoint = self.store.checkpoint();
        let session_checkpoint = self.session.checkpoint();
        let result = self.ensure_class_identity_inner(name, &mut Vec::new());
        if result.is_err() {
            self.session.rollback_to(self.store, session_checkpoint);
            self.store.rollback_to(store_checkpoint);
        }
        result
    }

    /// Establishes the canonical term and module-class identities for a
    /// top-level Scala object proven by its Scala 3.9 TASTy file. This only
    /// enters `SymbolInfo::Missing` shells; it does not complete members.
    /// Repeated calls in the shared loading session return the same pair.
    pub fn ensure_module_identity(
        &mut self,
        name: &BinaryName,
    ) -> Result<ModuleIdentity, ClassLoadError> {
        let store_checkpoint = self.store.checkpoint();
        let session_checkpoint = self.session.checkpoint();
        let result = self.ensure_module_identity_inner(name);
        if result.is_err() {
            self.session.rollback_to(self.store, session_checkpoint);
            self.store.rollback_to(store_checkpoint);
        }
        result
    }

    /// Completes the canonical `ModuleClass` of a top-level Scala object,
    /// publishing its parents and declared members into `ClassInfo` while
    /// retaining the identity returned by [`Self::ensure_module_identity`].
    /// Member signature references establish identities only; they do not
    /// recursively complete the referenced classes. Repeated calls in a
    /// shared loading session return the same module identity without adding
    /// another declarations scope.
    pub fn complete_module(&mut self, name: &BinaryName) -> Result<ModuleIdentity, ClassLoadError> {
        self.complete_module_with_tasty_cache(name, &mut HashMap::new())
            .0
    }

    pub(crate) fn complete_module_with_tasty_cache(
        &mut self,
        name: &BinaryName,
        decoded_cache: &mut HashMap<BinaryName, CachedDecodedModule>,
    ) -> (
        Result<ModuleIdentity, ClassLoadError>,
        bool,
        BTreeMap<BinaryName, ClassResource>,
        bool,
    ) {
        let profile_started = self.collect_load_profiles.then(Instant::now);
        let owns_profile = self.begin_direct_load_profile(name);
        let resource_before = self.resource_lookup_total_ns();
        let resource_nested_before = self
            .active_load_profile
            .as_ref()
            .map_or(0, |profile| profile.nested_resource_lookup_ns);
        let tasty_before = self
            .active_load_profile
            .as_ref()
            .map_or(0, |profile| profile.tasty_decode_exclusive_ns);
        let tasty_nested_before = self
            .active_load_profile
            .as_ref()
            .map_or(0, |profile| profile.nested_tasty_decode_ns);
        let store_checkpoint = self.store.checkpoint();
        let session_checkpoint = self.session.checkpoint();
        self.resource_observations.clear();
        self.resource_observations_complete = true;
        let was_collecting_resources = self.collect_resource_observations;
        self.collect_resource_observations = true;
        let mut decoded_cache_hit = false;
        let result = self.complete_module_inner(name, decoded_cache, &mut decoded_cache_hit);
        self.collect_resource_observations = was_collecting_resources;
        if result.is_err() {
            self.session.rollback_to(self.store, session_checkpoint);
            self.store.rollback_to(store_checkpoint);
            self.repository.discard_rolled_back_symbols(self.store);
        }
        if let (Some(started), Some(profile)) = (profile_started, self.active_load_profile.as_mut())
        {
            let elapsed_ns = started.elapsed().as_nanos();
            let resource_delta = profile.resource_lookup_ns.saturating_sub(resource_before);
            let nested_resource_delta = profile
                .nested_resource_lookup_ns
                .saturating_sub(resource_nested_before);
            let tasty_delta = profile
                .tasty_decode_exclusive_ns
                .saturating_sub(tasty_before);
            let nested_tasty_delta = profile
                .nested_tasty_decode_ns
                .saturating_sub(tasty_nested_before);
            let direct_resource_ns = resource_delta.saturating_sub(nested_resource_delta);
            let direct_tasty_ns = tasty_delta.saturating_sub(nested_tasty_delta);
            profile.module_completion_calls += 1;
            profile.module_completion_inclusive_ns += elapsed_ns;
            profile.module_completion_residual_ns +=
                elapsed_ns.saturating_sub(direct_resource_ns + direct_tasty_ns);
            if result.is_ok() {
                profile.module_completion_successes += 1;
            } else {
                profile.module_completion_errors += 1;
            }
        }
        let observations = self.take_resource_observations();
        let observations_complete = self.resource_observations_complete;
        self.finish_direct_load_profile(owns_profile);
        (
            result,
            decoded_cache_hit,
            observations,
            observations_complete,
        )
    }

    fn complete_module_inner(
        &mut self,
        name: &BinaryName,
        decoded_cache: &mut HashMap<BinaryName, CachedDecodedModule>,
        decoded_cache_hit: &mut bool,
    ) -> Result<ModuleIdentity, ClassLoadError> {
        let identity = self.ensure_module_identity_inner(name)?;
        match self.store.symbols.get(identity.module_class).info {
            SymbolInfo::Complete(info) => {
                let object_info = self.store.symbols.get(identity.object).info;
                let valid_object_type = matches!(
                    object_info,
                    SymbolInfo::Complete(object_type)
                        if matches!(self.store.types.get(object_type), Type::TypeRef { .. })
                            && self.store.types.get(object_type).reference_symbol()
                                == Some(identity.module_class)
                );
                let valid_class_info = matches!(
                    self.store.types.get(info),
                    Type::ClassInfo(class_info)
                        if class_info.class == identity.module_class
                            && self.store.scopes.contains(class_info.declarations)
                            && self.store.scopes.get(class_info.declarations).owner
                                == Some(identity.module_class)
                            && self.store.scopes.get(class_info.declarations)
                                .entered_symbols()
                                .all(|member| {
                                    self.store.symbols.contains(member)
                                        && self.store.symbols.get(member).owner
                                            == Some(identity.module_class)
                                })
                );
                if valid_object_type && valid_class_info {
                    self.session
                        .cache_resolved(identity.module_class_name.clone(), identity.module_class);
                    return Ok(identity);
                }
                return Err(ClassLoadError::IdentityConflict {
                    name: identity.module_class_name,
                    reason: "the completed module class, declarations scope, or object type is inconsistent"
                        .to_owned(),
                });
            }
            SymbolInfo::Missing
                if self.store.symbols.get(identity.object).info != SymbolInfo::Missing =>
            {
                return Err(ClassLoadError::IdentityConflict {
                    name: identity.module_class_name,
                    reason: "the object term is complete while its module class is missing"
                        .to_owned(),
                });
            }
            SymbolInfo::Error => {
                return Err(ClassLoadError::IdentityConflict {
                    name: identity.module_class_name,
                    reason: "the module class is marked as failed".to_owned(),
                });
            }
            SymbolInfo::Deferred(_) => {
                return Err(ClassLoadError::IdentityConflict {
                    name: identity.module_class_name,
                    reason: "the module class has an unrelated pending completion".to_owned(),
                });
            }
            SymbolInfo::Missing => {}
        }

        let lookup_started = self.collect_load_profiles.then(Instant::now);
        let lookup_result = self.class_path.find_class(name);
        self.record_resource_lookup(lookup_started, &lookup_result);
        let resource = lookup_result
            .map_err(|error| ClassLoadError::Io(name.clone(), Rc::new(error)))?
            .ok_or_else(|| ClassLoadError::NotFound(name.clone()))?;
        if resource.format() != ClassFormat::Tasty {
            return Err(ClassLoadError::IdentityConflict {
                name: name.clone(),
                reason: "a classfile cannot complete a Scala module class".to_owned(),
            });
        }
        if self.collect_resource_observations {
            self.resource_observations
                .insert(name.clone(), resource.clone());
        }

        let alias_error = RefCell::new(None);
        let class_path = &self.class_path;
        let resource_before_decode = self.resource_lookup_total_ns();
        let decoded = if let Some(cached) = decoded_cache.get(name)
            && cached.resource == resource
        {
            *decoded_cache_hit = true;
            cached.decoded.clone()
        } else {
            decoded_cache.remove(name);
            let decode_started = self.collect_load_profiles.then(Instant::now);
            let decoded = tasty_symbol::decode_module_class_with_resolvers(
                resource.bytes(),
                &identity.module_class_name,
                |owner, alias| match resolve_tasty_alias(class_path, owner, alias) {
                    Ok(target) => target,
                    Err(error) => {
                        alias_error.borrow_mut().get_or_insert(error);
                        None
                    }
                },
                |package, simple_name| match resolve_tasty_bare_reference(
                    class_path,
                    package,
                    simple_name,
                ) {
                    Ok(target) => target,
                    Err(error) => {
                        alias_error.borrow_mut().get_or_insert(error);
                        None
                    }
                },
            );
            self.record_tasty_decode(decode_started, resource_before_decode);
            if let Some(error) = alias_error.into_inner() {
                return Err(error.into_class_load_error().0);
            }
            let decoded = decoded.map_err(|error| {
                ClassLoadError::InvalidTastyFile(identity.module_class_name.clone(), error)
            })?;
            decoded_cache.insert(
                name.clone(),
                CachedDecodedModule {
                    resource: resource.clone(),
                    decoded: decoded.clone(),
                },
            );
            decoded
        };

        let module_class = self.store.symbols.get(identity.module_class);
        if module_class.kind != SymbolKind::ModuleClass
            || module_class.owner != self.store.symbols.get(identity.object).owner
            || module_class.info != SymbolInfo::Missing
        {
            return Err(ClassLoadError::IdentityConflict {
                name: identity.module_class_name,
                reason: "the module class shell is inconsistent or already being completed"
                    .to_owned(),
            });
        }

        let declarations = self
            .store
            .scopes
            .alloc(Scope::new(Some(identity.module_class)));
        self.session
            .record_symbol_update(self.store, identity.module_class);
        if let Some(declared) = &decoded.visibility {
            let visibility = self.tasty_visibility(&identity.module_class_name, declared);
            self.store.symbols.get_mut(identity.module_class).visibility = visibility;
        }

        let mut ancestry = vec![identity.module_class_name.clone()];
        let super_class = self.load_dependency(
            &identity.module_class_name,
            decoded.super_class,
            &mut ancestry,
        )?;
        let mut interfaces = Vec::with_capacity(decoded.interfaces.len());
        for dependency in decoded.interfaces {
            interfaces.push(self.load_dependency(
                &identity.module_class_name,
                dependency,
                &mut ancestry,
            )?);
        }

        let origin = self.store.symbols.get(identity.module_class).origin;
        for field in &decoded.fields {
            let resolved_type = self
                .lower_tasty_member_type(&identity.module_class_name, field.declared_type.as_ref());
            self.enter_field(
                identity.module_class,
                declarations,
                &field.name,
                field.flags,
                resolved_type,
                origin,
            );
        }
        for method in &decoded.methods {
            let resolved_type = self.lower_tasty_method(&identity.module_class_name, method);
            self.enter_method(
                identity.module_class,
                declarations,
                &method.name,
                method.flags,
                resolved_type,
                origin,
            );
        }

        self.complete_class(
            identity.module_class,
            declarations,
            Some(super_class),
            interfaces,
        );
        let object_type = self.type_ref(identity.module_class);
        self.session
            .record_symbol_update(self.store, identity.object);
        self.store
            .symbols
            .set_info(identity.object, SymbolInfo::Complete(object_type));
        self.session
            .cache_resolved(identity.module_class_name.clone(), identity.module_class);
        Ok(identity)
    }

    fn ensure_module_identity_inner(
        &mut self,
        name: &BinaryName,
    ) -> Result<ModuleIdentity, ClassLoadError> {
        if let Some(identity) = self.session.modules.get(name).cloned() {
            if self.module_identity_is_valid(name, &identity) {
                return Ok(identity);
            }
            return Err(ClassLoadError::IdentityConflict {
                name: name.clone(),
                reason: "the session refers to a module symbol removed from the semantic store"
                    .to_owned(),
            });
        }
        let lookup_started = self.collect_load_profiles.then(Instant::now);
        let lookup_result = self.class_path.find_class(name);
        self.record_resource_lookup(lookup_started, &lookup_result);
        let resource = lookup_result
            .map_err(|error| ClassLoadError::Io(name.clone(), Rc::new(error)))?
            .ok_or_else(|| ClassLoadError::NotFound(name.clone()))?;
        if resource.format() != ClassFormat::Tasty {
            return Err(ClassLoadError::IdentityConflict {
                name: name.clone(),
                reason: "a classfile does not prove Scala module identity".to_owned(),
            });
        }
        let resource_before = self.resource_lookup_total_ns();
        let decode_started = self.collect_load_profiles.then(Instant::now);
        let decoded_result = tasty_symbol::decode_module_identity(resource.bytes(), name);
        self.record_tasty_decode(decode_started, resource_before);
        let decoded = decoded_result
            .map_err(|error| ClassLoadError::InvalidTastyFile(name.clone(), error))?
            .ok_or_else(|| ClassLoadError::IdentityConflict {
                name: name.clone(),
                reason: "the TASTy file does not prove a top-level object and module class"
                    .to_owned(),
            })?;
        let package = self.session.packages.resolve(self.store, name);
        let object_name = Name::new(self.store.names.intern(name.simple_name()), Namespace::Term);
        let module_name = Name::new(
            self.store.names.intern(&format!("{}$", name.simple_name())),
            Namespace::Type,
        );
        let object = self.ensure_module_symbol(
            name,
            package,
            object_name,
            SymbolKind::Object,
            decoded.object_flags,
            decoded.object_visibility.as_ref(),
        )?;
        let module_class_name = BinaryName::from_internal(format!("{}$", name.as_internal()));
        let module_class = self.ensure_module_symbol(
            &module_class_name,
            package,
            module_name,
            SymbolKind::ModuleClass,
            decoded.module_class_flags,
            decoded.module_class_visibility.as_ref(),
        )?;
        self.session
            .record_origin(object, resource.origin().clone());
        self.session
            .record_origin(module_class, resource.origin().clone());
        if self
            .store
            .symbols
            .get(module_class)
            .links
            .companion
            .is_some()
            || (decoded.companion_class_kind.is_none()
                && self.store.symbols.get(object).links.companion.is_some())
        {
            return Err(ClassLoadError::IdentityConflict {
                name: name.clone(),
                reason: "an existing symbol has an unproven companion link".to_owned(),
            });
        }
        self.session
            .cache_identified(module_class_name.clone(), module_class);
        let companion_class = if decoded.companion_class_kind.is_some() {
            Some(self.ensure_class_identity_inner(name, &mut Vec::new())?)
        } else {
            None
        };
        if let Some(class) = companion_class {
            if Some(self.store.symbols.get(class).kind) != decoded.companion_class_kind {
                return Err(ClassLoadError::IdentityConflict {
                    name: name.clone(),
                    reason: "the class identity conflicts with its TASTy companion definition"
                        .to_owned(),
                });
            }
            self.link_class_object_companions(name, class, object)?;
        }
        let nested_type_visibilities = decoded
            .nested_type_visibilities
            .iter()
            .map(|(nested_name, declared)| {
                let binary_name =
                    BinaryName::from_internal(format!("{}${nested_name}", name.as_internal()));
                (
                    nested_name.clone(),
                    self.tasty_visibility(&binary_name, declared),
                )
            })
            .collect();
        let identity = ModuleIdentity {
            object,
            module_class,
            companion_class,
            object_name: name.clone(),
            module_class_name,
            nested_type_visibilities,
        };
        self.session.cache_module(name.clone(), identity.clone());
        Ok(identity)
    }

    fn module_identity_is_valid(&self, requested: &BinaryName, identity: &ModuleIdentity) -> bool {
        if identity.object == identity.module_class
            || identity.object_name != *requested
            || identity.module_class_name.as_internal() != format!("{}$", requested.as_internal())
            || !self.store.symbols.contains(identity.object)
            || !self.store.symbols.contains(identity.module_class)
        {
            return false;
        }
        let object = self.store.symbols.get(identity.object);
        let module_class = self.store.symbols.get(identity.module_class);
        object.kind == SymbolKind::Object
            && object.name.namespace() == Namespace::Term
            && self.store.names.resolve(object.name.text()) == requested.simple_name()
            && module_class.kind == SymbolKind::ModuleClass
            && module_class.name.namespace() == Namespace::Type
            && self.store.names.resolve(module_class.name.text())
                == format!("{}$", requested.simple_name())
            && object.owner.is_some()
            && object.owner == module_class.owner
    }

    fn ensure_module_symbol(
        &mut self,
        binary_name: &BinaryName,
        package: SymbolId,
        name: Name,
        kind: SymbolKind,
        flags: SymbolFlags,
        declared_visibility: Option<&tasty_symbol::DeclaredVisibility>,
    ) -> Result<SymbolId, ClassLoadError> {
        let scope = self
            .session
            .packages
            .scope_of(package)
            .expect("resolved package has a declaration scope");
        let existing = self.store.scopes.get(scope).lookup_all(&name).to_vec();
        if existing.len() > 1 {
            return Err(ClassLoadError::IdentityConflict {
                name: binary_name.clone(),
                reason: "the canonical package scope contains multiple declarations".to_owned(),
            });
        }
        if kind == SymbolKind::ModuleClass
            && self
                .session
                .identified
                .get(binary_name)
                .is_some_and(|identified| Some(identified) != existing.first())
        {
            return Err(ClassLoadError::IdentityConflict {
                name: binary_name.clone(),
                reason: "the loading session maps the module class name to another symbol"
                    .to_owned(),
            });
        }
        let visibility = if let Some(declared) = declared_visibility {
            self.tasty_visibility(binary_name, declared)
        } else {
            Visibility::Public
        };
        if let Some(&symbol) = existing.first() {
            let candidate = self.store.symbols.get(symbol);
            let can_upgrade = kind == SymbolKind::ModuleClass
                && candidate.kind == SymbolKind::Class
                && candidate.info == SymbolInfo::Missing
                && matches!(
                    candidate.origin,
                    SymbolOrigin::Classfile(_) | SymbolOrigin::Tasty(_)
                );
            if candidate.owner != Some(package)
                || (candidate.kind != kind && !can_upgrade)
                || candidate.info != SymbolInfo::Missing
                || !matches!(
                    candidate.origin,
                    SymbolOrigin::Classfile(_) | SymbolOrigin::Tasty(_)
                )
            {
                return Err(ClassLoadError::IdentityConflict {
                    name: binary_name.clone(),
                    reason: "the canonical package name is occupied by an incompatible symbol"
                        .to_owned(),
                });
            }
            if can_upgrade || candidate.flags != flags || candidate.visibility != visibility {
                self.session.record_symbol_update(self.store, symbol);
                let candidate = self.store.symbols.get_mut(symbol);
                candidate.kind = kind;
                candidate.flags = flags;
                candidate.visibility = visibility;
                candidate.origin = SymbolOrigin::Tasty(self.store.origins.register_tasty());
            }
            return Ok(symbol);
        }
        let symbol = self.store.symbols.alloc(Symbol {
            name,
            owner: Some(package),
            kind,
            flags,
            visibility,
            info: SymbolInfo::Missing,
            origin: SymbolOrigin::Tasty(self.store.origins.register_tasty()),
            annotations: Vec::new(),
            position: None,
            links: SymbolLinks::default(),
        });
        self.session
            .packages
            .enter_class(self.store, package, name, symbol);
        Ok(symbol)
    }

    fn link_class_object_companions(
        &mut self,
        name: &BinaryName,
        class: SymbolId,
        object: SymbolId,
    ) -> Result<(), ClassLoadError> {
        let class_links = self.store.symbols.get(class).links.companion;
        let object_links = self.store.symbols.get(object).links.companion;
        if class_links.is_some_and(|linked| linked != object)
            || object_links.is_some_and(|linked| linked != class)
        {
            return Err(ClassLoadError::IdentityConflict {
                name: name.clone(),
                reason: "a class or object already has a different companion link".to_owned(),
            });
        }
        if class_links.is_none() {
            self.session.record_symbol_update(self.store, class);
            self.store.symbols.get_mut(class).links.companion = Some(object);
        }
        if object_links.is_none() {
            self.session.record_symbol_update(self.store, object);
            self.store.symbols.get_mut(object).links.companion = Some(class);
        }
        Ok(())
    }

    fn ensure_class_identity_inner(
        &mut self,
        name: &BinaryName,
        ancestry: &mut Vec<BinaryName>,
    ) -> Result<SymbolId, ClassLoadError> {
        if let Some(profile) = &mut self.active_load_profile {
            profile.classes.insert(name.as_internal().to_owned());
        }
        if name.as_internal() == "java/lang/Object" {
            self.session
                .cache_identified(name.clone(), self.definitions.object_class);
            return Ok(self.definitions.object_class);
        }
        if let Some(symbol) = self.compiler_builtin_class(name) {
            self.session.cache_identified(name.clone(), symbol);
            return Ok(symbol);
        }
        if ancestry.len() >= MAX_INHERITANCE_DEPTH || ancestry.contains(name) {
            return Err(ClassLoadError::IdentityConflict {
                name: name.clone(),
                reason: "cyclic or excessively deep enclosing-class metadata".to_owned(),
            });
        }
        match self.repository.get(name) {
            Some(ClassEntry::Loading(symbol)) => {
                if let Some(profile) = &mut self.active_load_profile {
                    profile.shells_reused += 1;
                    profile
                        .identity_shells
                        .insert(name.as_internal().to_owned());
                }
                return Ok(*symbol);
            }
            Some(ClassEntry::Failed(_, failure)) => return Err(failure.error.clone()),
            _ => {}
        }
        if let Some(symbol) = self.session.identified.get(name).copied() {
            if !self.store.symbols.contains(symbol) {
                return Err(ClassLoadError::IdentityConflict {
                    name: name.clone(),
                    reason: "the session refers to a symbol removed from the semantic store"
                        .to_owned(),
                });
            }
            let info = self.store.symbols.get(symbol).info;
            match info {
                SymbolInfo::Missing => {
                    if let Some(profile) = &mut self.active_load_profile {
                        profile.shells_reused += 1;
                        profile
                            .identity_shells
                            .insert(name.as_internal().to_owned());
                    }
                }
                SymbolInfo::Complete(info)
                    if self.store.types.try_get(info).is_some_and(|ty| {
                        matches!(ty, Type::ClassInfo(info)
                        if info.class == symbol
                            && self.store.scopes.contains(info.declarations)
                            && self.store.scopes.get(info.declarations).owner == Some(symbol))
                    }) =>
                {
                    self.session.cache_resolved(name.clone(), symbol);
                }
                _ => {
                    return Err(ClassLoadError::IdentityConflict {
                        name: name.clone(),
                        reason: "the class identity has erroneous or invalid completion info"
                            .to_owned(),
                    });
                }
            }
            return Ok(symbol);
        }
        if let Some(&symbol) = self.session.resolved.get(name) {
            self.session.cache_identified(name.clone(), symbol);
            return Ok(symbol);
        }

        ancestry.push(name.clone());
        let result = self.ensure_class_identity_uncached(name, ancestry);
        let popped = ancestry.pop();
        debug_assert_eq!(popped.as_ref(), Some(name));
        result
    }

    fn ensure_class_identity_uncached(
        &mut self,
        name: &BinaryName,
        ancestry: &mut Vec<BinaryName>,
    ) -> Result<SymbolId, ClassLoadError> {
        let lookup_started = self.collect_load_profiles.then(Instant::now);
        let lookup_result = self.class_path.find_class(name);
        self.record_resource_lookup(lookup_started, &lookup_result);
        let resource = lookup_result
            .map_err(|error| ClassLoadError::Io(name.clone(), Rc::new(error)))?
            .ok_or_else(|| ClassLoadError::NotFound(name.clone()))?;
        let origin = resource.origin().clone();
        let header = (|| -> Result<_, ClassLoadError> {
            Ok(match resource.format() {
                ClassFormat::Class => {
                    let mut reader = Reader::new(resource.bytes());
                    let decode_started = self.collect_load_profiles.then(Instant::now);
                    let decoded_class_file = ClassFile::decode(&mut reader);
                    self.record_classfile_decode(decode_started);
                    let class_file = decoded_class_file
                        .map_err(|error| ClassLoadError::InvalidClassFile(name.clone(), error))?;
                    if !class_file.version.is_compatible() {
                        return Err(ClassLoadError::UnsupportedClassVersion(
                            name.clone(),
                            class_file.version,
                        ));
                    }
                    let actual = self.resolve_name(name, &class_file, class_file.this_class)?;
                    if actual != *name {
                        return Err(ClassLoadError::NameMismatch {
                            requested: name.clone(),
                            actual,
                        });
                    }
                    let owner = self.semantic_owner_name(name, &class_file)?;
                    let visibility = self.semantic_class_visibility_flags(name, &class_file)?;
                    (class_file.access_flags, None, visibility, owner)
                }
                ClassFormat::Tasty => {
                    let identity = tasty_symbol::decode_identity(resource.bytes(), name)
                        .map_err(|error| ClassLoadError::InvalidTastyFile(name.clone(), error))?;
                    (identity.flags, identity.visibility, None, None)
                }
            })
        })();
        let (flags, tasty_visibility, classfile_visibility, semantic_owner) =
            header.inspect_err(|_| {
                self.dependency_failure_context =
                    Some((name.clone(), origin.clone(), resource.format()));
            })?;

        let semantic_owner = semantic_owner
            .map(|owner| {
                self.record_dependency_edge(
                    name,
                    &owner,
                    ClassLoadDependencyReason::EnclosingOwnerIdentity,
                );
                self.ensure_class_identity_inner(&owner, ancestry)
            })
            .transpose()?;
        let package = self.session.packages.resolve(self.store, name);
        let text = self.store.names.intern(name.simple_name());
        let symbol_name = Name::new(text, Namespace::Type);
        let package_scope = self
            .session
            .packages
            .scope_of(package)
            .expect("resolved package has a declaration scope");
        let existing = self
            .store
            .scopes
            .get(package_scope)
            .lookup_all(&symbol_name);
        if existing.len() > 1 {
            return Err(ClassLoadError::IdentityConflict {
                name: name.clone(),
                reason: "the canonical package scope contains multiple declarations".to_owned(),
            });
        }
        if let Some(&symbol) = existing.first() {
            let candidate = self.store.symbols.get(symbol);
            let complete_class = match candidate.info {
                SymbolInfo::Complete(info) => {
                    matches!(self.store.types.get(info), Type::ClassInfo(_))
                }
                SymbolInfo::Missing => false,
                _ => false,
            };
            if !matches!(
                candidate.kind,
                SymbolKind::Class | SymbolKind::Trait | SymbolKind::ModuleClass
            ) || !matches!(
                candidate.origin,
                SymbolOrigin::Classfile(_) | SymbolOrigin::Tasty(_)
            ) || (candidate.info != SymbolInfo::Missing && !complete_class)
            {
                return Err(ClassLoadError::IdentityConflict {
                    name: name.clone(),
                    reason: "the canonical package name is occupied by an incompatible symbol"
                        .to_owned(),
                });
            }
            let expected_kind = if flags.is_interface() {
                SymbolKind::Trait
            } else {
                SymbolKind::Class
            };
            if candidate.kind != expected_kind {
                return Err(ClassLoadError::IdentityConflict {
                    name: name.clone(),
                    reason: "the existing class identity has a different class kind".to_owned(),
                });
            }
            if complete_class {
                self.session.cache_resolved(name.clone(), symbol);
            } else {
                self.session.cache_identified(name.clone(), symbol);
                if let Some(profile) = &mut self.active_load_profile {
                    profile.shells_reused += 1;
                    profile
                        .identity_shells
                        .insert(name.as_internal().to_owned());
                }
            }
            self.session.record_origin(symbol, origin);
            return Ok(symbol);
        }

        let kind = if flags.is_interface() {
            SymbolKind::Trait
        } else {
            SymbolKind::Class
        };
        let visibility = match tasty_visibility {
            Some(visibility) => self.tasty_visibility(name, &visibility),
            None => match classfile_visibility.unwrap_or(flags) {
                visibility if visibility.contains(ACC_PRIVATE) => Visibility::Private,
                visibility if visibility.contains(ACC_PROTECTED) => Visibility::Protected,
                visibility if visibility.is_public() => Visibility::Public,
                _ => Visibility::Package(package),
            },
        };
        let mut symbol_flags = SymbolFlags::JAVA_DEFINED;
        if flags.is_abstract() {
            symbol_flags = symbol_flags | SymbolFlags::ABSTRACT;
        }
        if flags.is_final() {
            symbol_flags = symbol_flags | SymbolFlags::FINAL;
        }
        if flags.is_synthetic() {
            symbol_flags = symbol_flags | SymbolFlags::SYNTHETIC;
        }
        let symbol = self.store.symbols.alloc(Symbol {
            name: symbol_name,
            owner: Some(semantic_owner.unwrap_or(package)),
            kind,
            flags: symbol_flags,
            visibility,
            info: SymbolInfo::Missing,
            origin: match resource.format() {
                ClassFormat::Class => {
                    SymbolOrigin::Classfile(self.store.origins.register_classfile())
                }
                ClassFormat::Tasty => SymbolOrigin::Tasty(self.store.origins.register_tasty()),
            },
            annotations: Vec::new(),
            position: None,
            links: SymbolLinks::default(),
        });
        self.session
            .packages
            .enter_class(self.store, package, symbol_name, symbol);
        self.session.cache_identified(name.clone(), symbol);
        self.session.record_origin(symbol, origin);
        if let Some(profile) = &mut self.active_load_profile {
            profile.shells_created += 1;
            profile
                .identity_shells
                .insert(name.as_internal().to_owned());
        }
        Ok(symbol)
    }

    /// Loads a class like [`Self::load_class`], retaining the selected
    /// classpath resource's origin and format on failure when known.
    ///
    /// This additive API leaves [`Self::load_class`] and its error type
    /// unchanged. Dependency failures carry the deepest failing
    /// dependency's context; absent classes and lookup errors have no
    /// resource context.
    pub fn load_class_detailed(&mut self, name: &BinaryName) -> Result<SymbolId, ClassLoadFailure> {
        self.dependency_failure_context = None;
        let store_checkpoint = self.store.checkpoint();
        let session_checkpoint = self.session.checkpoint();
        let result = if self.collect_load_profiles {
            self.profiled_load_class(name, &mut Vec::new())
        } else {
            self.load_class_with_inheritance_ancestry(name, &mut Vec::new())
        };
        let result = match result {
            Ok(symbol) => Ok(symbol),
            Err(error) => match self.repository.get(name) {
                Some(ClassEntry::Failed(_, failure)) => Err(failure.clone()),
                _ => Err(ClassLoadFailure {
                    error,
                    origin: None,
                    format: None,
                }),
            },
        };
        if result.is_err() {
            self.session.rollback_to(self.store, session_checkpoint);
            self.store.rollback_to(store_checkpoint);
            self.repository.discard_rolled_back_symbols(self.store);
        }
        result
    }

    fn compiler_builtin_class(&self, name: &BinaryName) -> Option<SymbolId> {
        match name.as_internal() {
            "scala/Int" => self
                .store
                .types
                .get(self.definitions.int)
                .reference_symbol(),
            "scala/Any" => Some(self.definitions.any_class),
            "scala/AnyRef" => Some(self.definitions.object_class),
            "scala/Nothing" => Some(self.definitions.nothing_class),
            _ => None,
        }
    }

    fn cached_dependency_context(
        &self,
        error: &ClassLoadError,
    ) -> Option<(Option<ClassOrigin>, Option<ClassFormat>)> {
        let ClassLoadError::DependencyFailure {
            dependency, source, ..
        } = error
        else {
            return None;
        };

        self.cached_dependency_context(source)
            .or_else(|| match self.repository.get(dependency) {
                Some(ClassEntry::Failed(_, failure)) => {
                    Some((failure.origin.clone(), failure.format))
                }
                _ => None,
            })
    }

    fn load_class_with_inheritance_ancestry(
        &mut self,
        name: &BinaryName,
        inheritance_ancestry: &mut Vec<BinaryName>,
    ) -> Result<SymbolId, ClassLoadError> {
        if let Some(symbol) = self.compiler_builtin_class(name) {
            return Ok(symbol);
        }

        // Checked first, and separately from `self.repository` below: a
        // name found here was resolved successfully by *some* loader
        // sharing this `session` (this one or an earlier one), and that
        // positive result is valid regardless of which loader's
        // `class_path` produced it (`LoadingSession`'s own doc comment).
        // A negative or in-progress result, by contrast, is *not*
        // necessarily valid for a differently configured loader, so it is
        // never stored here -- only in the loader-local `self.repository`
        // consulted next.
        if let Some(&symbol) = self.session.resolved.get(name) {
            if self.collect_resource_observations {
                // A resource read now cannot prove which bytes produced the
                // already completed semantic class, nor its transitive
                // hierarchy. Do not publish a reusable module-miss proof
                // when completion reused such a session entry.
                self.resource_observations_complete = false;
            }
            if let Some(profile) = &mut self.active_load_profile {
                profile.completed_cache_hits += 1;
            }
            return Ok(symbol);
        }

        match self.repository.get(name).cloned() {
            Some(ClassEntry::Loaded(symbol)) => return Ok(symbol),
            Some(ClassEntry::Failed(_, failure)) => return Err(failure.error),
            Some(ClassEntry::Loading(symbol)) => {
                if !inheritance_ancestry.contains(name) {
                    // A superclass may point back to a class which is
                    // loading through an ordinary member-type dependency
                    // (for example Object -> member Class -> superclass
                    // Object). Reuse its stable, incomplete identity; only
                    // a name already on this explicit supertype path is a
                    // genuine inheritance cycle.
                    return Ok(symbol);
                }
                let error = ClassLoadError::CircularInheritance(name.clone());
                // Same reasoning as the `DependencyFailure` branch below:
                // this class's `Symbol` was already allocated by
                // `enter_class` and may already be reachable through a
                // legitimate mutual member-type reference, so it must not
                // be left looking like a merely-not-yet-completed
                // `SymbolInfo::Missing` once the repository says `Failed`.
                self.store.symbols.get_mut(symbol).info = SymbolInfo::Error;
                let failure = ClassLoadFailure {
                    error: error.clone(),
                    origin: self
                        .loading_context
                        .get(name)
                        .map(|(origin, _)| origin.clone())
                        .or_else(|| self.session.origins.get(&symbol).cloned()),
                    format: self.loading_context.get(name).map(|(_, format)| *format),
                };
                self.repository
                    .mark_failed(name.clone(), Some(symbol), failure);
                return Err(error);
            }
            None => {}
        }

        if inheritance_ancestry.len() >= MAX_INHERITANCE_DEPTH {
            return Err(ClassLoadError::InheritanceDepthExceeded(name.clone()));
        }

        let previous_shell = self
            .session
            .identified
            .get(name)
            .copied()
            .map(|id| (id, self.store.symbols.get(id).clone()));
        inheritance_ancestry.push(name.clone());
        let load_result = self.load_uncached(name, inheritance_ancestry);
        let popped = inheritance_ancestry.pop();
        debug_assert_eq!(popped.as_ref(), Some(name));

        match load_result {
            Ok(symbol) => {
                self.repository.mark_loaded(name.clone(), symbol);
                self.session.cache_resolved(name.clone(), symbol);
                if let Some(profile) = &mut self.active_load_profile {
                    profile.full_completions += 1;
                    profile
                        .completed_classes
                        .insert(name.as_internal().to_owned());
                }
                Ok(symbol)
            }
            Err(failure) => {
                if let Some((id, symbol)) = previous_shell {
                    *self.store.symbols.get_mut(id) = symbol;
                }
                // Internal eager loads mark newly entered symbols as erroneous
                // until the outer public transaction rolls them back. A shell
                // published before this operation instead keeps its previous state.
                let still_loading_symbol = self.repository.get(name).and_then(ClassEntry::symbol);
                if let Some(symbol) = still_loading_symbol {
                    if !self.session.identified.contains_key(name) {
                        self.store.symbols.get_mut(symbol).info = SymbolInfo::Error;
                    }
                    self.in_progress_classes.remove(&symbol);
                }
                self.repository
                    .mark_failed(name.clone(), still_loading_symbol, failure.clone());
                Err(failure.error)
            }
        }
    }

    fn profiled_load_class(
        &mut self,
        name: &BinaryName,
        inheritance_ancestry: &mut Vec<BinaryName>,
    ) -> Result<SymbolId, ClassLoadError> {
        let is_root = self.active_load_profile.is_none();
        if is_root {
            let root_was_shell = self
                .session
                .identified
                .get(name)
                .is_some_and(|id| self.store.symbols.get(*id).info == SymbolInfo::Missing);
            let root = name.as_internal().to_owned();
            self.active_load_profile = Some(ActiveLoadProfile {
                root: root.clone(),
                root_was_shell,
                identity_shells: if root_was_shell {
                    BTreeSet::from([root])
                } else {
                    BTreeSet::new()
                },
                ..ActiveLoadProfile::default()
            });
        }
        let profile = self
            .active_load_profile
            .as_mut()
            .expect("profile is initialized for a top-level load");
        profile.depth += 1;
        profile.max_dependency_depth = profile.max_dependency_depth.max(profile.depth - 1);
        profile.classes.insert(name.as_internal().to_owned());

        let result = self.load_class_with_inheritance_ancestry(name, inheritance_ancestry);

        let profile = self
            .active_load_profile
            .as_mut()
            .expect("profile remains active until the top-level load returns");
        profile.depth -= 1;
        if is_root {
            self.finish_load_profile();
        }
        result
    }

    fn load_uncached(
        &mut self,
        name: &BinaryName,
        inheritance_ancestry: &mut Vec<BinaryName>,
    ) -> Result<SymbolId, ClassLoadFailure> {
        let uncached_started = self.collect_load_profiles.then(Instant::now);
        let resource_lookup_started = self.collect_load_profiles.then(Instant::now);
        let resource_result = self.class_path.find_class(name);
        self.record_resource_lookup(resource_lookup_started, &resource_result);
        let resource = match resource_result {
            Ok(Some(resource)) => resource,
            Ok(None) => {
                if let (Some(started), Some(profile)) =
                    (uncached_started, self.active_load_profile.as_mut())
                {
                    profile.load_uncached_inclusive_ns += started.elapsed().as_nanos();
                }
                return Err(ClassLoadFailure {
                    error: ClassLoadError::NotFound(name.clone()),
                    origin: None,
                    format: None,
                });
            }
            Err(error) => {
                if let (Some(started), Some(profile)) =
                    (uncached_started, self.active_load_profile.as_mut())
                {
                    profile.load_uncached_inclusive_ns += started.elapsed().as_nanos();
                }
                return Err(ClassLoadFailure {
                    error: ClassLoadError::Io(name.clone(), Rc::new(error)),
                    origin: None,
                    format: None,
                });
            }
        };

        let resource_origin = resource.origin().clone();
        let resource_format = resource.format();
        if self.collect_resource_observations {
            self.resource_observations
                .insert(name.clone(), resource.clone());
        }
        self.loading_context
            .insert(name.clone(), (resource_origin.clone(), resource_format));
        let result = match resource_format {
            ClassFormat::Class => self.load_uncached_class(
                name,
                resource.bytes(),
                resource_origin.clone(),
                inheritance_ancestry,
            ),
            ClassFormat::Tasty => self.load_uncached_tasty(
                name,
                resource.bytes(),
                resource_origin.clone(),
                inheritance_ancestry,
            ),
        };
        self.loading_context.remove(name);
        let observed_context = self.dependency_failure_context.take();
        let result = result.map_err(|error| {
            let cause_name = error_binary_name(error.root_cause());
            let failure_is_for_another_name =
                cause_name.is_some_and(|cause_name| cause_name != name);
            let observed_context = observed_context
                .filter(|(dependency, _, _)| Some(dependency) == cause_name)
                .map(|(_, origin, format)| (Some(origin), Some(format)));
            let dependency_context = self.cached_dependency_context(&error);
            let (origin, format) = observed_context.or(dependency_context).unwrap_or({
                if failure_is_for_another_name {
                    (None, None)
                } else {
                    (Some(resource_origin), Some(resource_format))
                }
            });
            ClassLoadFailure {
                error,
                origin,
                format,
            }
        });
        if let (Some(started), Some(profile)) =
            (uncached_started, self.active_load_profile.as_mut())
        {
            profile.load_uncached_inclusive_ns += started.elapsed().as_nanos();
        }
        result
    }

    /// Allocates a class's stable `SymbolId`, and its declarations scope,
    /// immediately — before any recursive supertype/member resolution.
    /// `dotty-core`'s enter-before-complete rule (`docs/classloader.md`
    /// §5) covers the `SymbolId`; the scope is allocated this early too so
    /// fields/methods can be entered into it as they're decoded, rather
    /// than collected separately and entered only once the whole class is
    /// known.
    fn enter_class(
        &mut self,
        name: &BinaryName,
        flags: ClassAccessFlags,
        origin: SymbolOrigin,
    ) -> Result<(SymbolId, ScopeId, SymbolOrigin), ClassLoadError> {
        let owner = self.session.packages.resolve(self.store, name);

        let text = self.store.names.intern(name.simple_name());
        let symbol_name = Name::new(text, Namespace::Type);

        // A JVM class file's own access_flags carry no ACC_PRIVATE/
        // ACC_PROTECTED bit (JVMS §4.1 Table 4.1-A) -- those only appear on
        // a *member* class's InnerClasses entry, not here -- so a top-level
        // class is only ever public or package-private. `flags` is shaped
        // exactly like a real `.class` file's access_flags for both
        // callers (`load_uncached_class` and, via
        // `tasty_symbol::decode_flags`, `load_uncached_tasty`), so this is
        // correct and complete for `.class` but not the final word for
        // `.tasty` -- see `load_uncached_tasty`'s own visibility patch,
        // right after it calls this, for where a `.tasty` class's real
        // (potentially more restrictive) `private`/`protected` visibility
        // is corrected in.
        let kind = if flags.is_interface() {
            SymbolKind::Trait
        } else {
            SymbolKind::Class
        };
        let visibility = if flags.is_public() {
            Visibility::Public
        } else {
            Visibility::Package(owner)
        };

        let mut symbol_flags = SymbolFlags::JAVA_DEFINED;
        if flags.is_abstract() {
            symbol_flags = symbol_flags | SymbolFlags::ABSTRACT;
        }
        if flags.is_final() {
            symbol_flags = symbol_flags | SymbolFlags::FINAL;
        }
        if flags.is_synthetic() {
            symbol_flags = symbol_flags | SymbolFlags::SYNTHETIC;
        }

        // `java/lang/Object` is the one real, classpath-loadable JVM class
        // among `Definitions`'s builtins (`Any`/`Nothing` are Scala
        // compiler fictions with no `.class`/`.tasty` of their own) --
        // reusing `definitions.object_class`'s `SymbolId` here, instead of
        // allocating a fresh one, means a `Type::TypeRef` built from
        // `Definitions` (e.g. an implicit JVM superclass) and one built
        // from actually loading `java/lang/Object` off the classpath name
        // the exact same symbol, not two permanently distinct ones.
        let builtin_object = name.as_internal() == "java/lang/Object";
        let existing = self
            .session
            .identified
            .get(name)
            .copied()
            .or_else(|| builtin_object.then_some(self.definitions.object_class));
        let (class_symbol, class_origin) = if let Some(symbol) = existing {
            if !self.store.symbols.contains(symbol) {
                return Err(ClassLoadError::IdentityConflict {
                    name: name.clone(),
                    reason: "the session refers to a symbol removed from the semantic store"
                        .to_owned(),
                });
            }
            let existing_symbol = self.store.symbols.get(symbol);
            let same_origin_kind = matches!(
                (existing_symbol.origin, origin),
                (SymbolOrigin::Classfile(_), SymbolOrigin::Classfile(_))
                    | (SymbolOrigin::Tasty(_), SymbolOrigin::Tasty(_))
            );
            if existing_symbol.kind != kind
                || (!builtin_object && !same_origin_kind)
                || (!builtin_object && existing_symbol.info != SymbolInfo::Missing)
            {
                return Err(ClassLoadError::IdentityConflict {
                    name: name.clone(),
                    reason:
                        "the existing shell has incompatible kind, provenance, or completion state"
                            .to_owned(),
                });
            }
            let class_origin = if builtin_object {
                origin
            } else {
                existing_symbol.origin
            };
            let semantic_owner = existing_symbol.owner.unwrap_or(owner);
            let visibility = if builtin_object {
                visibility
            } else {
                existing_symbol.visibility
            };
            self.session.record_symbol_update(self.store, symbol);
            *self.store.symbols.get_mut(symbol) = Symbol {
                name: symbol_name,
                owner: Some(semantic_owner),
                kind,
                flags: symbol_flags,
                visibility,
                info: SymbolInfo::Missing,
                origin: class_origin,
                annotations: Vec::new(),
                position: None,
                links: SymbolLinks::default(),
            };
            (symbol, class_origin)
        } else {
            let symbol = self.store.symbols.alloc(Symbol {
                name: symbol_name,
                owner: Some(owner),
                kind,
                flags: symbol_flags,
                visibility,
                info: SymbolInfo::Missing,
                origin,
                annotations: Vec::new(),
                position: None,
                links: SymbolLinks::default(),
            });
            (symbol, origin)
        };
        self.session
            .packages
            .enter_class(self.store, owner, symbol_name, class_symbol);
        let declarations = self.store.scopes.alloc(Scope::new(Some(class_symbol)));

        Ok((class_symbol, declarations, class_origin))
    }

    /// Resolves `super_class`/`interfaces` into `Type::TypeRef`s and
    /// completes `class_symbol`'s `SymbolInfo`. `declarations` was already
    /// allocated by [`Self::enter_class`] and populated (for a `.class`
    /// symbol) by the field/method decode loop by the time this runs.
    fn complete_class(
        &mut self,
        class_symbol: SymbolId,
        declarations: ScopeId,
        super_class: Option<SymbolId>,
        interfaces: Vec<SymbolId>,
    ) {
        let mut parents = Vec::with_capacity(interfaces.len() + 1);
        if let Some(parent) = super_class {
            parents.push(self.type_ref(parent));
        }
        for parent in interfaces {
            parents.push(self.type_ref(parent));
        }

        let class_info = self.store.types.alloc(Type::ClassInfo(ClassInfo {
            prefix: self.definitions.no_prefix,
            class: class_symbol,
            parents,
            declarations,
            self_type: None,
        }));
        self.store
            .symbols
            .set_info(class_symbol, SymbolInfo::Complete(class_info));
    }

    fn type_ref(&mut self, symbol: SymbolId) -> TypeId {
        self.store
            .types
            .alloc(Type::type_ref(self.definitions.no_prefix, symbol))
    }

    /// Allocates a field's `Symbol` and enters it into the class's
    /// declarations scope by name. Unlike a class, a field needs no
    /// enter-before-complete staging: its type is fully known the moment
    /// its descriptor (and, transitively, any class it references) is
    /// resolved, so it is allocated already `SymbolInfo::Complete`.
    fn enter_field(
        &mut self,
        class_symbol: SymbolId,
        declarations: ScopeId,
        field_name: &str,
        flags: FieldAccessFlags,
        resolved_type: TypeId,
        origin: SymbolOrigin,
    ) -> SymbolId {
        // Unlike a top-level class's access_flags, a field's *can* carry
        // ACC_PRIVATE/ACC_PROTECTED (JVMS §4.5 Table 4.5-A), so all four
        // Java visibilities are distinguishable here.
        let visibility = if flags.is_private() {
            Visibility::Private
        } else if flags.is_protected() {
            Visibility::Protected
        } else if flags.is_public() {
            Visibility::Public
        } else {
            let owner = self
                .store
                .symbols
                .get(class_symbol)
                .owner
                .expect("a loaded class always has a package owner");
            Visibility::Package(owner)
        };

        let mut symbol_flags = SymbolFlags::JAVA_DEFINED;
        if flags.is_static() {
            symbol_flags = symbol_flags | SymbolFlags::STATIC;
        }
        if flags.is_final() {
            symbol_flags = symbol_flags | SymbolFlags::FINAL;
        } else {
            symbol_flags = symbol_flags | SymbolFlags::MUTABLE;
        }
        if flags.is_synthetic() {
            symbol_flags = symbol_flags | SymbolFlags::SYNTHETIC;
        }

        let text = self.store.names.intern(field_name);
        let symbol_name = Name::new(text, Namespace::Term);

        let field_symbol = self.store.symbols.alloc(Symbol {
            name: symbol_name,
            owner: Some(class_symbol),
            kind: SymbolKind::Field,
            flags: symbol_flags,
            visibility,
            info: SymbolInfo::Complete(resolved_type),
            origin,
            annotations: Vec::new(),
            position: None,
            links: SymbolLinks::default(),
        });

        self.store
            .scopes
            .get_mut(declarations)
            .enter(symbol_name, field_symbol);
        field_symbol
    }

    /// Walks an already-parsed [`FieldType`] (JVMS §4.3.2) into a semantic
    /// `TypeId`: primitives map to their canonical [`Definitions`]
    /// identity, `Object` resolves via [`Self::ensure_dependency_identity`] into
    /// a `Type::TypeRef`, and `Array` becomes a `Type::JavaArray`
    /// recursing into its component type.
    fn lower_field_type(
        &mut self,
        owner: &BinaryName,
        field_type: &FieldType,
    ) -> Result<TypeId, ClassLoadError> {
        match field_type {
            FieldType::Byte => Ok(self.definitions.byte),
            FieldType::Char => Ok(self.definitions.char),
            FieldType::Double => Ok(self.definitions.double),
            FieldType::Float => Ok(self.definitions.float),
            FieldType::Int => Ok(self.definitions.int),
            FieldType::Long => Ok(self.definitions.long),
            FieldType::Short => Ok(self.definitions.short),
            FieldType::Boolean => Ok(self.definitions.boolean),
            FieldType::Object(class_name) => {
                let dependency = BinaryName::from_internal(class_name);
                let symbol = self.ensure_dependency_identity(owner, &dependency)?;
                Ok(self.type_ref(symbol))
            }
            FieldType::Array(component) => {
                let element = self.lower_field_type(owner, component)?;
                Ok(self.store.types.alloc(Type::JavaArray { element }))
            }
        }
    }

    /// Resolves an already-parsed [`FieldType`]'s optional `Signature`
    /// attribute and lowers the field's semantic type, in the order both a
    /// real field and a `Record` component need them. Shared by the
    /// field-building loop below and [`Self::resolve_record_components`]
    /// — the only difference between the two is that a record component
    /// has no access flags of its own in the class file format.
    ///
    /// When a `Signature` is present, it is strictly more precise than the
    /// erased descriptor (e.g. `List<T>` vs. erased `List`), so it is
    /// preferred as the resolved type; `field_type` is still parsed either
    /// way (it stays on [`crate::field_symbol::FieldSymbol`]/
    /// [`RecordComponentSymbol`] verbatim) but is only *lowered* when there
    /// is no signature to prefer instead.
    fn lower_field_like(
        &mut self,
        owner: &BinaryName,
        declarations: ScopeId,
        field_type: &FieldType,
        attributes: &[Attribute<'_>],
        constant_pool: &ConstantPool,
    ) -> Result<(Option<FieldSignature>, TypeId), ClassLoadError> {
        let signature = self.resolve_optional_signature(
            owner,
            attributes,
            constant_pool,
            FieldSignature::parse,
        )?;
        let resolved_type = match &signature {
            Some(FieldSignature(reference)) => {
                let env = self.class_type_var_env(declarations, None);
                self.lower_reference_type_signature(owner, &env, reference)?
            }
            None => self.lower_field_type(owner, field_type)?,
        };
        Ok((signature, resolved_type))
    }

    /// Enters each of `signature`'s declared type parameters (JVMS
    /// §4.7.9.1) as a real `SymbolKind::TypeParameter` symbol into
    /// `declarations` — a no-op when the class has no `Signature` attribute
    /// (the common, non-generic case) or declares none of its own. Every
    /// symbol is entered under its own `Namespace::Type` name *before* any
    /// bound is lowered (mirroring `enter_class`'s enter-before-complete
    /// staging one level down), so a bound that mentions another type
    /// parameter — an F-bounded parameter's own bound mentioning itself
    /// (`<T extends Comparable<T>>`), or one parameter's bound mentioning a
    /// sibling declared later (`<K extends Comparable<K>, V>`) — resolves
    /// correctly regardless of declaration order.
    ///
    /// Field/method signature lowering below resolves a `TypeVariable`
    /// reference against these same symbols via ordinary `Scope::lookup` on
    /// `declarations` (see [`Self::resolve_type_variable`]) — ordinary name
    /// lookup, since a type parameter and a same-named field/method never
    /// collide (`Namespace::Type` vs. `Namespace::Term`).
    fn enter_class_type_parameters(
        &mut self,
        owner: &BinaryName,
        declarations: ScopeId,
        signature: Option<&ClassSignature>,
        origin: SymbolOrigin,
    ) -> Result<(), ClassLoadError> {
        let Some(signature) = signature else {
            return Ok(());
        };

        let class_symbol = self.store.scopes.g…65962 tokens truncated…            BinaryName::from_internal("Good"),
                (
                    synthetic_class("Good", None),
                    ClassFormat::Class,
                    ClassOrigin::Directory(PathBuf::from("/good")),
                ),
            ),
        ]);
        let mut store = SemanticStore::new();
        let mut loader = ClassLoader::new(ResourceClassPath(resources), &mut store);

        let failure = loader
            .load_class_detailed(&BinaryName::from_internal("Broken"))
            .expect_err("malformed bytes must fail independently");
        assert_eq!(failure.code(), "classloader.invalid_class_file");
        assert_eq!(
            failure.origin,
            Some(ClassOrigin::Directory(PathBuf::from("/broken")))
        );

        let good = loader
            .load_class_detailed(&BinaryName::from_internal("Good"))
            .expect("a separate valid class should still load");
        assert!(matches!(
            loader.repository.get(&BinaryName::from_internal("Broken")),
            Some(ClassEntry::Failed(_, _))
        ));
        assert!(matches!(
            loader.repository.get(&BinaryName::from_internal("Good")),
            Some(ClassEntry::Loaded(symbol)) if *symbol == good
        ));
        drop(loader);
        assert_eq!(symbol_name(&store, good), "Good");
    }

    #[test]
    fn detailed_cached_failure_preserves_resource_provenance_without_reprobing() {
        let calls = Arc::new(AtomicUsize::new(0));
        let name = BinaryName::from_internal("Broken");
        let class_path = CountingClassPath {
            name: name.clone(),
            bytes: vec![0x00, 0x01, 0x02],
            calls: Arc::clone(&calls),
        };
        let mut store = SemanticStore::new();
        let mut loader = ClassLoader::new(class_path, &mut store);

        let first = loader.load_class_detailed(&name).unwrap_err();
        let second = loader.load_class_detailed(&name).unwrap_err();

        assert_eq!(calls.load(Ordering::Relaxed), 1);
        assert_eq!(first.code(), "classloader.invalid_class_file");
        assert_eq!(
            first.category(),
            crate::error::ClassLoadCategory::MalformedArtifact
        );
        assert_eq!(
            first.origin,
            Some(ClassOrigin::Directory(PathBuf::from("/classes")))
        );
        assert_eq!(first.format, Some(ClassFormat::Class));
        assert_eq!(second.code(), first.code());
        assert_eq!(second.origin, first.origin);
        assert_eq!(second.format, first.format);
    }

    #[test]
    fn detailed_dependency_failure_reports_the_missing_dependency_without_owner_origin() {
        let mut classes = HashMap::new();
        classes.insert(
            BinaryName::from_internal("PoolSample"),
            fixture_bytes("pool_sample/PoolSample.class"),
        );
        let mut store = SemanticStore::new();
        let mut loader = ClassLoader::new(InMemoryClassPath(classes), &mut store);

        let failure = loader
            .load_class_detailed(&BinaryName::from_internal("PoolSample"))
            .unwrap_err();
        let cached = loader
            .load_class_detailed(&BinaryName::from_internal("PoolSample"))
            .unwrap_err();

        assert_eq!(failure.code(), "classloader.dependency_failure");
        assert_eq!(
            failure.category(),
            crate::error::ClassLoadCategory::ResolutionOrDependency
        );
        assert_eq!(failure.root_cause_code(), "classloader.not_found");
        assert_eq!(
            failure.root_cause_category(),
            crate::error::ClassLoadCategory::Missing
        );
        assert_eq!(failure.origin, None);
        assert_eq!(failure.format, None);
        match (&failure.error, &cached.error) {
            (
                ClassLoadError::DependencyFailure { source: first, .. },
                ClassLoadError::DependencyFailure { source: second, .. },
            ) => assert!(Rc::ptr_eq(first, second)),
            other => panic!("expected cached dependency failures, got {other:?}"),
        }
    }

    #[test]
    fn detailed_dependency_failure_reports_the_dependency_artifact_origin() {
        let owner = BinaryName::from_internal("PoolSample");
        let dependency = BinaryName::from_internal("java/lang/Object");
        let resources = HashMap::from([
            (
                owner,
                (
                    fixture_bytes("pool_sample/PoolSample.class"),
                    ClassFormat::Class,
                    ClassOrigin::Directory(PathBuf::from("/owner")),
                ),
            ),
            (
                dependency,
                (
                    vec![0x00, 0x01, 0x02],
                    ClassFormat::Class,
                    ClassOrigin::Directory(PathBuf::from("/dependency")),
                ),
            ),
        ]);
        let mut store = SemanticStore::new();
        let mut loader = ClassLoader::new(ResourceClassPath(resources), &mut store);

        let failure = loader
            .load_class_detailed(&BinaryName::from_internal("PoolSample"))
            .unwrap_err();

        assert_eq!(failure.code(), "classloader.dependency_failure");
        assert_eq!(failure.root_cause_code(), "classloader.invalid_class_file");
        assert_eq!(
            failure.origin,
            Some(ClassOrigin::Directory(PathBuf::from("/dependency")))
        );
        assert_eq!(failure.format, Some(ClassFormat::Class));
    }

    #[test]
    fn a_cached_not_found_does_not_probe_the_same_classpath_twice() {
        let lookups = Arc::new(AtomicUsize::new(0));
        let mut store = SemanticStore::new();
        let mut loader =
            ClassLoader::new(CountingMissingClassPath(Arc::clone(&lookups)), &mut store);
        let name = BinaryName::from_internal("Missing");

        for _ in 0..2 {
            assert!(matches!(
                loader.load_class(&name),
                Err(ClassLoadError::NotFound(missing)) if missing == name
            ));
        }

        assert_eq!(lookups.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn io_error_from_the_classpath_is_wrapped_and_not_cached_as_not_found() {
        let mut store = SemanticStore::new();
        let mut loader = ClassLoader::new(AlwaysErrors, &mut store);

        let error = loader
            .load_class(&BinaryName::from_internal("Anything"))
            .unwrap_err();

        assert!(matches!(error, ClassLoadError::Io(name, _) if name.as_internal() == "Anything"));
    }

    #[test]
    fn invalid_class_file_when_the_bytes_do_not_decode() {
        let mut classes = HashMap::new();
        classes.insert(BinaryName::from_internal("Broken"), vec![0x00, 0x01, 0x02]);

        let mut store = SemanticStore::new();

        let mut loader = ClassLoader::new(InMemoryClassPath(classes), &mut store);
        let error = loader
            .load_class(&BinaryName::from_internal("Broken"))
            .unwrap_err();

        assert!(matches!(
            error,
            ClassLoadError::InvalidClassFile(name, _) if name.as_internal() == "Broken"
        ));
    }

    #[test]
    fn unsupported_class_version_when_major_version_exceeds_the_compatibility_ceiling() {
        let mut classes = HashMap::new();
        classes.insert(
            BinaryName::from_internal("FromTheFuture"),
            synthetic_future_version_class("FromTheFuture"),
        );

        let mut store = SemanticStore::new();

        let mut loader = ClassLoader::new(InMemoryClassPath(classes), &mut store);
        let error = loader
            .load_class(&BinaryName::from_internal("FromTheFuture"))
            .unwrap_err();

        assert!(matches!(
            error,
            ClassLoadError::UnsupportedClassVersion(name, version)
                if name.as_internal() == "FromTheFuture" && version.major == 71
        ));
    }

    #[test]
    fn detailed_unsupported_version_retains_selected_class_resource() {
        let name = BinaryName::from_internal("FromTheFuture");
        let class_path = CountingClassPath {
            name: name.clone(),
            bytes: synthetic_future_version_class("FromTheFuture"),
            calls: Arc::new(AtomicUsize::new(0)),
        };
        let mut store = SemanticStore::new();
        let mut loader = ClassLoader::new(class_path, &mut store);

        let failure = loader.load_class_detailed(&name).unwrap_err();

        assert_eq!(failure.code(), "classloader.unsupported_class_version");
        assert_eq!(
            failure.category(),
            crate::error::ClassLoadCategory::UnsupportedFeatureOrVersion
        );
        assert!(failure.origin.is_some());
        assert_eq!(failure.format, Some(ClassFormat::Class));
    }

    #[test]
    fn detailed_failure_in_a_jar_retains_the_jar_origin() {
        let mut jar_bytes = fs::read(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/pool_sample_jar/pool_sample_stored.jar"),
        )
        .expect("stored JAR fixture should exist");
        let file_name_length = usize::from(u16::from_le_bytes([jar_bytes[26], jar_bytes[27]]));
        let extra_length = usize::from(u16::from_le_bytes([jar_bytes[28], jar_bytes[29]]));
        let data_start = 30 + file_name_length + extra_length;
        let data_length = u32::from_le_bytes(jar_bytes[18..22].try_into().unwrap()) as usize;
        let data_end = data_start + data_length;
        jar_bytes[data_start] ^= 0xff;
        let checksum = crate::crc32::checksum(&jar_bytes[data_start..data_end]);
        jar_bytes[14..18].copy_from_slice(&checksum.to_le_bytes());
        let central_header = jar_bytes
            .windows(4)
            .position(|window| window == [0x50, 0x4b, 0x01, 0x02])
            .expect("fixture should have a central-directory entry");
        jar_bytes[central_header + 16..central_header + 20]
            .copy_from_slice(&checksum.to_le_bytes());

        let jar_path = std::env::temp_dir().join(format!(
            "dotty-classloader-malformed-resource-{}.jar",
            std::process::id()
        ));
        fs::write(&jar_path, jar_bytes).expect("temporary JAR should be writable");
        let jar_path_classpath = crate::jar_class_path::JarClassPath::new(&jar_path, 21)
            .expect("modified JAR should still have a valid archive structure");
        let mut store = SemanticStore::new();
        let mut loader = ClassLoader::new(jar_path_classpath, &mut store);

        let failure = loader
            .load_class_detailed(&BinaryName::from_internal("PoolSample"))
            .unwrap_err();

        assert_eq!(failure.code(), "classloader.invalid_class_file");
        assert_eq!(failure.format, Some(ClassFormat::Class));
        assert_eq!(failure.origin, Some(ClassOrigin::Jar(jar_path.clone())));
        drop(loader);
        fs::remove_file(jar_path).expect("temporary JAR should be removable");
    }

    #[test]
    fn malformed_reference_when_this_class_does_not_resolve() {
        let mut classes = HashMap::new();
        classes.insert(
            BinaryName::from_internal("Broken"),
            synthetic_malformed_this_class(),
        );

        let mut store = SemanticStore::new();

        let mut loader = ClassLoader::new(InMemoryClassPath(classes), &mut store);
        let error = loader
            .load_class(&BinaryName::from_internal("Broken"))
            .unwrap_err();

        assert!(matches!(
            error,
            ClassLoadError::MalformedReference(name, _) if name.as_internal() == "Broken"
        ));
    }

    #[test]
    fn name_mismatch_when_this_class_names_a_different_class() {
        let mut classes = HashMap::new();
        classes.insert(
            BinaryName::from_internal("RequestedUnderTheWrongName"),
            fixture_bytes("pool_sample/PoolSample.class"),
        );

        let mut store = SemanticStore::new();

        let mut loader = ClassLoader::new(InMemoryClassPath(classes), &mut store);
        let error = loader
            .load_class(&BinaryName::from_internal("RequestedUnderTheWrongName"))
            .unwrap_err();

        assert!(matches!(
            error,
            ClassLoadError::NameMismatch { requested, actual }
                if requested.as_internal() == "RequestedUnderTheWrongName"
                    && actual.as_internal() == "PoolSample"
        ));
    }

    #[test]
    fn dependency_failure_when_the_super_class_cannot_be_found() {
        let mut classes = HashMap::new();
        classes.insert(
            BinaryName::from_internal("PoolSample"),
            fixture_bytes("pool_sample/PoolSample.class"),
        );
        // Deliberately omit java/lang/Object and java/lang/Runnable so
        // resolving PoolSample's superclass fails.

        let mut store = SemanticStore::new();

        let mut loader = ClassLoader::new(InMemoryClassPath(classes), &mut store);
        let error = loader
            .load_class(&BinaryName::from_internal("PoolSample"))
            .unwrap_err();

        assert!(matches!(
            error,
            ClassLoadError::DependencyFailure { owner, dependency, source }
                if owner.as_internal() == "PoolSample"
                    && dependency.as_internal() == "java/lang/Object"
                    && matches!(*source, ClassLoadError::NotFound(_))
        ));
    }

    #[test]
    fn invalid_tasty_file_when_the_bytes_do_not_decode() {
        let mut classes = HashMap::new();
        classes.insert(BinaryName::from_internal("Broken"), vec![0x00, 0x01, 0x02]);

        let mut store = SemanticStore::new();

        let mut loader = ClassLoader::new(InMemoryTastyClassPath(classes), &mut store);
        let error = loader
            .load_class(&BinaryName::from_internal("Broken"))
            .unwrap_err();

        assert!(matches!(
            error,
            ClassLoadError::InvalidTastyFile(name, _) if name.as_internal() == "Broken"
        ));
    }

    #[test]
    fn detailed_tasty_failure_retains_the_selected_tasty_resource() {
        let mut classes = HashMap::new();
        classes.insert(BinaryName::from_internal("Broken"), vec![0x00, 0x01, 0x02]);
        let mut store = SemanticStore::new();
        let mut loader = ClassLoader::new(InMemoryTastyClassPath(classes), &mut store);

        let failure = loader
            .load_class_detailed(&BinaryName::from_internal("Broken"))
            .unwrap_err();

        assert_eq!(failure.code(), "classloader.invalid_tasty_file");
        assert_eq!(
            failure.category(),
            crate::error::ClassLoadCategory::MalformedArtifact
        );
        assert_eq!(failure.root_cause_code(), "tasty.invalid_header");
        assert_eq!(failure.format, Some(ClassFormat::Tasty));
        assert_eq!(
            failure.origin,
            Some(ClassOrigin::Directory(PathBuf::from("<memory>")))
        );
    }

    #[test]
    fn dependency_failure_when_a_tasty_classes_super_class_cannot_be_found() {
        let mut classes = HashMap::new();
        classes.insert(
            BinaryName::from_internal("Dog"),
            own_fixture_bytes("tasty_sample/Dog.tasty"),
        );
        // Deliberately omit java/lang/Object (and Animal) so resolving
        // Dog's implicit superclass fails.

        let mut store = SemanticStore::new();

        let mut loader = ClassLoader::new(InMemoryTastyClassPath(classes), &mut store);
        let error = loader
            .load_class(&BinaryName::from_internal("Dog"))
            .unwrap_err();
        assert!(matches!(
            error,
            ClassLoadError::DependencyFailure { owner, dependency, source }
                if owner.as_internal() == "Dog"
                    && dependency.as_internal() == "java/lang/Object"
                    && matches!(*source, ClassLoadError::NotFound(_))
        ));
    }

    /// `case_class/Point.tasty`'s real `case class Point(x: Int, y: Int)`
    /// (`dotty-tasty`'s own fixture, reused here rather than duplicated —
    /// `tasty_loading.rs`'s `Dog`/`Animal` carry no fields/methods at
    /// all) exercises `.tasty` field/method reconstruction end to end:
    /// `x`/`y` are constructor `CASEACCESSOR_TAG` parameters (not
    /// separate `ValDef`s — see
    /// `tasty_symbol::decode_constructor_accessor_fields`'s doc comment),
    /// while `<init>`/`copy`/`hashCode`/... come from ordinary `DefDef`
    /// template stats.
    #[test]
    fn enters_a_real_tasty_case_classs_fields_and_methods_into_the_class_declarations_scope() {
        let mut point = HashMap::new();
        point.insert(
            BinaryName::from_internal("Point"),
            dotty_tasty_fixture_bytes("case_class/Point.tasty"),
        );

        let mut dependencies = HashMap::new();
        dependencies.insert(
            BinaryName::from_internal("java/lang/Object"),
            synthetic_class("java/lang/Object", None),
        );
        dependencies.insert(
            BinaryName::from_internal("scala/Product"),
            synthetic_class("scala/Product", None),
        );
        dependencies.insert(
            BinaryName::from_internal("scala/Serializable"),
            synthetic_class("scala/Serializable", None),
        );
        dependencies.insert(
            BinaryName::from_internal("scala/Int"),
            synthetic_class("scala/Int", None),
        );

        // `Point` is `.tasty`-backed but its dependencies are plain
        // `.class` bytes here -- `InMemoryTastyClassPath` tags every
        // entry it holds the same format, so the two need separate
        // classpath entries composed together, the way `.tasty` and
        // `.class` files really do sit side by side on a JVM classpath.
        let class_path = CompositeClassPath::new(vec![
            Box::new(InMemoryTastyClassPath(point)),
            Box::new(InMemoryClassPath(dependencies)),
        ]);

        let mut store = SemanticStore::new();
        let mut loader = ClassLoader::new(class_path, &mut store);
        let point = loader
            .load_class(&BinaryName::from_internal("Point"))
            .expect("Point should load from its real .tasty fixture");
        drop(loader);

        let declarations = class_info(&store, point).declarations;
        let mut lookup = |member_name: &str| {
            let text = store.names.intern(member_name);
            let name = Name::new(text, Namespace::Term);
            store
                .scopes
                .get(declarations)
                .lookup(&name)
                .unwrap_or_else(|| panic!("{member_name} should be entered in the class scope"))
        };

        let x = lookup("x");
        assert_eq!(store.symbols.get(x).kind, SymbolKind::Field);
        assert_eq!(store.symbols.get(x).visibility, Visibility::Public);
        assert!(store.symbols.get(x).flags.contains(SymbolFlags::FINAL));

        let init = lookup("<init>");
        assert_eq!(store.symbols.get(init).kind, SymbolKind::Constructor);
        let copy = lookup("copy");
        assert_eq!(store.symbols.get(copy).kind, SymbolKind::Method);
        assert!(
            store
                .symbols
                .get(copy)
                .flags
                .contains(SymbolFlags::SYNTHETIC)
        );

        // `x`'s declared type is `Int`, encoded post-typecheck as a
        // `TYPEREF_TAG` node. `RawTree::name_refs` only picks up that
        // node's qualifier prefix ("scala", a plain UTF8 name-table
        // entry), never its own leaf name ("Int", which lives behind a
        // signature-shaped name-table entry `wire_name` doesn't render)
        // -- so `resolve_parent_name` reduces it to the bare, wrong name
        // "scala", not `None`. `lower_tasty_member_type` still tries to
        // load that guess, it just doesn't exist as a class on this
        // classpath (or anywhere), so this falls back to `Type::Error`
        // either way -- but the message below names exactly what was
        // guessed and why loading it failed, instead of the same opaque
        // string a genuinely un-nameable type tree would also produce
        // (see `ClassLoader::lower_tasty_member_type`'s doc comment).
        // Closing the underlying best-effort-reduction gap is a
        // documented follow-up, not silently claimed here.
        // `x`'s declared type is `Int`, encoded post-typecheck as a real
        // `TYPEREF_TAG` node with a `scala` `TERMREFpkg` prefix --
        // `resolve_reference_name` resolves this fully to `scala/Int`
        // (registered on this test's classpath below), so `x`'s type is
        // a genuine `Type::TypeRef` now, not a `Type::Error` guess.
        let x_type = member_type_id(&mut store, point, "x");
        let Some(symbol) = store.types.get(x_type).reference_symbol() else {
            panic!(
                "expected x's type to be a real Type::TypeRef, got {:?}",
                store.types.get(x_type)
            );
        };
        assert_eq!(symbol_name(&store, symbol), "Int");

        let init_type = member_type_id(&mut store, point, "<init>");
        let Type::Method(init_method) = store.types.get(init_type) else {
            panic!("expected <init>'s type to be a Type::Method");
        };
        assert_eq!(init_method.params.len(), 2);
        assert_eq!(
            symbol_name(&store, parent_symbol(&store, init_method.result)),
            "Unit"
        );
    }

    /// `lower_tasty_member_type`'s two fallback paths to `Type::Error`
    /// used to converge on the exact same generic message regardless of
    /// which one was actually hit; `enters_a_real_tasty_case_classs_...`
    /// above covers "a name was guessed but failed to load" end to end
    /// through a real fixture, so this covers the other path directly:
    /// `declared_type` genuinely `None` (`tasty_symbol::decode` itself
    /// could not reduce the type tree to any name at all).
    #[test]
    fn unresolved_tasty_member_type_names_the_reason_when_no_name_was_reduced_at_all() {
        let mut store = SemanticStore::new();
        let mut loader = ClassLoader::new(InMemoryClassPath(HashMap::new()), &mut store);

        let owner = BinaryName::from_internal("Owner");
        let error_type = loader.lower_tasty_member_type(&owner, None);
        drop(loader);

        let Type::Error(error) = store.types.get(error_type) else {
            panic!("expected a Type::Error");
        };
        let message = store.names.resolve(error.message);
        assert!(message.contains("could not reduce the declared type tree to a name"));
    }

    /// `tasty_symbol::decodes_a_private_class_with_a_generic_mixin_naming_its_own_enclosing_class`
    /// already proves, at the decode level, that `tasty_symbol::decode`
    /// preserves `scala3-library/scala/math/Ordering.tasty`'s real
    /// private nested `Reverse`'s `PRIVATE_TAG` — this used to prove the
    /// *loader* actually uses that too, by loading `Reverse` for real
    /// (which also loads `Ordering` itself, `Reverse`'s declared
    /// interface). `Ordering` itself mixes in a synthetic `Serializable`
    /// — the same unresolvable implicit-import reference as `BigInt`'s
    /// own `Serializable`
    /// (`tasty_symbol::decoding_fails_when_one_of_several_mixins_is_an_unresolvable_implicit_import_reference`)
    /// — so loading `Ordering` (and therefore `Reverse`) through a real
    /// `ClassLoader` now fails with `UnresolvedSupertype` instead of
    /// silently widening or narrowing any visibility, per the same
    /// explicit-unresolved-over-misleading-guess reasoning. Asserts that
    /// failure explicitly; this crate has no other real `.tasty` fixture
    /// with a non-public top-level class and no unresolvable implicit-
    /// import mixin anywhere in its dependency chain, so the loader-level
    /// (as opposed to `tasty_symbol::decode`-level, still covered by the
    /// `tasty_symbol` test named above) proof that
    /// `load_uncached_tasty`'s visibility patch is actually wired up is
    /// lost along with this fixture's usability, until either a suitable
    /// fixture is found or real scope/import resolution (issue #7)
    /// closes the underlying gap.
    #[test]
    fn loading_a_private_tasty_class_fails_when_its_own_interface_has_an_unresolved_mixin() {
        let mut tasty = HashMap::new();
        tasty.insert(
            BinaryName::from_internal("Reverse"),
            dotty_tasty_fixture_bytes("scala3-library/scala/math/Ordering.tasty"),
        );
        tasty.insert(
            BinaryName::from_internal("Ordering"),
            dotty_tasty_fixture_bytes("scala3-library/scala/math/Ordering.tasty"),
        );

        let mut classes = HashMap::new();
        for stub in [
            "java/lang/Object",
            "Comparator",
            "java/util/Comparator",
            "PartialOrdering",
            "scala/PartialOrdering",
            "scala/math/PartialOrdering",
            "Serializable",
            "scala/Serializable",
            "java/io/Serializable",
        ] {
            classes.insert(BinaryName::from_internal(stub), synthetic_class(stub, None));
        }

        let class_path = CompositeClassPath::new(vec![
            Box::new(InMemoryTastyClassPath(tasty)),
            Box::new(InMemoryClassPath(classes)),
        ]);

        let mut store = SemanticStore::new();
        let mut loader = ClassLoader::new(class_path, &mut store);
        let error = loader
            .load_class(&BinaryName::from_internal("Reverse"))
            .expect_err("Reverse should fail to load, via Ordering's own unresolved mixin");
        drop(loader);

        assert!(matches!(error, ClassLoadError::DependencyFailure { .. }));
    }

    /// The generic `Iterator` mixin in `Source.tasty` is an alias owned by
    /// `scala.package$`. The loader follows the alias definition and verifies
    /// its target against the configured classpath.
    #[test]
    fn loading_source_tasty_resolves_iterator_through_package_alias() {
        let mut tasty = HashMap::new();
        tasty.insert(
            BinaryName::from_internal("Source"),
            dotty_tasty_fixture_bytes("scala3-library/scala/io/Source.tasty"),
        );
        tasty.insert(
            BinaryName::from_internal("scala/package"),
            dotty_tasty_fixture_bytes("scala3-library/scala/package.tasty"),
        );

        let mut classes = HashMap::new();
        for stub in [
            "java/lang/Object",
            "scala/collection/Iterator",
            "java/io/Closeable",
        ] {
            classes.insert(BinaryName::from_internal(stub), synthetic_class(stub, None));
        }

        let class_path = CompositeClassPath::new(vec![
            Box::new(InMemoryTastyClassPath(tasty)),
            Box::new(InMemoryClassPath(classes)),
        ]);

        let mut store = SemanticStore::new();
        let mut loader = ClassLoader::new(class_path, &mut store);
        let source = loader
            .load_class(&BinaryName::from_internal("Source"))
            .expect("Source should load through its package-object Iterator alias");
        assert!(matches!(
            loader
                .repository
                .get(&BinaryName::from_internal("scala/collection/Iterator")),
            Some(ClassEntry::Loaded(iterator)) if *iterator != source
        ));
    }

    #[test]
    fn scala_package_term_alias_requires_a_present_classpath_target() {
        let mut tasty = HashMap::new();
        tasty.insert(
            BinaryName::from_internal("scala/package"),
            dotty_tasty_fixture_bytes("scala3-library/scala/package.tasty"),
        );
        let nil = BinaryName::from_internal("scala/collection/immutable/Nil");
        let class_path = CompositeClassPath::new(vec![
            Box::new(InMemoryTastyClassPath(tasty)),
            Box::new(InMemoryClassPath(HashMap::from([(
                nil.clone(),
                synthetic_class(nil.as_internal(), Some("java/lang/Object")),
            )]))),
        ]);
        let owner = BinaryName::from_internal("scala/package$");

        assert_eq!(
            resolve_tasty_term_alias(&class_path, &owner, "Nil").unwrap(),
            Some(nil)
        );
        assert_eq!(
            resolve_tasty_term_alias(&class_path, &owner, "NotAScalaTerm").unwrap(),
            None
        );
    }

    #[test]
    fn compiler_28_8_package_alias_tasty_is_accepted_by_the_loader() {
        let package_name = BinaryName::from_internal("dotty/tools/dotc/semanticdb/package");
        let class_path = InMemoryTastyClassPath(HashMap::from([(
            package_name,
            dotty_tasty_fixture_bytes("scala3-compiler/dotty/tools/dotc/semanticdb/package.tasty"),
        )]));
        let owner = BinaryName::from_internal("dotty/tools/dotc/semanticdb/package$");

        let target = resolve_tasty_alias_target(&class_path, &owner, "ExtractSemanticInfo")
            .expect("Scala 3.9 compiler package TASTy should be accepted")
            .expect("the pinned package TASTy contains this alias");
        assert!(matches!(
            target,
            tasty_symbol::TypeAliasTarget::Candidate(_)
                | tasty_symbol::TypeAliasTarget::Alias { .. }
        ));
    }

    #[test]
    fn compiler_package_alias_rejects_unvalidated_tasty_minor_versions() {
        let package_name = BinaryName::from_internal("dotty/tools/dotc/semanticdb/package");
        let mut bytes =
            dotty_tasty_fixture_bytes("scala3-compiler/dotty/tools/dotc/semanticdb/package.tasty");
        // The pinned compiler fixture uses the one-byte base-128 encoding for
        // each header version component. Change 28.8.0 to unvalidated 28.7.0.
        bytes[5] = 7 | 0x80;
        let class_path = InMemoryTastyClassPath(HashMap::from([(package_name, bytes)]));
        let owner = BinaryName::from_internal("dotty/tools/dotc/semanticdb/package$");

        let error = resolve_tasty_alias_target(&class_path, &owner, "ExtractSemanticInfo")
            .expect_err("an unvalidated Scala TASTy version must not be parsed as an alias");
        assert!(matches!(
            error,
            TastyAliasResolutionError::InvalidTasty(
                _,
                tasty_symbol::TastyDecodeError::UnsupportedClassloaderVersion {
                    major: 28,
                    minor: 7,
                    experimental: 0,
                },
                _,
                ClassFormat::Tasty,
            )
        ));
    }

    #[test]
    fn loading_source_reports_a_malformed_package_alias_tasty_file() {
        let resources = HashMap::from([
            (
                BinaryName::from_internal("Source"),
                (
                    dotty_tasty_fixture_bytes("scala3-library/scala/io/Source.tasty"),
                    ClassFormat::Tasty,
                    ClassOrigin::Directory(PathBuf::from("/owner")),
                ),
            ),
            (
                BinaryName::from_internal("scala/package"),
                (
                    b"malformed package object TASTy".to_vec(),
                    ClassFormat::Tasty,
                    ClassOrigin::Directory(PathBuf::from("/alias")),
                ),
            ),
        ]);

        let mut store = SemanticStore::new();
        let mut loader = ClassLoader::new(ResourceClassPath(resources), &mut store);
        let failure = loader
            .load_class_detailed(&BinaryName::from_internal("Source"))
            .expect_err("malformed package alias TASTy should be reported");
        assert!(matches!(
            failure.error,
            ClassLoadError::InvalidTastyFile(
                ref name,
                tasty_symbol::TastyDecodeError::Parse(_)
            ) if name.as_internal() == "scala/package"
        ));
        assert_eq!(
            failure.origin,
            Some(ClassOrigin::Directory(PathBuf::from("/alias")))
        );
        assert_eq!(failure.format, Some(ClassFormat::Tasty));
    }

    #[test]
    fn nested_dependency_alias_failure_preserves_cached_provenance_without_io() {
        let resources = HashMap::from([
            (
                BinaryName::from_internal("Owner"),
                (
                    synthetic_class("Owner", Some("Source")),
                    ClassFormat::Class,
                    ClassOrigin::Directory(PathBuf::from("/owner")),
                ),
            ),
            (
                BinaryName::from_internal("Source"),
                (
                    dotty_tasty_fixture_bytes("scala3-library/scala/io/Source.tasty"),
                    ClassFormat::Tasty,
                    ClassOrigin::Directory(PathBuf::from("/source")),
                ),
            ),
            (
                BinaryName::from_internal("scala/package"),
                (
                    b"malformed package object TASTy".to_vec(),
                    ClassFormat::Tasty,
                    ClassOrigin::Directory(PathBuf::from("/alias")),
                ),
            ),
        ]);
        let calls = Arc::new(AtomicUsize::new(0));
        let class_path = CountingResourceClassPath {
            resources,
            calls: Arc::clone(&calls),
        };
        let mut store = SemanticStore::new();
        let mut loader = ClassLoader::new(class_path, &mut store);

        let failure = loader
            .load_class_detailed(&BinaryName::from_internal("Owner"))
            .expect_err("the nested malformed alias resource must fail");
        assert!(matches!(
            failure.error,
            ClassLoadError::DependencyFailure { .. }
        ));
        assert!(matches!(
            failure.root_cause(),
            ClassLoadError::InvalidTastyFile(name, _) if name.as_internal() == "scala/package"
        ));
        assert_eq!(
            failure.origin,
            Some(ClassOrigin::Directory(PathBuf::from("/alias")))
        );
        assert_eq!(failure.format, Some(ClassFormat::Tasty));

        let calls_after_initial_failure = calls.load(Ordering::Relaxed);
        for name in ["Owner", "Source"] {
            let cached = loader
                .load_class_detailed(&BinaryName::from_internal(name))
                .expect_err("cached failures must retain the alias provenance");
            assert_eq!(
                cached.origin,
                Some(ClassOrigin::Directory(PathBuf::from("/alias")))
            );
            assert_eq!(cached.format, Some(ClassFormat::Tasty));
        }
        assert_eq!(calls.load(Ordering::Relaxed), calls_after_initial_failure);
    }

    #[test]
    fn package_alias_stays_unresolved_when_two_classpath_targets_match() {
        let mut tasty = HashMap::new();
        tasty.insert(
            BinaryName::from_internal("scala/package"),
            dotty_tasty_fixture_bytes("scala3-library/scala/package.tasty"),
        );
        let mut classes = HashMap::new();
        for target in ["collection/Iterator", "scala/collection/Iterator"] {
            classes.insert(
                BinaryName::from_internal(target),
                synthetic_class(target, None),
            );
        }
        let class_path = CompositeClassPath::new(vec![
            Box::new(InMemoryTastyClassPath(tasty)),
            Box::new(InMemoryClassPath(classes)),
        ]);

        assert_eq!(
            resolve_tasty_alias(
                &class_path,
                &BinaryName::from_internal("scala/package$"),
                "Iterator"
            )
            .unwrap(),
            None
        );
    }

    #[test]
    fn scala_cloneable_package_alias_resolves_to_the_java_class() {
        let tasty = HashMap::from([(
            BinaryName::from_internal("scala/package"),
            dotty_tasty_fixture_bytes("scala3-library/scala/package.tasty"),
        )]);
        let classes = HashMap::from([(
            BinaryName::from_internal("java/lang/Cloneable"),
            synthetic_class("java/lang/Cloneable", None),
        )]);
        let class_path = CompositeClassPath::new(vec![
            Box::new(InMemoryTastyClassPath(tasty)),
            Box::new(InMemoryClassPath(classes)),
        ]);

        assert_eq!(
            resolve_tasty_alias(
                &class_path,
                &BinaryName::from_internal("scala/package$"),
                "Cloneable"
            )
            .unwrap(),
            Some(BinaryName::from_internal("java/lang/Cloneable"))
        );
    }

    #[test]
    fn follows_a_two_hop_package_alias_chain() {
        let first_owner = BinaryName::from_internal("scala/package$");
        let second_owner = BinaryName::from_internal("library/package$");
        let target_owner = BinaryName::from_internal("library/package$");
        let mut aliases = HashMap::new();
        aliases.insert(
            (first_owner.clone(), "First".to_owned()),
            tasty_symbol::TypeAliasTarget::Alias {
                owner: second_owner.clone(),
                name: "Second".to_owned(),
            },
        );
        aliases.insert(
            (second_owner.clone(), "Second".to_owned()),
            tasty_symbol::TypeAliasTarget::Candidate(BinaryName::from_internal(
                "collection/Iterator",
            )),
        );

        let resolved = follow_tasty_alias_chain(first_owner, "First".to_owned(), |owner, alias| {
            Ok::<_, ()>(aliases.get(&(owner.clone(), alias.to_owned())).cloned())
        })
        .unwrap();

        assert_eq!(
            resolved,
            Some((
                target_owner,
                BinaryName::from_internal("collection/Iterator")
            ))
        );
    }

    #[test]
    fn stops_a_cyclic_package_alias_chain() {
        let first_owner = BinaryName::from_internal("scala/package$");
        let second_owner = BinaryName::from_internal("library/package$");
        let mut aliases = HashMap::new();
        aliases.insert(
            (first_owner.clone(), "First".to_owned()),
            tasty_symbol::TypeAliasTarget::Alias {
                owner: second_owner.clone(),
                name: "Second".to_owned(),
            },
        );
        aliases.insert(
            (second_owner.clone(), "Second".to_owned()),
            tasty_symbol::TypeAliasTarget::Alias {
                owner: first_owner.clone(),
                name: "First".to_owned(),
            },
        );

        let resolved = follow_tasty_alias_chain(first_owner, "First".to_owned(), |owner, alias| {
            Ok::<_, ()>(aliases.get(&(owner.clone(), alias.to_owned())).cloned())
        })
        .unwrap();

        assert_eq!(resolved, None);
    }

    #[test]
    fn circular_inheritance_is_detected_instead_of_recursing_forever() {
        // Synthetic: A's superclass is B, B's superclass is A. `javac`
        // rejects this at the source level, so it can only be produced
        // by hand-building the class files directly.
        let mut classes = HashMap::new();
        classes.insert(
            BinaryName::from_internal("A"),
            synthetic_class("A", Some("B")),
        );
        classes.insert(
            BinaryName::from_internal("B"),
            synthetic_class("B", Some("A")),
        );

        let mut store = SemanticStore::new();

        let mut loader = ClassLoader::new(InMemoryClassPath(classes), &mut store);
        let error = loader
            .load_class(&BinaryName::from_internal("A"))
            .unwrap_err();

        fn contains_circular_inheritance(error: &ClassLoadError) -> bool {
            match error {
                ClassLoadError::CircularInheritance(_) => true,
                ClassLoadError::DependencyFailure { source, .. } => {
                    contains_circular_inheritance(source)
                }
                _ => false,
            }
        }

        assert!(contains_circular_inheritance(&error));
    }

    #[test]
    fn circular_inheritance_retains_resource_context_in_cached_failures() {
        let resources = [("A", "B", "/a"), ("B", "A", "/b")]
            .into_iter()
            .map(|(name, parent, path)| {
                (
                    BinaryName::from_internal(name),
                    (
                        synthetic_class(name, Some(parent)),
                        ClassFormat::Class,
                        ClassOrigin::Directory(PathBuf::from(path)),
                    ),
                )
            })
            .collect();
        let mut store = SemanticStore::new();
        let mut loader = ClassLoader::new(ResourceClassPath(resources), &mut store);

        let failure = loader
            .load_class_detailed(&BinaryName::from_internal("A"))
            .expect_err("the synthetic superclass cycle must fail");
        assert!(matches!(
            failure.root_cause(),
            ClassLoadError::CircularInheritance(name) if name.as_internal() == "A"
        ));
        assert_eq!(
            failure.origin,
            Some(ClassOrigin::Directory(PathBuf::from("/a")))
        );
        assert_eq!(failure.format, Some(ClassFormat::Class));

        for name in ["A", "B"] {
            let cached = loader
                .load_class_detailed(&BinaryName::from_internal(name))
                .expect_err("cycle participants must retain their cached failure");
            assert!(matches!(
                cached.root_cause(),
                ClassLoadError::CircularInheritance(cause) if cause.as_internal() == "A"
            ));
            assert_eq!(
                cached.origin,
                Some(ClassOrigin::Directory(PathBuf::from("/a")))
            );
            assert_eq!(cached.format, Some(ClassFormat::Class));
        }
    }

    #[test]
    fn inheritance_ancestry_has_a_bounded_depth() {
        let mut classes = HashMap::new();
        for index in 0..=MAX_INHERITANCE_DEPTH {
            let this_name = format!("Deep{index}");
            let super_name = (index < MAX_INHERITANCE_DEPTH).then(|| format!("Deep{}", index + 1));
            classes.insert(
                BinaryName::from_internal(&this_name),
                synthetic_class(&this_name, super_name.as_deref()),
            );
        }

        let mut store = SemanticStore::new();
        let mut loader = ClassLoader::new(InMemoryClassPath(classes), &mut store);
        let error = loader
            .load_class(&BinaryName::from_internal("Deep0"))
            .expect_err("a malformed, excessively deep hierarchy is bounded");

        fn contains_depth_error(error: &ClassLoadError) -> bool {
            match error {
                ClassLoadError::InheritanceDepthExceeded(name) => {
                    name.as_internal() == format!("Deep{MAX_INHERITANCE_DEPTH}")
                }
                ClassLoadError::DependencyFailure { source, .. } => contains_depth_error(source),
                _ => false,
            }
        }

        assert!(contains_depth_error(&error), "unexpected error: {error:?}");
    }

    /// Public loads roll back allocations while retaining the cycle's typed error.
    #[test]
    fn circular_inheritance_rolls_back_both_new_symbols() {
        let mut store = SemanticStore::new();
        let mut loader = ClassLoader::new(
            InMemoryClassPath(HashMap::from([
                (
                    BinaryName::from_internal("A"),
                    synthetic_class("A", Some("B")),
                ),
                (
                    BinaryName::from_internal("B"),
                    synthetic_class("B", Some("A")),
                ),
            ])),
            &mut store,
        );
        let checkpoint = loader.store.checkpoint();
        let error = loader
            .load_class(&BinaryName::from_internal("A"))
            .unwrap_err();
        assert!(
            matches!(error.root_cause(), ClassLoadError::CircularInheritance(name) if name.as_internal() == "A")
        );
        assert_eq!(loader.store.checkpoint(), checkpoint);
        for name in ["A", "B"] {
            assert!(matches!(
                loader.repository.get(&BinaryName::from_internal(name)),
                Some(ClassEntry::Failed(None, _))
            ));
        }
    }

    #[test]
    fn bootstrap_member_dependency_may_reuse_an_in_progress_superclass_identity() {
        let object_name = BinaryName::from_internal("java/lang/Object");
        let class_name = BinaryName::from_internal("java/lang/Class");
        let mut classes = HashMap::new();
        classes.insert(
            object_name.clone(),
            synthetic_class_with_object_fields(
                object_name.as_internal(),
                None,
                &[("class", class_name.as_internal())],
            ),
        );
        classes.insert(
            class_name.clone(),
            synthetic_class(class_name.as_internal(), Some(object_name.as_internal())),
        );

        let mut store = SemanticStore::new();
        let mut loader = ClassLoader::new(InMemoryClassPath(classes), &mut store);
        let canonical_object = loader.definitions().object_class;
        let object = loader
            .load_class(&object_name)
            .expect("Object -> member Class -> superclass Object is not inheritance recursion");
        let class = loader
            .load_class(&class_name)
            .expect("Class should remain loaded after Object completes");
        drop(loader);

        assert_eq!(object, canonical_object);
        match store.symbols.get(object).info {
            SymbolInfo::Complete(info) => info,
            other => panic!("expected Object to complete, got {other:?}"),
        };
        let class_info = match store.symbols.get(class).info {
            SymbolInfo::Complete(info) => info,
            other => panic!("expected Class to complete, got {other:?}"),
        };
        let Type::ClassInfo(class_info) = store.types.get(class_info) else {
            panic!("expected Class ClassInfo");
        };
        let class_parents = class_info.parents.clone();
        assert_eq!(class_parents.len(), 1);
        assert_eq!(parent_symbol(&store, class_parents[0]), canonical_object);
        let object_class_type = member_type_id(&mut store, object, "class");
        assert_eq!(parent_symbol(&store, object_class_type), class);
    }

    /// Confirms the "reuse an in-progress shell" technique
    /// (`docs/classloader.md` §5/§9, Milestone 6): `Ping.other` is typed
    /// `Pong`, and `Pong.other` is typed `Ping` — a completely
    /// legitimate mutual reference `javac` compiles without complaint
    /// (see `tests/fixtures/ping_pong/generate.sh`), unlike the
    /// synthetic supertype cycle above, which must still hard-error.
    /// Each class completes only when explicitly requested, preserving
    /// the canonical identity already referenced by the other's signature.
    #[test]
    fn resolves_a_legitimate_mutual_field_type_reference_between_two_real_classes() {
        let mut classes = HashMap::new();
        classes.insert(
            BinaryName::from_internal("Ping"),
            own_fixture_bytes("ping_pong/Ping.class"),
        );
        classes.insert(
            BinaryName::from_internal("Pong"),
            own_fixture_bytes("ping_pong/Pong.class"),
        );
        classes.insert(
            BinaryName::from_internal("java/lang/Object"),
            synthetic_class("java/lang/Object", None),
        );

        let mut store = SemanticStore::new();

        let mut loader = ClassLoader::new(InMemoryClassPath(classes), &mut store);
        let ping = loader
            .load_class(&BinaryName::from_internal("Ping"))
            .expect("Ping should load despite the mutual field reference");
        let ping_other_type = member_type_id(loader.store, ping, "other");
        let pong = parent_symbol(loader.store, ping_other_type);
        assert_eq!(symbol_name(loader.store, pong), "Pong");
        assert_eq!(loader.store.symbols.get(pong).info, SymbolInfo::Missing);
        assert_eq!(
            loader
                .load_class(&BinaryName::from_internal("Pong"))
                .unwrap(),
            pong
        );
        drop(loader);

        let pong_other_type = member_type_id(&mut store, pong, "other");
        let ping_again = parent_symbol(&store, pong_other_type);
        assert_eq!(symbol_name(&store, ping_again), "Ping");
    }

    /// A failed root cannot publish a mutual member reference through a dependency.
    #[test]
    fn late_failure_does_not_publish_mutual_member_type_dependencies() {
        let classes = HashMap::from([
            (
                BinaryName::from_internal("MutA"),
                synthetic_class_with_object_fields(
                    "MutA",
                    None,
                    &[("other", "MutB"), ("broken", "DoesNotExist")],
                ),
            ),
            (
                BinaryName::from_internal("MutB"),
                synthetic_class_with_object_fields("MutB", None, &[("other", "MutA")]),
            ),
        ]);
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let mut loader = ClassLoader::with_definitions(
            InMemoryClassPath(classes.clone()),
            &mut store,
            definitions,
            LoadingSession::new(),
        );
        let checkpoint = loader.store.checkpoint();
        let error = loader
            .load_class(&BinaryName::from_internal("MutA"))
            .unwrap_err();
        assert!(
            matches!(error.root_cause(), ClassLoadError::NotFound(name) if name.as_internal() == "DoesNotExist")
        );
        assert_eq!(loader.store.checkpoint(), checkpoint);
        assert!(loader.session.identified.is_empty());
        let session = loader.into_session();
        let mut loader = ClassLoader::with_definitions(
            InMemoryClassPath(classes),
            &mut store,
            definitions,
            session,
        );
        let b = loader
            .load_class(&BinaryName::from_internal("MutB"))
            .unwrap();
        let other = member_type_id(loader.store, b, "other");
        let a = parent_symbol(loader.store, other);
        assert_eq!(symbol_name(loader.store, a), "MutA");
        assert_eq!(loader.store.symbols.get(a).info, SymbolInfo::Missing);
    }

    /// `lower_field_type`'s `FieldType::Array` branch recurses into its
    /// component type; every other test in this file
    /// only exercises plain `Object` fields, so this specifically
    /// targets `Ping.others: Pong[]` (real `javac` output, see
    /// `tests/fixtures/ping_pong/generate.sh`) to prove an
    /// array-of-`Object` field resolves its component class the same
    /// way a bare `Object` field does.
    #[test]
    fn resolves_the_component_class_of_an_array_typed_field() {
        let mut classes = HashMap::new();
        classes.insert(
            BinaryName::from_internal("Ping"),
            own_fixture_bytes("ping_pong/Ping.class"),
        );
        classes.insert(
            BinaryName::from_internal("Pong"),
            own_fixture_bytes("ping_pong/Pong.class"),
        );
        classes.insert(
            BinaryName::from_internal("java/lang/Object"),
            synthetic_class("java/lang/Object", None),
        );

        let mut store = SemanticStore::new();

        let mut loader = ClassLoader::new(InMemoryClassPath(classes), &mut store);
        let ping = loader
            .load_class(&BinaryName::from_internal("Ping"))
            .expect("Ping should load");
        drop(loader);

        let others_type = member_type_id(&mut store, ping, "others");
        let Type::JavaArray { element } = store.types.get(others_type) else {
            panic!("expected Ping.others to be a JavaArray type");
        };
        let pong = parent_symbol(&store, *element);

        assert_eq!(symbol_name(&store, pong), "Pong");
    }

    /// Same "reuse an in-progress shell" technique as the field-level
    /// test above, applied to a mutual *method* parameter/return type:
    /// `Ping.exchange(Pong): Pong` and `Pong.exchange(Ping): Ping`.
    #[test]
    fn resolves_a_legitimate_mutual_method_type_reference_between_two_real_classes() {
        let mut classes = HashMap::new();
        classes.insert(
            BinaryName::from_internal("Ping"),
            own_fixture_bytes("ping_pong/Ping.class"),
        );
        classes.insert(
            BinaryName::from_internal("Pong"),
            own_fixture_bytes("ping_pong/Pong.class"),
        );
        classes.insert(
            BinaryName::from_internal("java/lang/Object"),
            synthetic_class("java/lang/Object", None),
        );

        let mut store = SemanticStore::new();

        let mut loader = ClassLoader::new(InMemoryClassPath(classes), &mut store);
        let ping = loader
            .load_class(&BinaryName::from_internal("Ping"))
            .expect("Ping should load despite the mutual method reference");
        drop(loader);

        let exchange_type = member_type_id(&mut store, ping, "exchange");
        let Type::Method(method) = store.types.get(exchange_type) else {
            panic!("expected Ping.exchange to be a Type::Method");
        };
        let &[parameter] = method.params.as_slice() else {
            panic!(
                "expected exactly one parameter, got {}",
                method.params.len()
            );
        };
        let parameter_symbol = parent_symbol(&store, parameter.ty);
        let return_symbol = parent_symbol(&store, method.result);

        assert_eq!(symbol_name(&store, parameter_symbol), "Pong");
        assert_eq!(symbol_name(&store, return_symbol), "Pong");
    }

    /// `nested_sample/NestedSample.class` (real `javac` output, see
    /// `docs/classloader.md` §9 Milestone 7) is a nest host with two
    /// members: `NestedSample$Inner` (a static member class) and
    /// `NestedSample$1LocalRunnable` (an anonymous local class
    /// implementing `Runnable`). Loading `NestedSample` also eagerly
    /// resolves `max`'s erased-to-bound `Comparable` parameter/return
    /// type and `makeLocalRunnable`'s `String` parameter/`Runnable`
    /// return type, so those need synthetic stand-ins too, alongside
    /// `java/lang/Object`.
    fn nested_sample_classpath() -> HashMap<BinaryName, Vec<u8>> {
        let mut classes = HashMap::new();
        classes.insert(
            BinaryName::from_internal("NestedSample"),
            fixture_bytes("nested_sample/NestedSample.class"),
        );
        classes.insert(
            BinaryName::from_internal("NestedSample$Inner"),
            fixture_bytes("nested_sample/NestedSample$Inner.class"),
        );
        classes.insert(
            BinaryName::from_internal("NestedSample$1LocalRunnable"),
            fixture_bytes("nested_sample/NestedSample$1LocalRunnable.class"),
        );
        classes.insert(
            BinaryName::from_internal("java/lang/Object"),
            synthetic_class("java/lang/Object", None),
        );
        classes.insert(
            BinaryName::from_internal("java/lang/Runnable"),
            synthetic_class("java/lang/Runnable", None),
        );
        classes.insert(
            BinaryName::from_internal("java/lang/String"),
            synthetic_class("java/lang/String", None),
        );
        classes.insert(
            BinaryName::from_internal("java/lang/Comparable"),
            synthetic_class("java/lang/Comparable", None),
        );
        classes
    }

    #[test]
    fn profiles_enclosing_owner_identity_reached_from_a_member_type() {
        let mut classes = nested_sample_classpath();
        classes.insert(
            BinaryName::from_internal("UsesInner"),
            signature_class(
                "UsesInner",
                Some("java/lang/Object"),
                &[],
                None,
                &[("inner", "LNestedSample$Inner;", None)],
                &[],
            ),
        );
        let mut store = SemanticStore::new();
        let mut loader = ClassLoader::new(InMemoryClassPath(classes), &mut store);
        loader.set_load_profile_collection(true);

        loader
            .load_class(&BinaryName::from_internal("UsesInner"))
            .expect("UsesInner should load through its nested member type");

        let profile = loader.take_load_profiles().pop().unwrap();
        assert!(profile.dependency_edges.iter().any(|edge| {
            edge.owner == "UsesInner"
                && edge.dependency == "NestedSample$Inner"
                && edge.reason == ClassLoadDependencyReason::MemberTypeIdentity
        }));
        assert!(profile.dependency_edges.iter().any(|edge| {
            edge.owner == "NestedSample$Inner"
                && edge.dependency == "NestedSample"
                && edge.reason == ClassLoadDependencyReason::EnclosingOwnerIdentity
        }));
        assert!(profile.classes.contains("NestedSample$Inner"));
        assert!(profile.classes.contains("NestedSample"));
    }

    /// `NestedSample.max`'s real declaration is
    /// `public static <T extends Comparable<T>> T max(T a, T b) throws
    /// IllegalStateException` — a generic static method whose erased
    /// descriptor is `(Ljava/lang/Comparable;Ljava/lang/Comparable;)
    /// Ljava/lang/Comparable;` and whose real `Signature` attribute is
    /// `<T::Ljava/lang/Comparable<TT;>;>(TT;TT;)TT;` (confirmed via
    /// `javap -p -v`) — so it lowers to a real `Type::Poly` wrapping a
    /// `Type::Method` whose params/result reference `T` via `ParamRef`,
    /// not the erased `Comparable` the descriptor alone would give.
    #[test]
    fn lowers_a_real_fixtures_generic_static_method_into_a_poly_wrapped_method() {
        let mut store = SemanticStore::new();
        let mut loader = ClassLoader::new(InMemoryClassPath(nested_sample_classpath()), &mut store);
        let class_symbol = loader
            .load_class(&BinaryName::from_internal("NestedSample"))
            .expect("NestedSample should load");
        drop(loader);

        let max_type = member_type_id(&mut store, class_symbol, "max");
        let Type::Poly(poly) = store.types.get(max_type) else {
            panic!("expected max's type to be a Type::Poly");
        };
        assert_eq!(poly.params.len(), 1);
        let type_param = poly.params[0];
        assert_eq!(store.names.resolve(type_param.name.as_name().text()), "T");
        let method_result = poly.result;

        // T's bound is the implicit Object intersected with its one
        // interface bound, Comparable[T] -- same shape as a class-level
        // F-bounded type parameter (`class_bound: None` means Object).
        let Type::And { left, right } = store.types.get(type_param.bounds) else {
            panic!("expected T's bound to be an intersection with the implicit Object bound");
        };
        assert_eq!(symbol_name(&store, parent_symbol(&store, *left)), "Object");
        let Type::Applied { tycon, args } = store.types.get(*right) else {
            panic!("expected T's interface bound to be an Applied Comparable[T]");
        };
        assert_eq!(
            symbol_name(&store, parent_symbol(&store, *tycon)),
            "Comparable"
        );
        assert_eq!(
            store.symbols.get(parent_symbol(&store, *tycon)).info,
            SymbolInfo::Missing,
            "F-bounded class/method signatures require only Comparable's identity"
        );
        assert_eq!(args.len(), 1);
        assert_eq!(
            store.types.get(args[0]),
            &Type::ParamRef {
                binder: max_type,
                index: 0
            }
        );

        let Type::Method(method) = store.types.get(method_result) else {
            panic!("expected max's Poly result to be a Type::Method");
        };
        assert_eq!(method.params.len(), 2);
        assert_eq!(
            store.names.resolve(method.params[0].name.as_name().text()),
            "a"
        );
        assert_eq!(
            store.names.resolve(method.params[1].name.as_name().text()),
            "b"
        );
        for param in &method.params {
            assert_eq!(
                store.types.get(param.ty),
                &Type::ParamRef {
                    binder: max_type,
                    index: 0
                }
            );
        }
        assert_eq!(
            store.types.get(method.result),
            &Type::ParamRef {
                binder: max_type,
                index: 0
            }
        );
    }

    /// Confirms `NestMembers` (JVMS §4.7.29) names both of
    /// `NestedSample`'s real members. Kept `Unresolved` deliberately —
    /// see `resolve_nest_members`'s doc comment — so this only checks
    /// the names, not that the members were loaded.
    #[test]
    fn resolves_the_nest_members_of_a_real_nest_host() {
        let mut store = SemanticStore::new();
        let mut loader = ClassLoader::new(InMemoryClassPath(nested_sample_classpath()), &mut store);
        let symbol = loader
            .load_class(&BinaryName::from_internal("NestedSample"))
            .expect("NestedSample should load");

        let metadata = loader
            .metadata(symbol)
            .expect("NestedSample should have classfile metadata");
        let mut member_names: Vec<String> = metadata
            .nest_members
            .iter()
            .map(|member| match member {
                ClassRef::Unresolved(name) => name.as_internal().to_owned(),
                unexpected => panic!("expected an unresolved nest member ref, got {unexpected:?}"),
            })
            .collect();
        member_names.sort_unstable();

        assert_eq!(
            member_names,
            vec!["NestedSample$1LocalRunnable", "NestedSample$Inner"]
        );
    }

    /// Confirms `NestHost` (JVMS §4.7.28) names a real member's host.
    /// Kept `Unresolved` deliberately — see `resolve_optional_nest_host`'s
    /// doc comment for why this attribute is never eagerly loaded.
    #[test]
    fn resolves_the_nest_host_of_a_real_nest_member() {
        let mut store = SemanticStore::new();
        let mut loader = ClassLoader::new(InMemoryClassPath(nested_sample_classpath()), &mut store);
        let symbol = loader
            .load_class(&BinaryName::from_internal("NestedSample$Inner"))
            .expect("NestedSample$Inner should load");

        let metadata = loader
            .metadata(symbol)
            .expect("NestedSample$Inner should have classfile metadata");
        assert!(matches!(
            &metadata.nest_host,
            Some(ClassRef::Unresolved(name)) if name.as_internal() == "NestedSample"
        ));
    }

    /// `sealed_record_sample/Shape.class` (real `javac` output) is a
    /// `sealed interface Shape permits Circle, Square` — this confirms
    /// `PermittedSubclasses` (JVMS §4.7.31) names both real permitted
    /// subclasses, without loading them (deliberately: see
    /// `resolve_permitted_subclasses`'s doc comment for why that would
    /// collide with cycle detection here, since both are records
    /// implementing `Shape` itself).
    #[test]
    fn resolves_the_permitted_subclasses_of_a_real_sealed_interface() {
        let mut classes = HashMap::new();
        classes.insert(
            BinaryName::from_internal("Shape"),
            fixture_bytes("sealed_record_sample/Shape.class"),
        );
        classes.insert(
            BinaryName::from_internal("java/lang/Object"),
            synthetic_class("java/lang/Object", None),
        );

        let mut store = SemanticStore::new();

        let mut loader = ClassLoader::new(InMemoryClassPath(classes), &mut store);
        let symbol = loader
            .load_class(&BinaryName::from_internal("Shape"))
            .expect("Shape should load");

        let metadata = loader
            .metadata(symbol)
            .expect("Shape should have classfile metadata");
        let mut permitted_names: Vec<String> = metadata
            .permitted_subclasses
            .iter()
            .map(|permitted| match permitted {
                ClassRef::Unresolved(name) => name.as_internal().to_owned(),
                unexpected => {
                    panic!("expected an unresolved permitted-subclass ref, got {unexpected:?}")
                }
            })
            .collect();
        permitted_names.sort_unstable();

        assert_eq!(permitted_names, vec!["Shape$Circle", "Shape$Square"]);
    }

    /// `nested_sample/NestedSample.class`'s real `InnerClasses`
    /// attribute (JVMS §4.7.6) has two entries in different shapes,
    /// confirmed via `javap -p -v`: a `public static` member class
    /// (`Inner`, with an outer class and a simple name) and a local
    /// class (`LocalRunnable`, with a simple name but no outer class —
    /// local/anonymous classes have no enclosing-class reference of
    /// their own in this attribute).
    #[test]
    fn resolves_the_inner_classes_of_a_real_nest_host() {
        let mut store = SemanticStore::new();
        let mut loader = ClassLoader::new(InMemoryClassPath(nested_sample_classpath()), &mut store);
        let symbol = loader
            .load_class(&BinaryName::from_internal("NestedSample"))
            .expect("NestedSample should load");

        let metadata = loader
            .metadata(symbol)
            .expect("NestedSample should have classfile metadata");
        let entries = &metadata.inner_classes;
        let inner = |name: &str| {
            entries
                .iter()
                .find(|entry| match &entry.inner_class {
                    ClassRef::Unresolved(inner_name) => inner_name.as_internal() == name,
                    ClassRef::Resolved(_) => false,
                })
                .unwrap_or_else(|| panic!("expected an InnerClasses entry for {name}"))
        };

        let member = inner("NestedSample$Inner");
        assert_eq!(member.inner_name.as_deref(), Some("Inner"));
        assert!(matches!(
            &member.outer_class,
            Some(ClassRef::Unresolved(name)) if name.as_internal() == "NestedSample"
        ));
        assert!(member.access_flags.is_public());

        let local = inner("NestedSample$1LocalRunnable");
        assert_eq!(local.inner_name.as_deref(), Some("LocalRunnable"));
        assert!(local.outer_class.is_none());
    }

    /// `nested_sample/NestedSample$1LocalRunnable.class`'s real
    /// `EnclosingMethod` attribute (JVMS §4.7.7), confirmed via
    /// `javap -p -v`: this anonymous local class is enclosed by
    /// `NestedSample.makeLocalRunnable(String):Runnable`.
    #[test]
    fn resolves_the_enclosing_method_of_a_real_local_class() {
        let mut store = SemanticStore::new();
        let mut loader = ClassLoader::new(InMemoryClassPath(nested_sample_classpath()), &mut store);
        let symbol = loader
            .load_class(&BinaryName::from_internal("NestedSample$1LocalRunnable"))
            .expect("NestedSample$1LocalRunnable should load");

        let metadata = loader
            .metadata(symbol)
            .expect("NestedSample$1LocalRunnable should have classfile metadata");
        let enclosing = metadata
            .enclosing_method
            .as_ref()
            .expect("NestedSample$1LocalRunnable should have an EnclosingMethod attribute");

        assert!(matches!(
            &enclosing.class,
            ClassRef::Unresolved(name) if name.as_internal() == "NestedSample"
        ));
        assert_eq!(
            enclosing.method,
            Some((
                "makeLocalRunnable".to_owned(),
                "(Ljava/lang/String;)Ljava/lang/Runnable;".to_owned()
            ))
        );
    }

    /// A top-level class's `Symbol::owner` stays its package — this real
    /// `javac` fixture has no `InnerClasses`/`EnclosingMethod` attribute
    /// naming itself, so `resolve_semantic_owner` finds nothing to patch.
    #[test]
    fn a_top_level_classs_owner_is_still_its_package() {
        let mut store = SemanticStore::new();
        let mut loader = ClassLoader::new(InMemoryClassPath(nested_sample_classpath()), &mut store);
        let symbol = loader
            .load_class(&BinaryName::from_internal("NestedSample"))
            .expect("NestedSample should load");

        let owner = store
            .symbols
            .get(symbol)
            .owner
            .expect("a loaded top-level class always has a package owner");
        assert_eq!(store.symbols.get(owner).kind, SymbolKind::Package);
    }

    /// `NestedSample$Inner`'s own `InnerClasses` self-entry carries an
    /// `outer_class` (confirmed via `javap -p -v`, see
    /// `resolves_the_inner_classes_of_a_real_nest_host`'s doc comment), so
    /// its `Symbol::owner` becomes `NestedSample`'s own `SymbolId` — not
    /// the `""` (unnamed) package `BinaryName::package_path` would derive
    /// from its JVM `$`-joined binary name alone.
    #[test]
    fn a_member_classs_owner_is_its_real_enclosing_class_not_its_package() {
        let mut store = SemanticStore::new();
        let mut loader = ClassLoader::new(InMemoryClassPath(nested_sample_classpath()), &mut store);
        let outer = loader
            .load_class(&BinaryName::from_internal("NestedSample"))
            .expect("NestedSample should load");
        let inner = loader
            .load_class(&BinaryName::from_internal("NestedSample$Inner"))
            .expect("NestedSample$Inner should load");

        assert_eq!(store.symbols.get(inner).owner, Some(outer));
    }

    /// `NestedSample$1LocalRunnable`'s own `InnerClasses` self-entry has
    /// no `outer_class` (it's local, not a member class), so
    /// `resolve_semantic_owner` falls back to its `EnclosingMethod`
    /// attribute's `class` reference — still `NestedSample`, not the
    /// specific `makeLocalRunnable` method (a deliberate simplification,
    /// see `resolve_semantic_owner`'s doc comment).
    #[test]
    fn a_local_classs_owner_is_its_enclosing_class_via_enclosing_method() {
        let mut store = SemanticStore::new();
        let mut loader = ClassLoader::new(InMemoryClassPath(nested_sample_classpath()), &mut store);
        let outer = loader
            .load_class(&BinaryName::from_internal("NestedSample"))
            .expect("NestedSample should load");
        let local = loader
            .load_class(&BinaryName::from_internal("NestedSample$1LocalRunnable"))
            .expect("NestedSample$1LocalRunnable should load");

        assert_eq!(store.symbols.get(local).owner, Some(outer));
    }

    /// `Symbol::owner` becomes the enclosing class, but
    /// `Visibility::Package` must not follow it: a package-private nested
    /// class is visible everywhere in its *package*, not only from within
    /// its enclosing class. `NestedSample$1LocalRunnable` is package-
    /// private (confirmed via `javap -p`: no `public` modifier) while its
    /// new semantic owner `NestedSample` is `public` — so this also
    /// guards against a regression that conflates the two, which
    /// `assert_eq!(visibility, owner's visibility)` would not catch here
    /// since the two happen to differ.
    #[test]
    fn a_nested_classs_package_visibility_still_names_the_real_package_not_the_enclosing_class() {
        let mut store = SemanticStore::new();
        let mut loader = ClassLoader::new(InMemoryClassPath(nested_sample_classpath()), &mut store);
        let outer = loader
            .load_class(&BinaryName::from_internal("NestedSample"))
            .expect("NestedSample should load");
        let local = loader
            .load_class(&BinaryName::from_internal("NestedSample$1LocalRunnable"))
            .expect("NestedSample$1LocalRunnable should load");

        let local_symbol = store.symbols.get(local);
        assert_eq!(local_symbol.owner, Some(outer));

        let Visibility::Package(package) = local_symbol.visibility else {
            panic!(
                "expected NestedSample$1LocalRunnable to be package-private, got {:?}",
                local_symbol.visibility
            );
        };
        assert_ne!(
            package, outer,
            "the package-private boundary must be the real package, not the enclosing class"
        );
        assert_eq!(store.symbols.get(package).kind, SymbolKind::Package);
    }

    /// `sealed_record_sample/Shape$Circle.class` (real `javac` output) is
    /// `record Circle(double radius) implements Shape` — confirms
    /// `Record` (JVMS §4.7.30) decodes its one real, non-generic
    /// component.
    #[test]
    fn resolves_the_record_components_of_a_real_record_class() {
        let mut classes = HashMap::new();
        classes.insert(
            BinaryName::from_internal("Shape$Circle"),
            fixture_bytes("sealed_record_sample/Shape$Circle.class"),
        );
        classes.insert(
            BinaryName::from_internal("Shape"),
            fixture_bytes("sealed_record_sample/Shape.class"),
        );
        classes.insert(
            BinaryName::from_internal("java/lang/Object"),
            synthetic_class("java/lang/Object", None),
        );
        classes.insert(
            BinaryName::from_internal("java/lang/String"),
            synthetic_class("java/lang/String", None),
        );
        classes.insert(
            BinaryName::from_internal("java/lang/Record"),
            synthetic_class("java/lang/Record", None),
        );

        let mut store = SemanticStore::new();

        let mut loader = ClassLoader::new(InMemoryClassPath(classes), &mut store);
        let symbol = loader
            .load_class(&BinaryName::from_internal("Shape$Circle"))
            .expect("Shape$Circle should load");

        let metadata = loader
            .metadata(symbol)
            .expect("Shape$Circle should have classfile metadata");
        let components = metadata
            .record_components
            .as_ref()
            .expect("Shape$Circle should have a Record attribute");

        assert!(matches!(
            components.as_slice(),
            [only] if only.name() == "radius" && only.field_type() == &FieldType::Double
        ));
    }

    /// `sealed_record_sample/Shape.class` is the sealed *interface*, not
    /// a record — confirms `record_components()` distinguishes "not a
    /// record" (`None`) from "a record with zero components"
    /// (`Some(vec![])`).
    #[test]
    fn a_non_record_class_has_no_record_components() {
        let mut classes = HashMap::new();
        classes.insert(
            BinaryName::from_internal("Shape"),
            fixture_bytes("sealed_record_sample/Shape.class"),
        );
        classes.insert(
            BinaryName::from_internal("java/lang/Object"),
            synthetic_class("java/lang/Object", None),
        );

        let mut store = SemanticStore::new();

        let mut loader = ClassLoader::new(InMemoryClassPath(classes), &mut store);
        let symbol = loader
            .load_class(&BinaryName::from_internal("Shape"))
            .expect("Shape should load");

        let metadata = loader
            .metadata(symbol)
            .expect("Shape should have classfile metadata");
        assert!(metadata.record_components.is_none());
    }

    /// `nested_sample/NestedSample$Inner.class`'s real
    /// `@Deprecated(since = "1.0", forRemoval = true)` annotation
    /// (confirmed via `javap -p -v`) exercises `RuntimeVisibleAnnotations`
    /// (JVMS §4.7.16) end to end: since `java/lang/Deprecated` is on the
    /// classpath, its type resolves to a real `SymbolId` (unlike
    /// `resolves_every_element_value_variant_from_a_synthetic_annotation`'s
    /// unresolvable annotation type below), so the class's own `Symbol`
    /// also carries a matching real `dotty_core::Annotation`, and the raw
    /// sidecar's `String`/`Boolean` element values decode correctly.
    #[test]
    fn resolves_a_real_annotation_on_a_class() {
        let mut classes = nested_sample_classpath();
        classes.insert(
            BinaryName::from_internal("java/lang/Deprecated"),
            synthetic_class("java/lang/Deprecated", None),
        );

        let mut store = SemanticStore::new();

        let mut loader = ClassLoader::new(InMemoryClassPath(classes), &mut store);
        let symbol = loader
            .load_class(&BinaryName::from_internal("NestedSample$Inner"))
            .expect("NestedSample$Inner should load");

        let metadata = loader
            .metadata(symbol)
            .expect("NestedSample$Inner should have classfile metadata");
        let annotations = &metadata.annotations;
        let deprecated_symbol = annotations
            .iter()
            .find_map(|annotation| match &annotation.annotation_type {
                ClassRef::Resolved(annotation_symbol) => Some(*annotation_symbol),
                ClassRef::Unresolved(_) => None,
            })
            .expect("NestedSample$Inner should carry a resolved @Deprecated annotation");
        assert_eq!(
            loader.store.symbols.get(deprecated_symbol).info,
            SymbolInfo::Missing
        );
        assert!(loader.metadata(deprecated_symbol).is_none());
        assert!(
            !loader
                .session
                .resolved
                .contains_key(&BinaryName::from_internal("java/lang/Deprecated"))
        );
        let deprecated = annotations
            .iter()
            .find(|annotation| annotation.annotation_type == ClassRef::Resolved(deprecated_symbol))
            .expect("NestedSample$Inner should carry a @Deprecated annotation");

        let element = |name: &str| {
            deprecated
                .elements
                .iter()
                .find(|(element_name, _)| element_name == name)
                .map(|(_, value)| value)
                .unwrap_or_else(|| panic!("expected a {name} element"))
        };

        assert!(matches!(
            element("since"),
            AnnotationValue::String(value) if value == "1.0"
        ));
        assert!(matches!(
            element("forRemoval"),
            AnnotationValue::Boolean(true)
        ));

        drop(loader);
        assert_eq!(symbol_name(&store, deprecated_symbol), "Deprecated");

        let symbol_annotations = &store.symbols.get(symbol).annotations;
        assert_eq!(symbol_annotations.len(), 1);
        let core_annotation = store.annotations.get(symbol_annotations[0]);
        assert_eq!(core_annotation.tree, None);
        assert_eq!(parent_symbol(&store, core_annotation.ty), deprecated_symbol);
        assert_eq!(
            store.symbols.get(deprecated_symbol).info,
            SymbolInfo::Missing
        );
    }

    /// A hand-built, minimal, synthetic class file carrying one
    /// `RuntimeVisibleAnnotations` annotation with one element of every
    /// `ElementValue` tag kind (JVMS §4.7.16.1) that no real fixture
    /// exercises: `Byte`/`Char`/`Int`/`Long`/`Float`/`Double`/`Short`/
    /// `Enum`/`Class`/nested `Annotation`/`Array`. `javac` cannot
    /// produce every one of these in a single real annotation (a
    /// user-defined annotation type's element kinds are fixed by its
    /// declaration), so this closes the gap left after Milestone 7's
    /// `@Deprecated`-based test only covered `String`/`Boolean`, the
    /// same way other `synthetic_class_with_*` helpers in this module
    /// hand-build bytes for shapes `javac` can't produce.
    fn synthetic_class_with_varied_annotation_element_values() -> Vec<u8> {
        fn push_utf8(pool: &mut Vec<u8>, next_index: &mut u16, value: &str) -> u16 {
            let index = *next_index;
            pool.push(1); // CONSTANT_Utf8
            pool.extend_from_slice(&(value.len() as u16).to_be_bytes());
            pool.extend_from_slice(value.as_bytes());
            *next_index += 1;
            index
        }
        fn push_class(pool: &mut Vec<u8>, next_index: &mut u16, name_index: u16) -> u16 {
            let index = *next_index;
            pool.push(7); // CONSTANT_Class
            pool.extend_from_slice(&name_index.to_be_bytes());
            *next_index += 1;
            index
        }
        fn push_integer(pool: &mut Vec<u8>, next_index: &mut u16, value: i32) -> u16 {
            let index = *next_index;
            pool.push(3); // CONSTANT_Integer
            pool.extend_from_slice(&value.to_be_bytes());
            *next_index += 1;
            index
        }
        fn push_long(pool: &mut Vec<u8>, next_index: &mut u16, value: i64) -> u16 {
            let index = *next_index;
            pool.push(5); // CONSTANT_Long
            pool.extend_from_slice(&value.to_be_bytes());
            *next_index += 2; // a Long "counts as two entries" per JVMS §4.4.5
            index
        }
        fn push_float(pool: &mut Vec<u8>, next_index: &mut u16, value: f32) -> u16 {
            let index = *next_index;
            pool.push(4); // CONSTANT_Float
            pool.extend_from_slice(&value.to_be_bytes());
            *next_index += 1;
            index
        }
        fn push_double(pool: &mut Vec<u8>, next_index: &mut u16, value: f64) -> u16 {
            let index = *next_index;
            pool.push(6); // CONSTANT_Double
            pool.extend_from_slice(&value.to_be_bytes());
            *next_index += 2; // a Double "counts as two entries" per JVMS §4.4.5
            index
        }

        let mut pool = Vec::new();
        let mut next_index: u16 = 1;

        let class_name_index = push_utf8(&mut pool, &mut next_index, "C");
        let this_class_index = push_class(&mut pool, &mut next_index, class_name_index);

        let annotation_type_index = push_utf8(&mut pool, &mut next_index, "LTag;");

        let byte_name = push_utf8(&mut pool, &mut next_index, "byteField");
        let byte_value = push_integer(&mut pool, &mut next_index, 7);

        let char_name = push_utf8(&mut pool, &mut next_index, "charField");
        let char_value = push_integer(&mut pool, &mut next_index, 'A' as i32);

        let int_name = push_utf8(&mut pool, &mut next_index, "intField");
        let int_value = push_integer(&mut pool, &mut next_index, 123_456);

        let long_name = push_utf8(&mut pool, &mut next_index, "longField");
        let long_value = push_long(&mut pool, &mut next_index, 9_000_000_000);

        let float_name = push_utf8(&mut pool, &mut next_index, "floatField");
        let float_value = push_float(&mut pool, &mut next_index, 3.5);

        let double_name = push_utf8(&mut pool, &mut next_index, "doubleField");
        let double_value = push_double(&mut pool, &mut next_index, 2.5);

        let short_name = push_utf8(&mut pool, &mut next_index, "shortField");
        let short_value = push_integer(&mut pool, &mut next_index, 9);

        let class_name = push_utf8(&mut pool, &mut next_index, "classField");
        let class_value = push_utf8(&mut pool, &mut next_index, "Ljava/lang/String;");

        let enum_name = push_utf8(&mut pool, &mut next_index, "enumField");
        let enum_type = push_utf8(&mut pool, &mut next_index, "LKind;");
        let enum_const = push_utf8(&mut pool, &mut next_index, "FOO");

        let annotation_name = push_utf8(&mut pool, &mut next_index, "annotationField");
        let nested_type = push_utf8(&mut pool, &mut next_index, "LNested;");
        let nested_element_name = push_utf8(&mut pool, &mut next_index, "value");
        let nested_string_value = push_utf8(&mut pool, &mut next_index, "nested-value");

        let array_name = push_utf8(&mut pool, &mut next_index, "arrayField");
        let array_element_1 = push_integer(&mut pool, &mut next_index, 1);
        let array_element_2 = push_integer(&mut pool, &mut next_index, 2);
        let array_element_3 = push_integer(&mut pool, &mut next_index, 3);

        let attribute_name_index =
            push_utf8(&mut pool, &mut next_index, "RuntimeVisibleAnnotations");

        let mut annotation_bytes = Vec::new();
        annotation_bytes.extend_from_slice(&annotation_type_index.to_be_bytes());
        annotation_bytes.extend_from_slice(&11u16.to_be_bytes()); // num_element_value_pairs

        annotation_bytes.extend_from_slice(&byte_name.to_be_bytes());
        annotation_bytes.push(b'B');
        annotation_bytes.extend_from_slice(&byte_value.to_be_bytes());

        annotation_bytes.extend_from_slice(&char_name.to_be_bytes());
        annotation_bytes.push(b'C');
        annotation_bytes.extend_from_slice(&char_value.to_be_bytes());

        annotation_bytes.extend_from_slice(&int_name.to_be_bytes());
        annotation_bytes.push(b'I');
        annotation_bytes.extend_from_slice(&int_value.to_be_bytes());

        annotation_bytes.extend_from_slice(&long_name.to_be_bytes());
        annotation_bytes.push(b'J');
        annotation_bytes.extend_from_slice(&long_value.to_be_bytes());

        annotation_bytes.extend_from_slice(&float_name.to_be_bytes());
        annotation_bytes.push(b'F');
        annotation_bytes.extend_from_slice(&float_value.to_be_bytes());

        annotation_bytes.extend_from_slice(&double_name.to_be_bytes());
        annotation_bytes.push(b'D');
        annotation_bytes.extend_from_slice(&double_value.to_be_bytes());

        annotation_bytes.extend_from_slice(&short_name.to_be_bytes());
        annotation_bytes.push(b'S');
        annotation_bytes.extend_from_slice(&short_value.to_be_bytes());

        annotation_bytes.extend_from_slice(&class_name.to_be_bytes());
        annotation_bytes.push(b'c');
        annotation_bytes.extend_from_slice(&class_value.to_be_bytes());

        annotation_bytes.extend_from_slice(&enum_name.to_be_bytes());
        annotation_bytes.push(b'e');
        annotation_bytes.extend_from_slice(&enum_type.to_be_bytes());
        annotation_bytes.extend_from_slice(&enum_const.to_be_bytes());

        annotation_bytes.extend_from_slice(&annotation_name.to_be_bytes());
        annotation_bytes.push(b'@');
        annotation_bytes.extend_from_slice(&nested_type.to_be_bytes());
        annotation_bytes.extend_from_slice(&1u16.to_be_bytes()); // nested num_element_value_pairs
        annotation_bytes.extend_from_slice(&nested_element_name.to_be_bytes());
        annotation_bytes.push(b's');
        annotation_bytes.extend_from_slice(&nested_string_value.to_be_bytes());

        annotation_bytes.extend_from_slice(&array_name.to_be_bytes());
        annotation_bytes.push(b'[');
        annotation_bytes.extend_from_slice(&3u16.to_be_bytes()); // num_values
        for element in [array_element_1, array_element_2, array_element_3] {
            annotation_bytes.push(b'I');
            annotation_bytes.extend_from_slice(&element.to_be_bytes());
        }

        let mut attribute_body = Vec::new();
        attribute_body.extend_from_slice(&1u16.to_be_bytes()); // num_annotations
        attribute_body.extend_from_slice(&annotation_bytes);

        let mut bytes = Vec::new();
        bytes.extend_from_slice(&[0xCA, 0xFE, 0xBA, 0xBE]); // magic
        bytes.extend_from_slice(&[0x00, 0x00]); // minor
        bytes.extend_from_slice(&[0x00, 0x45]); // class-file major = 69
        bytes.extend_from_slice(&next_index.to_be_bytes()); // constant_pool_count
        bytes.extend_from_slice(&pool);
        bytes.extend_from_slice(&[0x00, 0x21]); // access_flags
        bytes.extend_from_slice(&this_class_index.to_be_bytes());
        bytes.extend_from_slice(&[0x00, 0x00]); // super_class = none
        bytes.extend_from_slice(&[0x00, 0x00]); // interfaces_count
        bytes.extend_from_slice(&[0x00, 0x00]); // fields_count
        bytes.extend_from_slice(&[0x00, 0x00]); // methods_count
        bytes.extend_from_slice(&[0x00, 0x01]); // attributes_count = 1
        bytes.extend_from_slice(&attribute_name_index.to_be_bytes());
        bytes.extend_from_slice(&(attribute_body.len() as u32).to_be_bytes());
        bytes.extend_from_slice(&attribute_body);
        bytes
    }

    #[test]
    fn resolves_every_element_value_variant_from_a_synthetic_annotation() {
        let mut classes = HashMap::new();
        classes.insert(
            BinaryName::from_internal("C"),
            synthetic_class_with_varied_annotation_element_values(),
        );

        let mut store = SemanticStore::new();

        let mut loader = ClassLoader::new(InMemoryClassPath(classes), &mut store);
        let symbol = loader
            .load_class(&BinaryName::from_internal("C"))
            .expect("C should load");

        let metadata = loader
            .metadata(symbol)
            .expect("C should have classfile metadata");
        let annotations = &metadata.annotations;
        let tag = annotations
            .first()
            .expect("C should carry the synthetic annotation");
        assert!(matches!(
            &tag.annotation_type,
            ClassRef::Unresolved(name) if name.as_internal() == "Tag"
        ));

        let element = |name: &str| {
            tag.elements
                .iter()
                .find(|(element_name, _)| element_name == name)
                .map(|(_, value)| value)
                .unwrap_or_else(|| panic!("expected a {name} element"))
        };

        assert!(matches!(element("byteField"), AnnotationValue::Byte(7)));
        assert!(matches!(
            element("charField"),
            AnnotationValue::Char(value) if *value == 'A' as i32
        ));
        assert!(matches!(element("intField"), AnnotationValue::Int(123_456)));
        assert!(matches!(
            element("longField"),
            AnnotationValue::Long(9_000_000_000)
        ));
        assert!(matches!(
            element("floatField"),
            AnnotationValue::Float(value) if *value == 3.5
        ));
        assert!(matches!(
            element("doubleField"),
            AnnotationValue::Double(value) if *value == 2.5
        ));
        assert!(matches!(element("shortField"), AnnotationValue::Short(9)));
        assert!(matches!(
            element("classField"),
            AnnotationValue::Class(value) if value == "Ljava/lang/String;"
        ));
        assert!(matches!(
            element("enumField"),
            AnnotationValue::Enum { type_descriptor, const_name }
                if type_descriptor == "LKind;" && const_name == "FOO"
        ));

        match element("annotationField") {
            AnnotationValue::Annotation(nested) => {
                assert!(matches!(
                    &nested.annotation_type,
                    ClassRef::Unresolved(name) if name.as_internal() == "Nested"
                ));
                assert!(matches!(
                    nested.elements.as_slice(),
                    [(name, AnnotationValue::String(value))]
                        if name == "value" && value == "nested-value"
                ));
            }
            unexpected => panic!("expected a nested annotation, got {unexpected:?}"),
        }

        match element("arrayField") {
            AnnotationValue::Array(values) => {
                let ints: Vec<i32> = values
                    .iter()
                    .map(|value| match value {
                        AnnotationValue::Int(value) => *value,
                        unexpected => panic!("expected an Int array element, got {unexpected:?}"),
                    })
                    .collect();
                assert_eq!(ints, vec![1, 2, 3]);
            }
            unexpected => panic!("expected an array element value, got {unexpected:?}"),
        }

        // "Tag" is not on this test's classpath, so it stays
        // `ClassRef::Unresolved` (per `resolve_annotation`'s doc comment)
        // and never gets a `dotty_core::Annotation` on the class's own
        // `Symbol` -- an annotation whose interface cannot be loaded is
        // not a class-load failure, but it also cannot become a real,
        // typed `Symbol::annotations` entry.
        drop(loader);
        assert!(store.symbols.get(symbol).annotations.is_empty());
    }

    /// A hand-built, minimal, synthetic class file with one field and one
    /// method, each carrying a single `RuntimeVisibleAnnotations` entry for
    /// an `Ann` annotation type that *is* on the classpath — exercises
    /// `ClassLoader::enter_annotations` on a field/method `Symbol`, not
    /// just a class's (already covered by `resolves_a_real_annotation_on_a_class`).
    fn synthetic_class_with_field_and_method_annotations() -> Vec<u8> {
        fn push_utf8(pool: &mut Vec<u8>, next_index: &mut u16, value: &str) -> u16 {
            let index = *next_index;
            pool.push(1); // CONSTANT_Utf8
            pool.extend_from_slice(&(value.len() as u16).to_be_bytes());
            pool.extend_from_slice(value.as_bytes());
            *next_index += 1;
            index
        }
        fn push_class(pool: &mut Vec<u8>, next_index: &mut u16, name_index: u16) -> u16 {
            let index = *next_index;
            pool.push(7); // CONSTANT_Class
            pool.extend_from_slice(&name_index.to_be_bytes());
            *next_index += 1;
            index
        }
        fn annotations_attribute_body(type_index: u16) -> Vec<u8> {
            let mut body = Vec::new();
            body.extend_from_slice(&1u16.to_be_bytes()); // num_annotations
            body.extend_from_slice(&type_index.to_be_bytes());
            body.extend_from_slice(&0u16.to_be_bytes()); // num_element_value_pairs
            body
        }

        let mut pool = Vec::new();
        let mut next_index: u16 = 1;
        let this_name_index = push_utf8(&mut pool, &mut next_index, "C");
        let this_class_index = push_class(&mut pool, &mut next_index, this_name_index);
        let field_name_index = push_utf8(&mut pool, &mut next_index, "field");
        let field_descriptor_index = push_utf8(&mut pool, &mut next_index, "I");
        let method_name_index = push_utf8(&mut pool, &mut next_index, "method");
        let method_descriptor_index = push_utf8(&mut pool, &mut next_index, "()V");
        let attr_name_index = push_utf8(&mut pool, &mut next_index, "RuntimeVisibleAnnotations");
        let ann_type_index = push_utf8(&mut pool, &mut next_index, "LAnn;");
        let annotation_body = annotations_attribute_body(ann_type_index);

        let mut bytes = Vec::new();
        bytes.extend_from_slice(&[0xCA, 0xFE, 0xBA, 0xBE]);
        bytes.extend_from_slice(&[0x00, 0x00]);
        bytes.extend_from_slice(&[0x00, 0x45]);
        bytes.extend_from_slice(&next_index.to_be_bytes()); // constant_pool_count
        bytes.extend_from_slice(&pool);
        bytes.extend_from_slice(&[0x00, 0x21]); // access_flags
        bytes.extend_from_slice(&this_class_index.to_be_bytes());
        bytes.extend_from_slice(&[0x00, 0x00]); // super_class = none
        bytes.extend_from_slice(&[0x00, 0x00]); // interfaces_count = 0
        bytes.extend_from_slice(&[0x00, 0x01]); // fields_count = 1
        bytes.extend_from_slice(&[0x00, 0x00]); // field access_flags
        bytes.extend_from_slice(&field_name_index.to_be_bytes());
        bytes.extend_from_slice(&field_descriptor_index.to_be_bytes());
        bytes.extend_from_slice(&[0x00, 0x01]); // field attributes_count = 1
        bytes.extend_from_slice(&attr_name_index.to_be_bytes());
        bytes.extend_from_slice(&(annotation_body.len() as u32).to_be_bytes());
        bytes.extend_from_slice(&annotation_body);
        bytes.extend_from_slice(&[0x00, 0x01]); // methods_count = 1
        bytes.extend_from_slice(&[0x00, 0x00]); // method access_flags
        bytes.extend_from_slice(&method_name_index.to_be_bytes());
        bytes.extend_from_slice(&method_descriptor_index.to_be_bytes());
        bytes.extend_from_slice(&[0x00, 0x01]); // method attributes_count = 1
        bytes.extend_from_slice(&attr_name_index.to_be_bytes());
        bytes.extend_from_slice(&(annotation_body.len() as u32).to_be_bytes());
        bytes.extend_from_slice(&annotation_body);
        bytes.extend_from_slice(&[0x00, 0x00]); // attributes_count = 0 (class-level)
        bytes
    }

    #[test]
    fn enters_a_resolved_field_and_method_annotation_onto_their_own_symbols() {
        let mut classes = HashMap::new();
        classes.insert(
            BinaryName::from_internal("C"),
            synthetic_class_with_field_and_method_annotations(),
        );
        classes.insert(
            BinaryName::from_internal("Ann"),
            synthetic_class("Ann", None),
        );

        let mut store = SemanticStore::new();
        let mut loader = ClassLoader::new(InMemoryClassPath(classes), &mut store);
        let class_symbol = loader
            .load_class(&BinaryName::from_internal("C"))
            .expect("C should load");
        drop(loader);

        let declarations = class_info(&store, class_symbol).declarations;
        let mut member_symbol = |member_name: &str| {
            let text = store.names.intern(member_name);
            let name = Name::new(text, Namespace::Term);
            store
                .scopes
                .get(declarations)
                .lookup(&name)
                .unwrap_or_else(|| panic!("{member_name} should be entered in the class scope"))
        };
        let field_symbol = member_symbol("field");
        let method_symbol = member_symbol("method");

        for symbol in [field_symbol, method_symbol] {
            let symbol_annotations = &store.symbols.get(symbol).annotations;
            assert_eq!(symbol_annotations.len(), 1);
            let core_annotation = store.annotations.get(symbol_annotations[0]);
            assert_eq!(core_annotation.tree, None);
            assert_eq!(
                symbol_name(&store, parent_symbol(&store, core_annotation.ty)),
                "Ann"
            );
            let annotation_identity = parent_symbol(&store, core_annotation.ty);
            assert_eq!(
                store.symbols.get(annotation_identity).info,
                SymbolInfo::Missing
            );
        }
    }

    /// `tasty_visibility` (issue #16): a package qualifier that is the
    /// class's own or an enclosing package keeps its qualifier; anything else
    /// narrows to plain `Private`/`Protected`, never `Public`.
    #[test]
    fn tasty_visibility_keeps_an_enclosing_package_qualifier_and_narrows_the_rest() {
        use tasty_symbol::{DeclaredQualifier, DeclaredVisibility};

        let package = |path: &str| DeclaredQualifier::Package(path.to_owned());
        let mut store = SemanticStore::new();
        let mut loader = ClassLoader::new(InMemoryClassPath(HashMap::new()), &mut store);
        let class = BinaryName::from_internal("a/b/C");

        let own =
            loader.tasty_visibility(&class, &DeclaredVisibility::PrivateWithin(package("a/b")));
        let outer =
            loader.tasty_visibility(&class, &DeclaredVisibility::ProtectedWithin(package("a")));
        let unrelated =
            loader.tasty_visibility(&class, &DeclaredVisibility::PrivateWithin(package("x/y")));
        // `a/bc` merely starts with the text of `a/b`; it does not enclose.
        let prefix_only = loader.tasty_visibility(
            &BinaryName::from_internal("a/bc/C"),
            &DeclaredVisibility::ProtectedWithin(package("a/b")),
        );
        let other = loader.tasty_visibility(
            &class,
            &DeclaredVisibility::ProtectedWithin(DeclaredQualifier::Other),
        );
        let a_b = loader.session.packages.resolve_package(loader.store, "a/b");
        let a = loader.session.packages.resolve_package(loader.store, "a");

        assert_eq!(own, Visibility::PrivateWithin(a_b));
        assert_eq!(outer, Visibility::ProtectedWithin(a));
        assert_eq!(unrelated, Visibility::Private);
        assert_eq!(prefix_only, Visibility::Protected);
        assert_eq!(other, Visibility::Protected);
    }
}
