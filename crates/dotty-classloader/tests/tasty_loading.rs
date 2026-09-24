//! End-to-end `.tasty`-backed loading through a real `ClassLoader` over a
//! real `DirectoryClassPath` (`docs/classloader.md` §9, Milestone 9),
//! using the real `scalac`-compiled fixtures under
//! `tests/fixtures/tasty_sample/` (copied from `dotty-tasty`'s own
//! fixture tree). Complements `directory_class_path.rs`, which only
//! tests `.tasty`-over-`.class` preference at the `ClassPathEntry`
//! level, and `tasty_symbol`'s own unit tests, which only test decoding
//! in isolation.

use dotty_classloader::classloader::{
    BinaryName, ClassLoader, CompositeClassPath, DirectoryClassPath,
};
use dotty_core::{
    ClassInfo, Name, Namespace, SemanticStore, SymbolId, SymbolInfo, SymbolKind, Type, TypeId,
    Visibility,
};
use std::fs;
use std::path::{Path, PathBuf};

#[path = "support/cross_adapter_annotations.rs"]
mod cross_adapter_annotations;

struct TemporaryDirectory(PathBuf);

impl TemporaryDirectory {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("dotty-classloader-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("temporary directory should be creatable");
        Self(path)
    }

    fn path(&self) -> &PathBuf {
        &self.0
    }
}

impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn tasty_sample_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tasty_sample")
}

/// A minimal, hand-built, clearly-synthetic `java/lang/Object` class
/// file: none of this crate's real fixtures define `Object` itself, and
/// a real one isn't needed here — only its existence as a loadable,
/// no-superclass, no-interfaces class matters, to let `Dog`/`Animal`'s
/// implicit superclass resolve successfully. Written under its own
/// temporary classpath root, kept separate from `tasty_sample/`'s real
/// `scalac` output.
fn write_synthetic_object_class(root: &Path) {
    let name = b"java/lang/Object";
    let mut pool = Vec::new();
    pool.push(1); // CONSTANT_Utf8 #1
    pool.extend_from_slice(&(name.len() as u16).to_be_bytes());
    pool.extend_from_slice(name);
    pool.push(7); // CONSTANT_Class #2 -> #1
    pool.extend_from_slice(&1u16.to_be_bytes());

    let mut bytes = Vec::new();
    bytes.extend_from_slice(&[0xCA, 0xFE, 0xBA, 0xBE]);
    bytes.extend_from_slice(&[0x00, 0x00]); // minor
    bytes.extend_from_slice(&[0x00, 0x45]); // class-file major = 69
    bytes.extend_from_slice(&[0x00, 0x03]); // constant_pool_count
    bytes.extend_from_slice(&pool);
    bytes.extend_from_slice(&[0x00, 0x21]); // access_flags
    bytes.extend_from_slice(&2u16.to_be_bytes()); // this_class -> #2
    bytes.extend_from_slice(&0u16.to_be_bytes()); // super_class (none)
    bytes.extend_from_slice(&[0x00, 0x00]); // interfaces_count
    bytes.extend_from_slice(&[0x00, 0x00]); // fields_count
    bytes.extend_from_slice(&[0x00, 0x00]); // methods_count
    bytes.extend_from_slice(&[0x00, 0x00]); // attributes_count

    fs::create_dir_all(root.join("java/lang")).unwrap();
    fs::write(root.join("java/lang/Object.class"), bytes).unwrap();
}

fn class_loader_over_tasty_sample<'store>(
    name: &str,
    store: &'store mut SemanticStore,
) -> (ClassLoader<'store, CompositeClassPath>, TemporaryDirectory) {
    let synthetic_jdk = TemporaryDirectory::new(name);
    write_synthetic_object_class(synthetic_jdk.path());

    let class_path = CompositeClassPath::new(vec![
        Box::new(DirectoryClassPath::new(tasty_sample_dir())),
        Box::new(DirectoryClassPath::new(synthetic_jdk.path().clone())),
    ]);
    (ClassLoader::new(class_path, store), synthetic_jdk)
}

/// The class's `Type::ClassInfo`, read back through its `Symbol`'s
/// `SymbolInfo::Complete`.
fn class_info(store: &SemanticStore, id: SymbolId) -> &ClassInfo {
    let symbol = store.symbols.get(id);
    let SymbolInfo::Complete(info_id) = symbol.info else {
        panic!("expected the class to have complete info");
    };
    let Type::ClassInfo(class_info) = store.types.get(info_id) else {
        panic!("expected a ClassInfo");
    };
    class_info
}

/// The `SymbolId` a `Type::TypeRef` (a `ClassInfo` parent) points at.
fn parent_symbol(store: &SemanticStore, ty: TypeId) -> SymbolId {
    let Some(symbol) = store.types.get(ty).reference_symbol() else {
        panic!("expected a TypeRef");
    };
    symbol
}

/// `Dog.tasty`'s `Animal` mixin is a real, post-typecheck reference
/// carrying its own compiled `TERMREFpkg` prefix
/// (`me.cytrowski.tastyfixtures`, this fixture's real source package —
/// see `tasty_symbol::resolve_reference_name`), so `Animal` is looked up
/// at that real qualified path, not bare — `tasty_sample/me/cytrowski/
/// tastyfixtures/Animal.tasty` is a second copy of the same
/// `Animal.tasty` bytes, alongside the pre-existing root-level
/// `Animal.tasty`/`Animal.class` the tests below still load directly by
/// their own bare request.
#[test]
fn loads_dog_with_animal_resolved_through_tasty_as_a_real_interface() {
    let mut store = SemanticStore::new();
    let (mut loader, _synthetic_jdk) =
        class_loader_over_tasty_sample("tasty-loading-dog-synthetic-jdk", &mut store);

    let dog = loader
        .load_class(&BinaryName::from_internal("Dog"))
        .expect("Dog should load from its real .tasty fixture");
    drop(loader);

    assert_eq!(store.symbols.get(dog).kind, SymbolKind::Class);
    let info = class_info(&store, dog);
    assert_eq!(info.parents.len(), 2);

    let super_symbol = parent_symbol(&store, info.parents[0]);
    assert_eq!(
        store
            .names
            .resolve(store.symbols.get(super_symbol).name.text()),
        "Object"
    );

    let interface_symbol = parent_symbol(&store, info.parents[1]);
    assert_eq!(
        store
            .names
            .resolve(store.symbols.get(interface_symbol).name.text()),
        "Animal"
    );
    assert_eq!(store.symbols.get(interface_symbol).kind, SymbolKind::Trait);
}

/// `tasty_sample/` also has a co-located dummy `Animal.class` (real
/// bytes, but for an unrelated class — see `directory_class_path.rs`'s
/// preference tests). If `.tasty`-over-`.class` preference broke and
/// this dummy file were decoded instead, `ClassLoader` would report
/// `NameMismatch` (its `this_class` is not `Animal`) rather than
/// produce this trait's real shape — so this test's success also
/// exercises that preference end-to-end through the full loader, not
/// just `ClassPathEntry::find_class` in isolation.
#[test]
fn loads_animal_directly_as_an_interface_with_no_declared_interfaces() {
    let mut store = SemanticStore::new();
    let (mut loader, _synthetic_jdk) =
        class_loader_over_tasty_sample("tasty-loading-animal-synthetic-jdk", &mut store);

    let animal = loader
        .load_class(&BinaryName::from_internal("Animal"))
        .expect("Animal should load from its real .tasty fixture, not the co-located dummy .class");
    drop(loader);

    assert_eq!(store.symbols.get(animal).kind, SymbolKind::Trait);
    let info = class_info(&store, animal);
    assert_eq!(info.parents.len(), 1);

    let super_symbol = parent_symbol(&store, info.parents[0]);
    assert_eq!(
        store
            .names
            .resolve(store.symbols.get(super_symbol).name.text()),
        "Object"
    );
}

fn tasty_visibility_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tasty_visibility")
}

/// Loads `class` (a class of the `visibility` fixture package) and returns
/// its symbol.
fn load_visibility_class(store: &mut SemanticStore, class: &str) -> SymbolId {
    let synthetic_jdk = TemporaryDirectory::new(&format!("tasty-visibility-{class}"));
    write_synthetic_object_class(synthetic_jdk.path());
    let class_path = CompositeClassPath::new(vec![
        Box::new(DirectoryClassPath::new(tasty_visibility_dir())),
        Box::new(DirectoryClassPath::new(synthetic_jdk.path().clone())),
    ]);
    let mut loader = ClassLoader::new(class_path, store);
    loader
        .load_class(&BinaryName::from_internal(format!(
            "me/cytrowski/tastyfixtures/visibility/{class}"
        )))
        .unwrap_or_else(|error| panic!("{class} should load: {error:?}"))
}

/// The package `levels_up` owners above the class (0 is its own package).
fn package_above(store: &SemanticStore, class: SymbolId, levels_up: usize) -> SymbolId {
    let mut symbol = store
        .symbols
        .get(class)
        .owner
        .expect("a class has a package");
    for _ in 0..levels_up {
        symbol = store
            .symbols
            .get(symbol)
            .owner
            .expect("an enclosing package");
    }
    symbol
}

/// Issue #16: `private[X]` on a `.tasty` class used to leave it `Public`.
#[test]
fn a_class_private_to_its_own_package_is_private_within_that_package() {
    let mut store = SemanticStore::new();

    let class = load_visibility_class(&mut store, "InOwnPackage");

    assert_eq!(
        store.symbols.get(class).visibility,
        Visibility::PrivateWithin(package_above(&store, class, 0))
    );
}

#[test]
fn a_class_private_to_an_enclosing_package_is_private_within_that_package() {
    let mut store = SemanticStore::new();

    let in_parent = load_visibility_class(&mut store, "InEnclosingPackage");
    let in_outer = load_visibility_class(&mut store, "InOuterPackage");

    // `visibility` is one level below `tastyfixtures`, and two below `me`'s
    // child `cytrowski`: me / cytrowski / tastyfixtures / visibility.
    assert_eq!(
        store.symbols.get(in_parent).visibility,
        Visibility::PrivateWithin(package_above(&store, in_parent, 1))
    );
    assert_eq!(
        store.symbols.get(in_outer).visibility,
        Visibility::PrivateWithin(package_above(&store, in_outer, 3))
    );
}

#[test]
fn a_top_level_private_class_is_private_within_its_package() {
    let mut store = SemanticStore::new();

    let class = load_visibility_class(&mut store, "PlainPrivate");

    assert_eq!(
        store.symbols.get(class).visibility,
        Visibility::PrivateWithin(package_above(&store, class, 0))
    );
}

#[test]
fn a_class_with_no_access_modifier_stays_public() {
    let mut store = SemanticStore::new();

    let class = load_visibility_class(&mut store, "Open");

    assert_eq!(store.symbols.get(class).visibility, Visibility::Public);
}

fn tasty_fields_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tasty_fields")
}

/// The names of the fields the loader entered into the declarations scope of
/// `class` (of the `fields` fixture package), for each of `probes`.
fn entered_fields<'a>(class: &str, probes: &[&'a str]) -> Vec<&'a str> {
    let synthetic_jdk = TemporaryDirectory::new(&format!("tasty-fields-{class}"));
    write_synthetic_object_class(synthetic_jdk.path());
    let class_path = CompositeClassPath::new(vec![
        Box::new(DirectoryClassPath::new(tasty_fields_dir())),
        Box::new(DirectoryClassPath::new(synthetic_jdk.path().clone())),
    ]);
    let mut store = SemanticStore::new();
    let mut loader = ClassLoader::new(class_path, &mut store);
    let symbol = loader
        .load_class(&BinaryName::from_internal(format!(
            "me/cytrowski/tastyfixtures/fields/{class}"
        )))
        .unwrap_or_else(|error| panic!("{class} should load: {error:?}"));
    drop(loader);

    let scope = class_info(&store, symbol).declarations;
    probes
        .iter()
        .copied()
        .filter(|probe| {
            let text = store.names.intern(probe);
            store
                .scopes
                .get(scope)
                .lookup(&Name::new(text, Namespace::Term))
                .is_some()
        })
        .collect()
}

/// Issue #11: a `val`/`var` constructor parameter is a field of a class
/// loaded from `.tasty`, as it is when loaded from `.class`; a plain
/// parameter is not.
#[test]
fn val_and_var_constructor_parameters_are_entered_as_fields() {
    assert_eq!(entered_fields("P", &["a", "b", "c"]), vec!["a", "c"]);
    assert_eq!(entered_fields("Q", &["d", "e"]), vec!["d", "e"]);
    assert_eq!(entered_fields("Body", &["x", "inBody"]), vec!["inBody"]);
}

/// The `ClassInfo` contract both adapters build (docs/dotty-core-design.md):
/// the canonical `no_prefix`, the class itself, and a declaration scope owned
/// by that class. Parent graphs may differ (TASTy keeps richer types).
fn assert_core_class_info_contract(
    store: &SemanticStore,
    definitions: dotty_core::Definitions,
    class: SymbolId,
) {
    let info = class_info(store, class);
    assert_eq!(info.prefix, definitions.no_prefix);
    assert_eq!(info.class, class);
    assert_eq!(store.scopes.get(info.declarations).owner, Some(class));
    for parent in &info.parents {
        assert!(store.types.get(*parent).reference_symbol().is_some());
    }
    assert_eq!(info.self_type, None);
}

/// Declares `names` as unresolved-until-needed classes of the package `path`,
/// as the unpickler's own tests do for the classes a fixture mentions.
fn stub_classes(
    store: &mut SemanticStore,
    packages: &mut dotty_core::Packages,
    path: &[&str],
    names: &[&str],
) {
    use dotty_core::{Symbol, SymbolFlags, SymbolLinks, SymbolOrigin, Visibility};
    let package = packages
        .enter(store, SymbolOrigin::Synthetic, path)
        .pop()
        .unwrap();
    for class in names {
        let name = Name::new(store.names.intern(class), Namespace::Type);
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

#[test]
fn classloader_and_tasty_unpickler_class_infos_share_one_core_contract() {
    // `InfoBase` is a parameterized trait with no self type; `Dog` is loaded
    // by the classloader from its own `.tasty`.
    const INFO_BASE: &[u8] =
        include_bytes!("../../dotty-tasty-unpickler/tests/fixtures/semantic/InfoBase.tasty");

    let mut store = SemanticStore::new();
    let (mut loader, _synthetic_jdk) =
        class_loader_over_tasty_sample("tasty-loading-convergence-synthetic-jdk", &mut store);
    let dog = loader
        .load_class(&BinaryName::from_internal("Dog"))
        .expect("Dog should load");
    let definitions = loader.definitions();
    drop(loader);

    let mut packages = dotty_core::Packages::new();
    stub_classes(&mut store, &mut packages, &["scala"], &["Any", "Nothing"]);
    stub_classes(&mut store, &mut packages, &["java", "lang"], &["Object"]);
    let file = dotty_tasty::tasty::TastyFile::parse_scala_3_9(INFO_BASE).unwrap();
    let mut unpickler = dotty_tasty_unpickler::tasty_unpickler::TastyUnpickler::with_packages(
        &file,
        &mut store,
        definitions,
        packages,
    );
    unpickler.enter_symbols().unwrap();
    let definition = file
        .ast_address_index()
        .unwrap()
        .iter_nodes()
        .filter(|node| node.tag == 131)
        .map(|node| u32::try_from(node.offset).unwrap())
        .min()
        .unwrap();
    unpickler
        .complete_symbol(definition)
        .expect("InfoBase completes");
    let base = unpickler.index().symbol_at(definition).unwrap();
    drop(unpickler);

    assert_core_class_info_contract(&store, definitions, dog);
    assert_core_class_info_contract(&store, definitions, base);
}
