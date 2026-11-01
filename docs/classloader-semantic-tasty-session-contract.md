# Classloader and semantic TASTy session contract

Status: implemented for bounded package and member resolution between an
active `TastyUnpickler` and `ClasspathSymbolResolver`.

This contract owns live session handoff and its rollback behavior. It does not
claim semantic acceptance for the frozen #1927 cohort or the Open/Animal/Dog
continuation in #1928. See [#1927](https://github.com/scytrowski/dotty-rs/issues/1927),
[#1928](https://github.com/scytrowski/dotty-rs/issues/1928), and the [#1928
ownership spike](https://github.com/scytrowski/dotty-rs/issues/1928#issuecomment-6555146979).

## Frozen inputs and provenance

Checkpoint A was evaluated from `origin/main` at
`6c4ec4609d8d76f30f69cf911caa4d927d00740d`, with the repository-pinned Rust
toolchain `1.98.1` (`rustc 1.98.1`, `cargo 1.98.1`) and Scala 3.9.0 source
revision `777528f19a58e794c9954a42f433373472ec57f8`.

The focused fixtures are unchanged repository artifacts:

| Fixture | SHA-256 | Use |
| --- | --- | --- |
| `crates/dotty-classloader/tests/fixtures/tasty_visibility/me/cytrowski/tastyfixtures/visibility/Open.tasty` | `49286e74a3d3693b08d2a29575f23089f6be5c486231390059f43c19a8929894` | Existing package and owner identity in resolver-first and unpickler-first order |
| `crates/dotty-tasty-unpickler/tests/fixtures/semantic/CompletionHolder.tasty` | `0c6c4a02f3ce54a752c7995cc62fd7d6c9d0ae5b0c3d169a0bce5669c93d52ed` | Active package/member callbacks and a late failure followed by retry |
| `crates/dotty-tasty/tests/fixtures/scala3-library/scala/collection/immutable/List.tasty` | `d5fa585bd0f7ecf28624532a7d8b4b3db4faee84d064022bf228955142e581c3` | Real classpath-backed package evidence for `scala` |
| `crates/dotty-tasty-unpickler/tests/fixtures/semantic/Distinct.tasty` | `58f40ea8596794b54c1516fc4f7975f65c7440b56d67adc67408b32975f14da8` | Existing independent address-indexed package and owner fixture |
| `crates/dotty-classloader/tests/fixtures/enclosing_generics/Outer.class` | `ab0681cadf8e5ba1b0f088817ff28035d383919ef8cb8207f731a0a644207398` | Live generic classfile materialization, metadata/origin/type parameters, and late rollback/retry |

The original #1927 cohort and its result files were not modified. The #1928
Open/Animal/Dog candidates remain unproven; the spike did not execute them
through a resolver-backed semantic load.

## Ownership decision

`TastySession` owns the one mutable `Packages` registry during a semantic
unpickling session. It also retains the resolver object across
`with_session` / `into_session_parts`, so `LoadingSession` identity caches,
module links, metadata, origins, ordered class type parameters, and undo
journals remain alive. The resolver temporarily receives the caller's
`Packages` value for one callback and returns that same value before the
unpickler continues.

The API delta is limited to defaulted package-aware methods on the core
`SymbolResolver` port (`has_package_root`, `take_packages`, and package-aware
checkpoint, rollback, member, and package operations); existing resolver
implementations keep their current behavior through defaults. The classloader
overrides those methods and adds a registry transfer that leaves its positive
`LoadingSession` fields intact. `TastySession` can be seeded from `Packages`
and retain a resolver; `TastyUnpickler::with_resolver` is fallible so two
separately entered package roots are rejected before attachment.

```text
Before callback
  TastyUnpickler/TastySession: Packages P, index, owner scopes
  ClasspathSymbolResolver:     LoadingSession caches/journals, empty package slot
  SemanticStore:               shared mutable arena state

During callback
  TastyUnpickler:               exclusively lends P
  ClasspathSymbolResolver:     temporarily owns P + retains other session state
  SemanticStore:               resolver may materialize symbols/scopes

After callback or typed error
  TastyUnpickler/TastySession: owns returned P
  ClasspathSymbolResolver:     owns its positive caches/journals; package slot empty
  SemanticStore:               committed callback state, or restored checkpoint
```

An adapter that already entered packages first transfers its registry with
`SymbolResolver::take_packages` into `TastySession::with_packages`; the
resolver keeps the rest of its `LoadingSession`. If the TASTy session already
owns a root and the incoming resolver also owns a root, attachment fails with
`TastySessionError::ConflictingPackageAuthorities`. The session never guesses
whether equal-looking paths name the same symbols. Begin the combined session
before either side independently enters a package, or transfer the first
authority before the next adapter starts. A rejected attachment returns the
unchanged session/unpickler and incoming resolver together, so the caller can
inspect or retry without losing either side's committed state.

### Alternatives considered

- **Scoped package handoff for every callback, with package-aware rollback:**
  selected. Rust moves the registry between exclusive owners; resolver journals
  stay in `LoadingSession`. The store, registry, index, and resolver can be
  restored in a defined order without aliasing.
- **Shared `Rc<RefCell<_>>` / `Arc<Mutex<_>>` session handle:** rejected. It
  would add runtime borrow/reentrancy rules and a second mutation protocol to
  concepts already passed by value. The repository forbids using shared
  interior mutability merely to satisfy borrowing.
- **Independent `Packages::snapshot()` values:** rejected for mutation. The
  core API documents that later entries and rollbacks diverge and can allocate
  duplicate package identities.
- **Consuming `LoadingSession::into_packages()` in the integration:** rejected.
  It drops positive class and module identity caches and their metadata/origin
  sidecars. The new `take_packages()` transfers only package ownership and
  leaves those resolver-owned fields in place.

## Callback and rollback order

Each public unpickler call records the store checkpoint, resolver checkpoint,
package mark, and its existing type/index/scope journals. The classpath
resolver's package-aware callback temporarily swaps in the canonical registry;
its own inner transaction marks that registry before materializing classes or
packages. A successful callback returns the registry and appends resolver
undo state. A callback error restores the resolver's inner transaction before
returning.

If a later unpickler step fails after a successful callback, rollback proceeds
in this order:

1. Restore existing symbol-info and annotation mutations in the unpickler.
2. Lend the canonical registry back to the resolver and restore resolver
   caches, class-entry journals, module metadata, origins, and package links
   to the resolver checkpoint.
3. Roll the package registry back to the enclosing unpickler mark, removing
   any package entries produced by other package-aware resolver
   implementations.
4. Truncate the semantic store arenas, then restore the unpickler's address
   indexes and type caches.

Resolver package-scope links are removed before store scopes are truncated.
Committed work before the call remains reachable, and the same unpickler and
resolver can retry.

The live classfile regression injects its failure after the resolver has
loaded `enclosing_generics.Outer` from the checked-in classpath fixture. It
checks that rollback removes the class's identity, package declaration,
metadata, origin, and type-parameter cache entries while preserving symbols
committed by the earlier `enter_symbols` call. Retry loads a valid `Outer`
again; after session handoff, the next unpickler operation confirms the
resolver still retains that class's identity and positive metadata state.

## Supported scope and first-failure matrix

Supported: package and member requests already expressed by the core
`SymbolResolver` contract, through a live unpickler sharing its canonical
store, once-bootstrapped `Definitions`, and one package registry. Resolver
classpaths remain owned by `dotty-classloader`; `dotty-core` only carries the
format-neutral package-aware callback and checkpoint contracts. Legacy
`ClassLoader::load_class` and `tasty_symbol` paths remain the defaults.

| Condition | Result | State guarantee |
| --- | --- | --- |
| Existing package from either adapter | Same `SymbolId` and `ScopeId`; namespace `Term`; original owner chain and declaration scope | One registry records the identity |
| Resolver finds a new real classpath package during unpickling | Package is visible through the returned registry and later calls | Resolver caches/journals remain owned by the resolver |
| Resolver returns no package/member | Existing typed unresolved result | No registry or store mutation |
| Resolver reports malformed/ambiguous owner or namespace | Existing typed resolver failure | Callback and outer transaction restore touched state |
| Error after package/class materialization | Typed error is preserved | Resolver state and package declarations roll back before store truncation |
| Retry in the same session | Canonical package/member IDs are reused on success | Earlier committed symbols and scopes remain usable |
| Both adapters independently entered a root before attachment | `TastySessionError::ConflictingPackageAuthorities` | No identity is selected or reconstructed |
| Foreign-store package IDs | Unsupported by the `Packages` contract | Caller must not transfer IDs between stores |

Semantic shape support remains unchanged. This contract does not add resolver
support for external aliases, annotations, inherited members, `Open.tasty`,
or the Animal/Dog graph. It does not change the frozen #1927 denominator or
outcomes.
