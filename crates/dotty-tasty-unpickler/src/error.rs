use std::fmt;

use dotty_core::ids::{SymbolId, TypeId};
use dotty_core::names::Namespace;
use dotty_core::resolution::ResolutionError;
use dotty_core::types::TypeRebindError;
use dotty_tasty::tasty::{AstError, TastyFileError};

/// Why semantic unpickling of a TASTy file failed.
///
/// Malformed or unsupported TASTy input is always reported through this type;
/// the unpickler never lowers an unsupported semantic shape to a placeholder.
/// Variants are introduced only when an unpickling pass needs them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnpickleError {
    /// The TASTy file itself failed a structural query, such as building the
    /// AST address index.
    Tasty(TastyFileError),
    /// An AST node could not be decoded into its structured form.
    Ast(AstError),
    /// A second symbol was entered for a definition address that already has
    /// one, which would break the address-identity invariant.
    DuplicateDefinition { address: u32 },
    /// A second declaration scope was entered for a symbol that already owns
    /// one.
    DuplicateScope { symbol: SymbolId },
    /// A name reference does not resolve to a name-table entry, or resolves
    /// through an unreasonably deep (possibly cyclic) chain of entries.
    InvalidNameReference { reference: u32 },
    /// A name-table entry with a tag this unpickler does not interpret.
    UnsupportedName { reference: u32 },
    /// The qualifier of a `private[X]` / `protected[X]` modifier has a tree
    /// shape this unpickler does not read. `tag` is the qualifier node's tag.
    /// Package names and references to enclosing definitions are supported.
    UnsupportedQualifier { tag: u8 },
    /// A reference points at an address that is not the start of a visible AST
    /// node (out of range, or inside another node's payload), or at one that
    /// has no entered symbol. `from` is the referring definition.
    InvalidReferenceTarget { from: u32, to: u32 },
    /// The qualifier of a `private[Q]` / `protected[Q]` modifier at
    /// `definition` is not a package, class, trait or object that encloses
    /// the qualified definition (or is the definition itself).
    InvalidQualifier { definition: u32 },
    /// A `PACKAGE` node whose path is neither a direct package reference
    /// (`TERMREFpkg`) nor a `SHAREDtype` link to one, which are the only forms
    /// this unpickler reads.
    UnsupportedPackagePath { address: u32 },
    /// No definition node exists at an address the tree walk expected one.
    MissingDefinition { address: u32 },
    /// A second `TypeId` was recorded for a type-node address that already
    /// has one, which would break the address-identity invariant.
    DuplicateType { address: u32 },
    /// The type node at `address` has a tag the type pass does not decode
    /// yet (a later increment) or that is not a type at all. The node is
    /// never lowered to a placeholder type.
    UnsupportedType { tag: u8, address: u32 },
    /// The type node at `from` refers to the definition at `to`, which is a
    /// visible AST node but has no symbol entered by pass 1 (for example a
    /// local definition, or a definition in another unit).
    MissingReferencedSymbol { from: u32, to: u32 },
    /// The type node at `from` refers to the definition at `to`, which has an
    /// entered symbol of the wrong kind: a `TYPEREF*` must name a type-namespace
    /// symbol, a `TERMREF*` a term-namespace one, and `THIS` a class.
    InvalidReferenceKind { from: u32, to: u32 },
    /// The `TYPEREFpkg` / `TERMREFpkg` node at `address` names a package
    /// that has not been entered into the package registry. Resolving
    /// packages outside the entered units belongs to the future resolver.
    UnresolvedPackage { address: u32, package: String },
    /// The name-based reference at `address` is well formed and its prefix is
    /// understood, but neither the entered state nor the resolver holds a
    /// member `name` of `namespace` in it. This is not evidence the member
    /// does not exist.
    UnresolvedMember {
        address: u32,
        prefix: TypeId,
        name: String,
        namespace: Namespace,
    },
    /// The prefix holds `candidates` symbols under the requested name and
    /// namespace (overloads), and the reference does not say which. Never
    /// resolved by taking the first.
    AmbiguousMember {
        address: u32,
        prefix: TypeId,
        name: String,
        candidates: usize,
    },
    /// The term reference at `address` carries a signature (`SIGNED` /
    /// `TARGETSIGNED`), and selecting a member by signature is not
    /// implemented. The signature is neither stripped nor ignored.
    UnsupportedSignedReference { address: u32, name: String },
    /// The compound type node at `address` has a shape the semantic model
    /// cannot express: its indexed children disagree with its wire shape.
    MalformedType { address: u32, reason: &'static str },
    /// The `TYPEBOUNDS` node at `address` carries variance markers, but the
    /// bound they apply to, `target`, is not a `TypeLambda`. Dotty applies
    /// markers only to an `HKTypeLambda`; they are never dropped or attached
    /// to anything else.
    InvalidBoundsVarianceTarget { address: u32, target: TypeId },
    /// The `TYPEBOUNDS` node at `address` carries `actual` variance markers,
    /// but the lambda `lambda` it applies to has `expected` parameters.
    BoundsVarianceArityMismatch {
        address: u32,
        lambda: TypeId,
        expected: usize,
        actual: usize,
    },
    /// Rebinding the lambda under the `TYPEBOUNDS` at `address` failed.
    RebindFailed {
        address: u32,
        error: TypeRebindError,
    },
    /// The `METHODtype` at `address` ends with a modifier, `tag`, that a method
    /// type does not use (only `IMPLICIT` and `GIVEN` do).
    InvalidMethodModifier { address: u32, tag: u8 },
    /// The `PARAMtype` at `from` names `binder`, which is not the start of a
    /// visible AST node.
    InvalidBinderReference { from: u32, binder: u32 },
    /// The `PARAMtype` at `from` names a node whose type, `binder`, is not a
    /// binder (a `TypeLambda`, `Poly` or `Method`).
    InvalidBinderKind { from: u32, binder: TypeId },
    /// The `PARAMtype` at `address` names parameter `index` of `binder`, which
    /// has only `arity` parameters.
    InvalidParameterIndex {
        address: u32,
        binder: TypeId,
        index: u32,
        arity: usize,
    },
    /// Parameter `index` of `binder` has an info, `bounds`, that is not a
    /// bounds type (`Bounds` or `AliasingBounds`).
    InvalidTypeParameterBounds {
        binder: TypeId,
        index: u32,
        bounds: TypeId,
    },
    /// The prefix of the name-based reference at `address` is a form whose
    /// members cannot be looked up here, and the resolver did not know it
    /// either.
    UnsupportedResolutionPrefix { address: u32, prefix: TypeId },
    /// The resolver failed on the reference at `address`, or answered with a
    /// symbol that cannot be the one requested.
    ResolverFailure {
        address: u32,
        error: ResolutionError,
    },
}

impl fmt::Display for UnpickleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Tasty(error) => write!(formatter, "invalid TASTy file: {error}"),
            Self::Ast(error) => write!(formatter, "invalid TASTy AST node: {error}"),
            Self::DuplicateDefinition { address } => write!(
                formatter,
                "a symbol was already entered for the definition at address {address}"
            ),
            Self::InvalidNameReference { reference } => {
                write!(formatter, "invalid name reference {reference}")
            }
            Self::UnsupportedName { reference } => {
                write!(formatter, "unsupported name entry at reference {reference}")
            }
            Self::UnsupportedQualifier { tag } => write!(
                formatter,
                "access qualifier with tag {tag} is not a package or enclosing definition"
            ),
            Self::InvalidReferenceTarget { from, to } => write!(
                formatter,
                "definition at address {from} refers to address {to}, which is not a valid target"
            ),
            Self::InvalidQualifier { definition } => write!(
                formatter,
                "the access qualifier of the definition at address {definition} does not enclose it"
            ),
            Self::UnsupportedPackagePath { address } => write!(
                formatter,
                "package at address {address} has an unsupported path form"
            ),
            Self::MissingDefinition { address } => {
                write!(formatter, "no definition node at address {address}")
            }
            Self::DuplicateType { address } => write!(
                formatter,
                "a type was already recorded for the type node at address {address}"
            ),
            Self::UnsupportedType { tag, address } => write!(
                formatter,
                "the type node at address {address} has tag {tag}, which is not decoded yet"
            ),
            Self::MissingReferencedSymbol { from, to } => write!(
                formatter,
                "the type node at address {from} refers to address {to}, which has no entered symbol"
            ),
            Self::InvalidReferenceKind { from, to } => write!(
                formatter,
                "the type node at address {from} refers to the definition at address {to}, which is the wrong kind of symbol for it"
            ),
            Self::UnresolvedPackage { address, package } => write!(
                formatter,
                "the package reference at address {address} names package `{package}`, which has not been entered"
            ),
            Self::UnresolvedMember {
                address,
                name,
                namespace,
                ..
            } => write!(
                formatter,
                "the reference at address {address} names {namespace:?} member `{name}`, which is not in its prefix"
            ),
            Self::AmbiguousMember {
                address,
                name,
                candidates,
                ..
            } => write!(
                formatter,
                "the reference at address {address} names `{name}`, which has {candidates} candidates in its prefix"
            ),
            Self::UnsupportedSignedReference { address, name } => write!(
                formatter,
                "the reference at address {address} to `{name}` carries a signature, which is not supported yet"
            ),
            Self::MalformedType { address, reason } => write!(
                formatter,
                "the type at address {address} is malformed: {reason}"
            ),
            Self::InvalidBoundsVarianceTarget { address, .. } => write!(
                formatter,
                "the bounds at address {address} carry variance markers, but the bound is not a type lambda"
            ),
            Self::BoundsVarianceArityMismatch {
                address,
                expected,
                actual,
                ..
            } => write!(
                formatter,
                "the bounds at address {address} carry {actual} variance markers for a lambda with {expected} parameters"
            ),
            Self::RebindFailed { address, error } => write!(
                formatter,
                "the variances of the bounds at address {address} could not be applied: {error}"
            ),
            Self::InvalidMethodModifier { address, tag } => write!(
                formatter,
                "the method type at address {address} carries modifier {tag}, which a method type does not use"
            ),
            Self::InvalidBinderReference { from, binder } => write!(
                formatter,
                "the parameter type at address {from} names address {binder}, which is not a node"
            ),
            Self::InvalidBinderKind { from, .. } => write!(
                formatter,
                "the parameter type at address {from} names a type that is not a binder"
            ),
            Self::InvalidParameterIndex {
                address,
                index,
                arity,
                ..
            } => write!(
                formatter,
                "the parameter type at address {address} names parameter {index} of a binder with {arity} parameters"
            ),
            Self::InvalidTypeParameterBounds { index, .. } => write!(
                formatter,
                "the info of type parameter {index} is not a bounds type"
            ),
            Self::UnsupportedResolutionPrefix { address, .. } => write!(
                formatter,
                "the reference at address {address} has a prefix whose members cannot be looked up"
            ),
            Self::ResolverFailure { address, error } => write!(
                formatter,
                "the resolver failed on the reference at address {address}: {error}"
            ),
            Self::DuplicateScope { symbol } => write!(
                formatter,
                "a declaration scope was already entered for symbol {}",
                symbol.index()
            ),
        }
    }
}

impl std::error::Error for UnpickleError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Tasty(error) => Some(error),
            Self::Ast(error) => Some(error),
            Self::RebindFailed { error, .. } => Some(error),
            Self::DuplicateDefinition { .. }
            | Self::DuplicateScope { .. }
            | Self::InvalidNameReference { .. }
            | Self::UnsupportedName { .. }
            | Self::UnsupportedQualifier { .. }
            | Self::InvalidReferenceTarget { .. }
            | Self::InvalidQualifier { .. }
            | Self::UnsupportedPackagePath { .. }
            | Self::MissingDefinition { .. }
            | Self::DuplicateType { .. }
            | Self::UnsupportedType { .. }
            | Self::MissingReferencedSymbol { .. }
            | Self::InvalidReferenceKind { .. }
            | Self::UnresolvedPackage { .. }
            | Self::UnresolvedMember { .. }
            | Self::AmbiguousMember { .. }
            | Self::UnsupportedSignedReference { .. }
            | Self::MalformedType { .. }
            | Self::InvalidBoundsVarianceTarget { .. }
            | Self::BoundsVarianceArityMismatch { .. }
            | Self::InvalidMethodModifier { .. }
            | Self::InvalidBinderReference { .. }
            | Self::InvalidBinderKind { .. }
            | Self::InvalidParameterIndex { .. }
            | Self::InvalidTypeParameterBounds { .. }
            | Self::UnsupportedResolutionPrefix { .. }
            | Self::ResolverFailure { .. } => None,
        }
    }
}

impl From<TastyFileError> for UnpickleError {
    fn from(error: TastyFileError) -> Self {
        Self::Tasty(error)
    }
}

impl From<AstError> for UnpickleError {
    fn from(error: AstError) -> Self {
        Self::Ast(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dotty_tasty::tasty::HeaderError;

    #[test]
    fn tasty_file_errors_convert_and_expose_their_source() {
        let source = TastyFileError::Header(HeaderError::InvalidMagic {
            actual: [0, 0, 0, 0],
        });
        let error = UnpickleError::from(source.clone());

        assert_eq!(error, UnpickleError::Tasty(source));
        assert!(std::error::Error::source(&error).is_some());
        assert!(error.to_string().starts_with("invalid TASTy file"));
    }

    #[test]
    fn duplicate_definition_names_the_address_and_has_no_source() {
        let error = UnpickleError::DuplicateDefinition { address: 46 };

        assert!(error.to_string().contains("address 46"));
        assert!(std::error::Error::source(&error).is_none());
    }

    #[test]
    fn ast_errors_convert_and_expose_their_source() {
        let source = AstError::InvalidTag { tag: 1, offset: 7 };
        let error = UnpickleError::from(source.clone());

        assert_eq!(error, UnpickleError::Ast(source));
        assert!(std::error::Error::source(&error).is_some());
        assert!(error.to_string().starts_with("invalid TASTy AST node"));
    }
}
