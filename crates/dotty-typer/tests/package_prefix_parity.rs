use dotty_classloader::classloader::{
    BinaryName, ClassFormat, ClassOrigin, ClassPathEntry, ClassPathError, ClassResource,
    ClasspathSymbolResolver, LoadingSession,
};
use dotty_core::ast::TreeKind;
use dotty_core::names::{Name, Namespace};
use dotty_core::types::{TermRefTarget, Type, TypeRefTarget};
use dotty_core::{
    Definitions, MemberRequest, Packages, ResolutionError, SemanticStore, SourceId, SourceText,
    SymbolId, SymbolKind, SymbolResolver,
};
use dotty_lexer::ContextualScanner;
use dotty_namer::name_compilation_unit;
use dotty_parser::parse_compilation_unit;
use dotty_tasty::tasty::TastyFile;
use dotty_tasty_unpickler::tasty_unpickler::TastyUnpickler;
use dotty_typer::SourceTyper;

const WIDGET: &[u8] =
    include_bytes!("../../dotty-tasty-unpickler/tests/fixtures/semantic/Widget.tasty");
const HOLDER: &[u8] =
    include_bytes!("../../dotty-tasty-unpickler/tests/fixtures/semantic/Holder.tasty");
// Compiled from Local.scala by the repository-pinned Scala 3.9.0 compiler.
const ISSUE_1173_LOCAL: &[u8] = include_bytes!("fixtures/issue-1173/a/consumer/b/Local.tasty");
const ISSUE_1173_BINARY_LOCAL: &[u8] =
    include_bytes!("fixtures/issue-1173/a/consumer/binary/Local.tasty");
const ISSUE_1174_PARENT_OBJ: &[u8] = include_bytes!("fixtures/issue-1174/a/core/Obj.tasty");
const ISSUE_1174_CURRENT_OBJ: &[u8] = include_bytes!("fixtures/issue-1174/a/ast/core/Obj.tasty");

struct Issue1173FixturePath;

impl ClassPathEntry for Issue1173FixturePath {
    fn find_class(&self, name: &BinaryName) -> Result<Option<ClassResource>, ClassPathError> {
        let (bytes, format) = match name.as_internal() {
            "a/consumer/b/Local" => (ISSUE_1173_LOCAL.to_vec(), ClassFormat::Tasty),
            "a/consumer/binary/Local" => (ISSUE_1173_BINARY_LOCAL.to_vec(), ClassFormat::Tasty),
            "java/lang/Object" => (java_object_stub(), ClassFormat::Class),
            _ => return Ok(None),
        };
        Ok(Some(ClassResource::new(
            bytes,
            format,
            ClassOrigin::Directory("<issue-1173-scala-3.9-fixture>".into()),
        )))
    }

    fn contains_package(&self, package: &[&str]) -> Result<bool, ClassPathError> {
        Ok(matches!(
            package,
            ["a", "consumer", "b"] | ["a", "consumer", "binary"]
        ))
    }
}

struct Issue1174StackedPath {
    include_current_package: bool,
}

impl ClassPathEntry for Issue1174StackedPath {
    fn find_class(&self, name: &BinaryName) -> Result<Option<ClassResource>, ClassPathError> {
        let (bytes, format) = match name.as_internal() {
            "a/core/Obj" => (ISSUE_1174_PARENT_OBJ.to_vec(), ClassFormat::Tasty),
            "a/ast/core/Obj" if self.include_current_package => {
                (ISSUE_1174_CURRENT_OBJ.to_vec(), ClassFormat::Tasty)
            }
            "java/lang/Object" => (java_object_stub(), ClassFormat::Class),
            _ => return Ok(None),
        };
        Ok(Some(ClassResource::new(
            bytes,
            format,
            ClassOrigin::Directory("<issue-1174-scala-3.9-fixture>".into()),
        )))
    }

    fn contains_package(&self, package: &[&str]) -> Result<bool, ClassPathError> {
        Ok(matches!(package, ["a", "core"])
            || (self.include_current_package && matches!(package, ["a", "ast", "core"])))
    }
}

struct DottyCoreClassPath;

impl ClassPathEntry for DottyCoreClassPath {
    fn find_class(&self, name: &BinaryName) -> Result<Option<ClassResource>, ClassPathError> {
        let bytes = match name.as_internal() {
            "dotty/tools/dotc/core/Contexts" => class_file_stub(name.as_internal()),
            "java/lang/Object" => java_object_stub(),
            _ => return Ok(None),
        };
        Ok(Some(ClassResource::new(
            bytes,
            ClassFormat::Class,
            ClassOrigin::Directory("<issue-1180-core-package-classfile-fixture>".into()),
        )))
    }

    fn contains_package(&self, package: &[&str]) -> Result<bool, ClassPathError> {
        Ok(matches!(
            package,
            ["dotty"]
                | ["dotty", "tools"]
                | ["dotty", "tools", "dotc"]
                | ["dotty", "tools", "dotc", "core"]
        ))
    }
}

struct TracedResolver<E: ClassPathEntry> {
    inner: ClasspathSymbolResolver<E>,
    package_requests: std::rc::Rc<std::cell::RefCell<Vec<(Vec<String>, Option<SymbolId>)>>>,
}

impl<E: ClassPathEntry> SymbolResolver for TracedResolver<E> {
    fn module_class_of_object(
        &self,
        store: &SemanticStore,
        object: SymbolId,
    ) -> Result<Option<SymbolId>, ResolutionError> {
        self.inner.module_class_of_object(store, object)
    }

    fn checkpoint(&self) -> dotty_core::ResolverCheckpoint {
        self.inner.checkpoint()
    }

    fn rollback_to(
        &mut self,
        store: &mut SemanticStore,
        checkpoint: dotty_core::ResolverCheckpoint,
    ) {
        self.inner.rollback_to(store, checkpoint);
    }

    fn resolve_member(
        &mut self,
        store: &mut SemanticStore,
        request: &MemberRequest,
    ) -> Result<Option<SymbolId>, ResolutionError> {
        self.inner.resolve_member(store, request)
    }

    fn resolve_package(
        &mut self,
        store: &mut SemanticStore,
        path: &[&str],
    ) -> Result<Option<SymbolId>, ResolutionError> {
        let result = self.inner.resolve_package(store, path)?;
        self.package_requests.borrow_mut().push((
            path.iter().map(|segment| (*segment).to_owned()).collect(),
            result,
        ));
        Ok(result)
    }
}

fn java_object_stub() -> Vec<u8> {
    let internal_name = b"java/lang/Object";
    let mut bytes = vec![0xCA, 0xFE, 0xBA, 0xBE, 0, 0, 0, 61, 0, 3, 1];
    bytes.extend_from_slice(&(internal_name.len() as u16).to_be_bytes());
    bytes.extend_from_slice(internal_name);
    bytes.extend_from_slice(&[7, 0, 1, 0, 0x21, 0, 2, 0, 0]);
    bytes.extend_from_slice(&[0; 8]);
    bytes
}

fn class_file_stub(internal_name: &str) -> Vec<u8> {
    let internal_name = internal_name.as_bytes();
    let object_name = b"java/lang/Object";
    let mut bytes = vec![0xCA, 0xFE, 0xBA, 0xBE, 0, 0, 0, 61, 0, 5, 1];
    bytes.extend_from_slice(&(internal_name.len() as u16).to_be_bytes());
    bytes.extend_from_slice(internal_name);
    bytes.extend_from_slice(&[7, 0, 1, 1]);
    bytes.extend_from_slice(&(object_name.len() as u16).to_be_bytes());
    bytes.extend_from_slice(object_name);
    bytes.extend_from_slice(&[7, 0, 3, 0, 0x21, 0, 2, 0, 4]);
    bytes.extend_from_slice(&[0; 8]);
    bytes
}

struct ExistingPackageResolver {
    expected_path: Vec<String>,
    package: SymbolId,
    module_class: SymbolId,
    object: SymbolId,
    relative: SymbolId,
    requests: std::rc::Rc<std::cell::RefCell<Vec<Vec<String>>>>,
}

impl SymbolResolver for ExistingPackageResolver {
    fn module_class_of_object(
        &self,
        _store: &SemanticStore,
        object: SymbolId,
    ) -> Result<Option<SymbolId>, ResolutionError> {
        Ok((object == self.object).then_some(self.module_class))
    }

    fn checkpoint(&self) -> dotty_core::ResolverCheckpoint {
        dotty_core::ResolverCheckpoint::new(0)
    }

    fn rollback_to(
        &mut self,
        _store: &mut SemanticStore,
        _checkpoint: dotty_core::ResolverCheckpoint,
    ) {
    }

    fn resolve_member(
        &mut self,
        store: &mut SemanticStore,
        request: &MemberRequest,
    ) -> Result<Option<SymbolId>, ResolutionError> {
        Ok((store.names.resolve(request.name.text()) == "Relative").then_some(self.relative))
    }

    fn resolve_package(
        &mut self,
        _store: &mut SemanticStore,
        path: &[&str],
    ) -> Result<Option<SymbolId>, ResolutionError> {
        let path: Vec<_> = path.iter().map(|segment| (*segment).to_owned()).collect();
        self.requests.borrow_mut().push(path.clone());
        Ok((path == self.expected_path).then_some(self.package))
    }
}

fn package_prefix_symbol(store: &SemanticStore, prefix: dotty_core::TypeId) -> SymbolId {
    let package = match store.types.get(prefix) {
        Type::ThisType { class } => *class,
        Type::TermRef {
            target: TermRefTarget::Symbol(symbol),
            ..
        }
        | Type::TypeRef {
            target: TypeRefTarget::Symbol(symbol),
            ..
        } => *symbol,
        other => panic!("expected a package prefix, got {other:?}"),
    };
    assert_eq!(store.symbols.get(package).kind, SymbolKind::Package);
    package
}

#[test]
fn source_and_tasty_references_use_the_same_package_prefix_identity() {
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();

    let widget_file = TastyFile::parse_scala_3_9(WIDGET).unwrap();
    let (widget_index, next_packages) = {
        let mut unpickler =
            TastyUnpickler::with_packages(&widget_file, &mut store, definitions, packages);
        unpickler.enter_symbols().unwrap();
        unpickler.into_parts()
    };
    packages = next_packages;
    let root = packages.symbol::<&str>(&[]).unwrap();
    let widget_name = Name::new(store.names.intern("Widget"), Namespace::Type);
    let widget = store
        .scopes
        .get(packages.scope_of(root).unwrap())
        .lookup(&widget_name)
        .unwrap();
    assert_eq!(widget_index.symbol_at(0), Some(root));

    let holder_file = TastyFile::parse_scala_3_9(HOLDER).unwrap();
    let addresses: Vec<_> = holder_file
        .ast_address_index()
        .unwrap()
        .iter_nodes()
        .filter(|node| node.tag == 117)
        .map(|node| u32::try_from(node.offset).unwrap())
        .collect();
    let (tasty_types, next_packages) = {
        let mut unpickler =
            TastyUnpickler::with_packages(&holder_file, &mut store, definitions, packages);
        unpickler.enter_symbols().unwrap();
        let mut found = Vec::new();
        for address in addresses {
            let Ok(ty) = unpickler.unpickle_type(address) else {
                continue;
            };
            found.push(ty);
        }
        let (_, packages) = unpickler.into_parts();
        (found, packages)
    };
    packages = next_packages;
    let tasty_prefix = tasty_types
        .iter()
        .find_map(|ty| match store.types.get(*ty) {
            Type::TypeRef { prefix, .. }
                if store.types.get(*ty).reference_symbol() == Some(widget) =>
            {
                Some(*prefix)
            }
            _ => None,
        })
        .expect("Holder contains a type reference to Widget");

    let source = SourceId::from_index(73);
    let source_text = "val x: Widget = 1";
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
        "Source.scala",
        &mut store,
        &mut packages,
    )
    .unwrap();
    let (value, type_tree) = parsed
        .ast
        .iter()
        .find_map(|(tree, node)| match &node.kind {
            TreeKind::ValDef(definition)
                if store.names.resolve(definition.name.as_name().text()) == "x" =>
            {
                Some((index.symbol_at(source, tree).unwrap(), definition.tpt))
            }
            _ => None,
        })
        .unwrap();
    let source_prefix = {
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        );
        let projected = typer.complete_symbol(value).unwrap();
        let Type::TypeRef {
            prefix,
            target: TypeRefTarget::Symbol(symbol),
        } = typer.store().types.get(projected)
        else {
            panic!("expected the source Widget reference")
        };
        assert_eq!(*symbol, widget);
        assert_eq!(
            typer.source_type_index().type_at(source, type_tree),
            Some(projected)
        );
        *prefix
    };

    assert_eq!(
        package_prefix_symbol(&store, source_prefix),
        package_prefix_symbol(&store, tasty_prefix)
    );
    assert_eq!(package_prefix_symbol(&store, source_prefix), root);
}

#[test]
fn current_package_import_reuses_package_identity_from_scala39_tasty() {
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let packages = Packages::new();
    let tasty_file = TastyFile::parse_scala_3_9(ISSUE_1173_LOCAL).unwrap();
    let (entered_symbols, packages) = {
        let mut unpickler =
            TastyUnpickler::with_packages(&tasty_file, &mut store, definitions, packages);
        let entered = unpickler
            .enter_symbols()
            .unwrap()
            .entered_symbols()
            .to_vec();
        let (_, packages) = unpickler.into_parts();
        (entered, packages)
    };
    let mut packages = packages;
    let relative_package = packages.symbol(&["a", "consumer", "b"]).unwrap();

    let source = SourceId::from_index(1173);
    let source_text = "package a.consumer { import b.Local.Relative; class Use { val value: Relative = new Relative } }";
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
        "Issue1173.scala",
        &mut store,
        &mut packages,
    )
    .unwrap();

    let current_package = packages.symbol(&["a", "consumer"]).unwrap();
    // Model a package known to the binary package registry but absent from
    // the source lexical scope; the resolver must return that same identity.
    let current_scope = packages.scope_of(current_package).unwrap();
    store.scopes.get_mut(current_scope).remove(relative_package);
    let (_value, type_tree) = parsed
        .ast
        .iter()
        .find_map(|(tree, node)| match &node.kind {
            TreeKind::ValDef(definition)
                if store.names.resolve(definition.name.as_name().text()) == "value" =>
            {
                Some((index.symbol_at(source, tree).unwrap(), definition.tpt))
            }
            _ => None,
        })
        .unwrap();
    let module_class = entered_symbols
        .iter()
        .copied()
        .find(|symbol| {
            let entry = store.symbols.get(*symbol);
            entry.kind == SymbolKind::ModuleClass
                && entry.owner == Some(relative_package)
                && store.names.resolve(entry.name.text()) == "Local$"
        })
        .unwrap();
    let local = entered_symbols
        .iter()
        .copied()
        .find(|symbol| {
            let entry = store.symbols.get(*symbol);
            entry.kind == SymbolKind::Object
                && entry.owner == Some(relative_package)
                && store.names.resolve(entry.name.text()) == "Local"
        })
        .unwrap();
    let relative = entered_symbols
        .iter()
        .copied()
        .find(|symbol| {
            let entry = store.symbols.get(*symbol);
            entry.kind == SymbolKind::Class
                && entry.owner == Some(module_class)
                && store.names.resolve(entry.name.text()) == "Relative"
        })
        .unwrap();
    let requests = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let resolver = ExistingPackageResolver {
        expected_path: vec!["a".to_owned(), "consumer".to_owned(), "b".to_owned()],
        package: relative_package,
        module_class,
        object: local,
        relative,
        requests: std::rc::Rc::clone(&requests),
    };
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    )
    .with_resolver(Box::new(resolver));

    typer.complete_symbol(_value).unwrap();

    let projected = typer
        .source_type_index()
        .type_at(source, type_tree)
        .unwrap();
    let Type::TypeRef {
        target: TypeRefTarget::Symbol(found),
        ..
    } = typer.store().types.get(projected)
    else {
        panic!("expected the imported TASTy class type")
    };
    assert_eq!(*found, relative);
    assert_eq!(requests.borrow().as_slice(), [&["a", "consumer", "b"]]);
}

#[test]
fn source_relative_class_import_resolves_through_the_scala39_classpath_artifact() {
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let source = SourceId::from_index(1174);
    let source_text =
        "package a.consumer { import binary.Local; class Use { val value: Local = new Local } }";
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
        "Issue1173Classpath.scala",
        &mut store,
        &mut packages,
    )
    .unwrap();
    let current_package = packages.symbol(&["a", "consumer"]).unwrap();
    let (value, type_tree) = parsed
        .ast
        .iter()
        .find_map(|(tree, node)| match &node.kind {
            TreeKind::ValDef(definition)
                if store.names.resolve(definition.name.as_name().text()) == "value" =>
            {
                Some((index.symbol_at(source, tree).unwrap(), definition.tpt))
            }
            _ => None,
        })
        .unwrap();
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    )
    .with_resolver(Box::new(ClasspathSymbolResolver::new(
        Issue1173FixturePath,
        definitions,
        LoadingSession::with_packages(packages.snapshot()),
    )));

    typer.complete_symbol(value).unwrap();

    let projected = typer
        .source_type_index()
        .type_at(source, type_tree)
        .unwrap();
    let Type::TypeRef {
        target: TypeRefTarget::Symbol(class),
        ..
    } = typer.store().types.get(projected)
    else {
        panic!("expected the imported class type")
    };
    assert_eq!(typer.store().symbols.get(*class).kind, SymbolKind::Class);
    assert_eq!(
        typer
            .store()
            .names
            .resolve(typer.store().symbols.get(*class).name.text()),
        "Local"
    );
    let package = typer.store().symbols.get(*class).owner.unwrap();
    assert_eq!(typer.store().symbols.get(package).kind, SymbolKind::Package);
    assert_eq!(
        typer.store().symbols.get(package).owner,
        Some(current_package)
    );
}

#[test]
fn dotty_core_import_resolves_to_its_package_before_a_later_lookup_blocker() {
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let source = SourceId::from_index(1180);
    let source_text = "package dotty.tools.dotc\nimport core.Contexts\nobject Use { def accepted(context: Contexts): Contexts = context; def blocked(context: Contexts): Int = context.noSuchMember }";
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
        "Issue1180DottyCoreImport.scala",
        &mut store,
        &mut packages,
    )
    .unwrap();
    let (accepted, accepted_tpt) = parsed
        .ast
        .iter()
        .find_map(|(tree, node)| match &node.kind {
            TreeKind::DefDef(definition)
                if store.names.resolve(definition.name.as_name().text()) == "accepted" =>
            {
                Some((index.symbol_at(source, tree).unwrap(), definition.tpt))
            }
            _ => None,
        })
        .unwrap();
    let (blocked, blocked_rhs) = parsed
        .ast
        .iter()
        .find_map(|(tree, node)| match &node.kind {
            TreeKind::DefDef(definition)
                if store.names.resolve(definition.name.as_name().text()) == "blocked" =>
            {
                Some((
                    index.symbol_at(source, tree).unwrap(),
                    definition.rhs.unwrap(),
                ))
            }
            _ => None,
        })
        .unwrap();
    let package_requests = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let resolver = TracedResolver {
        inner: ClasspathSymbolResolver::new(
            DottyCoreClassPath,
            definitions,
            LoadingSession::with_packages(packages.snapshot()),
        ),
        package_requests: std::rc::Rc::clone(&package_requests),
    };
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    )
    .with_resolver(Box::new(resolver));

    typer.complete_symbol(accepted).unwrap();

    let package_attempts = package_requests.borrow();
    let core_package = package_attempts
        .iter()
        .find_map(|(path, result)| {
            (path == &["dotty", "tools", "dotc", "core"])
                .then_some(*result)
                .flatten()
        })
        .unwrap();
    assert!(package_attempts.iter().any(|(path, result)| {
        path == &["dotty", "tools", "dotc", "core"] && *result == Some(core_package)
    }));
    drop(package_attempts);

    let accepted_type = typer
        .source_type_index()
        .type_at(source, accepted_tpt)
        .unwrap();
    let Type::TypeRef {
        target: TypeRefTarget::Symbol(context_class),
        ..
    } = typer.store().types.get(accepted_type)
    else {
        panic!("expected the imported class type")
    };
    assert_eq!(
        typer.store().symbols.get(*context_class).kind,
        SymbolKind::Class
    );
    assert_eq!(
        typer
            .store()
            .names
            .resolve(typer.store().symbols.get(*context_class).name.text()),
        "Contexts"
    );
    assert_eq!(
        typer.store().symbols.get(*context_class).owner,
        Some(core_package)
    );

    let context = typer.expression_context_for(blocked).unwrap();
    let blocked_error = typer.type_expression(blocked_rhs, context).unwrap_err();
    assert!(
        matches!(
            blocked_error,
            dotty_typer::TyperError::MemberNotFound { .. }
        ),
        "unexpected later blocker: {blocked_error:?}"
    );
}

#[test]
fn stacked_package_imports_use_current_then_enclosing_tasty_packages() {
    let source_text = "package a { package core { object SourceOnly }; package ast { import core.Obj; object Use { def run: Int = Obj.value } } }";

    for (index, include_current_package) in [true, false].into_iter().enumerate() {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let mut packages = Packages::new();
        let source = SourceId::from_index(1175 + index as u32);
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
            "Issue1174Stacked.scala",
            &mut store,
            &mut packages,
        )
        .unwrap();
        let expected_package = packages
            .symbol(if include_current_package {
                &["a", "ast"]
            } else {
                &["a"]
            })
            .unwrap();
        let (run, rhs) = parsed
            .ast
            .iter()
            .find_map(|(tree, node)| match &node.kind {
                TreeKind::DefDef(definition)
                    if store.names.resolve(definition.name.as_name().text()) == "run" =>
                {
                    Some((
                        index.symbol_at(source, tree).unwrap(),
                        definition.rhs.unwrap(),
                    ))
                }
                _ => None,
            })
            .unwrap();
        let mut typer = SourceTyper::new(
            &parsed.ast,
            source,
            &index,
            &mut store,
            definitions,
            &packages,
        )
        .with_resolver(Box::new(ClasspathSymbolResolver::new(
            Issue1174StackedPath {
                include_current_package,
            },
            definitions,
            LoadingSession::with_packages(packages.snapshot()),
        )));

        let context = typer.expression_context_for(run).unwrap();
        let typed_rhs = typer.type_expression(rhs, context).unwrap();
        let TreeKind::Select(selection) = &typer.typed_ast().get(typed_rhs).kind else {
            panic!("expected a selection through the imported object")
        };
        let object_ref = typer.typed_ast().get(selection.qualifier);
        let Type::TermRef {
            target: TermRefTarget::Symbol(object),
            ..
        } = typer.store().types.get(object_ref.ty)
        else {
            panic!("expected a canonical object reference")
        };
        let package = typer.store().symbols.get(*object).owner.unwrap();
        assert_eq!(typer.store().symbols.get(package).kind, SymbolKind::Package);
        assert_eq!(
            typer.store().symbols.get(package).owner,
            Some(expected_package)
        );
    }
}

#[test]
fn explicit_import_shadows_a_same_named_package_qualifier() {
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let source = SourceId::from_index(1179);
    let source_text = "package a.consumer.b { object Local { class PackageRelative } }; package other { object b { object Local { class Relative } } }; package a.consumer { import other.b; import b.Local.Relative; class Use { val value: Relative = new Relative } }";
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
        "Issue1174PackageImportPrecedence.scala",
        &mut store,
        &mut packages,
    )
    .unwrap();
    let (value, type_tree) = parsed
        .ast
        .iter()
        .filter_map(|(tree, node)| match &node.kind {
            TreeKind::ValDef(definition)
                if store.names.resolve(definition.name.as_name().text()) == "value" =>
            {
                Some((index.symbol_at(source, tree).unwrap(), definition.tpt))
            }
            _ => None,
        })
        .next()
        .unwrap();
    let relative = parsed
        .ast
        .iter()
        .find_map(|(tree, node)| match &node.kind {
            TreeKind::TypeDef(definition)
                if store.names.resolve(definition.name.as_name().text()) == "Relative" =>
            {
                index.symbol_at(source, tree)
            }
            _ => None,
        })
        .unwrap();
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    );

    typer.complete_symbol(value).unwrap();

    let projected = typer
        .source_type_index()
        .type_at(source, type_tree)
        .expect("the explicitly imported qualifier should resolve");
    let Type::TypeRef {
        target: TypeRefTarget::Symbol(found),
        ..
    } = typer.store().types.get(projected)
    else {
        panic!("expected the imported Relative type")
    };
    assert_eq!(*found, relative);
    let local_module = typer.store().symbols.get(relative).owner.unwrap();
    assert_eq!(
        typer.store().symbols.get(local_module).kind,
        SymbolKind::ModuleClass
    );
    let imported_b = typer.store().symbols.get(local_module).owner.unwrap();
    assert_eq!(
        typer.store().symbols.get(imported_b).kind,
        SymbolKind::ModuleClass
    );
    assert_eq!(
        typer.store().symbols.get(imported_b).owner,
        Some(packages.symbol(&["other"]).unwrap())
    );
}

#[test]
fn dotted_package_declaration_does_not_expose_synthetic_parent_for_imports() {
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    let source = SourceId::from_index(1177);
    let source_text = "package a.ast\nimport core.Obj\nobject Use { def run: Int = Obj.value }";
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
        "Issue1174Dotted.scala",
        &mut store,
        &mut packages,
    )
    .unwrap();
    let (run, rhs) = parsed
        .ast
        .iter()
        .find_map(|(tree, node)| match &node.kind {
            TreeKind::DefDef(definition)
                if store.names.resolve(definition.name.as_name().text()) == "run" =>
            {
                Some((
                    index.symbol_at(source, tree).unwrap(),
                    definition.rhs.unwrap(),
                ))
            }
            _ => None,
        })
        .unwrap();
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    )
    .with_resolver(Box::new(ClasspathSymbolResolver::new(
        Issue1174StackedPath {
            include_current_package: false,
        },
        definitions,
        LoadingSession::with_packages(packages.snapshot()),
    )));

    let context = typer.expression_context_for(run).unwrap();
    assert!(matches!(
        typer.type_expression(rhs, context),
        Err(dotty_typer::TyperError::ImportQualifierNotFound { .. })
    ));
}
