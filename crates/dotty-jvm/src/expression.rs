//! Lowering for the first, value-producing typed expression leaves.

use dotty_classfile::bytecode::CodeBuilder;
use dotty_core::ast::{TreeKind, Typed, TypedAst};
use dotty_core::definitions::Definitions;
use dotty_core::ids::{SymbolId, TreeId, TypeId};
use dotty_core::store::SemanticStore;
use dotty_core::symbols::SymbolInfo;
use dotty_core::types::{Constant, TermRefTarget, Type, TypeRefTarget};

use crate::{InvalidTypeReason, JvmCodegenError, JvmMethodMetadata, JvmType, JvmValueCategory};

pub(super) fn lower_value_expression(
    store: &SemanticStore,
    typed_ast: &TypedAst,
    definitions: &Definitions,
    method: &JvmMethodMetadata,
    tree_id: TreeId<Typed>,
    code: &mut CodeBuilder,
) -> Result<JvmValueCategory, JvmCodegenError> {
    let tree = typed_ast
        .try_get(tree_id)
        .ok_or(JvmCodegenError::UnsupportedTypedExpression {
            tree: tree_id,
            node: "missing tree",
        })?;
    validate_tree_type(store, tree_id, tree.ty)?;

    match &tree.kind {
        TreeKind::Literal(literal) => {
            let (value, primitive) = match literal.value {
                Constant::Int(value) => (value, JvmType::Int),
                Constant::Boolean(value) => (i32::from(value), JvmType::Boolean),
                _ => {
                    return Err(JvmCodegenError::UnsupportedTypedExpression {
                        tree: tree_id,
                        node: "non-Int/Boolean literal",
                    });
                }
            };
            let typed_primitive = primitive_type(store, tree.ty, definitions);
            let matches_constant_type = match store.types.try_get(tree.ty) {
                Some(Type::Constant(typed_value)) => typed_value == &literal.value,
                _ => typed_primitive == Some(primitive),
            };
            if !matches_constant_type || typed_primitive != Some(primitive) {
                return Err(JvmCodegenError::LiteralTypeMismatch {
                    tree: tree_id,
                    value: primitive,
                    typed: typed_primitive,
                });
            }
            code.iconst(value)
                .map_err(|error| JvmCodegenError::CodeBuilder {
                    tree: tree_id,
                    error,
                })?;
        }
        TreeKind::Ident(_) => {
            let symbol = match store.types.try_get(tree.ty) {
                Some(Type::TermRef {
                    target: TermRefTarget::Symbol(symbol),
                    ..
                }) => Some(*symbol),
                _ => None,
            };
            let Some(parameter) = symbol.and_then(|symbol| {
                method
                    .parameters()
                    .iter()
                    .find(|parameter| parameter.symbol() == symbol)
                    .copied()
            }) else {
                return Err(JvmCodegenError::InvalidParameterReference {
                    tree: tree_id,
                    symbol,
                });
            };

            if !store.symbols.contains(parameter.symbol()) {
                return Err(JvmCodegenError::InvalidParameterReference {
                    tree: tree_id,
                    symbol: Some(parameter.symbol()),
                });
            }
            let symbol_type = match store.symbols.info(parameter.symbol()) {
                SymbolInfo::Complete(ty) => Some(*ty),
                SymbolInfo::Missing | SymbolInfo::Deferred(_) | SymbolInfo::Error => None,
            };
            let found = symbol_type.and_then(|ty| primitive_type(store, ty, definitions));
            if found != Some(parameter.ty()) {
                return Err(JvmCodegenError::ParameterReferenceTypeMismatch {
                    tree: tree_id,
                    parameter: parameter.symbol(),
                    expected: parameter.ty(),
                    found,
                });
            }
            code.iload(usize::from(parameter.slot())).map_err(|error| {
                JvmCodegenError::CodeBuilder {
                    tree: tree_id,
                    error,
                }
            })?;
        }
        kind => {
            return Err(JvmCodegenError::UnsupportedTypedExpression {
                tree: tree_id,
                node: tree_kind_name(kind),
            });
        }
    }
    Ok(JvmValueCategory::IntLike)
}

fn validate_tree_type(
    store: &SemanticStore,
    tree: TreeId<Typed>,
    ty: TypeId,
) -> Result<(), JvmCodegenError> {
    let reason = match store.types.try_get(ty) {
        None => Some(InvalidTypeReason::MissingOrUnfilled),
        Some(Type::NoType) => Some(InvalidTypeReason::NoType),
        Some(Type::Error(_)) => Some(InvalidTypeReason::Error),
        Some(Type::NoPrefix) => Some(InvalidTypeReason::NoPrefix),
        Some(_) => None,
    };
    match reason {
        Some(reason) => Err(JvmCodegenError::InvalidExpressionType { tree, ty, reason }),
        None => Ok(()),
    }
}

fn primitive_type(store: &SemanticStore, ty: TypeId, definitions: &Definitions) -> Option<JvmType> {
    match store.types.try_get(ty)? {
        Type::Constant(Constant::Int(_)) => Some(JvmType::Int),
        Type::Constant(Constant::Boolean(_)) => Some(JvmType::Boolean),
        Type::TypeRef {
            target: TypeRefTarget::Symbol(symbol),
            ..
        } if *symbol == type_symbol(store, definitions.int)? => Some(JvmType::Int),
        Type::TypeRef {
            target: TypeRefTarget::Symbol(symbol),
            ..
        } if *symbol == type_symbol(store, definitions.boolean)? => Some(JvmType::Boolean),
        _ => None,
    }
}

fn type_symbol(store: &SemanticStore, ty: TypeId) -> Option<SymbolId> {
    match store.types.try_get(ty)? {
        Type::TypeRef {
            target: TypeRefTarget::Symbol(symbol),
            ..
        } => Some(*symbol),
        _ => None,
    }
}

fn tree_kind_name(kind: &TreeKind<Typed>) -> &'static str {
    match kind {
        TreeKind::Ident(_) => "Ident",
        TreeKind::Select(_) => "Select",
        TreeKind::This(_) => "This",
        TreeKind::Super(_) => "Super",
        TreeKind::Literal(_) => "Literal",
        TreeKind::Apply(_) => "Apply",
        TreeKind::TypeApply(_) => "TypeApply",
        TreeKind::New(_) => "New",
        TreeKind::Typed(_) => "Typed",
        TreeKind::NamedArg(_) => "NamedArg",
        TreeKind::Assign(_) => "Assign",
        TreeKind::Block(_) => "Block",
        TreeKind::If(_) => "If",
        TreeKind::Match(_) => "Match",
        TreeKind::CaseDef(_) => "CaseDef",
        TreeKind::Return(_) => "Return",
        TreeKind::While(_) => "While",
        TreeKind::Try(_) => "Try",
        TreeKind::Closure(_) => "Closure",
        TreeKind::ValDef(_) => "ValDef",
        TreeKind::DefDef(_) => "DefDef",
        TreeKind::TypeDef(_) => "TypeDef",
        TreeKind::Template(_) => "Template",
        TreeKind::PackageDef(_) => "PackageDef",
        TreeKind::Import(_) => "Import",
        TreeKind::Export(_) => "Export",
        TreeKind::TypeTree(_) => "TypeTree",
        TreeKind::SingletonTypeTree(_) => "SingletonTypeTree",
        TreeKind::AppliedTypeTree(_) => "AppliedTypeTree",
        TreeKind::RefinedTypeTree(_) => "RefinedTypeTree",
        TreeKind::LambdaTypeTree(_) => "LambdaTypeTree",
        TreeKind::MatchTypeTree(_) => "MatchTypeTree",
        TreeKind::ByNameTypeTree(_) => "ByNameTypeTree",
        TreeKind::TypeBoundsTree(_) => "TypeBoundsTree",
        TreeKind::Bind(_) => "Bind",
        TreeKind::Alternative(_) => "Alternative",
        TreeKind::UnApply(_) => "UnApply",
        TreeKind::Annotated(_) => "Annotated",
        TreeKind::Quote(_) => "Quote",
        TreeKind::Splice(_) => "Splice",
        TreeKind::QuotePattern(_) => "QuotePattern",
        TreeKind::SplicePattern(_) => "SplicePattern",
        TreeKind::Inlined(_) => "Inlined",
        TreeKind::PhaseSpecific(_) => "PhaseSpecific",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dotty_classfile::attribute::Attribute;
    use dotty_classfile::bytecode::CodeBuilder;
    use dotty_classfile::class_file::ClassFile;
    use dotty_classfile::class_file::ClassFileVersion;
    use dotty_classfile::class_file_builder::ClassFileBuilder;
    use dotty_classfile::descriptor::MethodDescriptor;
    use dotty_classfile::reader::Reader;
    use dotty_core::ast::{Ident, Tree};
    use dotty_core::ids::TypeId;
    use dotty_core::names::{Name, Namespace};
    use dotty_core::symbols::{
        Symbol, SymbolFlags, SymbolInfo, SymbolKind, SymbolLinks, SymbolOrigin, Visibility,
    };
    use dotty_core::types::{TermRefTarget, Type};

    use crate::{JvmMethodDescriptor, JvmParameterLocal};

    struct Fixture {
        store: SemanticStore,
        definitions: Definitions,
        ast: TypedAst,
        method: JvmMethodMetadata,
        parameters: Vec<SymbolId>,
    }

    impl Fixture {
        fn new(parameter_types: &[JvmType]) -> Self {
            let mut store = SemanticStore::new();
            let definitions = Definitions::bootstrap(&mut store);
            let class = symbol(&mut store, SymbolKind::Class, None, definitions.int);
            let method_symbol =
                symbol(&mut store, SymbolKind::Method, Some(class), definitions.int);
            let mut parameters = Vec::new();
            let mut locals = Vec::new();
            for (slot, ty) in (1u16..).zip(parameter_types.iter()) {
                let param_type = match ty {
                    JvmType::Int => definitions.int,
                    JvmType::Boolean => definitions.boolean,
                };
                let parameter = symbol(
                    &mut store,
                    SymbolKind::Parameter,
                    Some(method_symbol),
                    param_type,
                );
                parameters.push(parameter);
                locals.push(JvmParameterLocal {
                    symbol: parameter,
                    ty: *ty,
                    slot,
                    width: 1,
                });
            }
            let descriptor_text = format!(
                "({})I",
                parameter_types
                    .iter()
                    .map(|ty| match ty {
                        JvmType::Int => 'I',
                        JvmType::Boolean => 'Z',
                    })
                    .collect::<String>()
            );
            let descriptor = MethodDescriptor::parse(&descriptor_text).unwrap();
            let method = JvmMethodMetadata {
                symbol: method_symbol,
                name: "value".to_owned(),
                descriptor: JvmMethodDescriptor {
                    descriptor,
                    descriptor_text,
                    parameters: parameter_types.to_vec(),
                    result: crate::JvmReturnType::Value(JvmType::Int),
                },
                access_flags: dotty_classfile::access_flags::MethodAccessFlags(
                    dotty_classfile::access_flags::ACC_PUBLIC,
                ),
                parameters: locals,
            };
            Self {
                store,
                definitions,
                ast: TypedAst::new(),
                method,
                parameters,
            }
        }

        fn literal(&mut self, value: Constant, ty: TypeId) -> TreeId<Typed> {
            self.ast.alloc(Tree {
                kind: TreeKind::Literal(dotty_core::ast::Literal { value }),
                position: None,
                ty,
            })
        }

        fn ident(&mut self, ty: TypeId) -> TreeId<Typed> {
            self.ast.alloc(Tree {
                kind: TreeKind::Ident(Ident {
                    name: Name::new(self.store.names.intern("sameName"), Namespace::Term),
                    backquoted: false,
                }),
                position: None,
                ty,
            })
        }
    }

    fn symbol(
        store: &mut SemanticStore,
        kind: SymbolKind,
        owner: Option<SymbolId>,
        ty: TypeId,
    ) -> SymbolId {
        store.symbols.alloc(Symbol {
            name: Name::new(store.names.intern("sameName"), Namespace::Term),
            owner,
            kind,
            flags: SymbolFlags::EMPTY,
            visibility: Visibility::Public,
            info: SymbolInfo::Complete(ty),
            origin: SymbolOrigin::Synthetic,
            annotations: Vec::new(),
            position: None,
            links: SymbolLinks::default(),
        })
    }

    fn code_bytes(fixture: &Fixture, code: &CodeBuilder) -> Vec<u8> {
        let mut encodable = code.clone();
        encodable.ireturn().unwrap();
        let mut class = ClassFileBuilder::new(
            "Example",
            ClassFileVersion {
                minor: 0,
                major: 61,
            },
        )
        .unwrap();
        class
            .add_instance_method(
                "value",
                fixture.method.descriptor.descriptor_text.as_str(),
                |output| {
                    *output = encodable;
                    Ok(())
                },
            )
            .unwrap();
        let bytes = class.encode().unwrap();
        let mut reader = Reader::new(&bytes);
        let decoded = ClassFile::decode(&mut reader).unwrap();
        decoded.methods[0]
            .attributes
            .iter()
            .find_map(|attribute| match attribute {
                Attribute::Code(code) => Some(code.code.to_vec()),
                _ => None,
            })
            .unwrap()
    }

    fn lower_and_return(fixture: &Fixture, tree: TreeId<Typed>) -> (JvmValueCategory, Vec<u8>) {
        let mut code = CodeBuilder::new();
        let category = lower_value_expression(
            &fixture.store,
            &fixture.ast,
            &fixture.definitions,
            &fixture.method,
            tree,
            &mut code,
        )
        .unwrap();
        (category, code_bytes(fixture, &code))
    }

    #[test]
    fn lowers_integer_and_boolean_literals_to_int_like_values() {
        let mut fixture = Fixture::new(&[]);
        let int_value = fixture.literal(Constant::Int(42), fixture.definitions.int);
        let true_value = fixture.literal(Constant::Boolean(true), fixture.definitions.boolean);
        let false_value = fixture.literal(Constant::Boolean(false), fixture.definitions.boolean);

        assert_eq!(
            lower_and_return(&fixture, int_value),
            (JvmValueCategory::IntLike, vec![0x10, 42, 0xac])
        );
        assert_eq!(
            lower_and_return(&fixture, true_value),
            (JvmValueCategory::IntLike, vec![0x04, 0xac])
        );
        assert_eq!(
            lower_and_return(&fixture, false_value),
            (JvmValueCategory::IntLike, vec![0x03, 0xac])
        );
    }

    #[test]
    fn lowers_parameter_references_by_symbol_identity_and_ordered_slot() {
        let mut fixture = Fixture::new(&[JvmType::Int, JvmType::Int]);
        let int_type = fixture.definitions.int;
        let first = fixture.parameters[0];
        let second = fixture.parameters[1];
        let first_ref = fixture.store.types.alloc(Type::TermRef {
            prefix: fixture.definitions.no_prefix,
            target: TermRefTarget::Symbol(first),
        });
        let second_ref = fixture.store.types.alloc(Type::TermRef {
            prefix: fixture.definitions.no_prefix,
            target: TermRefTarget::Symbol(second),
        });
        let first_tree = fixture.ident(first_ref);
        let second_tree = fixture.ident(second_ref);
        assert_eq!(
            lower_and_return(&fixture, first_tree),
            (JvmValueCategory::IntLike, vec![0x1b, 0xac])
        );
        assert_eq!(
            lower_and_return(&fixture, second_tree),
            (JvmValueCategory::IntLike, vec![0x1c, 0xac])
        );

        let other = symbol(
            &mut fixture.store,
            SymbolKind::Parameter,
            Some(fixture.method.symbol()),
            int_type,
        );
        let other_ref = fixture.store.types.alloc(Type::TermRef {
            prefix: fixture.definitions.no_prefix,
            target: TermRefTarget::Symbol(other),
        });
        let other_tree = fixture.ident(other_ref);
        let mut code = CodeBuilder::new();
        code.iconst(0).unwrap();
        let before = code_bytes(&fixture, &code);
        assert_eq!(
            lower_value_expression(
                &fixture.store,
                &fixture.ast,
                &fixture.definitions,
                &fixture.method,
                other_tree,
                &mut code,
            ),
            Err(JvmCodegenError::InvalidParameterReference {
                tree: other_tree,
                symbol: Some(other),
            })
        );
        assert_eq!(code_bytes(&fixture, &code), before);
    }

    #[test]
    fn boolean_and_integer_parameters_use_their_ordered_int_like_slots() {
        let mut fixture = Fixture::new(&[JvmType::Boolean, JvmType::Int, JvmType::Int]);
        let parameter_symbols = fixture.parameters.clone();
        let references = parameter_symbols
            .iter()
            .copied()
            .map(|symbol| {
                let ty = fixture.store.types.alloc(Type::TermRef {
                    prefix: fixture.definitions.no_prefix,
                    target: TermRefTarget::Symbol(symbol),
                });
                fixture.ident(ty)
            })
            .collect::<Vec<_>>();
        for (tree, expected) in
            references
                .into_iter()
                .zip([vec![0x1b, 0xac], vec![0x1c, 0xac], vec![0x1d, 0xac]])
        {
            assert_eq!(
                lower_and_return(&fixture, tree),
                (JvmValueCategory::IntLike, expected)
            );
        }
    }

    #[test]
    fn rejects_mismatched_and_unsupported_leaves_without_mutating_code() {
        let mut fixture = Fixture::new(&[]);
        let mismatch = fixture.literal(Constant::Int(7), fixture.definitions.boolean);
        let singleton_type = fixture.store.types.alloc(Type::Constant(Constant::Int(5)));
        let mismatched_singleton = fixture.literal(Constant::Int(7), singleton_type);
        let unsupported = fixture.literal(Constant::Long(7), fixture.definitions.long);
        for (tree, expected) in [
            (
                mismatch,
                JvmCodegenError::LiteralTypeMismatch {
                    tree: mismatch,
                    value: JvmType::Int,
                    typed: Some(JvmType::Boolean),
                },
            ),
            (
                mismatched_singleton,
                JvmCodegenError::LiteralTypeMismatch {
                    tree: mismatched_singleton,
                    value: JvmType::Int,
                    typed: Some(JvmType::Int),
                },
            ),
            (
                unsupported,
                JvmCodegenError::UnsupportedTypedExpression {
                    tree: unsupported,
                    node: "non-Int/Boolean literal",
                },
            ),
        ] {
            let mut code = CodeBuilder::new();
            code.iconst(0).unwrap();
            let before = code_bytes(&fixture, &code);
            assert_eq!(
                lower_value_expression(
                    &fixture.store,
                    &fixture.ast,
                    &fixture.definitions,
                    &fixture.method,
                    tree,
                    &mut code,
                ),
                Err(expected)
            );
            assert_eq!(code_bytes(&fixture, &code), before);
        }
    }

    #[test]
    fn rejects_a_missing_or_sentinel_typed_node() {
        let mut fixture = Fixture::new(&[]);
        let tree = fixture.literal(Constant::Int(1), fixture.definitions.no_prefix);
        let mut code = CodeBuilder::new();
        code.iconst(0).unwrap();
        let before = code_bytes(&fixture, &code);
        assert_eq!(
            lower_value_expression(
                &fixture.store,
                &fixture.ast,
                &fixture.definitions,
                &fixture.method,
                tree,
                &mut code,
            ),
            Err(JvmCodegenError::InvalidExpressionType {
                tree,
                ty: fixture.definitions.no_prefix,
                reason: InvalidTypeReason::NoPrefix,
            })
        );
        assert_eq!(code_bytes(&fixture, &code), before);
    }
}
