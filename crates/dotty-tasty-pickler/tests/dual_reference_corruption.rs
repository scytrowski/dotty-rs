use dotty_core::ast::{AstArena, TreeKind};
use dotty_core::{
    Definitions, Packages, SemanticStore, SourceDefinition, SourceId, SourceText, SymbolFlags,
    SymbolId, SymbolInfo, SymbolOrigin, TreeId, Type, TypeRefTarget, Typed, Untyped,
};
use dotty_lexer::ContextualScanner;
use dotty_namer::{NamingOutcome, SourceParseStatus, name_compilation_unit_with_diagnostics};
use dotty_parser::parse_compilation_unit;
use dotty_tasty_pickler::{
    ClassMemberPicklePlan, ClassStatePicklePlan, DefaultConstructorPlan, PickleError, PicklePlan,
    PrimaryConstructorInputPlan, SourceMetadata, SourceTypedBindings,
};
use dotty_typer::SourceTyper;

const SOURCE: &str =
    "package example\nclass Counter(start: Int)\nclass Pair(first: Counter, second: Counter)\n";

#[derive(Clone)]
struct Bindings {
    planned_classes: Vec<SymbolId>,
}

impl SourceTypedBindings for Bindings {
    fn typed_at(&self, _source: SourceId, _tree: TreeId<Untyped>) -> Option<TreeId<Typed>> {
        None
    }

    fn planned_class_symbols(&self) -> &[SymbolId] {
        &self.planned_classes
    }
}

struct Fixture {
    store: SemanticStore,
    definitions: Definitions,
    source_ast: AstArena<Untyped>,
    typed_ast: AstArena<Typed>,
    source_index: dotty_core::SourceSemanticIndex,
    bindings: Bindings,
    source: SourceId,
    root: TreeId<Untyped>,
    pair_tree: TreeId<Untyped>,
    pair: SymbolId,
    constructor_tree: TreeId<Untyped>,
    constructor: SymbolId,
    parameters: Vec<SymbolId>,
    parameter_trees: Vec<TreeId<Untyped>>,
    members: Vec<ClassMemberPicklePlan>,
    source_name: String,
}

impl Fixture {
    fn new() -> Self {
        let source = SourceId::from_index(1876);
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let scanner = ContextualScanner::new(SOURCE).unwrap();
        assert!(scanner.diagnostics().is_empty());
        let parsed = parse_compilation_unit(
            SourceText::new(SOURCE).unwrap(),
            source,
            scanner,
            &mut store.names,
        );
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        let mut packages = Packages::new();
        let source_index = match name_compilation_unit_with_diagnostics(
            &parsed.ast,
            parsed.root,
            source,
            "PairSignatureOnly.scala",
            SourceParseStatus::Clean,
            &mut store,
            &mut packages,
        )
        .unwrap()
        {
            NamingOutcome::Named { index, diagnostics } => {
                assert!(diagnostics.is_empty(), "{diagnostics:?}");
                *index
            }
            NamingOutcome::Rejected { diagnostics } => panic!("{diagnostics:?}"),
        };

        let mut pair_tree = None;
        let mut counter = None;
        for (tree, source_tree) in parsed.ast.iter() {
            let TreeKind::TypeDef(definition) = &source_tree.kind else {
                continue;
            };
            match store.names.resolve(definition.name.as_name().text()) {
                "Pair" => pair_tree = Some(tree),
                "Counter" => counter = Some(source_index.symbol_at(source, tree).unwrap()),
                _ => {}
            }
        }
        let pair_tree = pair_tree.unwrap();
        let pair = source_index.symbol_at(source, pair_tree).unwrap();
        let pair_definition = match &parsed.ast.get(pair_tree).kind {
            TreeKind::TypeDef(definition) => definition,
            _ => unreachable!(),
        };
        let template = match &parsed.ast.get(pair_definition.rhs).kind {
            TreeKind::Template(template) => template,
            _ => unreachable!(),
        };
        let constructor_tree = template.constructor;
        let constructor = source_index.symbol_at(source, constructor_tree).unwrap();
        let constructor_definition = match &parsed.ast.get(constructor_tree).kind {
            TreeKind::DefDef(definition) => definition,
            _ => unreachable!(),
        };
        let parameter_trees = constructor_definition.value_param_clauses[0].clone();
        let parameters = parameter_trees
            .iter()
            .map(|&tree| {
                source_index
                    .derived_symbol_at(constructor, source, tree)
                    .unwrap()
            })
            .collect::<Vec<_>>();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &source_index,
            &mut store,
            definitions,
            &packages,
        );
        let counter = counter.unwrap();
        let counter_tree = match source_index.definition_of(counter).unwrap() {
            SourceDefinition::Canonical { tree, .. } => tree,
            _ => unreachable!(),
        };
        let counter_constructor = match &parsed.ast.get(counter_tree).kind {
            TreeKind::TypeDef(definition) => match &parsed.ast.get(definition.rhs).kind {
                TreeKind::Template(template) => source_index
                    .symbol_at(source, template.constructor)
                    .unwrap(),
                _ => unreachable!(),
            },
            _ => unreachable!(),
        };
        typer.complete_symbol(counter).unwrap();
        typer.complete_symbol(counter_constructor).unwrap();
        typer.complete_symbol(pair).unwrap();
        typer.complete_symbol(constructor).unwrap();
        for (&tree, &parameter) in parameter_trees.iter().zip(&parameters) {
            let canonical = source_index.symbol_at(source, tree).unwrap();
            typer.complete_symbol(canonical).unwrap();
            typer.complete_symbol(parameter).unwrap();
        }
        let typed_ast = clone_typed_arena(typer.typed_ast());
        drop(typer);

        let mut source_ast = clone_source_arena(&parsed.ast);
        let TreeKind::PackageDef(package) = &mut source_ast.get_mut(parsed.root).kind else {
            unreachable!()
        };
        package.stats = vec![pair_tree];
        Self {
            store,
            definitions,
            source_ast,
            typed_ast,
            source_index,
            bindings: Bindings {
                planned_classes: vec![counter, pair],
            },
            source,
            root: parsed.root,
            pair_tree,
            pair,
            constructor_tree,
            constructor,
            parameters,
            parameter_trees,
            members: Vec::new(),
            source_name: "PairSignatureOnly.scala".to_owned(),
        }
    }

    fn class_state_error(&self) -> Result<(), PickleError> {
        let plan = ClassStatePicklePlan {
            store: &self.store,
            definitions: &self.definitions,
            source_ast: &self.source_ast,
            typed_ast: &self.typed_ast,
            source_index: &self.source_index,
            typed_bindings: &self.bindings,
            source: SourceMetadata {
                id: self.source,
                name: self.source_name.clone(),
            },
            class: self.pair,
            class_tree: self.pair_tree,
            constructor: PrimaryConstructorInputPlan {
                symbol: self.constructor,
                source_tree: self.constructor_tree,
                parameter_clauses: vec![self.parameters.clone()],
            },
            members: &self.members,
        };
        plan.validate()
            .and_then(|validated| validated.pickle().map(|_| ()))
    }

    fn declaration_error(&self) -> Result<(), PickleError> {
        let plan = PicklePlan {
            store: &self.store,
            definitions: &self.definitions,
            typed_ast: &self.typed_ast,
            source_ast: &self.source_ast,
            source_index: &self.source_index,
            typed_bindings: &self.bindings,
            source: SourceMetadata {
                id: self.source,
                name: self.source_name.clone(),
            },
            source_root: self.root,
            class: self.pair,
            class_tree: self.pair_tree,
            constructor: DefaultConstructorPlan {
                symbol: self.constructor,
                source_tree: self.constructor_tree,
                parameter_clauses: vec![self.parameters.clone()],
            },
            methods: &[],
            local_values: &[],
        };
        plan.validate()
            .and_then(|validated| validated.declaration_skeleton().map(|_| ()))
    }

    fn second_reference_type(&self) -> (dotty_core::TypeId, dotty_core::SymbolId) {
        let info = self.store.symbols.get(self.parameters[1]).info;
        let SymbolInfo::Complete(ty) = info else {
            panic!("second constructor formal is complete");
        };
        let Some(Type::TypeRef {
            prefix,
            target: TypeRefTarget::Symbol(class),
        }) = self.store.types.try_get(ty)
        else {
            panic!("second formal has a canonical class reference type");
        };
        (*prefix, *class)
    }

    fn rewrite_constructor_signature(&mut self, second_type: dotty_core::TypeId) {
        let info = self.store.symbols.get(self.constructor).info;
        let SymbolInfo::Complete(ty) = info else {
            panic!("constructor signature is complete");
        };
        let Some(Type::Method(signature)) = self.store.types.try_get(ty).cloned() else {
            panic!("constructor has a method type");
        };
        let mut changed = signature;
        changed.params[1].ty = second_type;
        let changed_type = self.store.types.alloc(Type::Method(changed));
        self.store.symbols.get_mut(self.constructor).info = SymbolInfo::Complete(changed_type);
    }
}

fn clone_source_arena(arena: &AstArena<Untyped>) -> AstArena<Untyped> {
    let mut clone = AstArena::new();
    for (tree, node) in arena.iter() {
        assert_eq!(clone.alloc(node.clone()), tree);
    }
    clone
}

fn clone_typed_arena(arena: &AstArena<Typed>) -> AstArena<Typed> {
    let mut clone = AstArena::new();
    for (tree, node) in arena.iter() {
        assert_eq!(clone.alloc(node.clone()), tree);
    }
    clone
}

#[derive(Clone, Copy, Debug)]
enum Corruption {
    SecondSemanticType,
    SecondMethodType,
    ForeignSameNameClass,
    SecondSourceOrdinal,
    SecondOwner,
    SecondFlags,
    SecondScope,
}

fn corrupt(fixture: &mut Fixture, corruption: Corruption) -> PickleError {
    let second = fixture.parameters[1];
    let int = fixture.definitions.int;
    match corruption {
        Corruption::SecondSemanticType => {
            let canonical = fixture
                .source_index
                .symbol_at(fixture.source, fixture.parameter_trees[1])
                .unwrap();
            let SymbolInfo::Complete(expected_type) = fixture.store.symbols.get(canonical).info
            else {
                panic!("canonical class parameter type is complete");
            };
            fixture.store.symbols.get_mut(second).info = SymbolInfo::Complete(int);
            PickleError::UnsupportedTypeReference { ty: expected_type }
        }
        Corruption::SecondMethodType => {
            fixture.rewrite_constructor_signature(int);
            PickleError::UnsupportedTypeReference { ty: int }
        }
        Corruption::ForeignSameNameClass => {
            let (prefix, class) = fixture.second_reference_type();
            let mut foreign = fixture.store.symbols.get(class).clone();
            foreign.origin = SymbolOrigin::Source(SourceId::from_index(1877));
            let foreign = fixture.store.symbols.alloc(foreign);
            let foreign_type = fixture.store.types.alloc(Type::type_ref(prefix, foreign));
            fixture.store.symbols.get_mut(second).info = SymbolInfo::Complete(foreign_type);
            fixture.rewrite_constructor_signature(foreign_type);
            PickleError::UnsupportedTypeReference { ty: foreign_type }
        }
        Corruption::SecondSourceOrdinal => {
            let TreeKind::DefDef(definition) =
                &mut fixture.source_ast.get_mut(fixture.constructor_tree).kind
            else {
                unreachable!()
            };
            definition.value_param_clauses[0][1] = fixture.parameter_trees[0];
            PickleError::ScopeMembers {
                owner: fixture.constructor,
            }
        }
        Corruption::SecondOwner => {
            fixture.store.symbols.get_mut(second).owner = Some(fixture.pair);
            PickleError::Owner {
                symbol: second,
                expected: Some(fixture.constructor),
                actual: Some(fixture.pair),
            }
        }
        Corruption::SecondFlags => {
            fixture.store.symbols.get_mut(second).flags = SymbolFlags::MUTABLE;
            PickleError::SourceDefinition { symbol: second }
        }
        Corruption::SecondScope => {
            let scope = fixture
                .source_index
                .scope_of(fixture.constructor)
                .expect("constructor scope");
            fixture.store.scopes.get_mut(scope).remove(second);
            PickleError::ScopeMembers {
                owner: fixture.constructor,
            }
        }
    }
}

#[test]
fn second_formal_corruptions_fail_at_the_first_pickler_stage_without_output() {
    for corruption in [
        Corruption::SecondSemanticType,
        Corruption::SecondMethodType,
        Corruption::ForeignSameNameClass,
        Corruption::SecondSourceOrdinal,
        Corruption::SecondOwner,
        Corruption::SecondFlags,
        Corruption::SecondScope,
    ] {
        let mut fixture = Fixture::new();
        let expected = corrupt(&mut fixture, corruption);
        let class_state = fixture.class_state_error().unwrap_err();
        let declaration = fixture.declaration_error().unwrap_err();
        assert_eq!(class_state, expected, "class-state {corruption:?}");
        let expected_declaration = match corruption {
            Corruption::SecondSemanticType => PickleError::Unsupported {
                feature: dotty_tasty_pickler::UnsupportedFeature::PrimaryConstructorInput,
                symbol: Some(fixture.parameters[1]),
            },
            Corruption::ForeignSameNameClass => PickleError::SourceDefinition {
                symbol: fixture.second_reference_type().1,
            },
            Corruption::SecondSourceOrdinal => PickleError::SourceBinding {
                source: fixture.source,
                tree: fixture.parameter_trees[0],
                expected: fixture.parameters[1],
                actual: Some(fixture.parameters[0]),
            },
            Corruption::SecondOwner => PickleError::SourceDefinition {
                symbol: fixture.parameters[1],
            },
            Corruption::SecondScope => PickleError::Unsupported {
                feature: dotty_tasty_pickler::UnsupportedFeature::PrimaryConstructorInput,
                symbol: Some(fixture.parameters[1]),
            },
            _ => expected,
        };
        assert_eq!(
            declaration, expected_declaration,
            "declaration {corruption:?}"
        );
    }
}

#[test]
fn uncorrupted_two_reference_fixture_emits_both_tasty_paths() {
    let fixture = Fixture::new();
    fixture.class_state_error().unwrap();
    fixture.declaration_error().unwrap();
}
