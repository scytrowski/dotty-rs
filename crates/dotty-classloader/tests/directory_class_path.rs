use dotty_classloader::classloader::{BinaryName, ClassFormat, ClassPathEntry, DirectoryClassPath};
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
