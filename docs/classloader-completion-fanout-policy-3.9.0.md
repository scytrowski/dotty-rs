# Classloader completion fanout policy, Scala 3.9.0

Status: **baseline frozen; policy implementation in progress**. This report is
for the four frozen `scala/collection/immutable/List` requests only. The
historical `classloader-list-fanout-3.9.0.md` report remains unchanged.

## Pinned reproduction

The three-run focused profile was measured on dotty-rs source
`e5daf562db7f9a22a19a48264eeb09b2e84c9b22`, Scala source
`777528f19a58e794c9954a42f433373472ec57f8` (3.9.0 artifacts), and Temurin
JDK `21.0.8+9`. Completed Scala identities are present in the pinned
`scala-library-3.9.0.jar`; the two completed `java.*` identities are in the
pinned `java.base.jmod`. The three selected source files produce 11 method
observations per run; the four exact requests below were identical across all
three serial runs.

| Source method | Request | Completed | Visited | Depth | Hierarchy edges | Member identity edges / requests / successes | Cache hits |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| `ListMap.scala#tree=146` (`ListMap.addOne`) | `scala/collection/immutable/List` | 29 | 55 | 7 | 81 | 109 / 509 / 471 | 53 |
| `ListBuffer.scala#tree=1088` (`ListBuffer.removeAfter`) | `scala/collection/immutable/List` | 29 | 55 | 7 | 81 | 109 / 509 / 471 | 53 |
| `TrieMap.scala#tree=2721` (`LNode.inserted`) | `scala/collection/immutable/List` | 29 | 55 | 7 | 81 | 109 / 509 / 471 | 53 |
| `TrieMap.scala#tree=2780` (`LNode.removed`) | `scala/collection/immutable/List` | 29 | 55 | 7 | 81 | 109 / 509 / 471 | 53 |

The three current profile outputs matched for each semantic TSV. SHA-256:

- root counters: `0923b7199d21fcc8ddc716c8de606a97a23dcf6be6c36eaa052c40a8caeaef7d`
- dependency edges: `48530949f7fec3c3ede566c311def4d027896de94cac945a209998b403c20067`
- visited/completed identities: `fd8425ad14f03604366a32c4e30a7f2fccb38b244e4fdf5525630a286a0e5992`

The refreshed compact oracle is split into
[`current-roots`](classloader-list-fanout-3.9.0-current-roots.tsv),
[`current-edges`](classloader-list-fanout-3.9.0-current-edges.tsv), and
[`current-classes`](classloader-list-fanout-3.9.0-current-classes.tsv). Each
contains the same request identities across all three runs.

## Comparison with the frozen historical graph

The old evidence was measured at dotty-rs `616633b7e3e4b103dbaffd6a2e576bd92e82f41d`.
It reported 29 completions, 63 visited names, 81 hierarchy edges, 111 unique
member-identity edges, 435 member-identity requests, 300 successful member
references, and 53 completion-cache hits. The current main reproduces exactly
the same 29 completed binary names and exactly the same 81 directed hierarchy
edges for all four requests. Thus every full completion is still in the
pinned `List` parent/interface closure.

The complete visited/member-identity observations did change on current main:
55 names, 109 unique member-identity edges, 509 requests, and 471 successful
member references. This is not attributed to the fanout-policy work. Later
Scala type-projection changes changed member types referenced from the pinned
library; for example, the current graph records `scala/Any` and `scala/Nothing`
identity requests where the historical graph recorded several concrete
collection/factory type names. The completed class set and hierarchy graph
remain unchanged. The historical report is retained as historical evidence;
policy verification uses the current-main oracle linked above.

The historical checker correctly rejects current run output because its
visited/member counters are frozen to the older source revision. A current-main
checker must compare against the refreshed oracle without rewriting that
historical evidence.

## Safety threat model

The replacement gate must distinguish:

1. a real, acyclic, possibly wide superclass/interface closure;
2. unnecessary eager completion caused by member/signature identity requests;
3. cycles, excessive inheritance depth, retries, rollback, or stale resource
   evidence;
4. pathological breadth caused by malformed or misprojected dependencies;
5. fake or caller-asserted “hierarchy” classifications that do not match the
   pinned classes and their checked-in graph.

A dependency reason alone is not proof. The exception for the four measured
List requests must be tied to exact request roots, completed identities, and
an oracle-verified hierarchy graph. Raw total completions remain visible.

## Candidate policies

- **Raise 16 to a larger scalar threshold:** rejected. It still confuses a
  genuine hierarchy with arbitrary member fanout and has no evidence-backed
  distinction.
- **Use dependency-reason labels alone to classify completions:** rejected.
  A caller-supplied or misprojected `hierarchy` label could hide unrelated
  completion work.
- **Allow a bounded, exact oracle-verified hierarchy exception, with a separate
  general safety budget:** selected for implementation. Only the four pinned
  requests may exceed 16 without additional reviewed evidence. The raw count
  29 and all edges remain visible. Unknown high-fanout requests fail closed.
  The checker must verify the complete class set and graph against the refreshed
  pinned snapshot, reject cycles and any completed identity outside the
  hierarchy closure, and enforce independent depth/identity/edge budgets.

This decision does not change eager hierarchy semantics, class publication,
cache behavior, or the 29 raw completions. The implementation work and tests
will be recorded below before this report is marked resolved.

## Reproduction

```sh
SCALA39_ROOT=/path/to/pinned/scala-3.9.0 \
JAVA_HOME=/path/to/temurin-21.0.8 \
SCALA39_JDK_RELEASE=21 \
COURSIER_MAVEN_ROOT=/path/to/pinned-maven-cache \
TMPDIR="$PWD/target/tmp" CARGO_TARGET_DIR="$PWD/target" \
CARGO_BUILD_JOBS=1 RUST_TEST_THREADS=1 \
python3 tools/typer-classpath-corpus-audit/sequential_profile.py \
  --manifest docs/typer-list-fanout-manifest-3.9.0.tsv \
  --runs 3 --mode full --output-dir target/typer-list-fanout-current
```

This reproducer does not run the 1,236-file source corpus.
