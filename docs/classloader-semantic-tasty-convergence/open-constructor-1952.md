# Frozen `Open` constructor qualification

This report records the focused follow-up to the #1948 semantic comparison.
The historical `comparison.tsv` and `comparison-1948.tsv` remain untouched.

## Inputs and provenance

- Probe base revision: `b2c7fee27ef2dec416706f59abda050be35d2c4b` (#1951 merged).
- Probe test commit validated locally: `bad6e58698faafd6fcddf20bc3bd69f5b044f09f`.
- Published GitHub test commit: `c38bd34e31bf4e2b8b2c0e98fbbeb466b113c7ec`.
- Fixture: `crates/dotty-classloader/tests/fixtures/tasty_visibility/me/cytrowski/tastyfixtures/visibility/Open.tasty`.
- Fixture SHA-256: `49286e74a3d3693b08d2a29575f23089f6be5c486231390059f43c19a8929894`.
- Scala dependencies use the checked-in `tasty_sample`, `tasty_fields`,
  `tasty_visibility`, `scala3-library`, and `path_dependent` fixture roots
  already used by the semantic parity test. The resolver also receives the
  actual JDK module path from `JAVA_HOME/jmods`; no synthetic classes are
  added.
- JDK provenance: Eclipse Temurin `21.0.13+9-LTS`,
  `JAVA_HOME=/home/vdrake6/.sdkman/candidates/java/current`; the test reads
  `jmods` directly, so a `java` executable on `PATH` is not required.
  The used `jmods/java.base.jmod` SHA-256 is
  `fad51abeafe6a1ecb0c2696a88684dc417cc534b74b5d865042df7381d9d0e91`.
- The test bootstraps one canonical `SemanticStore`/`Definitions`, constructs
  a real `ClasspathSymbolResolver`, and attaches it to a fresh `TastySession`.

## Attempt and result

The test enters the unchanged TASTy file and locates the class and constructor
from the structural address index. It completes class `TYPEDEF@4`, verifies
the sole constructor remains `Missing`, and explicitly calls
`complete_symbol(21)` for the original constructor `DEFDEF`.

Result: **SUPPORTED**. Both independent fresh sessions produce the same
normalized graph:

```text
class=me.cytrowski.tastyfixtures.visibility.Open|owner=me.cytrowski.tastyfixtures.visibility.Open|kind=Constructor|visibility=Public|constructor=DEFDEF@21|binder=Method(Plain,params=0)|result=me.cytrowski.tastyfixtures.visibility.Open|parents=java.lang.Object
```

The test also checks the constructor occurs once in the original class scope,
has public visibility and constructor kind, uses the canonical `Open`
identity for its result, has no type-parameter binder, and the class has no
type-parameter declarations. Its parent is the canonical
`Definitions::object_class`. Repeating constructor completion returns the
same `TypeId` and does not grow the TASTy type index.

## Failure atomicity

While the original constructor is still `Missing`, the test calls
`complete_symbols(&[21, u32::MAX])`. The first address performs the actual
constructor completion and allocates its signature; the second returns the
exact typed error `MissingEnteredSymbol { address: 4294967295 }`. The batch
transaction then rolls back the constructor to `Missing` and restores the
TASTy type-index count. It confirms the previously completed class retains
the same symbol and `ClassInfo` identity.

The test retries `complete_symbol(21)` in the same unpickler/session, confirms
the constructor type is rebuilt, then repeats completion and verifies the
same type identity without further type-index allocations. After dropping
the unpickler it checks the original class scope still owns exactly one
constructor. This exercises rollback after real constructor work using the
unchanged fixture and resolver; the deliberate failure is the invalid second
batch address, not a semantic error in `Open`'s constructor projection.

## Reproduction

```sh
CARGO_TARGET_DIR=/path/to/target cargo test -p dotty-classloader --test semantic_tasty_parity frozen_open_constructor_is_explicitly_completed_with_real_dependencies
cargo test -p dotty-tasty-unpickler constructors
cargo test -p dotty-classloader --test tasty_loading
python3 tools/check-frontend-boundaries.py
cargo clippy --workspace --all-targets --locked -- -D warnings
RUSTDOCFLAGS='-D warnings' cargo doc --workspace --no-deps --locked
cargo test --workspace --all-targets --locked
cargo fmt --all -- --check
git diff --check
```

The focused parity test, existing constructor regressions, resolver-backed
classloader tests, frontend boundary check, workspace Clippy, workspace
rustdoc, and locked workspace tests passed on this probe. The test commit is
tests-only; the two historical comparison TSVs and production code are
unchanged.
