//! ID-independent semantic normalization shared by source/TASTy parity tests.

use std::collections::HashMap;

use dotty_core::ast::{TreeKind, UntypedNode};
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
const INFO_DEP_TASTY: &[u8] =
    include_bytes!("../../dotty-tasty-unpickler/tests/fixtures/semantic/InfoDep.tasty");
const INFO_CHILD_TASTY: &[u8] =
    include_bytes!("../../dotty-tasty-unpickler/tests/fixtures/semantic/InfoChild.tasty");
const INFO_HOLDER_TASTY: &[u8] =
    include_bytes!("../../dotty-tasty-unpickler/tests/fixtures/semantic/InfoHolder.tasty");
const METHODS_SOURCE: &str =
    include_str!("../../dotty-tasty-unpickler/tests/fixtures/semantic/Methods.scala");
const METHODS_TASTY: &[u8] =
    include_bytes!("../../dotty-tasty-unpickler/tests/fixtures/semantic/Methods.tasty");
const CONSTRUCTORS_SOURCE: &str =
    include_str!("../../dotty-tasty-unpickler/tests/fixtures/semantic/Constructors.scala");
const CTOR_PLAIN_TASTY: &[u8] =
    include_bytes!("../../dotty-tasty-unpickler/tests/fixtures/semantic/CtorPlain.tasty");

fn symbol_key(store: &SemanticStore, symbol: SymbolId) -> String {
    let entry = store.symbols.get(symbol);
    let name = store.names.resolve(entry.name.text());
    if entry.origin == SymbolOrigin::Builtin {
        let owner = match name {
            "Any" | "Nothing" | "Boolean" | "Byte" | "Char" | "Double" | "Float" | "Int"
            | "Long" | "Short" | "Unit" => "Package:scala",
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

fn normalized_visibility(store: &SemanticStore, visibility: Visibility) -> String {
    match visibility {
        Visibility::Public => "Public".to_owned(),
        Visibility::Private => "Private".to_owned(),
        Visibility::Protected => "Protected".to_owned(),
        Visibility::Package(symbol) => format!("Package({})", symbol_key(store, symbol)),
        Visibility::PrivateWithin(symbol) => {
            format!("PrivateWithin({})", symbol_key(store, symbol))
        }
        Visibility::ProtectedWithin(symbol) => {
            format!("ProtectedWithin({})", symbol_key(store, symbol))
        }
    }
}

fn normalized_symbol_header(store: &SemanticStore, symbol: SymbolId) -> String {
    let entry = store.symbols.get(symbol);
    let companion = entry
        .links
        .companion
        .map(|companion| symbol_key(store, companion))
        .unwrap_or_else(|| "None".to_owned());
    format!(
        "{}[flags={:?}, visibility={}, companion={companion}]",
        symbol_key(store, symbol),
        entry.flags,
        normalized_visibility(store, entry.visibility)
    )
}

fn package_prefix(store: &SemanticStore, ty: TypeId) -> Option<String> {
    let package = match store.types.get(ty) {
        Type::TypeRef { target, .. } => target.symbol(),
        dotty_core::types::Type::TermRef { target, .. } => target.symbol(),
        Type::ThisType { class } => Some(*class),
        _ => None,
    }?;
    (store.symbols.get(package).kind == SymbolKind::Package).then(|| symbol_key(store, package))
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
            let prefix = package_prefix(store, *prefix).map_or_else(
                || render_type(store, *prefix, binders, active),
                |package| format!("PackagePrefix({package})"),
            );
            format!("TermRef({prefix}, {target})")
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
                package_prefix(store, *prefix).map_or_else(
                    || render_type(store, *prefix, binders, active),
                    |package| format!("PackagePrefix({package})"),
                )
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
        .filter(|symbol| {
            let entry = store.symbols.get(*symbol);
            !(entry.kind == SymbolKind::Method
                && store.names.resolve(entry.name.text()) == "writeReplace"
                && entry.flags.contains(SymbolFlags::SYNTHETIC))
        })
        .map(|symbol| {
            let member_info = match *store.symbols.info(symbol) {
                SymbolInfo::Missing => "Missing".to_owned(),
                SymbolInfo::Deferred(_) => "Deferred".to_owned(),
                SymbolInfo::Error => "Error".to_owned(),
                SymbolInfo::Complete(ty) => render_type(store, ty, binders, active),
            };
            format!("{}={member_info}", normalized_symbol_header(store, symbol))
        })
        .collect::<Vec<_>>();
    declarations.sort();
    let self_type = match info.self_type {
        Some(ty) if is_default_module_self(store, info.class, ty) => "ImplicitModuleSelf".into(),
        Some(ty) => render_type(store, ty, binders, active),
        None if store.symbols.get(info.class).kind == SymbolKind::ModuleClass => {
            "ImplicitModuleSelf".into()
        }
        None => "None".into(),
    };
    format!(
        "ClassInfo(class={}, prefix={}, parents=[{}], members=[{}], self={})",
        normalized_symbol_header(store, info.class),
        render_type(store, info.prefix, binders, active),
        parents,
        declarations.join(", "),
        self_type
    )
}

fn is_default_module_self(store: &SemanticStore, class: SymbolId, ty: TypeId) -> bool {
    let class_entry = store.symbols.get(class);
    if class_entry.kind != SymbolKind::ModuleClass {
        return false;
    }
    let expected_name = store
        .names
        .resolve(class_entry.name.text())
        .trim_end_matches('$')
        .to_owned();
    let Type::TermRef {
        target: dotty_core::types::TermRefTarget::Symbol(object),
        ..
    } = store.types.get(ty)
    else {
        return false;
    };
    let object_entry = store.symbols.get(*object);
    object_entry.kind == SymbolKind::Object
        && object_entry.owner == class_entry.owner
        && store.names.resolve(object_entry.name.text()) == expected_name
}

fn normalized_member_infos(store: &SemanticStore, info: &ClassInfo, name: &str) -> Vec<String> {
    let mut members = store
        .scopes
        .get(info.declarations)
        .entered_symbols()
        .filter(|symbol| store.names.resolve(store.symbols.get(*symbol).name.text()) == name)
        .map(|symbol| {
            let info = match *store.symbols.info(symbol) {
                SymbolInfo::Complete(ty) => normalized_type(store, ty),
                SymbolInfo::Missing => "Missing".to_owned(),
                SymbolInfo::Deferred(_) => "Deferred".to_owned(),
                SymbolInfo::Error => "Error".to_owned(),
            };
            format!("{}={info}", normalized_symbol_header(store, symbol))
        })
        .collect::<Vec<_>>();
    members.sort();
    members
}

fn assert_semantic_parity(path: &str, source: &str, tasty: &str) {
    if source == tasty {
        return;
    }

    let source_chars = source.chars().collect::<Vec<_>>();
    let tasty_chars = tasty.chars().collect::<Vec<_>>();
    let first_difference = source_chars
        .iter()
        .zip(&tasty_chars)
        .position(|(source, tasty)| source != tasty)
        .unwrap_or(source_chars.len().min(tasty_chars.len()));
    let component = |snapshot: &[char]| {
        let is_boundary = |ch: char| matches!(ch, ',' | '[' | ']' | '(' | ')');
        let start = snapshot[..first_difference.min(snapshot.len())]
            .iter()
            .rposition(|ch| is_boundary(*ch))
            .map_or(0, |index| index + 1);
        let end = snapshot[first_difference.min(snapshot.len())..]
            .iter()
            .position(|ch| is_boundary(*ch))
            .map_or(snapshot.len(), |index| {
                first_difference.min(snapshot.len()) + index
            });
        snapshot[start..end].iter().collect::<String>()
    };

    panic!(
        "semantic mismatch at {path}; first differing component near character {first_difference}:\n  source: {:?}\n  TASTy:  {:?}",
        component(&source_chars),
        component(&tasty_chars),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use dotty_core::names::TypeName;
    use dotty_core::types::{PolyType, TypeParam};

    fn enter_common_classes(
        store: &mut SemanticStore,
        packages: &mut Packages,
        include_java_comparable: bool,
    ) {
        let java_lang_types: &[&str] = if include_java_comparable {
            &["Object", "Comparable"]
        } else {
            &["Object"]
        };
        for (path, names) in [
            (
                &["scala"][..],
                &[
                    "Any", "Nothing", "Boolean", "Byte", "Char", "Double", "Float", "Int", "Long",
                    "Short", "Unit",
                ][..],
            ),
            (&["java", "lang"][..], java_lang_types),
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
        let declaration = CLASS_INFOS_SOURCE
            .lines()
            .find(|line| {
                let declaration = line.trim_start();
                declaration.starts_with(&format!("{keyword} {name}["))
                    || declaration.trim_end_matches(':') == format!("{keyword} {name}")
            })
            .unwrap_or_else(|| panic!("source declaration `{name}` is missing from fixture"));
        source_type_info_from_source(
            &format!(
                "package me.cytrowski.tastyfixtures.semantic\n{}",
                declaration.trim()
            ),
            name,
            kind,
            &[],
        )
    }

    fn source_type_info_from_source(
        source_text: &str,
        name: &str,
        kind: SymbolKind,
        complete_members: &[&str],
    ) -> (SemanticStore, TypeId) {
        let source = SourceId::from_index(281);
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let mut packages = Packages::new();
        enter_common_classes(&mut store, &mut packages, false);
        let scanner = ContextualScanner::new(source_text).unwrap();
        let parsed = parse_compilation_unit(
            SourceText::new(source_text).unwrap(),
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
            .find_map(|(tree, node)| {
                let candidate = match &node.kind {
                    TreeKind::TypeDef(definition)
                        if store.names.resolve(definition.name.as_name().text()) == name =>
                    {
                        index.symbol_at(source, tree)
                    }
                    TreeKind::PhaseSpecific(UntypedNode::ModuleDef(definition))
                        if kind == SymbolKind::ModuleClass
                            && store.names.resolve(definition.name.as_name().text()) == name =>
                    {
                        let object = index.symbol_at(source, tree)?;
                        let owner = store.symbols.get(object).owner?;
                        index.derived_symbol_at(owner, source, tree)
                    }
                    _ => None,
                }?;
                (store.symbols.get(candidate).kind == kind
                    && index.definition_of(candidate).is_some())
                .then_some(candidate)
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
            let completed = typer.complete_symbol(symbol).unwrap();
            for member_name in complete_members {
                let member_symbols = parsed
                    .ast
                    .iter()
                    .filter_map(|(tree, node)| match node.kind {
                        TreeKind::PhaseSpecific(UntypedNode::ModuleDef(_)) => {
                            index.derived_symbol_at(symbol, source, tree)
                        }
                        _ => index.symbol_at(source, tree),
                    })
                    .filter(|member| {
                        let member = typer.store().symbols.get(*member);
                        member.owner == Some(symbol)
                            && (typer.store().names.resolve(member.name.text()) == *member_name
                                || typer
                                    .store()
                                    .names
                                    .resolve(member.name.text())
                                    .trim_end_matches('$')
                                    == *member_name)
                            && member.kind != SymbolKind::Object
                    })
                    .collect::<Vec<_>>();
                assert!(
                    !member_symbols.is_empty(),
                    "source member `{member_name}` was not named"
                );
                for member in member_symbols {
                    typer.complete_symbol(member).unwrap();
                }
            }
            completed
        };
        (store, completed)
    }

    fn tasty_type_info(
        name: &str,
        bytes: &'static [u8],
        kind: SymbolKind,
    ) -> (SemanticStore, TypeId) {
        tasty_type_info_with_units(name, &[bytes], kind, &[])
    }

    fn tasty_type_info_with_units(
        name: &str,
        fixture_bytes: &[&'static [u8]],
        kind: SymbolKind,
        complete_members: &[&str],
    ) -> (SemanticStore, TypeId) {
        let files = fixture_bytes
            .iter()
            .map(|bytes| TastyFile::parse_scala_3_9(bytes).unwrap())
            .collect::<Vec<_>>();
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let mut packages = Packages::new();
        enter_common_classes(&mut store, &mut packages, true);
        for file in &files {
            let mut unpickler =
                TastyUnpickler::with_packages(file, &mut store, definitions, packages);
            unpickler.enter_symbols().unwrap();
            let ast = file.ast_address_index().unwrap();
            let candidates = ast
                .iter_nodes()
                .filter_map(|node| {
                    let address = u32::try_from(node.offset).ok()?;
                    let raw = ast.get(address)?;
                    let StructuredNode::TypeDef(DefinitionBody::TypeDef {
                        name: type_name, ..
                    }) = raw.decode_structured().ok()?
                    else {
                        return None;
                    };
                    let entered_kind = unpickler.symbol_state_at(address)?.0;
                    (entered_kind == kind).then_some((address, file.names().get_utf8(type_name)))
                })
                .collect::<Vec<_>>();
            if kind == SymbolKind::ModuleClass {
                let completed = candidates
                    .iter()
                    .map(|(address, _)| (*address, unpickler.complete_symbol(*address).unwrap()))
                    .collect::<Vec<_>>();
                let (index, next_packages) = unpickler.into_parts();
                let selected = completed.into_iter().find(|(address, _)| {
                    index.symbol_at(*address).is_some_and(|symbol| {
                        let symbol_name =
                            store.names.resolve(store.symbols.get(symbol).name.text());
                        symbol_name == name || symbol_name.trim_end_matches('$') == name
                    })
                });
                if let Some((_, completed)) = selected {
                    return (store, completed);
                }
                packages = next_packages;
                continue;
            }
            if let Some((address, _)) = candidates
                .iter()
                .find(|(_, node_name)| *node_name == Some(name))
            {
                let completed = unpickler.complete_symbol(*address).unwrap();
                for member_name in complete_members {
                    let member_addresses = ast
                        .iter_nodes()
                        .filter_map(|node| {
                            let member_address = u32::try_from(node.offset).ok()?;
                            let raw = ast.get(member_address)?;
                            let name_ref = match raw.decode_structured().ok()? {
                                StructuredNode::ValDef(DefinitionBody::ValDef { name, .. })
                                | StructuredNode::TypeDef(DefinitionBody::TypeDef {
                                    name, ..
                                }) => name,
                                StructuredNode::DefDef(definition) => definition.name,
                                StructuredNode::Parameter(
                                    dotty_tasty::tasty::ParameterNode::TermParam { name, .. },
                                ) => name,
                                _ => return None,
                            };
                            (file.names().get_utf8(name_ref) == Some(*member_name))
                                .then_some(member_address)
                        })
                        .collect::<Vec<_>>();
                    assert!(
                        !member_addresses.is_empty(),
                        "TASTy member `{member_name}` was not entered"
                    );
                    for member_address in member_addresses {
                        unpickler.complete_symbol(member_address).unwrap();
                    }
                }
                return (store, completed);
            }
            packages = unpickler.into_parts().1;
        }
        panic!("TASTy type `{name}` was not entered")
    }

    fn class_infos_definition(keyword: &str, name: &str) -> String {
        let lines = CLASS_INFOS_SOURCE.lines().collect::<Vec<_>>();
        let start = lines
            .iter()
            .position(|line| line.trim_start().starts_with(&format!("{keyword} {name}")))
            .unwrap_or_else(|| panic!("fixture declaration `{name}` is missing"));
        let mut definition = Vec::new();
        for (offset, line) in lines[start..].iter().enumerate() {
            if offset > 0 && !line.trim().is_empty() && !line.starts_with(char::is_whitespace) {
                break;
            }
            definition.push(*line);
        }
        definition.join("\n").trim_end().to_owned()
    }

    fn methods_fixture_method(name: &str) -> String {
        METHODS_SOURCE
            .lines()
            .find(|line| line.trim_start().starts_with(&format!("def {name}")))
            .unwrap_or_else(|| panic!("method `{name}` is missing from source fixture"))
            .trim()
            .to_owned()
    }

    fn assert_source_and_tasty_method_parity(name: &str, source_text: &str, method_name: &str) {
        let (source_store, source_info) =
            source_type_info_from_source(source_text, "Methods", SymbolKind::Class, &[method_name]);
        let (tasty_store, tasty_info) = tasty_type_info_with_units(
            "Methods",
            &[METHODS_TASTY],
            SymbolKind::Class,
            &[method_name],
        );
        let source_members = normalized_member_infos(
            &source_store,
            match source_store.types.get(source_info) {
                Type::ClassInfo(info) => info,
                other => panic!("expected source ClassInfo, got {other:?}"),
            },
            method_name,
        );
        let tasty_members = normalized_member_infos(
            &tasty_store,
            match tasty_store.types.get(tasty_info) {
                Type::ClassInfo(info) => info,
                other => panic!("expected TASTy ClassInfo, got {other:?}"),
            },
            method_name,
        );
        assert_semantic_parity(
            &format!("me.cytrowski.tastyfixtures.semantic.Methods.{name}"),
            &source_members.join("\n"),
            &tasty_members.join("\n"),
        );
    }

    #[test]
    fn source_and_scala_390_tasty_class_info_have_the_same_structure() {
        let (source_store, source_info) =
            source_type_info("InfoParent", "class", SymbolKind::Class);
        let (tasty_store, tasty_info) =
            tasty_type_info("InfoParent", INFO_PARENT_TASTY, SymbolKind::Class);

        assert_semantic_parity(
            "me.cytrowski.tastyfixtures.semantic.InfoParent",
            &normalized_type(&source_store, source_info),
            &normalized_type(&tasty_store, tasty_info),
        );
    }

    #[test]
    fn source_and_scala_390_tasty_trait_info_have_the_same_structure() {
        let (source_store, source_info) = source_type_info("InfoBase", "trait", SymbolKind::Trait);
        let (tasty_store, tasty_info) =
            tasty_type_info("InfoBase", INFO_BASE_TASTY, SymbolKind::Trait);

        assert_semantic_parity(
            "me.cytrowski.tastyfixtures.semantic.InfoBase",
            &normalized_type(&source_store, source_info),
            &normalized_type(&tasty_store, tasty_info),
        );
    }

    #[test]
    fn source_and_scala_390_tasty_info_child_preserve_parents_and_self_type() {
        let source_text = format!(
            "package me.cytrowski.tastyfixtures.semantic\n{}\n{}\n{}\n{}",
            class_infos_definition("trait", "InfoBase"),
            class_infos_definition("trait", "InfoDep"),
            class_infos_definition("class", "InfoParent"),
            class_infos_definition("class", "InfoChild"),
        );
        let (source_store, source_info) = source_type_info_from_source(
            &source_text,
            "InfoChild",
            SymbolKind::Class,
            &["value", "Member", "method"],
        );
        let (tasty_store, tasty_info) = tasty_type_info_with_units(
            "InfoChild",
            &[
                INFO_PARENT_TASTY,
                INFO_BASE_TASTY,
                INFO_DEP_TASTY,
                INFO_CHILD_TASTY,
            ],
            SymbolKind::Class,
            &["value", "Member", "method"],
        );

        assert_semantic_parity(
            "me.cytrowski.tastyfixtures.semantic.InfoChild",
            &normalized_type(&source_store, source_info),
            &normalized_type(&tasty_store, tasty_info),
        );
    }

    #[test]
    fn source_and_scala_390_tasty_nested_class_info_match() {
        let source_text = "package me.cytrowski.tastyfixtures.semantic\nclass InfoParent[A]\nobject InfoHolder:\n  class Nested extends InfoParent[Int]";
        let (source_store, source_info) =
            source_type_info_from_source(source_text, "Nested", SymbolKind::Class, &[]);
        let (tasty_store, tasty_info) = tasty_type_info_with_units(
            "Nested",
            &[INFO_PARENT_TASTY, INFO_HOLDER_TASTY],
            SymbolKind::Class,
            &[],
        );

        assert_semantic_parity(
            "me.cytrowski.tastyfixtures.semantic.InfoHolder.Nested",
            &normalized_type(&source_store, source_info),
            &normalized_type(&tasty_store, tasty_info),
        );
    }

    #[test]
    fn source_and_scala_390_tasty_module_class_info_match() {
        let source_text = "package me.cytrowski.tastyfixtures.semantic\nobject InfoHolder:\n  class Nested extends InfoParent[Int]\n  object Inner:\n    val deep: Int = 0";
        let (source_store, source_info) = source_type_info_from_source(
            source_text,
            "InfoHolder",
            SymbolKind::ModuleClass,
            &["Inner"],
        );
        let (tasty_store, tasty_info) = tasty_type_info_with_units(
            "InfoHolder",
            &[INFO_PARENT_TASTY, INFO_HOLDER_TASTY],
            SymbolKind::ModuleClass,
            &[],
        );

        assert_semantic_parity(
            "me.cytrowski.tastyfixtures.semantic.InfoHolder$",
            &normalized_type(&source_store, source_info),
            &normalized_type(&tasty_store, tasty_info),
        );
    }

    #[test]
    fn source_and_scala_390_tasty_abstract_type_member_info_match() {
        let source_text = "package me.cytrowski.tastyfixtures.semantic\nclass Methods:\n  trait Box:\n    type Out";
        let (source_store, source_info) =
            source_type_info_from_source(source_text, "Box", SymbolKind::Trait, &["Out"]);
        let (tasty_store, tasty_info) =
            tasty_type_info_with_units("Box", &[METHODS_TASTY], SymbolKind::Trait, &["Out"]);

        assert_semantic_parity(
            "me.cytrowski.tastyfixtures.semantic.Methods.Box",
            &normalized_type(&source_store, source_info),
            &normalized_type(&tasty_store, tasty_info),
        );
    }

    #[test]
    fn source_and_scala_390_tasty_constructor_class_info_match() {
        let source_text = format!(
            "package me.cytrowski.tastyfixtures.semantic\n{}",
            CONSTRUCTORS_SOURCE
                .lines()
                .find(|line| line.starts_with("class CtorPlain"))
                .unwrap()
        );
        let (source_store, source_info) =
            source_type_info_from_source(&source_text, "CtorPlain", SymbolKind::Class, &["<init>"]);
        let (tasty_store, tasty_info) = tasty_type_info_with_units(
            "CtorPlain",
            &[CTOR_PLAIN_TASTY],
            SymbolKind::Class,
            &["<init>"],
        );

        assert_semantic_parity(
            "me.cytrowski.tastyfixtures.semantic.CtorPlain",
            &normalized_type(&source_store, source_info),
            &normalized_type(&tasty_store, tasty_info),
        );
    }

    #[test]
    fn source_and_scala_390_tasty_curried_method_signatures_match() {
        let source_text = format!(
            "package me.cytrowski.tastyfixtures.semantic\nclass Methods:\n  {}",
            methods_fixture_method("curried")
        );
        assert_source_and_tasty_method_parity("curried", &source_text, "curried");
    }

    #[test]
    fn source_and_scala_390_tasty_polymorphic_method_signatures_match() {
        let source_text = format!(
            "package me.cytrowski.tastyfixtures.semantic\nclass Methods:\n  {}",
            methods_fixture_method("polymorphic")
        );
        assert_source_and_tasty_method_parity("polymorphic", &source_text, "polymorphic");
    }

    #[test]
    fn source_and_scala_390_tasty_contextual_clause_kind_matches() {
        let method =
            methods_fixture_method("contextual").replace("Comparable", "java.lang.Comparable");
        let source_text = format!(
            "package java.lang {{ class Comparable[A] }}\npackage me.cytrowski.tastyfixtures.semantic {{ class Methods {{ {method} }} }}"
        );
        assert_source_and_tasty_method_parity("contextual", &source_text, "contextual");
    }

    #[test]
    fn source_and_scala_390_tasty_overloads_keep_distinct_signatures() {
        let overloads = METHODS_SOURCE
            .lines()
            .filter(|line| line.trim_start().starts_with("def overloaded("))
            .map(str::trim)
            .collect::<Vec<_>>()
            .join("\n  ");
        let source_text =
            format!("package me.cytrowski.tastyfixtures.semantic\nclass Methods:\n  {overloads}");
        assert_source_and_tasty_method_parity("overloaded", &source_text, "overloaded");
    }

    #[test]
    fn source_and_scala_390_tasty_empty_clause_is_preserved() {
        let source_text = format!(
            "package me.cytrowski.tastyfixtures.semantic\nclass Methods:\n  {}",
            methods_fixture_method("empty")
        );
        assert_source_and_tasty_method_parity("empty", &source_text, "empty");
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

    #[test]
    #[should_panic(expected = "first differing component")]
    fn parity_diagnostic_identifies_the_changed_nested_component() {
        assert_semantic_parity(
            "pkg.Owner.method",
            "Method([Int]) -> String",
            "Method([Long]) -> String",
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
