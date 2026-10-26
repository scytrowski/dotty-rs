//! Immutable proof for the canonical same-source Counter factory and reference return.

use dotty_core::ast::{ApplyKind, AstArena, TreeKind, Typed, Untyped};
use dotty_core::symbols::{SymbolFlags, SymbolInfo, SymbolKind, SymbolOrigin, Visibility};
use dotty_core::types::{MethodKind, TermRefTarget, Type, TypeRefTarget};
use dotty_core::{
    Definitions, SemanticStore, SourceId, SourceSemanticIndex, SourceTypedIndex, SymbolId, TreeId,
    TypeId,
};
use dotty_jvm::{JvmClassInitializationPlan, JvmClassMetadata, JvmCompilationUnit};

use crate::JvmTastyContractError;

/// Immutable proof that one public factory's typed body constructs the exact
/// public primary-constructor class selected by its declared reference result.
///
/// The proof retains the complete same-source JVM unit and source indexes, so
/// canonical identities and source/typed roots remain immutable and live. It
/// does not encode or publish any artifacts.
pub struct ValidatedSameBatchConstructorReference<'proof, 'unit> {
    _unit: &'proof JvmCompilationUnit<'unit>,
    _source_ast: &'proof AstArena<Untyped>,
    _source_index: &'proof SourceSemanticIndex,
    _typed_index: &'proof SourceTypedIndex,
    source: SourceId,
    counter: SymbolId,
    factory: SymbolId,
    constructor: SymbolId,
    create: SymbolId,
    seed: SymbolId,
    source_new: TreeId<Untyped>,
    source_select: TreeId<Untyped>,
    source_apply: TreeId<Untyped>,
    source_argument: TreeId<Untyped>,
    typed_new: TreeId<Typed>,
    typed_select: TreeId<Typed>,
    typed_apply: TreeId<Typed>,
    typed_argument: TreeId<Typed>,
    result_type: TypeId,
    method_descriptor: String,
    constructor_descriptor: String,
}

impl ValidatedSameBatchConstructorReference<'_, '_> {
    /// Original source unit containing the factory and returned class.
    pub const fn source_id(&self) -> SourceId {
        self.source
    }
    /// Exact source class identity returned by `create` and accepted by `identity`.
    pub const fn counter_class(&self) -> SymbolId {
        self.counter
    }
    /// Exact source class identity declaring both factory methods.
    pub const fn factory_class(&self) -> SymbolId {
        self.factory
    }
    /// Exact primary constructor selected by the typed `new` application.
    pub const fn constructor(&self) -> SymbolId {
        self.constructor
    }
    /// Exact source method identity for `create`.
    pub const fn create_method(&self) -> SymbolId {
        self.create
    }
    /// Exact factory-owned source formal passed to the constructor.
    pub const fn seed_parameter(&self) -> SymbolId {
        self.seed
    }
    /// Original source `New`, `Select`, and completed `Apply` roots.
    pub const fn source_roots(&self) -> (TreeId<Untyped>, TreeId<Untyped>, TreeId<Untyped>) {
        (self.source_new, self.source_select, self.source_apply)
    }
    /// Corresponding canonical typed `New`, `Select`, and completed `Apply` roots.
    pub const fn typed_roots(&self) -> (TreeId<Typed>, TreeId<Typed>, TreeId<Typed>) {
        (self.typed_new, self.typed_select, self.typed_apply)
    }
    /// Source and typed identities of the argument bound to the `seed` formal.
    pub const fn argument_roots(&self) -> (TreeId<Untyped>, TreeId<Typed>) {
        (self.source_argument, self.typed_argument)
    }
    /// The exact declared reference result type node, preserving its TypeId.
    pub const fn result_type(&self) -> TypeId {
        self.result_type
    }
    /// Exact JVM descriptor derived from the validated method type.
    pub fn method_descriptor(&self) -> &str {
        &self.method_descriptor
    }
    /// Exact JVM descriptor derived from the primary constructor.
    pub fn constructor_descriptor(&self) -> &str {
        &self.constructor_descriptor
    }
}

/// Independent proof for a direct reference parameter return. It carries no
/// constructor identity and cannot stand in for a constructor factory proof.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidatedSameBatchReferenceReturn {
    method: SymbolId,
    parameter: SymbolId,
    source_method: TreeId<Untyped>,
    source_rhs: TreeId<Untyped>,
    typed_rhs: TreeId<Typed>,
    result_type: TypeId,
    descriptor: String,
}

/// Original source evidence that accompanies one canonical JVM unit.
pub struct SameBatchSourceEvidence<'source> {
    /// Source identity recorded by the namer and typer.
    pub source: SourceId,
    /// Original parsed source AST from that session.
    pub source_ast: &'source AstArena<Untyped>,
    /// Canonical declaration/reference identities from that session.
    pub source_index: &'source SourceSemanticIndex,
    /// Exact source-to-typed root mappings from that session.
    pub typed_index: &'source SourceTypedIndex,
}

impl ValidatedSameBatchReferenceReturn {
    /// Exact declared source method symbol.
    pub const fn method(&self) -> SymbolId {
        self.method
    }
    /// Exact method-owned reference formal.
    pub const fn parameter(&self) -> SymbolId {
        self.parameter
    }
    /// Source method declaration and direct reference RHS.
    pub const fn source_roots(&self) -> (TreeId<Untyped>, TreeId<Untyped>) {
        (self.source_method, self.source_rhs)
    }
    /// Exact typed identifier root mapped from the source RHS.
    pub const fn typed_rhs(&self) -> TreeId<Typed> {
        self.typed_rhs
    }
    /// Exact declared result TypeId.
    pub const fn result_type(&self) -> TypeId {
        self.result_type
    }
    /// Exact JVM descriptor.
    pub fn descriptor(&self) -> &str {
        &self.descriptor
    }
}

/// Validate the canonical factory methods against the complete original source
/// unit and JVM plan. Class IDs are explicit inputs and are checked by identity;
/// a same-spelled class from another source or package cannot be substituted.
pub fn validate_same_batch_factory_result<'proof, 'unit>(
    unit: &'proof JvmCompilationUnit<'unit>,
    definitions: &Definitions,
    evidence: SameBatchSourceEvidence<'proof>,
    counter: SymbolId,
    factory: SymbolId,
) -> Result<ValidatedSameBatchConstructorReference<'proof, 'unit>, JvmTastyContractError> {
    let SameBatchSourceEvidence {
        source,
        source_ast,
        source_index,
        typed_index,
    } = evidence;
    let root = source_ast
        .iter()
        .find_map(|(id, tree)| matches!(tree.kind, TreeKind::PackageDef(_)).then_some(id))
        .ok_or_else(|| {
            mismatch(
                counter,
                None,
                None,
                "source unit",
                "missing package compilation-unit root",
            )
        })?;
    let TreeKind::PackageDef(package) = &source_ast.get(root).kind else {
        unreachable!()
    };
    if package.stats.len() != unit.classes.len() {
        return Err(mismatch(
            counter,
            None,
            Some(root),
            "complete source batch",
            "JVM unit does not contain every top-level source class",
        ));
    }
    for (tree, class_plan) in package.stats.iter().zip(&unit.classes) {
        let Some(TreeKind::TypeDef(definition)) = source_ast.try_get(*tree).map(|node| &node.kind)
        else {
            return Err(mismatch(
                class_plan.class,
                None,
                Some(*tree),
                "complete source batch",
                "all top-level siblings must be ordinary classes",
            ));
        };
        if !matches!(
            source_ast.try_get(definition.rhs).map(|node| &node.kind),
            Some(TreeKind::Template(_))
        ) || source_index.symbol_at(source, *tree) != Some(class_plan.class)
        {
            return Err(mismatch(
                class_plan.class,
                None,
                Some(*tree),
                "complete source batch",
                "source declaration identity/order differs from the JVM unit",
            ));
        }
    }
    if !unit.classes.iter().any(|plan| plan.class == counter)
        || !unit.classes.iter().any(|plan| plan.class == factory)
    {
        return Err(mismatch(
            counter,
            None,
            None,
            "batch class",
            "Counter and CounterFactory must both be planned in this complete JVM unit",
        ));
    }
    let metadata = unit
        .metadata(definitions)
        .map_err(JvmTastyContractError::JvmPlan)?;
    let counter_metadata = metadata
        .iter()
        .find(|m| m.identity().symbol() == counter)
        .ok_or_else(|| {
            mismatch(
                counter,
                None,
                None,
                "JVM metadata",
                "Counter metadata is absent",
            )
        })?;
    let factory_metadata = metadata
        .iter()
        .find(|m| m.identity().symbol() == factory)
        .ok_or_else(|| {
            mismatch(
                factory,
                None,
                None,
                "JVM metadata",
                "CounterFactory metadata is absent",
            )
        })?;
    validate_public_concrete_class(unit.store, source_index, source, counter)?;
    validate_public_concrete_class(unit.store, source_index, source, factory)?;
    if counter_metadata.identity().internal_name() != "example/Counter"
        || factory_metadata.identity().internal_name() != "example/CounterFactory"
    {
        return Err(mismatch(
            counter,
            None,
            None,
            "class path",
            "expected exact example.Counter and example.CounterFactory source roles",
        ));
    }

    let counter_source = source_class_tree(source_ast, source_index, source, counter)?;
    let Some(TreeKind::TypeDef(counter_def)) =
        source_ast.try_get(counter_source).map(|tree| &tree.kind)
    else {
        unreachable!()
    };
    let Some(TreeKind::Template(counter_template)) =
        source_ast.try_get(counter_def.rhs).map(|tree| &tree.kind)
    else {
        unreachable!()
    };
    let constructor = source_index
        .symbol_at(source, counter_template.constructor)
        .ok_or_else(|| {
            mismatch(
                counter,
                None,
                Some(counter_template.constructor),
                "primary constructor",
                "source index has no primary constructor identity",
            )
        })?;
    let Some(JvmClassInitializationPlan::PrimaryConstructor(jvm_constructor)) = unit
        .classes
        .iter()
        .find(|plan| plan.class == counter)
        .map(|plan| &plan.initialization)
    else {
        return Err(mismatch(
            counter,
            Some(constructor),
            Some(counter_template.constructor),
            "primary constructor",
            "Counter must have an explicit planned primary constructor",
        ));
    };
    if jvm_constructor.symbol() != constructor || !unit.store.symbols.contains(constructor) {
        return Err(mismatch(
            counter,
            Some(constructor),
            Some(counter_template.constructor),
            "primary constructor identity",
            "source and JVM plans do not share the canonical primary constructor",
        ));
    }
    let constructor_symbol = unit.store.symbols.get(constructor);
    let constructor_metadata = counter_metadata
        .constructor()
        .filter(|m| m.symbol() == constructor)
        .ok_or_else(|| {
            mismatch(
                counter,
                Some(constructor),
                Some(counter_template.constructor),
                "primary constructor",
                "constructor plan does not retain the source primary constructor",
            )
        })?;
    if constructor_symbol.kind != SymbolKind::Constructor
        || constructor_symbol.owner != Some(counter)
        || constructor_symbol.origin != SymbolOrigin::Source(source)
        || constructor_symbol.visibility != Visibility::Public
        || !constructor_metadata.access_flags().is_public()
        || constructor_metadata.descriptor().as_str() != "(I)V"
        || constructor_metadata.parameters().len() != 1
    {
        return Err(mismatch(
            counter,
            Some(constructor),
            Some(counter_template.constructor),
            "constructor identity/access/descriptor",
            "expected public source primary constructor Counter.<init>(Int)",
        ));
    }
    let ctor_type = complete_type(
        unit.store,
        constructor,
        counter,
        Some(constructor),
        counter_template.constructor,
    )?;
    let Type::Method(ctor_signature) = unit.store.types.try_get(ctor_type).ok_or_else(|| {
        mismatch(
            counter,
            Some(constructor),
            Some(counter_template.constructor),
            "constructor signature",
            "stale constructor type",
        )
    })?
    else {
        return Err(mismatch(
            counter,
            Some(constructor),
            Some(counter_template.constructor),
            "constructor signature",
            "generic constructor is unsupported",
        ));
    };
    let Some(TreeKind::DefDef(ctor_def)) = source_ast
        .try_get(counter_template.constructor)
        .map(|tree| &tree.kind)
    else {
        return Err(mismatch(
            counter,
            Some(constructor),
            Some(counter_template.constructor),
            "constructor source",
            "primary constructor source must be a DefDef",
        ));
    };
    if !ctor_def.type_params.is_empty()
        || ctor_def.value_param_clauses.len() != 1
        || ctor_def.value_param_clauses[0].len() != 1
    {
        return Err(mismatch(
            counter,
            Some(constructor),
            Some(counter_template.constructor),
            "constructor signature",
            "expected one non-generic primary-constructor clause with one formal",
        ));
    }
    let constructor_formal_tree = ctor_def.value_param_clauses[0][0];
    let constructor_formal = source_index
        .derived_symbol_at(constructor, source, constructor_formal_tree)
        .ok_or_else(|| {
            mismatch(
                counter,
                Some(constructor),
                Some(constructor_formal_tree),
                "constructor formal",
                "source index has no primary-constructor formal identity",
            )
        })?;
    if !unit.store.symbols.contains(constructor_formal)
        || constructor_metadata.parameters()[0].symbol() != constructor_formal
    {
        return Err(mismatch(
            counter,
            Some(constructor),
            Some(constructor_formal_tree),
            "constructor formal identity",
            "source and JVM plans do not share the derived primary-constructor formal",
        ));
    }
    let constructor_formal_info = unit.store.symbols.get(constructor_formal);
    if ctor_signature.kind != MethodKind::Plain
        || ctor_signature.params.len() != 1
        || ctor_signature.params[0].ty != definitions.int
        || ctor_signature.params[0].erased
        || ctor_signature.params[0].varargs
        || !matches!(source_ast.try_get(constructor_formal_tree).map(|tree| &tree.kind), Some(TreeKind::ValDef(parameter)) if parameter.rhs.is_none())
        || constructor_formal_info.kind != SymbolKind::Parameter
        || constructor_formal_info.owner != Some(constructor)
        || constructor_formal_info.origin != SymbolOrigin::Source(source)
        || constructor_formal_info.info != SymbolInfo::Complete(definitions.int)
    {
        return Err(mismatch(
            counter,
            Some(constructor),
            Some(counter_template.constructor),
            "constructor signature",
            "expected one plain Int primary-constructor formal",
        ));
    }

    let context = FactoryProofContext {
        unit,
        definitions,
        source,
        source_ast,
        source_index,
        typed_index,
        counter,
        factory,
        factory_metadata,
        counter_metadata,
    };

    let create = declared_method(
        unit.store,
        source_ast,
        source_index,
        source,
        factory,
        "create",
    )?;
    let (create_typed, create_result, create_descriptor) =
        validate_method_signature(&context, create, "seed", true)?;
    let constructor_body =
        validate_constructor_body(&context, constructor, create, create_typed, create_result)?;
    Ok(ValidatedSameBatchConstructorReference {
        _unit: unit,
        _source_ast: source_ast,
        _source_index: source_index,
        _typed_index: typed_index,
        source,
        counter,
        factory,
        constructor,
        create: create.method,
        seed: constructor_body.seed,
        source_new: constructor_body.source_new,
        source_select: constructor_body.source_select,
        source_apply: constructor_body.source_apply,
        source_argument: constructor_body.source_argument,
        typed_new: constructor_body.typed_new,
        typed_select: constructor_body.typed_select,
        typed_apply: constructor_body.typed_apply,
        typed_argument: constructor_body.typed_argument,
        result_type: create_result,
        method_descriptor: create_descriptor,
        constructor_descriptor: constructor_metadata.descriptor().as_str().to_owned(),
    })
}

/// Independently validate a direct reference-return method in the same source
/// and JVM batch. This proof does not require or imply a constructor call.
pub fn validate_same_batch_reference_return<'proof, 'unit>(
    unit: &'proof JvmCompilationUnit<'unit>,
    definitions: &'proof Definitions,
    evidence: SameBatchSourceEvidence<'proof>,
    counter: SymbolId,
    factory: SymbolId,
) -> Result<ValidatedSameBatchReferenceReturn, JvmTastyContractError> {
    let SameBatchSourceEvidence {
        source,
        source_ast,
        source_index,
        typed_index,
    } = evidence;
    let root = source_ast
        .iter()
        .find_map(|(id, tree)| matches!(tree.kind, TreeKind::PackageDef(_)).then_some(id))
        .ok_or_else(|| mismatch(counter, None, None, "source unit", "missing package root"))?;
    let TreeKind::PackageDef(package) = &source_ast.get(root).kind else {
        unreachable!()
    };
    if package.stats.len() != unit.classes.len() {
        return Err(mismatch(
            counter,
            None,
            Some(root),
            "complete source batch",
            "JVM unit does not contain every top-level source class",
        ));
    }
    for (tree, class_plan) in package.stats.iter().zip(&unit.classes) {
        let Some(TreeKind::TypeDef(_)) = source_ast.try_get(*tree).map(|node| &node.kind) else {
            return Err(mismatch(
                class_plan.class,
                None,
                Some(*tree),
                "complete source batch",
                "all top-level siblings must be ordinary classes",
            ));
        };
        if source_index.symbol_at(source, *tree) != Some(class_plan.class) {
            return Err(mismatch(
                class_plan.class,
                None,
                Some(*tree),
                "complete source batch",
                "source declaration identity/order differs from the JVM unit",
            ));
        }
    }
    if !unit.classes.iter().any(|plan| plan.class == counter)
        || !unit.classes.iter().any(|plan| plan.class == factory)
    {
        return Err(mismatch(
            counter,
            None,
            None,
            "batch class",
            "Counter and CounterFactory must both be planned in this complete JVM unit",
        ));
    }
    let metadata = unit
        .metadata(definitions)
        .map_err(JvmTastyContractError::JvmPlan)?;
    let class_metadata = |class: SymbolId, role: &'static str| {
        metadata
            .iter()
            .find(|item| item.identity().symbol() == class)
            .ok_or_else(|| mismatch(class, None, None, "JVM metadata", role))
    };
    let counter_metadata = class_metadata(counter, "Counter metadata is absent")?;
    let factory_metadata = class_metadata(factory, "CounterFactory metadata is absent")?;
    validate_public_concrete_class(unit.store, source_index, source, counter)?;
    validate_public_concrete_class(unit.store, source_index, source, factory)?;
    if counter_metadata.identity().internal_name() != "example/Counter"
        || factory_metadata.identity().internal_name() != "example/CounterFactory"
    {
        return Err(mismatch(
            counter,
            None,
            None,
            "class path",
            "expected exact example.Counter and example.CounterFactory source roles",
        ));
    }
    let context = FactoryProofContext {
        unit,
        definitions,
        source,
        source_ast,
        source_index,
        typed_index,
        counter,
        factory,
        factory_metadata,
        counter_metadata,
    };
    let identity = declared_method(
        unit.store,
        source_ast,
        source_index,
        source,
        factory,
        "identity",
    )?;
    let (typed_rhs, result_type, descriptor) =
        validate_method_signature(&context, identity, "counter", false)?;
    validate_reference_return(&context, identity, typed_rhs, result_type, descriptor)
}

#[derive(Clone, Copy)]
struct MethodSource {
    method: SymbolId,
    declaration: TreeId<Untyped>,
    rhs: TreeId<Untyped>,
    parameter_tree: TreeId<Untyped>,
    parameter: SymbolId,
}

struct FactoryProofContext<'proof, 'unit> {
    unit: &'proof JvmCompilationUnit<'unit>,
    definitions: &'proof Definitions,
    source: SourceId,
    source_ast: &'proof AstArena<Untyped>,
    source_index: &'proof SourceSemanticIndex,
    typed_index: &'proof SourceTypedIndex,
    counter: SymbolId,
    factory: SymbolId,
    factory_metadata: &'proof JvmClassMetadata,
    counter_metadata: &'proof JvmClassMetadata,
}

struct ConstructorBodyProof {
    seed: SymbolId,
    source_new: TreeId<Untyped>,
    source_select: TreeId<Untyped>,
    source_apply: TreeId<Untyped>,
    source_argument: TreeId<Untyped>,
    typed_new: TreeId<Typed>,
    typed_select: TreeId<Typed>,
    typed_apply: TreeId<Typed>,
    typed_argument: TreeId<Typed>,
}

fn declared_method(
    store: &SemanticStore,
    ast: &AstArena<Untyped>,
    index: &SourceSemanticIndex,
    source: SourceId,
    owner: SymbolId,
    name: &str,
) -> Result<MethodSource, JvmTastyContractError> {
    let owner_tree = source_class_tree(ast, index, source, owner)?;
    let Some(TreeKind::TypeDef(class_definition)) = ast.try_get(owner_tree).map(|tree| &tree.kind)
    else {
        unreachable!()
    };
    let Some(TreeKind::Template(template)) =
        ast.try_get(class_definition.rhs).map(|tree| &tree.kind)
    else {
        unreachable!()
    };
    let mut found = Vec::new();
    for tree in &template.body {
        let Some(TreeKind::DefDef(definition)) = ast.try_get(*tree).map(|node| &node.kind) else {
            continue;
        };
        if store.names.try_resolve(definition.name.as_name().text()) == Some(name) {
            found.push((*tree, definition));
        }
    }
    if found.len() != 1 {
        return Err(mismatch(
            owner,
            None,
            found.first().map(|(tree, _)| *tree),
            "source method",
            "expected exactly one direct source method for this role",
        ));
    }
    let (declaration, definition) = found[0];
    let method = index.symbol_at(source, declaration).ok_or_else(|| {
        mismatch(
            owner,
            None,
            Some(declaration),
            "source method identity",
            "source index has no method identity",
        )
    })?;
    if !store.symbols.contains(method) {
        return Err(mismatch(
            owner,
            Some(method),
            Some(declaration),
            "source method identity",
            "source index refers to a method outside the semantic store",
        ));
    }
    let Some(rhs) = definition.rhs else {
        return Err(mismatch(
            owner,
            Some(method),
            Some(declaration),
            "source method",
            "method has no source body",
        ));
    };
    if !definition.type_params.is_empty()
        || definition.value_param_clauses.len() != 1
        || definition.value_param_clauses[0].len() != 1
        || definition
            .source_param_clause_order
            .as_ref()
            .is_some_and(|order| order.len() != 1)
    {
        return Err(mismatch(
            owner,
            Some(method),
            Some(declaration),
            "source signature",
            "expected one non-generic plain value-parameter clause",
        ));
    }
    let parameter_tree = definition.value_param_clauses[0][0];
    if !matches!(ast.try_get(parameter_tree).map(|node| &node.kind), Some(TreeKind::ValDef(parameter)) if parameter.rhs.is_none())
    {
        return Err(mismatch(
            owner,
            Some(method),
            Some(parameter_tree),
            "source formal",
            "default-valued parameters are unsupported",
        ));
    }
    let parameter = index.symbol_at(source, parameter_tree).ok_or_else(|| {
        mismatch(
            owner,
            Some(method),
            Some(parameter_tree),
            "source formal",
            "source index has no formal identity",
        )
    })?;
    if !store.symbols.contains(parameter) {
        return Err(mismatch(
            owner,
            Some(method),
            Some(parameter_tree),
            "source formal identity",
            "source index refers to a formal outside the semantic store",
        ));
    }
    Ok(MethodSource {
        method,
        declaration,
        rhs,
        parameter_tree,
        parameter,
    })
}

fn validate_method_signature(
    context: &FactoryProofContext<'_, '_>,
    method: MethodSource,
    formal_name: &str,
    int_formal: bool,
) -> Result<(TreeId<Typed>, TypeId, String), JvmTastyContractError> {
    let unit = context.unit;
    let definitions = context.definitions;
    let index = context.source_index;
    let typed_index = context.typed_index;
    let source = context.source;
    let factory_metadata = context.factory_metadata;
    let counter_metadata = context.counter_metadata;
    let factory = context.factory;
    let counter = context.counter;
    let fail = |stage, reason| {
        mismatch(
            factory,
            Some(method.method),
            Some(method.declaration),
            stage,
            reason,
        )
    };
    if !unit.store.symbols.contains(method.method) || !unit.store.symbols.contains(method.parameter)
    {
        return Err(fail(
            "method identity",
            "source index method or formal is outside the semantic store",
        ));
    }
    let info = unit.store.symbols.get(method.method);
    if info.kind != SymbolKind::Method
        || info.owner != Some(factory)
        || info.origin != SymbolOrigin::Source(source)
        || info.visibility != Visibility::Public
        || info.flags.contains(SymbolFlags::ABSTRACT)
    {
        return Err(fail(
            "method identity/access",
            "method must be public, concrete, and owned by CounterFactory",
        ));
    }
    let method_plan = unit
        .classes
        .iter()
        .find(|class| class.class == factory)
        .and_then(|class| {
            class
                .methods
                .iter()
                .find(|candidate| candidate.method == method.method)
        })
        .ok_or_else(|| {
            fail(
                "JVM method plan",
                "method is not explicitly planned in the complete JVM unit",
            )
        })?;
    let method_metadata = factory_metadata
        .methods()
        .iter()
        .find(|candidate| candidate.symbol() == method.method)
        .ok_or_else(|| fail("JVM method metadata", "method metadata is absent"))?;
    if !method_metadata.access_flags().is_public() || method_metadata.access_flags().is_abstract() {
        return Err(fail(
            "JVM method access",
            "JVM method metadata must be public and concrete",
        ));
    }
    let method_type = complete_type(
        unit.store,
        method.method,
        factory,
        Some(method.method),
        method.declaration,
    )?;
    let Type::Method(signature) = unit
        .store
        .types
        .try_get(method_type)
        .ok_or_else(|| fail("method signature", "stale method type"))?
    else {
        return Err(fail(
            "method signature",
            "expected completed plain MethodType without type parameters",
        ));
    };
    let formal_type_ok = if int_formal {
        signature
            .params
            .first()
            .is_some_and(|param| param.ty == definitions.int)
    } else {
        signature
            .params
            .first()
            .is_some_and(|param| is_exact_reference(unit.store, definitions, param.ty, counter))
    };
    if signature.kind != MethodKind::Plain
        || signature.params.len() != 1
        || signature.params[0].erased
        || signature.params[0].varargs
        || !formal_type_ok
    {
        return Err(fail(
            "method signature",
            "expected exactly one plain Int or exact Counter reference formal",
        ));
    }
    let formal = unit.store.symbols.get(method.parameter);
    if formal.kind != SymbolKind::Parameter
        || formal.owner != Some(method.method)
        || formal.origin != SymbolOrigin::Source(source)
        || formal.info != SymbolInfo::Complete(signature.params[0].ty)
        || unit.store.names.try_resolve(formal.name.text()) != Some(formal_name)
        || index.symbol_at(source, method.parameter_tree) != Some(method.parameter)
    {
        return Err(fail(
            "formal identity",
            "parameter identity, owner, source name, or exact type is wrong",
        ));
    }
    if !is_exact_reference(unit.store, definitions, signature.result, counter) {
        return Err(fail(
            "method result",
            "declared result must be exact TypeRef(Symbol(Counter))",
        ));
    }
    let typed_rhs = typed_index.get(source, method.rhs).ok_or_else(|| {
        fail(
            "source-to-typed binding",
            "source RHS has no typed replacement in this session",
        )
    })?;
    if method_plan.body != Some(typed_rhs) {
        return Err(fail(
            "JVM body binding",
            "JVM method body differs from the exact source-mapped typed RHS",
        ));
    }
    let typed_root = unit
        .typed_ast
        .try_get(typed_rhs)
        .ok_or_else(|| fail("typed RHS", "mapped typed root is stale"))?;
    let typed_result_matches = if int_formal {
        is_exact_reference(unit.store, definitions, typed_root.ty, counter)
    } else {
        matches!(unit.store.types.try_get(typed_root.ty), Some(Type::TermRef { target: TermRefTarget::Symbol(target), .. }) if *target == method.parameter)
    };
    if !typed_result_matches {
        return Err(fail(
            "typed result",
            "typed RHS is not the exact declared Counter reference result",
        ));
    }
    let descriptor = if int_formal {
        format!("(I)L{};", counter_metadata.identity().internal_name())
    } else {
        format!(
            "(L{};)L{};",
            counter_metadata.identity().internal_name(),
            counter_metadata.identity().internal_name()
        )
    };
    if method_metadata.descriptor().as_str() != descriptor {
        return Err(fail(
            "JVM descriptor",
            "descriptor differs from exact reference signature",
        ));
    }
    Ok((typed_rhs, signature.result, descriptor))
}

fn validate_constructor_body(
    context: &FactoryProofContext<'_, '_>,
    constructor: SymbolId,
    method: MethodSource,
    typed_rhs: TreeId<Typed>,
    result_type: TypeId,
) -> Result<ConstructorBodyProof, JvmTastyContractError> {
    let unit = context.unit;
    let definitions = context.definitions;
    let ast = context.source_ast;
    let index = context.source_index;
    let typed_index = context.typed_index;
    let source = context.source;
    let factory = context.factory;
    let counter = context.counter;
    let fail = |stage, reason| JvmTastyContractError::FactoryReferenceMismatch {
        class: factory,
        method: Some(method.method),
        constructor: Some(constructor),
        source_tree: Some(method.declaration),
        typed_tree: Some(typed_rhs),
        stage,
        reason,
    };
    let root = unit
        .typed_ast
        .try_get(typed_rhs)
        .ok_or_else(|| fail("create typed shape", "typed RHS is stale"))?;
    let TreeKind::Apply(application) = &root.kind else {
        return Err(fail(
            "create typed shape",
            "expected Apply(Select(New(Counter), <init>), [seed])",
        ));
    };
    if application.kind != ApplyKind::Regular || application.args.len() != 1 {
        return Err(fail(
            "create typed shape",
            "constructor application must have one ordinary argument",
        ));
    }
    let typed_select = application.function;
    let select_tree = unit
        .typed_ast
        .try_get(typed_select)
        .ok_or_else(|| fail("create typed shape", "selected function is stale"))?;
    let TreeKind::Select(select) = &select_tree.kind else {
        return Err(fail(
            "create typed shape",
            "constructor function must be Select",
        ));
    };
    let typed_new = select.qualifier;
    let new_tree = unit
        .typed_ast
        .try_get(typed_new)
        .ok_or_else(|| fail("create typed shape", "New tree is stale"))?;
    let TreeKind::New(typed_new_node) = &new_tree.kind else {
        return Err(fail(
            "create typed shape",
            "constructor receiver must retain a typed New node",
        ));
    };
    let typed_new_type = unit
        .typed_ast
        .try_get(typed_new_node.tpt)
        .ok_or_else(|| fail("create typed shape", "typed New type tree is stale"))?;
    if !is_exact_reference(unit.store, definitions, typed_new_type.ty, counter)
        || !is_exact_reference(unit.store, definitions, new_tree.ty, counter)
        || !matches!(unit.store.types.try_get(select_tree.ty), Some(Type::TermRef { target: TermRefTarget::Symbol(target), .. }) if *target == constructor)
        || !is_exact_reference(unit.store, definitions, root.ty, counter)
        || !is_exact_reference(unit.store, definitions, result_type, counter)
    {
        return Err(fail(
            "create selected target",
            "typed New/result and selected constructor must retain exact Counter identities",
        ));
    }
    let argument = application.args[0];
    let argument_tree = unit
        .typed_ast
        .try_get(argument)
        .ok_or_else(|| fail("create argument", "typed argument is stale"))?;
    if !matches!(&argument_tree.kind, TreeKind::Ident(_))
        || !matches!(unit.store.types.try_get(argument_tree.ty), Some(Type::TermRef { target: TermRefTarget::Symbol(target), .. }) if *target == method.parameter)
    {
        return Err(fail(
            "create argument",
            "constructor argument must be the exact factory-owned seed formal",
        ));
    }
    let Some(TreeKind::Apply(source_apply_node)) = ast.try_get(method.rhs).map(|tree| &tree.kind)
    else {
        return Err(fail("create source shape", "source RHS must be Apply"));
    };
    let Some(TreeKind::Select(source_select_node)) = ast
        .try_get(source_apply_node.function)
        .map(|tree| &tree.kind)
    else {
        return Err(fail(
            "create source shape",
            "source constructor function must be Select",
        ));
    };
    let Some(TreeKind::New(source_new_node)) = ast
        .try_get(source_select_node.qualifier)
        .map(|tree| &tree.kind)
    else {
        return Err(fail(
            "create source shape",
            "source constructor receiver must be New",
        ));
    };
    let counter_name = unit.store.symbols.get(counter).name.text();
    if !matches!(ast.try_get(source_new_node.tpt).map(|tree| &tree.kind), Some(TreeKind::Ident(type_name)) if unit.store.names.try_resolve(type_name.name.text()) == unit.store.names.try_resolve(counter_name))
    {
        return Err(fail(
            "create source target",
            "source New type must name the exact Counter class selected by the typed New",
        ));
    }
    if !matches!(
        unit.store.names.try_resolve(source_select_node.name.text()),
        Some("<init>")
    ) || source_select_node.backquoted
        || source_apply_node.kind != ApplyKind::Regular
        || source_apply_node.args.len() != 1
        || index
            .symbol_at(source, source_apply_node.args[0])
            .is_some_and(|symbol| symbol != method.parameter)
        || !matches!(ast.try_get(source_apply_node.args[0]).map(|tree| &tree.kind), Some(TreeKind::Ident(ident)) if unit.store.names.try_resolve(ident.name.text()) == unit.store.names.try_resolve(unit.store.symbols.get(method.parameter).name.text()))
        || typed_index.get(source, method.rhs) != Some(typed_rhs)
    {
        return Err(fail(
            "create source-to-typed binding",
            "source New/Select/Apply/seed roots do not map to exact typed roots",
        ));
    }
    Ok(ConstructorBodyProof {
        seed: method.parameter,
        source_new: source_select_node.qualifier,
        source_select: source_apply_node.function,
        source_apply: method.rhs,
        source_argument: source_apply_node.args[0],
        typed_new,
        typed_select,
        typed_apply: typed_rhs,
        typed_argument: argument,
    })
}

fn validate_reference_return(
    context: &FactoryProofContext<'_, '_>,
    method: MethodSource,
    typed_rhs: TreeId<Typed>,
    result_type: TypeId,
    descriptor: String,
) -> Result<ValidatedSameBatchReferenceReturn, JvmTastyContractError> {
    let unit = context.unit;
    let definitions = context.definitions;
    let ast = context.source_ast;
    let index = context.source_index;
    let typed_index = context.typed_index;
    let source = context.source;
    let factory = context.factory;
    let counter = context.counter;
    let fail = |stage, reason| {
        mismatch(
            factory,
            Some(method.method),
            Some(method.declaration),
            stage,
            reason,
        )
    };
    let root = unit
        .typed_ast
        .try_get(typed_rhs)
        .ok_or_else(|| fail("identity typed RHS", "typed root is stale"))?;
    if !matches!(&root.kind, TreeKind::Ident(_))
        || !is_exact_reference(unit.store, definitions, result_type, counter)
        || !matches!(unit.store.types.try_get(root.ty), Some(Type::TermRef { target: TermRefTarget::Symbol(target), .. }) if *target == method.parameter)
        || !matches!(
            ast.try_get(method.rhs).map(|tree| &tree.kind),
            Some(TreeKind::Ident(_))
        )
        || index
            .symbol_at(source, method.rhs)
            .is_some_and(|symbol| symbol != method.parameter)
        || !matches!(ast.try_get(method.rhs).map(|tree| &tree.kind), Some(TreeKind::Ident(ident)) if unit.store.names.try_resolve(ident.name.text()) == unit.store.names.try_resolve(unit.store.symbols.get(method.parameter).name.text()))
        || typed_index.get(source, method.rhs) != Some(typed_rhs)
        || !matches!(ast.try_get(method.declaration).map(|tree| &tree.kind), Some(TreeKind::DefDef(definition)) if definition.value_param_clauses.first().is_some_and(|clause| clause.first() == Some(&method.parameter_tree)))
    {
        return Err(fail(
            "identity source/typed reference",
            "identity must return the exact reference formal directly",
        ));
    }
    Ok(ValidatedSameBatchReferenceReturn {
        method: method.method,
        parameter: method.parameter,
        source_method: method.declaration,
        source_rhs: method.rhs,
        typed_rhs,
        result_type,
        descriptor,
    })
}

fn validate_public_concrete_class(
    store: &SemanticStore,
    index: &SourceSemanticIndex,
    source: SourceId,
    class: SymbolId,
) -> Result<(), JvmTastyContractError> {
    let info = store.symbols.get(class);
    if info.kind != SymbolKind::Class
        || info.visibility != Visibility::Public
        || info.flags.contains(SymbolFlags::ABSTRACT)
        || info.origin != SymbolOrigin::Source(source)
        || index.definition_of(class).is_none()
    {
        return Err(mismatch(
            class,
            None,
            None,
            "class identity/access",
            "class must be original, public, concrete, ordinary, and source-declared",
        ));
    }
    Ok(())
}

fn source_class_tree(
    ast: &AstArena<Untyped>,
    index: &SourceSemanticIndex,
    source: SourceId,
    class: SymbolId,
) -> Result<TreeId<Untyped>, JvmTastyContractError> {
    ast.iter()
        .find_map(|(tree, node)| {
            (matches!(node.kind, TreeKind::TypeDef(_))
                && index.symbol_at(source, tree) == Some(class))
            .then_some(tree)
        })
        .ok_or_else(|| {
            mismatch(
                class,
                None,
                None,
                "source class declaration",
                "canonical class has no matching declaration in this source",
            )
        })
}

fn complete_type(
    store: &SemanticStore,
    symbol: SymbolId,
    class: SymbolId,
    method: Option<SymbolId>,
    tree: TreeId<Untyped>,
) -> Result<TypeId, JvmTastyContractError> {
    if !store.symbols.contains(symbol) {
        return Err(mismatch(
            class,
            method,
            Some(tree),
            "completed semantic signature",
            "source index refers to a symbol outside the semantic store",
        ));
    }
    match store.symbols.get(symbol).info {
        SymbolInfo::Complete(ty) if store.types.contains(ty) => Ok(ty),
        _ => Err(mismatch(
            class,
            method,
            Some(tree),
            "completed semantic signature",
            "symbol has no complete canonical type",
        )),
    }
}

fn is_exact_reference(
    store: &SemanticStore,
    definitions: &Definitions,
    ty: TypeId,
    class: SymbolId,
) -> bool {
    let Some(Type::TypeRef {
        prefix,
        target: TypeRefTarget::Symbol(target),
    }) = store.types.try_get(ty)
    else {
        return false;
    };
    if *target != class {
        return false;
    }
    if *prefix == definitions.no_prefix {
        return true;
    }
    let Some(package) = store
        .symbols
        .contains(class)
        .then(|| store.symbols.get(class).owner)
        .flatten()
    else {
        return false;
    };
    matches!(
        store.types.try_get(*prefix),
        Some(Type::TypeRef {
            prefix: package_prefix,
            target: TypeRefTarget::Symbol(package_target),
        }) if *package_prefix == definitions.no_prefix && *package_target == package
    )
}

fn mismatch(
    class: SymbolId,
    method: Option<SymbolId>,
    source_tree: Option<TreeId<Untyped>>,
    stage: &'static str,
    reason: &'static str,
) -> JvmTastyContractError {
    JvmTastyContractError::FactoryReferenceMismatch {
        class,
        method,
        constructor: None,
        source_tree,
        typed_tree: None,
        stage,
        reason,
    }
}
