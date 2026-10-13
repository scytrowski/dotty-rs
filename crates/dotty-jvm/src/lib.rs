//! A JVM backend adapter boundary for already-typed compiler semantics.
//!
//! This crate currently defines and validates a borrowed compilation plan. It
//! does not assemble a source frontend, infer identities from names, mutate
//! the caller's stores, or emit bytecode. See `docs/jvm-backend.md` for the
//! boundary and validation assumptions. Its first descriptor projection is
//! limited to primitive `Int`, `Boolean`, and result-only `Unit` methods.

use std::collections::HashSet;
use std::error::Error;
use std::fmt;

use dotty_core::ast::{Tree, Typed, TypedAst};
use dotty_core::definitions::Definitions;
use dotty_core::ids::{SymbolId, TreeId, TypeId};
use dotty_core::store::SemanticStore;
use dotty_core::symbols::{SymbolInfo, SymbolKind};
use dotty_core::types::Type;

mod descriptor;

/// JVMS limit for a method's parameter slots, including `this` for instance
/// methods.
pub const JVM_MAX_PARAMETER_SLOTS: usize = 255;

/// A JVM value representation supported by the current projection slice.
///
/// `Int` and `Boolean` have distinct descriptor letters (`I` and `Z`) but
/// both use the JVM's int-like category-one operand representation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JvmType {
    Int,
    Boolean,
}

/// Operand category for the JVM values currently supported here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JvmValueCategory {
    IntLike,
}

/// A JVM method result. `Void` is only valid as a result, never a parameter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JvmReturnType {
    Value(JvmType),
    Void,
}

/// Primitive JVM descriptor and value categories for one method signature.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JvmMethodDescriptor {
    descriptor: dotty_classfile::descriptor::MethodDescriptor,
    descriptor_text: String,
    /// Primitive parameters in declaration order.
    parameters: Vec<JvmType>,
    /// Primitive result, or JVM `void` for Scala `Unit`.
    result: JvmReturnType,
}

/// Borrowed input for validating one or more class plans.
///
/// The semantic store and typed AST remain owned by the caller. Plans retain
/// canonical symbol identities and roots in that exact AST arena.
pub struct JvmCompilationUnit<'a> {
    /// Canonical semantic symbols and types supplied by the frontend.
    pub store: &'a SemanticStore,
    /// Typed trees supplied by the frontend.
    pub typed_ast: &'a TypedAst,
    /// Classes in deterministic frontend order.
    pub classes: Vec<JvmClassPlan>,
}

/// One canonical class and the method bodies selected for backend work.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JvmClassPlan {
    /// Canonical class symbol in [`JvmCompilationUnit::store`].
    pub class: SymbolId,
    /// Methods in deterministic frontend order.
    pub methods: Vec<JvmMethodPlan>,
}

/// A method, its explicitly ordered parameter identities, and typed body root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JvmMethodPlan {
    /// Canonical method symbol in the semantic store.
    pub method: SymbolId,
    /// Parameter symbols in JVM/source order, supplied by the frontend.
    pub parameters: Vec<SymbolId>,
    /// Root of this method's body in the supplied typed AST arena.
    /// `None` represents a declaration without a body.
    pub body: Option<TreeId<Typed>>,
}

/// A plan failed validation against its caller-owned semantic input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JvmCodegenError {
    /// The class ID is outside the supplied symbol table.
    UnknownClass { class: SymbolId },
    /// The method ID is outside the supplied symbol table.
    UnknownMethod { method: SymbolId },
    /// The parameter ID is outside the supplied symbol table.
    UnknownParameter { parameter: SymbolId },
    /// A symbol has a kind that is not supported for its plan role.
    UnsupportedSymbolKind {
        symbol: SymbolId,
        expected: SymbolRole,
        found: SymbolKind,
    },
    /// A method or parameter is owned by a symbol other than the planned one.
    WrongOwner {
        symbol: SymbolId,
        expected: SymbolId,
        actual: Option<SymbolId>,
    },
    /// The same canonical method appears more than once in the unit.
    DuplicateMethodPlan { method: SymbolId },
    /// The same canonical class appears more than once in the unit.
    DuplicateClassPlan { class: SymbolId },
    /// The same parameter identity appears more than once in a method plan.
    DuplicateParameter {
        parameter: SymbolId,
        method: SymbolId,
    },
    /// A method plan does not name a typed body root.
    MissingTypedBody { method: SymbolId },
    /// A method body's typed root does not belong to the supplied arena.
    UnknownBodyRoot {
        method: SymbolId,
        tree: TreeId<Typed>,
    },
    /// A typed tree refers to a missing, reserved, or sentinel type.
    InvalidTreeType {
        tree: TreeId<Typed>,
        ty: TypeId,
        reason: InvalidTypeReason,
    },
    /// A semantic symbol has no completed semantic type.
    MissingSemanticInfo { symbol: SymbolId },
    /// A symbol's completed semantic type is missing, reserved, or a sentinel.
    InvalidSemanticType {
        symbol: SymbolId,
        ty: TypeId,
        reason: InvalidTypeReason,
    },
    /// A symbol carries `SymbolInfo::Error` and therefore has no usable type.
    ErrorSemanticInfo { symbol: SymbolId },
    /// A semantic node is outside a future emission subset.
    ///
    /// Validation does not currently choose an emission subset, so this is
    /// reserved for the code-generation stage that follows this contract.
    UnsupportedSemanticNode {
        tree: TreeId<Typed>,
        node: &'static str,
    },
    /// A method has an unsupported single-clause or monomorphism shape.
    UnsupportedMethodShape {
        method: SymbolId,
        ty: TypeId,
        kind: UnsupportedMethodShapeKind,
    },
    /// A supported primitive parameter's symbol type disagrees with its
    /// corresponding entry in the method's ordered `MethodType`.
    ParameterTypeMismatch {
        method: SymbolId,
        parameter: SymbolId,
        declared: TypeId,
        symbol_info: TypeId,
    },
    /// Scala `Unit` cannot appear in a JVM parameter list.
    UnitParameter {
        method: SymbolId,
        parameter: SymbolId,
        ty: TypeId,
    },
    /// An unsupported semantic type was used for a method value or result.
    UnsupportedJvmType {
        symbol: SymbolId,
        ty: TypeId,
        kind: UnsupportedJvmTypeKind,
    },
    /// A method descriptor exceeds the JVMS 255 parameter-slot limit.
    TooManyParameterSlots { method: SymbolId, slots: usize },
    /// A parameter modifier is not represented by this primitive-only slice.
    UnsupportedMethodParameter {
        method: SymbolId,
        index: usize,
        kind: UnsupportedMethodParameterKind,
    },
    /// An internally generated descriptor unexpectedly failed the classfile
    /// crate's parser.
    InvalidGeneratedDescriptor {
        method: SymbolId,
        error: dotty_classfile::descriptor::DescriptorError,
    },
    /// A supplied builtin type identity is malformed for this semantic store.
    InvalidBuiltinDefinition {
        builtin: PrimitiveBuiltin,
        ty: TypeId,
    },
}

/// Semantic role required by a compilation plan.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SymbolRole {
    Class,
    Method,
    Parameter,
}

/// Why a typed semantic slot cannot be consumed as an actual type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InvalidTypeReason {
    MissingOrUnfilled,
    NoType,
    Error,
    NoPrefix,
}

/// Unsupported method-type shape for this descriptor projection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnsupportedMethodShapeKind {
    Polymorphic,
    NestedCurriedClause,
    NonPlainClause,
    ParameterCountMismatch,
    NotMethodType,
}

/// Unsupported semantic type form in this descriptor projection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnsupportedJvmTypeKind {
    Alias,
    TypeParameter,
    MatchType,
    RefinedType,
    Union,
    Intersection,
    ScalaObject,
    ReferenceClass,
    NamedReference,
    TermReference,
    NestedMethod,
    Array,
    ByName,
    Repeated,
    Bounds,
    NonPrimitiveConstant,
    Other,
}

/// Primitive definition identity being read from [`Definitions`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrimitiveBuiltin {
    Int,
    Boolean,
    Unit,
}

/// An unsupported method parameter modifier.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnsupportedMethodParameterKind {
    Erased,
    Varargs,
}

impl fmt::Display for JvmCodegenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "JVM compilation input is invalid: {self:?}")
    }
}

impl Error for JvmCodegenError {}

impl JvmMethodPlan {
    /// Extracts this method's primitive-only JVM signature from canonical
    /// semantic types and this plan's explicitly ordered parameter symbols.
    ///
    /// The class, method, and parameter symbol kinds and ownership are checked
    /// before reading completed type information. The signature accepts one
    /// ordinary monomorphic parameter clause containing only `Int` and
    /// `Boolean`, with `Int`, `Boolean`, or result-only `Unit` as its result.
    pub fn descriptor(
        &self,
        store: &SemanticStore,
        owner: SymbolId,
        definitions: &Definitions,
    ) -> Result<JvmMethodDescriptor, JvmCodegenError> {
        descriptor::method_descriptor(self, store, owner, definitions)
    }
}

impl JvmCompilationUnit<'_> {
    /// Validates the plan without mutating or copying semantic state.
    ///
    /// The first error follows class order, method order, parameter order,
    /// then typed-arena allocation order. Symbol identity is always the
    /// canonical [`SymbolId`]; equal names never substitute for identity.
    pub fn validate(&self) -> Result<(), JvmCodegenError> {
        let mut seen_classes = HashSet::new();
        let mut seen_methods = HashSet::new();

        for class_plan in &self.classes {
            if !seen_classes.insert(class_plan.class) {
                return Err(JvmCodegenError::DuplicateClassPlan {
                    class: class_plan.class,
                });
            }

            let class = self
                .store
                .symbols
                .contains(class_plan.class)
                .then(|| self.store.symbols.get(class_plan.class))
                .ok_or(JvmCodegenError::UnknownClass {
                    class: class_plan.class,
                })?;
            if class.kind != SymbolKind::Class {
                return Err(JvmCodegenError::UnsupportedSymbolKind {
                    symbol: class_plan.class,
                    expected: SymbolRole::Class,
                    found: class.kind,
                });
            }
            self.validate_symbol_type(class_plan.class)?;

            for method_plan in &class_plan.methods {
                if !seen_methods.insert(method_plan.method) {
                    return Err(JvmCodegenError::DuplicateMethodPlan {
                        method: method_plan.method,
                    });
                }

                let method = self
                    .store
                    .symbols
                    .contains(method_plan.method)
                    .then(|| self.store.symbols.get(method_plan.method))
                    .ok_or(JvmCodegenError::UnknownMethod {
                        method: method_plan.method,
                    })?;
                if method.kind != SymbolKind::Method {
                    return Err(JvmCodegenError::UnsupportedSymbolKind {
                        symbol: method_plan.method,
                        expected: SymbolRole::Method,
                        found: method.kind,
                    });
                }
                if method.owner != Some(class_plan.class) {
                    return Err(JvmCodegenError::WrongOwner {
                        symbol: method_plan.method,
                        expected: class_plan.class,
                        actual: method.owner,
                    });
                }
                self.validate_symbol_type(method_plan.method)?;

                let mut seen_parameters = HashSet::new();
                for parameter_id in &method_plan.parameters {
                    if !seen_parameters.insert(*parameter_id) {
                        return Err(JvmCodegenError::DuplicateParameter {
                            parameter: *parameter_id,
                            method: method_plan.method,
                        });
                    }
                    let parameter = self
                        .store
                        .symbols
                        .contains(*parameter_id)
                        .then(|| self.store.symbols.get(*parameter_id))
                        .ok_or(JvmCodegenError::UnknownParameter {
                            parameter: *parameter_id,
                        })?;
                    if parameter.kind != SymbolKind::Parameter {
                        return Err(JvmCodegenError::UnsupportedSymbolKind {
                            symbol: *parameter_id,
                            expected: SymbolRole::Parameter,
                            found: parameter.kind,
                        });
                    }
                    if parameter.owner != Some(method_plan.method) {
                        return Err(JvmCodegenError::WrongOwner {
                            symbol: *parameter_id,
                            expected: method_plan.method,
                            actual: parameter.owner,
                        });
                    }
                    self.validate_symbol_type(*parameter_id)?;
                }

                let body = method_plan.body.ok_or(JvmCodegenError::MissingTypedBody {
                    method: method_plan.method,
                })?;
                if self.typed_ast.try_get(body).is_none() {
                    return Err(JvmCodegenError::UnknownBodyRoot {
                        method: method_plan.method,
                        tree: body,
                    });
                }
            }
        }

        for (tree_id, tree) in self.typed_ast.iter() {
            validate_tree_type(self.store, tree_id, tree)?;
        }
        Ok(())
    }

    fn validate_symbol_type(&self, symbol: SymbolId) -> Result<(), JvmCodegenError> {
        match self.store.symbols.info(symbol) {
            SymbolInfo::Missing | SymbolInfo::Deferred(_) => {
                Err(JvmCodegenError::MissingSemanticInfo { symbol })
            }
            SymbolInfo::Error => Err(JvmCodegenError::ErrorSemanticInfo { symbol }),
            SymbolInfo::Complete(ty) => validate_type(self.store, *ty).map_err(|reason| {
                JvmCodegenError::InvalidSemanticType {
                    symbol,
                    ty: *ty,
                    reason,
                }
            }),
        }
    }
}

fn validate_tree_type(
    store: &SemanticStore,
    tree_id: TreeId<Typed>,
    tree: &Tree<Typed>,
) -> Result<(), JvmCodegenError> {
    validate_type(store, tree.ty).map_err(|reason| JvmCodegenError::InvalidTreeType {
        tree: tree_id,
        ty: tree.ty,
        reason,
    })
}

fn validate_type(store: &SemanticStore, ty: TypeId) -> Result<(), InvalidTypeReason> {
    match store.types.try_get(ty) {
        None => Err(InvalidTypeReason::MissingOrUnfilled),
        Some(Type::NoType) => Err(InvalidTypeReason::NoType),
        Some(Type::Error(_)) => Err(InvalidTypeReason::Error),
        Some(Type::NoPrefix) => Err(InvalidTypeReason::NoPrefix),
        Some(_) => Ok(()),
    }
}

#[cfg(test)]
mod tests;
