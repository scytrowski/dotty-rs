use std::fs;
use std::path::PathBuf;
use std::process::Command;

struct TemporaryDirectory(PathBuf);

impl TemporaryDirectory {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("dotty-tasty-{name}-{}", std::process::id()));
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

#[test]
fn tasty_roundtrip_binary_reencodes_a_fixture() {
    let output = TemporaryDirectory::new("roundtrip-test");
    let input = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("crates/dotty-tasty/tests/fixtures/simple_def");

    let status = Command::new(env!("CARGO_BIN_EXE_tasty-roundtrip"))
        .args(["--mode=structured"])
        .arg(&input)
        .arg(output.path())
        .status()
        .expect("tasty-roundtrip binary should start");

    assert!(status.success());
    assert_eq!(
        fs::read(output.path().join("SimpleDef.tasty")).expect("round-tripped fixture exists"),
        fs::read(input.join("SimpleDef.tasty")).expect("source fixture exists")
    );
}
