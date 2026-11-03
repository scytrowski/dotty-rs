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
- Ordered test classpath: the `tasty_visibility` fixture directory, then
  JDK 21 `jmods` (`JAVA_HOME/jmods`, `java.base.jmod` SHA-256
  `fad51abeafe6a1ecb0c2696a88684dc417cc534b74b5d865042df7381d9d0e91`).
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

## B2 API choices

1. **One explicitly requested binary name (preferred):** add a crate-private
   resolver adapter method that accepts one `BinaryName` and calls the
   semantic loader path. The eventual `resolve_member`/adapter call site
   decides explicitly, before loading starts, that this exact request opts
   in. The method takes the resolver's classpath and session, supplies the
   same `Definitions` and store to one loader, and restores classpath/session
   on both results. It returns a typed semantic loading error with the
   requested name attached. A semantic error is terminal for this selected
   request; there is no legacy retry after a partial attempt.
2. **Resolver-wide allowlist/policy:** store an internal selected-name set
   or mode on the resolver and consult it from the generic class-load route.
   This preserves the public trait but adds policy state, makes opt-in timing
   less local, and risks a selected request being observed through nested
   callbacks. It is larger and less auditable than a one-request operation.

Selection belongs at a single explicit adapter call site, not in
`SymbolResolver`, a public constructor, or a global default. Nonselected
requests continue through `ClassLoader::load_class`. The selected call must
fail with an attributable typed error for unsupported shape, malformed TASTy,
missing resource or dependency; it must not silently fall back. Before
implementation, B2 must settle whether the error is a resolver error variant
or an adapter-local result type and prove it preserves the original
`SemanticTastyLoadError` and binary name.

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

No resolver delegation is demonstrated by B1. The resolver-facing B2 API,
typed error boundary, callback ordering and production behavior remain future
work after this audit merges.
