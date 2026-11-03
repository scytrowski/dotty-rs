# One plain Int constructor formal in the private semantic TASTy loader (#2028)

The crate-private `load_semantic_class_with_tasty_unpickler` path now accepts
the pinned Scala 3.9.0 `CtorPlain(x: Int)` unit in addition to its existing
zero-argument `Open` and `PlainPrivate` shapes. It still requires one
top-level class with the canonical Object parent and exactly one primary
constructor. The supported parameter is one ordinary term formal whose class
template copy is `PRIVATE+LOCAL`; the constructor-owned formal has no
additional modifiers, and its completed type resolves to the canonical
`Definitions::int` identity. The indexed constructor parameter remains owned
by the original constructor symbol. The constructor signature is a plain
one-parameter method returning the same class symbol.

The positive input is the unchanged
[`CtorPlain.tasty`](../../crates/dotty-tasty-unpickler/tests/fixtures/semantic/CtorPlain.tasty)
fixture, compiled by the pinned Scala 3.9.0 source revision
`777528f19a58e794c9954a42f433373472ec57f8`. The classloader test retains its
fixture origin, loads `Open`, `CtorPlain`, and `PlainPrivate` in one store,
checks repeat identity and canonical package/Object/Int relationships, then
compares normalized semantic summaries from two fresh stores.

The implementation is based on integrated `main`
`b79dc945d392a470d42997081602c6f1464fe02a`. The required real-JDK path uses
Temurin `21.0.13+9`; the pinned Scala source checkout is at the revision above.

`CtorWithVal`, `CtorGeneric`, `CtorTwoTermClauses`, and the existing real
secondary-constructor control remain typed unsupported shapes. The supported
slice does not include accessors or fields, generic/contextual/curried
constructors, multiple formals, multiple declarations, or a change to the
public legacy loader. The existing transaction owns the new `Int` binding;
late failure rolls it back while preserving committed classes and allows the
same valid bytes to retry.

Validation passed on this baseline:

```text
cargo +1.98.1 test -p dotty-classloader --locked
cargo +1.98.1 test -p dotty-tasty-unpickler --locked
cargo +1.98.1 fmt --all -- --check
python3 tools/check-frontend-boundaries.py
cargo +1.98.1 clippy --workspace --all-targets --locked -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo +1.98.1 doc --workspace --no-deps --locked
cargo +1.98.1 test --workspace --all-targets --locked
DOTTY_REQUIRE_SCALA39_SECONDARY_CONSTRUCTOR_TEST=1 cargo +1.98.1 test -p dotty-classloader --lib semantic_class_loader_rejects_secondary_constructor_with_rollback --locked -- --nocapture --test-threads=1
git diff --check
```

The test suites ran with the worktree-local temporary/target directories,
Java 21 on `PATH`, and the process file limit raised to 8192 for the classpath
tests.
