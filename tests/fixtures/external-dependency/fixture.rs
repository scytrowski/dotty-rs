//! A directory classpath shared by the two external client tests.

use dotty::classloader::{CompositeClassPath, DirectoryClassPath, JmodClassPath};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
};

pub struct ExternalDependency {
    pub directory: PathBuf,
    pub java_base: PathBuf,
}

impl ExternalDependency {
    pub fn compile() -> Self {
        let root = Path::new(env!("CARGO_TARGET_TMPDIR"));
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let directory = root.join(format!(
            "demo-external-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&directory).unwrap();
        let jdk = PathBuf::from(std::env::var_os("JAVA_HOME").expect("set JAVA_HOME to JDK 21"));
        let fixture = Self {
            directory,
            java_base: jdk.join("jmods/java.base.jmod"),
        };
        let output = Command::new(jdk.join("bin/javac"))
            .args(["--release", "21", "-g:none", "-d"])
            .arg(&fixture.directory)
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../external-dependency/External.java"))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "javac: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        fixture
    }

    pub fn classpath(&self) -> CompositeClassPath {
        CompositeClassPath::new(vec![
            Box::new(DirectoryClassPath::new(&self.directory)),
            Box::new(JmodClassPath::new(&self.java_base).unwrap()),
        ])
    }

    pub fn remove_external(&self) {
        fs::remove_file(self.directory.join("demo/External.class")).unwrap();
    }
}

impl Drop for ExternalDependency {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}
