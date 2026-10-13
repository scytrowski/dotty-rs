//! Oracle checks over sources compiled by the pinned Scala 3.9.0 compiler.

use std::fs;
use std::path::{Path, PathBuf};

use dotty_classfile::classfile::{ClassFile, Reader};
use dotty_core::Definitions;
use dotty_core::names::Namespace;
use dotty_core::store::SemanticStore;
use dotty_core::symbols::{SymbolKind, Visibility};
use dotty_tasty::tasty::TastyFile;
use dotty_tasty_unpickler::tasty_unpickler::TastyUnpickler;

#[derive(Debug, PartialEq, Eq)]
struct OracleSymbol {
    path: String,
    kind: SymbolKind,
    namespace: Namespace,
    visibility: Visibility,
}

fn symbols_from_tasty(bytes: &[u8]) -> Result<Vec<OracleSymbol>, String> {
    let file = TastyFile::parse_scala_3_9(bytes).map_err(|error| error.to_string())?;
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let entered = {
        let mut unpickler = TastyUnpickler::new(&file, &mut store, definitions);
        unpickler
            .enter_symbols()
            .map_err(|error| error.to_string())?
            .entered_symbols()
            .to_vec()
    };
    let mut result = entered
        .into_iter()
        .map(|symbol| {
            let entry = store.symbols.get(symbol);
            let mut parts = Vec::new();
            let mut current = Some(symbol);
            while let Some(id) = current {
                let parent = store.symbols.get(id);
                let name = store.names.resolve(parent.name.text());
                if !name.is_empty() {
                    parts.push(name.to_owned());
                }
                current = parent.owner;
            }
            parts.reverse();
            OracleSymbol {
                path: parts.join("."),
                kind: entry.kind,
                namespace: entry.name.namespace(),
                visibility: entry.visibility,
            }
        })
        .collect::<Vec<_>>();
    result.sort_by(|left, right| {
        (&left.path, left.kind as u8, left.namespace as u8).cmp(&(
            &right.path,
            right.kind as u8,
            right.namespace as u8,
        ))
    });
    Ok(result)
}

fn classfile_fields(path: &Path) -> Vec<String> {
    let bytes = fs::read(path).unwrap();
    let mut reader = Reader::new(&bytes);
    let class = ClassFile::decode(&mut reader).unwrap();
    class
        .fields
        .iter()
        .map(|field| {
            class
                .constant_pool
                .utf8(field.name_index)
                .unwrap()
                .to_owned()
        })
        .collect()
}

fn path(root: &Path, relative: &str) -> PathBuf {
    root.join(relative)
}

#[test]
#[ignore = "requires tools/typer-classpath-corpus-audit/module-oracle/run"]
fn pinned_scala39_oracle_pins_object_and_module_class_identity() {
    let root = PathBuf::from(
        std::env::var_os("DOTTY_MODULE_ORACLE_ROOT")
            .expect("run the module-oracle script to compile Scala 3.9.0 fixtures"),
    );
    let only = symbols_from_tasty(&fs::read(path(&root, "issue1046/Only.tasty")).unwrap()).unwrap();
    let pair = symbols_from_tasty(&fs::read(path(&root, "issue1046/Pair.tasty")).unwrap()).unwrap();
    let package =
        symbols_from_tasty(&fs::read(path(&root, "issue1046/pkg/package.tasty")).unwrap()).unwrap();
    let qualified =
        symbols_from_tasty(&fs::read(path(&root, "issue1046/Qualified.tasty")).unwrap()).unwrap();

    let kind_paths = |symbols: &[OracleSymbol], kind| {
        symbols
            .iter()
            .filter(|symbol| symbol.kind == kind)
            .map(|symbol| symbol.path.clone())
            .collect::<Vec<_>>()
    };

    assert!(kind_paths(&only, SymbolKind::Object).contains(&"issue1046.Only".to_owned()));
    assert!(kind_paths(&only, SymbolKind::ModuleClass).contains(&"issue1046.Only$".to_owned()));
    assert!(!kind_paths(&only, SymbolKind::Class).contains(&"issue1046.Only".to_owned()));

    assert!(kind_paths(&pair, SymbolKind::Class).contains(&"issue1046.Pair".to_owned()));
    assert!(kind_paths(&pair, SymbolKind::Object).contains(&"issue1046.Pair".to_owned()));
    assert!(kind_paths(&pair, SymbolKind::ModuleClass).contains(&"issue1046.Pair$".to_owned()));

    assert!(
        package
            .iter()
            .any(|symbol| symbol.kind == SymbolKind::Object && symbol.path.contains("package")),
        "package-object TASTy should enter an object identity: {package:?}"
    );

    let qualified_object = qualified
        .iter()
        .find(|symbol| symbol.kind == SymbolKind::Object && symbol.path == "issue1046.Qualified")
        .expect("qualified object has a TASTy object identity");
    assert_ne!(qualified_object.visibility, Visibility::Public);
    assert!(
        kind_paths(&qualified, SymbolKind::ModuleClass)
            .contains(&"issue1046.Qualified$".to_owned())
    );

    let dollar_class = path(&root, "issue1046/Dollar$.class");
    assert!(!path(&root, "issue1046/Dollar$.tasty").exists());
    assert!(
        !classfile_fields(&dollar_class)
            .iter()
            .any(|name| name == "MODULE$")
    );
    for scala_object in ["Only$", "Pair$", "Qualified$", "pkg/package$"] {
        assert!(
            classfile_fields(&path(&root, &format!("issue1046/{scala_object}.class")))
                .iter()
                .any(|name| name == "MODULE$"),
            "Scala module class {scala_object} exposes the MODULE$ field"
        );
    }

    println!("only.tasty symbols: {only:?}");
    println!("pair.tasty symbols: {pair:?}");
    println!("package.tasty symbols: {package:?}");
    println!("qualified.tasty symbols: {qualified:?}");
    println!(
        "Java Dollar$.class fields: {:?}",
        classfile_fields(&dollar_class)
    );
}

#[test]
fn malformed_tasty_is_not_module_evidence() {
    assert!(symbols_from_tasty(b"not a Scala TASTy file").is_err());
}
