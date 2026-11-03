use dotty::jvm_tasty_cross_class::ConstructorArgumentProofKind;
use dotty::jvm_tasty_plan::with_paired_jvm_tasty_batch_plan;
use dotty_core::ast::{Typed, Untyped};
use dotty_core::{SourceId, SymbolId, TreeId};
use dotty_tasty::ast::{APPLY_TAG, DEFDEF_TAG, PARAM_TAG, SELECTIN_TAG, TERMREFDIRECT_TAG};
use dotty_tasty::file::TastyFile;
use dotty_tasty::tasty::{RenderedNameSignature, RenderedParamSig, RenderedSignedName};
use dotty_tasty::term::{AstRefKind, RawTree, TermValue};
use dotty_tasty_pickler::{
    ClassStatePicklePlan, DirectFormalConstructorCallPairBinding, LocalValueBinding,
    OrderedConstructorCallArgumentPairBinding, PlannedCallBinding, SourceTypedBindings,
};

const FORWARD: &str = include_str!(
    "fixtures/jvm_tasty_mixed_origin_constructor_arguments/wrap-and-create/PairFactory.scala"
);
const REVERSE: &str = include_str!(
    "fixtures/jvm_tasty_mixed_origin_constructor_arguments/create-and-wrap/PairFactory.scala"
);

fn emitted(source: &str, source_id: u32) -> Vec<(String, Vec<u8>)> {
    with_paired_jvm_tasty_batch_plan(
        source,
        "PairFactory.scala",
        SourceId::from_index(source_id),
        |batch| {
            batch
                .classes
                .iter()
                .map(|class| {
                    (
                        class.tasty_path.clone(),
                        class.tasty_plan.pickle().unwrap().bytes().to_vec(),
                    )
                })
                .collect()
        },
    )
    .unwrap()
}

fn assert_frontend_proof(source: &str, source_id: u32, expected: [&str; 2]) {
    with_paired_jvm_tasty_batch_plan(
        source,
        "PairFactory.scala",
        SourceId::from_index(source_id),
        |batch| {
            assert_eq!(batch.source_id, SourceId::from_index(source_id));
            assert_eq!(batch.classes.len(), 3, "one frontend unit owns all declarations");
            assert_eq!(batch.jvm_unit.classes.len(), 3);
            assert_eq!(batch.constructor_call_proofs.len(), 1);

            let proof = &batch.constructor_call_proofs[0];
            let (store, source_ast, typed_ast, source_index, _source_typed_index, jvm_unit) =
                proof.session_inputs();
            assert_eq!(proof.source_id(), batch.source_id);
            assert!(std::ptr::eq(batch.jvm_unit, jvm_unit));
            assert_eq!(proof.method_descriptor(), "(Lexample/Counter;I)Lexample/Pair;");
            assert_eq!(
                proof.constructor_descriptor(),
                "(Lexample/Counter;Lexample/Counter;)V"
            );
            assert_eq!(proof.argument_proof().kind(), ConstructorArgumentProofKind::FormalNestedPair);
            let pair = proof.argument_proof().ordered_pair().unwrap();
            let arguments = pair.arguments();
            let labels = arguments.map(|argument| {
                if argument.formal().is_some() {
                    "formal"
                } else {
                    assert!(argument.nested_new().is_some());
                    "nested"
                }
            });
            assert_eq!(labels, expected);

            let formal_index = expected.iter().position(|origin| *origin == "formal").unwrap();
            let nested_index = expected.iter().position(|origin| *origin == "nested").unwrap();
            let formal = arguments[formal_index].formal().unwrap();
            let nested = arguments[nested_index].nested_new().unwrap();
            assert_eq!(formal.formal(), pair.method_formals()[0]);
            assert_eq!(nested.argument_formal(), pair.method_formals()[1]);
            assert_eq!(nested.class(), proof.formal_class().unwrap());
            assert_eq!(nested.source_outer_argument(), arguments[nested_index].source_argument());
            assert_eq!(nested.typed_outer_argument(), arguments[nested_index].typed_argument());
            assert_ne!(arguments[0].source_argument(), arguments[1].source_argument());
            assert_ne!(arguments[0].typed_argument(), arguments[1].typed_argument());
            assert_ne!(pair.constructor_formals()[0], pair.constructor_formals()[1]);

            let nested_roots = (
                nested.source_outer_argument(),
                nested.typed_outer_argument(),
                nested.source_apply(),
                nested.typed_apply(),
                nested.source_select(),
                nested.typed_select(),
                nested.source_new(),
                nested.typed_new(),
                nested.source_argument(),
                nested.typed_argument(),
            );
            eprintln!(
                "MIXED_FRONTEND_SESSION source={:?} classes={:?} members={:?} caller={:?} method={:?} method_descriptor={} method_formals={:?}/{:?} formal_types={:?} target={:?} constructor={:?} constructor_descriptor={} constructor_formals={:?}/{:?} rhs={:?} select={:?} new={:?} arguments={:?} nested={:?} nested_roots={:?} origins={:?}",
                batch.source_id,
                batch.classes.iter().map(|class| class.class).collect::<Vec<_>>(),
                batch
                    .classes
                    .iter()
                    .map(|class| (class.class, class.source_member_order.clone()))
                    .collect::<Vec<_>>(),
                proof.caller_class(),
                proof.method(),
                proof.method_descriptor(),
                pair.source_method_formals(),
                pair.method_formals(),
                pair.method_formal_types(),
                proof.target_class(),
                proof.constructor(),
                proof.constructor_descriptor(),
                pair.source_constructor_formals(),
                pair.constructor_formals(),
                (proof.source_rhs(), proof.typed_rhs()),
                (proof.source_select(), proof.typed_select()),
                (proof.source_new(), proof.typed_new()),
                arguments.map(|argument| (argument.source_argument(), argument.typed_argument())),
                (nested.class(), nested.constructor(), nested.argument_formal()),
                nested_roots,
                labels,
            );

            for (index, origin) in expected.into_iter().enumerate() {
                match (origin, arguments[index].formal(), arguments[index].nested_new()) {
                    ("formal", Some(formal), None) => assert_eq!(
                        formal.formal(),
                        pair.method_formals()[0],
                        "the direct Counter leaf is the original Counter method formal"
                    ),
                    ("nested", None, Some(nested)) => {
                        assert_eq!(nested.argument_formal(), pair.method_formals()[1]);
                        assert_eq!(nested.source_outer_argument(), arguments[index].source_argument());
                        assert_eq!(nested.typed_outer_argument(), arguments[index].typed_argument());
                    }
                    other => panic!("unexpected ordered argument proof: {other:?}"),
                }
            }

            for class in batch.classes {
                let plan = class.tasty_plan.plan();
                assert_eq!(plan.source.id, batch.source_id);
                assert!(std::ptr::eq(plan.store, store));
                assert!(std::ptr::eq(plan.source_ast, source_ast));
                assert!(std::ptr::eq(plan.typed_ast, typed_ast));
                assert!(std::ptr::eq(plan.source_index, source_index));
                for (source_formal, method_formal) in pair
                    .source_method_formals()
                    .into_iter()
                    .zip(pair.method_formals())
                {
                    assert_eq!(
                        plan.source_index.symbol_at(batch.source_id, source_formal),
                        Some(method_formal),
                        "all class plans borrow the source index for the same method formals"
                    );
                }
                for argument in arguments {
                    assert_eq!(
                        plan.typed_bindings.typed_at(batch.source_id, argument.source_argument()),
                        Some(argument.typed_argument()),
                        "original source argument roots map to their exact typed roots"
                    );
                }
            }
        },
    )
    .unwrap();
}

fn tasty(bytes: &[u8]) -> TastyFile<'_> {
    TastyFile::parse_and_validate_scala_3_9(bytes).expect("Rust TASTy 28.9 decodes")
}

fn method_body<'a>(
    tasty: &'a TastyFile<'_>,
    name: &str,
) -> (u32, dotty_tasty::ast::DefDefBody<'a>) {
    let index = tasty.ast_address_index().unwrap();
    index
        .iter_nodes_with_tag(DEFDEF_TAG)
        .find_map(|node| {
            let body = index
                .get(node.offset as u32)
                .unwrap()
                .decode_defdef_body()
                .unwrap();
            (tasty.names().render(body.name).unwrap() == name).then_some((node.offset as u32, body))
        })
        .unwrap_or_else(|| panic!("TASTy contains method {name}"))
}

fn parameter_addresses(index: &dotty_tasty::ast::AstAddressIndex<'_>, method: u32) -> Vec<u32> {
    index
        .iter_nodes_with_tag(PARAM_TAG)
        .filter(|candidate| {
            std::iter::successors(Some(*candidate), |node| index.parent_of(node.offset as u32))
                .any(|parent| parent.offset as u32 == method)
        })
        .map(|node| node.offset as u32)
        .collect()
}

fn signed_constructor(
    tasty: &TastyFile<'_>,
    select: &RawTree<'_>,
    parameters: &[&str],
    result: &str,
) {
    let RawTree::LengthNode(select) = select else {
        panic!("constructor function is SELECTin")
    };
    assert_eq!(select.tag, SELECTIN_TAG);
    let name = select.decode_select_in().unwrap().name;
    assert_eq!(
        tasty.names().render_signed_name(name).unwrap().unwrap(),
        RenderedSignedName::Signed {
            original: "<init>".to_owned(),
            signature: RenderedNameSignature {
                parameters: parameters
                    .iter()
                    .map(|parameter| RenderedParamSig::TermParameter((*parameter).to_owned()))
                    .collect::<Vec<_>>(),
                result: result.to_owned(),
            },
        }
    );
}

fn assert_mixed_wire(source: &str, source_id: u32, method: &str, expected: [&str; 2]) {
    let first = emitted(source, source_id);
    let second = emitted(source, source_id + 1);
    assert_eq!(
        first, second,
        "fresh source sessions emit deterministic bytes"
    );
    assert_eq!(
        first
            .iter()
            .map(|(path, _)| path.as_str())
            .collect::<Vec<_>>(),
        [
            "example/Counter.tasty",
            "example/Pair.tasty",
            "example/PairFactory.tasty"
        ]
    );
    let (_, bytes) = first
        .iter()
        .find(|(path, _)| path == "example/PairFactory.tasty")
        .unwrap();
    let tasty = tasty(bytes);
    let (method_address, body) = method_body(&tasty, method);
    assert_eq!(
        body.parameters
            .iter()
            .map(|parameter| tasty.names().render(parameter.name()).unwrap())
            .collect::<Vec<_>>(),
        ["existing", "seed"]
    );
    assert_eq!(body.parameters.len(), 2);
    assert_eq!(
        body.parameters
            .iter()
            .map(|parameter| {
                parameter
                    .decode_body()
                    .unwrap()
                    .type_tree
                    .name_refs()
                    .into_iter()
                    .map(|name| tasty.names().render(name).unwrap())
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>(),
        [vec!["Counter", "example"], vec!["Int", "scala"]]
    );
    assert_eq!(
        body.return_type
            .name_refs()
            .into_iter()
            .map(|name| tasty.names().render(name).unwrap())
            .collect::<Vec<_>>(),
        ["Pair", "example"]
    );
    let index = tasty.ast_address_index().unwrap();
    let parameter_addresses = parameter_addresses(&index, method_address);
    assert_eq!(parameter_addresses.len(), 2);
    eprintln!(
        "MIXED_TASTY_REFERENCES method={method} PARAM_existing={} PARAM_seed={} expected_argument_targets={:?}",
        parameter_addresses[0], parameter_addresses[1], expected
    );
    let RawTree::LengthNode(outer_node) = body.rhs.unwrap() else {
        panic!("method body is the original Pair constructor APPLY")
    };
    assert_eq!(outer_node.tag, APPLY_TAG);
    let outer = outer_node.decode_apply().unwrap();
    assert_eq!(outer.arguments.len(), 2);
    signed_constructor(
        &tasty,
        &outer.function,
        &["example.Counter", "example.Counter"],
        "example.Pair",
    );

    let mut observed = Vec::new();
    let mut term_ref_count = 0;
    for argument in &outer.arguments {
        match argument {
            RawTree::Leaf(term) if term.tag == TERMREFDIRECT_TAG => {
                term_ref_count += 1;
                let TermValue::AstRef(address) = &term.value else {
                    panic!("direct argument is a TERMREFdirect address")
                };
                observed.push("formal");
                assert_eq!(
                    argument.ast_refs()[0].kind,
                    AstRefKind::TermRefDirect,
                    "direct Counter argument keeps the canonical formal reference"
                );
                assert_eq!(*address, parameter_addresses[0]);
            }
            RawTree::LengthNode(inner_node) if inner_node.tag == APPLY_TAG => {
                let inner = inner_node.decode_apply().unwrap();
                assert_eq!(inner.arguments.len(), 1);
                signed_constructor(&tasty, &inner.function, &["scala.Int"], "example.Counter");
                let RawTree::Leaf(seed) = &inner.arguments[0] else {
                    panic!("nested Counter seed is the original method formal")
                };
                assert_eq!(seed.tag, TERMREFDIRECT_TAG);
                let TermValue::AstRef(seed_address) = &seed.value else {
                    panic!("seed TERMREFdirect points to a canonical PARAM address")
                };
                assert_eq!(*seed_address, parameter_addresses[1]);
                assert_eq!(
                    inner.arguments[0].ast_refs()[0].kind,
                    AstRefKind::TermRefDirect
                );
                term_ref_count += 1;
                observed.push("nested");
            }
            other => panic!("expected one direct and one nested argument: {other:?}"),
        }
    }
    assert_eq!(observed, expected);
    assert_eq!(
        term_ref_count, 2,
        "one direct reference plus one seed reference"
    );
}

struct ChangedMixedBindings<'a> {
    base: &'a dyn SourceTypedBindings,
    source: SourceId,
    method: SymbolId,
    pair: OrderedConstructorCallArgumentPairBinding,
    missing_source_edge: Option<TreeId<Untyped>>,
}

impl SourceTypedBindings for ChangedMixedBindings<'_> {
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

    fn direct_formal_constructor_call_pair(
        &self,
        source: SourceId,
        method: SymbolId,
    ) -> Option<DirectFormalConstructorCallPairBinding> {
        self.base
            .direct_formal_constructor_call_pair(source, method)
    }

    fn ordered_constructor_call_argument_pair(
        &self,
        source: SourceId,
        method: SymbolId,
    ) -> Option<OrderedConstructorCallArgumentPairBinding> {
        (source == self.source && method == self.method)
            .then_some(self.pair)
            .or_else(|| {
                self.base
                    .ordered_constructor_call_argument_pair(source, method)
            })
    }

    fn planned_call(&self, source: SourceId, method: SymbolId) -> Option<PlannedCallBinding> {
        self.base.planned_call(source, method)
    }

    fn local_value_bindings(&self, source: SourceId, method: SymbolId) -> Vec<LocalValueBinding> {
        self.base.local_value_bindings(source, method)
    }
}

fn validate_changed_pair(
    mutate: impl FnOnce(&mut OrderedConstructorCallArgumentPairBinding),
) -> Result<(), String> {
    validate_changed_pair_with_missing_edge(mutate, None)
}

fn validate_changed_pair_with_missing_edge(
    mutate: impl FnOnce(&mut OrderedConstructorCallArgumentPairBinding),
    missing_source_edge: Option<TreeId<Untyped>>,
) -> Result<(), String> {
    let source_id = SourceId::from_index(201409);
    with_paired_jvm_tasty_batch_plan(FORWARD, "PairFactory.scala", source_id, |batch| {
        let proof = batch.constructor_call_proofs.first().unwrap();
        let factory = batch
            .classes
            .iter()
            .find(|class| class.tasty_plan.plan().class == proof.caller_class())
            .unwrap();
        let method = proof.method();
        let source = batch.source_id;
        let original = factory.tasty_plan.plan();
        let mut pair = original
            .typed_bindings
            .ordered_constructor_call_argument_pair(source, method)
            .unwrap();
        mutate(&mut pair);
        let changed = ChangedMixedBindings {
            base: original.typed_bindings,
            source,
            method,
            pair,
            missing_source_edge,
        };
        let changed_plan = ClassStatePicklePlan {
            store: original.store,
            definitions: original.definitions,
            source_ast: original.source_ast,
            typed_ast: original.typed_ast,
            source_index: original.source_index,
            typed_bindings: &changed,
            source: original.source.clone(),
            class: original.class,
            class_tree: original.class_tree,
            constructor: original.constructor.clone(),
            members: original.members,
        };
        let validated = changed_plan
            .validate()
            .map_err(|error| format!("TASTy plan validation: {error:?}"))?;
        validated
            .validate_method_bodies()
            .map_err(|error| format!("typed body validation: {error:?}"))?;
        validated
            .pickle()
            .map(|_| ())
            .map_err(|error| format!("TASTy pickler: {error:?}"))
    })
    .unwrap()
}

#[test]
fn mixed_constructor_arguments_serialize_in_both_original_positions() {
    assert_frontend_proof(FORWARD, 201400, ["formal", "nested"]);
    assert_frontend_proof(REVERSE, 201402, ["nested", "formal"]);
    assert_mixed_wire(FORWARD, 201401, "wrapAndCreate", ["formal", "nested"]);
    assert_mixed_wire(REVERSE, 201403, "createAndWrap", ["nested", "formal"]);
}

#[test]
fn mixed_constructor_receipt_rejects_swapped_and_forged_provenance() {
    use dotty_tasty_pickler::OrderedConstructorCallArgumentOrigin as Origin;

    let swapped = validate_changed_pair(|pair| pair.arguments.swap(0, 1));
    assert!(swapped.is_err(), "position-swapped roots must be rejected");
    eprintln!("MIXED_NEGATIVE_FIRST_REJECTION swapped-roots {swapped:?}");

    let wrong_constructor = validate_changed_pair(|pair| {
        let Origin::NestedNew { call, .. } = pair.arguments[1].origin else {
            panic!("forward argument 1 is NestedNew")
        };
        pair.arguments[1].origin = Origin::NestedNew {
            call: dotty_tasty_pickler::NestedConstructorCallBinding {
                constructor: pair.method_formals[0],
                ..call
            },
            argument_formal: pair.method_formals[1],
        };
    });
    assert!(
        wrong_constructor.is_err(),
        "wrong constructor target must fail"
    );
    eprintln!("MIXED_NEGATIVE_FIRST_REJECTION wrong-nested-constructor {wrong_constructor:?}");

    let seed_used_as_direct_counter = validate_changed_pair(|pair| {
        let Origin::Formal {
            formal_type,
            result_type,
            ..
        } = pair.arguments[0].origin
        else {
            panic!("forward argument 0 is Formal")
        };
        pair.arguments[0].origin = Origin::Formal {
            formal: pair.method_formals[1],
            formal_type,
            result_type,
        };
    });
    assert!(
        seed_used_as_direct_counter.is_err(),
        "direct Counter origin cannot target the Int seed formal"
    );
    eprintln!(
        "MIXED_NEGATIVE_FIRST_REJECTION direct-formal-target {seed_used_as_direct_counter:?}"
    );

    let existing_used_as_nested_seed = validate_changed_pair(|pair| {
        let Origin::NestedNew { call, .. } = pair.arguments[1].origin else {
            panic!("forward argument 1 is NestedNew")
        };
        pair.arguments[1].origin = Origin::NestedNew {
            call,
            argument_formal: pair.method_formals[0],
        };
    });
    assert!(
        existing_used_as_nested_seed.is_err(),
        "nested seed cannot target the existing Counter formal"
    );
    eprintln!("MIXED_NEGATIVE_FIRST_REJECTION nested-seed-target {existing_used_as_nested_seed:?}");

    let duplicate_root = validate_changed_pair(|pair| {
        pair.arguments[1].source_argument = pair.arguments[0].source_argument;
        pair.arguments[1].typed_argument = pair.arguments[0].typed_argument;
    });
    assert!(
        duplicate_root.is_err(),
        "duplicate source/typed roots must fail"
    );
    eprintln!("MIXED_NEGATIVE_FIRST_REJECTION duplicate-source-typed-roots {duplicate_root:?}");

    let foreign_formal = validate_changed_pair(|pair| {
        let Origin::Formal {
            formal_type,
            result_type,
            ..
        } = pair.arguments[0].origin
        else {
            panic!("forward argument 0 is Formal")
        };
        let Origin::NestedNew { call, .. } = pair.arguments[1].origin else {
            panic!("forward argument 1 is NestedNew")
        };
        pair.arguments[0].origin = Origin::Formal {
            formal: call.constructor,
            formal_type,
            result_type,
        };
    });
    assert!(
        foreign_formal.is_err(),
        "a constructor symbol cannot forge a caller formal"
    );
    eprintln!("MIXED_NEGATIVE_FIRST_REJECTION foreign-formal {foreign_formal:?}");

    let missing_source_edge = with_paired_jvm_tasty_batch_plan(
        FORWARD,
        "PairFactory.scala",
        SourceId::from_index(201409),
        |batch| {
            batch
                .constructor_call_proofs
                .first()
                .unwrap()
                .argument_proof()
                .ordered_pair()
                .unwrap()
                .arguments()[0]
                .source_argument()
        },
    )
    .unwrap();
    let missing_edge = validate_changed_pair_with_missing_edge(|_| {}, Some(missing_source_edge));
    assert!(
        missing_edge.is_err(),
        "missing source-to-typed edges must fail"
    );
    eprintln!("MIXED_NEGATIVE_FIRST_REJECTION missing-source-typed-edge {missing_edge:?}");
}

#[test]
fn mixed_source_rejects_third_argument_deeper_construction_and_wrong_result() {
    let third = FORWARD.replace(
        "new Pair(existing, new Counter(seed))",
        "new Pair(existing, new Counter(seed), existing)",
    );
    let deeper = FORWARD.replace(
        "new Pair(existing, new Counter(seed))",
        "new Pair(existing, new Counter(new Counter(seed).get()))",
    );
    let wrong_result = FORWARD.replace(
        "def wrapAndCreate(existing: Counter, seed: Int): Pair =",
        "def wrapAndCreate(existing: Counter, seed: Int): Counter =",
    );
    let wrong_seed_type = FORWARD.replace(
        "new Pair(existing, new Counter(seed))",
        "new Pair(existing, new Counter(existing))",
    );
    let helper_result = FORWARD
        .replace(
            "class PairFactory:",
            "class PairFactory:\n  def make(seed: Int): Counter = new Counter(seed)",
        )
        .replace(
            "new Pair(existing, new Counter(seed))",
            "new Pair(existing, make(seed))",
        );
    let local_argument = FORWARD.replace(
        "    new Pair(existing, new Counter(seed))",
        "    val candidate: Counter = new Counter(seed)\n    new Pair(existing, candidate)",
    );
    let branch_argument = FORWARD.replace(
        "new Pair(existing, new Counter(seed))",
        "new Pair(existing, if seed > 0 then new Counter(seed) else existing)",
    );
    let foreign_counter = FORWARD
        .replace(
            "class PairFactory:",
            "class OtherCounter(start: Int)\n\nclass PairFactory:",
        )
        .replace(
            "new Pair(existing, new Counter(seed))",
            "new Pair(existing, new OtherCounter(seed))",
        );
    for (source_id, source) in [
        (201411, third),
        (201412, deeper),
        (201413, wrong_result),
        (201414, wrong_seed_type),
        (201415, helper_result),
        (201418, local_argument),
        (201419, branch_argument),
        (201420, foreign_counter),
    ] {
        let result = with_paired_jvm_tasty_batch_plan(
            &source,
            "PairFactory.scala",
            SourceId::from_index(source_id),
            |_| (),
        );
        eprintln!("MIXED_NEGATIVE_FIRST_REJECTION source_id={source_id} {result:?}");
        assert!(result.is_err(), "unsupported mixed shape cannot be planned");
    }
}

#[test]
fn renamed_factory_keeps_the_proof_structural_and_deterministic() {
    let renamed = FORWARD
        .replace("PairFactory", "AssemblerFactory")
        .replace("wrapAndCreate", "assemble");
    let first = emitted(&renamed, 201416);
    let second = emitted(&renamed, 201417);
    assert_eq!(first, second);
    let (path, bytes) = first
        .iter()
        .find(|(path, _)| path == "example/AssemblerFactory.tasty")
        .unwrap();
    assert_eq!(path, "example/AssemblerFactory.tasty");
    let tasty = tasty(bytes);
    let (_, body) = method_body(&tasty, "assemble");
    assert!(matches!(body.rhs, Some(RawTree::LengthNode(_))));
}
