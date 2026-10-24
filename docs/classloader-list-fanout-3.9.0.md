# Scala `List` classloader fanout audit

Status: **unresolved policy violation; no safe fanout reduction identified**.
The 16 completed-class limit remains unchanged and the reproduced maximum is
29. This report does not mark the gate as passing.

## Reproduction

The focused, serial profile uses the pinned Scala checkout at
`777528f19a58e794c9954a42f433373472ec57f8`, its 3.9.0 library artifacts, and
Temurin JDK 21.0.8. It selects only the three library source files in
[`typer-list-fanout-manifest-3.9.0.tsv`](typer-list-fanout-manifest-3.9.0.tsv)
and takes three warm runs. It does not run the full source corpus.
The measurements below used dotty-rs source revision
`616633b7e3e4b103dbaffd6a2e576bd92e82f41d` with the local audit telemetry
patches present: recursive enclosing-owner edge accounting and direct module
completion counters.

```sh
SCALA39_ROOT=/path/to/scala-3.9.0 \
JAVA_HOME=/path/to/temurin-21.0.8 \
SCALA39_JDK_RELEASE=21 \
COURSIER_MAVEN_ROOT=/path/to/pinned-maven-cache \
python3 tools/typer-classpath-corpus-audit/sequential_profile.py \
  --manifest docs/typer-list-fanout-manifest-3.9.0.tsv \
  --runs 3 --mode full --output-dir target/typer-list-fanout
python3 tools/typer-classpath-corpus-audit/check_list_fanout_profile.py \
  target/typer-list-fanout
```

The checker requires the exact four stable method trees and verifies their
metrics, edge graphs, and class sets across all three runs. It rejects a
missing root, a changed graph, or altered fanout. The raw repeated-run results
are retained locally under `target/typer-list-fanout`; the compact graph and
identity sets are checked in as
[`classloader-list-fanout-3.9.0-edges.tsv`](classloader-list-fanout-3.9.0-edges.tsv)
and
[`classloader-list-fanout-3.9.0-classes.tsv`](classloader-list-fanout-3.9.0-classes.tsv).

| Scala source root | Binary request | Runs | Completed | Visited | Depth | Hierarchy edges | Member identity attempts / successes | Cache hits | Module completions |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| `ListMap.scala#tree=146::scala.collection.mutable.ListMap.addOne` | `scala/collection/immutable/List` | 3 | 29 | 63 | 7 | 81 | 435 / 300 | 53 | 0 |
| `ListBuffer.scala#tree=1088::scala.collection.mutable.ListBuffer.removeAfter` | `scala/collection/immutable/List` | 3 | 29 | 63 | 7 | 81 | 435 / 300 | 53 | 0 |
| `TrieMap.scala#tree=2721::scala.collection.concurrent.LNode.inserted` | `scala/collection/immutable/List` | 3 | 29 | 63 | 7 | 81 | 435 / 300 | 53 | 0 |
| `TrieMap.scala#tree=2780::scala.collection.concurrent.LNode.removed` | `scala/collection/immutable/List` | 3 | 29 | 63 | 7 | 81 | 435 / 300 | 53 | 0 |

The three complete profile runs took 875, 895, and 884 ms for the selected
three-file audit. Across the same samples, the four exact methods averaged
108.7 ms (`ListMap.addOne`), 109.0 ms (`ListBuffer.removeAfter`), 111.3 ms
(`LNode.inserted`), and 107.3 ms (`LNode.removed`). These are bounded audit
timings, not a full-corpus performance estimate. The source method outcome
sidecar was byte-for-byte identical before and after adding opt-in edge
telemetry.

## What the graph says

The classloader profile separates directed edges by reason. For each root,
the graph has 192 unique directed edges:

- 81 `hierarchy` edges, each requested once;
- 111 `member_type_identity` edges, requested 435 times in total. Successful
  identity establishment accounts for the existing `member_type_references`
  counter of 300; the other attempts do not complete a class.
- no enclosing-owner edges or module-completion requests.

The hierarchy edges are emitted at the shared loader boundary used for the
decoded superclass/interface names. The pinned TASTy graph has 81 parent edges
connecting exactly 29 class identities, and this node set equals the profile's
29 `completed_classes`. The root `List` accounts for one completion; the 81
parent-edge loads account for 28 new parent completions plus 53 completed-cache
hits along shared ancestry. Thus every full completion in these requests is in
the real transitive parent/interface closure of
`scala.collection.immutable.List`; the other visited identities come from
member/signature references. The completed set is not an incidental set of
unrelated member types.

The 63 visited identities divide into 29 fully completed classes, 33 observed
as identity shells, and 6 names whose identity lookup did not establish a
shell. Five identities were first seen as shells and later completed, so the
shell and completion sets overlap. The profile also records 30 shell
creations and 154 shell reuses as events; these counts are not distinct-class
counts. The 53 completion-cache hits are repeated hierarchy visits through
shared ancestors in the parent graph.

## Decision and remaining gate

The loader deliberately completes superclass/interface dependencies while it
constructs `ClassInfo`; member/signature references remain identity-only. The
four traces show that this boundary is working as implemented: the 29 full
completions are the hierarchy closure, while the 300 successful member type
references do not trigger full completion. Deferring only some of those 29
parents would change when canonical parent `ClassInfo` and declaration scopes
become available. A safe change therefore needs a separate lazy hierarchy
contract and proof that ancestry and inherited declaration lookup complete on
demand. This evidence does not establish that such a change is safe, so this
increment does not alter loader semantics or loosen the fanout limit.

The current value **29 exceeds the invariant of 16**. Keep the safety gate
failing and the issue open pending a separately reviewed hierarchy-loading or
metric-policy decision. Do not describe this audit as a fanout fix.

Existing regression coverage exercises the transaction cases required for a
future lazy hierarchy change: TASTy parent failure rollback/retry
(`tasty_parent_failure_rolls_back_loaded_dependencies_and_retry_succeeds`),
real inheritance-cycle rejection
(`resolver_rollback_does_not_cache_a_genuine_inheritance_cycle`), changed
parent resource invalidation
(`changed_module_parent_invalidates_rolled_back_hierarchy_proof`),
pre-existing shell restoration and retry
(`resolver_rollback_restores_a_preexisting_shell_and_allows_retry`), and
identity handoff across loaders/stores
(`a_loading_sessions_negative_result_does_not_leak_into_a_second_loader_with_a_different_classpath`,
`an_identity_shell_survives_session_handoff_and_is_completed_in_place`). A
focused new regression also resolves a completed parent in each of two
independent stores and loading sessions
(`hierarchy_profiles_resolve_parent_identities_in_each_store_and_session`).
