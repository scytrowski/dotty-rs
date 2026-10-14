use dotty_classloader::classloader::{
    BinaryName, ClassFormat, ClassLoadCategory, ClassLoader, ClassOrigin, ClassPathEntry,
    DirectoryClassPath,
};
use dotty_core::SemanticStore;
use std::fs;
use std::path::PathBuf;

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

fn pool_sample_bytes() -> Vec<u8> {
    fs::read(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../dotty-classfile/tests/fixtures/pool_sample/PoolSample.class"),
    )
    .expect("PoolSample.class fixture should exist")
}

#[test]
fn finds_a_class_file_present_under_the_root() {
    let root = TemporaryDirectory::new("directory-class-path-found");
    fs::write(root.path().join("PoolSample.class"), pool_sample_bytes()).unwrap();

    let class_path = DirectoryClassPath::new(root.path().clone());
    let resource = class_path
        .find_class(&BinaryName::from_internal("PoolSample"))
        .expect("lookup should not fail")
        .expect("class should be found");

    assert_eq!(resource.bytes(), pool_sample_bytes().as_slice());
    assert_eq!(resource.format(), ClassFormat::Class);
}

#[test]
fn detailed_load_failure_retains_directory_resource_provenance() {
    let root = TemporaryDirectory::new("directory-class-path-malformed-class");
    fs::write(root.path().join("Broken.class"), [0x00, 0x01, 0x02]).unwrap();
    let mut store = SemanticStore::new();
    let mut loader = ClassLoader::new(DirectoryClassPath::new(root.path().clone()), &mut store);

    let failure = loader
        .load_class_detailed(&BinaryName::from_internal("Broken"))
        .unwrap_err();

    assert_eq!(failure.code(), "classloader.invalid_class_file");
    assert_eq!(failure.category(), ClassLoadCategory::MalformedArtifact);
    assert_eq!(
        failure.origin,
        Some(ClassOrigin::Directory(root.path().clone()))
    );
    assert_eq!(failure.format, Some(ClassFormat::Class));
}

fn animal_tasty_bytes() -> Vec<u8> {
    fs::read(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tasty_sample/Animal.tasty"),
    )
    .expect("Animal.tasty fixture should exist")
}

#[test]
fn prefers_a_tasty_entry_over_a_class_entry_for_the_same_name() {
    let root = TemporaryDirectory::new("directory-class-path-tasty-preferred");
    fs::write(root.path().join("Animal.tasty"), animal_tasty_bytes()).unwrap();
    fs::write(root.path().join("Animal.class"), pool_sample_bytes()).unwrap();

    let class_path = DirectoryClassPath::new(root.path().clone());
    let resource = class_path
        .find_class(&BinaryName::from_internal("Animal"))
        .expect("lookup should not fail")
        .expect("class should be found");

    assert_eq!(resource.bytes(), animal_tasty_bytes().as_slice());
    assert_eq!(resource.format(), ClassFormat::Tasty);
}

#[test]
fn falls_back_to_class_when_no_tasty_entry_exists() {
    let root = TemporaryDirectory::new("directory-class-path-tasty-fallback");
    fs::write(root.path().join("PoolSample.class"), pool_sample_bytes()).unwrap();

    let class_path = DirectoryClassPath::new(root.path().clone());
    let resource = class_path
        .find_class(&BinaryName::from_internal("PoolSample"))
        .expect("lookup should not fail")
        .expect("class should be found");

    assert_eq!(resource.bytes(), pool_sample_bytes().as_slice());
    assert_eq!(resource.format(), ClassFormat::Class);
}

#[test]
fn returns_none_for_a_missing_class_without_erroring() {
    let root = TemporaryDirectory::new("directory-class-path-missing");

    let class_path = DirectoryClassPath::new(root.path().clone());
    let resource = class_path
        .find_class(&BinaryName::from_internal("does/not/Exist"))
        .expect("lookup should not fail");

    assert!(resource.is_none());
}

#[cfg(unix)]
#[test]
fn counts_a_symlinked_class_file_as_package_evidence() {
    use std::os::unix::fs::symlink;

    let workspace = TemporaryDirectory::new("directory-class-path-symlink-workspace");
    let root = workspace.path().join("root");
    let package = root.join("pool");
    fs::create_dir_all(&package).unwrap();
    let target = workspace.path().join("PoolSample.class");
    fs::write(&target, pool_sample_bytes()).unwrap();
    symlink(target, package.join("PoolSample.class")).unwrap();

    let class_path = DirectoryClassPath::new(root);

    assert!(class_path.contains_package(&["pool"]).unwrap());
    assert!(
        class_path
            .find_class(&BinaryName::from_internal("pool/PoolSample"))
            .unwrap()
            .is_some()
    );
}

#[test]
fn rejects_a_path_traversal_name_instead_of_escaping_the_root() {
    let workspace = TemporaryDirectory::new("directory-class-path-traversal-workspace");
    fs::write(
        workspace.path().join("secret.class"),
        b"outside the classpath root",
    )
    .unwrap();

    let root = workspace.path().join("root");
    fs::create_dir_all(&root).unwrap();
    let class_path = DirectoryClassPath::new(root);

    let error = class_path
        .find_class(&BinaryName::from_internal("../secret"))
        .expect_err("a path-traversal name must be rejected, not resolved");

    assert!(error.to_string().contains("not safe to use as a path"));
}

#[test]
fn rejects_an_absolute_path_name_instead_of_escaping_the_root() {
    let secret = TemporaryDirectory::new("directory-class-path-absolute-secret");
    fs::write(secret.path().join("secret.class"), b"outside the root").unwrap();
    let absolute_name = secret.path().join("secret").to_string_lossy().into_owned();

    let root = TemporaryDirectory::new("directory-class-path-absolute-root");
    let class_path = DirectoryClassPath::new(root.path().clone());

    let error = class_path
        .find_class(&BinaryName::from_internal(absolute_name))
        .expect_err("an absolute name must be rejected, not resolved");

    assert!(error.to_string().contains("not safe to use as a path"));
}

#[test]
fn resolves_a_nested_package_path_under_the_root() {
    let root = TemporaryDirectory::new("directory-class-path-nested");
    fs::create_dir_all(root.path().join("java/lang")).unwrap();
    fs::write(
        root.path().join("java/lang/PoolSample.class"),
        pool_sample_bytes(),
    )
    .unwrap();

    let class_path = DirectoryClassPath::new(root.path().clone());
    let resource = class_path
        .find_class(&BinaryName::from_internal("java/lang/PoolSample"))
        .expect("lookup should not fail")
        .expect("class should be found");

    assert_eq!(resource.bytes(), pool_sample_bytes().as_slice());
}

#[test]
fn package_prefix_finds_class_resources_in_nested_packages_without_changing_direct_checks() {
    let root = TemporaryDirectory::new("directory-package-prefix-descendant");
    fs::create_dir_all(root.path().join("a/b/c")).unwrap();
    fs::create_dir_all(root.path().join("a/beta")).unwrap();
    fs::create_dir_all(root.path().join("a/empty")).unwrap();
    fs::write(root.path().join("a/b/c/Leaf.class"), b"class bytes").unwrap();
    fs::write(root.path().join("a/beta/Only.scala"), b"source only").unwrap();
    fs::write(root.path().join("a/beta/Only.txt"), b"not a class resource").unwrap();
    fs::write(root.path().join("a/beta/Only.jar"), b"not a class resource").unwrap();

    let class_path = DirectoryClassPath::new(root.path().clone());

    assert!(!class_path.contains_package(&["a"]).unwrap());
    assert!(!class_path.contains_package(&["a", "b"]).unwrap());
    assert!(class_path.contains_package_prefix(&["a"]).unwrap());
    assert!(class_path.contains_package_prefix(&["a", "b"]).unwrap());
    assert!(
        class_path
            .contains_package_prefix(&["a", "b", "c"])
            .unwrap()
    );
    assert!(!class_path.contains_package_prefix(&["a", "d"]).unwrap());
    assert!(!class_path.contains_package_prefix(&["a", "beta"]).unwrap());
    assert!(!class_path.contains_package_prefix(&["a", "empty"]).unwrap());
    assert!(class_path.contains_package_prefix(&[]).unwrap());
}

#[test]
fn package_prefix_rejects_unsafe_segments() {
    let root = TemporaryDirectory::new("directory-package-prefix-unsafe");
    fs::create_dir_all(root.path().join("safe")).unwrap();
    fs::write(root.path().join("safe/Leaf.class"), b"class bytes").unwrap();

    let class_path = DirectoryClassPath::new(root.path().clone());

    for package in [
        &["..", "safe"][..],
        &["safe/.."],
        &["safe\\.."],
        &["safe:else"],
        &["safe\0else"],
    ] {
        assert!(!class_path.contains_package_prefix(package).unwrap());
    }
}

#[cfg(unix)]
#[test]
fn package_prefix_does_not_follow_symlinked_files_or_directories() {
    use std::os::unix::fs::symlink;

    let workspace = TemporaryDirectory::new("directory-package-prefix-symlinks");
    let root = workspace.path().join("root");
    let package = root.join("pkg");
    fs::create_dir_all(&package).unwrap();
    let outside = workspace.path().join("outside");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("Escaped.class"), b"outside class").unwrap();
    symlink(outside.join("Escaped.class"), package.join("Escaped.class")).unwrap();
    symlink(&package, package.join("loop")).unwrap();
    symlink(
        workspace.path().join("missing.class"),
        package.join("Broken.class"),
    )
    .unwrap();

    let class_path = DirectoryClassPath::new(root);

    assert!(!class_path.contains_package_prefix(&["pkg"]).unwrap());
}
