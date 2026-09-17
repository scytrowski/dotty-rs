use dotty_classloader::classloader::{BinaryName, ClassPathEntry, DirectoryClassPath};
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
