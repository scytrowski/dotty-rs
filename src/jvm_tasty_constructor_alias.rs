//! Read-only, one-session proof for the Scala 3 constructor-reference alias.
//!
//! The proof preserves the source and typed identities needed by later TASTy
//! and JVM work. It does not create replacement symbols or publish bytes.

use std::fmt;

use dotty_core::ast::{AstArena, Modifier, TreeKind, Typed, TypedAst, Untyped, VisibilitySyntax};
use dotty_core::symbols::{SymbolFlags, SymbolInfo, SymbolKind, SymbolOrigin, Visibility};
use dotty_core::types::{ClassInfo, TermRefTarget, Type, TypeRefTarget};
use dotty_core::{
    Definitions, Packages, SemanticStore, SourceDefinition, SourceId, SourceSemanticIndex,
    SourceTypedIndex, SymbolId, TreeId,
};
use dotty_lexer::ContextualScanner;
use dotty_namer::{NamingOutcome, SourceParseStatus, name_compilation_unit_with_diagnostics};
use dotty_parser::parse_compilation_unit;
use dotty_typer::SourceTyper;

/// Contextual failures for the bounded constructor-reference alias proof.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConstructorReferenceAliasError {
    /// Source does not have the exact two-class shape required by this proof.
    SourceShape(&'static str),
    /// A declaration does not map to its original canonical source identity.
    SourceIdentity(&'static str),
    /// Owner, kind, visibility, origin, or completed symbol info is invalid.
    SemanticIdentity(&'static str),
    /// A type does not resolve to the exact Counter class identity.
    TypeIdentity(&'static str),
    /// The original source tree does not have the exact typed binding/edge.
    TypedBinding(&'static str),
    /// The complete source batch does not establish the required ABI shape.
    AbiShape(&'static str),
}

impl fmt::Display for ConstructorReferenceAliasError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "constructor-reference alias proof failed: {self:?}")
    }
}

impl std::error::Error for ConstructorReferenceAliasError {}

/// Immutable receipt retaining the original canonical symbols and source/typed
/// edges for `CounterOwner(counter: Counter)` and `inner: Counter = counter`.
pub struct ValidatedConstructorReferenceAlias<'session> {
    _store: &'session SemanticStore,
    _source_ast: &'session AstArena<Untyped>,
    _typed_ast: &'session TypedAst,
    _source_index: &'session SourceSemanticIndex,
    _source_typed_index: &'session SourceTypedIndex,
    definitions: Definitions,
    source_root: TreeId<Untyped>,
    source_id: SourceId,
    counter_class: SymbolId,
    owner_class: SymbolId,
    constructor: SymbolId,
    template_parameter: SymbolId,
    constructor_parameter: SymbolId,
    field: SymbolId,
    read_method: SymbolId,
    counter_get: SymbolId,
    source_initializer: TreeId<Untyped>,
    typed_initializer: TreeId<Typed>,
    source_read: TreeId<Untyped>,
    typed_read: TreeId<Typed>,
    constructor_descriptor: String,
    field_descriptor: String,
}

/// Borrowed session views retained by the receipt for later producer planning.
pub struct ConstructorReferenceAliasSession<'session> {
    /// Canonical semantic symbols and types from the original frontend run.
    pub store: &'session SemanticStore,
    /// Original source trees.
    pub source_ast: &'session AstArena<Untyped>,
    /// Original typed trees.
    pub typed_ast: &'session TypedAst,
    /// Canonical source declaration identities.
    pub source_index: &'session SourceSemanticIndex,
    /// Exact source-to-typed root mappings.
    pub source_typed_index: &'session SourceTypedIndex,
    /// Canonical builtin identities from this session.
    pub definitions: Definitions,
    /// Original complete package root.
    pub source_root: TreeId<Untyped>,
}

impl ValidatedConstructorReferenceAlias<'_> {
    pub const fn source_id(&self) -> SourceId {
        self.source_id
    }
    pub const fn counter_class(&self) -> SymbolId {
        self.counter_class
    }
    pub const fn owner_class(&self) -> SymbolId {
        self.owner_class
    }
    pub const fn constructor(&self) -> SymbolId {
        self.constructor
    }
    pub const fn template_parameter(&self) -> SymbolId {
        self.template_parameter
    }
    pub const fn constructor_parameter(&self) -> SymbolId {
        self.constructor_parameter
    }
    pub const fn field(&self) -> SymbolId {
        self.field
    }
    pub const fn read_method(&self) -> SymbolId {
        self.read_method
    }
    pub const fn counter_get(&self) -> SymbolId {
        self.counter_get
    }
    pub const fn source_initializer(&self) -> TreeId<Untyped> {
        self.source_initializer
    }
    pub const fn typed_initializer(&self) -> TreeId<Typed> {
        self.typed_initializer
    }
    pub const fn source_read(&self) -> TreeId<Untyped> {
        self.source_read
    }
    pub const fn typed_read(&self) -> TreeId<Typed> {
        self.typed_read
    }
    pub fn constructor_descriptor(&self) -> &str {
        &self.constructor_descriptor
    }
    pub fn field_descriptor(&self) -> &str {
        &self.field_descriptor
    }
    /// JVM descriptor for the checked `Counter.get()` selection.
    pub const fn read_target_descriptor(&self) -> &'static str {
        "()I"
    }
    /// Exact semantic session whose canonical IDs are retained by this proof.
    pub fn session(&self) -> ConstructorReferenceAliasSession<'_> {
        ConstructorReferenceAliasSession {
            store: self._store,
            source_ast: self._source_ast,
            typed_ast: self._typed_ast,
            source_index: self._source_index,
            source_typed_index: self._source_typed_index,
            definitions: self.definitions,
            source_root: self.source_root,
        }
    }
}

/// Parse, name, and type the complete source once, then pass an immutable
/// canonical proof to a callback while all original session stores are alive.
/// The accepted source shape is deliberately fixed to `Counter` followed by
/// `CounterOwner` in one package. No JVM/TASTy artifacts are produced.
pub fn with_constructor_reference_alias_proof<R>(
    source_text: &str,
    source_name: &str,
    source_id: SourceId,
    consume: impl for<'session> FnOnce(ValidatedConstructorReferenceAlias<'session>) -> R,
) -> Result<R, ConstructorReferenceAliasError> {
    let source = dotty_core::SourceText::new(source_text)
        .map_err(|_| ConstructorReferenceAliasError::SourceShape("invalid source text"))?;
    let scanner = ContextualScanner::new(source_text)
        .map_err(|_| ConstructorReferenceAliasError::SourceShape("lexing failed"))?;
    if !scanner.diagnostics().is_empty() {
        return Err(ConstructorReferenceAliasError::SourceShape(
            "lexical diagnostics",
        ));
    }
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let parsed = parse_compilation_unit(source, source_id, scanner, &mut store.names);
    if !parsed.diagnostics.is_empty() {
        return Err(ConstructorReferenceAliasError::SourceShape(
            "parser diagnostics",
        ));
    }
    let mut packages = Packages::new();
    let source_index = match name_compilation_unit_with_diagnostics(
        &parsed.ast,
        parsed.root,
        source_id,
        source_name,
        SourceParseStatus::Clean,
        &mut store,
        &mut packages,
    )
    .map_err(|_| ConstructorReferenceAliasError::SourceShape("naming failed"))?
    {
        NamingOutcome::Named { index, diagnostics } if diagnostics.is_empty() => index,
        _ => {
            return Err(ConstructorReferenceAliasError::SourceShape(
                "namer diagnostics",
            ));
        }
    };
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source_id,
        &source_index,
        &mut store,
        definitions,
        &packages,
    );
    let proof_parts = type_and_validate_source(
        source_id,
        parsed.root,
        &parsed.ast,
        &source_index,
        &mut typer,
    )?;
    let proof = finish_proof(ProofInput {
        source_id,
        definitions,
        source_root: parsed.root,
        store: typer.store(),
        source_ast: &parsed.ast,
        typed_ast: typer.typed_ast(),
        source_index: &source_index,
        source_typed_index: typer.source_typed_index(),
        parts: proof_parts,
    })?;
    Ok(consume(proof))
}

struct ProofParts {
    counter: SymbolId,
    owner: SymbolId,
    constructor: SymbolId,
    template_parameter: SymbolId,
    constructor_parameter: SymbolId,
    field: SymbolId,
    read: SymbolId,
    get: SymbolId,
    initializer: TreeId<Untyped>,
    read_rhs: TreeId<Untyped>,
}

struct ProofInput<'session> {
    source_id: SourceId,
    definitions: Definitions,
    source_root: TreeId<Untyped>,
    store: &'session SemanticStore,
    source_ast: &'session AstArena<Untyped>,
    typed_ast: &'session TypedAst,
    source_index: &'session SourceSemanticIndex,
    source_typed_index: &'session SourceTypedIndex,
    parts: ProofParts,
}

fn type_and_validate_source(
    source_id: SourceId,
    root: TreeId<Untyped>,
    ast: &AstArena<Untyped>,
    index: &SourceSemanticIndex,
    typer: &mut SourceTyper<'_>,
) -> Result<ProofParts, ConstructorReferenceAliasError> {
    let fail = ConstructorReferenceAliasError::SourceShape;
    let TreeKind::PackageDef(package) = &ast.get(root).kind else {
        return Err(fail("root must be package"));
    };
    let [counter_tree, owner_tree] = package.stats.as_slice() else {
        return Err(fail("batch must contain exactly Counter and CounterOwner"));
    };
    let (counter_name, counter, counter_template, counter_ctor) =
        class_parts(*counter_tree, source_id, ast, index, &typer.store().names)?;
    let (owner_name, owner, owner_template, constructor) =
        class_parts(*owner_tree, source_id, ast, index, &typer.store().names)?;
    if counter_name != "Counter" || owner_name != "CounterOwner" {
        return Err(fail("batch declaration order/names"));
    }
    for (tree, symbol) in [(*counter_tree, counter), (*owner_tree, owner)] {
        let TreeKind::TypeDef(definition) = &ast.get(tree).kind else {
            unreachable!()
        };
        if definition.metadata.modifiers.iter().any(|modifier| {
            matches!(
                modifier,
                Modifier::Trait | Modifier::Enum | Modifier::EnumCase | Modifier::Abstract
            )
        }) || !definition.metadata.annotations.is_empty()
            || ast.get(tree).position.is_none()
        {
            return Err(fail("classes must be ordinary, concrete, and unannotated"));
        }
        if typer.store().symbols.get(symbol).visibility != Visibility::Public
            || typer
                .store()
                .symbols
                .get(symbol)
                .flags
                .contains(SymbolFlags::ABSTRACT)
        {
            return Err(ConstructorReferenceAliasError::SemanticIdentity(
                "classes must be public and concrete",
            ));
        }
    }
    if !counter_template.parents.is_empty()
        || !owner_template.parents.is_empty()
        || counter_template.self_val.is_some()
        || owner_template.self_val.is_some()
    {
        return Err(fail(
            "ordinary classes without parents or self types required",
        ));
    }
    let counter_ctor_def = def(ast, counter_template.constructor)?;
    if !counter_ctor_def.type_params.is_empty() {
        return Err(fail("generic Counter is unsupported"));
    }
    let [counter_clause] = counter_ctor_def.value_param_clauses.as_slice() else {
        return Err(fail("Counter requires one plain constructor clause"));
    };
    if counter_clause.len() != 1 {
        return Err(fail("Counter requires one constructor formal"));
    }
    let TreeKind::ValDef(counter_formal_def) = &ast.get(counter_clause[0]).kind else {
        return Err(fail("Counter constructor formal must be a value parameter"));
    };
    if counter_formal_def.rhs.is_some()
        || counter_formal_def
            .metadata
            .modifiers
            .iter()
            .any(|modifier| {
                matches!(
                    modifier,
                    Modifier::Var | Modifier::Given | Modifier::Implicit | Modifier::Erased
                )
            })
    {
        return Err(fail("Counter constructor formal must be a plain value"));
    }
    let counter_formal = index
        .derived_symbol_at(counter_ctor, source_id, counter_clause[0])
        .ok_or(ConstructorReferenceAliasError::SourceIdentity(
            "Counter constructor formal",
        ))?;

    let ctor_def = def(ast, owner_template.constructor)?;
    if !ctor_def.type_params.is_empty()
        || ctor_def.value_param_clauses.len() != 1
        || ctor_def.value_param_clauses[0].len() != 1
    {
        return Err(fail(
            "CounterOwner requires exactly one non-generic value clause with one formal",
        ));
    }
    let formal_tree = ctor_def.value_param_clauses[0][0];
    let template_parameter = index.symbol_at(source_id, formal_tree).ok_or(
        ConstructorReferenceAliasError::SourceIdentity("class-owned template parameter"),
    )?;
    let constructor_parameter = index
        .derived_symbol_at(constructor, source_id, formal_tree)
        .ok_or(ConstructorReferenceAliasError::SourceIdentity(
            "constructor-owned derived parameter",
        ))?;
    let formal_name = name_of(ast, formal_tree, &typer.store().names)
        .ok_or(fail("constructor formal must be a value parameter"))?;
    if formal_name != "counter" {
        return Err(fail("constructor formal must be named counter"));
    }
    let formal = ast.get(formal_tree);
    let TreeKind::ValDef(formal_def) = &formal.kind else {
        return Err(fail("constructor formal must be a val definition"));
    };
    if !formal_def
        .metadata
        .modifiers
        .contains(&Modifier::PrivateLocal)
        || formal_def.metadata.modifiers.iter().any(|modifier| {
            matches!(
                modifier,
                Modifier::Var | Modifier::Given | Modifier::Implicit | Modifier::Erased
            )
        })
        || formal_def.rhs.is_some()
        || formal_def.metadata.visibility.is_some()
        || !formal_def.metadata.annotations.is_empty()
    {
        return Err(fail(
            "constructor formal must be plain, non-accessor, non-annotated value",
        ));
    }

    for class in [counter, owner] {
        typer.complete_symbol(class).map_err(|_| {
            ConstructorReferenceAliasError::SemanticIdentity("class signature completion")
        })?;
    }
    for symbol in [
        counter_ctor,
        counter_formal,
        constructor,
        template_parameter,
        constructor_parameter,
    ] {
        typer.complete_symbol(symbol).map_err(|_| {
            ConstructorReferenceAliasError::SemanticIdentity("constructor signature completion")
        })?;
    }
    let mut field = None;
    let mut read = None;
    let mut get = None;
    let mut initializer = None;
    let mut read_rhs = None;
    let mut counter_state_seen = false;
    let mut counter_get_seen = false;
    let mut counter_set_seen = false;
    for (class, template) in [(counter, counter_template), (owner, owner_template)] {
        for &member_tree in &template.body {
            match &ast.get(member_tree).kind {
                TreeKind::ValDef(value) if class == owner => {
                    if field.is_some() {
                        return Err(fail("duplicate CounterOwner.inner field"));
                    }
                    let symbol = index.symbol_at(source_id, member_tree).ok_or(
                        ConstructorReferenceAliasError::SourceIdentity("inner field"),
                    )?;
                    if typer.store().names.resolve(value.name.as_name().text()) != "inner" {
                        return Err(fail(
                            "CounterOwner only supports the inner field in this proof",
                        ));
                    }
                    let rhs = value.rhs.ok_or(fail("inner field initializer required"))?;
                    if value.metadata.visibility
                        != Some(VisibilitySyntax::Private { qualifier: None })
                        || value
                            .metadata
                            .modifiers
                            .iter()
                            .any(|modifier| matches!(modifier, Modifier::Var | Modifier::Lazy))
                        || !value.metadata.annotations.is_empty()
                    {
                        return Err(fail(
                            "inner must be an unqualified private eager immutable field without annotations",
                        ));
                    }
                    typer.complete_symbol(symbol).map_err(|_| {
                        ConstructorReferenceAliasError::SemanticIdentity(
                            "field signature completion",
                        )
                    })?;
                    let field_type = match typer.store().symbols.get(symbol).info {
                        SymbolInfo::Complete(ty) => ty,
                        _ => {
                            return Err(ConstructorReferenceAliasError::SemanticIdentity(
                                "field type incomplete",
                            ));
                        }
                    };
                    let typed = typer
                        .type_expression_expected(
                            rhs,
                            typer.field_initializer_context_for(symbol).map_err(|_| {
                                ConstructorReferenceAliasError::TypedBinding(
                                    "field initializer context",
                                )
                            })?,
                            field_type,
                        )
                        .map_err(|_| {
                            ConstructorReferenceAliasError::TypedBinding("field initializer typing")
                        })?;
                    if typer.source_typed_index().get(source_id, rhs) != Some(typed) {
                        return Err(ConstructorReferenceAliasError::TypedBinding(
                            "field initializer source/typed edge",
                        ));
                    }
                    field = Some(symbol);
                    initializer = Some(rhs);
                }
                TreeKind::ValDef(value) if class == counter => {
                    if typer.store().names.resolve(value.name.as_name().text()) != "current"
                        || !value.metadata.modifiers.contains(&Modifier::Var)
                        || value.metadata.visibility
                            != Some(VisibilitySyntax::Private { qualifier: None })
                        || !value.metadata.annotations.is_empty()
                        || counter_state_seen
                    {
                        return Err(fail(
                            "Counter must have exactly one private mutable current field",
                        ));
                    }
                    counter_state_seen = true;
                }
                TreeKind::DefDef(method) if member_tree != template.constructor => {
                    let symbol = index
                        .symbol_at(source_id, member_tree)
                        .ok_or(ConstructorReferenceAliasError::SourceIdentity("method"))?;
                    let method_name = typer
                        .store()
                        .names
                        .resolve(method.name.as_name().text())
                        .to_owned();
                    if (class == owner && method_name != "read")
                        || (class == counter && !matches!(method_name.as_str(), "get" | "set"))
                    {
                        return Err(fail(
                            "unexpected method in constructor-reference proof classes",
                        ));
                    }
                    typer.complete_symbol(symbol).map_err(|_| {
                        ConstructorReferenceAliasError::SemanticIdentity(
                            "method signature completion",
                        )
                    })?;
                    for formal in method.value_param_clauses.iter().flatten() {
                        let parameter = index.symbol_at(source_id, *formal).ok_or(
                            ConstructorReferenceAliasError::SourceIdentity("method parameter"),
                        )?;
                        typer.complete_symbol(parameter).map_err(|_| {
                            ConstructorReferenceAliasError::SemanticIdentity(
                                "method parameter completion",
                            )
                        })?;
                    }
                    if class == counter && method_name == "get" {
                        if counter_get_seen
                            || method
                                .value_param_clauses
                                .iter()
                                .any(|clause| !clause.is_empty())
                            || method.rhs.is_none()
                        {
                            return Err(fail(
                                "Counter.get must be one concrete zero-argument method",
                            ));
                        }
                        counter_get_seen = true;
                        let rhs = method.rhs.unwrap();
                        let context = typer.expression_context_for(symbol).map_err(|_| {
                            ConstructorReferenceAliasError::TypedBinding("get method context")
                        })?;
                        typer.type_expression(rhs, context).map_err(|_| {
                            ConstructorReferenceAliasError::TypedBinding("Counter.get typing")
                        })?;
                        get = Some(symbol);
                    }
                    if class == counter && method_name == "set" {
                        if counter_set_seen
                            || method.value_param_clauses.len() != 1
                            || method.value_param_clauses[0].len() != 1
                            || method.rhs.is_none()
                        {
                            return Err(fail(
                                "Counter.set must be one concrete one-argument method",
                            ));
                        }
                        counter_set_seen = true;
                    }
                    if class == owner && method_name == "read" {
                        if read.is_some()
                            || method
                                .value_param_clauses
                                .iter()
                                .any(|clause| !clause.is_empty())
                            || method.rhs.is_none()
                        {
                            return Err(fail(
                                "CounterOwner.read must be a concrete zero-argument method",
                            ));
                        }
                        let rhs = method.rhs.unwrap();
                        let context = typer.expression_context_for(symbol).map_err(|_| {
                            ConstructorReferenceAliasError::TypedBinding("read method context")
                        })?;
                        let typed = typer.type_expression(rhs, context).map_err(|_| {
                            ConstructorReferenceAliasError::TypedBinding("read method typing")
                        })?;
                        if typer.source_typed_index().get(source_id, rhs) != Some(typed) {
                            return Err(ConstructorReferenceAliasError::TypedBinding(
                                "read source/typed edge",
                            ));
                        }
                        read = Some(symbol);
                        read_rhs = Some(rhs);
                    }
                }
                TreeKind::ValDef(_) | TreeKind::DefDef(_) => {}
                _ => return Err(fail("only field and method members are supported")),
            }
        }
    }
    if !counter_state_seen || !counter_get_seen || !counter_set_seen {
        return Err(fail(
            "Counter must contain current, get, and set exactly once",
        ));
    }
    Ok(ProofParts {
        counter,
        owner,
        constructor,
        template_parameter,
        constructor_parameter,
        field: field.ok_or(fail("missing inner field"))?,
        read: read.ok_or(fail("missing read method"))?,
        get: get.ok_or(fail("missing Counter.get method"))?,
        initializer: initializer.ok_or(fail("missing field initializer"))?,
        read_rhs: read_rhs.ok_or(fail("missing read body"))?,
    })
}

fn finish_proof<'session>(
    input: ProofInput<'session>,
) -> Result<ValidatedConstructorReferenceAlias<'session>, ConstructorReferenceAliasError> {
    let ProofInput {
        source_id,
        definitions,
        source_root,
        store,
        source_ast: ast,
        typed_ast,
        source_index: index,
        source_typed_index: typed_index,
        parts,
    } = input;
    let fail = ConstructorReferenceAliasError::SemanticIdentity;
    let ProofParts {
        counter,
        owner,
        constructor,
        template_parameter,
        constructor_parameter,
        field,
        read,
        get,
        initializer,
        read_rhs,
    } = parts;
    let counter_info = class_info(store, counter)?;
    let owner_info = class_info(store, owner)?;
    if store.symbols.get(counter).owner != store.symbols.get(owner).owner
        || counter_info.declarations == owner_info.declarations
    {
        return Err(ConstructorReferenceAliasError::SourceIdentity(
            "classes must share one package owner",
        ));
    }
    for (symbol, kind, expected_owner) in [
        (counter, SymbolKind::Class, store.symbols.get(counter).owner),
        (owner, SymbolKind::Class, store.symbols.get(owner).owner),
        (constructor, SymbolKind::Constructor, Some(owner)),
        (template_parameter, SymbolKind::Parameter, Some(owner)),
        (
            constructor_parameter,
            SymbolKind::Parameter,
            Some(constructor),
        ),
        (field, SymbolKind::Field, Some(owner)),
        (read, SymbolKind::Method, Some(owner)),
        (get, SymbolKind::Method, Some(counter)),
    ] {
        let info = store.symbols.get(symbol);
        if info.kind != kind
            || info.owner != expected_owner
            || info.origin != SymbolOrigin::Source(source_id)
        {
            return Err(fail("symbol kind, owner, or source origin"));
        }
    }
    if template_parameter == constructor_parameter {
        return Err(fail(
            "template and constructor parameters must be distinct symbols",
        ));
    }
    if store.symbols.get(template_parameter).visibility != Visibility::Private
        || store.symbols.get(constructor_parameter).visibility != Visibility::Public
        || store.symbols.get(constructor).visibility != Visibility::Public
        || store.symbols.get(field).visibility != Visibility::Private
        || store
            .symbols
            .get(field)
            .flags
            .contains(SymbolFlags::MUTABLE | SymbolFlags::LAZY)
    {
        return Err(fail("parameter/field visibility or mutability"));
    }
    for symbol in [
        counter,
        owner,
        constructor,
        template_parameter,
        constructor_parameter,
        field,
        read,
        get,
    ] {
        if !matches!(index.definition_of(symbol), Some(SourceDefinition::Canonical { source, .. } | SourceDefinition::Derived { source, .. }) if source == source_id)
        {
            return Err(ConstructorReferenceAliasError::SourceIdentity(
                "symbol source-index provenance",
            ));
        }
    }
    let (
        Some(SourceDefinition::Canonical {
            source: counter_source,
            tree: counter_tree,
        }),
        Some(SourceDefinition::Canonical {
            source: owner_source,
            tree: owner_tree,
        }),
    ) = (index.definition_of(counter), index.definition_of(owner))
    else {
        return Err(ConstructorReferenceAliasError::SourceIdentity(
            "class declaration source index",
        ));
    };
    if counter_source != source_id
        || owner_source != source_id
        || index.symbol_at(source_id, counter_tree) != Some(counter)
        || index.symbol_at(source_id, owner_tree) != Some(owner)
    {
        return Err(ConstructorReferenceAliasError::SourceIdentity(
            "class declaration tree binding",
        ));
    }
    let owner_constructor_tree = match &ast.get(owner_tree).kind {
        TreeKind::TypeDef(definition) => match &ast.get(definition.rhs).kind {
            TreeKind::Template(template) => template.constructor,
            _ => {
                return Err(ConstructorReferenceAliasError::SourceIdentity(
                    "Owner template",
                ));
            }
        },
        _ => {
            return Err(ConstructorReferenceAliasError::SourceIdentity(
                "Owner class tree",
            ));
        }
    };
    let formal_tree = match &ast.get(owner_constructor_tree).kind {
        TreeKind::DefDef(definition) => definition
            .value_param_clauses
            .first()
            .and_then(|clause| clause.first())
            .copied(),
        _ => None,
    }
    .ok_or(ConstructorReferenceAliasError::SourceIdentity(
        "original constructor formal tree",
    ))?;
    if index.symbol_at(source_id, formal_tree) != Some(template_parameter)
        || index.derived_symbol_at(constructor, source_id, formal_tree)
            != Some(constructor_parameter)
        || index.definition_of(template_parameter)
            != Some(SourceDefinition::Canonical {
                source: source_id,
                tree: formal_tree,
            })
        || index.definition_of(constructor_parameter)
            != Some(SourceDefinition::Derived {
                source: source_id,
                tree: formal_tree,
            })
        || store.symbols.get(template_parameter).position
            != store.symbols.get(constructor_parameter).position
    {
        return Err(ConstructorReferenceAliasError::SourceIdentity(
            "template/derived parameter provenance or source range",
        ));
    }
    let counter_scope = store.scopes.get(counter_info.declarations);
    let owner_scope = store.scopes.get(owner_info.declarations);
    if counter_scope.owner != Some(counter) || owner_scope.owner != Some(owner) {
        return Err(ConstructorReferenceAliasError::SemanticIdentity(
            "ClassInfo declaration scope owner",
        ));
    }
    for (scope, member) in [
        (counter_scope, get),
        (owner_scope, constructor),
        (owner_scope, field),
        (owner_scope, read),
    ] {
        if !scope
            .lookup_all(&store.symbols.get(member).name)
            .contains(&member)
        {
            return Err(ConstructorReferenceAliasError::SemanticIdentity(
                "class member is absent from canonical ClassInfo scope",
            ));
        }
    }
    let field_tree = match index.definition_of(field) {
        Some(SourceDefinition::Canonical { source, tree }) if source == source_id => tree,
        _ => {
            return Err(ConstructorReferenceAliasError::SourceIdentity(
                "inner field source declaration",
            ));
        }
    };
    let read_tree = match index.definition_of(read) {
        Some(SourceDefinition::Canonical { source, tree }) if source == source_id => tree,
        _ => {
            return Err(ConstructorReferenceAliasError::SourceIdentity(
                "read method source declaration",
            ));
        }
    };
    let owner_body = match &ast.get(owner_tree).kind {
        TreeKind::TypeDef(definition) => match &ast.get(definition.rhs).kind {
            TreeKind::Template(template) => &template.body,
            _ => {
                return Err(ConstructorReferenceAliasError::SourceIdentity(
                    "Owner template body",
                ));
            }
        },
        _ => {
            return Err(ConstructorReferenceAliasError::SourceIdentity(
                "Owner source class",
            ));
        }
    };
    if owner_body
        .iter()
        .position(|tree| *tree == field_tree)
        .zip(owner_body.iter().position(|tree| *tree == read_tree))
        .is_none_or(|(field_index, read_index)| field_index >= read_index)
    {
        return Err(ConstructorReferenceAliasError::SourceShape(
            "inner field must precede read in source order",
        ));
    }
    let source_rhs_name = match &ast.get(initializer).kind {
        TreeKind::Ident(id) => id.name.text(),
        _ => {
            return Err(ConstructorReferenceAliasError::TypedBinding(
                "initializer source must be original counter identifier",
            ));
        }
    };
    if store.names.resolve(source_rhs_name) != "counter" {
        return Err(ConstructorReferenceAliasError::TypedBinding(
            "initializer source name",
        ));
    }
    let counter_type = completed_type(store, counter)?;
    let template_type = completed_type(store, template_parameter)?;
    let constructor_type = completed_type(store, constructor_parameter)?;
    let field_type = completed_type(store, field)?;
    for ty in [template_type, constructor_type, field_type] {
        if !is_counter_type(store, ty, counter) {
            return Err(ConstructorReferenceAliasError::TypeIdentity(
                "formal/field type must resolve to the same package-qualified Counter type",
            ));
        }
    }
    if let Type::ClassInfo(info) = store
        .types
        .try_get(counter_type)
        .ok_or(fail("Counter class info type"))?
    {
        if info.class != counter {
            return Err(fail("Counter ClassInfo identity"));
        }
    } else {
        return Err(fail("Counter must have ordinary ClassInfo"));
    }
    let package = store
        .symbols
        .get(owner)
        .owner
        .ok_or(fail("package owner"))?;
    if store.symbols.get(package).kind != SymbolKind::Package {
        return Err(ConstructorReferenceAliasError::SourceIdentity(
            "class owner must be a package",
        ));
    }
    for (symbol, expected_result) in [(get, definitions.int), (read, definitions.int)] {
        let signature = completed_type(store, symbol)?;
        if !matches!(store.types.try_get(signature), Some(Type::Method(method)) if method.params.is_empty() && method.kind == dotty_core::MethodKind::Plain && method.result == expected_result)
        {
            return Err(ConstructorReferenceAliasError::AbiShape(
                "read/get must have exact ()Int signatures",
            ));
        }
    }
    let constructor_signature = completed_type(store, constructor)?;
    if !matches!(store.types.try_get(constructor_signature), Some(Type::Method(method)) if method.params.len() == 1 && method.kind == dotty_core::MethodKind::Plain && method.params[0].ty == constructor_type)
    {
        return Err(ConstructorReferenceAliasError::AbiShape(
            "constructor must have exact one-argument Counter signature",
        ));
    }

    let typed_initializer = typed_index.get(source_id, initializer).ok_or(
        ConstructorReferenceAliasError::TypedBinding("missing original initializer binding"),
    )?;
    let typed_read = typed_index.get(source_id, read_rhs).ok_or(
        ConstructorReferenceAliasError::TypedBinding("missing original read binding"),
    )?;
    let typed_rhs = typed_ast.try_get(typed_initializer).ok_or(
        ConstructorReferenceAliasError::TypedBinding("stale typed initializer tree"),
    )?;
    if !matches!(typed_rhs.kind, TreeKind::Ident(_))
        || !term_ref_targets_no_prefix(store, typed_rhs.ty, constructor_parameter)
    {
        return Err(ConstructorReferenceAliasError::TypedBinding(
            "typed initializer must retain constructor-owned counter reference",
        ));
    }
    let TreeKind::Apply(apply) = &typed_ast
        .try_get(typed_read)
        .ok_or(ConstructorReferenceAliasError::TypedBinding(
            "stale typed read tree",
        ))?
        .kind
    else {
        return Err(ConstructorReferenceAliasError::TypedBinding(
            "read must apply selected get method",
        ));
    };
    let TreeKind::Select(get_select) = &typed_ast
        .try_get(apply.function)
        .ok_or(ConstructorReferenceAliasError::TypedBinding(
            "stale get selection",
        ))?
        .kind
    else {
        return Err(ConstructorReferenceAliasError::TypedBinding(
            "read must select get",
        ));
    };
    let receiver = typed_ast.try_get(get_select.qualifier).ok_or(
        ConstructorReferenceAliasError::TypedBinding("stale field receiver"),
    )?;
    if !term_ref_targets(store, receiver.ty, field)
        || !term_ref_targets(store, typed_ast.get(apply.function).ty, get)
    {
        return Err(ConstructorReferenceAliasError::TypedBinding(
            "read must select Counter.get through original inner field",
        ));
    }
    // This proof is deliberately pinned to the top-level `example` package.
    // Checking only the immediate owner name would incorrectly accept
    // `package outer.example` and emit descriptors for `example.Counter`.
    let package_name = store.names.resolve(store.symbols.get(package).name.text());
    let package_parent = store.symbols.get(package).owner;
    let is_top_level_package = package_parent.is_some_and(|parent| {
        let root = store.symbols.get(parent);
        root.kind == SymbolKind::Package
            && root.owner.is_none()
            && store.names.resolve(root.name.text()).is_empty()
    });
    let counter_name = store.names.resolve(store.symbols.get(counter).name.text());
    let owner_name = store.names.resolve(store.symbols.get(owner).name.text());
    if package_name != "example"
        || !is_top_level_package
        || counter_name != "Counter"
        || owner_name != "CounterOwner"
    {
        return Err(ConstructorReferenceAliasError::AbiShape(
            "canonical package/class names",
        ));
    }
    let internal = format!("{package_name}/{counter_name}");
    Ok(ValidatedConstructorReferenceAlias {
        _store: store,
        _source_ast: ast,
        _typed_ast: typed_ast,
        _source_index: index,
        _source_typed_index: typed_index,
        definitions,
        source_root,
        source_id,
        counter_class: counter,
        owner_class: owner,
        constructor,
        template_parameter,
        constructor_parameter,
        field,
        read_method: read,
        counter_get: get,
        source_initializer: initializer,
        typed_initializer,
        source_read: read_rhs,
        typed_read,
        constructor_descriptor: format!("(L{internal};)V"),
        field_descriptor: format!("L{internal};"),
    })
}

fn class_parts<'a>(
    tree: TreeId<Untyped>,
    source: SourceId,
    ast: &'a AstArena<Untyped>,
    index: &SourceSemanticIndex,
    names: &dotty_core::NameInterner,
) -> Result<
    (
        String,
        SymbolId,
        &'a dotty_core::ast::Template<Untyped>,
        SymbolId,
    ),
    ConstructorReferenceAliasError,
> {
    let TreeKind::TypeDef(definition) = &ast.get(tree).kind else {
        return Err(ConstructorReferenceAliasError::SourceShape(
            "batch declarations must be classes",
        ));
    };
    let TreeKind::Template(template) = &ast.get(definition.rhs).kind else {
        return Err(ConstructorReferenceAliasError::SourceShape(
            "class must have ordinary template",
        ));
    };
    let class =
        index
            .symbol_at(source, tree)
            .ok_or(ConstructorReferenceAliasError::SourceIdentity(
                "class symbol",
            ))?;
    let constructor = index.symbol_at(source, template.constructor).ok_or(
        ConstructorReferenceAliasError::SourceIdentity("primary constructor"),
    )?;
    Ok((
        names.resolve(definition.name.as_name().text()).to_owned(),
        class,
        template,
        constructor,
    ))
}

fn def(
    ast: &AstArena<Untyped>,
    tree: TreeId<Untyped>,
) -> Result<&dotty_core::ast::DefDef<Untyped>, ConstructorReferenceAliasError> {
    match &ast.get(tree).kind {
        TreeKind::DefDef(definition) => Ok(definition),
        _ => Err(ConstructorReferenceAliasError::SourceShape(
            "expected definition",
        )),
    }
}

fn name_of(
    ast: &AstArena<Untyped>,
    tree: TreeId<Untyped>,
    names: &dotty_core::NameInterner,
) -> Option<String> {
    match &ast.get(tree).kind {
        TreeKind::ValDef(definition) => {
            Some(names.resolve(definition.name.as_name().text()).to_owned())
        }
        _ => None,
    }
}

fn class_info(
    store: &SemanticStore,
    symbol: SymbolId,
) -> Result<&ClassInfo, ConstructorReferenceAliasError> {
    let id = completed_type(store, symbol)?;
    match store.types.try_get(id) {
        Some(Type::ClassInfo(info)) if info.class == symbol => Ok(info),
        _ => Err(ConstructorReferenceAliasError::SemanticIdentity(
            "class has no matching ClassInfo",
        )),
    }
}

fn completed_type(
    store: &SemanticStore,
    symbol: SymbolId,
) -> Result<dotty_core::TypeId, ConstructorReferenceAliasError> {
    match store.symbols.get(symbol).info {
        SymbolInfo::Complete(ty) => Ok(ty),
        _ => Err(ConstructorReferenceAliasError::SemanticIdentity(
            "symbol info is not complete",
        )),
    }
}

fn is_counter_type(store: &SemanticStore, ty: dotty_core::TypeId, counter: SymbolId) -> bool {
    let Some(Type::TypeRef {
        prefix,
        target: TypeRefTarget::Symbol(symbol),
    }) = store.types.try_get(ty)
    else {
        return false;
    };
    let Some(package) = store.symbols.get(counter).owner else {
        return false;
    };
    *symbol == counter
        && matches!(store.types.try_get(*prefix), Some(Type::TypeRef { target: TypeRefTarget::Symbol(prefix_symbol), .. }) if *prefix_symbol == package)
}

fn term_ref_targets(store: &SemanticStore, ty: dotty_core::TypeId, symbol: SymbolId) -> bool {
    matches!(store.types.try_get(ty), Some(Type::TermRef { target: TermRefTarget::Symbol(found), .. }) if *found == symbol)
}

fn term_ref_targets_no_prefix(
    store: &SemanticStore,
    ty: dotty_core::TypeId,
    symbol: SymbolId,
) -> bool {
    let Some(Type::TermRef {
        prefix,
        target: TermRefTarget::Symbol(found),
    }) = store.types.try_get(ty)
    else {
        return false;
    };
    *found == symbol && matches!(store.types.try_get(*prefix), Some(Type::NoPrefix))
}
