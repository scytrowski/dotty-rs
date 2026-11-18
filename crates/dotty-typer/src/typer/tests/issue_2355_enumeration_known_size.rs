use super::*;
use dotty_classloader::classloader::{
    ClasspathSymbolResolver, CompositeClassPath, JarClassPath, JdkClassPath, LoadingSession,
};
use dotty_core::{SourceText, ast::TreeKind};
use dotty_lexer::ContextualScanner;
use dotty_namer::name_compilation_unit;
use std::{cell::RefCell, fs, path::PathBuf, rc::Rc};

const SCALA_REVISION: &str = "777528f19a58e794c9954a42f433373472ec57f8";
const TARGET_FILE: &str = "library/src/scala/Enumeration.scala";
const TARGET_TREE: u32 = 700;
const FAILURE_TREE: u32 = 627;

fn pinned_scala_root() -> PathBuf {
    let root = PathBuf::from(std::env::var_os("SCALA39_ROOT").expect("SCALA39_ROOT is required"));
    let revision = std::process::Command::new("git")
        .args(["-C", root.to_str().unwrap(), "rev-parse", "HEAD"])
        .output()
        .expect("git should read the pinned Scala checkout");
    assert!(
        revision.status.success(),
        "Scala checkout revision is readable"
    );
    assert_eq!(
        String::from_utf8(revision.stdout).unwrap().trim(),
        SCALA_REVISION,
        "the evidence test uses the repository-pinned Scala 3.9 source"
    );
    root
}

fn classpath() -> CompositeClassPath {
    let java_home = PathBuf::from(std::env::var_os("JAVA_HOME").expect("JAVA_HOME is required"));
    assert_eq!(
        std::env::var("SCALA39_JDK_RELEASE").expect("SCALA39_JDK_RELEASE is required"),
        "21"
    );
    let release = std::env::var("SCALA39_JDK_RELEASE")
        .unwrap()
        .parse::<u16>()
        .unwrap();
    let mut entries: Vec<Box<dyn ClassPathEntry>> = vec![Box::new(
        JdkClassPath::new(java_home.join("jmods")).expect("pinned JDK jmods should load"),
    )];
    let jars = std::env::split_paths(
        &std::env::var_os("SCALA39_CLASSPATH").expect("SCALA39_CLASSPATH is required"),
    )
    .collect::<Vec<_>>();
    assert!(
        !jars.is_empty(),
        "the pinned Scala artifact classpath is explicit"
    );
    for jar in jars {
        assert!(
            jar.is_file(),
            "Scala classpath artifact exists: {}",
            jar.display()
        );
        entries.push(Box::new(
            JarClassPath::new(jar.clone(), release)
                .unwrap_or_else(|error| panic!("cannot load {}: {error}", jar.display())),
        ));
    }
    CompositeClassPath::new(entries)
}

fn owner_path(store: &SemanticStore, mut symbol: SymbolId) -> String {
    let mut segments = Vec::new();
    loop {
        let current = store.symbols.get(symbol);
        if current.kind != SymbolKind::Package {
            segments.push(store.names.resolve(current.name.text()).to_owned());
        }
        let Some(owner) = current.owner else { break };
        symbol = owner;
    }
    segments.reverse();
    segments.join(".")
}

fn run_target_session(text: &str) -> String {
    let source = SourceId::from_index(11);
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let scanner = ContextualScanner::new(text).expect("pinned Enumeration.scala scans");
    let parsed = dotty_parser::parse_compilation_unit(
        SourceText::new(text).expect("pinned Enumeration.scala is UTF-8"),
        source,
        scanner,
        &mut store.names,
    );
    let mut packages = Packages::new();
    let index = name_compilation_unit(
        &parsed.ast,
        parsed.root,
        source,
        TARGET_FILE,
        &mut store,
        &mut packages,
    )
    .expect("the complete pinned Enumeration.scala file names");
    let (method_tree, method_node) = parsed
        .ast
        .iter()
        .find(|(tree, _)| tree.index() == TARGET_TREE)
        .expect("the verified ValueSet.knownSize DefDef source tree exists");
    let TreeKind::DefDef(method_def) = &method_node.kind else {
        panic!("the verified root is a DefDef")
    };
    let method = index
        .symbol_at(source, method_tree)
        .expect("the method has a canonical namer symbol");
    let owner = store
        .symbols
        .get(method)
        .owner
        .expect("knownSize has its containing source class");
    let canonical_owner = owner_path(&store, owner);
    assert_eq!(canonical_owner, "Enumeration.ValueSet");
    let (receiver_decl_tree, receiver_symbol) = parsed
        .ast
        .iter()
        .find_map(|(tree, node)| {
            let TreeKind::ValDef(definition) = &node.kind else {
                return None;
            };
            (store.names.resolve(definition.name.as_name().text()) == "nnIds")
                .then(|| index.symbol_at(source, tree).map(|symbol| (tree, symbol)))
                .flatten()
                .filter(|(_, symbol)| store.symbols.get(*symbol).owner == Some(owner))
        })
        .expect("nnIds has its original source declaration and canonical owner");
    let TreeKind::ValDef(receiver_decl) = &parsed.ast.get(receiver_decl_tree).kind else {
        unreachable!("the nnIds declaration is a ValDef")
    };
    let receiver_type = parsed
        .ast
        .get(receiver_decl.tpt)
        .position
        .and_then(|position| {
            let range = position.span().range();
            text.get(range.start() as usize..range.end() as usize)
        })
        .unwrap_or("unavailable");
    assert_eq!(
        method_node.position.unwrap().span().range().start(),
        11800,
        "the source-tree identity and span are pinned"
    );
    assert_eq!(method_def.rhs.map(TreeId::index), Some(699));

    let observed = Rc::new(RefCell::new(None::<String>));
    let observed_by_failure = Rc::clone(&observed);
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    )
    .with_resolver(Box::new(ClasspathSymbolResolver::new(
        classpath(),
        definitions,
        LoadingSession::with_packages(packages.snapshot()),
    )));
    typer.transaction_failure_error_observer = Some(Rc::new(move |typer, error| {
        let (failed_tree, failed_node) = typer
            .arena
            .iter()
            .find(|(tree, _)| tree.index() == FAILURE_TREE)
            .expect("the reported first-failure source tree exists");
        let failed_span = failed_node
            .position
            .map(|position| position.span().range())
            .expect("the first-failure source tree retains its span");
        let failed_kind = match &failed_node.kind {
            TreeKind::Ident(ident) => {
                format!("Ident({})", typer.store.names.resolve(ident.name.text()))
            }
            TreeKind::Select(select) => {
                format!("Select({})", typer.store.names.resolve(select.name.text()))
            }
            other => format!("Other({other:?})"),
        };
        *observed_by_failure.borrow_mut() = Some(format!(
            "failure_tree={} span={}..{} kind={} error={error:?}",
            failed_tree.index(),
            failed_span.start(),
            failed_span.end(),
            failed_kind,
        ));
    }));
    let result = typer.type_method_definition(method_tree);
    let failure = observed
        .borrow()
        .clone()
        .expect("the transaction observer records the original error before rollback");
    assert!(
        matches!(
            result,
            Err(TyperError::SymbolResolution {
                tree_index: FAILURE_TREE,
                ..
            })
        ),
        "the complete original DefDef remains blocked at its observed tree: {result:?}"
    );
    assert!(
        failure.contains("failed to load because scala/ImpureFunction1 failed"),
        "{failure}"
    );
    assert!(
        failure.contains("class not found: scala/ImpureFunction1"),
        "{failure}"
    );
    format!(
        "source_revision={SCALA_REVISION}\nsource={TARGET_FILE}#tree={TARGET_TREE}\nowner=scala.{canonical_owner}\nowner_symbol={owner:?}\nmethod=knownSize\nmethod_symbol={method:?}\nsource_span=11800..11840\nexpected_result=Int\nactual_result=unavailable\nreceiver=nnIds\nreceiver_symbol={receiver_symbol:?}\nreceiver_decl_tree={}\nreceiver_type={receiver_type}\n{failure}",
        receiver_decl_tree.index()
    )
}

fn run_wrong_owner_control(root: &std::path::Path) -> String {
    let relative_file = "library/src/scala/collection/mutable/ArrayBuilder.scala";
    let text = fs::read_to_string(root.join(relative_file)).expect("control source exists");
    let source = SourceId::from_index(12);
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let scanner = ContextualScanner::new(&text).expect("control source scans");
    let parsed = dotty_parser::parse_compilation_unit(
        SourceText::new(&text).expect("control source is UTF-8"),
        source,
        scanner,
        &mut store.names,
    );
    let mut packages = Packages::new();
    let index = name_compilation_unit(
        &parsed.ast,
        parsed.root,
        source,
        relative_file,
        &mut store,
        &mut packages,
    )
    .expect("the whole control source names");
    let (method_tree, method_node) = parsed
        .ast
        .iter()
        .find(|(tree, _)| tree.index() == 48)
        .expect("frozen ArrayBuilder.knownSize control tree exists");
    let TreeKind::DefDef(definition) = &method_node.kind else {
        panic!("the control source tree is a DefDef")
    };
    let method = index
        .symbol_at(source, method_tree)
        .expect("control method has a canonical source symbol");
    let owner = store
        .symbols
        .get(method)
        .owner
        .expect("control method has its containing owner");
    let canonical_owner = owner_path(&store, owner);
    assert_eq!(canonical_owner, "ArrayBuilder");
    let mut typer = SourceTyper::new(
        &parsed.ast,
        source,
        &index,
        &mut store,
        definitions,
        &packages,
    )
    .with_resolver(Box::new(ClasspathSymbolResolver::new(
        classpath(),
        definitions,
        LoadingSession::with_packages(packages.snapshot()),
    )));
    typer
        .type_method_definition(method_tree)
        .expect("the same-named knownSize on the separate owner is supported");
    assert!(matches!(
        typer.store().symbols.info(method),
        SymbolInfo::Complete(_)
    ));
    assert!(
        typer
            .source_typed_index()
            .get(source, method_tree)
            .is_some()
    );
    let expected = parsed
        .ast
        .get(definition.tpt)
        .position
        .and_then(|position| {
            let range = position.span().range();
            text.get(range.start() as usize..range.end() as usize)
        })
        .unwrap_or("unavailable");
    format!(
        "source={relative_file}#tree=48 owner=scala.collection.mutable.{canonical_owner} method=knownSize result={expected} status=typed"
    )
}

#[test]
#[ignore = "run with SCALA39_ROOT, SCALA39_CLASSPATH, and JDK 21 for issue #2355 evidence"]
fn scala39_enumeration_known_size_records_first_failure_deterministically() {
    let root = pinned_scala_root();
    let text = fs::read_to_string(root.join(TARGET_FILE)).expect("official source file exists");
    let first = run_target_session(&text);
    let second = run_target_session(&text);
    assert_eq!(
        first, second,
        "two fresh sessions produce the same normalized evidence"
    );
    let wrong_owner_control = run_wrong_owner_control(&root);
    println!(
        "ISSUE_2355_EVIDENCE_BEGIN\n{first}\ncontrol={wrong_owner_control}\nISSUE_2355_EVIDENCE_END"
    );
}
