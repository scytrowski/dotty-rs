use dotty::jvm_tasty_cross_class::{
    CrossClassConstructorCallProofError, MutableLocalReferenceReassignmentProofInput,
    validate_mutable_local_reference_reassignment_proof,
};
use dotty::jvm_tasty_plan::{
    mutable_local_reference_assignment_binding, mutable_local_reference_declaration_binding,
};
use dotty_core::ast::{ApplyKind, AstArena, TreeKind, Typed};
use dotty_core::{Definitions, Packages, SemanticStore, SourceId, SourceText, TermRefTarget, Type};
use dotty_core::{SourceTypedIndex, SymbolId, TreeId, Untyped};
use dotty_jvm::JvmCompilationUnit;
use dotty_lexer::ContextualScanner;
use dotty_namer::{NamingOutcome, SourceParseStatus, name_compilation_unit_with_diagnostics};
use dotty_parser::parse_compilation_unit;
use dotty_tasty::file::TastyFile;
use dotty_tasty_pickler::{
    ClassMemberPicklePlan, ClassStatePicklePlan, LocalCounterInitializerBinding, LocalValueBinding,
    MutableCounterLocalInitializerBinding, NestedConstructorCallBinding,
    PrimaryConstructorInputPlan, SourceMetadata, SourceTypedBindings,
};
use dotty_typer::{SourceTyper, TypedLocalBinding};
use std::path::PathBuf;
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

const SOURCE: &str =
    include_str!("fixtures/jvm_tasty_mutable_reference_rebinding/MutableReferenceRebinding.scala");
const REQUIRE_SCALA_FRAGMENT: &str = "DOTTY_REQUIRE_SCALA_TASTY_MUTABLE_COUNTER_FRAGMENT";
const SCALA_FRAGMENT_SUPPORT: &str = r#"package example

class Counter(start: Int):
  private var current: Int = start
  def get(): Int = current
  def set(value: Int): Unit = current = value

class Pair(first: Counter, second: Counter):
  private val left: Counter = first
  private val right: Counter = second
  def leftValue(): Int = left.get()
  def rightValue(): Int = right.get()
"#;

struct FragmentBindings<'a> {
    typed: &'a SourceTypedIndex,
    classes: Vec<SymbolId>,
    methods: Vec<SymbolId>,
    locals: Vec<LocalValueBinding>,
    source: SourceId,
    method: SymbolId,
    binding: MutableCounterLocalInitializerBinding,
    assignment: dotty_tasty_pickler::MutableCounterReferenceAssignmentBinding,
    initializers: Vec<LocalCounterInitializerBinding>,
}

struct FragmentInitializerSession<'a> {
    source_id: SourceId,
    method: SymbolId,
    scope: dotty_core::ScopeId,
    source_block: TreeId<Untyped>,
    typed_block: TreeId<Typed>,
    store: &'a SemanticStore,
    source_ast: &'a AstArena<Untyped>,
    typed_ast: &'a AstArena<Typed>,
    source_index: &'a dotty_core::SourceSemanticIndex,
    typed_bindings: &'a SourceTypedIndex,
}

impl SourceTypedBindings for FragmentBindings<'_> {
    fn typed_at(&self, source: SourceId, tree: TreeId<Untyped>) -> Option<TreeId<Typed>> {
        self.typed.get(source, tree)
    }
    fn planned_class_symbols(&self) -> &[SymbolId] {
        &self.classes
    }
    fn planned_method_symbols(&self) -> &[SymbolId] {
        &self.methods
    }
    fn local_value_bindings(&self, source: SourceId, method: SymbolId) -> Vec<LocalValueBinding> {
        if source == self.source && method == self.method {
            self.locals.clone()
        } else {
            Vec::new()
        }
    }
    fn mutable_counter_local_initializer(
        &self,
        source: SourceId,
        method: SymbolId,
    ) -> Option<MutableCounterLocalInitializerBinding> {
        (source == self.source && method == self.method).then_some(self.binding)
    }
    fn local_counter_initializers(
        &self,
        source: SourceId,
        method: SymbolId,
    ) -> Vec<LocalCounterInitializerBinding> {
        if source == self.source && method == self.method {
            self.initializers.clone()
        } else {
            Vec::new()
        }
    }

    fn mutable_counter_reference_assignment(
        &self,
        source: SourceId,
        method: SymbolId,
    ) -> Option<dotty_tasty_pickler::MutableCounterReferenceAssignmentBinding> {
        (source == self.source && method == self.method).then_some(self.assignment)
    }
}

fn stale_symbol_id(store: &SemanticStore) -> dotty_core::SymbolId {
    let mut foreign = SemanticStore::new();
    loop {
        let name = foreign.names.intern("stale");
        let symbol = dotty_core::Symbol {
            name: dotty_core::Name::new(name, dotty_core::Namespace::Term),
            owner: None,
            kind: dotty_core::SymbolKind::Value,
            flags: dotty_core::SymbolFlags::EMPTY,
            visibility: dotty_core::Visibility::Public,
            info: dotty_core::SymbolInfo::Missing,
            origin: dotty_core::SymbolOrigin::Synthetic,
            annotations: Vec::new(),
            position: None,
            links: dotty_core::SymbolLinks::default(),
        };
        let id = foreign.symbols.alloc(symbol);
        if !store.symbols.contains(id) {
            return id;
        }
    }
}

fn fragment_initializer_bindings(
    session: FragmentInitializerSession<'_>,
    locals: [SymbolId; 2],
    declarations: [TreeId<Untyped>; 2],
) -> Vec<LocalCounterInitializerBinding> {
    let FragmentInitializerSession {
        source_id,
        method,
        scope,
        source_block,
        typed_block,
        store,
        source_ast,
        typed_ast,
        source_index,
        typed_bindings,
    } = session;
    declarations
        .into_iter()
        .zip(locals)
        .map(|(source_declaration, local)| {
            let typed_declaration = typed_bindings
                .get(source_id, source_declaration)
                .expect("typed predecessor declaration");
            let (TreeKind::ValDef(source_val), TreeKind::ValDef(typed_val)) = (
                &source_ast.get(source_declaration).kind,
                &typed_ast.get(typed_declaration).kind,
            ) else {
                panic!("predecessor declarations are ValDefs")
            };
            let source_initializer = source_val.rhs.expect("predecessor source initializer");
            let typed_initializer = typed_val.rhs.expect("predecessor typed initializer");
            let (TreeKind::Apply(source_apply), TreeKind::Apply(typed_apply)) = (
                &source_ast.get(source_initializer).kind,
                &typed_ast.get(typed_initializer).kind,
            ) else {
                panic!("predecessor initializers are direct constructor applications")
            };
            assert_eq!(source_apply.kind, ApplyKind::Regular);
            assert_eq!(typed_apply.kind, ApplyKind::Regular);
            let source_select = source_apply.function;
            let typed_select = typed_apply.function;
            let (TreeKind::Select(source_select_node), TreeKind::Select(typed_select_node)) = (
                &source_ast.get(source_select).kind,
                &typed_ast.get(typed_select).kind,
            ) else {
                panic!("predecessor initializer functions are constructor selects")
            };
            let source_new = source_select_node.qualifier;
            let typed_new = typed_select_node.qualifier;
            let (TreeKind::New(source_new_node), TreeKind::New(typed_new_node)) = (
                &source_ast.get(source_new).kind,
                &typed_ast.get(typed_new).kind,
            ) else {
                panic!("predecessor constructor selections qualify a New")
            };
            assert_eq!(typed_bindings.get(source_id, source_new), Some(typed_new));
            assert_eq!(
                typed_bindings.get(source_id, source_new_node.tpt),
                Some(typed_new_node.tpt)
            );
            let source_argument = *source_apply.args.first().expect("one constructor arg");
            let typed_argument = *typed_apply.args.first().expect("one typed constructor arg");
            let constructor = match store.types.get(typed_ast.get(typed_select).ty) {
                Type::TermRef {
                    target: TermRefTarget::Symbol(symbol),
                    ..
                } => *symbol,
                other => panic!("constructor select has TermRef type: {other:?}"),
            };
            let argument_formal = match store.types.get(typed_ast.get(typed_argument).ty) {
                Type::TermRef {
                    target: TermRefTarget::Symbol(symbol),
                    ..
                } => *symbol,
                other => panic!("constructor argument has formal TermRef type: {other:?}"),
            };
            let constructor_tree = match source_index.definition_of(constructor) {
                Some(dotty_core::SourceDefinition::Canonical { source, tree })
                    if source == source_id =>
                {
                    tree
                }
                other => panic!("constructor has canonical source: {other:?}"),
            };
            let TreeKind::DefDef(constructor_definition) = &source_ast.get(constructor_tree).kind
            else {
                panic!("constructor definition")
            };
            let constructor_formal_tree = *constructor_definition
                .value_param_clauses
                .first()
                .and_then(|clause| clause.first())
                .expect("one constructor formal");
            let constructor_formal = source_index
                .derived_symbol_at(constructor, source_id, constructor_formal_tree)
                .expect("canonical constructor formal symbol");
            let class = store
                .symbols
                .get(constructor)
                .owner
                .expect("constructor owner");
            let formal_type = match store.symbols.get(constructor_formal).info {
                dotty_core::SymbolInfo::Complete(ty) => ty,
                other => panic!("constructor argument formal is complete: {other:?}"),
            };
            assert_eq!(
                store.symbols.get(constructor_formal).owner,
                Some(constructor)
            );
            assert_eq!(store.symbols.get(argument_formal).owner, Some(method));
            let local_type = match store.symbols.get(local).info {
                dotty_core::SymbolInfo::Complete(ty) => ty,
                other => panic!("predecessor local is complete: {other:?}"),
            };
            assert_eq!(store.symbols.get(local).owner, Some(method));
            let call = NestedConstructorCallBinding {
                source_apply: source_initializer,
                typed_apply: typed_initializer,
                source_select,
                source_new,
                source_argument,
                typed_select,
                typed_new,
                typed_argument,
                class,
                constructor,
                formal: constructor_formal,
                formal_type,
                result_type: typed_ast.get(typed_initializer).ty,
            };
            LocalCounterInitializerBinding {
                source_block,
                typed_block,
                source_declaration,
                typed_declaration,
                source_initializer,
                typed_initializer,
                source_type_tree: source_val.tpt,
                typed_type_tree: typed_val.tpt,
                local,
                scope,
                local_type,
                argument_formal,
                call,
            }
        })
        .collect()
}

#[derive(Clone, Copy, Debug)]
enum Mutation {
    None,
    MissingAssignment,
    MissingRhs,
    SwapAssignmentOperands,
    WrongBinding,
    MissingSelectedBinding,
    ImmutableSelected,
    MissingSourceVarModifier,
    ExtraSourceVarModifier,
    WrongOwner,
    WrongScope,
    WrongInitializer,
    WrongFirstInitializerSourceSelectName,
    WrongFirstInitializerTypedSelectName,
    WrongSecondInitializerSourceSelectName,
    WrongSecondInitializerTypedSelectName,
    MissingSelectedAnnotation,
    WrongSelectedAnnotation,
    SelectedInitializerNotIdent,
    WrongAssignmentTarget,
    StaleAssignmentTarget,
    WrongSetterReceiver,
    WrongPairArguments,
    StalePairConstructor,
    WrongPairNewTypeTreeKind,
    WrongPairNewType,
    WrongBlockType,
    TypedPairApplyKind,
    TypedSetterApplyKind,
    SourceSetterSelectName,
    TypedSetterSelectName,
    SourcePairSelectName,
    TypedPairSelectName,
    WrongResult,
    ReorderStatements,
    DuplicateAssignment,
    ForeignAssignedType,
    MissingSourceBlockPosition,
    MissingSourceSelectedPosition,
    ForeignSourceSelectedPosition,
    MissingTypedAssignmentPosition,
    MismatchedTypedAssignmentPosition,
    WrongFirstPrefixInitializerKindAfterProof,
    WrongSecondPrefixInitializerTypeAfterProof,
    MissingFirstPrefixInitializerBindingAfterProof,
    MissingSecondPrefixInitializerSpanAfterProof,
    ReorderedPrefixReceiptAfterProof,
    StaleSelectedInitializerAfterProof,
    MissingSelectedInitializerSpanAfterProof,
    MismatchedSelectedInitializerSpanAfterProof,
    ForeignSelectedInitializerSourceAfterProof,
    AssignmentChildEdgeAfterProof,
    AssignmentUnitTypeAfterProof,
    AssignmentSiblingTargetAfterProof,
    AssignmentSecondMatchesRhsAfterProof,
    AssignmentStaleTargetAfterProof,
    AssignmentBeforeDeclarationAfterProof,
    AssignmentDuplicateAfterProof,
    AssignmentUnsupportedRhsAfterProof,
    AssignmentImmutableTargetAfterProof,
    AssignmentNonlocalTargetAfterProof,
}

#[derive(Debug)]
struct Receipt {
    selected: dotty_core::SymbolId,
    initializer_target: dotty_core::SymbolId,
    declaration_binding: dotty_tasty_pickler::MutableCounterLocalInitializerBinding,
    target: dotty_core::SymbolId,
    first: dotty_core::SymbolId,
    second: dotty_core::SymbolId,
    assignment: dotty_core::TreeId<dotty_core::Untyped>,
    typed_assignment: dotty_core::TreeId<Typed>,
    setter_receiver: dotty_core::TreeId<dotty_core::Untyped>,
    pair_args: [dotty_core::TreeId<dotty_core::Untyped>; 2],
    fragment: Vec<u8>,
    assignment_fragment: Vec<u8>,
}

#[derive(Debug)]
enum ValidationError {
    Proof(CrossClassConstructorCallProofError),
    Pickle(dotty_tasty_pickler::PickleError),
}

fn validate(
    source: &str,
    source_number: u32,
    mutation: Mutation,
) -> Result<Receipt, ValidationError> {
    let source_id = SourceId::from_index(source_number);
    let source_text = SourceText::new(source).unwrap();
    let scanner = ContextualScanner::new(source).unwrap();
    assert!(scanner.diagnostics().is_empty());
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let mut parsed = parse_compilation_unit(source_text, source_id, scanner, &mut store.names);
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let mut packages = Packages::new();
    let source_index = match name_compilation_unit_with_diagnostics(
        &parsed.ast,
        parsed.root,
        source_id,
        "mutable-reference-rebinding.scala",
        SourceParseStatus::Clean,
        &mut store,
        &mut packages,
    )
    .unwrap()
    {
        NamingOutcome::Named { index, diagnostics } if diagnostics.is_empty() => index,
        outcome => panic!("source names without diagnostics: {outcome:?}"),
    };
    let TreeKind::PackageDef(package) = &parsed.ast.get(parsed.root).kind else {
        panic!("package root")
    };
    let mut classes = Vec::new();
    let mut method_tree = None;
    for &class_tree in &package.stats {
        let TreeKind::TypeDef(class_def) = &parsed.ast.get(class_tree).kind else {
            continue;
        };
        let class = source_index.symbol_at(source_id, class_tree).unwrap();
        let TreeKind::Template(template) = &parsed.ast.get(class_def.rhs).kind else {
            panic!("template")
        };
        let constructor = source_index
            .symbol_at(source_id, template.constructor)
            .unwrap();
        let TreeKind::DefDef(constructor_def) = &parsed.ast.get(template.constructor).kind else {
            panic!("constructor")
        };
        let constructor_formals = constructor_def
            .value_param_clauses
            .iter()
            .flatten()
            .map(|tree| source_index.symbol_at(source_id, *tree).unwrap())
            .collect::<Vec<_>>();
        classes.push((class, constructor, constructor_formals));
        for &member in &template.body {
            let TreeKind::DefDef(def) = &parsed.ast.get(member).kind else {
                continue;
            };
            if store.names.resolve(def.name.as_name().text()) == "create" {
                method_tree = Some((member, def.clone()));
            }
        }
    }
    let (method_tree, method_def) = method_tree.expect("PairFactory.create");
    let method = source_index.symbol_at(source_id, method_tree).unwrap();
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source_id,
        &source_index,
        &mut store,
        definitions,
        &packages,
    );
    for (class, constructor, formals) in &classes {
        typer.complete_symbol(*class).unwrap();
        typer.complete_symbol(*constructor).unwrap();
        for formal in formals {
            typer.complete_symbol(*formal).unwrap();
        }
    }
    typer.complete_symbol(method).unwrap();
    for tree in method_def.value_param_clauses.iter().flatten() {
        typer
            .complete_symbol(source_index.symbol_at(source_id, *tree).unwrap())
            .unwrap();
    }
    let rhs = method_def.rhs.unwrap();
    let context = typer.expression_context_for(method).unwrap();
    typer.type_expression(rhs, context).unwrap();
    let TreeKind::Block(block) = &parsed.ast.get(rhs).kind else {
        panic!("factory Block")
    };
    assert_eq!(block.stats.len(), 5);
    let block_stats = block.stats.clone();
    let block_expr = block.expr;
    let local_bindings: [TypedLocalBinding; 3] = block.stats[..3]
        .iter()
        .map(|tree| {
            typer
                .typed_local_binding_at(source_id, *tree)
                .expect("typed local binding")
        })
        .collect::<Vec<_>>()
        .try_into()
        .unwrap();
    let mut source_typed_index = typer.source_typed_index().clone();
    let mut typed_ast = AstArena::<Typed>::new();
    for (_, node) in typer.typed_ast().iter() {
        typed_ast.alloc(node.clone());
    }
    let assignment = block.stats[3];
    let selected = local_bindings[2].symbol();
    let selected_scope = local_bindings[2].scope();
    let typed_assignment = source_typed_index.get(source_id, assignment).unwrap();
    let (source_assignment_lhs, source_assignment_rhs) = {
        let TreeKind::Assign(assign) = &parsed.ast.get(assignment).kind else {
            panic!("assignment")
        };
        (assign.lhs, assign.rhs)
    };
    drop(typer);

    let mut supplied_selected = Some(local_bindings[2]);
    match mutation {
        Mutation::None => {}
        Mutation::MissingAssignment => {
            source_typed_index.remove(source_id, assignment);
        }
        Mutation::MissingRhs => {
            source_typed_index.remove(source_id, source_assignment_rhs);
        }
        Mutation::SwapAssignmentOperands => {
            let TreeKind::Assign(assign) = &mut parsed.ast.get_mut(assignment).kind else {
                unreachable!()
            };
            std::mem::swap(&mut assign.lhs, &mut assign.rhs);
        }
        Mutation::WrongBinding => {
            supplied_selected = Some(local_bindings[0]);
        }
        Mutation::MissingSelectedBinding => supplied_selected = None,
        Mutation::ImmutableSelected => {
            store.symbols.get_mut(selected).flags = store
                .symbols
                .get(selected)
                .flags
                .difference(dotty_core::SymbolFlags::MUTABLE);
        }
        Mutation::MissingSourceVarModifier => {
            let TreeKind::ValDef(selected_def) = &mut parsed.ast.get_mut(block_stats[2]).kind
            else {
                unreachable!()
            };
            selected_def.metadata.modifiers.clear();
        }
        Mutation::ExtraSourceVarModifier => {
            let TreeKind::ValDef(selected_def) = &mut parsed.ast.get_mut(block_stats[2]).kind
            else {
                unreachable!()
            };
            selected_def
                .metadata
                .modifiers
                .push(dotty_core::ast::Modifier::Lazy);
        }
        Mutation::WrongOwner => {
            let owner = store.symbols.get(method).owner.unwrap();
            store.symbols.get_mut(selected).owner = Some(owner);
        }
        Mutation::WrongScope => {
            store.scopes.get_mut(selected_scope).owner = Some(selected);
        }
        Mutation::WrongInitializer => {
            let second_initializer = match &parsed.ast.get(block_stats[1]).kind {
                TreeKind::ValDef(def) => def.rhs,
                _ => unreachable!(),
            };
            let TreeKind::ValDef(selected_def) = &mut parsed.ast.get_mut(block_stats[2]).kind
            else {
                unreachable!()
            };
            selected_def.rhs = second_initializer;
        }
        Mutation::WrongFirstInitializerSourceSelectName
        | Mutation::WrongFirstInitializerTypedSelectName
        | Mutation::WrongSecondInitializerSourceSelectName
        | Mutation::WrongSecondInitializerTypedSelectName => {
            let initializer_index = if matches!(
                mutation,
                Mutation::WrongFirstInitializerSourceSelectName
                    | Mutation::WrongFirstInitializerTypedSelectName
            ) {
                0
            } else {
                1
            };
            let initializer = match &parsed.ast.get(block_stats[initializer_index]).kind {
                TreeKind::ValDef(def) => def.rhs.unwrap(),
                _ => unreachable!(),
            };
            let TreeKind::Apply(apply) = &parsed.ast.get(initializer).kind else {
                unreachable!()
            };
            let source_function = apply.function;
            let selection_name =
                dotty_core::Name::new(store.names.intern("create"), dotty_core::Namespace::Term);
            if matches!(
                mutation,
                Mutation::WrongFirstInitializerSourceSelectName
                    | Mutation::WrongSecondInitializerSourceSelectName
            ) {
                let TreeKind::Select(selection) = &mut parsed.ast.get_mut(source_function).kind
                else {
                    unreachable!()
                };
                selection.name = selection_name;
            } else {
                let typed_function = source_typed_index.get(source_id, source_function).unwrap();
                let TreeKind::Select(selection) = &mut typed_ast.get_mut(typed_function).kind
                else {
                    unreachable!()
                };
                selection.name = selection_name;
            }
        }
        Mutation::MissingSelectedAnnotation => {
            let selected_type = match &parsed.ast.get(block_stats[2]).kind {
                TreeKind::ValDef(def) => def.tpt,
                _ => unreachable!(),
            };
            source_typed_index.remove(source_id, selected_type);
        }
        Mutation::WrongSelectedAnnotation => {
            let selected_type = match &parsed.ast.get(block_stats[2]).kind {
                TreeKind::ValDef(def) => def.tpt,
                _ => unreachable!(),
            };
            let first_type = match &parsed.ast.get(block_stats[0]).kind {
                TreeKind::ValDef(def) => def.tpt,
                _ => unreachable!(),
            };
            let typed_first_type = source_typed_index.get(source_id, first_type).unwrap();
            source_typed_index.remove(source_id, selected_type);
            source_typed_index
                .insert(source_id, selected_type, typed_first_type)
                .unwrap();
        }
        Mutation::SelectedInitializerNotIdent => {
            let selected_initializer = match &parsed.ast.get(block_stats[2]).kind {
                TreeKind::ValDef(def) => def.rhs.unwrap(),
                _ => unreachable!(),
            };
            let typed_initializer = source_typed_index
                .get(source_id, selected_initializer)
                .unwrap();
            let assignment_kind = typed_ast.get(typed_assignment).kind.clone();
            let initializer_type = typed_ast.get(typed_initializer).ty;
            typed_ast.get_mut(typed_initializer).kind = assignment_kind;
            typed_ast.get_mut(typed_initializer).ty = initializer_type;
        }
        Mutation::WrongAssignmentTarget => {
            let TreeKind::Assign(assign) = &mut parsed.ast.get_mut(assignment).kind else {
                unreachable!()
            };
            assign.rhs = source_assignment_lhs;
        }
        Mutation::StaleAssignmentTarget => {
            let TreeKind::Assign(assign) = &typed_ast.get(typed_assignment).kind else {
                unreachable!()
            };
            let rhs = assign.rhs;
            let prefix = match store.types.get(typed_ast.get(rhs).ty) {
                dotty_core::Type::TermRef { prefix, .. } => *prefix,
                _ => unreachable!(),
            };
            let stale_type = store.types.alloc(dotty_core::Type::TermRef {
                prefix,
                target: dotty_core::TermRefTarget::Symbol(stale_symbol_id(&store)),
            });
            typed_ast.get_mut(rhs).ty = stale_type;
        }
        Mutation::WrongSetterReceiver => {
            let TreeKind::Apply(setter) = &parsed.ast.get(block_stats[4]).kind else {
                unreachable!()
            };
            let function = setter.function;
            let TreeKind::Select(selection) = &mut parsed.ast.get_mut(function).kind else {
                unreachable!()
            };
            selection.qualifier = source_assignment_rhs;
        }
        Mutation::WrongPairArguments => {
            let TreeKind::Apply(pair) = &mut parsed.ast.get_mut(block_expr).kind else {
                unreachable!()
            };
            pair.args.swap(0, 1);
        }
        Mutation::StalePairConstructor => {
            let TreeKind::Apply(pair) = &parsed.ast.get(block_expr).kind else {
                unreachable!()
            };
            let typed_function = source_typed_index.get(source_id, pair.function).unwrap();
            let prefix = match store.types.get(typed_ast.get(typed_function).ty) {
                dotty_core::Type::TermRef { prefix, .. } => *prefix,
                _ => unreachable!(),
            };
            let stale_type = store.types.alloc(dotty_core::Type::TermRef {
                prefix,
                target: dotty_core::TermRefTarget::Symbol(stale_symbol_id(&store)),
            });
            typed_ast.get_mut(typed_function).ty = stale_type;
        }
        Mutation::WrongPairNewTypeTreeKind => {
            let TreeKind::Apply(pair) = &parsed.ast.get(block_expr).kind else {
                unreachable!()
            };
            let pair_function = pair.function;
            let TreeKind::Select(selection) = &parsed.ast.get(pair_function).kind else {
                unreachable!()
            };
            let pair_new = selection.qualifier;
            let TreeKind::New(new) = &parsed.ast.get(pair_new).kind else {
                unreachable!()
            };
            let typed_type_tree = source_typed_index.get(source_id, new.tpt).unwrap();
            let assignment_kind = typed_ast.get(typed_assignment).kind.clone();
            let original_type = typed_ast.get(typed_type_tree).ty;
            typed_ast.get_mut(typed_type_tree).kind = assignment_kind;
            typed_ast.get_mut(typed_type_tree).ty = original_type;
        }
        Mutation::WrongPairNewType => {
            let TreeKind::Apply(pair) = &parsed.ast.get(block_expr).kind else {
                unreachable!()
            };
            let TreeKind::Select(selection) = &parsed.ast.get(pair.function).kind else {
                unreachable!()
            };
            let TreeKind::New(new) = &parsed.ast.get(selection.qualifier).kind else {
                unreachable!()
            };
            let typed_type_tree = source_typed_index.get(source_id, new.tpt).unwrap();
            typed_ast.get_mut(typed_type_tree).ty = definitions.int;
        }
        Mutation::WrongBlockType => {
            let typed_block = source_typed_index.get(source_id, rhs).unwrap();
            typed_ast.get_mut(typed_block).ty = definitions.int;
        }
        Mutation::TypedPairApplyKind => {
            let typed_pair = source_typed_index.get(source_id, block_expr).unwrap();
            let TreeKind::Apply(pair) = &mut typed_ast.get_mut(typed_pair).kind else {
                unreachable!()
            };
            pair.kind = dotty_core::ast::ApplyKind::Using;
        }
        Mutation::TypedSetterApplyKind => {
            let typed_setter = source_typed_index.get(source_id, block_stats[4]).unwrap();
            let TreeKind::Apply(setter) = &mut typed_ast.get_mut(typed_setter).kind else {
                unreachable!()
            };
            setter.kind = dotty_core::ast::ApplyKind::Using;
        }
        Mutation::SourceSetterSelectName | Mutation::TypedSetterSelectName => {
            let TreeKind::Apply(setter) = &parsed.ast.get(block_stats[4]).kind else {
                unreachable!()
            };
            let source_function = setter.function;
            let selection_name =
                dotty_core::Name::new(store.names.intern("create"), dotty_core::Namespace::Term);
            if matches!(mutation, Mutation::SourceSetterSelectName) {
                let TreeKind::Select(selection) = &mut parsed.ast.get_mut(source_function).kind
                else {
                    unreachable!()
                };
                selection.name = selection_name;
            } else {
                let typed_function = source_typed_index.get(source_id, source_function).unwrap();
                let TreeKind::Select(selection) = &mut typed_ast.get_mut(typed_function).kind
                else {
                    unreachable!()
                };
                selection.name = selection_name;
            }
        }
        Mutation::SourcePairSelectName | Mutation::TypedPairSelectName => {
            let TreeKind::Apply(pair) = &parsed.ast.get(block_expr).kind else {
                unreachable!()
            };
            let source_function = pair.function;
            let selection_name =
                dotty_core::Name::new(store.names.intern("create"), dotty_core::Namespace::Term);
            if matches!(mutation, Mutation::SourcePairSelectName) {
                let TreeKind::Select(selection) = &mut parsed.ast.get_mut(source_function).kind
                else {
                    unreachable!()
                };
                selection.name = selection_name;
            } else {
                let typed_function = source_typed_index.get(source_id, source_function).unwrap();
                let TreeKind::Select(selection) = &mut typed_ast.get_mut(typed_function).kind
                else {
                    unreachable!()
                };
                selection.name = selection_name;
            }
        }
        Mutation::WrongResult => {
            let typed_pair = source_typed_index.get(source_id, block_expr).unwrap();
            let local_type = match store.symbols.get(selected).info {
                dotty_core::SymbolInfo::Complete(ty) => ty,
                _ => unreachable!(),
            };
            typed_ast.get_mut(typed_pair).ty = local_type;
        }
        Mutation::ReorderStatements => {
            let TreeKind::Block(block) = &mut parsed.ast.get_mut(rhs).kind else {
                unreachable!()
            };
            block.stats.swap(3, 4);
        }
        Mutation::DuplicateAssignment => {
            let TreeKind::Block(block) = &mut parsed.ast.get_mut(rhs).kind else {
                unreachable!()
            };
            block.stats.push(assignment);
        }
        Mutation::ForeignAssignedType => {
            let TreeKind::Assign(assign) = &typed_ast.get(typed_assignment).kind else {
                unreachable!()
            };
            let rhs = assign.rhs;
            let TreeKind::PackageDef(package) = &parsed.ast.get(parsed.root).kind else {
                unreachable!()
            };
            let foreign = package
                .stats
                .iter()
                .find_map(|tree| {
                    let TreeKind::TypeDef(def) = &parsed.ast.get(*tree).kind else {
                        return None;
                    };
                    (store.names.resolve(def.name.as_name().text()) == "Pair")
                        .then(|| source_index.symbol_at(source_id, *tree).unwrap())
                })
                .unwrap();
            let foreign_type = store.types.alloc(dotty_core::Type::TermRef {
                prefix: definitions.no_prefix,
                target: dotty_core::TermRefTarget::Symbol(foreign),
            });
            typed_ast.get_mut(rhs).ty = foreign_type;
        }
        Mutation::MissingSourceBlockPosition => parsed.ast.get_mut(rhs).position = None,
        Mutation::MissingSourceSelectedPosition => {
            parsed.ast.get_mut(block_stats[2]).position = None;
        }
        Mutation::ForeignSourceSelectedPosition => {
            let position = parsed.ast.get(block_stats[2]).position.unwrap();
            parsed.ast.get_mut(block_stats[2]).position = Some(dotty_core::SourceSpan::new(
                SourceId::from_index(source_number.wrapping_add(1)),
                position.span(),
            ));
        }
        Mutation::MissingTypedAssignmentPosition => {
            typed_ast.get_mut(typed_assignment).position = None;
        }
        Mutation::MismatchedTypedAssignmentPosition => {
            let rhs_position = parsed.ast.get(source_assignment_rhs).position.unwrap();
            typed_ast.get_mut(typed_assignment).position = Some(rhs_position);
        }
        Mutation::WrongFirstPrefixInitializerKindAfterProof
        | Mutation::WrongSecondPrefixInitializerTypeAfterProof
        | Mutation::MissingFirstPrefixInitializerBindingAfterProof
        | Mutation::MissingSecondPrefixInitializerSpanAfterProof
        | Mutation::ReorderedPrefixReceiptAfterProof
        | Mutation::StaleSelectedInitializerAfterProof
        | Mutation::MissingSelectedInitializerSpanAfterProof
        | Mutation::MismatchedSelectedInitializerSpanAfterProof
        | Mutation::ForeignSelectedInitializerSourceAfterProof
        | Mutation::AssignmentChildEdgeAfterProof
        | Mutation::AssignmentUnitTypeAfterProof
        | Mutation::AssignmentSiblingTargetAfterProof
        | Mutation::AssignmentSecondMatchesRhsAfterProof
        | Mutation::AssignmentStaleTargetAfterProof
        | Mutation::AssignmentBeforeDeclarationAfterProof
        | Mutation::AssignmentDuplicateAfterProof
        | Mutation::AssignmentUnsupportedRhsAfterProof
        | Mutation::AssignmentImmutableTargetAfterProof
        | Mutation::AssignmentNonlocalTargetAfterProof => {}
    }
    let current_stats_len = match &parsed.ast.get(rhs).kind {
        TreeKind::Block(block) => block.stats.len(),
        _ => panic!("Block survives mutation"),
    };
    let (
        mut declaration_binding,
        mut assignment_binding,
        selected_receipt,
        initializer_target_receipt,
        assigned_target_receipt,
        semantic_originals,
        assignment_receipt,
        typed_assignment_receipt,
        setter_receiver_receipt,
        pair_args_receipt,
        initializer_bindings,
    ) = {
        let jvm_unit = JvmCompilationUnit {
            store: &store,
            typed_ast: &typed_ast,
            classes: Vec::new(),
        };
        let proof = validate_mutable_local_reference_reassignment_proof(
            MutableLocalReferenceReassignmentProofInput {
                source_id,
                method_tree,
                store: &store,
                definitions: &definitions,
                source_ast: &parsed.ast,
                typed_ast: &typed_ast,
                source_index: &source_index,
                source_typed_index: &source_typed_index,
                original_bindings: [Some(local_bindings[0]), Some(local_bindings[1])],
                selected_binding: supplied_selected,
                jvm_unit: &jvm_unit,
            },
        )
        .map_err(ValidationError::Proof)?;
        assert_eq!(proof.scope(), selected_scope);
        assert_eq!(proof.source_block(), rhs);
        assert_eq!(proof.source_assignment(), assignment);
        assert_eq!(proof.typed_assignment(), typed_assignment);
        assert_eq!(proof.source_assignment_lhs(), source_assignment_lhs);
        assert_eq!(proof.source_assignment_rhs(), source_assignment_rhs);
        assert_eq!(
            current_stats_len, 5,
            "accepted Block has the frozen statement count"
        );
        let declaration_binding = mutable_local_reference_declaration_binding(&proof);
        let assignment_binding = mutable_local_reference_assignment_binding(&proof);
        let initializer_bindings = fragment_initializer_bindings(
            FragmentInitializerSession {
                source_id,
                method,
                scope: selected_scope,
                source_block: rhs,
                typed_block: source_typed_index
                    .get(source_id, rhs)
                    .expect("typed source block"),
                store: &store,
                source_ast: &parsed.ast,
                typed_ast: &typed_ast,
                source_index: &source_index,
                typed_bindings: &source_typed_index,
            },
            declaration_binding.prefix_locals,
            declaration_binding.prefix_source_declarations,
        );
        (
            declaration_binding,
            assignment_binding,
            proof.selected(),
            proof.initializer_target(),
            proof.assigned_target(),
            proof.semantic_originals(),
            proof.source_assignment(),
            proof.typed_assignment(),
            proof.source_setter_receiver(),
            proof.source_pair_arguments(),
            initializer_bindings,
        )
    };
    match mutation {
        Mutation::WrongFirstPrefixInitializerKindAfterProof => {
            let typed_rhs = initializer_bindings[0].typed_initializer;
            let typed_argument = initializer_bindings[0].call.typed_argument;
            let replacement = typed_ast.get(typed_argument).kind.clone();
            typed_ast.get_mut(typed_rhs).kind = replacement;
        }
        Mutation::WrongSecondPrefixInitializerTypeAfterProof => {
            let typed_rhs = initializer_bindings[1].typed_initializer;
            typed_ast.get_mut(typed_rhs).ty = definitions.int;
        }
        Mutation::MissingFirstPrefixInitializerBindingAfterProof => {
            source_typed_index.remove(source_id, initializer_bindings[0].source_initializer);
        }
        Mutation::MissingSecondPrefixInitializerSpanAfterProof => {
            typed_ast
                .get_mut(initializer_bindings[1].typed_initializer)
                .position = None;
        }
        Mutation::ReorderedPrefixReceiptAfterProof => {
            declaration_binding.prefix_source_declarations.swap(0, 1);
            declaration_binding.prefix_typed_declarations.swap(0, 1);
            declaration_binding.prefix_locals.swap(0, 1);
        }
        Mutation::StaleSelectedInitializerAfterProof => {
            let source_rhs = declaration_binding.source_initializer;
            let typed_declaration = declaration_binding.typed_declaration;
            let old_typed_rhs = declaration_binding.typed_initializer;
            let mut stale_arena = AstArena::<Typed>::new();
            for (_, tree) in typed_ast.iter() {
                stale_arena.alloc(tree.clone());
            }
            let stale = stale_arena.alloc(typed_ast.get(old_typed_rhs).clone());
            declaration_binding.typed_initializer = stale;
            let TreeKind::ValDef(typed_val) = &mut typed_ast.get_mut(typed_declaration).kind else {
                unreachable!()
            };
            typed_val.rhs = Some(stale);
            source_typed_index.remove(source_id, source_rhs);
            source_typed_index
                .insert(source_id, source_rhs, stale)
                .unwrap();
        }
        Mutation::MissingSelectedInitializerSpanAfterProof => {
            typed_ast
                .get_mut(declaration_binding.typed_initializer)
                .position = None;
        }
        Mutation::MismatchedSelectedInitializerSpanAfterProof => {
            let assignment_position = typed_ast.get(typed_assignment).position.unwrap();
            typed_ast
                .get_mut(declaration_binding.typed_initializer)
                .position = Some(assignment_position);
        }
        Mutation::ForeignSelectedInitializerSourceAfterProof => {
            let position = typed_ast
                .get(declaration_binding.typed_initializer)
                .position
                .unwrap();
            typed_ast
                .get_mut(declaration_binding.typed_initializer)
                .position = Some(dotty_core::SourceSpan::new(
                SourceId::from_index(999_999),
                position.span(),
            ));
        }
        Mutation::AssignmentChildEdgeAfterProof => {
            source_typed_index.remove(source_id, assignment_binding.source_rhs);
        }
        Mutation::AssignmentUnitTypeAfterProof => {
            typed_ast.get_mut(assignment_binding.typed_assignment).ty = definitions.int;
        }
        Mutation::AssignmentSiblingTargetAfterProof => {
            assignment_binding.rhs_local = local_bindings[0].symbol();
        }
        Mutation::AssignmentSecondMatchesRhsAfterProof => {
            assignment_binding.second_local = assignment_binding.rhs_local;
        }
        Mutation::AssignmentStaleTargetAfterProof => {
            assignment_binding.local = stale_symbol_id(&store);
        }
        Mutation::AssignmentBeforeDeclarationAfterProof => {
            let TreeKind::Block(block) = &mut parsed.ast.get_mut(rhs).kind else {
                unreachable!()
            };
            block.stats.swap(2, 3);
            let typed_block = source_typed_index.get(source_id, rhs).unwrap();
            let TreeKind::Block(block) = &mut typed_ast.get_mut(typed_block).kind else {
                unreachable!()
            };
            block.stats.swap(2, 3);
        }
        Mutation::AssignmentDuplicateAfterProof => {
            let TreeKind::Block(block) = &mut parsed.ast.get_mut(rhs).kind else {
                unreachable!()
            };
            block.stats[4] = assignment;
            let typed_block = source_typed_index.get(source_id, rhs).unwrap();
            let TreeKind::Block(block) = &mut typed_ast.get_mut(typed_block).kind else {
                unreachable!()
            };
            block.stats[4] = typed_assignment;
        }
        Mutation::AssignmentUnsupportedRhsAfterProof => {
            let replacement = typed_ast
                .get(assignment_binding.typed_assignment)
                .kind
                .clone();
            typed_ast.get_mut(assignment_binding.typed_rhs).kind = replacement;
        }
        Mutation::AssignmentImmutableTargetAfterProof => {
            store.symbols.get_mut(assignment_binding.local).flags = dotty_core::SymbolFlags::EMPTY;
        }
        Mutation::AssignmentNonlocalTargetAfterProof => {
            assignment_binding.local = method;
        }
        _ => {}
    }
    let factory = store.symbols.get(method).owner.unwrap();
    let factory_tree = match source_index.definition_of(factory).unwrap() {
        dotty_core::SourceDefinition::Canonical { tree, .. } => tree,
        _ => panic!("PairFactory has a canonical source definition"),
    };
    let TreeKind::TypeDef(factory_def) = &parsed.ast.get(factory_tree).kind else {
        panic!("PairFactory definition")
    };
    let TreeKind::Template(factory_template) = &parsed.ast.get(factory_def.rhs).kind else {
        panic!("PairFactory template")
    };
    let factory_constructor = source_index
        .symbol_at(source_id, factory_template.constructor)
        .unwrap();
    let local_values = block_stats[..3]
        .iter()
        .enumerate()
        .map(|(index, &source_tree)| LocalValueBinding {
            source_tree,
            symbol: local_bindings[index].symbol(),
            typed_declaration: source_typed_index.get(source_id, source_tree).unwrap(),
            scope: local_bindings[index].scope(),
        })
        .collect::<Vec<_>>();
    let planned_classes = classes.iter().map(|(class, _, _)| *class).collect();
    let planned_methods = vec![method];
    let fragment_bindings = FragmentBindings {
        typed: &source_typed_index,
        classes: planned_classes,
        methods: planned_methods,
        locals: local_values,
        source: source_id,
        method,
        binding: declaration_binding,
        assignment: assignment_binding,
        initializers: initializer_bindings,
    };
    let member = [ClassMemberPicklePlan::Method {
        symbol: method,
        source_tree: method_tree,
    }];
    let class_plan = ClassStatePicklePlan {
        store: &store,
        definitions: &definitions,
        source_ast: &parsed.ast,
        typed_ast: &typed_ast,
        source_index: &source_index,
        typed_bindings: &fragment_bindings,
        source: SourceMetadata {
            id: source_id,
            name: "MutableReferenceRebinding.scala".to_owned(),
        },
        class: factory,
        class_tree: factory_tree,
        constructor: PrimaryConstructorInputPlan {
            symbol: factory_constructor,
            source_tree: factory_template.constructor,
            parameter_clauses: Vec::new(),
        },
        members: &member,
    };
    let validated_fragment = class_plan
        .validate_mutable_counter_declaration_fragment(method)
        .map_err(ValidationError::Pickle)?;
    let fragment = validated_fragment.emit().map_err(ValidationError::Pickle)?;
    let validated_assignment_fragment = class_plan
        .validate_mutable_counter_reference_assignment_fragment(method)
        .map_err(ValidationError::Pickle)?;
    let assignment_fragment = validated_assignment_fragment
        .emit()
        .map_err(ValidationError::Pickle)?;
    Ok(Receipt {
        selected: selected_receipt,
        initializer_target: initializer_target_receipt,
        declaration_binding,
        target: assigned_target_receipt,
        first: semantic_originals[0],
        second: semantic_originals[1],
        assignment: assignment_receipt,
        typed_assignment: typed_assignment_receipt,
        setter_receiver: setter_receiver_receipt,
        pair_args: pair_args_receipt,
        fragment: fragment.bytes().to_vec(),
        assignment_fragment: assignment_fragment.bytes().to_vec(),
    })
}

fn verify_fragment_with_pinned_scala(candidate: &[u8], label: &str, expect_assignment: bool) {
    match std::env::var(REQUIRE_SCALA_FRAGMENT).as_deref() {
        Ok("1") => {}
        Ok(value) => panic!("{REQUIRE_SCALA_FRAGMENT} must equal 1, got {value:?}"),
        Err(std::env::VarError::NotPresent) => {
            eprintln!(
                "skipping pinned Scala fragment reader; set {REQUIRE_SCALA_FRAGMENT}=1 to require it"
            );
            return;
        }
        Err(error) => panic!("read {REQUIRE_SCALA_FRAGMENT}: {error}"),
    }

    let scala_root = PathBuf::from(std::env::var_os("SCALA39_ROOT").expect("SCALA39_ROOT"));
    let revision = checked_command(
        Command::new("git")
            .args(["-C"])
            .arg(&scala_root)
            .args(["rev-parse", "HEAD"]),
    );
    assert_eq!(
        String::from_utf8_lossy(&revision.stdout).trim(),
        "777528f19a58e794c9954a42f433373472ec57f8"
    );
    let java_home = PathBuf::from(std::env::var_os("JAVA_HOME").expect("JAVA_HOME"));
    let java = java_home.join("bin/java");
    let maven =
        PathBuf::from(std::env::var_os("COURSIER_MAVEN_ROOT").expect("COURSIER_MAVEN_ROOT"));
    let jar_paths = [
        maven.join("org/scala-lang/scala3-library_3/3.9.0/scala3-library_3-3.9.0.jar"),
        maven.join("org/scala-lang/scala-library/3.9.0/scala-library-3.9.0.jar"),
        maven.join("org/scala-lang/scala3-compiler_3/3.9.0/scala3-compiler_3-3.9.0.jar"),
        maven.join("org/scala-lang/tasty-core_3/3.9.0/tasty-core_3-3.9.0.jar"),
        maven.join("org/scala-lang/scala3-interfaces/3.9.0/scala3-interfaces-3.9.0.jar"),
        maven.join("org/scala-lang/modules/scala-asm/9.9.0-scala-1/scala-asm-9.9.0-scala-1.jar"),
        maven.join("org/scala-sbt/compiler-interface/1.12.0/compiler-interface-1.12.0.jar"),
        maven.join("org/scala-sbt/util-interface/1.11.5/util-interface-1.11.5.jar"),
    ];
    let expected_jar_hashes = [
        "9343f9651453945fc6a9b75bb4fd230418ee28a1f88ea28ee429f3ebe0511aba",
        "b844bf6fb3080c73df6b9c523b7a69b9ef4a980bb15e5c37c35443d1d0b0e0cd",
        "3e2c09a5ca6cef17a55f2f66e123815f9c4ea061fce20d0e9d2371989405a61a",
        "66e12dda98bb23e864f3a31f8527c8338bf47bce9b85cc380b368c69c1ba8d0f",
        "64dfde1d2c6309d65a01c5f3ea836137eb84338dcadeb16f85764561ef4e10db",
        "75ac366e8ecb691e06a7e85041eed0f67919a646e5262fa0901225698c104375",
        "92f487a505eafdd0a033e4ff4f38a81509a655c76257c64e6808dd873cc145c3",
        "1fc950a9e1a35385e79316e23c120d4fd184f6a7af0274e00a703b0dc76376af",
    ];
    for (jar, expected_hash) in jar_paths.iter().zip(expected_jar_hashes) {
        assert!(
            jar.is_file(),
            "pinned Scala dependency exists: {}",
            jar.display()
        );
        let digest = checked_command(Command::new("sha256sum").arg(jar));
        let actual_hash = String::from_utf8_lossy(&digest.stdout)
            .split_whitespace()
            .next()
            .expect("sha256sum emits a digest")
            .to_owned();
        assert_eq!(
            actual_hash,
            expected_hash,
            "Scala dependency hash {}",
            jar.display()
        );
    }
    let compiler_classpath = std::env::join_paths(&jar_paths).expect("compiler classpath");
    let java_version = checked_command(Command::new(&java).args(["-version"]));
    let java_version = format!(
        "{}{}",
        String::from_utf8_lossy(&java_version.stdout),
        String::from_utf8_lossy(&java_version.stderr)
    );
    assert!(java_version.contains("21.0.13"), "{java_version}");
    let java_hash = checked_command(Command::new("sha256sum").arg(&java));
    assert_eq!(
        String::from_utf8_lossy(&java_hash.stdout)
            .split_whitespace()
            .next()
            .expect("sha256sum emits a digest"),
        "9cad3f2620d3ef2ed10b26f922af7e773d54fdaa05f615d6d6df014f3fec4155"
    );
    let version = checked_command(
        Command::new(&java)
            .arg("-XX:-UsePerfData")
            .arg(format!(
                "-Djava.io.tmpdir={}",
                std::env::temp_dir().display()
            ))
            .args(["-cp"])
            .arg(&compiler_classpath)
            .args(["dotty.tools.dotc.Main", "-version"]),
    );
    assert!(String::from_utf8_lossy(&version.stdout).contains("Scala compiler version 3.9.0 --"));

    let root = scala_fragment_scratch(label);
    let candidate_root = root.join("candidate");
    let package_dir = candidate_root.join("example");
    let dependencies = root.join("dependencies");
    std::fs::create_dir_all(&package_dir).expect("create candidate package directory");
    std::fs::create_dir(&dependencies).expect("create dependency directory");
    let candidate_path = package_dir.join("PairFactory.tasty");
    let support_source = root.join("CounterPair.scala");
    std::fs::write(&candidate_path, candidate).expect("write Rust fragment candidate");
    std::fs::write(&support_source, SCALA_FRAGMENT_SUPPORT).expect("write Scala dependencies");

    checked_command(
        Command::new(&java)
            .arg("-XX:-UsePerfData")
            .arg(format!(
                "-Djava.io.tmpdir={}",
                std::env::temp_dir().display()
            ))
            .args(["-cp"])
            .arg(&compiler_classpath)
            .args(["dotty.tools.dotc.Main", "-classpath"])
            .arg(&compiler_classpath)
            .args(["-release", "21", "-d"])
            .arg(&dependencies)
            .arg(&support_source),
    );
    let reader_classpath = std::env::join_paths(
        std::iter::once(candidate_root.as_path())
            .chain(std::iter::once(dependencies.as_path()))
            .chain(jar_paths.iter().map(PathBuf::as_path)),
    )
    .expect("reader classpath");
    let decompiled = checked_command(
        Command::new(&java)
            .arg("-XX:-UsePerfData")
            .arg(format!(
                "-Djava.io.tmpdir={}",
                std::env::temp_dir().display()
            ))
            .args(["-cp"])
            .arg(&compiler_classpath)
            .args(["dotty.tools.dotc.decompiler.Main", "-classpath"])
            .arg(reader_classpath)
            .args(["-color:never"])
            .arg(&candidate_path),
    );
    let decompiled = String::from_utf8_lossy(&decompiled.stdout);
    assert!(
        decompiled.contains("var selected: example.Counter = first"),
        "Scala 3.9 decompiles Rust mutable VALDEF and original target:\n{decompiled}"
    );
    if expect_assignment {
        assert!(
            decompiled.contains("selected = second"),
            "Scala 3.9 decompiles the original reference ASSIGN with its exact RHS:\n{decompiled}"
        );
        assert!(!decompiled.contains("selected = first"), "{decompiled}");
    } else {
        assert!(
            decompiled.contains("\n      first\n"),
            "Scala 3.9 decompiles the original initializer target as the fragment result:\n{decompiled}"
        );
    }
    assert!(!dependencies.join("example/PairFactory.tasty").exists());
    std::fs::remove_dir_all(&root).expect("remove fragment reader scratch");
}

fn checked_command(command: &mut Command) -> Output {
    let output = command.output().unwrap_or_else(|error| {
        panic!("start command {command:?}: {error}");
    });
    assert!(
        output.status.success(),
        "command failed: {command:?}\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn scala_fragment_scratch(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock follows Unix epoch")
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "dotty-mutable-counter-fragment-{}-{label}-{nonce}",
        std::process::id()
    ));
    std::fs::create_dir(&path).expect("create unique fragment reader scratch");
    path
}

#[test]
fn mutable_reference_reassignment_receipt_accepts_canonical_variants() {
    let original = validate(SOURCE, 22_990, Mutation::None).unwrap();
    assert_ne!(original.first, original.second);
    assert!(!original.fragment.is_empty());
    TastyFile::parse_and_validate_scala_3_9(&original.fragment)
        .expect("Rust-emitted mutable declaration fragment is well-formed TASTy 28.9");
    TastyFile::parse_and_validate_scala_3_9(&original.assignment_fragment)
        .expect("Rust-emitted mutable assignment fragment is well-formed TASTy 28.9");
    verify_fragment_with_pinned_scala(&original.fragment, "original", false);
    verify_fragment_with_pinned_scala(&original.assignment_fragment, "assignment", true);
    assert_eq!(original.initializer_target, original.first);
    assert_eq!(original.declaration_binding.target, original.first);
    assert_eq!(original.declaration_binding.local, original.selected);
    assert_ne!(
        original.declaration_binding.source_declaration,
        original.declaration_binding.source_block
    );
    assert_eq!(original.target, original.second);
    assert_ne!(original.setter_receiver, original.assignment);
    assert_ne!(original.pair_args[0], original.pair_args[1]);
    assert_ne!(
        original.assignment.index(),
        original.typed_assignment.index()
    );
    let renamed = SOURCE
        .replace("val first: Counter", "val alpha: Counter")
        .replace("selected: Counter = first", "selected: Counter = alpha")
        .replace("Pair(first, second)", "Pair(alpha, second)");
    let renamed = validate(&renamed, 22_991, Mutation::None).unwrap();
    assert_eq!(renamed.target, renamed.second);
    assert_ne!(renamed.selected, renamed.first);
    assert_ne!(renamed.selected, renamed.second);

    let reversed = SOURCE.replace(
        "    val first: Counter = new Counter(firstSeed)\n    val second: Counter = new Counter(secondSeed)",
        "    val second: Counter = new Counter(secondSeed)\n    val first: Counter = new Counter(firstSeed)",
    );
    let reversed = validate(&reversed, 22_992, Mutation::None).unwrap();
    verify_fragment_with_pinned_scala(&reversed.fragment, "reversed", false);
    verify_fragment_with_pinned_scala(&reversed.assignment_fragment, "reversed-assignment", true);
    assert_ne!(reversed.first, reversed.second);
    assert_eq!(reversed.initializer_target, reversed.first);
    assert_eq!(reversed.declaration_binding.target, reversed.first);
    assert_ne!(
        reversed.first,
        reversed.declaration_binding.prefix_locals[0]
    );

    let changed_target = SOURCE.replace("selected = second", "selected = first");
    assert!(matches!(
        validate(
            &changed_target,
            22_993,
            Mutation::AssignmentSecondMatchesRhsAfterProof
        ),
        Err(ValidationError::Pickle(
            dotty_tasty_pickler::PickleError::Unsupported {
                feature: dotty_tasty_pickler::UnsupportedFeature::BodyExpression,
                ..
            }
        ))
    ));
}

#[test]
fn mutable_reference_reassignment_rejects_forged_edges_and_shapes() {
    for mutation in [
        Mutation::MissingAssignment,
        Mutation::MissingRhs,
        Mutation::SwapAssignmentOperands,
        Mutation::WrongBinding,
        Mutation::MissingSelectedBinding,
        Mutation::ImmutableSelected,
        Mutation::MissingSourceVarModifier,
        Mutation::ExtraSourceVarModifier,
        Mutation::WrongOwner,
        Mutation::WrongScope,
        Mutation::WrongInitializer,
        Mutation::WrongFirstInitializerSourceSelectName,
        Mutation::WrongFirstInitializerTypedSelectName,
        Mutation::WrongSecondInitializerSourceSelectName,
        Mutation::WrongSecondInitializerTypedSelectName,
        Mutation::MissingSelectedAnnotation,
        Mutation::WrongSelectedAnnotation,
        Mutation::SelectedInitializerNotIdent,
        Mutation::WrongAssignmentTarget,
        Mutation::StaleAssignmentTarget,
        Mutation::WrongSetterReceiver,
        Mutation::WrongPairArguments,
        Mutation::StalePairConstructor,
        Mutation::WrongPairNewTypeTreeKind,
        Mutation::WrongPairNewType,
        Mutation::WrongBlockType,
        Mutation::TypedPairApplyKind,
        Mutation::TypedSetterApplyKind,
        Mutation::SourceSetterSelectName,
        Mutation::TypedSetterSelectName,
        Mutation::SourcePairSelectName,
        Mutation::TypedPairSelectName,
        Mutation::WrongResult,
        Mutation::ReorderStatements,
        Mutation::DuplicateAssignment,
        Mutation::ForeignAssignedType,
        Mutation::MissingSourceBlockPosition,
        Mutation::MissingSourceSelectedPosition,
        Mutation::ForeignSourceSelectedPosition,
        Mutation::MissingTypedAssignmentPosition,
        Mutation::MismatchedTypedAssignmentPosition,
    ] {
        assert!(
            validate(SOURCE, 23_000 + mutation as u32, mutation).is_err(),
            "{mutation:?}"
        );
    }
}

#[test]
fn assignment_mapping_failures_are_typed_binding_errors() {
    assert!(matches!(
        validate(SOURCE, 23_020, Mutation::MissingAssignment),
        Err(ValidationError::Proof(
            CrossClassConstructorCallProofError::TypedBinding(_)
        ))
    ));
    assert!(matches!(
        validate(SOURCE, 23_021, Mutation::MissingRhs),
        Err(ValidationError::Proof(
            CrossClassConstructorCallProofError::TypedBinding(_)
        ))
    ));
    for (offset, mutation) in [
        Mutation::MissingSelectedAnnotation,
        Mutation::WrongSelectedAnnotation,
        Mutation::SelectedInitializerNotIdent,
        Mutation::StaleAssignmentTarget,
        Mutation::StalePairConstructor,
        Mutation::MissingTypedAssignmentPosition,
        Mutation::MismatchedTypedAssignmentPosition,
    ]
    .into_iter()
    .enumerate()
    {
        assert!(
            matches!(
                validate(SOURCE, 23_030 + offset as u32, mutation),
                Err(ValidationError::Proof(
                    CrossClassConstructorCallProofError::TypedBinding(_)
                ))
            ),
            "{mutation:?}"
        );
    }
    for (offset, mutation) in [
        Mutation::MissingSourceBlockPosition,
        Mutation::MissingSourceSelectedPosition,
        Mutation::ForeignSourceSelectedPosition,
    ]
    .into_iter()
    .enumerate()
    {
        assert!(
            matches!(
                validate(SOURCE, 23_040 + offset as u32, mutation),
                Err(ValidationError::Proof(
                    CrossClassConstructorCallProofError::SourceIdentity(_)
                ))
            ),
            "{mutation:?}"
        );
    }
}

#[test]
fn mutable_counter_assignment_fragment_rejects_forged_targets_and_shapes() {
    for (offset, mutation) in [
        Mutation::AssignmentChildEdgeAfterProof,
        Mutation::AssignmentUnitTypeAfterProof,
        Mutation::AssignmentSiblingTargetAfterProof,
        Mutation::AssignmentStaleTargetAfterProof,
        Mutation::AssignmentBeforeDeclarationAfterProof,
        Mutation::AssignmentDuplicateAfterProof,
        Mutation::AssignmentUnsupportedRhsAfterProof,
        Mutation::AssignmentImmutableTargetAfterProof,
        Mutation::AssignmentNonlocalTargetAfterProof,
    ]
    .into_iter()
    .enumerate()
    {
        let result = validate(SOURCE, 23_150 + offset as u32, mutation);
        assert!(
            matches!(result, Err(ValidationError::Pickle(_))),
            "the assignment fragment rejects forged provenance or unsupported shape: {mutation:?}: {result:?}"
        );
    }
}

#[test]
fn mutable_declaration_requires_exact_source_var_and_mutable_flag() {
    for mutation in [
        Mutation::ImmutableSelected,
        Mutation::MissingSourceVarModifier,
        Mutation::ExtraSourceVarModifier,
    ] {
        let result = validate(SOURCE, 23_060 + mutation as u32, mutation);
        let rejected = match mutation {
            Mutation::ImmutableSelected => matches!(
                &result,
                Err(ValidationError::Proof(
                    CrossClassConstructorCallProofError::SourceIdentity(_)
                ))
            ),
            Mutation::MissingSourceVarModifier | Mutation::ExtraSourceVarModifier => matches!(
                &result,
                Err(ValidationError::Pickle(
                    dotty_tasty_pickler::PickleError::Unsupported {
                        feature: dotty_tasty_pickler::UnsupportedFeature::SymbolMetadata,
                        ..
                    }
                ))
            ),
            _ => unreachable!(),
        };
        assert!(rejected, "{mutation:?}: {result:?}");
    }
}

#[test]
fn fragment_authenticates_both_predecessor_constructor_initializers() {
    for mutation in [
        Mutation::WrongFirstPrefixInitializerKindAfterProof,
        Mutation::WrongSecondPrefixInitializerTypeAfterProof,
    ] {
        let result = validate(SOURCE, 23_080 + mutation as u32, mutation);
        assert!(
            matches!(result, Err(ValidationError::Pickle(_))),
            "fragment rejects a forged predecessor initializer at its own boundary: {mutation:?}: {result:?}"
        );
    }
    for mutation in [
        Mutation::MissingFirstPrefixInitializerBindingAfterProof,
        Mutation::MissingSecondPrefixInitializerSpanAfterProof,
        Mutation::ReorderedPrefixReceiptAfterProof,
    ] {
        let result = validate(SOURCE, 23_090 + mutation as u32, mutation);
        assert!(
            matches!(result, Err(ValidationError::Pickle(_))),
            "fragment authenticates source/typed bindings, spans, and declaration order: {mutation:?}: {result:?}"
        );
    }
}

#[test]
fn fragment_rejects_a_stale_selected_initializer_with_a_typed_error() {
    let result = validate(
        SOURCE,
        23_100 + Mutation::StaleSelectedInitializerAfterProof as u32,
        Mutation::StaleSelectedInitializerAfterProof,
    );
    assert!(
        matches!(
            result,
            Err(ValidationError::Pickle(
                dotty_tasty_pickler::PickleError::InvalidTypedTree { .. }
            ))
        ),
        "a stale selected initializer is rejected without panicking: {result:?}"
    );
}

#[test]
fn fragment_authenticates_selected_initializer_spans_after_proof() {
    for mutation in [
        Mutation::MissingSelectedInitializerSpanAfterProof,
        Mutation::MismatchedSelectedInitializerSpanAfterProof,
    ] {
        let result = validate(SOURCE, 23_110 + mutation as u32, mutation);
        assert!(
            matches!(
                result,
                Err(ValidationError::Pickle(
                    dotty_tasty_pickler::PickleError::BodySpan { .. }
                ))
            ),
            "fragment rejects absent or mismatched selected initializer span: {mutation:?}: {result:?}"
        );
    }
    let mutation = Mutation::ForeignSelectedInitializerSourceAfterProof;
    let result = validate(SOURCE, 23_120 + mutation as u32, mutation);
    assert!(
        matches!(
            result,
            Err(ValidationError::Pickle(
                dotty_tasty_pickler::PickleError::SourceSpan { .. }
            ))
        ),
        "fragment rejects a selected initializer span from another source: {result:?}"
    );
}
