//! JDK-directory path behavior that crosses the JMOD and resolver adapters.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use dotty_classloader::classloader::{
    BinaryName, ClassPathEntry, ClassPathError, ClassResource, ClasspathSymbolResolver,
    JdkClassPath, LoadingSession,
};
use dotty_core::{Definitions, SemanticStore, SymbolKind, SymbolResolver};

struct CountingClassPath {
    inner: JdkClassPath,
    class_reads: Arc<AtomicUsize>,
}

impl ClassPathEntry for CountingClassPath {
    fn find_class(&self, name: &BinaryName) -> Result<Option<ClassResource>, ClassPathError> {
        self.class_reads.fetch_add(1, Ordering::SeqCst);
        self.inner.find_class(name)
    }

    fn contains_package(&self, package: &[&str]) -> Result<bool, ClassPathError> {
        self.inner.contains_package(package)
    }

    fn contains_package_prefix(&self, package: &[&str]) -> Result<bool, ClassPathError> {
        self.inner.contains_package_prefix(package)
    }
}

#[test]
fn package_evidence_does_not_read_or_materialize_a_class() {
    let jmods = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/jdk_classpath");
    let class_reads = Arc::new(AtomicUsize::new(0));
    let class_path = CountingClassPath {
        inner: JdkClassPath::new(jmods).unwrap(),
        class_reads: Arc::clone(&class_reads),
    };
    let mut store = SemanticStore::new();
    let definitions = Definitions::bootstrap(&mut store);
    let mut resolver = ClasspathSymbolResolver::new(class_path, definitions, LoadingSession::new());

    let package = resolver
        .resolve_package(&mut store, &["pool"])
        .unwrap()
        .expect("JMOD class resources are positive package evidence");

    assert_eq!(store.symbols.get(package).kind, SymbolKind::Package);
    assert_eq!(class_reads.load(Ordering::SeqCst), 0);
}
