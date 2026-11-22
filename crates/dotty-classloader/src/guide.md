## Supply an explicit classpath and handle a missing class

A classpath can be assembled without probing the host JDK. This example uses
an empty composite and therefore reports a typed missing-artifact failure,
with no filesystem dependency.

```rust
use dotty_core::{Definitions, SemanticStore};
use dotty_classloader::classloader::{
    BinaryName, ClassLoadCategory, ClassLoader, CompositeClassPath, LoadingSession,
};

let mut store = SemanticStore::new();
let definitions = Definitions::bootstrap(&mut store);
let classpath = CompositeClassPath::new(Vec::new());
let mut loader = ClassLoader::with_definitions(
    classpath, &mut store, definitions, LoadingSession::new(),
);
let failure = loader.load_class_detailed(&BinaryName::from_qualified("example.Missing"))
    .expect_err("empty classpath cannot contain the class");
assert_eq!(failure.category(), ClassLoadCategory::Missing);
assert!(failure.origin.is_none());
let session = loader.into_session();
// Retain this session with store and definitions when constructing another loader.
# let _ = session;
```

Use directory, JAR, JMOD, or JDK entries when artifacts are available. Composite
entry order determines lookup precedence; a resource with both formats prefers
TASTy. JAR construction reads and indexes the archive and can fail before any
class is requested. Untrusted binary names must pass path-safety checks before
being mapped onto filesystem or archive paths.

Bootstrap definitions once. Carry [`classloader::LoadingSession`] between
loaders sharing a store so package, class, and builtin identities remain
canonical. A failure in a requested class can wrap a dependency failure;
[`classloader::ClassLoadFailure::root_cause`] retains that distinction together
with the deepest selected resource origin when available. Optional profiling
counters and inclusive timings describe work rather than semantic guarantees.
