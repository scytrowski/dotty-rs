use crate::binary_name::BinaryName;
use dotty_classfile::access_flags::{
    ACC_ABSTRACT, ACC_FINAL, ACC_INTERFACE, ACC_PRIVATE, ACC_PROTECTED, ACC_PUBLIC, ACC_STATIC,
    ACC_SYNTHETIC, ClassAccessFlags, FieldAccessFlags, MethodAccessFlags,
};
use dotty_tasty::tasty::{
    ABSTRACT_TAG, APPLIEDTPT_TAG, APPLIEDTYPE_TAG, ARTIFACT_TAG, AppliedTypeNode, AstError,
    CASEACCESSOR_TAG, DEFDEF_TAG, DefDefBody, DefinitionBody, DefinitionTail, FIELDACCESSOR_TAG,
    FINAL_TAG, IdentNode, LOCAL_TAG, MUTABLE_TAG, PRIVATE_TAG, PRIVATEQUALIFIED_TAG, PROTECTED_TAG,
    PROTECTEDQUALIFIED_TAG, ParameterNode, RawName, RawTree, Reader, ReferenceNode, SHAREDTERM_TAG,
    SHAREDTYPE_TAG, STATIC_TAG, SYNTHETIC_TAG, SelectNode, StandardSection, StructuredNode,
    StructuredTree, TEMPLATE_TAG, TERMREF_TAG, TERMREFPKG_TAG, TERMREFSYMBOL_TAG, TRAIT_TAG,
    TYPEDEF_TAG, TYPEREF_TAG, TYPEREFPKG_TAG, TYPEREFSYMBOL_TAG, TastyFile, TastyFileError,
    TermValue, VALDEF_TAG,
};
use std::collections::HashSet;
use std::fmt;

/// The class-level facts reconstructable from a `.tasty` file without full
/// type-checking: enough to enter a `dotty-core` `Symbol` and recurse into
/// its supertypes, mirroring what `.class` decoding gives
/// (`docs/classloader.md` §9).
///
/// `fields`/`methods` cover ordinary `ValDef`/`DefDef` template members;
/// each member's own declared type resolves to a name the same
/// best-effort way a supertype does (see `resolve_parent_name`) — a
/// type shape this decoder does not recognize (anything past a plain or
/// generic class reference: tuples, function types, refinements,
/// dependent/path types, ...) is simply `None`, not a decode failure, so
/// the member itself is still reconstructed with an unknown type rather
/// than dropped or aborting the whole class. A member's own type
/// parameters (a generic method or a class's own type/val parameters
/// beyond a plain reference) are not modeled — see
/// [`decode_members`]'s doc comment. `signature`/`nest_host`/etc. (JVM
/// classfile-only concepts) are not reconstructed from `.tasty` at all,
/// so a `.tasty`-backed symbol still gets no [`crate::symbol::ClassfileMetadata`]
/// entry.
#[derive(Debug)]
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
#[derive(Debug)]
pub(crate) struct DecodedTastyField {
    pub name: String,
    pub flags: FieldAccessFlags,
    /// `None` when [`resolve_parent_name`] can't reduce the field's
    /// declared type tree to a name — see [`DecodedTastyClass`]'s doc
    /// comment.
    pub declared_type: Option<BinaryName>,
}

/// One `DefDef` reconstructed from a class's `Template.stats`.
#[derive(Debug)]
pub(crate) struct DecodedTastyMethod {
    pub name: String,
    pub flags: MethodAccessFlags,
    /// Every term parameter across every clause, flattened into one list
    /// — real Scala erasure already compiles a curried method
    /// (`def f(x: Int)(y: Int)`) down to one JVM method with every
    /// parameter concatenated, so this matches what a `.class`-loaded
    /// version of the same method would show. A parameter clause's own
    /// type parameters are skipped (generics are not modeled yet).
    pub parameters: Vec<DecodedTastyParameter>,
    /// `.tasty` always encodes an explicit return type tree, even for a
    /// `Unit`-returning method (`Unit` is an ordinary class reference in
    /// Scala's type system, not a separate "void" encoding the way a
    /// `.class` descriptor has one) — so `None` here means only "this
    /// decoder could not reduce the tree to a name", per
    /// [`DecodedTastyClass`]'s doc comment, exactly like an unresolvable
    /// field/parameter type; it is never a stand-in for `Unit`.
    pub return_type: Option<BinaryName>,
}

/// One term parameter of a [`DecodedTastyMethod`].
#[derive(Debug)]
pub(crate) struct DecodedTastyParameter {
    pub name: String,
    pub declared_type: Option<BinaryName>,
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
    /// A supertype after the first `Template` parent (i.e. a mixin
    /// interface, not the implicit superclass slot) has no name this
    /// decoder can resolve — see `resolve_parent_name`'s doc comment
    /// for exactly which shapes are and are not resolved.
    UnresolvedSupertype,
}

impl fmt::Display for TastyDecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Parse(source) => write!(formatter, "failed to parse .tasty file: {source}"),
            Self::Ast(source) => write!(formatter, "failed to decode .tasty AST: {source}"),
            Self::MissingTypeDef => {
                write!(formatter, "no matching TypeDef found in .tasty file")
            }
            Self::NoTemplateBody => write!(formatter, "TypeDef has no Template body"),
            Self::UnresolvedSupertype => {
                write!(
                    formatter,
                    "a supertype reference could not be resolved to a name"
                )
            }
        }
    }
}

impl std::error::Error for TastyDecodeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Parse(source) => Some(source),
            Self::Ast(source) => Some(source),
            Self::MissingTypeDef | Self::NoTemplateBody | Self::UnresolvedSupertype => None,
        }
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
/// [`resolve_parent_name`]'s doc comment for why that's the right
/// context to use.
#[cfg(test)]
pub(crate) fn decode(
    bytes: &[u8],
    requested: &BinaryName,
) -> Result<DecodedTastyClass, TastyDecodeError> {
    decode_with_alias_resolver(bytes, requested, |_, _| None)
}

/// Decodes a class while allowing the classpath owner to resolve a named
/// alias when an otherwise real TASTy reference has a class prefix this
/// structural decoder cannot follow (for example `scala.package$.Iterator`).
pub(crate) fn decode_with_alias_resolver(
    bytes: &[u8],
    requested: &BinaryName,
    mut resolve_alias: impl FnMut(&BinaryName, &str) -> Option<BinaryName>,
) -> Result<DecodedTastyClass, TastyDecodeError> {
    let file = TastyFile::parse_scala_3_9(bytes)?;
    let index = file.ast_address_index()?;
    let requested_internal = requested.as_internal();
    let simple_name = requested_internal
        .rsplit('/')
        .next()
        .unwrap_or(requested_internal);
    let package = match requested_internal.rsplit_once('/') {
        Some((package, _)) => package,
        None => "",
    };

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

    let mut parents = template.parents.iter();
    let super_class = match parents.next() {
        Some(parent) => resolve_parent_name_with_alias(&file, parent, package, &mut resolve_alias)
            .unwrap_or_else(|| BinaryName::from_internal("java/lang/Object")),
        None => BinaryName::from_internal("java/lang/Object"),
    };

    let mut interfaces = Vec::new();
    for parent in parents {
        interfaces.push(
            resolve_parent_name_with_alias(&file, parent, package, &mut resolve_alias)
                .ok_or(TastyDecodeError::UnresolvedSupertype)?,
        );
    }

    let mut fields = decode_constructor_accessor_fields(&file, &template.term_params, package)?;
    let (stats_fields, methods) = decode_members(&file, &template.stats, package)?;
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
            declared_type: resolve_parent_name(file, &body.type_tree, package),
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
/// A `DefDef`/`ValDef`'s own type parameters (a generic method, or a
/// `TypeParam` entry in its parameter list) are not modeled — every
/// `ParameterNode::TypeParam` is skipped, matching `.class` loading not
/// yet lowering `.tasty` generics onto `Type::Poly`/`Type::TypeLambda`
/// the way a JVM `Signature` attribute is (`docs/classloader.md` §9).
fn decode_members(
    file: &TastyFile<'_>,
    stats: &[RawTree<'_>],
    package: &str,
) -> Result<(Vec<DecodedTastyField>, Vec<DecodedTastyMethod>), TastyDecodeError> {
    let mut fields = Vec::new();
    let mut methods = Vec::new();

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
                    declared_type: resolve_parent_name(file, &type_tree, package),
                });
            }
            DEFDEF_TAG => {
                let StructuredNode::DefDef(DefDefBody {
                    name,
                    parameters,
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
                    let declared_type = parameter
                        .decode_body()
                        .ok()
                        .and_then(|body| resolve_parent_name(file, &body.type_tree, package));
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
                    resolve_parent_name(file, &return_type, package)
                };

                methods.push(DecodedTastyMethod {
                    name: method_name,
                    flags: decode_method_flags(&tail),
                    parameters: decoded_parameters,
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
    match file.names().entries().get(reference as usize)? {
        RawName::Utf8(text) => Some(text.clone()),
        RawName::Qualified { prefix, selector } => {
            let prefix = resolve_qualified_name(file, *prefix)?;
            let selector = resolve_qualified_name(file, *selector)?;
            Some(format!("{prefix}/{selector}"))
        }
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
            match current_package {
                Some(current)
                    if !current.is_empty() && !package.contains('/') && package != "_root_" =>
                {
                    Some(format!("{current}/{package}"))
                }
                _ => Some(package),
            }
        }
        RawTree::Leaf(term) if matches!(term.tag, SHAREDTERM_TAG | SHAREDTYPE_TAG) => {
            let reference = term.ast_ref()?;
            let shared = decode_shared_tree(file, reference.address)?;
            resolve_reference_prefix_at_depth(file, &shared, current_package, depth + 1)
        }
        _ => None,
    }
}

/// What [`resolve_reference_name`] learned about a tree.
///
/// A plain `Option<BinaryName>` cannot distinguish "this isn't a reference
/// shape I understand at all, try your own heuristic" from "this
/// unambiguously *is* a real, post-typecheck reference, but I can't name
/// its prefix" — and [`resolve_parent_name`] must treat those two cases
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
/// prefix/name-table semantics instead of [`resolve_parent_name`]'s
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
                    let name = match current_package {
                        Some(package) if !package.is_empty() && !name.contains('/') => {
                            format!("{package}/{name}")
                        }
                        _ => name,
                    };
                    ReferenceResolution::Resolved(BinaryName::from_internal(name))
                })
                .unwrap_or(ReferenceResolution::Unresolved)
        }
        StructuredTree::Ident(IdentNode { type_tree, .. }) => {
            resolve_reference_name_at_depth(file, &type_tree, current_package, depth + 1)
        }
        _ => ReferenceResolution::NotAReference,
    }
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
fn resolve_parent_name(
    file: &TastyFile<'_>,
    parent: &RawTree<'_>,
    package: &str,
) -> Option<BinaryName> {
    resolve_parent_name_in_package(file, parent, package, None)
}

fn resolve_parent_name_in_package(
    file: &TastyFile<'_>,
    parent: &RawTree<'_>,
    package: &str,
    package_scope: Option<&str>,
) -> Option<BinaryName> {
    let reference = match package_scope {
        Some(_) => resolve_reference_name_in_package(file, parent, package_scope),
        None => resolve_reference_name(file, parent),
    };
    match reference {
        ReferenceResolution::Resolved(resolved) => return Some(resolved),
        // A real reference this decoder cannot fully qualify (an
        // implicit-import reference such as `scala.package$.Iterator`,
        // see `ReferenceResolution`'s own doc comment) must not fall
        // through to the heuristic below and come back out as a
        // misleading same-package guess.
        ReferenceResolution::Unresolved => return None,
        ReferenceResolution::NotAReference => {}
    }

    let mut names: Vec<String> = parent
        .name_refs()
        .into_iter()
        .filter_map(|reference| wire_name(file, reference))
        .collect();

    if names.is_empty() {
        return match package_scope {
            Some(_) => resolve_applied_type_name_in_package(file, parent, package, package_scope),
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

    let joined = names.join("/");
    let qualified = if already_qualified || package.is_empty() {
        joined
    } else {
        format!("{package}/{joined}")
    };
    Some(BinaryName::from_internal(qualified))
}

fn resolve_parent_name_with_alias(
    file: &TastyFile<'_>,
    parent: &RawTree<'_>,
    package: &str,
    resolve_alias: &mut impl FnMut(&BinaryName, &str) -> Option<BinaryName>,
) -> Option<BinaryName> {
    let package_alias = unresolved_alias_reference(file, parent)
        .and_then(|(owner, alias)| resolve_alias(&owner, &alias));
    package_alias.or_else(|| resolve_parent_name(file, parent, package))
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
        let candidate =
            resolve_parent_name_in_package(file, &rhs, current_package, Some(current_package));
        return Ok(candidate.map(TypeAliasTarget::Candidate));
    }
    Ok(None)
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

#[cfg(test)]
mod tests {
    use super::*;
    use dotty_tasty::tasty::{AstChildNode, SimpleTerm};

    fn fixture_bytes(relative_path: &str) -> Vec<u8> {
        std::fs::read(
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../dotty-tasty/tests/fixtures")
                .join(relative_path),
        )
        .unwrap_or_else(|error| panic!("fixture {relative_path} should exist: {error}"))
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
        assert_eq!(
            resolve_symbol_address(&file, address).as_deref(),
            Some("Ordering$Reverse")
        );
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
    /// [`resolve_parent_name`]'s flatten-based fallback, still covered
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
    /// structural decoder alone cannot resolve this alias, so this
    /// decode-only entry point still reports `UnresolvedSupertype`; the
    /// classloader's classpath-aware entry point follows the alias and
    /// validates its target.
    #[test]
    fn decoding_fails_when_a_generic_mixins_tycon_is_an_unresolvable_implicit_import_reference() {
        let error = decode(
            &fixture_bytes("scala3-library/scala/io/Source.tasty"),
            &BinaryName::from_internal("Source"),
        )
        .unwrap_err();

        assert!(matches!(error, TastyDecodeError::UnresolvedSupertype));
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

        assert!(matches!(error, TastyDecodeError::UnresolvedSupertype));
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
}
