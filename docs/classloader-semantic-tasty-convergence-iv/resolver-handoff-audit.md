# Resolver handoff audit (B1)

This is a tests-and-documentation audit for #2060. It does not change the
default loader, add a resolver policy, or add an operation to `SymbolResolver`.

## Frozen inputs

- Refreshed `main`: `481ddef0527d5a40e065b23102641c265e912dc4` (GitHub
  `main` branch API confirmed this exact commit on 2026-11-03).
- Scala reference: 3.9.0, revision
  `777528f19a58e794c9954a42f433373472ec57f8`; TASTy 28.9.0.
- Positive input: original
  `crates/dotty-classloader/tests/fixtures/tasty_visibility/me/cytrowski/tastyfixtures/visibility/Open.tasty`,
  SHA-256 `49286e74a3d3693b08d2a29575f23089f6be5c486231390059f43c19a8929894`.
  Its checked-in source is
  `crates/dotty-classloader/tests/fixtures/tasty_visibility/Visibility.scala`.
- Ordered test classpath: the `tasty_visibility` fixture directory, then
  JDK 21 `jmods` (`JAVA_HOME/jmods`, `java.base.jmod` SHA-256
  `fad51abeafe6a1ecb0c2696a88684dc417cc534b74b5d865042df7381d9d0e91`).
  The local JDK was Eclipse Temurin `21.0.13+9`.
  Tests bootstrap one `Definitions` value per `SemanticStore`; each fresh run
  gets a fresh store and `LoadingSession`.

## Existing ownership and transaction boundary

`ClasspathSymbolResolver::load_class` takes its `Option<E>` classpath and
`LoadingSession`, constructs a short-lived `ClassLoader::with_definitions`
with the resolver's same `Definitions` and borrowed `SemanticStore`, calls
the public legacy `ClassLoader::load_class`, then recovers both values from
`ClassLoader::into_parts` before returning either `Ok` or `Err`. The resolver
operation is exclusive while these fields are temporarily absent. Its caller
wraps resolution in `transact`: success retains changed session/store state
and appends an undo record; `Ok(None)` and `Err` restore session journals,
the canonical `Object` symbol and store checkpoint. The classpath itself is
not mutated by loader operations and is reinstalled even when the load
returns an error.

The normal resolver route first selects this legacy operation. It does not
reach `load_semantic_class_with_tasty_unpickler`: that operation is
`pub(crate)` on `ClassLoader`, directly callable from the classloader crate's
own tests, and is not a `SymbolResolver` method. Thus B1 has no resolver
semantic success to fake. Its first pre-change boundary is the resolver's
call to `loader.load_class(name)` in `resolver.rs`, before semantic shape
validation. Tests compare the direct legacy and private semantic controls,
and separately exercise the resolver's actual legacy transfer path.

The private semantic operation uses a loader-local `semantic_tasty_loaded`
repeat cache. A newly constructed loader does not inherit that cache. The
session does carry canonical identified/resolved class identities, package
registry state, origin/metadata sidecars and rollback journals. Repository
entries are also loader-local; the long-lived positive identity is in
`LoadingSession`. On semantic failure the adapter rolls the store/session
back and discards invalidated repository symbols before returning a typed
`SemanticTastyLoadError`; it does not fall back to legacy loading. Normal
legacy loading has its own loader-local repository and its documented failed
load behavior. These distinct cache lifetimes must remain explicit in B2.

Resource selection remains the configured `ClassPathEntry`'s responsibility:
the first matching classpath entry wins, while a TASTy resource is preferred
to a class file within an entry. `ClassResource::origin` is captured only
after lookup. Semantic `Open` uses the fixture directory origin; its required
`java/lang/Object` dependency comes from the JDK entry. Semantic loading
creates a nested resolver over the borrowed classpath for TASTy references;
the outer resolver's mutable classpath/session ownership is not re-entered
through that callback. B2 must preserve this non-reentrant ownership shape or
introduce a separately reviewed transaction/callback design.

## Frozen B2 request and error contract

B2 uses one explicitly selected `BinaryName` operation. It does not add a
resolver-wide mode, allowlist, public constructor option, or `SymbolResolver`
trait method. The crate-private adapter boundary is:

```rust
pub(crate) struct SemanticMemberLoadFailure {
    pub(crate) binary_name: BinaryName,
    pub(crate) source: SemanticTastyLoadError,
}

pub(crate) fn load_semantic_class_for_member(
    &mut self,
    store: &mut SemanticStore,
    name: &BinaryName,
) -> Result<SymbolId, SemanticMemberLoadFailure>;
```

`SemanticMemberLoadFailure` keeps the requested name even when the underlying
error describes a dependency (or an `UnsupportedShape` with no name of its
own), and retains the original typed `SemanticTastyLoadError`. The method
takes `self.class_path` and `self.session`, constructs one
`ClassLoader::with_definitions` with the resolver's same `Definitions` and
borrowed store, calls `load_semantic_class_with_tasty_unpickler(name)`, then
restores both values from `into_parts` on either result. No callback may
re-enter the temporarily emptied outer resolver; nested TASTy references use
the semantic loader's borrowed-classpath resolver as in the current control.

The explicit call site is the empty-candidate, package-owned type branch in
`resolve_member_inner`, immediately after deriving and path-validating its
exact `binary_name`. B2's selected request calls
`load_semantic_class_for_member(store, &binary_name)` there; every request not
explicitly routed through that call retains the current legacy
`load_class` path. Selection is made for that one computed name before any
semantic mutation begins. A semantic error is terminal for that request;
there is no retry through the legacy loader.

The selected branch runs inside `resolve_member`'s existing `transact`. B2's
private transaction closure carries a named error through rollback:

```rust
enum MemberResolutionFailure {
    Resolver(ResolutionError),
    SemanticLoad(SemanticMemberLoadFailure),
}

type MemberResolutionResult<T> = Result<T, MemberResolutionFailure>;

fn transact<T, E>(
    &mut self,
    store: &mut SemanticStore,
    operation: impl FnOnce(&mut Self, &mut SemanticStore) -> Result<Option<T>, E>,
) -> Result<Option<T>, E>;
```

`resolve_member_inner` returns `MemberResolutionResult<Option<SymbolId>>` and
maps its existing core errors to `MemberResolutionFailure::Resolver`; the
selected adapter maps its wrapper to `MemberResolutionFailure::SemanticLoad`.
`transact` must therefore be generic over its closure error type, while
retaining its existing behavior: `Ok(Some(_))` commits the session undo mark;
`Ok(None)` and every `Err(_)` restore the session, canonical `Object`, and
store checkpoint. The semantic loader's inner transaction first rolls back
its own partial mutations. The public `SymbolResolver::resolve_member`
boundary still returns `ResolutionError`; its selected-error adapter maps the
wrapper to `ResolutionError::Malformed { reason }`, with `reason` containing
the requested `BinaryName` and the full `SemanticTastyLoadError` debug form.
This is the sole text conversion, after rollback; the typed wrapper is not
discarded or converted into a legacy retry inside the transaction. Existing
`ResolutionError` and the public trait remain unchanged.

For the original fixture and negative controls, B2's exact outcomes are:

| Selected request and classpath | Adapter result before trait-boundary conversion | Required state/result |
|---|---|---|
| Original `Open`, fixture directory followed by JDK 21 `jmods` | `Ok(symbol)` | `symbol` is the canonical `Open`; class kind/owner/package/origin match the B1 receipt, parent is canonical `java.lang.Object`, and semantic constructor result is `Open`. Resolver transaction commits. |
| `Open` name resolves first to a class-file resource, which is unsupported by semantic loading | `Err(SemanticMemberLoadFailure { binary_name: Open, source: UnsupportedShape("the selected classpath resource is not TASTy") })` | No legacy retry; semantic-loader and resolver checkpoints are restored. |
| Truncated bytes selected as `Open.tasty` | `Err(SemanticMemberLoadFailure { binary_name: Open, source: ClassLoad(InvalidTastyFile(Open, _)) })` | No legacy retry; no positive resolved/origin/cache entry remains. |
| Original `Open.tasty` with `java/lang/Object` omitted from the classpath | `Err(SemanticMemberLoadFailure { binary_name: Open, source: UnsupportedShape("the configured classpath cannot resolve java.lang.Object") })` | The error retains the selected root and names the missing required dependency; no legacy retry, and both semantic and outer resolver checkpoints are restored. |

These error shapes use existing `SemanticTastyLoadError` variants exercised by
the loader controls. The `BinaryName` in the wrapper always identifies the
selected root `Open`; the missing-`Object` `UnsupportedShape` reason identifies
the unavailable dependency. Ordinary, nonselected requests continue through
`ClassLoader::load_class` unchanged.

## Evidence and B2 acceptance contract

The focused controls exercise: original `Open` through each existing loader
route in separately allocated stores; owner/package authority, class kind,
visibility, Object parent, constructor owner and result target, resource
origin, and each route's same-loader repeat behavior; repeated fresh-run
normalization; resolver transfer on success and a missing-class error; and a
new-loader same-session collision with rollback and later ordinary lookup.
The constructor result intentionally differs: legacy resolves to canonical
`scala.Unit`, while semantic `Open` uses the class itself. This is retained as
an explicit delta rather than normalized away.

The test checks in both runs' normalized output in
[`resolver-handoff-receipts.tsv`](resolver-handoff-receipts.tsv) and verifies
that file against observations on each execution. Numeric IDs are asserted to
differ between the independent adapters but are not written into or compared
by the normalized receipts.

The same-session policy for B2 is: the first successful selected request
publishes one canonical symbol in `LoadingSession`; another selected semantic
attempt from a different loader over that session returns a typed
`UnsupportedShape` identity-collision error without changing store/session;
an ordinary legacy lookup may reuse the already resolved canonical symbol.
If a selected attempt fails before publication, rollback leaves no positive
resolved/origin entry or stale completed repository entry, and retrying the
same selected request is permitted. Repeats in the *same* loader use its
semantic cache. Negative and `Loading` repository states are never transferred
as session-wide facts.

| Requirement | Evidence |
|---|---|
| Exact real positive and provenance | Frozen hash/classpath above; loader audit test asserts selected bytes and directory origin. |
| Fresh stores and non-comparable IDs | Test runs both adapters in fresh stores, asserts their numeric `SymbolId`s differ, and compares only normalized semantic relationships. |
| Repeat/cache distinctions | Same-loader semantic repeat; new-loader session collision; ordinary legacy reuse. |
| Error restoration | Resolver missing-name failure leaves its classpath and session usable; subsequent ordinary request succeeds. Semantic rollback/collision tests assert exact checkpoint equality and absence/presence of resolved/repository/cache entries. |
| Explicit selection/no fallback | Pre-change boundary evidence: no resolver semantic operation exists; B2 must call one explicit-name method and return its semantic error directly. |
| Package authority and origin | Assert class owner equals canonical session package, Object equals canonical `Definitions::object_class`, and `origin` is the selected resource. |

## Validation record

Executed with Rust `1.98.1 (48a229cea 2026-09-01)`, JDK 21 above,
task-isolated `TMPDIR` and `CARGO_TARGET_DIR`, `CARGO_PROFILE_TEST_DEBUG=0`,
`CARGO_INCREMENTAL=0`, and serial Cargo jobs.

- Passed: `cargo fmt --all -- --check`.
- Passed: `python3 tools/check-frontend-boundaries.py`.
- Passed: `cargo clippy --workspace --all-targets --locked -- -D warnings`.
- Passed: `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked`.
- Passed: `cargo test -p dotty-classloader --locked -j1`, including the new
  resolver handoff, identity collision, rollback and two-run receipt controls.
- Not completed: `DOTTY_CLASSFILE_REQUIRE_JDK=1 cargo test --workspace
  --all-targets --locked -j1` stopped while compiling
  `tests/jvm_stateful_class_contract.rs` with
  `rustc-LLVM ERROR: IO failure on output stream: No space left on device`;
  the shared temporary filesystem reached 100% usage. This result is not a
  pass. The classloader acceptance tests did run with JDK 21 and its
  `java.base.jmod`.
- The issue execution has a canonical trace; its report is finalized separately
  from this audit and is not reproduced here.

No resolver delegation is demonstrated by B1. The resolver-facing B2 API,
typed error boundary, callback ordering and production behavior remain future
work after this audit merges.
