# InfoBase / InfoWrapped pair resolver audit (V/A1)

This is an evidence and contract audit for #2202. It adds no production
behavior, changes no default loader policy, and does not implement V/B1, V/B2,
or V/B3.

## Frozen inputs and reproduction

- Tested repository commit: `39828fa7eb6916845a01395e761089fcd6f21854`.
- Scala source compatibility: Scala 3.9.0, source revision
  `777528f19a58e794c9954a42f433373472ec57f8`; TASTy 28.9.
- Original positive inputs (unchanged):

  | Unit | Checked-in path | SHA-256 | Original indexed entries |
  | --- | --- | --- | --- |
  | `InfoBase` | `crates/dotty-tasty-unpickler/tests/fixtures/semantic/InfoBase.tasty` | `e4d58a733e8cc9a516b5d248dd6fb97acb2cb19d763e1e390cfcd1249bd7f5f9` | `TYPEDEF@4`, constructor `DEFDEF@28`; `InfoBase[A]` is a trait, with no ordinary members |
  | `InfoWrapped` | `crates/dotty-tasty-unpickler/tests/fixtures/semantic/InfoWrapped.tasty` | `07cca416641dda27a333df99603bf7b119d0bc5660098c5d378a09064e1630fd` | `TYPEDEF@5`, constructor `DEFDEF@80`, direct `m` at `DEFDEF@115`; `InfoWrapped[F[_]]` is a class |

  The checked-in original-index ledger in
  [`classloader-semantic-tasty-convergence-3.9.0.md`](../classloader-semantic-tasty-convergence-3.9.0.md#issue-1929-fixture-ledger)
  records the indexed declarations and the applied edge. The focused graph
  regression resolves those addresses from each fixture's retained index; it
  does not compare arena IDs between stores.
- Local runtime: Eclipse Temurin OpenJDK `21.0.13+9-LTS`;
  `JAVA_HOME/jmods`, with `java.base.jmod` SHA-256
  `fad51abeafe6a1ecb0c2696a88684dc417cc534b74b5d865042df7381d9d0e91`.
- Pair test classpath order: the two exact checked-in TASTy byte arrays in the
  pair fixture entry, then the real JDK 21 jmods entry. The pair fixture entry
  tags its selected bytes `Tasty` with the test origin `<memory>`; the second
  entry supplies canonical JVM dependencies, including `java/lang/Object`.
- Selected-resolver probe classpath order: the two checked-in fixture files
  selected by binary name from `dotty-tasty-unpickler/tests/fixtures/semantic`,
  then real JDK 21 jmods. Its fixture resource origin is the corresponding
  fixture directory root; JVM dependencies retain their Jmod origin. The bytes
  come from the named checked-in files. No generated or rewritten positive
  fixtures are used.

With `JAVA_HOME` set to a JDK 21 installation and `TMPDIR` set to a writable
temporary directory, reproduce the pair success and selected resolver outcomes
with:

```sh
cargo test -p dotty-classloader --lib info_pair_semantic_adapter_enters_before_completion_and_reuses_identity_in_both_orders --locked -- --nocapture --test-threads=1
cargo test -p dotty-classloader --lib info_pair_selected_resolver_records_first_typed_outcome_per_root --locked -- --nocapture --test-threads=1
```

On this checkout, both commands passed. The pair test creates a new
`SemanticStore` and loader for each order, `InfoBase` first and `InfoWrapped`
first. It repeats both loads in each store and compares normalized graphs, not
numeric IDs across stores. The selected-resolver probe also uses a fresh store,
package registry, resolver, and request independently for each root.

## Measured behavior

### Existing pair operation

`ClassLoader::load_info_pair_with_tasty_unpickler` succeeds for both requested
roots and both request orders. The two fresh-store runs are semantically equal:

- one canonical parent identity is shared by the `InfoWrapped -> InfoBase`
  edge;
- `InfoWrapped` directly declares `m`, whose completed type is `F[Int]`;
- the applied parent is `InfoBase[F[Int]]`;
- both package entries are unique and both declarations retain their
  constructor/type-parameter shapes (`InfoBase[A]` and `InfoWrapped[F[_]]`);
- repeated loads return the same identity within each store.

The measured graph is bounded to those two units and the canonical JDK/builtin
references. It does not show general graph traversal or inherited-member
lookup.

### Existing selected single-root resolver operation

The crate-private selected operation is
`resolve_member_with_semantic_class`; the test calls its selected-request
transaction path directly so it can preserve and assert the internal typed
cause before the public `ResolutionError::Malformed` conversion.

| Explicit selected type request | First typed failure | Meaning |
| --- | --- | --- |
| `InfoWrapped` | `UnsupportedShape("the class template must have no type parameters and one constructor declaration")` | The single-root semantic adapter rejects the generic class shape before it can project the original pair graph. |
| `InfoBase` | `UnsupportedShape("the requested top-level declaration must have a class template")` | The single-root adapter expects a class template and does not accept this trait root. |

These occur before a missing-input or preexisting-publication check: both
selected `.tasty` resources are present and selected from the first classpath
entry; each probe starts with an empty class identity cache and an empty fixture
package scope. They are explicit unsupported-shape errors, not trait identity
predicate failures, legacy fallback, classpath misses, or evidence of a
transaction defect. The public method wraps the same typed cause in
`ResolutionError::Malformed` after the transaction has rolled back.

For each failure, the observed store and `LoadingSession` checkpoints are
identical before and after the request. The original classpath is recovered and
usable; no selected identity, resolved entry, or origin remains published.
The failure does not reach a successful class/trait identity check, a resolver
negative-member cache entry, or any alternate legacy load. The pair operation's
own regression separately verifies its existing successful two-root
publication.

## Handoff state and rollback surfaces

- **Classpath and session transfer:** `ClasspathSymbolResolver` takes its
  owned classpath and `LoadingSession`, builds a short-lived
  `ClassLoader::with_definitions` over the same store and definitions, then
  recovers the classpath and session from `into_parts` on success and error.
  They must be restored before the resolver returns.
- **Positive session state:** `LoadingSession` owns identified/resolved class
  IDs, module identities, packages, metadata, ordered class type parameters,
  origins, and symbol/package undo state. These survive loader handoff and are
  covered by its checkpoint.
- **Package entries:** the `PackageRegistry` journals class-to-package-scope
  links separately from package creation. Rollback removes class links before
  rolling back package/store arenas, so a pair failure must not leave only one
  root visible in an existing package.
- **Loader-local state:** `ClassRepository` and
  `semantic_tasty_loaded` belong to one `ClassLoader`; neither is returned by
  `into_parts`. The pair method inserts both repeat-cache entries only after
  the whole graph succeeds. On pair error it restores pair package insertions,
  session/store changes, builtin alias ownership and canonical `Object`, and
  discards invalid repository symbols. A resolver bridge must account for the
  fact that these caches do not cross the temporary-loader boundary.
- **Resolver transaction:** `transact` couples a store checkpoint with the
  session checkpoint and the prior `Object` symbol, then rolls them back on
  error. Package class-entry journals are part of the session checkpoint.
  The resolver's negative member cache and decoded/validated module caches are
  separate module-specific state; a pair bridge must not record pair misses in
  those caches. Any future pair-specific positive or negative cache needs an
  explicit checkpoint/invalidation rule before being introduced.
- **Origins and borrowed resolver:** selected fixture origins are captured
  from `ClassResource` and stored by canonical symbol ID only after a
  successful operation. The pair's nested resolver borrows the selected
  classpath for indexed TASTy dependencies while the outer resolver has
  temporarily transferred ownership; it must not re-enter the outer mutable
  resolver. A failed pair must not leave origins for either root.

## Minimal future contract: V/B1 -> V/B2 -> V/B3

This audit does not implement these stages; it freezes their smallest safe
boundary.

### V/B1 — one private pair bridge

Add one crate-private `load_semantic_info_pair_for_member` bridge, reached only
from an explicit selected `MemberRequest` for the exact `InfoBase` or
`InfoWrapped` binary name. It delegates entry/completion to the existing pair
loader rather than duplicating its algorithm. The request's `selected_name`
chooses which identity is returned. A successful bridge **may and must publish
both canonical identities**: completing the requested wrapper requires the
parent, and pair success is atomic. It must report both identities through the
session/package/origin surfaces so subsequent selected requests reuse them;
loader-local caches alone cannot serve that purpose after `into_parts`.

### V/B2 — selected identity, errors, and transactions

- Validate `InfoWrapped` as the exact package-owned `SymbolKind::Class` with a
  complete self-consistent `ClassInfo` and declaration scope. Validate
  `InfoBase` separately as the exact package-owned `SymbolKind::Trait` with a
  complete trait-compatible `ClassInfo` and declaration scope. Both checks
  must confirm the corresponding session identity, origin, binary name, and
  unique package type entry. Do not widen the current class-only predicate to
  accept arbitrary traits.
- The pair is opt-in only for those exact selected requests. Preserve the
  first typed selected-unit error (including unit name, selected origin and
  format where available); never retry through `load_class` or convert
  unsupported shape into `Ok(None)`.
- Reject collisions if either canonical identity or either package type name
  is already owned by a different loading path. A selected request must not
  silently claim one member while conflicting with the coupled other root.
- Treat the pair as one outer resolver transaction: on any typed error restore
  both roots, both package entries, all new session identities/origins/type
  parameters, symbol updates, `Object`/builtin aliases, and store allocations.
  Restore the original classpath and `LoadingSession` in all cases. Preserve
  any state that predates the transaction.
- A successful selected request returns its requested class or trait ID while
  retaining both pair identities and their origins as the defined atomic
  effect. Repeat selected requests must reuse the same IDs, even though the
  temporary loader's `semantic_tasty_loaded` cache itself is not transferred.

### V/B3 — regression and default-path boundary

Retain this pair graph and both selected root probes. Add regressions for both
request orders; both identities published and reused; class versus trait
identity checks; preexisting collision for either root; missing, non-TASTy,
malformed, and late second-unit failures; package-entry/origin/cache rollback;
and retry after failure with valid bytes. Assert store/session checkpoints and
preservation of preexisting package/Object state. Keep ordinary `load_class`
and unselected resolver requests on the legacy path.

| Gate | Evidence expected in the future implementation |
| --- | --- |
| Selected class request | `InfoWrapped` class identity and both pair roots complete |
| Selected trait request | `InfoBase` trait identity and both pair roots complete |
| Cross-unit semantics | `InfoWrapped -> InfoBase[F[Int]]`, direct `m: F[Int]`, shared parent ID |
| Atomic publication | Both package entries, identities and origins appear together |
| Collision | Either root's conflicting prior identity rejects before pair mutation |
| Error and rollback | Typed first unit failure, no legacy retry, exact preexisting state restored |
| Retry and determinism | Valid retry succeeds; normalized graph agrees across both orders and fresh stores |
| Compatibility | Default `load_class`, Typer and `dotty-core` unchanged |

## Scope boundary

Both requested selected single-root probes fail on shapes the existing pair
loader already supports. The audit found no separate semantic prerequisite:
the approved future bridge can call the existing pair operation and preserve
the existing error/rollback boundary. No new loader, generic graph traversal,
default cutover, fixture generation, or production change is required for this
issue. Historical TSV reports are unchanged.
