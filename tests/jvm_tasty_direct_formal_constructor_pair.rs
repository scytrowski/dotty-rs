use dotty::jvm_tasty_plan::with_paired_jvm_tasty_batch_plan;
use dotty_core::ast::{AstArena, Typed, Untyped};
use dotty_core::{SourceId, SymbolId, TreeId};
use dotty_tasty_pickler::{
    ClassStatePicklePlan, DirectFormalConstructorCallPairBinding, LocalValueBinding, PickleError,
    PlannedCallBinding, SourceTypedBindings, ValidatedClassStatePlan,
};

const SOURCE: &str = include_str!("fixtures/jvm_tasty_dual_reference_forwarding/PairFactory.scala");

struct DirectFormalBindings<'a> {
    base: &'a dyn SourceTypedBindings,
    source: SourceId,
    method: SymbolId,
    receipt: DirectFormalConstructorCallPairBinding,
    missing_source_edge: Option<TreeId<Untyped>>,
}

impl SourceTypedBindings for DirectFormalBindings<'_> {
    fn typed_at(&self, source: SourceId, tree: TreeId<Untyped>) -> Option<TreeId<Typed>> {
        if self.missing_source_edge == Some(tree) {
            None
        } else {
            self.base.typed_at(source, tree)
        }
    }

    fn planned_class_symbols(&self) -> &[SymbolId] {
        self.base.planned_class_symbols()
    }

    fn planned_method_symbols(&self) -> &[SymbolId] {
        self.base.planned_method_symbols()
    }

    fn direct_formal_constructor_call_pair(
        &self,
        source: SourceId,
        method: SymbolId,
    ) -> Option<DirectFormalConstructorCallPairBinding> {
        (source == self.source && method == self.method).then_some(self.receipt)
    }

    fn nested_constructor_call(
        &self,
        source: SourceId,
        method: SymbolId,
    ) -> Option<dotty_tasty_pickler::NestedConstructorCallBinding> {
        self.base.nested_constructor_call(source, method)
    }

    fn nested_constructor_call_pair(
        &self,
        source: SourceId,
        method: SymbolId,
    ) -> Option<dotty_tasty_pickler::NestedConstructorCallPairBinding> {
        self.base.nested_constructor_call_pair(source, method)
    }

    fn planned_call(&self, source: SourceId, method: SymbolId) -> Option<PlannedCallBinding> {
        self.base.planned_call(source, method)
    }

    fn local_value_bindings(&self, source: SourceId, method: SymbolId) -> Vec<LocalValueBinding> {
        self.base.local_value_bindings(source, method)
    }
}

fn validate_with_receipt(
    plan: &ValidatedClassStatePlan<'_, '_>,
    source: SourceId,
    method: SymbolId,
    receipt: DirectFormalConstructorCallPairBinding,
    missing_source_edge: Option<TreeId<Untyped>>,
) -> Result<(), PickleError> {
    validate_with_typed_arena(plan, source, method, receipt, missing_source_edge, None)
}

fn validate_with_typed_arena(
    plan: &ValidatedClassStatePlan<'_, '_>,
    source: SourceId,
    method: SymbolId,
    receipt: DirectFormalConstructorCallPairBinding,
    missing_source_edge: Option<TreeId<Untyped>>,
    typed_ast: Option<&AstArena<Typed>>,
) -> Result<(), PickleError> {
    let original = plan.plan();
    let bindings = DirectFormalBindings {
        base: original.typed_bindings,
        source,
        method,
        receipt,
        missing_source_edge,
    };
    let plan = ClassStatePicklePlan {
        store: original.store,
        definitions: original.definitions,
        source_ast: original.source_ast,
        typed_ast: typed_ast.unwrap_or(original.typed_ast),
        source_index: original.source_index,
        typed_bindings: &bindings,
        source: original.source.clone(),
        class: original.class,
        class_tree: original.class_tree,
        constructor: original.constructor.clone(),
        members: original.members,
    };
    let validated = plan.validate()?;
    validated.validate_method_bodies()?;
    validated.pickle().map(|_| ())
}

#[test]
fn direct_formal_pair_requires_the_exact_ordered_receipt_and_source_edges() {
    let source_id = SourceId::from_index(19642);
    let mut callback_ran = false;
    with_paired_jvm_tasty_batch_plan(SOURCE, "PairFactory.scala", source_id, |batch| {
        callback_ran = true;
        let proof = batch
            .constructor_call_proofs
            .first()
            .expect("the original method has one direct-formal constructor proof");
        let class = batch
            .classes
            .iter()
            .find(|class| class.tasty_plan.plan().class == proof.caller_class())
            .expect("the caller has a TASTy plan");
        let source = batch.source_id;
        let method = proof.method();
        let receipt = class
            .tasty_plan
            .plan()
            .typed_bindings
            .direct_formal_constructor_call_pair(source, method)
            .expect("the batch adapter carries the validated original proof");
        assert!(validate_with_receipt(class.tasty_plan, source, method, receipt, None).is_ok());

        let mut swapped_formals = receipt;
        swapped_formals.argument_formals.swap(0, 1);
        assert!(matches!(
            validate_with_receipt(class.tasty_plan, source, method, swapped_formals, None),
            Err(PickleError::Unsupported { .. }) | Err(PickleError::Reference { .. })
        ));

        let mut swapped_typed_roots = receipt;
        swapped_typed_roots.typed_arguments.swap(0, 1);
        assert!(matches!(
            validate_with_receipt(class.tasty_plan, source, method, swapped_typed_roots, None),
            Err(PickleError::Unsupported { .. })
        ));

        let mut typed_ast = AstArena::new();
        for (_, tree) in class.tasty_plan.plan().typed_ast.iter() {
            typed_ast.alloc(tree.clone());
        }
        let first_argument_type = typed_ast.get(receipt.typed_arguments[0]).ty;
        typed_ast.get_mut(receipt.typed_arguments[1]).ty = first_argument_type;
        assert!(matches!(
            validate_with_typed_arena(
                class.tasty_plan,
                source,
                method,
                receipt,
                None,
                Some(&typed_ast)
            ),
            Err(PickleError::Unsupported { .. })
        ));

        assert!(matches!(
            validate_with_receipt(
                class.tasty_plan,
                source,
                method,
                receipt,
                Some(receipt.source_arguments[1]),
            ),
            Err(PickleError::BodyBinding { .. })
        ));
    })
    .expect("the genuine paired source plan reaches the receipt checks");
    assert!(callback_ran);
}
