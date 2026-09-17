//! End-to-end `.tasty`-backed loading through a real `ClassLoader` over a
//! real `DirectoryClassPath` (`docs/classloader.md` §9, Milestone 9),
//! using the real `scalac`-compiled fixtures under
//! `tests/fixtures/tasty_sample/` (copied from `dotty-tasty`'s own
//! fixture tree). Complements `directory_class_path.rs`, which only
//! tests `.tasty`-over-`.class` preference at the `ClassPathEntry`
//! level, and `tasty_symbol`'s own unit tests, which only test decoding
//! in isolation.

use dotty_classloader::classloader::{
    BinaryName, ClassLoader, ClassRef, CompositeClassPath, DirectoryClassPath,
};
use std::fs;
use std::path::{Path, PathBuf};

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
    bytes.extend_from_slice(&[0x00, 0x45]); // major = 69 (JDK 25)
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

fn class_loader_over_tasty_sample(
    name: &str,
) -> (ClassLoader<CompositeClassPath>, TemporaryDirectory) {
    let synthetic_jdk = TemporaryDirectory::new(name);
    write_synthetic_object_class(synthetic_jdk.path());

    let class_path = CompositeClassPath::new(vec![
        Box::new(DirectoryClassPath::new(tasty_sample_dir())),
        Box::new(DirectoryClassPath::new(synthetic_jdk.path().clone())),
    ]);
    (ClassLoader::new(class_path), synthetic_jdk)
}

#[test]
fn loads_dog_with_animal_resolved_through_tasty_as_a_real_interface() {
    let (loader, _synthetic_jdk) =
        class_loader_over_tasty_sample("tasty-loading-dog-synthetic-jdk");

    let dog = loader
        .load_class(&BinaryName::from_internal("Dog"))
        .expect("Dog should load from its real .tasty fixture");

    assert!(!dog.flags().is_interface());
    assert!(matches!(
        dog.super_class(),
        Some(ClassRef::Resolved(super_symbol))
            if super_symbol.name().as_internal() == "java/lang/Object"
    ));
    assert!(matches!(
        dog.interfaces().as_slice(),
        [ClassRef::Resolved(interface_symbol)]
            if interface_symbol.name().as_internal() == "Animal"
                && interface_symbol.flags().is_interface()
    ));
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
    let (loader, _synthetic_jdk) =
        class_loader_over_tasty_sample("tasty-loading-animal-synthetic-jdk");

    let animal = loader
        .load_class(&BinaryName::from_internal("Animal"))
        .expect("Animal should load from its real .tasty fixture, not the co-located dummy .class");

    assert!(animal.flags().is_interface());
    assert!(animal.interfaces().is_empty());
    assert!(matches!(
        animal.super_class(),
        Some(ClassRef::Resolved(super_symbol))
            if super_symbol.name().as_internal() == "java/lang/Object"
    ));
}
