//! Canonical primitive member selections; expression emission remains separate.
// This private reference contract is deliberately introduced one issue before
// the expression emitter that will consume it, and is exercised by unit tests.
#![allow(dead_code)]

use super::*;
use crate::{TypeRole, UnsupportedFeature};

struct OperatorContract {
    owner: Builtin,
    source_name: &'static str,
    parameters: Vec<TypeId>,
    result: TypeId,
    argument_count: Option<usize>,
}

impl<'a> EncodingContext<'a> {
    pub(crate) fn is_registered_int_arithmetic_target(
        &self,
        target: SymbolId,
    ) -> Result<bool, PickleError> {
        let symbols = self
            .definitions
            .int_primitive_operator_symbols(self.store)
            .ok_or(PickleError::BuiltinIdentity {
                builtin: Builtin::Int,
                ty: self.definitions.int,
            })?;
        Ok(symbols.all()[..5].contains(&target))
    }

    /// Projects one registered primitive operator selection.
    ///
    /// `receiver` is the already projected source receiver, and
    /// `receiver_type`, `parameter_types` and `result_type` come from its typed
    /// selection/signature. `argument_count` is `None` for the parameterless
    /// `unary_!` select and `Some(n)` for a typed application. The receiver's
    /// own label is retained exactly in the emitted SELECT/SELECTin node.
    pub(crate) fn primitive_operator_select(
        &mut self,
        target: SymbolId,
        receiver: AstLabel,
        receiver_type: TypeId,
        parameter_types: &[TypeId],
        result_type: TypeId,
        argument_count: Option<usize>,
    ) -> Result<AstLabel, PickleError> {
        self.run(|context| {
            context.charge(1)?;
            let record = context.symbol(target)?;
            let contract = context.operator_contract(target)?;
            let owner_symbol = context.builtin(contract.owner)?;
            if record.kind != SymbolKind::Method {
                return Err(reference(target, ReferenceError::Kind));
            }
            if record.owner != Some(owner_symbol) {
                return Err(reference(target, ReferenceError::Owner));
            }
            if record.origin != SymbolOrigin::Builtin
                || record.visibility != Visibility::Public
                || !record.flags.is_empty()
                || !record.annotations.is_empty()
            {
                return Err(reference(target, ReferenceError::Metadata));
            }
            context.namespace(target, false)?;
            let scope = context.class_scope(owner_symbol)?;
            if !context
                .store
                .scopes
                .get(scope)
                .lookup_all(&record.name)
                .contains(&target)
            {
                return Err(reference(target, ReferenceError::NotRegistered));
            }

            let signature_type = context.complete(target)?;
            context.validate_operator_signature(signature_type, &contract)?;
            if argument_count != contract.argument_count {
                return Err(PickleError::PrimitiveOperatorArity {
                    symbol: target,
                    expected: contract.argument_count,
                    actual: argument_count,
                });
            }
            if parameter_types.len() != contract.parameters.len() {
                return Err(PickleError::PrimitiveOperatorArity {
                    symbol: target,
                    expected: contract.argument_count,
                    actual: Some(parameter_types.len()),
                });
            }
            for (index, (&actual, &expected)) in
                parameter_types.iter().zip(&contract.parameters).enumerate()
            {
                context.ty(actual)?;
                if actual != expected {
                    return Err(PickleError::PrimitiveOperatorParameter {
                        symbol: target,
                        index,
                        expected,
                        actual,
                    });
                }
            }
            context.ty(result_type)?;
            if result_type != contract.result {
                return Err(PickleError::PrimitiveOperatorResult {
                    symbol: target,
                    expected: contract.result,
                    actual: result_type,
                });
            }
            let actual_receiver = context.primitive_value_type(receiver_type)?;
            let expected_receiver = match contract.owner {
                Builtin::Int => context.definitions.int,
                Builtin::Boolean => context.definitions.boolean,
                _ => {
                    return Err(PickleError::BuiltinIdentity {
                        builtin: contract.owner,
                        ty: receiver_type,
                    });
                }
            };
            if actual_receiver != expected_receiver {
                return Err(PickleError::PrimitiveOperatorReceiver {
                    symbol: target,
                    expected: expected_receiver,
                    actual: actual_receiver,
                });
            }

            // TASTy stores semantic operator spellings verbatim, including
            // names such as `/` that are not ordinary identifiers.
            let source_spelling = context.store.names.try_resolve(record.name.text()).ok_or(
                PickleError::InvalidName {
                    name: record.name.text(),
                },
            )?;
            if source_spelling.is_empty()
                || source_spelling.len() > context.limits.max_name_bytes
                || source_spelling.contains('\0')
            {
                return Err(PickleError::Unsupported {
                    feature: UnsupportedFeature::Name,
                    symbol: Some(target),
                });
            }
            if source_spelling != contract.source_name {
                return Err(reference(target, ReferenceError::Metadata));
            }
            let method_name = context.intern_wire_text(contract.source_name)?;
            if contract.argument_count.is_none() {
                return context
                    .wire
                    .push(AstNode::NatAst {
                        tag: SELECT_TAG,
                        value: AstOperand::Nat(method_name),
                        child: receiver,
                    })
                    .map_err(construction);
            }

            let result_name = context.primitive_type_name(contract.result)?;
            let mut parameter_names = Vec::with_capacity(contract.parameters.len());
            for &parameter in &contract.parameters {
                parameter_names.push(
                    i32::try_from(context.primitive_type_name(parameter)?).map_err(|_| {
                        PickleError::LimitExceeded {
                            kind: LimitKind::Names,
                            limit: i32::MAX as usize,
                        }
                    })?,
                );
            }
            let signed = context.intern(NameKey::SignedMany(
                method_name,
                result_name,
                parameter_names,
            ))?;
            let owner_type = match contract.owner {
                Builtin::Int => context.definitions.int,
                Builtin::Boolean => context.definitions.boolean,
                _ => {
                    return Err(PickleError::BuiltinIdentity {
                        builtin: contract.owner,
                        ty: receiver_type,
                    });
                }
            };
            let owner_type = context.lower(owner_type, 0)?;
            context
                .wire
                .push(AstNode::Length {
                    tag: SELECTIN_TAG,
                    fields: vec![
                        AstField::Nat(signed),
                        AstField::Tree(receiver),
                        AstField::Tree(owner_type),
                    ],
                })
                .map_err(construction)
        })
    }

    fn operator_contract(&mut self, target: SymbolId) -> Result<OperatorContract, PickleError> {
        let int_symbols = self
            .definitions
            .int_primitive_operator_symbols(self.store)
            .ok_or(PickleError::BuiltinIdentity {
                builtin: Builtin::Int,
                ty: self.definitions.int,
            })?;
        if let Some(index) = int_symbols
            .all()
            .iter()
            .position(|&symbol| symbol == target)
        {
            let source_name = ["+", "-", "*", "/", "%", "<", "<=", ">", ">="][index];
            let result = if index < 5 {
                self.definitions.int
            } else {
                self.definitions.boolean
            };
            return Ok(OperatorContract {
                owner: Builtin::Int,
                source_name,
                parameters: vec![self.definitions.int],
                result,
                argument_count: Some(1),
            });
        }

        let boolean_symbols = self.definitions.boolean_primitive_operator_ids().ok_or(
            PickleError::BuiltinIdentity {
                builtin: Builtin::Boolean,
                ty: self.definitions.boolean,
            },
        )?;
        if target == boolean_symbols.unary_not {
            return Ok(OperatorContract {
                owner: Builtin::Boolean,
                source_name: "unary_!",
                parameters: Vec::new(),
                result: self.definitions.boolean,
                argument_count: None,
            });
        }
        if target == boolean_symbols.and_then || target == boolean_symbols.or_else {
            return Ok(OperatorContract {
                owner: Builtin::Boolean,
                source_name: if target == boolean_symbols.and_then {
                    "&&"
                } else {
                    "||"
                },
                parameters: vec![self.definitions.boolean],
                result: self.definitions.boolean,
                argument_count: Some(1),
            });
        }
        if boolean_symbols.eager_and == target
            || boolean_symbols.eager_or == target
            || boolean_symbols.xor == target
            || boolean_symbols.equal == target
            || boolean_symbols.not_equal == target
        {
            return Err(PickleError::Unsupported {
                feature: UnsupportedFeature::PrimitiveOperator,
                symbol: Some(target),
            });
        }
        Err(reference(target, ReferenceError::NotRegistered))
    }

    fn validate_operator_signature(
        &self,
        signature_type: TypeId,
        contract: &OperatorContract,
    ) -> Result<(), PickleError> {
        if contract.argument_count.is_none() {
            if signature_type != contract.result {
                return Err(PickleError::MalformedType {
                    ty: signature_type,
                    role: TypeRole::MethodResult,
                });
            }
            return Ok(());
        }
        let Type::Method(signature) = self.ty(signature_type)? else {
            return Err(PickleError::MalformedType {
                ty: signature_type,
                role: TypeRole::Parameter,
            });
        };
        if signature.kind != MethodKind::Plain
            || signature.params.len() != contract.parameters.len()
            || signature.result != contract.result
            || signature
                .params
                .iter()
                .zip(&contract.parameters)
                .any(|(parameter, &expected)| {
                    parameter.ty != expected || parameter.erased || parameter.varargs
                })
        {
            return Err(PickleError::MalformedType {
                ty: signature_type,
                role: TypeRole::Parameter,
            });
        }
        Ok(())
    }

    fn primitive_value_type(&self, ty: TypeId) -> Result<TypeId, PickleError> {
        match self.ty(ty)? {
            Type::TermRef {
                target: TermRefTarget::Symbol(symbol),
                ..
            } => self.complete(*symbol),
            Type::Constant(Constant::Int(_)) => Ok(self.definitions.int),
            Type::Constant(Constant::Boolean(_)) => Ok(self.definitions.boolean),
            _ => Ok(ty),
        }
    }

    fn primitive_type_name(&mut self, ty: TypeId) -> Result<NameRef, PickleError> {
        let builtin = if ty == self.definitions.int {
            Builtin::Int
        } else if ty == self.definitions.boolean {
            Builtin::Boolean
        } else {
            return Err(PickleError::UnsupportedTypeReference { ty });
        };
        let symbol = self.builtin(builtin)?;
        let scala = self.utf8("scala")?;
        let name = self.utf8(self.text(self.symbol(symbol)?.name.text())?)?;
        self.intern(NameKey::Qualified(scala, name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_context<'a>(
        store: &'a SemanticStore,
        definitions: &'a Definitions,
        index: &'a SourceSemanticIndex,
        source: SourceId,
    ) -> EncodingContext<'a> {
        EncodingContext::new(
            store,
            definitions,
            index,
            source,
            ProjectionLimits::default(),
        )
        .unwrap()
    }

    fn root<'a>(fragment: &'a ProjectedFragment, label: AstLabel) -> RawTree<'a> {
        let offset = fragment.ast.address(label).unwrap() as usize;
        RawTree::decode_with_base_offset(&mut Reader::new(&fragment.ast.bytes()[offset..]), offset)
            .unwrap()
    }

    fn external_meaning(tree: RawTree<'_>, names: &NameTable) -> (String, String) {
        let RawTree::NatAst {
            tag: TYPEREF_TAG,
            value,
            child,
            ..
        } = tree
        else {
            panic!("expected TYPEREF")
        };
        let RawTree::Leaf(SimpleTerm {
            tag: TERMREFPKG_TAG,
            value: TermValue::NameRef(package),
            ..
        }) = *child
        else {
            panic!("expected TERMREFpkg")
        };
        (names.render(package).unwrap(), names.render(value).unwrap())
    }

    fn make_receiver(context: &mut EncodingContext<'_>, int: bool) -> AstLabel {
        context
            .wire_node(AstNode::Leaf(SimpleTerm {
                tag: if int { INTCONST_TAG } else { TRUECONST_TAG },
                offset: 0,
                value: if int {
                    TermValue::Int(7)
                } else {
                    TermValue::Boolean(true)
                },
            }))
            .unwrap()
    }

    #[test]
    fn canonical_operator_references_match_scala_39_select_and_signed_name_shapes() {
        let source = SourceId::from_index(11);
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let index = SourceSemanticIndex::new();
        let int_symbols = definitions.int_primitive_operator_symbols(&store).unwrap();
        // These are the semantic names in the pinned Scala-generated
        // tests/fixtures/tasty_pickler/tasty/example/Algorithms.tasty file.
        let scala_int_operators = ["+", "-", "*", "/", "%", "<", "<=", ">", ">="];
        for (position, symbol) in int_symbols.all().into_iter().enumerate() {
            let expected_operator = scala_int_operators[position];
            let result = if position < 5 {
                definitions.int
            } else {
                definitions.boolean
            };
            let before = store.checkpoint();
            let mut context = make_context(&store, &definitions, &index, source);
            let receiver = make_receiver(&mut context, true);
            let select = context
                .primitive_operator_select(
                    symbol,
                    receiver,
                    definitions.int,
                    &[definitions.int],
                    result,
                    Some(1),
                )
                .unwrap_or_else(|error| panic!("operator {expected_operator:?}: {error:?}"));
            let fragment = context.finish(&[select]).unwrap();
            let RawTree::LengthNode(raw) = root(&fragment, select) else {
                panic!("binary primitive selection should be length-delimited")
            };
            let selected = raw.decode_select_in().unwrap();
            let rendered = fragment
                .names
                .render_signed_name(selected.name)
                .unwrap()
                .unwrap();
            let expected_result = if result == definitions.int {
                "scala.Int"
            } else {
                "scala.Boolean"
            };
            assert_eq!(
                rendered,
                RenderedSignedName::Signed {
                    original: expected_operator.to_owned(),
                    signature: RenderedNameSignature {
                        result: expected_result.to_owned(),
                        parameters: vec![RenderedParamSig::TermParameter("scala.Int".into())],
                    },
                }
            );
            assert!(matches!(
                selected.qualifier,
                RawTree::Leaf(SimpleTerm {
                    tag: INTCONST_TAG,
                    ..
                })
            ));
            assert_eq!(
                external_meaning(selected.underlying_type, &fragment.names),
                ("scala".into(), "Int".into())
            );
            assert_eq!(store.checkpoint(), before);
        }

        let boolean_symbols = definitions.boolean_primitive_operator_ids().unwrap();
        for (symbol, expected_operator) in [
            (boolean_symbols.unary_not, "unary_!"),
            (boolean_symbols.and_then, "&&"),
            (boolean_symbols.or_else, "||"),
        ] {
            let unary = symbol == boolean_symbols.unary_not;
            let before = store.checkpoint();
            let mut context = make_context(&store, &definitions, &index, source);
            let receiver = make_receiver(&mut context, false);
            let boolean_parameter_types = [definitions.boolean];
            let select = context
                .primitive_operator_select(
                    symbol,
                    receiver,
                    definitions.boolean,
                    if unary { &[] } else { &boolean_parameter_types },
                    definitions.boolean,
                    if unary { None } else { Some(1) },
                )
                .unwrap();
            let fragment = context.finish(&[select]).unwrap();
            if unary {
                let raw = root(&fragment, select).decode_select().unwrap();
                assert_eq!(raw.tag, SELECT_TAG);
                assert_eq!(fragment.names.get_utf8(raw.name), Some(expected_operator));
                assert!(matches!(
                    raw.qualifier,
                    RawTree::Leaf(SimpleTerm {
                        tag: TRUECONST_TAG,
                        ..
                    })
                ));
            } else {
                let RawTree::LengthNode(raw) = root(&fragment, select) else {
                    panic!("binary Boolean selection should be length-delimited")
                };
                let selected = raw.decode_select_in().unwrap();
                assert_eq!(
                    fragment
                        .names
                        .render_signed_name(selected.name)
                        .unwrap()
                        .unwrap(),
                    RenderedSignedName::Signed {
                        original: expected_operator.to_owned(),
                        signature: RenderedNameSignature {
                            result: "scala.Boolean".into(),
                            parameters: vec![RenderedParamSig::TermParameter(
                                "scala.Boolean".into()
                            )],
                        },
                    }
                );
                assert!(matches!(
                    selected.qualifier,
                    RawTree::Leaf(SimpleTerm {
                        tag: TRUECONST_TAG,
                        ..
                    })
                ));
                assert_eq!(
                    external_meaning(selected.underlying_type, &fragment.names),
                    ("scala".into(), "Boolean".into())
                );
            }
            assert_eq!(store.checkpoint(), before);
        }
    }

    #[test]
    fn operator_selection_rejects_wrong_receiver_arity_parameter_and_result() {
        let source = SourceId::from_index(12);
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let index = SourceSemanticIndex::new();
        let symbols = definitions.boolean_primitive_operator_ids().unwrap();
        let symbol = symbols.and_then;
        let mut context = make_context(&store, &definitions, &index, source);
        let receiver = make_receiver(&mut context, true);
        assert_eq!(
            context.primitive_operator_select(
                symbol,
                receiver,
                definitions.int,
                &[definitions.boolean],
                definitions.boolean,
                Some(1),
            ),
            Err(PickleError::PrimitiveOperatorReceiver {
                symbol,
                expected: definitions.boolean,
                actual: definitions.int,
            })
        );
        assert_eq!(context.finish(&[]), Err(PickleError::ProjectionAborted));

        let mut context = make_context(&store, &definitions, &index, source);
        let receiver = make_receiver(&mut context, false);
        assert_eq!(
            context.primitive_operator_select(
                symbol,
                receiver,
                definitions.boolean,
                &[definitions.boolean],
                definitions.boolean,
                None,
            ),
            Err(PickleError::PrimitiveOperatorArity {
                symbol,
                expected: Some(1),
                actual: None,
            })
        );

        let mut context = make_context(&store, &definitions, &index, source);
        let receiver = make_receiver(&mut context, false);
        assert_eq!(
            context.primitive_operator_select(
                symbol,
                receiver,
                definitions.boolean,
                &[definitions.int],
                definitions.boolean,
                Some(1),
            ),
            Err(PickleError::PrimitiveOperatorParameter {
                symbol,
                index: 0,
                expected: definitions.boolean,
                actual: definitions.int,
            })
        );

        let mut context = make_context(&store, &definitions, &index, source);
        let receiver = make_receiver(&mut context, false);
        assert_eq!(
            context.primitive_operator_select(
                symbol,
                receiver,
                definitions.boolean,
                &[definitions.boolean],
                definitions.int,
                Some(1),
            ),
            Err(PickleError::PrimitiveOperatorResult {
                symbol,
                expected: definitions.boolean,
                actual: definitions.int,
            })
        );
    }

    #[test]
    fn operator_selection_rejects_noncanonical_metadata_and_enforces_budgets() {
        let source = SourceId::from_index(15);
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let index = SourceSemanticIndex::new();
        let symbols = definitions.boolean_primitive_operator_ids().unwrap();

        let mut context = make_context(&store, &definitions, &index, source);
        let receiver = make_receiver(&mut context, false);
        assert_eq!(
            context.primitive_operator_select(
                symbols.eager_and,
                receiver,
                definitions.boolean,
                &[definitions.boolean],
                definitions.boolean,
                Some(1),
            ),
            Err(PickleError::Unsupported {
                feature: UnsupportedFeature::PrimitiveOperator,
                symbol: Some(symbols.eager_and),
            })
        );

        store.symbols.get_mut(symbols.and_then).flags = SymbolFlags::FINAL;
        let mut context = make_context(&store, &definitions, &index, source);
        let receiver = make_receiver(&mut context, false);
        assert_eq!(
            context.primitive_operator_select(
                symbols.and_then,
                receiver,
                definitions.boolean,
                &[definitions.boolean],
                definitions.boolean,
                Some(1),
            ),
            Err(PickleError::Reference {
                symbol: symbols.and_then,
                reason: ReferenceError::Metadata,
            })
        );

        store.symbols.get_mut(symbols.and_then).flags = SymbolFlags::EMPTY;
        store.symbols.get_mut(symbols.and_then).owner = Some(definitions.object_class);
        let mut context = make_context(&store, &definitions, &index, source);
        let receiver = make_receiver(&mut context, false);
        assert_eq!(
            context.primitive_operator_select(
                symbols.and_then,
                receiver,
                definitions.boolean,
                &[definitions.boolean],
                definitions.boolean,
                Some(1),
            ),
            Err(PickleError::Reference {
                symbol: symbols.and_then,
                reason: ReferenceError::Owner,
            })
        );

        // Context initialization resolves canonical builtins and the supplied
        // receiver consumes one item; leave no fuel for the selection itself.
        let limits = ProjectionLimits {
            max_items: 5,
            ..ProjectionLimits::default()
        };
        let mut context =
            EncodingContext::new(&store, &definitions, &index, source, limits).unwrap();
        let receiver = make_receiver(&mut context, false);
        assert_eq!(
            context.primitive_operator_select(
                symbols.and_then,
                receiver,
                definitions.boolean,
                &[definitions.boolean],
                definitions.boolean,
                Some(1),
            ),
            Err(PickleError::LimitExceeded {
                kind: LimitKind::Items,
                limit: 5,
            })
        );
        assert_eq!(context.finish(&[]), Err(PickleError::ProjectionAborted));
    }

    #[test]
    fn operator_selection_rejects_unregistered_symbols_and_stale_ids() {
        let source = SourceId::from_index(13);
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let index = SourceSemanticIndex::new();
        let symbols = definitions.int_primitive_operator_symbols(&store).unwrap();
        let symbol_checkpoint = store.checkpoint();
        let custom = store.symbols.alloc(Symbol {
            name: Name::new(store.names.intern("+"), Namespace::Term),
            owner: Some(definitions.object_class),
            kind: SymbolKind::Method,
            flags: SymbolFlags::EMPTY,
            visibility: Visibility::Public,
            info: SymbolInfo::Complete(definitions.int),
            origin: SymbolOrigin::Source(source),
            annotations: Vec::new(),
            position: None,
            links: SymbolLinks::default(),
        });
        let mut context = make_context(&store, &definitions, &index, source);
        let receiver = make_receiver(&mut context, true);
        assert_eq!(
            context.primitive_operator_select(
                custom,
                receiver,
                definitions.int,
                &[definitions.int],
                definitions.int,
                Some(1),
            ),
            Err(PickleError::Reference {
                symbol: custom,
                reason: ReferenceError::NotRegistered,
            })
        );
        drop(context);
        store.rollback_to(symbol_checkpoint);

        let mut context = make_context(&store, &definitions, &index, source);
        let receiver = make_receiver(&mut context, true);
        let stale = custom;
        assert_eq!(
            context.primitive_operator_select(
                stale,
                receiver,
                definitions.int,
                &[definitions.int],
                definitions.int,
                Some(1),
            ),
            Err(PickleError::InvalidSymbol { symbol: stale })
        );

        drop(context);
        let type_checkpoint = store.checkpoint();
        let stale_type = store.types.alloc(Type::NoType);
        store.rollback_to(type_checkpoint);
        let mut context = make_context(&store, &definitions, &index, source);
        let receiver = make_receiver(&mut context, true);
        assert_eq!(
            context.primitive_operator_select(
                symbols.plus,
                receiver,
                stale_type,
                &[definitions.int],
                definitions.int,
                Some(1),
            ),
            Err(PickleError::InvalidType { ty: stale_type })
        );
    }

    #[test]
    fn canonical_boolean_identity_survives_but_malformed_signature_fails_closed() {
        let source = SourceId::from_index(14);
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let index = SourceSemanticIndex::new();
        let symbols = definitions.boolean_primitive_operator_ids().unwrap();
        let symbol = symbols.and_then;
        let malformed = store.types.alloc(Type::Method(MethodType {
            params: vec![MethodParam {
                name: TermName::new(store.names.intern("rhs")),
                ty: definitions.int,
                erased: false,
                varargs: false,
            }],
            result: definitions.boolean,
            kind: MethodKind::Plain,
        }));
        store.symbols.get_mut(symbol).info = SymbolInfo::Complete(malformed);
        let mut context = make_context(&store, &definitions, &index, source);
        let receiver = make_receiver(&mut context, false);
        assert_eq!(
            context.primitive_operator_select(
                symbol,
                receiver,
                definitions.boolean,
                &[definitions.boolean],
                definitions.boolean,
                Some(1),
            ),
            Err(PickleError::MalformedType {
                ty: malformed,
                role: TypeRole::Parameter,
            })
        );

        store.symbols.get_mut(symbol).info = SymbolInfo::Missing;
        let mut context = make_context(&store, &definitions, &index, source);
        let receiver = make_receiver(&mut context, false);
        assert_eq!(
            context.primitive_operator_select(
                symbol,
                receiver,
                definitions.boolean,
                &[definitions.boolean],
                definitions.boolean,
                Some(1),
            ),
            Err(PickleError::IncompleteSymbol {
                symbol,
                info: SymbolInfo::Missing,
            })
        );
    }
}
