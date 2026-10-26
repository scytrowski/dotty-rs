//! Read-only proofs for the bounded same-source cross-class reference used by
//! the JVM/TASTy producer sequence.
//!
//! This validates source and typed provenance only. It does not lower or
//! serialize a cross-class TASTy signature or method body.

use std::fmt;

use dotty_core::ast::{ApplyKind, AstArena, TreeKind, Typed, TypedAst, Untyped};
use dotty_core::{
    Definitions, SemanticStore, SourceDefinition, SourceId, SourceSemanticIndex, SourceTypedIndex,
    SymbolFlags, SymbolId, SymbolInfo, SymbolKind, TermRefTarget, TreeId, Type, TypeRefTarget,
};
use dotty_jvm::{JvmClassPlan, JvmCompilationUnit};

/// A typed failure while validating the canonical factory/identity proofs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CrossClassFactoryProofError {
    /// The source does not have the required complete class/method shape.
    SourceShape(&'static str),
    /// A source declaration is not bound to the canonical identity in this session.
    SourceIdentity(&'static str),
    /// A symbol has the wrong owner, visibility, flags, or semantic type.
    SemanticIdentity(&'static str),
    /// The exact source-to-typed mapping or canonical typed tree is invalid.
    TypedBinding(&'static str),
    /// The complete JVM unit does not contain the source-proven constructor/method.
    JvmPlan(&'static str),
}

impl fmt::Display for CrossClassFactoryProofError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "cross-class factory proof failed: {self:?}")
    }
}

impl std::error::Error for CrossClassFactoryProofError {}

/// Borrowed inputs for the canonical factory or identity proof.
pub struct CrossClassFactoryProofInput<'session> {
    /// Source identity used by the parser and namer.
    pub source_id: SourceId,
    /// Exact package root containing the complete two-class source unit.
    pub source_root: TreeId<Untyped>,
    /// Canonical semantic symbols and types for this session.
    pub store: &'session SemanticStore,
    /// Canonical primitive definitions for this session.
    pub definitions: &'session Definitions,
    /// Source trees from this session.
    pub source_ast: &'session AstArena<Untyped>,
    /// Typed trees from this session.
    pub typed_ast: &'session TypedAst,
    /// Canonical source declaration and scope mappings from this session.
    pub source_index: &'session SourceSemanticIndex,
    /// Exact source-to-typed tree mappings from this session.
    pub source_typed_index: &'session SourceTypedIndex,
    /// Complete JVM plans from the same semantic and typed session.
    pub jvm_unit: &'session JvmCompilationUnit<'session>,
}

/// The independently validated shape of the source method.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CrossClassFactoryProofKind {
    /// `CounterFactory.create(seed: Int): Counter = new Counter(seed)`.
    Factory,
    /// `identity(counter: Counter): Counter = counter` in CounterFactory or CounterIdentity.
    Identity,
}

/// Read-only canonical identity proof for a factory constructor call or the
/// independent reference-identity control method.
pub struct ValidatedCrossClassFactoryProof<'session> {
    _store: &'session SemanticStore,
    _source_ast: &'session AstArena<Untyped>,
    _typed_ast: &'session TypedAst,
    _source_index: &'session SourceSemanticIndex,
    _source_typed_index: &'session SourceTypedIndex,
    _jvm_unit: &'session JvmCompilationUnit<'session>,
    source_id: SourceId,
    kind: CrossClassFactoryProofKind,
    counter_class: SymbolId,
    owner_class: SymbolId,
    constructor: SymbolId,
    method: SymbolId,
    method_formal: SymbolId,
    source_rhs: TreeId<Untyped>,
    typed_rhs: TreeId<Typed>,
    typed_constructor_select: Option<TreeId<Typed>>,
    typed_new: Option<TreeId<Typed>>,
    descriptor: String,
}

impl ValidatedCrossClassFactoryProof<'_> {
    /// Frontend source identity shared by declarations and typed roots.
    pub fn source_id(&self) -> SourceId {
        self.source_id
    }
    /// Canonical `Counter` class identity in this frontend session.
    pub fn counter_class(&self) -> SymbolId {
        self.counter_class
    }
    /// Canonical factory or identity-owner class identity.
    pub fn owner_class(&self) -> SymbolId {
        self.owner_class
    }
    /// Canonical `Counter.<init>(Int)` identity.
    pub fn constructor(&self) -> SymbolId {
        self.constructor
    }
    /// Canonical `create` or `identity` method identity.
    pub fn method(&self) -> SymbolId {
        self.method
    }
    /// Canonical source formal owned by the proved method.
    pub fn method_formal(&self) -> SymbolId {
        self.method_formal
    }
    /// Exact source RHS root.
    pub fn source_rhs(&self) -> TreeId<Untyped> {
        self.source_rhs
    }
    /// Exact source-bound typed RHS root retained by the JVM method plan.
    pub fn typed_rhs(&self) -> TreeId<Typed> {
        self.typed_rhs
    }
    /// Exact typed constructor selection for the factory proof.
    pub fn typed_constructor_select(&self) -> Option<TreeId<Typed>> {
        self.typed_constructor_select
    }
    /// Exact typed `New` node for the factory proof.
    pub fn typed_new(&self) -> Option<TreeId<Typed>> {
        self.typed_new
    }
    /// JVM method descriptor established from the canonical method plan.
    pub fn descriptor(&self) -> &str {
        &self.descriptor
    }
    /// Whether this is the constructor factory or the separate identity control.
    pub fn kind(&self) -> CrossClassFactoryProofKind {
        self.kind
    }
}

/// Prove canonical source, typed-tree, constructor, method, and JVM-plan
/// identities for the issue #1691 factory and identity-control fixtures.
///
/// The function only inspects borrowed session state. It does not type source,
/// mutate semantic stores, emit class files, or produce TASTy bytes.
pub fn validate_cross_class_factory_proof(
    input: CrossClassFactoryProofInput<'_>,
    kind: CrossClassFactoryProofKind,
) -> Result<ValidatedCrossClassFactoryProof<'_>, CrossClassFactoryProofError> {
    let CrossClassFactoryProofInput {
        source_id,
        source_root,
        store,
        definitions,
        source_ast,
        typed_ast,
        source_index,
        source_typed_index,
        jvm_unit,
    } = input;
    if !std::ptr::eq(jvm_unit.store, store) {
        return Err(CrossClassFactoryProofError::SemanticIdentity(
            "JVM unit uses a different semantic store",
        ));
    }
    if !std::ptr::eq(jvm_unit.typed_ast, typed_ast) {
        return Err(CrossClassFactoryProofError::TypedBinding(
            "JVM unit uses a different typed AST",
        ));
    }
    if store.types.try_get(definitions.int).is_none()
        || store.types.try_get(definitions.no_prefix).is_none()
    {
        return Err(CrossClassFactoryProofError::SemanticIdentity(
            "primitive definitions do not belong to the supplied semantic store",
        ));
    }
    let root = source_ast
        .try_get(source_root)
        .ok_or(CrossClassFactoryProofError::SourceShape(
            "package root is stale",
        ))?;
    let TreeKind::PackageDef(package) = &root.kind else {
        return Err(CrossClassFactoryProofError::SourceShape(
            "source root must be a package",
        ));
    };
    let [counter_tree, owner_tree] = package.stats.as_slice() else {
        return Err(CrossClassFactoryProofError::SourceShape(
            "source unit must contain exactly Counter and one proof class",
        ));
    };
    let source_class_name = |tree| match source_ast.try_get(tree).map(|node| &node.kind) {
        Some(TreeKind::TypeDef(definition)) => {
            store.names.resolve(definition.name.as_name().text())
        }
        _ => "",
    };
    let owner_name = source_class_name(*owner_tree).to_owned();
    if source_class_name(*counter_tree) != "Counter"
        || match kind {
            CrossClassFactoryProofKind::Factory => owner_name != "CounterFactory",
            CrossClassFactoryProofKind::Identity => {
                owner_name != "CounterFactory" && owner_name != "CounterIdentity"
            }
        }
    {
        return Err(CrossClassFactoryProofError::SourceShape(
            "source declarations must be Counter followed by the expected proof class",
        ));
    }
    validate_plain_class_source(source_ast, *counter_tree).map_err(|_| {
        CrossClassFactoryProofError::SourceShape(
            "Counter must be an ordinary primary-constructor class",
        )
    })?;
    validate_plain_class_source(source_ast, *owner_tree).map_err(|_| {
        CrossClassFactoryProofError::SourceShape("proof owner must be an ordinary class")
    })?;
    let counter = source_index.symbol_at(source_id, *counter_tree).ok_or(
        CrossClassFactoryProofError::SourceIdentity("Counter has no canonical source symbol"),
    )?;
    let owner = source_index.symbol_at(source_id, *owner_tree).ok_or(
        CrossClassFactoryProofError::SourceIdentity("proof owner has no canonical source symbol"),
    )?;
    validate_canonical_definition(source_index, source_id, *counter_tree, counter).map_err(
        |_| {
            CrossClassFactoryProofError::SourceIdentity(
                "Counter identity is not bound to this exact source declaration",
            )
        },
    )?;
    validate_canonical_definition(source_index, source_id, *owner_tree, owner).map_err(|_| {
        CrossClassFactoryProofError::SourceIdentity(
            "proof owner is not bound to this exact source declaration",
        )
    })?;
    let counter_declarations = factory_class_declarations(store, source_index, counter)?;
    let owner_declarations = factory_class_declarations(store, source_index, owner)?;
    if !store.symbols.contains(counter) || !store.symbols.contains(owner) || counter == owner {
        return Err(CrossClassFactoryProofError::SourceIdentity(
            "class identity is stale or aliased",
        ));
    }
    let counter_symbol = store.symbols.get(counter);
    let owner_symbol = store.symbols.get(owner);
    if counter_symbol.kind != SymbolKind::Class
        || owner_symbol.kind != SymbolKind::Class
        || counter_symbol.visibility != dotty_core::Visibility::Public
        || owner_symbol.visibility != dotty_core::Visibility::Public
        || has_any_flag(
            counter_symbol.flags,
            SymbolFlags::CASE | SymbolFlags::ENUM | SymbolFlags::ABSTRACT | SymbolFlags::SYNTHETIC,
        )
        || has_any_flag(
            owner_symbol.flags,
            SymbolFlags::CASE | SymbolFlags::ENUM | SymbolFlags::ABSTRACT | SymbolFlags::SYNTHETIC,
        )
    {
        return Err(CrossClassFactoryProofError::SemanticIdentity(
            "both classes must be public ordinary concrete classes",
        ));
    }
    let package_owner =
        counter_symbol
            .owner
            .ok_or(CrossClassFactoryProofError::SemanticIdentity(
                "Counter has no canonical package owner",
            ))?;
    if !store.symbols.contains(package_owner)
        || owner_symbol.owner != Some(package_owner)
        || store.symbols.get(package_owner).kind != SymbolKind::Package
    {
        return Err(CrossClassFactoryProofError::SemanticIdentity(
            "both classes must have the exact same package owner",
        ));
    }
    let package_scope =
        source_index
            .scope_of(package_owner)
            .ok_or(CrossClassFactoryProofError::SourceIdentity(
                "canonical package has no declaration scope in the source index",
            ))?;
    if source_index.symbol_at(source_id, source_root) != Some(package_owner)
        || !store.scopes.contains(package_scope)
        || store.scopes.get(package_scope).owner != Some(package_owner)
        || [counter, owner].iter().any(|class| {
            !store
                .scopes
                .get(package_scope)
                .entered_symbols()
                .any(|member| member == *class)
        })
    {
        return Err(CrossClassFactoryProofError::SourceIdentity(
            "Counter and proof owner must be members of the exact source package scope",
        ));
    }
    let TreeKind::TypeDef(counter_def) = &source_ast.get(*counter_tree).kind else {
        unreachable!()
    };
    let TreeKind::Template(counter_template) = &source_ast.get(counter_def.rhs).kind else {
        unreachable!()
    };
    let ctor_tree = counter_template.constructor;
    let TreeKind::DefDef(ctor_def) = &source_ast.get(ctor_tree).kind else {
        unreachable!()
    };
    if counter_template.body.iter().any(|tree| source_ast.try_get(*tree).is_none_or(|node| matches!(&node.kind,
        TreeKind::DefDef(definition) if store.names.resolve(definition.name.as_name().text()) == "<init>"))) {
        return Err(CrossClassFactoryProofError::SourceShape("secondary Counter constructors are unsupported"));
    }
    if !ctor_def.type_params.is_empty()
        || ctor_def.value_param_clauses.len() != 1
        || ctor_def.value_param_clauses[0].len() != 1
    {
        return Err(CrossClassFactoryProofError::SourceShape(
            "Counter must have one plain Int primary-constructor formal",
        ));
    }
    let constructor = source_index.symbol_at(source_id, ctor_tree).ok_or(
        CrossClassFactoryProofError::SourceIdentity(
            "Counter primary constructor has no canonical identity",
        ),
    )?;
    if !counter_declarations
        .entered_symbols()
        .any(|symbol| symbol == constructor)
    {
        return Err(CrossClassFactoryProofError::SourceIdentity(
            "Counter primary constructor is absent from its canonical declaration scope",
        ));
    }
    validate_canonical_definition(source_index, source_id, ctor_tree, constructor).map_err(
        |_| {
            CrossClassFactoryProofError::SourceIdentity(
                "constructor is not bound to its exact source tree",
            )
        },
    )?;
    let ctor_formal_tree = ctor_def.value_param_clauses[0][0];
    let ctor_formal_node =
        source_ast
            .try_get(ctor_formal_tree)
            .ok_or(CrossClassFactoryProofError::SourceShape(
                "constructor formal source tree is stale",
            ))?;
    if !matches!(&ctor_formal_node.kind, TreeKind::ValDef(parameter) if parameter.rhs.is_none()) {
        return Err(CrossClassFactoryProofError::SourceShape(
            "Counter constructor formal must not have a default value",
        ));
    }
    let ctor_formal = source_index
        .derived_symbol_at(constructor, source_id, ctor_formal_tree)
        .ok_or(CrossClassFactoryProofError::SourceIdentity(
            "constructor formal is not canonically owned by the constructor",
        ))?;
    if !store.symbols.contains(constructor)
        || store.symbols.get(constructor).kind != SymbolKind::Constructor
        || store.symbols.get(constructor).owner != Some(counter)
        || store.symbols.get(constructor).visibility != dotty_core::Visibility::Public
        || has_any_flag(
            store.symbols.get(constructor).flags,
            SymbolFlags::ABSTRACT | SymbolFlags::SYNTHETIC,
        )
        || !store.symbols.contains(ctor_formal)
        || store.symbols.get(ctor_formal).owner != Some(constructor)
    {
        return Err(CrossClassFactoryProofError::SemanticIdentity(
            "Counter primary constructor must be public and canonically owned by Counter",
        ));
    }
    let SymbolInfo::Complete(ctor_type) = store.symbols.get(constructor).info else {
        return Err(CrossClassFactoryProofError::SemanticIdentity(
            "Counter constructor signature is incomplete",
        ));
    };
    let Some(Type::Method(ctor_signature)) = store.types.try_get(ctor_type) else {
        return Err(CrossClassFactoryProofError::SemanticIdentity(
            "Counter constructor must have one plain method type",
        ));
    };
    let SymbolInfo::Complete(ctor_formal_type) = store.symbols.get(ctor_formal).info else {
        return Err(CrossClassFactoryProofError::SemanticIdentity(
            "constructor formal type is incomplete",
        ));
    };
    if ctor_def.metadata.visibility.is_some()
        || ctor_signature.kind != dotty_core::MethodKind::Plain
        || ctor_signature.params.len() != 1
        || ctor_signature.params[0].ty != definitions.int
        || ctor_formal_type != definitions.int
    {
        return Err(CrossClassFactoryProofError::SemanticIdentity(
            "Counter constructor must have exactly one Int formal",
        ));
    }
    let TreeKind::TypeDef(owner_def) = &source_ast.get(*owner_tree).kind else {
        unreachable!()
    };
    let TreeKind::Template(owner_template) = &source_ast.get(owner_def.rhs).kind else {
        unreachable!()
    };
    if !owner_template.body.iter().any(|tree| {
        source_ast
            .try_get(*tree)
            .is_some_and(|node| matches!(node.kind, TreeKind::DefDef(_)))
    }) {
        return Err(CrossClassFactoryProofError::SourceShape(
            "proof class must declare its concrete method",
        ));
    }
    let method_name = match kind {
        CrossClassFactoryProofKind::Factory => "create",
        CrossClassFactoryProofKind::Identity => "identity",
    };
    let method_trees = owner_template
        .body
        .iter()
        .copied()
        .filter_map(
            |tree| match source_ast.try_get(tree).map(|node| &node.kind) {
                Some(TreeKind::DefDef(definition))
                    if store.names.resolve(definition.name.as_name().text()) == method_name =>
                {
                    Some((tree, definition))
                }
                _ => None,
            },
        )
        .collect::<Vec<_>>();
    let [(method_tree, method_def)] = method_trees.as_slice() else {
        return Err(CrossClassFactoryProofError::SourceShape(
            "proof class must declare exactly one method of the expected name",
        ));
    };
    if !method_def.type_params.is_empty()
        || method_def.value_param_clauses.len() != 1
        || method_def.value_param_clauses[0].len() != 1
        || method_def.rhs.is_none()
        || method_def.metadata.visibility.is_some()
    {
        return Err(CrossClassFactoryProofError::SourceShape(
            "method must be concrete, non-generic, and have one plain formal",
        ));
    }
    let method = source_index.symbol_at(source_id, *method_tree).ok_or(
        CrossClassFactoryProofError::SourceIdentity("method has no canonical source symbol"),
    )?;
    if !owner_declarations
        .entered_symbols()
        .any(|symbol| symbol == method)
    {
        return Err(CrossClassFactoryProofError::SourceIdentity(
            "proof method is absent from its canonical declaration scope",
        ));
    }
    validate_canonical_definition(source_index, source_id, *method_tree, method).map_err(|_| {
        CrossClassFactoryProofError::SourceIdentity("method is not bound to its exact source tree")
    })?;
    let method_formal_tree = method_def.value_param_clauses[0][0];
    let method_formal_node =
        source_ast
            .try_get(method_formal_tree)
            .ok_or(CrossClassFactoryProofError::SourceShape(
                "method formal source tree is stale",
            ))?;
    if !matches!(&method_formal_node.kind, TreeKind::ValDef(parameter) if parameter.rhs.is_none()) {
        return Err(CrossClassFactoryProofError::SourceShape(
            "proof method formal must not have a default value",
        ));
    }
    let method_formal = source_index
        .symbol_at(source_id, method_formal_tree)
        .ok_or(CrossClassFactoryProofError::SourceIdentity(
            "method formal has no canonical source symbol",
        ))?;
    validate_canonical_definition(source_index, source_id, method_formal_tree, method_formal)
        .map_err(|_| {
            CrossClassFactoryProofError::SourceIdentity(
                "method formal is not bound to its exact source tree",
            )
        })?;
    if !store.symbols.contains(method)
        || store.symbols.get(method).kind != SymbolKind::Method
        || store.symbols.get(method).owner != Some(owner)
        || store.symbols.get(method).visibility != dotty_core::Visibility::Public
        || has_any_flag(
            store.symbols.get(method).flags,
            SymbolFlags::ABSTRACT
                | SymbolFlags::SYNTHETIC
                | SymbolFlags::STATIC
                | SymbolFlags::GIVEN
                | SymbolFlags::IMPLICIT
                | SymbolFlags::ERASED
                | SymbolFlags::EXTENSION,
        )
        || !store.symbols.contains(method_formal)
        || store.symbols.get(method_formal).owner != Some(method)
    {
        return Err(CrossClassFactoryProofError::SemanticIdentity(
            "proof method must be public, concrete, source-defined, and own its formal",
        ));
    }
    let SymbolInfo::Complete(method_type) = store.symbols.get(method).info else {
        return Err(CrossClassFactoryProofError::SemanticIdentity(
            "proof method signature is incomplete",
        ));
    };
    let Some(Type::Method(signature)) = store.types.try_get(method_type) else {
        return Err(CrossClassFactoryProofError::SemanticIdentity(
            "proof method must have one plain method type",
        ));
    };
    let expected_formal_type = if kind == CrossClassFactoryProofKind::Factory {
        definitions.int
    } else {
        let SymbolInfo::Complete(formal_type) = store.symbols.get(method_formal).info else {
            return Err(CrossClassFactoryProofError::SemanticIdentity(
                "identity formal type is incomplete",
            ));
        };
        formal_type
    };
    let SymbolInfo::Complete(method_formal_type) = store.symbols.get(method_formal).info else {
        return Err(CrossClassFactoryProofError::SemanticIdentity(
            "method formal type is incomplete",
        ));
    };
    if signature.kind != dotty_core::MethodKind::Plain
        || signature.params.len() != 1
        || signature.params[0].ty != expected_formal_type
        || method_formal_type != expected_formal_type
        || (kind == CrossClassFactoryProofKind::Identity
            && !is_canonical_class_type_ref(
                store,
                expected_formal_type,
                counter,
                package_owner,
                definitions.no_prefix,
            ))
        || !is_canonical_class_type_ref(
            store,
            signature.result,
            counter,
            package_owner,
            definitions.no_prefix,
        )
    {
        return Err(CrossClassFactoryProofError::SemanticIdentity(
            "method signature must use the exact Counter reference result and planned formal",
        ));
    }
    let source_rhs = method_def
        .rhs
        .ok_or(CrossClassFactoryProofError::SourceShape(
            "proof method body is missing",
        ))?;
    let source_rhs_node =
        source_ast
            .try_get(source_rhs)
            .ok_or(CrossClassFactoryProofError::SourceShape(
                "method RHS source root is stale",
            ))?;
    let typed_rhs = source_typed_index.get(source_id, source_rhs).ok_or(
        CrossClassFactoryProofError::TypedBinding(
            "method RHS has no exact source-to-typed mapping",
        ),
    )?;
    let typed_root =
        typed_ast
            .try_get(typed_rhs)
            .ok_or(CrossClassFactoryProofError::TypedBinding(
                "typed RHS root is stale",
            ))?;
    let (typed_constructor_select, typed_new) = match kind {
        CrossClassFactoryProofKind::Factory => {
            let TreeKind::Apply(source_apply) = &source_rhs_node.kind else {
                return Err(CrossClassFactoryProofError::SourceShape(
                    "factory RHS must be Apply(Select(New(Counter), <init>), [seed])",
                ));
            };
            let source_function = source_ast.try_get(source_apply.function).ok_or(
                CrossClassFactoryProofError::SourceShape("factory function source root is stale"),
            )?;
            let TreeKind::Select(source_select) = &source_function.kind else {
                return Err(CrossClassFactoryProofError::SourceShape(
                    "factory Apply must select the constructor",
                ));
            };
            let source_qualifier = source_ast.try_get(source_select.qualifier).ok_or(
                CrossClassFactoryProofError::SourceShape("factory constructor qualifier is stale"),
            )?;
            let TreeKind::New(_) = &source_qualifier.kind else {
                return Err(CrossClassFactoryProofError::SourceShape(
                    "factory constructor selection must qualify New(Counter)",
                ));
            };
            let argument_tree =
                *source_apply
                    .args
                    .first()
                    .ok_or(CrossClassFactoryProofError::SourceShape(
                        "factory call has no argument",
                    ))?;
            let source_argument_node = source_ast.try_get(argument_tree).ok_or(
                CrossClassFactoryProofError::SourceShape("factory argument source root is stale"),
            )?;
            let TreeKind::Ident(source_argument) = &source_argument_node.kind else {
                return Err(CrossClassFactoryProofError::SourceShape(
                    "factory argument must be the seed identifier",
                ));
            };
            let formal_name = match &method_formal_node.kind {
                TreeKind::ValDef(definition) => {
                    store.names.resolve(definition.name.as_name().text())
                }
                _ => {
                    return Err(CrossClassFactoryProofError::SourceShape(
                        "method formal must be a value parameter",
                    ));
                }
            };
            if source_apply.kind != ApplyKind::Regular || source_apply.args.len() != 1 {
                return Err(CrossClassFactoryProofError::SourceShape(
                    "factory source body must use one regular constructor argument",
                ));
            }
            if store.names.resolve(source_argument.name.text()) != formal_name {
                return Err(CrossClassFactoryProofError::SourceShape(
                    "factory source argument must be the seed identifier",
                ));
            }
            if store.names.resolve(source_select.name.text()) != "<init>" {
                return Err(CrossClassFactoryProofError::SourceShape(
                    "factory source selection must name <init>",
                ));
            }
            let source_function_typed = source_typed_index
                .get(source_id, source_apply.function)
                .ok_or(CrossClassFactoryProofError::TypedBinding(
                    "constructor selection has no typed source mapping",
                ))?;
            let source_new_typed = source_typed_index
                .get(source_id, source_select.qualifier)
                .ok_or(CrossClassFactoryProofError::TypedBinding(
                    "New expression has no typed source mapping",
                ))?;
            let source_argument_typed = source_typed_index
                .get(source_id, source_apply.args[0])
                .ok_or(CrossClassFactoryProofError::TypedBinding(
                    "constructor argument has no typed source mapping",
                ))?;
            if [
                source_function_typed,
                source_new_typed,
                source_argument_typed,
            ]
            .iter()
            .any(|tree| typed_ast.try_get(*tree).is_none())
            {
                return Err(CrossClassFactoryProofError::TypedBinding(
                    "source mapping points outside the typed tree arena",
                ));
            }
            let TreeKind::Apply(typed_apply) = &typed_root.kind else {
                return Err(CrossClassFactoryProofError::TypedBinding(
                    "typed factory RHS is not Apply",
                ));
            };
            if typed_apply.kind != ApplyKind::Regular
                || typed_apply.args.len() != 1
                || source_typed_index.get(source_id, source_apply.function)
                    != Some(typed_apply.function)
                || source_typed_index.get(source_id, source_apply.args[0])
                    != Some(typed_apply.args[0])
            {
                return Err(CrossClassFactoryProofError::TypedBinding(
                    "typed factory Apply does not preserve exact source function and argument bindings",
                ));
            }
            let Some(typed_function) = typed_ast.try_get(typed_apply.function) else {
                return Err(CrossClassFactoryProofError::TypedBinding(
                    "typed factory function is stale",
                ));
            };
            let TreeKind::Select(typed_select) = &typed_function.kind else {
                return Err(CrossClassFactoryProofError::TypedBinding(
                    "typed factory function is not Select",
                ));
            };
            if source_typed_index.get(source_id, source_select.qualifier)
                != Some(typed_select.qualifier)
            {
                return Err(CrossClassFactoryProofError::TypedBinding(
                    "typed constructor receiver is not the exact mapped source New",
                ));
            }
            let Some(typed_new_node) = typed_ast.try_get(typed_select.qualifier) else {
                return Err(CrossClassFactoryProofError::TypedBinding(
                    "typed New is stale",
                ));
            };
            let TreeKind::New(typed_new_tree) = &typed_new_node.kind else {
                return Err(CrossClassFactoryProofError::TypedBinding(
                    "typed constructor receiver is not New",
                ));
            };
            let Some(typed_argument) = typed_ast.try_get(typed_apply.args[0]) else {
                return Err(CrossClassFactoryProofError::TypedBinding(
                    "typed constructor argument is stale",
                ));
            };
            if !matches!(&typed_argument.kind, TreeKind::Ident(ident) if store.names.resolve(ident.name.text()) == formal_name)
                || !matches!(store.types.try_get(typed_argument.ty), Some(Type::TermRef { target: TermRefTarget::Symbol(target), .. }) if *target == method_formal)
            {
                return Err(CrossClassFactoryProofError::TypedBinding(
                    "typed factory argument is not the canonical seed formal",
                ));
            }
            if !matches!(store.types.try_get(typed_function.ty), Some(Type::TermRef { target: TermRefTarget::Symbol(target), .. }) if *target == constructor)
            {
                return Err(CrossClassFactoryProofError::TypedBinding(
                    "typed factory selection does not target Counter's canonical constructor",
                ));
            }
            let typed_new_type_tree = typed_ast.try_get(typed_new_tree.tpt).ok_or(
                CrossClassFactoryProofError::TypedBinding("typed New type tree is stale"),
            )?;
            if !is_canonical_class_type_ref(
                store,
                typed_new_type_tree.ty,
                counter,
                package_owner,
                definitions.no_prefix,
            ) {
                return Err(CrossClassFactoryProofError::TypedBinding(
                    "typed New does not retain the canonical Counter TypeRef",
                ));
            }
            if !matches!(store.types.try_get(typed_root.ty), Some(Type::TypeRef { target: TypeRefTarget::Symbol(target), .. }) if *target == counter)
            {
                return Err(CrossClassFactoryProofError::TypedBinding(
                    "typed factory result is not the canonical Counter TypeRef",
                ));
            }
            (Some(typed_apply.function), Some(typed_select.qualifier))
        }
        CrossClassFactoryProofKind::Identity => {
            let TreeKind::Ident(source_ident) = &source_rhs_node.kind else {
                return Err(CrossClassFactoryProofError::SourceShape(
                    "identity RHS must be the parameter identifier",
                ));
            };
            let TreeKind::Ident(typed_ident) = &typed_root.kind else {
                return Err(CrossClassFactoryProofError::TypedBinding(
                    "identity RHS must remain a typed Ident",
                ));
            };
            if source_typed_index.get(source_id, source_rhs) != Some(typed_rhs)
                || store.names.resolve(source_ident.name.text())
                    != store.names.resolve(typed_ident.name.text())
                || !matches!(store.types.try_get(typed_root.ty), Some(Type::TermRef { target: TermRefTarget::Symbol(target), .. }) if *target == method_formal)
            {
                return Err(CrossClassFactoryProofError::TypedBinding(
                    "identity body must return its exact canonical Counter formal",
                ));
            }
            (None, None)
        }
    };
    if kind == CrossClassFactoryProofKind::Factory
        && !matches!(store.types.try_get(typed_root.ty), Some(Type::TypeRef { target: TypeRefTarget::Symbol(target), .. }) if *target == counter)
    {
        return Err(CrossClassFactoryProofError::TypedBinding(
            "typed method RHS is not the declared canonical Counter result type",
        ));
    }
    let counter_plan = unique_class_plan(jvm_unit, counter, "Counter").map_err(|_| {
        CrossClassFactoryProofError::JvmPlan("complete JVM unit omits canonical Counter")
    })?;
    let mut owner_plans = jvm_unit.classes.iter().filter(|plan| plan.class == owner);
    let owner_plan = owner_plans
        .next()
        .ok_or(CrossClassFactoryProofError::JvmPlan(
            "complete JVM unit omits proof owner",
        ))?;
    if owner_plans.next().is_some() {
        return Err(CrossClassFactoryProofError::JvmPlan(
            "proof owner occurs more than once in the JVM unit",
        ));
    }
    let dotty_jvm::JvmClassInitializationPlan::PrimaryConstructor(primary) =
        &counter_plan.initialization
    else {
        return Err(CrossClassFactoryProofError::JvmPlan(
            "Counter JVM plan has no validated primary constructor",
        ));
    };
    if primary.symbol() != constructor || primary.parameters() != [ctor_formal] {
        return Err(CrossClassFactoryProofError::JvmPlan(
            "Counter JVM plan does not use the exact planned primary constructor/formal",
        ));
    }
    validate_planned_method(
        jvm_unit,
        owner_plan,
        method,
        &[method_formal],
        Some(typed_rhs),
        method_name,
    )
    .map_err(|_| {
        CrossClassFactoryProofError::JvmPlan(
            "method plan does not retain the exact source formal and typed body",
        )
    })?;
    let metadata = jvm_unit.metadata(definitions).map_err(|_| {
        CrossClassFactoryProofError::JvmPlan("complete JVM unit metadata is invalid")
    })?;
    let counter_metadata = metadata
        .iter()
        .find(|class| class.identity().symbol() == counter)
        .ok_or(CrossClassFactoryProofError::JvmPlan(
            "Counter metadata is missing",
        ))?;
    let constructor_metadata =
        counter_metadata
            .constructor()
            .ok_or(CrossClassFactoryProofError::JvmPlan(
                "Counter JVM constructor metadata is missing",
            ))?;
    if constructor_metadata.symbol() != constructor
        || constructor_metadata
            .parameters()
            .iter()
            .map(|parameter| parameter.symbol())
            .collect::<Vec<_>>()
            != [ctor_formal]
        || !constructor_metadata
            .access_flags()
            .contains(dotty_classfile::access_flags::ACC_PUBLIC)
        || constructor_metadata.descriptor().as_str() != "(I)V"
    {
        return Err(CrossClassFactoryProofError::JvmPlan(
            "Counter primary constructor must be the exact public (I)V plan",
        ));
    }
    let owner_metadata = metadata
        .iter()
        .find(|class| class.identity().symbol() == owner)
        .ok_or(CrossClassFactoryProofError::JvmPlan(
            "proof owner metadata is missing",
        ))?;
    let method_metadata = owner_metadata
        .methods()
        .iter()
        .find(|candidate| candidate.symbol() == method)
        .ok_or(CrossClassFactoryProofError::JvmPlan(
            "method JVM metadata is missing",
        ))?;
    let descriptor = method_metadata.descriptor();
    if method_metadata.name() != method_name
        || method_metadata
            .parameters()
            .iter()
            .map(|parameter| parameter.symbol())
            .collect::<Vec<_>>()
            != [method_formal]
        || !method_metadata
            .access_flags()
            .contains(dotty_classfile::access_flags::ACC_PUBLIC)
    {
        return Err(CrossClassFactoryProofError::JvmPlan(
            "method JVM metadata does not retain its public name and exact formal",
        ));
    }
    let expected_descriptor = if kind == CrossClassFactoryProofKind::Factory {
        "(I)Lexample/Counter;"
    } else {
        "(Lexample/Counter;)Lexample/Counter;"
    };
    let counter_internal_name = counter_metadata.identity().internal_name();
    if counter_internal_name != "example/Counter" || descriptor.as_str() != expected_descriptor {
        return Err(CrossClassFactoryProofError::JvmPlan(
            "method JVM descriptor does not match the canonical proof signature",
        ));
    }
    Ok(ValidatedCrossClassFactoryProof {
        _store: store,
        _source_ast: source_ast,
        _typed_ast: typed_ast,
        _source_index: source_index,
        _source_typed_index: source_typed_index,
        _jvm_unit: jvm_unit,
        source_id,
        kind,
        counter_class: counter,
        owner_class: owner,
        constructor,
        method,
        method_formal,
        source_rhs,
        typed_rhs,
        typed_constructor_select,
        typed_new,
        descriptor: descriptor.as_str().to_owned(),
    })
}

fn factory_class_declarations<'a>(
    store: &'a SemanticStore,
    source_index: &SourceSemanticIndex,
    class: SymbolId,
) -> Result<&'a dotty_core::Scope, CrossClassFactoryProofError> {
    if !store.symbols.contains(class) {
        return Err(CrossClassFactoryProofError::SourceIdentity(
            "class identity is stale",
        ));
    }
    let symbol = store.symbols.get(class);
    let SymbolInfo::Complete(class_type) = symbol.info else {
        return Err(CrossClassFactoryProofError::SemanticIdentity(
            "class info is incomplete",
        ));
    };
    let Some(Type::ClassInfo(info)) = store.types.try_get(class_type) else {
        return Err(CrossClassFactoryProofError::SemanticIdentity(
            "class symbol does not carry ClassInfo",
        ));
    };
    if !store.scopes.contains(info.declarations)
        || info.class != class
        || store.scopes.get(info.declarations).owner != Some(class)
        || source_index.scope_of(class) != Some(info.declarations)
    {
        return Err(CrossClassFactoryProofError::SourceIdentity(
            "class declaration scope is not associated with its canonical source identity",
        ));
    }
    Ok(store.scopes.get(info.declarations))
}

/// A typed failure while validating the bounded `Reader.read(counter)` proof.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CrossClassMethodRefError {
    /// The untyped source does not have the required two-class shape.
    SourceShape(&'static str),
    /// A source declaration is not bound to the canonical identity in this session.
    SourceIdentity(&'static str),
    /// A class, member, or parameter has an unsupported semantic type or owner.
    SemanticIdentity(&'static str),
    /// A source tree does not map to the exact typed tree being validated.
    TypedBinding(&'static str),
    /// The complete JVM unit does not contain the canonical source plans.
    JvmPlan(&'static str),
}

impl fmt::Display for CrossClassMethodRefError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "cross-class method reference proof failed: {self:?}")
    }
}

impl std::error::Error for CrossClassMethodRefError {}

/// Borrowed inputs from one frontend/JVM planning session.
///
/// The source root must be the package tree that owns the two planned classes.
/// All IDs are interpreted through these borrowed session stores and indexes.
/// Arena IDs do not authenticate their originating arena, so callers must
/// supply every input from the same frontend session.
pub struct CrossClassMethodRefInput<'session> {
    /// Source identity used by the parser and namer.
    pub source_id: SourceId,
    /// The exact package root from the untyped source arena.
    pub source_root: TreeId<Untyped>,
    /// Canonical symbols and semantic types for this session.
    pub store: &'session SemanticStore,
    /// Canonical primitive definitions for this session.
    pub definitions: &'session Definitions,
    /// Source trees from this session.
    pub source_ast: &'session AstArena<Untyped>,
    /// Typed trees from this session.
    pub typed_ast: &'session TypedAst,
    /// Canonical source declaration and scope mappings from this session.
    pub source_index: &'session SourceSemanticIndex,
    /// Source-to-typed mappings from this session.
    pub source_typed_index: &'session SourceTypedIndex,
    /// JVM plans built from the same semantic store and typed arena.
    pub jvm_unit: &'session JvmCompilationUnit<'session>,
}

/// Validated source/typed identity for `Reader.read(counter: Counter) = counter.get()`.
///
/// This proof is read-only and retains the input borrows so that its IDs and
/// typed roots remain tied to the exact compilation session that established
/// them. It proves only the concrete zero-argument public `Counter.get(): Int`
/// call shape; it does not claim that a TASTy producer can serialize it.
pub struct ValidatedCrossClassMethodRef<'session> {
    _store: &'session SemanticStore,
    _source_ast: &'session AstArena<Untyped>,
    _typed_ast: &'session TypedAst,
    _source_index: &'session SourceSemanticIndex,
    _source_typed_index: &'session SourceTypedIndex,
    _jvm_unit: &'session JvmCompilationUnit<'session>,
    source_id: SourceId,
    counter_class: SymbolId,
    reader_class: SymbolId,
    counter_get: SymbolId,
    reader_read: SymbolId,
    counter_formal: SymbolId,
    source_rhs: TreeId<Untyped>,
    typed_rhs: TreeId<Typed>,
    typed_select: TreeId<Typed>,
    typed_receiver: TreeId<Typed>,
    method_descriptor: &'static str,
}

impl ValidatedCrossClassMethodRef<'_> {
    /// Source file identity shared by both declarations and typed roots.
    pub fn source_id(&self) -> SourceId {
        self.source_id
    }

    /// Canonical planned `Counter` class identity.
    pub fn counter_class(&self) -> SymbolId {
        self.counter_class
    }

    /// Canonical planned `Reader` class identity.
    pub fn reader_class(&self) -> SymbolId {
        self.reader_class
    }

    /// Canonical public `Counter.get(): Int` method identity.
    pub fn counter_get(&self) -> SymbolId {
        self.counter_get
    }

    /// Canonical `Reader.read(counter: Counter): Int` method identity.
    pub fn reader_read(&self) -> SymbolId {
        self.reader_read
    }

    /// Canonical `counter` formal owned by `Reader.read`.
    pub fn counter_formal(&self) -> SymbolId {
        self.counter_formal
    }

    /// Source body root for the call.
    pub fn source_rhs(&self) -> TreeId<Untyped> {
        self.source_rhs
    }

    /// Typed body root for the call.
    pub fn typed_rhs(&self) -> TreeId<Typed> {
        self.typed_rhs
    }

    /// Typed `counter.get` selection node.
    pub fn typed_select(&self) -> TreeId<Typed> {
        self.typed_select
    }

    /// Typed receiver identifier node.
    pub fn typed_receiver(&self) -> TreeId<Typed> {
        self.typed_receiver
    }

    /// JVM method descriptor proven by the supported zero-argument Int result.
    pub fn method_descriptor(&self) -> &'static str {
        self.method_descriptor
    }
}

/// Validate canonical source, typed-tree, declaration-scope, and JVM-plan
/// identity for the bounded `Counter` / `Reader` selected-call shape.
///
/// This operation only inspects borrowed state. It does not mutate the store,
/// allocate symbols, emit classfiles, or construct TASTy bytes.
pub fn validate_cross_class_method_ref(
    input: CrossClassMethodRefInput<'_>,
) -> Result<ValidatedCrossClassMethodRef<'_>, CrossClassMethodRefError> {
    let CrossClassMethodRefInput {
        source_id,
        source_root,
        store,
        definitions,
        source_ast,
        typed_ast,
        source_index,
        source_typed_index,
        jvm_unit,
    } = input;

    if !std::ptr::eq(jvm_unit.store, store) {
        return Err(CrossClassMethodRefError::SemanticIdentity(
            "JVM unit uses a different semantic store",
        ));
    }
    if !std::ptr::eq(jvm_unit.typed_ast, typed_ast) {
        return Err(CrossClassMethodRefError::TypedBinding(
            "JVM unit uses a different typed AST",
        ));
    }
    let Some(source_root_tree) = source_ast.try_get(source_root) else {
        return Err(CrossClassMethodRefError::SourceShape(
            "source root is not present in the source AST",
        ));
    };
    let TreeKind::PackageDef(package) = &source_root_tree.kind else {
        return Err(CrossClassMethodRefError::SourceShape(
            "source root must be one package declaration",
        ));
    };
    let [counter_tree, reader_tree] = package.stats.as_slice() else {
        return Err(CrossClassMethodRefError::SourceShape(
            "source package must contain exactly Counter and Reader",
        ));
    };
    let class_name = |tree: TreeId<Untyped>| match source_ast.try_get(tree).map(|tree| &tree.kind) {
        Some(TreeKind::TypeDef(definition)) => {
            store.names.resolve(definition.name.as_name().text())
        }
        _ => "",
    };
    if class_name(*counter_tree) != "Counter" || class_name(*reader_tree) != "Reader" {
        return Err(CrossClassMethodRefError::SourceShape(
            "source declarations must be Counter followed by Reader",
        ));
    }
    validate_plain_class_source(source_ast, *counter_tree)?;
    validate_plain_class_source(source_ast, *reader_tree)?;
    let counter_class = source_index.symbol_at(source_id, *counter_tree).ok_or(
        CrossClassMethodRefError::SourceIdentity("Counter has no canonical source symbol"),
    )?;
    let reader_class = source_index.symbol_at(source_id, *reader_tree).ok_or(
        CrossClassMethodRefError::SourceIdentity("Reader has no canonical source symbol"),
    )?;
    validate_canonical_definition(source_index, source_id, *counter_tree, counter_class)?;
    validate_canonical_definition(source_index, source_id, *reader_tree, reader_class)?;
    if counter_class == reader_class {
        return Err(CrossClassMethodRefError::SemanticIdentity(
            "Counter and Reader must be distinct canonical classes",
        ));
    }
    if !store.symbols.contains(counter_class) || !store.symbols.contains(reader_class) {
        return Err(CrossClassMethodRefError::SourceIdentity(
            "class source index contains a stale canonical symbol",
        ));
    }
    if store.symbols.get(counter_class).kind != SymbolKind::Class
        || store.symbols.get(reader_class).kind != SymbolKind::Class
        || store.symbols.get(counter_class).visibility != dotty_core::Visibility::Public
        || store.symbols.get(reader_class).visibility != dotty_core::Visibility::Public
        || has_any_flag(
            store.symbols.get(counter_class).flags,
            SymbolFlags::CASE | SymbolFlags::ENUM | SymbolFlags::ABSTRACT | SymbolFlags::SYNTHETIC,
        )
        || has_any_flag(
            store.symbols.get(reader_class).flags,
            SymbolFlags::CASE | SymbolFlags::ENUM | SymbolFlags::ABSTRACT | SymbolFlags::SYNTHETIC,
        )
    {
        return Err(CrossClassMethodRefError::SemanticIdentity(
            "both declarations must be public ordinary classes",
        ));
    }
    let package_owner = store.symbols.get(counter_class).owner.ok_or(
        CrossClassMethodRefError::SemanticIdentity(
            "Counter and Reader must have the exact same package owner",
        ),
    )?;
    if !store.symbols.contains(package_owner) {
        return Err(CrossClassMethodRefError::SemanticIdentity(
            "class package owner is stale",
        ));
    }
    if store.symbols.get(reader_class).owner != Some(package_owner)
        || store.symbols.get(package_owner).kind != SymbolKind::Package
    {
        return Err(CrossClassMethodRefError::SemanticIdentity(
            "Counter and Reader must have the exact same package owner",
        ));
    }
    let counter_members = class_declarations(store, source_index, counter_class)?;
    let reader_members = class_declarations(store, source_index, reader_class)?;

    let (counter_get_tree, counter_get_definition) = find_method(
        source_ast,
        store,
        *counter_tree,
        "get",
        "Counter must declare exactly one concrete get method",
    )?;
    let (reader_read_tree, reader_read_definition) = find_method(
        source_ast,
        store,
        *reader_tree,
        "read",
        "Reader must declare exactly one concrete read method",
    )?;
    if !counter_get_definition.type_params.is_empty()
        || counter_get_definition.value_param_clauses.len() != 1
        || !counter_get_definition.value_param_clauses[0].is_empty()
        || !reader_read_definition.type_params.is_empty()
        || reader_read_definition.value_param_clauses.len() != 1
        || reader_read_definition.value_param_clauses[0].len() != 1
    {
        return Err(CrossClassMethodRefError::SourceShape(
            "get must be get() and read must have one plain Counter formal",
        ));
    }
    let counter_get = source_index.symbol_at(source_id, counter_get_tree).ok_or(
        CrossClassMethodRefError::SourceIdentity("Counter.get has no canonical source symbol"),
    )?;
    let reader_read = source_index.symbol_at(source_id, reader_read_tree).ok_or(
        CrossClassMethodRefError::SourceIdentity("Reader.read has no canonical source symbol"),
    )?;
    validate_canonical_definition(source_index, source_id, counter_get_tree, counter_get)?;
    validate_canonical_definition(source_index, source_id, reader_read_tree, reader_read)?;
    if !store.symbols.contains(counter_get) || !store.symbols.contains(reader_read) {
        return Err(CrossClassMethodRefError::SourceIdentity(
            "method source index contains a stale canonical symbol",
        ));
    }
    validate_member(store, counter_members, counter_class, counter_get, "get")?;
    validate_member(store, reader_members, reader_class, reader_read, "read")?;
    validate_public_concrete_method(store, counter_get, "Counter.get")?;
    validate_public_concrete_method(store, reader_read, "Reader.read")?;

    let counter_formal_tree = reader_read_definition.value_param_clauses[0][0];
    let counter_formal = source_index
        .symbol_at(source_id, counter_formal_tree)
        .ok_or(CrossClassMethodRefError::SourceIdentity(
            "Reader.read counter formal has no canonical source symbol",
        ))?;
    validate_canonical_definition(source_index, source_id, counter_formal_tree, counter_formal)?;
    if !store.symbols.contains(counter_formal) {
        return Err(CrossClassMethodRefError::SourceIdentity(
            "formal source index contains a stale canonical symbol",
        ));
    }
    if store.symbols.get(counter_formal).kind != SymbolKind::Parameter
        || store.symbols.get(counter_formal).owner != Some(reader_read)
        || store
            .names
            .resolve(store.symbols.get(counter_formal).name.text())
            != "counter"
    {
        return Err(CrossClassMethodRefError::SemanticIdentity(
            "counter must be a Reader.read-owned parameter",
        ));
    }
    let SymbolInfo::Complete(formal_type) = store.symbols.get(counter_formal).info else {
        return Err(CrossClassMethodRefError::SemanticIdentity(
            "counter formal type is not complete",
        ));
    };
    if !is_canonical_class_type_ref(
        store,
        formal_type,
        counter_class,
        package_owner,
        definitions.no_prefix,
    ) {
        return Err(CrossClassMethodRefError::SemanticIdentity(
            "counter formal must refer to the planned Counter class identity",
        ));
    }

    let counter_get_type = complete_method_type(store, counter_get, "Counter.get")?;
    if !counter_get_type.params.is_empty()
        || counter_get_type.result != definitions.int
        || counter_get_type.kind != dotty_core::MethodKind::Plain
    {
        return Err(CrossClassMethodRefError::SemanticIdentity(
            "Counter.get must have the exact zero-argument Int signature ()I",
        ));
    }
    let reader_type = complete_method_type(store, reader_read, "Reader.read")?;
    if reader_type.params.len() != 1
        || reader_type.params[0].ty != formal_type
        || reader_type.params[0].erased
        || reader_type.params[0].varargs
        || reader_type.result != definitions.int
        || reader_type.kind != dotty_core::MethodKind::Plain
    {
        return Err(CrossClassMethodRefError::SemanticIdentity(
            "Reader.read must have one plain Counter formal and return Int",
        ));
    }

    let source_rhs = reader_read_definition
        .rhs
        .ok_or(CrossClassMethodRefError::SourceShape(
            "Reader.read must have a concrete body",
        ))?;
    let Some(source_rhs_tree) = source_ast.try_get(source_rhs) else {
        return Err(CrossClassMethodRefError::SourceShape(
            "Reader.read body is not present in the source AST",
        ));
    };
    let TreeKind::Apply(source_apply) = &source_rhs_tree.kind else {
        return Err(CrossClassMethodRefError::SourceShape(
            "Reader.read body must be counter.get()",
        ));
    };
    let Some(source_function_tree) = source_ast.try_get(source_apply.function) else {
        return Err(CrossClassMethodRefError::SourceShape(
            "Reader.read selected function is not present in the source AST",
        ));
    };
    let TreeKind::Select(source_select) = &source_function_tree.kind else {
        return Err(CrossClassMethodRefError::SourceShape(
            "Reader.read body must select get on its formal",
        ));
    };
    let Some(source_receiver_tree) = source_ast.try_get(source_select.qualifier) else {
        return Err(CrossClassMethodRefError::SourceShape(
            "Reader.read receiver is not present in the source AST",
        ));
    };
    let TreeKind::Ident(source_receiver) = &source_receiver_tree.kind else {
        return Err(CrossClassMethodRefError::SourceShape(
            "Reader.read receiver must be the counter formal identifier",
        ));
    };
    if store.names.resolve(source_select.name.text()) != "get"
        || store.names.resolve(source_receiver.name.text()) != "counter"
        || !source_apply.args.is_empty()
    {
        return Err(CrossClassMethodRefError::SourceShape(
            "Reader.read body must be the zero-argument counter.get() call",
        ));
    }
    let typed_rhs = source_typed_index.get(source_id, source_rhs).ok_or(
        CrossClassMethodRefError::TypedBinding("Reader.read body has no source-to-typed binding"),
    )?;
    let Some(typed_rhs_tree) = typed_ast.try_get(typed_rhs) else {
        return Err(CrossClassMethodRefError::TypedBinding(
            "Reader.read typed body root is stale",
        ));
    };
    let TreeKind::Apply(typed_apply) = &typed_rhs_tree.kind else {
        return Err(CrossClassMethodRefError::TypedBinding(
            "Reader.read typed body is not an Apply",
        ));
    };
    if typed_apply.kind != ApplyKind::Regular || !typed_apply.args.is_empty() {
        return Err(CrossClassMethodRefError::TypedBinding(
            "typed call must be one plain zero-argument Apply",
        ));
    }
    if source_typed_index.get(source_id, source_apply.function) != Some(typed_apply.function) {
        return Err(CrossClassMethodRefError::TypedBinding(
            "selected method root is not the source-bound typed function",
        ));
    }
    let Some(typed_function_tree) = typed_ast.try_get(typed_apply.function) else {
        return Err(CrossClassMethodRefError::TypedBinding(
            "typed selected function root is stale",
        ));
    };
    let TreeKind::Select(typed_select) = &typed_function_tree.kind else {
        return Err(CrossClassMethodRefError::TypedBinding(
            "typed call function is not a Select",
        ));
    };
    if source_typed_index.get(source_id, source_select.qualifier) != Some(typed_select.qualifier) {
        return Err(CrossClassMethodRefError::TypedBinding(
            "receiver is not the source-bound typed qualifier",
        ));
    }
    let Some(typed_receiver_tree) = typed_ast.try_get(typed_select.qualifier) else {
        return Err(CrossClassMethodRefError::TypedBinding(
            "typed receiver root is stale",
        ));
    };
    let TreeKind::Ident(typed_receiver) = &typed_receiver_tree.kind else {
        return Err(CrossClassMethodRefError::TypedBinding(
            "typed receiver is not an identifier",
        ));
    };
    if store.names.resolve(typed_receiver.name.text()) != "counter"
        || store.names.resolve(typed_select.name.text()) != "get"
        || !matches!(
            store.types.try_get(typed_function_tree.ty),
            Some(Type::TermRef { prefix, target: TermRefTarget::Symbol(target) })
                if *target == counter_get && *prefix == typed_receiver_tree.ty
        )
        || !matches!(
            store.types.try_get(typed_receiver_tree.ty),
            Some(Type::TermRef { prefix, target: TermRefTarget::Symbol(target) })
                if *target == counter_formal && *prefix == definitions.no_prefix
        )
    {
        return Err(CrossClassMethodRefError::TypedBinding(
            "typed receiver and selected method must retain canonical symbol targets",
        ));
    }
    if store.types.try_get(typed_rhs_tree.ty).is_none()
        || store.types.try_get(definitions.int).is_none()
        || typed_rhs_tree.ty != definitions.int
    {
        return Err(CrossClassMethodRefError::TypedBinding(
            "typed call result must be the canonical Int type",
        ));
    }

    let counter_get_rhs =
        counter_get_definition
            .rhs
            .ok_or(CrossClassMethodRefError::SourceShape(
                "Counter.get must have a concrete body",
            ))?;
    if source_ast.try_get(counter_get_rhs).is_none() {
        return Err(CrossClassMethodRefError::SourceShape(
            "Counter.get source body root is stale",
        ));
    }
    let counter_get_body = source_typed_index.get(source_id, counter_get_rhs).ok_or(
        CrossClassMethodRefError::TypedBinding("Counter.get body has no source-to-typed binding"),
    )?;
    if typed_ast.try_get(counter_get_body).is_none() {
        return Err(CrossClassMethodRefError::TypedBinding(
            "Counter.get typed body root is stale",
        ));
    }
    let counter_method_plan = unique_class_plan(jvm_unit, counter_class, "Counter")?;
    let reader_method_plan = unique_class_plan(jvm_unit, reader_class, "Reader")?;
    let counter_get_plan = validate_planned_method(
        jvm_unit,
        counter_method_plan,
        counter_get,
        &[],
        Some(counter_get_body),
        "Counter.get",
    )?;
    validate_planned_method(
        jvm_unit,
        reader_method_plan,
        reader_read,
        &[counter_formal],
        Some(typed_rhs),
        "Reader.read",
    )?;
    let descriptor = counter_get_plan
        .descriptor(store, counter_class, definitions)
        .map_err(|_| {
            CrossClassMethodRefError::JvmPlan(
                "Counter.get JVM descriptor cannot be derived from its canonical plan",
            )
        })?;
    if descriptor.as_str() != "()I" {
        return Err(CrossClassMethodRefError::JvmPlan(
            "Counter.get JVM plan does not have the exact ()I descriptor",
        ));
    }

    Ok(ValidatedCrossClassMethodRef {
        _store: store,
        _source_ast: source_ast,
        _typed_ast: typed_ast,
        _source_index: source_index,
        _source_typed_index: source_typed_index,
        _jvm_unit: jvm_unit,
        source_id,
        counter_class,
        reader_class,
        counter_get,
        reader_read,
        counter_formal,
        source_rhs,
        typed_rhs,
        typed_select: typed_apply.function,
        typed_receiver: typed_select.qualifier,
        method_descriptor: "()I",
    })
}

fn validate_canonical_definition(
    source_index: &SourceSemanticIndex,
    source: SourceId,
    tree: TreeId<Untyped>,
    symbol: SymbolId,
) -> Result<(), CrossClassMethodRefError> {
    if source_index.symbol_at(source, tree) != Some(symbol)
        || source_index.definition_of(symbol) != Some(SourceDefinition::Canonical { source, tree })
    {
        return Err(CrossClassMethodRefError::SourceIdentity(
            "symbol is not the canonical identity for this exact source tree",
        ));
    }
    Ok(())
}

fn validate_plain_class_source(
    source_ast: &AstArena<Untyped>,
    class_tree: TreeId<Untyped>,
) -> Result<(), CrossClassMethodRefError> {
    let Some(class_node) = source_ast.try_get(class_tree) else {
        return Err(CrossClassMethodRefError::SourceShape(
            "class declaration is not present in the source AST",
        ));
    };
    let TreeKind::TypeDef(definition) = &class_node.kind else {
        return Err(CrossClassMethodRefError::SourceShape(
            "top-level declaration must be an ordinary class",
        ));
    };
    let Some(template_node) = source_ast.try_get(definition.rhs) else {
        return Err(CrossClassMethodRefError::SourceShape(
            "class template is not present in the source AST",
        ));
    };
    let TreeKind::Template(template) = &template_node.kind else {
        return Err(CrossClassMethodRefError::SourceShape(
            "class declaration must have an ordinary template",
        ));
    };
    if !template.parents.is_empty() || template.self_val.is_some() {
        return Err(CrossClassMethodRefError::SourceShape(
            "cross-class reference proof excludes explicit parents and self types",
        ));
    }
    let Some(constructor_node) = source_ast.try_get(template.constructor) else {
        return Err(CrossClassMethodRefError::SourceShape(
            "class constructor is not present in the source AST",
        ));
    };
    let TreeKind::DefDef(constructor) = &constructor_node.kind else {
        return Err(CrossClassMethodRefError::SourceShape(
            "class primary constructor must be a source method",
        ));
    };
    if !constructor.type_params.is_empty() {
        return Err(CrossClassMethodRefError::SourceShape(
            "generic classes are outside the cross-class reference proof",
        ));
    }
    Ok(())
}

fn class_declarations<'a>(
    store: &'a SemanticStore,
    source_index: &SourceSemanticIndex,
    class: SymbolId,
) -> Result<&'a dotty_core::Scope, CrossClassMethodRefError> {
    if !store.symbols.contains(class) {
        return Err(CrossClassMethodRefError::SemanticIdentity(
            "class identity is stale",
        ));
    }
    let SymbolInfo::Complete(class_type) = store.symbols.get(class).info else {
        return Err(CrossClassMethodRefError::SemanticIdentity(
            "class info is not complete",
        ));
    };
    let Some(Type::ClassInfo(info)) = store.types.try_get(class_type) else {
        return Err(CrossClassMethodRefError::SemanticIdentity(
            "class symbol does not carry ClassInfo",
        ));
    };
    if !store.scopes.contains(info.declarations)
        || info.class != class
        || store.scopes.get(info.declarations).owner != Some(class)
        || source_index.scope_of(class) != Some(info.declarations)
    {
        return Err(CrossClassMethodRefError::SemanticIdentity(
            "class declaration scope is not the canonical source scope",
        ));
    }
    let declarations = store.scopes.get(info.declarations);
    if declarations.entered_symbols().any(|member| {
        if !store.symbols.contains(member) {
            return true;
        }
        let symbol = store.symbols.get(member);
        symbol.kind == SymbolKind::TypeParameter && symbol.owner == Some(class)
    }) {
        return Err(CrossClassMethodRefError::SourceShape(
            "generic classes are outside the cross-class reference proof",
        ));
    }
    Ok(declarations)
}

fn find_method<'a>(
    source_ast: &'a AstArena<Untyped>,
    store: &SemanticStore,
    class_tree: TreeId<Untyped>,
    name: &str,
    reason: &'static str,
) -> Result<(TreeId<Untyped>, &'a dotty_core::ast::DefDef<Untyped>), CrossClassMethodRefError> {
    let Some(class_node) = source_ast.try_get(class_tree) else {
        return Err(CrossClassMethodRefError::SourceShape(
            "class declaration is not present in the source AST",
        ));
    };
    let TreeKind::TypeDef(class_definition) = &class_node.kind else {
        return Err(CrossClassMethodRefError::SourceShape(
            "top-level declaration must be a class",
        ));
    };
    let Some(template_node) = source_ast.try_get(class_definition.rhs) else {
        return Err(CrossClassMethodRefError::SourceShape(
            "class template is not present in the source AST",
        ));
    };
    let TreeKind::Template(template) = &template_node.kind else {
        return Err(CrossClassMethodRefError::SourceShape(
            "class declaration must have a template",
        ));
    };
    let mut matches = Vec::new();
    for tree in template.body.iter().copied() {
        let Some(node) = source_ast.try_get(tree) else {
            return Err(CrossClassMethodRefError::SourceShape(
                "class member is not present in the source AST",
            ));
        };
        if let TreeKind::DefDef(definition) = &node.kind
            && store.names.resolve(definition.name.as_name().text()) == name
        {
            matches.push((tree, definition));
        }
    }
    match matches.as_slice() {
        [(tree, definition)] => Ok((*tree, *definition)),
        _ => Err(CrossClassMethodRefError::SourceShape(reason)),
    }
}

fn validate_member(
    store: &SemanticStore,
    declarations: &dotty_core::Scope,
    owner: SymbolId,
    member: SymbolId,
    name: &str,
) -> Result<(), CrossClassMethodRefError> {
    if !store.symbols.contains(member) {
        return Err(CrossClassMethodRefError::SemanticIdentity(
            "method identity is stale",
        ));
    }
    let symbol = store.symbols.get(member);
    if symbol.owner != Some(owner)
        || store.names.resolve(symbol.name.text()) != name
        || declarations.lookup_all(&symbol.name) != [member]
    {
        return Err(CrossClassMethodRefError::SemanticIdentity(
            "method owner or declaration-scope membership is not canonical",
        ));
    }
    Ok(())
}

fn validate_public_concrete_method(
    store: &SemanticStore,
    method: SymbolId,
    label: &'static str,
) -> Result<(), CrossClassMethodRefError> {
    if !store.symbols.contains(method) {
        return Err(CrossClassMethodRefError::SemanticIdentity(
            "method identity is stale",
        ));
    }
    let symbol = store.symbols.get(method);
    if symbol.kind != SymbolKind::Method
        || symbol.visibility != dotty_core::Visibility::Public
        || has_any_flag(
            symbol.flags,
            SymbolFlags::ABSTRACT
                | SymbolFlags::SYNTHETIC
                | SymbolFlags::ERASED
                | SymbolFlags::STATIC
                | SymbolFlags::EXTENSION
                | SymbolFlags::GIVEN
                | SymbolFlags::IMPLICIT,
        )
    {
        return Err(CrossClassMethodRefError::SemanticIdentity(match label {
            "Counter.get" => "Counter.get must be public, concrete, and source-defined",
            _ => "Reader.read must be public, concrete, and source-defined",
        }));
    }
    Ok(())
}

fn has_any_flag(flags: SymbolFlags, excluded: SymbolFlags) -> bool {
    !flags.intersection(excluded).is_empty()
}

fn is_canonical_class_type_ref(
    store: &SemanticStore,
    ty: dotty_core::TypeId,
    class: SymbolId,
    package: SymbolId,
    no_prefix: dotty_core::TypeId,
) -> bool {
    let Some(Type::TypeRef {
        prefix,
        target: TypeRefTarget::Symbol(target),
    }) = store.types.try_get(ty)
    else {
        return false;
    };
    *target == class
        && matches!(
            store.types.try_get(*prefix),
            Some(Type::TypeRef {
                prefix: package_prefix,
                target: TypeRefTarget::Symbol(package_target),
            }) if *package_prefix == no_prefix && *package_target == package
        )
}

fn complete_method_type<'a>(
    store: &'a SemanticStore,
    method: SymbolId,
    _label: &'static str,
) -> Result<&'a dotty_core::MethodType, CrossClassMethodRefError> {
    if !store.symbols.contains(method) {
        return Err(CrossClassMethodRefError::SemanticIdentity(
            "method identity is stale",
        ));
    }
    let SymbolInfo::Complete(method_type) = store.symbols.get(method).info else {
        return Err(CrossClassMethodRefError::SemanticIdentity(
            "method signature is not complete",
        ));
    };
    match store.types.try_get(method_type) {
        Some(Type::Method(method_type)) => Ok(method_type),
        _ => Err(CrossClassMethodRefError::SemanticIdentity(
            "method must have one non-generic plain method type",
        )),
    }
}

fn unique_class_plan<'a>(
    jvm_unit: &'a JvmCompilationUnit<'_>,
    class: SymbolId,
    label: &'static str,
) -> Result<&'a JvmClassPlan, CrossClassMethodRefError> {
    let mut matching = jvm_unit.classes.iter().filter(|plan| plan.class == class);
    let Some(plan) = matching.next() else {
        return Err(CrossClassMethodRefError::JvmPlan(match label {
            "Counter" => "canonical Counter is absent from the complete JVM unit",
            _ => "canonical Reader is absent from the complete JVM unit",
        }));
    };
    if matching.next().is_some() {
        return Err(CrossClassMethodRefError::JvmPlan(
            "a canonical class occurs more than once in the JVM unit",
        ));
    }
    Ok(plan)
}

fn validate_planned_method<'a>(
    jvm_unit: &JvmCompilationUnit<'_>,
    class_plan: &'a JvmClassPlan,
    method: SymbolId,
    parameters: &[SymbolId],
    body: Option<TreeId<Typed>>,
    label: &'static str,
) -> Result<&'a dotty_jvm::JvmMethodPlan, CrossClassMethodRefError> {
    let total_occurrences = jvm_unit
        .classes
        .iter()
        .flat_map(|plan| plan.methods.iter())
        .filter(|plan| plan.method == method)
        .count();
    if total_occurrences == 0 {
        return Err(CrossClassMethodRefError::JvmPlan(match label {
            "Counter.get" => "canonical Counter.get is absent from Counter's JVM plan",
            _ => "canonical Reader.read is absent from Reader's JVM plan",
        }));
    }
    if total_occurrences != 1 {
        return Err(CrossClassMethodRefError::JvmPlan(
            "canonical method must occur exactly once in the complete JVM unit",
        ));
    }
    let mut matching = class_plan
        .methods
        .iter()
        .filter(|plan| plan.method == method);
    let Some(plan) = matching.next() else {
        return Err(CrossClassMethodRefError::JvmPlan(match label {
            "Counter.get" => "canonical Counter.get is absent from Counter's JVM plan",
            _ => "canonical Reader.read is absent from Reader's JVM plan",
        }));
    };
    if matching.next().is_some() || plan.parameters != parameters || plan.body != body {
        return Err(CrossClassMethodRefError::JvmPlan(
            "planned method identity, formal order, or typed body does not match source",
        ));
    }
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::{has_any_flag, is_canonical_class_type_ref};
    use dotty_core::{Definitions, SemanticStore, SymbolFlags, Type, TypeRefTarget};

    #[test]
    fn excluded_class_and_method_flags_are_checked_individually() {
        let class_exclusions = [
            SymbolFlags::CASE,
            SymbolFlags::ENUM,
            SymbolFlags::ABSTRACT,
            SymbolFlags::SYNTHETIC,
        ];
        let method_exclusions = [
            SymbolFlags::ABSTRACT,
            SymbolFlags::SYNTHETIC,
            SymbolFlags::ERASED,
            SymbolFlags::STATIC,
            SymbolFlags::EXTENSION,
            SymbolFlags::GIVEN,
            SymbolFlags::IMPLICIT,
        ];

        let class_mask = class_exclusions
            .into_iter()
            .fold(SymbolFlags::EMPTY, SymbolFlags::union);
        let method_mask = method_exclusions
            .into_iter()
            .fold(SymbolFlags::EMPTY, SymbolFlags::union);
        for flag in class_exclusions {
            assert!(has_any_flag(flag, class_mask), "class flag {flag:?}");
        }
        for flag in method_exclusions {
            assert!(has_any_flag(flag, method_mask), "method flag {flag:?}");
        }
        assert!(!has_any_flag(SymbolFlags::EMPTY, class_mask));
        assert!(!has_any_flag(SymbolFlags::EMPTY, method_mask));
    }

    #[test]
    fn canonical_class_type_rejects_an_unrelated_prefix() {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let class = definitions.object_class;
        let package = definitions.any_class;
        let package_prefix = store.types.alloc(Type::TypeRef {
            prefix: definitions.no_prefix,
            target: TypeRefTarget::Symbol(package),
        });
        let correct = store.types.alloc(Type::TypeRef {
            prefix: package_prefix,
            target: TypeRefTarget::Symbol(class),
        });
        let wrong_prefix = store.types.alloc(Type::TypeRef {
            prefix: definitions.int,
            target: TypeRefTarget::Symbol(class),
        });

        assert!(is_canonical_class_type_ref(
            &store,
            correct,
            class,
            package,
            definitions.no_prefix
        ));
        assert!(!is_canonical_class_type_ref(
            &store,
            wrong_prefix,
            class,
            package,
            definitions.no_prefix
        ));
    }
}
