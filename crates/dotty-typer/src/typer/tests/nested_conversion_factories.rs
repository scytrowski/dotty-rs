use super::*;

const FACTORIES: &str = r#"
trait Factory[A] {
  class Ops(lhs: A)
  implicit def localView(lhs: A): Ops = new Ops(lhs)
}
object Views {
  implicit def viaWitness[B](x: B)(implicit num: Factory[B]): Factory[B]#Ops = new num.Ops(x)
}
object Controls {
  def samePath[B](x: B)(implicit n: Factory[B], m: Factory[B]): n.Ops = new n.Ops(x)
  def wrongInput[B](x: Boolean)(implicit n: Factory[B]): n.Ops = new n.Ops(x)
}
"#;

#[test]
fn nested_conversion_factories_preserve_the_enclosing_instance() {
    for name in ["localView", "viaWitness"] {
        let (parsed, mut store, packages, defs, index, source) = parse_and_name(FACTORIES);
        let inner = class_symbol(&parsed, &store, &index, source, "Ops");
        let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, name);
        let Some(SourceDefinition::Canonical { tree, .. }) = index.definition_of(method) else {
            panic!("source factory")
        };
        let TreeKind::DefDef(def) = &parsed.ast.get(tree).kind else {
            unreachable!()
        };
        let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages);
        typer.type_method_definition(tree).unwrap();
        let ast = typer.typed_ast();
        let body = ast.get(typer.source_typed_index().get(source, rhs).unwrap());
        let TreeKind::Apply(call) = &body.kind else {
            panic!("constructor application")
        };
        let Type::TypeRef {
            prefix,
            target: TypeRefTarget::Symbol(target),
        } = *typer.store.types.get(body.ty)
        else {
            panic!("selected nested class")
        };
        assert_eq!(target, inner);
        let [argument] = call.args[..] else {
            panic!("one source argument")
        };
        assert_eq!(
            typer
                .store
                .types
                .get(ast.get(argument).ty)
                .reference_symbol(),
            index.symbol_at(source, def.value_param_clauses[0][0])
        );
        let prefix = typer.store.types.get(prefix);
        if name == "localView" {
            assert_eq!(
                *prefix,
                Type::ThisType {
                    class: typer.store.symbols.owner(inner).unwrap()
                }
            );
        } else {
            assert!(
                matches!(prefix, Type::TermRef {target: TermRefTarget::Symbol(witness), ..}
                if Some(*witness) == index.symbol_at(source, def.value_param_clauses[1][0]))
            );
        }
    }
}

#[test]
fn nested_conversion_results_distinguish_witnesses_and_projected_outer_arguments() {
    let (parsed, mut store, packages, defs, index, source) = parse_and_name(FACTORIES);
    let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "samePath");
    let Some(SourceDefinition::Canonical { tree, .. }) = index.definition_of(method) else {
        panic!("source control")
    };
    let TreeKind::DefDef(def) = &parsed.ast.get(tree).kind else {
        unreachable!()
    };
    let sibling = index
        .symbol_at(source, def.value_param_clauses[1][1])
        .unwrap();
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages);
    typer.type_method_definition(tree).unwrap();
    let body = typer.source_typed_index().get(source, rhs).unwrap();
    let actual = typer.typed_ast().get(body).ty;
    let Type::TypeRef {
        prefix,
        target: TypeRefTarget::Symbol(inner),
    } = *typer.store.types.get(actual)
    else {
        panic!("nested class")
    };
    let Type::TermRef {
        prefix: enclosing,
        target: TermRefTarget::Symbol(witness),
    } = *typer.store.types.get(prefix)
    else {
        panic!("first witness")
    };
    assert_ne!(sibling, witness);
    let other = typer.store.types.alloc(Type::term_ref(enclosing, sibling));
    let sibling_ops = typer.store.types.alloc(Type::type_ref(other, inner));
    assert!(!typer.conforms(actual, sibling_ops).unwrap());
    let SymbolInfo::Complete(outer) = typer.store.symbols.get(witness).info else {
        panic!("witness type")
    };
    let Type::Applied { tycon, .. } = *typer.store.types.get(outer) else {
        panic!("Factory[B]")
    };
    let projected = typer.store.types.alloc(Type::type_ref(outer, inner));
    assert!(typer.conforms(actual, projected).unwrap());
    let wrong = typer.store.types.alloc(Type::Applied {
        tycon,
        args: vec![defs.boolean],
    });
    let wrong_ops = typer.store.types.alloc(Type::type_ref(wrong, inner));
    assert!(!typer.conforms(actual, wrong_ops).unwrap());
}

#[test]
fn nested_constructor_rejects_an_operand_outside_the_witness_type() {
    let (parsed, mut store, packages, defs, index, source) = parse_and_name(FACTORIES);
    let (method, rhs) = method_definition_and_rhs(&parsed, &store, &index, source, "wrongInput");
    let Some(SourceDefinition::Canonical { tree, .. }) = index.definition_of(method) else {
        panic!("source control")
    };
    let mut typer = SourceTyper::new(&parsed.ast, source, &index, &mut store, defs, &packages);
    let checkpoint = typer.store.checkpoint();
    let error = typer.type_method_definition(tree).unwrap_err();
    assert!(
        matches!(error, TyperError::ApplicationArgumentConformanceUnsupported { source: s, tree_index, argument_index: 0, .. } if s == source && tree_index == rhs.index()),
        "{error:?}"
    );
    assert!(typer.source_typed_index().get(source, rhs).is_none());
    assert_eq!(typer.store.checkpoint(), checkpoint);
}
