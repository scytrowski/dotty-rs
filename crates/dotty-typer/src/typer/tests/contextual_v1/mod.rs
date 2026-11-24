use super::*;
use dotty_classloader::classloader::{
    ClassPathEntry, ClasspathSymbolResolver, CompositeClassPath, JarClassPath, JdkClassPath,
    LoadingSession,
};
use std::path::PathBuf;

mod clauses;
mod fractional_conversion;
mod inherited_nested;
mod lookup;
mod numeric_ops_parent;
mod search;

fn upstream(path: &str) -> String {
    let root = PathBuf::from(std::env::var_os("SCALA39_ROOT").expect("pinned Scala checkout"));
    println!(
        "AUDIT_SOURCE\t{}\t{path}",
        std::thread::current().name().unwrap()
    );
    std::fs::read_to_string(root.join(path)).expect("upstream acceptance case")
}

fn audit_resolver(
    definitions: Definitions,
    packages: &Packages,
) -> ClasspathSymbolResolver<CompositeClassPath> {
    let java = PathBuf::from(std::env::var_os("JAVA_HOME").expect("JDK 21"));
    let mut entries: Vec<Box<dyn ClassPathEntry>> =
        vec![Box::new(JdkClassPath::new(java.join("jmods")).unwrap())];
    let classpath = std::env::var_os("SCALA39_CLASSPATH").expect("pinned Scala runtime jars");
    for jar in std::env::split_paths(&classpath) {
        entries.push(Box::new(JarClassPath::new(jar, 21).unwrap()));
    }
    ClasspathSymbolResolver::new(
        CompositeClassPath::new(entries),
        definitions,
        LoadingSession::with_packages(packages.snapshot()),
    )
}

fn outcome(status: &str) {
    println!(
        "AUDIT_OUTCOME\t{}\t{status}",
        std::thread::current().name().unwrap()
    );
}

fn declaration(index: &SourceSemanticIndex, symbol: SymbolId) -> TreeId<Untyped> {
    let Some(SourceDefinition::Canonical { tree, .. }) = index.definition_of(symbol) else {
        panic!("expected a canonical declaration")
    };
    tree
}

fn referent(typer: &SourceTyper<'_>, tree: TreeId<Typed>) -> SymbolId {
    let Type::TermRef {
        target: TermRefTarget::Symbol(symbol),
        ..
    } = typer.store().types.get(typer.typed_ast().get(tree).ty)
    else {
        panic!("expected a symbol reference")
    };
    *symbol
}
