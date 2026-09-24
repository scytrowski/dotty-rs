//! Cross-adapter symbol-annotation storage convergence (Milestone 5e1, issue
//! #129 §33): `dotty-classloader` (JVM classfile attributes) and
//! `dotty-tasty-unpickler` (Milestone 5e1's own `complete_symbol_annotations`)
//! both attach `AnnotationId`s to `Symbol.annotations`, from the very same
//! `dotty_core::types::Annotation`/`AnnotationArena` shared representation.
//!
//! Milestone 6 (classloader/`SymbolResolver` integration) has not landed yet,
//! so the two adapters cannot yet run against one shared `SemanticStore`:
//! `ClassLoader::new` bootstraps its own `Definitions` internally, with no
//! way to hand it an existing one, and `Definitions::bootstrap` is
//! documented as exactly-once per store. This test therefore runs each
//! adapter over its own store and compares the *contract* their results
//! satisfy, not one shared arena — which is what issue #129 §33 actually
//! asks for ("the semantic storage contract should converge", not "equal
//! payloads").

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use dotty_classloader::classloader::{BinaryName, ClassLoader, DirectoryClassPath};
use dotty_core::store::SemanticStore;
use dotty_core::symbols::SymbolInfo;
use dotty_core::types::{AnnotationArguments, Type};
use dotty_core::{Definitions, Packages};
use dotty_tasty::tasty::TastyFile;
use dotty_tasty_unpickler::tasty_unpickler::TastyUnpickler;

const SYMBOL_ANNOTATED: &[u8] =
    include_bytes!("../../../dotty-tasty-unpickler/tests/fixtures/semantic/SymbolAnnotated.tasty");
/// `@SymbolMarker val x`, pinned the same way
/// `dotty-tasty-unpickler/src/enter.rs`'s own unit tests pin it.
const SYMBOL_ANNOTATED_X: u32 = 31;

struct TemporaryDirectory(PathBuf);

impl TemporaryDirectory {
    /// `name` plus a process-wide counter: `classloader_side` is called from
    /// several tests that `cargo test` runs concurrently in one process (so
    /// `std::process::id()` alone is not unique enough — every call used to
    /// share one directory and race on it).
    fn new(name: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "dotty-classloader-cross-adapter-{name}-{}-{unique}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("temporary directory should be creatable");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn push_utf8(pool: &mut Vec<u8>, next_index: &mut u16, value: &str) -> u16 {
    let index = *next_index;
    pool.push(1); // CONSTANT_Utf8
    pool.extend_from_slice(&(value.len() as u16).to_be_bytes());
    pool.extend_from_slice(value.as_bytes());
    *next_index += 1;
    index
}

fn push_class(pool: &mut Vec<u8>, next_index: &mut u16, name_index: u16) -> u16 {
    let index = *next_index;
    pool.push(7); // CONSTANT_Class
    pool.extend_from_slice(&name_index.to_be_bytes());
    *next_index += 1;
    index
}

/// A hand-built, minimal, synthetic class file: `this_name`, no
/// superclass/interfaces/members, no attributes — mirrors
/// `loader.rs`'s own `synthetic_class` test helper.
fn synthetic_class(this_name: &str) -> Vec<u8> {
    let mut pool = Vec::new();
    let mut next_index: u16 = 1;
    let name_index = push_utf8(&mut pool, &mut next_index, this_name);
    let class_index = push_class(&mut pool, &mut next_index, name_index);

    let mut bytes = Vec::new();
    bytes.extend_from_slice(&[0xCA, 0xFE, 0xBA, 0xBE]);
    bytes.extend_from_slice(&[0x00, 0x00]);
    bytes.extend_from_slice(&[0x00, 0x45]); // class-file major = 69
    bytes.extend_from_slice(&next_index.to_be_bytes()); // constant_pool_count
    bytes.extend_from_slice(&pool);
    bytes.extend_from_slice(&[0x00, 0x21]); // access_flags
    bytes.extend_from_slice(&class_index.to_be_bytes());
    bytes.extend_from_slice(&[0x00, 0x00]); // super_class = none
    bytes.extend_from_slice(&[0x00, 0x00]); // interfaces_count
    bytes.extend_from_slice(&[0x00, 0x00]); // fields_count
    bytes.extend_from_slice(&[0x00, 0x00]); // methods_count
    bytes.extend_from_slice(&[0x00, 0x00]); // attributes_count
    bytes
}

/// A hand-built, minimal, synthetic class file named `C`, no superclass, with
/// one class-level `RuntimeVisibleAnnotations` attribute (JVMS §4.7.16)
/// naming the argument-free annotation class `Ann` — mirrors `loader.rs`'s
/// own `synthetic_class_with_field_and_method_annotations` test helper,
/// adapted from a field/method attribute to a class-level one.
fn synthetic_class_with_a_class_level_annotation() -> Vec<u8> {
    let mut pool = Vec::new();
    let mut next_index: u16 = 1;
    let this_name_index = push_utf8(&mut pool, &mut next_index, "C");
    let this_class_index = push_class(&mut pool, &mut next_index, this_name_index);
    let attr_name_index = push_utf8(&mut pool, &mut next_index, "RuntimeVisibleAnnotations");
    let ann_type_index = push_utf8(&mut pool, &mut next_index, "LAnn;");

    let mut annotation_body = Vec::new();
    annotation_body.extend_from_slice(&1u16.to_be_bytes()); // num_annotations
    annotation_body.extend_from_slice(&ann_type_index.to_be_bytes());
    annotation_body.extend_from_slice(&0u16.to_be_bytes()); // num_element_value_pairs

    let mut bytes = Vec::new();
    bytes.extend_from_slice(&[0xCA, 0xFE, 0xBA, 0xBE]);
    bytes.extend_from_slice(&[0x00, 0x00]);
    bytes.extend_from_slice(&[0x00, 0x45]);
    bytes.extend_from_slice(&next_index.to_be_bytes()); // constant_pool_count
    bytes.extend_from_slice(&pool);
    bytes.extend_from_slice(&[0x00, 0x21]); // access_flags
    bytes.extend_from_slice(&this_class_index.to_be_bytes());
    bytes.extend_from_slice(&[0x00, 0x00]); // super_class = none
    bytes.extend_from_slice(&[0x00, 0x00]); // interfaces_count
    bytes.extend_from_slice(&[0x00, 0x00]); // fields_count
    bytes.extend_from_slice(&[0x00, 0x00]); // methods_count
    bytes.extend_from_slice(&[0x00, 0x01]); // attributes_count = 1 (class-level)
    bytes.extend_from_slice(&attr_name_index.to_be_bytes());
    bytes.extend_from_slice(&(annotation_body.len() as u32).to_be_bytes());
    bytes.extend_from_slice(&annotation_body);
    bytes
}

/// Loads `C` (annotated with `@Ann`) through a real `ClassLoader`, in its
/// own store, and returns the store plus `C`'s `dotty_core` `Symbol.
/// annotations`, whose one entry must have `tree: None` and a class-like
/// `ty` (this is the classloader side of the convergence contract; see
/// `loader.rs`'s own, more thorough `resolves_a_real_annotation_on_a_class`
/// and `enters_a_resolved_field_and_method_annotation_onto_their_own_symbols`
/// unit tests for the rest of that adapter's coverage).
fn classloader_side() -> (SemanticStore, Vec<dotty_core::ids::AnnotationId>) {
    let root = TemporaryDirectory::new("classloader-side");
    fs::write(
        root.path().join("C.class"),
        synthetic_class_with_a_class_level_annotation(),
    )
    .unwrap();
    fs::write(root.path().join("Ann.class"), synthetic_class("Ann")).unwrap();
    fs::create_dir_all(root.path().join("java/lang")).unwrap();
    fs::write(
        root.path().join("java/lang/Object.class"),
        synthetic_class("java/lang/Object"),
    )
    .unwrap();

    let mut store = SemanticStore::new();
    let class_path = DirectoryClassPath::new(root.path().to_path_buf());
    let mut loader = ClassLoader::new(class_path, &mut store);
    let symbol = loader
        .load_class(&BinaryName::from_internal("C"))
        .expect("C should load");
    drop(loader);

    let annotations = store.symbols.get(symbol).annotations.clone();
    (store, annotations)
}

/// Completes `SYMBOL_ANNOTATED_X`'s annotations through a real
/// `TastyUnpickler`, in its own store, and returns the store plus its
/// `AnnotationId`s (the TASTy side of the convergence contract; see
/// `dotty-tasty-unpickler/tests/symbol_annotations.rs` for that adapter's
/// own, more thorough coverage).
fn tasty_side() -> (SemanticStore, Vec<dotty_core::ids::AnnotationId>) {
    let file = TastyFile::parse_scala_3_9(SYMBOL_ANNOTATED).unwrap();
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let mut packages = Packages::new();
    // `SymbolMarker`, the fixture's own annotation class, is defined in a
    // sibling unit this test does not enter — stood in for exactly as
    // `tests/symbol_annotations.rs` does.
    let package = packages
        .enter(
            &mut store,
            dotty_core::symbols::SymbolOrigin::Synthetic,
            &["me", "cytrowski", "tastyfixtures", "semantic"],
        )
        .pop()
        .unwrap();
    let name = dotty_core::names::Name::new(
        store.names.intern("SymbolMarker"),
        dotty_core::names::Namespace::Type,
    );
    let marker = store.symbols.alloc(dotty_core::symbols::Symbol {
        name,
        owner: Some(package.symbol),
        kind: dotty_core::symbols::SymbolKind::Class,
        flags: dotty_core::symbols::SymbolFlags::EMPTY,
        visibility: dotty_core::symbols::Visibility::Public,
        info: SymbolInfo::Missing,
        origin: dotty_core::symbols::SymbolOrigin::Synthetic,
        annotations: Vec::new(),
        position: None,
        links: dotty_core::symbols::SymbolLinks::default(),
    });
    store.scopes.get_mut(package.scope).enter(name, marker);

    let mut unpickler = TastyUnpickler::with_packages(&file, &mut store, definitions, packages);
    unpickler.enter_symbols().unwrap();
    let annotations = unpickler
        .complete_symbol_annotations(SYMBOL_ANNOTATED_X)
        .unwrap();
    (store, annotations)
}

#[test]
fn both_adapters_attach_annotation_ids_to_symbol_annotations() {
    let (_classloader_store, classfile_annotations) = classloader_side();
    let (_tasty_store, tasty_annotations) = tasty_side();

    assert_eq!(classfile_annotations.len(), 1);
    assert_eq!(tasty_annotations.len(), 1);
}

#[test]
fn both_adapters_represent_an_absent_typed_tree_as_none() {
    let (classloader_store, classfile_annotations) = classloader_side();
    let (tasty_store, tasty_annotations) = tasty_side();

    let classfile_annotation = classloader_store.annotations.get(classfile_annotations[0]);
    let tasty_annotation = tasty_store.annotations.get(tasty_annotations[0]);
    assert_eq!(classfile_annotation.tree, None);
    assert_eq!(tasty_annotation.tree, None);
}

#[test]
fn both_adapters_name_a_class_like_annotation_type() {
    let (classloader_store, classfile_annotations) = classloader_side();
    let (tasty_store, tasty_annotations) = tasty_side();

    let classfile_annotation = classloader_store.annotations.get(classfile_annotations[0]);
    let tasty_annotation = tasty_store.annotations.get(tasty_annotations[0]);
    assert!(matches!(
        classloader_store.types.get(classfile_annotation.ty),
        Type::TypeRef { .. }
    ));
    assert!(matches!(
        tasty_store.types.get(tasty_annotation.ty),
        Type::TypeRef { .. }
    ));
}

/// The one documented divergence (issue #129 §33 explicitly allows this: "do
/// not require equal payloads"): the classloader adapter does not yet map a
/// classfile's decoded element values into `AnnotationArguments::Known` (it
/// keeps them exclusively in its own JVM-facing `SemanticAnnotation`
/// sidecar, per `annotation.rs`'s own doc comment), so every classfile
/// annotation is `Unavailable` regardless of its real element count, while
/// TASTy's argument-free `@SymbolMarker` is `Known([])` — a genuinely
/// different, but equally valid, answer to "were there any arguments".
#[test]
fn the_two_adapters_diverge_on_argument_availability_by_design() {
    let (classloader_store, classfile_annotations) = classloader_side();
    let (tasty_store, tasty_annotations) = tasty_side();

    let classfile_annotation = classloader_store.annotations.get(classfile_annotations[0]);
    let tasty_annotation = tasty_store.annotations.get(tasty_annotations[0]);
    assert_eq!(
        classfile_annotation.arguments,
        AnnotationArguments::Unavailable
    );
    assert_eq!(
        tasty_annotation.arguments,
        AnnotationArguments::Known(Vec::new())
    );
}
