//! ID-independent semantic normalization shared by source/TASTy parity tests.

use std::collections::HashMap;

use dotty_core::ast::TreeKind;
use dotty_core::ids::{SourceId, SymbolId, TypeId};
use dotty_core::names::{Name, Namespace};
use dotty_core::symbols::{
    Scope, Symbol, SymbolFlags, SymbolInfo, SymbolKind, SymbolLinks, SymbolOrigin, Visibility,
};
use dotty_core::types::{ClassInfo, Type, TypeRefTarget};
use dotty_core::{Definitions, Packages, SemanticStore, SourceText};
use dotty_lexer::ContextualScanner;
use dotty_namer::name_compilation_unit;
use dotty_parser::parse_compilation_unit;
use dotty_tasty::tasty::{DefinitionBody, StructuredNode, TastyFile};
use dotty_tasty_unpickler::tasty_unpickler::TastyUnpickler;
use dotty_typer::SourceTyper;

const CLASS_INFOS_SOURCE: &str =
    include_str!("../../dotty-tasty-unpickler/tests/fixtures/semantic/ClassInfos.scala");
const INFO_PARENT_TASTY: &[u8] =
    include_bytes!("../../dotty-tasty-unpickler/tests/fixtures/semantic/InfoParent.tasty");
const INFO_BASE_TASTY: &[u8] =
    include_bytes!("../../dotty-tasty-unpickler/tests/fixtures/semantic/InfoBase.tasty");

fn symbol_key(store: &SemanticStore, symbol: SymbolId) -> String {
    let entry = store.symbols.get(symbol);
    let name = store.names.resolve(entry.name.text());
    if entry.origin == SymbolOrigin::Builtin {
        let owner = match name {
            "Any" | "Nothing" => "Package:scala",
            "Object" => "Package:java/Package:lang",
            _ => "",
        };
        if !owner.is_empty() {
            return format!("{owner}/Type:{name}:{:?}", entry.kind);
        }
    }
    let component = if entry.kind == SymbolKind::Package {
        format!("Package:{name}")
    } else {
        format!("{:?}:{}:{:?}", entry.name.namespace(), name, entry.kind)
    };
    match entry.owner {
        Some(owner)
            if store.symbols.get(owner).kind != SymbolKind::Package
                || !store
                    .names
                    .resolve(store.symbols.get(owner).name.text())
                    .is_empty() =>
        {
            format!("{}/{component}", symbol_key(store, owner))
        }
        _ => component,
    }
}

fn normalized_type(store: &SemanticStore, ty: TypeId) -> String {
    render_type(store, ty, &mut HashMap::new(), &mut Vec::new())
}

fn render_type(
    store: &SemanticStore,
    ty: TypeId,
    binders: &mut HashMap<TypeId, usize>,
    active: &mut Vec<TypeId>,
) -> String {
    if let Some(label) = binders.get(&ty) {
        return format!("binder#{label}");
    }
    if active.contains(&ty) {
        return "recursive".to_owned();
    }
    active.push(ty);
    let result = match store.types.get(ty) {
        Type::NoType => "NoType".to_owned(),
        Type::Error(error) => format!("Error({})", store.names.resolve(error.message)),
        Type::NoPrefix => "NoPrefix".to_owned(),
        Type::TermRef { prefix, target } => {
            let target = match target {
                dotty_core::types::TermRefTarget::Symbol(symbol) => symbol_key(store, *symbol),
                dotty_core::types::TermRefTarget::Name(name) => {
                    format!("name:{}", store.names.resolve(name.as_name().text()))
                }
            };
            format!(
                "TermRef({}, {target})",
                render_type(store, *prefix, binders, active)
            )
        }
        Type::TypeRef { prefix, target } => {
            let target = match target {
                TypeRefTarget::Symbol(symbol) => symbol_key(store, *symbol),
                TypeRefTarget::Name(name) => {
                    format!("name:{}", store.names.resolve(name.as_name().text()))
                }
            };
            let prefix = if target == "Package:java/Package:lang/Type:Object:Class"
                || target.starts_with("Package:scala/Type:")
            {
                // The source Typer uses canonical no-prefix builtin types,
                // while Scala 3.9 TASTy writes them through scala/java.lang
                // package prefixes. Normalize this named builtin family.
                "NoPrefix".to_owned()
            } else {
                render_type(store, *prefix, binders, active)
            };
            format!("TypeRef({prefix}, {target})")
        }
        Type::ThisType { class } => format!("This({})", symbol_key(store, *class)),
        Type::SuperType {
            this_type,
            super_type,
        } => format!(
            "Super({}, {})",
            render_type(store, *this_type, binders, active),
            render_type(store, *super_type, binders, active)
        ),
        Type::Constant(constant) => match constant {
            dotty_core::types::Constant::Unit => "Unit".to_owned(),
            dotty_core::types::Constant::Null => "Null".to_owned(),
            dotty_core::types::Constant::Boolean(value) => format!("Boolean({value})"),
            dotty_core::types::Constant::Byte(value) => format!("Byte({value})"),
            dotty_core::types::Constant::Short(value) => format!("Short({value})"),
            dotty_core::types::Constant::Char(value) => format!("Char({value})"),
            dotty_core::types::Constant::Int(value) => format!("Int({value})"),
            dotty_core::types::Constant::Long(value) => format!("Long({value})"),
            dotty_core::types::Constant::FloatBits(value) => format!("FloatBits({value})"),
            dotty_core::types::Constant::DoubleBits(value) => format!("DoubleBits({value})"),
            dotty_core::types::Constant::String(value) => {
                format!("String({})", store.names.resolve(*value))
            }
            dotty_core::types::Constant::StringUtf16(value) => format!("StringUtf16({value:?})"),
            dotty_core::types::Constant::Class(value) => {
                format!("Class({})", render_type(store, *value, binders, active))
            }
        },
        Type::Applied { tycon, args } => format!(
            "Applied({}, [{}])",
            render_type(store, *tycon, binders, active),
            args.iter()
                .map(|arg| render_type(store, *arg, binders, active))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Type::Bounds { low, high } => format!(
            "Bounds({}, {})",
            render_type(store, *low, binders, active),
            render_type(store, *high, binders, active)
        ),
        Type::AliasingBounds { alias } => {
            format!("Alias({})", render_type(store, *alias, binders, active))
        }
        Type::ByName { result } => {
            format!("ByName({})", render_type(store, *result, binders, active))
        }
        Type::Flexible { underlying } => format!(
            "Flexible({})",
            render_type(store, *underlying, binders, active)
        ),
        Type::And { left, right } => format!(
            "And({}, {})",
            render_type(store, *left, binders, active),
            render_type(store, *right, binders, active)
        ),
        Type::Or { left, right } => format!(
            "Or({}, {})",
            render_type(store, *left, binders, active),
            render_type(store, *right, binders, active)
        ),
        Type::Refined { parent, name, info } => format!(
            "Refined({}, {:?}:{}, {})",
            render_type(store, *parent, binders, active),
            name.namespace(),
            store.names.resolve(name.text()),
            render_type(store, *info, binders, active)
        ),
        Type::Recursive { parent } => {
            let label = binders.len();
            binders.insert(ty, label);
            let parent = render_type(store, *parent, binders, active);
            binders.remove(&ty);
            format!("Recursive#{label}({parent})")
        }
        Type::RecThis { binder } => match binders.get(binder) {
            Some(label) => format!("RecThis#{label}"),
            None => "RecThis#unbound".to_owned(),
        },
        Type::Method(method) => {
            let label = binders.len();
            binders.insert(ty, label);
            let params = method
                .params
                .iter()
                .map(|param| {
                    format!(
                        "{}:{}:erased={}:varargs={}",
                        store.names.resolve(param.name.as_name().text()),
                        render_type(store, param.ty, binders, active),
                        param.erased,
                        param.varargs
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            let result = render_type(store, method.result, binders, active);
            binders.remove(&ty);
            format!("Method#{label}({:?}; [{params}]) -> {result}", method.kind)
        }
        Type::Poly(poly) => render_type_params(
            store,
            ty,
            &poly.params,
            poly.result,
            binders,
            active,
            "Poly",
        ),
        Type::TypeLambda(lambda) => render_type_params(
            store,
            ty,
            &lambda.params,
            lambda.result,
            binders,
            active,
            "Lambda",
        ),
        Type::ParamRef { binder, index } => match binders.get(binder) {
            Some(label) => format!("ParamRef#{label}:{index}"),
            None => format!("ParamRef#unbound:{index}"),
        },
        Type::Match(matching) => format!(
            "Match({}, {}, [{}])",
            render_type(store, matching.bound, binders, active),
            render_type(store, matching.scrutinee, binders, active),
            matching
                .cases
                .iter()
                .map(|case| render_type(store, *case, binders, active))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Type::MatchCase { pattern, result } => format!(
            "Case({}, {})",
            render_type(store, *pattern, binders, active),
            render_type(store, *result, binders, active)
        ),
        Type::Annotated {
            underlying,
            annotation,
        } => {
            let annotation_type = store.annotations.get(*annotation).ty;
            format!(
                "Annotated({}, {})",
                render_type(store, *underlying, binders, active),
                render_type(store, annotation_type, binders, active)
            )
        }
        Type::Wildcard { bounds } => {
            format!("Wildcard({})", render_type(store, *bounds, binders, active))
        }
        Type::JavaArray { element } => format!(
            "JavaArray({})",
            render_type(store, *element, binders, active)
        ),
        Type::ClassInfo(info) => normalized_class_info(store, info, binders, active),
    };
    active.pop();
    result
}

fn render_type_params(
    store: &SemanticStore,
    binder: TypeId,
    params: &[dotty_core::types::TypeParam],
    result: TypeId,
    binders: &mut HashMap<TypeId, usize>,
    active: &mut Vec<TypeId>,
    kind: &str,
) -> String {
    let label = binders.len();
    binders.insert(binder, label);
    let params = params
        .iter()
        .map(|param| {
            format!(
                "{}:{:?}:{}",
                store.names.resolve(param.name.as_name().text()),
                param.declared_variance,
                render_type(store, param.bounds, binders, active)
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    let result = render_type(store, result, binders, active);
    binders.remove(&binder);
    format!("{kind}#{label}([{params}]) -> {result}")
}

fn normalized_class_info(
    store: &SemanticStore,
    info: &ClassInfo,
    binders: &mut HashMap<TypeId, usize>,
    active: &mut Vec<TypeId>,
) -> String {
    let parents = info
        .parents
        .iter()
        .map(|parent| render_type(store, *parent, binders, active))
        .collect::<Vec<_>>()
        .join(", ");
    let mut declarations = store
        .scopes
        .get(info.declarations)
        .entered_symbols()
        .map(|symbol| {
            let member_info = match *store.symbols.info(symbol) {
                SymbolInfo::Missing => "Missing".to_owned(),
                SymbolInfo::Deferred(_) => "Deferred".to_owned(),
                SymbolInfo::Error => "Error".to_owned(),
                SymbolInfo::Complete(ty) => render_type(store, ty, binders, active),
            };
            format!("{}={member_info}", symbol_key(store, symbol))
        })
        .collect::<Vec<_>>();
    declarations.sort();
    format!(
        "ClassInfo(class={}, prefix={}, parents=[{}], members=[{}], self={})",
        symbol_key(store, info.class),
        render_type(store, info.prefix, binders, active),
        parents,
        declarations.join(", "),
        info.self_type
            .map(|ty| render_type(store, ty, binders, active))
            .unwrap_or_else(|| "None".to_owned())
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use dotty_core::names::TypeName;
    use dotty_core::types::{PolyType, TypeParam};

    fn enter_common_tasty_classes(store: &mut SemanticStore, packages: &mut Packages) {
        for (path, names) in [
            (&["scala"][..], &["Any", "Nothing"][..]),
            (&["java", "lang"][..], &["Object"][..]),
        ] {
            let package = packages
                .enter(store, SymbolOrigin::Synthetic, path)
                .pop()
                .unwrap();
            for name in names {
                let name = Name::new(store.names.intern(name), Namespace::Type);
                let symbol = store.symbols.alloc(Symbol {
                    name,
                    owner: Some(package.symbol),
                    kind: SymbolKind::Class,
                    flags: SymbolFlags::EMPTY,
                    visibility: Visibility::Public,
                    info: SymbolInfo::Missing,
                    origin: SymbolOrigin::Synthetic,
                    annotations: Vec::new(),
                    position: None,
                    links: SymbolLinks::default(),
                });
                store.scopes.get_mut(package.scope).enter(name, symbol);
            }
        }
    }

    fn source_type_info(name: &str, keyword: &str, kind: SymbolKind) -> (SemanticStore, TypeId) {
        let source = SourceId::from_index(281);
        let declaration = CLASS_INFOS_SOURCE
            .lines()
            .find(|line| {
                let declaration = line.trim_start();
                declaration.starts_with(&format!("{keyword} {name}["))
                    || declaration.trim_end_matches(':') == format!("{keyword} {name}")
            })
            .unwrap_or_else(|| panic!("source declaration `{name}` is missing from fixture"));
        let source_text = format!(
            "package me.cytrowski.tastyfixtures.semantic\n{}",
            declaration.trim()
        );
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let mut packages = Packages::new();
        let scanner = ContextualScanner::new(&source_text).unwrap();
        let parsed = parse_compilation_unit(
            SourceText::new(&source_text).unwrap(),
            source,
            scanner,
            &mut store.names,
        );
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        let index = name_compilation_unit(
            &parsed.ast,
            parsed.root,
            source,
            "ClassInfos.scala",
            &mut store,
            &mut packages,
        )
        .unwrap();
        let symbol = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::TypeDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == name
                        && index
                            .symbol_at(source, tree)
                            .is_some_and(|symbol| store.symbols.get(symbol).kind == kind)
                        && index
                            .definition_of(index.symbol_at(source, tree)?)
                            .is_some() =>
                {
                    index.symbol_at(source, tree)
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("source type `{name}` was not named"));
        let completed = {
            let mut typer = SourceTyper::new(
                &parsed.ast,
                source,
                &index,
                &mut store,
                definitions,
                &packages,
            );
            typer.complete_symbol(symbol).unwrap()
        };
        (store, completed)
    }

    fn tasty_type_info(name: &str, bytes: &[u8], kind: SymbolKind) -> (SemanticStore, TypeId) {
        let file = TastyFile::parse_scala_3_9(bytes).unwrap();
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let mut packages = Packages::new();
        enter_common_tasty_classes(&mut store, &mut packages);
        let mut unpickler = TastyUnpickler::with_packages(&file, &mut store, definitions, packages);
        unpickler.enter_symbols().unwrap();
        let ast = file.ast_address_index().unwrap();
        let address = ast
            .iter_nodes()
            .find_map(|node| {
                let address = u32::try_from(node.offset).ok()?;
                let raw = ast.get(address)?;
                let StructuredNode::TypeDef(DefinitionBody::TypeDef {
                    name: type_name, ..
                }) = raw.decode_structured().ok()?
                else {
                    return None;
                };
                (file.names().get_utf8(type_name) == Some(name)
                    && unpickler
                        .symbol_state_at(address)
                        .is_some_and(|(entered_kind, _)| entered_kind == kind))
                .then_some(address)
            })
            .unwrap_or_else(|| panic!("TASTy type `{name}` was not entered"));
        let completed = unpickler.complete_symbol(address).unwrap();
        (store, completed)
    }

    #[test]
    fn source_and_scala_390_tasty_class_info_have_the_same_structure() {
        let (source_store, source_info) =
            source_type_info("InfoParent", "class", SymbolKind::Class);
        let (tasty_store, tasty_info) =
            tasty_type_info("InfoParent", INFO_PARENT_TASTY, SymbolKind::Class);

        assert_eq!(
            normalized_type(&source_store, source_info),
            normalized_type(&tasty_store, tasty_info),
            "semantic mismatch for me.cytrowski.tastyfixtures.semantic.InfoParent"
        );
    }

    #[test]
    fn source_and_scala_390_tasty_trait_info_have_the_same_structure() {
        let (source_store, source_info) = source_type_info("InfoBase", "trait", SymbolKind::Trait);
        let (tasty_store, tasty_info) =
            tasty_type_info("InfoBase", INFO_BASE_TASTY, SymbolKind::Trait);

        assert_eq!(
            normalized_type(&source_store, source_info),
            normalized_type(&tasty_store, tasty_info),
            "semantic mismatch for me.cytrowski.tastyfixtures.semantic.InfoBase"
        );
    }

    fn class_info_fixture(padding: usize, parent_name: &str) -> (SemanticStore, TypeId) {
        let mut store = SemanticStore::new();
        for index in 0..padding {
            store.types.alloc(Type::NoType);
            store.scopes.alloc(Scope::new(None));
            let name = store.names.intern(&format!("unused-{index}"));
            store.symbols.alloc(Symbol {
                name: Name::new(name, Namespace::Term),
                owner: None,
                kind: SymbolKind::Value,
                flags: SymbolFlags::EMPTY,
                visibility: Visibility::Public,
                info: SymbolInfo::Missing,
                origin: SymbolOrigin::Synthetic,
                annotations: Vec::new(),
                position: None,
                links: SymbolLinks::default(),
            });
        }
        let definitions = Definitions::bootstrap(&mut store);
        let root = store.names.intern("");
        let root_symbol = store.symbols.alloc(Symbol {
            name: Name::new(root, Namespace::Term),
            owner: None,
            kind: SymbolKind::Package,
            flags: SymbolFlags::EMPTY,
            visibility: Visibility::Public,
            info: SymbolInfo::Missing,
            origin: SymbolOrigin::Synthetic,
            annotations: Vec::new(),
            position: None,
            links: SymbolLinks::default(),
        });
        let class_name = store.names.intern("C");
        let class = store.symbols.alloc(Symbol {
            name: Name::new(class_name, Namespace::Type),
            owner: Some(root_symbol),
            kind: SymbolKind::Class,
            flags: SymbolFlags::EMPTY,
            visibility: Visibility::Public,
            info: SymbolInfo::Missing,
            origin: SymbolOrigin::Synthetic,
            annotations: Vec::new(),
            position: None,
            links: SymbolLinks::default(),
        });
        let parent_name = store.names.intern(parent_name);
        let parent = store.symbols.alloc(Symbol {
            name: Name::new(parent_name, Namespace::Type),
            owner: Some(root_symbol),
            kind: SymbolKind::Class,
            flags: SymbolFlags::EMPTY,
            visibility: Visibility::Public,
            info: SymbolInfo::Missing,
            origin: SymbolOrigin::Synthetic,
            annotations: Vec::new(),
            position: None,
            links: SymbolLinks::default(),
        });
        let scope = store.scopes.alloc(Scope::new(Some(class)));
        let parent_type = store.types.alloc(Type::TypeRef {
            prefix: definitions.no_prefix,
            target: TypeRefTarget::Symbol(parent),
        });
        let info = store.types.alloc(Type::ClassInfo(ClassInfo {
            prefix: definitions.no_prefix,
            class,
            parents: vec![parent_type],
            declarations: scope,
            self_type: None,
        }));
        (store, info)
    }

    #[test]
    fn class_info_normalization_ignores_arena_ids_and_scope_ids() {
        let (left_store, left) = class_info_fixture(0, "Base");
        let (right_store, right) = class_info_fixture(7, "Base");

        assert_ne!(left, right);
        assert_eq!(
            normalized_type(&left_store, left),
            normalized_type(&right_store, right)
        );
    }

    #[test]
    fn class_info_normalization_detects_a_changed_parent() {
        let (left_store, left) = class_info_fixture(0, "Base");
        let (right_store, right) = class_info_fixture(3, "OtherBase");

        assert_ne!(
            normalized_type(&left_store, left),
            normalized_type(&right_store, right)
        );
    }

    fn poly_fixture(padding: usize, index: u32) -> (SemanticStore, TypeId) {
        let mut store = SemanticStore::new();
        for _ in 0..padding {
            store.types.alloc(Type::NoType);
        }
        let binder = store.types.reserve();
        let reference = store.types.alloc(Type::ParamRef {
            binder: binder.id(),
            index,
        });
        let low = store.types.alloc(Type::NoType);
        let high = store.types.alloc(Type::NoType);
        let bounds = store.types.alloc(Type::Bounds { low, high });
        let name = store.names.intern("A");
        let ty = store.types.fill(
            binder,
            Type::Poly(PolyType {
                params: vec![TypeParam {
                    name: TypeName::new(name),
                    bounds,
                    declared_variance: None,
                }],
                result: reference,
            }),
        );
        (store, ty)
    }

    #[test]
    fn polymorphic_type_normalization_uses_local_binder_labels() {
        let (left_store, left) = poly_fixture(0, 0);
        let (right_store, right) = poly_fixture(9, 0);

        assert_ne!(left, right);
        assert_eq!(
            normalized_type(&left_store, left),
            normalized_type(&right_store, right)
        );
    }

    #[test]
    fn polymorphic_type_normalization_detects_a_changed_parameter_reference() {
        let (left_store, left) = poly_fixture(0, 0);
        let (right_store, right) = poly_fixture(2, 1);

        assert_ne!(
            normalized_type(&left_store, left),
            normalized_type(&right_store, right)
        );
    }
}
