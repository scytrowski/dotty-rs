//! JVM classfiles only ever produce the four Java visibilities.
//!
//! `dotty-core`'s `Visibility` also has `PrivateWithin` / `ProtectedWithin`
//! for Scala's qualified `private[Q]` / `protected[Q]`. A classfile cannot
//! express those (JVMS access flags are just public/private/protected/none),
//! so the `.class` lowering must never yield them. This checks that over
//! real JDK classes, which between them contain public, private, protected
//! and package-private classes and members.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use dotty_classloader::classloader::{
    BinaryName, ClassFormat, ClassLoader, ClassOrigin, ClassPathEntry, ClassPathError,
    ClassResource, DirectoryClassPath,
};
use dotty_core::ids::SymbolId;
use dotty_core::store::SemanticStore;
use dotty_core::symbols::Visibility;

fn corpus_root(version: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../dotty-classfile/tests/fixtures/jdk_corpus")
        .join(version)
}

fn corpus_classes_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../dotty-classfile/tests/fixtures/jdk_corpus/classes.txt")
}

/// The real corpus classes, with a minimal synthetic stand-in for any class
/// they depend on that is not in the corpus (`java/lang/Object`, the
/// interfaces they implement, the types in their signatures). The real
/// classes are what is under test; the stand-ins are public and empty.
struct CorpusWithStubs(DirectoryClassPath);

impl ClassPathEntry for CorpusWithStubs {
    fn find_class(&self, name: &BinaryName) -> Result<Option<ClassResource>, ClassPathError> {
        if let Some(real) = self.0.find_class(name)? {
            return Ok(Some(real));
        }
        let super_name = (name.as_internal() != "java/lang/Object").then_some("java/lang/Object");
        Ok(Some(ClassResource::new(
            stub_class(name.as_internal(), super_name),
            ClassFormat::Class,
            ClassOrigin::Directory(PathBuf::from("<stub>")),
        )))
    }
}

/// A minimal public class file: no interfaces, fields, methods or attributes.
fn stub_class(this_name: &str, super_name: Option<&str>) -> Vec<u8> {
    fn utf8(pool: &mut Vec<u8>, next: &mut u16, value: &str) -> u16 {
        let index = *next;
        pool.push(1); // CONSTANT_Utf8
        pool.extend_from_slice(&(value.len() as u16).to_be_bytes());
        pool.extend_from_slice(value.as_bytes());
        *next += 1;
        index
    }
    fn class(pool: &mut Vec<u8>, next: &mut u16, name_index: u16) -> u16 {
        let index = *next;
        pool.push(7); // CONSTANT_Class
        pool.extend_from_slice(&name_index.to_be_bytes());
        *next += 1;
        index
    }

    let (mut pool, mut next) = (Vec::new(), 1u16);
    let this_name_index = utf8(&mut pool, &mut next, this_name);
    let this_class = class(&mut pool, &mut next, this_name_index);
    let super_class = super_name.map(|name| {
        let name_index = utf8(&mut pool, &mut next, name);
        class(&mut pool, &mut next, name_index)
    });

    let mut bytes = vec![0xCA, 0xFE, 0xBA, 0xBE, 0x00, 0x00, 0x00, 0x45];
    bytes.extend_from_slice(&next.to_be_bytes()); // constant_pool_count
    bytes.extend_from_slice(&pool);
    bytes.extend_from_slice(&0x0021u16.to_be_bytes()); // ACC_PUBLIC | ACC_SUPER
    bytes.extend_from_slice(&this_class.to_be_bytes());
    bytes.extend_from_slice(&super_class.unwrap_or(0).to_be_bytes());
    bytes.extend_from_slice(&[0; 8]); // interfaces, fields, methods, attributes
    bytes
}

/// Binary names listed in the corpus's `classes.txt`.
fn corpus_classes() -> Vec<BinaryName> {
    fs::read_to_string(corpus_classes_path())
        .expect("classes.txt should exist")
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(BinaryName::from_internal)
        .collect()
}

fn is_java_visibility(visibility: Visibility) -> bool {
    matches!(
        visibility,
        Visibility::Public | Visibility::Private | Visibility::Protected | Visibility::Package(_)
    )
}

/// Every class and member symbol produced by loading the corpus.
fn loaded_symbols(corpus_root: &Path) -> (SemanticStore, Vec<SymbolId>) {
    let mut store = SemanticStore::new();
    let mut symbols = Vec::new();
    {
        let mut loader = ClassLoader::new(
            CorpusWithStubs(DirectoryClassPath::new(corpus_root.to_path_buf())),
            &mut store,
        );
        for name in corpus_classes() {
            let class = loader
                .load_class(&name)
                .unwrap_or_else(|error| panic!("{} should load: {error:?}", name.as_internal()));
            symbols.push(class);
            let metadata = loader
                .metadata(class)
                .expect("a loaded class has classfile metadata");
            symbols.extend(metadata.fields.iter().map(|field| field.symbol()));
            symbols.extend(metadata.methods.iter().filter_map(|method| method.symbol()));
        }
    }
    (store, symbols)
}

#[test]
fn loaded_classes_and_members_never_get_a_qualified_visibility() {
    for version in ["jdk23", "jdk24", "jdk25", "jdk26"] {
        let root = corpus_root(version);
        let (store, symbols) = loaded_symbols(&root);

        assert!(!symbols.is_empty(), "{version} produced no symbols");
        for symbol in symbols {
            let visibility = store.symbols.get(symbol).visibility;
            assert!(
                is_java_visibility(visibility),
                "{version}: symbol {} has non-JVM visibility {visibility:?}",
                symbol.index()
            );
        }
    }
}

#[test]
fn the_corpus_exercises_every_java_visibility() {
    // Guards the test above against passing vacuously on a corpus that only
    // contains, say, public members.
    for version in ["jdk23", "jdk24", "jdk25", "jdk26"] {
        let root = corpus_root(version);
        let (store, symbols) = loaded_symbols(&root);

        let seen: HashSet<&str> = symbols
            .iter()
            .map(|symbol| match store.symbols.get(*symbol).visibility {
                Visibility::Public => "public",
                Visibility::Private => "private",
                Visibility::Protected => "protected",
                Visibility::Package(_) => "package",
                Visibility::PrivateWithin(_) | Visibility::ProtectedWithin(_) => "qualified",
            })
            .collect();

        for expected in ["public", "private", "protected", "package"] {
            assert!(
                seen.contains(expected),
                "{version}: no {expected} symbol in the corpus"
            );
        }
        assert!(!seen.contains("qualified"));
    }
}
