use crate::binary_name::BinaryName;
use dotty_classfile::access_flags::{
    ACC_ABSTRACT, ACC_FINAL, ACC_INTERFACE, ACC_PRIVATE, ACC_PROTECTED, ACC_PUBLIC, ACC_STATIC,
    ACC_SYNTHETIC, ClassAccessFlags, FieldAccessFlags, MethodAccessFlags,
};
use dotty_tasty::tasty::{
    ABSTRACT_TAG, APPLIEDTPT_TAG, APPLIEDTYPE_TAG, ARTIFACT_TAG, AppliedTypeNode, AstError,
    CASEACCESSOR_TAG, DEFDEF_TAG, DefDefBody, DefinitionBody, DefinitionTail, FIELDACCESSOR_TAG,
    FINAL_TAG, MUTABLE_TAG, PRIVATE_TAG, PROTECTED_TAG, ParameterNode, RawName, RawTree,
    STATIC_TAG, SYNTHETIC_TAG, StructuredNode, TEMPLATE_TAG, TRAIT_TAG, TYPEDEF_TAG, TastyFile,
    TastyFileError, VALDEF_TAG,
};
use std::fmt;

/// The class-level facts reconstructable from a `.tasty` file without full
/// type-checking: enough to enter a `dotty-core` `Symbol` and recurse into
/// its supertypes, mirroring what `.class` decoding gives
/// (`docs/classloader.md` §9).
///
/// `fields`/`methods` cover ordinary `ValDef`/`DefDef` template members;
/// each member's own declared type resolves to a name the same
/// best-effort way a supertype does (see [`resolve_parent_name`]) — a
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
    pub super_class: BinaryName,
    pub interfaces: Vec<BinaryName>,
    pub fields: Vec<DecodedTastyField>,
    pub methods: Vec<DecodedTastyMethod>,
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

/// Why decoding a `.tasty` file into a [`DecodedTastyClass`] failed.
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
    /// decoder can resolve — see [`resolve_parent_name`]'s doc comment
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
pub(crate) fn decode(
    bytes: &[u8],
    requested: &BinaryName,
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
        Some(parent) => resolve_parent_name(&file, parent, package)
            .unwrap_or_else(|| BinaryName::from_internal("java/lang/Object")),
        None => BinaryName::from_internal("java/lang/Object"),
    };

    let mut interfaces = Vec::new();
    for parent in parents {
        interfaces.push(
            resolve_parent_name(&file, parent, package)
                .ok_or(TastyDecodeError::UnresolvedSupertype)?,
        );
    }

    let mut fields = decode_constructor_accessor_fields(&file, &template.term_params, package)?;
    let (stats_fields, methods) = decode_members(&file, &template.stats, package)?;
    fields.extend(stats_fields);

    Ok(DecodedTastyClass {
        flags,
        super_class,
        interfaces,
        fields,
        methods,
    })
}

/// Reconstructs a field for every primary-constructor term parameter
/// tagged [`FIELDACCESSOR_TAG`] or [`CASEACCESSOR_TAG`] — a `class C(val
/// x: Int)`/`case class C(x: Int)` parameter that is also a real field.
///
/// A `val`/`var`-less constructor parameter (plain `class C(x: Int)`)
/// carries neither tag and is skipped: it is only ever a constructor-local
/// binding at the source level, even though real Scala bytecode happens
/// to also retain it as a private synthetic JVM field to support later
/// use inside the class body — an implementation detail this reconstructs
/// the *source-level* member set, not the compiled one, so it stays
/// unmodeled, mirroring how `.class` loading does not surface a
/// JVM-only synthetic field as a "real" member either.
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
        let is_accessor = body.tail.iter().any(|item| {
            matches!(
                item,
                DefinitionTail::Modifier(FIELDACCESSOR_TAG | CASEACCESSOR_TAG)
            )
        });
        if !is_accessor {
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

/// Reconstructs a supertype parent's referenced [`BinaryName`], or
/// `None` when it can't be — either because the parent carries no name
/// reference at all (the implicit `Object`/`AnyRef` superclass
/// constructor call, which `.tasty` encodes without any name-table
/// entry, JVMS §4.1's implicit interface superclass follows the same
/// shape), or because every name reference it does carry fails to
/// render as a plain name.
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
    let mut names: Vec<String> = parent
        .name_refs()
        .into_iter()
        .filter_map(|reference| wire_name(file, reference))
        .collect();

    if names.is_empty() {
        return resolve_applied_type_name(file, parent, package);
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

    resolve_parent_name(file, &tycon, package)
}

/// Reads a raw AST name-table reference directly rather than through
/// [`TastyFile::render_name`]: AST fields such as [`RawTree::name_refs`]
/// and a definition's own `name()` are zero-based direct indices into
/// the wire name table, not the one-based [`dotty_tasty::tasty::NameRef`]
/// convention `render_name`/`NameTable::get` expect (matching how
/// `dotty-tasty`'s own fixture tests read these same fields). Returns
/// `None` for anything other than a direct UTF-8 entry (a signature- or
/// symbol-shaped name-table entry, which this decoder does not need to
/// render).
fn wire_name(file: &TastyFile<'_>, reference: u32) -> Option<String> {
    file.names()
        .entries()
        .get(reference as usize)
        .and_then(RawName::as_utf8)
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_bytes(relative_path: &str) -> Vec<u8> {
        std::fs::read(
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../dotty-tasty/tests/fixtures")
                .join(relative_path),
        )
        .unwrap_or_else(|error| panic!("fixture {relative_path} should exist: {error}"))
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
            vec![BinaryName::from_internal("Animal")]
        );
    }

    /// `.tasty` `TypeDef`s carry no package of their own (`decode`'s own
    /// doc comment), so `Dog.tasty`'s bare `Animal` reference must be
    /// resolved relative to whatever package `Dog` was actually
    /// requested under — not left bare, and not the fixture directory's
    /// name. Requesting a name under a package no real fixture happens
    /// to live in still exercises exactly this: `decode` still finds
    /// the `Dog` `TypeDef` by simple name, and `Animal` still resolves,
    /// now qualified with that same package.
    #[test]
    fn qualifies_a_same_package_mixin_with_the_requested_classs_package() {
        let decoded = decode(
            &fixture_bytes("inheritance/Dog.tasty"),
            &BinaryName::from_internal("com/example/Dog"),
        )
        .unwrap();

        assert_eq!(
            decoded.interfaces,
            vec![BinaryName::from_internal("com/example/Animal")]
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
    /// exercise. Without [`resolve_applied_type_name`], this parent's
    /// `name_refs()` is empty (see its doc comment) and decoding fails
    /// with `UnresolvedSupertype`.
    #[test]
    fn decodes_a_class_with_a_generic_mixin_encoded_as_an_applied_type() {
        let decoded = decode(
            &fixture_bytes("scala3-library/scala/io/Source.tasty"),
            &BinaryName::from_internal("Source"),
        )
        .unwrap();

        assert!(decoded.flags.is_abstract());
        assert_eq!(decoded.super_class.as_internal(), "java/lang/Object");
        assert_eq!(
            decoded.interfaces,
            vec![
                BinaryName::from_internal("Iterator"),
                BinaryName::from_internal("Closeable"),
            ]
        );
    }

    /// `scala3-library/scala/math/BigInt.tasty`'s real `BigInt` mixes in
    /// `Ordered[BigInt]` alongside two plain (non-generic) mixins —
    /// covering an `AppliedType` mixin that isn't the only, or the
    /// first, non-superclass parent.
    #[test]
    fn decodes_a_final_class_with_a_generic_mixin_among_plain_ones() {
        let decoded = decode(
            &fixture_bytes("scala3-library/scala/math/BigInt.tasty"),
            &BinaryName::from_internal("BigInt"),
        )
        .unwrap();

        assert!(decoded.flags.is_final());
        assert_eq!(decoded.super_class.as_internal(), "java/lang/Object");
        assert_eq!(
            decoded.interfaces,
            vec![
                BinaryName::from_internal("ScalaNumericConversions"),
                BinaryName::from_internal("Serializable"),
                BinaryName::from_internal("Ordered"),
            ]
        );
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
        assert_eq!(decoded.super_class.as_internal(), "java/lang/Object");
        assert_eq!(
            decoded.interfaces,
            vec![BinaryName::from_internal("Ordering")]
        );
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
    /// `AppliedType` whose own `tycon` is a bare `TYPEREF` referencing
    /// its target purely by a prefix-relative name/symbol pair.
    /// [`RawTree::name_refs`] deliberately does not treat `TYPEREF`'s
    /// value as a visitable name the way it does `IDENT`/`IDENTTPT` (it
    /// only whitelists tags for genuine source-level identifier
    /// occurrences — see `visit_name_refs`'s match arms in
    /// `dotty-tasty`), so [`resolve_applied_type_name`] correctly gives
    /// up here rather than guessing. A real, disclosed limitation, not
    /// a bug: this asserts it surfaces as `UnresolvedSupertype`, not a
    /// silently dropped interface or a panic.
    #[test]
    fn unresolved_supertype_when_a_generic_mixins_tycon_is_a_bare_typeref() {
        let error = decode(
            &fixture_bytes("scala3-library/scala/concurrent/impl/CompletionLatch.tasty"),
            &BinaryName::from_internal("CompletionLatch"),
        )
        .unwrap_err();

        assert!(matches!(error, TastyDecodeError::UnresolvedSupertype));
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
