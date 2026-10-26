use crate::binary_name::BinaryName;
use crate::error::ClassLoadCategory;
use dotty_classfile::access_flags::{
    ACC_ABSTRACT, ACC_FINAL, ACC_INTERFACE, ACC_PRIVATE, ACC_PROTECTED, ACC_PUBLIC, ACC_STATIC,
    ACC_SYNTHETIC, ClassAccessFlags, FieldAccessFlags, MethodAccessFlags,
};
use dotty_core::{SymbolFlags, SymbolKind};
use dotty_tasty::tasty::{
    ABSTRACT_TAG, APPLIEDTPT_TAG, APPLIEDTYPE_TAG, ARTIFACT_TAG, AppliedTypeNode, ApplyNode,
    AstError, CASEACCESSOR_TAG, CONTRAVARIANT_TAG, COVARIANT_TAG, DEFDEF_TAG, DefDefBody,
    DefinitionBody, DefinitionTail, EMPTYCLAUSE_TAG, FIELDACCESSOR_TAG, FINAL_TAG, IDENTTPT_TAG,
    IdentNode, LOCAL_TAG, MUTABLE_TAG, OBJECT_TAG, PACKAGE_TAG, PRIVATE_TAG, PRIVATEQUALIFIED_TAG,
    PROTECTED_TAG, PROTECTEDQUALIFIED_TAG, ParameterNode, RawName, RawTree, Reader, ReferenceNode,
    RenderedSignedName, SELECTIN_TAG, SHAREDTERM_TAG, SHAREDTYPE_TAG, STABLE_TAG, STATIC_TAG,
    SYNTHETIC_TAG, SelectNode, StandardSection, StructuredNode, StructuredTree, TEMPLATE_TAG,
    TERMREF_TAG, TERMREFPKG_TAG, TERMREFSYMBOL_TAG, TRAIT_TAG, TYPEDEF_TAG, TYPEREF_TAG,
    TYPEREFDIRECT_TAG, TYPEREFPKG_TAG, TYPEREFSYMBOL_TAG, TastyFile, TastyFileError, TermValue,
    TypeApplyNode, VALDEF_TAG,
};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::time::Instant;

/// The class-level facts reconstructable from a `.tasty` file without full
/// type-checking: enough to enter a `dotty-core` `Symbol` and recurse into
/// its supertypes, mirroring what `.class` decoding gives
/// (`docs/classloader.md` §9).
///
/// `fields`/`methods` cover ordinary `ValDef`/`DefDef` template members;
/// each member's own declared type resolves to a name the same
/// best-effort way a supertype does (see `resolve_parent_name_with_bare`) — a
/// type shape this decoder does not recognize (anything past named/applied
/// types and method type-parameter references: tuples, function types,
/// refinements, dependent/path types, ...) is simply `None`, not a decode
/// failure, so the member itself is still reconstructed with an unknown type
/// rather than dropped or aborting the whole class. Generic methods preserve
/// their type-parameter order and address-based references for the supported
/// subset; unsupported bounds remain explicit. Class-parameter references
/// are retained as distinct indexed markers, though class type parameter
/// declarations are not modeled. `signature`/`nest_host`/etc. (JVM
/// classfile-only concepts) are not reconstructed from `.tasty` at all,
/// so a `.tasty`-backed symbol still gets no [`crate::symbol::ClassfileMetadata`]
/// entry.
#[derive(Debug, Clone)]
pub(crate) struct DecodedTastyClass {
    pub flags: ClassAccessFlags,
    /// The class's real `PRIVATE_TAG`/`PROTECTED_TAG` modifier, kept
    /// separately from `flags` — see [`DeclaredVisibility`]'s doc
    /// comment for why `ClassAccessFlags` alone cannot carry this.
    /// `None` covers both "no visibility modifier at all" (Scala's
    /// default: public) and any other modifier tag this decoder does
    /// not track.
    pub visibility: Option<DeclaredVisibility>,
    pub super_class: BinaryName,
    pub interfaces: Vec<BinaryName>,
    pub fields: Vec<DecodedTastyField>,
    pub methods: Vec<DecodedTastyMethod>,
}

/// Facts needed to publish a TASTy-backed identity without resolving its
/// parents or lowering any member types.
pub(crate) struct DecodedTastyIdentity {
    pub flags: ClassAccessFlags,
    pub visibility: Option<DeclaredVisibility>,
}

/// Low-overhead timing and outcome counters for one opt-in classloader TASTy
/// decode. Durations are exclusive across the listed stages; resolver
/// callbacks are separately timed and remain nested inside their caller stage.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct TastyDecodeProfile {
    pub format_version: Option<String>,
    pub outcome: String,
    pub parse_ns: u128,
    pub index_ns: u128,
    pub definition_selection_ns: u128,
    pub parent_resolution_ns: u128,
    pub parent_resolution_exclusive_ns: u128,
    pub constructor_accessor_ns: u128,
    pub constructor_accessor_exclusive_ns: u128,
    pub member_projection_ns: u128,
    pub member_projection_exclusive_ns: u128,
    pub callback_errors: usize,
    pub parent_alias_callbacks: TastyCallbackProfile,
    pub parent_bare_callbacks: TastyCallbackProfile,
    pub constructor_accessor_callbacks: TastyCallbackProfile,
    pub member_bare_callbacks: TastyCallbackProfile,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct TastyCallbackProfile {
    pub calls: usize,
    pub resolved: usize,
    pub unresolved: usize,
    pub errors: usize,
    pub elapsed_ns: u128,
    pub observations: Vec<TastyCallbackObservation>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TastyCallbackObservation {
    pub key: String,
    pub outcome: String,
    pub elapsed_ns: u128,
}

impl TastyDecodeProfile {
    pub(crate) fn mark_callback_errors(&mut self, keys: &[String]) {
        for profile in [
            &mut self.parent_alias_callbacks,
            &mut self.parent_bare_callbacks,
            &mut self.constructor_accessor_callbacks,
            &mut self.member_bare_callbacks,
        ] {
            for observation in &mut profile.observations {
                if keys.iter().any(|key| key == &observation.key)
                    && observation.outcome == "unresolved"
                {
                    observation.outcome = "error".to_owned();
                    profile.unresolved = profile.unresolved.saturating_sub(1);
                    profile.errors += 1;
                    self.callback_errors += 1;
                }
            }
        }
        if self.callback_errors > 0 {
            self.outcome = "callback_error".to_owned();
        }
    }
}

struct ProfileTimer<'a> {
    target: Option<&'a mut u128>,
    started: Option<Instant>,
}

impl<'a> ProfileTimer<'a> {
    fn new(target: Option<&'a mut u128>) -> Self {
        let started = target.as_ref().map(|_| Instant::now());
        Self { target, started }
    }
}

impl Drop for ProfileTimer<'_> {
    fn drop(&mut self) {
        if let (Some(target), Some(started)) = (&mut self.target, self.started) {
            **target += started.elapsed().as_nanos();
        }
    }
}

fn profile_callback<T>(
    key: impl FnOnce() -> String,
    callback: impl FnOnce() -> Option<T>,
    profile: Option<&mut TastyCallbackProfile>,
) -> Option<T> {
    let Some(profile) = profile else {
        return callback();
    };
    let started = Instant::now();
    let result = callback();
    profile.calls += 1;
    let outcome = if result.is_some() {
        profile.resolved += 1;
        "resolved"
    } else {
        profile.unresolved += 1;
        "unresolved"
    };
    let elapsed_ns = started.elapsed().as_nanos();
    profile.elapsed_ns += elapsed_ns;
    profile.observations.push(TastyCallbackObservation {
        key: key(),
        outcome: outcome.to_owned(),
        elapsed_ns,
    });
    result
}

/// The authoritative top-level module identities found in one `.tasty`
/// resource. This deliberately records identity only; it does not decode the
/// module class's parents or members.
pub(crate) struct DecodedTastyModuleIdentity {
    pub object_flags: SymbolFlags,
    pub object_visibility: Option<DeclaredVisibility>,
    pub module_class_flags: SymbolFlags,
    pub module_class_visibility: Option<DeclaredVisibility>,
    pub companion_class_kind: Option<SymbolKind>,
    /// Visibility declared on nested type definitions in the module class's
    /// TASTy template. JVM `InnerClasses` access flags can be less restrictive.
    /// The map includes default-public declarations with a `None` value.
    pub nested_type_visibilities: HashMap<String, Option<DeclaredVisibility>>,
}

/// Stable detail for a TASTy parent whose binary identity could not be
/// recovered. Audit tooling can use the raw tree to inspect wire shapes
/// without guessing a binary name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnresolvedParentReference {
    /// Zero-based position in the `Template` parent list.
    pub parent_index: usize,
    /// AST-section address of the enclosing `Template` node. Together with
    /// `parent_index`, this identifies a parent without relying on arena IDs.
    pub template_wire_address: usize,
    /// Coarse deterministic shape category.
    pub shape: String,
    /// Whether the parent contains a semantic reference tree.
    pub has_semantic_reference: bool,
    /// Whether that reference carries an explicit package/selection prefix.
    pub has_reference_prefix: bool,
    /// Name-table spellings visible in the raw parent tree.
    pub visible_names: Vec<String>,
    /// TASTy reference/selection tag at the first semantic reference shape,
    /// as a stable hexadecimal tag byte when the tree contains one.
    pub reference_tag: Option<String>,
    /// Name-table spelling carried by the first semantic reference.
    pub reference_name: Option<String>,
    /// Package/selection prefix recoverable from the first semantic reference.
    pub reference_prefix: Option<String>,
    /// Owner and alias name when the tree contains a package-object alias
    /// candidate. This is evidence about the encoded owner, not proof that
    /// the alias target exists on the classpath.
    pub alias_owner: Option<String>,
    pub alias_name: Option<String>,
    /// Bounded normalized rendering of the raw parent tree's wire tag shape.
    pub raw_tree: String,
}

/// Test/audit view of one declared TASTy template parent. It records whether
/// the current structural and classpath fallback resolvers found a binary
/// identity; it does not attempt to load that parent's dependencies.
#[cfg(test)]
#[derive(Debug)]
pub(crate) struct ParentReferenceProfile {
    pub reference: UnresolvedParentReference,
    pub resolved_binary_name: Option<String>,
    pub resolution_stage: &'static str,
}

/// A `.tasty` class-level visibility modifier that `ClassAccessFlags`
/// (`flags`) cannot represent: JVMS Table 4.1-A defines no
/// `ACC_PRIVATE`/`ACC_PROTECTED` bit for a *top-level* class's own
/// `access_flags` (only a *nested* class's `InnerClasses` entry carries
/// one — see `ClassLoader::enter_class`'s doc comment) — so
/// `ClassAccessFlags`, being JVM-shaped, only ever distinguishes public
/// from "not public", the same way it does for a real `.class` file.
/// `.tasty` actually knows the difference (Scala's `private`/`protected`
/// are real, distinct access levels — `Visibility::Private` is far more
/// restrictive than package-private, and `Visibility::Protected` is not
/// the same access boundary as either), so it is threaded through here
/// instead of being collapsed the way [`decode_flags`] collapses it for
/// `ClassAccessFlags`'s sake.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DeclaredVisibility {
    Private,
    Protected,
    /// `private[qualifier]`.
    PrivateWithin(DeclaredQualifier),
    /// `protected[qualifier]`.
    ProtectedWithin(DeclaredQualifier),
}

/// The qualifier of a `private[X]` / `protected[X]` modifier, as far as this
/// name-based reader can tell it.
///
/// Only a package qualifier can be named without resolving symbols, so that
/// is all it reports; the loader turns it into a package `SymbolId`. Any other
/// shape (an enclosing class, a shared or symbol reference) is `Other`, and
/// the loader then falls back to the plain, more restrictive
/// `Private`/`Protected` instead of widening to `Public`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DeclaredQualifier {
    /// A package, as a `/`-joined path (`me/cytrowski`).
    Package(String),
    Other,
}

/// One `ValDef` reconstructed from a class's `Template.stats`.
#[derive(Debug, Clone)]
pub(crate) struct DecodedTastyField {
    pub name: String,
    pub flags: FieldAccessFlags,
    /// `None` when [`resolve_parent_name_with_bare`] can't reduce the field's
    /// declared type tree to a name — see [`DecodedTastyClass`]'s doc
    /// comment.
    pub declared_type: Option<BinaryName>,
}

/// One `DefDef` reconstructed from a class's `Template.stats`.
#[derive(Debug, Clone)]
pub(crate) struct DecodedTastyMethod {
    pub name: String,
    pub flags: MethodAccessFlags,
    /// Method-local type parameters, in their TASTy header order. Type
    /// references in the signature point back to these entries by AST
    /// address and are represented as indexed references, never by spelling.
    pub type_parameters: Vec<DecodedTastyTypeParameter>,
    /// Every term parameter across every clause, flattened into one list
    /// — real Scala erasure already compiles a curried method
    /// (`def f(x: Int)(y: Int)`) down to one JVM method with every
    /// parameter concatenated, so this matches what a `.class`-loaded
    /// version of the same method would show.
    pub parameters: Vec<DecodedTastyParameter>,
    /// Whether the declaration has a term parameter clause. An explicit
    /// empty `()` clause has no parameters, so this is kept separately from
    /// `parameters` to distinguish it from a parameterless definition.
    pub has_term_clause: bool,
    /// `.tasty` always encodes an explicit return type tree, even for a
    /// `Unit`-returning method (`Unit` is an ordinary class reference in
    /// Scala's type system, not a separate "void" encoding the way a
    /// `.class` descriptor has one) — so `None` here means only "this
    /// decoder could not reduce the tree to a name", per
    /// [`DecodedTastyClass`]'s doc comment, exactly like an unresolvable
    /// field/parameter type; it is never a stand-in for `Unit`.
    pub return_type: Option<DecodedTastyType>,
}

/// A TASTy type shape retained for a bounded subset of generic member
/// signatures. Unsupported shapes remain `None` at the member boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DecodedTastyType {
    Named(BinaryName),
    ParamRef(u32),
    /// A reference to a parameter declared by the containing class. Kept
    /// distinct from method-local parameters so lowering can preserve exact
    /// evidence for a bounded receiver substitution.
    ClassParamRef(u32),
    Applied {
        tycon: Box<DecodedTastyType>,
        arguments: Vec<DecodedTastyType>,
    },
}

/// One method-local TASTy type parameter and its decoded bounds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DecodedTastyTypeParameter {
    pub name: String,
    pub lower_bound: Option<DecodedTastyType>,
    pub upper_bound: Option<DecodedTastyType>,
    pub variance: Option<u8>,
    /// False means the wire bound shape could not be represented. The loader
    /// keeps the method and lowers an explicit error bound instead of
    /// dropping or widening the generic signature.
    pub bounds_supported: bool,
}

/// One term parameter of a [`DecodedTastyMethod`].
#[derive(Debug, Clone)]
pub(crate) struct DecodedTastyParameter {
    pub name: String,
    pub declared_type: Option<DecodedTastyType>,
}

/// Why decoding a `.tasty` file into a `DecodedTastyClass` failed.
///
/// `pub`, not `pub(crate)`, even though the owning `tasty_symbol` module
/// is private: this type appears as a field of the public
/// [`crate::error::ClassLoadError::InvalidTastyFile`] variant and is
/// re-exported through the `classloader` facade module so it stays
/// nameable from outside the crate, the same treatment every other
/// symbol/error type introduced by a milestone gets.
#[derive(Debug, Clone)]
pub enum TastyDecodeError {
    /// The bytes did not parse as a Scala 3.9 `.tasty` file at all.
    Parse(TastyFileError),
    /// The file parsed, but its format is outside the classloader's
    /// explicitly validated Scala 3.9 input set (28.8.0 and 28.9.0).
    UnsupportedClassloaderVersion {
        /// File major version.
        major: u32,
        /// File minor version.
        minor: u32,
        /// File experimental version.
        experimental: u32,
    },
    /// The file parsed, but indexing/decoding its AST nodes failed.
    Ast(AstError),
    /// No top-level `TypeDef` in the file has the requested class's
    /// simple name (a `.tasty` file can define several `TypeDef`s for
    /// one source class — the class itself, its companion module, a
    /// `Mirror`-derived synthetic type — so this is not "the file is
    /// empty", just "none of them match").
    MissingTypeDef,
    /// The matching `TypeDef` has no `Template` body to read parents
    /// from (there is always one for a class/trait/object; seeing this
    /// means the file's shape isn't one this decoder recognizes).
    NoTemplateBody,
    /// A selected module class has no declared superclass in its `Template`.
    /// Unlike the legacy class projection, module completion never invents
    /// `java/lang/Object` when the hierarchy is absent.
    MissingModuleSuperclass,
    /// A declared `Template` parent (superclass or mixin interface) has no
    /// name this decoder can resolve — see `resolve_parent_name_with_bare`'s
    /// doc comment for exactly which shapes are and are not resolved.
    UnresolvedSupertype(Box<UnresolvedParentReference>),
    /// More than one top-level definition claims the requested module
    /// identity in the same TASTy package.
    AmbiguousModuleIdentity,
}

impl fmt::Display for TastyDecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Parse(source) => write!(formatter, "failed to parse .tasty file: {source}"),
            Self::UnsupportedClassloaderVersion {
                major,
                minor,
                experimental,
            } => write!(
                formatter,
                "TASTy version {major}.{minor}.{experimental} is not enabled for the Scala 3.9 classloader; supported versions are 28.8.0 and 28.9.0"
            ),
            Self::Ast(source) => write!(formatter, "failed to decode .tasty AST: {source}"),
            Self::MissingTypeDef => {
                write!(formatter, "no matching TypeDef found in .tasty file")
            }
            Self::NoTemplateBody => write!(formatter, "TypeDef has no Template body"),
            Self::MissingModuleSuperclass => {
                write!(
                    formatter,
                    "module-class Template has no declared superclass"
                )
            }
            Self::AmbiguousModuleIdentity => {
                write!(formatter, "TASTy has ambiguous top-level module identities")
            }
            Self::UnresolvedSupertype(parent) => write!(
                formatter,
                "unresolved supertype reference at parent {}: shape={}, semantic_reference={}, reference_prefix={}, names={:?}, raw={}",
                parent.parent_index,
                parent.shape,
                parent.has_semantic_reference,
                parent.has_reference_prefix,
                parent.visible_names,
                parent.raw_tree
            ),
        }
    }
}

impl std::error::Error for TastyDecodeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Parse(source) => Some(source),
            Self::UnsupportedClassloaderVersion { .. } => None,
            Self::Ast(source) => Some(source),
            Self::MissingTypeDef
            | Self::NoTemplateBody
            | Self::MissingModuleSuperclass
            | Self::UnresolvedSupertype(_)
            | Self::AmbiguousModuleIdentity => None,
        }
    }
}

impl TastyDecodeError {
    /// Stable identifier for the retained TASTy projection failure kind.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Parse(_) => "tasty.parse_error",
            Self::UnsupportedClassloaderVersion { .. } => "tasty.unsupported_version",
            Self::Ast(_) => "tasty.ast_error",
            Self::MissingTypeDef => "tasty.missing_type_def",
            Self::NoTemplateBody => "tasty.no_template_body",
            Self::MissingModuleSuperclass => "tasty.missing_module_superclass",
            Self::UnresolvedSupertype(_) => "tasty.unresolved_supertype",
            Self::AmbiguousModuleIdentity => "tasty.ambiguous_module_identity",
        }
    }

    /// Stable identifier for the deepest typed parse cause where the
    /// parser exposes one, or this projection error's own code otherwise.
    pub fn root_cause_code(&self) -> &'static str {
        use dotty_tasty::{HeaderError, file::TastyFileError};
        match self {
            Self::Parse(TastyFileError::Header(HeaderError::UnsupportedVersion { .. })) => {
                "tasty.unsupported_version"
            }
            Self::Parse(TastyFileError::Header(HeaderError::IncompatibleVersion { .. })) => {
                "tasty.incompatible_version"
            }
            Self::Parse(TastyFileError::Header(HeaderError::InvalidMagic { .. })) => {
                "tasty.invalid_magic"
            }
            Self::Parse(TastyFileError::Header(_)) => "tasty.invalid_header",
            Self::Parse(TastyFileError::Asts(error)) | Self::Ast(error) => ast_error_code(error),
            Self::Parse(_) => "tasty.parse_error",
            Self::UnsupportedClassloaderVersion { .. } => "tasty.unsupported_version",
            other => other.code(),
        }
    }

    /// Category based on available typed information.
    pub fn category(&self) -> ClassLoadCategory {
        use dotty_tasty::{HeaderError, file::TastyFileError};
        match self {
            Self::Parse(TastyFileError::Header(
                HeaderError::UnsupportedVersion { .. } | HeaderError::IncompatibleVersion { .. },
            )) => ClassLoadCategory::UnsupportedFeatureOrVersion,
            Self::Parse(TastyFileError::Asts(error)) | Self::Ast(error) => {
                ast_error_category(error)
            }
            Self::Parse(_) => ClassLoadCategory::MalformedArtifact,
            Self::UnsupportedClassloaderVersion { .. } => {
                ClassLoadCategory::UnsupportedFeatureOrVersion
            }
            Self::MissingTypeDef
            | Self::NoTemplateBody
            | Self::MissingModuleSuperclass
            | Self::UnresolvedSupertype(_)
            | Self::AmbiguousModuleIdentity => ClassLoadCategory::Conditional,
        }
    }
}

fn ast_error_code(error: &AstError) -> &'static str {
    use dotty_tasty::term::TermError;
    match error {
        AstError::UnsupportedCategory { .. } => "tasty.ast.unsupported_category",
        AstError::UnsupportedRelocation { .. } => "tasty.ast.unsupported_relocation",
        AstError::RecursionLimit { .. } => "tasty.ast.recursion_limit",
        AstError::Term(TermError::UnsupportedCategory { .. }) => "tasty.term.unsupported_category",
        AstError::Term(TermError::RecursionLimit { .. }) => "tasty.term.recursion_limit",
        AstError::InvalidTag { .. } => "tasty.ast.invalid_tag",
        AstError::InvalidAstReference { .. } => "tasty.ast.invalid_reference",
        AstError::UnexpectedTag { .. } => "tasty.ast.unexpected_tag",
        AstError::AstNodeCountMismatch { .. } => "tasty.ast.node_count_mismatch",
        AstError::RelocationDidNotConverge { .. } => "tasty.ast.relocation_did_not_converge",
        AstError::Read(_) | AstError::Term(_) => "tasty.ast_error",
    }
}

fn ast_error_category(error: &AstError) -> ClassLoadCategory {
    use dotty_tasty::term::TermError;
    match error {
        AstError::UnsupportedCategory { .. }
        | AstError::UnsupportedRelocation { .. }
        | AstError::RecursionLimit { .. }
        | AstError::Term(
            TermError::UnsupportedCategory { .. } | TermError::RecursionLimit { .. },
        ) => ClassLoadCategory::UnsupportedFeatureOrVersion,
        AstError::RelocationDidNotConverge { .. } => ClassLoadCategory::Conditional,
        AstError::Read(_)
        | AstError::Term(_)
        | AstError::InvalidTag { .. }
        | AstError::UnexpectedTag { .. }
        | AstError::InvalidAstReference { .. }
        | AstError::AstNodeCountMismatch { .. } => ClassLoadCategory::MalformedArtifact,
    }
}

impl From<TastyFileError> for TastyDecodeError {
    fn from(source: TastyFileError) -> Self {
        Self::Parse(source)
    }
}
impl From<AstError> for TastyDecodeError {
    fn from(source: AstError) -> Self {
        Self::Ast(source)
    }
}

/// Decodes `bytes` as a `.tasty` file and reconstructs the class named
/// `requested` (matched by its simple name — the part after the last
/// `/` — since a `.tasty` file's own `TypeDef`s carry no package
/// qualification of their own).
///
/// `requested`'s own package (everything before that last `/`, `""` for
/// the root package) doubles as the package every bare, unqualified
/// parent/member-type name reference is resolved relative to — see
/// [`resolve_parent_name_with_bare`]'s doc comment for why that's the right
/// context to use.
#[cfg(test)]
pub(crate) fn decode(
    bytes: &[u8],
    requested: &BinaryName,
) -> Result<DecodedTastyClass, TastyDecodeError> {
    decode_impl(
        bytes,
        requested,
        |_, _| None,
        |package, simple_name| {
            let internal = if package.is_empty() {
                simple_name.to_owned()
            } else {
                format!("{package}/{simple_name}")
            };
            Some(BinaryName::from_internal(internal))
        },
        false,
        false,
        None,
    )
}

/// Decodes a class with classpath-backed resolution for aliases and bare
/// references. The latter callback is consulted only for an unqualified name
/// that structural TASTy resolution could not identify.
pub(crate) fn decode_with_resolvers(
    bytes: &[u8],
    requested: &BinaryName,
    mut resolve_alias: impl FnMut(&BinaryName, &str) -> Option<BinaryName>,
    mut resolve_bare: impl FnMut(&str, &str) -> Option<BinaryName>,
) -> Result<DecodedTastyClass, TastyDecodeError> {
    decode_impl(
        bytes,
        requested,
        &mut resolve_alias,
        &mut resolve_bare,
        true,
        false,
        None,
    )
}

pub(crate) fn decode_with_resolvers_profiled(
    bytes: &[u8],
    requested: &BinaryName,
    mut resolve_alias: impl FnMut(&BinaryName, &str) -> Option<BinaryName>,
    mut resolve_bare: impl FnMut(&str, &str) -> Option<BinaryName>,
    profile: &mut TastyDecodeProfile,
) -> Result<DecodedTastyClass, TastyDecodeError> {
    let result = decode_impl(
        bytes,
        requested,
        &mut resolve_alias,
        &mut resolve_bare,
        true,
        false,
        Some(profile),
    );
    profile.outcome = match &result {
        Ok(_) => "success".to_owned(),
        Err(TastyDecodeError::UnresolvedSupertype(_)) => "unresolved_parent".to_owned(),
        Err(_) => "error".to_owned(),
    };
    result
}

/// Decodes the exact top-level `ModuleClass` established by
/// [`decode_module_identity`]. `requested` is the module-class binary name
/// (`Foo$`), while the TASTy definition is encoded with an `ObjectClass`
/// name. Requiring the `OBJECT` marker and package prevents an ordinary
/// `Foo` class or a nested definition from being selected by name alone.
pub(crate) fn decode_module_class_with_resolvers(
    bytes: &[u8],
    requested: &BinaryName,
    mut resolve_alias: impl FnMut(&BinaryName, &str) -> Option<BinaryName>,
    mut resolve_bare: impl FnMut(&str, &str) -> Option<BinaryName>,
) -> Result<DecodedTastyClass, TastyDecodeError> {
    decode_impl(
        bytes,
        requested,
        &mut resolve_alias,
        &mut resolve_bare,
        true,
        true,
        None,
    )
}

pub(crate) fn decode_module_class_with_resolvers_profiled(
    bytes: &[u8],
    requested: &BinaryName,
    mut resolve_alias: impl FnMut(&BinaryName, &str) -> Option<BinaryName>,
    mut resolve_bare: impl FnMut(&str, &str) -> Option<BinaryName>,
    profile: &mut TastyDecodeProfile,
) -> Result<DecodedTastyClass, TastyDecodeError> {
    let result = decode_impl(
        bytes,
        requested,
        &mut resolve_alias,
        &mut resolve_bare,
        true,
        true,
        Some(profile),
    );
    profile.outcome = match &result {
        Ok(_) => "success".to_owned(),
        Err(TastyDecodeError::UnresolvedSupertype(_)) => "unresolved_parent".to_owned(),
        Err(_) => "error".to_owned(),
    };
    result
}

/// Profiles every parent of the requested top-level TASTy definition using
/// the same reference resolution callbacks as class decoding. Keeping this
/// separate from `decode_impl` lets focused diagnostics inspect later parents
/// even when an earlier parent is unresolved; it does not change acceptance or
/// loading behavior.
#[cfg(test)]
pub(crate) fn profile_parent_references(
    bytes: &[u8],
    requested: &BinaryName,
    module_class: bool,
    mut resolve_alias: impl FnMut(&BinaryName, &str) -> Option<BinaryName>,
    mut resolve_bare: impl FnMut(&str, &str) -> Option<BinaryName>,
) -> Result<Vec<ParentReferenceProfile>, TastyDecodeError> {
    let file = parse_classloader_tasty(bytes)?;
    let index = file.ast_address_index()?;
    let requested_internal = requested.as_internal();
    let simple_name = requested_internal
        .rsplit('/')
        .next()
        .unwrap_or(requested_internal);
    let package = requested_internal
        .rsplit_once('/')
        .map_or("", |(package, _)| package);
    let mut matching_nodes = index.iter().filter(|node| {
        if node.tag != TYPEDEF_TAG {
            return false;
        }
        let Ok(StructuredNode::TypeDef(DefinitionBody::TypeDef {
            name,
            tail,
            type_or_template,
        })) = node.decode_structured()
        else {
            return false;
        };
        if module_class {
            module_class_wire_name(&file, name).as_deref() == Some(simple_name)
                && has_modifier(&tail, OBJECT_TAG)
                && is_in_requested_top_level_package(&file, &index, node.offset, package)
                && matches!(type_or_template, RawTree::LengthNode(template) if template.tag == TEMPLATE_TAG)
        } else {
            wire_name(&file, name).as_deref() == Some(simple_name)
        }
    });
    let node = matching_nodes
        .next()
        .ok_or(TastyDecodeError::MissingTypeDef)?;
    if matching_nodes.next().is_some() {
        return Err(TastyDecodeError::AmbiguousModuleIdentity);
    }
    let StructuredNode::TypeDef(DefinitionBody::TypeDef {
        type_or_template, ..
    }) = node.decode_structured()?
    else {
        return Err(TastyDecodeError::MissingTypeDef);
    };
    let RawTree::LengthNode(template_node) = type_or_template else {
        return Err(TastyDecodeError::NoTemplateBody);
    };
    if template_node.tag != TEMPLATE_TAG {
        return Err(TastyDecodeError::NoTemplateBody);
    }
    let StructuredNode::Template(template) = template_node.decode_structured()? else {
        return Err(TastyDecodeError::NoTemplateBody);
    };
    let template_wire_address = index
        .children_of(node.offset as u32)
        .find(|child| child.tag == TEMPLATE_TAG)
        .map(|child| child.offset)
        .ok_or(TastyDecodeError::NoTemplateBody)?;

    template
        .parents
        .iter()
        .enumerate()
        .map(|(parent_index, parent)| {
            let alias_candidate = unresolved_alias_reference(&file, parent)
                .filter(|(owner, _)| matches!(owner.simple_name(), "package" | "package$"));
            let mut alias_attempted = false;
            let mut alias_resolved = false;
            let mut bare_attempted = false;
            let mut bare_resolved = false;
            let resolved = resolve_parent_name_with_alias(
                &file,
                parent,
                package,
                &mut |owner, alias| {
                    alias_attempted = true;
                    let result = resolve_alias(owner, alias);
                    alias_resolved = result.is_some();
                    result
                },
                &mut |owner_package, simple_name| {
                    bare_attempted = true;
                    let result = resolve_bare(owner_package, simple_name);
                    bare_resolved = result.is_some();
                    result
                },
            );
            let resolution_stage = if alias_resolved {
                "package-object-alias-resolved"
            } else if bare_resolved {
                "classpath-bare-candidate-resolved"
            } else if resolved.is_some() {
                "semantic-or-structural-reference-resolved"
            } else if alias_attempted || alias_candidate.is_some() {
                "package-object-alias-unresolved"
            } else if bare_attempted {
                "classpath-bare-candidate-unresolved"
            } else {
                match resolve_reference_name_in_package(&file, parent, Some(package)) {
                    ReferenceResolution::Unresolved => "semantic-prefix-unresolved",
                    ReferenceResolution::NotAReference
                        if parent_has_semantic_reference(&file, parent, package) =>
                    {
                        "unsupported-semantic-reference-shape"
                    }
                    ReferenceResolution::NotAReference => "unsupported-nonsemantic-parent-shape",
                    ReferenceResolution::Resolved(_) => "semantic-reference-unresolved",
                }
            };
            Ok(ParentReferenceProfile {
                reference: unresolved_parent_reference(
                    &file,
                    parent,
                    parent_index,
                    package,
                    template_wire_address,
                ),
                resolved_binary_name: resolved.map(|name| name.as_internal().to_owned()),
                resolution_stage,
            })
        })
        .collect()
}

/// Parses the two stable TASTy formats validated for Scala 3.9 classpath
/// artifacts. This policy is intentionally narrower than the generic
/// compatible-minor relation: the compiler artifacts used by this
/// repository are 28.8.0, and the matching Scala 3.9 library is 28.9.0.
pub(crate) fn parse_classloader_tasty(bytes: &[u8]) -> Result<TastyFile<'_>, TastyDecodeError> {
    let file = TastyFile::parse(bytes)?;
    let header = file.header();
    if !matches!(
        (
            header.major_version,
            header.minor_version,
            header.experimental_version
        ),
        (28, 8, 0) | (28, 9, 0)
    ) {
        return Err(TastyDecodeError::UnsupportedClassloaderVersion {
            major: header.major_version,
            minor: header.minor_version,
            experimental: header.experimental_version,
        });
    }
    Ok(file)
}

pub(crate) fn decode_identity(
    bytes: &[u8],
    requested: &BinaryName,
) -> Result<DecodedTastyIdentity, TastyDecodeError> {
    let file = parse_classloader_tasty(bytes)?;
    let index = file.ast_address_index()?;
    let requested_internal = requested.as_internal();
    let simple_name = requested_internal
        .rsplit('/')
        .next()
        .unwrap_or(requested_internal);
    let node = index
        .iter()
        .find(|node| {
            if node.tag != TYPEDEF_TAG {
                return false;
            }
            let Ok(definition) = node.decode_definition() else {
                return false;
            };
            wire_name(&file, definition.name()).as_deref() == Some(simple_name)
        })
        .ok_or(TastyDecodeError::MissingTypeDef)?;

    let StructuredNode::TypeDef(DefinitionBody::TypeDef {
        tail,
        type_or_template,
        ..
    }) = node.decode_structured()?
    else {
        return Err(TastyDecodeError::MissingTypeDef);
    };
    let RawTree::LengthNode(template_node) = type_or_template else {
        return Err(TastyDecodeError::NoTemplateBody);
    };
    if template_node.tag != TEMPLATE_TAG {
        return Err(TastyDecodeError::NoTemplateBody);
    }
    if !matches!(
        template_node.decode_structured()?,
        StructuredNode::Template(_)
    ) {
        return Err(TastyDecodeError::NoTemplateBody);
    }

    Ok(DecodedTastyIdentity {
        flags: decode_flags(&tail),
        visibility: decode_visibility(&file, &tail),
    })
}

/// Finds a Scala object and its module class only when both exact definitions
/// are direct members of the requested package in the authoritative TASTy
/// file. A classfile or a `$`-suffixed spelling alone is never evidence.
pub(crate) fn decode_module_identity(
    bytes: &[u8],
    requested: &BinaryName,
) -> Result<Option<DecodedTastyModuleIdentity>, TastyDecodeError> {
    let file = parse_classloader_tasty(bytes)?;
    let index = file.ast_address_index()?;
    let package_path = requested.package_path();
    let object_name = requested.simple_name();
    let module_class_name = format!("{object_name}$");

    let mut objects = Vec::new();
    let mut module_classes = Vec::new();
    let mut companion_classes = Vec::new();

    for node in index.iter() {
        if !is_in_requested_top_level_package(&file, &index, node.offset, package_path) {
            continue;
        }
        match node.tag {
            VALDEF_TAG => {
                let StructuredNode::ValDef(DefinitionBody::ValDef { name, tail, .. }) =
                    node.decode_structured()?
                else {
                    continue;
                };
                if wire_name(&file, name).as_deref() == Some(object_name)
                    && has_modifier(&tail, OBJECT_TAG)
                {
                    objects.push((symbol_flags(&tail), decode_visibility(&file, &tail)));
                }
            }
            TYPEDEF_TAG => {
                let StructuredNode::TypeDef(DefinitionBody::TypeDef {
                    name,
                    type_or_template,
                    tail,
                }) = node.decode_structured()?
                else {
                    continue;
                };
                let Some(name) = module_class_wire_name(&file, name) else {
                    continue;
                };
                let has_template = matches!(
                    type_or_template,
                    RawTree::LengthNode(ref template) if template.tag == TEMPLATE_TAG
                );
                if name == module_class_name && has_template && has_modifier(&tail, OBJECT_TAG) {
                    let mut nested_type_visibilities = HashMap::new();
                    if let RawTree::LengthNode(template_node) = type_or_template {
                        for stat in template_node.decode_template_structure()?.stats {
                            let RawTree::LengthNode(definition) = stat else {
                                continue;
                            };
                            if definition.tag != TYPEDEF_TAG {
                                continue;
                            }
                            let StructuredNode::TypeDef(DefinitionBody::TypeDef {
                                name, tail, ..
                            }) = definition.decode_structured()?
                            else {
                                continue;
                            };
                            if let Some(name) = wire_name(&file, name) {
                                nested_type_visibilities
                                    .insert(name, decode_visibility(&file, &tail));
                            }
                        }
                    }
                    module_classes.push((
                        symbol_flags(&tail),
                        decode_visibility(&file, &tail),
                        nested_type_visibilities,
                    ));
                } else if name == object_name && has_template && !has_modifier(&tail, OBJECT_TAG) {
                    companion_classes.push(if has_modifier(&tail, TRAIT_TAG) {
                        SymbolKind::Trait
                    } else {
                        SymbolKind::Class
                    });
                }
            }
            _ => {}
        }
    }

    let [object] = objects.as_slice() else {
        return if objects.len() > 1 {
            Err(TastyDecodeError::AmbiguousModuleIdentity)
        } else {
            Ok(None)
        };
    };
    let [module_class] = module_classes.as_slice() else {
        return if module_classes.len() > 1 {
            Err(TastyDecodeError::AmbiguousModuleIdentity)
        } else {
            Ok(None)
        };
    };
    if companion_classes.len() > 1 {
        return Err(TastyDecodeError::AmbiguousModuleIdentity);
    }

    Ok(Some(DecodedTastyModuleIdentity {
        object_flags: object.0,
        object_visibility: object.1.clone(),
        module_class_flags: module_class.0,
        module_class_visibility: module_class.1.clone(),
        companion_class_kind: companion_classes.first().copied(),
        nested_type_visibilities: module_class.2.clone(),
    }))
}

fn is_in_requested_top_level_package(
    file: &TastyFile<'_>,
    index: &dotty_tasty::tasty::AstAddressIndex<'_>,
    address: usize,
    requested_package: &str,
) -> bool {
    let Ok(address) = u32::try_from(address) else {
        return false;
    };
    let Some(parent) = index.parent_of(address) else {
        return requested_package.is_empty();
    };
    if parent.tag != PACKAGE_TAG {
        return false;
    }
    let Some(package_node) = index.get(parent.offset as u32) else {
        return false;
    };
    let Ok(package) = package_node.decode_package() else {
        return false;
    };
    let RawTree::Leaf(term) = package.path else {
        return false;
    };
    if term.tag != TERMREFPKG_TAG {
        return false;
    }
    let TermValue::NameRef(reference) = term.value else {
        return false;
    };
    resolve_qualified_name(file, reference).as_deref() == Some(requested_package)
}

fn has_modifier(tail: &[DefinitionTail<'_>], tag: u8) -> bool {
    tail.iter()
        .any(|item| matches!(item, DefinitionTail::Modifier(found) if *found == tag))
}

fn symbol_flags(tail: &[DefinitionTail<'_>]) -> SymbolFlags {
    use dotty_tasty::tasty::{
        CASE_TAG, ENUM_TAG, ERASED_TAG, EXPORTED_TAG, EXTENSION_TAG, GIVEN_TAG, IMPLICIT_TAG,
        INLINE_TAG, LAZY_TAG, OPAQUE_TAG, OVERRIDE_TAG, SEALED_TAG, TRANSPARENT_TAG,
    };

    tail.iter().fold(SymbolFlags::EMPTY, |flags, item| {
        let DefinitionTail::Modifier(tag) = item else {
            return flags;
        };
        flags
            | match *tag {
                ABSTRACT_TAG => SymbolFlags::ABSTRACT,
                FINAL_TAG => SymbolFlags::FINAL,
                SEALED_TAG => SymbolFlags::SEALED,
                CASE_TAG => SymbolFlags::CASE,
                IMPLICIT_TAG => SymbolFlags::IMPLICIT,
                GIVEN_TAG => SymbolFlags::GIVEN,
                LAZY_TAG => SymbolFlags::LAZY,
                MUTABLE_TAG => SymbolFlags::MUTABLE,
                INLINE_TAG => SymbolFlags::INLINE,
                TRANSPARENT_TAG => SymbolFlags::TRANSPARENT,
                OPAQUE_TAG => SymbolFlags::OPAQUE,
                EXTENSION_TAG => SymbolFlags::EXTENSION,
                STATIC_TAG => SymbolFlags::STATIC,
                SYNTHETIC_TAG => SymbolFlags::SYNTHETIC,
                ERASED_TAG => SymbolFlags::ERASED,
                OVERRIDE_TAG => SymbolFlags::OVERRIDE,
                ENUM_TAG => SymbolFlags::ENUM,
                EXPORTED_TAG => SymbolFlags::EXPORTED,
                _ => SymbolFlags::EMPTY,
            }
    })
}

fn decode_impl(
    bytes: &[u8],
    requested: &BinaryName,
    mut resolve_alias: impl FnMut(&BinaryName, &str) -> Option<BinaryName>,
    mut resolve_bare: impl FnMut(&str, &str) -> Option<BinaryName>,
    strict_superclass: bool,
    module_class: bool,
    mut profile: Option<&mut TastyDecodeProfile>,
) -> Result<DecodedTastyClass, TastyDecodeError> {
    let parsed = {
        let timer = ProfileTimer::new(profile.as_deref_mut().map(|profile| &mut profile.parse_ns));
        let parsed = parse_classloader_tasty(bytes);
        drop(timer);
        parsed?
    };
    if let Some(profile) = profile.as_deref_mut() {
        let header = parsed.header();
        profile.format_version = Some(format!(
            "{}.{}.{}",
            header.major_version, header.minor_version, header.experimental_version
        ));
    }
    let file = parsed;
    let index_result = {
        let timer = ProfileTimer::new(profile.as_deref_mut().map(|profile| &mut profile.index_ns));
        let index = file.ast_address_index();
        drop(timer);
        index
    };
    let index = index_result?;
    let requested_internal = requested.as_internal();
    let simple_name = requested_internal
        .rsplit('/')
        .next()
        .unwrap_or(requested_internal);
    let package = match requested_internal.rsplit_once('/') {
        Some((package, _)) => package,
        None => "",
    };

    let definition_selection_timer = ProfileTimer::new(
        profile
            .as_deref_mut()
            .map(|profile| &mut profile.definition_selection_ns),
    );
    let mut matching_nodes = index.iter().filter(|node| {
            if node.tag != TYPEDEF_TAG {
                return false;
            }
            let Ok(StructuredNode::TypeDef(DefinitionBody::TypeDef {
                name,
                tail,
                type_or_template,
            })) = node.decode_structured() else {
                return false;
            };
            if module_class {
                module_class_wire_name(&file, name).as_deref() == Some(simple_name)
                    && has_modifier(&tail, OBJECT_TAG)
                    && is_in_requested_top_level_package(
                        &file,
                        &index,
                        node.offset,
                        package,
                    )
                    && matches!(type_or_template, RawTree::LengthNode(template) if template.tag == TEMPLATE_TAG)
            } else {
                wire_name(&file, name).as_deref() == Some(simple_name)
            }
        });
    let node = matching_nodes
        .next()
        .ok_or(TastyDecodeError::MissingTypeDef)?;
    if matching_nodes.next().is_some() {
        return Err(TastyDecodeError::AmbiguousModuleIdentity);
    }

    let StructuredNode::TypeDef(DefinitionBody::TypeDef {
        tail,
        type_or_template,
        ..
    }) = node.decode_structured()?
    else {
        return Err(TastyDecodeError::MissingTypeDef);
    };

    let flags = decode_flags(&tail);
    let visibility = decode_visibility(&file, &tail);

    let RawTree::LengthNode(template_node) = &type_or_template else {
        return Err(TastyDecodeError::NoTemplateBody);
    };
    if template_node.tag != TEMPLATE_TAG {
        return Err(TastyDecodeError::NoTemplateBody);
    }
    let StructuredNode::Template(template) = template_node.decode_structured()? else {
        return Err(TastyDecodeError::NoTemplateBody);
    };
    let template_wire_address = index
        .children_of(node.offset as u32)
        .find(|child| child.tag == TEMPLATE_TAG)
        .map(|child| child.offset)
        .ok_or(TastyDecodeError::NoTemplateBody)?;
    drop(definition_selection_timer);

    let mut parent_alias_callbacks = TastyCallbackProfile::default();
    let mut parent_bare_callbacks = TastyCallbackProfile::default();
    let collect_callback_profile = profile.is_some();
    let parent_resolution = {
        let timer = ProfileTimer::new(
            profile
                .as_deref_mut()
                .map(|profile| &mut profile.parent_resolution_ns),
        );
        let result = {
            let mut profiled_alias = |owner: &BinaryName, alias: &str| {
                profile_callback(
                    || format!("alias|{}|{alias}", owner.as_internal()),
                    || resolve_alias(owner, alias),
                    collect_callback_profile.then_some(&mut parent_alias_callbacks),
                )
            };
            let mut profiled_bare = |package: &str, simple_name: &str| {
                profile_callback(
                    || format!("bare|{package}|{simple_name}"),
                    || resolve_bare(package, simple_name),
                    collect_callback_profile.then_some(&mut parent_bare_callbacks),
                )
            };
            (|| {
                let mut parents = template.parents.iter().enumerate();
                let super_class = match parents.next() {
                    Some((parent_index, parent)) => {
                        let resolved = resolve_parent_name_with_alias(
                            &file,
                            parent,
                            package,
                            &mut profiled_alias,
                            &mut profiled_bare,
                        );
                        match (resolved, strict_superclass) {
                            (Some(parent), _) => parent,
                            (None, false) => BinaryName::from_internal("java/lang/Object"),
                            (None, true) => {
                                return Err(TastyDecodeError::UnresolvedSupertype(Box::new(
                                    unresolved_parent_reference(
                                        &file,
                                        parent,
                                        parent_index,
                                        package,
                                        template_wire_address,
                                    ),
                                )));
                            }
                        }
                    }
                    None if module_class => {
                        return Err(TastyDecodeError::MissingModuleSuperclass);
                    }
                    None => BinaryName::from_internal("java/lang/Object"),
                };

                let mut interfaces = Vec::new();
                for (parent_index, parent) in parents {
                    let resolution = resolve_parent_name_with_alias(
                        &file,
                        parent,
                        package,
                        &mut profiled_alias,
                        &mut profiled_bare,
                    );
                    interfaces.push(resolution.ok_or_else(|| {
                        TastyDecodeError::UnresolvedSupertype(Box::new(
                            unresolved_parent_reference(
                                &file,
                                parent,
                                parent_index,
                                package,
                                template_wire_address,
                            ),
                        ))
                    })?);
                }
                Ok((super_class, interfaces))
            })()
        };
        drop(timer);
        result
    };
    if let Some(profile) = profile.as_deref_mut() {
        profile.parent_alias_callbacks = parent_alias_callbacks;
        profile.parent_bare_callbacks = parent_bare_callbacks;
        profile.parent_resolution_exclusive_ns = profile.parent_resolution_ns.saturating_sub(
            profile.parent_alias_callbacks.elapsed_ns + profile.parent_bare_callbacks.elapsed_ns,
        );
    }
    let (super_class, interfaces) = parent_resolution?;

    let mut constructor_bare_callbacks = TastyCallbackProfile::default();
    let fields_result = {
        let timer = ProfileTimer::new(
            profile
                .as_deref_mut()
                .map(|profile| &mut profile.constructor_accessor_ns),
        );
        let mut profiled_bare = |package: &str, simple_name: &str| {
            profile_callback(
                || format!("bare|{package}|{simple_name}"),
                || resolve_bare(package, simple_name),
                collect_callback_profile.then_some(&mut constructor_bare_callbacks),
            )
        };
        let fields = decode_constructor_accessor_fields(
            &file,
            &template.term_params,
            package,
            &mut profiled_bare,
        );
        drop(timer);
        fields
    };
    if let Some(profile) = profile.as_deref_mut() {
        profile.constructor_accessor_callbacks = constructor_bare_callbacks;
        profile.constructor_accessor_exclusive_ns = profile
            .constructor_accessor_ns
            .saturating_sub(profile.constructor_accessor_callbacks.elapsed_ns);
    }
    let mut fields = fields_result?;
    let class_type_parameters = template
        .type_params
        .iter()
        .filter(|parameter| matches!(parameter, ParameterNode::TypeParam { .. }))
        .enumerate()
        .filter_map(|(parameter_index, parameter)| {
            let payload = parameter.payload();
            let address = index
                .iter()
                .find(|indexed| {
                    indexed.tag == dotty_tasty::tasty::TYPEPARAM_TAG
                        && indexed.payload.len() == payload.len()
                        && indexed.payload.as_ptr() == payload.as_ptr()
                })
                .and_then(|indexed| u32::try_from(indexed.offset).ok())?;
            Some((address, u32::try_from(parameter_index).ok()?))
        })
        .collect::<HashMap<_, _>>();
    let mut member_bare_callbacks = TastyCallbackProfile::default();
    let members_result = {
        let timer = ProfileTimer::new(
            profile
                .as_deref_mut()
                .map(|profile| &mut profile.member_projection_ns),
        );
        let mut profiled_bare = |package: &str, simple_name: &str| {
            profile_callback(
                || format!("bare|{package}|{simple_name}"),
                || resolve_bare(package, simple_name),
                collect_callback_profile.then_some(&mut member_bare_callbacks),
            )
        };
        let members = decode_members(
            &file,
            &template.stats,
            package,
            &class_type_parameters,
            &mut profiled_bare,
        );
        drop(timer);
        members
    };
    if let Some(profile) = profile {
        profile.member_bare_callbacks = member_bare_callbacks;
        profile.member_projection_exclusive_ns = profile
            .member_projection_ns
            .saturating_sub(profile.member_bare_callbacks.elapsed_ns);
    }
    let (stats_fields, methods) = members_result?;
    fields.extend(stats_fields);

    Ok(DecodedTastyClass {
        flags,
        visibility,
        super_class,
        interfaces,
        fields,
        methods,
    })
}

fn unresolved_parent_reference(
    file: &TastyFile<'_>,
    parent: &RawTree<'_>,
    parent_index: usize,
    package: &str,
    template_wire_address: usize,
) -> UnresolvedParentReference {
    let visible_names = parent_visible_names(file, parent);
    let has_semantic_reference = parent_has_semantic_reference(file, parent, package);
    // A path through Scala's synthetic `package` object is an implicit
    // import alias, not an explicit package prefix for the referenced class.
    let has_reference_prefix = !visible_names.iter().any(|name| name == "package")
        && parent_has_reference_prefix(file, parent, package, 0);
    let shape = match parent_reference_shape(file, parent).as_str() {
        "qualified-reference" if has_reference_prefix => "package-qualified".to_owned(),
        "other" if !visible_names.is_empty() && !has_semantic_reference => {
            "bare-implicit-like".to_owned()
        }
        "other" if has_semantic_reference => "unsupported-prefix".to_owned(),
        shape => shape.to_owned(),
    };
    let reference_details = parent_reference_details(file, parent, 0);
    let alias = unresolved_alias_reference(file, parent);
    UnresolvedParentReference {
        parent_index,
        template_wire_address,
        shape,
        has_semantic_reference,
        has_reference_prefix,
        visible_names,
        reference_tag: reference_details
            .as_ref()
            .map(|details| details.tag.clone()),
        reference_name: reference_details
            .as_ref()
            .and_then(|details| details.name.clone()),
        reference_prefix: reference_details.and_then(|details| details.prefix),
        alias_owner: alias
            .as_ref()
            .map(|(owner, _)| owner.as_internal().to_owned()),
        alias_name: alias.map(|(_, alias)| alias),
        raw_tree: parent_raw_shape(parent, 0),
    }
}

struct ParentReferenceDetails {
    tag: String,
    name: Option<String>,
    prefix: Option<String>,
}

fn parent_reference_details(
    file: &TastyFile<'_>,
    tree: &RawTree<'_>,
    depth: usize,
) -> Option<ParentReferenceDetails> {
    const MAX_PARENT_PROFILE_DEPTH: usize = 128;
    if depth >= MAX_PARENT_PROFILE_DEPTH {
        return None;
    }
    match tree.decode_structured().ok() {
        Some(StructuredTree::Reference(reference)) => Some(ParentReferenceDetails {
            tag: format!("0x{:02x}", reference.tag),
            name: resolve_qualified_name(file, reference.reference),
            prefix: resolve_reference_prefix(file, &reference.qualifier),
        }),
        Some(StructuredTree::Ident(ident)) => {
            parent_reference_details(file, &ident.type_tree, depth + 1)
        }
        Some(StructuredTree::Select(selection)) => Some(ParentReferenceDetails {
            tag: format!("0x{:02x}", selection.tag),
            name: wire_name(file, selection.name),
            prefix: parent_profile_owner(file, &selection.qualifier, depth + 1),
        }),
        Some(StructuredTree::AstChild(child)) => {
            parent_reference_details(file, &child.child, depth + 1)
        }
        Some(StructuredTree::Length(StructuredNode::Apply(apply))) => {
            parent_reference_details(file, &apply.function, depth + 1)
        }
        Some(StructuredTree::Length(StructuredNode::TypeApply(apply))) => {
            parent_reference_details(file, &apply.function, depth + 1)
        }
        Some(StructuredTree::Length(StructuredNode::AppliedType(applied))) => {
            parent_reference_details(file, &applied.tycon, depth + 1)
        }
        Some(StructuredTree::Length(StructuredNode::Typed(typed))) => {
            parent_reference_details(file, &typed.type_tree, depth + 1)
                .or_else(|| parent_reference_details(file, &typed.expression, depth + 1))
        }
        Some(StructuredTree::Length(StructuredNode::SelectOuter(selection))) => {
            parent_reference_details(file, &selection.underlying_type, depth + 1)
                .or_else(|| parent_reference_details(file, &selection.qualifier, depth + 1))
        }
        Some(StructuredTree::Length(StructuredNode::SelectIn(selection))) => {
            parent_reference_details(file, &selection.underlying_type, depth + 1).or_else(|| {
                Some(ParentReferenceDetails {
                    tag: format!("0x{:02x}", selection.tag),
                    name: wire_name(file, selection.name),
                    prefix: parent_profile_owner(file, &selection.qualifier, depth + 1),
                })
            })
        }
        _ => match tree {
            RawTree::Leaf(term) if term.ast_ref().is_some() => {
                let reference = term.ast_ref()?;
                let shared = decode_shared_tree(file, reference.address)?;
                parent_reference_details(file, &shared, depth + 1)
            }
            RawTree::Ast { child, .. } | RawTree::NatAst { child, .. } => {
                parent_reference_details(file, child, depth + 1)
            }
            _ => None,
        },
    }
}

fn parent_profile_owner(file: &TastyFile<'_>, tree: &RawTree<'_>, depth: usize) -> Option<String> {
    parent_wire_path(file, tree, depth).or_else(|| {
        parent_visible_names(file, tree)
            .into_iter()
            .find(|name| name.contains('/'))
    })
}

/// Reconstructs a bounded structural path from TASTy nodes. The path alone is
/// not sufficient evidence for production resolution: the selected-parent
/// caller separately requires exactly one terminal type selection, rejecting
/// nested owners whose JVM binary name is not established by this path.
fn parent_wire_path(file: &TastyFile<'_>, tree: &RawTree<'_>, depth: usize) -> Option<String> {
    const MAX_PARENT_PROFILE_DEPTH: usize = 128;
    if depth >= MAX_PARENT_PROFILE_DEPTH {
        return None;
    }
    if let Ok(structured) = tree.decode_structured() {
        let path = match structured {
            StructuredTree::Reference(reference) => {
                let name = resolve_qualified_name(file, reference.reference)?;
                if matches!(reference.tag, TERMREFPKG_TAG | TYPEREFPKG_TAG) {
                    let prefix = parent_wire_path(file, &reference.qualifier, depth + 1);
                    Some(match prefix.as_deref() {
                        None | Some("") | Some("_root_") => name,
                        Some(owner) => format!("{}/{name}", owner.trim_matches('/')),
                    })
                } else {
                    Some(name)
                }
            }
            StructuredTree::Select(selection) => {
                let owner = parent_wire_path(file, &selection.qualifier, depth + 1)?;
                let name = wire_name(file, selection.name)?;
                Some(format!("{owner}/{name}"))
            }
            StructuredTree::AstChild(child) => parent_wire_path(file, &child.child, depth + 1),
            StructuredTree::Ident(ident) => parent_wire_path(file, &ident.type_tree, depth + 1),
            StructuredTree::Length(StructuredNode::AppliedType(applied)) => {
                parent_wire_path(file, &applied.tycon, depth + 1)
            }
            StructuredTree::Length(StructuredNode::Apply(apply)) => {
                parent_wire_path(file, &apply.function, depth + 1)
            }
            StructuredTree::Length(StructuredNode::TypeApply(apply)) => {
                parent_wire_path(file, &apply.function, depth + 1)
            }
            StructuredTree::Length(StructuredNode::SelectIn(selection)) => {
                let owner = parent_wire_path(file, &selection.qualifier, depth + 1)?;
                let name = wire_name(file, selection.name)?;
                Some(format!("{owner}/{name}"))
            }
            StructuredTree::Length(StructuredNode::Typed(typed)) => {
                parent_wire_path(file, &typed.type_tree, depth + 1)
                    .or_else(|| parent_wire_path(file, &typed.expression, depth + 1))
            }
            StructuredTree::Length(StructuredNode::SelectOuter(selection)) => {
                parent_wire_path(file, &selection.underlying_type, depth + 1)
                    .or_else(|| parent_wire_path(file, &selection.qualifier, depth + 1))
            }
            _ => None,
        };
        if path.is_some() {
            return path;
        }
    }
    match tree {
        RawTree::Leaf(term) if term.ast_ref().is_some() => {
            let reference = term.ast_ref()?;
            resolve_symbol_address(file, reference.address).or_else(|| {
                let shared = decode_shared_tree(file, reference.address)?;
                parent_wire_path(file, &shared, depth + 1)
            })
        }
        RawTree::Ast { child, .. } | RawTree::NatAst { child, .. } => {
            parent_wire_path(file, child, depth + 1)
        }
        RawTree::Leaf(term) if term.name_ref().is_some() => {
            resolve_qualified_name(file, term.name_ref()?)
        }
        _ => None,
    }
}

fn parent_has_reference_prefix(
    file: &TastyFile<'_>,
    tree: &RawTree<'_>,
    package: &str,
    depth: usize,
) -> bool {
    const MAX_PARENT_PROFILE_DEPTH: usize = 256;
    if depth >= MAX_PARENT_PROFILE_DEPTH {
        return false;
    }
    match tree {
        RawTree::Leaf(term) if term.ast_ref().is_some() => term
            .ast_ref()
            .and_then(|reference| decode_shared_tree(file, reference.address))
            .is_some_and(|shared| parent_has_reference_prefix(file, &shared, package, depth + 1)),
        RawTree::Ast { child, .. } => parent_has_reference_prefix(file, child, package, depth + 1),
        _ => match tree.decode_structured() {
            Ok(StructuredTree::Reference(ReferenceNode {
                tag: TERMREFPKG_TAG | TYPEREFPKG_TAG,
                ..
            }))
            | Ok(StructuredTree::Select(_)) => true,
            Ok(StructuredTree::Reference(ReferenceNode {
                tag: TERMREF_TAG | TYPEREF_TAG,
                qualifier,
                ..
            })) => resolve_reference_prefix_in_package(file, &qualifier, Some(package)).is_some(),
            Ok(StructuredTree::Length(StructuredNode::AppliedType(applied))) => {
                parent_has_reference_prefix(file, &applied.tycon, package, depth + 1)
            }
            Ok(StructuredTree::Length(StructuredNode::TypeApply(apply))) => {
                parent_has_reference_prefix(file, &apply.function, package, depth + 1)
            }
            Ok(StructuredTree::Length(StructuredNode::Apply(apply))) => {
                parent_has_reference_prefix(file, &apply.function, package, depth + 1)
            }
            Ok(StructuredTree::Length(StructuredNode::SelectIn(selection))) => {
                parent_has_reference_prefix(file, &selection.qualifier, package, depth + 1)
                    || parent_has_reference_prefix(
                        file,
                        &selection.underlying_type,
                        package,
                        depth + 1,
                    )
            }
            Ok(StructuredTree::Length(StructuredNode::Typed(typed))) => {
                parent_has_reference_prefix(file, &typed.type_tree, package, depth + 1)
                    || parent_has_reference_prefix(file, &typed.expression, package, depth + 1)
            }
            Ok(StructuredTree::Length(StructuredNode::SelectOuter(selection))) => {
                parent_has_reference_prefix(file, &selection.underlying_type, package, depth + 1)
                    || parent_has_reference_prefix(file, &selection.qualifier, package, depth + 1)
            }
            Ok(StructuredTree::Ident(ident)) => {
                parent_has_reference_prefix(file, &ident.type_tree, package, depth + 1)
            }
            Ok(StructuredTree::AstChild(child)) => {
                parent_has_reference_prefix(file, &child.child, package, depth + 1)
            }
            _ => {
                if let RawTree::NatAst { child, .. } = tree {
                    parent_has_reference_prefix(file, child, package, depth + 1)
                } else {
                    matches!(
                        resolve_reference_name_in_package(file, tree, Some(package)),
                        ReferenceResolution::Resolved(_)
                    )
                }
            }
        },
    }
}

fn parent_raw_shape(tree: &RawTree<'_>, depth: usize) -> String {
    const MAX_PARENT_PROFILE_DEPTH: usize = 64;
    if depth >= MAX_PARENT_PROFILE_DEPTH {
        return "DepthLimit".to_owned();
    }
    match tree {
        RawTree::Leaf(term) => format!("Leaf(tag={})", term.tag),
        RawTree::Ast { tag, child, .. } => {
            format!(
                "Ast(tag={tag}, child={})",
                parent_raw_shape(child, depth + 1)
            )
        }
        RawTree::NatAst { tag, child, .. } => format!(
            "NatAst(tag={tag}, child={})",
            parent_raw_shape(child, depth + 1)
        ),
        RawTree::LengthNode(node) => match node.decode_structured() {
            Ok(StructuredNode::Apply(apply)) => format!(
                "Apply(function={}, arguments={})",
                parent_raw_shape(&apply.function, depth + 1),
                apply.arguments.len()
            ),
            Ok(StructuredNode::TypeApply(apply)) => format!(
                "TypeApply(function={}, arguments={})",
                parent_raw_shape(&apply.function, depth + 1),
                apply.type_arguments.len()
            ),
            Ok(StructuredNode::AppliedType(applied)) => format!(
                "AppliedType(tycon={}, arguments={})",
                parent_raw_shape(&applied.tycon, depth + 1),
                applied.arguments.len()
            ),
            Ok(StructuredNode::Typed(typed)) => format!(
                "Typed(expression={}, type={})",
                parent_raw_shape(&typed.expression, depth + 1),
                parent_raw_shape(&typed.type_tree, depth + 1)
            ),
            Ok(StructuredNode::SelectOuter(selection)) => format!(
                "SelectOuter(qualifier={}, underlying={})",
                parent_raw_shape(&selection.qualifier, depth + 1),
                parent_raw_shape(&selection.underlying_type, depth + 1)
            ),
            Ok(StructuredNode::SelectIn(selection)) => format!(
                "SelectIn(tag={}, name={}, qualifier={}, underlying={})",
                selection.tag,
                selection.name,
                parent_raw_shape(&selection.qualifier, depth + 1),
                parent_raw_shape(&selection.underlying_type, depth + 1)
            ),
            _ => format!("LengthNode(tag={})", node.tag),
        },
    }
}

fn parent_visible_names(file: &TastyFile<'_>, tree: &RawTree<'_>) -> Vec<String> {
    parent_visible_names_at_depth(file, tree, 0)
}

fn parent_visible_names_at_depth(
    file: &TastyFile<'_>,
    tree: &RawTree<'_>,
    depth: usize,
) -> Vec<String> {
    const MAX_PARENT_PROFILE_DEPTH: usize = 256;
    if depth >= MAX_PARENT_PROFILE_DEPTH {
        return Vec::new();
    }
    let mut names = tree
        .name_refs()
        .into_iter()
        .filter_map(|reference| wire_name(file, reference))
        .collect::<Vec<_>>();
    if let RawTree::Leaf(term) = tree
        && let Some(reference) = term.name_ref()
    {
        if let Some(name) = wire_name(file, reference) {
            names.push(name);
        } else if matches!(term.tag, TERMREFPKG_TAG | TYPEREFPKG_TAG)
            && let Some(name) = resolve_qualified_name(file, reference)
        {
            names.push(name);
        }
    }
    if let Ok(StructuredTree::Length(StructuredNode::AppliedType(applied))) =
        tree.decode_structured()
    {
        names.extend(parent_visible_names_at_depth(
            file,
            &applied.tycon,
            depth + 1,
        ));
    } else if let Ok(StructuredTree::Length(StructuredNode::TypeApply(apply))) =
        tree.decode_structured()
    {
        names.extend(parent_visible_names_at_depth(
            file,
            &apply.function,
            depth + 1,
        ));
        for argument in &apply.type_arguments {
            names.extend(parent_visible_names_at_depth(file, argument, depth + 1));
        }
    } else if let RawTree::Leaf(term) = tree
        && let Some(reference) = term.ast_ref()
        && let Some(shared) = decode_shared_tree(file, reference.address)
    {
        names.extend(parent_visible_names_at_depth(file, &shared, depth + 1));
    } else if let Ok(StructuredTree::Reference(reference)) = tree.decode_structured() {
        if let Some(name) = wire_name(file, reference.reference) {
            names.push(name);
        }
        names.extend(parent_visible_names_at_depth(
            file,
            &reference.qualifier,
            depth + 1,
        ));
    } else if let RawTree::Ast { child, .. } | RawTree::NatAst { child, .. } = tree {
        names.extend(parent_visible_names_at_depth(file, child, depth + 1));
    } else if let Ok(StructuredTree::Length(StructuredNode::Apply(apply))) =
        tree.decode_structured()
    {
        names.extend(parent_visible_names_at_depth(
            file,
            &apply.function,
            depth + 1,
        ));
    } else if let Ok(StructuredTree::Length(StructuredNode::SelectIn(selection))) =
        tree.decode_structured()
    {
        if let Some(name) = wire_name(file, selection.name) {
            names.push(name);
        }
        names.extend(parent_visible_names_at_depth(
            file,
            &selection.qualifier,
            depth + 1,
        ));
        names.extend(parent_visible_names_at_depth(
            file,
            &selection.underlying_type,
            depth + 1,
        ));
    } else if let Ok(StructuredTree::Length(StructuredNode::Typed(typed))) =
        tree.decode_structured()
    {
        names.extend(parent_visible_names_at_depth(
            file,
            &typed.type_tree,
            depth + 1,
        ));
        names.extend(parent_visible_names_at_depth(
            file,
            &typed.expression,
            depth + 1,
        ));
    } else if let Ok(StructuredTree::Select(selection)) = tree.decode_structured() {
        if let Some(name) = wire_name(file, selection.name) {
            names.push(name);
        }
        names.extend(parent_visible_names_at_depth(
            file,
            &selection.qualifier,
            depth + 1,
        ));
    }
    names.sort();
    names.dedup();
    names
}

fn parent_has_semantic_reference(file: &TastyFile<'_>, tree: &RawTree<'_>, package: &str) -> bool {
    parent_has_semantic_reference_at_depth(file, tree, package, 0)
}

fn parent_has_semantic_reference_at_depth(
    file: &TastyFile<'_>,
    tree: &RawTree<'_>,
    package: &str,
    depth: usize,
) -> bool {
    const MAX_PARENT_PROFILE_DEPTH: usize = 256;
    if depth >= MAX_PARENT_PROFILE_DEPTH {
        return false;
    }
    if matches!(
        resolve_reference_name_in_package(file, tree, Some(package)),
        ReferenceResolution::Resolved(_) | ReferenceResolution::Unresolved
    ) {
        return true;
    }
    match tree {
        RawTree::Leaf(term)
            if matches!(
                term.tag,
                TERMREFPKG_TAG | TYPEREFPKG_TAG | TERMREF_TAG | TYPEREF_TAG
            ) =>
        {
            true
        }
        RawTree::Leaf(term) if term.ast_ref().is_some() => term
            .ast_ref()
            .and_then(|reference| decode_shared_tree(file, reference.address))
            .is_some_and(|shared| {
                parent_has_semantic_reference_at_depth(file, &shared, package, depth + 1)
            }),
        RawTree::Ast { child, .. } | RawTree::NatAst { child, .. } => {
            parent_has_semantic_reference_at_depth(file, child, package, depth + 1)
        }
        _ => match tree.decode_structured() {
            Ok(StructuredTree::Length(StructuredNode::AppliedType(applied))) => {
                parent_has_semantic_reference_at_depth(file, &applied.tycon, package, depth + 1)
            }
            Ok(StructuredTree::Length(StructuredNode::Apply(apply))) => {
                parent_has_semantic_reference_at_depth(file, &apply.function, package, depth + 1)
            }
            Ok(StructuredTree::Length(StructuredNode::TypeApply(apply))) => {
                parent_has_semantic_reference_at_depth(file, &apply.function, package, depth + 1)
            }
            Ok(StructuredTree::Length(StructuredNode::SelectIn(selection))) => {
                parent_has_semantic_reference_at_depth(
                    file,
                    &selection.qualifier,
                    package,
                    depth + 1,
                ) || parent_has_semantic_reference_at_depth(
                    file,
                    &selection.underlying_type,
                    package,
                    depth + 1,
                )
            }
            Ok(StructuredTree::Length(StructuredNode::Typed(typed))) => {
                parent_has_semantic_reference_at_depth(file, &typed.type_tree, package, depth + 1)
                    || parent_has_semantic_reference_at_depth(
                        file,
                        &typed.expression,
                        package,
                        depth + 1,
                    )
            }
            Ok(StructuredTree::Length(StructuredNode::SelectOuter(selection))) => {
                parent_has_semantic_reference_at_depth(
                    file,
                    &selection.underlying_type,
                    package,
                    depth + 1,
                ) || parent_has_semantic_reference_at_depth(
                    file,
                    &selection.qualifier,
                    package,
                    depth + 1,
                )
            }
            Ok(StructuredTree::Ident(ident)) => {
                parent_has_semantic_reference_at_depth(file, &ident.type_tree, package, depth + 1)
            }
            Ok(StructuredTree::Select(selection)) => parent_has_semantic_reference_at_depth(
                file,
                &selection.qualifier,
                package,
                depth + 1,
            ),
            _ => false,
        },
    }
}

fn parent_reference_shape(file: &TastyFile<'_>, tree: &RawTree<'_>) -> String {
    parent_reference_shape_at_depth(file, tree, 0)
}

fn parent_reference_shape_at_depth(
    file: &TastyFile<'_>,
    tree: &RawTree<'_>,
    depth: usize,
) -> String {
    const MAX_PARENT_PROFILE_DEPTH: usize = 256;
    if depth >= MAX_PARENT_PROFILE_DEPTH {
        return "depth-limit".to_owned();
    }
    match tree.decode_structured() {
        Ok(StructuredTree::Reference(reference)) => match reference.tag {
            TERMREFPKG_TAG | TYPEREFPKG_TAG => "package-qualified".to_owned(),
            TERMREFSYMBOL_TAG | TYPEREFSYMBOL_TAG => "direct-symbol".to_owned(),
            TERMREF_TAG | TYPEREF_TAG => {
                if parent_has_reference_prefix(file, &reference.qualifier, "", depth + 1) {
                    "package-qualified".to_owned()
                } else {
                    "qualified-reference".to_owned()
                }
            }
            _ => "reference".to_owned(),
        },
        Ok(StructuredTree::Select(_)) => "selected-qualified".to_owned(),
        Ok(StructuredTree::Length(StructuredNode::AppliedType(_))) => "applied".to_owned(),
        Ok(StructuredTree::Length(StructuredNode::TypeApply(apply))) => {
            parent_reference_shape_at_depth(file, &apply.function, depth + 1)
        }
        Ok(StructuredTree::Length(StructuredNode::Apply(apply))) => {
            parent_reference_shape_at_depth(file, &apply.function, depth + 1)
        }
        Ok(StructuredTree::Length(StructuredNode::SelectIn(selection))) => {
            let qualifier_shape =
                parent_reference_shape_at_depth(file, &selection.qualifier, depth + 1);
            let underlying_shape =
                parent_reference_shape_at_depth(file, &selection.underlying_type, depth + 1);
            if qualifier_shape == "package-qualified" || underlying_shape == "package-qualified" {
                "package-qualified".to_owned()
            } else if qualifier_shape != "other" {
                qualifier_shape
            } else {
                underlying_shape
            }
        }
        Ok(StructuredTree::Length(StructuredNode::Typed(typed))) => {
            let ty_shape = parent_reference_shape_at_depth(file, &typed.type_tree, depth + 1);
            if ty_shape == "other" {
                parent_reference_shape_at_depth(file, &typed.expression, depth + 1)
            } else {
                ty_shape
            }
        }
        Ok(StructuredTree::Length(StructuredNode::SelectOuter(selection))) => {
            parent_reference_shape_at_depth(file, &selection.underlying_type, depth + 1)
        }
        Ok(StructuredTree::Ident(ident)) => {
            let inner = parent_reference_shape_at_depth(file, &ident.type_tree, depth + 1);
            if inner == "other" {
                "identifier-type-reference".to_owned()
            } else {
                inner
            }
        }
        _ => match tree {
            RawTree::Leaf(term) if term.ast_ref().is_some() => term
                .ast_ref()
                .and_then(|reference| decode_shared_tree(file, reference.address))
                .map(|shared| parent_reference_shape_at_depth(file, &shared, depth + 1))
                .unwrap_or_else(|| "shared".to_owned()),
            RawTree::Leaf(term) if matches!(term.tag, TERMREFPKG_TAG | TYPEREFPKG_TAG) => {
                "package-qualified".to_owned()
            }
            RawTree::Ast { child, .. } | RawTree::NatAst { child, .. } => {
                parent_reference_shape_at_depth(file, child, depth + 1)
            }
            _ if matches!(
                resolve_reference_name(file, tree),
                ReferenceResolution::Resolved(_)
            ) =>
            {
                "qualified-reference".to_owned()
            }
            _ => "other".to_owned(),
        },
    }
}

/// Reconstructs a field for every primary-constructor term parameter that is
/// also a member: a `class C(val x: Int)` / `class C(var x: Int)` /
/// `case class C(x: Int)` parameter.
///
/// A member parameter is recognised by **not** being `private[this]`, i.e. by
/// not carrying both `PRIVATE` and `LOCAL`, or by carrying
/// [`CASEACCESSOR_TAG`] / [`FIELDACCESSOR_TAG`]. `val x` has an empty modifier
/// tail and `var x` only `MUTABLE`: `FIELDACCESSOR` is on the generated
/// *setter* (`x_=`), not on the parameter, so looking only for the accessor
/// tags missed every ordinary `val`/`var` parameter (issue #11).
/// `private val x` and `protected val x` keep their `PRIVATE`/`PROTECTED`
/// modifier without `LOCAL`, so they are fields too, with that access.
///
/// A plain constructor parameter (`class C(x: Int)`) is `private[this]` and is
/// skipped: it is only ever a constructor-local binding at the source level,
/// even though real Scala bytecode happens to also retain it as a private
/// synthetic JVM field to support later use inside the class body — an
/// implementation detail this reconstructs the *source-level* member set, not
/// the compiled one, so it stays unmodeled, mirroring how `.class` loading
/// does not surface a JVM-only synthetic field as a "real" member either. An
/// explicit `private[this] val x` has the same tail and is skipped with it.
///
/// `template.stats` never repeats these parameters as separate `ValDef`s
/// (dotty's own pickler does not duplicate a constructor-parameter field's
/// declaration), so this is the *only* source of these fields —
/// [`decode_members`] alone would silently miss every one of them.
fn decode_constructor_accessor_fields(
    file: &TastyFile<'_>,
    term_params: &[ParameterNode<'_>],
    package: &str,
    resolve_bare: &mut impl FnMut(&str, &str) -> Option<BinaryName>,
) -> Result<Vec<DecodedTastyField>, TastyDecodeError> {
    let mut fields = Vec::new();

    for parameter in term_params {
        let ParameterNode::TermParam { .. } = parameter else {
            continue;
        };
        let body = parameter.decode_body()?;
        if !is_member_parameter(&body.tail) {
            continue;
        }
        let Some(field_name) = wire_name(file, parameter.name()) else {
            continue;
        };
        fields.push(DecodedTastyField {
            name: field_name,
            flags: decode_field_flags(&body.tail),
            declared_type: resolve_parent_name_with_bare(
                file,
                &body.type_tree,
                package,
                resolve_bare,
            ),
        });
    }

    Ok(fields)
}

/// Whether a constructor term parameter with this modifier tail is a member
/// of its class (see [`decode_constructor_accessor_fields`]).
fn is_member_parameter(tail: &[DefinitionTail<'_>]) -> bool {
    let has = |wanted: u8| {
        tail.iter()
            .any(|item| matches!(item, DefinitionTail::Modifier(tag) if *tag == wanted))
    };
    has(FIELDACCESSOR_TAG) || has(CASEACCESSOR_TAG) || !(has(PRIVATE_TAG) && has(LOCAL_TAG))
}

/// Decodes the supported subset of a TASTy member type while retaining
/// references to the enclosing method's exact type-parameter AST nodes.
fn decode_tasty_member_type(
    file: &TastyFile<'_>,
    tree: &RawTree<'_>,
    package: &str,
    type_parameters: &HashMap<u32, u32>,
    class_type_parameters: &HashMap<u32, u32>,
    resolve_bare: &mut impl FnMut(&str, &str) -> Option<BinaryName>,
    depth: usize,
) -> Option<DecodedTastyType> {
    const MAX_MEMBER_TYPE_DEPTH: usize = 128;
    if depth >= MAX_MEMBER_TYPE_DEPTH {
        return None;
    }

    if let RawTree::Leaf(term) = tree
        && matches!(term.tag, SHAREDTYPE_TAG | SHAREDTERM_TAG)
    {
        let reference = term.ast_ref()?;
        let shared = decode_shared_tree(file, reference.address)?;
        return decode_tasty_member_type(
            file,
            &shared,
            package,
            type_parameters,
            class_type_parameters,
            resolve_bare,
            depth + 1,
        );
    }

    if let Ok(StructuredTree::Ident(ident)) = tree.decode_structured()
        && ident.tag == IDENTTPT_TAG
    {
        return decode_tasty_member_type(
            file,
            &ident.type_tree,
            package,
            type_parameters,
            class_type_parameters,
            resolve_bare,
            depth + 1,
        );
    }

    if let RawTree::Leaf(term) = tree
        && term.tag == TYPEREFDIRECT_TAG
        && let Some(reference) = term.ast_ref()
        && let Some(&index) = type_parameters.get(&reference.address)
    {
        return Some(DecodedTastyType::ParamRef(index));
    }
    if let RawTree::Leaf(term) = tree
        && term.tag == TYPEREFDIRECT_TAG
        && let Some(reference) = term.ast_ref()
        && let Some(&index) = class_type_parameters.get(&reference.address)
    {
        return Some(DecodedTastyType::ClassParamRef(index));
    }

    if let Ok(StructuredTree::Reference(ReferenceNode {
        tag: TYPEREFSYMBOL_TAG,
        reference,
        ..
    })) = tree.decode_structured()
        && let Some(&index) = type_parameters.get(&reference)
    {
        return Some(DecodedTastyType::ParamRef(index));
    }
    if let Ok(StructuredTree::Reference(ReferenceNode {
        tag: TYPEREFSYMBOL_TAG,
        reference,
        ..
    })) = tree.decode_structured()
        && let Some(&index) = class_type_parameters.get(&reference)
    {
        return Some(DecodedTastyType::ClassParamRef(index));
    }

    if let Ok(StructuredTree::Length(StructuredNode::AppliedType(applied))) =
        tree.decode_structured()
    {
        let tycon = decode_tasty_member_type(
            file,
            &applied.tycon,
            package,
            type_parameters,
            class_type_parameters,
            resolve_bare,
            depth + 1,
        )?;
        let arguments = applied
            .arguments
            .iter()
            .map(|argument| {
                decode_tasty_member_type(
                    file,
                    argument,
                    package,
                    type_parameters,
                    class_type_parameters,
                    resolve_bare,
                    depth + 1,
                )
            })
            .collect::<Option<Vec<_>>>()?;
        return Some(DecodedTastyType::Applied {
            tycon: Box::new(tycon),
            arguments,
        });
    }

    if let ReferenceResolution::Resolved(name) = resolve_reference_name(file, tree)
        && name.as_internal() == "scala/Any"
    {
        // `Any` is the Scala top type, not the JVM superclass used when
        // reducing `AnyRef`/`Object` in a class hierarchy. Preserve its
        // canonical identity in value/member types such as Poly bounds.
        return Some(DecodedTastyType::Named(name));
    }

    resolve_parent_name_with_bare(file, tree, package, resolve_bare).map(DecodedTastyType::Named)
}

fn decode_tasty_type_parameter(
    file: &TastyFile<'_>,
    parameter: &ParameterNode<'_>,
    package: &str,
    type_parameters: &HashMap<u32, u32>,
    class_type_parameters: &HashMap<u32, u32>,
    resolve_bare: &mut impl FnMut(&str, &str) -> Option<BinaryName>,
) -> Option<DecodedTastyTypeParameter> {
    let ParameterNode::TypeParam { name, .. } = parameter else {
        return None;
    };
    let parameter_name = wire_name(file, *name)?;
    let mut decoded = DecodedTastyTypeParameter {
        name: parameter_name,
        lower_bound: None,
        upper_bound: None,
        variance: None,
        bounds_supported: false,
    };
    let Some(body) = parameter.decode_body().ok() else {
        return Some(decoded);
    };
    let shared_type_tree = match &body.type_tree {
        RawTree::Leaf(term) if matches!(term.tag, SHAREDTYPE_TAG | SHAREDTERM_TAG) => term
            .ast_ref()
            .and_then(|reference| decode_shared_tree(file, reference.address)),
        _ => None,
    };
    let type_tree = shared_type_tree.as_ref().unwrap_or(&body.type_tree);
    let Ok(StructuredTree::Length(StructuredNode::TypeBounds(bounds))) =
        type_tree.decode_structured()
    else {
        return Some(decoded);
    };
    // `TYPEBOUNDStpt` carries its alias in `alias`, while a one-child
    // `TYPEBOUNDS` is itself the alias form used by method type-parameter
    // headers. Neither shape is an ordinary lower bound with an implicit
    // `Any` upper bound, so leave it unsupported rather than widening it.
    if bounds.alias.is_some() || bounds.high.is_none() {
        return Some(decoded);
    }

    let lower_bound = decode_tasty_member_type(
        file,
        &bounds.low_or_alias,
        package,
        type_parameters,
        class_type_parameters,
        resolve_bare,
        0,
    );
    let upper_bound = bounds.high.as_ref().map(|high| {
        decode_tasty_member_type(
            file,
            high,
            package,
            type_parameters,
            class_type_parameters,
            resolve_bare,
            0,
        )
    });
    if let Some(lower_bound) = lower_bound {
        if let Some(Some(upper_bound)) = upper_bound {
            decoded.upper_bound = Some(upper_bound);
            decoded.bounds_supported = true;
        }
        if decoded.bounds_supported {
            decoded.lower_bound = Some(lower_bound);
            decoded.variance = body.tail.iter().find_map(|item| match item {
                DefinitionTail::Modifier(tag)
                    if matches!(*tag, COVARIANT_TAG | CONTRAVARIANT_TAG | STABLE_TAG) =>
                {
                    Some(*tag)
                }
                _ => None,
            });
        }
    }
    Some(decoded)
}

/// Reconstructs every `ValDef`/`DefDef` directly in `stats` (a class's
/// `Template.stats`) into a [`DecodedTastyField`]/[`DecodedTastyMethod`].
///
/// Anything else in `stats` — a nested `TypeDef` (an inner class, a type
/// alias, an abstract type member), an `Import`/`Export`, or a bare term
/// statement a compiler-generated template can carry — is skipped: a
/// nested class gets its own top-level `TypeDef` elsewhere in the file
/// (or another file entirely) and is loaded separately, on demand, the
/// same way `.class` never eagerly loads an `InnerClasses` entry either.
///
/// Generic method signatures preserve method-local type parameters and
/// address-based references for the supported named/applied type shapes.
/// Higher-kinded, aliased, dependent, and other unrecognized bound/type
/// shapes remain unresolved rather than being guessed.
fn decode_members(
    file: &TastyFile<'_>,
    stats: &[RawTree<'_>],
    package: &str,
    class_type_parameters: &HashMap<u32, u32>,
    resolve_bare: &mut impl FnMut(&str, &str) -> Option<BinaryName>,
) -> Result<(Vec<DecodedTastyField>, Vec<DecodedTastyMethod>), TastyDecodeError> {
    let mut fields = Vec::new();
    let mut methods = Vec::new();
    let address_index = file.ast_address_index()?;

    for stat in stats {
        let RawTree::LengthNode(node) = stat else {
            continue;
        };

        match node.tag {
            VALDEF_TAG => {
                let StructuredNode::ValDef(DefinitionBody::ValDef {
                    name,
                    type_tree,
                    tail,
                    ..
                }) = node.decode_structured()?
                else {
                    continue;
                };
                let Some(field_name) = wire_name(file, name) else {
                    continue;
                };
                fields.push(DecodedTastyField {
                    name: field_name,
                    flags: decode_field_flags(&tail),
                    declared_type: resolve_parent_name_with_bare(
                        file,
                        &type_tree,
                        package,
                        resolve_bare,
                    ),
                });
            }
            DEFDEF_TAG => {
                let StructuredNode::DefDef(DefDefBody {
                    name,
                    parameters,
                    clauses,
                    return_type,
                    tail,
                    ..
                }) = node.decode_structured()?
                else {
                    continue;
                };
                let Some(method_name) = wire_name(file, name) else {
                    continue;
                };

                let type_parameter_addresses = parameters
                    .iter()
                    .filter(|parameter| matches!(parameter, ParameterNode::TypeParam { .. }))
                    .map(|parameter| {
                        let payload = parameter.payload();
                        address_index
                            .iter()
                            .find(|indexed| {
                                indexed.tag == dotty_tasty::tasty::TYPEPARAM_TAG
                                    && indexed.payload.len() == payload.len()
                                    && indexed.payload.as_ptr() == payload.as_ptr()
                            })
                            .and_then(|indexed| u32::try_from(indexed.offset).ok())
                    })
                    .collect::<Vec<_>>();
                let type_parameters_by_address = type_parameter_addresses
                    .iter()
                    .enumerate()
                    .filter_map(|(index, address)| {
                        let address = (*address)?;
                        Some((address, u32::try_from(index).ok()?))
                    })
                    .collect::<HashMap<_, _>>();
                let type_parameters = parameters
                    .iter()
                    .filter(|parameter| matches!(parameter, ParameterNode::TypeParam { .. }))
                    .map(|parameter| {
                        decode_tasty_type_parameter(
                            file,
                            parameter,
                            package,
                            &type_parameters_by_address,
                            class_type_parameters,
                            resolve_bare,
                        )
                    })
                    .collect::<Option<Vec<_>>>();
                let Some(type_parameters) = type_parameters else {
                    continue;
                };

                let mut decoded_parameters = Vec::with_capacity(parameters.len());
                for parameter in &parameters {
                    let ParameterNode::TermParam {
                        name: parameter_name,
                        ..
                    } = parameter
                    else {
                        continue;
                    };
                    let Some(parameter_name) = wire_name(file, *parameter_name) else {
                        continue;
                    };
                    let declared_type = parameter.decode_body().ok().and_then(|body| {
                        decode_tasty_member_type(
                            file,
                            &body.type_tree,
                            package,
                            &type_parameters_by_address,
                            class_type_parameters,
                            resolve_bare,
                            0,
                        )
                    });
                    decoded_parameters.push(DecodedTastyParameter {
                        name: parameter_name,
                        declared_type,
                    });
                }

                // A constructor's own `return_type` tree is a `This`-typed
                // constructor-call convention, not a normal value type to
                // resolve a name from (JVMS agrees: a `.class` constructor
                // descriptor is always `V`, never resolved either) --
                // see `ClassLoader::lower_tasty_method`'s doc comment for
                // where the loader supplies `Unit` instead.
                let return_type = if method_name == "<init>" {
                    None
                } else {
                    decode_tasty_member_type(
                        file,
                        &return_type,
                        package,
                        &type_parameters_by_address,
                        class_type_parameters,
                        resolve_bare,
                        0,
                    )
                };
                let has_term_clause = parameters
                    .iter()
                    .any(|parameter| matches!(parameter, ParameterNode::TermParam { .. }))
                    || clauses.contains(&EMPTYCLAUSE_TAG);

                methods.push(DecodedTastyMethod {
                    name: method_name,
                    flags: decode_method_flags(&tail),
                    type_parameters,
                    parameters: decoded_parameters,
                    has_term_clause,
                    return_type,
                });
            }
            _ => {}
        }
    }

    Ok((fields, methods))
}

/// The `.tasty` counterpart of [`decode_flags`], for one `ValDef`. Unlike
/// a class, a field has no JVM-only `ACC_INTERFACE`-shaped bit to
/// translate; instead, `val`/`var` (the *absence*/presence of
/// [`MUTABLE_TAG`]) maps onto `ACC_FINAL` — Scala has no separate
/// "final" modifier for a field the way JVMS does, `val` itself already
/// means it.
fn decode_field_flags(tail: &[DefinitionTail<'_>]) -> FieldAccessFlags {
    let mut bits = ACC_PUBLIC;
    let mut mutable = false;

    for item in tail {
        let DefinitionTail::Modifier(tag) = item else {
            continue;
        };

        match *tag {
            MUTABLE_TAG => mutable = true,
            STATIC_TAG => bits |= ACC_STATIC,
            SYNTHETIC_TAG | ARTIFACT_TAG => bits |= ACC_SYNTHETIC,
            PRIVATE_TAG => bits = (bits & !ACC_PUBLIC) | ACC_PRIVATE,
            PROTECTED_TAG => bits = (bits & !ACC_PUBLIC) | ACC_PROTECTED,
            _ => {}
        }
    }

    if !mutable {
        bits |= ACC_FINAL;
    }
    FieldAccessFlags(bits)
}

/// The `.tasty` counterpart of [`decode_flags`], for one `DefDef`.
fn decode_method_flags(tail: &[DefinitionTail<'_>]) -> MethodAccessFlags {
    let mut bits = ACC_PUBLIC;

    for item in tail {
        let DefinitionTail::Modifier(tag) = item else {
            continue;
        };

        match *tag {
            ABSTRACT_TAG => bits |= ACC_ABSTRACT,
            FINAL_TAG => bits |= ACC_FINAL,
            STATIC_TAG => bits |= ACC_STATIC,
            SYNTHETIC_TAG | ARTIFACT_TAG => bits |= ACC_SYNTHETIC,
            PRIVATE_TAG => bits = (bits & !ACC_PUBLIC) | ACC_PRIVATE,
            PROTECTED_TAG => bits = (bits & !ACC_PUBLIC) | ACC_PROTECTED,
            _ => {}
        }
    }

    MethodAccessFlags(bits)
}

fn decode_flags(tail: &[DefinitionTail<'_>]) -> ClassAccessFlags {
    let mut bits = ACC_PUBLIC;

    for item in tail {
        let DefinitionTail::Modifier(tag) = item else {
            continue;
        };

        match *tag {
            TRAIT_TAG => bits |= ACC_INTERFACE | ACC_ABSTRACT,
            ABSTRACT_TAG => bits |= ACC_ABSTRACT,
            FINAL_TAG => bits |= ACC_FINAL,
            PRIVATE_TAG | PROTECTED_TAG => bits &= !ACC_PUBLIC,
            _ => {}
        }
    }

    ClassAccessFlags(bits)
}

/// The `.tasty` counterpart of [`decode_flags`]'s `PRIVATE_TAG`/
/// `PROTECTED_TAG` handling, kept as a real [`DeclaredVisibility`]
/// instead of being collapsed into `ClassAccessFlags`'s single "not
/// public" bit — see [`DeclaredVisibility`]'s doc comment.
fn decode_visibility(
    file: &TastyFile<'_>,
    tail: &[DefinitionTail<'_>],
) -> Option<DeclaredVisibility> {
    tail.iter().find_map(|item| match item {
        DefinitionTail::Modifier(PRIVATE_TAG) => Some(DeclaredVisibility::Private),
        DefinitionTail::Modifier(PROTECTED_TAG) => Some(DeclaredVisibility::Protected),
        DefinitionTail::QualifiedModifier(modifier) => {
            let qualifier = decode_qualifier(file, &modifier.child);
            match modifier.tag {
                PRIVATEQUALIFIED_TAG => Some(DeclaredVisibility::PrivateWithin(qualifier)),
                PROTECTEDQUALIFIED_TAG => Some(DeclaredVisibility::ProtectedWithin(qualifier)),
                _ => None,
            }
        }
        _ => None,
    })
}

fn decode_qualifier(file: &TastyFile<'_>, tree: &RawTree<'_>) -> DeclaredQualifier {
    match tree {
        RawTree::Leaf(term) if term.tag == TYPEREFPKG_TAG => match term.value {
            TermValue::NameRef(name) => resolve_qualified_name(file, name)
                .map_or(DeclaredQualifier::Other, DeclaredQualifier::Package),
            _ => DeclaredQualifier::Other,
        },
        _ => DeclaredQualifier::Other,
    }
}

/// Resolves a name-table reference into a `/`-joined qualified string,
/// recursing through [`RawName::Qualified`] entries — unlike [`wire_name`],
/// which only reads a direct `UTF8` entry (adequate for a plain
/// `IDENT`/`SELECT` chain's per-segment names, but not for a real
/// `TERMREFpkg`'s own `NameRef`, which names a whole package and is
/// commonly a `Qualified` entry itself, e.g. `java.io` is
/// `Qualified { prefix: "java", selector: "io" }`, not one flat `UTF8`
/// entry — `docs/tasty-format-3.9.0.md` §4.2).
fn resolve_qualified_name(file: &TastyFile<'_>, reference: u32) -> Option<String> {
    resolve_qualified_name_at_depth(file, reference, 0)
}

fn resolve_qualified_name_at_depth(
    file: &TastyFile<'_>,
    reference: u32,
    depth: usize,
) -> Option<String> {
    const MAX_NAME_DEPTH: usize = 256;
    if depth >= MAX_NAME_DEPTH {
        return None;
    }
    match file.names().entries().get(reference as usize)? {
        RawName::Utf8(text) => Some(text.clone()),
        RawName::Qualified { prefix, selector } => {
            let prefix = resolve_qualified_name_at_depth(file, *prefix, depth + 1)?;
            let selector = resolve_qualified_name_at_depth(file, *selector, depth + 1)?;
            Some(format!("{prefix}/{selector}"))
        }
        RawName::Signed {
            result_signature, ..
        } => resolve_qualified_name_at_depth(file, *result_signature, depth + 1),
        _ => None,
    }
}

/// Decodes a fresh [`RawTree`] starting at `address` in the `ASTs`
/// section — what a `SHAREDterm`/`SHAREDtype` reference
/// (`docs/tasty-format-3.9.0.md` §6.5) points at: the compiler serializes
/// a repeated subtree once and has every other occurrence reference its
/// address, so following one means decoding a second, independent tree
/// rooted at that address, not looking anything up in an already-built
/// one.
fn decode_shared_tree<'a>(file: &TastyFile<'a>, address: u32) -> Option<RawTree<'a>> {
    let section = file.section(StandardSection::Asts)?;
    let mut reader =
        Reader::with_range(section.payload, address as usize, section.payload.len()).ok()?;
    RawTree::decode_with_base_offset(&mut reader, 0).ok()
}

/// Resolves the package path named by a real, post-typecheck `TYPEREF`/
/// `TERMREF`'s prefix subtree (`docs/tasty-format-3.9.0.md` §6.5's
/// `Type`/`Path` grammar) — only the shapes this decoder actually
/// understands:
///
/// - `TERMREFpkg`: by far the common case for a reference to another
///   compiled class — its `NameRef` names the package directly (see
///   [`resolve_qualified_name`]).
/// - `SHAREDterm`/`SHAREDtype`: the compiler shares a repeated prefix
///   subtree by address instead of re-emitting it; followed via
///   [`decode_shared_tree`] and resolved recursively.
///
/// Other prefix shapes — `THIS` (a nested class's own enclosing
/// instance), `TERMREFin`/`TYPEREFin` (disambiguating an owner from a
/// name clash), direct symbol references, and references nested inside
/// another object/module path — return `None` here. Direct
/// `TERMREFsymbol`/`TYPEREFsymbol` targets are handled by
/// [`resolve_reference_name`]; this helper stays limited to package
/// prefixes so it does not guess at an owner path.
fn resolve_reference_prefix(file: &TastyFile<'_>, prefix: &RawTree<'_>) -> Option<String> {
    resolve_reference_prefix_in_package(file, prefix, None)
}

fn resolve_reference_prefix_in_package(
    file: &TastyFile<'_>,
    prefix: &RawTree<'_>,
    current_package: Option<&str>,
) -> Option<String> {
    resolve_reference_prefix_at_depth(file, prefix, current_package, 0)
}

fn resolve_reference_prefix_at_depth(
    file: &TastyFile<'_>,
    prefix: &RawTree<'_>,
    current_package: Option<&str>,
    depth: usize,
) -> Option<String> {
    const MAX_REFERENCE_DEPTH: usize = 256;
    if depth >= MAX_REFERENCE_DEPTH {
        return None;
    }
    match prefix {
        RawTree::Leaf(term) if term.tag == TERMREFPKG_TAG => {
            let reference = term.name_ref()?;
            let package = resolve_qualified_name(file, reference)?;
            Some(if package == "_root_" {
                String::new()
            } else {
                package
            })
        }
        RawTree::Leaf(term) if matches!(term.tag, SHAREDTERM_TAG | SHAREDTYPE_TAG) => {
            let reference = term.ast_ref()?;
            if let Some(name) = resolve_symbol_address(file, reference.address) {
                return Some(qualify_symbol_address(name, current_package));
            }
            let shared = decode_shared_tree(file, reference.address)?;
            resolve_reference_prefix_at_depth(file, &shared, current_package, depth + 1)
        }
        RawTree::Leaf(term) if term.ast_ref().is_some() => {
            let reference = term.ast_ref()?;
            if let Some(name) = resolve_symbol_address(file, reference.address) {
                return Some(qualify_symbol_address(name, current_package));
            }
            let target = decode_shared_tree(file, reference.address)?;
            if let ReferenceResolution::Resolved(name) =
                resolve_reference_name_at_depth(file, &target, current_package, depth + 1)
            {
                return Some(name.as_internal().to_owned());
            }
            resolve_reference_prefix_at_depth(file, &target, current_package, depth + 1)
        }
        RawTree::Leaf(term) if term.name_ref().is_some() => {
            resolve_qualified_name(file, term.name_ref()?)
        }
        _ => match prefix.decode_structured().ok()? {
            StructuredTree::AstChild(child) => {
                resolve_reference_prefix_at_depth(file, &child.child, current_package, depth + 1)
            }
            StructuredTree::Length(StructuredNode::AppliedType(AppliedTypeNode {
                tycon, ..
            })) => resolve_reference_prefix_at_depth(file, &tycon, current_package, depth + 1),
            StructuredTree::Ident(IdentNode { type_tree, .. }) => {
                resolve_reference_prefix_at_depth(file, &type_tree, current_package, depth + 1)
            }
            StructuredTree::Select(SelectNode {
                tag,
                name,
                qualifier,
            }) => {
                let segment = wire_name(file, name)?;
                let owner = resolve_reference_prefix_at_depth(
                    file,
                    &qualifier,
                    current_package,
                    depth + 1,
                )?;
                let selected_type_member = tag == dotty_tasty::tasty::SELECTTPT_TAG
                    && matches!(qualifier, RawTree::Leaf(term) if term.ast_ref().is_some());
                Some(if selected_type_member && !owner.is_empty() {
                    format!("{}{}{}", owner.trim_matches('/'), "$", segment)
                } else if owner.is_empty() || owner == "_root_" {
                    segment
                } else {
                    format!("{}/{segment}", owner.trim_matches('/'))
                })
            }
            StructuredTree::Reference(ReferenceNode {
                tag: TERMREFPKG_TAG,
                reference,
                qualifier,
            }) => {
                let segment = resolve_qualified_name(file, reference)?;
                let owner =
                    resolve_reference_prefix_at_depth(file, &qualifier, current_package, depth + 1);
                match owner.as_deref() {
                    Some("") | Some("/") | Some("_root_") | None => Some(segment),
                    Some(owner) => {
                        let owner = owner.trim_matches('/');
                        if owner.is_empty() {
                            Some(segment)
                        } else {
                            Some(format!("{owner}/{segment}"))
                        }
                    }
                }
            }
            StructuredTree::Reference(ReferenceNode {
                tag:
                    TERMREF_TAG
                    | TYPEREF_TAG
                    | dotty_tasty::tasty::TERMREFDIRECT_TAG
                    | dotty_tasty::tasty::TYPEREFDIRECT_TAG
                    | dotty_tasty::tasty::TERMREFIN_TAG
                    | dotty_tasty::tasty::TYPEREFIN_TAG,
                reference,
                qualifier,
            }) => {
                let name = resolve_qualified_name(file, reference)?;
                let prefix = resolve_reference_prefix_at_depth(
                    file,
                    &qualifier,
                    current_package,
                    depth + 1,
                )?;
                let prefix = prefix.trim_matches('/');
                if prefix.is_empty() || prefix == "_root_" {
                    Some(name)
                } else {
                    Some(format!("{prefix}/{name}"))
                }
            }
            _ => None,
        },
    }
}

fn qualify_symbol_address(name: String, current_package: Option<&str>) -> String {
    match current_package {
        Some(package) if !package.is_empty() && !name.contains('/') => {
            format!("{package}/{name}")
        }
        _ => name,
    }
}

/// What [`resolve_reference_name`] learned about a tree.
///
/// A plain `Option<BinaryName>` cannot distinguish "this isn't a reference
/// shape I understand at all, try your own heuristic" from "this
/// unambiguously *is* a real, post-typecheck reference, but I can't name
/// its prefix" — and [`resolve_parent_name_with_bare`] must treat those two cases
/// completely differently: the first still falls back to its
/// flatten-and-guess heuristic (the only thing that has ever handled
/// pre-typecheck `IDENT`/`SELECT` chains), but the second must not — a
/// same-package guess for a reference we *know* is not same-package
/// (its prefix is real, just unreadable to this decoder) is not a
/// best-effort fallback, it is actively misleading, so it becomes an
/// explicit, reported [`TastyDecodeError::UnresolvedSupertype`] instead
/// (see `docs/classloader.md`'s Milestone 9 follow-up notes and
/// https://github.com/scytrowski/dotty-rs/issues/7).
#[derive(Debug)]
enum ReferenceResolution {
    Resolved(BinaryName),
    /// Definitely a real, post-typecheck reference, but this decoder cannot
    /// name its target — classpath-aware alias lookup may resolve
    /// package-object aliases, but general scope/import resolution is still
    /// outside this best-effort decoder (see [`resolve_reference_prefix`]'s
    /// doc comment for exactly which prefix shapes it does understand).
    Unresolved,
    /// Not a reference shape this function recognizes at all — the caller
    /// should fall back to its own heuristic exactly as if this function
    /// did not exist.
    NotAReference,
}

/// Resolves a real, post-typecheck `TYPEREF`/`TERMREF` node — or one
/// nested inside the elaborated `IDENTTPT` pretty-print form a `.tasty`
/// `extends` clause's own reference tree wraps it in — into a fully
/// qualified [`BinaryName`], using [`resolve_reference_prefix`]'s
/// prefix/name-table semantics instead of [`resolve_parent_name_with_bare`]'s
/// flatten-and-guess fallback.
///
/// Returns [`ReferenceResolution::Unresolved`] — never a bare,
/// unqualified name — when the simple name resolves but its prefix does
/// not. `TERMREFsymbol`/`TYPEREFsymbol` carry AST addresses rather than
/// name-table references; their target is resolved through the AST owner
/// chain, using the current package when the owner chain has no package
/// node. Other unreadable references stay unresolved so the caller does not
/// turn them into a same-package guess.
fn resolve_reference_name(file: &TastyFile<'_>, tree: &RawTree<'_>) -> ReferenceResolution {
    resolve_reference_name_in_package(file, tree, None)
}

fn resolve_reference_name_in_package(
    file: &TastyFile<'_>,
    tree: &RawTree<'_>,
    current_package: Option<&str>,
) -> ReferenceResolution {
    resolve_reference_name_at_depth(file, tree, current_package, 0)
}

fn resolve_reference_name_at_depth(
    file: &TastyFile<'_>,
    tree: &RawTree<'_>,
    current_package: Option<&str>,
    depth: usize,
) -> ReferenceResolution {
    const MAX_REFERENCE_DEPTH: usize = 256;
    if depth >= MAX_REFERENCE_DEPTH {
        return ReferenceResolution::Unresolved;
    }
    if let RawTree::Leaf(term) = tree
        && matches!(term.tag, SHAREDTERM_TAG | SHAREDTYPE_TAG)
    {
        let Some(reference) = term.ast_ref() else {
            return ReferenceResolution::Unresolved;
        };
        let Some(shared) = decode_shared_tree(file, reference.address) else {
            return ReferenceResolution::Unresolved;
        };
        return resolve_reference_name_at_depth(file, &shared, current_package, depth + 1);
    }
    let Ok(structured) = tree.decode_structured() else {
        return ReferenceResolution::NotAReference;
    };
    match structured {
        StructuredTree::Reference(ReferenceNode {
            tag: TERMREF_TAG | TYPEREF_TAG,
            reference,
            qualifier,
        }) => {
            let Some(simple_name) = resolve_qualified_name(file, reference) else {
                return ReferenceResolution::Unresolved;
            };
            let prefix = match current_package {
                Some(_) => resolve_reference_prefix_in_package(file, &qualifier, current_package),
                None => resolve_reference_prefix(file, &qualifier),
            };
            match prefix {
                Some(package) => {
                    let package = package.trim_matches('/');
                    let name = if package.is_empty() {
                        simple_name
                    } else {
                        format!("{package}/{simple_name}")
                    };
                    ReferenceResolution::Resolved(BinaryName::from_internal(name))
                }
                None => ReferenceResolution::Unresolved,
            }
        }
        StructuredTree::Reference(ReferenceNode {
            tag: TERMREFSYMBOL_TAG | TYPEREFSYMBOL_TAG,
            reference,
            ..
        }) => {
            let resolved = resolve_symbol_address(file, reference);
            resolved
                .map(|name| {
                    ReferenceResolution::Resolved(BinaryName::from_internal(
                        qualify_symbol_address(name, current_package),
                    ))
                })
                .unwrap_or(ReferenceResolution::Unresolved)
        }
        StructuredTree::Length(StructuredNode::SelectIn(selection)) => {
            let Some(member) = render_member_name(file, selection.name) else {
                return ReferenceResolution::Unresolved;
            };
            if !member.chars().all(is_binary_name_character) {
                return ReferenceResolution::Unresolved;
            }
            if let ReferenceResolution::Resolved(name) = resolve_reference_name_at_depth(
                file,
                &selection.underlying_type,
                current_package,
                depth + 1,
            ) {
                return ReferenceResolution::Resolved(name);
            }
            let Some(name) = render_member_name(file, selection.name) else {
                return ReferenceResolution::Unresolved;
            };
            let resolution = resolve_reference_prefix_at_depth(
                file,
                &selection.qualifier,
                current_package,
                depth + 1,
            )
            .and_then(|prefix| {
                let prefix = prefix.trim_matches('/');
                if matches!(prefix, "scala/package" | "scala/package$") {
                    return None;
                }
                let selected_name = name.rsplit('/').next().unwrap_or(&name);
                if selected_name == "<init>"
                    && matches!(selection.qualifier, RawTree::Ast { .. })
                    && !prefix.is_empty()
                {
                    Some(ReferenceResolution::Resolved(BinaryName::from_internal(
                        prefix.to_owned(),
                    )))
                } else if prefix.is_empty() || prefix == "_root_" {
                    Some(ReferenceResolution::Resolved(BinaryName::from_internal(
                        name,
                    )))
                } else if matches!(selection.qualifier, RawTree::Ast { .. })
                    && prefix.rsplit('/').next().is_some_and(|segment| {
                        segment == selected_name
                            || segment
                                .strip_suffix(selected_name)
                                .is_some_and(|owner| owner.ends_with('$'))
                    })
                {
                    Some(ReferenceResolution::Resolved(BinaryName::from_internal(
                        prefix.to_owned(),
                    )))
                } else {
                    Some(ReferenceResolution::Resolved(BinaryName::from_internal(
                        format!("{prefix}/{name}"),
                    )))
                }
            });
            resolution.unwrap_or(ReferenceResolution::Unresolved)
        }
        StructuredTree::Select(_) => {
            let Some(name) = parent_wire_path(file, tree, depth + 1)
                .map(|name| name.strip_prefix("_root_/").unwrap_or(&name).to_owned())
                .filter(|name| name.contains('/') && !name.starts_with('/'))
            else {
                return ReferenceResolution::Unresolved;
            };
            if !has_one_terminal_type_selection(file, tree, depth + 1) {
                return ReferenceResolution::Unresolved;
            }
            ReferenceResolution::Resolved(BinaryName::from_internal(name))
        }
        StructuredTree::Ident(IdentNode { type_tree, .. }) => {
            resolve_reference_name_at_depth(file, &type_tree, current_package, depth + 1)
        }
        StructuredTree::Length(StructuredNode::TypeApply(TypeApplyNode { function, .. }))
        | StructuredTree::Length(StructuredNode::Apply(ApplyNode { function, .. })) => {
            resolve_reference_name_at_depth(file, &function, current_package, depth + 1)
        }
        StructuredTree::Length(StructuredNode::AppliedType(AppliedTypeNode { tycon, .. })) => {
            resolve_reference_name_at_depth(file, &tycon, current_package, depth + 1)
        }
        StructuredTree::AstChild(child) => {
            resolve_reference_name_at_depth(file, &child.child, current_package, depth + 1)
        }
        _ => ReferenceResolution::NotAReference,
    }
}

/// Returns true only when this is one terminal type selection whose qualifier
/// is a chain of package references. Term references and selections can name
/// objects or nested types; joining those names with `/` would lose the JVM
/// `$` owner boundary.
fn has_one_terminal_type_selection(file: &TastyFile<'_>, tree: &RawTree<'_>, depth: usize) -> bool {
    fn is_package_qualifier(file: &TastyFile<'_>, tree: &RawTree<'_>, depth: usize) -> bool {
        const MAX_SELECTION_DEPTH: usize = 256;
        if depth >= MAX_SELECTION_DEPTH {
            return false;
        }
        if let RawTree::Leaf(term) = tree
            && matches!(term.tag, SHAREDTERM_TAG | SHAREDTYPE_TAG)
        {
            let Some(reference) = term.ast_ref() else {
                return false;
            };
            let Some(target) = decode_shared_tree(file, reference.address) else {
                return false;
            };
            return is_package_qualifier(file, &target, depth + 1);
        }
        if let RawTree::Leaf(term) = tree {
            return matches!(term.tag, TERMREFPKG_TAG | TYPEREFPKG_TAG)
                && term
                    .name_ref()
                    .and_then(|name| resolve_qualified_name(file, name))
                    .is_some();
        }
        match tree.decode_structured() {
            Ok(StructuredTree::AstChild(child)) => {
                is_package_qualifier(file, &child.child, depth + 1)
            }
            Ok(StructuredTree::Ident(ident)) => {
                is_package_qualifier(file, &ident.type_tree, depth + 1)
            }
            Ok(StructuredTree::Select(selection)) => {
                // In TASTy, a term SELECT rooted in a package may name either
                // a subpackage or an object. Admit only package paths
                // independently covered by pinned Scala/JDK fixtures; do not
                // infer arbitrary selected segments from the current package.
                let selected_package = wire_name(file, selection.name).and_then(|name| {
                    let owner = parent_wire_path(file, &selection.qualifier, depth + 1)?;
                    let owner = owner.strip_prefix("_root_/").unwrap_or(&owner);
                    Some(if owner.is_empty() || owner == "_root_" {
                        name
                    } else {
                        format!("{owner}/{name}")
                    })
                });
                selection.tag == dotty_tasty::tasty::SELECT_TAG
                    && selected_package.as_deref().is_some_and(|package| {
                        matches!(
                            package,
                            "scala" | "scala/annotation" | "java/io" | "java/lang"
                        )
                    })
                    && is_package_qualifier(file, &selection.qualifier, depth + 1)
            }
            Ok(StructuredTree::Reference(reference)) => {
                matches!(reference.tag, TERMREFPKG_TAG | TYPEREFPKG_TAG)
                    && is_package_qualifier(file, &reference.qualifier, depth + 1)
            }
            _ => false,
        }
    }

    let Ok(StructuredTree::Select(selection)) = tree.decode_structured() else {
        return false;
    };
    selection.tag == dotty_tasty::tasty::SELECTTPT_TAG
        && is_package_qualifier(file, &selection.qualifier, depth + 1)
}

/// Resolves a TASTy-local symbol address through its defining tree and owner
/// chain. The address is an AST address, never a name-table index. Only
/// visible type definitions inside a package owner chain produce a binary
/// name; malformed, cyclic, or unsupported shapes stay unresolved.
fn resolve_symbol_address(file: &TastyFile<'_>, address: u32) -> Option<String> {
    const MAX_OWNER_DEPTH: usize = 256;

    let index = file.ast_address_index().ok()?;
    let definition = index.get(address)?;
    if definition.tag != TYPEDEF_TAG {
        return None;
    }
    let StructuredNode::TypeDef(DefinitionBody::TypeDef { name, .. }) =
        definition.decode_structured().ok()?
    else {
        return None;
    };
    let mut names = vec![file.render_name(name).ok()?];
    let mut package = None;
    let mut parent = index.parent_of(address);
    let mut visited = HashSet::new();
    for _ in 0..MAX_OWNER_DEPTH {
        let Some(node) = parent else {
            break;
        };
        let parent_address = u32::try_from(node.offset).ok()?;
        if !visited.insert(parent_address) {
            return None;
        }
        if node.tag == TYPEDEF_TAG {
            let owner = index.get(parent_address)?;
            if let Ok(StructuredNode::TypeDef(DefinitionBody::TypeDef { name, .. })) =
                owner.decode_structured()
            {
                names.push(file.render_name(name).ok()?);
            }
        } else if node.tag == dotty_tasty::tasty::PACKAGE_TAG {
            let package_node = index.get(parent_address)?;
            if let Ok(StructuredNode::Package(package_node)) = package_node.decode_structured() {
                package = package_node
                    .path
                    .decode_structured()
                    .ok()
                    .and_then(|path| match path {
                        StructuredTree::Reference(ReferenceNode {
                            tag: TERMREFPKG_TAG,
                            reference,
                            ..
                        }) => resolve_qualified_name(file, reference),
                        _ => None,
                    });
            }
            parent = None;
            break;
        }
        parent = index.parent_of(parent_address);
    }
    if parent.is_some() {
        return None;
    }
    names.reverse();
    let mut nested_name = names.first().cloned().unwrap_or_default();
    for name in names.iter().skip(1) {
        let name = name.trim_start_matches('$');
        if !nested_name.ends_with('$') {
            nested_name.push('$');
        }
        nested_name.push_str(name);
    }
    let internal = format!(
        "{}{}",
        package
            .map(|package| format!("{package}/"))
            .unwrap_or_default(),
        nested_name
    );
    Some(internal)
}

/// Reconstructs a supertype parent's referenced [`BinaryName`], or
/// `None` when it can't be — either because the parent carries no name
/// reference at all (the implicit `Object`/`AnyRef` superclass
/// constructor call, which `.tasty` encodes without any name-table
/// entry, JVMS §4.1's implicit interface superclass follows the same
/// shape), or because every name reference it does carry fails to
/// render as a plain name.
///
/// Tried first, before any of the flatten-based heuristic below: a real
/// `TYPEREF`/`TERMREF`'s own prefix/name-table semantics
/// ([`resolve_reference_name`]) — this is what an ordinary post-typecheck
/// member type (`Point.x: Int`) and an implicit-import parent reference
/// (`Source`'s `Closeable`, resolved via its `java.io` `TERMREFpkg`
/// prefix, not the current package) both actually are on the wire, and
/// the heuristic below cannot see either correctly: `Int`'s own `TYPEREF`
/// name is invisible to [`RawTree::name_refs`] (`TYPEREF_TAG` is not one
/// of the tags it visits), and `Closeable`'s bare `IDENTTPT` name has no
/// way to know it means `java.io.Closeable` rather than the current
/// package's own `Closeable`.
///
/// [`RawTree::name_refs`] returns every name-table reference visible in
/// a parent tree in source order; empirically (see this crate's
/// Milestone 9 plan), a same-package mixin (`class Dog extends Animal`)
/// carries exactly one directly-named reference (`"Animal"`) followed by
/// an unrelated symbol reference with no direct UTF-8 name, while a
/// cross-package mixin (`case class Point`'s implicit `Product`/
/// `Serializable`) carries a fully-named, reversed qualification chain
/// — sometimes rooted with a leading synthetic `_root_` marker (e.g.
/// `["Product", "scala", "_root_"]`), sometimes not (`Serializable`'s
/// own chain is just `["Serializable", "scala"]`, no `_root_`) — so
/// "already fully qualified" is decided by chain *length* (more than
/// one name-table reference), not by `_root_`'s presence, which is only
/// ever stripped, never load-bearing for the decision. Filtering to
/// only the references that do resolve to a direct name, in order,
/// handles both shapes uniformly: reverse them, and either join a
/// multi-element chain as-is (dropping a leading `_root_` marker first,
/// if present) or, for a single bare name, qualify it with `package` —
/// `.tasty` never repeats the enclosing class's own package for a
/// reference resolved directly in scope, unlike a cross-package
/// reference, which always carries its full chain (see [`decode`]'s doc
/// comment for where `package` comes from).
///
/// This is still best-effort, not full scope resolution: a name that's
/// bare because it comes from an implicit import (`scala._`,
/// `java.lang._`) rather than genuinely being in the same package (an
/// import this decoder does not track) is indistinguishable, from the
/// name table alone, from a genuine same-package reference, and is
/// qualified with `package` the same way — which is wrong for that
/// case, exactly as bare (unqualified) resolution was wrong for it
/// before this fix. Only real symbol resolution can disambiguate the
/// two.
fn resolve_parent_name_with_bare(
    file: &TastyFile<'_>,
    parent: &RawTree<'_>,
    package: &str,
    resolve_bare: &mut impl FnMut(&str, &str) -> Option<BinaryName>,
) -> Option<BinaryName> {
    resolve_parent_name_in_package_with_bare(file, parent, package, Some(package), resolve_bare)
}

fn resolve_parent_name_in_package(
    file: &TastyFile<'_>,
    parent: &RawTree<'_>,
    package: &str,
    package_scope: Option<&str>,
) -> Option<BinaryName> {
    resolve_parent_name_in_package_with_bare(file, parent, package, package_scope, &mut |_, _| None)
}

fn resolve_parent_name_in_package_with_bare(
    file: &TastyFile<'_>,
    parent: &RawTree<'_>,
    package: &str,
    package_scope: Option<&str>,
    resolve_bare: &mut impl FnMut(&str, &str) -> Option<BinaryName>,
) -> Option<BinaryName> {
    resolve_parent_name_in_package_with_bare_at_depth(
        file,
        parent,
        package,
        package_scope,
        resolve_bare,
        0,
    )
    .filter(|name| name.simple_name().chars().all(is_binary_name_character))
}

fn resolve_parent_name_in_package_with_bare_at_depth(
    file: &TastyFile<'_>,
    parent: &RawTree<'_>,
    package: &str,
    package_scope: Option<&str>,
    resolve_bare: &mut impl FnMut(&str, &str) -> Option<BinaryName>,
    depth: usize,
) -> Option<BinaryName> {
    const MAX_PARENT_TREE_DEPTH: usize = 256;
    if depth >= MAX_PARENT_TREE_DEPTH {
        return None;
    }
    if let Some(selection) = find_select_in_reference(parent, 0) {
        let (selected_member, nested_owner) = match selection.decode_structured().ok()? {
            StructuredTree::Length(StructuredNode::SelectIn(selection)) => (
                render_member_name(file, selection.name)?,
                contains_nested_ast_child(&selection.underlying_type, 0),
            ),
            _ => return None,
        };
        // Operator names require Scala's name mangling before they can be
        // used as JVM binary names. Keep them unresolved until that encoding
        // is available instead of treating source spelling as a path.
        if !selected_member.chars().all(is_binary_name_character) {
            return None;
        }
        // A nested AST owner needs a verified JVM nested name. Otherwise a
        // package path guess can turn `Phases.Phase` into `Phases/Phase`.
        if nested_owner {
            return match resolve_reference_name_in_package(file, &selection, package_scope) {
                ReferenceResolution::Resolved(name)
                    if name.simple_name().contains('$') && name.is_path_safe() =>
                {
                    Some(name)
                }
                _ => None,
            };
        }
        match resolve_reference_name_in_package(file, &selection, package_scope) {
            ReferenceResolution::Resolved(name) => return Some(name),
            ReferenceResolution::Unresolved => return None,
            ReferenceResolution::NotAReference => {}
        }
    }
    if let Ok(StructuredTree::Length(StructuredNode::Apply(ApplyNode { function, .. }))) =
        parent.decode_structured()
    {
        return resolve_parent_name_in_package_with_bare_at_depth(
            file,
            &function,
            package,
            package_scope,
            resolve_bare,
            depth + 1,
        );
    }
    let reference = match package_scope {
        Some(_) => resolve_reference_name_in_package(file, parent, package_scope),
        None => resolve_reference_name(file, parent),
    };
    match reference {
        ReferenceResolution::Resolved(resolved) => {
            if !resolved.simple_name().chars().all(is_binary_name_character) {
                return None;
            }
            if matches!(
                resolved.as_internal(),
                "scala/Any" | "scala/AnyRef" | "scala/Object"
            ) {
                return Some(BinaryName::from_internal("java/lang/Object"));
            }
            return Some(resolved);
        }
        // A real semantic reference this decoder cannot fully qualify must
        // not fall through to the package/implicit-import heuristic and come
        // back out as a misleading same-package guess. The narrow package-
        // object alias lookup already ran in `resolve_parent_name_with_alias`.
        ReferenceResolution::Unresolved => {
            let simple_name = unresolved_semantic_reference_name(file, parent)?;
            // Scala 3.9 represents AnyRef as the JVM Object superclass in
            // emitted class hierarchies. This is a compiler-known identity,
            // not a fallback for arbitrary unreadable symbol addresses.
            if matches!(
                simple_name.as_str(),
                "Any" | "AnyRef" | "Object" | "java/lang/Object"
            ) {
                return Some(BinaryName::from_internal("java/lang/Object"));
            }
            return None;
        }
        ReferenceResolution::NotAReference => {}
    }

    let mut names: Vec<String> = parent
        .name_refs()
        .into_iter()
        .filter_map(|reference| wire_name(file, reference))
        .collect();

    if names.iter().any(|name| {
        name.split('/')
            .any(|segment| !segment.chars().all(is_binary_name_character))
    }) {
        return None;
    }

    if names.is_empty() {
        return match package_scope {
            Some(_) => {
                let RawTree::LengthNode(node) = parent else {
                    return None;
                };
                if !matches!(node.tag, APPLIEDTPT_TAG | APPLIEDTYPE_TAG) {
                    return None;
                }
                let StructuredNode::AppliedType(AppliedTypeNode { tycon, .. }) =
                    node.decode_structured().ok()?
                else {
                    return None;
                };
                resolve_parent_name_in_package_with_bare_at_depth(
                    file,
                    &tycon,
                    package,
                    package_scope,
                    resolve_bare,
                    depth + 1,
                )
            }
            None => resolve_applied_type_name(file, parent, package),
        };
    }

    names.reverse();
    let already_qualified = names.len() > 1;
    if names.first().map(String::as_str) == Some("_root_") {
        names.remove(0);
    }
    if names.is_empty() {
        return None;
    }

    if !already_qualified {
        return resolve_bare(package, &names[0]);
    }

    let joined = names.join("/");
    let qualified = if already_qualified || package.is_empty() {
        joined
    } else {
        format!("{package}/{joined}")
    };
    Some(BinaryName::from_internal(qualified))
}

fn find_select_in_reference<'a>(tree: &RawTree<'a>, depth: usize) -> Option<RawTree<'a>> {
    const MAX_DEPTH: usize = 128;
    if depth >= MAX_DEPTH {
        return None;
    }
    if let RawTree::Ast { child, .. } | RawTree::NatAst { child, .. } = tree {
        return find_select_in_reference(child, depth + 1);
    }
    if let RawTree::LengthNode(node) = tree {
        if node.tag == dotty_tasty::tasty::SELECTIN_TAG {
            return Some(tree.clone());
        }
        return match node.decode_structured().ok()? {
            StructuredNode::Apply(apply) => find_select_in_reference(&apply.function, depth + 1),
            StructuredNode::TypeApply(apply) => {
                find_select_in_reference(&apply.function, depth + 1)
            }
            StructuredNode::AppliedType(applied) => {
                find_select_in_reference(&applied.tycon, depth + 1)
            }
            _ => None,
        };
    }
    match tree.decode_structured().ok()? {
        StructuredTree::AstChild(child) => find_select_in_reference(&child.child, depth + 1),
        StructuredTree::Ident(IdentNode { type_tree, .. }) => {
            find_select_in_reference(&type_tree, depth + 1)
        }
        _ => None,
    }
}

fn is_binary_name_character(character: char) -> bool {
    unicode_ident::is_xid_continue(character) || matches!(character, '$' | '<' | '>')
}

fn contains_nested_ast_child(tree: &RawTree<'_>, depth: usize) -> bool {
    const MAX_DEPTH: usize = 128;
    if depth >= MAX_DEPTH {
        return true;
    }
    if matches!(tree, RawTree::Ast { .. }) {
        return true;
    }
    if let RawTree::NatAst { child, .. } = tree {
        return contains_nested_ast_child(child, depth + 1);
    }
    match tree.decode_structured() {
        Ok(StructuredTree::AstChild(child)) => contains_nested_ast_child(&child.child, depth + 1),
        Ok(StructuredTree::Length(StructuredNode::Apply(ApplyNode { function, .. }))) => {
            contains_nested_ast_child(&function, depth + 1)
        }
        Ok(StructuredTree::Length(StructuredNode::AppliedType(AppliedTypeNode {
            tycon, ..
        }))) => contains_nested_ast_child(&tycon, depth + 1),
        Ok(StructuredTree::Ident(IdentNode { type_tree, .. })) => {
            contains_nested_ast_child(&type_tree, depth + 1)
        }
        _ => false,
    }
}

/// Returns the simple name only for an unresolved named TYPEREF/TERMREF,
/// following wrappers used by extends and applied types. Direct symbol refs
/// intentionally return `None`: if their AST address or owner chain is bad,
/// textual name guessing would hide malformed semantic identity.
fn unresolved_semantic_reference_name(file: &TastyFile<'_>, tree: &RawTree<'_>) -> Option<String> {
    unresolved_semantic_reference_name_at_depth(file, tree, 0)
}

fn unresolved_semantic_reference_name_at_depth(
    file: &TastyFile<'_>,
    tree: &RawTree<'_>,
    depth: usize,
) -> Option<String> {
    const MAX_DEPTH: usize = 128;
    if depth >= MAX_DEPTH {
        return None;
    }
    if let RawTree::LengthNode(node) = tree
        && matches!(node.tag, APPLIEDTPT_TAG | APPLIEDTYPE_TAG)
    {
        let StructuredNode::AppliedType(AppliedTypeNode { tycon, .. }) =
            node.decode_structured().ok()?
        else {
            return None;
        };
        return unresolved_semantic_reference_name_at_depth(file, &tycon, depth + 1);
    }
    match tree.decode_structured().ok()? {
        StructuredTree::Ident(IdentNode { type_tree, .. }) => {
            unresolved_semantic_reference_name_at_depth(file, &type_tree, depth + 1)
        }
        StructuredTree::Length(StructuredNode::SelectIn(selection)) => {
            render_tree_name(file, selection.name)
        }
        StructuredTree::Length(StructuredNode::Apply(ApplyNode { function, .. })) => {
            unresolved_semantic_reference_name_at_depth(file, &function, depth + 1)
        }
        StructuredTree::Reference(ReferenceNode {
            tag: TERMREF_TAG | TYPEREF_TAG,
            reference,
            ..
        }) => resolve_qualified_name(file, reference),
        _ => None,
    }
}

fn resolve_parent_name_with_alias(
    file: &TastyFile<'_>,
    parent: &RawTree<'_>,
    package: &str,
    resolve_alias: &mut impl FnMut(&BinaryName, &str) -> Option<BinaryName>,
    resolve_bare: &mut impl FnMut(&str, &str) -> Option<BinaryName>,
) -> Option<BinaryName> {
    if let Some((owner, alias)) = unresolved_alias_reference(file, parent) {
        if matches!(owner.simple_name(), "package" | "package$") {
            return resolve_alias(&owner, &alias);
        }
        if parent_reference_shape(file, parent) == "selected-qualified"
            && unresolved_alias_qualifier_is_package(file, parent, 0)
            && unresolved_alias_package_prefix(file, parent)
                .is_some_and(|package_owner| package_owner == owner.as_internal())
        {
            let package_object =
                BinaryName::from_internal(format!("{}/package$", owner.as_internal()));
            if let Some(resolved) = resolve_alias(&package_object, &alias) {
                return Some(resolved);
            }
        }
    }
    resolve_parent_name_with_bare(file, parent, package, resolve_bare)
}

fn unresolved_alias_qualifier_is_package(
    file: &TastyFile<'_>,
    tree: &RawTree<'_>,
    depth: usize,
) -> bool {
    const MAX_ALIAS_TREE_DEPTH: usize = 128;
    if depth >= MAX_ALIAS_TREE_DEPTH {
        return false;
    }
    if let RawTree::LengthNode(node) = tree {
        if matches!(node.tag, APPLIEDTPT_TAG | APPLIEDTYPE_TAG) {
            let Ok(StructuredNode::AppliedType(applied)) = node.decode_structured() else {
                return false;
            };
            return unresolved_alias_qualifier_is_package(file, &applied.tycon, depth + 1);
        }
        if node.tag == dotty_tasty::tasty::SELECTIN_TAG {
            let Ok(StructuredNode::SelectIn(selection)) = node.decode_structured() else {
                return false;
            };
            return parent_reference_shape(file, &selection.qualifier) == "package-qualified";
        }
    }
    match tree.decode_structured().ok() {
        Some(StructuredTree::Ident(ident)) => {
            unresolved_alias_qualifier_is_package(file, &ident.type_tree, depth + 1)
        }
        Some(StructuredTree::Length(StructuredNode::Apply(apply))) => {
            unresolved_alias_qualifier_is_package(file, &apply.function, depth + 1)
        }
        Some(StructuredTree::Reference(ReferenceNode { qualifier, .. }))
        | Some(StructuredTree::Select(SelectNode { qualifier, .. })) => {
            parent_reference_shape(file, &qualifier) == "package-qualified"
        }
        Some(StructuredTree::Length(StructuredNode::Typed(typed))) => {
            unresolved_alias_qualifier_is_package(file, &typed.type_tree, depth + 1)
                || unresolved_alias_qualifier_is_package(file, &typed.expression, depth + 1)
        }
        _ => false,
    }
}

/// Returns the selected qualifier recorded by the bounded parent profiler.
/// The caller uses it only for selected-qualified parents and still requires
/// the exact package object's TASTy to resolve the named alias.
fn unresolved_alias_package_prefix(file: &TastyFile<'_>, tree: &RawTree<'_>) -> Option<String> {
    parent_reference_details(file, tree, 0).and_then(|details| details.prefix)
}

/// Extracts the owner and simple name of a reference that could not be
/// resolved by the ordinary TASTy prefix reader. Applied type arguments and
/// the `Ident` wrapper used by extends clauses are transparent here.
fn unresolved_alias_reference(
    file: &TastyFile<'_>,
    tree: &RawTree<'_>,
) -> Option<(BinaryName, String)> {
    unresolved_alias_reference_at_depth(file, tree, 0)
}

fn declared_term_alias_reference(
    file: &TastyFile<'_>,
    tree: &RawTree<'_>,
    depth: usize,
) -> Option<(BinaryName, String)> {
    const MAX_ALIAS_TREE_DEPTH: usize = 128;
    if depth >= MAX_ALIAS_TREE_DEPTH {
        return None;
    }
    if let RawTree::Leaf(term) = tree
        && matches!(term.tag, SHAREDTERM_TAG | SHAREDTYPE_TAG)
        && let Some(reference) = term.ast_ref()
    {
        let shared = decode_shared_tree(file, reference.address)?;
        return declared_term_alias_reference(file, &shared, depth + 1);
    }
    match tree.decode_structured().ok()? {
        StructuredTree::Reference(ReferenceNode {
            tag: TERMREF_TAG,
            reference,
            qualifier,
        }) => {
            let name = resolve_qualified_name(file, reference)?;
            let owner = resolve_selection_path_at_depth(file, &qualifier, depth + 1)?;
            Some((owner, name))
        }
        _ => None,
    }
}

fn unresolved_alias_reference_at_depth(
    file: &TastyFile<'_>,
    tree: &RawTree<'_>,
    depth: usize,
) -> Option<(BinaryName, String)> {
    const MAX_ALIAS_TREE_DEPTH: usize = 128;
    if depth >= MAX_ALIAS_TREE_DEPTH {
        return None;
    }
    if let RawTree::LengthNode(node) = tree {
        if node.tag == dotty_tasty::tasty::SELECTIN_TAG {
            let StructuredNode::SelectIn(selection) = node.decode_structured().ok()? else {
                return None;
            };
            let alias = wire_name(file, selection.name)?;
            let owner = resolve_selection_path_at_depth(file, &selection.qualifier, depth + 1)?;
            return Some((owner, alias));
        }
        if matches!(node.tag, APPLIEDTPT_TAG | APPLIEDTYPE_TAG) {
            let StructuredNode::AppliedType(AppliedTypeNode { tycon, .. }) =
                node.decode_structured().ok()?
            else {
                return None;
            };
            return unresolved_alias_reference_at_depth(file, &tycon, depth + 1);
        }
    }
    let structured = tree.decode_structured().ok()?;
    match structured {
        StructuredTree::Ident(IdentNode { type_tree, .. }) => {
            unresolved_alias_reference_at_depth(file, &type_tree, depth + 1)
        }
        StructuredTree::Length(StructuredNode::Apply(ApplyNode { function, .. })) => {
            unresolved_alias_reference_at_depth(file, &function, depth + 1)
        }
        StructuredTree::Reference(ReferenceNode {
            tag: TERMREF_TAG | TYPEREF_TAG,
            reference,
            qualifier,
        }) => {
            let alias = resolve_qualified_name(file, reference)?;
            let owner = resolve_selection_path_at_depth(file, &qualifier, depth + 1)?;
            Some((owner, alias))
        }
        StructuredTree::Select(SelectNode {
            name, qualifier, ..
        }) => {
            let alias = wire_name(file, name)?;
            let owner = resolve_selection_path_at_depth(file, &qualifier, depth + 1)?;
            Some((owner, alias))
        }
        _ => None,
    }
}

fn resolve_selection_path_at_depth(
    file: &TastyFile<'_>,
    tree: &RawTree<'_>,
    depth: usize,
) -> Option<BinaryName> {
    const MAX_ALIAS_TREE_DEPTH: usize = 128;
    if depth >= MAX_ALIAS_TREE_DEPTH {
        return None;
    }
    match resolve_reference_name(file, tree) {
        ReferenceResolution::Resolved(name) => return Some(name),
        ReferenceResolution::Unresolved => {}
        ReferenceResolution::NotAReference => {}
    }
    if let RawTree::Leaf(term) = tree
        && matches!(term.tag, SHAREDTERM_TAG | SHAREDTYPE_TAG)
    {
        let reference = term.ast_ref()?;
        let shared = decode_shared_tree(file, reference.address)?;
        return resolve_selection_path_at_depth(file, &shared, depth + 1);
    }
    if let RawTree::Leaf(term) = tree
        && let Some(reference) = term.ast_ref()
    {
        if let Some(name) = resolve_symbol_address(file, reference.address) {
            return Some(BinaryName::from_internal(name));
        }
        let target = decode_shared_tree(file, reference.address)?;
        return resolve_selection_path_at_depth(file, &target, depth + 1);
    }
    if let Some(package) = resolve_reference_prefix(file, tree) {
        return Some(BinaryName::from_internal(package));
    }
    let StructuredTree::Select(SelectNode {
        name, qualifier, ..
    }) = tree.decode_structured().ok()?
    else {
        return None;
    };
    let prefix = resolve_selection_path_at_depth(file, &qualifier, depth + 1)?;
    let simple = wire_name(file, name)?;
    Some(BinaryName::from_internal(format!(
        "{}/{simple}",
        prefix.as_internal()
    )))
}

/// A generic mixin (e.g. `Iterable[Char]`) is encoded as an
/// `AppliedTpt`/`AppliedType` node — a category-5 payload
/// [`RawTree::name_refs`] deliberately treats as opaque (per its own
/// doc comment: "structured AST decoders can inspect those payloads
/// when needed"). This is exactly that: decode it and recurse on its
/// `tycon` (the applied type's own base reference, e.g. `Iterable`),
/// ignoring type arguments entirely — this decoder only ever needs a
/// name, never a semantic (possibly generic) type.
fn resolve_applied_type_name(
    file: &TastyFile<'_>,
    parent: &RawTree<'_>,
    package: &str,
) -> Option<BinaryName> {
    resolve_applied_type_name_in_package(file, parent, package, None)
}

fn resolve_applied_type_name_in_package(
    file: &TastyFile<'_>,
    parent: &RawTree<'_>,
    package: &str,
    package_scope: Option<&str>,
) -> Option<BinaryName> {
    let RawTree::LengthNode(node) = parent else {
        return None;
    };
    if node.tag != APPLIEDTPT_TAG && node.tag != APPLIEDTYPE_TAG {
        return None;
    }

    let StructuredNode::AppliedType(AppliedTypeNode { tycon, .. }) =
        node.decode_structured().ok()?
    else {
        return None;
    };

    resolve_parent_name_in_package(file, &tycon, package, package_scope)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TypeAliasTarget {
    Alias { owner: BinaryName, name: String },
    Candidate(BinaryName),
}

/// Reads a type-alias definition from a `.tasty` file and reduces its
/// right-hand side to either another package-object alias or a candidate
/// name. The classpath-aware caller follows aliases and validates the final
/// candidate before returning it as a resolved parent.
pub(crate) fn resolve_type_alias_target(
    file: &TastyFile<'_>,
    alias_name: &str,
    current_package: &str,
) -> Result<Option<TypeAliasTarget>, TastyDecodeError> {
    let index = file.ast_address_index()?;
    for node in index.iter() {
        if node.tag != TYPEDEF_TAG {
            continue;
        }
        let StructuredNode::TypeDef(DefinitionBody::TypeDef {
            name,
            type_or_template,
            ..
        }) = node.decode_structured()?
        else {
            continue;
        };
        if file.render_name(name).ok().as_deref() != Some(alias_name) {
            continue;
        }
        let rhs = match type_or_template {
            RawTree::LengthNode(lambda) if lambda.tag == dotty_tasty::tasty::LAMBDATPT_TAG => {
                lambda.decode_lambda_tpt()?.body
            }
            RawTree::LengthNode(template) if template.tag == TEMPLATE_TAG => continue,
            rhs => rhs,
        };
        if let Some((owner, name)) = unresolved_alias_reference(file, &rhs)
            && matches!(owner.simple_name(), "package" | "package$")
        {
            return Ok(Some(TypeAliasTarget::Alias { owner, name }));
        }
        if let Some((owner, name)) = unresolved_alias_reference(file, &rhs) {
            let package_path = owner.package_path();
            let owner_path = if !current_package.is_empty()
                && package_path.starts_with(&format!("{current_package}/"))
            {
                owner
                    .as_internal()
                    .strip_prefix(&format!("{current_package}/"))
                    .unwrap_or(owner.as_internal())
                    .to_owned()
            } else {
                owner.as_internal().to_owned()
            };
            let candidate = BinaryName::from_internal(format!("{owner_path}/{name}"));
            return Ok(Some(TypeAliasTarget::Candidate(
                package_relative_candidate(candidate, current_package),
            )));
        }
        let candidate =
            resolve_parent_name_in_package(file, &rhs, current_package, Some(current_package))
                .map(|candidate| package_relative_candidate(candidate, current_package));
        return Ok(candidate.map(TypeAliasTarget::Candidate));
    }
    Ok(None)
}

fn direct_term_alias_reference(
    file: &TastyFile<'_>,
    tree: &RawTree<'_>,
    depth: usize,
) -> Option<(BinaryName, String)> {
    const MAX_ALIAS_TREE_DEPTH: usize = 128;
    if depth >= MAX_ALIAS_TREE_DEPTH {
        return None;
    }
    if let RawTree::LengthNode(node) = tree
        && node.tag == SELECTIN_TAG
    {
        let StructuredNode::SelectIn(selection) = node.decode_structured().ok()? else {
            return None;
        };
        let name = wire_name(file, selection.name)?;
        let owner = resolve_selection_path_at_depth(file, &selection.qualifier, depth + 1)?;
        return Some((owner, name));
    }
    match tree.decode_structured().ok()? {
        StructuredTree::Ident(IdentNode { type_tree, .. }) => {
            direct_term_alias_reference(file, &type_tree, depth + 1)
        }
        StructuredTree::Length(StructuredNode::Typed(typed)) => {
            direct_term_alias_reference(file, &typed.expression, depth + 1)
        }
        StructuredTree::Reference(ReferenceNode {
            tag: TERMREF_TAG,
            reference,
            qualifier,
        }) => {
            let name = resolve_qualified_name(file, reference)?;
            let owner = resolve_selection_path_at_depth(file, &qualifier, depth + 1)?;
            Some((owner, name))
        }
        StructuredTree::Select(SelectNode {
            name, qualifier, ..
        }) => {
            let name = wire_name(file, name)?;
            let owner = resolve_selection_path_at_depth(file, &qualifier, depth + 1)?;
            Some((owner, name))
        }
        _ => None,
    }
}

/// Reads a package-object value alias and reduces its right-hand side to a
/// classpath candidate. The caller validates that the candidate is a real
/// module identity before exposing it as a term.
pub(crate) fn resolve_term_alias_target(
    file: &TastyFile<'_>,
    alias_name: &str,
    current_package: &str,
) -> Result<Option<TypeAliasTarget>, TastyDecodeError> {
    let index = file.ast_address_index()?;
    for node in index.iter() {
        if node.tag != VALDEF_TAG {
            continue;
        }
        if !is_public_package_object_value(file, &index, node, current_package)? {
            continue;
        }
        let StructuredNode::ValDef(DefinitionBody::ValDef {
            name,
            type_tree,
            rhs: Some(rhs),
            tail,
        }) = node.decode_structured()?
        else {
            continue;
        };
        if file.render_name(name).ok().as_deref() != Some(alias_name) {
            continue;
        }
        let Some((owner, name)) = direct_term_alias_reference(file, &rhs, 0) else {
            return Ok(None);
        };
        if has_modifier(&tail, MUTABLE_TAG) {
            return Ok(None);
        }
        let Some((declared_owner, declared_name)) =
            declared_term_alias_reference(file, &type_tree, 0)
        else {
            return Ok(None);
        };
        if declared_owner != owner || declared_name != name {
            return Ok(None);
        }
        let package_path = owner.package_path();
        let owner_path = if !current_package.is_empty()
            && package_path.starts_with(&format!("{current_package}/"))
        {
            owner
                .as_internal()
                .strip_prefix(&format!("{current_package}/"))
                .unwrap_or(owner.as_internal())
                .to_owned()
        } else {
            owner.as_internal().to_owned()
        };
        let candidate = BinaryName::from_internal(format!("{owner_path}/{name}"));
        return Ok(Some(TypeAliasTarget::Candidate(
            package_relative_candidate(candidate, current_package),
        )));
    }
    Ok(None)
}

fn is_public_package_object_value(
    file: &TastyFile<'_>,
    index: &dotty_tasty::tasty::AstAddressIndex<'_>,
    value: &dotty_tasty::tasty::RawNode<'_>,
    current_package: &str,
) -> Result<bool, TastyDecodeError> {
    let Ok(value_address) = u32::try_from(value.offset) else {
        return Ok(false);
    };
    let Some(template) = index.parent_of(value_address) else {
        return Ok(false);
    };
    if template.tag != TEMPLATE_TAG {
        return Ok(false);
    }
    let Some(template_owner) = index.parent_of(template.offset as u32) else {
        return Ok(false);
    };
    if template_owner.tag != TYPEDEF_TAG {
        return Ok(false);
    }
    if !is_package_object_in_requested_package(file, index, template_owner.offset, current_package)
    {
        return Ok(false);
    }
    let Some(owner) = index.get(template_owner.offset as u32) else {
        return Ok(false);
    };
    let StructuredNode::TypeDef(DefinitionBody::TypeDef { name, tail, .. }) =
        owner.decode_structured()?
    else {
        return Ok(false);
    };
    if !matches!(
        file.render_name(name).ok().as_deref(),
        Some("package" | "package$")
    ) || !has_modifier(&tail, OBJECT_TAG)
    {
        return Ok(false);
    }

    let StructuredNode::ValDef(DefinitionBody::ValDef { tail, .. }) = value.decode_structured()?
    else {
        return Ok(false);
    };
    Ok(is_public_package_object_value_tail(&tail))
}

fn is_public_package_object_value_tail(tail: &[DefinitionTail<'_>]) -> bool {
    let inaccessible = [PRIVATE_TAG, PROTECTED_TAG, LOCAL_TAG, MUTABLE_TAG];
    !tail.iter().any(|item| match item {
        DefinitionTail::Modifier(tag) => inaccessible.contains(tag),
        DefinitionTail::QualifiedModifier(modifier) => {
            matches!(modifier.tag, PRIVATEQUALIFIED_TAG | PROTECTEDQUALIFIED_TAG)
        }
        DefinitionTail::Annotation(_) => false,
    })
}

fn is_package_object_in_requested_package(
    file: &TastyFile<'_>,
    index: &dotty_tasty::tasty::AstAddressIndex<'_>,
    address: usize,
    requested_package: &str,
) -> bool {
    let Ok(address) = u32::try_from(address) else {
        return false;
    };
    let Some(parent) = index.parent_of(address) else {
        return false;
    };
    if parent.tag != PACKAGE_TAG {
        return false;
    }
    let Some(package_node) = index.get(parent.offset as u32) else {
        return false;
    };
    let Ok(package) = package_node.decode_package() else {
        return false;
    };
    resolve_reference_prefix(file, &package.path).as_deref() == Some(requested_package)
}

fn package_relative_candidate(candidate: BinaryName, current_package: &str) -> BinaryName {
    let prefix = format!("{current_package}/");
    candidate
        .as_internal()
        .strip_prefix(&prefix)
        .filter(|_| !current_package.is_empty())
        .map_or(candidate.clone(), BinaryName::from_internal)
}

/// Compatibility helper for unit coverage that only needs a direct target.
#[cfg(test)]
fn resolve_type_alias_candidate(
    file: &TastyFile<'_>,
    alias_name: &str,
    current_package: &str,
) -> Option<BinaryName> {
    match resolve_type_alias_target(file, alias_name, current_package).ok()?? {
        TypeAliasTarget::Candidate(candidate) => Some(candidate),
        TypeAliasTarget::Alias { .. } => None,
    }
}

/// Reads a raw AST name-table reference: AST fields such as
/// [`RawTree::name_refs`] and a definition's own `name()` are
/// [`dotty_tasty::tasty::NameRef`]s, zero-based indexes into the name table.
/// Returns `None` for anything other than a direct UTF-8 entry (a signature-
/// or symbol-shaped name-table entry, which this decoder does not need to
/// render).
fn wire_name(file: &TastyFile<'_>, reference: u32) -> Option<String> {
    file.names().get_utf8(reference).map(str::to_owned)
}

fn module_class_wire_name(file: &TastyFile<'_>, reference: u32) -> Option<String> {
    match file.names().get(reference)? {
        dotty_tasty::tasty::RawName::ObjectClass { underlying } => {
            Some(format!("{}$", wire_name(file, *underlying)?))
        }
        dotty_tasty::tasty::RawName::Utf8(name) => Some(name.clone()),
        _ => None,
    }
}

fn render_tree_name(file: &TastyFile<'_>, reference: u32) -> Option<String> {
    resolve_qualified_name(file, reference)
}

/// Render the selected member's source-level name, including the original
/// spelling of a signed name. Its signature is not part of the binary class
/// name; the selection's owner tree establishes where the member lives.
fn render_member_name(file: &TastyFile<'_>, reference: u32) -> Option<String> {
    match file.render_signed_name(reference).ok()? {
        Some(RenderedSignedName::Signed { original, .. })
        | Some(RenderedSignedName::TargetSigned { original, .. }) => Some(original),
        None => resolve_qualified_name(file, reference).or_else(|| wire_name(file, reference)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dotty_tasty::tasty::{AstChildNode, SimpleTerm, Writer};

    fn tsv_cell(value: &str) -> String {
        if value.is_empty() {
            return "-".to_owned();
        }
        value
            .replace('\\', "\\\\")
            .replace('\t', "\\t")
            .replace('\r', "\\r")
            .replace('\n', "\\n")
    }

    /// Opt-in profile of the frozen #1342 roots. This reads only the pinned
    /// TASTy resources named by the root TSV and loads those roots and their
    /// dependencies; it never runs source corpus typing and does not change
    /// the semantic report used by CI.
    #[test]
    #[ignore = "run with pinned Scala 3.9 JAR paths and SCALA39_PARENT_PROFILE_OUTPUT"]
    fn profile_frozen_unresolved_supertype_parent_roots() {
        use crate::class_path::{ClassFormat, ClassPathEntry, CompositeClassPath};
        use crate::jar_class_path::JarClassPath;
        use crate::jdk_class_path::JdkClassPath;
        use std::path::PathBuf;

        let compiler_jar =
            PathBuf::from(std::env::var("SCALA39_COMPILER_JAR").expect("set SCALA39_COMPILER_JAR"));
        let library_jar =
            PathBuf::from(std::env::var("SCALA39_LIBRARY_JAR").expect("set SCALA39_LIBRARY_JAR"));
        let scala3_library_jar =
            PathBuf::from(std::env::var("SCALA39_LIBRARY3_JAR").expect("set SCALA39_LIBRARY3_JAR"));
        let jdk_jmods =
            PathBuf::from(std::env::var("JAVA_HOME").expect("set JAVA_HOME to Temurin 21.0.8"))
                .join("jmods");
        let output_path = PathBuf::from(
            std::env::var("SCALA39_PARENT_PROFILE_OUTPUT")
                .expect("set SCALA39_PARENT_PROFILE_OUTPUT"),
        );
        let class_path = CompositeClassPath::new(vec![
            Box::new(JarClassPath::new(&compiler_jar, 21).expect("open compiler JAR")),
            Box::new(JarClassPath::new(&library_jar, 21).expect("open Scala library JAR")),
            Box::new(JarClassPath::new(&scala3_library_jar, 21).expect("open Scala 3 library JAR")),
            Box::new(JdkClassPath::new(jdk_jmods.clone()).expect("open JDK 21 jmods")),
        ]);
        let load_class_path = CompositeClassPath::new(vec![
            Box::new(JarClassPath::new(&compiler_jar, 21).expect("open compiler JAR for loading")),
            Box::new(
                JarClassPath::new(&library_jar, 21).expect("open Scala library JAR for loading"),
            ),
            Box::new(
                JarClassPath::new(&scala3_library_jar, 21)
                    .expect("open Scala 3 library JAR for loading"),
            ),
            Box::new(JdkClassPath::new(jdk_jmods).expect("open JDK 21 jmods for loading")),
        ]);
        let mut semantic_store = dotty_core::SemanticStore::new();
        let mut class_loader =
            crate::loader::ClassLoader::new(load_class_path, &mut semantic_store);
        let roots = include_str!(
            "../../../docs/typer-classpath-corpus-audit-post-compiler-modules-3.9.0-tasty-supertype-roots.tsv"
        );
        let mut rows = String::from(
            "requested_class_root\trequested_identity\tweighted_observations\tresource_path\tartifact\tartifact_version\tartifact_sha256\ttemplate_wire_address\tparent_index\tshape\tresolution_stage\tresolved_binary_name\treference_tag\treference_name\treference_prefix\talias_owner\talias_name\thas_semantic_reference\thas_reference_prefix\tvisible_names\traw_tree\tprofile_first_unresolved_parent_index\tstandalone_decode_parent_index\tstandalone_decode_cause\tdeepest_typed_cause\tfailing_binary_name\tfailure_parent_index\tfailure_parent_template_wire_address\tfailure_parent_shape\tfailure_reference_tag\tfailure_reference_name\tfailure_reference_prefix\tdependency_chain\tdeepest_resource_origin\tsource_audit_observations\tsource_audit_files\n",
        );
        let mut root_count = 0usize;
        let mut total_weight = 0u64;
        let mut compiler_count = 0usize;
        let mut compiler_weight = 0u64;
        let mut library_count = 0usize;
        let mut library_weight = 0u64;
        let mut load_causes = std::collections::BTreeMap::new();
        let mut source_audit_metrics =
            std::collections::BTreeMap::<String, (u64, std::collections::BTreeSet<String>)>::new();
        for line in include_str!(
            "../../../docs/classloader-unresolved-tasty-parent-source-provenance-3.9.0.tsv"
        )
        .lines()
        .skip(1)
        .filter(|line| !line.trim().is_empty())
        {
            let fields = line.split('\t').collect::<Vec<_>>();
            assert_eq!(fields.len(), 3, "invalid source-provenance row: {line}");
            let (_, requested_and_root) = fields[0]
                .split_once("::requested=")
                .expect("source-provenance key includes requested identity");
            let (requested_root, _) = requested_and_root
                .split_once("::root=")
                .expect("source-provenance key includes deepest root");
            let observations = fields[2]
                .parse::<u64>()
                .expect("source-provenance observations are numeric");
            let (total, files) = source_audit_metrics
                .entry(requested_root.to_owned())
                .or_default();
            *total += observations;
            files.insert(fields[1].to_owned());
        }
        let frozen_requested_roots = roots
            .lines()
            .skip(1)
            .filter(|line| !line.trim().is_empty())
            .map(|line| line.split('\t').next().expect("root row has identity"))
            .collect::<std::collections::BTreeSet<_>>();
        let frozen_root_weights = roots
            .lines()
            .skip(1)
            .filter(|line| !line.trim().is_empty())
            .map(|line| {
                let fields = line.split('\t').collect::<Vec<_>>();
                assert_eq!(fields.len(), 6, "invalid frozen-root row: {line}");
                (
                    fields[0].to_owned(),
                    fields[1].parse::<u64>().expect("weight is numeric"),
                )
            })
            .collect::<std::collections::BTreeMap<_, _>>();
        let observed_requested_roots = source_audit_metrics
            .keys()
            .map(String::as_str)
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(frozen_requested_roots.len(), 50);
        assert_eq!(observed_requested_roots, frozen_requested_roots);
        assert_eq!(
            source_audit_metrics
                .values()
                .map(|(observations, _)| observations)
                .sum::<u64>(),
            8_948
        );
        assert_eq!(
            source_audit_metrics
                .values()
                .flat_map(|(_, files)| files.iter())
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            279
        );

        let mut joined_source_observations = 0u64;
        for line in roots.lines().skip(1).filter(|line| !line.trim().is_empty()) {
            let fields = line.split('\t').collect::<Vec<_>>();
            assert_eq!(fields.len(), 6, "invalid frozen-root row: {line}");
            let requested_root = fields[0];
            let weighted_observations = fields[1].parse::<u64>().expect("weight is numeric");
            let resource_path = fields[2];
            let artifact = fields[3];
            let resource_name = resource_path
                .strip_suffix(".tasty")
                .expect("frozen resource path is a TASTy entry");
            let resource = class_path
                .find_class(&BinaryName::from_internal(resource_name))
                .expect("read frozen TASTy resource")
                .expect("frozen resource exists on pinned classpath");
            assert_eq!(resource.format(), ClassFormat::Tasty, "{requested_root}");
            let selected_origin = match resource.origin() {
                crate::class_path::ClassOrigin::Jar(path) => path,
                origin => panic!("expected pinned JAR origin for {requested_root}, got {origin:?}"),
            };
            let expected_jar = if artifact.starts_with("scala3-compiler_") {
                &compiler_jar
            } else {
                &library_jar
            };
            assert_eq!(
                selected_origin, expected_jar,
                "resource origin for {requested_root}"
            );

            let requested = BinaryName::from_internal(requested_root);
            let profiles = profile_parent_references(
                resource.bytes(),
                &requested,
                requested_root.ends_with('$'),
                |owner, alias| {
                    crate::loader::resolve_tasty_alias(&class_path, owner, alias)
                        .ok()
                        .flatten()
                },
                |package, simple_name| {
                    crate::loader::resolve_tasty_bare_reference(&class_path, package, simple_name)
                        .ok()
                        .flatten()
                },
            )
            .unwrap_or_else(|error| panic!("profile {requested_root}: {error}"));
            assert!(!profiles.is_empty(), "{requested_root} has no parents");
            let first_unresolved_parent = profiles
                .iter()
                .find(|profile| profile.resolved_binary_name.is_none())
                .map(|profile| profile.reference.parent_index);
            let decode_result = if requested_root.ends_with('$') {
                decode_module_class_with_resolvers(
                    resource.bytes(),
                    &requested,
                    |owner, alias| {
                        crate::loader::resolve_tasty_alias(&class_path, owner, alias)
                            .ok()
                            .flatten()
                    },
                    |package, simple_name| {
                        crate::loader::resolve_tasty_bare_reference(
                            &class_path,
                            package,
                            simple_name,
                        )
                        .ok()
                        .flatten()
                    },
                )
            } else {
                decode_with_resolvers(
                    resource.bytes(),
                    &requested,
                    |owner, alias| {
                        crate::loader::resolve_tasty_alias(&class_path, owner, alias)
                            .ok()
                            .flatten()
                    },
                    |package, simple_name| {
                        crate::loader::resolve_tasty_bare_reference(
                            &class_path,
                            package,
                            simple_name,
                        )
                        .ok()
                        .flatten()
                    },
                )
            };
            let (standalone_parent_index, standalone_decode_cause) = match decode_result {
                Err(error) => {
                    let parent_index = match &error {
                        TastyDecodeError::UnresolvedSupertype(parent) => Some(parent.parent_index),
                        _ => None,
                    };
                    assert_eq!(
                        parent_index, first_unresolved_parent,
                        "independent per-parent profile must identify the decoder's first failure for {requested_root}"
                    );
                    (parent_index, error.root_cause_code())
                }
                Ok(_) => (None, "none"),
            };
            let (load_cause, failing_binary_name, failure_parent, dependency_chain, failure_origin) =
                match class_loader.load_class_detailed(&requested) {
                    Ok(_) => ("none", String::new(), None, String::new(), String::new()),
                    Err(failure) => {
                        let mut chain = Vec::new();
                        let mut deepest = &failure.error;
                        while let crate::error::ClassLoadError::DependencyFailure {
                            owner,
                            dependency,
                            source,
                        } = deepest
                        {
                            chain.push(format!(
                                "{}=>{}",
                                owner.as_internal(),
                                dependency.as_internal()
                            ));
                            deepest = source.as_ref();
                        }
                        let (failing_name, parent) = match deepest {
                            crate::error::ClassLoadError::InvalidTastyFile(
                                name,
                                TastyDecodeError::UnresolvedSupertype(parent),
                            ) => (name.as_internal().to_owned(), Some(parent.as_ref().clone())),
                            _ => match deepest {
                                crate::error::ClassLoadError::NotFound(name)
                                | crate::error::ClassLoadError::Io(name, _)
                                | crate::error::ClassLoadError::InvalidClassFile(name, _)
                                | crate::error::ClassLoadError::UnsupportedClassVersion(name, _)
                                | crate::error::ClassLoadError::MalformedReference(name, _)
                                | crate::error::ClassLoadError::MalformedDescriptor(name, _)
                                | crate::error::ClassLoadError::MalformedSignature(name, _)
                                | crate::error::ClassLoadError::InvalidTastyFile(name, _)
                                | crate::error::ClassLoadError::CircularInheritance(name)
                                | crate::error::ClassLoadError::InheritanceDepthExceeded(name) => {
                                    (name.as_internal().to_owned(), None)
                                }
                                crate::error::ClassLoadError::NameMismatch {
                                    requested, ..
                                } => (requested.as_internal().to_owned(), None),
                                crate::error::ClassLoadError::IdentityConflict { name, .. } => {
                                    (name.as_internal().to_owned(), None)
                                }
                                crate::error::ClassLoadError::UnresolvedTypeVariable(name, _) => {
                                    (name.as_internal().to_owned(), None)
                                }
                                crate::error::ClassLoadError::DependencyFailure { .. } => {
                                    unreachable!("dependency wrapper was unwrapped above")
                                }
                            },
                        };
                        let origin = match failure.origin.as_ref() {
                            Some(crate::class_path::ClassOrigin::Jar(path)) => format!(
                                "jar:{}",
                                path.file_name().unwrap_or_default().to_string_lossy()
                            ),
                            Some(crate::class_path::ClassOrigin::Directory(path)) => format!(
                                "directory:{}",
                                path.file_name().unwrap_or_default().to_string_lossy()
                            ),
                            Some(crate::class_path::ClassOrigin::Jmod(path)) => format!(
                                "jmod:{}",
                                path.file_name().unwrap_or_default().to_string_lossy()
                            ),
                            None => String::new(),
                        };
                        (
                            failure.root_cause_code(),
                            failing_name,
                            parent,
                            chain.join("|"),
                            origin,
                        )
                    }
                };
            load_causes.insert(requested_root.to_owned(), load_cause);

            for profile in profiles {
                let reference = profile.reference;
                let cells = [
                    requested_root.to_owned(),
                    if requested_root.ends_with('$') {
                        "ModuleClass".to_owned()
                    } else {
                        "Class".to_owned()
                    },
                    weighted_observations.to_string(),
                    resource_path.to_owned(),
                    artifact.to_owned(),
                    fields[4].to_owned(),
                    fields[5].to_owned(),
                    reference.template_wire_address.to_string(),
                    reference.parent_index.to_string(),
                    reference.shape,
                    profile.resolution_stage.to_owned(),
                    profile.resolved_binary_name.unwrap_or_default(),
                    reference.reference_tag.unwrap_or_default(),
                    reference.reference_name.unwrap_or_default(),
                    reference.reference_prefix.unwrap_or_default(),
                    reference.alias_owner.unwrap_or_default(),
                    reference.alias_name.unwrap_or_default(),
                    reference.has_semantic_reference.to_string(),
                    reference.has_reference_prefix.to_string(),
                    reference.visible_names.join("|"),
                    reference.raw_tree,
                    first_unresolved_parent
                        .map(|index| index.to_string())
                        .unwrap_or_default(),
                    standalone_parent_index
                        .map(|index| index.to_string())
                        .unwrap_or_default(),
                    standalone_decode_cause.to_owned(),
                    load_cause.to_owned(),
                    failing_binary_name.clone(),
                    failure_parent
                        .as_ref()
                        .map(|parent| parent.parent_index.to_string())
                        .unwrap_or_default(),
                    failure_parent
                        .as_ref()
                        .map(|parent| parent.template_wire_address.to_string())
                        .unwrap_or_default(),
                    failure_parent
                        .as_ref()
                        .map(|parent| parent.shape.clone())
                        .unwrap_or_default(),
                    failure_parent
                        .as_ref()
                        .and_then(|parent| parent.reference_tag.clone())
                        .unwrap_or_default(),
                    failure_parent
                        .as_ref()
                        .and_then(|parent| parent.reference_name.clone())
                        .unwrap_or_default(),
                    failure_parent
                        .as_ref()
                        .and_then(|parent| parent.reference_prefix.clone())
                        .unwrap_or_default(),
                    dependency_chain.clone(),
                    failure_origin.clone(),
                    source_audit_metrics
                        .get(requested_root)
                        .map(|(observations, _)| observations.to_string())
                        .unwrap_or_else(|| "0".to_owned()),
                    source_audit_metrics
                        .get(requested_root)
                        .map(|(_, files)| files.len().to_string())
                        .unwrap_or_else(|| "0".to_owned()),
                ];
                for (index, cell) in cells.iter().enumerate() {
                    if index > 0 {
                        rows.push('\t');
                    }
                    rows.push_str(&tsv_cell(cell));
                }
                rows.push('\n');
            }
            joined_source_observations += source_audit_metrics
                .get(requested_root)
                .expect("every frozen root has source-audit observations")
                .0;
            root_count += 1;
            total_weight += weighted_observations;
            if artifact.starts_with("scala3-compiler_") {
                compiler_count += 1;
                compiler_weight += weighted_observations;
            } else {
                library_count += 1;
                library_weight += weighted_observations;
            }
        }

        assert_eq!((root_count, total_weight), (50, 8_949));
        assert_eq!(joined_source_observations, 8_948);
        assert_eq!((compiler_count, compiler_weight), (18, 8_590));
        assert_eq!((library_count, library_weight), (32, 359));
        let load_cause_counts = load_causes.values().fold(
            std::collections::BTreeMap::<&str, usize>::new(),
            |mut counts, cause| {
                *counts.entry(cause).or_default() += 1;
                counts
            },
        );
        let historical_profile =
            include_str!("../../../docs/classloader-unresolved-tasty-parent-profile-3.9.0.tsv");
        let historical_headers = historical_profile
            .lines()
            .next()
            .expect("historical parent profile has headers")
            .split('\t')
            .collect::<Vec<_>>();
        let historical_root_index = historical_headers
            .iter()
            .position(|header| *header == "requested_class_root")
            .expect("historical profile has root column");
        let historical_cause_index = historical_headers
            .iter()
            .position(|header| *header == "deepest_typed_cause")
            .expect("historical profile has cause column");
        let historical_unresolved_roots = historical_profile
            .lines()
            .skip(1)
            .filter(|line| !line.trim().is_empty())
            .filter_map(|line| {
                let fields = line.split('\t').collect::<Vec<_>>();
                (fields.get(historical_cause_index) == Some(&"tasty.unresolved_supertype"))
                    .then(|| fields[historical_root_index].to_owned())
            })
            .collect::<std::collections::BTreeSet<_>>();
        let current_unresolved_roots = load_causes
            .iter()
            .filter_map(|(root, cause)| {
                (*cause == "tasty.unresolved_supertype").then_some(root.clone())
            })
            .collect::<std::collections::BTreeSet<_>>();
        let recovered_roots = historical_unresolved_roots
            .difference(&current_unresolved_roots)
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        let newly_unresolved_roots = current_unresolved_roots
            .difference(&historical_unresolved_roots)
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        let weighted_roots = |roots: &std::collections::BTreeSet<String>| {
            roots
                .iter()
                .map(|root| {
                    *frozen_root_weights
                        .get(root)
                        .expect("root has frozen weight")
                })
                .sum::<u64>()
        };
        let historical_weight = weighted_roots(&historical_unresolved_roots);
        let current_weight = weighted_roots(&current_unresolved_roots);
        let recovered_weight = weighted_roots(&recovered_roots);
        let newly_unresolved_weight = weighted_roots(&newly_unresolved_roots);
        assert_eq!(historical_unresolved_roots.len(), 41);
        assert_eq!(current_unresolved_roots.len(), 7);
        assert_eq!(
            historical_unresolved_roots.len() - current_unresolved_roots.len(),
            34
        );
        assert_eq!(historical_weight, 379);
        assert_eq!(current_weight, 17);
        assert_eq!(recovered_roots.len(), 34);
        assert_eq!(recovered_weight, 362);
        assert!(newly_unresolved_roots.is_empty());
        assert_eq!(newly_unresolved_weight, 0);
        assert_eq!(
            historical_weight + newly_unresolved_weight,
            current_weight + recovered_weight
        );
        assert_eq!(
            load_cause_counts.get("tasty.unresolved_supertype"),
            Some(&current_unresolved_roots.len())
        );
        std::fs::write(&output_path, rows).expect("write parent profile TSV");
        eprintln!(
            "parent outcomes: {} to {} distinct unresolved roots; {} recovered roots / {recovered_weight} recovered weighted observations, {} newly unresolved roots / {newly_unresolved_weight} newly unresolved weighted observations ({} -> {} unresolved weighted observations)",
            historical_unresolved_roots.len(),
            current_unresolved_roots.len(),
            recovered_roots.len(),
            newly_unresolved_roots.len(),
            historical_weight,
            current_weight,
        );
        eprintln!(
            "wrote {root_count} roots ({total_weight} weighted observations) to {}",
            output_path.display()
        );
    }

    fn fixture_bytes(relative_path: &str) -> Vec<u8> {
        std::fs::read(
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../dotty-tasty/tests/fixtures")
                .join(relative_path),
        )
        .unwrap_or_else(|error| panic!("fixture {relative_path} should exist: {error}"))
    }

    fn unpickler_fixture_bytes(relative_path: &str) -> Vec<u8> {
        std::fs::read(
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../dotty-tasty-unpickler/tests/fixtures/semantic")
                .join(relative_path),
        )
        .unwrap_or_else(|error| panic!("fixture {relative_path} should exist: {error}"))
    }

    fn with_header_version(mut bytes: Vec<u8>, major: u8, minor: u8, experimental: u8) -> Vec<u8> {
        // The pinned test versions all fit in one TASTy base-128 group; the
        // high bit marks the final group.
        bytes[4] = major | 0x80;
        bytes[5] = minor | 0x80;
        bytes[6] = experimental | 0x80;
        bytes
    }

    #[test]
    fn classloader_accepts_only_validated_scala_3_9_tasty_formats() {
        let compiler_28_8 = fixture_bytes("scala3-compiler/dotty/tools/dotc/core/Types.tasty");
        let file = parse_classloader_tasty(&compiler_28_8).unwrap();
        assert_eq!(
            (
                file.header().major_version,
                file.header().minor_version,
                file.header().experimental_version
            ),
            (28, 8, 0)
        );
        assert!(matches!(
            TastyFile::parse_scala_3_9(&compiler_28_8),
            Err(TastyFileError::Header(
                dotty_tasty::header::HeaderError::UnsupportedVersion { .. }
            ))
        ));

        let library_28_9 = fixture_bytes("scala3-library/scala/io/Source.tasty");
        let file = parse_classloader_tasty(&library_28_9).unwrap();
        assert_eq!(
            (
                file.header().major_version,
                file.header().minor_version,
                file.header().experimental_version
            ),
            (28, 9, 0)
        );

        for (major, minor, experimental) in
            [(28u32, 7u32, 0u32), (28, 10, 0), (27, 9, 0), (28, 8, 1)]
        {
            let bytes = with_header_version(
                compiler_28_8.clone(),
                major as u8,
                minor as u8,
                experimental as u8,
            );
            let error = parse_classloader_tasty(&bytes).unwrap_err();
            assert!(matches!(
                &error,
                TastyDecodeError::UnsupportedClassloaderVersion {
                    major: found_major,
                    minor: found_minor,
                    experimental: found_experimental,
                } if (*found_major, *found_minor, *found_experimental) == (major, minor, experimental)
            ));
            assert_eq!(error.root_cause_code(), "tasty.unsupported_version");
            assert_eq!(
                error.category(),
                ClassLoadCategory::UnsupportedFeatureOrVersion
            );
        }

        assert!(matches!(
            parse_classloader_tasty(&compiler_28_8[..12]),
            Err(TastyDecodeError::Parse(_))
        ));
    }

    #[test]
    fn real_scala_3_9_compiler_module_identity_is_decoded_from_28_8_tasty() {
        let types = fixture_bytes("scala3-compiler/dotty/tools/dotc/core/Types.tasty");
        let requested = BinaryName::from_internal("dotty/tools/dotc/core/Types");

        decode_module_identity(&types, &requested)
            .unwrap()
            .expect("the pinned Types TASTy declares an Object and ModuleClass");

        let universal_type =
            fixture_bytes("scala3-compiler/dotty/tools/dotc/semanticdb/UniversalType.tasty");
        decode_identity(
            &universal_type,
            &BinaryName::from_internal("dotty/tools/dotc/semanticdb/UniversalType"),
        )
        .expect("the pinned compiler class identity is readable from 28.8 TASTy");

        let decoded = decode_module_class_with_resolvers(
            &types,
            &BinaryName::from_internal("dotty/tools/dotc/core/Types$"),
            |_, _| None,
            |_, _| None,
        )
        .expect("the authoritative TypeUtils parent should decode from compiler TASTy");
        assert_eq!(
            decoded.super_class.as_internal(),
            "dotty/tools/dotc/core/TypeUtils"
        );

        let contexts = fixture_bytes("scala3-compiler/dotty/tools/dotc/core/Contexts.tasty");
        decode_module_identity(
            &contexts,
            &BinaryName::from_internal("dotty/tools/dotc/core/Contexts"),
        )
        .unwrap()
        .expect("the pinned Contexts TASTy declares an Object");

        assert!(matches!(
            decode_identity(
                &types,
                &BinaryName::from_internal("dotty/tools/dotc/core/Absent")
            ),
            Err(TastyDecodeError::MissingTypeDef)
        ));
        assert!(
            decode_module_identity(
                &types,
                &BinaryName::from_internal("dotty/tools/dotc/core/Absent")
            )
            .unwrap()
            .is_none()
        );
    }

    #[test]
    fn decodes_a_proven_module_pair_and_its_real_companion_class() {
        let bytes = unpickler_fixture_bytes("Both.tasty");
        let module_class = decode_module_class_with_resolvers(
            &bytes,
            &BinaryName::from_internal("me/cytrowski/tastyfixtures/semantic/Both$"),
            |_, _| None,
            |package, simple_name| {
                Some(BinaryName::from_internal(format!(
                    "{package}/{simple_name}"
                )))
            },
        )
        .unwrap();
        assert!(
            module_class
                .methods
                .iter()
                .any(|method| method.name == "make")
        );

        let identity = decode_module_identity(
            &bytes,
            &BinaryName::from_internal("me/cytrowski/tastyfixtures/semantic/Both"),
        )
        .unwrap()
        .expect("Both.tasty contains an object");

        assert_eq!(identity.companion_class_kind, Some(SymbolKind::Class));
    }

    #[test]
    fn decodes_overloaded_methods_from_the_module_class_template() {
        let bytes = fixture_bytes("scala3-library/scala/math/BigInt.tasty");
        let decoded = decode_module_class_with_resolvers(
            &bytes,
            &BinaryName::from_internal("scala/math/BigInt$"),
            |_, _| None,
            |package, simple_name| {
                Some(BinaryName::from_internal(format!(
                    "{package}/{simple_name}"
                )))
            },
        )
        .unwrap();

        let apply_count = decoded
            .methods
            .iter()
            .filter(|method| method.name == "apply")
            .count();
        assert!(
            apply_count > 1,
            "expected overloaded apply methods, found {:?}",
            decoded
                .methods
                .iter()
                .map(|method| &method.name)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_standalone_standard_library_object_has_no_companion_class() {
        let bytes = fixture_bytes("scala3-library/scala/jdk/CollectionConverters.tasty");
        let identity = decode_module_identity(
            &bytes,
            &BinaryName::from_internal("scala/jdk/CollectionConverters"),
        )
        .unwrap()
        .expect("CollectionConverters.tasty contains an object");

        assert_eq!(identity.companion_class_kind, None);
    }

    #[test]
    fn a_class_tasty_without_an_object_is_not_module_evidence() {
        let bytes = fixture_bytes("scala3-library/scala/annotation/StaticAnnotation.tasty");

        assert!(
            decode_module_identity(
                &bytes,
                &BinaryName::from_internal("scala/annotation/StaticAnnotation")
            )
            .unwrap()
            .is_none()
        );
    }

    #[test]
    fn resolves_a_shared_function1_superclass_to_object() {
        let bytes = fixture_bytes("scala3-library/scala/Function1.tasty");
        let decoded = decode(&bytes, &BinaryName::from_internal("scala/Function1"))
            .expect("the shared AnyRef superclass in Function1 should resolve");

        assert_eq!(
            decoded.super_class,
            BinaryName::from_internal("java/lang/Object")
        );
    }

    #[test]
    fn resolves_a_frozen_compiler_parent_through_an_ast_wrapped_reference() {
        let bytes = fixture_bytes("scala3-compiler/dotty/tools/dotc/core/Symbols.tasty");
        let profiles = profile_parent_references(
            &bytes,
            &BinaryName::from_internal("dotty/tools/dotc/core/Symbols$"),
            true,
            |_, _| None,
            |_, _| None,
        )
        .expect("the pinned compiler TASTy should profile");
        let sym_utils = profiles
            .iter()
            .find(|p| p.reference.parent_index == 0)
            .expect("Symbols parent 0 is SymUtils");
        assert_eq!(
            sym_utils.reference.reference_name.as_deref(),
            Some("SymUtils")
        );
        assert_eq!(
            sym_utils.reference.reference_prefix.as_deref(),
            Some("dotty/tools/dotc/core")
        );
        assert_eq!(
            sym_utils.resolved_binary_name.as_deref(),
            Some("dotty/tools/dotc/core/SymUtils")
        );
    }

    #[test]
    fn resolves_nested_applied_compiler_parent_constructor() {
        let trees_bytes = fixture_bytes("scala3-compiler/dotty/tools/dotc/ast/Trees.tasty");
        let trees_file = parse_classloader_tasty(&trees_bytes).unwrap();
        let trees_index = trees_file.ast_address_index().unwrap();
        let nested_binary_name = trees_index
            .iter()
            .filter(|node| node.tag == TYPEDEF_TAG)
            .find_map(|node| {
                let StructuredNode::TypeDef(DefinitionBody::TypeDef { name, .. }) =
                    node.decode_structured().ok()?
                else {
                    return None;
                };
                (trees_file.render_name(name).ok()?.as_str() == "Instance")
                    .then(|| resolve_symbol_address(&trees_file, node.offset as u32))?
            })
            .expect("Trees.tasty must identify the nested Instance owner chain");
        assert_eq!(nested_binary_name, "Trees$Instance");

        for (file, requested, expected_trait) in [
            (
                "scala3-compiler/dotty/tools/dotc/ast/tpd.tasty",
                "dotty/tools/dotc/ast/tpd$",
                "dotty/tools/dotc/ast/TypedTreeInfo",
            ),
            (
                "scala3-compiler/dotty/tools/dotc/ast/untpd.tasty",
                "dotty/tools/dotc/ast/untpd$",
                "dotty/tools/dotc/ast/UntypedTreeInfo",
            ),
        ] {
            let bytes = fixture_bytes(file);
            let decoded = decode_module_class_with_resolvers(
                &bytes,
                &BinaryName::from_internal(requested),
                |_, _| None,
                |_, _| None,
            )
            .unwrap_or_else(|error| panic!("{requested}: {error:?}"));

            assert_eq!(
                decoded.super_class.as_internal(),
                "dotty/tools/dotc/ast/Trees$Instance",
                "{requested}"
            );
            assert_eq!(
                decoded.interfaces,
                vec![BinaryName::from_internal(expected_trait)],
                "{requested}"
            );
        }
    }

    #[test]
    fn does_not_flatten_nested_select_in_owner_or_accept_operator_source_name() {
        let unicode_child = fixture_bytes("scala3-parent-unicode/review/Child.tasty");
        let decoded = decode_with_resolvers(
            &unicode_child,
            &BinaryName::from_internal("review/Child"),
            |_, _| None,
            |_, _| None,
        )
        .expect("valid Unicode superclass names must remain resolvable");
        assert_eq!(decoded.super_class.as_internal(), "review/Żółw");

        let combining_child = fixture_bytes("scala3-parent-unicode/review/CafeChild.tasty");
        let decoded = decode_with_resolvers(
            &combining_child,
            &BinaryName::from_internal("review/CafeChild"),
            |_, _| None,
            |_, _| None,
        )
        .expect("valid Unicode combining marks must remain resolvable");
        assert_eq!(decoded.super_class.as_internal(), "review/Café");

        let recheck = fixture_bytes("scala3-compiler/dotty/tools/dotc/transform/Recheck.tasty");
        let error = decode_with_resolvers(
            &recheck,
            &BinaryName::from_internal("dotty/tools/dotc/transform/Recheck"),
            |_, _| None,
            |_, _| None,
        )
        .expect_err("the nested Phase owner must not be flattened into a package path");
        assert!(matches!(
            error,
            TastyDecodeError::UnresolvedSupertype(parent)
                if parent.parent_index == 0 && parent.reference_name.as_deref() == Some("Phase")
        ));

        for tuple in ["Tuple2", "Tuple3"] {
            let bytes = fixture_bytes(&format!("scala3-library/scala/{tuple}.tasty"));
            let error = decode_with_resolvers(
                &bytes,
                &BinaryName::from_internal(format!("scala/{tuple}")),
                |_, _| None,
                |_, _| None,
            )
            .expect_err("source operator spelling is not a binary constructor name");
            assert!(
                matches!(
                    error,
                    TastyDecodeError::UnresolvedSupertype(ref parent)
                        if parent.parent_index == 0 && parent.reference_name.as_deref() == Some("*:")
                ),
                "{tuple}: {error:?}"
            );
        }
    }

    #[test]
    fn parent_profiles_report_absolute_template_addresses_from_the_ast_index() {
        for (fixture, requested, expected_address) in [
            (
                "scala3-compiler/dotty/tools/dotc/core/Types.tasty",
                "dotty/tools/dotc/core/Types$",
                410usize,
            ),
            (
                "scala3-compiler/dotty/tools/dotc/core/Symbols.tasty",
                "dotty/tools/dotc/core/Symbols$",
                308usize,
            ),
        ] {
            let bytes = fixture_bytes(fixture);
            let file = parse_classloader_tasty(&bytes).unwrap();
            let index = file.ast_address_index().unwrap();
            let profiles = profile_parent_references(
                &bytes,
                &BinaryName::from_internal(requested),
                true,
                |_, _| None,
                |_, _| None,
            )
            .unwrap();

            assert!(!profiles.is_empty(), "{requested}");
            assert!(
                profiles.iter().all(|profile| {
                    profile.reference.template_wire_address == expected_address
                        && index
                            .get_node(expected_address as u32)
                            .is_some_and(|node| node.tag == TEMPLATE_TAG)
                }),
                "{requested}: {profiles:#?}"
            );

            let expected_parent = if requested.ends_with("Symbols$") {
                "dotty/tools/dotc/core/SymUtils"
            } else {
                "dotty/tools/dotc/core/TypeUtils"
            };
            let decoded = decode_module_class_with_resolvers(
                &bytes,
                &BinaryName::from_internal(requested),
                |_, _| None,
                |_, _| None,
            )
            .unwrap_or_else(|error| panic!("authoritative parent for {requested}: {error:?}"));
            assert_eq!(
                decoded.super_class.as_internal(),
                expected_parent,
                "{requested}"
            );
        }
    }

    #[test]
    fn profiles_retains_cap_parent_references_through_apply_and_select_in() {
        let bytes = fixture_bytes("scala3-library/scala/annotation/retainsCap.tasty");
        let decoded = decode_with_resolvers(
            &bytes,
            &BinaryName::from_internal("scala/annotation/retainsCap"),
            |_, _| None,
            |_, _| None,
        )
        .expect("the package-owned Annotation parents are fully identifiable in TASTy");
        assert_eq!(
            decoded.super_class.as_internal(),
            "scala/annotation/Annotation"
        );
        assert_eq!(
            decoded.interfaces[0].as_internal(),
            "scala/annotation/StaticAnnotation"
        );

        let profiles = profile_parent_references(
            &bytes,
            &BinaryName::from_internal("scala/annotation/retainsCap"),
            false,
            |_, _| None,
            |_, _| None,
        )
        .expect("the parent trees should retain their diagnostic evidence");
        let parent = &profiles[1];

        assert_eq!(parent.reference.parent_index, 1);
        assert!(
            parent.reference.template_wire_address > 0,
            "profile: {parent:#?}"
        );
        assert!(
            parent.reference.has_semantic_reference,
            "profile: {parent:#?}"
        );
        assert!(
            parent.reference.has_reference_prefix,
            "profile: {parent:#?}"
        );
        assert!(
            parent
                .reference
                .visible_names
                .iter()
                .any(|name| name == "StaticAnnotation")
        );
        assert!(
            parent
                .reference
                .visible_names
                .iter()
                .any(|name| name == "scala/annotation")
        );
        assert_eq!(
            parent.reference.shape, "selected-qualified",
            "profile: {parent:#?}"
        );
        assert_eq!(parent.reference.reference_tag.as_deref(), Some("0x71"));
        assert_eq!(
            parent.reference.reference_name.as_deref(),
            Some("StaticAnnotation")
        );
        assert_eq!(
            parent.reference.reference_prefix.as_deref(),
            Some("scala/annotation")
        );
        assert_eq!(
            parent.reference.alias_owner.as_deref(),
            Some("scala/annotation")
        );
        assert_eq!(
            parent.reference.alias_name.as_deref(),
            Some("StaticAnnotation")
        );
        assert_eq!(
            parent.resolved_binary_name.as_deref(),
            Some("scala/annotation/StaticAnnotation")
        );
    }

    #[test]
    fn pins_predef_parent_to_its_exact_same_package_identity() {
        let bytes = fixture_bytes("scala3-library/scala/Predef.tasty");
        let profiles = profile_parent_references(
            &bytes,
            &BinaryName::from_internal("scala/Predef$"),
            true,
            |_, _| None,
            |_, _| None,
        )
        .expect("the pinned Scala 3.9 Predef TASTy should be structurally readable");
        let parent = profiles.first().expect("Predef declares a parent");

        assert_eq!(parent.reference.parent_index, 0);
        assert_eq!(parent.reference.shape, "package-qualified");
        assert_eq!(
            parent.reference.reference_name.as_deref(),
            Some("LowPriorityImplicits")
        );
        assert!(parent.reference.has_semantic_reference);
        assert!(parent.reference.has_reference_prefix);
        assert_eq!(parent.reference.reference_prefix.as_deref(), Some("scala"));
        assert_eq!(
            parent.resolution_stage,
            "semantic-or-structural-reference-resolved"
        );
        assert_eq!(
            parent.resolved_binary_name.as_deref(),
            Some("scala/LowPriorityImplicits")
        );

        let decoded = decode_module_class_with_resolvers(
            &bytes,
            &BinaryName::from_internal("scala/Predef$"),
            |_, _| None,
            |_, _| None,
        )
        .expect("Predef's TASTy hierarchy should decode without import fallback");
        assert_eq!(
            decoded.super_class.as_internal(),
            "scala/LowPriorityImplicits"
        );
    }

    #[test]
    fn keeps_object_owned_selected_parent_unresolved() {
        let bytes = fixture_bytes("scala3-parent-selected/review/OwnerBaseChild.tasty");
        let mut attempted_package_alias = false;
        let error = decode_with_resolvers(
            &bytes,
            &BinaryName::from_internal("review/Child"),
            |_, _| {
                attempted_package_alias = true;
                None
            },
            |_, _| None,
        )
        .expect_err("an object-owned type path is not a package-qualified JVM name");

        assert!(
            !attempted_package_alias,
            "object-owned type selections must not be reinterpreted as package aliases"
        );

        assert!(matches!(
            &error,
            TastyDecodeError::UnresolvedSupertype(parent)
                if parent.parent_index == 1
                    && parent.shape == "selected-qualified"
                    && parent.reference_prefix.as_deref() == Some("Owner")
        ));
    }

    #[test]
    fn does_not_resolve_object_owned_selected_parent_as_package_alias() {
        let bytes = fixture_bytes("scala3-parent-selected/review/FullyQualifiedOwnerChild.tasty");
        let mut attempted_package_alias = false;
        let result = decode_with_resolvers(
            &bytes,
            &BinaryName::from_internal("review/FullyQualifiedOwnerChild"),
            |owner, alias| {
                attempted_package_alias = true;
                assert_eq!(owner.as_internal(), "review/Owner/package$");
                assert_eq!(alias, "Base");
                Some(BinaryName::from_internal("review/Injected"))
            },
            |_, _| None,
        );

        assert!(
            !attempted_package_alias,
            "object-owned selections must not be reinterpreted as package aliases"
        );
        assert!(
            result.is_err(),
            "an object-owned parent must not resolve through a package alias"
        );
    }

    #[test]
    fn keeps_two_object_owned_selected_parent_unresolved() {
        let error = decode(
            &fixture_bytes("scala3-parent-selected/review/TwoOwnerBaseChild.tasty"),
            &BinaryName::from_internal("review/TwoOwnersChild"),
        )
        .expect_err("nested object owners cannot be joined as package segments");

        assert!(matches!(
            &error,
            TastyDecodeError::UnresolvedSupertype(parent)
                if parent.parent_index == 1 && parent.shape == "selected-qualified"
        ));
    }

    #[test]
    fn keeps_fully_qualified_object_owned_selected_parent_unresolved() {
        let error = decode(
            &fixture_bytes("scala3-parent-selected/review/FullyQualifiedOwnerChild.tasty"),
            &BinaryName::from_internal("review/FullyQualifiedOwnerChild"),
        )
        .expect_err("a fully qualified object path is not a package path");

        assert!(matches!(
            &error,
            TastyDecodeError::UnresolvedSupertype(parent)
                if parent.parent_index == 1 && parent.shape == "selected-qualified"
        ));
    }

    #[test]
    fn keeps_fully_qualified_nested_object_parent_unresolved() {
        let error = decode(
            &fixture_bytes("scala3-parent-selected/review/FullyQualifiedNestedChild.tasty"),
            &BinaryName::from_internal("review/FullyQualifiedNestedChild"),
        )
        .expect_err("a fully qualified nested object path is not a package path");

        assert!(matches!(
            &error,
            TastyDecodeError::UnresolvedSupertype(parent)
                if parent.parent_index == 1 && parent.shape == "selected-qualified"
        ));
    }

    #[test]
    fn decodes_array_java_parents_from_selected_package_paths() {
        let bytes = fixture_bytes("scala3-library/scala/Array.tasty");
        let decoded = decode(&bytes, &BinaryName::from_internal("scala/Array"))
            .expect("Array's selected-qualified java parent paths should resolve");

        assert_eq!(decoded.super_class.as_internal(), "java/lang/Object");
        assert_eq!(
            decoded.interfaces,
            vec![
                BinaryName::from_internal("java/io/Serializable"),
                BinaryName::from_internal("java/lang/Cloneable"),
            ]
        );
    }

    #[test]
    fn resolves_a_package_qualified_type_alias_parent_through_package_tasty() {
        let bytes = fixture_bytes("scala3-library/scala/collection/mutable/Cloneable.tasty");
        let mut resolved_scala_alias = false;
        let decoded = decode_with_resolvers(
            &bytes,
            &BinaryName::from_internal("scala/collection/mutable/Cloneable"),
            |owner, alias| {
                if owner.as_internal() == "scala/package$" && alias == "Cloneable" {
                    resolved_scala_alias = true;
                    Some(BinaryName::from_internal("java/lang/Cloneable"))
                } else {
                    None
                }
            },
            |_, _| None,
        )
        .expect("pinned mutable.Cloneable TASTy should decode");

        assert!(resolved_scala_alias);
        assert_eq!(
            decoded.interfaces,
            vec![BinaryName::from_internal("java/lang/Cloneable")]
        );
    }

    #[test]
    fn resolves_package_object_aliases_in_their_defining_package() {
        let bytes = fixture_bytes("scala3-library/scala/package.tasty");
        let file = TastyFile::parse_scala_3_9(&bytes).unwrap();

        assert_eq!(
            resolve_type_alias_candidate(&file, "Iterator", "scala"),
            Some(BinaryName::from_internal("collection/Iterator"))
        );
    }

    #[test]
    fn resolves_package_object_term_aliases_in_their_defining_package() {
        let bytes = fixture_bytes("scala3-library/scala/package.tasty");
        let file = TastyFile::parse_scala_3_9(&bytes).unwrap();

        assert_eq!(
            resolve_term_alias_target(&file, "Nil", "scala").unwrap(),
            Some(TypeAliasTarget::Candidate(BinaryName::from_internal(
                "collection/immutable/Nil"
            )))
        );
        assert_eq!(
            resolve_term_alias_target(&file, "AnyRef", "scala").unwrap(),
            None,
            "a package-object value constructed with `new` is not an object alias"
        );
    }

    #[test]
    fn package_term_alias_lookup_rejects_nested_and_inaccessible_values() {
        let bytes = fixture_bytes("scala3-library/scala/package.tasty");
        let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
        let index = file.ast_address_index().unwrap();
        let nested_value = index
            .iter()
            .filter(|node| node.tag == VALDEF_TAG)
            .find(|node| {
                index
                    .parent_of(node.offset as u32)
                    .is_none_or(|parent| parent.tag != TEMPLATE_TAG)
            })
            .expect("package TASTy should contain nested values as well as package members");

        assert!(!is_public_package_object_value(&file, &index, nested_value, "scala").unwrap());
        assert!(is_public_package_object_value_tail(&[]));
        assert!(!is_public_package_object_value_tail(&[
            DefinitionTail::Modifier(PRIVATE_TAG,)
        ]));
        assert!(!is_public_package_object_value_tail(&[
            DefinitionTail::Modifier(PROTECTED_TAG,)
        ]));
        assert!(!is_public_package_object_value_tail(&[
            DefinitionTail::Modifier(LOCAL_TAG,)
        ]));
        assert!(!is_public_package_object_value_tail(&[
            DefinitionTail::Modifier(MUTABLE_TAG,)
        ]));
    }

    #[test]
    fn resolves_a_symbol_reference_through_its_tasty_owner_chain() {
        let bytes = fixture_bytes("scala3-library/scala/math/Ordering.tasty");
        let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
        let index = file.ast_address_index().unwrap();
        let address = index
            .iter()
            .find_map(|node| {
                if node.tag != TYPEDEF_TAG {
                    return None;
                }
                let StructuredNode::TypeDef(DefinitionBody::TypeDef { name, .. }) =
                    node.decode_structured().ok()?
                else {
                    return None;
                };
                (wire_name(&file, name).as_deref() == Some("Reverse"))
                    .then(|| u32::try_from(node.offset).ok())
                    .flatten()
            })
            .expect("Reverse's type definition should have an AST address");
        let reference = RawTree::NatAst {
            tag: TYPEREFSYMBOL_TAG,
            offset: 0,
            value: address,
            child: Box::new(RawTree::Leaf(SimpleTerm {
                tag: dotty_tasty::tasty::THIS_TAG,
                offset: 0,
                value: TermValue::Tag,
            })),
        };
        assert!(matches!(
            resolve_reference_name_in_package(&file, &reference, Some("scala/math")),
            ReferenceResolution::Resolved(name)
                if name.as_internal() == "scala/math/Ordering$Reverse"
        ));
        let shared = RawTree::Leaf(SimpleTerm {
            tag: SHAREDTYPE_TAG,
            offset: 0,
            value: TermValue::AstRef(address),
        });
        assert_eq!(
            resolve_reference_prefix_in_package(&file, &shared, Some("scala/math")).as_deref(),
            Some("scala/math/Ordering$Reverse")
        );

        let malformed = RawTree::NatAst {
            tag: TYPEREFSYMBOL_TAG,
            offset: 0,
            value: u32::MAX,
            child: Box::new(RawTree::Leaf(SimpleTerm {
                tag: dotty_tasty::tasty::THIS_TAG,
                offset: 0,
                value: TermValue::Tag,
            })),
        };
        assert!(matches!(
            resolve_reference_name_in_package(&file, &malformed, Some("scala/math")),
            ReferenceResolution::Unresolved
        ));
    }

    #[test]
    fn decodes_a_trait_as_an_interface_with_no_declared_interfaces() {
        let decoded = decode(
            &fixture_bytes("inheritance/Animal.tasty"),
            &BinaryName::from_internal("Animal"),
        )
        .unwrap();

        assert!(decoded.flags.is_interface());
        assert_eq!(decoded.super_class.as_internal(), "java/lang/Object");
        assert!(decoded.interfaces.is_empty());
    }

    /// `Dog.tasty`'s `Animal` mixin is a real, post-typecheck reference
    /// carrying its own compiled `TERMREFpkg` prefix (`me.cytrowski.
    /// tastyfixtures`, this fixture's real source package) —
    /// [`resolve_reference_name`] resolves it directly, so it no longer
    /// matters that `Dog` itself was requested with no package at all.
    #[test]
    fn decodes_a_class_with_a_trait_mixin_as_an_interface() {
        let decoded = decode(
            &fixture_bytes("inheritance/Dog.tasty"),
            &BinaryName::from_internal("Dog"),
        )
        .unwrap();

        assert!(!decoded.flags.is_interface());
        assert_eq!(decoded.super_class.as_internal(), "java/lang/Object");
        assert_eq!(
            decoded.interfaces,
            vec![BinaryName::from_internal(
                "me/cytrowski/tastyfixtures/Animal"
            )]
        );
    }

    /// The same fixture requested under a completely different,
    /// fictitious package: `Animal`'s reference still resolves to its
    /// own real compiled package, not `com/example` — a real
    /// `TERMREFpkg`-qualified reference is authoritative and never
    /// deferred to the requested class's own package the way a
    /// genuinely bare (unresolvable) reference is (see
    /// [`resolve_parent_name_with_bare`]'s flatten-based fallback, still covered
    /// by `does_not_requalify_an_already_cross_package_mixin` below).
    #[test]
    fn a_real_qualified_mixin_resolves_to_its_own_package_not_the_requested_ones() {
        let decoded = decode(
            &fixture_bytes("inheritance/Dog.tasty"),
            &BinaryName::from_internal("com/example/Dog"),
        )
        .unwrap();

        assert_eq!(
            decoded.interfaces,
            vec![BinaryName::from_internal(
                "me/cytrowski/tastyfixtures/Animal"
            )]
        );
    }

    #[test]
    fn decodes_a_case_class_with_cross_package_mixins() {
        let decoded = decode(
            &fixture_bytes("case_class/Point.tasty"),
            &BinaryName::from_internal("Point"),
        )
        .unwrap();

        assert_eq!(decoded.super_class.as_internal(), "java/lang/Object");
        assert_eq!(
            decoded.interfaces,
            vec![
                BinaryName::from_internal("scala/Product"),
                BinaryName::from_internal("scala/Serializable"),
            ]
        );
    }

    /// The same fixture as `decodes_a_case_class_with_cross_package_mixins`,
    /// but requested under a (fictitious) non-root package — proving the
    /// requested package only fills in a *bare* reference and is never
    /// applied on top of `Product`/`Serializable`'s already fully
    /// `_root_`-qualified chain.
    #[test]
    fn does_not_requalify_an_already_cross_package_mixin() {
        let decoded = decode(
            &fixture_bytes("case_class/Point.tasty"),
            &BinaryName::from_internal("geometry/Point"),
        )
        .unwrap();

        assert_eq!(
            decoded.interfaces,
            vec![
                BinaryName::from_internal("scala/Product"),
                BinaryName::from_internal("scala/Serializable"),
            ]
        );
    }

    /// `scala3-library/scala/io/Source.tasty`'s real `Source` mixes in
    /// `Iterable[Char]` — a generic mixin encoded as an `AppliedTpt`
    /// node, not the plain `IdentTpt`/`SelectTpt` the other tests here
    /// exercise. `resolve_applied_type_name` correctly extracts and
    /// recurses into its `tycon`: `Closeable`'s own reference (a plain,
    /// non-generic mixin) carries a real `TERMREFpkg` prefix and
    /// resolves fully, proving that machinery works. `Iterator`'s tycon
    /// does not — it is `scala.package$.Iterator`, a reference through
    /// Scala's compiler-synthesized `package object scala` (a type-alias
    /// member, not a direct package-qualified class reference). The
    /// structural decoder alone cannot resolve this alias. A deliberately
    /// plausible same-package callback must not replace that semantic
    /// reference with `scala/io/Iterator`; the classloader's classpath-aware
    /// entry point instead follows the package-object alias and validates its
    /// actual target.
    #[test]
    fn decoding_fails_when_a_generic_mixins_tycon_is_an_unresolvable_implicit_import_reference() {
        let error = decode_with_resolvers(
            &fixture_bytes("scala3-library/scala/io/Source.tasty"),
            &BinaryName::from_internal("scala/io/Source"),
            |_, _| None,
            |package, simple_name| {
                Some(BinaryName::from_internal(format!(
                    "{package}/{simple_name}"
                )))
            },
        )
        .unwrap_err();

        let TastyDecodeError::UnresolvedSupertype(parent) = error else {
            panic!("expected unresolved supertype, got {error:?}");
        };
        assert_eq!(parent.shape, "applied");
        assert_eq!(parent.parent_index, 1);
        assert!(parent.template_wire_address > 0, "profile: {parent:#?}");
        assert_eq!(
            parent.reference_tag.as_deref(),
            Some("0x75"),
            "profile: {parent:#?}"
        );
        assert_eq!(parent.reference_name.as_deref(), Some("Iterator"));
        assert_eq!(parent.reference_prefix.as_deref(), Some("scala/package"));
        assert_eq!(parent.alias_owner.as_deref(), Some("scala/package"));
        assert_eq!(parent.alias_name.as_deref(), Some("Iterator"));
        assert!(parent.has_semantic_reference);
        assert!(!parent.has_reference_prefix);
        assert!(parent.visible_names.iter().any(|name| name == "Iterator"));
        assert!(!parent.raw_tree.is_empty());
    }

    /// `scala3-library/scala/math/BigInt.tasty`'s real `BigInt` mixes in
    /// `Ordered[BigInt]` alongside two plain (non-generic) mixins —
    /// covering an `AppliedType` mixin (not `AppliedTpt`) that isn't the
    /// only, or the first, non-superclass parent. `ScalaNumericConversions`
    /// and `Ordered` both carry real `TERMREFpkg` prefixes and resolve
    /// fully (same machinery proven by the `AppliedTpt` case above), but
    /// the synthetic `Serializable` mixin is the same kind of
    /// unresolvable implicit-import reference as `Iterator` there, so —
    /// per the same `ReferenceResolution`/`UnresolvedSupertype` reasoning
    /// — the whole decode now fails rather than silently guessing a bare
    /// `Serializable`.
    #[test]
    fn decoding_fails_when_one_of_several_mixins_is_an_unresolvable_implicit_import_reference() {
        let error = decode(
            &fixture_bytes("scala3-library/scala/math/BigInt.tasty"),
            &BinaryName::from_internal("BigInt"),
        )
        .unwrap_err();

        assert!(matches!(
            error,
            TastyDecodeError::UnresolvedSupertype(parent)
                if parent.parent_index > 0 && !parent.raw_tree.is_empty()
        ));
    }

    /// `scala3-library/scala/math/Ordering.tasty`'s private nested
    /// `Reverse` mixes in `Ordering[T]` — its own enclosing class,
    /// applied to a type parameter, still just an `AppliedType` mixin
    /// resolving to the bare name `Ordering`.
    #[test]
    fn decodes_a_private_class_with_a_generic_mixin_naming_its_own_enclosing_class() {
        let decoded = decode(
            &fixture_bytes("scala3-library/scala/math/Ordering.tasty"),
            &BinaryName::from_internal("Reverse"),
        )
        .unwrap();

        assert!(!decoded.flags.is_public());
        assert_eq!(decoded.visibility, Some(DeclaredVisibility::Private));
        assert_eq!(decoded.super_class.as_internal(), "java/lang/Object");
        assert_eq!(
            decoded.interfaces,
            vec![BinaryName::from_internal("Ordering")]
        );
    }

    #[test]
    fn decodes_a_public_classs_visibility_as_none() {
        let decoded = decode(
            &fixture_bytes("inheritance/Dog.tasty"),
            &BinaryName::from_internal("Dog"),
        )
        .unwrap();

        assert_eq!(decoded.visibility, None);
    }

    #[test]
    fn missing_type_def_when_no_definition_matches_the_requested_name() {
        let error = decode(
            &fixture_bytes("inheritance/Dog.tasty"),
            &BinaryName::from_internal("DoesNotExist"),
        )
        .unwrap_err();

        assert!(matches!(error, TastyDecodeError::MissingTypeDef));
    }

    #[test]
    fn parse_error_when_the_bytes_are_not_a_tasty_file() {
        let error =
            decode(b"not a tasty file", &BinaryName::from_internal("Anything")).unwrap_err();

        assert!(matches!(error, TastyDecodeError::Parse(_)));
    }

    /// `opaque/UserId.tasty` declares a real `opaque type UserId`, not
    /// a class/trait/object — a `TYPEDEF_TAG` whose `type_or_template`
    /// is a type tree, not a `Template`. `decode` searches every
    /// `TYPEDEF_TAG` regardless of what kind of definition it is (a
    /// type alias included), so requesting one by name reaches this
    /// path instead of `MissingTypeDef`.
    /// `decode`'s own `?` on `node.decode_structured()`/
    /// `template_node.decode_structured()` is the only place a
    /// [`TastyDecodeError::Ast`] can arise (`file.ast_address_index()`'s
    /// own `AstError`s are converted to [`TastyDecodeError::Parse`], not
    /// `Ast`, since that call fails before any specific `TypeDef` is
    /// even found) — but no real fixture happens to be malformed in
    /// exactly that way. Corrupting one byte of a real, otherwise-valid
    /// `.tasty` file (`inheritance/Animal.tasty`) is the standard way to
    /// exercise a decode-time structural error without hand-building a
    /// whole synthetic wire-format file (mirrors
    /// `loader.rs::synthetic_malformed_this_class`'s equivalent
    /// technique for `.class`). This offset was found by flipping each
    /// byte of the fixture in turn and keeping one whose mutation still
    /// parses as a `.tasty` file and still indexes successfully (so
    /// `decode` gets as far as finding the `Animal` `TypeDef`) but fails
    /// exactly at `decode_structured()`.
    #[test]
    fn ast_error_when_a_definitions_body_is_corrupted() {
        let mut bytes = fixture_bytes("inheritance/Animal.tasty");
        bytes[343] = bytes[343].wrapping_add(1);

        let error = decode(&bytes, &BinaryName::from_internal("Animal")).unwrap_err();

        assert!(matches!(error, TastyDecodeError::Ast(_)));
    }

    #[test]
    fn no_template_body_when_the_matching_type_def_is_a_type_alias() {
        let error = decode(
            &fixture_bytes("opaque/UserId.tasty"),
            &BinaryName::from_internal("UserId"),
        )
        .unwrap_err();

        assert!(matches!(error, TastyDecodeError::NoTemplateBody));
    }

    /// `scala3-library/scala/concurrent/impl/CompletionLatch.tasty`'s
    /// real `CompletionLatch` mixes in `Try[T] => Unit` (a function
    /// type, `Function1[Try[T], Unit]` under the hood) — an
    /// `AppliedType` whose own `tycon` is a real, post-typecheck
    /// `TYPEREF` referencing its target by a prefix/name-table pair
    /// (`scala`'s `TERMREFpkg` prefix plus `Function1`'s own `NameRef`),
    /// not a source-level `IDENT`/`SELECT` chain. Before
    /// [`resolve_reference_name`] existed, [`RawTree::name_refs`]
    /// couldn't see this shape at all (it only visits `IDENT`/`IDENTTPT`
    /// occurrences, not a bare `TYPEREF`'s own name), so
    /// [`resolve_applied_type_name`] gave up and this surfaced as
    /// `UnresolvedSupertype`. It now resolves correctly.
    #[test]
    fn resolves_a_generic_mixins_bare_typeref_tycon() {
        let decoded = decode(
            &fixture_bytes("scala3-library/scala/concurrent/impl/CompletionLatch.tasty"),
            &BinaryName::from_internal("CompletionLatch"),
        )
        .unwrap();

        assert_eq!(
            decoded.interfaces,
            vec![BinaryName::from_internal("scala/Function1")]
        );
    }

    /// `decode_flags` is a pure, narrow mapping function. `TRAIT_TAG`/
    /// `ABSTRACT_TAG`/`FINAL_TAG`/`PRIVATE_TAG` are all exercised
    /// end-to-end above, through real fixtures (`Animal`, `Source`,
    /// `BigInt`, `Ordering`'s `Reverse`); `PROTECTED_TAG` has no such
    /// fixture anywhere in `dotty-tasty`'s real `scala3-library` corpus
    /// (confirmed by scanning every committed fixture's `TypeDef`
    /// modifiers), so it — and the all-defaults case — are tested
    /// directly against hand-built `tail` vectors instead, the same
    /// treatment `manifest.rs`'s hand-written-input tests give its own
    /// narrow parsing concern.
    #[test]
    fn decode_flags_clears_public_for_a_protected_modifier() {
        let flags = decode_flags(&[DefinitionTail::Modifier(PROTECTED_TAG)]);

        assert!(!flags.is_public());
    }

    #[test]
    fn decode_flags_defaults_to_public_with_no_modifiers() {
        let flags = decode_flags(&[]);

        assert!(flags.is_public());
    }

    /// [`decode_flags`] collapses `PRIVATE_TAG`/`PROTECTED_TAG` down to
    /// the same "not public" `ClassAccessFlags` bit (correct for
    /// `.class`, which has no other way to represent a top-level class's
    /// visibility — see [`DeclaredVisibility`]'s doc comment), but
    /// [`decode_visibility`] must keep them apart.
    /// Runs `check` with some real, parsed file; `decode_visibility` only
    /// reads its name table for qualified modifiers.
    fn with_a_file(check: impl FnOnce(&TastyFile<'_>)) {
        let bytes = fixture_bytes("case_class/Point.tasty");
        let file = TastyFile::parse_scala_3_9(&bytes).unwrap();
        check(&file);
    }

    #[test]
    fn decode_visibility_distinguishes_private_from_protected() {
        with_a_file(|file| {
            assert_eq!(
                decode_visibility(file, &[DefinitionTail::Modifier(PRIVATE_TAG)]),
                Some(DeclaredVisibility::Private)
            );
            assert_eq!(
                decode_visibility(file, &[DefinitionTail::Modifier(PROTECTED_TAG)]),
                Some(DeclaredVisibility::Protected)
            );
        });
    }

    #[test]
    fn decode_visibility_is_none_with_no_modifiers() {
        with_a_file(|file| assert_eq!(decode_visibility(file, &[]), None));
    }

    fn visibility_fixture(class: &str) -> Vec<u8> {
        std::fs::read(
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/tasty_visibility/me/cytrowski/tastyfixtures/visibility")
                .join(format!("{class}.tasty")),
        )
        .unwrap_or_else(|error| panic!("fixture {class} should exist: {error}"))
    }

    fn declared_visibility(class: &str) -> Option<DeclaredVisibility> {
        decode(
            &visibility_fixture(class),
            &BinaryName::from_internal(class),
        )
        .unwrap()
        .visibility
    }

    fn package(path: &str) -> DeclaredVisibility {
        DeclaredVisibility::PrivateWithin(DeclaredQualifier::Package(path.to_owned()))
    }

    /// Issue #16: a qualified modifier used to be skipped, which left the
    /// class at the default `Public`.
    #[test]
    fn decodes_a_private_within_a_package_qualifier() {
        assert_eq!(
            declared_visibility("InEnclosingPackage"),
            Some(package("me/cytrowski/tastyfixtures"))
        );
        assert_eq!(
            declared_visibility("InOwnPackage"),
            Some(package("me/cytrowski/tastyfixtures/visibility"))
        );
        assert_eq!(declared_visibility("InOuterPackage"), Some(package("me")));
    }

    /// The compiler writes a top-level `private` class as private to its own
    /// package, so it is a qualified modifier too.
    #[test]
    fn a_top_level_private_class_is_private_within_its_package() {
        assert_eq!(
            declared_visibility("PlainPrivate"),
            Some(package("me/cytrowski/tastyfixtures/visibility"))
        );
    }
    fn qualified_tail(tag: u8, child: RawTree<'static>) -> Vec<DefinitionTail<'static>> {
        vec![DefinitionTail::QualifiedModifier(AstChildNode {
            tag,
            offset: 0,
            child,
        })]
    }

    /// A real `protected[visibility]` (Scala only allows it on a member, so
    /// the fixture nests the class in an object). `decode` finds nested
    /// classes by simple name, which is how `Ordering.tasty`'s `Reverse` is
    /// read too.
    #[test]
    fn decodes_a_real_protected_within_a_package_qualifier() {
        let decoded = decode(
            &visibility_fixture("Holder"),
            &BinaryName::from_internal("InProtected"),
        )
        .unwrap();

        assert_eq!(
            decoded.visibility,
            Some(DeclaredVisibility::ProtectedWithin(
                DeclaredQualifier::Package("me/cytrowski/tastyfixtures/visibility".to_owned())
            ))
        );
    }

    #[test]
    fn decodes_a_protected_qualified_modifier() {
        with_a_file(|file| {
            // Name reference 0 is a plain entry of the name table.
            let child =
                RawTree::Leaf(SimpleTerm::new(TYPEREFPKG_TAG, TermValue::NameRef(0)).unwrap());
            let tail = qualified_tail(PROTECTEDQUALIFIED_TAG, child);

            assert!(matches!(
                decode_visibility(file, &tail),
                Some(DeclaredVisibility::ProtectedWithin(
                    DeclaredQualifier::Package(_)
                ))
            ));
        });
    }

    #[test]
    fn a_qualifier_that_is_not_a_package_name_is_reported_as_other() {
        with_a_file(|file| {
            let child =
                RawTree::Leaf(SimpleTerm::new(SHAREDTYPE_TAG, TermValue::AstRef(4)).unwrap());
            let tail = qualified_tail(PRIVATEQUALIFIED_TAG, child);

            assert_eq!(
                decode_visibility(file, &tail),
                Some(DeclaredVisibility::PrivateWithin(DeclaredQualifier::Other))
            );
        });
    }

    #[test]
    fn an_open_class_has_no_declared_visibility() {
        assert_eq!(declared_visibility("Open"), None);
    }

    fn fields_fixture(class: &str) -> Vec<u8> {
        std::fs::read(
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/tasty_fields/me/cytrowski/tastyfixtures/fields")
                .join(format!("{class}.tasty")),
        )
        .unwrap_or_else(|error| panic!("fixture {class} should exist: {error}"))
    }

    fn decoded_fields(class: &str) -> Vec<DecodedTastyField> {
        decode(&fields_fixture(class), &BinaryName::from_internal(class))
            .unwrap()
            .fields
    }

    fn field_names(fields: &[DecodedTastyField]) -> Vec<&str> {
        fields.iter().map(|field| field.name.as_str()).collect()
    }

    /// Issue #11: `class P(val a: Int, b: Int, var c: Int)`. `FIELDACCESSOR`
    /// is on the setter `c_=`, not on `a` or `c`, so a `val`/`var` parameter
    /// is recognised by not being `private[this]`.
    #[test]
    fn a_val_and_a_var_constructor_parameter_are_fields_and_a_plain_one_is_not() {
        let fields = decoded_fields("P");

        assert_eq!(field_names(&fields), vec!["a", "c"]);
        // A `val` is final, a `var` is not.
        assert!(fields[0].flags.is_public() && fields[0].flags.is_final());
        assert!(fields[1].flags.is_public() && !fields[1].flags.is_final());
    }

    #[test]
    fn a_private_or_protected_val_parameter_is_a_field_with_that_access() {
        let fields = decoded_fields("Q");
        assert_eq!(field_names(&fields), vec!["d", "e"]);
        assert!(fields[0].flags.is_private());
        assert!(fields[1].flags.is_protected());
    }

    #[test]
    fn a_plain_parameter_is_not_a_field_but_a_body_val_still_is() {
        assert_eq!(field_names(&decoded_fields("Body")), vec!["inBody"]);
    }

    /// `case_class/Point.tasty`'s real `case class Point(x: Int, y: Int)`
    /// never repeats `x`/`y` as `ValDef`s in `Template.stats` — they only
    /// appear as `CASEACCESSOR_TAG`-tagged primary-constructor term
    /// parameters, which is exactly what
    /// [`decode_constructor_accessor_fields`] (not [`decode_members`])
    /// reconstructs.
    #[test]
    fn decodes_a_case_classs_constructor_accessor_fields() {
        let decoded = decode(
            &fixture_bytes("case_class/Point.tasty"),
            &BinaryName::from_internal("Point"),
        )
        .unwrap();

        let names: Vec<&str> = decoded
            .fields
            .iter()
            .map(|field| field.name.as_str())
            .collect();
        assert_eq!(names, vec!["x", "y"]);
        for field in &decoded.fields {
            assert!(field.flags.is_public());
            assert!(field.flags.is_final());
        }
    }

    /// `Point`'s compiler-synthesized case class boilerplate (`copy`,
    /// `hashCode`, `equals`, ...) alongside its real primary constructor
    /// — `<init>`'s own `return_type` is always `None` (never a value
    /// type to resolve a name from), and a synthesized method carries
    /// `ACC_SYNTHETIC` (from `SYNTHETIC_TAG`/`ARTIFACT_TAG`).
    #[test]
    fn decodes_a_case_classs_synthesized_methods_including_the_constructor() {
        let decoded = decode(
            &fixture_bytes("case_class/Point.tasty"),
            &BinaryName::from_internal("Point"),
        )
        .unwrap();

        let init = decoded
            .methods
            .iter()
            .find(|method| method.name == "<init>")
            .expect("<init> should be decoded");
        assert_eq!(init.parameters.len(), 2);
        assert_eq!(init.return_type, None);
        assert!(init.flags.is_public());

        let copy = decoded
            .methods
            .iter()
            .find(|method| method.name == "copy")
            .expect("copy should be decoded");
        assert!(copy.flags.is_synthetic());
        assert_eq!(copy.parameters.len(), 2);
    }

    #[test]
    fn decodes_scala39_some_apply_with_address_based_type_parameter_refs() {
        let decoded = decode_module_class_with_resolvers(
            &fixture_bytes("scala3-library/scala/Some.tasty"),
            &BinaryName::from_internal("scala/Some$"),
            |_, _| None,
            |package, simple_name| {
                Some(BinaryName::from_internal(format!(
                    "{package}/{simple_name}"
                )))
            },
        )
        .expect("pinned Scala 3.9 Some TASTy should decode");
        let apply = decoded
            .methods
            .iter()
            .find(|method| method.name == "apply")
            .unwrap_or_else(|| {
                panic!(
                    "Some.apply should be decoded; methods: {:?}",
                    decoded
                        .methods
                        .iter()
                        .map(|method| &method.name)
                        .collect::<Vec<_>>()
                )
            });

        assert_eq!(apply.type_parameters.len(), 1);
        assert_eq!(apply.type_parameters[0].name, "A");
        assert!(apply.type_parameters[0].bounds_supported);
        assert!(matches!(
            apply.type_parameters[0].upper_bound.as_ref(),
            Some(DecodedTastyType::Named(name)) if name.as_internal() == "scala/Any"
        ));
        assert!(
            matches!(
                apply.parameters[0].declared_type.as_ref(),
                Some(DecodedTastyType::ParamRef(0))
            ),
            "unexpected Some.apply parameter type: {:?}",
            apply.parameters[0]
        );
        assert!(matches!(
            apply.return_type.as_ref(),
            Some(DecodedTastyType::Applied { tycon, arguments })
                if matches!(tycon.as_ref(), DecodedTastyType::Named(name)
                    if name.as_internal() == "scala/Some")
                    && matches!(arguments.as_slice(), [DecodedTastyType::ParamRef(0)])
        ));
    }

    #[test]
    fn decodes_scala39_function1_apply_class_parameter_references() {
        let decoded = decode(
            &fixture_bytes("scala3-library/scala/Function1.tasty"),
            &BinaryName::from_internal("scala/Function1"),
        )
        .expect("pinned Scala 3.9 Function1 TASTy should decode");
        let apply = decoded
            .methods
            .iter()
            .find(|method| method.name == "apply")
            .expect("Function1.apply should be decoded");

        assert_eq!(apply.parameters.len(), 1);
        assert!(matches!(
            apply.parameters[0].declared_type,
            Some(DecodedTastyType::ClassParamRef(0))
        ));
        assert!(matches!(
            apply.return_type,
            Some(DecodedTastyType::ClassParamRef(1))
        ));
    }

    #[test]
    fn rejects_a_single_child_typebounds_alias_in_a_method_type_parameter() {
        let bytes = fixture_bytes("scala3-library/scala/Some.tasty");
        let file = TastyFile::parse(&bytes).expect("pinned Some TASTy should parse");
        let index = file.ast_address_index().unwrap();
        let parameter_node = index
            .iter()
            .find(|node| node.tag == dotty_tasty::tasty::TYPEPARAM_TAG)
            .expect("Some.apply should have a method type parameter")
            .decode_parameter()
            .unwrap();
        let body = parameter_node.decode_body().unwrap();
        let bounds_tree = match &body.type_tree {
            RawTree::Leaf(term) if term.tag == SHAREDTYPE_TAG => decode_shared_tree(
                &file,
                term.ast_ref()
                    .expect("shared type has an AST reference")
                    .address,
            )
            .expect("shared bounds should resolve"),
            other => other.clone(),
        };
        let StructuredTree::Length(StructuredNode::TypeBounds(mut bounds)) =
            bounds_tree.decode_structured().unwrap()
        else {
            panic!("Some.apply type parameter should have TYPEBOUNDS")
        };

        // In TYPEBOUNDS, one child encodes AliasingBounds. Removing the high
        // child from the pinned Nothing .. Any bounds gives that wire shape.
        bounds.high = None;
        let mut writer = Writer::new();
        bounds.encode(&mut writer).unwrap();
        let encoded = writer.into_inner();
        let parameter = ParameterNode::TypeParam {
            name: parameter_node.name(),
            body: &encoded,
            payload: &encoded,
            offset: 0,
        };

        let decoded = decode_tasty_type_parameter(
            &file,
            &parameter,
            "scala",
            &HashMap::new(),
            &HashMap::new(),
            &mut |_, _| None,
        )
        .expect("type parameter shape should be retained");

        assert!(!decoded.bounds_supported);
        assert_eq!(decoded.lower_bound, None);
        assert_eq!(decoded.upper_bound, None);
    }
}
