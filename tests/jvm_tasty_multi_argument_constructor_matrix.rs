use dotty::jvm_tasty_plan::with_paired_jvm_tasty_batch_plan;
use dotty_core::ast::{Typed, Untyped};
use dotty_core::{SourceId, SymbolId, TreeId};
use dotty_tasty_pickler::{
    ClassStatePicklePlan, LocalValueBinding, NestedConstructorCallBinding,
    NestedConstructorCallPairBinding, PickleError, PlannedCallBinding, SourceTypedBindings,
    ValidatedClassStatePlan,
};

const SOURCE: &str =
    include_str!("fixtures/jvm_tasty_multi_argument_constructor_calls/PairFactory.scala");

struct PairReceiptBindings<'a> {
    base: &'a dyn SourceTypedBindings,
    source: SourceId,
    method: SymbolId,
    pair: Option<NestedConstructorCallPairBinding>,
    missing_source_edge: Option<TreeId<Untyped>>,
}

impl SourceTypedBindings for PairReceiptBindings<'_> {
    fn typed_at(&self, source: SourceId, tree: TreeId<Untyped>) -> Option<TreeId<Typed>> {
        if self.missing_source_edge == Some(tree) {
            return None;
        }
        self.base.typed_at(source, tree)
    }

    fn planned_class_symbols(&self) -> &[SymbolId] {
        self.base.planned_class_symbols()
    }

    fn planned_method_symbols(&self) -> &[SymbolId] {
        self.base.planned_method_symbols()
    }

    fn nested_constructor_call(
        &self,
        source: SourceId,
        method: SymbolId,
    ) -> Option<NestedConstructorCallBinding> {
        self.base.nested_constructor_call(source, method)
    }

    fn nested_constructor_call_pair(
        &self,
        source: SourceId,
        method: SymbolId,
    ) -> Option<NestedConstructorCallPairBinding> {
        (source == self.source && method == self.method)
            .then_some(self.pair)
            .flatten()
    }

    fn planned_call(&self, source: SourceId, method: SymbolId) -> Option<PlannedCallBinding> {
        self.base.planned_call(source, method)
    }

    fn local_value_bindings(&self, source: SourceId, method: SymbolId) -> Vec<LocalValueBinding> {
        self.base.local_value_bindings(source, method)
    }
}

fn validate_with_pair(
    plan: &ValidatedClassStatePlan<'_, '_>,
    source: SourceId,
    method: SymbolId,
    pair: Option<NestedConstructorCallPairBinding>,
    missing_source_edge: Option<TreeId<Untyped>>,
) -> Result<(), PickleError> {
    let original = plan.plan();
    let bindings = PairReceiptBindings {
        base: original.typed_bindings,
        source,
        method,
        pair,
        missing_source_edge,
    };
    ClassStatePicklePlan {
        store: original.store,
        definitions: original.definitions,
        source_ast: original.source_ast,
        typed_ast: original.typed_ast,
        source_index: original.source_index,
        typed_bindings: &bindings,
        source: original.source.clone(),
        class: original.class,
        class_tree: original.class_tree,
        constructor: original.constructor.clone(),
        members: original.members,
    }
    .validate()?
    .validate_method_bodies()
}

#[test]
fn multi_argument_factory_variation_matrix_records_first_paired_result() {
    let source = SOURCE;
    let counter_start = source.find("class Counter(start").unwrap();
    let pair_start = source.find("class Pair(first").unwrap();
    let factory_start = source.find("class PairFactory:").unwrap();
    let counter = &source[counter_start..pair_start];
    let pair = &source[pair_start..factory_start];
    let factory = &source[factory_start..];
    let reordered = format!("package example\n\n{factory}\n{pair}\n{counter}");
    let renamed = source
        .replace("package example", "package renamed.example")
        .replace("class PairFactory:", "class Assembler:")
        .replace("def create(", "def assemble(")
        .replace("PairFactory", "Assembler")
        .replace("class Counter", "class Token")
        .replace("Counter", "Token")
        .replace("class Pair(", "class Envelope(")
        .replace("): Pair =", "): Envelope =")
        .replace("new Pair(", "new Envelope(");
    let accepted_controls = [
        ("renamed package-level identities", renamed),
        ("reordered declarations", reordered),
        (
            "swapped formal order",
            source.replace(
                "create(first: Int, second: Int)",
                "create(second: Int, first: Int)",
            ),
        ),
        (
            "swapped nested expressions",
            source.replace(
                "new Counter(first), new Counter(second)",
                "new Counter(second), new Counter(first)",
            ),
        ),
        (
            "legitimate repeated caller formal",
            source.replace(
                "new Counter(first), new Counter(second)",
                "new Counter(first), new Counter(first)",
            ),
        ),
    ];
    for (ordinal, (name, variant)) in accepted_controls.into_iter().enumerate() {
        let mut callback_ran = false;
        with_paired_jvm_tasty_batch_plan(
            &variant,
            "PairFactory.scala",
            SourceId::from_index(19130 + ordinal as u32),
            |batch| {
                callback_ran = true;
                assert_eq!(batch.constructor_call_proofs.len(), 1, "{name}");
                let proof = &batch.constructor_call_proofs[0];
                let pair = proof
                    .nested_argument_pair()
                    .expect("the source proof retains both ordered nested calls");
                assert_ne!(pair[0].source_apply(), pair[1].source_apply(), "{name}");
                assert_ne!(pair[0].typed_apply(), pair[1].typed_apply(), "{name}");
                let caller = batch
                    .classes
                    .iter()
                    .find(|class| class.class == proof.caller_class())
                    .expect("the proof's caller has a TASTy plan");
                caller
                    .tasty_plan
                    .validate_method_bodies()
                    .unwrap_or_else(|error| {
                        panic!("ordered pair body validates for {name}: {error:?}")
                    });
                let authentic = caller
                    .tasty_plan
                    .plan()
                    .typed_bindings
                    .nested_constructor_call_pair(batch.source_id, proof.method())
                    .expect("the batch adapter preserves both immutable proof receipts");
                let mut duplicate = authentic;
                duplicate.calls[1] = duplicate.calls[0];
                let mut wrong_constructor = authentic;
                wrong_constructor.calls[1].constructor = proof.constructor();
                let mut wrong_class = authentic;
                wrong_class.calls[1].class = proof.caller_class();
                let mut stale_leaf = authentic;
                stale_leaf.calls[1].typed_argument = stale_leaf.calls[0].typed_argument;
                let mut forged_receipts = vec![
                    ("missing pair receipt", None),
                    ("duplicate second callsite", Some(duplicate)),
                    (
                        "reversed callsite order",
                        Some(NestedConstructorCallPairBinding {
                            calls: [authentic.calls[1], authentic.calls[0]],
                            argument_formals: [
                                authentic.argument_formals[1],
                                authentic.argument_formals[0],
                            ],
                        }),
                    ),
                    ("wrong nested constructor", Some(wrong_constructor)),
                    ("wrong nested class", Some(wrong_class)),
                    ("stale second leaf", Some(stale_leaf)),
                ];
                if authentic.argument_formals[0] != authentic.argument_formals[1] {
                    let mut wrong_caller_formal = authentic;
                    wrong_caller_formal.argument_formals[1] =
                        wrong_caller_formal.argument_formals[0];
                    forged_receipts.push(("wrong caller formal", Some(wrong_caller_formal)));
                }
                for (case, receipts) in forged_receipts {
                    assert!(
                        validate_with_pair(
                            caller.tasty_plan,
                            batch.source_id,
                            proof.method(),
                            receipts,
                            None,
                        )
                        .is_err(),
                        "forged receipt must fail closed: {case} in {name}"
                    );
                }
                assert!(
                    validate_with_pair(
                        caller.tasty_plan,
                        batch.source_id,
                        proof.method(),
                        Some(authentic),
                        Some(authentic.calls[1].source_argument),
                    )
                    .is_err(),
                    "a missing second source/typed index edge must fail closed"
                );
            },
        )
        .unwrap_or_else(|error| panic!("ordered pair plans for {name}: {error:?}"));
        assert!(
            callback_ran,
            "accepted source reaches the producer callback: {name}"
        );
    }

    let rejected_controls = [
        (
            "unrelated method call argument",
            source
                .replace(
                    "class PairFactory:\n  def create",
                    "class PairFactory:\n  def forward(value: Int): Int = value\n  def create",
                )
                .replace(
                    "new Counter(first), new Counter(second)",
                    "new Counter(forward(first)), new Counter(second)",
                ),
            "nested Counter leaf must be an original factory formal",
        ),
        (
            "third nested allocation",
            source.replace(
                "new Counter(first), new Counter(second)",
                "new Counter(new Counter(first).get()), new Counter(second)",
            ),
            "nested Counter leaf must be an original factory formal",
        ),
        (
            "branch argument",
            source.replace(
                "new Counter(first), new Counter(second)",
                "new Counter(if first < 0 then second else first), new Counter(second)",
            ),
            "nested Counter leaf must be an original factory formal",
        ),
        (
            "defaulted factory formal",
            source.replace("create(first: Int, second: Int)", "create(first: Int = 0, second: Int)"),
            "factory formals cannot have default values",
        ),
        (
            "generic factory method",
            source.replace("def create(", "def create[A]("),
            "generic methods are unsupported",
        ),
        (
            "overloaded factory method",
            source.replace(
                "    new Pair(new Counter(first), new Counter(second))",
                "    new Pair(new Counter(first), new Counter(second))\n  def create(seed: String, other: Int): Pair =\n    new Pair(new Counter(0), new Counter(1))",
            ),
            "TypeNameNotFound",
        ),
        (
            "default constructor",
            source.replace("second: Counter):", "second: Counter = new Counter(0)):"),
            "InvalidPrimaryConstructor",
        ),
        (
            "generic constructor",
            source
                .replace("class Pair(first: Counter", "class Pair[A](first: Counter")
                .replace("): Pair =", "): Pair[Any] ="),
            "generic primary constructors are unsupported",
        ),
        (
            "secondary constructor",
            source.replace(
                "  def leftValue()",
                "  def this(first: Counter) = this(first, new Counter(0))\n  def leftValue()",
            ),
            "method is not owned by its declaring class",
        ),
        (
            "contextual constructor",
            source
                .replace(
                    "class Pair(first: Counter, second: Counter):",
                    "class Pair(first: Counter)(using second: Counter):",
                )
                .replace(
                    "new Pair(new Counter(first), new Counter(second))",
                    "new Pair(new Counter(first))(using new Counter(second))",
                ),
            "InvalidPrimaryConstructor",
        ),
    ];
    for (ordinal, (name, variant, expected)) in rejected_controls.into_iter().enumerate() {
        let mut callback_ran = false;
        let result = with_paired_jvm_tasty_batch_plan(
            &variant,
            "PairFactory.scala",
            SourceId::from_index(19130 + ordinal as u32),
            |_| callback_ran = true,
        );
        let Err(error) = result else {
            panic!(
                "Scala-valid but unsupported control unexpectedly passed paired planning: {name}"
            );
        };
        assert!(
            !callback_ran,
            "pair rejection must precede producer callbacks: {name}"
        );
        assert!(format!("{error:?}").contains(expected), "{name}: {error:?}");
    }

    let invalid_controls = [
        (
            "wrong Pair arity",
            source.replace(
                "new Pair(new Counter(first), new Counter(second))",
                "new Pair(new Counter(first))",
            ),
            "ApplicationArityMismatch",
        ),
        (
            "wrong Counter argument type",
            source.replace("new Counter(first)", "new Counter(\"wrong\")"),
            "StringLiteralTypingDeferred",
        ),
        (
            "duplicate method formal",
            source.replace(
                "create(first: Int, second: Int)",
                "create(first: Int, first: Int)",
            ),
            "AmbiguousTermReference",
        ),
    ];
    for (ordinal, (name, variant, expected)) in invalid_controls.into_iter().enumerate() {
        let error = with_paired_jvm_tasty_batch_plan(
            &variant,
            "PairFactory.scala",
            SourceId::from_index(19140 + ordinal as u32),
            |_| (),
        )
        .expect_err("Scala-invalid source must reject before paired planning");
        assert!(format!("{error:?}").contains(expected), "{name}: {error:?}");
    }
}
