# Semantic TASTy convergence II: bounded single-class audit

Status: completed for the two frozen inputs; the final CI gate remains.

## Frozen inputs and environment

This audit is pinned to fresh `origin/main` commit
`53b55d2126934da5f2d2ca0b4a0bea9e67f15bc6`, Rust `1.98.1` (the repository
toolchain), and Temurin JDK `21.0.13+9` at
`/home/vdrake6/.sdkman/candidates/java/current`. Scala fixture semantics are
pinned to Scala 3.9.0 revision
`777528f19a58e794c9954a42f433373472ec57f8`. The two preselected inputs, exact
binary names, source declarations/visibility, TASTy versions, and SHA-256
digests are frozen in [the two-row manifest](classloader-semantic-tasty-convergence-ii-inputs.tsv).
The manifest was committed before any semantic measurements.

The semantic tests use one ordered classpath, matching
`semantic_tasty_parity.rs`: `crates/dotty-classloader/tests/fixtures/tasty_sample`,
`crates/dotty-classloader/tests/fixtures/tasty_fields`,
`crates/dotty-classloader/tests/fixtures/tasty_visibility`,
`crates/dotty-tasty/tests/fixtures/scala3-library`,
`crates/dotty-tasty/tests/fixtures/path_dependent`, then
`$JAVA_HOME/jmods` (JDK 21). Fresh audit runs set `TMPDIR` and
`java.io.tmpdir` to a private directory under the isolated issue worktree;
the host `/tmp` is full and is not used.

Historical comparison artifacts are treated as immutable evidence. Their
pre-measurement hashes are:

| Artifact | SHA-256 |
| --- | --- |
| `docs/classloader-semantic-tasty-convergence/cohort.tsv` | `bfa365d830a627ce46bb5682a93cad90e26dbc9a90ead76bcaa65a48539a8a95` |
| `docs/classloader-semantic-tasty-convergence/comparison.tsv` | `34097e61b691ff6ee9c16089336c495decc31241a10e448c1a1776849888aa4e` |
| `docs/classloader-semantic-tasty-convergence/comparison-1948.tsv` | `a4b14bdf2ba921363358f568b8bce636e0e24453c2a3c6e57bd61478c231e6e0` |

The earlier 14-unit / 126-row denominator and all its outputs remain out of
scope for regeneration. The two selected positive fixtures are unchanged; a
single existing cohort fixture is used only for the bounded constructor probe.
Outcomes are normalized by semantic role and qualified names, never by numeric
`SymbolId` or `TypeId`; within each store, canonical symbol references,
owner/visibility/flags, parent and signatures, constructor kind/owner/result,
and origin are checked explicitly. The semantic constructor parent is compared
to `Definitions::object_class`, its result to the exact entered class
`SymbolId`, and the legacy constructor result to its canonical
`Definitions::unit` `TypeId`. Serialized constructor result and effective
constructor result remain separate observations.

## Audit procedure

The regression in `crates/dotty-classloader/tests/semantic_tasty_parity.rs`
discovers the class `TYPEDEF` and primary constructor `DEFDEF` structurally in
each original TASTy tree, records addresses as fixture evidence only, and
explicitly completes both nodes through the existing resolver-backed
`TastySession`. Each run uses a new `SemanticStore`, exactly one
`Definitions::bootstrap`, and the frozen classpath. It repeats this for two
independent sessions per class and compares normalized receipts byte for byte.
It separately invokes legacy `ClassLoader::load_class` in fresh stores and
checks repeatability. The compact per-fixture receipt is in [the result TSV]
(classloader-semantic-tasty-convergence-ii-results.tsv).

Both unpickler runs complete the class and constructor. For both fixtures the
original constructor `DEFDEF@21` has one empty clause and serializes its result
as `scala.Unit`; effective semantic completion gives a plain zero-argument
constructor whose result is its owning class. The legacy loader reports
`()Plain:Unit`. This is an observed constructor-signature semantic delta, not a
load failure. `PlainPrivate` is represented as
`PrivateWithin(me.cytrowski.tastyfixtures.visibility)`, matching its Scala
`private` source declaration under Scala 3.9.0 semantics.

The legacy loader loads both classes from the frozen visibility fixture
directory, assigns public visibility to both constructors, and completes
`java.lang.Object` as the sole parent. The unpickler produces the same class
identity role, class visibility, constructor owner/kind/visibility, and parent.
The result does not claim exact signature parity: the constructor result
representation differs as described above.

Negative controls feed each original class through resolver-backed semantic
completion without the JDK modules (leaving the external `java.lang.Object`
dependency unavailable), present the same original bytes under a mismatched
requested package, and truncate each original TASTy by one byte. Missing
Object produces a typed `ResolverFailure` / `UnresolvedPackage`; completion
rolls back the entered class to `Missing`, and the unpickler type count is
unchanged. The requested-owner control compares the entered class path against
the mismatched requested owner and returns a test-local typed
`RequestedOwnerMismatch`; no class completion or partial info is published.
Separate legacy controls still report typed
`NotFound` / `DependencyFailure` and unchanged store checkpoints. The TASTy
parser rejects truncated input. No partial class is accepted and no `Any` or
`Object` fallback is inferred. Existing malformed-input and missing-dependency
regressions remain intact.

## Result

**2/2 frozen classes are eligible for explicit class and primary-constructor
completion** through one narrow shape: a top-level class with one empty
primary-constructor clause, no type parameters or declared members, and a
resolvable `java.lang.Object` parent, with either public visibility or private
visibility qualified to the containing package. This is semantic completion
eligibility, not exact legacy signature parity: both rows retain the reported
`Unit` versus owning-class constructor result delta.

The historical #1927 `<missing>` observation does not establish a constructor
failure because that comparison did not explicitly complete pending
constructors. The bounded neighboring probe explicitly completed the existing
`fields/Body(x: Int)` TASTy with the same resolver/bootstrap strategy:

| Probe | Explicit completion result |
| --- | --- |
| `fields/Body(x: Int)` primary constructor | Supported; one `Int` parameter, owning class result |

Therefore the first unsupported neighboring constructor shape remains
unmeasured. The bounded prerequisite to measure it is a preselected, frozen
constructor whose parameter type has a semantic shape not handled by this
bootstrap plus resolver path (unavailable dependency or unsupported type-tree
shape), followed by explicit `complete_symbol` on that constructor and capture
of its `UnpickleError` or resulting `Type::Error`. The successful `Body(Int)`
probe rules out an ordinary single primitive parameter as that boundary. The
historical `<missing>` row is not used as evidence. The two selected positive
targets and the 14-unit/126-row denominator remain unchanged. No production
gate or reusable loader contract was changed.

Validation ran with the pinned toolchain and isolated temporary/build
directories (`TMPDIR` under the issue worktree, JDK `java.io.tmpdir` under the
same worktree):

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked
cargo test --workspace --all-targets --locked
cargo test -p dotty-classloader --test semantic_tasty_parity --locked
cargo test -p dotty-tasty-unpickler --locked
cargo test -p dotty-classloader --test real_jdk_class_path --locked -- --ignored
git diff --check
```

All commands passed; the explicit real-JDK gate passed its three ignored JDK
21 tests. Final hashes confirm all three historical artifacts above remain
unchanged.

## Structural selection contract (#1990)

This change is based on `origin/main` commit
`bd90c1a82d89f315c7613570d6d5f518e0e76d8b` (including merged prerequisite
#2002). The positive inputs remain the exact frozen fixtures: Open
`49286e74a3d3693b08d2a29575f23089f6be5c486231390059f43c19a8929894` and
PlainPrivate
`db045c091f6bc71cdee1fef6c81cd09fe1f520f740f09fec008dd5f1fd98c4ba`.

The opt-in top-level TASTy adapter now derives its selection receipt directly
from the original decoded unit and its AST address index. The receipt contains
the requested binary name plus the authentic `TYPEDEF` and primary-constructor
wire addresses; it contains no allocated semantic IDs. Preflight requires one
package tree, a declared package and class name matching the requested binary
name, one class template (not an object or trait) without extra declarations,
and exactly one `<init>` definition with one empty parameter clause.
Unsupported structures are rejected before symbol entry. After entry, both original addresses must
resolve in the retained `TastySemanticIndex` to a class and constructor, and
the completed constructor must be owned by that class. The post-entry class
identity is also checked against the requested package path.

The constructor clause preflight requires the literal `EMPTYCLAUSE` marker;
`SPLITCLAUSE` is only a boundary and does not establish an empty parameter
clause. The AST address index must also link the selected class to its template
and the template to its one direct `<init>` stat. A nested or unrelated
constructor elsewhere in the unit cannot satisfy that selection.

The same pinned root and constructor addresses were removed from the
InfoBase/InfoWrapped adapter. That pair validates its package and root kind
from source structure, then discovers each root, constructor and the wrapped
class's sole direct method from the entered index and store-owned identities.
No fixture address is a production selection criterion. The Open and
PlainPrivate fixture bytes and historical cohort artifacts remain unchanged.
Constructor completion remains outside discovery; serialized `Unit` and the
completed owning-class result retain their separate meanings described above.

## Bounded Open semantic-load transaction (#1991)

This contract is implemented from fresh `main` at
`02f666dcf794a24fd4ba3676380444461c8e0f72`, after the #1990 merge. The
existing Open control fixture remains byte-identical at SHA-256
`49286e74a3d3693b08d2a29575f23089f6be5c486231390059f43c19a8929894`.

The opt-in Open adapter now has one private operation boundary after classpath
selection and structural preflight. It owns the store and `LoadingSession`
checkpoints, canonical builtin snapshots, semantic projection, validation,
and publication. The sole supported mutation plan is the existing one-class
Open shape: classpath resolution may add package/class entries and complete
the canonical `Object`; unpickling may enter the selected root and bind the
existing canonical `Unit`. Shape preflight rejects other trees before this
boundary begins. This is not an automatic transaction for arbitrary semantic
store writes or a general multi-unit loader transaction.

On success, the adapter records the selected resource origin and publishes
`resolved`, repository `Loaded`, and semantic-TASTy cache entries only after
class and constructor identity, ownership, and completion checks pass. On
failure, any completed Open root is removed from its surviving package scope,
then `LoadingSession` restores symbol updates, identity/module/resolution and
sidecar journals, and package declarations. The canonical `Object` and
builtin alias snapshots are restored next; only then are store arenas
truncated and repository entries pointing at rolled-back symbols discarded.
No negative classpath result is cached. A retry probes the same configured
classpath resource and may safely materialize it again. Previously committed
symbols, including canonical `Unit` and unrelated package declarations,
remain intact.

The focused negative test injects failure through this operation after the
real Open class and constructor have completed. It asserts store/session
checkpoints and surviving committed declarations are unchanged, confirms the
class has no loaded/resolved cache entry, and retries the original resource in
the same loader. Existing positive-shape and neighboring Info, Option, and
default-loader paths retain their prior contracts; this transaction does not
add any accepted class shape.
