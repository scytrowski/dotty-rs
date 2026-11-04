# Resolver semantic class opt-in

Issue #2064 adds one crate-private request path for the original Scala 3.9
`Open.tasty` fixture. The request is explicit at the member-resolution call
site and carries the exact `BinaryName`; ordinary `SymbolResolver` calls still
use `ClassLoader::load_class` and its existing TASTy-first classpath behavior.
The public `SymbolResolver` trait and default routing are unchanged.

The selected path moves the resolver's existing classpath and `LoadingSession`
into one `ClassLoader::with_definitions` instance, passing the same
`Definitions` and borrowed `SemanticStore`. It invokes the existing private
semantic class operation, then restores the classpath and session from
`into_parts` before returning either success or failure. TASTy references use
the semantic loader's borrowed-classpath resolver; they do not re-enter the
temporarily empty outer resolver.

`SemanticMemberLoadFailure` retains both the requested root name and the
original `SemanticTastyLoadError`. The member transaction accepts that typed
error, rolls back its session journals, canonical `Object`, and store
checkpoint, and only then converts it to the existing public
`ResolutionError::Malformed` representation. A selected semantic failure is
terminal for that request and never retries through the legacy loader.

Repeated selected `Open` requests reuse the existing canonical symbol only
when its complete class info contains the zero-argument constructor whose
result is that same class. A preexisting legacy `Open` has the observed
`Unit` constructor result and is rejected with a typed identity-collision
error before semantic mutation. An ordinary resolver request after semantic
success reuses the canonical identity from the shared package scope and
session.

The unpickler enters the semantic root directly into the canonical package
scope. On successful semantic loading, the classloader adopts that existing
scope link into `PackageRegistry`'s class-entry journal. This lets an enclosing
resolver checkpoint remove the link before the caller truncates store arenas;
inner semantic failures still remove the root link in the semantic loader's
own transaction.

The positive fixture is
`crates/dotty-classloader/tests/fixtures/tasty_visibility/me/cytrowski/tastyfixtures/visibility/Open.tasty`
with SHA-256
`49286e74a3d3693b08d2a29575f23089f6be5c486231390059f43c19a8929894`.
The fixture classpath entry precedes JDK 21 `jmods`; Scala reference sources
are pinned to revision `777528f19a58e794c9954a42f433373472ec57f8`.
